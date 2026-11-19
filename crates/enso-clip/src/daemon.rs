//! The daemon: it watches the clipboard (`ext-data-control`), keeps each
//! new thing copied, and keeps the newest on the clipboard when the program
//! that copied it quits — so what was copied from a window that's since
//! closed still pastes.
//!
//! What a password manager marks as a secret
//! (`x-kde-passwordManagerHint`, which KeePassXC and others set) is never
//! read, let alone kept.

use std::{
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use enso_core::ipc;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    pipe::{PipeFlags, pipe_with},
};
use smithay_client_toolkit::reexports::{
    calloop::{
        EventLoop,
        channel::{self, Event as ChannelEvent, Sender},
    },
    calloop_wayland_source::WaylandSource,
    client::{
        Connection, Dispatch, Proxy, QueueHandle, event_created_child,
        globals::{GlobalListContents, registry_queue_init},
        protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
    },
    protocols::ext::data_control::v1::client::{
        ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
        ext_data_control_manager_v1::ExtDataControlManagerV1,
        ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
        ext_data_control_source_v1::{self, ExtDataControlSourceV1},
    },
};

use crate::store::{self, Clip, Store};

/// The name the daemon listens under.
pub const NAME: &str = "enso-clip";

/// The text types, best first.
const TEXT: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];
const PNG: &str = "image/png";
/// What password managers mark a secret with.
const SECRET: &str = "x-kde-passwordManagerHint";
/// How long a program is given to hand over what it copied.
const RECEIVE: Duration = Duration::from_secs(3);

/// The formats something is offered in, and the bytes of each.
type Formats = Arc<Vec<(String, Vec<u8>)>>;

pub struct Daemon {
    store: Store,
    conn: Connection,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
    qh: QueueHandle<Daemon>,
    /// Ours, while we're the one offering the clipboard.
    serving: Option<(ExtDataControlSourceV1, Formats)>,
    /// We just made ourselves the clipboard: the next selection is ours.
    ours_next: bool,
    /// The offer that is the clipboard now.
    current: Option<ExtDataControlOfferV1>,
    received: Sender<Clip>,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

fn formats(clip: &Clip) -> Formats {
    Arc::new(match clip {
        Clip::Text(text) => TEXT
            .iter()
            .map(|mime| ((*mime).to_owned(), text.as_bytes().to_vec()))
            .collect(),
        Clip::Png(png) => vec![(PNG.to_owned(), png.clone())],
    })
}

impl Daemon {
    /// Offer `clip` as the clipboard ourselves.
    fn serve(&mut self, clip: &Clip) {
        if let Some((source, _)) = self.serving.take() {
            source.destroy();
        }
        let formats = formats(clip);
        let source = self.manager.create_data_source(&self.qh, ());
        for (mime, _) in formats.iter() {
            source.offer(mime.clone());
        }
        self.device.set_selection(Some(&source));
        self.ours_next = true;
        self.serving = Some((source, formats));
        let _ = self.conn.flush();
    }

    /// Something new is the clipboard: read it, unless it's ours or secret.
    fn selection(&mut self, offer: Option<ExtDataControlOfferV1>) {
        if let Some(old) = self.current.take() {
            old.destroy();
        }
        let Some(offer) = offer else {
            // Nothing on the clipboard: its program has gone. Keep the
            // newest thing there.
            if let Some(newest) = self.store.newest() {
                self.serve(&newest);
            }
            return;
        };
        if std::mem::take(&mut self.ours_next) {
            self.current = Some(offer);
            return;
        }
        // Someone else copied: we no longer offer anything.
        if let Some((source, _)) = self.serving.take() {
            source.destroy();
        }
        let mimes: Vec<String> = offer
            .data::<Mutex<Vec<String>>>()
            .and_then(|mimes| mimes.lock().ok().map(|mimes| mimes.clone()))
            .unwrap_or_default();
        let wanted = if mimes.iter().any(|mime| mime == SECRET) {
            None
        } else {
            TEXT.iter()
                .find(|text| mimes.iter().any(|mime| mime == *text))
                .copied()
                .or_else(|| mimes.iter().any(|mime| mime == PNG).then_some(PNG))
        };
        if let Some(mime) = wanted {
            self.receive(&offer, mime);
        }
        self.current = Some(offer);
    }

    /// Ask for `mime` and read it on a thread of its own.
    fn receive(&self, offer: &ExtDataControlOfferV1, mime: &'static str) {
        let Ok((read, write)) = pipe_with(PipeFlags::CLOEXEC) else {
            return;
        };
        offer.receive(mime.to_owned(), write.as_fd());
        drop(write);
        let _ = self.conn.flush();
        let received = self.received.clone();
        let _ = thread::Builder::new()
            .name("enso-clip-read".to_owned())
            .spawn(move || {
                let limit = if mime == PNG {
                    store::MAX_PICTURE
                } else {
                    store::MAX_TEXT
                };
                let Some(bytes) = read_all(read, limit) else {
                    return;
                };
                let clip = if mime == PNG {
                    Clip::Png(bytes)
                } else {
                    match String::from_utf8(bytes) {
                        Ok(text) if !text.trim().is_empty() => Clip::Text(text),
                        _ => return,
                    }
                };
                let _ = received.send(clip);
            });
    }

