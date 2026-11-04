//! What a screen reader, a magnifier and a keyboard need from every screen.
//!
//! GPUI reports an element to assistive technology only when it has both an
//! id and a role, and plain strings have neither — so text drawn with
//! `.child(tr!(…))` is pixels, not words. Found running the app under UI
//! Automation on Windows and AT-SPI on Linux, 2026-11-04: the desk panel
//! exposed twenty controls and not one word of text, sliders had no names and
//! could not be reached with Tab, and four buttons were all called "Off".
//!
//! These helpers are the vocabulary for fixing that once rather than per
//! screen: a [`heading`] that a screen reader can jump between, a [`label`]
//! that is actually read, a [`group`] that names a set of controls so two
//! identical buttons on two devices can be told apart, a [`reading`] that says
//! a setting and its value as one phrase, and [`LevelSlider`], a slider that
//! has a name, takes focus, answers the arrow keys, and announces its value.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Context, ElementId, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled, Text, Window, accesskit::Role, div, px,
    relative,
};
use gpui_component::h_flex;

use crate::ui::theme::{self, Palette, accent};

/// A heading a screen reader can list and jump between.
///
/// The visible text is drawn inaccessibly and the heading carries it as its
/// name, so it is announced once — "Monitors, heading level 2" — rather than
/// as a heading followed by the same word again.
pub fn heading(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    level: usize,
) -> gpui::Stateful<gpui::Div> {
    let text = text.into();
    div()
        .id(id)
        .role(Role::Heading)
        .aria_level(level)
        .aria_label(text.clone())
        .child(Text::new_inaccessible(text))
}

/// Text that assistive technology reads.
pub fn label(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Text {
    Text::new(id.into(), text.into())
}

/// A name for an icon-only button, said by a screen reader and not drawn.
///
/// The component library's button takes its accessible name only from a
/// visible label, and an ellipsis button has none: it read as "button" and
/// nothing more. Assistive technology names a button from the text inside it
/// when it has no label, so this is that text, laid out at no size at all.
/// The id is the caller's, so two such buttons under one parent stay distinct.
pub fn spoken_name(id: impl Into<ElementId>, name: impl Into<SharedString>) -> impl IntoElement {
    div()
        .absolute()
        .size_0()
        .overflow_hidden()
        .child(Text::new(id.into(), name.into()))
}

/// "Interface language, English": a setting's name with its current choice,
/// for a control that cannot say its own value. Just the name when nothing is
/// chosen.
pub fn choice(name: SharedString, chosen: Option<SharedString>) -> SharedString {
    match chosen {
        Some(chosen) => format!("{name}, {chosen}").into(),
        None => name,
    }
}

/// A named set of controls.
///
/// The fix for buttons that look the same on two devices: a screen reader
/// announces the group's name as focus enters it, so "Off" inside "Stream
/// Deck XL, screen brightness" is a different button from "Off" inside
/// "Vocaster Two, input 2, 48 volt phantom power", without either having to
/// print the device's name on its face.
pub fn group(id: impl Into<ElementId>, name: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    div().id(id).role(Role::Group).aria_label(name.into())
}

/// A setting and its current value, laid out side by side and read as one
/// phrase: "Brightness, 100%".
pub fn reading(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    value: impl Into<SharedString>,
    pal: Palette,
) -> gpui::Stateful<gpui::Div> {
    let name = name.into();
    let value = value.into();
    let phrase = SharedString::from(format!("{name}, {value}"));
    // A label node's text is its value on UI Automation, so the phrase is set
    // as both: as the name alone it arrived on Windows as an empty text.
    h_flex()
        .id(id)
        .role(Role::Label)
        .aria_label(phrase.clone())
        .aria_value(phrase)
        .justify_between()
        .child(div().text_sm().child(Text::new_inaccessible(name)))
        .child(
            div()
                .text_sm()
                .text_color(pal.text_muted)
                .child(Text::new_inaccessible(value)),
        )
}

/// The span a [`LevelSlider`] moves over, and how far one step goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The lowest value; a Key Light's brightness starts at 3, not 0.
    pub minimum: u16,
    /// The highest value.
    pub maximum: u16,
    /// One arrow-key step: 50 for a Key Light's colour temperature, whose
    /// neighbouring kelvin values land on the same mired.
    pub step: u16,
}

impl Span {
    /// From zero to `maximum` in ones — a monitor control.
    pub const fn to(maximum: u16) -> Self {
        Self {
            minimum: 0,
            maximum,
            step: 1,
        }
    }

    fn clamp(self, value: u16) -> u16 {
        value.clamp(self.minimum, self.maximum.max(self.minimum))
    }

