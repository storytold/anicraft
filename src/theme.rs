//! Design tokens. Every colour, radius and target size in the shell comes from here
//! (docs/ui-guidelines.md).
//!
//! Three palettes share these roles: Graphite (dark, the default), Light (ADR 0077) and high
//! contrast (ADR 0022). The look (ADR 0008): graphite neutrals, geometric controls, borders only where they separate
//! layout regions, flat buttons, and vibrant colour spent where it carries meaning (timeline
//! blocks, playhead, selection). Corners follow the role of a shape, not one global value:
//! structure is square, controls are softened slightly, floating surfaces are rounder.

use egui::{Color32, CornerRadius, Shadow, Stroke, Visuals};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Menu bar, status bar: the frame around the work.
    pub chrome: Color32,
    pub panel: Color32,
    /// Panel headers, alternate rows, windows.
    pub panel_alt: Color32,
    /// Raised surfaces: buttons at rest.
    pub header: Color32,
    /// Buttons under the pointer.
    pub hover: Color32,
    /// The active tool or a toggle that is on: a lighter block, its icon in `on_active`.
    pub active: Color32,
    pub on_active: Color32,
    /// Icons at rest: a step down from `icon_hot` so the active one stands out.
    pub icon: Color32,
    /// Icons under the pointer.
    pub icon_hot: Color32,
    /// The tint inside an icon's closed shapes, under its outline (ADR 0117): two-tone icons for
    /// themes that want colour in them. Fully transparent, as in the built-in palettes: outlines
    /// only. Drawn only under icons in the icon roles (`icon`, `icon_hot`, `on_active`).
    pub icon_accent: Color32,
    /// Inset surfaces: text and number fields.
    pub field: Color32,
    /// Lines between layout regions and around popups.
    pub border: Color32,
    /// Edges of checkboxes and radio buttons (≥ 3:1 so the unticked state is visible).
    pub control_edge: Color32,
    /// Fine grid lines inside a region (timeline cells, row rules).
    pub separator: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Selection, focus, active tool.
    pub accent: Color32,
    pub accent_dim: Color32,
    /// Where an accent bar ends (ADR 0117): the bars that mark structure (a section band's edge,
    /// the active tab's line, a picked row's edge) run from `accent` to this along their length.
    /// The same as `accent` in the built-in palettes: a flat bar.
    pub accent_end: Color32,
    /// Accent-coloured text (the current frame, links, prompts): the accent lightened until it
    /// reads at 4.5:1 on the panels.
    pub accent_text: Color32,
    /// Text and icons on filled `accent` and `danger` buttons (Save, Delete, Restore).
    pub on_fill: Color32,
    /// Dark marks drawn on vibrant blocks (keyframe dots, tween arrows, labels, the number on
    /// the playhead's flag).
    pub ink: Color32,
    pub pasteboard: Color32,
    pub stage_shadow: Color32,
    /// Timeline: span holding artwork.
    pub span: Color32,
    /// Timeline: empty span.
    pub span_empty: Color32,
    /// Timeline: frame cells of the active layer.
    pub row_active: Color32,
    /// Timeline: classic tween span (violet), shape tween span (amber) and motion tween span
    /// (sky blue, also its curves in the Motion Editor).
    pub tween: Color32,
    pub tween_shape: Color32,
    pub tween_motion: Color32,
    /// Timeline: a sound's waveform on an empty span (on artwork spans it is drawn in ink).
    pub waveform: Color32,
    pub tween_line: Color32,
    pub playhead: Color32,
    pub selection: Color32,
    /// Destructive buttons.
    pub danger: Color32,
    pub error: Color32,
    pub ok: Color32,
    /// Shape hint markers on the stage (ADR 0023): not on an edge yet (rose), placed on the
    /// tween's first keyframe (amber, like the shape tween span) and on its last (mint).
    pub hint_unplaced: Color32,
    pub hint_start: Color32,
    pub hint_end: Color32,
    /// Onion skin (ADR 0047): frames before the playhead (blue, as Animate users know it) and
    /// after it (green); the defaults the onion menu can change. Also the ruler's markers.
    pub onion_past: Color32,
    pub onion_future: Color32,
    /// A symbol's 9-slice guides while it is open for editing (ADR 0055), dashed over a light
    /// underlay so they read on any artwork.
    pub slice_guide: Color32,
    /// A button's hit area (ADR 0057): laid over its other states while it is open for editing,
    /// and over the movie in Test Movie's Show Hit Areas, at [`HIT_AREA_ALPHA`].
    pub hit_area: Color32,
    /// Library symbol kinds.
    pub kind_graphic: Color32,
    pub kind_movie_clip: Color32,
    pub kind_button: Color32,
    /// Buttons and fields carry a visible `control_edge` (WCAG 1.4.11: a control's boundary at
    /// 3:1). Off in the graphite look, where fills and hover carry it; on in high contrast.
    pub control_outline: bool,
    /// A light-on-dark palette (Graphite, high contrast) or dark-on-light (Light, ADR 0077): which
    /// of egui's base looks the widgets start from (text cursor, scroll bars, resize grips).
    pub dark: bool,
    /// Buttons and fields carry a hairline in `border` at rest: the light look, where a flat grey
    /// on a light panel would read as a printed box rather than a control.
    pub control_hairline: bool,
    /// Timeline: the tint on every fifth frame column (a whisper of light on dark, of shade on
    /// light).
    pub cell_tint: Color32,
    /// The line around the stage, where it meets the pasteboard.
    pub stage_edge: Color32,
    /// Opacity of the shadow under windows and under menus and tooltips (black).
    pub window_shadow: u8,
    pub popup_shadow: u8,
    /// Corner radii by role (see `Shape` below).
    pub radius_control: u8,
    pub radius_menu: u8,
    pub radius_window: u8,
}

/// Corner shapes are chosen by what a surface *is*:
///
/// * **Structure is square** (`0.0`): panels, section bands, the timeline grid and its blocks,
///   selections, the stage. They tile against each other; rounding would leave gaps.
/// * **Controls are softened** (`radius_control`): buttons, fields, tools, list highlights,
///   swatches, the playhead flag. Rectangular and geometric, but not sharp.
/// * **Floating surfaces are rounder** (`radius_menu`, `radius_window`): menus, tooltips,
///   dialogs and windows sit above the work, and their shape says so.
/// * **Round only where the thing is round**: keyframe dots, dials, onion-skin marks.
pub const SQUARE: f32 = 0.0;

/// The graphite look (ADR 0008), the default.
pub const GRAPHITE: Tokens = Tokens {
    chrome: Color32::from_rgb(0x13, 0x13, 0x16),
    panel: Color32::from_rgb(0x1b, 0x1b, 0x1f),
    panel_alt: Color32::from_rgb(0x22, 0x22, 0x27),
    header: Color32::from_rgb(0x2c, 0x2c, 0x32),
    hover: Color32::from_rgb(0x3a, 0x3a, 0x42),
    active: Color32::from_rgb(0x4a, 0x4a, 0x53),
    on_active: Color32::WHITE,
    icon: Color32::from_rgb(0xb4, 0xb4, 0xbc),
    icon_hot: Color32::WHITE,
    icon_accent: Color32::TRANSPARENT,
    field: Color32::from_rgb(0x0e, 0x0e, 0x11),
    border: Color32::from_rgb(0x36, 0x36, 0x3e),
    control_edge: Color32::from_rgb(0x78, 0x78, 0x82),
    separator: Color32::from_rgb(0x26, 0x26, 0x2c),
    text: Color32::from_rgb(0xee, 0xee, 0xf0),
    text_dim: Color32::from_rgb(0xb0, 0xb0, 0xb8),
    // ≥ 4.5:1 on every surface up to `header`, so even hints stay readable.
    text_faint: Color32::from_rgb(0x98, 0x98, 0xa2),
    accent: Color32::from_rgb(0x2e, 0x66, 0xf0),
    accent_end: Color32::from_rgb(0x2e, 0x66, 0xf0),
    accent_dim: Color32::from_rgb(0x1f, 0x35, 0x70),
    accent_text: Color32::from_rgb(0x6f, 0x97, 0xff),
    on_fill: Color32::WHITE,
    ink: Color32::from_rgb(0x0e, 0x0e, 0x10),
    pasteboard: Color32::from_rgb(0x2a, 0x2a, 0x30),
    stage_shadow: Color32::from_black_alpha(150),
    span: Color32::from_rgb(0x22, 0xc7, 0xa9),
    span_empty: Color32::from_rgb(0x2e, 0x2e, 0x35),
    row_active: Color32::from_rgb(0x24, 0x27, 0x33),
    tween: Color32::from_rgb(0x8a, 0x5c, 0xff),
    tween_shape: Color32::from_rgb(0xf2, 0xa1, 0x3b),
    tween_motion: Color32::from_rgb(0x4c, 0xb8, 0xf5),
    waveform: Color32::from_rgb(0x7c, 0xc4, 0xff),
    tween_line: Color32::from_rgb(0x0e, 0x0e, 0x10),
    playhead: Color32::from_rgb(0xff, 0x4d, 0x3d),
    selection: Color32::from_rgb(0x2e, 0x66, 0xf0),
    danger: Color32::from_rgb(0xd8, 0x34, 0x2a),
    error: Color32::from_rgb(0xff, 0x6e, 0x60),
    ok: Color32::from_rgb(0x5f, 0xe0, 0x9a),
    hint_unplaced: Color32::from_rgb(0xff, 0x5c, 0x8a),
    hint_start: Color32::from_rgb(0xff, 0xc2, 0x3d),
    hint_end: Color32::from_rgb(0x4f, 0xe3, 0xa8),
    onion_past: Color32::from_rgb(0x4f, 0x8d, 0xff),
    onion_future: Color32::from_rgb(0x4c, 0xd0, 0x7d),
    slice_guide: Color32::from_rgb(0xd9, 0x3c, 0xc4),
    hit_area: Color32::from_rgb(0x1f, 0xc8, 0x8c),
    kind_graphic: Color32::from_rgb(0x22, 0xc7, 0xa9),
    kind_movie_clip: Color32::from_rgb(0x5a, 0x8f, 0xff),
    kind_button: Color32::from_rgb(0xff, 0xb5, 0x3d),
    control_outline: false,
    dark: true,
    control_hairline: false,
    cell_tint: Color32::from_rgba_premultiplied(6, 6, 6, 6),
    stage_edge: Color32::from_black_alpha(160),
    window_shadow: 150,
    popup_shadow: 130,
    radius_control: 3,
    radius_menu: 6,
    radius_window: 10,
};

