//! Input into a desktop that is not the input desktop.
//!
//! Windows routes injected input to the desktop the calling thread is attached to, so -
//! exactly like capture - this runs on a thread that has been switched with
//! `SetThreadDesktop` and owns no windows.
//!
//! That thread is long-lived rather than one per event, because two of the things input
//! needs only work while it is held:
//!
//! * `AttachThreadInput` joins the target's input queue to ours, which is the only way
//!   `SetKeyboardState` can reach it - and a posted message never updates the key state by
//!   itself, so without the join `GetKeyState` inside the target reports every modifier as
//!   up and Ctrl+C arrives as a plain `c`;
//! * `SetFocus` only crosses to another thread's window inside a joined queue, which is what
//!   gives a hidden desktop's window a real focus instead of only a remembered one.
//!
//! Every behaviour claimed above was measured on the machine this was written on with
//! `probe/winprobe.c` and `probe/03_posttest.py`, which drive the same messages into an
//! ordinary Win32 EDIT and read back both the messages it received and the text it holds.

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use winapi::shared::minwindef::{BOOL, DWORD, FALSE, LPARAM, TRUE, UINT};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::processthreadsapi::GetCurrentThreadId;
use winapi::um::winuser::{
    AttachThreadInput, BringWindowToTop, ClientToScreen, EnumWindows, GetAncestor, GetClassNameW,
    GetClientRect, GetDoubleClickTime, GetForegroundWindow, GetSystemMetrics, GetTopWindow,
    GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, INPUT_u,
    IsIconic, IsWindow, IsWindowVisible, IsZoomed, MapVirtualKeyW, PostMessageW,
    RealChildWindowFromPoint,
    ScreenToClient, SendInput, SendMessageTimeoutW, SetActiveWindow, SetFocus,
    SetForegroundWindow, SetKeyboardState, SetWindowPos, WindowFromPoint, GA_ROOT, GW_HWNDNEXT,
    INPUT, INPUT_KEYBOARD,
    INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_KEYUP, LPINPUT, MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT,
    SM_CXSCREEN, SM_CYSCREEN, WHEEL_DELTA, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP, XBUTTON1, XBUTTON2,
};

use super::desktop::AttachedDesktop;
use super::last_error;

/// Marks our own events, the way enigo tags its input: the peer-side hook can then tell
/// injected input from a real user, which is what privacy mode does.
const INPUT_TAG: usize = 0x5244_534B; // "RDSK"

/// The virtual keys the target is given as held, taken from the state the protocol reports.
/// The state table is indexed by virtual key and bit 0x80 means "down".
const VK_SHIFT: u8 = 0x10;
const VK_CONTROL: u8 = 0x11;
const VK_MENU: u8 = 0x12;
const KEY_DOWN: u8 = 0x80;

/// `MAPVK_VK_TO_VSC`: the scan code for a virtual key. Spelled out because the winapi crate
/// keeps this one behind a feature the build does not enable.
const MAPVK_VK_TO_VSC: u32 = 0;

/// The non-client messages. Windows sends these instead of `WM_LBUTTON*` for a click outside
/// the client area, and they carry a hit-test code rather than a position alone.
///
/// Nothing in the posted path produced them, and that is the whole of "the close box does
/// nothing": the click was posted as a click on the contents, at coordinates that are outside
/// the contents, and a window ignores a client click that is not in its client area.
const WM_NCHITTEST: u32 = 0x0084;
const WM_NCMOUSEMOVE: u32 = 0x00A0;
const WM_SYSCOMMAND: u32 = 0x0112;

/// The parts of a frame a click can land on. `DefWindowProc` answers `WM_NCHITTEST` with one
/// of these, which is how the frame itself says where its caption buttons are - they are
/// per-monitor sizes and not worth recomputing.
const HTNOWHERE: i32 = 0;
const HTCLIENT: i32 = 1;
const HTCAPTION: i32 = 2;
const HTSYSMENU: i32 = 3;
const HTGROWBOX: i32 = 4;
const HTMENU: i32 = 5;
const HTHSCROLL: i32 = 6;
const HTVSCROLL: i32 = 7;
const HTMINBUTTON: i32 = 8;
const HTMAXBUTTON: i32 = 9;
const HTLEFT: i32 = 10;
const HTRIGHT: i32 = 11;
const HTTOP: i32 = 12;
const HTTOPLEFT: i32 = 13;
const HTTOPRIGHT: i32 = 14;
const HTBOTTOM: i32 = 15;
const HTBOTTOMLEFT: i32 = 16;
const HTBOTTOMRIGHT: i32 = 17;
const HTBORDER: i32 = 18;
const HTCLOSE: i32 = 20;
const HTHELP: i32 = 21;

/// The commands the frame's own boxes stand for.
const SC_CLOSE: usize = 0xF060;
const SC_MINIMIZE: usize = 0xF020;
const SC_MAXIMIZE: usize = 0xF030;
const SC_RESTORE: usize = 0xF120;
/// `HTBORDER` is the thin frame of a window that cannot be resized; it is listed with the
/// resize edges because that is the only reading of it that does anything.
const RESIZE_EDGES: [i32; 9] = [
    HTGROWBOX, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT,
];

/// `SendMessageTimeout`: give up on a window that is not answering instead of blocking the
/// input thread on it. `SMTO_BLOCK` keeps other jobs out while the answer is pending, so the
/// hit code cannot be taken from a window the queue has already moved on from.
const SMTO_BLOCK: u32 = 0x0001;
const SMTO_ABORTIFHUNG: u32 = 0x0002;

/// `SetWindowPos` flags. The one that is absent matters as much: leaving `SWP_NOZORDER` out
/// is what brings the window to the front, the way dragging one does.
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOACTIVATE: u32 = 0x0010;

/// Frame styles and metrics, for reading a caption back when the window will not hit test
/// itself.
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const SM_CXSIZE: i32 = 30;
const SM_CXSIZEFRAME: i32 = 32;
const SM_CXPADDEDBORDER: i32 = 92;

/// `WS_CAPTION`, the frame style that says a window owns a caption.
const WS_CAPTION: u32 = 0x00C0_0000;

/// The class of a context menu. A menu drives itself from a modal loop and is already in
/// front; activating it takes its input state away, which closes the menu without ever
/// running the item that was clicked.
const MENU_CLASS: &str = "#32768";

/// How far down from a window's top edge a caption box can be, in pixels. This is asked for
/// explicitly rather than taken from a metric because Win11 draws the caption of an
/// extended-frame window - like File Explorer - 48 pixels tall, taller than the classic
/// `SM_CYCAPTION`, and the boxes fill that band top to bottom.
const CAPTION_BAND_HEIGHT: i32 = 48;

/// A window that is not really there for a click: the DWM overlays parked over a normal
/// window's caption boxes, tool tips, the input-method UI, and the rest. `WindowFromPoint`
/// still reports them, but a real mouse passes through them to the window underneath.
const WS_EX_TRANSPARENT: i32 = 0x0000_0020;
const WS_EX_NOACTIVATE: i32 = 0x0800_0000;

