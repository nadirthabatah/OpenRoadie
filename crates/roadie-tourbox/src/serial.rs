//! The host half: finding a TourBox's serial port and reading events off it.
//!
//! A TourBox appears as a USB CDC serial port, and opening one is a short
//! conversation, not a plain read. An Elite says nothing at all until it is
//! sent [`UNLOCK_MESSAGE`] — a fact this desk's own hardware proved on
//! 2026-09-02, after the transcribed claim that "a TourBox streams without
//! being talked to" had cost three sessions of silent listeners. So
//! [`TourBox::open_path`] sends the unlock, collects whatever reply comes
//! (an Elite answers with 26 bytes; a device that answers nothing is not
//! treated as broken, because the NEO drivers read events without one), and
//! then sends [`SETUP_MESSAGE`] to quiet the haptics. Only after that do
//! single-byte events flow.
//!
//! Discovery is deliberately narrow. A serial port is not a safe thing to
//! open speculatively: the same interface carries modems, microcontrollers
//! and industrial equipment, and writing to the wrong one can do real harm.
//! So [`ports`] returns only ports whose USB identity is a TourBox this
//! build recognises, and anything else has to be named explicitly through
//! [`TourBox::open_path`].

use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use serialport::{SerialPort, SerialPortType};
use thiserror::Error;

use crate::ProtocolError;
use crate::event::{Event, SETUP_MESSAGE, UNLOCK_MESSAGE, decode};
use crate::model::{Model, identify};

/// The line rate a TourBox is driven at.
///
/// A USB CDC port carries bytes over USB regardless of what rate is asked
/// for, so this matters less than it would on a real UART. It is set anyway
/// because the value is what every other driver uses, and a port left at
/// whatever the last program chose is a difference nobody wants to debug.
pub const BAUD_RATE: u32 = 115_200;

/// How long a read waits before reporting that nothing arrived.
///
/// Short enough that a caller polling for events stays responsive to a
/// cancel, long enough not to spin.
const READ_TIMEOUT: Duration = Duration::from_millis(100);

/// Why a TourBox could not be found, opened or read.
#[derive(Debug, Error)]
pub enum SerialError {
    /// The host's serial ports could not be listed at all.
    #[error("could not list this computer's serial ports: {reason}")]
    Enumeration {
        /// What the platform reported.
        reason: String,
    },
    /// A port was found but would not open.
    #[error("could not open the TourBox at {path}: {reason}")]
    Open {
        /// The port that would not open.
        path: String,
        /// What the platform reported.
        reason: String,
    },
    /// The port opened but the unlock conversation could not be carried out.
    #[error("could not wake the TourBox at {path}: {reason}")]
    Handshake {
        /// The port that was being woken.
        path: String,
        /// What the platform reported.
        reason: String,
    },
    /// The port opened but a read failed.
    #[error("could not read from the TourBox at {path}: {reason}")]
    Read {
        /// The port being read.
        path: String,
        /// What the platform reported.
        reason: String,
    },
    /// The port closed underneath us, which is what unplugging looks like.
    #[error("the TourBox at {path} was disconnected")]
    Disconnected {
        /// The port that went away.
        path: String,
    },
    /// A byte arrived that the protocol does not explain.
    #[error("the TourBox at {path} sent something unexpected: {source}")]
    Protocol {
        /// The port it came from.
        path: String,
        /// What was wrong with it.
        source: ProtocolError,
    },
}

/// A serial port that a TourBox is behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// The device path to open.
    pub path: String,
    /// Which TourBox it is.
    pub model: &'static Model,
    /// The serial number the USB descriptor carries, when it carries one.
    pub serial_number: Option<String>,
}

impl Port {
    /// A sentence naming this port, for a screen reader.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.serial_number {
            Some(serial) => format!("{} at {}, serial {serial}", self.model.name, self.path),
            None => format!("{} at {}", self.model.name, self.path),
        }
    }
}

/// Every TourBox this build recognises, by its USB identity.
///
/// A TourBox Neo reaches the host through a general-purpose serial bridge
/// and so cannot be told apart from any other device using the same bridge;
/// it will not appear here. See [`crate::model::CP210X_VENDOR_ID`].
///
/// # Errors
///
/// [`SerialError::Enumeration`] if the platform's port list cannot be read.
pub fn ports() -> Result<Vec<Port>, SerialError> {
    let found = serialport::available_ports().map_err(|error| SerialError::Enumeration {
        reason: error.to_string(),
    })?;

    let mut ports = Vec::new();
    for port in found {
        let SerialPortType::UsbPort(usb) = &port.port_type else {
            continue;
        };
        let Some(model) = identify(usb.vid, usb.pid) else {
            continue;
        };
        if !is_usable_path(&port.port_name) {
            continue;
        }
        tracing::debug!(
            path = %port.port_name,
            model = model.name,
            "found a TourBox"
        );
        ports.push(Port {
            path: port.port_name.clone(),
            model,
            serial_number: usb.serial_number.clone(),
        });
    }
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ports)
}

