# Principal-engineer review of AniCraft's own decisions (November 2026)

Reviewed at `main` 17626339 (2026-11-20): 205,000 lines of Rust in 14 crates and the shell, 115
ADRs, 2,257 tests. The question asked of every decision, ours included: is this how a principal
engineer would do it, and if not, what is the better way?

**The short answer.** The foundations hold up: the persistent document with pointer-shortcut
equality, the oracle renderer, the command registry, the bounded importers, the plug-in sandbox
and the automation roots are sound and unusually well measured. The weak points are at the
edges of those designs, where a rule is kept by a hand-maintained list instead of by the
compiler or by structure: which fields are compared, which state belongs to a document, which
parameters are paths, which tests may be slow. Seven findings are fixed in this pull request;
eighteen are backlog items for the lead to slot.

## How this was done

* Read by the reviewer: the engine's session, undo history, file format, `vfs` and automation
  roots, the render service, the document model's equality and evaluation limits, the command
  sweep, the layering tests, the tab switch, `docs/architecture.md` and the performance record.
* Four read-only passes by helper agents (tests, ADRs against code, security boundaries, the
  shell), whose claims are marked **[agent]** below unless the reviewer re-read the code
  (**[verified]**). An agent's claim is a lead, not a fact, until someone re-reads it.
* Measured on the shared build machine (Windows 10, 32 threads, NVMe), debug test profile.
  Other builds were running on it part of the time, so timings are indications; the ratios
  were reproduced.
* Not reviewed: the craft-libs crates (codecs, zip, XML, the `.anicraft` envelope), which are
  outside this repository; the GPU shaders; the web build beyond its `vfs`.
* Two findings are security-relevant. AGENTS.md rule 20 keeps those out of public text: they
  are in a private draft advisory on the repository and are not described here.

## Measurements

| What | Number |
|---|---|
| `cargo test --workspace`, warm dependencies, `-j 4` | 8 min 47 s: build 3 min 14 s (engine, gpu, cli, shell rebuilt), tests 325 s |
| Tests | 2,257 passed, 0 failed, 17 ignored, in 43 binaries (16 of them hold no test) |
| The engine's unit-test binary | 271 to 318 s of those 325 s |
| `tests::panic_hunt_every_command` alone | 318.0 s; the other 816 engine tests add up to 33.6 s |
| The same sweep as 17 tests, 4 test threads (after this PR) | the engine binary takes 91 to 105 s; all tests 146 s (2,275 passed) |
| One starting document of the sweep, alone / 2 at once / 4 at once / 17 at once | 19 s / 32 s / 36 s / 121 to 220 s each |
| Memory of the engine test process during the sweep | 0.85 GB (one test), 1.57 GB (17 at once) |
| `tabs::tests::a_document_that_is_not_shown_keeps_no_pictures` | failed 6 of 16 runs before this PR, 0 of 24 after |
| `fsync` of a saved document on this disk | 0.5 ms for 1 MB, 16 ms for 65 MB |
| Largest files | `src/stage.rs` 5,391 lines, `src/app.rs` 5,286, `src/panels.rs` 3,960 |
| Longest functions **[agent]** | `App::open_panel` 1,920 lines, `stage::show` 1,265, `timeline::show` 1,232 |
| craft-libs pins in `Cargo.toml` | 16 dependencies at 11 different `rev`s |

Render, edit, undo and save budgets were not re-measured: `docs/performance.md` records them
with their commands, `bench --check` enforces them, and a release build under the machine's
build cap was not worth the hour.

## Findings, by risk to correctness and reliability

Sizes: **S** a day or less, **M** a few days, **L** a design and more than a week.

### 1. An older build silently drops a newer file's data (L, backlog R-01)

* **Question.** The file format has a version, but only one feature ever raised it. What
  happens to everything else?
