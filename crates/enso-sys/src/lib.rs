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
//! - [`auth`] — checking the user's own password. No dependencies, always
//!   available.
//! - [`audio`] — volume of the output and of each application, through
//!   PipeWire (feature `audio`).
//! - [`net`] — whether the machine is online and how, read from `/sys` and
//!   `/proc`. No dependencies, always available.
//!
//! The D-Bus backends are blocking: every call waits for the service's
//! answer, so interfaces call them from a worker thread. Each also has a
//! `watch` that blocks while reporting changes, for a thread of its own.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod auth;
#[cfg(any(feature = "iwd", feature = "bluez", feature = "logind"))]
// logind alone only makes calls; the rest is for the services that are watched.
#[cfg_attr(not(any(feature = "iwd", feature = "bluez")), allow(dead_code))]
mod bus;
pub mod net;
pub mod power;

#[cfg(feature = "audio")]
pub mod audio;

#[cfg(feature = "bluez")]
pub mod bluetooth;
#[cfg(feature = "logind")]
pub mod logind;
#[cfg(feature = "iwd")]
pub mod wifi;

#[cfg(any(feature = "iwd", feature = "bluez", feature = "logind"))]
pub use bus::Error;
