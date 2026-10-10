//! Shared helpers.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use winapi::um::errhandlingapi::GetLastError;

/// NUL-terminated UTF-16, the form every `*W` API here expects.
pub fn wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
    v.push(0);
    v
}

pub fn last_error() -> String {
    let code = unsafe { GetLastError() };
    format!("error {code} (0x{code:08X})")
}