/// High contrast (ADR 0022): the same roles on black, with white text, a yellow accent for
/// focus, selection and the active tool, outlined controls and brighter timeline blocks. Every
/// pair a role puts together meets WCAG AA (text 4.5:1, most of it 7:1 and more; UI parts 3:1),
/// checked by `guidelines_tests`.
pub const HIGH_CONTRAST: Tokens = Tokens {
    chrome: Color32::from_rgb(0x00, 0x00, 0x00),
    panel: Color32::from_rgb(0x00, 0x00, 0x00),
    panel_alt: Color32::from_rgb(0x10, 0x10, 0x10),
    header: Color32::from_rgb(0x1c, 0x1c, 0x1c),
    hover: Color32::from_rgb(0x38, 0x38, 0x38),
    active: Color32::from_rgb(0xff, 0xd8, 0x1a),
    on_active: Color32::from_rgb(0x00, 0x00, 0x00),
    icon: Color32::from_rgb(0xff, 0xff, 0xff),
    icon_hot: Color32::from_rgb(0xff, 0xd8, 0x1a),
    icon_accent: Color32::TRANSPARENT,
    field: Color32::from_rgb(0x00, 0x00, 0x00),
    border: Color32::from_rgb(0x9a, 0x9a, 0x9a),
    control_edge: Color32::from_rgb(0xd6, 0xd6, 0xd6),
    separator: Color32::from_rgb(0x44, 0x44, 0x44),
    text: Color32::from_rgb(0xff, 0xff, 0xff),
    text_dim: Color32::from_rgb(0xe8, 0xe8, 0xe8),
    text_faint: Color32::from_rgb(0xc8, 0xc8, 0xc8),
    accent: Color32::from_rgb(0xff, 0xd8, 0x1a),
    accent_end: Color32::from_rgb(0xff, 0xd8, 0x1a),
    accent_dim: Color32::from_rgb(0x4d, 0x3d, 0x00),
    accent_text: Color32::from_rgb(0xff, 0xd8, 0x1a),
    on_fill: Color32::from_rgb(0x00, 0x00, 0x00),
    ink: Color32::from_rgb(0x00, 0x00, 0x00),
    pasteboard: Color32::from_rgb(0x1c, 0x1c, 0x1c),
    stage_shadow: Color32::from_black_alpha(200),
    span: Color32::from_rgb(0x3d, 0xf0, 0xcf),
    span_empty: Color32::from_rgb(0x26, 0x26, 0x26),
    row_active: Color32::from_rgb(0x1f, 0x1a, 0x00),
    tween: Color32::from_rgb(0xb3, 0x99, 0xff),
    tween_shape: Color32::from_rgb(0xff, 0xb8, 0x5c),
    tween_motion: Color32::from_rgb(0x6b, 0xd0, 0xff),
    tween_line: Color32::from_rgb(0x00, 0x00, 0x00),
    waveform: Color32::from_rgb(0x8f, 0xd3, 0xff),
    playhead: Color32::from_rgb(0xff, 0x6a, 0x5c),
    // Drawn over the artwork, which is usually light: a deep blue stays visible on white.
    selection: Color32::from_rgb(0x1f, 0x5b, 0xff),
    danger: Color32::from_rgb(0xff, 0x70, 0x63),
    error: Color32::from_rgb(0xff, 0x9a, 0x8f),
    ok: Color32::from_rgb(0x74, 0xff, 0xb0),
    hint_unplaced: Color32::from_rgb(0xff, 0x7a, 0xa3),
    hint_start: Color32::from_rgb(0xff, 0xd8, 0x1a),
    hint_end: Color32::from_rgb(0x74, 0xff, 0xb0),
    onion_past: Color32::from_rgb(0x6f, 0xa8, 0xff),
    onion_future: Color32::from_rgb(0x5c, 0xf0, 0x9a),
    slice_guide: Color32::from_rgb(0xff, 0x6e, 0xe6),
    hit_area: Color32::from_rgb(0x3d, 0xff, 0xb0),
    kind_graphic: Color32::from_rgb(0x3d, 0xf0, 0xcf),
    kind_movie_clip: Color32::from_rgb(0x85, 0xad, 0xff),
    kind_button: Color32::from_rgb(0xff, 0xc8, 0x5c),
    control_outline: true,
    dark: true,
    control_hairline: false,
    cell_tint: Color32::from_rgba_premultiplied(6, 6, 6, 6),
    stage_edge: Color32::from_black_alpha(160),
    window_shadow: 150,
    popup_shadow: 130,
    radius_control: 3,
    radius_menu: 6,
    radius_window: 10,
};

/// A colour written as a hex triplet, `0xrrggbb`, as the light palette is specified (ADR 0077).
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The light look (ADR 0077): designed for daylight, not the dark one inverted. Cool paper greys
/// a step apart (the frame darkest, raised surfaces lightest, fields white), near-black ink for
/// text, hairlines on controls, soft shadows. Colour still goes where it means something, but in
/// deeper mid-tones: the timeline's mint, violet, amber and sky blocks sit at 3:1 against the
/// panels and keep dark marks at 4.5:1, the same rules Graphite meets.
pub const LIGHT: Tokens = Tokens {
    chrome: rgb(0xe1e1e6),
    panel: rgb(0xf0f0f3),
    panel_alt: rgb(0xf8f8fa),
    header: rgb(0xe3e3e8),
    hover: rgb(0xd5d5dc),
    active: rgb(0xd0dcfa),
    on_active: rgb(0x0b1f5c),
    icon: rgb(0x4c4c56),
    icon_hot: rgb(0x101014),
    icon_accent: Color32::TRANSPARENT,
    field: rgb(0xffffff),
    border: rgb(0xc9c9d1),
    control_edge: rgb(0x7a7a85),
    separator: rgb(0xdfdfe5),
    text: rgb(0x17171b),
    text_dim: rgb(0x44444d),
    text_faint: rgb(0x5c5c66),
    accent: rgb(0x2457e0),
    accent_end: rgb(0x2457e0),
    accent_dim: rgb(0xd3dffb),
    accent_text: rgb(0x1d4ccc),
    on_fill: rgb(0xffffff),
    ink: rgb(0x101014),
    pasteboard: rgb(0xd2d2d9),
    stage_shadow: Color32::from_black_alpha(34),
    span: rgb(0x15957d),
    span_empty: rgb(0xdbdbe2),
    row_active: rgb(0xe2e8f7),
    tween: rgb(0x8f6cf5),
    tween_shape: rgb(0xb97a12),
    tween_motion: rgb(0x2e8fcf),
    waveform: rgb(0x1a5ca8),
    tween_line: rgb(0x101014),
    playhead: rgb(0xe8463a),
    selection: rgb(0x2457e0),
    danger: rgb(0xc42e26),
    error: rgb(0xb8261c),
    ok: rgb(0x126e3e),
    hint_unplaced: rgb(0xf05582),
    hint_start: rgb(0xf2a51f),
    hint_end: rgb(0x2fc88c),
    onion_past: rgb(0x2f6ce6),
    onion_future: rgb(0x1f9a50),
    slice_guide: rgb(0xc42fb0),
    hit_area: rgb(0x13b07a),
    kind_graphic: rgb(0x128a74),
    kind_movie_clip: rgb(0x2f62d8),
    kind_button: rgb(0xb06c08),
    control_outline: false,
    dark: false,
    control_hairline: true,
    cell_tint: Color32::from_black_alpha(9),
    stage_edge: Color32::from_black_alpha(70),
    window_shadow: 46,
    popup_shadow: 40,
    radius_control: 3,
    radius_menu: 6,
    radius_window: 10,
};

