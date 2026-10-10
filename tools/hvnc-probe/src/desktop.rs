//! Window station and desktop plumbing.
//!
//! A hidden desktop is only useful when it lives in the *interactive* window
//! station (`WinSta0`): a desktop created while the process sits in a service
//! window station is not backed by the display and cannot be captured. So every
//! entry point here switches the calling process to WinSta0, does the work, and
//! restores the previous station.

use std::ptr;
use winapi::ctypes::c_void;
use winapi::shared::minwindef::{DWORD, FALSE};
use winapi::shared::windef::HDESK;
use winapi::um::winuser::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, GetProcessWindowStation,
    GetUserObjectInformationW, OpenDesktopW, OpenWindowStationW, SetProcessWindowStation,
    SetThreadDesktop, UOI_NAME,
};

use crate::util::{last_error, wide};

/// MSDN `DESKTOP_ALL_ACCESS`; spelled out instead of relying on which winapi
/// module re-exports it.
pub const DESKTOP_ALL_ACCESS: DWORD = 0x000F_01FF;
/// MSDN `WINSTA_ALL_ACCESS`.
pub const WINSTA_ALL_ACCESS: DWORD = 0x0000_037F;

/// `WinSta0` is the interactive window station: the only one attached to the
/// display, the keyboard and the mouse.
pub const INTERACTIVE_WINSTA: &str = "WinSta0";

/// Run `f` with the calling process attached to WinSta0.
fn in_interactive_winsta<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    unsafe {
        // HWINSTA is private in winapi, so the handle type is left to inference.
        let winsta =
            OpenWindowStationW(wide(INTERACTIVE_WINSTA).as_ptr(), FALSE, WINSTA_ALL_ACCESS);
        if winsta.is_null() {
            return Err(format!(
                "OpenWindowStationW({INTERACTIVE_WINSTA}) failed: {}",
                last_error()
            ));
        }
        let previous = GetProcessWindowStation();
        if FALSE == SetProcessWindowStation(winsta) {
            let e = last_error();
            CloseWindowStation(winsta);
            return Err(format!("SetProcessWindowStation(WinSta0) failed: {e}"));
        }
        let result = f();
        SetProcessWindowStation(previous);
        CloseWindowStation(winsta);
        result
    }
}

/// An open handle to a desktop. Closing it is what removes the desktop, so the
/// type is deliberately not `Clone`.
pub struct Desktop {
    name: String,
    handle: HDESK,
}

impl Desktop {
    /// Open `name` if it already exists, otherwise create it inside WinSta0.
    pub fn create_or_open(name: &str) -> Result<Self, String> {
        if !valid_name(name) {
            return Err(format!(
                "invalid desktop name {name:?}: keep it to letters, digits, '-' and '_'"
            ));
        }
        let name_owned = name.to_owned();
        in_interactive_winsta(move || unsafe {
            let handle = open_raw(&name_owned);
            if !handle.is_null() {
                return Ok(Desktop {
                    name: name_owned,
                    handle,
                });
            }
            let handle = CreateDesktopW(
                wide(&name_owned).as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                0,
                DESKTOP_ALL_ACCESS,
                ptr::null_mut(),
            );
            if handle.is_null() {
                return Err(format!("CreateDesktopW({name_owned}) failed: {}", last_error()));
            }
            Ok(Desktop {
                name: name_owned,
                handle,
            })
        })
    }

    /// Open an existing desktop, without creating one.
    pub fn open(name: &str) -> Result<Self, String> {
        let name_owned = name.to_owned();
        in_interactive_winsta(move || unsafe {
            let handle = open_raw(&name_owned);
            if handle.is_null() {
                Err(format!("OpenDesktopW({name_owned}) failed: {}", last_error()))
            } else {
                Ok(Desktop {
                    name: name_owned,
                    handle,
                })
            }
        })
    }

    /// The form `STARTUPINFOW::lpDesktop` wants: window station *and* desktop.
    pub fn qualified_name(&self) -> String {
        format!("{}\\{}", INTERACTIVE_WINSTA, self.name)
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            CloseDesktop(self.handle);
        }
    }
}