/// Whether a port path is the one to actually open.
///
/// macOS publishes every serial device twice: `/dev/tty.NAME` is the
/// dial-in side and `/dev/cu.NAME` is the call-out side. They are the same
/// hardware, so listing both would report one TourBox as two, and the
/// `tty.` half is the wrong one to hand anybody — opening it blocks waiting
/// for a carrier signal that a controller never asserts. Found on a TourBox
/// Elite on 2026-08-31, where enumeration returned both.
///
/// Every other platform names a serial port once, so there is nothing to
/// choose between and this is the identity function there.
#[cfg(target_os = "macos")]
fn is_usable_path(path: &str) -> bool {
    !path.rsplit('/').next().unwrap_or(path).starts_with("tty.")
}

/// Whether a port path is the one to actually open. See the macOS version:
/// only macOS publishes a serial device under two names.
#[cfg(not(target_os = "macos"))]
fn is_usable_path(_path: &str) -> bool {
    true
}

/// How many polls to spend collecting the unlock reply.
///
/// Each poll waits up to [`READ_TIMEOUT`], so this bounds the wait at
/// roughly 700 milliseconds for a device that never answers. The Elite on
/// this project's desk answered inside the first poll.
const UNLOCK_REPLY_POLLS: usize = 7;

/// How many bytes an Elite answers the unlock with.
///
/// Observed on hardware, 2026-09-02: 26 bytes beginning `0x07`. Collected
/// as an upper bound rather than demanded, because the NEO drivers read
/// events without any reply at all.
const UNLOCK_REPLY_LEN: usize = 26;

/// Wake the controller and quiet its haptics, returning whatever it
/// answered to the unlock.
///
/// The order is the contract: [`UNLOCK_MESSAGE`] first, the reply
/// collected, [`SETUP_MESSAGE`] after. An Elite that has not been sent the
/// unlock streams nothing at all — see the module documentation for how
/// that was learned. A device that answers nothing is configured anyway
/// rather than refused, because a missing reply is how the models without
/// the requirement behave, not evidence of a fault.
///
/// Generic over plain reads and writes rather than taking a serial port,
/// so the exchange is testable with no controller on the desk.
fn unlock<P: Read + Write + ?Sized>(port: &mut P) -> std::io::Result<Vec<u8>> {
    port.write_all(&UNLOCK_MESSAGE)?;
    port.flush()?;
    let mut reply = Vec::new();
    let mut buffer = [0_u8; 64];
    for _ in 0..UNLOCK_REPLY_POLLS {
        match port.read(&mut buffer) {
            Ok(0) => {
                if !reply.is_empty() {
                    break;
                }
            }
            Ok(filled) => {
                reply.extend_from_slice(&buffer[..filled]);
                if reply.len() >= UNLOCK_REPLY_LEN {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => {
                if !reply.is_empty() {
                    break;
                }
            }
            Err(error) => return Err(error),
        }
    }
    port.write_all(&SETUP_MESSAGE)?;
    port.flush()?;
    Ok(reply)
}

/// An open TourBox.
pub struct TourBox {
    port: Box<dyn SerialPort>,
    path: String,
    model: Option<&'static Model>,
}

impl std::fmt::Debug for TourBox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TourBox")
            .field("path", &self.path)
            .field("model", &self.model.map(|model| model.name))
            .finish_non_exhaustive()
    }
}

impl TourBox {
    /// Open a TourBox found by [`ports`].
    ///
    /// # Errors
    ///
    /// [`SerialError::Open`] if the port will not open. On macOS that is
    /// usually another program already holding it.
    pub fn open(port: &Port) -> Result<Self, SerialError> {
        Self::open_path(&port.path, Some(port.model))
    }

