//! `enso-auth` — asks for your password when the system needs it, and for
//! codes when pairing Bluetooth devices.
//!
//! Run once per session, it is polkit's authentication agent and BlueZ's
//! pairing agent. Each question is asked by a short-lived prompt (this
//! program again, as `enso-auth prompt`): a card over the dimmed screen.
//! The agent itself never sees a password: the prompt gives it straight
//! to polkit's helper, which checks it and tells polkit.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod daemon;
mod helper;
mod prompt;
mod request;
mod users;

use std::{io, process::ExitCode};

use enso_core::Context;

use crate::request::Request;

const HELP: &str = "\
enso-auth — asks for your password when the system needs it

Usage: enso-auth              Run the agent (start it with the session)
       enso-auth try KIND     Show a prompt as it would look; nothing is checked
                              KIND: password, confirm, pin, passkey, show,
                                    authorize, service

The agent answers polkit — mounting internal disks, changing system
settings, pkexec — and BlueZ, when pairing devices that want a code. Each
question is a card over the screen: Enter answers yes, Esc no. Typed
passwords go only to polkit's own helper.

Options:
  -h, --help       Print this help
  -V, --version    Print the version
";

enum Mode {
    Agent,
    Prompt,
    Try(String),
}

fn main() -> ExitCode {
    let mode = match parse_args(lexopt::Parser::from_env()) {
        Ok(Some(mode)) => mode,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-auth: {message}\nTry 'enso-auth --help'.");
            return ExitCode::from(2);
        }
    };
    match mode {
        Mode::Agent => match daemon::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("enso-auth: {err}");
                ExitCode::FAILURE
            }
        },
        Mode::Prompt => {
            // What's typed here stays here: no core dumps, no peeking.
            let _ = rustix::process::set_dumpable_behavior(
                rustix::process::DumpableBehavior::NotDumpable,
            );
            let request = match request::read(io::stdin().lock()) {
                Ok(request) => request,
                Err(err) => {
                    eprintln!("enso-auth: no question to ask: {err}");
                    println!("deny");
                    return ExitCode::FAILURE;
                }
            };
            show(request, false)
        }
        Mode::Try(kind) => match sample(&kind) {
            Some(request) => show(request, true),
            None => {
                eprintln!("enso-auth: no such prompt `{kind}`\nTry 'enso-auth --help'.");
                ExitCode::from(2)
            }
        },
    }
}

fn show(request: Request, preview: bool) -> ExitCode {
    let ctx = Context::load();
    ctx.report_warnings("enso-auth");
    let appearance = ctx.config.appearance.clone();
    let out: Option<Box<dyn io::Write>> = if preview {
        None
    } else {
        Some(Box::new(io::stdout()))
    };
    match enso_ui::run(prompt::Prompt::new(ctx, request, out, preview), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-auth: {err}");
            if !preview {
                println!("deny");
            }
            ExitCode::FAILURE
        }
    }
}

/// A made-up question of each kind, for `enso-auth try`.
fn sample(kind: &str) -> Option<Request> {
    let device = "Headphones".to_owned();
    Some(match kind {
        "password" => Request::Password {
            message: "Authentication is required to mount a disk.".into(),
            action: "org.freedesktop.udisks2.filesystem-mount-system".into(),
            program: None,
            users: vec![std::env::var("USER").unwrap_or_else(|_| "you".into())],
            cookie: String::new(),
        },
        "confirm" => Request::Confirm {
            device,
            passkey: 482_913,
        },
        "pin" => Request::Pin { device },
        "passkey" => Request::Passkey {
            device: "Keyboard".into(),
        },
        "show" => Request::Show {
            device: "Keyboard".into(),
            code: "039264".into(),
        },
        "authorize" => Request::Authorize { device },
        "service" => Request::Service {
            device,
            service: "audio".into(),
        },
        _ => return None,
    })
}

fn parse_args(mut parser: lexopt::Parser) -> Result<Option<Mode>, String> {
    use lexopt::prelude::*;

    let mut mode = Mode::Agent;
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => return Ok(Some(mode)),
            Some(Short('h') | Long("help")) => {
                print!("{HELP}");
                return Ok(None);
            }
            Some(Short('V') | Long("version")) => {
                println!("enso-auth {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some(Value(word)) if matches!(mode, Mode::Agent) => {
                let word = word.string().map_err(|err| err.to_string())?;
                mode = match word.as_str() {
                    "prompt" => Mode::Prompt,
                    "try" => Mode::Try(
                        parser
                            .value()
                            .map_err(|_| "try what? password, confirm, pin, passkey, show, authorize, or service".to_owned())?
                            .string()
                            .map_err(|err| err.to_string())?,
                    ),
                    other => return Err(format!("unknown command `{other}`")),
                };
            }
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sample_exists() {
        for kind in [
            "password",
            "confirm",
            "pin",
            "passkey",
            "show",
            "authorize",
            "service",
        ] {
            assert!(sample(kind).is_some(), "{kind}");
        }
        assert!(sample("nope").is_none());
    }
}
