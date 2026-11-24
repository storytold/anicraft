//! Hybrid Automatic Character Rigging, Dual-Quaternion Mesh Deformation & Facial Rig (Auto Rig Studio).
//!
//! Stores pixel-accurate (`pos_px`) and normalized `[0..=1]` (`norm_uv`) joint coordinates,
//! per-joint confidence scores and low-confidence warnings, fixed-length bone bindings with
//! anatomical rotation limits, silhouette-conforming triangle meshes with normalized geodesic
//! skin weights, and independent facial & lip-sync controls (`FacePose`).
//!
//! Pure functional evaluation (`deform_layer_rig`): at any timeline frame, evaluates the posed
//! armature (`armature::bones_at`) and tweened `FacePose` (`face_pose_at`) to produce a
//! tear-free, volume-preserving `WarpMesh` rendered identically by both the CPU oracle renderer
//! and the Metal `wgpu` GPU stage.

use crate::armature::{self, Armature, BoneId, Pose};
use crate::warp::WarpMesh;
use crate::{BitmapId, Element, ElementId, ElementKind, Keyframe, Layer, SymbolId};
use anicraft_geom::{Path, PathCmd, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Confidence below which a joint is highlighted with a low-confidence warning ring.
pub const DEFAULT_LOW_CONFIDENCE: f32 = 0.58;

/// Unique id of a character rig within a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RigId(pub u64);

/// Automatically detected character art style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterStyle {
    Stickman,
    NoirSilhouette,
    RealisticCartoon,
    #[default]
    StylizedHuman,
    ExaggeratedProportions,
    LayeredArtwork,
}

impl CharacterStyle {
    pub const ALL: [CharacterStyle; 6] = [
        CharacterStyle::Stickman,
        CharacterStyle::NoirSilhouette,
        CharacterStyle::RealisticCartoon,
        CharacterStyle::StylizedHuman,
        CharacterStyle::ExaggeratedProportions,
        CharacterStyle::LayeredArtwork,
    ];

    pub fn key(self) -> &'static str {
        match self {
            CharacterStyle::Stickman => "stickman",
            CharacterStyle::NoirSilhouette => "noir_silhouette",
            CharacterStyle::RealisticCartoon => "realistic_cartoon",
            CharacterStyle::StylizedHuman => "stylized_human",
            CharacterStyle::ExaggeratedProportions => "exaggerated_proportions",
            CharacterStyle::LayeredArtwork => "layered_artwork",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CharacterStyle::Stickman => "Stickman",
            CharacterStyle::NoirSilhouette => "Noir Silhouette",
            CharacterStyle::RealisticCartoon => "Realistic Cartoon",
            CharacterStyle::StylizedHuman => "Stylized Human",
            CharacterStyle::ExaggeratedProportions => "Exaggerated Proportions",
            CharacterStyle::LayeredArtwork => "Layered Artwork",
        }
    }

    pub fn from_key(s: &str) -> Option<CharacterStyle> {
        let clean = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        CharacterStyle::ALL.into_iter().find(|c| c.key() == clean)
    }
}

/// Detected 2D camera view orientation of the character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewAngle {
    #[default]
    Front,
    ThreeQuarterLeft,
    ThreeQuarterRight,
    SideLeft,
    SideRight,
}

impl ViewAngle {
    pub const ALL: [ViewAngle; 5] = [
        ViewAngle::Front,
        ViewAngle::ThreeQuarterLeft,
        ViewAngle::ThreeQuarterRight,
        ViewAngle::SideLeft,
        ViewAngle::SideRight,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ViewAngle::Front => "front",
            ViewAngle::ThreeQuarterLeft => "three_quarter_left",
            ViewAngle::ThreeQuarterRight => "three_quarter_right",
            ViewAngle::SideLeft => "side_left",
            ViewAngle::SideRight => "side_right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewAngle::Front => "Front",
            ViewAngle::ThreeQuarterLeft => "3/4 Left",
            ViewAngle::ThreeQuarterRight => "3/4 Right",
            ViewAngle::SideLeft => "Side Left",
            ViewAngle::SideRight => "Side Right",
        }
    }

    pub fn from_key(s: &str) -> Option<ViewAngle> {
        let clean = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        ViewAngle::ALL.into_iter().find(|v| v.key() == clean)
    }
}

