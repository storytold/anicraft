# Array panel — interaction design brief

**Status:** working design brief for the experimental Array branch; not a product-wide policy.
**Scope:** the Array editing experience, its session semantics, and stage controls. Follow
[AGENTS.md](../AGENTS.md), [product principles](product-principles.md), and
[UI guidelines](ui-guidelines.md) for implementation conventions.

## The idea

An Array previews a spatial arrangement of one selected Symbol instance, with
properties interpolated across samples using draggable property stops. Spatial
commit creates ordinary independent instances. A Linear Array can also commit
as a native animation of the selected Symbol on the timeline.

Preserve the existing `array.create` engine command and its inclusive count
semantics: **10 means 10 total instances, including the original.** Instance 1
repositions the original and retains its identity. The preview is reversible;
Commit records one undo step and selects the result.

## A deliberate mode, not an always-active panel

An Array panel appears in AniCraft's normal dock system. The topmost setting is:

**Array type:** `None` (default), `Linear`, `Grid`, `Radial`, `Along Path`.

Linear, Grid, and Radial are available. Along Path is a future distribution.

- **None** means no active Array editing transaction. The panel may remain open.
  No stage preview or Array manipulation handles are active.
- Selecting a supported type with an eligible source selection **enters Array mode**.
  Preview generation starts immediately. Show **Commit** and **Cancel**.
- Changing between supported types does **not** exit Array mode, change the
  source selection, discard property stops, or erase geometry settings held for
  the other type. Regenerate from the original snapshot, not from preview copies.
- **Commit** keeps the baked artwork, ends the gesture as one undo step, returns
  the dropdown to None, and clears the parameter configuration. The next Array starts
  with defaults anchored to the selected symbol's current position.
- **Escape**, **Cancel**, or changing the dropdown to **None** abandons the
  preview and restores the pre-entry document and selection without an undo step.
  These exits return the dropdown to None and retain the configuration while the original
  instance stays selected.
- A hidden, closed, or backgrounded Array panel must **not silently discard** an
  active session. Make the active operation discoverable; design safe navigation
  and explicit exit. Avoid hijacking Escape from an open child popup or text field:
  normal UI dismissal should take precedence where necessary.
- If other editing actions conflict with the active preview, do not permit them
  to destroy the transaction. Treat array preview as an explicit editing session.
- The mode's geometry and property configuration is **view state** and is not
  itself written into the document or undo stack.

### Undo within Array mode

Use **Undo** and **Redo** from the Edit menu or its shortcuts to move through edits in the
current Array session. Ctrl+Shift+Z also invokes Redo while Array mode is active. Each history
step restores the distribution type, each mode's geometry, counts, stacking, easing, every
property stop, and the corresponding stop selection. The preview is regenerated from the same source.

Stage, stop, and numeric drags each make one step when the pointer is released. A drag of the
Spiral End handle groups Sweep and Spiral Offset into that step. Numeric typing previews live
and makes one step when the field is committed. While typing, the field keeps its own text
undo. Escape discards the current text edit. A new change after Undo clears the Array redo
branch. Dim Stage, handle visibility, and other editor controls stay outside this history.

Undo stops at the configuration with which the session opened. Commit records the current
result in one document-level step. Cancel, Escape, and None keep the current draft while the
source remains selected, then discard the session history. Reopening the draft starts a fresh
Array history. Only the compact Array settings are stored per step.

### Quitting with an Array preview

The window close button, application menu and Quit shortcut open the same Array question.
**Apply Array and Quit** uses the selected output and commits in one document
undo step. Timeline Animation opens its configuration dialog first. Cancelling
that dialog returns to the active Array.
**Discard Array and Quit** restores the original artwork without a document undo step.
**Keep Editing** or Escape dismisses the question and keeps the preview and its session history.
An invalid preview leaves Discard and Keep Editing available.

After Apply or Discard, the regular document close flow checks for unsaved changes across all tabs.
Cancelling that second question leaves AniCraft open with the Array already applied or discarded,
according to the earlier choice. A discarded draft can still be reopened while its source remains selected.

### Graphics-backend restart

Graphics recovery saves the document state, including unsaved document edits and committed
Arrays. An active Array draft and its session Undo/Redo history are lost when AniCraft restarts
on another graphics backend. Restoring both with the original source and document is deferred
to [session-recovery work](roadmap.md).

### Remembering versus resetting

Keep one unfinished configuration, including geometry for each array type, easing and property stops,
after Cancel, Escape or None. Clear the draft as soon as the source selection changes or another
document becomes active. Returning to the original instance then starts with fresh defaults.
Commit clears the configuration. Dim Stage and gizmo visibility carry over across every exit
and source change.

Offer a distinct **Reset Settings to Defaults** action. This resets remembered
stops and geometry and is separate from Cancel. Because it can erase elaborate
stop configurations, ask for confirmation before overwriting customized values;
do not reset the document or break the preview transaction. The exact reset
button position can be refined during UI review. Remembered settings across
an app restart are a future enhancement; retention within the running session
is required now.

