//! Lists that aren't a folder: the files used lately, and what a search
//! finds. Each comes back as a [`Listing`] rooted where everything in it
//! is — `/` for recent files, the folder searched for results — with each
//! entry's name its path from there, so opening, statting, and every
//! operation work on it as on a folder's.
//!
//! Search walks the folder with the launcher's walker (git's ignore files
//! honoured, build output and caches left out), then ranks names with its
//! fuzzy matcher; or, searching inside files, reads each text file and
//! keeps the first line that matches. [`crate::query`]'s words narrow
//! either.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use enso_fs::{
    grep,
    list::{self, Kind, Listing, Meta},
    places, recent,
    search::{self, FileIndex, Scope},
};

use crate::query::Query;

/// Results kept at most.
pub const MOST: usize = 1000;
/// How long a walk of a folder answers for it.
const FRESH: Duration = Duration::from_secs(20);

/// A gathered list: the listing, and each entry's note (a search's
/// matching line), by index.
#[derive(Debug, Default)]
pub struct Gathered {
    pub listing: Listing,
    pub notes: Vec<String>,
}

/// What's in the folder `inner` of the archive at `archive`: its own
/// files and folders, sized as the archive says.
pub fn archive(archive: &Path, inner: &str) -> Result<Gathered, String> {
    let members =
        enso_fs::archive::list(archive).map_err(|err| format!("Can't look inside: {err}"))?;
    let prefix = if inner.is_empty() {
        String::new()
    } else {
        format!("{inner}/")
    };
    // Each child once: a folder may be named only by what's in it.
    let mut children: std::collections::BTreeMap<String, (bool, u64)> = Default::default();
    for member in &members {
        let Some(rest) = member.path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((folder, _)) => {
                children.entry(folder.to_owned()).or_insert((true, 0));
            }
            None => {
                let entry = children
                    .entry(rest.to_owned())
                    .or_insert((member.is_dir, member.size));
                *entry = (member.is_dir || entry.0, member.size);
            }
        }
    }
    let base = if inner.is_empty() {
        archive.to_owned()
    } else {
        archive.join(inner)
    };
    let mut listing = Listing::from_entries(
        &base,
        children.iter().map(|(name, (is_dir, _))| {
            (name.as_str(), if *is_dir { Kind::Dir } else { Kind::File })
        }),
    );
    // Sizes from the archive; the time, the archive's own.
    let modified = fs::metadata(archive)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| jiff::Timestamp::try_from(time).ok())
        .map_or(0, |time| time.as_nanosecond() as i64);
    let metas: Vec<Option<Meta>> = children
        .values()
        .map(|(is_dir, size)| {
            Some(Meta {
                size: if *is_dir { 0 } else { *size },
                modified,
                created: None,
                accessed: modified,
                mode: if *is_dir { 0o755 } else { 0o644 },
                uid: 0,
                gid: 0,
                link_size: None,
            })
        })
        .collect();
    listing.apply(0, &metas);
    Ok(Gathered {
        listing,
        notes: Vec::new(),
    })
}

/// The files used lately that are still there, newest first.
pub fn recent() -> Gathered {
    let found = recent::read(&recent::file(), MOST);
    let entries: Vec<(String, Kind)> = found
        .into_iter()
        .filter_map(|recent| {
            let meta = fs::metadata(&recent.path).ok()?;
            let name = recent.path.to_str()?.strip_prefix('/')?.to_owned();
            Some((name, if meta.is_dir() { Kind::Dir } else { Kind::File }))
        })
        .collect();
    Gathered {
        listing: Listing::from_entries(
            "/",
            entries.iter().map(|(name, kind)| (name.as_str(), *kind)),
        ),
        notes: Vec::new(),
    }
}

/// Search below `root` for `query`: in names, or (`content`) inside files.
pub fn search(
    root: &Path,
    hidden: bool,
    query: &str,
    content: bool,
    cancel: &AtomicBool,
) -> Result<Gathered, String> {
    let query = Query::parse(query);
    if query.text.is_empty() && !query.narrows() {
        return Ok(Gathered::default());
    }
    let matcher = if content {
        if query.text.is_empty() {
            return Ok(Gathered::default());
        }
        Some(grep::matcher(&query.text)?)
    } else {
        None
    };
    let index = walked(root, hidden);
    if cancel.load(Ordering::Relaxed) {
        return Ok(Gathered::default());
    }
    let prefix = format!("{}/", root.to_string_lossy().trim_end_matches('/'));
    let relative = |path: &str| path.strip_prefix(&prefix).map(str::to_owned);
    // By name: the matcher's best, a few times over if words will narrow
    // them. Inside files, or by words alone: everything, in walk order.
    let candidates: Vec<(String, bool)> = if !content && !query.text.is_empty() {
        let wanted = if query.narrows() { MOST * 8 } else { MOST };
        index
            .search(&query.text, wanted)
            .into_iter()
            .filter_map(|hit| Some((relative(&hit.path)?, hit.is_dir)))
            .collect()
    } else {
        index
            .paths()
            .filter(|(_, is_dir)| !(content && *is_dir))
            .filter_map(|(path, is_dir)| Some((relative(path)?, is_dir)))
            .collect()
    };
    let candidates = narrow(root, &query, candidates, cancel);
    let mut entries = Vec::new();
    let mut notes = Vec::new();
    for (name, is_dir) in candidates {
        if cancel.load(Ordering::Relaxed) || entries.len() >= MOST {
            break;
        }
        let note = match &matcher {
            Some(matcher) => match grep::first_match(&root.join(&name), matcher) {
                Some(line) => line,
                None => continue,
            },
            None => String::new(),
        };
        entries.push((name, if is_dir { Kind::Dir } else { Kind::File }));
        notes.push(note);
    }
    Ok(Gathered {
        listing: Listing::from_entries(
            root,
            entries.iter().map(|(name, kind)| (name.as_str(), *kind)),
        ),
        notes,
    })
}