/// The DWM overlay classes known by name, in case a build leaves the style flags off.
const DWM_SCAFFOLDING: &str = "TITLE_BAR_SCAFFOLDING_WINDOW_CLASS";
const DWM_DESKTOP_BRIDGE: &str = "Microsoft.UI.Content.DesktopChildSiteBridge";

/// `SetWindowPos` band levers. A hidden desktop has no foreground process to grant the raise,
/// so a window is bounced through the topmost band and back, which re-orders it above every
/// ordinary window without leaving it stuck on top.
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;

#[derive(Debug, Clone, Copy)]
pub enum Action {
    MouseMove { x: i32, y: i32 },
    LeftClick { x: i32, y: i32 },
    KeyDown { vk: u16 },
    KeyUp { vk: u16 },
}

/// Plays `actions` on `desktop`, in order, with `SendInput`.
///
/// Kept for the probe tool, which uses it to show what `SendInput` does and does not reach
/// on a desktop that is not the input one. The host uses the posted paths below instead.
pub fn send(desktop: &str, actions: &[Action]) -> Result<(), String> {
    let desktop = desktop.to_owned();
    let actions = actions.to_vec();
    thread::spawn(move || send_on_thread(&desktop, &actions))
        .join()
        .map_err(|_| "input thread panicked".to_owned())?
}

fn send_on_thread(desktop: &str, actions: &[Action]) -> Result<(), String> {
    let _attached = AttachedDesktop::attach(desktop)?;
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    for action in actions {
        match *action {
            Action::MouseMove { x, y } => {
                send_mouse(x, y, 0, 0, screen_w, screen_h)?;
            }
            Action::LeftClick { x, y } => {
                send_mouse(x, y, MOUSEEVENTF_LEFTDOWN, 0, screen_w, screen_h)?;
                thread::sleep(Duration::from_millis(20));
                send_mouse(x, y, MOUSEEVENTF_LEFTUP, 0, screen_w, screen_h)?;
            }
            Action::KeyDown { vk } => send_key(vk, false)?,
            Action::KeyUp { vk } => send_key(vk, true)?,
        }
        thread::sleep(Duration::from_millis(15));
    }
    Ok(())
}

fn send_mouse(
    x: i32,
    y: i32,
    flags: u32,
    wheel: i32,
    screen_w: i32,
    screen_h: i32,
) -> Result<(), String> {
    // Absolute mouse events are normalised over the primary monitor's extent.
    let nx = normalise(x, screen_w);
    let ny = normalise(y, screen_h);
    let input = INPUT {
        type_: INPUT_MOUSE,
        u: unsafe {
            let mut u: INPUT_u = zeroed();
            *u.mi_mut() = MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: wheel as u32,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | flags,
                time: 0,
                dwExtraInfo: INPUT_TAG,
            };
            u
        },
    };
    push(input)
}

fn normalise(v: i32, span: i32) -> i32 {
    // 0..65535 over the desktop's pixel range; clamp instead of wrapping.
    if span <= 1 {
        return 0;
    }
    let v = v.clamp(0, span - 1) as i64;
    ((v * 65535) / (span - 1) as i64) as i32
}

fn send_key(vk: u16, up: bool) -> Result<(), String> {
    let input = INPUT {
        type_: INPUT_KEYBOARD,
        u: unsafe {
            let mut u: INPUT_u = zeroed();
            *u.ki_mut() = KEYBDINPUT {
                wVk: vk,
                // Scan-code injection would be needed for DirectInput games; virtual keys
                // are enough for ordinary windows.
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INPUT_TAG,
            };
            u
        },
    };
    push(input)
}

fn push(input: INPUT) -> Result<(), String> {
    let mut inputs = [input];
    let sent = unsafe {
        SendInput(
            1,
            inputs.as_mut_ptr() as LPINPUT,
            size_of::<INPUT>() as std::os::raw::c_int,
        )
    };
    if sent as UINT != 1 {
        return Err(format!(
            "SendInput delivered {sent} of 1 events (blocked by UIPI, or the desktop is gone)"
        ));
    }
    Ok(())
}

/// The button a mouse event is about.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A mouse event to inject, as the remote side asked for it.
#[derive(Debug, Clone, Copy)]
pub enum MouseAction {
    Move,
    Down(MouseButton),
    Up(MouseButton),
    /// Wheel movement in *notches*, the unit the protocol carries. Windows wants the same
    /// movement in multiples of `WHEEL_DELTA`, which is 120 - a scroll that skips the
    /// multiplication moves the view by 1/120 of a line, which is to say not at all.
    Wheel(i32),
    /// Tilt-wheel movement, same unit and same multiplication.
    HorizontalWheel(i32),
    /// Mouse 4 and 5.
    XButton { forward: bool, down: bool },
}

/// What the worker answers with.
enum Reply {
    Note(String),
    Handle(isize),
}

/// One job for the worker, with the key state to install before it runs.
///
/// The state travels with the job rather than being read by the worker, so a keystroke and
/// the click it belongs to cannot be given different modifier states by a race with the next
/// event from the network.
struct Job {
    desktop: String,
    keys: [u8; 256],
    what: What,
    reply: Sender<Result<Reply, String>>,
}

enum What {
    Mouse {
        x: i32,
        y: i32,
        action: MouseAction,
    },
    Key {
        hwnd: Option<isize>,
        vk: u16,
        ch: Option<char>,
        down: bool,
    },
    Text {
        hwnd: Option<isize>,
        text: String,
    },
    Focus {
        hwnd: Option<isize>,
    },
    WindowAt {
        x: i32,
        y: i32,
    },
}

static JOBS: OnceLock<Sender<Job>> = OnceLock::new();

fn jobs() -> &'static Sender<Job> {
    JOBS.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        thread::spawn(move || worker(rx));
        tx
    })
}

/// Runs one job and returns its answer, or the reason it could not run.
fn submit(what: What) -> Result<Reply, String> {
    let (reply, answers) = channel::<Result<Reply, String>>();
    let job = Job {
        desktop: super::SHADOW_DESKTOP.to_owned(),
        keys: super::mode::key_state(),
        what,
        reply,
    };
    jobs()
        .send(job)
        .map_err(|_| "the input worker is gone".to_owned())?;
    answers
        .recv()
        .map_err(|_| "the input worker died before answering".to_owned())?
}

fn worker(rx: Receiver<Job>) {
    // Held for the life of the thread. Capture and input both act on the calling thread's
    // desktop, and a desktop handle only stays open while someone holds it.
    let mut attached: Option<AttachedDesktop> = None;
    let mut attached_name = String::new();
    // The target thread whose input queue ours is joined to, if any.
    let mut joined: Option<DWORD> = None;

    for job in rx {
        if attached.is_none() || attached_name != job.desktop {
            match AttachedDesktop::attach(&job.desktop) {
                Ok(desktop) => {
                    // A different desktop means the old join is meaningless too.
                    joined = None;
                    attached = Some(desktop);
                    attached_name = job.desktop.clone();
                }
                Err(e) => {
                    let _ = job.reply.send(Err(e));
                    continue;
                }
            }
        }
        let mut keys = job.keys;
        let result = run(&job.what, &mut keys, &mut joined);
        let _ = job.reply.send(result);
    }
    drop(attached);
}

