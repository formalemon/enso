//! Laptop power, thermals, and backlights, read straight from sysfs.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// A view of sysfs. Use [`Sysfs::system`] for the real one; tests point it at
/// a scratch directory.
#[derive(Clone, Debug)]
pub struct Sysfs {
    root: PathBuf,
}

/// What a battery is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    /// Plugged in but holding its charge (charge thresholds).
    NotCharging,
    Unknown,
}

/// State of the system battery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Battery {
    /// Kernel name, e.g. `BAT1`.
    pub name: String,
    /// Charge level, `0..=100`.
    pub percent: u8,
    pub status: BatteryStatus,
    /// Power flowing in or out right now, in milliwatts.
    pub power_mw: Option<u32>,
    /// Minutes until empty while discharging, or until full while charging,
    /// at the present rate.
    pub minutes_left: Option<u32>,
}

impl Battery {
    /// On external power (charging, full, or held at a threshold).
    pub fn is_plugged_in(&self) -> bool {
        !matches!(
            self.status,
            BatteryStatus::Discharging | BatteryStatus::Unknown
        )
    }
}

/// The ACPI platform profile (performance / balanced / low-power).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformProfile {
    pub current: String,
    pub choices: Vec<String>,
    /// Whether this user may switch it. The kernel leaves that to root
    /// unless the system grants more.
    pub writable: bool,
}

impl PlatformProfile {
    /// The choice after the current one, wrapping around.
    pub fn next(&self) -> Option<&str> {
        let at = self.choices.iter().position(|c| *c == self.current)?;
        self.choices
            .get((at + 1) % self.choices.len())
            .map(String::as_str)
    }
}

/// Which light a [`Backlight`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacklightKind {
    Screen,
    Keyboard,
}

impl BacklightKind {
    /// The kernel subsystem the device lives in, which is also how logind
    /// names it.
    pub fn subsystem(self) -> &'static str {
        match self {
            Self::Screen => "backlight",
            Self::Keyboard => "leds",
        }
    }
}

/// A dimmable light: the screen's backlight or the keyboard's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlight {
    pub kind: BacklightKind,
    /// Kernel name, e.g. `intel_backlight`.
    pub name: String,
    /// Current level, `0..=max`.
    pub brightness: u32,
    pub max: u32,
}

impl Backlight {
    /// Current level as `0..=100`.
    pub fn percent(&self) -> u8 {
        (f64::from(self.brightness) / f64::from(self.max) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }

    /// The raw level for `percent`. A screen never goes fully dark this way:
    /// its lowest step is the dimmest lit one.
    pub fn level(&self, percent: u8) -> u32 {
        let level = (f64::from(percent.min(100)) / 100.0 * f64::from(self.max)).round() as u32;
        match self.kind {
            BacklightKind::Screen => level.max(1),
            BacklightKind::Keyboard => level,
        }
    }
}

/// A fan and how fast it is turning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fan {
    /// What the firmware calls it (`cpu_fan`), or `fan1` if it does not say.
    pub label: String,
    pub rpm: u32,
}

impl Default for Sysfs {
    fn default() -> Self {
        Self::system()
    }
}

impl Sysfs {
    /// The real `/sys`.
    pub fn system() -> Self {
        Self::at("/sys")
    }

    /// A sysfs tree rooted elsewhere.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The system battery, if there is one. Batteries of peripherals
    /// (mice, headsets) are skipped.
    pub fn battery(&self) -> Option<Battery> {
        entries(&self.root.join("class/power_supply"))
            .into_iter()
            .find_map(|dir| {
                if read(&dir.join("type"))? != "Battery" {
                    return None;
                }
                if read(&dir.join("scope")).is_some_and(|scope| scope == "Device") {
                    return None;
                }
                let percent: u8 = read(&dir.join("capacity"))?.parse().ok()?;
                let (power_mw, minutes_left) = battery_rate(&dir);
                let status = match read(&dir.join("status")).as_deref() {
                    Some("Charging") => BatteryStatus::Charging,
                    Some("Discharging") => BatteryStatus::Discharging,
                    Some("Full") => BatteryStatus::Full,
                    Some("Not charging") => BatteryStatus::NotCharging,
                    _ => BatteryStatus::Unknown,
                };
                Some(Battery {
                    name: dir.file_name()?.to_string_lossy().into_owned(),
                    percent: percent.min(100),
                    // A rate only means something while charge is moving.
                    minutes_left: minutes_left.filter(|_| {
                        matches!(status, BatteryStatus::Charging | BatteryStatus::Discharging)
                    }),
                    status,
                    power_mw,
                })
            })
    }

