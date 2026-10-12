//! SendInput into a desktop that is not the input desktop.
//!
//! Windows routes injected input to the desktop the calling thread is attached to, so -
//! exactly like capture - this runs on a thread of its own that has been switched with
//! `SetThreadDesktop` and owns no windows.
//!
//! MSDN notes that SendInput is subject to UIPI: input only reaches windows of an equal
//! or lower integrity level, and a blocked call reports neither an error code nor a
//! shorter-than-expected return. Every call is checked.

use std::mem::{size_of, zeroed};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, TRUE, UINT};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::winuser::{
    BringWindowToTop, ClientToScreen, EnumWindows, GetAncestor, GetClientRect, GetDoubleClickTime,
    GetForegroundWindow, GetSystemMetrics, GetWindowTextW, INPUT_u, IsWindowVisible, PostMessageW,
    RealChildWindowFromPoint, ScreenToClient, SendInput, SetFocus, SetForegroundWindow,
    WindowFromPoint, GA_ROOT, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    LPINPUT, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SM_CXSCREEN,
    SM_CYSCREEN, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use super::desktop::AttachedDesktop;
use super::last_error;

/// Marks our own events, the way enigo tags its input: the peer-side hook can then tell
/// injected input from a real user, which is what privacy mode does.
const INPUT_TAG: usize = 0x5244_534B; // "RDSK"

#[derive(Debug, Clone, Copy)]
pub enum Action {
    MouseMove { x: i32, y: i32 },
    LeftClick { x: i32, y: i32 },
    KeyDown { vk: u16 },
    KeyUp { vk: u16 },
}

/// Plays `actions` on `desktop`, in order.
pub fn send(desktop: &str, actions: &[Action]) -> Result<(), String> {
    let desktop = desktop.to_owned();
    let actions = actions.to_vec();
    thread::spawn(move || send_on_thread(&desktop, &actions))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn send_on_thread(desktop: &str, actions: &[Action]) -> Result<(), String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    for action in actions {
        match *action {
            Action::MouseMove { x, y } => {
                send_mouse(x, y, 0, 0, screen_w, screen_h)?;
            }
            Action::LeftClick { x, y } => {
                send_mouse(x, y, MOUSEEVENTF_LEFTDOWN, 0, screen_w, screen_h)?;
                thread::sleep(Duration::from_millis(20));
                send_mouse(x, y, MOUSEEVENTF_LEFTUP, 0, screen_w, screen_h)?;
            }
            Action::KeyDown { vk } => send_key(vk, false)?,
            Action::KeyUp { vk } => send_key(vk, true)?,
        }
        thread::sleep(Duration::from_millis(15));
    }
    Ok(())
}

fn send_mouse(
    x: i32,
    y: i32,
    flags: u32,
    wheel: i32,
    screen_w: i32,
    screen_h: i32,
) -> Result<(), String> {
    // Absolute mouse events are normalised over the primary monitor's extent.
    let nx = normalise(x, screen_w);
    let ny = normalise(y, screen_h);
    let input = INPUT {
        type_: INPUT_MOUSE,
        u: unsafe {
            let mut u: INPUT_u = zeroed();
            *u.mi_mut() = MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: wheel as u32,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | flags,
                time: 0,
                dwExtraInfo: INPUT_TAG,
            };
            u
        },
    };
    push(input)
}

fn normalise(v: i32, span: i32) -> i32 {
    // 0..65535 over the desktop's pixel range; clamp instead of wrapping.
    if span <= 1 {
        return 0;
    }
    let v = v.clamp(0, span - 1) as i64;
    ((v * 65535) / (span - 1) as i64) as i32
}

fn send_key(vk: u16, up: bool) -> Result<(), String> {
    let input = INPUT {
        type_: INPUT_KEYBOARD,
        u: unsafe {
            let mut u: INPUT_u = zeroed();
            *u.ki_mut() = KEYBDINPUT {
                wVk: vk,
                // Scan-code injection would be needed for DirectInput games; virtual keys
                // are enough for ordinary windows.
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INPUT_TAG,
            };
            u
        },
    };
    push(input)
}

fn push(input: INPUT) -> Result<(), String> {
    let mut inputs = [input];
    let sent = unsafe {
        SendInput(
            1,
            inputs.as_mut_ptr() as LPINPUT,
            size_of::<INPUT>() as std::os::raw::c_int,
        )
    };
    if sent as UINT != 1 {
        return Err(format!(
            "SendInput delivered {sent} of 1 events (blocked by UIPI, or the desktop is gone)"
        ));
    }
    Ok(())
}

