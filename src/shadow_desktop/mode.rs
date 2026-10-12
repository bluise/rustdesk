//! The shadow-desktop session: whether the mode is on, what it owns, and where the remote
//! pointer and keyboard focus are.
//!
//! A hidden desktop has no real cursor and no foreground window, so both have to be kept
//! here instead of asked of Windows:
//!
//! * relative mouse events arrive as deltas, so they are accumulated into
//!   [`cursor`] and turned back into a point on the desktop;
//! * keyboard input goes to the window the remote side last clicked on, remembered as
//!   [`focus`].
//!
//! The session also owns the desktop handle, which is what keeps the desktop - and the
//! program started on it - alive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use super::desktop::Desktop;
use super::{launch, windows, SHADOW_DESKTOP};

/// Started when the mode comes up: a desktop with nothing on it cannot be worked with.
pub const DEFAULT_PROGRAM: &str = "explorer.exe";
/// Frames per second ceiling. PrintWindow redraws every window per frame, so this is the
/// host's own limit rather than the client's.
pub const DEFAULT_FPS: u32 = 10;

/// The size frames are produced at, if the host was told to scale them down.
///
/// The desktop itself always keeps the session's resolution: changing that would change the
/// physical screen, which is the one thing this mode must not do. Scaling the frames only
/// trades detail for bandwidth and CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: i32,
    pub height: i32,
}

struct Session {
    /// Closing its last handle destroys the desktop and everything on it.
    _desktop: Desktop,
    program_pid: Option<u32>,
    cursor: (i32, i32),
    focus: Option<isize>,
    fps: u32,
    frame_size: Option<FrameSize>,
    /// The desktop's own size, which relative mouse movement is clamped to.
    size: (i32, i32),
}

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Session>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Runs `f` on the session, if the mode is on.
fn with<T>(f: impl FnOnce(&mut Session) -> T) -> Option<T> {
    slot().lock().ok()?.as_mut().map(f)
}

pub fn is_active() -> bool {
    slot()
        .lock()
        .map(|session| session.is_some())
        .unwrap_or(false)
}

/// Turns the mode on: switch the process into WinSta0, create the desktop, start the
/// program on it, and remember the frame rate.
///
/// Doing nothing when it is already on makes this safe to call from wherever the setting is
/// read.
pub fn enable(program: Option<&str>, fps: u32, frame_size: Option<FrameSize>) -> Result<(), String> {
    let mut guard = slot()
        .lock()
        .map_err(|_| "shadow session lock is poisoned".to_owned())?;
    if guard.is_some() {
        return Ok(());
    }

    // The process has to sit in WinSta0 for capture and input threads to be able to attach,
    // and it has to keep sitting there.
    super::desktop::ensure_process_in_interactive_winsta()?;
    let desktop = Desktop::create_or_open(SHADOW_DESKTOP)?;
    let pid = if explorer_running_on(&desktop) {
        // A daemon restart would otherwise launch another explorer onto a desktop that
        // keeps every window, so the user ends up with several "This PC" windows.
        eprintln!("[shadow_desktop] explorer is already on the desktop, not starting another");
        None
    } else {
        let program = program.unwrap_or(DEFAULT_PROGRAM);
        match launch::spawn_on_desktop(&desktop.qualified_name(), program, &[]) {
            Ok(pid) => {
                eprintln!("[shadow_desktop] started {program} on the desktop");
                Some(pid)
            }
            Err(e) => {
                // The desktop is still usable by hand, so this is not fatal.
                eprintln!("[shadow_desktop] could not start the program: {e}");
                None
            }
        }
    };

    *guard = Some(Session {
        _desktop: desktop,
        program_pid: pid,
        cursor: (0, 0),
        focus: None,
        fps: fps.max(1),
        frame_size,
        size: super::capture::desktop_size(None)?,
    });
    Ok(())
}

/// Whether Windows Explorer already has a window on this desktop.
///
/// The desktop and its windows outlive a restart of the daemon, so starting the program
/// again without asking first stacks a second explorer - and a third - on top.
fn explorer_running_on(desktop: &Desktop) -> bool {
    const EXPLORER_CLASSES: [&str; 2] = ["CabinetWClass", "ExploreWClass"];
    match windows::list(&desktop.qualified_name()) {
        Ok(list) => list
            .iter()
            .any(|w| EXPLORER_CLASSES.iter().any(|c| w.class.eq_ignore_ascii_case(c))),
        Err(e) => {
            // A listing that failed is not proof there is none, so start the program the
            // way this did before the check existed.
            eprintln!("[shadow_desktop] could not list the desktop's windows: {e}");
            false
        }
    }
}

