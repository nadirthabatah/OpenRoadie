//! Agent IPC client.
//!
//! The agent owns all device I/O, so the GUI never opens a device — it connects
//! to the agent's local socket and (a) keeps one [`Agent::observe`] request open
//! for the agent's state, and (b) forwards "apply now" / "read" device commands.
//! Both run on one dedicated OS thread with a tokio runtime (the GPUI thread owns
//! no async runtime): results cross back over `mpsc` to the GPUI loop.
//!
//! There is no poll cadence to tune. `observe` carries a generation, and the
//! agent answers the moment its state differs from the one this client last saw,
//! so the GUI is told *when* to look instead of asking on a timer — and because
//! every answer is the complete state, a reconnect needs no resynchronisation:
//! ask again with generation 0 and the next answer is the whole truth.
//!
//! What is left to time is failure. `launch::spawn_agent` brings the agent up
//! when the socket stays down — gated by [`SpawnReflex`], which fires
//! immediately for an agent that was never reachable but gives a lost
//! connection [`SPAWN_AFTER_LOSS`] first (the deliberate quits and the
//! supervised restarts announce themselves within that window) and never
//! fires at a live agent newer than this GUI. A stretch without a
//! usable connection longer than [`UNREACHABLE_AFTER`] is pushed to the GUI as
//! [`GuiUpdate::Unreachable`] so the window can say so instead of waiting
//! forever. A dead agent is noticed the moment the socket closes; a *hung* one
//! is noticed when its hold window passes without an answer.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use roadie_core::config::Lighting;
use roadie_core::hid::{
    DeviceRoute, Dpi, DpiInfo, LightCommand, ReceiverSelector, SmartShiftStatus, WriteError,
};
use roadie_ipc::desk::{
    AudioFailure, AudioInputChange, AudioInterfaceSummary, ControllerSummary, DisplayControl,
    DisplayFailure, DisplayReading, DisplaySettings, DisplaySummary, MacroPadSummary,
    NetworkLightChange, NetworkLightFailure, NetworkLightSummary, StreamDeckChange,
    StreamDeckFailure, StreamDeckSummary,
};
use roadie_ipc::{
    AgentClient, AgentSnapshot, ClientKind, ConfigReloadError, Generation, OBSERVE_HOLD,
    Observation, PROTOCOL_VERSION, PairingCommandError, PairingFailure,
};
use tarpc::context;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

/// Minimum gap between agent-launch attempts while the socket is unreachable.
/// Long enough that a missing or crash-looping binary can't be respawned in a
/// tight loop, short enough that a quit / crashed agent is recovered promptly.
const SPAWN_RETRY_PERIOD: Duration = Duration::from_secs(30);

/// How long a *lost* connection must stay down before the spawn reflex may
/// fire. Every cause of a warm loss has a better first responder — launchd's
/// crash respawn, the agent's self-exec on update, the tray-Quit deep link —
/// and the reflex is the responder of last resort, so it waits them out
/// (~8 reconnect attempts). A connection that never existed has no first
/// responder; the cold path fires on the first failed attempt.
const SPAWN_AFTER_LOSS: Duration = Duration::from_secs(2);

/// How long to wait before retrying a connect that failed. This is a retry
/// cadence, not a poll: once connected, nothing here runs on a timer. Short
/// enough that a just-started agent is picked up immediately.
const RECONNECT_DELAY: Duration = Duration::from_millis(250);

/// How long the client may go without a usable connection before the GUI is
/// told the agent is genuinely unreachable rather than still starting (agent
/// start plus a worst-case first enumeration is ~6 s).
const UNREACHABLE_AFTER: Duration = Duration::from_secs(15);

/// Request deadline for a held `observe`, above the agent's own
/// [`OBSERVE_HOLD`]: tarpc cancels a handler whose deadline passes, so a
/// shorter one would kill the hold instead of waiting it out.
const OBSERVE_DEADLINE: Duration = OBSERVE_HOLD.saturating_add(Duration::from_secs(5));

