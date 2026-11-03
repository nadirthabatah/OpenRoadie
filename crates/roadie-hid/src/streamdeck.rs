//! Elgato Stream Decks over this host's HID stack.
//!
//! The counterpart of [`roadie_streamdeck`], which holds the protocol and
//! knows no host. This module is the host: enumeration, opening the right
//! collection, and the report traffic. It mirrors how `roadie-device` and
//! this crate split the HID++ side, so a second implementation of the
//! protocol layer stays possible and the protocol stays testable without a
//! device.
//!
//! # What is verified, and what is not
//!
//! Worth being precise, because "untested driver" is too blunt a description.
//!
//! **Tested:** the encodings, in `roadie-streamdeck`; and this module's own
//! behaviour — what it sends for a brightness or reset, how it decodes and
//! diffs key reports, that a foreign report id is skipped rather than fatal,
//! that a truncated one is an error rather than a row of phantom releases,
//! that a missed report still resolves both transitions, and that a vanished
//! device surfaces instead of hanging. Those run against a scripted device
//! (see the tests below), which is what the [`DeckTransport`] seam exists for.
//!
//! **Not tested:** the OS calls themselves, and every assumption about what a
//! real device does in response. Three things need hardware, and `roadie
//! streamdeck verify` exists to settle them: which HID collection carries the
//! key and image traffic (see [`Attached::is_preferred_collection`]), whether
//! the original Stream Deck mirrors its keys within a row, and the gen 1
//! report layouts.

use async_hid::{
    AsyncHidFeatureHandle as _, AsyncHidRead as _, AsyncHidWrite as _, Device, DeviceFeatureHandle,
    DeviceReader, DeviceWriter,
};
use hidpp::async_trait;
use roadie_device::backend::BackendError;
use roadie_streamdeck::image::key_image_packets;
use roadie_streamdeck::model::{ELGATO_VENDOR_ID, Model, identify};
use roadie_streamdeck::report::{self, Brightness, KeyEvent, KeyStates};

use crate::transport::enumerate_devices;

/// Largest input report this module will read.
///
/// The biggest key report in the catalog is the XL's four-byte header plus 32
/// keys; the surplus is headroom for a model that pads or reports more, since
/// a short read is decoded from its filled prefix anyway.
const INPUT_BUFFER: usize = 512;

/// A Stream Deck collection the OS is reporting.
///
/// One physical Stream Deck usually appears as several of these — a HID device
/// exposes one node per top-level collection — so this is "a way in", not "a
/// device". [`preferred`] picks between them.
pub struct Attached {
    /// Which Stream Deck this is.
    pub model: &'static Model,
    /// The OS-reported product name.
    pub name: String,
    /// Serial number, when the OS reports one.
    pub serial_number: Option<String>,
    /// HID usage page of this collection.
    pub usage_page: u16,
    /// HID usage id of this collection.
    pub usage_id: u16,
    device: Device,
}

impl Attached {
    /// Whether this collection is the one [`preferred`] would choose.
    ///
    /// Vendor-specific traffic conventionally lives on a vendor-defined usage
    /// page (`0xFF00` and above), and the Stream Deck's key and image reports
    /// are vendor-specific. That is the rule applied here.
    ///
    /// It is a convention rather than a fact read off the device, which is
    /// exactly why `roadie streamdeck verify` prints every collection it
    /// found and which one was chosen: if the choice is wrong on some model,
    /// that output says so immediately instead of leaving a silent
    /// misbehaviour to diagnose.
    #[must_use]
    pub fn is_preferred_collection(&self) -> bool {
        self.usage_page >= 0xff00
    }
}

impl std::fmt::Debug for Attached {
    /// Hand-written because the open handle is not `Debug` and printing it
    /// would say nothing useful anyway; the usage pair is rendered in hex
    /// because that is how every HID reference writes it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attached")
            .field("model", &self.model.name)
            .field("name", &self.name)
            .field("serial_number", &self.serial_number)
            .field("usage_page", &format_args!("{:#06x}", self.usage_page))
            .field("usage_id", &format_args!("{:#06x}", self.usage_id))
            .finish_non_exhaustive()
    }
}

