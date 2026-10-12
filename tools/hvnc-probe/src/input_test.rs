//! End-to-end check of the posted input path, on a real hidden desktop.
//!
//! Nothing here has a mechanism of its own: `mode::enable` brings the session up the way the
//! daemon does, `input::post_text` / `input::post_key` / `input::send_mouse_at` inject, and
//! the target is an ordinary Win32 EDIT running on the hidden desktop.
//!
//! The target is `winprobe.exe` from the repository's `probe/` directory rather than the
//! host's own Notepad, for the same reason `target.rs` gives: on Windows 11 Notepad is the
//! WinUI rewrite, whose text area is not an EDIT control and ignores posted messages
//! entirely. That would make the test report a failure of the mechanism when it is a failure
//! of the target.
//!
//! Every check is on something the *target* did - the text the control now holds, the key
//! state it saw, the message it received - not on the fact that a call returned Ok. A posted
//! message that arrives and means nothing is exactly the failure mode this is looking for.

use std::fs;
use std::mem::zeroed;
use std::thread;
use std::time::{Duration, Instant};

use winapi::shared::minwindef::{LPARAM, WPARAM};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::winuser::{
    GetAncestor, GetClassNameW, GetTopWindow, GetWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextW, IsIconic, IsWindow, IsWindowVisible, IsZoomed, PostMessageW, SendMessageW,
    ShowWindow, WindowFromPoint, GA_ROOT, GW_HWNDNEXT, SW_RESTORE, WM_GETTEXT, WM_GETTEXTLENGTH,
};

use crate::shadow_desktop::desktop::{AttachedDesktop, Desktop};
use crate::shadow_desktop::input::{self, MouseAction, MouseButton};
use crate::shadow_desktop::{launch, mode, windows, SHADOW_DESKTOP};

/// The window class `winprobe.exe` registers, and the children it puts on the window.
const TARGET_CLASS: &str = "ShadowProbe";
const TARGET_CHILD: &str = "Edit";
const TARGET_BUTTON: &str = "Button";
const TARGET_TOOLBAR: &str = "ToolbarWindow32";
/// The titles the two probe instances are launched with, so each can be found by name.
const TITLE_ALPHA: &str = "Shadow Probe Alpha";
const TITLE_BETA: &str = "Shadow Probe Beta";

/// The parts of a frame this test steers by, spelled out so the test does not depend on the
/// product's own spelling of them.
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_CLOSE: usize = 0xF060;
const HTCAPTION: i32 = 2;
const HTMINBUTTON: i32 = 8;
const HTMAXBUTTON: i32 = 9;
const HTCLOSE: i32 = 20;

const VK_BACK: u16 = 0x08;
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_RETURN: u16 = 0x0D;
const VK_SPACE: u16 = 0x20;
const VK_A: u16 = 0x41;
const VK_C: u16 = 0x43;
const VK_RSHIFT: u16 = 0xA1;