/// Which palette the interface uses (Preferences › Appearance › Theme).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Palette {
    /// Dark (Graphite), the default look. (Saved under its old name, so older files still open.)
    #[default]
    Graphite,
    HighContrast,
    /// Light (ADR 0077).
    Light,
}

impl Palette {
    /// The palettes Preferences offers, in the order it lists them: name and a one-line note.
    pub const ALL: [(Palette, &'static str, &'static str); 3] = [
        (Palette::Graphite, tr_noop!("Dark (Graphite)"), tr_noop!("The standard dark look: graphite panels, colour where it means something.")),
        (Palette::Light, tr_noop!("Light"), tr_noop!("Light grey panels and dark text, for bright rooms and eyes that find dark interfaces hard.")),
        (Palette::HighContrast, tr_noop!("High contrast"), tr_noop!("White text on black, a yellow focus and selection colour, outlined controls.")),
    ];

    pub fn tokens(self) -> &'static Tokens {
        match self {
            Palette::Graphite => &GRAPHITE,
            Palette::HighContrast => &HIGH_CONTRAST,
            Palette::Light => &LIGHT,
        }
    }

    fn index(self) -> u8 {
        match self {
            Palette::Graphite => 0,
            Palette::HighContrast => 1,
            Palette::Light => 2,
        }
    }
}

/// The palette in use. Process-wide view state, like the interface scale: one window, one look.
/// With [`USER_LOOK`] set, a theme of the user's is drawn over that palette (ADR 0117).
#[cfg(not(test))]
static ACTIVE: AtomicU8 = AtomicU8::new(0);

// Tests run side by side on threads of one process, each with its own app: each keeps its own
// palette, so switching themes in one never shows in another.
#[cfg(test)]
thread_local! {
    static ACTIVE: AtomicU8 = const { AtomicU8::new(0) };
    static USER: std::cell::Cell<Option<&'static Tokens>> = const { std::cell::Cell::new(None) };
}

/// Set in [`ACTIVE`] while a theme of the user's is drawn; the low bits stay its base palette.
const USER_LOOK: u8 = 0x80;

fn active() -> u8 {
    #[cfg(not(test))]
    return ACTIVE.load(Ordering::Relaxed);
    #[cfg(test)]
    return ACTIVE.with(|a| a.load(Ordering::Relaxed));
}

fn set_active(i: u8) {
    #[cfg(not(test))]
    ACTIVE.store(i, Ordering::Relaxed);
    #[cfg(test)]
    ACTIVE.with(|a| a.store(i, Ordering::Relaxed));
}

/// The user's theme in use (ADR 0117). Tokens are read tens of thousands of times a frame, from
/// any thread, as plain references: each set of tokens ever applied is kept for the life of the
/// program (a few hundred bytes; the same tokens are kept once), the one in use sits behind a
/// lock, and each thread remembers it until the count says it changed, so reading takes no lock.
#[cfg(not(test))]
mod user_look {
    use super::Tokens;
    use std::cell::Cell;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    static KEPT: Mutex<Vec<&'static Tokens>> = Mutex::new(Vec::new());
    static NOW: Mutex<Option<&'static Tokens>> = Mutex::new(None);
    static CHANGES: AtomicU32 = AtomicU32::new(0);
    thread_local! {
        static SEEN: Cell<(u32, Option<&'static Tokens>)> = const { Cell::new((0, None)) };
    }

    pub fn get() -> Option<&'static Tokens> {
        let changes = CHANGES.load(Ordering::Acquire);
        SEEN.with(|seen| {
            let (at, tokens) = seen.get();
            if at == changes {
                return tokens;
            }
            let tokens = NOW.lock().map_or(None, |now| *now);
            seen.set((changes, tokens));
            tokens
        })
    }

    pub fn set(tokens: Tokens) {
        let kept: Option<&'static Tokens> = KEPT.lock().ok().map(|mut all| match all.iter().find(|t| ***t == tokens) {
            Some(t) => *t,
            None => {
                let t: &'static Tokens = Box::leak(Box::new(tokens));
                all.push(t);
                t
            }
        });
        if let Ok(mut now) = NOW.lock() {
            *now = kept;
            // Counted under the lock: a thread that sees the new count reads the new tokens.
            CHANGES.fetch_add(1, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod user_look {
    use super::Tokens;

    pub fn get() -> Option<&'static Tokens> {
        super::USER.with(|u| u.get())
    }

    pub fn set(tokens: Tokens) {
        super::USER.with(|u| u.set(Some(Box::leak(Box::new(tokens)))));
    }
}

/// The built-in palette in use, or the one the user's theme in use is based on.
pub fn palette() -> Palette {
    match active() & !USER_LOOK {
        1 => Palette::HighContrast,
        2 => Palette::Light,
        _ => Palette::Graphite,
    }
}

/// Whether a theme of the user's is drawn (ADR 0117), rather than a built-in palette.
pub fn user_theme_active() -> bool {
    active() & USER_LOOK != 0
}

/// The tokens of the palette in use, read as `T.panel` or `T.rc()`. A palette change shows from
/// the next read on, so switching themes is live in the same frame.
pub struct ActiveTokens;

impl std::ops::Deref for ActiveTokens {
    type Target = Tokens;
    fn deref(&self) -> &Tokens {
        if user_theme_active()
            && let Some(t) = user_look::get()
        {
            return t;
        }
        palette().tokens()
    }
}

pub static T: ActiveTokens = ActiveTokens;

impl Tokens {
    /// Control corner radius for painter calls.
    pub fn rc(&self) -> f32 {
        f32::from(self.radius_control)
    }
}

/// The colour roles by name, in the order `Tokens` lists them: what a theme file may set (ADR
/// 0117) and what Export Current Theme writes. One list, so a role can't be read but not written.
macro_rules! colour_roles {
    ($($role:ident),* $(,)?) => {
        /// Every colour role's name.
        pub const COLOUR_ROLES: &[&str] = &[$(stringify!($role)),*];

        impl Tokens {
            /// The colour of the role named `role`.
            pub fn colour(&self, role: &str) -> Option<Color32> {
                match role {
                    $(stringify!($role) => Some(self.$role),)*
                    _ => None,
                }
            }

            /// Sets the role named `role`; false when there is no such role.
            pub fn set_colour(&mut self, role: &str, colour: Color32) -> bool {
                match role {
                    $(stringify!($role) => self.$role = colour,)*
                    _ => return false,
                }
                true
            }
        }
    };
}

colour_roles!(
    chrome,
    panel,
    panel_alt,
    header,
    hover,
    active,
    on_active,
    icon,
    icon_hot,
    icon_accent,
    field,
    border,
    control_edge,
    separator,
    text,
    text_dim,
    text_faint,
    accent,
    accent_dim,
    accent_end,
    accent_text,
    on_fill,
    ink,
    pasteboard,
    stage_shadow,
    span,
    span_empty,
    row_active,
    tween,
    tween_shape,
    tween_motion,
    waveform,
    tween_line,
    playhead,
    selection,
    danger,
    error,
    ok,
    hint_unplaced,
    hint_start,
    hint_end,
    onion_past,
    onion_future,
    slice_guide,
    hit_area,
    kind_graphic,
    kind_movie_clip,
    kind_button,
    cell_tint,
    stage_edge,
);

/// The roles that may be see-through (shadows, tints and the icon fill, laid over what is under
/// them). Every other role is a surface, a text colour or a mark and must be solid: a contrast
/// ratio means nothing against a colour you can see through.
pub const TRANSLUCENT_ROLES: &[&str] = &["icon_accent", "stage_shadow", "cell_tint", "stage_edge"];

/// A colour as a theme file writes it (ADR 0117): red, green, blue and opacity, 0 to 255.
pub fn role_colour(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// WCAG 2.x contrast ratio of two solid colours (relative luminance, 1:1 to 21:1).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let ch = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `over` laid on the solid colour `under`, as the painter blends it.
pub fn laid_over(over: Color32, under: Color32) -> Color32 {
    let keep = 255 - u16::from(over.a());
    let mix = |o: u8, u: u8| (u16::from(o) + (u16::from(u) * keep + 127) / 255).min(255) as u8;
    Color32::from_rgb(mix(over.r(), under.r()), mix(over.g(), under.g()), mix(over.b(), under.b()))
}

/// Which bar a palette is held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// The built-in palettes: WCAG AA and the stricter house rules (body text at 7:1).
    House,
    /// A theme of the user's (ADR 0117): WCAG AA, text 4.5:1 and what identifies a control 3:1.
    Aa,
}

/// A pair of roles drawn together that doesn't reach the contrast it needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Shortfall {
    /// The roles, by name: `text_dim on panel`.
    pub pair: String,
    pub ratio: f32,
    pub needs: f32,
    /// Text, icons and the focus ring: without them the interface can't be read or used, so a
    /// user's theme that falls short here is refused. The rest (blocks against the grid, control
    /// edges) is reported, not refused.
    pub required: bool,
}