## Geometry: interactive stage handles

**Linear:** the controlling metaphor is a **barbell**:

- A draggable **Start** circle is the registration point of instance 1.
- A draggable **End** circle is the registration point of the last instance.
- A thin line connects them. Its invisible hit target extends beyond the painted line so the
  bar is easy to grab, including when it is diagonal.
- Drag either circle to update Start/End X/Y, total distance, direction, and
  derived per-copy spacing immediately in the panel.
- Drag the line itself to translate both endpoints together.
- The last instance occupies the end point when count is greater than one.
  For count = 1, only the Start influences the result; handle the End visibly
  but without dividing by zero.
- Start and End are authoritative in the panel and on stage. Coordinates, Direction, Length,
  Spacing X and Spacing Y use the same draggable and typeable fields as Instances.
  With Distribution easing, Spacing X/Y show the nominal spacing across the full span.
  Derived edits hold Start fixed and move End. Direction preserves length, Length preserves
  direction, and a spacing component changes only its corresponding axis. At zero length a
  new length extends along +X and Direction becomes available again. Changing count keeps both
  endpoints fixed. With one instance, spacing is unavailable and the other geometry fields
  remain editable.
- The handles and line update the existing live preview. Commit, Cancel, Escape and undo
  follow the same rules as edits to numeric fields.
- Draw a small white circle with a dark outline for each generated instance, including the
  original, at its custom pivot or registration point. Grid always marks the symbol
  registration point. The circles are visual indicators in all modes. The blue Linear handles remain the draggable controls.
- Dragging is available across the pasteboard. Keep feedback live while reusing stable preview
  artwork and unaffected stage tiles where possible. Measure inside- and outside-stage drags at
  different counts before and after performance changes.
- Auto rendering can warm a bounded set of isolated Blur pictures and the backdrop once the
  current Linear Array and view settle. When the copies form an independent normal-blend
  foreground stack, moving Start, End or the bar places those pictures at the current geometry.
  Dim Stage makes the preview a foreground stack when the copies have no shared compositing
  group. A pause of 65 ms or release requests exact tiles and replaces the provisional picture
  as one frame. Parameter changes, a new view scale, masks, shared groups and other filter types
  use the normal rendering path. Commit and export render the baked document through the
  existing pipeline.
