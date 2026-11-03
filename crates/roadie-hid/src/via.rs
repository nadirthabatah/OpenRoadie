//! VIA/QMK keyboards and macro pads over this host's HID stack.
//!
//! The counterpart of [`roadie_via`], which holds the protocol and knows no
//! host. One implementation reaches the hundreds of macro pads and keyboards
//! running QMK with VIA enabled, which is why this is worth more per line than
//! any single-vendor driver: the alternative is a driver per board.
//!
//! # What is verified, and what is not
//!
//! **Tested:** the framing and parsing, in `roadie-via`; and this module's
//! own behaviour — that a stray report is skipped rather than read as an
//! answer, that a device stuck sending stray reports gives up rather than
//! hanging, that an unknown protocol revision is refused before anything is
//! written, and that a write is confirmed by reading back rather than assumed.
//! Those run against a scripted device through [`ViaTransport`].
//!
//! **Not tested:** the OS calls, and every assumption about how a real board
//! answers. In particular, whether the vendor collection this matches on is
//! the one that carries VIA traffic on any given board.
//!
//! # Why writes are careful here
//!
//! A wrong keycode written to a keyboard takes a key away from whoever is
//! using it, and the tool that did it is then the tool they have to use to fix
//! it. So: the protocol revision is checked before any write, the write is
//! read back and compared, and a mismatch is reported rather than swallowed.

use std::time::Duration;

use async_hid::{AsyncHidRead as _, AsyncHidWrite as _, Device, DeviceReader, DeviceWriter};
use hidpp::async_trait;
use roadie_device::backend::BackendError;
use roadie_via::command::{Command, Response, check_protocol};
use roadie_via::identity::{REPORT_LEN, is_via_collection};

use crate::transport::enumerate_devices;

/// How many unrelated reports to skip before giving up on an answer.
///
/// A QMK board can send input reports at any time, so the answer to a question
/// is not necessarily the next report to arrive. Skipping them is right;
/// skipping them forever is not — a board that never answers would hang the
/// caller with no explanation, which is worse than an error.
const MAX_STRAY_REPORTS: usize = 8;

/// How long to wait for any single answer.
///
/// The stray-report limit only helps when reports keep arriving. A board that
/// goes silent — wrong collection, firmware without VIA, a device that is not
/// a keyboard at all — would otherwise leave the caller waiting forever with
/// nothing on screen, which for someone working by ear is indistinguishable
/// from the program having crashed.
///
/// Generous rather than tight: QMK answers in milliseconds, and the cost of
/// being wrong in the other direction is a spurious failure on a slow board.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// The largest matrix edge worth scanning when reading a keymap blind.
///
/// VIA gives no way to ask a board how big its matrix is, so reading a keymap
/// means reading positions until you decide to stop, one round trip each. The
/// caller picks how far; this is where "as far as you like" stops.
///
/// 32 is roughly twice the largest edge any real QMK board has, so it costs
/// nothing anyone would notice and bounds the worst case at about a thousand
/// round trips instead of the sixty-five thousand a byte allows. The reason to
/// bound it at all is the assistant: a person who types `--rows 255` waits and
/// then presses ctrl-C, while a model that asks for it — invited to, by a
/// description that says to raise the number if a key is missing — leaves its
/// client waiting on a call with no way to cancel.
pub const MAX_SCAN_EDGE: u8 = 32;

/// How far to actually scan, given what was asked for.
///
/// Returns the edge to use and whether it was cut down, because a scan that
/// quietly stopped short is indistinguishable from a keyboard with nothing
/// there — and "nothing there" is the wrong conclusion to hand anyone.
#[must_use]
pub fn scan_edge(asked: u8) -> (u8, bool) {
    if asked > MAX_SCAN_EDGE {
        (MAX_SCAN_EDGE, true)
    } else {
        (asked, false)
    }
}

/// A VIA-capable HID collection the OS is reporting.
///
/// A candidate rather than a confirmed VIA device: the vendor usage page this
/// matches on is shared with other firmware, so only a successful protocol
/// exchange proves one. [`Session::open`] performs that exchange.
pub struct Attached {
    /// The OS-reported product name.
    pub name: String,
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// Serial number, when the OS reports one.
    pub serial_number: Option<String>,
    device: Device,
}

