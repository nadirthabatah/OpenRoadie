//! Framing a key image into the packets a Stream Deck accepts.
//!
//! Scope is deliberately narrow: this module chops an **already-encoded**
//! image blob into HID output reports. Producing that blob — scaling to the
//! model's [`size_px`](crate::model::KeyScreens::size_px), applying its
//! [`rotation`](crate::model::KeyScreens::rotation), and encoding to its
//! [`format`](crate::model::KeyScreens::format) — needs an imaging library
//! and belongs to a host layer, not to a pure protocol crate.
//!
//! Each model's [`ImageFraming`] picks the packet shape. The original 2015
//! Stream Deck's is refused explicitly rather than emitted wrong; see
//! [`ProtocolError::ImageFramingUnsupported`].

use crate::ProtocolError;
use crate::model::{ImageFraming, Model};

/// Total size of one [`ImageFraming::Gen2`] image output report, report ID and
/// header included.
pub const GEN2_PACKET_LEN: usize = 1024;

/// Size of the [`ImageFraming::Gen2`] image packet header.
const GEN2_HEADER_LEN: usize = 8;

/// Image bytes carried by one [`ImageFraming::Gen2`] packet.
const GEN2_PAYLOAD_LEN: usize = GEN2_PACKET_LEN - GEN2_HEADER_LEN;

/// Largest image the [`ImageFraming::Gen2`] framing can address.
///
/// The page number is a 16-bit wire field, so pages `0..=u16::MAX` are all
/// that can be named. No real key image comes close — a 96×96 JPEG is tens of
/// kilobytes against this ~66 MB ceiling — but the alternative to checking is
/// a counter that saturates and emits a run of packets all claiming the last
/// page, which is a silently corrupt upload rather than a refusal.
const GEN2_MAX_IMAGE_LEN: usize = GEN2_PAYLOAD_LEN * (u16::MAX as usize + 1);

/// Split an encoded key image into the output reports that upload it.
///
/// Every returned packet is exactly [`GEN2_PACKET_LEN`] bytes, zero-padded,
/// with the report ID as byte 0 — the shape a platform HID write expects.
/// The packets must be written in order; the last one carries the flag that
/// tells the device to display what it has received.
///
/// An empty `image` still yields one terminating packet, which is how a key
/// is cleared.
///
/// # Errors
///
/// - [`ProtocolError::ScreenlessModel`] if the model has no key screens.
/// - [`ProtocolError::KeyOutOfRange`] if `key` is not a key on this model.
/// - [`ProtocolError::ImageFramingUnsupported`] for the original Stream Deck.
/// - [`ProtocolError::ImageTooLarge`] if `image` needs more pages than the
///   wire's 16-bit page counter can name.
pub fn key_image_packets(
    model: &Model,
    key: u16,
    image: &[u8],
) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let Some(screens) = model.screens else {
        return Err(ProtocolError::ScreenlessModel { model: model.name });
    };
    if key >= model.key_count() {
        return Err(ProtocolError::KeyOutOfRange {
            index: key,
            count: model.key_count(),
        });
    }
    // Catalogued key counts are far below u8::MAX, and `key` was just bounds
    // checked against one, so the wire field cannot truncate.
    let key_byte = u8::try_from(key).map_err(|_| ProtocolError::KeyOutOfRange {
        index: key,
        count: model.key_count(),
    })?;
    match screens.framing {
        ImageFraming::Gen2 => gen2_packets(key_byte, image),
        ImageFraming::Gen1Mini => mini_packets(key_byte, image),
        ImageFraming::Gen1Original => {
            Err(ProtocolError::ImageFramingUnsupported { model: model.name })
        }
    }
}

/// Total size of one [`ImageFraming::Gen1Mini`] image output report.
pub const MINI_PACKET_LEN: usize = 1024;

/// Size of the [`ImageFraming::Gen1Mini`] image packet header.
const MINI_HEADER_LEN: usize = 16;

/// Image bytes carried by one [`ImageFraming::Gen1Mini`] packet.
const MINI_PAYLOAD_LEN: usize = MINI_PACKET_LEN - MINI_HEADER_LEN;

/// Largest image the Mini's 8-bit page counter can address — about 258 KB,
/// against an 80×80 bitmap's 19,254 bytes.
const MINI_MAX_IMAGE_LEN: usize = MINI_PAYLOAD_LEN * (u8::MAX as usize + 1);

