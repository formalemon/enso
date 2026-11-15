//! `enso-ctl` — control center GUI and CLI.
//!
//! With no arguments it opens the panel; with a command it acts and exits,
//! so the same binary serves keybindings, scripts, and the mouse.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod panel;
mod theme;

use std::{io::Write, process::ExitCode};

use cli::Command;
use enso_core::Context;
use enso_sys::power::{BatteryStatus, Sysfs};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("enso-ctl: {err}\nTry 'enso-ctl --help'.");
            return ExitCode::from(2);
        }
    };

    let mut ctx = Context::load();
    let mut out = std::io::stdout().lock();
    let result = match command {
        Command::Help => write!(out, "{}", cli::HELP).map_err(|err| err.to_string()),
        Command::Version => {
            writeln!(out, "enso-ctl {}", env!("CARGO_PKG_VERSION")).map_err(|err| err.to_string())
        }
        Command::Theme(command) => {
            ctx.report_warnings("enso-ctl");
            theme::run(&ctx, &command, &mut out)
        }
        Command::Power => power(&mut out).map_err(|err| err.to_string()),
        Command::Planned(domain) => {
            eprintln!("enso-ctl: `{domain}` is planned but not implemented yet");
            return ExitCode::from(2);
        }
        Command::Panel => {
            drop(out);
            let config = ctx.section("ctl");
            ctx.report_warnings("enso-ctl");
            let appearance = ctx.config.appearance.clone();
            enso_ui::run(panel::Panel::new(ctx, config), &appearance).map_err(|err| err.to_string())
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

/// `enso-ctl power`: a plain-text summary read from `/sys`.
fn power(out: &mut impl Write) -> std::io::Result<()> {
    let sysfs = Sysfs::system();
    match sysfs.battery() {
        Some(battery) => {
            let status = match battery.status {
                BatteryStatus::Charging => "charging",
                BatteryStatus::Discharging => "discharging",
                BatteryStatus::Full => "full",
                BatteryStatus::NotCharging => "plugged in, not charging",
                BatteryStatus::Unknown => "unknown",
            };
            writeln!(out, "battery      {}% ({status})", battery.percent)?;
        }
        None => writeln!(out, "battery      none")?,
    }
    if let Some(profile) = sysfs.platform_profile() {
        writeln!(
            out,
            "profile      {} (of: {})",
            profile.current,
            profile.choices.join(", ")
        )?;
    }
    if let Some(temperature) = sysfs.cpu_temperature() {
        writeln!(out, "temperature  {temperature:.0}°C")?;
    }
    if let Some(backlight) = sysfs.backlight() {
        writeln!(out, "backlight    {backlight}%")?;
    }
    Ok(())
}
