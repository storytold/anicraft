# ADR 0138: Fills of one merged drawing join without a seam

* Status: accepted (2026-11-27), built
* Relates to: ADR 0031 (merge drawing), ADR 0002 (the render pipeline), ADR 0019 and ADR 0029
  (the GPU stage draws what the oracle prepares), ADR 0070 (soft masks), ADR 0122 (a moved raw
  selection floats), ADR 0125 (the roadmap as a guideline; backlog item P-013), issue #182

## Context

Merge drawing leaves one element per fill region. Two regions that meet share an edge exactly:
the same Bézier pieces, one traced each way. Each fill was drawn on its own, anti-aliased, one
over the other. In a pixel the shared edge crosses, the first fill covers a part `a` and the
second the rest, `1 − a`, and drawing one over the other lets `a · (1 − a)` of what is below
through: up to a quarter of the stage's colour, as a hairline along every shared edge
(issue #182, a red fill cut by a curve on a white stage). The outlines are right; the mistake
is in treating two coverages of one pixel as if they were independent, when the two fills lie
side by side. It is the same on the CPU oracle and the GPU stage, which composites the way the
oracle does, and in masks, where two such fills united as `a + b − ab`.

## Decision

1. **The display list says which fills are side by side.** `DisplayItem::joins` is set by
   `evaluate` on a shape of a merged drawing that follows another shape of the same drawing:
   the raw shapes of one keyframe on one layer (a group's, or a symbol's layer's, are drawings
   of their own). It is a fact about the document, so every renderer gets it and `evaluate`
   stays pure.

2. **A run of joined fills is added up, then composited once.** The renderer makes a run of
   consecutive joined fills that are drawn as fills in the same layer, clip and group, with
   nothing between them (a fill's own stroke ends the run: it is drawn before the next fill).
   Per pixel a run starts transparent, with nothing covered. Each fill, of coverage `k` and
   colour `s`, with `c` covered so far:
   * hides what it must overlap, `o = max(0, c + k − 1)`: the run's colour times
     `1 − o / c · s.a`;
   * adds `s · k`, and `c` becomes `min(1, c + k)`.

   When the run ends its colour goes src-over onto what is below, times its clip's coverage.
   For two fills that share an edge `c + k = 1`: nothing overlaps, the colours add up to full
   cover and nothing below shows. Half-transparent fills side by side stay half-transparent
   along the edge, where one over the other made the edge denser.

3. **Side by side as far as the pixel has room, one over the other past that.** The rule
   takes two partial coverages of a pixel to overlap as little as they can. Where either fill
   covers the pixel whole it is exactly src-over, so fills that do lie over each other (a
   shape moved onto the drawing and still floating, ADR 0122; a file written by hand) are
   drawn as before everywhere except in pixels both their edges cross, and there the
   difference is a fraction of a pixel's coverage. We chose this over trusting the flag
   blindly (adding without the overlap term), which would brighten or darken overlapping
   fills over their whole area.

4. **Masks join the same way.** A coverage mask unites a run's coverage as one fill's; a soft
   mask composites the run's colour. A mask drawn as two fills no longer leaks or hides along
   their edge.

5. **The oracle defines it, the GPU follows.** `Fill::run` numbers the runs in a prepared
   scene. The fine shader keeps the run's colour and coverage per pixel and ends the run at
   the first entry that isn't of it; the number rides in the op header's flags. The parity
   tests compare both on joined fills in the open, in masks, in a soft mask and in a fading
   group.

6. **What it costs.** A pixel a fill covers whole, where the run has nothing yet, is drawn at
   once, so only the pixels along edges go through the run's buffers, which each render thread
   keeps. A 1280 × 720 frame of 1,000 joined fills takes the oracle 2.4 ms, and 2.5 ms drawn
   one over the other (the same within noise; with buffers made anew for every band it was
   5.8 ms).

## Consequences

* The hairline of #182 is gone from the stage (CPU and GPU), Test Movie and every raster export,
  for drawings made here and raw shapes opened from Animate files alike.
* Pixels along the shared edges of merged fills change; nothing else does. One golden picture
  had such an edge and was made anew (`crates/xfl/tests/corpus/filters`, one row of 160
  pixels between a dark and a light fill): each pixel is now the even mix of the two fills it
  lies half in, (96, 111, 106), where it had the white stage in it, (148, 151, 132). The other
  31 corpus frames are within their tolerance and were left as they were.
* Edges that are shared but flattened differently (two shapes that only happen to touch, not
  cut by the merge) add up to within the flattening tolerance, a tenth of a pixel, instead of
  exactly.
* *Not covered:* the vector exports (SVG, HTML canvas, Lottie) write each fill as its own path,
  and a viewer that anti-aliases them one over the other shows the same hairline there; fills
  of one drawing separated in the stacking order by a line (the merge keeps a drawing's fills
  under its lines, so this is rare); a fill meeting a drawing object or another layer.
* *Rejected:* growing each fill under its neighbours (wrong for transparent fills, and it
  needs to know which edges are shared); drawing a merged drawing through one coverage pass
  with a paint per region (a second rasterizer for both renderers); supersampling (it thins the
  hairline, never removes it).