/// Every pair of roles a palette draws together, with the ratio it must reach: text 4.5:1 (WCAG
/// 1.4.3 AA; the timeline's small numbers are small text too), UI components and focus 3:1
/// (1.4.11). The built-in palettes also meet stricter house rules. Returns the pairs that fall
/// short of `standard`; computed from the colours, never assumed from a palette's name.
pub fn shortfalls(t: &Tokens, standard: Standard) -> Vec<Shortfall> {
    let mut out = Vec::new();
    // `house`: what the built-ins meet. `aa`: the WCAG AA minimum for that kind of pair.
    let mut need = |pair: String, fg: Color32, bg: Color32, house: f32, aa: f32, required: bool| {
        let needs = if standard == Standard::House { house } else { aa };
        let ratio = contrast(fg, bg);
        if ratio < needs {
            out.push(Shortfall { pair, ratio, needs, required });
        }
    };
    let surfaces = [("panel", t.panel), ("panel_alt", t.panel_alt), ("chrome", t.chrome), ("header", t.header), ("field", t.field)];
    for (s, bg) in surfaces {
        need(format!("text on {s}"), t.text, bg, 7.0, 4.5, true);
        need(format!("text_dim on {s}"), t.text_dim, bg, 4.5, 4.5, true);
        need(format!("text_faint on {s}"), t.text_faint, bg, 4.5, 4.5, true);
        need(format!("accent_text on {s}"), t.accent_text, bg, 4.5, 4.5, true);
        need(format!("error on {s}"), t.error, bg, 4.5, 4.5, true);
        need(format!("ok on {s}"), t.ok, bg, 4.5, 4.5, true);
    }
    need("text on hover".into(), t.text, t.hover, 4.5, 4.5, true);
    // A selected row.
    need("text on accent_dim".into(), t.text, t.accent_dim, 4.5, 4.5, true);
    // Text, and the marks drawn across the timeline grid while dragging frames (the drop outline,
    // a span's new end), on the active row.
    need("text on row_active".into(), t.text, t.row_active, 4.5, 4.5, true);
    // Icons at rest and under the pointer, and on the active tool's block.
    need("icon on panel".into(), t.icon, t.panel, 4.5, 3.0, true);
    need("icon_hot on header".into(), t.icon_hot, t.header, 4.5, 3.0, true);
    need("icon_hot on hover".into(), t.icon_hot, t.hover, 4.5, 3.0, true);
    need("on_active on active".into(), t.on_active, t.active, 7.0, 3.0, true);
    if t.icon_accent.a() > 0 {
        // Two-tone icons (ADR 0117): the outline still reads on the tint inside it.
        need("icon on icon_accent (over panel)".into(), t.icon, laid_over(t.icon_accent, t.panel), 3.0, 3.0, true);
        need("icon_hot on icon_accent (over hover)".into(), t.icon_hot, laid_over(t.icon_accent, t.hover), 3.0, 3.0, true);
        need("on_active on icon_accent (over active)".into(), t.on_active, laid_over(t.icon_accent, t.active), 3.0, 3.0, true);
    }
    // Text on filled buttons (Save, Restore, destructive actions).
    need("on_fill on accent".into(), t.on_fill, t.accent, 4.5, 4.5, true);
    need("on_fill on danger".into(), t.on_fill, t.danger, 4.5, 4.5, true);
    // Marks on timeline blocks and the playhead's number; the blocks against the grid.
    for (b, bg) in [("span", t.span), ("tween", t.tween), ("tween_shape", t.tween_shape), ("tween_motion", t.tween_motion), ("playhead", t.playhead)]
    {
        need(format!("ink on {b}"), t.ink, bg, 4.5, 4.5, true);
        need(format!("{b} on panel"), bg, t.panel, 3.0, 3.0, false);
    }
    // Shape hint letters (ink) on every marker colour.
    for (h, bg) in [("hint_unplaced", t.hint_unplaced), ("hint_start", t.hint_start), ("hint_end", t.hint_end)] {
        need(format!("ink on {h}"), t.ink, bg, 4.5, 4.5, true);
    }
    need("text on span_empty".into(), t.text, t.span_empty, 4.5, 4.5, true);
    // Sound layers: waveforms on empty spans and on the active row.
    need("waveform on span_empty".into(), t.waveform, t.span_empty, 4.5, 3.0, false);
    need("waveform on row_active".into(), t.waveform, t.row_active, 4.5, 3.0, false);
    // Focus rings, selection, the active tool and checkbox edges (WCAG 1.4.11). An accent bar's
    // far end is as much a mark as its near one.
    for (s, bg) in [("panel", t.panel), ("panel_alt", t.panel_alt), ("chrome", t.chrome)] {
        need(format!("accent on {s}"), t.accent, bg, 3.0, 3.0, true);
        need(format!("accent_end on {s}"), t.accent_end, bg, 3.0, 3.0, false);
        need(format!("control_edge on {s}"), t.control_edge, bg, 3.0, 3.0, false);
    }
    need("active on panel".into(), t.active, t.panel, if t.control_outline { 3.0 } else { 1.0 }, 1.0, false);
    // Stage selection is drawn over artwork, usually on a white stage.
    need("selection on a white stage".into(), t.selection, Color32::WHITE, 3.0, 3.0, false);
    if !t.dark {
        // Light: the white stage stands off the pasteboard by its edge line, and the pasteboard
        // is a step darker than the panels so the work area reads as one region.
        need("text on pasteboard".into(), t.text, t.pasteboard, 7.0, 4.5, true);
        need("pasteboard on a white stage".into(), t.pasteboard, Color32::WHITE, 1.4, 1.0, false);
        need("selection on panel".into(), t.selection, t.panel, 3.0, 3.0, false);
    }
    if t.control_outline {
        // High contrast: every region border and control outline is a visible line.
        need("border on panel".into(), t.border, t.panel, 3.0, 3.0, false);
        need("control_edge on field".into(), t.control_edge, t.field, 3.0, 3.0, false);
        need("control_edge on header".into(), t.control_edge, t.header, 3.0, 3.0, false);
    }
    out
}

/// Whether the colours read light on dark: text lighter than the panels it sits on.
pub fn reads_as_dark(t: &Tokens) -> bool {
    contrast(t.text, Color32::BLACK) > contrast(t.panel, Color32::BLACK)
}

/// An accent bar (ADR 0117): the thin square bar that marks structure in the accent colour (a
/// section band's edge, the active tab's line, a picked row's edge). Flat in `accent` unless the
/// palette gives the bar another end (`accent_end`): then it runs from one to the other along its
/// length. The one place a gradient is drawn, so no widget invents its own.
pub fn accent_bar(p: &egui::Painter, rect: egui::Rect) {
    let (from, to) = (T.accent, T.accent_end);
    if from == to {
        p.rect_filled(rect, SQUARE, from);
        return;
    }
    let mut mesh = egui::Mesh::default();
    let along_y = rect.height() >= rect.width();
    for (corner, far) in [(rect.left_top(), false), (rect.right_top(), !along_y), (rect.right_bottom(), true), (rect.left_bottom(), along_y)] {
        mesh.colored_vertex(corner, if far { to } else { from });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
}

/// Smallest click target (WCAG 2.5.8 asks for 24 × 24; we use 28).
pub const TARGET: f32 = 28.0;
/// The rulers' thickness along the stage's top and left edges (ADR 0076).
pub const RULER: f32 = 18.0;
/// Tool buttons in the tools bar, and the glyph inside them.
pub const TOOL_BUTTON: f32 = 36.0;
pub const TOOL_ICON: f32 = 18.0;

/// Depth for floating surfaces: tight and downward, so a window lifts off the work without a
/// glow around it.
fn depth(y: i8, blur: u8, alpha: u8) -> Shadow {
    Shadow { offset: [0, y], blur, spread: 0, color: Color32::from_black_alpha(alpha) }
}

/// Fonts and the look, at start-up.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx, None, None, &[]);
    apply_palette(ctx, Palette::Graphite);
}

/// Switches the palette and restyles every widget with it (Preferences › Theme), live.
pub fn apply_palette(ctx: &egui::Context, palette: Palette) {
    set_active(palette.index());
    restyle(ctx, *palette.tokens());
}

/// Switches to a theme of the user's (ADR 0117): `tokens` as its file gave them, over `base`
/// (which the egui look and the title bar go by). The caller has checked them (`theme_file`);
/// nothing here is applied in part.
pub fn apply_user_theme(ctx: &egui::Context, base: Palette, tokens: Tokens) {
    user_look::set(tokens);
    set_active(base.index() | USER_LOOK);
    restyle(ctx, tokens);
}