fn run(what: &What, keys: &mut [u8; 256], joined: &mut Option<DWORD>) -> Result<Reply, String> {
    match what {
        What::Mouse { x, y, action } => mouse(keys, joined, *x, *y, *action).map(Reply::Note),
        What::Key {
            hwnd,
            vk,
            ch,
            down,
        } => key(keys, joined, *hwnd, *vk, *ch, *down).map(Reply::Note),
        What::Text { hwnd, text } => post_text_at(keys, joined, *hwnd, text).map(Reply::Note),
        What::Focus { hwnd } => focus(keys, joined, *hwnd).map(Reply::Note),
        What::WindowAt { x, y } => Ok(Reply::Handle(unsafe {
            deepest_at(POINT { x: *x, y: *y }) as isize
        })),
    }
}

/// Joins the target's input queue to ours and installs `keys` there.
///
/// The join is what makes the state shared, and it is held until the target changes:
/// releasing it before the target has drained its queue - which is always, because posting
/// is asynchronous - would hand the target back a queue whose modifiers are all up.
unsafe fn join(target: HWND, keys: &mut [u8; 256], joined: &mut Option<DWORD>) {
    let theirs = GetWindowThreadProcessId(target, null_mut());
    let ours = GetCurrentThreadId();
    if theirs == 0 || theirs == ours {
        return;
    }
    if *joined != Some(theirs) {
        if let Some(previous) = joined.take() {
            AttachThreadInput(ours, previous, FALSE);
        }
        if AttachThreadInput(ours, theirs, TRUE) != FALSE {
            *joined = Some(theirs);
        } else {
            return;
        }
    }
    SetKeyboardState(keys.as_mut_ptr());
}

/// The `MK_*` bits a mouse message carries: the buttons already down, plus the modifiers the
/// operator is holding.
///
/// The shell and the common controls read these out of the message rather than calling
/// `GetKeyState`, so a click that does not carry them is a click with no modifier even when
/// the modifier is set in the queue.
fn button_state(keys: &[u8; 256], held: u32) -> u32 {
    let mut state = held;
    if keys[VK_SHIFT as usize] & KEY_DOWN != 0 {
        state |= MK_SHIFT as u32;
    }
    if keys[VK_CONTROL as usize] & KEY_DOWN != 0 {
        state |= MK_CONTROL as u32;
    }
    state
}

fn held(keys: &[u8; 256], vk: u8) -> bool {
    keys[vk as usize] & KEY_DOWN != 0
}

/// Injects a mouse event at `(x, y)` into this desktop.
///
/// Posted window messages rather than `SendInput`: Windows delivers injected input to the
/// input desktop only, so on a hidden desktop every `SendInput` call reports "delivered 0
/// of 1" and the screen is dead to the operator. Posting reaches any desktop, and pays for
/// it with the parts Windows would otherwise do itself - the frame (title bar, its buttons,
/// dragging, activation) and the shell gestures that insist on a real button state.
pub fn send_mouse_at(desktop: &str, x: i32, y: i32, action: MouseAction) -> Result<String, String> {
    let _ = desktop;
    match submit(What::Mouse { x, y, action })? {
        Reply::Note(note) => Ok(note),
        Reply::Handle(_) => Ok(String::new()),
    }
}

fn mouse(
    keys: &mut [u8; 256],
    joined: &mut Option<DWORD>,
    x: i32,
    y: i32,
    action: MouseAction,
) -> Result<String, String> {
    // A move carries its position and becomes the point later events act on. Button and
    // wheel events are sent with no position at all, so they act on that remembered point;
    // acting on the point they arrived with sent every click to (0,0). A client that does
    // put a point in a button event means it, so a non-zero one is kept.
    let (x, y) = match action {
        MouseAction::Move => {
            super::mode::set_cursor((x, y));
            (x, y)
        }
        _ if x == 0 && y == 0 => super::mode::cursor().unwrap_or((x, y)),
        _ => (x, y),
    };
    // A drag outlives the job that started it. While one is in progress every move moves the
    // window - even when the pointer has left the caption or the frame, which is the normal
    // course of a drag - and a release ends it wherever the pointer happens to be. Both are
    // handled here, before the point is classified, so a drag cannot be lost to the hit test
    // and a stale one cannot leak into the next press.
    match action {
        MouseAction::Move => {
            if grab().is_some() {
                let mut note = String::new();
                unsafe { drag(x, y, &mut note) };
                return Ok(note);
            }
        }
        MouseAction::Up(MouseButton::Left) => {
            if let Some(grab) = take_grab() {
                return Ok(format!("released the {}; ", grab.name()));
            }
        }
        _ => {}
    }

    let target = unsafe { deepest_at(POINT { x, y }) };
    if target.is_null() {
        return Err(format!("no window at {x},{y}"));
    }
    unsafe { join(target, keys, joined) };

    // The frame owns the parts of a window that are not its contents, and the frame is what
    // decides what a click on one of them means. A point outside the client area is not a
    // click on the window at all, and sending it as one is why the caption and its buttons
    // were dead.
    let frame = unsafe {
        let root = GetAncestor(target, GA_ROOT);
        if root.is_null() {
            target
        } else {
            root
        }
    };
    let hit = hit_test(frame, x, y);
    if hit != HTCLIENT && hit != HTNOWHERE {
        return non_client(frame, hit, x, y, action);
    }

    let held_buttons = HELD_BUTTON.load(Ordering::SeqCst);
    let mut note = String::new();
    // Mouse messages carry client coordinates; the wheel is the exception and takes the
    // screen point.
    let (cx, cy) = client_point(target, x, y);
    // The wheel's low word is the key state and its high word the movement; every other
    // message puts the buttons there.
    let state = button_state(keys, held_buttons) as usize;
    let (message, wparam, lparam) = match action {
        MouseAction::Move => (WM_MOUSEMOVE, state, pack(cx, cy)),
        MouseAction::Down(button) => {
            // A press is always preceded by a move on a real desktop, and a control that
            // highlights whatever the pointer is over - a tool bar, and most of the shell -
            // will not act on a press it never saw the pointer arrive for. The client does
            // send moves, but not when a click follows another click at the same spot.
            unsafe {
                PostMessageW(target, WM_MOUSEMOVE, state, pack(cx, cy));
            }
            // A posted click cannot activate a window, and an inactive window ignores it -
            // except a context menu, which drives itself and is already in front. Activating
            // one takes its input state away and closes it without running the item that was
            // clicked, so a click on a menu item arrived as nothing at all.
            if class_of(target) != MENU_CLASS {
                unsafe { activate(target, &mut note) };
            }
            let double = matches!(button, MouseButton::Left) && second_press(x, y);
            HELD_BUTTON.store(button_mask(button), Ordering::SeqCst);
            (
                if double {
                    WM_LBUTTONDBLCLK
                } else {
                    down_message(button)
                },
                button_state(keys, button_mask(button)) as usize,
                pack(cx, cy),
            )
        }
        MouseAction::Up(button) => {
            HELD_BUTTON.store(0, Ordering::SeqCst);
            (up_message(button), state, pack(cx, cy))
        }
        MouseAction::Wheel(notches) => (
            WM_MOUSEWHEEL,
            state | wheel_bits(notches),
            pack(x, y),
        ),
        MouseAction::HorizontalWheel(notches) => (
            WM_MOUSEHWHEEL,
            state | wheel_bits(notches),
            pack(x, y),
        ),
        MouseAction::XButton { forward, down } => {
            let which = if forward { XBUTTON2 } else { XBUTTON1 };
            (
                if down { WM_XBUTTONDOWN } else { WM_XBUTTONUP },
                state | (((which as u32) << 16) as usize),
                pack(cx, cy),
            )
        }
    };
    // The window may have closed between being found and being posted to - a click on its
    // close box is exactly what asks for that - and the release that follows it is then a
    // no-op rather than an error.
    if unsafe { IsWindow(target) } == FALSE {
        note.push_str("the window is gone; ");
        return Ok(note);
    }
    if unsafe { PostMessageW(target, message, wparam, lparam) } == FALSE {
        return Err(format!(
            "PostMessage({message:#x}) to {:#x} failed: {}",
            target as usize,
            last_error()
        ));
    }
    // Remember where a click landed: keyboard input follows it, because a hidden desktop
    // never has a foreground window to ask.
    if let MouseAction::Down(_) = action {
        super::mode::set_focus(Some(target as isize));
    }
    note.push_str(&format!(
        "mouse {action:?} at {x},{y} -> {:#x} {} {message:#x}",
        target as usize,
        class_of(target)
    ));
    Ok(note)
}

