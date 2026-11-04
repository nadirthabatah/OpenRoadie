//! Drawing a Focusrite audio interface.
//!
//! # Why gain is two buttons and not a slider
//!
//! A slider needs a maximum, and these interfaces do not report one. A monitor
//! answers every read with its own ceiling beside the value, which is what
//! makes the monitor sliders here honest; a Focusrite answers with a byte and
//! nothing to scale it against — this desk's Vocaster accepts and reads back
//! every value to 255 without complaint. A slider drawn against a maximum
//! nobody stated would be wrong at both ends, and wrong in a way that looks
//! authoritative.
//!
//! Stepping also happens to be the better control for the way this app is
//! used: two named buttons and a number that is read out are unambiguous
//! aloud, where a dragged thumb is not.
//!
//! # The phantom-power confirmation
//!
//! Switching 48 volts on can destroy a ribbon microphone, and nothing in the
//! protocol can see what is plugged in. So the toggle does not do it: the
//! first press asks the agent, the agent refuses and sends back the sentence
//! explaining the cost, and only a second, explicit press goes through. The
//! words come from the agent rather than from here, so what somebody reads is
//! the same text the command line reads out.
//!
//! Switching it **off** asks nothing. That is how somebody makes the interface
//! safe again, and a confirmation in front of the safe direction is how a
//! confirmation stops being read.

