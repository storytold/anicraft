//! The web editor in a real browser (ADR 0137, docs/web.md): what an artist does there, end to
//! end, in headless Chrome or Edge driven over the DevTools protocol. It serves the built
//! `web/` folder on this computer, then starts the editor, saves (a download), opens a file
//! (the browser's file picker), imports, exports a sequence, presses the editor's shortcuts, and
//! reloads to see preferences and autosave kept. Every check reads what the browser itself
//! shows: the downloads folder, the site's storage, whether the page was reloaded.
//!
//! It needs the built bundle and a browser, so it is not part of `cargo test`:
//! `scripts/web-check.sh` builds `web/` and runs it. `ANICRAFT_WEB_DIR` names another bundle
//! (an unzipped release asset), `ANICRAFT_BROWSER` the browser, and `ANICRAFT_BROWSER_ARGS`
//! adds arguments (`--no-sandbox` in a container).
//!
//! Only the standard library: a few lines of HTTP for the site and of WebSocket (RFC 6455) for
//! the protocol, both to 127.0.0.1 only.
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

// ----- the site ---------------------------------------------------------------------------------

/// Serves `root` on 127.0.0.1 for as long as the test runs; returns its address.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let addr = listener.local_addr().expect("its address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || answer(stream, &root));
        }
    });
    format!("http://{addr}/")
}

fn answer(mut stream: TcpStream, root: &Path) {
    let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    // The rest of the request: headers up to the empty line.
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
        line.clear();
    }
    let path = first.split_whitespace().nth(1).unwrap_or("/").split(['?', '#']).next().unwrap_or("/");
    let mut file = root.to_path_buf();
    for part in path.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..") {
        file.push(part);
    }
    if file.is_dir() {
        file.push("index.html");
    }
    let kind = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    };
    let _ = match std::fs::read(&file) {
        Ok(body) => stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).and_then(|()| stream.write_all(&body)),
        Err(_) => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    };
}

// ----- the browser ------------------------------------------------------------------------------

fn find_browser() -> PathBuf {
    if let Some(b) = std::env::var_os("ANICRAFT_BROWSER") {
        return PathBuf::from(b);
    }
    let fixed = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ];
    let named = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"];
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).flat_map(|dir| named.map(|n| dir.join(n)));
    fixed.iter().map(PathBuf::from).chain(on_path).find(|p| p.is_file()).expect("no Chrome, Chromium or Edge found: set ANICRAFT_BROWSER to one")
}

/// The browser process; it and its profile folder go when the test ends, passed or failed.
struct Browser {
    child: Child,
    dir: PathBuf,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The browser lets go of its profile a moment after it dies.
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.dir).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Starts the browser headless with a fresh profile under `dir`; returns it and the address of
/// its DevTools WebSocket.
fn launch(dir: &Path) -> (Browser, String) {
    let exe = find_browser();
    let extra: Vec<String> = std::env::var("ANICRAFT_BROWSER_ARGS").map(|a| a.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    let mut child = Command::new(&exe)
        .args(["--headless=new", "--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--window-size=1400,900"])
        // WebGL2 without a graphics card (a CI runner).
        .arg("--enable-unsafe-swiftshader")
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .args(extra)
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("could not start {}: {e}", exe.display()));
    let stderr = child.stderr.take().expect("the browser's stderr");
    let (tx, rx) = channel();
    // Read to the end, so the browser never blocks on a full pipe.
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(url) = line.strip_prefix("DevTools listening on ") {
                let _ = tx.send(url.trim().to_string());
            }
        }
    });
    let browser = Browser { child, dir: dir.to_path_buf() };
    let url = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| panic!("{} did not say where its DevTools port is", exe.display()));
    (browser, url)
}

// ----- the DevTools protocol over a WebSocket ---------------------------------------------------