/// Puts a window of the hidden desktop in front and gives it focus.
///
/// A hidden desktop has no foreground window of its own, so injected input has nothing to
/// land on: SendInput reports success and the keystroke goes nowhere. This is what gives
/// it a receiver.
pub fn focus_window(desktop: &str, hwnd: Option<isize>) -> Result<String, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || focus_on_thread(&desktop, hwnd))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn focus_on_thread(desktop: &str, hwnd: Option<isize>) -> Result<String, String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (target, label) = pick_target(hwnd)?;
    let mut notes = vec![format!("target {label}")];
    unsafe {
        notes.push(format!(
            "foreground before: {:#x}",
            GetForegroundWindow() as usize
        ));
        if SetForegroundWindow(target) == FALSE {
            notes.push(format!("SetForegroundWindow failed: {}", last_error()));
        } else {
            notes.push("SetForegroundWindow ok".to_owned());
        }
        if BringWindowToTop(target) == FALSE {
            notes.push(format!("BringWindowToTop failed: {}", last_error()));
        } else {
            notes.push("BringWindowToTop ok".to_owned());
        }
        if SetFocus(target).is_null() {
            notes.push(format!("SetFocus failed: {}", last_error()));
        } else {
            notes.push("SetFocus ok".to_owned());
        }
        notes.push(format!(
            "foreground after: {:#x}",
            GetForegroundWindow() as usize
        ));
    }
    Ok(notes.join("; "))
}

/// Types into a window of the hidden desktop by posting the characters to it.
///
/// Posting does not need the window to hold focus, which makes it the dependable path
/// while focus handling on a hidden desktop is still being worked out.
pub fn type_text(desktop: &str, hwnd: Option<isize>, text: &str) -> Result<String, String> {
    let desktop = desktop.to_owned();
    let text = text.to_owned();
    thread::spawn(move || type_on_thread(&desktop, hwnd, &text))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn type_on_thread(desktop: &str, hwnd: Option<isize>, text: &str) -> Result<String, String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (target, label) = pick_target(hwnd)?;
    let mut sent = 0usize;
    for ch in text.chars() {
        unsafe {
            if PostMessageW(target, WM_CHAR, ch as usize, 0) != FALSE {
                sent += 1;
            }
        }
        thread::sleep(Duration::from_millis(30));
    }
    Ok(format!(
        "posted {sent}/{} characters to {label}",
        text.chars().count()
    ))
}

/// The window a command should act on: the one it was given, or the control under the
/// centre of the first visible titled window.
///
/// The frame is not the receiver - a text box or a canvas is, and it is a child of the
/// frame. Aiming at the frame is why a posted character can be accepted and then ignored.
fn pick_target(hwnd: Option<isize>) -> Result<(HWND, String), String> {
    if let Some(handle) = hwnd.filter(|h| *h != 0) {
        return Ok((handle as HWND, format!("{handle:#x}")));
    }
    unsafe {
        let mut found: Option<(HWND, String)> = None;
        EnumWindows(
            Some(first_titled),
            &mut found as *mut Option<(HWND, String)> as LPARAM,
        );
        match found {
            Some((frame, title)) => {
                let target = deepest_child_at(frame);
                let suffix = if target == frame {
                    ""
                } else {
                    " (child control)"
                };
                Ok((target, format!("{:#x} {title:?}{suffix}", target as usize)))
            }
            None => Err("no visible window with a title on this desktop".to_owned()),
        }
    }
}

/// Walks into the window under the centre of `frame`, the way a click would land.
unsafe fn deepest_child_at(frame: HWND) -> HWND {
    let mut client: RECT = zeroed();
    if GetClientRect(frame, &mut client) == FALSE {
        return frame;
    }
    let mut point = POINT {
        x: (client.left + client.right) / 2,
        y: (client.top + client.bottom) / 2,
    };
    if ClientToScreen(frame, &mut point) == FALSE {
        return frame;
    }
    let mut current = WindowFromPoint(point);
    if current.is_null() {
        return frame;
    }
    loop {
        let mut local = point;
        if ScreenToClient(current, &mut local) == FALSE {
            return current;
        }
        let child = RealChildWindowFromPoint(current, local);
        if child.is_null() || child == current {
            return current;
        }
        current = child;
    }
}