impl std::fmt::Debug for Attached {
    /// Hand-written because the device handle is not `Debug`, and the ids are
    /// rendered in hex because that is how every USB reference writes them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attached")
            .field("name", &self.name)
            .field("vendor_id", &format_args!("{:04x}", self.vendor_id))
            .field("product_id", &format_args!("{:04x}", self.product_id))
            .field("serial_number", &self.serial_number)
            .finish_non_exhaustive()
    }
}

/// Every collection that could be a VIA endpoint.
///
/// # Errors
///
/// Fails if the host's HID stack cannot be enumerated.
pub async fn attached() -> Result<Vec<Attached>, BackendError> {
    Ok(enumerate_devices()
        .await?
        .into_iter()
        .filter(|device| is_via_collection(device.usage_page, device.usage_id))
        .map(|device| Attached {
            name: device.name.clone(),
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            serial_number: device.serial_number.clone(),
            device,
        })
        .collect())
}

/// The two operations a VIA session performs on a transport.
///
/// A trait rather than the concrete `async-hid` handles, so the exchange logic
/// above it — skipping stray reports, checking the echoed command, confirming
/// a write — is exercised against a scripted device. Without this seam none of
/// that could be checked without a keyboard to risk.
#[async_trait]
pub trait ViaTransport: Send {
    /// Send one output report.
    async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError>;

    /// Wait for the next input report, returning how many bytes it filled.
    async fn read_input_report(&mut self, buffer: &mut [u8]) -> Result<usize, BackendError>;
}

/// The transport this host actually uses.
struct HostTransport {
    reader: DeviceReader,
    writer: DeviceWriter,
}

#[async_trait]
impl ViaTransport for HostTransport {
    async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
        // Windows reserves byte zero for a report ID, even for QMK's
        // unnumbered raw reports. async-hid removes it on reads but does not
        // insert it on writes; without it Windows consumes the VIA command.
        #[cfg(target_os = "windows")]
        let framed = windows_output_report(report);
        #[cfg(target_os = "windows")]
        let report = framed.as_slice();
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

/// Frame an unnumbered VIA payload for Windows' HID output buffer.
#[cfg(target_os = "windows")]
fn windows_output_report(payload: &[u8]) -> Vec<u8> {
    let mut report = Vec::with_capacity(payload.len() + 1);
    report.push(0);
    report.extend_from_slice(payload);
    report
}

/// An open conversation with one VIA device.
pub struct Session {
    transport: Box<dyn ViaTransport>,
    protocol: u16,
    layers: u8,
}

impl Session {
    /// Open a device and confirm it speaks a VIA revision this build knows.
    ///
    /// The handshake is not optional politeness: matching the vendor usage
    /// page only makes a device a candidate, and the layer count read here is
    /// what every later bounds check is against.
    ///
    /// # Errors
    ///
    /// [`BackendError`] if the device cannot be opened or does not answer, and
    /// a protocol error if it speaks a revision this build has not been
    /// written against — refused rather than addressed on a guess.
    pub async fn open(attached: &Attached) -> Result<Self, BackendError> {
        let (reader, writer) = attached
            .device
            .open()
            .await
            .map_err(crate::transport::backend_error)?;
        Self::with_transport(Box::new(HostTransport { reader, writer })).await
    }

    /// Build a session over any transport, performing the same handshake.
    ///
    /// Public so a caller can drive a board over something other than this
    /// host's HID stack — and so the tests below can drive a scripted one.
    ///
    /// # Errors
    ///
    /// As [`Self::open`].
    pub async fn with_transport(transport: Box<dyn ViaTransport>) -> Result<Self, BackendError> {
        let mut session = Self {
            transport,
            protocol: 0,
            layers: 0,
        };
        let Response::ProtocolVersion(protocol) =
            session.exchange(Command::GetProtocolVersion).await?
        else {
            return Err(unexpected(
                "the device answered a protocol query with something else",
            ));
        };
        check_protocol(protocol).map_err(|error| unexpected(&error.to_string()))?;
        session.protocol = protocol;

        let Response::LayerCount(layers) = session.exchange(Command::GetLayerCount).await? else {
            return Err(unexpected(
                "the device answered a layer query with something else",
            ));
        };
        session.layers = layers;
        Ok(session)
    }

