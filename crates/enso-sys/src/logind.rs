//! Brightness and machine power through logind.
//!
//! Backlight files in `/sys` are writable by root only. logind's
//! `SetBrightness` lets the user of the active session set them — no udev
//! rule, no setuid helper — which is what desktop brightness keys need.
//!
//! The same goes for turning the machine off: logind lets the user at the
//! keyboard shut down, restart, and suspend without being root, and tells
//! every service that wants to know first.

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

const MANAGER: &str = "org.freedesktop.login1.Manager";
const MANAGER_PATH: &str = "/org/freedesktop/login1";

/// Something logind can do to the whole machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    PowerOff,
    Reboot,
    Suspend,
}

impl Machine {
    const fn method(self) -> &'static str {
        match self {
            Self::PowerOff => "PowerOff",
            Self::Reboot => "Reboot",
            Self::Suspend => "Suspend",
        }
    }
}

/// Shut down, restart, or suspend. Returns once logind has accepted the
/// request; the machine follows on its own. Refused — with logind's reason —
/// when policy forbids it, for instance while someone else is logged in.
pub fn machine(action: Machine) -> Result<()> {
    // `false`: fail rather than open a password prompt we could not show.
    Service::connect("org.freedesktop.login1")?.run(
        MANAGER_PATH,
        MANAGER,
        action.method(),
        &(false,),
    )
}

/// End the caller's own session: every program in it is stopped and the
/// display manager, or the console, takes over. The blunt way to log out,
/// for when the compositor cannot be asked to quit.
pub fn end_session() -> Result<()> {
    Service::connect("org.freedesktop.login1")?.run(OWN_SESSION, SESSION, "Terminate", &())
}
