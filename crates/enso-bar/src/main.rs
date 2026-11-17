//! `enso-bar` — a minimal status bar for Niri.
//!
//! Three floating islands on a transparent strip: the windows of the current
//! workspace — opening out to every workspace under the pointer — (from
//! Niri's IPC socket), the clock, and a status cluster: network (from `/sys`
//! and `/proc`), battery (from `/sys`), and the notification bell; scrolling
//! on it turns the volume (through PipeWire). Resting the pointer on the
//! status island opens the control center, when `enso-ctl` is installed. Works with no config file and follows
//! `enso-ctl theme set` live.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bar;
mod config;
mod dropdown;
mod icons;
mod niri;
mod sound;

use std::process::ExitCode;

use enso_core::Context;

const HELP: &str = "\
enso-bar — minimal Wayland status bar for Niri

Usage: enso-bar [OPTIONS]

Options:
  -h, --help       Print this help
  -V, --version    Print the version

The left island shows a dot for each window on the current workspace. Hover
it to see every workspace; click a dot to focus that window, or a group of
dots to switch to that workspace. The right island shows network, battery,
and the notification bell; scroll on it to turn the volume, rest the pointer
on it to open the control center (if enso-ctl is installed), click it for the
notification history.

Configuration is optional: the [bar] table of ~/.config/enso/config.toml.
  height   = 36                    bar height in pixels; the status icons
                                   scale with it
  position = \"top\"                 \"top\" or \"bottom\"
  clock    = \"%a %-d %b  %H:%M\"    strftime format
  control_center = true            open enso-ctl while the pointer rests on
                                   the status island
  opacity  = 0.9                   how solid the islands are, 0.0 to 1.0
";

fn main() -> ExitCode {
    if let Err(code) = parse_args() {
        return code;
    }

    let mut ctx = Context::load();
    let config = ctx.section("bar");
    ctx.report_warnings("enso-bar");
    let appearance = ctx.config.appearance.clone();

    match enso_ui::run(bar::Bar::new(ctx, config), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-bar: {err}");
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
            println!("enso-bar {}", env!("CARGO_PKG_VERSION"));
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(arg)) => {
            eprintln!("enso-bar: {}\nTry 'enso-bar --help'.", arg.unexpected());
            Err(ExitCode::from(2))
        }
        Err(err) => {
            eprintln!("enso-bar: {err}");
            Err(ExitCode::from(2))
        }
    }
}
