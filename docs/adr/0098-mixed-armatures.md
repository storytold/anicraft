# ADR 0098: Mixed armatures (symbol instances and shapes in one armature)

* Status: accepted (2026-11-16); §6's "the engine writes the same envelope itself" is over: `craft-anicraft-format` takes the version (`write_version`), and the engine only chooses it (`format::version_for`)
* Relates to: roadmap M7 (Bone/IK tool), ADR 0043 (instance armatures), ADR 0071 (shape
  armatures), ADR 0084 (springs, translation; first proposal of this), ADR 0090 (painted
  weights, reshaping; second proposal), ADR 0003 (file format), ADR 0004 (destructive actions)

## Context

An armature was one of two things: symbol instances joined by bones (ADR 0043), or one shape
with bones inside it (ADR 0071). A character is usually both: a body drawn as a shape that
bends, with a head, hands or props that are symbols. Animate keeps the two apart, and artists
work around it with separate armatures that never quite stay joined. ADR 0084 and ADR 0090 each
proposed joining them and left it out, because `Armature.skin` was *the* shape and half the
model branched on "skinned or not". Behaviour of the two kinds was learned from Adobe's public
help pages and from watching the program. The mixed form is our own design; nothing was copied.

## Decision

1. **A bone says what it is.** `Bone.shape: Option<ElementId>`: `Some(s)` is a bone **inside
   shape `s`**. Its `child` is a joint node (an id that is no element), its `parent` is `s` or
   the node of another bone inside `s`, and it bends `s`'s outline through `s`'s skin. `None`
   is a bone that **moves an element as a whole**: a symbol instance (ADR 0043), or a shape. Its
   `parent` may be any element or any joint node. `Armature.skins: Vec<Skin>` holds one skin per
   shape with bones. Nothing else is new. A skin's bindings name only bones inside its own
   shape, and automatic binding weighs only those.
2. **Why this and not ADR 0090's sketch.** That sketch had "a bone's parent/child naming an
   instance, a shape, or a node of one", which leaves the kind to be inferred from what is on a
   keyframe. `evaluate` must not depend on a lookup that a missing element changes, and
   `sanitize` can't check a file without the elements. A mark on the bone makes every rule
   local: which skin a bone bends, whether its child needs an element, what a hostile file may
   say.
3. **A shape can be moved as a whole.** This is what lets a chain run instance → shape: the
   bone from the instance ends on the shape (it turns rigidly, as an instance does), and the
   shape's own bones start there. A shape that a bone moves and that has no bones inside is
   allowed and has no skin. On an armature layer every shape is an object (merge drawing leaves
   it alone, as ADR 0071 decided for the first shape).
4. **One pose, one solver, one tween.** `armature::place` is ADR 0043's, unchanged in its
   arithmetic: it walks the bones parent first and places each child, element or node, from its
   parent. `armature::apply` sets the transform of every element a bone moves and then bends
   every skinned shape (`skin::deform`, ADR 0071's linear blend skinning, per skin). `solve`
   (CCD, limits, pins, slides), `pose_at` (tweens with easing) and `spring::sprung_pose` never
   knew what a bone moved and still don't: IK from a head instance turns the bones inside the
   body, a spring on the head's bone swings the plume on it, a sliding joint inside the body
   carries the head. `evaluate` stays pure, and every export sees posed instances and bent
   outlines through it, as before.
5. **Bit-exactness.** At rest a skinned shape's outline is the drawn path itself (the same
   `Arc`), and instances are placed by ADR 0043's formula, so an armature of instances or of one
   shape evaluates to the bits it did before this change. A skin's matrices are computed in
   the shape's own space exactly as ADR 0071 did. Loading no longer rescales bindings whose
   weights already sum to 1 within 1e-6, so a file's weights don't drift when it is saved again.
