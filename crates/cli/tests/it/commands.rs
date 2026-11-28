//! What each subcommand does, through `run`: the behaviour the command line had before RFC-0018,
//! kept.

use serde_json::{Value, json};

use crate::{invoke, workdir};

fn ok(args: &[&str]) -> String {
    let got = invoke(args.iter().copied());
    assert_eq!((got.code, got.err.as_str()), (0, ""), "{args:?}");
    got.out.trim_end().to_owned()
}

fn failed(args: &[&str]) -> String {
    let got = invoke(args.iter().copied());
    assert_eq!((got.code, got.out.as_str()), (1, ""), "{args:?}: {}", got.err);
    assert!(got.err.starts_with("error: "), "{}", got.err);
    got.err
}

#[test]
fn commands_lists_every_command_as_json() {
    let list: Value = serde_json::from_str(&ok(&["commands"])).unwrap();
    assert!(list.as_array().unwrap().iter().any(|c| c["id"] == "timeline.insert_keyframe"));
}

#[test]
fn info_reads_the_sample_and_a_file_and_an_animate_document() {
    let info: Value = serde_json::from_str(&ok(&["info", "--sample"])).unwrap();
    assert!(info["layers"].is_array() || info.get("layers").is_some() || info.is_object(), "{info}");
    // #226: an Animate document is imported as File › Open does, untitled.
    let xfl = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../xfl/tests/corpus/shapes/shapes.xfl");
    assert!(ok(&["info", xfl.to_str().unwrap()]).contains("\"layers\""));
    let dir = workdir("info");
    let missing = dir.join("missing.anicraft");
    let err = failed(&["info", missing.to_str().unwrap()]);
    assert!(err.contains("missing.anicraft"), "{err}");
}

/// Frame flags on the command line count from 1, the engine's frames from 0.
#[test]
fn frame_flags_count_from_one_and_json_frames_from_zero() {
    let dir = workdir("frames");
    let (cli, exec) = (dir.join("cli.png"), dir.join("exec.png"));
    let by_cli: Value = serde_json::from_str(&ok(&["render", "--sample", "--frame", "1", "--out", cli.to_str().unwrap()])).unwrap();
    let params = json!({"path": exec.to_str().unwrap(), "frame": 0}).to_string();
    let by_exec: Value = serde_json::from_str(&ok(&["exec", "--sample", "file.export_png", &params])).unwrap();
    assert_eq!((&by_cli["frame"], &by_exec["frame"]), (&json!(0), &json!(0)));
    assert_eq!(std::fs::read(&cli).unwrap(), std::fs::read(&exec).unwrap());
}

#[test]
fn render_and_render_seq_write_pngs() {
    let dir = workdir("png");
    let png = dir.join("f.png");
    ok(&["render", "--sample", "--frame", "12", "--scale", "0.25", "--out", png.to_str().unwrap()]);
    assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG");
    let seq = dir.join("seq");
    let done: Value =
        serde_json::from_str(&ok(&["render-seq", "--sample", "--from", "2", "--to", "3", "--scale", "0.1", "--out-dir", seq.to_str().unwrap()]))
            .unwrap();
    assert_eq!(done["frames"], 2, "{done}");
    assert!(seq.join("frame_00002.png").exists() && !seq.join("frame_00001.png").exists());
}

#[test]
fn animations_are_written_and_replacing_one_needs_overwrite() {
    let dir = workdir("anim");
    for (cmd, file, magic) in [("gif", "a.gif", &b"GIF8"[..]), ("apng", "a.png", &b"\x89PNG"[..]), ("webp", "a.webp", &b"RIFF"[..])] {
        let path = dir.join(file);
        let p = path.to_str().unwrap();
        ok(&[cmd, "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--out", p]);
        assert!(std::fs::read(&path).unwrap().starts_with(magic), "{cmd}");
        // Replacing it needs --overwrite, bare or as `--overwrite yes`; `no` is the default.
        failed(&[cmd, "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--out", p]);
        failed(&[cmd, "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--out", p, "--overwrite", "no"]);
        ok(&[cmd, "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--out", p, "--overwrite"]);
        ok(&[cmd, "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--out", p, "--overwrite", "yes"]);
    }
    let p = dir.join("loop.png");
    ok(&["apng", "--sample", "--from", "1", "--to", "2", "--scale", "0.1", "--loop", "3", "--background", "--out", p.to_str().unwrap()]);
    ok(&[
        "apng",
        "--sample",
        "--from",
        "1",
        "--to",
        "2",
        "--scale",
        "0.1",
        "--loop",
        "forever",
        "--background",
        "yes",
        "--overwrite",
        "--out",
        p.to_str().unwrap(),
    ]);
}

