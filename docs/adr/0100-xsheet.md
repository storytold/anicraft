# ADR 0100: The X-sheet is the timeline, stood up

* Status: accepted (2026-11-15)
* Relates to: ADR 0075 (docks), ADR 0065 (drops on a frame), ADR 0099 (studio chrome)

## Context

The filmstrip under the stage is already the exposure sheet: layers, keyframes, tweens
and sounds on one playhead. Tahoma2D, OpenToonz and a tracker show that same grid with
frames running down the page and one column per level. KoolDraw's timeline can turn
the same way. A second clock, or a notes grid stored beside the document, would drift
from the film the stage is playing.

## Decisions

1. **One timeline.** The X-sheet reads and edits `app.s.frame`, the layer list and the
   keyframes the horizontal timeline already uses. Insert, cut, paste, tweens and the
   Motion Editor are that timeline's commands. The sheet does not invent a frame
   format.
2. **It is a panel.** `xsheet` docks with the Library in the right-hand group of
   Essentials, Animator and Designer, and opens there the first time a workspace saved
   before it existed is loaded. Closing it keeps it closed. The filmstrip stays where
   it is.
3. **Audio is a sound on the cell.** Right-click places or removes a Library sound
   with `sound.place` and `sound.remove`. A file dropped on a column lands on that
   layer and frame, the same route as a drop on the filmstrip, and only when the
   pointer is actually over a column.

## Consequences

* Arrow keys while the pointer is over the sheet step a frame (up, down) or a layer
  (left, right). Over the filmstrip they keep their old meaning.
* Storyboard shots and saving into a 2kool.tv account are still separate work.
