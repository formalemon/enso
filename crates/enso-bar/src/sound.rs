//! The output's volume and the microphone, for the status island.
//!
//! A background thread follows the sound server and reports what it hears
//! whenever that changes: the output's level (what the island shows as it
//! turns, and what scrolling steps from), and whether anyone is listening
//! to the microphone. Without a sound server the island simply has none of
//! it.

use std::{thread, time::Duration};

use enso_sys::audio::Mixer;

/// The output's volume in percent, and whether it is muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub volume: u8,
    pub muted: bool,
}

/// The microphone, as far as the island cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mic {
    /// Some application is listening to it.
    pub listened: bool,
    pub muted: bool,
    /// Its level, in percent.
    pub volume: u8,
}

/// Everything the sound server says that the bar shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// The output's level (`None`: no output).
    pub output: Option<Level>,
    pub mic: Mic,
}

/// Follow the sound server on a background thread, calling `on_change`
/// with what it hears each time that changes. Keeps trying if the sound
/// server is not up yet, or goes away; stops when `on_change` returns
/// `false`.
pub fn watch(mut on_change: impl FnMut(Heard) -> bool + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-sound".to_owned())
        .spawn(move || {
            let mut last = None;
            let mut report = |heard: Heard| {
                if last == Some(heard) {
                    return true;
                }
                last = Some(heard);
                on_change(heard)
            };
            loop {
                if let Ok(mixer) = Mixer::connect() {
                    let level = || {
                        let Ok(sound) = mixer.state() else {
                            return Heard::default();
                        };
                        Heard {
                            output: sound.output.map(|output| Level {
                                volume: output.volume,
                                muted: output.muted,
                            }),
                            mic: sound.input.map_or(Mic::default(), |input| Mic {
                                listened: !sound.recording.is_empty(),
                                muted: input.muted,
                                volume: input.volume,
                            }),
                        }
                    };
                    let mut listening = report(level());
                    if listening {
                        // Returns when the listener is gone or the server is.
                        let _ = mixer.watch(|| {
                            listening = report(level());
                            listening
                        });
                    }
                    if !listening {
                        return;
                    }
                }
                if !report(Heard::default()) {
                    return;
                }
                thread::sleep(Duration::from_secs(5));
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-bar: cannot follow the volume: {err}");
    }
}

/// Set the output's volume, on a thread of its own: the bar never waits on
/// the sound server. The new level comes back through [`watch`].
pub fn set(volume: u8) {
    let spawned = thread::Builder::new()
        .name("enso-volume".to_owned())
        .spawn(move || {
            if let Err(err) = Mixer::connect().and_then(|mixer| mixer.set_volume(volume)) {
                eprintln!("enso-bar: cannot set the volume: {err}");
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-bar: cannot set the volume: {err}");
    }
}

/// Scrolling that adds up to one step of volume: a wheel notch.
const NOTCH: f32 = 15.0;
/// Percent per step.
const STEP: i32 = 5;

/// Turns scroll distance into volume steps, so a wheel moves a step per
/// notch and a touchpad's stream of small movements does not race.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    carried: f32,
}

impl Scroll {
    /// Take a scroll of `dy` (positive is downward, which is quieter) and
    /// return the level to set, if this crossed into a new step.
    pub fn level(&mut self, dy: f32, current: u8) -> Option<u8> {
        self.carried += dy;
        let steps = (self.carried / NOTCH).trunc();
        if steps == 0.0 {
            return None;
        }
        self.carried -= steps * NOTCH;
        // Onto the grid of steps, so 43 % goes to 45 % or 40 %, not 48 %.
        let current = i32::from(current);
        let target = if steps < 0.0 {
            (current / STEP - steps as i32) * STEP
        } else {
            ((current + STEP - 1) / STEP - steps as i32) * STEP
        };
        let target = target.clamp(0, 100) as u8;
        (i32::from(target) != current).then_some(target)
    }

    /// Forget a half-finished step, when the pointer moves away.
    pub fn reset(&mut self) {
        self.carried = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notch_is_a_step_and_small_scrolls_add_up() {
        let mut scroll = Scroll::default();
        // Up a notch, down a notch.
        assert_eq!(scroll.level(-15.0, 40), Some(45));
        assert_eq!(scroll.level(15.0, 45), Some(40));
        // A touchpad: nothing until a notch's worth has gone by.
        assert_eq!(scroll.level(-6.0, 40), None);
        assert_eq!(scroll.level(-6.0, 40), None);
        assert_eq!(scroll.level(-6.0, 40), Some(45));
        // The remainder carries over.
        assert_eq!(scroll.level(-12.0, 45), Some(50));
        // A fast flick is several steps at once.
        scroll.reset();
        assert_eq!(scroll.level(-45.0, 50), Some(65));
    }

    #[test]
    fn levels_land_on_the_grid_and_stay_in_range() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.level(-15.0, 43), Some(45));
        assert_eq!(scroll.level(15.0, 43), Some(40));
        assert_eq!(scroll.level(-15.0, 98), Some(100));
        assert_eq!(scroll.level(-15.0, 100), None);
        assert_eq!(scroll.level(15.0, 3), Some(0));
        assert_eq!(scroll.level(15.0, 0), None);
        // Reversing direction cancels what was carried.
        scroll.reset();
        assert_eq!(scroll.level(-10.0, 50), None);
        assert_eq!(scroll.level(10.0, 50), None);
        assert_eq!(scroll.level(-10.0, 50), None);
    }
}
