//! Shadow desktop: a hidden workspace the remote side drives, shown to it through a real
//! monitor while the machine's own screens are dark.
//!
//! Three pieces make that work, and nothing else in the host has to know about it:
//!
//! * [`desktop`] opens the hidden desktop inside the interactive window station;
//! * [`launch`] puts a shell on it, so there is something to work with;
//! * [`mode`] plugs in a virtual display, takes the physical ones off screen, and switches
//!   the session to the hidden desktop.
//!
//! That last switch is what joins the two halves. A monitor shows the session's active
//! desktop, so the new virtual display shows the hidden workspace - which is why capture,
//! input, the display list and the switch-screen message are all the ordinary paths, and
//! the remote side is looking at a real screen through the platform capturer rather than at
//! a capture path of this module's own.
//!
//! Only compiled with the `shadow-desktop` feature, and inert without it.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

pub mod desktop;
pub mod launch;
pub mod mode;
pub mod windows;

/// The desktop everything in this mode runs on.
pub const SHADOW_DESKTOP: &str = "RustDeskShadow";

/// NUL-terminated UTF-16, the form every `*W` API here expects.
pub(crate) fn wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
    v.push(0);
    v
}

pub(crate) fn last_error() -> String {
    let code = unsafe { winapi::um::errhandlingapi::GetLastError() };
    format!("error {code} (0x{code:08X})")
}