* **Evidence [verified].** `crates/engine/src/format.rs:23`: a document is written as version
  3 unless it has a mixed armature (version 4). `format.rs:295` refuses a higher version with
  a clear message. Every other feature since version 3 added `#[serde(default)]` fields at the
  same version (layer opacity, parent layers, warps, filter tracks, data tables, reference
  videos, soft masks…). Unknown fields are ignored on load, so a build older than the feature
  opens the file without it, and saving writes the file back without it. ADR 0118 §1 says so
  in a parenthesis; no ADR decides it. A new enum variant at the same version fails the whole
  parse with a generic "not a valid .anicraft file" **[agent]**.
* **Risk.** Silent, permanent loss of work, in exactly the case users hit: two machines, or a
  studio where not everyone updated. It grows with every release, and 0.3.0 is already out.
* **Better.** Write the features a document uses into the envelope (`"requires": ["warp",
  "data-tables"]`), computed like `version_for`. A build that doesn't know one refuses, or
  opens read-only, with a message naming it. That keeps ADR 0098's good property (a document
  that uses nothing new still opens everywhere) for every feature, not one. Cheaper stopgap
  (S): compare the envelope's `generator` version with this build's and warn before saving
  over a file a newer AniCraft wrote. Needs an ADR and a change in `craft-anicraft-format`.

### 2. A saved document was not flushed before it replaced the old one (S, fixed)

* **Evidence [verified].** `format::save` wrote a temporary file and renamed it over the
  document. `vfs::write_new` did not `fsync`; the crash-recovery copy did
  (`recovery.rs:149`). So the copy made in case of a crash was more durable than the save
  itself: after a power cut, a file system may have the rename and not the data, leaving an
  empty document where the old one was.
* **Fixed.** `vfs::write_new` syncs before returning. Cost measured above: 16 ms on a 65 MB
  file. Durability cannot be tested without cutting power; the change is three lines.
* **Left (S, R-02).** The audio export's temporary file (`cmds_sound.rs:367`) is renamed
  without a sync, and a save replaces a symbolic link with a plain file and resets the
  file's permissions. Both are worth one small PR with the decision written down.

### 3. A cleared render cache could be filled again by work in progress (S, fixed)

* **Evidence [verified].** `RenderService::forget_pictures` cleared the cache when another
  document was shown (ADR 0112: a document put aside keeps no pictures), but a batch the
  worker was drawing was inserted afterwards, and queued jobs for the old document still ran.
  The shell test that asserts the rule failed 6 runs in 16 on this machine with 4 test
  threads; it had been passing by timing.
* **Fixed.** The cache counts its clears; the worker drops a batch that began before one, and
  the queue is emptied. 0 failures in 24 runs, and a new engine test covers it directly.

### 4. Document equality was a hand-kept list of fields (S, fixed)

* **Question.** `Session::execute` decides "did this command change anything?" with
  `doc != before.doc`. Undo steps, the dirty flag, the state id and every render cache hang on
  that comparison. Who guarantees it looks at every field?
* **Evidence [verified].** `Shape`, `Keyframe`, `Layer`, `Timeline`, `Document`, `Bitmap`,
  `Sound`, `Warp` and `DepthKey` implement `PartialEq` by hand (rightly: the derive would not
  short-cut on shared `Arc`s). Each listed its fields as `self.a == o.a && …`. All of them
  were complete today (`Layer` has 21). A field added without its line would make an edit to
  it alone look like no change: no undo step, not dirty, stale pictures, and no test fails.
* **Fixed.** Each comparison now destructures `self` exhaustively, so a new field is a
  compile error until it is compared or explicitly ignored.
* **Noted.** `Layer` compares floats by bits; elements compare transforms with `==`, so a NaN
  transform makes the document unequal to itself and every command a recorded change. Load
  and save repair non-finite transforms, so this needs a bug elsewhere first.

### 5. Which state belongs to a document is a list, and the list has holes (M to L, R-03)

