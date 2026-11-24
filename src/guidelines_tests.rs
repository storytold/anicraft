//! Enforces docs/ui-guidelines.md: typography and colour come from `theme`, never raw values.

const SOURCES: [(&str, &str); 36] = [
    ("transform3d_tool.rs", include_str!("transform3d_tool.rs")),
    ("rich_edit.rs", include_str!("rich_edit.rs")),
    ("frame_picker_panel.rs", include_str!("frame_picker_panel.rs")),
    ("windows.rs", include_str!("windows.rs")),
    ("ease_editor.rs", include_str!("ease_editor.rs")),
    ("colour.rs", include_str!("colour.rs")),
    ("drop.rs", include_str!("drop.rs")),
    ("recent.rs", include_str!("recent.rs")),
    ("start.rs", include_str!("start.rs")),
    ("actions_panel.rs", include_str!("actions_panel.rs")),
    ("test_movie.rs", include_str!("test_movie.rs")),
    ("autosave.rs", include_str!("autosave.rs")),
    ("icons.rs", include_str!("icons.rs")),
    ("pathtools.rs", include_str!("pathtools.rs")),
    ("platform.rs", include_str!("platform.rs")),
    ("plugins_ui.rs", include_str!("plugins_ui.rs")),
    ("prefs.rs", include_str!("prefs.rs")),
    ("theme_file.rs", include_str!("theme_file.rs")),
    ("a11y.rs", include_str!("a11y.rs")),
    ("app.rs", include_str!("app.rs")),
    ("dialogs.rs", include_str!("dialogs.rs")),
    ("menus.rs", include_str!("menus.rs")),
    ("panels.rs", include_str!("panels.rs")),
    ("stage.rs", include_str!("stage.rs")),
    ("stage_a11y.rs", include_str!("stage_a11y.rs")),
    ("timeline.rs", include_str!("timeline.rs")),
    ("widthtool.rs", include_str!("widthtool.rs")),
    ("motion_editor.rs", include_str!("motion_editor.rs")),
    ("shape_hints.rs", include_str!("shape_hints.rs")),
    ("bone_tool.rs", include_str!("bone_tool.rs")),
    ("find_panel.rs", include_str!("find_panel.rs")),
    ("movie_explorer.rs", include_str!("movie_explorer.rs")),
    ("dock.rs", include_str!("dock.rs")),
    ("dock_ui.rs", include_str!("dock_ui.rs")),
    ("publish_panel.rs", include_str!("publish_panel.rs")),
    ("main.rs", ""),
];