pub fn run(probe_exe: &str, log: &str) -> Result<(), String> {
    let _ = fs::remove_file(log);
    println!("--- shadow desktop input, end to end ---");

    // The session, exactly as the daemon brings it up.
    mode::enable(Some("explorer.exe"), 10, None)?;
    println!("session  : up on desktop {SHADOW_DESKTOP}");

    // The desktop is shared with the daemon and with previous runs, which may have left a
    // stray Explorer or probe window on it. A stray top-level window can sit over the caption
    // boxes the checks below click on, so the leftovers are closed before anything is
    // measured.
    close_leftover("CabinetWClass");
    close_leftover(TARGET_CLASS);

    let qualified = Desktop::create_or_open(SHADOW_DESKTOP)?.qualified_name();
    // Two windows, placed so each keeps its caption clear of the other and their bodies
    // overlap: raising one above the other is something a desktop does and one window cannot
    // demonstrate.
    let alpha = [
        log.to_owned(),
        TITLE_ALPHA.to_owned(),
        "100".to_owned(),
        "100".to_owned(),
        "740".to_owned(),
        "500".to_owned(),
    ];
    let pid = launch::spawn_on_desktop(&qualified, probe_exe, &alpha)?;
    println!("target   : {probe_exe} (pid {pid}) on {qualified}");

    let frame = wait_for_window(TARGET_CLASS, TITLE_ALPHA, Duration::from_secs(25))?;
    let edit = wait_for_child(frame, TARGET_CHILD, Duration::from_secs(5))?;
    println!("windows  : frame {frame:#x}, edit {edit:#x}");
    let (x, y, width, height) = rect_of(edit);
    let (cx, cy) = (x + width / 2, y + height / 2);
    println!("geometry : edit at {x},{y} {width}x{height}; clicks go to {cx},{cy}");
    println!();

    let mut report = Report::default();

    // A click has no position of its own on the wire: the client sends a move first and the
    // button events act on where it left the pointer. Driven the same way here.
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Move)?;
    settle();

    // 1. Text typed on the client. The client resolves the character itself, so this is the
    //    path that carries no virtual key at all.
    input::post_text(SHADOW_DESKTOP, Some(edit), "hello")?;
    settle();
    let text = text_of(edit);
    report.check(
        "typing reaches the control",
        text.contains("hello"),
        format!("text={text:?}"),
    );

    // 2. Space and Return, which arrive as control keys and carry no character: the target
    //    has to derive them from the key, which only happens if the scan code and the key
    //    state let its own TranslateMessage do the work.
    press(edit, VK_SPACE, None)?;
    press(edit, VK_RETURN, None)?;
    settle();
    let text = text_of(edit);
    report.check(
        "space and return reach the control",
        text.contains("hello ") && text.contains('\n'),
        format!("text={text:?}"),
    );

    // 3. Backspace, which a text control inserts and deletes through WM_CHAR rather than
    //    WM_KEYDOWN.
    let before = text_of(edit).chars().count();
    press(edit, VK_BACK, None)?;
    settle();
    let after = text_of(edit).chars().count();
    report.check(
        "backspace deletes",
        after < before,
        format!("{before} -> {after} characters"),
    );

    // 4. Ctrl. A posted key never touches the key state table, so the check is not that a
    //    message arrived but that the target's GetKeyState reports the modifier.
    let mark = log_len(log);
    mode::set_modifiers(&[VK_CONTROL]);
    press(edit, VK_C, None)?;
    mode::set_modifiers(&[]);
    settle();
    let seen = log_since(log, mark);
    report.check(
        "Ctrl is visible inside the target",
        seen.contains("ctrl=1"),
        first_line(&seen),
    );

    // 5. Shift kept for a click: the shell and the list views read MK_SHIFT out of the
    //    message, the target's GetKeyState reads the queue, and both have to agree.
    let mark = log_len(log);
    mode::set_modifiers(&[VK_SHIFT]);
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Up(MouseButton::Left))?;
    mode::set_modifiers(&[]);
    settle();
    let seen = log_since(log, mark);
    report.check(
        "Shift+click carries the modifier",
        seen.contains("WM_LBUTTONDOWN") && seen.contains("shift=1"),
        first_line(&seen),
    );

    // 6. The wheel. The protocol carries notches; Windows wants multiples of 120, and a
    //    scroll that skips the multiplication moves the view by 1/120 of a line.
    let mark = log_len(log);
    let note = input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Wheel(1))?;
    settle();
    let seen = log_since(log, mark);
    let delta = wheel_delta(&seen);
    report.check(
        "one wheel notch is 120",
        delta == Some(120),
        format!("delta={delta:?} note={note:?} log={:?}", first_line(&seen)),
    );

    // 8. Tilt wheel, same unit and its own message.
    let mark = log_len(log);
    let note = input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::HorizontalWheel(1))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "tilt wheel has its own message",
        seen.contains("WM_MOUSEHWHEEL"),
        format!("note={note:?} log={:?}", first_line(&seen)),
    );

    // 9. Mouse 4 and 5, which the router used to drop on the floor.
    let mark = log_len(log);
    let note = input::send_mouse_at(
        SHADOW_DESKTOP,
        cx,
        cy,
        MouseAction::XButton {
            forward: false,
            down: true,
        },
    )?;
    input::send_mouse_at(
        SHADOW_DESKTOP,
        cx,
        cy,
        MouseAction::XButton {
            forward: false,
            down: false,
        },
    )?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "mouse 4/5 reach the window",
        seen.contains("WM_XBUTTONDOWN"),
        format!("note={note:?} log={:?}", first_line(&seen)),
    );

    // 11. A double click has to arrive as one. The desktop has no cursor for Windows to
    //     measure the two presses against, so the interval and the distance are judged here.
    let mark = log_len(log);
    let note = input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Up(MouseButton::Left))?;
    thread::sleep(Duration::from_millis(120));
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Up(MouseButton::Left))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "double click arrives as a double click",
        seen.contains("WM_LBUTTONDBLCLK"),
        format!("note={note:?}"),
    );

    // 12. Drag: the press is carried by the moves that follow it, which is the only thing
    //     that tells a drag from a hover on a desktop with no button state of its own.
    let mark = log_len(log);
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx + 40, cy + 20, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, cx + 40, cy + 20, MouseAction::Up(MouseButton::Left))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "a drag carries the held button",
        seen.contains("WM_LBUTTONDOWN") && seen.contains("WM_LBUTTONUP") && seen.contains("wp=0x0001"),
        first_line(&seen),
    );

    // 13. The middle button, which is its own message and its own bit.
    let mark = log_len(log);
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Middle))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Up(MouseButton::Middle))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "middle click is a middle click",
        seen.contains("WM_MBUTTONDOWN"),
        first_line(&seen),
    );

    // 14. Alt is delivered as a *system* key. With Alt held the key has to go out as
    //     WM_SYSKEYDOWN, or the window menu, Alt+Tab, Alt+F4 and every Alt shortcut are
    //     invisible to the target.
    let mark = log_len(log);
    mode::set_modifiers(&[VK_MENU]);
    press(edit, VK_A, None)?;
    mode::set_modifiers(&[]);
    settle();
    let seen = log_since(log, mark);
    report.check(
        "Alt is delivered as a system key",
        seen.contains("WM_SYSKEYDOWN"),
        first_line(&seen),
    );

    // 15. A click makes the window the active one. A hidden desktop has no foreground process
    //     to inherit that from, so this is the part the mode has to do by hand. It is measured
    //     the way a target would see it - from a thread whose input queue is joined to the
    //     target's, which is what GetActiveWindow and GetFocus answer for.
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, cx, cy, MouseAction::Up(MouseButton::Left))?;
    settle();
    let (foreground, active, focused) = query_active(frame, edit)?;
    report.check(
        "a click makes the window active",
        active == frame || focused == edit,
        format!("foreground={foreground:#x} active={active:#x} focus={focused:#x} (frame {frame:#x}, edit {edit:#x})"),
    );

    // 11. Relative movement, which the client's pointer-lock mode uses: the desktop has no
    //     cursor to move, so the position is accumulated and clamped.
    mode::set_cursor((100, 100));
    let moved = mode::move_cursor_by(50, -20, 1920, 1080);
    let clamped = mode::move_cursor_by(-500, -500, 1920, 1080);
    report.check(
        "relative movement accumulates and clamps",
        moved == Some((150, 80)) && clamped == Some((0, 0)),
        format!("moved={moved:?} clamped={clamped:?}"),
    );

    // 12. The key state itself, including the alias: a target may check the generic modifier
    //     or the side-specific spelling.
    mode::set_modifiers(&[VK_RSHIFT]);
    let held = mode::key_state();
    mode::set_modifiers(&[]);
    let released = mode::key_state();
    report.check(
        "modifiers reach the key state, and let go of it",
        held[VK_SHIFT as usize] & 0x80 != 0
            && held[VK_RSHIFT as usize] & 0x80 != 0
            && released[VK_SHIFT as usize] & 0x80 == 0,
        format!(
            "rshift held: generic shift={:#04x} rshift={:#04x}; after release: shift={:#04x}",
            held[VK_SHIFT as usize], held[VK_RSHIFT as usize], released[VK_SHIFT as usize]
        ),
    );

    // ================= the frame, which is not its contents =================
    //
    // A click on a caption, on one of the boxes in it, or on a border is not a click on the
    // window's contents. Windows turns it into a non-client message carrying a hit-test code,
    // and the frame - not the control - decides what it means. Every check below drives the
    // product's own path and then reads the answer out of the target: the command the frame
    // ran, the box it did it from, and the rectangle the window ended up with.

    let beta_log = format!("{log}.beta");
    let beta = {
        let args = [
            beta_log.clone(),
            TITLE_BETA.to_owned(),
            "140".to_owned(),
            "360".to_owned(),
            "740".to_owned(),
            "500".to_owned(),
        ];
        launch::spawn_on_desktop(&qualified, probe_exe, &args)?;
        wait_for_window(TARGET_CLASS, TITLE_BETA, Duration::from_secs(25))?
    };
    println!();
    println!("frames   : alpha {frame:#x}, beta {beta:#x}");

    // A point inside both windows' bodies. Whoever owns it is the window the desktop considers
    // frontmost, which is how "clicking the caption selected the window" is read back.
    let (alpha_left, alpha_top, _, _) = rect_of(frame);
    let overlap = (alpha_left + 300, alpha_top + 400);

    // 17. Clicking a caption selects the window: it comes to the front, the way clicking a
    //     background window does on a real desktop. The second window is the one on top
    //     before the click, because it was created second.
    let before = top_window_at(overlap)?;
    let (caption_x, caption_y) = frame_caption_point(frame)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Move)?;
    let note = input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let after = top_window_at(overlap)?;
    println!(
        "  z-order after the caption click: {:?}",
        z_order()?
    );
    report.check(
        "a click on the caption selects the window",
        before == beta && after == frame,
        format!("caption {caption_x},{caption_y}; over {overlap:?}: {before:#x} -> {after:#x}; note={note:?}"),
    );

    // 18. A tool bar button. It is an ordinary client click, but only a control that tracks
    //     the pointer produces a command from one - which is the whole of "the tool bar does
    //     not work". The button reports by sending its command to the frame.
    let mark = log_len(log);
    let toolbar = wait_for_child(frame, TARGET_TOOLBAR, Duration::from_secs(5))?;
    let (toolbar_x, toolbar_y, toolbar_w, toolbar_h) = rect_of(toolbar);
    let (bx, by) = (toolbar_x + 15, toolbar_y + toolbar_h / 2);
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Up(MouseButton::Left))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "a tool bar button runs its command",
        seen.contains("WM_COMMAND id=2001"),
        format!(
            "clicked {bx},{by} on a {toolbar_w}x{toolbar_h} tool bar; log={:?}",
            first_line(&seen)
        ),
    );

    // 19. An ordinary push button, the same thing with no tool bar in the way.
    let mark = log_len(log);
    let button = wait_for_child(frame, TARGET_BUTTON, Duration::from_secs(5))?;
    let (button_x, button_y, button_w, button_h) = rect_of(button);
    let (bx, by) = (button_x + button_w / 2, button_y + button_h / 2);
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, bx, by, MouseAction::Up(MouseButton::Left))?;
    settle();
    let seen = log_since(log, mark);
    report.check(
        "a push button runs its command",
        seen.contains("WM_COMMAND id=1002"),
        format!("clicked {bx},{by}; log={:?}", first_line(&seen)),
    );

    // 20. Dragging a window by its caption moves it. The product moves the window itself
    //     rather than sending `WM_NCLBUTTONDOWN`, because that message starts a drag loop
    //     inside the frame's own thread which a posted release can never end.
    thread::sleep(Duration::from_millis(700)); // clear of the double-click interval
    let before_rect = rect_of(frame);
    let (caption_x, caption_y) = frame_caption_point(frame)?;
    let note = input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x + 60, caption_y + 40, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x + 60, caption_y + 40, MouseAction::Up(MouseButton::Left))?;
    settle();
    let after_rect = rect_of(frame);
    report.check(
        "dragging the caption moves the window",
        after_rect.0 == before_rect.0 + 60 && after_rect.1 == before_rect.1 + 40,
        format!("{before_rect:?} -> {after_rect:?}; note={note:?}"),
    );

    // 21. Dragging a border resizes it, and the edges that were not grabbed stay put.
    let before_rect = rect_of(frame);
    let (edge_x, edge_y) = (
        before_rect.0 + before_rect.2 - 2,
        before_rect.1 + before_rect.3 / 2,
    );
    let note = input::send_mouse_at(SHADOW_DESKTOP, edge_x, edge_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, edge_x, edge_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, edge_x + 80, edge_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, edge_x + 80, edge_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let after_rect = rect_of(frame);
    report.check(
        "dragging a border resizes the window",
        after_rect.2 > before_rect.2 + 60
            && after_rect.2 < before_rect.2 + 100
            && after_rect.0 == before_rect.0
            && after_rect.1 == before_rect.1,
        format!("{before_rect:?} -> {after_rect:?}; note={note:?}"),
    );

    // 22. The minimise box. A real press on it is turned by the frame into a system command;
    //     the product sends that command, because the non-client press would leave the frame
    //     waiting for a release that never comes.
    let (box_x, box_y) = frame_box_point(frame, HTMINBUTTON)?;
    let note = input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let iconized = on_desktop(move || unsafe { IsIconic(frame as HWND) })? != 0;
    report.check(
        "the minimise box minimises",
        iconized,
        format!("box {box_x},{box_y}; note={note:?}"),
    );
    on_desktop(move || unsafe { ShowWindow(frame as HWND, SW_RESTORE) })?;
    settle();
    // Restoring does not put the window back on top, so raise it before the next box is
    // clicked, the way the operator would.
    raise_via_caption(frame)?;

    // 23. The maximise box, and back again.
    let (box_x, box_y) = frame_box_point(frame, HTMAXBUTTON)?;
    println!("  point {box_x},{box_y}: {}", describe_point(box_x, box_y));
    let note = input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let zoomed = on_desktop(move || unsafe { IsZoomed(frame as HWND) })? != 0;
    report.check(
        "the maximise box maximises",
        zoomed,
        format!("box {box_x},{box_y}; note={note:?}"),
    );
    on_desktop(move || unsafe { ShowWindow(frame as HWND, SW_RESTORE) })?;
    settle();
    raise_via_caption(frame)?;

    // 23b. A double click on the caption maximises, and a second restores. This is the one
    //      caption gesture the previous check did not cover, and it is a real one: clicking
    //      the empty strip at the top of a window is how it is made full-screen.
    let (caption_x, caption_y) = frame_caption_point(frame)?;
    thread::sleep(Duration::from_millis(700)); // clear of the double-click interval
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Up(MouseButton::Left))?;
    thread::sleep(Duration::from_millis(80));
    let note = input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let zoomed = on_desktop(move || unsafe { IsZoomed(frame as HWND) })? != 0;
    report.check(
        "a double click on the caption maximises",
        zoomed,
        format!("caption {caption_x},{caption_y}; note={note:?}"),
    );

    // 24. The close box, which is the check this whole section exists for: the window has to
    //     be gone, and the frame has to have said why.
    let (box_x, box_y) = frame_box_point(frame, HTCLOSE)?;
    let mark = log_len(log);
    let note = input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    let seen = log_since(log, mark);
    let alive = on_desktop(move || unsafe { IsWindow(frame as HWND) })? != 0;
    report.check(
        "the close box closes the window",
        !alive && seen.contains("WM_SYSCOMMAND wp=0xF060"),
        format!(
            "box {box_x},{box_y}; still open={alive}; note={note:?}; log={:?}",
            first_line(&seen)
        ),
    );

    // 25. A real Explorer window, which is the case the operator reported. It is started on
    //     the hidden desktop, found by class, and closed with its own close box.
    println!();
    let folder = "D:\\WorkBuddy\\2026-10-12-20-25-15\\probe";
    let known = windows_of_class("CabinetWClass");
    launch::spawn_on_desktop(&qualified, "explorer.exe", &[folder.to_owned()])?;
    let (closed, detail) = match wait_for_new_window("CabinetWClass", &known, Duration::from_secs(25)) {
        Ok(window) => match frame_box_point(window, HTCLOSE) {
            Ok((box_x, box_y)) => {
                input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Move)?;
                input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Down(MouseButton::Left))?;
                input::send_mouse_at(SHADOW_DESKTOP, box_x, box_y, MouseAction::Up(MouseButton::Left))?;
                settle();
                thread::sleep(Duration::from_millis(700));
                let alive = on_desktop(move || unsafe { IsWindow(window as HWND) })? != 0;
                (
                    !alive,
                    format!("window {window:#x}, close box {box_x},{box_y}; still open={alive}"),
                )
            }
            Err(e) => (false, format!("window {window:#x}: {e}")),
        },
        Err(e) => (false, e),
    };
    report.check(
        "an Explorer window closes from its own close box",
        closed,
        detail,
    );

    // 26. A right click has to stay a right click; reading the button out of the wrong half of
    //     the mask is what turns every context menu into a left click.
    //
    //     Last on purpose: a right click on a text control opens its context menu, and from
    //     then on the window under the point is that menu (`#32768`) rather than the control.
    //     That is the correct target - it is what the operator is looking at - but it makes
    //     every later mouse check a check of the menu, so this one ends the sequence. It runs
    //     on the second window because the first one has just been closed.
    let other_edit = wait_for_child(beta, TARGET_CHILD, Duration::from_secs(5))?;
    let (other_x, other_y, other_w, other_h) = rect_of(other_edit);
    let (rx, ry) = (other_x + other_w / 2, other_y + other_h / 2);
    let mark = log_len(&beta_log);
    input::send_mouse_at(SHADOW_DESKTOP, rx, ry, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, rx, ry, MouseAction::Down(MouseButton::Right))?;
    input::send_mouse_at(SHADOW_DESKTOP, rx, ry, MouseAction::Up(MouseButton::Right))?;
    settle();
    let seen = log_since(&beta_log, mark);
    report.check(
        "right click is a right click",
        seen.contains("WM_RBUTTONDOWN"),
        format!("log={:?}", first_line(&seen)),
    );

    println!();
    println!("--- target log (the raw evidence) ---");
    match fs::read_to_string(log) {
        Ok(body) => {
            for line in body.lines().take(60) {
                println!("  {line}");
            }
        }
        Err(e) => println!("  could not read {log}: {e}"),
    }

    println!();
    let total = report.pass + report.fail;
    println!("{} of {total} checks passed", report.pass);
    if report.fail > 0 {
        return Err(format!("{} check(s) failed", report.fail));
    }
    Ok(())
}

