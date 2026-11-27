//! In-memory fixture authored for the PR #660 rendering investigation.

use super::*;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const CACHE_BYTES: usize = 64 * 1024 * 1024;
const SAMPLES: [usize; 2] = [TILE as usize - 12, TILE as usize + 12];
const POSE_ROWS: [usize; 3] = [116, 180, 228];

fn paint_stage(ctx: &egui::Context, app: &mut App) -> egui::FullOutput {
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 900.0))), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| show(app, ui));
    out.textures_delta.clear();
    out
}

/// Read pixels from the topmost painted tile at each side of the seam. The red
/// stripe's vertical position identifies the tween pose independently of the tile metadata.
fn painted_frames(app: &App, out: &egui::FullOutput, images: &[Arc<FrameImage>]) -> [Option<u32>; 2] {
    SAMPLES.map(|x| {
        let tile = out.shapes.iter().rev().find_map(|shape| {
            let egui::Shape::Mesh(mesh) = &shape.shape else { return None };
            app.stage_cache.tiles.iter().find(|tile| {
                tile.tex.id() == mesh.texture_id
                    && tile.rect[0] <= x as f32
                    && (x as f32) < tile.rect[2]
                    && tile.rect[1] <= POSE_ROWS[0] as f32
                    && (POSE_ROWS[2] as f32) < tile.rect[3]
            })
        })?;
        let image = images.iter().find(|image| tile.key == image.key).expect("painted texture has a raster sample");
        let column = x - tile.rect[0] as usize;
        let red = |row| {
            let i = ((row - tile.rect[1] as usize) * image.width as usize + column) * 4;
            let pixel = &image.rgba_premul[i..i + 4];
            pixel[0] > 200 && pixel[1] < 80 && pixel[2] < 80
        };
        let marked: Vec<_> = POSE_ROWS.into_iter().enumerate().filter(|&(_, row)| red(row)).collect();
        assert_eq!(marked.len(), 1, "sampled tile must contain exactly one tween pose");
        Some(marked[0].0 as u32)
    })
}

fn paints_artwork(app: &App, out: &egui::FullOutput) -> bool {
    out.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Mesh(mesh)
        if app.stage_cache.tiles.iter().any(|tile| tile.tex.id() == mesh.texture_id))
    })
}

fn fixture(pasteboard: bool) -> (egui::Context, App, Vec<Arc<FrameImage>>) {
    let ctx = egui::Context::default();
    ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::MIN);
    let mut session = anicraft_engine::Session::default();
    session.doc.stage.width = if pasteboard { TILE } else { 2 * TILE };
    session.doc.stage.height = 256;
    for (command, args) in [
        ("draw.rect", json!({"x": 450, "y": 96, "w": 96, "h": 40, "fill": "#ff0000", "stroke": null})),
        ("modify.convert_to_symbol", json!({"name": "Playback fixture"})),
        ("symbol.set_kind", json!({"kind": "movie_clip"})),
        (
            "element.set_filters",
            json!({"filters": [
                {"type": "glow", "blur_x": 12, "blur_y": 12, "strength": 2, "quality": 3},
                {"type": "drop_shadow", "blur_x": 10, "blur_y": 10, "distance": 8, "angle": 45, "quality": 3}
            ]}),
        ),
        ("timeline.create_motion_tween", json!({})),
    ] {
        session.execute(command, &args).unwrap();
    }
    session.set_frame(1);
    session.execute("element.translate", &json!({"dx": 24, "dy": 64})).unwrap();
    session.set_frame(2);
    session.execute("element.translate", &json!({"dx": 0, "dy": 48})).unwrap();
    session.set_frame(0);

    let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
    app.view.fit = false;
    app.view.zoom = 1.0;
    app.view.clip_to_stage = !pasteboard;
    app.s.aids.show_grid = false;
    app.prefs.stage_renderer = crate::prefs::StageRenderer::Cpu;
    app.prefs.render_mode = RenderMode::Auto;
    app.playing = true;
    app.stage_cache.service = Some(RenderService::without_thread(CACHE_BYTES));
    let first = paint_stage(&ctx, &mut app);
    assert_eq!(app.stage_cache.ready.0, app.stage_cache.ready.1);
    let images: Vec<_> = app.stage_cache.tiles.iter().map(|tile| app.stage_cache.service.as_ref().unwrap().cached(tile.key).unwrap()).collect();
    assert_eq!(painted_frames(&app, &first, &images), [Some(0), Some(0)]);
    (ctx, app, images)
}

/// Requests at the fixture's fixed 100% zoom, over the rectangles drawn for frame zero.
fn jobs_at(app: &App, frame: u32) -> Vec<FrameJob> {
    let pic = app.s.stage_picture();
    let stage = &pic.doc.stage;
    app.stage_cache
        .tiles
        .iter()
        .filter(|tile| tile.frame == 0)
        .map(|tile| {
            let specs = tiles_for(1.0, (stage.width, stage.height), tile.rect, 0, Some(stage.background), !app.view.clip_to_stage);
            assert_eq!(specs.len(), 1);
            make_job_for(0, pic.state, &pic.doc, frame, specs[0].view, None)
        })
        .collect()
}

fn seam_tile(jobs: &[FrameJob], side: usize) -> &FrameJob {
    let x = (side as u32 * TILE) as f32;
    jobs.iter().find(|j| j.view.transform.translation() == Vec2::new(-x, 0.0)).unwrap()
}

