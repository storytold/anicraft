//! AniCraft headless front end: the command line, the JSON-lines `serve` loop and the MCP
//! server (`mcp`). Every operation goes through the engine's command registry. `run` is the whole
//! program, so it can be tested in-process as well as through the binary; `anicraft-cli --help`
//! lists the subcommands. The conventions are RFC-0018's: exit 0, 1 (a command failed) or 2 (a
//! usage error), results on stdout, `error: <message>` on stderr.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod args;
mod bench_stroke;
#[cfg(not(target_arch = "wasm32"))]
pub mod control;
mod error;
pub mod mcp;

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anicraft_engine::access::{self, Roots};
use anicraft_engine::{Session, commands, format};
use serde_json::{Value, json};

use args::{
    BenchArgs, Cli, Command, Document, ExecArgs, ExportArgs, FlaDumpArgs, FormatsArgs, GifArgs, GuideArgs, GuideCommand, InfoArgs, Loop, McpArgs,
    MotionArgs, NewArgs, OptionalDocument, Plugins, RenderSeqArgs, RunArgs, SearchArgs, ServeArgs, parse,
};
pub use error::CliError;

/// Writes a line. A closed pipe (`anicraft-cli commands | head`) is not worth an error: the rest
/// of the output is dropped.
macro_rules! outln {
    ($to:expr, $($arg:tt)*) => {
        drop(writeln!($to, $($arg)*))
    };
}

/// The clap definition of the command line: what `--help`, the completions and the man pages are
/// made from.
pub fn command() -> clap::Command {
    args::command()
}

/// Runs the CLI on its arguments (without the program name), writing results to `out` and
/// diagnostics to `err`. Returns the process exit code: 0 on success, 1 when a command failed, 2
/// for a usage error. Help and the version go to `out`, exit 0.
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let failed = |e: CliError, err: &mut dyn Write| {
        outln!(err, "error: {e}");
        e.exit_code()
    };
    if let Some(argument) = nightly_protocol_argument(&args) {
        return failed(CliError::NightlyProtocol { argument }, err);
    }
    // `--screenshot` is a desktop-app feature: hand over to the `anicraft` binary next to us.
    if args.iter().any(|a| a == "--screenshot") {
        return delegate_to_desktop(&args).unwrap_or_else(|e| failed(e, err));
    }
    // Help and the version (`help`, `-h`, `<subcommand> --help`, `-V`) come back as errors too.
    let cli = match parse(&args) {
        Ok(cli) => cli,
        Err(e) => {
            let to: &mut dyn Write = if e.use_stderr() { err } else { out };
            let _ = write!(to, "{}", e.render());
            return e.exit_code();
        }
    };
    match dispatch(cli, out, err) {
        Ok(()) => 0,
        Err(e) => failed(e, err),
    }
}

/// The Nightly launcher's two arguments, if the command line carries either (also as `=` forms):
/// this headless tool has no launch profile and no window to put in one, so it refuses them before
/// any subcommand is read.
fn nightly_protocol_argument(argv: &[OsString]) -> Option<String> {
    argv.iter()
        .filter_map(|a| a.to_str())
        .find(|a| *a == "--launch-profile" || *a == "--profile-root" || a.starts_with("--launch-profile=") || a.starts_with("--profile-root="))
        .map(str::to_owned)
}

/// Runs the desktop app next to this binary with the same arguments, and returns the exit code it
/// ended with: 128 + the signal when a signal ended it, as the shell reports it.
fn delegate_to_desktop(argv: &[OsString]) -> Result<i32, CliError> {
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(format!("anicraft{}", std::env::consts::EXE_SUFFIX))));
    let Some(exe) = exe.filter(|p| p.exists()) else { return Err(CliError::NoDesktopApp) };
    let status = std::process::Command::new(&exe).args(argv).status().map_err(|source| CliError::StartDesktop { path: exe, source })?;
    Ok(exit_code_of(status))
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

