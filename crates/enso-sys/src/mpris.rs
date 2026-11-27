//! What's playing, through MPRIS: every media player on the session bus —
//! a browser tab playing YouTube, Spotify, mpv — with its track, where it is
//! in it, and the buttons it offers.
//!
//! Players come and go as they please (a tab closes, a video ends), so
//! nothing here holds on to one: [`Mpris::players`] is a complete snapshot,
//! and every action names its player by bus name. Positions are not sent as
//! they change — a player only says where it is when asked, or when it jumps
//! ([`Seeked`]) — so a [`Player`] keeps when it was read, and
//! [`Player::position_at`] counts on from there.
//!
//! [`Seeked`]: https://specifications.freedesktop.org/mpris-spec/latest/Player_Interface.html#Signal:Seeked

use std::{
    future::poll_fn,
    pin::Pin,
    task::Poll,
    thread,
    time::{Duration, Instant},
};

use zbus::{
    Connection, MatchRule, MessageStream,
    export::futures_core::Stream,
    message::Type,
    wire::{Dict, ObjectPath, Variant},
};

use crate::bus::{Error, Props, Result, prop, string};

/// Every player's bus name starts with this.
const PREFIX: &str = "org.mpris.MediaPlayer2.";
/// The one object a player exports.
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Whether a player is playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    // In the order players are offered: what's playing comes first.
    Playing,
    Paused,
    Stopped,
}

/// What a player is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// The player's name for the track, needed to move within it.
    pub id: Option<String>,
    pub title: String,
    /// Artists, joined: for a video, the channel.
    pub artist: String,
    pub album: String,
    pub length: Option<Duration>,
    /// The cover: a `file://` path, or an `https://` address.
    pub art: Option<String>,
}

/// One media player, as it was when read.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    /// Its bus name, which every action takes.
    pub bus: String,
    /// What to call it: "Firefox", "Spotify".
    pub name: String,
    pub status: Status,
    pub track: Track,
    /// Where it was in the track at [`read`](Self::read).
    pub position: Duration,
    pub read: Instant,
    /// How fast it plays: 1.0 is normal.
    pub rate: f64,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub can_seek: bool,
}

impl Player {
    /// Where it is in the track at `now`: counted on from the last reading
    /// while it plays, and never past the end.
    pub fn position_at(&self, now: Instant) -> Duration {
        let mut position = self.position;
        if self.status == Status::Playing {
            let elapsed = now.saturating_duration_since(self.read).as_secs_f64();
            position += Duration::from_secs_f64(elapsed * self.rate.max(0.0));
        }
        match self.track.length {
            Some(length) => position.min(length),
            None => position,
        }
    }

    /// Whether it can be moved to another point in the track.
    pub fn seekable(&self) -> bool {
        self.can_seek && self.track.length.is_some_and(|length| !length.is_zero())
    }
}

/// A connection to the session bus, for the players on it.
#[derive(Clone, Debug)]
pub struct Mpris {
    conn: Connection,
}

impl Mpris {
    pub fn connect() -> Result<Self> {
        let conn = zbus::block_on(Connection::session())
            .map_err(|err| Error::new(format!("cannot reach the session bus: {err}")))?;
        Ok(Self { conn })
    }

