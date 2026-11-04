//! `roadie doctor` — why nothing is working, and what to do about it.
//!
//! Every other command in this program assumes it can reach your hardware.
//! When it cannot, what it can honestly say is "nothing found", which is the
//! same sentence whether your desk is empty, a cable is out, or this process
//! is not allowed to open the devices sitting right there. Those have entirely
//! different answers and the first one is almost never the real cause.
//!
//! So: one command that checks each thing in turn and, for anything wrong,
//! says what to do about it in words you can act on. It is written to be read
//! aloud — a numbered list of steps, not a table of green and red dots — which
//! is the form this information has to take for it to be usable at all without
//! sight, and is a better form for everyone.
//!
//! The gathering touches the machine; the reasoning does not. [`diagnose`]
//! turns facts into findings and is where every judgement lives, so the advice
//! this command gives is checked rather than only ever seen on the one machine
//! it was written on.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::spoken::counted;

/// Exit status for "something is wrong that will stop this program working".
///
/// Distinct from a failure of the command itself: `doctor` succeeded at
/// checking, and found a problem. A script that could not tell those apart
/// would report a broken tool when the tool is working perfectly and the
/// permissions are not.
const PROBLEMS_FOUND: u8 = 2;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print machine-readable JSON instead of prose.
    ///
    /// The same rendering the MCP `diagnose` tool returns, so a script and an
    /// assistant diagnosing the same machine cannot be told different things.
    #[arg(long)]
    pub json: bool,
}

/// What the machine actually looks like right now.
///
/// A plain record, so [`diagnose`] can be given a machine that does not exist
/// — the machine with no permissions, the machine with an empty desk — and
/// checked against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The operating system, as `cfg` reports it.
    pub platform: Platform,
    /// HID devices the survey could see.
    pub hid_devices: usize,
    /// Cameras the OS reported.
    pub cameras: usize,
    /// Monitors, and whether their control channel could be opened.
    pub monitors: Monitors,
    /// Raw HID nodes present, and how many this process could open.
    ///
    /// `None` off Linux, where there is no equivalent to count.
    pub hidraw: Option<Hidraw>,
    /// Whether a running agent answered.
    pub agent_reachable: bool,
    /// Where the configuration file would be, and whether it is there.
    pub config: Option<PathBuf>,
    /// Whether that configuration file exists.
    pub config_exists: bool,
    /// Saved Stream Deck layouts.
    pub layouts: usize,
}

/// What the display layer could see.
///
/// Kept apart from the HID and camera counts because a monitor's permission
/// story is a different one: on Linux the kernel publishes each display's EDID
/// to anybody, while the I2C node that carries the controls is group-owned. So
/// a monitor can be found perfectly and still refuse every read, and the two
/// numbers here are what tells that apart from having no monitor at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Monitors {
    /// How many displays were found, reachable or not.
    pub found: usize,
    /// How many of those answered.
    pub answering: usize,
    /// Why the first unreachable one could not be opened.
    ///
    /// One rather than all of them: they nearly always share a cause, and a
    /// list of the same sentence repeated is worse to listen to than one.
    pub unreachable: Option<String>,
}

/// Which operating system, for advice that differs by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux, where device access is udev rules.
    Linux,
    /// macOS, where device access is a privacy consent grant.
    MacOs,
    /// Windows, where HID devices are open to any process.
    Windows,
    /// Anything else.
    Other,
}

/// Raw HID nodes on a Linux machine — only the ones this program drives.
///
/// Counting every node made `doctor` advise opening an ASUS lighting
/// controller nothing here can configure, which is exactly the hand-out the
/// shipped rules refuse to make. A node counts when its vendor is one the
/// shipped rules name, or when its report descriptor declares the VIA usage
/// page — the test `roadie via` itself applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hidraw {
    /// How many `/dev/hidraw*` nodes this program drives exist.
    pub present: usize,
    /// How many of them this process could open for reading.
    pub openable: usize,
    /// How many of them this process could open for writing as well.
    ///
    /// Separate from [`Self::openable`] because this program *writes*: DPI,
    /// key images, keycodes, backlight. A rule that grants read but not write
    /// — `MODE="0644"` rather than `0660`, or an ACL with only `r` — leaves
    /// every read succeeding and every change failing, which is the most
    /// confusing shape a permissions problem can take. Checking only reads
    /// would report all-clear on exactly that machine.
    pub writable: usize,
    /// USB vendor ids of blocked VIA boards whose vendor the shipped rules do
    /// not name — the only devices a local rule line is the answer for.
    ///
    /// Carried so the advice can be a rule someone pastes rather than a
    /// research task. "Write a udev rule" is not a step; a line with the right
    /// four hex digits already in it is.
    pub blocked_vendors: Vec<u16>,
    /// Blocked nodes whose vendor the shipped rules already name. A rule line
    /// for these is advice already followed; the cause is elsewhere.
    pub blocked_covered: usize,
    /// Whether `70-roadie.rules` is installed where udev reads it.
    pub rules_installed: bool,
    /// Whether this is a remote session, such as SSH. `uaccess` grants a
    /// device to whoever is signed in at the machine itself, so on a remote
    /// session installed rules apply to nobody, and that has to be said
    /// rather than mistaken for missing rules.
    pub remote: bool,
}

/// Vendors `packaging/linux/udev/70-roadie.rules` grants hidraw access to.
/// A test reads that file, so the two cannot drift apart silently.
const SHIPPED_HIDRAW_VENDORS: [u16; 2] = [0x046d, 0x0fd9];

/// Where an installed copy of the shipped rules may live.
const RULES_LOCATIONS: [&str; 3] = [
    "/etc/udev/rules.d/70-roadie.rules",
    "/usr/lib/udev/rules.d/70-roadie.rules",
    "/lib/udev/rules.d/70-roadie.rules",
];

/// How a check came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to do.
    Fine(String),
    /// Something is wrong, with the steps that fix it.
    Problem {
        /// What is wrong, in one sentence.
        detail: String,
        /// What to do, in order.
        fix: Vec<String>,
    },
    /// Could not be determined, and why that is not itself a fault.
    Undetermined(String),
}

/// One thing checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: &'static str,
    /// What was found.
    pub verdict: Verdict,
}

/// Read the machine and diagnose it, without printing anything.
///
/// The half of this command the MCP server needs: the findings as data, so an
/// assistant can read the steps out rather than parse the text meant for a
/// terminal.
pub async fn examine() -> Vec<Check> {
    diagnose(&gather().await)
}

/// Look at the machine and say what is wrong with it.
///
/// # Errors
///
/// Does not fail on a broken machine — that is the case it exists for. Fails
/// only when the survey itself cannot run.
pub async fn run(args: DoctorArgs) -> Result<ExitCode> {
    let facts = gather().await;
    let checks = diagnose(&facts);
    let problems = checks
        .iter()
        .filter(|check| matches!(check.verdict, Verdict::Problem { .. }))
        .count();
    if args.json {
        println!("{}", render_json(&checks));
    } else {
        print!("{}", render(&checks, problems));
    }
    Ok(if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(PROBLEMS_FOUND)
    })
}