* **Evidence.** `App` has about 172 fields **[agent]**. ADR 0112 swaps a document's state on
  a tab switch through a macro listing 30 of them plus `settle()` (`src/tabs.rs:274`); the
  ADR rejected a per-document struct "for no behaviour". Whatever is not on the list leaks
  into the next document.
  * **[verified]** The History panel's picks and the Save As Command dialog number one
    document's undo steps. `sync_picks` (`src/macros_ui.rs:99`) kept them when the other
    document's history was as long, so Save As Command could save other steps than it listed.
    **Fixed** in this PR, with a test.
  * **[verified]** The live control port runs commands with `s.execute` directly
    (`src/remote.rs:127`): a remote `file.open` replaces the shown document in place, without
    `settle`, a tab or a view reset, and `poll` does not wait for an open modal dialog or
    text being typed (`remote.rs:73` checks drags and two dialogs).
  * **[agent]** Also not swapped or settled: the data-table view and Import Again's target
    (`app.rs:405-407`, consumed by the next `data.import_table` in whichever document is
    shown), Find's current hit and Library ids, the Edit Multiple Frames selection, and the
    open-in-place path (`tabs.rs:424-430`), which skips `settle()` so an open pen path or text
    edit carries into the opened file. A file dialog's answer carries no tab id
    (`app.rs:4736`). A parked document's autosaver is never polled, so a failed write shows
    only when its tab is shown again (`autosave.rs:296`).
* **Risk.** Wrong-document edits and saves. Each is small; the class keeps growing because
  nothing fails when a new per-document field is added.
* **Better.** Move per-document UI state into one `DocUi` struct that lives in the tab, so a
  switch swaps one value and a new field is per-document by where it is declared. Until then
  (S): a test that fails when `App` gains a field that is in neither the swapped list nor an
  explicit "belongs to the window" list. Route remote commands that open documents through
  `tabs::open`, and hold remote edits while `modal_open()` (S to M, R-04).

### 6. Command parameters are prose, so three safety nets are hand-kept lists (L, R-05)

* **Evidence [verified].** `CommandSpec::params` is a sentence (`lib.rs:142`). Consequences:
  * `access::PATH_PARAMS` lists which parameters are paths. The test that guards it finds
    path parameters by looking for the words `path`, `paths`, `dir`, `folder`, `data_home`
    and `appimage` in that sentence (`access_tests.rs:65`). A path parameter called `out` or
    `file` would pass unlisted. `vfs` still checks the access itself, so this costs the early,
    well-worded refusal, not the boundary.
  * The command sweep's hostile inputs are about 150 hand-written objects that each feature
    extends (`crates/engine/src/tests.rs`, `panic_hunt`). A command whose author forgot gets only the generic ones.
  * MCP clients get a description, not a schema, so an assistant learns parameter types by
    trial.
* **Better.** Describe parameters as data: name, type, range, optional, "is a path
  (read/write)". Generate the documentation sentence, the MCP JSON Schema, `PATH_PARAMS` and
  type-directed hostile values from it, and let commands read parameters through it. It can
  land namespace by namespace behind the existing string. Needs an ADR.

### 7. One test was 98 % of the engine's test time (S done; M left, R-06)

* **Evidence [verified].** See Measurements. `panic_hunt_every_command` ran 17 starting
  documents × every command × the hostile list × 2, on one thread: 318 s, against 33.6 s for
  the other 816 tests together.
* **Done.** The sweep is one test per starting document (`panic_hunt_every_command::<start>`,
  same documents, commands, inputs and final checks), so the test runner spreads them. The
  engine's tests take 91 to 105 s with 4 test threads, and the whole suite's tests 146 s instead of
  325 s.
* **Left.** It scales badly: one start takes 19 s alone but 36 s with four at once and over
  120 s with seventeen, with processors idle. Limiting rayon to one thread changed nothing
  (31 s for four), so it is not the thread pool; the process reaching 1.5 GB points at the
  allocator or at something large being cloned per command. Profile one start before
  guessing. A second question worth asking of the design: most of those 180,000 command runs
  per start reject their input in the first line, so sampling inputs by declared parameter
  type (finding 6) would find more with less.

### 8. About two dozen tests assert wall-clock time (S, own PR, R-07)