/// A desktop handle that is also attached to the calling thread.
///
/// Everything that talks to the desktop directly - capture and input - has to go
/// through this, because both `GetDC(NULL)` and `SendInput` act on the desktop of
/// the calling thread.
pub struct AttachedDesktop {
    handle: HDESK,
    name: String,
}

impl AttachedDesktop {
    /// Open `desktop` (bare or already `WinSta0\name`) and attach this thread.
    ///
    /// The thread must own no windows and no hooks, so callers run this on a
    /// fresh thread rather than on a UI thread.
    pub fn attach(desktop: &str) -> Result<Self, String> {
        ensure_process_in_interactive_winsta()?;
        let qualified = qualified(desktop);
        unsafe {
            let handle = OpenDesktopW(wide(&qualified).as_ptr(), 0, FALSE, DESKTOP_ALL_ACCESS);
            if handle.is_null() {
                return Err(format!(
                    "OpenDesktopW({qualified}) failed: {}",
                    last_error()
                ));
            }
            if FALSE == SetThreadDesktop(handle) {
                let e = last_error();
                CloseDesktop(handle);
                return Err(format!(
                    "SetThreadDesktop({qualified}) failed: {e} (does this thread already own a window?)"
                ));
            }
            Ok(AttachedDesktop {
                handle,
                name: qualified,
            })
        }
    }

    pub fn handle(&self) -> HDESK {
        self.handle
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for AttachedDesktop {
    fn drop(&mut self) {
        unsafe {
            CloseDesktop(self.handle);
        }
    }
}

/// `WinSta0\name`, unless the caller qualified it already.
fn qualified(desktop: &str) -> String {
    if desktop.contains('\\') {
        desktop.to_owned()
    } else {
        format!("{}\\{}", INTERACTIVE_WINSTA, desktop)
    }
}

/// Move the calling process into WinSta0 if it is not there yet.
///
/// SetThreadDesktop only accepts a desktop belonging to the *process's* window
/// station. An ordinary in-session process is already in WinSta0 and this returns
/// immediately; a service (session 0) process has to be moved, and then has to
/// stay moved for the rest of its life - hence the handle is deliberately leaked.
pub fn ensure_process_in_interactive_winsta() -> Result<(), String> {
    unsafe {
        let current = GetProcessWindowStation();
        if !current.is_null()
            && object_name(current as *mut c_void).eq_ignore_ascii_case(INTERACTIVE_WINSTA)
        {
            return Ok(());
        }
        // winapi keeps the HWINSTA alias private, so the handle is left to
        // inference and only ever touched through the API calls.
        let winsta = OpenWindowStationW(wide(INTERACTIVE_WINSTA).as_ptr(), FALSE, WINSTA_ALL_ACCESS);
        if winsta.is_null() {
            return Err(format!(
                "OpenWindowStationW({INTERACTIVE_WINSTA}) failed: {}",
                last_error()
            ));
        }
        if FALSE == SetProcessWindowStation(winsta) {
            let e = last_error();
            CloseWindowStation(winsta);
            return Err(format!("SetProcessWindowStation(WinSta0) failed: {e}"));
        }
        Ok(())
    }
}

/// The kernel object name behind a handle (`WinSta0` for the interactive station,
/// `Service-0x0-...$` for a service one).
unsafe fn object_name(handle: *mut c_void) -> String {
    let mut buf = [0u16; 128];
    let mut needed: DWORD = 0;
    if FALSE
        == GetUserObjectInformationW(
            handle as _,
            UOI_NAME as i32,
            buf.as_mut_ptr() as _,
            (buf.len() * std::mem::size_of::<u16>()) as DWORD,
            &mut needed,
        )
    {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
        .trim_end_matches('\\')
        .to_owned()
}

unsafe fn open_raw(name: &str) -> HDESK {
    OpenDesktopW(wide(name).as_ptr(), 0, FALSE, DESKTOP_ALL_ACCESS)
}

/// Desktop names are also used as object names inside the window station, so
/// keep them boring instead of producing a confusing failure deep in Win32.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}