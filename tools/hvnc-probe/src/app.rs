//! Command line front end: one subcommand per mechanism, so every step of scheme
//! C1 can be proven on its own on a real machine before any of it is wired into
//! RustDesk.

use std::io::Write;
use std::time::Duration;

use crate::bmp;
use crate::target;
use crate::shadow_desktop::{capture, desktop, input, launch, windows, SHADOW_DESKTOP};

/// Printed on every run: when a report says "it failed", this says which binary produced
/// it, so an old copy on disk cannot be mistaken for a new one.
const BUILD_STAMP: &str = "hvnc-probe 0.1.0 / module 2026-10-10f (classic edit target)";

pub fn run(args: &[String]) -> Result<(), String> {
    println!("{BUILD_STAMP}");
    let (command, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => return Err(usage()),
    };
    match command {
        "create" => create(rest),
        "launch" => launch_cmd(rest),
        "capture" => capture_cmd(rest),
        "diag" => diag_cmd(rest),
        "windows" => windows_cmd(rest),
        "click" => click_cmd(rest),
        "move" => move_cmd(rest),
        "key" => key_cmd(rest),
        "focus" => focus_cmd(rest),
        "type" => type_cmd(rest),
        "target" => target_cmd(rest),
        "demo" => demo(rest),
        "help" | "-h" | "--help" => {
            println!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{}", usage())),
    }
}

fn usage() -> String {
    "\
hvnc-probe <command> [args]

  create  <desktop>                        create the desktop, hold it open until Enter
  launch  <desktop> <program> [args...]    start a program on that desktop
  capture <desktop> <out.bmp> [frames] [interval_ms]
  diag    <desktop> [out-prefix]           try every capture method and report each
  windows <desktop>                        list the windows living on that desktop
  click   <desktop> <x> <y>                left click at x,y on that desktop
  move    <desktop> <x> <y>                move the pointer there
  key     <desktop> <vk> [down|up]         virtual key, hex - 0D is Enter
  focus   <desktop> [hwnd]                 bring a window of that desktop to the front
  type    <desktop> <text> [hwnd]          post characters to a window there
  target  <desktop>                        open a plain text target on that desktop
  demo    [program] [args...]              create + launch + capture, then hold open

Run `create` in one terminal and drive it from another: a desktop is destroyed as
soon as its last handle is closed, which also kills the windows on it."
        .to_owned()
}

fn create(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let desktop = desktop::Desktop::create_or_open(name)?;
    println!("desktop ready: {}", desktop.qualified_name());
    wait_for_enter("press Enter to close it (the desktop and its windows go away)")
}

fn launch_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let program = arg(rest, 1, "<program>")?;
    // Opening it first gives a clear error when the desktop is not there, instead
    // of a CreateProcessW failure that does not say which part was missing.
    let desktop = desktop::Desktop::open(name)?;
    let pid = launch::spawn_on_desktop(&desktop.qualified_name(), program, &rest[2..])?;
    println!("started pid {pid} on {}", desktop.qualified_name());
    Ok(())
}

fn capture_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let out = arg(rest, 1, "<out.bmp>")?;
    let count = parse_or(rest.get(2), 1usize, "frames")?;
    let interval = parse_or(rest.get(3), 500u64, "interval_ms")?;
    let frames = capture::capture_frames(
        name,
        count,
        Duration::from_millis(interval),
        None,
    )?;
    let total = frames.len();
    for (i, frame) in frames.iter().enumerate() {
        let path = frame_path(out, i, total);
        bmp::write(&path, frame.width, frame.height, &frame.bgra)
            .map_err(|e| format!("writing {path} failed: {e}"))?;
        println!("{path}  {}x{}", frame.width, frame.height);
    }
    Ok(())
}

fn focus_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    println!("{}", input::focus_window(name, parse_hwnd(rest.get(1))?)?);
    Ok(())
}

fn type_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let text = arg(rest, 1, "<text>")?;
    println!("{}", input::type_text(name, parse_hwnd(rest.get(2))?, text)?);
    Ok(())
}

fn target_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    target::run(name)
}

/// Window handles are printed as hex, so accept them that way too.
fn parse_hwnd(raw: Option<&String>) -> Result<Option<isize>, String> {
    match raw {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim().trim_start_matches("0x");
            isize::from_str_radix(trimmed, 16)
                .map(Some)
                .map_err(|e| format!("bad hwnd {raw:?}: {e}"))
        }
    }
}

