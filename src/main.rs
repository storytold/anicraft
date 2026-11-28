//! AniCraft desktop app.
//!
//! ```text
//! anicraft [file.anicraft] [--screenshot out.png] [--renderer wgpu|gl] [--open <panel>]
//!          [--theme dark|light|high-contrast] [--scale <percent>] [--lang <code>|auto|pseudo]
//! anicraft --version
//! ```
//! `--screenshot` starts normally, draws ~120 frames, saves the window as PNG and exits.
//!
//! Built for `wasm32-unknown-unknown` the same program is the web editor (`src/web.rs`,
//! ADR 0036, docs/web.md).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Release builds are Windows GUI programs, so no console window opens beside the app (the
// packaging checks the PE subsystem, ADR 0026). Debug builds keep the console for logs; the
// headless front end is anicraft-cli.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The browser build leaves out what only the desktop has (screenshot runs, the recovery folder,
// window chrome, `--open` panels); that code stays compiled for it but unused (ADR 0036).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

// First, so its `tr!` macros are in scope in every module after it.
#[macro_use]
mod i18n;
mod a11y;
mod actions_panel;
mod aids_dialog;
mod app;
mod audio_keys;
mod autosave;
mod brush_panel;
mod brushtool;
#[cfg(not(target_arch = "wasm32"))]
mod build_flavor;
mod colour;
mod colour_panel;
mod envelope;
mod find_panel;
mod font_fallback;
mod publish_panel;
mod erasertool;
mod bind_tool;
mod warp_tool;
mod bone_tool;
mod camera_tool;
mod context_help;
mod convert_symbol;
mod data_ui;
mod layer_depth;
mod layer_props;
mod desktop_ui;
mod dialogs;
mod doc_settings;
mod dock;
mod dock_float;
mod dock_ui;
mod drop;
mod ease_editor;
mod gpu_stage;
#[cfg(not(target_arch = "wasm32"))]
mod gpu_watch;
mod gradient_tool;
mod icons;
mod keymap;
#[cfg(not(target_arch = "wasm32"))]
mod launch_profile;
mod library;
mod menus;
#[cfg(target_os = "macos")]
mod native_menu;
#[cfg(not(target_arch = "wasm32"))]
mod updates;
mod mixed;
mod motion_editor;
mod movie_explorer;
#[cfg(not(target_arch = "wasm32"))]
mod nightly_session;
mod panels;
mod parenting;
mod motion_path;
mod onion;
mod pathtools;
mod platform;
mod plugin_export_ui;
#[cfg(test)]
mod plugin_fixture_tests;
#[cfg(test)]
mod plugin_ui_tests;
#[cfg(test)]
mod macros_ui_tests;
mod macros_ui;
mod plugins_ui;
mod prefs;
#[cfg(not(target_arch = "wasm32"))]
mod process_identity;
#[cfg(not(target_arch = "wasm32"))]
mod profile_root;
mod recent;
#[cfg(not(target_arch = "wasm32"))]
mod remote;
mod rich_edit;
mod rulers;
mod scale_grid;
mod shape_envelope;
mod shape_dialogs;
mod sound;
mod shape_hints;
mod stage;
mod stage_a11y;
mod swatches_panel;
mod tabs;
mod start;
mod startup;
mod templates;
mod test_movie;
mod video_import;
mod theme;
mod theme_file;
mod timeline;
mod trace_dialog;
#[cfg(target_arch = "wasm32")]
mod web;
mod widthtool;
mod windows;
mod xform;
mod workspaces_window;

#[cfg(not(target_arch = "wasm32"))]
use anicraft_engine::Session;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write as _;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

/// What opening the command line's file said, for the status bar.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq)]
enum Opened {
    /// Nothing to report (no file, or an AniCraft document).
    Quiet,
    /// The import report of an Animate document.
    Imported(String),
    /// It didn't open (the sample is there instead): the file's name and why.
    Failed(String, String),
}

/// The session to start with: the file given on the command line (through `file.open`, so
/// `.fla` / `.xfl` import works the same as from the menu), else the sample. A file that can't
/// be opened says why in the window (#226: a release build has no console, so the reason used
/// to go nowhere and a double-clicked document seemed to open as the sample).
#[cfg(not(target_arch = "wasm32"))]
fn open_session(file: Option<&Path>) -> (Session, Opened) {
    let Some(p) = file else { return (Session::new(anicraft_engine::doc::sample::demo()), Opened::Quiet) };
    let mut s = Session::default();
    match s.execute("file.open", &serde_json::json!({"path": p.display().to_string()})) {
        Ok(v) => match v.get("report").and_then(|r| r.as_str()) {
            Some(r) => {
                startup::line(&format!("imported {}: {r}", p.display()));
                (s, Opened::Imported(r.to_string()))
            }
            None => (s, Opened::Quiet),
        },
        Err(e) => {
            startup::line(&format!("could not open {}: {e}; starting with the sample", p.display()));
            (Session::new(anicraft_engine::doc::sample::demo()), Opened::Failed(crate::recent::display_name(p), e.to_string()))
        }
    }
}