/// What the client thread tells the GPUI loop.
pub enum GuiUpdate {
    /// The agent's state, as of a generation this client had not seen.
    Snapshot(AgentSnapshot),
    /// No usable connection for [`UNREACHABLE_AFTER`]: the agent is genuinely
    /// unreachable (not just starting up). Sent once per outage; the next
    /// snapshot supersedes it.
    Unreachable,
    /// The agent answered the handshake with a *newer* protocol — the app was
    /// updated on disk while this GUI kept running, and only a relaunch
    /// helps. Sent once per episode.
    OutdatedGui,
    /// Result of an agent-owned standalone-light command. The typed failure
    /// reaches the GPUI state model instead of being reduced to a log line.
    LightCommandResult {
        /// Runtime/config key of the light that issued the command.
        key: String,
        /// Monotonic request id used to ignore stale results.
        request_id: u64,
        /// The control whose write produced this result.
        command: LightCommand,
        /// Agent acceptance or typed device failure.
        result: Result<(), WriteError>,
    },
    /// Whether the agent adopted the config currently on disk.
    ConfigReloadResult(Result<(), ConfigReloadError>),
    /// A pairing command could not be delivered, so no session will ever appear
    /// in the observed state to explain the silence. Reported locally rather
    /// than faked as a session the agent never had.
    PairingUndeliverable(PairingFailure),
}

/// A device command sent from the GPUI thread to the client thread. Reads carry
/// a `oneshot` for the reply; standalone-light writes return a result event so
/// the GUI can surface device failures after an optimistic update.
pub enum Command {
    SetDpi(DeviceRoute, Dpi),
    SetLighting(DeviceRoute, Lighting),
    SetLight(DeviceRoute, LightCommand, String, u64),
    SetLightManualPower(DeviceRoute, bool, String, u64),
    SetSmartShift(DeviceRoute, SmartShiftStatus),
    ReadDpi(DeviceRoute, oneshot::Sender<Result<DpiInfo, WriteError>>),
    ReadSmartShift(
        DeviceRoute,
        oneshot::Sender<Result<SmartShiftStatus, WriteError>>,
    ),
    ReloadConfig,
    /// Ask the agent to fire the macOS Accessibility prompt. The agent owns the
    /// CGEventTap, so the system dialog must name (and authorize) the *agent*
    /// binary, not the GUI — prompting locally would grant the wrong process.
    RequestAccessibilityPrompt,
    /// Pairing (agent-owned, since it opens the receiver): begin a session,
    /// pair a discovered device by address, or cancel. Events stream back via
    /// the separate [`IpcClient::pairing`] long-poll, not these commands.
    StartPairing(ReceiverSelector),
    PairDevice([u8; 6]),
    CancelPairing,
    /// Drain the agent's live event-monitor buffer for the debug Diagnostics
    /// monitor. The first poll enables monitoring agent-side; the agent
    /// auto-disables it once polls stop.
    #[cfg(all(target_os = "macos", debug_assertions))]
    PollEventMonitor(oneshot::Sender<Vec<roadie_ipc::MonitorEvent>>),
    /// Everything on the desk that is not a HID++ peripheral.
    ///
    /// Grouped rather than flattened here because these are one surface: they
    /// share a deadline, they are all on-demand rather than observed, and they
    /// all belong to one panel. Keeping them behind one variant also keeps
    /// [`handle`] a readable dispatch over the rest.
    Desk(DeskCommand),
}

/// One request about the desk.
///
/// Every one carries a reply channel, because none of them is
/// fire-and-forget: a panel showing a device's state has nothing to show until
/// the device answers, and every write reports what the device took rather
/// than what it was asked for.
pub enum DeskCommand {
    ListDisplays(oneshot::Sender<Vec<DisplaySummary>>),
    ReadDisplay(
        String,
        oneshot::Sender<Result<DisplaySettings, DisplayFailure>>,
    ),
    SetDisplay(
        String,
        DisplayControl,
        u16,
        oneshot::Sender<Result<DisplayReading, DisplayFailure>>,
    ),
    ListNetworkLights(oneshot::Sender<Vec<NetworkLightSummary>>),
    SetNetworkLight(
        String,
        NetworkLightChange,
        oneshot::Sender<Result<NetworkLightSummary, NetworkLightFailure>>,
    ),
    ListStreamDecks(oneshot::Sender<Vec<StreamDeckSummary>>),
    SetStreamDeck(
        String,
        StreamDeckChange,
        oneshot::Sender<Result<StreamDeckSummary, StreamDeckFailure>>,
    ),
    ListAudioInterfaces(oneshot::Sender<Vec<AudioInterfaceSummary>>),
    SetAudioInput(
        String,
        u16,
        AudioInputChange,
        oneshot::Sender<Result<AudioInterfaceSummary, AudioFailure>>,
    ),
    ListControllers(oneshot::Sender<Vec<ControllerSummary>>),
    ListMacroPads(oneshot::Sender<Vec<MacroPadSummary>>),
}

