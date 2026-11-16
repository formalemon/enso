//! Volume through PipeWire.
//!
//! This speaks the PulseAudio wire protocol to the socket PipeWire serves it
//! on (`pipewire-pulse`, part of every PipeWire desktop). That protocol has
//! exactly the model a mixer wants — an output, the applications playing to
//! it, a volume and a mute for each — and PipeWire does the work of mapping
//! it onto devices and routes. The protocol itself is pure Rust.
//!
//! The same goes for the other direction: the microphones there are, which
//! one is in use, its level and whether it is muted.
//!
//! [`Mixer::state`] is a complete snapshot; take it again whenever
//! [`Mixer::watch`] reports a change.

use std::{
    ffi::CStr,
    fmt,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use pulseaudio::protocol::{
    self, ChannelVolume, Command, CommandReply, Prop, ProtocolError, SubscriptionMask, Volume,
};

/// What we call ourselves to the sound server.
const CLIENT_NAME: &CStr = c"enso";

/// The sound server said no, or could not be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<ProtocolError> for Error {
    fn from(err: ProtocolError) -> Self {
        Self(err.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The device in use for one direction — the output sound goes to, or the
/// microphone it comes from — and its level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// What the device calls itself, e.g. `Built-in Audio Analog Stereo`.
    pub name: String,
    /// Volume in percent. `100` is full; a server may report more.
    pub volume: u8,
    pub muted: bool,
}

/// The microphone in use and its level. Same shape as an [`Output`].
pub type Input = Output;

/// A device that could be the output, or the microphone: speakers, a
/// Bluetooth headset, a webcam's microphone, …
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Opaque handle for [`Mixer::set_output`] and [`Mixer::set_input`].
    pub id: String,
    /// What the device calls itself.
    pub name: String,
    /// It is the one in use.
    pub current: bool,
}

/// An application that is playing (or ready to play) sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// The application's name; also its handle for [`Mixer::set_app_volume`].
    /// All of an application's streams — every tab of a browser — are one
    /// `App`.
    pub name: String,
    /// Volume in percent, relative to the output's.
    pub volume: u8,
    pub muted: bool,
}

/// Everything about sound at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sound {
    /// The output in use, if there is one.
    pub output: Option<Output>,
    /// Every output there is, by name.
    pub outputs: Vec<Device>,
    /// Applications with a stream open, by name.
    pub apps: Vec<App>,
    /// The microphone in use, if there is one.
    pub input: Option<Input>,
    /// Every microphone there is, by name.
    pub inputs: Vec<Device>,
}

/// A connection to the sound server.
#[derive(Clone, Debug)]
pub struct Mixer {
    link: Arc<Mutex<Link>>,
}

impl Mixer {
    /// Connect to the sound server of this session.
    pub fn connect() -> Result<Self> {
        Ok(Self {
            link: Arc::new(Mutex::new(Link::open()?)),
        })
    }

    fn link(&self) -> std::sync::MutexGuard<'_, Link> {
        // A panic mid-exchange leaves nothing worth protecting.
        self.link.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current state.
    pub fn state(&self) -> Result<Sound> {
        let mut link = self.link();
        let server: protocol::ServerInfo = link.ask(Command::GetServerInfo)?;
        let sinks: protocol::SinkInfoList = link.ask(Command::GetSinkInfoList)?;
        // Every output has a "monitor" source that records what it plays;
        // that is not a microphone.
        let sources: Vec<protocol::SourceInfo> = link
            .ask::<protocol::SourceInfoList>(Command::GetSourceInfoList)?
            .into_iter()
            .filter(|source| source.monitor_of_sink_index.is_none())
            .collect();

        let label = |description: &Option<std::ffi::CString>, name: &CStr| {
            description
                .as_deref()
                .unwrap_or(name)
                .to_string_lossy()
                .into_owned()
        };
        let in_use =
            |name: &CStr, default: &Option<std::ffi::CString>| default.as_deref() == Some(name);
        let mut outputs = Vec::new();
        let mut output = None;
        for sink in &sinks {
            let current = in_use(&sink.name, &server.default_sink_name);
            let name = label(&sink.description, &sink.name);
            if current {
                output = Some(Output {
                    name: name.clone(),
                    volume: percent(&sink.cvolume),
                    muted: sink.muted,
                });
            }
            outputs.push(Device {
                id: sink.name.to_string_lossy().into_owned(),
                name,
                current,
            });
        }
        let mut inputs = Vec::new();
        let mut input = None;
        for source in &sources {
            let current = in_use(&source.name, &server.default_source_name);
            let name = label(&source.description, &source.name);
            if current {
                input = Some(Input {
                    name: name.clone(),
                    volume: percent(&source.cvolume),
                    muted: source.muted,
                });
            }
            inputs.push(Device {
                id: source.name.to_string_lossy().into_owned(),
                name,
                current,
            });
        }
        outputs.sort_by_key(|device| device.name.to_lowercase());
        inputs.sort_by_key(|device| device.name.to_lowercase());

        let mut apps: Vec<App> = Vec::new();
        for input in link.playing()? {
            let name = app_name(&input);
            match apps.iter_mut().find(|app| app.name == name) {
                // Several streams of one application: the loudest speaks for
                // them, and they are muted only if all are.
                Some(app) => {
                    app.volume = app.volume.max(percent(&input.cvolume));
                    app.muted &= input.muted;
                }
                None => apps.push(App {
                    name,
                    volume: percent(&input.cvolume),
                    muted: input.muted,
                }),
            }
        }
        apps.sort_by_key(|app| app.name.to_lowercase());
        Ok(Sound {
            output,
            outputs,
            apps,
            input,
            inputs,
        })
    }

