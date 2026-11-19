//! What enso-notify adds to a notification itself: buttons that act here,
//! without the sender.
//!
//! - **A one-time code** in a message — "Your code is 482 913" — gets a
//!   button that copies it, so nobody types six digits off a toast.
//! - **A picture file** sent by a program that offers nothing to do with it
//!   — niri's "Screenshot captured" — opens when the toast is clicked, and
//!   gets *Open* and *Show in Files*.
//!
//! These keys start with `enso:`, so they are never mistaken for a
//! sender's own: invoking one does its work here, and the sender hears only
//! that its notification was dismissed.

use std::path::{Path, PathBuf};

use crate::stack::{Action, Notification};

/// The prefix every local action's key carries.
pub const PREFIX: &str = "enso:";

/// What a local action does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Local {
    /// Put this on the clipboard.
    Copy(String),
    /// Open the file with what the desktop opens its type with.
    Open(PathBuf),
    /// Show the file in the file manager.
    Show(PathBuf),
    /// Put the machine to sleep.
    Sleep,
}

impl Local {
    pub fn key(&self) -> String {
        match self {
            Self::Copy(text) => format!("{PREFIX}copy:{text}"),
            Self::Open(path) => format!("{PREFIX}open:{}", path.display()),
            Self::Show(path) => format!("{PREFIX}show:{}", path.display()),
            Self::Sleep => format!("{PREFIX}sleep"),
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        let rest = key.strip_prefix(PREFIX)?;
        let (verb, what) = rest.split_once(':').unwrap_or((rest, ""));
        match verb {
            "copy" if !what.is_empty() => Some(Self::Copy(what.to_owned())),
            "open" if !what.is_empty() => Some(Self::Open(PathBuf::from(what))),
            "show" if !what.is_empty() => Some(Self::Show(PathBuf::from(what))),
            "sleep" => Some(Self::Sleep),
            _ => None,
        }
    }

    fn action(&self, label: impl Into<String>) -> Action {
        Action {
            key: self.key(),
            label: label.into(),
        }
    }
}

/// Add what enso can do with `note`: a button for a one-time code, and
/// for a picture file (`picture`, from the `image-path` hint) when the
/// sender offers nothing itself. `home` is where a picture must be to
/// count — a sender's cached avatar isn't something to open.
pub fn enrich(note: &mut Notification, picture: Option<&Path>, home: &Path) {
    if let Some(code) = one_time_code(&note.summary, &note.body) {
        note.actions
            .push(Local::Copy(code.clone()).action(format!("Copy {code}")));
    }
    let Some(picture) = picture else {
        return;
    };
    let ours = picture.starts_with(home)
        && !picture.starts_with(home.join(".cache"))
        && !picture
            .components()
            .any(|part| part.as_os_str() == ".local");
    if ours
        && note
            .actions
            .iter()
            .all(|action| action.key.starts_with(PREFIX))
        && picture.is_file()
    {
        note.actions
            .push(Local::Open(picture.to_owned()).action("Open"));
        note.actions
            .push(Local::Show(picture.to_owned()).action("Show in Files"));
    }
}

/// What clicking the toast itself does, when the sender offers nothing:
/// open the picture, if enso added that.
pub fn default_of(note: &Notification) -> Option<String> {
    note.actions
        .iter()
        .find(|action| matches!(Local::parse(&action.key), Some(Local::Open(_))))
        .map(|action| action.key.clone())
}

/// Words that say a number is a code to type somewhere.
const CODE_WORDS: [&str; 10] = [
    "code",
    "otp",
    "one-time",
    "one time",
    "passcode",
    "verification",
    "verify",
    "pin",
    "2fa",
    "security",
];

/// A one-time code in a message that says it's one: four to eight digits,
/// perhaps split in two by a space or a dash ("482 913"), alone as a word.
/// The digits only, as they'd be typed.
pub fn one_time_code(summary: &str, body: &str) -> Option<String> {
    let text = format!("{summary}\n{body}");
    let lower = text.to_lowercase();
    if !CODE_WORDS.iter().any(|word| lower.contains(word)) {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let starts_word = i == 0 || !chars[i - 1].is_alphanumeric();
        if !(starts_word && chars[i].is_ascii_digit()) {
            i += 1;
            continue;
        }
        // Digits, with at most one space or dash between two groups.
        let mut digits = String::new();
        let mut j = i;
        let mut splits = 0;
        while j < chars.len() {
            let c = chars[j];
            if c.is_ascii_digit() {
                digits.push(c);
                j += 1;
            } else if (c == ' ' || c == '-')
                && splits == 0
                && !digits.is_empty()
                && chars.get(j + 1).is_some_and(char::is_ascii_digit)
            {
                splits += 1;
                j += 1;
            } else {
                break;
            }
        }
        let ends_word = chars.get(j).is_none_or(|c| !c.is_alphanumeric());
        // Not a time, an amount, or a year: those come with : . , / or are
        // too short or long to be a code.
        let follows = chars.get(j).copied();
        let looks_like_more = matches!(follows, Some(':' | '.' | ',' | '/'))
            && chars.get(j + 1).is_some_and(char::is_ascii_digit);
        if ends_word && !looks_like_more && (4..=8).contains(&digits.len()) {
            return Some(digits);
        }
        i = j.max(i + 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_codes_only_where_a_code_is_meant() {
        let code = |summary: &str, body: &str| one_time_code(summary, body);
        assert_eq!(
            code("GitHub", "Your verification code is 482913."),
            Some("482913".into())
        );
        assert_eq!(
            code("Bank", "OTP: 482 913 — don't share it"),
            Some("482913".into())
        );
        assert_eq!(code("Login code", "1234-5678"), Some("12345678".into()));
        assert_eq!(
            code("Signal", "G-482913 is your code"),
            Some("482913".into())
        );
        // Numbers that aren't codes, or with nothing calling them one.
        assert_eq!(code("Mira", "Tea at 1600? Room 4021"), None);
        assert_eq!(code("Code review", "Merged at 14:30"), None);
        assert_eq!(code("Your code", "costs 1,250.00 now"), None);
        assert_eq!(code("PIN", "123"), None);
        assert_eq!(code("Code", "call 0123456789012"), None);
    }

    #[test]
    fn keys_round_trip_and_never_pass_for_a_senders() {
        for local in [
            Local::Copy("482913".into()),
            Local::Open("/home/kai/Pictures/a b.png".into()),
            Local::Show("/home/kai/x".into()),
            Local::Sleep,
        ] {
            assert_eq!(Local::parse(&local.key()), Some(local));
        }
        assert_eq!(Local::parse("default"), None);
        assert_eq!(Local::parse("enso:copy:"), None);
    }

    #[test]
    fn a_picture_of_ones_own_opens_and_a_cached_one_does_not() {
        let home = std::env::temp_dir().join(format!("enso-notify-local-{}", std::process::id()));
        let shots = home.join("Pictures");
        std::fs::create_dir_all(&shots).unwrap();
        let shot = shots.join("shot.png");
        std::fs::write(&shot, b"png").unwrap();

        let mut note = Notification::default();
        enrich(&mut note, Some(&shot), &home);
        let labels: Vec<&str> = note.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Open", "Show in Files"]);
        assert_eq!(default_of(&note), Some(Local::Open(shot.clone()).key()));

        // A sender with actions of its own keeps its toast to itself.
        let mut busy = Notification {
            actions: vec![Action {
                key: "reply".into(),
                label: "Reply".into(),
            }],
            ..Notification::default()
        };
        enrich(&mut busy, Some(&shot), &home);
        assert_eq!(busy.actions.len(), 1);

        // An avatar in a cache is not for opening.
        let cache = home.join(".cache/app");
        std::fs::create_dir_all(&cache).unwrap();
        let avatar = cache.join("avatar.png");
        std::fs::write(&avatar, b"png").unwrap();
        let mut chat = Notification::default();
        enrich(&mut chat, Some(&avatar), &home);
        assert!(chat.actions.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }
}