- Exact Array tiles are retained for the current view and pose. At most one older displayed
  tile stands in for each pending tile, and the worker drops results for superseded keys.
  Filtered GPU submissions share a scratch budget. A tile that cannot fit uses the CPU with
  the same authored quality. See the [filtered Array measurements](performance.md#filtered-array-gpu-pressure).

### Radial and spiral distribution

The adjacent **Circle** and **Spiral** icon buttons select the Radial distribution. Circle
starts selected with a 360° sweep. It uses `N` intervals and limits Sweep to
−360° through 360°. The first instance occupies Start and the final instance leaves one
interval at the seam. Spiral uses `N−1` intervals to place the first and last instances
at both endpoints. One instance always occupies Start. Editing Sweep keeps the chosen topology
at every angle, including at full turns and across positive and negative sweep boundaries.

Spiral reveals **Spiral Offset** and **Centre Drift X/Y** and accepts multiple turns
in either direction. At a fraction `t` of the distribution, angular travel is `Sweep × t` and
the unsigned turn count is `abs(Sweep × t) / 360°`. The local centre is `Centre + Centre Drift × turns`.
The radius around that centre is `max(0, Radius + Spiral Offset × turns)`. Both rates are in
pixels per turn. Selecting Spiral seeds a zero offset at 48 pixels per turn and a zero sweep at
one turn. Circle retains the offset and drift for the next switch and uses a fixed radius and
centre. A zero offset entered while editing Spiral keeps successive turns at the same radius.
After the radius reaches zero, the rest of an inward spiral follows the drifting centre.
**Rotate instances** uses signed angular travel and places each registration point on the curve.

### Radial and spiral stage controller

- Drag **Centre** to translate the whole arrangement. Drag **Start** to edit the starting
  radius and angle. Drag **End** to edit the sweep and ending radius. In closed mode both endpoints
  share a radius. In open mode, editing End radius updates Spiral Offset for the current turn count.
- Link the handles with a circular arc or spiral guide. Dragging the guide translates the
  arrangement like the Linear bar. Keep the guide visible even when copies fall short of End.
- In open mode, a bar links **Centre** to the **Drift** grip at the local centre after one turn.
  Drag Drift in either axis to steer the path without changing radius or Sweep. Drag the bar to
  translate the arrangement. At zero drift, the Drift ring surrounds Centre and stays grabbable.
- Synchronize Centre X/Y, Radius, Start angle, Sweep, Spiral Offset, and Centre Drift X/Y immediately with the panel.
  Keep the current instance-rotation option and show the controller through **Show/Hide Gizmo**.
- Accumulate signed angular movement from one pointer sample to the next so the sweep retains
  its winding count across 0°/360°. With drift active, place End under the drag point by solving
  Sweep and Spiral Offset near the current winding. Hold the last valid placement if the local
  solution cannot reach the pointer. The numeric Sweep field changes angular travel alone.
- Circle stops at ±360°, where Start and End share one geometric point. A drag past
  that point holds the full turn until the pointer retraces the seam. The final instance still
  sits one interval before Start.
- Keep coincident handles independently reachable at a full circle, zero sweep, or the centre.
  When Start and End overlap, drag Start from the inner disc and End from a broad annulus that
  extends beyond its visible outline. Grips and their labels keep their own positions as they
  pass each other. Ignore angular changes while the pointer is at the centre.

Position-only drags reuse existing instances and unaffected stage tiles where eligible. The
preview uses the standard rendering path when instance rotation changes the artwork's footprint.
The [filtered Array benchmark](performance.md#filtered-array-preview) records frame
times, stage update cadence, and the pixel limits of reusing an isolated Blur image.

### Grid distribution

Grid starts with 3 rows and 3 columns, spaced 60 px apart. Rows × Columns includes the
original and cannot exceed 500. Instances follow row-major order from Start. Each instance's
symbol registration point sits on the lattice, including when the instance has an authored
transformation point. Signed spacing permits axis reversals and overlap.

- Drag **Start** or **End** to resize the rectangle about the opposite corner. Drag the
  border or interior to translate it, or edit **Start X/Y**. A one-row or one-column Grid
  collapses along one axis and remembers that axis's signed spacing. At 1 × 1, either handle
  moves the instance. The centre of coincident grips grabs Start and the ring grabs End.
- Changing a count preserves the span on that axis when it has at least two instances
  before and after the edit. Editing **Row Spacing (Y)** or **Column Spacing (X)** changes
  the corresponding span from Start. Both fields remain editable on a one-instance axis.
- The count and spacing locks start off and work independently. The count lock uses
  Columns for both counts, limited to 22 per axis. Editing either count updates both.
  The spacing lock uses the column spacing's magnitude and keeps each axis's sign. Editing
  either spacing updates both magnitudes. A count edit preserves the edited axis's span
  where possible and adjusts the other axis to maintain equal spacing. A corner drag uses
  the smaller of the two candidate spacing magnitudes, so both spans pass smoothly through
  zero as either axis crosses. The two signs follow the corner independently.
- **Flip on Reverse** starts on. Each axis with negative spacing and at least two
  instances mirrors the artwork around each instance's own registration point. Reversing
  both axes mirrors both directions. Turning it off keeps the artwork's orientation in
  every quadrant. The toggle affects the artwork after stop transforms and leaves positions
  and row-major ordering intact.
- **Property Distribution** selects one mode for all stops. **X + Y** is the default and
  preserves the existing row-major progression, using the instance index from Start divided
  by the total count minus one. **X** samples the column index divided by columns minus one
  and repeats across rows. **Y** samples the row index divided by rows minus one and repeats
  across columns. A single-instance sampling axis uses 0% progression. Indices follow
  logical order from Start in every quadrant.
- The chosen progression feeds the existing Transform, Colour Effect, and Filters stop
  sampler with Global or Per Segment easing. The base lattice stays evenly spaced. The
  distribution mode is part of session Undo/Redo and the retained Grid draft. Existing
  command parameters that omit the setting use **X + Y**.
- Distribution easing for placement is available for Linear and Radial. During position-only edits,
  Grid reuses the preview instances and eligible stage tiles. Changes to the effective
  mirror orientation rebuild the preview and its tile coverage.

### Staggered Grid offsets

The collapsible **Stagger** section enables **Offset Rows** and **Offset Columns**
independently. Each uses a positive integer **Every N** and signed **X Offset** and
**Y Offset** values in stage pixels. Both systems start off with **Every N** set to 2.
The default row vector is (+50, 0) and the default column vector is (0, +50).

With **Accumulate** off, every Nth row or column gets one offset, starting with the
Nth. With **Accumulate** on, each qualifying row or column adds an increment and
the rows or columns in between retain that displacement. The shared checkbox
starts off and applies to both systems. Each system leaves its first row or column
unshifted. At **Every N** = 1, every subsequent row or column qualifies.

At an intersection the row and column vectors add. For example, a second-row
offset of +50 X and a third-column offset of −20 Y move that instance by (+50, −20).
Indices follow the logical ordering from Start in every quadrant. The original
instance at Start stays anchored. Flip on Reverse still follows the base directions.
Property sampling and stacking use the same logical indices as the base Grid.

The gizmo rectangle represents the base lattice. Artwork may extend past it or
overlap. Resizing changes the base spacing. Translating the rectangle moves the
whole staggered arrangement. Changing counts recomputes which rows and columns
qualify. Stagger settings follow the normal draft and session Undo/Redo behavior.
Position-only edits reuse preview instances, and eligible stage tiles follow the
displaced positions, including outside the rectangle.

### Gizmo visibility

Place a compact circle–line–circle toggle beside **Dim Stage**. Use a rounded button with a
visible surface in both states and the theme's active treatment when on. Its tooltip follows
the action, **Hide Array Handles** or **Show Array Handles**. It starts on in all modes. Hiding
it removes the handles, guides and registration markers while live artwork and numeric controls
remain available. Visibility is an editor setting shared by all controllers.

## Dim Stage

**Dim Stage** is a checkbox, **checked by default** during Array mode.
Unrelated artwork is visually subdued and the entire Array is drawn above it, preserving the
preview instances' internal order and their clipping. The stage restores the actual document
stacking when the checkbox is off. Dimming, foreground composition and markers are editor-only
and never change colour effects, document order, or exported artwork.

The artist can turn it off to inspect true colours, alpha, blend, filters, and
overlap against the unmodified surrounding artwork.

## Property stops — reuse the existing gradient interaction, not its colour meaning

Use the existing gradient stop bar's interaction conventions and, ideally, its
shared implementation: add/select/drag/reorder/remove stop handles, keyboard
nudge, editable stop position, and the selected-stop model.

However, the **Array ramp is not a colour gradient**. A stop may eventually
capture transform, opacity, tint/brightness/advanced colour effects, filter
parameters, playback offset for Graphic symbols, stacking depth, and easing.

- Show a **neutral horizontal track** with draggable pentagonal stop handles and a distinct
  selected marker. The track carries positions and selection. Numeric values sit below it.
- One rounded container beneath the track holds the selected stop's properties. Its section
  order is **Transform**, **Colour Effect**, **Filters**, **Playback**, and **Stacking Order**.
  Show only implemented sections. **Transform**, **Colour Effect**, and **Filters** collapse
  independently.
- **Transform** offers draggable and typeable Scale X and Y in percent and Rotation in degrees.
  Both default stops start with the source's authored scales and angle, as shown in Properties.
  Edits retain the source shear. The shared Scale lock preserves the selected stop's scale
  ratio while editing. Rotation interpolates the entered degrees directly, so 0° to 720° takes
  two turns. **Rotate instances** still adds the distribution's rotation. Scale and rotation
  keep the instance's transformation point in place after the distribution positions it.
- **Colour Effect** offers None, Brightness, Tint, and Advanced. Brightness runs from −100%
  (black) through 0% (unchanged) to +100% (white). Tint uses the shared colour picker and an
  independent amount from 0% to 100%. Advanced has Alpha, Red, Green, and Blue rows, each with
  a multiplier from −100% to 100% and an offset from −255 to 255. All numeric fields can be
  dragged horizontally or typed.
- Each stop has one Alpha multiplier. Advanced edits it in the Alpha channel row alongside
  a separate Alpha offset. The other styles show it as the 0–100% Alpha field. Leaving Advanced
  floors a negative Alpha multiplier at 0% and removes the channel offsets with that style.
  Switching from Brightness or Tint to Advanced starts with the equivalent channel transform.
- The default stops copy the source instance's colour style and Alpha. Each baked instance uses
  its sampled effect through [ADR 0056's shared colour model](adr/0056-colour-effects.md).
  `array.create` uses this behaviour with `source_values: true`. In that mode the stop's scale,
  rotation and colour values are authored instance values, and omitted properties use the source
  values. Older commands retain their relative scale, angle and colour semantics. Commit records
  the authored stop payload and the mode so replay matches the preview.
- A new stop copies the complete snapshot of the nearest stop on its left, including its
  outbound ease. Before the earliest stop it copies that first stop. It keeps its own position
  and remains independently editable. Deleting it leaves the surviving snapshots intact.
- Within a colour style, values interpolate directly, including both Tint colour and amount.
  None meets the neutral value
  of the neighbouring style. Other mixed styles interpolate multipliers and offsets in
  Advanced space, matching Classic Tweens. Geometry edits leave the authored values intact.
  Changes made in one opening of the Tint picker form one session undo step.
- Preserve independently draggable stops at positions 0..100%. Example:
  positions 0%, 40%, 100% can have Alpha 75%, 100%, 3%; dragging the 40% stop
  changes how quickly that middle value is approached.
- The source instance provides the complete filter stack, including its order and on/off states.
  Each entry has a collapsible card in **Filters**, including multiple entries of the same kind.
  Cards start collapsed in each Array session. Their expansion follows source-stack positions
  when switching stops or distribution modes.
  All seven filter families are available as stop cards. Preview and Commit preserve the stack
  structure.
- **Blur** retains its X and Y size fields and uses the source quality. **Drop Shadow** and
  **Glow** use the shared Properties controls for Blur X/Y, Strength, Colour, Quality, Inner,
  and Knockout. Drop Shadow also offers Angle, Distance, and Hide Object. **Bevel** uses the
  shared controls for Blur X/Y, Strength, Angle, Distance, Shadow Colour, Highlight Colour,
  Quality, Type (Inner, Outer, Full), and Knockout. Both colour pickers include opacity.
  **Gradient Glow** and **Gradient Bevel** use the Properties ramp editor alongside Blur X/Y,
  Strength, Angle, Distance, Quality, Type, and Knockout. Ramp markers select individual colours
  independently of the Array stop track. The selected ramp stop has a colour picker and a
  draggable or typeable position. The pinned stops at 0% for Gradient Glow and 50% for Gradient
  Bevel can change RGB and stay fixed at zero opacity. Drop Shadow, Bevel, Gradient Glow, and
  Gradient Bevel angles use the shortest arc. An exact half turn follows the authored direction.
- **Adjust Color** shares the Properties drag-and-type fields for Brightness, Contrast, and
  Saturation (−100 to 100) and Hue (−180° to 180°). Hue interpolates through its signed range,
  so a transition from −180° to 180° passes through 0°. Filters easing applies to all four
  values, including safe clamping on overshoot. Adjust Color stays in its source filter-stack
  position and works alongside the independent **Colour Effect** section.
- Both ramps retain the source ramp's count and order. Each editable colour stop can move between
  its neighbours. Gradient Bevel ramp stops stay on their original side of the midpoint. Add or
  remove colour stops in the source's Properties panel before starting the Array. Inserting an
  Array property stop copies all the preceding stop's ramp values and its outbound ease.
- The default stops start with the source values. Moving a stop preserves its values. Disabled
  Adjust Color entries show their source values read-only. Other off filters can store stop
  values while staying off. If the source stack changes while a draft is
  inactive, reopening the draft initializes the filter values from the current source. Other
  stop properties carry over. Edits made during one opening of a filter colour picker undo together.
- `array.create` accepts a `filters` list at each stop with one object per supported source entry
  in stack order. Omitted or empty lists use the source values. Existing `blurs` lists still set
  the Blur X/Y values in Blur order. Earlier `filters` lists can omit all entries of Gradient
  Bevel or Adjust Color to keep those entries at their source settings. An omitted `stops` field
  inherits the source ramp.
  A supplied ramp matches the source count and order and keeps its pin in the same slot.
- Property families share one set of stop positions.
- Future z-order interpolation can sample numeric depths and then sort copies
  stably, since visual stacking is discrete.
- A radial alpha progression of 100% -> 0% -> 100% can be achieved with three
  stops. Do not impose aesthetic loop-correction rules.

### Easing

The Easing section above the track offers **Global** and **Per Segment**. The Ease picker shares
the named presets and curve thumbnails used by Classic and Motion Tweens
([ADR 0081](adr/0081-ease-presets-and-custom-eases.md)). New named families appear in Array
automatically. The default is Global with Linear, and the default section starts collapsed to
keep the stop editor in reach. Creating and editing custom curves remains in the tween editors.
Distribution, Transform, Colour Effect, and Filters start checked and each use ordinary linear
progression when unchecked. Filters is available when the source has a filter.

- **Global** remaps the complete 0–100% progression before looking up values along the track.
  Intermediate values can be reached sooner or later while the authored stops stay in place.
- **Per Segment** uses the outbound preset on each stop for the interval up to the next stop.
  Exactly one stop is selected. Its preset appears in the picker, which is disabled on the
  final stop while its properties remain editable. Reordering carries each preset with its
  stop. Global and outbound presets are independent and survive mode changes.
- Inserting a stop copies the inherited snapshot's outbound preset. Deleting a stop leaves
  the surviving curves intact.
  The new stop is selected on insertion. After deletion the next available stop is selected.
- Segment boundaries stay fixed. When stops are inset, Distribution uses Linear between
  the track edge and the first or last authored stop. Transform, Colour Effect, and Filters hold the
  nearest stop outside the authored range.
  Exactly coincident stops keep the stop track's existing sampling order. Spans no wider than
  floating-point epsilon avoid a division.
- Distribution easing changes where the instances sample the path. The path, the guide, the
  numeric settings and the closed-loop interval count stay fixed. **Rotate instances** follows
  the eased angular position when Distribution is checked.
- Curves with overshoot, such as **Back Out** and **Elastic In**, can travel beyond an interval
  or either end of the path. Closed radial curves wrap across the seam. Open arcs and spirals
  extend their sweep, radial offset and Centre Drift beyond the guide. A one-instance array
  stays at Start.
- In Per Segment mode, a property that overshoots continues the current segment's interpolation.
  Global overshoot continues from the track endpoint's value using the nearest usable stop span.
  Colour Effect easing applies to all colour values, including Alpha. Filters easing applies to
  continuous filter numbers and colours, including the RGBA channels and positions of matching
  Gradient Glow and Gradient Bevel ramp stops. Overshooting ramp positions stay in order and can
  meet at a shared position on the same side of the pin. Quality, Type, and switches hold from
  each authored stop until the next using uneased progression. Colour, Scale, and filter
  parameters remain in their valid ranges. Transform Rotation uses the unwrapped authored degrees.

## Future distribution modes (not in this milestone)

- **Along Path:** distribute evenly by arc length, by equal path parameter
  intervals, or at Bézier anchor **nodes**. Closed paths avoid duplicated
  initial nodes. Path selection and tangent alignment need later UI work.
- **Chaos:** a global intensity slider (0..100%) with property categories
  such as position, rotation, scale and Alpha; deterministic random seed so
  editing stops doesn't reshuffle unrelated positions.
- Source can eventually be multiple selected instances, a group, a drawing
  object, or mergeable raw artwork. Initially limit to one symbol instance,
  with explicit feedback.

## Commit output

**Commit As** is a two-option selector above Cancel and Commit at the bottom
of the Array panel. It defaults to **Spatial Array**, which
commits the existing preview. **Create Unique Symbols**
is only shown for Spatial Array. The output choice is part of the Array draft
and participates in Array Undo/Redo.

**Timeline Animation** is available for Linear, Circle, and Spiral Arrays. The
**Create Timeline Animation** dialog edits duration in frames, shows seconds
from the document FPS, and starts at one second rounded to whole frames. Linear
uses **Classic Tween** (default) or **Hold Keyframes**. Circle and Spiral use **Motion Tween**. The preview stays spatial until commit. Cancelling the dialog
keeps the preview, property stops, and Array history. The dialog remembers the
entered duration if reopened within the same Array session.

- Classic Tween uses ordinary keyframes at the authored property stops and
  includes Start and End. Native Position, Rotation, Scale, Color, and Filters
  groups carry compatible Array easing. Instance count does not control the
  number of Classic keys.
- Hold Keyframes uses every evaluated spatial sample in Linear order. An Array
  with one sample holds the Start pose for the full duration.
- Both outputs use one instance identity and its existing Symbol definition.
  Exposure starts at the playhead and lasts exactly the selected duration.
  Each pose uses the nearest frame, with halfway cases rounded up. The first
  and last poses of a moving animation occupy the first and last frames.
- Commit is one document undo step. It uses the original layer, requires a
  compatible source keyframe, and reports any occupied destination range.
  Later content stays at its original frame. Validation errors leave the draft
  open. A duration collision lists every affected frame and its numbered stops
  or samples. **Use N frames** applies the shortest valid duration using the
  same rounding as conversion. Coincident stops must be moved apart.

Classic Tween supports Linear and compatible Per Segment easing and can apply
non-Linear Global easing to a single span with endpoint stops. It reports
frame collisions and combinations that cannot preserve the Array evaluation,
including non-Linear Global easing across intermediate stops, clamped property
overshoot, long rotations and wrapped filter angles, certain combined Colour
Effect and Alpha transitions, and collapsed or sign-changing animated scales.
Linear position can overshoot within one native span. Hold samples support the
full Linear Array property and easing evaluation.

### Circle motion paths

Circle produces one native Motion Tween span on the original layer, independent
of the spatial instance count. It follows the authored Start, radius and signed
sweep through up to one complete turn. Equal angular arcs of at most 30° are
converted to tangent-matched cubic Béziers with radial error below 0.00004% of
the radius before single-precision coordinate rounding. The native path walker
uses arc length to traverse the path including full revolutions, where the
first and last positions coincide. The stage's Motion Path tools can edit
the anchors and handles without changing the authored timing.

Distribution can remain Linear or use a monotonic preset (Quad, Cubic,
Quart, Quint, Sine, or Circ). Global easing maps elapsed time to distance
over the whole path. Per Segment easing uses each stop's outbound ease
and its exact normalized time as a distance boundary, including the same
end margins as the spatial Array. The native Motion Tween saves this
timing separately from the curve, property keys, and their easing.
Back, Bounce, and Elastic are rejected for Distribution because they
overshoot or reverse direction. There is no separate path timing editor.

Authored property stops map to native
Motion Editor keys at their rounded frame positions, with held values out to
any missing endpoint. Duration collisions use **Use N frames**. The path's
position keys stay independent of these property keys. Scale, rotation,
alpha, supported Colour Effects, and existing filter-slot parameters can vary.
Per Segment easing maps to the respective Transform, Colour Effect, and
Filters tracks. Nonlinear Global easing across interior stops and overshoot
on bounded properties are unavailable.

Native colour tracks support Alpha, Brightness, Tint, and Advanced channels.
Mixed styles use Advanced where the interpolation can be represented exactly.
Filter tracks support numeric values, colours including gradient ramp colours,
and native stepped quality and switches. Constant settings stay on the
original filter stack. Varying gradient ramp offsets and bevel type are
unavailable. Filter angles retain shortest-arc travel using unwrapped keys.

**Rotate** adds signed sweep rotation to the authored stop rotation. With
nonlinear Distribution, the signed sweep follows eased path distance while
stop rotation keeps its own native keys and easing. With Linear Distribution,
compatible nonlinear rotation segments combine the two contributions into an
editable native ease curve. Scale tracks
cannot reach or cross zero. With an offset transformation point, Circle
supports translation and scaling that leaves that point's registration
relationship unchanged. Rotation with an offset transformation point and
transform stops that shift registration away from the path are unavailable.
Circle requires a nonzero sweep and a positive radius within the native path
range.

### Spiral motion paths

Open Radial conversion follows the same Start angle, signed Sweep, Spiral
Offset, and Centre Drift X/Y as the spatial evaluator. Offset changes radius
and drift moves the centre per unsigned turn, while Sweep sets travel direction.
Circle mode ignores the stored offset and drift. Open Radial with both rates
zero uses the same geometry and timing as Circle through one turn. Beyond
that it can traverse the same coordinates on several successive turns.

Spiral uses tangent-matched cubic Bézier segments through multiple signed
revolutions. The segment count grows with the radius, angular sweep, and
change in radius. The fit targets 0.01 px before native coordinate rounding
and keeps the evaluator's exact Start and End. The path is measured in
traversal order, including coincident sections and self-intersections.
The fit allows up to 256 curved cubics plus a straight continuation at
zero radius. Native coordinate and timing-map limits also apply.

Linear, Global, and Per Segment use the same monotonic Distribution presets
and normalized stop boundaries as Circle. The native Motion Tween eases time
to the authored angular parameter and maps that parameter to distance along
the cubics. The map is simplified within 0.01 px of measured distance (or
one floating-point epsilon of the total distance on large paths), independently
of frame count. Path-handle edits keep the stored pacing. Back, Bounce, and
Elastic remain unavailable for Distribution.

Spiral reuses the Circle property tracks, stop frame rounding, collision
correction, and transformation-point restrictions. **Rotate** adds the same
signed, eased angular travel to the authored Rotation track, retaining whole
turns within the native rotation range. If an inward Spiral reaches zero radius
early, its registration follows the drifting centre along a straight segment or
holds at a stationary centre. The join may have a corner as in the spatial
evaluator, and rotation can continue during the hold. A stationary path with
no length or a path beyond native limits is rejected with the draft intact.

## Create Unique Symbols

**Spatial Array** offers **Create Unique Symbols** in Linear, Radial, and
Grid. It starts off, so ordinary Commit keeps all instances on the source Symbol.
Toggling it participates in Array session Undo/Redo and is retained with a
cancelled draft. The Library stays unchanged during preview and on Cancel.

With the option on, Commit copies the source definition and its timeline once per
Array element, including the one at Start. The copies use AniCraft's existing
Library duplication rules. Each has fresh layer and element identities in its
timeline. Nested Symbol references and shared assets remain shared, and linkage
classes stay with the original definition. The original Symbol remains in the
Library and existing instances elsewhere keep referencing it.

Copies are placed in the source folder. Names extend the highest numeric suffix
among case-insensitive siblings in the naming family, including other Library
entries that could collide. For example, with Box, Box_1, Box_3, and Box_8, the
next Array starts at Box_9 even if Box_8 is the selected source. The sequence
follows Array placement order. If the next name exceeds the Library name limit,
the preview stays open so the artist can turn off the option or cancel.

Commit uses the same property-stop evaluation as the preview. It changes only
the Symbol reference on each evaluated instance, preserving editable transforms,
colour, alpha, filter parameters, and the source filter stack's ordering and
enabled states. The instances and their new definitions are recorded in one
document undo step. Redo restores the same definitions and references.

## Deferred Features

### Gizmo Keyboard Nudging

Linear, Grid, and Radial Arrays should let artists move the entire gizmo with
the arrow keys. Each press moves it by 1 px, or by 10 px with Shift. The
movement should match dragging the Linear line, Radial circle or guide, or
Grid rectangle. It should preserve the Array's shape, relative geometry,
spacing, and orientation, and participate in Array session Undo/Redo.

Numeric fields, text fields, and other keyboard-editable controls should keep
their arrow-key behavior while focused.

### Further Array to Timeline outputs

Linear, Circle, and Spiral conversion are available in
[Commit output](#commit-output). Future work includes arbitrary Path conversion. Grid traversal order is undecided. Sequential reveal and
staggered animation starts across multiple instances or unique Symbols remain
deferred.

### Alternative Drop Shadow compositing

Keep ordinary per-instance shadow compositing as the default. Explore an option that composites
each instance's shadow behind all Array instances, including those earlier in the stack.
A single collective shadow for the whole Array is a separate compositing approach.
The rendering implications remain open.

### Spiral Drawing Tool

The current Tools panel and drawing command registry have no standalone Spiral Drawing Tool.
Explore adding one to AniCraft's main Tools panel. It would draw an editable vector spiral
path with a stroke, without requiring an Array or a Symbol selection. Offer stroke colour,
thickness, and the applicable options from AniCraft's existing stroke controls.

The Spiral Array's `RadialDistribution::point` already evaluates a continuous curve. Its
Centre, Start, and End handles and guide translation are candidates for reuse. A drawing tool
would sample or fit that curve into an editable path. Open questions include the initial drag
gesture, how to expose sweep and radial growth, whether Centre Drift belongs in the drawing
tool, and whether the spiral remains parametric after drawing or becomes ordinary path nodes.

## Milestone sequence and acceptance criteria

1. **Editing lifecycle:** dropdown None/Linear/Radial with future options
   disabled; immediate live preview upon entering mode; switching types
   regenerates from the same original, without accumulating copies; Commit
   and Cancel/Escape/None have the distinct effects above. Closing/switching
   dock tabs doesn't silently cancel.
2. **Stop track:** neutral visualization, existing gradient-edit gestures
   and keyboard interactions, Alpha value in a collapsible Colour Effect
   section, multi-stop interpolation. Gradient's original colour editor
   must behave exactly as it did before.
3. **Linear controls and stop editor:** rounded selected-stop container, Linear Start/End/bar,
   synchronized coordinates and derived spacing, and a gizmo visibility toggle beside the
   editor-only **Dim Stage** checkbox.
4. **Interaction polish** covers editable derived geometry, visible registration markers in both
   modes, foreground Dim Stage composition, and measured improvements to live Linear dragging.
5. **Radial distribution and spiral foundations** covers explicit topology, signed sweep,
   radial offset, and recovery tied to the selected source.
6. **Radial and spiral stage controls** covers Centre, Start, End and Drift, guide translation,
   independent radial growth and centre displacement, and signed sweep winding during a drag.
7. **Transform property stops** adds independent scale axes, the Scale lock and additive rotation.
8. **Session undo and redo** covers coalesced gestures, numeric edits, and the single-step Commit.
9. **Easing** covers global and per-segment presets, independent family assignment, and overshoot.
10. **Independent outbound easing and Quit** covers stop-owned presets, insertion inheritance,
    and resolving a preview before the normal Save/Discard/Cancel question.
11. **Colour Effect property stops** covers Brightness, Tint, and Advanced channels alongside Alpha.
12. **Blur filter stops** vary each source Blur independently while preserving the filter stack.
13. **Responsive filtered dragging** repositions cached Linear Blur pictures during eligible Auto
    gestures and renders exact artwork at rest.
14. **Drop Shadow and Glow stops** add colour, continuous filter values and held switches and quality.
    Mixed active stacks use the ordinary renderer. Eligible Blur-only previews retain their drag cache.
15. **Bevel stops** add independent shadow and highlight RGBA colours, shortest-arc angles,
    and stepped Quality, Type, and Knockout using the existing filter renderer.
16. **Gradient Glow stops** add the shared ramp editor with source-defined stop order and count,
    eased positions and RGBA, and the transparent pin at 0%.
17. **Gradient Bevel stops** reuse the shared ramp editor with the transparent midpoint at 50%
    and ordered interpolation on each side of the pin.
18. **Adjust Color stops** complete all seven filter families with independent signed Brightness,
    Contrast, Saturation, and Hue controls.
19. Circle and Spiral controls and filter card polish.
20. Grid placement, rectangular stage gizmo, counts and spacing, locks, and optional
    mirroring when crossing either axis. Grid properties follow row-major order.

21. Global Grid Property Distribution with X, X + Y, and Y modes.
22. Staggered Grid offsets with independent row and column vectors and shared accumulation.
23. Shared Create Unique Symbols Commit option with independent timelines and collision-safe names.
24. Linear Array to Timeline, using native Classic Tween and Hold Keyframes with a shared Commit As selection.
25. Circle Array to Timeline with editable native Motion Tween paths, including full signed revolutions.

Later: Keyboard nudging, Path/node placement, more stop properties,
custom easing, Chaos, multi-selection, and optional grouping/merging.

For each milestone run relevant engine and shell tests, Clippy and UI screenshot
checks as prescribed by AGENTS.md. Add lifecycle regression tests for: an
inclusive copy count; switching modes; Escape and None restoring the original
selection/document; configuration retained after Cancel, Escape and None; Commit producing
one undo step, preserving the view options and initializing the next Array from the new source;
no accumulating copies during repeated preview edits; and correct
stop-bar behaviour in both Gradient and Array editors.

For the Linear gizmo, verify both endpoint drags, line translation, numeric fields, fixed
endpoints when count changes, visibility toggling and session exits during stage edits.

**Do not modify AGENTS.md merely to encode this feature's design.** A lasting
architectural choice can be recorded in an ADR following the repository's normal
reservation process if maintainers choose to adopt it.