    /// The active platform profile and the ones the firmware offers.
    pub fn platform_profile(&self) -> Option<PlatformProfile> {
        let acpi = self.root.join("firmware/acpi");
        let current = read(&acpi.join("platform_profile"))?;
        let choices = read(&acpi.join("platform_profile_choices"))
            .map(|choices| choices.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        // Opening for writing changes nothing; it only asks permission.
        let writable = fs::OpenOptions::new()
            .write(true)
            .open(acpi.join("platform_profile"))
            .is_ok();
        Some(PlatformProfile {
            current,
            choices,
            writable,
        })
    }

    /// Switch platform profile. Needs write access to sysfs, which normally
    /// means root or a udev rule granting it.
    pub fn set_platform_profile(&self, profile: &str) -> io::Result<()> {
        fs::write(self.root.join("firmware/acpi/platform_profile"), profile)
    }

    /// CPU temperature in °C: the package sensor when present, otherwise the
    /// hottest thermal zone.
    pub fn cpu_temperature(&self) -> Option<f32> {
        let zones: Vec<(String, f32)> = entries(&self.root.join("class/thermal"))
            .into_iter()
            .filter_map(|dir| {
                let kind = read(&dir.join("type"))?;
                let millidegrees: f32 = read(&dir.join("temp"))?.parse().ok()?;
                Some((kind, millidegrees / 1000.0))
            })
            .collect();
        zones
            .iter()
            .find(|(kind, _)| kind == "x86_pkg_temp")
            .or_else(|| zones.iter().max_by(|a, b| a.1.total_cmp(&b.1)))
            .map(|(_, temp)| *temp)
    }

    /// Fans that report a speed, in hwmon order.
    pub fn fans(&self) -> Vec<Fan> {
        entries(&self.root.join("class/hwmon"))
            .into_iter()
            .flat_map(|dir| entries(&dir))
            .filter_map(|path| {
                let file = path.file_name()?.to_str()?;
                let fan = file
                    .strip_suffix("_input")
                    .filter(|f| f.starts_with("fan"))?;
                let rpm = read(&path)?.parse().ok()?;
                let label = read(&path.with_file_name(format!("{fan}_label")))
                    .unwrap_or_else(|| fan.to_owned());
                Some(Fan { label, rpm })
            })
            .collect()
    }

    /// The screen's backlight. With several to choose from, the one the
    /// firmware drives wins over a raw panel control, as the kernel advises.
    pub fn screen_backlight(&self) -> Option<Backlight> {
        let rank = |dir: &Path| match read(&dir.join("type")).as_deref() {
            Some("firmware") => 0,
            Some("platform") => 1,
            _ => 2,
        };
        entries(&self.root.join("class/backlight"))
            .into_iter()
            .filter_map(|dir| Some((rank(&dir), backlight(BacklightKind::Screen, &dir)?)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, backlight)| backlight)
    }

    /// The keyboard's backlight, on laptops that have one.
    pub fn keyboard_backlight(&self) -> Option<Backlight> {
        entries(&self.root.join("class/leds"))
            .into_iter()
            .filter(|dir| {
                dir.file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("kbd_backlight"))
            })
            .find_map(|dir| backlight(BacklightKind::Keyboard, &dir))
    }

    /// Set a backlight's raw level directly. Needs write access to sysfs,
    /// which normally means root or a udev rule; `logind::set_brightness`
    /// (feature `logind`) is the way that needs neither.
    pub fn set_backlight(&self, backlight: &Backlight, level: u32) -> io::Result<()> {
        let path = self
            .root
            .join("class")
            .join(backlight.kind.subsystem())
            .join(&backlight.name)
            .join("brightness");
        fs::write(path, level.min(backlight.max).to_string())
    }
}

/// Read one backlight directory.
fn backlight(kind: BacklightKind, dir: &Path) -> Option<Backlight> {
    let brightness = read(&dir.join("brightness"))?.parse().ok()?;
    let max: u32 = read(&dir.join("max_brightness"))?.parse().ok()?;
    (max > 0).then(|| Backlight {
        kind,
        name: dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        brightness,
        max,
    })
}

