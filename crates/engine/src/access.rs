//! Automation roots (ADR 0124): the folders an agent's commands may read from and write to.
//!
//! `anicraft-cli mcp` and the live control port run every command inside [`confine`]. While it
//! is in force on a thread, each file a command touches is resolved ([`resolve`]: absolute,
//! `..` removed, symbolic links and Windows junctions followed) and refused unless it lies in
//! a read root (to read) or the write root (to write). The check sits in three places:
//!
//! * [`check_params`], at the start of `Session::execute`: every path a command is *given*
//!   ([`PATH_PARAMS`]), so a refusal comes before anything runs and names the parameter;
//! * [`crate::vfs`], which all commands read and write through: paths a command *derives*
//!   (the document's own file, publish outputs, links stored in a document, a saved command's
//!   steps);
//! * the few places that hand a path to something else (ffmpeg, the plug-in registry).
//!
//! Without [`confine`] nothing here does anything: the app and the plain CLI are not limited.
//! The in-memory file store (the browser) is not limited either; it has no disk to protect.

use serde_json::Value;
use std::cell::RefCell;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Most read roots one policy holds.
pub const MAX_ROOTS: usize = 64;

/// Most components a checked path may have.
const MAX_COMPONENTS: usize = 256;

/// What a command wants to do with a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Read what a file holds, or list a folder: inside a read root.
    Read,
    /// Create, replace, rename or delete: inside the write root.
    Write,
    /// Ask whether it exists or how large it is: inside either (a command asks before it
    /// replaces a file, so the write root's files must be visible that far).
    Probe,
}

/// The folders automation may use, each resolved once when the policy is made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    read: Vec<PathBuf>,
    write: Option<PathBuf>,
}

