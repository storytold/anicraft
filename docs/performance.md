# Performance

Rule: **measure first, then optimize, then record the number here.** Every optimization lands
with a benchmark delta and a test that it did not change output (the CPU renderer is the
oracle; its output must stay bit-identical).

## How to measure

```sh
cargo run --release -p anicraft-cli -- bench                      # 50 layers × 200 shapes × 240 frames
cargo run --release -p anicraft-cli -- bench --layers 200 --shapes 500 --frames 600
cargo run --release -p anicraft-cli -- bench --stroke 6000        # one long Brush stroke (ADR 0129)
cargo run --release -p anicraft-cli -- bench --stroke-file drawing.anicraft   # the same steps on a file
```

`bench` builds a deterministic stress document (`doc::sample::stress`) and reports file size,
save/load time, evaluate/render time per frame, zoomed render with culling, snapshot cost,
per-edit and per-undo time, history bytes per step and hit-test time. In the app, the status
bar shows render time, shapes drawn and culled, frame-cache size and hit rate, and history
memory.

## Budgets

The default `bench` scenario: 50 layers × 200 shapes × 240 frames, 200k element instances.

| Scenario | Budget | Measured 2026-11-06 (release, 32 threads) | Enforced by |
|---|---|---|---|
| Edit command on a 200k-instance document | < 1 ms | 0.013 ms ✅ | `bench --check` |
| Undo / redo | < 1 ms | 0.009 ms ✅ | `bench --check` |
| Undo memory per small edit | < 16 KB | 2.7 KB ✅ | test, every PR |
| Document snapshot | < 50 µs, shares everything | 0.04 µs ✅ | test (sharing) and `bench --check` |
| Evaluate one frame (200k instances) | < 4 ms | 1.7 ms ✅ | `bench --check` |
| Render 1080p, ~10k visible shapes | < 16.7 ms (60 fps) | 15.3–15.8 ms ✅ | `bench --check` |
| Render at 400% zoom | < 16.7 ms | 9.0–9.3 ms ✅ | `bench --check` |
| Shapes drawn at 400% zoom | ≤ 1/8 of the frame's shapes | 5.5% ✅ | test, every PR |
| Hit test | < 200 µs | 52 µs ✅ | `bench --check` |
| File size | < 400 bytes per element instance | 342 ✅ | test, every PR |
| Save / load 65 MB file | < 500 ms | 123 / 146 ms ✅ | `bench --check` |
| UI frame while rendering | never blocked | worker thread ✅ | design (ADR 0002) |
| Autosave: time on the editing thread | < 1 ms | 7–15 µs ✅ | `bench --check` |
| Autosave of the 65 MB file, in the background | < 2 s | 183–184 ms ✅ | `bench --check` |
| A long Brush stroke: tracing its outline (ADR 0129) | bounded: `MAX_STAMP_WORK` cells visited, whatever the stroke | 45 to 310 ms for 1,500 to 100,000 samples, sizes 3 to 60 (2026-11-21; 1,500 ms and growing before) | test, every PR (the budget, not the time) |
| A long Brush stroke in the editor | the trace is never on the window's thread | worker thread ✅; the merge (75 to 220 ms at the largest) still is | test, every PR |
| GPU stage, 1,000 shapes at 4K, stage tiles (M3 exit) | < 16.7 ms | 4.5–4.7 ms ✅ (2026-11-10, RTX 5090) | `gpu_bench --check` |
| GPU stage, 10k shapes at 4K, stage tiles | < 16.7 ms | 10.1–10.4 ms ✅ | `gpu_bench --check` |
| GPU stage, 50k shapes at 1080p, stage tiles | < 33.3 ms (30 fps) | 24.8–26.3 ms ✅ | `gpu_bench --check` |
| GPU stage, 50k shapes at 4K, one texture | < 33.3 ms | 22.5–23.8 ms ✅ | `gpu_bench --check` |
| GPU stage, onion ±5 outlines / tinted at 1080p, stage tiles | ≤ 1.25 | 1.03–1.06 ✅ | `gpu_bench --check` |

