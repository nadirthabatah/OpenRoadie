//! The desk panel: everything on the desk that is not a HID++ peripheral.
//!
//! A thin drawing of [`DeskModel`]. Everything that decides what appears here
//! lives in that module, where it can be tested; these files are layout, the
//! slider entities, and the calls to the agent. One child module per family,
//! because a monitor, a light, a Stream Deck, a preamp and a controller have
//! almost nothing in common but this window.
//!
//! Some controls the command line offers are deliberately missing.
//!
//! **Power** and **saving to the monitor's own memory** are absent for the
//! reason the wire types give: a monitor powered off over DDC may stop
//! answering DDC, and that memory wears out. Both belong behind a prompt that
//! says so.
//!
//! **Switching input** is absent for a reason of its own, which applies only
//! here. On the command line it is a deliberate typed sentence; in a panel it
//! is one mis-click, and the cost of the mistake is that the screen goes dark
//! and the way back is the same panel you can no longer see. So the current
//! input is shown and not offered — reading it is the useful half, and it is
//! the half that cannot go wrong.
//!
//! **Mass storage mode** on an audio interface is shown and not offered: it
//! only takes effect after the interface is power-cycled, and a switch whose
//! result appears at the next unplug is a switch that looks broken.

mod audio;
mod decks;
mod displays;
mod lights;
mod peripherals;

use std::collections::BTreeMap;

use gpui::{
    App, AsyncApp, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    WeakEntity, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{Disableable as _, h_flex, scroll::ScrollableElement as _, v_flex};
use roadie_ipc::desk::{
    AudioInterfaceSummary, ControllerSummary, DisplayControl, DisplaySummary, MacroPadSummary,
    NetworkLightSummary, StreamDeckSummary,
};

use tokio::sync::mpsc;

use super::model::DeskModel;
use crate::services::ipc::{Command, DeskCommand};
use crate::state::AppState;
use crate::ui::a11y;
use crate::ui::components::control_button;
use crate::ui::theme::{self, Palette, Typography as _};
use crate::windows::AuxWindow;

pub(super) use lights::LightKnob;

/// A slider, and the value it is currently showing.
///
/// The second half is the point. A slider is dragged to where a person let go
/// of it, and the device then answers with what it actually took — which is
/// often somewhere else, because a monitor clamps to its own maximum and a Key
/// Light has a brightness floor. Without remembering what the slider was last
/// told, there is nothing to compare the device's answer against, and the thumb
/// stays where the drag ended while the number beside it says something else.
struct Slot {
    state: Entity<a11y::LevelModel>,
    shown: u16,
}

pub struct DeskPanel {
    /// Held so the window keeps following the OS light/dark setting; dropping
    /// it strands the window on whichever theme it opened in.
    appearance: Option<Subscription>,
    model: DeskModel,
    /// One slider per monitor control, keyed by the monitor's handle so two
    /// monitors cannot share a slider's state.
    display_sliders: BTreeMap<(String, DisplayControl), Slot>,
    /// The same for lights.
    light_sliders: BTreeMap<(String, LightKnob), Slot>,
    /// Held for their lifetime, not detached: a slider release that outlives
    /// this panel has nothing left to update.
    subscriptions: Vec<Subscription>,
    /// The scans making up the current refresh. Replaced wholesale on a
    /// refresh, which cancels whatever the previous one was still waiting for —
    /// the answers would be dropped by the generation fence anyway, so waiting
    /// for them is only a slower way to ignore them.
    scans: Vec<Task<()>>,
    /// One in-flight write per thing that can be written.
    ///
    /// Keyed rather than accumulated for two reasons. It bounds the map by
    /// what is on screen instead of growing for the life of the window. And
    /// starting a second write to the same control cancels the first, which is
    /// what dragging a slider twice should mean: the newer value wins, and the
    /// older reply is not left to arrive afterwards and put the older number
    /// back.
    writes: BTreeMap<WriteKey, Task<()>>,
    /// Phantom-power warnings waiting to be accepted, by interface and input.
    ///
    /// The sentence comes from the agent rather than from here, so the words
    /// somebody reads before switching 48 volts on are the same words the
    /// command line reads out — written once, beside the code that knows what
    /// the risk is.
    pending_phantom: BTreeMap<(String, u16), String>,
    /// What the last write to a device said, when it refused.
    ///
    /// Kept per device so a refusal stays next to the thing that refused,
    /// rather than in one banner that cannot say which row it belongs to.
    failures: BTreeMap<String, String>,
    /// Tracked by the window's heading and focused when it opens. Nothing
    /// else here takes focus on its own, and with nothing focused a screen
    /// reader announced the window and Tab went nowhere.
    title_focus: gpui::FocusHandle,
}

/// What a write is about, so a newer one can replace it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum WriteKey {
    /// One control on one monitor.
    Display(String, DisplayControl),
    /// One knob on one light.
    Light(String, LightKnob),
    /// A light's power, which is not a knob.
    LightPower(String),
    /// A Stream Deck, which has one write at a time and no separate controls:
    /// its brightness and its reset both go through the same call.
    Deck(String),
    /// One input on one audio interface.
    AudioInput(String, u16),
}

