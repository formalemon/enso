//! Doing things to files: copying, moving, deleting, renaming, making
//! folders — with progress, a way to stop, a say in what happens when a
//! name is taken, and a record of what was done so it can be undone.
//!
//! These run on a worker thread; a [`Watcher`] is how they talk to the
//! window while they do: progress as it goes, a question when a name is
//! taken, and whether to stop.
//!
//! Copies go through `copy_file_range`, so the kernel moves the bytes —
//! and on filesystems that can, shares them instead (a reflink on btrfs or
//! XFS is instant). Moves within a filesystem are renames; across
//! filesystems they are a copy, then the original is removed once its copy
//! is whole. Modes and modification times come along; links are copied as
//! links.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
};

use crate::trash;

/// What to do when a file being copied or moved meets one of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolve {
    /// Replace what's there (folders: merge into it).
    Replace,
    /// Leave both: the newcomer takes a free name, `photo 2.png`.
    KeepBoth,
    Skip,
    /// Stop everything.
    Cancel,
}

/// Where an operation has got to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub bytes: u64,
    pub total_bytes: u64,
    pub items: usize,
    pub total_items: usize,
    /// What's being worked on.
    pub current: PathBuf,
}

/// The window's side of an operation.
pub trait Watcher {
    fn progress(&mut self, progress: &Progress);
    /// `source` is going where `target` already is: what now? `all` asks
    /// for the same answer for the rest.
    fn conflict(&mut self, source: &Path, target: &Path) -> (Resolve, bool);
    fn stop(&self) -> bool;
}

/// What an operation did, for the window to report and for undo.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Each thing done: where it was and where it went (for a deletion,
    /// where it was, twice).
    pub done: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub stopped: bool,
}

/// Something done that can be undone.
#[derive(Clone, Debug)]
pub enum Done {
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Many renamed at once ([`rename_all`]), each pair `(from, to)`.
    RenamedAll(Vec<(PathBuf, PathBuf)>),
    /// Copies (or new folders and files) that undoing removes — to the
    /// trash, so even an undo can be undone by hand.
    Created(Vec<PathBuf>),
    Trashed(Vec<trash::Item>),
}

impl Done {
    /// In words, for "Undo …".
    pub fn describe(&self) -> String {
        let count = |n: usize, one: &str, many: &str| crate::format::count(n, one, many);
        match self {
            Self::Renamed { .. } => "rename".to_owned(),
            Self::Moved(pairs) => format!("move of {}", count(pairs.len(), "item", "items")),
            Self::RenamedAll(pairs) => {
                format!("renaming of {}", count(pairs.len(), "item", "items"))
            }
            Self::Created(paths) => count(paths.len(), "new item", "new items"),
            Self::Trashed(items) => format!("trashing of {}", count(items.len(), "item", "items")),
        }
    }
}

/// Undo `done`. Returns what would redo it.
pub fn undo(done: &Done) -> io::Result<Done> {
    match done {
        Done::Renamed { from, to } => {
            refuse_existing(from)?;
            fs::rename(to, from)?;
            Ok(Done::Renamed {
                from: to.clone(),
                to: from.clone(),
            })
        }
        Done::Moved(pairs) => {
            let mut back = Vec::new();
            for (from, to) in pairs.iter().rev() {
                refuse_existing(from)?;
                move_one(to, from)?;
                back.push((to.clone(), from.clone()));
            }
            back.reverse();
            Ok(Done::Moved(back))
        }
        Done::RenamedAll(pairs) => {
            let back: Vec<(PathBuf, PathBuf)> = pairs
                .iter()
                .map(|(from, to)| (to.clone(), from.clone()))
                .collect();
            rename_all(&back)
        }
        Done::Created(paths) => {
            let items = paths
                .iter()
                .map(|path| trash::trash(path))
                .collect::<io::Result<_>>()?;
            Ok(Done::Trashed(items))
        }
        Done::Trashed(items) => {
            let restored = items
                .iter()
                .map(trash::restore)
                .collect::<io::Result<_>>()?;
            Ok(Done::Created(restored))
        }
    }
}