/// A path as people write it: without Windows' `\\?\` prefix.
pub fn display(path: &Path) -> String {
    let s = path.display().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

impl Roots {
    /// A policy from the folders given on the command line. Each must be a folder that exists;
    /// it is resolved now, so a link put in its place later doesn't move the root.
    pub fn new(read: &[PathBuf], write: Option<&Path>) -> Result<Roots, String> {
        if read.len() > MAX_ROOTS {
            return Err(format!("at most {MAX_ROOTS} read roots"));
        }
        let root = |p: &Path, flag: &str| -> Result<PathBuf, String> {
            if p.as_os_str().is_empty() {
                return Err(format!("{flag} needs a folder"));
            }
            let c = std::fs::canonicalize(p).map_err(|e| format!("{flag} {}: {e}", p.display()))?;
            if !c.is_dir() {
                return Err(format!("{flag} {}: not a folder", p.display()));
            }
            Ok(c)
        };
        Ok(Roots {
            read: read.iter().map(|p| root(p, "--automation-read-root")).collect::<Result<_, _>>()?,
            write: write.map(|p| root(p, "--automation-write-root")).transpose()?,
        })
    }

    /// No folder at all: every file access is refused.
    pub fn none() -> Roots {
        Roots::default()
    }

    /// The read roots, resolved.
    pub fn read(&self) -> &[PathBuf] {
        &self.read
    }

    /// The write root, resolved.
    pub fn write(&self) -> Option<&Path> {
        self.write.as_deref()
    }

    fn inside(&self, resolved: &Path, access: Access) -> bool {
        let read = || self.read.iter().any(|r| resolved.starts_with(r));
        let write = || self.write.as_ref().is_some_and(|w| resolved.starts_with(w));
        match access {
            Access::Read => read(),
            Access::Write => write(),
            Access::Probe => read() || write(),
        }
    }

    /// Why `path` was refused, naming the roots.
    fn refusal(&self, path: &Path, access: Access) -> String {
        let list = |roots: &[PathBuf]| roots.iter().map(|r| display(r)).collect::<Vec<_>>().join(", ");
        let read = match self.read.len() {
            0 => "no automation read root was given".to_string(),
            1 => format!("the automation read root is {}", list(&self.read)),
            _ => format!("the automation read roots are {}", list(&self.read)),
        };
        let write = match &self.write {
            None => "no automation write root was given".to_string(),
            Some(w) => format!("the automation write root is {}", display(w)),
        };
        let p = display(path);
        match access {
            Access::Read => format!("{p} is outside the folders automation may read: {read}"),
            Access::Write => format!("{p} is outside the folder automation may write to: {write}"),
            Access::Probe => format!("{p} is outside the folders automation may use: {read}, and {write}"),
        }
    }

    /// Whether `path` (absolute) may be looked at on disk at all. On Windows, resolving a path
    /// on another computer (`\\server\share\…`) contacts that computer and offers it the
    /// person's sign-in, and a device path (`\\.\…`) is no file: both are refused by their
    /// spelling, before the file system is asked. A share is allowed when a root is on it.
    fn reachable(&self, abs: &Path) -> bool {
        use std::path::Prefix;
        let share = |p: &Path| match p.components().next() {
            Some(Component::Prefix(x)) => match x.kind() {
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => Some(Some((server.to_ascii_lowercase(), share.to_ascii_lowercase()))),
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => None,
                Prefix::Verbatim(_) | Prefix::DeviceNS(_) => Some(None),
            },
            _ => None,
        };
        match share(abs) {
            None => true,
            Some(None) => false,
            Some(there) => self.read.iter().chain(&self.write).any(|r| share(r) == Some(there.clone())),
        }
    }

    /// `path` resolved, when this policy allows `access` to it; the error names the roots.
    pub fn permit(&self, path: &Path, access: Access) -> Result<PathBuf, String> {
        let abs = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !self.reachable(&abs) {
            return Err(self.refusal(&abs, access));
        }
        let resolved = resolve(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if self.inside(&resolved, access) { Ok(resolved) } else { Err(self.refusal(&resolved, access)) }
    }
}

/// The file `path` names, as the operating system would find it: absolute, with `.` and `..`
/// gone and every symbolic link and junction followed, in the spelling the disk has (so on
/// Windows `C:\WORK\A.PNG` and `c:\work\a.png` resolve alike). It is walked one name at a
/// time: a name that exists is resolved by the system, one that doesn't (a file about to be
/// written, folders about to be made) is kept as given, and `..` then steps back by name, so
/// a link reached after a missing folder and `..` is still followed. A link that points
/// nowhere is refused: writing through it would create a file wherever it points.
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    let invalid = |m: &str| io::Error::new(io::ErrorKind::InvalidInput, m.to_string());
    let abs = std::path::absolute(path)?;
    let parts: Vec<Component> = abs.components().collect();
    if parts.len() > MAX_COMPONENTS {
        return Err(invalid("the path is too deep"));
    }
    // The drive or share and its root folder, which must exist.
    let start = parts.iter().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).count();
    if start == 0 || !matches!(parts.get(start - 1), Some(Component::RootDir)) {
        return Err(invalid("the path is not absolute"));
    }
    let mut out = std::fs::canonicalize(parts.iter().take(start).collect::<PathBuf>())?;
    for c in parts.iter().skip(start) {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(invalid("the path leaves the root folder"));
                }
            }
            Component::Normal(name) => {
                let next = out.join(name);
                out = match std::fs::canonicalize(&next) {
                    Ok(real) => real,
                    // Not there, or a link to something that isn't: the second can't be allowed.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        if std::fs::symlink_metadata(&next).is_ok() {
                            return Err(invalid("the path goes through a link that points nowhere"));
                        }
                        next
                    }
                    Err(e) => return Err(e),
                };
            }
            Component::RootDir | Component::Prefix(_) => return Err(invalid("the path is not valid")),
        }
    }
    Ok(out)
}