**Memory and frame time stay flat (2026-11-23, P-004, ADR 0132; #314, #477).** Measured
before changing anything, then after, on the same machine (release, 32 threads).

*What the picture cache holds.* A drag makes a new document state on every pointer move, a
zoom gesture a new zoom level on every frame, and each one's tiles used to stay until the
768 MB budget (256 MB in the web build) was full.

| Scenario | Before | After |
|---|---|---|
| Web build in Node (`tests/wasm.rs`): WebAssembly memory over a drag of 120 moves, a 1024 × 512 view | 27 MB to 255 MB (the cache at 240 MB) | 27 MB throughout (the cache at 4 MB) |
| Desktop, Ctrl+wheel from fit to 6400 % (84 frames): pictures cached | 147 pictures, 136 MB | 44 pictures, 44 MB (the view and the ring around it, at this zoom and the one before) |
| Desktop, a drag of 300 moves in a 1600 × 940 window: pictures cached, textures, process memory | not run; by the sizes below a move adds a view of about 12 MB, so the 768 MB budget is full after some 60 moves | 42 pictures (38 MB), 12 textures, the process at 102 MB |
| Desktop, the sample's 48 frames played five times in that window | the budget | the same: 768 MB of pictures after the first lap and after the fifth (the movie at this size is larger than the budget), 192 textures, the process at 814 to 826 MB |

*A stage view at each zoom*, 1600 × 900 in 512 px tiles on the CPU oracle, a drawing of 64
brush strokes merged into three shapes
(`cargo test --release -p anicraft-engine --lib stage_timings -- --ignored --nocapture`):

| Zoom | Tiles | Edges walked, before | after | Prepare, before | after | One thread (as the browser draws), before | after | 32 threads, before | after |
|---|---|---|---|---|---|---|---|---|---|
| 100 % | 6 | 42,708 | 21,935 | 1.27 ms | 1.41 ms | 9.0 ms | 8.1 ms | 2.8 ms | 2.9 ms |
| 400 % | 12 | 133,288 | 31,317 | 2.94 ms | 2.08 ms | 31.7 ms | 23.3 ms | 8.0 ms | 8.3 ms |
| 1600 % | 12 | 226,884 | 22,901 | 3.67 ms | 2.14 ms | 34.4 ms | 20.1 ms | 8.7 ms | 7.9 ms |
| 6400 % | 8 | 281,304 | 13,416 | 3.34 ms | 1.32 ms | 33.5 ms | 13.0 ms | 6.7 ms | 6.2 ms |

Before, a tile walked every edge of every shape that touched it, cut finer the deeper the
zoom; now the parts of a fill above, below and right of the tile are left out, and the
pictures are the same to the last bit (`render::cut_tests` compares the float pixmaps). On one
thread a tile at 6400 % costs what a tile at 100 % costs (1.6 ms against 1.4 ms; it was 4.2 ms
against 1.5 ms). With 512 strokes the same view went from 25.1 ms to 12.0 ms on one thread.

*Frame time during a zoom gesture.* The stage asks for nothing while the zoom is changing, so
a frame of the gesture costs the interface alone (0.7 ms on the desktop, at every zoom up to 6400 %). In the
browser each of those frames used to draw the whole view on the interface's thread: 9 to
34 ms for the drawing above.

Playing is bounded by the budget, not by this change: uncompressed, a 1600 × 940 view is
about 12 MB a frame, so a movie of some 60 frames fills 768 MB. Storing flat and repeated
tiles once is the next step (P-004 in the roadmap).

The checks are relations, not timings (`engine::render_budget_tests`,
`stage_budget_tests`, `render::cut_tests`, `tests/wasm.rs`): a long drag or zoom holds what a
short one holds, playing longer adds nothing, a deep tile walks fewer edges than the whole
drawing has at 100 %, and what the screen needs is drawn before look-ahead.

**1080p render, back under budget (2026-11-06).** It had drifted to 19.4–20.2 ms (v0.1.0
measured 18.0–18.9 ms on the same machine; the 16.5 ms of 2026-10-28 didn't reproduce).
Timing each step of a frame showed the time was outside the drawing itself:

| Change | 1080p | 400% |
|---|---|---|
| Before | 19.4–20.2 ms | 11.6–11.9 ms |
| Background filled per band, in parallel, instead of one serial 33 MB fill | 16.9–17.5 ms | 9.1–9.6 ms |
| Edges wholly above or below a band skipped before splitting; fill polygons moved, not copied | 17.0–17.2 ms | 8.9–9.1 ms |
| Prepared polygons freed on the pool after the frame, off the critical path | 15.3–15.8 ms | 9.0–9.3 ms |

Output is unchanged: the sample's frames and a bitmap document render byte-identical before
and after.

**Onion skin ±5 at 1080p (2026-11-10, ADR 0047).** Through the stage's tiled path (40 tiles
of 256 px, one scene evaluation per frame, tiles in parallel), release build, 32 threads, best
of frames 12, 30 and 48:
`cargo test --release -p anicraft-engine onion_timings -- --ignored --nocapture`.

| Scene | Off | ±5 tinted, fading | ±5 outlines | Keyframes only ±5 | Edit Multiple Frames ±5 |
|---|---|---|---|---|---|
| Sample (7 items a frame; 67 with ±5) | 5.9–6.1 ms | 6.0–6.5 ms | 6.5–7.0 ms | 5.8–6.2 ms | 6.0–6.1 ms |
| Stress 20 × 100 (2,000 items a frame; 22,000 with ±5) | 7.2–7.9 ms | 16.2–17.2 ms | 34–39 ms | 7.5–7.6 ms | 6.6–7.3 ms |

Composing the stress scene with ±5 (ten extra evaluations, then each layer's onion frames
merged under its own artwork) took 8.3 ms at first: 3.9 ms of it was putting the items in layer
order (an append, then a reorder). One k-way merge by layer, each item moved once, brought that
to 0.85 ms, and evaluating the extra frames in parallel brought the whole to 3.0–3.3 ms. Output
is unchanged (the onion tests compare items and pixels). The settings are part of every tile's
key, so the frame cache keeps working: looking at the same frame again is 40 hits in 0.02 ms,
and other settings are other pictures.

**Strokes as one outline (2026-11-17, ADR 0110).** The stroker used to hand the rasterizers a
quad per segment and a disc or fan per bend, whose overlaps made edge pixels too dark. It now
builds one outline per run of segments and unites them (`anicraft_geom::polygons::union`, about
60 ns an edge on an outline that is already nearly clean; uniting the old pieces would have
cost 1.6 ms for one wide curve). The stroker itself is about three times slower (a wide curve:
20 → 60 µs) and the rasterizers get 3 to 4 times fewer edges. CPU oracle, median of 80
renders, 32 threads, before → after:

| Scene | Edges | Before | After |
|---|---|---|---|
| 2,000 outlined ovals, stars and rounded rectangles, 1920 × 1080 | 703,000 → 254,000 | 9.3 ms | 9.0 ms |
| The demo scene, 1280 × 720 | 1,345 → 767 | 2.5 ms | 2.6 ms |
| Seven tangled lines (waves, dashes, a knot, a hairpin), 400 × 300 | 4,995 → 1,347 | 0.51 ms | 0.63 ms |
| The same at 300 % | 12,341 → 2,284 | 2.5 ms | 2.65 ms |
| 300 self-crossing scribbles, 1280 × 720 | 499,000 → 126,000 | 5.9 ms | 6.4 ms |

Convex closed outlines (rectangles, ovals, rounded rectangles) and straight lines skip the
union: their outline can't overlap. Output changes only at stroke edges (14 corpus goldens
regenerated; ADR 0110 lists what changed).

**Outlines as hairlines (2026-11-10, ADR 0048).** Outline mode drew half-pixel strokes (four
edges per segment and a disc at every bend); it now draws hairlines, one edge per segment,
covered by distance. Same test, same machine:

