//! Whether the shadow desktop is on, and the switch that brings the real monitor up.
//!
//! The work itself is the host's virtual-display privacy mode ([`crate::privacy_mode`]),
//! reused rather than reimplemented: it is the only Windows way to get a screen the
//! ordinary capturer can read, and it already knows how to put the display topology back
//! when it is done.
//!
//! The mode is tied to a connection, not to the daemon. The local screens go dark while
//! somebody is connected and come back when that connection ends; "on" at boot would leave
//! the machine unusable locally for as long as it runs.

use hbb_common::{config::Config, log};
use std::sync::atomic::{AtomicI32, Ordering};

use crate::privacy_mode::{self, PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY};

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

/// The connection this module turned the mode on for, or 0 when it is not ours.
///
/// Kept because a privacy mode the peer asked for through its own button lives in the same
/// place; turning that one off when a connection ends is not this module's business.
static OWNER: AtomicI32 = AtomicI32::new(0);

/// Whether the mode is on, whoever turned it on.
pub fn is_active() -> bool {
    privacy_mode::is_in_privacy_mode()
}

/// The implementation that makes a real monitor, which is the whole point: with it, capture
/// and input stay on the paths every other screen uses.
pub fn impl_key() -> &'static str {
    PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY
}

/// Whether this machine can run the mode at all.
///
/// The implementation only appears in the supported list when the virtual display driver is
/// installed and the self-service is running - and if it did not, turning "privacy mode" on
/// would silently fall back to a different implementation and cover the local screen with
/// overlay windows instead of giving the peer a real display.
pub fn available() -> bool {
    privacy_mode::get_supported_privacy_mode_impl()
        .iter()
        .any(|(key, _)| *key == PRIVACY_MODE_IMPL_WIN_VIRTUAL_DISPLAY)
}

/// Why the mode cannot run, for the log and for the operator.
pub const NOT_AVAILABLE: &str = "no virtual display is available. Install the virtual display driver (RustDesk IDD or usbmmidd) and keep the self-service running, then restart the host";

/// Brings the monitor up for `conn_id`, if the setting asks for it.
///
/// Awaited by the login path in the background: plugging a monitor in takes seconds, which
/// is far too long to hold a login on.
pub async fn enable(conn_id: i32) {
    if privacy_mode::is_in_privacy_mode() {
        log::debug!("shadow desktop: privacy mode is already on, leaving it alone");
        return;
    }
    if !available() {
        log::error!("shadow desktop is off: {}", NOT_AVAILABLE);
        return;
    }
    match privacy_mode::turn_on_privacy(impl_key(), conn_id).await {
        Some(Ok(true)) => {
            OWNER.store(conn_id, Ordering::SeqCst);
            log::info!(
                "shadow desktop is on for connection {conn_id}: a virtual display is the primary screen now"
            );
        }
        Some(Ok(false)) => log::error!("shadow desktop did not come up: privacy mode reported off"),
        Some(Err(e)) => log::error!("shadow desktop did not come up: {e}"),
        None => log::error!("shadow desktop did not come up: privacy mode is not available"),
    }
}

/// Takes the monitor back down, if this module was the one that put it up for `conn_id`.
pub fn disable(conn_id: i32) {
    if OWNER
        .compare_exchange(conn_id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let _ = privacy_mode::turn_off_privacy(conn_id, None);
    log::info!("shadow desktop is off for connection {conn_id}: the physical displays are back");
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