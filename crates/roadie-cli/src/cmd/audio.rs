//! `roadie audio` — Focusrite Scarlett and Vocaster interfaces.
//!
//! The audio itself is standard USB audio class and belongs to the operating
//! system; what this reaches is everything *around* it — preamp gain, mute,
//! and 48 volt phantom power — over the interface's vendor control channel.
//! Recording is never interrupted: only the vendor interface is claimed, and
//! the audio ones are left where the system put them.
//!
//! # Why phantom power is the one thing that asks first
//!
//! Every other change here is reversible by making the opposite change.
//! Switching 48 volts **on** is not, because what it can damage is at the
//! other end of a cable this software cannot see. So it refuses once, says
//! what it would do in a sentence meant to be heard, and names the exact
//! command that goes ahead — options in prose to type back, rather than a
//! list to navigate. Switching it *off* asks nothing: that is how somebody
//! makes the interface safe again, and an obstacle in front of the safe
//! direction is an obstacle in the wrong place.

use std::fmt::Write as _;

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use roadie_focusrite::{Attached, Session, Snapshot, attached};
use roadie_scarlett::risk::{Acknowledged, Risk};

use crate::spoken::counted;

#[derive(Debug, Args)]
pub struct AudioArgs {
    #[command(subcommand)]
    pub cmd: Option<AudioCmd>,
}

#[derive(Debug, Subcommand)]
pub enum AudioCmd {
    /// List every audio interface this build can configure.
    List,
    /// Say what one interface is doing, input by input.
    Status(SelectArgs),
    /// Read or set the preamp gain on one input.
    Gain(GainArgs),
    /// Mute or unmute one input.
    Mute(MuteArgs),
    /// Switch 48 volt phantom power on or off for one input.
    Phantom(PhantomArgs),
    /// On Windows, finish setting up the control channel, once per computer.
    /// Raises one Windows permission prompt. Other systems need nothing.
    Prepare,
}

#[derive(Debug, Args)]
pub struct SelectArgs {
    /// Case-insensitive part of the interface's name or serial number, as
    /// `roadie audio list` says it. Not needed when only one is attached.
    #[arg(long)]
    device: Option<String>,
}

#[derive(Debug, Args)]
pub struct GainArgs {
    #[command(flatten)]
    select: SelectArgs,
    /// Which input, counted from one the way the box labels them.
    input: u16,
    /// The new gain. Left out, the current gain is read.
    value: Option<u8>,
}