/// How long to let a desk command run before giving up on it.
///
/// tarpc's default deadline is ten seconds, which is the right order for a
/// HID++ write and far too short here: finding Elgato lights means listening
/// to multicast for three seconds and then reading each light that answered,
/// and a DDC exchange carries a mandatory wait between its two halves. A
/// deadline that expires mid-scan looks exactly like "you have no monitors",
/// which is the one wrong answer worth spending seconds to avoid.
const DESK_DEADLINE: Duration = Duration::from_secs(45);

/// A context with [`DESK_DEADLINE`] instead of tarpc's default.
fn desk_context() -> context::Context {
    let mut ctx = context::current();
    ctx.deadline = Instant::now() + DESK_DEADLINE;
    ctx
}

/// Handle the GUI holds to talk to the agent: a stream of state updates and a
/// sender for device commands. Pairing progress arrives through the same state
/// updates as everything else.
mod launch;

pub use launch::mark_suite_quitting;
use launch::spawn_agent;

pub struct IpcClient {
    pub updates: mpsc::UnboundedReceiver<GuiUpdate>,
    pub commands: mpsc::UnboundedSender<Command>,
}

/// Spawn the IPC client thread. Returns immediately; the thread connects (and
/// reconnects) on its own.
#[must_use]
pub fn spawn() -> IpcClient {
    let (update_tx, updates) = mpsc::unbounded_channel();
    let (commands, mut cmd_rx) = mpsc::unbounded_channel::<Command>();

    let spawn_result = std::thread::Builder::new()
        .name("roadie-ipc-client".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(error = %e, "tokio runtime init failed; IPC client exiting");
                    return;
                }
            };
            rt.block_on(async move {
                observe_loop(&update_tx, &mut cmd_rx).await;
            });
        });
    if let Err(e) = spawn_result {
        warn!(error = %e, "could not spawn IPC client thread — agent state unavailable");
    }

    IpcClient { updates, commands }
}

/// The state/command loop.
///
/// One `observe` request is kept in flight at all times, carrying the last
/// generation this client saw; the agent answers when its state differs from
/// that, or after its hold window with the same state as a heartbeat. Commands
/// share the connection — tarpc multiplexes requests, and the in-flight poll is
/// held across command handling so a device write never cancels it.
async fn observe_loop(
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
) {
    let mut link: Option<LiveConnection> = None;
    // Connect counter feeding `LiveConnection::id` — never reused, so a
    // result from a replaced connection can always be told apart.
    let mut conn_seq: u64 = 0;
    // The agent is normally started by launchd, but the GUI brings it up when
    // the socket is down (see `launch::spawn_agent`), gated by the reflex.
    let mut reflex = SpawnReflex::new(Instant::now());
    let mut notified_unreachable = false;
    let mut notified_outdated = false;
    let mut inflight: Option<ObserveFuture> = None;
    // A config reload asked for while no agent was connected (see `handle`).
    let mut reload_owed = false;
    let mut retry = ticker(RECONNECT_DELAY);
    loop {
        // Taken for the duration of the select so the completed arm can consume
        // it while the others hand it back untouched.
        let mut pending = inflight.take();
        let woken = tokio::select! {
            (id, observed) = maybe(pending.as_mut()) => Woken::Observed(id, observed),
            cmd = cmd_rx.recv() => Woken::Command(cmd),
            _ = retry.tick(), if pending.is_none() => Woken::Reconnect,
        };
        match woken {
            // The poll answered. `pending` is finished, so it is deliberately
            // not handed back — the arms below arm a successor instead.
            Woken::Observed(id, observed) => match link.as_mut() {
                Some(conn) if conn.id == id => {
                    if let Ok(observation) = observed {
                        reflex.connected();
                        notified_unreachable = false;
                        notified_outdated = false;
                        if observation.generation != conn.seen {
                            conn.seen = observation.generation;
                            let _ = update_tx.send(GuiUpdate::Snapshot(observation.snapshot));
                        }
                        inflight = Some(observe(conn));
                    } else {
                        // The connection dropped (agent self-exec on update,
                        // or a crash). Reconnecting re-reads the whole state,
                        // so nothing is lost.
                        link = None;
                        reflex.lost(Instant::now());
                    }
                }
                // A poll from a connection this loop no longer holds — a
                // command reconnected while it was in flight, or the link is
                // down. Its result, success or failure, says nothing about
                // the live connection, so it must neither advance `seen` nor
                // tear anything down. What it does mean: the live connection
                // (created mid-command with the old poll still occupying the
                // slot) has no observe yet — arm its first one.
                _ => {
                    if let Some(conn) = link.as_ref() {
                        inflight = Some(observe(conn));
                    }
                }
            },
            Woken::Command(None) => break, // GUI dropped the sender → shut down
            Woken::Command(Some(cmd)) => {
                inflight = pending;
                if handle(&mut link, &mut conn_seq, &mut reload_owed, update_tx, cmd)
                    .await
                    .is_err()
                {
                    link = None;
                    reflex.lost(Instant::now());
                }
            }
            Woken::Reconnect => match ensure(&mut link, &mut conn_seq).await {
                Ok(conn) => {
                    reflex.connected();
                    if reload_owed && reload(&conn.client, update_tx).await.is_err() {
                        link = None;
                        reflex.lost(Instant::now());
                    } else {
                        reload_owed = false;
                        inflight = Some(observe(conn));
                    }
                }
                Err(ConnectFailure::Unreachable) => reflex.agent_unreachable(),
                Err(ConnectFailure::NewerAgent) => {
                    reflex.newer_agent_running();
                    if !notified_outdated {
                        notified_outdated = true;
                        let _ = update_tx.send(GuiUpdate::OutdatedGui);
                    }
                }
            },
        }
        let now = Instant::now();
        if let Some(down_at) = reflex.down_since() {
            if !notified_unreachable && now.saturating_duration_since(down_at) >= UNREACHABLE_AFTER
            {
                notified_unreachable = true;
                let _ = update_tx.send(GuiUpdate::Unreachable);
            }
            if reflex.should_fire(now) {
                spawn_agent();
                reflex.fired(now);
            }
        }
    }
}