/// The button a mouse event is about.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A mouse event to inject, as the remote side asked for it.
#[derive(Debug, Clone, Copy)]
pub enum MouseAction {
    Move,
    Down(MouseButton),
    Up(MouseButton),
    Wheel(i32),
}

/// The window at a point on this desktop, the way a click would find it.
pub fn window_at(desktop: &str, x: i32, y: i32) -> Result<isize, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || {
        let _attached = AttachedDesktop::attach(&desktop)?;
        Ok(unsafe { deepest_at(POINT { x, y }) } as isize)
    })
    .join()
    .map_err(|_| "input thread panicked".to_owned())?
}

/// Which button the operator is holding, so a move can carry it and a drag can be told
/// apart from a hover.
static HELD_BUTTON: AtomicU32 = AtomicU32::new(0);
/// The last press, for telling a double click's second press apart from a fresh one.
static LAST_PRESS: Mutex<Option<(Instant, i32, i32)>> = Mutex::new(None);

/// Injects a mouse event at `(x, y)` into this desktop.
///
/// Posted window messages rather than `SendInput`: Windows delivers injected input to the
/// input desktop only, so on a hidden desktop every `SendInput` call reports "delivered 0
/// of 1" and the screen is dead to the operator. Posting reaches any desktop, and pays for
/// it with the parts Windows would otherwise do itself - the frame (title bar, its buttons,
/// dragging, activation) and the shell gestures that insist on a real button state.
pub fn send_mouse_at(desktop: &str, x: i32, y: i32, action: MouseAction) -> Result<String, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || mouse_at_on_thread(&desktop, x, y, action))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn mouse_at_on_thread(
    desktop: &str,
    x: i32,
    y: i32,
    action: MouseAction,
) -> Result<String, String> {
    // A move carries its position and becomes the point later events act on. Button and
    // wheel events carry none - the protocol leaves their x/y zero - so they act on that
    // remembered point; acting on the point they arrived with sent every click to (0,0).
    let (x, y) = match action {
        MouseAction::Move => {
            super::mode::set_cursor((x, y));
            (x, y)
        }
        _ => super::mode::cursor().unwrap_or((x, y)),
    };
    let _attached = AttachedDesktop::attach(desktop)?;
    let target = unsafe { deepest_at(POINT { x, y }) };
    if target.is_null() {
        return Err(format!("no window at {x},{y}"));
    }
    let mut note = String::new();
    // Mouse messages carry client coordinates; the wheel is the exception and takes the
    // screen point.
    let (cx, cy) = client_point(target, x, y);
    let (message, wparam, lparam) = match action {
        MouseAction::Move => (
            WM_MOUSEMOVE,
            HELD_BUTTON.load(Ordering::SeqCst) as usize,
            pack(cx, cy),
        ),
        MouseAction::Down(button) => {
            // A posted click cannot activate a window, and everything the frame owns - the
            // title bar, its buttons, dragging - is decided by Windows, not by the control.
            activate(target, &mut note);
            let double = matches!(button, MouseButton::Left) && second_press(x, y);
            HELD_BUTTON.store(button_mask(button), Ordering::SeqCst);
            (
                if double {
                    WM_LBUTTONDBLCLK
                } else {
                    down_message(button)
                },
                button_mask(button) as usize,
                pack(cx, cy),
            )
        }
        MouseAction::Up(button) => {
            HELD_BUTTON.store(0, Ordering::SeqCst);
            (up_message(button), 0, pack(cx, cy))
        }
        MouseAction::Wheel(delta) => (
            WM_MOUSEWHEEL,
            ((delta as u32 & 0xFFFF) << 16) as usize,
            pack(x, y),
        ),
    };
    if unsafe { PostMessageW(target, message, wparam, lparam) } == FALSE {
        return Err(format!(
            "PostMessage({message:#x}) to {:#x} failed: {}",
            target as usize,
            last_error()
        ));
    }
    // Remember where a click landed: keyboard input follows it, because a hidden desktop
    // never has a foreground window to ask.
    if let MouseAction::Down(_) = action {
        super::mode::set_focus(Some(target as isize));
    }
    note.push_str(&format!(
        "mouse {action:?} at {x},{y} -> {:#x} {message:#x}",
        target as usize
    ));
    Ok(note)
}

