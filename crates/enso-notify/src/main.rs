//! `enso-notify` — a notification daemon for the Freedesktop Desktop
//! Notifications specification.
//!
//! Toasts stack in the top-right corner, and glide up toward the bar when
//! dismissed or expired. Three pieces: the D-Bus service ([`dbus`]), the
//! stack's state and choreography ([`stack`]), and drawing ([`daemon`]).

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod daemon;
mod dbus;
mod stack;

use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use enso_core::Context;

const HELP: &str = "\
enso-notify — desktop notification daemon

Usage: enso-notify [OPTIONS]

Runs until stopped, showing notifications sent to org.freedesktop.Notifications.
Click a toast to dismiss it.

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Configuration is optional: the [notify] table of ~/.config/enso/config.toml.
  width       = 360     toast width in pixels
  timeout_ms  = 5000    lifetime of notifications that do not set one
  max_visible = 5       toasts shown at once; more wait in a queue
  gap         = 10      space between toasts
";

fn main() -> ExitCode {
    if let Err(code) = parse_args() {
        return code;
    }

    let mut ctx = Context::load();
    let config = ctx.section("notify");
    ctx.report_warnings("enso-notify");
    let appearance = ctx.config.appearance.clone();

    // The runtime owns the daemon while it runs; this flag carries a fatal
    // startup problem (already reported on stderr) back out as the exit code.
    let failed = Arc::new(AtomicBool::new(false));
    let daemon = daemon::Daemon::new(ctx, config, failed.clone());

    match enso_ui::run(daemon, &appearance) {
        Ok(()) if !failed.load(Ordering::Relaxed) => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("enso-notify: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Handle `--help`/`--version`; `Err` carries the exit code to stop with.
fn parse_args() -> Result<(), ExitCode> {
    use lexopt::prelude::*;

    // There are no other options, so the first argument decides.
    match lexopt::Parser::from_env().next() {
        Ok(None) => Ok(()),
        Ok(Some(Short('h') | Long("help"))) => {
            print!("{HELP}");
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(Short('V') | Long("version"))) => {
            println!("enso-notify {}", env!("CARGO_PKG_VERSION"));
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(arg)) => {
            eprintln!(
                "enso-notify: {}\nTry 'enso-notify --help'.",
                arg.unexpected()
            );
            Err(ExitCode::from(2))
        }
        Err(err) => {
            eprintln!("enso-notify: {err}");
            Err(ExitCode::from(2))
        }
    }
}
