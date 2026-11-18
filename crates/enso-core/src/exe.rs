//! This program and its companions, as things to run.
//!
//! A tool that starts a copy of itself — a lock screen from its watcher, a
//! prompt from its agent, a picker from its portal — must not look itself
//! up by path. Once the binary is rebuilt or upgraded the path names a
//! different program, or, between the old file going and the new one
//! arriving, nothing at all: the kernel then reports the old path plus
//! `" (deleted)"`, and starting that fails. `/proc/self/exe` is the very
//! file this process runs, which the kernel keeps for as long as it runs,
//! so [`again`] always starts and always starts the same version — parent
//! and child never disagree about what they say to each other.
//!
//! Some paths outlive this process instead: git's config, a D-Bus service
//! file, a window opened for someone else. Those want the program as it is
//! installed, so a rebuild needs no change to them: that's [`installed`].

use std::{
    env,
    ffi::OsStr,
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

/// The file this process runs, for as long as it runs.
const SELF: &str = "/proc/self/exe";

/// What the kernel names a program started through [`SELF`].
const KERNEL_NAME: &str = "exe";

/// The longest name the kernel keeps for a process (its `comm`).
const NAME_MAX: usize = 15;

/// A command that runs this program again: the same binary, whatever has
/// become of its path since this process started. Its `argv[0]` is the
/// program's own name; the copy should call [`restore_name`] first thing.
pub fn again() -> Command {
    let mut command = Command::new(SELF);
    command.arg0(name());
    command
}

/// This program's name: the file name it was started as, without the
/// kernel's `" (deleted)"`.
pub fn name() -> String {
    let path = started_as();
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            let arg0 = env::args_os().next()?;
            Some(Path::new(&arg0).file_name()?.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "enso".to_owned())
}

/// Give back the name the kernel takes from a program started by
/// [`again`]: it names the process after the path it was started through,
/// `exe`, which is what `ps`, `top`, and the keyring's "who's asking"
/// would show. Does nothing for a program started any other way.
pub fn restore_name() {
    let Ok(current) = fs::read_to_string("/proc/self/comm") else {
        return;
    };
    let Some(arg0) = env::args_os().next() else {
        return;
    };
    if let Some(name) = wanted_name(current.trim_end(), &arg0) {
        // Writing one's own comm needs no privilege; a failure only
        // leaves the name as it was.
        let _ = fs::write("/proc/self/comm", name);
    }
}

/// The name to give a process called `current` that was started with
/// `arg0`, if it needs one.
fn wanted_name(current: &str, arg0: &OsStr) -> Option<String> {
    if current != KERNEL_NAME {
        return None;
    }
    let name = Path::new(arg0).file_name()?.to_string_lossy();
    if name == KERNEL_NAME {
        return None;
    }
    let mut end = name.len().min(NAME_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    Some(name[..end].to_owned())
}

/// This program as something to write down for later — in git's config,
/// a D-Bus service file, a window opened on someone else's behalf: the
/// one of its name on `PATH` (an installed link, say) when there is one,
/// so a rebuild changes nothing, otherwise the path it was started from.
pub fn installed() -> PathBuf {
    on_path(&name()).unwrap_or_else(started_as)
}

/// Another tool of the suite: the one installed next to this program if
/// there is one (so a build tree or a private prefix uses its own),
/// otherwise whichever is on `PATH`. Anything with a slash is taken as
/// written.
pub fn companion(program: &str) -> PathBuf {
    if program.contains('/') {
        return PathBuf::from(program);
    }
    started_as()
        .parent()
        .map(|dir| dir.join(program))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

/// The path this process was started from, without the kernel's
/// `" (deleted)"` when the file has since been replaced or removed.
fn started_as() -> PathBuf {
    let exe = env::current_exe().unwrap_or_default();
    match exe
        .to_str()
        .and_then(|text| text.strip_suffix(" (deleted)"))
    {
        Some(original) => PathBuf::from(original),
        None => exe,
    }
}

/// The first executable file called `program` in a directory on `PATH`.
fn on_path(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    /// Set in a copy of the test program to make [`probe`] do its work.
    const PROBE: &str = "ENSO_EXE_PROBE";

    #[test]
    fn names_only_what_the_kernel_misnamed() {
        assert_eq!(wanted_name("enso-lock", OsStr::new("enso-lock")), None);
        assert_eq!(
            wanted_name("exe", OsStr::new("enso-lock")).as_deref(),
            Some("enso-lock")
        );
        assert_eq!(
            wanted_name("exe", OsStr::new("/usr/bin/enso-keyring")).as_deref(),
            Some("enso-keyring")
        );
        assert_eq!(
            wanted_name("exe", OsStr::new("a-name-much-too-long")).as_deref(),
            Some("a-name-much-too")
        );
        assert_eq!(wanted_name("exe", OsStr::new("exe")), None);
    }

    #[test]
    fn companions_are_found_next_door_or_taken_as_written() {
        assert_eq!(companion("/bin/true"), PathBuf::from("/bin/true"));
        assert_eq!(
            companion("no-such-companion"),
            PathBuf::from("no-such-companion")
        );
    }

    #[test]
    fn this_program_has_a_name() {
        let name = name();
        assert!(!name.is_empty());
        assert!(!name.contains("(deleted)"));
    }

    /// Run a copy of this test program from a file that is then deleted, as
    /// a rebuild does to a running tool, and have it start itself again.
    #[test]
    fn starts_again_after_its_file_is_gone() {
        let dir = crate::test_dir("exe-again");
        fs::create_dir_all(&dir).unwrap();
        let copy = dir.join("enso-exe-probe");
        fs::copy(env::current_exe().unwrap(), &copy).unwrap();
        let output = Command::new(&copy)
            .args(["exe::tests::probe", "--exact", "--nocapture"])
            .env(PROBE, "parent")
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&dir);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{text}");
        assert!(text.contains("child: enso-exe-probe"), "{text}");
    }

    /// The other half of the test above: does nothing unless asked.
    #[test]
    fn probe() {
        match env::var(PROBE).as_deref() {
            Ok("parent") => {
                fs::remove_file(env::current_exe().unwrap()).unwrap();
                assert!(started_as().ends_with("enso-exe-probe"));
                let output = again()
                    .args(["exe::tests::probe", "--exact", "--nocapture"])
                    .env(PROBE, "child")
                    .output()
                    .unwrap();
                assert!(output.status.success());
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
            Ok("child") => {
                restore_name();
                let comm = fs::read_to_string("/proc/self/comm").unwrap();
                println!("child: {}", comm.trim_end());
            }
            _ => {}
        }
    }
}
