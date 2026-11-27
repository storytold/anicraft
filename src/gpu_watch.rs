//! Keeping the window up when the graphics driver fails mid-session (ADR 0092; #207, #255).
//!
//! wgpu treats an error nobody catches as fatal: its default handler panics, eframe unwinds and
//! the window closes, unsaved work and all. Seen under heavy GPU load: `Surface::configure` →
//! "Failed to wait for GPU to come idle before reconfiguring the Surface". So the window's
//! device gets handlers of ours ([`watch`]) that write down what happened instead of panicking:
//! an uncaptured-error handler and a device-lost callback. And the window's surface statuses go
//! through [`surface_action`], which reconfigures a surface that failed and, when that keeps
//! failing, makes a new one.
//!
//! Each frame the app reads the news ([`take_news`], [`tick`]):
//! * **an error** the driver reported: unsaved work is autosaved at once and the status bar says
//!   AniCraft carried on (once a minute at most); the log has the details;
//! * **the device lost**, or the surface failing for [`STUCK_SECS`] on end: nothing can draw on
//!   this device again (eframe can't make a new one), so the work is autosaved one last time,
//!   the startup record ([`crate::startup::lost_mid_session`]) makes the next starts try another
//!   way first. A stuck surface can close normally. A confirmed device loss reopens in a fresh
//!   process, since eframe paints again before processing Close and can panic on the lost device.
//!
//! The GPU stage hears of the device's loss through [`lost`] (a renderer sharing the window's
//! device can't install callbacks of its own: one per device), and drops to the CPU.

use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

struct PaintRecovery {
    file: Option<std::path::PathBuf>,
    shot: Option<std::path::PathBuf>,
    checkpoints: Vec<anicraft_engine::recovery::Checkpoint>,
}

thread_local! {
    /// The current window thread's last complete document state, kept across the call to paint.
    static BEFORE_PAINT: std::cell::RefCell<Option<PaintRecovery>> = const { std::cell::RefCell::new(None) };
}

fn recovery_state(app: &crate::app::App) -> PaintRecovery {
    PaintRecovery {
        file: app.s.path.clone().filter(|_| !app.s.is_dirty()),
        shot: app.shot.as_ref().map(|shot| shot.path.clone()),
        checkpoints: crate::tabs::graphics_checkpoints(app),
    }
}

pub fn prepare_paint(app: &crate::app::App) {
    BEFORE_PAINT.with(|slot| slot.replace(Some(recovery_state(app))));
}

pub fn finished_window() {
    BEFORE_PAINT.with(|slot| slot.take());
}

fn handoff(why: &str, recovery: PaintRecovery) -> ! {
    crate::startup::line(&format!("reopening in a fresh process after graphics device loss ({why})"));
    for checkpoint in recovery.checkpoints {
        if let Err(e) = checkpoint.flush() {
            crate::startup::line(&format!("could not flush a graphics recovery copy: {e}"));
        }
    }
    crate::startup::lost_mid_session();
    let _replacement = match crate::startup::replace_lost_window(recovery.file.as_deref(), recovery.shot.as_deref()) {
        Ok(child) => child,
        Err(e) => {
            crate::startup::give_up(&format!("the graphics device was lost and the recovery window could not open: {e}")); // i18n-exempt: startup diagnostic wrapped by give_up
            std::process::exit(1);
        }
    };
    // Do not unwind through a failed renderer and its surface. The OS releases the process's
    // graphics resources and recovery locks before the replacement's startup pipe closes.
    std::process::exit(0);
}

/// An isolated screenshot fixture can destroy its own test device just before egui paints.
#[cfg(debug_assertions)]
pub fn loss_probe(ctx: &egui::Context, device: &eframe::wgpu::Device) {
    if crate::startup::started() && ctx.data_mut(|data| data.remove_temp(egui::Id::unique("array-gpu-loss-probe"))) == Some(true) {
        record_lost("injected graphics device loss before egui-wgpu paint"); // i18n-exempt: debug fixture log
        device.destroy();
        ctx.request_repaint();
    }
}

