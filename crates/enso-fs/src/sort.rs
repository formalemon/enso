//! Putting a listing in order, the way people count.
//!
//! Names compare *naturally*: case doesn't matter, runs of digits compare
//! as numbers (`track 2` before `track 10`), and a leading dot doesn't
//! decide anything (`.config` sorts beside `config`). Ties fall back to the
//! exact bytes, so the order is total and stable from one run to the next.
//!
//! Comparing two names this way has to fold their case, which costs more
//! than the comparison itself; [`Keys`] folds every name once per listing,
//! and sorting then only shuffles indices.

use std::cmp::Ordering;

use crate::list::{Entry, Listing, Meta};

/// What to order by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum By {
    #[default]
    Name,
    Size,
    Modified,
    Created,
    /// The extension, then the name.
    Extension,
}

impl By {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Size,
        Self::Modified,
        Self::Created,
        Self::Extension,
    ];

    /// The word for it in configuration files.
    pub fn name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Created => "created",
            Self::Extension => "extension",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|by| by.name() == name)
    }
}

/// A complete ordering: what to order by, which way, and whether folders
/// come first whatever the rest says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sort {
    pub by: By,
    pub descending: bool,
    pub folders_first: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            by: By::Name,
            descending: false,
            folders_first: true,
        }
    }
}

/// Every name of a listing folded for comparing: lower case, without a
/// leading dot. One buffer, like the listing's own.
#[derive(Clone, Debug, Default)]
pub struct Keys {
    folded: Vec<u8>,
    /// End of each entry's folded name in `folded`; it starts where the
    /// previous one ends.
    ends: Vec<u32>,
}

impl Keys {
    pub fn new(listing: &Listing) -> Self {
        let mut keys = Self {
            folded: Vec::with_capacity(listing.len() * 16),
            ends: Vec::with_capacity(listing.len()),
        };
        for index in 0..listing.len() {
            fold(listing.name(index), &mut keys.folded);
            keys.ends
                .push(u32::try_from(keys.folded.len()).unwrap_or(u32::MAX));
        }
        keys
    }

    pub fn get(&self, index: usize) -> &[u8] {
        let start = index
            .checked_sub(1)
            .map_or(0, |previous| self.ends[previous] as usize);
        &self.folded[start..self.ends[index] as usize]
    }
}

