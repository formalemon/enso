//! `enso-wall` — the wallpaper: one picture per theme, and a default for the
//! themes that have none.
//!
//! Run it once at login and it stays behind every window on every screen.
//! It follows `enso-ctl theme set` live, fading from one theme's picture to
//! the next, and `enso-wall set` changes a picture without a restart. With
//! nothing set at all it shows the theme's base colour, so it is useful from
//! the first run.
//!
//! At rest it holds no pixels: a frame is drawn, handed to the compositor,
//! and let go. The process then sleeps until the theme, a wallpaper choice,
//! or a monitor changes.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cache;
mod wall;

use std::{
    env, fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::ExitCode,
};

use enso_core::{
    Context,
    wallpaper::{self, Chosen, Source, WallConfig},
};
use enso_ui::Picture;

const HELP: &str = "\
enso-wall — wallpaper, one picture per theme

Usage: enso-wall [COMMAND]

Commands:
  (none)                        Show the wallpaper and follow theme changes
  set <IMAGE> [--theme <ID>]    Use IMAGE for theme ID; without --theme, as
                                the default for themes that have no picture
  set <IMAGE> --current         Use IMAGE for the theme that is active now
  unset [--theme <ID> | --current]
                                Forget a picture chosen with `set`
  list                          Show which picture each theme gets, and why

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Pictures are PNG, JPEG, WebP, or GIF. A running enso-wall follows `set`, `unset`, and
`enso-ctl theme set` at once, fading between pictures.

Pictures can also be declared in ~/.config/enso/config.toml; a picture
chosen with `set` wins over the one declared for the same slot.
  [wall]
  default = \"~/Pictures/walls/quiet.jpg\"
  fit     = \"cover\"            \"cover\" fills the screen and crops;
                               \"contain\" shows the whole picture
  [wall.themes]
  rose-pine-dawn = \"~/Pictures/walls/morning.png\"

A theme with no picture and no default shows its base colour.
";

/// Which slot a `set` or `unset` is about.
#[derive(Debug, PartialEq, Eq)]
enum Slot {
    Default,
    Theme(String),
    /// Whichever theme is active.
    Current,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    Set { image: PathBuf, slot: Slot },
    Unset { slot: Slot },
    List,
}