/// What part of a window a screen point is over, as Windows decides it.
///
/// The window is asked first: `WM_NCHITTEST` is exactly what the system sends itself before
/// every mouse message, and `DefWindowProc` answers it from the frame's real geometry,
/// including the sizes of the caption boxes, which are per-monitor and not worth recomputing.
/// A window that answers "nowhere" - or does not answer in time - falls back to the geometry
/// below.
pub fn hit_test(frame: HWND, x: i32, y: i32) -> i32 {
    if frame.is_null() {
        return HTNOWHERE;
    }
    let mut answer: usize = 0;
    let answered = unsafe {
        SendMessageTimeoutW(
            frame,
            WM_NCHITTEST,
            0,
            pack(x, y),
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            200,
            &mut answer,
        )
    } != 0;
    if answered {
        let code = answer as i32;
        if code != HTNOWHERE {
            // A window with an extended frame - File Explorer on Win11 and its kin - draws its
            // caption in the client area and reports the whole top strip as HTCLIENT. Its
            // caption boxes are still where the frame geometry puts them, so they are read
            // back here; a window whose caption is genuinely part of the non-client area never
            // reports HTCLIENT for the top strip, so this cannot misfire on one.
            if code == HTCLIENT {
                if let Some(box_code) = unsafe { caption_box_geometry(frame, x, y) } {
                    return box_code;
                }
            }
            return code;
        }
    }
    geometric_hit_test(frame, x, y)
}

/// The caption box a screen point is over, read from the frame's own geometry rather than its
/// answer to `WM_NCHITTEST`.
///
/// Used for windows that have extended their frame into the client area and report the whole
/// caption as client. The boxes fill the caption band top to bottom, so only the horizontal
/// positions matter; the system menu is on the left, and close, maximise and minimise are on
/// the right in that order, each one `SM_CXSIZE` wide.
unsafe fn caption_box_geometry(frame: HWND, x: i32, y: i32) -> Option<i32> {
    let style = GetWindowLongW(frame, GWL_STYLE) as u32;
    if style & WS_CAPTION == 0 {
        return None;
    }
    let (left, top, right, _) = window_rect(frame);
    if y < top || y >= top + CAPTION_BAND_HEIGHT || x < left || x >= right {
        return None;
    }
    let button_width = GetSystemMetrics(SM_CXSIZE) + GetSystemMetrics(SM_CXPADDEDBORDER);
    if style & WS_SYSMENU != 0 && x < left + button_width {
        return Some(HTSYSMENU);
    }
    if style & WS_SYSMENU != 0 && x >= right - button_width {
        return Some(HTCLOSE);
    }
    if style & WS_MAXIMIZEBOX != 0 && x >= right - 2 * button_width {
        return Some(HTMAXBUTTON);
    }
    if style & WS_MINIMIZEBOX != 0 && x >= right - 3 * button_width {
        return Some(HTMINBUTTON);
    }
    None
}

/// The hit test a frame's own geometry implies, for a window that will not answer for itself.
///
/// Only what can be measured is derived: the caption band and the frame's borders come from
/// the difference between the window rectangle and the client rectangle, so no constant has
/// to be right about the current theme or the monitor's scale. Getting a caption box wrong is
/// worse than not guessing at all - it closes or minimises a window the operator only meant
/// to select - so a point in the caption away from the boxes is reported as the caption and
/// the boxes themselves are only distinguished when the style says they exist.
fn geometric_hit_test(frame: HWND, x: i32, y: i32) -> i32 {
    let style = unsafe { GetWindowLongW(frame, GWL_STYLE) } as u32;
    let window = window_rect(frame);
    let mut client: RECT = unsafe { zeroed() };
    if window.2 <= window.0 || unsafe { GetClientRect(frame, &mut client) } == FALSE {
        return HTNOWHERE;
    }
    let mut top_left = POINT {
        x: client.left,
        y: client.top,
    };
    let mut bottom_right = POINT {
        x: client.right,
        y: client.bottom,
    };
    if unsafe {
        ClientToScreen(frame, &mut top_left) == FALSE || ClientToScreen(frame, &mut bottom_right) == FALSE
    } {
        return HTNOWHERE;
    }
    if x >= top_left.x && x < bottom_right.x && y >= top_left.y && y < bottom_right.y {
        return HTCLIENT;
    }
    if x < window.0 || x >= window.2 || y < window.1 || y >= window.3 {
        return HTNOWHERE;
    }
    let left_border = top_left.x - window.0;
    let right_border = window.2 - bottom_right.x;
    let bottom_border = window.3 - bottom_right.y;
    let resizable = style & WS_THICKFRAME != 0;
    let edge = if resizable {
        (unsafe { GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER) }) as i32
    } else {
        left_border.max(right_border).max(bottom_border)
    };

    if y < top_left.y {
        // The caption band, from the top of the window to the top of the client area.
        let box_width = unsafe { GetSystemMetrics(SM_CXSIZE) } as i32;
        if style & WS_SYSMENU != 0 && x < window.0 + left_border + box_width {
            return HTSYSMENU;
        }
        let boxes_end = window.2 - right_border;
        if style & WS_SYSMENU != 0 && x >= boxes_end - box_width {
            return HTCLOSE;
        }
        if style & WS_MAXIMIZEBOX != 0 && x >= boxes_end - 2 * box_width {
            return HTMAXBUTTON;
        }
        if style & WS_MINIMIZEBOX != 0 && x >= boxes_end - 3 * box_width {
            return HTMINBUTTON;
        }
        return HTCAPTION;
    }

    // Outside the caption, inside the frame: one of the resizable borders, or the corner of
    // two of them.
    let left = x < window.0 + edge;
    let right = x >= window.2 - edge;
    let bottom = y >= window.3 - edge;
    match (left, right, bottom) {
        (true, _, true) => HTBOTTOMLEFT,
        (_, true, true) => HTBOTTOMRIGHT,
        (true, _, _) => HTLEFT,
        (_, true, _) => HTRIGHT,
        (_, _, true) => HTBOTTOM,
        _ => HTBORDER,
    }
}

