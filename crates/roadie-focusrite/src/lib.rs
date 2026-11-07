//! Reaching a Focusrite interface's control channel on this host.
//!
//! The host-facing sibling of [`roadie_scarlett`], which holds the protocol
//! and the per-model tables and knows nothing about USB. The same split as
//! [`roadie_ddc`] and `roadie-display`: the pure half keeps the wasm
//! portability claim, and everything that opens a handle lives here.
//!
//! # What a Focusrite exposes, and which part this drives
//!
//! An interface presents several USB interfaces at once. The audio ones are
//! standard USB audio class and belong to the operating system — this crate
//! never touches them, so recording keeps working while settings change.
//! What it does claim is the **vendor-specific interface**, class 255, which
//! carries the control protocol and which no operating system driver wants.
//! That was the open question in the project's planning notes and it is now
//! settled on hardware: on macOS the vendor interface has no exclusive owner,
//! and claiming it needs no privileges and disturbs nothing.
//!
//! Every exchange is two control transfers, which is [`roadie_scarlett`]'s
//! business; this crate supplies the handle and the ordering.
//!
//! # A note on Linux
//!
//! The USB path here compiles and runs on all three platforms, and its
//! failure mode where a kernel driver has already claimed the interface is an
//! error rather than a disturbance — nothing is force-detached, so a busy
//! interface reports itself instead of interrupting the audio. Linux is still
//! expected to prefer ALSA in the end, because `snd-usb-audio` publishes
//! these same settings as ordinary mixer controls; the name mapping for that
//! is built in `roadie_scarlett::alsa` and the binding over it is not.
//!
//! [`roadie_ddc`]: https://docs.rs/roadie-ddc

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod mock;
pub mod session;
pub mod transport;
pub mod windows_setup;

pub use session::{Session, Settings, Snapshot};
pub use transport::{Attached, Transport, attached};

use roadie_scarlett::{PlanError, ProtocolError};

/// Everything that can go wrong between this host and an interface.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ControlError {
    /// The host's USB stack could not be listed at all.
    #[error("could not list this computer's USB devices: {reason}")]
    Enumeration {
        /// What the platform reported.
        reason: String,
    },

    /// The device was found but would not open.
    ///
    /// On macOS and Linux this is nearly always another program holding it;
    /// on Linux it can also be permissions on the USB node.
    #[error("could not open {name}: {reason}")]
    Open {
        /// Which interface.
        name: String,
        /// What the platform reported.
        reason: String,
    },

    /// The vendor control interface could not be claimed.
    ///
    /// Told apart from [`Self::Open`] deliberately: the device is present and
    /// openable, and it is specifically the control interface that is
    /// unavailable — which is a different thing to be told, and points at a
    /// different cause.
    #[error(
        "could not claim the control interface of {name}: {reason}. The audio side is \
         unaffected; something else is holding the control channel."
    )]
    Claim {
        /// Which interface.
        name: String,
        /// What the platform reported.
        reason: String,
    },

    /// Windows bound its own WinUSB driver to the control interface but never
    /// published a path to open it, because the device names no interface
    /// GUID. Nothing is holding it; Windows simply has not finished.
    /// [`windows_setup`] supplies the GUID, which is the one-time fix.
    #[error(
        "Windows has not finished setting up the control interface of {name}. Nothing else \
         is holding it, and no Focusrite software is needed. Run this once, and accept the \
         Windows permission prompt it raises: roadie audio prepare"
    )]
    WindowsUnpublished {
        /// Which interface.
        name: String,
    },

    /// A transfer failed.
    #[error("a control transfer to {name} failed: {reason}")]
    Transfer {
        /// Which interface.
        name: String,
        /// What the platform reported.
        reason: String,
    },

    /// The interface answered something the protocol does not accept.
    #[error("{0}")]
    Protocol(#[from] ProtocolError),

    /// The change asked for cannot be expressed on this model.
    #[error("{0}")]
    Plan(#[from] PlanError),

    /// This build has no table for the model that answered.
    ///
    /// A product id under Focusrite's vendor id that is not in
    /// [`roadie_scarlett::device::MODELS`]. Named rather than guessed at: the
    /// address layouts differ per model, and writing one model's addresses to
    /// another is how a setting nobody asked about changes.
    #[error(
        "this build has no settings table for Focusrite product {product_id:#06x}. It was \
         found and identified, and that is exactly what a device-support request needs."
    )]
    UnknownModel {
        /// The product id that answered.
        product_id: u16,
    },

    /// The model does not have the setting that was asked for.
    #[error("a {model} has no {setting} to read or change")]
    NoSuchSetting {
        /// The model asked about.
        model: &'static str,
        /// The setting asked for, named the way a person would.
        setting: &'static str,
    },

    /// An input number the model does not have.
    #[error("{model} input {asked} does not exist; it has {count} of those")]
    NoSuchInput {
        /// The model asked about.
        model: &'static str,
        /// The input number asked for, counted from one.
        asked: u16,
        /// How many it has.
        count: u16,
    },

    /// A risky write was attempted without the acknowledgement it needs.
    ///
    /// Carries the sentence rather than a code, because the caller's job on
    /// receiving this is to put that sentence to a person.
    #[error("{0}")]
    NotAcknowledged(String),
}

/// A convenient result type for this crate.
pub type Result<T> = std::result::Result<T, ControlError>;
