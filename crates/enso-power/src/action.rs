//! What the power menu can do, and how each thing is done.
//!
//! Nothing here goes through a command-line wrapper: the machine is turned
//! off through logind, and logging out is a request on Niri's own socket.
//! Locking is the one exception by design — it belongs to another program
//! (`enso-lock`, unless configured otherwise), and starting that program is
//! the whole of the action.

use std::{
    env, fmt, io,
    path::PathBuf,
    process::{Command, Stdio},
};

use enso_core::{Icon, Role, style::icon};
use enso_sys::logind::{self, Machine};
use niri_ipc::{Request, socket::Socket};
use serde::Deserialize;

/// One entry of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Lock,
    Suspend,
    /// Leave Niri.
    Logout,
    Restart,
    Shutdown,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::Lock,
        Self::Suspend,
        Self::Logout,
        Self::Restart,
        Self::Shutdown,
    ];

    /// The word on the command line and in the config file.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Suspend => "suspend",
            Self::Logout => "logout",
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock",
            Self::Suspend => "Sleep",
            Self::Logout => "Log out",
            Self::Restart => "Restart",
            Self::Shutdown => "Shut down",
        }
    }

    /// What a second press confirms, as the end of "Press again to …".
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Suspend => "sleep",
            Self::Logout => "log out",
            Self::Restart => "restart",
            Self::Shutdown => "shut down",
        }
    }

    /// The letters of its name that may pick it, best first: the one
    /// people reach for, then the rest of the name. Never `j` or `k`, which
    /// move the selection.
    const fn letters(self) -> &'static [char] {
        match self {
            Self::Lock => &['l', 'o', 'c'],
            Self::Suspend => &['s', 'p', 'e', 'l'],
            Self::Logout => &['o', 'l', 'g', 'u', 't'],
            Self::Restart => &['r', 't', 'e', 's', 'a'],
            Self::Shutdown => &['s', 'd', 'h', 'u', 't', 'o', 'w', 'n'],
        }
    }

    /// Who chooses a letter first when two want the same one: what is used
    /// most, and what is most expected to be on its first letter.
    const PRIORITY: [Self; 5] = [
        Self::Lock,
        Self::Shutdown,
        Self::Restart,
        Self::Logout,
        Self::Suspend,
    ];

    /// Where the letter `key` sits in its label, to be marked there: a
    /// letter that starts a word if there is one (the `o` of "out" in "Log
    /// out"), else its first appearance. A byte range.
    pub fn mark(self, key: char) -> Option<std::ops::Range<usize>> {
        let label = self.label();
        let mut first = None;
        let mut previous = ' ';
        for (at, c) in label.char_indices() {
            if c.to_ascii_lowercase() == key {
                let range = at..at + c.len_utf8();
                if !previous.is_alphanumeric() {
                    return Some(range);
                }
                first.get_or_insert(range);
            }
            previous = c;
        }
        first
    }

    pub const fn icon(self) -> Icon {
        match self {
            Self::Lock => icon::LOCK,
            Self::Suspend => icon::SLEEP,
            Self::Logout => icon::LOGOUT,
            Self::Restart => icon::RESTART,
            Self::Shutdown => icon::POWER,
        }
    }

    /// The palette role that tints it: cool for what can be undone at once,
    /// warming towards what cannot.
    pub const fn role(self) -> Role {
        match self {
            Self::Lock => Role::Foam,
            Self::Suspend => Role::Iris,
            Self::Logout => Role::Gold,
            Self::Restart => Role::Rose,
            Self::Shutdown => Role::Love,
        }
    }

    /// The key for each of `actions`, in order: every one its own letter,
    /// from its own name, so that what is marked is what is pressed.
    pub fn keys(actions: &[Self]) -> Vec<Option<char>> {
        let mut keys = vec![None; actions.len()];
        let mut taken = Vec::new();
        for action in Self::PRIORITY {
            let Some(at) = actions.iter().position(|a| *a == action) else {
                continue;
            };
            let key = action
                .letters()
                .iter()
                .copied()
                .find(|c| !taken.contains(c));
            taken.extend(key);
            keys[at] = key;
        }
        keys
    }

    /// Whether it ends the session, and with it anything unsaved.
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Logout | Self::Restart | Self::Shutdown)
    }

    /// Do it. Returns once the request has been accepted; the screen
    /// locking or the machine going down follows on its own.
    pub fn run(self, lock_command: &[String]) -> Result<(), Error> {
        match self {
            Self::Lock => lock(lock_command),
            Self::Suspend => machine(Machine::Suspend),
            Self::Restart => machine(Machine::Reboot),
            Self::Shutdown => machine(Machine::PowerOff),
            Self::Logout => logout(),
        }
    }
}

