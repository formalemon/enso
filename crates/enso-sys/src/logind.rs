//! Brightness through logind.
//!
//! Backlight files in `/sys` are writable by root only. logind's
//! `SetBrightness` lets the user of the active session set them — no udev
//! rule, no setuid helper — which is what desktop brightness keys need.

use crate::{
    bus::{Result, Service},
    power::Backlight,
};

const SESSION: &str = "org.freedesktop.login1.Session";
/// logind resolves this path to the caller's own session.
const OWN_SESSION: &str = "/org/freedesktop/login1/session/auto";

/// Set a backlight to `brightness`, in the device's own units
/// (`0..=backlight.max`; larger values are clamped).
pub fn set_brightness(backlight: &Backlight, brightness: u32) -> Result<()> {
    Service::connect("org.freedesktop.login1")?.run(
        OWN_SESSION,
        SESSION,
        "SetBrightness",
        &(
            backlight.kind.subsystem(),
            backlight.name.as_str(),
            brightness.min(backlight.max),
        ),
    )
}
