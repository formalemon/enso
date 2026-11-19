//! `enso-power` — the power menu: lock, log out, restart, shut down.
//!
//! Opens focused as a short row of choices. Each has a key — a letter of its
//! own name, marked there (`l`, `o`, `r`, `s`) — so leaving is two
//! keystrokes from anywhere: the binding that opens the menu, and the
//! letter. The three that end the session ask for the
//! letter (or `Enter`) once more first. `Esc` or a click outside dismisses,
//! and running it again while it is open closes it.
//!
//! Given an action as its argument it skips the menu and just does it, which
//! is what key bindings for "lock" or "sleep" want.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod action;
mod menu;

use std::{
    env, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::ExitCode,
};

use enso_core::Context;

use crate::action::Action;

const HELP: &str = "\
enso-power — power menu

Usage: enso-power [ACTION]

With no ACTION, opens the menu. With one, does it at once, no questions:
  lock        Lock the screen (runs enso-lock)
  suspend     Sleep
  logout      Leave Niri
  restart     Restart the machine
  shutdown    Turn the machine off

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Keys: the letter marked in each name
  l  lock      o  log out      r  restart      s  shut down      p  sleep
  Left / Right, Tab, j / k, Ctrl+n / Ctrl+p    move
  Enter    choose          Esc    take back a question, then close

Logging out, restarting, and shutting down ask for a second press.
Running enso-power while it is open closes it.

Configuration is optional: the [power] table of ~/.config/enso/config.toml.
  actions      = [\"lock\", \"suspend\", \"logout\", \"restart\", \"shutdown\"]
                               what is offered, in order
  confirm      = true          ask before the three that end the session
  lock_command = [\"enso-lock\"] what \"lock\" runs
";

fn main() -> ExitCode {
    let direct = match parse_args(lexopt::Parser::from_env()) {
        Ok(Some(direct)) => direct,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-power: {message}\nTry 'enso-power --help'.");
            return ExitCode::from(2);
        }
    };

    let mut ctx = Context::load();
    let config: menu::PowerConfig = ctx.section("power");
    ctx.report_warnings("enso-power");

    if let Some(action) = direct {
        return match action.run(&config.lock_command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("enso-power: cannot {}: {err}", action.verb());
                ExitCode::FAILURE
            }
        };
    }

    // Invoked while already open: close that one instead of opening another.
    let socket =
        env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("enso-power.sock"));
    let listener = match &socket {
        Some(path) if UnixStream::connect(path).is_ok() => return ExitCode::SUCCESS,
        Some(path) => {
            // Nobody is listening, so anything there is left over.
            let _ = fs::remove_file(path);
            UnixListener::bind(path).ok()
        }
        None => None,
    };
    let listening = listener.is_some();

    let appearance = ctx.config.appearance.clone();
    let result = enso_ui::run(menu::Menu::new(ctx, config, listener), &appearance);
    if let (true, Some(path)) = (listening, &socket) {
        let _ = fs::remove_file(path);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-power: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Parse the command line: `Ok(Some(None))` opens the menu,
/// `Ok(Some(Some(action)))` acts at once, and `Ok(None)` means it was
/// answered already (`--help`, `--version`).
fn parse_args(mut parser: lexopt::Parser) -> Result<Option<Option<Action>>, String> {
    use lexopt::prelude::*;

    let mut action = None;
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => return Ok(Some(action)),
            Some(Short('h') | Long("help")) => {
                print!("{HELP}");
                return Ok(None);
            }
            Some(Short('V') | Long("version")) => {
                println!("enso-power {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some(Value(word)) if action.is_none() => {
                let word = word.string().map_err(|err| err.to_string())?;
                action =
                    Some(Action::from_id(&word).ok_or_else(|| format!("unknown action `{word}`"))?);
            }
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Option<Action>>, String> {
        parse_args(lexopt::Parser::from_args(args.iter().copied()))
    }

    #[test]
    fn parses_an_optional_action() {
        assert_eq!(parse(&[]), Ok(Some(None)));
        assert_eq!(parse(&["lock"]), Ok(Some(Some(Action::Lock))));
        assert_eq!(parse(&["shutdown"]), Ok(Some(Some(Action::Shutdown))));
        assert!(parse(&["hibernate"]).is_err());
        assert!(parse(&["lock", "restart"]).is_err());
        assert!(parse(&["--now"]).is_err());
    }
}
