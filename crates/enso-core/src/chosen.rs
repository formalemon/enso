//! Settings chosen in enso's own interface — the control center's Screen &
//! sleep page — kept apart from `config.toml`, which is yours to write and
//! may well be a link into your dotfiles.
//!
//! They live in the state directory (`settings.toml`), in the same tables
//! as `config.toml`, and are laid over it key by key as it is read: what
//! was chosen here wins, everything else comes from the config file.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{Config, Paths, disk};

/// The file, in the state directory.
pub const FILE: &str = "settings.toml";

/// When the screen locks, goes dark, and sleeps: the `[lock]` defaults that
/// the lock screen and the control center share.
pub mod lock {
    /// Seconds without input before the watcher locks the screen.
    pub const IDLE_AFTER: u64 = 300;
    /// Seconds without input on the lock screen before the screens go dark.
    pub const SCREEN_OFF_AFTER: u64 = 30;
    /// Seconds without input on the lock screen before the machine sleeps;
    /// `0` never.
    pub const SUSPEND_AFTER: u64 = 0;
    /// Lock just before the machine sleeps.
    pub const BEFORE_SLEEP: bool = true;
}

/// Do not disturb, as chosen: whether it's on, and when it ends by itself
/// (seconds since the Unix epoch; `0` when it lasts until turned off). The
/// notification daemon, the bar's bell, and the control center all read it
/// here, so they agree on when it's over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Quiet {
    #[serde(default)]
    pub do_not_disturb: bool,
    #[serde(default)]
    pub do_not_disturb_until: u64,
}

impl Quiet {
    /// As `config.toml` and the interface have it.
    pub fn read(paths: &Paths) -> Self {
        section(paths, "notify")
    }

    /// Whether it holds at `now` (seconds since the epoch).
    pub fn is_on(self, now: u64) -> bool {
        self.do_not_disturb && (self.do_not_disturb_until == 0 || now < self.do_not_disturb_until)
    }

    /// When it ends by itself, if it's on and does.
    pub fn ends(self, now: u64) -> Option<u64> {
        (self.is_on(now) && self.do_not_disturb_until > 0).then_some(self.do_not_disturb_until)
    }

    /// Choose it: on until turned off (`until` 0), on until then, or off.
    pub fn set(paths: &Paths, on: bool, until: u64) -> io::Result<()> {
        set(
            paths,
            "notify",
            "do_not_disturb_until",
            i64::try_from(until).unwrap_or(i64::MAX),
        )?;
        set(paths, "notify", "do_not_disturb", on)
    }
}

pub fn path(paths: &Paths) -> PathBuf {
    paths.state_dir.join(FILE)
}

/// What has been chosen, as tables. Missing is nothing chosen.
fn read(path: &Path) -> Result<toml::Table, String> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml::Table>()
            .map_err(|err| format!("{}: {err}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// Lay what was chosen over `sections`, key by key within each table.
pub(crate) fn overlay(paths: &Paths, sections: &mut toml::Table) -> Result<(), String> {
    for (name, chosen) in read(&path(paths))? {
        let toml::Value::Table(chosen) = chosen else {
            continue;
        };
        let table = sections
            .entry(name)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(table) = table {
            table.extend(chosen);
        }
    }
    Ok(())
}

/// One tool's table — `config.toml`'s, with what was chosen over it —
/// without loading anything else (themes, say): quick enough to call each
/// time the state directory changes. Whatever can't be read is defaults.
pub fn section<T: serde::de::DeserializeOwned + Default>(paths: &Paths, name: &str) -> T {
    let mut config = Config::load(paths).unwrap_or_default();
    let _ = config.overlay_chosen(paths);
    config.section(name).unwrap_or_default()
}

/// Choose `value` for `key` in the `[section]` table, keeping everything
/// else that was chosen.
pub fn set(
    paths: &Paths,
    section: &str,
    key: &str,
    value: impl Into<toml::Value>,
) -> io::Result<()> {
    let path = path(paths);
    let mut chosen = read(&path).map_err(io::Error::other)?;
    let table = chosen
        .entry(section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(table) = table else {
        return Err(io::Error::other(format!("[{section}] isn't a table")));
    };
    table.insert(key.to_owned(), value.into());
    let text = format!(
        "# Chosen in enso's interface; laid over config.toml as it is read.\n{}",
        toml::to_string(&chosen).map_err(io::Error::other)?
    );
    disk::write_atomic(&path, text.as_bytes(), 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, test_dir};

    #[test]
    fn chosen_settings_win_key_by_key() {
        let root = test_dir("chosen");
        let paths = Paths::rooted(&root);
        fs::create_dir_all(paths.config_file().parent().unwrap()).unwrap();
        fs::write(
            paths.config_file(),
            "[lock]\nidle_after = 600\nclock = \"%H.%M\"\n",
        )
        .unwrap();
        set(&paths, "lock", "idle_after", 120).unwrap();
        set(&paths, "lock", "before_sleep", false).unwrap();

        let mut ctx = Context::load_from(paths);
        let lock: toml::Table = ctx.section("lock");
        assert_eq!(lock["idle_after"].as_integer(), Some(120));
        assert_eq!(lock["before_sleep"].as_bool(), Some(false));
        // What wasn't chosen is the config file's.
        assert_eq!(lock["clock"].as_str(), Some("%H.%M"));
        assert!(ctx.warnings.is_empty(), "{:?}", ctx.warnings);
        // The quick way in reads the same.
        let quick: toml::Table = section(&ctx.paths, "lock");
        assert_eq!(quick, lock);
        let _ = fs::remove_dir_all(root);
    }
}