/// `--theme` / `--scale`: the look for this run only (screenshots of high contrast or 200 %),
/// over the saved preferences, without saving them.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Default, PartialEq)]
struct Look {
    theme: Option<theme::Palette>,
    /// `--theme <file>.json`: a theme file of the user's (ADR 0117), its folder the themes
    /// folder for the run.
    theme_file: Option<PathBuf>,
    scale: Option<f32>,
    /// `--lang`: a locale code, `auto` or `pseudo` (ADR 0040).
    lang: Option<String>,
    /// `--shot-width`: the screenshot scaled down to this many pixels across (the user guide's
    /// pictures, ADR 0069).
    shot_width: Option<u32>,
}

/// This run's Nightly session record, shared with the window's exit hook, and what the first
/// window says about it. `session` is read by the exit hook, which only macOS has.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone)]
struct Nightly {
    session: nightly_session::Shared,
    /// The record of a session that did not finish (shown once, then cleared).
    previous_unclean: bool,
    /// The record could not be taken: a secondary window, or a profile that refused it.
    health_warning: bool,
    /// Set once a window showed the interruption, so the record is not cleared before it is.
    notice_shown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
fn run(attempt: startup::Attempt, file: Option<&Path>, shot: Option<PathBuf>, open: Option<String>, look: Look, control: Option<u16>, nightly: Nightly) -> eframe::Result {
    let options = attempt.options(eframe::NativeOptions {
        viewport: platform::viewport(egui::ViewportBuilder::default().with_title(platform::name()).with_inner_size([1600.0, 940.0]).with_min_inner_size([960.0, 600.0])),
        ..Default::default()
    });
    // The embedded interface font joins the text library before anything lays text out:
    // opening an Animate document from the command line lays its text out right here.
    anicraft_engine::text::Fonts::register(theme::embedded_fonts());
    // `--lang` before the window exists, so its first status line is in that language too.
    i18n::force(look.lang.as_deref());
    let (session, report) = open_session(file);
    let file_given = file.map(Path::to_path_buf);
    eframe::run_native(
        platform::name(),
        options,
        Box::new(move |cc| {
            // A document the system hands over while AniCraft is idle wakes it (ADR 0062).
            platform::wake_on_requests(&cc.egui_ctx);
            startup::log_adapter(cc.wgpu_render_state.as_ref());
            let starting_fresh = file_given.is_none() && shot.is_none();
            let mut a = app::App::new(cc, session, shot);
            // The previous Nightly session did not finish, or this window cannot record its own:
            // said once, in the status bar, as the recovery notes are.
            if nightly.previous_unclean {
                a.status_err(tr!("The previous AniCraft Nightly session did not finish normally. The start-up log in Preferences › About says what it did last; no report has been sent."));
                // The status bar holds one line, and a document's own report below replaces this one
                // before the first frame: only a start with nothing to add counts as having said it.
                if matches!(report, Opened::Quiet) {
                    nightly.notice_shown.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else if nightly.health_warning {
                a.status_err(tr!(
                    "AniCraft Nightly could not record its session health in its profile. You can still edit, but an abnormal exit is not noticed."
                ));
            }
            // macOS has no code path after `run_native`: a clean quit finishes here.
            #[cfg(target_os = "macos")]
            {
                let shared = nightly.session.clone();
                let shown = nightly.notice_shown.clone();
                a.at_exit = Some(Box::new(move || nightly_session::finish_once(&shared, shown.load(std::sync::atomic::Ordering::Relaxed))));
            }
            // A file named on the command line keeps its tab, even an imported one (ADR 0112).
            if file_given.is_some() {
                a.tabs.keep_first();
            }
            if let Some(s) = a.shot.as_mut() {
                s.width = look.shot_width;
            }
            if a.shot.is_none() && platform::stable_services_allowed() {
                a.show_update_outcome_on_start();
            }
            // No document to open: the start screen, unless it was switched off (ADR 0059).
            if starting_fresh && a.prefs.start_screen {
                start::open(&mut a);
            }
            a.preview_look(&cc.egui_ctx, look.theme, look.theme_file.as_deref(), look.scale);
            // `--control`: an assistant can edit this window live (ADR 0104).
            if let Some(port) = control {
                remote::start(&mut a, &cc.egui_ctx, port);
            }
            a.open_panel(open.as_deref());
            match report {
                // An imported document says what came across, as opening it from the menu does.
                Opened::Imported(r) => a.status_ok(tr!("{report} Save to keep it as an AniCraft document.", report = r)),
                Opened::Failed(name, e) => a.status_err(tr!("Could not open {name}: {e}", name = name, e = e)),
                Opened::Quiet => {}
            }
            Ok(Box::new(a))
        }),
    )
}

/// In the browser the page's script calls this once the module is loaded: the editor starts on
/// the page's canvas.
#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

/// What the command line asks for. A plain argument is the document to open: how Windows
/// (`"%1"` in the MSI's file association) and Linux (`Exec=anicraft %f` in the desktop entry)
/// hand over a double-clicked document. macOS sends it as an Apple Event instead (ADR 0062).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Default, PartialEq)]
struct Args {
    shot: Option<PathBuf>,
    file: Option<PathBuf>,
    open: Option<String>,
    renderer: Option<String>,
    look: Look,
    /// `--control <port>`: the live control port (ADR 0104).
    control: Option<u16>,
    /// `--version` / `--help`: print and leave.
    print: Option<Print>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq)]
enum Print {
    Version,
    Help,
    /// `--check-theme <file>`: whether a theme file would be used, and why not (ADR 0117).
    CheckTheme(PathBuf),
}

/// Reads the arguments (without the program name). `Err` is the message for a bad one.
#[cfg(not(target_arch = "wasm32"))]
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => out.shot = Some(PathBuf::from(args.next().ok_or("--screenshot needs a file name")?)), // i18n-exempt: a console message
            // i18n-exempt: a console message
            "--open" => out.open = Some(args.next().filter(|value| !value.starts_with('-')).ok_or("--open needs a panel name")?),
            // i18n-exempt: a console message
            "--control" => out.control = Some(anicraft_cli::control::parse_port(&args.next().unwrap_or_default()).map_err(|e| format!("--control: {e}"))?),
            // i18n-exempt: a console message
            "--renderer" => out.renderer = Some(args.next().filter(|value| !value.starts_with('-')).ok_or("--renderer needs a renderer name")?),
            "--shot-width" => match args.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(w) if (64..=8192).contains(&w) => out.look.shot_width = Some(w),
                _ => return Err("--shot-width takes a width in pixels, from 64 to 8192".into()), // i18n-exempt: a console message
            },
            "--theme" => {
                let (palette, file) = match args.next().as_deref() {
                    Some("graphite" | "dark") => (Some(theme::Palette::Graphite), None),
                    Some("light") => (Some(theme::Palette::Light), None),
                    Some("high-contrast") => (Some(theme::Palette::HighContrast), None),
                    Some(file) if file.to_ascii_lowercase().ends_with(".json") => (None, Some(PathBuf::from(file))),
                    other => return Err(format!("--theme takes dark (graphite), light, high-contrast or a theme file (.json), not {}", other.unwrap_or("nothing"))), // i18n-exempt: a console message
                };
                out.look.theme = palette;
                out.look.theme_file = file;
            }
            "--scale" => match args.next().and_then(|s| s.trim_end_matches('%').parse::<f32>().ok()) {
                Some(pct) if pct.is_finite() => out.look.scale = Some(pct / 100.0),
                _ => return Err("--scale takes a percentage, such as 150".into()), // i18n-exempt: a console message
            },
            "--lang" => match args.next() {
                Some(code) if i18n::known(&code) => out.look.lang = Some(code),
                other => {
                    let offered: Vec<&str> = i18n::offered().into_iter().map(|(c, _)| c).collect();
                    return Err(format!("--lang takes auto, pseudo or a language this build has ({}), not {}", offered.join(", "), other.as_deref().unwrap_or("nothing"))); // i18n-exempt: a console message
                }
            },
            // Printed by the release packaging checks and for bug reports.
            "--version" | "-V" => out.print = Some(Print::Version),
            "--help" | "-h" => out.print = Some(Print::Help),
            "--check-theme" => out.print = Some(Print::CheckTheme(PathBuf::from(args.next().ok_or("--check-theme needs a theme file")?))), // i18n-exempt: a console message
            // End of the options: everything after it is a file, even a name that reads like one
            // of the flags above. A Nightly start arrives that way from the Launcher
            // (`launch_profile::parse`), so its documents are never taken for flags.
            "--" => {
                for rest in args.by_ref() {
                    out.file = Some(PathBuf::from(rest));
                }
            }
            // Not a file: what older macOS added when Finder started an app.
            other if platform::is_system_argument(other) => {}
            other => out.file = Some(PathBuf::from(other)),
        }
    }
    Ok(out)
}

