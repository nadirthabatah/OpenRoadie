//! Drawing the two families that are identity and nothing else.
//!
//! Neither a TourBox nor a VIA board has settings this panel can change, and
//! both are drawn anyway. A device that is plugged in and working should
//! appear on a panel called "the desk" — leaving it off would mean somebody
//! checking whether the app can see their controller finds an empty space and
//! concludes it cannot.
//!
//! What each card does instead is say where its settings actually live, so the
//! absence of knobs reads as a signpost rather than as something missing.

use gpui::{IntoElement, ParentElement, prelude::FluentBuilder as _};
use roadie_ipc::desk::{ControllerSummary, MacroPadSummary};

use super::DeskPanel;
use crate::ui::a11y;
use crate::ui::theme::{Palette, Typography as _};

impl DeskPanel {
    /// One TourBox: what it is, and where its buttons are configured.
    pub(super) fn controller_card(
        controller: &ControllerSummary,
        index: usize,
        pal: Palette,
    ) -> impl IntoElement {
        Self::card(&format!("controller-{index}"), controller.name.clone(), pal)
            .child(a11y::heading("name", controller.name.clone(), 3).text_body())
            .child(Self::caption(
                "describe",
                tr!(
                    "%{buttons} buttons and %{wheels} wheels",
                    buttons = controller.buttons,
                    wheels = controller.wheels
                ),
                pal,
            ))
            .when(controller.haptics, |card| {
                card.child(Self::caption("haptics", tr!("It can buzz."), pal))
            })
            .child(Self::caption(
                "where",
                tr!(
                    "A controller stores no settings of its own. What each button and wheel \
                         does is set in your configuration file."
                ),
                pal,
            ))
    }

    /// One VIA board: what it is, and what its handshake said.
    pub(super) fn pad_card(pad: &MacroPadSummary, index: usize, pal: Palette) -> impl IntoElement {
        Self::card(&format!("pad-{index}"), pad.name.clone(), pal)
            .child(a11y::heading("name", pad.name.clone(), 3).text_body())
            .when(!pad.reachable, |card| {
                card.when_some(pad.unreachable_reason.clone(), |card, why| {
                    card.child(Self::caption("unreachable", why, pal))
                })
            })
            .when(pad.reachable, |card| {
                card.child(Self::caption(
                    "describe",
                    tr!(
                        "Speaks VIA protocol %{protocol}, with %{layers} keymap layers.",
                        protocol = pad.protocol,
                        layers = pad.layers
                    ),
                    pal,
                ))
                .child(Self::caption(
                    "where",
                    tr!(
                        "Reading and changing its keymap is done from the command line for \
                             now."
                    ),
                    pal,
                ))
            })
    }
}