/// A battery's present power in milliwatts and, from it, the minutes until
/// it is empty or full. Batteries report either energy (µWh, µW) or charge
/// (µAh, µA); both work the same way.
fn battery_rate(dir: &Path) -> (Option<u32>, Option<u32>) {
    let number = |file: &str| read(&dir.join(file))?.parse::<f64>().ok();
    let (now, full, rate) = match (number("energy_now"), number("power_now")) {
        (Some(now), Some(rate)) => (now, number("energy_full"), rate),
        _ => match (number("charge_now"), number("current_now")) {
            (Some(now), Some(rate)) => (now, number("charge_full"), rate),
            _ => return (None, None),
        },
    };
    let rate = rate.abs();
    let power_mw = number("power_now").map(f64::abs).or_else(|| {
        // µA × µV is 10⁻¹² W.
        Some(rate * number("voltage_now")? / 1e6)
    });
    let power_mw = power_mw.map(|microwatts| (microwatts / 1000.0).round() as u32);
    if rate <= 0.0 {
        return (power_mw, None);
    }
    let charging = read(&dir.join("status")).as_deref() == Some("Charging");
    let remaining = match (charging, full) {
        (true, Some(full)) => (full - now).max(0.0),
        (true, None) => return (power_mw, None),
        (false, _) => now,
    };
    (power_mw, Some((remaining / rate * 60.0).round() as u32))
}

/// Directory entries sorted by name; empty if the directory is unreadable.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

