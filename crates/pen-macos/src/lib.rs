//! Pen pressure on macOS (ADR 0096).
//!
//! winit 0.30 delivers a mouse event for a stylus and drops the tablet point, and egui never
//! sees trackpad force either. This crate reads that one tablet point and hands back
//! `0.0..=1.0`. The shell stays free of `unsafe`. A mouse is not a pen: turning Speed as
//! pressure off leaves a real pen alone and paints a mouse at full width.
//!
//! Hover is not a sample. A tablet point with the tip up and no pressure is ignored, so a pen
//! resting nearby does not thin a mouse stroke. The stroke is drawn on a later frame, so while
//! the tip is down the last pressure is kept. A Huion pen's force is on its HID report: the
//! driver hands the app a mouse event whose pressure is 0 or 1.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// How long a tablet point still belongs to the mouse event the system sends with it.
pub const PEN_WINDOW_SECS: f64 = 0.03;

/// `NSEventTypeTabletPoint`.
const TABLET_POINT: u64 = 23;
/// `NSEventSubtypeTabletPoint` on a mouse event. The same number means something else on other events.
const SUBTYPE_TABLET_POINT: i16 = 1;

static HAVE: AtomicBool = AtomicBool::new(false);
static BITS: AtomicU32 = AtomicU32::new(0);
static AT: AtomicU64 = AtomicU64::new(0);
/// The tip is down. The frame is drawn after the mouse event, so the pressure has to outlive
/// the 30 ms window until the button comes up.
static TIP_DOWN: AtomicBool = AtomicBool::new(false);

/// How long a tip-down reading is kept while the frame catches up.
const HOLD_SECS: f64 = 2.0;

