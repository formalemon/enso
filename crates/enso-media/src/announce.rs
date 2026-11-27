//! Telling the desktop what's playing, and taking its buttons: an MPRIS
//! player on the session bus, as every media player has. The now-playing
//! card under the bar's clock shows it among the others, and the media
//! keys reach it.
//!
//! The bus is served from a thread of its own; [`Announcer::tell`] hands it
//! what's playing whenever that changes, and what the desktop asks for
//! comes back through the function given at the start, on that thread.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread,
    time::{Duration, Instant},
};

use zbus::{
    ObjectPath, OwnedVariant, Variant, connection, interface, object_server::SignalEmitter,
};

use crate::Error;

const PATH: &str = "/org/mpris/MediaPlayer2";

/// Whether it plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// What's playing, as told to the desktop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playing {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The cover, as a `file://` address.
    pub art: Option<String>,
    pub length: Option<Duration>,
    /// Where it is, as of the telling.
    pub position: Duration,
    pub status: Status,
    pub can_next: bool,
    pub can_previous: bool,
    /// Which track: another number for another one.
    pub track: u64,
}

/// What the desktop asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Move by this many microseconds (back, if fewer than none).
    Seek(i64),
    /// Go to here.
    SetPosition(Duration),
    /// Come to the front.
    Raise,
    Quit,
}

type Asker = Arc<dyn Fn(Asked) + Send + Sync>;

/// What's told, and when, so the position can be counted on.
#[derive(Clone, Debug)]
struct Told {
    playing: Playing,
    at: Instant,
}

impl Told {
    fn position(&self) -> Duration {
        let on = if self.playing.status == Status::Playing {
            self.at.elapsed()
        } else {
            Duration::ZERO
        };
        let at = self.playing.position + on;
        self.playing.length.map_or(at, |length| at.min(length))
    }

    fn track_id(&self) -> String {
        format!("/org/enso/track/{}", self.playing.track)
    }
}

type State = Arc<Mutex<Told>>;

fn lock(state: &State) -> std::sync::MutexGuard<'_, Told> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Update {
    Playing(Playing),
    Seeked(Duration),
}

/// A player on the bus, until this goes.
#[derive(Debug)]
pub struct Announcer {
    updates: mpsc::Sender<Update>,
}

impl Announcer {
    /// Become `org.mpris.MediaPlayer2.<name>.instance<pid>`, called
    /// `identity` (as the card's byline says it), of `desktop`'s entry.
    /// `asked` is called with what the desktop asks for.
    pub fn start(
        name: &str,
        identity: &str,
        desktop: &str,
        asked: impl Fn(Asked) + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let bus = format!(
            "org.mpris.MediaPlayer2.{name}.instance{}",
            std::process::id()
        );
        let state: State = Arc::new(Mutex::new(Told {
            playing: Playing::default(),
            at: Instant::now(),
        }));
        let asked: Asker = Arc::new(asked);
        let root = Root {
            identity: identity.to_owned(),
            desktop: desktop.to_owned(),
            asked: Arc::clone(&asked),
        };
        let player = Player {
            state: Arc::clone(&state),
            asked,
        };
        let (updates, inbox) = mpsc::channel();
        let (started, start) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("enso-media-mpris".to_owned())
            .spawn(move || {
                // Built inside `block_on`, then left: with no thread inside
                // it, zbus's own helper drives the connection, answering
                // the desktop while this waits for news.
                let built = zbus::block_on(
                    connection::Builder::session()
                        .name(bus.clone())
                        .serve_at(PATH, root)
                        .serve_at(PATH, player)
                        .build(),
                );
                let connection = match built {
                    Ok(connection) => {
                        let _ = started.send(Ok(()));
                        connection
                    }
                    Err(err) => {
                        let _ = started.send(Err(err.to_string()));
                        return;
                    }
                };
                for update in inbox {
                    let _ = zbus::block_on(async {
                        let player = connection
                            .object_server()
                            .interface::<_, Player>(PATH)
                            .await?;
                        let emitter = player.signal_emitter();
                        match update {
                            Update::Playing(playing) => {
                                let before = {
                                    let mut told = lock(&state);
                                    let before = told.playing.clone();
                                    *told = Told {
                                        playing: playing.clone(),
                                        at: Instant::now(),
                                    };
                                    before
                                };
                                changed(&*player.get().await, emitter, &before, &playing).await
                            }
                            Update::Seeked(at) => Player::seeked(emitter, micros(at)).await,
                        }
                    });
                }
                // Done with: off the bus, so the desktop forgets the player.
                let _ = zbus::block_on(connection.release_name(bus.as_str()));
            })
            .map_err(|err| Error::new(format!("cannot announce: {err}")))?;
        match start.recv() {
            Ok(Ok(())) => Ok(Self { updates }),
            Ok(Err(err)) => Err(Error::new(format!("cannot announce: {err}"))),
            Err(_) => Err(Error::new("cannot announce")),
        }
    }