    /// Ten notches across the span for Page Up and Page Down, rounded to a
    /// whole number of steps and never smaller than one.
    fn page(self) -> u16 {
        let steps = (self.maximum.saturating_sub(self.minimum) / self.step.max(1) / 10).max(1);
        steps.saturating_mul(self.step.max(1))
    }
}

/// The state behind one [`LevelSlider`]: where it is and where it can go.
pub struct LevelModel {
    value: u16,
    span: Span,
    /// Where the track was last laid out, for turning a pointer into a value.
    /// A cell rather than a field updated through the entity, because it is
    /// written during prepaint, where notifying would redraw forever.
    track: Rc<Cell<Option<Bounds<Pixels>>>>,
    dragging: bool,
}

/// A value someone settled on: a key press, an assistive-technology action,
/// or the release of a drag. Moving the thumb mid-drag does not emit one, so a
/// device is written once per decision rather than once per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelChosen(pub u16);

impl EventEmitter<LevelChosen> for LevelModel {}

impl LevelModel {
    /// A slider over `span`, currently at `value`.
    pub fn new(value: u16, span: Span) -> Self {
        Self {
            value: span.clamp(value),
            span,
            track: Rc::default(),
            dragging: false,
        }
    }

    /// Follow the device when it reports something other than what is shown.
    /// Not a choice, so nothing is emitted.
    pub fn sync(&mut self, value: u16, cx: &mut Context<Self>) {
        let value = self.span.clamp(value);
        if value != self.value && !self.dragging {
            self.value = value;
            cx.notify();
        }
    }

    fn choose(&mut self, value: u16, cx: &mut Context<Self>) {
        self.value = self.span.clamp(value);
        cx.emit(LevelChosen(self.value));
        cx.notify();
    }

    fn fraction(&self) -> f32 {
        let width = f32::from(self.span.maximum.saturating_sub(self.span.minimum)).max(1.0);
        f32::from(self.value.saturating_sub(self.span.minimum)) / width
    }

    fn at(&self, x: Pixels) -> Option<u16> {
        let track = self.track.get()?;
        let width = f32::from(track.size.width);
        if width <= 0.0 {
            return None;
        }
        let fraction = (f32::from(x - track.origin.x) / width).clamp(0.0, 1.0);
        let span = f32::from(self.span.maximum.saturating_sub(self.span.minimum));
        let step = f32::from(self.span.step.max(1));
        let offset = (fraction * span / step).round() * step;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "an offset within a u16 span, rounded to a whole step"
        )]
        let value = self.span.minimum.saturating_add(offset as u16);
        Some(self.span.clamp(value))
    }
}

/// The new value for a navigation key, or `None` for a key a slider ignores.
/// Pure, so the keyboard contract is tested without a window.
pub fn level_after_key(key: &str, value: u16, span: Span) -> Option<u16> {
    let step = span.step.max(1);
    Some(span.clamp(match key {
        "right" | "up" => value.saturating_add(step),
        "left" | "down" => value.saturating_sub(step),
        "pageup" => value.saturating_add(span.page()),
        "pagedown" => value.saturating_sub(span.page()),
        "home" => span.minimum,
        "end" => span.maximum,
        _ => return None,
    }))
}

/// A slider with a name, a place in the Tab order, the arrow, Page and
/// Home/End keys, assistive-technology increment and decrement, and a pointer.
///
/// Built here rather than taken from the component library, whose slider can
/// be given no name and takes no keyboard focus: on the desk panel it read as
/// "slider" three times over, and a keyboard could not reach it at all.
#[derive(IntoElement)]
pub struct LevelSlider {
    id: ElementId,
    model: Entity<LevelModel>,
    name: SharedString,
    /// How the value is said — "100%", "5000 K" — so the number is announced
    /// in the unit a person thinks in, not as a raw register value.
    spoken: SharedString,
}

impl LevelSlider {
    /// A slider over `model`, announced as `name` with its value as `spoken`.
    pub fn new(
        id: impl Into<ElementId>,
        model: &Entity<LevelModel>,
        name: impl Into<SharedString>,
        spoken: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            model: model.clone(),
            name: name.into(),
            spoken: spoken.into(),
        }
    }
}

impl RenderOnce for LevelSlider {
    #[expect(
        clippy::too_many_lines,
        reason = "every input path, keys, three assistive actions and a pointer drag, is wired in one chain"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let state = self.model.read(cx);
        let value = state.value;
        let span = state.span;
        let fraction = state.fraction();
        let track = state.track.clone();
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| {
                // A handle is not a Tab stop unless it says so; without this
                // every slider could be focused by a screen reader and never
                // reached with Tab.
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);
        let model = self.model.clone();