fn restyle(ctx: &egui::Context, t: Tokens) {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.panel_alt;
    v.extreme_bg_color = t.field;
    v.text_edit_bg_color = Some(t.field);
    v.faint_bg_color = t.panel_alt;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent_dim;
    v.selection.stroke = Stroke::new(2.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_shadow = depth(10, 28, t.window_shadow);
    v.popup_shadow = depth(4, 12, t.popup_shadow);
    v.window_corner_radius = CornerRadius::same(t.radius_window);
    v.menu_corner_radius = CornerRadius::same(t.radius_menu);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(t.radius_control);
        w.expansion = 0.0;
    }
    // Separators, group frames and other non-interactive shapes are structure: square.
    v.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
    // Separators and group frames: the layout lines.
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text_dim);
    // Buttons are flat fills at rest; no border on every button (a hairline in the light look).
    let rest = if t.control_outline {
        Stroke::new(1.0, t.control_edge)
    } else if t.control_hairline {
        Stroke::new(1.0, t.border)
    } else {
        Stroke::NONE
    };
    v.widgets.inactive.weak_bg_fill = t.header;
    v.widgets.inactive.bg_fill = t.header;
    v.widgets.inactive.bg_stroke = rest;
    v.widgets.inactive.fg_stroke = Stroke::new(1.5, t.text);
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = if t.control_outline { Stroke::new(1.0, t.text) } else { Stroke::NONE };
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, t.text);
    // Pressed and keyboard-focused widgets (egui styles focus as "active"): a visible ring.
    v.widgets.active.weak_bg_fill = t.accent_dim;
    v.widgets.active.bg_fill = t.accent_dim;
    v.widgets.active.bg_stroke = Stroke::new(2.0, t.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.5, t.text);
    v.widgets.open.weak_bg_fill = t.hover;
    v.widgets.open.bg_fill = t.hover;
    v.widgets.open.bg_stroke = rest;
    // egui keeps a dark and a light style and picks one by the OS theme; both are ours, so the
    // palette chosen here is the one drawn (Follow system is decided in `Prefs::palette`). Naming
    // its theme keeps the window's title bar (Windows, macOS) dark or light with it.
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size = egui::vec2(TARGET, TARGET);
        s.spacing.icon_width = 16.0;
        s.spacing.icon_width_inner = 9.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll = scroll_bars();
        // Built-in text styles follow the roles, so plain widgets match the guidelines too.
        use egui::TextStyle;
        s.text_styles.insert(TextStyle::Body, font(Type::Body));
        s.text_styles.insert(TextStyle::Button, font(Type::Body));
        s.text_styles.insert(TextStyle::Small, font(Type::Caption));
        s.text_styles.insert(TextStyle::Heading, font(Type::Title));
        s.text_styles.insert(TextStyle::Monospace, font(Type::Data));
    });
}

/// Scroll bars (#483) are part of the layout, not an overlay that shows on hover: wherever there
/// is more than fits, a bar is there at rest, in its own lane beside the content, showing how
/// much more there is and where you are in it, and wide enough to grab without aiming.
pub const SCROLL_BAR_WIDTH: f32 = 10.0;

fn scroll_bars() -> egui::style::ScrollStyle {
    // The handle takes the text colour, toned down at rest so a dark theme isn't striped with
    // white bars: still at least 3:1 against the panel (a control's contrast), brighter under
    // the pointer and while dragged.
    // (egui applies these opacities only to "floating" bars, so the bar floats but keeps its
    // own lane at full width: laid out like a solid bar, toned like a floating one.)
    egui::style::ScrollStyle {
        floating: true,
        bar_width: SCROLL_BAR_WIDTH,
        floating_width: SCROLL_BAR_WIDTH,
        floating_allocated_width: SCROLL_BAR_WIDTH,
        handle_min_length: TARGET,
        bar_inner_margin: 2.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        dormant_handle_opacity: 0.45,
        active_handle_opacity: 0.7,
        interact_handle_opacity: 0.9,
        ..egui::style::ScrollStyle::floating()
    }
}

/// The exception: a one-row toolbar too wide for its place scrolls sideways under a thin bar
/// laid over it. A lane of its own would push everything below it down whenever the row's
/// contents changed width.
pub fn toolbar_scroll_bars(ui: &mut egui::Ui) {
    ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
}

mod craft_fonts {
    include!(concat!(env!("OUT_DIR"), "/fonts.rs"));
}

/// Text being typed on the stage: document content, so it follows the stage zoom rather than a
/// UI role (clamped to stay readable).
pub fn stage_text_font(size: f32) -> egui::FontId {
    egui::FontId::proportional(size.clamp(12.0, 64.0))
}

/// The editor's face for a run of stage text at `size` screen pixels (within what the editor
/// can show legibly).
pub fn stage_text_run_font(size: f32) -> egui::FontId {
    egui::FontId::proportional(if size.is_finite() { size.clamp(6.0, 96.0) } else { 14.0 })
}

/// A run's own colour (document data) as the editor shows it, at least faintly visible.
pub fn stage_text_colour(c: anicraft_engine::doc::Rgba) -> Color32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(c.r), b(c.g), b(c.b), b(c.a.max(0.25)))
}

/// The typefaces embedded at build time (Inter, JetBrains Mono), for text on the stage.
pub fn embedded_fonts() -> Vec<std::sync::Arc<[u8]>> {
    [craft_fonts::UI_FONT, craft_fonts::MONO_FONT].into_iter().flatten().map(std::sync::Arc::from).collect()
}

/// Noto Sans Georgian (craft-fonts `Geor`) when this build embedded a face that parses and covers
/// the Georgian letters. The interface offers Georgian only then (`i18n::ready_codes`), and
/// `install_fonts` loads the same face: epaint panics on bytes it cannot parse, so a damaged file
/// must never reach it.
pub fn georgian_font() -> Option<&'static [u8]> {
    static FACE: std::sync::OnceLock<Option<&'static [u8]>> = std::sync::OnceLock::new();
    *FACE.get_or_init(|| craft_fonts::GEORGIAN_FONT.filter(|b| covers_range(b, '\u{10D0}'..='\u{10FA}')))
}

/// Noto Sans Hebrew (craft-fonts `Hebr`) when this build embedded a face that parses and covers
/// the Hebrew letters (Alef..Tav), loaded under the same guard as [`georgian_font`]. It draws Hebrew
/// letters in names; the interface is not offered in Hebrew yet (`i18n::READY`).
pub fn hebrew_font() -> Option<&'static [u8]> {
    static FACE: std::sync::OnceLock<Option<&'static [u8]>> = std::sync::OnceLock::new();
    *FACE.get_or_init(|| craft_fonts::HEBREW_FONT.filter(|b| covers_range(b, '\u{05D0}'..='\u{05EA}')))
}

/// Whether `bytes` is a valid font file that covers every Unicode scalar in `range`.
fn covers_range(bytes: &[u8], range: std::ops::RangeInclusive<char>) -> bool {
    let letters: String = range.collect();
    let fonts = anicraft_engine::text::Fonts::from_faces([bytes.to_vec()]);
    fonts.layout(&letters, &anicraft_engine::text::Style::default()).is_ok_and(|laid| laid.missing == 0)
}

#[cfg(test)]
mod georgian_face_tests {
    #[test]
    fn garbage_bytes_are_not_a_font() {
        assert!(!super::covers_range(b"not a font file at all", '\u{10D0}'..='\u{10FA}'));
        assert!(!super::covers_range(&[], '\u{10D0}'..='\u{10FA}'));
        assert!(!super::covers_range(b"not a font file at all", '\u{05D0}'..='\u{05EA}'));
        assert!(!super::covers_range(&[], '\u{05D0}'..='\u{05EA}'));
    }
}

/// Which typefaces are active (for About and diagnostics), in the interface language.
pub fn font_summary() -> String {
    match (craft_fonts::UI_FONT, craft_fonts::MONO_FONT) {
        (Some(_), Some(_)) => tr!("Inter + JetBrains Mono (craft-fonts, {size} KB)", size = craft_fonts::CRAFT_FONT_BYTES / 1024),
        (Some(_), None) => tr!("Inter + Hack (fallback)").into(),
        (None, Some(_)) => tr!("Ubuntu (fallback) + JetBrains Mono").into(),
        (None, None) => tr!("Ubuntu + Hack (built-in defaults; build with CRAFT_FONTS_DIR for Inter + JetBrains Mono)").into(),
    }
}

/// A user-chosen system font: family name, file bytes, face index.
pub type UserFont = (String, Vec<u8>, u32);

/// Installs the UI typefaces (ADR 0005). Order in each family: the user's system font (from
/// Preferences), then the craft-fonts pair (when embedded at build time), then egui's bundled
/// fonts, which stay as fallbacks for glyph coverage, then Noto Sans Georgian / Hebrew (when
/// embedded) for the scripts none of them has, then `fallbacks`: faces of this computer for
/// scripts none of the above has (Japanese, Chinese, Korean, Arabic…; `font_fallback`, #482).
/// Being last, they change nothing that drew before.
pub fn install_fonts(ctx: &egui::Context, ui_font: Option<&UserFont>, data_font: Option<&UserFont>, fallbacks: &[UserFont]) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut defs = FontDefinitions::default();
    for (bytes, name, family) in
        [(craft_fonts::UI_FONT, "craft-inter", FontFamily::Proportional), (craft_fonts::MONO_FONT, "craft-jetbrains-mono", FontFamily::Monospace)]
    {
        if let Some(b) = bytes {
            defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(b)));
            defs.families.entry(family).or_default().insert(0, name.to_owned());
        }
    }
    for (bytes, name) in [(georgian_font(), "craft-noto-sans-georgian"), (hebrew_font(), "craft-noto-sans-hebrew")] {
        if let Some(b) = bytes {
            defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(b)));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families.entry(family).or_default().push(name.to_owned());
            }
        }
    }
    for (name, bytes, index) in fallbacks {
        let key = format!("fallback:{name}:{index}");
        let mut data = FontData::from_owned(bytes.clone());
        data.index = *index;
        defs.font_data.insert(key.clone(), Arc::new(data));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(key.clone());
        }
    }
    for (user, family) in [(ui_font, FontFamily::Proportional), (data_font, FontFamily::Monospace)] {
        if let Some((name, bytes, index)) = user {
            let key = format!("user:{name}");
            let mut data = FontData::from_owned(bytes.clone());
            data.index = *index;
            defs.font_data.insert(key.clone(), Arc::new(data));
            defs.families.entry(family).or_default().insert(0, key);
        }
    }
    ctx.set_fonts(defs);
}

