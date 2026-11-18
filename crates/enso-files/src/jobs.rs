//! Work done off the drawing thread, and the messages it sends back.
//!
//! Listing a folder is two passes ([`enso_fs::list`]): the names come back
//! first, in one message, and the window draws them; the metadata follows
//! in batches from a pool of threads. Leaving the folder cancels whatever
//! of the second pass is left.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use enso_fs::{
    list::{self, Listing, Meta},
    ops::{self, Progress, Report, Resolve},
};
use enso_ui::Proxy;

use crate::pane::{Request, describe};

#[derive(Debug)]
pub enum Message {
    /// A folder's entries, or why there are none.
    Listed {
        generation: u64,
        result: Result<Listing, String>,
    },
    /// Metadata for entries `start..` of that listing.
    Stats {
        generation: u64,
        start: usize,
        metas: Vec<Option<Meta>>,
    },
    /// The folder being shown changed on disk (or, with `gone`, went).
    Changed { generation: u64, gone: bool },
    /// The active theme changed.
    ThemeChanged,
    /// The repository the folder is in, and how it stands (`None`: not
    /// in one).
    Git {
        generation: u64,
        status: Option<(PathBuf, enso_fs::git::Status)>,
    },
    /// The drives, as udisks has them now.
    Volumes(Vec<enso_sys::udisks::Volume>),
    /// A drive was mounted here, or why not.
    Mounted(Result<PathBuf, String>),
    /// A drive was ejected (what to say), or why not.
    Ejected(Result<String, String>),
    /// A thumbnail, or that there's none to be had.
    Thumb {
        path: PathBuf,
        thumb: Option<enso_fs::thumbs::Thumb>,
    },
    /// An operation got further.
    Progress(Progress),
    /// An operation found a name taken, and waits for `reply`.
    Conflict {
        source: PathBuf,
        target: PathBuf,
        reply: mpsc::Sender<(Resolve, bool)>,
    },
    /// An operation finished (or stopped).
    Finished { op: Op, report: Report },
}

/// Makes thumbnails on a few threads, newest request first: what was asked
/// for last is what's on screen now.
#[derive(Debug)]
pub struct Thumbnailer {
    queue: Queue,
}

/// Thumbnails asked for, and the signal that wakes a worker.
type Queue = Arc<(
    std::sync::Mutex<Vec<(PathBuf, enso_fs::thumbs::Size)>>,
    std::sync::Condvar,
)>;

impl Thumbnailer {
    /// Start the workers; they sleep until asked.
    pub fn start(proxy: &Proxy<Message>) -> Self {
        let queue: Queue = Arc::default();
        let threads =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
        for _ in 0..threads {
            let (queue, proxy) = (queue.clone(), proxy.clone());
            let _ = thread::Builder::new()
                .name("enso-files-thumbs".to_owned())
                .spawn(move || {
                    loop {
                        let (path, size) = {
                            let (lock, ready) = &*queue;
                            let Ok(mut waiting) = lock.lock() else { return };
                            loop {
                                if let Some(next) = waiting.pop() {
                                    break next;
                                }
                                let Ok(woken) = ready.wait(waiting) else {
                                    return;
                                };
                                waiting = woken;
                            }
                        };
                        let thumb = enso_fs::thumbs::thumbnail(&path, size);
                        if !proxy.send(Message::Thumb { path, thumb }) {
                            return;
                        }
                    }
                });
        }
        Self { queue }
    }

    pub fn request(&self, path: PathBuf, size: enso_fs::thumbs::Size) {
        let (lock, ready) = &*self.queue;
        if let Ok(mut waiting) = lock.lock() {
            waiting.push((path, size));
            ready.notify_one();
        }
    }

    /// Forget what was asked for and not yet started: a folder left.
    pub fn clear(&self) {
        if let Ok(mut waiting) = self.queue.0.lock() {
            waiting.clear();
        }
    }
}

/// Something done to files on a worker thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Copy {
        sources: Vec<PathBuf>,
        into: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        into: PathBuf,
    },
    Duplicate(PathBuf),
    Delete(Vec<PathBuf>),
}

