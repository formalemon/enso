//! One view onto one folder: where it is, what's in it, in what order, and
//! where the cursor is — with no drawing, so every rule here is tested
//! without a window.
//!
//! A pane never reads the disk itself. [`Pane::navigate`] says what to load
//! and hands back a generation number; the listing arrives later through
//! [`Pane::set_listing`] and its metadata through [`Pane::apply_stats`],
//! and anything carrying an older generation — a folder already left — is
//! ignored.

use std::{
    collections::HashMap,
    fmt, io,
    path::{Path, PathBuf},
    rc::Rc,
};

use enso_fs::{
    fuzzy::Pattern,
    list::{Entry, Listing, Meta},
    sort::{self, By, Keys, Sort},
};

/// Whether the folder is there to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    Loading,
    Ready,
    /// Why it can't be shown, in words for the person looking.
    Failed(String),
}

/// What opening the entry under the cursor means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// Go into it.
    Dir(PathBuf),
    /// Hand it to its application.
    File(PathBuf),
}

/// Which entries a pane shows at all, beyond dotfiles and the filter: a
/// picker's file types, say. Given a name and whether it's a folder.
#[derive(Clone)]
pub struct Keep(pub Rc<KeepFn>);

/// Whether to show an entry, given its name and whether it's a folder.
pub type KeepFn = dyn Fn(&str, bool) -> bool;

impl Keep {
    pub fn new(keep: impl Fn(&str, bool) -> bool + 'static) -> Self {
        Self(Rc::new(keep))
    }
}

impl fmt::Debug for Keep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Keep")
    }
}

/// A folder to load: the request a navigation makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub path: PathBuf,
    pub generation: u64,
}

#[derive(Debug)]
pub struct Pane {
    location: PathBuf,
    listing: Listing,
    keys: Keys,
    /// Entries shown, in order: indices into `listing`.
    shown: Vec<u32>,
    /// Position in `shown`.
    cursor: usize,
    sort: Sort,
    hidden: bool,
    /// What the folder is being filtered by, as typed and as prepared.
    filter: String,
    pattern: Pattern,
    keep: Option<Keep>,
    /// How many entries would show without the filter.
    unfiltered: usize,
    load: Load,
    generation: u64,
    /// How many entries' metadata has arrived for this generation.
    statted: usize,
    /// The name to put the cursor on when the listing arrives.
    want: Option<Vec<u8>>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// Where the cursor was in each folder left, to put it back on return.
    remembered: HashMap<PathBuf, Vec<u8>>,
}

impl Pane {
    /// A pane about to show `location`; load it with the returned request.
    pub fn new(location: PathBuf, sort: Sort, hidden: bool) -> (Self, Request) {
        let mut pane = Self {
            location: PathBuf::new(),
            listing: Listing::default(),
            keys: Keys::default(),
            shown: Vec::new(),
            cursor: 0,
            sort,
            hidden,
            filter: String::new(),
            pattern: Pattern::default(),
            keep: None,
            unfiltered: 0,
            load: Load::Loading,
            generation: 0,
            statted: 0,
            want: None,
            back: Vec::new(),
            forward: Vec::new(),
            remembered: HashMap::new(),
        };
        let request = pane.go(location);
        (pane, request)
    }

    pub fn location(&self) -> &Path {
        &self.location
    }

    pub fn load(&self) -> &Load {
        &self.load
    }

    /// How many entries are shown.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// How many dotfiles are being left out.
    pub fn hidden_count(&self) -> usize {
        if self.hidden {
            0
        } else {
            (0..self.listing.len())
                .filter(|&index| self.listing.is_hidden(index))
                .count()
        }
    }

    /// How many entries there are before the filter.
    pub fn unfiltered_len(&self) -> usize {
        self.unfiltered
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The `row`th entry shown.
    pub fn entry(&self, row: usize) -> &Entry {
        self.listing.entry(self.index(row))
    }

    pub fn name(&self, row: usize) -> std::borrow::Cow<'_, str> {
        self.listing.display_name(self.index(row))
    }

