//! The command line as clap derive types. Values arrive typed, so the commands never validate a
//! string; what clap can't say alone is a usage error built here, printed and exited (2) like
//! clap's own. The help text is the doc comments (RFC-0018 rule 1.8).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::builder::BoolishValueParser;
use clap::error::ErrorKind;
use clap::{Arg, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::Value;

/// The end of `--help` of every command.
const EXIT_STATUS: &str = "Exit status:\n  0  success\n  1  a command failed\n  2  usage error\n\nA process ended by a signal exits 128 + the signal number (130 for Ctrl-C), as in the shell.";

/// What a document argument is.
const DOCUMENT_NOTE: &str = "The document is an .anicraft file, an Animate document (.fla, .xfl) or `--sample`, a built-in sample.";

const FRAMES_NOTE: &str = "Frame flags on the command line (--frame, --from, --to) are 1-based, as in the timeline. Frame numbers inside JSON params, scripts and command results are 0-based, as everywhere in the engine.";

const ROOTS_NOTE: &str = "Commands read files only inside the read roots and write only inside the write root: links and `..` are followed first, and a refusal names the root. --automation-read-root may be given several times (docs/mcp.md).";

/// AniCraft without a window: headless animation engine, MCP server and the app's control channel
#[derive(Parser, Debug)]
#[command(name = "anicraft-cli", version, propagate_version = true, arg_required_else_help = true, after_help = EXIT_STATUS, after_long_help = format!("{FRAMES_NOTE}\n\n{EXIT_STATUS}"))]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Print a document's state as JSON
    #[command(after_help = DOCUMENT_NOTE)]
    Info(InfoArgs),
    /// List the streams and CPic* record tags of a legacy Flash 8-CS4 .fla file as JSON
    FlaDump(FlaDumpArgs),
    /// Render one frame to a PNG file
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    Render(RenderArgs),
    /// Render a frame range to numbered PNG files in a folder
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    RenderSeq(RenderSeqArgs),
    /// Write an animated GIF
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    Gif(GifArgs),
    /// Write an animated PNG, in full colour with real transparency
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    Apng(MotionArgs),
    /// Write an animated WebP (lossless), in full colour with real transparency
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    Webp(MotionArgs),
    /// Export in any format: a built-in one or an exporter plug-in
    ///
    /// The built-in formats are gif, apng, webp, lottie, html, video, svg, png, png-sequence,
    /// svg-sequence and sprite-sheet; exporter plug-ins are named by id and need --plugins.
    /// `formats` lists them all. --options are the plug-in's options, or a built-in's extra
    /// params, as a JSON object.
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{FRAMES_NOTE}"))]
    Export(ExportArgs),
    /// List every export format, plug-ins included, as JSON
    Formats(FormatsArgs),
    /// List every command with its parameters, as JSON
    Commands,
    /// Run a script on a document: a JSON array of {cmd, params}, or a saved command
    ///
    /// The steps are one undo step, and nothing changes if one fails. With FILE, the document is
    /// FILE; with --sample, the first argument is the script.
    #[command(after_help = DOCUMENT_NOTE, override_usage = "anicraft-cli run [OPTIONS] <FILE> <SCRIPT>\n       anicraft-cli run [OPTIONS] --sample <SCRIPT>")]
    Run(RunArgs),
    /// Run one command on a document and print its result
    ///
    /// With FILE, the arguments are FILE, the command id and its JSON params; with --sample, the
    /// command id and its JSON params.
    #[command(after_help = DOCUMENT_NOTE, override_usage = "anicraft-cli exec [OPTIONS] <FILE> <COMMAND> [PARAMS]\n       anicraft-cli exec [OPTIONS] --sample <COMMAND> [PARAMS]")]
    Exec(ExecArgs),
    /// Create an empty document
    New(NewArgs),
    /// Write the user guide as HTML pages, or search it
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Guide(GuideArgs),
    /// Performance probe on a generated stress document (use --release)
    Bench(BenchArgs),
    /// Serve JSON-lines requests on stdin and stdout
    ///
    /// One request per line, for example
    /// {"id":1,"cmd":"draw.rect","params":{"x":10,"y":10,"w":50,"h":50}}.
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{ROOTS_NOTE}"))]
    Serve(ServeArgs),
    /// Serve the Model Context Protocol on stdin and stdout, for AI assistants (docs/mcp.md)
    ///
    /// Both roots are required: an assistant's commands read files only inside the read roots and
    /// write only inside the write root. With --bridge, the same tools act live on the AniCraft
    /// window started with `anicraft --control <port>` (docs/control-protocol.md); the window
    /// applies the roots to this assistant's commands.
    #[command(after_help = format!("{DOCUMENT_NOTE}\n{ROOTS_NOTE}"))]
    Mcp(McpArgs),
}

