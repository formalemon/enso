//! `enso-files` — the enso file manager, and in time the file picker every
//! application gets through the desktop portal.
//!
//! See `docs/plans/enso-files.md` for where this is going. What is here:
//! a window onto one folder at a time, listed in two passes so it is on
//! screen before its sizes are known ([`enso_fs::list`]), driven by keys or
//! the pointer alike.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bench;
mod bulk;
mod config;
mod frecent;
mod gather;
mod jobs;
mod kind;
mod location;
mod pane;
mod pick;
mod picker;
mod portal;
mod query;
mod recall;
mod selection;
mod service;
mod settings;
mod window;

use std::{
    env,
    io::{self, Read, Write},
    path::PathBuf,
    process::ExitCode,
};

use enso_core::Context;

const HELP: &str = "\
enso-files — files, the enso way

Usage: enso-files [FOLDER]
       enso-files pick [OPTIONS]
       enso-files portal enable | disable | status
       enso-files --service
       enso-files --bench list FOLDER

Opens FOLDER (the current folder, or home when started from elsewhere).

`enso-files portal enable` makes the picker every application's file
chooser, through the desktop portal, with nothing outside your home
directory; `portal disable` gives the choosing back; `portal status` says
which it is.

Options:
      --select PATH         Open PATH's folder with PATH selected (again for
                            more)
      --service             Be the desktop portal's file chooser (D-Bus starts
                            it, once `enso-files portal enable` has set it up)
      --bench list FOLDER   Time listing FOLDER, both passes, and sorting it
  -h, --help                Print this help
  -V, --version             Print the version

Keys:
  Up / Down, j / k, Ctrl+p / Ctrl+n   move          PgUp / PgDn   a page
  Enter, Right, l                     open          Home, g g     first
  Left, h, Backspace, Alt+Up          up a folder   End, G        last
  Alt+Left / Alt+Right, H / L         back / forward
  Space         choose (and move on)  Shift+arrows  choose a run
  Ctrl+A        choose all            *             turn the choice round
  Esc           clear the filter, then the choice (a picker: then cancel)
  /, Ctrl+F     filter as you type    Ctrl+H, .     show dotfiles
  ~, g h        home                  Ctrl+B        the sidebar
  Ctrl+R        reload                Menu          the menu for this row
  Ctrl+Shift+C  copy the path         Ctrl+Q        close

Mouse: click to go to, Ctrl+click and Shift+click to choose, double-click to
open, right-click for a menu, the path to go up, side buttons for back and
forward, double-click the title row to maximise.

Configuration is optional: the [files] table of ~/.config/enso/config.toml.
  hidden        = false        show dotfiles
  sort          = \"name\"       or size, modified, created, extension
  descending    = false
  folders_first = true
  density       = \"comfortable\" or compact
";

const PICK_HELP: &str = "\
enso-files pick — choose files, a folder, or where to save, and print it

Usage: enso-files pick [OPTIONS]

Prints each chosen path on its own line; exits 1 if nothing was chosen.

Options:
  -m, --multiple            Allow choosing several files, from any folders
  -d, --directory           Choose a folder instead of files
  -s, --save NAME           Choose where to save NAME
  -f, --filter SPEC         Offer a kind of file: NAME:PATTERN,PATTERN where a
                            pattern is a glob (*.png) or a MIME type
                            (image/webp); repeat for several
  -t, --title TITLE         The window's title
  -a, --accept LABEL        What the button that finishes says
  -C, --folder FOLDER       Where to start
  -0, --null                End each path with NUL instead of a newline
      --request             Read the whole request as TOML from stdin, and
                            print the answer as TOML (for the portal)

Keys: as in the file manager, and
  Enter         choose the file, go into the folder
  Ctrl+Enter    finish with what's chosen (or, for a folder, this one)
  Tab           between the list and the name, when saving
  Esc           clear the filter, then the choice, then cancel
";

enum Command {
    /// A folder to open, and things in it to show selected.
    Open(Option<PathBuf>, Vec<PathBuf>),
    Service,
    Portal(String),
    Pick(Box<Pick>),
    Bench(PathBuf),
}

struct Pick {
    request: pick::Request,
    null: bool,
    /// Request and answer as TOML, for the portal service.
    toml: bool,
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
        Command::Open(folder, select) => open(folder, select),
        Command::Pick(pick) => run_pick(*pick),
        Command::Service => service::run(),
        Command::Portal(command) => portal::run(&command),
    }
}

fn load() -> (Context, config::FilesConfig) {
    let mut ctx = Context::load();
    let config: config::FilesConfig = ctx.section("files");
    ctx.report_warnings("enso-files");
    (ctx, config)
}

