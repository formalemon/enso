//! The control center, opened as a dropdown under the status island.
//!
//! The bar does not draw the panel: it runs `enso-ctl --dropdown` and tells
//! it, a line at a time on its standard input, whether the pointer is on the
//! island (`enter`, `leave`). The panel closes itself once the pointer is on
//! neither the island nor the panel. Without `enso-ctl` installed, hovering
//! simply does nothing — the bar stays a complete tool on its own.

use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
};

use crate::config::Position;

#[derive(Debug)]
pub struct Dropdown {
    program: PathBuf,
    running: Option<(Child, ChildStdin)>,
    /// The program could not be started; do not keep trying.
    missing: bool,
}

impl Dropdown {
    /// The `enso-ctl` installed next to this bar if there is one (so a build
    /// tree or a private prefix uses its own), otherwise whichever is on
    /// `PATH`.
    pub fn new() -> Self {
        let sibling = env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("enso-ctl")))
            .filter(|path| path.is_file());
        Self::with_program(sibling.unwrap_or_else(|| PathBuf::from("enso-ctl")))
    }

    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            running: None,
            missing: false,
        }
    }

    /// Whether a panel is up. Also notices, and cleans up after, one that
    /// has closed.
    pub fn is_open(&mut self) -> bool {
        if let Some((child, _)) = &mut self.running
            && !matches!(child.try_wait(), Ok(None))
        {
            self.running = None;
        }
        self.running.is_some()
    }

    /// Open the panel on the bar's edge of the screen; if one is already up,
    /// tell it the pointer is back on the island.
    pub fn open(&mut self, position: Position) {
        if self.is_open() {
            return self.tell(true);
        }
        if self.missing {
            return;
        }
        let edge = match position {
            Position::Top => "top",
            Position::Bottom => "bottom",
        };
        let spawned = Command::new(&self.program)
            .arg(format!("--dropdown={edge}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                self.running = child.stdin.take().map(|stdin| (child, stdin));
            }
            Err(err) => {
                self.missing = true;
                if err.kind() == io::ErrorKind::NotFound {
                    eprintln!(
                        "enso-bar: enso-ctl is not installed, so the status island opens nothing"
                    );
                } else {
                    eprintln!("enso-bar: cannot open the control center: {err}");
                }
            }
        }
    }

    /// Tell an open panel whether the pointer is on the island.
    pub fn tell(&mut self, hovering: bool) {
        if let Some((_, stdin)) = &mut self.running {
            let line: &[u8] = if hovering { b"enter\n" } else { b"leave\n" };
            // A panel that has just closed is not listening; `is_open` will
            // notice next time.
            let _ = stdin.write_all(line).and_then(|()| stdin.flush());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;

    #[test]
    fn a_missing_program_is_tried_once_and_then_left_alone() {
        let mut dropdown = Dropdown::with_program("/nonexistent/enso-ctl");
        assert!(!dropdown.is_open());
        dropdown.open(Position::Top);
        assert!(dropdown.missing && !dropdown.is_open());
        // Telling nobody is harmless.
        dropdown.tell(false);
        dropdown.open(Position::Top);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn a_closed_panel_is_noticed_and_can_be_opened_again() {
        // `cat` stands in for the panel: it stays for as long as it is fed.
        let mut dropdown = Dropdown::with_program("cat");
        dropdown.open(Position::Top);
        assert!(!dropdown.missing);
        // It does not know `--dropdown`, so it leaves at once.
        let mut waited = 0;
        while dropdown.is_open() && waited < 200 {
            thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        assert!(!dropdown.is_open());
        // Writing to it now must not take the bar down with it.
        dropdown.tell(true);
        dropdown.open(Position::Bottom);
        assert!(!dropdown.missing);
    }
}