/// The Mini's framing: `02 01 page 00 last key+1`, then ten zeros.
///
/// Two independent drivers agree on every byte, including the two that
/// differ from the original Stream Deck: pages count from 0 here and from 1
/// there, and the key is one-based in both.
fn mini_packets(key: u8, image: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
    let too_large = || ProtocolError::ImageTooLarge {
        bytes: image.len(),
        max: MINI_MAX_IMAGE_LEN,
    };
    if image.len() > MINI_MAX_IMAGE_LEN {
        return Err(too_large());
    }
    let wire_key = key.checked_add(1).ok_or_else(too_large)?;
    let mut packets = Vec::new();
    let mut remaining = image;
    let mut page: u8 = 0;
    loop {
        let take = remaining.len().min(MINI_PAYLOAD_LEN);
        let (chunk, rest) = remaining.split_at(take);
        let last = rest.is_empty();
        let mut packet = Vec::with_capacity(MINI_PACKET_LEN);
        packet.extend_from_slice(&[0x02, 0x01, page, 0x00, u8::from(last), wire_key]);
        packet.resize(MINI_HEADER_LEN, 0);
        packet.extend_from_slice(chunk);
        packet.resize(MINI_PACKET_LEN, 0);
        packets.push(packet);
        if last {
            return Ok(packets);
        }
        remaining = rest;
        page = page.checked_add(1).ok_or_else(too_large)?;
    }
}

