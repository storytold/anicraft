# Parity with the reference app

AniCraft aims at full feature parity with the reference app, Adobe Animate. This page lists what
that app does, area by area, and where AniCraft stands on each feature. The
[roadmap](roadmap.md) orders the gaps into a backlog; the `P-nnn` ids here point into it.

AniCraft is an independent, open-source project, and is not affiliated with or endorsed by Adobe.

## How this page was made, and how to keep it

* **Clean room.** The feature list is written in our own words from the reference app's public
  documentation and from general knowledge of it. Nothing here is copied from that
  documentation, and there are no screenshots or assets of it (`standards/clean-room.md`).
* **Status was checked on 2026-11-20** against the command reference
  ([guide/reference/commands.md](guide/reference/commands.md), generated from the registry), the
  [user guide](guide/index.md), searches of `src/` and `crates/`, and the status table in the
  roadmap. A **†** marks a status taken from the roadmap's text alone, with no second source;
  a **?** marks one that could not be verified at all.
* **Status words.** **done**: works, with at most small differences named in the note.
  **partial**: usable, and the note says what is missing. **missing**: not there. **won't do**:
  not planned, with the reason (a proprietary format or service, or a target its maker has
  retired). **ours**: something AniCraft has that the reference app doesn't, listed where it
  replaces a feature.
* **Keeping it.** A PR that changes a feature's status changes its row here, in the same PR. A
  row that turns **done** loses its backlog id.
