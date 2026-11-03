//! Windows: the monitor-configuration API, which does the protocol for us.
//!
//! This is the platform that justifies where the seam is. `dxva2.dll` takes a
//! VCP code and a value and handles framing, checksums, timing and retries
//! inside the driver stack — so this backend implements [`VcpBackend`]
//! directly and never touches [`crate::Ddc`] or a packet. A seam one layer
//! lower would have handed Windows a packet layer to route around.
//!
//! What it does not do is identify. `dxva2` describes a monitor as whatever
//! the driver called it, which is very often "Generic PnP Monitor" for every
//! panel on the desk. The EDID is in the registry, under the monitor's device
//! instance, and [`edid_for`] follows the `HMONITOR` there — so a display is
//! named when Windows kept its EDID, and numbered only when it did not.
//!
//! Nor does it pace writes. `dxva2` waits for each reply, but nothing stops a
//! read from following a write sooner than the 50 ms the specification asks
//! for. On an RTK HG560T34 over HDMI, 2026-11-03, every read-back straight
//! after a brightness write failed with an I²C error while the write itself
//! had landed — so the backend owes that gap itself, with [`Pacer`].

use std::ffi::c_void;
use std::time::Instant;

use roadie_ddc::{Capabilities, Edid, Feature, Value};
use windows_sys::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitor, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, SaveCurrentSettings, SetVCPFeature,
};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFOEXW,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;

use crate::backend::{DisplayError, VcpBackend, boxed};
use crate::ddc::{Pacer, Pacing};
use crate::{Display, DisplayId};

/// One physical monitor, as `dxva2` hands it out.
///
/// The handle is kept as an `isize` rather than as the raw pointer type it
/// arrives as, so that nothing in this struct is a pointer whose provenance a
/// reader has to reason about. It is a kernel handle, not an address this
/// process may dereference, and storing it as a number says so.
#[derive(Debug)]
pub(crate) struct PhysicalMonitor {
    handle: isize,
    name: String,
    /// The gap still owed after the last write.
    pacer: Pacer,
}

impl PhysicalMonitor {
    /// The handle, back in the shape the API wants.
    fn raw(&self) -> *mut c_void {
        self.handle as *mut c_void
    }

    /// Wait out whatever the last write still owes before the next call.
    fn settle(&self) {
        let wait = self.pacer.wait_at(Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }

    /// Record that a write just went out.
    fn wrote(&mut self) {
        self.pacer
            .finished(Instant::now(), Pacing::SPECIFIED.after_write);
    }

    /// Shape a failed call as an error, using the OS's own explanation.
    fn failed(&self, what: &str) -> DisplayError {
        DisplayError::Transport {
            name: self.name.clone(),
            reason: format!("{what}: {}", std::io::Error::last_os_error()),
            // `dxva2` has already retried inside the driver stack. A failure
            // that reaches here is one the OS has stopped believing in, and
            // asking again would only spend the timeout twice.
            retryable: false,
        }
    }
}

impl Drop for PhysicalMonitor {
    #[expect(
        unsafe_code,
        reason = "DestroyPhysicalMonitor is the release half of GetPhysicalMonitorsFromHMONITOR"
    )]
    fn drop(&mut self) {
        // SAFETY: `handle` came from `GetPhysicalMonitorsFromHMONITOR` and has
        // not been destroyed before now — this type is the only owner and is
        // not `Clone`. Destroying it exactly once is what the API asks for.
        unsafe {
            DestroyPhysicalMonitor(self.raw());
        }
    }
}

impl VcpBackend for PhysicalMonitor {
    fn name(&self) -> String {
        self.name.clone()
    }

    #[expect(unsafe_code, reason = "the dxva2 monitor-configuration API is a C API")]
    fn get(&mut self, feature: Feature) -> Result<Value, DisplayError> {
        self.settle();
        let mut kind = 0;
        let mut current = 0_u32;
        let mut maximum = 0_u32;
        // SAFETY: the handle is live for the lifetime of `self`, and the three
        // out-parameters are stack locals that outlive the call.
        let ok = unsafe {
            GetVCPFeatureAndVCPFeatureReply(
                self.raw(),
                feature.code(),
                &raw mut kind,
                &raw mut current,
                &raw mut maximum,
            )
        };
        if ok != TRUE {
            return Err(self.failed("reading a feature"));
        }
        // The API reports both as 32-bit, but MCCS values are 16-bit and a
        // driver that returns more than that has said something impossible.
        // Saturating is the forgiving reading: a clamped maximum still lets a
        // percentage be computed, where a hard error would take the whole
        // display away over one bad field.
        Ok(Value {
            current: u16::try_from(current).unwrap_or(u16::MAX),
            maximum: u16::try_from(maximum).unwrap_or(u16::MAX),
        })
    }

