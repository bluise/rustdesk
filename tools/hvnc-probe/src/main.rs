//! hvnc-probe - standalone proof of the hidden-desktop mechanism behind the
//! shadow-desktop mode (scheme C1).
//!
//! Windows only, and deliberately outside the repository workspace: it must never
//! take part in a real build. See README.md for how to run it.

#[cfg(not(windows))]
compile_error!("hvnc-probe only builds on Windows: it drives CreateDesktopW / BitBlt / SendInput.");

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod bmp;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod launch;
#[cfg(windows)]
mod util;
#[cfg(windows)]
mod windows;

// The real crate's module, compiled here from its own source through `#[path]`: the gating
// below is a copy of the line in src/lib.rs, so `cargo check` in this directory is a
// compile check for the product code, and `--no-default-features` shows that the module
// stays out of the build entirely when the feature is off.
#[cfg(all(windows, feature = "shadow-desktop"))]
#[allow(dead_code)]
#[path = "../../../src/shadow_desktop/mod.rs"]
mod shadow_desktop;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = app::run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}