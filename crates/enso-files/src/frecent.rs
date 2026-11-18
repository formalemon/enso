//! The folders you go to: how often, and how lately — "frecency", as
//! browsers and zoxide rank them. The command palette offers the best
//! first, so a folder used every day is a few letters away.
//!
//! Kept in the state directory, one folder a line: visits, the last
//! visit (seconds since 1970), the path.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use enso_core::disk;

pub const FILE: &str = "folders";
/// Folders remembered at most; the least used go first.
const MOST: usize = 500;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Visits {
    folders: HashMap<PathBuf, (u32, i64)>,
}

impl Visits {
    pub fn load(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap_or_default();
        let folders = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, ' ');
                let count = parts.next()?.parse().ok()?;
                let last = parts.next()?.parse().ok()?;
                let path = PathBuf::from(parts.next()?);
                Some((path, (count, last)))
            })
            .collect();
        Self { folders }
    }

    /// A visit to `folder`, at `now` (seconds since 1970).
    pub fn visit(&mut self, folder: &Path, now: i64) {
        let entry = self.folders.entry(folder.to_owned()).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = now;
        if self.folders.len() > MOST {
            let weakest = self
                .folders
                .iter()
                .min_by(|a, b| score(*a.1, now).total_cmp(&score(*b.1, now)))
                .map(|(path, _)| path.clone());
            if let Some(weakest) = weakest {
                self.folders.remove(&weakest);
            }
        }
    }

    /// The best `limit` folders that are still there, best first.
    pub fn best(&self, limit: usize, now: i64) -> Vec<PathBuf> {
        let mut ranked: Vec<(&PathBuf, f64, i64)> = self
            .folders
            .iter()
            .map(|(path, &visits)| (path, score(visits, now), visits.1))
            .collect();
        // Equal scores: the one gone to last first.
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)));
        ranked
            .into_iter()
            .map(|(path, ..)| path.clone())
            .filter(|path| path.is_dir())
            .take(limit)
            .collect()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut text = String::new();
        for (folder, (count, last)) in &self.folders {
            // A line per folder: a name with a line break can't be kept.
            if let Some(folder) = folder.to_str().filter(|folder| !folder.contains('\n')) {
                text.push_str(&format!("{count} {last} {folder}\n"));
            }
        }
        disk::write_atomic(path, text.as_bytes(), disk::PRIVATE)
    }
}

/// Visits, weighed by how lately: within the hour counts four times,
/// the day twice, the week as is, older a quarter.
fn score((count, last): (u32, i64), now: i64) -> f64 {
    let age = now - last;
    let weight = if age < 3_600 {
        4.0
    } else if age < 86_400 {
        2.0
    } else if age < 604_800 {
        1.0
    } else {
        0.25
    };
    f64::from(count) * weight
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn often_and_lately_come_first() {
        let root = std::env::temp_dir().join(format!("enso-files-frecent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["daily", "once", "old"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let now = 1_800_000_000;
        let mut visits = Visits::default();
        for _ in 0..3 {
            visits.visit(&root.join("old"), now - 30 * 86_400);
        }
        visits.visit(&root.join("once"), now - 60);
        for _ in 0..3 {
            visits.visit(&root.join("daily"), now - 3 * 3_600);
        }
        visits.visit(&root.join("gone"), now);
        let best = visits.best(10, now);
        assert_eq!(
            best,
            [root.join("daily"), root.join("once"), root.join("old")]
        );

        let file = root.join("state/folders");
        visits.save(&file).unwrap();
        assert_eq!(Visits::load(&file), visits);
        fs::remove_dir_all(root).unwrap();
    }
}