fn main() -> ExitCode {
    let command = match parse_args(lexopt::Parser::from_env()) {
        Ok(Some(command)) => command,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-wall: {message}\nTry 'enso-wall --help'.");
            return ExitCode::from(2);
        }
    };

    let mut ctx = Context::load();
    let config: WallConfig = ctx.section("wall");
    ctx.report_warnings("enso-wall");

    let result = match command {
        Command::Run => return run(ctx, config),
        Command::Set { image, slot } => set(&ctx, &image, slot),
        Command::Unset { slot } => unset(&ctx, slot),
        Command::List => {
            list(&ctx, &config);
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-wall: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(ctx: Context, config: WallConfig) -> ExitCode {
    // Two wallpapers would only paint over each other. The socket is the
    // claim: whoever can connect to it knows one is already up.
    let socket = env::var_os("XDG_RUNTIME_DIR").map(|dir| {
        let display = env::var("WAYLAND_DISPLAY").unwrap_or_default();
        PathBuf::from(dir).join(format!("enso-wall-{display}.sock"))
    });
    let _claim = match &socket {
        Some(path) if UnixStream::connect(path).is_ok() => {
            eprintln!("enso-wall: already running");
            return ExitCode::SUCCESS;
        }
        Some(path) => {
            // Nobody is listening, so anything there is left over.
            let _ = fs::remove_file(path);
            UnixListener::bind(path).ok()
        }
        None => None,
    };

    let appearance = ctx.config.appearance.clone();
    let result = enso_ui::run_screens(wall::Wall::new(ctx, config), &appearance);
    if let Some(path) = &socket {
        let _ = fs::remove_file(path);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-wall: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The theme id a slot names, checked against the themes that exist.
fn theme_of(ctx: &Context, slot: Slot) -> Result<Option<String>, String> {
    match slot {
        Slot::Default => Ok(None),
        Slot::Current => Ok(Some(ctx.theme.id.clone())),
        Slot::Theme(id) if ctx.themes.get(&id).is_some() => Ok(Some(id)),
        Slot::Theme(id) => {
            let known: Vec<&str> = ctx.themes.iter().map(|theme| theme.id.as_str()).collect();
            Err(format!("unknown theme `{id}` (have: {})", known.join(", ")))
        }
    }
}

fn describe(theme: Option<&str>) -> String {
    theme.map_or_else(|| "the default".to_owned(), |id| format!("theme {id}"))
}

fn set(ctx: &Context, image: &Path, slot: Slot) -> Result<(), String> {
    let theme = theme_of(ctx, slot)?;
    // Recorded as an absolute path: the wallpaper is drawn by a process
    // started somewhere else entirely.
    let image = std::path::absolute(image).map_err(|err| format!("{}: {err}", image.display()))?;
    // Refuse now what could not be shown later.
    let picture = Picture::open(&image)?;
    let mut chosen = Chosen::load(&ctx.paths);
    chosen.set(theme.as_deref(), &image);
    chosen.save(&ctx.paths).map_err(|err| err.to_string())?;
    let (width, height) = picture.size();
    println!(
        "{} → {} ({width}×{height})",
        describe(theme.as_deref()),
        image.display()
    );
    Ok(())
}

fn unset(ctx: &Context, slot: Slot) -> Result<(), String> {
    let theme = theme_of(ctx, slot)?;
    let mut chosen = Chosen::load(&ctx.paths);
    if !chosen.unset(theme.as_deref()) {
        println!("{}: nothing was set", describe(theme.as_deref()));
        return Ok(());
    }
    chosen.save(&ctx.paths).map_err(|err| err.to_string())?;
    println!("{}: forgotten", describe(theme.as_deref()));
    Ok(())
}

fn list(ctx: &Context, config: &WallConfig) {
    let chosen = Chosen::load(&ctx.paths);
    for theme in ctx.themes.iter() {
        let marker = if theme.id == ctx.theme.id { "●" } else { " " };
        let picture = match wallpaper::resolve(config, &chosen, &theme.id) {
            Some((path, Source::Theme)) => path.display().to_string(),
            Some((path, Source::Default)) => format!("{}  (default)", path.display()),
            None => format!("{}  (no picture: base colour)", theme.colors.base),
        };
        println!("{marker} {:<18} {picture}", theme.id);
    }
}

/// Parse the command line. `Ok(None)` means it was answered already
/// (`--help`, `--version`).
fn parse_args(mut parser: lexopt::Parser) -> Result<Option<Command>, String> {
    use lexopt::prelude::*;

    let mut words: Vec<String> = Vec::new();
    let mut slot = Slot::Default;
    loop {
        match parser.next().map_err(|err| err.to_string())? {
            None => break,
            Some(Short('h') | Long("help")) => {
                print!("{HELP}");
                return Ok(None);
            }
            Some(Short('V') | Long("version")) => {
                println!("enso-wall {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some(Long("theme")) => {
                let id = parser.value().map_err(|err| err.to_string())?;
                slot = Slot::Theme(id.string().map_err(|err| err.to_string())?);
            }
            Some(Long("current")) => slot = Slot::Current,
            Some(Value(word)) => words.push(word.string().map_err(|err| err.to_string())?),
            Some(arg) => return Err(arg.unexpected().to_string()),
        }
    }

    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        [] if slot == Slot::Default => Command::Run,
        ["set", image] => Command::Set {
            image: PathBuf::from(image),
            slot,
        },
        ["set"] => return Err("`set` needs an image".to_owned()),
        ["unset"] => Command::Unset { slot },
        ["list"] if slot == Slot::Default => Command::List,
        [] | ["list"] => return Err("--theme and --current go with `set` or `unset`".to_owned()),
        [other, ..] => return Err(format!("unexpected `{other}`")),
    };
    Ok(Some(command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Command>, String> {
        parse_args(lexopt::Parser::from_args(args.iter().copied()))
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&[]), Ok(Some(Command::Run)));
        assert_eq!(parse(&["list"]), Ok(Some(Command::List)));
        assert_eq!(
            parse(&["set", "a.png"]),
            Ok(Some(Command::Set {
                image: "a.png".into(),
                slot: Slot::Default
            }))
        );
        assert_eq!(
            parse(&["set", "--theme", "rose-pine-dawn", "a.png"]),
            Ok(Some(Command::Set {
                image: "a.png".into(),
                slot: Slot::Theme("rose-pine-dawn".to_owned())
            }))
        );
        assert_eq!(
            parse(&["unset", "--current"]),
            Ok(Some(Command::Unset {
                slot: Slot::Current
            }))
        );
        assert!(parse(&["set"]).is_err());
        assert!(parse(&["--theme", "x"]).is_err());
        assert!(parse(&["frobnicate"]).is_err());
        assert!(parse(&["--frobnicate"]).is_err());
    }

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<WallConfig>("wall").unwrap(),
            WallConfig::default()
        );
    }
}