/// A document to open: a path, or the built-in sample.
#[derive(Args, Debug)]
pub(crate) struct Document {
    /// The document
    #[arg(value_name = "FILE", required_unless_present = "sample")]
    pub(crate) file: Option<PathBuf>,
    /// Use the built-in sample instead of a file
    #[arg(long, conflicts_with = "file")]
    pub(crate) sample: bool,
}

/// A document that may be left out: a new, empty one then.
#[derive(Args, Debug)]
pub(crate) struct OptionalDocument {
    /// The document
    #[arg(value_name = "FILE")]
    pub(crate) file: Option<PathBuf>,
    /// Use the built-in sample instead of a file
    #[arg(long, conflicts_with = "file")]
    pub(crate) sample: bool,
}

/// `--plugins`, for what opens a session.
#[derive(Args, Debug)]
pub(crate) struct Plugins {
    /// Load the WebAssembly plug-ins in a folder first
    #[arg(long, value_name = "FOLDER", env = "ANICRAFT_PLUGINS", value_parser = non_empty_path)]
    pub(crate) plugins: Option<PathBuf>,
}

/// `--from` and `--to` (1-based, inclusive) and `--scale`.
#[derive(Args, Debug)]
pub(crate) struct Range {
    /// First frame, counting from 1
    #[arg(long, value_name = "N", value_parser = frame_number)]
    pub(crate) from: Option<u32>,
    /// Last frame, counting from 1
    #[arg(long, value_name = "N", value_parser = frame_number)]
    pub(crate) to: Option<u32>,
    /// Scale of the stage, 1 for its own size
    #[arg(long, value_name = "S", value_parser = finite_number)]
    pub(crate) scale: Option<f32>,
}

#[derive(Args, Debug)]
pub(crate) struct InfoArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct FlaDumpArgs {
    /// A legacy .fla file
    #[arg(value_name = "FILE")]
    pub(crate) file: PathBuf,
    /// Only this stream's tags
    #[arg(long, value_name = "NAME")]
    pub(crate) stream: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct RenderArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    /// The PNG file to write
    #[arg(long = "out", value_name = "FRAME.PNG")]
    pub(crate) output: PathBuf,
    /// The frame, counting from 1 (the first one without it)
    #[arg(long, value_name = "N", value_parser = frame_number)]
    pub(crate) frame: Option<u32>,
    /// Scale of the stage, 1 for its own size
    #[arg(long, value_name = "S", value_parser = finite_number)]
    pub(crate) scale: Option<f32>,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct RenderSeqArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    /// The folder to write frame_00001.png, ... into
    #[arg(long = "out-dir", value_name = "DIR")]
    pub(crate) output: PathBuf,
    #[command(flatten)]
    pub(crate) range: Range,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct GifArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    /// The GIF file to write
    #[arg(long = "out", value_name = "ANIM.GIF")]
    pub(crate) output: PathBuf,
    #[command(flatten)]
    pub(crate) range: Range,
    /// Replace the file if it exists (`--overwrite yes` still works)
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true)]
    pub(crate) overwrite: bool,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

/// How many times an animation plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loop {
    Forever,
    Times(u32),
}

fn loop_count(text: &str) -> Result<Loop, String> {
    if text == "forever" {
        return Ok(Loop::Forever);
    }
    text.parse().map(Loop::Times).map_err(|_| format!("forever, or how many times it plays, not '{text}'"))
}