        slider_frame(
            self.id,
            self.name,
            self.spoken,
            [
                f64::from(value),
                f64::from(span.minimum),
                f64::from(span.maximum),
                f64::from(span.step.max(1)),
            ],
            &focus,
            focused,
            pal,
        )
        .on_key_down({
            let model = model.clone();
            move |event, _window, cx| {
                let key = event.keystroke.key.clone();
                model.update(cx, |state, cx| {
                    if let Some(next) = level_after_key(&key, state.value, state.span) {
                        cx.stop_propagation();
                        if next != state.value {
                            state.choose(next, cx);
                        }
                    }
                });
            }
        })
        .on_a11y_action(gpui::accesskit::Action::Increment, {
            let model = model.clone();
            move |_, _, cx| {
                model.update(cx, |state, cx| {
                    if let Some(next) = level_after_key("right", state.value, state.span) {
                        state.choose(next, cx);
                    }
                });
            }
        })
        .on_a11y_action(gpui::accesskit::Action::Decrement, {
            let model = model.clone();
            move |_, _, cx| {
                model.update(cx, |state, cx| {
                    if let Some(next) = level_after_key("left", state.value, state.span) {
                        state.choose(next, cx);
                    }
                });
            }
        })
        .on_a11y_action(gpui::accesskit::Action::SetValue, {
            let model = model.clone();
            move |data, _, cx| {
                if let Some(gpui::accesskit::ActionData::NumericValue(asked)) = data {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "clamped to u16's range first"
                    )]
                    let asked = asked.round().clamp(0.0, f64::from(u16::MAX)) as u16;
                    model.update(cx, |state, cx| state.choose(asked, cx));
                }
            }
        })
        .on_mouse_down(MouseButton::Left, {
            let model = model.clone();
            let focus = focus.clone();
            move |event, window, cx| {
                window.focus(&focus, cx);
                model.update(cx, |state, cx| {
                    if let Some(at) = state.at(event.position.x) {
                        state.dragging = true;
                        state.value = at;
                        cx.notify();
                    }
                });
            }
        })
        .on_mouse_move({
            let model = model.clone();
            move |event, _, cx| {
                model.update(cx, |state, cx| {
                    if state.dragging
                        && let Some(at) = state.at(event.position.x)
                        && at != state.value
                    {
                        state.value = at;
                        cx.notify();
                    }
                });
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let model = model.clone();
            move |_, _, cx| release(&model, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| release(&model, cx))
        .child(slider_track(fraction, track, pal))
    }
}

/// How a [`StateSlider`] says its value.
pub type Spoken = Rc<dyn Fn(f32) -> SharedString>;

/// [`LevelSlider`]'s keyboard, name and focus, driving the component
/// library's own `SliderState`, so a panel already built on that state and its
/// `Change` and `Release` events keeps every line of its logic.
///
/// Moving it emits `Change`, and every settled value — a key press, an
/// assistive-technology step, the release of a drag — emits `Release`, which is
/// exactly what a mouse drag on the library's slider emits.
#[derive(IntoElement)]
pub struct StateSlider {
    id: ElementId,
    state: Entity<gpui_component::slider::SliderState>,
    name: SharedString,
    spoken: Spoken,
}

impl StateSlider {
    /// A slider over `state`, announced as `name`, its value said by `spoken`.
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<gpui_component::slider::SliderState>,
        name: impl Into<SharedString>,
        spoken: impl Fn(f32) -> SharedString + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            name: name.into(),
            spoken: Rc::new(spoken),
        }
    }
}

/// Settle a library slider on `value`: move it, then say it moved and that
/// the move is final.
fn settle(
    state: &Entity<gpui_component::slider::SliderState>,
    value: f32,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_component::slider::{SliderEvent, SliderValue};
    state.update(cx, |slider, cx| {
        let (low, high, step) = (slider.min_value(), slider.max_value(), slider.step_value());
        let stepped = if step > 0.0 {
            low + ((value - low) / step).round() * step
        } else {
            value
        };
        let value = SliderValue::Single(stepped.clamp(low, high));
        slider.set_value(value, window, cx);
        cx.emit(SliderEvent::Change(value));
        cx.emit(SliderEvent::Release(value));
    });
}