    pub fn is_hidden(&self, row: usize) -> bool {
        self.listing.is_hidden(self.index(row))
    }

    pub fn path(&self, row: usize) -> PathBuf {
        self.listing.path(self.index(row))
    }

    fn index(&self, row: usize) -> usize {
        self.shown[row] as usize
    }

    /// Where the cursor is, if there is anything to be on.
    pub fn cursor(&self) -> Option<usize> {
        (!self.shown.is_empty()).then_some(self.cursor)
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    // Navigation.

    /// Go to `path`, remembering where we were for Back.
    pub fn navigate(&mut self, path: PathBuf) -> Option<Request> {
        if path == self.location {
            return None;
        }
        let from = self.location.clone();
        let request = self.go(path);
        if !from.as_os_str().is_empty() {
            self.back.push(from);
        }
        self.forward.clear();
        Some(request)
    }

    /// Go to `path` with the cursor on the entry called `name`.
    pub fn navigate_to(&mut self, path: PathBuf, name: &[u8]) -> Option<Request> {
        let request = if path == self.location {
            self.reload()
        } else {
            self.navigate(path)?
        };
        self.want = Some(name.to_vec());
        Some(request)
    }

    /// Go up a folder, with the cursor on the one we came out of.
    pub fn parent(&mut self) -> Option<Request> {
        let parent = self.location.parent()?.to_owned();
        let child = self.location.file_name()?.as_encoded_bytes().to_vec();
        let request = self.navigate(parent)?;
        self.want = Some(child);
        Some(request)
    }

    pub fn back(&mut self) -> Option<Request> {
        let path = self.back.pop()?;
        let from = self.location.clone();
        let request = self.go(path);
        self.forward.push(from);
        Some(request)
    }

    pub fn forward(&mut self) -> Option<Request> {
        let path = self.forward.pop()?;
        let from = self.location.clone();
        let request = self.go(path);
        self.back.push(from);
        Some(request)
    }

    /// Read the same folder again, keeping the cursor where it is.
    pub fn reload(&mut self) -> Request {
        self.want = self.cursor_name();
        self.generation += 1;
        self.statted = 0;
        Request {
            path: self.location.clone(),
            generation: self.generation,
        }
    }

    /// Leave the current folder for `path`, with no history.
    fn go(&mut self, path: PathBuf) -> Request {
        if let Some(name) = self.cursor_name() {
            self.remembered.insert(self.location.clone(), name);
        }
        self.want = self.remembered.get(&path).cloned();
        self.location = path;
        self.filter.clear();
        self.pattern = Pattern::default();
        self.listing = Listing::default();
        self.keys = Keys::default();
        self.shown.clear();
        self.cursor = 0;
        self.load = Load::Loading;
        self.generation += 1;
        self.statted = 0;
        Request {
            path: self.location.clone(),
            generation: self.generation,
        }
    }

    // Arrivals.

    /// The folder's entries have been read (or couldn't be). Returns
    /// whether they were for this pane's current folder.
    pub fn set_listing(&mut self, generation: u64, result: Result<Listing, String>) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Ok(listing) => {
                // A reload keeps its cursor; a new folder takes `want`.
                let want = self.want.take().or_else(|| self.cursor_name());
                self.keys = Keys::new(&listing);
                self.listing = listing;
                self.load = Load::Ready;
                self.refresh(want);
            }
            Err(why) => {
                self.listing = Listing::default();
                self.keys = Keys::default();
                self.shown.clear();
                self.load = Load::Failed(why);
            }
        }
        true
    }

    /// Metadata for entries `start..` of the listing has arrived. Returns
    /// whether it was for this folder.
    ///
    /// An order that depends on metadata is put right once, when the last
    /// of it is in: re-sorting a large folder on every batch would cost
    /// more than it shows.
    pub fn apply_stats(&mut self, generation: u64, start: usize, metas: &[Option<Meta>]) -> bool {
        if generation != self.generation || start + metas.len() > self.listing.len() {
            return false;
        }
        self.listing.apply(start, metas);
        self.statted += metas.len();
        if self.statted >= self.listing.len()
            && self.sort.by != By::Name
            && self.sort.by != By::Extension
        {
            self.refresh(self.cursor_name());
        }
        true
    }

    /// Whether every entry's metadata has arrived.
    #[cfg(test)]
    pub fn is_statted(&self) -> bool {
        self.statted >= self.listing.len()
    }

    // Order and filtering.

    /// Show only what fuzzily matches `filter`, in the folder's order,
    /// with the cursor on the best match. An empty filter shows everything,
    /// the cursor staying where it was.
    pub fn set_filter(&mut self, filter: &str) {
        if filter == self.filter {
            return;
        }
        let keep = self.cursor_name();
        self.filter = filter.to_owned();
        self.pattern = Pattern::new(filter);
        self.refresh(keep);
        if !filter.is_empty() {
            let best = (0..self.shown.len()).max_by_key(|&row| {
                // Earlier rows win ties: `max_by_key` keeps the last of equals.
                let score = self.pattern.score(&self.name(row)).unwrap_or(i32::MIN);
                (score, std::cmp::Reverse(row))
            });
            self.cursor = best.unwrap_or(0);
        }
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn set_sort(&mut self, sort: Sort) {
        if sort != self.sort {
            self.sort = sort;
            self.refresh(self.cursor_name());
        }
    }

    /// Show only what `keep` keeps (and everything, with `None`).
    pub fn set_keep(&mut self, keep: Option<Keep>) {
        self.keep = keep;
        self.refresh(self.cursor_name());
    }

    pub fn shows_hidden(&self) -> bool {
        self.hidden
    }

    pub fn toggle_hidden(&mut self) {
        self.hidden = !self.hidden;
        self.refresh(self.cursor_name());
    }

    /// Work out what is shown and in what order, with the cursor on `keep`
    /// if it is still there — or, if it was hidden away, as near to where
    /// it was as can be.
    fn refresh(&mut self, keep: Option<Vec<u8>>) {
        let previous = self.cursor;
        let mut shown = sort::order(&self.listing, &self.keys, self.sort);
        if !self.hidden {
            shown.retain(|&index| !self.listing.is_hidden(index as usize));
        }
        if let Some(Keep(keep)) = &self.keep {
            shown.retain(|&index| {
                let index = index as usize;
                keep(
                    &self.listing.display_name(index),
                    self.listing.entry(index).kind.is_dir(),
                )
            });
        }
        self.unfiltered = shown.len();
        if !self.filter.is_empty() {
            shown.retain(|&index| {
                self.pattern
                    .score(&self.listing.display_name(index as usize))
                    .is_some()
            });
        }
        self.shown = shown;
        self.cursor = keep
            .and_then(|name| {
                self.shown
                    .iter()
                    .position(|&index| self.listing.name(index as usize) == name)
            })
            .unwrap_or_else(|| previous.min(self.shown.len().saturating_sub(1)));
    }

    // The cursor.

    fn cursor_name(&self) -> Option<Vec<u8>> {
        let row = self.cursor()?;
        Some(self.listing.name(self.index(row)).to_vec())
    }

    /// Put the cursor on `row` (clamped). Returns whether it moved.
    pub fn set_cursor(&mut self, row: usize) -> bool {
        let row = row.min(self.shown.len().saturating_sub(1));
        let moved = row != self.cursor;
        self.cursor = row;
        moved
    }

    /// Move the cursor by `delta` rows, stopping at the ends.
    pub fn move_cursor(&mut self, delta: isize) -> bool {
        let row = self.cursor.saturating_add_signed(delta);
        self.set_cursor(row)
    }

    /// What Enter on the cursor does.
    pub fn open(&self) -> Option<Open> {
        let row = self.cursor()?;
        let path = self.path(row);
        Some(if self.entry(row).kind.is_dir() {
            Open::Dir(path)
        } else {
            Open::File(path)
        })
    }
}

