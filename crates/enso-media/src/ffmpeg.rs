//! Running ffmpeg and ffprobe: found on the `PATH`, quiet, and killed with
//! whatever started them.

use std::{
    ffi::OsStr,
    path::Path,
    process::{self, Command, Stdio},
    time::Duration,
};

use crate::Error;

/// Whether ffmpeg and ffprobe are both there to run.
pub fn available() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|tool| on_path(tool))
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(tool);
            candidate.is_file()
                && std::fs::metadata(&candidate).is_ok_and(|meta| {
                    std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0
                })
        })
    })
}

/// `tool` with `args`, its output piped to us, its errors kept quiet.
pub(crate) fn command<I, S>(tool: &str, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(tool);
    if tool == "ffmpeg" {
        command.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    }
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// Run `tool` to its end and take what it wrote.
pub(crate) fn output<I, S>(tool: &str, args: I) -> Result<Vec<u8>, Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = command(tool, args)
        .output()
        .map_err(|err| missing(tool, &err))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(Error::new(format!("{tool} could not read it")))
    }
}

/// Start `tool`, to be read from as it goes.
pub(crate) fn spawn<I, S>(tool: &str, args: I) -> Result<Child, Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    command(tool, args)
        .spawn()
        .map(Child)
        .map_err(|err| missing(tool, &err))
}

fn missing(tool: &str, err: &std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::NotFound {
        Error::new(format!("{tool} isn't installed"))
    } else {
        Error::new(format!("cannot run {tool}: {err}"))
    }
}

/// A running ffmpeg, stopped when this goes.
#[derive(Debug)]
pub(crate) struct Child(pub process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A time as ffmpeg takes one: seconds, to the millisecond.
pub(crate) fn seconds(at: Duration) -> String {
    format!("{:.3}", at.as_secs_f64())
}

/// A path as an argument that can't be mistaken for an option.
pub(crate) fn input(path: &Path) -> std::ffi::OsString {
    let mut arg = std::ffi::OsString::from("file:");
    arg.push(path.as_os_str());
    arg
}
