//! How warm the screens should be, and when to look again.
//!
//! Warmth is a share from `0` (as the screen is) to `1` (as warm as
//! `warmth` says), and it changes slowly: over `fade` minutes centred on
//! sunset, and again on sunrise. Turned on or off by hand while following
//! the sun, it stays that way until the next sunset or sunrise, and then
//! the sun has its say again.

use jiff::{SignedDuration, Timestamp, tz::TimeZone};

use crate::sun::{self, Daylight, Place};

/// What's been chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Warm from sunset to sunrise.
    #[default]
    Sun,
    /// Warm now (until `until`, if set: then back to the sun).
    On,
    /// As the screen is (likewise).
    Off,
}

/// The choice, and when a choice made by hand gives way to the sun.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub mode: Mode,
    /// Seconds since the epoch; `0` for never.
    pub until: i64,
}

impl Choice {
    /// The mode in force at `now`.
    pub fn mode_at(self, now: i64) -> Mode {
        if self.until > 0 && now >= self.until {
            Mode::Sun
        } else {
            self.mode
        }
    }
}

/// The day and night around `now`: the sunset before it and the sunrise
/// after, or the other way about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    pub warm: f64,
    /// When the light next changes: the next sunrise or sunset.
    pub turn: Option<Timestamp>,
    /// Night: between sunset and sunrise.
    pub night: bool,
}

/// How warm the sun says the screen should be at `now`, with a fade of
/// `fade` minutes, at `place`.
pub fn sky(now: Timestamp, place: Place, fade: i64) -> Sky {
    let zone = TimeZone::system();
    let today = now.to_zoned(zone.clone()).date();
    let half = SignedDuration::from_secs(fade.max(1) * 30);
    // Yesterday's, today's, and tomorrow's turns, in order.
    let mut turns: Vec<(Timestamp, bool)> = Vec::new();
    let mut polar = None;
    for offset in [-1, 0, 1] {
        let Ok(day) = today.checked_add(jiff::Span::new().days(offset)) else {
            continue;
        };
        match sun::daylight(day, place) {
            // Rising: night ends. Setting: night begins.
            Daylight::Between { rise, set } => {
                turns.push((rise, false));
                turns.push((set, true));
            }
            Daylight::AllDay if offset == 0 => polar = Some(false),
            Daylight::AllNight if offset == 0 => polar = Some(true),
            _ => {}
        }
    }
    if let Some(night) = polar {
        return Sky {
            warm: if night { 1.0 } else { 0.0 },
            turn: None,
            night,
        };
    }
    turns.sort();
    // The last turn before now says whether it's night; a turn within half
    // a fade either side blends.
    let night = turns
        .iter()
        .rev()
        .find(|(at, _)| *at <= now)
        .is_some_and(|(_, night)| *night);
    let mut warm: f64 = if night { 1.0 } else { 0.0 };
    for (at, to_night) in &turns {
        let from = *at - half;
        let to = *at + half;
        if now >= from && now <= to {
            let span = to.duration_since(from).as_secs_f64().max(1.0);
            let progress = now.duration_since(from).as_secs_f64() / span;
            warm = if *to_night { progress } else { 1.0 - progress };
        }
    }
    let turn = turns.iter().find(|(at, _)| *at > now).map(|(at, _)| *at);
    Sky {
        warm: warm.clamp(0.0, 1.0),
        turn,
        night,
    }
}

/// The colour temperature for a share of warmth.
pub fn kelvin(warm: f64, warmth: u32) -> u32 {
    let day = 6500.0;
    (day + (f64::from(warmth.clamp(1000, 6500)) - day) * warm.clamp(0.0, 1.0)).round() as u32
}

/// How much to let red, green, and blue through at `kelvin`, `1` for the
/// screen as it is (6500 K): Tanner Helland's fit of the black body,
/// scaled so that 6500 K changes nothing.
pub fn channels(kelvin: u32) -> [f64; 3] {
    fn raw(kelvin: f64) -> [f64; 3] {
        let t = kelvin / 100.0;
        let red = if t <= 66.0 {
            255.0
        } else {
            329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
        };
        let green = if t <= 66.0 {
            99.470_802_586_1 * t.ln() - 161.119_568_166_1
        } else {
            288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
        };
        let blue = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
        };
        [red, green, blue].map(|c| (c / 255.0).clamp(0.0, 1.0))
    }
    let white = raw(6500.0);
    let at = raw(f64::from(kelvin));
    [0, 1, 2].map(|i| (at[i] / white[i]).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmth_follows_the_sun_with_a_fade() {
        let kolkata = Place {
            latitude: 22.57,
            longitude: 88.36,
        };
        // Midday UTC is evening in India, near sunset (about 11:22 UTC).
        let at = |h: i8, m: i8| {
            jiff::civil::date(2026, 11, 19)
                .at(h, m, 0, 0)
                .to_zoned(TimeZone::UTC)
                .unwrap()
                .timestamp()
        };
        assert_eq!(sky(at(6, 0), kolkata, 30).warm, 0.0);
        assert!(!sky(at(6, 0), kolkata, 30).night);
        let fading = sky(at(11, 22), kolkata, 30).warm;
        assert!((0.3..0.7).contains(&fading), "{fading}");
        assert_eq!(sky(at(14, 0), kolkata, 30).warm, 1.0);
        assert!(sky(at(14, 0), kolkata, 30).night);
        assert!(sky(at(14, 0), kolkata, 30).turn.is_some());
    }

    #[test]
    fn a_choice_by_hand_gives_way_to_the_sun() {
        let choice = Choice {
            mode: Mode::Off,
            until: 1_000,
        };
        assert_eq!(choice.mode_at(999), Mode::Off);
        assert_eq!(choice.mode_at(1_000), Mode::Sun);
        assert_eq!(Choice::default().mode_at(5), Mode::Sun);
    }

    #[test]
    fn day_light_changes_nothing_and_night_is_warm() {
        assert_eq!(kelvin(0.0, 3500), 6500);
        assert_eq!(kelvin(1.0, 3500), 3500);
        let day = channels(6500);
        assert!(day.iter().all(|c| (c - 1.0).abs() < 1e-9));
        let [r, g, b] = channels(3500);
        assert!(r == 1.0 && g < 0.9 && b < g);
    }
}
