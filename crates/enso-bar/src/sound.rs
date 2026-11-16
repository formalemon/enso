//! The output's volume, for the status island's speaker.
//!
//! A background thread follows the sound server and reports the level
//! whenever it changes. Without a sound server the island simply has no
//! speaker.

use std::{thread, time::Duration};

use enso_sys::audio::Mixer;

/// The output's volume in percent, and whether it is muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub volume: u8,
    pub muted: bool,
}

/// Follow the output's volume on a background thread, calling `on_change`
/// with each new level (`None` while there is no output). Keeps trying if
/// the sound server is not up yet, or goes away; stops when `on_change`
/// returns `false`.
pub fn watch(mut on_change: impl FnMut(Option<Level>) -> bool + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-sound".to_owned())
        .spawn(move || {
            let mut last = None;
            let mut report = |level: Option<Level>| {
                if last == Some(level) {
                    return true;
                }
                last = Some(level);
                on_change(level)
            };
            loop {
                if let Ok(mixer) = Mixer::connect() {
                    let level = || {
                        let output = mixer.state().ok()?.output?;
                        Some(Level {
                            volume: output.volume,
                            muted: output.muted,
                        })
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
                if !report(None) {
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
