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

/// Block, calling `before` each time the machine is about to sleep — a lid
/// closed, `systemctl suspend`, the menu — and only letting it sleep once
/// `before` has returned.
///
/// logind grants that pause to whoever holds a *delay* inhibitor: a file
/// descriptor that says "wait for me". It is held while awake, let go when
/// `before` is done, and taken again on waking. logind waits a few seconds
/// at most, so `before` has to be quick.
pub fn before_sleep(mut before: impl FnMut()) -> Result<()> {
    use std::{future::poll_fn, pin::Pin};

    use zbus::{MatchRule, MessageStream, export::futures_core::Stream, message::Type};

    let service = Service::connect("org.freedesktop.login1")?;
    let hold = || -> Result<zbus::OwnedFd> {
        service.call(
            MANAGER_PATH,
            MANAGER,
            "Inhibit",
            &("sleep", "enso-lock", "Locking the screen first", "delay"),
        )
    };
    let mut signals = zbus::block_on(async {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.login1")
            .interface(MANAGER)
            .member("PrepareForSleep")
            .build()?;
        MessageStream::for_match_rule(rule, service.connection(), None).await
    })?;

    let mut held = Some(hold()?);
    loop {
        let message = zbus::block_on(poll_fn(|cx| Pin::new(&mut signals).poll_next(cx)))
            .ok_or_else(|| crate::Error::new("the system bus connection closed"))??;
        let Ok(going_down) = message.body().deserialize::<bool>() else {
            continue;
        };
        if going_down {
            before();
            // Closing the descriptor is what lets the machine go.
            held = None;
        } else if held.is_none() {
            // Awake again. Without logind's leave the next sleep is simply
            // not waited for, so failing here is not the end.
            held = hold().ok();
        }
    }
}
