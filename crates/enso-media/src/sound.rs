//! A file's sound, heard: decoded by ffmpeg from a point on, played by
//! PipeWire through its PulseAudio socket. The server asks for sound as it
//! plays it and says how far it has got, so the clock is set from here —
//! by what is heard, not by what was sent.
//!
//! One stream from one point: seeking starts another. Two threads, one
//! reading what the server says and one doing the rest, so neither waits
//! on the other.

use std::{
    ffi::CStr,
    io::{BufReader, Cursor, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::ChildStdout,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};

use pulseaudio::protocol::{
    self, ChannelMap, ChannelVolume, Command, CorkStreamParams, LatencyParams,
    PlaybackStreamParams, Prop, Props, SampleFormat, SampleSpec, SetStreamMuteParams,
};

use crate::{Error, clock::Shared, ffmpeg};

/// What it's called on the server: what the volume panel lists.
const NAME: &CStr = c"enso";
/// The sound as sent: 48 kHz, two channels, 16 bits each.
const RATE: u32 = 48_000;
const CHANNELS: u8 = 2;
const BYTES_PER_SECOND: u32 = RATE * CHANNELS as u32 * 2;
/// How much the server keeps ahead: a quarter of a second.
const BUFFER: u32 = BYTES_PER_SECOND / 4;
/// How often to ask how far it has got, while it plays.
const ASK_EVERY: Duration = Duration::from_millis(100);

/// Tags for the replies that need telling apart.
const LATENCY: u32 = 1_000_000;
const DRAINED: u32 = 1_000_001;

#[derive(Debug)]
enum Event {
    /// The server said something.
    Said(u32, Box<Command>),
    /// The server says how far it has got.
    Latency(protocol::PlaybackLatency),
    Pause,
    Resume,
    Mute(bool),
    /// Whoever played it is done with it.
    Stop,
    /// The server went away.
    Gone,
}

/// Sound playing, or ready to; stopped when this goes.
#[derive(Debug)]
pub(crate) struct Sound {
    events: Sender<Event>,
}

impl Sound {
    /// Play `path`'s sound from `from` (or be ready to, if not
    /// `playing`), setting `timing`'s clock as it's heard.
    pub fn start(
        path: &Path,
        from: Duration,
        (playing, muted, video): (bool, bool, bool),
        timing: Shared,
    ) -> Result<Self, Error> {
        let mut feed = ffmpeg::spawn(
            "ffmpeg",
            Vec::<std::ffi::OsString>::from([
                "-ss".into(),
                ffmpeg::seconds(from).into(),
                "-i".into(),
                ffmpeg::input(path),
                "-map".into(),
                "0:a:0".into(),
                "-vn".into(),
                "-sn".into(),
                "-dn".into(),
                "-f".into(),
                "s16le".into(),
                "-ac".into(),
                CHANNELS.to_string().into(),
                "-ar".into(),
                RATE.to_string().into(),
                "-".into(),
            ]),
        )?;
        let out = feed
            .0
            .stdout
            .take()
            .ok_or_else(|| Error::new("ffmpeg gave nothing to read"))?;
        let mut link = Link::open()?;
        let reply: protocol::CreatePlaybackStreamReply =
            link.ask(Command::CreatePlaybackStream(PlaybackStreamParams {
                sample_spec: SampleSpec {
                    format: SampleFormat::S16Le,
                    channels: CHANNELS,
                    sample_rate: RATE,
                },
                channel_map: ChannelMap::stereo(),
                cvolume: Some(ChannelVolume::norm(CHANNELS)),
                sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
                buffer_attr: protocol::stream::BufferAttr {
                    max_length: u32::MAX,
                    target_length: BUFFER,
                    pre_buffering: u32::MAX,
                    minimum_request_length: u32::MAX,
                    fragment_size: u32::MAX,
                },
                flags: protocol::stream::StreamFlags {
                    start_corked: !playing,
                    ..Default::default()
                },
                props: {
                    let mut props = Props::new();
                    props.set(Prop::MediaRole, if video { c"video" } else { c"music" });
                    props
                },
                ..Default::default()
            }))?;
        let (events, inbox) = mpsc::channel();
        let reader = link.socket.get_ref().try_clone().map_err(io)?;
        let said = events.clone();
        let version = link.version;
        thread::Builder::new()
            .name("enso-media-heard".to_owned())
            .spawn(move || listen(reader, version, &said))
            .map_err(io)?;
        let stream = Stream {
            link,
            channel: reply.channel,
            index: reply.stream_index,
            out,
            _feed: feed,
            from,
            fed_all: false,
            playing,
            asked: None,
            timing,
            buffer: Vec::new(),
        };
        thread::Builder::new()
            .name("enso-media-sound".to_owned())
            .spawn(move || stream.run(reply.requested_bytes, muted, &inbox))
            .map_err(io)?;
        Ok(Self { events })
    }

    pub fn pause(&self) {
        let _ = self.events.send(Event::Pause);
    }

    pub fn resume(&self) {
        let _ = self.events.send(Event::Resume);
    }

    pub fn mute(&self, muted: bool) {
        let _ = self.events.send(Event::Mute(muted));
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
    }
}

fn io(err: std::io::Error) -> Error {
    Error::new(format!("the sound server: {err}"))
}

impl From<protocol::ProtocolError> for Error {
    fn from(err: protocol::ProtocolError) -> Self {
        Self::new(format!("the sound server: {err}"))
    }
}

/// Read what the server says, a whole message at a time, until it goes.
fn listen(socket: UnixStream, version: u16, said: &Sender<Event>) {
    let mut socket = BufReader::new(socket);
    let mut header = [0u8; protocol::DESCRIPTOR_SIZE];
    loop {
        if socket.read_exact(&mut header).is_err() {
            break;
        }
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let channel = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        let mut message = header.to_vec();
        message.resize(header.len() + length as usize, 0);
        if socket.read_exact(&mut message[header.len()..]).is_err() {
            break;
        }
        // Sound sent to us is for recording; there's none of that here.
        if channel != u32::MAX {
            continue;
        }
        let mut message = Cursor::new(message);
        let event = match protocol::read_command_message(&mut message, version) {
            Ok((LATENCY, Command::Reply)) => {
                let mut reader = protocol::TagStructReader::new(&mut message, version);
                match reader.read::<protocol::PlaybackLatency>() {
                    Ok(latency) => Event::Latency(latency),
                    Err(_) => continue,
                }
            }
            Ok((seq, command)) => Event::Said(seq, Box::new(command)),
            Err(_) => continue,
        };
        if said.send(event).is_err() {
            return;
        }
    }
    let _ = said.send(Event::Gone);
}

/// The stream, from the side that feeds it.
struct Stream {
    link: Link,
    channel: u32,
    /// Its index among the server's streams, to mute it by.
    index: u32,
    out: ChildStdout,
    _feed: ffmpeg::Child,
    from: Duration,
    fed_all: bool,
    playing: bool,
    /// When it was last asked how far it has got.
    asked: Option<Instant>,
    timing: Shared,
    buffer: Vec<u8>,
}

impl Stream {
    fn run(mut self, first: u32, muted: bool, inbox: &Receiver<Event>) {
        if muted {
            self.mute(true);
        }
        if self.feed(first).is_err() {
            return self.lost();
        }
        for event in inbox {
            let done = match event {
                Event::Said(seq, said) => match *said {
                    Command::Request(request) if request.channel == self.channel => {
                        self.feed(request.length).and_then(|()| self.ask())
                    }
                    Command::Reply if seq == DRAINED => {
                        self.timing.lock().sound_ended = true;
                        Ok(())
                    }
                    Command::Started(_) => self.ask(),
                    _ => Ok(()),
                },
                Event::Latency(latency) => {
                    self.heard(&latency);
                    Ok(())
                }
                Event::Pause => {
                    self.playing = false;
                    self.cork(true)
                }
                Event::Resume => {
                    self.playing = true;
                    self.asked = None;
                    self.cork(false).and_then(|()| self.ask())
                }
                Event::Mute(muted) => {
                    self.mute(muted);
                    Ok(())
                }
                Event::Stop => break,
                Event::Gone => Err(Error::new("the sound server went away")),
            };
            if done.is_err() {
                return self.lost();
            }
        }
        let _ = self.link.send(&Command::DeletePlaybackStream(self.channel));
        self.hang_up();
    }

    /// Close the connection, so the thread listening to it ends too.
    fn hang_up(&self) {
        let _ = self
            .link
            .socket
            .get_ref()
            .shutdown(std::net::Shutdown::Both);
    }

    /// Send up to `length` bytes of sound; at its end, let the server play
    /// out what it has and say so.
    fn feed(&mut self, length: u32) -> Result<(), Error> {
        if self.fed_all {
            return Ok(());
        }
        self.buffer.resize(length as usize, 0);
        let mut filled = 0;
        while filled < self.buffer.len() {
            match self.out.read(&mut self.buffer[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(read) => filled += read,
            }
        }
        // Whole samples only.
        let filled = filled - filled % 4;
        if filled > 0 {
            protocol::write_memblock(
                self.link.socket.get_mut(),
                self.channel,
                &self.buffer[..filled],
                0,
            )?;
        }
        if filled < self.buffer.len() {
            self.fed_all = true;
            self.link
                .send_as(DRAINED, &Command::DrainPlaybackStream(self.channel))?;
        }
        Ok(())
    }

    /// Ask how far it has got, now and then.
    fn ask(&mut self) -> Result<(), Error> {
        if self.asked.is_some_and(|asked| asked.elapsed() < ASK_EVERY) {
            return Ok(());
        }
        self.asked = Some(Instant::now());
        self.link.send_as(
            LATENCY,
            &Command::GetPlaybackLatency(LatencyParams {
                channel: self.channel,
                now: SystemTime::now(),
            }),
        )
    }

    /// The server said how far it has got: set the clock by it.
    fn heard(&self, latency: &protocol::PlaybackLatency) {
        if !self.playing || !latency.playing {
            return;
        }
        let played = latency.read_offset.max(0) as f64 / f64::from(BYTES_PER_SECOND);
        let behind = latency.sink_usec as f64 / 1_000_000.0;
        let at = self.from + Duration::from_secs_f64((played - behind).max(0.0));
        self.timing.lock().clock.heard(at, Instant::now());
    }

    fn cork(&mut self, cork: bool) -> Result<(), Error> {
        self.link
            .send(&Command::CorkPlaybackStream(CorkStreamParams {
                channel: self.channel,
                cork,
            }))
    }

    fn mute(&mut self, mute: bool) {
        let _ = self
            .link
            .send(&Command::SetSinkInputMute(SetStreamMuteParams {
                index: self.index,
                mute,
            }));
    }

    /// The server went away: the clock goes on without it.
    fn lost(&self) {
        self.hang_up();
        let mut timing = self.timing.lock();
        timing.sound_lost = true;
        timing.clock.run(self.playing, Instant::now());
    }
}

/// The connection to the sound server.
struct Link {
    socket: BufReader<UnixStream>,
    version: u16,
    sequence: u32,
}

impl Link {
    fn open() -> Result<Self, Error> {
        let path = pulseaudio::socket_path_from_env()
            .ok_or_else(|| Error::new("no sound server in this session"))?;
        let socket = UnixStream::connect(&path).map_err(io)?;
        let mut link = Self {
            socket: BufReader::new(socket),
            version: protocol::MAX_VERSION,
            sequence: 0,
        };
        // PipeWire lets any local client in, but the handshake still wants
        // a cookie of the usual size; without a cookie file, any will do.
        const COOKIE_LENGTH: usize = 256;
        let cookie = pulseaudio::cookie_path_from_env()
            .and_then(|path| std::fs::read(path).ok())
            .filter(|cookie| cookie.len() == COOKIE_LENGTH)
            .unwrap_or_else(|| vec![0; COOKIE_LENGTH]);
        let auth: protocol::AuthReply = link.ask(Command::Auth(protocol::AuthParams {
            version: protocol::MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        }))?;
        link.version = auth.version.min(protocol::MAX_VERSION);
        let mut props = Props::new();
        props.set(Prop::ApplicationName, NAME);
        props.set(Prop::ApplicationIconName, c"multimedia-player");
        let _: protocol::SetClientNameReply = link.ask(Command::SetClientName(props))?;
        Ok(link)
    }

    fn send(&mut self, command: &Command) -> Result<(), Error> {
        // Low tags for what's only acknowledged; the high ones are named.
        self.sequence = (self.sequence + 1) % LATENCY;
        let seq = self.sequence;
        self.send_as(seq, command)
    }

    fn send_as(&mut self, seq: u32, command: &Command) -> Result<(), Error> {
        protocol::write_command_message(self.socket.get_mut(), seq, command, self.version)?;
        self.socket.get_mut().flush().map_err(io)
    }

    fn ask<R: protocol::CommandReply>(&mut self, command: Command) -> Result<R, Error> {
        self.send(&command)?;
        let (_, reply) = protocol::read_reply_message(&mut self.socket, self.version)?;
        Ok(reply)
    }
}
