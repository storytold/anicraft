//! Armatures (ADR 0043): bones that join symbol instances on an armature layer, poses that
//! turn them, and inverse kinematics that finds a pose from a dragged bone end.
//!
//! An armature is a tree of [`Bone`]s on a [`LayerKind::Armature`](crate::LayerKind) layer.
//! Each bone starts at a **joint** on its *parent* instance (its head) and ends on its *child*
//! instance (its tail); the child turns about the joint, carrying everything below it. An
//! instance is the child of at most one bone, and the instance no bone ends in is the root: it
//! keeps its own transform, so moving it moves the whole armature.
//!
//! A **pose** is a joint angle per bone ([`Pose`]), stored on each keyframe of the armature
//! layer and tweened between keyframes with the keyframe's easing. Angles are relative: 0 is the
//! bone as it was drawn, measured against its parent, so turning a parent carries its children.
//! Forward kinematics ([`place`]) turns a pose into the instances' transforms; it is pure, and
//! [`crate::evaluate_layer_elements`] applies it, so every renderer, export and the player see
//! the posed instances.
//!
//! Inverse kinematics ([`solve`]) is cyclic coordinate descent (CCD): sweeping from the dragged
//! bone up its chain, each joint turns so the effector points at the target, clamped to the
//! joint's limits. It works on joint angles directly, so limits are a clamp per step and the
//! result is a pose by construction.
//!
//! A joint may also **slide** (ADR 0084, Animate's joint translation): along its parent's x and
//! y axes, within limits, measured in layer pixels. A pose then holds the slide next to the
//! angle, forward kinematics moves the joint (and what hangs from it) by it, and the solver
//! slides joints only where turning them can't reach. **Springs** (per-bone strength and
//! damping) add secondary motion on top of the keyed pose; see [`crate::spring`].
//!
//! An armature may be **mixed** (ADR 0098): its bones move symbol instances and whole shapes
//! rigidly (their transforms), and bones *inside* a shape ([`Bone::shape`]) bend its outline
//! through that shape's [`Skin`]. The two kinds join freely: a bone inside a shape may carry an
//! instance on its end, and an instance's bone may end in a shape that has bones of its own.
//! One pose, one solver and one tween serve all of it.

use crate::skin::{self, Skin};
use crate::{Element, ElementId, Keyframe, Layer};
use anicraft_geom::{Affine, Vec2};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Most bones in one armature.
pub const MAX_BONES: usize = 256;
/// Longest bone name kept (bytes).
pub const MAX_NAME: usize = 256;
/// Joint limits stay within a turn either way (radians).
pub const MAX_LIMIT: f32 = std::f32::consts::TAU;
/// Largest joint angle kept (radians): many turns, never infinity.
pub const MAX_ANGLE: f32 = 64.0 * std::f32::consts::TAU;
/// IK stops once the effector is this close to its target (layer units, pixels).
pub const IK_TOLERANCE: f32 = 0.01;
/// Most CCD sweeps one solve takes.
pub const IK_SWEEPS: usize = 2048;
/// How often a stalled solve bends its joints to get going again, and by how much (radians).
const IK_KICKS: usize = 4;
const IK_KICK: f32 = 0.3;
/// Farthest a joint may slide either way along an axis (layer units, pixels).
pub const MAX_SLIDE: f32 = 10_000.0;

/// Unique id of a bone within a document (from the document's id counter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BoneId(pub u64);

/// How far a joint may turn either way from where the bone was drawn (radians, `min ≤ 0 ≤ max`
/// is usual but not required).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub min: f32,
    pub max: f32,
}

impl Limits {
    /// `a` kept inside the limits.
    pub fn clamp(&self, a: f32) -> f32 {
        a.clamp(self.min, self.max)
    }

    /// Finite, ordered and within [`MAX_LIMIT`]; `None` when nothing sane is left.
    pub fn sanitized(self) -> Option<Limits> {
        self.sanitized_within(MAX_LIMIT)
    }

    /// Finite, ordered and within `±bound`; `None` when nothing sane is left.
    pub fn sanitized_within(self, bound: f32) -> Option<Limits> {
        if !(self.min.is_finite() && self.max.is_finite()) {
            return None;
        }
        let (a, b) = (self.min.clamp(-bound, bound), self.max.clamp(-bound, bound));
        Some(Limits { min: a.min(b), max: a.max(b) })
    }
}

/// A bone's spring (ADR 0084): secondary motion that makes it lag behind and swing past the
/// pose it is keyed to when the bones it hangs from (or its own pose) move, then settle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    /// How stiff the spring is, 0…1 (shown as 0–100): 0 is no spring at all; a stiffer spring
    /// swings faster and lags less (from half a swing to ten swings a second).
    pub strength: f32,
    /// How quickly the swing dies down, 0…1 (shown as 0–100): 0 swings on, 1 absorbs all of it
    /// (the same as no spring).
    pub damping: f32,
}