/// The gen 2 framing: `02 07 key last length page`, all little-endian.
fn gen2_packets(key_byte: u8, image: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
    if image.len() > GEN2_MAX_IMAGE_LEN {
        return Err(ProtocolError::ImageTooLarge {
            bytes: image.len(),
            max: GEN2_MAX_IMAGE_LEN,
        });
    }

    let mut packets = Vec::new();
    let mut remaining = image;
    let mut page: u16 = 0;
    loop {
        let take = remaining.len().min(GEN2_PAYLOAD_LEN);
        let (chunk, rest) = remaining.split_at(take);
        let last = rest.is_empty();
        // `take` is capped at GEN2_PAYLOAD_LEN, itself far below u16::MAX, so
        // this conversion is total; it is written fallibly rather than as a
        // cast so a future packet size cannot silently truncate it.
        let length = u16::try_from(take).map_err(|_| ProtocolError::ImageTooLarge {
            bytes: image.len(),
            max: GEN2_MAX_IMAGE_LEN,
        })?;

        let mut packet = Vec::with_capacity(GEN2_PACKET_LEN);
        packet.extend_from_slice(&[
            0x02,
            0x07,
            key_byte,
            u8::from(last),
            length.to_le_bytes()[0],
            length.to_le_bytes()[1],
            page.to_le_bytes()[0],
            page.to_le_bytes()[1],
        ]);
        packet.extend_from_slice(chunk);
        packet.resize(GEN2_PACKET_LEN, 0);
        packets.push(packet);

        if last {
            break;
        }
        remaining = rest;
        // Cannot overflow: the length check above bounds the page count to
        // u16::MAX + 1 pages, so the final page is reached before this runs
        // on the last one.
        page = page.checked_add(1).ok_or(ProtocolError::ImageTooLarge {
            bytes: image.len(),
            max: GEN2_MAX_IMAGE_LEN,
        })?;
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::{GEN2_PACKET_LEN, key_image_packets};
    use crate::ProtocolError;
    use crate::model::{ELGATO_VENDOR_ID, Model, identify};

    fn mk2() -> &'static Model {
        identify(ELGATO_VENDOR_ID, 0x0080).expect("the MK.2 is catalogued")
    }

    /// Read a packet's declared payload length back out of its header.
    fn declared_len(packet: &[u8]) -> usize {
        usize::from(u16::from_le_bytes([packet[4], packet[5]]))
    }

    /// Read a packet's page number back out of its header.
    fn page(packet: &[u8]) -> u16 {
        u16::from_le_bytes([packet[6], packet[7]])
    }

    #[test]
    fn a_small_image_fits_one_terminating_packet() {
        let packets = key_image_packets(mk2(), 0, &[0xaa; 100]).expect("a valid upload");
        assert_eq!(packets.len(), 1);
        let packet = &packets[0];
        assert_eq!(packet.len(), GEN2_PACKET_LEN);
        assert_eq!(&packet[..4], &[0x02, 0x07, 0, 1], "key 0, last packet");
        assert_eq!(declared_len(packet), 100);
        assert_eq!(page(packet), 0);
        assert_eq!(&packet[8..108], &[0xaa; 100]);
        assert!(packet[108..].iter().all(|&b| b == 0), "padded with zeroes");
    }

    #[test]
    fn an_empty_image_still_yields_one_packet_so_a_key_can_be_cleared() {
        let packets = key_image_packets(mk2(), 4, &[]).expect("a valid upload");
        assert_eq!(packets.len(), 1);
        assert_eq!(&packets[0][..4], &[0x02, 0x07, 4, 1]);
        assert_eq!(declared_len(&packets[0]), 0);
    }

    #[test]
    fn a_large_image_is_paged_and_only_the_last_packet_is_flagged() {
        // Two full payloads plus a remainder.
        let image = vec![0x5a; 1016 * 2 + 7];
        let packets = key_image_packets(mk2(), 2, &image).expect("a valid upload");
        assert_eq!(packets.len(), 3);
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet.len(), GEN2_PACKET_LEN);
            assert_eq!(packet[2], 2, "every packet names the same key");
            assert_eq!(usize::from(page(packet)), index, "pages count up from 0");
        }
        assert_eq!(packets[0][3], 0);
        assert_eq!(packets[1][3], 0);
        assert_eq!(packets[2][3], 1, "only the final packet is flagged last");
        assert_eq!(declared_len(&packets[0]), 1016);
        assert_eq!(declared_len(&packets[1]), 1016);
        assert_eq!(declared_len(&packets[2]), 7);
    }

    #[test]
    fn an_exactly_full_payload_does_not_emit_a_trailing_empty_packet() {
        let packets = key_image_packets(mk2(), 0, &vec![1; 1016]).expect("a valid upload");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][3], 1);
        assert_eq!(declared_len(&packets[0]), 1016);
    }

    #[test]
    fn the_payload_reassembles_to_the_original_image() {
        let image: Vec<u8> = (0..5000u32)
            .map(|byte| u8::try_from(byte % 251).expect("a remainder below 251 fits a byte"))
            .collect();
        let packets = key_image_packets(mk2(), 1, &image).expect("a valid upload");
        let reassembled: Vec<u8> = packets
            .iter()
            .flat_map(|packet| packet[8..8 + declared_len(packet)].to_vec())
            .collect();
        assert_eq!(reassembled, image);
    }

    #[test]
    fn an_image_past_the_page_counters_reach_is_refused_not_wrapped() {
        // One byte past what the 16-bit page field can address. Allocating the
        // ceiling itself would cost ~66 MB, so this probes the boundary from
        // above with a slice that is never actually paged.
        let oversized = vec![0u8; super::GEN2_MAX_IMAGE_LEN + 1];
        let error = key_image_packets(mk2(), 0, &oversized).expect_err("beyond the page counter");
        assert!(matches!(error, ProtocolError::ImageTooLarge { .. }));
    }

    #[test]
    fn a_key_the_model_does_not_have_is_refused() {
        let error = key_image_packets(mk2(), 15, &[0]).expect_err("the MK.2 has 15 keys, 0..=14");
        assert!(matches!(
            error,
            ProtocolError::KeyOutOfRange {
                index: 15,
                count: 15
            }
        ));
    }

    #[test]
    fn a_screenless_model_is_refused_before_anything_is_framed() {
        let pedal = identify(ELGATO_VENDOR_ID, 0x0086).expect("the Pedal is catalogued");
        let error = key_image_packets(pedal, 0, &[0]).expect_err("the Pedal has no screens");
        assert!(matches!(error, ProtocolError::ScreenlessModel { .. }));
    }

    fn mini() -> &'static Model {
        identify(ELGATO_VENDOR_ID, 0x0063).expect("the Mini is catalogued")
    }

    #[test]
    fn a_mini_packet_header_is_the_bytes_both_reference_drivers_send() {
        // Spelled out rather than computed: `02 01 page 00 last key+1` then
        // ten zeros, as python-elgato-streamdeck and the Rust
        // elgato-streamdeck crate both write it for the Mini.
        let image = vec![0xab; 1500];
        let packets = key_image_packets(mini(), 4, &image).expect("the Mini frames images");
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[0][..16],
            [
                0x02, 0x01, 0x00, 0x00, 0x00, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        assert_eq!(
            packets[1][..16],
            [
                0x02, 0x01, 0x01, 0x00, 0x01, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        for packet in &packets {
            assert_eq!(packet.len(), super::MINI_PACKET_LEN);
        }
        // 1008 bytes ride in the first packet and 492 in the second, the
        // rest of which is padding.
        assert!(packets[0][16..].iter().all(|byte| *byte == 0xab));
        assert!(packets[1][16..16 + 492].iter().all(|byte| *byte == 0xab));
        assert!(packets[1][16 + 492..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn an_80_pixel_mini_bitmap_takes_twenty_packets_with_one_last_flag() {
        // 54 bytes of header plus 80 × 80 × 3: the size every Mini key image is.
        let image = vec![0; 19_254];
        let packets = key_image_packets(mini(), 0, &image).expect("the Mini frames images");
        assert_eq!(packets.len(), 20);
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(usize::from(packet[2]), index, "pages count from zero");
            assert_eq!(packet[4], u8::from(index == 19), "only the last is last");
            assert_eq!(packet[5], 1, "key 0 travels as 1");
        }
    }

    #[test]
    fn the_original_stream_deck_framing_is_refused_rather_than_guessed_at() {
        let original = identify(ELGATO_VENDOR_ID, 0x0060).expect("the original is catalogued");
        let error = key_image_packets(original, 0, &[0]).expect_err("gen 1 framing differs");
        assert!(matches!(
            error,
            ProtocolError::ImageFramingUnsupported { .. }
        ));
    }

    /// Every invariant the wire depends on, across the sizes where framing
    /// goes wrong.
    ///
    /// These bytes are what a Stream Deck actually receives, and this project
    /// cannot look at one to see whether the picture arrived. Exact multiples
    /// of the payload and the sizes either side of them are where a framing
    /// bug lives — an extra empty packet, a page counter off by one, a last
    /// flag on the wrong packet — and every one of those shows up on hardware
    /// as an image that never appears, with nothing on this side to say why.
    #[test]
    fn every_framing_invariant_holds_at_and_around_every_page_boundary() {
        const PAYLOAD: usize = GEN2_PACKET_LEN - 8;
        let mut sizes: Vec<usize> = vec![0, 1, 2, PAYLOAD - 1, PAYLOAD, PAYLOAD + 1];
        for pages in 2..=5 {
            sizes.push(PAYLOAD * pages - 1);
            sizes.push(PAYLOAD * pages);
            sizes.push(PAYLOAD * pages + 1);
        }

        for size in sizes {
            let image: Vec<u8> = (0..size)
                .map(|index| u8::try_from(index % 251).expect("below 251 fits a byte"))
                .collect();
            let packets = key_image_packets(mk2(), 1, &image)
                .unwrap_or_else(|error| panic!("{size} bytes should frame: {error}"));

            assert!(!packets.is_empty(), "{size}: no packets at all");

            // Every packet is exactly one wire packet, padded. A short one is
            // a packet the device would read past the end of.
            for (at, packet) in packets.iter().enumerate() {
                assert_eq!(
                    packet.len(),
                    GEN2_PACKET_LEN,
                    "{size}: packet {at} is {} bytes",
                    packet.len()
                );
            }

            // Pages run 0, 1, 2, ... with no gaps and no repeats.
            for (at, packet) in packets.iter().enumerate() {
                assert_eq!(
                    usize::from(page(packet)),
                    at,
                    "{size}: packet {at} claims page {}",
                    page(packet)
                );
            }

            // Exactly one packet is flagged last, and it is the final one.
            let flagged: Vec<usize> = packets
                .iter()
                .enumerate()
                .filter(|(_, packet)| packet[3] == 1)
                .map(|(at, _)| at)
                .collect();
            assert_eq!(
                flagged,
                vec![packets.len() - 1],
                "{size}: the last-packet flag is on {flagged:?} of {} packets",
                packets.len()
            );

            // The declared lengths account for the image exactly — no byte
            // dropped, none invented.
            let declared: usize = packets.iter().map(|packet| declared_len(packet)).sum();
            assert_eq!(declared, size, "{size}: declared lengths sum to {declared}");

            // And the payload reassembles to the original, byte for byte.
            let reassembled: Vec<u8> = packets
                .iter()
                .flat_map(|packet| packet[8..8 + declared_len(packet)].to_vec())
                .collect();
            assert_eq!(reassembled, image, "{size}: reassembly differs");

            // No packet may claim more payload than one holds, and every
            // packet but the last must be completely full.
            //
            // The fullness half is not pedantry. Underfilling still delivers
            // the image — the device reads the declared length — so nothing
            // would look wrong; it would simply take more USB transfers per
            // key, on every key, for the life of the driver. That is the kind
            // of regression that is invisible in a test suite whose
            // invariants are only self-consistent, which is exactly what this
            // one was until a mutation walked through it.
            let last = packets.len() - 1;
            for (at, packet) in packets.iter().enumerate() {
                assert!(
                    declared_len(packet) <= PAYLOAD,
                    "{size}: packet {at} claims {} bytes of a {PAYLOAD}-byte payload",
                    declared_len(packet)
                );
                if at < last {
                    assert_eq!(
                        declared_len(packet),
                        PAYLOAD,
                        "{size}: packet {at} of {} is not full, so the image takes \
                         more transfers than it needs",
                        packets.len()
                    );
                }
            }
        }
    }
}