impl DeskPanel {
    /// Take a finished write out of [`Self::writes`], so the controls it held
    /// greyed out come back.
    ///
    /// Only the write in the slot ever gets here: starting a newer one for the
    /// same key replaces it, and dropping it cancels it first. Left in place,
    /// a finished write kept its controls disabled until the window closed —
    /// one step of gain and both gain buttons were gone.
    fn finish_write(panel: &WeakEntity<Self>, key: &WriteKey, cx: &mut AsyncApp) {
        let _ = panel.update(cx, |panel, cx| {
            panel.writes.remove(key);
            cx.notify();
        });
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title_focus = cx.focus_handle();
        title_focus.focus(window, cx);
        let mut panel = Self {
            appearance: None,
            model: DeskModel::default(),
            display_sliders: BTreeMap::new(),
            light_sliders: BTreeMap::new(),
            subscriptions: Vec::new(),
            scans: Vec::new(),
            writes: BTreeMap::new(),
            pending_phantom: BTreeMap::new(),
            failures: BTreeMap::new(),
            title_focus,
        };
        panel.refresh(cx);
        panel
    }

    /// Ask the agent what is on the desk.
    ///
    /// Every family is asked for at once and answers independently, so a desk
    /// with a Stream Deck and no lights does not wait the full multicast
    /// window before showing the deck. The monitor scan is the one that runs
    /// on after its list, because each monitor's readings are a second call.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(sender) = AppState::try_read(cx).map(AppState::ipc_sender) else {
            return;
        };
        let generation = self.model.begin_scan();
        // Sliders belong to the handles of the scan that made them.
        self.display_sliders.clear();
        self.light_sliders.clear();
        self.subscriptions.clear();
        // A warning nobody answered belongs to the desk as it was; after a
        // rescan the input it named may not be there any more.
        self.pending_phantom.clear();
        self.failures.clear();

        // Replacing the vector cancels the previous scan's tasks.
        self.scans = vec![
            Self::scan_displays(sender.clone(), generation, cx),
            Self::scan(
                sender.clone(),
                generation,
                DeskCommand::ListNetworkLights,
                DeskModel::accept_lights,
                cx,
            ),
            Self::scan(
                sender.clone(),
                generation,
                DeskCommand::ListStreamDecks,
                DeskModel::accept_decks,
                cx,
            ),
            Self::scan(
                sender.clone(),
                generation,
                DeskCommand::ListAudioInterfaces,
                DeskModel::accept_interfaces,
                cx,
            ),
            Self::scan(
                sender.clone(),
                generation,
                DeskCommand::ListControllers,
                DeskModel::accept_controllers,
                cx,
            ),
            Self::scan(
                sender,
                generation,
                DeskCommand::ListMacroPads,
                DeskModel::accept_pads,
                cx,
            ),
        ];
    }

    /// Ask for one list and hand it to the model.
    ///
    /// Every family but the monitors is exactly this: one call, one answer,
    /// one fenced hand-off. Writing it once means a family cannot quietly skip
    /// the generation fence, which is the bug this shape exists to prevent.
    fn scan<T: Send + 'static>(
        sender: mpsc::UnboundedSender<Command>,
        generation: u64,
        make: impl FnOnce(tokio::sync::oneshot::Sender<T>) -> DeskCommand + Send + 'static,
        accept: impl FnOnce(&mut DeskModel, u64, T) -> bool + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender.send(Command::Desk(make(reply))).is_err() {
                return;
            }
            let Ok(found) = answer.await else { return };
            let _ = panel.update(cx, |panel, cx| {
                accept(&mut panel.model, generation, found);
                cx.notify();
            });
        })
    }

    /// The monitors, which are the one family that needs a second call.
    ///
    /// Each monitor's readings are their own DDC exchange, so the list arrives
    /// first and the values fill in behind it — and this is also the scan that
    /// ends the spinner, because it is the one that is still working after the
    /// others have answered.
    fn scan_displays(
        sender: mpsc::UnboundedSender<Command>,
        generation: u64,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender
                .send(Command::Desk(DeskCommand::ListDisplays(reply)))
                .is_err()
            {
                return;
            }
            let Ok(found) = answer.await else { return };
            let ids: Vec<String> = found
                .iter()
                .filter(|display| display.reachable)
                .map(|display| display.id.clone())
                .collect();
            let accepted = panel
                .update(cx, |panel, cx| {
                    let taken = panel.model.accept_displays(generation, found);
                    cx.notify();
                    taken
                })
                .unwrap_or(false);
            if !accepted {
                return;
            }
            // Only monitors that answered the probe are asked for readings;
            // the rest would spend a DDC timeout each to say so twice.
            for id in ids {
                let (reply, answer) = tokio::sync::oneshot::channel();
                if sender
                    .send(Command::Desk(DeskCommand::ReadDisplay(id, reply)))
                    .is_err()
                {
                    return;
                }
                if let Ok(Ok(settings)) = answer.await {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.model.accept_settings(generation, settings);
                        cx.notify();
                    });
                }
            }
            let _ = panel.update(cx, |panel, cx| {
                panel.model.finish_scan(generation);
                cx.notify();
            });
        })
    }

    /// The card every family draws itself inside: a group named for the
    /// device, so everything inside it is announced as belonging to it.
    fn card(
        key: &str,
        name: impl Into<gpui::SharedString>,
        pal: Palette,
    ) -> gpui::Stateful<gpui::Div> {
        a11y::group(gpui::SharedString::from(format!("desk-card-{key}")), name)
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(pal.panel)
    }

    /// A device's name as a heading, with whatever it last refused underneath.
    fn card_heading(&self, id: &str, name: &str, _pal: Palette) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(a11y::heading("name", name.to_owned(), 3).text_body())
            .when_some(self.failures.get(id).cloned(), |heading, why| {
                heading.child(
                    div()
                        .text_caption()
                        .text_color(warning_colour())
                        .child(a11y::label("refused", why)),
                )
            })
    }

    /// A line of supporting text inside a card, read by a screen reader.
    fn caption(
        slug: &'static str,
        text: impl Into<gpui::SharedString>,
        pal: Palette,
    ) -> impl IntoElement {
        div()
            .text_caption()
            .text_color(pal.text_muted)
            .child(a11y::label(slug, text))
    }

    /// One caption row: what the control is, and what it currently reads,
    /// announced as one phrase.
    fn reading_row(
        slug: impl Into<gpui::ElementId>,
        label: String,
        value: String,
        pal: Palette,
    ) -> impl IntoElement {
        a11y::reading(slug, label, value, pal)
    }
}

