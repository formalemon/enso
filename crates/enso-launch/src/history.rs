//! What was launched before, so it ranks a little higher next time.
//!
//! One small text file in the state directory: a line per application or
//! file with how often and how recently it was chosen. Losing it loses
//! nothing but the nudge.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Entries kept on disk; the least recently used are dropped beyond this.
const KEEP: usize = 400;
const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default)]
pub struct History {
    path: Option<PathBuf>,
    /// Key → (times chosen, when last, in seconds since the epoch).
    entries: HashMap<String, (u32, u64)>,
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl History {
    /// Read the history at `path`; a missing or odd file is an empty one.
    pub fn load(path: &Path) -> Self {
        let entries = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let count = fields.next()?.parse().ok()?;
                let last = fields.next()?.parse().ok()?;
                Some((fields.next()?.to_owned(), (count, last)))
            })
            .collect();
        Self {
            path: Some(path.to_owned()),
            entries,
        }
    }

    /// Points added to a match's score: more for something chosen often,
    /// and more again if that was lately.
    pub fn boost(&self, key: &str, now: u64) -> i32 {
        let Some((count, last)) = self.entries.get(key) else {
            return 0;
        };
        let recency = match now.saturating_sub(*last) {
            age if age <= DAY => 30,
            age if age <= 7 * DAY => 15,
            age if age <= 30 * DAY => 5,
            _ => 0,
        };
        (*count).min(8) as i32 * 5 + recency
    }

    /// Note that `key` was chosen, and save.
    pub fn record(&mut self, key: &str, now: u64) {
        if key.contains(['\n', '\t']) {
            return;
        }
        let entry = self.entries.entry(key.to_owned()).or_insert((0, now));
        *entry = (entry.0.saturating_add(1), now);

        let Some(path) = &self.path else {
            return;
        };
        let mut lines: Vec<(&String, &(u32, u64))> = self.entries.iter().collect();
        lines.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(b.0)));
        let text: String = lines
            .into_iter()
            .take(KEEP)
            .map(|(key, (count, last))| format!("{count}\t{last}\t{key}\n"))
            .collect();
        let saved = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(path, text));
        if let Err(err) = saved {
            eprintln!("enso-launch: cannot save the launch history: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_how_often_and_how_recently() {
        let dir = std::env::temp_dir().join(format!("enso-launch-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("state/launch-history");
        let mut history = History::load(&path);
        let t0 = 1_800_000_000;
        assert_eq!(history.boost("app:firefox", t0), 0);

        history.record("app:firefox", t0);
        history.record("app:firefox", t0);
        history.record("file:/home/mira/tea.md", t0);
        // Twice today beats once today.
        assert_eq!(history.boost("app:firefox", t0), 40);
        assert_eq!(history.boost("file:/home/mira/tea.md", t0), 35);
        // The nudge fades with time but the habit is remembered.
        assert_eq!(history.boost("app:firefox", t0 + 3 * DAY), 25);
        assert_eq!(history.boost("app:firefox", t0 + 20 * DAY), 15);
        assert_eq!(history.boost("app:firefox", t0 + 90 * DAY), 10);
        // The count stops mattering past a point.
        for _ in 0..20 {
            history.record("app:firefox", t0);
        }
        assert_eq!(history.boost("app:firefox", t0), 70);

        // It is all on disk.
        let reloaded = History::load(&path);
        assert_eq!(reloaded.boost("app:firefox", t0), 70);
        assert_eq!(reloaded.boost("file:/home/mira/tea.md", t0), 35);
        // Garbage is skipped, not fatal.
        fs::write(&path, "nonsense\n3\t100\tapp:ok\n").unwrap();
        let odd = History::load(&path);
        assert_eq!(odd.entries.len(), 1);
        assert_eq!(odd.boost("app:ok", 100), 45);
        fs::remove_dir_all(dir).unwrap();
    }
}