/// Append `name` folded for comparison to `out`.
fn fold(name: &[u8], out: &mut Vec<u8>) {
    let name = name.strip_prefix(b".").unwrap_or(name);
    match std::str::from_utf8(name) {
        // Most names are ASCII; folding those needs no allocation.
        Ok(text) if text.is_ascii() => out.extend(text.bytes().map(|b| b.to_ascii_lowercase())),
        Ok(text) => {
            for c in text.chars().flat_map(char::to_lowercase) {
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        Err(_) => out.extend(name.iter().map(u8::to_ascii_lowercase)),
    }
}

/// Compare two names naturally: digit runs by value, everything else byte
/// by byte. Both should already be folded ([`Keys`]).
pub fn natural(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (a_end, b_end) = (digits_end(a, i), digits_end(b, j));
            let (a_num, b_num) = (trim_zeros(&a[i..a_end]), trim_zeros(&b[j..b_end]));
            // A longer number without its leading zeros is a bigger one;
            // equal lengths compare digit by digit.
            let order = a_num.len().cmp(&b_num.len()).then_with(|| a_num.cmp(b_num));
            if order != Ordering::Equal {
                return order;
            }
            // `7` before `07`: fewer zeros first, so equal values still order.
            let zeros = (a_end - i).cmp(&(b_end - j));
            if zeros != Ordering::Equal {
                return zeros;
            }
            (i, j) = (a_end, b_end);
        } else {
            match a[i].cmp(&b[j]) {
                Ordering::Equal => (i, j) = (i + 1, j + 1),
                order => return order,
            }
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn digits_end(text: &[u8], from: usize) -> usize {
    text[from..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(text.len(), |n| from + n)
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&d| d == b'0').count();
    // Keep one zero of an all-zero run, so `0` is still a number.
    &digits[zeros.min(digits.len().saturating_sub(1))..]
}

/// The extension a name sorts by: after the last dot, not counting a
/// leading one (`.bashrc` has none). Folded, like the name.
fn extension(folded: &[u8]) -> &[u8] {
    match folded.iter().rposition(|&b| b == b'.') {
        Some(dot) if dot > 0 => &folded[dot + 1..],
        _ => b"",
    }
}

/// The order to show `listing` in: indices into it.
pub fn order(listing: &Listing, keys: &Keys, sort: Sort) -> Vec<u32> {
    let mut indices: Vec<u32> = (0..listing.len() as u32).collect();
    indices.sort_unstable_by(|&a, &b| compare(listing, keys, sort, a as usize, b as usize));
    indices
}

/// Compare entries `a` and `b` of `listing` under `sort`.
pub fn compare(listing: &Listing, keys: &Keys, sort: Sort, a: usize, b: usize) -> Ordering {
    let (entry_a, entry_b) = (listing.entry(a), listing.entry(b));
    if sort.folders_first {
        let folders = entry_b.kind.is_dir().cmp(&entry_a.kind.is_dir());
        if folders != Ordering::Equal {
            return folders;
        }
    }
    let by_name =
        || natural(keys.get(a), keys.get(b)).then_with(|| listing.name(a).cmp(listing.name(b)));
    let order = match sort.by {
        By::Name => by_name(),
        By::Extension => extension(keys.get(a))
            .cmp(extension(keys.get(b)))
            .then_with(by_name),
        By::Size => by_meta(entry_a, entry_b, |meta| meta.size).then_with(by_name),
        By::Modified => by_meta(entry_a, entry_b, |meta| meta.modified).then_with(by_name),
        By::Created => by_meta(entry_a, entry_b, |meta| meta.created).then_with(by_name),
    };
    if sort.descending {
        order.reverse()
    } else {
        order
    }
}

/// Order by something only the second pass knows. Folders all compare
/// equal by size (theirs means nothing to a reader), so they stay in name
/// order among themselves; entries not yet read go last.
fn by_meta<T: Ord>(a: &Entry, b: &Entry, key: impl Fn(&Meta) -> T) -> Ordering {
    let value = |entry: &Entry| {
        let size_of_folder = entry.kind.is_dir();
        entry.meta.as_ref().map(|meta| (size_of_folder, key(meta)))
    };
    match (value(a), value(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::Kind;

    fn listing(names: &[&str]) -> Listing {
        Listing::from_entries(
            "/",
            names.iter().map(|name| {
                let kind = if name.ends_with('/') {
                    Kind::Dir
                } else {
                    Kind::File
                };
                (name.trim_end_matches('/'), kind)
            }),
        )
    }

    fn sorted(names: &[&str], sort: Sort) -> Vec<String> {
        let listing = listing(names);
        let keys = Keys::new(&listing);
        order(&listing, &keys, sort)
            .into_iter()
            .map(|i| listing.display_name(i as usize).into_owned())
            .collect()
    }

    #[test]
    fn names_count_like_people_do() {
        assert_eq!(
            sorted(
                &[
                    "track 10.ogg",
                    "Track 2.ogg",
                    "track 1.ogg",
                    "track 02.ogg",
                    "track 0.ogg"
                ],
                Sort::default()
            ),
            [
                "track 0.ogg",
                "track 1.ogg",
                "Track 2.ogg",
                "track 02.ogg",
                "track 10.ogg"
            ]
        );
        assert_eq!(
            sorted(&["b", "A", "a", "B", "Émile", "eve"], Sort::default()),
            ["A", "a", "B", "b", "eve", "Émile"]
        );
        // Numbers longer than any integer type still compare.
        assert_eq!(
            sorted(
                &["x99999999999999999999999", "x100000000000000000000000"],
                Sort::default()
            ),
            ["x99999999999999999999999", "x100000000000000000000000"]
        );
    }

    #[test]
    fn dotfiles_sort_beside_their_namesakes() {
        assert_eq!(
            sorted(
                &["config", "zsh", ".config", "apple", ".bashrc"],
                Sort::default()
            ),
            ["apple", ".bashrc", ".config", "config", "zsh"]
        );
    }

    #[test]
    fn folders_come_first_unless_told_otherwise() {
        let names = ["b.txt", "c/", "a/", "a.txt"];
        assert_eq!(
            sorted(&names, Sort::default()),
            ["a", "c", "a.txt", "b.txt"]
        );
        let mixed = Sort {
            folders_first: false,
            ..Sort::default()
        };
        assert_eq!(sorted(&names, mixed), ["a", "a.txt", "b.txt", "c"]);
        let descending = Sort {
            descending: true,
            ..Sort::default()
        };
        // Descending reverses within the groups, not the groups.
        assert_eq!(sorted(&names, descending), ["c", "a", "b.txt", "a.txt"]);
    }

    #[test]
    fn by_extension_then_name() {
        let sort = Sort {
            by: By::Extension,
            ..Sort::default()
        };
        assert_eq!(
            sorted(&["b.rs", "a.toml", "README", "a.rs", ".bashrc"], sort),
            [".bashrc", "README", "a.rs", "b.rs", "a.toml"]
        );
    }

    #[test]
    fn by_size_waits_for_the_second_pass() {
        let mut listing = listing(&["big", "small", "unread", "folder/"]);
        let meta = |size| Meta {
            size,
            modified: 0,
            created: None,
            accessed: 0,
            mode: 0o644,
            uid: 0,
            gid: 0,
            link_size: None,
        };
        listing.apply(0, &[Some(meta(900)), Some(meta(3)), None, Some(meta(4096))]);
        let keys = Keys::new(&listing);
        let sort = Sort {
            by: By::Size,
            folders_first: false,
            ..Sort::default()
        };
        let names: Vec<String> = order(&listing, &keys, sort)
            .into_iter()
            .map(|i| listing.display_name(i as usize).into_owned())
            .collect();
        // Files by size, then the folder (its size means nothing), then
        // whatever hasn't been read yet.
        assert_eq!(names, ["small", "big", "folder", "unread"]);
    }

    #[test]
    fn sort_names_round_trip() {
        for by in By::ALL {
            assert_eq!(By::from_name(by.name()), Some(by));
        }
        assert_eq!(By::from_name("colour"), None);
    }
}
