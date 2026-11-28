//! AniCraft CPU oracle renderer (layer L2).
//!
//! Deterministic: the same display list and view always produce the same bytes, whatever the
//! thread count. Each output row is written by exactly one task and shapes composite in order.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod paint;
pub mod raster;
pub mod stroke;
pub mod filters;
pub mod hairline;
pub mod html;
pub mod lottie;
mod lottie_keys;
pub mod svg;
pub mod warp;

use anicraft_doc::{BlendMode, ColorTransform, DisplayItem, DisplayList, Document, MaskMode, Paint, Rgba, evaluate};
use anicraft_geom::{Affine, FillRule, Rect, Vec2};
use paint::{Premul, Shader, premultiply};
use raster::{Coverage, IRect};
use rayon::prelude::*;

/// Largest surface we allocate (pixels): 4096 × 4096.
pub const MAX_PIXELS: u64 = 4096 * 4096;
/// Flattening tolerance in output pixels.
pub const TOLERANCE: f32 = 0.1;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("image {0}×{1} is too large (limit {MAX_PIXELS} pixels)")]
    TooLarge(u32, u32),
    #[error("PNG encoding failed: {0}")]
    Png(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Premultiplied f32 RGBA surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<Premul>,
}

impl Pixmap {
    pub fn new(width: u32, height: u32) -> Result<Pixmap, RenderError> {
        let n = u64::from(width) * u64::from(height);
        if n > MAX_PIXELS {
            return Err(RenderError::TooLarge(width, height));
        }
        Ok(Pixmap { width, height, data: vec![[0.0; 4]; n as usize] })
    }

    pub fn fill(&mut self, c: Rgba) {
        let p = premultiply(c);
        self.data.fill(p);
    }

    /// Straight-alpha 8-bit RGBA bytes.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for p in &self.data {
            let a = p[3].clamp(0.0, 1.0);
            let un = |v: f32| if a > 1e-6 { (v / a).clamp(0.0, 1.0) } else { 0.0 };
            let q = |v: f32| (v * 255.0 + 0.5) as u8;
            out.extend_from_slice(&[q(un(p[0])), q(un(p[1])), q(un(p[2])), q(a)]);
        }
        out
    }

    /// Premultiplied 8-bit RGBA (what GPU textures with premultiplied alpha expect).
    pub fn to_premul_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for p in &self.data {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out.extend_from_slice(&[q(p[0]), q(p[1]), q(p[2]), q(p[3])]);
        }
        out
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, RenderError> {
        encode_png(self.width, self.height, &self.to_rgba8())
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<Premul> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.data.get(y as usize * self.width as usize + x as usize).copied()
    }
}

/// Encodes straight-alpha RGBA8 as PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, RenderError> {
    let img = craft_image_codecs::Image::from_u8(width, height, craft_image_codecs::ChannelLayout::Rgba, rgba.to_vec())
        .map_err(|e| RenderError::Png(e.to_string()))?;
    craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default())
        .map_err(|e| RenderError::Png(e.to_string()))
}

/// Where and how to draw.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Stage → pixel transform.
    pub transform: Affine,
    pub width: u32,
    pub height: u32,
    /// Fill colour before drawing (`None` = transparent).
    pub background: Option<Rgba>,
}

impl View {
    /// The whole stage at `scale`.
    pub fn stage(doc: &Document, scale: f32) -> View {
        let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        View {
            transform: Affine::scale(s, s),
            width: ((doc.stage.width as f32 * s).round() as u32).max(1),
            height: ((doc.stage.height as f32 * s).round() as u32).max(1),
            background: Some(doc.stage.background),
        }
    }
}

/// Statistics from one render (for the status bar and perf logs).
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub items: usize,
    pub drawn: usize,
    /// Items skipped because they lie outside the view.
    pub culled: usize,
    pub edges: usize,
    pub pixels_touched: u64,
}

/// Renders frame `frame` of `doc` at `scale` with the stage background.
pub fn render_frame(doc: &Document, frame: u32, scale: f32) -> Result<Pixmap, RenderError> {
    let dl = evaluate(doc, frame);
    render(&dl, &View::stage(doc, scale)).map(|(p, _)| p)
}

/// Renders a display list.
pub fn render(dl: &DisplayList, view: &View) -> Result<(Pixmap, RenderStats), RenderError> {
    render_cut(dl, view, true)
}

/// [`render`]; `cut` off prepares every item whole (see [`unseen`]), which draws the same
/// pixels more slowly: the tests compare the two.
fn render_cut(dl: &DisplayList, view: &View, cut: bool) -> Result<(Pixmap, RenderStats), RenderError> {
    let mut pm = Pixmap::new(view.width, view.height)?;
    // The background is filled band by band, in parallel, just before each band draws (a
    // serial fill of a 1080p float buffer cost as much as all the drawing).
    let background = view.background.map(premultiply);
    let surface = IRect { x0: 0, y0: 0, x1: view.width as i32, y1: view.height as i32 };

    // Filtered groups are drawn on their own, filtered, and come back as one image each.
    let runs = filtered_runs(dl);
    let in_run = in_runs(dl, &runs);
    let joined = joined_runs(dl);

    // Phase 1 (parallel over items): cull, flatten, stroke and build shaders. Output keeps
    // display-list order.
    let mut prepared: Vec<Vec<Prepared>> = dl.items.par_iter().zip(&in_run).zip(&joined).map(|((item, &skip), &run)| if skip { Vec::new() } else { prepare(item, &view.transform, surface, soft_art(dl, item), cut, run) }).collect();
    for r in &runs {
        if let (Some(op), Some(slot)) = (filtered_group(dl, r, view, surface)?, prepared.get_mut(r.start)) {
            slot.push(op);
        }
    }
    let mut stats = RenderStats { items: dl.items.len(), ..Default::default() };
    let mut ops: Vec<Prepared> = Vec::with_capacity(prepared.len());
    let mut mask_ops: Vec<Prepared> = Vec::new();
    for (i, p) in prepared.into_iter().enumerate() {
        if p.is_empty() && !in_run.get(i).copied().unwrap_or(false) {
            stats.culled += 1;
        }
        for op in p {
            if op.masks.is_some() { mask_ops.push(op) } else { ops.push(op) }
        }
    }
    let clips = Clips::new(&dl.clip_parents, &dl.clip_modes, mask_ops);
    // A clipped op can only touch its clip's area; an op whose clip is empty draws nothing.
    ops.retain_mut(|op| {
        let Some(c) = op.clip else { return true };
        op.bounds = op.bounds.intersect(&clips.bounds(c));
        !op.bounds.is_empty()
    });
    drop_lone_runs(&mut ops);
    stats.drawn = ops.len();
    for op in &ops {
        stats.edges += op.polys.iter().map(Vec::len).sum::<usize>();
        stats.pixels_touched += (op.bounds.width() * op.bounds.height()) as u64;
    }

    // Phase 2 (parallel over horizontal bands): each band rasterizes every op that touches
    // it, in order, into its own rows. A row is owned by exactly one band, so the result is
    // identical to a serial render whatever the thread count.
    let w = view.width as usize;
    let chains = group_chains(&dl.groups);
    pm.data.par_chunks_mut(BAND_ROWS * w.max(1)).enumerate().for_each(|(bi, rows)| {
        if let Some(bg) = background {
            rows.fill(bg);
        }
        let y0 = (bi * BAND_ROWS) as i32;
        let band = IRect { x0: 0, y0, x1: surface.x1, y1: (y0 + BAND_ROWS as i32).min(surface.y1) };
        let mut cov = Coverage::new(IRect { x0: 0, y0: 0, x1: 0, y1: 0 });
        let mut row = Vec::new();
        let mut band_clips = BandClips::default();
        // Open groups, outermost first: each composites into its own band buffer, which blends
        // into the one below at the group's alpha when the group's run of ops ends.
        let mut open: Vec<OpenGroup> = Vec::new();
        // The run of joined fills being added up (ADR 0138); it goes into the group or the rows
        // its fills are drawn in when the next op isn't one of it. Its buffers are this
        // thread's, kept from band to band and frame to frame (a run leaves them empty):
        // making them anew for every band cost as much as the drawing.
        let mut run = RUN.take();
        for op in &ops {
            let b = IRect { x0: op.bounds.x0, y0: op.bounds.y0.max(band.y0), x1: op.bounds.x1, y1: op.bounds.y1.min(band.y1) };
            if b.is_empty() {
                continue;
            }
            if run.id != 0 && run.id != op.run {
                let mask = run.clip.map(|c| band_clips.get(&clips, c, band, w, &mut cov, &mut row));
                run.flush(band.y0, w, open.last_mut().map_or(&mut *rows, |top| &mut top.buf), mask.as_deref().map(Vec::as_slice));
            }
            let want = op.group.and_then(|g| chains.get(g as usize)).map_or(&[][..], Vec::as_slice);
            let keep = open.iter().zip(want).take_while(|(o, g)| o.id == **g).count();
            while open.len() > keep {
                close_group(&mut open, rows, &dl.groups, band.y0, w);
            }
            for &g in want.iter().skip(keep) {
                open.push(OpenGroup { id: g, buf: vec![[0.0; 4]; rows.len()], dirty: NO_PIXELS });
            }
            let mask = op.clip.map(|c| band_clips.get(&clips, c, band, w, &mut cov, &mut row));
            if op.hairline {
                cov.reset_direct(b);
                for poly in &op.polys {
                    cov.add_hairline(poly);
                }
            } else {
                cov.reset(b);
                for poly in &op.polys {
                    cov.add_polygon(poly);
                }
            }
            if op.run != 0 {
                if let Some(top) = open.last_mut() {
                    top.dirty = top.dirty.union(&b);
                }
                (run.id, run.clip) = (op.run, op.clip);
                let len = rows.len();
                let under = open.last_mut().map_or(&mut *rows, |top| &mut top.buf);
                run.add(&cov, op, b, (band.y0, w, len), &mut row, Some((under, mask.as_deref().map(Vec::as_slice))));
                continue;
            }
            match open.last_mut() {
                Some(top) => {
                    top.dirty = top.dirty.union(&b);
                    composite(&cov, op, b, band.y0, w, &mut top.buf, &mut row, mask.as_deref().map(Vec::as_slice));
                }
                None => composite(&cov, op, b, band.y0, w, rows, &mut row, mask.as_deref().map(Vec::as_slice)),
            }
        }
        if run.id != 0 {
            let mask = run.clip.map(|c| band_clips.get(&clips, c, band, w, &mut cov, &mut row));
            run.flush(band.y0, w, open.last_mut().map_or(&mut *rows, |top| &mut top.buf), mask.as_deref().map(Vec::as_slice));
        }
        while !open.is_empty() {
            close_group(&mut open, rows, &dl.groups, band.y0, w);
        }
        RUN.set(run);
    });
    // Tens of thousands of small polygon buffers: free them on the pool, off the frame's
    // critical path (it cost about 2 ms at 1080p).
    drop_in_background((ops, clips));
    Ok((pm, stats))
}

/// Frees `x` on the thread pool, off the caller's path. In the browser (wasm32, one thread, ADR
/// 0036) it is freed here: rayon runs a spawned job only when the thread next waits on the pool.
pub fn drop_in_background<T: Send + 'static>(x: T) {
    #[cfg(not(target_arch = "wasm32"))]
    rayon::spawn(move || drop(x));
    #[cfg(target_arch = "wasm32")]
    drop(x);
}

/// Why a display list (or one stage tile of it) needs the full renderer: what another renderer
/// drawing a prepared [`Scene`] can't (see [`prepare_scene`] and, for the GPU stage's limits,
/// ADR 0032).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedsFull {
    /// Mask layers nested deeper than the other renderer handles.
    Clips,
    /// Symbol instances nested deeper than the other renderer handles.
    Groups,
    /// Filtered symbols nested deeper than the other renderer handles.
    Filters,
    /// More bitmaps at once, or larger ones, than the other renderer handles.
    Bitmaps,
}