/// The value a navigation key leads to on a library slider, or `None` for a
/// key it leaves alone.
pub fn state_after_key(key: &str, value: f32, low: f32, high: f32, step: f32) -> Option<f32> {
    let step = if step > 0.0 {
        step
    } else {
        (high - low) / 100.0
    };
    let page = (((high - low) / 10.0) / step).round().max(1.0) * step;
    Some(
        match key {
            "right" | "up" => value + step,
            "left" | "down" => value - step,
            "pageup" => value + page,
            "pagedown" => value - page,
            "home" => low,
            "end" => high,
            _ => return None,
        }
        .clamp(low, high),
    )
}

impl RenderOnce for StateSlider {
    #[expect(
        clippy::too_many_lines,
        reason = "every input path, keys, three assistive actions and a pointer drag, is wired in one chain"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pal = theme::palette(cx);
        let (value, low, high, step) = {
            let state = self.state.read(cx);
            (
                state.value().end(),
                state.min_value(),
                state.max_value(),
                state.step_value(),
            )
        };
        let fraction = if high > low {
            ((value - low) / (high - low)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| {
                // A handle is not a Tab stop unless it says so; without this
                // every slider could be focused by a screen reader and never
                // reached with Tab.
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let track = window
            .use_keyed_state((self.id.clone(), "track"), cx, |_, _| {
                Rc::new(Cell::new(None::<Bounds<Pixels>>))
            })
            .read(cx)
            .clone();
        let dragging = window
            .use_keyed_state((self.id.clone(), "dragging"), cx, |_, _| {
                Rc::new(Cell::new(false))
            })
            .read(cx)
            .clone();
        let focused = focus.is_focused(window);
        let state = self.state.clone();
        let at = {
            let track = track.clone();
            move |x: Pixels| -> Option<f32> {
                let bounds = track.get()?;
                let width = f32::from(bounds.size.width);
                (width > 0.0).then(|| {
                    let fraction = (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0);
                    low + (high - low) * fraction
                })
            }
        };

        slider_frame(
            self.id,
            self.name,
            (self.spoken)(value),
            [
                f64::from(value),
                f64::from(low),
                f64::from(high),
                f64::from(if step > 0.0 { step } else { 1.0 }),
            ],
            &focus,
            focused,
            pal,
        )
        .on_key_down({
            let state = state.clone();
            move |event, window, cx| {
                if let Some(next) = state_after_key(&event.keystroke.key, value, low, high, step) {
                    cx.stop_propagation();
                    settle(&state, next, window, cx);
                }
            }
        })
        .on_a11y_action(gpui::accesskit::Action::Increment, {
            let state = state.clone();
            move |_, window, cx| {
                if let Some(next) = state_after_key("right", value, low, high, step) {
                    settle(&state, next, window, cx);
                }
            }
        })
        .on_a11y_action(gpui::accesskit::Action::Decrement, {
            let state = state.clone();
            move |_, window, cx| {
                if let Some(next) = state_after_key("left", value, low, high, step) {
                    settle(&state, next, window, cx);
                }
            }
        })
        .on_a11y_action(gpui::accesskit::Action::SetValue, {
            let state = state.clone();
            move |data, window, cx| {
                if let Some(gpui::accesskit::ActionData::NumericValue(asked)) = data {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a slider value, well inside f32's range"
                    )]
                    settle(&state, *asked as f32, window, cx);
                }
            }
        })
        .on_mouse_down(MouseButton::Left, {
            let state = state.clone();
            let focus = focus.clone();
            let dragging = dragging.clone();
            let at = at.clone();
            move |event, window, cx| {
                window.focus(&focus, cx);
                if let Some(next) = at(event.position.x) {
                    dragging.set(true);
                    state.update(cx, |slider, cx| {
                        let value = gpui_component::slider::SliderValue::Single(next);
                        slider.set_value(value, window, cx);
                        cx.emit(gpui_component::slider::SliderEvent::Change(value));
                    });
                }
            }
        })
        .on_mouse_move({
            let state = state.clone();
            let dragging = dragging.clone();
            let at = at.clone();
            move |event, window, cx| {
                if dragging.get()
                    && let Some(next) = at(event.position.x)
                {
                    state.update(cx, |slider, cx| {
                        let value = gpui_component::slider::SliderValue::Single(next);
                        slider.set_value(value, window, cx);
                        cx.emit(gpui_component::slider::SliderEvent::Change(value));
                    });
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let state = state.clone();
            let dragging = dragging.clone();
            move |_, window, cx| {
                if dragging.replace(false) {
                    let value = state.read(cx).value().end();
                    settle(&state, value, window, cx);
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let state = state.clone();
            move |_, window, cx| {
                if dragging.replace(false) {
                    let value = state.read(cx).value().end();
                    settle(&state, value, window, cx);
                }
            }
        })
        .child(slider_track(fraction, track, pal))
    }
}

fn release(model: &Entity<LevelModel>, cx: &mut App) {
    model.update(cx, |state, cx| {
        if state.dragging {
            state.dragging = false;
            let value = state.value;
            state.choose(value, cx);
        }
    });
}

/// Make a [`LevelModel`] in a view's context.
pub fn level_model<V>(value: u16, span: Span, cx: &mut Context<V>) -> Entity<LevelModel> {
    cx.new(|_| LevelModel::new(value, span))
}

/// What both sliders share: the slider node — named, valued, focusable — and
/// a focus ring that does not depend on colour alone, a 2px border shown only
/// while focused, so a magnifier user following focus sees it at any zoom.
/// `numbers` is the value, minimum, maximum and step.
fn slider_frame(
    id: ElementId,
    name: SharedString,
    spoken: SharedString,
    numbers: [f64; 4],
    focus: &FocusHandle,
    focused: bool,
    pal: theme::Palette,
) -> gpui::Stateful<gpui::Div> {
    let [value, minimum, maximum, step] = numbers;
    div()
        .id(id)
        .role(Role::Slider)
        .aria_label(name)
        .aria_value(spoken)
        .aria_numeric_value(value)
        .aria_min_numeric_value(minimum)
        .aria_max_numeric_value(maximum)
        .aria_numeric_value_step(step)
        .track_focus(focus)
        .w_full()
        .h(px(28.))
        .flex()
        .items_center()
        .rounded_md()
        .border_2()
        .border_color(if focused {
            pal.text_primary
        } else {
            gpui::transparent_black()
        })
        .px_2()
}

/// The drawn track: groove, filled part and thumb, recording its own bounds
/// in `track` so a pointer position can be turned into a value.
fn slider_track(
    fraction: f32,
    track: Rc<Cell<Option<Bounds<Pixels>>>>,
    pal: theme::Palette,
) -> gpui::Div {
    div()
        .relative()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(pal.border)
        .child(
            gpui::canvas(
                move |bounds, _, _| track.set(Some(bounds)),
                |_, (), _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(relative(fraction))
                .rounded_full()
                .bg(accent()),
        )
        .child(
            div()
                .absolute()
                .top(px(-5.))
                .left(relative(fraction))
                .ml(px(-8.))
                .size(px(16.))
                .rounded_full()
                .bg(pal.panel)
                .border_2()
                .border_color(accent()),
        )
}

#[cfg(test)]
mod tests {
    use super::{Span, level_after_key};

    const PERCENT: Span = Span::to(100);
    /// A Key Light's colour temperature.
    const KELVIN: Span = Span {
        minimum: 2900,
        maximum: 7000,
        step: 50,
    };

    #[test]
    fn the_arrow_keys_move_one_step_and_stop_at_the_ends() {
        assert_eq!(level_after_key("right", 50, PERCENT), Some(51));
        assert_eq!(level_after_key("up", 50, PERCENT), Some(51));
        assert_eq!(level_after_key("left", 50, PERCENT), Some(49));
        assert_eq!(level_after_key("down", 50, PERCENT), Some(49));
        assert_eq!(level_after_key("right", 100, PERCENT), Some(100));
        assert_eq!(level_after_key("left", 0, PERCENT), Some(0));
    }

    #[test]
    fn page_home_and_end_jump() {
        assert_eq!(level_after_key("pageup", 95, PERCENT), Some(100));
        assert_eq!(level_after_key("pagedown", 5, PERCENT), Some(0));
        assert_eq!(level_after_key("pageup", 40, PERCENT), Some(50));
        assert_eq!(level_after_key("home", 37, PERCENT), Some(0));
        assert_eq!(level_after_key("end", 37, PERCENT), Some(100));
    }

    #[test]
    fn a_span_with_a_floor_and_a_step_is_honoured() {
        assert_eq!(level_after_key("right", 5000, KELVIN), Some(5050));
        assert_eq!(level_after_key("left", 2900, KELVIN), Some(2900));
        assert_eq!(level_after_key("home", 5000, KELVIN), Some(2900));
        // Ten notches of 4100 kelvin, rounded to whole steps of 50.
        assert_eq!(level_after_key("pageup", 2900, KELVIN), Some(3300));
    }

    #[test]
    fn a_key_a_slider_does_not_use_is_left_for_others() {
        // Tab especially: a slider that swallowed it would trap focus.
        assert_eq!(level_after_key("tab", 37, PERCENT), None);
        assert_eq!(level_after_key("a", 37, PERCENT), None);
    }
}