/// Every Stream Deck collection this host currently reports.
///
/// Returns all of them, including collections [`preferred`] would not choose, so a
/// caller diagnosing a device can see what the OS actually offers.
///
/// # Errors
///
/// Fails if the host's HID stack cannot be enumerated.
pub async fn attached() -> Result<Vec<Attached>, BackendError> {
    let devices = enumerate_devices().await?;
    Ok(devices
        .into_iter()
        .filter_map(|device| {
            let model = identify(device.vendor_id, device.product_id)?;
            Some(Attached {
                model,
                name: device.name.clone(),
                serial_number: device.serial_number.clone(),
                usage_page: device.usage_page,
                usage_id: device.usage_id,
                device,
            })
        })
        .collect())
}

/// How a physical device is told apart from another of the same model.
///
/// The serial number where the OS reports one; the product id otherwise, in
/// which case two identical serial-less decks collapse to one entry. That is
/// the honest outcome for hardware the host gives no way to distinguish, and
/// it is why `verify` prints serials.
fn identity(attached: &Attached) -> String {
    attached
        .serial_number
        .clone()
        .unwrap_or_else(|| format!("pid:{:04x}", attached.model.product_id))
}

/// Choose one collection per physical device, preferring the vendor page.
///
/// Borrows rather than clones: an open handle is not duplicable, and a
/// caller wants to open the collection this picked, not a copy of its
/// description.
#[must_use]
pub fn preferred(collections: &[Attached]) -> Vec<&Attached> {
    let described: Vec<(String, bool)> = collections
        .iter()
        .map(|attached| (identity(attached), attached.is_preferred_collection()))
        .collect();
    choose(&described)
        .into_iter()
        .filter_map(|index| collections.get(index))
        .collect()
}

/// Which of a set of collections to keep, by index.
///
/// The selection rule on its own, over `(identity, sits on a vendor page)`
/// pairs rather than over open device handles — those cannot be constructed
/// in a test, and this rule is one of the two things about Stream Deck
/// support that most needs to be right (see the module docs). Keeping it
/// separable is what lets it be checked at all.
///
/// The first collection seen for an identity is kept, and later replaced only
/// by a vendor-page one. Order is otherwise preserved, so the result follows
/// enumeration order rather than an accident of the grouping.
fn choose(collections: &[(String, bool)]) -> Vec<usize> {
    let mut chosen: Vec<usize> = Vec::new();
    for (index, (identity, vendor_page)) in collections.iter().enumerate() {
        match chosen
            .iter_mut()
            .find(|held| collections[**held].0 == *identity)
        {
            Some(held) if !collections[*held].1 && *vendor_page => *held = index,
            Some(_) => {}
            None => chosen.push(index),
        }
    }
    chosen
}

/// The two operations a Stream Deck session performs on a transport.
///
/// A trait rather than the concrete `async-hid` handles so the session logic
/// above it — report encoding, decoding, and the key-state diff — is exercised
/// by tests against a scripted device. Without this seam that logic could only
/// ever be checked by plugging hardware in, which is the whole difficulty with
/// a driver.
#[async_trait]
pub trait DeckTransport: Send {
    /// Send one feature report, report id included as byte 0.
    async fn write_feature_report(&mut self, report: &[u8]) -> Result<(), BackendError>;

    /// Send one output report, report id included as byte 0.
    async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError>;

    /// Wait for the next input report, returning how many bytes it filled.
    async fn read_input_report(&mut self, buffer: &mut [u8]) -> Result<usize, BackendError>;
}

/// The transport this host actually uses.
struct HostTransport {
    feature: DeviceFeatureHandle,
    reader: DeviceReader,
    writer: DeviceWriter,
}

#[async_trait]
impl DeckTransport for HostTransport {
    async fn write_feature_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
        self.feature
            .write_feature_report(report)
            .await
            .map_err(crate::transport::backend_error)
    }

    async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
        self.writer
            .write_output_report(report)
            .await
            .map_err(crate::transport::backend_error)
    }

    async fn read_input_report(&mut self, buffer: &mut [u8]) -> Result<usize, BackendError> {
        self.reader
            .read_input_report(buffer)
            .await
            .map_err(crate::transport::backend_error)
    }
}

/// An open Stream Deck: a transport, and the key state to diff against.
pub struct Session {
    model: &'static Model,
    transport: Box<dyn DeckTransport>,
    states: KeyStates,
}

