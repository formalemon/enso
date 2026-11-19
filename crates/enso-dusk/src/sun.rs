//! When the sun rises and sets here, from nothing but the timezone.
//!
//! The timezone database knows where each zone's city is
//! (`zone1970.tab`: `Asia/Kolkata` is at 22°32′N 88°22′E), which is near
//! enough: sunset a few hundred kilometres away is minutes off, and the
//! screen warms over half an hour anyway. A latitude and longitude in the
//! config win over it.
//!
//! The times are NOAA's sunrise equation, to the minute.

use std::{f64::consts::PI, fs};

use jiff::{Timestamp, civil::Date, tz::TimeZone};

/// Where the sun is reckoned for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Degrees north.
    pub latitude: f64,
    /// Degrees east.
    pub longitude: f64,
}

/// The day's light: sunrise and sunset, or the sun never setting or never
/// rising at all (near the poles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daylight {
    Between { rise: Timestamp, set: Timestamp },
    AllDay,
    AllNight,
}

/// The place of the system's timezone, from `zone1970.tab`.
pub fn place_of_timezone() -> Option<Place> {
    let name = TimeZone::system().iana_name()?.to_owned();
    let table = fs::read_to_string("/usr/share/zoneinfo/zone1970.tab")
        .or_else(|_| fs::read_to_string("/usr/share/zoneinfo/zone.tab"))
        .ok()?;
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split('\t');
            let (_, coordinates, zone) = (fields.next()?, fields.next()?, fields.next()?);
            (zone == name).then(|| parse_iso6709(coordinates)).flatten()
        })
}

/// `+4043-07400` (±DDMM±DDDMM) or `+404251-0740023` (with seconds).
fn parse_iso6709(text: &str) -> Option<Place> {
    let split = text[1..].find(['+', '-'])? + 1;
    let (lat, lon) = text.split_at(split);
    let angle = |part: &str, degree_digits: usize| -> Option<f64> {
        let sign = if part.starts_with('-') { -1.0 } else { 1.0 };
        let digits = &part[1..];
        let degrees: f64 = digits.get(..degree_digits)?.parse().ok()?;
        let minutes: f64 = digits.get(degree_digits..degree_digits + 2)?.parse().ok()?;
        let seconds: f64 = digits
            .get(degree_digits + 2..)
            .filter(|rest| !rest.is_empty())
            .map_or(Some(0.0), |rest| rest.parse().ok())?;
        Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
    };
    Some(Place {
        latitude: angle(lat, 2)?,
        longitude: angle(lon, 3)?,
    })
}

/// Sunrise and sunset on `date` (a civil day, here) at `place`.
pub fn daylight(date: Date, place: Place) -> Daylight {
    // Days since 2000-01-01 12:00 UTC, at local noon.
    let days = f64::from(
        date.since(jiff::civil::date(2000, 1, 1))
            .map_or(0, |span| span.get_days()),
    );
    let n = days - place.longitude / 360.0;
    let mean_anomaly = (357.5291 + 0.985_600_28 * n).rem_euclid(360.0);
    let m = mean_anomaly.to_radians();
    let center = 1.9148 * m.sin() + 0.0200 * (2.0 * m).sin() + 0.0003 * (3.0 * m).sin();
    let ecliptic = (mean_anomaly + center + 180.0 + 102.9372)
        .rem_euclid(360.0)
        .to_radians();
    let transit = 2_451_545.0 + n + 0.0053 * m.sin() - 0.0069 * (2.0 * ecliptic).sin();
    let declination = (ecliptic.sin() * 23.44_f64.to_radians().sin()).asin();
    let latitude = place.latitude.to_radians();
    // The sun's centre 0.833° below the horizon: refraction and its disc.
    let cos_hour = ((-0.833_f64).to_radians().sin() - latitude.sin() * declination.sin())
        / (latitude.cos() * declination.cos());
    if cos_hour < -1.0 {
        return Daylight::AllDay;
    }
    if cos_hour > 1.0 {
        return Daylight::AllNight;
    }
    let hour = cos_hour.acos() / (2.0 * PI);
    let julian = |day: f64| {
        let seconds = ((day - 2_440_587.5) * 86_400.0).round() as i64;
        Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH)
    };
    Daylight::Between {
        rise: julian(transit - hour),
        set: julian(transit + hour),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_coordinates_as_the_table_writes_them() {
        let new_york = parse_iso6709("+404251-0740023").unwrap();
        assert!((new_york.latitude - 40.714).abs() < 0.01);
        assert!((new_york.longitude + 74.006).abs() < 0.01);
        let kolkata = parse_iso6709("+2232+08822").unwrap();
        assert!((kolkata.latitude - 22.533).abs() < 0.01);
        assert!((kolkata.longitude - 88.367).abs() < 0.01);
        assert_eq!(parse_iso6709("nonsense"), None);
    }

    #[test]
    fn sunset_in_kolkata_and_midsummer_in_the_arctic() {
        let kolkata = Place {
            latitude: 22.5726,
            longitude: 88.3639,
        };
        let Daylight::Between { rise, set } = daylight(jiff::civil::date(2026, 11, 19), kolkata)
        else {
            panic!("the sun rises in Kolkata");
        };
        // About 05:56 and 16:52 IST (00:26 and 11:22 UTC), to within
        // minutes: the screen warms over half an hour.
        let utc = |t: Timestamp| t.to_zoned(TimeZone::UTC).time();
        let near = |t: Timestamp, h: i8, m: i8| {
            let at = utc(t);
            let minutes = i32::from(at.hour()) * 60 + i32::from(at.minute());
            (minutes - (i32::from(h) * 60 + i32::from(m))).abs() <= 8
        };
        assert!(near(rise, 0, 26), "{rise}");
        assert!(near(set, 11, 22), "{set}");

        let tromso = Place {
            latitude: 69.65,
            longitude: 18.96,
        };
        assert_eq!(
            daylight(jiff::civil::date(2026, 6, 21), tromso),
            Daylight::AllDay
        );
        assert_eq!(
            daylight(jiff::civil::date(2026, 12, 21), tromso),
            Daylight::AllNight
        );
    }
}