/// The spawn reflex: what the loop knows about the agent link, and the rule
/// for when `launch::spawn_agent` may fire — all timing, no I/O, driven by an
/// explicit `now` so the tests can pin it.
struct SpawnReflex {
    link: Link,
    /// The last connect attempt found a live agent *newer* than this GUI:
    /// spawning cannot help (kickstart is a no-op on a running service and a
    /// fresh copy exits as a duplicate) — only a GUI relaunch does.
    agent_is_newer: bool,
    last_fired: Option<Instant>,
}

/// The reflex's view of the agent link. `Cold` and `Lost` differ in who else
/// might act: a connection that never existed has no first responder, while
/// every cause of losing one has a better first responder than this GUI.
enum Link {
    /// A usable, version-matched connection exists.
    Connected,
    /// No connection has ever existed — down since process start.
    Cold { since: Instant },
    /// An established connection dropped at `since`: launchd's respawn, the
    /// agent's self-exec, and the tray-Quit deep link all announce
    /// themselves within [`SPAWN_AFTER_LOSS`].
    Lost { since: Instant },
}

impl SpawnReflex {
    fn new(now: Instant) -> Self {
        Self {
            link: Link::Cold { since: now },
            agent_is_newer: false,
            last_fired: None,
        }
    }

    fn connected(&mut self) {
        self.link = Link::Connected;
        self.agent_is_newer = false;
    }

    /// An established connection dropped. A no-op while already down: the
    /// original downtime keeps its start (and `Cold` stays cold — a
    /// connection that came and went inside one command dispatch was never
    /// established from the loop's point of view).
    fn lost(&mut self, now: Instant) {
        if matches!(self.link, Link::Connected) {
            self.link = Link::Lost { since: now };
        }
    }

    fn agent_unreachable(&mut self) {
        self.agent_is_newer = false;
    }

    fn newer_agent_running(&mut self) {
        self.agent_is_newer = true;
    }

    /// When the downtime started, `None` while connected — the unreachable
    /// banner's clock.
    fn down_since(&self) -> Option<Instant> {
        match self.link {
            Link::Connected => None,
            Link::Cold { since } | Link::Lost { since } => Some(since),
        }
    }

    /// The trigger rule: fire immediately while cold, wait out the first
    /// responders after a loss, never at a newer agent, at most once per
    /// [`SPAWN_RETRY_PERIOD`].
    fn should_fire(&self, now: Instant) -> bool {
        if self.agent_is_newer {
            return false;
        }
        let waited = match self.link {
            Link::Connected => return false,
            Link::Cold { .. } => true,
            Link::Lost { since } => now.saturating_duration_since(since) >= SPAWN_AFTER_LOSS,
        };
        waited
            && self
                .last_fired
                .is_none_or(|t| now.saturating_duration_since(t) >= SPAWN_RETRY_PERIOD)
    }

