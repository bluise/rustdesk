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
use std::thread;
use std::time::Duration;
use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, TRUE, UINT};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::winuser::{
    BringWindowToTop, ClientToScreen, EnumWindows, GetClientRect, GetForegroundWindow,
    GetSystemMetrics, GetWindowTextW, IsWindowVisible, PostMessageW, RealChildWindowFromPoint,
    ScreenToClient, SendInput, SetFocus, SetForegroundWindow, WindowFromPoint, INPUT,
    INPUT_KEYBOARD, INPUT_MOUSE, INPUT_u, KEYBDINPUT, KEYEVENTF_KEYUP, LPINPUT,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT,
    SM_CXSCREEN, SM_CYSCREEN, WM_CHAR,
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
                send_mouse(x, y, 0, screen_w, screen_h)?;
            }
            Action::LeftClick { x, y } => {
                send_mouse(x, y, MOUSEEVENTF_LEFTDOWN, screen_w, screen_h)?;
                thread::sleep(Duration::from_millis(20));
                send_mouse(x, y, MOUSEEVENTF_LEFTUP, screen_w, screen_h)?;
            }
            Action::KeyDown { vk } => send_key(vk, false)?,
            Action::KeyUp { vk } => send_key(vk, true)?,
        }
        thread::sleep(Duration::from_millis(15));
    }
    Ok(())
}

fn send_mouse(x: i32, y: i32, flags: u32, screen_w: i32, screen_h: i32) -> Result<(), String> {
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
                mouseData: 0,
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
                let suffix = if target == frame { "" } else { " (child control)" };
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