/// The policy in force on this thread, and the first access it refused.
struct Active {
    roots: Arc<Roots>,
    denied: Option<String>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
    /// Temporary files AniCraft made itself while confined (exact paths).
    static OWN: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Keeps a policy in force on this thread until dropped; the one before it comes back then.
#[must_use = "the roots apply until this is dropped"]
pub struct Confined {
    before: Option<Active>,
    /// Tied to its thread.
    _here: std::marker::PhantomData<*const ()>,
}

impl Drop for Confined {
    fn drop(&mut self) {
        let before = self.before.take();
        ACTIVE.with(|a| *a.borrow_mut() = before);
    }
}

/// From now until the result is dropped, commands on this thread touch files inside `roots`
/// only.
pub fn confine(roots: Arc<Roots>) -> Confined {
    let before = ACTIVE.with(|a| a.borrow_mut().replace(Active { roots, denied: None }));
    Confined { before, _here: std::marker::PhantomData }
}

/// Runs `f` with no policy in force: for the app's own follow-up to an agent's command (its
/// settings, the Open Recent list), which is not the agent's file access.
pub fn unconfined<R>(f: impl FnOnce() -> R) -> R {
    let held = ACTIVE.with(|a| a.borrow_mut().take());
    let _back = Confined { before: held, _here: std::marker::PhantomData };
    f()
}

/// Whether a policy is in force on this thread.
pub fn active() -> bool {
    ACTIVE.with(|a| a.borrow().is_some())
}

/// The first refusal since [`confine`] (or the last call), for the error an agent reads: a
/// command that asks "is it there?" about a refused file answers "not found" on its own.
pub fn take_denial() -> Option<String> {
    ACTIVE.with(|a| a.borrow_mut().as_mut().and_then(|a| a.denied.take()))
}

/// A temporary file AniCraft makes for itself (a soundtrack for ffmpeg): allowed while this
/// lives, whatever the roots.
pub struct Own(PathBuf);

impl Own {
    /// The file.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        OWN.with(|o| {
            let mut o = o.borrow_mut();
            if let Some(i) = o.iter().position(|p| p == &self.0) {
                o.swap_remove(i);
            }
        });
    }
}

/// Marks `path` as a temporary file of AniCraft's own. The caller chose the name; never pass
/// a path that came from params or a document.
pub fn own(path: &Path) -> Own {
    OWN.with(|o| o.borrow_mut().push(path.to_path_buf()));
    Own(path.to_path_buf())
}

/// Whether `access` to `path` is allowed on this thread. Always, with no policy in force.
pub fn check(path: &Path, access: Access) -> io::Result<()> {
    let Some(roots) = ACTIVE.with(|a| a.borrow().as_ref().map(|a| a.roots.clone())) else { return Ok(()) };
    if crate::vfs::in_memory() || OWN.with(|o| o.borrow().iter().any(|p| p == path)) {
        return Ok(());
    }
    roots.permit(path, access).map(|_| ()).map_err(|why| {
        ACTIVE.with(|a| {
            if let Some(a) = a.borrow_mut().as_mut() {
                a.denied.get_or_insert_with(|| why.clone());
            }
        });
        io::Error::new(io::ErrorKind::PermissionDenied, why)
    })
}

/// Whether `path` is a temporary file of AniCraft's own ([`own`]).
pub fn is_own(path: &Path) -> bool {
    OWN.with(|o| o.borrow().iter().any(|p| p == path))
}

/// The path to hand a program that opens the file itself (ffmpeg). With no policy in force it
/// is `path` as given. Under a policy it is the checked file's resolved, absolute spelling:
/// the program must open the file that was checked, and it may read the given text another
/// way (ffmpeg takes `file:/tmp/x.mp4` as a protocol, where the file system sees a folder
/// named `file:`).
pub fn for_program(path: &Path, access: Access) -> io::Result<PathBuf> {
    check(path, access)?;
    if !active() || crate::vfs::in_memory() || is_own(path) {
        return Ok(path.to_path_buf());
    }
    resolve(path).map(|p| PathBuf::from(display(&p)))
}