#[test]
fn new_makes_a_document_that_info_reads() {
    let dir = workdir("new");
    let path = dir.join("n.anicraft");
    ok(&["new", "--out", path.to_str().unwrap(), "--width", "320", "--height", "200", "--fps", "12.5"]);
    let info: Value = serde_json::from_str(&ok(&["info", path.to_str().unwrap()])).unwrap();
    assert!(info.to_string().contains("320") && info.to_string().contains("200"), "{info}");
}

#[test]
fn export_any_format_built_in_or_plugin() {
    let dir = workdir("export");
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    // A built-in format by id.
    let svg = dir.join("f.svg");
    ok(&["export", "--sample", "--format", "svg", "--frame", "3", "--out", svg.to_str().unwrap()]);
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<svg"));
    // A sequence goes to a folder, one file per frame (#504).
    let seq = dir.join("vectors");
    ok(&["export", "--sample", "--format", "svg-sequence", "--from", "2", "--to", "3", "--out", seq.to_str().unwrap()]);
    assert!(seq.join("frame_00002.svg").exists() && seq.join("frame_00003.svg").exists() && !seq.join("frame_00001.svg").exists());
    // A format's flags are the ones it uses: a frame for a range format and a range for a frame format are refused.
    for args in [vec!["--format", "svg", "--from", "1"], vec!["--format", "svg-sequence", "--frame", "1"], vec!["--format", "png", "--to", "2"]] {
        let mut full = vec!["export", "--sample", "--out", "unused"];
        full.extend(args);
        let got = invoke(full.iter().copied());
        assert_eq!(got.code, 2, "{full:?}: {}", got.err);
        assert!(got.err.contains("is not used by the"), "{}", got.err);
    }
    // An unknown format names the known ones; options must be an object for a built-in.
    let err = failed(&["export", "--sample", "--format", "nope", "--out", "x"]);
    assert!(err.contains("lottie") && err.contains("--plugins"), "{err}");
    let got = invoke(["export", "--sample", "--format", "svg", "--out", svg.to_str().unwrap(), "--options", "[1]"]);
    assert_eq!((got.code, got.err.as_str()), (2, "error: --options must be a JSON object\n"));
}

#[test]
fn plugins_add_formats() {
    let dir = workdir("plugin");
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    // An exporter plug-in that writes the request it was handed.
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
    let plugins = plugins.to_str().unwrap();
    let listed = ok(&["formats", "--plugins", plugins]);
    assert!(listed.contains("org.test.count") && listed.contains("\"gif\""));
    assert!(!ok(&["formats"]).contains("org.test.count"), "plug-ins load only on request");
    let out = dir.join("movie.txt");
    let o = out.to_str().unwrap();
    let v = ok(&["export", "--sample", "--format", "org.test.count", "--from", "2", "--to", "5", "--out", o, "--plugins", plugins]);
    assert!(v.contains("\"frames\":4"), "{v}");
    let movie: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!((movie["result"]["from"].as_u64(), movie["result"]["frames"].as_u64()), (Some(1), Some(4)));
    // Replacing it needs --overwrite.
    failed(&["export", "--sample", "--format", "org.test.count", "--out", o, "--plugins", plugins]);
    ok(&["export", "--sample", "--format", "org.test.count", "--out", o, "--plugins", plugins, "--overwrite", "yes"]);
    // A folder with nothing loadable is still the plug-ins folder.
    ok(&["info", "--sample", "--plugins", dir.join("not-yet").to_str().unwrap()]);
}