    fn fired(&mut self, now: Instant) {
        self.last_fired = Some(now);
    }
}

/// A usable, declared connection, carrying everything that is true only *of
/// this connection*: the identity that tags its in-flight poll, and the
/// generation ledger — a replacement agent numbers its own generations, so
/// `seen` lives and dies with the connection instead of being reset by
/// discipline at every disconnect site.
struct LiveConnection {
    client: AgentClient,
    /// This connection's slot in the connect sequence. A settled poll tagged
    /// with another id belongs to a connection already gone, and is dropped.
    id: u64,
    /// Latest generation seen on this connection. Starts at 0 — "I have seen
    /// nothing" — so the first answer is the agent's whole state.
    seen: Generation,
}

/// Why [`observe_loop`] woke up. Named so the in-flight poll can be handed back
/// after the select ends rather than mutated from inside a borrowed arm.
enum Woken {
    /// The long-poll answered, or its connection dropped — tagged with the
    /// [`LiveConnection::id`] it was armed on.
    Observed(u64, Result<Observation, ()>),
    /// A device command, or `None` once the GUI drops the sender.
    Command(Option<Command>),
    /// Time to try connecting again.
    Reconnect,
}

/// A long-poll in flight. Boxed because it is stored across loop turns, and it
/// owns a clone of the client so the loop can still replace its own link
/// while the poll is outstanding — which is why the output carries the
/// connection id: the loop must be able to tell whose answer this is.
type ObserveFuture = Pin<Box<dyn Future<Output = (u64, Result<Observation, ()>)> + Send>>;

/// Ask for the next state newer than what this connection has seen.
fn observe(conn: &LiveConnection) -> ObserveFuture {
    let client = conn.client.clone();
    let id = conn.id;
    let seen = conn.seen;
    Box::pin(async move {
        let mut ctx = context::current();
        ctx.deadline = Instant::now() + OBSERVE_DEADLINE;
        let observed = client.observe(ctx, seen).await.map_err(|error| {
            debug!(%error, "observe failed — reconnecting");
        });
        (id, observed)
    })
}

/// Await a future that may not exist, never resolving when there is none. The
/// caller pairs it with a precondition, so "none" is a disabled select arm
/// rather than a stall.
async fn maybe<F: Future>(future: Option<F>) -> F::Output {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

/// A tokio interval that *delays* missed ticks instead of bursting them: while
/// a connection is live this arm is disabled for hours, and a fresh burst of
/// backdated ticks on reconnect would buy nothing.
fn ticker(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval
}

/// Why [`ensure`] couldn't produce a usable client.
enum ConnectFailure {
    /// Socket down, handshake failed, or the agent is *older* than us — in
    /// every case the fix is an agent (re)start, which the spawn retry and
    /// the agent-side takeover drive; keep retrying.
    Unreachable,
    /// The agent is *newer* than us: this GUI process is the stale side and
    /// only a relaunch helps. Surfaced to the user as [`GuiUpdate::OutdatedGui`].
    NewerAgent,
}

/// Ensure a live connection, connecting — and stamping a fresh id — on demand.
async fn ensure<'a>(
    link: &'a mut Option<LiveConnection>,
    conn_seq: &mut u64,
) -> Result<&'a LiveConnection, ConnectFailure> {
    if link.is_none() {
        // The handshake happens before any real RPC: mismatched bincode layouts
        // would otherwise surface only as opaque RpcErrors and a silently empty
        // device list. Refuse with a clear log instead, and report the
        // direction — who is stale decides who must restart.
        let connection = roadie_ipc::client::connect().await.map_err(|error| {
            debug!(%error, "no usable agent");
            ConnectFailure::Unreachable
        })?;
        match connection.version {
            version if version == PROTOCOL_VERSION => {
                // Declare before any other RPC: a dormant agent arms only on
                // this — merely connecting no longer wakes it.
                connection
                    .client
                    .declare_client(context::current(), ClientKind::Gui)
                    .await
                    .map_err(|error| {
                        debug!(%error, "agent dropped during the declare handshake");
                        ConnectFailure::Unreachable
                    })?;
                *conn_seq += 1;
                *link = Some(LiveConnection {
                    client: connection.client,
                    id: *conn_seq,
                    seen: 0,
                });
                debug!("connected to agent IPC socket");
            }
            version if version < PROTOCOL_VERSION => {
                warn!(
                    agent = version,
                    gui = PROTOCOL_VERSION,
                    "agent IPC protocol is older — waiting for the agent to be replaced"
                );
                return Err(ConnectFailure::Unreachable);
            }
            version => {
                warn!(
                    agent = version,
                    gui = PROTOCOL_VERSION,
                    "agent IPC protocol is newer — this GUI needs a relaunch"
                );
                return Err(ConnectFailure::NewerAgent);
            }
        }
    }
    // `link` is `Some` here (just set, or already was); the `None` arm is
    // unreachable but keeps this `expect`-free.
    link.as_ref().ok_or(ConnectFailure::Unreachable)
}

