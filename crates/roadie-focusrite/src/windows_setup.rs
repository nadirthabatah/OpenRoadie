//! The one-time Windows step that makes the control interface reachable.
//!
//! Windows matches a Focusrite's vendor interface to Microsoft's own WinUSB
//! driver, but a WinUSB interface is only published for programs to open
//! under an interface GUID, and these devices name none. So the driver is
//! loaded, nothing holds the interface, and still nothing can open it.
//! Verified on a Vocaster Two, 2026-11-03: `Device Parameters` carried no
//! `DeviceInterfaceGUIDs`, and no device interface existed for the node.
//!
//! The fix is the one an INF's `AddReg` (or Zadig) would make: write a GUID
//! into the interface's `Device Parameters` and restart that one interface.
//! The audio interfaces are separate device nodes and are never touched.
//! Writing under `HKLM` needs an administrator, so `prepare` raises one
//! Windows permission prompt and does nothing else that needs it.

/// The interface GUID OpenRoadie publishes a Focusrite control interface
/// under. Any fixed GUID works; it only has to be the same on every run.
pub const INTERFACE_GUID: &str = "{8E4F2A6C-3B1D-4C7A-9E52-0F6D7B3A1C94}";

/// Whether a claim failure is Windows' unpublished-interface case.
///
/// Matched on `nusb`'s wording, which is the only signal it gives: the
/// error kind it uses, `Unsupported`, also covers a non-WinUSB driver.
#[must_use]
pub fn is_unpublished(reason: &str) -> bool {
    reason.contains("DeviceInterfaceGUIDs")
}

/// The PowerShell that publishes every Focusrite WinUSB interface still
/// missing a GUID, and restarts each one it changed.
///
/// An interface that already carries a GUID is left alone, so a vendor
/// driver's own registration is never overwritten and a second run changes
/// nothing. The exit status is the number of interfaces it could not fix.
#[must_use]
pub fn script() -> String {
    format!(
        r"$failed = 0
Get-PnpDevice -PresentOnly | Where-Object {{
    $_.InstanceId -match '^USB\\VID_1235&PID_[0-9A-F]{{4}}&MI_[0-9A-F]{{2}}\\' -and
    (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_Service).Data -eq 'WINUSB'
}} | ForEach-Object {{
    $key = 'HKLM:\SYSTEM\CurrentControlSet\Enum\' + $_.InstanceId + '\Device Parameters'
    $have = (Get-ItemProperty -Path $key -ErrorAction SilentlyContinue).DeviceInterfaceGUIDs
    if (-not $have) {{
        try {{
            New-ItemProperty -Path $key -Name DeviceInterfaceGUIDs -PropertyType MultiString -Value @('{INTERFACE_GUID}') -Force -ErrorAction Stop | Out-Null
            pnputil /restart-device $_.InstanceId | Out-Null
            if ($LASTEXITCODE -ne 0) {{ $failed++ }}
        }} catch {{ $failed++ }}
    }}
}}
exit $failed
"
    )
}

/// How `prepare` ended. (Not a link: `prepare` exists only on Windows.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prepared {
    /// Every interface that needed it is published now.
    Done,
    /// The permission prompt was declined, or never answered.
    Declined,
    /// This many interfaces could not be changed.
    Failed(i32),
}

/// Run [`script`] as administrator, raising one Windows permission prompt,
/// and wait for it.
///
/// # Errors
///
/// Fails when the script cannot be written to the temporary directory or
/// PowerShell cannot be started at all.
#[cfg(target_os = "windows")]
pub fn prepare() -> std::io::Result<Prepared> {
    let path = std::env::temp_dir().join("openroadie-focusrite-prepare.ps1");
    std::fs::write(&path, script())?;
    // The outer PowerShell exists only to raise the prompt: `-Verb RunAs` is
    // the documented way to ask for elevation without any native code here.
    let elevate = format!(
        "try {{ $p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"' -ErrorAction Stop; \
         exit $p.ExitCode }} catch {{ exit -1 }}",
        path.display()
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &elevate])
        .status();
    let _ = std::fs::remove_file(&path);
    Ok(match status?.code() {
        Some(0) => Prepared::Done,
        Some(-1) | None => Prepared::Declined,
        Some(n) => Prepared::Failed(n),
    })
}

#[cfg(test)]
mod tests {
    use super::{INTERFACE_GUID, is_unpublished, script};

    #[test]
    fn nusbs_unpublished_wording_is_recognised_and_a_busy_driver_is_not() {
        // Literal `nusb` 0.1 messages: the first is what the Vocaster gave.
        assert!(is_unpublished(
            "Could not find DeviceInterfaceGUIDs in registry. WinUSB driver may not be \
             correctly installed for this interface."
        ));
        assert!(!is_unpublished(
            r#"Interface driver is "usbaudio2", not WinUSB"#
        ));
    }

    #[test]
    fn the_script_never_overwrites_a_guid_and_touches_only_focusrite_interfaces() {
        let script = script();
        assert!(script.contains(INTERFACE_GUID));
        assert!(script.contains("if (-not $have)"));
        // An interface node, never the composite parent the audio hangs off.
        assert!(script.contains(r"VID_1235&PID_[0-9A-F]{4}&MI_[0-9A-F]{2}\\"));
        assert!(script.contains("-eq 'WINUSB'"));
    }
}
