# ADR 0098: Turn the canvas

* Status: accepted (2026-11-16)
* Relates to: the stage view (ADR 0007), rulers (ADR 0076), grab/rotate/scale (ADR 0101)

## Context

Drawing a long stroke with the wrist parked in one place is easier when the paper can turn under
the hand. The turn is Shift+Space and a drag, about the middle of the window. Space alone still
slides the paper. The drawing itself does not turn. A later export, a tween and a symbol still
see the artwork upright.

Shift+R already turns the selection (ADR 0101). It does not stand the view back up. View ›
Reset Rotation does that.

The pan is a slide in screen pixels: the hand tool, the space bar and the wheel move the picture
by the same distance the pointer moved, whichever way the paper is turned. Turning must not jump
the point that sits under the middle of the window, and standing the view back up must not jump
it either, and must not change the zoom.

Ctrl+Space and a drag zooms the same view. On macOS the chord is Command+Space. The point under
the press stays, and dragging back to the press height restores the zoom. It is not a document
edit.

## Decision

1. **The turn is view state.** `StageView::rotation` is clockwise degrees. It is not a document
   command, it is not the camera's rotation, and it is not stored in the file.
2. **Pan stays outside the turn.** A hand drag adds screen pixels to the pan. A turn orbits that
   pan by the same angle, so the stage point under the canvas centre stays. Reset Rotation
   orbits the pan back and sets the angle to 0. Fit in Window stands the view upright and clears
   the pan, as a fitted view always has.
3. **Shift+Space and a drag** starts the turn, with the pen or the mouse, whichever tool is in
   hand. It does not start a stroke, a bucket or a pen knot, and it does not pan. Space alone
   still pans. The middle button still pans, and it wins over the turn. The Hand tool's drag
   pans, unless Shift is held with Space, which turns instead. A text field keeps the keys.
   Shift+R still rotates the selection. R is still the Rectangle tool.
4. **Ctrl+Space and a drag zooms the view.** On macOS that chord is Command+Space. Dragging up
   zooms in and dragging down zooms out. The change is measured from the press, so returning
   to that height restores the zoom. The stage point under the press stays, including while
   the view is turned. The drag does not paint, pan or turn. Shift+Space still turns. Space
   alone still pans. The middle button still pans, and it wins over the zoom. A text field
   keeps the keys. Command without Space is left to the tool.
5. **Upright drawing stays as it was.** At 0° the screen mapping, the tile images, the selection
   boxes, the grid and the guides are the ones the stage already drew. A turn draws tiles and
   boxes as the same corners after the turn. Ruler ticks stay on the window edges. The wheel
   still zooms around the pointer, and the stage point under that pointer stays.

## Consequences

* The zoom badge shows the angle while the view is turned.
* Ruler numbers do not follow the turned stage axes. The grid and the guides do.
* A rounded rectangle's preview is a sharp quad while the view is turned. The shape that lands
  still has its corner radius.
* Free-transform handles sit on the turned box. Their grab boxes stay aligned to the screen.
* The drag zoom uses the same limits as the other zooms, 5% to 6400%.