/// Read the machine.
async fn gather() -> Facts {
    // A survey that fails is itself a fact worth reporting as "saw nothing",
    // not a reason to abandon the other seven checks. Someone running `doctor`
    // has already hit a failure; a second one is not news.
    let hid_devices = roadie_hid::survey::hid_peripherals()
        .await
        .map_or(0, |found| found.len());
    let config = roadie_core::paths::config_path().ok();
    Facts {
        platform: platform(),
        hid_devices,
        cameras: roadie_camera::enumerate_all_cameras().len(),
        monitors: survey_monitors(),
        hidraw: count_hidraw(),
        agent_reachable: agent_reachable().await,
        config_exists: config.as_ref().is_some_and(|path| path.is_file()),
        config,
        layouts: crate::cmd::streamdeck::layout_library()
            .ok()
            .map_or(0, |path| count_layouts(&path)),
    }
}

/// What the display layer can see, and what stopped it.
///
/// Deliberately does not look on the network. `doctor` is the fast triage
/// command, multicast discovery costs seconds, and an Elgato light's problems
/// are not the permission problems this command exists to sort out.
fn survey_monitors() -> Monitors {
    let Ok(mut displays) = roadie_display::enumerate() else {
        return Monitors::default();
    };
    let found = displays.len();
    let mut answering = 0;
    let mut unreachable = None;
    for display in &mut displays {
        // The MCCS version is the cheapest question there is, and a monitor
        // that answers it will answer the rest.
        match display.get(roadie_ddc::Feature::McssVersion) {
            Ok(_) => answering += 1,
            Err(error) => {
                unreachable.get_or_insert_with(|| error.to_string());
            }
        }
    }
    Monitors {
        found,
        answering,
        unreachable,
    }
}

/// This build's operating system.
const fn platform() -> Platform {
    if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    }
}

/// Count `/dev/hidraw*` nodes and how many open.
///
/// Vendor-neutral on purpose. The permissions crate probes Logitech nodes
/// specifically, which was right when this program was a Logitech utility and
/// is wrong now: someone whose only device is a Stream Deck would be told
/// their access is fine while nothing works.
fn count_hidraw() -> Option<Hidraw> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let entries = std::fs::read_dir("/dev").ok()?;
    let nodes: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("hidraw"))
        })
        .filter(|path| {
            vendor_of(path).is_some_and(|vendor| SHIPPED_HIDRAW_VENDORS.contains(&vendor))
                || is_via_node(path)
        })
        .collect();

    let mut openable = 0;
    let mut writable = 0;
    let mut blocked_vendors: Vec<u16> = Vec::new();
    let mut blocked_covered = 0;
    for node in &nodes {
        // Opening for write sends nothing to the device; it only asks the
        // kernel whether this process would be allowed to.
        if std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .is_ok()
        {
            openable += 1;
            writable += 1;
            continue;
        }
        if std::fs::File::open(node).is_ok() {
            openable += 1;
            continue;
        }
        match vendor_of(node) {
            Some(vendor) if SHIPPED_HIDRAW_VENDORS.contains(&vendor) => blocked_covered += 1,
            Some(vendor) if !blocked_vendors.contains(&vendor) => blocked_vendors.push(vendor),
            _ => {}
        }
    }
    blocked_vendors.sort_unstable();
    Some(Hidraw {
        present: nodes.len(),
        openable,
        writable,
        blocked_vendors,
        blocked_covered,
        rules_installed: RULES_LOCATIONS
            .iter()
            .any(|path| std::path::Path::new(path).exists()),
        remote: std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_TTY").is_some(),
    })
}

/// Whether a hidraw node is a VIA board, read from the report descriptor
/// the kernel publishes for it.
fn is_via_node(node: &std::path::Path) -> bool {
    node.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            std::fs::read(format!("/sys/class/hidraw/{name}/device/report_descriptor")).ok()
        })
        .is_some_and(|descriptor| declares_via_page(&descriptor))
}

/// Whether a HID report descriptor declares the VIA usage page.
///
/// Walks the items rather than searching for bytes, so a page value inside
/// some other item's data cannot be mistaken for one. A short item's prefix
/// carries its size in the low two bits (3 meaning 4 bytes); `0xFE` opens a
/// long item whose size is the next byte. Usage Page is the global item with
/// tag bits `0x04`.
fn declares_via_page(descriptor: &[u8]) -> bool {
    let mut at = 0;
    while let Some(&prefix) = descriptor.get(at) {
        if prefix == 0xfe {
            let size = descriptor.get(at + 1).map_or(0, |size| usize::from(*size));
            at += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            other => usize::from(other),
        };
        let Some(data) = descriptor.get(at + 1..at + 1 + size) else {
            return false;
        };
        if prefix & 0xfc == 0x04 {
            let mut page = [0u8; 4];
            page[..size].copy_from_slice(data);
            if u32::from_le_bytes(page) == u32::from(roadie_via::identity::USAGE_PAGE) {
                return true;
            }
        }
        at += 1 + size;
    }
    false
}

/// The USB vendor id behind a `/dev/hidrawN` node.
fn vendor_of(node: &std::path::Path) -> Option<u16> {
    let name = node.file_name()?.to_str()?;
    let uevent = std::fs::read_to_string(format!("/sys/class/hidraw/{name}/device/uevent")).ok()?;
    vendor_in_uevent(&uevent)
}

/// The USB vendor id in a kernel `uevent` file's contents.
///
/// The kernel writes `HID_ID=bus:vendor:product` with each field as
/// zero-padded uppercase hex, so this parses hex and compares numerically —
/// `0000046D` and `046d` are the same number, which they are.
///
/// Split from reading the file because this is the part that can be wrong,
/// and being wrong here is not cosmetic: the id ends up in a udev rule that
/// someone pastes into `/etc/udev/rules.d` as root. A rule naming the wrong
/// vendor grants access to the wrong device and not to theirs, and looks
/// close enough to correct to waste an afternoon.
fn vendor_in_uevent(uevent: &str) -> Option<u16> {
    uevent.lines().find_map(|line| {
        let rest = line.strip_prefix("HID_ID=")?;
        u16::from_str_radix(rest.split(':').nth(1)?.trim(), 16).ok()
    })
}

/// The udev lines that would give this user access to `vendors`.
///
/// Written out in full, ready to paste. Someone told to "add a udev rule" has
/// been given a research task; someone handed the line with the right four hex
/// digits already in it has been given a step.
fn udev_lines(vendors: &[u16]) -> Vec<String> {
    vendors
        .iter()
        .map(|vendor| {
            format!("SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor:04x}\", TAG+=\"uaccess\"")
        })
        .collect()
}

/// Layout files in a directory.
fn count_layouts(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .count()
    })
}