#[derive(Default)]
struct Report {
    pass: usize,
    fail: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let detail = detail.as_ref();
        println!(
            "{}  {name:<42} {detail}",
            if ok { "PASS" } else { "FAIL" }
        );
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
    }
}

/// A key press and release through the product's own path.
fn press(edit: isize, vk: u16, ch: Option<char>) -> Result<(), String> {
    input::post_key(SHADOW_DESKTOP, Some(edit), vk, ch, true)?;
    input::post_key(SHADOW_DESKTOP, Some(edit), vk, ch, false)?;
    Ok(())
}

/// The target processes posted work on its own thread, so a check has to let it run.
fn settle() {
    thread::sleep(Duration::from_millis(400));
}

fn wait_for_window(class: &str, title: &str, timeout: Duration) -> Result<isize, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let found = windows::list(SHADOW_DESKTOP)
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.class.eq_ignore_ascii_case(class) && w.title == title);
        if let Some(w) = found {
            return Ok(w.hwnd);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no {class} {title:?} on {SHADOW_DESKTOP} within {timeout:?}; saw {:?}",
                window_names()
            ));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn window_names() -> Vec<String> {
    windows::list(SHADOW_DESKTOP)
        .unwrap_or_default()
        .into_iter()
        .map(|w| format!("{} {:?}", w.class, w.title))
        .collect()
}

