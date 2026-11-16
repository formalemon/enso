//! Holding the machine in its low-power profile while it is locked.
//!
//! The ACPI platform profile is the firmware's own dial for how hard the
//! machine may work: in `low-power` the processor is capped and the fans
//! stay down. Nothing is stopped by it — a download or a build carries on,
//! slower — which is the trade a locked machine wants: close to idle, with
//! everything still there on unlock.
//!
//! The profile is the system's, not ours, so what it was before is written
//! down first. If the lock screen dies before putting it back, the next one
//! finds the note and restores from it rather than mistaking `low-power`
//! for the user's choice.

use std::{
    fs,
    path::{Path, PathBuf},
};

use enso_sys::power::Sysfs;

const LOW_POWER: &str = "low-power";
/// File name of the note, in the runtime directory.
const NOTE: &str = "enso-lock.profile";

/// The low-power profile, held until dropped.
#[derive(Debug)]
pub struct Saver {
    sysfs: Sysfs,
    /// The profile to go back to, when one was changed.
    restore: Option<String>,
    note: Option<PathBuf>,
}

impl Saver {
    /// Switch to the low-power profile, remembering the one in use. Does
    /// nothing — quietly — where there is no such dial, or no permission
    /// to turn it (see `contrib/90-enso-power-profile.rules`).
    pub fn engage(runtime_dir: Option<&Path>) -> Self {
        Self::engage_on(Sysfs::system(), runtime_dir)
    }

    fn engage_on(sysfs: Sysfs, runtime_dir: Option<&Path>) -> Self {
        let note = runtime_dir.map(|dir| dir.join(NOTE));
        let mut saver = Self {
            sysfs,
            restore: None,
            note,
        };
        let Some(profile) = saver.sysfs.platform_profile() else {
            return saver;
        };
        if !profile.writable || !profile.choices.iter().any(|choice| choice == LOW_POWER) {
            return saver;
        }
        // A note left behind is what the profile really was.
        let noted = saver
            .note
            .as_deref()
            .and_then(|note| fs::read_to_string(note).ok())
            .map(|text| text.trim().to_owned())
            .filter(|noted| profile.choices.contains(noted));
        let before = noted.unwrap_or(profile.current.clone());
        if before == LOW_POWER {
            // Already there by the user's own choice: nothing to hold.
            saver.forget();
            return saver;
        }
        if let Some(note) = &saver.note {
            let _ = fs::write(note, &before);
        }
        if profile.current == LOW_POWER || saver.sysfs.set_platform_profile(LOW_POWER).is_ok() {
            saver.restore = Some(before);
        } else {
            saver.forget();
        }
        saver
    }

    fn forget(&self) {
        if let Some(note) = &self.note {
            let _ = fs::remove_file(note);
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        let Some(before) = self.restore.take() else {
            return;
        };
        // Only undo what is still ours: if the profile was changed while
        // the screen was locked, that choice stands.
        let ours = self
            .sysfs
            .platform_profile()
            .is_some_and(|profile| profile.current == LOW_POWER);
        if ours && let Err(err) = self.sysfs.set_platform_profile(&before) {
            eprintln!("enso-lock: cannot restore the {before} power profile: {err}");
        }
        self.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pretend `/sys` with a platform profile, and a runtime directory.
    fn machine(name: &str, current: &str) -> (PathBuf, Sysfs) {
        let root = std::env::temp_dir().join(format!("enso-lock-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let acpi = root.join("sys/firmware/acpi");
        fs::create_dir_all(&acpi).unwrap();
        fs::write(acpi.join("platform_profile"), current).unwrap();
        fs::write(
            acpi.join("platform_profile_choices"),
            "low-power balanced performance",
        )
        .unwrap();
        let sysfs = Sysfs::at(root.join("sys"));
        (root, sysfs)
    }

    fn current(sysfs: &Sysfs) -> String {
        sysfs.platform_profile().unwrap().current
    }

    #[test]
    fn holds_low_power_and_puts_the_profile_back() {
        let (root, sysfs) = machine("hold", "performance");
        let saver = Saver::engage_on(sysfs.clone(), Some(&root));
        assert_eq!(current(&sysfs), LOW_POWER);
        assert_eq!(fs::read_to_string(root.join(NOTE)).unwrap(), "performance");
        drop(saver);
        assert_eq!(current(&sysfs), "performance");
        assert!(!root.join(NOTE).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_profile_chosen_while_locked_is_left_alone() {
        let (root, sysfs) = machine("changed", "balanced");
        let saver = Saver::engage_on(sysfs.clone(), Some(&root));
        sysfs.set_platform_profile("performance").unwrap();
        drop(saver);
        assert_eq!(current(&sysfs), "performance");

        // Already in low-power by choice: nothing is held, nothing noted.
        sysfs.set_platform_profile(LOW_POWER).unwrap();
        let saver = Saver::engage_on(sysfs.clone(), Some(&root));
        assert!(!root.join(NOTE).exists());
        drop(saver);
        assert_eq!(current(&sysfs), LOW_POWER);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_from_a_lock_screen_that_died_holding_it() {
        let (root, sysfs) = machine("died", LOW_POWER);
        fs::write(root.join(NOTE), "balanced\n").unwrap();
        let saver = Saver::engage_on(sysfs.clone(), Some(&root));
        assert_eq!(current(&sysfs), LOW_POWER);
        drop(saver);
        assert_eq!(current(&sysfs), "balanced");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_machine_without_the_dial_is_left_untouched() {
        let root = std::env::temp_dir().join(format!("enso-lock-{}-none", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let saver = Saver::engage_on(Sysfs::at(root.join("sys")), Some(&root));
        assert!(saver.restore.is_none());
        drop(saver);
        assert!(!root.join(NOTE).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