/// Names of the bundled typefaces actually embedded in this build.
pub fn bundled_names() -> (&'static str, &'static str) {
    // i18n-exempt: font family names
    (if craft_fonts::UI_FONT.is_some() { "Inter" } else { "Ubuntu" }, if craft_fonts::MONO_FONT.is_some() { "JetBrains Mono" } else { "Hack" })
}

/// Typographic roles (docs/ui-guidelines.md › Typography). **Prose is proportional; data is
/// monospace.** Data means values that are measured, counted or typed as codes: frame numbers,
/// timecode, rulers, coordinates, sizes, percentages, readouts, shortcut keys, paths and
/// preformatted text. Fixed-width digits do not jitter while a value changes, and columns line
/// up. Everything people *read* (labels, menus, buttons, names, dialog text, hints) stays
/// proportional. Code outside this module uses these roles, never raw font sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Dialog titles.
    Title,
    /// Panel and section headings (sentence case, bold; see `section`).
    Heading,
    /// Names and primary text: layer and symbol names, list items.
    Body,
    /// Secondary prose: status messages, field labels.
    Label,
    /// Hints, explanations, footnotes.
    Caption,
    /// Values and readouts: numeric fields, fps, time, coordinates, zoom.
    Data,
    /// Dense data: ruler ticks, playhead and badge numbers.
    DataSmall,
    /// The one hero number: current frame.
    DataLarge,
    /// Keyboard shortcuts and keycaps.
    Key,
    /// File paths, ids and preformatted documents (licence texts, JSON).
    Path,
}

/// Font for a role.
pub fn font(t: Type) -> egui::FontId {
    use egui::FontId;
    match t {
        Type::Title => FontId::proportional(17.0),
        Type::Heading => FontId::proportional(12.5),
        Type::Body => FontId::proportional(13.0),
        Type::Label => FontId::proportional(12.0),
        Type::Caption => FontId::proportional(11.0),
        Type::Data => FontId::monospace(12.0),
        Type::DataSmall => FontId::monospace(10.0),
        Type::DataLarge => FontId::monospace(14.5),
        Type::Key => FontId::monospace(11.5),
        Type::Path => FontId::monospace(11.0),
    }
}

/// Default colour for a role (override with `.color()` for state: error, accent…).
pub fn color(t: Type) -> Color32 {
    match t {
        Type::Title | Type::Heading | Type::Body | Type::Data | Type::DataLarge | Type::Path => T.text,
        Type::Label | Type::DataSmall => T.text_dim,
        Type::Caption | Type::Key => T.text_faint,
    }
}

/// Horizontal row layout matching the active language direction.
pub fn row_layout() -> egui::Layout {
    if crate::i18n::is_rtl() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) }
}

/// Single-row horizontal layout matching the active language direction.
/// Mirrors [`egui::Ui::horizontal`] by constraining the row's initial height to
/// `interact_size.y` so `Align::Center` does not stretch across a panel or scroll area.
pub fn horizontal_row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let initial_size = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(initial_size, row_layout(), add)
}

/// A menu title with its access key underlined (ADR 0064), as Windows and Linux show it while
/// Alt is held or the menu bar has the keyboard. `at`: the character (not byte) to underline.
pub fn access_key_label(s: &str, at: Option<usize>) -> egui::text::LayoutJob {
    let visual = crate::i18n::display(s);
    let at = if crate::i18n::is_rtl() { at.and_then(|i| s.chars().nth(i)).and_then(|ch| visual.chars().position(|c| c == ch)) } else { at };
    let s = visual.as_ref();
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat { font_id: font(Type::Body), color: Color32::PLACEHOLDER, ..Default::default() };
    let marked = egui::TextFormat { underline: Stroke::new(1.0, T.text), ..plain.clone() };
    let cut = |n: usize| s.char_indices().nth(n).map_or(s.len(), |(b, _)| b);
    match at {
        Some(i) if i < s.chars().count() => {
            let (a, b) = (cut(i), cut(i + 1));
            job.append(s.get(..a).unwrap_or(""), 0.0, plain.clone());
            job.append(s.get(a..b).unwrap_or(""), 0.0, marked);
            job.append(s.get(b..).unwrap_or(""), 0.0, plain);
        }
        _ => job.append(s, 0.0, plain),
    }
    job
}

/// Styled text in a role that preserves its logical reading-order string alongside the
/// visually reordered `RichText` so accessibility labels and multi-line wrapping never see
/// pre-reordered characters.
#[derive(Clone)]
pub struct Text {
    rich: egui::RichText,
    logical: String,
    role: Type,
    color_override: Option<Color32>,
}

impl Text {
    /// Override the role colour while preserving the logical string for wrapping and accessibility.
    pub fn color(mut self, c: Color32) -> Self {
        self.rich = self.rich.color(c);
        self.color_override = Some(c);
        self
    }

    /// Render the text in bold while preserving the logical string.
    pub fn strong(mut self) -> Self {
        self.rich = self.rich.strong();
        self
    }

    /// Render the text in italics while preserving the logical string.
    pub fn italics(mut self) -> Self {
        self.rich = self.rich.italics();
        self
    }

    /// Render the text underlined while preserving the logical string.
    pub fn underline(mut self) -> Self {
        self.rich = self.rich.underline();
        self
    }

    /// Original logical reading-order text before visual RTL reordering.
    pub fn logical(&self) -> &str {
        &self.logical
    }
}

impl From<Text> for egui::WidgetText {
    fn from(t: Text) -> Self {
        egui::WidgetText::RichText(std::sync::Arc::new(t.rich))
    }
}

/// Inputs accepted by [`checkbox`], [`radio_value`], [`label`], and [`form_note`], carrying an
/// optional `(role, color_override, logical_text)` so controls keep logical accessibility names
/// and multi-line notes wrap logically before per-row visual reordering.
pub trait StyledText {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>);
}

impl StyledText for Text {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        let meta = (self.role, self.color_override, self.logical);
        (egui::WidgetText::RichText(std::sync::Arc::new(self.rich)), Some(meta))
    }
}

impl StyledText for &str {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        let visual = crate::i18n::display(self).into_owned();
        (egui::WidgetText::Text(visual), Some((Type::Body, None, self.to_owned())))
    }
}

impl StyledText for String {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        let visual = crate::i18n::display(&self).into_owned();
        (egui::WidgetText::Text(visual), Some((Type::Body, None, self)))
    }
}

impl StyledText for std::borrow::Cow<'_, str> {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        let logical = self.into_owned();
        let visual = crate::i18n::display(&logical).into_owned();
        (egui::WidgetText::Text(visual), Some((Type::Body, None, logical)))
    }
}

impl StyledText for egui::RichText {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        (egui::WidgetText::RichText(std::sync::Arc::new(self)), None)
    }
}

impl StyledText for egui::WidgetText {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        (self, None)
    }
}

impl StyledText for egui::text::LayoutJob {
    fn into_styled(self) -> (egui::WidgetText, Option<(Type, Option<Color32>, String)>) {
        (egui::WidgetText::LayoutJob(std::sync::Arc::new(self)), None)
    }
}

/// Text in a role using logical character order without RTL reordering (internal helper for
/// pre-wrapped rows and raw glyphs).
pub fn text_logical(t: Type, s: impl Into<String>) -> egui::RichText {
    let rt = egui::RichText::new(s).font(font(t)).color(color(t));
    match t {
        Type::Title | Type::Heading => rt.strong(),
        _ => rt,
    }
}

/// Text in a role: font, size, colour (and weight for titles and headings). Applies visual RTL
/// reordering for single-line text when the active UI language is right to left while keeping the
/// original logical string for accessibility and multi-line wrapping.
pub fn text(t: Type, s: impl Into<String>) -> Text {
    let logical = s.into();
    let visual = crate::i18n::display(&logical).into_owned();
    Text { rich: text_logical(t, visual), logical, role: t, color_override: None }
}

/// Single-line label that displays visual RTL text while keeping the logical string for AccessKit.
pub fn label(ui: &mut egui::Ui, text: impl StyledText) -> egui::Response {
    use crate::a11y::Named;
    let (wt, meta) = text.into_styled();
    let resp = ui.label(wt);
    if let Some((_, _, logical)) = meta.filter(|(_, _, s)| !s.is_empty() && crate::i18n::is_rtl()) {
        resp.set_name(&logical);
    }
    resp
}