    /// The VIA protocol revision this device reported.
    #[must_use]
    pub const fn protocol(&self) -> u16 {
        self.protocol
    }

    /// How many keymap layers this device holds.
    #[must_use]
    pub const fn layers(&self) -> u8 {
        self.layers
    }

    /// Read the keycode at one position.
    ///
    /// # Errors
    ///
    /// [`BackendError`] if the layer does not exist, or the device does not
    /// answer.
    pub async fn keycode(&mut self, layer: u8, row: u8, column: u8) -> Result<u16, BackendError> {
        self.check_layer(layer)?;
        let Response::Keycode(keycode) = self
            .exchange(Command::GetKeycode { layer, row, column })
            .await?
        else {
            return Err(unexpected(
                "the device answered a keycode query with something else",
            ));
        };
        Ok(keycode)
    }

    /// Write a keycode, then read it back and confirm it landed.
    ///
    /// The read-back is the point. A write that silently did not take leaves
    /// someone pressing a key that does the old thing while the tool insists
    /// it changed — and the way out of that is the same tool they no longer
    /// trust. Confirming costs one more exchange and removes the whole class.
    ///
    /// # Errors
    ///
    /// [`BackendError`] if the layer does not exist, the device does not
    /// answer, or the keycode read back differs from the one written.
    pub async fn set_keycode(
        &mut self,
        layer: u8,
        row: u8,
        column: u8,
        keycode: u16,
    ) -> Result<(), BackendError> {
        self.check_layer(layer)?;
        self.exchange(Command::SetKeycode {
            layer,
            row,
            column,
            keycode,
        })
        .await?;
        let landed = self.keycode(layer, row, column).await?;
        if landed != keycode {
            return Err(unexpected(&format!(
                "wrote {keycode:#06x} to layer {layer}, row {row}, column {column}, \
                 but the keyboard reads back {landed:#06x}; the keymap was not changed \
                 as asked"
            )));
        }
        Ok(())
    }

    /// Refuse a layer the keyboard does not have.
    fn check_layer(&self, layer: u8) -> Result<(), BackendError> {
        if layer >= self.layers {
            return Err(unexpected(&format!(
                "layer {layer} does not exist; this keyboard has {} (0 to {})",
                self.layers,
                self.layers.saturating_sub(1)
            )));
        }
        Ok(())
    }

