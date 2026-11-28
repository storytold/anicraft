//! Why a subcommand failed. The exit code comes from the variant, never from the message text
//! (RFC-0018 rules 1.2 and 2.4): a mistake in how the program was started is exit 2, a command
//! that ran and failed is exit 1. clap's own errors are exit 2 and are printed by clap.

use std::path::PathBuf;

use anicraft_engine::EngineError;

/// The message of a refused Nightly launch: what the Nightly window's launcher passes, which this
/// headless tool has no use for.
pub(crate) const NIGHTLY_PROTOCOL: &str = "--launch-profile and --profile-root belong to the AniCraft Nightly window";

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    // ----- exit 2: the command line is wrong in a way only the run can tell -----
    #[error(
        "{NIGHTLY_PROTOCOL} (`{argument}`); anicraft-cli is the stable headless front end. Run `anicraft --launch-profile nightly-v1 --profile-root <folder> --` instead."
    )]
    NightlyProtocol { argument: String },
    /// An automation root that is not a folder that exists, or that cannot be resolved.
    #[error("{0}")]
    Roots(String),
    #[error("--automation-{flag}-root {}: {source}", path.display())]
    RootPath { flag: &'static str, path: PathBuf, source: std::io::Error },
    #[error("--options must be a JSON object")]
    OptionsNotObject,
    #[error("guide search needs words to look for, e.g. anicraft-cli guide search onion skin")]
    NoSearchTerms,
    #[error("bench --check needs a release build: cargo run --release -p anicraft-cli -- bench --check yes")]
    CheckNeedsRelease,
    #[error("mcp --bridge needs sockets, which this build doesn't have")]
    NoSockets,
    #[error("--bridge {target}: {reason}")]
    BridgeTarget { target: String, reason: String },
    #[error("{output} {} is the document it works on, which it would replace: choose another file{}", path.display(), if *in_place_helps { ", or pass --in-place" } else { "" })]
    WouldReplace { output: &'static str, path: PathBuf, in_place_helps: bool },
    #[error("{flag} is not used by the {format} format")]
    FlagNotUsed { flag: &'static str, format: String },
    #[error("--in-place is for a --save onto the opened file, and this --save is to another file or there is no file")]
    InPlaceNotUsed,

    // ----- exit 1: it ran and failed -----
    /// The engine's commands take paths as JSON, which holds only text.
    #[error("{}: the path is not valid Unicode", .0.display())]
    NotUnicode(PathBuf),
    #[error("{id}: {source}")]
    Command { id: String, source: EngineError },
    #[error("step {step}: {source}")]
    Step { step: usize, source: Box<CliError> },
    #[error("step {step}: missing \"cmd\"")]
    MissingCmd { step: usize },
    #[error("{}: {source}", path.display())]
    Open { path: PathBuf, source: EngineError },
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{}: expected a JSON array of {{cmd, params}} or a saved command: {source}", path.display())]
    ScriptSyntax { path: PathBuf, source: serde_json::Error },
    #[error("{}: expected a JSON array of {{cmd, params}}: {source}", path.display())]
    ScriptShape { path: PathBuf, source: serde_json::Error },
    #[error("{}: {source}", path.display())]
    Fla { path: PathBuf, source: anicraft_fla::cfb::Error },
    #[error("{}: not a legacy Flash document (its first bytes are not the compound binary signature)", path.display())]
    NotLegacyFla { path: PathBuf },
    #[error("no export format '{id}' (formats: {known}; exporter plug-ins load with --plugins <folder>)")]
    UnknownFormat { id: String, known: String },
    #[error("{0}")]
    Bridge(String),
    #[error("{0}")]
    Stdio(std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("over budget: {0}")]
    OverBudget(String),
    #[error("--screenshot needs the AniCraft desktop app (`anicraft`) next to anicraft-cli; build it with `cargo build`")]
    NoDesktopApp,
    #[error("could not start {}: {source}", path.display())]
    StartDesktop { path: PathBuf, source: std::io::Error },
}

impl CliError {
    /// RFC-0018 rule 1.2: 2 when the program was started wrongly, else 1.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::NightlyProtocol { .. }
            | CliError::Roots(_)
            | CliError::RootPath { .. }
            | CliError::OptionsNotObject
            | CliError::NoSearchTerms
            | CliError::CheckNeedsRelease
            | CliError::NoSockets
            | CliError::BridgeTarget { .. }
            | CliError::FlagNotUsed { .. }
            | CliError::WouldReplace { .. }
            | CliError::InPlaceNotUsed => 2,
            _ => 1,
        }
    }
}
