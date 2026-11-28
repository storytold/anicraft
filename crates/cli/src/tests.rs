//! Unit tests of what the integration tests (`tests/it`) can't reach: the in-process helpers,
//! the serve loop, the budgets and the plug-in and export paths.

use super::*;
use anicraft_engine::EngineError;

/// Runs the CLI in-process: its exit code, stdout and stderr.
fn cli<S: AsRef<str>>(args: &[S]) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(args.iter().map(|a| OsString::from(a.as_ref())), &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// Stdout of a run that must succeed.
fn ok<S: AsRef<str>>(args: &[S]) -> String {
    let (code, out, err) = cli(args);
    assert_eq!(code, 0, "{}: {err}", args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" "));
    out.trim_end().to_string()
}

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
    let e = exec(&mut s, "file.export_video", &json!({"path": "a.mp4", "ffmpeg": "/nonexistent/anicraft-ffmpeg"})).unwrap_err().to_string();
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
    let zoomed =
        anicraft_render::View { transform: anicraft_geom::Affine::translate(-2880.0, -1620.0).mul(&anicraft_geom::Affine::scale(4.0, 4.0)), ..view };
    let (_, st) = anicraft_render::render(&dl, &zoomed).unwrap();
    assert!(st.items >= 1000 && st.drawn * 8 <= st.items, "{} of {} shapes drawn at 400%", st.drawn, st.items);
}

#[test]
fn export_prefers_out_over_a_destination_in_options() {
    use anicraft_engine::vfs;

    vfs::use_memory();
    let export = |format: &str, out: &str, options: &str| {
        let argv = ["export", "--sample", "--format", format, "--out", out, "--options", options].map(str::to_string);
        serde_json::from_str::<Value>(&ok(&argv)).unwrap()
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
fn budget_check_reads_bench_output() {
    let mut out = json!({"edit_command_ms": 0.01, "undo_ms": 0.01, "snapshot_clone_us": 0.4, "evaluate_ms_per_frame": 1.5,
        "render_1080p_ms_per_frame": 20.0, "render_zoom400_ms_per_frame": 10.0, "hit_test_us": 50.0, "file": {"save_ms": 100.0}, "autosave": {"ui_us": 5.0, "background_ms": 300.0}});
    let (rows, missed) = check_budgets(&out);
    assert_eq!(rows.len(), TIME_BUDGETS.len());
    assert_eq!(missed, ["render 1080p (ms): 20 (budget 16.7)", "load the 65 MB file (ms): not measured"]);
    out["render_1080p_ms_per_frame"] = json!(16.0);
    out["file"]["load_ms"] = json!(120.0);
    assert!(check_budgets(&out).1.is_empty());
}

/// RFC-0018 rule 1.2: the exit code comes from the error's variant, not from its message.
#[test]
fn exit_codes_come_from_the_variant() {
    let usage = [
        CliError::NightlyProtocol { argument: "--launch-profile".into() },
        CliError::Roots("not a folder".into()),
        CliError::RootPath { flag: "read", path: PathBuf::from("x"), source: std::io::Error::other("e") },
        CliError::OptionsNotObject,
        CliError::NoSearchTerms,
        CliError::CheckNeedsRelease,
        CliError::NoSockets,
        CliError::BridgeTarget { target: "x".into(), reason: "r".into() },
        CliError::FlagNotUsed { flag: "--frame", format: "gif".into() },
        CliError::WouldReplace { output: "--out", path: PathBuf::from("x"), in_place_helps: false },
        CliError::InPlaceNotUsed,
    ];
    for e in &usage {
        assert_eq!(e.exit_code(), 2, "{e}");
    }
    // A message that looks like a usage error is still a failure when it comes from a command.
    let from_a_command = CliError::Command { id: "file.save".into(), source: EngineError::Param("--automation-read-root needs a folder".into()) };
    assert_eq!(from_a_command.exit_code(), 1, "{from_a_command}");
    let failures =
        [CliError::NoDesktopApp, CliError::Bridge("no app".into()), CliError::OverBudget("render".into()), CliError::NotUnicode(PathBuf::from("x"))];
    for e in &failures {
        assert_eq!(e.exit_code(), 1, "{e}");
    }
}