* **Evidence [agent; four read by the reviewer].** Rule 28 says tests should not assert
  timings. 23 default-run tests do, the tightest `skin_tests.rs:301` (`< 50 ms`, debug
  build), `i18n/tests.rs:555` (`< 20 µs` a lookup), `library_tests.rs:318` (`< 500 ms`,
  commented "CI varies"), and two-second bounds in `find`, `actions`, `plugins`, `morph` and
  `weights`. Five more are ignored `*_timings` benchmarks written as tests.
* **Risk.** They pass on a 32-thread workstation and fail on a loaded 4-core runner; the
  failure says nothing about the code. They are the likeliest source of red CI on `main`.
* **Better.** Replace each bound with a relation that does not depend on the machine: work
  counted (fuel, visited nodes, allocations) or "ten times the input takes less than
  twenty times as long". Budgets belong in `bench --check`. This cuts assertions, so by
  rules 31 and 32 it is a PR of its own that lists every one; nothing was changed here.

### 9. Tests that cannot run report success (S, R-08)

* **Evidence [agent].** 17 GPU parity tests return early when there is no adapter
  (`crates/gpu/src/tests.rs:791`), about 30 text tests when no font is found, three video
  tests without ffmpeg, the guide's search test without node. Each prints at most a line and
  counts as passed. Only two honour `CRAFT_FONTS_REQUIRED=1`; the sample plug-ins have the
  right pattern (`ANICRAFT_EXAMPLE_PLUGINS` set turns a missing module into a failure).
* **Risk.** The GPU-against-oracle guarantee, the project's second principle, holds in CI
  only while lavapipe keeps installing. If it stops, CI stays green and checks nothing.
* **Better.** One variable, set in CI, that turns every "skipped" into a failure
  (`ANICRAFT_TESTS_REQUIRE=gpu,fonts,ffmpeg,node`), read by one shared helper.

### 10. Undo memory is budgeted without the media (S, R-09)

* **Evidence [verified].** `history::unique_bytes` (`history.rs:323`) counts layers and
  symbols. Bitmaps and sounds are not counted, though a snapshot keeps their `Arc`s alive:
  importing and deleting a 50 MB sound leaves 50 MB held by a step the budget sees as a few
  hundred bytes. The budget is 512 MB a document and ADR 0112 accepts one budget per tab, 32
  tabs.
* **Better.** Count bitmap and sound bytes not shared with the neighbouring state (the same
  pointer test the layers use), and make the budget one for the process, shared by the tabs.

### 11. The automation policy is ambient, per thread (L, R-10)

* **Evidence [verified].** `access::confine` sets a thread-local policy; `vfs` consults it.
  A worker thread has none, so each place that leaves the command's thread must remember to
  ask first; `video.rs:756` does, with a comment saying why. Two engine modules use `std::fs`
  directly against the convention (`recovery.rs`, `cmds_desktop.rs`) **[agent]**.
* **Risk.** The boundary is correct today because every author remembered. The next worker
  thread that reads a file is unconfined by default, silently.
* **Better.** Deny `std::fs` in the engine with a clippy `disallowed-methods` list (two
  allowed modules), which makes the convention a build error; later, hand `vfs` an explicit
  capability instead of reading a thread-local. The first is S and worth doing soon.

### 12. `"confirm": true` is asked of the caller it is meant to stop (product decision, R-11)

* **Evidence [agent; ADR 0004 and 0104 §11].** Irreversible commands are refused unless the
  parameters say `"confirm": true`. Over MCP and the control port the caller writes that flag
  itself. It prevents accidents; it is not consent.
* **Better.** For commands that arrive over the control port, show the confirmation in the
  window and let the person answer. Headless `mcp` has no person, so there the roots are the
  protection and the flag stays. This changes what assistants can do unattended, so it is the
  lead's call.

### 13. External programs run without a time limit (S to M, R-12)

* **Evidence [agent, not re-read].** The ffmpeg probe (`video.rs:207`), export
  (`cmds_file.rs:744`, `child.wait()`) and one decode path have no timeout and one reads
  stderr without bound. `file.video_info` over the control port runs on the window's thread,
  so an input that stalls ffmpeg freezes the window.
