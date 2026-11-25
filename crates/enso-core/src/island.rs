//! Where the bar's clock island is, for what grows out of it.
//!
//! The bar on each screen keeps a one-line note of its clock island in
//! `$XDG_RUNTIME_DIR/enso-bar/<output>`: whose it is, which edge the bar is
//! on, how tall it is, the island's rectangle in logical pixels from the
//! bar's top left (for a bar along the top, the screen's), and how solid
//! the islands are drawn. The launcher reads it to open out of the
//! island instead of in the middle of the screen.
//!
//! Nothing depends on it: with no bar there is no note, and a note left by a
//! bar that has gone is told apart by its process, which is no longer there.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// The bar's clock island, as the bar last drew it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Island {
    /// The bar's process.
    pub pid: u32,
    /// Whether the bar is along the top of the screen (otherwise the bottom).
    pub top: bool,
    /// The bar's height, in logical pixels.
    pub bar: f32,
    /// The island: left, top, width, height, in logical pixels from the
    /// bar's top left.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// How solid the islands are drawn, `0..=1`.
    pub opacity: f32,
}

impl Island {
    /// The note, as written.
    pub fn line(&self) -> String {
        format!(
            "enso-bar pid={} edge={} bar={} x={} y={} w={} h={} opacity={}\n",
            self.pid,
            if self.top { "top" } else { "bottom" },
            self.bar,
            self.x,
            self.y,
            self.w,
            self.h,
            self.opacity
        )
    }

    /// Read a note. `None` if it isn't one.
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        (words.next()? == "enso-bar").then_some(())?;
        let mut island = Self {
            pid: 0,
            top: true,
            bar: 0.0,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            opacity: 1.0,
        };
        for word in words {
            let (key, value) = word.split_once('=')?;
            let number = || value.parse::<f32>().ok().filter(|n| n.is_finite());
            match key {
                "pid" => island.pid = value.parse().ok()?,
                "edge" => island.top = value == "top",
                "bar" => island.bar = number()?,
                "x" => island.x = number()?,
                "y" => island.y = number()?,
                "w" => island.w = number()?,
                "h" => island.h = number()?,
                "opacity" => island.opacity = number()?.clamp(0.0, 1.0),
                // Something a later bar says: not for us.
                _ => {}
            }
        }
        (island.pid != 0 && island.w > 0.0 && island.h > 0.0).then_some(island)
    }
}

/// Where the notes are kept.
fn dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("enso-bar"))
}

/// The note for the bar on `output`.
fn note(output: &str) -> Option<PathBuf> {
    // A screen's name is a connector's (`eDP-1`); anything that could step
    // out of the directory is not one.
    (!output.is_empty() && !output.contains(['/', '\0']) && output != "..")
        .then(|| dir().map(|dir| dir.join(output)))?
}

/// Keep `island` as the clock island of the bar on `output`.
pub fn publish(output: &str, island: &Island) -> io::Result<()> {
    let path = note(output).ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Written aside and moved into place: a reader sees the old note or the
    // new one, never half of one.
    let aside = path.with_extension(format!("{}.new", std::process::id()));
    fs::write(&aside, island.line())?;
    fs::rename(&aside, &path)
}

/// Let go of the note for `output`, if it is this process's.
pub fn withdraw(output: &str) {
    if let Some(path) = note(output)
        && read(&path).is_some_and(|island| island.pid == std::process::id())
    {
        let _ = fs::remove_file(path);
    }
}

/// The clock island of the bar on `output`, if that bar is running.
pub fn find(output: &str) -> Option<Island> {
    let island = read(&note(output)?)?;
    alive(island.pid).then_some(island)
}

fn read(path: &Path) -> Option<Island> {
    Island::parse(&fs::read_to_string(path).ok()?)
}

/// Whether `pid` is a running bar.
fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|name| name.trim() == "enso-bar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_read_back_as_written() {
        let island = Island {
            pid: 826,
            top: true,
            bar: 36.0,
            x: 812.5,
            y: 4.0,
            w: 295.0,
            h: 28.0,
            opacity: 0.9,
        };
        assert_eq!(Island::parse(&island.line()), Some(island));
        let bottom = Island {
            top: false,
            ..island
        };
        assert_eq!(Island::parse(&bottom.line()), Some(bottom));
        // Words a later bar adds are passed over.
        let later = island.line().replace('\n', " corner=9\n");
        assert_eq!(Island::parse(&later), Some(island));
    }

    #[test]
    fn what_isnt_a_note_is_refused() {
        assert_eq!(Island::parse(""), None);
        assert_eq!(Island::parse("enso-launch pid=3 w=1 h=1"), None);
        assert_eq!(Island::parse("enso-bar pid=3 w=0 h=28"), None);
        assert_eq!(Island::parse("enso-bar pid=x w=10 h=28"), None);
        assert_eq!(Island::parse("enso-bar pid=3 w=NaN h=28"), None);
        assert_eq!(Island::parse("enso-bar w=10 h=28"), None);
    }

    #[test]
    fn screen_names_stay_in_their_directory() {
        assert!(note("").is_none());
        assert!(note("..").is_none());
        assert!(note("../x").is_none());
        if env::var_os("XDG_RUNTIME_DIR").is_some_and(|dir| !dir.is_empty()) {
            assert!(note("eDP-1").is_some_and(|path| path.ends_with("enso-bar/eDP-1")));
        }
    }

    #[test]
    fn a_note_without_its_bar_is_not_found() {
        // No bar runs as this test, whatever its process is called.
        assert!(!alive(std::process::id()));
        assert!(!alive(u32::MAX));
    }
}
