//! `enso-bar` — a minimal status bar for Niri.
//!
//! Three floating islands on a transparent strip: workspaces (from Niri's IPC
//! socket), the clock, and battery (from `/sys`). Works with no config file
//! and follows `enso-ctl theme set` live.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bar;
mod config;
mod niri;

use std::process::ExitCode;

use enso_core::Context;

const HELP: &str = "\
enso-bar — minimal Wayland status bar for Niri

Usage: enso-bar [OPTIONS]

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Configuration is optional: the [bar] table of ~/.config/enso/config.toml.
  height   = 36                    bar height in pixels
  position = \"top\"                 \"top\" or \"bottom\"
  clock    = \"%a %-d %b  %H:%M\"    strftime format
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
