//! Checking the user's own password, for unlocking the screen.
//!
//! Password hashes live in `/etc/shadow`, which only root can read. The
//! system's answer for everyone else is `unix_chkpwd`: a small setuid helper
//! shipped with PAM whose one job is to tell a user whether a password is
//! theirs. It is what `pam_unix` itself runs when it is not root, so this
//! asks the same question the login prompt does, without linking PAM (a C
//! library) and without this process ever holding a privilege.
//!
//! What it does not do is run the rest of the PAM stack: no fingerprint
//! readers, no smart cards, no `pam_faillock` counting. It is the password,
//! and only the password.

use std::{
    fmt, fs,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
};

use zeroize::Zeroizing;

/// Where distributions install the helper.
const HELPERS: &[&str] = &[
    "/usr/bin/unix_chkpwd",
    "/usr/sbin/unix_chkpwd",
    "/sbin/unix_chkpwd",
    "/usr/lib/unix_chkpwd",
    "/run/wrappers/bin/unix_chkpwd",
];

/// Longest password PAM accepts.
const MAX_PASSWORD: usize = 512;

/// The password could not be checked at all — which is not the same as it
/// being wrong.
#[derive(Debug)]
pub enum Error {
    /// No `unix_chkpwd` on this system.
    NoHelper,
    /// The account name of this process could not be worked out.
    NoUser,
    /// The helper could not be run.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHelper => f.write_str("unix_chkpwd (part of PAM) is not installed"),
            Self::NoUser => f.write_str("cannot tell which account this is"),
            Self::Io(err) => write!(f, "cannot run unix_chkpwd: {err}"),
        }
    }
}

impl std::error::Error for Error {}

/// Whether passwords can be checked here: the helper is installed and the
/// account has a name. Worth asking *before* locking a screen.
pub fn available() -> Result<(), Error> {
    helper()?;
    user().map(drop)
}

/// The login name of the account this process runs as.
pub fn user() -> Result<String, Error> {
    let uid = fs::metadata("/proc/self").map(|meta| meta.uid()).ok();
    let from_passwd = uid.and_then(crate::accounts::user_name);
    from_passwd
        // Accounts from a directory service are not in the file. The helper
        // checks the name against the real uid itself, so the environment
        // can suggest but not lie.
        .or_else(|| std::env::var("USER").ok().filter(|name| !name.is_empty()))
        .ok_or(Error::NoUser)
}

fn helper() -> Result<&'static Path, Error> {
    HELPERS
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .ok_or(Error::NoHelper)
}

/// Whether `password` is this account's password. Blocks while the helper
/// hashes it — a deliberate fraction of a second — so call it off the
/// interface thread.
pub fn verify(password: &[u8]) -> Result<bool, Error> {
    // Nothing the helper would accept; and a NUL would end the password
    // early, checking something other than what was typed.
    if password.is_empty() || password.len() > MAX_PASSWORD || password.contains(&0) {
        return Ok(false);
    }
    let user = user()?;
    let mut child = Command::new(helper()?)
        .arg(&user)
        .arg("nullok")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Error::Io)?;

    // The helper reads one NUL-terminated password from its standard input.
    let mut line = Zeroizing::new(Vec::with_capacity(password.len() + 1));
    line.extend_from_slice(password);
    line.push(0);
    let written = child.stdin.take().map(|mut stdin| stdin.write_all(&line));
    drop(line);

    let status = child.wait().map_err(Error::Io)?;
    match written {
        Some(Ok(())) => Ok(status.success()),
        Some(Err(err)) => Err(Error::Io(err)),
        None => Err(Error::Io(io::Error::other("no pipe to the helper"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_could_not_be_a_password_without_asking() {
        assert!(!verify(b"").unwrap());
        assert!(!verify(b"with\0nul").unwrap());
        assert!(!verify(&[b'a'; MAX_PASSWORD + 1]).unwrap());
    }
}