fn dispatch(cli: Cli, mut out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    match cli.command {
        Command::Info(InfoArgs { document, plugins }) => {
            let s = session(&plugins, source(&document), err)?;
            outln!(out, "{}", serde_json::to_string_pretty(&s.info())?);
        }
        Command::FlaDump(a) => fla_dump(&a, out)?,
        Command::Render(a) => {
            refuse_replacing_input(a.document.file.as_deref(), &a.output, "--out")?;
            let mut s = session(&a.plugins, source(&a.document), err)?;
            let mut p = json!({"path": unicode(&a.output)?, "frame": a.frame.map_or(0, zero_based)});
            if let Some(sc) = a.scale {
                p["scale"] = json!(sc);
            }
            outln!(out, "{}", exec(&mut s, "file.export_png", &p)?);
        }
        Command::RenderSeq(a) => render_seq(&a, out, err)?,
        Command::Gif(a) => gif(&a, out, err)?,
        Command::Apng(a) => motion("apng", &a, out, err)?,
        Command::Webp(a) => motion("webp", &a, out, err)?,
        Command::Export(a) => export(&a, out, err)?,
        Command::Formats(FormatsArgs { plugins }) => {
            let mut s = session(&plugins, Source::New, err)?;
            let v = exec(&mut s, "file.export_formats", &json!({}))?;
            outln!(out, "{}", serde_json::to_string_pretty(&v["formats"])?);
        }
        Command::Commands => {
            let list: Vec<Value> = commands().iter().map(|c| c.to_json()).collect();
            outln!(out, "{}", serde_json::to_string_pretty(&list)?);
        }
        Command::Run(a) => run_script(&a, out, err)?,
        Command::Exec(a) => exec_command(&a, out, err)?,
        Command::New(a) => new_document(&a, out)?,
        Command::Guide(GuideArgs { search: Some(GuideCommand::Search(a)), .. }) => guide_search(&a, out)?,
        Command::Guide(GuideArgs { output: Some(dir), .. }) => {
            let n = anicraft_guide::write_site(&dir).map_err(|source| CliError::Io { path: dir.clone(), source })?;
            outln!(out, "wrote {n} files of the user guide to {}; open {}/index.html", dir.display(), dir.display());
        }
        Command::Guide(_) => {}
        Command::Bench(a) => bench(&a, out)?,
        Command::Serve(a) => serve_command(&a, &mut out, err)?,
        Command::Mcp(a) => mcp_command(&a, &mut out, err)?,
    }
    Ok(())
}

/// What a session opens.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// A new, empty document.
    New,
    /// The built-in sample.
    Sample,
    /// A file.
    File(&'a Path),
}

fn source(document: &Document) -> Source<'_> {
    pick(document.file.as_deref(), document.sample)
}

fn optional_source(document: &OptionalDocument) -> Source<'_> {
    pick(document.file.as_deref(), document.sample)
}

fn pick(file: Option<&Path>, sample: bool) -> Source<'_> {
    match (file, sample) {
        (Some(file), _) => Source::File(file),
        (None, true) => Source::Sample,
        (None, false) => Source::New,
    }
}

/// A path as the text the engine's commands take: they hold paths in JSON, which holds only text.
fn unicode(path: &Path) -> Result<&str, CliError> {
    path.to_str().ok_or_else(|| CliError::NotUnicode(path.to_owned()))
}

/// Frame numbers on the command line count from 1; the engine's from 0. clap already refused 0.
fn zero_based(frame: u32) -> u32 {
    frame.saturating_sub(1)
}

/// Whether two paths name the same existing file, through links and `.` and `..`.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// RFC-0018 rule 1.6: an export never replaces the document it was made from.
fn refuse_replacing_input(input: Option<&Path>, output: &Path, flag: &'static str) -> Result<(), CliError> {
    match input {
        Some(input) if same_file(input, output) => Err(CliError::WouldReplace { output: flag, path: output.to_owned(), in_place_helps: false }),
        _ => Ok(()),
    }
}

/// `--save` onto the opened document replaces it, which only `--in-place` allows; `--in-place`
/// with a `--save` elsewhere has nothing to allow.
fn check_save(input: Option<&Path>, save: Option<&Path>, in_place: bool) -> Result<(), CliError> {
    let Some(save) = save else { return Ok(()) };
    match (input.is_some_and(|input| same_file(input, save)), in_place) {
        (true, false) => Err(CliError::WouldReplace { output: "--save", path: save.to_owned(), in_place_helps: true }),
        (false, true) => Err(CliError::InPlaceNotUsed),
        _ => Ok(()),
    }
}