/// A listing failure in words for the person looking at the folder.
pub fn describe(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::PermissionDenied => {
            "You don't have permission to see what's in here.".into()
        }
        io::ErrorKind::NotFound => "This folder doesn't exist (any more).".into(),
        io::ErrorKind::NotADirectory => "This isn't a folder.".into(),
        _ => format!("This folder can't be read: {err}."),
    }
}

#[cfg(test)]
mod tests {
    use enso_fs::list::Kind;

    use super::*;

    fn listing(dir: &str, names: &[&str]) -> Listing {
        Listing::from_entries(
            dir,
            names.iter().map(|name| match name.strip_suffix('/') {
                Some(name) => (name, Kind::Dir),
                None => (*name, Kind::File),
            }),
        )
    }

    fn names(pane: &Pane) -> Vec<String> {
        (0..pane.len())
            .map(|row| pane.name(row).into_owned())
            .collect()
    }

    fn cursor_name(pane: &Pane) -> String {
        pane.name(pane.cursor().unwrap()).into_owned()
    }

    fn arrive(pane: &mut Pane, request: &Request, names: &[&str]) {
        let path = request.path.to_str().unwrap();
        assert!(pane.set_listing(request.generation, Ok(listing(path, names))));
    }

    fn pane_at(path: &str, names: &[&str]) -> Pane {
        let (mut pane, request) = Pane::new(path.into(), Sort::default(), false);
        arrive(&mut pane, &request, names);
        pane
    }

