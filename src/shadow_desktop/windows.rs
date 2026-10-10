//! List the windows that live on a hidden desktop.
//!
//! Needed before anything can be steered: input focus has to be set on a window that
//! really exists on that desktop, and titles are the only way to tell which one. It is
//! also the cheapest way to answer "is anything actually running over there?" while
//! bringing the mechanism up.

use std::thread;
use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, MAX_PATH, TRUE};
use winapi::shared::windef::{HWND, RECT};
use winapi::um::winuser::{
    EnumDesktopWindows, GetClassNameW, GetWindowRect, GetWindowTextW, IsWindowVisible,
};

use super::desktop::AttachedDesktop;
use super::last_error;

#[derive(Debug)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub class: String,
    pub visible: bool,
    pub rect: (i32, i32, i32, i32),
}

pub fn list(desktop: &str) -> Result<Vec<WindowInfo>, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || list_on_thread(&desktop))
        .join()
        .map_err(|_| "enumeration thread panicked".to_owned())?
}

fn list_on_thread(desktop: &str) -> Result<Vec<WindowInfo>, String> {
    // Enumeration and the title reads below both talk to that desktop's windows.
    let attached = AttachedDesktop::attach(desktop)?;
    let mut windows: Vec<WindowInfo> = Vec::new();
    let ok = unsafe {
        EnumDesktopWindows(
            attached.handle(),
            Some(collect),
            &mut windows as *mut Vec<WindowInfo> as LPARAM,
        )
    };
    if ok == FALSE {
        return Err(format!(
            "EnumDesktopWindows({}) failed: {}",
            attached.name(),
            last_error()
        ));
    }
    Ok(windows)
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam as *mut Vec<WindowInfo>);
    if windows.len() >= 512 {
        return TRUE;
    }
    let mut rect: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut rect);
    windows.push(WindowInfo {
        hwnd: hwnd as isize,
        title: read_string(|buf, cap| GetWindowTextW(hwnd, buf, cap)),
        class: read_string(|buf, cap| GetClassNameW(hwnd, buf, cap)),
        visible: IsWindowVisible(hwnd) != FALSE,
        rect: (rect.left, rect.top, rect.right, rect.bottom),
    });
    TRUE
}

unsafe fn read_string(f: impl Fn(*mut u16, i32) -> i32) -> String {
    let mut buf = [0u16; MAX_PATH];
    let len = f(buf.as_mut_ptr(), buf.len() as i32);
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}