# ADR 0102: Pen pressure is read in one place

* Status: accepted (2026-11-15)
* Relates to: ADR 0068 (brush pressure), ADR 0096 (macOS tablet point, reserved by #336)

## Context

The brush already has the settings: Size and Opacity follow a pressure curve, and Speed as
pressure stands in for a mouse. On macOS a stylus arrives as a mouse event. winit drops the
tablet point, so those settings stay at full width. The X-sheet, the docks and grab, rotate
and scale all handle the pointer and the keyboard. A second reader in any of them would drift
from this one.

## Decisions

1. **One conduit.** macOS pressure comes from `crates/pen-macos`, the same reader as ADR 0096.
   Only `src/platform.rs` calls it. The shell stays free of `unsafe`. The crate does not take
   the workspace lints, because those forbid `unsafe` and that forbid cannot be inherited and
   then overridden. It allows `unsafe` for the two AppKit monitor sites.
2. **The brush asks once.** `frame_pressure` reads the platform first, then an egui touch
   force. Nothing else in the shell reads a tablet event.
3. **A pen stroke stays a pen stroke.** The first finite reading owns the stroke. A later
   frame with no tablet point keeps the last reading. Speed as pressure fills in only while
   no pen has arrived. A light touch, including pressure 0 with the tip down, is a sample.
   Hover is not.
4. **The reading outlives the event.** The stroke is drawn on a later frame, and that frame
   can be more than 30 ms after the mouse event (a GPU wait is enough). While the tip is
   down the last pressure is kept, for up to 2 seconds. Mouse up clears the tip. A mouse
   drag whose pressure sits between 0 and 1 is a pen even when the driver omits the tablet
   subtype. A mouse that reports 0 or 1 is not. `NSEvent.pressure` of 0 or 1 does not hide
   a CoreGraphics tablet pressure in between, and that 0 or 1 does not replace a real force
   from the same instant. Only an `NSEventTypeTabletPoint` may.
5. **A Huion pen is read from its HID report.** The Huion driver (vendor 0x256C) posts a
   mouse event whose pressure is 0 or 1. The pen's report carries the tip force: report 8
   at bytes 6..8 and report 10, both on a scale of 16383. A normal press on this tablet
   already goes past 8191, and that ceiling was painting the rest of the stroke at full
   width. While that tip is down, this reading is the one the brush uses. A redraw does
   not consult `currentEvent` again: that event is the mouse copy, and its pressure of 0
   or 1 was replacing the pen. A mouse is unchanged when the pen is up. The IOKit calls
   live in `crates/pen-macos`, next to the AppKit monitor. The shell still has no `unsafe`.
6. **Thinnest is a width, and it goes to 0.001.** The brush size and the width at no
   pressure both start at 0.001 stage pixels. A light touch is that wide. Full pressure
   is the Size. The old floor was a tenth of the size, and the size control stopped at 1.

## Consequences

* Theme, keymap, docks and the transform keys cannot clear or replace a pen sample.
* A mouse is unchanged: no pen means full width, or speed when that option is on.
