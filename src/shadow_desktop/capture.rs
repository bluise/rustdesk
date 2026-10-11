//! Capturing a desktop that is not on screen.
//!
//! `GetDC(NULL)` resolves to the desktop of the calling *thread*, which is why every read
//! happens on a thread switched there with `SetThreadDesktop`. That thread must own no
//! windows and no hooks, so each capture run gets a fresh thread.
//!
//! A hidden desktop cannot be read the way a visible one can: BitBlt off its desktop DC
//! comes back with ERROR_ACCESS_DENIED (5), and the frame is uniformly black. Window by
//! window is the way in instead - `PrintWindow` renders a window whether or not its
//! desktop is on screen, which is the same pairing the HVNC tooling uses.
//!
//! [`Method::all`] is tried in order by [`capture_frames`] (first non-black frame wins)
//! and reported one by one by [`diagnose`], so a machine where one path is refused still
//! has a way to say which one works.

use std::mem::{size_of, zeroed};
use std::ptr;
use std::thread;
use std::time::Duration;
use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, TRUE, UINT};
use winapi::shared::windef::{HBITMAP, HDC, HWND, RECT};
use winapi::um::wingdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDCW, DeleteDC, DeleteObject,
    GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    SRCCOPY,
};
use winapi::um::winuser::{
    EnumWindows, GetDC, GetSystemMetrics, GetWindowRect, IsWindowVisible, PrintWindow, ReleaseDC,
    SM_CXSCREEN, SM_CYSCREEN,
};

use super::desktop::AttachedDesktop;
use super::{last_error, wide};