/// Closes every window of a class, for sweeping the leftovers of a previous run off the
/// shared desktop before anything is measured.
fn close_leftover(class: &str) {
    let targets = windows_of_class(class);
    let count = targets.len();
    for hwnd in targets {
        let _ = on_desktop(move || unsafe {
            PostMessageW(hwnd as HWND, WM_SYSCOMMAND, SC_CLOSE, 0);
        });
    }
    if count > 0 {
        thread::sleep(Duration::from_millis(600));
    }
}

/// Brings a window back to the front the way the operator would, by clicking its caption.
fn raise_via_caption(frame: isize) -> Result<(), String> {
    let (caption_x, caption_y) = frame_caption_point(frame)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Move)?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Down(MouseButton::Left))?;
    input::send_mouse_at(SHADOW_DESKTOP, caption_x, caption_y, MouseAction::Up(MouseButton::Left))?;
    settle();
    Ok(())
}

fn windows_of_class(class: &str) -> Vec<isize> {
    windows::list(SHADOW_DESKTOP)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.class.eq_ignore_ascii_case(class) && w.visible)
        .map(|w| w.hwnd)
        .collect()
}

/// Waits for a window of a class that was not there before, so a window the desktop already
/// had cannot be mistaken for the one just asked for.
fn wait_for_new_window(class: &str, known: &[isize], timeout: Duration) -> Result<isize, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(hwnd) = windows_of_class(class)
            .into_iter()
            .find(|hwnd| !known.contains(hwnd))
        {
            return Ok(hwnd);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no new {class} window within {timeout:?}; saw {:?}",
                window_names()
            ));
        }
        thread::sleep(Duration::from_millis(300));
    }
}

