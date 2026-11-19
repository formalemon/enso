//! `enso-bar` — a minimal status bar for Niri.
//!
//! Three floating islands on a transparent strip: the windows of the current
//! workspace — opening out to every workspace under the pointer — (from
//! Niri's IPC socket), the clock, and a status cluster: network (from `/sys`
//! and `/proc`), battery (from `/sys`), and the notification bell; scrolling
//! on it turns the volume (through PipeWire). Resting the pointer on the
//! status island opens the control center, when `enso-ctl` is installed, and
//! resting on the clock shows what's playing, when `enso-play` is. Works with no config file and follows
//! `enso-ctl theme set` live.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bar;
mod calendar;
mod config;
mod dropdown;
mod every;
mod icons;
mod light;
mod music;
mod niri;
mod osd;
mod sound;
mod water;

use std::process::ExitCode;

use enso_core::Context;

const HELP: &str = "\
enso-bar — minimal Wayland status bar for Niri

Usage: enso-bar [OPTIONS]

With no options, a bar on every screen, coming and going with the screens.

Options:
  --output NAME             only the bar on this screen (`niri msg outputs`
                            names them)
  --dropdown[=top|bottom]   show the month under the clock, as the bar does
                            while the pointer rests there and nothing plays
  -h, --help       Print this help
  -V, --version    Print the version

The left island shows a dot for each window on the current workspace. Hover
it to see every workspace; click a dot to focus that window, or a group of
dots to switch to that workspace. The right island shows network, battery,
and the notification bell; scroll on it to turn the volume, rest the pointer
on it to open the control center (if enso-ctl is installed), click it for the
notification history. With enso-play installed, the clock turns to sea
while something plays; rest the pointer on it to see what's playing, click
it to play or pause.

Configuration is optional: the [bar] table of ~/.config/enso/config.toml.
  height   = 36                    bar height in pixels; the status icons
                                   scale with it
  position = \"top\"                 \"top\" or \"bottom\"
  clock    = \"%a %-d %b  %H:%M\"    strftime format
  control_center = true            open enso-ctl while the pointer rests on
                                   the status island
  now_playing = true               the clock's sea while something plays,
                                   enso-play under it, a click to pause
  calendar = true                  the month under the clock while the
                                   pointer rests there and nothing plays
  opacity  = 0.9                   how solid the islands are, 0.0 to 1.0
  outputs  = []                    the screens to have a bar, by name; none
                                   listed: every screen
";

/// What was asked for.
enum Mode {
    /// A bar on every screen.
    Every,
    /// The bar on one screen.
    On(String),
    /// The calendar, hanging from this edge.
    Calendar(calendar::Edge),
}

fn main() -> ExitCode {
    let output = match parse_args() {
        Err(code) => return code,
        Ok(Mode::Calendar(edge)) => {
            return match calendar::run(edge) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("enso-bar: {err}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(Mode::Every) => {
            let mut ctx = Context::load();
            let config: config::BarConfig = ctx.section("bar");
            ctx.report_warnings("enso-bar");
            return match every::run(&config.outputs) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("enso-bar: {err}");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(Mode::On(output)) => {
            every::end_with_the_watcher();
            output
        }
    };

    let mut ctx = Context::load();
    let config = ctx.section("bar");
    ctx.report_warnings("enso-bar");
    let appearance = ctx.config.appearance.clone();

    match enso_ui::run(bar::Bar::new(ctx, config).on(output), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-bar: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Handle `--help`/`--version`, `--output`, and `--dropdown` (the
/// calendar, on this edge); `Err` carries the exit code to stop with.
fn parse_args() -> Result<Mode, ExitCode> {
    use lexopt::prelude::*;

    // There are no other options, so the first argument decides.
    let mut parser = lexopt::Parser::from_env();
    match parser.next() {
        Ok(None) => Ok(Mode::Every),
        Ok(Some(Long("output"))) => match parser.value().map(|value| value.string()) {
            Ok(Ok(name)) => Ok(Mode::On(name)),
            Ok(Err(err)) | Err(err) => {
                eprintln!("enso-bar: {err}");
                Err(ExitCode::from(2))
            }
        },
        Ok(Some(Long("dropdown"))) => match parser.optional_value() {
            None => Ok(Mode::Calendar(calendar::Edge::Top)),
            Some(edge) if edge == "top" => Ok(Mode::Calendar(calendar::Edge::Top)),
            Some(edge) if edge == "bottom" => Ok(Mode::Calendar(calendar::Edge::Bottom)),
            Some(edge) => {
                eprintln!(
                    "enso-bar: --dropdown is top or bottom, not {}",
                    edge.to_string_lossy()
                );
                Err(ExitCode::from(2))
            }
        },
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
