//! Direct clients for the system interfaces the enso suite talks to.
//!
//! The rule for this crate: speak to the kernel or the service itself
//! (`/sys`, sockets, D-Bus), never to a CLI wrapper.
//!
//! - [`power`] — battery, platform profile, temperatures, fans, and backlight
//!   levels, read from `/sys`. No dependencies, always available.
//! - [`wifi`] — Wi-Fi through iwd (feature `iwd`).
//! - [`bluetooth`] — Bluetooth through BlueZ (feature `bluez`).
//! - [`logind`] — setting brightness without root, and shutting down,
//!   restarting, or suspending the machine (feature `logind`).
//! - [`dconf`] — desktop settings, the dark-or-light preference among them,
//!   through dconf's own service (feature `dconf`).
//! - [`udisks`] — disks, partitions, and USB sticks: listing, mounting,
//!   ejecting (feature `udisks`).
//! - [`systemd`] — restarting a user service so it reads new settings
//!   (feature `systemd`).
//! - [`auth`] — checking the user's own password. No dependencies, always
//!   available.
//! - [`audio`] — volume of the output and of each application, through
//!   PipeWire (feature `audio`).
//! - [`mpris`] — what media players are playing, and playing, pausing,
//!   skipping, and seeking them (feature `mpris`).
//! - [`net`] — whether the machine is online and how, read from `/sys` and
//!   `/proc`. No dependencies, always available.
//!
//! The D-Bus backends are blocking: every call waits for the service's
//! answer, so interfaces call them from a worker thread. Each also has a
//! `watch` that blocks while reporting changes, for a thread of its own.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod accounts;
pub mod auth;
#[cfg(any(
    feature = "iwd",
    feature = "bluez",
    feature = "logind",
    feature = "dconf",
    feature = "systemd",
    feature = "udisks",
    feature = "secrets",
    feature = "mpris"
))]
// logind, dconf, and systemd only make calls, and only the last two are on
// the session bus: short of every feature, some of this is unused.
#[cfg_attr(
    not(all(
        any(feature = "iwd", feature = "bluez"),
        any(feature = "dconf", feature = "systemd")
    )),
    allow(dead_code)
)]
mod bus;
pub mod net;
pub mod power;

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "bluez")]
pub mod bluetooth;
#[cfg(feature = "dconf")]
pub mod dconf;
#[cfg(feature = "logind")]
pub mod logind;
#[cfg(feature = "mpris")]
pub mod mpris;
#[cfg(feature = "secrets")]
pub mod secrets;
#[cfg(feature = "systemd")]
pub mod systemd;
#[cfg(feature = "udisks")]
pub mod udisks;
#[cfg(feature = "iwd")]
pub mod wifi;

#[cfg(any(
    feature = "iwd",
    feature = "bluez",
    feature = "logind",
    feature = "dconf",
    feature = "systemd",
    feature = "udisks",
    feature = "mpris"
))]
pub use bus::Error;