6. **The file format is versioned by need.** Two forms of an armature:
   * the **first form** (envelope versions up to 3): `bones` and, for a shape armature, its one
     `skin`. Every bone of an armature with a `skin` is inside that shape, which is how it is
     read: the marks are filled in on load.
   * the **mixed form** (envelope version 4): `bones` with `shape` on those inside a shape, and
     `skins`.

   An armature the first form can say is always written in it, and a document is written as
   version 4 only if some armature in it (in a scene or a symbol) is mixed
   (`format::version_for`). So older files load unchanged, a document without mixed armatures
   is byte for byte what earlier builds wrote and opens in them, and a document with one is
   refused by earlier builds with their existing message ("file version 4 is newer than this
   AniCraft (supports 3)") instead of opening with bones that move nothing. Removing the bones
   that mix the kinds takes a document back to version 3. The alternative, bumping the version
   for every document, would have locked users of the released build out of files that use
   nothing new. The envelope lives in the shared `craft-anicraft-format` crate, which writes one
   fixed version. For version 4 the engine writes the same envelope itself (same fields, order
   and compact-or-pretty rule) until that crate takes a version argument.
7. **Repair on load** (`Armature::sanitize`). On top of ADR 0043's rules: one skin per shape
   (the first wins), within the point limit; a bone inside a shape needs that shape's skin, a
   parent that is the shape or a node inside it, and a child that is no skinned shape; a skin
   with no bones goes; bindings keep only bones of their own shape. Bones are ordered parent
   first across the kinds, and what can't be placed (cycles, bones hanging from dropped ones)
   goes. `library::fresh_ids` now renames skins' shapes and bones' marks with the elements (a
   duplicated symbol's shape armature lost its skin before).
8. **Commands** (the `bone.*` namespace, which plug-ins may already call):
   * `bone.add {from | parent, to, …}`: `to` may be an instance or a shape. `parent` (a bone)
     starts the new bone on that bone's end: for a bone inside a shape, on its joint node. A
     bone from something that hangs from a bone always starts at that bone's end (#239).
   * `bone.add_in_shape {parent, tail}`: when `parent` ends on a shape, this is that shape's
     first bone inside it, starting at the parent's end; the shape gets its skin then.
   * `bone.remove`: the removed bones' elements (instances and whole shapes, with everything
     below them) go to a layer of their own as they look at the playhead. A shape left with no
     bones inside it loses its skin and keeps the outline it showed there, in every pose. With
     the last bone the armature dissolves. It still asks first when the armature has more than
     one pose (ADR 0004).
   * `bone.bind`, `bone.paint_weights`, `bone.set_weight` act on the shape their bone is
     inside. Point indices are that shape's.
   * `bone.info` adds `members`, `mixed`, `shapes` (each with its bones and points) and each
     bone's `shape`. `shape` and `points` stay, naming the first shape.
9. **The shell.** The Bone tool decides by where a drag begins and ends:
   * from an instance (or the end of a bone that moves one): the bone moves the instance or
     shape under the release;
   * from inside a shape: a bone inside it, as before;
   * from a bone's end that is in or on a shape: released over a symbol instance that is in no
     armature, the bone carries that instance; anywhere else it continues inside the shape.

   Shapes are joined as a whole only from an instance's side. From a shape's bone, a release
   over another loose shape draws a bone inside the first shape, because artists draw bones
   across overlapping artwork all the time. `bone.add` joins it headlessly. The Bind tool shows
   the points of every shape with bones and binds the points of the picked bone's shape. The
   Selection tool poses the whole armature by dragging any bone or member.

## Consequences

* Renderers, exporters and the GPU stage did not change.
* Measured (optimised test profile, this machine): a mixed armature of 25 bones, two shapes of
  1,002 points and 8 instances binds in 1.6 ms and poses in 0.066 ms per frame; IK through all
  25 bones takes 0.07 ms. The 5-bone test character evaluates in 0.0024 ms per frame.
* A skin's matrices live in the shape's space and elements are placed in the layer's. They
  agree when the shape's own transform is a move, a turn or an even scale. On a shape that was
  skewed or scaled unevenly before rigging, an instance on its bones follows the bones as the
  stage draws them, while the outline bends in the shape's skewed space, so they can part
  slightly. ADR 0071 already drew bones this way.
* Instance placement at rest is ADR 0043's product of transforms: exact for moved artwork, and
  within float rounding for rotated or scaled parents, as it always was.

## Not yet

* Joining two existing armatures into one, and re-parenting a bone.
* Joining a whole shape from a shape's bone with the Bone tool (the command does it).
* Skinning that keeps instances exactly on a skewed shape's outline; an instance that follows
  several bones (weights on an instance).
* XFL import of armatures; runtime armatures.