impl std::fmt::Display for NeedsFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NeedsFull::Clips => "deeply nested masks",
            NeedsFull::Groups => "deeply nested symbols",
            NeedsFull::Filters => "deeply nested filters",
            NeedsFull::Bitmaps => "too many or too large bitmaps",
        })
    }
}

/// One fill operation in surface pixels, exactly as this renderer prepares it: flattened at
/// [`TOLERANCE`], strokes already outlined by the stroker, the paint's shader (with its gradient
/// table) built, and pixel bounds clamped to the surface (and to its clip's bounds). Other
/// renderers (the GPU stage, ADR 0019 and 0029) start from these, so the geometry and the
/// colours are the oracle's own.
pub struct Fill {
    pub polys: Vec<Vec<Vec2>>,
    pub rule: FillRule,
    pub shader: Shader,
    pub bounds: IRect,
    /// Drawn only inside this clip ([`Scene::clips`]): coverage times the clip's coverage.
    pub clip: Option<u32>,
    /// Composited inside this group ([`Scene::groups`]) and the groups around it.
    pub group: Option<u32>,
    /// The pixels of this filtered layer ([`Scene::layers`]), where it covers the fill (a
    /// rectangle on whole pixels), instead of `shader` (then a transparent placeholder). As
    /// [`render`] draws it: the layer's pixel at `(x, y)` is at `(x - x0, y - y0)` of its
    /// `bounds`, transparent outside.
    pub layer: Option<u32>,
    /// A hairline (outline mode, ADR 0048): `polys` are open polylines (a closed one repeats
    /// its first point), drawn by [`hairline`]'s distance coverage; `rule` doesn't apply.
    pub hairline: bool,
    /// Not 0: one of a run of joined fills (ADR 0138), the fills next to it in drawing order
    /// with the same number. They are fills of one merged drawing, which lie side by side, so
    /// a pixel two of them share an edge in is covered by both together, not by one over the
    /// other. Per pixel, a run starts transparent with nothing covered; each of its fills, of
    /// coverage `k` and colour `s`, with `c` of the pixel covered so far, first hides what it
    /// must overlap (`o = max(0, c + k − 1)`: the run's colour times `1 − o / c · s.a`), then
    /// adds `s · k`, and `c` becomes `min(1, c + k)`; when the run ends its colour, times its
    /// clip's coverage, goes src-over onto what is below. Where one fill covers the pixel
    /// whole this is src-over, so a fill that covers a pixel whole while the run has nothing
    /// there (`c` is 0) goes src-over onto what is below at once, inside its clip, and leaves
    /// `c` at 0. Clip artwork joins the same way, without that shortcut: a coverage clip
    /// unites with the run's `c` at its end, a soft one composites the run's colour.
    pub run: u32,
}

/// A filtered group (a symbol instance with filters) as [`render`] draws it: its items on
/// their own, into a transparent buffer of `bounds` (its artwork grown by how far its filters
/// reach, within the surface's room), then its filters in order. The result is composited as
/// one fill ([`Fill::layer`]) in the group, clipped by the clip around the instance.
pub struct FilterLayer {
    /// The group's items, with the group neutral (its opacity and blend mode apply when the
    /// layer is composited) and clips from outside it removed. It may hold filtered groups of
    /// its own (nested symbols): prepare it as its own scene.
    pub list: DisplayList,
    /// Where `list` draws: `bounds`' size, transparent.
    pub view: View,
    /// The buffer's place, in surface pixels.
    pub bounds: IRect,
    /// The group's filters, resolved for the view's scale ([`filters::resolve`]).
    pub filters: Vec<filters::FilterOp>,
}

/// A clip (a mask layer's area) as [`render`] composites it.
pub struct ClipLayer {
    /// The clip this one is inside (always an earlier one; a malformed link is already `None`
    /// and its bounds empty, as [`render`] treats it).
    pub parent: Option<u32>,
    /// How the artwork reveals (ADR 0070). Coverage: `art` is solid fill areas whose coverages
    /// unite (`a + b − ab`). Alpha or luminance: `art` is painted fills (their strokes too)
    /// composited src-over onto transparent, and the result's opacity, or its luminance
    /// ([`MaskMode::reveal`]), is the clip's coverage.
    pub mode: MaskMode,
    pub art: Vec<Fill>,
    /// Where the clip, and so anything drawn inside it, can be non-zero.
    pub bounds: IRect,
}

/// A group (a symbol instance composited on its own) as [`render`] composites it.
pub struct GroupLayer {
    /// The groups it is in, outermost first, ending with itself.
    pub chain: Vec<u32>,
    /// Opacity, already sanitized as [`render`] uses it (finite, `0..=1`).
    pub alpha: f32,
    pub blend: BlendMode,
}

/// A display list prepared exactly as [`render`] prepares it, for another renderer: the fills
/// in drawing order, with the clips and groups they are drawn in. Compositing it per pixel as
/// [`render`]'s band compositor does reproduces [`render`]: each fill's analytic coverage under
/// its fill rule, times its clip's coverage (the union of the clip's artwork, times its parent
/// clip's), src-over into the innermost open group; a group opens (transparent) when the first
/// of its fills draws and blends into what is below at its alpha and in its blend mode
/// ([`blend`]) when its run of fills ends.
pub struct Scene {
    pub fills: Vec<Fill>,
    pub clips: Vec<ClipLayer>,
    pub groups: Vec<GroupLayer>,
    /// Filtered groups, drawn and filtered before the fills that show them ([`Fill::layer`]).
    pub layers: Vec<FilterLayer>,
}

/// The display list as a [`Scene`], with the same culling and statistics as [`render`]. Bitmap
/// fills come as [`Shader::Bitmap`]; filtered groups as [`Scene::layers`]. It doesn't fail
/// today; the `Err` is for what a future display list could hold that only [`render`] draws.
pub fn prepare_scene(dl: &DisplayList, view: &View) -> Result<(Scene, RenderStats), NeedsFull> {
    let surface = IRect { x0: 0, y0: 0, x1: view.width as i32, y1: view.height as i32 };
    let runs = filtered_runs(dl);
    let in_run = in_runs(dl, &runs);
    let joined = joined_runs(dl);
    let mut prepared: Vec<Vec<Prepared>> = dl.items.par_iter().zip(&in_run).zip(&joined).map(|((item, &skip), &run)| if skip { Vec::new() } else { prepare(item, &view.transform, surface, soft_art(dl, item), true, run) }).collect();
    let mut layers = Vec::new();
    for r in &runs {
        if let (Some(setup), Some(slot)) = (filter_setup(dl, r, view, surface), prepared.get_mut(r.start)) {
            let mut op = setup.op(Shader::Solid([0.0; 4]));
            op.layer = Some(layers.len() as u32);
            slot.push(op);
            layers.push(setup.layer);
        }
    }
    let mut stats = RenderStats { items: dl.items.len(), ..Default::default() };
    let mut ops: Vec<Prepared> = Vec::with_capacity(prepared.len());
    let mut mask_ops: Vec<Prepared> = Vec::new();
    for (i, p) in prepared.into_iter().enumerate() {
        if p.is_empty() && !in_run.get(i).copied().unwrap_or(false) {
            stats.culled += 1;
        }
        for op in p {
            if op.masks.is_some() { mask_ops.push(op) } else { ops.push(op) }
        }
    }
    // The same clips, bounds and culling of clipped fills as `render`.
    let clips = Clips::new(&dl.clip_parents, &dl.clip_modes, mask_ops);
    ops.retain_mut(|op| {
        let Some(c) = op.clip else { return true };
        op.bounds = op.bounds.intersect(&clips.bounds(c));
        !op.bounds.is_empty()
    });
    drop_lone_runs(&mut ops);
    let fill = |op: Prepared| Fill { polys: op.polys, rule: op.rule, shader: op.shader, bounds: op.bounds, clip: op.clip, group: op.group, layer: op.layer, hairline: op.hairline, run: op.run };
    let mut fills = Vec::with_capacity(ops.len());
    for op in ops {
        stats.edges += op.polys.iter().map(Vec::len).sum::<usize>();
        stats.pixels_touched += (op.bounds.width() * op.bounds.height()) as u64;
        fills.push(fill(op));
    }
    stats.drawn = fills.len();
    let Clips { parents, modes, art, bounds } = clips;
    let clips = parents.into_iter().zip(modes).zip(art).zip(bounds).map(|(((parent, mode), art), bounds)| ClipLayer { parent, mode, art: art.into_iter().map(fill).collect(), bounds }).collect();
    let groups = group_chains(&dl.groups)
        .into_iter()
        .zip(&dl.groups)
        .map(|(chain, g)| GroupLayer { chain, alpha: if g.alpha.is_finite() { g.alpha.clamp(0.0, 1.0) } else { 1.0 }, blend: g.blend })
        .collect();
    Ok((Scene { fills, clips, groups, layers }, stats))
}

/// A run of items inside a filtered group that has no filtered group around it.
struct FilteredRun {
    group: u32,
    start: usize,
    end: usize,
}

fn filtered_runs(dl: &DisplayList) -> Vec<FilteredRun> {
    if dl.groups.iter().all(|g| g.filters.is_empty()) {
        return Vec::new();
    }
    let chains = group_chains(&dl.groups);
    let top = |i: usize| -> Option<u32> {
        let g = dl.items.get(i)?.group?;
        chains.get(g as usize)?.iter().copied().find(|&c| dl.groups.get(c as usize).is_some_and(|x| !x.filters.is_empty()))
    };
    let mut out: Vec<FilteredRun> = Vec::new();
    for i in 0..dl.items.len() {
        match (top(i), out.last_mut()) {
            (Some(g), Some(last)) if last.group == g && last.end == i => last.end = i + 1,
            (Some(g), _) => out.push(FilteredRun { group: g, start: i, end: i + 1 }),
            (None, _) => {}
        }
    }
    out
}

/// Which items are inside a filtered run (drawn with their group, not on their own).
fn in_runs(dl: &DisplayList, runs: &[FilteredRun]) -> Vec<bool> {
    let mut in_run = vec![false; dl.items.len()];
    for r in runs {
        for f in in_run.iter_mut().take(r.end).skip(r.start) {
            *f = true;
        }
    }
    in_run
}

/// Largest buffer a filtered group is drawn into (pixels): what [`render`] accepts.
const MAX_FILTER_PIXELS: i64 = MAX_PIXELS as i64;

/// A filtered group set up for drawing: the layer, and the part of it the surface shows, with
/// the clip and group it is composited in.
struct FilterSetup {
    layer: FilterLayer,
    shown: IRect,
    clip: Option<u32>,
    group: u32,
}

impl FilterSetup {
    /// The op that composites the layer (a rectangle on whole pixels), painted by `shader`.
    fn op(&self, shader: Shader) -> Prepared {
        let s = self.shown;
        let (x0, y0, x1, y1) = (s.x0 as f32, s.y0 as f32, s.x1 as f32, s.y1 as f32);
        Prepared {
            polys: vec![vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]],
            rule: FillRule::NonZero,
            shader,
            bounds: s,
            clip: self.clip,
            masks: None,
            group: Some(self.group),
            layer: None,
            hairline: false,
            run: 0,
        }
    }
}