    fn keep(&mut self, clip: &Clip) {
        if let Err(err) = self.store.add(clip) {
            eprintln!("enso-clip: cannot keep what was copied: {err}");
        }
    }

    /// A word from the picker or the command line.
    fn command(&mut self, word: &str) {
        let (verb, rest) = word.split_once(' ').unwrap_or((word, ""));
        let id = rest.trim().parse::<u64>().ok();
        match (verb, id) {
            ("use", Some(id)) => {
                if let Some(clip) = self.store.get(id) {
                    self.keep(&clip);
                    self.serve(&clip);
                }
            }
            ("delete", Some(id)) => {
                let _ = self.store.delete(id);
            }
            ("clear", _) => {
                let _ = self.store.clear();
            }
            _ => {}
        }
    }
}

/// Read until the end, or `limit` bytes, or the program stalls.
fn read_all(fd: OwnedFd, limit: usize) -> Option<Vec<u8>> {
    let deadline = Instant::now() + RECEIVE;
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let timeout = Timespec::try_from(left).ok()?;
        let mut fds = [PollFd::new(&file, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).ok()? == 0 {
            return None;
        }
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            return Some(bytes);
        }
        bytes.extend_from_slice(&buf[..read]);
        if bytes.len() > limit {
            return None;
        }
    }
}

/// Run the daemon until the session ends.
pub fn run() -> Result<(), String> {
    let _only = match ipc::only(NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => return Err("already running".to_owned()),
    };
    let dir = store::dir().ok_or("no XDG_RUNTIME_DIR to keep the history in")?;
    let store = Store::new(dir).map_err(|err| format!("cannot keep a history: {err}"))?;

    let conn =
        Connection::connect_to_env().map_err(|err| format!("cannot connect to Wayland: {err}"))?;
    let (globals, queue) =
        registry_queue_init::<Daemon>(&conn).map_err(|err| format!("Wayland registry: {err}"))?;
    let qh = queue.handle();
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "the compositor has no seat".to_owned())?;
    let manager: ExtDataControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "the compositor lacks ext-data-control".to_owned())?;
    let device = manager.get_data_device(&seat, &qh, ());

    let mut event_loop: EventLoop<'static, Daemon> =
        EventLoop::try_new().map_err(|err| err.to_string())?;
    let handle = event_loop.handle();
    let (received, clips) = channel::channel::<Clip>();
    handle
        .insert_source(clips, |event, (), daemon| {
            if let ChannelEvent::Msg(clip) = event {
                daemon.keep(&clip);
            }
        })
        .map_err(|err| err.to_string())?;
    let (sender, words) = channel::channel::<String>();
    handle
        .insert_source(words, |event, (), daemon| {
            if let ChannelEvent::Msg(word) = event {
                daemon.command(&word);
            }
        })
        .map_err(|err| err.to_string())?;
    if let Err(err) = ipc::listen(NAME, move |word| sender.send(word.to_owned()).is_ok()) {
        eprintln!("enso-clip: the picker will not reach this one: {err}");
    }
    WaylandSource::new(conn.clone(), queue)
        .insert(handle)
        .map_err(|err| err.to_string())?;
    let mut daemon = Daemon {
        store,
        conn,
        manager,
        device,
        qh,
        serving: None,
        ours_next: false,
        current: None,
        received,
    };
    event_loop
        .run(None, &mut daemon, |_| {})
        .map_err(|err| err.to_string())
}

impl Dispatch<ExtDataControlDeviceV1, ()> for Daemon {
    fn event(
        daemon: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id } => daemon.selection(id),
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                offer.destroy();
            }
            ext_data_control_device_v1::Event::Finished => std::process::exit(0),
            _ => {}
        }
    }

    event_created_child!(Daemon, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, Mutex::new(Vec::new())),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, Mutex<Vec<String>>> for Daemon {
    fn event(
        _: &mut Self,
        _: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        mimes: &Mutex<Vec<String>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Ok(mut mimes) = mimes.lock()
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for Daemon {
    fn event(
        daemon: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                let Some((_, formats)) = daemon.serving.as_ref().filter(|(ours, _)| ours == source)
                else {
                    return;
                };
                let formats = Arc::clone(formats);
                let _ = thread::Builder::new()
                    .name("enso-clip-send".to_owned())
                    .spawn(move || {
                        if let Some((_, bytes)) =
                            formats.iter().find(|(mime, _)| *mime == mime_type)
                        {
                            let _ = std::fs::File::from(fd).write_all(bytes);
                        }
                    });
            }
            ext_data_control_source_v1::Event::Cancelled => {
                if daemon
                    .serving
                    .as_ref()
                    .is_some_and(|(ours, _)| ours == source)
                {
                    daemon.serving = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

macro_rules! ignore {
    ($($object:ty, $data:ty);* $(;)?) => {$(
        impl Dispatch<$object, $data> for Daemon {
            fn event(
                _: &mut Self,
                _: &$object,
                _: <$object as Proxy>::Event,
                _: &$data,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore! {
    WlRegistry, GlobalListContents;
    WlSeat, ();
    ExtDataControlManagerV1, ();
}