impl Spring {
    /// Whether it moves anything: some strength, and damping that lets it swing.
    pub fn is_live(&self) -> bool {
        self.strength > 0.0 && self.damping < 1.0
    }

    /// Both values finite and within 0…1; `None` when they aren't numbers.
    pub fn sanitized(self) -> Option<Spring> {
        (self.strength.is_finite() && self.damping.is_finite()).then(|| Spring { strength: self.strength.clamp(0.0, 1.0), damping: self.damping.clamp(0.0, 1.0) })
    }
}

/// A bone: from a joint on its parent instance to its child instance, which it turns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub id: BoneId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The instance the bone starts on (the joint is a point on it).
    pub parent: ElementId,
    /// The instance the bone moves.
    pub child: ElementId,
    /// The joint, in the parent instance's local space: the child turns about it.
    pub head: Vec2,
    /// The bone's end, in the child instance's local space (where a drag pulls it).
    pub tail: Vec2,
    /// The child's transform in the parent's local space at joint angle 0 (as it was drawn).
    pub rest: Affine,
    /// How far the joint may turn; `None`: freely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// Pinned: inverse kinematics never turns it (nor the bones it hangs from), so its end stays
    /// where it is while the rest of the armature follows a drag.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// How far the joint may slide along its parent's x axis (pixels, ADR 0084); `None`: it
    /// doesn't slide that way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_x: Option<Limits>,
    /// How far the joint may slide along its parent's y axis (pixels); `None`: it doesn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_y: Option<Limits>,
    /// The bone's spring (ADR 0084); `None`: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spring: Option<Spring>,
    /// The shape this bone is inside (ADR 0071, ADR 0098): the bone bends that shape's outline
    /// through its [`Skin`], its `child` is a **joint node** (an id that is no element), and its
    /// `parent` is the shape itself or the node of another bone inside it. `None`: the bone
    /// moves an element (a symbol instance, or a whole shape) rigidly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<ElementId>,
}

impl Bone {
    /// A bone as drawn: turning freely, not sliding, no spring, not pinned.
    pub fn new(id: BoneId, parent: ElementId, child: ElementId, head: Vec2, tail: Vec2, rest: Affine) -> Bone {
        Bone { id, name: String::new(), parent, child, head, tail, rest, limits: None, pinned: false, slide_x: None, slide_y: None, spring: None, shape: None }
    }

    /// A bone inside `shape` as drawn (ADR 0071): from `parent` (the shape, or the joint node of
    /// another bone inside it) to a new joint node `child`, `head` and `tail` in the shape's
    /// space.
    pub fn in_shape(id: BoneId, shape: ElementId, parent: ElementId, child: ElementId, head: Vec2, tail: Vec2) -> Bone {
        Bone { shape: Some(shape), ..Bone::new(id, parent, child, head, tail, Affine::IDENTITY) }
    }

    /// Whether the joint may slide at all.
    pub fn slides(&self) -> bool {
        self.slide_x.is_some() || self.slide_y.is_some()
    }

    /// Slide `v` as this joint allows it: each axis within its limits, 0 along an axis it
    /// doesn't slide on.
    pub fn allow_slide(&self, v: Vec2) -> Vec2 {
        let axis = |l: Option<Limits>, x: f32| match l {
            Some(l) if x.is_finite() => l.clamp(x.clamp(-MAX_SLIDE, MAX_SLIDE)),
            Some(l) => l.clamp(0.0),
            None => 0.0,
        };
        Vec2::new(axis(self.slide_x, v.x), axis(self.slide_y, v.y))
    }

    /// Its spring, when it has one that moves anything.
    pub fn live_spring(&self) -> Option<Spring> {
        self.spring.filter(Spring::is_live)
    }

    /// `a` as this joint allows it (finite, within its limits).
    pub fn allow(&self, a: f32) -> f32 {
        let a = if a.is_finite() { a.clamp(-MAX_ANGLE, MAX_ANGLE) } else { 0.0 };
        self.limits.map_or(a, |l| l.clamp(a))
    }

    fn is_sane(&self) -> bool {
        self.parent != self.child && self.head.x.is_finite() && self.head.y.is_finite() && self.tail.x.is_finite() && self.tail.y.is_finite() && self.rest.is_finite() && self.rest.inverse().is_some()
    }
}

/// The bones of an armature layer, each listed after the bone it hangs from.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(from = "ArmatureIn")]
pub struct Armature {
    pub bones: Vec<Bone>,
    /// The shapes with bones inside them (ADR 0071), one [`Skin`] each: the bones marked with
    /// that shape ([`Bone::shape`]) bend its outline. Empty for an armature of instances only.
    pub skins: Vec<Skin>,
    /// The armature's springs are turned off (Properties › Bone › Springs, ADR 0084): it plays
    /// its keyed poses exactly, for posing without the swing.
    pub springs_off: bool,
    /// Optional Hybrid Automatic Character Rig bound to this armature layer.
    pub char_rig: Option<Arc<crate::autorig::CharacterRig>>,
}