/// A click outside the client area.
///
/// The frame is the receiver for all of these, and the hit code - not the coordinates - is
/// what says which part of it was hit. The three groups are treated differently, and the
/// reason is the same for all three: a posted message is processed by the frame's own thread
/// on its own terms.
///
/// * The boxes that stand for a command - close, minimise, maximise - run that command.
///   `WM_SYSCOMMAND` is what the frame sends *itself* once a real press and release land on
///   one of them, so this is the system's own answer rather than a way around it.
/// * The caption and the borders are moved and resized here instead of through
///   `WM_NCLBUTTONDOWN`. That message makes the frame start a modal drag loop inside its own
///   thread, and on a desktop nobody is operating by hand the loop ends only when a real
///   button release arrives - which it never does, because the release would have to be
///   processed by the thread that is inside the loop. `SetWindowPos` reaches the same result
///   without the trap.
/// * Everything else in the band - the frame's scroll bars, the menu bar, the system menu,
///   the help box - is left to the keyboard, which is the only path that can reach it without
///   that loop. Those are reported rather than silently dropped.
fn non_client(frame: HWND, hit: i32, x: i32, y: i32, action: MouseAction) -> Result<String, String> {
    let mut note = String::new();
    match action {
        MouseAction::Move => {
            // A real desktop highlights whatever box the pointer is over, and this is the
            // message that drives it. A drag is not continued here: it was handled before the
            // point was classified, because it continues whether or not the point is still on
            // the frame.
            unsafe {
                PostMessageW(frame, WM_NCMOUSEMOVE, hit as usize, pack(x, y));
            }
        }
        MouseAction::Down(MouseButton::Left) => {
            if class_of(frame) != MENU_CLASS {
                unsafe { activate(frame, &mut note) };
            }
            unsafe {
                PostMessageW(frame, WM_NCMOUSEMOVE, hit as usize, pack(x, y));
            }
            match hit {
                HTCLOSE => system_command(frame, SC_CLOSE, &mut note),
                HTMINBUTTON => {
                    let command = if unsafe { IsIconic(frame) } != FALSE {
                        SC_RESTORE
                    } else {
                        SC_MINIMIZE
                    };
                    system_command(frame, command, &mut note)
                }
                HTMAXBUTTON => {
                    let command = if unsafe { IsZoomed(frame) } != FALSE {
                        SC_RESTORE
                    } else {
                        SC_MAXIMIZE
                    };
                    system_command(frame, command, &mut note)
                }
                // The second press of a double click on a caption asks for the window to be
                // maximised or put back, which is one more command rather than a drag.
                HTCAPTION => {
                    if second_press(x, y) {
                        let command = if unsafe { IsZoomed(frame) } != FALSE {
                            SC_RESTORE
                        } else {
                            SC_MAXIMIZE
                        };
                        system_command(frame, command, &mut note)
                    } else {
                        unsafe { grab_move(frame, x, y, &mut note) }
                    }
                }
                edge if RESIZE_EDGES.contains(&edge) => unsafe { grab_resize(frame, edge, &mut note) },
                HTSYSMENU => note.push_str("system menu: activate only; "),
                HTMENU => note.push_str("menu bar: activate only; "),
                HTHSCROLL | HTVSCROLL => note.push_str("frame scroll bar: activate only; "),
                HTHELP => note.push_str("help box: activate only; "),
                other => note.push_str(&format!("frame part {other}: activate only; ")),
            }
        }
        MouseAction::Up(MouseButton::Left) => {
            if let Some(grab) = take_grab() {
                note.push_str(&format!("released the {}; ", grab.name()));
            }
            // No `WM_NCLBUTTONUP` is sent to close the pair. Half of the non-client button
            // protocol has no defined meaning, and the half that does - the press - is the
            // half that would trap the frame in a modal loop.
        }
        MouseAction::Down(button) => {
            // A right or middle press on the frame is the system menu in a real session; the
            // menu is modal there, so it is left to the keyboard (Alt+Space) and the press is
            // reduced to selecting the window. A menu itself is never activated, for the
            // reason above.
            if class_of(frame) != MENU_CLASS {
                unsafe { activate(frame, &mut note) };
            }
            note.push_str(&format!("{button:?} on the frame: activate only; "));
        }
        MouseAction::Up(button) => note.push_str(&format!("{button:?} up on the frame; ")),
        MouseAction::Wheel(_) | MouseAction::HorizontalWheel(_) => {
            note.push_str("wheel over the frame: no target; ");
        }
        MouseAction::XButton { .. } => note.push_str("side button on the frame: activate only; "),
    }
    note.push_str(&format!(
        "nc hit={hit} at {x},{y} -> {:#x} {} {action:?}",
        frame as usize,
        class_of(frame)
    ));
    Ok(note)
}

/// The window rectangle, as `left, top, right, bottom`.
fn window_rect(hwnd: HWND) -> (i32, i32, i32, i32) {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == FALSE {
        return (0, 0, 0, 0);
    }
    (rect.left, rect.top, rect.right, rect.bottom)
}

/// Runs one of the frame's own boxes.
fn system_command(frame: HWND, command: usize, note: &mut String) {
    unsafe {
        if PostMessageW(frame, WM_SYSCOMMAND, command, 0) == FALSE {
            note.push_str(&format!(
                "system command {command:#x} failed: {}; ",
                last_error()
            ));
        } else {
            note.push_str(&format!("system command {command:#x}; "));
        }
    }
}

/// A move or a resize that a press on the caption or a border started.
///
/// It is kept outside the job that started it because the moves that continue it arrive as
/// separate jobs, and it ends when the button is released. The frame it belongs to is held
/// too, so a drag that wanders off the window still moves the window it started on.
#[derive(Clone, Copy)]
enum Grab {
    Move {
        frame: isize,
        from_cursor_x: i32,
        from_cursor_y: i32,
    },
    Resize {
        frame: isize,
        edge: i32,
        started: (i32, i32, i32, i32),
    },
}