fn open(folder: Option<PathBuf>, select: Vec<PathBuf>) -> ExitCode {
    let (ctx, config) = load();
    let select: Vec<PathBuf> = select
        .into_iter()
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .collect();
    let location = folder
        .or_else(|| {
            select
                .first()
                .and_then(|path| path.parent())
                .map(PathBuf::from)
        })
        .or_else(|| env::current_dir().ok())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .filter(|path| path.is_dir())
        .unwrap_or_else(enso_fs::places::home);
    let appearance = ctx.config.appearance.clone();
    let mut files = window::Files::new(ctx, config, location);
    files.preselect(&select);
    report(enso_ui::run(files, &appearance))
}

fn run_pick(pick: Pick) -> ExitCode {
    let (ctx, config) = load();
    let outcome = picker::Outcome::default();
    let appearance = ctx.config.appearance.clone();
    let files = window::Files::picker(ctx, config, pick.request, outcome.clone());
    let ran = report(enso_ui::run(files, &appearance));
    if ran != ExitCode::SUCCESS {
        return ran;
    }
    let Some(answer) = outcome.take() else {
        return ExitCode::from(1);
    };
    let mut out = io::stdout().lock();
    let written = if pick.toml {
        toml::to_string(&answer)
            .map_err(io::Error::other)
            .and_then(|text| out.write_all(text.as_bytes()))
    } else {
        answer.paths.iter().try_for_each(|path| {
            out.write_all(path.as_os_str().as_encoded_bytes())?;
            out.write_all(if pick.null { b"\0" } else { b"\n" })
        })
    };
    match written.and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-files: cannot write the answer: {err}");
            ExitCode::FAILURE
        }
    }
}

fn report(result: Result<(), enso_ui::Error>) -> ExitCode {
    match result {
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
    let mut select = Vec::new();
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => return Ok(Some(Command::Open(folder, select))),
            Some(Long("select")) => {
                select.push(PathBuf::from(
                    parser.value().map_err(|err| err.to_string())?,
                ));
            }
            Some(Short('h') | Long("help")) => {
                print!("{HELP}");
                return Ok(None);
            }
            Some(Short('V') | Long("version")) => {
                println!("enso-files {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some(Long("service")) => return Ok(Some(Command::Service)),
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
            Some(Value(word)) if folder.is_none() && word == "portal" => {
                let command = parser
                    .value()
                    .map_err(|_| "portal what? enable, disable, or status".to_owned())?;
                return Ok(Some(Command::Portal(
                    command.to_string_lossy().into_owned(),
                )));
            }
            Some(Value(word)) if folder.is_none() && word == "pick" => {
                return parse_pick(parser)
                    .map(|pick| pick.map(|pick| Command::Pick(Box::new(pick))));
            }
            Some(Value(path)) if folder.is_none() => folder = Some(PathBuf::from(path)),
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }
}

fn parse_pick(mut parser: lexopt::Parser) -> Result<Option<Pick>, String> {
    use lexopt::prelude::*;

    let text = |value: std::ffi::OsString| value.string().map_err(|err| err.to_string());
    let (mut multiple, mut directory, mut save) = (false, false, None);
    let mut filters = Vec::new();
    let (mut title, mut accept, mut start) = (String::new(), None, None);
    let (mut null, mut toml) = (false, false);
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => break,
            Some(Short('h') | Long("help")) => {
                print!("{PICK_HELP}");
                return Ok(None);
            }
            Some(Short('m') | Long("multiple")) => multiple = true,
            Some(Short('d') | Long("directory")) => directory = true,
            Some(Short('s') | Long("save")) => {
                save = Some(text(parser.value().map_err(|e| e.to_string())?)?)
            }
            Some(Short('f') | Long("filter")) => {
                let spec = text(parser.value().map_err(|e| e.to_string())?)?;
                filters.push(
                    pick::Filter::parse(&spec)
                        .ok_or_else(|| format!("no patterns in the filter `{spec}`"))?,
                );
            }
            Some(Short('t') | Long("title")) => {
                title = text(parser.value().map_err(|e| e.to_string())?)?
            }
            Some(Short('a') | Long("accept")) => {
                accept = Some(text(parser.value().map_err(|e| e.to_string())?)?)
            }
            Some(Short('C') | Long("folder")) => {
                start = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?))
            }
            Some(Short('0') | Long("null")) => null = true,
            Some(Long("request")) => toml = true,
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }
    let request = if toml {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| format!("cannot read the request: {err}"))?;
        toml::from_str(&input).map_err(|err| format!("the request doesn't read: {err}"))?
    } else {
        let mode = match save {
            Some(name) => pick::Mode::Save { name },
            None => pick::Mode::Open {
                multiple,
                directory,
            },
        };
        let mut request = pick::Request::new(mode);
        request.title = title;
        request.accept_label = accept;
        request.filters = filters;
        request.folder = start.map(|folder| std::path::absolute(&folder).unwrap_or(folder));
        request
    };
    Ok(Some(Pick {
        request,
        null,
        toml,
    }))
}