* **Headless.** The Headless column and the count under the summary follow
  [RFC-0052](https://github.com/storytold/craft-contrib/blob/main/rfcs/0052-headless-reach.md). A row is `yes`
  only when it cites a command id from the registry.

## Summary

| Area | Done (ours included) | Partial | Missing | Won't do |
|---|---|---|---|---|
| Workspace and panels | 23 | 5 | 4 | 2 |
| Tools | 19 | 7 | 7 | 1 |
| Drawing model | 16 | 3 | 4 | 0 |
| Colour | 8 | 1 | 2 | 2 |
| Text | 8 | 5 | 5 | 2 |
| Symbols and Library | 13 | 3 | 4 | 0 |
| Timeline | 16 | 2 | 9 | 0 |
| Tweens and the Motion Editor | 12 | 2 | 4 | 0 |
| Bones and rigging | 8 | 2 | 3 | 0 |
| Camera | 5 | 1 | 0 | 0 |
| Layers | 11 | 1 | 2 | 0 |
| Filters and blends | 6 | 1 | 1 | 0 |
| Sound | 10 | 3 | 4 | 0 |
| Video | 2 | 2 | 2 | 2 |
| Interactivity and scripting | 6 | 4 | 5 | 3 |
| Publishing and export | 11 | 2 | 3 | 7 |
| Import formats | 6 | 2 | 7 | 2 |
| Assets and sharing | 4 | 1 | 2 | 4 |
| Accessibility | 4 | 2 | 1 | 0 |
| Preferences and shortcuts | 6 | 2 | 0 | 1 |
| Automation | 4 | 2 | 1 | 1 |
| **All** | **198** | **53** | **70** | **27** |

The counts are of the rows below, so they weigh a small feature like a large one. The backlog,
not this table, says what matters most.

**Headless ([RFC-0052](https://github.com/storytold/craft-contrib/blob/main/rfcs/0052-headless-reach.md)): 0 yes, 0 no (window 0, os 0, engine-gap 0), 0 partial, 348 unknown.**
The **Headless** column at the end of each table says whether `anicraft-cli` and its MCP server (`anicraft-cli mcp`) can do the feature without the desktop window. An empty cell is `unknown`, the starting value. A row can be `yes` only when it cites a command id from the registry the CLI and the MCP server load (see the [command reference](guide/reference/commands.md)); no row is marked `yes` yet (a few rows already cite ids, such as `align.match_width`, and can be checked first), and `yes` is never written without one. A checked row that isn't reachable says `no (<reach>)` or `partial (<reach>)`, where the reach is `window` (needs the desktop window, a dialog or the pointer), `os` (needs an OS or online service the engine doesn't wrap) or `engine-gap` (could be a command, isn't one yet). A `partial` row's notes also say which part a headless client can do. Update the counts here when you mark rows.

## Workspace and panels

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Dockable panels: drag a tab to dock, stack, float, resize, fold | **done** | [ADR 0075](adr/0075-dockable-panels-and-workspaces.md), [ADR 0087](adr/0087-dockable-panels-second-slice.md); [workspace](guide/workspace.md). |
| Docks collapsed to icon strips with flyouts | **done** | [ADR 0075](adr/0075-dockable-panels-and-workspaces.md). |
| Workspace presets, saved workspaces, reset | **done** | Essentials, Animator, Designer and your own. |
| Several documents open as tabs | **partial** | [ADR 0112](adr/0112-several-documents-open-at-once.md). Missing: a tab pulled into its own window, reopening the last session (P-050). |
| Start screen with presets, recent files and templates | **done** | [ADR 0059](adr/0059-files-from-the-desktop-recent-files-and-start-screen.md), [ADR 0065](adr/0065-templates-svg-into-the-library-and-drops-on-the-timeline.md). |
| Tools panel | **partial** | Fixed on the left, one button per tool. Missing: tools grouped under one button with a flyout, a panel that reflows as it widens or moves, a custom tool set (P-024; [#198](https://github.com/storytold/anicraft/issues/198), [#246](https://github.com/storytold/anicraft/issues/246)). |
| Properties panel that follows the selection (document, tool, frame, object) | **done** | Mixed values shown for several objects. |
| Timeline panel | **done** | See Timeline. |
| Library panel | **done** | See Symbols and Library. |
| Colour and Swatches panels | **done** | See Colour. |
| Align panel | **done** | Align, distribute, space evenly, match size, To Stage. |
| Transform and Info panels | **partial** | Position, scale, rotation, skew and the transformation point are rows in Properties; there is no separate Transform panel with Duplicate and Transform, and no Info panel with the pointer's colour and position (P-050). |
| History panel | **done** | With saved commands ([ADR 0097](adr/0097-command-macros.md)). |
| Scene panel | **done** | A scene menu on the edit bar and Modify › Scene ([ADR 0011](adr/0011-scenes.md)). |
| Movie Explorer | **done** | [ADR 0058](adr/0058-find-and-replace-and-movie-explorer.md). Renaming from the tree is missing. |
| Find and Replace | **done** | Text, fonts, colours, symbols, sounds, bitmaps ([ADR 0058](adr/0058-find-and-replace-and-movie-explorer.md)). Missing: font size and style, colour tolerance. |
| Output panel | **done** | In Test Movie and Window › Output. |
| Actions panel | **done** | See Interactivity. |
| Motion Editor panel | **done** | See Tweens. |
| Layer Depth panel | **done** | [ADR 0082](adr/0082-camera-depth-attaching-and-colour.md). |
| Brush Library panel | **done** | [ADR 0068](adr/0068-brush-library-pressure-curves-and-the-paint-brush.md). |
| Frame Picker panel | **missing** | P-026 ([#452](https://github.com/storytold/anicraft/issues/452)). |
| Motion Presets panel | **missing** | P-041. |
| Components and Component Parameters panels | **won't do** | They hold a ready-made widget set for a retired runtime. Buttons, text fields and plug-ins cover the use; see Interactivity. |
| Code Snippets panel | **missing** | P-051. The Actions panel's list of simple actions covers the commonest snippets. |
| Assets panel (stock and saved assets) | **partial** | Templates and the Library cover saved assets on this computer; the stock assets are a service (won't do). |
| Cloud libraries panel | **won't do** | A proprietary service. |
| Edit bar: scene and symbol path, zoom | **done** | |
| Rulers, guides and grid | **done** | [ADR 0076](adr/0076-rulers-grid-and-guides.md). See Drawing model for snapping. |
| Stage view: zoom, fit, 100 %, pasteboard, Clip Content Outside the Stage | **done** | [#251](https://github.com/storytold/anicraft/issues/251). |
| Stage view: rotate the view | **missing** | P-029 (part of outside PR [#394](https://github.com/storytold/anicraft/issues/394)). |
| Preview modes (outlines for the whole document, fast, anti-aliased) | **partial** | Outlines are per layer and for all layers at once from the timeline header. Missing: the View › Preview Mode menu (P-050). |
| Command palette | **ours** | Ctrl+K finds any command by name. |
| Help that opens at the thing you point at | **ours** | F1, [ADR 0096](adr/0096-guide-search-and-context-help.md). |

## Tools

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Selection | **partial** | Raw shapes, objects, marquee, bend an edge, move a corner ([ADR 0041](adr/0041-selection-tool-on-raw-drawings.md), [ADR 0044](adr/0044-selection-across-layers.md)). A marquee takes the instances and objects it touches, and View › Hide Edges hides the selection's marks ([ADR 0134](adr/0134-outside-parity-batch-zoom-outlines-and-egui-0-37.md)). Missing: a preference for the marquee to take only what it surrounds; the first-use hint about fills and outlines (P-017). |
| Subselection | **done** | [ADR 0042](adr/0042-subselection-and-merging-everywhere.md), [ADR 0045](adr/0045-raw-points-and-raw-shapes-inside-symbols.md). |
| Free Transform | **partial** | Scale, skew, rotate, the transformation point, a box that turns with the object ([ADR 0111](adr/0111-the-transformation-point-a-turning-box-and-layer-opacity.md)), Envelope ([ADR 0080](adr/0080-drawing-parity-leftovers.md)). Missing: Distort. Open bug: rotation on Windows and FreeBSD ([#479](https://github.com/storytold/anicraft/issues/479)), not reproduced on Windows 10 (P-005). |
| Gradient Transform | **done** | Gradient and bitmap fills, Lock Fill. |
| Lasso | **missing** | P-018 ([#515](https://github.com/storytold/anicraft/issues/515)). |
| Polygon (lasso) | **missing** | P-018. |
| Magic Wand | **missing** | P-018. |
| Pen, with Add, Delete and Convert Anchor Point | **done** | [ADR 0115](adr/0115-pen-parity-and-keep-area-tweens.md). Add, delete and convert are gestures of the Pen and Subselection, not tools of their own. |
| Text | **partial** | See Text. Typing happens in a field beside the stage, not in the block itself (P-019; [#303](https://github.com/storytold/anicraft/issues/303)). |
| Line | **done** | |
| Rectangle and Oval | **done** | Corner radius; the oval's start and end angle and inner radius are missing (P-020). |
| Rectangle Primitive and Oval Primitive | **missing** | Shapes whose corners and angles stay editable after drawing (P-020). |
| PolyStar | **done** | [ADR 0080](adr/0080-drawing-parity-leftovers.md). |
| Pencil | **done** | Straighten, Smooth and Ink. |
| Classic Brush | **done** | Tips, pressure, five paint modes ([ADR 0033](adr/0033-brush-tool.md), [ADR 0080](adr/0080-drawing-parity-leftovers.md)). |
| Paint Brush (strokes with a brush look) | **partial** | [ADR 0068](adr/0068-brush-library-pressure-curves-and-the-paint-brush.md), [ADR 0093](adr/0093-tilt-textured-brushes-and-profiles-in-the-merge.md): width-profiled strokes, a brush library made of parameters. Missing: art and pattern brushes drawn from artwork (P-028). |
| Fluid Brush | **partial** | Pressure curves, velocity, tapers and smoothing exist as Brush settings ([ADR 0068](adr/0068-brush-library-pressure-curves-and-the-paint-brush.md)); there is no separate tool. Stroke response is the complaint in [#287](https://github.com/storytold/anicraft/issues/287) and outside PR [#336](https://github.com/storytold/anicraft/issues/336) (P-028). |
| Pressure and tilt from a tablet | **partial** | Pressure on Windows, macOS and X11. Missing: Linux Wayland (outside PR [#320](https://github.com/storytold/anicraft/issues/320)), tilt on desktop systems (the window library doesn't report it) (P-014; [#290](https://github.com/storytold/anicraft/issues/290)). |
| Eraser, with modes and the Faucet | **done** | [ADR 0034](adr/0034-eraser-and-eyedropper.md). Pressure on the eraser is missing. |
| Paint Bucket, with gap closing | **done** | [#57](https://github.com/storytold/anicraft/issues/57) can close. |
| Ink Bottle | **done** | |
| Eyedropper | **done** | [ADR 0034](adr/0034-eraser-and-eyedropper.md), [ADR 0109](adr/0109-shape-commands-picker-eyedropper-and-transformation-origins.md). |
| Width | **done** | [ADR 0018](adr/0018-variable-width-strokes.md). |
| Bone and Bind | **done** | See Bones. |
| Asset Warp | **partial** | [ADR 0079](adr/0079-asset-warp.md). Missing: bones on the warp mesh, a mesh that follows the outline, symbols (P-042; [#444](https://github.com/storytold/anicraft/issues/444)). |
| Camera | **done** | See Camera. |
| Hand | **done** | Space with any tool. |
| Zoom | **done** | |
| Rotation (turn the stage view) | **missing** | P-029. |
| Time Scrub | **missing** | Drag on the stage to move the playhead (P-029). |
| 3D Rotation and 3D Translation | **missing** | P-044 ([#516](https://github.com/storytold/anicraft/issues/516), [#53](https://github.com/storytold/anicraft/issues/53)). |
| Object Drawing toggle and Snap to Objects magnet | **done** | |
| Deco and Spray Brush | **won't do** | Its maker removed them from the reference app. A spray brush is proposed in outside PR [#445](https://github.com/storytold/anicraft/issues/445) (P-047). |
| Tool options in Properties while a tool is active | **done** | |

## Drawing model

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Merge drawing: raw strokes and fills that cut and join | **done** | [ADR 0031](adr/0031-merge-drawing.md), [ADR 0041](adr/0041-selection-tool-on-raw-drawings.md), [ADR 0042](adr/0042-subselection-and-merging-everywhere.md), [ADR 0045](adr/0045-raw-points-and-raw-shapes-inside-symbols.md); [drawing](guide/drawing.md). Open bug: a hairline between merged curved fills ([#182](https://github.com/storytold/anicraft/issues/182), P-013). |
| Object drawing | **done** | Edit inside by double-click. Editing without opening is missing ([#181](https://github.com/storytold/anicraft/issues/181)). |
| Groups, nested, edited in place | **done** | [ADR 0073](adr/0073-groups-combine-objects-and-selection-modifiers.md). |
| Break Apart | **done** | |
| Combine Objects: Union, Intersect, Punch, Crop | **done** | [ADR 0073](adr/0073-groups-combine-objects-and-selection-modifiers.md). One layer at a time. |
| Arrange (front, back, forward, backward) | **done** | |
| Align, distribute | **done** | |
| Match Size, Space evenly | **done** | `align.match_width`, `align.match_height`, `align.match_size`, `align.space_h`, `align.space_v` (#566). |
| Transform: rotate 90°, flip, remove transform, scale and rotate by numbers | **done** | |
| Envelope | **done** | [ADR 0080](adr/0080-drawing-parity-leftovers.md). Shapes and drawing objects only. |
| Distort | **missing** | P-020. |
| Modify › Shape: Convert Lines to Fills, Expand Fill, Soften Fill Edges | **done** | [ADR 0109](adr/0109-shape-commands-picker-eyedropper-and-transformation-origins.md). Properties repeats Convert Lines to Fills at the top of a shape's rows; with only a fill selected the button is unavailable. |
| Modify › Shape: Smooth, Straighten, Advanced Smooth, Optimize | **missing** | P-020 ([#510](https://github.com/storytold/anicraft/issues/510)). |
| Stroke styles: solid, dashed, dotted; caps, joins, miter | **done** | [ADR 0046](adr/0046-stroke-caps-joins-and-dashes.md). |
| Stroke styles: ragged, stipple, hatched | **missing** in authoring | They import from Animate files as solid lines and the report says so† (P-032). |
| Stroke scale modes and hairlines | **partial** | Hairlines are drawn in outline view only. Missing: Normal, Horizontal, Vertical, None scaling and the hairline style (P-032). |
| Variable-width strokes and width profiles | **done** | [ADR 0018](adr/0018-variable-width-strokes.md). Saved custom profiles are missing. |
| Snapping: objects, grid, guides, pixels | **done** | [ADR 0076](adr/0076-rulers-grid-and-guides.md). |
| Snap Align and Edit Snapping; units other than pixels; a movable ruler origin | **partial** | Missing as listed (P-030; [#44](https://github.com/storytold/anicraft/issues/44)). |
| Cut, copy, paste, Paste in Place, Duplicate | **done** | A picture copied in another app pastes as a bitmap ([ADR 0108](adr/0108-floating-paste-clipboard-pictures-and-refused-merges.md)). |
| Vectors pasted from other drawing apps | **partial** | Pictures paste; vectors don't yet (P-038; [#62](https://github.com/storytold/anicraft/issues/62)). |
| Trace Bitmap, Convert to Bitmap | **done** | Missing: a live preview in the dialog. |
| Edit Multiple Frames | **done** | [ADR 0047](adr/0047-onion-skin-and-edit-multiple-frames.md), [ADR 0051](adr/0051-edit-multiple-frames-complete-and-onion-skin-in-symbols.md). |

## Colour

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Stroke and fill colours, swap, black and white, none | **done** | |
| Colour picker: swatches, mixer, hex, alpha | **done** | [#432](https://github.com/storytold/anicraft/issues/432) to [#437](https://github.com/storytold/anicraft/issues/437); a selected solid fill also has a Fill alpha row directly in Properties ([#645](https://github.com/storytold/anicraft/issues/645)). |
| Live preview on the artwork while pointing at a swatch | **missing** | P-025 ([#436](https://github.com/storytold/anicraft/issues/436)). |
| Solid, linear and radial fills; overflow modes; linear RGB; focal point | **done** | [ADR 0074](adr/0074-gradient-editor-gradient-transform-and-lock-fill.md). |
| Bitmap fills | **done** | |
| Gradient and bitmap strokes | **missing** | Proposed in [ADR 0080](adr/0080-drawing-parity-leftovers.md) (P-032). |
| Lock Fill | **done** | |
| Swatches panel: document swatches, reorder, import and export | **done** | [ADR 0078](adr/0078-swatches-panel.md). Missing: swatch folders, sorting. |
| Tagged swatches (change one, every use follows) | **partial** | Swatches are saved colours; uses don't follow a change (P-025). |
| Adobe swatch files (`.aco`, `.ase`, `.clr`) | **won't do** | Proprietary formats. GIMP `.gpl` and our own JSON are read. |
| Online colour themes | **won't do** | A proprietary service. |
| Colour effect on instances: brightness, tint, alpha, advanced | **done** | |
| Eyedropper from anywhere on the stage while a picker is open | **done** | [ADR 0109](adr/0109-shape-commands-picker-eyedropper-and-transformation-origins.md). Picking outside the window is missing. |

## Text

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Static text: font, size, colour, bold, italic, alignment, spacing, wrap width | **done** | [ADR 0012](adr/0012-text.md); [text](guide/text.md). |
| Several styles in one block | **done** | [ADR 0085](adr/0085-per-character-and-paragraph-styles.md). |
| Typing in the block on the stage, shown in its real font | **missing** | P-019 ([#303](https://github.com/storytold/anicraft/issues/303)). |
| Dynamic and input text, instance names, embedded characters | **done** | [ADR 0066](adr/0066-dynamic-and-input-text-fields.md), [ADR 0107](adr/0107-text-paths-in-place-lines-on-a-path-and-the-text-field-api.md). |
| `htmlText`, `maxChars`, `restrict`, change events | **done** | [ADR 0107](adr/0107-text-paths-in-place-lines-on-a-path-and-the-text-field-api.md). |
| Scrolling fields, selection and focus from scripts | **partial** | Missing: `scrollV`, selection members (P-031). |
| Links on text | **partial** | Written to SVG; they don't click in Test Movie or the HTML page (P-035). |
| Justified text | **missing** | Drawn left-aligned (P-033). |
| Superscript, subscript, first-line indent, margins | **missing** | P-033. |
| Vertical text | **missing** | P-033. |
| Mixed directions (Hebrew or Arabic with Latin and numbers) | **partial** | Static text, text on a path and fields follow the Unicode bidi algorithm, in every export; in a field the arrow keys move the caret the way they point ([ADR 0130](adr/0130-mixed-directions-and-fallback-faces.md); [text](guide/text.md)). Missing: a direction setting for a paragraph with alignment to its start, and right-to-left text a script sets in the HTML page (P-033). |
| Letters the font lacks drawn from a fallback face | **done** | From a list of system faces, then any installed face; the command says which family lent them ([ADR 0130](adr/0130-mixed-directions-and-fallback-faces.md)). |
| Anti-alias modes and device fonts | **partial** | Text is stored as outlines, so it draws the same everywhere; there is no choice of anti-aliasing or of device fonts. |
| Break Apart text into letters, then shapes | **done** | One step to shapes. |
| Filters on text | **done** | [ADR 0099](adr/0099-filter-colour-and-stepped-tracks-built-in-presets-and-filters-on-text.md). |
| Spell checking | **missing** | P-033. |
| Font embedding dialog | **done** | The Embed list on a field ([ADR 0107](adr/0107-text-paths-in-place-lines-on-a-path-and-the-text-field-api.md)), with the font's licence bits checked. |
| Text on a path | **ours** | [ADR 0070](adr/0070-soft-masks-and-text-on-a-path.md), [ADR 0107](adr/0107-text-paths-in-place-lines-on-a-path-and-the-text-field-api.md). |
| Text Layout Framework text | **won't do** | Retired by its maker; such text in old files comes in as plain blocks?. |
| Fonts from an online font service | **won't do** | A proprietary service; fonts on this computer are used. |

## Symbols and Library

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Graphic, movie clip and button symbols | **done** | [symbols](guide/symbols-and-library.md). |
| Convert to Symbol, with name, type, folder | **done** | Missing: the registration point grid in the dialog (P-021). |
| New Symbol (empty, Ctrl+F8) | **missing** | P-021. |
| Edit in place, edit symbols, the path back | **done** | [ADR 0045](adr/0045-raw-points-and-raw-shapes-inside-symbols.md). A rotated instance opens upright. |
| Graphic looping: loop, play once, single frame, first frame | **done** | Reverse loop modes import from Animate files†. |
| Swap Symbol, Duplicate Symbol | **done** | |
| Break apart a symbol into layers; convert layers to a symbol | **missing** | P-021 ([#517](https://github.com/storytold/anicraft/issues/517)). |
| Button states and hit area, button sounds | **done** | [ADR 0057](adr/0057-button-sounds-and-hit-areas.md). |
| 9-slice scaling | **partial** | [ADR 0055](adr/0055-nine-slice-scaling.md). Missing in the HTML canvas export, which scales the whole instance and says so (P-035). |
| Transformation point kept on an instance | **done** | [ADR 0111](adr/0111-the-transformation-point-a-turning-box-and-layer-opacity.md). |
| Library: folders, sort, search, preview, use counts, unused items | **done** | [ADR 0050](adr/0050-library-management.md). |
| Drag from the Library to the stage | **done** | [ADR 0053](adr/0053-library-drag-swap-bitmap-mixed-properties.md). [#50](https://github.com/storytold/anicraft/issues/50) can close. |
| Item properties: linkage class, smoothing | **partial** | Missing: linkage on bitmaps and sounds, bitmap compression settings (P-040). |
| Swap Bitmap on an instance | **done** | Swapping every use from the Library is missing (P-040). |
| Edit a bitmap in an outside editor | **missing** | P-040. |
| Another open document's Library; drag between documents | **partial** | Pasting an instance brings its symbols; the other Library isn't shown (P-040). |
| Shared libraries (author-time and runtime) | **missing** | Author-time sharing: P-040. Runtime sharing belongs to a retired runtime (won't do). |
| Symbols exported for scripts (class linkage) | **done** | `script.set_class`. |
| Instance names, colour effect, blend, filters, visibility | **done** | |
| Sprite sheet from a symbol | **done** | From the timeline or a symbol†. |

## Timeline

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Frames, keyframes, blank keyframes: insert, remove, clear | **done** | [timeline](guide/timeline.md). |
| Frame selection, drag to move, cut, copy and paste frames | **done** | A click on the ruler leaves no frames selected, so F5 acts on every layer ([#195](https://github.com/storytold/anicraft/issues/195), [ADR 0131](adr/0131-layer-selection-the-ruler-and-new-masks.md)). |
| Span length by dragging | **done** | [ADR 0083](adr/0083-motion-editor-and-timeline-retiming.md). |
| Convert to Keyframes | **done** | [#272](https://github.com/storytold/anicraft/issues/272). |
| Reverse Frames | **missing** | P-021. |
| Distribute to Layers, Distribute to Keyframes | **partial** | Distribute to Layers: `modify.distribute_to_layers` (#517; objects, not raw shapes or tweened keyframes). Missing: Distribute to Keyframes (P-021). |
| Convert a tween to frame-by-frame animation | **missing** | P-021. Bake to Keyframes exists for data bindings only. |
| Frame labels | **partial** | Names only. Missing: comment and anchor labels (comments in Animate files are dropped and reported) (P-021). |
| Onion skin: markers, colours, outlines, per layer | **done** | [ADR 0047](adr/0047-onion-skin-and-edit-multiple-frames.md), [ADR 0061](adr/0061-timeline-polish.md). |
| Edit Multiple Frames | **done** | [ADR 0051](adr/0051-edit-multiple-frames-complete-and-onion-skin-in-symbols.md). |
| Playback: play, loop, step, first and last frame | **done** | |
| Loop a range of frames | **missing** | Loop Playback loops the whole scene (P-021). |
| Current frame and frame rate as fields | **done** | [#461](https://github.com/storytold/anicraft/issues/461), [#462](https://github.com/storytold/anicraft/issues/462). |
| Frame View sizes, short rows, layer height | **done** | [ADR 0061](adr/0061-timeline-polish.md). |
| Frame thumbnails in the cells (Preview) | **missing** | P-050. |
| Scenes: add, duplicate, rename, reorder | **done** | [ADR 0011](adr/0011-scenes.md). |
| Layers on the timeline | **done** | See Layers. |
| Tween spans in their colours, property-key diamonds | **done** | |
| Frame-by-frame scrubbing with sound | **done** | [ADR 0095](adr/0095-audio-output-and-sound-polish.md). |
| Centre the playhead; scroll to the playhead | **partial** | The view follows the playhead; there is no Centre Frame button?. |
| Split Audio on the timeline | **missing** | P-022 ([#502](https://github.com/storytold/anicraft/issues/502)). |
| Paste and Overwrite Frames; Paste Motion | **missing** | P-041. |
| Undo and redo buttons on the timeline | **missing** | P-050 ([#369](https://github.com/storytold/anicraft/issues/369)). Edit › Undo and the History panel do it. |
| Auto-keyframe while editing in a tween | **done** | |
| Keyframe navigation (previous and next keyframe) | **missing** | P-026 ([#51](https://github.com/storytold/anicraft/issues/51)). |
| Layer parenting view | **done** | [ADR 0113](adr/0113-layer-parenting.md). |
| Camera and depth controls in the timeline header | **done** | |

## Tweens and the Motion Editor

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Classic tweens: position, scale, rotation, skew, colour, filters | **done** | [tweens](guide/tweens.md). |
| Classic tween options: rotate direction and turns, orient to path, sync, snap, scale | **partial** | Orient and extra turns†. Missing: Sync and Snap as switches (P-041). |
| Classic ease presets and the custom ease editor | **done** | [ADR 0081](adr/0081-ease-presets-and-custom-eases.md). |
| Eases per property group on a classic tween | **done** | [ADR 0081](adr/0081-ease-presets-and-custom-eases.md)†. |
| Classic motion guides | **done** | |
| Motion tweens: one object, property keyframes | **done** | [ADR 0016](adr/0016-motion-tweens.md). |
| Motion paths edited on the stage | **done** | [ADR 0030](adr/0030-motion-paths.md), [ADR 0038](adr/0038-motion-path-points.md), [ADR 0049](adr/0049-motion-path-pins.md). |
| Motion Editor: curves per property, eases, handles | **done** | [ADR 0083](adr/0083-motion-editor-and-timeline-retiming.md), [ADR 0092](adr/0092-surviving-gpu-failures-and-motion-editor-filter-tracks.md), [ADR 0099](adr/0099-filter-colour-and-stepped-tracks-built-in-presets-and-filters-on-text.md). |
| Split, join, retime, stretch a motion tween | **done** | [ADR 0083](adr/0083-motion-editor-and-timeline-retiming.md). |
| Motion presets: save a tween, apply it elsewhere | **missing** | P-041. |
| Copy and paste motion; paste properties | **missing** | P-041. |
| 3D properties in a motion tween | **missing** | With the 3D tools (P-044). |
| Shape tweens | **done** | A raw shape's outline morphs with its fill, whatever the number of sides, and rides a motion guide with the fill it edges (an edge two fills share goes with the lower one). |
| Shape hints | **done** | [ADR 0023](adr/0023-shape-hints.md). [#82](https://github.com/storytold/anicraft/issues/82) can close. Missing: hints inside a symbol edited in place. |
| Shape tween blend (distributive, angular) | **partial** | One blend; no choice?. |
| Keep area while tweening | **ours** | [ADR 0115](adr/0115-pen-parity-and-keep-area-tweens.md). |
| Motion tweens from Animate files | **missing** | They open as keyframes without curves or paths (P-023). |
| Saved eases in the document | **done** | [ADR 0081](adr/0081-ease-presets-and-custom-eases.md). |

## Bones and rigging

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Armatures of symbol instances | **done** | [ADR 0043](adr/0043-bones-and-inverse-kinematics.md); [bones](guide/masks-guides-camera-bones.md). |
| Armatures inside a shape, with the Bind tool | **done** | [ADR 0071](adr/0071-shape-armatures.md). |
| Poses keyed and tweened, with easing | **done** | |
| Joint limits, pinning | **done** | |
| Joint translation, spring and damping | **done** | [ADR 0084](adr/0084-bone-springs-joint-translation-and-editing-members.md). |
| Mixed armatures of instances and shapes | **done** | [ADR 0098](adr/0098-mixed-armatures.md). |
| Painted weights | **ours** | [ADR 0090](adr/0090-painted-weights-and-reshaping-bound-shapes.md). |
| Rigging on a warp mesh: bones made with Asset Warp, soft and hard bones | **missing** | P-042 ([#444](https://github.com/storytold/anicraft/issues/444), [#213](https://github.com/storytold/anicraft/issues/213)). |
| Rig mapping: put a saved rig onto other artwork | **missing** | P-042. |
| Runtime armatures (posed by script while the movie plays) | **missing** | P-042. |
| Armatures from Animate files | **partial** | They open as plain layers and the report says so (P-043). |
| Parenting layers as a simple rig | **done** | [ADR 0113](adr/0113-layer-parenting.md). A parent that changes over time is missing. |
| Squash and stretch on a bone | **partial** | Not in the reference app either; asked for in [#503](https://github.com/storytold/anicraft/issues/503) (P-042). |

## Camera

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Camera layer: pan, zoom, rotate | **done** | [ADR 0027](adr/0027-camera-layer.md). |
| Camera tweens | **done** | Classic and motion. |
| Camera colour effects: tint, brightness, advanced | **done** | [ADR 0082](adr/0082-camera-depth-attaching-and-colour.md). |
| Layer depth with perspective; the Layer Depth panel | **done** | [ADR 0082](adr/0082-camera-depth-attaching-and-colour.md). Depth inside symbols is missing. |
| Attach a layer to the camera | **done** | [ADR 0082](adr/0082-camera-depth-attaching-and-colour.md). |
| Camera and layer depth from Animate files | **partial** | No public description of how they are stored; they open as plain layers (P-043). |

## Layers

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Normal layers: add, delete, rename, reorder by drag, show, lock, outline | **done** | [#54](https://github.com/storytold/anicraft/issues/54), [#59](https://github.com/storytold/anicraft/issues/59) and [#61](https://github.com/storytold/anicraft/issues/61) can close. |
| Layer folders | **done** | |
| Mask and masked layers | **done** | A new mask is locked, as in the reference app, so its edge in the layer colour shows only while it is unlocked for editing ([#429](https://github.com/storytold/anicraft/issues/429), [ADR 0131](adr/0131-layer-selection-the-ruler-and-new-masks.md)). |
| Soft masks (alpha, luminance) | **ours** | [ADR 0070](adr/0070-soft-masks-and-text-on-a-path.md). |
| Guide and motion guide layers | **done** | |
| Layer Properties dialog | **done** | [ADR 0061](adr/0061-timeline-polish.md). One layer at a time (P-027; [#464](https://github.com/storytold/anicraft/issues/464)). |
| Selecting several layers; acting on them together | **partial** | Ctrl-click, Shift-click and Shift+Up/Down select; drag, Delete Layer and New Folder act on them all ([#512](https://github.com/storytold/anicraft/issues/512), [ADR 0131](adr/0131-layer-selection-the-ruler-and-new-masks.md)). Missing: the row toggles and Move Layer Up and Down on several layers, Layer Properties for several (P-027). |
| Layer colour, layer height | **done** | |
| Layer opacity | **done** | [ADR 0111](adr/0111-the-transformation-point-a-turning-box-and-layer-opacity.md). It shows in exports; an authoring-only transparency is missing. |
| Layer parenting | **done** | [ADR 0113](adr/0113-layer-parenting.md). |
| Layer depth | **done** | See Camera. |
| Layer effects: colour effect, blend mode and filters on a layer or a frame | **missing** | P-027 ([#464](https://github.com/storytold/anicraft/issues/464), [#60](https://github.com/storytold/anicraft/issues/60)). |
| Camera layer, armature layers, sound on a layer | **done** | |
| Copy, cut, paste and duplicate layers | **missing**? | Not found in the command reference (P-021). |

## Filters and blends

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Drop shadow, blur, glow, bevel, gradient glow, gradient bevel, adjust colour | **done** | [ADR 0025](adr/0025-bevel-and-gradient-filters.md). [#278](https://github.com/storytold/anicraft/issues/278) can close. |
| Filter list: reorder, switch off, copy and paste, presets | **done** | [ADR 0091](adr/0091-filter-presets-order-on-off-and-tweens.md). |
| Filters tweened, and as curves in the Motion Editor | **done** | [ADR 0092](adr/0092-surviving-gpu-failures-and-motion-editor-filter-tracks.md), [ADR 0099](adr/0099-filter-colour-and-stepped-tracks-built-in-presets-and-filters-on-text.md). |
| Filters on movie clips, buttons and text | **done** | |
| Filters on graphic symbols, layers and frames | **missing** | P-027. |
| Blend modes (the fourteen) on instances | **done** | |
| Blend modes on a layer or a frame | **partial** | Layer opacity only (P-027). |
| Filters and blends in every renderer and export | **done** | CPU, GPU, HTML; SVG and Lottie say what they can't carry. |

## Sound

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Import WAV, MP3, Ogg Vorbis | **done** | [ADR 0020](adr/0020-audio.md); [images and sound](guide/images-and-sound.md). |
| Import AIFF, compressed WAV, FLAC, AAC | **missing** | P-037. |
| Sound on a keyframe; waveform on the timeline | **done** | |
| Sync: Event, Start, Stop, Stream | **done** | |
| Repeat and loop | **done** | |
| Effects and the envelope editor: fades, left and right, custom | **done** | [ADR 0039](adr/0039-sound-effects-volume-and-symbol-sounds.md). |
| In and out points (trim) in the envelope editor | **missing** | P-022 ([#502](https://github.com/storytold/anicraft/issues/502)). |
| Split Audio | **missing** | P-022 ([#502](https://github.com/storytold/anicraft/issues/502)). |
| Layer volume and mute | **ours** | [ADR 0039](adr/0039-sound-effects-volume-and-symbol-sounds.md). |
| Sounds inside symbols; button sounds | **done** | [ADR 0057](adr/0057-button-sounds-and-hit-areas.md). |
| Scrubbing with sound | **done** | [ADR 0095](adr/0095-audio-output-and-sound-polish.md). |
| Lip sync onto a mouth symbol | **partial** | [ADR 0114](adr/0114-audio-to-keyframes-and-lip-sync.md): five mouth shapes; the reference app tells about a dozen apart (P-054). |
| Convert audio to keyframes | **ours** | [ADR 0114](adr/0114-audio-to-keyframes-and-lip-sync.md). |
| Sound in exports: video, HTML, audio file | **done** | Missing: audio in Lottie (the format has none worth using). |
| Sound from scripts | **partial** | Timeline sounds play; sound and `stopAllSounds` from scripts are missing (P-031). |
| Per-sound compression settings for publishing | **partial** | The HTML page embeds sounds as stored; there are no per-sound settings (P-036). |
| Choosing the output device | **missing** | The system default is used (P-054). |

## Video

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Import video as reference frames or a reference layer | **done** | [ADR 0116](adr/0116-importing-video-through-the-users-ffmpeg.md), through the user's own ffmpeg. [#369](https://github.com/storytold/anicraft/issues/369) can close for video. |
| Video embedded on the timeline, scrubbed with the playhead | **partial** | The frames come in as bitmaps; the video's sound does not come with a reference layer (P-045). |
| Video with an alpha channel | **missing** | P-045. |
| A playback component that loads video while the movie plays | **won't do** | It belongs to a retired runtime. The HTML export could carry a `<video>` element: P-045. |
| FLV and F4V encoding and cue points | **won't do** | Retired formats. |
| Export video (MP4, WebM, MOV) | **done** | Through the user's ffmpeg; the reference app sends this to a separate encoder app. |
| Video from Animate files | **partial** | Screen Video opens as a still; other codecs are named in the report. |
| Video in the web build | **missing** | No ffmpeg in a browser (P-045). |

## Interactivity and scripting

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Actions panel with a script editor, problems shown by line | **done** | [ADR 0024](adr/0024-frame-scripts-and-test-movie.md); [interactivity](guide/interactivity.md). |
| Actions without writing code (a wizard) | **done** | The Actions panel's list of actions. |
| Frame scripts: timeline control, events, properties, variables, functions, loops | **done** | [ADR 0052](adr/0052-actionscript-small-logic.md): a subset of ActionScript 3.0 that runs whole or not at all. |
| ActionScript 3.0 beyond the subset: arrays, objects, `switch`, strings, classes | **partial** | P-031 ([#3](https://github.com/storytold/anicraft/issues/3)). Scripts are always kept with the document. |
| Event model: bubbling, stage listeners, event fields | **partial** | P-031. |
| ActionScript 1.0 and 2.0; scripts on instances | **missing** | Kept and reported, not run. Low priority: P-031 ([#293](https://github.com/storytold/anicraft/issues/293)). |
| JavaScript frame scripts for HTML canvas documents | **missing** | The HTML page runs the same script subset; hand-written JavaScript in frames is not run (P-031). |
| Test Movie | **partial** | Its own window, Output, pointer and keys. Missing: full screen, zoom, remembered size (P-035). |
| Test Scene; Test in a browser | **missing** | Export HTML Canvas and open the page (P-035). |
| Debugger: breakpoints, stepping, variables | **missing** | `trace` and the Output panel only (P-031). |
| Compiler errors and warnings panel | **done** | In the Actions panel and Output. |
| Code hints and completion | **missing**? | Problems are shown by line; no completion or syntax colouring was found (P-051). |
| Document class and symbol classes | **partial** | Names are kept; class files don't run. |
| Components (buttons, lists, sliders, video players) | **won't do** | A widget set for a retired runtime. |
| Runtime targets and their APIs (the desktop and mobile runtime, the browser plug-in) | **won't do** | Retired or proprietary runtimes. |
| Custom platform support through a vendor SDK | **won't do** | A proprietary SDK. Plug-ins can add export formats ([ADR 0017](adr/0017-wasm-plugins.md)). |
| Buttons with events without code | **done** | |
| Text fields from scripts | **done** | See Text. |

## Publishing and export

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Publish Settings, profiles, Publish | **done** | [ADR 0060](adr/0060-publish-settings-and-publish.md); [exporting](guide/exporting-and-publish.md). |
| HTML canvas page | **done** | [ADR 0028](adr/0028-html-canvas-export.md): one self-contained page. Gaps: 9-slice, bent bitmaps, stream sounds after a go-to (P-035). |
| HTML page options: responsive scaling, centre, loop, preloader, hosted libraries, separate assets | **partial** | One inline page. Missing: responsive scaling choices, a preloader, assets as separate files, a template (P-035). |
| Video | **done** | MP4, WebM and others through ffmpeg. |
| Animated GIF | **done** | |
| PNG sequence; a single PNG | **done** | |
| JPEG image and sequence | **missing** | P-036. |
| SVG image | **done** | And an SVG sequence ([#504](https://github.com/storytold/anicraft/issues/504)). |
| Sprite sheet with data | **done** | JSON. Missing: other engines' data formats (P-036). |
| Texture atlas for game engines | **missing** | P-036. |
| Animated PNG and WebP | **ours** | [ADR 0063](adr/0063-animated-png-and-webp-export.md). Lossy WebP is missing: no permissive encoder exists. |
| Lottie | **ours** | [ADR 0089](adr/0089-lottie-keyframes-and-eases.md). |
| Audio file | **done** | WAV. |
| SWF | **won't do** | The player is retired and blocked by browsers. Artwork from SWF files is a separate question: see Import. |
| Projectors (stand-alone players) | **won't do** | Retired with the player. |
| Desktop and mobile app packages | **won't do** | A proprietary runtime. |
| WebGL and glTF documents | **won't do** for now | The reference app kept them in beta; the HTML canvas page covers the web. |
| VR documents | **won't do** | A beta feature of the reference app with no stable target. |
| Animated widget packages for other layout apps | **won't do** | A proprietary package format. |
| Quick share to social sites and video services | **won't do** | Services; export a video or GIF and upload it. |
| Export Image and Export Animated GIF dialogs with a preview and optimisation | **partial** | Options in the export dialog; no side-by-side preview (P-036). |
| Publish settings read from Animate files | **missing** | P-036. |
| Export formats added by plug-ins | **ours** | [ADR 0017](adr/0017-wasm-plugins.md); not yet in Publish (P-051). |

## Import formats

| Feature | Status | Notes | Headless |
|---|---|---|---|
| `.fla` (zipped, CS5 and later) and XFL folders | **partial** | [ADR 0009](adr/0009-xfl-import.md), [ADR 0054](adr/0054-fla-packages-and-a-hardened-zip-reader.md); [opening Animate files](guide/opening-animate-files.md). Missing: motion tweens (P-023), armatures, camera, parenting, sound in and out points (P-043). A stroke of a kind AniCraft doesn't know is drawn solid; a fill of an unknown kind, and the rest of an outline run after a command it can't read, are left out (the runs after it are drawn); each is counted in the report ([ADR 0133](adr/0133-an-animate-shape-is-never-cut-down-quietly.md)). Open reports, cause not confirmed: [#498](https://github.com/storytold/anicraft/issues/498), [#347](https://github.com/storytold/anicraft/issues/347) (P-006). |
| Binary `.fla` (Flash 8 to CS4) | **partial** | [ADR 0120](adr/0120-legacy-fla-compound-binary-import.md); [opening Animate files](guide/opening-animate-files.md). Scenes, layers and folders, keyframes, shapes with gradients and dashed and dotted strokes, groups, symbol instances, classic and shape tweens, text, bitmaps, sounds on frames, frame scripts (kept, not run) and library folders ([#163](https://github.com/storytold/anicraft/issues/163); outside PRs [#478](https://github.com/storytold/anicraft/issues/478) and [#481](https://github.com/storytold/anicraft/issues/481)). Missing: filters, movie clip component settings, bitmap fills, sound in and out points; Flash MX 2004 and earlier (P-039; [#289](https://github.com/storytold/anicraft/issues/289)). |
| Saving `.fla` or XFL | **missing** | AniCraft saves `.anicraft`. Not planned before import is complete (P-043). |
| SWF artwork | **missing** | Needs a clean-room and licence decision first (P-048; [#172](https://github.com/storytold/anicraft/issues/172)). |
| SVG | **done** | [ADR 0021](adr/0021-svg-clips-masks-text.md). Missing: matching the stage to the SVG ([#155](https://github.com/storytold/anicraft/issues/155), P-038). |
| PNG, JPEG | **done** | |
| GIF (animated, as frames), BMP, WebP, TIFF | **missing**? | PNG and JPEG are read; no reader for the others was found (P-037). |
| Image sequences as frames | **partial**? | Files dropped together are routed by type ([ADR 0059](adr/0059-files-from-the-desktop-recent-files-and-start-screen.md)); numbered files becoming successive keyframes was not verified (P-037). |
| Illustrator files, with layers | **won't do** as such | A proprietary format. SVG and the vector clipboard (P-038) are the route. |
| Photoshop files, with layers | **missing** | The layer format is documented publicly; a candidate through craft-libs (P-038). |
| Sound: WAV, MP3, Ogg | **done** | |
| Sound: AIFF, FLAC, ADPCM WAV | **missing** | P-037. |
| Video | **done** | See Video. |
| Vectors from the clipboard | **missing** | P-038 ([#62](https://github.com/storytold/anicraft/issues/62)). |
| Colour palettes: `.gpl`, our JSON | **done** | |
| Colour palettes: Adobe formats | **won't do** | Proprietary. |
| Data tables: CSV, TSV, JSON | **ours** | [ADR 0118](adr/0118-data-tables-that-drive-animation.md). |

## Assets and sharing

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Templates: built in and your own | **done** | [ADR 0065](adr/0065-templates-svg-into-the-library-and-drops-on-the-timeline.md). |
| Save an animation or a rig as a reusable asset | **partial** | Copy between documents and templates do it; there is no asset file with a rig in it (P-042). |
| Stock assets from the vendor | **won't do** | A service. |
| Cloud libraries, cloud documents, sync of settings | **won't do** | Proprietary services. |
| Share for review online | **won't do** | A service. |
| Publish to social sites | **won't do** | A service. |
| Package a document with its media (XFL folder) | **done** | `.anicraft` holds its media. Linked reference video stays outside (P-045). |
| Document recovery (autosave) | **done** | [ADR 0015](adr/0015-crash-recovery-autosave.md). |
| File information and metadata | **missing** | P-050. |
| Export and import of brushes, filter presets, swatches and saved commands as files | **done** | JSON files. |
| A user ease or brush library shared between documents | **missing** | Eases live in the document (P-041). |

## Accessibility

| Feature | Status | Notes | Headless |
|---|---|---|---|
| The app by keyboard: every control reachable, no trap | **done** | [ADR 0022](adr/0022-accessibility.md); [docs/accessibility.md](accessibility.md). |
| The app with a screen reader | **partial** | Names and roles through AccessKit, checked by test. Missing: passes by hand with Narrator, VoiceOver and Orca (P-015). |
| High contrast and interface scaling | **done** | A light high-contrast palette is missing. |
| Accessible content: names, descriptions and tab order on instances for the published movie | **missing** | The HTML page gives the canvas one label; objects inside have none (P-049). |
| Captions and text alternatives in the published page | **partial** | Text fields are real text?; no caption track (P-049). |
| Colour never the only signal; contrast checked by test | **done** | [ADR 0022](adr/0022-accessibility.md), [ADR 0064](adr/0064-accessibility-second-pass.md). A reduced-motion setting: ?. |
| Shortcuts that can be changed | **done** | |

## Preferences and shortcuts

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Preferences dialog | **done** | [preferences](guide/preferences-shortcuts-accessibility.md). |
| Keyboard shortcut editor, with the reference app's keys by default | **done** | Saving a set of keys as a file to share: ?. |
| Interface themes | **done** | Dark, light, high contrast, and your own ([ADR 0117](adr/0117-user-themes-from-a-folder.md)). |
| Interface languages | **partial** | The machinery is there ([ADR 0040](adr/0040-interface-languages.md)); no full translation ships yet (P-055). Right-to-left: P-034 ([#45](https://github.com/storytold/anicraft/issues/45)). |
| Undo levels; document-level undo | **done** | A memory budget instead of a count ([ADR 0001](adr/0001-undo-persistent-snapshots.md)). Object-level undo is missing. |
| Drawing preferences: smoothing, recognise lines and shapes, click accuracy | **partial** | Smoothing per tool. Missing: shape recognition, accuracy settings (P-030). |
| Sync settings through an account | **won't do** | A service. |
| Renderer and GPU choices | **done** | With fallbacks ([ADR 0092](adr/0092-surviving-gpu-failures-and-motion-editor-filter-tracks.md)). |
| Autosave interval and recovery | **done** | |

## Automation

| Feature | Status | Notes | Headless |
|---|---|---|---|
| Scripting the app (the reference app's JavaScript API) | **partial** | Every command runs from the CLI, JSON-lines `serve`, MCP and the live control port ([ADR 0104](adr/0104-live-control-port.md), [ADR 0124](adr/0124-automation-roots.md)); the reference app's own script files don't run (see below). |
| Running the reference app's script files (`.jsfl`) | **won't do** | Its API is the app's object model. Plug-ins and saved commands are our route. |
| Commands menu: save steps from History, run them again | **done** | [ADR 0097](adr/0097-command-macros.md); [saved commands](guide/saved-commands.md). |
| Extensions and custom panels | **partial** | WebAssembly plug-ins add commands, eases and export formats ([ADR 0017](adr/0017-wasm-plugins.md)); they can't draw panels yet (P-051). |
| Custom tools written by users | **missing** | P-051. |
| Headless rendering and export | **ours** | `anicraft-cli`; [working headless](guide/working-headless.md). |
| Agents: an MCP server limited to the folders you give it | **ours** | [ADR 0124](adr/0124-automation-roots.md). |
| Data-driven animation | **ours** | [ADR 0118](adr/0118-data-tables-that-drive-animation.md). |
