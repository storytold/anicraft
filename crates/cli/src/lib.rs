//! AniCraft headless front end: argument parsing, the JSON-lines `serve` loop and the MCP
//! server (`mcp`). Every operation goes through the engine's command registry.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bench_stroke;
#[cfg(not(target_arch = "wasm32"))]
pub mod control;
pub mod mcp;

use anicraft_engine::access::{self, Roots};
use anicraft_engine::{Session, commands, format};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub const USAGE: &str = "\
AniCraft CLI — headless animation engine

USAGE:
  anicraft-cli info <file|--sample>
  anicraft-cli fla-dump <legacy.fla> [--stream <name>]   the legacy Flash 8–CS4 container's
                                                         streams and CPic* record tags (JSON)
  anicraft-cli render <file|--sample> --out <frame.png> [--frame N] [--scale S]
  anicraft-cli render-seq <file|--sample> --out-dir <dir> [--from A] [--to B] [--scale S]
  anicraft-cli gif <file|--sample> --out <anim.gif> [--from A] [--to B] [--scale S] [--overwrite yes]
  anicraft-cli apng|webp <file|--sample> --out <anim.png|anim.webp> [--from A] [--to B] [--scale S]
                         [--loop forever|N] [--background yes] [--overwrite yes]
                                         full colour with real transparency (WebP: lossless)
  anicraft-cli export <file|--sample> --format <id> --out <path> [--from A] [--to B] [--frame N]
                      [--options '<json>'] [--overwrite yes]
                                         any export format: the built-in ones (gif, apng, webp,
                                         lottie, html, video, svg, png, png-sequence, svg-sequence,
                                         sprite-sheet)
                                         and exporter plug-ins (by id, with --plugins); --options
                                         are the plug-in's options, or a built-in's extra params
  anicraft-cli formats                   lists every export format, plug-ins included (JSON)
  anicraft-cli commands
  anicraft-cli run <file|--sample> <script.json> [--save <out.anicraft>]
                                         a JSON array of {cmd, params}, or a saved command
                                         (one undo step; nothing changes if a step fails)
  anicraft-cli exec <file|--sample> <command.id> [<json-params>] [--save <out.anicraft>]
  anicraft-cli new --out <file.anicraft> [--width W] [--height H] [--fps F]
  anicraft-cli guide --out <folder>      writes the user guide as HTML pages (open index.html)
  anicraft-cli guide search <words> [--limit N] [--json yes]
                                         searches the user guide: chapter, heading, page and a
                                         passage for each result, best first (as its search page)
  anicraft-cli bench [--layers N] [--shapes N] [--frames N]   performance probe (use --release)
  anicraft-cli bench --check yes         the default scenario against the budgets in
                                         docs/performance.md; fails when one is missed
  anicraft-cli bench --stroke <samples>  one long Brush stroke: painting it, picking it, moving
                                         it and the merge when it is deselected (ADR 0129)
  anicraft-cli bench --stroke-file <file.anicraft>
                                         the same steps on a document's own artwork
  anicraft-cli serve [file|--sample] [--automation-read-root <folder> --automation-write-root <folder>]
                                         JSON-lines on stdin/stdout:
        {\"id\":1,\"cmd\":\"draw.rect\",\"params\":{\"x\":10,\"y\":10,\"w\":50,\"h\":50}}
                                         with the two roots, commands read and write files
                                         inside them only, as for mcp
  anicraft-cli mcp [file|--sample] --automation-read-root <folder> --automation-write-root <folder>
                                         Model Context Protocol server on stdin/stdout,
                                         for AI assistants (see docs/mcp.md). Both roots
                                         are required (exit code 2 without them): commands
                                         read files only inside the read root and write
                                         only inside the write root; links and `..` are
                                         followed first, and a refusal names the root.
                                         --automation-read-root may be given several times
  anicraft-cli mcp --bridge <port> --automation-read-root <folder> --automation-write-root <folder>
                                         the same tools, acting live on the AniCraft window
                                         started with `anicraft --control <port>`
                                         (docs/control-protocol.md); the window applies
                                         the roots to this assistant's commands
  anicraft-cli --version

  --plugins <folder>   loads the WebAssembly plug-ins in a folder first (also the
                       ANICRAFT_PLUGINS environment variable); see docs/plugins.md.
                       Without it, headless runs use no plug-ins, so they are repeatable.

Frame flags on the command line (--frame, --from, --to) are 1-based, as in the
timeline. Frame numbers inside JSON params, scripts and command results are
0-based, as everywhere in the engine.";

/// Parsed flags: positional args plus `--key value` pairs.
pub struct Args {
    pub positional: Vec<String>,
    pub flags: Vec<(String, String)>,
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let mut positional = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--sample" {
                positional.push(a.clone());
            } else if let Some((k, v)) = a.strip_prefix("--").and_then(|f| f.split_once('=')) {
                // `--flag=value`, as MCP client configurations often write the roots.
                flags.push((k.to_string(), v.to_string()));
            } else if let Some(k) = a.strip_prefix("--") {
                let v = it.next().ok_or_else(|| format!("flag --{k} needs a value"))?;
                flags.push((k.to_string(), v.clone()));
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Args { positional, flags })
    }
    /// Every value of a flag that may be given several times, in order.
    pub fn all(&self, k: &str) -> Vec<&str> {
        self.flags.iter().filter(|(key, _)| key == k).map(|(_, v)| v.as_str()).collect()
    }
    pub fn flag(&self, k: &str) -> Option<&str> {
        self.flags.iter().rev().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }
    pub fn num<T: std::str::FromStr>(&self, k: &str) -> Result<Option<T>, String> {
        self.flag(k).map(|v| v.parse::<T>().map_err(|_| format!("--{k} expects a number, got '{v}'"))).transpose()
    }
    fn float(&self, k: &str) -> Result<Option<f32>, String> {
        let value = self.num::<f32>(k)?;
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(format!("--{k} expects a finite number"));
        }
        Ok(value)
    }
}

/// How the error starts when `mcp` is started without its roots; the program then exits with
/// [`USAGE_EXIT`] (AGENTS.md rule 18, ADR 0124).
pub const ROOTS_REQUIRED: &str = "mcp needs --automation-read-root <folder> and --automation-write-root <folder>";

/// How the error starts when the Nightly window's launch protocol is given to this tool.
pub const NIGHTLY_PROTOCOL: &str = "--launch-profile and --profile-root belong to the AniCraft Nightly window";

/// The exit code of a refused start: the command line is missing something.
pub const USAGE_EXIT: i32 = 2;

/// The exit code for an error from [`run`].
pub fn exit_code(error: &str) -> i32 {
    let unknown_option = error.strip_prefix(UNKNOWN_OPTION).is_some_and(|rest| rest.split_whitespace().nth(1) == Some("option"));
    if unknown_option
        || error.starts_with(ROOTS_REQUIRED)
        || error.starts_with(NIGHTLY_PROTOCOL)
        || error.starts_with("--automation-")
        || error.starts_with("flag --automation-")
    {
        USAGE_EXIT
    } else {
        1
    }
}

/// The Nightly launcher's two arguments, if the command line carries either (also as `=` forms):
/// this headless tool has no launch profile and no window to put in one, so it refuses them before
/// any subcommand is read.
pub fn nightly_protocol_argument(argv: &[String]) -> Option<&str> {
    argv.iter()
        .find(|a| *a == "--launch-profile" || *a == "--profile-root" || a.starts_with("--launch-profile=") || a.starts_with("--profile-root="))
        .map(String::as_str)
}

/// The folders given with `--automation-read-root` (once or more) and `--automation-write-root`
/// (once), from the launch folder when relative. `None` when neither flag is there; one
/// without the other is an error, as is a root that is not a folder.
pub fn automation_folders(a: &Args) -> Result<Option<(Vec<PathBuf>, PathBuf)>, String> {
    let (read, write) = (a.all("automation-read-root"), a.all("automation-write-root"));
    if read.is_empty() && write.is_empty() {
        return Ok(None);
    }
    let folder = |flag: &str, v: &str| -> Result<PathBuf, String> {
        if v.trim().is_empty() {
            return Err(format!("--automation-{flag}-root needs a folder"));
        }
        std::path::absolute(v).map_err(|e| format!("--automation-{flag}-root {v}: {e}"))
    };
    let write = match write.as_slice() {
        [] => return Err("--automation-read-root needs --automation-write-root <folder> too (it may be the same folder)".into()),
        [one] => folder("write", one)?,
        _ => return Err("--automation-write-root may be given once (--automation-read-root may be repeated)".into()),
    };
    if read.is_empty() {
        return Err("--automation-write-root needs --automation-read-root <folder> too (it may be the same folder)".into());
    }
    let read = read.iter().map(|v| folder("read", v)).collect::<Result<Vec<_>, _>>()?;
    Ok(Some((read, write)))
}

/// [`automation_folders`] as a policy: each root resolved (links followed) and checked to be
/// a folder that exists.
pub fn automation_roots(a: &Args) -> Result<Option<Arc<Roots>>, String> {
    let Some((read, write)) = automation_folders(a)? else { return Ok(None) };
    Roots::new(&read, Some(&write)).map(|r| Some(Arc::new(r)))
}