/// One connection to the browser: calls go out, answers and events come back as JSON.
struct DevTools {
    out: TcpStream,
    incoming: Receiver<Value>,
    next: u64,
    session: Option<String>,
    /// Events not yet looked at, oldest first.
    events: VecDeque<Value>,
}

impl DevTools {
    fn connect(url: &str) -> DevTools {
        let rest = url.strip_prefix("ws://").expect("a ws:// address");
        let (host, path) = rest.split_once('/').expect("a path after the host");
        let mut out = TcpStream::connect(host).expect("the DevTools port");
        // The opening handshake (RFC 6455 §4.1); the key is any 16 bytes in base64.
        write!(out, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: QW5pQ3JhZnRXZWJDaGVjaw==\r\nSec-WebSocket-Version: 13\r\n\r\n").expect("the handshake");
        let mut reader = BufReader::new(out.try_clone().expect("a second handle"));
        let mut status = String::new();
        reader.read_line(&mut status).expect("the handshake's answer");
        assert!(status.contains("101"), "the browser refused the WebSocket: {status}");
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
            line.clear();
        }
        let (tx, incoming) = channel();
        std::thread::spawn(move || {
            while let Some(text) = read_message(&mut reader) {
                if let Ok(v) = serde_json::from_slice::<Value>(&text)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        DevTools { out, incoming, next: 0, session: None, events: VecDeque::new() }
    }

    /// Calls `method` and waits for its answer; events that arrive meanwhile are kept.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = &self.session {
            msg["sessionId"] = json!(s);
        }
        send_text(&mut self.out, msg.to_string().as_bytes());
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let v = match self.incoming.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(v) => v,
                Err(RecvTimeoutError::Timeout) => panic!("{method} got no answer in a minute"),
                Err(RecvTimeoutError::Disconnected) => panic!("the browser closed the connection during {method}"),
            };
            if v["id"].as_u64() == Some(id) {
                assert!(v.get("error").is_none(), "{method} failed: {}", v["error"]);
                return v["result"].clone();
            }
            if v.get("method").is_some() {
                self.events.push_back(v);
            }
        }
    }

    /// Takes in what arrived, for up to `wait`.
    fn pump(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        while let Ok(v) = self.incoming.recv_timeout(until.saturating_duration_since(Instant::now())) {
            if v.get("method").is_some() {
                self.events.push_back(v);
            }
        }
    }

    /// The first kept event named `method`, taken out; waits up to `wait` for one.
    fn event(&mut self, method: &str, wait: Duration) -> Option<Value> {
        let until = Instant::now() + wait;
        loop {
            if let Some(i) = self.events.iter().position(|e| e["method"] == method) {
                return self.events.remove(i);
            }
            if Instant::now() >= until {
                return None;
            }
            self.pump(Duration::from_millis(50));
        }
    }

    /// The value of a JavaScript expression in the page.
    fn eval(&mut self, expression: &str) -> Value {
        let r = self.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true}));
        assert!(r.get("exceptionDetails").is_none(), "{expression} threw: {}", r["exceptionDetails"]);
        r["result"]["value"].clone()
    }

    /// Presses and lets go of a key. `text` is what it types, for keys that type.
    fn key(&mut self, key: &str, code: &str, vk: u32, ctrl: bool, text: Option<&str>) {
        let mut down = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": if ctrl { 2 } else { 0 }});
        if let Some(t) = text {
            down["text"] = json!(t);
        }
        let mut up = down.clone();
        up["type"] = json!("keyUp");
        self.call("Input.dispatchKeyEvent", down);
        self.call("Input.dispatchKeyEvent", up);
        // A frame or two for the editor to act on it.
        self.pump(Duration::from_millis(400));
    }

    fn ctrl(&mut self, letter: char) {
        let upper = letter.to_ascii_uppercase();
        self.key(&letter.to_string(), &format!("Key{upper}"), upper as u32, true, None);
    }

    fn enter(&mut self) {
        self.key("Enter", "Enter", 13, false, Some("\r"));
    }

    /// Answers the file picker the page just opened with `file`.
    fn choose(&mut self, file: &Path) {
        let opened = self.event("Page.fileChooserOpened", Duration::from_secs(10)).expect("the browser's file picker did not open");
        self.call("DOM.setFileInputFiles", json!({"files": [file.display().to_string()], "backendNodeId": opened["params"]["backendNodeId"]}));
        self.pump(Duration::from_millis(1500));
    }
}