/// The folders given with `--automation-read-root` (once or more) and `--automation-write-root`
/// (once) as a policy: relative ones from the launch folder, each resolved (links followed) and
/// checked to be a folder that exists.
fn automation_roots(read: &[PathBuf], write: &Path) -> Result<Arc<Roots>, CliError> {
    let folder =
        |flag: &'static str, path: &Path| std::path::absolute(path).map_err(|source| CliError::RootPath { flag, path: path.to_owned(), source });
    let read = read.iter().map(|path| folder("read", path)).collect::<Result<Vec<_>, _>>()?;
    Roots::new(&read, Some(&folder("write", write)?)).map(Arc::new).map_err(CliError::Roots)
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

/// Opens a session from a path or the sample.
pub fn open_session(src: Source<'_>) -> Result<Session, CliError> {
    match src {
        Source::New => Ok(Session::default()),
        Source::Sample => Ok(Session::new(anicraft_doc::sample::demo())),
        // An Animate document imports as File › Open does (#226), untitled.
        Source::File(p) if anicraft_engine::is_animate(p) => {
            let mut s = Session::default();
            exec(&mut s, "file.open", &json!({"path": unicode(p)?}))?;
            Ok(s)
        }
        Source::File(p) => {
            let doc = format::load(p).map_err(|source| CliError::Open { path: p.to_owned(), source })?;
            let mut s = Session::new(doc);
            s.path = Some(p.to_owned());
            Ok(s)
        }
    }
}

/// [`open_session`], with the plug-ins of `--plugins <folder>` (or `ANICRAFT_PLUGINS`) loaded.
fn session(plugins: &Plugins, src: Source<'_>, err: &mut dyn Write) -> Result<Session, CliError> {
    let mut s = open_session(src)?;
    if let Some(folder) = &plugins.plugins {
        let r = exec(&mut s, "plugin.reload", &json!({"folder": unicode(folder)?}))?;
        for f in r["failed"].as_array().into_iter().flatten() {
            outln!(err, "plug-in {} not loaded: {}", f["file"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or(""));
        }
    }
    Ok(s)
}

fn exec(s: &mut Session, id: &str, params: &Value) -> Result<Value, CliError> {
    s.execute(id, params).map_err(|source| CliError::Command { id: id.to_owned(), source })
}

/// `file.save` to `path`.
fn save(s: &mut Session, path: &Path) -> Result<(), CliError> {
    exec(s, "file.save", &json!({"path": unicode(path)?})).map(drop)
}

/// The `from`, `to` and `scale` parameters of the commands that take a frame range.
fn range_params(p: &mut Value, range: &args::Range) {
    for (key, frame) in [("from", range.from), ("to", range.to)] {
        if let Some(f) = frame {
            p[key] = json!(zero_based(f));
        }
    }
    if let Some(sc) = range.scale {
        p["scale"] = json!(sc);
    }
}

fn fla_dump(a: &FlaDumpArgs, out: &mut dyn Write) -> Result<(), CliError> {
    // A file over the container's cap is refused by its size, and the read stops one byte past
    // the cap, so one that grows in between is refused before parsing too.
    let (path, max) = (a.file.as_path(), anicraft_fla::cfb::MAX_FILE);
    let io = |source| CliError::Io { path: path.to_owned(), source };
    let fla = |source| CliError::Fla { path: path.to_owned(), source };
    if anicraft_engine::vfs::len(path).map_err(io)? > max {
        return Err(fla(anicraft_fla::cfb::Error::TooLarge));
    }
    let data = anicraft_engine::vfs::read_bounded(path, max).map_err(io)?;
    if data.len() as u64 > max {
        return Err(fla(anicraft_fla::cfb::Error::TooLarge));
    }
    if !anicraft_fla::is_legacy(&data) {
        return Err(CliError::NotLegacyFla { path: path.to_owned() });
    }
    // With --stream, only that stream is read: another one's damage can't fail it.
    let streams = anicraft_fla::dump_only(&data, a.stream.as_deref()).map_err(fla)?;
    let list: Vec<Value> = streams
        .iter()
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
    outln!(out, "{}", serde_json::to_string_pretty(&list)?);
    Ok(())
}

fn render_seq(a: &RenderSeqArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    let mut s = session(&a.plugins, source(&a.document), err)?;
    let mut p = json!({"dir": unicode(&a.output)?});
    range_params(&mut p, &a.range);
    outln!(out, "{}", exec(&mut s, "file.export_png_sequence", &p)?);
    Ok(())
}

fn gif(a: &GifArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    refuse_replacing_input(a.document.file.as_deref(), &a.output, "--out")?;
    let mut s = session(&a.plugins, source(&a.document), err)?;
    let mut p = json!({"path": unicode(&a.output)?});
    range_params(&mut p, &a.range);
    if a.overwrite {
        p["confirm"] = json!(true);
    }
    outln!(out, "{}", exec(&mut s, "file.export_gif", &p)?);
    Ok(())
}

/// `apng` and `webp`: full colour with real transparency.
fn motion(format: &str, a: &MotionArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    refuse_replacing_input(a.document.file.as_deref(), &a.output, "--out")?;
    let mut s = session(&a.plugins, source(&a.document), err)?;
    let mut p = json!({"path": unicode(&a.output)?});
    range_params(&mut p, &a.range);
    if a.overwrite {
        p["confirm"] = json!(true);
    }
    if let Some(Loop::Times(n)) = a.loops {
        p["loop"] = json!(n);
    }
    if a.background {
        p["background"] = json!(true);
    }
    outln!(out, "{}", exec(&mut s, &format!("file.export_{format}"), &p)?);
    Ok(())
}

/// `export --format <id>`: a built-in format through its own command, or an exporter plug-in
/// through `file.export_plugin` (ADR 0088).
fn export(a: &ExportArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    refuse_replacing_input(a.document.file.as_deref(), &a.output, "--out")?;
    // One frame, or a range: png and svg take the first, the others the second.
    let one_frame = matches!(a.format.as_str(), "png" | "svg");
    if one_frame && (a.from.is_some() || a.to.is_some()) {
        return Err(CliError::FlagNotUsed { flag: if a.from.is_some() { "--from" } else { "--to" }, format: a.format.clone() });
    }
    if !one_frame && a.frame.is_some() {
        return Err(CliError::FlagNotUsed { flag: "--frame", format: a.format.clone() });
    }
    let mut s = session(&a.plugins, source(&a.document), err)?;
    let list = exec(&mut s, "file.export_formats", &json!({}))?;
    let formats = list["formats"].as_array().cloned().unwrap_or_default();
    let Some(f) = formats.iter().find(|f| f["id"] == a.format.as_str()) else {
        let ids: Vec<&str> = formats.iter().filter_map(|f| f["id"].as_str()).collect();
        return Err(CliError::UnknownFormat { id: a.format.clone(), known: ids.join(", ") });
    };
    let (cmd, target, plugin) = (f["command"].as_str().unwrap_or(""), f["target"].as_str().unwrap_or("path"), f["plugin"] == true);
    let mut p = json!({});
    if plugin {
        p["format"] = json!(a.format);
        if let Some(o) = &a.options {
            p["options"] = o.clone();
        }
    } else if let Some(Value::Object(o)) = &a.options {
        for (k, v) in o {
            p[k] = v.clone();
        }
    } else if a.options.is_some() {
        return Err(CliError::OptionsNotObject);
    }
    p[target] = json!(unicode(&a.output)?);
    for (key, frame) in [("frame", a.frame), ("from", a.from), ("to", a.to)] {
        if let Some(f) = frame {
            p[key] = json!(zero_based(f));
        }
    }
    if a.overwrite {
        p["confirm"] = json!(true);
    }
    outln!(out, "{}", exec(&mut s, cmd, &p)?);
    Ok(())
}

fn run_script(a: &RunArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    check_save(a.file.as_deref(), a.save.as_deref(), a.in_place)?;
    let src = pick(a.file.as_deref(), a.sample);
    let mut s = session(&a.plugins, src, err)?;
    let script = &a.script;
    let text = std::fs::read_to_string(script).map_err(|source| CliError::Io { path: script.clone(), source })?;
    // Windows editors often save UTF-8 with a byte-order mark; JSON parsers reject it.
    let parsed: Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|source| CliError::ScriptSyntax { path: script.clone(), source })?;
    // A saved command (ADR 0097) runs as one transactional step, as in the app.
    if parsed.is_object() {
        let v = exec(&mut s, "macro.run", &json!({"path": unicode(script)?}))?;
        if let Some(path) = &a.save {
            save(&mut s, path)?;
        }
        outln!(out, "{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let steps: Vec<Value> = serde_json::from_value(parsed).map_err(|source| CliError::ScriptShape { path: script.clone(), source })?;
    let mut results = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let id = step.get("cmd").and_then(Value::as_str).ok_or(CliError::MissingCmd { step: n })?;
        let params = step.get("params").cloned().unwrap_or_else(|| json!({}));
        results.push(exec(&mut s, id, &params).map_err(|e| CliError::Step { step: n, source: Box::new(e) })?);
    }
    if let Some(path) = &a.save {
        save(&mut s, path)?;
    }
    outln!(out, "{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

fn exec_command(a: &ExecArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    check_save(a.file.as_deref(), a.save.as_deref(), a.in_place)?;
    let mut s = session(&a.plugins, pick(a.file.as_deref(), a.sample), err)?;
    let v = exec(&mut s, &a.command, &a.params)?;
    if let Some(path) = &a.save {
        save(&mut s, path)?;
    }
    outln!(out, "{v}");
    Ok(())
}

fn new_document(a: &NewArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let mut s = Session::default();
    let mut p = json!({});
    for (key, v) in [("width", a.width), ("height", a.height)] {
        if let Some(v) = v {
            p[key] = json!(v);
        }
    }
    if let Some(v) = a.fps {
        p["fps"] = json!(v);
    }
    exec(&mut s, "file.new", &p)?;
    outln!(out, "{}", exec(&mut s, "file.save", &json!({"path": unicode(&a.output)?}))?);
    Ok(())
}

/// The user guide as the static HTML book Help › User Guide opens (ADR 0069): searching it, with
/// the ranking the book's search page uses (ADR 0096).
fn guide_search(a: &SearchArgs, out: &mut dyn Write) -> Result<(), CliError> {
    let words = a.words.join(" ");
    if anicraft_guide::search::terms(&words).is_empty() {
        return Err(CliError::NoSearchTerms);
    }
    let hits =
        anicraft_guide::search::search(anicraft_guide::PAGES, &anicraft_guide::search::sections(anicraft_guide::PAGES), &words, a.limit as usize);
    if a.json {
        let list: Vec<Value> =
            hits.iter().map(|h| json!({"chapter": h.chapter, "heading": h.heading, "url": h.url, "snippet": h.snippet, "score": h.score})).collect();
        outln!(out, "{}", serde_json::to_string_pretty(&list)?);
    } else if hits.is_empty() {
        outln!(out, "nothing in the user guide matches “{}”", words.trim());
    } else {
        let lines: Vec<String> =
            hits.iter().enumerate().map(|(i, h)| format!("{}. {} · {}\n   {}\n   {}", i + 1, h.heading, h.chapter, h.url, h.snippet)).collect();
        outln!(out, "{}", lines.join("\n\n"));
    }
    Ok(())
}

fn serve_command(a: &ServeArgs, out: &mut &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    // Before anything is opened: a bad root stops the start.
    let roots = a.roots().map(|(read, write)| automation_roots(read, write)).transpose()?;
    let mut s = session(&a.plugins, optional_source(&a.document), err)?;
    serve_in(&mut s, roots.as_ref(), std::io::stdin().lock(), out).map_err(CliError::Stdio)
}

fn mcp_command(a: &McpArgs, out: &mut &mut dyn Write, err: &mut dyn Write) -> Result<(), CliError> {
    // An agent's entry point refuses to start without both roots (AGENTS.md rule 18; clap
    // requires them), before any file is opened or any connection made.
    let roots = automation_roots(&a.read_root, &a.write_root)?;
    if let Some(target) = &a.bridge {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = target;
            return Err(CliError::NoSockets);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let addr = control::parse_bridge_target(target).map_err(|reason| CliError::BridgeTarget { target: target.clone(), reason })?;
            let mut b = control::Bridge::new(addr).with_roots(&roots).map_err(CliError::Bridge)?;
            return mcp::serve(&mut b, std::io::stdin().lock(), out).map_err(CliError::Stdio);
        }
    }
    // The file and plug-ins named at the start are the person's own choice; the roots apply to
    // what the assistant asks for from here on.
    let mut s = mcp::Headless { session: session(&a.plugins, optional_source(&a.document), err)?, roots };
    mcp::serve(&mut s, std::io::stdin().lock(), out).map_err(CliError::Stdio)
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
pub(crate) fn bench(a: &BenchArgs, stdout: &mut dyn Write) -> Result<(), CliError> {
    if a.stroke.is_some() || a.stroke_file.is_some() {
        outln!(stdout, "{}", bench_stroke::bench_stroke(a)?);
        return Ok(());
    }
    let check = a.check;
    if check && cfg!(debug_assertions) {
        return Err(CliError::CheckNeedsRelease);
    }
    use anicraft_doc::{evaluate, sample};
    use anicraft_engine::history::document_bytes;
    use anicraft_render::{View, render};

    let layers = a.layers.unwrap_or(50);
    let shapes = a.shapes.unwrap_or(200);
    let frames = a.frames.unwrap_or(240);
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
        outln!(stdout, "{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    let (rows, missed) = check_budgets(&out);
    // The report is the result, on stdout; the budgets that were missed are the error.
    outln!(stdout, "{}", serde_json::to_string_pretty(&json!({"measured": out, "budgets": rows}))?);
    if missed.is_empty() { Ok(()) } else { Err(CliError::OverBudget(missed.join("; "))) }
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
mod tests;