/// Input artwork format used to build the rig.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigSourceKind {
    #[default]
    PngBitmap,
    SvgVector,
    LayeredArtwork,
}

impl RigSourceKind {
    pub fn key(self) -> &'static str {
        match self {
            RigSourceKind::PngBitmap => "png_bitmap",
            RigSourceKind::SvgVector => "svg_vector",
            RigSourceKind::LayeredArtwork => "layered_artwork",
        }
    }

    pub fn from_key(s: &str) -> RigSourceKind {
        match s.trim().to_ascii_lowercase().as_str() {
            "svg" | "svg_vector" => RigSourceKind::SvgVector,
            "layered" | "layered_artwork" => RigSourceKind::LayeredArtwork,
            _ => RigSourceKind::PngBitmap,
        }
    }
}

/// One anatomical joint in the character rig, holding both pixel-accurate local coordinates
/// and normalized `[0..=1]` bounding-box coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigJoint {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Pixel-accurate rest coordinate in the rigged shape's local space.
    pub pos_px: Vec2,
    /// Normalized `[0..=1]` coordinate within `CharacterRig::bounds_px` for cross-scene reuse.
    pub norm_uv: Vec2,
    /// Calibrated CV/geometric landmark confidence (`0..=1`).
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub low_confidence: bool,
    #[serde(default = "yes_bool", skip_serializing_if = "is_yes_bool")]
    pub present: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual_override: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

fn yes_bool() -> bool {
    true
}

fn is_yes_bool(v: &bool) -> bool {
    *v
}

/// A connected bone linking `from_joint` -> `to_joint` with fixed rest length and rotation limits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigBoneBinding {
    pub id: String,
    pub from_joint: String,
    pub to_joint: String,
    /// Bound `BoneId` on the layer's `Armature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bone_id: Option<BoneId>,
    pub head: Vec2,
    pub tail: Vec2,
    pub rest_length_px: f32,
    #[serde(default = "yes_bool")]
    pub fixed_length: bool,
    pub limits_deg: [f32; 2],
    #[serde(default = "yes_bool")]
    pub deform: bool,
}

/// One bone's normalized influence on a mesh vertex: `(deform_bone_index, weight)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VertexWeight(pub u16, pub f32);

/// Silhouette-conforming constrained triangle mesh with normalized geodesic skin weights.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinnedMesh {
    pub rest_vertices: Vec<Vec2>,
    pub triangles: Vec<[u32; 3]>,
    pub deform_bones: Vec<BoneId>,
    pub weights: Vec<Vec<VertexWeight>>,
}

/// Lip-sync mouth visemes driven manually or automatically from timeline speech audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RigViseme {
    #[default]
    Rest,
    Open,
    Wide,
    Round,
    Closed,
    Fv,
    L,
    Wq,
    D,
    R,
    S,
    Uh,
}

impl RigViseme {
    pub const ALL: [RigViseme; 12] = [
        RigViseme::Rest,
        RigViseme::Open,
        RigViseme::Wide,
        RigViseme::Round,
        RigViseme::Closed,
        RigViseme::Fv,
        RigViseme::L,
        RigViseme::Wq,
        RigViseme::D,
        RigViseme::R,
        RigViseme::S,
        RigViseme::Uh,
    ];