    #[expect(unsafe_code, reason = "the dxva2 monitor-configuration API is a C API")]
    fn set(&mut self, feature: Feature, value: u16) -> Result<(), DisplayError> {
        self.settle();
        // SAFETY: the handle is live for the lifetime of `self`.
        let ok = unsafe { SetVCPFeature(self.raw(), feature.code(), u32::from(value)) };
        self.wrote();
        if ok != TRUE {
            return Err(self.failed("writing a feature"));
        }
        Ok(())
    }

    #[expect(unsafe_code, reason = "the dxva2 monitor-configuration API is a C API")]
    fn capabilities(&mut self) -> Result<Capabilities, DisplayError> {
        self.settle();
        let mut length = 0_u32;
        // SAFETY: the handle is live and `length` is a stack local.
        let ok = unsafe { GetCapabilitiesStringLength(self.raw(), &raw mut length) };
        if ok != TRUE {
            return Err(self.failed("asking how long the capability string is"));
        }

        // The only length in this backend that comes from outside the program:
        // the two transfer paths size their buffers from fixed arrays this
        // crate owns, while this one is whatever the driver says. A driver
        // that reports success and a length of zero would leave an empty
        // `Vec`, whose pointer is dangling — so it is refused here rather than
        // handed to the API, which is the one case where the safety comment
        // below could otherwise be vacuously true.
        if length == 0 {
            return Err(DisplayError::Capabilities {
                name: self.name.clone(),
                source: roadie_ddc::CapabilitiesError::Empty,
            });
        }
        let length = length as usize;
        let mut buffer = vec![0_u8; length];
        // SAFETY: the handle is live, and the buffer has exactly the `length`
        // bytes the previous call asked us to provide.
        let ok = unsafe {
            CapabilitiesRequestAndCapabilitiesReply(
                self.raw(),
                buffer.as_mut_ptr(),
                // The length is the driver's own answer; it cannot be wider
                // than the u32 it arrived in.
                u32::try_from(length).unwrap_or(u32::MAX),
            )
        };
        if ok != TRUE {
            return Err(self.failed("reading the capability string"));
        }

        // The string is NUL-terminated and the length includes the terminator.
        // Leaving it on would put a zero byte inside a parsed model name,
        // which reaches a screen reader as a name followed by silence.
        let text = buffer.split(|byte| *byte == 0).next().unwrap_or(&buffer);
        Capabilities::parse(text).map_err(|source| DisplayError::Capabilities {
            name: self.name.clone(),
            source,
        })
    }

    #[expect(unsafe_code, reason = "the dxva2 monitor-configuration API is a C API")]
    fn save_settings(&mut self) -> Result<(), DisplayError> {
        self.settle();
        // SAFETY: the handle is live for the lifetime of `self`.
        let ok = unsafe { SaveCurrentSettings(self.raw()) };
        self.wrote();
        if ok != TRUE {
            return Err(self.failed("saving the current settings"));
        }
        Ok(())
    }
}

/// Collects the `HMONITOR`s `EnumDisplayMonitors` calls back with.
///
/// The callback is a plain C function pointer with no captures, so the vector
/// it fills travels through the `LPARAM` the API provides for exactly this.
#[expect(
    unsafe_code,
    reason = "EnumDisplayMonitors takes a C callback and an untyped user pointer"
)]
unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _context: HDC,
    _bounds: *mut RECT,
    data: LPARAM,
) -> windows_sys::core::BOOL {
    // SAFETY: `data` is the `LPARAM` passed to `EnumDisplayMonitors` below,
    // which is a pointer to the `Vec` living on that function's stack for the
    // whole of the enumeration. The API calls this synchronously and on the
    // same thread, so the borrow cannot overlap with any other.
    let monitors = unsafe { &mut *(data as *mut Vec<HMONITOR>) };
    monitors.push(monitor);
    TRUE
}

/// Every physical monitor Windows can offer a control channel to.
///
/// Fallible in signature and infallible in fact: nothing here can fail in a
/// way worth reporting, since a display adapter with no controllable monitor
/// behind it is an ordinary state rather than an error. The `Result` is the
/// shape [`crate::enumerate`] dispatches to on all three platforms, and on
/// Linux reading the kernel's display directory genuinely can fail.
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature across three platforms; the Linux one can fail"
)]
#[expect(
    unsafe_code,
    reason = "the display enumeration and monitor-configuration APIs are C APIs"
)]
pub(crate) fn enumerate() -> Result<Vec<Display>, DisplayError> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `collect` matches the callback signature the API expects, and
    // the pointer handed to it borrows `handles`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            (&raw mut handles) as LPARAM,
        );
    }

    let mut displays = Vec::new();
    for handle in handles {
        let mut count = 0_u32;
        // SAFETY: `handle` came from the enumeration above and `count` is a
        // stack local.
        let ok = unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(handle, &raw mut count) };
        if ok != TRUE || count == 0 {
            // A display adapter with no controllable monitor behind it. Not an
            // error: laptop panels and virtual displays look exactly like this.
            continue;
        }

        let mut physical = vec![
            PHYSICAL_MONITOR {
                hPhysicalMonitor: std::ptr::null_mut(),
                szPhysicalMonitorDescription: [0; 128],
            };
            count as usize
        ];
        // SAFETY: the vector has exactly `count` elements, which is the size
        // the previous call reported.
        let ok = unsafe { GetPhysicalMonitorsFromHMONITOR(handle, count, physical.as_mut_ptr()) };
        if ok != TRUE {
            continue;
        }

        let output = handle;
        for (position, monitor) in (0_u32..).zip(physical) {
            // `PHYSICAL_MONITOR` is a packed struct, so its fields are copied
            // out before being read. Borrowing one in place would be an
            // unaligned reference, which is undefined behaviour even unread.
            let description = monitor.szPhysicalMonitorDescription;
            let handle = monitor.hPhysicalMonitor as isize;
            let edid = edid_for(output, position);
            let name = String::from_utf16_lossy(&description)
                .trim_end_matches('\0')
                .trim()
                .to_owned();
            let index = displays.len();
            let name = if name.is_empty() {
                format!("display {}", index + 1)
            } else {
                name
            };
            let backend = PhysicalMonitor {
                handle,
                name,
                pacer: Pacer::default(),
            };
            // The handle stays numbered, because `dxva2` very often calls
            // every panel on the desk the same thing; the spoken name comes
            // from the EDID whenever Windows kept one.
            displays.push(Display::new(
                DisplayId::new(format!("display-{}", index + 1)),
                edid,
                boxed(backend),
            ));
        }
    }
    Ok(displays)
}