    /// What's playing now.
    pub fn tell(&self, playing: Playing) {
        let _ = self.updates.send(Update::Playing(playing));
    }

    /// It jumped to `at`.
    pub fn seeked(&self, at: Duration) {
        let _ = self.updates.send(Update::Seeked(at));
    }
}

/// Say which properties changed.
async fn changed(
    player: &Player,
    emitter: &SignalEmitter<'static>,
    before: &Playing,
    now: &Playing,
) -> zbus::Result<()> {
    if before.status != now.status {
        player.playback_status_changed(emitter).await?;
    }
    if before.title != now.title
        || before.artist != now.artist
        || before.album != now.album
        || before.art != now.art
        || before.length != now.length
        || before.track != now.track
    {
        player.metadata_changed(emitter).await?;
        player.can_seek_changed(emitter).await?;
    }
    if before.can_next != now.can_next {
        player.can_go_next_changed(emitter).await?;
    }
    if before.can_previous != now.can_previous {
        player.can_go_previous_changed(emitter).await?;
    }
    Ok(())
}

fn micros(at: Duration) -> i64 {
    i64::try_from(at.as_micros()).unwrap_or(i64::MAX)
}

struct Root {
    identity: String,
    desktop: String,
    asked: Asker,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        (self.asked)(Asked::Raise);
    }

    fn quit(&self) {
        (self.asked)(Asked::Quit);
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn identity(&self) -> String {
        self.identity.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn desktop_entry(&self) -> String {
        self.desktop.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".to_owned()]
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    state: State,
    asked: Asker,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        (self.asked)(Asked::Next);
    }

    fn previous(&self) {
        (self.asked)(Asked::Previous);
    }

    fn pause(&self) {
        (self.asked)(Asked::Pause);
    }

    fn play_pause(&self) {
        (self.asked)(Asked::PlayPause);
    }

    fn stop(&self) {
        (self.asked)(Asked::Stop);
    }

    fn play(&self) {
        (self.asked)(Asked::Play);
    }

    fn seek(&self, offset: i64) {
        (self.asked)(Asked::Seek(offset));
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        // Only for the track it's on, as the specification says.
        if track_id.as_str() == lock(&self.state).track_id() && position >= 0 {
            (self.asked)(Asked::SetPosition(Duration::from_micros(position as u64)));
        }
    }

    fn open_uri(&self, _uri: &str) {}

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match lock(&self.state).playing.status {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
        .to_owned()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        let told = lock(&self.state).clone();
        let playing = &told.playing;
        let mut map = HashMap::new();
        let mut put = |key: &str, value: Variant<'_>| {
            if let Ok(value) = OwnedVariant::try_from(value) {
                map.insert(key.to_owned(), value);
            }
        };
        if let Ok(path) = ObjectPath::try_from(told.track_id()) {
            put("mpris:trackid", Variant::from(path));
        }
        put("xesam:title", Variant::from(playing.title.clone()));
        if let Some(artist) = &playing.artist {
            put("xesam:artist", Variant::from(vec![artist.clone()]));
        }
        if let Some(album) = &playing.album {
            put("xesam:album", Variant::from(album.clone()));
        }
        if let Some(art) = &playing.art {
            put("mpris:artUrl", Variant::from(art.clone()));
        }
        if let Some(length) = playing.length {
            put("mpris:length", Variant::from(micros(length)));
        }
        map
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        micros(lock(&self.state).position())
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn volume(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        lock(&self.state).playing.can_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        lock(&self.state).playing.can_previous
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        lock(&self.state).playing.length.is_some()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}
