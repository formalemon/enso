//! `enso-notify` — a notification daemon for the Freedesktop Desktop
//! Notifications specification.
//!
//! Toasts stack in the top-right corner, and glide up toward the bar when
//! dismissed or expired. Three pieces: the D-Bus service ([`dbus`]), the
//! stack's state and choreography ([`stack`]), and drawing ([`daemon`]).

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod battery;
mod center;
mod daemon;
mod dbus;
mod local;
mod markup;
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
       enso-notify history [toggle|open|close]

Runs until stopped, showing notifications sent to org.freedesktop.Notifications.

`enso-notify history` asks the running daemon to show what has come in — the
last 50 notifications, newest first, five at a time; scroll for the rest. It
is what a click on enso-bar's bell does, and works from a key binding too.
Click a row to remove it; Esc, a second `history`, or moving the pointer away
closes it.

Click a toast to open what it is about (when the sender offers that) and
dismiss it; click one of its buttons to choose that action; right-click to
dismiss it without doing anything. Toasts stay for as long as the pointer is
on them.

Some buttons are enso-notify's own: a one-time code in a message (\"Your
code is 482 913\") gets one that copies it, and a screenshot gets Open and
Show in Files. With a battery, it warns at 10 % and again at 5 % (with
Sleep now), and takes the warnings back when the power comes.

Do not disturb (the control center, `enso-ctl dnd 1h`) sends notifications
quietly to the history instead; critical ones still show.

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Configuration is optional: the [notify] table of ~/.config/enso/config.toml.
  width       = 360     toast width in pixels
  timeout_ms  = 5000    lifetime of notifications that do not set one
  max_visible = 5       toasts shown at once; more wait in a queue
  gap         = 10      space between toasts
  history_rows = 5      notifications the history shows at once
  history_size = 50     notifications the history remembers
  history_keep = true   keep the history across restarts and logins
  do_not_disturb = false
                        notifications go quietly to the history
  do_not_disturb_until = 0
                        when it ends by itself, in seconds since 1970
                        (0: when it's turned off); the control center sets it
";

fn main() -> ExitCode {
    match parse_args() {
        Ok(None) => {}
        Ok(Some(ask)) => return send(ask),
        Err(code) => return code,
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

/// Pass an ask to the running daemon.
fn send(ask: daemon::Ask) -> ExitCode {
    let sent = enso_core::ipc::socket(daemon::NAME)
        .ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is not set"))
        .and_then(|path| enso_core::ipc::say(&path, ask.word()));
    match sent {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-notify: no running enso-notify to ask ({err})");
            ExitCode::FAILURE
        }
    }
}

/// Handle the arguments: `Ok(Some(ask))` is something to tell the running
/// daemon, `Ok(None)` means be the daemon, and `Err` carries the exit code
/// to stop with.
fn parse_args() -> Result<Option<daemon::Ask>, ExitCode> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_env();
    let usage = |message: String| {
        eprintln!("enso-notify: {message}\nTry 'enso-notify --help'.");
        Err(ExitCode::from(2))
    };
    match parser.next() {
        Ok(None) => Ok(None),
        Ok(Some(Short('h') | Long("help"))) => {
            print!("{HELP}");
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(Short('V') | Long("version"))) => {
            println!("enso-notify {}", env!("CARGO_PKG_VERSION"));
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(Value(command))) if command == "history" => match parser.next() {
            Ok(None) => Ok(Some(daemon::Ask::Toggle)),
            Ok(Some(Value(word))) => match daemon::Ask::parse(&word.to_string_lossy()) {
                Some(ask) => Ok(Some(ask)),
                None => usage(format!(
                    "unknown history command {word:?}: expected toggle, open, or close"
                )),
            },
            Ok(Some(arg)) => usage(arg.unexpected().to_string()),
            Err(err) => usage(err.to_string()),
        },
        Ok(Some(arg)) => usage(arg.unexpected().to_string()),
        Err(err) => usage(err.to_string()),
    }
}
