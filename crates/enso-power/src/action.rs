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

    /// The key that picks it without moving the selection there first.
    pub const fn key(self) -> char {
        match self {
            Self::Lock => 'l',
            Self::Suspend => 'z',
            Self::Logout => 'e',
            Self::Restart => 'r',
            Self::Shutdown => 's',
        }
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
    fn ids_round_trip_and_keys_are_distinct() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
            // Navigation keys must stay navigation.
            assert!(!['j', 'k', 'n', 'p'].contains(&action.key()), "{action:?}");
        }
        let mut keys: Vec<char> = Action::ALL.iter().map(|action| action.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Action::ALL.len());
        assert_eq!(Action::from_id("hibernate"), None);
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