/// Run one device command. `Err` signals a dropped connection so the caller
/// reconnects; the command's own failure is reported back over its oneshot.
///
/// A reload asked for while no agent is connected sets `reload_owed` instead of
/// failing: the file is already written, and the agent this client next
/// reaches may be one that was running all along on the old settings (the GUI
/// writes its first config before its first connect). The debt is paid on that
/// connect, and its result is what the window reports. An agent that never
/// comes is already reported by the unreachable frame.
async fn handle(
    link: &mut Option<LiveConnection>,
    conn_seq: &mut u64,
    reload_owed: &mut bool,
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
    cmd: Command,
) -> Result<(), ()> {
    // keep `link` None on connect failure; that's not a dropped live connection
    let Ok(conn) = ensure(link, conn_seq).await else {
        undeliverable(update_tx, reload_owed, cmd);
        return Ok(());
    };
    let client = &conn.client;
    // This reload covers an owed one; any other command pays the debt first.
    if std::mem::take(reload_owed) && !matches!(cmd, Command::ReloadConfig) {
        reload(client, update_tx).await?;
    }
    let ctx = context::current();
    match cmd {
        Command::SetDpi(route, dpi) => log_apply(client.set_dpi(ctx, route, dpi).await)?,
        Command::SetLighting(route, lighting) => {
            log_apply(client.set_lighting(ctx, route, lighting).await)?;
        }
        Command::SetLight(route, command, key, request_id) => {
            send_light_result(
                update_tx,
                key,
                request_id,
                command,
                client.set_light(ctx, route, command).await,
            )?;
        }
        Command::SetLightManualPower(route, enabled, key, request_id) => {
            send_light_result(
                update_tx,
                key,
                request_id,
                LightCommand::Power(enabled),
                client.set_light_manual_power(ctx, route, enabled).await,
            )?;
        }
        Command::SetSmartShift(route, status) => {
            log_apply(client.set_smartshift(ctx, route, status).await)?;
        }
        Command::ReadDpi(route, reply) => {
            let _ = reply.send(rpc_result(client.read_dpi(ctx, route).await)?);
        }
        Command::ReadSmartShift(route, reply) => {
            let _ = reply.send(rpc_result(client.read_smartshift(ctx, route).await)?);
        }
        Command::ReloadConfig => reload(client, update_tx).await?,
        Command::RequestAccessibilityPrompt => client
            .request_accessibility_prompt(ctx)
            .await
            .map_err(|_| ())?,
        Command::StartPairing(selector) => {
            pairing_command_result(update_tx, client.start_pairing(ctx, selector).await)?;
        }
        Command::PairDevice(address) => {
            pairing_command_result(update_tx, client.pair_device(ctx, address).await)?;
        }
        Command::CancelPairing => {
            pairing_command_result(update_tx, client.cancel_pairing(ctx).await)?;
        }
        #[cfg(all(target_os = "macos", debug_assertions))]
        Command::PollEventMonitor(reply) => {
            let _ = reply.send(rpc_result(client.poll_event_monitor(ctx).await)?);
        }
        Command::Desk(desk) => handle_desk(client, desk).await?,
    }
    Ok(())
}

/// Ask the agent to re-read the config file and report what it said.
///
/// A transport failure is not the agent rejecting the config, but it is still
/// a reload that did not happen — and the file on disk has already changed.
/// Staying silent would leave the window showing the new settings while the
/// agent keeps running the old ones, which is exactly the divergence the
/// window fails closed on.
async fn reload(
    client: &AgentClient,
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
) -> Result<(), ()> {
    match client.reload_config(context::current()).await {
        Ok(result) => {
            let _ = update_tx.send(GuiUpdate::ConfigReloadResult(result));
            Ok(())
        }
        Err(error) => {
            let _ = update_tx.send(GuiUpdate::ConfigReloadResult(Err(ConfigReloadError {
                message: format!("saved, but the agent could not be reached to apply it: {error}"),
            })));
            Err(())
        }
    }
}

