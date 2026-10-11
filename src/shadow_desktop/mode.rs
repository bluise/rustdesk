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
use super::{launch, SHADOW_DESKTOP};

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
    let pid = match launch::spawn_on_desktop(&desktop.qualified_name(), program.unwrap_or(DEFAULT_PROGRAM), &[]) {
        Ok(pid) => Some(pid),
        Err(e) => {
            // The desktop is still usable by hand, so this is not fatal.
            eprintln!("[shadow_desktop] could not start the program: {e}");
            None
        }
    };

    *guard = Some(Session {
        _desktop: desktop,
        program_pid: pid,
        cursor: (0, 0),
        focus: None,
        fps: fps.max(1),
        frame_size,
        size: super::capture::desktop_size()?,
    });
    Ok(())
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