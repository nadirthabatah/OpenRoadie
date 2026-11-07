use std::{
    fmt::{self, Write as _},
    process::ExitCode,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Args;
use roadie_camera::Camera;
use roadie_core::device::{BatteryInfo, DeviceInventory, DeviceModelInfo, PairedDevice};
use roadie_ipc::{AgentSnapshot, AgentStatus, ClientKind, PROTOCOL_VERSION, client};
use tarpc::context;

#[derive(Debug, Args)]
pub struct ListArgs {}

/// Exit status for "the scan succeeded, but nothing is connected" — distinct
/// from the failure status a real enumeration error produces.
const NOTHING_FOUND: u8 = 2;

/// Print every connected receiver, paired device and Logitech webcam.
///
/// Reads the running agent's inventory when one is reachable — the agent is
/// the process that actually holds device permissions, so its answer is the
/// GUI's answer and no second identity opens the same HID nodes. Falls back
/// to direct enumeration (this process's own permission identity) when no
/// agent responds; the provenance goes to stderr so scripts keep parsing
/// stdout.
///
/// Returns the `NOTHING_FOUND` status when neither a HID++ device nor a webcam
/// is present, so scripts can tell "no hardware" apart from a failed
/// enumeration.
pub async fn run(_args: ListArgs) -> Result<ExitCode> {
    let (inventories, agent_status) = if let Some(snapshot) = agent_snapshot().await {
        eprintln!("(inventory read from the running agent)");
        (snapshot.inventory, Some(snapshot.status))
    } else {
        if cfg!(target_os = "macos") {
            eprintln!(
                "(no agent reachable — reading hardware directly; macOS judges this \
                 process's Input Monitoring grant, not the agent's)"
            );
        } else {
            eprintln!("(no agent reachable — reading hardware directly)");
        }
        let inventories = roadie_hid::enumerate()
            .await
            .context("failed to enumerate HID++ devices")?;
        (inventories, None)
    };
    let cameras = roadie_camera::enumerate_cameras();

    if inventories.is_empty() && cameras.is_empty() {
        println!("No Logitech HID++ devices or webcams found.");
        println!();
        // This command covers Logitech devices only, which is the wrong answer
        // to give someone whose Stream Deck or macro pad is plugged in and
        // working. Naming the two commands that do cover it costs two lines
        // and saves the conclusion that nothing is supported.
        println!("This command lists Logitech devices only.");
        println!("  roadie devices  — every peripheral attached, whoever made it");
        println!("  roadie doctor   — why nothing is being found, and what to do");
        println!();
        print_empty_notes(agent_status.as_ref());
        return Ok(ExitCode::from(NOTHING_FOUND));
    }

    for (i, inv) in inventories.iter().enumerate() {
        if i != 0 {
            println!();
        }
        print_inventory(inv);
    }

    if !cameras.is_empty() {
        if !inventories.is_empty() {
            println!();
        }
        print_cameras(&cameras);
    }

    Ok(ExitCode::SUCCESS)
}

/// One agent snapshot, or `None` when the CLI should read hardware itself:
/// no agent listening, a hung handshake, a protocol mismatch, or a stalled
/// snapshot call.
async fn agent_snapshot() -> Option<AgentSnapshot> {
    let conn = tokio::time::timeout(Duration::from_secs(2), client::connect())
        .await
        .ok()?
        .ok()?;
    if conn.version != PROTOCOL_VERSION {
        eprintln!(
            "note: the agent speaks protocol v{}, this CLI expects v{PROTOCOL_VERSION} — \
             reading hardware directly",
            conn.version
        );
        return None;
    }
    // Identify as a CLI so a dormant agent (launch-at-login off, started at
    // login) serves this query without arming its whole input stack.
    tokio::time::timeout(
        Duration::from_secs(2),
        conn.client
            .declare_client(context::current(), ClientKind::Cli),
    )
    .await
    .ok()?
    .ok()?;
    tokio::time::timeout(
        Duration::from_secs(5),
        conn.client.snapshot(context::current()),
    )
    .await
    .ok()?
    .ok()
}

/// Why the list is empty. With an agent status in hand the reason is known;
/// without one, fall back to the generic checklist.
fn print_empty_notes(status: Option<&AgentStatus>) {
    match status {
        Some(status) if !status.input_monitoring_granted => {
            println!("Notes:");
            println!(
                "  - The agent does not hold Input Monitoring. Grant it to OpenRoadie Agent: \
                 System Settings → Privacy & Security → Input Monitoring (the + picker \
                 cannot browse into the app bundle — use Go to Folder)."
            );
        }
        Some(status) if status.hid_open_failures => {
            println!("Notes:");
            println!(
                "  - Input Monitoring is granted, but the agent's device opens keep \
                 failing — another app may hold the devices (quit Logi Options+), or \
                 macOS is serving a stale permission session: log out and back in."
            );
        }
        _ => {
            println!("Notes:");
            if cfg!(any(target_os = "macos", target_os = "windows")) {
                println!(
                    "  - Quit Logi Options+ if it is installed: it holds Logitech devices \
                     and both programs fight over HID++ access."
                );
            }
            if cfg!(target_os = "macos") {
                println!(
                    "  - A Bluetooth-direct mouse (e.g. Lift, Signature) needs Input \
                     Monitoring permission: System Settings → Privacy & Security → Input \
                     Monitoring."
                );
            }
            // Bolt, Unifying, Nano and Lightspeed receivers are all recognised
            // now; this note once said only Bolt was, long after it stopped
            // being true.
            println!(
                "  - A receiver this build does not recognise still appears in roadie \
                 devices, with the two numbers a device-support request needs."
            );
            println!("  - roadie doctor says why a device could not be opened.");
        }
    }
}

fn print_cameras(cameras: &[Camera]) {
    println!("Cameras ({} Logitech UVC)", cameras.len());
    for cam in cameras {
        println!(
            "  - {} (camera, vid={:04x} pid={:04x}{caps}, id={})",
            cam.name,
            cam.vendor_id,
            cam.product_id,
            cam.unique_id,
            caps = CameraCapabilitiesDisplay {
                resolution: cam.max_resolution,
                fps: cam.max_fps,
            },
        );
    }
}

struct CameraCapabilitiesDisplay {
    resolution: Option<(u32, u32)>,
    fps: Option<u32>,
}

impl fmt::Display for CameraCapabilitiesDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.resolution, self.fps) {
            (Some((width, height)), Some(fps)) => {
                write!(f, ", up to {width}x{height}@{fps}")
            }
            (Some((width, height)), None) => write!(f, ", up to {width}x{height}"),
            _ => Ok(()),
        }
    }
}

