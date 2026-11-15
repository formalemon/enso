//! `enso-launch` — a keyboard-first application launcher.
//!
//! Opens focused, filters as you type, and gets out of the way: `Enter`
//! launches, `Esc` or a click outside dismisses, arrows or `Ctrl+n`/`Ctrl+p`
//! move the selection.
//!
//! Applications come straight from `.desktop` entries. Fuzzy file search is
//! the next source to add; the list and ranking code are already independent
//! of where entries come from.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod apps;
mod fuzzy;
mod launcher;

use std::process::ExitCode;

use enso_core::Context;

const HELP: &str = "\
enso-launch — application launcher

Usage: enso-launch [OPTIONS]

Options:
      --list       Print launchable applications and exit
  -h, --help       Print this help
  -V, --version    Print the version

Keys:
  type             filter               Enter        launch
  Up / Ctrl+p      previous             Esc          close
  Down / Ctrl+n    next                 Ctrl+u / w   clear / delete word

Configuration is optional: the [launch] table of ~/.config/enso/config.toml.
  width       = 560            card width in pixels
  max_results = 8              rows shown before the list scrolls
  terminal    = \"alacritty\"    used for terminal applications
";

enum Mode {
    Launcher,
    List,
}

fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(mode) => mode,
        Err(code) => return code,
    };

    let apps = apps::load();
    if let Mode::List = mode {
        for app in &apps {
            println!("{}\t{}", app.id, app.name);
        }
        return ExitCode::SUCCESS;
    }

    let mut ctx = Context::load();
    let config = ctx.section("launch");
    ctx.report_warnings("enso-launch");
    let appearance = ctx.config.appearance.clone();

    match enso_ui::run(launcher::Launcher::new(ctx, config, apps), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-launch: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Parse flags; `Err` carries the exit code to stop with.
fn parse_args() -> Result<Mode, ExitCode> {
    use lexopt::prelude::*;

    let mut mode = Mode::Launcher;
    let mut parser = lexopt::Parser::from_env();
    loop {
        match parser.next() {
            Ok(None) => return Ok(mode),
            Ok(Some(Long("list"))) => mode = Mode::List,
            Ok(Some(Short('h') | Long("help"))) => {
                print!("{HELP}");
                return Err(ExitCode::SUCCESS);
            }
            Ok(Some(Short('V') | Long("version"))) => {
                println!("enso-launch {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            Ok(Some(arg)) => {
                eprintln!(
                    "enso-launch: {}\nTry 'enso-launch --help'.",
                    arg.unexpected()
                );
                return Err(ExitCode::from(2));
            }
            Err(err) => {
                eprintln!("enso-launch: {err}");
                return Err(ExitCode::from(2));
            }
        }
    }
}
