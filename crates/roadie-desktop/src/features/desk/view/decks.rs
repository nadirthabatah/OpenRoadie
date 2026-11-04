//! Drawing a Stream Deck.
//!
//! # Why brightness is three buttons and not a slider
//!
//! Every other continuous control in this panel is a slider, and a slider is
//! only honest when the device can be asked where it currently sits. A Stream
//! Deck cannot: the protocol has a brightness write and no matching read. A
//! slider would therefore open at some number nobody measured, and a thumb
//! resting at "100" beside a deck that is actually dimmed is precisely the
//! confidently-wrong display this panel refuses to draw for monitors.
//!
//! So brightness is offered as what it really is here — three actions, not a
//! position. Pressing one is a thing you did; none of them claims to be a
//! thing the deck is. The three levels are the device crate's own constants,
//! so the panel and the command line dim a deck by exactly the same amount.

use gpui::{
    App, Context, ElementId, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{Disableable as _, h_flex};
use roadie_ipc::desk::{StreamDeckChange, StreamDeckSummary};

use super::{DeskPanel, WriteKey};
use crate::services::ipc::{Command, DeskCommand};
use crate::state::AppState;
use crate::ui::a11y;
use crate::ui::components::control_button;
use crate::ui::theme::{Palette, Typography as _};

/// The three brightness levels the device crate names.
///
/// Mirrored as plain percentages because the wire speaks percentages — but
/// taken from the same three the protocol crate defines, so "dim" means one
/// thing across the app rather than one thing per front end.
const FULL: u8 = 100;
const DIM: u8 = 30;
const OFF: u8 = 0;

impl DeskPanel {
    /// Write one Stream Deck.
    ///
    /// There is nothing to read back, so what lands here is either silence or
    /// the reason it was refused. See [`StreamDeckChange`].
    fn set_deck(&mut self, id: String, change: StreamDeckChange, cx: &mut Context<Self>) {
        let Some(sender) = AppState::try_read(cx).map(AppState::ipc_sender) else {
            return;
        };
        let key = WriteKey::Deck(id.clone());
        let task = cx.spawn(async move |panel, cx| {
            let (reply, answer) = tokio::sync::oneshot::channel();
            if sender
                .send(Command::Desk(DeskCommand::SetStreamDeck(
                    id.clone(),
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
                        panel.model.accept_deck(updated);
                        cx.notify();
                    });
                }
                Ok(Err(why)) => {
                    let _ = panel.update(cx, |panel, cx| {
                        panel.failures.insert(id, why.to_string());
                        cx.notify();
                    });
                }
                Err(_) => {}
            }
        });
        self.writes.insert(key, task);
    }

    /// One deck: what it is, and the four things that can be done to it.
    pub(super) fn deck_card(
        &mut self,
        deck: &StreamDeckSummary,
        deck_index: u64,
        pal: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // A deck that would not open gets its name and the reason. Almost
        // always another program is holding it, which is a thing a person can
        // do something about — so the sentence matters more than the controls.
        let key = format!("deck-{deck_index}");
        if !deck.reachable {
            return Self::card(&key, deck.name.clone(), pal)
                .child(a11y::heading("name", deck.name.clone(), 3).text_body())
                .when_some(deck.unreachable_reason.clone(), |card, why| {
                    card.child(Self::caption("unreachable", why, pal))
                })
                .into_any_element();
        }

        let describe = if deck.dials > 0 {
            tr!(
                "%{keys} keys and %{dials} dials",
                keys = deck.keys,
                dials = deck.dials
            )
        } else {
            tr!("%{keys} keys", keys = deck.keys)
        };

        Self::card(&key, deck.name.clone(), pal)
            .child(self.card_heading(&deck.id, &deck.name, pal))
            .child(Self::caption("describe", describe, pal))
            .child(
                // Says the deck too: Narrator names only the innermost group it enters,
                // so two decks' "Screen brightness" could not be told apart.
                a11y::group(
                    "brightness",
                    format!("{}, {}", deck.name, tr!("Screen brightness")),
                )
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_caption()
                        .child(gpui::Text::new_inaccessible(tr!("Screen brightness"))),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Self::brightness_button(
                            deck,
                            deck_index,
                            "full",
                            tr!("Full brightness"),
                            FULL,
                            cx,
                        ))
                        .child(Self::brightness_button(
                            deck,
                            deck_index,
                            "dim",
                            tr!("Dim"),
                            DIM,
                            cx,
                        ))
                        .child(Self::brightness_button(
                            deck,
                            deck_index,
                            "off",
                            tr!("Off"),
                            OFF,
                            cx,
                        )),
                ),
            )
            .child(
                control_button(ElementId::NamedInteger(
                    "desk-deck-reset".into(),
                    deck_index,
                ))
                .label(tr!("Clear the keys"))
                .disabled(self.writes.contains_key(&WriteKey::Deck(deck.id.clone())))
                .on_click({
                    let panel = cx.entity().downgrade();
                    let id = deck.id.clone();
                    move |_event, _window: &mut Window, cx: &mut App| {
                        let id = id.clone();
                        let _ = panel.update(cx, |panel, cx| {
                            panel.set_deck(
                                id,
                                StreamDeckChange {
                                    reset: true,
                                    ..StreamDeckChange::default()
                                },
                                cx,
                            );
                            cx.notify();
                        });
                    }
                }),
            )
            .into_any_element()
    }

    /// One of the three brightness actions.
    fn brightness_button(
        deck: &StreamDeckSummary,
        deck_index: u64,
        slug: &'static str,
        label: gpui::SharedString,
        percent: u8,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = deck.id.clone();
        control_button(ElementId::NamedInteger(
            format!("desk-deck-{slug}").into(),
            deck_index,
        ))
        .label(label)
        .on_click({
            let panel = cx.entity().downgrade();
            move |_event, _window: &mut Window, cx: &mut App| {
                let id = id.clone();
                let _ = panel.update(cx, |panel, cx| {
                    panel.set_deck(
                        id,
                        StreamDeckChange {
                            brightness_percent: Some(percent),
                            reset: false,
                        },
                        cx,
                    );
                    cx.notify();
                });
            }
        })
    }
}
