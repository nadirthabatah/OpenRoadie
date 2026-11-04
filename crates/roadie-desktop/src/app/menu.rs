//! Application menus and actions.
//!
//! GPUI's menu support is driven by registered actions + a `Keymap`: the
//! platform layer reads bindings via `cx.set_menus` and stamps the matching
//! `keyEquivalent` onto each `NSMenuItem`. App-level actions (Hide, Quit)
//! get global listeners; window-level actions (Close, Minimize, Zoom) are
//! attached to window root views.
//!
//! On Linux/Windows the menus + key bindings are stored but never surfaced
//! in a top-of-screen bar — calling `install` there is a harmless no-op.

use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, actions};
use roadie_core::brand::{HELP_URL, RELEASES_URL, REPO_URL};
use url::Url;

use crate::state::AppState;
use crate::ui::battery::battery_charging_no_reading;

/// Keymap context installed on the main application surface.
pub(crate) const APP_KEY_CONTEXT: &str = "OpenRoadie";

actions!(
    roadie,
    [
        /// Move focus to the next control from a text field in a dialog.
        NextField,
        /// Move focus to the previous control from a text field in a dialog.
        PreviousField,
        /// Close the focused window.
        CloseWindow,
        /// Bring all OpenRoadie windows to the front.
        BringAllToFront,
        /// Check for an application update.
        CheckForUpdates,
        /// Hide the OpenRoadie window (macOS).
        Hide,
        /// Hide every other application (macOS).
        HideOthers,
        /// Minimize the active window.
        Minimize,
        /// Return from device details to the device gallery.
        NavigateBack,
        /// Open the About window.
        OpenAbout,
        /// Open the Add Device (pairing) window.
        OpenAddDevice,
        /// Open the monitors-and-lights window.
        OpenDesk,
        /// Open the user's OpenRoadie configuration folder.
        OpenConfigFolder,
        /// Open the OpenRoadie help page.
        OpenHelp,
        /// Open the latest release page.
        OpenLatestRelease,
        /// Open the OpenRoadie GitHub repository.
        OpenRepository,
        /// Open the Settings window.
        OpenSettings,
        /// Quit the application.
        Quit,
        /// Reveal every hidden application (macOS).
        ShowAll,
        /// Zoom (maximize) the active window.
        Zoom,
    ]
);

/// Wire global action handlers, key equivalents, and publish the menu bar.
pub fn install(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    }
    cx.on_action(|_: &Quit, cx| cx.quit());
    // Fallback for future windows that forget to attach a view-level
    // CloseWindow handler. Existing window roots handle this directly so the
    // focused window is removed during normal action dispatch.
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &OpenSettings, cx| crate::windows::settings::open(cx));
    cx.on_action(|_: &OpenAbout, cx| {
        crate::windows::settings::open_at(crate::windows::settings::SettingsPage::About, cx);
    });
    cx.on_action(|_: &OpenAddDevice, cx| crate::windows::add_device::open(cx));
    cx.on_action(|_: &OpenDesk, cx| crate::features::desk::view::open(cx));
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    cx.on_action(|_: &CheckForUpdates, cx| check_for_updates(cx));
    cx.on_action(|_: &OpenConfigFolder, cx| {
        if let Ok(path) = roadie_core::paths::config_dir()
            && let Some(url) = file_url(&path)
        {
            cx.open_url(&url);
        }
    });
    cx.on_action(|_: &OpenHelp, cx| cx.open_url(HELP_URL));
    cx.on_action(|_: &OpenLatestRelease, cx| cx.open_url(RELEASES_URL));
    cx.on_action(|_: &OpenRepository, cx| cx.open_url(REPO_URL));

    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-z", gpui_component::input::Undo, None),
        KeyBinding::new("cmd-shift-z", gpui_component::input::Redo, None),
        KeyBinding::new("cmd-x", gpui_component::input::Cut, None),
        KeyBinding::new("cmd-c", gpui_component::input::Copy, None),
        KeyBinding::new("cmd-v", gpui_component::input::Paste, None),
        KeyBinding::new("cmd-a", gpui_component::input::SelectAll, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-h", Hide, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("alt-left", NavigateBack, Some(APP_KEY_CONTEXT)),
        // A text field binds Tab to indent; in a dialog that kept focus in the
        // field. These win for fields inside one, being bound later.
        KeyBinding::new("tab", NextField, Some("FieldDialog > Input")),
        KeyBinding::new("shift-tab", PreviousField, Some("FieldDialog > Input")),
        KeyBinding::new("tab", NextField, Some("FieldDialog")),
        KeyBinding::new("shift-tab", PreviousField, Some("FieldDialog")),
    ]);

    cx.set_menus(menus(cx));
}