#[derive(Args, Debug)]
pub(crate) struct MotionArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    /// The file to write (.png for apng, .webp for webp)
    #[arg(long = "out", value_name = "ANIM")]
    pub(crate) output: PathBuf,
    #[command(flatten)]
    pub(crate) range: Range,
    /// How many times it plays: forever, or a number
    #[arg(long = "loop", value_name = "forever|N", value_parser = loop_count)]
    pub(crate) loops: Option<Loop>,
    /// Paint the stage background instead of leaving it transparent
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true)]
    pub(crate) background: bool,
    /// Replace the file if it exists (`--overwrite yes` still works)
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true)]
    pub(crate) overwrite: bool,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct ExportArgs {
    #[command(flatten)]
    pub(crate) document: Document,
    /// The format id (`formats` lists them)
    #[arg(long, value_name = "ID")]
    pub(crate) format: String,
    /// The file to write, or the folder for a sequence
    #[arg(long = "out", value_name = "PATH")]
    pub(crate) output: PathBuf,
    /// First frame, counting from 1
    #[arg(long, value_name = "N", value_parser = frame_number, conflicts_with = "frame")]
    pub(crate) from: Option<u32>,
    /// Last frame, counting from 1
    #[arg(long, value_name = "N", value_parser = frame_number, conflicts_with = "frame")]
    pub(crate) to: Option<u32>,
    /// The frame of png and svg, counting from 1
    #[arg(long, value_name = "N", value_parser = frame_number)]
    pub(crate) frame: Option<u32>,
    /// Extra parameters of the format, as a JSON object
    #[arg(long, value_name = "JSON", value_parser = json_text)]
    pub(crate) options: Option<Value>,
    /// Replace the file if it exists (`--overwrite yes` still works)
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true)]
    pub(crate) overwrite: bool,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct FormatsArgs {
    #[command(flatten)]
    pub(crate) plugins: Plugins,
}

#[derive(Args, Debug)]
pub(crate) struct RunArgs {
    /// FILE and the script, or only the script with --sample
    #[arg(value_names = ["FILE", "SCRIPT"], num_args = 1..=2, required = true)]
    inputs: Vec<OsString>,
    /// Use the built-in sample instead of a file
    #[arg(long)]
    pub(crate) sample: bool,
    /// Save the document afterwards, to this file
    #[arg(long, value_name = "OUT.ANICRAFT")]
    pub(crate) save: Option<PathBuf>,
    /// Let --save replace the document that was opened
    #[arg(long, requires = "save")]
    pub(crate) in_place: bool,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
    /// The document, from the arguments. Filled in by [`parse`].
    #[arg(skip)]
    pub(crate) file: Option<PathBuf>,
    /// The script, from the arguments. Filled in by [`parse`].
    #[arg(skip)]
    pub(crate) script: PathBuf,
}

#[derive(Args, Debug)]
pub(crate) struct ExecArgs {
    /// FILE, the command id and its JSON params; with --sample, the id and the params
    #[arg(value_names = ["FILE", "COMMAND", "PARAMS"], num_args = 1..=3, required = true)]
    inputs: Vec<OsString>,
    /// Use the built-in sample instead of a file
    #[arg(long)]
    pub(crate) sample: bool,
    /// Save the document afterwards, to this file
    #[arg(long, value_name = "OUT.ANICRAFT")]
    pub(crate) save: Option<PathBuf>,
    /// Let --save replace the document that was opened
    #[arg(long, requires = "save")]
    pub(crate) in_place: bool,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
    /// The document, from the arguments. Filled in by [`parse`].
    #[arg(skip)]
    pub(crate) file: Option<PathBuf>,
    /// The command id, from the arguments. Filled in by [`parse`].
    #[arg(skip)]
    pub(crate) command: String,
    /// The params, from the arguments. Filled in by [`parse`].
    #[arg(skip)]
    pub(crate) params: Value,
}

#[derive(Args, Debug)]
pub(crate) struct NewArgs {
    /// The document to write
    #[arg(long = "out", value_name = "FILE.ANICRAFT")]
    pub(crate) output: PathBuf,
    /// Stage width in pixels
    #[arg(long, value_name = "W")]
    pub(crate) width: Option<u32>,
    /// Stage height in pixels
    #[arg(long, value_name = "H")]
    pub(crate) height: Option<u32>,
    /// Frames per second
    #[arg(long, value_name = "F", value_parser = finite_number)]
    pub(crate) fps: Option<f32>,
}

#[derive(Args, Debug)]
pub(crate) struct GuideArgs {
    /// The folder to write the pages to (open index.html)
    #[arg(long = "out", value_name = "FOLDER", required = true)]
    pub(crate) output: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) search: Option<GuideCommand>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum GuideCommand {
    /// Search the user guide: chapter, heading, page and a passage for each result, best first
    Search(SearchArgs),
}

