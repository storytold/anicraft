//! Visual Frame Picker panel (P-026): Adobe Animate-style visual pose grid for Graphic and
//! Movie Clip symbol instances (lip-sync mouths, hand poses, eye blinks, props).
//!
//! Renders miniature `72×72` CPU-oracle thumbnails (`anicraft_render`) cached by library
//! pointer signature + `(SymbolId, frame_idx)` up to `256` frames, with a `1..N` badge,
//! keyframe label, search filter, Loop Mode selector, `Create keyframe` toggle, and 1-click
//! pose picking (`frame_picker.pick`).

use crate::a11y::{self, Named};
use crate::app::App;
use crate::theme::{T, Type, color, font, text};
use anicraft_engine::cmds_frame_picker::{loop_mode_key, resolve_instance};
use anicraft_engine::doc::{
    Document, Element, ElementId, ElementKind, Layer, LayerId, LayerKind, LoopMode, Scene,
    SymbolId, SymbolKind, evaluate_layer_elements,
};
use anicraft_engine::geom::{Affine, Rect as GeomRect};
use egui::{Align2, ColorImage, Id, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, WidgetType, pos2, vec2};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Thumbnail raster resolution in pixels (`72×72`).
pub const THUMB_PX: u32 = 72;
/// Maximum symbol frames rendered & cached in the Frame Picker grid.
pub const MAX_CACHED_FRAMES: usize = 256;

type ThumbKey = (usize, SymbolId, u32);

/// View state and bounded thumbnail cache for the Frame Picker panel.
pub struct FramePickerUi {
    /// Case-insensitive filter matching frame number (`1..=N`) or keyframe label.
    pub filter: String,
    /// Automatically insert a keyframe at the playhead when picking a pose.
    pub auto_keyframe: bool,
    /// Override loop mode when picking (`None` follows the selected instance's current mode,
    /// defaulting to `SingleFrame` on first pick when desired).
    pub loop_override: Option<LoopMode>,
    last_element: Option<ElementId>,
    bounds_cache: Option<((usize, SymbolId), GeomRect)>,
    thumbs: HashMap<ThumbKey, Option<TextureHandle>>,
}

impl Default for FramePickerUi {
    fn default() -> Self {
        FramePickerUi {
            filter: String::new(),
            auto_keyframe: true,
            loop_override: None,
            last_element: None,
            bounds_cache: None,
            thumbs: HashMap::new(),
        }
    }
}

impl FramePickerUi {
    #[cfg(test)]
    pub fn cached_count(&self) -> usize {
        self.thumbs.len()
    }
}

fn library_signature(doc: &Document) -> usize {
    let sym_h = doc
        .library
        .iter()
        .fold(0usize, |h, s| h.wrapping_mul(131).wrapping_add(Arc::as_ptr(s) as usize));
    let bmp_h = doc
        .bitmaps
        .iter()
        .fold(0usize, |h, b| h.wrapping_mul(137).wrapping_add(Arc::as_ptr(b) as usize));
    sym_h ^ bmp_h
}

fn evaluate_symbol_pose(doc: &Document, sym_id: SymbolId, frame: u32) -> anicraft_engine::doc::DisplayList {
    let mut d = doc.clone();
    let mut layer = Layer::new(LayerId(u64::MAX - 1), "");
    if let Some(k) = layer.keyframe_mut(0) {
        k.elements = vec![Arc::new(Element {
            id: ElementId(u64::MAX - 2),
            name: String::new(),
            transform: Affine::IDENTITY,
            color: Default::default(),
            transform_3d: None,
            kind: ElementKind::Symbol {
                symbol: sym_id,
                loop_mode: LoopMode::SingleFrame,
                first_frame: frame,
                blend: Default::default(),
                filters: Vec::new(),
                events: Vec::new(),
                pivot: None,
            },
        })];
    }
    let mut scene = Scene::new("");
    scene.timeline.layers.push(Arc::new(layer));
    d.scenes = vec![Arc::new(scene)];
    anicraft_engine::doc::evaluate_scene(&d, 0, 0)
}

fn display_list_bounds(dl: &anicraft_engine::doc::DisplayList) -> GeomRect {
    dl.items
        .iter()
        .filter(|i| !i.guide && i.masks.is_none())
        .fold(GeomRect::EMPTY, |acc, i| {
            let reach = i.stroke.as_ref().map_or(0.0, |s| s.reach());
            acc.union(&i.path.bounds().inflate(reach).transformed(&i.transform))
        })
}