| Scene | Off | ±5 tinted, fading | ±5 outlines | Keyframes only ±5 | Edit Multiple Frames ±5 |
|---|---|---|---|---|---|
| Sample | 5.7 ms | 5.9 ms | 5.5 ms | 5.9 ms | 5.7 ms |
| Stress 20 × 100 | 7.6 ms | 14.8 ms | **17.1 ms** (was 34–39) | 7.3 ms | 7.2 ms |

**Crash-recovery autosave (2026-11-09, ADR 0015).** `bench` now runs one autosave the way the
app does: an `Autosaver` in a scratch recovery folder. The editing thread only hands over an
`Arc` snapshot of the document; the worker serializes and writes atomically (temp file,
flushed to disk, renamed).

| Document | Editing thread | Serialize | Write + flush + rename | Total, in the background |
|---|---|---|---|---|
| 200k instances, 65 MB | 6.6–15.4 µs | 124–128 ms | 52–55 ms | 183–184 ms |
| 800k instances (100 × 400 × 240), 262 MB | 9.9 µs | 501 ms | 210 ms | 725 ms |

With the default "every minute or 25 changes", even the 262 MB document keeps the worker busy
for about 1% of the time, and the UI never waits for it.

**Opening a 50 MB `.fla` (2026-11-11, ADR 0054; release, 32 threads, one used).** A generated
archive (`measure_importing_a_50_mb_fla` in `crates/xfl/tests/fla.rs`: 400 symbols of 600
edges each, deflated, and 24 noisy 1024 × 512 PNGs, stored; 54.6 MB, 425 files, 65.8 MB
inflated), five runs each:

```sh
cargo test --release -p anicraft-xfl --test fla -- --ignored --nocapture
```

| Step | CRC-32 byte by byte | CRC-32 by slicing by 8 |
|---|---|---|
| Open (end record, central directory, every local header, overlap check) | 0.19–0.25 ms | 0.18–0.24 ms |
| Read and check every entry (inflate, size, CRC) | 128–140 ms (471–516 MB/s) | 53–54 ms (1.22–1.24 GB/s) |
| Whole import (read, XML, edges, library, timeline) | 388–458 ms | **311–328 ms** |

The checks on every entry (exact size, checksum) now cost about 30 ms of the import; the rest
is parsing and building the document.

### GPU stage (M3, ADR 0019)

`cargo run --release -p anicraft-gpu --example gpu_bench` (2026-11-09, RTX 5090 over Vulkan,
32 threads). GPU ms is the whole call: preparing the fills (shared with the oracle), binning,
upload, the compute pass and the readback. Parity is mean / max difference from the oracle per
8-bit channel.

| Scene | Size | CPU oracle | GPU stage (prepare + bin + GPU) | Parity |
|---|---|---|---|---|
| Stress, 50 × 200 (≈10k drawn) | 1920×1080 | 11.3 ms | 13.6–14.3 ms (3.1 + 5.0 + 4.6) | < 0.0001 / 1 |
| Stress, 50 × 200 | 3840×2160 | 26.6–27.3 ms | 22.1–22.4 ms (2.8 + 8.0 + 8.4) | < 0.0001 / 1 |
| 1,000 shapes | 1920×1080 | 4.4–4.6 ms | 2.8–2.9 ms | < 0.0001 / 1 |
| 1,000 shapes (M3 target: 4K at 60 fps) | 3840×2160 | 17.4–17.8 ms | 8.8–9.0 ms ✅ | < 0.0001 / 1 |
| Sample scene | 3840×2160 | 18.2 ms | 8.8 ms | < 0.0001 / 1 |

Binning went from 16 ms to 5 ms on the stress frame by binning fills in chunks with reused
scratch (no allocation per edge piece), freeing the per-chunk buffers on the pool, and
scattering the tile lists in parallel per tile row. What is left: CPU binning on very large
scenes (next: GPU binning) and the 33 MB readback at 4K (next: render into the texture egui
shows).

### GPU stage, second slice: no readback, masks and groups (M3, ADR 0029)

