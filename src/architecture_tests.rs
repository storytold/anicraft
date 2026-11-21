//! The layering rule from AGENTS.md, enforced: a crate depends only on crates in lower layers,
//! nothing below the shell uses the UI toolkit, and the workspace map in AGENTS.md matches the
//! crates that exist.

use std::collections::BTreeMap;
use std::path::Path;

/// Each crate's layer, as AGENTS.md documents it. The shell (`anicraft`, `src/`) is L5.
const LAYERS: &[(&str, u8)] =
    &[("geom", 0), ("xml", 0), ("plugins", 0), ("audio", 0), ("actions", 0), ("guide", 0), ("doc", 1), ("text", 1), ("render", 2), ("xfl", 2), ("svg", 2), ("engine", 3), ("gpu", 3), ("cli", 4)];
const SHELL_LAYER: u8 = 5;
/// UI crates only the shell may use.
const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "egui-wgpu", "egui_glow"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Dependency names in a manifest's `[dependencies]`, `[dev-dependencies]` and
/// `[build-dependencies]` tables, the same tables for one target
/// (`[target.'cfg(windows)'.dependencies]`) and a dependency with a table of its own
/// (`[dependencies.serde]`).
fn dependencies(manifest: &str) -> Vec<String> {
    const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']').trim();
            // The table's own name is what follows the last dot outside quotes: a target's
            // `cfg(...)` is quoted and may hold dots.
            let (mut quote, mut cut) = (None, None);
            for (i, c) in header.char_indices() {
                match (quote, c) {
                    (None, '\'' | '"') => quote = Some(c),
                    (Some(q), c) if c == q => quote = None,
                    (None, '.') => cut = Some(i),
                    _ => {}
                }
            }
            let (before, last) = cut.map_or(("", header), |i| (&header[..i], &header[i + 1..]));
            in_deps = TABLES.contains(&last);
            // `[dependencies.serde]`: the dependency is named in the header itself.
            if !in_deps && TABLES.iter().any(|t| before == *t || before.ends_with(&format!(".{t}"))) {
                out.push(last.trim_matches(['\'', '"']).to_string());
            }
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.split(['=', '.', ' ']).next().filter(|n| !n.is_empty()) {
            out.push(name.to_string());
        }
    }
    out
}

#[test]
fn crates_depend_only_on_lower_layers_and_never_on_the_ui() {
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(root().join("crates")).expect("crates/ exists").flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        seen.push(name.clone());
        let Some(own) = layer_of(&name) else {
            problems.push(format!("crates/{name} has no layer: add it to LAYERS here and to the workspace map in AGENTS.md"));
            continue;
        };
        for dep in dependencies(&text) {
            if UI_CRATES.contains(&dep.as_str()) {
                problems.push(format!("crates/{name} (L{own}) depends on {dep}; only the shell may use the UI toolkit"));
            }
            if let Some(d) = dep.strip_prefix("anicraft-")
                && let Some(l) = layer_of(d)
                && l >= own
            {
                problems.push(format!("crates/{name} (L{own}) depends on anicraft-{d} (L{l}); a crate may use only lower layers"));
            }
        }
    }
    assert!(seen.len() >= LAYERS.len(), "expected every crate in LAYERS to exist, found {seen:?}");
    // The shell may use any crate.
    for dep in dependencies(&std::fs::read_to_string(root().join("Cargo.toml")).unwrap_or_default()) {
        if let Some(l) = dep.strip_prefix("anicraft-").and_then(layer_of) {
            assert!(l < SHELL_LAYER);
        }
    }
    assert!(problems.is_empty(), "layering broken:\n{}", problems.join("\n"));
}

#[test]
fn the_workspace_map_in_agents_md_matches_the_crates() {
    let agents = std::fs::read_to_string(root().join("AGENTS.md")).expect("AGENTS.md");
    let mut documented: BTreeMap<String, u8> = BTreeMap::new();
    for line in agents.lines() {
        let mut words = line.split_whitespace();
        if let (Some(path), Some(layer)) = (words.next(), words.next())
            && let Some(name) = path.strip_prefix("crates/")
            && let Some(l) = layer.strip_prefix('L').and_then(|n| n.parse::<u8>().ok())
        {
            documented.insert(name.to_string(), l);
        }
    }
    let expected: BTreeMap<String, u8> = LAYERS.iter().map(|(n, l)| (n.to_string(), *l)).collect();
    assert_eq!(documented, expected, "AGENTS.md's workspace map and the layers enforced here disagree");
}

#[test]
fn dependency_lines_are_read_from_every_table() {
    let m = "[package]\nname = \"x\"\n\n[dependencies]\nanicraft-geom.workspace = true\nserde = { version = \"1\" }\n# a comment\n\n[dev-dependencies]\npng.workspace = true\n\n[lints]\nworkspace = true\n";
    assert_eq!(dependencies(m), ["anicraft-geom", "serde", "png"]);
    // A dependency for one target, or with a table of its own, counts like any other: a crate
    // can't reach the UI toolkit or a higher layer by naming it there.
    let m = "[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]\negui = \"0.36\"\n\n[target.x86_64-pc-windows-msvc.build-dependencies]\nwinit = \"0.30\"\n\n[dependencies.anicraft-engine]\npath = \"../engine\"\n\n[features]\ndefault = []\n";
    assert_eq!(dependencies(m), ["egui", "winit", "anicraft-engine"]);
}