    #[test]
    fn a_folder_shows_sorted_without_dotfiles() {
        let pane = pane_at("/home/me", &["b.txt", ".bashrc", "a/", "c/", "a.txt"]);
        assert_eq!(names(&pane), ["a", "c", "a.txt", "b.txt"]);
        assert_eq!(pane.hidden_count(), 1);
        assert_eq!(pane.cursor(), Some(0));
        assert_eq!(pane.open(), Some(Open::Dir("/home/me/a".into())));
    }

    #[test]
    fn showing_dotfiles_keeps_the_cursor_on_its_file() {
        let mut pane = pane_at("/h", &["b", ".a", "c"]);
        pane.set_cursor(1);
        assert_eq!(cursor_name(&pane), "c");
        pane.toggle_hidden();
        assert_eq!(names(&pane), [".a", "b", "c"]);
        assert_eq!(cursor_name(&pane), "c");
        // Hiding the file under the cursor leaves it near where it was.
        pane.set_cursor(0);
        pane.toggle_hidden();
        assert_eq!(cursor_name(&pane), "b");
    }

    #[test]
    fn listings_for_folders_already_left_are_ignored() {
        let (mut pane, first) = Pane::new("/a".into(), Sort::default(), false);
        let second = pane.navigate("/b".into()).unwrap();
        assert!(!pane.set_listing(first.generation, Ok(listing("/a", &["stale"]))));
        assert_eq!(pane.load(), &Load::Loading);
        arrive(&mut pane, &second, &["fresh"]);
        assert_eq!(names(&pane), ["fresh"]);
        assert!(!pane.apply_stats(first.generation, 0, &[None]));
    }

    #[test]
    fn going_up_lands_on_the_folder_we_came_from() {
        let mut pane = pane_at("/home/me/dev", &["x"]);
        let up = pane.parent().unwrap();
        assert_eq!(up.path, Path::new("/home/me"));
        arrive(&mut pane, &up, &["Documents/", "dev/", "music/"]);
        assert_eq!(cursor_name(&pane), "dev");
    }

    #[test]
    fn back_and_forward_return_the_cursor_too() {
        let mut pane = pane_at("/one", &["a", "b", "c"]);
        pane.set_cursor(2);
        let two = pane.navigate("/two".into()).unwrap();
        arrive(&mut pane, &two, &["x", "y"]);
        pane.set_cursor(1);

        let back = pane.back().unwrap();
        assert_eq!(back.path, Path::new("/one"));
        arrive(&mut pane, &back, &["a", "b", "c"]);
        assert_eq!(cursor_name(&pane), "c");
        assert!(pane.can_go_forward());

        let forward = pane.forward().unwrap();
        arrive(&mut pane, &forward, &["x", "y"]);
        assert_eq!(cursor_name(&pane), "y");
        assert!(!pane.can_go_forward());

        // A new place drops the forward history.
        pane.back().unwrap();
        pane.navigate("/three".into()).unwrap();
        assert!(!pane.can_go_forward());
        // Going nowhere isn't a step.
        assert_eq!(pane.navigate("/three".into()), None);
    }

