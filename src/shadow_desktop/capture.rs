//! GDI capture of a desktop that is not on screen.
//!
//! `GetDC(NULL)` resolves to the desktop that the calling *thread* is attached to, so
//! the shadow desktop can only be read from a thread switched there with
//! `SetThreadDesktop`. Such a thread must own no windows and no hooks, which is why every
//! capture run gets a thread of its own.

use std::mem::{size_of, zeroed};
use std::ptr;
use std::thread;
use std::time::Duration;
use winapi::shared::minwindef::FALSE;
use winapi::shared::windef::{HBITMAP, HDC};
use winapi::um::wingdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
};
use winapi::um::winuser::{GetDC, GetSystemMetrics, ReleaseDC, SM_CXSCREEN, SM_CYSCREEN};

use super::desktop::AttachedDesktop;
use super::last_error;

/// One frame, top-down BGRA - the same layout `scrap` hands to the encoder.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Grabs `count` frames of `desktop`, `interval` apart, from one attached thread.
///
/// `size` defaults to the session's primary resolution; pass it to override what Win32
/// reports for that desktop.
pub fn capture_frames(
    desktop: &str,
    count: usize,
    interval: Duration,
    size: Option<(i32, i32)>,
) -> Result<Vec<Frame>, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || capture_on_thread(&desktop, count, interval, size))
        .join()
        .map_err(|_| "capture thread panicked".to_owned())?
}

fn capture_on_thread(
    desktop: &str,
    count: usize,
    interval: Duration,
    size: Option<(i32, i32)>,
) -> Result<Vec<Frame>, String> {
    // Kept alive for the whole run: dropping it closes the desktop handle.
    let _attached = AttachedDesktop::attach(desktop)?;
    unsafe {
        let screen_dc: HDC = GetDC(ptr::null_mut());
        if screen_dc.is_null() {
            return Err(format!("GetDC(NULL) failed: {}", last_error()));
        }
        let (width, height) =
            size.unwrap_or((GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)));
        if width <= 0 || height <= 0 {
            ReleaseDC(ptr::null_mut(), screen_dc);
            return Err(format!("bad capture size {width}x{height}"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bmp: HBITMAP = CreateCompatibleBitmap(screen_dc, width, height);
        if mem_dc.is_null() || bmp.is_null() {
            let e = last_error();
            if !mem_dc.is_null() {
                DeleteDC(mem_dc);
            }
            if !bmp.is_null() {
                DeleteObject(bmp as _);
            }
            ReleaseDC(ptr::null_mut(), screen_dc);
            return Err(format!(
                "could not build a {width}x{height} capture buffer: {e}"
            ));
        }

        let previous = SelectObject(mem_dc, bmp as _);
        let mut frames = Vec::with_capacity(count);
        for i in 0..count {
            if FALSE == BitBlt(mem_dc, 0, 0, width, height, screen_dc, 0, 0, SRCCOPY | CAPTUREBLT) {
                // Not fatal: a hidden desktop can sit unchanged between frames.
                eprintln!("[shadow_desktop] BitBlt failed: {}", last_error());
            }
            frames.push(read_frame(mem_dc, bmp, width, height)?);
            if i + 1 < count {
                thread::sleep(interval);
            }
        }

        SelectObject(mem_dc, previous);
        DeleteDC(mem_dc);
        DeleteObject(bmp as _);
        ReleaseDC(ptr::null_mut(), screen_dc);
        Ok(frames)
    }
}

fn read_frame(dc: HDC, bmp: HBITMAP, width: i32, height: i32) -> Result<Frame, String> {
    unsafe {
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        // A negative height asks for top-down rows, matching our frame layout.
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;

        let mut bgra = vec![0u8; width as usize * height as usize * 4];
        let copied = GetDIBits(
            dc,
            bmp,
            0,
            height as u32,
            bgra.as_mut_ptr() as _,
            &mut info,
            DIB_RGB_COLORS,
        );
        if copied == 0 {
            return Err(format!("GetDIBits failed: {}", last_error()));
        }
        Ok(Frame {
            width: width as usize,
            height: height as usize,
            bgra,
        })
    }
}