    pub fn key(self) -> &'static str {
        match self {
            RigViseme::Rest => "rest",
            RigViseme::Open => "open",
            RigViseme::Wide => "wide",
            RigViseme::Round => "round",
            RigViseme::Closed => "closed",
            RigViseme::Fv => "fv",
            RigViseme::L => "l",
            RigViseme::Wq => "wq",
            RigViseme::D => "d",
            RigViseme::R => "r",
            RigViseme::S => "s",
            RigViseme::Uh => "uh",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RigViseme::Rest => "Rest",
            RigViseme::Open => "AA / Open",
            RigViseme::Wide => "EE / Wide",
            RigViseme::Round => "OH / Round",
            RigViseme::Closed => "MBP / Closed",
            RigViseme::Fv => "FV",
            RigViseme::L => "L / Dental",
            RigViseme::Wq => "OO / Purse",
            RigViseme::D => "D / Alveolar",
            RigViseme::R => "R / Retroflex",
            RigViseme::S => "S / Sibilant",
            RigViseme::Uh => "UH / Mid",
        }
    }

    pub fn from_key(s: &str) -> Option<RigViseme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rest" | "x" | "neutral" => Some(RigViseme::Rest),
            "open" | "aa" | "a" | "ah" => Some(RigViseme::Open),
            "wide" | "ee" | "c" | "ih" => Some(RigViseme::Wide),
            "round" | "oh" | "e" => Some(RigViseme::Round),
            "closed" | "mbp" | "m" | "b" | "p" => Some(RigViseme::Closed),
            "fv" | "f" | "v" | "g" => Some(RigViseme::Fv),
            "l" | "h" => Some(RigViseme::L),
            "wq" | "oo" | "woo" => Some(RigViseme::Wq),
            "d" | "t" | "n" => Some(RigViseme::D),
            "r" => Some(RigViseme::R),
            "s" | "z" | "sh" => Some(RigViseme::S),
            "uh" => Some(RigViseme::Uh),
            _ => None,
        }
    }

    /// Canonical `(jaw_open_delta, mouth_width_delta)` contribution of each viseme.
    pub fn mouth_params(self) -> (f32, f32) {
        match self {
            RigViseme::Rest => (0.0, 0.0),
            RigViseme::Open => (0.85, 0.08),
            RigViseme::Wide => (0.38, 0.55),
            RigViseme::Round => (0.68, -0.35),
            RigViseme::Closed => (-0.12, 0.05),
            RigViseme::Fv => (0.18, 0.15),
            RigViseme::L => (0.42, 0.12),
            RigViseme::Wq => (0.28, -0.52),
            RigViseme::D => (0.26, 0.24),
            RigViseme::R => (0.32, -0.28),
            RigViseme::S => (0.14, 0.42),
            RigViseme::Uh => (0.52, 0.04),
        }
    }
}

/// Independent facial & lip-sync parameters keyed on timeline keyframes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FacePose {
    #[serde(default, skip_serializing_if = "is_zero_vec")]
    pub eye_gaze: Vec2,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub eyebrow_l: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub eyebrow_r: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub eyebrow_tilt_l: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub eyebrow_tilt_r: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub blink_l: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub blink_r: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub jaw_open: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub mouth_smile: f32,
    #[serde(default, skip_serializing_if = "is_rest_viseme")]
    pub viseme: RigViseme,
}

fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_vec(v: &Vec2) -> bool {
    *v == Vec2::ZERO
}

fn is_rest_viseme(v: &RigViseme) -> bool {
    *v == RigViseme::Rest
}

impl FacePose {
    pub fn is_default(&self) -> bool {
        *self == FacePose::default()
    }

    pub fn sanitized(self) -> FacePose {
        let cl = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        FacePose {
            eye_gaze: Vec2::new(cl(self.eye_gaze.x, -1.0, 1.0), cl(self.eye_gaze.y, -1.0, 1.0)),
            eyebrow_l: cl(self.eyebrow_l, -1.0, 1.0),
            eyebrow_r: cl(self.eyebrow_r, -1.0, 1.0),
            eyebrow_tilt_l: cl(self.eyebrow_tilt_l, -1.0, 1.0),
            eyebrow_tilt_r: cl(self.eyebrow_tilt_r, -1.0, 1.0),
            blink_l: cl(self.blink_l, 0.0, 1.0),
            blink_r: cl(self.blink_r, 0.0, 1.0),
            jaw_open: cl(self.jaw_open, 0.0, 1.0),
            mouth_smile: cl(self.mouth_smile, -1.0, 1.0),
            viseme: self.viseme,
        }
    }

    pub fn lerp(&self, o: &FacePose, t: f32) -> FacePose {
        use anicraft_geom::lerp;
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        FacePose {
            eye_gaze: self.eye_gaze.lerp(o.eye_gaze, t),
            eyebrow_l: lerp(self.eyebrow_l, o.eyebrow_l, t),
            eyebrow_r: lerp(self.eyebrow_r, o.eyebrow_r, t),
            eyebrow_tilt_l: lerp(self.eyebrow_tilt_l, o.eyebrow_tilt_l, t),
            eyebrow_tilt_r: lerp(self.eyebrow_tilt_r, o.eyebrow_tilt_r, t),
            blink_l: lerp(self.blink_l, o.blink_l, t),
            blink_r: lerp(self.blink_r, o.blink_r, t),
            jaw_open: lerp(self.jaw_open, o.jaw_open, t),
            mouth_smile: lerp(self.mouth_smile, o.mouth_smile, t),
            // Visemes switch cleanly like mouth frames while continuous jaw/lip blend interpolates
            viseme: if t < 0.5 { self.viseme } else { o.viseme },
        }
    }
}