#[test]
fn typography_goes_through_theme_roles() {
    // Raw font families/sizes outside theme.rs bypass the prose/data rules.
    let banned = ["FontId::", ".monospace()", ".size(", "RichText::new("];
    let mut found = Vec::new();
    for (file, src) in SOURCES {
        for (n, line) in src.lines().enumerate() {
            for b in banned {
                if line.contains(b) {
                    found.push(format!("{file}:{}: `{b}` (use theme::text / theme::font)", n + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "UI guideline violations:\n{}", found.join("\n"));
}

#[test]
fn colours_and_shapes_come_from_tokens() {
    // Raw colours and radii outside theme.rs drift from the palette and the square look.
    let banned = ["Color32::from_rgb(", "Color32::from_rgba_unmultiplied(", "CornerRadius::same("];
    let mut found = Vec::new();
    for (file, src) in SOURCES {
        for (n, line) in src.lines().enumerate() {
            for b in banned {
                if line.contains(b) {
                    found.push(format!("{file}:{}: `{b}` (add a token to theme::T)", n + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "UI guideline violations:\n{}", found.join("\n"));
}

use crate::theme::contrast;

/// Every pair of tokens a palette draws together that falls short of the ratio it must reach
/// (`theme::shortfalls`, the rules a user's theme is held to as well, ADR 0117): the built-in
/// palettes meet WCAG AA and the stricter house rules.
fn palette_failures(name: &str, t: &crate::theme::Tokens) -> Vec<String> {
    crate::theme::shortfalls(t, crate::theme::Standard::House).into_iter().map(|s| format!("{name}: {} is {:.2}:1, needs {}:1", s.pair, s.ratio, s.needs)).collect()
}

#[test]
fn palette_meets_contrast_and_target_minimums() {
    use crate::theme::{GRAPHITE, HIGH_CONTRAST, LIGHT, Palette, TARGET, TOOL_BUTTON};
    let mut found = palette_failures("Graphite", &GRAPHITE);
    found.extend(palette_failures("Light", &LIGHT));
    found.extend(palette_failures("High contrast", &HIGH_CONTRAST));
    assert!(found.is_empty(), "contrast below WCAG AA:\n{}", found.join("\n"));
    // Held to plain WCAG AA, as a user's theme is (ADR 0117), they pass with nothing to note.
    for t in [GRAPHITE, LIGHT, HIGH_CONTRAST] {
        assert_eq!(crate::theme::shortfalls(&t, crate::theme::Standard::Aa), [], "a built-in palette below AA");
    }
    // High contrast is stricter than AA: every text step reads at 7:1 (AAA) on its surfaces.
    let hc = HIGH_CONTRAST;
    for bg in [hc.panel, hc.panel_alt, hc.chrome, hc.header, hc.hover, hc.field] {
        for fg in [hc.text, hc.text_dim, hc.text_faint, hc.accent_text, hc.icon] {
            assert!(contrast(fg, bg) >= 7.0, "high contrast text below 7:1: {:.2}", contrast(fg, bg));
        }
    }
    // Light is designed for light, not the dark look inverted (ADR 0077): dark text on light
    // surfaces, fields the lightest of them, the frame (chrome) darker than the panels, and egui
    // told it is a light look. Graphite and high contrast are the other way round.
    for (name, t) in [("Graphite", GRAPHITE), ("High contrast", HIGH_CONTRAST), ("Light", LIGHT)] {
        let lum = |c: egui::Color32| contrast(c, egui::Color32::BLACK);
        assert_eq!(t.dark, lum(t.text) > lum(t.panel), "{name}: `dark` says which way text goes");
        assert_eq!(t.dark, crate::theme::reads_as_dark(&t), "{name}");
    }
    let lum = |c: egui::Color32| contrast(c, egui::Color32::BLACK);
    assert!(lum(LIGHT.field) >= lum(LIGHT.panel_alt) && lum(LIGHT.panel_alt) > lum(LIGHT.panel) && lum(LIGHT.panel) > lum(LIGHT.chrome), "light surfaces step down from fields to the frame");
    const { assert!(LIGHT.control_hairline && !GRAPHITE.control_hairline, "light controls carry a hairline") };
    // Every palette the preferences offer is checked, once each.
    assert!(Palette::ALL.iter().all(|(p, _, _)| matches!(p, Palette::Graphite | Palette::HighContrast | Palette::Light)));
    for p in [Palette::Graphite, Palette::Light, Palette::HighContrast] {
        assert_eq!(Palette::ALL.iter().filter(|(q, _, _)| *q == p).count(), 1, "{p:?} offered once");
    }
    // Click targets (WCAG 2.5.8 asks for at least 24 × 24).
    const { assert!(TARGET >= 24.0 && TOOL_BUTTON >= TARGET) };
}

#[test]
fn data_roles_are_monospace_and_prose_roles_are_not() {
    use crate::theme::{Type, font};
    use egui::FontFamily;
    for t in [Type::Data, Type::DataSmall, Type::DataLarge, Type::Key, Type::Path] {
        assert_eq!(font(t).family, FontFamily::Monospace, "{t:?}");
    }
    for t in [Type::Title, Type::Heading, Type::Body, Type::Label, Type::Caption] {
        assert_eq!(font(t).family, FontFamily::Proportional, "{t:?}");
    }
}

/// Every floating window comes from `crate::windows::new`, which keeps it below the menu bar
/// (a dialog's title bar must never cover File, Edit, View).
#[test]
fn floating_windows_stay_below_the_menu_bar() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    assert!(files.len() > 40);
    let opened = ["egui::Window::new(", concat!("Window", "::new(")];
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if name == "windows.rs" || name.ends_with("tests.rs") {
            continue;
        }
        let src = std::fs::read_to_string(&f).unwrap();
        for o in opened {
            assert!(!src.contains(o), "{name} opens a window with {o}; use crate::windows::new (kept below the menu bar)");
        }
    }
}

/// #483: a scroll bar is there at rest wherever there is more than fits, in a lane of its own
/// beside the content and wide enough to grab; it does not wait for the pointer to come near.
#[test]
fn scroll_bars_are_always_shown_in_their_own_lane() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let style = ctx.global_style().spacing.scroll;
    // Not over the content: a solid bar, or a "floating" one that keeps a lane as wide as the
    // bar; and seen at rest, not only when the pointer comes near.
    assert!(!style.floating || style.floating_allocated_width >= style.bar_width, "scroll bars do not float over the content: {style:?}");
    assert!(!style.floating || style.dormant_handle_opacity > 0.0, "scroll bars show at rest: {style:?}");
    assert!(style.bar_width >= 8.0 && style.handle_min_length >= crate::theme::TARGET, "{style:?}");
    // Ten rows in room for three: the content gives up the bar's lane with no pointer anywhere.
    // (The lane opens over a few frames.)
    let mut widths = (0.0, 0.0);
    for _ in 0..30 {
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), ..Default::default() }, |ui| {
            ui.allocate_ui(egui::vec2(200.0, 60.0), |ui| {
                widths.0 = ui.available_width();
                egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                    widths.1 = ui.available_width();
                    for row in 0..10 {
                        ui.label(format!("row {row}"));
                    }
                });
            });
        });
        out.textures_delta.clear();
    }
    assert!(widths.0 - widths.1 >= crate::theme::SCROLL_BAR_WIDTH, "the bar has its lane at rest: {widths:?}");
}
