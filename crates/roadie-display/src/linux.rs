//! Linux: the DDC line the kernel already publishes.
//!
//! Two independent facilities, and the difference between them decides how
//! this file behaves when something is not permitted.
//!
//! The kernel parses nothing but does expose everything. Each connector under
//! `/sys/class/drm` carries a `status` file saying whether anything is plugged
//! into it and an `edid` file holding the display's own identification block,
//! both **world-readable**. So a display can be found and named with no
//! privilege whatsoever.
//!
//! Talking to it is the other facility: an I²C character device, published for
//! the connector at `ddc/i2c-dev/i2c-N`, and group-owned. A user who is not in
//! the `i2c` group gets `EACCES` from a monitor that is working perfectly.
//!
//! Those two facts together are why enumeration here never drops a display it
//! cannot open. A name without control is a useful answer — it is the
//! difference between "your monitor is there and this build cannot reach it,
//! here is why" and an empty list.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::{fs, io};

use roadie_ddc::Edid;
use roadie_ddc::packet::{Frame, I2C_ADDRESS};

use crate::backend::{DdcTransport, DisplayError, Unreachable};
use crate::ddc::Ddc;
use crate::{Display, DisplayId};

/// Where the kernel publishes one directory per display connector.
const DRM: &str = "/sys/class/drm";

/// `I2C_RDWR`: perform a sequence of transfers in one call.
///
/// Chosen over `I2C_SLAVE` plus ordinary reads and writes because it does not
/// need the address bound to the file description first, and so works even
/// when a kernel driver has already claimed the address — which on a graphics
/// card is common.
///
/// Typed as [`libc::Ioctl`] rather than as a fixed-width integer, because that
/// alias is `c_ulong` against glibc and `c_int` against musl. Writing it as
/// either and casting at the call site compiles cleanly on the one and
/// truncates on the other, which is the sort of thing that is only ever found
/// by building for the other.
const I2C_RDWR: libc::Ioctl = 0x0707;

/// `I2C_M_RD`: this message is a read.
const I2C_M_RD: u16 = 0x0001;

/// One transfer in an `I2C_RDWR` call. Layout is the kernel's `struct i2c_msg`.
#[repr(C)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

/// The argument to `I2C_RDWR`. Layout is the kernel's
/// `struct i2c_rdwr_ioctl_data`.
#[repr(C)]
struct I2cRdwrData {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

/// A connector's I²C bus, opened.
pub(crate) struct I2cBus {
    file: File,
    /// The display's name, for error messages — not the bus path, because
    /// "LG ULTRAFINE did not answer" is the sentence someone can act on.
    name: String,
}

impl I2cBus {
    /// Open `path`, which is a `/dev/i2c-*` node.
    ///
    /// The failure is a sentence rather than a [`DisplayError`] because the
    /// caller is about to attach it to a display that has a name, and the
    /// display's name is the useful half of that sentence.
    fn open(path: &Path, name: String) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| format!("{}: {}", path.display(), explain(&error)))?;
        Ok(Self { file, name })
    }

    /// Run one transfer against the display's DDC/CI address.
    #[expect(
        unsafe_code,
        reason = "I2C_RDWR is an ioctl; there is no safe wrapper for it in this graph"
    )]
    fn transfer(&mut self, buffer: &mut [u8], read: bool) -> Result<usize, DisplayError> {
        let len = u16::try_from(buffer.len()).unwrap_or(u16::MAX);
        let mut message = I2cMsg {
            addr: u16::from(I2C_ADDRESS),
            flags: if read { I2C_M_RD } else { 0 },
            len,
            buf: buffer.as_mut_ptr(),
        };
        let mut data = I2cRdwrData {
            msgs: &raw mut message,
            nmsgs: 1,
        };

        let fd: RawFd = self.file.as_raw_fd();
        // SAFETY: `fd` is an open file description this struct owns for its
        // whole lifetime. `data` points at one `I2cMsg` that outlives the
        // call, whose `buf` points at `buffer`'s allocation with `len` bytes
        // available — `len` is clamped to `buffer.len()` above. Both structs
        // are `repr(C)` with the kernel's own layout.
        let result = unsafe { libc::ioctl(fd, I2C_RDWR, &raw mut data) };
        if result < 0 {
            let error = io::Error::last_os_error();
            return Err(DisplayError::Transport {
                name: self.name.clone(),
                reason: explain(&error),
                // `EAGAIN` is the bus saying it was busy and `ENXIO` is a
                // display that did not acknowledge its address — which a panel
                // waking from standby does for a moment. Both are worth asking
                // again about; a permission error never will be.
                retryable: matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::ENXIO)),
            });
        }
        Ok(buffer.len())
    }
}