/// Rename many at once: each `(from, to)` within a filesystem. A new name
/// may be one of the old ones — swapping two names, or shifting numbers
/// along — so when one is, everything goes through a temporary name first.
/// A new name taken by anything not being renamed is refused before
/// anything changes; a failure part way puts back what was done.
pub fn rename_all(pairs: &[(PathBuf, PathBuf)]) -> io::Result<Done> {
    let pairs: Vec<(PathBuf, PathBuf)> = pairs
        .iter()
        .filter(|(from, to)| from != to)
        .cloned()
        .collect();
    let sources: std::collections::HashSet<&Path> =
        pairs.iter().map(|(from, _)| from.as_path()).collect();
    let mut targets = std::collections::HashSet::new();
    for (_, to) in &pairs {
        if !targets.insert(to.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("two would be called {}", to.display()),
            ));
        }
        if !sources.contains(to.as_path()) {
            refuse_existing(to)?;
        }
    }
    let crossing = pairs.iter().any(|(_, to)| sources.contains(to.as_path()));
    // Each step taken, to undo if a later one fails.
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let step = |from: &Path, to: &Path, done: &mut Vec<(PathBuf, PathBuf)>| {
        fs::rename(from, to)?;
        done.push((from.to_owned(), to.to_owned()));
        io::Result::Ok(())
    };
    let result = (|| {
        if crossing {
            let mut parked = Vec::new();
            for (index, (from, to)) in pairs.iter().enumerate() {
                let dir = from.parent().unwrap_or(Path::new("/"));
                let tmp = dir.join(format!(".enso-renaming-{}-{index}", std::process::id()));
                step(from, &tmp, &mut done)?;
                parked.push((tmp, to));
            }
            for (tmp, to) in parked {
                step(&tmp, to, &mut done)?;
            }
        } else {
            for (from, to) in &pairs {
                step(from, to, &mut done)?;
            }
        }
        io::Result::Ok(())
    })();
    if let Err(err) = result {
        for (from, to) in done.iter().rev() {
            let _ = fs::rename(to, from);
        }
        return Err(err);
    }
    Ok(Done::RenamedAll(pairs))
}

fn refuse_existing(path: &Path) -> io::Result<()> {
    if path.exists() || path.is_symlink() {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("something is at {} now", path.display()),
        ))
    } else {
        Ok(())
    }
}

/// Why `name` can't be a file's name, if it can't.
pub fn check_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        Err("A name can't be empty.")
    } else if name == "." || name == ".." {
        Err("“.” and “..” are taken by every folder.")
    } else if name.contains('/') {
        Err("A name can't have “/” in it.")
    } else if name.contains('\0') {
        Err("A name can't have a NUL in it.")
    } else if name.len() > 255 {
        Err("That name is too long.")
    } else {
        Ok(())
    }
}

/// A name in `dir` like `name` that nothing has: `name` itself if free,
/// else `stem 2.ext`, `stem 3.ext`, ….
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let taken = |path: &Path| path.exists() || path.is_symlink();
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    // `photo 2.png` again becomes `photo 3.png`, not `photo 2 2.png`.
    let (stem, from) = match stem.rsplit_once(' ') {
        Some((base, n)) if n.parse::<usize>().is_ok_and(|n| n >= 2) => {
            (base, n.parse().unwrap_or(2) + 1)
        }
        _ => (stem, 2),
    };
    (from..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|path| !taken(path))
        .unwrap_or(first)
}

/// Rename `path` to `name`, in the same folder. Refuses to replace
/// anything.
pub fn rename(path: &Path, name: &str) -> io::Result<Done> {
    check_name(name).map_err(io::Error::other)?;
    let to = path.with_file_name(name);
    if to == path {
        return Ok(Done::Renamed {
            from: path.to_owned(),
            to,
        });
    }
    // Changing only the case on a case-insensitive filesystem is fine.
    let same_file = |a: &Path, b: &Path| matches!((fs::symlink_metadata(a), fs::symlink_metadata(b)), (Ok(a), Ok(b)) if a.ino() == b.ino() && a.dev() == b.dev());
    if (to.exists() || to.is_symlink()) && !same_file(path, &to) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("“{name}” is taken"),
        ));
    }
    fs::rename(path, &to)?;
    Ok(Done::Renamed {
        from: path.to_owned(),
        to,
    })
}

/// A new folder in `dir`, named `name` or the first free name like it.
pub fn new_folder(dir: &Path, name: &str) -> io::Result<PathBuf> {
    check_name(name).map_err(io::Error::other)?;
    let path = free_name(dir, name);
    fs::create_dir(&path)?;
    Ok(path)
}

/// A new empty file in `dir`, likewise.
pub fn new_file(dir: &Path, name: &str) -> io::Result<PathBuf> {
    check_name(name).map_err(io::Error::other)?;
    let path = free_name(dir, name);
    File::create_new(&path)?;
    Ok(path)
}