/// Runs one command for an agent: inside `roots` when given (ADR 0124). When the command
/// fails after a file was refused, the error is the refusal, which names the roots (a command
/// that asked "is it there?" would otherwise say the file is missing).
pub(crate) fn execute_in(roots: Option<&Arc<Roots>>, run: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    let Some(roots) = roots else { return run() };
    let _confined = access::confine(roots.clone());
    run().map_err(|e| match access::take_denial() {
        Some(why) if !e.contains(&why) => why,
        _ => e,
    })
}

/// Opens a session from a path or `--sample`.
pub fn open_session(src: Option<&str>) -> Result<Session, String> {
    match src {
        None => Ok(Session::default()),
        Some("--sample") => Ok(Session::new(anicraft_doc::sample::demo())),
        // An Animate document imports as File › Open does (#226), untitled.
        Some(p) if anicraft_engine::is_animate(std::path::Path::new(p)) => {
            let mut s = Session::default();
            s.execute("file.open", &serde_json::json!({"path": p})).map_err(|e| e.to_string())?;
            Ok(s)
        }
        Some(p) => {
            let doc = format::load(std::path::Path::new(p)).map_err(|e| format!("{p}: {e}"))?;
            let mut s = Session::new(doc);
            s.path = Some(PathBuf::from(p));
            Ok(s)
        }
    }
}

/// [`open_session`], with the plug-ins of `--plugins <folder>` (or `ANICRAFT_PLUGINS`) loaded.
pub fn session(a: &Args, src: Option<&str>) -> Result<Session, String> {
    let mut s = open_session(src)?;
    let folder = a.flag("plugins").map(str::to_string).or_else(|| std::env::var("ANICRAFT_PLUGINS").ok().filter(|v| !v.trim().is_empty()));
    if let Some(folder) = folder {
        let r = exec(&mut s, "plugin.reload", &json!({"folder": folder}))?;
        for f in r["failed"].as_array().into_iter().flatten() {
            eprintln!("plug-in {} not loaded: {}", f["file"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or(""));
        }
    }
    Ok(s)
}

fn exec(s: &mut Session, id: &str, params: &Value) -> Result<Value, String> {
    s.execute(id, params).map_err(|e| format!("{id}: {e}"))
}

fn one_based(a: &Args, k: &str) -> Result<Option<u32>, String> {
    match a.num::<u32>(k)? {
        Some(0) => Err(format!("--{k} is 1-based; the first frame is 1")),
        Some(n) => Ok(Some(n - 1)),
        None => Ok(None),
    }
}

/// The usage of one subcommand, under `USAGE:`: its entries in [`USAGE`] with their
/// continuation lines, so the two never drift apart. `None` for a name [`USAGE`] doesn't list.
pub fn subcommand_usage(cmd: &str) -> Option<String> {
    const ENTRY: &str = "  anicraft-cli ";
    let mut out = String::new();
    let mut taking = false;
    // The entries are the block after `USAGE:`, up to the first blank line.
    for line in USAGE.lines().skip_while(|l| *l != "USAGE:").skip(1).take_while(|l| !l.trim().is_empty()) {
        if let Some(rest) = line.strip_prefix(ENTRY) {
            // `apng|webp` names two subcommands; `--version` is also `version`.
            let names = rest.split_whitespace().next().unwrap_or("");
            taking = names.split('|').any(|n| n == cmd || n.strip_prefix("--") == Some(cmd));
        }
        if taking {
            out.push('\n');
            out.push_str(line);
        }
    }
    (!out.is_empty()).then(|| format!("USAGE:{out}"))
}

/// The options a subcommand reads, beyond `--plugins`, which every one takes. `None` for a name
/// that is no subcommand.
fn options_of(cmd: &str) -> Option<&'static [&'static str]> {
    Some(match cmd {
        "render" => &["out", "frame", "scale"],
        "render-seq" => &["out-dir", "from", "to", "scale"],
        "gif" => &["out", "from", "to", "scale", "overwrite"],
        "apng" | "webp" => &["out", "from", "to", "scale", "overwrite", "loop", "background"],
        "export" => &["format", "out", "from", "to", "frame", "options", "overwrite"],
        "run" | "exec" => &["save"],
        "new" => &["out", "width", "height", "fps"],
        "guide" => &["out", "limit", "json"],
        "bench" => &["layers", "shapes", "frames", "check", "stroke", "stroke-file"],
        "serve" => &["automation-read-root", "automation-write-root"],
        "mcp" => &["automation-read-root", "automation-write-root", "bridge"],
        "info" | "formats" | "commands" | "version" | "--version" | "-V" => &[],
        _ => return None,
    })
}

/// How the error starts when a subcommand is given an option it doesn't read.
const UNKNOWN_OPTION: &str = "unknown ";