impl Op {
    /// What it's doing, for the status line: "Copying 3 items".
    pub fn describe(&self) -> String {
        let count = |n: usize| enso_fs::format::count(n, "item", "items");
        match self {
            Self::Copy { sources, .. } => format!("Copying {}", count(sources.len())),
            Self::Move { sources, .. } => format!("Moving {}", count(sources.len())),
            Self::Duplicate(_) => "Duplicating".to_owned(),
            Self::Delete(paths) => format!("Deleting {}", count(paths.len())),
        }
    }
}

/// Start `op` on its own thread. Setting the returned flag stops it.
pub fn start(op: Op, proxy: &Proxy<Message>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let mut watcher = Reporter {
        proxy: proxy.clone(),
        stop: stop.clone(),
        last: None,
    };
    let finished = proxy.clone();
    let spawned = thread::Builder::new()
        .name("enso-files-op".to_owned())
        .spawn(move || {
            let report = match &op {
                Op::Copy { sources, into } => ops::copy(sources, into, &mut watcher),
                Op::Move { sources, into } => ops::move_into(sources, into, &mut watcher),
                Op::Duplicate(path) => ops::duplicate(path, &mut watcher),
                Op::Delete(paths) => ops::delete(paths, &mut watcher),
            };
            finished.send(Message::Finished { op, report });
        });
    if let Err(err) = spawned {
        eprintln!("enso-files: cannot start: {err}");
    }
    stop
}

/// An operation's side of the window: progress at most every 50 ms, and
/// questions asked and waited for.
struct Reporter {
    proxy: Proxy<Message>,
    stop: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl ops::Watcher for Reporter {
    fn progress(&mut self, progress: &Progress) {
        let now = Instant::now();
        if self
            .last
            .is_none_or(|last| now - last >= Duration::from_millis(50))
        {
            self.last = Some(now);
            self.proxy.send(Message::Progress(progress.clone()));
        }
    }

    fn conflict(&mut self, source: &Path, target: &Path) -> (Resolve, bool) {
        let (reply, answer) = mpsc::channel();
        self.proxy.send(Message::Conflict {
            source: source.to_owned(),
            target: target.to_owned(),
            reply,
        });
        // No answer (the window went): stop.
        answer.recv().unwrap_or((Resolve::Cancel, true))
    }

    fn stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Lists folders in the background, one at a time.
#[derive(Debug, Default)]
pub struct Loader {
    cancel: Option<Arc<AtomicBool>>,
}

impl Loader {
    /// Start listing what `request` asks for, abandoning the last listing.
    pub fn load(&mut self, request: Request, proxy: &Proxy<Message>) {
        self.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let proxy = proxy.clone();
        let spawned = thread::Builder::new()
            .name("enso-files-list".to_owned())
            .spawn(move || run(request.path, request.generation, &cancel, &proxy));
        if let Err(err) = spawned {
            eprintln!("enso-files: cannot start listing: {err}");
        }
    }

    /// Stop the listing under way, if there is one.
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(path: PathBuf, generation: u64, cancel: &AtomicBool, proxy: &Proxy<Message>) {
    let listing = match Listing::read(&path) {
        Ok(listing) => listing,
        Err(err) => {
            proxy.send(Message::Listed {
                generation,
                result: Err(describe(&err)),
            });
            return;
        }
    };
    // The window takes the listing; the second pass works from a copy, which
    // costs one memcpy of the names.
    let names = listing.clone();
    if !proxy.send(Message::Listed {
        generation,
        result: Ok(listing),
    }) || cancel.load(Ordering::Relaxed)
    {
        return;
    }
    let sent = list::stat(&names, list::threads(), cancel, |start, metas| {
        if !proxy.send(Message::Stats {
            generation,
            start,
            metas,
        }) {
            // The window has gone: no one to tell.
            cancel.store(true, Ordering::Relaxed);
        }
    });
    if let Err(err) = sent {
        eprintln!("enso-files: cannot read sizes in {}: {err}", path.display());
    }
}