    /// Make a device from [`Sound::outputs`] the output. What is playing
    /// moves over to it.
    pub fn set_output(&self, device: &str) -> Result<()> {
        self.link().tell(Command::SetDefaultSink(c_string(device)?))
    }

    /// Make a device from [`Sound::inputs`] the microphone.
    pub fn set_input(&self, device: &str) -> Result<()> {
        self.link()
            .tell(Command::SetDefaultSource(c_string(device)?))
    }

    /// Set the microphone's level, in percent.
    pub fn set_input_volume(&self, volume: u8) -> Result<()> {
        let mut link = self.link();
        let source = link.default_source()?.ok_or_else(no_input)?;
        link.tell(Command::SetSourceVolume(protocol::SetDeviceVolumeParams {
            device_index: Some(source.index),
            device_name: None,
            volume: scaled(&source.cvolume, volume),
        }))
    }

    /// Mute or unmute the microphone.
    pub fn set_input_muted(&self, muted: bool) -> Result<()> {
        let mut link = self.link();
        let source = link.default_source()?.ok_or_else(no_input)?;
        link.tell(Command::SetSourceMute(protocol::SetDeviceMuteParams {
            device_index: Some(source.index),
            device_name: None,
            mute: muted,
        }))
    }

    /// Set the output's volume, in percent.
    pub fn set_volume(&self, volume: u8) -> Result<()> {
        let mut link = self.link();
        let sink = link.default_sink()?.ok_or_else(no_output)?;
        link.tell(Command::SetSinkVolume(protocol::SetDeviceVolumeParams {
            device_index: Some(sink.index),
            device_name: None,
            volume: scaled(&sink.cvolume, volume),
        }))
    }

    /// Mute or unmute the output.
    pub fn set_muted(&self, muted: bool) -> Result<()> {
        let mut link = self.link();
        let sink = link.default_sink()?.ok_or_else(no_output)?;
        link.tell(Command::SetSinkMute(protocol::SetDeviceMuteParams {
            device_index: Some(sink.index),
            device_name: None,
            mute: muted,
        }))
    }

    /// Set the volume of every stream of an application, in percent.
    pub fn set_app_volume(&self, app: &str, volume: u8) -> Result<()> {
        let mut link = self.link();
        for input in link.streams_of(app)? {
            link.tell(Command::SetSinkInputVolume(
                protocol::SetStreamVolumeParams {
                    index: input.index,
                    volume: scaled(&input.cvolume, volume),
                },
            ))?;
        }
        Ok(())
    }

    /// Mute or unmute every stream of an application.
    pub fn set_app_muted(&self, app: &str, muted: bool) -> Result<()> {
        let mut link = self.link();
        for input in link.streams_of(app)? {
            link.tell(Command::SetSinkInputMute(protocol::SetStreamMuteParams {
                index: input.index,
                mute: muted,
            }))?;
        }
        Ok(())
    }