    /// Open a serial port that is believed to be a TourBox.
    ///
    /// For a model whose USB identity cannot be recognised — see [`ports`]
    /// — where the caller has named the port themselves.
    ///
    /// # Errors
    ///
    /// [`SerialError::Open`] if the port will not open.
    pub fn open_path(path: &str, model: Option<&'static Model>) -> Result<Self, SerialError> {
        let mut port = serialport::new(path, BAUD_RATE)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|error| SerialError::Open {
                path: path.to_owned(),
                reason: error.to_string(),
            })?;
        let reply = unlock(port.as_mut()).map_err(|error| SerialError::Handshake {
            path: path.to_owned(),
            reason: error.to_string(),
        })?;
        if reply.is_empty() {
            tracing::debug!(path, "opened a TourBox; nothing answered the unlock");
        } else {
            tracing::debug!(
                path,
                reply_len = reply.len(),
                "opened a TourBox and it answered the unlock"
            );
        }
        Ok(Self {
            port,
            path: path.to_owned(),
            model,
        })
    }

    /// The port this was opened on.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Which TourBox this is, when that is known.
    #[must_use]
    pub fn model(&self) -> Option<&'static Model> {
        self.model
    }

    /// Wait for the next event.
    ///
    /// `Ok(None)` means the wait expired with nothing to report, which is
    /// the ordinary state of a controller nobody is touching — not an
    /// error, and not a reason to stop polling.
    ///
    /// # Errors
    ///
    /// [`SerialError::Disconnected`] when the device goes away,
    /// [`SerialError::Read`] for any other I/O failure, and
    /// [`SerialError::Protocol`] for a byte the protocol does not explain.
    /// A protocol error is worth reporting and is not worth stopping for:
    /// the next byte is a fresh event, because the encoding has no framing
    /// to resynchronise.
    pub fn read_event(&mut self) -> Result<Option<Event>, SerialError> {
        let mut byte = [0u8; 1];
        match self.port.read(&mut byte) {
            Ok(0) => Ok(None),
            Ok(_) => {
                let event = decode(byte[0]).map_err(|source| SerialError::Protocol {
                    path: self.path.clone(),
                    source,
                })?;
                tracing::trace!(byte = byte[0], event = %event.describe(), "TourBox event");
                Ok(Some(event))
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => Ok(None),
            // An unplugged USB serial port stops answering rather than
            // reporting a closed file, and the kind it reports for that
            // differs per platform. Both mean the same thing to a caller.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::BrokenPipe | ErrorKind::NotConnected | ErrorKind::UnexpectedEof
                ) =>
            {
                Err(SerialError::Disconnected {
                    path: self.path.clone(),
                })
            }
            Err(error) => Err(SerialError::Read {
                path: self.path.clone(),
                reason: error.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MODELS;

    /// The description is what a screen reader is handed, so it has to be a
    /// sentence and it has to name the port someone would type.
    #[test]
    fn a_port_describes_itself_with_its_path() {
        let port = Port {
            path: "/dev/cu.usbmodem000000011".to_owned(),
            model: &MODELS[0],
            serial_number: None,
        };
        assert_eq!(
            port.describe(),
            "TourBox Elite at /dev/cu.usbmodem000000011"
        );
    }

    /// A serial number is worth saying when there is one, because it is how
    /// two identical controllers are told apart.
    #[test]
    fn a_port_with_a_serial_number_says_it() {
        let port = Port {
            path: "/dev/cu.usbmodem000000011".to_owned(),
            model: &MODELS[0],
            serial_number: Some("00000001".to_owned()),
        };
        assert_eq!(
            port.describe(),
            "TourBox Elite at /dev/cu.usbmodem000000011, serial 00000001"
        );
    }

    /// A port made of script: reads come from a queue of chunks, writes are
    /// recorded, and an empty queue times out the way a quiet serial port
    /// does. What the unlock exchange needs and nothing more.
    struct ScriptedPort {
        written: Vec<u8>,
        replies: Vec<Vec<u8>>,
        fail_writes: bool,
    }

    impl ScriptedPort {
        fn answering(replies: Vec<Vec<u8>>) -> Self {
            Self {
                written: Vec::new(),
                replies,
                fail_writes: false,
            }
        }
    }

    impl std::io::Read for ScriptedPort {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.replies.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "nothing arrived",
                ));
            }
            let chunk = self.replies.remove(0);
            buffer[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    impl std::io::Write for ScriptedPort {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail_writes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "the port is gone",
                ));
            }
            self.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The reply the Elite on this project's desk actually sent, byte for
    /// byte, on 2026-09-02 — the first bytes that controller ever produced
    /// for this codebase.
    const ELITE_REPLY: [u8; 26] = [
        0x07, 0xca, 0x31, 0x79, 0x47, 0x6e, 0xf4, 0xdb, 0x1d, 0xd5, 0x6c, 0xb1, 0xf0, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x03, 0x02, 0x01, 0x00, 0x00,
    ];

    /// The order is the contract: unlock, then setup, and the reply handed
    /// back. The bytes are spelled out rather than taken from the constants
    /// under test, because a test that agrees with the code proves nothing.
    #[test]
    fn the_unlock_goes_out_before_the_setup_and_the_reply_comes_back() {
        let mut port = ScriptedPort::answering(vec![ELITE_REPLY.to_vec()]);
        let reply = unlock(&mut port).expect("the exchange completes");
        assert_eq!(reply, ELITE_REPLY);
        assert_eq!(
            &port.written[..8],
            &[0x55, 0x00, 0x07, 0x88, 0x94, 0x00, 0x1a, 0xfe],
            "the unlock command must go out first, exactly as published"
        );
        assert_eq!(
            &port.written[8..],
            &SETUP_MESSAGE,
            "the setup message follows the unlock"
        );
    }

    /// A reply fragmented across reads is one reply. Serial ports owe
    /// nobody whole messages.
    #[test]
    fn a_reply_split_across_reads_is_collected_whole() {
        let (head, tail) = ELITE_REPLY.split_at(10);
        let mut port = ScriptedPort::answering(vec![head.to_vec(), tail.to_vec()]);
        let reply = unlock(&mut port).expect("the exchange completes");
        assert_eq!(reply, ELITE_REPLY);
    }

    /// A device that answers nothing is configured anyway. The models
    /// without the unlock requirement never answer, and refusing them
    /// would trade three sessions of silence for a hard error on working
    /// hardware.
    #[test]
    fn a_silent_device_is_still_sent_the_setup() {
        let mut port = ScriptedPort::answering(Vec::new());
        let reply = unlock(&mut port).expect("silence is not a fault here");
        assert_eq!(reply, [] as [u8; 0]);
        let setup_start = port.written.len() - SETUP_MESSAGE.len();
        assert_eq!(&port.written[setup_start..], &SETUP_MESSAGE);
    }

    /// A port that cannot be written to is a fault, and it surfaces rather
    /// than being read from anyway.
    #[test]
    fn a_write_failure_surfaces_instead_of_being_read_past() {
        let mut port = ScriptedPort {
            written: Vec::new(),
            replies: vec![ELITE_REPLY.to_vec()],
            fail_writes: true,
        };
        let error = unlock(&mut port).expect_err("a dead port must surface");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    /// macOS names one serial device twice. Listing both reported a single
    /// TourBox as two on real hardware, and the `tty.` name is the one that
    /// blocks on open, so it is the one dropped.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_dial_in_half_of_a_macos_serial_port_is_not_listed() {
        assert!(is_usable_path("/dev/cu.usbmodem000000011"));
        assert!(!is_usable_path("/dev/tty.usbmodem000000011"));
    }

    /// The filter keys on the file name, not on the string anywhere. A
    /// directory that happens to contain "tty." must not disqualify a port.
    #[cfg(target_os = "macos")]
    #[test]
    fn only_the_file_name_decides_whether_a_port_is_the_dial_in_half() {
        assert!(is_usable_path("/dev/tty.d/cu.usbmodem1"));
    }

    /// Enumeration must not fail on a machine with no TourBox attached, and
    /// must not invent one. This runs on CI hosts with no hardware at all.
    /// When hardware *is* attached it also holds the rule above: one
    /// controller is one entry.
    #[test]
    fn enumeration_on_a_machine_with_no_tourbox_finds_nothing_and_does_not_fail() {
        match ports() {
            Ok(found) => {
                // One controller is one entry. Keyed on the serial number
                // rather than on the path, because the duplicate this
                // guards against is the same device under two paths.
                let mut serials: Vec<&str> = found
                    .iter()
                    .filter_map(|port| port.serial_number.as_deref())
                    .collect();
                let before = serials.len();
                serials.sort_unstable();
                serials.dedup();
                assert_eq!(
                    before,
                    serials.len(),
                    "one TourBox was listed more than once: {found:?}"
                );
                for port in &found {
                    assert_eq!(port.model.name, "TourBox Elite");
                }
            }
            // A container with no serial subsystem at all is a legitimate
            // outcome here, and not a failure of this crate.
            Err(SerialError::Enumeration { .. }) => {}
            Err(other) => panic!("enumeration reported the wrong kind of failure: {other}"),
        }
    }
}
