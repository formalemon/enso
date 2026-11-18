//! `enso-files` — the enso file manager, and in time the file picker every
//! application gets through the desktop portal.
//!
//! See `docs/plans/enso-files.md` for where this is going. What is here:
//! a window onto one folder at a time, listed in two passes so it is on
//! screen before its sizes are known ([`enso_fs::list`]), driven by keys or
//! the pointer alike.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bench;
mod config;
mod jobs;
mod pane;
mod window;

use std::{env, path::PathBuf, process::ExitCode};

use enso_core::Context;

const HELP: &str = "\
enso-files — files, the enso way

Usage: enso-files [FOLDER]
       enso-files --bench list FOLDER

Opens FOLDER (the current folder, or home when started from elsewhere).

Options:
      --bench list FOLDER   Time listing FOLDER, both passes, and sorting it
  -h, --help                Print this help
  -V, --version             Print the version

Keys:
  Up / Down, j / k, Ctrl+p / Ctrl+n   move          PgUp / PgDn   a page
  Enter, Right, l                     open          Home, g g     first
  Left, h, Backspace, Alt+Up          up a folder   End, G        last
  Alt+Left / Alt+Right, H / L         back / forward
  ~, g h        home                  Ctrl+H, .     show dotfiles
  Ctrl+R        reload                Ctrl+Q        close

Mouse: click to choose, double-click to open, the path to go up, side
buttons for back and forward, double-click the title row to maximise.

Configuration is optional: the [files] table of ~/.config/enso/config.toml.
  hidden        = false        show dotfiles
  sort          = \"name\"       or size, modified, created, extension
  descending    = false
  folders_first = true
  density       = \"comfortable\" or compact
";

enum Command {
    Open(Option<PathBuf>),
    Bench(PathBuf),
}

fn main() -> ExitCode {
    let command = match parse_args(lexopt::Parser::from_env()) {
        Ok(Some(command)) => command,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-files: {message}\nTry 'enso-files --help'.");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Bench(folder) => bench::list(&folder),
        Command::Open(folder) => open(folder),
    }
}

fn open(folder: Option<PathBuf>) -> ExitCode {
    let mut ctx = Context::load();
    let config: config::FilesConfig = ctx.section("files");
    ctx.report_warnings("enso-files");

    let location = folder
        .or_else(|| env::current_dir().ok())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .filter(|path| path.is_dir())
        .unwrap_or_else(enso_fs::places::home);

    let appearance = ctx.config.appearance.clone();
    match enso_ui::run(window::Files::new(ctx, config, location), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-files: {err}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(mut parser: lexopt::Parser) -> Result<Option<Command>, String> {
    use lexopt::prelude::*;

    let mut folder = None;
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => return Ok(Some(Command::Open(folder))),
            Some(Short('h') | Long("help")) => {
                print!("{HELP}");
                return Ok(None);
            }
            Some(Short('V') | Long("version")) => {
                println!("enso-files {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some(Long("bench")) => {
                let what = parser.value().map_err(|err| err.to_string())?;
                if what != "list" {
                    return Err(format!(
                        "nothing to bench called `{}`",
                        what.to_string_lossy()
                    ));
                }
                let folder = parser.value().map_err(|err| err.to_string())?;
                return Ok(Some(Command::Bench(folder.into())));
            }
            Some(Value(path)) if folder.is_none() => folder = Some(PathBuf::from(path)),
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }
}
