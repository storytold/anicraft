# ADR 0103: A stroke's fill keeps its alpha

* Status: accepted (2026-11-16)
* Relates to: ADR 0074 (the fill picker), ADR 0097 (fill and stroke alpha, reserved by #354)

## Context

The document already stores an alpha on a solid colour and on every gradient stop, and the
renderer already paints it. The Studio branch was cut from main before that control was on a
picker. The Fill picker in Properties showed a six-digit hex and threw the alpha away, so a
brush stroke could not be a wash. ADR 0097 records the same decision on its own branch. This
branch carries that behaviour without taking that number.

## Decisions

1. **A solid fill and a stroke carry alpha.** The colour picker (Tools squares, the Colour
   panel, Properties) shows a labeled Alpha row: a slider and a percent, 0% clear, 100%
   opaque. Hex may be `#rrggbb` or `#rrggbbaa`. Choosing a swatch keeps the alpha you had.
   None is still no fill, not a clear one.
2. **Each gradient stop keeps its own alpha.** The stop's marker is drawn at that alpha, and
   its name says the percent.
3. **A brush stroke that is not fully opaque stays its own shape, and you see that alpha
   while the pen is down.** Opaque ink still joins. A wash composites over what is behind it,
   and two washes of one colour stay separate so the crossing darkens. The ink drawn before
   the pen lifts is already that alpha.

## Consequences

* An older file with no alpha byte stays opaque.
* Alpha 0 is a fill the hit test and the eraser can still find. Removing the fill is None.
* A wash does not join the raw drawing. An opaque stroke joins as before.
