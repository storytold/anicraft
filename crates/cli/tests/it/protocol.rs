//! `serve` and `mcp` speak a protocol on stdio: nothing else is written to stdout, and `mcp`
//! refuses to start without its roots (AGENTS.md rule 18).

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::{invoke, workdir};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_anicraft-cli"))
}

/// Feeds `input` to the binary and returns its exit code, stdout and stderr.
fn talk(args: &[&str], input: &str) -> (i32, String, String) {
    let mut child = binary().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let got = child.wait_with_output().unwrap();
    (got.status.code().unwrap(), String::from_utf8(got.stdout).unwrap(), String::from_utf8(got.stderr).unwrap())
}

#[test]
fn serve_writes_only_json_lines_to_stdout() {
    let (code, out, err) = talk(&["serve", "--sample"], "{\"id\":1,\"cmd\":\"edit.select_all\"}\n{\"cmd\":\"quit\"}\n");
    assert_eq!((code, err.as_str()), (0, ""));
    let lines: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["ready"], true);
    assert_eq!(lines[1]["ok"], true);
    assert_eq!(lines.last().unwrap()["bye"], true);
}

#[test]
fn serve_ends_quietly_when_stdin_closes() {
    let (code, out, err) = talk(&["serve"], "");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out.lines().count(), 1, "only the ready line: {out}");
}

#[test]
fn mcp_answers_on_stdout_inside_its_roots() {
    let dir = workdir("mcp");
    let root = dir.to_str().unwrap();
    let init = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"t\",\"version\":\"1\"}}}\n";
    let (code, out, err) = talk(&["mcp", "--sample", "--automation-read-root", root, &format!("--automation-write-root={root}")], init);
    assert_eq!((code, err.as_str()), (0, ""));
    let reply: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(reply["id"], 1, "{out}");
    assert!(out.lines().all(|l| serde_json::from_str::<Value>(l).is_ok()), "stdout holds JSON-RPC only: {out}");
}

/// AGENTS.md rule 18: `mcp` refuses to start without both roots, bridge or not, with the usage
/// exit code, before it opens anything; a root that is not a folder stops the start too.
#[test]
fn mcp_refuses_to_start_without_both_automation_roots() {
    let dir = workdir("mcp-roots");
    let (work, file) = (dir.to_str().unwrap().to_owned(), dir.join("f.json"));
    std::fs::write(&file, "{}").unwrap();
    let file = file.to_str().unwrap();
    for argv in [
        vec!["mcp"],
        vec!["mcp", "--sample"],
        vec!["mcp", "no-such-file.anicraft"],
        vec!["mcp", "--bridge", "9878"],
        vec!["mcp", "--automation-read-root", &work],
        vec!["mcp", "--automation-write-root", &work],
        vec!["mcp", "--bridge", "9878", "--automation-write-root", &work],
        vec!["mcp", "--automation-read-root", &work, "--automation-write-root", &work, "--automation-write-root", &work],
        vec!["mcp", "--automation-read-root", "", "--automation-write-root", &work],
        vec!["mcp", "--automation-read-root", &work, "--automation-write-root"],
        vec!["mcp", "--automation-read-root=", &format!("--automation-write-root={work}")],
        vec!["serve", "--automation-read-root", &work],
        vec!["serve", "--automation-write-root", &work],
    ] {
        let got = invoke(argv.iter().copied());
        assert_eq!((got.code, got.out.as_str()), (2, ""), "{argv:?}: {}", got.err);
        assert!(got.err.contains("--automation-"), "{argv:?}: {}", got.err);
    }
    let missing = dir.join("nowhere");
    let got = invoke(["mcp", "--automation-read-root", missing.to_str().unwrap(), "--automation-write-root", &work]);
    assert!(got.code == 2 && got.err.starts_with("error: ") && got.err.contains("nowhere"), "{}", got.err);
    let got = invoke(["serve", "--automation-read-root", &work, "--automation-write-root", file]);
    assert_eq!(got.code, 2, "{}", got.err);
    // The help says so.
    let help = invoke(["mcp", "--help"]).out;
    assert!(help.contains("--automation-read-root") && help.contains("--automation-write-root") && help.contains("required"), "{help}");
}

/// `--bridge` takes a port; a bad one is a usage error before anything connects.
#[test]
fn a_bad_bridge_port_is_a_usage_error() {
    let dir = workdir("bridge");
    let root = dir.to_str().unwrap();
    for target in ["nope", "0", "70000", ""] {
        let got = invoke(["mcp", "--bridge", target, "--automation-read-root", root, "--automation-write-root", root]);
        assert_eq!((got.code, got.out.as_str()), (2, ""), "{target:?}: {}", got.err);
    }
    // A bridge acts on a window: no file, no plug-ins.
    let got = invoke(["mcp", "--bridge", "9878", "--sample", "--automation-read-root", root, "--automation-write-root", root]);
    assert_eq!(got.code, 2, "{}", got.err);
    let got = invoke(["mcp", "--bridge", "9878", "--plugins", root, "--automation-read-root", root, "--automation-write-root", root]);
    assert_eq!(got.code, 2, "{}", got.err);
}

/// The Nightly window's launch arguments belong to the desktop app.
#[test]
fn the_nightly_launch_protocol_is_refused() {
    for argv in [vec!["--launch-profile", "nightly-v1", "info", "--sample"], vec!["info", "--sample", "--profile-root=/x"]] {
        let got = invoke(argv.iter().copied());
        assert_eq!((got.code, got.out.as_str()), (2, ""), "{argv:?}");
        assert!(got.err.contains("belong to the AniCraft Nightly window"), "{}", got.err);
    }
}

/// `serve` with roots keeps the commands inside them, and says so in the reply.
#[test]
fn serve_with_roots_refuses_a_write_outside_them() {
    let dir = workdir("serve-roots");
    let (work, out) = (dir.join("work"), dir.join("out"));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let request = |path: &std::path::Path| {
        format!("{{\"id\":1,\"cmd\":\"file.save_as\",\"params\":{{\"path\":{}}}}}\n", serde_json::json!(path.to_str().unwrap()))
    };
    let input = format!("{}{}{{\"cmd\":\"quit\"}}\n", request(&work.join("no.anicraft")), request(&out.join("yes.anicraft")));
    let (code, stdout, err) =
        talk(&["serve", "--sample", "--automation-read-root", work.to_str().unwrap(), "--automation-write-root", out.to_str().unwrap()], &input);
    assert_eq!((code, err.as_str()), (0, ""));
    let lines: Vec<Value> = stdout.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[1]["ok"], false, "{stdout}");
    assert!(lines[1]["error"].as_str().unwrap().contains("automation write root"), "{stdout}");
    assert_eq!(lines[2]["ok"], true, "{stdout}");
    assert!(out.join("yes.anicraft").exists() && !work.join("no.anicraft").exists());
}

/// `mcp --bridge` starts without the app and answers on stdout; a call says how to start it.
#[test]
fn the_bridge_answers_without_the_app() {
    let dir = workdir("bridge-down");
    let root = dir.to_str().unwrap();
    // A port nothing listens on: bound, then released.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
    let init = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"t\",\"version\":\"1\"}}}\n";
    let (code, out, err) = talk(&["mcp", "--bridge", &port, "--automation-read-root", root, "--automation-write-root", root], init);
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let reply: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(reply["id"], 1, "{out}");
}
