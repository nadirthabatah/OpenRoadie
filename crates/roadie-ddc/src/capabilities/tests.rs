//! Capability-string tests.
//!
//! The well-formed fixture is shaped like the strings Dell and LG panels
//! actually return. The malformed ones are not inventions either: unbalanced
//! parentheses, a missing outer wrapper, and junk between segments are all
//! things monitors ship, and each has its own test because "be forgiving" is a
//! claim that means nothing until the specific forgiveness is pinned down.

use super::*;

const DELL: &str = "(prot(monitor)type(lcd)model(U2723QE)cmds(01 02 03 07 0C E3 F3)\
vcp(02 04 08 10 12 14(01 05 08 0B) 16 18 1A 52 60(0F 10 11 1B) AC AE B2 B6 C0 C6 C8 C9 \
D6(01 04 05) DF)mswhql(1)asset_eep(40)mccs_ver(2.1))";

#[test]
fn a_well_formed_string_parses_without_a_single_warning() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    assert_eq!(caps.model.as_deref(), Some("U2723QE"));
    assert_eq!(caps.display_type.as_deref(), Some("lcd"));
    assert_eq!(caps.mccs_version.as_deref(), Some("2.1"));
    assert_eq!(caps.commands, [0x01, 0x02, 0x03, 0x07, 0x0C, 0xE3, 0xF3]);
    assert_eq!(caps.warnings, Vec::<String>::new());
}

#[test]
fn the_feature_list_keeps_the_order_the_monitor_gave() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    let codes: Vec<u8> = caps.features.iter().map(|e| e.feature.code()).collect();
    assert_eq!(
        codes,
        [
            0x02, 0x04, 0x08, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1A, 0x52, 0x60, 0xAC, 0xAE, 0xB2,
            0xB6, 0xC0, 0xC6, 0xC8, 0xC9, 0xD6, 0xDF
        ]
    );
}

#[test]
fn a_continuous_feature_lists_no_values() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    let brightness = caps.entry(Feature::Brightness).unwrap();
    assert_eq!(brightness.values, [] as [u8; 0]);
}

#[test]
fn a_discrete_feature_lists_the_values_the_monitor_accepts() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    let power = caps.entry(Feature::PowerMode).unwrap();
    assert_eq!(power.values, [0x01, 0x04, 0x05]);
}

#[test]
fn the_monitors_own_input_list_is_what_a_caller_gets() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    // 0x1b is this panel's USB-C. No specification names it; only the monitor
    // does, which is the whole reason to parse this string.
    assert_eq!(
        caps.inputs(),
        [
            InputSource::DisplayPort1,
            InputSource::DisplayPort2,
            InputSource::Hdmi1,
            InputSource::Other(0x1B),
        ]
    );
}

#[test]
fn a_feature_the_monitor_did_not_list_is_reported_as_absent() {
    let caps = Capabilities::parse_str(DELL).unwrap();

    assert!(caps.supports(Feature::Brightness));
    // This panel has no speakers, so no volume.
    assert!(!caps.supports(Feature::Volume));
    assert!(caps.entry(Feature::Volume).is_none());
}

#[test]
fn a_monitor_that_lists_input_source_without_values_gets_an_empty_list() {
    // Not the standard table as a fallback: offering DisplayPort 2 to a
    // monitor that has one HDMI port switches nothing and reads as a bug.
    let caps = Capabilities::parse_str("(vcp(10 60))").unwrap();

    assert!(caps.supports(Feature::InputSource));
    assert_eq!(caps.inputs(), []);
}

#[test]
fn a_string_with_no_outer_parentheses_still_parses() {
    let caps = Capabilities::parse_str("prot(monitor)model(VG248)vcp(10 12)").unwrap();

    assert_eq!(caps.model.as_deref(), Some("VG248"));
    assert!(caps.supports(Feature::Contrast));
}

#[test]
fn two_adjacent_groups_are_not_mistaken_for_one_wrapped_group() {
    // `(a)(b)` peeled as a wrapper would become `a)(b`, losing a segment.
    let caps = Capabilities::parse_str("(model(X)(vcp(10)))").unwrap();

    assert_eq!(caps.model.as_deref(), Some("X"));
    assert!(caps.supports(Feature::Brightness));
}

#[test]
fn a_truncated_string_yields_what_arrived_and_says_it_was_truncated() {
    // A capability read that stopped early. The feature list is most of the
    // way there, and throwing it away would lose the monitor entirely.
    let caps = Capabilities::parse_str("(model(U2723QE)vcp(10 12 60(0F 11").unwrap();

    assert_eq!(caps.model.as_deref(), Some("U2723QE"));
    assert!(caps.supports(Feature::Brightness));
    assert_eq!(
        caps.inputs(),
        [InputSource::DisplayPort1, InputSource::Hdmi1]
    );
    assert!(
        caps.warnings.iter().any(|w| w.contains("never closed")),
        "{:?}",
        caps.warnings
    );
}

#[test]
fn a_value_list_separated_from_its_feature_by_a_space_still_attaches_to_it() {
    let caps = Capabilities::parse_str("(vcp(60 (0F 11)))").unwrap();

    assert_eq!(
        caps.inputs(),
        [InputSource::DisplayPort1, InputSource::Hdmi1]
    );
}

#[test]
fn a_value_list_with_no_feature_before_it_is_dropped_with_a_warning() {
    let caps = Capabilities::parse_str("(vcp((0F 11) 10))").unwrap();

    assert_eq!(caps.features.len(), 1);
    assert_eq!(caps.features[0].feature, Feature::Brightness);
    assert!(
        caps.warnings
            .iter()
            .any(|w| w.contains("no feature before")),
        "{:?}",
        caps.warnings
    );
}

