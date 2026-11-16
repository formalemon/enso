//! What the notification daemon tells the rest of the suite: how many
//! notifications went by unattended, and whether its history is open.
//!
//! Like the active theme, it is a small file in the state directory that
//! interested tools watch — the bar draws its bell from it — so neither side
//! needs the other to be running, or even installed.

use std::{fs, io};

use crate::Paths;

/// File name inside the state directory.
const FILE: &str = "notifications";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Notices {
    /// Notifications that expired unattended since the history was last open.
    pub unseen: u32,
    /// The history card is open.
    pub open: bool,
}

impl Notices {
    /// The current state; nothing recorded reads as nothing to report.
    pub fn read(paths: &Paths) -> Self {
        fs::read_to_string(paths.state_dir.join(FILE))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Record the state for whoever is watching.
    pub fn write(self, paths: &Paths) -> io::Result<()> {
        fs::create_dir_all(&paths.state_dir)?;
        fs::write(paths.state_dir.join(FILE), self.encode())
    }

    fn parse(text: &str) -> Self {
        let mut lines = text.lines().map(str::trim);
        Self {
            unseen: lines.next().and_then(|line| line.parse().ok()).unwrap_or(0),
            open: lines.next() == Some("open"),
        }
    }

    fn encode(self) -> String {
        let open = if self.open { "open" } else { "closed" };
        format!("{}\n{open}\n", self.unseen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_anything() {
        let busy = Notices {
            unseen: 3,
            open: true,
        };
        assert_eq!(Notices::parse(&busy.encode()), busy);
        assert_eq!(
            Notices::parse(&Notices::default().encode()),
            Notices::default()
        );
        assert_eq!(Notices::parse(""), Notices::default());
        assert_eq!(Notices::parse("many\nperhaps\n"), Notices::default());
        assert_eq!(
            Notices::parse("7"),
            Notices {
                unseen: 7,
                open: false
            }
        );
    }

    #[test]
    fn reads_what_was_written() {
        let root = std::env::temp_dir().join(format!("enso-core-notices-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths::rooted(&root);
        assert_eq!(Notices::read(&paths), Notices::default());
        let state = Notices {
            unseen: 2,
            open: false,
        };
        state.write(&paths).expect("write state");
        assert_eq!(Notices::read(&paths), state);
        fs::remove_dir_all(root).expect("clean up");
    }
}
