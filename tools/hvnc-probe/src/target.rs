//! A classic text target on the shadow desktop, for testing input end to end.
//!
//! The window is created *on the hidden desktop* (this thread is attached there first),
//! and it is a plain Win32 window with an EDIT child - the kind of control that really
//! handles posted messages. So if text sent by `type` shows up in the edit box, the posted
//! input path works; if it does not, the path is wrong and nothing about the test target
//! could explain it.
//!
//! It also sidesteps the host's own Notepad: on Windows 11 that is the WinUI rewrite,
//! whose text area is not an EDIT control and therefore ignores posted messages entirely.

use std::io::Read;
use std::mem::zeroed;
use std::ptr;
use std::thread;

use winapi::shared::minwindef::{HINSTANCE, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::HWND;
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW,
    PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, COLOR_WINDOW, CW_USEDEFAULT,
    ES_AUTOVSCROLL, ES_MULTILINE, MSG, WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use crate::shadow_desktop::{desktop::AttachedDesktop, desktop::Desktop, last_error, wide};

/// Shows the target and pumps messages until the operator presses Enter.
pub fn run(desktop: &str) -> Result<(), String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || run_on_thread(&desktop))
        .join()
        .map_err(|_| "target thread panicked".to_owned())?
}

fn run_on_thread(desktop: &str) -> Result<(), String> {
    // Hold the desktop open for as long as the window lives: closing its last handle
    // destroys the desktop and its windows with it.
    let _keep_open = Desktop::create_or_open(crate::shadow_desktop::SHADOW_DESKTOP)?;
    // No windows on this thread before the switch: SetThreadDesktop refuses otherwise.
    let attached = AttachedDesktop::attach(desktop)?;
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(ptr::null());
        let class_name = wide("HvncProbeTarget");

        let mut wc: WNDCLASSW = zeroed();
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wc.hbrBackground = (COLOR_WINDOW + 1) as _;
        wc.lpszClassName = class_name.as_ptr();
        if RegisterClassW(&wc) == 0 {
            return Err(format!("RegisterClassW failed: {}", last_error()));
        }

        let title = wide("hvnc-probe target");
        let parent = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            600,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null_mut(),
        );
        if parent.is_null() {
            return Err(format!("CreateWindowExW(parent) failed: {}", last_error()));
        }

        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide("EDIT").as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE | ES_AUTOVSCROLL,
            0,
            0,
            880,
            560,
            parent,
            ptr::null_mut(),
            instance,
            ptr::null_mut(),
        );
        if edit.is_null() {
            return Err(format!("CreateWindowExW(edit) failed: {}", last_error()));
        }

        println!("target window is up on {}", attached.name());
        println!("  parent hwnd : {:#x}", parent as usize);
        println!("  edit hwnd   : {:#x}   <- aim `type` at this one", edit as usize);
        println!("press Enter in this terminal to close it");

        // Stopping reads the console on another thread, so the message loop stays free. The
        // handle travels as a plain integer: HWND is a raw pointer and is not Send.
        let parent_raw = parent as usize;
        thread::spawn(move || {
            let mut byte = [0u8; 1];
            let _ = std::io::stdin().read(&mut byte);
            PostMessageW(parent_raw as HWND, WM_CLOSE, 0, 0);
        });

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE | WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}