/// Re-publish the menu bar with the current locale's titles. Called after a
/// live language switch — unlike [`install`] it only restamps the menu strings,
/// leaving the already-registered action handlers and key bindings untouched.
pub fn rebuild(cx: &mut App) {
    cx.set_menus(menus(cx));
}

/// Run a manual update check and open Settings → Updates where its status is
/// rendered. Shared by the app menu and agent tray IPC commands.
pub fn check_for_updates(cx: &mut App) {
    if let Some(updater) = crate::platform::updater::shared(cx) {
        updater.update(cx, gpui_updater::Updater::check);
    }
    crate::windows::settings::open_at(crate::windows::settings::SettingsPage::Updates, cx);
}

fn menus(cx: &App) -> Vec<Menu> {
    vec![
        Menu {
            // The app menu's name is the product name, not a translatable string.
            name: "OpenRoadie".into(),
            disabled: false,
            items: vec![
                MenuItem::action(tr!("About OpenRoadie"), OpenAbout),
                MenuItem::action(tr!("Check for Updates…"), CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action(tr!("Settings…"), OpenSettings),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("Services", gpui::SystemMenuType::Services),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action(tr!("Hide OpenRoadie"), Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action(tr!("Hide Others"), HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action(tr!("Show All"), ShowAll),
                #[cfg(target_os = "macos")]
                MenuItem::separator(),
                MenuItem::action(tr!("Quit OpenRoadie"), Quit),
            ],
        },
        Menu {
            name: tr!("Edit"),
            disabled: false,
            items: vec![
                MenuItem::os_action(tr!("Undo"), gpui_component::input::Undo, OsAction::Undo),
                MenuItem::os_action(tr!("Redo"), gpui_component::input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(tr!("Cut"), gpui_component::input::Cut, OsAction::Cut),
                MenuItem::os_action(tr!("Copy"), gpui_component::input::Copy, OsAction::Copy),
                MenuItem::os_action(tr!("Paste"), gpui_component::input::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action(
                    tr!("Select All"),
                    gpui_component::input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            name: tr!("View"),
            disabled: false,
            items: vec![
                MenuItem::action(tr!("Settings…"), OpenSettings),
                MenuItem::action(tr!("About OpenRoadie"), OpenAbout),
                MenuItem::separator(),
                MenuItem::action(tr!("Open Configuration Folder"), OpenConfigFolder),
            ],
        },
        Menu {
            name: tr!("Device"),
            disabled: false,
            items: device_menu_items(cx),
        },
        Menu {
            name: tr!("Window"),
            disabled: false,
            items: vec![
                MenuItem::action(tr!("Close Window"), CloseWindow),
                MenuItem::separator(),
                MenuItem::action(tr!("Minimize"), Minimize),
                MenuItem::action(tr!("Zoom"), Zoom),
                MenuItem::separator(),
                MenuItem::action(tr!("Bring All to Front"), BringAllToFront),
            ],
        },
        Menu {
            name: tr!("Help"),
            disabled: false,
            items: vec![
                MenuItem::action(tr!("OpenRoadie Help"), OpenHelp),
                MenuItem::separator(),
                MenuItem::action(tr!("Open GitHub Repository"), OpenRepository),
                MenuItem::action(tr!("Latest Release"), OpenLatestRelease),
            ],
        },
    ]
}

fn device_menu_items(cx: &App) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::action(tr!("Add Device…"), OpenAddDevice),
        // Its own item rather than an entry in the device list below, because
        // a monitor and a Key Light are not paired peripherals: they are found
        // over a video cable and over the network, and neither appears in the
        // inventory that list is built from.
        MenuItem::action(tr!("The Desk…"), OpenDesk),
        MenuItem::separator(),
    ];

    let state = AppState::try_global(cx);
    match state.as_ref().map(|state| state.read(cx)) {
        Some(state) if !state.devices().is_empty() => {
            for record in state.devices() {
                let title = match &record.battery {
                    Some(battery) if battery_charging_no_reading(battery) => {
                        format!("{} · {}", record.display_name, tr!("Charging"))
                    }
                    Some(battery) => format!("{} · {}%", record.display_name, battery.percentage),
                    None => record.display_name.clone(),
                };
                items.push(MenuItem::action(title, gpui::NoAction).disabled(true));
            }
        }
        _ => {
            items
                .push(MenuItem::action(tr!("No devices connected"), gpui::NoAction).disabled(true));
        }
    }

    items
}

pub(crate) fn file_url(path: &std::path::Path) -> Option<String> {
    Url::from_file_path(path).ok().map(Into::into)
}
