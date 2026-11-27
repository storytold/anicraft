//! Editor-only pictures for independently composited Linear Array instances. Exact tiles
//! still come from the usual stage jobs, including after a pause or release.

use super::TileSpec;
use anicraft_engine::doc::{DisplayList, Element, ElementId, Filter, Group, LayerId, Stroke};
use anicraft_engine::geom::{Affine, Rect as ArtRect, Vec2};
use anicraft_engine::render::View;
use anicraft_engine::render_service::{FrameJob, RenderService, frame_key, make_job};
use anicraft_engine::StateId;
use egui::{Color32, Context, Painter, Pos2, Rect, TextureHandle, pos2};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const PAUSE: f64 = 0.065;
const WARM_DELAY: f64 = 0.100;
// Leave room for the backdrop and a dense set of copies on a high-DPI display.
const MAX_PIXELS: u64 = 8 * 1024 * 1024;
const MAX_EDGE: f32 = 2048.0;
const MAX_ITEMS: usize = 12_000;
const UPLOADS_PER_FRAME: usize = 32;

pub(super) struct Scene<'a> {
    pub generation: StateId,
    pub state: StateId,
    pub dim: bool,
    pub scale: f32,
    pub jobs: &'a [(TileSpec, FrameJob)],
    pub instances: &'a [(LayerId, Arc<Element>)],
}

#[derive(PartialEq)]
struct Key {
    generation: StateId,
    dim: bool,
    raster: u64,
    tiles: Vec<u64>,
}

#[derive(Default)]
pub(super) struct Cache {
    key: Option<Key>,
    plan: Option<Plan>,
    attempted: bool,
    stable_since: Option<(StateId, f64)>,
    moved: Option<(StateId, f64)>,
    #[cfg(test)]
    pub presented: Option<StateId>,
}

impl Cache {
    pub fn update(&mut self, ctx: &Context, svc: &RenderService, scene: Scene<'_>, moved: bool) {
        let Some((_, first)) = scene.jobs.first() else { return };
        let key = Key {
            generation: scene.generation,
            dim: scene.dim,
            raster: first.raster,
            tiles: scene.jobs.iter().map(|(_, j)| frame_key(scene.generation, j.frame, &j.view, None)).collect(),
        };
        if self.key.as_ref() != Some(&key) {
            *self = Self { key: Some(key), ..Self::default() };
        }
        let (now, held) = ctx.input(|i| (i.time, i.pointer.primary_down()));
        if moved {
            self.moved = Some((scene.state, now));
        }
        if self.stable_since.is_none_or(|(state, _)| state != scene.state) {
            self.stable_since = Some((scene.state, now));
        }
        if !self.attempted && !held && let Some((_, since)) = self.stable_since {
            if now - since < WARM_DELAY {
                ctx.request_repaint_after(Duration::from_secs_f64(WARM_DELAY - (now - since)));
            } else {
                self.attempted = true;
                self.plan = Plan::new(scene);
            }
        }
        if let Some(plan) = &mut self.plan
            && !plan.upload(ctx, svc)
        {
            self.plan = None;
        }
    }

    pub fn pending(&self) -> Vec<FrameJob> {
        self.plan.as_ref().map_or_else(Vec::new, |plan| plan.layers.iter().filter_map(|l| l.job.clone()).collect())
    }

    /// Present only a current move for which all sources are available. An undo or a numeric
    /// edit changes state without a gizmo move. A pause and release immediately request exact tiles.
    pub fn provisional(&self, ctx: &Context, state: StateId, dragging: bool) -> Option<bool> {
        let (_, moved_at) = self.moved.filter(|(s, _)| *s == state)?;
        if !self.warmed() { return None; }
        let remaining = PAUSE - (ctx.input(|i| i.time) - moved_at);
        let moving = dragging && remaining > 0.0;
        if moving {
            ctx.request_repaint_after(Duration::from_secs_f64(remaining));
        }
        Some(moving)
    }

    pub fn warmed(&self) -> bool {
        self.plan.as_ref().is_some_and(|plan| plan.layers.iter().all(|l| l.texture.is_some()))
    }