impl Session {
    /// Open `attached` for control and key events.
    ///
    /// # Errors
    ///
    /// Fails if either handle cannot be opened — on Linux typically a
    /// permissions problem on the `hidraw` node, which is what the udev rules
    /// exist to fix.
    pub async fn open(attached: &Attached) -> Result<Self, BackendError> {
        let feature = attached
            .device
            .open_feature_handle()
            .await
            .map_err(crate::transport::backend_error)?;
        let (reader, writer) = attached
            .device
            .open()
            .await
            .map_err(crate::transport::backend_error)?;
        Ok(Self::with_transport(
            attached.model,
            Box::new(HostTransport {
                feature,
                reader,
                writer,
            }),
        ))
    }

    /// Build a session over any transport.
    ///
    /// Public so a caller can drive a Stream Deck over something other than
    /// this host's HID stack — and so the tests below can drive one over a
    /// scripted device.
    #[must_use]
    pub fn with_transport(model: &'static Model, transport: Box<dyn DeckTransport>) -> Self {
        Self {
            model,
            transport,
            states: KeyStates::released(model),
        }
    }

    /// Which Stream Deck this session drives.
    #[must_use]
    pub fn model(&self) -> &'static Model {
        self.model
    }

    /// Set the key screens' brightness.
    ///
    /// # Errors
    ///
    /// Fails if the feature report cannot be written.
    pub async fn set_brightness(&mut self, brightness: Brightness) -> Result<(), BackendError> {
        let report = report::set_brightness(self.model, brightness);
        self.transport.write_feature_report(report.as_bytes()).await
    }

    /// Reset the device to its stock standby screen.
    ///
    /// # Errors
    ///
    /// Fails if the feature report cannot be written.
    pub async fn reset(&mut self) -> Result<(), BackendError> {
        let report = report::reset(self.model);
        self.transport.write_feature_report(report.as_bytes()).await
    }

    /// Show an encoded image on one key.
    ///
    /// `encoded` must already be in the format the model's screens accept —
    /// `roadie_streamdeck::render::key_image` produces it. The packets are
    /// written in order, and the device displays what it has received when it
    /// sees the last one.
    ///
    /// # Errors
    ///
    /// Fails if the model has no key screens, if `key` is not one of its
    /// keys, if this generation's framing is not implemented, or if a write
    /// fails partway — in which case the key is left showing whatever it had,
    /// since the device only commits on the final packet.
    pub async fn set_key_image(&mut self, key: u16, encoded: &[u8]) -> Result<(), BackendError> {
        let packets = key_image_packets(self.model, key, encoded)
            .map_err(|error| BackendError::Backend(error.to_string()))?;
        for packet in &packets {
            self.transport.write_output_report(packet).await?;
        }
        Ok(())
    }

    /// Wait for the next input report and return the key transitions in it.
    ///
    /// An empty result is normal: the device re-reports its whole keyboard, so
    /// a report in which nothing changed carries no transitions. A report this
    /// model does not use for key state is skipped rather than treated as an
    /// error, since a device may interleave other reports on the same
    /// collection.
    ///
    /// # Errors
    ///
    /// Fails if the read fails, or if a key-state report is malformed — a
    /// short report is an error rather than a row of invented key releases.
    pub async fn next_events(&mut self) -> Result<Vec<KeyEvent>, BackendError> {
        let mut buffer = [0u8; INPUT_BUFFER];
        let read = self.transport.read_input_report(&mut buffer).await?;
        match report::decode_key_states(self.model, &buffer[..read]) {
            Ok(states) => {
                let events = states.changes_since(&self.states);
                self.states = states;
                Ok(events)
            }
            // Another report kind on the same collection is not this layer's
            // business; only a malformed *key* report is.
            Err(roadie_streamdeck::ProtocolError::UnexpectedReport { .. }) => Ok(Vec::new()),
            Err(error) => Err(BackendError::Backend(error.to_string())),
        }
    }
}

/// An Elgato device this build does not recognize.
///
/// Reported separately rather than dropped: "my Stream Deck is not detected"
/// and "my Stream Deck is a model this build has never heard of" look
/// identical to a user, and only this distinguishes them. A product id here is
/// the one fact needed to add the model to the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unrecognized {
    /// USB product id, the value a catalogue entry is keyed on.
    pub product_id: u16,
    /// The OS-reported product name.
    pub name: String,
    /// HID usage page of this collection.
    pub usage_page: u16,
    /// HID usage id of this collection.
    pub usage_id: u16,
}

