//! Asking for the login password: the card is this program again, as
//! `enso-keyring prompt`, its own short-lived process. Lines go between:
//!
//! - to the card: `mode <create|unlock|old|changed>`, `asker <name>`, then
//!   after each try `ok` (done: it closes) or `again <mode> <words, hex>`;
//! - from the card: `password <hex>`, or `cancel`.
//!
//! The password is hex on the way, so nothing in it can end a line, and
//! wiped on both sides once used.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

use crate::crypto::{hex, unhex};

/// What the card asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The first time: the login password, to make the keyring with.
    Create,
    Unlock,
    /// The login password changed while locked: the one before it.
    OldPassword,
    /// The login password changed while unlocked: the new one.
    Changed,
}

impl Mode {
    pub fn word(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Unlock => "unlock",
            Self::OldPassword => "old",
            Self::Changed => "changed",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "create" => Self::Create,
            "unlock" => Self::Unlock,
            "old" => Self::OldPassword,
            "changed" => Self::Changed,
            _ => return None,
        })
    }
}

/// What was typed, by what it was asked for.
#[derive(Debug)]
pub enum Asked {
    Password(Zeroizing<Vec<u8>>),
    Old(Zeroizing<Vec<u8>>),
    New(Zeroizing<Vec<u8>>),
}

/// Bring the card up in `mode` for `asker`, and hand each try to `answer`
/// until it says yes or the card is put away. Returns whether it said yes.
pub fn ask(
    mode: Mode,
    asker: &str,
    mut answer: impl FnMut(Asked) -> Result<(), (String, Mode)>,
) -> bool {
    let Ok(mut child) = Command::new(crate::myself())
        .arg("prompt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return false;
    };
    let (Some(mut to), Some(from)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return false;
    };
    let mut from = BufReader::new(from);
    let asker = asker.replace('\n', " ");
    if writeln!(to, "mode {}\nasker {asker}", mode.word()).is_err() {
        let _ = child.kill();
        return false;
    }
    let mut mode = mode;
    let mut done = false;
    loop {
        let mut line = Zeroizing::new(String::new());
        if from.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Some(typed) = line.trim_end().strip_prefix("password ").and_then(unhex) else {
            break;
        };
        let typed = Zeroizing::new(typed);
        let asked = match mode {
            Mode::Create | Mode::Unlock => Asked::Password(typed),
            Mode::OldPassword => Asked::Old(typed),
            Mode::Changed => Asked::New(typed),
        };
        match answer(asked) {
            Ok(()) => {
                let _ = writeln!(to, "ok");
                done = true;
                break;
            }
            Err((words, next)) => {
                mode = next;
                if writeln!(to, "again {} {}", next.word(), hex(words.as_bytes())).is_err() {
                    break;
                }
            }
        }
    }
    drop(to);
    let _ = child.wait();
    done
}