#[derive(Args, Debug)]
pub(crate) struct SearchArgs {
    /// What to look for
    #[arg(value_name = "WORDS", required = true, num_args = 1..)]
    pub(crate) words: Vec<String>,
    /// The most results
    #[arg(long, value_name = "N", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub(crate) limit: u32,
    /// Print the results as JSON
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true)]
    pub(crate) json: bool,
}

/// A number of samples for `bench --stroke`, or `yes` for the default.
fn samples(text: &str) -> Result<u32, String> {
    if matches!(text, "yes" | "true") {
        return Ok(crate::bench_stroke::DEFAULT_SAMPLES);
    }
    text.parse().map_err(|_| "a number of samples (or yes)".to_string())
}

#[derive(Args, Debug)]
pub(crate) struct BenchArgs {
    /// Layers of the stress document
    #[arg(long, value_name = "N", conflicts_with_all = ["stroke", "stroke_file"])]
    pub(crate) layers: Option<u32>,
    /// Shapes in each layer
    #[arg(long, value_name = "N", conflicts_with_all = ["stroke", "stroke_file"])]
    pub(crate) shapes: Option<u32>,
    /// Frames of the stress document
    #[arg(long, value_name = "N", conflicts_with_all = ["stroke", "stroke_file"])]
    pub(crate) frames: Option<u32>,
    /// Compare the default scenario with the budgets in docs/performance.md; fails when one is missed
    #[arg(long, num_args = 0..=1, default_missing_value = "yes", default_value = "no", value_parser = BoolishValueParser::new(), value_name = "yes|no", hide_possible_values = true, conflicts_with_all = ["layers", "shapes", "frames", "stroke", "stroke_file"])]
    pub(crate) check: bool,
    /// One long Brush stroke of this many samples: painting it, picking it, moving it and the merge when it is deselected (ADR 0129)
    #[arg(long, value_name = "SAMPLES", num_args = 0..=1, default_missing_value = "yes", value_parser = samples, conflicts_with = "stroke_file")]
    pub(crate) stroke: Option<u32>,
    /// The same steps on a document's own artwork
    #[arg(long, value_name = "FILE.ANICRAFT")]
    pub(crate) stroke_file: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub(crate) struct ServeArgs {
    #[command(flatten)]
    pub(crate) document: OptionalDocument,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
    /// A folder commands may read from (repeatable); needs --automation-write-root
    #[arg(long = "automation-read-root", value_name = "FOLDER", value_parser = non_empty_path, requires = "write_root")]
    read_root: Vec<PathBuf>,
    /// The one folder commands may write to; needs --automation-read-root
    #[arg(long = "automation-write-root", value_name = "FOLDER", value_parser = non_empty_path, requires = "read_root")]
    write_root: Option<PathBuf>,
}

impl ServeArgs {
    /// The roots, when both were given.
    pub(crate) fn roots(&self) -> Option<(&[PathBuf], &Path)> {
        Some((self.read_root.as_slice(), self.write_root.as_deref()?)).filter(|(read, _)| !read.is_empty())
    }
}

#[derive(Args, Debug)]
pub(crate) struct McpArgs {
    #[command(flatten)]
    pub(crate) document: OptionalDocument,
    #[command(flatten)]
    pub(crate) plugins: Plugins,
    /// Act live on the AniCraft window listening on this port (or host:port), not on a file
    #[arg(long, value_name = "PORT", conflicts_with_all = ["file", "sample"])]
    pub(crate) bridge: Option<String>,
    /// A folder the assistant's commands may read from (repeatable)
    #[arg(long = "automation-read-root", value_name = "FOLDER", value_parser = non_empty_path, required = true)]
    pub(crate) read_root: Vec<PathBuf>,
    /// The one folder the assistant's commands may write to (it may be a read root too)
    #[arg(long = "automation-write-root", value_name = "FOLDER", value_parser = non_empty_path, required = true)]
    pub(crate) write_root: PathBuf,
}

/// A path that is not empty.
fn non_empty_path(text: &str) -> Result<PathBuf, String> {
    if text.trim().is_empty() { Err("needs a folder".into()) } else { Ok(PathBuf::from(text)) }
}

fn frame_number(text: &str) -> Result<u32, String> {
    match text.parse::<u32>() {
        Ok(0) => Err("frames count from 1; the first frame is 1".into()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("a whole number from 1, not '{text}'")),
    }
}

/// A finite number: `NaN` and `inf` parse as floats but mean nothing here.
fn finite_number(text: &str) -> Result<f32, String> {
    let value: f32 = text.parse().map_err(|_| format!("a number, not '{text}'"))?;
    if value.is_finite() { Ok(value) } else { Err("a finite number".into()) }
}

fn json_text(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))
}