/// Complete character rig definition stored on an Armature Layer and in `Document::character_rigs`
/// for instant reuse across scenes and animations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterRig {
    pub id: RigId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_shape: Option<ElementId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitmap_id: Option<BitmapId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<SymbolId>,
    pub source_kind: RigSourceKind,
    pub character_style: CharacterStyle,
    pub view_angle: ViewAngle,
    pub image_size: [u32; 2],
    pub bounds_px: Rect,
    pub overall_confidence: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub joints: Vec<RigJoint>,
    pub bones: Vec<RigBoneBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_mesh: Option<SkinnedMesh>,
}

impl CharacterRig {
    pub fn joint(&self, id: &str) -> Option<&RigJoint> {
        self.joints.iter().find(|j| j.id == id && j.present)
    }

    pub fn joint_any(&self, id: &str) -> Option<&RigJoint> {
        self.joints.iter().find(|j| j.id == id)
    }

    pub fn joint_mut(&mut self, id: &str) -> Option<&mut RigJoint> {
        self.joints.iter_mut().find(|j| j.id == id)
    }

    /// Converts a pixel-space point `p` into normalized `[0..=1]` character-bounds UV.
    pub fn to_norm_uv(&self, p: Vec2) -> Vec2 {
        let bw = (self.bounds_px.max.x - self.bounds_px.min.x).max(1.0);
        let bh = (self.bounds_px.max.y - self.bounds_px.min.y).max(1.0);
        Vec2::new(
            ((p.x - self.bounds_px.min.x) / bw).clamp(0.0, 1.0),
            ((p.y - self.bounds_px.min.y) / bh).clamp(0.0, 1.0),
        )
    }

    /// Maps normalized `[0..=1]` UV back into pixel coordinates for a target bounding box.
    pub fn from_norm_uv(uv: Vec2, bounds: Rect) -> Vec2 {
        let bw = (bounds.max.x - bounds.min.x).max(1.0);
        let bh = (bounds.max.y - bounds.min.y).max(1.0);
        Vec2::new(bounds.min.x + uv.x * bw, bounds.min.y + uv.y * bh)
    }
}

/// Evaluates the interpolated `FacePose` on `layer` at `frame` using keyframe easing.
pub fn face_pose_at(layer: &Layer, frame: u32) -> FacePose {
    let Some(idx) = layer.keyframe_index_at(frame) else {
        return FacePose::default();
    };
    let Some(kf): Option<&Arc<Keyframe>> = layer.keyframes.get(idx) else {
        return FacePose::default();
    };
    let here = kf.face_pose.unwrap_or_default().sanitized();
    let Some(tween) = &kf.tween else {
        return here;
    };
    let Some(next) = layer.keyframes.get(idx + 1) else {
        return here;
    };
    if next.start != kf.end() || frame == kf.start {
        return here;
    }
    let t = tween.easing.apply((frame - kf.start) as f32 / kf.duration.max(1) as f32);
    here.lerp(&next.face_pose.unwrap_or_default().sanitized(), t)
}

/// Rigid 2D bone segment mapping `(rest_head, rest_tail) -> (posed_head, posed_tail)` with unit rotor `(cos, sin)`.
#[derive(Clone, Copy, Debug)]
struct RigidBone {
    rest_head: Vec2,
    posed_head: Vec2,
    cos: f32,
    sin: f32,
    moved: bool,
}