fn mono() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// Remember a tip-down tablet point. `at` is seconds on the same clock as the read.
fn remember(pressure: f32, at: f64) {
    if !(pressure.is_finite() && at.is_finite()) {
        return;
    }
    BITS.store(pressure.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    AT.store(at.to_bits(), Ordering::Relaxed);
    HAVE.store(true, Ordering::Relaxed);
}

/// The tablet point remembered just now, for the frame that draws it.
fn live() -> Option<f32> {
    pressure_as_of(mono())
}

fn tip_down() -> bool {
    TIP_DOWN.load(Ordering::Relaxed)
}

/// The stored reading, when there is one: pressure and the time it was taken.
fn stored() -> Option<(f32, f64)> {
    if !HAVE.load(Ordering::Relaxed) {
        return None;
    }
    let p = f32::from_bits(BITS.load(Ordering::Relaxed));
    let at = f64::from_bits(AT.load(Ordering::Relaxed));
    if p.is_finite() && at.is_finite() { Some((p, at)) } else { None }
}

fn set_tip_down(down: bool) {
    TIP_DOWN.store(down, Ordering::Relaxed);
}

/// The tablet point at `now`. Fresh for [`PEN_WINDOW_SECS`], and for [`HOLD_SECS`] while the
/// tip is still down, because the stroke is drawn on a later frame.
pub fn pressure_as_of(now: f64) -> Option<f32> {
    if !HAVE.load(Ordering::Relaxed) {
        return None;
    }
    let at = f64::from_bits(AT.load(Ordering::Relaxed));
    if !(at.is_finite() && now.is_finite()) || now + 1e-3 < at {
        return None;
    }
    let age = now - at;
    let keep = age <= PEN_WINDOW_SECS || (tip_down() && age <= HOLD_SECS);
    if !keep {
        return None;
    }
    let p = f32::from_bits(BITS.load(Ordering::Relaxed));
    if p.is_finite() { Some(p.clamp(0.0, 1.0)) } else { None }
}

fn is_up(kind: u64) -> bool {
    // Left, right and other mouse up.
    matches!(kind, 2 | 4 | 26)
}

fn is_drag(kind: u64) -> bool {
    matches!(kind, 6 | 7 | 27)
}

/// The candidate that is actually a force.
///
/// A mouse button is 0 or 1, and a missing tablet field is 0. A stylus in the middle of its
/// range is the one to keep. When every candidate sits on an end, 1 wins over 0, so a mouse
/// stays a mouse.
pub fn prefer_pressure(candidates: &[f32]) -> f32 {
    let mut end = 1.0;
    let mut saw = false;
    for &raw in candidates {
        if !raw.is_finite() {
            continue;
        }
        let p = raw.clamp(0.0, 1.0);
        if p > 0.02 && p < 0.98 {
            return p;
        }
        if !saw || (p - 1.0).abs() <= 0.02 {
            end = p;
            saw = true;
        }
    }
    end
}

/// Whether `pressure` may replace the reading stored at `prev` (`(pressure, time)`).
///
/// `authoritative` is an `NSEventTypeTabletPoint`. A mouse event that says 0 or 1 in the same
/// instant is the copy the system sends beside the pen, and it must not erase a real force.
pub fn accept_sample(authoritative: bool, pressure: f32, prev: Option<(f32, f64)>, now: f64) -> bool {
    if authoritative {
        return true;
    }
    let Some((prev_p, prev_at)) = prev else {
        return true;
    };
    if !(prev_at.is_finite() && now.is_finite() && pressure.is_finite()) {
        return false;
    }
    let fresh = now >= prev_at && now - prev_at <= PEN_WINDOW_SECS;
    let incoming_end = pressure <= 0.02 || pressure >= 0.98;
    let prev_mid = prev_p > 0.02 && prev_p < 0.98;
    !(fresh && incoming_end && prev_mid)
}

/// What one event contributes. `(tip still down, pressure to remember)`.
///
/// A mouse button is 0 or 1 and is not a pen. A pressure between those ends is, even when the
/// driver leaves off the tablet subtype. Once the tip is down, 0 or 1 on the drag is a light
/// or a hard press and is kept.
pub fn reported(down: bool, kind: u64, subtype: i16, tip: bool, pressure: f32) -> (bool, Option<f32>) {
    if is_up(kind) {
        return (false, classify(kind, subtype, tip, pressure));
    }
    if let Some(p) = classify(kind, subtype, tip, pressure) {
        return (true, Some(p));
    }
    if is_mouse(kind) && pressure.is_finite() {
        let p = pressure.clamp(0.0, 1.0);
        let mid = p > 0.02 && p < 0.98;
        if mid || (down && is_drag(kind)) {
            return (true, Some(p));
        }
    }
    (down, None)
}

/// Pressure from one AppKit event, or nothing when it is a mouse, a hover, or not finite.
///
/// `kind` is `NSEventType`, `subtype` is `NSEventSubtype`, `tip_down` is the pen-tip button
/// (tablet points only; pass false for a mouse event).
pub fn classify(kind: u64, subtype: i16, tip_down: bool, pressure: f32) -> Option<f32> {
    if !pressure.is_finite() {
        return None;
    }
    let p = pressure.clamp(0.0, 1.0);
    if kind == TABLET_POINT {
        // Hover reports pressure 0 with the tip up. A light touch is either the tip button or
        // a pressure above none, including a driver that leaves the button mask empty.
        if tip_down || p > 0.0 { Some(p) } else { None }
    } else if is_mouse(kind) && subtype == SUBTYPE_TABLET_POINT {
        Some(p)
    } else {
        None
    }
}

fn is_mouse(kind: u64) -> bool {
    // Left/right/other mouse down, up, and dragged. Not moved, not entered.
    matches!(kind, 1 | 2 | 3 | 4 | 6 | 7 | 25 | 26 | 27)
}

mod hid;

/// Start listening. Safe to call more than once. No-op off the main thread and off macOS.
pub fn install() {
    #[cfg(target_os = "macos")]
    {
        imp::install();
        hid::install();
    }
}

/// The pen's pressure for the event being handled, when it is a stylus.
pub fn current() -> Option<f32> {
    #[cfg(target_os = "macos")]
    {
        imp::current()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{accept_sample, live, prefer_pressure, remember};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventButtonMask, NSEventMask, NSEventType};
    use objc2_core_graphics::{CGEvent, CGEventField};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub fn install() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::Relaxed) {
            return;
        }
        if MainThreadMarker::new().is_none() {
            STARTED.store(false, Ordering::Relaxed);
            return;
        }
        let block = block2::RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // A throw from a tablet field must not leave this callback. Rust cannot unwind through it.
            let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
                // SAFETY: AppKit calls the monitor with the event it is dispatching, alive for this call.
                let ev = unsafe { event.as_ref() };
                note(ev);
            }));
            event.as_ptr()
        });
        let mask = NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::LeftMouseDragged
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseUp
            | NSEventMask::RightMouseDragged
            | NSEventMask::OtherMouseDown
            | NSEventMask::OtherMouseUp
            | NSEventMask::OtherMouseDragged
            | NSEventMask::TabletPoint;
        // SAFETY: the block returns the same event pointer it was given, so delivery is unchanged.
        // AppKit removes the monitor if the token is dropped, so both live for the process.
        let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
        std::mem::forget(block);
        std::mem::forget(token);
    }

    pub fn current() -> Option<f32> {
        install();
        super::hid::install();
        // The Huion pen's force is not on the mouse event. Its HID report wins while the tip is down.
        if let Some(p) = super::hid::fresh(super::mono()) {
            return Some(p);
        }
        // The monitor already stored the drag. Reading `currentEvent` again during a redraw
        // sees a mouse copy at 0 or 1 and was wiping that reading (ADR 0102).
        live()
    }

    fn note(event: &NSEvent) {
        let _ = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
            let ty = event.r#type();
            let kind = ty.0 as u64;
            // A key or a scroll has no pressure field. Reading it aborts the process.
            if ty != NSEventType::TabletPoint && !super::is_mouse(kind) {
                return;
            }
            // `subtype` is for mouse events. On a tablet point the same call can raise.
            let mouse = super::is_mouse(kind);
            let mut subtype = if mouse { event.subtype().0 } else { 0 };
            let tip = ty == NSEventType::TabletPoint && event.buttonMask().contains(NSEventButtonMask::PenTip);
            let ns_pressure = event.pressure();
            let field = tablet_field(event);
            if super::hid::tracing_events() && matches!(kind, 1 | 6 | 23) {
                let cg = field.map(|(p, sub)| format!("{p:.3} tablet={sub}")).unwrap_or_else(|| "-".to_string());
                super::hid::trace_event(&format!("ns kind={kind} sub={subtype} ns={ns_pressure:.3} cg={cg}"));
            }
            let mut pressure = ns_pressure;
            if let Some((p, sub)) = field {
                pressure = prefer_pressure(&[pressure, p]);
                if sub {
                    subtype = super::SUBTYPE_TABLET_POINT;
                }
            }
            let (down, sample) = super::reported(super::tip_down(), kind, subtype, tip, pressure);
            super::set_tip_down(down);
            if let Some(p) = sample {
                let now = super::mono();
                let prev = super::stored();
                // Only the tablet-point event itself may replace a real force with 0 or 1.
                // The mouse copy sent beside it is often full pressure.
                if accept_sample(ty == NSEventType::TabletPoint, p, prev, now) {
                    remember(p, now);
                }
            }
        }));
    }

    /// CoreGraphics tablet pressure, when the mouse event's own pressure is 0 or 1.
    /// A throw here leaves the AppKit reading in place.
    fn tablet_field(event: &NSEvent) -> Option<(f32, bool)> {
        let read = objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
            let cg = event.CGEvent()?;
            let tablet = CGEvent::double_value_field(Some(&cg), CGEventField::TabletEventPointPressure);
            let mouse = CGEvent::double_value_field(Some(&cg), CGEventField::MouseEventPressure);
            let sub = CGEvent::integer_value_field(Some(&cg), CGEventField::MouseEventSubtype);
            let p = prefer_pressure(&[tablet as f32, mouse as f32]);
            Some((p, sub == i64::from(super::SUBTYPE_TABLET_POINT)))
        }));
        read.ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pen_is_a_tablet_point_and_a_mouse_is_not() {
        assert_eq!(classify(TABLET_POINT, 0, true, 0.0), Some(0.0), "tip down, no force: thinnest");
        assert_eq!(classify(TABLET_POINT, 0, true, 0.12), Some(0.12));
        assert_eq!(classify(TABLET_POINT, 0, false, 0.2), Some(0.2), "a driver that omits the button");
        assert_eq!(classify(TABLET_POINT, 0, false, 0.0), None, "hover");
        assert_eq!(classify(TABLET_POINT, 0, true, f32::NAN), None);
        assert_eq!(classify(TABLET_POINT, 0, true, 2.0), Some(1.0));
        // Left mouse dragged, carrying a tablet point.
        assert_eq!(classify(6, SUBTYPE_TABLET_POINT, false, 0.05), Some(0.05));
        // The same drag as an ordinary mouse, even if something left a pressure in the event.
        assert_eq!(classify(6, 0, false, 0.4), None);
        assert_eq!(classify(1, 0, false, 1.0), None, "mouse down is full width, not a pen");
        assert_eq!(classify(10, SUBTYPE_TABLET_POINT, false, 0.5), None, "a key is not a pen");
    }

    #[test]
    fn a_tablet_point_covers_the_mouse_copy_and_then_expires() {
        set_tip_down(false);
        remember(0.12, 10.0);
        assert_eq!(pressure_as_of(10.02), Some(0.12));
        assert_eq!(pressure_as_of(10.0 + PEN_WINDOW_SECS + 0.01), None);
        set_tip_down(true);
        assert_eq!(pressure_as_of(10.4), Some(0.12), "the frame is drawn while the tip is still down");
        set_tip_down(false);
        assert_eq!(pressure_as_of(10.4), None);
        remember(0.0, 20.0);
        assert_eq!(pressure_as_of(20.0), Some(0.0), "a light touch is a real sample");
    }

    #[test]
    fn a_drag_carries_the_pen_when_no_tablet_event_arrives() {
        // Left mouse dragged, subtype tablet point.
        assert_eq!(reported(false, 6, SUBTYPE_TABLET_POINT, false, 0.33), (true, Some(0.33)));
        // The same drag from a driver that omits the subtype, lighter than a mouse button.
        assert_eq!(reported(false, 6, 0, false, 0.33), (true, Some(0.33)));
        // A mouse with no sensor reports 0 or 1. Neither is a pen.
        assert_eq!(reported(false, 6, 0, false, 0.0), (false, None));
        assert_eq!(reported(false, 6, 0, false, 1.0), (false, None));
        // Once that pen is down, a hard press and a light drag are real samples.
        assert_eq!(reported(true, 6, 0, false, 1.0), (true, Some(1.0)));
        assert_eq!(reported(true, 6, 0, false, 0.0), (true, Some(0.0)));
        // Left mouse up puts the tip up.
        assert_eq!(reported(true, 2, 0, false, 1.0).0, false);
    }

    #[test]
    fn a_mouse_copy_does_not_erase_a_real_force() {
        // The tablet field in the middle of the range wins over a mouse event at full pressure.
        assert_eq!(prefer_pressure(&[1.0, 0.0, 0.33]), 0.33);
        assert_eq!(prefer_pressure(&[0.0, 1.0]), 1.0, "a mouse stays a mouse");
        assert_eq!(prefer_pressure(&[f32::NAN]), 1.0);
        // A tablet point may replace the force. The mouse copy at 1, in the same instant, may not.
        assert!(accept_sample(true, 1.0, Some((0.33, 10.0)), 10.01));
        assert!(!accept_sample(false, 1.0, Some((0.33, 10.0)), 10.01));
        assert!(!accept_sample(false, 0.0, Some((0.33, 10.0)), 10.01));
        assert!(accept_sample(false, 0.5, Some((0.33, 10.0)), 10.01), "a real change on the drag is kept");
        assert!(accept_sample(false, 1.0, Some((0.33, 10.0)), 10.0 + PEN_WINDOW_SECS + 0.01), "a later hard press is real");
    }
}