fn print_inventory(inv: &DeviceInventory) {
    let uid = inv.receiver.unique_id.as_deref().unwrap_or("—");
    println!(
        "{} ({}, vid={:04x} pid={:04x})",
        inv.receiver.name, uid, inv.receiver.vendor_id, inv.receiver.product_id
    );

    if inv.paired.is_empty() {
        println!("  no paired devices");
        return;
    }

    for d in &inv.paired {
        println!("  - {}", PairedDeviceDisplay(d));
        if let Some(model) = d.model_info.as_ref() {
            println!("    {}", DeviceModelDisplay(model));
        }
    }
}

struct PairedDeviceDisplay<'a>(&'a PairedDevice);

impl fmt::Display for PairedDeviceDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let device = self.0;
        let dot = if device.online { "●" } else { "○" };
        let codename = device.codename.as_deref().unwrap_or("Unknown device");
        write!(
            f,
            "slot {} {dot} {codename} ({}, ",
            device.slot,
            LowercaseDebug(device.kind)
        )?;
        match device.wpid {
            Some(wpid) => write!(f, "wpid={wpid:04x}, ")?,
            None => write!(f, "wpid=?, ")?,
        }
        match device.battery.as_ref() {
            Some(battery) => write!(f, "{}", BatteryDisplay(battery))?,
            None => write!(f, "battery=—")?,
        }
        write!(f, ")")
    }
}

struct BatteryDisplay<'a>(&'a BatteryInfo);

impl fmt::Display for BatteryDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let battery = self.0;
        write!(
            f,
            "battery={}% {} ({})",
            battery.percentage,
            LowercaseDebug(battery.level),
            LowercaseDebug(battery.status)
        )
    }
}

struct DeviceModelDisplay<'a>(&'a DeviceModelInfo);

impl fmt::Display for DeviceModelDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let model = self.0;
        write!(f, "     model_ids=[")?;
        let mut separator = "";
        for id in model.model_ids {
            write!(f, "{separator}{id:04x}")?;
            separator = ",";
        }
        write!(
            f,
            "] ext={:02x} serial={} unit_id=",
            model.extended_model_id,
            model.serial_number.as_deref().unwrap_or("—")
        )?;
        for byte in model.unit_id {
            write!(f, "{byte:02x}")?;
        }
        write!(f, " transports=")?;

        separator = "";
        for (enabled, name) in [
            (model.transports.usb, "usb"),
            (model.transports.equad, "equad"),
            (model.transports.btle, "btle"),
            (model.transports.bluetooth, "bt"),
        ] {
            if enabled {
                write!(f, "{separator}{name}")?;
                separator = "+";
            }
        }
        if separator.is_empty() {
            write!(f, "—")?;
        }

        Ok(())
    }
}