/// An armature as files hold it. Two forms (ADR 0098):
///
/// * the **first form** (format versions up to 3): `bones`, and for a shape armature its one
///   `skin`; every bone of an armature with a `skin` is inside that shape;
/// * the **mixed form** (format version 4): `bones` that say which shape they are inside
///   (`shape`), and `skins`, one per shape with bones.
///
/// An armature the first form can say is always written in it, so documents without mixed
/// armatures are byte for byte what earlier builds wrote, and earlier builds open them.
#[derive(Deserialize)]
struct ArmatureIn {
    #[serde(default)]
    bones: Vec<Bone>,
    #[serde(default)]
    skin: Option<Skin>,
    #[serde(default)]
    skins: Vec<Skin>,
    #[serde(default)]
    springs_off: bool,
    #[serde(default)]
    char_rig: Option<Arc<crate::autorig::CharacterRig>>,
}

impl From<ArmatureIn> for Armature {
    fn from(a: ArmatureIn) -> Armature {
        let (mut bones, mut skins) = (a.bones, a.skins);
        if let Some(k) = a.skin {
            // The first form: every bone is inside the one shape.
            for b in &mut bones {
                b.shape.get_or_insert(k.shape);
            }
            skins.insert(0, k);
        }
        Armature { bones, skins, springs_off: a.springs_off, char_rig: a.char_rig }
    }
}

#[derive(Serialize)]
struct ArmatureOut<'a> {
    bones: Cow<'a, [Bone]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skin: Option<&'a Skin>,
    #[serde(skip_serializing_if = "<[Skin]>::is_empty")]
    skins: &'a [Skin],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    springs_off: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    char_rig: Option<&'a Arc<crate::autorig::CharacterRig>>,
}

impl Serialize for Armature {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let out = if self.is_mixed() {
            ArmatureOut { bones: Cow::Borrowed(&self.bones), skin: None, skins: &self.skins, springs_off: self.springs_off, char_rig: self.char_rig.as_ref() }
        } else {
            // The first form doesn't mark its bones: with a skin, all of them are inside it.
            let bones = if self.skins.is_empty() { Cow::Borrowed(self.bones.as_slice()) } else { Cow::Owned(self.bones.iter().map(|b| Bone { shape: None, ..b.clone() }).collect()) };
            ArmatureOut { bones, skin: self.skins.first(), skins: &[], springs_off: self.springs_off, char_rig: None }
        };
        out.serialize(ser)
    }
}

impl Armature {
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Whether the armature needs the mixed file form (ADR 0098): more than one shape with
    /// bones, bones inside a shape next to bones that move elements, or a bound character rig.
    /// Otherwise it is an armature of instances (ADR 0043) or of one shape (ADR 0071), written as it always was.
    pub fn is_mixed(&self) -> bool {
        let only = self.skins.first().map(|k| k.shape);
        self.char_rig.is_some() || self.skins.len() > 1 || self.bones.iter().any(|b| b.shape != only)
    }

    /// The skin of `shape`, when the armature has bones inside it.
    pub fn skin(&self, shape: ElementId) -> Option<&Skin> {
        self.skins.iter().find(|k| k.shape == shape)
    }

    pub fn skin_mut(&mut self, shape: ElementId) -> Option<&mut Skin> {
        self.skins.iter_mut().find(|k| k.shape == shape)
    }

    /// The skin bone `id` bends: its shape's, when the bone is inside a shape.
    pub fn skin_of(&self, id: BoneId) -> Option<&Skin> {
        self.skin(self.bone(id)?.shape?)
    }

    /// The bones inside `shape`, in armature order.
    pub fn bones_in(&self, shape: ElementId) -> impl Iterator<Item = &Bone> {
        self.bones.iter().filter(move |b| b.shape == Some(shape))
    }

    /// How many points of its shape bone `id` moves (0 for a bone that moves an element).
    pub fn bound_count(&self, id: BoneId) -> usize {
        self.skin_of(id).map_or(0, |k| k.points.iter().filter(|b| b.weight(id) > 0.0).count())
    }

    pub fn bone(&self, id: BoneId) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    pub fn bone_mut(&mut self, id: BoneId) -> Option<&mut Bone> {
        self.bones.iter_mut().find(|b| b.id == id)
    }

    /// The bone that moves instance `el` (ends in it), if any.
    /// Whether any spring moves the armature (springs on, a bone with a live spring).
    pub fn has_springs(&self) -> bool {
        !self.springs_off && self.bones.iter().any(|b| b.live_spring().is_some())
    }