/// A point on the frame that Windows itself reports as `want`.
///
/// The window is asked rather than the product, so the test finds a close box the same way
/// Windows does and the product is judged only on what it does with the point. The scan runs
/// across the caption band, over several rows, because how tall that band is depends on the
/// theme and on the monitor's scale.
fn frame_part_scan(frame: isize, want: i32, from_right: bool) -> Result<(i32, i32), String> {
    on_desktop(move || unsafe {
        let mut rect: RECT = zeroed();
        if GetWindowRect(frame as HWND, &mut rect) == 0 {
            return None;
        }
        let reach = (rect.right - rect.left).clamp(1, 600);
        let mut seen = std::collections::BTreeSet::new();
        for row in [5, 11, 17, 23, 29] {
            let y = rect.top + row;
            for step in 1..reach {
                let x = if from_right {
                    rect.right - step
                } else {
                    rect.left + step
                };
                // Ask the product's own hit test, not the window directly: the product reads
                // the caption boxes back out of the geometry for an extended-frame window, and
                // this test must look for the close box the same way the click will.
                let code = input::hit_test(frame as HWND, x, y);
                seen.insert(code);
                if code == want {
                    return Some((x, y));
                }
            }
        }
        // Report what the window did say, so a missing box can be told apart from a window
        // whose caption is somewhere else or drawn by something that does not hit test.
        let style = GetWindowLongW(frame as HWND, -16) as u32;
        let mut title = [0u16; 128];
        let tlen = GetWindowTextW(frame as HWND, title.as_mut_ptr(), 128);
        let title = if tlen > 0 {
            String::from_utf16_lossy(&title[..tlen as usize])
        } else {
            "(untitled)".to_owned()
        };
        println!(
            "  scan {frame:#x} {title:?} style={style:#x} rect=({},{},{},{}) for {want}: codes along the top = {seen:?}",
            rect.left, rect.top, rect.right, rect.bottom
        );
        None
    })?
    .ok_or_else(|| {
        format!(
            "nothing on the {} of the caption of {frame:#x} reports {want}",
            if from_right { "right" } else { "left" }
        )
    })
}

