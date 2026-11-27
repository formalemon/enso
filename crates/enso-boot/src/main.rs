//! `enso-boot` — the boot menu: Arch or Windows, under a pagoda on an
//! island in the sky at dusk, the camera going slowly round it; then,
//! choosing Arch, the terraces the login screen starts from, so the way
//! from the menu to the password is one picture.
//!
//! Built for the firmware (`--target x86_64-unknown-uefi`) it is the menu
//! itself. Built for Linux it is its installer, which says what to copy
//! where and how to give the menu a firmware boot entry.

#![cfg_attr(target_os = "uefi", no_std)]
#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate alloc;

#[cfg(all(test, not(target_os = "uefi")))]
mod art;
#[cfg(target_os = "uefi")]
mod firmware;
#[cfg(not(target_os = "uefi"))]
mod host;
#[cfg_attr(not(any(test, target_os = "uefi")), allow(dead_code))]
mod menu;

#[cfg(not(target_os = "uefi"))]
fn main() -> std::process::ExitCode {
    host::main()
}
