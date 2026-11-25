//! Cards that hang from the bar: the control center under the status
//! island, and what's playing under the clock.
//!
//! The bar does not draw them: it runs the tool with `--dropdown` (`enso-ctl`,
//! `enso-play`) and tells it, a line at a time on its standard input, whether
//! the pointer is on the island (`enter`, `leave`). The card closes itself
//! once the pointer is on neither the island nor the card. Without the tool
//! installed, hovering simply does nothing — the bar stays a complete tool
//! on its own.
//!
//! There is only ever one control center (enso-ctl keeps to one copy), and
//! it and the notification history take turns: each closes the other as it
//! opens (the history sends it away itself). So resting on the bell, which
//! opens the history, closes the control center; resting on the status
//! island brings it back.

use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
};

use crate::config::Position;

#[derive(Debug)]
pub struct Dropdown {
    /// The tool's name, which it keeps to one copy by.
    tool: &'static str,
    /// What it hangs under, for the message when it can't be had.
    under: &'static str,
    program: PathBuf,
    running: Option<(Child, ChildStdin)>,
    /// The program could not be started; do not keep trying.
    missing: bool,
}

impl Dropdown {
    /// The control center: the `enso-ctl` installed next to this bar if
    /// there is one (so a build tree or a private prefix uses its own),
    /// otherwise whichever is on `PATH`.
    pub fn control_center() -> Self {
        Self::with_program(
            "enso-ctl",
            "the status island",
            enso_core::exe::companion("enso-ctl"),
        )
    }

    /// The month, under the clock while nothing plays: this very bar, run
    /// again as a card (`enso-bar --dropdown`).
    pub fn calendar() -> Self {
        Self::with_program(
            crate::calendar::NAME,
            "the clock",
            enso_core::exe::companion("enso-bar"),
        )
    }

    /// What's playing, from `enso-play`, found the same way.
    pub fn now_playing() -> Self {
        Self::with_program(
            "enso-play",
            "the clock",
            enso_core::exe::companion("enso-play"),
        )
    }

    pub fn with_program(
        tool: &'static str,
        under: &'static str,
        program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            tool,
            under,
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
        // One opened from the keyboard is already up: there is only ever
        // one, so resting here leaves it be.
        if self.missing || enso_core::ipc::is_listening(self.tool) {
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
                        "enso-bar: {} is not installed, so {} opens nothing",
                        self.tool, self.under
                    );
                } else {
                    eprintln!("enso-bar: cannot open {}: {err}", self.tool);
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

/// The notification history, which `enso-notify` shows when asked. Like
/// the control center it belongs to another tool; without `enso-notify`
/// the bell is simply a bell. It is told over the daemon's socket (see
/// [`enso_core::ipc`]) — the daemon is always running, and a word costs
/// nothing — that the pointer rests on the bell (`hover`: show the history,
/// and keep it) and that it has left (`leave`: it goes, unless the pointer
/// is on the card).
#[derive(Debug)]
pub struct History {
    /// The name the daemon listens under.
    tool: &'static str,
    /// The bell asked for it and hasn't said it has left yet.
    held: bool,
}

impl History {
    pub fn new() -> Self {
        Self::listening_as("enso-notify")
    }

    pub fn listening_as(tool: &'static str) -> Self {
        Self { tool, held: false }
    }

    /// The pointer rests on the bell: show the history, and keep it while it
    /// stays.
    pub fn hover(&mut self) {
        self.held = enso_core::ipc::tell(self.tool, "hover");
    }

    /// The pointer has left the bell, if it was holding the history open.
    pub fn leave(&mut self) {
        if std::mem::take(&mut self.held) {
            enso_core::ipc::tell(self.tool, "leave");
        }
    }

    /// Whether the bell is holding the history open.
    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        self.held
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;

    #[test]
    fn a_missing_program_is_tried_once_and_then_left_alone() {
        let mut dropdown =
            Dropdown::with_program("enso-ctl", "the island", "/nonexistent/enso-ctl");
        assert!(!dropdown.is_open());
        dropdown.open(Position::Top);
        assert!(dropdown.missing && !dropdown.is_open());
        // Telling nobody is harmless.
        dropdown.tell(false);
        dropdown.open(Position::Top);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn a_daemon_that_isnt_there_holds_nothing() {
        let mut history = History::listening_as("enso-test-nobody");
        history.hover();
        assert!(!history.is_held());
        // Nothing to let go of: harmless.
        history.leave();
        assert!(!history.is_held());
    }

    #[test]
    fn a_closed_panel_is_noticed_and_can_be_opened_again() {
        // `cat` stands in for the panel: it stays for as long as it is fed.
        let mut dropdown = Dropdown::with_program("enso-test", "the island", "cat");
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