    #[test]
    fn a_reload_keeps_the_cursor_on_its_file() {
        let mut pane = pane_at("/h", &["a", "b", "c"]);
        pane.set_cursor(1);
        let reload = pane.reload();
        arrive(&mut pane, &reload, &["0", "a", "b", "c"]);
        assert_eq!(cursor_name(&pane), "b");
    }

    #[test]
    fn an_unreadable_folder_says_why() {
        let (mut pane, request) = Pane::new("/root".into(), Sort::default(), false);
        let why = describe(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(pane.set_listing(request.generation, Err(why.clone())));
        assert_eq!(pane.load(), &Load::Failed(why));
        assert!(pane.is_empty());
        assert_eq!(pane.cursor(), None);
        assert_eq!(pane.open(), None);
    }

    #[test]
    fn an_order_by_size_settles_when_the_last_size_arrives() {
        let (mut pane, request) = Pane::new(
            "/h".into(),
            Sort {
                by: By::Size,
                folders_first: false,
                descending: false,
            },
            false,
        );
        arrive(&mut pane, &request, &["big", "small", "mid"]);
        // Sizes unknown: the listing's own order.
        pane.set_cursor(2);
        let meta = |size| {
            Some(Meta {
                size,
                modified: 0,
                created: None,
                accessed: 0,
                mode: 0o644,
                uid: 0,
                gid: 0,
                link_size: None,
            })
        };
        let at_cursor = cursor_name(&pane);
        assert!(pane.apply_stats(request.generation, 0, &[meta(900), meta(1)]));
        assert!(!pane.is_statted());
        assert_eq!(cursor_name(&pane), at_cursor);
        assert!(pane.apply_stats(request.generation, 2, &[meta(50)]));
        assert!(pane.is_statted());
        assert_eq!(names(&pane), ["small", "mid", "big"]);
        // Still on the same file, wherever it went.
        assert_eq!(cursor_name(&pane), at_cursor);
    }

    #[test]
    fn filtering_keeps_the_order_and_finds_the_best_match() {
        let mut pane = pane_at(
            "/h",
            &[
                "notes.txt",
                "Cargo.toml",
                "cargo-notes.md",
                "src/",
                ".cargo/",
            ],
        );
        pane.set_filter("crgo");
        assert_eq!(names(&pane), ["cargo-notes.md", "Cargo.toml"]);
        assert_eq!(cursor_name(&pane), "Cargo.toml");
        pane.set_filter("notes");
        assert_eq!(names(&pane), ["cargo-notes.md", "notes.txt"]);
        // The name that starts with the words wins.
        assert_eq!(cursor_name(&pane), "notes.txt");
        assert_eq!(pane.unfiltered_len(), 4);
        pane.set_filter("zzz");
        assert!(pane.is_empty());
        // Clearing it shows everything, the cursor where it was.
        pane.set_filter("notes");
        pane.set_filter("");
        assert_eq!(pane.len(), 4);
        assert_eq!(cursor_name(&pane), "notes.txt");
        // A new folder starts unfiltered.
        pane.set_filter("src");
        pane.navigate("/elsewhere".into());
        assert_eq!(pane.filter(), "");
    }

    #[test]
    fn the_cursor_stops_at_the_ends() {
        let mut pane = pane_at("/h", &["a", "b", "c"]);
        assert!(!pane.move_cursor(-1));
        assert!(pane.move_cursor(5));
        assert_eq!(cursor_name(&pane), "c");
        assert!(pane.move_cursor(-1));
        assert_eq!(cursor_name(&pane), "b");
    }
}