fn wait_for_tiles(app: &App, jobs: &[FrameJob], images: &mut Vec<Arc<FrameImage>>) {
    let service = app.stage_cache.service.as_ref().unwrap();
    images.extend(jobs.iter().map(|j| service.wait_for(j.key, 5_000).unwrap()));
}

#[test]
fn playback_holds_one_pose_when_lookahead_finishes_either_tile_first() {
    for (pasteboard, side) in [(false, 0), (false, 1), (true, 0), (true, 1)] {
        let (ctx, mut app, mut images) = fixture(pasteboard);
        app.s.set_frame(1);
        let jobs = jobs_at(&app, 1);
        let first = seam_tile(&jobs, side);
        assert!(first.display_list().groups.iter().any(|group| group.filters.len() == 2));
        let service = RenderService::new(CACHE_BYTES, None);
        assert!(service.has_thread());
        // Finish an entire look-ahead pass containing just one seam tile.
        service.prefetch(vec![first.clone()]);
        images.push(service.wait_for(first.key, 5_000).unwrap());
        assert!(service.cached(seam_tile(&jobs, 1 - side).key).is_none());
        app.stage_cache.service = Some(service);

        let partial = paint_stage(&ctx, &mut app);
        assert_eq!(app.stage_cache.ready.0, 1);
        assert_eq!(
            painted_frames(&app, &partial, &images),
            [Some(0), Some(0)],
            "partial look-ahead painted the wrong poses at the seam (pasteboard={pasteboard}, side={side})"
        );
        wait_for_tiles(&app, &jobs, &mut images);
        let settled = paint_stage(&ctx, &mut app);
        assert_eq!(painted_frames(&app, &settled, &images), [Some(1), Some(1)]);
    }
}

#[test]
fn playback_finishes_one_worker_pass_even_after_the_playhead_advances() {
    use anicraft_engine::render_service::Raster;

    let (ctx, mut app, mut images) = fixture(false);
    let jobs = jobs_at(&app, 1);
    let next = jobs_at(&app, 2);
    let service = RenderService::new(CACHE_BYTES, None);
    assert!(service.has_thread());
    let (release, blocked) = mpsc::channel::<()>();
    let blocked = Mutex::new(blocked);
    let right = seam_tile(&jobs, 1);
    let slow_view = right.view.transform;
    service.set_rasterizer(Some(Arc::new(move |list, view| {
        if view.transform == slow_view {
            blocked.lock().unwrap().recv_timeout(Duration::from_secs(10)).map_err(|e| e.to_string())?;
        }
        let (pixmap, stats) = anicraft_engine::render::render(list, view).map_err(|e| e.to_string())?;
        Ok((Raster::Premul8(pixmap.to_premul_rgba8()), stats))
    })));
    let raster = service.raster_id();
    let mut batch = jobs.clone();
    batch.sort_by_key(|j| j.view.transform == slow_view);
    for job in &mut batch {
        // Inject a gated oracle for this pass while keeping the ordinary CPU cache keys.
        job.raster = raster;
    }
    service.request_batch(batch);
    let left = seam_tile(&jobs, 0);
    images.push(service.wait_for(left.key, 5_000).unwrap());
    assert!(service.cached(right.key).is_none() && service.busy());
    service.set_rasterizer(None);
    app.stage_cache.service = Some(service);
    app.s.set_frame(1);
    let partial = paint_stage(&ctx, &mut app);
    assert_eq!(painted_frames(&app, &partial, &images), [Some(0), Some(0)]);
    app.s.set_frame(2);
    let waiting = paint_stage(&ctx, &mut app);
    assert_eq!(painted_frames(&app, &waiting, &images), [Some(0), Some(0)]);
    release.send(()).unwrap();
    wait_for_tiles(&app, &jobs, &mut images);
    let completed = paint_stage(&ctx, &mut app);
    assert_eq!(
        painted_frames(&app, &completed, &images),
        [Some(1), Some(1)],
        "an advancing playhead should still be able to display a completed render"
    );

    paint_stage(&ctx, &mut app);
    wait_for_tiles(&app, &next, &mut images);
    let current = paint_stage(&ctx, &mut app);
    assert_eq!(painted_frames(&app, &current, &images), [Some(2), Some(2)]);
}

#[test]
fn playback_uses_the_backdrop_when_the_previous_view_is_unavailable() {
    for change in ["cold start", "pan", "zoom", "document edit", "eviction"] {
        let (ctx, mut app, _images) = fixture(false);
        app.s.set_frame(1);
        match change {
            "pan" => app.view.pan.x += 500.0,
            "zoom" => app.view.zoom = 0.8,
            "document edit" => {
                app.s.execute("draw.rect", &json!({"x": 40, "y": 30, "w": 5, "h": 5})).unwrap();
            }
            _ => {}
        }
        let jobs = jobs_at(&app, 1);
        if change == "cold start" {
            app.stage_cache.forget_document();
        } else if change == "eviction" {
            app.stage_cache.tiles.retain(|tile| tile.rect[0] == 0.0);
        }
        let service = RenderService::new(CACHE_BYTES, None);
        assert!(service.has_thread());
        let first = seam_tile(&jobs, 0);
        service.prefetch(vec![first.clone()]);
        service.wait_for(first.key, 5_000).unwrap();
        app.stage_cache.service = Some(service);

        let partial = paint_stage(&ctx, &mut app);
        assert!(app.stage_cache.ready.0 < app.stage_cache.ready.1);
        assert!(!paints_artwork(&app, &partial), "{change}: backdrop should cover a cold or invalidated view");
    }
}
