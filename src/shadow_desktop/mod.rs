//! Shadow desktop: the remote side drives this machine while its own screens are dark.
//!
//! There is no capture path of its own here any more. The mode *is* a real monitor: a
//! virtual display is plugged in, made the primary screen at the origin, and the physical
//! displays are moved off screen - exactly what [`crate::privacy_mode`] already does for
//! its virtual-display implementation. Because it is a real monitor, every ordinary path
//! keeps working untouched:
//!
//! * capture goes through the platform capturer like it does for any other screen,
//! * input goes through the platform path, because the desktop is the session's own,
//! * the display list and the switch-screen message come from the normal enumeration.
//!
//! Only compiled with the `shadow-desktop` feature, and inert without it. [`mode`] is the
//! switch and the only thing this module still owns.
pub mod mode;