#[derive(Debug, Args)]
pub struct MuteArgs {
    #[command(flatten)]
    select: SelectArgs,
    /// Which input, counted from one.
    input: u16,
    /// Whether to mute it. Left out, the current state is read.
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

#[derive(Debug, Args)]
pub struct PhantomArgs {
    #[command(flatten)]
    select: SelectArgs,
    /// Which input, counted from one.
    input: u16,
    /// Whether to switch 48 volt phantom power on. Left out, it is read.
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
    /// Go ahead with switching 48 volts on, having read what it can damage.
    #[arg(long)]
    r#yes: bool,
}

impl AudioCmd {
    /// Dispatch, defaulting to the list.
    ///
    /// # Errors
    ///
    /// Fails when the USB stack cannot be listed, when no interface matches
    /// the selection, or when an interface refuses a change.
    pub fn run(self) -> Result<()> {
        match self {
            Self::List => list(),
            Self::Status(args) => status(&args),
            Self::Gain(args) => gain(&args),
            Self::Mute(args) => mute(&args),
            Self::Phantom(args) => phantom(&args),
            Self::Prepare => prepare(),
        }
    }
}

#[cfg(target_os = "windows")]
fn prepare() -> Result<()> {
    use roadie_focusrite::windows_setup::{Prepared, prepare};

    println!("Windows will ask for permission to change this computer. Accept it to go on.");
    match prepare()? {
        Prepared::Done => {
            println!("Done. The control channel is set up; check it with: roadie audio status");
            Ok(())
        }
        Prepared::Declined => Err(anyhow!(
            "Nothing was changed, because the permission prompt was declined or not answered. \
             Run roadie audio prepare again to retry."
        )),
        Prepared::Failed(count) => Err(anyhow!(
            "{} could not be set up. Unplugging the interface, plugging it back in, and \
             running roadie audio prepare again is the next thing to try.",
            counted(
                usize::try_from(count).unwrap_or(1),
                "interface",
                "interfaces"
            )
        )),
    }
}

#[cfg(not(target_os = "windows"))]
#[expect(clippy::unnecessary_wraps, reason = "matches the Windows signature")]
fn prepare() -> Result<()> {
    println!("Nothing to prepare: only Windows needs this step.");
    Ok(())
}

/// Every interface, whether or not this build can drive it.
fn found() -> Result<Vec<Attached>> {
    let mut usable = Vec::new();
    let mut refused = Vec::new();
    for entry in attached()? {
        match entry {
            Ok(interface) => usable.push(interface),
            Err(why) => refused.push(why.to_string()),
        }
    }
    for why in &refused {
        println!("{why}");
    }
    Ok(usable)
}

fn list() -> Result<()> {
    let interfaces = found()?;
    if interfaces.is_empty() {
        println!(
            "No Focusrite interface found. It has to be plugged in with a cable that carries \
             data, not only power."
        );
        return Ok(());
    }
    print!("{}", attached_line(interfaces.len()));
    for interface in &interfaces {
        println!("  {}", interface.describe());
    }
    println!("Say what one is doing with: roadie audio status");
    Ok(())
}

/// The sentence counting what was found.
///
/// Its own function because the count and the verb have to agree — "1 audio
/// interface is" and "2 audio interfaces are" — and that is a rule with a
/// test rather than an intention. `counted` supplies the number, so the
/// phrases here must not repeat it.
#[must_use]
pub fn attached_line(count: usize) -> String {
    format!(
        "{} attached.\n",
        counted(count, "audio interface is", "audio interfaces are")
    )
}

/// The one interface to act on.
///
/// Ambiguity is an error rather than a guess: changing the gain on the wrong
/// interface is not something somebody working by ear would notice.
fn select(args: &SelectArgs) -> Result<Attached> {
    let interfaces = found()?;
    let Some(query) = args.device.as_deref() else {
        return match interfaces.len() {
            1 => interfaces
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("unreachable: one interface")),
            0 => Err(anyhow!(
                "No Focusrite interface found. Run roadie audio list, which says what was \
                 looked for."
            )),
            many => Err(anyhow!(
                "{} attached, so which one has to be said: add --device and part of a name. \
                 Run roadie audio list to see them.",
                counted(many, "interface is", "interfaces are")
            )),
        };
    };
    let wanted = query.to_lowercase();
    let mut matched: Vec<Attached> = interfaces
        .into_iter()
        .filter(|interface| {
            interface.name.to_lowercase().contains(&wanted)
                || interface
                    .serial_number
                    .as_deref()
                    .is_some_and(|serial| serial.to_lowercase().contains(&wanted))
        })
        .collect();
    match matched.len() {
        1 => Ok(matched.remove(0)),
        0 => Err(anyhow!(
            "No interface's name or serial number contains {query:?}. Run roadie audio list \
             to see them."
        )),
        _ => Err(anyhow!(
            "{query:?} matches more than one interface: {}. Use more of a name.",
            matched
                .iter()
                .map(Attached::describe)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn open(args: &SelectArgs) -> Result<(Attached, Session)> {
    let interface = select(args)?;
    let session = Session::open(&interface)?;
    Ok((interface, session))
}

fn status(args: &SelectArgs) -> Result<()> {
    let (_, mut session) = open(args)?;
    let snapshot = session.snapshot()?;
    print!("{}", describe_snapshot(&snapshot));
    Ok(())
}

/// What an interface is doing, written to be read aloud.
///
/// A free function over the snapshot rather than a method on the session, so
/// the strings that ship are the strings the tests sweep.
#[must_use]
pub fn describe_snapshot(snapshot: &Snapshot) -> String {
    let mut said = format!(
        "{}, firmware version {}.\n",
        snapshot.model, snapshot.firmware
    );
    for input in &snapshot.inputs {
        let mut parts = Vec::new();
        if let Some(gain) = input.gain {
            parts.push(format!("gain {gain}"));
        }
        if let Some(muted) = input.muted {
            parts.push(if muted {
                "muted".to_owned()
            } else {
                "not muted".to_owned()
            });
        }
        if let Some(phantom) = input.phantom {
            parts.push(if phantom {
                "48 volt phantom power on".to_owned()
            } else {
                "phantom power off".to_owned()
            });
        }
        if parts.is_empty() {
            continue;
        }
        let _ = writeln!(said, "  Input {}: {}.", input.input, parts.join(", "));
    }
    if snapshot.msd_mode == Some(true) {
        said.push_str(
            "  This interface is still in mass storage mode, which is how it leaves the \
             factory. It presents a small disk carrying registration files. Everything here \
             works anyway, so nothing needs doing about it.\n",
        );
    }
    said
}

fn gain(args: &GainArgs) -> Result<()> {
    let (interface, mut session) = open(&args.select)?;
    let Some(value) = args.value else {
        let now = session.gain(args.input)?;
        println!("{} input {} gain is {now}.", interface.name, args.input);
        return Ok(());
    };
    let before = session.gain(args.input)?;
    session.set_gain(args.input, value)?;
    let after = session.gain(args.input)?;
    if after == value {
        println!(
            "{} input {} gain set to {after}, and reads back the same.",
            interface.name, args.input
        );
        println!("To undo: roadie audio gain {} {before}", args.input);
    } else {
        println!(
            "{} input {} gain was set to {value} and reads back {after}. The interface \
             settled on a different number than it was given.",
            interface.name, args.input
        );
    }
    Ok(())
}

fn mute(args: &MuteArgs) -> Result<()> {
    let (interface, mut session) = open(&args.select)?;
    let Some(state) = args.state.as_deref() else {
        let now = session.muted(args.input)?;
        println!(
            "{} input {} is {}.",
            interface.name,
            args.input,
            if now { "muted" } else { "not muted" }
        );
        return Ok(());
    };
    let wanted = state == "on";
    session.set_muted(args.input, wanted)?;
    let after = session.muted(args.input)?;
    println!(
        "{} input {} is now {}.",
        interface.name,
        args.input,
        if after { "muted" } else { "not muted" }
    );
    Ok(())
}

fn phantom(args: &PhantomArgs) -> Result<()> {
    let (interface, mut session) = open(&args.select)?;
    let Some(state) = args.state.as_deref() else {
        let now = session.phantom(args.input)?;
        println!(
            "{} input {} has 48 volt phantom power {}.",
            interface.name,
            args.input,
            if now { "on" } else { "off" }
        );
        return Ok(());
    };
    let wanted = state == "on";

    // The acknowledgement is built here, where the risk is in hand, and only
    // when the flag was given. That is what stops a `--yes` several
    // functions away from authorising something nobody read about.
    let acknowledged = match (Risk::of_phantom(args.input, wanted), args.r#yes) {
        (Some(risk), true) => Some(Acknowledged::of(risk)),
        (Some(risk), false) => {
            println!("{}", risk.spoken());
            println!(
                "Nothing has changed. To go ahead: roadie audio phantom {} on --yes",
                args.input
            );
            return Ok(());
        }
        (None, _) => None,
    };

    session.set_phantom(args.input, wanted, acknowledged)?;
    let after = session.phantom(args.input)?;
    println!(
        "{} input {} now has 48 volt phantom power {}.",
        interface.name,
        args.input,
        if after { "on" } else { "off" }
    );
    if after {
        println!(
            "To switch it off again: roadie audio phantom {} off",
            args.input
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
