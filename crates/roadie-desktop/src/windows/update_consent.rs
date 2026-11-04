//! First-run prompt asking whether to enable the opt-in update check.
//!
//! Update checks default to off (the README's "no telemetry, no auto-update
//! poller" promise). This small window — shown once, gated by
//! [`AppSettings::update_prompt_seen`](roadie_core::config::AppSettings) —
//! is how a user opts in on first launch. Either choice marks the prompt seen
//! so it never reappears; "Enable" also runs one check immediately.

use crate::ui::theme::Typography as _;
use gpui::{
    App, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement as _, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_updater::Updater;

use crate::app::menu::{CloseWindow, Minimize, Zoom};
use crate::state::{AppState, StateEvent};
use crate::ui::theme;
use crate::windows::{self, AuxWindow};

/// Standalone first-run update-consent window root view.
pub struct UpdateConsentView {
    focus_handle: FocusHandle,
    appearance_obs: Option<Subscription>,
}

impl UpdateConsentView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            focus_handle,
            appearance_obs: None,
        }
    }
}

impl AuxWindow for UpdateConsentView {
    fn set_appearance_obs(&mut self, sub: Subscription) {
        self.appearance_obs = Some(sub);
    }
}

/// Open the first-run update-consent window.
pub fn open(cx: &mut App) {
    windows::open_or_focus(
        |reg| &mut reg.update_consent,
        "OpenRoadie",
        Size::new(px(380.), px(220.)),
        UpdateConsentView::new,
        cx,
    );
}

/// Persist the user's answer, run one check if they opted in, and close.
fn answer(enabled: bool, window: &mut Window, cx: &mut App) {
    AppState::update(cx, |state, cx| {
        state.record_update_consent(enabled);
        cx.emit(StateEvent::SettingsChanged);
    });
    if enabled && let Some(updater) = crate::platform::updater::shared(cx) {
        updater.update(cx, Updater::check);
    }
    window.remove_window();
}

impl Render for UpdateConsentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        theme::apply_ui_scale(window, cx);
        let pal = theme::palette(cx);

        let title = tr!("Check for updates?");
        let body = tr!(
            "OpenRoadie can check GitHub once per launch for a new version — query \
             only, no automatic download or telemetry. You can change this anytime \
             in Settings."
        );
        // A dialog, named and described, so the question and its terms are read
        // out as the window opens rather than left to be found after the buttons.
        v_flex()
            .id("update-consent")
            .role(gpui::accesskit::Role::Dialog)
            .aria_label(title.clone())
            .aria_description(body.clone())
            .size_full()
            .bg(pal.page)
            .text_color(pal.text_primary)
            .track_focus(&self.focus_handle)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            // Linux only: a client-side titlebar at the top of the window; the
            // centred content sits in the flex-column below it. macOS / Windows
            // keep their native titlebar.
            .when(cfg!(target_os = "linux"), |this| {
                this.child(windows::aux_title_bar(tr!("OpenRoadie"), cx))
            })
            .child(
                v_flex()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .p_6()
                    .child(crate::ui::a11y::heading("title", title, 1).text_heading())
                    .child(
                        div()
                            .max_w(px(320.))
                            .text_body()
                            .text_center()
                            .text_color(pal.text_muted)
                            .child(crate::ui::a11y::label("body", body)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .pt_2()
                            .child(
                                Button::new("update-consent-decline")
                                    .outline()
                                    .label(tr!("Not now"))
                                    .on_click(|_, window, cx| answer(false, window, cx)),
                            )
                            .child(
                                Button::new("update-consent-accept")
                                    .primary()
                                    .label(tr!("Enable"))
                                    .on_click(|_, window, cx| answer(true, window, cx)),
                            ),
                    ),
            )
    }
}
