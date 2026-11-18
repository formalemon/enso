//! The places a file manager's sidebar starts with: home, the XDG user
//! folders (Downloads, Pictures, …), and how much room is left where.
//!
//! The user folders are read from `user-dirs.dirs`, the file
//! `xdg-user-dirs` writes, where a folder can be renamed or translated;
//! without it the usual English names are assumed. Only folders that exist
//! are offered.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

/// One of the XDG user folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UserDir {
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Templates,
    Public,
}

impl UserDir {
    /// In the order a sidebar shows them.
    pub const ALL: [Self; 8] = [
        Self::Desktop,
        Self::Documents,
        Self::Downloads,
        Self::Music,
        Self::Pictures,
        Self::Videos,
        Self::Templates,
        Self::Public,
    ];

    /// The key in `user-dirs.dirs`, without `XDG_` and `_DIR`.
    fn key(self) -> &'static str {
        match self {
            Self::Desktop => "DESKTOP",
            Self::Documents => "DOCUMENTS",
            Self::Downloads => "DOWNLOAD",
            Self::Music => "MUSIC",
            Self::Pictures => "PICTURES",
            Self::Videos => "VIDEOS",
            Self::Templates => "TEMPLATES",
            Self::Public => "PUBLICSHARE",
        }
    }

    /// The folder's usual name, and what it is called when shown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "Desktop",
            Self::Documents => "Documents",
            Self::Downloads => "Downloads",
            Self::Music => "Music",
            Self::Pictures => "Pictures",
            Self::Videos => "Videos",
            Self::Templates => "Templates",
            Self::Public => "Public",
        }
    }
}

/// The user's home directory.
pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn config_home(home: &Path) -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}

/// Where each user folder is, whether or not it exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDirs {
    dirs: Vec<(UserDir, PathBuf)>,
}

impl UserDirs {
    /// This user's folders.
    pub fn load() -> Self {
        let home = home();
        let file =
            fs::read_to_string(config_home(&home).join("user-dirs.dirs")).unwrap_or_default();
        Self::parse(&file, &home)
    }

    /// Folders from the text of a `user-dirs.dirs`; any it doesn't mention
    /// take their usual name in `home`. A folder set to `$HOME` itself is
    /// how a user says they don't want it, so it is left out.
    pub fn parse(file: &str, home: &Path) -> Self {
        let dirs = UserDir::ALL
            .into_iter()
            .filter_map(|dir| {
                let path = match lookup(file, dir.key()) {
                    Some(value) => expand(&value, home)?,
                    None => home.join(dir.label()),
                };
                (path != home).then_some((dir, path))
            })
            .collect();
        Self { dirs }
    }

    pub fn get(&self, dir: UserDir) -> Option<&Path> {
        self.dirs
            .iter()
            .find(|(which, _)| *which == dir)
            .map(|(_, path)| path.as_path())
    }

    /// The folders that exist, in sidebar order.
    pub fn existing(&self) -> impl Iterator<Item = (UserDir, &Path)> {
        self.dirs
            .iter()
            .filter(|(_, path)| path.is_dir())
            .map(|(dir, path)| (*dir, path.as_path()))
    }
}

/// The quoted value of `XDG_<key>_DIR=` in `file`, if set.
fn lookup(file: &str, key: &str) -> Option<String> {
    let wanted = format!("XDG_{key}_DIR");
    file.lines().map(str::trim).find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name.trim() != wanted {
            return None;
        }
        let value = value.trim();
        let value = value.strip_prefix('"')?.strip_suffix('"')?;
        Some(value.replace("\\\"", "\"").replace("\\\\", "\\"))
    })
}

/// A value as `user-dirs.dirs` allows it: `$HOME/…` or an absolute path.
fn expand(value: &str, home: &Path) -> Option<PathBuf> {
    if value == "$HOME" {
        return Some(home.to_owned());
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        return Some(home.join(rest));
    }
    value.starts_with('/').then(|| PathBuf::from(value))
}

/// Room on a filesystem, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    /// What an unprivileged user can still write: `statvfs`'s `f_bavail`,
    /// not `f_bfree`, which counts the part reserved for root.
    pub free: u64,
}

impl Space {
    /// How full, `0.0..=1.0`.
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        1.0 - (self.free as f64 / self.total as f64) as f32
    }
}

/// Room on the filesystem holding `path`.
pub fn space(path: &Path) -> io::Result<Space> {
    let stat = rustix::fs::statvfs(path)?;
    Ok(Space {
        total: stat.f_blocks.saturating_mul(stat.f_frsize),
        free: stat.f_bavail.saturating_mul(stat.f_frsize),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dirs_come_from_the_file_or_their_usual_names() {
        let home = Path::new("/home/me");
        let file = r#"
# This file is written by xdg-user-dirs-update
XDG_DOWNLOAD_DIR="$HOME/Téléchargements"
XDG_PICTURES_DIR="/data/photos"
XDG_PUBLICSHARE_DIR="$HOME"
XDG_MUSIC_DIR=unquoted
"#;
        let dirs = UserDirs::parse(file, home);
        assert_eq!(
            dirs.get(UserDir::Downloads),
            Some(Path::new("/home/me/Téléchargements"))
        );
        assert_eq!(dirs.get(UserDir::Pictures), Some(Path::new("/data/photos")));
        // Set to home itself: turned off.
        assert_eq!(dirs.get(UserDir::Public), None);
        // Malformed: as if it wasn't there.
        assert_eq!(dirs.get(UserDir::Music), Some(Path::new("/home/me/Music")));
        assert_eq!(
            dirs.get(UserDir::Documents),
            Some(Path::new("/home/me/Documents"))
        );
    }

    #[test]
    fn only_existing_folders_are_offered() {
        let root = std::env::temp_dir().join(format!("enso-fs-places-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Downloads")).unwrap();
        fs::create_dir_all(root.join("Videos")).unwrap();
        let dirs = UserDirs::parse("", &root);
        let existing: Vec<UserDir> = dirs.existing().map(|(dir, _)| dir).collect();
        assert_eq!(existing, [UserDir::Downloads, UserDir::Videos]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn space_is_measured() {
        let space = space(Path::new("/")).unwrap();
        assert!(space.total > 0);
        assert!(space.free <= space.total);
        assert!((0.0..=1.0).contains(&space.used_fraction()));
    }
}