/// Where and how a filtered group's items are drawn on their own (the group neutral, clips from
/// outside it left for later); `None` when nothing of it can show.
fn filter_setup(dl: &DisplayList, run: &FilteredRun, view: &View, surface: IRect) -> Option<FilterSetup> {
    let group = dl.groups.get(run.group as usize)?;
    let items = dl.items.get(run.start..run.end).unwrap_or(&[]);
    let scale = view.transform.mean_scale();
    let margin = (group.filters.iter().map(anicraft_doc::Filter::margin).sum::<f32>() * scale).ceil() + 2.0;
    // Where it can draw: its artwork's bounds, grown by how far the filters spread.
    let mut r = Rect::EMPTY;
    for it in items.iter().filter(|i| i.masks.is_none() && !i.guide) {
        let pad = it.stroke.as_ref().map_or(0.0, anicraft_doc::Stroke::reach) + 1.0;
        r = r.union(&it.path.bounds().inflate(pad).transformed(&view.transform.mul(&it.transform)));
    }
    if r.is_empty() || !r.min.x.is_finite() || !r.max.y.is_finite() {
        return None;
    }
    let reach = margin.clamp(0.0, (i32::MAX / 4) as f32) as i32;
    // How far past the surface it draws: the reach, but no further than keeps the buffer within
    // what `render` accepts (the largest `c` with (w + 2c)(h + 2c) <= the limit); a filter
    // reaching further is cut there, off the surface.
    let (sw, sh) = (surface.width() as f64, surface.height() as f64);
    let room = ((((sw + sh).powi(2) - 4.0 * (sw * sh - MAX_FILTER_PIXELS as f64)).max(0.0).sqrt() - (sw + sh)) / 4.0).floor().max(0.0) as i32;
    let around = reach.min(room);
    let limit = IRect { x0: surface.x0.saturating_sub(around), y0: surface.y0.saturating_sub(around), x1: surface.x1.saturating_add(around), y1: surface.y1.saturating_add(around) };
    let b = IRect {
        x0: (r.min.x.floor() as i32).saturating_sub(reach),
        y0: (r.min.y.floor() as i32).saturating_sub(reach),
        x1: (r.max.x.ceil() as i32).saturating_add(reach),
        y1: (r.max.y.ceil() as i32).saturating_add(reach),
    }
    .intersect(&limit);
    if b.is_empty() || (b.width() as i64) * (b.height() as i64) > MAX_FILTER_PIXELS {
        return None;
    }
    let shown = b.intersect(&surface);
    if shown.is_empty() {
        return None;
    }
    // The group's own clips (masks inside the symbol) apply before filtering; others after.
    let inner: std::collections::HashSet<u32> = items.iter().filter_map(|i| i.masks).collect();
    let mut groups = dl.groups.clone();
    if let Some(g) = groups.get_mut(run.group as usize) {
        *g = anicraft_doc::Group { parent: None, alpha: 1.0, blend: BlendMode::Normal, filters: Default::default(), clip: None };
    }
    let list = DisplayList {
        items: items.iter().map(|i| DisplayItem { clip: i.clip.filter(|c| inner.contains(c)), ..i.clone() }).collect(),
        clip_parents: dl.clip_parents.iter().map(|p| p.filter(|c| inner.contains(c))).collect(),
        clip_modes: dl.clip_modes.clone(),
        groups,
        links: Vec::new(),
        left_out: dl.left_out,
    };
    let sub_view = View { transform: Affine::translate(-b.x0 as f32, -b.y0 as f32).mul(&view.transform), width: b.width() as u32, height: b.height() as u32, background: None };
    let filters = group.filters.iter().map(|f| filters::resolve(f, scale)).collect();
    Some(FilterSetup { layer: FilterLayer { list, view: sub_view, bounds: b, filters }, shown, clip: group.clip, group: run.group })
}

/// Draws a filtered group's items on their own, applies its filters, and returns the result as
/// one image op in the group, clipped by the clip in force where the instance is.
fn filtered_group(dl: &DisplayList, run: &FilteredRun, view: &View, surface: IRect) -> Result<Option<Prepared>, RenderError> {
    let Some(setup) = filter_setup(dl, run, view, surface) else { return Ok(None) };
    let Some(group) = dl.groups.get(run.group as usize) else { return Ok(None) };
    let b = setup.layer.bounds;
    let (mut pm, _) = render(&setup.layer.list, &setup.layer.view)?;
    filters::apply(&mut pm.data, b.width(), b.height(), &group.filters, view.transform.mean_scale());
    Ok(Some(setup.op(Shader::Image { data: std::sync::Arc::new(pm.data), x0: b.x0, y0: b.y0, w: b.width() })))
}

/// A group being composited in one band: its own buffer (laid out like the band's rows) and
/// the part of it drawn into.
struct OpenGroup {
    id: u32,
    buf: Vec<Premul>,
    dirty: IRect,
}

/// Each group's chain of groups, outermost first. A malformed parent link (not earlier in the
/// list) is treated as no parent.
pub(crate) fn group_chains(groups: &[anicraft_doc::Group]) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = Vec::with_capacity(groups.len());
    for (i, g) in groups.iter().enumerate() {
        let mut chain = g.parent.filter(|&p| (p as usize) < i).and_then(|p| out.get(p as usize)).cloned().unwrap_or_default();
        chain.push(i as u32);
        out.push(chain);
    }
    out
}

/// Blends the innermost open group into what is below it, at the group's alpha and in its
/// blend mode.
fn close_group(open: &mut Vec<OpenGroup>, rows: &mut [Premul], groups: &[anicraft_doc::Group], band_y0: i32, w: usize) {
    let Some(g) = open.pop() else { return };
    let alpha = groups.get(g.id as usize).map_or(1.0, |x| if x.alpha.is_finite() { x.alpha.clamp(0.0, 1.0) } else { 1.0 });
    let mode = groups.get(g.id as usize).map_or(BlendMode::Normal, |x| x.blend);
    let dst: &mut [Premul] = match open.last_mut() {
        Some(below) => {
            below.dirty = below.dirty.union(&g.dirty);
            &mut below.buf
        }
        None => rows,
    };
    let d = g.dirty;
    if d.is_empty() || alpha <= 0.0 {
        return;
    }
    if !matches!(mode, BlendMode::Normal | BlendMode::Layer) {
        for py in d.y0..d.y1 {
            let start = (py - band_y0) as usize * w + d.x0 as usize;
            let (Some(src), Some(out)) = (g.buf.get(start..start + d.width()), dst.get_mut(start..start + d.width())) else { continue };
            for (o, s) in out.iter_mut().zip(src) {
                if s[3] > 0.0 {
                    *o = blend(mode, [s[0] * alpha, s[1] * alpha, s[2] * alpha, s[3] * alpha], *o);
                }
            }
        }
        return;
    }
    for py in d.y0..d.y1 {
        let start = (py - band_y0) as usize * w + d.x0 as usize;
        let (Some(src), Some(out)) = (g.buf.get(start..start + d.width()), dst.get_mut(start..start + d.width())) else { continue };
        for (o, s) in out.iter_mut().zip(src) {
            let sa = s[3] * alpha;
            if sa <= 0.0 {
                continue;
            }
            let inv = 1.0 - sa;
            o[0] = s[0] * alpha + o[0] * inv;
            o[1] = s[1] * alpha + o[1] * inv;
            o[2] = s[2] * alpha + o[2] * inv;
            o[3] = sa + o[3] * inv;
        }
    }
}

/// Source `s` over backdrop `b` (both premultiplied) in blend mode `mode`: the W3C
/// compositing formula `Cr = (1 − αb)·Cs + (1 − αs)·Cb + αs·αb·B(cb, cs)` for the separable
/// modes; Alpha and Erase keep or clear the backdrop by the source's opacity.
pub(crate) fn blend(mode: BlendMode, s: Premul, b: Premul) -> Premul {
    let (sa, ba) = (s[3].clamp(0.0, 1.0), b[3].clamp(0.0, 1.0));
    match mode {
        BlendMode::Alpha => return [b[0] * sa, b[1] * sa, b[2] * sa, ba * sa],
        BlendMode::Erase => {
            let k = 1.0 - sa;
            return [b[0] * k, b[1] * k, b[2] * k, ba * k];
        }
        _ => {}
    }
    let un = |c: f32, a: f32| if a > 1e-6 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
    let f = |cb: f32, cs: f32| -> f32 {
        let hard = |cb: f32, cs: f32| if cs <= 0.5 { cb * 2.0 * cs } else { let t = 2.0 * cs - 1.0; cb + t - cb * t };
        match mode {
            BlendMode::Multiply => cb * cs,
            BlendMode::Screen => cb + cs - cb * cs,
            BlendMode::Darken => cb.min(cs),
            BlendMode::Lighten => cb.max(cs),
            BlendMode::Difference => (cb - cs).abs(),
            BlendMode::Add => (cb + cs).min(1.0),
            BlendMode::Subtract => (cb - cs).max(0.0),
            BlendMode::Invert => 1.0 - cb,
            BlendMode::HardLight => hard(cb, cs),
            BlendMode::Overlay => hard(cs, cb),
            BlendMode::Normal | BlendMode::Layer | BlendMode::Alpha | BlendMode::Erase => cs,
        }
    };
    let mut out = [0.0; 4];
    for i in 0..3 {
        let (cs, cb) = (un(s[i], sa), un(b[i], ba));
        out[i] = (1.0 - ba) * s[i] + (1.0 - sa) * b[i] + sa * ba * f(cb, cs);
    }
    out[3] = sa + ba - sa * ba;
    out
}

/// Rows per parallel band.
const BAND_ROWS: usize = 32;

/// One fill operation, ready to rasterize: polygons in pixel space, its paint and bounds.
struct Prepared {
    polys: Vec<Vec<Vec2>>,
    rule: FillRule,
    shader: Shader,
    bounds: IRect,
    /// Drawn only inside this clip.
    clip: Option<u32>,
    /// Mask artwork for this clip (not drawn).
    masks: Option<u32>,
    /// Composited inside this group (and its parents).
    group: Option<u32>,
    /// A filtered layer's pixels instead of `shader` ([`Fill::layer`]; only in [`prepare_scene`]).
    layer: Option<u32>,
    /// A hairline ([`Fill::hairline`]).
    hairline: bool,
    /// The run of joined fills it is in, or 0 ([`Fill::run`]).
    run: u32,
}

/// Per item, the run of joined fills its fill is in, or 0 ([`Fill::run`]): an item that joins
/// the one before it ([`DisplayItem::joins`]) when both fills are drawn as fills, in the same
/// layer, clip and group, with nothing drawn between them (the earlier one has no stroke).
fn joined_runs(dl: &DisplayList) -> Vec<u32> {
    let mut out = vec![0u32; dl.items.len()];
    let plain = |i: &DisplayItem| i.fill.is_some() && i.outline.is_none() && !i.guide && i.warp.is_none();
    for (at, pair) in dl.items.windows(2).enumerate() {
        let [a, b] = pair else { continue };
        if !(b.joins && plain(a) && plain(b) && a.stroke.is_none() && a.layer == b.layer && a.clip == b.clip && a.group == b.group && a.masks == b.masks) {
            continue;
        }
        let Some(&before) = out.get(at) else { continue };
        // A new run is numbered by its second item, which is never item 0.
        let id = if before != 0 { before } else { u32::try_from(at + 1).unwrap_or(0) };
        for slot in out.iter_mut().skip(at).take(2) {
            *slot = id;
        }
    }
    out
}

/// Takes the run off a fill no neighbour shares it with (the others were culled, or draw
/// nothing): alone it is an ordinary fill.
fn drop_lone_runs(ops: &mut [Prepared]) {
    let ids: Vec<u32> = ops.iter().map(|o| o.run).collect();
    for (i, op) in ops.iter_mut().enumerate() {
        let same = |j: Option<usize>| j.and_then(|j| ids.get(j)).is_some_and(|&r| r == op.run);
        if op.run != 0 && !same(i.checked_sub(1)) && !same(i.checked_add(1)) {
            op.run = 0;
        }
    }
}

/// A run of joined fills in one band ([`Fill::run`]): its colour and how much of each pixel it
/// covers so far, laid out like the band's rows, and the part drawn into.
#[derive(Default)]
struct Run {
    id: u32,
    clip: Option<u32>,
    colour: Vec<Premul>,
    cover: Vec<f32>,
    dirty: Option<IRect>,
}

thread_local! {
    /// A band's run buffers, for the next band this thread draws.
    static RUN: std::cell::RefCell<Run> = std::cell::RefCell::default();
}