/// `--help` (the console's text, in English).
#[cfg(not(target_arch = "wasm32"))]
// i18n-exempt: the --help console text
const HELP: &str = "anicraft [file.anicraft|file.fla|file.xfl] [--version] [--screenshot out.png [--shot-width <px>]] [--renderer gl|wgpu|vulkan|dx12|metal] [--theme dark|light|high-contrast|<file>.json] [--check-theme <file>.json] [--scale <percent>] [--lang <code>|auto|pseudo] [--control <port>] [--open palette|shortcuts|helpbone|lastscene|start|drop|droplibrary|select|selectzoom|about|files|confirm|transform|shapecommands|expandfill|softenfill|tracebitmap|docsettings|origins|pivot|layeropacity|points|merge|rawparts|rawedit|rawsub|rawpoints|rawsnap|rulers|editguides|editgrid|symboledit|groups|combine|nineslice|nineslicedit|brush|brushing|polystar|paintinside|drawingedit|shapeenvelope|brushlibrary|brushcurve|paintbrush|eraser|erasing|erasemerge|tiltbrush|eyedropper|pen|frames|span|bucket|tlzoom|tlwide|motion|motionspan|motionpath|motionpoints|motionedit|motionpins|motionpinsspan|hints|hintsend|camera|cameraview|bones|springbones|shapebones|shapeweights|mixedbones|layerdepth|layerdepthview|filters|strokes|prefs|licenses|zoom|recovery|plugins|actions|script|scriptproblems|testmovie|testmovieinside|testscript|textfields|testtext|htmltext|embedfield|richtext|softmask|pathtext|pathedit|button|testhits|exporthtml|publish|publishreport|publishanim|sound|envelope|ease|easepreset|plugineases|pluginexport|pluginexportwindow|library|librarymenu|libraryfolders|colour|layerprops|frameview|layercolour|timelinepolish|layerselect|dock|dockicons|floattabs|docknarrow|workspaces|animator|designer|gradient|gradientbucket|swatches|swatchpanel|video|videoimport|datachart|datatable]\n\
     --renderer   the window's graphics (also ANICRAFT_RENDERER); default automatic, falling back to OpenGL\n\
     --theme      a built-in look, or a theme file of your own, for this run only\n\
     --check-theme  says whether a theme file would be used (exit 0) or why it is refused (exit 1)\n\
     --open       shows a panel at start (handy with --screenshot for visual checks)\n\
     --shot-width scales the screenshot down to this width (the user guide's pictures)\n\
     ANICRAFT_STAGE_RENDERER=gpu|cpu  stage renderer (default gpu, falling back to cpu; ADR 0032)\n\
     --theme      the interface palette for this run (not saved)\n\
     --scale      the interface scale for this run, 75 to 200 (not saved)\n\
     --lang       the interface language for this run (not saved); pseudo tests the layout\n\
     --control    lets an assistant edit this window live through a port on this computer\n\
                  (also ANICRAFT_CONTROL_PORT; 9878 by convention; see docs/control-protocol.md)\n\
     Headless use: anicraft-cli --help";