/// One desk request, answered.
///
/// Split from [`handle`] so each stays a readable dispatch, and because these
/// share a property the rest do not: every one is on-demand and every one runs
/// on [`desk_context`]'s longer deadline.
async fn handle_desk(client: &AgentClient, cmd: DeskCommand) -> Result<(), ()> {
    match cmd {
        DeskCommand::ListDisplays(reply) => {
            let _ = reply.send(rpc_result(client.list_displays(desk_context()).await)?);
        }
        DeskCommand::ReadDisplay(id, reply) => {
            let _ = reply.send(rpc_result(client.read_display(desk_context(), id).await)?);
        }
        DeskCommand::SetDisplay(id, control, value, reply) => {
            let _ = reply.send(rpc_result(
                client.set_display(desk_context(), id, control, value).await,
            )?);
        }
        DeskCommand::ListNetworkLights(reply) => {
            let _ = reply.send(rpc_result(
                client.list_network_lights(desk_context()).await,
            )?);
        }
        DeskCommand::SetNetworkLight(id, change, reply) => {
            let _ = reply.send(rpc_result(
                client.set_network_light(desk_context(), id, change).await,
            )?);
        }
        DeskCommand::ListStreamDecks(reply) => {
            let _ = reply.send(rpc_result(client.list_stream_decks(desk_context()).await)?);
        }
        DeskCommand::SetStreamDeck(id, change, reply) => {
            let _ = reply.send(rpc_result(
                client.set_stream_deck(desk_context(), id, change).await,
            )?);
        }
        DeskCommand::ListAudioInterfaces(reply) => {
            let _ = reply.send(rpc_result(
                client.list_audio_interfaces(desk_context()).await,
            )?);
        }
        DeskCommand::SetAudioInput(id, input, change, reply) => {
            let _ = reply.send(rpc_result(
                client
                    .set_audio_input(desk_context(), id, input, change)
                    .await,
            )?);
        }
        DeskCommand::ListControllers(reply) => {
            let _ = reply.send(rpc_result(client.list_controllers(desk_context()).await)?);
        }
        DeskCommand::ListMacroPads(reply) => {
            let _ = reply.send(rpc_result(client.list_macro_pads(desk_context()).await)?);
        }
    }
    Ok(())
}

/// An accepted pairing command needs no reply — its progress shows up in the
/// observed state. A *rejected* one never becomes a session, so the refusal is
/// reported here or the window would wait for something that will never come.
fn pairing_command_result(
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
    result: Result<Result<(), PairingCommandError>, tarpc::client::RpcError>,
) -> Result<(), ()> {
    match result.map_err(|_| ())? {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = update_tx.send(GuiUpdate::PairingUndeliverable(PairingFailure::from(error)));
            Ok(())
        }
    }
}

/// A fire-and-forget "apply now": `Err(())` (transport drop) propagates so the
/// caller reconnects; a device-side failure is logged, not surfaced.
fn log_apply(r: Result<Result<(), WriteError>, tarpc::client::RpcError>) -> Result<(), ()> {
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            warn!(error = %e, "agent rejected device command");
            Ok(())
        }
        Err(_) => Err(()),
    }
}

fn send_light_result(
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
    key: String,
    request_id: u64,
    command: LightCommand,
    result: Result<Result<(), WriteError>, tarpc::client::RpcError>,
) -> Result<(), ()> {
    if let Ok(result) = result {
        let _ = update_tx.send(GuiUpdate::LightCommandResult {
            key,
            request_id,
            command,
            result,
        });
        Ok(())
    } else {
        let _ = update_tx.send(GuiUpdate::LightCommandResult {
            key,
            request_id,
            command,
            result: Err(WriteError::AgentUnavailable),
        });
        Err(())
    }
}

/// Unwrap a tarpc transport result: `Err(())` (connection dropped) propagates so
/// the caller reconnects; the inner application `Result` is returned for the reply.
fn rpc_result<T>(r: Result<T, tarpc::client::RpcError>) -> Result<T, ()> {
    r.map_err(|_| ())
}

/// Settle a command no agent could take: a reload is owed to the next
/// connection (see [`handle`]), everything else is answered as disconnected.
fn undeliverable(
    update_tx: &mpsc::UnboundedSender<GuiUpdate>,
    reload_owed: &mut bool,
    cmd: Command,
) {
    if matches!(cmd, Command::ReloadConfig) {
        *reload_owed = true;
    } else {
        reply_disconnected(update_tx, cmd);
    }
}