fn frame_box_point(frame: isize, want: i32) -> Result<(i32, i32), String> {
    frame_part_scan(frame, want, true)
}

/// A point in the caption that is not one of the boxes in it.
fn frame_caption_point(frame: isize) -> Result<(i32, i32), String> {
    frame_part_scan(frame, HTCAPTION, false)
}

/// The top-level window at a point: whoever the desktop would hand that point to.
fn top_window_at(point: (i32, i32)) -> Result<isize, String> {
    on_desktop(move || unsafe {
        let at = WindowFromPoint(POINT {
            x: point.0,
            y: point.1,
        });
        if at.is_null() {
            return 0;
        }
        let root = GetAncestor(at, GA_ROOT);
        (if root.is_null() { at } else { root }) as isize
    })
}

/// The top-level windows in z-order, top first, for reading a raise back out of the desktop.
fn z_order() -> Result<Vec<(isize, String)>, String> {
    on_desktop(|| unsafe {
        let mut out = Vec::new();
        let mut current = GetTopWindow(std::ptr::null_mut());
        while !current.is_null() && out.len() < 40 {
            let mut buf = [0u16; 128];
            let len = GetClassNameW(current, buf.as_mut_ptr(), buf.len() as i32);
            let name = if len > 0 {
                String::from_utf16_lossy(&buf[..len as usize])
            } else {
                "?".to_owned()
            };
            out.push((current as isize, name));
            current = GetWindow(current, GW_HWNDNEXT);
        }
        out
    })
}

