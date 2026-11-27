//! `enso-view` — a viewer for pictures and videos. Opens one (or a
//! folder) in a window of its own, and steps through the others beside it:
//! swipe, zoom, turn, play, copy, make the wallpaper, or throw away.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod album;
mod draw;
mod item;
mod photo;
mod video;
mod viewer;

use std::{path::PathBuf, process::ExitCode};

use enso_core::Context;

const HELP: &str = "\
enso-view — pictures and videos, one at a time

Usage: enso-view [PICTURE | VIDEO | FOLDER]
       enso-view enable | disable

Shows PICTURE or VIDEO, and steps through the others in its folder (or
FOLDER's, or this one's). PNG, JPEG, WebP, and GIF (its first frame); a
photo is turned the way the camera was held. MP4, WebM, Matroska,
QuickTime, AVI, and Ogg videos play through ffmpeg, when it's installed.

`enable` makes it what opens pictures and videos (those you haven't
chosen something else for); `disable` gives them back.

Keys:
  Right, Left       next, previous       two fingers sideways  the same
  Home / End        first / last         Space, k   play or pause a video
  Shift+arrows      back, on 5 seconds   m          sound on or off
  + / -, wheel      zoom                 0, 1       fit, every pixel
  r / R             turn right / left    i          details
  Ctrl+C            copy it              w          make it the wallpaper
  Delete            move to the trash    Ctrl+Z     put it back
  e                 show it in Files     f, F11     full screen
  ?                 the keys             Esc, q     close

Options:
  -h, --help        Print this help
  -V, --version     Print the version
";

/// What it opens, as a desktop entry: no line in the launcher (there is
/// nothing to view without a picture), only the opening of pictures and
/// videos.
const ENTRY: &str = "enso-view.desktop";
const MIMES: [&str; 13] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "video/mp4",
    "video/x-m4v",
    "video/webm",
    "video/x-matroska",
    "video/quicktime",
    "video/x-msvideo",
    "video/ogg",
    "video/3gpp",
    "video/mpeg",
];

fn entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Viewer\n\
         Comment=Step through the pictures and videos in a folder\n\
         Exec={} %f\n\
         Icon=image-viewer\n\
         Terminal=false\n\
         NoDisplay=true\n\
         Categories=Graphics;AudioVideo;Viewer;\n\
         MimeType={};\n\
         StartupWMClass=enso-view\n",
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
                println!("enso-view {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Ok(Some(Value(word))) if target.is_none() && word == "enable" => return enable(),
            Ok(Some(Value(word))) if target.is_none() && word == "disable" => return disable(),
            Ok(Some(Value(path))) if target.is_none() => target = Some(PathBuf::from(path)),
            Ok(Some(arg)) => {
                eprintln!("enso-view: {}\nTry 'enso-view --help'.", arg.unexpected());
                return ExitCode::from(2);
            }
            Err(err) => {
                eprintln!("enso-view: {err}");
                return ExitCode::from(2);
            }
        }
    }
    let target = target
        .or_else(|| std::env::current_dir().ok())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .unwrap_or_default();
    let ctx = Context::load();
    ctx.report_warnings("enso-view");
    let appearance = ctx.config.appearance.clone();
    let album = album::Album::open(&target);
    match enso_ui::run(viewer::Viewer::new(ctx, album), &appearance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-view: {err}");
            ExitCode::FAILURE
        }
    }
}

fn enable() -> ExitCode {
    let exe = enso_core::exe::installed();
    match enso_fs::open::adopt(ENTRY, &entry(&exe), &MIMES) {
        Ok(taken) if taken.is_empty() => {
            println!("enso-view is set up; every kind it opens already had something chosen.");
            ExitCode::SUCCESS
        }
        Ok(taken) => {
            println!("These open in enso-view now: {}.", taken.join(", "));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-view: cannot set it up: {err}");
            ExitCode::FAILURE
        }
    }
}

fn disable() -> ExitCode {
    match enso_fs::open::abandon(ENTRY) {
        Ok(()) => {
            println!("Pictures and videos open as they did before enso-view.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-view: {err}");
            ExitCode::FAILURE
        }
    }
}