/// Reply to a read command that the agent is unreachable; writes are
/// fire-and-forget so they have nothing to reply to.
fn reply_disconnected(update_tx: &mpsc::UnboundedSender<GuiUpdate>, cmd: Command) {
    // Transient, not a permanent feature error: the agent is just restarting,
    // so the panel should keep retrying, not latch "unsupported".
    match cmd {
        Command::ReadDpi(_, reply) => {
            let _ = reply.send(Err(WriteError::AgentUnavailable));
        }
        Command::ReadSmartShift(_, reply) => {
            let _ = reply.send(Err(WriteError::AgentUnavailable));
        }
        Command::SetLight(_, command, key, request_id) => {
            let _ = update_tx.send(GuiUpdate::LightCommandResult {
                key,
                request_id,
                command,
                result: Err(WriteError::AgentUnavailable),
            });
        }
        Command::SetLightManualPower(_, enabled, key, request_id) => {
            let _ = update_tx.send(GuiUpdate::LightCommandResult {
                key,
                request_id,
                command: LightCommand::Power(enabled),
                result: Err(WriteError::AgentUnavailable),
            });
        }
        Command::StartPairing(_) | Command::PairDevice(_) => {
            let _ = update_tx.send(GuiUpdate::PairingUndeliverable(
                PairingFailure::AgentRestarted,
            ));
        }
        // A reload is never refused here: `handle` owes it to the next
        // connection instead.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_never_reached_agent_is_spawned_immediately() {
        let t0 = Instant::now();
        let reflex = SpawnReflex::new(t0);
        assert!(reflex.should_fire(t0));
    }

    #[test]
    fn a_lost_connection_waits_out_the_first_responders() {
        // A supervised restart, a self-exec, or the quit deep link announce
        // themselves within the grace window; the reflex must not race them.
        let t0 = Instant::now();
        let mut reflex = SpawnReflex::new(t0);
        reflex.connected();
        assert!(!reflex.should_fire(t0 + Duration::from_secs(120)));
        reflex.lost(t0 + Duration::from_secs(120));
        assert!(!reflex.should_fire(t0 + Duration::from_secs(121)));
        assert!(reflex.should_fire(t0 + Duration::from_secs(120) + SPAWN_AFTER_LOSS));
    }

    #[test]
    fn a_live_newer_agent_is_never_spawned_at() {
        // Kickstart would no-op and a fresh copy exits as a duplicate; only
        // relaunching the GUI helps, so firing is pure churn.
        let t0 = Instant::now();
        let mut reflex = SpawnReflex::new(t0);
        reflex.newer_agent_running();
        assert!(!reflex.should_fire(t0 + Duration::from_secs(120)));
        // The newer agent going away (it was quit or replaced) re-arms the
        // reflex on the next failed attempt.
        reflex.agent_unreachable();
        assert!(reflex.should_fire(t0 + Duration::from_secs(120)));
    }

    #[test]
    fn retries_are_rate_limited() {
        let t0 = Instant::now();
        let mut reflex = SpawnReflex::new(t0);
        reflex.fired(t0);
        assert!(!reflex.should_fire(t0 + Duration::from_secs(29)));
        assert!(reflex.should_fire(t0 + SPAWN_RETRY_PERIOD));
    }

    #[test]
    fn a_reload_with_no_agent_is_owed_rather_than_refused() {
        // The GUI writes its first config before its first connect. Refusing
        // that reload locked the whole window behind a configuration error on
        // every fresh install, though the agent was about to be reached.
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        let mut reload_owed = false;

        undeliverable(&update_tx, &mut reload_owed, Command::ReloadConfig);

        assert!(reload_owed, "the reload must wait for the next connection");
        assert!(
            update_rx.try_recv().is_err(),
            "nothing may be reported as refused"
        );
    }

    #[test]
    fn other_commands_with_no_agent_are_still_answered() {
        let (update_tx, mut update_rx) = mpsc::unbounded_channel();
        let mut reload_owed = false;

        undeliverable(
            &update_tx,
            &mut reload_owed,
            Command::StartPairing(ReceiverSelector::First),
        );

        assert!(!reload_owed, "only a reload is owed");
        assert!(
            matches!(update_rx.try_recv(), Ok(GuiUpdate::PairingUndeliverable(_))),
            "a pairing request must still hear that it went nowhere"
        );
    }
}
