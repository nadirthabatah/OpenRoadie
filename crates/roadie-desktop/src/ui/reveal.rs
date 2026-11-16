//! Keeping the focused control on screen.
//!
//! GPUI moves focus with Tab but never scrolls to it. On a page taller than
//! its window, focus walked off the bottom and stayed there: Settings,
//! Appearance put "125%" 900 px below a 640 px window (found under Orca on
//! Linux, 2026-11-16). A screen magnifier following focus then zoomed onto
//! nothing, and anyone reading the screen lost the focus ring.
//!
//! [`reveal`] wraps a control and, the frame focus arrives inside it, scrolls
//! whatever clips it by exactly the distance that brings it into view. It
//! scrolls the way a person would, with one wheel event, because that is the
//! one thing every scroll container here already answers — GPUI's own
//! `list`, plain `overflow_y_scroll` divs, gpui-component's scrollbars —
//! where the alternative, GPUI's `request_autoscroll`, makes a `list` lay its
//! rows out twice in one frame and trips its accessibility tree
//! ("Duplicate a11y node id"; a release build drops those nodes instead).

use std::cell::RefCell;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseMoveEvent, Pixels, PlatformInput, Point,
    ScrollDelta, ScrollWheelEvent, TouchPhase, Window,
};

/// Scroll `child` into view whenever focus moves onto it or into it.
///
/// Wrapping changes nothing about layout: the child's own layout box is
/// this element's. Nest freely — the innermost wrapper around the focused
/// control decides, so a card in a grid wins over the grid around it.
pub fn reveal(id: impl Into<ElementId>, child: impl IntoElement) -> Reveal {
    Reveal {
        id: id.into(),
        child: Some(child.into_any_element()),
    }
}

/// See [`reveal`].
pub struct Reveal {
    id: ElementId,
    child: Option<AnyElement>,
}

/// What a [`Reveal`] remembers between frames: the focus scope standing for
/// its subtree, and the focused element it last brought into view, so it
/// scrolls once per arrival and never fights the user's own scrolling.
struct RevealState {
    scope: FocusHandle,
    shown: Option<FocusHandle>,
}

/// The one scroll a frame asked for, made after the frame is drawn; `by` is
/// zero when the control focus reached was already in view.
struct Scroll {
    window: AnyWindowHandle,
    at: Point<Pixels>,
    by: Pixels,
}

thread_local! {
    /// Filled by the innermost wrapper focus arrived in this frame; wrappers
    /// further out see it taken and stand down.
    static PENDING: RefCell<Option<Scroll>> = const { RefCell::new(None) };
}

impl IntoElement for Reveal {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Reveal {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut child = self
            .child
            .take()
            .unwrap_or_else(|| gpui::Empty.into_any_element());
        (child.request_layout(window, cx), child)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        // The child first, so a wrapper nested inside has already claimed
        // this frame's scroll by the time this one decides.
        child.prepaint(window, cx);
        let Some(id) = id else {
            return;
        };
        window.with_element_state::<RevealState, _>(id, |state, window| {
            let mut state = state.unwrap_or_else(|| RevealState {
                scope: cx.focus_handle(),
                shown: None,
            });
            // The scope marks this element's place in the dispatch tree,
            // which is what `contains_focused` walks; it is never a tab stop,
            // and nothing here focuses it.
            window.set_focus_handle(&state.scope, cx);
            let inside = window
                .focused(cx)
                .filter(|_| state.scope.contains_focused(window, cx));
            if inside.is_some() && inside != state.shown {
                // The content mask is what clips this element: the viewport
                // of the scroll container around it. A wrapper with nothing
                // to scroll still claims the frame — the card already in
                // view must stop the grid around it scrolling to its top.
                let view = window.content_mask().bounds;
                let by = if view.size.height > Pixels::ZERO {
                    scroll_shift(view.top(), view.bottom(), bounds.top(), bounds.bottom())
                } else {
                    Pixels::ZERO
                };
                claim(
                    Scroll {
                        window: window.window_handle(),
                        at: view.center(),
                        by,
                    },
                    window,
                );
            }
            state.shown = inside;
            ((), state)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

/// Take this window's scroll for the frame unless a wrapper nearer the
/// focused control already has, and make it once the frame is drawn.
fn claim(scroll: Scroll, window: &mut Window) {
    let first = PENDING.with_borrow_mut(|pending| {
        if pending.as_ref().is_some_and(|p| p.window == scroll.window) {
            return false;
        }
        *pending = Some(scroll);
        true
    });
    if !first {
        return;
    }
    window.on_next_frame(|window, cx| {
        let Some(scroll) = PENDING.with_borrow_mut(Option::take) else {
            return;
        };
        if scroll.window != window.window_handle() || scroll.by == Pixels::ZERO {
            return;
        }
        // A wheel turn over the container's middle, then the pointer put
        // back where it was, so no hover is left behind on whatever sits
        // there.
        let pointer = window.mouse_position();
        let modifiers = window.modifiers();
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: scroll.at,
                delta: ScrollDelta::Pixels(gpui::point(Pixels::ZERO, scroll.by)),
                modifiers,
                touch_phase: TouchPhase::Moved,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: pointer,
                pressed_button: None,
                modifiers,
            }),
            cx,
        );
    });
    window.refresh();
}

/// How far to scroll so the span from `wanted_top` to `wanted_bottom` sits
/// inside the view: positive scrolls up, negative down, zero when it is
/// already shown — the sign a wheel event's delta uses. Something taller
/// than the view is aligned by its top, where reading starts.
fn scroll_shift(
    view_top: Pixels,
    view_bottom: Pixels,
    wanted_top: Pixels,
    wanted_bottom: Pixels,
) -> Pixels {
    if wanted_top < view_top || wanted_bottom - wanted_top > view_bottom - view_top {
        view_top - wanted_top
    } else if wanted_bottom > view_bottom {
        view_bottom - wanted_bottom
    } else {
        Pixels::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_shift;
    use gpui::px;

    #[test]
    fn something_already_shown_does_not_move() {
        assert_eq!(scroll_shift(px(0.), px(600.), px(100.), px(140.)), px(0.));
    }

    #[test]
    fn something_below_scrolls_down_until_its_bottom_shows() {
        assert_eq!(
            scroll_shift(px(0.), px(600.), px(900.), px(940.)),
            px(-340.)
        );
    }

    #[test]
    fn something_above_scrolls_up_until_its_top_shows() {
        assert_eq!(scroll_shift(px(100.), px(700.), px(40.), px(80.)), px(60.));
    }

    #[test]
    fn something_taller_than_the_view_shows_its_top() {
        assert_eq!(
            scroll_shift(px(0.), px(600.), px(500.), px(1300.)),
            px(-500.)
        );
    }
}