use Access::{Read, Write};

/// Every parameter that names a file or folder, by command, and what the command does there.
/// `engine::tests` checks that no command with such a parameter is missing.
pub const PATH_PARAMS: &[(&str, &[(&str, Access)])] = &[
    ("file.new_from_template", &[("path", Read)]),
    ("file.open", &[("path", Read)]),
    ("file.open_recent", &[("path", Read)]),
    ("file.save", &[("path", Write)]),
    ("file.save_as", &[("path", Write)]),
    ("file.save_as_template", &[("folder", Write)]),
    ("file.delete_template", &[("folder", Write)]),
    ("file.templates", &[("folder", Read)]),
    ("file.import", &[("path", Read)]),
    ("file.import_psd", &[("path", Read)]),
    ("file.import_to_library", &[("path", Read)]),
    ("file.import_video", &[("path", Read)]),
    ("file.video_info", &[("path", Read)]),
    ("layer.set_video", &[("path", Read)]),
    ("file.drop", &[("path", Read), ("paths", Read)]),
    ("file.export_png", &[("path", Write)]),
    ("file.export_svg", &[("path", Write)]),
    ("file.export_video", &[("path", Write)]),
    ("file.export_sprite_sheet", &[("path", Write)]),
    ("file.export_png_sequence", &[("dir", Write)]),
    ("file.export_svg_sequence", &[("dir", Write)]),
    ("file.export_gif", &[("path", Write)]),
    ("file.export_apng", &[("path", Write)]),
    ("file.export_webp", &[("path", Write)]),
    ("file.export_lottie", &[("path", Write)]),
    ("file.export_html", &[("path", Write)]),
    ("file.export_plugin", &[("path", Write)]),
    ("file.export_audio", &[("path", Write)]),
    ("file.recovery_list", &[("dir", Read)]),
    ("file.recover", &[("dir", Read)]),
    ("file.discard_recovery", &[("dir", Write)]),
    ("file.autosave", &[("dir", Write)]),
    ("desktop.integrate", &[("appimage", Read), ("data_home", Write)]),
    ("desktop.remove", &[("data_home", Write)]),
    ("desktop.status", &[("data_home", Read), ("appimage", Read)]),
    ("brush.preset_export", &[("path", Write)]),
    ("brush.preset_import", &[("path", Read)]),
    ("filter.preset_export", &[("path", Write)]),
    ("filter.preset_import", &[("path", Read)]),
    ("swatch.export", &[("path", Write)]),
    ("swatch.import", &[("path", Read)]),
    ("data.import_table", &[("path", Read)]),
    ("plugin.install", &[("path", Read)]),
    ("plugin.reload", &[("folder", Read)]),
    ("macro.run", &[("path", Read), ("folder", Read)]),
    ("macro.list", &[("folder", Read)]),
    ("macro.save", &[("folder", Write)]),
    ("macro.rename", &[("folder", Write)]),
    ("macro.delete", &[("folder", Write)]),
    ("macro.move", &[("folder", Write)]),
    ("macro.import", &[("folder", Write), ("path", Read)]),
    ("rig.auto_detect", &[("path", Read)]),
    ("rig.save_preset", &[("path", Write)]),
    ("rig.load_preset", &[("path", Read), ("artwork_path", Read)]),
];