impl DeskPanel {
    /// Every family that found something, each under its own heading.
    fn sections(
        &mut self,
        pal: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let displays: Vec<DisplaySummary> = self.model.displays().to_vec();
        let lights: Vec<NetworkLightSummary> = self.model.lights().to_vec();
        let decks: Vec<StreamDeckSummary> = self.model.decks().to_vec();
        let interfaces: Vec<AudioInterfaceSummary> = self.model.interfaces().to_vec();
        let controllers: Vec<ControllerSummary> = self.model.controllers().to_vec();
        let pads: Vec<MacroPadSummary> = self.model.pads().to_vec();

        // Built as (heading, cards) so a family with nothing found contributes
        // nothing at all — no heading standing over an empty space.
        let sections: Vec<(gpui::SharedString, Vec<gpui::AnyElement>)> = vec![
            (
                tr!("Monitors"),
                displays
                    .iter()
                    .map(|display| {
                        self.display_card(display, pal, window, cx)
                            .into_any_element()
                    })
                    .collect(),
            ),
            (
                tr!("Lights"),
                lights
                    .iter()
                    .enumerate()
                    .map(|(index, light)| {
                        // Indexed rather than keyed by address only because
                        // `ElementId` takes an integer here; the address still
                        // decides which light a write reaches.
                        self.light_card(light, index as u64, pal, window, cx)
                            .into_any_element()
                    })
                    .collect(),
            ),
            (
                tr!("Stream Decks"),
                decks
                    .iter()
                    .enumerate()
                    .map(|(index, deck)| {
                        self.deck_card(deck, index as u64, pal, cx)
                            .into_any_element()
                    })
                    .collect(),
            ),
            (
                tr!("Audio interfaces"),
                interfaces
                    .iter()
                    .enumerate()
                    .map(|(index, interface)| {
                        self.audio_card(interface, index as u64, pal, cx)
                            .into_any_element()
                    })
                    .collect(),
            ),
            (
                tr!("Controllers"),
                controllers
                    .iter()
                    .enumerate()
                    .map(|(index, controller)| {
                        Self::controller_card(controller, index, pal).into_any_element()
                    })
                    .collect(),
            ),
            (
                tr!("Keyboards and macro pads"),
                pads.iter()
                    .enumerate()
                    .map(|(index, pad)| Self::pad_card(pad, index, pal).into_any_element())
                    .collect(),
            ),
        ];
        sections
            .into_iter()
            .enumerate()
            .filter(|(_, (_, cards))| !cards.is_empty())
            .flat_map(|(index, (heading, cards))| {
                std::iter::once(
                    a11y::heading(("desk-section", index as u64), heading, 2)
                        .text_body()
                        .into_any_element(),
                )
                .chain(cards)
            })
            .collect()
    }
}