/// Sends one masked text frame (RFC 6455 §5.2; a client must mask, the key may be anything).
fn send_text(out: &mut TcpStream, payload: &[u8]) {
    let mut frame = vec![0x81];
    match payload.len() {
        n if n < 126 => frame.push(0x80 | n as u8),
        n if n < 65_536 => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mask = [0x41, 0x6e, 0x69, 0x43];
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    out.write_all(&frame).expect("the browser took the message");
}

/// Reads one whole message (its frames joined); `None` when the connection closes.
fn read_message(reader: &mut impl Read) -> Option<Vec<u8>> {
    let mut message = Vec::new();
    loop {
        let mut head = [0u8; 2];
        reader.read_exact(&mut head).ok()?;
        let len = match head[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                reader.read_exact(&mut b).ok()?;
                u64::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                reader.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        let mut payload = vec![0u8; usize::try_from(len).ok()?];
        reader.read_exact(&mut payload).ok()?;
        match head[0] & 0x0f {
            // A close frame.
            8 => return None,
            // Pings and pongs carry nothing for us.
            9 | 10 => continue,
            _ => message.extend_from_slice(&payload),
        }
        if head[0] & 0x80 != 0 {
            return Some(message);
        }
    }
}

// ----- the run ----------------------------------------------------------------------------------

/// The downloads the page started since the last call, each finished: its name and bytes.
/// Counted from the browser's own download events, so a file is read only once it is whole and
/// nothing else in the folder is mistaken for one. Waits for the first to start.
fn downloaded(dir: &Path, t: &mut DevTools) -> Vec<(String, Vec<u8>)> {
    let mut begun: Vec<(String, String)> = Vec::new();
    let mut done: Vec<String> = Vec::new();
    let until = Instant::now() + Duration::from_secs(120);
    // A run of downloads takes a moment to all start (or to be dropped by the browser).
    let mut quiet = Instant::now() + Duration::from_secs(90);
    while Instant::now() < until && (Instant::now() < quiet || begun.iter().any(|(guid, _)| !done.contains(guid))) {
        while let Some(e) = t.event("Browser.downloadWillBegin", Duration::ZERO) {
            let p = &e["params"];
            begun.push((p["guid"].as_str().unwrap_or_default().to_string(), p["suggestedFilename"].as_str().unwrap_or_default().to_string()));
            quiet = Instant::now() + Duration::from_millis(1500);
        }
        while let Some(e) = t.event("Browser.downloadProgress", Duration::ZERO) {
            let p = &e["params"];
            assert_ne!(p["state"], "canceled", "the browser cancelled a download");
            if p["state"] == "completed" {
                done.push(p["guid"].as_str().unwrap_or_default().to_string());
            }
        }
        t.pump(Duration::from_millis(100));
    }
    assert!(begun.iter().all(|(guid, _)| done.contains(guid)), "a download did not finish: {begun:?}");
    begun.into_iter().map(|(_, name)| (name.clone(), std::fs::read(dir.join(&name)).unwrap_or_else(|e| panic!("the download {name}: {e}")))).collect()
}

/// Waits until the editor runs on the page (its loading text is gone), and gives it the keyboard.
fn started(t: &mut DevTools) {
    let until = Instant::now() + Duration::from_secs(180);
    loop {
        let text = t.eval("document.getElementById('anicraft_loading')?.textContent ?? null");
        match text.as_str() {
            None => break,
            Some(s) if s.contains("could not start") => panic!("{s}"),
            Some(_) => assert!(Instant::now() < until, "the editor did not start in three minutes"),
        }
        t.pump(Duration::from_millis(250));
    }
    t.pump(Duration::from_millis(1000));
    t.eval("document.getElementById('anicraft_canvas').focus(); true");
    // Leave the start screen for the document (the sample animation).
    t.key("Escape", "Escape", 27, false, None);
}

fn names(files: &[(String, Vec<u8>)]) -> Vec<&str> {
    files.iter().map(|(n, _)| n.as_str()).collect()
}

/// How many files a ZIP archive lists (its end record, PKWARE APPNOTE 4.3.16).
fn zip_entries(zip: &[u8]) -> usize {
    zip.windows(4).rposition(|w| w == b"PK\x05\x06").and_then(|at| zip.get(at + 10..at + 12)).map_or(0, |n| usize::from(u16::from_le_bytes([n[0], n[1]])))
}

#[test]
#[ignore = "needs the built web/ folder and Chrome, Chromium or Edge: run scripts/web-check.sh"]
fn an_artist_can_work_in_a_real_browser() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let web = std::env::var_os("ANICRAFT_WEB_DIR").map_or_else(|| repo.join("web"), PathBuf::from);
    for f in ["index.html", "anicraft.js", "anicraft_bg.wasm"] {
        assert!(web.join(f).is_file(), "{} is missing: build the web editor first (scripts/build-web.sh)", web.join(f).display());
    }
    let dir = std::env::temp_dir().join(format!("anicraft-web-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let got = dir.join("downloads");
    std::fs::create_dir_all(&got).expect("a folder for the downloads");
    let site = serve(web);
    let (_browser, ws) = launch(&dir);
    let mut t = DevTools::connect(&ws);
    let target = t.call("Target.createTarget", json!({"url": "about:blank"}))["targetId"].clone();
    t.call("Browser.setDownloadBehavior", json!({"behavior": "allow", "downloadPath": got.display().to_string(), "eventsEnabled": true}));
    let session = t.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}))["sessionId"].as_str().expect("a session").to_string();
    t.session = Some(session);
    for domain in ["Page.enable", "Runtime.enable", "DOM.enable"] {
        t.call(domain, json!({}));
    }
    t.call("Page.setInterceptFileChooserDialog", json!({"enabled": true}));
    t.call("Emulation.setDeviceMetricsOverride", json!({"width": 1400, "height": 900, "deviceScaleFactor": 1, "mobile": false}));
    t.call("Page.navigate", json!({"url": site}));
    started(&mut t);

    // Save: the name is asked once, then the document arrives in the downloads (#55).
    t.ctrl('s');
    t.enter();
    let first = downloaded(&got, &mut t);
    assert_eq!(first.len(), 1, "Save downloads the document: {:?}", names(&first));
    let (name, saved) = &first[0];
    let sample: Value = serde_json::from_slice(saved).expect("the saved document is the .anicraft JSON");

    // Preferences are kept in the site's storage (#56): the saved file is now a recent one.
    let prefs = t.eval("localStorage.getItem('anicraft.prefs')");
    assert!(prefs.as_str().is_some_and(|p| p.contains(name.as_str())), "the preferences in the site's storage list the saved file as recent: {prefs}");

    // Open (#55): another document that happens to have the open one's name, as a backup in
    // another folder does. It opens; saving it downloads it, not the first one again.
    let other = dir.join("other").join(name);
    std::fs::create_dir_all(other.parent().expect("its folder")).expect("a folder");
    std::fs::copy(repo.join("crates/engine/tests/fixtures/armature-v3-shape.anicraft"), &other).expect("the fixture document");
    t.ctrl('o');
    t.choose(&other);
    t.ctrl('s');
    let second = downloaded(&got, &mut t);
    assert_eq!(second.len(), 1, "saving the opened document downloads it: {:?}", names(&second));
    let reopened: Value = serde_json::from_slice(&second[0].1).expect("a document");
    assert_ne!(reopened, sample, "the picked file opened, though a document with its name was open");

    // Import to Stage (Ctrl+R, which a browser would reload the page on): the picture is in
    // the document that is saved next.
    t.eval("window.anicraftCheck = true");
    let picture = repo.join("assets/app-icon/hicolor/128x128/apps/ai.storyteller.anicraft.png");
    t.ctrl('r');
    t.choose(&picture);
    t.ctrl('s');
    let third = downloaded(&got, &mut t);
    assert_eq!(third.len(), 1, "saving after the import: {:?}", names(&third));
    assert!(third[0].1.len() > second[0].1.len(), "the imported picture is in the saved document");

    // F5 inserts a frame, as in the desktop app; it must not reload the page (the document is
    // saved, so nothing would even ask).
    t.key("F5", "F5", 116, false, None);
    t.pump(Duration::from_millis(1500));
    assert_eq!(t.eval("window.anicraftCheck === true"), json!(true), "F5 and Ctrl+R are the editor's keys: the page was not reloaded");

    // Export a PNG sequence from the command palette: every frame arrives, as one archive (a
    // browser drops most of a run of downloads).
    t.ctrl('k');
    t.call("Input.insertText", json!({"text": "Export PNG Sequence"}));
    t.pump(Duration::from_millis(500));
    t.enter();
    t.enter();
    let frames = downloaded(&got, &mut t);
    assert_eq!(frames.len(), 1, "a sequence is one download: {:?}", names(&frames));
    assert!(frames[0].0.ends_with(".zip"), "an archive: {:?}", names(&frames));
    let entries = zip_entries(&frames[0].1);
    assert!(entries > 1, "the archive holds the frames: {entries}");

    // Preferences are read back when the page loads (#56): with autosave set to every edit,
    // one edit after a reload leaves a recovery copy in the site's storage at once.
    t.eval("(() => { const p = JSON.parse(localStorage.getItem('anicraft.prefs')); p.autosave_edits = 1; localStorage.setItem('anicraft.prefs', JSON.stringify(p)); return true; })()");
    t.call("Page.reload", json!({}));
    // Unsaved work (the inserted frame) asks before leaving: the browser's own question.
    assert!(t.event("Page.javascriptDialogOpening", Duration::from_secs(10)).is_some(), "leaving with unsaved work asks first");
    t.call("Page.handleJavaScriptDialog", json!({"accept": true}));
    t.pump(Duration::from_millis(1000));
    started(&mut t);
    assert_eq!(t.eval("window.anicraftCheck ?? null"), Value::Null, "the page was reloaded");
    let recovery = "Object.keys(localStorage).filter((k) => k.startsWith('anicraft.recovery.')).length";
    assert_eq!(t.eval(recovery), json!(0), "nothing to recover before an edit");
    t.key("F5", "F5", 116, false, None);
    let until = Instant::now() + Duration::from_secs(20);
    while t.eval(recovery) == json!(0) && Instant::now() < until {
        // The pointer moving keeps frames coming, as an artist at work does.
        t.call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": 700, "y": 300}));
        t.pump(Duration::from_millis(300));
    }
    assert_ne!(t.eval(recovery), json!(0), "the reloaded editor read autosave-every-edit from its preferences and kept a recovery copy");
    assert_eq!(t.eval("JSON.parse(localStorage.getItem('anicraft.prefs')).autosave_edits"), json!(1), "and the preference is still what was set");

    // Nothing went wrong on the way that the page reported.
    t.pump(Duration::from_millis(300));
    let errors: Vec<String> = t.events.iter().filter(|e| e["method"] == "Runtime.exceptionThrown" || (e["method"] == "Runtime.consoleAPICalled" && e["params"]["type"] == "error")).map(Value::to_string).collect();
    assert!(errors.is_empty(), "the page reported errors: {errors:?}");
}
