//! Control reports out, key events in.
//!
//! Every function here is a pure transformation between typed values and
//! bytes: nothing opens a device, and nothing here decides *when* a report
//! is sent. A host layer pairs these with an open HID handle.
//!
//! Buffers are produced in the shape the platform HID APIs expect for a
//! numbered report — the report ID is byte 0, and the rest is the payload —
//! so a caller hands [`FeatureReport::as_bytes`] straight to a feature-report
//! write, and hands a read buffer straight to [`decode_key_states`].

use crate::ProtocolError;
use crate::model::{Generation, Model};

/// Report ID that carries key state on every model.
const KEY_STATE_REPORT_ID: u8 = 0x01;

/// Where key states begin in a [`Generation::Gen1`] input report, past the
/// report ID.
const GEN1_KEY_OFFSET: usize = 1;

/// Where key states begin in a [`Generation::Gen2`] input report: the report
/// ID plus a three-byte header.
const GEN2_KEY_OFFSET: usize = 4;

/// Total length a [`Generation::Gen1`] feature report is padded to.
const GEN1_FEATURE_LEN: usize = 17;

/// Total length a [`Generation::Gen2`] feature report is padded to.
const GEN2_FEATURE_LEN: usize = 32;

/// A key screen backlight level, as a percentage.
///
/// A newtype rather than a bare `u8` because the wire field is a percentage
/// and the hardware's behavior above 100 is undefined — rejecting the value
/// at construction keeps that from ever reaching a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Brightness(u8);

impl Brightness {
    /// Full brightness.
    pub const FULL: Self = Self(100);
    /// Screens off, without powering the device down.
    pub const OFF: Self = Self(0);
    /// Clearly dimmed but still lit — a visible change to check a device by,
    /// without leaving anyone looking at dark keys if the restore fails.
    pub const DIM: Self = Self(30);

    /// Build a brightness from a percentage.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::BrightnessOutOfRange`] above 100.
    pub const fn new(percent: u8) -> Result<Self, ProtocolError> {
        if percent > 100 {
            return Err(ProtocolError::BrightnessOutOfRange { percent });
        }
        Ok(Self(percent))
    }

    /// The percentage this value carries.
    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }
}

/// One outbound feature report, report ID included as byte 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureReport(Vec<u8>);

impl FeatureReport {
    /// The report ID this report is addressed to.
    ///
    /// Never panics: every constructor in this module writes the ID first.
    #[must_use]
    pub fn report_id(&self) -> u8 {
        self.0.first().copied().unwrap_or_default()
    }

    /// The full buffer, ready to hand to a feature-report write.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Pad `bytes` out to the fixed length this generation's feature reports use.
fn padded(mut bytes: Vec<u8>, len: usize) -> FeatureReport {
    bytes.resize(len, 0);
    FeatureReport(bytes)
}

/// Build the report that sets key screen brightness.
#[must_use]
pub fn set_brightness(model: &Model, brightness: Brightness) -> FeatureReport {
    match model.generation {
        Generation::Gen1 => padded(
            vec![0x05, 0x55, 0xaa, 0xd1, 0x01, brightness.percent()],
            GEN1_FEATURE_LEN,
        ),
        Generation::Gen2 => padded(vec![0x03, 0x08, brightness.percent()], GEN2_FEATURE_LEN),
    }
}

/// Build the report that resets the device to its stock standby screen,
/// clearing every uploaded key image.
#[must_use]
pub fn reset(model: &Model) -> FeatureReport {
    match model.generation {
        Generation::Gen1 => padded(vec![0x0b, 0x63], GEN1_FEATURE_LEN),
        Generation::Gen2 => padded(vec![0x03, 0x02], GEN2_FEATURE_LEN),
    }
}

/// Whether a key is down, for every key on a model.
///
/// Held as one entry per key, already un-mirrored into key-index order, so a
/// caller never has to remember whether this model scans its rows backwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStates(Vec<bool>);

impl KeyStates {
    /// All keys up — the state to diff a device's first report against.
    #[must_use]
    pub fn released(model: &Model) -> Self {
        Self(vec![false; usize::from(model.key_count())])
    }