/// Wraps `logical` in logical reading order at the available width, reorders each visual row when
/// RTL is active, and keeps `logical` on the AccessKit label.
pub fn wrapped_label_with_sense(ui: &mut egui::Ui, role: Type, col: Color32, sense: egui::Sense, logical: &str) -> egui::Response {
    if !crate::i18n::is_rtl() {
        return ui.add(egui::Label::new(text_logical(role, logical).color(col)).sense(sense).wrap());
    }
    let font_id = font(role);
    let max_width = ui.available_width().max(1.0);
    let lines = wrap_logical_lines(ui, &font_id, logical, max_width);
    let line_height = ui.fonts_mut(|f| f.row_height(&font_id));
    let galleys: Vec<std::sync::Arc<egui::Galley>> = lines
        .into_iter()
        .map(|line| {
            let visual = crate::i18n::display(&line).into_owned();
            ui.painter().layout_no_wrap(visual, font_id.clone(), col)
        })
        .collect();
    let max_w = galleys.iter().map(|g| g.rect.width()).fold(0.0f32, f32::max).min(max_width);
    let total_h = galleys.iter().map(|g| g.rect.height().max(line_height)).sum::<f32>().max(line_height);
    let alloc_w = max_w.max(1.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(alloc_w, total_h), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), logical));
    let mut y = rect.top();
    for g in galleys {
        let h = g.rect.height().max(line_height);
        let x = (rect.right() - g.rect.width()).max(rect.left());
        ui.painter().galley(egui::pos2(x, y), g, col);
        y += h;
    }
    resp
}

pub fn wrapped_label(ui: &mut egui::Ui, role: Type, logical: &str) -> egui::Response {
    wrapped_label_with_sense(ui, role, color(role), egui::Sense::hover(), logical)
}

fn wrap_logical_lines(ui: &egui::Ui, font_id: &egui::FontId, logical: &str, max_width: f32) -> Vec<String> {
    let measure = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), Color32::WHITE).rect.width());
    let space_w = measure(" ");
    let mut lines = Vec::new();
    for para in logical.split('\n') {
        if para.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut cur_w = 0.0f32;
        for word in para.split_whitespace() {
            let word_w = measure(word);
            if !current.is_empty() && cur_w + space_w + word_w <= max_width {
                current.push(' ');
                current.push_str(word);
                cur_w += space_w + word_w;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                cur_w = 0.0;
            }
            if word_w <= max_width {
                current.push_str(word);
                cur_w = word_w;
            } else {
                // Split oversized words/filenames at character boundaries, always consuming >= 1 char.
                let mut rest = word;
                while !rest.is_empty() {
                    let ends: Vec<usize> = rest.char_indices().map(|(idx, ch)| idx + ch.len_utf8()).collect();
                    let mut lo = 1usize;
                    let mut hi = ends.len();
                    while lo < hi {
                        let mid = (lo + hi).div_ceil(2);
                        if measure(&rest[..ends[mid - 1]]) <= max_width {
                            lo = mid;
                        } else {
                            hi = mid - 1;
                        }
                    }
                    let (chunk, tail) = rest.split_at(ends[lo - 1]);
                    rest = tail;
                    if rest.is_empty() {
                        cur_w = measure(chunk);
                        current.push_str(chunk);
                    } else {
                        lines.push(chunk.to_owned());
                    }
                }
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Buttons have no resting border, but a checkbox or radio button without one would vanish
/// when off. These give selection controls their own visible edge.
fn with_control_edges<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = Stroke::new(1.0, T.control_edge);
        w.hovered.bg_stroke = Stroke::new(1.0, T.text_dim);
        add(ui)
    })
    .inner
}

pub fn checkbox(ui: &mut egui::Ui, on: &mut bool, label: impl StyledText) -> egui::Response {
    use crate::a11y::Named;
    let (wt, meta) = label.into_styled();
    let resp = with_control_edges(ui, |ui| ui.checkbox(on, wt));
    if let Some((_, _, name)) = meta.filter(|(_, _, s)| !s.is_empty() && crate::i18n::is_rtl()) {
        resp.set_name(&name);
    }
    resp
}

pub fn radio_value<V: PartialEq>(ui: &mut egui::Ui, current: &mut V, value: V, label: impl StyledText) -> egui::Response {
    use crate::a11y::Named;
    let (wt, meta) = label.into_styled();
    let resp = with_control_edges(ui, |ui| ui.radio_value(current, value, wt));
    if let Some((_, _, name)) = meta.filter(|(_, _, s)| !s.is_empty() && crate::i18n::is_rtl()) {
        resp.set_name(&name);
    }
    resp
}

/// The narrowest a field or list shrinks to in a narrow panel (ADR 0087).
pub const MIN_FIELD: f32 = 56.0;

/// A field's or list's width: `w`, or the room left in the row when that is less (never under
/// `MIN_FIELD`), so a narrow panel shrinks it rather than cutting it off.
pub fn fit(ui: &egui::Ui, w: f32) -> f32 {
    // In a wrapping row `available_width` is a whole row; the room left on this one is less.
    w.min(ui.available_size_before_wrap().x).max(MIN_FIELD)
}

/// Label column widths for forms: dialogs (Preferences) and the narrow dock.
pub const FORM_LABEL_DIALOG: f32 = 150.0;
pub const FORM_LABEL_DOCK: f32 = 78.0;

/// A form row on a two-column grid: the label left-aligned in a fixed column, the controls after
/// it. Every row's controls start at the same x, so a form reads as one column of values.
///
/// In a narrow panel (ADR 0087) a row whose controls don't fit beside the label puts them under
/// it instead, wrapping if they still don't fit, so nothing is drawn past the panel's edge. The
/// controls' width beside the label is remembered per row, so the choice holds steady while the
/// panel is resized; a change of choice redraws the frame at once.
pub fn form_row<R>(ui: &mut egui::Ui, label_w: f32, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let id = ui.scope_id().with(("form_row", label));
    let avail = ui.available_width();
    let gap = ui.spacing().item_spacing.x;
    let need: Option<f32> = ui.data(|d| d.get_temp(id));
    if need.is_some_and(|n| label_w + gap + n > avail + 0.5) {
        wrapped_label(ui, Type::Label, label);
        return ui.horizontal_wrapped(add).inner;
    }
    let rtl = crate::i18n::is_rtl();
    let (r, used) = horizontal_row(ui, |ui| {
        ui.allocate_ui_with_layout(egui::vec2(label_w, TARGET), row_layout(), |ui| {
            ui.set_min_size(egui::vec2(label_w, TARGET));
            ui.set_max_width(label_w);
            wrapped_label(ui, Type::Label, label);
        });
        let start = if rtl { ui.cursor().right() } else { ui.cursor().left() };
        let r = add(ui);
        let span = if rtl { start - ui.min_rect().left() } else { ui.min_rect().right() - start };
        (r, span)
    })
    .inner;
    if need.is_none_or(|n| (n - used).abs() > 0.5) {
        ui.data_mut(|d| d.insert_temp(id, used));
        if label_w + gap + used > avail + 0.5 {
            ui.ctx().request_discard("a form row no longer fits beside its label"); // i18n-exempt: a debug reason, never shown
        }
    }
    r
}

/// A note that belongs to the controls above it (an explanation, a preview): aligned with the
/// control column, wrapping within it, never back at the label column.
pub fn form_note(ui: &mut egui::Ui, label_w: f32, note: impl StyledText) {
    let (wt, meta) = note.into_styled();
    horizontal_row(ui, |ui| {
        // A row puts one item spacing between its label column and the controls.
        ui.add_space(label_w + ui.spacing().item_spacing.x);
        match meta {
            Some((role, color_override, logical)) => {
                let effective_role = if matches!(wt, egui::WidgetText::Text(_)) { Type::Caption } else { role };
                let col = color_override.unwrap_or_else(|| color(effective_role));
                if effective_role == Type::Path {
                    // Paths keep literal spacing and logical order even when RTL prose wrapping is active.
                    ui.add(egui::Label::new(text_logical(effective_role, logical).color(col)).wrap());
                } else {
                    wrapped_label_with_sense(ui, effective_role, col, egui::Sense::hover(), &logical);
                }
            }
            None => {
                ui.add(egui::Label::new(wt).wrap());
            }
        }
    });
}

/// How a collapsible section header looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// A full-width band with layout borders and an accent edge: top-level dock sections.
    Band,
    /// Title followed by a horizontal rule across the row: groups inside a panel or dialog.
    Rule,
}

/// A collapsible section: a header the whole width of the row (one big click target, keyboard
/// focusable, announced as expanded/collapsed), then `add` while open. Open state is kept per
/// `id` for the session. `detail` is an optional data value shown after the title in the data
/// face (a frame number, a count).
pub fn section<R>(ui: &mut egui::Ui, id: &str, style: Section, title: &str, detail: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    section_with_header(ui, id, style, title, detail, add).1
}