/// eframe paints after the app returns. A loss first reported during that paint can trigger
/// a staging-buffer panic before the app sees another frame. Recover before unwinding into HAL.
pub fn recover_painter_panic(info: &std::panic::PanicHookInfo<'_>) {
    let Some(why) = HEALTH.try_lock().ok().and_then(|health| health.as_ref()?.lost.clone()) else { return };
    let file = info.location().map_or("", |location| location.file());
    if !lost_painter_panic(file, &crate::startup::panic_text(info.payload())) { return; }
    if let Some(recovery) = BEFORE_PAINT.try_with(|slot| slot.try_borrow_mut().ok()?.take()).ok().flatten() {
        handoff(&why, recovery);
    }
}

fn lost_painter_panic(file: &str, message: &str) -> bool {
    file.contains("egui-wgpu-") || (file.contains("wgpu-hal-") && file.contains("swapchain"))
        // Packaged builds can remap dependency source paths.
        || message.starts_with("Failed to create staging buffer for ") // i18n-exempt: matching a dependency's English panic
}

/// Errors written to the log in full; after that, one in [`LOG_EVERY`] (a driver can report one
/// a frame).
const LOG_FIRST: u32 = 20;
const LOG_EVERY: u32 = 500;
/// A surface failing for this long without a pause counts as lost.
pub const STUCK_SECS: f32 = 8.0;
/// A pause this long between surface failures ends the streak (the window drew in between).
const STREAK_GAP: Duration = Duration::from_millis(1500);
/// Failed reconfigures in a row before the surface is made anew.
const RECREATE_AFTER: u32 = 3;
/// The status bar says the driver reported a problem at most this often.
const NOTICE_EVERY_SECS: f64 = 60.0;

#[derive(Default)]
struct Health {
    /// Errors the driver reported (uncaptured), and the newest one's text.
    errors: u32,
    last_error: Option<String>,
    /// Errors not yet seen by the app.
    unseen: bool,
    /// The device's loss, with the driver's words.
    lost: Option<String>,
    /// The surface's current run of failures: since when, the latest, and how many reconfigures
    /// in a row failed.
    failing_since: Option<Instant>,
    last_failure: Option<Instant>,
    in_a_row: u32,
    /// The app was told the device is gone (so it acts once).
    handled: bool,
    /// When the app last said so in the status bar (app time).
    noticed_at: Option<f64>,
}

impl Health {
    /// Notes an error the driver reported; how many there have been.
    fn error(&mut self, text: &str) -> u32 {
        self.errors = self.errors.saturating_add(1);
        self.last_error = Some(text.chars().take(2000).collect());
        self.unseen = true;
        self.errors
    }

    /// Notes the device's loss, keeping the first reason; whether this was the first.
    fn lose(&mut self, why: &str) -> bool {
        let first = self.lost.is_none();
        self.lost.get_or_insert_with(|| why.chars().take(2000).collect());
        first
    }

    /// What happened since the app last looked, as of `now`: each error and the loss once.
    fn news(&mut self, now: Instant) -> News {
        let error = if std::mem::take(&mut self.unseen) { self.last_error.clone() } else { None };
        let failing = self.last_failure.is_some_and(|t| now.saturating_duration_since(t) <= STREAK_GAP);
        let stuck = failing && self.failing_since.is_some_and(|t| now.saturating_duration_since(t).as_secs_f32() >= STUCK_SECS);
        let gone_now = self.lost.clone().or_else(|| stuck.then(|| format!("the window's surface kept failing for {STUCK_SECS} s"))); // i18n-exempt: log text
        let gone = if self.handled { None } else { gone_now };
        if gone.is_some() {
            self.handled = true;
        }
        News { error, gone, failing }
    }

