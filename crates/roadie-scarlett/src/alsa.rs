//! What each control is called on Linux, where the kernel already owns the
//! interface.
//!
//! This module is the Linux half of the story, and it exists because the rest
//! of this crate does **not** apply there.
//!
//! On Linux the kernel's `snd-usb-audio` claims the Scarlett's control
//! interface, so reaching it over raw USB would mean detaching the driver that
//! carries the audio — on an audio interface. There is no need to: the kernel
//! already exposes every one of these settings as an ordinary ALSA mixer
//! control. So on Linux a host reads and writes *names*, and this module is
//! how a model plus an input number becomes one.
//!
//! [`crate::packet`] and [`crate::transaction`] are for macOS and Windows,
//! where nothing claims the interface.
//!
//! # Why the names are worth generating rather than guessing
//!
//! They look regular and are not. The number in a name is the input as a
//! person counts it, from one — but which input a control belongs to is a
//! per-model fact. A Scarlett Solo 4th Gen has one phantom switch and it is on
//! input **two**. A 2i2 3rd Gen has one that covers **both** inputs, so its
//! control is named for a range. An 18i20 3rd Gen groups **four** inputs per
//! switch. And the 4th generation turned "air" from a switch into a choice,
//! which changes the last word of its name.
//!
//! Get one wrong and nothing fails loudly: the name simply matches no control,
//! and the setting silently does not exist.

use core::fmt::Write as _;

use crate::device::Model;

/// A control's kind, which is the last word of its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// On or off.
    Switch,
    /// A choice among several settings.
    Enum,
    /// A continuous value.
    Volume,
}

impl Kind {
    const fn word(self) -> &'static str {
        match self {
            Self::Switch => "Switch",
            Self::Enum => "Enum",
            Self::Volume => "Volume",
        }
    }
}

/// The settings this module can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Control {
    /// 48 V phantom power. See [`crate::risk`] before writing one.
    PhantomPower,
    /// Line or instrument level.
    Level,
    /// The 10 dB pad.
    Pad,
    /// Focusrite's "air" voicing.
    Air,
    /// Preamp gain.
    Gain,
    /// The automatic gain routine.
    Autogain,
    /// Clip-safe.
    Safe,
    /// The Vocaster's DSP.
    Dsp,
    /// Muting an input.
    Mute,
}

impl Control {
    /// The words that go in the middle of the name.
    const fn word(self) -> &'static str {
        match self {
            Self::PhantomPower => "Phantom Power",
            Self::Level => "Level",
            Self::Pad => "Pad",
            Self::Air => "Air",
            Self::Gain => "Gain",
            Self::Autogain => "Autogain",
            Self::Safe => "Safe",
            Self::Dsp => "DSP",
            Self::Mute => "Mute",
        }
    }

    /// How many of this control a model has, and the first input it applies to.
    const fn extent(self, model: &Model) -> (u8, u8) {
        match self {
            Self::PhantomPower => (model.phantom_pairs, model.phantom_first),
            Self::Level => (model.level_inputs, model.level_first),
            Self::Pad => (model.pad_inputs, 0),
            Self::Air => (model.air_inputs, model.air_first),
            // The gain family all follow the gain count and start at input one.
            Self::Gain | Self::Autogain | Self::Safe => (model.gain_inputs, 0),
            Self::Dsp => (model.dsp_inputs, 0),
            Self::Mute => (model.mute_inputs, 0),
        }
    }

    /// This control's kind on this model.
    const fn kind(self, model: &Model) -> Kind {
        match self {
            Self::Level => Kind::Enum,
            // A switch on the older families and a choice on the 4th
            // generation, which changes the last word of the name.
            Self::Air => {
                if model.air_is_enum {
                    Kind::Enum
                } else {
                    Kind::Switch
                }
            }
            Self::Gain => Kind::Volume,
            Self::PhantomPower
            | Self::Pad
            | Self::Autogain
            | Self::Safe
            | Self::Dsp
            | Self::Mute => Kind::Switch,
        }
    }
}

/// How many of `control` this model has.
///
/// Zero is the answer for a control the model does not have, and a caller
/// should treat it as "this interface cannot do that" rather than as an error.
#[must_use]
pub const fn count(model: &Model, control: Control) -> u8 {
    control.extent(model).0
}

/// The ALSA control name for instance `index` of `control`, counting from zero.
///
/// `None` when this model has no such control, or has fewer than `index + 1`
/// of them — which is the same answer for the same reason: there is no control
/// to name, and inventing a name would produce one that matches nothing.
#[must_use]
pub fn name(model: &Model, control: Control, index: u8) -> Option<String> {
    let (count, first) = control.extent(model);
    if index >= count {
        return None;
    }
    let kind = control.kind(model).word();
    let word = control.word();

    // Phantom power on most of the 3rd generation covers several inputs at
    // once, and its control is named for the range rather than for one input.
    // Every other control is per-input.
    let per = if control == Control::PhantomPower {
        model.inputs_per_phantom
    } else {
        1
    };

    let mut out = String::new();
    if per > 1 {
        let from = index.saturating_mul(per).saturating_add(1);
        let to = index.saturating_add(1).saturating_mul(per);
        // Writing to a String cannot fail; the result is discarded rather than
        // unwrapped so this stays panic-free.
        let _ = write!(out, "Line In {from}-{to} {word} Capture {kind}");
    } else {
        let at = index.saturating_add(1).saturating_add(first);
        let _ = write!(out, "Line In {at} {word} Capture {kind}");
    }
    Some(out)
}

/// Every name this model has for `control`, in input order.
#[must_use]
pub fn names(model: &Model, control: Control) -> Vec<String> {
    (0..count(model, control))
        .filter_map(|index| name(model, control, index))
        .collect()
}