#[test]
fn a_token_that_is_not_hex_is_skipped_and_named_in_the_warning() {
    let caps = Capabilities::parse_str("(vcp(10 ZZ 12))").unwrap();

    assert_eq!(caps.features.len(), 2);
    assert!(
        caps.warnings.iter().any(|w| w.contains("ZZ")),
        "{:?}",
        caps.warnings
    );
}

#[test]
fn lowercase_hex_parses_the_same_as_uppercase() {
    let upper = Capabilities::parse_str("(vcp(10 D6(01 04)))").unwrap();
    let lower = Capabilities::parse_str("(vcp(10 d6(01 04)))").unwrap();

    assert_eq!(upper.features, lower.features);
}

#[test]
fn a_bare_key_with_no_parentheses_is_skipped_with_a_warning() {
    let caps = Capabilities::parse_str("(prot(monitor)mswhql vcp(10))").unwrap();

    assert!(caps.supports(Feature::Brightness));
    assert!(
        caps.warnings.iter().any(|w| w.contains("mswhql")),
        "{:?}",
        caps.warnings
    );
}

#[test]
fn a_wrapper_is_peeled_and_two_adjacent_groups_are_not() {
    // Tested against the helper directly rather than through `parse_str`. The
    // segment scanner downstream is forgiving enough to recover from a wrong
    // peel on most inputs, which is good for monitors and bad for tests: it
    // would hide this distinction rather than exercise it.
    assert_eq!(strip_outer_parens("(vcp(10))"), Some("vcp(10)"));
    assert_eq!(strip_outer_parens("(a)(b)"), None);
    assert_eq!(strip_outer_parens("(model(X))(vcp(10))"), None);
    assert_eq!(strip_outer_parens("vcp(10)"), None);
    assert_eq!(strip_outer_parens("(vcp(10)"), None);
}

#[test]
fn a_string_whose_first_parenthesis_closes_early_is_not_peeled() {
    // `(vcp(10)` is missing its final parenthesis. Peeling the outer pair here
    // would hand the scanner `vcp(10`, whose segment never closes — a warning
    // about a monitor that is in fact fine.
    let caps = Capabilities::parse_str("(vcp(10)").unwrap();

    assert!(caps.supports(Feature::Brightness));
    assert_eq!(caps.warnings, Vec::<String>::new());
}

#[test]
fn a_monitor_that_lists_no_input_source_at_all_gets_an_empty_list() {
    // The gap the empty-values test leaves open: there, the feature is present
    // with nothing after it; here it is absent entirely, and a fallback to the
    // standard table would offer inputs this monitor does not have.
    let caps = Capabilities::parse_str("(vcp(10 12))").unwrap();

    assert!(!caps.supports(Feature::InputSource));
    assert_eq!(caps.inputs(), []);
}

#[test]
fn a_bad_hex_token_in_the_command_list_is_named_in_the_warning() {
    // The command list is parsed by a different function than the feature
    // list, so its warning needs its own test or it goes unwatched.
    let caps = Capabilities::parse_str("(cmds(01 ZZ 03)vcp(10))").unwrap();

    assert_eq!(caps.commands, [0x01, 0x03]);
    assert!(
        caps.warnings
            .iter()
            .any(|w| w.contains("ZZ") && w.contains("cmds")),
        "{:?}",
        caps.warnings
    );
}

#[test]
fn nul_padding_inside_a_value_is_trimmed_out_of_it() {
    // Monitors pad short reads with NUL, and `str::trim` leaves it alone: it
    // is not whitespace. Left in, a model name reads as "U2723QE" and speaks
    // as "U2723QE" followed by silence, with nothing on screen to explain it.
    let caps = Capabilities::parse_str("(model(U2723QE\0\0)vcp(10))").unwrap();

    assert_eq!(caps.model.as_deref(), Some("U2723QE"));
}

#[test]
fn a_capability_read_that_returned_only_padding_is_empty_not_unparsable() {
    // "the monitor sent nothing" is a truer thing to say than "the monitor
    // sent something I could not read".
    assert_eq!(
        Capabilities::parse_str("\0\0\0").unwrap_err(),
        CapabilitiesError::Empty
    );
}

#[test]
fn trailing_nul_padding_is_trimmed_rather_than_parsed() {
    // Monitors pad short capability reads with NULs. Left in, they land in the
    // model name and print as blanks after it.
    let caps = Capabilities::parse_str("(model(X)vcp(10))\0\0\0").unwrap();

    assert_eq!(caps.model.as_deref(), Some("X"));
    assert_eq!(caps.warnings, Vec::<String>::new());
}

#[test]
fn an_empty_string_is_an_error_rather_than_an_empty_monitor() {
    assert_eq!(
        Capabilities::parse_str("").unwrap_err(),
        CapabilitiesError::Empty
    );
    assert_eq!(
        Capabilities::parse_str("   ").unwrap_err(),
        CapabilitiesError::Empty
    );
}

#[test]
fn text_with_no_segments_at_all_is_an_error() {
    assert_eq!(
        Capabilities::parse_str("no parentheses here").unwrap_err(),
        CapabilitiesError::NoSegments
    );
}

#[test]
fn bytes_that_are_not_text_are_an_error_rather_than_a_panic() {
    assert_eq!(
        Capabilities::parse(&[0xFF, 0xFE, 0xFD]).unwrap_err(),
        CapabilitiesError::NotText
    );
}

#[test]
fn a_capability_string_arriving_as_bytes_parses_the_same_as_text() {
    assert_eq!(
        Capabilities::parse(DELL.as_bytes()).unwrap(),
        Capabilities::parse_str(DELL).unwrap()
    );
}
