//! RFC-0018 rule 2.7: help, versions, usage errors with exit 2 on stderr, and nothing written
//! before the arguments are checked.

use crate::{invoke, workdir};

const SUBCOMMANDS: [&str; 16] = [
    "info",
    "fla-dump",
    "render",
    "render-seq",
    "gif",
    "apng",
    "webp",
    "export",
    "formats",
    "commands",
    "run",
    "exec",
    "new",
    "guide",
    "bench",
    "serve",
];

fn usage_error(args: &[&str]) -> String {
    let got = invoke(args.iter().copied());
    assert_eq!(got.code, 2, "{args:?}: {}", got.err);
    assert_eq!(got.out, "", "{args:?} wrote to stdout");
    assert!(got.err.starts_with("error: ") || got.err.contains("Usage:"), "{args:?}: {}", got.err);
    got.err
}

#[test]
fn help_and_the_version_go_to_stdout_with_exit_0() {
    for args in [&["--help"][..], &["-h"], &["help"], &["--version"], &["-V"]] {
        let got = invoke(args.iter().copied());
        assert_eq!((got.code, got.err.as_str()), (0, ""), "{args:?}");
        assert!(got.out.contains("anicraft-cli"), "{args:?}: {}", got.out);
    }
    let help = invoke(["--help"]).out;
    assert!(help.contains("Exit status:") && help.contains("128 + the signal number"), "{help}");
    assert!(help.contains("1-based") && help.contains("0-based"), "the frame numbering is explained: {help}");
    for sub in SUBCOMMANDS.iter().chain(&["mcp"]) {
        for flag in ["--help", "-h"] {
            let got = invoke([sub, &flag]);
            assert_eq!((got.code, got.err.as_str()), (0, ""), "{sub} {flag}");
            assert!(got.out.contains(&format!("Usage: anicraft-cli {sub}")), "{sub}: {}", got.out);
        }
        assert_eq!(invoke(["help", sub]).out, invoke([sub, "--help"]).out, "{sub}");
    }
}

#[test]
fn every_subcommand_takes_the_version_and_there_is_no_version_subcommand() {
    let want = env!("CARGO_PKG_VERSION");
    for sub in SUBCOMMANDS.iter().chain(&["mcp"]) {
        for flag in ["--version", "-V"] {
            let got = invoke([sub, &flag]);
            assert_eq!((got.code, got.out.as_str(), got.err.as_str()), (0, format!("anicraft-cli-{sub} {want}\n").as_str(), ""), "{sub} {flag}");
        }
    }
    assert_eq!(invoke(["--version"]).out, format!("anicraft-cli {want}\n"));
    usage_error(&["version"]);
}

/// With no arguments the help goes to stderr and the exit code is 2.
#[test]
fn no_arguments_is_a_usage_error() {
    let got = invoke(Vec::<&str>::new());
    assert_eq!((got.code, got.out.as_str()), (2, ""));
    assert!(got.err.contains("Usage: anicraft-cli"), "{}", got.err);
}

#[test]
fn an_unknown_subcommand_or_flag_is_exit_2_on_every_subcommand() {
    usage_error(&["nope"]);
    usage_error(&["--no-such-flag"]);
    usage_error(&["nope", "--help-me"]);
    for sub in &SUBCOMMANDS {
        let err = usage_error(&[sub, "--no-such-option", "1"]);
        assert!(err.contains("--no-such-option"), "{sub}: {err}");
    }
    // The typo that used to render frame 1 as if it had been asked for.
    let dir = workdir("typo");
    let png = dir.join("f.png");
    let err = usage_error(&["render", "--sample", "--out", png.to_str().unwrap(), "--frmae", "12"]);
    assert!(err.contains("--frmae") && err.contains("--frame"), "clap suggests the flag: {err}");
    assert!(!png.exists(), "nothing was written");
}