/// Turns the mode off. Dropping the desktop handle destroys the desktop, which takes the
/// program started on it with it.
pub fn disable() {
    if let Ok(mut guard) = slot().lock() {
        *guard = None;
    }
}

pub fn fps() -> u32 {
    with(|s| s.fps).unwrap_or(DEFAULT_FPS)
}

/// The size frames should be scaled to, or `None` to keep the desktop's own resolution.
pub fn frame_size() -> Option<FrameSize> {
    with(|s| s.frame_size).flatten()
}

/// Reads the frame size setting: `WxH`, or `N%` of the desktop's own size.
///
/// Anything unusable - including an empty value, which is what an unset option reads as -
/// means "leave the frames at the desktop's resolution" rather than a guess.
pub fn parse_frame_size(value: &str, desktop: (i32, i32)) -> Option<FrameSize> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(percent) = value.strip_suffix('%') {
        let percent: i32 = percent.trim().parse().ok()?;
        if percent <= 0 {
            return None;
        }
        let (width, height) = desktop;
        return Some(FrameSize {
            width: (width * percent / 100).max(1),
            height: (height * percent / 100).max(1),
        });
    }
    let (width, height) = value.split_once(['x', 'X'])?;
    let width: i32 = width.trim().parse().ok()?;
    let height: i32 = height.trim().parse().ok()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(FrameSize { width, height })
}

/// Reads the frame rate setting, clamped to something a desktop can actually deliver.
pub fn parse_fps(value: &str) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .map(|fps| fps.clamp(1, 60))
        .unwrap_or(DEFAULT_FPS)
}

/// Whether the mode should be on.
///
/// An unset option means on: the setting only exists in a build that carries the mode, so
/// "unset" and "off" would mean the same thing - and then the build would do nothing.
pub fn wanted(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes" | "true" | "1" | "on"
    )
}

pub fn cursor() -> Option<(i32, i32)> {
    with(|s| s.cursor)
}

/// The desktop's own size, for clamping relative movement.
pub fn desktop_size() -> Option<(i32, i32)> {
    with(|s| s.size)
}

/// Moves the tracked pointer, clamped to the desktop so it cannot wander off.
pub fn move_cursor_by(dx: i32, dy: i32, width: i32, height: i32) -> Option<(i32, i32)> {
    with(|s| {
        let x = (s.cursor.0 + dx).clamp(0, width.max(1) - 1);
        let y = (s.cursor.1 + dy).clamp(0, height.max(1) - 1);
        s.cursor = (x, y);
        s.cursor
    })
}

pub fn set_cursor(point: (i32, i32)) -> Option<(i32, i32)> {
    with(|s| {
        s.cursor = point;
        s.cursor
    })
}

/// The window keyboard input should go to: the last one a click landed on.
pub fn focus() -> Option<isize> {
    with(|s| s.focus).flatten()
}

pub fn set_focus(hwnd: Option<isize>) {
    with(|s| s.focus = hwnd);
}

/// The pid of the program started on the shadow desktop, if it came up.
pub fn program_pid() -> Option<u32> {
    with(|s| s.program_pid).flatten()
}

/// Which screen each connection is looking at, so input can be routed to where the
/// operator's eyes are.
///
/// The video side knows this when it picks a capturer; it only has to say so. Until it says
/// anything, a connection is assumed to be on the hidden desktop, which is the screen listed
/// first and therefore the one a client lands on by default.
static VIEWING_SHADOW: OnceLock<Mutex<HashMap<i32, bool>>> = OnceLock::new();

fn viewing_shadow() -> &'static Mutex<HashMap<i32, bool>> {
    VIEWING_SHADOW.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn set_viewing_shadow(conn: i32, shadow: bool) {
    if let Ok(mut map) = viewing_shadow().lock() {
        map.insert(conn, shadow);
    }
}

pub fn is_viewing_shadow(conn: i32) -> bool {
    viewing_shadow()
        .lock()
        .ok()
        .and_then(|map| map.get(&conn).copied())
        .unwrap_or(true)
}

/// The same question without a connection to ask about.
///
/// Keyboard input arrives without a connection id, and the shadow desktop is one screen
/// shared by whoever is connected, so the last switch to say which screen is on view is
/// what the keyboard path goes by.
static VIEWING_SHADOW_ANY: AtomicBool = AtomicBool::new(true);

pub fn set_viewing_shadow_any(shadow: bool) {
    VIEWING_SHADOW_ANY.store(shadow, Ordering::SeqCst);
}

pub fn is_viewing_shadow_any() -> bool {
    VIEWING_SHADOW_ANY.load(Ordering::SeqCst)
}