    /// Whether the key at `index` is currently down.
    ///
    /// Returns `false` for an index this model does not have, which is the
    /// truthful answer: a key that does not exist is not being pressed.
    #[must_use]
    pub fn is_pressed(&self, index: u16) -> bool {
        self.0.get(usize::from(index)).copied().unwrap_or(false)
    }

    /// How many keys this state covers.
    #[must_use]
    pub fn len(&self) -> u16 {
        // Bounded by the model's key count, which no catalog entry puts
        // anywhere near u16::MAX.
        u16::try_from(self.0.len()).unwrap_or(u16::MAX)
    }

    /// Whether this state covers no keys at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The transitions from `previous` to this state, in key order.
    ///
    /// Diffing rather than trusting the device to send edges: Stream Decks
    /// report the whole keyboard on every report, so a dropped or coalesced
    /// report would otherwise strand a key in the wrong state.
    #[must_use]
    pub fn changes_since(&self, previous: &Self) -> Vec<KeyEvent> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(index, &pressed)| {
                let key = u16::try_from(index).ok()?;
                if pressed == previous.is_pressed(key) {
                    return None;
                }
                Some(KeyEvent {
                    key,
                    action: if pressed {
                        KeyAction::Pressed
                    } else {
                        KeyAction::Released
                    },
                })
            })
            .collect()
    }
}

/// Which way a key moved.
///
/// Named rather than a `bool`, so a call site cannot silently invert it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// The key went down.
    Pressed,
    /// The key came back up.
    Released,
}

/// One key transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Key index, in the model's own left-to-right, top-to-bottom order.
    pub key: u16,
    /// Which way it moved.
    pub action: KeyAction,
}

/// Decode an input report into the key states it carries.
///
/// `report` is the buffer as read from the device, report ID included.
///
/// # Errors
///
/// Returns [`ProtocolError::UnexpectedReport`] for a report ID this model
/// does not use for key state, and [`ProtocolError::ShortReport`] when the
/// buffer ends before the model's last key — either would otherwise be read
/// as "every remaining key is up", inventing key releases that never
/// happened.
pub fn decode_key_states(model: &Model, report: &[u8]) -> Result<KeyStates, ProtocolError> {
    let Some(&report_id) = report.first() else {
        return Err(ProtocolError::ShortReport {
            expected: 1,
            found: 0,
        });
    };
    if report_id != KEY_STATE_REPORT_ID {
        return Err(ProtocolError::UnexpectedReport { report_id });
    }
    let offset = match model.generation {
        Generation::Gen1 => GEN1_KEY_OFFSET,
        Generation::Gen2 => GEN2_KEY_OFFSET,
    };
    let count = usize::from(model.key_count());
    let expected = offset + count;
    if report.len() < expected {
        return Err(ProtocolError::ShortReport {
            expected,
            found: report.len(),
        });
    }

    let mut states = vec![false; count];
    for (reported, &byte) in report[offset..expected].iter().enumerate() {
        let reported = u16::try_from(reported).map_err(|_| ProtocolError::KeyOutOfRange {
            index: u16::MAX,
            count: model.key_count(),
        })?;
        let index = model.key_index_from_reported(reported)?;
        // Any non-zero byte is a press; the hardware sends 1, but treating
        // only 1 as pressed would drop a key on firmware that reports a
        // pressure or a repeat count instead.
        states[usize::from(index)] = byte != 0;
    }
    Ok(KeyStates(states))
}

#[cfg(test)]
mod tests {
    use super::{Brightness, KeyAction, KeyStates, decode_key_states, reset, set_brightness};
    use crate::ProtocolError;
    use crate::model::{ELGATO_VENDOR_ID, MODELS, identify};

