#!/usr/bin/env bash
# Runs the web editor in a real browser (ADR 0137, docs/web.md): builds web/ and drives it in
# headless Chrome, Chromium or Edge through tests/web_browser.rs (start, save, open, import,
# export a sequence, the editor's shortcuts, preferences and autosave across a reload).
#
#   scripts/web-check.sh                                  # build web/, then check it
#   ANICRAFT_WEB_DIR=unzipped scripts/web-check.sh        # check a bundle already built (a release zip, unzipped)
#   ANICRAFT_BROWSER=/path/to/chrome scripts/web-check.sh # choose the browser
#   ANICRAFT_BROWSER_ARGS=--no-sandbox scripts/web-check.sh   # in a container
#
# Nothing keeps running afterwards: the test serves the folder on 127.0.0.1 and closes the
# browser itself.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
[[ $# -eq 0 ]] || { echo "usage: scripts/web-check.sh" >&2; exit 2; }
if [[ -z "${ANICRAFT_WEB_DIR:-}" ]]; then
  scripts/build-web.sh
fi
cargo test -p anicraft --test web_browser -- --ignored --nocapture
