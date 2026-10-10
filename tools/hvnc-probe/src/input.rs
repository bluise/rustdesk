//! SendInput into a desktop that is not the input desktop.
//!
//! Windows routes injected input to the desktop the calling thread is attached
//! to, so - exactly like capture - this runs on a thread of its own that has
//! been switched with SetThreadDesktop and owns no windows.
//!
//! MSDN notes that SendInput is subject to UIPI: input only reaches windows of
//! an equal or lower integrity level, and a blocked call reports neither an
//! error code nor a shorter-than-expected return. Every call is checked.

use std::mem::{size_of, zeroed};
use std::thread;
use std::time::Duration;
use winapi::shared::minwindef::UINT;
use winapi::um::winuser::{
    GetSystemMetrics, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, INPUT_u, KEYBDINPUT,
    KEYEVENTF_KEYUP, LPINPUT, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEINPUT, SM_CXSCREEN, SM_CYSCREEN,
};

use crate::desktop::AttachedDesktop;

/// Marks our own events, the way enigo tags its input: the peer-side hook can
/// then tell injected input from a real user, which is what privacy mode does.
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
    let (screen_w, screen_h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
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
                // Scan-code injection would be needed for DirectInput games;
                // virtual keys are enough for ordinary windows.
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