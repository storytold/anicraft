# ADR 0137: The web editor is checked in a real browser, and keeps its keys and its files there

* Status: accepted (2026-11-27)
* Relates to: backlog item P-007 (`docs/roadmap.md`), issues
  [#55](https://github.com/storytold/anicraft/issues/55),
  [#56](https://github.com/storytold/anicraft/issues/56),
  [#316](https://github.com/storytold/anicraft/issues/316) and
  [#210](https://github.com/storytold/anicraft/issues/210), ADR 0036 (the web build), ADR 0015
  (autosave), ADR 0060 (publish), ADR 0064 (the menu bar from the keyboard), ADR 0112 (several
  documents), `docs/web.md`

## Context

ADR 0036 built the editor for the browser and tested it everywhere but in one: every crate
builds for wasm32, the engine's tests run inside WebAssembly in Node, and the file commands run
natively against the in-memory store. `docs/web.md` said so: "There was no browser on the build
machine." #55 ("import, export and save do not work") and #56 ("preferences are not saved")
were filed before that slice, and nobody could say from a test whether they still held.

So we ran the optimized bundle in headless Chrome 158 and did what an artist does. Most of it
worked: the editor starts (1.7 to 1.9 s to the first frame here), Save downloads the document,
Open and the imports use the browser's picker, ten of the eleven exports on the menu download
(the sample's sprite sheet is past its pixel limit and says so; video is dimmed), preferences
survive a reload, and a reload after the autosave interval offers the copy. Three
things did not, and none of them could have been seen without a browser:

* **A file with the name of an open document would not open.** The shell asks "is this file
  already open?" by path, and in a browser a picked file's path is its name. Picking
  `animation.anicraft` while a document of that name was open showed "already open" and threw
  the picked bytes away: a backup from another folder, or the same file edited since, could
  not be opened at all.
* **Sequences lost most of their frames.** A PNG or SVG sequence went out as one download per
  frame. A browser lets a page start only a few downloads in a row and drops the rest without
  telling the page: of a 48-frame sequence Chrome kept 18 PNG files in one run and 10 SVG
  files in the next, and the status bar said the export had worked.
* **The browser acted on the editor's shortcuts.** eframe keeps a handful of keys from the
  browser (Ctrl+S, Ctrl+O, Ctrl+P, Tab, the arrows) and leaves the rest, F5 and Ctrl+R by
  name. F5 is Insert Frame, the key an animator presses most: in the web editor it reloaded
  the page, at once when the document was saved. Ctrl+R (Import to Stage) did the same, and
  Ctrl+D, Ctrl+G, Ctrl+L, Ctrl+U, F6, F7 and Alt+F went to bookmarks, find, the address bar,
  the page source, the browser's toolbar and its menu.

## Decision

1. **The web editor has a check that runs it in a real browser** (`tests/web_browser.rs`, run
   by `scripts/web-check.sh` after `scripts/build-web.sh`). It serves the built `web/` folder
   on 127.0.0.1, starts Chrome, Chromium or Edge headless with a fresh profile, and drives the
   page over the DevTools protocol: start, Save, Open (a file with the open document's name),
   Import to Stage, F5, a PNG sequence from the command palette, then a reload to see that
   preferences are read back and autosave keeps a copy.
   * **Rust, standard library only.** Rule 11 keeps the repository free of handwritten
     JavaScript, and the protocol needs little: an HTTP answer per file and a WebSocket client
     (RFC 6455) that sends masked text frames and joins the frames it reads, about a hundred
     lines, to this computer only. No new dependency.
   * **It reads only what the browser itself shows**: the downloads the page started (from the
     browser's download events, read when complete), the site's storage, the file picker
     opening, the leave-page question, whether the page was reloaded, and errors the page
     reports. It never reads pixels and never clicks at a coordinate: every step is a key an
     artist presses (Ctrl+S, Ctrl+O, Ctrl+R, F5, Ctrl+K and a command's name), so a change to
     the layout does not break it.
   * **It is not part of `cargo test`.** It needs a six-minute wasm build and a browser, so it
     is marked `#[ignore]` with the reason and the script runs it; a missing bundle or browser
     fails with what to do, never passes quietly. `ANICRAFT_WEB_DIR` points it at any bundle,
     so the same check runs on an unzipped release asset.

2. **In a browser a picked or dropped file always opens, in a tab of its own.** `file.open`
   there never counts as "already open": the name is all the page knows of the file. Open
   Recent still shows the open tab, since that is the copy the page holds.

3. **What one command writes leaves as one download**: the file itself, or one ZIP when there
   are several (`platform::as_one_download`, the archive Publish already made, ADR 0060). A
   sequence arrives as `frames.zip` with its folder inside, a sprite sheet with its frame map
   as `sprites.zip`. The archive is named after the folder the files share, else the first
   file. If the files don't fit a ZIP (65,535 files, 4 GB) they go one by one after all and
   the status bar says why.

4. **The editor takes from the browser the keys it acts on, and only those**
   (`platform::claims_key`, called from a key listener on the window in `web.rs`). The list is
   the shortcut table as the user set it, the fixed keys (F1, Ctrl+K, Ctrl+U, Ctrl+,), and on
   Windows and Linux F10, Alt alone and Alt with a menu's letter (ADR 0064). The browser's own
   action is called off; the key reaches the editor as before.
   * **Everything else stays the browser's**, so the keyboard is never trapped in the page:
     F12, Ctrl+T, Ctrl+Tab, Alt+D and whatever the user unbinds.
   * **Typing is never taken**: a key without Ctrl, ⌘ or Alt that is not a function key is
     left alone (calling it off would stop text reaching a text field), as is anything typed
     with AltGr. **Cut, copy and paste are never taken**: they arrive as the browser's
     clipboard events.
   * A browser does not let any page have Ctrl+N, Ctrl+T or Ctrl+W (⌘ on a Mac): New and Close
     are in the File menu and the command palette there, and `docs/web.md` says so.

5. **Release packaging stays the team's.** The app side is done: `packaging/web/package.sh`
   makes `dist/release/anicraft-web-<version>.zip` and the Web CI job runs it on every PR.
   What the workflows need, each in a PR of its own (rule 17): `has-web: true` in
   `.github/workflows/release.yml`, which makes the family release workflow run that script
   and attach the zip (#316, #210); and one line in `.github/workflows/web.yml` after the
   packaging step, `ANICRAFT_WEB_DIR=web cargo test -p anicraft --test web_browser --locked --
   --ignored` (on a runner that has Chrome or Chromium; in a container add
   `ANICRAFT_BROWSER_ARGS=--no-sandbox`).

## Consequences

* #55 and #56 are answered by a test instead of a reading of the code: Save, Open, Import and
  Export work in Chrome, and preferences are written to and read from the site's storage.
* The check takes 21 s once the bundle is built (five runs in a row here, all green). Against
  the bundle built from the code before this change it fails at the second step, with the
  first document saved again in place of the picked one.
* A sequence of two or more frames in the browser is now an archive. One more step to
  unpack it, and no frame is lost.
* An artist who pressed F5 to reload the page uses the browser's button; the editor says
  nothing about it because nothing happens that needs saying: a frame is inserted, as on the
  desktop.

## Not in this slice

* The check runs by hand and from the script; CI runs it once the workflow line above is in.
* Firefox and Safari: the check drives Chromium browsers only (the DevTools protocol). The
  key listener and the ZIP use nothing specific to one browser.
* A copy written when the tab is hidden or closed (autosave still waits for its interval or
  its count of edits), a real "Save" to a chosen file through the File System Access API,
  IndexedDB for large recovery copies, and the size of the download (21.5 MB here, 6.8 MB
  gzipped, built without the craft-fonts faces).