* **Better.** One helper that runs a child with a deadline, bounded output and Cancel, used
  by every call; `video_info` moves to the worker that imports already use.

### 14. The window's thread still waits for slow things (M, R-13)

* **Evidence [agent].** Every command runs synchronously in the frame (`app.rs:1034`), so
  opening, saving or importing a large file stops drawing; publish, video import and plug-in
  export already run on workers. On Windows and Linux the native file dialog is called
  inline (`platform.rs:1032`), which also stops remote calls and autosave while it is up.
* **Better.** A small "slow command" path: the commands that touch files run on a worker
  against a snapshot and hand back a document, as publish does. The persistent document makes
  this cheap; it was built for it.

### 15. Integration tests are eleven binaries, not one per crate (S, own PR, R-14)

* **Evidence [agent; layout verified].** Rule 30 asks for `tests/it/main.rs`. No crate has
  it: the engine has seven binaries, xfl two, plugins two, the root one. 16 of the 43 test
  binaries cargo runs hold no test. `crates/engine/tests/xfl_corpus.rs` includes a file from
  another crate's test tree by path.
* **Better.** One `tests/it/main.rs` per crate with the present files as modules. It is a
  move, so its own PR (rule 8). Fewer links per `cargo test`, and shared helpers stop being
  `#[path]` includes.

### 16. The layering test did not read every dependency table (S, fixed)

* **Evidence [verified].** `src/architecture_tests.rs` read `[dependencies]`,
  `[dev-dependencies]` and `[build-dependencies]` only. `crates/cli/Cargo.toml` already has a
  `[target.'cfg(…)'.dependencies]` table, which it skipped: a crate could have named egui, or
  a higher layer, there.
* **Fixed.** Target tables and `[dependencies.name]` tables are read, with a test.

### 17. Screenshot scenes live inside the application (M, own PR, R-15)

* **Evidence [agent; file sizes verified].** `App::open_panel` is 1,920 lines of `--open`
  screenshot states inside `src/app.rs`, 36 % of the file. `stage::show` and `timeline::show`
  are over 1,200 lines each. 27 `#[allow(clippy::…)]` outside tests, most of them
  `too_many_arguments`, stand against rule 13.
* **Risk.** Review cost and merge conflicts, not correctness: three tracks edit `app.rs` at
  once. The function-size silences hide where a parameter struct is wanted.
* **Better.** Move the scenes to `src/shots.rs` (a pure move). Split `show` by the phases its
  own comments already name. Replace argument lists with small context structs as each
  function is next touched.

### 18. The edit loop rebuilds four crates (M, R-16)

* **Evidence [verified].** With `incremental = false` (a deliberate disk-space choice,
  `docs/development.md`), any edit in the engine rebuilds the engine (62,000 lines), gpu, cli
  and the shell (65,000 lines): 3 min 14 s at `-j 4` for the test profile. Tests compile into
  the same crates, so a test-only edit pays the same.
* **Better.** Measure first (`cargo build --timings`), then split along the seam that
  already exists: the session, history, format and `vfs` as an engine core; the `cmds_*`
  families above it. The shell's panels could follow. Worth doing only if the timings show
  the split crates dominate.

### 19. Temporary folders and helpers are re-invented per test (S, R-17)

* **Evidence [agent].** About 100 tests make `temp_dir()/anicraft-<tag>-<pid>`; three add a
  random suffix. A recycled process id finds another run's leftovers, and a panicking test
  leaves its folder. `fn run` is defined in about 80 engine test files; the font lookup four
  times. The golden self-test writes to one fixed path, so two `cargo test` runs sharing a
  target folder collide.
* **Better.** A small test-support module (a scratch folder that is unique and removed on
  drop, `run`, `fonts`) used by all of them.

### 20. Dependencies and ADR records (S each)

* **craft-libs pins [verified].** Rule 10 asks for one `rev`; `Cargo.toml` has 11. A `rev`
  move is its own PR (R-18).