/// Commands whose `path`, `dir` or `folder` is not a place on disk (an element, a Library
/// folder id, a motion path), or that only edit a list kept in memory.
pub const NOT_FILES: &[&str] = &[
    "find.goto",
    "layer.delete",
    "layer.move",
    "layer.set_opacity",
    "motion.info",
    "motion.set_path",
    "motion.clear_path",
    "motion.orient",
    "motion.add_point",
    "motion.delete_points",
    "motion.move_path",
    "motion.unpin_point",
    "motion.move_keys",
    "frames.edit_multiple",
    "draw.path",
    "path.extend",
    "path.move_points",
    // Deletes the plug-in's own file in the plug-ins folder: asked where it is deleted.
    "plugin.remove",
    "text.attach_path",
    "text.path",
    "text.release_path",
    "library.items",
    "library.new_folder",
    "library.rename",
    "library.move",
    "library.duplicate",
    "library.delete",
    "modify.convert_to_symbol",
    "symbol.new_empty",
    "prefs.recent_pin",
    "prefs.recent_remove",
    "file.recent_list",
    "publish.settings_set",
    "data.list",
];

/// Checks the paths `params` gives command `id` against the policy in force (none: allowed).
/// The error names the parameter and the roots.
pub fn check_params(id: &str, params: &Value) -> Result<(), String> {
    if !active() {
        return Ok(());
    }
    let Some((_, keys)) = PATH_PARAMS.iter().find(|(c, _)| *c == id) else { return Ok(()) };
    for (key, access) in *keys {
        let one = |v: &Value| -> Result<(), String> {
            // Not text: the command's own validation says so.
            let Some(text) = v.as_str() else { return Ok(()) };
            check(Path::new(text), *access).map_err(|e| format!("param '{key}': {e}"))
        };
        match params.get(key) {
            Some(Value::Array(list)) => list.iter().try_for_each(one)?,
            Some(v) => one(v)?,
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    /// `<temp>/…/{in,out,secret}`, with a file in `in` and one in `secret`.
    fn scratch(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("anicraft-access-{tag}-{}-{}", std::process::id(), crate::vfs::random_suffix()));
        for d in ["in", "out", "secret"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("in").join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("secret").join("s.txt"), b"s").unwrap();
        Scratch(dir)
    }
    fn roots(s: &Scratch) -> Roots {
        Roots::new(&[s.0.join("in")], Some(&s.0.join("out"))).unwrap()
    }

    #[test]
    fn inside_is_allowed_outside_is_refused_naming_the_root() {
        let s = scratch("inout");
        let r = roots(&s);
        assert!(r.permit(&s.0.join("in").join("a.txt"), Access::Read).is_ok());
        assert!(r.permit(&s.0.join("out").join("new").join("b.png"), Access::Write).is_ok(), "folders not made yet");
        let e = r.permit(&s.0.join("secret").join("s.txt"), Access::Read).unwrap_err();
        assert!(e.contains("automation read root") && e.contains(&display(&r.read()[0])), "{e}");
        let e = r.permit(&s.0.join("in").join("b.png"), Access::Write).unwrap_err();
        assert!(e.contains("automation write root") && e.contains(&display(r.write().unwrap())), "{e}");
        // Reading needs a read root, writing the write root; asking "is it there?" either.
        assert!(r.permit(&s.0.join("out").join("b.png"), Access::Read).is_err());
        assert!(r.permit(&s.0.join("out").join("b.png"), Access::Probe).is_ok());
        assert!(r.permit(&s.0.join("in").join("a.txt"), Access::Probe).is_ok());
        assert!(r.permit(&s.0.join("secret"), Access::Probe).is_err());
        // A sibling whose name starts like the root's is not inside it.
        std::fs::create_dir_all(s.0.join("in2")).unwrap();
        assert!(r.permit(&s.0.join("in2").join("x"), Access::Read).is_err());
    }

    #[test]
    fn a_root_itself_is_inside_and_dot_dot_cannot_leave() {
        let s = scratch("dots");
        let r = roots(&s);
        assert!(r.permit(&s.0.join("in"), Access::Read).is_ok());
        assert!(r.permit(&s.0.join("out"), Access::Write).is_ok());
        assert!(r.permit(&s.0.join("in").join("..").join("secret").join("s.txt"), Access::Read).is_err());
        assert!(r.permit(&s.0.join("out").join("nope").join("..").join("..").join("secret").join("x"), Access::Write).is_err());
        // `..` that stays inside is fine, through folders that exist or don't.
        assert!(r.permit(&s.0.join("in").join("..").join("in").join("a.txt"), Access::Read).is_ok());
        assert!(r.permit(&s.0.join("out").join("nope").join("..").join("b.png"), Access::Write).is_ok());
        // Relative paths start at the working folder, which is not a root here.
        assert!(r.permit(Path::new("a.txt"), Access::Read).is_err());
        assert!(r.permit(Path::new(""), Access::Read).is_err());
    }

    #[test]
    fn roots_must_be_folders_that_exist() {
        let s = scratch("roots");
        assert!(Roots::new(&[s.0.join("missing")], None).unwrap_err().contains("--automation-read-root"));
        assert!(Roots::new(&[], Some(&s.0.join("in").join("a.txt"))).unwrap_err().contains("--automation-write-root"));
        assert!(Roots::new(&[PathBuf::new()], None).is_err());
        let none = Roots::none();
        assert!(none.permit(&s.0.join("in").join("a.txt"), Access::Probe).unwrap_err().contains("no automation read root"));
    }

    /// A link inside a root to somewhere outside it is outside; one from outside in is inside.
    #[test]
    fn links_are_followed_before_the_check() {
        let s = scratch("links");
        let r = roots(&s);
        let link = |target: &Path, at: &Path| -> bool {
            #[cfg(unix)]
            return std::os::unix::fs::symlink(target, at).is_ok();
            // Windows: a junction, which needs no privilege (symbolic links do).
            #[cfg(windows)]
            return std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(at).arg(target).output().is_ok_and(|o| o.status.success());
            #[cfg(not(any(unix, windows)))]
            return false;
        };
        assert!(link(&s.0.join("secret"), &s.0.join("in").join("way-out")), "this system makes links");
        assert!(r.permit(&s.0.join("in").join("way-out").join("s.txt"), Access::Read).is_err());
        assert!(r.permit(&s.0.join("in").join("way-out").join("new.txt"), Access::Probe).is_err(), "a file not there yet, too");
        assert!(link(&s.0.join("out"), &s.0.join("in").join("to-out")));
        assert!(r.permit(&s.0.join("in").join("to-out").join("x.png"), Access::Write).is_ok(), "where it lands decides");
        assert!(link(&s.0.join("in"), &s.0.join("secret").join("way-in")));
        assert!(r.permit(&s.0.join("secret").join("way-in").join("a.txt"), Access::Read).is_ok());
        // A link to nowhere: writing through it would create its target.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(s.0.join("secret").join("made"), s.0.join("out").join("dangling")).unwrap();
            assert!(r.permit(&s.0.join("out").join("dangling"), Access::Write).is_err());
        }
        // A link reached after a folder that isn't there and `..` is followed all the same.
        assert!(r.permit(&s.0.join("in").join("missing").join("..").join("way-out").join("new.txt"), Access::Probe).is_err());
        assert!(r.permit(&s.0.join("out").join("missing").join("..").join("..").join("in").join("way-out").join("new"), Access::Probe).is_err());
        // A root given through a link is the folder it leads to.
        let via = Roots::new(&[s.0.join("secret").join("way-in")], None).unwrap();
        assert_eq!(via.read(), r.read());
    }

    #[cfg(windows)]
    #[test]
    fn windows_spellings_resolve_alike() {
        let s = scratch("win");
        let r = roots(&s);
        let upper = PathBuf::from(s.0.join("in").join("a.txt").display().to_string().to_uppercase());
        assert!(r.permit(&upper, Access::Read).is_ok(), "letter case");
        let slashes = PathBuf::from(s.0.join("in").join("a.txt").display().to_string().replace('\\', "/"));
        assert!(r.permit(&slashes, Access::Read).is_ok(), "forward slashes");
        // A device name inside the root is the device, not a file there.
        assert!(r.permit(&s.0.join("out").join("NUL"), Access::Write).is_err());
        // Another computer is refused by its spelling: asking it would offer it a sign-in.
        // (`.invalid` never resolves, so a regression here fails slowly rather than leaks.)
        for there in [r"\\anicraft-test.invalid\share\a.png", r"\\?\UNC\anicraft-test.invalid\share\a.png", "//anicraft-test.invalid/share/a.png", r"\\.\PhysicalDrive0", r"\\?\GLOBALROOT\Device\x"] {
            let e = r.permit(Path::new(there), Access::Probe).unwrap_err();
            assert!(e.contains("outside the folders automation may use"), "{there}: {e}");
        }
        assert!(!display(&r.read()[0]).starts_with(r"\\?\"));
    }

    #[test]
    fn a_policy_is_scoped_to_its_guard_and_thread() {
        let s = scratch("scope");
        let secret = s.0.join("secret").join("s.txt");
        assert!(check(&secret, Access::Read).is_ok(), "no policy: no limit");
        {
            let _g = confine(Arc::new(roots(&s)));
            assert!(active());
            assert_eq!(check(&secret, Access::Read).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            assert!(check(&s.0.join("in").join("a.txt"), Access::Read).is_ok());
            assert!(take_denial().is_some_and(|d| d.contains("s.txt")));
            assert!(take_denial().is_none());
            assert!(unconfined(|| check(&secret, Access::Read)).is_ok());
            assert!(check(&secret, Access::Read).is_err(), "back in force");
            let roots = Arc::new(roots(&s));
            std::thread::spawn(move || {
                let _ = &roots;
                assert!(!active());
            })
            .join()
            .unwrap();
            // AniCraft's own temporary file, while it is held.
            let tmp = std::env::temp_dir().join(format!("anicraft-access-own-{}", crate::vfs::random_suffix()));
            assert!(check(&tmp, Access::Write).is_err());
            let held = own(&tmp);
            assert!(check(&tmp, Access::Write).is_ok());
            assert_eq!(for_program(&tmp, Access::Write).unwrap(), tmp);
            // A program is handed the file that was checked, spelt so it can't be read another way.
            let given = for_program(&s.0.join("in").join(".").join("a.txt"), Access::Read).unwrap();
            assert!(given.is_absolute() && given.ends_with("a.txt") && !given.display().to_string().contains("file:"), "{}", given.display());
            assert!(for_program(&secret, Access::Read).is_err());
            drop(held);
            assert!(check(&tmp, Access::Write).is_err());
        }
        assert!(!active() && check(&secret, Access::Read).is_ok());
        assert_eq!(for_program(Path::new("a.mp4"), Access::Write).unwrap(), Path::new("a.mp4"), "no policy: as given");
    }

    #[test]
    fn params_are_checked_by_name_before_a_command_runs() {
        let s = scratch("params");
        let _g = confine(Arc::new(roots(&s)));
        let inside = s.0.join("in").join("a.txt").display().to_string();
        let outside = s.0.join("secret").join("s.txt").display().to_string();
        assert!(check_params("file.open", &serde_json::json!({"path": inside})).is_ok());
        let e = check_params("file.open", &serde_json::json!({"path": outside})).unwrap_err();
        assert!(e.starts_with("param 'path':") && e.contains("automation read root"), "{e}");
        assert!(check_params("file.save_as", &serde_json::json!({"path": inside})).is_err(), "the read root is not written to");
        assert!(check_params("file.drop", &serde_json::json!({"paths": [inside, outside]})).is_err());
        assert!(check_params("draw.rect", &serde_json::json!({"path": outside})).is_ok(), "not a file parameter");
        assert!(check_params("file.open", &serde_json::json!({"path": 7})).is_ok(), "the command refuses that itself");
    }
}