impl Run {
    /// Adds coverage `cov` (over `b`) of `op` to the run, in a band of `len` pixels, `w` to a
    /// row, whose first row is `band_y0`. With `under` (what the run will go onto, and the
    /// clip coverage it is drawn in), a pixel the fill covers whole where the run has nothing
    /// yet is drawn there at once: most pixels, which then cost what they do outside a run.
    fn add(&mut self, cov: &Coverage, op: &Prepared, b: IRect, (band_y0, w, len): (i32, usize, usize), c: &mut Vec<f32>, mut under: Option<(&mut [Premul], Option<&[f32]>)>) {
        if self.colour.len() < len {
            self.colour = vec![[0.0; 4]; len];
            self.cover = vec![0.0; len];
        }
        self.dirty = Some(self.dirty.map_or(b, |d| d.union(&b)));
        let bw = b.width();
        c.clear();
        c.resize(bw, 0.0);
        let solid = op.shader.is_solid();
        for ry in 0..b.height() {
            cov.resolve_row(ry, op.rule, c);
            let py = b.y0 + ry as i32;
            let start = (py - band_y0) as usize * w + b.x0 as usize;
            let (Some(colour), Some(cover)) = (self.colour.get_mut(start..start + bw), self.cover.get_mut(start..start + bw)) else { continue };
            // This row of what is under the run, and of its clip's coverage.
            let (mut below, mut clip) = (None, None);
            if let Some((dst, mask)) = under.as_mut() {
                below = dst.get_mut(start..start + bw);
                clip = mask.and_then(|m| m.get(start..start + bw));
                if mask.is_some() && clip.is_none() {
                    below = None;
                }
            }
            for (i, ((r, have), &k)) in colour.iter_mut().zip(cover.iter_mut()).zip(c.iter()).enumerate() {
                let k = k.min(1.0);
                if k <= 1e-4 {
                    continue;
                }
                let s = match solid {
                    Some(s) => s,
                    None => op.shader.at(b.x0 + i as i32, py),
                };
                if k >= 1.0 {
                    if *have <= 0.0
                        && let Some(d) = below.as_mut().and_then(|row| row.get_mut(i))
                    {
                        let m = clip.map_or(1.0, |m| m.get(i).copied().unwrap_or(0.0));
                        if m > 1e-4 {
                            let inv = 1.0 - s[3] * m;
                            *d = [s[0] * m + d[0] * inv, s[1] * m + d[1] * inv, s[2] * m + d[2] * inv, s[3] * m + d[3] * inv];
                        }
                        continue;
                    }
                    // Over what the run has there: src-over, which is what the lines below
                    // come to.
                    let keep = 1.0 - s[3];
                    *r = [r[0] * keep + s[0], r[1] * keep + s[1], r[2] * keep + s[2], r[3] * keep + s[3]];
                    *have = 1.0;
                    continue;
                }
                // Side by side as far as the pixel has room; what can't be lies over the run.
                let over = *have + k - 1.0;
                if over > 0.0 && *have > 0.0 {
                    let keep = 1.0 - over / *have * s[3];
                    *r = [r[0] * keep, r[1] * keep, r[2] * keep, r[3] * keep];
                }
                *r = [r[0] + s[0] * k, r[1] + s[1] * k, r[2] + s[2] * k, r[3] + s[3] * k];
                *have = (*have + k).min(1.0);
            }
        }
    }

    /// Ends the run: `each` gets every pixel drawn into (its index in the band, its colour and
    /// how much of it is covered), and the run is empty again.
    fn end(&mut self, band_y0: i32, w: usize, mut each: impl FnMut(usize, Premul, f32)) {
        self.id = 0;
        let Some(d) = self.dirty.take() else { return };
        for py in d.y0..d.y1 {
            let start = (py - band_y0) as usize * w + d.x0 as usize;
            let (Some(colour), Some(cover)) = (self.colour.get_mut(start..start + d.width()), self.cover.get_mut(start..start + d.width())) else { continue };
            for (i, (r, have)) in colour.iter_mut().zip(cover.iter_mut()).enumerate() {
                if *have > 0.0 {
                    each(start + i, *r, *have);
                    (*r, *have) = ([0.0; 4], 0.0);
                }
            }
        }
    }

    /// Ends the run src-over onto `dst` (the band's rows, or a group's), inside `mask`.
    fn flush(&mut self, band_y0: i32, w: usize, dst: &mut [Premul], mask: Option<&[f32]>) {
        self.end(band_y0, w, |at, s, _| {
            let m = match mask {
                Some(mask) => mask.get(at).copied().unwrap_or(0.0),
                None => 1.0,
            };
            let Some(d) = dst.get_mut(at) else { return };
            let inv = 1.0 - s[3] * m;
            *d = [s[0] * m + d[0] * inv, s[1] * m + d[1] * inv, s[2] * m + d[2] * inv, s[3] * m + d[3] * inv];
        });
    }
}

/// The clips of a display list: their mask artwork and where each can be non-zero.
struct Clips {
    parents: Vec<Option<u32>>,
    modes: Vec<MaskMode>,
    /// Mask artwork, grouped by clip.
    art: Vec<Vec<Prepared>>,
    bounds: Vec<IRect>,
}

const NO_PIXELS: IRect = IRect { x0: 0, y0: 0, x1: 0, y1: 0 };

impl Clips {
    fn new(parents: &[Option<u32>], modes: &[MaskMode], mask_ops: Vec<Prepared>) -> Clips {
        let n = parents.len();
        let modes = (0..n).map(|i| modes.get(i).copied().unwrap_or_default()).collect();
        let mut art: Vec<Vec<Prepared>> = (0..n).map(|_| Vec::new()).collect();
        for op in mask_ops {
            if let Some(a) = op.masks.and_then(|c| art.get_mut(c as usize)) {
                a.push(op);
            }
        }
        // A parent always precedes its children, so one pass in order intersects the chain.
        // A malformed parent link (not earlier in the list) masks everything out.
        let mut bounds: Vec<IRect> = Vec::with_capacity(n);
        let mut clean: Vec<Option<u32>> = Vec::with_capacity(n);
        for (i, a) in art.iter().enumerate() {
            let own = a.iter().fold(NO_PIXELS, |r, op| r.union(&op.bounds));
            let parent = parents.get(i).copied().flatten();
            let b = match parent {
                None => own,
                Some(p) => bounds.get(p as usize).filter(|_| (p as usize) < i).map_or(NO_PIXELS, |pb| own.intersect(pb)),
            };
            bounds.push(b);
            clean.push(parent.filter(|&p| (p as usize) < i));
        }
        Clips { parents: clean, modes, art, bounds }
    }

    fn bounds(&self, c: u32) -> IRect {
        self.bounds.get(c as usize).copied().unwrap_or(NO_PIXELS)
    }
}

/// Clip coverage for one band, computed on first use: `w × band height` values in `0..=1`.
#[derive(Default)]
struct BandClips {
    cov: Vec<Option<std::rc::Rc<Vec<f32>>>>,
}

impl BandClips {
    fn get(&mut self, clips: &Clips, c: u32, band: IRect, w: usize, cov: &mut Coverage, row: &mut Vec<f32>) -> std::rc::Rc<Vec<f32>> {
        let i = c as usize;
        if self.cov.len() < clips.parents.len() {
            self.cov.resize(clips.parents.len(), None);
        }
        if let Some(Some(done)) = self.cov.get(i) {
            return done.clone();
        }
        let parent = clips.parents.get(i).copied().flatten().map(|p| self.get(clips, p, band, w, cov, row));
        let mut buf = vec![0.0f32; w * band.height()];
        let mode = clips.modes.get(i).copied().unwrap_or_default();
        if mode.is_soft() {
            // A soft mask: its artwork painted onto transparent, then each pixel's opacity or
            // luminance.
            let mut px: Vec<Premul> = vec![[0.0; 4]; w * band.height()];
            let mut run = Run::default();
            for op in clips.art.get(i).into_iter().flatten() {
                let b = op.bounds.intersect(&band);
                if b.is_empty() {
                    continue;
                }
                if run.id != 0 && run.id != op.run {
                    run.flush(band.y0, w, &mut px, None);
                }
                cov.reset(b);
                for poly in &op.polys {
                    cov.add_polygon(poly);
                }
                if op.run != 0 {
                    run.id = op.run;
                    run.add(cov, op, b, (band.y0, w, px.len()), row, None);
                    continue;
                }
                composite(cov, op, b, band.y0, w, &mut px, row, None);
            }
            run.flush(band.y0, w, &mut px, None);
            for (d, p) in buf.iter_mut().zip(&px) {
                *d = mode.reveal(*p);
            }
        }
        // Union of the artwork's coverage (a ∪ b = a + b − ab), anti-aliased like any fill; a
        // run of joined fills (ADR 0138) unites as one, with what it covers together.
        let mut run = Run::default();
        let unite = |buf: &mut [f32], at: usize, k: f32| {
            if let Some(d) = buf.get_mut(at) {
                *d = *d + k - *d * k;
            }
        };
        for op in clips.art.get(i).into_iter().flatten().filter(|_| !mode.is_soft()) {
            let b = op.bounds.intersect(&band);
            if b.is_empty() {
                continue;
            }
            if run.id != 0 && run.id != op.run {
                run.end(band.y0, w, |at, _, k| unite(&mut buf, at, k));
            }
            cov.reset(b);
            for poly in &op.polys {
                cov.add_polygon(poly);
            }
            if op.run != 0 {
                run.id = op.run;
                run.add(cov, op, b, (band.y0, w, buf.len()), row, None);
                continue;
            }
            row.clear();
            row.resize(b.width(), 0.0);
            for ry in 0..b.height() {
                cov.resolve_row(ry, op.rule, row);
                let start = (b.y0 + ry as i32 - band.y0) as usize * w + b.x0 as usize;
                let Some(dst) = buf.get_mut(start..start + b.width()) else { continue };
                for (d, &k) in dst.iter_mut().zip(row.iter()) {
                    let k = k.clamp(0.0, 1.0);
                    *d = *d + k - *d * k;
                }
            }
        }
        run.end(band.y0, w, |at, _, k| unite(&mut buf, at, k));
        if let Some(p) = parent {
            for (d, m) in buf.iter_mut().zip(p.iter()) {
                *d *= m;
            }
        }
        let buf = std::rc::Rc::new(buf);
        if let Some(slot) = self.cov.get_mut(i) {
            *slot = Some(buf.clone());
        }
        buf
    }
}

/// Whether `item` is artwork of a soft mask (drawn with its paint, ADR 0070).
fn soft_art(dl: &DisplayList, item: &DisplayItem) -> bool {
    item.masks.is_some_and(|c| dl.clip_mode(c).is_soft())
}

/// Whether every point of `pts` is in one of the three places where an edge between two of them
/// adds nothing to any pixel of `surface`: above it, below it, or to its right (ADR 0132).
///
/// The rasterizer skips an edge above or below the rows it fills, and what an edge at or past
/// the right end of a row deposits falls in cells no pixel reads. An edge to the *left* does
/// count (it carries the winding into the row), so the left is not one of the places. Above is
/// exact at the surface's first row; below and right start `far` pixels out, for a caller that
/// asks about points it has not computed (a curve's hull: its points are sums of the hull's,
/// rounded).
fn unseen(pts: &[Vec2], surface: IRect, far: f32) -> bool {
    let (top, bottom, right) = (surface.y0 as f32, surface.y1 as f32 + far, surface.x1 as f32 + far);
    pts.iter().all(|p| p.y <= top) || pts.iter().all(|p| p.y >= bottom) || pts.iter().all(|p| p.x >= right)
}

/// How far past the surface's bottom and right a curve's hull must be for the curve to be left
/// as its chord: more than the rounding of its points' sums could be (a few parts in ten
/// million of the coordinates, which is under a pixel at any surface size the renderer takes).
fn far(surface: IRect) -> f32 {
    1.0 + surface.x1.max(surface.y1) as f32 * 1e-4
}