impl RigidBone {
    fn from_segments(rest_head: Vec2, rest_tail: Vec2, posed_head: Vec2, posed_tail: Vec2) -> Self {
        let u = rest_tail - rest_head;
        let v = posed_tail - posed_head;
        let ul = u.length();
        let vl = v.length();
        if ul < 1e-5 || vl < 1e-5 {
            let d = posed_head - rest_head;
            return RigidBone {
                rest_head,
                posed_head,
                cos: 1.0,
                sin: 0.0,
                moved: d.length() > 1e-4,
            };
        }
        let ux = u.x / ul;
        let uy = u.y / ul;
        let vx = v.x / vl;
        let vy = v.y / vl;
        let cos = ux * vx + uy * vy;
        let sin = ux * vy - uy * vx;
        let moved = (posed_head - rest_head).length() > 1e-4
            || (cos - 1.0).abs() > 1e-6
            || sin.abs() > 1e-6;
        RigidBone { rest_head, posed_head, cos, sin, moved }
    }

    #[inline]
    fn apply(&self, p: Vec2) -> Vec2 {
        let r = p - self.rest_head;
        Vec2::new(
            self.posed_head.x + self.cos * r.x - self.sin * r.y,
            self.posed_head.y + self.sin * r.x + self.cos * r.y,
        )
    }
}

/// Smooth compact-support `C^2` Wendland radial basis kernel `φ(q) = (1 - q)^4 (4q + 1)` for `q ∈ [0, 1]`.
#[inline]
fn wendland_c2(dist: f32, radius: f32) -> f32 {
    if radius <= 1e-4 || dist >= radius {
        return 0.0;
    }
    let q = (dist / radius).clamp(0.0, 1.0);
    let omq = 1.0 - q;
    let omq2 = omq * omq;
    omq2 * omq2 * (4.0 * q + 1.0)
}