fn diag_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let prefix = rest.get(1).map(|s| s.as_str()).unwrap_or("diag");
    let reports = capture::diagnose(name, None)?;
    println!("capture methods on {name}:");
    for report in reports {
        println!(
            "  {:<26} {}  {}",
            report.method.label(),
            if report.ok { "OK  " } else { "FAIL" },
            report.note
        );
        if let Some(frame) = report.frame {
            let path = format!("{}-{}.bmp", prefix, report.method.label());
            bmp::write(&path, frame.width, frame.height, &frame.bgra)
                .map_err(|e| format!("writing {path} failed: {e}"))?;
            println!("      wrote {path}");
        }
    }
    Ok(())
}

fn windows_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let list = windows::list(name)?;
    if list.is_empty() {
        println!("no windows on {name}");
    }
    for w in list {
        println!(
            "{:#x}  visible={} rect={:?} class={:?} title={:?}",
            w.hwnd, w.visible, w.rect, w.class, w.title
        );
    }
    Ok(())
}

fn click_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let x = parse_or(rest.get(1), 0i32, "<x>")?;
    let y = parse_or(rest.get(2), 0i32, "<y>")?;
    input::send(name, &[input::Action::LeftClick { x, y }])?;
    println!("clicked {x},{y} on {name}");
    Ok(())
}

fn move_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let x = parse_or(rest.get(1), 0i32, "<x>")?;
    let y = parse_or(rest.get(2), 0i32, "<y>")?;
    input::send(name, &[input::Action::MouseMove { x, y }])?;
    println!("moved to {x},{y} on {name}");
    Ok(())
}

fn key_cmd(rest: &[String]) -> Result<(), String> {
    let name = arg(rest, 0, "<desktop>")?;
    let raw = arg(rest, 1, "<vk>")?;
    let vk = u16::from_str_radix(raw.trim_start_matches("0x"), 16)
        .map_err(|e| format!("bad virtual key {raw:?}: {e}"))?;
    let action = match rest.get(2).map(|s| s.as_str()) {
        Some("up") => input::Action::KeyUp { vk },
        Some("down") | None => input::Action::KeyDown { vk },
        Some(other) => return Err(format!("expected down|up, got {other:?}")),
    };
    input::send(name, &[action])?;
    println!("sent {vk:#x} on {name}");
    Ok(())
}

fn demo(rest: &[String]) -> Result<(), String> {
    let program = rest.first().map(|s| s.as_str()).unwrap_or("notepad.exe");
    let args = if rest.is_empty() { &rest[0..0] } else { &rest[1..] };
    let name = SHADOW_DESKTOP;
    let desktop = desktop::Desktop::create_or_open(name)?;
    println!("desktop ready: {}", desktop.qualified_name());
    let pid = launch::spawn_on_desktop(&desktop.qualified_name(), program, args)?;
    println!("started {program} (pid {pid}) - nothing should have appeared on screen");
    std::thread::sleep(Duration::from_millis(1500));

    let frames = capture::capture_frames(name, 3, Duration::from_millis(700), None)?;
    for (i, frame) in frames.iter().enumerate() {
        let path = frame_path("shadow.bmp", i, frames.len());
        bmp::write(&path, frame.width, frame.height, &frame.bgra)
            .map_err(|e| format!("writing {path} failed: {e}"))?;
        println!("{path}  {}x{}", frame.width, frame.height);
    }
    println!("open the bitmaps: they should show {program}, which is not on screen.");
    println!("while this runs you can attach from another terminal, e.g.");
    println!("  hvnc-probe windows RustDeskShadow");
    println!("  hvnc-probe click RustDeskShadow 400 300");
    wait_for_enter("press Enter to close the desktop (and {name}'s windows with it)")
}

fn arg<'a>(rest: &'a [String], index: usize, what: &str) -> Result<&'a str, String> {
    rest.get(index)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("missing {what}\n{}", usage()))
}

fn parse_or<T: std::str::FromStr>(raw: Option<&String>, default: T, what: &str) -> Result<T, String> {
    match raw {
        None => Ok(default),
        Some(raw) => raw
            .parse()
            .map_err(|_| format!("bad {what}: {raw:?}")),
    }
}

/// `out.bmp` for a single frame, `out-0.bmp`, `out-1.bmp` when there are several.
fn frame_path(base: &str, index: usize, total: usize) -> String {
    if total <= 1 {
        return base.to_owned();
    }
    match base.rfind('.') {
        Some(dot) => format!("{}-{index}{}", &base[..dot], &base[dot..]),
        None => format!("{base}-{index}"),
    }
}

fn wait_for_enter(prompt: &str) -> Result<(), String> {
    print!("{prompt}... ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("reading stdin failed: {e}"))?;
    Ok(())
}