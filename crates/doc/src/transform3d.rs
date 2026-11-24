//! 3D Postcard rotation and depth translation on document elements (`P-044`).

use anicraft_geom::perspective3d::{DEFAULT_FOV_DEG, MAX_FOV_DEG, MIN_FOV_DEG, Projector3D};
use anicraft_geom::{Vec2, lerp};
use serde::{Deserialize, Serialize};

/// Largest 3D rotation angle stored on an element (degrees, unwrapped so multi-turn tweens spin).
pub const MAX_3D_ANGLE_DEG: f32 = 100_000.0;
/// Largest 3D Z translation in stage pixels (`+Z` recedes into screen, `-Z` approaches camera).
pub const MAX_3D_Z: f32 = 100_000.0;
/// Largest vanishing-point coordinate on either axis.
pub const MAX_VANISHING: f32 = 1_000_000.0;

fn default_fov() -> f32 {
    DEFAULT_FOV_DEG
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_default_fov(v: &f32) -> bool {
    (*v - DEFAULT_FOV_DEG).abs() <= 1e-4
}

/// 3D Postcard orientation, Z depth, field of view, and optional stage vanishing point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform3D {
    /// Pitch around the horizontal X axis, in degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rx_deg: f32,
    /// Yaw around the vertical Y axis, in degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ry_deg: f32,
    /// Roll around the optical Z axis, in degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rz_deg: f32,
    /// Translation along Z in stage pixels (`> 0` farther away, `< 0` closer to viewer).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tz: f32,
    /// Perspective camera field-of-view in degrees (`10..=150`, default `55`).
    #[serde(default = "default_fov", skip_serializing_if = "is_default_fov")]
    pub fov_deg: f32,
    /// Custom stage-space vanishing point (`None` uses the stage centre).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vanishing: Option<Vec2>,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            rx_deg: 0.0,
            ry_deg: 0.0,
            rz_deg: 0.0,
            tz: 0.0,
            fov_deg: DEFAULT_FOV_DEG,
            vanishing: None,
        }
    }
}

impl Transform3D {
    /// True when every field is at its rest default (nothing to serialize).
    pub fn is_identity(&self) -> bool {
        let s = self.sanitized();
        s.rx_deg.abs() <= 1e-5
            && s.ry_deg.abs() <= 1e-5
            && s.rz_deg.abs() <= 1e-5
            && s.tz.abs() <= 1e-4
            && (s.fov_deg - DEFAULT_FOV_DEG).abs() <= 1e-4
            && s.vanishing.is_none()
    }

    /// True when this 3D transform visibly alters geometry (non-zero rotation or depth).
    pub fn has_effect(&self) -> bool {
        let s = self.sanitized();
        s.rx_deg.abs() > 1e-5 || s.ry_deg.abs() > 1e-5 || s.rz_deg.abs() > 1e-5 || s.tz.abs() > 1e-4
    }

    /// Clamps every field into its valid finite range.
    pub fn sanitized(&self) -> Self {
        let clean = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let vanishing = self.vanishing.filter(|v| v.is_finite() && v.x.abs() <= MAX_VANISHING && v.y.abs() <= MAX_VANISHING);
        Self {
            rx_deg: clean(self.rx_deg, -MAX_3D_ANGLE_DEG, MAX_3D_ANGLE_DEG, 0.0),
            ry_deg: clean(self.ry_deg, -MAX_3D_ANGLE_DEG, MAX_3D_ANGLE_DEG, 0.0),
            rz_deg: clean(self.rz_deg, -MAX_3D_ANGLE_DEG, MAX_3D_ANGLE_DEG, 0.0),
            tz: clean(self.tz, -MAX_3D_Z, MAX_3D_Z, 0.0),
            fov_deg: clean(self.fov_deg, MIN_FOV_DEG, MAX_FOV_DEG, DEFAULT_FOV_DEG),
            vanishing,
        }
    }

    /// Interpolates smoothly between `self` and `other` at progress `t`.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let a = self.sanitized();
        let b = other.sanitized();
        let t = if t.is_finite() { t } else { 0.0 };
        let vanishing = match (a.vanishing, b.vanishing) {
            (Some(va), Some(vb)) => Some(va.lerp(vb, t)),
            (Some(v), None) | (None, Some(v)) => Some(v),
            (None, None) => None,
        };
        Self {
            rx_deg: lerp(a.rx_deg, b.rx_deg, t),
            ry_deg: lerp(a.ry_deg, b.ry_deg, t),
            rz_deg: lerp(a.rz_deg, b.rz_deg, t),
            tz: lerp(a.tz, b.tz, t),
            fov_deg: lerp(a.fov_deg, b.fov_deg, t),
            vanishing,
        }
        .sanitized()
    }

    /// Builds a [`Projector3D`] around `pivot` on a stage of `stage_w × stage_h`.
    pub fn projector(&self, pivot: Vec2, stage_w: f32, stage_h: f32) -> Projector3D {
        let s = self.sanitized();
        Projector3D::new(s.rx_deg, s.ry_deg, s.rz_deg, s.tz, s.fov_deg, pivot, s.vanishing, stage_w, stage_h)
    }
}