    /// Whether the status bar should say so at app time `at` (at most once a minute).
    fn notice_due(&mut self, at: f64) -> bool {
        let due = self.noticed_at.is_none_or(|t| at - t >= NOTICE_EVERY_SECS || at < t);
        if due {
            self.noticed_at = Some(at);
        }
        due
    }
}

/// The window's graphics health: one per process, as there is one window device. Everything
/// that decides anything is a method of [`Health`], so a test works on a `Health` of its own:
/// this one is read by every app frame ([`tick`]), on whichever thread an app is running.
static HEALTH: Mutex<Option<Health>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Health) -> R) -> R {
    let mut g = HEALTH.lock().unwrap_or_else(PoisonError::into_inner);
    f(g.get_or_insert_with(Health::default))
}

/// Notes an error the driver reported (never panics; the log gets the first ones in full).
pub fn record_error(text: &str) {
    let n = with(|h| h.error(text));
    if n <= LOG_FIRST || n.is_multiple_of(LOG_EVERY) {
        crate::startup::line(&format!("graphics error {n} (AniCraft carried on): {}", text.trim()));
    }
}

/// Notes that the device was lost (a driver reset, or the GPU taken away).
pub fn record_lost(why: &str) {
    if with(|h| h.lose(why)) {
        crate::startup::line(&format!("the window's graphics device was lost: {}", why.trim()));
    }
}

/// The device's loss, if it happened (the GPU stage then stops using the window's device).
pub fn lost() -> Option<String> {
    with(|h| h.lost.clone())
}

/// What the window's surface said when a frame couldn't be had (the cases of
/// `wgpu::CurrentSurfaceTexture` other than success).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    /// An error inside the call (it went to the error handler): for one, the surface could not
    /// be configured, and every frame fails until it is.
    Validation,
}

/// What to do about a frame the surface couldn't give, the `in_a_row`-th failed reconfigure
/// in a row: reconfigure it (the next frame tries again), make a new surface when reconfiguring
/// keeps failing or the surface is gone, and skip the frame while the window is hidden or the
/// GPU is busy.
pub fn surface_action(s: Surface, in_a_row: u32) -> eframe::egui_wgpu::SurfaceErrorAction {
    use eframe::egui_wgpu::SurfaceErrorAction as A;
    match s {
        Surface::Occluded | Surface::Timeout => A::SkipFrame,
        Surface::Lost => A::RecreateSurface,
        Surface::Outdated | Surface::Validation if in_a_row >= RECREATE_AFTER => A::RecreateSurface,
        Surface::Outdated | Surface::Validation => A::Reconfigure,
    }
}

/// eframe's surface-status callback (set on every wgpu attempt, [`crate::startup::Attempt::options`]).
pub fn on_surface_status(status: &eframe::wgpu::CurrentSurfaceTexture) -> eframe::egui_wgpu::SurfaceErrorAction {
    if lost().is_some() {
        return eframe::egui_wgpu::SurfaceErrorAction::SkipFrame;
    }
    use eframe::wgpu::CurrentSurfaceTexture as C;
    let s = match status {
        C::Timeout => Surface::Timeout,
        C::Occluded => Surface::Occluded,
        C::Outdated => Surface::Outdated,
        C::Lost => Surface::Lost,
        // Success and Suboptimal never come here (egui draws those frames).
        _ => Surface::Validation,
    };
    let now = Instant::now();
    let in_a_row = with(|h| {
        if s == Surface::Occluded {
            return 0;
        }
        if h.last_failure.is_none_or(|t| now.duration_since(t) > STREAK_GAP) {
            h.failing_since = Some(now);
            h.in_a_row = 0;
        }
        h.last_failure = Some(now);
        if matches!(s, Surface::Validation | Surface::Outdated) {
            h.in_a_row = h.in_a_row.saturating_add(1);
        }
        h.in_a_row
    });
    let action = surface_action(s, in_a_row);
    let recreate = matches!(action, eframe::egui_wgpu::SurfaceErrorAction::RecreateSurface);
    // The first of a run, and each new surface: a failing surface would otherwise log a line a frame.
    if (s == Surface::Validation && in_a_row <= 1) || recreate {
        crate::startup::line(&format!("the window's surface: {s:?}, {} ({in_a_row} in a row)", match action {
            eframe::egui_wgpu::SurfaceErrorAction::Reconfigure => "reconfiguring it", // i18n-exempt: log text
            eframe::egui_wgpu::SurfaceErrorAction::RecreateSurface => "making it anew", // i18n-exempt: log text
            eframe::egui_wgpu::SurfaceErrorAction::SkipFrame => "skipping the frame", // i18n-exempt: log text
        }));
    }
    if in_a_row >= RECREATE_AFTER {
        with(|h| h.in_a_row = 0);
    }
    action
}