#[test]
fn repeated_flags_bad_values_and_conflicts_are_usage_errors() {
    let dir = workdir("values");
    let png = dir.join("f.png");
    let out = png.to_str().unwrap();
    for args in [
        vec!["render", "--sample", "--out", out, "--frame", "1", "--frame", "2"],
        vec!["render", "--sample", "--out", out, "--frame", "0"],
        vec!["render", "--sample", "--out", out, "--frame", "-1"],
        vec!["render", "--sample", "--out", out, "--frame", "x"],
        vec!["render", "--sample", "--out", out, "--scale", "NaN"],
        vec!["render", "--sample", "--out", out, "--scale", "inf"],
        vec!["render", "--sample", "--out", out, "--scale", "1e40"],
        vec!["render", "--sample", "--out", out, "--scale"],
        vec!["render", "--sample", "--out"],
        vec!["render", "--sample", "--out", "--frame", "2"],
        vec!["render", "--sample", "nowhere.anicraft", "--out", out],
        vec!["render", "--out", out],
        vec!["render", "--sample"],
        vec!["render-seq", "--sample", "--out-dir", out, "--from", "0"],
        vec!["gif", "--sample", "--out", out, "--overwrite", "maybe"],
        vec!["apng", "--sample", "--out", out, "--loop", "twice"],
        vec!["apng", "--sample", "--out", out, "--loop", "-1"],
        vec!["new", "--out", out, "--width", "wide"],
        vec!["new", "--out", out, "--fps", "NaN"],
        vec!["new"],
        vec!["export", "--sample", "--format", "svg", "--out", out, "--options", "{not json"],
        vec!["export", "--sample", "--format", "png", "--out", out, "--frame", "1", "--from", "1"],
        vec!["exec", "--sample", "edit.select_all", "{not json"],
        vec!["exec", "--sample"],
        vec!["exec"],
        vec!["run", "--sample"],
        vec!["run", "--sample", "a.json", "b.json"],
        vec!["run", "--sample", "a.json", "--in-place"],
        vec!["guide"],
        vec!["guide", "search"],
        vec!["guide", "search", "pen", "--limit", "x"],
        vec!["guide", "search", "pen", "--limit", "0"],
        vec!["guide", "search", "pen", "--limit", "101"],
        vec!["guide", "--out", out, "--json", "yes"],
        vec!["bench", "--layers", "many"],
        vec!["bench", "--stroke", "many"],
        vec!["bench", "--check", "--layers", "3"],
        vec!["bench", "--stroke", "5", "--stroke-file", "f.anicraft"],
        vec!["info"],
        vec!["info", "--sample", "--plugins"],
        vec!["info", "--sample", "--plugins", ""],
        vec!["formats", "--sample"],
        vec!["commands", "--json"],
    ] {
        usage_error(&args);
    }
    assert!(!png.exists(), "nothing was written");
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing was created");
}

/// A value that starts with `-` is written `--out=-image.png`, and a negative number is a value.
#[test]
fn a_dash_value_needs_the_equals_form() {
    let dir = workdir("dash");
    let render = |out: &str| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_anicraft-cli"));
        command.current_dir(&dir).args(["render", "--sample", out, "--frame=1", "--scale=0.1"]).output().unwrap()
    };
    let got = render("--out=-image.png");
    assert_eq!(got.status.code(), Some(0), "{}", String::from_utf8_lossy(&got.stderr));
    assert!(dir.join("-image.png").exists());
    usage_error(&["render", "--sample", "--out", "-image.png"]);
}

#[test]
fn a_blank_plugins_variable_counts_as_unset() {
    // The environment is shared by the tests of this binary: the variable is set only around one call.
    let dir = workdir("env");
    let out = dir.join("a.png");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_anicraft-cli"));
    let got = command
        .args(["render", "--sample", "--frame", "1", "--scale", "0.1", "--out", out.to_str().unwrap()])
        .env("ANICRAFT_PLUGINS", "  ")
        .output()
        .unwrap();
    assert_eq!((got.status.code(), String::from_utf8_lossy(&got.stderr).as_ref()), (Some(0), ""));
    assert!(out.exists());
}

/// No environment variable sets a destructive flag, and none of the flags that overwrite declares one.
#[test]
fn no_destructive_flag_reads_an_environment_variable() {
    fn walk(command: &clap::Command, path: &str, found: &mut Vec<String>) {
        for arg in command.get_arguments() {
            let destructive = matches!(arg.get_id().as_str(), "overwrite" | "in_place" | "force" | "delete");
            if destructive && arg.get_env().is_some() {
                found.push(format!("{path} --{}", arg.get_id()));
            }
        }
        for sub in command.get_subcommands() {
            walk(sub, &format!("{path} {}", sub.get_name()), found);
        }
    }
    let mut found = Vec::new();
    walk(&anicraft_cli::command(), "anicraft-cli", &mut found);
    assert!(found.is_empty(), "{found:?}");
    // `--plugins` is the one flag with a variable, and `--help` lists it.
    assert!(invoke(["info", "--help"]).out.contains("ANICRAFT_PLUGINS"));
}

