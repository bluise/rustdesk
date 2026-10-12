//! Whether the shadow desktop is on, and the switch that brings it up.
//!
//! The mode is one real monitor carrying a hidden workspace, which is two pieces working
//! together:
//!
//! * a hidden desktop, so the remote side works somewhere the machine's own user is not,
//!   and somewhere that is not part of any desktop being looked at;
//! * the host's virtual-display privacy mode, which plugs a real monitor in, makes it
//!   primary at the origin and takes the physical displays off screen.
//!
//! [`desktop::Desktop::make_current`] is what joins them: switching the session to the
//! hidden desktop makes the new monitor show it, and that is why capture, input, the
//! display list and the switch-screen message all stay the ordinary ones.
//!
//! The mode is tied to a connection rather than to the daemon: the local screens go dark
//! while somebody is connected and come back when that connection ends.

use hbb_common::{config::Config, log};
use std::sync::{Mutex, OnceLock};

use crate::privacy_mode::{self, PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY};

use super::{desktop, launch, windows, SHADOW_DESKTOP};

/// Started when the mode comes up: a desktop with nothing on it cannot be worked with.
pub const DEFAULT_PROGRAM: &str = "explorer.exe";

/// The desktop the local user is on. The mode hands the session back to it before the
/// physical displays return, so those screens show their own desktop and not the workspace.
const INTERACTIVE_DESKTOP: &str = "Default";

struct Session {
    /// Closing its last handle destroys the workspace and everything on it, which is what
    /// gives the next session a clean one.
    _desktop: desktop::Desktop,
    /// The connection the mode was brought up for.
    owner: i32,
}

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Session>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Whether the mode is on.
pub fn is_active() -> bool {
    slot().lock().map(|session| session.is_some()).unwrap_or(false)
}

/// The implementation that makes a real monitor, which is the whole point: with it, capture
/// and input stay on the paths every other screen uses.
pub fn impl_key() -> &'static str {
    PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY
}

/// Whether this machine can run the mode at all.
///
/// The implementation only appears in the supported list when the virtual display driver is
/// installed and the self-service is running - and if it did not, asking for "privacy mode"
/// would silently fall back to a different implementation and cover the local screen with
/// overlay windows instead of giving the peer a real display.
pub fn available() -> bool {
    privacy_mode::get_supported_privacy_mode_impl()
        .iter()
        .any(|(key, _)| *key == PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY)
}

/// Why the mode cannot run, for the log and for the operator.
pub const NOT_AVAILABLE: &str = "no virtual display is available. Install the virtual display driver (RustDesk IDD or usbmmidd) and keep the self-service running, then restart the host";

/// Whether the setting asks for the mode.
///
/// An unset option means on: the setting only exists in a build that carries the mode, so
/// "unset" and "off" would mean the same thing - and then the build would do nothing.
pub fn wanted(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes" | "true" | "1" | "on"
    )
}

/// [`wanted`] applied to the option as it is set right now.
pub fn wanted_now() -> bool {
    wanted(&Config::get_option(
        base::config::keys::OPTION_SHADOW_DESKTOP,
    ))
}

/// Brings the workspace up for `conn_id`.
///
/// Awaited by the login path in the background: plugging a monitor in takes seconds, which
/// is far too long to hold a login on.
pub async fn enable(conn_id: i32) {
    if is_active() || privacy_mode::is_in_privacy_mode() {
        log::debug!("shadow desktop: already on, leaving it alone");
        return;
    }
    if !available() {
        log::error!("shadow desktop is off: {}", NOT_AVAILABLE);
        return;
    }

    // The workspace has to exist before the session is switched to it.
    let workspace = match open_workspace() {
        Ok(workspace) => workspace,
        Err(e) => {
            log::error!("shadow desktop could not start: {e}");
            return;
        }
    };

    // A real monitor: the virtual display is plugged in, made primary at the origin, and
    // the physical displays are moved off screen.
    let turned_on = privacy_mode::turn_on_privacy(impl_key(), conn_id).await;
    match turned_on {
        Some(Ok(true)) => {}
        other => {
            log::error!("shadow desktop did not come up: {}", describe(other));
            return;
        }
    }

    // The session goes to the hidden desktop, so the new monitor shows that instead of the
    // machine's own desktop.
    if let Err(e) = workspace.make_current() {
        log::error!("shadow desktop could not switch the session: {e}");
        let _ = privacy_mode::turn_off_privacy(conn_id, None);
        return;
    }

    if let Ok(mut guard) = slot().lock() {
        *guard = Some(Session {
            _desktop: workspace,
            owner: conn_id,
        });
    }
    log::info!(
        "shadow desktop is on for connection {conn_id}: a real monitor is showing the hidden workspace"
    );
}

