//! Drawing an Elgato light.

use gpui::{
    App, Context, Entity, IntoElement, ParentElement, Styled, Text, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{Selectable as _, h_flex, v_flex};
use roadie_ipc::desk::{NetworkLightChange, NetworkLightSummary};

use super::{DeskPanel, Slot, WriteKey};
use crate::services::ipc::{Command, DeskCommand};
use crate::state::AppState;
use crate::ui::a11y::{self, LevelChosen, LevelModel, LevelSlider, Span};
use crate::ui::components::Toggle;
use crate::ui::theme::{Palette, Typography as _};

/// A light's two continuous controls, for keying its sliders apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::features::desk) enum LightKnob {
    Brightness,
    Kelvin,
}

/// What a Key Light accepts, which is the light's own range rather than ours.
const LIGHT_BRIGHTNESS: Span = Span {
    minimum: 3,
    maximum: 100,
    step: 1,
};
/// The light counts in mireds, so neighbouring Kelvin values map to the same
/// one. Stepping in fifties keeps every stop distinct.
const LIGHT_KELVIN: Span = Span {
    minimum: 2900,
    maximum: 7000,
    step: 50,
};

impl DeskPanel {
    /// Write one light and take back what it then reads.
    fn set_light(
        &mut self,
        id: String,
        change: NetworkLightChange,
        key: WriteKey,
        cx: &mut Context<Self>,
    ) {
        let Some(sender) = AppState::try_read(cx).map(AppState::ipc_sender) else {
            return;
        };
        let done = key.clone();
        let task = cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender
                .send(Command::Desk(DeskCommand::SetNetworkLight(
                    id, change, reply,
                )))
                .is_err()
            {
                Self::finish_write(&panel, &done, cx);
                return;
            }
            if let Ok(Ok(updated)) = answer.await {
                let _ = panel.update(cx, |panel, cx| {
                    panel.model.apply_light(updated);
                    cx.notify();
                });
            }
            Self::finish_write(&panel, &done, cx);
        });
        self.writes.insert(key, task);
    }

    /// The slider for one light control, kept in step with the light.
    fn light_slider(
        &mut self,
        light: &NetworkLightSummary,
        knob: LightKnob,
        cx: &mut Context<Self>,
    ) -> Entity<LevelModel> {
        let key = (light.id.clone(), knob);
        let value = match knob {
            LightKnob::Brightness => light.brightness,
            LightKnob::Kelvin => light.kelvin,
        };
        if let Some(existing) = self.light_sliders.get_mut(&key) {
            if existing.shown != value {
                existing.shown = value;
                let state = existing.state.clone();
                state.update(cx, |slider, cx| slider.sync(value, cx));
                return state;
            }
            return existing.state.clone();
        }
        let span = match knob {
            LightKnob::Brightness => LIGHT_BRIGHTNESS,
            LightKnob::Kelvin => LIGHT_KELVIN,
        };
        let slider = a11y::level_model(value, span, cx);
        let id = light.id.clone();
        let subscription =
            cx.subscribe(&slider, move |panel, _slider, chosen: &LevelChosen, cx| {
                let change = match knob {
                    LightKnob::Brightness => NetworkLightChange {
                        brightness_percent: Some(chosen.0),
                        ..NetworkLightChange::default()
                    },
                    LightKnob::Kelvin => NetworkLightChange {
                        kelvin: Some(chosen.0),
                        ..NetworkLightChange::default()
                    },
                };
                panel.set_light(id.clone(), change, WriteKey::Light(id.clone(), knob), cx);
            });
        self.subscriptions.push(subscription);
        self.light_sliders.insert(
            key,
            Slot {
                state: slider.clone(),
                shown: value,
            },
        );
        slider
    }

    /// A slider with its name and reading drawn above it. The slider says both
    /// itself, so the drawn row is for the eye alone.
    pub(super) fn labelled_slider(
        slug: &'static str,
        name: gpui::SharedString,
        spoken: String,
        model: &Entity<LevelModel>,
        pal: Palette,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_caption()
                            .child(Text::new_inaccessible(name.clone())),
                    )
                    .child(
                        div()
                            .text_caption()
                            .text_color(pal.text_muted)
                            .child(Text::new_inaccessible(spoken.clone().into())),
                    ),
            )
            .child(LevelSlider::new(slug, model, name, spoken))
    }

    /// One light: on/off, brightness, colour temperature.
    pub(super) fn light_card(
        &mut self,
        light: &NetworkLightSummary,
        light_index: u64,
        pal: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = format!("light-{light_index}");
        // A light that would not say what it is doing gets its name and the
        // reason, and no controls at all: every number on it would be a zero
        // standing in for "not known", and a slider sitting at zero reads as a
        // light turned all the way down rather than one that is not answering.
        if !light.reachable {
            return Self::card(&key, light.name.clone(), pal)
                .child(a11y::heading("name", light.name.clone(), 3).text_body())
                .when_some(light.unreachable_reason.clone(), |card, why| {
                    card.child(Self::caption("unreachable", why, pal))
                })
                .into_any_element();
        }
        let brightness = self.light_slider(light, LightKnob::Brightness, cx);
        let kelvin = self.light_slider(light, LightKnob::Kelvin, cx);
        let id = light.id.clone();
        let on = light.on;
        Self::card(&key, light.name.clone(), pal)
            .child(
                h_flex()
                    .justify_between()
                    .child(a11y::heading("name", light.name.clone(), 3).text_body())
                    .child(
                        Toggle::new(
                            gpui::ElementId::NamedInteger("desk-light".into(), light_index),
                            tr!("Light"),
                        )
                        .label(if on { tr!("On") } else { tr!("Off") })
                        .selected(on)
                        .on_change({
                            // `Toggle::on_change` hands back an `App`, not
                            // this panel's context, so the panel is
                            // reached weakly: if it has gone, the write
                            // has nothing left to report to and is
                            // dropped rather than resurrecting it.
                            let panel = cx.entity().downgrade();
                            move |_checked: &bool, _window: &mut Window, cx: &mut App| {
                                let id = id.clone();
                                let _ = panel.update(cx, |panel, cx| {
                                    panel.set_light(
                                        id.clone(),
                                        NetworkLightChange {
                                            power: Some(!on),
                                            ..NetworkLightChange::default()
                                        },
                                        WriteKey::LightPower(id),
                                        cx,
                                    );
                                });
                            }
                        }),
                    ),
            )
            .child(Self::labelled_slider(
                "brightness",
                tr!("Brightness"),
                format!("{}%", light.brightness),
                &brightness,
                pal,
            ))
            .child(Self::labelled_slider(
                "kelvin",
                tr!("Colour temperature"),
                format!("{} K", light.kelvin),
                &kelvin,
                pal,
            ))
            .into_any_element()
    }
}
