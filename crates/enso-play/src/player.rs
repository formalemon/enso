//! The players, kept off the interface's thread: one worker carries out
//! what's asked (play, skip, seek) and reads the players afresh after each;
//! another listens for news from them. Covers load on threads of their own.

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use enso_sys::mpris::{Mpris, Player};

use crate::art::{self, Cover};

/// Something asked of a player.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    PlayPause(String),
    Next(String),
    Previous(String),
    /// Move within the track, as it was read.
    Seek(Box<Player>, Duration),
    /// Load the cover at this address.
    Cover(String),
    /// Read the players again.
    Refresh,
}

/// News for the card.
#[derive(Debug)]
pub enum Update {
    Players(Vec<Player>),
    Cover(String, Option<Cover>),
}

/// The way to the worker. Requests wait until it's [served](Inbox::serve).
#[derive(Clone, Debug)]
pub struct Worker {
    requests: Sender<Request>,
}

/// What the worker will take its requests from.
#[derive(Debug)]
pub struct Inbox {
    pub(crate) requests: Receiver<Request>,
    refresh: Sender<Request>,
}

impl Worker {
    pub fn new() -> (Self, Inbox) {
        let (requests, inbox) = mpsc::channel();
        let refresh = requests.clone();
        (
            Self { requests },
            Inbox {
                requests: inbox,
                refresh,
            },
        )
    }

    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

impl Inbox {
    /// Start the workers. `send` passes news to the interface and says
    /// whether anyone's still there to hear it.
    pub fn serve(
        self,
        mpris: Mpris,
        cache: PathBuf,
        send: impl Fn(Update) -> bool + Clone + Send + 'static,
    ) {
        let Self {
            requests: inbox,
            refresh,
        } = self;
        let watcher = mpris.clone();
        let _ = thread::Builder::new()
            .name("enso-play-watch".to_owned())
            .spawn(move || {
                if let Err(err) = watcher.watch(|| refresh.send(Request::Refresh).is_ok()) {
                    eprintln!("enso-play: stopped following the players: {err}");
                }
            });
        let _ = thread::Builder::new()
            .name("enso-play".to_owned())
            .spawn(move || serve(&mpris, &cache, &inbox, &send));
    }
}

fn serve(
    mpris: &Mpris,
    cache: &Path,
    inbox: &Receiver<Request>,
    send: &(impl Fn(Update) -> bool + Clone + Send + 'static),
) {
    let mut asked = HashSet::new();
    let mut pending = VecDeque::new();
    loop {
        let request = match pending.pop_front() {
            Some(request) => request,
            None => match inbox.recv() {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        let done = match &request {
            Request::PlayPause(bus) => mpris.play_pause(bus),
            Request::Next(bus) => mpris.next(bus),
            Request::Previous(bus) => mpris.previous(bus),
            Request::Seek(player, to) => mpris.set_position(player, *to),
            Request::Cover(url) => {
                if asked.insert(url.clone()) {
                    let (url, cache, send) = (url.clone(), cache.to_path_buf(), send.clone());
                    let _ = thread::Builder::new()
                        .name("enso-play-cover".to_owned())
                        .spawn(move || {
                            let cover = art::load(&url, &cache);
                            send(Update::Cover(url, cover));
                        });
                }
                continue;
            }
            Request::Refresh => Ok(()),
        };
        if let Err(err) = done {
            eprintln!("enso-play: {err}");
        }
        // Several pieces of news at once need only one reading; anything
        // else waiting is kept for after it.
        while let Ok(next) = inbox.try_recv() {
            if next != Request::Refresh {
                pending.push_back(next);
            }
        }
        let players = match mpris.players() {
            Ok(players) => players,
            Err(err) => {
                eprintln!("enso-play: cannot read the players: {err}");
                continue;
            }
        };
        if !send(Update::Players(players)) {
            return;
        }
    }
}