/// Installs our handlers on the window's device: errors and the device's loss are written down
/// (and the window woken to act on them) instead of panicking.
pub fn watch(device: &eframe::wgpu::Device, ctx: &egui::Context) {
    with(|health| *health = Health::default());
    let c = ctx.clone();
    device.on_uncaptured_error(std::sync::Arc::new(move |e: eframe::wgpu::Error| {
        if matches!(e, eframe::wgpu::Error::OutOfMemory { .. }) {
            record_lost(&e.to_string());
        } else {
            record_error(&e.to_string());
        }
        c.request_repaint();
    }));
    let c = ctx.clone();
    device.set_device_lost_callback(move |reason, msg| {
        // Dropping the device on purpose (closing the window) isn't a failure.
        if reason != eframe::wgpu::DeviceLostReason::Destroyed {
            record_lost(&msg);
        }
        c.request_repaint();
    });
}

/// What happened since the app last looked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct News {
    /// A new error the driver reported, the newest.
    pub error: Option<String>,
    /// The device is gone (lost, or its surface failing for [`STUCK_SECS`] on end): the window
    /// must close. Said once.
    pub gone: Option<String>,
    /// The surface is failing right now (the app keeps waking to see it through).
    pub failing: bool,
}

/// The news for this frame.
pub fn take_news() -> News {
    let now = Instant::now();
    with(|h| h.news(now))
}

/// Whether the status bar should say so now (at most once a minute).
fn notice_due(at: f64) -> bool {
    with(|h| h.notice_due(at))
}