/// The one control whose name carries no input number.
///
/// Whether phantom power survives a power cycle is a property of the
/// interface rather than of an input, so it is named without one.
pub const PHANTOM_PERSISTENCE: &str = "Phantom Power Persistence Capture Switch";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{VENDOR_ID, find};

    fn model(product_id: u16) -> &'static Model {
        find(VENDOR_ID, product_id).expect("a known interface")
    }

    #[test]
    fn an_ordinary_switch_is_named_for_its_input_counting_from_one() {
        // The 4i4 3rd Gen: two pads, on inputs one and two.
        let pads = names(model(0x8212), Control::Pad);
        assert_eq!(
            pads,
            [
                "Line In 1 Pad Capture Switch",
                "Line In 2 Pad Capture Switch"
            ]
        );
    }

    #[test]
    fn a_phantom_switch_covering_two_inputs_is_named_for_the_range() {
        // The 2i2 3rd Gen has one switch across both inputs. Naming it
        // "Line In 1" would match no control at all.
        let phantom = names(model(0x8210), Control::PhantomPower);
        assert_eq!(phantom, ["Line In 1-2 Phantom Power Capture Switch"]);
    }

    #[test]
    fn a_phantom_switch_covering_four_inputs_counts_in_fours() {
        // The 18i20 3rd Gen: two switches, four inputs each.
        let phantom = names(model(0x8215), Control::PhantomPower);
        assert_eq!(
            phantom,
            [
                "Line In 1-4 Phantom Power Capture Switch",
                "Line In 5-8 Phantom Power Capture Switch"
            ]
        );
    }

    #[test]
    fn a_solo_fourth_gen_puts_its_phantom_switch_on_the_second_input() {
        // The case most likely to be got wrong by assuming controls start at
        // input one. This interface has exactly one phantom switch and it is
        // not on input one.
        let phantom = names(model(0x8218), Control::PhantomPower);
        assert_eq!(phantom, ["Line In 2 Phantom Power Capture Switch"]);
    }

    #[test]
    fn a_solo_third_gen_puts_its_level_control_on_the_second_input_too() {
        // Same shape, different control and a different model — so the offset
        // is genuinely per-control and not a single quirk.
        let level = names(model(0x8211), Control::Level);
        assert_eq!(level, ["Line In 2 Level Capture Enum"]);
    }

    #[test]
    fn air_changed_from_a_switch_to_a_choice_in_the_fourth_generation() {
        // The last word of the name changes with it, so a host that assumed
        // "Switch" would address nothing on the newer interfaces.
        assert_eq!(
            names(model(0x8212), Control::Air),
            [
                "Line In 1 Air Capture Switch",
                "Line In 2 Air Capture Switch"
            ]
        );
        assert_eq!(
            names(model(0x821A), Control::Air),
            ["Line In 1 Air Capture Enum", "Line In 2 Air Capture Enum"]
        );
    }

    #[test]
    fn gain_is_a_volume_and_the_rest_of_its_family_are_switches() {
        let gen4 = model(0x821A);
        assert_eq!(
            names(gen4, Control::Gain),
            [
                "Line In 1 Gain Capture Volume",
                "Line In 2 Gain Capture Volume"
            ]
        );
        assert_eq!(
            names(gen4, Control::Safe),
            [
                "Line In 1 Safe Capture Switch",
                "Line In 2 Safe Capture Switch"
            ]
        );
    }

    #[test]
    fn a_control_the_model_lacks_is_named_not_at_all() {
        // The 2nd generation has no software phantom power. Producing a name
        // would produce one matching nothing, which is worse than saying no.
        let gen2 = model(0x8203);
        assert_eq!(count(gen2, Control::PhantomPower), 0);
        assert_eq!(name(gen2, Control::PhantomPower, 0), None);
        assert_eq!(
            names(gen2, Control::PhantomPower),
            [] as [std::string::String; 0]
        );
    }

    #[test]
    fn asking_past_the_last_one_is_the_same_answer() {
        let gen4 = model(0x821A);
        assert!(name(gen4, Control::Gain, 1).is_some());
        assert_eq!(name(gen4, Control::Gain, 2), None);
        assert_eq!(name(gen4, Control::Gain, u8::MAX), None);
    }

    #[test]
    fn every_model_names_as_many_controls_as_it_claims_to_have() {
        // Guards the two halves against drifting apart: the count comes from
        // one field and the names from a loop over it.
        for model in crate::device::MODELS {
            for control in [
                Control::PhantomPower,
                Control::Level,
                Control::Pad,
                Control::Air,
                Control::Gain,
                Control::Autogain,
                Control::Safe,
                Control::Dsp,
                Control::Mute,
            ] {
                assert_eq!(
                    names(model, control).len(),
                    usize::from(count(model, control)),
                    "{} names a different number of {control:?} than it has",
                    model.name
                );
            }
        }
    }

    #[test]
    fn no_model_names_two_controls_the_same_thing() {
        // A collision would mean two settings addressing one control, and the
        // second silently overwriting the first.
        for model in crate::device::MODELS {
            let mut all: Vec<String> = [
                Control::PhantomPower,
                Control::Level,
                Control::Pad,
                Control::Air,
                Control::Gain,
                Control::Autogain,
                Control::Safe,
                Control::Dsp,
                Control::Mute,
            ]
            .into_iter()
            .flat_map(|control| names(model, control))
            .collect();
            let before = all.len();
            all.sort_unstable();
            all.dedup();
            assert_eq!(all.len(), before, "{} names two controls alike", model.name);
        }
    }

    #[test]
    fn the_persistence_control_carries_no_input_number() {
        // It is a property of the interface, not of an input.
        assert!(!PHANTOM_PERSISTENCE.contains("Line In"));
    }
}