/// Whether an agent answers.
async fn agent_reachable() -> bool {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        roadie_ipc::client::connect(),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Turn facts into findings.
///
/// Every judgement this command makes lives here, taking a plain record and
/// returning plain findings, so the advice can be checked against machines
/// this one will never be — the machine with no permissions, the empty desk,
/// the one where a Stream Deck is the only device.
#[must_use]
pub fn diagnose(facts: &Facts) -> Vec<Check> {
    vec![
        device_access(facts),
        devices_visible(facts),
        monitors_reachable(facts),
        agent(facts),
        configuration(facts),
        layouts(facts),
    ]
}

/// Can this process open the devices at all?
///
/// First, because it is the cause of most of what follows: a permissions
/// problem makes an attached device invisible, and being told "no devices
/// found" first would send someone to check their cables.
fn device_access(facts: &Facts) -> Check {
    let verdict = match (facts.platform, facts.hidraw.as_ref()) {
        (Platform::Linux, Some(hidraw)) => linux_access(hidraw),
        (Platform::Linux, None) => Verdict::Undetermined(
            "could not read /dev to look for HID devices, which is unusual and worth \
             investigating on its own"
                .to_owned(),
        ),
        (Platform::MacOs, _) => macos_access(facts),
        (Platform::Windows, _) => Verdict::Fine(
            "Windows lets any program open HID devices; there is nothing to grant".to_owned(),
        ),
        (Platform::Other, _) => Verdict::Undetermined(
            "this platform is not one whose device permissions are known here".to_owned(),
        ),
    };
    Check {
        name: "Permission to open devices",
        verdict,
    }
}

/// The Linux reading of the hidraw counts.
fn linux_access(hidraw: &Hidraw) -> Verdict {
    if hidraw.present == 0 {
        return Verdict::Undetermined(
            "no /dev/hidraw devices exist at all, so there is nothing to have permission \
             for. Either nothing is plugged in, or this is a container or virtual machine \
             with no USB passed through."
                .to_owned(),
        );
    }
    if hidraw.openable == 0 {
        return Verdict::Problem {
            detail: format!(
                "{} attached and this program cannot open any of them. \
                 This is a permissions problem, not a hardware one — the devices are \
                 there.",
                counted(hidraw.present, "HID device is", "HID devices are")
            ),
            fix: udev_fix(hidraw),
        };
    }
    // Checked before the read counts below, because a machine where reads work
    // and writes do not is the one that looks fine and behaves worst: every
    // listing succeeds, every change fails, and nothing says why.
    if hidraw.writable < hidraw.openable {
        return Verdict::Problem {
            detail: format!(
                "{} of {} readable HID devices can also be written to. This program \
                 changes settings on your devices, so reading alone is not enough — \
                 listings will work and every change will fail.",
                hidraw.writable, hidraw.openable
            ),
            fix: vec![
                "The udev rule granting access needs to grant writing too: MODE=\"0660\" \
                 rather than MODE=\"0644\", with your user in the named group."
                    .to_owned(),
                "Check what the device allows now: ls -l /dev/hidraw*".to_owned(),
                "Reload the rules without rebooting: sudo udevadm control --reload-rules \
                 && sudo udevadm trigger"
                    .to_owned(),
                "Then unplug the device and plug it back in — a rule change does not \
                 reach a device that is already open."
                    .to_owned(),
            ],
        };
    }
    if hidraw.openable < hidraw.present {
        return Verdict::Problem {
            detail: format!(
                "{} of {} attached HID devices can be opened. The rest are visible but \
                 out of reach, so some of your peripherals will work here and some will \
                 not.",
                hidraw.openable, hidraw.present
            ),
            fix: udev_fix(hidraw),
        };
    }
    // Not "all 1 attached HID device", which is what a count alone gives for
    // the commonest case on a desk with one thing plugged in.
    Verdict::Fine(if hidraw.present == 1 {
        "the attached HID device can be opened".to_owned()
    } else {
        format!("all {} attached HID devices can be opened", hidraw.present)
    })
}

/// The steps that give this user access to the devices it cannot open.
///
/// Written around the vendors actually blocked. The shipped rules name the
/// vendors this program drives rather than matching every HID device — a
/// wildcard would hand the logged-in user everything on the bus — so a
/// peripheral from a vendor not in that list needs a line, and this is that
/// line with the digits already filled in.
fn udev_fix(hidraw: &Hidraw) -> Vec<String> {
    let mut steps = Vec::new();
    // Whether any step below changes the rules, which is what reloading and
    // replugging are for. Advice to reload rules nobody changed is noise.
    let mut rules_change = false;
    if !hidraw.rules_installed {
        steps.push(
            "Install the udev rules this project ships: sudo cp \
             packaging/linux/udev/70-roadie.rules /etc/udev/rules.d/"
                .to_owned(),
        );
        rules_change = true;
    } else if hidraw.blocked_covered > 0 && hidraw.remote {
        steps.push(
            "The udev rules this project ships are installed, and they grant access to \
             whoever is signed in at this computer itself. This session is remote, over \
             SSH for instance, so they do not apply to it. Run roadie from a session at \
             the computer, or put sudo in front of it for a one-off check."
                .to_owned(),
        );
    }
    let lines = udev_lines(&hidraw.blocked_vendors);
    if !lines.is_empty() {
        rules_change = true;
        steps.push(format!(
            "Those rules name the vendors this program drives, and the {} not among \
             them. Put {} in /etc/udev/rules.d/71-roadie-local.rules — a separate \
             file, so upgrading this program does not overwrite it:",
            counted(
                lines.len(),
                "device you cannot open is",
                "devices you cannot open are"
            ),
            if lines.len() == 1 {
                "this line".to_owned()
            } else {
                format!("these {} lines", lines.len())
            }
        ));
        steps.extend(lines.into_iter().map(|line| format!("    {line}")));
    }
    if rules_change {
        steps.push(
            "Reload the rules without rebooting: sudo udevadm control --reload-rules && \
             sudo udevadm trigger"
                .to_owned(),
        );
    }
    if rules_change || !hidraw.remote {
        steps.push(
            "Unplug the device and plug it back in — a rule applies when a device \
             appears, so one already attached keeps the permissions it was given."
                .to_owned(),
        );
    }
    steps.push("Run roadie doctor again to confirm.".to_owned());
    steps
}

/// The macOS reading, which is a consent grant rather than a file mode.
fn macos_access(facts: &Facts) -> Verdict {
    if facts.hid_devices > 0 {
        return Verdict::Fine(
            "HID devices are being read, so Input Monitoring is granted to this program".to_owned(),
        );
    }
    // Deliberately hedged. macOS gives no way to tell "not allowed to read
    // HID" apart from "nothing is attached" — both come back as an empty list
    // — and asserting the permission cause on a Mac with nothing plugged in
    // would send someone into System Settings to fix nothing. Permissions are
    // much the likelier of the two, so it is reported as a problem; which one
    // it is, is left to the reader, with the way to tell.
    Verdict::Problem {
        detail: "no HID device could be read. That is either a permission this program \
                 does not have, or nothing being plugged in — macOS reports both as an \
                 empty list, so this cannot tell them apart. If a mouse or keyboard is \
                 attached, it is the permission."
            .to_owned(),
        fix: vec![
            "Input Monitoring is granted per program, so the app having it does not give \
             it to this command, and the other way round."
                .to_owned(),
            "Open System Settings, then Privacy & Security, then Input Monitoring.".to_owned(),
            "Turn on the entry for this program. If it is not listed, run any roadie \
             command that touches a device once and macOS will add it."
                .to_owned(),
            "Quit and reopen your terminal. A grant applies to a process when it starts, \
             so the session you were already in keeps the answer it had."
                .to_owned(),
            "Run roadie doctor again to confirm. If it still finds nothing and you are \
             sure something is plugged in, that is worth reporting."
                .to_owned(),
        ],
    }
}

/// Is anything actually there?
fn devices_visible(facts: &Facts) -> Check {
    let total = facts.hid_devices + facts.cameras;
    let verdict = if total == 0 {
        Verdict::Problem {
            detail: "no HID device and no camera was found. Monitors and Elgato lights \
                     are checked separately and are not covered by this line."
                .to_owned(),
            fix: vec![
                "If the permission check above found a problem, fix that first — it is \
                 almost certainly the cause of this one."
                    .to_owned(),
                "Otherwise check the cable and the port, and try a different port.".to_owned(),
                "A device connected over Bluetooth has to be paired with the operating \
                 system first; this program does not pair it for you."
                    .to_owned(),
            ],
        }
    } else {
        Verdict::Fine(format!(
            "{} and {} found; roadie devices lists them",
            counted(facts.hid_devices, "HID device", "HID devices"),
            counted(facts.cameras, "camera", "cameras")
        ))
    };
    Check {
        name: "Peripherals found",
        verdict,
    }
}

/// Monitors, which have their own permission story.
///
/// Its own check rather than folded into the one above, because "found but
/// not answering" is the interesting state and it has no equivalent for a HID
/// device: on Linux the EDID is world-readable and the I2C node is not, so a
/// perfectly working monitor appears in the list and refuses every read. That
/// is a group membership, it is fixable in one command, and nothing else in
/// this program would ever tell someone about it.
fn monitors_reachable(facts: &Facts) -> Check {
    let monitors = &facts.monitors;
    let verdict = if monitors.found == 0 {
        // Not a problem. Plenty of machines have no external monitor, and a
        // laptop's own screen never speaks DDC.
        Verdict::Fine(
            "no external monitor was found; a laptop's own screen never speaks DDC and is \
             not expected here"
                .to_owned(),
        )
    } else if monitors.answering == monitors.found {
        Verdict::Fine(format!(
            "{} found, and all of them answer; roadie display list shows them",
            counted(monitors.found, "monitor", "monitors")
        ))
    } else {
        Verdict::Problem {
            detail: format!(
                "{} found, and {} of them answer. The first one that did not said: {}",
                counted(monitors.found, "monitor", "monitors"),
                monitors.answering,
                monitors
                    .unreachable
                    .as_deref()
                    .unwrap_or("nothing this program could read")
            ),
            fix: monitor_fix_steps(),
        }
    };
    Check {
        name: "Monitors reachable",
        verdict,
    }
}

/// The ordered steps for a monitor that is seen but does not answer.
///
/// The i2c group is real advice on exactly one platform. Every fix list is
/// read aloud, and the first step heard on a Mac must not be one that cannot
/// be taken there — so the Linux step exists only where it can be followed,
/// and the wording of the step after it does not lean on a step that may not
/// have been said.
fn monitor_fix_steps() -> Vec<String> {
    let mut steps = Vec::new();
    if cfg!(target_os = "linux") {
        steps.push(
            "The I2C devices belong to the i2c group. Add your user to it, then log out \
             and back in."
                .to_owned(),
        );
        steps.push(
            "Otherwise the commonest cause by far is DDC/CI switched off in the monitor's \
             own menu, where many ship it off."
                .to_owned(),
        );
    } else {
        steps.push(
            "The commonest cause by far is DDC/CI switched off in the monitor's own menu, \
             where many ship it off."
                .to_owned(),
        );
    }
    steps.push(
        "A monitor connected through some docks and KVM switches cannot carry DDC at all, \
         whatever the settings say."
            .to_owned(),
    );
    steps
}

/// The agent, which is not required for the CLI but changes what takes effect.
fn agent(facts: &Facts) -> Check {
    let verdict = if facts.agent_reachable {
        Verdict::Fine("the background agent is running and answering".to_owned())
    } else {
        // Deliberately not a problem. Every command here works without it,
        // and calling a working setup broken because an optional part is not
        // running is how a diagnostic loses the trust it needs.
        Verdict::Undetermined(
            "no background agent is running. Nothing here needs one — but a configuration \
             change will not take effect in the app until the agent reloads it, and on \
             macOS the agent is what holds the permission grant that button remapping \
             uses."
                .to_owned(),
        )
    };
    Check {
        name: "Background agent",
        verdict,
    }
}

/// Where settings live.
fn configuration(facts: &Facts) -> Check {
    let verdict = match (&facts.config, facts.config_exists) {
        (Some(path), true) => Verdict::Fine(format!("{}", path.display())),
        (Some(path), false) => Verdict::Undetermined(format!(
            "no configuration file yet. It will be written to {} the first time you \
             change something. That is the normal state on a new machine.",
            path.display()
        )),
        (None, _) => Verdict::Problem {
            detail: "could not work out where configuration should live, which means \
                     nothing can be saved."
                .to_owned(),
            fix: vec![
                "This normally means HOME is unset. Check that your shell has it: echo \
                 $HOME"
                    .to_owned(),
            ],
        },
    };
    Check {
        name: "Configuration",
        verdict,
    }
}

/// Saved layouts, so a bundle's contents are never a surprise.
fn layouts(facts: &Facts) -> Check {
    let verdict = if facts.layouts == 0 {
        Verdict::Undetermined(
            "no Stream Deck layouts saved. roadie streamdeck example <name> starts one.".to_owned(),
        )
    } else {
        Verdict::Fine(format!(
            "{} saved; roadie profile export carries them",
            counted(facts.layouts, "layout", "layouts")
        ))
    };
    Check {
        name: "Saved layouts",
        verdict,
    }
}

/// One check as JSON, for `--json` and for the MCP `diagnose` tool.
///
/// The steps travel with the finding rather than in a separate list, because a
/// consumer that has one without the other has half of what it needs.
#[must_use]
pub fn check_json(check: &Check) -> serde_json::Value {
    let (state, detail) = match &check.verdict {
        Verdict::Fine(detail) => ("ok", detail),
        Verdict::Undetermined(detail) => ("note", detail),
        Verdict::Problem { detail, .. } => ("problem", detail),
    };
    let mut entry = serde_json::Map::new();
    entry.insert("check".to_owned(), serde_json::json!(check.name));
    entry.insert("state".to_owned(), serde_json::json!(state));
    entry.insert("detail".to_owned(), serde_json::json!(detail));
    if let Verdict::Problem { fix, .. } = &check.verdict {
        entry.insert("steps".to_owned(), serde_json::json!(fix));
    }
    serde_json::Value::Object(entry)
}

/// What this build is and where it is running.
///
/// Carried in both renderings because this output exists to be pasted into a
/// bug report, and these are the first two things anyone reading one has to
/// ask for. Leaving them out costs a round trip, which is a poor trade
/// anywhere and a worse one for someone working by dictation.
#[must_use]
pub fn provenance() -> String {
    format!(
        "roadie {} on {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

/// The whole diagnosis as JSON.
#[must_use]
pub fn render_json(checks: &[Check]) -> String {
    let problems = checks
        .iter()
        .filter(|check| matches!(check.verdict, Verdict::Problem { .. }))
        .count();
    let document = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "checks": checks.iter().map(check_json).collect::<Vec<_>>(),
        "problems": problems,
    });
    serde_json::to_string_pretty(&document)
        .unwrap_or_else(|_| r#"{"error":"the diagnosis could not be rendered as JSON"}"#.to_owned())
}

/// The report, as the text a person receives.
///
/// Problems are repeated at the end as one numbered list of steps. Someone who
/// has just heard eight checks read out should not have to scroll back through
/// them to find the two things they actually have to do.
#[must_use]
pub fn render(checks: &[Check], problems: usize) -> String {
    let mut out = String::new();
    for check in checks {
        match &check.verdict {
            Verdict::Fine(detail) => {
                let _ = writeln!(out, "OK    {}: {detail}", check.name);
            }
            Verdict::Undetermined(detail) => {
                let _ = writeln!(out, "NOTE  {}: {detail}", check.name);
            }
            Verdict::Problem { detail, .. } => {
                let _ = writeln!(out, "FIX   {}: {detail}", check.name);
            }
        }
    }
    let _ = writeln!(out);

    if problems == 0 {
        let _ = writeln!(out, "Nothing needs fixing.");
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", provenance());
        return out;
    }

    // Spelled out rather than "1 thing(s)". This is read aloud as often as it
    // is read on screen, and a screen reader says "thing open paren s close
    // paren", which is worse than either word would have been.
    let _ = if problems == 1 {
        writeln!(out, "One thing to fix:")
    } else {
        writeln!(
            out,
            "{problems} things to fix. In order, because the first often causes the rest:"
        )
    };
    let mut step = 1;
    for check in checks {
        let Verdict::Problem { detail, fix } = &check.verdict else {
            continue;
        };
        let _ = writeln!(out);
        let _ = writeln!(out, "{}: {detail}", check.name);
        for instruction in fix {
            let _ = writeln!(out, "  {step}. {instruction}");
            step += 1;
        }
    }
    // Last, not first. Someone running this wants to hear what is wrong
    // before they hear which build it is; the build matters when they report
    // it, and a report is the whole output anyway.
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", provenance());
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Check, Facts, Hidraw, Monitors, Platform, Verdict, counted, diagnose, render, render_json,
        udev_lines, vendor_in_uevent,
    };

    /// A machine where everything is fine, to vary one thing at a time from.
    fn healthy() -> Facts {
        Facts {
            platform: Platform::Linux,
            hid_devices: 3,
            cameras: 1,
            monitors: Monitors::default(),
            hidraw: Some(Hidraw {
                present: 4,
                openable: 4,
                writable: 4,
                blocked_vendors: Vec::new(),
                blocked_covered: 0,
                rules_installed: true,
                remote: false,
            }),
            agent_reachable: true,
            config: Some(PathBuf::from("/home/me/.config/roadie/config.toml")),
            config_exists: true,
            layouts: 2,
        }
    }

    /// Facts for a desk with `found` monitors, `answering` of which answer.
    fn with_monitors(found: usize, answering: usize) -> Facts {
        Facts {
            monitors: Monitors {
                found,
                answering,
                unreachable: (answering < found).then(|| {
                    "LG ULTRAFINE cannot be reached: /dev/i2c-7: permission denied. The \
                     I2C devices belong to the i2c group"
                        .to_owned()
                }),
            },
            ..healthy()
        }
    }

    #[test]
    fn a_desk_with_no_external_monitor_is_not_a_problem() {
        // Plenty of machines have none, and a laptop's own screen never speaks
        // DDC. Calling that broken is how a diagnostic loses the trust it
        // needs to be worth running.
        let checks = diagnose(&with_monitors(0, 0));
        assert!(matches!(
            find(&checks, "Monitors reachable"),
            Verdict::Fine(_)
        ));
    }

    #[test]
    fn monitors_that_all_answer_are_reported_fine() {
        let checks = diagnose(&with_monitors(2, 2));
        let Verdict::Fine(detail) = find(&checks, "Monitors reachable") else {
            panic!("two answering monitors is not a problem");
        };
        assert!(detail.contains("2 monitors"), "{detail}");
    }

    #[test]
    fn a_monitor_that_is_seen_but_will_not_answer_names_the_group() {
        // The state with no equivalent among HID devices, and the reason this
        // check exists: on Linux the EDID is world-readable and the I2C node
        // is not, so a working monitor appears in the list and refuses every
        // read. Nothing else in this program would tell someone about that.
        let checks = diagnose(&with_monitors(2, 1));
        let Verdict::Problem { detail, fix } = find(&checks, "Monitors reachable") else {
            panic!("a monitor that will not answer is a problem worth naming");
        };
        assert!(detail.contains("1 of them answer"), "{detail}");
        assert!(detail.contains("i2c group"), "{detail}");
        assert!(
            fix.iter().any(|step| step.contains("DDC/CI switched off")),
            "the commonest cause has to be offered everywhere: {fix:?}"
        );
    }

    /// Every fix step is read aloud, so each platform's list carries only the
    /// steps that can be taken there: the i2c group is Linux's fix and Linux's
    /// alone, and a Mac must not hear it first — or at all.
    #[test]
    fn the_monitor_fix_steps_fit_the_platform_they_are_read_on() {
        let checks = diagnose(&with_monitors(2, 1));
        let Verdict::Problem { fix, .. } = find(&checks, "Monitors reachable") else {
            panic!("a monitor that will not answer is a problem worth naming");
        };
        assert_eq!(
            fix.iter().any(|step| step.contains("i2c group")),
            cfg!(target_os = "linux"),
            "the i2c group step belongs to Linux and to Linux only: {fix:?}"
        );
        assert!(
            fix.first()
                .is_some_and(|step| !step.starts_with("Otherwise")),
            "the first step heard must not lean on one that was not said: {fix:?}"
        );
    }

    #[test]
    fn one_monitor_takes_the_singular() {
        let checks = diagnose(&with_monitors(1, 1));
        let Verdict::Fine(detail) = find(&checks, "Monitors reachable") else {
            panic!("one answering monitor is not a problem");
        };
        assert!(
            detail.contains("1 monitor found"),
            "not \"1 monitors\": {detail}"
        );
    }

    #[test]
    fn the_missing_peripherals_line_no_longer_claims_to_cover_everything() {
        // It counts HID devices and cameras only. Monitors and Elgato lights
        // are found by other means, and a sentence saying "no peripheral of
        // any kind" would be read as covering them too.
        let checks = diagnose(&Facts {
            hid_devices: 0,
            cameras: 0,
            ..healthy()
        });
        let Verdict::Problem { detail, .. } = find(&checks, "Peripherals found") else {
            panic!("nothing at all is a problem");
        };
        assert!(detail.contains("checked separately"), "{detail}");
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Verdict {
        &checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no check named {name}"))
            .verdict
    }

    fn problems(checks: &[Check]) -> usize {
        checks
            .iter()
            .filter(|check| matches!(check.verdict, Verdict::Problem { .. }))
            .count()
    }

    #[test]
    fn a_healthy_machine_has_nothing_to_fix() {
        let checks = diagnose(&healthy());
        assert_eq!(problems(&checks), 0);
        assert!(render(&checks, 0).contains("Nothing needs fixing."));
    }

    /// The case this command exists for. Devices attached, none openable, is
    /// a permissions problem — and saying "no devices found" instead sends
    /// someone to check cables that are perfectly well plugged in.
    #[test]
    fn devices_present_but_unopenable_is_named_as_permissions() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 4,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: false,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;

        let checks = diagnose(&facts);
        let Verdict::Problem { detail, fix } = find(&checks, "Permission to open devices") else {
            panic!("unopenable devices must be a problem");
        };
        assert!(
            detail.contains("permissions problem, not a hardware one"),
            "{detail}"
        );
        assert!(
            fix.iter().any(|step| step.contains("udevadm")),
            "the fix has to be a command someone can run: {fix:?}"
        );
        assert!(
            fix.iter().any(|step| step.contains("plug it back in")),
            "a rule applies when a device appears, so this step is not optional: {fix:?}"
        );
    }

    /// Permission problems come before "nothing found", because the first
    /// causes the second and fixing them in the other order fixes nothing.
    #[test]
    fn permission_is_reported_before_devices_are_missing() {
        let checks = diagnose(&healthy());
        let names: Vec<&str> = checks.iter().map(|check| check.name).collect();
        let permission = names
            .iter()
            .position(|name| *name == "Permission to open devices")
            .expect("a permission check");
        let devices = names
            .iter()
            .position(|name| *name == "Peripherals found")
            .expect("a devices check");
        assert!(permission < devices, "{names:?}");
    }

    /// The step that matters most on Linux. "Add a udev rule" is a research
    /// task; a line with the right four hex digits already in it is a step
    /// someone can carry out without knowing what udev is.
    #[test]
    fn a_blocked_vendor_produces_the_exact_rule_line_to_paste() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 2,
            openable: 0,
            writable: 0,
            // A blocked Elgato deck, which the shipped rules already name, and
            // a VIA macro pad from a vendor they cannot.
            blocked_vendors: vec![0x4653],
            blocked_covered: 1,
            rules_installed: true,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;

        let checks = diagnose(&facts);
        let Verdict::Problem { fix, .. } = find(&checks, "Permission to open devices") else {
            panic!("unopenable devices must be a problem");
        };
        let steps = fix.join("\n");
        assert!(
            steps.contains(r#"ATTRS{idVendor}=="4653""#),
            "the vendor id has to be in the line, in lower-case hex: {steps}"
        );
        // This was once offered for Elgato, whose line the shipped rules
        // already carry: advice already followed, handed out again as a fix.
        assert!(!steps.contains(r#""0fd9""#), "{steps}");
        assert!(
            steps.contains(r#"SUBSYSTEM=="hidraw""#),
            "a line that is not a valid rule is worse than no line: {steps}"
        );
        assert!(steps.contains(r#"TAG+="uaccess""#), "{steps}");
        assert!(
            steps.contains("71-roadie-local.rules"),
            "it has to go in a separate file, or an upgrade overwrites it: {steps}"
        );
    }

    /// The kernel's own format, as `/sys/class/hidraw/hidrawN/device/uevent`
    /// actually writes it. Getting this wrong puts the wrong four digits into
    /// a rule someone pastes as root.
    #[test]
    fn a_vendor_id_is_read_out_of_a_real_uevent_file() {
        let uevent = "DRIVER=hid-generic\n\
                      HID_ID=0003:0000046D:0000C52B\n\
                      HID_NAME=Logitech USB Receiver\n\
                      HID_PHYS=usb-0000:00:14.0-3/input2\n\
                      MODALIAS=hid:b0003g0001v0000046Dp0000C52B\n";
        assert_eq!(vendor_in_uevent(uevent), Some(0x046d));
    }

    #[test]
    fn the_vendor_field_is_the_second_one_not_the_first() {
        // Field one is the bus (0003 = USB). Reading it instead would report
        // vendor 0x0003 for every device on the machine — plausible-looking,
        // uniformly wrong, and the rule would match nothing.
        assert_eq!(
            vendor_in_uevent("HID_ID=0003:00000FD9:00000080\n"),
            Some(0x0fd9)
        );
    }

    /// Case and zero-padding vary between kernels and between fields.
    #[test]
    fn case_and_padding_do_not_change_the_number() {
        assert_eq!(
            vendor_in_uevent("HID_ID=0003:0000046d:0000C52B"),
            Some(0x046d)
        );
        assert_eq!(vendor_in_uevent("HID_ID=3:46D:C52B"), Some(0x046d));
    }

    /// A file without the line, an empty file, or a malformed one must give
    /// nothing rather than a wrong number — the advice then omits the rule,
    /// which is a worse experience and a much better outcome than a rule
    /// naming a device the person does not own.
    #[test]
    fn anything_unparseable_yields_no_vendor_rather_than_a_wrong_one() {
        assert_eq!(vendor_in_uevent(""), None);
        assert_eq!(
            vendor_in_uevent("DRIVER=hid-generic\nHID_NAME=Thing\n"),
            None
        );
        assert_eq!(vendor_in_uevent("HID_ID=0003\n"), None, "no vendor field");
        assert_eq!(vendor_in_uevent("HID_ID=0003:zzzz:0001\n"), None, "not hex");
        assert_eq!(
            vendor_in_uevent("HID_ID=0003:000146D5:0001\n"),
            None,
            "wider than a u16 is not a vendor id"
        );
    }

    /// A line that merely contains the marker is not the line. `MODALIAS`
    /// sits in the same file and carries the same digits in another shape.
    #[test]
    fn only_a_line_that_starts_with_the_marker_counts() {
        assert_eq!(
            vendor_in_uevent("X_HID_ID=0003:0000FFFF:0001\nHID_ID=0003:0000046D:0001\n"),
            Some(0x046d)
        );
    }

    /// Hex, not decimal. A rule reading `idVendor=="4057"` for Elgato matches
    /// nothing, and looks close enough to right to waste an afternoon.
    #[test]
    fn vendor_ids_in_rules_are_four_hex_digits() {
        assert_eq!(
            udev_lines(&[0x0fd9]),
            vec![r#"SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0fd9", TAG+="uaccess""#.to_owned()]
        );
        // Leading zeros are significant in a udev match.
        assert!(udev_lines(&[0x046d])[0].contains(r#""046d""#));
        assert!(udev_lines(&[0x0001])[0].contains(r#""0001""#));
    }

    /// When the blocked vendors could not be read, the advice must still be
    /// usable rather than trailing off into a sentence with no line under it.
    #[test]
    fn advice_without_vendor_ids_is_still_a_complete_set_of_steps() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 2,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: false,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;

        let checks = diagnose(&facts);
        let Verdict::Problem { fix, .. } = find(&checks, "Permission to open devices") else {
            panic!("unopenable devices must be a problem");
        };
        let steps = fix.join("\n");
        assert!(steps.contains("70-roadie.rules"), "{steps}");
        assert!(steps.contains("udevadm"), "{steps}");
        assert!(steps.contains("plug it back in"), "{steps}");
        assert!(
            !steps.contains("71-roadie-local.rules"),
            "with no ids to offer, do not send someone to write a file they cannot \
             fill in: {steps}"
        );
    }

    /// Some devices reachable and some not is its own answer: half a desk
    /// working looks like a flaky program rather than an incomplete rule.
    #[test]
    fn a_partly_reachable_desk_is_its_own_finding() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 4,
            openable: 2,
            writable: 2,
            // The Kiiboom Cybrix 16's vendor: a VIA board the shipped rules
            // cannot name.
            blocked_vendors: vec![0x5343],
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        let checks = diagnose(&facts);
        let Verdict::Problem { detail, fix } = find(&checks, "Permission to open devices") else {
            panic!("a partly reachable desk is a problem");
        };
        assert!(detail.contains("2 of 4"), "{detail}");
        // This is the case where a generated rule helps most: the rules are
        // installed and working for some devices, so "install the udev rules"
        // alone would read as advice already followed.
        assert!(
            fix.iter().any(|step| step.contains(r#""5343""#)),
            "it has to name the vendor that is actually blocked: {fix:?}"
        );
        assert!(
            !fix.iter()
                .any(|step| step.contains("Install the udev rules")),
            "the rules are installed; saying otherwise is advice already followed: {fix:?}"
        );
    }

    /// Found on a headless Ubuntu box over SSH, 2026-11-03: the rules were
    /// installed and named Logitech, and `doctor` told the user to install
    /// them and add a Logitech line anyway.
    #[test]
    fn installed_rules_over_ssh_are_explained_rather_than_reinstalled() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 3,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 3,
            rules_installed: true,
            remote: true,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;
        let checks = diagnose(&facts);
        let Verdict::Problem { fix, .. } = find(&checks, "Permission to open devices") else {
            panic!("unopenable devices must be a problem");
        };
        let steps = fix.join("\n");
        assert!(steps.contains("This session is remote"), "{steps}");
        assert!(steps.contains("sudo"), "{steps}");
        assert!(!steps.contains("Install the udev rules"), "{steps}");
        assert!(!steps.contains("71-roadie-local.rules"), "{steps}");
        assert!(
            !steps.contains("udevadm"),
            "nothing changed the rules: {steps}"
        );
    }

    #[test]
    fn the_shipped_vendor_list_is_what_the_rules_file_grants() {
        let rules = include_str!("../../../../packaging/linux/udev/70-roadie.rules");
        for vendor in super::SHIPPED_HIDRAW_VENDORS {
            let line = format!(r#"SUBSYSTEM=="hidraw", ATTRS{{idVendor}}=="{vendor:04x}""#);
            assert!(rules.contains(&line), "{line} is not in 70-roadie.rules");
        }
        let granted = rules
            .lines()
            .filter(|line| line.starts_with(r#"SUBSYSTEM=="hidraw", ATTRS{idVendor}"#))
            .count();
        assert_eq!(
            granted,
            super::SHIPPED_HIDRAW_VENDORS.len(),
            "a vendor was added there only"
        );
    }

    #[test]
    fn a_via_board_is_recognised_by_its_usage_page_item_only() {
        use super::declares_via_page;
        // Usage Page 0xFF60, Usage 0x61, as QMK's raw HID descriptor opens.
        assert!(declares_via_page(&[
            0x06, 0x60, 0xff, 0x09, 0x61, 0xa1, 0x01
        ]));
        // A generic mouse: Usage Page 0x01.
        assert!(!declares_via_page(&[0x05, 0x01, 0x09, 0x02, 0xa1, 0x01]));
        // The same three bytes inside a four-byte Logical Maximum's data are
        // data, not a Usage Page item.
        assert!(!declares_via_page(&[0x27, 0x06, 0x60, 0xff, 0x00]));
        // A truncated descriptor is not a VIA board, nor a panic.
        assert!(!declares_via_page(&[0x06, 0x60]));
    }

    /// A container or VM with no USB passed through. Not a fault, and calling
    /// it one would have someone rewriting working udev rules.
    #[test]
    fn a_machine_with_no_hid_nodes_at_all_is_not_blamed_for_permissions() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 0,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        let checks = diagnose(&facts);
        let Verdict::Undetermined(detail) = find(&checks, "Permission to open devices") else {
            panic!("no nodes at all is not a permissions problem");
        };
        assert!(detail.contains("container"), "{detail}");
    }

    /// On macOS the grant is per program, and the single most common wrong
    /// belief is that granting it to the app grants it to the terminal.
    #[test]
    fn macos_advice_names_input_monitoring_and_that_it_is_per_program() {
        let facts = Facts {
            platform: Platform::MacOs,
            hid_devices: 0,
            cameras: 0,
            monitors: Monitors::default(),
            hidraw: None,
            ..healthy()
        };
        let checks = diagnose(&facts);
        let Verdict::Problem { detail, fix } = find(&checks, "Permission to open devices") else {
            panic!("no readable HID on macOS is a problem");
        };
        assert!(
            fix.iter().any(|step| step.contains("Input Monitoring")),
            "{fix:?}"
        );
        assert!(
            fix.iter().any(|step| step.contains("per program")),
            "{fix:?}"
        );
        // macOS gives no way to tell "not allowed" from "nothing attached".
        // Asserting the permission cause would send someone with an empty desk
        // into System Settings to fix nothing.
        assert!(
            detail.contains("cannot tell them apart"),
            "the ambiguity has to be stated, not papered over: {detail}"
        );
        assert!(
            fix.iter().any(|step| step.contains("reopen your terminal")),
            "a grant applies at process start; without this step nothing changes: {fix:?}"
        );
    }

    #[test]
    fn windows_has_nothing_to_grant_and_says_so() {
        let facts = Facts {
            platform: Platform::Windows,
            hidraw: None,
            ..healthy()
        };
        let checks = diagnose(&facts);
        assert!(matches!(
            find(&checks, "Permission to open devices"),
            Verdict::Fine(_)
        ));
    }

    /// A diagnostic that calls a working setup broken loses the trust it needs
    /// to be useful. The agent is optional for everything the CLI does.
    #[test]
    fn a_missing_agent_is_a_note_and_never_a_problem() {
        let mut facts = healthy();
        facts.agent_reachable = false;
        let checks = diagnose(&facts);
        assert!(matches!(
            find(&checks, "Background agent"),
            Verdict::Undetermined(_)
        ));
        assert_eq!(problems(&checks), 0, "a missing agent breaks nothing");
    }

    /// The normal state on a new machine — which is exactly the machine
    /// someone runs this on — must not read as a fault.
    #[test]
    fn a_machine_with_no_configuration_yet_is_not_broken() {
        let mut facts = healthy();
        facts.config_exists = false;
        let checks = diagnose(&facts);
        let Verdict::Undetermined(detail) = find(&checks, "Configuration") else {
            panic!("no config yet is normal");
        };
        assert!(detail.contains("normal state on a new machine"), "{detail}");
        assert_eq!(problems(&checks), 0);
    }

    #[test]
    fn nowhere_to_save_settings_is_a_real_problem() {
        let mut facts = healthy();
        facts.config = None;
        facts.config_exists = false;
        let checks = diagnose(&facts);
        let Verdict::Problem { fix, .. } = find(&checks, "Configuration") else {
            panic!("nowhere to save is a problem");
        };
        assert!(fix.iter().any(|step| step.contains("HOME")), "{fix:?}");
    }

    /// Someone who has just heard several checks read out should not have to
    /// scroll back to find the two things they actually have to do.
    #[test]
    fn the_steps_are_repeated_at_the_end_as_one_numbered_list() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 4,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;
        facts.config = None;

        let checks = diagnose(&facts);
        let text = render(&checks, problems(&checks));
        assert!(text.contains("things to fix"), "{text}");
        for step in ["  1. ", "  2. ", "  3. "] {
            assert!(text.contains(step), "missing {step} in:\n{text}");
        }
    }

    /// Numbering runs across every problem rather than restarting per check,
    /// so "step 5" means one thing rather than several.
    #[test]
    fn the_steps_are_numbered_once_across_the_whole_list() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 4,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: false,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;
        let checks = diagnose(&facts);
        let text = render(&checks, problems(&checks));
        let numbers: Vec<&str> = text
            .lines()
            .filter_map(|line| {
                line.trim()
                    .split('.')
                    .next()
                    .filter(|_| line.starts_with("  "))
            })
            .collect();
        assert_eq!(
            numbers,
            vec!["1", "2", "3", "4", "5", "6", "7"],
            "numbering must run straight through: {text}"
        );
    }

    /// The sweep over every shape the report takes, including the ones a
    /// machine with no hardware never produces.
    #[test]
    fn every_shape_of_diagnosis_is_worth_listening_to() {
        let machines = [
            ("a healthy machine", healthy()),
            (
                "a machine with no access",
                Facts {
                    hidraw: Some(Hidraw {
                        present: 3,
                        openable: 0,
                        writable: 0,
                        blocked_vendors: vec![0x0fd9],
                        blocked_covered: 0,
                        rules_installed: true,
                        remote: false,
                    }),
                    hid_devices: 0,
                    cameras: 0,
                    monitors: Monitors::default(),
                    ..healthy()
                },
            ),
            (
                "a partly reachable machine",
                Facts {
                    hidraw: Some(Hidraw {
                        present: 3,
                        openable: 1,
                        writable: 1,
                        blocked_vendors: vec![0x046d],
                        blocked_covered: 0,
                        rules_installed: true,
                        remote: false,
                    }),
                    ..healthy()
                },
            ),
            (
                "a mac with nothing readable",
                Facts {
                    platform: Platform::MacOs,
                    hidraw: None,
                    hid_devices: 0,
                    cameras: 0,
                    monitors: Monitors::default(),
                    ..healthy()
                },
            ),
            (
                "a machine with nowhere to save",
                Facts {
                    config: None,
                    config_exists: false,
                    layouts: 0,
                    ..healthy()
                },
            ),
        ];
        for (what, facts) in machines {
            let checks = diagnose(&facts);
            let problems = checks
                .iter()
                .filter(|check| matches!(check.verdict, Verdict::Problem { .. }))
                .count();
            let text = render(&checks, problems);
            crate::spoken::assert_listenable(&text, what);
            crate::spoken::assert_agrees(&text, what);
        }
    }

    /// Both renderings must carry the build and the platform.
    ///
    /// This output exists to be pasted into a bug report, and those are the
    /// first two things anyone reading one has to ask for. A round trip to
    /// find them out is a poor trade anywhere and a worse one for someone
    /// working by dictation.
    #[test]
    fn the_report_says_which_build_and_which_platform() {
        let checks = diagnose(&healthy());

        let text = render(&checks, 0);
        assert!(
            text.contains(env!("CARGO_PKG_VERSION")),
            "the text report does not name the version: {text}"
        );
        assert!(
            text.contains(std::env::consts::OS),
            "the text report does not name the platform: {text}"
        );

        let json: serde_json::Value =
            serde_json::from_str(&render_json(&checks)).expect("the report is JSON");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["platform"], std::env::consts::OS);

        // And it still reads well aloud with the new line in front of it.
        crate::spoken::assert_listenable(&text, "the report with provenance");
        crate::spoken::assert_agrees(&text, "the report with provenance");
    }

    /// The machine that looks fine and behaves worst.
    ///
    /// Reads succeed, so every listing works and nothing suggests a
    /// permissions problem — and every change fails, because this program
    /// writes. A check that only opened devices for reading would report
    /// all-clear on exactly this machine, which is the one most in need of
    /// being told.
    #[test]
    fn devices_that_can_be_read_but_not_written_are_a_problem() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 3,
            openable: 3,
            writable: 1,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        let checks = diagnose(&facts);
        let access = checks
            .iter()
            .find(|check| check.name == "Permission to open devices")
            .expect("the access check is always run");
        let Verdict::Problem { detail, fix } = &access.verdict else {
            panic!(
                "read-only access must be a problem, got {:?}",
                access.verdict
            );
        };
        assert!(
            detail.contains("written to"),
            "the detail must say which half is missing: {detail}"
        );
        assert!(
            fix.iter().any(|step| step.contains("0660")),
            "the fix must name the mode that grants writing: {fix:?}"
        );
        crate::spoken::assert_listenable(&render(&checks, 1), "the read-only verdict");
        crate::spoken::assert_agrees(&render(&checks, 1), "the read-only verdict");
    }

    /// And the ordinary machine, where everything readable is writable too,
    /// must not be told it has a problem it does not have.
    #[test]
    fn devices_that_can_be_both_read_and_written_are_fine() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 3,
            openable: 3,
            writable: 3,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        let checks = diagnose(&facts);
        let access = checks
            .iter()
            .find(|check| check.name == "Permission to open devices")
            .expect("the access check is always run");
        assert!(
            matches!(access.verdict, Verdict::Fine(_)),
            "got {:?}",
            access.verdict
        );
    }

    /// A consumer that gets a problem without its steps has half of what it
    /// needs, and no way to know the other half exists.
    #[test]
    fn a_problem_in_json_carries_its_steps_with_it() {
        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 2,
            openable: 0,
            writable: 0,
            blocked_vendors: vec![0x0fd9],
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;

        let text = render_json(&diagnose(&facts));
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        let checks = parsed["checks"].as_array().expect("an array");
        let problem = checks
            .iter()
            .find(|check| check["state"] == "problem")
            .expect("a problem");
        let steps = problem["steps"].as_array().expect("steps travel with it");
        assert_ne!(steps.len(), 0);
        assert!(
            text.contains("0fd9"),
            "the generated rule is in the JSON too"
        );
    }

    /// A check with nothing wrong carries no steps, rather than an empty list
    /// a consumer might render as a heading with nothing under it.
    #[test]
    fn a_check_with_nothing_wrong_has_no_steps_key() {
        let text = render_json(&diagnose(&healthy()));
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed["problems"], 0);
        for check in parsed["checks"].as_array().expect("an array") {
            assert_ne!(check["state"], "problem");
            assert!(check.get("steps").is_none(), "{check}");
        }
    }

    /// "1 thing(s)" is unbearable read aloud: a screen reader says "thing open
    /// paren s close paren".
    #[test]
    fn counts_use_words_rather_than_parenthesised_plurals() {
        assert_eq!(counted(1, "camera", "cameras"), "1 camera");
        assert_eq!(counted(0, "camera", "cameras"), "0 cameras");
        assert_eq!(counted(2, "camera", "cameras"), "2 cameras");

        let mut facts = healthy();
        facts.hidraw = Some(Hidraw {
            present: 4,
            openable: 0,
            writable: 0,
            blocked_vendors: Vec::new(),
            blocked_covered: 0,
            rules_installed: true,
            remote: false,
        });
        facts.hid_devices = 0;
        facts.cameras = 0;
        facts.config = Some(PathBuf::from("/x/config.toml"));
        let checks = diagnose(&facts);
        let text = render(&checks, problems(&checks));
        assert!(!text.contains("(s)"), "{text}");
    }

    /// Every problem has to carry at least one step. A finding that says
    /// something is wrong and nothing about what to do is a worse outcome
    /// than not checking.
    #[test]
    fn every_problem_carries_something_to_do_about_it() {
        let machines = [
            Facts {
                hidraw: Some(Hidraw {
                    present: 4,
                    openable: 0,
                    writable: 0,
                    blocked_vendors: Vec::new(),
                    blocked_covered: 0,
                    rules_installed: true,
                    remote: false,
                }),
                hid_devices: 0,
                cameras: 0,
                monitors: Monitors::default(),
                ..healthy()
            },
            Facts {
                hidraw: Some(Hidraw {
                    present: 4,
                    openable: 1,
                    writable: 1,
                    blocked_vendors: Vec::new(),
                    blocked_covered: 0,
                    rules_installed: true,
                    remote: false,
                }),
                ..healthy()
            },
            Facts {
                platform: Platform::MacOs,
                hidraw: None,
                hid_devices: 0,
                cameras: 0,
                monitors: Monitors::default(),
                ..healthy()
            },
            Facts {
                config: None,
                config_exists: false,
                ..healthy()
            },
        ];
        for facts in machines {
            for check in diagnose(&facts) {
                let Verdict::Problem { detail, fix } = check.verdict else {
                    continue;
                };
                assert!(!fix.is_empty(), "{} said '{detail}' and no fix", check.name);
                assert!(
                    fix.iter().all(|step| !step.trim().is_empty()),
                    "{} has an empty step",
                    check.name
                );
            }
        }
    }
}