impl DdcTransport for I2cBus {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn send(&mut self, frame: &Frame) -> Result<(), DisplayError> {
        let mut bytes = [0_u8; 8];
        let payload = frame.as_bytes();
        let len = payload.len().min(bytes.len());
        bytes[..len].copy_from_slice(&payload[..len]);
        self.transfer(&mut bytes[..len], false).map(|_| ())
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, DisplayError> {
        self.transfer(buffer, true)
    }
}

/// Turn an I/O error into a sentence rather than a code.
///
/// `EACCES` on an I²C node is the single most likely thing to go wrong here,
/// and "Permission denied (os error 13)" tells someone nothing they can act
/// on. The group is the answer, so the group is what it says.
fn explain(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(libc::EACCES) => "permission denied. The I2C devices belong to the i2c group; \
             adding your user to it, logging out and back in is what grants access"
            .to_owned(),
        _ => error.to_string(),
    }
}

/// Every connected display, whether or not its control line can be opened.
pub(crate) fn enumerate() -> Result<Vec<Display>, DisplayError> {
    enumerate_under(Path::new(DRM))
}

/// [`enumerate`], against a given display-subsystem root.
///
/// The root is a parameter for one reason: it is what lets the promise this
/// function makes — that a display is never dropped for being unreachable —
/// be a tested fact rather than a comment. `/sys` cannot be written to, and
/// the machines this is built on have no monitors, so without this the whole
/// enumeration path would be checked only by reading it.
fn enumerate_under(root: &Path) -> Result<Vec<Display>, DisplayError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // No display subsystem at all: a container, a headless server, a
        // kernel built without DRM. That is not a failure to enumerate, it is
        // an enumeration whose answer is none — and reporting it as an error
        // would turn "you have no monitors" into "something went wrong",
        // which is a worse answer to the same question.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(DisplayError::Access {
                path: root.display().to_string(),
                reason: explain(&error),
            });
        }
    };

    let mut displays = Vec::new();
    for entry in entries.flatten() {
        let connector = entry.path();
        if !connected(&connector) {
            continue;
        }
        let id = DisplayId::new(entry.file_name().to_string_lossy().into_owned());
        let edid = read_edid(&connector);
        let name = edid
            .as_ref()
            .map_or_else(|| id.as_str().to_owned(), Edid::describe);

        let node = match i2c_node(&connector) {
            Some(node) => Ok(node),
            None => i2c_by_edid(&connector),
        };
        let backend = match node {
            Ok(node) => match I2cBus::open(&node, name.clone()) {
                Ok(bus) => crate::backend::boxed(Ddc::new(bus)),
                Err(reason) => Unreachable::boxed(name, reason),
            },
            Err(reason) => Unreachable::boxed(name, reason),
        };
        displays.push(Display::new(id, edid, backend));
    }

    // `read_dir` yields in whatever order the filesystem holds. Sorting by the
    // connector name gives `roadie display list` the same order twice running,
    // which matters more than usual when the list is being read aloud and
    // someone is counting positions in it.
    displays.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(displays)
}

/// Whether this directory is a connector with something plugged into it.
fn connected(connector: &Path) -> bool {
    fs::read_to_string(connector.join("status")).is_ok_and(|status| status.trim() == "connected")
}

/// The display's identification block, if the kernel has one.
///
/// A connector that is connected but whose `edid` is empty is a real state:
/// the kernel reports the link before it has read the block, and some KVMs
/// never let it. That is a display with no name, not an absent display, and
/// [`enumerate`] keeps it either way.
///
/// There is deliberately no length check here. `Edid::parse` requires a full
/// 128-byte base block and rejects anything shorter, empty included, so a
/// guard for it would be a branch that cannot fire — and a branch that cannot
/// fire reads as load-bearing to whoever finds it next.
fn read_edid(connector: &Path) -> Option<Edid> {
    Edid::parse(&fs::read(connector.join("edid")).ok()?).ok()
}

/// The `/dev/i2c-*` node carrying this connector's DDC line.
///
/// Two layouts, because the kernel changed where it puts this. Modern drivers
/// publish `<connector>/ddc/i2c-dev/i2c-N`; older ones put an `i2c-N`
/// directory directly under the connector. Both are checked rather than one
/// being assumed, since the cost of guessing wrong is a display that silently
/// cannot be controlled.
fn i2c_node(connector: &Path) -> Option<PathBuf> {
    let named = |directory: PathBuf| -> Option<PathBuf> {
        fs::read_dir(directory).ok()?.flatten().find_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.starts_with("i2c-")
                .then(|| PathBuf::from("/dev").join(name))
        })
    };
    named(connector.join("ddc").join("i2c-dev")).or_else(|| named(connector.to_path_buf()))
}

