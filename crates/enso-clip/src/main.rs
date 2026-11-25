//! `enso-clip` — clipboard history for Niri.
//!
//! Started with the session, it keeps what's copied — text and pictures —
//! for as long as the session lasts, in memory only, and keeps the newest
//! on the clipboard after the program that copied it has gone.
//! `enso-clip pick` (bound to Mod+V) opens a card to choose an earlier one.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod daemon;
mod picker;
mod store;

use std::process::ExitCode;

use enso_core::{Context, ipc};

const HELP: &str = "\
enso-clip — clipboard history

Usage: enso-clip [COMMAND]

With no command, runs: start it with the session.

Commands:
  pick      Open the history: type to narrow it down, Enter copies the one
            chosen (ready to paste), Delete forgets it; again to close
  list      Print the history, newest first, one line each
  clear     Forget everything

What's copied is kept in memory, in $XDG_RUNTIME_DIR/enso-clip (readable
by you alone), and never written to the disk; the last 50 things. What a
password manager marks as secret is never read.
";

fn pick() -> Result<(), String> {
    let _only = match ipc::only(picker::NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => {
            ipc::tell(picker::NAME, "close");
            return Ok(());
        }
    };
    let ctx = Context::load();
    ctx.report_warnings("enso-clip");
    let appearance = ctx.config.appearance.clone();
    // The screen it opens on, and the bar's clock there to open out of.
    let (screen, perch) = enso_ui::emerge::Perch::find();
    let picker = picker::Picker::new(ctx).on(screen, perch);
    enso_ui::run(picker, &appearance).map_err(|err| err.to_string())
}

fn list() -> Result<(), String> {
    let dir = store::dir().ok_or("no XDG_RUNTIME_DIR")?;
    for id in store::list(&dir) {
        match store::read(&dir, id) {
            Some(store::Clip::Text(text)) => {
                let line: String = text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(100)
                    .collect();
                println!("{id}\t{line}");
            }
            Some(store::Clip::Png(png)) => println!("{id}\t(picture, {} kB)", png.len() / 1024),
            None => {}
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => daemon::run(),
        ["pick"] => pick(),
        ["list"] => list(),
        ["clear"] => {
            ipc::tell(daemon::NAME, "clear");
            Ok(())
        }
        ["-h" | "--help"] => {
            print!("{HELP}");
            Ok(())
        }
        ["-V" | "--version"] => {
            println!("enso-clip {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [other, ..] => {
            eprintln!("enso-clip: unknown command `{other}`\nTry 'enso-clip --help'.");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-clip: {err}");
            ExitCode::FAILURE
        }
    }
}