    pub fn bone_into(&self, el: ElementId) -> Option<&Bone> {
        self.bones.iter().find(|b| b.child == el)
    }

    /// The bone `b` hangs from: the one that moves its parent instance.
    pub fn parent_bone(&self, b: &Bone) -> Option<&Bone> {
        self.bone_into(b.parent)
    }

    /// Whether `el` (an element, or a joint node) is part of the armature.
    pub fn contains(&self, el: ElementId) -> bool {
        self.bones.iter().any(|b| b.parent == el || b.child == el)
    }

    /// Every element in the armature (instances and shapes, never joint nodes), roots first.
    pub fn members(&self) -> Vec<ElementId> {
        let mut out: Vec<ElementId> = self.roots();
        for b in self.bones.iter().filter(|b| b.shape.is_none()) {
            if !out.contains(&b.child) {
                out.push(b.child);
            }
        }
        out
    }

    /// The elements no bone moves (one for an armature made with the Bone tool: an instance,
    /// or a shape with bones inside it).
    pub fn roots(&self) -> Vec<ElementId> {
        let mut out = Vec::new();
        for b in &self.bones {
            if self.bone_into(b.parent).is_none() && !out.contains(&b.parent) {
                out.push(b.parent);
            }
        }
        out
    }

    /// Bone `id` and every bone below it, in armature order.
    pub fn subtree(&self, id: BoneId) -> Vec<BoneId> {
        let mut inside: HashSet<ElementId> = HashSet::new();
        let mut out = Vec::new();
        for b in &self.bones {
            if b.id == id || inside.contains(&b.parent) {
                inside.insert(b.child);
                out.push(b.id);
            }
        }
        out
    }

    /// The chain from bone `id` up to its root, `id` first (bounded by the bone count).
    pub fn chain(&self, id: BoneId) -> Vec<&Bone> {
        let mut out = Vec::new();
        let mut cur = self.bone(id);
        while let Some(b) = cur {
            if out.len() >= self.bones.len() || out.iter().any(|o: &&Bone| o.id == b.id) {
                break;
            }
            out.push(b);
            cur = self.parent_bone(b);
        }
        out
    }

    /// Repairs a loaded armature: drops bones with non-finite numbers, repeated ids, a second
    /// bone into one element and bones in cycles; keeps every bone after its parent; keeps at
    /// most [`MAX_BONES`]. A bone inside a shape must hang from that shape or from another bone
    /// inside it, and the shape needs a skin; a skin needs bones, an outline within the point
    /// limit and one sane binding per point. Returns whether anything changed.
    pub fn sanitize(&mut self) -> bool {
        let before = self.clone();
        // One skin per shape, each within the point limit (plus any shape bound to a `CharacterRig`).
        let mut shapes = HashSet::new();
        self.skins.retain(|k| skin::anchors(&k.rest).len() <= skin::MAX_SKIN_POINTS && shapes.insert(k.shape));
        if let Some(rig_shape) = self.char_rig.as_ref().and_then(|r| r.target_shape) {
            shapes.insert(rig_shape);
        }
        let mut seen_id = HashSet::new();
        let mut seen_child = HashSet::new();
        let mut kept: Vec<Bone> = Vec::new();
        for mut b in std::mem::take(&mut self.bones) {
            // A joint node is no element: a bone inside a shape can't end on a shape with bones.
            let node_ok = b.shape.is_none_or(|s| shapes.contains(&s) && !shapes.contains(&b.child));
            if !b.is_sane() || !node_ok || !seen_id.insert(b.id) || !seen_child.insert(b.child) {
                continue;
            }
            b.limits = b.limits.and_then(Limits::sanitized);
            b.slide_x = b.slide_x.and_then(|l| l.sanitized_within(MAX_SLIDE));
            b.slide_y = b.slide_y.and_then(|l| l.sanitized_within(MAX_SLIDE));
            b.spring = b.spring.and_then(Spring::sanitized);
            if b.name.len() > MAX_NAME {
                let cut = (0..=MAX_NAME).rev().find(|&i| b.name.is_char_boundary(i)).unwrap_or(0);
                b.name.truncate(cut);
            }
            kept.push(b);
        }
        // Order: a bone after the bone moving its parent. What never gets placed is a cycle, or
        // a bone inside a shape that hangs from something outside it.
        let children: HashSet<ElementId> = kept.iter().map(|b| b.child).collect();
        let mut placed: HashSet<ElementId> = kept.iter().map(|b| b.parent).filter(|p| !children.contains(p)).collect();
        let mut node_in: HashMap<ElementId, ElementId> = HashMap::new();
        let mut ordered = Vec::with_capacity(kept.len());
        let mut rest = kept;
        loop {
            let (ready, wait): (Vec<Bone>, Vec<Bone>) = rest.into_iter().partition(|b| placed.contains(&b.parent) && b.shape.is_none_or(|s| b.parent == s || node_in.get(&b.parent) == Some(&s)));
            if ready.is_empty() {
                break;
            }
            for b in ready {
                placed.insert(b.child);
                if let Some(s) = b.shape {
                    node_in.insert(b.child, s);
                }
                ordered.push(b);
            }
            rest = wait;
        }
        ordered.truncate(MAX_BONES);
        self.bones = ordered;
        let boned: HashSet<ElementId> = self.bones.iter().filter_map(|b| b.shape).collect();
        self.skins.retain(|k| boned.contains(&k.shape));
        skin::sanitize(self);
        *self != before
    }
}