/// Every Elgato device this host reports that is *not* in the catalogue.
///
/// # Errors
///
/// Fails if the host's HID stack cannot be enumerated.
pub async fn unrecognized() -> Result<Vec<Unrecognized>, BackendError> {
    let devices = enumerate_devices().await?;
    let mut found: Vec<Unrecognized> = devices
        .into_iter()
        .filter(|device| is_elgato(device.vendor_id))
        .filter(|device| identify(device.vendor_id, device.product_id).is_none())
        .map(|device| Unrecognized {
            product_id: device.product_id,
            name: device.name.clone(),
            usage_page: device.usage_page,
            usage_id: device.usage_id,
        })
        .collect();
    found.sort_by_key(|device| (device.product_id, device.usage_page, device.usage_id));
    found.dedup();
    Ok(found)
}

/// Whether a vendor id could be a Stream Deck at all.
///
/// Exposed for callers filtering a device list before doing the fuller
/// [`identify`] lookup.
#[must_use]
pub fn is_elgato(vendor_id: u16) -> bool {
    vendor_id == ELGATO_VENDOR_ID
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use hidpp::async_trait;
    use roadie_device::backend::BackendError;
    use roadie_streamdeck::model::{ELGATO_VENDOR_ID, Model, identify};
    use roadie_streamdeck::report::{Brightness, KeyAction};

    use super::{DeckTransport, Session, is_elgato};

    /// A Stream Deck that exists only in this test: it hands out the input
    /// reports it was scripted with and records every feature report written
    /// to it.
    ///
    /// This is what lets the session's own behaviour — the encoding it sends,
    /// the decoding and diffing it does — be checked without a device.
    #[derive(Default)]
    struct Scripted {
        /// Input reports still to be delivered, in order.
        inputs: VecDeque<Vec<u8>>,
        /// Every feature report written, in order.
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        /// Every output report written, in order.
        outputs: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    /// What a scripted device recorded: feature reports, then output reports.
    type Recorded = (Arc<Mutex<Vec<Vec<u8>>>>, Arc<Mutex<Vec<Vec<u8>>>>);

    impl Scripted {
        fn new(inputs: Vec<Vec<u8>>) -> (Self, Recorded) {
            let written = Arc::new(Mutex::new(Vec::new()));
            let outputs = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    inputs: inputs.into(),
                    written: Arc::clone(&written),
                    outputs: Arc::clone(&outputs),
                },
                (written, outputs),
            )
        }
    }

    #[async_trait]
    impl DeckTransport for Scripted {
        async fn write_feature_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
            self.written
                .lock()
                .expect("the test holds this lock alone")
                .push(report.to_vec());
            Ok(())
        }

        async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
            self.outputs
                .lock()
                .expect("the test holds this lock alone")
                .push(report.to_vec());
            Ok(())
        }

        async fn read_input_report(&mut self, buffer: &mut [u8]) -> Result<usize, BackendError> {
            let Some(report) = self.inputs.pop_front() else {
                // Scripted input exhausted: behave like a device that went
                // away rather than blocking the test for ever.
                return Err(BackendError::Disconnected);
            };
            buffer[..report.len()].copy_from_slice(&report);
            Ok(report.len())
        }
    }

    fn mk2() -> &'static Model {
        identify(ELGATO_VENDOR_ID, 0x0080).expect("the MK.2 is catalogued")
    }

    /// A gen 2 key report with the listed reported positions held down.
    fn gen2_report(pressed: &[usize]) -> Vec<u8> {
        let mut report = vec![0u8; 4 + 15];
        report[0] = 0x01;
        for &index in pressed {
            report[4 + index] = 1;
        }
        report
    }

    /// Shorthand for a collection description: identity, and whether it sits
    /// on a vendor-defined usage page.
    fn described(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
        pairs
            .iter()
            .map(|(identity, vendor)| ((*identity).to_string(), *vendor))
            .collect()
    }

    #[test]
    fn one_collection_is_kept_as_is() {
        assert_eq!(super::choose(&described(&[("deck-a", true)])), vec![0]);
        assert_eq!(super::choose(&described(&[("deck-a", false)])), vec![0]);
    }

    #[test]
    fn the_vendor_page_collection_wins_whichever_order_it_arrives_in() {
        assert_eq!(
            super::choose(&described(&[("deck-a", false), ("deck-a", true)])),
            vec![1],
            "a vendor-page collection replaces a generic one seen first"
        );
        assert_eq!(
            super::choose(&described(&[("deck-a", true), ("deck-a", false)])),
            vec![0],
            "and a generic one never displaces a vendor-page collection"
        );
    }

    #[test]
    fn two_physical_devices_both_survive() {
        let chosen = super::choose(&described(&[
            ("deck-a", false),
            ("deck-b", false),
            ("deck-a", true),
            ("deck-b", true),
        ]));
        assert_eq!(chosen, vec![2, 3], "one vendor-page entry per device");
    }

    #[test]
    fn several_generic_collections_for_one_device_collapse_to_the_first() {
        assert_eq!(
            super::choose(&described(&[
                ("deck-a", false),
                ("deck-a", false),
                ("deck-a", false),
            ])),
            vec![0]
        );
    }

    #[test]
    fn nothing_attached_chooses_nothing() {
        assert_eq!(super::choose(&[]), [] as [usize; 0]);
    }

    #[test]
    fn only_elgato_is_a_candidate() {
        assert!(is_elgato(ELGATO_VENDOR_ID));
        assert!(!is_elgato(0x046d));
    }

    #[tokio::test]
    async fn brightness_reaches_the_device_as_the_protocol_encodes_it() {
        let (scripted, (written, _)) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        session
            .set_brightness(Brightness::DIM)
            .await
            .expect("the scripted device accepts writes");

        let sent = written.lock().expect("uncontended");
        assert_eq!(sent.len(), 1);
        assert_eq!(&sent[0][..3], &[0x03, 0x08, Brightness::DIM.percent()]);
        assert_eq!(sent[0].len(), 32, "gen 2 feature reports are padded to 32");
    }

    #[tokio::test]
    async fn reset_reaches_the_device_as_the_protocol_encodes_it() {
        let (scripted, (written, _)) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        session.reset().await.expect("accepted");

        let sent = written.lock().expect("uncontended");
        assert_eq!(&sent[0][..2], &[0x03, 0x02]);
    }

    #[tokio::test]
    async fn a_key_image_is_written_as_ordered_packets_ending_in_a_last_one() {
        let (scripted, (_, outputs)) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        // Large enough to need several packets, so ordering and the last-packet
        // flag are both exercised rather than collapsing into one write.
        let encoded = vec![0x5au8; 1016 * 2 + 5];
        session
            .set_key_image(3, &encoded)
            .await
            .expect("the scripted device accepts writes");

        let sent = outputs.lock().expect("uncontended");
        assert_eq!(sent.len(), 3, "three packets for this payload");
        for (index, packet) in sent.iter().enumerate() {
            assert_eq!(packet.len(), 1024, "every packet is a full report");
            assert_eq!(packet[2], 3, "each names the key it is for");
            assert_eq!(
                u16::from_le_bytes([packet[6], packet[7]]),
                u16::try_from(index).expect("small"),
                "pages arrive in order"
            );
        }
        assert_eq!(sent[0][3], 0);
        assert_eq!(sent[1][3], 0);
        assert_eq!(sent[2][3], 1, "only the last packet commits the image");
    }

    /// The whole image path in one test: render a picture, push it through a
    /// session, then reassemble the payload the device would have received and
    /// decode it back.
    ///
    /// Each stage is covered on its own elsewhere. This checks they compose —
    /// that what `render` produces is what the framer accepts, that the framer's
    /// packets survive the transport in order, and that a device reassembling
    /// them by the rules in their headers gets the original bytes back. A
    /// mismatch anywhere in that chain shows up here and nowhere else.
    #[tokio::test]
    async fn a_rendered_image_survives_the_whole_path_to_the_device() {
        use roadie_streamdeck::render;

        let model = mk2();
        let picture = render::solid(model, 0x20, 0x90, 0xd0).expect("has screens");
        let encoded = render::key_image(model, &picture).expect("encodes");

        let (scripted, (_, outputs)) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(model, Box::new(scripted));
        session.set_key_image(5, &encoded).await.expect("accepted");

        // Reassemble the way the device would: each packet declares its own
        // payload length at bytes 4..6, after an eight-byte header.
        let packets = outputs.lock().expect("uncontended");
        let mut reassembled = Vec::new();
        for packet in packets.iter() {
            let length = usize::from(u16::from_le_bytes([packet[4], packet[5]]));
            reassembled.extend_from_slice(&packet[8..8 + length]);
        }
        assert_eq!(
            reassembled, encoded,
            "the bytes the device receives must be the bytes render produced"
        );

        let decoded = image::load_from_memory(&reassembled).expect("still a valid image");
        assert_eq!(
            image::GenericImageView::dimensions(&decoded),
            (72, 72),
            "and still the key's size"
        );
    }

    #[tokio::test]
    async fn an_image_for_a_key_the_model_lacks_writes_nothing_at_all() {
        let (scripted, (_, outputs)) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        session
            .set_key_image(15, &[0u8; 32])
            .await
            .expect_err("the MK.2 has keys 0..=14");

        assert!(
            outputs.lock().expect("uncontended").is_empty(),
            "a refused image must not half-write to the device"
        );
    }

    #[tokio::test]
    async fn a_press_and_release_arrive_as_two_events_across_two_reports() {
        let (scripted, ..) = Scripted::new(vec![gen2_report(&[2]), gen2_report(&[])]);
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        let pressed = session.next_events().await.expect("a scripted report");
        assert_eq!(pressed.len(), 1);
        assert_eq!(pressed[0].key, 2);
        assert_eq!(pressed[0].action, KeyAction::Pressed);

        let released = session.next_events().await.expect("a scripted report");
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].key, 2);
        assert_eq!(released[0].action, KeyAction::Released);
    }

    #[tokio::test]
    async fn holding_a_key_reports_nothing_further() {
        let (scripted, ..) = Scripted::new(vec![
            gen2_report(&[0]),
            gen2_report(&[0]),
            gen2_report(&[0]),
        ]);
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        assert_eq!(session.next_events().await.expect("report").len(), 1);
        assert_eq!(
            session.next_events().await.expect("report"),
            [] as [roadie_streamdeck::report::KeyEvent; 0]
        );
        assert_eq!(
            session.next_events().await.expect("report"),
            [] as [roadie_streamdeck::report::KeyEvent; 0]
        );
    }

    #[tokio::test]
    async fn a_report_this_model_does_not_use_for_keys_is_skipped_not_fatal() {
        // A device may interleave other report ids on the same collection.
        // Treating one as an error would end a watch session for a report
        // that simply is not ours.
        let mut foreign = gen2_report(&[]);
        foreign[0] = 0x05;
        let (scripted, ..) = Scripted::new(vec![foreign, gen2_report(&[7])]);
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        assert!(
            session.next_events().await.expect("skipped").is_empty(),
            "a foreign report yields no events"
        );
        let events = session.next_events().await.expect("a key report");
        assert_eq!(events[0].key, 7);
    }

    #[tokio::test]
    async fn a_truncated_key_report_is_an_error_not_a_row_of_phantom_releases() {
        let mut truncated = gen2_report(&[]);
        truncated.truncate(9);
        let (scripted, ..) = Scripted::new(vec![truncated]);
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        session
            .next_events()
            .await
            .expect_err("a short key report must not decode");
    }

    #[tokio::test]
    async fn a_missed_report_still_resolves_both_transitions() {
        // The report where key 1 came up was never delivered; the next one
        // shows 5 down instead. Diffing whole snapshots recovers both.
        let (scripted, ..) = Scripted::new(vec![gen2_report(&[1]), gen2_report(&[5])]);
        let mut session = Session::with_transport(mk2(), Box::new(scripted));

        assert_eq!(session.next_events().await.expect("report").len(), 1);
        let events = session.next_events().await.expect("report");
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].key, events[0].action), (1, KeyAction::Released));
        assert_eq!((events[1].key, events[1].action), (5, KeyAction::Pressed));
    }

    #[tokio::test]
    async fn a_vanished_device_surfaces_rather_than_hanging() {
        let (scripted, ..) = Scripted::new(Vec::new());
        let mut session = Session::with_transport(mk2(), Box::new(scripted));
        session.next_events().await.expect_err("the device is gone");
    }
}