/// Whether the end points of `path`'s segments under `m` reach all four sides of `surface` or
/// beyond: then its pixel bounds are the whole surface whatever its curves do in between, and
/// the curves off the surface may be left as chords without moving them.
fn spans(path: &anicraft_geom::Path, m: &Affine, surface: IRect) -> bool {
    use anicraft_geom::PathCmd;
    let mut r = Rect::EMPTY;
    let (mut start, mut last) = (Vec2::ZERO, Vec2::ZERO);
    for c in &path.cmds {
        match *c {
            PathCmd::MoveTo(p) => {
                start = m.apply(p);
                last = start;
            }
            PathCmd::LineTo(p) | PathCmd::QuadTo(_, p) | PathCmd::CubicTo(_, _, p) => {
                let p = m.apply(p);
                // Both ends of a drawn segment are among the flattened points.
                if last.is_finite() && p.is_finite() {
                    r.include(last);
                    r.include(p);
                }
                last = p;
            }
            PathCmd::Close => last = start,
        }
    }
    r.min.x <= surface.x0 as f32 && r.min.y <= surface.y0 as f32 && r.max.x >= surface.x1 as f32 && r.max.y >= surface.y1 as f32
}

/// Takes out of each closed polygon the points between two neighbours when all three are
/// above, below or to the right of `surface` ([`unseen`]): the two edges at such a point add
/// nothing to any pixel, and neither does the one edge that replaces them, so the pixels are
/// the same to the last bit with fewer edges to walk. One place at a time: a chord between two
/// points above the surface is above it, but not one from above it to its right.
fn drop_unseen(polys: &mut [Vec<Vec2>], surface: IRect) {
    let (top, bottom, right) = (surface.y0 as f32, surface.y1 as f32, surface.x1 as f32);
    let places: [&dyn Fn(&Vec2) -> bool; 3] = [&|p| p.y <= top, &|p| p.y >= bottom, &|p| p.x >= right];
    let mut inside: Vec<bool> = Vec::new();
    for poly in polys.iter_mut() {
        for place in places {
            inside.clear();
            inside.extend(poly.iter().map(place));
            if !inside.windows(3).any(|w| w.iter().all(|b| *b)) {
                continue;
            }
            let mut i = 0usize;
            poly.retain(|_| {
                let at = i;
                i += 1;
                !(at > 0 && inside.get(at - 1..at + 2).is_some_and(|w| w.iter().all(|b| *b)))
            });
        }
    }
}

/// Turns a display item into fill operations (fill, then stroke), or none when culled. `soft`:
/// the item is a soft mask's artwork, prepared with its paints like any artwork (still marked
/// as mask artwork). `cut`: what can't reach a pixel is left out ([`unseen`]); off, the item is
/// prepared whole, for the tests that show the pixels are the same.
fn prepare(item: &DisplayItem, view: &Affine, surface: IRect, soft: bool, cut: bool, run: u32) -> Vec<Prepared> {
    let mut out = Vec::new();
    let m = view.mul(&item.transform);
    if !m.is_finite() {
        return out;
    }
    // Guide artwork is for editing only: it never renders.
    if item.guide {
        return out;
    }
    // Mask artwork is its fill area only: no strokes, no opacity (as animators expect); a soft
    // mask's artwork is drawn as it looks.
    let is_mask = item.masks.is_some() && !soft;
    if is_mask && item.fill.is_none() {
        return out;
    }
    // Cull before flattening: control-point bounds are a cheap, conservative superset.
    let pad = if is_mask { 1.0 } else { item.stroke.as_ref().map_or(0.0, anicraft_doc::Stroke::reach) + 1.0 };
    let bounds = item.path.bounds().inflate(pad).transformed(&m);
    let surface_rect = Rect::new(0.0, 0.0, surface.x1 as f32, surface.y1 as f32);
    if bounds.is_empty() || bounds.intersect(&surface_rect).is_empty() {
        return out;
    }
    // Zoomed far into a large filled shape (a drawing merged from many brush strokes), nearly
    // all of its curves are off the surface: those above, below or right of it become one line
    // each instead of being cut into pieces nobody sees (ADR 0132). Only where that provably
    // changes nothing: a fill alone (a stroke's dashes and width follow the length along the
    // whole line), not a hairline, and a path that spans the surface (see `spans`).
    let outlined = item.outline.is_some() && item.masks.is_none();
    let filled_only = !outlined && (is_mask || item.stroke.is_none());
    let lines = if cut && filled_only && spans(&item.path, &m, surface) {
        let far = far(surface);
        item.path.flatten_with(&m, TOLERANCE, |hull| hull.iter().all(|p| p.is_finite()) && unseen(hull, surface, far))
    } else {
        item.path.flatten(&m, TOLERANCE)
    };
    if lines.is_empty() {
        return out;
    }
    let (clip, masks, group) = (item.clip, item.masks, item.group);
    // (Mask artwork is never outlined: it draws nothing of its own.)
    if let Some(color) = item.outline.filter(|_| item.masks.is_none()) {
        // Outline mode: a hairline in the layer colour, no fill (ADR 0048).
        let shader = Shader::Solid(premultiply(color));
        if shader.is_solid().is_some_and(|c| c[3] <= 0.0) {
            return out;
        }
        let polys: Vec<Vec<Vec2>> = lines
            .into_iter()
            .map(|l| {
                let mut pts = l.points;
                if l.closed
                    && let Some(&first) = pts.first()
                {
                    pts.push(first);
                }
                pts
            })
            .collect();
        if let Some(b) = hairline::bounds(&polys, surface) {
            out.push(Prepared { polys, rule: FillRule::NonZero, shader, bounds: b, clip, masks, group, layer: None, hairline: true, run: 0 });
        }
        return out;
    }
    let mut push = |mut polys: Vec<Vec<Vec2>>, rule, shader: Shader, run: u32| {
        if shader.is_solid().is_some_and(|c| c[3] <= 0.0) {
            return;
        }
        // The bounds first, from every point: they place the coverage buffer.
        if let Some((b, beyond)) = pixel_bounds_of(&polys, surface) {
            if cut && beyond {
                drop_unseen(&mut polys, surface);
            }
            out.push(Prepared { polys, rule, shader, bounds: b, clip, masks, group, layer: None, hairline: false, run });
        }
    };

    if is_mask {
        push(lines.iter().map(|l| l.points.clone()).collect(), item.fill_rule, Shader::Solid([1.0; 4]), run);
        return out;
    }
    // The stroke is outlined from the flattened lines first, so the fill can then take them
    // without a copy; it is still drawn after the fill.
    let stroke = item.stroke.as_ref().and_then(|st| {
        // Caps, joins, dashes (ADR 0046) and width profiles (ADR 0018); thin strokes and
        // sub-pixel dash patterns fade instead of dropping out.
        let (polys, alpha) = stroke::stroke_geometry(&lines, st, &m, true)?;
        let fx = ColorTransform::alpha(alpha).concat(&item.color);
        Some((polys, Shader::new(&Paint::Solid(st.color), &m, &fx, None)))
    });
    if let Some(fill) = &item.fill {
        let polys: Vec<Vec<Vec2>> = lines.into_iter().map(|l| l.points).collect();
        let shader = match (&item.warp, fill, &item.image) {
            // A warped bitmap (ADR 0079): the image drawn through its mesh, prepared once over
            // the fill's pixels and shown one image pixel per surface pixel.
            (Some(mesh), Paint::Bitmap { repeat, matrix, .. }, Some(px)) => {
                let prepared = pixel_bounds(&polys, surface).and_then(|b| Some((warp::texture(mesh, &m, matrix, *repeat, px, item.smooth, b)?, b)));
                match prepared {
                    Some((tex, b)) => Shader::Bitmap { inv: Affine::translate(-(b.x0 as f32), -(b.y0 as f32)), pixels: tex, repeat: false, fx: item.color, smooth: false },
                    None => Shader::Solid([0.0; 4]),
                }
            }
            _ => Shader::new(fill, &m, &item.color, item.image.as_ref()).smoothed(item.smooth),
        };
        push(polys, item.fill_rule, shader, run);
    }
    if let Some((polys, shader)) = stroke {
        push(polys, FillRule::NonZero, shader, 0);
    }
    out
}

fn pixel_bounds(polys: &[Vec<Vec2>], surface: IRect) -> Option<IRect> {
    pixel_bounds_of(polys, surface).map(|(b, _)| b)
}

/// The pixels of `surface` that `polys` can touch, and whether they reach above, below or
/// right of it (then some of their points may be [`unseen`]).
fn pixel_bounds_of(polys: &[Vec<Vec2>], surface: IRect) -> Option<(IRect, bool)> {
    let mut r = Rect::EMPTY;
    for p in polys.iter().flatten() {
        r.include(*p);
    }
    if !(r.max.x >= r.min.x && r.max.y >= r.min.y) {
        return None; // no points
    }
    let b = IRect {
        x0: (r.min.x.floor().max(surface.x0 as f32)) as i32,
        y0: (r.min.y.floor().max(surface.y0 as f32)) as i32,
        x1: (r.max.x.ceil().min(surface.x1 as f32)) as i32,
        y1: (r.max.y.ceil().min(surface.y1 as f32)) as i32,
    };
    let beyond = r.min.y <= surface.y0 as f32 || r.max.y >= surface.y1 as f32 || r.max.x >= surface.x1 as f32;
    (!b.is_empty()).then_some((b, beyond))
}

/// Composites coverage `cov` (over `b`) of `op` into the band whose first row is `band_y0`,
/// scaled by the band's clip coverage `mask` when the op is clipped.
#[allow(clippy::too_many_arguments)]
fn composite(cov: &Coverage, op: &Prepared, b: IRect, band_y0: i32, w: usize, rows: &mut [Premul], c: &mut Vec<f32>, mask: Option<&[f32]>) {
    let bw = b.width();
    c.clear();
    c.resize(bw, 0.0);
    let solid = op.shader.is_solid();
    for ry in 0..b.height() {
        cov.resolve_row(ry, op.rule, c);
        let py = b.y0 + ry as i32;
        let start = (py - band_y0) as usize * w + b.x0 as usize;
        if let Some(mask) = mask {
            let Some(m) = mask.get(start..start + bw) else { continue };
            for (k, m) in c.iter_mut().zip(m) {
                *k *= m;
            }
        }
        let Some(dst) = rows.get_mut(start..start + bw) else { continue };
        for (i, (d, &k)) in dst.iter_mut().zip(c.iter()).enumerate() {
            if k <= 1e-4 {
                continue;
            }
            let s = match solid {
                Some(s) => s,
                None => op.shader.at(b.x0 + i as i32, py),
            };
            let sa = s[3] * k;
            let inv = 1.0 - sa;
            d[0] = s[0] * k + d[0] * inv;
            d[1] = s[1] * k + d[1] * inv;
            d[2] = s[2] * k + d[2] * inv;
            d[3] = sa + d[3] * inv;
        }
    }
}

#[cfg(test)]
mod nine_slice_tests;

