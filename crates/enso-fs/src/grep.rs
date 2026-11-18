//! Looking inside files: what to look for, and the first line that has
//! it — for enso-files' search inside files and the launcher's.
//!
//! The words as typed are found as they are, ignoring case unless they
//! have capitals; `/like this/` is a regular expression. Binary files and
//! very large ones are passed over.

use std::{fs, io::Read, path::Path};

use regex::{Regex, RegexBuilder};

/// Files read at most this big.
const BIGGEST: u64 = 8 * 1024 * 1024;
/// A NUL in this much of the start makes a file binary.
const SNIFF: usize = 8192;
/// A matching line kept at most this long, in characters.
const LINE: usize = 200;

/// What to look for, from what was typed.
pub fn matcher(text: &str) -> Result<Regex, String> {
    let (pattern, regex) = match text
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
    {
        Some(inner) if !inner.is_empty() => (inner.to_owned(), true),
        _ => (regex::escape(text), false),
    };
    let caseless = !regex && !text.chars().any(char::is_uppercase);
    RegexBuilder::new(&pattern)
        .case_insensitive(caseless)
        .size_limit(1 << 20)
        .build()
        .map_err(|err| format!("That pattern doesn't read: {err}"))
}

/// The first line of the file at `path` that `matcher` finds, trimmed
/// (and cut short if long); nothing if none does or it isn't text.
pub fn first_match(path: &Path, matcher: &Regex) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > BIGGEST {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(BIGGEST).read_to_end(&mut bytes).ok()?;
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let line = text.lines().find(|line| matcher.is_match(line))?;
    let line = line.trim();
    Some(match line.char_indices().nth(LINE) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_as_typed_or_a_pattern() {
        assert!(matcher("sky").unwrap().is_match("The Sky"));
        assert!(!matcher("Sky").unwrap().is_match("the sky"));
        assert!(matcher("a.b").unwrap().is_match("a.b"));
        assert!(!matcher("a.b").unwrap().is_match("axb"));
        assert!(matcher("/a.b/").unwrap().is_match("axb"));
        assert!(matcher("/fn (/").is_err());
    }

    #[test]
    fn the_first_line_of_text_files_only() {
        let dir = std::env::temp_dir().join(format!("enso-fs-grep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("notes.md"),
            "intro\n   a note about the sky   \nsky again\n",
        )
        .unwrap();
        fs::write(dir.join("blob.bin"), b"sky\0\0").unwrap();
        let sky = matcher("sky").unwrap();
        assert_eq!(
            first_match(&dir.join("notes.md"), &sky).as_deref(),
            Some("a note about the sky")
        );
        assert_eq!(first_match(&dir.join("blob.bin"), &sky), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