/// A sysfs attribute, trimmed; `None` if missing or unreadable.
fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[(&str, &str)]) -> (PathBuf, Sysfs) {
        let root = std::env::temp_dir().join(format!("enso-sys-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        (root.clone(), Sysfs::at(root))
    }

    #[test]
    fn reads_the_system_battery_and_skips_peripherals() {
        let (root, sys) = tree(
            "battery",
            &[
                ("class/power_supply/ACAD/type", "Mains\n"),
                ("class/power_supply/AAA_mouse/type", "Battery\n"),
                ("class/power_supply/AAA_mouse/scope", "Device\n"),
                ("class/power_supply/AAA_mouse/capacity", "12\n"),
                ("class/power_supply/BAT1/type", "Battery\n"),
                ("class/power_supply/BAT1/capacity", "82\n"),
                ("class/power_supply/BAT1/status", "Not charging\n"),
            ],
        );
        let battery = sys.battery().unwrap();
        assert_eq!(battery.name, "BAT1");
        assert_eq!(battery.percent, 82);
        assert_eq!(battery.status, BatteryStatus::NotCharging);
        assert!(battery.is_plugged_in());
        assert_eq!((battery.power_mw, battery.minutes_left), (None, None));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn estimates_power_and_time_from_either_kind_of_gauge() {
        // A charge gauge (µAh, µA, µV), discharging at 1.5 A and 12 V.
        let (root, sys) = tree(
            "rate-charge",
            &[
                ("class/power_supply/BAT1/type", "Battery\n"),
                ("class/power_supply/BAT1/capacity", "50\n"),
                ("class/power_supply/BAT1/status", "Discharging\n"),
                ("class/power_supply/BAT1/charge_now", "3000000\n"),
                ("class/power_supply/BAT1/charge_full", "6000000\n"),
                ("class/power_supply/BAT1/current_now", "1500000\n"),
                ("class/power_supply/BAT1/voltage_now", "12000000\n"),
            ],
        );
        let battery = sys.battery().unwrap();
        assert_eq!(battery.power_mw, Some(18_000));
        assert_eq!(battery.minutes_left, Some(120));
        fs::remove_dir_all(root).unwrap();

        // An energy gauge (µWh, µW), charging at 20 W with 10 Wh to go.
        let (root, sys) = tree(
            "rate-energy",
            &[
                ("class/power_supply/BAT0/type", "Battery\n"),
                ("class/power_supply/BAT0/capacity", "80\n"),
                ("class/power_supply/BAT0/status", "Charging\n"),
                ("class/power_supply/BAT0/energy_now", "40000000\n"),
                ("class/power_supply/BAT0/energy_full", "50000000\n"),
                ("class/power_supply/BAT0/power_now", "20000000\n"),
            ],
        );
        let battery = sys.battery().unwrap();
        assert_eq!(battery.power_mw, Some(20_000));
        assert_eq!(battery.minutes_left, Some(30));
        fs::remove_dir_all(root).unwrap();

        // Full and idle: no flow, so no estimate.
        let (root, sys) = tree(
            "rate-idle",
            &[
                ("class/power_supply/BAT1/type", "Battery\n"),
                ("class/power_supply/BAT1/capacity", "100\n"),
                ("class/power_supply/BAT1/status", "Full\n"),
                ("class/power_supply/BAT1/charge_now", "6000000\n"),
                ("class/power_supply/BAT1/current_now", "0\n"),
                ("class/power_supply/BAT1/voltage_now", "12000000\n"),
            ],
        );
        let battery = sys.battery().unwrap();
        assert_eq!((battery.power_mw, battery.minutes_left), (Some(0), None));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_profile_temperature_fans_and_backlights() {
        let (root, sys) = tree(
            "misc",
            &[
                ("firmware/acpi/platform_profile", "balanced\n"),
                (
                    "firmware/acpi/platform_profile_choices",
                    "low-power balanced performance\n",
                ),
                ("class/thermal/thermal_zone0/type", "acpitz\n"),
                ("class/thermal/thermal_zone0/temp", "71000\n"),
                ("class/thermal/thermal_zone1/type", "x86_pkg_temp\n"),
                ("class/thermal/thermal_zone1/temp", "54500\n"),
                ("class/thermal/cooling_device0/type", "Processor\n"),
                ("class/backlight/intel_backlight/brightness", "480\n"),
                ("class/backlight/intel_backlight/max_brightness", "960\n"),
                ("class/backlight/intel_backlight/type", "raw\n"),
                ("class/backlight/acpi_video0/brightness", "3\n"),
                ("class/backlight/acpi_video0/max_brightness", "10\n"),
                ("class/backlight/acpi_video0/type", "firmware\n"),
                ("class/leds/input3::capslock/brightness", "0\n"),
                ("class/leds/input3::capslock/max_brightness", "1\n"),
                ("class/leds/asus::kbd_backlight/brightness", "1\n"),
                ("class/leds/asus::kbd_backlight/max_brightness", "3\n"),
                ("class/hwmon/hwmon6/name", "asus\n"),
                ("class/hwmon/hwmon6/fan1_input", "3600\n"),
                ("class/hwmon/hwmon6/fan1_label", "cpu_fan\n"),
                ("class/hwmon/hwmon6/fan2_input", "0\n"),
                ("class/hwmon/hwmon8/temp1_input", "54000\n"),
            ],
        );
        let profile = sys.platform_profile().unwrap();
        assert_eq!(profile.current, "balanced");
        assert_eq!(profile.choices, ["low-power", "balanced", "performance"]);
        assert!(profile.writable);
        assert_eq!(profile.next(), Some("performance"));
        assert_eq!(sys.cpu_temperature(), Some(54.5));

        let fans = sys.fans();
        let fans: Vec<(&str, u32)> = fans.iter().map(|f| (f.label.as_str(), f.rpm)).collect();
        assert_eq!(fans, [("cpu_fan", 3600), ("fan2", 0)]);

        // The firmware's control is preferred over the raw panel one.
        let screen = sys.screen_backlight().unwrap();
        assert_eq!(
            (screen.name.as_str(), screen.percent()),
            ("acpi_video0", 30)
        );
        assert_eq!(screen.level(50), 5);
        // A screen never dims to nothing; a keyboard light may.
        assert_eq!(screen.level(0), 1);
        let keys = sys.keyboard_backlight().unwrap();
        assert_eq!(
            (keys.name.as_str(), keys.percent()),
            ("asus::kbd_backlight", 33)
        );
        assert_eq!(keys.level(0), 0);

        sys.set_backlight(&screen, 99).unwrap();
        assert_eq!(sys.screen_backlight().unwrap().brightness, 10);
        sys.set_backlight(&keys, 3).unwrap();
        assert_eq!(sys.keyboard_backlight().unwrap().percent(), 100);

        sys.set_platform_profile("performance").unwrap();
        let profile = sys.platform_profile().unwrap();
        assert_eq!(profile.current, "performance");
        // The last choice wraps to the first.
        assert_eq!(profile.next(), Some("low-power"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_hardware_is_none_not_an_error() {
        let sys = Sysfs::at("/nonexistent/enso-sys");
        assert_eq!(sys.battery(), None);
        assert_eq!(sys.platform_profile(), None);
        assert_eq!(sys.cpu_temperature(), None);
        assert_eq!(sys.screen_backlight(), None);
        assert_eq!(sys.keyboard_backlight(), None);
        assert!(sys.fans().is_empty());
    }
}
