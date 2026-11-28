//! `bench --stroke`: what one long, fast, pressured Brush stroke costs (issue #519). The stroke
//! is painted, picked, moved, floated and deselected (which merges it into the drawing, ADR
//! 0122), and each step is timed. `--stroke-file` times the same steps on the artwork of a
//! document instead, so a reported file can be measured as it is.

use crate::{Args, avg_ms, open_session, round};
use anicraft_engine::Session;
use serde_json::{Value, json};
use std::time::Instant;

/// Samples of the default scenario: about 50 seconds of drawing at a tablet's 120 reports a
/// second.
pub const DEFAULT_SAMPLES: u32 = 6000;
/// Most samples the scenario takes (the command's own limit is `geom::brush::MAX_SAMPLES`).
const MAX_SAMPLES: u32 = 100_000;

/// A fast scribble over a 1280 × 720 stage: loops that drift across it and cross themselves
/// again and again, with the pressure swelling and fading. The same for a given `n`.
pub fn scribble(n: u32) -> Vec<[f32; 3]> {
    let n = n.clamp(2, MAX_SAMPLES);
    (0..n)
        .map(|i| {
            // About 16 stage units between samples: a fast hand.
            let t = i as f32 * 0.06;
            let x = 640.0 + 420.0 * (t * 0.11).sin() + 170.0 * (t * 0.83).cos();
            let y = 360.0 + 200.0 * (t * 0.07).cos() + 130.0 * (t * 0.83).sin();
            [x, y, 0.55 + 0.45 * (t * 0.37).sin()]
        })
        .collect()
}

fn ms(t: Instant) -> f64 {
    round(t.elapsed().as_secs_f64() * 1000.0)
}

/// Curve and line segments of every shape on the playhead's frame.
fn segments(s: &Session) -> usize {
    s.display_list().items.iter().map(|i| i.path.cmds.len()).sum()
}

pub fn bench_stroke(a: &Args) -> Result<String, String> {
    let file = a.flag("stroke-file");
    let samples = match a.flag("stroke") {
        Some("yes" | "true") | None => DEFAULT_SAMPLES,
        Some(v) => v.parse::<u32>().map_err(|_| "--stroke takes a number of samples (or yes)".to_string())?.clamp(2, MAX_SAMPLES),
    };
    let mut out = json!({"build": if cfg!(debug_assertions) { "debug (use --release for real numbers)" } else { "release" }});
    let mut s = match file {
        Some(p) => {
            out["file"] = json!(p);
            open_session(Some(p))?
        }
        None => {
            let mut s = Session::default();
            let _ = s.execute("draw.object_drawing", &json!({"on": false}));
            let t = Instant::now();
            let v = s.execute("draw.brush", &json!({"points": scribble(samples), "size": 10, "color": "#000000", "object": false})).map_err(|e| e.to_string())?;
            out["samples"] = json!(samples);
            out["draw_brush_ms"] = json!(ms(t));
            out["pieces"] = json!(v["elements"].as_array().map_or(0, Vec::len));
            out["merged"] = v["merged"].clone();
            s
        }
    };
    out["segments"] = json!(segments(&s));
    out["elements"] = json!(s.visible_elements().len());

    let doc = s.doc.clone();
    let frame = s.frame;
    out["evaluate_ms"] = json!(round(avg_ms(5, |_| {
        let _ = anicraft_doc::evaluate(&doc, frame);
    })));
    let view = anicraft_render::View::stage(&doc, 1.0);
    let dl = anicraft_doc::evaluate(&doc, frame);
    out["render_ms"] = json!(round(avg_ms(3, |_| {
        let _ = anicraft_render::render(&dl, &view);
    })));
    out["hit_test_us"] = json!(round(
        avg_ms(200, |i| {
            let _ = s.hit_test(anicraft_geom::Vec2::new((i * 37 % 1280) as f32, (i * 53 % 720) as f32), 2.0);
        }) * 1000.0
    ));

    // The reporter's steps: select it, move it, let go of the mouse (it floats), deselect (it
    // merges into the drawing).
    let mut step = |s: &mut Session, key: &str, id: &str, p: Value| -> Result<Value, String> {
        let t = Instant::now();
        let v = s.execute(id, &p).map_err(|e| format!("{id}: {e}"))?;
        out[key] = json!(ms(t));
        Ok(v)
    };
    step(&mut s, "select_all_ms", "edit.select_all", json!({}))?;
    step(&mut s, "move_ms", "element.translate", json!({"dx": 24, "dy": 16}))?;
    step(&mut s, "float_ms", "draw.float", json!({}))?;
    step(&mut s, "deselect_merge_ms", "edit.deselect", json!({}))?;
    let t = Instant::now();
    let _ = s.undo();
    out["undo_ms"] = json!(ms(t));
    out["segments_after"] = json!(segments(&s));
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}