impl Grab {
    fn name(self) -> &'static str {
        match self {
            Grab::Move { .. } => "caption",
            Grab::Resize { .. } => "border",
        }
    }
}

static GRAB: Mutex<Option<Grab>> = Mutex::new(None);

fn grab() -> Option<Grab> {
    *GRAB.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn take_grab() -> Option<Grab> {
    GRAB.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Starts dragging the frame by its caption. The offset from the pointer to the window's
/// origin is kept, so the window does not jump to put its corner under the pointer.
unsafe fn grab_move(frame: HWND, x: i32, y: i32, note: &mut String) {
    let rect = window_rect(frame);
    *GRAB.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Grab::Move {
        frame: frame as isize,
        from_cursor_x: x - rect.0,
        from_cursor_y: y - rect.1,
    });
    note.push_str("caption grabbed; ");
}

unsafe fn grab_resize(frame: HWND, edge: i32, note: &mut String) {
    let started = window_rect(frame);
    *GRAB.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Grab::Resize {
        frame: frame as isize,
        edge,
        started,
    });
    note.push_str("border grabbed; ");
}

/// Continues a move or a resize that is in progress, and does nothing when none is.
///
/// `SWP_NOZORDER` is deliberately left out: bringing the window to the front is what dragging
/// one does, and on a desktop with no foreground window it is the only thing that makes the
/// drag visible. The frame it acts on is the one the press started on, kept in the grab, so a
/// drag that wanders off the window still moves the right one.
unsafe fn drag(x: i32, y: i32, note: &mut String) {
    let Some(active) = grab() else {
        return;
    };
    let (placed, what) = match active {
        Grab::Move {
            frame,
            from_cursor_x,
            from_cursor_y,
        } => (
            SetWindowPos(
                frame as HWND,
                null_mut(),
                x - from_cursor_x,
                y - from_cursor_y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            ),
            "dragged",
        ),
        Grab::Resize {
            frame,
            edge,
            started,
        } => {
            let (left, top, right, bottom) = resize_rect(started, edge, x, y);
            (
                SetWindowPos(
                    frame as HWND,
                    null_mut(),
                    left,
                    top,
                    right - left,
                    bottom - top,
                    SWP_NOACTIVATE,
                ),
                "resized",
            )
        }
    };
    if placed == FALSE {
        note.push_str(&format!("{what} failed: {}; ", last_error()));
    } else {
        note.push_str(&format!("{what}; "));
    }
}

/// The window rectangle a border drag asks for. The edges the grabbed border does not touch
/// stay where they were, and the size is kept above a floor so the window cannot be turned
/// inside out.
fn resize_rect(started: (i32, i32, i32, i32), edge: i32, x: i32, y: i32) -> (i32, i32, i32, i32) {
    const MIN_WIDTH: i32 = 160;
    const MIN_HEIGHT: i32 = 96;
    let (mut left, mut top, mut right, mut bottom) = started;
    if matches!(edge, HTLEFT | HTTOPLEFT | HTBOTTOMLEFT) {
        left = x.min(right - MIN_WIDTH);
    }
    if matches!(edge, HTRIGHT | HTTOPRIGHT | HTBOTTOMRIGHT) {
        right = x.max(left + MIN_WIDTH);
    }
    if matches!(edge, HTTOP | HTTOPLEFT | HTTOPRIGHT) {
        top = y.min(bottom - MIN_HEIGHT);
    }
    if matches!(edge, HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT) {
        bottom = y.max(top + MIN_HEIGHT);
    }
    (left, top, right, bottom)
}

/// The window's class, for the log: "the event went somewhere" and "the event went to the
/// window that means something" are different failures and the handle alone cannot tell them
/// apart.
fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return "?".to_owned();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Movement in notches turned into the delta the message wants, in the high word.
fn wheel_bits(notches: i32) -> usize {
    // The field is signed 16 bit, so a burst larger than one screen of scrolling is clamped
    // rather than wrapped into movement the other way.
    let delta = (notches.saturating_mul(WHEEL_DELTA as i32)).clamp(-32767, 32767);
    (((delta as u32) & 0xFFFF) << 16) as usize
}

/// Posts a keystroke to a window of this desktop.
///
/// `hwnd` defaults to the window a click last landed on, which is what a hidden desktop has
/// instead of a foreground window. `ch`, when given, is the character the client already
/// resolved - text that no layout on this machine could reproduce - and is posted as its
/// own `WM_CHAR`. When there is none, the target's own `TranslateMessage` produces the
/// character from the key, which is why the scan code in `lParam` and the key state have to
/// be right.
pub fn post_key(
    desktop: &str,
    hwnd: Option<isize>,
    vk: u16,
    ch: Option<char>,
    down: bool,
) -> Result<String, String> {
    let _ = desktop;
    match submit(What::Key {
        hwnd,
        vk,
        ch,
        down,
    })? {
        Reply::Note(note) => Ok(note),
        Reply::Handle(_) => Ok(String::new()),
    }
}

fn key(
    keys: &mut [u8; 256],
    joined: &mut Option<DWORD>,
    hwnd: Option<isize>,
    vk: u16,
    ch: Option<char>,
    down: bool,
) -> Result<String, String> {
    let (target, label) = match hwnd.or_else(super::mode::focus) {
        Some(handle) => (handle as HWND, format!("{handle:#x}")),
        None => pick_target(None)?,
    };
    unsafe { join(target, keys, joined) };

    // Alt is the one modifier Windows delivers as a *system* key. A window menu, Alt+Tab,
    // Alt+F4 and every Alt shortcut live behind the SYS form, so Alt held makes this a
    // system key rather than an ordinary one.
    let sys = held(keys, VK_MENU);
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) };
    let extended = is_extended(vk);
    let lparam = key_lparam(scan, down, extended, sys);
    let message = match (down, sys) {
        (true, false) => WM_KEYDOWN,
        (true, true) => WM_SYSKEYDOWN,
        (false, false) => WM_KEYUP,
        (false, true) => WM_SYSKEYUP,
    };
    unsafe {
        if PostMessageW(target, message, vk as usize, lparam) == FALSE {
            return Err(format!(
                "PostMessage({message:#x}) to {:#x} failed: {}",
                target as usize,
                last_error()
            ));
        }
        if down {
            if let Some(ch) = ch {
                // Text the client resolved itself: the character is what the control reads,
                // and the key only exists to keep the message stream looking like a keyboard.
                for unit in ch.encode_utf16(&mut [0u16; 2]).to_vec() {
                    PostMessageW(target, WM_CHAR, unit as usize, lparam);
                }
            }
        }
    }
    Ok(format!(
        "key {vk:#x} {} -> {label}",
        if down { "down" } else { "up" }
    ))
}

