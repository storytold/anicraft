# Runs the web editor in a real browser (ADR 0137, docs/web.md): builds web/ and drives it in
# headless Chrome or Edge through tests/web_browser.rs. The PowerShell twin of web-check.sh.
#
#   scripts\web-check.ps1                                   # build web\, then check it
#   $env:ANICRAFT_WEB_DIR = 'unzipped'; scripts\web-check.ps1   # check a bundle already built
#   $env:ANICRAFT_BROWSER = 'C:\path\to\chrome.exe'             # choose the browser
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)
if (-not $env:ANICRAFT_WEB_DIR) {
    & (Join-Path $PSScriptRoot 'build-web.ps1')
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
cargo test -p anicraft --test web_browser -- --ignored --nocapture
exit $LASTEXITCODE
