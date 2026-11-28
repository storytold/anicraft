# ADR 0136: The third issue batch: what merged, what is held, and three decisions it took

* Status: accepted (2026-11-27)
* Relates to: ADR 0016 (motion tweens; this changes its point 7), ADR 0004 (confirmation
  tiers), ADR 0031 and ADR 0122 (raw shapes merge), ADR 0046 (stroke joins), ADR 0074 (the
  fill picker), ADR 0125 (a slot for every pull request), ADR 0129 and ADR 0132 (work for a
  frame stays bounded), backlog P-018, P-020, P-057 and P-058

## Context

The third pass over the open issues (#506 and up) found sixteen outside pull requests opened
against them while it ran. Fifteen were reviewed and merged with their authors' commits, each
with fixes on top; one is held. Most of that is ordinary and is in the changelog and in the
roadmap's pull request table. Four things in it are decisions, because they are hard to
reverse or because they overrule something written down.

## Decision

### 1. Create Motion Tween asks for the symbol, in the window only (changes ADR 0016, point 7)

ADR 0016 said that several objects on a key become one graphic symbol, that "Animate asks; we
do it and say so", and that a single raw shape is tweened as it is. Two of those three did not
hold up:

* **We never said so.** The command returned `converted`, and nothing showed it. An artist
  found a symbol in the Library they had not asked for (#604).
* **A raw shape can't be a tween's target for long.** A motion tween follows one element by
  its id. A raw shape's id does not survive a merge: drawing across it, or deselecting a
  floating selection over it, replaces it with new pieces (ADR 0031, ADR 0122), and the span
  then tweens nothing.

So, with outside PR #618:

* Raw artwork, a bitmap, a group, or several objects are converted to one symbol first. One
  symbol instance, one drawing object or one text block is tweened as it is: those keep their
  identity. (The reference app converts drawing objects too; we have no reason to.)
* **In the window** the conversion is offered in a small window that says a motion tween
  animates a symbol and asks for the symbol's type, with Convert and Cancel, Enter and Esc.
  This is not a confirmation in ADR 0004's sense: nothing is lost and one undo takes it back.
  It is Convert to Symbol's question, asked because the answer (a movie clip, a button or a
  graphic in the Library) is the artist's to give. The symbol is named "Tween n" and can be
  renamed in the Library. The window is not shown when nothing needs converting.
* **The command never asks.** `timeline.create_motion_tween` converts, takes an optional
  `kind`, and answers with the symbol it made and its name and type, so the CLI and MCP can
  say so. It needs no `confirm`.
* The type offered first differs: the window offers what Convert to Symbol last used, the
  command makes a graphic symbol unless told otherwise, as it always did.

### 2. A classic tween of raw artwork stays allowed

#603 asks that a classic tween be refused on raw, ungrouped shapes, as the reference app
turns them into symbols first. Here a classic tween matches the objects of two keyframes and
moves what matches; raw shapes that match are moved like anything else, and those that don't
are held. Nothing is lost or silently wrong, and refusing it would take away something that
works. The difference from the reference app is deliberate. A raw shape that should change
its outline wants a shape tween, which the guide says.

### 3. `paint.set_fill` with no type keeps each fill's own kind

`paint.set_fill` took the kind of fill from the tool's swatch when `type` was left out, and
then gave that kind to every selected shape. With a solid swatch and a radial fill selected,
`{stops: ..}` made the shape solid (#626). Outside PR #634 made the picker always send the
type; that hid the fault from the window and left it in the command.

Now, with no `type`: a `color` asks for a solid fill, and anything else changes each fill as
the kind it is, the swatch as its own and each selected shape as its own. Gradient parts given
to a solid fill change nothing there. The picker sends the whole gradient it shows on every
edit, so the swatch becomes that gradient, as the reference app's colour panel sets the tool's
fill. `element.set_fill` also takes an `alpha` alone, which changes only the opacity of solid
fills (#645): sending a whole colour for an opacity change gave several shapes one colour.

### 4. Joins between raw lines are held until their cost is bounded (outside PR #620)

#589 is a real bug: lines drawn one after another are separate runs of the merged drawing, so
each ends in its caps and no join is drawn where two meet. #620 fixes the picture by joining
runs of one style into outlines when a frame is drawn. It is not merged, because of where it
does the work:

* on every frame drawn, over the whole display list, with nothing kept between frames;
* for strokes that are not opaque, each join clips everything gathered so far, which grows
  with the square of a long polyline's length (ADR 0129 and ADR 0132 exist because of costs
  of this shape);
* as a third builder of chains, beside `geom::morph::chain` and the one shape tweens use;
* and only in the CPU renderer, the GPU stage and SVG: Lottie, the HTML page and Convert
  Lines to Fills would then differ from the stage.

It becomes backlog item P-058, with what it needs: a budget and a test on ten thousand edges,
the join made once for an evaluated frame, the shared chain builder, and the same joins in
every export. The branch was rebuilt without it, so its commits can merge later as they are.

## What else the batch settled, for the record

* **The Lasso** (outside PR #624) is a tool over one command, `raw.select_area`, which takes a
  closed loop of at most 10,000 points. The window thins the loop before sending it. With the
  Lasso came a change to `Session::execute`: a selection command that cuts raw shapes and then
  selects nothing rolls its cut back, for the marquee too.
* **Smooth, Straighten and Optimize Curves** (outside PR #623) are `modify.smooth`,
  `modify.straighten` and `modify.optimize`, with the geometry in `crates/geom`. Strength is 0
  to 100 as in the reference app's dialogs; the mapping to tolerances is ours. Straighten and
  Smooth leave a closed outline alone when the result would lose nearly all its area, and
  Optimize leaves one alone when it is far past the fitter's limit.
* **A click on a motion tween span selects the span** (#595), as in the reference app;
  Ctrl+click (⌘ on macOS) selects one frame. That is as designed.

## Consequences

* A document made by a script that relied on a single raw shape being tweened directly now
  gets a graphic symbol in its Library. The result says so.
* A script that sent `paint.set_fill {stops}` to turn a selected solid fill into the swatch's
  kind of gradient must now name the `type`.
* Corners between separately drawn lines still show caps, not joins, until P-058.
* The roadmap's pull request table has rows for the sixteen pull requests this batch read, and
  none for the others opened since 2026-11-24: those are the lead's to slot.

## Alternatives considered

* **Keep ADR 0016's silent conversion and add only the status line.** It honours the ADR as
  written and keeps the flow unbroken, but leaves the type unchosen, which the reporter asked
  for and the reference app gives, and an outside pull request had built the window well.
* **Make the conversion a confirmed command** (`confirm: true`, ADR 0004). That tier is for
  loss that can't be undone. This is one undo step.
* **Merge #620 and bound it afterwards.** The pictures are right today, but a cost in the path
  every frame takes is the kind that is found by an artist with a large drawing, not by a
  test, and two decisions already on file were written after exactly that.
* **Refuse classic tweens of raw shapes for parity.** Parity is the goal where the reference
  app does more or does it better. Here it does less.
