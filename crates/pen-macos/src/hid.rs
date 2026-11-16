//! Tip pressure from a Huion (UC-Logic) pen.
//!
//! The Huion driver posts a mouse event for the stylus. That event's pressure is 0 or 1, so the
//! brush paints at full width. The pen's own HID report carries the force. Vendor 0x256C,
//! report 8 is the driver packet (pressure at bytes 6..8) and report 10 is the standard digitizer
//! packet. Both run to 16383: this tablet's report 8 goes past 8191 on a normal press.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Huion / Gaomon / UC-Logic.
const HUION_VENDOR: i32 = 9580;
/// A report newer than this still belongs to a stroke. The pen sends much faster while it is down.
const FRESH_SECS: f64 = 0.1;

static TIP: AtomicBool = AtomicBool::new(false);
static HAVE: AtomicBool = AtomicBool::new(false);
static BITS: AtomicU32 = AtomicU32::new(0);
static AT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub tip_down: bool,
    pub pressure: f32,
}

/// Pressure from one HID report, or nothing when the packet is not a pen.
///
/// `report_id` is the HID id. `bytes` may or may not repeat that id as its first byte.
/// A zero id means the id is only in the first byte.
pub fn parse_report(report_id: u32, bytes: &[u8]) -> Option<PenSample> {
    let (report_id, payload) = match bytes.first().copied() {
        Some(b) if report_id == 0 && (b == 0x08 || b == 0x0A) && bytes.len() > 1 => (u32::from(b), &bytes[1..]),
        Some(b) if u32::from(b) == report_id && (report_id == 0x08 || report_id == 0x0A) && bytes.len() > 1 => (report_id, &bytes[1..]),
        _ => (report_id, bytes),
    };
    // Need the flag byte plus X, Y and a 16-bit pressure.
    if payload.len() < 7 {
        return None;
    }
    let flags = payload[0];
    let raw = u16::from_le_bytes([payload[5], payload[6]]);
    let max = match report_id {
        // Vendor packet. Pressure sits at bytes 6..8. This tablet's presses go past 8191,
        // so the full scale is the same 16383 as the digitizer report.
        0x08 => 16383u16,
        // Standard digitizer packet from this tablet's report descriptor.
        0x0A => 16383u16,
        _ => return None,
    };
    // On the vendor packet, bit 6 with no force is the button pad. On the digitizer packet
    // that same bit means the pen is in range.
    if report_id == 0x08 && flags & 0x40 != 0 && raw == 0 {
        return None;
    }
    let pressure = (f32::from(raw) / f32::from(max)).clamp(0.0, 1.0);
    // The tip switch is bit 0 on the digitizer packet and bit 1 on the vendor packet.
    // A non-zero force is a touch either way.
    let tip_down = raw > 0 || flags & 0x01 != 0 || flags & 0x02 != 0;
    Some(PenSample { tip_down, pressure })
}