    /// The MK.2: gen 2, 15 keys, reported left-to-right.
    fn mk2() -> &'static crate::model::Model {
        identify(ELGATO_VENDOR_ID, 0x0080).expect("the MK.2 is catalogued")
    }

    /// The original: gen 1, 15 keys, reported right-to-left within a row.
    fn original() -> &'static crate::model::Model {
        identify(ELGATO_VENDOR_ID, 0x0060).expect("the original is catalogued")
    }

    #[test]
    fn the_named_brightness_levels_are_in_range() {
        // These are constructed by hand rather than through `new`, so the
        // invariant `new` enforces is asserted for them too.
        for level in [Brightness::OFF, Brightness::DIM, Brightness::FULL] {
            assert!(level.percent() <= 100);
            assert_eq!(
                Brightness::new(level.percent()).expect("a valid percentage"),
                level
            );
        }
        assert!(Brightness::OFF < Brightness::DIM);
        assert!(Brightness::DIM < Brightness::FULL);
    }

    #[test]
    fn brightness_rejects_more_than_full() {
        let error = Brightness::new(101).expect_err("101 percent is not a brightness");
        assert!(matches!(
            error,
            ProtocolError::BrightnessOutOfRange { percent: 101 }
        ));
        assert_eq!(
            Brightness::new(100).expect("100 is valid"),
            Brightness::FULL
        );
        assert_eq!(Brightness::new(0).expect("0 is valid"), Brightness::OFF);
    }

    #[test]
    fn gen2_brightness_carries_the_percentage_in_its_third_byte() {
        let report = set_brightness(mk2(), Brightness::new(60).expect("60 is valid"));
        assert_eq!(report.report_id(), 0x03);
        assert_eq!(&report.as_bytes()[..3], &[0x03, 0x08, 60]);
        assert_eq!(report.as_bytes().len(), 32);
    }

    #[test]
    fn gen1_brightness_uses_its_own_prefix_and_length() {
        let report = set_brightness(original(), Brightness::new(60).expect("60 is valid"));
        assert_eq!(report.report_id(), 0x05);
        assert_eq!(&report.as_bytes()[..6], &[0x05, 0x55, 0xaa, 0xd1, 0x01, 60]);
        assert_eq!(report.as_bytes().len(), 17);
    }

    #[test]
    fn reset_differs_per_generation() {
        assert_eq!(&reset(mk2()).as_bytes()[..2], &[0x03, 0x02]);
        assert_eq!(&reset(original()).as_bytes()[..2], &[0x0b, 0x63]);
    }

    /// Build a gen-2 key report with `pressed` reported positions down.
    fn gen2_report(pressed: &[usize]) -> Vec<u8> {
        let mut report = vec![0u8; 4 + 15];
        report[0] = 0x01;
        for &index in pressed {
            report[4 + index] = 1;
        }
        report
    }

    #[test]
    fn a_gen2_report_decodes_at_its_own_offset() {
        let states = decode_key_states(mk2(), &gen2_report(&[0, 7])).expect("a valid report");
        assert!(states.is_pressed(0));
        assert!(states.is_pressed(7));
        assert!(!states.is_pressed(1));
        assert_eq!(states.len(), 15);
    }

    #[test]
    fn a_gen1_report_is_unmirrored_into_key_order() {
        let mut report = vec![0u8; 1 + 15];
        report[0] = 0x01;
        // Reported position 0 is the first row's rightmost key, index 4.
        report[1] = 1;
        let states = decode_key_states(original(), &report).expect("a valid report");
        assert!(states.is_pressed(4), "reported 0 must land on key 4");
        assert!(!states.is_pressed(0));
    }

    #[test]
    fn a_short_report_is_an_error_not_a_row_of_releases() {
        let mut truncated = gen2_report(&[]);
        truncated.truncate(10);
        let error = decode_key_states(mk2(), &truncated).expect_err("a truncated report");
        assert!(matches!(
            error,
            ProtocolError::ShortReport {
                expected: 19,
                found: 10
            }
        ));
    }

    #[test]
    fn an_empty_report_is_short_rather_than_a_panic() {
        let error = decode_key_states(mk2(), &[]).expect_err("an empty report");
        assert!(matches!(
            error,
            ProtocolError::ShortReport {
                expected: 1,
                found: 0
            }
        ));
    }

    #[test]
    fn another_report_id_is_refused() {
        let mut foreign = gen2_report(&[]);
        foreign[0] = 0x02;
        let error = decode_key_states(mk2(), &foreign).expect_err("a foreign report");
        assert!(matches!(
            error,
            ProtocolError::UnexpectedReport { report_id: 0x02 }
        ));
    }

    #[test]
    fn any_non_zero_byte_counts_as_a_press() {
        let mut report = gen2_report(&[]);
        report[4] = 0xff;
        let states = decode_key_states(mk2(), &report).expect("a valid report");
        assert!(states.is_pressed(0));
    }

    #[test]
    fn diffing_reports_only_the_keys_that_moved() {
        let idle = KeyStates::released(mk2());
        let one_down = decode_key_states(mk2(), &gen2_report(&[3])).expect("a valid report");
        let events = one_down.changes_since(&idle);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, 3);
        assert_eq!(events[0].action, KeyAction::Pressed);

        // Holding the same key produces nothing at all.
        let still_down = decode_key_states(mk2(), &gen2_report(&[3])).expect("a valid report");
        assert_eq!(still_down.changes_since(&one_down).len(), 0);

        // Letting go produces exactly one release.
        let events = idle.changes_since(&still_down);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, 3);
        assert_eq!(events[0].action, KeyAction::Released);
    }

    #[test]
    fn simultaneous_transitions_are_all_reported_in_key_order() {
        let idle = KeyStates::released(mk2());
        let two_down = decode_key_states(mk2(), &gen2_report(&[9, 2])).expect("a valid report");
        let events = two_down.changes_since(&idle);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].key, 2, "events come back in key order");
        assert_eq!(events[1].key, 9);
    }

    #[test]
    fn a_missed_report_still_resolves_to_the_right_state() {
        // Press 1, then a report where 1 is up and 5 is down arrives with the
        // intermediate report lost. Both transitions must still surface.
        let one_down = decode_key_states(mk2(), &gen2_report(&[1])).expect("a valid report");
        let five_down = decode_key_states(mk2(), &gen2_report(&[5])).expect("a valid report");
        let events = five_down.changes_since(&one_down);
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].key, events[0].action), (1, KeyAction::Released));
        assert_eq!((events[1].key, events[1].action), (5, KeyAction::Pressed));
    }

    /// Arbitrary bytes from a device must never panic the decoder.
    ///
    /// This is the closest thing to hardware testing available here. A real
    /// Stream Deck is the one thing this project cannot get its hands on, so
    /// the next best assurance is to assume it sends anything at all — a
    /// truncated report, a foreign report id, a burst of noise from a device
    /// being unplugged mid-transfer — and require an error rather than a
    /// crash. A panic here takes down the process that was watching for key
    /// presses, which on this project's own terms is someone's macro pad
    /// going dead mid-use.
    ///
    /// Deterministic rather than randomised: a fuzz test that finds a crash
    /// only on some runs is a fuzz test nobody can act on. The generator is a
    /// plain LCG so a failure names a seed that reproduces it.
    #[test]
    fn no_report_of_any_shape_panics_the_decoder() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for model in MODELS {
            for case in 0..20_000_u32 {
                let len = (next() % 600) as usize;
                let mut report = vec![0_u8; len];
                for byte in &mut report {
                    *byte = (next() & 0xff) as u8;
                }
                // Whatever comes back, it must be a value or an error, and the
                // states it reports must be self-consistent: nothing may claim
                // more keys than the model has.
                if let Ok(states) = decode_key_states(model, &report) {
                    assert_eq!(
                        states.len(),
                        model.key_count(),
                        "{} case {case}: decoded {} key states for a {}-key model",
                        model.name,
                        states.len(),
                        model.key_count()
                    );
                    // And reading every key must stay inside the buffer.
                    for index in 0..states.len() {
                        let _ = states.is_pressed(index);
                    }
                    // Diffing against a fresh state must not panic either:
                    // that is the path a watcher runs on every report.
                    let _ = states.changes_since(&KeyStates::released(model));
                }
            }
        }
    }

    /// The same for a key index past the end, which a diff could produce if a
    /// model's key count and a report ever disagreed.
    #[test]
    fn asking_about_a_key_that_does_not_exist_is_false_rather_than_a_panic() {
        for model in MODELS {
            let states = KeyStates::released(model);
            assert!(!states.is_pressed(model.key_count()));
            assert!(!states.is_pressed(u16::MAX));
        }
    }
}
