//! `enso-listen` — a player for sound files. Opens one (or a folder) and
//! plays the folder through: the cover large, a wavy timeline, the tracks
//! below. One copy at a time: opening another file hands it to the copy
//! that's playing.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cover;
mod listener;
mod queue;

use std::{path::PathBuf, process::ExitCode};

use enso_core::{Context, ipc};

const HELP: &str = "\
enso-listen — sound files, one after another

Usage: enso-listen [FILE | FOLDER]
       enso-listen enable | disable

Plays FILE, and the folder's other sound files after it (or FOLDER's
from its first, or this one's): MP3, FLAC, Ogg, Opus, AAC, WAV, and the
rest, through ffmpeg. It's a player like any other to the desktop: the
now-playing card under the bar's clock shows it, and the media keys reach
it. Opened again with another file, the one already open plays it.

`enable` makes it what opens sound files (those you haven't chosen
something else for); `disable` gives them back.

Keys:
  Space, k          play or pause        n / p, ↓ / ↑   next, previous
  ← / →, h / l      back, on 5 seconds   m              sound on or off
  Home / End        first, last          e              show it in Files
  two fingers sideways on the cover      the next, the one before
  Esc, q            close

Options:
  -h, --help        Print this help
  -V, --version     Print the version
";

const ENTRY: &str = "enso-listen.desktop";
const MIMES: [&str; 14] = [
    "audio/mpeg",
    "audio/mp3",
    "audio/flac",
    "audio/x-flac",
    "audio/ogg",
    "audio/x-vorbis+ogg",
    "audio/x-opus+ogg",
    "audio/opus",
    "audio/mp4",
    "audio/x-m4a",
    "audio/aac",
    "audio/wav",
    "audio/x-wav",
    "audio/x-aiff",
];

fn entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Listen\n\
         Comment=Play the sound files in a folder, one after another\n\
         Exec={} %f\n\
         Icon=multimedia-audio-player\n\
         Terminal=false\n\
         NoDisplay=true\n\
         Categories=AudioVideo;Audio;Player;\n\
         MimeType={};\n\
         StartupWMClass=enso-listen\n",
        exe.display(),
        MIMES.join(";")
    )
}

fn main() -> ExitCode {
    let mut parser = lexopt::Parser::from_env();
    let mut target: Option<PathBuf> = None;
    loop {
        use lexopt::prelude::*;
        match parser.next() {
            Ok(None) => break,
            Ok(Some(Short('h') | Long("help"))) => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            Ok(Some(Short('V') | Long("version"))) => {
                println!("enso-listen {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Ok(Some(Value(word))) if target.is_none() && word == "enable" => return enable(),
            Ok(Some(Value(word))) if target.is_none() && word == "disable" => return disable(),
            Ok(Some(Value(path))) if target.is_none() => target = Some(PathBuf::from(path)),
            Ok(Some(arg)) => {
                eprintln!(
                    "enso-listen: {}\nTry 'enso-listen --help'.",
                    arg.unexpected()
                );
                return ExitCode::from(2);
            }
            Err(err) => {
                eprintln!("enso-listen: {err}");
                return ExitCode::from(2);
            }
        }
    }
    let target = target
        .or_else(|| std::env::current_dir().ok())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .unwrap_or_default();
    // One at a time: a copy already playing plays this instead.
    let _only = match ipc::only(listener::NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => {
            if ipc::tell(listener::NAME, &format!("open {}", target.display())) {
                return ExitCode::SUCCESS;
            }
            None
        }
    };
    let ctx = Context::load();
    ctx.report_warnings("enso-listen");
    let appearance = ctx.config.appearance.clone();
    let queue = queue::Queue::open(&target);
    match enso_ui::run(listener::Listener::new(ctx, queue), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-listen: {err}");
            ExitCode::FAILURE
        }
    }
}

fn enable() -> ExitCode {
    let exe = enso_core::exe::installed();
    match enso_fs::open::adopt(ENTRY, &entry(&exe), &MIMES) {
        Ok(taken) if taken.is_empty() => {
            println!(
                "enso-listen is set up; every kind of sound file already had a player chosen."
            );
            ExitCode::SUCCESS
        }
        Ok(taken) => {
            println!("These play in enso-listen now: {}.", taken.join(", "));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-listen: cannot set it up: {err}");
            ExitCode::FAILURE
        }
    }
}

fn disable() -> ExitCode {
    match enso_fs::open::abandon(ENTRY) {
        Ok(()) => {
            println!("Sound files open as they did before enso-listen.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-listen: {err}");
            ExitCode::FAILURE
        }
    }
}