/// For `Command::mut_args`: an empty or blank variable counts as unset (RFC-0018 rule 1.7),
/// where clap would pass the empty value to the parser.
fn blank_env_is_unset(arg: Arg) -> Arg {
    let blank = arg.get_env().and_then(std::env::var_os).is_some_and(|v| v.to_str().is_some_and(|v| v.trim().is_empty()));
    if blank { arg.env(None::<&str>) } else { arg }
}

fn blank_envs(command: clap::Command) -> clap::Command {
    command.mut_args(blank_env_is_unset).mut_subcommands(blank_envs)
}

/// The clap command: what `--help`, the completions and the man pages are made of.
pub(crate) fn command() -> clap::Command {
    blank_envs(Cli::command())
}

/// A usage error in the way clap prints one, under the usage of `subcommand`.
fn usage_error(subcommand: &str, message: impl std::fmt::Display) -> clap::Error {
    let mut command = command();
    command.build();
    let mut command = command.find_subcommand_mut(subcommand).map_or_else(command_fallback, std::mem::take);
    command.error(ErrorKind::ArgumentConflict, message)
}

fn command_fallback() -> clap::Command {
    command()
}

/// Splits the positional arguments of `run` and `exec` into the document and the rest: with
/// `--sample`, none of them is the document.
fn split(subcommand: &str, sample: bool, inputs: &[OsString], names: &[&str]) -> Result<(Option<PathBuf>, Vec<OsString>), clap::Error> {
    let (file, rest) = if sample { (None, inputs) } else { (inputs.first().map(PathBuf::from), inputs.get(1..).unwrap_or_default()) };
    // The names in `[]` are optional, the others are required.
    let required = names.iter().filter(|n| !n.starts_with('[')).count();
    if rest.len() < required || rest.len() > names.len() {
        let shape = if sample { format!("--sample {}", names.join(" ")) } else { format!("<FILE> {}", names.join(" ")) };
        return Err(usage_error(subcommand, format!("expected {shape}")));
    }
    Ok((file, rest.to_vec()))
}

/// Parses the arguments (without the program name). Help and the version come back as errors too,
/// which print to stdout and exit 0.
pub(crate) fn parse(args: &[OsString]) -> Result<Cli, clap::Error> {
    let matches = command().try_get_matches_from(std::iter::once(OsString::from("anicraft-cli")).chain(args.iter().cloned()))?;
    let mut cli = Cli::from_arg_matches(&matches)?;
    match &mut cli.command {
        Command::Run(run) => {
            let (file, rest) = split("run", run.sample, &run.inputs, &["SCRIPT"])?;
            run.file = file;
            run.script = rest.first().map(PathBuf::from).unwrap_or_default();
        }
        Command::Exec(exec) => {
            let (file, rest) = split("exec", exec.sample, &exec.inputs, &["COMMAND", "[PARAMS]"])?;
            exec.file = file;
            let text =
                |s: &OsString| s.to_str().map(str::to_owned).ok_or_else(|| usage_error("exec", "the command id and params must be valid Unicode"));
            exec.command = rest.first().map(text).transpose()?.unwrap_or_default();
            exec.params = match rest.get(1) {
                Some(t) => serde_json::from_str(&text(t)?).map_err(|e| usage_error("exec", format!("the params are not JSON: {e}")))?,
                None => serde_json::json!({}),
            };
        }
        Command::Mcp(mcp) => {
            if let Some(target) = &mcp.bridge {
                // The variable alone doesn't conflict: only the flag says this run wants plug-ins.
                let sub = matches.subcommand_matches("mcp");
                if sub.and_then(|m| m.value_source("plugins")) == Some(clap::parser::ValueSource::CommandLine) {
                    return Err(usage_error("mcp", "--bridge acts on the open AniCraft window: it takes no --plugins"));
                }
                #[cfg(not(target_arch = "wasm32"))]
                crate::control::parse_bridge_target(target).map_err(|reason| usage_error("mcp", format!("--bridge {target}: {reason}")))?;
                #[cfg(target_arch = "wasm32")]
                let _ = target;
            }
        }
        _ => {}
    }
    Ok(cli)
}