/// Refuses an option the subcommand doesn't read, before anything is opened or written: a
/// mistyped `--frmae 12` was dropped, and frame 1 written as if it had been asked for.
fn check_options(cmd: &str, a: &Args) -> Result<(), String> {
    let Some(known) = options_of(cmd) else { return Ok(()) };
    match a.flags.iter().find(|(key, _)| key != "plugins" && !known.contains(&key.as_str())) {
        Some((key, _)) => Err(format!("{UNKNOWN_OPTION}{cmd} option --{key}

{}", subcommand_usage(cmd).unwrap_or_default())),
        None => Ok(()),
    }
}

/// Runs the CLI; returns the text to print on success.
pub fn run(argv: &[String]) -> Result<String, String> {
    if let Some(argument) = nightly_protocol_argument(argv) {
        return Err(format!(
            "{NIGHTLY_PROTOCOL} (`{argument}`); anicraft-cli is the stable headless front end. Run `anicraft --launch-profile nightly-v1 --profile-root <folder> --` instead."
        ));
    }
    let Some((cmd, rest)) = argv.split_first() else { return Ok(USAGE.to_string()) };
    if cmd == "help" || cmd == "--help" || cmd == "-h" {
        if let Some(sub) = rest.first().filter(|s| *s != "--help" && *s != "-h") {
            return subcommand_usage(sub).ok_or_else(|| format!("unknown subcommand '{sub}'\n\n{USAGE}"));
        }
        return Ok(USAGE.to_string());
    }
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        return subcommand_usage(cmd).ok_or_else(|| format!("unknown subcommand '{cmd}'\n\n{USAGE}"));
    }
    let a = Args::parse(rest)?;
    check_options(cmd, &a)?;
    let src = a.positional.first().map(String::as_str);
    match cmd.as_str() {
        // The release packaging smoke-tests every build with this (packaging/, ADR 0026).
        "version" | "--version" | "-V" => Ok(format!("anicraft-cli {}", env!("CARGO_PKG_VERSION"))),
        "commands" => {
            let list: Vec<Value> = commands().iter().map(|c| c.to_json()).collect();
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())
        }
        // The user guide as the static HTML book Help › User Guide opens (ADR 0069).
        // Searching it, with the ranking the book's search page uses (ADR 0096).
        "guide" if src == Some("search") => {
            let words = a.positional.get(1..).unwrap_or_default().join(" ");
            if anicraft_guide::search::terms(&words).is_empty() {
                return Err("guide search needs words to look for, e.g. anicraft-cli guide search onion skin".into());
            }
            let limit = a.num::<usize>("limit")?.unwrap_or(10).clamp(1, 100);
            let hits = anicraft_guide::search::search(anicraft_guide::PAGES, &anicraft_guide::search::sections(anicraft_guide::PAGES), &words, limit);
            if a.flag("json").is_some_and(|v| v == "yes") {
                let list: Vec<Value> = hits.iter().map(|h| json!({"chapter": h.chapter, "heading": h.heading, "url": h.url, "snippet": h.snippet, "score": h.score})).collect();
                return serde_json::to_string_pretty(&list).map_err(|e| e.to_string());
            }
            if hits.is_empty() {
                return Ok(format!("nothing in the user guide matches “{}”", words.trim()));
            }
            let lines: Vec<String> = hits.iter().enumerate().map(|(i, h)| format!("{}. {} · {}\n   {}\n   {}", i + 1, h.heading, h.chapter, h.url, h.snippet)).collect();
            Ok(lines.join("\n\n"))
        }
        "guide" => {
            let out = a.flag("out").ok_or("guide needs --out <folder>, or search <words>")?;
            let n = anicraft_guide::write_site(std::path::Path::new(out)).map_err(|e| format!("{out}: {e}"))?;
            Ok(format!("wrote {n} files of the user guide to {out}; open {out}/index.html"))
        }
        "info" => {
            let s = session(&a, Some(src.ok_or("info needs a file or --sample")?))?;
            serde_json::to_string_pretty(&s.info()).map_err(|e| e.to_string())
        }
        // The legacy Flash container's stream tree and record tags (ADR 0120): the
        // reverse-engineering microscope for Flash 8–CS4 era `.fla` files.
        "fla-dump" => {
            let p = src.ok_or("fla-dump needs a legacy .fla file")?;
            // A file over the container's cap is refused by its size, and the read stops one
            // byte past the cap, so one that grows in between is refused before parsing too.
            let (path, max) = (std::path::Path::new(p), anicraft_fla::cfb::MAX_FILE);
            let too_large = || format!("{p}: {}", anicraft_fla::cfb::Error::TooLarge);
            if anicraft_engine::vfs::len(path).map_err(|e| format!("{p}: {e}"))? > max {
                return Err(too_large());
            }
            let data = anicraft_engine::vfs::read_bounded(path, max).map_err(|e| format!("{p}: {e}"))?;
            if data.len() as u64 > max {
                return Err(too_large());
            }
            if !anicraft_fla::is_legacy(&data) {
                return Err(format!("{p}: not a legacy Flash document (its first bytes are not the compound binary signature)"));
            }
            // With --stream, only that stream is read: another one's damage can't fail it.
            let streams = anicraft_fla::dump_only(&data, a.flag("stream")).map_err(|e| format!("{p}: {e}"))?;
            let list: Vec<Value> = streams.iter()
                .map(|s| {
                    let mut v = json!({
                        "stream": s.path,
                        "size": s.size,
                        "head": s.head,
                        "tags": s.tags.iter().map(|(o, n)| json!({"at": o, "name": n})).collect::<Vec<_>>(),
                    });
                    // A scan that stopped at its budget says so: its tags are the first ones only.
                    if !s.tags_complete {
                        v["tags_truncated"] = json!(true);
                    }
                    v
                })
                .collect();
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())
        }
        "render" => {
            let mut s = session(&a, Some(src.ok_or("render needs a file or --sample")?))?;
            let out = a.flag("out").ok_or("render needs --out <file.png>")?;
            let mut p = json!({"path": out, "frame": one_based(&a, "frame")?.unwrap_or(0)});
            if let Some(sc) = a.float("scale")? {
                p["scale"] = json!(sc);
            }
            exec(&mut s, "file.export_png", &p).map(|v| v.to_string())
        }
        "render-seq" => {
            let mut s = session(&a, Some(src.ok_or("render-seq needs a file or --sample")?))?;
            let dir = a.flag("out-dir").ok_or("render-seq needs --out-dir <dir>")?;
            let mut p = json!({"dir": dir});
            if let Some(f) = one_based(&a, "from")? {
                p["from"] = json!(f);
            }
            if let Some(t) = one_based(&a, "to")? {
                p["to"] = json!(t);
            }
            if let Some(sc) = a.float("scale")? {
                p["scale"] = json!(sc);
            }
            exec(&mut s, "file.export_png_sequence", &p).map(|v| v.to_string())
        }
        "gif" | "apng" | "webp" => {
            let mut s = session(&a, Some(src.ok_or(format!("{cmd} needs a file or --sample"))?))?;
            let cmd = cmd.as_str();
            let ext = if cmd == "apng" { "png" } else { cmd };
            let out = a.flag("out").ok_or(format!("{cmd} needs --out <anim.{ext}>"))?;
            let mut p = json!({"path": out});
            if let Some(f) = one_based(&a, "from")? {
                p["from"] = json!(f);
            }
            if let Some(t) = one_based(&a, "to")? {
                p["to"] = json!(t);
            }
            if let Some(sc) = a.float("scale")? {
                p["scale"] = json!(sc);
            }
            if a.flag("overwrite").is_some_and(|v| v == "yes" || v == "true") {
                p["confirm"] = json!(true);
            }
            if cmd != "gif" {
                match a.flag("loop") {
                    None | Some("forever") => {}
                    Some(n) => p["loop"] = json!(n.parse::<u32>().map_err(|_| format!("--loop is forever or how many times it plays, not '{n}'"))?),
                }
                if a.flag("background").is_some_and(|v| v == "yes" || v == "true") {
                    p["background"] = json!(true);
                }
            }
            exec(&mut s, &format!("file.export_{cmd}"), &p).map(|v| v.to_string())
        }
        "formats" => {
            let mut s = session(&a, None)?;
            let v = exec(&mut s, "file.export_formats", &json!({}))?;
            serde_json::to_string_pretty(&v["formats"]).map_err(|e| e.to_string())
        }
        "export" => export(&a, src),
        "new" => {
            let mut s = Session::default();
            let out = a.flag("out").ok_or("new needs --out <file.anicraft>")?;
            let mut p = json!({});
            for k in ["width", "height"] {
                if let Some(v) = a.num::<u32>(k)? {
                    p[k] = json!(v);
                }
            }
            if let Some(v) = a.float("fps")? {
                p["fps"] = json!(v);
            }
            exec(&mut s, "file.new", &p)?;
            exec(&mut s, "file.save", &json!({"path": out})).map(|v| v.to_string())
        }
        "exec" => {
            let mut s = session(&a, Some(src.ok_or("exec needs a file or --sample")?))?;
            let id = a.positional.get(1).ok_or("exec needs a command id")?;
            let params: Value = match a.positional.get(2) {
                Some(t) => serde_json::from_str(t).map_err(|e| format!("params are not JSON: {e}"))?,
                None => json!({}),
            };
            let v = exec(&mut s, id, &params)?;
            if let Some(out) = a.flag("save") {
                exec(&mut s, "file.save", &json!({"path": out}))?;
            }
            Ok(v.to_string())
        }
        "run" => {
            let mut s = session(&a, Some(src.ok_or("run needs a file or --sample")?))?;
            let script = a.positional.get(1).ok_or("run needs a script.json")?;
            let text = std::fs::read_to_string(script).map_err(|e| format!("{script}: {e}"))?;
            // Windows editors often save UTF-8 with a byte-order mark; JSON parsers reject it.
            let parsed: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{script}: expected a JSON array of {{cmd, params}} or a saved command: {e}"))?;
            // A saved command (ADR 0097) runs as one transactional step, as in the app.
            if parsed.is_object() {
                let v = exec(&mut s, "macro.run", &json!({"path": script}))?;
                if let Some(out) = a.flag("save") {
                    exec(&mut s, "file.save", &json!({"path": out}))?;
                }
                return serde_json::to_string_pretty(&v).map_err(|e| e.to_string());
            }
            let steps: Vec<Value> = serde_json::from_value(parsed).map_err(|e| format!("{script}: expected a JSON array of {{cmd, params}}: {e}"))?;
            let mut results = Vec::new();
            for (i, step) in steps.iter().enumerate() {
                let id = step.get("cmd").and_then(Value::as_str).ok_or_else(|| format!("step {}: missing \"cmd\"", i + 1))?;
                let params = step.get("params").cloned().unwrap_or_else(|| json!({}));
                results.push(exec(&mut s, id, &params).map_err(|e| format!("step {}: {e}", i + 1))?);
            }
            if let Some(out) = a.flag("save") {
                exec(&mut s, "file.save", &json!({"path": out}))?;
            }
            serde_json::to_string_pretty(&results).map_err(|e| e.to_string())
        }
        "bench" => bench(&a),
        "serve" => {
            // Before anything is opened: a bad root stops the start.
            let roots = automation_roots(&a)?;
            let mut s = session(&a, src)?;
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            serve_in(&mut s, roots.as_ref(), stdin.lock(), &mut stdout).map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "mcp" => {
            // An agent's entry point refuses to start without both roots (AGENTS.md rule 18),
            // before any file is opened or any connection made.
            let Some((read, write)) = automation_folders(&a)? else {
                return Err(format!("{ROOTS_REQUIRED}: the folders the assistant's commands may read from and write to (they may be the same folder). See docs/mcp.md."));
            };
            let roots = Arc::new(Roots::new(&read, Some(&write))?);
            if let Some(target) = a.flag("bridge") {
                if src.is_some() || a.flag("plugins").is_some() {
                    return Err("mcp --bridge acts on the open AniCraft window: it takes no file or --plugins".into());
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = target;
                    return Err("mcp --bridge needs sockets, which this build doesn't have".into());
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let mut b = control::Bridge::new(control::parse_bridge_target(target)?).with_roots(&roots)?;
                    let stdin = std::io::stdin();
                    let mut stdout = std::io::stdout();
                    mcp::serve(&mut b, stdin.lock(), &mut stdout).map_err(|e| e.to_string())?;
                    return Ok(String::new());
                }
            }
            // The file and plug-ins named at the start are the person's own choice; the roots
            // apply to what the assistant asks for from here on.
            let mut s = mcp::Headless { session: session(&a, src)?, roots };
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            mcp::serve(&mut s, stdin.lock(), &mut stdout).map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        other => Err(format!("unknown subcommand '{other}'\n\n{USAGE}")),
    }
}

/// `export --format <id>`: a built-in format through its own command, or an exporter plug-in
/// through `file.export_plugin` (ADR 0088).
fn export(a: &Args, src: Option<&str>) -> Result<String, String> {
    let mut s = session(a, Some(src.ok_or("export needs a file or --sample")?))?;
    let id = a.flag("format").ok_or("export needs --format <id> (anicraft-cli formats lists them)")?;
    let out = a.flag("out").ok_or("export needs --out <path>")?;
    let list = exec(&mut s, "file.export_formats", &json!({}))?;
    let formats = list["formats"].as_array().cloned().unwrap_or_default();
    let Some(f) = formats.iter().find(|f| f["id"] == id) else {
        let ids: Vec<&str> = formats.iter().filter_map(|f| f["id"].as_str()).collect();
        return Err(format!("no export format '{id}' (formats: {}; exporter plug-ins load with --plugins <folder>)", ids.join(", ")));
    };
    let (cmd, target, plugin) = (f["command"].as_str().unwrap_or(""), f["target"].as_str().unwrap_or("path"), f["plugin"] == true);
    let mut p = json!({});
    let options: Option<Value> = a.flag("options").map(|t| serde_json::from_str(t).map_err(|e| format!("--options is not JSON: {e}"))).transpose()?;
    if plugin {
        p["format"] = json!(id);
        if let Some(o) = options {
            p["options"] = o;
        }
    } else if let Some(Value::Object(o)) = options {
        for (k, v) in o {
            p[k] = v;
        }
    } else if options.is_some() {
        return Err("--options must be a JSON object".into());
    }
    p[target] = json!(out);
    if matches!(id, "png" | "svg") {
        if let Some(f) = one_based(a, "frame")? {
            p["frame"] = json!(f);
        }
    } else {
        for k in ["from", "to"] {
            if let Some(f) = one_based(a, k)? {
                p[k] = json!(f);
            }
        }
    }
    if a.flag("overwrite").is_some_and(|v| v == "yes" || v == "true") {
        p["confirm"] = json!(true);
    }
    exec(&mut s, cmd, &p).map(|v| v.to_string())
}

fn avg_ms(n: u32, mut f: impl FnMut(u32)) -> f64 {
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    t.elapsed().as_secs_f64() * 1000.0 / f64::from(n.max(1))
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Timing budgets for the default `bench` scenario (50 layers × 200 shapes × 240 frames,
/// 200k element instances), in release builds: `(key in the bench output, what, limit)`.
/// Keep in step with the table in `docs/performance.md`. Budgets that don't depend on the
/// machine (memory, sharing, file size, culling) are tests instead, run on every PR.
pub const TIME_BUDGETS: &[(&str, &str, f64)] = &[
    ("edit_command_ms", "edit command (ms)", 1.0),
    ("undo_ms", "undo (ms)", 1.0),
    ("snapshot_clone_us", "document snapshot (µs)", 50.0),
    ("evaluate_ms_per_frame", "evaluate a frame (ms)", 4.0),
    ("render_1080p_ms_per_frame", "render 1080p (ms)", 16.7),
    ("render_zoom400_ms_per_frame", "render at 400% (ms)", 16.7),
    ("hit_test_us", "hit test (µs)", 200.0),
    ("file.save_ms", "save the 65 MB file (ms)", 500.0),
    ("file.load_ms", "load the 65 MB file (ms)", 500.0),
    ("autosave.ui_us", "autosave: time on the editing thread (µs)", 1000.0),
    ("autosave.background_ms", "autosave the 65 MB file in the background (ms)", 2000.0),
];

/// One crash-recovery autosave of `doc` as the app does it: an `Autosaver` in a scratch
/// recovery folder, timing what the editing thread pays (the hand-off) apart from the
/// background serialize + atomic write (temp file, flush to disk, rename).
fn bench_autosave(doc: &anicraft_doc::Document) -> Value {
    use anicraft_engine::recovery::{Autosaver, Outcome, SessionLock};
    let dir = std::env::temp_dir().join(format!("anicraft-bench-recovery-{}", std::process::id()));
    let Ok(mut saver) = SessionLock::acquire(&dir).and_then(Autosaver::start) else { return json!({"error": "no scratch folder"}) };
    let t = Instant::now();
    saver.write(doc.clone(), None);
    let ui_us = t.elapsed().as_secs_f64() * 1e6;
    let mut out = json!({"error": "timed out"});
    while t.elapsed().as_secs() < 120 {
        if let Some(o) = saver.poll().into_iter().next() {
            let background_ms = t.elapsed().as_secs_f64() * 1000.0;
            out = match o {
                Outcome::Written { stats, .. } => json!({
                    "ui_us": round(ui_us), "background_ms": round(background_ms),
                    "serialize_ms": round(stats.serialize_ms), "write_ms": round(stats.write_ms),
                    "mb": round(stats.bytes as f64 / 1048576.0),
                }),
                Outcome::Failed { error, .. } => json!({"error": error}),
                Outcome::Cleared { .. } => out,
            };
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    saver.finish(None);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Compares bench output with [`TIME_BUDGETS`]: one row per budget, and the ones missed.
pub fn check_budgets(out: &Value) -> (Vec<Value>, Vec<String>) {
    let mut rows = Vec::new();
    let mut missed = Vec::new();
    for (key, what, limit) in TIME_BUDGETS {
        let value = key.split('.').try_fold(out, |v, k| v.get(k)).and_then(Value::as_f64);
        let ok = value.is_some_and(|v| v <= *limit);
        if !ok {
            missed.push(match value {
                Some(v) => format!("{what}: {v} (budget {limit})"),
                None => format!("{what}: not measured"),
            });
        }
        rows.push(json!({"budget": what, "limit": limit, "measured": value, "ok": ok}));
    }
    (rows, missed)
}

/// Performance probe on a generated stress document. Run in release for meaningful numbers:
/// `cargo run --release -p anicraft-cli -- bench`. With `--check yes`, the default scenario
/// is compared with [`TIME_BUDGETS`] and a missed budget is an error.
pub fn bench(a: &Args) -> Result<String, String> {
    if a.flag("stroke").is_some() || a.flag("stroke-file").is_some() {
        return bench_stroke::bench_stroke(a);
    }
    let check = a.flag("check").is_some_and(|v| v == "yes" || v == "true");
    if check {
        if cfg!(debug_assertions) {
            return Err("bench --check needs a release build: cargo run --release -p anicraft-cli -- bench --check yes".into());
        }
        if ["layers", "shapes", "frames"].iter().any(|k| a.flag(k).is_some()) {
            return Err("the budgets are for the default scenario; leave out --layers, --shapes and --frames with --check".into());
        }
    }
    use anicraft_doc::{evaluate, sample};
    use anicraft_engine::history::document_bytes;
    use anicraft_render::{View, render};

    let layers = a.num::<u32>("layers")?.unwrap_or(50);
    let shapes = a.num::<u32>("shapes")?.unwrap_or(200);
    let frames = a.num::<u32>("frames")?.unwrap_or(240);
    let t = Instant::now();
    let doc = sample::stress(layers, shapes, frames);
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;
    let instances: usize = doc.timeline(0).layers.iter().flat_map(|l| l.keyframes.iter()).map(|k| k.elements.len()).sum();

    let mut json_text = String::new();
    let save_ms = avg_ms(1, |_| json_text = format::to_json(&doc).unwrap_or_default());
    let load_ms = avg_ms(1, |_| {
        let _ = format::from_json(&json_text);
    });

    let autosave = bench_autosave(&doc);

    let eval_ms = avg_ms(48, |i| {
        let _ = evaluate(&doc, (i * 5) % frames.max(1));
    });
    let view = View::stage(&doc, 1.0);
    let mut drawn = 0;
    let render_ms = avg_ms(12, |i| {
        if let Ok((_, st)) = render(&evaluate(&doc, (i * 20) % frames.max(1)), &view) {
            drawn = st.drawn;
        }
    });
    // 400% zoom on the stage centre: most shapes are off-screen and culled.
    let zoomed = View { transform: anicraft_geom::Affine::translate(-2880.0, -1620.0).mul(&anicraft_geom::Affine::scale(4.0, 4.0)), ..view };
    let mut culled = 0;
    let zoom_ms = avg_ms(12, |i| {
        if let Ok((_, st)) = render(&evaluate(&doc, (i * 20) % frames.max(1)), &zoomed) {
            culled = st.culled;
        }
    });
    let clone_us = avg_ms(1000, |_| {
        let _ = doc.clone();
    }) * 1000.0;

    let mut s = Session::new(doc);
    let _ = s.execute("layer.select", &json!({"index": 0}));
    s.set_frame(7);
    let first = s.visible_elements().first().map(|e| e.id.0);
    if let Some(id) = first {
        let _ = s.execute("element.select", &json!({"ids": [id]}));
    }
    let edit_ms = avg_ms(100, |_| {
        let _ = s.execute("element.translate", &json!({"dx": 1, "dy": 0}));
    });
    let steps = s.history().undo_len().max(1);
    let per_step = s.history().bytes() / steps;
    let undo_ms = avg_ms(50, |_| {
        let _ = s.undo();
    });
    let hit_us = avg_ms(500, |i| {
        let _ = s.hit_test(anicraft_geom::Vec2::new((i * 37 % 1920) as f32, (i * 53 % 1080) as f32), 2.0);
    }) * 1000.0;

    let out = json!({
        "document": {"layers": layers, "shapes_per_layer": shapes, "frames": frames, "element_instances": instances,
                     "estimated_mb": round(document_bytes(&s.doc) as f64 / 1048576.0), "build_ms": round(build_ms)},
        "file": {"json_mb": round(json_text.len() as f64 / 1048576.0), "save_ms": round(save_ms), "load_ms": round(load_ms)},
        "autosave": autosave,
        "evaluate_ms_per_frame": round(eval_ms),
        "render_1080p_ms_per_frame": round(render_ms), "render_shapes_drawn": drawn,
        "render_zoom400_ms_per_frame": round(zoom_ms), "render_zoom400_culled": culled,
        "snapshot_clone_us": round(clone_us),
        "edit_command_ms": round(edit_ms), "undo_bytes_per_step": per_step, "undo_ms": round(undo_ms),
        "hit_test_us": round(hit_us),
        "threads": std::thread::available_parallelism().map_or(1, |n| n.get()),
        "build": if cfg!(debug_assertions) { "debug (use --release for real numbers)" } else { "release" },
    });
    if !check {
        return serde_json::to_string_pretty(&out).map_err(|e| e.to_string());
    }
    let (rows, missed) = check_budgets(&out);
    let report = serde_json::to_string_pretty(&json!({"measured": out, "budgets": rows})).map_err(|e| e.to_string())?;
    if missed.is_empty() {
        Ok(report)
    } else {
        Err(format!("{report}\n\nover budget: {}", missed.join("; ")))
    }
}

/// Longest request line `serve` and `mcp` read (64 MiB: far above any real command, base64 images included).
pub(crate) const MAX_LINE: usize = 64 << 20;

/// One input line, read with a bound.
pub(crate) enum Line {
    Eof,
    Text(String),
    TooLong,
}

/// Reads the next line, never holding more than `max` bytes of it; an overlong line is skipped to its end.
pub(crate) fn read_line<R: BufRead>(input: &mut R, max: usize) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let n = std::io::Read::take(&mut *input, max as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.len() > max && buf.last() != Some(&b'\n') {
        loop {
            let mut skip = Vec::new();
            let k = std::io::Read::take(&mut *input, 1 << 20).read_until(b'\n', &mut skip)?;
            if k == 0 || skip.last() == Some(&b'\n') {
                return Ok(Line::TooLong);
            }
        }
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    String::from_utf8(buf).map(Line::Text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Over `serve` and `mcp` a caller may not name the program a command runs (ADR 0004 spirit: an
/// assistant must not start arbitrary executables); the person sets `ANICRAFT_FFMPEG` instead.
/// Direct CLI use (`exec`, `run`) keeps the `ffmpeg` parameter. Every command that takes one
/// means the same by it (Export Video, Import Video, video info, relinking a layer's video), so
/// the parameter is refused by name: a new command that runs ffmpeg is covered when it is added.
pub(crate) fn refuse_remote_ffmpeg(_cmd: &str, params: &Value) -> Result<(), String> {
    if params.get("ffmpeg").is_some_and(|v| !v.is_null()) {
        return Err("the ffmpeg parameter is not accepted here: set the ANICRAFT_FFMPEG environment variable to choose the program".into());
    }
    Ok(())
}

/// Answers one JSON request, with no automation roots.
pub fn handle_request(s: &mut Session, line: &str) -> Value {
    handle_request_in(s, None, line)
}

/// Answers one JSON request; with `roots`, the command reads and writes inside them only.
pub fn handle_request_in(s: &mut Session, roots: Option<&Arc<Roots>>, line: &str) -> Value {
    // Windows shells often prefix piped UTF-8 with a byte-order mark.
    let line = line.trim_start_matches('\u{feff}');
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return json!({"id": null, "ok": false, "error": format!("invalid JSON: {e}")}),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let Some(cmd) = req.get("cmd").and_then(Value::as_str) else {
        return json!({"id": id, "ok": false, "error": "missing \"cmd\""});
    };
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match cmd {
        "commands" => Ok(Value::Array(commands().iter().map(|c| c.to_json()).collect())),
        _ => refuse_remote_ffmpeg(cmd, &params).and_then(|()| execute_in(roots, || s.execute(cmd, &params).map_err(|e| e.to_string()))),
    };
    match result {
        Ok(v) => json!({"id": id, "ok": true, "result": v, "revision": s.revision, "frame": s.frame}),
        Err(e) => json!({"id": id, "ok": false, "error": e}),
    }
}

/// JSON-lines loop: one request per line, one response per line. Ends on EOF or `{"cmd":"quit"}`.
pub fn serve<R: BufRead, W: Write>(s: &mut Session, input: R, out: &mut W) -> std::io::Result<()> {
    serve_in(s, None, input, out)
}

/// [`serve`], inside automation roots when given.
pub fn serve_in<R: BufRead, W: Write>(s: &mut Session, roots: Option<&Arc<Roots>>, input: R, out: &mut W) -> std::io::Result<()> {
    writeln!(out, "{}", json!({"ready": true, "app": "AniCraft", "version": env!("CARGO_PKG_VERSION")}))?;
    out.flush()?;
    let mut input = input;
    loop {
        let line = match read_line(&mut input, MAX_LINE)? {
            Line::Eof => break,
            Line::TooLong => {
                writeln!(out, "{}", json!({"id": null, "ok": false, "error": "request line too long"}))?;
                out.flush()?;
                continue;
            }
            Line::Text(l) => l,
        };
        let line = line.trim_start_matches('\u{feff}');
        if line.trim().is_empty() {
            continue;
        }
        if line.contains("\"quit\"") && serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("cmd").cloned()) == Some(json!("quit")) {
            writeln!(out, "{}", json!({"ok": true, "bye": true}))?;
            break;
        }
        let resp = handle_request_in(s, roots, line);
        writeln!(out, "{resp}")?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `<temp>/…/{work,out,secret}`: `work` is the read root and `out` the write root.
    pub(crate) struct Roots3 {
        pub dir: std::path::PathBuf,
        pub roots: std::sync::Arc<anicraft_engine::access::Roots>,
    }
    impl Drop for Roots3 {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    impl Roots3 {
        pub fn new(tag: &str) -> Roots3 {
            let dir = std::env::temp_dir().join(format!("anicraft-cli-roots-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for d in ["work", "out", "secret"] {
                std::fs::create_dir_all(dir.join(d)).unwrap();
            }
            std::fs::write(dir.join("secret").join("s.json"), b"{\"steps\": []}").unwrap();
            let roots = std::sync::Arc::new(anicraft_engine::access::Roots::new(&[dir.join("work")], Some(&dir.join("out"))).unwrap());
            Roots3 { dir, roots }
        }
        pub fn path(&self, parts: &[&str]) -> String {
            parts.iter().fold(self.dir.clone(), |p, x| p.join(x)).display().to_string()
        }
    }

    /// AGENTS.md rule 18: `mcp` refuses to start without both roots, bridge or not, with the
    /// usage exit code, before it opens anything.
    #[test]
    fn mcp_refuses_to_start_without_both_automation_roots() {
        let r = Roots3::new("start");
        let run = |v: &[&str]| run(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let (work, out) = (r.path(&["work"]), r.path(&["out"]));
        for argv in [
            vec!["mcp"],
            vec!["mcp", "--sample"],
            vec!["mcp", "no-such-file.anicraft"],
            vec!["mcp", "--bridge", "9878"],
            vec!["mcp", "--automation-read-root", &work],
            vec!["mcp", "--automation-write-root", &out],
            vec!["mcp", "--bridge", "9878", "--automation-write-root", &out],
            vec!["mcp", "--automation-read-root", &work, "--automation-write-root", &out, "--automation-write-root", &out],
            vec!["mcp", "--automation-read-root", "", "--automation-write-root", &out],
            vec!["mcp", "--automation-read-root", &work, "--automation-write-root"],
        ] {
            let e = run(&argv).unwrap_err();
            assert!(e.contains("--automation-read-root") || e.contains("--automation-write-root"), "{argv:?}: {e}");
            assert_eq!(exit_code(&e), USAGE_EXIT, "{argv:?}: {e}");
        }
        // A root that is not a folder that exists stops the start too.
        let missing = r.path(&["nowhere"]);
        let e = run(&["mcp", "--automation-read-root", &missing, "--automation-write-root", &out]).unwrap_err();
        assert!(e.contains("nowhere") && exit_code(&e) == USAGE_EXIT, "{e}");
        let file = r.path(&["secret", "s.json"]);
        assert!(run(&["serve", "--automation-read-root", &work, "--automation-write-root", &file]).is_err());
        assert_eq!(exit_code("unknown subcommand 'x'"), 1);
        // The help says so.
        let help = run(&["mcp", "--help"]).unwrap();
        assert!(help.contains("--automation-read-root") && help.contains("--automation-write-root") && help.contains("required"), "{help}");
        // The flags parse: several read roots, one write root, relative ones from the launch folder.
        let a = Args::parse(&["--automation-read-root".into(), work.clone(), "--automation-read-root".into(), out.clone(), "--automation-write-root".into(), ".".into()]).unwrap();
        let (read, write) = automation_folders(&a).unwrap().unwrap();
        assert_eq!(read.len(), 2);
        assert!(write.is_absolute());
        assert!(automation_folders(&Args::parse(&[]).unwrap()).unwrap().is_none(), "serve without roots works as before");
        let eq = Args::parse(&[format!("--automation-read-root={work}"), format!("--automation-write-root={out}")]).unwrap();
        assert_eq!(automation_folders(&eq).unwrap().map(|(r, _)| r.len()), Some(1), "--flag=value");
        assert_eq!(exit_code(&run(&["mcp", "--automation-read-root=", &format!("--automation-write-root={out}")]).unwrap_err()), USAGE_EXIT);
    }

    #[test]
    fn serve_keeps_inside_its_roots_when_given_them() {
        let r = Roots3::new("serve");
        let mut s = Session::default();
        let req = |cmd: &str, path: String| json!({"cmd": cmd, "params": {"path": path}}).to_string();
        let refused = handle_request_in(&mut s, Some(&r.roots), &req("file.save_as", r.path(&["secret", "x.anicraft"])));
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("automation write root"), "{refused}");
        assert_eq!(handle_request_in(&mut s, Some(&r.roots), &req("file.save_as", r.path(&["out", "x.anicraft"])))["ok"], true);
        // Without roots, serve is the plain CLI it was.
        assert_eq!(handle_request(&mut s, &req("file.save_as", r.path(&["secret", "y.anicraft"])))["ok"], true);
    }

    #[test]
    fn float_flags_reject_nonfinite_values_without_changing_numeric_parsing() {
        for key in ["fps", "scale"] {
            let flag = format!("--{key}");
            for value in ["NaN", "inf", "+inf", "-inf", "1e40", "-1e40"] {
                let args = Args::parse(&[flag.clone(), value.into()]).unwrap();
                assert_eq!(args.float(key).unwrap_err(), format!("--{key} expects a finite number"));
            }
            for (value, expected) in [("0", 0.0), ("-1", -1.0), ("1.25", 1.25), ("1e2", 100.0)] {
                let args = Args::parse(&[flag.clone(), value.into()]).unwrap();
                assert_eq!(args.float(key).unwrap(), Some(expected));
            }
            let bad = Args::parse(&[flag.clone(), "oops".into()]).unwrap();
            assert_eq!(bad.float(key).unwrap_err(), format!("--{key} expects a number, got 'oops'"));
            assert_eq!(Args::parse(&[]).unwrap().float(key).unwrap(), None);
            assert!(Args::parse(&[flag]).is_err());
        }
        let integer = Args::parse(&["--frame".into(), "12".into()]).unwrap();
        assert_eq!(integer.num::<u32>("frame").unwrap(), Some(12));
        let bad_integer = Args::parse(&["--frame".into(), "NaN".into()]).unwrap();
        assert_eq!(bad_integer.num::<u32>("frame").unwrap_err(), "--frame expects a number, got 'NaN'");
    }

    /// #226: `info`, `render` and the other subcommands take an Animate document as File › Open
    /// does (an import, untitled), not only `.anicraft` files.
    #[test]
    fn subcommands_open_animate_documents() {
        let xfl = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../xfl/tests/corpus/shapes/shapes.xfl");
        let s = open_session(Some(&xfl.display().to_string())).unwrap();
        assert!(s.path.is_none() && !s.doc.timeline(0).layers.is_empty());
        let info = run(&["info".to_string(), xfl.display().to_string()]).unwrap();
        assert!(info.contains("\"layers\""), "{info}");
    }

    #[test]
    fn serve_refuses_ffmpeg_path_but_exec_keeps_it() {
        let mut s = Session::default();
        let r = handle_request(&mut s, r#"{"cmd":"file.export_video","params":{"path":"a.mp4","ffmpeg":"/bin/sh"}}"#);
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("ANICRAFT_FFMPEG"));
        // Every command that runs ffmpeg takes the same parameter, and none may be given it here.
        let takes_it: Vec<&str> = commands().iter().filter(|c| c.params.contains("ffmpeg?")).map(|c| c.id).collect();
        assert!(takes_it.contains(&"file.export_video") && takes_it.contains(&"file.import_video"), "{takes_it:?}");
        for id in takes_it {
            let r = handle_request(&mut s, &json!({"cmd": id, "params": {"path": "a.mp4", "ffmpeg": "/bin/sh"}}).to_string());
            assert!(r["error"].as_str().is_some_and(|e| e.contains("ANICRAFT_FFMPEG")), "{id}: {r}");
        }
        // Direct CLI use is not filtered: the command runs and fails on the missing program, not on the parameter.
        let e = exec(&mut s, "file.export_video", &json!({"path": "a.mp4", "ffmpeg": "/nonexistent/anicraft-ffmpeg"})).unwrap_err();
        assert!(!e.contains("ANICRAFT_FFMPEG environment variable"), "{e}");
    }

    #[test]
    fn long_lines_are_bounded() {
        let mut input = &b"abcdefghij\nok\r\nxy"[..];
        assert!(matches!(read_line(&mut input, 4).unwrap(), Line::TooLong));
        assert!(matches!(read_line(&mut input, 4).unwrap(), Line::Text(t) if t == "ok"));
        assert!(matches!(read_line(&mut input, 4).unwrap(), Line::Text(t) if t == "xy"));
        assert!(matches!(read_line(&mut input, 4).unwrap(), Line::Eof));
    }

    #[test]
    fn serve_round_trip() {
        let mut s = Session::default();
        let input = b"\xef\xbb\xbf{\"id\":1,\"cmd\":\"draw.rect\",\"params\":{\"x\":1,\"y\":1,\"w\":5,\"h\":5}}\nnot json\n{\"id\":3,\"cmd\":\"nope\"}\n\xef\xbb\xbf{\"cmd\":\"quit\"}\n{\"id\":4,\"cmd\":\"edit.undo\"}\n";
        let mut out = Vec::new();
        serve(&mut s, &input[..], &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["ready"], true);
        assert_eq!(lines[1]["ok"], true);
        assert_eq!(lines[2]["ok"], false);
        assert_eq!(lines[3]["ok"], false);
        assert_eq!(lines[4]["bye"], true);
        assert_eq!(lines.len(), 5, "nothing runs after quit");
        assert_eq!(s.visible_elements().len(), 1);
    }

    /// `fla-dump` reads a file only up to the legacy container's cap, and refuses one over it
    /// before parsing; a file that isn't a compound binary is refused by its first bytes.
    #[test]
    fn fla_dump_refuses_files_over_the_cap_and_other_formats() {
        let dir = std::env::temp_dir().join(format!("anicraft-cli-fla-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("new.fla");
        std::fs::write(&zip, b"PK\x03\x04").unwrap();
        let err = run(&["fla-dump".into(), zip.display().to_string()]).unwrap_err();
        assert!(err.contains("not a legacy Flash document"), "{err}");
        // One byte over the cap, sparse where the file system allows: refused, not parsed.
        #[cfg(unix)]
        {
            let big = dir.join("big.fla");
            let f = std::fs::File::create(&big).unwrap();
            std::io::Write::write_all(&mut &f, &anicraft_fla::MAGIC).unwrap();
            f.set_len(anicraft_fla::cfb::MAX_FILE + 1).unwrap();
            let err = run(&["fla-dump".into(), big.display().to_string()]).unwrap_err();
            assert!(err.contains("larger than AniCraft reads"), "{err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_names_the_workspace_version() {
        for flag in ["--version", "-V", "version"] {
            assert_eq!(run(&[flag.to_string()]).unwrap(), format!("anicraft-cli {}", env!("CARGO_PKG_VERSION")));
        }
    }

    #[test]
    fn render_rejects_unknown_options_before_opening_the_document() {
        let argv = ["render", "missing.anicraft", "--frmae", "12"].map(str::to_owned);
        let e = run(&argv).unwrap_err();
        assert!(e.starts_with("unknown render option --frmae
"), "{e}");
        assert!(e.contains("[--frame N]"), "the error shows what render takes: {e}");
        assert_eq!(exit_code(&e), USAGE_EXIT);
    }

    /// Every subcommand refuses an option it would drop, and none refuses one its usage names.
    #[test]
    fn every_subcommand_refuses_an_option_it_does_not_read() {
        let named = |cmd: &str| -> Vec<String> {
            let usage = subcommand_usage(cmd).unwrap_or_else(|| panic!("{cmd} has a usage entry"));
            // `--release` is cargo's, in the note on how to run the benchmark.
            let mut found: Vec<String> = Vec::new();
            // The entry lines and the bracketed lines under them; the notes beside them are prose.
            let syntax = usage.lines().filter(|l| l.starts_with("  anicraft-cli ") || l.trim_start().starts_with("[--"));
            for word in syntax.flat_map(|l| l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))) {
                if let Some(name) = word.strip_prefix("--").filter(|n| !n.is_empty() && !matches!(*n, "sample" | "plugins" | "version" | "release"))
                    && !found.iter().any(|f| f == name)
                {
                    found.push(name.to_string());
                }
            }
            found
        };
        for cmd in ["info", "render", "render-seq", "gif", "apng", "webp", "export", "formats", "commands", "run", "exec", "new", "guide", "bench", "serve", "mcp"] {
            let known = options_of(cmd).unwrap_or_else(|| panic!("{cmd} is a subcommand"));
            for name in named(cmd) {
                assert!(known.contains(&name.as_str()), "{cmd} --{name} is in the usage but would be refused");
            }
            for name in known {
                assert!(named(cmd).iter().any(|n| n == name), "{cmd} takes --{name}, which its usage doesn't name");
            }
            let argv = [cmd, "--no-such-option", "1", "--plugins", "nowhere"].map(str::to_owned);
            let e = run(&argv).unwrap_err();
            assert!(e.starts_with(&format!("unknown {cmd} option --no-such-option")), "{cmd}: {e}");
        }
        assert!(run(&["nope".to_string(), "--x".into(), "1".into()]).unwrap_err().starts_with("unknown subcommand"), "a wrong name is still the error for a wrong name");
    }

    #[test]
    fn render_sample_to_png() {
        let dir = std::env::temp_dir().join(format!("anicraft-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("f.png");
        let argv: Vec<String> = ["render", "--sample", "--frame", "12", "--scale", "0.25", "--out", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
        run(&argv).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        assert!(run(&["render".into(), "--sample".into(), "--frame".into(), "0".into(), "--out".into(), "x.png".into()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The budgets in docs/performance.md that don't depend on the machine, checked on every
    /// build. (Timing budgets are `bench --check yes`, on a known machine.)
    #[test]
    fn machine_independent_budgets() {
        use anicraft_doc::{evaluate, sample};
        use anicraft_engine::history::document_bytes;
        // 20 layers × 500 shapes × 120 frames: 100k element instances.
        let doc = sample::stress(20, 500, 120);
        let instances: usize = doc.timeline(0).layers.iter().flat_map(|l| l.keyframes.iter()).map(|k| k.elements.len()).sum();
        assert_eq!(instances, 100_000);

        // A snapshot (for undo, the render worker, autosave) shares everything: O(1).
        let snap = doc.clone();
        assert!(snap.scenes.iter().zip(&doc.scenes).all(|(a, b)| std::sync::Arc::ptr_eq(a, b)));

        // Undo memory per small edit < 16 KB, however large the document.
        let mut s = Session::new(doc);
        s.execute("layer.select", &json!({"index": 3})).unwrap();
        s.set_frame(30);
        let id = s.visible_elements()[0].id.0;
        s.execute("element.select", &json!({"ids": [id]})).unwrap();
        s.execute("element.translate", &json!({"dx": 5, "dy": 0})).unwrap();
        let step = s.history().bytes();
        assert!(step < 16 * 1024, "one edit kept {step} bytes of undo history (budget 16 KB)");
        assert!(step * 1000 < document_bytes(&s.doc), "an edit keeps what changed, not the document");

        // File size: < 400 bytes per element instance (shared paths are written once).
        let small = sample::stress(10, 100, 48);
        let n: usize = small.timeline(0).layers.iter().flat_map(|l| l.keyframes.iter()).map(|k| k.elements.len()).sum();
        let bytes = format::to_json(&small).unwrap().len();
        assert!(bytes < n * 400, "{} bytes per instance (budget 400)", bytes / n.max(1));

        // At 400% zoom on the stage centre, off-screen shapes are culled: at most 1/8 drawn.
        let dl = evaluate(&small, 5);
        let view = anicraft_render::View::stage(&small, 1.0);
        let zoomed = anicraft_render::View { transform: anicraft_geom::Affine::translate(-2880.0, -1620.0).mul(&anicraft_geom::Affine::scale(4.0, 4.0)), ..view };
        let (_, st) = anicraft_render::render(&dl, &zoomed).unwrap();
        assert!(st.items >= 1000 && st.drawn * 8 <= st.items, "{} of {} shapes drawn at 400%", st.drawn, st.items);
    }

    #[test]
    fn budget_check_reads_bench_output() {
        let mut out = json!({"edit_command_ms": 0.01, "undo_ms": 0.01, "snapshot_clone_us": 0.4, "evaluate_ms_per_frame": 1.5,
            "render_1080p_ms_per_frame": 20.0, "render_zoom400_ms_per_frame": 10.0, "hit_test_us": 50.0, "file": {"save_ms": 100.0}, "autosave": {"ui_us": 5.0, "background_ms": 300.0}});
        let (rows, missed) = check_budgets(&out);
        assert_eq!(rows.len(), TIME_BUDGETS.len());
        assert_eq!(missed, ["render 1080p (ms): 20 (budget 16.7)", "load the 65 MB file (ms): not measured"]);
        out["render_1080p_ms_per_frame"] = json!(16.0);
        out["file"]["load_ms"] = json!(120.0);
        assert!(check_budgets(&out).1.is_empty());
        // Debug builds and custom sizes can't be checked against the budgets.
        if cfg!(debug_assertions) {
            assert!(run(&["bench".into(), "--check".into(), "yes".into()]).unwrap_err().contains("release"));
        }
    }

    /// `bench --stroke` walks the reporter's steps of #519 (paint, pick, move, float, deselect,
    /// undo) and `--stroke-file` the same on a document: the stroke becomes one merged shape,
    /// the merge on deselect keeps it, and undo puts the drawing back as it was.
    #[test]
    fn stroke_bench_paints_moves_and_merges_a_long_stroke() {
        let out: Value = serde_json::from_str(&run(&["bench".into(), "--stroke".into(), "400".into()]).unwrap()).unwrap();
        assert_eq!(out["samples"], 400);
        assert_eq!(out["merged"], true);
        let segments = out["segments"].as_u64().unwrap();
        assert!(segments > 50, "a scribble has an outline of many curves: {segments}");
        assert_eq!(out["segments_after"].as_u64(), Some(segments), "undo takes the move and its merge back");
        for step in ["draw_brush_ms", "hit_test_us", "move_ms", "float_ms", "deselect_merge_ms", "undo_ms", "render_ms"] {
            assert!(out[step].as_f64().is_some_and(|v| v >= 0.0), "{step} was timed: {out}");
        }
        assert!(run(&["bench".into(), "--stroke".into(), "many".into()]).unwrap_err().contains("number of samples"));
        // The same steps on a saved document's artwork.
        let dir = std::env::temp_dir().join(format!("anicraft-cli-stroke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("stroke.anicraft");
        let mut s = Session::default();
        s.execute("draw.brush", &json!({"points": bench_stroke::scribble(200), "size": 8, "object": false})).unwrap();
        s.execute("file.save", &json!({"path": file.display().to_string()})).unwrap();
        let out: Value = serde_json::from_str(&run(&["bench".into(), "--stroke-file".into(), file.display().to_string()]).unwrap()).unwrap();
        assert!(out["samples"].is_null() && out["elements"].as_u64() == Some(1) && out["deselect_merge_ms"].is_number(), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugins_folder_flag() {
        let dir = std::env::temp_dir().join(format!("anicraft-cli-plugins-{}", std::process::id()));
        let argv: Vec<String> = ["--sample", "--plugins", dir.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
        let a = Args::parse(&argv).unwrap();
        let s = session(&a, Some("--sample")).unwrap();
        // A folder that doesn't exist yet is the plug-ins folder, with nothing in it.
        assert_eq!(s.plugins.folder(), Some(dir.as_path()));
        assert!(s.plugins.is_empty());
    }

    #[test]
    fn export_any_format_built_in_or_plugin() {
        let dir = std::env::temp_dir().join(format!("anicraft-cli-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        let argv = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        // A built-in format by id.
        let svg = dir.join("f.svg");
        run(&argv(&["export", "--sample", "--format", "svg", "--frame", "3", "--out", svg.to_str().unwrap()])).unwrap();
        assert!(std::fs::read_to_string(&svg).unwrap().contains("<svg"));
        // A sequence goes to a folder, one file per frame (#504).
        let seq = dir.join("vectors");
        run(&argv(&["export", "--sample", "--format", "svg-sequence", "--from", "2", "--to", "3", "--out", seq.to_str().unwrap()])).unwrap();
        assert!(std::fs::read_to_string(seq.join("frame_00002.svg")).unwrap().contains("<svg") && seq.join("frame_00003.svg").exists() && !seq.join("frame_00001.svg").exists());
        // An exporter plug-in that writes "frames:" and the count it was handed.
        let m = r#"{"id":"org.test.count","name":"Count","kind":"exporter","format":{"name":"Count","extension":"txt"}}"#;
        let req = r#"{"op":"movie"}"#;
        let wat = format!(
            r#"(module (import "anicraft" "call" (func $call (param i32 i32) (result i32))) (import "anicraft" "response" (func $response (param i32 i32) (result i32)))
              (memory (export "memory") 4) (data (i32.const 1024) "{}") (data (i32.const 4096) "{}")
              (func (export "ac_abi_version") (result i32) i32.const 1)
              (func (export "ac_manifest") (result i64) (i64.or (i64.shl (i64.const {}) (i64.const 32)) (i64.const 1024)))
              (func (export "ac_alloc") (param i32) (result i32) i32.const 60000)
              (func (export "ac_export") (param i32 i32) (result i64) (local $n i32)
                (local.set $n (call $response (i32.const 100000) (call $call (i32.const 4096) (i32.const {}))))
                (i64.or (i64.shl (i64.extend_i32_u (local.get $n)) (i64.const 32)) (i64.const 100000))))"#,
            m.replace('"', "\\22"),
            req.replace('"', "\\22"),
            m.len(),
            req.len()
        );
        std::fs::write(dir.join("plugins").join("count.wasm"), wat::parse_str(&wat).unwrap()).unwrap();
        let plugins = dir.join("plugins");
        let listed = run(&argv(&["formats", "--plugins", plugins.to_str().unwrap()])).unwrap();
        assert!(listed.contains("org.test.count") && listed.contains("\"gif\""));
        let out = dir.join("movie.txt");
        let v = run(&argv(&["export", "--sample", "--format", "org.test.count", "--from", "2", "--to", "5", "--out", out.to_str().unwrap(), "--plugins", plugins.to_str().unwrap()])).unwrap();
        assert!(v.contains("\"frames\":4"), "{v}");
        let movie: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((movie["result"]["from"].as_u64(), movie["result"]["frames"].as_u64()), (Some(1), Some(4)));
        // Replacing it needs --overwrite yes; unknown formats name the known ones.
        assert!(run(&argv(&["export", "--sample", "--format", "org.test.count", "--out", out.to_str().unwrap(), "--plugins", plugins.to_str().unwrap()])).is_err());
        run(&argv(&["export", "--sample", "--format", "org.test.count", "--out", out.to_str().unwrap(), "--plugins", plugins.to_str().unwrap(), "--overwrite", "yes"])).unwrap();
        let err = run(&argv(&["export", "--sample", "--format", "org.test.count", "--out", out.to_str().unwrap()])).unwrap_err();
        assert!(err.contains("lottie") && err.contains("--plugins"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_prefers_out_over_a_destination_in_options() {
        use anicraft_engine::vfs;

        vfs::use_memory();
        let export = |format: &str, out: &str, options: &str| {
            let argv = ["export", "--sample", "--format", format, "--out", out, "--options", options].map(str::to_string);
            serde_json::from_str::<Value>(&run(&argv).unwrap()).unwrap()
        };
        let written_paths = || vfs::take_written().into_iter().map(|(path, _)| path).collect::<Vec<_>>();

        let svg = export("svg", "chosen.svg", r#"{"path":"preset.svg","frame":2}"#);
        assert_eq!(svg["frame"], 2);
        assert_eq!(written_paths(), [PathBuf::from("chosen.svg")]);

        let sequence = export("png-sequence", "chosen-frames", r#"{"dir":"preset-frames","from":0,"to":0,"scale":0.05}"#);
        assert_eq!(sequence["frames"], 1);
        assert_eq!(written_paths(), [PathBuf::from("chosen-frames").join("frame_00001.png")]);
    }

    #[test]
    fn run_takes_a_saved_command_file() {
        let dir = std::env::temp_dir().join(format!("anicraft-cli-macro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Spin.json");
        std::fs::write(&file, r#"{"format": "anicraft-command", "version": 1, "steps": [{"cmd": "edit.select_all"}, {"cmd": "modify.rotate_cw"}]}"#).unwrap();
        let out = run(&["run".into(), "--sample".into(), file.display().to_string()]).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!((v["name"].as_str(), v["steps"].as_u64()), (Some("Spin"), Some(2)), "{out}");
        // A failing step names itself and leaves nothing behind.
        std::fs::write(&file, r#"{"steps": [{"cmd": "edit.select_all"}, {"cmd": "modify.rotate_cw"}, {"cmd": "layer.set_height", "params": {"height": 7}}]}"#).unwrap();
        let e = run(&["run".into(), "--sample".into(), file.display().to_string()]).unwrap_err();
        assert!(e.contains("stopped at step 3 of 3"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_and_errors() {
        assert!(run(&[]).unwrap().contains("USAGE"));
        assert!(run(&["bogus".into()]).is_err());
        assert!(run(&["info".into()]).is_err());
        assert!(run(&["commands".into()]).unwrap().contains("timeline.insert_keyframe"));
        assert!(run(&["guide".into()]).is_err());
    }

    #[test]
    fn guide_search_prints_chapter_heading_page_and_passage() {
        let s = |v: &[&str]| run(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let out = s(&["guide", "search", "onion", "skin"]).unwrap();
        let first = out.lines().next().unwrap_or("");
        assert_eq!(first, "1. Onion skin · The timeline", "{out}");
        assert!(out.contains("timeline.html#onion-skin"), "{out}");
        let json: Value = serde_json::from_str(&s(&["guide", "search", "export", "gif", "--json", "yes", "--limit", "3"]).unwrap()).unwrap();
        let list = json.as_array().unwrap();
        assert!(list.len() <= 3 && list[0]["url"] == "getting-started.html#export-a-gif-or-a-video", "{json}");
        assert!(s(&["guide", "search", "zzzzqqq"]).unwrap().starts_with("nothing in the user guide matches"));
        assert!(s(&["guide", "search"]).is_err() && s(&["guide", "search", "?!"]).is_err());
        assert!(s(&["guide", "search", "pen", "--limit", "x"]).is_err());
    }

    #[test]
    fn usage_clarifies_frame_numbering_split() {
        let help = run(&["help".into()]).unwrap();
        assert!(help.contains("0-based"), "CLI usage should clarify that JSON command frame numbers are 0-based");
        assert!(help.contains("1-based"), "CLI usage should clarify that command-line frame flags are 1-based");

        let dir = std::env::temp_dir().join(format!("anicraft-cli-frame-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out_cli = dir.join("cli.png");
        let out_exec = dir.join("exec.png");

        let res_cli: serde_json::Value = serde_json::from_str(
            &run(&["render".into(), "--sample".into(), "--frame".into(), "1".into(), "--out".into(), out_cli.to_str().unwrap().into()]).unwrap(),
        )
        .unwrap();

        let res_exec: serde_json::Value = serde_json::from_str(
            &run(&[
                "exec".into(),
                "--sample".into(),
                "file.export_png".into(),
                format!(r#"{{"path": "{}", "frame": 0}}"#, out_exec.to_str().unwrap().replace('\\', "/")),
            ])
            .unwrap(),
        )
        .unwrap();

        assert_eq!(res_cli["frame"], 0);
        assert_eq!(res_exec["frame"], 0);
        assert_eq!(std::fs::read(&out_cli).unwrap(), std::fs::read(&out_exec).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subcommand_help_prints_usage() {
        let subcommands = ["info", "render", "render-seq", "gif", "apng", "webp", "export", "formats", "commands", "run", "exec", "new", "guide", "bench", "serve", "mcp"];
        for sub in subcommands {
            let out_long = run(&[sub.to_string(), "--help".to_string()]).unwrap();
            assert!(out_long.contains("USAGE:"), "help for {sub} should contain USAGE:");
            assert!(out_long.contains(sub), "help for {sub} should mention {sub}");

            let out_short = run(&[sub.to_string(), "-h".to_string()]).unwrap();
            assert_eq!(out_long, out_short);

            let out_help = run(&["help".to_string(), sub.to_string()]).unwrap();
            assert_eq!(out_long, out_help);
        }

        // Subcommand help with other arguments present
        assert_eq!(
            run(&["render".into(), "--scale".into(), "2".into(), "--help".into()]).unwrap(),
            run(&["render".into(), "--help".into()]).unwrap()
        );

        // Unknown subcommand with --help or help still reports unknown
        assert!(run(&["bogus".into(), "--help".into()]).is_err());
        assert!(run(&["help".into(), "bogus".into()]).is_err());
    }

    #[test]
    fn subcommand_usage_matches_usage_text() {
        let subcommands =
            ["info", "render", "render-seq", "gif", "apng", "webp", "export", "formats", "commands", "run", "exec", "new", "guide", "bench", "serve", "mcp", "version"];
        for sub in subcommands {
            let u = subcommand_usage(sub).expect("subcommand_usage should exist");
            let body = u.strip_prefix("USAGE:\n").expect("should start with USAGE:\n");
            assert!(USAGE.contains(body), "USAGE should contain body for {sub}");
        }
        assert_eq!(subcommand_usage("bogus"), None);
        // Every entry of a subcommand with several comes along, with its continuation lines.
        let guide = subcommand_usage("guide").unwrap_or_default();
        assert!(guide.contains("guide --out") && guide.contains("guide search") && guide.contains("best first"), "{guide}");
        assert!(!guide.contains("bench"), "{guide}");
        let export = subcommand_usage("export").unwrap_or_default();
        assert!(export.contains("--options") && export.contains("exporter plug-ins"), "{export}");
        // Words inside an entry are not subcommands.
        for word in ["anicraft-cli", "--sample", "file", "", "apng|webp", "--plugins"] {
            assert_eq!(subcommand_usage(word), None, "{word:?}");
        }
    }
}