Same bench and machine. *Read-back* is the call above; *direct* is the calling thread's time
with `render_texture` (prepare + bin + upload and submit, the window's path); *tiled* is the
stage's way, 512 px tiles drawn in parallel, until the GPU has finished.

| Scene | Size | CPU oracle | Read-back | Direct | Tiled (stage) | Parity |
|---|---|---|---|---|---|---|
| Stress, 50 × 200 (≈10k shapes) | 1920×1080 | 10.6 ms | 13.4 ms | 9.4–10.4 ms (3.0 + 4.7 + 1.7) | 8.7 ms | < 0.0001 / 1 |
| Stress, 50 × 200 | 3840×2160 | 25.8–28.1 ms | 23.2 ms | 16.8–18.7 ms (2.7 + 7.5 + 4.8) | **10.2 ms** ✅ 60 fps | < 0.0001 / 1 |
| 1,000 shapes | 1920×1080 | 4.4–5.0 ms | 2.9–3.0 ms | 1.1–1.2 ms | 1.8 ms | < 0.0001 / 1 |
| 1,000 shapes (M3 target: 4K at 60 fps) | 3840×2160 | 16.2–19.6 ms | 8.4–8.5 ms | **2.1–2.4 ms** ✅ | 4.8 ms | < 0.0001 / 1 |
| Masked stress (every other layer a mask) | 1920×1080 | 25.4–26.2 ms | 10.7 ms | 8.7–9.4 ms | 6.6 ms | < 0.0001 / 1 |
| Masked stress | 3840×2160 | 74.8–80.0 ms | 20.5–21.8 ms | 14.6–15.3 ms | 10.9 ms | < 0.0001 / 1 |
| Sample with a mask and faded symbols | 3840×2160 | 19.1–19.9 ms | 8.3–8.9 ms | 2.1–2.6 ms | 4.6 ms | < 0.0001 / 1 |

Masked scenes are where the GPU gains most (7× at 4K): the oracle computes each clip's
coverage per band into a buffer, the GPU per pixel from the artwork's tile entries. Binning is
the largest term of a whole 10k-shape frame, but tiles spread it over every core; it stays on
the CPU until scenes of 50k+ shapes or 8K stages need more (ADR 0029). Small scenes pay a
little per-tile set-up in the tiled stage (1.8 vs 1.1 ms at 1080p); reusing buffers across
tiles is next.

### GPU stage, third slice: bitmaps and filters, the default stage (M3, ADR 0032)

Same bench and machine, on Vulkan and on Direct3D 12. The CPU column is also what these
frames cost the stage before this slice, when bitmaps and filters always fell back to the CPU.

| Scene | Size | CPU oracle | Direct (Vulkan) | Tiled (Vulkan) | Direct (DX12) | Tiled (DX12) | Parity |
|---|---|---|---|---|---|---|---|
| Filtered sample (drop shadow + bevel, glow, a bitmap sky) | 1920×1080 | 9.8 ms | 1.6 ms | 3.7 ms | 1.5 ms | 5.4 ms | < 0.0001 / 1 |
| Filtered sample | 3840×2160 | 31.5 ms | **3.9 ms** | 9.5 ms ✅ 60 fps | 4.2 ms | 14.0 ms | < 0.0001 / 1 |
| 200 bitmap fills (8 images) | 1920×1080 | 10.9 ms | 1.5 ms | 2.0 ms | 1.2 ms | 2.9 ms | 0.0002 / 172 (1 px) |
| 200 bitmap fills | 3840×2160 | 37.2 ms | **4.4 ms** | 5.7 ms | 4.3 ms | 8.7 ms | 0.0002 / 1 |
| 1,000 shapes | 3840×2160 | 17.1 ms | 2.0 ms | 4.7 ms | 2.9 ms | 8.7 ms | < 0.0001 / 1 |

Each stage tile a filtered symbol touches draws and filters its own copy of the layer (with
the filters' reach around the tile), so tiling costs more than on plain scenes; sharing the
layer across a frame's tiles is next. The one large difference is a single pixel whose centre
falls within float rounding of a non-repeating bitmap's edge (ADR 0032). Bitmaps are uploaded
once per image and cached (256 MB budget), so after the first frame they cost no transfer.

### GPU stage, fourth slice: binning on the GPU, hairlines (M3, ADR 0048)

`cargo run --release -p anicraft-gpu --example gpu_bench` (2026-11-10, RTX 5090 over Vulkan,
32 threads, a machine shared with other builds: best of three runs). *Direct* is the calling
thread's time with `render_texture`, *tiled* the stage's way (512 px tiles in parallel until
the GPU has finished).

| Scene | Size | CPU oracle | Before: direct / tiled | After, binned on the CPU: direct / tiled | After, Auto: direct / tiled | Parity (mean / max, 1/255) |
|---|---|---|---|---|---|---|
| Stress, 50 × 200 (≈10k shapes) | 1920×1080 | 9.8 ms | 9.0 / 7.4 ms | 8.6 / 5.5 ms | **3.9** / 5.5 ms | < 0.0001 / 1 |
| Stress, 50 × 200 | 3840×2160 | 25.9 ms | 21.7 / 10.9 ms | 16.4 / 10.1 ms | **4.7** / 10.1 ms | < 0.0001 / 1 |
| 1,000 shapes (binned on the CPU: few edges) | 3840×2160 | 16.9 ms | 2.2 / 4.5 ms | 1.6 / 4.2 ms | 1.7 / 4.5 ms | < 0.0001 / 1 |
| 50,000 shapes (250 × 200) | 1920×1080 | 31.7 ms | 52.2 / 32.4 ms | 33.5 / 26.8 ms | **17.7** / 24.8 ms | < 0.0001 / 1 |
| 50,000 shapes | 3840×2160 | 73.1 ms | 122.1 / 75.3 ms | 103.9 / 61.5 ms | **22.5** / **36.7** ms | < 0.0001 / 1 |
| Masked stress (binned on the CPU: masks) | 3840×2160 | 72.4 ms | 13.8 / 12.3 ms | 12.0 / 10.7 ms | 11.3 / 10.5 ms | < 0.0001 / 1 |
| Onion skin off (stress 20 × 100) | 1920×1080 | 5.0 ms | 2.1 / 2.7 ms | 1.8 / 2.2 ms | 1.1 / 2.3 ms | < 0.0001 / 1 |
| Onion ±5, tinted (22,000 items) | 1920×1080 | 15.1 ms | 15.7 / 12.6 ms | 12.8 / 10.5 ms | 7.1 / 9.8 ms | < 0.0001 / 1 |
| Onion ±5, tinted | 3840×2160 | 37.3 ms | 32.1 / 25.1 ms | 20.3 / 22.0 ms | 8.5 / 16.5 ms | 0.0001 / 1 |
| Onion ±5, **outlines** | 1920×1080 | 20.0 ms (was 36.0) | 74.7 / 73.0 ms | 12.9 / 10.4 ms | 7.3 / **10.1** ms | < 0.0001 / 1 |
| Onion ±5, **outlines** | 3840×2160 | 37.9 ms (was 60.2) | 135.4 / 95.4 ms | 20.3 / 18.1 ms | 8.6 / **15.3** ms | < 0.0001 / 1 |

*Before* is `main` before this slice (CPU binning with float backdrops, stroked hairlines),
measured the same day on the same machine. In stage tiles the GPU binning wins once frames are
heavy (50k shapes: 61.5 → 36.7 ms at 4K) and never loses below `AUTO_MIN_EDGES`; a whole frame
in one texture gains most (10k shapes at 4K: 16.4 → 4.7 ms). **Outline mode is as fast as
tinted onion skin** on the GPU stage: 10.1 vs 9.8 ms at 1080p in tiles, 15.3 vs 16.5 ms at 4K,
7.3 vs 7.1 ms in one texture (it was 73 vs 12.6 ms in tiles).

On **Direct3D 12** (same card, `WGPU_BACKEND=dx12`, two runs) the GPU binning still wins in
one texture (10k shapes at 4K: 17.5 → 6.3 ms; 50k: 154 → 30 ms) but loses in stage tiles
(onion ±5 at 1080p: 9.7 → 14.4 ms tinted, 10.0 → 15.4 ms outlines; the passes cost more per
dispatch there, as ADR 0032 saw for the fine pass), so `Binning::Auto` bins on the GPU on
Vulkan only. With CPU binning, DX12's onion outlines take 10.1 ms in tiles against 10.0 tinted.

The CPU stage (the oracle through the render service's 256 px tiles, `onion_timings`, best of
frames 12, 30 and 48) with hairlines: onion ±5 outlines **17.1 ms** against 14.8 tinted at
1080p (was 34–39 against 16.2–17.2).

### Filtered Array preview

Run these manual benchmarks on an otherwise idle machine. They use the normal Cargo test
profile and report timings without imposing a machine-dependent pass threshold.

```sh
cargo test --workspace --locked array_panel::tests::measure_filtered_preview -- --ignored --nocapture --test-threads=1
cargo test --workspace --locked array_panel::tests::measure_cached_blur_translation -- --ignored --nocapture --test-threads=1
```

The first benchmark drives the real Array editor with pointer events and the CPU stage renderer.
It uses the `array-blur` fixture, a 640 × 400 stage in a 1600 × 940 window at 65% zoom, with
Dim Stage on, three authored stops, and one enabled Medium Blur on each copy. Counts include
the original. Immediate mode renders each change synchronously. Six warm-up moves precede 30
timed moves for each interaction. Frame time includes editor updates and painting commands.
Stage time measures the synchronous tile batch, before texture upload. The benchmark checks
that each measured move changes document state and produces a complete, fresh stage image.

Baseline on Windows x86-64, Intel Core Ultra 9 185H, 22 logical processors available to the
test. Each pair is p50 / p95 in milliseconds.

| Instances | Interaction | Frame ms | Stage batch ms |
|---:|---|---:|---:|
| 10 | End on stage | 10.77 / 13.08 | 5.34 / 8.05 |
| 10 | End on pasteboard | 10.59 / 13.59 | 5.60 / 8.33 |
| 10 | Blur X field | 11.18 / 13.08 | 6.38 / 7.93 |
| 50 | End on stage | 14.44 / 19.13 | 9.58 / 12.85 |
| 50 | End on pasteboard | 14.84 / 17.97 | 9.63 / 12.14 |
| 50 | Blur X field | 17.65 / 56.41 | 12.54 / 43.48 |
| 200 | End on stage | 106.12 / 147.28 | 90.06 / 130.79 |
| 200 | End on pasteboard | 80.62 / 99.18 | 68.15 / 85.47 |
| 200 | Blur X field | 116.77 / 191.47 | 99.42 / 171.60 |

At 200 copies the stage batch dominates frame time. A separate run put the onstage End
median at 95.58 ms and the Blur X median at 98.54 ms, which gives a sense of run-to-run
variation on this machine.

The Auto probe starts from the same fixture and moves the Linear End handle onstage at a
target cadence of 60 Hz. It asks for one repaint between pointer moves. A complete update
shows the current document state while the button is still down. A partial update has some
current stage tiles available. Missing tiles retain their previous pictures. The probe pauses
with the button held and polls for the current image. It then moves once more and measures
catch-up from button release at the new position.

| Instances | Complete or partial updates during 30 moves | Catch-up with button held | Catch-up after release on next move |
|---:|---:|---:|---:|
| 10 | 0 | 24.63 ms | 4.14 ms |
| 50 | 0 | 30.97 ms | 2.53 ms |
| 200 | 0 | 139.35 ms | 74.46 ms |

The sustained drag displayed older artwork in both runs. Each count caught up while the
button was still held once the pointer paused. Auto can switch to the background renderer
when a synchronous stage batch exceeds 12 ms. Each pointer edit can then replace the pending
batch, so a completed render of an earlier state may miss its chance to appear.

The cache probe uses the same 34 × 15 shape and a constant Medium Blur of X 22 px, Y 3 px
for every copy. It renders one padded transparent image at 100% and composites translated
copies in document order over a transparent 640 × 400 surface. It times the composite and
compares every whole-pixel result with a fresh CPU render of the same Array. A half-pixel
move tests whether the same cached sample can stand in for the exact result. The probe
measures composition plus canvas clear with a warm cache. It excludes editor work, cache
lookup, texture upload, and GPU presentation.

| Instances | Fresh render p50 | Cached composite p50 | Pixels differing after half-pixel move |
|---:|---:|---:|---:|
| 10 | 4.03 ms | 0.41 ms | 8,930 |
| 50 | 15.56 ms | 1.52 ms | 1,584 |
| 200 | 165.27 ms | 13.69 ms | 1,181 |

All 108 whole-pixel placements, including warm-up, matched byte for byte in RGBA8. The
half-pixel oracle differed at every tested count. These timings characterize the isolated
composition opportunity on this fixture with a shared raster per copy.

When a filtered Array moves, the engine keeps its position-only preview generation. The
stage falls back to state-based keys for every visible tile because its current position cache
uses path bounds, and a filter can draw beyond them. This invalidates even tiles outside the
affected region. The footprint check also builds a position map before it discovers filters
and falls back. A Blur value change takes the full preview command path and creates a new
generation on each measured field update. The renderer still quantizes blur widths to device
pixels, so an eventual pixel cache could reuse resolved widths across edits that land on the
same rasterized value.

The original proposal was to reuse a padded image on translations at the same
device-pixel phase. It would need to key the artwork frame, view scale, transform basis,
resolved filters and effective colour, retain correct clipping and stacking, and composite
with the current backdrop. Rotation, scale, filter edits that change the resolved effect,
or a change in device-pixel phase require a suitable fresh raster. For fractional drags,
a translated image could serve as a transient preview while the ordinary renderer draws
the exact current image and the final released state. The probe covers isolated normal-blend
copies with no masks. It does not exercise a GPU layer cache.

#### Provisional Linear drag preview

Auto now uses the position-only Array generation to keep a bounded warm set of
premultiplied textures. The normal render service draws the backdrop tiles and the
isolated Blur copies off the input path once the view and Array have been still for
100 ms. The warm texture set holds one generation and view, up to 8 Mi pixels (32 MiB of
RGBA8 textures), with a 2048-pixel patch edge limit and 32 texture uploads per UI frame.
The render service accounts for the CPU images in its existing cache budget.

During an eligible gizmo move, the stage draws the backdrop and places the warmed
copy textures in stacking order. The ordinary exact tile batch resumes after 65 ms
without motion or on release. The provisional frame stays visible until the whole
current exact batch is ready. Texture interpolation at fractional offsets and separate
8-bit layer composition can differ from fresh rasterization. The stage keeps the provisional
pictures in their own cache, and the existing exact path renders the document and exports.

Eligibility requires Linear geometry, stable realized transforms, an enabled source Blur,
and an independent normal-blend foreground stack. Dim Stage can supply that order. The
cache defers to standard rendering for cold starts, over-budget scenes, other active
filter types, masks, shared groups, camera or onion previews, and unsupported stacking.
Numerical edits, including Blur X/Y, use the exact stage path. Changing properties,
renderers, or the view scale invalidates the warm set. Stage clipping is applied while
drawing, so a prepared copy can cross onto the pasteboard without being cropped to its old
position.

The same `measure_filtered_preview` command now also runs paired Auto trials with the
cache disabled and enabled in the same binary. Each trial starts with the same fixture,
uses a wall-clock egui input clock, and samples 30 moves after six warm-up moves. Pointer
time measures the UI frame that consumes each event. The probe counts current exact,
provisional, and partial frames on the event frame and one intervening repaint. These are
editor frame timings on the CPU stage, before display presentation.

| Instances | Auto drag | Original pointer ms p50 / p95 | Warm-cache pointer ms p50 / p95 | Fresh frames out of 30, original → enabled |
|---:|---|---:|---:|---:|
| 10 | End on stage | 4.51 / 7.96 | 2.85 / 4.59 | 0 → 30 |
| 10 | End crossing stage edge | 3.13 / 5.07 | 2.33 / 4.65 | 0 → 30 |
| 10 | Translate bar | 3.86 / 5.62 | 2.33 / 4.72 | 0 → 30 |
| 10 | Blur X field | 4.36 / 8.51 | 9.85 / 21.98 | 2 → 19 |
| 50 | End on stage | 3.35 / 7.38 | 2.79 / 4.80 | 0 → 30 |
| 50 | End crossing stage edge | 4.58 / 9.83 | 2.71 / 4.32 | 0 → 30 |
| 50 | Translate bar | 2.90 / 5.34 | 2.93 / 4.40 | 0 → 30 |
| 50 | Blur X field | 3.58 / 6.45 | 3.64 / 6.53 | 0 → 0 |
| 200 | End on stage | 5.22 / 6.11 | 3.16 / 5.26 | 0 → 30 |
| 200 | End crossing stage edge | 4.28 / 5.67 | 1.07 / 2.61 | 0 → 30 |
| 200 | Translate bar | 2.51 / 3.08 | 2.70 / 4.13 | 0 → 30 |
| 200 | Blur X field | 3.64 / 4.58 | 2.94 / 4.22 | 0 → 0 |

Every fresh geometry frame in the enabled trials used provisional pictures. The original
geometry trials in the table showed no fresh full or partial tiles during the sampled moves, consistent
with the checkpoint. The 200-copy End drag still took 102.23 ms p50 in Immediate, compared
with 106.12 ms at the checkpoint. Immediate continues to rasterize each edit synchronously.
All Blur-field samples took the ordinary stage path in both Auto trials. At 10 copies, the
trial with the cache enabled showed more exact updates and longer pointer frames. Auto's
12 ms scheduling threshold depends on the preceding render, and both trials showed zero
cache reuse on Blur-field edits.

The textures were ready in 121–197 ms, including the 100 ms dwell. Across the enabled
geometry trials, exact catch-up took 77–159 ms from the end of the timed motion with the
button held, and 22–90 ms from release on the next move. The original geometry trials took
19–183 ms held and 3–98 ms after release. The enabled trials kept the current artwork
visible while waiting for exact tiles. The paired numbers are one run on Windows x86-64
with the Intel Core Ultra 9 185H and 22 logical processors.

A repeat run with the final texture cap produced the same 30/30 provisional updates in each
geometry trial and zero during Blur edits. For 200 copies, End measured 4.78 → 2.08 ms p50
on stage and 4.56 → 1.07 ms while crossing the stage edge. The 10-copy bar trial showed how
Auto can vary with render cost: the original path drew 27/30 exact updates at 17.30 ms p50,
and the warm cache drew 30/30 provisional updates at 2.50 ms p50.

### Filtered Array GPU pressure

On Intel Arc integrated graphics with Vulkan on Windows, the 2×-scale tile probe draws ten
64 px shapes in 20 tiles of 512 × 512 px. It measures 30 batches, alternating translations
in both directions. The switch variant starts with a radial batch and then translates the
linear arrangement. The allocator report distinguishes live allocations from reserved blocks
and covers wgpu's allocator for the test device.

```powershell
$env:WGPU_BACKEND = 'vulkan'
cargo test -p anicraft-gpu --locked measure_filtered_tile_pressure -- --ignored --nocapture --test-threads=1
cargo test --locked high_quality_shadow_glow_gpu_drag_lifecycle -- --ignored --nocapture --test-threads=1
```

In the original path, the probe held up to 384 completed textures, matching the render service's
native tile budget. High quality increases filter reach and runs more blur passes. The passes
reuse the same buffers, but the extra padding increases each layer's area. Each tile draws the
filtered instances that touch it, so tiles can repeat the same padded layer work. Separate tile
submissions can retain all those intermediates concurrently. New pointer updates replace the
pending batch already, but prior submissions and finished textures can still consume memory.

Exact preview pictures expire when their keys leave the visible preview. The background worker
checks again before rasterizing each tile and releases stale results as they finish. The stage
keeps one displayed replacement per missing tile while newer tiles arrive. The display owns its
texture handle independently of the render cache.

Filtered window textures and headless readbacks share an allowance of 64 MiB on integrated,
virtual, and unclassified devices, or 128 MiB when the adapter reports a discrete GPU. A scoped
allocation failure brings the discrete allowance back to 64 MiB for that renderer. The estimate
counts float layers, coverage, paints, tile lists, edges, per-pass parameters and ramps, GPU
binning buffers, and the output texture or both readback buffers. Small buffers are charged
at least one 4 KiB page to account for allocation overhead. Bitmap and finished tile caches
have their own bounds. Driver allocation policy, window resources and other applications can
add pressure beyond those estimates. Allocator
reports are optional diagnostics. Per-resource texture and buffer limits are checked
independently, and no driver ID or reported VRAM size sets the allowance.

The accounting retires submissions after their queue fence completes on native wgpu backends.
The browser stage uses the CPU, and the budget rejects filtered GPU work on browser WebGPU,
where a blocking poll cannot fence a submission. A tile that exceeds the allowance renders on
the CPU with the same filter stack and quality. The GPU probe holds up to 40 textures to model
one visible set and one incoming set. Blur's isolated provisional pictures retain their separate
cache and exact redraw after the gesture.

| Ten instances at 2× | Peak live MiB, before → after | Filter scratch MiB, before → after |
|---|---:|---:|
| Drop Shadow, Low | 421 → 53 | 15 → 8 |
| Drop Shadow, High | 440 → 106 | 28 → 47 |
| Glow, High | 456 → 120 | 39 → 49 |
| Drop Shadow + Glow, Low | 447 → 112 | 33 → 51 |
| Drop Shadow + Glow, High | 528 → 113 | 102 → 55 |
| Drop Shadow + Glow, High, Radial to Linear | 532 → 113 | 102 → 55 |

The mixed High switch case reserved 768 MiB before and 256 MiB after. Its live peak in the
latter half of the fixed run was 111 MiB. Individual scratch readings reflect where the driver
was in the queue when sampled, so a lighter trial can report more live scratch after the change.
The original memory probe completed successfully. The reported device loss therefore remains
dependent on the actual graphics budget and concurrent use of the window device.
The switch case's median tile-batch call time was 27 ms before and 81–119 ms in two fixed runs.
The fixed runs also overlapped other validation work, and their call times include waits for
room in the GPU queue. The timings cover the synthetic batch calls, whose submissions can
queue ahead of painting.

The separate editor regression used the real shared Vulkan device with default Array settings.
It ran eight Radial-to-Linear switches and 80 bar pointer moves with reversals and stage-boundary
crossings. Over two runs, pointer frames took 2.42–2.78 ms p50 and 3.85–5.14 ms p95. The driver
reported live peaks of 14.1–15.9 MiB, with the same peaks in the latter halves of the runs. Exact
tiles settled after each release, including after Undo/Redo, Cancel and Commit. The tile probe
above is a separate, higher-pressure workload.

The portability review repeated the same 2× pressure probe on this Intel Arc adapter with
Vulkan and Direct3D 12. The mixed High switch case peaked at 114 MiB live on Vulkan and
82 MiB on Direct3D 12. Each reported 256 MiB in reserved allocator blocks. Their median
batch calls took 72 ms and 238 ms respectively in these runs. The measurements describe
these two driver paths on one Windows machine. Discrete and unknown adapter policies are
covered by mocked budgets. An OpenGL window also displayed the mixed-filter Array using
readback from a separate Vulkan device on the same adapter.

The 80-move editor regression passed on both APIs. Vulkan pointer frames measured 3.06 ms p50
and 5.56 ms p95, and driver-reported live allocations peaked at 12.5 MiB both overall and in
the latter half. Direct3D 12 measured 2.43 ms p50 and 5.73 ms p95, with a 46.9 MiB overall peak
and a 34.2 MiB latter-half peak. The test waits for GPU startup separately, so these timings
start once the stage is rendering on the selected device.

The Bevel milestone adds a repeat of the editor regression with ten instances, each with
High-quality Blur, Drop Shadow, Glow, and Bevel. On the same Windows Intel Arc adapter,
Vulkan measured 4.06 ms p50 and 7.99 ms p95, with a 39.5 MiB live allocation peak overall and
in the latter half. Direct3D 12 measured 3.98 ms p50 and 6.03 ms p95, with 112.2 MiB overall and
106.9 MiB in the latter half. Both runs settled to exact stage tiles after each drag. Set
`WGPU_BACKEND` to `vulkan` or `dx12` and run
`cargo test --locked high_quality_bevel_stack_gpu_drag_lifecycle -- --ignored --nocapture --test-threads=1`
to repeat the test.

The Gradient Bevel review repeated the editor workload with ten instances, each with High-quality
Blur, Drop Shadow, Glow, Bevel, Gradient Glow, and Gradient Bevel. On the Windows Intel Arc adapter,
the first Direct3D 12 run exceeded the test's 15-second stage-settle deadline. Two reruns on the
same adapter completed all eight Radial-to-Linear switches and 80 pointer moves, including one
after the other verification workloads had finished. Pointer-frame p95 was 10.51 ms on the first
successful rerun and 6.91 ms on the isolated rerun. The Vulkan run completed with a 15.97 ms
pointer-frame p95. The cause of the initial timeout remains undetermined.

During Circle and Spiral UI verification, `stage_work_is_submitted_only_inside_the_frame`
failed in the parallel workspace run at the assertion that submissions wait between frames.
The test passed in a process by itself. The rest of the workspace suite and the exhaustive
command fuzzer passed separately.

To exercise the failing paint path separately, run a **debug build** with isolated logs and
recovery files. The fixture destroys AniCraft's test device just before the painter runs.
The replacement process should open the **Recover unsaved work** dialog. The Vulkan and
Direct3D 12 runs showed the staging-buffer panic, followed by the handoff before cleanup could
unwind further. The recovery copy contains the source artwork and its filters.

```powershell
$env:ANICRAFT_LOG_DIR = "$pwd\target\array-gpu-loss-logs"
$env:ANICRAFT_RECOVERY_DIR = "$pwd\target\array-gpu-loss-recovery"
cargo run --locked -- --renderer vulkan --open array-gpu-loss-probe --screenshot shots/array-gpu-recovery.png
```
### Gradient selection stipple (#295)

The selection overlay inverts the artwork's 256-entry gradient ramp with alpha ignored. It
checks the colour range and midpoint of each 64-device-pixel strip before testing the pixels
covered by dots. Sharp lookup transitions and a possible radial minimum inside a strip trigger
refinement. One frame shares a limit of 131,072 pixel probes and 16,384 extra rectangles over
all gradient fills. Each fill reserves some of its allowance for later scanline spans.

After a span or frame uses its allowance, the remaining strips blend their corner colours.
This keeps the entire selected area covered. Some narrow transitions can be softened or missed
on busy fills. Features missing from the artwork's lookup table also stay out of the stipple.

Two optimized CPU benchmark runs used identical pre-clipped 3840 × 2160 spans, with 31 timed
mesh builds per implementation after warm-up. The 14-stop elliptical radial had an offset
focal point, two clusters of close stops, a 31° rotation, and non-uniform scale. Times are the
range of the two medians and include mesh allocation. Span preparation and drawing are outside
the timings.

| Spread | Original mesh | Pixel-by-pixel adaptive mesh | Bounded mesh |
|---|---:|---:|---:|
| Pad | 32,460 rectangles, 9.1–10.7 ms | 54,992 rectangles, 106.2–126.2 ms | 48,405 rectangles, 10.5–12.3 ms |
| Reflect | 32,460 rectangles, 15.2–18.9 ms | 68,675 rectangles, 211.2–225.2 ms | 48,844 rectangles, 18.5–22.1 ms |

The bounded mesh took 1.15–1.21× the original CPU time in these runs. The reflected fill can
still exceed a 60 fps frame budget on this machine.

### How budgets are enforced

* **Budgets that don't depend on the machine** (undo memory, snapshot sharing, file size,
  culling) are a test (`machine_independent_budgets` in `crates/cli`), so the CI gate fails
  any PR that breaks them.
* **Timing budgets** are `cargo run --release -p anicraft-cli -- bench --check yes`. It
  prints every measurement next to its budget and fails when one is missed. Shared CI
  runners vary too much in speed and core count to gate on time, so this runs on a known
  machine before a release and after performance work. A dedicated benchmark runner would
  let CI run it too.
* The limits live in `TIME_BUDGETS` in `crates/cli/src/lib.rs`; keep that list and this
  table in step. The GPU stage's are `BUDGETS` in `crates/gpu/examples/gpu_bench.rs`
  (`--check`; the CLI has no GPU).

## Animated PNG and WebP (2026-11-11, ADR 0063)

The sample, all 48 frames, release build, Windows 10, 32 threads
(`cargo test --release -p anicraft-engine anim::tests::sizes -- --ignored --nocapture`).
Times include rendering every frame.

| Export | 1280 × 720 | Time | 640 × 360 | Time |
|---|---|---|---|---|
| Animated GIF (256 colours, 1-bit alpha) | 1464 KB | 421 ms | 660 KB | 110 ms |
| Animated PNG | 2019 KB | 183 ms | 741 KB | 52 ms |
| Animated WebP, lossless | 1829 KB | 130 ms | 666 KB | 36 ms |

The sample moves something in every frame, so all 48 frames are stored. Its sky is a full-stage
shape, so the transparent and stage-colour exports come out the same size. Compressing each
APNG frame at the `png` crate's best level instead of its default brought the file from 3387 KB
to 2019 KB for 25 % more time. The frames compress in parallel, which is why both formats are
faster than GIF, whose LZW coding runs on one thread.

## Sound (2026-11-09, ADR 0020)

Release build, Windows 10, one 120-second 48 kHz stereo sine, through `anicraft-cli run`
(times include starting the CLI and loading the sample):

| Step | Time |
|---|---|
| Import + decode WAV (23 MB) | 129 ms |
| Import + decode Ogg Vorbis (420 KB) | 147 ms |
| Import + decode MP3 (2.9 MB, 192 kb/s) | 99 ms |
| `file.export_audio`, 120 s at 48 kHz (exact copy path) | 95 ms |
| `file.export_audio`, 120 s at 44.1 kHz (Catmull-Rom resampling) | 128 ms |

Mixing runs about 1,000× faster than real time, so the audio callback's share of a 10 ms device
buffer is tiny; the stage never waits on sound.

## Decisions

* [ADR 0001](adr/0001-undo-persistent-snapshots.md): persistent document and snapshot undo
* [ADR 0002](adr/0002-render-pipeline.md): render worker, frame cache, band parallelism, culling
* [ADR 0003](adr/0003-file-format-v2.md): compact file format with shared paths
* [ADR 0015](adr/0015-crash-recovery-autosave.md): crash-recovery autosave on a worker thread
* [ADR 0019](adr/0019-gpu-stage.md): GPU stage renderer (first slice), parity with the oracle
* [ADR 0029](adr/0029-gpu-stage-direct-masks-groups.md): GPU stage without readback, masks and
  groups on the GPU, binning kept on the CPU
* [ADR 0032](adr/0032-gpu-stage-bitmaps-filters-default.md): bitmaps and filters on the GPU,
  Direct3D 12, and the GPU as the default stage renderer
* [ADR 0048](adr/0048-gpu-binning-and-hairlines.md): tile binning on the GPU, one tile format
  for both binnings, and hairlines for outline mode