/// Every window in z-order that contains a point, with the style bits that say whether it is a
/// real target or an overlay, for reading a hit out of the desktop.
fn describe_point(x: i32, y: i32) -> String {
    match on_desktop(move || unsafe {
        let mut parts = Vec::new();
        let mut current = GetTopWindow(std::ptr::null_mut());
        let mut seen = 0;
        while !current.is_null() && seen < 12 {
            let mut rect: RECT = zeroed();
            GetWindowRect(current, &mut rect);
            if x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom {
                let mut buf = [0u16; 128];
                let len = GetClassNameW(current, buf.as_mut_ptr(), buf.len() as i32);
                let class = if len > 0 {
                    String::from_utf16_lossy(&buf[..len as usize])
                } else {
                    "?".to_owned()
                };
                let ex = GetWindowLongW(current, -20) as i32;
                let visible = IsWindowVisible(current) != 0;
                parts.push(format!(
                    "{:#x} {class} ex={ex:#x} visible={visible}",
                    current as usize
                ));
                seen += 1;
            }
            current = GetWindow(current, GW_HWNDNEXT);
        }
        parts.join("  |  ")
    }) {
        Ok(s) => s,
        Err(e) => e,
    }
}

/// FindWindowExW only searches the calling thread's desktop, so it needs one of its own.
fn wait_for_child(parent: isize, class: &str, timeout: Duration) -> Result<isize, String> {
    let mut name: Vec<u16> = class.encode_utf16().collect();
    name.push(0);
    let deadline = Instant::now() + timeout;
    loop {
        let name = name.clone();
        let found = on_desktop(move || unsafe {
            winapi::um::winuser::FindWindowExW(
                parent as HWND,
                std::ptr::null_mut(),
                name.as_ptr(),
                std::ptr::null(),
            ) as isize
        })?;
        if found != 0 {
            return Ok(found);
        }
        if Instant::now() >= deadline {
            return Err(format!("no {class:?} child under {parent:#x}"));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn on_desktop<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    thread::spawn(move || {
        // Best effort: the read below works on the current desktop, and attaching is what
        // makes that desktop the shadow one.
        let _attached = AttachedDesktop::attach(SHADOW_DESKTOP);
        f()
    })
    .join()
    .map_err(|_| "desktop thread panicked".to_owned())
}

/// What the target's own input queue says about who is active and focused.
///
/// `GetActiveWindow` and `GetFocus` answer for the calling thread's queue, so asking from a
/// thread that has not joined the target's would always say "nothing" and prove nothing.
fn query_active(frame: isize, edit: isize) -> Result<(isize, isize, isize), String> {
    on_desktop(move || unsafe {
        use winapi::shared::minwindef::{FALSE, TRUE};
        use winapi::um::processthreadsapi::GetCurrentThreadId;
        use winapi::um::winuser::{
            AttachThreadInput, GetActiveWindow, GetFocus, GetForegroundWindow,
            GetWindowThreadProcessId,
        };
        let ours = GetCurrentThreadId();
        let theirs = GetWindowThreadProcessId(edit as HWND, std::ptr::null_mut());
        let joined = theirs != 0
            && theirs != ours
            && AttachThreadInput(ours, theirs, TRUE) != FALSE;
        let foreground = GetForegroundWindow() as isize;
        let active = GetActiveWindow() as isize;
        let focused = GetFocus() as isize;
        if joined {
            AttachThreadInput(ours, theirs, FALSE);
        }
        (foreground, active, focused)
    })
}

fn rect_of(hwnd: isize) -> (i32, i32, i32, i32) {
    let mut rect: RECT = unsafe { zeroed() };
    unsafe { GetWindowRect(hwnd as HWND, &mut rect) };
    (
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    )
}

/// The control's text. `GetWindowTextW` only returns captions for a window in another
/// process, so the question goes to the control itself.
fn text_of(hwnd: isize) -> String {
    unsafe {
        let len = SendMessageW(hwnd as HWND, WM_GETTEXTLENGTH, 0, 0) as usize;
        let mut buf = vec![0u16; len + 4];
        let copied = SendMessageW(
            hwnd as HWND,
            WM_GETTEXT,
            buf.len() as WPARAM,
            buf.as_mut_ptr() as LPARAM,
        ) as usize;
        let end = copied.min(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }
}

fn log_len(log: &str) -> usize {
    fs::metadata(log).map(|m| m.len() as usize).unwrap_or(0)
}

fn log_since(log: &str, from: usize) -> String {
    match fs::read(log) {
        Ok(bytes) => String::from_utf8_lossy(&bytes[from.min(bytes.len())..]).into_owned(),
        Err(_) => String::new(),
    }
}

fn first_line(body: &str) -> String {
    body.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(nothing in the log)")
        .trim()
        .to_owned()
}

/// The wheel delta the target reported, out of its own log lines.
fn wheel_delta(seen: &str) -> Option<i32> {
    for line in seen.lines() {
        if !line.contains("WM_MOUSEWHEEL") {
            continue;
        }
        let Some(rest) = line.split("wp=0x").nth(1) else {
            continue;
        };
        let Some(hex) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(value) = u32::from_str_radix(hex, 16) {
            return Some(((value >> 16) as u16) as i16 as i32);
        }
    }
    None
}