/// NUL-terminated UTF-16, the shape every `W` call wants.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The device instance a monitor's interface path names.
///
/// `EnumDisplayDevicesW` hands back an interface path, such as
/// `\\?\DISPLAY#RTK6034#5&23865088&0&UID8448#{e6f07b5f-…}`, and the registry
/// files the EDID under the instance it is built from, with `\` for `#` and
/// the interface class dropped: `DISPLAY\RTK6034\5&23865088&0&UID8448`.
fn instance_id(interface: &str) -> Option<String> {
    let rest = interface.strip_prefix(r"\\?\")?;
    let (instance, class) = rest.rsplit_once('#')?;
    class.starts_with('{').then(|| instance.replace('#', r"\"))
}

/// The EDID of the `position`th physical monitor behind `output`, as Windows
/// recorded it when the monitor arrived. `None` when any step comes up empty:
/// a monitor without a name is still a monitor that can be driven.
#[expect(
    unsafe_code,
    reason = "the display-device and registry APIs are C APIs"
)]
fn edid_for(output: HMONITOR, position: u32) -> Option<Edid> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).ok()?;
    // SAFETY: `output` came from `EnumDisplayMonitors`, and `info` is a
    // `MONITORINFOEXW` whose `cbSize` says so, which is how the API is told
    // it may write the extended form.
    if unsafe { GetMonitorInfoW(output, (&raw mut info).cast()) } != TRUE {
        return None;
    }

    let mut device = DISPLAY_DEVICEW {
        cb: u32::try_from(size_of::<DISPLAY_DEVICEW>()).ok()?,
        ..DISPLAY_DEVICEW::default()
    };
    // SAFETY: `szDevice` is the NUL-terminated adapter name the previous
    // call wrote, and `device` carries its own size in `cb`.
    let ok = unsafe {
        EnumDisplayDevicesW(
            info.szDevice.as_ptr(),
            position,
            &raw mut device,
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
    };
    if ok != TRUE {
        return None;
    }
    let interface = String::from_utf16_lossy(&device.DeviceID);
    let instance = instance_id(interface.trim_end_matches('\0'))?;

    let key = wide(&format!(
        r"SYSTEM\CurrentControlSet\Enum\{instance}\Device Parameters"
    ));
    let value = wide("EDID");
    let mut length = 0_u32;
    // SAFETY: both strings are NUL-terminated locals; a null buffer asks
    // only for the length, which lands in a stack local.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut length,
        )
    };
    if status != ERROR_SUCCESS || length == 0 {
        return None;
    }
    let mut bytes = vec![0_u8; usize::try_from(length).ok()?];
    // SAFETY: the buffer has exactly the `length` bytes the previous call
    // asked for, and `length` tells the API so.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            bytes.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    Edid::parse(bytes.get(..usize::try_from(length).ok()?)?).ok()
}

#[cfg(test)]
mod tests {
    use super::instance_id;

    #[test]
    fn an_interface_path_becomes_the_instance_the_registry_files_its_edid_under() {
        // Literal path and instance from this desk's HG560T34, 2026-11-03.
        assert_eq!(
            instance_id(
                r"\\?\DISPLAY#RTK6034#5&23865088&0&UID8448#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(r"DISPLAY\RTK6034\5&23865088&0&UID8448")
        );
    }

    #[test]
    fn a_path_that_is_not_an_interface_path_names_nothing() {
        assert_eq!(instance_id(r"DISPLAY\RTK6034\5&23865088&0&UID8448"), None);
        assert_eq!(
            instance_id(r"\\?\DISPLAY#RTK6034#5&23865088&0&UID8448"),
            None
        );
    }
}
