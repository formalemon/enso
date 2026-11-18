//! Work done off the drawing thread, and the messages it sends back.
//!
//! Listing a folder is two passes ([`enso_fs::list`]): the names come back
//! first, in one message, and the window draws them; the metadata follows
//! in batches from a pool of threads. Leaving the folder cancels whatever
//! of the second pass is left.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use enso_fs::list::{self, Listing, Meta};
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
