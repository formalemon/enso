//! `enso-greet` — the login screen, as a greeter for greetd.
//!
//! greetd owns everything that matters for security: the console, PAM, and
//! starting the session as the right user. It starts this program (inside a
//! niri of its own, as its `greeter` user) to ask who is logging in and to
//! pass PAM's questions on. Once the user is in, this asks greetd for their
//! session, quits its niri, and greetd starts the session in its place.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod greet;
mod greetd;
mod setup;

use std::{env, fs, path::PathBuf, process::ExitCode};

use enso_core::{Context, Paths};
use niri_ipc::{Action, Request, socket::Socket};

use crate::{
    greet::{Greet, GreetConfig},
    greetd::{Greetd, Pretend, Wire},
};

const HELP: &str = "\
enso-greet — the login screen, for greetd

Usage: enso-greet             run as greetd's greeter (inside its niri)
       enso-greet preview     try it here: the password is \"enso\", and Esc
                              leaves; nobody is logged in
       enso-greet enable      make it the login screen
       enso-greet disable     how to go back to the console's login

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Type the password and press Enter. Tab, or the arrow keys, choose another
user. Esc clears the field.

Settings are optional, in /etc/enso/greet.toml (it runs before anyone has
logged in, so a home's config is out of reach):
  default_user = \"\"              who is offered first; \"\" for the first
  command      = \"niri-session\"  the session, started by the user's login
                                 shell so their profile sets it up
The look follows /etc/enso/config.toml if there is one, the defaults if not.
";

enum Run {
    Greet,
    Preview,
    Enable,
    Disable,
}

fn main() -> ExitCode {
    let run = match parse_args(lexopt::Parser::from_env()) {
        Ok(run) => run,
        Err(code) => return code,
    };
    match run {
        Run::Greet => greet(),
        Run::Preview => preview(),
        Run::Enable => enable(),
        Run::Disable => disable(),
    }
}

/// Settings and looks for a screen nobody has logged in to yet.
fn system_context() -> (Context, GreetConfig) {
    let paths = Paths {
        config_dir: PathBuf::from(setup::SYSTEM_DIR),
        // Nothing is chosen or remembered before a login.
        state_dir: PathBuf::from("/nonexistent/enso-greet"),
        cache_dir: PathBuf::from("/nonexistent/enso-greet"),
        data_dirs: vec![
            PathBuf::from("/usr/local/share/enso"),
            PathBuf::from("/usr/share/enso"),
        ],
    };
    let ctx = Context::load_from(paths);
    ctx.report_warnings("enso-greet");
    let config = match fs::read_to_string(setup::GREET_CONFIG) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!(
                "enso-greet: {}: {err}; using the defaults",
                setup::GREET_CONFIG
            );
            GreetConfig::default()
        }),
        Err(_) => GreetConfig::default(),
    };
    (ctx, config)
}

/// Ask the greeter's niri to quit, so that greetd carries on: with the
/// session if it was asked for, with a new greeter if not.
fn quit_niri() {
    if let Ok(mut socket) = Socket::connect() {
        let _ = socket.send(Request::Action(Action::Quit {
            skip_confirmation: true,
        }));
    }
}

fn greet() -> ExitCode {
    let Some(sock) = env::var_os("GREETD_SOCK") else {
        eprintln!(
            "enso-greet: not started by greetd ($GREETD_SOCK is unset).\n\
             Try 'enso-greet preview' to see it, or 'enso-greet enable'."
        );
        return ExitCode::FAILURE;
    };
    // Whatever happens from here, the greeter's niri goes with it: greetd
    // then starts a fresh greeter rather than leaving a blank screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        quit_niri();
    }));
    let code = match Wire::connect(sock.as_ref()) {
        Ok(wire) => show(Box::new(wire), false),
        Err(err) => {
            eprintln!("enso-greet: cannot reach greetd: {err}");
            ExitCode::FAILURE
        }
    };
    quit_niri();
    code
}

fn preview() -> ExitCode {
    show(Box::new(Pretend::default()), true)
}

fn show(greetd: Box<dyn Greetd>, preview: bool) -> ExitCode {
    let (ctx, config) = if preview {
        let ctx = Context::load();
        ctx.report_warnings("enso-greet");
        let config = GreetConfig {
            default_user: env::var("USER").unwrap_or_default(),
            ..GreetConfig::default()
        };
        (ctx, config)
    } else {
        system_context()
    };
    let users = enso_sys::accounts::login_users();
    let appearance = ctx.config.appearance.clone();
    match enso_ui::run_screens(Greet::new(ctx, config, users, greetd, preview), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-greet: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Where `enable` leaves the files for root.
fn staging() -> PathBuf {
    Paths::from_env()
        .data_dirs
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("greet")
}

fn enable() -> ExitCode {
    if !PathBuf::from("/usr/bin/greetd").exists()
        && !PathBuf::from("/usr/local/bin/greetd").exists()
    {
        eprintln!("enso-greet: greetd isn't installed (pacman -S greetd).");
        return ExitCode::FAILURE;
    }
    let ctx = Context::load();
    ctx.report_warnings("enso-greet");
    let program = env::current_exe()
        .and_then(fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("enso-greet"));
    let user = env::var("USER").unwrap_or_default();
    let fonts = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|data| data.join("fonts"));
    let family = ctx.config.appearance.font.clone();
    let private_font = fonts
        .and_then(|fonts| setup::private_font(&fonts, &family))
        .map(|dir| (family, dir));
    let setup = setup::Setup {
        dir: staging(),
        program,
        user,
        base: ctx.theme.colors.base.hex(),
        private_font,
    };
    match setup::enable(&setup) {
        Ok(said) => {
            print!("{said}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-greet: {}: {err}", setup.dir.display());
            ExitCode::FAILURE
        }
    }
}

fn disable() -> ExitCode {
    match setup::disable(&staging()) {
        Ok(said) => {
            print!("{said}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-greet: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Parse the arguments. `Err` carries the exit code to stop with.
fn parse_args(mut parser: lexopt::Parser) -> Result<Run, ExitCode> {
    use lexopt::prelude::*;

    let run = match parser.next() {
        Ok(None) => return Ok(Run::Greet),
        Ok(Some(Value(word))) => match word.to_str() {
            Some("preview") => Run::Preview,
            Some("enable") => Run::Enable,
            Some("disable") => Run::Disable,
            _ => {
                eprintln!("enso-greet: unknown command {word:?}\nTry 'enso-greet --help'.");
                return Err(ExitCode::from(2));
            }
        },
        Ok(Some(Short('h') | Long("help"))) => {
            print!("{HELP}");
            return Err(ExitCode::SUCCESS);
        }
        Ok(Some(Short('V') | Long("version"))) => {
            println!("enso-greet {}", env!("CARGO_PKG_VERSION"));
            return Err(ExitCode::SUCCESS);
        }
        Ok(Some(arg)) => {
            eprintln!("enso-greet: {}\nTry 'enso-greet --help'.", arg.unexpected());
            return Err(ExitCode::from(2));
        }
        Err(err) => {
            eprintln!("enso-greet: {err}");
            return Err(ExitCode::from(2));
        }
    };
    match parser.next() {
        Ok(None) => Ok(run),
        Ok(Some(arg)) => {
            eprintln!("enso-greet: {}\nTry 'enso-greet --help'.", arg.unexpected());
            Err(ExitCode::from(2))
        }
        Err(err) => {
            eprintln!("enso-greet: {err}");
            Err(ExitCode::from(2))
        }
    }
}
