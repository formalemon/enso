//! `enso-ctl` — control center GUI and CLI.
//!
//! With no arguments it opens the panel; with a command it acts and exits,
//! so the same binary serves keybindings, scripts, and the mouse.
//!
//! Wi-Fi goes through iwd, Bluetooth through BlueZ, sound through PipeWire,
//! brightness through `/sys` or logind, and everything else is read from
//! `/sys` — always the service or the kernel itself, never another program's
//! command line.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod commands;
mod niri;
mod panel;
mod system;
mod theme;

use std::{io::Write, process::ExitCode};

use cli::Command;
use enso_core::Context;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("enso-ctl: {err}\nTry 'enso-ctl --help'.");
            return ExitCode::from(2);
        }
    };

    let mut out = std::io::stdout().lock();
    let result = match command {
        Command::Help => write!(out, "{}", cli::HELP).map_err(|err| err.to_string()),
        Command::Version => {
            writeln!(out, "enso-ctl {}", env!("CARGO_PKG_VERSION")).map_err(|err| err.to_string())
        }
        Command::Theme(command) => {
            let ctx = Context::load();
            ctx.report_warnings("enso-ctl");
            theme::run(&ctx, &command, &mut out)
        }
        Command::Power => commands::power(&mut out),
        Command::Brightness(light, level) => commands::brightness(light, level, &mut out),
        Command::Profile(command) => commands::profile(&command, &mut out),
        Command::Wifi(command) => commands::wifi(&command, &mut out),
        Command::Bluetooth(command) => commands::bluetooth(&command, &mut out),
        Command::Volume(app, change) => commands::volume(app.as_deref(), change, &mut out),
        Command::Mic(change) => commands::mic(change, &mut out),
        Command::SoundDevice { input, name } => {
            commands::sound_device(input, name.as_deref(), &mut out)
        }
        Command::Panel(mode) => {
            drop(out);
            open_panel(mode)
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-ctl: {err}");
            ExitCode::FAILURE
        }
    }
}

fn open_panel(mode: cli::PanelMode) -> Result<(), String> {
    let mut ctx = Context::load();
    let config = ctx.section("ctl");
    ctx.report_warnings("enso-ctl");
    let appearance = ctx.config.appearance.clone();
    enso_ui::run(panel::Panel::new(ctx, config, mode), &appearance).map_err(|err| err.to_string())
}
