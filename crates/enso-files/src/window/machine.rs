//! What the window asks of the machine while it draws: the time, how full
//! a disk is, whether the trash has anything in it. Asked through here,
//! so that tests can answer for the machine and draw the same window on
//! any of them, at any hour.

use std::{io, path::Path};

use enso_fs::places::{self, Space};

#[derive(Clone, Copy)]
pub(super) struct Machine {
    /// Now, in the time zone dates are shown in.
    pub now: fn() -> jiff::Zoned,
    /// How full the filesystem holding a path is.
    pub space: fn(&Path) -> io::Result<Space>,
    /// Whether the trash has anything in it.
    pub trash_full: fn() -> bool,
}

impl Machine {
    /// This machine, as it is.
    pub fn real() -> Self {
        Self {
            now: jiff::Zoned::now,
            space: places::space,
            trash_full: || {
                std::fs::read_dir(super::Files::trash_dir())
                    .is_ok_and(|mut entries| entries.next().is_some())
            },
        }
    }
}

impl std::fmt::Debug for Machine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Machine")
    }
}