#[cfg(test)]
mod layer_opacity_tests;
#[cfg(test)]
mod cut_tests;
#[cfg(test)]
mod join_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_doc::{ElementId, LayerId, sample};
    use anicraft_geom::Path;
    use std::sync::Arc;

    fn item(path: Path, fill: Paint) -> DisplayItem {
        DisplayItem {
            joins: false,
            path: Arc::new(path),
            transform: Affine::IDENTITY,
            fill: Some(fill),
            stroke: None,
            fill_rule: FillRule::NonZero,
            color: ColorTransform::IDENTITY,
            layer: LayerId(0),
            element: ElementId(0),
            outline: None,
            clip: None,
            masks: None,
            guide: false,
            group: None,
            image: None,
            smooth: true,
            warp: None,
        }
    }

    fn list(items: Vec<DisplayItem>) -> DisplayList {
        DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items, clip_parents: Vec::new(), groups: Vec::new() }
    }

    /// A 40 × 40 opaque blue square masked by `art` in `mode`, on transparent.
    fn soft(mode: MaskMode, art: Vec<DisplayItem>) -> Pixmap {
        let mut square = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)));
        square.clip = Some(0);
        let mut items = vec![square];
        items.extend(art.into_iter().map(|mut a| {
            a.masks = Some(0);
            a
        }));
        let mut dl = DisplayList { items, clip_parents: vec![None], ..Default::default() };
        dl.set_clip_mode(0, mode);
        render(&dl, &view(40, 40)).unwrap().0
    }

    #[test]
    fn soft_masks_reveal_by_opacity_or_luminance() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let half_red = || item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::new(1.0, 0.0, 0.0, 0.5)));
        // Alpha: the artwork's opacity is the coverage; its colour doesn't matter.
        let pm = soft(MaskMode::Alpha, vec![half_red()]);
        assert!(close(pm.pixel(20, 20).unwrap()[3], 0.5) && close(pm.pixel(20, 20).unwrap()[2], 0.5), "{:?}", pm.pixel(20, 20));
        // Coverage (today's masks): the same artwork reveals fully.
        assert_eq!(soft(MaskMode::Coverage, vec![half_red()]).pixel(20, 20).unwrap(), [0.0, 0.0, 1.0, 1.0]);
        // Luminance: 0.2125 r + 0.7154 g + 0.0721 b, times the opacity.
        let pm = soft(MaskMode::Luminance, vec![half_red()]);
        assert!(close(pm.pixel(20, 20).unwrap()[3], 0.2125 * 0.5), "{:?}", pm.pixel(20, 20));
        for (c, want) in [(Rgba::WHITE, 1.0), (Rgba::BLACK, 0.0), (Rgba::new(0.0, 1.0, 0.0, 1.0), 0.7154), (Rgba::new(0.5, 0.5, 0.5, 1.0), 0.5)] {
            let pm = soft(MaskMode::Luminance, vec![item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(c))]);
            assert!(close(pm.pixel(5, 5).unwrap()[3], want), "{c:?}: {:?}", pm.pixel(5, 5));
        }
        // Overlapping artwork composites (src-over) before it masks: two 0.5 layers give 0.75.
        let pm = soft(MaskMode::Alpha, vec![half_red(), half_red()]);
        assert!(close(pm.pixel(20, 20).unwrap()[3], 0.75));
        // The artwork's colour effect (an instance's alpha) counts.
        let mut faded = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::WHITE));
        faded.color = ColorTransform::alpha(0.25);
        assert!(close(soft(MaskMode::Alpha, vec![faded]).pixel(20, 20).unwrap()[3], 0.25));
        // Strokes count in a soft mask (not in a coverage one).
        let mut ring = item(Path::rect(10.0, 10.0, 20.0, 20.0), Paint::Solid(Rgba::TRANSPARENT));
        ring.stroke = Some(anicraft_doc::Stroke::new(Rgba::WHITE, 4.0));
        assert!(close(soft(MaskMode::Alpha, vec![ring.clone()]).pixel(10, 20).unwrap()[3], 1.0));
        assert_eq!(soft(MaskMode::Coverage, vec![ring]).pixel(10, 1).unwrap()[3], 0.0);
        // Outside the artwork nothing shows.
        let small = item(Path::rect(0.0, 0.0, 10.0, 10.0), Paint::Solid(Rgba::WHITE));
        assert_eq!(soft(MaskMode::Alpha, vec![small]).pixel(30, 30).unwrap()[3], 0.0);
    }

    #[test]
    fn an_alpha_gradient_mask_fades_linearly() {
        // Transparent at x = 0 to opaque at x = 40: pixel x reveals (x + 0.5) / 40.
        let stops = vec![anicraft_doc::GradientStop { offset: 0.0, color: Rgba::new(1.0, 1.0, 1.0, 0.0) }, anicraft_doc::GradientStop { offset: 1.0, color: Rgba::WHITE }];
        let grad = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::linear(Vec2::new(0.0, 0.0), Vec2::new(40.0, 0.0), stops));
        let pm = soft(MaskMode::Alpha, vec![grad.clone()]);
        for x in [0u32, 10, 20, 30, 39] {
            let a = pm.pixel(x, 20).unwrap()[3];
            // The gradient table has 256 steps: within one step.
            assert!((a - (x as f32 + 0.5) / 40.0).abs() < 1.0 / 255.0 + 1e-3, "x {x}: {a}");
        }
        // White to transparent as luminance: the same ramp (white's luminance is 1).
        let lum = soft(MaskMode::Luminance, vec![grad]);
        assert!(pm.data.iter().zip(&lum.data).all(|(a, b)| (a[3] - b[3]).abs() < 1e-4));
    }

    #[test]
    fn a_soft_mask_inside_another_mask_multiplies() {
        // Clip 1 (alpha, 0.5) inside clip 0 (coverage, left half): right half hidden, left at 0.5.
        let mut square = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)));
        square.clip = Some(1);
        let mut outer = item(Path::rect(0.0, 0.0, 20.0, 40.0), Paint::Solid(Rgba::WHITE));
        outer.masks = Some(0);
        let mut inner = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::new(1.0, 1.0, 1.0, 0.5)));
        inner.masks = Some(1);
        inner.clip = Some(0);
        let mut dl = DisplayList { items: vec![square, outer, inner], clip_parents: vec![None, Some(0)], ..Default::default() };
        dl.set_clip_mode(1, MaskMode::Alpha);
        let (pm, _) = render(&dl, &view(40, 40)).unwrap();
        assert!((pm.pixel(5, 5).unwrap()[3] - 0.5).abs() < 1e-4);
        assert_eq!(pm.pixel(30, 5).unwrap()[3], 0.0);
        // And the scene a GPU renderer gets carries the mode.
        let (scene, _) = prepare_scene(&dl, &view(40, 40)).unwrap();
        assert_eq!(scene.clips.iter().map(|c| c.mode).collect::<Vec<_>>(), vec![MaskMode::Coverage, MaskMode::Alpha]);
    }

    #[test]
    fn mask_reveals_only_its_fill_area() {
        // A 40 × 40 square masked by a circle of radius 10: only the circle's area shows, in
        // the square's colour; the mask's own colour, stroke and opacity play no part.
        let mut square = item(Path::rect(0.0, 0.0, 40.0, 40.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)));
        square.clip = Some(0);
        let mut mask = item(Path::ellipse(20.0, 20.0, 10.0, 10.0), Paint::Solid(Rgba::new(1.0, 0.0, 0.0, 0.2)));
        mask.masks = Some(0);
        mask.stroke = Some(anicraft_doc::Stroke::new(Rgba::BLACK, 8.0));
        let dl = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![square, mask], clip_parents: vec![None], groups: Vec::new() };
        let (pm, _) = render(&dl, &view(40, 40)).unwrap();
        // Pixel for pixel, the same as the circle drawn in the square's colour.
        let circle = item(Path::ellipse(20.0, 20.0, 10.0, 10.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)));
        let (plain, _) = render(&list(vec![circle]), &view(40, 40)).unwrap();
        assert!(pm.data.iter().zip(&plain.data).all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)));
        let area = alpha_sum(&plain);
        assert_eq!(pm.pixel(20, 20).unwrap(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(pm.pixel(2, 2).unwrap()[3], 0.0);
        // Two overlapping mask shapes reveal their union, not a cancelled overlap.
        let mut dl2 = dl.clone();
        let mut m2 = item(Path::rect(20.0, 0.0, 20.0, 40.0), Paint::Solid(Rgba::WHITE));
        m2.masks = Some(0);
        dl2.items.push(m2);
        let (pm2, _) = render(&dl2, &view(40, 40)).unwrap();
        assert_eq!(pm2.pixel(25, 20).unwrap()[3], 1.0);
        assert_eq!(pm2.pixel(35, 2).unwrap()[3], 1.0);
        assert!((alpha_sum(&pm2) - (800.0 + area / 2.0)).abs() < 8.0, "{}", alpha_sum(&pm2));
    }

    #[test]
    fn colour_transforms_apply_to_solids_gradients_and_bitmaps_alike() {
        // ADR 0056: each straight channel c·mul + add, clamped, on every kind of paint.
        let fx = ColorTransform { mul: [-1.0, 0.5, 1.0, 0.8], add: [1.0, 0.25, -0.5, 0.0] };
        let want = |c: Rgba| {
            let o = fx.apply(c);
            [o.r * o.a, o.g * o.a, o.b * o.a, o.a]
        };
        let near = |p: [f32; 4], q: [f32; 4]| p.iter().zip(q).all(|(a, b)| (a - b).abs() < 2e-3);
        let c = Rgba::new(0.2, 0.6, 0.9, 1.0);
        // Solid.
        let mut solid = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Solid(c));
        solid.color = fx;
        let (pm, _) = render(&list(vec![solid]), &view(4, 4)).unwrap();
        assert!(near(pm.pixel(2, 2).unwrap(), want(c)), "{:?} vs {:?}", pm.pixel(2, 2), want(c));
        // A two-stop gradient of one colour is that colour, through the transform.
        let stops = vec![anicraft_doc::GradientStop { offset: 0.0, color: c }, anicraft_doc::GradientStop { offset: 1.0, color: c }];
        let mut grad = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::linear(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), stops));
        grad.color = fx;
        let (pm, _) = render(&list(vec![grad]), &view(4, 4)).unwrap();
        assert!(near(pm.pixel(1, 2).unwrap(), want(c)));
        // A bitmap of one colour, sampled anywhere.
        let px = Arc::new(anicraft_doc::Pixels { width: 2, height: 2, rgba: [51u8, 153, 230, 255].repeat(4) });
        let b = Rgba::new(51.0 / 255.0, 153.0 / 255.0, 230.0 / 255.0, 1.0);
        let mut bmp = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Bitmap { bitmap: anicraft_doc::BitmapId(1), repeat: true, matrix: Affine::IDENTITY });
        bmp.image = Some(px);
        bmp.color = fx;
        let (pm, _) = render(&list(vec![bmp]), &view(4, 4)).unwrap();
        assert!(near(pm.pixel(1, 1).unwrap(), want(b)), "{:?} vs {:?}", pm.pixel(1, 1), want(b));
        // Nested transforms compose exactly: drawing through the composition is drawing through
        // one then the other.
        let inner = ColorTransform::tint(Rgba::new(1.0, 0.0, 0.0, 1.0), 0.5);
        let outer = ColorTransform::brightness(-0.4);
        let mut nested = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Solid(c));
        nested.color = outer.concat(&inner);
        let (pm, _) = render(&list(vec![nested]), &view(4, 4)).unwrap();
        let o = outer.apply(inner.apply(c));
        assert!(near(pm.pixel(2, 2).unwrap(), [o.r, o.g, o.b, 1.0]));
    }

    #[test]
    fn groups_fade_as_one() {
        // A red square under a blue one, overlapping, in a group at 50% over white.
        let white = View { background: Some(Rgba::WHITE), ..view(12, 4) };
        let mut red = item(Path::rect(0.0, 0.0, 8.0, 4.0), Paint::Solid(Rgba::new(1.0, 0.0, 0.0, 1.0)));
        let mut blue = item(Path::rect(4.0, 0.0, 8.0, 4.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 1.0)));
        red.group = Some(0);
        blue.group = Some(0);
        let grouped = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![red.clone(), blue.clone()], clip_parents: vec![], groups: vec![anicraft_doc::Group { parent: None, alpha: 0.5, blend: Default::default(), filters: Default::default(), clip: None }] };
        let (pm, _) = render(&grouped, &white).unwrap();
        let near = |p: Premul, q: [f32; 4]| p.iter().zip(q).all(|(a, b)| (a - b).abs() < 1e-4);
        // In the overlap only the top square shows through: half blue, half white, no red.
        assert!(near(pm.pixel(6, 1).unwrap(), [0.5, 0.5, 1.0, 1.0]), "{:?}", pm.pixel(6, 1));
        assert!(near(pm.pixel(1, 1).unwrap(), [1.0, 0.5, 0.5, 1.0]));
        // Fading each square on its own would let the red show through the blue.
        let (mut r2, mut b2) = (red, blue);
        (r2.group, b2.group) = (None, None);
        r2.color = ColorTransform::alpha(0.5);
        b2.color = ColorTransform::alpha(0.5);
        let (each, _) = render(&list(vec![r2, b2]), &white).unwrap();
        assert!(each.pixel(6, 1).unwrap()[0] > 0.2, "per-shape fading mixes in red");
        // Nested groups multiply; a group with a bad parent link is a top-level group.
        let mut inner = item(Path::rect(0.0, 0.0, 12.0, 4.0), Paint::Solid(Rgba::BLACK));
        inner.group = Some(1);
        let nested = DisplayList { left_out: false, links: Vec::new(),
            clip_modes: Vec::new(),
            items: vec![inner],
            clip_parents: vec![],
            groups: vec![anicraft_doc::Group { parent: None, alpha: 0.5, blend: Default::default(), filters: Default::default(), clip: None }, anicraft_doc::Group { parent: Some(0), alpha: 0.5, blend: Default::default(), filters: Default::default(), clip: None }],
        };
        let (n, _) = render(&nested, &white).unwrap();
        assert!(near(n.pixel(3, 2).unwrap(), [0.75, 0.75, 0.75, 1.0]), "{:?}", n.pixel(3, 2));
        let mut bad = nested.clone();
        bad.groups[1].parent = Some(7);
        let (b, _) = render(&bad, &white).unwrap();
        assert!(near(b.pixel(3, 2).unwrap(), [0.5, 0.5, 0.5, 1.0]));
        // Spanning several render bands changes nothing.
        let mut tall = item(Path::rect(0.0, 0.0, 4.0, 100.0), Paint::Solid(Rgba::BLACK));
        tall.group = Some(0);
        let (t, _) = render(&DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![tall], clip_parents: vec![], groups: vec![anicraft_doc::Group { parent: None, alpha: 0.25, blend: Default::default(), filters: Default::default(), clip: None }] }, &View { background: Some(Rgba::WHITE), ..view(4, 100) }).unwrap();
        assert!((0..100).all(|y| near(t.pixel(1, y).unwrap(), [0.75, 0.75, 0.75, 1.0])));
    }

    #[test]
    fn prepared_scenes_match_the_renderers_preparation_and_name_what_needs_more() {
        // A filled, stroked square and an off-screen circle: fill then stroke, one culled.
        let mut sq = item(Path::rect(2.0, 2.0, 8.0, 8.0), Paint::Solid(Rgba::BLACK));
        sq.stroke = Some(anicraft_doc::Stroke::new(Rgba::WHITE, 2.0));
        let off = item(Path::ellipse(500.0, 500.0, 10.0, 10.0), Paint::Solid(Rgba::BLACK));
        let dl = list(vec![sq.clone(), off]);
        let (scene, st) = prepare_scene(&dl, &view(16, 16)).unwrap();
        let (_, rst) = render(&dl, &view(16, 16)).unwrap();
        let fills = &scene.fills;
        assert_eq!(fills.len(), 2);
        assert_eq!((st.drawn, st.culled, st.edges), (rst.drawn, rst.culled, rst.edges));
        assert!(fills[0].shader.is_solid() == Some([0.0, 0.0, 0.0, 1.0]) && fills[1].shader.is_solid() == Some([1.0; 4]));
        assert!(fills.iter().all(|f| f.bounds.x0 >= 0 && f.bounds.x1 <= 16 && f.clip.is_none() && f.group.is_none()));
        // Clips: the mask artwork is the clip's, not a fill; a clipped fill is cut to the
        // clip's bounds; a nested clip's bounds are inside its parent's; a clip whose parent
        // link is bad is empty, and what it clips is culled, as `render` does.
        let mut clipped = sq.clone();
        clipped.stroke = None;
        clipped.clip = Some(1);
        let mut outer = item(Path::rect(0.0, 0.0, 6.0, 16.0), Paint::Solid(Rgba::WHITE));
        outer.masks = Some(0);
        let mut inner = item(Path::rect(4.0, 0.0, 12.0, 16.0), Paint::Solid(Rgba::WHITE));
        inner.masks = Some(1);
        let mut lost = clipped.clone();
        lost.clip = Some(2);
        let dl = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![clipped, outer, inner, lost], clip_parents: vec![None, Some(0), Some(9)], groups: Vec::new() };
        let (scene, st) = prepare_scene(&dl, &view(16, 16)).unwrap();
        let (_, rst) = render(&dl, &view(16, 16)).unwrap();
        assert_eq!((st.drawn, st.edges, st.pixels_touched), (rst.drawn, rst.edges, rst.pixels_touched));
        assert_eq!(scene.fills.len(), 1);
        assert_eq!((scene.fills[0].clip, scene.fills[0].bounds), (Some(1), raster::IRect { x0: 4, y0: 2, x1: 6, y1: 10 }));
        assert_eq!(scene.clips.len(), 3);
        assert_eq!((scene.clips[1].parent, scene.clips[1].art.len(), scene.clips[1].bounds), (Some(0), 1, raster::IRect { x0: 4, y0: 0, x1: 6, y1: 16 }));
        assert!(scene.clips[2].parent.is_none() && scene.clips[2].bounds.is_empty());
        // Groups: chains outermost first, alpha sanitized.
        let mut grouped = sq.clone();
        grouped.group = Some(1);
        let g = |parent, alpha| anicraft_doc::Group { parent, alpha, blend: BlendMode::Multiply, filters: Default::default(), clip: None };
        let dl = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![grouped], clip_parents: vec![], groups: vec![g(None, 2.0), g(Some(0), f32::NAN)] };
        let (scene, _) = prepare_scene(&dl, &view(16, 16)).unwrap();
        assert!(scene.fills.iter().all(|f| f.group == Some(1)));
        assert_eq!((scene.groups[1].chain.clone(), scene.groups[0].alpha, scene.groups[1].alpha, scene.groups[1].blend), (vec![0, 1], 1.0, 1.0, BlendMode::Multiply));
        // A filtered group is one layer, shown by one fill in the outermost filtered group, its
        // filters resolved for the view; drawing the layer and filtering it is what `render`
        // composites there.
        let mut filtered = dl.clone();
        filtered.groups[0].filters = std::sync::Arc::from(vec![anicraft_doc::Filter::Blur { blur_x: 4.0, blur_y: 4.0, quality: 1 }]);
        let (scene, st) = prepare_scene(&filtered, &view(16, 16)).unwrap();
        let (pm, rst) = render(&filtered, &view(16, 16)).unwrap();
        assert_eq!((st.drawn, st.culled), (rst.drawn, rst.culled));
        assert_eq!(scene.layers.len(), 1);
        let (f, l) = (&scene.fills[0], &scene.layers[0]);
        assert_eq!((scene.fills.len(), f.layer, f.group, f.bounds), (1, Some(0), Some(0), raster::IRect { x0: 0, y0: 0, x1: 16, y1: 16 }));
        assert_eq!(l.filters, vec![filters::FilterOp::Blur(filters::BoxBlur { nx: 4, ny: 4, passes: 1 })]);
        assert_eq!((l.view.width, l.view.height), (l.bounds.width() as u32, l.bounds.height() as u32));
        let (mut layer, _) = render(&l.list, &l.view).unwrap();
        filters::apply(&mut layer.data, l.bounds.width(), l.bounds.height(), &filtered.groups[0].filters, 1.0);
        let at = |x: i32, y: i32| layer.data[((y - l.bounds.y0) * l.bounds.width() as i32 + x - l.bounds.x0) as usize];
        assert_eq!(pm.pixel(5, 6).unwrap(), at(5, 6), "the group is opaque and normal: the layer as it is");
        // Bitmap fills come as bitmap shaders.
        let px = std::sync::Arc::new(anicraft_doc::Pixels { width: 1, height: 1, rgba: vec![255; 4] });
        let mut bm = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Bitmap { bitmap: anicraft_doc::BitmapId(1), repeat: false, matrix: Affine::IDENTITY });
        bm.image = Some(px);
        let (scene, _) = prepare_scene(&list(vec![bm]), &view(16, 16)).unwrap();
        assert!(matches!(scene.fills[0].shader, Shader::Bitmap { .. }) && scene.layers.is_empty());
    }

    #[test]
    fn guide_artwork_never_renders() {
        let mut g = item(Path::rect(0.0, 0.0, 8.0, 8.0), Paint::Solid(Rgba::BLACK));
        g.guide = true;
        let (pm, st) = render(&list(vec![g]), &view(8, 8)).unwrap();
        assert_eq!((alpha_sum(&pm), st.drawn), (0.0, 0));
    }

    #[test]
    fn empty_mask_hides_everything_and_unclipped_items_are_untouched() {
        let mut clipped = item(Path::rect(0.0, 0.0, 8.0, 8.0), Paint::Solid(Rgba::BLACK));
        clipped.clip = Some(0);
        let free = item(Path::rect(0.0, 0.0, 2.0, 2.0), Paint::Solid(Rgba::BLACK));
        let (pm, st) = render(&DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![clipped, free.clone()], clip_parents: vec![None], groups: Vec::new() }, &view(8, 8)).unwrap();
        assert_eq!(st.drawn, 1);
        assert_eq!(pm, render(&list(vec![free]), &view(8, 8)).unwrap().0);
    }

    #[test]
    fn nested_clips_intersect_and_bad_links_draw_nothing() {
        let mut fill = item(Path::rect(0.0, 0.0, 64.0, 64.0), Paint::Solid(Rgba::BLACK));
        fill.clip = Some(1);
        let mut outer = item(Path::rect(0.0, 0.0, 32.0, 64.0), Paint::Solid(Rgba::BLACK));
        outer.masks = Some(0);
        let mut inner = item(Path::rect(0.0, 16.0, 64.0, 32.0), Paint::Solid(Rgba::BLACK));
        inner.masks = Some(1);
        let items = vec![fill, outer, inner];
        // Clip 1 lies inside clip 0: a 32 × 32 square across two render bands survives.
        let (pm, _) = render(&DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: items.clone(), clip_parents: vec![None, Some(0)], groups: Vec::new() }, &view(64, 64)).unwrap();
        assert!((alpha_sum(&pm) - 32.0 * 32.0).abs() < 1e-2, "{}", alpha_sum(&pm));
        // A parent link pointing forward (malformed) or out of range masks everything out.
        for parents in [vec![Some(1), Some(1)], vec![None, Some(9)]] {
            let (pm, _) = render(&DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: items.clone(), clip_parents: parents, groups: Vec::new() }, &view(64, 64)).unwrap();
            assert_eq!(alpha_sum(&pm), 0.0);
        }
    }

    fn view(w: u32, h: u32) -> View {
        View { transform: Affine::IDENTITY, width: w, height: h, background: None }
    }

    fn alpha_sum(p: &Pixmap) -> f32 {
        p.data.iter().map(|c| c[3]).sum()
    }

    #[test]
    fn circle_area_matches_pi_r2() {
        let r = 40.0;
        let dl = list(vec![item(Path::ellipse(50.0, 50.0, r, r), Paint::Solid(Rgba::BLACK))]);
        let (pm, _) = render(&dl, &view(100, 100)).unwrap();
        let area = std::f32::consts::PI * r * r;
        let err = (alpha_sum(&pm) - area).abs() / area;
        assert!(err < 0.005, "relative error {err}");
    }

    #[test]
    fn rect_is_pixel_exact_and_clipped() {
        let dl = list(vec![item(Path::rect(-10.0, 2.0, 15.0, 3.0), Paint::Solid(Rgba::WHITE))]);
        let (pm, _) = render(&dl, &view(8, 8)).unwrap();
        assert_eq!(pm.pixel(0, 2).unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(pm.pixel(4, 4).unwrap(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(pm.pixel(5, 3).unwrap()[3], 0.0);
        assert_eq!(pm.pixel(0, 1).unwrap()[3], 0.0);
        assert!((alpha_sum(&pm) - 15.0).abs() < 1e-3);
    }

    #[test]
    fn stroke_covers_expected_area() {
        let mut it = item(Path::line(Vec2::new(10.0, 20.0), Vec2::new(90.0, 20.0)), Paint::Solid(Rgba::BLACK));
        it.fill = None;
        it.stroke = Some(anicraft_doc::Stroke::new(Rgba::BLACK, 4.0));
        let (pm, _) = render(&list(vec![it]),&view(100, 40)).unwrap();
        // 80 × 4 body plus two round caps (π · 2²)
        let expected = 80.0 * 4.0 + std::f32::consts::PI * 4.0;
        assert!((alpha_sum(&pm) - expected).abs() / expected < 0.01, "{}", alpha_sum(&pm));
    }

    fn stroked(path: Path, width: f32, profile: Option<anicraft_doc::WidthProfile>) -> DisplayItem {
        let mut it = item(path, Paint::Solid(Rgba::BLACK));
        it.fill = None;
        it.stroke = Some(anicraft_doc::Stroke::new(Rgba::BLACK, width).with_profile(profile));
        it
    }

    #[test]
    fn a_tapered_stroke_covers_the_profile_integral() {
        // Taper both ends on an 80-unit line, nominal width 8: the half-width is 4·f(t), f the
        // monotone cubic 0 → 1 → 0. Each half is a Hermite segment (h = ½, ends 0 and 1,
        // tangents 2 and 0), whose integral is h·(0 + 1)/2 + h²·(2 − 0)/12 = 7/24; no caps.
        let taper = anicraft_doc::ProfilePreset::TaperBoth.profile();
        let (pm, _) = render(&list(vec![stroked(Path::line(Vec2::new(10.0, 20.0), Vec2::new(90.0, 20.0)), 8.0, taper.clone())]), &view(100, 40)).unwrap();
        let expected = 2.0 * 4.0 * 80.0 * (7.0 / 12.0);
        assert!((alpha_sum(&pm) - expected).abs() / expected < 0.01, "{} vs {expected}", alpha_sum(&pm));
        // Thin at the ends, full in the middle.
        assert_eq!(pm.pixel(50, 17).unwrap()[3], 1.0);
        assert_eq!(pm.pixel(12, 17).unwrap()[3], 0.0);
        // The profile lives in the shape's space: twice the scale, four times the area.
        let mut big = stroked(Path::line(Vec2::new(10.0, 20.0), Vec2::new(90.0, 20.0)), 8.0, taper);
        big.transform = Affine::scale(2.0, 2.0);
        let (pm2, _) = render(&list(vec![big]), &view(200, 80)).unwrap();
        assert!((alpha_sum(&pm2) / alpha_sum(&pm) - 4.0).abs() < 0.04, "{}", alpha_sum(&pm2) / alpha_sum(&pm));
    }

    #[test]
    fn constant_profiles_match_uniform_strokes_and_one_side_stays_on_its_side() {
        let line = || Path::line(Vec2::new(10.0, 20.0), Vec2::new(90.0, 20.0));
        let p = |l: f32, r: f32| anicraft_doc::WidthProfile::new(vec![anicraft_doc::WidthPoint::new(0.5, l, r)]);
        // 1.5 × a 4-wide stroke is a 6-wide one: body plus round caps.
        let (wide, _) = render(&list(vec![stroked(line(), 4.0, p(1.5, 1.5))]), &view(100, 40)).unwrap();
        let expected = 80.0 * 6.0 + std::f32::consts::PI * 9.0;
        assert!((alpha_sum(&wide) - expected).abs() / expected < 0.01, "{}", alpha_sum(&wide));
        // All on the left (above a line heading right): a 4-wide band, with half-disc caps.
        let (left, _) = render(&list(vec![stroked(line(), 4.0, p(2.0, 0.0))]), &view(100, 40)).unwrap();
        let expected = 80.0 * 4.0 + std::f32::consts::PI * 4.0;
        assert!((alpha_sum(&left) - expected).abs() / expected < 0.01, "{}", alpha_sum(&left));
        assert_eq!(left.pixel(50, 17).unwrap()[3], 1.0);
        assert_eq!(left.pixel(50, 22).unwrap()[3], 0.0, "nothing on the right");
        // A mirror swaps the sides as seen on screen.
        let mut flipped = stroked(line(), 4.0, p(2.0, 0.0));
        flipped.transform = Affine::translate(0.0, 40.0).mul(&Affine::scale(1.0, -1.0));
        let (f, _) = render(&list(vec![flipped]), &view(100, 40)).unwrap();
        assert_eq!(f.pixel(50, 22).unwrap()[3], 1.0);
        assert_eq!(f.pixel(50, 17).unwrap()[3], 0.0);
    }

    /// `cargo test -p anicraft-render --release -- --ignored --nocapture variable_stroke_cost`
    #[test]
    #[ignore = "measurement"]
    fn variable_stroke_cost() {
        use anicraft_doc::{Cap, Dash, Join, StrokeStyle};
        let strokes = |profile: Option<anicraft_doc::WidthProfile>, style: StrokeStyle| {
            let items = (0..300)
                .map(|i| {
                    let (x, y) = ((i % 20) as f32 * 64.0, (i / 20) as f32 * 48.0);
                    let mut p = Path::new();
                    p.move_to(Vec2::new(x, y + 40.0)).cubic_to(Vec2::new(x + 10.0, y - 20.0), Vec2::new(x + 50.0, y + 80.0), Vec2::new(x + 60.0, y + 10.0));
                    let mut it = stroked(p, 6.0, profile.clone());
                    it.stroke = it.stroke.map(|st| st.with_style(style.clone()));
                    it
                })
                .collect();
            list(items)
        };
        let round = StrokeStyle::default();
        let miter = StrokeStyle { cap: Cap::Square, join: Join::Miter, ..StrokeStyle::default() };
        let dashed = StrokeStyle { cap: Cap::Butt, dash: Dash::dashed(8.0, 4.0), ..StrokeStyle::default() };
        let dotted = StrokeStyle { dash: Dash::dotted(4.0), ..StrokeStyle::default() };
        let taper = || anicraft_doc::ProfilePreset::TaperBoth.profile();
        for (name, dl) in [
            ("uniform", strokes(None, round.clone())),
            ("taper_both", strokes(taper(), round)),
            ("bulge", strokes(anicraft_doc::ProfilePreset::Bulge.profile(), StrokeStyle::default())),
            ("square + miter", strokes(None, miter)),
            ("dashed 8/4", strokes(None, dashed.clone())),
            ("dotted 4", strokes(None, dotted)),
            ("taper_both dashed", strokes(taper(), dashed)),
        ] {
            let mut ms: Vec<f64> = (0..9)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = render(&dl, &view(1280, 720)).unwrap();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            ms.sort_by(f64::total_cmp);
            println!("300 strokes, 1280x720, {name}: median {:.2} ms", ms[4]);
        }
    }

    #[test]
    fn src_over_order() {
        let a = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Solid(Rgba::new(1.0, 0.0, 0.0, 1.0)));
        let b = item(Path::rect(0.0, 0.0, 4.0, 4.0), Paint::Solid(Rgba::new(0.0, 0.0, 1.0, 0.5)));
        let (pm, _) = render(&list(vec![a, b]),&view(4, 4)).unwrap();
        let p = pm.pixel(1, 1).unwrap();
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[2] - 0.5).abs() < 1e-5 && (p[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn demo_renders_deterministically() {
        let d = sample::demo();
        let a = render_frame(&d, 7, 0.5).unwrap();
        let b = render_frame(&d, 7, 0.5).unwrap();
        assert_eq!(a.to_rgba8(), b.to_rgba8());
        assert_eq!((a.width, a.height), (640, 360));
        // Frames differ because things move.
        let c = render_frame(&d, 20, 0.5).unwrap();
        assert_ne!(a.to_rgba8(), c.to_rgba8());
        let png = a.encode_png().unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn offscreen_items_are_culled_without_changing_pixels() {
        let on = item(Path::rect(2.0, 2.0, 4.0, 4.0), Paint::Solid(Rgba::BLACK));
        let off = item(Path::ellipse(500.0, 500.0, 10.0, 10.0), Paint::Solid(Rgba::BLACK));
        let (a, sa) = render(&list(vec![on.clone(), off]),&view(16, 16)).unwrap();
        let (b, _) = render(&list(vec![on]),&view(16, 16)).unwrap();
        assert_eq!(sa.culled, 1);
        assert_eq!(sa.drawn, 1);
        assert_eq!(a, b);
    }

    #[test]
    fn band_boundaries_do_not_change_coverage() {
        // A rectangle with fractional edges spanning three 32-row bands keeps its exact area.
        let dl = list(vec![item(Path::rect(3.25, 20.5, 40.0, 61.75), Paint::Solid(Rgba::BLACK))]);
        let (pm, _) = render(&dl, &view(64, 100)).unwrap();
        assert!((alpha_sum(&pm) - 40.0 * 61.75).abs() < 1e-2, "{}", alpha_sum(&pm));
        // and a full row inside the shape is fully covered on both sides of a band edge
        for y in [31, 32] {
            assert_eq!(pm.pixel(10, y).unwrap()[3], 1.0);
        }
    }

    #[test]
    fn many_shapes_render_identically_across_runs() {
        let d = sample::stress(4, 300, 24);
        let a = render_frame(&d, 5, 0.5).unwrap();
        for _ in 0..3 {
            assert_eq!(render_frame(&d, 5, 0.5).unwrap(), a);
        }
    }

    #[test]
    fn huge_surface_is_refused() {
        assert!(matches!(Pixmap::new(100_000, 100_000), Err(RenderError::TooLarge(..))));
    }

    #[test]
    fn blend_modes_follow_the_compositing_formulas() {
        let red = [1.0, 0.0, 0.0, 1.0];
        let grey = [0.5, 0.5, 0.5, 1.0];
        let close = |a: Premul, b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(close(blend(BlendMode::Multiply, red, grey), [0.5, 0.0, 0.0, 1.0]));
        assert!(close(blend(BlendMode::Screen, red, grey), [1.0, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Darken, red, grey), [0.5, 0.0, 0.0, 1.0]));
        assert!(close(blend(BlendMode::Lighten, red, grey), [1.0, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Difference, red, grey), [0.5, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Add, red, grey), [1.0, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Subtract, red, grey), [0.0, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Invert, red, grey), [0.5, 0.5, 0.5, 1.0]));
        assert!(close(blend(BlendMode::Erase, red, grey), [0.0, 0.0, 0.0, 0.0]));
        assert!(close(blend(BlendMode::Alpha, [0.0, 0.0, 0.0, 0.5], grey), [0.25, 0.25, 0.25, 0.5]));
        // Over nothing, a blend mode is just the source; a clear source leaves the backdrop.
        assert!(close(blend(BlendMode::Multiply, red, [0.0; 4]), red));
        assert!(close(blend(BlendMode::Multiply, [0.0; 4], grey), grey));
    }

    #[test]
    fn a_multiply_group_darkens_what_is_below() {
        let bg = item(Path::rect(0.0, 0.0, 20.0, 10.0), Paint::Solid(Rgba::new(0.5, 0.5, 1.0, 1.0)));
        let mut top = item(Path::rect(0.0, 0.0, 10.0, 10.0), Paint::Solid(Rgba::new(1.0, 0.0, 0.0, 1.0)));
        top.group = Some(0);
        let dl = DisplayList { left_out: false, links: Vec::new(), clip_modes: Vec::new(), items: vec![bg, top], clip_parents: vec![], groups: vec![anicraft_doc::Group { parent: None, alpha: 1.0, blend: BlendMode::Multiply, filters: Default::default(), clip: None }] };
        let (pm, _) = render(&dl, &View { background: Some(Rgba::WHITE), ..view(20, 10) }).unwrap();
        let p = pm.pixel(5, 5).unwrap();
        assert!((p[0] - 0.5).abs() < 1e-3 && p[1].abs() < 1e-3 && p[2].abs() < 1e-3, "red × lavender: {p:?}");
        let q = pm.pixel(15, 5).unwrap();
        assert!((q[2] - 1.0).abs() < 1e-3, "outside the group unchanged: {q:?}");
    }
}

#[cfg(test)]
#[path = "warp_tests.rs"]
mod warp_tests;
