//! Start a program on the shadow desktop.
//!
//! `STARTUPINFOW::lpDesktop` decides which desktop the new process gets, so the program
//! needs no modification and never lands on the visible desktop. Children inherit the
//! desktop, which is why launching one explorer or shell is enough to give the remote
//! side somewhere to work.

use std::mem::{size_of, zeroed};
use std::ptr;
use winapi::shared::minwindef::FALSE;
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use winapi::um::winbase::CREATE_NEW_CONSOLE;

use super::wide;

/// Starts `program` on `qualified_desktop` (`WinSta0\RustDeskShadow`), returning its pid.
pub fn spawn_on_desktop(
    qualified_desktop: &str,
    program: &str,
    args: &[String],
) -> Result<u32, String> {
    // CreateProcessW is allowed to write into the command line buffer.
    let mut desktop = wide(qualified_desktop);
    let mut cmdline = wide(command_line(program, args));
    unsafe {
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        // A bare "RustDeskShadow" would be resolved against the child's window station,
        // so the qualified name is what actually pins it down.
        si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = zeroed();
        let ok = CreateProcessW(
            ptr::null(),
            cmdline.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            FALSE,
            CREATE_NEW_CONSOLE,
            ptr::null_mut(),
            ptr::null(),
            &mut si,
            &mut pi,
        );
        if ok == FALSE {
            let code = winapi::um::errhandlingapi::GetLastError();
            return Err(format!(
                "CreateProcessW({program}) on {qualified_desktop} failed: error {code} (0x{code:08X})"
            ));
        }
        CloseHandle(pi.hThread);
        let pid = pi.dwProcessId;
        CloseHandle(pi.hProcess);
        Ok(pid)
    }
}

fn command_line(program: &str, args: &[String]) -> String {
    let mut parts = vec![quote(program)];
    parts.extend(args.iter().map(|a| quote(a)));
    parts.join(" ")
}

/// The quoting rules CreateProcessW itself parses.
fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in s.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            c => {
                backslashes = 0;
                out.push(c);
            }
        }
    }
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}