/// [`section`], also returning its header (a panel's keyboard anchor, `a11y::anchor`).
pub fn section_with_header<R>(
    ui: &mut egui::Ui,
    id: &str,
    style: Section,
    title: &str,
    detail: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (egui::Response, Option<R>) {
    use egui::{Sense, StrokeKind, pos2, vec2};
    let key = ui.make_persistent_id(("section", id));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(key, true));
    let height = if style == Section::Band { 32.0 } else { TARGET };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(key, open));
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::DisclosureTriangle, enabled, open, title));
    let rtl = crate::i18n::is_rtl();
    let p = ui.painter();
    let hot = resp.hovered() || resp.has_focus();
    match style {
        Section::Band => {
            p.rect_filled(rect, SQUARE, if hot { T.header } else { T.panel_alt });
            p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, T.border));
            p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, T.border));
            let bar_x = if rtl { rect.right() - 3.0 } else { rect.left() };
            accent_bar(p, egui::Rect::from_min_size(pos2(bar_x, rect.top() + 1.0), vec2(3.0, rect.height() - 1.0)));
        }
        Section::Rule => {
            if hot {
                p.rect_filled(rect, T.rc(), T.panel_alt);
            }
        }
    }
    if resp.has_focus() {
        p.rect_stroke(rect.shrink(1.0), T.rc(), Stroke::new(2.0, T.accent), StrokeKind::Inside);
    }
    // Chevron: right/left when closed, down when open.
    let pad = if style == Section::Band { 10.0 } else { 2.0 };
    let chevron_cx = if rtl { rect.right() - pad - 7.0 } else { rect.left() + pad + 7.0 };
    let chevron = egui::Rect::from_center_size(pos2(chevron_cx, rect.center().y), vec2(14.0, 14.0));
    let icon = if open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight };
    crate::icons::paint(p, chevron, icon, if hot { T.text } else { T.text_dim });
    let visual_title = crate::i18n::display(title).into_owned();
    let mut job = egui::text::LayoutJob::simple_singleline(visual_title, font(Type::Heading), color(Type::Heading));
    if rtl {
        let mut x = rect.right() - pad - 20.0;
        job.wrap = egui::text::TextWrapping::truncate_at_width((x - rect.left() - 4.0).max(0.0));
        let title_g = p.layout_job(job);
        let w = title_g.rect.width();
        p.galley(pos2(x - w, rect.center().y - title_g.rect.height() * 0.5), title_g, color(Type::Heading));
        x -= w;
        if let Some(d) = detail {
            let g = p.layout_no_wrap(crate::i18n::display(d).into_owned(), font(Type::Data), T.text_dim);
            x -= 8.0;
            let dw = g.rect.width();
            p.galley(pos2(x - dw, rect.center().y - g.rect.height() * 0.5), g, T.text_dim);
            x -= dw;
        }
        if style == Section::Rule && x - 12.0 > rect.left() {
            p.hline(rect.left()..=(x - 12.0), rect.center().y, Stroke::new(1.0, T.border));
        }
    } else {
        let mut x = rect.left() + pad + 20.0;
        // A title longer than the row (a narrow dock, a long translation) ends in "…" at its edge;
        // the full title is the header's accessible name.
        job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - x - 4.0).max(0.0));
        let title_g = p.layout_job(job);
        let w = title_g.rect.width();
        p.galley(pos2(x, rect.center().y - title_g.rect.height() * 0.5), title_g, color(Type::Heading));
        x += w;
        if let Some(d) = detail {
            let g = p.layout_no_wrap(d.to_string(), font(Type::Data), T.text_dim);
            x += 8.0;
            let dw = g.rect.width();
            p.galley(pos2(x, rect.center().y - g.rect.height() * 0.5), g, T.text_dim);
            x += dw;
        }
        if style == Section::Rule && x + 12.0 < rect.right() {
            p.hline((x + 12.0)..=rect.right(), rect.center().y, Stroke::new(1.0, T.border));
        }
    }
    if !open {
        if style == Section::Rule {
            ui.add_space(2.0);
        }
        return (resp, None);
    }
    if style == Section::Rule {
        ui.add_space(2.0);
    }
    let r = add(ui);
    ui.add_space(if style == Section::Rule { 10.0 } else { 0.0 });
    (resp, Some(r))
}

/// Runs `add` with every widget's text in `role` (e.g. numeric fields in the Data role).
pub fn with_type<R>(ui: &mut egui::Ui, role: Type, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(font(role));
        add(ui)
    })
    .inner
}

/// The document colour of an interface colour (the onion skin's theme colours, ADR 0047).
pub fn to_rgba(c: Color32) -> anicraft_engine::doc::Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    anicraft_engine::doc::Rgba::new(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, f32::from(a) / 255.0)
}

/// The heat map of a bone's weights (ADR 0090), the same in every palette because it is drawn
/// over artwork: blue (none) through cyan, green and yellow to red (all of the point).
pub const HEAT: [Color32; 5] = [
    Color32::from_rgb(0x2b, 0x4c, 0xd9),
    Color32::from_rgb(0x1f, 0xb5, 0xd6),
    Color32::from_rgb(0x2f, 0xc2, 0x5a),
    Color32::from_rgb(0xf2, 0xd0, 0x2a),
    Color32::from_rgb(0xe8, 0x3a, 0x2a),
];

/// The colour of weight `w` (0…1) on [`HEAT`], blended between its stops.
pub fn heat(w: f32) -> Color32 {
    let w = if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 };
    let x = w * (HEAT.len() - 1) as f32;
    let i = (x as usize).min(HEAT.len() - 2);
    let t = x - i as f32;
    let (a, b) = (HEAT.get(i).copied().unwrap_or(Color32::BLACK), HEAT.get(i + 1).copied().unwrap_or(Color32::BLACK));
    let mix = |p: u8, q: u8| (f32::from(p) + (f32::from(q) - f32::from(p)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Gives a text field the keyboard with all of `text` selected, so typing replaces what a
/// dialog offers (a name to change, a default name) and the arrow keys still edit it
/// (#196, #322).
pub fn focus_selected(ui: &egui::Ui, field: &egui::Response, text: &str) {
    field.request_focus();
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), field.id);
    }
}

/// Construct an RGB colour derived from artwork rather than a UI theme token.
/// UI control colours must still come from T.
pub(crate) fn artwork_rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn to_color32(c: anicraft_engine::doc::Rgba) -> Color32 {
    let c = c.sanitized();
    Color32::from_rgba_unmultiplied(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        (c.a * 255.0).round() as u8,
    )
}

/// Where a dragged panel tab would dock (ADR 0075): the accent, light enough to see the
/// panels under it.
pub fn dock_drop() -> Color32 {
    T.accent.gamma_multiply(0.28)
}

/// How strongly a button's hit area is laid over the artwork (ADR 0057): enough to see where the
/// pointer finds the button, light enough to see the state under it.
pub const HIT_AREA_ALPHA: f32 = 0.35;

/// The tint and opacity a button's hit area is drawn in (ADR 0057).
pub fn hit_area() -> (anicraft_engine::doc::Rgba, f32) {
    (to_rgba(T.hit_area), HIT_AREA_ALPHA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_text_preserves_logical_label_and_role_for_wrappers() {
        let styled = text(Type::Caption, "שלום עולם").color(T.error).strong();
        assert_eq!(styled.logical(), "שלום עולם");
        let (_, meta) = styled.into_styled();
        assert_eq!(meta, Some((Type::Caption, Some(T.error), "שלום עולם".to_owned())));
    }

    #[test]
    fn wrap_logical_lines_splits_oversized_words_without_overflow() {
        let ctx = egui::Context::default();
        let font_id = font(Type::Path);
        let word = "very_long_filename_without_spaces_1234567890.anicraft";
        let mut lines_normal = Vec::new();
        let mut lines_tiny = Vec::new();
        let mut max_line_w = 0.0f32;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let max_w = 80.0;
            lines_normal = wrap_logical_lines(ui, &font_id, word, max_w);
            max_line_w = lines_normal
                .iter()
                .map(|l| ui.fonts_mut(|f| f.layout_no_wrap(l.clone(), font_id.clone(), Color32::WHITE).rect.width()))
                .fold(0.0f32, f32::max);
            lines_tiny = wrap_logical_lines(ui, &font_id, "אבג", 0.5);
        })
        .textures_delta
        .clear();
        assert!(lines_normal.len() > 1, "oversized word must split: {lines_normal:?}");
        assert_eq!(lines_normal.concat(), word);
        assert!(max_line_w <= 80.0 + 0.01, "line width {max_line_w} exceeded 80.0");
        assert_eq!(lines_tiny, vec!["א", "ב", "ג"], "must consume >= 1 char even when max_width < glyph width");
    }

    #[test]
    fn form_note_preserves_repeated_spaces_for_path() {
        let prev = crate::i18n::code();
        crate::i18n::set_language("he");
        let ctx = egui::Context::default();
        let path = "/home/A  B/anicraft.log";
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            form_note(ui, FORM_LABEL_DIALOG, text(Type::Path, path));
        });
        out.textures_delta.clear();
        crate::i18n::set_language(prev);
        let rendered_texts: Vec<&str> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(ts) => Some(ts.galley.text()),
                _ => None,
            })
            .collect();
        assert!(rendered_texts.iter().any(|t| t.contains("A  B")), "Type::Path in form_note must preserve repeated spaces: {rendered_texts:?}");
    }
}
