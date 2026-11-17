//! `enso-lock` — locks the screen until the password is typed.
//!
//! The lock is the compositor's own (`ext-session-lock`): from the moment it
//! is granted, Niri shows nothing but this program's surfaces and gives
//! input to nothing else, and it stays that way if this program dies. What
//! is drawn on top of that guarantee is a clock, the date, and a field that
//! fills with dots.
//!
//! A locked machine is one nobody is looking at, so it is also made a quiet
//! one. Nothing is redrawn unless a key is pressed or the minute turns;
//! after half a minute alone the screens are powered off and even the minute
//! is no longer followed; and the machine is held in its low-power profile
//! until it is unlocked. Everything else in the session keeps running —
//! throttled, not stopped — so unlocking returns to exactly what was left.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod lock;
mod saver;
mod watch;

use std::process::ExitCode;

use enso_core::Context;
use enso_sys::auth;
use niri_ipc::Action;

const HELP: &str = "\
enso-lock — lock the screen

Usage: enso-lock [OPTIONS]
       enso-lock watch

With `watch` it locks nothing now: it stays with the session and locks when
the machine has been left alone, and just before it sleeps. Start it with
the compositor.

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Type your password and press Enter. Esc or Ctrl+u clears the field.
After three wrong attempts each further one costs a wait, up to 30 seconds.

Configuration is optional: the [lock] table of ~/.config/enso/config.toml.
  clock            = \"%H:%M\"        the large clock (strftime)
  date             = \"%A, %-d %B\"   the line under it; \"\" for none
  wallpaper        = true           the theme's wallpaper, blurred, behind
                                    the clock; false for the plain colour
  screen_off_after = 30             seconds alone before the screens are
                                    powered off; 0 leaves them on
  suspend_after    = 0              seconds alone before the machine is
                                    suspended; 0 never
  power_saver      = true           hold the low-power profile while locked
  idle_after       = 300            for `watch`: seconds alone before
                                    locking; 0 never
  before_sleep     = true           for `watch`: lock before the machine sleeps

The wallpaper is the one enso-wall shows: see `enso-wall --help`.
";

fn main() -> ExitCode {
    let watch = match parse_args(lexopt::Parser::from_env()) {
        Ok(watch) => watch,
        Err(code) => return code,
    };

    // A lock that cannot check a password is a trap. Find out first.
    if let Err(err) = auth::available() {
        eprintln!("enso-lock: not locking: {err}");
        return ExitCode::FAILURE;
    }

    let mut ctx = Context::load();
    let config: lock::LockConfig = ctx.section("lock");
    ctx.report_warnings("enso-lock");
    let appearance = ctx.config.appearance.clone();
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);

    if watch {
        return match watch::run(&config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("enso-lock: {err}");
                ExitCode::FAILURE
            }
        };
    }

    // One lock screen at a time: a second would find the session locked
    // already, and would undo the first one's power saving on its way out.
    let _only = match enso_core::ipc::only("enso-lock") {
        enso_core::ipc::Only::Mine(guard) => guard,
        enso_core::ipc::Only::Taken => return ExitCode::SUCCESS,
    };

    let saver = config
        .power_saver
        .then(|| saver::Saver::engage(runtime_dir.as_deref()));
    let result = enso_ui::run_screens(lock::Lock::new(ctx, config), &appearance);
    // Back to how the machine was — whether it was unlocked or never locked.
    drop(saver);

    match result {
        Ok(()) => {
            // Whoever just typed the password should not find dark screens.
            lock::screens(Action::PowerOnMonitors {});
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-lock: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Parse the arguments: whether to watch rather than lock. `Err` carries
/// the exit code to stop with.
fn parse_args(mut parser: lexopt::Parser) -> Result<bool, ExitCode> {
    use lexopt::prelude::*;

    match parser.next() {
        Ok(None) => Ok(false),
        Ok(Some(Value(word))) if word == "watch" => Ok(true),
        Ok(Some(Short('h') | Long("help"))) => {
            print!("{HELP}");
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(Short('V') | Long("version"))) => {
            println!("enso-lock {}", env!("CARGO_PKG_VERSION"));
            Err(ExitCode::SUCCESS)
        }
        Ok(Some(arg)) => {
            eprintln!("enso-lock: {}\nTry 'enso-lock --help'.", arg.unexpected());
            Err(ExitCode::from(2))
        }
        Err(err) => {
            eprintln!("enso-lock: {err}");
            Err(ExitCode::from(2))
        }
    }
}