    pub fn paint(&mut self, painter: &Painter, state: StateId, instances: &[(LayerId, Arc<Element>)], screen: impl Fn(Vec2) -> Pos2) {
        if let Some(plan) = &self.plan {
            for layer in &plan.layers {
                let Some(texture) = &layer.texture else { continue };
                let [x0, y0, x1, y1] = layer.rect;
                let at = match layer.copy {
                    Some(index) => {
                        let Some((_, element)) = instances.get(index) else { continue };
                        element.transform.translation()
                    }
                    None => Vec2::ZERO,
                };
                let rect = Rect::from_min_max(screen(at + Vec2::new(x0, y0)), screen(at + Vec2::new(x1, y1)));
                if painter.clip_rect().intersects(rect) {
                    painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
            #[cfg(test)]
            { self.presented = Some(state); }
            let _ = state;
        }
    }
}

struct Plan {
    /// Backdrop tiles first, then each copy in the order the exact renderer composites them.
    layers: Vec<Layer>,
}

struct Layer {
    rect: [f32; 4],
    copy: Option<usize>,
    job: Option<FrameJob>,
    texture: Option<TextureHandle>,
}

impl Plan {
    fn new(scene: Scene<'_>) -> Option<Self> {
        let (_, first) = scene.jobs.first()?;
        let positions: HashMap<_, _> = scene.instances.iter().enumerate().map(|(i, (_, element))| (element.id, i)).collect();
        let mut list = first.display_list();
        let Runs { split, mut groups, remap } = isolate(&list, &positions)?;
        let copies = list.items.split_off(split);
        list.links.clear();
        let backdrop = Arc::new(list);
        let mut pixels = 0;
        let mut layers = Vec::with_capacity(scene.jobs.len() + positions.len());
        for (tile, job) in scene.jobs {
            add_pixels(&mut pixels, &job.view)?;
            let part = ("Array backdrop", scene.dim); // i18n-exempt: render job cache key
            let job = make_job(scene.state, &first.doc, first.frame, job.view, None).with_prepared_list(part, backdrop.clone());
            layers.push(Layer { rect: tile.rect, copy: None, job: Some(job), texture: None });
        }
        for items in copies.chunk_by(|a, b| a.element == b.element) {
            let id = items.first()?.element;
            let index = *positions.get(&id)?;
            let at = scene.instances.get(index)?.1.transform.translation();
            // An instance must be one complete run. Its nested groups have already been
            // checked for sharing, clipping and backdrop-dependent blending.
            let groups = groups.remove(&id)?;
            let (view, rect) = patch_view(items, &groups, at, scene.scale)?;
            add_pixels(&mut pixels, &view)?;
            let list = DisplayList {
                items: items.iter().map(|i| anicraft_engine::doc::DisplayItem {
                    group: i.group.and_then(|g| remap.get(g as usize).copied().flatten()), ..i.clone()
                }).collect(),
                groups,
                ..DisplayList::default()
            };
            // Use the CPU oracle to warm small patches. Exact tiles keep the selected rasterizer.
            let part = ("Array copy", scene.dim, id); // i18n-exempt: render job cache key
            let job = make_job(scene.state, &first.doc, first.frame, view, None).with_prepared_list(part, Arc::new(list));
            layers.push(Layer { rect, copy: Some(index), job: Some(job), texture: None });
        }
        Some(Self { layers })
    }

    fn upload(&mut self, ctx: &Context, svc: &RenderService) -> bool {
        let mut uploads = 0;
        for layer in &mut self.layers {
            let Some(img) = layer.job.as_ref().and_then(|j| svc.cached(j.key)) else { continue };
            if img.error.is_some() || img.rgba_premul.len() != img.width as usize * img.height as usize * 4 {
                return false;
            }
            let image = egui::ColorImage::from_rgba_premultiplied([img.width as usize, img.height as usize], &img.rgba_premul);
            layer.texture = Some(ctx.load_texture("filtered Array preview", image, egui::TextureOptions::LINEAR)); // i18n-exempt: texture debug label
            layer.job = None;
            uploads += 1;
            if uploads == UPLOADS_PER_FRAME {
                ctx.request_repaint();
                break;
            }
        }
        true
    }
}

/// Copies must be a suffix of complete, independent normal-blend runs. Keep the normal stage
/// path for masks, clips, other effects, and alpha or blend groups shared across owners.
struct Runs {
    split: usize,
    groups: HashMap<ElementId, Vec<Group>>,
    remap: Vec<Option<u32>>,
}

fn isolate(list: &DisplayList, positions: &HashMap<ElementId, usize>) -> Option<Runs> {
    if !list.clip_parents.is_empty() || list.items.len() > MAX_ITEMS || list.groups.len() > MAX_ITEMS {
        return None;
    }
    let split = list.items.iter().position(|i| positions.contains_key(&i.element))?;
    let mut owners = vec![None; list.groups.len()];
    for (index, item) in list.items.iter().enumerate() {
        let owner = positions.contains_key(&item.element).then_some(item.element);
        if owner.is_some() != (index >= split) || item.clip.is_some() || item.masks.is_some() {
            return None;
        }
        let mut parent = item.group;
        while let Some(id) = parent {
            let assigned = owners.get_mut(id as usize)?;
            if let Some(other) = assigned {
                if *other != owner { return None; }
                break;
            }
            *assigned = Some(owner);
            let group = list.groups.get(id as usize)?;
            if group.parent.is_some_and(|p| p >= id) { return None; }
            parent = group.parent;
        }
    }
    let mut groups: HashMap<ElementId, Vec<Group>> = HashMap::new();
    let mut remap = vec![None; list.groups.len()];
    for (id, (group, owner)) in list.groups.iter().zip(owners).enumerate() {
        let Some(Some(owner)) = owner else { continue };
        if !group.blend.is_normal() || group.clip.is_some() || group.filters.iter().any(|f| !matches!(f, Filter::Blur { .. })) {
            return None;
        }
        let run = groups.entry(owner).or_default();
        let parent = group.parent.and_then(|p| remap.get(p as usize).copied().flatten());
        *remap.get_mut(id)? = Some(run.len() as u32);
        run.push(Group { parent, ..group.clone() });
    }
    Some(Runs { split, groups, remap })
}

fn patch_view(items: &[anicraft_engine::doc::DisplayItem], groups: &[Group], at: Vec2, scale: f32) -> Option<(View, [f32; 4])> {
    let mut bounds = ArtRect::EMPTY;
    for item in items {
        let pad = item.stroke.as_ref().map_or(0.0, Stroke::reach) + 1.0;
        bounds = bounds.union(&item.path.bounds().inflate(pad).transformed(&item.transform));
    }
    // Include the entire filter footprint, even for copies now outside the viewport. The
    // support of nested Blur groups is bounded by the sum of their shared Filter margins.
    let pad = groups.iter().filter(|g| !g.filters.is_empty()).map(|g| {
        (g.filters.iter().map(Filter::margin).sum::<f32>() * scale).ceil() + 2.0
    }).sum::<f32>() + 2.0;
    let lo = [bounds.min.x, bounds.min.y].map(|v| (v * scale - pad).floor());
    let hi = [bounds.max.x, bounds.max.y].map(|v| (v * scale + pad).ceil());
    if !lo.into_iter().chain(hi).all(|v| v.abs() <= 4_194_304.0) { return None; }
    let [w, h] = [hi[0] - lo[0], hi[1] - lo[1]];
    if !(1.0..=MAX_EDGE).contains(&w) || !(1.0..=MAX_EDGE).contains(&h) { return None; }
    let view = View { transform: Affine::translate(-lo[0], -lo[1]).mul(&Affine::scale(scale, scale)), width: w as u32, height: h as u32, background: None };
    let rect = [lo[0] / scale - at.x, lo[1] / scale - at.y, hi[0] / scale - at.x, hi[1] / scale - at.y];
    Some((view, rect))
}

fn add_pixels(total: &mut u64, view: &View) -> Option<()> {
    *total += u64::from(view.width) * u64::from(view.height);
    (*total <= MAX_PIXELS).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anicraft_engine::doc::BlendMode;
    use anicraft_engine::Session;
    use serde_json::json;

    #[test]
    fn copy_runs_reject_masks_backdrop_blends_and_shared_groups() {
        let mut s = Session::default();
        s.execute("draw.rect", &json!({"x": 0, "y": 0, "w": 35, "h": 15, "fill": "#aa2233"})).unwrap();
        s.execute("modify.convert_to_symbol", &json!({"name": "Source", "kind": "movie_clip"})).unwrap();
        s.execute("element.set_filters", &json!({"filters": [{"type": "blur", "blur_x": 6, "blur_y": 18}]})).unwrap();
        s.preview_array(&json!({"count": 2, "dx": 5})).unwrap();
        let positions = s.selection.ids().enumerate().map(|(i, id)| (id, i)).collect();
        let list = s.display_list();
        assert!(isolate(&list, &positions).is_some());

        let mut masked = list.as_ref().clone();
        masked.clip_parents.push(None);
        masked.items[0].clip = Some(0);
        assert!(isolate(&masked, &positions).is_none());

        let mut blended = list.as_ref().clone();
        blended.groups[0].blend = BlendMode::Multiply;
        assert!(isolate(&blended, &positions).is_none());

        let mut shared = list.as_ref().clone();
        shared.items[1].group = shared.items[0].group;
        assert!(isolate(&shared, &positions).is_none());
    }
}
