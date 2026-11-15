//! Change notification for small state files, via inotify.
//!
//! This is how running tools follow `enso-ctl theme set` without polling and
//! without a daemon: the active theme lives in a state file, and every
//! component watches its directory.

use std::{io, mem::MaybeUninit, path::Path, thread};

use rustix::fs::inotify::{self, CreateFlags, Reader, WatchFlags};

/// Call `on_change` on a background thread whenever a file in `dir` is
/// written, created, replaced, or removed. Bursts of events are coalesced.
/// Watching stops when `on_change` returns `false`.
///
/// `dir` is created if missing so the watch works before anything is written.
pub fn watch_dir(
    dir: &Path,
    mut on_change: impl FnMut() -> bool + Send + 'static,
) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let fd = inotify::init(CreateFlags::CLOEXEC)?;
    inotify::add_watch(
        &fd,
        dir,
        WatchFlags::CLOSE_WRITE | WatchFlags::MOVED_TO | WatchFlags::CREATE | WatchFlags::DELETE,
    )?;

    thread::Builder::new()
        .name("enso-watch".to_owned())
        .spawn(move || {
            let mut buffer = [MaybeUninit::<u8>::uninit(); 1024];
            let mut reader = Reader::new(&fd, &mut buffer);
            loop {
                // Blocks until something happens in the directory.
                if reader.next().is_err() {
                    return;
                }
                while !reader.is_buffer_empty() {
                    if reader.next().is_err() {
                        return;
                    }
                }
                if !on_change() {
                    return;
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn reports_writes_and_atomic_replacements() {
        let dir = std::env::temp_dir().join(format!("enso-ui-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (tx, rx) = mpsc::channel();
        // The directory does not exist yet; watching creates it.
        watch_dir(&dir, move || tx.send(()).is_ok()).expect("start watching");
        let wait = Duration::from_secs(5);

        std::fs::write(dir.join("theme"), "rose-pine-dawn\n").expect("write");
        rx.recv_timeout(wait).expect("a direct write is reported");

        // `enso-ctl theme set` writes a temporary file and renames it over.
        while rx.try_recv().is_ok() {}
        std::fs::write(dir.join(".theme.tmp"), "rose-pine-main\n").expect("write");
        std::fs::rename(dir.join(".theme.tmp"), dir.join("theme")).expect("rename");
        rx.recv_timeout(wait)
            .expect("an atomic replacement is reported");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