    /// Block, calling `on_change` whenever the sound state may have changed,
    /// until it returns `false`. Take a new [`Mixer::state`] each time.
    ///
    /// Events arrive on a connection of their own, so they never get in
    /// between a question and its answer on the main one.
    pub fn watch(&self, mut on_change: impl FnMut() -> bool) -> Result<()> {
        const SETTLE: Duration = Duration::from_millis(40);
        let mut link = Link::open()?;
        link.tell(Command::Subscribe(
            SubscriptionMask::SINK
                | SubscriptionMask::SOURCE
                | SubscriptionMask::SINK_INPUT
                | SubscriptionMask::SERVER,
        ))?;
        loop {
            link.event()?;
            // One change is a burst of events (dragging a slider, a stream
            // starting): let it land, set the rest aside, look once.
            loop {
                thread::sleep(SETTLE);
                if !link.has_more()? {
                    break;
                }
                while link.has_more()? {
                    link.event()?;
                }
            }
            if !on_change() {
                return Ok(());
            }
        }
    }
}

fn no_output() -> Error {
    Error("there is no sound output".to_owned())
}

fn no_input() -> Error {
    Error("there is no microphone".to_owned())
}

fn c_string(text: &str) -> Result<std::ffi::CString> {
    std::ffi::CString::new(text).map_err(|_| Error("that is not a device".to_owned()))
}

/// One authenticated connection: a question, then its answer, in turn.
#[derive(Debug)]
struct Link {
    socket: BufReader<UnixStream>,
    version: u16,
    sequence: u32,
}

impl Link {
    fn open() -> Result<Self> {
        let path = pulseaudio::socket_path_from_env()
            .ok_or_else(|| Error("no sound server socket in this session".to_owned()))?;
        let socket = UnixStream::connect(&path)
            .map_err(|err| Error(format!("cannot reach the sound server: {err}")))?;
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

        let mut props = protocol::Props::new();
        props.set(Prop::ApplicationName, CLIENT_NAME);
        let _: protocol::SetClientNameReply = link.ask(Command::SetClientName(props))?;
        Ok(link)
    }

    fn send(&mut self, command: &Command) -> Result<()> {
        self.sequence = self.sequence.wrapping_add(1);
        protocol::write_command_message(
            self.socket.get_mut(),
            self.sequence,
            command,
            self.version,
        )?;
        Ok(())
    }

    /// Send a command and read its reply.
    fn ask<R: CommandReply>(&mut self, command: Command) -> Result<R> {
        self.send(&command)?;
        let (_, reply) = protocol::read_reply_message(&mut self.socket, self.version)?;
        Ok(reply)
    }

    /// Send a command that is only acknowledged.
    fn tell(&mut self, command: Command) -> Result<()> {
        self.send(&command)?;
        protocol::read_ack_message(&mut self.socket)?;
        Ok(())
    }

    /// Read (and discard) the next event on a subscribed connection.
    fn event(&mut self) -> Result<()> {
        protocol::read_command_message(&mut self.socket, self.version)?;
        Ok(())
    }

    /// Whether something is waiting to be read, without waiting for it.
    fn has_more(&mut self) -> Result<bool> {
        if !self.socket.buffer().is_empty() {
            return Ok(true);
        }
        self.socket.get_ref().set_nonblocking(true)?;
        let waiting = match self.socket.fill_buf() {
            Ok([]) => Err(Error("the sound server closed the connection".to_owned())),
            Ok(_) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(err) => Err(err.into()),
        };
        self.socket.get_ref().set_nonblocking(false)?;
        waiting
    }

    fn default_sink(&mut self) -> Result<Option<protocol::SinkInfo>> {
        let server: protocol::ServerInfo = self.ask(Command::GetServerInfo)?;
        let Some(name) = server.default_sink_name else {
            return Ok(None);
        };
        let sinks: protocol::SinkInfoList = self.ask(Command::GetSinkInfoList)?;
        Ok(sinks.into_iter().find(|sink| sink.name == name))
    }

    fn default_source(&mut self) -> Result<Option<protocol::SourceInfo>> {
        let server: protocol::ServerInfo = self.ask(Command::GetServerInfo)?;
        let Some(name) = server.default_source_name else {
            return Ok(None);
        };
        let sources: protocol::SourceInfoList = self.ask(Command::GetSourceInfoList)?;
        Ok(sources.into_iter().find(|source| source.name == name))
    }

