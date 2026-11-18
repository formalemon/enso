//! Following a folder as it changes, for as long as anyone is looking.
//!
//! A [`Watch`] reports, on a thread of its own, that something in a folder
//! was created, removed, renamed, written, or had its details changed —
//! and stops the moment it is dropped. The thread sleeps in `poll` on the
//! inotify descriptor and an eventfd that dropping signals, so an idle
//! watch costs nothing and leaving a folder leaves nothing behind.
//!
//! Bursts are gathered: after the first event the watch waits a moment
//! for the rest (a download writing, a build), then reports once.

use std::{
    io,
    mem::MaybeUninit,
    path::Path,
    thread::{self, JoinHandle},
    time::Duration,
};

use rustix::{
    event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll},
    fd::OwnedFd,
    fs::inotify::{self, CreateFlags, Reader, WatchFlags},
};

/// How long a burst may take to settle before it is reported.
const SETTLE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct Watch {
    stop: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Call `changed` whenever `dir`'s entries change. `gone` is true when
    /// the folder itself was removed or moved away; nothing more is
    /// reported after that.
    pub fn start(dir: &Path, mut changed: impl FnMut(bool) + Send + 'static) -> io::Result<Self> {
        let inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        inotify::add_watch(
            &inotify,
            dir,
            WatchFlags::CREATE
                | WatchFlags::DELETE
                | WatchFlags::MOVED_FROM
                | WatchFlags::MOVED_TO
                | WatchFlags::CLOSE_WRITE
                | WatchFlags::ATTRIB
                | WatchFlags::DELETE_SELF
                | WatchFlags::MOVE_SELF,
        )?;
        let stop = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stopped = stop.try_clone()?;
        let thread = thread::Builder::new()
            .name("enso-fs-watch".to_owned())
            .spawn(move || run(&inotify, &stopped, &mut changed))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = rustix::io::write(&self.stop, &1u64.to_ne_bytes());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(inotify: &OwnedFd, stop: &OwnedFd, changed: &mut dyn FnMut(bool)) {
    let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
    loop {
        if !wait(inotify, stop, None) {
            return;
        }
        let mut gone = drain(inotify, &mut buffer);
        // Let the burst finish; stop early if asked.
        let settle = Timespec {
            tv_sec: 0,
            tv_nsec: SETTLE.as_nanos() as i64,
        };
        while wait(inotify, stop, Some(&settle)) {
            gone |= drain(inotify, &mut buffer);
            if is_stopped(stop) {
                return;
            }
        }
        if is_stopped(stop) {
            return;
        }
        changed(gone);
        if gone {
            return;
        }
    }
}

/// Wait for inotify to have something (true), for the stop signal or the
/// timeout (false).
fn wait(inotify: &OwnedFd, stop: &OwnedFd, timeout: Option<&Timespec>) -> bool {
    let mut fds = [
        PollFd::new(inotify, PollFlags::IN),
        PollFd::new(stop, PollFlags::IN),
    ];
    match poll(&mut fds, timeout) {
        Ok(0) => false,
        Ok(_) => fds[1].revents().is_empty() && !fds[0].revents().is_empty(),
        Err(rustix::io::Errno::INTR) => true,
        Err(_) => false,
    }
}

fn is_stopped(stop: &OwnedFd) -> bool {
    let mut fds = [PollFd::new(stop, PollFlags::IN)];
    matches!(poll(&mut fds, Some(&Timespec { tv_sec: 0, tv_nsec: 0 })), Ok(n) if n > 0)
}

/// Read every pending event. Returns whether the folder itself went.
fn drain(inotify: &OwnedFd, buffer: &mut [MaybeUninit<u8>]) -> bool {
    let mut reader = Reader::new(inotify, buffer);
    let mut gone = false;
    while let Ok(event) = reader.next() {
        let mask = event.events();
        gone |= mask.intersects(
            inotify::ReadFlags::DELETE_SELF
                | inotify::ReadFlags::MOVE_SELF
                | inotify::ReadFlags::IGNORED,
        );
    }
    gone
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, time::Instant};

    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("enso-fs-watch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_burst_is_reported_once() {
        let dir = temp("burst");
        let (tx, rx) = mpsc::channel();
        let watch = Watch::start(&dir, move |gone| {
            let _ = tx.send(gone);
        })
        .unwrap();
        for i in 0..20 {
            fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "only once"
        );
        fs::rename(dir.join("f0"), dir.join("g0")).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
        drop(watch);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dropping_stops_at_once() {
        let dir = temp("stop");
        let watch = Watch::start(&dir, |_| {}).unwrap();
        let start = Instant::now();
        drop(watch);
        assert!(start.elapsed() < Duration::from_millis(500));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_that_goes_says_so() {
        let dir = temp("gone");
        let (tx, rx) = mpsc::channel();
        let _watch = Watch::start(&dir, move |gone| {
            let _ = tx.send(gone);
        })
        .unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    }
}

#[cfg(test)]
mod later {
    use std::{fs, sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn a_change_long_after_starting_is_reported() {
        let dir = std::env::temp_dir().join(format!("enso-fs-watch-later-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let _watch = Watch::start(&dir, move |gone| {
            let _ = tx.send(gone);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        fs::write(dir.join("late"), "x").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)), Ok(false));
        fs::remove_dir_all(dir).unwrap();
    }
}