/// The version `--version`, About and the Launcher's check print.
#[cfg(not(target_arch = "wasm32"))]
fn version_line() -> String {
    let built = env!("ANICRAFT_BUILT_VERSION");
    match build_flavor::COMPILED {
        build_flavor::BuildFlavor::NightlyV1 => build_flavor::version(build_flavor::BuildFlavor::NightlyV1, built),
        build_flavor::BuildFlavor::Stable => format!("{built} ({})", app::RELEASE_STAGE),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn initialize_nightly_process_identity() -> Result<(), process_identity::IdentityError> {
    process_identity::initialize(build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1, platform::APP_ID_NIGHTLY)
}

/// Whether a refusal may also open a native message box. Standard error always carries the reason;
/// this only adds a box where one renders without the app's event loop. Never on macOS: rfd's alert
/// there needs the AppKit run loop this start has not reached, so a refused nightly launch hung in
/// an invisible modal instead of exiting (GridCraft Mac smoke, 2026-11-26).
#[cfg(not(target_arch = "wasm32"))]
fn refusal_dialog(flavor: build_flavor::BuildFlavor, desktop_session: bool, macos: bool) -> bool {
    build_flavor::needs_native_error(flavor, desktop_session) && !macos
}

/// Says why the launch profile was refused, on the console and (nightly, with a desktop whose
/// modal renders on its own: Windows, X11/Wayland) in a dialog: a windowed build has no console to
/// show it on. macOS gets the console line only (`refusal_dialog`).
#[cfg(not(target_arch = "wasm32"))]
fn report_launch_profile_error(error: &str) {
    eprintln!("AniCraft launch profile: {error}");
    let desktop_session = cfg!(windows)
        || cfg!(target_os = "macos")
        || (cfg!(target_os = "linux") && ["DISPLAY", "WAYLAND_DISPLAY"].into_iter().any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty())));
    if refusal_dialog(build_flavor::COMPILED, desktop_session, cfg!(target_os = "macos")) {
        if let Err(error) = initialize_nightly_process_identity() {
            eprintln!("AniCraft launch profile: {error}");
            return;
        }
        let _ = rfd::MessageDialog::new()
            .set_title(tr!("AniCraft Nightly"))
            .set_description(error)
            .set_level(rfd::MessageLevel::Error)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

/// Selects the launch profile before any startup effect: no log line, no folder, no window, no
/// update check. Returns the arguments the rest of the start reads, or `None` once `--version`
/// has been answered (the only answer that comes before the profile is validated).
#[cfg(not(target_arch = "wasm32"))]
fn prepare_launch() -> Option<Vec<std::ffi::OsString>> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // Plain `--version`, or the Launcher's `--launch-profile nightly-v1 --profile-root <dir>
    // --version`: answered before root validation, process identity, IPC or any window.
    if build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1 && (build_flavor::version_query(&argv) || launch_profile::version_query(&argv))
    {
        let _ = writeln!(std::io::stdout(), "AniCraft {}", version_line()); // i18n-exempt: console text
        return None;
    }
    // Only a nightly build reads the wrapper's root, so a stable copy never sees the variable.
    let wrapper_root = if build_flavor::COMPILED == build_flavor::BuildFlavor::NightlyV1 { std::env::var_os(launch_profile::ROOT_ENV) } else { None };
    let early = launch_profile::parse_startup(argv, wrapper_root)
        .and_then(|launch| {
            build_flavor::require_profile(build_flavor::COMPILED, &launch.profile)?;
            initialize_nightly_process_identity().map_err(|error| error.to_string())?;
            Ok(launch)
        })
        .and_then(|launch| {
            launch_profile::prepare(launch, |root| {
                launch_profile::validate_root(root, |key| std::env::var_os(key), std::env::current_exe().ok().as_deref())
            })
        })
        .unwrap_or_else(|error| {
            report_launch_profile_error(&error);
            std::process::exit(2)
        });
    let identity = if early.profile.is_nightly() { platform::LaunchIdentity::NightlyV1 } else { platform::LaunchIdentity::Stable };
    if let Err(error) = launch_profile::install(early.profile).and_then(|()| platform::select_identity(identity)) {
        report_launch_profile_error(&error);
        std::process::exit(2);
    }
    Some(early.args)
}

/// `--control <port>` or `ANICRAFT_CONTROL_PORT`; a bad port stops the start, before a window.
#[cfg(not(target_arch = "wasm32"))]
fn control_port(flag: Option<u16>) -> Option<u16> {
    match flag {
        Some(p) => Some(p),
        None => match std::env::var(anicraft_cli::control::PORT_ENV).ok().filter(|v| !v.trim().is_empty()) {
            Some(v) => match anicraft_cli::control::parse_port(&v) {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("{}: {e}", anicraft_cli::control::PORT_ENV);
                    std::process::exit(2);
                }
            },
            None => None,
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let Some(arguments) = prepare_launch() else { return Ok(()) };
    // Nightly has no updater: the Launcher owns its version (`docs/nightly-release-preparation.md`).
    if platform::stable_services_allowed()
        && let Some(result) = updates::finish_helper()
    {
        let failed = result.is_err();
        if let Err(error) = result {
            eprintln!("Update failed: {error}");
        }
        std::process::exit(i32::from(failed));
    }
    // A file name that is not Unicode cannot be opened; it is said rather than dropped silently
    // (the launcher's arguments themselves were read as bytes in `prepare_launch`).
    let mut words = Vec::with_capacity(arguments.len());
    let mut after_delimiter = false;
    for argument in arguments {
        match argument.into_string() {
            Ok(word) => {
                after_delimiter |= word == "--";
                words.push(word);
            }
            // After `--` every word is a file: one that is not Unicode is said and skipped.
            Err(what) if after_delimiter => eprintln!("AniCraft could not read a file name on the command line: {what:?}"), // i18n-exempt: a console message
            // Before it the word could be an option or its value: dropping it would hand the next
            // word to the wrong option (a file name read as `--screenshot`'s output, say).
            Err(what) => {
                eprintln!("AniCraft cannot read an option or its value that is not Unicode: {what:?}"); // i18n-exempt: a console message
                std::process::exit(2);
            }
        }
    }
    let args = match parse_args(words) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    match args.print {
        Some(Print::Version) => {
            println!("AniCraft {}", version_line());
            return Ok(());
        }
        Some(Print::Help) => {
            println!("{HELP}");
            return Ok(());
        }
        Some(Print::CheckTheme(file)) => {
            // The same checks Preferences makes, without a window: 0 when the theme would be
            // used, 1 with the reason when it is refused.
            match theme_file::read(&file) {
                Ok(t) => {
                    println!("{}: a theme AniCraft can use (base: {})", file.display(), theme_file::base_name(t.base)); // i18n-exempt: a console message
                    for n in &t.notes {
                        println!("  low contrast, not refused: {} is {:.2}:1 (3:1 or more reads better)", n.pair, n.ratio); // i18n-exempt: a console message
                    }
                    return Ok(());
                }
                Err(why) => {
                    eprintln!("{}: refused. {}", file.display(), why.message()); // i18n-exempt: a console message
                    std::process::exit(1);
                }
            }
        }
        None => {}
    }
    let Args { shot, file, open, renderer, look, control, .. } = args;
    // The control port is a stable service: Nightly never opens one, nor reads a port left in the
    // environment by a launch that forwarded its variables.
    let control = if platform::stable_services_allowed() { control_port(control) } else { None };
    // macOS hands documents over as Apple Events, the one that launched the app included: listen
    // from before the event loop starts until it ends (ADR 0062).
    let _documents = platform::listen_for_documents();

    // The window: the graphics asked for, then this platform's order down to OpenGL. A way that
    // fails or panics before the window is up falls through to the next; a start that died
    // inside a driver last time tries that way last (#207, `startup`).
    let mut last = startup::begin(platform::log_dir());
    // Nightly's own health record (`src/nightly_session.rs`): no document names or diagnostics,
    // just whether the last session finished. Stable passes no root and touches nothing.
    let (nightly_record, health_warning) = match nightly_session::Session::begin_with_health(
        launch_profile::current().root(),
        env!("ANICRAFT_BUILT_VERSION"),
        Some(env!("ANICRAFT_BUILD_SHA")),
    ) {
        Ok(state) => state,
        Err(error) => {
            startup::line(&format!("Nightly session health is unavailable: {error}"));
            (None, true)
        }
    };
    let previous_unclean = nightly_record.as_ref().is_some_and(nightly_session::Session::previous_unclean);
    let notice_shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let nightly_session: nightly_session::Shared = std::sync::Arc::new(std::sync::Mutex::new(nightly_record));
    let nightly = Nightly { session: nightly_session.clone(), previous_unclean, health_warning, notice_shown: notice_shown.clone() };
    let mut file = file;
    let mut reopened = 0;
    let mut why = String::from("no attempt"); // i18n-exempt: log text
    // Once, and again each time the window closed to reopen another way.
    let mut opening = true;
    // Set when a window that had drawn then unwound with a panic: the next start reports it.
    let mut ended_in_panic = false;
    // What the command line asks for is tried first, until a session loses its device on it.
    let mut forced = platform::forced_backend(renderer.as_deref());
    let result: eframe::Result = 'attempts: {
        while opening {
            opening = false;
            let attempts = startup::plan(forced, prefs::Prefs::load().window_backend, &platform::window_attempts(platform::OS), &last);
            for (n, attempt) in attempts.iter().enumerate() {
                startup::trying(attempt, n + 1, attempts.len());
                let mut unwound = false;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(*attempt, file.as_deref(), shot.clone(), open.clone(), look.clone(), control, nightly.clone())));
                why = match result {
                    Ok(Ok(())) => {
                        // The window's graphics device was lost mid-session (ADR 0092): it closed
                        // with the work autosaved, to open again another way, which offers it back.
                        if let Some(reopen) = startup::take_reopen()
                            && reopened < MAX_REOPENS
                        {
                            reopened += 1;
                            startup::line(&format!("opening the window again, {} last", attempt.label()));
                            last.retain(|l| *l != attempt.label());
                            last.push(attempt.label());
                            forced = None;
                            file = reopen;
                            opening = true;
                            break;
                        }
                        startup::line("closed");
                        break 'attempts Ok(());
                    }
                Ok(Err(e)) => e.to_string(),
                Err(p) => {
                    unwound = true;
                    format!("{} panicked: {}", attempt.label(), startup::panic_text(p.as_ref())) // i18n-exempt: a log line
                }
            };
            startup::line(&format!("{} failed: {why}", attempt.label()));
            // It ran and then stopped: not a start-up problem, and opening a second window would
            // surprise (autosave offers the work back next time). A graphics failure makes the
            // next starts try this way late (ADR 0092).
            if startup::started() {
                if startup::is_graphics_failure(&why) {
                    startup::lost_mid_session();
                }
                ended_in_panic = unwound;
                break 'attempts Err(eframe::Error::AppCreation(why.into()));
            }
            // It never came up: the next starts try it last, as one that died in a driver.
            startup::failed_to_open();
        }
    }
    startup::give_up(&why);
    Err(eframe::Error::AppCreation(why.into()))
    };
    // A controlled return, even for a handled startup error. A renderer failure before the window
    // (and so before any notice) has not shown an earlier interruption: the record stays, so the
    // retry or the next start still reports it. On macOS a window that was created has already
    // finished in its exit hook, so this is a no-op there.
    // A window that drew and then panicked is not a controlled return: the record stays so the next
    // start reports the interruption (a panic before the first frame was retried or reported above).
    if !ended_in_panic {
        nightly_session::finish_once(&nightly_session, notice_shown.load(std::sync::atomic::Ordering::Relaxed));
    }
    result
}