/// Posts a run of text to a window of this desktop, one character at a time.
///
/// The protocol's `seq` carries text a platform input method has already composed, so the
/// characters are what the control reads and no key is involved. `hwnd` defaults to the
/// window a click last landed on, the same way a keystroke does.
pub fn post_text(desktop: &str, hwnd: Option<isize>, text: &str) -> Result<String, String> {
    let _ = desktop;
    match submit(What::Text {
        hwnd,
        text: text.to_owned(),
    })? {
        Reply::Note(note) => Ok(note),
        Reply::Handle(_) => Ok(String::new()),
    }
}

fn post_text_at(
    keys: &mut [u8; 256],
    joined: &mut Option<DWORD>,
    hwnd: Option<isize>,
    body: &str,
) -> Result<String, String> {
    let (target, label) = match hwnd.or_else(super::mode::focus) {
        Some(handle) => (handle as HWND, format!("{handle:#x}")),
        None => pick_target(None)?,
    };
    unsafe { join(target, keys, joined) };
    let mut sent = 0usize;
    let mut units = 0usize;
    for ch in body.chars() {
        // A character outside the basic plane is two UTF-16 units, and posting only the code
        // point would put a number no control recognises into the message.
        for unit in ch.encode_utf16(&mut [0u16; 2]).to_vec() {
            units += 1;
            if unsafe { PostMessageW(target, WM_CHAR, unit as usize, 0) } != FALSE {
                sent += 1;
            }
        }
    }
    Ok(format!(
        "posted {sent}/{units} characters to {label}"
    ))
}

/// Puts a window of the hidden desktop in front and gives it focus.
///
/// A hidden desktop has no foreground window of its own, so injected input has nothing to
/// land on. `SetFocus` only crosses to another thread's window while the two input queues
/// are joined, which the worker holds, so this is a real focus rather than a remembered one.
pub fn focus_window(desktop: &str, hwnd: Option<isize>) -> Result<String, String> {
    let _ = desktop;
    match submit(What::Focus { hwnd })? {
        Reply::Note(note) => Ok(note),
        Reply::Handle(_) => Ok(String::new()),
    }
}

fn focus(keys: &mut [u8; 256], joined: &mut Option<DWORD>, hwnd: Option<isize>) -> Result<String, String> {
    let (target, label) = pick_target(hwnd)?;
    unsafe { join(target, keys, joined) };
    let mut notes = vec![format!("target {label}")];
    unsafe {
        notes.push(format!(
            "foreground before: {:#x}",
            GetForegroundWindow() as usize
        ));
        if SetForegroundWindow(target) == FALSE {
            notes.push(format!("SetForegroundWindow failed: {}", last_error()));
        } else {
            notes.push("SetForegroundWindow ok".to_owned());
        }
        if BringWindowToTop(target) == FALSE {
            notes.push(format!("BringWindowToTop failed: {}", last_error()));
        } else {
            notes.push("BringWindowToTop ok".to_owned());
        }
        if SetFocus(target).is_null() {
            notes.push(format!("SetFocus failed: {}", last_error()));
        } else {
            notes.push("SetFocus ok".to_owned());
        }
        notes.push(format!(
            "foreground after: {:#x}",
            GetForegroundWindow() as usize
        ));
    }
    Ok(notes.join("; "))
}

/// Types into a window of the hidden desktop by posting the characters to it.
pub fn type_text(desktop: &str, hwnd: Option<isize>, text: &str) -> Result<String, String> {
    post_text(desktop, hwnd, text)
}

/// The window at a point on this desktop, the way a click would find it.
pub fn window_at(desktop: &str, x: i32, y: i32) -> Result<isize, String> {
    let _ = desktop;
    match submit(What::WindowAt { x, y })? {
        Reply::Handle(handle) => Ok(handle),
        Reply::Note(_) => Ok(0),
    }
}

/// Which button the operator is holding, so a move can carry it and a drag can be told
/// apart from a hover.
static HELD_BUTTON: AtomicU32 = AtomicU32::new(0);
/// The last press, for telling a double click's second press apart from a fresh one.
static LAST_PRESS: Mutex<Option<(Instant, i32, i32)>> = Mutex::new(None);

/// Client coordinates of a screen point inside `hwnd`.
fn client_point(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut point = POINT { x, y };
    if unsafe { ScreenToClient(hwnd, &mut point) } == FALSE {
        return (x, y);
    }
    (point.x, point.y)
}

/// Two 16-bit halves, low first - the layout every mouse message expects.
fn pack(x: i32, y: i32) -> isize {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize
}

/// The `lParam` a keystroke carries: repeat count, scan code, and the bits that say whether
/// the key is extended, whether Alt was down and whether this is the release.
///
/// A zero `lParam` still translates into a character, but anything that reads the scan code
/// back - games, and controls that re-derive the character themselves - sees key 0.
fn key_lparam(scan: u32, down: bool, extended: bool, sys: bool) -> isize {
    let mut v: i32 = 1; // repeat count
    v |= ((scan & 0xFF) as i32) << 16;
    if extended {
        v |= 1 << 24;
    }
    if sys {
        v |= 1 << 29;
    }
    if !down {
        v |= 1u32.wrapping_shl(31) as i32;
    }
    v as isize
}

/// The keys Windows reports with the extended flag set; without it the arrow cluster and
/// the navigation cluster are indistinguishable from their numpad twins.
fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // PageUp, PageDown, End, Home, Left, Up, Right, Down
            | 0x2D | 0x2E // Insert, Delete
            | 0x5B | 0x5C | 0x5D // Meta, RWin, Apps
            | 0x6F // Divide
            | 0x90 // NumLock
            | 0xA3 | 0xA5 // RControl, RAlt
    )
}

fn button_mask(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => MK_LBUTTON as u32,
        MouseButton::Right => MK_RBUTTON as u32,
        MouseButton::Middle => MK_MBUTTON as u32,
    }
}

fn down_message(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => WM_LBUTTONDOWN,
        MouseButton::Right => WM_RBUTTONDOWN,
        MouseButton::Middle => WM_MBUTTONDOWN,
    }
}

fn up_message(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => WM_LBUTTONUP,
        MouseButton::Right => WM_RBUTTONUP,
        MouseButton::Middle => WM_MBUTTONUP,
    }
}

/// True when this press is the second half of a double click: the same spot within the
/// system's interval. Without it the shell selects the desktop icon and refuses to open
/// it, because it never saw the press a double click is built from.
fn second_press(x: i32, y: i32) -> bool {
    let interval = unsafe { GetDoubleClickTime() }.max(1) as u64;
    let now = Instant::now();
    let mut last = LAST_PRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = *last;
    let second = matches!(previous, Some((at, px, py))
        if now.duration_since(at) <= Duration::from_millis(interval)
            && (px - x).abs() <= 4
            && (py - y).abs() <= 4);
    *last = if second { None } else { Some((now, x, y)) };
    second
}

