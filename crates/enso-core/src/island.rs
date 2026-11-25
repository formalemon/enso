//! Where the bar's islands are, for what grows out of them.
//!
//! The bar on each screen keeps a one-line note of its islands in
//! `$XDG_RUNTIME_DIR/enso-bar/<output>`: whose it is, which edge the bar is
//! on, how tall and how wide it is, each island's rectangle in logical
//! pixels from the bar's top left (for a bar along the top, the screen's),
//! which island the pointer lights, and how solid the islands are drawn.
//! The launcher reads it to open out of the clock instead of in the middle
//! of the screen; the control center opens out of the status island, and
//! the notification history out of the bell's.
//!
//! Nothing depends on it: with no bar there is no note, and a note left by a
//! bar that has gone is told apart by its process, which is no longer there.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// Which of the bar's islands something grows out of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Which {
    /// The clock, in the middle.
    #[default]
    Clock,
    /// The status island at the right: network, battery, what's unusual.
    Status,
    /// The bell, on its own at the right end.
    Bell,
}

impl Which {
    pub fn word(self) -> &'static str {
        match self {
            Self::Clock => "clock",
            Self::Status => "status",
            Self::Bell => "bell",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "clock" => Some(Self::Clock),
            "status" => Some(Self::Status),
            "bell" => Some(Self::Bell),
            _ => None,
        }
    }
}

/// Where one island is: left, top, width, height, in logical pixels from
/// the bar's top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Spot {
    fn word(&self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.w, self.h)
    }

    fn parse(word: &str) -> Option<Self> {
        let mut numbers = word
            .split(',')
            .map(|n| n.parse::<f32>().ok().filter(|n| n.is_finite()));
        let mut next = || numbers.next().flatten();
        let spot = Self {
            x: next()?,
            y: next()?,
            w: next()?,
            h: next()?,
        };
        (spot.w > 0.0 && spot.h > 0.0).then_some(spot)
    }
}

/// The bar's clock island, as the bar last drew it, and the others with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
    /// The bar's width — the screen's, for a bar along an edge — or `0`
    /// when it didn't say.
    pub screen: f32,
    /// The status island, when the bar shows one.
    pub status: Option<Spot>,
    /// The bell's island.
    pub bell: Option<Spot>,
    /// The island the pointer is on, which the bar lights a shade.
    pub lit: Option<Which>,
}

impl Island {
    /// The note, as written.
    pub fn line(&self) -> String {
        let mut line = format!(
            "enso-bar pid={} edge={} bar={} x={} y={} w={} h={} opacity={}",
            self.pid,
            if self.top { "top" } else { "bottom" },
            self.bar,
            self.x,
            self.y,
            self.w,
            self.h,
            self.opacity
        );
        if self.screen > 0.0 {
            line += &format!(" screen={}", self.screen);
        }
        for (key, spot) in [("status", self.status), ("bell", self.bell)] {
            if let Some(spot) = spot {
                line += &format!(" {key}={}", spot.word());
            }
        }
        if let Some(lit) = self.lit {
            line += &format!(" lit={}", lit.word());
        }
        line + "\n"
    }

    /// The island `which`, if the bar has it: this note with that island's
    /// rectangle in place of the clock's.
    pub fn at(&self, which: Which) -> Option<Self> {
        let spot = match which {
            Which::Clock => return Some(*self),
            Which::Status => self.status?,
            Which::Bell => self.bell?,
        };
        Some(Self {
            x: spot.x,
            y: spot.y,
            w: spot.w,
            h: spot.h,
            ..*self
        })
    }

    /// Read a note. `None` if it isn't one.
    pub fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        (words.next()? == "enso-bar").then_some(())?;
        let mut island = Self {
            top: true,
            opacity: 1.0,
            ..Self::default()
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
                "screen" => island.screen = number()?,
                "status" => island.status = Some(Spot::parse(value)?),
                "bell" => island.bell = Some(Spot::parse(value)?),
                // An island this reader doesn't know is lit: none it knows.
                "lit" => island.lit = Which::parse(value),
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

/// The note of the bar on `output` (its clock island, and the others with
/// it), if that bar is running.
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
            ..Island::default()
        };
        assert_eq!(Island::parse(&island.line()), Some(island));
        // With the islands at the right, and one of them lit.
        let all = Island {
            screen: 1920.0,
            status: Some(Spot {
                x: 1700.0,
                y: 4.0,
                w: 168.0,
                h: 28.0,
            }),
            bell: Some(Spot {
                x: 1876.0,
                y: 4.0,
                w: 28.0,
                h: 28.0,
            }),
            lit: Some(Which::Bell),
            ..island
        };
        assert_eq!(Island::parse(&all.line()), Some(all));
        let bell = all.at(Which::Bell).expect("a bell");
        assert_eq!((bell.x, bell.w, bell.bar, bell.screen), (1876.0, 28.0, 36.0, 1920.0));
        assert_eq!(all.at(Which::Clock), Some(all));
        assert_eq!(island.at(Which::Status), None);
        // A light this reader doesn't know of lights nothing.
        let later = all.line().replace("lit=bell", "lit=tray");
        assert_eq!(Island::parse(&later).map(|island| island.lit), Some(None));
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
        assert_eq!(Island::parse("enso-bar pid=3 w=10 h=28 bell=1,2,3"), None);
        assert_eq!(Island::parse("enso-bar pid=3 w=10 h=28 bell=1,2,0,4"), None);
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
