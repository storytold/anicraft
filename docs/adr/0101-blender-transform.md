# ADR 0101: Grab, rotate and scale from the keyboard

* Status: accepted (2026-11-15)
* Relates to: ADR 0031 (transforms), ADR 0040 (interface text)

## Context

People who already work in Blender reach for G to move, R to turn and S to size, then type a
number and press Enter. AniCraft already moves a selection with the Selection tool and turns
and sizes it with Free Transform (`Q`). Those keys are not all free: R is the Rectangle tool
and S is the Ink Bottle. G is free. Shift+R and Shift+S are free.

## Decisions

1. **The same transform.** G, R and S start a modal on the current selection. Grab stays on
   Selection or Free Transform, and switches to Selection otherwise. Rotate and scale switch
   to Free Transform. The artwork moves by `modify.transform` (or `frames.transform` when
   Edit Multiple Frames is on), about the centre of the selection as it was when the modal
   started. Confirm is one undo step. Esc or a right-click restores the gesture and records
   nothing.
2. **A taken key keeps its job.** The chord is the letter alone when the live keymap has
   nothing on it, and Shift held with that letter when the letter is taken. If both are
   taken, the transform does not steal either. On the default map that is G, Shift+R and
   Shift+S. While the modal is open the letters switch grab, rotate and scale, and the
   tools those letters usually are do not fire.
3. **A typed number is exact.** Digits, one decimal point and a leading minus. Enter or a
   click confirms. For a turn, the number is degrees counterclockwise on the screen.
   `Affine::rotate` is clockwise on this y-down stage, so a typed positive angle is stored
   with the sign flipped. A typed move is stage pixels, and positive Y is down, the same
   direction as the pointer and the property fields. A typed scale is a factor about the
   centre (negative mirrors). X or Y locks that axis for a move or a scale. The Free
   Transform drag still follows the pointer with its own sign; this modal does not change
   that drag.

## Consequences

* Nothing selected, camera preview, or a drag already open: the modal does not start.
* The status line is the readout (distance, degrees or factor) until confirm or cancel.
