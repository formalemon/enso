//! A directory's entries, read in two passes.
//!
//! The first pass ([`Listing::read`]) asks the kernel for names and kinds
//! only — `getdents64`, which hands over whole buffers of entries per call —
//! so a folder of fifty thousand files is ready to draw in milliseconds.
//! Symbolic links are the exception: each is followed once, so a link to a
//! folder sorts and opens as a folder from the first frame. Links are rare,
//! and this keeps "is it a folder" settled before anything is shown.
//!
//! The second pass ([`stat`]) fills sizes, dates, and permissions with
//! `statx` on a pool of threads, handing results back in batches as they
//! finish, and stops early when asked to — the folder may have been left
//! before it is done.
//!
//! Names are kept as bytes, all in one buffer: one allocation per folder
//! rather than one per entry, and names that are not UTF-8 are listed, not
//! lost.

use std::{
    borrow::Cow,
    ffi::OsStr,
    io,
    ops::Range,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
};

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, StatxFlags, open, statx};

/// What a directory entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Dir,
    File,
    /// A symbolic link, and what it leads to.
    Link(Target),
    /// A device, socket, or pipe.
    Special,
}

/// Where a symbolic link leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Dir,
    File,
    Special,
    /// Nowhere: the target does not exist, or cannot be reached.
    Broken,
}

impl Kind {
    /// A folder, or a link to one: something to open by going into it.
    pub fn is_dir(self) -> bool {
        matches!(self, Self::Dir | Self::Link(Target::Dir))
    }

    pub fn is_link(self) -> bool {
        matches!(self, Self::Link(_))
    }
}

/// Nanoseconds since the Unix epoch.
pub type Time = i64;

/// What the second pass learns about an entry. For a link, everything but
/// `link_size` is the target's, as file managers show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Size in bytes (for a folder, what the filesystem reports for it).
    pub size: u64,
    pub modified: Time,
    /// When the file was made, where the filesystem records it.
    pub created: Option<Time>,
    pub accessed: Time,
    /// Permission bits (`0o7777`), without the file type.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// The link itself's size: the length of the path it holds.
    pub link_size: Option<u64>,
}

/// One entry: where its name is, what it is, and — once the second pass
/// gets to it — its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    start: u32,
    len: u16,
    pub kind: Kind,
    pub meta: Option<Meta>,
}

impl Entry {
    fn range(&self) -> Range<usize> {
        let start = self.start as usize;
        start..start + usize::from(self.len)
    }
}

/// A directory's entries, in the order the filesystem gave them (sort with
/// [`crate::sort`]). `.` and `..` are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    dir: PathBuf,
    names: Vec<u8>,
    entries: Vec<Entry>,
}

impl Listing {
    /// The first pass: every entry's name and kind.
    pub fn read(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let fd = open(
            dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut listing = Self {
            dir: dir.to_owned(),
            names: Vec::with_capacity(4096),
            entries: Vec::with_capacity(64),
        };
        for entry in Dir::read_from(&fd)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let kind = match entry.file_type() {
                FileType::Directory => Kind::Dir,
                FileType::RegularFile => Kind::File,
                FileType::Symlink => Kind::Link(follow(&fd, name)),
                // Some filesystems leave the type to a stat.
                FileType::Unknown => match statx(
                    &fd,
                    OsStr::from_bytes(name),
                    AtFlags::SYMLINK_NOFOLLOW,
                    StatxFlags::TYPE,
                ) {
                    Ok(stat) => match FileType::from_raw_mode(u32::from(stat.stx_mode)) {
                        FileType::Directory => Kind::Dir,
                        FileType::RegularFile => Kind::File,
                        FileType::Symlink => Kind::Link(follow(&fd, name)),
                        _ => Kind::Special,
                    },
                    // Gone already, or unreadable: listed, as a plain file.
                    Err(_) => Kind::File,
                },
                _ => Kind::Special,
            };
            listing.push(name, kind);
        }
        Ok(listing)
    }

    /// A listing of the given names and kinds, for tests and for entries
    /// that do not come from a directory (search results, the basket).
    pub fn from_entries<'a>(
        dir: impl AsRef<Path>,
        entries: impl IntoIterator<Item = (&'a str, Kind)>,
    ) -> Self {
        let mut listing = Self {
            dir: dir.as_ref().to_owned(),
            ..Self::default()
        };
        for (name, kind) in entries {
            listing.push(name.as_bytes(), kind);
        }
        listing
    }

    fn push(&mut self, name: &[u8], kind: Kind) {
        // Linux names are at most 255 bytes, and a buffer of every name in
        // one folder stays well inside 4 GiB.
        let (Ok(start), Ok(len)) = (u32::try_from(self.names.len()), u16::try_from(name.len()))
        else {
            return;
        };
        self.names.extend_from_slice(name);
        self.entries.push(Entry {
            start,
            len,
            kind,
            meta: None,
        });
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, index: usize) -> &Entry {
        &self.entries[index]
    }

