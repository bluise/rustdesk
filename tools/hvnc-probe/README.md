# hvnc-probe

Standalone Windows probe for the **hidden-desktop** mechanism behind the
shadow-desktop mode (scheme C1). It exists so the three primitives can be proven
on a real machine *before* any of them is wired into RustDesk.

Not part of the workspace (`Cargo.toml` excludes it) and not built by CI: it is
Windows-only and must never slow down a real build.

## What it proves

| Primitive | Win32 mechanism | Why it needs its own probe |
|---|---|---|
| Hidden desktop | `OpenWindowStationW("WinSta0")` + `CreateDesktopW` | Only a desktop inside the **interactive** window station is backed by the display; a service process sits in its own station and gets a desktop that cannot be captured |
| Program off-screen | `CreateProcessW` with `STARTUPINFOW::lpDesktop` | Nothing about the target program is modified: it just never lands on the visible desktop |
| Capture off-screen | thread attached with `SetThreadDesktop`, then `GetDC(NULL)` + `BitBlt` | `GetDC(NULL)` resolves to the **calling thread's** desktop, so capture must run on a thread that was switched there - and that thread may own no windows or hooks |
| Input off-screen | same attachment, then `SendInput` | Same rule: injected input goes to the calling thread's desktop. MSDN adds that SendInput is subject to UIPI |
| Window discovery | `EnumDesktopWindows` | Focus has to be set on a window that really exists on that desktop |

This is the standard HVNC (Hidden VNC) toolkit: `CreateDesktop` + GDI
capture/`PrintWindow` + `EnumWindows` + `SendInput`.

## Build and run

On Windows, from this directory:

```cmd
cargo build --release
target\release\hvnc-probe.exe demo
```

`demo` creates a desktop named `RustDeskShadow`, starts `notepad.exe` (or the
program you pass) on it, waits, and writes `shadow-0.bmp` … `shadow-2.bmp`.

**Nothing should appear on the physical screen** while the demo runs; opening the
bitmaps should show the started program. That is the whole point of the probe.

Step by step (two terminals, because a desktop is destroyed as soon as its last
handle closes - which also kills the windows on it):

```cmd
:: terminal 1 - create the desktop and hold it open
hvnc-probe create RustDeskShadow

:: terminal 2 - drive it
hvnc-probe launch RustDeskShadow notepad.exe
hvnc-probe windows RustDeskShadow
hvnc-probe capture RustDeskShadow shot.bmp 3 500
hvnc-probe click RustDeskShadow 400 300
hvnc-probe key RustDeskShadow 0D
```

If the machine is different: `error` lines state which Win32 call failed and with
which code, and every `SendInput` call is checked (`SendInput delivered 0 of 1`
means UIPI blocked it, or the desktop is gone).

## Known limits, to check on the target machines

- **Session.** A service (session 0) process has to move itself into `WinSta0`
  first; `desktop.rs::ensure_process_in_interactive_winsta` does that and leaks
  the handle on purpose. The in-session RustDesk process is already in `WinSta0`,
  which makes it the natural host for this code.
- **UIPI.** Input only reaches windows of an equal or lower integrity level, so
  an elevated window on the hidden desktop needs an accordingly elevated injector.
- **GDI content only.** `BitBlt` misses most full-screen DirectX/OpenGL output and
  is CPU-bound; the hidden desktop is mostly ordinary windows, but a browser with
  hardware acceleration is worth measuring before promising a frame rate.
- **DPR/DPI.** The capture size comes from `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`
  of that desktop; per-monitor DPI on a mixed setup needs verifying.
- **Focus.** `SendInput` types into whatever window has focus. Picking the right
  window (`EnumDesktopWindows` + `SetFocus`/`SetForegroundWindow` on that desktop)
  is not implemented here yet - it is the next thing the probe should grow.
- **No clipboard routing, no audio, no file transfer** - those are the same
  channels RustDesk already has, and stay outside this mechanism.

## Where this lands in RustDesk

The three touch points for a real integration, once the probe behaves:

1. **Capture.** `libs/scrap/src/dxgi/gdi.rs` already does `CreateDCW` + `BitBlt`;
   it needs a variant whose thread is attached to the shadow desktop (create the
   thread, `SetThreadDesktop`, then build the DC chain), plus the desktop handle
   itself.
2. **Input.** `src/server/input_service.rs` injects through `libs/enigo`
   (`SendInput`, tagged with `ENIGO_INPUT_EXTRA_VALUE`). The desktop attachment has
   to happen on the injecting thread; `src/privacy_mode/win_input.rs` shows the
   existing hook/tag pattern to reuse.
3. **Window selection.** The controller has to see the shadow desktop's windows
   (`EnumDesktopWindows`) and pick one - the per-display machinery in
   `src/server/connection.rs` (`switch_display`, `capture_displays`) is the
   closest existing analogue for exposing a *choice of what to look at*.

Only the peer-side host process changes; the protocol does not have to.

## Caveat

The same mechanism is the core of HVNC malware: it lets one side work on a machine
without the other side seeing anything. Keep it to machines you own or are
explicitly authorised to administer, and keep the connection log auditable.