#[test]
fn run_takes_a_script_or_a_saved_command() {
    let dir = workdir("run");
    let script = dir.join("s.json");
    std::fs::write(&script, "\u{feff}[{\"cmd\": \"edit.select_all\"}, {\"cmd\": \"modify.rotate_cw\"}]").unwrap();
    let results: Value = serde_json::from_str(&ok(&["run", "--sample", script.to_str().unwrap()])).unwrap();
    assert_eq!(results.as_array().map(Vec::len), Some(2), "{results}");
    // A step names itself.
    std::fs::write(&script, r#"[{"cmd": "edit.select_all"}, {"params": {}}]"#).unwrap();
    assert!(failed(&["run", "--sample", script.to_str().unwrap()]).contains("step 2: missing \"cmd\""));
    std::fs::write(&script, r#"[{"cmd": "edit.select_all"}, {"cmd": "no.such"}]"#).unwrap();
    assert!(failed(&["run", "--sample", script.to_str().unwrap()]).starts_with("error: step 2: no.such: "));
    std::fs::write(&script, "not json").unwrap();
    assert!(failed(&["run", "--sample", script.to_str().unwrap()]).contains("expected a JSON array of {cmd, params} or a saved command"));
    std::fs::write(&script, "[1]").unwrap();
    assert!(failed(&["run", "--sample", script.to_str().unwrap()]).contains("step 1: missing \"cmd\""));
    std::fs::write(&script, "5").unwrap();
    assert!(failed(&["run", "--sample", script.to_str().unwrap()]).contains("expected a JSON array of {cmd, params}:"));
    assert!(failed(&["run", "--sample", dir.join("none.json").to_str().unwrap()]).contains("none.json"));
    // A saved command (ADR 0097) runs as one transactional step.
    let file = dir.join("Spin.json");
    std::fs::write(&file, r#"{"format": "anicraft-command", "version": 1, "steps": [{"cmd": "edit.select_all"}, {"cmd": "modify.rotate_cw"}]}"#)
        .unwrap();
    let v: Value = serde_json::from_str(&ok(&["run", "--sample", file.to_str().unwrap()])).unwrap();
    assert_eq!((v["name"].as_str(), v["steps"].as_u64()), (Some("Spin"), Some(2)), "{v}");
    // A failing step names itself and leaves nothing behind.
    std::fs::write(
        &file,
        r#"{"steps": [{"cmd": "edit.select_all"}, {"cmd": "modify.rotate_cw"}, {"cmd": "layer.set_height", "params": {"height": 7}}]}"#,
    )
    .unwrap();
    assert!(failed(&["run", "--sample", file.to_str().unwrap()]).contains("stopped at step 3 of 3"));
    // --save writes the document, for a saved command too.
    std::fs::write(&file, r#"{"steps": [{"cmd": "edit.select_all"}]}"#).unwrap();
    let saved = dir.join("saved.anicraft");
    ok(&["run", "--sample", file.to_str().unwrap(), "--save", saved.to_str().unwrap()]);
    assert!(saved.exists());
}

#[test]
fn exec_runs_one_command_and_saves() {
    let dir = workdir("exec");
    let saved = dir.join("e.anicraft");
    let v: Value =
        serde_json::from_str(&ok(&["exec", "--sample", "draw.rect", r#"{"x":1,"y":1,"w":5,"h":5}"#, "--save", saved.to_str().unwrap()])).unwrap();
    assert!(v.is_object(), "{v}");
    assert!(saved.exists());
    // The command id is the first argument after the document, and one that doesn't exist is a failure.
    assert!(failed(&["exec", saved.to_str().unwrap(), "no.such"]).starts_with("error: no.such: "));
}

#[test]
fn guide_writes_the_book_and_searches_it() {
    let dir = workdir("guide");
    let site = dir.join("site");
    let wrote = ok(&["guide", "--out", site.to_str().unwrap()]);
    assert!(wrote.starts_with("wrote ") && site.join("index.html").exists(), "{wrote}");
    let out = ok(&["guide", "search", "onion", "skin"]);
    assert_eq!(out.lines().next(), Some("1. Onion skin · The timeline"), "{out}");
    assert!(out.contains("timeline.html#onion-skin"), "{out}");
    let json: Value = serde_json::from_str(&ok(&["guide", "search", "export", "gif", "--json", "yes", "--limit", "3"])).unwrap();
    let list = json.as_array().unwrap();
    assert!(list.len() <= 3 && list[0]["url"] == "getting-started.html#export-a-gif-or-a-video", "{json}");
    let bare: Value = serde_json::from_str(&ok(&["guide", "search", "export", "gif", "--json"])).unwrap();
    assert!(bare.is_array());
    assert!(ok(&["guide", "search", "zzzzqqq"]).starts_with("nothing in the user guide matches"));
    // Words with nothing to search for are a usage error, found by the run.
    let got = invoke(["guide", "search", "?!"]);
    assert_eq!((got.code, got.out.as_str()), (2, ""), "{}", got.err);
}

/// `fla-dump` reads a file only up to the legacy container's cap, and refuses one over it before
/// parsing; a file that isn't a compound binary is refused by its first bytes.
#[test]
fn fla_dump_refuses_files_over_the_cap_and_other_formats() {
    let dir = workdir("fla");
    let zip = dir.join("new.fla");
    std::fs::write(&zip, b"PK\x03\x04").unwrap();
    assert!(failed(&["fla-dump", zip.to_str().unwrap()]).contains("not a legacy Flash document"));
    assert!(failed(&["fla-dump", dir.join("none.fla").to_str().unwrap()]).contains("none.fla"));
    // One byte over the cap, sparse where the file system allows: refused, not parsed.
    #[cfg(unix)]
    {
        let big = dir.join("big.fla");
        let f = std::fs::File::create(&big).unwrap();
        std::io::Write::write_all(&mut &f, &anicraft_fla::MAGIC).unwrap();
        f.set_len(anicraft_fla::cfb::MAX_FILE + 1).unwrap();
        assert!(failed(&["fla-dump", big.to_str().unwrap()]).contains("larger than AniCraft reads"));
    }
}

/// `bench --stroke` walks the reporter's steps of #519 and `--stroke-file` the same on a document.
#[test]
fn stroke_bench_paints_moves_and_merges_a_long_stroke() {
    let out: Value = serde_json::from_str(&ok(&["bench", "--stroke", "400"])).unwrap();
    assert_eq!(out["samples"], 400);
    assert_eq!(out["merged"], true);
    let segments = out["segments"].as_u64().unwrap();
    assert!(segments > 50, "a scribble has an outline of many curves: {segments}");
    assert_eq!(out["segments_after"].as_u64(), Some(segments), "undo takes the move and its merge back");
    for step in ["draw_brush_ms", "hit_test_us", "move_ms", "float_ms", "deselect_merge_ms", "undo_ms", "render_ms"] {
        assert!(out[step].as_f64().is_some_and(|v| v >= 0.0), "{step} was timed: {out}");
    }
    // The same steps on a saved document's artwork.
    let dir = workdir("stroke");
    let file = dir.join("stroke.anicraft");
    ok(&["new", "--out", file.to_str().unwrap()]);
    ok(&["exec", file.to_str().unwrap(), "draw.rect", r#"{"x":1,"y":1,"w":50,"h":50}"#, "--save", file.to_str().unwrap(), "--in-place"]);
    let out: Value = serde_json::from_str(&ok(&["bench", "--stroke-file", file.to_str().unwrap()])).unwrap();
    assert!(out["samples"].is_null() && out["deselect_merge_ms"].is_number(), "{out}");
}

/// The timing budgets need a release build; a debug build says so and exits 2.
#[cfg(debug_assertions)]
#[test]
fn bench_check_needs_a_release_build() {
    let got = invoke(["bench", "--check", "yes"]);
    assert_eq!((got.code, got.out.as_str()), (2, ""));
    assert!(got.err.contains("release"), "{}", got.err);
}

/// A small stress document, measured: the report is JSON alone on stdout.
#[test]
fn bench_reports_json_on_stdout() {
    let report: Value = serde_json::from_str(&ok(&["bench", "--layers", "2", "--shapes", "3", "--frames", "4"])).unwrap();
    assert_eq!(report["document"]["layers"], 2);
    assert!(report["render_1080p_ms_per_frame"].is_number());
}
