//! Writing a file so that whoever reads it sees the old contents or the
//! new, whole — never half — and so that after a crash it's the same.
//!
//! The new contents go to a file beside the old one, reach the disk, and
//! then take the old one's name in a single step. Writing through a
//! symbolic link writes where it points, and leaves the link a link.

use std::{
    fs,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// For files only their owner may read: passwords, history, what was
/// opened when.
pub const PRIVATE: u32 = 0o600;
/// For files anyone may read: settings, themes, indexes.
pub const SHARED: u32 = 0o644;

/// Replace `path`'s contents with `bytes`, made with `mode` (less the
/// umask), creating the folders it's in if need be.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let target = through_links(path);
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let tmp = beside(dir, &target);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            // On the disk before it takes the old one's place, so a crash
            // leaves one or the other, not an empty file.
            file.sync_all()
        });
    let result = written.and_then(|()| fs::rename(&tmp, &target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Where a link at `path` points, followed to the end; `path` itself if
/// it isn't a link (or leads nowhere yet).
fn through_links(path: &Path) -> PathBuf {
    let is_link = fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link && let Ok(target) = fs::canonicalize(path) {
        return target;
    }
    path.to_owned()
}

/// A name for the new contents beside `path`: hidden, and its own to this
/// process and this write.
fn beside(dir: &Path, path: &Path) -> PathBuf {
    static WRITES: AtomicU32 = AtomicU32::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let count = WRITES.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{name}.{}-{count}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn replaces_whole_with_its_mode_and_leaves_nothing_behind() {
        let dir = crate::test_dir("atomic");
        let file = dir.join("deep/state.toml");
        write_atomic(&file, b"one", PRIVATE).unwrap();
        write_atomic(&file, b"two", PRIVATE).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let left: Vec<_> = fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1, "a temporary file was left behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writes_through_a_link_and_keeps_it() {
        let dir = crate::test_dir("atomic-link");
        fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let real = dir.join("dotfiles/theme.toml");
        fs::write(&real, "old").unwrap();
        let link = dir.join("theme.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new", SHARED).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        fs::remove_dir_all(dir).unwrap();
    }
}