    /// Streams applications are playing, leaving out ones without a volume
    /// of their own.
    fn playing(&mut self) -> Result<Vec<protocol::SinkInputInfo>> {
        let inputs: protocol::SinkInputInfoList = self.ask(Command::GetSinkInputInfoList)?;
        Ok(inputs
            .into_iter()
            .filter(|input| input.has_volume)
            .collect())
    }

    fn streams_of(&mut self, app: &str) -> Result<Vec<protocol::SinkInputInfo>> {
        let streams: Vec<_> = self
            .playing()?
            .into_iter()
            .filter(|input| app_name(input) == app)
            .collect();
        if streams.is_empty() {
            return Err(Error(format!("{app} is not playing anything")));
        }
        Ok(streams)
    }
}

/// What to call the application behind a stream: its name, else its
/// program, else whatever the stream is called.
fn app_name(input: &protocol::SinkInputInfo) -> String {
    let text = |bytes: &[u8]| {
        let text = String::from_utf8_lossy(bytes.strip_suffix(&[0]).unwrap_or(bytes));
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    [Prop::ApplicationName, Prop::ApplicationProcessBinary]
        .into_iter()
        .find_map(|prop| input.props.get(prop).and_then(text))
        .or_else(|| text(input.name.to_bytes()))
        .unwrap_or_else(|| "Unknown".to_owned())
}

/// The loudest channel, in percent.
fn percent(volume: &ChannelVolume) -> u8 {
    let loudest = volume
        .channels()
        .iter()
        .map(Volume::as_u32)
        .max()
        .unwrap_or(0);
    raw_to_percent(loudest)
}

fn raw_to_percent(raw: u32) -> u8 {
    let norm = u64::from(Volume::NORM.as_u32());
    ((u64::from(raw) * 100 + norm / 2) / norm).min(255) as u8
}

fn percent_to_raw(percent: u8) -> u32 {
    let norm = u64::from(Volume::NORM.as_u32());
    ((u64::from(percent) * norm + 50) / 100) as u32
}

/// `current` with its loudest channel at `percent` and the others in the
/// same proportion to it as before, so a left/right balance survives.
fn scaled(current: &ChannelVolume, percent: u8) -> ChannelVolume {
    let target = u64::from(percent_to_raw(percent));
    let loudest = u64::from(
        current
            .channels()
            .iter()
            .map(Volume::as_u32)
            .max()
            .unwrap_or(0),
    );
    let mut volume = ChannelVolume::empty();
    for channel in current.channels() {
        // From silence there is no proportion to keep.
        let raw = (u64::from(channel.as_u32()) * target + loudest / 2)
            .checked_div(loudest)
            .unwrap_or(target);
        volume.push(Volume::from_u32_clamped(raw as u32));
    }
    volume
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(raw: &[u32]) -> ChannelVolume {
        let mut volume = ChannelVolume::empty();
        for raw in raw {
            volume.push(Volume::from_u32_clamped(*raw));
        }
        volume
    }

    fn raws(volume: &ChannelVolume) -> Vec<u32> {
        volume.channels().iter().map(Volume::as_u32).collect()
    }

    #[test]
    fn percentages_round_trip() {
        let norm = Volume::NORM.as_u32();
        assert_eq!(raw_to_percent(0), 0);
        assert_eq!(raw_to_percent(norm), 100);
        assert_eq!(raw_to_percent(norm / 2), 50);
        for percent in 0..=150 {
            assert_eq!(raw_to_percent(percent_to_raw(percent)), percent);
        }
        // The loudest channel is the one reported.
        assert_eq!(percent(&channels(&[norm / 4, norm / 2])), 50);
    }

    #[test]
    fn setting_a_volume_keeps_the_balance() {
        let norm = Volume::NORM.as_u32();
        // Left at half of right: still so at any level.
        let lopsided = channels(&[norm / 4, norm / 2]);
        assert_eq!(raws(&scaled(&lopsided, 100)), [norm / 2, norm]);
        assert_eq!(raws(&scaled(&lopsided, 0)), [0, 0]);
        // From silence there is no balance to keep.
        assert_eq!(raws(&scaled(&channels(&[0, 0]), 50)), [norm / 2, norm / 2]);
        assert_eq!(scaled(&channels(&[norm]), 30).channels().len(), 1);
    }
}
