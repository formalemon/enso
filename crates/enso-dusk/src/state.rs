//! What's been chosen, and how it stands, in one small file in the state
//! directory (`dusk`): read by the control center to light its moon, and
//! by `enso-dusk` itself when it starts.
//!
//! ```text
//! mode sun          sun, on, or off
//! until 0           when a choice made by hand gives way to the sun
//! warm 1            whether the screens are warm now
//! ```

use std::{fs, io};

use enso_core::{Paths, disk};

use crate::plan::{Choice, Mode};

const FILE: &str = "dusk";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub choice: Choice,
    pub warm: bool,
}

impl State {
    pub fn read(paths: &Paths) -> Self {
        fs::read_to_string(paths.state_dir.join(FILE))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn write(self, paths: &Paths) -> io::Result<()> {
        disk::write_atomic(&paths.state_dir.join(FILE), self.encode().as_bytes(), 0o644)
    }

    fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once(' ') else {
                continue;
            };
            match (key, value.trim()) {
                ("mode", "on") => state.choice.mode = Mode::On,
                ("mode", "off") => state.choice.mode = Mode::Off,
                ("mode", _) => state.choice.mode = Mode::Sun,
                ("until", until) => state.choice.until = until.parse().unwrap_or(0),
                ("warm", warm) => state.warm = warm == "1",
                _ => {}
            }
        }
        state
    }

    fn encode(self) -> String {
        let mode = match self.choice.mode {
            Mode::Sun => "sun",
            Mode::On => "on",
            Mode::Off => "off",
        };
        format!(
            "mode {mode}\nuntil {}\nwarm {}\n",
            self.choice.until,
            u8::from(self.warm)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let state = State {
            choice: Choice {
                mode: Mode::Off,
                until: 42,
            },
            warm: true,
        };
        assert_eq!(State::parse(&state.encode()), state);
        assert_eq!(State::parse("nonsense"), State::default());
    }
}
