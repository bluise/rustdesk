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

use std::sync::{Mutex, OnceLock};

use super::desktop::Desktop;
use super::{launch, SHADOW_DESKTOP};

/// Started when the mode comes up: a desktop with nothing on it cannot be worked with.
pub const DEFAULT_PROGRAM: &str = "explorer.exe";
/// Frames per second ceiling. PrintWindow redraws every window per frame, so this is the
/// host's own limit rather than the client's.
pub const DEFAULT_FPS: u32 = 10;

struct Session {
    /// Closing its last handle destroys the desktop and everything on it.
    _desktop: Desktop,
    program_pid: Option<u32>,
    cursor: (i32, i32),
    focus: Option<isize>,
    fps: u32,
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
pub fn enable(program: Option<&str>, fps: u32) -> Result<(), String> {
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

pub fn cursor() -> Option<(i32, i32)> {
    with(|s| s.cursor)
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