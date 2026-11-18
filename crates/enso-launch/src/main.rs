//! `enso-launch` — a keyboard-first launcher for applications and files.
//!
//! Opens focused as a bare search field, lists matches as you type, and gets
//! out of the way: `Enter` launches, `Esc` or a click outside dismisses,
//! arrows or `Ctrl+n`/`Ctrl+p` move the selection. Running it again while it
//! is open closes it, so one key binding toggles it.
//!
//! Applications come straight from `.desktop` entries ([`apps`]) and files
//! from a walk of the home directory ([`files`]); both are gathered on
//! background threads and remembered in `~/.cache/enso` so the next launch
//! has them at once. A file opens with the application the desktop
//! associates with it ([`open`]).

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod history;
mod launcher;

use std::{
    env, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::Instant,
};

use enso_core::Context;

const HELP: &str = "\
enso-launch — application and file launcher

Usage: enso-launch [OPTIONS]

Options:
      --list          Print launchable applications and exit
      --find <QUERY>  Print the files matching QUERY, with timings, and exit
  -h, --help          Print this help
  -V, --version       Print the version

Nothing is listed until you start typing: applications from the first
letter, files from the second. Running enso-launch while it is open closes it.

Keys:
  type             search               Enter        launch, or open the file
  Up / Ctrl+p      previous             Ctrl+Enter   open the file's folder
  Down / Ctrl+n    next                 Esc          close
  Ctrl+u / w       clear / delete word

Configuration is optional: the [launch] table of ~/.config/enso/config.toml.
  width       = 560            card width in pixels
  max_results = 8              rows shown before the list scrolls
  terminal    = \"alacritty\"    used for terminal applications
  files       = true           search files as well as applications
  file_roots  = [\"~\", \"~/.config\"]
                               directories whose files are searched
  hidden      = false          include dotfiles and dot-directories
  ignore      = []             names, globs or paths to leave out, e.g.
                               [\"*.iso\", \"Music/incoming\", \"~/VMs\"];
                               \"!name\" keeps what would be left out
  gitignore   = true           honour .gitignore and .ignore files, in a
                               repository or not
  default_ignores = true       leave out build output, dependency trees and
                               caches: node_modules, __pycache__, a target
                               beside a Cargo.toml, a dist beside a
                               package.json, *.o, *.pyc, and the like

Version-control internals (.git) are always left out.
";

enum Mode {
    Launcher,
    List,
    Find(String),
}

fn main() -> ExitCode {
    let mode = match parse_args() {
        Ok(mode) => mode,
        Err(code) => return code,
    };

    if let Mode::List = mode {
        for app in enso_fs::apps::load() {
            println!("{}\t{}", app.id, app.name);
        }
        return ExitCode::SUCCESS;
    }

    let mut ctx = Context::load();
    let config: launcher::LaunchConfig = ctx.section("launch");
    ctx.report_warnings("enso-launch");
    let appearance = ctx.config.appearance.clone();

    if let Mode::Find(query) = mode {
        find(&ctx, &config, &query);
        return ExitCode::SUCCESS;
    }

    // Invoked while already open: close that one instead of opening another.
    let socket =
        env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("enso-launch.sock"));
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

    let background = launcher::Background::default();
    let launcher = launcher::Launcher::new(ctx, config, background.clone(), listener);
    let result = enso_ui::run(launcher, &appearance);

    // The window is gone. Stop being "the open launcher" at once, then let
    // the file walk finish so the next launch starts from a fresh index.
    if let (true, Some(path)) = (listening, &socket) {
        let _ = fs::remove_file(path);
    }
    let pending: Vec<_> = background
        .lock()
        .map(|mut handles| handles.drain(..).collect())
        .unwrap_or_default();
    for handle in pending {
        let _ = handle.join();
    }

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-launch: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `--find`: run one file search from the terminal, saying how long each
/// step took. The same code paths the window uses, minus the window.
fn find(ctx: &Context, config: &launcher::LaunchConfig, query: &str) {
    let home = env::var("HOME").unwrap_or_default();
    let scope = config.scope(Path::new(&home));
    let cache = ctx.paths.cache_dir.join(launcher::FILE_INDEX);

    let started = Instant::now();
    let saved = enso_fs::search::FileIndex::read(&cache);
    if let Some(index) = &saved {
        eprintln!(
            "saved index: {} entries, read in {:.1?}",
            index.len(),
            started.elapsed()
        );
    }
    // Walk on another thread, as the launcher does, so the walk's timing is
    // not hidden behind the search's.
    let walking = Instant::now();
    let index = thread::scope(|threads| threads.spawn(|| enso_fs::search::walk(&scope)).join())
        .unwrap_or_default();
    eprintln!(
        "walked: {} entries in {:.1?}",
        index.len(),
        walking.elapsed()
    );
    if saved.as_ref() != Some(&index)
        && let Err(err) = index.write(&cache)
    {
        eprintln!("enso-launch: cannot save the file index: {err}");
    }

    let searching = Instant::now();
    let hits = index.search(query, 20);
    eprintln!("searched in {:.1?}", searching.elapsed());
    for hit in hits {
        let slash = if hit.is_dir { "/" } else { "" };
        println!("{:>4}  {}{slash}", hit.score, hit.path);
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
            Ok(Some(Long("find"))) => match parser.value().map(|value| value.string()) {
                Ok(Ok(query)) => mode = Mode::Find(query),
                Ok(Err(err)) | Err(err) => {
                    eprintln!("enso-launch: {err}");
                    return Err(ExitCode::from(2));
                }
            },
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
