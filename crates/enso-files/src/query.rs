//! Query words: narrowing what's shown by more than its name, in the filter
//! (`/`) and in search (`Ctrl+F`) alike.
//!
//! - `type:image` (or `video`, `audio`, `archive`, `code`, `document`,
//!   `program`, `folder`) — what kind of file;
//! - `ext:rs` — its extension;
//! - `size:>10M`, `size:<500k` — bigger or smaller than;
//! - `modified:<7d`, `modified:>1y` — changed less or more than that long
//!   ago (`h`, `d`, `w`, `m` months, `y`);
//! - `name:report` — a word for the name, even one that looks like these.
//!
//! Everything else is the name, matched as before.

use std::time::Duration;

use enso_fs::list::Meta;

use crate::kind::Kind;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Word {
    Kind(Kind),
    Ext(String),
    /// Bigger (`true`) or smaller than this many bytes.
    Size(bool, u64),
    /// Changed more (`true`) or less long ago than this.
    Age(bool, Duration),
}

/// A query: the name part, and the words that narrow it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// What the name is matched against.
    pub text: String,
    words: Vec<Word>,
}

impl Query {
    pub fn parse(query: &str) -> Self {
        let mut text = Vec::new();
        let mut words = Vec::new();
        for part in query.split_whitespace() {
            match part
                .split_once(':')
                .and_then(|(key, value)| word(key, value))
            {
                Some(Ok(word)) => words.push(word),
                Some(Err(name)) => text.push(name),
                None => text.push(part.to_owned()),
            }
        }
        Self {
            text: text.join(" "),
            words,
        }
    }

    /// Whether there are words beyond the name.
    pub fn narrows(&self) -> bool {
        !self.words.is_empty()
    }

    /// Whether answering takes metadata (size, time).
    pub fn needs_meta(&self) -> bool {
        self.words
            .iter()
            .any(|word| matches!(word, Word::Size(..) | Word::Age(..)))
    }

    /// Whether an entry passes the words (the name is matched elsewhere).
    /// Without metadata, a size or age can't pass. `now` is in
    /// nanoseconds since 1970, as [`Meta`]'s times are.
    pub fn admits(&self, name: &str, is_dir: bool, meta: Option<&Meta>, now: i64) -> bool {
        let executable = meta.is_some_and(|meta| meta.mode & 0o111 != 0);
        let kind = Kind::of(name, is_dir, executable);
        self.words.iter().all(|word| match word {
            Word::Kind(wanted) => kind == *wanted,
            Word::Ext(ext) => name
                .rsplit_once('.')
                .is_some_and(|(stem, found)| !stem.is_empty() && found.eq_ignore_ascii_case(ext)),
            Word::Size(bigger, bytes) => {
                !is_dir
                    && meta
                        .is_some_and(|meta| (meta.size > *bytes) == *bigger && meta.size != *bytes)
            }
            Word::Age(older, age) => meta.is_some_and(|meta| {
                let since = u128::try_from(now - meta.modified).unwrap_or(0);
                (since > age.as_nanos()) == *older
            }),
        })
    }
}

/// `key:value` as a word; `Err` for `name:` (its value is name text);
/// `None` if it isn't a word at all.
fn word(key: &str, value: &str) -> Option<Result<Word, String>> {
    Some(Ok(match key.to_ascii_lowercase().as_str() {
        "name" => return Some(Err(value.to_owned())),
        "type" | "kind" => Word::Kind(match value.to_ascii_lowercase().as_str() {
            "image" | "picture" | "photo" => Kind::Image,
            "video" | "film" => Kind::Video,
            "audio" | "sound" | "music" => Kind::Audio,
            "archive" => Kind::Archive,
            "code" => Kind::Code,
            "document" | "doc" | "text" => Kind::Document,
            "program" => Kind::Program,
            "folder" | "dir" => Kind::Folder,
            _ => return None,
        }),
        "ext" => {
            let ext = value.trim_start_matches('.');
            if ext.is_empty() {
                return None;
            }
            Word::Ext(ext.to_owned())
        }
        "size" => {
            let (bigger, amount) = compare(value)?;
            Word::Size(bigger, bytes(amount)?)
        }
        "modified" | "changed" | "age" => {
            let (older, amount) = compare(value)?;
            Word::Age(older, age(amount)?)
        }
        _ => return None,
    }))
}

/// `>…` or `<…`: more (`true`) or less, and the amount.
fn compare(value: &str) -> Option<(bool, &str)> {
    match value.as_bytes().first() {
        Some(b'>') => Some((true, &value[1..])),
        Some(b'<') => Some((false, &value[1..])),
        _ => None,
    }
}

/// `10M`, `500k`, `2G`, `1024`.
fn bytes(amount: &str) -> Option<u64> {
    let amount = amount.trim().to_ascii_lowercase();
    let amount = amount.trim_end_matches('b');
    let (number, unit) = match amount.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((at, _)) => amount.split_at(at),
        None => (amount, ""),
    };
    let number: f64 = number.parse().ok()?;
    let unit: f64 = match unit {
        "" => 1.0,
        "k" => 1e3,
        "m" => 1e6,
        "g" => 1e9,
        "t" => 1e12,
        _ => return None,
    };
    (number >= 0.0).then_some((number * unit) as u64)
}

/// `7d`, `12h`, `2w`, `3m` (months), `1y`.
fn age(amount: &str) -> Option<Duration> {
    let amount = amount.trim().to_ascii_lowercase();
    let unit = amount.chars().last()?;
    let number: f64 = amount[..amount.len() - unit.len_utf8()].parse().ok()?;
    let hour = 3600.0;
    let seconds = number
        * match unit {
            'h' => hour,
            'd' => 24.0 * hour,
            'w' => 7.0 * 24.0 * hour,
            'm' => 30.0 * 24.0 * hour,
            'y' => 365.0 * 24.0 * hour,
            _ => return None,
        };
    (seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(size: u64, days_old: i64, now: i64) -> Meta {
        Meta {
            size,
            modified: now - days_old * 86_400 * 1_000_000_000,
            created: None,
            accessed: now,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            link_size: None,
        }
    }

    #[test]
    fn words_come_out_of_the_name() {
        let query = Query::parse("holiday type:image size:>1M modified:<30d");
        assert_eq!(query.text, "holiday");
        assert!(query.narrows() && query.needs_meta());
        let now = 1_800_000_000 * 1_000_000_000;
        assert!(query.admits("beach.jpg", false, Some(&meta(3_000_000, 2, now)), now));
        assert!(
            !query.admits("beach.jpg", false, Some(&meta(3_000, 2, now)), now),
            "too small"
        );
        assert!(
            !query.admits("beach.jpg", false, Some(&meta(3_000_000, 90, now)), now),
            "too old"
        );
        assert!(
            !query.admits("notes.md", false, Some(&meta(3_000_000, 2, now)), now),
            "not a picture"
        );
        assert!(
            !query.admits("beach.jpg", false, None, now),
            "unknown size can't pass"
        );

        let query = Query::parse("ext:.rs name:type:odd main");
        assert_eq!(query.text, "type:odd main");
        assert!(query.admits("main.rs", false, None, now));
        assert!(!query.admits("main.rsx", false, None, now));
        assert!(
            !query.admits(".rs", false, None, now),
            "a dotfile has no extension"
        );
        // Not words: left as name.
        assert_eq!(
            Query::parse("type:nonsense 10:30").text,
            "type:nonsense 10:30"
        );
        assert_eq!(bytes("1.5G"), Some(1_500_000_000));
        assert_eq!(age("2w"), Some(Duration::from_secs(14 * 86_400)));
    }
}