/// The CLI historically rendered these enums by lowercasing their `Debug`
/// names. Keep that exact spelling without allocating an intermediate string.
struct LowercaseDebug<T>(T);

impl<T: fmt::Debug> fmt::Display for LowercaseDebug<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(LowercaseWriter(f), "{:?}", self.0)
    }
}

struct LowercaseWriter<'a, 'b>(&'a mut fmt::Formatter<'b>);

impl fmt::Write for LowercaseWriter<'_, '_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for character in value.chars().flat_map(char::to_lowercase) {
            self.0.write_char(character)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod format_tests {
    use roadie_core::device::{BatteryLevel, BatteryStatus, DeviceKind, DeviceTransports};

    use super::{
        BatteryDisplay, CameraCapabilitiesDisplay, DeviceModelDisplay, PairedDevice,
        PairedDeviceDisplay,
    };
    use super::{BatteryInfo, DeviceModelInfo};

    fn base_device() -> PairedDevice {
        PairedDevice {
            slot: 1,
            codename: Some("MX Master 3S".to_string()),
            wpid: Some(0x4082),
            kind: DeviceKind::Mouse,
            online: true,
            battery: None,
            model_info: None,
            capabilities: None,
        }
    }

    #[test]
    fn online_device_uses_filled_dot_and_reports_fields() {
        let d = base_device();
        let out = PairedDeviceDisplay(&d).to_string();
        assert_eq!(out, "slot 1 ● MX Master 3S (mouse, wpid=4082, battery=—)");
    }

    #[test]
    fn offline_device_uses_hollow_dot() {
        let mut d = base_device();
        d.online = false;
        let out = PairedDeviceDisplay(&d).to_string();
        assert!(out.starts_with("slot 1 ○ "));
    }

    #[test]
    fn missing_codename_and_wpid_fall_back_to_placeholders() {
        let mut d = base_device();
        d.codename = None;
        d.wpid = None;
        let out = PairedDeviceDisplay(&d).to_string();
        assert_eq!(out, "slot 1 ● Unknown device (mouse, wpid=?, battery=—)");
    }

    #[test]
    fn battery_info_is_embedded_when_present() {
        let mut d = base_device();
        d.battery = Some(BatteryInfo {
            percentage: 42,
            level: BatteryLevel::Low,
            status: BatteryStatus::Discharging,
        });
        let out = PairedDeviceDisplay(&d).to_string();
        assert!(out.contains("battery=42% low (discharging)"));
    }

    #[test]
    fn battery_status_debug_names_are_lowercased_verbatim() {
        // `ChargingSlow`'s Debug form has no separator; lowercasing alone
        // yields "chargingslow", not "charging_slow" or "charging slow".
        let b = BatteryInfo {
            percentage: 10,
            level: BatteryLevel::Critical,
            status: BatteryStatus::ChargingSlow,
        };
        assert_eq!(
            BatteryDisplay(&b).to_string(),
            "battery=10% critical (chargingslow)"
        );
    }

    #[test]
    fn camera_capabilities_include_resolution_and_optional_fps() {
        assert_eq!(
            CameraCapabilitiesDisplay {
                resolution: Some((1920, 1080)),
                fps: Some(60),
            }
            .to_string(),
            ", up to 1920x1080@60"
        );
        assert_eq!(
            CameraCapabilitiesDisplay {
                resolution: Some((1920, 1080)),
                fps: None,
            }
            .to_string(),
            ", up to 1920x1080"
        );
        assert_eq!(
            CameraCapabilitiesDisplay {
                resolution: None,
                fps: Some(60),
            }
            .to_string(),
            ""
        );
    }

    fn base_model() -> DeviceModelInfo {
        DeviceModelInfo {
            entity_count: 1,
            serial_number: None,
            unit_id: [0x00, 0x01, 0x02, 0x03],
            transports: DeviceTransports::default(),
            model_ids: [0xb042, 0, 0],
            extended_model_id: 0x02,
        }
    }

    #[test]
    fn model_with_no_transports_shows_placeholder_and_missing_serial() {
        let m = base_model();
        let out = DeviceModelDisplay(&m).to_string();
        assert_eq!(
            out,
            "     model_ids=[b042,0000,0000] ext=02 serial=— unit_id=00010203 transports=—"
        );
    }

    #[test]
    fn model_transports_join_in_declared_field_order() {
        let mut m = base_model();
        m.transports = DeviceTransports {
            usb: true,
            equad: false,
            btle: true,
            bluetooth: true,
        };
        m.serial_number = Some("SN123".to_string());
        let out = DeviceModelDisplay(&m).to_string();
        assert!(out.contains("transports=usb+btle+bt"));
        assert!(out.contains("serial=SN123"));
    }
}
