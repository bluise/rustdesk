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
//! The Win32 primitives here were proven first with `tools/hvnc-probe`, which builds the
//! same calls as a standalone exe; keep the two in step while the mode is brought up.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

pub mod desktop;
pub mod launch;

/// The desktop everything in this mode runs on.
pub const SHADOW_DESKTOP: &str = "RustDeskShadow";

/// NUL-terminated UTF-16, the form every `*W` API here expects.
pub(crate) fn wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
    v.push(0);
    v
}