/// Brings the window's frame forward and makes it the active one. A posted message cannot do
/// either, and an inactive window ignores the click.
///
/// `SetForegroundWindow` is heavily restricted for a process that is not the foreground one,
/// and on a desktop nobody is looking at there is no foreground process to be - which is why
/// it reports success here and still leaves the desktop with nothing in the foreground.
/// `SetActiveWindow` and `SetFocus` only reach a window belonging to the calling thread's
/// input queue, and the worker joined the target's queue before calling this, so those two are
/// the ones that actually take: the frame is drawn as active, and applications that ignore
/// input while they think they are in the background start taking it.
unsafe fn activate(target: HWND, note: &mut String) {
    let (frame, raised, active) = unsafe {
        let frame = GetAncestor(target, GA_ROOT);
        let frame = if frame.is_null() { target } else { frame };
        // A hidden desktop has no foreground process, so `SetForegroundWindow` and
        // `BringWindowToTop` both refuse to raise a window that does not belong to the
        // foreground thread - which is every window. `SetWindowPos` is not refused, and a
        // plain `HWND_TOP` does not cross the process boundary on such a desktop, so the
        // window is bounced through the topmost band and back. That re-orders it above every
        // ordinary window, the way a click on a background window does on a real desktop.
        let raised = SetWindowPos(
            frame,
            HWND_TOPMOST as HWND,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) != FALSE
            && SetWindowPos(
                frame,
                HWND_NOTOPMOST as HWND,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            ) != FALSE
            || SetForegroundWindow(frame) != FALSE
            || BringWindowToTop(frame) != FALSE;
        // The return is the previously active window, which is legitimately null when there
        // was none, so the call is reported rather than judged.
        let previous = SetActiveWindow(frame);
        SetFocus(target);
        (frame, raised, !previous.is_null())
    };
    note.push_str(&format!(
        "activated {:#x} (raised={raised}, active={active}); ",
        frame as usize
    ));
}

/// The window a command should act on: the one it was given, or the control under the
/// centre of the first visible titled window.
///
/// The frame is not the receiver - a text box or a canvas is, and it is a child of the
/// frame. Aiming at the frame is why a posted character can be accepted and then ignored.
fn pick_target(hwnd: Option<isize>) -> Result<(HWND, String), String> {
    if let Some(handle) = hwnd.filter(|h| *h != 0) {
        return Ok((handle as HWND, format!("{handle:#x}")));
    }
    unsafe {
        let mut found: Option<(HWND, String)> = None;
        EnumWindows(
            Some(first_titled),
            &mut found as *mut Option<(HWND, String)> as LPARAM,
        );
        match found {
            Some((frame, title)) => {
                let target = deepest_child_at(frame);
                let suffix = if target == frame {
                    ""
                } else {
                    " (child control)"
                };
                Ok((target, format!("{:#x} {title:?}{suffix}", target as usize)))
            }
            None => Err("no visible window with a title on this desktop".to_owned()),
        }
    }
}

/// Walks into the window under the centre of `frame`, the way a click would land.
unsafe fn deepest_child_at(frame: HWND) -> HWND {
    let mut client: RECT = zeroed();
    if GetClientRect(frame, &mut client) == FALSE {
        return frame;
    }
    let mut point = POINT {
        x: (client.left + client.right) / 2,
        y: (client.top + client.bottom) / 2,
    };
    if ClientToScreen(frame, &mut point) == FALSE {
        return frame;
    }
    let mut current = WindowFromPoint(point);
    if current.is_null() {
        return frame;
    }
    loop {
        let mut local = point;
        if ScreenToClient(current, &mut local) == FALSE {
            return current;
        }
        let child = RealChildWindowFromPoint(current, local);
        if child.is_null() || child == current {
            return current;
        }
        current = child;
    }
}

/// Walks from the window containing `point` down to the deepest child, the way a click does.
///
/// DWM puts an invisible `TITLE_BAR_SCAFFOLDING_WINDOW_CLASS` window over the caption boxes of
/// a normal window so it can animate them, and `WindowFromPoint` reports that window instead
/// of the one it decorates. A click aimed at a close box would land on it and do nothing, so
/// it is stepped over to the window it sits above.
/// Walks from the window at `point` down to the deepest child, the way a click does.
///
/// The window is found by walking the desktop's z-order from the top, not with
/// `WindowFromPoint`: that call reports the DWM overlay parked over a caption box
/// (`TITLE_BAR_SCAFFOLDING_WINDOW_CLASS`, `Microsoft.UI.Content.DesktopChildSiteBridge` and
/// their like), and stepping to "the window below" from an owned window does not reliably
/// chain to its owner's siblings. Walking top-down and skipping the windows a real mouse
/// passes through lands on the window the operator is actually aiming at.
unsafe fn deepest_at(point: POINT) -> HWND {
    let mut top = GetTopWindow(null_mut());
    while !top.is_null() {
        if IsWindowVisible(top) != FALSE
            && !is_click_through(top)
            && window_contains(top, point)
        {
            return deepest_child(top, point);
        }
        top = GetWindow(top, GW_HWNDNEXT);
    }
    null_mut()
}

/// The deepest child of `window` at `point`, or `window` itself when there is none.
///
/// The DWM caption overlays can be children of the window they decorate, not only top-level
/// windows, so they are skipped here at every depth.
unsafe fn deepest_child(mut current: HWND, point: POINT) -> HWND {
    loop {
        let mut local = point;
        if ScreenToClient(current, &mut local) == FALSE {
            return current;
        }
        let child = RealChildWindowFromPoint(current, local);
        if child.is_null() || child == current || is_overlay(child) {
            return current;
        }
        current = child;
    }
}

/// True when a window's own rectangle contains a screen point.
unsafe fn window_contains(hwnd: HWND, point: POINT) -> bool {
    let (left, top, right, bottom) = window_rect(hwnd);
    point.x >= left && point.x < right && point.y >= top && point.y < bottom
}

/// A window a real mouse would pass through on its way to the one underneath, at any depth:
/// the DWM overlays parked over the caption boxes (`TITLE_BAR_SCAFFOLDING_WINDOW_CLASS`,
/// `Microsoft.UI.Content.DesktopChildSiteBridge`) and anything carrying the click-through
/// style.
fn is_overlay(hwnd: HWND) -> bool {
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as i32;
    if ex & WS_EX_TRANSPARENT != 0 {
        return true;
    }
    let class = class_of(hwnd);
    class == DWM_SCAFFOLDING || class == DWM_DESKTOP_BRIDGE
}

/// A top-level window that is not a real click target: everything `is_overlay` covers, plus a
/// window that refuses activation - tool tips, the input-method UI and their like - which
/// only ever appear at the top level.
fn is_click_through(hwnd: HWND) -> bool {
    if is_overlay(hwnd) {
        return true;
    }
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as i32;
    ex & WS_EX_NOACTIVATE != 0
}

unsafe extern "system" fn first_titled(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let slot = &mut *(lparam as *mut Option<(HWND, String)>);
    if slot.is_some() {
        return FALSE;
    }
    if IsWindowVisible(hwnd) == FALSE {
        return TRUE;
    }
    let mut buf = [0u16; 256];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if len <= 0 {
        return TRUE;
    }
    *slot = Some((hwnd, String::from_utf16_lossy(&buf[..len as usize])));
    FALSE
}