/// Applies localized facial & lip-sync offsets in rest head space BEFORE head/neck/body bone skinning,
/// so blinks, brow raises, eye gaze, jaw opening, and visemes ride seamlessly with any head rotation!
fn apply_facial_rest_offset(rig: &CharacterRig, face: &FacePose, p: Vec2) -> Vec2 {
    if face.is_default() {
        return p;
    }
    let Some(head_j) = rig.joint("head") else {
        return p;
    };
    let head_h = rig
        .joint("neck")
        .and_then(|n| rig.joint("head_tip").map(|t| (n.pos_px - t.pos_px).length()))
        .unwrap_or((rig.bounds_px.max.y - rig.bounds_px.min.y) * 0.14)
        .max(24.0);

    // Quick bounding-sphere reject outside head region
    if (p - head_j.pos_px).length() > head_h * 1.45 {
        return p;
    }

    let mut delta = Vec2::ZERO;

    // 1. Independent Left & Right Eye Gaze + Eyelid Blink + Eyebrow Raise/Tilt
    for (side, blink, brow_raise, brow_tilt) in [
        ("l", face.blink_l, face.eyebrow_l, face.eyebrow_tilt_l),
        ("r", face.blink_r, face.eyebrow_r, face.eyebrow_tilt_r),
    ] {
        if let Some(eye) = rig.joint(&format!("eye_{side}")) {
            let eye_r = (head_h * 0.15).max(8.0);
            // Eye gaze shifts iris/pupil core subtly within socket
            if face.eye_gaze != Vec2::ZERO {
                let w_gaze = wendland_c2((p - eye.pos_px).length(), eye_r * 0.72);
                delta = delta + face.eye_gaze * (eye_r * 0.28 * w_gaze);
            }
            // Eyelid blink brings upper & lower eyelids smoothly together toward eye center line
            if blink > 0.001 {
                let up_pt = rig
                    .joint(&format!("eyelid_upper_{side}"))
                    .map_or(Vec2::new(eye.pos_px.x, eye.pos_px.y - eye_r * 0.42), |j| j.pos_px);
                let lo_pt = rig
                    .joint(&format!("eyelid_lower_{side}"))
                    .map_or(Vec2::new(eye.pos_px.x, eye.pos_px.y + eye_r * 0.42), |j| j.pos_px);
                let gap = (lo_pt.y - up_pt.y).abs().max(eye_r * 0.55);
                let w_up = wendland_c2((p - up_pt).length(), eye_r * 0.95);
                let w_lo = wendland_c2((p - lo_pt).length(), eye_r * 0.85);
                delta.y += blink * (gap * 0.68) * w_up;
                delta.y -= blink * (gap * 0.28) * w_lo;
            }
        }
        if (brow_raise.abs() > 0.001 || brow_tilt.abs() > 0.001)
            && let Some(brow) = rig.joint(&format!("eyebrow_{side}"))
        {
            let brow_r = (head_h * 0.20).max(10.0);
            let w_b = wendland_c2((p - brow.pos_px).length(), brow_r);
            if w_b > 0.0 {
                let horiz = (p.x - brow.pos_px.x) / brow_r;
                let sign = if side == "l" { 1.0 } else { -1.0 };
                delta.y += (-brow_raise * head_h * 0.075 + brow_tilt * sign * horiz * head_h * 0.055) * w_b;
            }
        }
    }

    // 2. Jaw, Lips, Smile & Lip-Sync Viseme Deformation (smooth elliptical C^2 jaw field, zero V-beak pinching)
    let (vis_jaw, vis_wide) = face.viseme.mouth_params();
    let total_jaw = (if face.viseme != RigViseme::Rest {
        face.jaw_open.max(vis_jaw)
    } else {
        face.jaw_open
    })
    .clamp(-0.12, 1.0);
    let total_wide = (face.mouth_smile * 0.55 + vis_wide * 0.60).clamp(-0.75, 0.85);

    if (total_jaw.abs() > 0.001 || total_wide.abs() > 0.001 || face.mouth_smile.abs() > 0.001)
        && let Some(mouth) = rig.joint("mouth_center")
    {
        let mouth_rx = (head_h * 0.34).max(16.0);
        let mouth_ry = (head_h * 0.28).max(14.0);
        let lip_lo = rig
            .joint("lip_lower")
            .map_or(Vec2::new(mouth.pos_px.x, mouth.pos_px.y + mouth_ry * 0.18), |j| j.pos_px);
        let chin = rig
            .joint("chin")
            .map_or(Vec2::new(mouth.pos_px.x, mouth.pos_px.y + mouth_ry * 0.72), |j| j.pos_px);
        let jaw_mid = (lip_lo + chin) * 0.5;

        // Wide elliptical distance around lower jaw so the jaw opens as a smooth rounded arc
        let ellip_d = Vec2::new(
            (p.x - jaw_mid.x) / mouth_rx,
            (p.y - jaw_mid.y) / mouth_ry,
        )
        .length();
        let w_jaw = wendland_c2(ellip_d, 1.0);

        // Smooth C^1 transition across mouth parting line (0 above upper lip, 1 across lower lip & chin)
        let ramp_t = ((p.y - (mouth.pos_px.y - mouth_ry * 0.12)) / (mouth_ry * 0.42)).clamp(0.0, 1.0);
        let lower_gate = ramp_t * ramp_t * (3.0 - 2.0 * ramp_t);

        let open_px = head_h * 0.056 * total_jaw;
        delta.y += open_px * w_jaw * lower_gate;

        // Subtle upper-lip lift on wide/open vowels
        let ellip_up = Vec2::new(
            (p.x - mouth.pos_px.x) / (mouth_rx * 0.75),
            (p.y - (mouth.pos_px.y - mouth_ry * 0.18)) / (mouth_ry * 0.55),
        )
        .length();
        let w_up = wendland_c2(ellip_up, 1.0) * (1.0 - lower_gate);
        delta.y -= open_px * 0.16 * w_up;

        // Mouth corners widen/narrow (EE/Smile vs OH/OO purse) and curve upward on smile
        for (corner_id, dir_x) in [("mouth_r", -1.0f32), ("mouth_l", 1.0f32)] {
            let c_pt = rig
                .joint(corner_id)
                .map_or(Vec2::new(mouth.pos_px.x + dir_x * mouth_rx * 0.42, mouth.pos_px.y), |j| {
                    j.pos_px
                });
            let wc = wendland_c2((p - c_pt).length(), mouth_rx * 0.72);
            if wc > 0.0 {
                delta.x += dir_x * total_wide * (head_h * 0.042) * wc;
                delta.y -= face.mouth_smile * (head_h * 0.032) * wc;
            }
        }
    }

    p + delta
}

