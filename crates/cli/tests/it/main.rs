//! Integration tests, built as one test binary: each file is a module. They call `run` in-process,
//! except where a test needs the real binary (a protocol on stdio).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod cli;
mod commands;
mod man_and_completions;
mod protocol;

use std::ffi::OsString;
use std::path::PathBuf;

/// What `run` returned and wrote.
pub struct Outcome {
    pub code: i32,
    pub out: String,
    pub err: String,
}

pub fn invoke<I, T>(args: I) -> Outcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = anicraft_cli::run(args, &mut out, &mut err);
    Outcome { code, out: String::from_utf8_lossy(&out).into_owned(), err: String::from_utf8_lossy(&err).into_owned() }
}

/// A new, empty folder for one test.
pub fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anicraft-cli-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
