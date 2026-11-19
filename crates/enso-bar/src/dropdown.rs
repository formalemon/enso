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
//! opens. So a click on the island, which opens the history, also sends the
//! control center away, and resting there opens nothing while either is up.

use std::{
    io::{self, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    thread,
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

    /// Ask an open panel to close now: something else wants its place.
    pub fn close(&mut self) {
        if let Some((_, stdin)) = &mut self.running {
            let _ = stdin.write_all(b"close\n").and_then(|()| stdin.flush());
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
/// the bell is simply a bell.
#[derive(Debug)]
pub struct History {
    program: PathBuf,
    /// The program could not be started; do not keep trying.
    missing: bool,
}

impl History {
    pub fn new() -> Self {
        Self::with_program(enso_core::exe::companion("enso-notify"))
    }

    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            missing: false,
        }
    }

    /// Open the history, or close it if it is open.
    pub fn toggle(&mut self) {
        if self.missing {
            return;
        }
        let spawned = Command::new(&self.program)
            .args(["history", "toggle"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // It says so itself when no daemon is running.
            .spawn();
        match spawned {
            // It passes the word on and exits; wait for it off to the side
            // so it is not left behind as a zombie.
            Ok(mut child) => {
                let waiting = thread::Builder::new()
                    .name("enso-history".to_owned())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                if let Err(err) = waiting {
                    eprintln!("enso-bar: cannot wait for enso-notify: {err}");
                }
            }
            Err(err) => {
                self.missing = true;
                if err.kind() == io::ErrorKind::NotFound {
                    eprintln!("enso-bar: enso-notify is not installed, so the bell opens nothing");
                } else {
                    eprintln!("enso-bar: cannot open the notification history: {err}");
                }
            }
        }
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
        dropdown.close();
        dropdown.open(Position::Top);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn a_missing_notification_daemon_is_asked_once() {
        let mut history = History::with_program("/nonexistent/enso-notify");
        history.toggle();
        assert!(history.missing);
        history.toggle();
        // One that exists is simply run.
        let mut history = History::with_program("true");
        history.toggle();
        assert!(!history.missing);
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