/// Per frame (desktop): acts on the news. A reported error autosaves at once and says so; a
/// device gone saves the work for the next window, records how it went, and closes this one so
/// it can open again another way ([`crate::startup::ask_reopen`]).
pub fn tick(app: &mut crate::app::App, ctx: &egui::Context) {
    let news = take_news();
    if let Some(why) = lost() {
        handoff(&why, recovery_state(app));
    }
    if news.error.is_some() || news.failing {
        app.recovery.doc.write_now(&app.s);
    }
    if news.failing {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
    if news.error.is_some() && news.gone.is_none() && app.shot.is_none() && notice_due(ctx.input(|i| i.time)) {
        app.status_err(tr!("The graphics driver reported a problem; AniCraft carried on and autosaved your work (the details are in anicraft.log)."));
    }
    let Some(why) = news.gone else { return };
    crate::startup::line(&format!("closing the window to open it again another way ({why})"));
    // The work goes to the recovery folder for the next window (the lock goes, so it finds it).
    crate::tabs::finish_all(app, false);
    crate::startup::lost_mid_session();
    // A saved document opens again as it was; unsaved work is offered back.
    let file = app.s.path.clone().filter(|_| !app.s.is_dirty());
    crate::startup::ask_reopen(file);
    app.allow_close = true;
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// Tests that share the stage renderer's test doubles take turns (`gpu_stage`).
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui_wgpu::SurfaceErrorAction as A;

    #[test]
    fn a_lost_painter_can_be_identified_after_source_path_remapping() {
        assert!(lost_painter_panic("src/renderer.rs", "Failed to create staging buffer for index data."));
        assert!(lost_painter_panic("egui-wgpu-0.36.2/src/renderer.rs", "driver failure"));
        assert!(lost_painter_panic("wgpu-hal-30.0.1/src/vulkan/swapchain/native.rs", "driver failure"));
        assert!(!lost_painter_panic("src/renderer.rs", "an unrelated bug"));
    }

    /// The surface policy: a failed configure (the error of #255) is retried by reconfiguring,
    /// then by a new surface; a hidden window or a busy GPU skips the frame.
    #[test]
    fn a_failing_surface_is_reconfigured_then_made_anew() {
        assert!(matches!(surface_action(Surface::Validation, 1), A::Reconfigure));
        assert!(matches!(surface_action(Surface::Outdated, 2), A::Reconfigure));
        assert!(matches!(surface_action(Surface::Validation, RECREATE_AFTER), A::RecreateSurface));
        assert!(matches!(surface_action(Surface::Lost, 0), A::RecreateSurface));
        assert!(matches!(surface_action(Surface::Occluded, 9), A::SkipFrame));
        assert!(matches!(surface_action(Surface::Timeout, 0), A::SkipFrame));
    }

    /// Errors and the device's loss are written down, never a panic; the app hears of each
    /// once. On a `Health` of its own: the process's is read by every app frame, so a test
    /// running an app on another thread took this test's news between its lines (and could be
    /// told by this test's loss to close its window).
    #[test]
    fn errors_and_the_loss_come_to_the_app_once() {
        let mut h = Health::default();
        let now = Instant::now();
        assert_eq!(h.news(now), News::default());
        assert_eq!(h.error("Failed to wait for GPU to come idle before reconfiguring the Surface"), 1);
        let n = h.news(now);
        assert!(n.error.as_deref().is_some_and(|e| e.contains("come idle")) && n.gone.is_none());
        assert_eq!(h.news(now).error, None, "said once");
        assert_eq!(h.lost, None);
        assert!(h.lose("Device lost: DXGI_ERROR_DEVICE_HUNG"));
        assert!(!h.lose("a second loss keeps the first reason"));
        assert_eq!(h.lost.as_deref(), Some("Device lost: DXGI_ERROR_DEVICE_HUNG"));
        assert!(h.news(now).gone.is_some());
        assert_eq!(h.news(now).gone, None, "the app acts once");
        // The notice comes at most once a minute.
        assert!(h.notice_due(10.0) && !h.notice_due(30.0) && h.notice_due(75.0));
    }

    /// A surface failing without a pause for [`STUCK_SECS`] counts as the device gone; one
    /// that drew again since does not.
    #[test]
    fn a_surface_stuck_failing_counts_as_gone() {
        let start = Instant::now();
        let later = start + Duration::from_secs(9);
        let mut h = Health { failing_since: Some(start), last_failure: Some(later), ..Health::default() };
        let n = h.news(later);
        assert!(n.failing && n.gone.is_some_and(|why| why.contains("kept failing")));
        let mut h = Health { failing_since: Some(start), last_failure: Some(start + Duration::from_secs(1)), ..Health::default() };
        let n = h.news(later);
        assert!(!n.failing && n.gone.is_none(), "it drew again since");
    }

    /// The process's health is read by app frames on any thread while this module's tests
    /// run: what they take from it is no longer anything a test here asserts on.
    #[test]
    fn app_frames_on_other_threads_do_not_disturb_the_tests() {
        let readers: Vec<_> = (0..4).map(|_| std::thread::spawn(|| (0..200).for_each(|_| drop(take_news())))).collect();
        for _ in 0..200 {
            errors_and_the_loss_come_to_the_app_once();
        }
        assert!(readers.into_iter().all(|t| t.join().is_ok()));
    }
}
