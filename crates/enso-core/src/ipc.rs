//! Words between enso's tools, and one copy of a tool at a time.
//!
//! A tool that others may need to reach listens on a Unix socket in
//! `$XDG_RUNTIME_DIR` named after it — `enso-notify.sock`, `enso-ctl.sock` —
//! for one word per connection: `close`, `toggle`, … [`tell`] says one and
//! waits for no answer; a tool that is not running is simply not told, so
//! each tool still works alone.
//!
//! A tool that must not run twice holds a lock on `<tool>.lock` there for as
//! long as it runs ([`only`]). The kernel lets go of it when the process
//! ends, however it ends, so a crash never leaves a tool locked out.

use std::{
    env,
    fs::{self, File, TryLockError},
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    thread,
    time::Duration,
};

/// How long a word may take to say, or to arrive: anyone slower is ignored.
const PATIENCE: Duration = Duration::from_millis(500);

/// Where per-session sockets and locks live.
fn runtime_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// The socket `tool` listens on.
pub fn socket(tool: &str) -> Option<PathBuf> {
    runtime_dir().map(|dir| dir.join(format!("{tool}.sock")))
}

/// Say `word` to `tool`. Whether it was there to hear it.
pub fn tell(tool: &str, word: &str) -> bool {
    socket(tool).is_some_and(|path| say(&path, word).is_ok())
}

/// Whether `tool` is running and listening. Knocking says nothing: the
/// tool hears an empty word, which means nothing.
pub fn is_listening(tool: &str) -> bool {
    socket(tool).is_some_and(|path| UnixStream::connect(path).is_ok())
}

/// Say `word` on the socket at `path`.
pub fn say(path: &std::path::Path, word: &str) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(PATIENCE))?;
    writeln!(stream, "{word}")
}

/// Listen as `tool` on a background thread: `heard` gets each word, trimmed,
/// until it returns `false`. A socket left behind by a copy that died is
/// cleared away first.
pub fn listen(tool: &str, mut heard: impl FnMut(&str) -> bool + Send + 'static) -> io::Result<()> {
    let path = socket(tool).ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    // A socket nobody answers on is left over from a copy that died.
    if UnixStream::connect(&path).is_err() {
        let _ = fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;
    thread::Builder::new()
        .name(format!("{tool}-words"))
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                // Whoever connects says one word and hangs up; nobody gets
                // to hold the line.
                let _ = stream.set_read_timeout(Some(PATIENCE));
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() && !heard(line.trim()) {
                    return;
                }
            }
        })?;
    Ok(())
}

/// Whether this is the one running copy of a tool.
#[derive(Debug)]
pub enum Only {
    /// It is: the lock is held for as long as this lives. (`None` where
    /// there is nowhere to keep a lock: then nothing can be kept to one.)
    Mine(Option<File>),
    /// Another copy is running.
    Taken,
}

/// Claim being the one running copy of `tool`.
pub fn only(tool: &str) -> Only {
    let Some(dir) = runtime_dir() else {
        return Only::Mine(None);
    };
    let Ok(file) = File::create(dir.join(format!("{tool}.lock"))) else {
        return Only::Mine(None);
    };
    match file.try_lock() {
        Ok(()) => Only::Mine(Some(file)),
        Err(TryLockError::WouldBlock) => Only::Taken,
        Err(TryLockError::Error(_)) => Only::Mine(None),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn words_reach_a_listener_and_nobody_is_harmless() {
        let dir = env::temp_dir().join(format!("enso-ipc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.sock");
        let _ = fs::remove_file(&path);
        assert!(say(&path, "close").is_err());

        let listener = UnixListener::bind(&path).unwrap();
        let (sent, got) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                sent.send(line.trim().to_owned()).unwrap();
            }
        });
        say(&path, "close").unwrap();
        say(&path, "toggle").unwrap();
        let heard: Vec<String> = got.iter().take(2).collect();
        assert_eq!(heard, ["close", "toggle"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_is_held_once() {
        let dir = env::temp_dir().join(format!("enso-ipc-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.lock");
        let first = File::create(&path).unwrap();
        first.try_lock().unwrap();
        let second = File::create(&path).unwrap();
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        drop(first);
        second.try_lock().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
