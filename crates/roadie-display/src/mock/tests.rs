use roadie_ddc::packet::Request;
use roadie_ddc::{Feature, Reply, Value};

use super::{Fault, Panel};
use crate::DdcTransport;

/// Ask the panel one question and hand back the raw reply bytes.
fn exchange(panel: &mut Panel, request: Request) -> Vec<u8> {
    panel.send(&request.frame()).expect("the panel accepts");
    let mut buffer = [0_u8; 64];
    let read = panel.receive(&mut buffer).expect("the panel answers");
    buffer[..read].to_vec()
}

#[test]
fn a_feature_reply_is_framed_the_way_the_specification_says() {
    let mut panel = Panel::new("test panel").with_feature(0x10, 50, 100);
    let bytes = exchange(&mut panel, Request::Get(Feature::Brightness));

    // Spelled out rather than computed: source 0x6e, length 0x88 (the high bit
    // plus eight payload bytes), the reply opcode 0x02, result 0x00, the
    // echoed feature 0x10, type 0x00, maximum 100 and current 50 as sixteen-bit
    // values, then the checksum.
    //
    // The checksum is the XOR of every byte before it, seeded with the virtual
    // host address 0x50 and not with either of the two addresses actually on
    // the bus:
    //   0x50 ^ 0x6e ^ 0x88 ^ 0x02 ^ 0x00 ^ 0x10 ^ 0x00 ^ 0x00 ^ 0x64 ^ 0x00 ^ 0x32 = 0xf2
    assert_eq!(
        bytes,
        vec![
            0x6E, 0x88, 0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x32, 0xF2
        ]
    );
}

#[test]
fn roadie_ddc_reads_back_what_the_panel_wrote() {
    let mut panel = Panel::new("test panel").with_feature(0x10, 50, 100);
    let request = Request::Get(Feature::Brightness);
    let bytes = exchange(&mut panel, request);

    let reply = request.parse_reply(&bytes).expect("a well-formed reply");
    assert_eq!(
        reply,
        Reply::Feature {
            feature: Feature::Brightness,
            momentary: false,
            value: Value {
                current: 50,
                maximum: 100,
            },
        }
    );
}

#[test]
fn a_write_changes_what_the_panel_reports() {
    // A `Set` is answered by nothing, so the only evidence it landed is the
    // panel's own state afterwards.
    let mut panel = Panel::new("test panel").with_feature(0x10, 50, 100);
    panel
        .send(
            &Request::Set {
                feature: Feature::Brightness,
                value: 80,
            }
            .frame(),
        )
        .expect("the panel accepts");
    assert_eq!(panel.feature(0x10), Some((80, 100)));
}

#[test]
fn a_missing_feature_answers_unsupported() {
    let mut panel = Panel::new("test panel");
    let request = Request::Get(Feature::Volume);
    let bytes = exchange(&mut panel, request);

    let error = request
        .parse_reply(&bytes)
        .expect_err("a panel without the feature says so");
    assert_eq!(
        error,
        roadie_ddc::packet::ProtocolError::Unsupported { feature: 0x62 }
    );
}

#[test]
fn not_ready_is_a_well_formed_null_message() {
    let mut panel = Panel::new("test panel")
        .with_feature(0x10, 50, 100)
        .with_fault(Fault::NotReady);
    let request = Request::Get(Feature::Brightness);
    let bytes = exchange(&mut panel, request);

    // Source, length with no payload, checksum. A null message has to be
    // *correctly framed* or the layer above would read it as corruption and
    // never learn that waiting is the answer.
    // 0x50 ^ 0x6e ^ 0x80 = 0xbe. The length byte is part of the sum, high bit
    // and all, which is the easiest thing to get wrong by hand.
    assert_eq!(bytes, vec![0x6E, 0x80, 0xBE]);
    assert_eq!(
        request.parse_reply(&bytes),
        Err(roadie_ddc::packet::ProtocolError::Null)
    );
}

#[test]
fn a_stale_reply_is_the_previous_answer() {
    // The hazard the whole protocol layer exists to catch: read the bus too
    // early and you get the answer to the last question, with nothing in the
    // reply itself admitting it.
    let mut panel = Panel::new("test panel")
        .with_feature(0x10, 50, 100)
        .with_feature(0x12, 70, 100);

    let brightness = Request::Get(Feature::Brightness);
    exchange(&mut panel, brightness);

    panel = panel.with_fault(Fault::Stale);
    let contrast = Request::Get(Feature::Contrast);
    let bytes = exchange(&mut panel, contrast);

    // Asked about contrast, answered about brightness. The echoed feature code
    // is the only thing that gives it away.
    assert_eq!(
        contrast.parse_reply(&bytes),
        Err(roadie_ddc::packet::ProtocolError::WrongFeature {
            asked: 0x12,
            answered: 0x10,
        })
    );
}

#[test]
fn a_corrupted_checksum_is_caught() {
    let mut panel = Panel::new("test panel")
        .with_feature(0x10, 50, 100)
        .with_fault(Fault::BadChecksum);
    let request = Request::Get(Feature::Brightness);
    let bytes = exchange(&mut panel, request);

    assert!(matches!(
        request.parse_reply(&bytes),
        Err(roadie_ddc::packet::ProtocolError::Checksum { .. })
    ));
}

#[test]
fn a_silent_panel_returns_nothing() {
    let mut panel = Panel::new("test panel")
        .with_feature(0x10, 50, 100)
        .with_fault(Fault::Silent);
    let bytes = exchange(&mut panel, Request::Get(Feature::Brightness));
    assert_eq!(bytes, [] as [u8; 0]);
}

#[test]
fn capability_fragments_carry_their_offset() {
    // Thirty-three bytes, so the string needs two fragments and the second one
    // has to name where it starts.
    let text = "(prot(monitor)type(lcd)vcp(10 12))";
    let mut panel = Panel::new("test panel").with_capabilities(text);

    let first = Request::Capabilities { offset: 0 };
    let bytes = exchange(&mut panel, first);
    let Ok(Reply::Capabilities { offset, fragment }) = first.parse_reply(&bytes) else {
        panic!("expected a capability fragment");
    };
    assert_eq!(offset, 0);
    assert_eq!(fragment.len(), 32);

    let second = Request::Capabilities { offset: 32 };
    let bytes = exchange(&mut panel, second);
    let Ok(Reply::Capabilities { offset, fragment }) = second.parse_reply(&bytes) else {
        panic!("expected a capability fragment");
    };
    assert_eq!(offset, 32);
    assert_eq!(fragment, b"))");
}

#[test]
fn the_panel_records_what_went_on_the_wire() {
    let mut panel = Panel::new("test panel").with_feature(0x10, 50, 100);
    panel
        .send(&Request::Get(Feature::Brightness).frame())
        .expect("the panel accepts");

    // The documented read-brightness sequence, spelled out: host address,
    // length with two payload bytes, the get opcode, the feature, checksum.
    assert_eq!(panel.seen(), [vec![0x51, 0x82, 0x01, 0x10, 0xAC]]);
}