/// One joint in a pose: its angle (radians, 0 as drawn) and how far it slides along its
/// parent's axes (pixels, ADR 0084; 0 for a joint that doesn't slide).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointAngle {
    pub bone: BoneId,
    pub angle: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl JointAngle {
    pub fn new(bone: BoneId, angle: f32) -> JointAngle {
        JointAngle { bone, angle, x: 0.0, y: 0.0 }
    }

    pub fn slide(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

/// A pose: joint angles by bone. A bone it doesn't list is as drawn (angle 0).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pose(pub Vec<JointAngle>);

impl Pose {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The angle of bone `b` (0 when the pose doesn't list it).
    pub fn angle(&self, b: BoneId) -> f32 {
        self.0.iter().find(|j| j.bone == b).map_or(0.0, |j| j.angle)
    }

    pub fn set(&mut self, b: BoneId, angle: f32) {
        match self.0.iter_mut().find(|j| j.bone == b) {
            Some(j) => j.angle = angle,
            None => self.0.push(JointAngle::new(b, angle)),
        }
    }

    /// How far bone `b`'s joint slides (0 when the pose doesn't list it).
    pub fn slide(&self, b: BoneId) -> Vec2 {
        self.0.iter().find(|j| j.bone == b).map_or(Vec2::ZERO, JointAngle::slide)
    }

    pub fn set_slide(&mut self, b: BoneId, v: Vec2) {
        match self.0.iter_mut().find(|j| j.bone == b) {
            Some(j) => (j.x, j.y) = (v.x, v.y),
            None => self.0.push(JointAngle { bone: b, angle: 0.0, x: v.x, y: v.y }),
        }
    }

    /// Keeps only the bones in `keep`.
    pub fn retain(&mut self, keep: impl Fn(BoneId) -> bool) {
        self.0.retain(|j| keep(j.bone));
    }

    /// Every bone of `arm` at the angle its joint allows: the pose as it shows.
    pub fn allowed(&self, arm: &Armature) -> Pose {
        Pose(
            arm.bones
                .iter()
                .map(|b| {
                    let s = b.allow_slide(self.slide(b.id));
                    JointAngle { bone: b.id, angle: b.allow(self.angle(b.id)), x: s.x, y: s.y }
                })
                .collect(),
        )
    }

    /// The pose `t` of the way to `o`, joint by joint (angles and slides blend linearly, so a
    /// joint that turned a whole way round between keys turns that way in the tween).
    pub fn lerp(&self, o: &Pose, t: f32, arm: &Armature) -> Pose {
        Pose(
            arm.bones
                .iter()
                .map(|b| {
                    let s = b.allow_slide(self.slide(b.id).lerp(o.slide(b.id), t));
                    JointAngle { bone: b.id, angle: b.allow(anicraft_geom::lerp(self.angle(b.id), o.angle(b.id), t)), x: s.x, y: s.y }
                })
                .collect(),
        )
    }

    /// Repairs a loaded pose: finite angles in range, one per bone, at most [`MAX_BONES`].
    /// Returns whether anything changed.
    pub fn sanitize(&mut self) -> bool {
        let before = self.0.len();
        let mut changed = false;
        let mut seen = HashSet::new();
        self.0.retain(|j| seen.insert(j.bone));
        self.0.truncate(MAX_BONES);
        for j in &mut self.0 {
            let a = if j.angle.is_finite() { j.angle.clamp(-MAX_ANGLE, MAX_ANGLE) } else { 0.0 };
            if a.to_bits() != j.angle.to_bits() {
                j.angle = a;
                changed = true;
            }
            for v in [&mut j.x, &mut j.y] {
                let s = if v.is_finite() { v.clamp(-MAX_SLIDE, MAX_SLIDE) } else { 0.0 };
                if s.to_bits() != v.to_bits() {
                    *v = s;
                    changed = true;
                }
            }
        }
        changed || self.0.len() != before
    }
}

/// `R(angle)` about point `j`.
pub(crate) fn turn_about(j: Vec2, angle: f32) -> Affine {
    Affine::translate(j.x, j.y).mul(&Affine::rotate(angle)).mul(&Affine::translate(-j.x, -j.y))
}

/// Forward kinematics: the transform (in the layer's space) of every instance in `elements`
/// once `pose` turns the armature's joints. A root keeps its own transform; a child is placed
/// from its parent: `R_joint(angle) · parent · rest`. An instance whose parent isn't on the
/// keyframe keeps its own transform. A shape armature's joint nodes (ADR 0071) are placed
/// the same way from the shape's transform.
pub fn place(arm: &Armature, elements: &[Arc<Element>], pose: &Pose) -> HashMap<ElementId, Affine> {
    let mut world: HashMap<ElementId, Affine> = elements.iter().map(|e| (e.id, e.transform)).collect();
    for b in &arm.bones {
        let Some(wp) = world.get(&b.parent).copied() else { continue };
        // A joint node is no element: it is placed whether or not anything is on the keyframe.
        if b.shape.is_none() && !world.contains_key(&b.child) {
            continue;
        }
        let (_, wc) = joint(b, &wp, b.allow(pose.angle(b.id)), b.allow_slide(pose.slide(b.id)));
        if wc.is_finite() {
            world.insert(b.child, wc);
        }
    }
    world
}

/// The layer-space step a joint slides by `s` (pixels along its parent `wp`'s x and y axes).
/// Exactly zero for no slide.
pub fn slide_step(wp: &Affine, s: Vec2) -> Vec2 {
    if s == Vec2::ZERO {
        return Vec2::ZERO;
    }
    let (ex, ey) = slide_axes(wp);
    ex * s.x + ey * s.y
}

/// The unit directions a joint on parent `wp` slides along: its x and y axes in layer space.
pub fn slide_axes(wp: &Affine) -> (Vec2, Vec2) {
    let ex = wp.apply_vector(Vec2::new(1.0, 0.0)).normalized();
    let ey = wp.apply_vector(Vec2::new(0.0, 1.0)).normalized();
    (if ex == Vec2::ZERO { Vec2::new(1.0, 0.0) } else { ex }, if ey == Vec2::ZERO { Vec2::new(0.0, 1.0) } else { ey })
}

/// One joint of forward kinematics: where bone `b`'s joint is and the transform of what it
/// moves, from its parent's transform `wp`, joint angle `a` and slide `s` (both as the joint
/// allows them). Without a slide this is exactly ADR 0043's `R_joint(a) · wp · rest`.
pub fn joint(b: &Bone, wp: &Affine, a: f32, s: Vec2) -> (Vec2, Affine) {
    let d = slide_step(wp, s);
    let base = wp.mul(&b.rest);
    if d == Vec2::ZERO {
        let j = wp.apply(b.head);
        return (j, turn_about(j, a).mul(&base));
    }
    let j = wp.apply(b.head) + d;
    (j, turn_about(j, a).mul(&Affine::translate(d.x, d.y).mul(&base)))
}

/// Puts the elements a bone moves (instances, whole shapes) where `pose` has them, and bends
/// every shape with bones inside it (copy-on-write: only what changes is cloned). Returns
/// whether anything changed.
pub fn apply(arm: &Armature, elements: &mut [Arc<Element>], pose: &Pose) -> bool {
    if arm.is_empty() {
        return false;
    }
    let mut moved = false;
    // An armature of one shape (ADR 0071) moves no element: only its outline bends.
    if arm.bones.iter().any(|b| b.shape.is_none()) {
        let world = place(arm, elements, pose);
        for e in elements.iter_mut() {
            if arm.bone_into(e.id).is_none() {
                continue;
            }
            if let Some(m) = world.get(&e.id)
                && *m != e.transform
            {
                Arc::make_mut(e).transform = *m;
                moved = true;
            }
        }
    }
    skin::apply(arm, elements, pose) || moved
}

/// The pose layer `layer` has at `frame`: its covering keyframe's, or, inside a tween, the pose
/// between that keyframe's and the next one's at the tween's eased time. Exact at keyframes.
/// `None` without an armature or on an empty frame.
pub fn pose_at(layer: &Layer, frame: u32) -> Option<Pose> {
    let arm = layer.armature.as_deref().filter(|_| layer.kind == crate::LayerKind::Armature)?;
    let idx = layer.keyframe_index_at(frame)?;
    let kf: &Keyframe = layer.keyframes.get(idx)?;
    let here = kf.pose.allowed(arm);
    let (Some(tween), None) = (&kf.tween, &kf.motion) else { return Some(here) };
    let Some(next) = layer.keyframes.get(idx + 1) else { return Some(here) };
    if next.start != kf.end() || frame == kf.start {
        return Some(here);
    }
    let t = tween.easing.apply((frame - kf.start) as f32 / kf.duration.max(1) as f32);
    Some(kf.pose.lerp(&next.pose, t, arm))
}

/// Where a bone is drawn: its joint (head) and end (tail), in the layer's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneAt {
    pub id: BoneId,
    pub head: Vec2,
    pub tail: Vec2,
    /// The joint angle (radians, as the joint allows it).
    pub angle: f32,
    /// How far the joint slides (pixels along its parent's axes, as the joint allows it).
    pub slide: Vec2,
}

/// Every bone's joint and end with the instances in `elements` placed as they show (posed:
/// what [`crate::evaluate_layer_elements`] gives on an armature layer).
pub fn bones_at(arm: &Armature, elements: &[Arc<Element>], pose: &Pose) -> Vec<BoneAt> {
    // The elements are where they show. Joint nodes inside shapes are no elements: forward
    // kinematics places them from what they hang from.
    let mut world: HashMap<ElementId, Affine> = elements.iter().map(|e| (e.id, e.transform)).collect();
    for b in arm.bones.iter().filter(|b| b.shape.is_some()) {
        let Some(wp) = world.get(&b.parent).copied() else { continue };
        let (_, wc) = joint(b, &wp, b.allow(pose.angle(b.id)), b.allow_slide(pose.slide(b.id)));
        if wc.is_finite() {
            world.insert(b.child, wc);
        }
    }
    arm.bones
        .iter()
        .filter_map(|b| {
            let (wp, wc) = (world.get(&b.parent)?, world.get(&b.child)?);
            let slide = b.allow_slide(pose.slide(b.id));
            Some(BoneAt { id: b.id, head: wp.apply(b.head) + slide_step(wp, slide), tail: wc.apply(b.tail), angle: b.allow(pose.angle(b.id)), slide })
        })
        .collect()
}

/// Layer point `p` in the space of the instance (or joint node) bone `id` moves, with the
/// armature posed by `pose`: the `local` for [`solve`] that makes the grabbed point follow a drag
/// from where it is, so grabbing a bone along its length never snaps its end to the pointer.
pub fn grab_point(arm: &Armature, elements: &[Arc<Element>], pose: &Pose, id: BoneId, p: Vec2) -> Option<Vec2> {
    let child = arm.bone(id)?.child;
    let q = place(arm, elements, pose).get(&child)?.inverse()?.apply(p);
    (q.x.is_finite() && q.y.is_finite()).then_some(q)
}

/// What a solve came to.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// The new pose (every bone of the armature).
    pub pose: Pose,
    /// How far the effector ended from the target.
    pub error: f32,
    /// How many bones the drag could turn (0: the bone is pinned, or hangs from a pinned one).
    pub chain: usize,
}

/// The bones a drag of bone `id` may turn: `id` and the bones it hangs from, up to the first
/// that must stay put: a pinned bone, or one a pinned bone hangs from (turning it would move
/// the pinned bone).
pub fn movable_chain(arm: &Armature, id: BoneId) -> Vec<&Bone> {
    let frozen: HashSet<BoneId> = arm.bones.iter().filter(|b| b.pinned).flat_map(|p| arm.chain(p.id)).map(|b| b.id).collect();
    arm.chain(id).into_iter().take_while(|b| !frozen.contains(&b.id)).collect()
}

/// Inverse kinematics: the pose that brings point `local` of bone `id`'s child instance (its
/// local space; the bone's tail when dragging the bone end) to `target` (layer space), turning
/// only the bones [`movable_chain`] allows. `elements` are the keyframe's instances (the roots
/// at their own transforms), `pose` the pose to start from. Cyclic coordinate descent: each
/// sweep turns the joints from the dragged bone upward so the effector points at the target,
/// each clamped to its limits, until the effector is within [`IK_TOLERANCE`] or stops moving.
/// An unreachable target leaves the chain stretched straight toward it, the root where it was.
///
/// Joints that slide (ADR 0084) slide only where turning can't reach: when the turning sweeps
/// leave the effector short of the target, a second round of sweeps also slides each joint, after
/// turning it, toward the target within its limits. A target the joints reach by turning is
/// solved exactly as without sliding. `None` when the armature has no bone `id` or its instances
/// aren't on the keyframe.
pub fn solve(arm: &Armature, elements: &[Arc<Element>], pose: &Pose, id: BoneId, local: Vec2, target: Vec2) -> Option<Solved> {
    let bone = arm.bone(id)?;
    if !(local.x.is_finite() && local.y.is_finite() && target.x.is_finite() && target.y.is_finite()) {
        return None;
    }
    let chain: Vec<&Bone> = movable_chain(arm, id);
    let effector = |pose: &Pose| -> Option<(HashMap<ElementId, Affine>, Vec2)> {
        let world = place(arm, elements, pose);
        let e = world.get(&bone.child)?.apply(local);
        Some((world, e))
    };
    let start = pose.allowed(arm);
    effector(&start)?;
    let (mut pose, mut error) = ccd(arm, &chain, &effector, start, target, false);
    if error > IK_TOLERANCE && chain.iter().any(|b| b.slides()) {
        let (p, e) = ccd(arm, &chain, &effector, pose.clone(), target, true);
        if e < error {
            (pose, error) = (p, e);
        }
    }
    Some(Solved { pose, error, chain: chain.len() })
}

/// The effector (with every instance's placement) for a pose; see [`solve`].
type Effector<'a> = dyn Fn(&Pose) -> Option<(HashMap<ElementId, Affine>, Vec2)> + 'a;

/// Cyclic coordinate descent from `pose` toward `target`, turning (and with `slide`, sliding)
/// the joints of `chain`. Returns the best pose seen and its error.
fn ccd(arm: &Armature, chain: &[&Bone], effector: &Effector<'_>, mut pose: Pose, target: Vec2, slide: bool) -> (Pose, f32) {
    let chain: Vec<(BoneId, ElementId, Vec2)> = chain.iter().map(|b| (b.id, b.parent, b.head)).collect();
    let Some((_, start)) = effector(&pose) else { return (pose, f32::INFINITY) };
    let mut error = start.distance(target);
    let mut best = (pose.clone(), error);
    let mut kicks = 0;
    for _ in 0..IK_SWEEPS {
        if error < best.1 {
            best = (pose.clone(), error);
        }
        if error <= IK_TOLERANCE || chain.is_empty() {
            break;
        }
        let Some((world, mut e)) = effector(&pose) else { break };
        let mut turned = 0.0f32;
        // From the dragged bone up: turning a joint moves only what is below it, so the joints
        // above keep the places this sweep began with.
        for &(b, parent, head) in &chain {
            let Some(wp) = world.get(&parent) else { continue };
            let Some(joint) = arm.bone(b) else { continue };
            let j = wp.apply(head) + slide_step(wp, pose.slide(b));
            let (u, v) = (e - j, target - j);
            if u.length() >= 1e-6 && v.length() >= 1e-6 {
                let want = u.cross(v).atan2(u.dot(v));
                let cur = pose.angle(b);
                let next = joint.allow(cur + want);
                let by = next - cur;
                if by != 0.0 {
                    pose.set(b, next);
                    e = turn_about(j, by).apply(e);
                    turned = turned.max(by.abs());
                }
            }
            // Then slide the joint (and all below it) toward the target, along the axes it
            // slides on: the effector moves by the same step.
            if slide && joint.slides() {
                let (ex, ey) = slide_axes(wp);
                let r = target - e;
                let cur = pose.slide(b);
                let want = match (joint.slide_x.is_some(), joint.slide_y.is_some()) {
                    (true, false) => Vec2::new(r.dot(ex), 0.0),
                    (false, true) => Vec2::new(0.0, r.dot(ey)),
                    _ => {
                        // r = ex·sx + ey·sy (the axes may be skewed).
                        let det = ex.cross(ey);
                        if det.abs() < 1e-6 { Vec2::new(r.dot(ex), r.dot(ey)) } else { Vec2::new(r.cross(ey) / det, ex.cross(r) / det) }
                    }
                };
                let next = joint.allow_slide(cur + want);
                let by = next - cur;
                if by != Vec2::ZERO {
                    pose.set_slide(b, next);
                    e = e + ex * by.x + ey * by.y;
                    turned = turned.max(by.length() * 1e-2);
                }
            }
        }
        let Some((_, e)) = effector(&pose) else { break };
        let was = error;
        error = e.distance(target);
        // Stuck: limits, a chain stretched straight at an unreachable target, or CCD's blind
        // spot, a straight chain whose target lies on its own line (every joint sees angle 0).
        // A few kicks bend the joints a little and descent goes on; the best pose seen wins.
        if turned < 1e-7 || ((was - error).abs() < 1e-6 * was.max(1.0) && turned < 1e-4) {
            if error < best.1 {
                best = (pose.clone(), error);
            }
            if kicks >= IK_KICKS {
                break;
            }
            kicks += 1;
            for (i, &(b, _, _)) in chain.iter().enumerate() {
                let Some(joint) = arm.bone(b) else { continue };
                let cur = pose.angle(b);
                let kick = IK_KICK * if (i + kicks) % 2 == 0 { 1.0 } else { -1.0 };
                let next = joint.allow(cur + kick);
                pose.set(b, if next != cur { next } else { joint.allow(cur - kick) });
            }
            error = effector(&pose).map_or(error, |(_, e)| e.distance(target));
            continue;
        }
    }
    if error < best.1 {
        best = (pose, error);
    }
    best
}

#[cfg(test)]
#[path = "armature_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mixed_tests.rs"]
mod mixed_tests;