/// Client coordinates of a screen point inside `hwnd`.
fn client_point(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut point = POINT { x, y };
    if unsafe { ScreenToClient(hwnd, &mut point) } == FALSE {
        return (x, y);
    }
    (point.x, point.y)
}

/// Two 16-bit halves, low first - the layout every mouse message expects.
fn pack(x: i32, y: i32) -> isize {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
}

fn button_mask(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => MK_LBUTTON,
        MouseButton::Right => MK_RBUTTON,
        MouseButton::Middle => MK_MBUTTON,
    }
}

fn down_message(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => WM_LBUTTONDOWN,
        MouseButton::Right => WM_RBUTTONDOWN,
        MouseButton::Middle => WM_MBUTTONDOWN,
    }
}

fn up_message(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => WM_LBUTTONUP,
        MouseButton::Right => WM_RBUTTONUP,
        MouseButton::Middle => WM_MBUTTONUP,
    }
}

/// True when this press is the second half of a double click: the same spot within the
/// system's interval. Without it the shell selects the desktop icon and refuses to open
/// it, because it never saw the press a double click is built from.
fn second_press(x: i32, y: i32) -> bool {
    let interval = unsafe { GetDoubleClickTime() }.max(1) as u64;
    let now = Instant::now();
    let mut last = LAST_PRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = *last;
    let second = matches!(previous, Some((at, px, py))
        if now.duration_since(at) <= Duration::from_millis(interval)
            && (px - x).abs() <= 4
            && (py - y).abs() <= 4);
    *last = if second { None } else { Some((now, x, y)) };
    second
}

/// Brings the window's frame forward. A posted message cannot activate a window, and an
/// inactive window ignores the click.
unsafe fn activate(target: HWND, note: &mut String) {
    let frame = GetAncestor(target, GA_ROOT);
    let frame = if frame.is_null() { target } else { frame };
    if SetForegroundWindow(frame) == FALSE {
        let _ = BringWindowToTop(frame);
    }
    note.push_str(&format!("activated {:#x}; ", frame as usize));
}

/// Posts one half of a keystroke to a window of this desktop.
///
/// The down half also posts the character, which is what text controls read; the up half
/// only closes the key. `hwnd` defaults to the window a click last landed on.
pub fn post_key(
    desktop: &str,
    hwnd: Option<isize>,
    vk: u16,
    ch: Option<char>,
    down: bool,
) -> Result<String, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || post_key_on_thread(&desktop, hwnd, vk, ch, down))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn post_key_on_thread(
    desktop: &str,
    hwnd: Option<isize>,
    vk: u16,
    ch: Option<char>,
    down: bool,
) -> Result<String, String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (target, label) = match hwnd.or_else(super::mode::focus) {
        Some(handle) => (handle as HWND, format!("{handle:#x}")),
        None => pick_target(None)?,
    };
    unsafe {
        if down {
            if PostMessageW(target, WM_KEYDOWN, vk as usize, 0) == FALSE {
                return Err(format!("PostMessage(WM_KEYDOWN) failed: {}", last_error()));
            }
            if let Some(ch) = ch {
                if PostMessageW(target, WM_CHAR, ch as usize, 0) == FALSE {
                    return Err(format!("PostMessage(WM_CHAR) failed: {}", last_error()));
                }
            }
        } else if PostMessageW(target, WM_KEYUP, vk as usize, 0) == FALSE {
            return Err(format!("PostMessage(WM_KEYUP) failed: {}", last_error()));
        }
    }
    Ok(format!("key {vk:#x} -> {label}"))
}

/// Walks from the window containing `point` down to the deepest child, the way a click does.
unsafe fn deepest_at(point: POINT) -> HWND {
    let mut current = WindowFromPoint(point);
    loop {
        if current.is_null() {
            return current;
        }
        let mut local = point;
        if ScreenToClient(current, &mut local) == FALSE {
            return current;
        }
        let child = RealChildWindowFromPoint(current, local);
        if child.is_null() || child == current {
            return current;
        }
        current = child;
    }
}

unsafe extern "system" fn first_titled(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let slot = &mut *(lparam as *mut Option<(HWND, String)>);
    if slot.is_some() {
        return FALSE;
    }
    if IsWindowVisible(hwnd) == FALSE {
        return TRUE;
    }
    let mut buf = [0u16; 256];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len <= 0 {
        return TRUE;
    }
    *slot = Some((hwnd, String::from_utf16_lossy(&buf[..len as usize])));
    FALSE
}
