//! `enso-ctl` — control center GUI and CLI.
//!
//! With no arguments it opens the panel; with a command it acts and exits,
//! so the same binary serves keybindings, scripts, and the mouse.
//!
//! Wi-Fi goes through iwd, Bluetooth through BlueZ, sound through PipeWire,
//! brightness through `/sys` or logind, the dark-or-light preference through
//! dconf, and everything else is read from `/sys` — always the service or
//! the kernel itself, never another program's command line.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod commands;
mod panel;
mod system;
mod theme;

use std::{io::Write, process::ExitCode};

use cli::Command;
use enso_core::{Context, ipc};

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
            let mut ctx = Context::load();
            let config: panel::CtlConfig = ctx.section("ctl");
            ctx.report_warnings("enso-ctl");
            theme::run(&ctx, &config.theme_targets, &command, &mut out)
        }
        Command::Cursor(command) => {
            let mut ctx = Context::load();
            let config: panel::CtlConfig = ctx.section("ctl");
            ctx.report_warnings("enso-ctl");
            theme::cursor::run(&ctx, &config.theme_targets, &command, &mut out)
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
        Command::Quiet(switch) => commands::quiet(switch, &mut out),
        Command::Awake(switch) => commands::keep_awake(switch, &mut out),
        Command::Copy(text) => commands::copy(&text),
        Command::Colour => commands::colour(&mut out),
        Command::Panel(mode, page) => {
            drop(out);
            open_panel(mode, page)
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

fn open_panel(mode: cli::PanelMode, page: Option<panel::Page>) -> Result<(), String> {
    // One control center at a time. Asked for again from the keyboard, the
    // one that is open closes — the key toggles it; asked for by resting on
    // the bar, the one that is open is left as it is; asked for a page, the
    // one that is open turns to it.
    let _only = match ipc::only(panel::NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => {
            if let Some(page) = page {
                ipc::tell(panel::NAME, &format!("page {}", page.name()));
            } else if mode == cli::PanelMode::Modal {
                ipc::tell(panel::NAME, "close");
            }
            return Ok(());
        }
    };
    // The notification history hangs from the same corner of the bar: it
    // goes when the control center comes (and the other way about).
    ipc::tell("enso-notify", "close");

    let mut ctx = Context::load();
    let config = ctx.section("ctl");
    ctx.report_warnings("enso-ctl");
    let appearance = ctx.config.appearance.clone();
    enso_ui::run(panel::Panel::new(ctx, config, mode).asking(page), &appearance)
        .map_err(|err| err.to_string())
}
