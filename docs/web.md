# The web editor

AniCraft also runs in a web browser: the same program, built for WebAssembly
(`wasm32-unknown-unknown`), drawing into a page with WebGL2. How it is built, and why, is in
[ADR 0036](adr/0036-web-build.md), and how it is checked in a real browser in
[ADR 0137](adr/0137-the-web-editor-is-checked-in-a-real-browser.md). Nothing is hosted yet and the
releases don't carry it yet (see [Releases](#releases)); you build it and serve it yourself. `packaging/web/package.sh` builds it and zips it as the release
download `dist/release/anicraft-web-<version>.zip`, the static site to unzip onto a web server.

## Build

You need the wasm32 target and the `wasm-bindgen` command-line tool, at the version
`Cargo.lock` pins for the `wasm-bindgen` crate (the script checks and tells you which). Both are
MIT OR Apache-2.0.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked   # the version build-web asks for

scripts/build-web.sh          # macOS, Linux, Git Bash
scripts\build-web.ps1         # Windows PowerShell
```

`--dev` (`-Dev`) builds faster but makes a larger, slower module. The output goes to `web/`
(git-ignored):

| File | What |
|---|---|
| `index.html` | the page (from `packaging/web/index.html`) |
| `anicraft.js` | wasm-bindgen's loader |
| `anicraft_bg.wasm` | the editor, about 21.5 MB (6.8 MB gzipped) |
| `anicraft.png` | the page icon |
| `guide/` | the user guide as HTML pages (`anicraft-cli guide`), opened by Help › User Guide |

A plain `cargo build --target wasm32-unknown-unknown` (the whole workspace, default features)
also works: that is what CI checks.

## Serve locally

Browsers don't load WebAssembly modules from `file://`, so serve the folder over HTTP with any
static server, for example:

```sh
python3 -m http.server --directory web 8080
```

Then open <http://localhost:8080>. `localhost` counts as a secure page, so Web Locks (used by
autosave) work. On another host, serve over HTTPS for the same reason. Compressing
`anicraft_bg.wasm` (gzip or brotli) cuts the download to about a third.

## How it differs from the desktop app

* **Opening and importing** use the browser's file picker. **Saving and exporting** download
  the file: Save asks for the file name the first time, then downloads under that name. The
  browser picks the folder, usually Downloads. An export of several files (a PNG or SVG
  sequence, a sprite sheet with its frame map, a publish) arrives as one ZIP named after its
  folder, `frames.zip`: a browser drops most of a run of separate downloads.
* **A file you open always opens**, in a tab of its own, even when a document of the same name
  is open: the browser tells the page a file's name and nothing else, so two different files
  can share one.
* **Autosave** keeps recovery copies in the browser's storage for the site. If a tab closes or
  crashes with unsaved work, the next start offers to restore it, as the desktop app does after
  a crash. Copies of tabs that are still open are never offered. Browsers keep only a few MB per
  site, so documents with large images or sounds may not fit; the status bar says when a copy
  fails.
* **Dropping files** on the page works as on the desktop (ADR 0059): documents open, drawings,
  images and sounds land where they are dropped on the stage or in the Library. An XFL folder
  can't be dropped (the browser hands over files, not folders): drop the `.fla`.
* **Preferences** are kept in the browser's storage too, with Open Recent and the start
  screen's pictures. A recent file reopens while the page still holds it; after a reload it
  shows as missing (the browser never gives its path back): open it again with Open.
* **Leaving the page** with unsaved changes asks first.
* **Shortcuts** are the desktop ones; on a Mac they use ⌘. The editor's keys are the editor's:
  F5 inserts a frame and Ctrl+R imports, where a page would reload, and Alt+F opens the
  editor's File menu. Keys the editor doesn't use stay the browser's (F12, Ctrl+T, Alt+D), and
  so do the keys you unbind in Edit › Keyboard Shortcuts. No page can have Ctrl+N, Ctrl+T or
  Ctrl+W (⌘ on a Mac): the browser keeps them, so use File › New and File › Close, or the
  command palette (Ctrl+K). To reload the page, use the browser's button.
* **Sound playback** uses Web Audio (ADR 0095): playback, scrubbing, previews and Test Movie,
  with the playhead following the sound. The browser opens sound on the first play (a click).
  Where a browser has no Web Audio, playback is silent with a notice.

## Not in the web editor yet

| Missing | Why, and what to do meanwhile |
|---|---|
| Video export | It runs ffmpeg, which a page can't start. The menu item is dimmed; export a GIF, PNG sequence or HTML page. |
| GPU stage | The stage draws on the CPU, on one thread. The GPU renderer builds for the web but isn't wired to WebGPU yet. |
| Installing plug-ins | Plug-ins live in a folder on the computer. Use the desktop app. |
| Fonts on your computer | A page can't see them. Text uses the bundled faces. |
| XFL folders | A browser can't hand over a folder. Open the `.fla` instead. |
| Large autosaves | Limited by the browser's storage (about 5 MB). |
| Parallel rendering | The web has no threads here, so exports render one frame at a time. |

## Releases

`packaging/web/package.sh` makes `dist/release/anicraft-web-<version>.zip`, the static site to
unzip onto a web server, and the Web CI job runs it on every pull request. Releases will carry
it once `has-web` is switched on in `.github/workflows/release.yml` (the family release workflow
then runs that script and attaches the zip); that is a workflow change of its own
([#316](https://github.com/storytold/anicraft/issues/316)).

## What was verified, and how

**In a real browser** ([ADR 0137](adr/0137-the-web-editor-is-checked-in-a-real-browser.md)):
`scripts/web-check.sh` (`.ps1` on Windows) builds `web/` and runs `tests/web_browser.rs`, which
serves the folder on this computer and drives headless Chrome, Chromium or Edge as an artist
would, by keyboard: the editor starts; Save downloads the document; a file with the open
document's name opens through the browser's picker; Import to Stage (Ctrl+R) brings a picture
in; F5 inserts a frame and the page is not reloaded; a PNG sequence from the command palette
arrives as one ZIP; after a reload (which asks first, with unsaved work) the preferences are
read back and one edit leaves a recovery copy in the site's storage; and the page reported no
error. It takes about 20 s after the build.

```sh
scripts/web-check.sh                                   # build, then check
ANICRAFT_WEB_DIR=unzipped-release scripts/web-check.sh # check a bundle you already have
ANICRAFT_BROWSER=/usr/bin/chromium scripts/web-check.sh
```

Not checked by it: Firefox and Safari, sound, drag and drop from the desktop, and how the
canvas looks. To check those by hand: build, serve, then in the page open a `.anicraft` file,
draw, save (a download arrives), close the tab with unsaved work, reopen it (Restore is
offered), and export a GIF.

**Without a browser**, as before:

* every crate builds for wasm32, and clippy is clean for it;
* `crates/engine/tests/wasm.rs` runs inside WebAssembly in Node: a frame renders with no
  worker thread, a document saves and reopens through memory, PNG/GIF/HTML/PNG-sequence exports
  are written, video export refuses, and a plug-in runs on wasmi;
* the built editor module loads in Node's V8 and runs `main` to its "no canvas" exit without a
  panic;
* natively, `crates/engine/src/web_tests.rs` runs every file command against the in-memory
  store the browser uses.

Run the WebAssembly tests yourself (Node and the wasm-bindgen CLI installed):

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test -p anicraft-engine --target wasm32-unknown-unknown --test wasm
```