    /// The entry's name, as the filesystem has it.
    pub fn name(&self, index: usize) -> &[u8] {
        &self.names[self.entries[index].range()]
    }

    /// The entry's name for showing: UTF-8 as it is, anything else with
    /// replacement marks where the bytes don't decode.
    pub fn display_name(&self, index: usize) -> Cow<'_, str> {
        String::from_utf8_lossy(self.name(index))
    }

    pub fn path(&self, index: usize) -> PathBuf {
        self.dir.join(OsStr::from_bytes(self.name(index)))
    }

    /// A dotfile, hidden unless asked for.
    pub fn is_hidden(&self, index: usize) -> bool {
        self.name(index).first() == Some(&b'.')
    }

    /// Where the entry is, by name, if it is listed.
    pub fn find(&self, name: &[u8]) -> Option<usize> {
        (0..self.len()).find(|&index| self.name(index) == name)
    }

    /// Take a batch of the second pass's results: `metas[i]` belongs to
    /// entry `start + i`. Entries that could not be read stay without.
    pub fn apply(&mut self, start: usize, metas: &[Option<Meta>]) {
        for (entry, meta) in self.entries[start..].iter_mut().zip(metas) {
            if meta.is_some() {
                entry.meta = *meta;
            }
        }
    }

    /// Whether every entry that can have metadata has it.
    pub fn is_complete(&self) -> bool {
        self.entries.iter().all(|entry| entry.meta.is_some())
    }
}

/// Where the link `name` in `dir` leads.
fn follow(dir: &impl rustix::fd::AsFd, name: &[u8]) -> Target {
    match statx(
        dir,
        OsStr::from_bytes(name),
        AtFlags::empty(),
        StatxFlags::TYPE,
    ) {
        Ok(stat) => match FileType::from_raw_mode(u32::from(stat.stx_mode)) {
            FileType::Directory => Target::Dir,
            FileType::RegularFile => Target::File,
            _ => Target::Special,
        },
        Err(_) => Target::Broken,
    }
}

/// How many entries a thread takes at a time: big enough that handing out
/// work costs nothing, small enough that the first batch is back quickly.
const BATCH: usize = 256;

/// The second pass: fill in every entry's metadata, on up to `threads`
/// threads. Each finished batch goes to `done(start, metas)` — from
/// whichever thread finished it, in no particular order — ready for
/// [`Listing::apply`]. Returns early, with work undone, once `cancel` is set.
pub fn stat(
    listing: &Listing,
    threads: usize,
    cancel: &AtomicBool,
    done: impl Fn(usize, Vec<Option<Meta>>) + Sync,
) -> io::Result<()> {
    let fd = open(
        listing.dir(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let next = AtomicUsize::new(0);
    let threads = threads.clamp(1, listing.len().div_ceil(BATCH).max(1));
    let work = || {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let start = next.fetch_add(BATCH, Ordering::Relaxed);
            if start >= listing.len() {
                return;
            }
            let end = (start + BATCH).min(listing.len());
            let metas = (start..end)
                .map(|index| meta_of(&fd, listing.name(index), listing.entry(index).kind))
                .collect();
            done(start, metas);
        }
    };
    if threads == 1 {
        work();
    } else {
        thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(work);
            }
        });
    }
    Ok(())
}

/// What `stat` asks for: everything shown, nothing more.
const WANTED: StatxFlags = StatxFlags::TYPE
    .union(StatxFlags::MODE)
    .union(StatxFlags::UID)
    .union(StatxFlags::GID)
    .union(StatxFlags::SIZE)
    .union(StatxFlags::MTIME)
    .union(StatxFlags::ATIME)
    .union(StatxFlags::BTIME);

fn meta_of(dir: &impl rustix::fd::AsFd, name: &[u8], kind: Kind) -> Option<Meta> {
    let name = OsStr::from_bytes(name);
    // Cached attributes are good enough to show, and on a network
    // filesystem far quicker than asking the server.
    let flags = AtFlags::STATX_DONT_SYNC;
    if let Kind::Link(target) = kind {
        let link = statx(dir, name, flags | AtFlags::SYMLINK_NOFOLLOW, WANTED).ok()?;
        let mut meta = match target {
            Target::Broken => convert(&link),
            _ => statx(dir, name, flags, WANTED)
                .map(|stat| convert(&stat))
                .unwrap_or_else(|_| convert(&link)),
        };
        meta.link_size = Some(link.stx_size);
        Some(meta)
    } else {
        statx(dir, name, flags | AtFlags::SYMLINK_NOFOLLOW, WANTED)
            .ok()
            .map(|stat| convert(&stat))
    }
}