/// Takes the workspace down, if this module brought it up for `conn_id`.
pub fn disable(conn_id: i32) {
    let session = match take_owned(conn_id) {
        Some(session) => session,
        None => return,
    };

    // The local desktop comes back first: restore the displays after that, and the physical
    // screens show their own desktop instead of flashing the workspace on their way back.
    match desktop::Desktop::open(INTERACTIVE_DESKTOP) {
        Ok(local) => {
            if let Err(e) = local.make_current() {
                log::error!("shadow desktop could not switch the session back: {e}");
            }
        }
        Err(e) => log::error!("shadow desktop could not open the local desktop: {e}"),
    }
    let _ = privacy_mode::turn_off_privacy(conn_id, None);
    // Dropping the session closes the workspace's last handle, which destroys it.
    drop(session);
    log::info!(
        "shadow desktop is off for connection {conn_id}: the local desktop and its displays are back"
    );
}

/// Brings the mode up for a connection that just logged in, without holding the login.
pub fn enable_on_login(conn_id: i32) {
    if !wanted_now() {
        log::info!("shadow desktop is turned off");
        return;
    }
    tokio::spawn(async move { enable(conn_id).await });
}

/// Takes the mode down when its connection closes.
pub fn disable_on_close(conn_id: i32) {
    disable(conn_id);
}

/// Called once at daemon start.
///
/// Physical displays that were moved off screen by a run that was killed never come back by
/// themselves, so the recorded topology is restored before anything else happens.
pub fn clear_after_crash() {
    crate::privacy_mode::restore_reg_connectivity(false, true);
}

fn take_owned(conn_id: i32) -> Option<Session> {
    let mut guard = slot().lock().ok()?;
    match guard.as_ref() {
        Some(session) if session.owner == conn_id => guard.take(),
        _ => None,
    }
}

/// Opens the hidden desktop and, unless it already has one, starts a shell on it.
fn open_workspace() -> Result<desktop::Desktop, String> {
    let workspace = desktop::Desktop::create_or_open(SHADOW_DESKTOP)?;
    if explorer_running_on(&workspace) {
        // A desktop keeps its windows, so starting the program again would stack a second
        // explorer - and a third - on top of the one already there.
        log::info!("shadow desktop: the workspace already has a shell");
        return Ok(workspace);
    }
    match launch::spawn_on_desktop(&workspace.qualified_name(), DEFAULT_PROGRAM, &[]) {
        Ok(pid) => log::info!("shadow desktop: started {DEFAULT_PROGRAM} on the workspace ({pid})"),
        // The workspace is still usable by hand, so this is not fatal.
        Err(e) => log::warn!("shadow desktop could not start {DEFAULT_PROGRAM}: {e}"),
    }
    Ok(workspace)
}

/// Whether the workspace already has an Explorer window.
fn explorer_running_on(desktop: &desktop::Desktop) -> bool {
    const EXPLORER_CLASSES: [&str; 2] = ["CabinetWClass", "ExploreWClass"];
    match windows::list(&desktop.qualified_name()) {
        Ok(list) => list.iter().any(|w| {
            EXPLORER_CLASSES
                .iter()
                .any(|class| w.class.eq_ignore_ascii_case(class))
        }),
        Err(e) => {
            // A listing that failed is not proof there is none, so start the program the
            // way this did before the check existed.
            log::warn!("shadow desktop could not list the workspace's windows: {e}");
            false
        }
    }
}

/// What a privacy-mode result means, in one line.
fn describe(res: Option<hbb_common::ResultType<bool>>) -> String {
    match res {
        Some(Ok(false)) => "privacy mode reported it is off".to_owned(),
        Some(Err(e)) => e.to_string(),
        None => "privacy mode is not available".to_owned(),
        Some(Ok(true)) => "ok".to_owned(),
    }
}