/// `PW_RENDERFULLCONTENT`: also ask for content drawn by DirectComposition.
const PW_RENDERFULLCONTENT: UINT = 0x0000_0002;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GetDC(NULL)` then BitBlt, plain SRCCOPY.
    DesktopDcBlt,
    /// The same with CAPTUREBLT, which also takes layered windows.
    DesktopDcBltLayered,
    /// A DC for the "DISPLAY" device instead of the screen DC.
    DisplayDcBlt,
    /// `PrintWindow` per top-level window, drawn onto the desktop canvas.
    PrintWindow,
}

impl Method {
    pub fn all() -> [Method; 4] {
        [
            Method::PrintWindow,
            Method::DesktopDcBltLayered,
            Method::DesktopDcBlt,
            Method::DisplayDcBlt,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Method::DesktopDcBlt => "getdc+bitblt",
            Method::DesktopDcBltLayered => "getdc+bitblt+captureblt",
            Method::DisplayDcBlt => "createdc-DISPLAY+bitblt",
            Method::PrintWindow => "printwindow-per-window",
        }
    }
}

/// One frame, top-down BGRA - the same layout `scrap` hands to the encoder.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// What one method managed to produce, for [`diagnose`].
pub struct MethodReport {
    pub method: Method,
    pub ok: bool,
    pub note: String,
    pub frame: Option<Frame>,
}

/// Grabs `count` frames of `desktop`, `interval` apart, using the first method that
/// produces a picture at all.
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

/// Tries every method once and reports what each one gave, without guessing.
pub fn diagnose(desktop: &str, size: Option<(i32, i32)>) -> Result<Vec<MethodReport>, String> {
    let desktop = desktop.to_owned();
    thread::spawn(move || diagnose_on_thread(&desktop, size))
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
    let (width, height) = desktop_size(size)?;

    let mut chosen = None;
    let mut refused = Vec::new();
    for method in Method::all() {
        match capture_one(method, width, height) {
            Ok(frame) if !is_black(&frame) => {
                chosen = Some(method);
                break;
            }
            Ok(_) => refused.push(format!("{}: came back black", method.label())),
            Err(e) => refused.push(format!("{}: {e}", method.label())),
        }
    }
    let method = match chosen {
        Some(method) => method,
        None => {
            return Err(format!(
                "no capture method produced a picture ({})",
                refused.join("; ")
            ))
        }
    };
    eprintln!("[shadow_desktop] capturing with {}", method.label());

    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        frames.push(capture_one(method, width, height)?);
        if i + 1 < count {
            thread::sleep(interval);
        }
    }
    Ok(frames)
}

fn diagnose_on_thread(desktop: &str, size: Option<(i32, i32)>) -> Result<Vec<MethodReport>, String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (width, height) = desktop_size(size)?;
    let mut reports = Vec::new();
    for method in Method::all() {
        match capture_one(method, width, height) {
            Ok(frame) => {
                let black = is_black(&frame);
                reports.push(MethodReport {
                    method,
                    ok: !black,
                    note: if black {
                        "frame came back uniformly black".to_owned()
                    } else {
                        format!("{}x{} frame with content", frame.width, frame.height)
                    },
                    frame: Some(frame),
                });
            }
            Err(e) => reports.push(MethodReport {
                method,
                ok: false,
                note: e,
                frame: None,
            }),
        }
    }
    Ok(reports)
}

fn desktop_size(size: Option<(i32, i32)>) -> Result<(i32, i32), String> {
    let (width, height) =
        size.unwrap_or(unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) });
    if width <= 0 || height <= 0 {
        return Err(format!("bad capture size {width}x{height}"));
    }
    Ok((width, height))
}

fn capture_one(method: Method, width: i32, height: i32) -> Result<Frame, String> {
    match method {
        Method::DesktopDcBlt => blt_from_screen_dc(width, height, SRCCOPY, false),
        Method::DesktopDcBltLayered => blt_from_screen_dc(width, height, SRCCOPY | CAPTUREBLT, false),
        Method::DisplayDcBlt => blt_from_screen_dc(width, height, SRCCOPY, true),
        Method::PrintWindow => print_window_frame(width, height),
    }
}

fn blt_from_screen_dc(width: i32, height: i32, rop: u32, via_device: bool) -> Result<Frame, String> {
    unsafe {
        let screen_dc: HDC = if via_device {
            CreateDCW(wide("DISPLAY").as_ptr(), ptr::null(), ptr::null(), ptr::null())
        } else {
            GetDC(ptr::null_mut())
        };
        if screen_dc.is_null() {
            return Err(format!("no screen DC for this desktop: {}", last_error()));
        }
        let result = blt_to_frame(screen_dc, width, height, rop);
        if via_device {
            DeleteDC(screen_dc);
        } else {
            ReleaseDC(ptr::null_mut(), screen_dc);
        }
        result
    }
}

/// Copies `screen_dc` into a bitmap and reads it back as a frame.
fn blt_to_frame(screen_dc: HDC, width: i32, height: i32, rop: u32) -> Result<Frame, String> {
    unsafe {
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bmp: HBITMAP = CreateCompatibleBitmap(screen_dc, width, height);
        if mem_dc.is_null() || bmp.is_null() {
            if !mem_dc.is_null() {
                DeleteDC(mem_dc);
            }
            if !bmp.is_null() {
                DeleteObject(bmp as _);
            }
            return Err(format!(
                "could not build a {width}x{height} buffer: {}",
                last_error()
            ));
        }
        let previous = SelectObject(mem_dc, bmp as _);
        // Unlike the visible desktop, every one of these calls can be refused outright
        // here, so the failure is reported instead of quietly leaving a black frame.
        let blt = BitBlt(mem_dc, 0, 0, width, height, screen_dc, 0, 0, rop);
        let result = if blt == FALSE {
            Err(format!("BitBlt failed: {}", last_error()))
        } else {
            read_frame(mem_dc, bmp, width, height)
        };
        SelectObject(mem_dc, previous);
        DeleteDC(mem_dc);
        DeleteObject(bmp as _);
        result
    }
}

/// Draws every top-level window of this desktop onto one canvas with PrintWindow.
///
/// A window renders through PrintWindow whether or not its desktop is on screen, which is
/// the only path that worked on a real machine here.
fn print_window_frame(width: i32, height: i32) -> Result<Frame, String> {
    unsafe {
        let screen_dc = GetDC(ptr::null_mut());
        if screen_dc.is_null() {
            return Err(format!("no screen DC for this desktop: {}", last_error()));
        }
        let canvas_dc = CreateCompatibleDC(screen_dc);
        let canvas_bmp: HBITMAP = CreateCompatibleBitmap(screen_dc, width, height);
        if canvas_dc.is_null() || canvas_bmp.is_null() {
            if !canvas_dc.is_null() {
                DeleteDC(canvas_dc);
            }
            if !canvas_bmp.is_null() {
                DeleteObject(canvas_bmp as _);
            }
            ReleaseDC(ptr::null_mut(), screen_dc);
            return Err(format!(
                "could not build a {width}x{height} canvas: {}",
                last_error()
            ));
        }
        let previous = SelectObject(canvas_dc, canvas_bmp as _);

        let mut state = Canvas {
            dc: canvas_dc,
            printed: 0,
            failed: 0,
            width,
            height,
        };
        EnumWindows(Some(print_window), &mut state as *mut Canvas as LPARAM);

        let frame = read_frame(canvas_dc, canvas_bmp, width, height);
        SelectObject(canvas_dc, previous);
        DeleteDC(canvas_dc);
        DeleteObject(canvas_bmp as _);
        ReleaseDC(ptr::null_mut(), screen_dc);

        let frame = frame?;
        if state.printed == 0 {
            return Err(format!(
                "PrintWindow rendered no window ({} refused)",
                state.failed
            ));
        }
        Ok(frame)
    }
}

struct Canvas {
    dc: HDC,
    printed: usize,
    failed: usize,
    width: i32,
    height: i32,
}

unsafe extern "system" fn print_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let canvas = &mut *(lparam as *mut Canvas);
    if IsWindowVisible(hwnd) == FALSE {
        return TRUE;
    }
    let mut rect: RECT = zeroed();
    if GetWindowRect(hwnd, &mut rect) == FALSE {
        return TRUE;
    }
    let (x, y) = (rect.left, rect.top);
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    if w <= 0 || h <= 0 {
        return TRUE;
    }
    // Off-desktop windows are simply clipped; the canvas keeps them out of frame.
    if x >= canvas.width || y >= canvas.height || x + w <= 0 || y + h <= 0 {
        return TRUE;
    }

    let dc = CreateCompatibleDC(canvas.dc);
    let bmp: HBITMAP = CreateCompatibleBitmap(canvas.dc, w, h);
    if dc.is_null() || bmp.is_null() {
        if !dc.is_null() {
            DeleteDC(dc);
        }
        if !bmp.is_null() {
            DeleteObject(bmp as _);
        }
        canvas.failed += 1;
        return TRUE;
    }
    let previous = SelectObject(dc, bmp as _);
    // Some windows ignore the full-content flag, so it is retried without it rather than
    // lost.
    if PrintWindow(hwnd, dc, PW_RENDERFULLCONTENT) == FALSE && PrintWindow(hwnd, dc, 0) == FALSE {
        canvas.failed += 1;
    } else if BitBlt(canvas.dc, x, y, w, h, dc, 0, 0, SRCCOPY) == FALSE {
        eprintln!("[shadow_desktop] compositing failed: {}", last_error());
        canvas.failed += 1;
    } else {
        canvas.printed += 1;
    }
    SelectObject(dc, previous);
    DeleteObject(bmp as _);
    DeleteDC(dc);
    TRUE
}

/// A cheap look at whether anything was drawn at all: a desktop whose capture path is
/// refused comes back uniformly black, which must not be mistaken for a picture.
fn is_black(frame: &Frame) -> bool {
    frame
        .bgra
        .chunks_exact(4)
        .step_by(97)
        .all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8)
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