fn convert(stat: &rustix::fs::Statx) -> Meta {
    let time = |t: rustix::fs::StatxTimestamp| t.tv_sec * 1_000_000_000 + i64::from(t.tv_nsec);
    let has = |flag: StatxFlags| StatxFlags::from_bits_retain(stat.stx_mask).contains(flag);
    Meta {
        size: stat.stx_size,
        modified: time(stat.stx_mtime),
        created: has(StatxFlags::BTIME).then(|| time(stat.stx_btime)),
        accessed: time(stat.stx_atime),
        mode: u32::from(stat.stx_mode) & 0o7777,
        uid: stat.stx_uid,
        gid: stat.stx_gid,
        link_size: None,
    }
}

/// A sensible number of threads for the second pass on this machine: most
/// of the cores, leaving some for drawing and everything else.
pub fn threads() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 8))
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink, sync::Mutex};

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("enso-fs-list-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sorted_names(listing: &Listing) -> Vec<String> {
        let mut names: Vec<String> = (0..listing.len())
            .map(|i| listing.display_name(i).into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_first_pass_knows_names_and_kinds() {
        let root = temp("kinds");
        fs::create_dir(root.join("folder")).unwrap();
        fs::write(root.join("file.txt"), "hello").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        symlink("folder", root.join("to-folder")).unwrap();
        symlink("file.txt", root.join("to-file")).unwrap();
        symlink("nowhere", root.join("broken")).unwrap();

        let listing = Listing::read(&root).unwrap();
        assert_eq!(
            sorted_names(&listing),
            [
                ".hidden",
                "broken",
                "file.txt",
                "folder",
                "to-file",
                "to-folder"
            ]
        );
        let kind = |name: &str| listing.entry(listing.find(name.as_bytes()).unwrap()).kind;
        assert_eq!(kind("folder"), Kind::Dir);
        assert_eq!(kind("file.txt"), Kind::File);
        assert_eq!(kind("to-folder"), Kind::Link(Target::Dir));
        assert!(kind("to-folder").is_dir());
        assert_eq!(kind("to-file"), Kind::Link(Target::File));
        assert_eq!(kind("broken"), Kind::Link(Target::Broken));
        assert!(listing.is_hidden(listing.find(b".hidden").unwrap()));
        assert!(!listing.is_hidden(listing.find(b"folder").unwrap()));
        assert_eq!(
            listing.path(listing.find(b"file.txt").unwrap()),
            root.join("file.txt")
        );
        // Nothing is known about sizes until the second pass.
        assert!(listing.entries().iter().all(|entry| entry.meta.is_none()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_that_are_not_utf8_are_kept() {
        let root = temp("bytes");
        let name = OsStr::from_bytes(b"caf\xe9.txt");
        fs::write(root.join(name), "").unwrap();
        let listing = Listing::read(&root).unwrap();
        assert_eq!(listing.name(0), b"caf\xe9.txt");
        assert_eq!(listing.display_name(0), "caf\u{fffd}.txt");
        assert!(listing.path(0).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_second_pass_fills_metadata_in_batches() {
        let root = temp("stat");
        for i in 0..(BATCH * 3 + 7) {
            fs::write(root.join(format!("f{i}")), vec![0; i % 10]).unwrap();
        }
        symlink("f9", root.join("link")).unwrap();
        let mut listing = Listing::read(&root).unwrap();
        let batches = Mutex::new(Vec::new());
        stat(&listing, 4, &AtomicBool::new(false), |start, metas| {
            batches.lock().unwrap().push((start, metas));
        })
        .unwrap();
        for (start, metas) in batches.into_inner().unwrap() {
            listing.apply(start, &metas);
        }
        assert!(listing.is_complete());
        let meta = |name: &str| {
            listing
                .entry(listing.find(name.as_bytes()).unwrap())
                .meta
                .unwrap()
        };
        assert_eq!(meta("f9").size, 9);
        assert_eq!(meta("f3").size, 3);
        assert_eq!(meta("f3").mode & 0o600, 0o600);
        // A link shows its target's size, and remembers its own.
        assert_eq!(meta("link").size, 9);
        assert_eq!(meta("link").link_size, Some(2));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_second_pass_stops_when_cancelled() {
        let root = temp("cancel");
        for i in 0..BATCH * 4 {
            fs::write(root.join(format!("f{i}")), "").unwrap();
        }
        let listing = Listing::read(&root).unwrap();
        let cancel = AtomicBool::new(false);
        let batches = AtomicUsize::new(0);
        stat(&listing, 1, &cancel, |_, _| {
            batches.fetch_add(1, Ordering::Relaxed);
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(batches.load(Ordering::Relaxed), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        assert!(Listing::read("/nonexistent/enso-fs").is_err());
    }
}
