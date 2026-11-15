//! Direct readers for the system interfaces the enso suite talks to.
//!
//! The rule for this crate: speak to the kernel or the service itself
//! (`/sys`, sockets, D-Bus), never to a CLI wrapper. Today it covers the
//! `/sys` hardware interfaces and has no dependencies; the D-Bus backends
//! (BlueZ, iwd) and PipeWire will join as optional features so tools only
//! pay for what they use.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod power;
