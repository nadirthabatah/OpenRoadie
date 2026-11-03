//! Finding a Focusrite interface on USB, and carrying bytes to its control
//! channel.
//!
//! The control protocol is two transfers per exchange: the request goes out
//! on `bRequest` 2 and the answer is fetched on `bRequest` 3, both addressed
//! to the **vendor-specific interface** rather than to the device. Which
//! interface that is varies by model, so it is discovered from the
//! descriptors rather than assumed — a Vocaster Two answers on interface 3,
//! and nothing guarantees the next model will.

use std::time::Duration;

use nusb::transfer::{Control, ControlType, Recipient};
use roadie_scarlett::device::{Model, VENDOR_ID, find};

use crate::{ControlError, Result};

/// `bRequest` for sending a request.
const SEND: u8 = 2;

/// `bRequest` for fetching the answer to one.
const FETCH: u8 = 3;

/// The USB class that marks the control interface.
///
/// 255 is "vendor specific" — the value that means the class standards do
/// not describe this interface, which is exactly why it is the one carrying
/// a protocol no standard describes.
const VENDOR_SPECIFIC: u8 = 0xff;

/// How long any single transfer may take.
///
/// An interface answers in milliseconds. The generosity is for a busy bus,
/// and the cost of being wrong in the impatient direction is a spurious
/// failure on hardware that was about to answer.
const TIMEOUT: Duration = Duration::from_secs(2);

/// A Focusrite interface the host can see.
///
/// Carries what a list needs and no live handle, so enumerating costs
/// nothing and a caller can name every interface on the desk without opening
/// any of them.
#[derive(Debug, Clone)]
pub struct Attached {
    /// What the model is called on the box.
    pub name: &'static str,
    /// The model's table and capabilities.
    pub model: &'static Model,
    /// The serial number the USB descriptor carries, when it carries one.
    pub serial_number: Option<String>,
    /// The vendor-specific interface number carrying the control protocol.
    pub control_interface: u8,
    /// The device, for opening later.
    info: nusb::DeviceInfo,
}

impl Attached {
    /// A sentence naming this interface, for a screen reader.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.serial_number {
            Some(serial) => format!("{}, serial {serial}", self.name),
            None => self.name.to_owned(),
        }
    }
}

/// Every Focusrite interface attached to this computer.
///
/// A device under Focusrite's vendor id with no table in this build is
/// reported as [`ControlError::UnknownModel`] rather than skipped, because "I can
/// see it and cannot drive it" is a different answer from "there is nothing
/// there" — and the first one is a device-support request waiting to be
/// filed. It is returned per device, so one unknown interface does not hide
/// a known one sitting beside it.
///
/// # Errors
///
/// [`ControlError::Enumeration`] if the host's USB stack cannot be listed at all.
pub fn attached() -> Result<Vec<Result<Attached>>> {
    let devices = nusb::list_devices().map_err(|error| ControlError::Enumeration {
        reason: error.to_string(),
    })?;
    Ok(devices
        .filter(|info| info.vendor_id() == VENDOR_ID)
        .map(|info| {
            let product_id = info.product_id();
            let model =
                find(VENDOR_ID, product_id).ok_or(ControlError::UnknownModel { product_id })?;
            let control_interface = info
                .interfaces()
                .find(|interface| interface.class() == VENDOR_SPECIFIC)
                .map(nusb::InterfaceInfo::interface_number)
                .ok_or_else(|| ControlError::Claim {
                    name: model.name.to_owned(),
                    reason: "it exposes no vendor-specific interface, so this build cannot \
                             tell which one carries the control protocol"
                        .to_owned(),
                })?;
            Ok(Attached {
                name: model.name,
                model,
                serial_number: info.serial_number().map(str::to_owned),
                control_interface,
                info,
            })
        })
        .collect())
}

/// The two halves of one exchange, as a seam.
///
/// A trait rather than the concrete handle so the session above it — the
/// start-up handshake, the sequence checking, the read-modify-write — is
/// exercised against a scripted device. Without this seam none of that could
/// be tested without an interface to risk, and the one setting here that can
/// damage equipment is on the other side of it.
pub trait Transport {
    /// Send one framed request.
    ///
    /// # Errors
    ///
    /// [`ControlError::Transfer`] if the transfer fails.
    fn send(&mut self, bytes: &[u8]) -> Result<()>;

    /// Fetch the answer, asking for at most `len` bytes.
    ///
    /// # Errors
    ///
    /// [`ControlError::Transfer`] if the transfer fails.
    fn fetch(&mut self, len: usize) -> Result<Vec<u8>>;

    /// A name for this device, for putting in an error.
    fn name(&self) -> &str;
}

/// The transport this host actually uses.
pub struct UsbTransport {
    interface: nusb::Interface,
    index: u16,
    name: String,
}

impl UsbTransport {
    /// Open `attached` and claim its control interface.
    ///
    /// Only the vendor interface is claimed. The audio interfaces stay with
    /// the operating system, so recording and playback carry on undisturbed
    /// while settings are read and written.
    ///
    /// # Errors
    ///
    /// [`ControlError::Open`] if the device will not open, and [`ControlError::Claim`] if
    /// the control interface is held by something else. Nothing is
    /// force-detached: a busy interface is reported rather than taken.
    pub fn open(attached: &Attached) -> Result<Self> {
        let name = attached.describe();
        let device = attached.info.open().map_err(|error| ControlError::Open {
            name: name.clone(),
            reason: error.to_string(),
        })?;
        let interface = device
            .claim_interface(attached.control_interface)
            .map_err(|error| {
                let reason = error.to_string();
                if crate::windows_setup::is_unpublished(&reason) {
                    ControlError::WindowsUnpublished { name: name.clone() }
                } else {
                    ControlError::Claim {
                        name: name.clone(),
                        reason,
                    }
                }
            })?;
        tracing::debug!(
            device = %name,
            interface = attached.control_interface,
            "claimed a Focusrite control interface"
        );
        Ok(Self {
            interface,
            index: u16::from(attached.control_interface),
            name,
        })
    }

    /// The control setup for one direction.
    ///
    /// The recipient is the *interface*, not the device: the protocol is
    /// addressed to the vendor interface, and a request sent to the device
    /// instead is answered by silence rather than by a complaint.
    fn control(&self, request: u8) -> Control {
        Control {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request,
            value: 0,
            index: self.index,
        }
    }
}

impl Transport for UsbTransport {
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.interface
            .control_out_blocking(self.control(SEND), bytes, TIMEOUT)
            .map_err(|error| ControlError::Transfer {
                name: self.name.clone(),
                reason: error.to_string(),
            })?;
        Ok(())
    }

    fn fetch(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0_u8; len];
        let filled = self
            .interface
            .control_in_blocking(self.control(FETCH), &mut buffer, TIMEOUT)
            .map_err(|error| ControlError::Transfer {
                name: self.name.clone(),
                reason: error.to_string(),
            })?;
        buffer.truncate(filled);
        Ok(buffer)
    }

    fn name(&self) -> &str {
        &self.name
    }
}
