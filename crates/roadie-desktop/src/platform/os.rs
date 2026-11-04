//! Best-effort host OS version string for the diagnostics report, syncing the
//! native window chrome (titlebar) to the in-app appearance preference, and the
//! operating system's high-contrast and text-size settings.

use roadie_core::config::Appearance;

/// The OS product version (e.g. `"15.5"` on macOS), or `None` when unavailable.
#[must_use]
// Not `expect`: only the macOS arm returns `Some`, so off macOS the lint never
// fires and the expectation would go unfulfilled.
#[expect(clippy::allow_attributes, reason = "see above")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "Option is the cross-platform contract; non-macOS arms return None"
)]
pub fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
        Some(if v.patchVersion == 0 {
            format!("{}.{}", v.majorVersion, v.minorVersion)
        } else {
            format!("{}.{}.{}", v.majorVersion, v.minorVersion, v.patchVersion)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Sync the **whole app's** native chrome (system titlebar, traffic lights) to
/// the appearance preference, so a forced light/dark theme isn't betrayed by a
/// titlebar that still tracks the OS. `System` clears the override (the chrome
/// follows the OS, matching the resolved theme); `Light` / `Dark` pin the
/// matching `NSAppearance`. No-op off macOS, where window chrome isn't ours to
/// paint this way.
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "reading the framework's NSAppearanceName statics to set NSApp.appearance"
)]
pub fn set_app_appearance(appearance: Appearance) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let named = match appearance {
        Appearance::System => None,
        // SAFETY: `NSAppearanceNameAqua` is an AppKit `extern static` typed
        // `&'static NSAppearanceName`; its non-lazy binding is resolved when the
        // framework loads, before `main`, and AppKit never reassigns it — so
        // this is a shared read of a live `&'static` of the declared type.
        Appearance::Light => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
        // SAFETY: `NSAppearanceNameDarkAqua` is an AppKit `extern static` typed
        // `&'static NSAppearanceName`; its non-lazy binding is resolved when the
        // framework loads, before `main`, and AppKit never reassigns it — so
        // this is a shared read of a live `&'static` of the declared type.
        Appearance::Dark => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
    };
    NSApplication::sharedApplication(mtm).setAppearance(named.as_deref());
}

#[cfg(not(target_os = "macos"))]
pub fn set_app_appearance(_appearance: Appearance) {}

/// Whether the operating system asks apps to draw in high contrast: Windows'
/// contrast themes, or the GNOME high-contrast switch (and a `HighContrast`
/// GTK theme, which other desktops set instead). Read whenever the theme is
/// applied, so turning it on in the OS takes effect at the next appearance
/// change or relaunch.
#[cfg(target_os = "windows")]
#[expect(
    unsafe_code,
    reason = "SystemParametersInfoW is the only way to read the contrast setting"
)]
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};

    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>()
            .try_into()
            .unwrap_or(u32::MAX),
        dwFlags: 0,
        lpszDefaultScheme: std::ptr::null_mut(),
    };
    // SAFETY: `contrast` is a live, correctly sized HIGHCONTRASTW whose
    // `cbSize` says so, which is what SPI_GETHIGHCONTRAST writes into; the
    // call only reads the user's setting.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&raw mut contrast).cast(),
            0,
        )
    };
    read != 0 && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// See the Windows twin.
#[cfg(target_os = "linux")]
pub fn high_contrast() -> bool {
    let named = |theme: &str| theme.to_ascii_lowercase().contains("highcontrast");
    if std::env::var("GTK_THEME").is_ok_and(|theme| named(&theme)) {
        return true;
    }
    let setting = |schema: &str, key: &str| {
        std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    setting("org.gnome.desktop.a11y.interface", "high-contrast").is_some_and(|on| on == "true")
        || setting("org.gnome.desktop.interface", "gtk-theme").is_some_and(|theme| named(&theme))
}

/// See the Windows twin: here, Accessibility, Display, Increase contrast.
#[cfg(target_os = "macos")]
pub fn high_contrast() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast()
}

/// See the Windows twin.
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub fn high_contrast() -> bool {
    false
}

/// The operating system's text-size multiplier: Windows' "Make text bigger"
/// (Accessibility, Text size) or GNOME's text scaling factor, `1.0` when
/// unset. Read once per process — the setting is applied at launch.
#[must_use]
pub fn text_scale() -> f32 {
    static SCALE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| read_text_scale().unwrap_or(1.0).clamp(1.0, 2.25))
}

#[cfg(target_os = "windows")]
#[expect(
    unsafe_code,
    reason = "RegGetValueW is the only way to read the text-size setting"
)]
fn read_text_scale() -> Option<f32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

    let key: Vec<u16> = "Software\\Microsoft\\Accessibility\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "TextScaleFactor\0".encode_utf16().collect();
    let mut percent: u32 = 0;
    let mut size: u32 = 4;
    // SAFETY: both names are NUL-terminated UTF-16 that outlive the call, and
    // `percent` is a live u32 whose size `size` states — all a DWORD read writes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut percent).cast(),
            &raw mut size,
        )
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a percentage between 100 and 225"
    )]
    (status == 0).then_some(percent as f32 / 100.0)
}

#[cfg(target_os = "linux")]
fn read_text_scale() -> Option<f32> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn read_text_scale() -> Option<f32> {
    None
}