* **ADR statuses [verified].** No ADR said it had been amended. Seven status lines now do
  (0009, 0015, 0029, 0040, 0054, 0092, 0098). Still open: ADR 0067 §1 names a
  `rust-toolchain.toml` that does not exist (CI's toolchain is whatever the shared workflow
  installs; `rust-version = "1.95"` is only a floor), and `docs/adr/README.md` explains one
  of nine gaps in the numbering.
* **`docs/architecture.md` [verified].** It described format version 2, a CPU-drawn stage,
  planned crates that became others, and left five crates out of its map. Corrected here.

## Questioned and found sound

Worth saying, because a review that only lists faults misleads:

* **Undo as persistent snapshots (ADR 0001).** Equality and memory accounting ride on shared
  pointers, measured at 0.04 µs a snapshot and 2.7 KB a small edit.
* **Evaluation limits.** `MAX_EVAL_ELEMENTS` exists because someone noticed that a depth cap
  of 32 still allows 2³² expansions (`eval.rs:21`).
* **The plug-in sandbox [agent].** Default-deny command policy, fuel, memory and deadline on
  every entry point, guest lengths capped before copying, no route to file commands.
* **The control port [agent].** Loopback only, a 256-bit token compared in constant time
  before dispatch, bounded lines and connections; a browser's request is refused as not JSON.
* **Automation roots [agent; resolve path read].** Canonicalised roots, component-wise
  containment, Windows device and share spellings refused before the disk is asked, and the
  executable-path parameter refused for agents. Its accepted limits are written down.
* **Panics.** Crates deny `unwrap`, `expect` and `panic!` outside tests; the agents' fifteen
  importer candidates and the shell's were all guarded where read.
* **`BASE_VERSION == 3` asserted in a test.** It looks like a constant test; it pins a value
  an outside crate's `rev` bump could change, which would lock older builds out. Keep it.
* **The performance record.** Budgets with dates, machines, commands and enforcement.

## Fixed in this pull request

| Commit | Finding |
|---|---|
| `fix(doc)`: equality names every field | 4 |
| `fix(engine)`: a saved document is on the disk before it replaces the old one | 2 |
| `fix(engine)`: pictures still being drawn are not kept once forgotten | 3 |
| `fix(tabs)`: History picks and Save As Command stay with their document | 5 |
| `test(architecture)`: the layering check reads target and per-dependency tables | 16 |
| `test(engine)`: the command sweep is one test per starting document | 7 |
| `docs`: architecture.md and seven ADR status lines | 20 |

## Backlog, in the order proposed

| Id | Size | What |
|---|---|---|
| R-01 | L | Required-features list in the `.anicraft` envelope; refuse or open read-only (ADR) |
| R-03 | M–L | Per-document UI state in one struct owned by the tab; first a test for unlisted `App` fields |
| R-04 | S–M | Remote commands that open documents go through tabs; remote edits wait for modals |
| R-07 | S | Replace wall-clock assertions with relations (own PR, lists each) |
| R-08 | S | `ANICRAFT_TESTS_REQUIRE` in CI: a skipped GPU, font, ffmpeg or node test fails |
| R-09 | S | Undo budget counts bitmaps and sounds; one budget for all tabs |
| R-10 | S, then L | Deny `std::fs` in the engine by lint; later an explicit file capability |
| R-12 | S–M | Deadlines and bounded output for every external program |
| R-05 | L | Typed command parameters: schema, generated docs, path table and hostile inputs (ADR) |
| R-06 | M | Profile the command sweep's contention; sample inputs by type |
| R-13 | M | File commands off the window's thread |
| R-02 | S | Sync the audio export's temporary file; decide links and permissions on save |
| R-14 | S | One integration-test binary per crate (move-only PR) |
| R-17 | S | Shared test support: scratch folders, `run`, fonts |
| R-15 | M | Screenshot scenes out of `app.rs`; split the 1,200-line `show` functions (move-only PRs) |
| R-16 | M | Measure crate build times; split the engine if they say so |
| R-18 | S | craft-libs at one `rev` |
| R-11 | decision | Confirmation in the window for irreversible commands over the control port |
