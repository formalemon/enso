//! The history, kept where only this user can read it and nothing outlives
//! the session: `$XDG_RUNTIME_DIR/enso-clip/`, a file per thing copied
//! (`7.txt`, `8.png`) and an `index` of their numbers, newest first.
//! Nothing is written to the disk proper.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// How many things are kept.
pub const KEEP: usize = 50;
/// The most a text may be to be kept, in bytes.
pub const MAX_TEXT: usize = 1 << 20;
/// The most a picture may be, in bytes.
pub const MAX_PICTURE: usize = 16 << 20;

/// One thing copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clip {
    Text(String),
    /// A PNG, as it was copied.
    Png(Vec<u8>),
}

impl Clip {
    fn extension(&self) -> &'static str {
        match self {
            Self::Text(_) => "txt",
            Self::Png(_) => "png",
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Png(png) => png,
        }
    }
}

/// The history's directory.
pub fn dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    runtime.is_absolute().then(|| runtime.join("enso-clip"))
}

/// The history as the daemon keeps it: numbers, newest first.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    ids: Vec<u64>,
    next: u64,
}

impl Store {
    /// A fresh history: whatever an earlier daemon left is cleared away.
    pub fn new(dir: PathBuf) -> io::Result<Self> {
        let _ = fs::remove_dir_all(&dir);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        let store = Self {
            dir,
            ids: Vec::new(),
            next: 1,
        };
        store.write_index()?;
        Ok(store)
    }

    /// Keep `clip` as the newest; the same thing copied again moves to the
    /// top rather than being kept twice. Returns its number.
    pub fn add(&mut self, clip: &Clip) -> io::Result<u64> {
        if let Some(position) = self
            .ids
            .iter()
            .position(|id| read(&self.dir, *id).as_ref() == Some(clip))
        {
            let id = self.ids.remove(position);
            self.ids.insert(0, id);
            self.write_index()?;
            return Ok(id);
        }
        let id = self.next;
        self.next += 1;
        write_private(
            &self.dir.join(format!("{id}.{}", clip.extension())),
            clip.bytes(),
        )?;
        self.ids.insert(0, id);
        for old in self.ids.split_off(self.ids.len().min(KEEP)) {
            remove(&self.dir, old);
        }
        self.write_index()?;
        Ok(id)
    }

    /// Forget one.
    pub fn delete(&mut self, id: u64) -> io::Result<()> {
        self.ids.retain(|kept| *kept != id);
        remove(&self.dir, id);
        self.write_index()
    }

    /// Forget everything.
    pub fn clear(&mut self) -> io::Result<()> {
        for id in std::mem::take(&mut self.ids) {
            remove(&self.dir, id);
        }
        self.write_index()
    }

    pub fn get(&self, id: u64) -> Option<Clip> {
        self.ids
            .contains(&id)
            .then(|| read(&self.dir, id))
            .flatten()
    }

    pub fn newest(&self) -> Option<Clip> {
        self.ids.first().and_then(|id| read(&self.dir, *id))
    }

    fn write_index(&self) -> io::Result<()> {
        let text: String = self.ids.iter().map(|id| format!("{id}\n")).collect();
        let temp = self.dir.join("index.new");
        write_private(&temp, text.as_bytes())?;
        fs::rename(temp, self.dir.join("index"))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

fn remove(dir: &Path, id: u64) {
    for extension in ["txt", "png"] {
        let _ = fs::remove_file(dir.join(format!("{id}.{extension}")));
    }
}

/// One kept thing, read back.
pub fn read(dir: &Path, id: u64) -> Option<Clip> {
    if let Ok(text) = fs::read_to_string(dir.join(format!("{id}.txt"))) {
        return Some(Clip::Text(text));
    }
    fs::read(dir.join(format!("{id}.png"))).ok().map(Clip::Png)
}

/// The history as the picker sees it: numbers, newest first.
pub fn list(dir: &Path) -> Vec<u64> {
    fs::read_to_string(dir.join("index"))
        .map(|text| {
            text.lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_first_without_twins() {
        let dir = std::env::temp_dir().join(format!("enso-clip-{}", std::process::id()));
        let mut store = Store::new(dir.clone()).unwrap();
        let a = store.add(&Clip::Text("tea".into())).unwrap();
        let b = store.add(&Clip::Png(vec![1, 2, 3])).unwrap();
        assert_eq!(list(&dir), [b, a]);
        // Copied again: to the top, still once.
        assert_eq!(store.add(&Clip::Text("tea".into())).unwrap(), a);
        assert_eq!(list(&dir), [a, b]);
        assert_eq!(read(&dir, b), Some(Clip::Png(vec![1, 2, 3])));
        store.delete(a).unwrap();
        assert_eq!(list(&dir), [b]);
        for n in 0..KEEP + 5 {
            store.add(&Clip::Text(format!("n{n}"))).unwrap();
        }
        assert_eq!(list(&dir).len(), KEEP);
        assert_eq!(store.newest(), Some(Clip::Text(format!("n{}", KEEP + 4))));
        store.clear().unwrap();
        assert!(list(&dir).is_empty());
        // Only this user may look.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_dir_all(dir);
    }
}