/// Where the kernel lists every I²C character device.
const I2C_DEV: &str = "/sys/class/i2c-dev";

/// Where a display answers EDID reads on its DDC bus.
const EDID_ADDRESS: u16 = 0x50;

/// What is said when a connector has no DDC line by either route.
const NO_LINE: &str = "the graphics driver published no I2C line for this connector, so it \
                       cannot be controlled over DDC on this machine";

/// Find a connector's DDC bus by its EDID, for a driver that does not link it.
///
/// NVIDIA's proprietary driver creates an I²C adapter for every output but
/// publishes no `ddc` link from the connector to it — found on an RTX 3060
/// under Ubuntu 26.04, 2026-11-03, where every monitor was named and none
/// could be reached. The way back is the one ddcutil takes: read the EDID off
/// each candidate bus and keep the one that matches the block the kernel
/// already published for the connector.
///
/// Only buses whose adapter hangs off the connector's own graphics card are
/// candidates. A motherboard SMBus also answers at `0x50` — that is where a
/// memory module's SPD EEPROM lives — and nothing here has any business
/// writing an offset to one.
///
/// # Errors
///
/// A sentence for the display when no bus matches: the permission sentence
/// when a candidate could not be opened, since that is the likely cause and
/// the one that can be fixed, and [`NO_LINE`] otherwise.
fn i2c_by_edid(connector: &Path) -> Result<PathBuf, String> {
    let wanted = fs::read(connector.join("edid")).unwrap_or_default();
    let Some(wanted) = wanted.get(..128) else {
        return Err(NO_LINE.to_owned());
    };
    let Some(card) = fs::canonicalize(connector)
        .ok()
        .and_then(|path| graphics_card(&path).map(Path::to_path_buf))
    else {
        return Err(NO_LINE.to_owned());
    };
    let mut refused = None;
    for (node, adapter) in i2c_adapters() {
        if adapter.parent() != Some(card.as_path()) {
            continue;
        }
        match read_edid_from(&node) {
            Ok(found) if found == wanted => return Ok(node),
            Err(error) if error.raw_os_error() == Some(libc::EACCES) => {
                refused = Some(format!("{}: {}", node.display(), explain(&error)));
            }
            // Another output's display, or a bus with nothing on it.
            _ => {}
        }
    }
    Err(refused.unwrap_or_else(|| NO_LINE.to_owned()))
}

/// The graphics card a connector belongs to: the directory above the `drm`
/// component of its resolved sysfs path, such as
/// `/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0`.
fn graphics_card(connector: &Path) -> Option<&Path> {
    connector
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "drm"))
        .and_then(Path::parent)
}

/// Every `/dev/i2c-N`, with its adapter's resolved sysfs directory.
fn i2c_adapters() -> Vec<(PathBuf, PathBuf)> {
    let Ok(entries) = fs::read_dir(I2C_DEV) else {
        return Vec::new();
    };
    let mut adapters: Vec<(PathBuf, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let adapter = fs::canonicalize(entry.path().join("device")).ok()?;
            Some((PathBuf::from("/dev").join(entry.file_name()), adapter))
        })
        .collect();
    adapters.sort();
    adapters
}

/// Read a display's 128-byte EDID base block off one bus: write offset 0 to
/// address `0x50`, then read, as one `I2C_RDWR` so nothing can come between.
#[expect(
    unsafe_code,
    reason = "I2C_RDWR is an ioctl; there is no safe wrapper for it in this graph"
)]
fn read_edid_from(node: &Path) -> io::Result<[u8; 128]> {
    let file = OpenOptions::new().read(true).write(true).open(node)?;
    let mut offset = [0_u8];
    let mut block = [0_u8; 128];
    let mut messages = [
        I2cMsg {
            addr: EDID_ADDRESS,
            flags: 0,
            len: 1,
            buf: offset.as_mut_ptr(),
        },
        I2cMsg {
            addr: EDID_ADDRESS,
            flags: I2C_M_RD,
            len: 128,
            buf: block.as_mut_ptr(),
        },
    ];
    let mut data = I2cRdwrData {
        msgs: messages.as_mut_ptr(),
        nmsgs: 2,
    };
    // SAFETY: `file` is open for the whole call. `data` points at two
    // `I2cMsg`s that outlive it, whose `buf`s point at `offset` (1 byte) and
    // `block` (128 bytes), matching their `len`s exactly. Both structs are
    // `repr(C)` with the kernel's own layout.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), I2C_RDWR, &raw mut data) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(block)
}

#[cfg(test)]
mod tests;
