//! The location typed as text (`Ctrl+L`): where it leads, and Tab
//! completing it.
//!
//! `~` is home, a relative path is taken from the folder being shown, and a
//! `file://` URI or a path to a file are welcome too — pasting either is a
//! common way to get somewhere.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// What typing `text` and pressing Enter means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Folder(PathBuf),
    /// A file: go to its folder, with the cursor on it.
    File(PathBuf),
    /// Nowhere that exists.
    Missing,
}

/// `text` as a path: `~` expanded, relative to `from`.
pub fn resolve(text: &str, from: &Path, home: &Path) -> PathBuf {
    let text = text.trim();
    if let Some(path) = enso_fs::uri::to_path(text) {
        return path;
    }
    let path = if text == "~" {
        home.to_owned()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        from.join(text)
    };
    normalize(&path)
}

/// Take out `.` and `..` without asking the disk (a `..` after a link goes
/// up from the link, as a shell's `cd` does).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

pub fn target(text: &str, from: &Path, home: &Path) -> Target {
    let path = resolve(text, from, home);
    if path.is_dir() {
        Target::Folder(path)
    } else if path.exists() {
        Target::File(path)
    } else {
        Target::Missing
    }
}

/// `path` as it is typed: under home, with `~`.
pub fn display(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~/".to_owned(),
        Ok(rest) => format!("~/{}/", rest.display()),
        Err(_) if path == Path::new("/") => "/".to_owned(),
        Err(_) => format!("{}/", path.display()),
    }
}

/// Tab: complete the last part of `text` from the folders (and, with
/// `files`, the files) that start with it, ignoring case — all of a lone
/// match, with a `/` after a folder; as much as the matches share
/// otherwise. `None` when nothing matches or there is nothing to add.
pub fn complete(text: &str, from: &Path, home: &Path, files: bool, hidden: bool) -> Option<String> {
    let (typed_dir, partial) = match text.rfind('/') {
        Some(slash) => (&text[..=slash], &text[slash + 1..]),
        None if text == "~" => return Some("~/".to_owned()),
        None => ("", text),
    };
    let dir = if typed_dir.is_empty() {
        from.to_owned()
    } else {
        resolve(typed_dir, from, home)
    };
    let lower = partial.to_lowercase();
    let mut matches: Vec<(String, bool)> = fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            let is_dir = entry.path().is_dir();
            let wanted = (is_dir || files)
                && name.to_lowercase().starts_with(&lower)
                && (hidden || partial.starts_with('.') || !name.starts_with('.'));
            wanted.then_some((name, is_dir))
        })
        .collect();
    matches.sort();
    let completed = match matches.as_slice() {
        [] => return None,
        [(name, is_dir)] => format!("{name}{}", if *is_dir { "/" } else { "" }),
        many => shared_prefix(many.iter().map(|(name, _)| name.as_str())),
    };
    let result = format!("{typed_dir}{completed}");
    (result != text && completed.len() >= partial.len()).then_some(result)
}

/// The longest start all `names` share, ignoring case; spelled as the
/// first of them spells it.
fn shared_prefix<'a>(mut names: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = names.next() else {
        return String::new();
    };
    let mut length = first.len();
    for name in names {
        length = first
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a.to_lowercase().eq(b.to_lowercase()))
            .last()
            .map_or(0, |((at, a), _)| at + a.len_utf8())
            .min(length);
    }
    first[..length].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("enso-files-location-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["Documents", "Downloads", "dev/enso", ".config"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("notes.txt"), "").unwrap();
        root
    }

    #[test]
    fn typed_paths_resolve() {
        let home = Path::new("/home/me");
        let here = Path::new("/home/me/dev");
        assert_eq!(resolve("~", here, home), home);
        assert_eq!(
            resolve("~/Pictures", here, home),
            Path::new("/home/me/Pictures")
        );
        assert_eq!(
            resolve("enso/../sumi", here, home),
            Path::new("/home/me/dev/sumi")
        );
        assert_eq!(resolve("/usr/./lib/", here, home), Path::new("/usr/lib"));
        assert_eq!(resolve("../../../..", here, home), Path::new("/"));
        assert_eq!(
            resolve("file:///tmp/a%20b", here, home),
            Path::new("/tmp/a b")
        );
        assert_eq!(display(Path::new("/home/me/dev"), home), "~/dev/");
        assert_eq!(display(home, home), "~/");
        assert_eq!(display(Path::new("/usr"), home), "/usr/");
        assert_eq!(display(Path::new("/"), home), "/");
    }

    #[test]
    fn enter_goes_to_folders_and_files() {
        let root = tree("target");
        assert_eq!(
            target("dev", &root, &root),
            Target::Folder(root.join("dev"))
        );
        assert_eq!(
            target("~/notes.txt", &root, &root),
            Target::File(root.join("notes.txt"))
        );
        assert_eq!(target("nope", &root, &root), Target::Missing);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tab_completes_what_it_can() {
        let root = tree("tab");
        let complete = |text: &str| complete(text, &root, &root, false, false);
        assert_eq!(complete("de").as_deref(), Some("dev/"));
        assert_eq!(complete("~/dev/e").as_deref(), Some("~/dev/enso/"));
        // Two matches: as far as they agree.
        assert_eq!(complete("do").as_deref(), Some("Do"));
        assert_eq!(complete("Do"), None, "nothing more to add");
        assert_eq!(complete("doc").as_deref(), Some("Documents/"));
        // Files only when asked for; dotfiles only when typed or shown.
        assert_eq!(complete("no"), None);
        assert_eq!(
            super::complete("no", &root, &root, true, false).as_deref(),
            Some("notes.txt")
        );
        assert_eq!(complete(".c").as_deref(), Some(".config/"));
        assert_eq!(complete("~").as_deref(), Some("~/"));
        assert_eq!(complete("zzz"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