/// Trash each of `paths`. What can't be trashed is reported, not fatal.
pub fn trash_all(paths: &[PathBuf]) -> (Vec<trash::Item>, Vec<(PathBuf, String)>) {
    let mut trashed = Vec::new();
    let mut failed = Vec::new();
    for path in paths {
        match trash::trash(path) {
            Ok(item) => trashed.push(item),
            Err(err) => failed.push((path.clone(), err.to_string())),
        }
    }
    (trashed, failed)
}

/// Delete `paths` for good.
pub fn delete(paths: &[PathBuf], watcher: &mut dyn Watcher) -> Report {
    let mut report = Report::default();
    let mut progress = Progress {
        total_items: paths.len(),
        ..Progress::default()
    };
    for path in paths {
        if watcher.stop() {
            report.stopped = true;
            break;
        }
        progress.current = path.clone();
        watcher.progress(&progress);
        let removed = if path.is_dir() && !path.is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match removed {
            Ok(()) => report.done.push((path.clone(), path.clone())),
            Err(err) => report.failed.push((path.clone(), err.to_string())),
        }
        progress.items += 1;
    }
    watcher.progress(&progress);
    report
}

/// Copy `sources` into the folder `into`.
pub fn copy(sources: &[PathBuf], into: &Path, watcher: &mut dyn Watcher) -> Report {
    transfer(sources, into, false, watcher)
}

/// Move `sources` into the folder `into`.
pub fn move_into(sources: &[PathBuf], into: &Path, watcher: &mut dyn Watcher) -> Report {
    transfer(sources, into, true, watcher)
}