/// RFC-0018 rule 1.6: an output never replaces its input unless `--in-place` asks.
#[test]
fn an_output_never_replaces_its_input_unless_in_place_is_given() {
    let dir = workdir("replace");
    let doc = dir.join("a.anicraft");
    let d = doc.to_str().unwrap();
    assert_eq!(invoke(["new", "--out", d]).code, 0);
    let before = std::fs::read(&doc).unwrap();
    for args in [
        vec!["render", d, "--out", d],
        vec!["render-seq", d, "--out-dir", d],
        vec!["gif", d, "--out", d, "--overwrite"],
        vec!["apng", d, "--out", d, "--overwrite"],
        vec!["webp", d, "--out", d, "--overwrite"],
        vec!["export", d, "--format", "svg", "--out", d, "--overwrite"],
    ] {
        let got = invoke(args.iter().copied());
        assert_ne!(got.code, 0, "{args:?}");
    }
    let script = dir.join("s.json");
    std::fs::write(&script, r#"[{"cmd": "edit.select_all"}]"#).unwrap();
    let s = script.to_str().unwrap();
    // `run` and `exec` save onto the opened document only with --in-place.
    let got = invoke(["run", d, s, "--save", d]);
    assert_eq!(got.code, 2, "{}", got.err);
    assert!(got.err.contains("--in-place") && got.err.contains("replace"), "{}", got.err);
    let got = invoke(["exec", d, "edit.select_all", "--save", d]);
    assert_eq!(got.code, 2, "{}", got.err);
    // --in-place with nothing to replace has nothing to allow.
    let elsewhere = dir.join("b.anicraft");
    let got = invoke(["run", d, s, "--save", elsewhere.to_str().unwrap(), "--in-place"]);
    assert_eq!(got.code, 2, "{}", got.err);
    assert!(!elsewhere.exists());
    assert_eq!(std::fs::read(&doc).unwrap(), before, "nothing replaced the document");
    // With --in-place it is saved.
    let got = invoke(["run", d, s, "--save", d, "--in-place"]);
    assert_eq!(got.code, 0, "{}", got.err);
    let got = invoke(["exec", d, "edit.select_all", "--save", d, "--in-place"]);
    assert_eq!(got.code, 0, "{}", got.err);
    // A different file is the ordinary case.
    let got = invoke(["run", d, s, "--save", elsewhere.to_str().unwrap()]);
    assert_eq!(got.code, 0, "{}", got.err);
    assert!(elsewhere.exists());
}

/// A path that is not valid Unicode reaches the command: a document is read from it, and the
/// engine's JSON-valued commands say so by name. (macOS refuses such a file name, so it runs elsewhere.)
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn a_path_that_is_not_unicode_is_a_path_not_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let dir = workdir("nonunicode");
    let bad = dir.join(OsString::from_vec(b"caf\xe9.anicraft".to_vec()));
    let made = invoke([OsString::from("new"), "--out".into(), dir.join("ok.anicraft").into()]);
    assert_eq!(made.code, 0, "{}", made.err);
    std::fs::copy(dir.join("ok.anicraft"), &bad).unwrap();
    // Reading a document by that name works: it is a PathBuf all the way to the loader.
    let got = invoke([OsString::from("info"), bad.clone().into()]);
    assert_eq!(got.code, 0, "{}", got.err);
    // Writing goes through a JSON command, which holds only text: refused by name, exit 1.
    let got = invoke([OsString::from("render"), "--sample".into(), "--out".into(), bad.into()]);
    assert_eq!((got.code, got.out.as_str()), (1, ""), "{}", got.err);
    assert!(got.err.contains("is not valid Unicode"), "{}", got.err);
}

/// `--screenshot` is the desktop app's: this binary hands the whole command line to the `anicraft`
/// next to it, and ends with the code it ended with, 128 + the signal when a signal ended it.
#[cfg(unix)]
mod desktop {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use crate::workdir;

    /// Runs the command, again when the system says the file is busy: another test's child process
    /// may still hold the copy this one just wrote open between its fork and its exec.
    fn output(command: &mut Command) -> std::process::Output {
        for _ in 0..50 {
            match command.output() {
                Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(std::time::Duration::from_millis(100)),
                other => return other.unwrap(),
            }
        }
        command.output().unwrap()
    }

    /// A copy of the binary in a folder of its own, with `anicraft` beside it when `app` is given.
    fn installed(name: &str, app: Option<&str>) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = workdir(name);
        let cli = dir.join("anicraft-cli");
        std::fs::copy(env!("CARGO_BIN_EXE_anicraft-cli"), &cli).unwrap();
        if let Some(script) = app {
            let app = dir.join("anicraft");
            std::fs::write(&app, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (dir, cli)
    }

    #[test]
    fn without_the_desktop_app_it_says_so_with_exit_1() {
        let (_dir, cli) = installed("no-desktop", None);
        let got = output(Command::new(cli).args(["--screenshot", "x.png"]));
        assert_eq!(got.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&got.stderr).contains("needs the AniCraft desktop app"), "{}", String::from_utf8_lossy(&got.stderr));
    }

    #[test]
    fn the_desktop_apps_exit_code_is_passed_on() {
        let (dir, cli) = installed("desktop-code", Some("echo \"$@\" > args.txt; exit 3"));
        let got = output(Command::new(cli).current_dir(&dir).args(["--screenshot", "x.png", "--look", "dark"]));
        assert_eq!(got.status.code(), Some(3));
        assert_eq!(std::fs::read_to_string(dir.join("args.txt")).unwrap().trim(), "--screenshot x.png --look dark");
    }

    #[test]
    fn a_signal_that_ended_the_desktop_app_is_128_plus_the_signal() {
        let (_dir, cli) = installed("desktop-signal", Some("kill -TERM $$"));
        let got = output(Command::new(cli).args(["--screenshot", "x.png"]));
        assert_eq!(got.status.code(), Some(128 + 15));
    }
}
