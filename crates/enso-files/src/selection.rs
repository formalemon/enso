//! What is chosen: the basket.
//!
//! The selection is made of paths, not rows, so it outlives the folder it
//! was made in: pick two photos here, go elsewhere, pick a document, and
//! all three are chosen. It keeps the order things were chosen in, which is
//! the order they are handed over.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default)]
pub struct Selection {
    order: Vec<PathBuf>,
    set: HashSet<PathBuf>,
}

impl Selection {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.set.contains(path)
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.order
    }

    /// Choose `path` (again: no change).
    pub fn insert(&mut self, path: PathBuf) {
        if self.set.insert(path.clone()) {
            self.order.push(path);
        }
    }

    pub fn remove(&mut self, path: &Path) {
        if self.set.remove(path) {
            self.order.retain(|chosen| chosen != path);
        }
    }

    /// Choose `path` if it wasn't, unchoose it if it was. Returns whether
    /// it is chosen now.
    pub fn toggle(&mut self, path: PathBuf) -> bool {
        if self.contains(&path) {
            self.remove(&path);
            false
        } else {
            self.insert(path);
            true
        }
    }

    /// Choose only `path`.
    pub fn only(&mut self, path: PathBuf) {
        self.clear();
        self.insert(path);
    }

    pub fn clear(&mut self) {
        self.order.clear();
        self.set.clear();
    }

    /// Unchoose everything chosen in `folder`, keeping what was chosen
    /// elsewhere.
    pub fn clear_in(&mut self, folder: &Path) {
        self.order.retain(|path| path.parent() != Some(folder));
        self.set.retain(|path| path.parent() != Some(folder));
    }

    /// How many folders the chosen things are in.
    pub fn folders(&self) -> usize {
        self.order
            .iter()
            .filter_map(|path| path.parent())
            .collect::<HashSet<_>>()
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_cross_folders_and_keep_their_order() {
        let mut chosen = Selection::default();
        chosen.insert("/photos/b.jpg".into());
        chosen.insert("/photos/a.jpg".into());
        chosen.insert("/docs/cv.pdf".into());
        chosen.insert("/photos/b.jpg".into());
        assert_eq!(chosen.len(), 3);
        assert_eq!(chosen.folders(), 2);
        assert_eq!(
            chosen.paths(),
            [
                PathBuf::from("/photos/b.jpg"),
                "/photos/a.jpg".into(),
                "/docs/cv.pdf".into()
            ]
        );
        chosen.clear_in(Path::new("/photos"));
        assert_eq!(chosen.paths(), [PathBuf::from("/docs/cv.pdf")]);
    }

    #[test]
    fn toggling_and_choosing_one() {
        let mut chosen = Selection::default();
        assert!(chosen.toggle("/a".into()));
        assert!(chosen.toggle("/b".into()));
        assert!(!chosen.toggle("/a".into()));
        assert_eq!(chosen.paths(), [PathBuf::from("/b")]);
        chosen.only("/c".into());
        assert_eq!(chosen.paths(), [PathBuf::from("/c")]);
        assert!(!chosen.contains(Path::new("/b")));
    }
}