    /// Send one command and read the answer to *that* command.
    ///
    /// Reports echoing a different command are skipped, because a QMK board
    /// can send an unrelated raw report at any moment and the answer is not
    /// guaranteed to be next. Skipping is bounded: a board that never answers
    /// produces an error naming what was asked, rather than hanging forever
    /// with nothing on screen.
    async fn exchange(&mut self, command: Command) -> Result<Response, BackendError> {
        self.transport
            .write_output_report(&command.encode())
            .await?;
        let mut buffer = [0_u8; REPORT_LEN];
        let mut strays = 0_usize;
        while strays <= MAX_STRAY_REPORTS {
            let filled = tokio::time::timeout(
                ANSWER_TIMEOUT,
                self.transport.read_input_report(&mut buffer),
            )
            .await
            .map_err(|_| {
                unexpected(&format!(
                    "the device did not answer command {:#04x} within {} seconds. It may \
                     not be a VIA device, or the HID collection this driver picked may be \
                     the wrong one for it.",
                    command.id() as u8,
                    ANSWER_TIMEOUT.as_secs()
                ))
            })??;
            match Response::parse(command, &buffer[..filled]) {
                Ok(response) => return Ok(response),
                // Not this command's answer. A QMK board sends unrelated raw
                // reports whenever it likes, so this is ordinary — read again.
                Err(roadie_via::ProtocolError::Mismatched { .. }) => strays += 1,
                Err(error) => return Err(unexpected(&error.to_string())),
            }
        }
        // The count is the one actually reached, not the constant. A message
        // naming a number the code did not do is what someone diagnosing a
        // board counts against, and is then misled by.
        Err(unexpected(&format!(
            "the keyboard sent {strays} unrelated reports and never answered command \
             {:#04x}",
            command.id() as u8
        )))
    }
}

/// Phrase a protocol-level surprise as a backend error.
fn unexpected(detail: &str) -> BackendError {
    BackendError::Backend(detail.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use hidpp::async_trait;
    use roadie_device::backend::BackendError;
    use roadie_via::command::CommandId;
    use roadie_via::identity::REPORT_LEN;

    use super::{MAX_SCAN_EDGE, MAX_STRAY_REPORTS, Session, ViaTransport, scan_edge};

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_output_keeps_the_via_command_after_report_id_zero() {
        // Literal protocol query and Windows wire buffer: the missing zero
        // made the real Cybrix 16 time out instead of reporting protocol 12.
        let payload = [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ];
        let expected = [
            0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0,
        ];
        assert_eq!(super::windows_output_report(&payload), expected);
    }

    /// A device that answers from a script.
    ///
    /// Hands back queued reports in order, which is what makes the
    /// stray-report and read-back paths checkable without a keyboard to risk.
    /// The written traffic goes to a shared recorder rather than a field,
    /// because the session takes ownership of the transport and a test still
    /// needs to see what went out.
    struct Scripted {
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        replies: Vec<[u8; REPORT_LEN]>,
        /// Never answer once the script is spent, rather than erroring.
        ///
        /// The difference matters: an error is something the code already
        /// handles, and silence is the case the timeout exists for.
        silent_when_spent: bool,
    }

    impl Scripted {
        fn new(replies: Vec<[u8; REPORT_LEN]>) -> Self {
            Self::recording(replies, Arc::new(Mutex::new(Vec::new())))
        }

        fn recording(replies: Vec<[u8; REPORT_LEN]>, written: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
            Self {
                written,
                replies,
                silent_when_spent: false,
            }
        }

        /// A device that answers the script and then goes quiet.
        fn then_silent(replies: Vec<[u8; REPORT_LEN]>) -> Self {
            Self {
                silent_when_spent: true,
                ..Self::new(replies)
            }
        }
    }

    #[async_trait]
    impl ViaTransport for Scripted {
        async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
            self.written
                .lock()
                .expect("the recorder is not poisoned")
                .push(report.to_vec());
            Ok(())
        }

        async fn read_input_report(&mut self, buffer: &mut [u8]) -> Result<usize, BackendError> {
            if self.replies.is_empty() {
                if self.silent_when_spent {
                    // Never resolves. Under `start_paused` tokio advances its
                    // clock once every task is idle, so the timeout fires
                    // without the test actually waiting.
                    std::future::pending::<()>().await;
                }
                return Err(BackendError::Backend("the script ran out".to_owned()));
            }
            let reply = self.replies.remove(0);
            buffer[..REPORT_LEN].copy_from_slice(&reply);
            Ok(REPORT_LEN)
        }
    }

    fn reply(id: CommandId, payload: &[(usize, u8)]) -> [u8; REPORT_LEN] {
        let mut report = [0_u8; REPORT_LEN];
        report[0] = id as u8;
        for &(index, value) in payload {
            report[index] = value;
        }
        report
    }

    fn handshake() -> Vec<[u8; REPORT_LEN]> {
        vec![
            reply(CommandId::GetProtocolVersion, &[(1, 0), (2, 9)]),
            reply(CommandId::GetLayerCount, &[(1, 4)]),
        ]
    }

    /// A board's answer to a keycode read, echoing the position asked about.
    ///
    /// The echo is not decoration: it is what tells this position's answer
    /// from the previous one's, since every keycode reply carries the same
    /// command byte. A fixture that left it zero would be a board that no
    /// real firmware resembles, and would let a bug through here.
    fn keycode_reply_at(layer: u8, row: u8, column: u8, keycode: u16) -> [u8; REPORT_LEN] {
        let [high, low] = keycode.to_be_bytes();
        reply(
            CommandId::GetKeycode,
            &[(1, layer), (2, row), (3, column), (4, high), (5, low)],
        )
    }

    /// The commonest case in these tests: the origin key.
    fn keycode_reply(keycode: u16) -> [u8; REPORT_LEN] {
        keycode_reply_at(0, 0, 0, keycode)
    }

    #[test]
    fn a_scan_wider_than_any_real_board_is_cut_down_and_says_so() {
        assert_eq!(scan_edge(6), (6, false));
        assert_eq!(scan_edge(MAX_SCAN_EDGE), (MAX_SCAN_EDGE, false));
        // A scan that quietly stopped short is indistinguishable from a
        // keyboard with nothing there, so the caller has to be told.
        assert_eq!(scan_edge(255), (MAX_SCAN_EDGE, true));
    }

    #[tokio::test]
    async fn opening_reads_the_protocol_and_the_layer_count() {
        let session = Session::with_transport(Box::new(Scripted::new(handshake())))
            .await
            .expect("the handshake succeeds");
        assert_eq!(session.protocol(), 9);
        assert_eq!(session.layers(), 4);
    }

    /// Protocol 12 is what QMK has shipped since its keycode refactor, and it
    /// is what the boards actually on desks today report — the Kiiboom Cybrix
    /// 16 that first hit this path spoke 12 and was refused by a build that
    /// knew only 9.
    #[tokio::test]
    async fn a_protocol_12_board_is_accepted() {
        let replies = vec![
            reply(CommandId::GetProtocolVersion, &[(1, 0), (2, 12)]),
            reply(CommandId::GetLayerCount, &[(1, 4)]),
        ];
        let session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("a protocol 12 handshake succeeds");
        assert_eq!(session.protocol(), 12);
        assert_eq!(session.layers(), 4);
    }

    /// Refusing an unknown revision before anything is written is the whole
    /// safety story: VIA payload layouts have changed between revisions, and
    /// addressing one we do not know means writing bytes of unknown meaning
    /// into someone's keymap.
    #[tokio::test]
    async fn an_unknown_protocol_revision_is_refused_at_the_door() {
        let replies = vec![reply(CommandId::GetProtocolVersion, &[(1, 0), (2, 11)])];
        let Err(error) = Session::with_transport(Box::new(Scripted::new(replies))).await else {
            panic!("an unknown revision must be refused");
        };
        assert!(format!("{error}").contains("11"), "{error}");
    }

    /// A QMK board sends unrelated raw reports whenever it likes. Reading the
    /// next report positionally would hand another command's payload back as
    /// a keycode.
    #[tokio::test]
    async fn a_stray_report_is_skipped_rather_than_read_as_the_answer() {
        let mut replies = handshake();
        replies.push(reply(CommandId::GetLayerCount, &[(1, 99)]));
        replies.push(keycode_reply(0x0068));
        let mut session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("the handshake succeeds");
        assert_eq!(
            session.keycode(0, 0, 0).await.expect("the real answer"),
            0x0068
        );
    }

    /// Skipping strays forever would hang the caller with nothing on screen,
    /// which is worse than an error that says what was asked.
    #[tokio::test]
    async fn a_board_that_never_answers_gives_up_rather_than_hanging() {
        let mut replies = handshake();
        for _ in 0..=MAX_STRAY_REPORTS {
            replies.push(reply(CommandId::GetLayerCount, &[(1, 4)]));
        }
        let mut session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("the handshake succeeds");
        let error = session.keycode(0, 0, 0).await.expect_err("it must give up");
        let text = format!("{error}");
        assert!(text.contains("never answered"), "{text}");
        // The number in the message has to be the number of reports actually
        // skipped. Someone diagnosing a board counts them.
        assert!(
            text.contains(&format!("sent {} unrelated", MAX_STRAY_REPORTS + 1)),
            "{text}"
        );
    }

    /// A silent device must produce an error rather than a wait with nothing
    /// on screen. The stray limit cannot catch this: it only counts reports
    /// that arrive, and here none do.
    #[tokio::test(start_paused = true)]
    async fn a_device_that_says_nothing_at_all_times_out_rather_than_hanging() {
        // The handshake, then silence.
        let mut session = Session::with_transport(Box::new(Scripted::then_silent(handshake())))
            .await
            .expect("the handshake succeeds");
        let error = session
            .keycode(0, 0, 0)
            .await
            .expect_err("silence must surface");
        let text = format!("{error}");
        assert!(text.contains("did not answer"), "{text}");
        assert!(
            text.contains("wrong one for it"),
            "the message has to name the likely cause, not just the timeout: {text}"
        );
    }

    /// The boundary the limit sits on: a board that sends exactly as many
    /// strays as are tolerated and then answers has answered, and giving up
    /// one report early would call a working keyboard broken.
    #[tokio::test]
    async fn a_board_that_answers_on_the_last_tolerated_report_is_not_given_up_on() {
        let mut replies = handshake();
        for _ in 0..MAX_STRAY_REPORTS {
            replies.push(reply(CommandId::GetLayerCount, &[(1, 4)]));
        }
        replies.push(keycode_reply(0x0068));
        let mut session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("the handshake succeeds");
        assert_eq!(
            session.keycode(0, 0, 0).await.expect("the late answer"),
            0x0068
        );
    }

    #[tokio::test]
    async fn a_write_that_lands_is_reported_as_success() {
        let mut replies = handshake();
        replies.push(reply(
            CommandId::SetKeycode,
            &[(1, 0), (2, 1), (3, 2), (4, 0x00), (5, 0x68)],
        ));
        replies.push(keycode_reply_at(0, 1, 2, 0x0068));
        let mut session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("the handshake succeeds");
        session
            .set_keycode(0, 1, 2, 0x0068)
            .await
            .expect("the write lands");
    }

    /// The failure this read-back exists to catch. A write that silently did
    /// not take leaves someone pressing a key that does the old thing while
    /// the tool insists it changed.
    #[tokio::test]
    async fn a_write_that_did_not_take_is_reported_rather_than_assumed() {
        let mut replies = handshake();
        replies.push(reply(
            CommandId::SetKeycode,
            &[(1, 0), (2, 1), (3, 2), (4, 0x00), (5, 0x68)],
        ));
        replies.push(keycode_reply_at(0, 1, 2, 0x0004));
        let mut session = Session::with_transport(Box::new(Scripted::new(replies)))
            .await
            .expect("the handshake succeeds");
        let error = session
            .set_keycode(0, 1, 2, 0x0068)
            .await
            .expect_err("the mismatch must surface");
        let text = format!("{error}");
        assert!(text.contains("0x0068"), "{text}");
        assert!(text.contains("0x0004"), "{text}");
        assert!(text.contains("not changed"), "{text}");
    }

    /// A layer past the end is refused with the range, not sent to the board.
    #[tokio::test]
    async fn a_layer_the_keyboard_does_not_have_is_refused_locally() {
        let mut session = Session::with_transport(Box::new(Scripted::new(handshake())))
            .await
            .expect("the handshake succeeds");
        let error = session
            .keycode(9, 0, 0)
            .await
            .expect_err("layer 9 of 4 does not exist");
        let text = format!("{error}");
        assert!(text.contains("0 to 3"), "{text}");
    }

    /// A short write leaves the tail of the previous report in place, which is
    /// how a stray keycode ends up somewhere nobody asked for.
    #[tokio::test]
    async fn every_request_goes_out_as_a_full_report() {
        let mut replies = handshake();
        replies.push(keycode_reply(0x0004));
        let written = Arc::new(Mutex::new(Vec::new()));
        let transport = Box::new(Scripted::recording(replies, Arc::clone(&written)));
        let mut session = Session::with_transport(transport)
            .await
            .expect("the handshake succeeds");
        session.keycode(0, 0, 0).await.expect("an answer");
        let sent = written.lock().expect("the recorder is not poisoned");
        assert_eq!(sent.len(), 3, "handshake is two exchanges, then the read");
        assert!(
            sent.iter().all(|report| report.len() == REPORT_LEN),
            "every request must be a full report: {sent:?}"
        );
    }
}
