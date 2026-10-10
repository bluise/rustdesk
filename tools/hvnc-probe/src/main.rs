//! hvnc-probe - a command line over the crate's `shadow-desktop` module.
//!
//! It exists to prove the hidden-desktop mechanism on a real Windows machine, and it
//! deliberately contains no mechanism of its own: every primitive comes from
//! `src/shadow_desktop/`, compiled here from its own source through `#[path]`. So a run
//! of this probe exercises the product code, and `cargo check` in this directory is a
//! local compile check for that module.
//!
//! Being a command line over the module, it can only be built with the feature on -
//! proving the module stays out when the feature is off is a crate-level property, tested
//! separately rather than here.
//!
//! Windows only, and outside the repository workspace: it must never affect a real build.
//! See README.md for how to run it.

#[cfg(not(windows))]
compile_error!("hvnc-probe only builds on Windows: it drives CreateDesktopW / BitBlt / SendInput.");

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod bmp;

// The gating below copies the line in src/lib.rs on purpose: this probe must compile the
// module under exactly the conditions the crate does.
#[cfg(all(windows, feature = "shadow-desktop"))]
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