fn shared_symbol_bounds(ui_state: &mut FramePickerUi, doc: &Document, sig: usize, sym_id: SymbolId, count: u32) -> GeomRect {
    if let Some((key, b)) = ui_state.bounds_cache
        && key == (sig, sym_id)
    {
        return b;
    }
    let mut union_b = GeomRect::EMPTY;
    let cap = count.min(MAX_CACHED_FRAMES as u32);
    // Sample keyframes (up to 32) so multi-pose mouth/hand charts share consistent framing.
    let stride = (cap / 32).max(1);
    let mut f = 0;
    while f < cap {
        let dl = evaluate_symbol_pose(doc, sym_id, f);
        union_b = union_b.union(&display_list_bounds(&dl));
        f = f.saturating_add(stride);
    }
    ui_state.bounds_cache = Some(((sig, sym_id), union_b));
    union_b
}

fn get_or_render_thumb(
    ctx: &egui::Context,
    ui_state: &mut FramePickerUi,
    doc: &Document,
    sig: usize,
    sym_id: SymbolId,
    frame_idx: u32,
    shared_bounds: GeomRect,
) -> Option<TextureHandle> {
    let key = (sig, sym_id, frame_idx);
    if let Some(existing) = ui_state.thumbs.get(&key) {
        return existing.clone();
    }
    if ui_state.thumbs.len() >= MAX_CACHED_FRAMES {
        // Evict stale symbol entries first; if still full, reset bounded cache.
        ui_state.thumbs.retain(|&(s, id, _), _| s == sig && id == sym_id);
        if ui_state.thumbs.len() >= MAX_CACHED_FRAMES {
            ui_state.thumbs.clear();
        }
    }

    let dl = evaluate_symbol_pose(doc, sym_id, frame_idx);
    let own_b = display_list_bounds(&dl);
    if own_b.is_empty() || !(own_b.width().is_finite() && own_b.height().is_finite()) {
        ui_state.thumbs.insert(key, None);
        return None;
    }
    let framing = if !shared_bounds.is_empty() && shared_bounds.width().is_finite() && shared_bounds.height().is_finite() {
        shared_bounds
    } else {
        own_b
    };
    let (w, h) = (THUMB_PX, THUMB_PX);
    let pad = 8.0f32;
    let scale = ((w as f32 - pad) / framing.width().max(1.0))
        .min((h as f32 - pad) / framing.height().max(1.0))
        .clamp(0.02, 8.0);
    let c = framing.center();
    let transform = Affine::translate(w as f32 * 0.5, h as f32 * 0.5)
        .mul(&Affine::scale(scale, scale))
        .mul(&Affine::translate(-c.x, -c.y));
    let view = anicraft_engine::render::View {
        transform,
        width: w,
        height: h,
        background: None,
    };
    let rendered = anicraft_engine::render::render(&dl, &view).ok().map(|(px, _)| {
        let image = ColorImage::from_rgba_premultiplied([w as usize, h as usize], &px.to_premul_rgba8());
        ctx.load_texture(
            format!("fp-{}-{}", sym_id.0, frame_idx),
            image,
            TextureOptions::LINEAR,
        )
    });
    ui_state.thumbs.insert(key, rendered.clone());
    rendered
}

fn loop_label(mode: LoopMode) -> &'static str {
    match mode {
        LoopMode::SingleFrame => tr!("Single Frame"),
        LoopMode::PlayOnce => tr!("Play Once"),
        LoopMode::Loop => tr!("Loop"),
    }
}

fn frame_label_at(sym: &anicraft_engine::doc::Symbol, f: u32) -> &str {
    sym.timeline
        .layers
        .iter()
        .find_map(|l| {
            l.keyframe_starting_at(f)
                .and_then(|i| l.keyframes.get(i))
                .map(|k| k.label.as_str())
                .filter(|lb| !lb.is_empty())
        })
        .unwrap_or("")
}

fn frame_non_empty(sym: &anicraft_engine::doc::Symbol, f: u32) -> bool {
    sym.timeline
        .layers
        .iter()
        .filter(|l| l.visible && !matches!(l.kind, LayerKind::Folder | LayerKind::Guide | LayerKind::Camera))
        .any(|l| evaluate_layer_elements(l, f).is_some_and(|(_, els)| !els.is_empty()))
}