/// An action that could not be carried out, in words for the menu.
#[derive(Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn machine(action: Machine) -> Result<(), Error> {
    logind::machine(action).map_err(|err| Error(err.to_string()))
}

fn logout() -> Result<(), Error> {
    let quit = Request::Action(niri_ipc::Action::Quit {
        skip_confirmation: true,
    });
    match Socket::connect().and_then(|mut socket| socket.send(quit)) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(message)) => Err(Error(format!("Niri refused: {message}"))),
        // Not under Niri (or it is not answering): end the session the way
        // any desktop can be ended.
        Err(_) => logind::end_session().map_err(|err| Error(err.to_string())),
    }
}

fn lock(command: &[String]) -> Result<(), Error> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| Error("no lock command is configured".to_owned()))?;
    Command::new(companion(program))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map(drop)
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => Error(format!("{program} is not installed")),
            _ => Error(format!("cannot start {program}: {err}")),
        })
}

/// A bare program name resolves to the one installed next to this binary if
/// there is one (so a build tree or a private prefix uses its own),
/// otherwise to whichever is on `PATH`. Anything with a slash is taken as
/// written.
fn companion(program: &str) -> PathBuf {
    if program.contains('/') {
        return PathBuf::from(program);
    }
    env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(program)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("hibernate"), None);
    }

    /// Each key is a letter of the name it picks, marked where it stands.
    #[test]
    fn keys_are_letters_of_their_names() {
        use Action::*;
        // The usual menu: L, o, R, S.
        assert_eq!(
            Action::keys(&[Lock, Logout, Restart, Shutdown]),
            [Some('l'), Some('o'), Some('r'), Some('s')]
        );
        // With Sleep too, Shut down keeps the S, and Sleep takes its p —
        // whatever order the menu lists them in.
        assert_eq!(
            Action::keys(&[Suspend, Shutdown, Lock]),
            [Some('p'), Some('s'), Some('l')]
        );
        let all = Action::keys(&Action::ALL);
        let mut distinct: Vec<char> = all.iter().flatten().copied().collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), Action::ALL.len());
        for (action, key) in Action::ALL.into_iter().zip(all) {
            let key = key.unwrap();
            // Navigation keys stay navigation.
            assert!(!['j', 'k'].contains(&key), "{action:?}");
            assert!(action.label().to_lowercase().contains(key), "{action:?}");
        }

        // Marked where a word starts with it: the o of "out", not of "Log".
        assert_eq!(Logout.mark('o'), Some(4..5));
        assert_eq!(&Logout.label()[4..5], "o");
        assert_eq!(Lock.mark('l'), Some(0..1));
        assert_eq!(Shutdown.mark('s'), Some(0..1));
        // Else where it first appears.
        assert_eq!(Suspend.mark('p'), Some(4..5));
        assert_eq!(Lock.mark('z'), None);
    }

    #[test]
    fn only_what_ends_the_session_is_final() {
        assert!(!Action::Lock.is_final() && !Action::Suspend.is_final());
        assert!(Action::Logout.is_final() && Action::Shutdown.is_final());
    }

    #[test]
    fn a_missing_lock_program_says_so() {
        let err = Action::Lock
            .run(&["enso-lock-that-is-not-installed".to_owned()])
            .unwrap_err();
        assert!(err.to_string().contains("not installed"), "{err}");
        assert!(Action::Lock.run(&[]).is_err());
        assert_eq!(companion("/bin/true"), PathBuf::from("/bin/true"));
    }
}