/// Duplicate `path` beside itself: `report.txt` → `report 2.txt`.
pub fn duplicate(path: &Path, watcher: &mut dyn Watcher) -> Report {
    let Some(dir) = path.parent() else {
        return Report::default();
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let target = free_name(dir, &name);
    let mut report = Report::default();
    let mut progress = Progress {
        total_bytes: size_of(path),
        total_items: 1,
        ..Progress::default()
    };
    match copy_tree(
        path,
        &target,
        &mut progress,
        watcher,
        &mut Settled::default(),
    ) {
        Ok(Some(())) => report.done.push((path.to_owned(), target)),
        Ok(None) => report.stopped = true,
        Err(err) => report.failed.push((path.to_owned(), err.to_string())),
    }
    report
}

/// Answers given "for all the rest".
#[derive(Default)]
struct Settled {
    resolve: Option<Resolve>,
}

impl Settled {
    fn ask(&mut self, watcher: &mut dyn Watcher, source: &Path, target: &Path) -> Resolve {
        if let Some(resolve) = self.resolve {
            return resolve;
        }
        let (resolve, all) = watcher.conflict(source, target);
        if all {
            self.resolve = Some(resolve);
        }
        resolve
    }
}

fn transfer(sources: &[PathBuf], into: &Path, moving: bool, watcher: &mut dyn Watcher) -> Report {
    let mut report = Report::default();
    let mut progress = Progress {
        total_bytes: sources.iter().map(|source| size_of(source)).sum(),
        total_items: sources.len(),
        ..Progress::default()
    };
    let mut settled = Settled::default();
    for source in sources {
        if watcher.stop() {
            report.stopped = true;
            break;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        let mut target = into.join(name);
        if into.starts_with(source) {
            report
                .failed
                .push((source.clone(), "a folder can't go inside itself".to_owned()));
            continue;
        }
        if &target == source {
            // Copying something onto itself: keep both; moving: nothing to do.
            if moving {
                report.skipped.push(source.clone());
                continue;
            }
            target = free_name(into, &name.to_string_lossy());
        } else if target.exists() || target.is_symlink() {
            match settled.ask(watcher, source, &target) {
                Resolve::Skip => {
                    report.skipped.push(source.clone());
                    continue;
                }
                Resolve::Cancel => {
                    report.stopped = true;
                    break;
                }
                Resolve::KeepBoth => target = free_name(into, &name.to_string_lossy()),
                Resolve::Replace => {
                    let merging = source.is_dir() && target.is_dir() && !target.is_symlink();
                    if !merging && let Err(err) = remove_any(&target) {
                        report.failed.push((source.clone(), err.to_string()));
                        continue;
                    }
                }
            }
        }
        progress.current = source.clone();
        watcher.progress(&progress);
        let result = if moving {
            match fs::rename(source, &target) {
                Ok(()) => {
                    progress.bytes += size_of(&target);
                    Ok(Some(()))
                }
                // Another filesystem: copy, then remove the original once
                // the copy is whole.
                Err(err) if err.raw_os_error() == Some(18) || target.is_dir() => {
                    match copy_tree(source, &target, &mut progress, watcher, &mut settled) {
                        Ok(Some(())) => remove_any(source).map(|()| Some(())),
                        other => other,
                    }
                }
                Err(err) => Err(err),
            }
        } else {
            copy_tree(source, &target, &mut progress, watcher, &mut settled)
        };
        match result {
            Ok(Some(())) => report.done.push((source.clone(), target)),
            Ok(None) => {
                report.stopped = true;
                break;
            }
            Err(err) => report.failed.push((source.clone(), err.to_string())),
        }
        progress.items += 1;
        watcher.progress(&progress);
    }
    report
}

/// Move one thing, across filesystems if it must.
fn move_one(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(err) if err.raw_os_error() == Some(18) => {
            struct Quiet;
            impl Watcher for Quiet {
                fn progress(&mut self, _: &Progress) {}
                fn conflict(&mut self, _: &Path, _: &Path) -> (Resolve, bool) {
                    (Resolve::Cancel, true)
                }
                fn stop(&self) -> bool {
                    false
                }
            }
            copy_tree(
                from,
                to,
                &mut Progress::default(),
                &mut Quiet,
                &mut Settled::default(),
            )?;
            remove_any(from)
        }
        other => other,
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// How many bytes copying `path` moves.
fn size_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| size_of(&entry.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

/// Copy `source` to `target`: a file, a link, or a folder and everything
/// in it (into an existing folder, merging). `Ok(None)` when stopped.
fn copy_tree(
    source: &Path,
    target: &Path,
    progress: &mut Progress,
    watcher: &mut dyn Watcher,
    settled: &mut Settled,
) -> io::Result<Option<()>> {
    if watcher.stop() {
        return Ok(None);
    }
    let meta = fs::symlink_metadata(source)?;
    if meta.is_symlink() {
        symlink(fs::read_link(source)?, target)?;
    } else if meta.is_dir() {
        if !target.is_dir() {
            fs::create_dir(target)?;
        }
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let to = target.join(entry.file_name());
            if (to.exists() || to.is_symlink()) && !(entry.path().is_dir() && to.is_dir()) {
                match settled.ask(watcher, &entry.path(), &to) {
                    Resolve::Skip => continue,
                    Resolve::Cancel => return Ok(None),
                    Resolve::KeepBoth => {
                        let free = free_name(target, &entry.file_name().to_string_lossy());
                        if copy_tree(&entry.path(), &free, progress, watcher, settled)?.is_none() {
                            return Ok(None);
                        }
                        continue;
                    }
                    Resolve::Replace => remove_any(&to)?,
                }
            }
            if copy_tree(&entry.path(), &to, progress, watcher, settled)?.is_none() {
                return Ok(None);
            }
        }
        fs::set_permissions(target, meta.permissions())?;
    } else if meta.is_file() {
        if copy_file(source, target, progress, watcher)?.is_none() {
            let _ = fs::remove_file(target);
            return Ok(None);
        }
    } else {
        return Err(io::Error::other(format!(
            "{} is a device, socket, or pipe, which can't be copied",
            source.display()
        )));
    }
    // Keep when it was last changed (folders and files; not links).
    if !meta.is_symlink()
        && let Ok(modified) = meta.modified()
        && let Ok(file) = File::open(target)
    {
        let _ = file.set_times(fs::FileTimes::new().set_modified(modified));
    }
    Ok(Some(()))
}

/// Bytes moved per step: big enough to be quick, small enough that
/// progress moves and stopping is prompt.
const CHUNK: usize = 8 << 20;

fn copy_file(
    source: &Path,
    target: &Path,
    progress: &mut Progress,
    watcher: &mut dyn Watcher,
) -> io::Result<Option<()>> {
    let mut from = File::open(source)?;
    let mut to = File::create_new(target)?;
    to.set_permissions(from.metadata()?.permissions())?;
    progress.current = source.to_owned();
    loop {
        if watcher.stop() {
            return Ok(None);
        }
        let copied = match rustix::fs::copy_file_range(&from, None, &to, None, CHUNK) {
            Ok(n) => n,
            // Not offered here (some filesystems, some kernels): by hand.
            Err(err)
                if err == rustix::io::Errno::XDEV
                    || err == rustix::io::Errno::NOSYS
                    || err == rustix::io::Errno::INVAL
                    || err == rustix::io::Errno::OPNOTSUPP =>
            {
                let mut buffer = vec![0; 1 << 20];
                let n = from.read(&mut buffer)?;
                to.write_all(&buffer[..n])?;
                n
            }
            Err(err) => return Err(err.into()),
        };
        if copied == 0 {
            break;
        }
        progress.bytes += copied as u64;
        watcher.progress(progress);
    }
    Ok(Some(()))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn many_renamed_at_once_even_swapped() {
        let root = std::env::temp_dir().join(format!("enso-fs-rename-all-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for (name, text) in [("a", "A"), ("b", "B"), ("c", "C"), ("keep", "K")] {
            fs::write(root.join(name), text).unwrap();
        }
        let at = |name: &str| root.join(name);
        // a and b swap; c becomes d.
        let done =
            rename_all(&[(at("a"), at("b")), (at("b"), at("a")), (at("c"), at("d"))]).unwrap();
        assert_eq!(fs::read_to_string(at("a")).unwrap(), "B");
        assert_eq!(fs::read_to_string(at("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(at("d")).unwrap(), "C");
        // Undone, all the way back, swap and all.
        undo(&done).unwrap();
        assert_eq!(fs::read_to_string(at("a")).unwrap(), "A");
        assert_eq!(fs::read_to_string(at("c")).unwrap(), "C");
        assert!(!at("d").exists());
        // A name already taken by something else: nothing changes.
        assert!(rename_all(&[(at("a"), at("x")), (at("c"), at("keep"))]).is_err());
        assert!(at("a").exists() && !at("x").exists());
        assert!(rename_all(&[(at("a"), at("x")), (at("b"), at("x"))]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    struct Answers {
        resolve: Resolve,
        all: bool,
        asked: usize,
        stop_after: Option<usize>,
        progressed: Cell<usize>,
    }

    impl Answers {
        fn new(resolve: Resolve) -> Self {
            Self {
                resolve,
                all: false,
                asked: 0,
                stop_after: None,
                progressed: Cell::new(0),
            }
        }
    }

    impl Watcher for Answers {
        fn progress(&mut self, _: &Progress) {
            self.progressed.set(self.progressed.get() + 1);
        }
        fn conflict(&mut self, _: &Path, _: &Path) -> (Resolve, bool) {
            self.asked += 1;
            (self.resolve, self.all)
        }
        fn stop(&self) -> bool {
            self.stop_after
                .is_some_and(|after| self.progressed.get() >= after)
        }
    }

    fn sandbox(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("enso-fs-ops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/src")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/notes.txt"), "notes").unwrap();
        fs::write(root.join("a/src/main.rs"), "fn main() {}").unwrap();
        symlink("notes.txt", root.join("a/link")).unwrap();
        root
    }

    #[test]
    fn copies_keep_contents_links_and_times() {
        let root = sandbox("copy");
        let mut watcher = Answers::new(Resolve::Skip);
        let report = copy(&[root.join("a")], &root.join("b"), &mut watcher);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(
            fs::read_to_string(root.join("b/a/src/main.rs")).unwrap(),
            "fn main() {}"
        );
        assert_eq!(
            fs::read_link(root.join("b/a/link")).unwrap(),
            Path::new("notes.txt")
        );
        let (old, new) = (
            fs::metadata(root.join("a/notes.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            fs::metadata(root.join("b/a/notes.txt"))
                .unwrap()
                .modified()
                .unwrap(),
        );
        assert_eq!(old, new);
        assert!(root.join("a/notes.txt").exists(), "the original stays");
        assert!(watcher.progressed.get() > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copying_onto_itself_keeps_both() {
        let root = sandbox("self");
        let mut watcher = Answers::new(Resolve::Cancel);
        let report = copy(&[root.join("a/notes.txt")], &root.join("a"), &mut watcher);
        assert_eq!(report.done[0].1, root.join("a/notes 2.txt"));
        assert_eq!(watcher.asked, 0);
        let report = copy(&[root.join("a")], &root.join("a/src"), &mut watcher);
        assert_eq!(report.failed.len(), 1, "not into itself");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn taken_names_are_resolved_as_asked() {
        let root = sandbox("conflict");
        fs::write(root.join("b/notes.txt"), "theirs").unwrap();
        let source = [root.join("a/notes.txt")];

        let report = copy(&source, &root.join("b"), &mut Answers::new(Resolve::Skip));
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            fs::read_to_string(root.join("b/notes.txt")).unwrap(),
            "theirs"
        );

        copy(
            &source,
            &root.join("b"),
            &mut Answers::new(Resolve::KeepBoth),
        );
        assert_eq!(
            fs::read_to_string(root.join("b/notes 2.txt")).unwrap(),
            "notes"
        );

        copy(
            &source,
            &root.join("b"),
            &mut Answers::new(Resolve::Replace),
        );
        assert_eq!(
            fs::read_to_string(root.join("b/notes.txt")).unwrap(),
            "notes"
        );

        let report = copy(&source, &root.join("b"), &mut Answers::new(Resolve::Cancel));
        assert!(report.stopped);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacing_a_folder_merges_and_asks_inside() {
        let root = sandbox("merge");
        fs::create_dir_all(root.join("b/a/src")).unwrap();
        fs::write(root.join("b/a/keep.txt"), "mine").unwrap();
        fs::write(root.join("b/a/src/main.rs"), "old").unwrap();
        let mut watcher = Answers::new(Resolve::Replace);
        watcher.all = true;
        copy(&[root.join("a")], &root.join("b"), &mut watcher);
        assert_eq!(
            fs::read_to_string(root.join("b/a/keep.txt")).unwrap(),
            "mine"
        );
        assert_eq!(
            fs::read_to_string(root.join("b/a/src/main.rs")).unwrap(),
            "fn main() {}"
        );
        assert_eq!(watcher.asked, 1, "asked once, for all");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moves_rename_and_undo_puts_back() {
        let root = sandbox("move");
        let report = move_into(
            &[root.join("a/notes.txt")],
            &root.join("b"),
            &mut Answers::new(Resolve::Skip),
        );
        assert!(root.join("b/notes.txt").exists());
        assert!(!root.join("a/notes.txt").exists());
        let done = Done::Moved(report.done);
        let redo = undo(&done).unwrap();
        assert!(root.join("a/notes.txt").exists());
        assert!(!root.join("b/notes.txt").exists());
        undo(&redo).unwrap();
        assert!(root.join("b/notes.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renames_check_their_names() {
        let root = sandbox("rename");
        assert!(rename(&root.join("a/notes.txt"), "a/b").is_err());
        assert!(rename(&root.join("a/notes.txt"), "").is_err());
        assert!(rename(&root.join("a/notes.txt"), "src").is_err(), "taken");
        let done = rename(&root.join("a/notes.txt"), "Notes.md").unwrap();
        assert!(root.join("a/Notes.md").exists());
        undo(&done).unwrap();
        assert!(root.join("a/notes.txt").exists());
        assert_eq!(
            check_name(".."),
            Err("“.” and “..” are taken by every folder.")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn free_names_count_up() {
        let root = sandbox("names");
        assert_eq!(
            free_name(&root.join("a"), "new.txt"),
            root.join("a/new.txt")
        );
        assert_eq!(
            free_name(&root.join("a"), "notes.txt"),
            root.join("a/notes 2.txt")
        );
        fs::write(root.join("a/notes 2.txt"), "").unwrap();
        assert_eq!(
            free_name(&root.join("a"), "notes.txt"),
            root.join("a/notes 3.txt")
        );
        assert_eq!(
            free_name(&root.join("a"), "notes 2.txt"),
            root.join("a/notes 3.txt")
        );
        assert_eq!(free_name(&root.join("a"), "src"), root.join("a/src 2"));
        let folder = new_folder(&root.join("a"), "src").unwrap();
        assert_eq!(folder, root.join("a/src 2"));
        assert!(folder.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stopping_leaves_no_half_files() {
        let root = sandbox("stop");
        fs::write(root.join("a/big"), vec![7; 3 * CHUNK]).unwrap();
        let mut watcher = Answers::new(Resolve::Skip);
        watcher.stop_after = Some(2);
        let report = copy(&[root.join("a/big")], &root.join("b"), &mut watcher);
        assert!(report.stopped);
        assert!(!root.join("b/big").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_and_duplicating() {
        let root = sandbox("delete");
        let report = duplicate(&root.join("a/src"), &mut Answers::new(Resolve::Skip));
        assert_eq!(report.done[0].1, root.join("a/src 2"));
        assert!(root.join("a/src 2/main.rs").exists());
        let report = delete(
            &[root.join("a/src 2"), root.join("a/link")],
            &mut Answers::new(Resolve::Skip),
        );
        assert_eq!(report.done.len(), 2);
        assert!(!root.join("a/src 2").exists());
        assert!(root.join("a/notes.txt").exists(), "a link's target stays");
        fs::remove_dir_all(root).unwrap();
    }
}
