//! Shadow desktop: a hidden Windows desktop the remote side can drive while the
//! machine's own screen shows nothing.
//!
//! Only compiled with the `shadow-desktop` feature, and every existing capture, input
//! and launch path stays exactly as it is without it - this is an addition, not a
//! rewrite of the host.
//!
//! A desktop only draws and takes input while it lives in the *interactive* window
//! station (`WinSta0`), so the mode has to run inside the user's own session; a service
//! (session 0) process has to move itself there first, which [`desktop`] does.
//!
//! Everything here is std + winapi on purpose: `tools/hvnc-probe` compiles these very
//! files through `#[path]` and is the local compile check and the on-machine proof of
//! the mechanism, so an extra dependency would break that.
#![allow(dead_code)] // the mode is not wired into the host yet; drop this once it is.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

pub mod capture;
pub mod desktop;
pub mod input;
pub mod launch;
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