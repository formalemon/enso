//! Keep awake: the idle lock held off — for a while, or until turned off.
//!
//! Chosen in the control center (or `enso-ctl awake`), kept as one small
//! file in the state directory, and read by whoever cares: the lock watcher,
//! which doesn't lock an idle machine while it's on, and the bar, which
//! shows a cup while it is. Like the theme, neither side needs the other.

use std::{fs, io};

use crate::{Paths, disk};

/// File name inside the state directory.
const FILE: &str = "awake";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Awake {
    /// The machine locks when left alone, as usual.
    #[default]
    Off,
    /// Kept awake until this moment, in seconds since the Unix epoch.
    Until(u64),
    /// Kept awake until turned off.
    On,
}

impl Awake {
    /// What was chosen; nothing recorded, or something unreadable, is off.
    pub fn read(paths: &Paths) -> Self {
        fs::read_to_string(paths.state_dir.join(FILE))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Record the choice for whoever is watching. `Off` removes the file.
    pub fn write(self, paths: &Paths) -> io::Result<()> {
        let path = paths.state_dir.join(FILE);
        match self {
            Self::Off => match fs::remove_file(&path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            },
            Self::Until(at) => disk::write_atomic(&path, format!("{at}\n").as_bytes(), 0o644),
            Self::On => disk::write_atomic(&path, b"on\n", 0o644),
        }
    }

    fn parse(text: &str) -> Self {
        match text.trim() {
            "on" => Self::On,
            number => number.parse().map_or(Self::Off, Self::Until),
        }
    }

    /// Whether it holds at `now` (seconds since the epoch).
    pub fn is_on(self, now: u64) -> bool {
        match self {
            Self::Off => false,
            Self::Until(at) => at > now,
            Self::On => true,
        }
    }

    /// When it ends by itself, if it does and hasn't yet.
    pub fn ends(self, now: u64) -> Option<u64> {
        match self {
            Self::Until(at) if at > now => Some(at),
            _ => None,
        }
    }
}

/// Seconds since the Unix epoch, now.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir;

    #[test]
    fn reads_what_was_written_and_ends_on_time() {
        let root = test_dir("awake");
        let paths = Paths::rooted(&root);
        fs::create_dir_all(&paths.state_dir).unwrap();
        assert_eq!(Awake::read(&paths), Awake::Off);

        Awake::Until(1_000).write(&paths).unwrap();
        assert_eq!(Awake::read(&paths), Awake::Until(1_000));
        assert!(Awake::read(&paths).is_on(999));
        assert!(!Awake::read(&paths).is_on(1_000));
        assert_eq!(Awake::Until(1_000).ends(10), Some(1_000));
        assert_eq!(Awake::Until(1_000).ends(2_000), None);

        Awake::On.write(&paths).unwrap();
        assert!(Awake::read(&paths).is_on(u64::MAX));
        assert_eq!(Awake::On.ends(0), None);

        Awake::Off.write(&paths).unwrap();
        Awake::Off.write(&paths).unwrap();
        assert_eq!(Awake::read(&paths), Awake::Off);
        assert_eq!(Awake::parse("soon"), Awake::Off);
        let _ = fs::remove_dir_all(root);
    }
}