fn store(sample: PenSample) {
    // Hover is ignored. A tip-up packet from the other interface must not cancel a stroke,
    // and a stale tip ages out through the timestamp in `fresh`.
    if !sample.tip_down || !sample.pressure.is_finite() {
        return;
    }
    TIP.store(true, Ordering::Relaxed);
    BITS.store(sample.pressure.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    AT.store(super::mono().to_bits(), Ordering::Relaxed);
    HAVE.store(true, Ordering::Relaxed);
}

/// The pen's pressure when its tip is down and a report has just arrived.
pub fn fresh(now: f64) -> Option<f32> {
    if !HAVE.load(Ordering::Relaxed) || !TIP.load(Ordering::Relaxed) {
        return None;
    }
    let at = f64::from_bits(AT.load(Ordering::Relaxed));
    if !(at.is_finite() && now.is_finite()) || now + 1e-3 < at || now - at > FRESH_SECS {
        return None;
    }
    let p = f32::from_bits(BITS.load(Ordering::Relaxed));
    if p.is_finite() { Some(p.clamp(0.0, 1.0)) } else { None }
}

/// First lines of a live stroke, so a wrong byte offset can be read back. Not a feature.
pub(crate) fn trace(line: &str) {
    write_log(0, 32, line);
}

/// Whether another AppKit event line would be kept. Callers format only while this is true.
pub(crate) fn tracing_events() -> bool {
    EVENT_N.load(Ordering::Relaxed) < 40
}

pub(crate) fn trace_event(line: &str) {
    write_log(1, 40, line);
}

static EVENT_N: AtomicU32 = AtomicU32::new(0);

fn write_log(slot: u32, cap: u32, line: &str) {
    use std::io::Write;
    static HID_N: AtomicU32 = AtomicU32::new(0);
    static CLEARED: AtomicBool = AtomicBool::new(false);
    if !CLEARED.swap(true, Ordering::Relaxed) {
        let _ = std::fs::File::create("/tmp/anicraft-pen.log");
    }
    let n = if slot == 0 { &HID_N } else { &EVENT_N };
    if n.fetch_add(1, Ordering::Relaxed) >= cap {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/anicraft-pen.log") {
        let _ = writeln!(f, "{line}");
    }
}

/// Start listening for the Huion pen. Safe to call more than once. No-op off macOS.
pub fn install() {
    #[cfg(target_os = "macos")]
    imp::install();
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{parse_report, store, trace, HUION_VENDOR};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    const CF_NUMBER_INT32: i32 = 3;
    const CF_STRING_UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    struct DictCbs {
        _private: [u8; 0],
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(alloc: *const c_void, c: *const i8, encoding: u32) -> *const c_void;
        fn CFNumberCreate(alloc: *const c_void, kind: i32, value: *const c_void) -> *const c_void;
        fn CFDictionaryCreate(
            alloc: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            n: isize,
            key_cbs: *const DictCbs,
            value_cbs: *const DictCbs,
        ) -> *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        static kCFTypeDictionaryKeyCallBacks: DictCbs;
        static kCFTypeDictionaryValueCallBacks: DictCbs;
        static kCFRunLoopCommonModes: *const c_void;
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDManagerCreate(alloc: *const c_void, options: u32) -> *mut c_void;
        fn IOHIDManagerSetDeviceMatching(manager: *mut c_void, matching: *const c_void);
        fn IOHIDManagerRegisterDeviceMatchingCallback(
            manager: *mut c_void,
            callback: unsafe extern "C" fn(*mut c_void, i32, *mut c_void, *mut c_void),
            context: *mut c_void,
        );
        fn IOHIDManagerRegisterInputReportCallback(
            manager: *mut c_void,
            callback: unsafe extern "C" fn(*mut c_void, i32, *mut c_void, i32, u32, *mut u8, isize),
            context: *mut c_void,
        );
        fn IOHIDManagerScheduleWithRunLoop(manager: *mut c_void, run_loop: *mut c_void, mode: *const c_void);
        fn IOHIDManagerOpen(manager: *mut c_void, options: u32) -> i32;
    }

    pub fn install() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if STARTED.load(Ordering::Relaxed) {
            return;
        }
        let now = super::super::mono();
        let next = f64::from_bits(NEXT.load(Ordering::Relaxed));
        if next.is_finite() && now < next {
            return;
        }
        if STARTED.swap(true, Ordering::Relaxed) {
            return;
        }
        // SAFETY: the manager, the matching dictionary and the callbacks live for the process.
        // The report pointer is only read for this call. The device is not seized, so the
        // Huion driver keeps the cursor. Common modes include the drag-tracking mode, so
        // reports keep arriving while the tip is down.
        let code = unsafe { start() };
        if code != 0 {
            STARTED.store(false, Ordering::Relaxed);
            NEXT.store((now + 2.0).to_bits(), Ordering::Relaxed);
            trace(&format!("hid open failed {code:#x}"));
        } else {
            trace("hid open 0");
        }
    }

    unsafe fn start() -> i32 {
        unsafe {
            let manager = IOHIDManagerCreate(std::ptr::null(), 0);
            if manager.is_null() {
                return -1;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), c"VendorID".as_ptr(), CF_STRING_UTF8);
            let vendor = HUION_VENDOR;
            let value = CFNumberCreate(std::ptr::null(), CF_NUMBER_INT32, (&raw const vendor).cast());
            let keys = [key];
            let values = [value];
            let matching = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );
            IOHIDManagerSetDeviceMatching(manager, matching);
            IOHIDManagerRegisterDeviceMatchingCallback(manager, on_device, std::ptr::null_mut());
            IOHIDManagerRegisterInputReportCallback(manager, on_report, std::ptr::null_mut());
            IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), kCFRunLoopCommonModes);
            // The manager is not released: dropping the CoreFoundation object would stop the reports.
            IOHIDManagerOpen(manager, 0)
        }
    }

    unsafe extern "C" fn on_device(_ctx: *mut c_void, result: i32, _sender: *mut c_void, _device: *mut c_void) {
        trace(&format!("hid device {result:#x}"));
    }

    unsafe extern "C" fn on_report(
        _ctx: *mut c_void,
        result: i32,
        _sender: *mut c_void,
        _kind: i32,
        report_id: u32,
        report: *mut u8,
        length: isize,
    ) {
        unsafe {
            if result != 0 || report.is_null() || length <= 0 || length > 64 {
                return;
            }
            let bytes = std::slice::from_raw_parts(report, length as usize);
            let hex: String = bytes.iter().take(12).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            if let Some(sample) = parse_report(report_id, bytes) {
                trace(&format!("hid id={report_id:#x} tip={} p={:.3} {hex}", sample.tip_down, sample.pressure));
                store(sample);
            } else {
                trace(&format!("hid id={report_id:#x} unused {hex}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digitizer_packet_carries_the_tip_force() {
        // Report 10, id repeated: tip bit, pressure 8192 of 16383.
        let bytes = [0x0A, 0x41, 0x00, 0x10, 0x00, 0x20, 0x00, 0x20, 0x00, 0x00];
        let s = parse_report(0x0A, &bytes).unwrap_or(PenSample { tip_down: false, pressure: -1.0 });
        assert!(s.tip_down, "pen packet");
        assert!((s.pressure - 8192.0 / 16383.0).abs() < 0.001, "{}", s.pressure);
        // The same packet without the id byte in front.
        let s = parse_report(0x0A, &bytes[1..]).unwrap_or(PenSample { tip_down: false, pressure: -1.0 });
        assert!((s.pressure - 8192.0 / 16383.0).abs() < 0.001);
        // Hover: in range, no force.
        let hover = parse_report(0x0A, &[0x40, 0, 0, 0, 0, 0, 0]).unwrap_or(PenSample { tip_down: true, pressure: -1.0 });
        assert!(!hover.tip_down, "hover");
    }

    #[test]
    fn a_vendor_packet_carries_the_tip_force_and_a_pad_packet_does_not() {
        // Report 8: flags, X, Y, pressure 4096 of 16383.
        let bytes = [0x08, 0x80, 0x10, 0x00, 0x20, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00];
        let s = parse_report(0x08, &bytes).unwrap_or(PenSample { tip_down: false, pressure: -1.0 });
        assert!(s.tip_down);
        assert!((s.pressure - 4096.0 / 16383.0).abs() < 0.001, "{}", s.pressure);
        // Bit 6 set and no force: the button pad.
        assert!(parse_report(0x08, &[0x40, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(parse_report(0x01, &[0x08, 0x80, 0, 0, 0, 0, 0x00, 0x10]).is_none());
        // The id is only in the buffer when the callback reports id 0.
        let s = parse_report(0, &bytes).unwrap_or(PenSample { tip_down: false, pressure: -1.0 });
        assert!((s.pressure - 4096.0 / 16383.0).abs() < 0.001);
    }
}