    /// Every player there is, the ones playing first, then the paused, then
    /// the rest; in the order they appeared within each. A player that
    /// leaves while it's being read is left out.
    pub fn players(&self) -> Result<Vec<Player>> {
        let names: Vec<String> = self.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ListNames",
            &(),
        )?;
        let mut players: Vec<Player> = names
            .into_iter()
            .filter(|name| name.starts_with(PREFIX))
            .filter_map(|bus| self.player(bus).ok())
            .collect();
        players.sort_by_key(|player| player.status);
        Ok(players)
    }

    fn player(&self, bus: String) -> Result<Player> {
        let read = Instant::now();
        let props: Props = self.call(&bus, PATH, PROPERTIES, "GetAll", &(PLAYER,))?;
        // The name is a nicety: a player without the root interface is
        // still a player.
        let root: Props = self
            .call(&bus, PATH, PROPERTIES, "GetAll", &(ROOT,))
            .unwrap_or_default();
        let status = match prop::<&str>(&props, "PlaybackStatus") {
            Some("Playing") => Status::Playing,
            Some("Paused") => Status::Paused,
            _ => Status::Stopped,
        };
        let position = prop::<i64>(&props, "Position").unwrap_or(0);
        // Some players report a rate of 0 while paused, or none at all.
        let rate = prop::<f64>(&props, "Rate").filter(|rate| *rate > 0.0);
        let track = props
            .get("Metadata")
            .and_then(|value| <&Dict<'_, '_>>::try_from(value).ok())
            .map(track)
            .unwrap_or_default();
        Ok(Player {
            name: player_name(&bus, &root),
            bus,
            status,
            track,
            position: micros(position),
            read,
            rate: rate.unwrap_or(1.0),
            can_play: prop(&props, "CanPlay").unwrap_or(false),
            can_pause: prop(&props, "CanPause").unwrap_or(false),
            can_next: prop(&props, "CanGoNext").unwrap_or(false),
            can_previous: prop(&props, "CanGoPrevious").unwrap_or(false),
            can_seek: prop(&props, "CanSeek").unwrap_or(false),
        })
    }

    /// Play if paused, pause if playing.
    pub fn play_pause(&self, bus: &str) -> Result<()> {
        self.act(bus, "PlayPause", &())
    }

    pub fn play(&self, bus: &str) -> Result<()> {
        self.act(bus, "Play", &())
    }

    pub fn pause(&self, bus: &str) -> Result<()> {
        self.act(bus, "Pause", &())
    }

    pub fn next(&self, bus: &str) -> Result<()> {
        self.act(bus, "Next", &())
    }

    pub fn previous(&self, bus: &str) -> Result<()> {
        self.act(bus, "Previous", &())
    }

    /// Move `player` to `position` in its track: to the point itself when
    /// the track has an id to name, otherwise by the distance from where it
    /// was read to be.
    pub fn set_position(&self, player: &Player, position: Duration) -> Result<()> {
        let to = i64::try_from(position.as_micros()).unwrap_or(i64::MAX);
        if let Some(id) = &player.track.id
            && let Ok(id) = ObjectPath::try_from(id.as_str())
        {
            return self.act(&player.bus, "SetPosition", &(id, to));
        }
        let from = i64::try_from(player.position_at(Instant::now()).as_micros()).unwrap_or(0);
        self.act(&player.bus, "Seek", &(to - from,))
    }

    fn act<B>(&self, bus: &str, method: &str, body: &B) -> Result<()>
    where
        B: zbus::export::serde::Serialize + zbus::wire::DynamicType,
    {
        self.call(bus, PATH, PLAYER, method, body)
    }

    fn call<B, R>(
        &self,
        bus: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R>
    where
        B: zbus::export::serde::Serialize + zbus::wire::DynamicType,
        R: for<'d> zbus::export::serde::Deserialize<'d> + zbus::wire::Type,
    {
        zbus::block_on(async {
            let reply = self
                .conn
                .call_method(Some(bus), path, Some(interface), method, body)
                .await?;
            Ok(reply.body().deserialize::<R>()?)
        })
    }

    /// Block, calling `on_change` after each burst of news from the players
    /// — a property changing, a jump in the track, a player coming or going
    /// — until it returns `false`.
    pub fn watch(&self, mut on_change: impl FnMut() -> bool) -> Result<()> {
        const SETTLE: Duration = Duration::from_millis(60);
        let closed = || Error::new("the session bus connection closed");
        let (mut players, mut names) = zbus::block_on(async {
            // Anything the players' one object says: properties, Seeked.
            let players = MatchRule::builder()
                .msg_type(Type::Signal)
                .path(PATH)
                .build()?;
            let names = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender("org.freedesktop.DBus")
                .interface("org.freedesktop.DBus")
                .member("NameOwnerChanged")
                .arg0ns(ROOT)
                .build()?;
            zbus::Result::Ok((
                MessageStream::for_match_rule(players, &self.conn, None).await?,
                MessageStream::for_match_rule(names, &self.conn, None).await?,
            ))
        })?;
        // Ready when either stream has something; `Err` once one has ended.
        let mut next = |drain: bool| {
            zbus::block_on(poll_fn(|cx| {
                let mut heard = false;
                for stream in [&mut players, &mut names] {
                    loop {
                        match Pin::new(&mut *stream).poll_next(cx) {
                            Poll::Ready(Some(_)) => heard = true,
                            Poll::Ready(None) => return Poll::Ready(Err(closed())),
                            Poll::Pending => break,
                        }
                    }
                }
                if heard || drain {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }))
        };
        loop {
            next(false)?;
            // Let the rest of the burst arrive, then set it aside unread.
            thread::sleep(SETTLE);
            next(true)?;
            if !on_change() {
                return Ok(());
            }
        }
    }
}

/// A player's own name for itself, from its desktop entry ("firefox" →
/// "Firefox") or its identity, or else its bus name. The suite's own
/// players (`enso-view`, `enso-listen`) go by their identities, the
/// names their windows have: Viewer, Listen.
fn player_name(bus: &str, root: &Props) -> String {
    let own = bus
        .strip_prefix(PREFIX)
        .is_some_and(|rest| rest.starts_with("enso_"));
    if own
        && let Some(identity) = string(root, "Identity").filter(|identity| !identity.is_empty())
    {
        return identity;
    }
    if let Some(entry) = string(root, "DesktopEntry").filter(|entry| !entry.is_empty()) {
        // `org.mozilla.firefox` → `firefox`.
        let last = entry.rsplit('.').next().unwrap_or(&entry);
        return capitalised(last);
    }
    if let Some(identity) = string(root, "Identity").filter(|identity| !identity.is_empty()) {
        return identity;
    }
    let rest = bus.strip_prefix(PREFIX).unwrap_or(bus);
    capitalised(rest.split('.').next().unwrap_or(rest))
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The track, from a player's `Metadata`.
fn track(metadata: &Dict<'_, '_>) -> Track {
    let mut track = Track::default();
    for (key, value) in metadata.iter() {
        let Variant::Str(key) = key else { continue };
        let value = plain(value);
        match &**key {
            "mpris:trackid" => track.id = text(value),
            "xesam:title" => track.title = text(value).unwrap_or_default(),
            "xesam:artist" => track.artist = texts(value).join(", "),
            "xesam:album" => track.album = text(value).unwrap_or_default(),
            "mpris:artUrl" => track.art = text(value).filter(|url| !url.is_empty()),
            "mpris:length" => {
                track.length = match value {
                    Variant::I64(us) => Some(micros(*us)),
                    Variant::U64(us) => Some(Duration::from_micros(*us)),
                    Variant::I32(us) => Some(micros((*us).into())),
                    Variant::U32(us) => Some(Duration::from_micros((*us).into())),
                    _ => None,
                }
                .filter(|length| !length.is_zero());
            }
            _ => {}
        }
    }
    track
}

/// The value inside however many variants it's wrapped in.
fn plain<'a>(mut value: &'a Variant<'a>) -> &'a Variant<'a> {
    while let Variant::Variant(inner) = value {
        value = inner;
    }
    value
}

/// A string, or an object path, as text.
fn text(value: &Variant<'_>) -> Option<String> {
    match plain(value) {
        Variant::Str(s) => Some(s.to_string()),
        Variant::ObjectPath(path) => Some(path.as_str().to_owned()),
        _ => None,
    }
}

/// A list of strings — or the one string some players send instead.
fn texts(value: &Variant<'_>) -> Vec<String> {
    match plain(value) {
        Variant::Array(items) => items
            .inner()
            .iter()
            .filter_map(text)
            .filter(|s| !s.is_empty())
            .collect(),
        other => text(other).into_iter().filter(|s| !s.is_empty()).collect(),
    }
}

/// Microseconds, as MPRIS counts, never below zero.
fn micros(us: i64) -> Duration {
    Duration::from_micros(us.max(0).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(status: Status, position: u64, length: Option<u64>) -> Player {
        Player {
            bus: format!("{PREFIX}test"),
            name: "Test".into(),
            status,
            track: Track {
                length: length.map(Duration::from_secs),
                ..Track::default()
            },
            position: Duration::from_secs(position),
            read: Instant::now(),
            rate: 1.0,
            can_play: true,
            can_pause: true,
            can_next: false,
            can_previous: false,
            can_seek: true,
        }
    }

    #[test]
    fn a_playing_track_counts_on_and_stops_at_its_end() {
        let playing = player(Status::Playing, 10, Some(12));
        let later = playing.read + Duration::from_secs(1);
        assert_eq!(playing.position_at(later), Duration::from_secs(11));
        assert_eq!(
            playing.position_at(later + Duration::from_secs(5)),
            Duration::from_secs(12)
        );
        // Paused, it stays where it was.
        let paused = player(Status::Paused, 10, Some(12));
        assert_eq!(
            paused.position_at(paused.read + Duration::from_secs(3)),
            Duration::from_secs(10)
        );
        // A live stream has no length to move within.
        assert!(!player(Status::Playing, 0, None).seekable());
        assert!(paused.seekable());
    }

    #[test]
    fn players_are_offered_playing_first() {
        let mut statuses = [Status::Stopped, Status::Paused, Status::Playing];
        statuses.sort();
        assert_eq!(statuses, [Status::Playing, Status::Paused, Status::Stopped]);
    }

    #[test]
    fn players_are_named_as_people_know_them() {
        let none = Props::new();
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.firefox.instance_1_178", &none),
            "Firefox"
        );
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.spotify", &none),
            "Spotify"
        );
        assert_eq!(
            player_name("org.mpris.MediaPlayer2.enso_listen.instance42", &none),
            "Enso_listen"
        );
    }

    #[test]
    fn metadata_is_read_whatever_it_is_wrapped_in() {
        let mut dict = Dict::new(
            <&str as zbus::wire::Type>::SIGNATURE,
            <Variant<'_> as zbus::wire::Type>::SIGNATURE,
        );
        let wrapped = |v: Variant<'static>| Variant::Variant(Box::new(v));
        dict.add("xesam:title", wrapped(Variant::from("Lofi beats")))
            .unwrap();
        dict.add("xesam:artist", wrapped(Variant::from(vec!["Chill", "Hop"])))
            .unwrap();
        dict.add("mpris:length", wrapped(Variant::from(2_067_000_000_i64)))
            .unwrap();
        dict.add(
            "mpris:artUrl",
            wrapped(Variant::from("file:///tmp/cover.png")),
        )
        .unwrap();
        let track = track(&dict);
        assert_eq!(track.title, "Lofi beats");
        assert_eq!(track.artist, "Chill, Hop");
        assert_eq!(track.length, Some(Duration::from_secs(2067)));
        assert_eq!(track.art.as_deref(), Some("file:///tmp/cover.png"));
    }
}