/// Evaluates Dual-Quaternion / 2D Unit-Rotor Skinning + Facial Pose on `rig`'s `SkinnedMesh`
/// under `arm` and `pose`. Returns `None` when completely at rest (zero overhead).
pub fn deform_skinned_mesh(
    arm: &Armature,
    elements: &[Arc<Element>],
    pose: &Pose,
    face: &FacePose,
) -> Option<(ElementId, WarpMesh)> {
    let rig = arm.char_rig.as_deref()?;
    let target_id = rig.target_shape?;
    let skin_mesh = rig.skin_mesh.as_ref()?;
    if skin_mesh.rest_vertices.is_empty() || skin_mesh.triangles.is_empty() {
        return None;
    }

    // Gather posed bone segments in layer space, then convert into target shape's local space
    let target_xf_inv = elements
        .iter()
        .find(|e| e.id == target_id)
        .and_then(|e| e.transform.inverse())
        .unwrap_or(anicraft_geom::Affine::IDENTITY);

    let posed_at: HashMap<BoneId, (Vec2, Vec2)> = armature::bones_at(arm, elements, pose)
        .into_iter()
        .map(|b| (b.id, (target_xf_inv.apply(b.head), target_xf_inv.apply(b.tail))))
        .collect();

    let rest_map: HashMap<BoneId, (Vec2, Vec2)> = rig
        .bones
        .iter()
        .filter_map(|rb| rb.bone_id.map(|bid| (bid, (rb.head, rb.tail))))
        .collect();

    let mut rigid_bones: Vec<RigidBone> = Vec::with_capacity(skin_mesh.deform_bones.len());
    let mut any_bone_moved = false;
    for &bid in &skin_mesh.deform_bones {
        let (rh, rt) = rest_map.get(&bid).copied().unwrap_or((Vec2::ZERO, Vec2::new(1.0, 0.0)));
        let (ph, pt) = posed_at.get(&bid).copied().unwrap_or((rh, rt));
        let rb = RigidBone::from_segments(rh, rt, ph, pt);
        any_bone_moved |= rb.moved;
        rigid_bones.push(rb);
    }

    if !any_bone_moved && face.is_default() {
        return None;
    }

    let mut shown = Vec::with_capacity(skin_mesh.rest_vertices.len());
    let identity_bone = RigidBone {
        rest_head: Vec2::ZERO,
        posed_head: Vec2::ZERO,
        cos: 1.0,
        sin: 0.0,
        moved: false,
    };

    for (vi, &rest_v) in skin_mesh.rest_vertices.iter().enumerate() {
        // Apply localized facial blendshape offset in rest space first so it rotates with the head bone!
        let v = apply_facial_rest_offset(rig, face, rest_v);
        let influences = skin_mesh.weights.get(vi).map_or(&[][..], Vec::as_slice);
        if influences.is_empty() {
            shown.push(v);
            continue;
        }
        if influences.len() == 1 {
            let b = rigid_bones.get(usize::from(influences[0].0)).unwrap_or(&identity_bone);
            shown.push(b.apply(v));
            continue;
        }

        // Multi-bone joint blend: combine Linear Blend Skinning with 2D Dual-Quaternion radial preservation
        // so bent elbows and knees keep 100% of their limb thickness without pinching or tearing!
        let mut p_lbs = Vec2::ZERO;
        let mut pivot_blend = Vec2::ZERO;
        let mut radial_vec = Vec2::ZERO;
        let mut target_radius = 0.0f32;
        let mut w_sum = 0.0f32;

        for &VertexWeight(bi, w) in influences {
            if w <= 0.0 || !w.is_finite() {
                continue;
            }
            let b = rigid_bones.get(usize::from(bi)).unwrap_or(&identity_bone);
            let tp = b.apply(v);
            let u_vec = tp - b.posed_head;
            p_lbs = p_lbs + tp * w;
            pivot_blend = pivot_blend + b.posed_head * w;
            radial_vec = radial_vec + u_vec * w;
            target_radius += w * u_vec.length();
            w_sum += w;
        }

        if w_sum <= 1e-6 {
            shown.push(v);
            continue;
        }
        p_lbs = p_lbs * (1.0 / w_sum);
        pivot_blend = pivot_blend * (1.0 / w_sum);
        radial_vec = radial_vec * (1.0 / w_sum);
        target_radius /= w_sum;

        let cur_r = radial_vec.length();
        let out_p = if cur_r > 1e-3 && cur_r >= target_radius * 0.40 && target_radius > cur_r {
            // Restore up to 30% of lost rotational radius across sharp joint bends without exploding opposing vectors
            let restored_r = cur_r + 0.65 * (target_radius - cur_r);
            let scale = (restored_r / cur_r).clamp(1.0, 1.30);
            let p_dqs = pivot_blend + radial_vec * scale;
            p_lbs.lerp(p_dqs, 0.75)
        } else {
            p_lbs
        };
        shown.push(out_p);
    }

    Some((
        target_id,
        WarpMesh {
            rest: skin_mesh.rest_vertices.clone(),
            shown,
            tris: skin_mesh.triangles.clone(),
        },
    ))
}

