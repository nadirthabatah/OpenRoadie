//! Drawing a monitor.

use gpui::{
    Context, Entity, IntoElement, ParentElement, Styled, Text, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{h_flex, v_flex};
use roadie_ipc::desk::{DisplayControl, DisplayReading, DisplaySummary};

use super::{DeskPanel, Slot, WriteKey};
use crate::features::desk::model::describe_value;
use crate::services::ipc::{Command, DeskCommand};
use crate::state::AppState;
use crate::ui::a11y::{self, LevelChosen, LevelModel, LevelSlider, Span};
use crate::ui::theme::{Palette, Typography as _};

impl DeskPanel {
    /// Write one monitor control and take back what the monitor then reads.
    fn set_display(
        &mut self,
        id: String,
        control: DisplayControl,
        value: u16,
        cx: &mut Context<Self>,
    ) {
        let Some(sender) = AppState::try_read(cx).map(AppState::ipc_sender) else {
            return;
        };
        let key = WriteKey::Display(id.clone(), control);
        let task = cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender
                .send(Command::Desk(DeskCommand::SetDisplay(
                    id.clone(),
                    control,
                    value,
                    reply,
                )))
                .is_err()
            {
                return;
            }
            if let Ok(Ok(reading)) = answer.await {
                let _ = panel.update(cx, |panel, cx| {
                    panel.model.apply_reading(&id, reading);
                    cx.notify();
                });
            }
        });
        // Inserting replaces, and dropping the old task cancels it.
        self.writes.insert(key, task);
    }

    /// The slider for one monitor control, made on first sight of it and moved
    /// to match the device whenever the device disagrees with it.
    fn display_slider(
        &mut self,
        id: &str,
        reading: DisplayReading,
        cx: &mut Context<Self>,
    ) -> Entity<LevelModel> {
        let key = (id.to_owned(), reading.control);
        if let Some(existing) = self.display_sliders.get_mut(&key) {
            if existing.shown != reading.current {
                // The monitor took something other than what it was handed —
                // clamped to its own maximum, or rounded. The thumb follows the
                // monitor, because the monitor is what is true.
                existing.shown = reading.current;
                let state = existing.state.clone();
                state.update(cx, |slider, cx| slider.sync(reading.current, cx));
                return state;
            }
            return existing.state.clone();
        }
        let slider = a11y::level_model(reading.current, Span::to(reading.maximum.max(1)), cx);
        let handle = id.to_owned();
        let control = reading.control;
        // A choice, not a movement: a key press, an assistive-technology step,
        // or the release of a drag. A monitor answers a DDC write in tens of
        // milliseconds, and writing on every pixel of a drag would queue
        // hundreds of exchanges behind a person's thumb.
        let subscription =
            cx.subscribe(&slider, move |panel, _slider, chosen: &LevelChosen, cx| {
                panel.set_display(handle.clone(), control, chosen.0, cx);
            });
        self.subscriptions.push(subscription);
        self.display_sliders.insert(
            key,
            Slot {
                state: slider.clone(),
                shown: reading.current,
            },
        );
        slider
    }

    /// One monitor: its name, then its controls or the reason it has none.
    pub(super) fn display_card(
        &mut self,
        display: &DisplaySummary,
        pal: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let readings: Vec<DisplayReading> = self.model.readings(&display.id).to_vec();
        let reason = display.unreachable_reason.clone();
        let rows: Vec<_> = readings
            .iter()
            .map(|reading| {
                let (slug, label) = match reading.control {
                    DisplayControl::Brightness => ("brightness", tr!("Brightness")),
                    DisplayControl::Contrast => ("contrast", tr!("Contrast")),
                    DisplayControl::Volume => ("volume", tr!("Volume")),
                    DisplayControl::Input => ("input", tr!("Input")),
                };
                let value = describe_value(*reading);
                // Input is read, never written — see this module's header. So
                // it is a reading; every other control is a slider that says
                // its own name and value, and drawing a second reading above it
                // would announce the same thing twice.
                if reading.control == DisplayControl::Input {
                    return Self::reading_row(slug, label.to_string(), value, pal)
                        .into_any_element();
                }
                let slider = self.display_slider(&display.id, *reading, cx);
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_caption()
                                    .child(Text::new_inaccessible(label.clone())),
                            )
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(Text::new_inaccessible(value.clone().into())),
                            ),
                    )
                    .child(LevelSlider::new(slug, &slider, label, value))
                    .into_any_element()
            })
            .collect();

        Self::card(
            &format!("display-{}", display.id),
            display.name.clone(),
            pal,
        )
        .child(a11y::heading("name", display.name.clone(), 3).text_body())
        .when_some(reason, |card, why| {
            card.child(Self::caption("unreachable", why, pal))
        })
        .children(rows)
    }
}