/// Times one run opens the window again after its graphics device was lost (ADR 0092); past
/// that it closes, and the next start offers the work back.
#[cfg(not(target_arch = "wasm32"))]
const MAX_REOPENS: u32 = 2;

#[cfg(test)]
mod args_tests {
    use super::*;

    fn parse(args: &[&str]) -> Args {
        parse_args(args.iter().map(|a| a.to_string())).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn last_theme_option_wins_across_builtin_and_file_themes() {
        let file = "themes/Plum.JSON";
        let look = parse(&["--theme", file, "--theme", "light"]).look;
        assert_eq!((look.theme, look.theme_file), (Some(theme::Palette::Light), None));
        let look = parse(&["--theme", "dark", "--theme", file]).look;
        assert_eq!((look.theme, look.theme_file), (None, Some(PathBuf::from(file))));
    }

    #[test]
    fn desktop_panel_and_renderer_options_require_values() {
        for (flag, expected) in [("--open", "--open needs a panel name"), ("--renderer", "--renderer needs a renderer name")] {
            for args in [&[flag][..], &[flag, "--help"], &[flag, "--version"]] {
                assert_eq!(parse_args(args.iter().map(|arg| arg.to_string())).unwrap_err(), expected, "{args:?}");
            }
        }
        let a = parse(&["--open", "testmovie", "--renderer", "wgpu", "walk.anicraft"]);
        assert_eq!((a.open.as_deref(), a.renderer.as_deref()), (Some("testmovie"), Some("wgpu")));
        assert_eq!(a.file, Some(PathBuf::from("walk.anicraft")));
    }

    /// Windows (`"%1"`) and Linux (`%f`) hand a double-clicked document over as the one plain
    /// argument, spaces and all; it opens through `file.open`, as File › Open does.
    #[test]
    fn a_double_clicked_document_arrives_on_the_command_line_and_opens() {
        for path in [r"C:\Users\Ana Lima\My Films\walk cycle.anicraft", "/home/ana/Films/old scene.fla", "/home/ana/Films/Scene/DOMDocument.xfl", "relative.anicraft"] {
            assert_eq!(parse(&[path]).file, Some(PathBuf::from(path)), "{path}");
        }
        // The process serial number older macOS added is not a file.
        assert_eq!(parse(&["-psn_0_1234567"]).file, None);
        let a = parse(&["walk.anicraft", "--screenshot", "shots/x.png", "--open", "testmovie"]);
        assert_eq!((a.file, a.shot, a.open.as_deref()), (Some(PathBuf::from("walk.anicraft")), Some(PathBuf::from("shots/x.png")), Some("testmovie")));
        assert!(parse_args(["--scale".to_string(), "lots".to_string()]).is_err());
        assert!(parse_args(["--screenshot".to_string()]).is_err());
        assert_eq!(parse(&["--screenshot", "a.png", "--shot-width", "1200"]).look.shot_width, Some(1200));
        assert!(parse_args(["--shot-width".to_string(), "9".to_string()]).is_err());
        assert_eq!(parse(&["--version"]).print, Some(Print::Version));
        // A theme file of the user's (ADR 0117), for a run or only to check it.
        assert_eq!(parse(&["--theme", "themes/Plum.JSON"]).look.theme_file, Some(PathBuf::from("themes/Plum.JSON")));
        assert_eq!(parse(&["--theme", "light"]).look.theme, Some(theme::Palette::Light));
        assert!(parse_args(["--theme".to_string(), "plum".to_string()]).is_err());
        assert_eq!(parse(&["--check-theme", "plum.json"]).print, Some(Print::CheckTheme(PathBuf::from("plum.json"))));
        assert!(parse_args(["--check-theme".to_string()]).is_err());
        // `--` ends the options: everything after it is a file, even a name that reads like a flag
        // (the Launcher hands a Nightly start its documents over the same delimiter).
        assert_eq!((parse(&["--"]).file, parse(&["--"]).print), (None, None));
        let flag_named_file = parse(&["--", "--version"]);
        assert_eq!((flag_named_file.file, flag_named_file.print), (Some(PathBuf::from("--version")), None));
        assert_eq!(parse(&["--", "a.anicraft"]).file, Some(PathBuf::from("a.anicraft")));
        // Files on both sides of the delimiter open in the order the plain arguments always took:
        // the last one named is the one that is opened.
        assert_eq!(parse(&["--", "a.anicraft", "--other.anicraft"]).file, Some(PathBuf::from("--other.anicraft")));
        assert_eq!(parse(&["--screenshot", "shot.png", "--"]).file, None);
        // `--control <port>` (ADR 0104): a port from 1 to 65535, or an error before any window.
        assert_eq!(parse(&["--control", "9878"]).control, Some(9878));
        for bad in [&["--control"][..], &["--control", "0"], &["--control", "99999"], &["--control", "port"]] {
            assert!(parse_args(bad.iter().map(|a| a.to_string())).is_err(), "{bad:?}");
        }
        // And the file opens: the session starts with it, not the sample.
        let dir = std::env::temp_dir().join(format!("anicraft-shell-args-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("walk cycle.anicraft");
        let mut s = Session::new(anicraft_engine::doc::sample::interactive());
        s.execute("file.save", &serde_json::json!({"path": path.display().to_string()})).unwrap();
        let file = parse(&[&path.display().to_string()]).file;
        let (opened, _) = open_session(file.as_deref());
        assert_eq!(opened.path.as_deref(), Some(path.as_path()));
        assert!(opened.doc == s.doc, "the document on the command line, not the sample");
        // A file that isn't there: the sample, never a crash.
        let (fallback, said) = open_session(Some(&dir.join("missing.anicraft")));
        assert!(fallback.path.is_none());
        // #226: and the window says why (a release build has no console to print it on).
        assert!(matches!(&said, Opened::Failed(name, why) if name == "missing.anicraft" && !why.is_empty()), "{said:?}");
        // An Animate document double-clicked or opened with AniCraft imports, with its report.
        let xfl = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/xfl/tests/corpus/shapes/shapes.xfl");
        let (imported, said) = open_session(Some(&xfl));
        assert!(matches!(said, Opened::Imported(_)), "{said:?}");
        assert!(imported.path.is_none() && !imported.doc.timeline(0).layers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod guide_reference_tests;
#[cfg(test)]
mod guidelines_tests;
#[cfg(test)]
mod dock_narrow_tests;
#[cfg(test)]
mod native_menu_tests;
#[cfg(test)]
mod architecture_tests;
#[cfg(test)]
mod packaging_tests;
#[cfg(test)]
mod rulers_tests;
#[cfg(test)]
mod retime_ui_tests;
#[cfg(test)]
mod layer_ui_tests;
#[cfg(test)]
mod video_import_tests;
#[cfg(test)]
mod parenting_ui_tests;
#[cfg(test)]
mod tilt_ui_tests;
#[cfg(test)]
mod long_stroke_tests;
mod ink;
#[cfg(test)]
mod zoom_tests;
#[cfg(test)]
mod stage_budget_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod refusal_dialog_tests {
    use super::refusal_dialog;
    use crate::build_flavor::BuildFlavor;

    #[test]
    fn a_refusal_never_blocks_on_a_modal_before_the_event_loop_on_macos() {
        assert!(refusal_dialog(BuildFlavor::NightlyV1, true, false), "a desktop refusal still shows the box where one renders");
        assert!(!refusal_dialog(BuildFlavor::NightlyV1, true, true), "macOS has no run loop yet: the modal would hang");
        assert!(!refusal_dialog(BuildFlavor::NightlyV1, false, false), "a headless session has no box");
        assert!(!refusal_dialog(BuildFlavor::Stable, true, false), "only a nightly refusal explains itself in a box");
    }
}