/// Deforms a vector `Path` through a `WarpMesh` via piecewise barycentric triangle interpolation
/// (used when rigging vector `<svg>` shapes like Stickmen!).
pub fn deform_path_by_mesh(rest_path: &Path, mesh: &WarpMesh) -> Arc<Path> {
    if mesh.tris.is_empty() || mesh.rest.len() != mesh.shown.len() {
        return Arc::new(rest_path.clone());
    }
    let map_pt = |p: Vec2| -> Vec2 {
        let mut best_tri: Option<(f32, Vec2)> = None;
        for &[i0, i1, i2] in &mesh.tris {
            let (Some(&a), Some(&b), Some(&c)) = (
                mesh.rest.get(i0 as usize),
                mesh.rest.get(i1 as usize),
                mesh.rest.get(i2 as usize),
            ) else {
                continue;
            };
            let (Some(&sa), Some(&sb), Some(&sc)) = (
                mesh.shown.get(i0 as usize),
                mesh.shown.get(i1 as usize),
                mesh.shown.get(i2 as usize),
            ) else {
                continue;
            };
            let det = (b - a).cross(c - a);
            if det.abs() <= 1e-8 {
                continue;
            }
            let l1 = (p - a).cross(c - a) / det;
            let l2 = (b - a).cross(p - a) / det;
            let l0 = 1.0 - l1 - l2;
            let mapped = sa * l0 + sb * l1 + sc * l2;
            if l0 >= -1e-4 && l1 >= -1e-4 && l2 >= -1e-4 {
                return mapped;
            }
            let excess = (-l0).max(0.0) + (-l1).max(0.0) + (-l2).max(0.0);
            if best_tri.is_none_or(|(best_e, _)| excess < best_e) {
                best_tri = Some((excess, mapped));
            }
        }
        best_tri.map_or(p, |(_, m)| m)
    };

    let cmds = rest_path
        .cmds
        .iter()
        .map(|cmd| match *cmd {
            PathCmd::MoveTo(p) => PathCmd::MoveTo(map_pt(p)),
            PathCmd::LineTo(p) => PathCmd::LineTo(map_pt(p)),
            PathCmd::QuadTo(q, p) => PathCmd::QuadTo(map_pt(q), map_pt(p)),
            PathCmd::CubicTo(c1, c2, p) => PathCmd::CubicTo(map_pt(c1), map_pt(c2), map_pt(p)),
            PathCmd::Close => PathCmd::Close,
        })
        .collect();
    Arc::new(Path { cmds })
}

/// Applies vector outline deformation when a `CharacterRig` targets a non-bitmap vector `Shape`.
pub fn apply_vector_rig(
    arm: &Armature,
    elements: &mut [Arc<Element>],
    pose: &Pose,
    face: &FacePose,
    rest_paths: &HashMap<ElementId, Arc<Path>>,
) -> bool {
    let Some((target_id, mesh)) = deform_skinned_mesh(arm, elements, pose, face) else {
        return false;
    };
    let Some(el) = elements.iter_mut().find(|e| e.id == target_id) else {
        return false;
    };
    let ElementKind::Shape(sh) = &el.kind else {
        return false;
    };
    if matches!(sh.fill, Some(crate::Paint::Bitmap { .. })) {
        // Bitmap shapes are deformed directly via `DisplayItem::warp` in `eval.rs`!
        return false;
    }
    let Some(rest) = rest_paths.get(&target_id) else {
        return false;
    };
    let bent = deform_path_by_mesh(rest, &mesh);
    if *sh.path == *bent {
        return false;
    }
    if let ElementKind::Shape(sh_mut) = &mut Arc::make_mut(el).kind {
        sh_mut.path = bent;
        return true;
    }
    false
}
