//! `enso-play` — what's playing, under the bar's clock.
//!
//! A card with the cover, the title and who it's by, a timeline that waves
//! while it plays, and the buttons — for any media player on the session
//! bus (MPRIS): a YouTube tab in Firefox, Spotify, mpv. `enso-bar` opens it
//! while the pointer rests on the clock; a key binding opens it on purpose;
//! and the media keys can play, pause, and skip through it without a card.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod art;
mod card;
mod player;
mod wave;

use std::process::ExitCode;

use enso_core::{Context, ipc};
use enso_sys::mpris::{Mpris, Player, Status};

use crate::{
    card::{Card, Edge, Mode},
    player::Worker,
};

const HELP: &str = "\
enso-play — what's playing, under the bar's clock

Usage: enso-play [COMMAND]

With no command, opens the card, or closes it if it's open: one key
binding toggles it. Keys: Space play/pause, h/l or arrows move 5 s,
n/p next and previous, Tab the next player, Esc or q close.

Commands, for the media keys:
  play-pause    play if paused, pause if playing
  next          skip to the next track
  previous      back to the previous one
  status        say what's playing

They act on whichever player is playing, or the last one paused.

Options:
  --dropdown[=top|bottom]   hang the card under the bar, following the
                            pointer (what enso-bar runs)
  -h, --help                print this help
  -V, --version             print the version
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Card(Mode),
    PlayPause,
    Next,
    Previous,
    Status,
}

fn main() -> ExitCode {
    let command = match parse(std::env::args_os().skip(1)) {
        Ok(Some(command)) => command,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-play: {message}\nTry 'enso-play --help'.");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Card(mode) => open(mode),
        other => act(&other),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("enso-play: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Read the command line. `Ok(None)` when it asked for help or the version,
/// already printed.
fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Option<Command>, String> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut command = None;
    let mut mode = Mode::Modal;
    while let Some(arg) = parser.next().map_err(|err| err.to_string())? {
        match arg {
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(None);
            }
            Short('V') | Long("version") => {
                println!("enso-play {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Long("dropdown") => {
                let edge = parser.optional_value().map(|value| value.string());
                mode = Mode::Dropdown(match edge.transpose().map_err(|err| err.to_string())? {
                    None => Edge::Top,
                    Some(edge) if edge == "top" => Edge::Top,
                    Some(edge) if edge == "bottom" => Edge::Bottom,
                    Some(edge) => return Err(format!("no such edge: {edge}")),
                });
            }
            Value(word) if command.is_none() => {
                command = Some(
                    match word.string().map_err(|err| err.to_string())?.as_str() {
                        "play-pause" => Command::PlayPause,
                        "next" => Command::Next,
                        "previous" => Command::Previous,
                        "status" => Command::Status,
                        other => return Err(format!("no such command: {other}")),
                    },
                );
            }
            other => return Err(other.unexpected().to_string()),
        }
    }
    match command {
        None => Ok(Some(Command::Card(mode))),
        Some(_) if mode != Mode::Modal => Err("`--dropdown` only opens the card".to_owned()),
        Some(command) => Ok(Some(command)),
    }
}

fn open(mode: Mode) -> Result<(), String> {
    // One card at a time. Asked for again from the keyboard, the one that
    // is open closes; asked for from the bar, it's left as it is.
    let _only = match ipc::only(card::NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => {
            if mode == Mode::Modal {
                ipc::tell(card::NAME, "close");
            }
            return Ok(());
        }
    };
    let mpris = Mpris::connect().map_err(|err| err.to_string())?;
    let players = mpris.players().map_err(|err| err.to_string())?;
    // Resting on the clock with nothing playing opens nothing at all.
    if players.is_empty() && mode != Mode::Modal {
        return Ok(());
    }

    let ctx = Context::load();
    ctx.report_warnings("enso-play");
    let appearance = ctx.config.appearance.clone();
    let (worker, inbox) = Worker::new();
    let card = Card::new(ctx, mode, players, worker, Some((mpris, inbox)));
    enso_ui::run(card, &appearance).map_err(|err| err.to_string())
}

/// A media key's command: carried out on the player that matters most —
/// whichever plays, else the first paused.
fn act(command: &Command) -> Result<(), String> {
    let mpris = Mpris::connect().map_err(|err| err.to_string())?;
    let players = mpris.players().map_err(|err| err.to_string())?;
    let Some(player) = players.first() else {
        if *command == Command::Status {
            println!("Nothing playing");
            return Ok(());
        }
        return Err("nothing is playing".to_owned());
    };
    let done = match command {
        Command::PlayPause => mpris.play_pause(&player.bus),
        Command::Next => mpris.next(&player.bus),
        Command::Previous => mpris.previous(&player.bus),
        Command::Status => {
            println!("{}", status(player));
            Ok(())
        }
        Command::Card(_) => Ok(()),
    };
    done.map_err(|err| err.to_string())
}

/// One line: `Playing  Title — Artist  (Firefox)`.
fn status(player: &Player) -> String {
    let state = match player.status {
        Status::Playing => "Playing",
        Status::Paused => "Paused",
        Status::Stopped => "Stopped",
    };
    let mut line = format!("{state}  {}", player.track.title);
    if !player.track.artist.is_empty() {
        line.push_str(" — ");
        line.push_str(&player.track.artist);
    }
    line.push_str(&format!("  ({})", player.name));
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Result<Option<Command>, String> {
        parse(words.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn the_command_line_reads_as_written() {
        assert_eq!(words(&[]), Ok(Some(Command::Card(Mode::Modal))));
        assert_eq!(
            words(&["--dropdown"]),
            Ok(Some(Command::Card(Mode::Dropdown(Edge::Top))))
        );
        assert_eq!(
            words(&["--dropdown=bottom"]),
            Ok(Some(Command::Card(Mode::Dropdown(Edge::Bottom))))
        );
        assert_eq!(words(&["play-pause"]), Ok(Some(Command::PlayPause)));
        assert_eq!(words(&["next"]), Ok(Some(Command::Next)));
        assert!(words(&["--dropdown=left"]).is_err());
        assert!(words(&["--dropdown", "next"]).is_err());
        assert!(words(&["shuffle"]).is_err());
    }
}
