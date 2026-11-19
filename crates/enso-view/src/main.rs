//! `enso-view` — an image viewer. Opens a picture (or a folder) in a
//! window of its own, and steps through the others beside it: zoom, turn,
//! copy, make the wallpaper, or throw away, from the keys.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod album;
mod photo;
mod viewer;

use std::{path::PathBuf, process::ExitCode};

use enso_core::Context;

const HELP: &str = "\
enso-view — pictures, one at a time

Usage: enso-view [PICTURE | FOLDER]
       enso-view enable | disable

Shows PICTURE, and steps through the others in its folder (or FOLDER's,
or this one's). PNG, JPEG, WebP, and GIF (its first frame); a photo is
turned the way the camera was held.

`enable` makes it what opens pictures (those you haven't chosen something
else for); `disable` gives them back.

Keys:
  Right, Space, l   next                 + / -, wheel   zoom
  Left, Backspace   previous             0              fit the window
  Home / End        first / last         1, double-click every pixel
  r / R             turn right / left    drag           move about
  Ctrl+C            copy the picture     w              make it the wallpaper
  Delete            move to the trash    Ctrl+Z         put it back
  e                 show it in Files     f              maximise
  Esc, q            close

Options:
  -h, --help        Print this help
  -V, --version     Print the version
";

/// What it opens, as a desktop entry: no line in the launcher (there is
/// nothing to view without a picture), only the opening of pictures.
const ENTRY: &str = "enso-view.desktop";
const MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

fn entry(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Image Viewer\n\
         Comment=Step through the pictures in a folder\n\
         Exec={} %f\n\
         Icon=image-viewer\n\
         Terminal=false\n\
         NoDisplay=true\n\
         Categories=Graphics;Viewer;\n\
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
            println!("enso-view is set up; every kind of picture already had a viewer chosen.");
            ExitCode::SUCCESS
        }
        Ok(taken) => {
            println!("Pictures open in enso-view now: {}.", taken.join(", "));
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
            println!("Pictures open as they did before enso-view.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("enso-view: {err}");
            ExitCode::FAILURE
        }
    }
}
