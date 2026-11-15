//! Laptop power and thermals, read straight from sysfs.

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
                    status,
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
        Some(PlatformProfile { current, choices })
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

    /// Screen backlight level, `0..=100`.
    pub fn backlight(&self) -> Option<u8> {
        entries(&self.root.join("class/backlight"))
            .into_iter()
            .find_map(|dir| {
                let now: f32 = read(&dir.join("brightness"))?.parse().ok()?;
                let max: f32 = read(&dir.join("max_brightness"))?.parse().ok()?;
                (max > 0.0).then(|| (now / max * 100.0).round().clamp(0.0, 100.0) as u8)
            })
    }
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
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_profile_temperature_and_backlight() {
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
            ],
        );
        let profile = sys.platform_profile().unwrap();
        assert_eq!(profile.current, "balanced");
        assert_eq!(profile.choices, ["low-power", "balanced", "performance"]);
        assert_eq!(sys.cpu_temperature(), Some(54.5));
        assert_eq!(sys.backlight(), Some(50));

        sys.set_platform_profile("performance").unwrap();
        assert_eq!(sys.platform_profile().unwrap().current, "performance");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_hardware_is_none_not_an_error() {
        let sys = Sysfs::at("/nonexistent/enso-sys");
        assert_eq!(sys.battery(), None);
        assert_eq!(sys.platform_profile(), None);
        assert_eq!(sys.cpu_temperature(), None);
        assert_eq!(sys.backlight(), None);
    }
}