impl Render for DeskPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        let scanning = self.model.is_scanning();
        let empty = self.model.is_empty();
        let sections = self.sections(pal, window, cx);

        // The heading and its Refresh stay put; everything below scrolls. Six
        // families of hardware do not fit any window worth opening, and before
        // this the cards below the fold simply could not be reached — the panel
        // drew a TourBox nobody could scroll to.
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .items_center()
                    .p_4()
                    .child(
                        gpui::InteractiveElement::track_focus(
                            a11y::heading("desk-title", tr!("The desk"), 1),
                            &self.title_focus,
                        )
                        .text_body(),
                    )
                    .child(
                        control_button("desk-refresh")
                            .label(tr!("Refresh"))
                            .accessibility_label(crate::ui::a11y::maybe_unavailable(
                                tr!("Refresh"),
                                scanning,
                            ))
                            .disabled(scanning)
                            .on_click(cx.listener(|panel, _event, _window, cx| {
                                panel.refresh(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap_4()
                    .px_4()
                    .pb_4()
                    .overflow_y_scrollbar()
                    .when(scanning, |column| {
                        column.child(Self::caption(
                            "desk-scanning",
                            tr!("Looking for everything on the desk…"),
                            pal,
                        ))
                    })
                    .when(empty, |column| {
                        column.child(Self::caption(
                            "desk-empty",
                            tr!("Nothing on the desk was found."),
                            pal,
                        ))
                    })
                    .children(sections),
            )
    }
}

impl AuxWindow for DeskPanel {
    fn set_appearance_obs(&mut self, sub: Subscription) {
        self.appearance = Some(sub);
    }
}

/// The colour for something that was refused, or that is about to cost
/// something.
///
/// The palette has no `danger` of its own — its fields are surfaces and text
/// weights — so this borrows the red the status dots already use rather than
/// introducing a second red that could drift from it. Colour is never the only
/// carrier here: every warning it paints is a sentence that says the same
/// thing, which is what a screen reader gets.
fn warning_colour() -> gpui::Rgba {
    gpui::rgb(theme::status_disabled())
}

/// The window's native title — one definition for opening and for the
/// live-language retitle, so the two cannot drift.
pub(crate) fn window_title() -> gpui::SharedString {
    tr!("The desk")
}

/// Open the panel, or focus it if it is already up.
pub fn open(cx: &mut App) {
    crate::windows::open_or_focus(
        |registry| &mut registry.desk,
        window_title(),
        gpui::Size::new(gpui::px(460.), gpui::px(620.)),
        DeskPanel::new,
        cx,
    );
}