/// Everything below `root`: walked afresh, unless it was walked moments
/// ago — a search refined as it's typed doesn't walk the disk each time.
fn walked(root: &Path, hidden: bool) -> Arc<FileIndex> {
    type Walk = (PathBuf, bool, Instant, Arc<FileIndex>);
    static LAST: Mutex<Option<Walk>> = Mutex::new(None);
    if let Ok(last) = LAST.lock()
        && let Some((at, was_hidden, when, index)) = last.as_ref()
        && at == root
        && *was_hidden == hidden
        && when.elapsed() < FRESH
    {
        return index.clone();
    }
    let home = places::home();
    let scope = Scope::new(&[root.to_string_lossy().into_owned()], hidden, &[], &home);
    let index = Arc::new(search::walk(&scope));
    if let Ok(mut last) = LAST.lock() {
        *last = Some((root.to_owned(), hidden, Instant::now(), index.clone()));
    }
    index
}

/// Keep the candidates the query's words admit, statting them if it
/// takes their size or age.
fn narrow(
    root: &Path,
    query: &Query,
    candidates: Vec<(String, bool)>,
    cancel: &AtomicBool,
) -> Vec<(String, bool)> {
    if !query.narrows() {
        return candidates;
    }
    let base = |name: &str| name.rsplit('/').next().unwrap_or(name).to_owned();
    if !query.needs_meta() {
        return candidates
            .into_iter()
            .filter(|(name, is_dir)| query.admits(&base(name), *is_dir, None, 0))
            .collect();
    }
    let listing = Listing::from_entries(
        root,
        candidates
            .iter()
            .map(|(name, is_dir)| (name.as_str(), if *is_dir { Kind::Dir } else { Kind::File })),
    );
    let metas = std::sync::Mutex::new(vec![None::<Meta>; listing.len()]);
    let _ = list::stat(&listing, list::threads(), cancel, |start, batch| {
        if let Ok(mut metas) = metas.lock() {
            metas[start..start + batch.len()].copy_from_slice(&batch);
        }
    });
    let metas = metas.into_inner().unwrap_or_default();
    let now = jiff::Timestamp::now().as_nanosecond() as i64;
    candidates
        .into_iter()
        .zip(metas)
        .filter(|((name, is_dir), meta)| query.admits(&base(name), *is_dir, meta.as_ref(), now))
        .map(|(candidate, _)| candidate)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_names_contents_and_words() {
        let root = std::env::temp_dir().join(format!("enso-files-gather-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        // A Rust project: its `target` is build output.
        fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello, Sky\");\n}\n",
        )
        .unwrap();
        fs::write(root.join("src/deep/helper.rs"), "// helps\nfn help() {}\n").unwrap();
        fs::write(root.join("notes.md"), "a note about the sky\n").unwrap();
        fs::write(root.join("blob.bin"), b"sky\0\0\0").unwrap();
        fs::write(root.join("target/main.rs"), "sky").unwrap();
        let none = AtomicBool::new(false);
        let names = |found: &Gathered| -> Vec<String> {
            (0..found.listing.len())
                .map(|i| found.listing.display_name(i).into_owned())
                .collect()
        };

        let found = search(&root, false, "helper", false, &none).unwrap();
        assert_eq!(names(&found), ["src/deep/helper.rs"]);
        assert_eq!(found.listing.path(0), root.join("src/deep/helper.rs"));

        // Inside files: build output and binaries left out, the line kept.
        let mut found = search(&root, false, "sky", true, &none).unwrap();
        let mut pairs: Vec<_> = names(&found)
            .into_iter()
            .zip(found.notes.drain(..))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            [
                ("notes.md".to_owned(), "a note about the sky".to_owned()),
                (
                    "src/main.rs".to_owned(),
                    "println!(\"Hello, Sky\");".to_owned()
                ),
            ]
        );
        // Capitals mean it: only the exact case.
        let found = search(&root, false, "Sky", true, &none).unwrap();
        assert_eq!(names(&found), ["src/main.rs"]);
        let found = search(&root, false, "/fn [a-z]+\\(\\)/", true, &none).unwrap();
        assert_eq!(found.listing.len(), 2);
        assert!(search(&root, false, "/fn (/", true, &none).is_err());

        // Words alone: every Rust file here.
        let mut found = names(&search(&root, false, "ext:rs", false, &none).unwrap());
        found.sort();
        assert_eq!(found, ["src/deep/helper.rs", "src/main.rs"]);
        fs::remove_dir_all(root).unwrap();
    }
}