/// Draws the dockable **Frame Picker** panel.
pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);

    let resolved = resolve_instance(&app.s, None, None).ok();
    let Some((lid, el)) = resolved else {
        ui.add(egui::Label::new(text(Type::Heading, tr!("Frame Picker")).strong()).wrap());
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                text(
                    Type::Body,
                    tr!("Select a Graphic or Movie Clip symbol instance on the stage to browse and pick its frames."),
                )
                .color(T.text_dim),
            )
            .wrap(),
        );
        return;
    };

    let ElementKind::Symbol {
        symbol: sym_id,
        loop_mode: inst_loop,
        first_frame: inst_first,
        ..
    } = el.kind
    else {
        return;
    };

    let Some(sym_arc) = app.s.doc.library.iter().find(|s| s.id == sym_id).cloned() else {
        return;
    };

    if app.frame_picker.last_element != Some(el.id) {
        app.frame_picker.last_element = Some(el.id);
        app.frame_picker.loop_override = None;
    }
    let active_loop = app.frame_picker.loop_override.unwrap_or(inst_loop);
    let total_frames = sym_arc.timeline.frame_count().max(1);
    let shown_cap = total_frames.min(MAX_CACHED_FRAMES as u32);

    // 1. Symbol title & frame count summary
    ui.add(egui::Label::new(text(Type::Heading, &sym_arc.name).strong()).wrap());
    let frames_str = trn!("{n} frame", "{n} frames", total_frames as usize);
    ui.add(
        egui::Label::new(
            text(
                Type::Caption,
                tr!(
                    "{frames} · current: {current}",
                    frames = frames_str,
                    current = inst_first + 1
                ),
            )
            .color(T.text_dim),
        )
        .wrap(),
    );
    if sym_arc.kind != SymbolKind::Graphic {
        ui.add(
            egui::Label::new(
                text(
                    Type::Caption,
                    tr!("Tip: Graphic symbols scrub in sync with the main timeline."),
                )
                .color(T.text_faint),
            )
            .wrap(),
        );
    }
    ui.add_space(6.0);

    // 2. Loop Mode selector & Create Keyframe toggle
    let combo_w = (ui.available_width() - 4.0).clamp(60.0, 220.0);
    egui::ComboBox::from_id_salt("frame_picker_loop_mode")
        .width(combo_w)
        .selected_text(text(Type::Body, loop_label(active_loop)))
        .show_ui(ui, |ui| {
            for mode in [LoopMode::SingleFrame, LoopMode::PlayOnce, LoopMode::Loop] {
                if ui
                    .selectable_label(active_loop == mode, text(Type::Body, loop_label(mode)))
                    .clicked()
                {
                    app.frame_picker.loop_override = Some(mode);
                    app.run(
                        "frame_picker.pick",
                        json!({
                            "element": el.id.0,
                            "layer": lid.0,
                            "frame_index": inst_first.min(total_frames - 1),
                            "loop": loop_mode_key(mode),
                            "auto_keyframe": false,
                        }),
                    );
                }
            }
        })
        .response
        .set_name(tr!("Loop mode"));

    ui.add_space(2.0);
    ui.checkbox(
        &mut app.frame_picker.auto_keyframe,
        text(Type::Body, tr!("Create keyframe")),
    )
    .on_hover_text(tr!(
        "Automatically insert a keyframe at the playhead when clicking a frame thumbnail"
    ));

    // 3. Filter box (by frame number or label)
    ui.add_space(4.0);
    let filter_w = (ui.available_width() - 4.0).max(40.0);
    ui.add(
        egui::TextEdit::singleline(&mut app.frame_picker.filter)
            .desired_width(filter_w)
            .hint_text(tr!("Filter frames or labels…")),
    )
    .set_name(tr!("Filter frames"));
    ui.add_space(6.0);

    // 4. Responsive thumbnail grid (`72×72` oracle thumbnails)
    let query = app.frame_picker.filter.trim().to_lowercase();
    let mut visible_frames: Vec<(u32, String, bool)> = Vec::new();
    for f in 0..shown_cap {
        let lbl = frame_label_at(&sym_arc, f);
        if !query.is_empty() {
            let num_str = (f + 1).to_string();
            if !num_str.contains(&query) && !lbl.to_lowercase().contains(&query) {
                continue;
            }
        }
        visible_frames.push((f, lbl.to_string(), frame_non_empty(&sym_arc, f)));
    }

    if visible_frames.is_empty() {
        ui.add(
            egui::Label::new(text(Type::Caption, tr!("No matching frames.")).color(T.text_dim))
                .wrap(),
        );
        return;
    }

    let sig = library_signature(&app.s.doc);
    let shared_bounds = shared_symbol_bounds(&mut app.frame_picker, &app.s.doc, sig, sym_id, shown_cap);

    let card_w = 72.0f32;
    let thumb_h = 64.0f32;
    let card_h = 86.0f32;
    let gap = 6.0f32;
    let avail = ui.available_width().max(card_w);
    let cols = ((avail + gap) / (card_w + gap)).floor().max(1.0) as usize;
    let ctx = ui.ctx().clone();

    let mut picked_frame: Option<u32> = None;
    for chunk in visible_frames.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            for (f, lbl, non_empty) in chunk {
                let is_current = *f == inst_first;
                let (rect, resp) = ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                let a11y_name = if lbl.is_empty() {
                    tr!("Frame {n}", n = f + 1)
                } else {
                    tr!("Frame {n}: {label}", n = f + 1, label = lbl)
                };
                a11y::control(ui, &resp, WidgetType::Button, &a11y_name, Some(is_current));

                let bg = if is_current {
                    T.active
                } else if resp.hovered() {
                    T.hover
                } else {
                    T.field
                };
                let painter = ui.painter();
                painter.rect_filled(rect, T.rc(), bg);
                let border_stroke = if is_current {
                    Stroke::new(2.0, T.accent)
                } else if resp.hovered() {
                    Stroke::new(1.0, T.selection)
                } else {
                    Stroke::new(1.0, T.border)
                };
                painter.rect_stroke(rect, T.rc(), border_stroke, StrokeKind::Inside);
                a11y::focus_ring(ui, &resp);

                let thumb_rect = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(card_w - 8.0, thumb_h - 6.0));
                if *non_empty
                    && let Some(tex) = get_or_render_thumb(
                        &ctx,
                        &mut app.frame_picker,
                        &app.s.doc,
                        sig,
                        sym_id,
                        *f,
                        shared_bounds,
                    )
                {
                    painter.image(
                        tex.id(),
                        thumb_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    painter.text(
                        thumb_rect.center(),
                        Align2::CENTER_CENTER,
                        "·",
                        font(Type::Caption),
                        T.text_faint,
                    );
                }

                // 1..N frame badge in top-left corner
                let badge_text = format!("{}", f + 1);
                let badge_galley = painter.layout_no_wrap(
                    badge_text,
                    font(Type::DataSmall),
                    if is_current { T.on_fill } else { T.text },
                );
                let badge_rect = Rect::from_min_size(
                    rect.min + vec2(3.0, 3.0),
                    vec2((badge_galley.rect.width() + 6.0).max(16.0), badge_galley.rect.height() + 2.0),
                );
                painter.rect_filled(
                    badge_rect,
                    T.rc(),
                    if is_current { T.accent } else { T.panel_alt },
                );
                painter.galley(
                    pos2(badge_rect.center().x - badge_galley.rect.width() * 0.5, badge_rect.top() + 1.0),
                    badge_galley,
                    if is_current { T.on_fill } else { T.text },
                );

                // Bottom caption: frame label (or frame number when unlabelled)
                let footer = if lbl.is_empty() {
                    format!("#{}", f + 1)
                } else {
                    lbl.chars().take(10).collect::<String>()
                };
                let footer_col = if is_current { T.on_active } else { color(Type::Caption) };
                let footer_clip = Rect::from_min_max(pos2(rect.left() + 3.0, rect.bottom() - 19.0), pos2(rect.right() - 3.0, rect.bottom() - 2.0));
                painter.with_clip_rect(footer_clip.intersect(painter.clip_rect())).text(
                    footer_clip.center(),
                    Align2::CENTER_CENTER,
                    footer,
                    font(Type::Caption),
                    footer_col,
                );

                let resp = resp.on_hover_text(&a11y_name);
                if resp.clicked() {
                    picked_frame = Some(*f);
                }
                let _ = Id::new(("fp_cell", sym_id.0, *f));
            }
        });
        ui.add_space(gap);
    }

    if total_frames > shown_cap {
        ui.add(
            egui::Label::new(
                text(
                    Type::Caption,
                    tr!(
                        "Showing first {shown} of {total} frames.",
                        shown = shown_cap,
                        total = total_frames
                    ),
                )
                .color(T.text_faint),
            )
            .wrap(),
        );
    }

    if let Some(chosen_f) = picked_frame {
        let mode_to_set = app.frame_picker.loop_override.unwrap_or(if inst_loop == LoopMode::Loop {
            LoopMode::SingleFrame
        } else {
            inst_loop
        });
        app.frame_picker.loop_override = Some(mode_to_set);
        app.run(
            "frame_picker.pick",
            json!({
                "element": el.id.0,
                "layer": lid.0,
                "frame_index": chosen_f,
                "loop": loop_mode_key(mode_to_set),
                "auto_keyframe": app.frame_picker.auto_keyframe,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_picker_panel_renders_and_caches_mouth_chart_thumbnails() {
        let ctx = egui::Context::default();
        let mut session = anicraft_engine::Session::default();
        session
            .execute("file.new_from_template", &json!({"template": "lip-sync"}))
            .unwrap();
        let mouth_lid = session
            .timeline()
            .layers
            .iter()
            .find(|l| l.name == "Mouth")
            .unwrap()
            .id;
        session
            .execute("layer.select", &json!({"layer": mouth_lid.0, "select": true}))
            .unwrap();

        let mut app = App::with_context(&ctx, session, Some(std::path::PathBuf::from("unused.png")));
        app.show_panel(crate::dock::Panel::FramePicker);
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 940.0))),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| app.frame(ui)).textures_delta.clear();
        }
        // All 10 mouth poses of the Lip-sync mouth chart template are rasterized & cached.
        assert_eq!(app.frame_picker.cached_count(), 10);
    }
}