use gpui::{
    App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{Disableable as _, Selectable as _, h_flex, v_flex};
use roadie_ipc::desk::{AudioFailure, AudioInputChange, AudioInputSettings, AudioInterfaceSummary};

use super::{DeskPanel, WriteKey, warning_colour};
use crate::features::desk::model::describe_gain;
use crate::services::ipc::{Command, DeskCommand};
use crate::state::AppState;
use crate::ui::a11y;
use crate::ui::components::{Toggle, control_button};
use crate::ui::theme::{Palette, Typography as _};

/// How far one press moves the gain.
///
/// Five rather than one because these run over a range of tens, and one would
/// make crossing it a chore; and five rather than ten because a preamp is set
/// by ear and ten steps past the place somebody was listening for.
const GAIN_STEP: u8 = 5;

impl DeskPanel {
    /// Write one input and take back the whole interface as it then reads.
    fn set_audio_input(
        &mut self,
        id: String,
        input: u16,
        change: AudioInputChange,
        cx: &mut Context<Self>,
    ) {
        let Some(sender) = AppState::try_read(cx).map(AppState::ipc_sender) else {
            return;
        };
        let key = WriteKey::AudioInput(id.clone(), input);
        let task = cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender
                .send(Command::Desk(DeskCommand::SetAudioInput(
                    id.clone(),
                    input,
                    change,
                    reply,
                )))
                .is_err()
            {
                return;
            }
            match answer.await {
                Ok(Ok(updated)) => {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.failures.remove(&id);
                        panel.pending_phantom.remove(&(id.clone(), input));
                        panel.model.apply_interface(updated);
                        cx.notify();
                    });
                }
                Ok(Err(why)) => {
                    let _ = panel.update(cx, |panel, cx| {
                        // A demand to acknowledge is not a failure: it is the
                        // warning, and it belongs beside the switch that
                        // raised it rather than in the card's error line.
                        if let AudioFailure::NeedsAcknowledgement(said) = &why {
                            panel.pending_phantom.insert((id, input), said.clone());
                        } else {
                            panel.failures.insert(id, why.to_string());
                        }
                        cx.notify();
                    });
                }
                Err(_) => {}
            }
        });
        self.writes.insert(key, task);
    }

    /// Ask for one change to one input, from a callback that has only an
    /// [`App`] — which is every control on this card.
    fn request(
        panel: &gpui::WeakEntity<Self>,
        id: String,
        input: u16,
        change: AudioInputChange,
        cx: &mut App,
    ) {
        let _ = panel.update(cx, |panel, cx| {
            panel.set_audio_input(id, input, change, cx);
            cx.notify();
        });
    }

    /// One interface: what it is, then one block per input.
    pub(super) fn audio_card(
        &mut self,
        interface: &AudioInterfaceSummary,
        interface_index: u64,
        pal: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let card_key = format!("audio-{interface_index}");
        if !interface.reachable {
            return Self::card(&card_key, interface.name.clone(), pal)
                .child(a11y::heading("name", interface.name.clone(), 3).text_body())
                .when_some(interface.unreachable_reason.clone(), |card, why| {
                    card.child(Self::caption("unreachable", why, pal))
                })
                .into_any_element();
        }

        let inputs: Vec<AudioInputSettings> = interface.inputs.clone();
        let rows: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(row, settings)| {
                // Keyed by interface and row together: two interfaces of the
                // same model have the same input numbers, and sharing an
                // element id between them would share their focus too.
                let key = interface_index * 16 + row as u64;
                self.input_block(&interface.id, *settings, key, pal, cx)
                    .into_any_element()
            })
            .collect();

        Self::card(&card_key, interface.name.clone(), pal)
            .child(self.card_heading(&interface.id, &interface.name, pal))
            .when(interface.mass_storage == Some(true), |card| {
                // Not a fault and not a thing to fix — everything works with it
                // on. Said because people expect it to matter and then go
                // looking for a switch that would not help.
                card.child(Self::caption(
                    "mass-storage",
                    tr!(
                        "This interface is still showing its registration disk, which is how it \
                     leaves the factory. Everything here works anyway."
                    ),
                    pal,
                ))
            })
            .children(rows)
            .into_any_element()
    }

    /// One input: its gain, its mute, and its phantom power.
    ///
    /// A control the model does not have on this input is left out rather than
    /// drawn dead: `None` means there is no such knob here, which is a fact
    /// about the box and not a failure.
    fn input_block(
        &mut self,
        id: &str,
        settings: AudioInputSettings,
        key: u64,
        pal: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let input = settings.input;
        let busy = self
            .writes
            .contains_key(&WriteKey::AudioInput(id.to_owned(), input));
        let warning = self.pending_phantom.get(&(id.to_owned(), input)).cloned();

        let title = tr!("Input %{number}", number = input);
        // A group per input, so every control inside is announced as
        // belonging to it: "Input 2, Gain, Turn up" rather than a third
        // "Turn up" with nothing saying which input it raises.
        a11y::group(
            ElementId::NamedInteger("desk-audio-input".into(), key),
            title.clone(),
        )
        .flex()
        .flex_col()
        .gap_1()
        .child(a11y::heading("title", title, 4).text_caption())
        .when_some(settings.gain, |block, gain| {
            block.child(Self::gain_row(id, input, key, gain, busy, pal, cx))
        })
        .when_some(settings.muted, |block, muted| {
            block.child(Self::mute_row(id, input, key, muted, cx))
        })
        .when_some(settings.phantom, |block, phantom| {
            block.child(Self::phantom_row(id, input, key, phantom, cx))
        })
        .when_some(warning, |block, said| {
            block.child(Self::phantom_warning(id, input, key, said, cx))
        })
    }

    /// The gain reading and its two steps.
    fn gain_row(
        id: &str,
        input: u16,
        key: u64,
        gain: u8,
        busy: bool,
        pal: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(Self::reading_row(
                "gain",
                tr!("Gain").to_string(),
                describe_gain(Some(gain)),
                pal,
            ))
            .child(
                // The reading above is text, which Tab passes by; the steps' group
                // carries the value so it is heard on the way to the buttons.
                a11y::group(
                    "gain-steps",
                    format!(
                        "{}, {}, {}",
                        tr!("Input %{number}", number = input),
                        tr!("Gain"),
                        describe_gain(Some(gain))
                    ),
                )
                .flex()
                .gap_2()
                .child(Self::gain_button(
                    id,
                    input,
                    key,
                    "down",
                    tr!("Turn down"),
                    gain.saturating_sub(GAIN_STEP),
                    busy,
                    cx,
                ))
                .child(Self::gain_button(
                    id,
                    input,
                    key,
                    "up",
                    tr!("Turn up"),
                    gain.saturating_add(GAIN_STEP),
                    busy,
                    cx,
                )),
            )
    }

    /// One of the two gain steps.
    #[expect(
        clippy::too_many_arguments,
        reason = "each one is a distinct fact about which button this is; a params struct for two call sites in one file would be ceremony"
    )]
    fn gain_button(
        id: &str,
        input: u16,
        key: u64,
        slug: &'static str,
        label: gpui::SharedString,
        target: u8,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = id.to_owned();
        let panel = cx.entity().downgrade();
        control_button(ElementId::NamedInteger(
            format!("desk-audio-gain-{slug}").into(),
            key,
        ))
        .label(label)
        .disabled(busy)
        .on_click(move |_event, _window: &mut Window, cx: &mut App| {
            Self::request(
                &panel,
                id.clone(),
                input,
                AudioInputChange {
                    gain: Some(target),
                    ..AudioInputChange::default()
                },
                cx,
            );
        })
    }

    /// The mute switch.
    fn mute_row(
        id: &str,
        input: u16,
        key: u64,
        muted: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = id.to_owned();
        let panel = cx.entity().downgrade();
        h_flex()
            .justify_between()
            .child(div().text_caption().child(tr!("Mute")))
            .child(
                Toggle::new(
                    ElementId::NamedInteger("desk-audio-mute".into(), key),
                    tr!("Mute"),
                )
                .label(if muted {
                    tr!("Muted")
                } else {
                    tr!("Not muted")
                })
                .selected(muted)
                .on_change(
                    move |_checked: &bool, _window: &mut Window, cx: &mut App| {
                        Self::request(
                            &panel,
                            id.clone(),
                            input,
                            AudioInputChange {
                                muted: Some(!muted),
                                ..AudioInputChange::default()
                            },
                            cx,
                        );
                    },
                ),
            )
    }

    /// The phantom-power switch.
    ///
    /// Deliberately sends the change *unacknowledged*. Off goes straight
    /// through; on comes back refused, carrying the sentence that
    /// [`Self::phantom_warning`] then shows.
    fn phantom_row(
        id: &str,
        input: u16,
        key: u64,
        phantom: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = id.to_owned();
        let panel = cx.entity().downgrade();
        h_flex()
            .justify_between()
            .child(div()
                    // Drawn, not read: the switch beside it carries the same words.
                    .text_caption()
                    .child(tr!("48 volt phantom power")))
            .child(
                Toggle::new(
                    ElementId::NamedInteger("desk-audio-phantom".into(), key),
                    tr!("48 volt phantom power"),
                )
                .label(if phantom { tr!("On") } else { tr!("Off") })
                .selected(phantom)
                .on_change(
                    move |_checked: &bool, _window: &mut Window, cx: &mut App| {
                        Self::request(
                            &panel,
                            id.clone(),
                            input,
                            AudioInputChange {
                                phantom: Some(!phantom),
                                ..AudioInputChange::default()
                            },
                            cx,
                        );
                    },
                ),
            )
    }

    /// What switching phantom power on would cost, and the two ways out.
    fn phantom_warning(
        id: &str,
        input: u16,
        key: u64,
        said: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accept_id = id.to_owned();
        let dismiss_id = id.to_owned();
        let accept_panel = cx.entity().downgrade();
        let dismiss_panel = cx.entity().downgrade();
        v_flex()
            .gap_1()
            // An alert, so a screen reader speaks it the moment it appears:
            // this is the sentence somebody has to hear before 48 volts go
            // onto a cable, and they cannot be expected to go looking for it.
            .child(
                div()
                    .id("phantom-warning")
                    .role(gpui::accesskit::Role::Alert)
                    .aria_label(said.clone())
                    .text_caption()
                    .text_color(warning_colour())
                    .child(gpui::Text::new_inaccessible(said.into())),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        control_button(ElementId::NamedInteger(
                            "desk-audio-phantom-yes".into(),
                            key,
                        ))
                        .label(tr!("Switch it on"))
                        .on_click(
                            move |_event, _window: &mut Window, cx: &mut App| {
                                Self::request(
                                    &accept_panel,
                                    accept_id.clone(),
                                    input,
                                    AudioInputChange {
                                        phantom: Some(true),
                                        phantom_acknowledged: true,
                                        ..AudioInputChange::default()
                                    },
                                    cx,
                                );
                            },
                        ),
                    )
                    .child(
                        control_button(ElementId::NamedInteger(
                            "desk-audio-phantom-no".into(),
                            key,
                        ))
                        .label(tr!("Leave it off"))
                        .on_click(
                            move |_event, _window: &mut Window, cx: &mut App| {
                                let id = dismiss_id.clone();
                                let _ = dismiss_panel.update(cx, |panel, cx| {
                                    panel.pending_phantom.remove(&(id, input));
                                    cx.notify();
                                });
                            },
                        ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, Window};
    use roadie_core::config::Config;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;
    use crate::features::desk::view::DeskPanel;
    use crate::services::assets::AssetResolver;
    use crate::state::{AppState, ConfigPersistence};

    /// An interface shaped like the one this was written against.
    fn vocaster() -> AudioInterfaceSummary {
        AudioInterfaceSummary {
            id: "V2VD42B2703F98".to_owned(),
            name: "Vocaster Two".to_owned(),
            firmware: 1749,
            mass_storage: Some(true),
            inputs: vec![AudioInputSettings {
                input: 1,
                gain: Some(70),
                muted: Some(false),
                phantom: Some(false),
            }],
            reachable: true,
            unreachable_reason: None,
        }
    }

    fn press(cx: &mut gpui::VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).expect("a parseable keystroke");
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
    }

    /// The last write asked for, if any.
    fn last_write(
        receiver: &mut UnboundedReceiver<Command>,
    ) -> Option<(String, u16, AudioInputChange)> {
        let mut found = None;
        while let Ok(command) = receiver.try_recv() {
            if let Command::Desk(DeskCommand::SetAudioInput(id, input, change, _)) = command {
                found = Some((id, input, change));
            }
        }
        found
    }

    /// Drive the real panel: focus the gain step and activate it.
    ///
    /// This is the one path no other test covers — a press on a real button,
    /// through the callback, into the command the agent would receive. Every
    /// other test here works one layer in from the button, and a button wired
    /// to nothing would pass all of them.
    #[gpui::test]
    fn turning_the_gain_down_asks_for_one_step_below_what_the_interface_reads(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        cx.update(|cx| {
            let cache = AssetResolver::new();
            let state = cx.new(|_| {
                AppState::with_runtime(
                    Config::ephemeral(),
                    &[],
                    &[],
                    &cache,
                    &[],
                    ConfigPersistence::MemoryOnly,
                    commands,
                )
            });
            AppState::set_global(state, cx);
        });

        let (panel, cx) = cx.add_window_view(DeskPanel::new);
        panel.update(cx, |panel, _| {
            let scan = panel.model.begin_scan();
            panel.model.accept_interfaces(scan, vec![vocaster()]);
            panel.model.finish_scan(scan);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        // A fresh panel scans for every family on the way up; those asks are
        // not what this is about.
        let _ = last_write(&mut receiver);

        // Refresh is the first tab stop, the gain step is the next.
        cx.update(Window::focus_next);
        cx.update(Window::focus_next);
        press(cx, "enter");

        let (id, input, change) =
            last_write(&mut receiver).expect("a press on the button asks the agent for a write");
        assert_eq!(id, "V2VD42B2703F98");
        assert_eq!(input, 1);
        assert_eq!(
            change.gain,
            Some(65),
            "a step below the 70 the interface reads, not below whatever was last sent"
        );
        assert!(
            !change.phantom_acknowledged,
            "a gain step must never carry an acknowledgement"
        );
    }

    /// The half of the phantom gate that lives in the panel.
    ///
    /// The agent refuses an unacknowledged switch-on and that refusal has its
    /// own tests. What only this can prove is that the panel *asks* — that the
    /// first press carries no acknowledgement, so the refusal is reached and
    /// the warning gets read. A toggle that helpfully acknowledged itself
    /// would switch 48 volts on with nobody having seen a word about it, and
    /// every test on the agent side would still pass.
    #[gpui::test]
    fn the_first_press_on_phantom_power_carries_no_acknowledgement(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        cx.update(|cx| {
            let cache = AssetResolver::new();
            let state = cx.new(|_| {
                AppState::with_runtime(
                    Config::ephemeral(),
                    &[],
                    &[],
                    &cache,
                    &[],
                    ConfigPersistence::MemoryOnly,
                    commands,
                )
            });
            AppState::set_global(state, cx);
        });

        let (panel, cx) = cx.add_window_view(DeskPanel::new);
        panel.update(cx, |panel, _| {
            let scan = panel.model.begin_scan();
            panel.model.accept_interfaces(scan, vec![vocaster()]);
            panel.model.finish_scan(scan);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let _ = last_write(&mut receiver);

        // Refresh, the two gain steps, mute, then phantom power.
        for _ in 0..5 {
            cx.update(Window::focus_next);
        }
        press(cx, "enter");

        let (_, input, change) =
            last_write(&mut receiver).expect("a press on the switch asks the agent for a write");
        assert_eq!(input, 1);
        assert_eq!(
            change.phantom,
            Some(true),
            "the switch was off, so a press asks for it on"
        );
        assert!(
            !change.phantom_acknowledged,
            "the panel must not acknowledge on somebody's behalf — the refusal is what shows \
             them what it costs"
        );
    }
}
