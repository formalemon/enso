//! Between the system backends and the interface.
//!
//! The backends in `enso-sys` block: a Wi-Fi connection takes seconds. So the
//! panel never calls them. It hands [`Request`]s to [`System`], whose worker
//! threads do the waiting and report back with [`Update`]s — a fresh snapshot
//! after anything changes, whether the panel caused it or not.
//!
//! The helpers at the top are shared with the command line, which is free to
//! block.

use std::{
    io,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use enso_core::{Paths, chosen};
use enso_sys::{
    audio::{Mixer, Sound},
    bluetooth::{Bluetooth, Bluez},
    logind,
    power::{Backlight, BacklightKind, Battery, Fan, PlatformProfile, Sysfs},
    wifi::{ConnectError, Iwd, Wifi},
};

/// How often the readings that send no notifications (battery, temperature,
/// fans) are looked at while the panel is open.
const POLL: Duration = Duration::from_secs(2);

/// `2 h 10 min`, `45 min`.
pub fn duration(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// The raw level one step up or down from where `backlight` is. A step is
/// `percent` of the range; left to itself a screen moves 5 % and a keyboard
/// light one level, since it has only a handful.
pub fn step(backlight: &Backlight, percent: Option<u8>, up: bool) -> u32 {
    let of_range = |percent: u8| (backlight.max * u32::from(percent) / 100).max(1);
    let delta = match (percent, backlight.kind) {
        (Some(percent), _) => of_range(percent),
        (None, BacklightKind::Screen) => of_range(5),
        (None, BacklightKind::Keyboard) => 1,
    };
    let level = if up {
        backlight
            .brightness
            .saturating_add(delta)
            .min(backlight.max)
    } else {
        backlight.brightness.saturating_sub(delta)
    };
    match backlight.kind {
        // Stepping down never turns the screen off.
        BacklightKind::Screen => level.max(1),
        BacklightKind::Keyboard => level,
    }
}

/// Set a backlight's raw level: directly if `/sys` lets us, otherwise by
/// asking logind, which lets the user of the active session do it.
pub fn set_backlight(sysfs: &Sysfs, backlight: &Backlight, level: u32) -> Result<(), String> {
    if sysfs.set_backlight(backlight, level).is_ok() {
        return Ok(());
    }
    logind::set_brightness(backlight, level)
        .map_err(|err| format!("cannot set the brightness: {err}"))
}

/// Switch the power profile.
pub fn set_profile(sysfs: &Sysfs, profile: &str) -> Result<(), String> {
    sysfs.set_platform_profile(profile).map_err(|err| {
        if err.kind() == io::ErrorKind::PermissionDenied {
            "switching the power profile needs write access to \
             /sys/firmware/acpi/platform_profile, which only root has on this system"
                .to_owned()
        } else {
            format!("cannot switch the power profile: {err}")
        }
    })
}

/// The hardware readings the panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    pub battery: Option<Battery>,
    pub profile: Option<PlatformProfile>,
    /// CPU temperature in °C.
    pub temperature: Option<f32>,
    pub fans: Vec<Fan>,
    pub screen: Option<Backlight>,
}

impl Hardware {
    pub fn read(sysfs: &Sysfs) -> Self {
        Self {
            battery: sysfs.battery(),
            profile: sysfs.platform_profile(),
            temperature: sysfs.cpu_temperature(),
            fans: sysfs.fans(),
            screen: sysfs.screen_backlight(),
        }
    }
}

/// Something for a worker to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Set the screen's backlight to a raw level.
    Backlight(u32),
    Profile(String),
    /// Set the output's volume, in percent.
    Volume(u8),
    Mute(bool),
    /// Set an application's volume, in percent.
    AppVolume(String, u8),
    AppMute(String, bool),
    /// Make a device the output.
    Output(String),
    /// Make a device the microphone.
    Input(String),
    /// Set the microphone's level, in percent.
    MicVolume(u8),
    MicMute(bool),
    WifiPower(bool),
    WifiScan,
    WifiConnect {
        network: String,
        passphrase: Option<zeroize::Zeroizing<String>>,
    },
    WifiDisconnect,
    BluetoothPower(bool),
    BluetoothDiscover(bool),
    BluetoothConnect(String),
    BluetoothDisconnect(String),
    /// Keep a choice from Settings: `key` in the `[section]` table of the
    /// state directory's `settings.toml` ([`chosen`]).
    Keep {
        section: &'static str,
        key: &'static str,
        value: Setting,
    },
}

/// A value Settings keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Seconds(u64),
    Switch(bool),
}

/// News from a worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Hardware(Hardware),
    /// `None`: no iwd, or no wireless device.
    Wifi(Option<Wifi>),
    /// `None`: no BlueZ, or no adapter.
    Bluetooth(Option<Bluetooth>),
    /// `None`: no sound server.
    Sound(Option<Sound>),
    /// How a [`Request::WifiConnect`] ended.
    WifiConnect {
        network: String,
        result: Result<(), ConnectError>,
    },
    /// How a [`Request::BluetoothConnect`] ended.
    BluetoothConnect {
        device: String,
        result: Result<(), String>,
    },
    /// Some other request did not work.
    Failed(String),
}

/// Handles to the worker threads.
#[derive(Clone, Debug)]
pub struct System {
    hardware: Sender<Request>,
    wifi: Sender<Request>,
    bluetooth: Sender<Request>,
    sound: Sender<Request>,
    settings: Sender<Request>,
}

impl System {
    /// Start the workers. Each reports its domain's state straight away and
    /// then whenever it changes, through `report`, until that returns `false`.
    pub fn start(paths: Paths, report: impl Fn(Update) -> bool + Clone + Send + 'static) -> Self {
        let (settings, settings_requests) = mpsc::channel();
        spawn("enso-settings", {
            let report = report.clone();
            move || settings_worker(&paths, &settings_requests, &report)
        });
        let (hardware, hardware_requests) = mpsc::channel();
        let (wifi, wifi_requests) = mpsc::channel();
        let (bluetooth, bluetooth_requests) = mpsc::channel();
        let (sound, sound_requests) = mpsc::channel();
        spawn("enso-sound", {
            let report = report.clone();
            move || sound_worker(&sound_requests, report)
        });
        spawn("enso-hardware", {
            let report = report.clone();
            move || hardware_worker(&hardware_requests, &report)
        });
        spawn("enso-wifi", {
            let report = report.clone();
            move || wifi_worker(wifi_requests, report)
        });
        spawn("enso-bluetooth", move || {
            bluetooth_worker(bluetooth_requests, report);
        });
        Self {
            hardware,
            wifi,
            bluetooth,
            sound,
            settings,
        }
    }

    /// A system with no workers: requests pile up in the returned receiver.
    /// For tests, and for drawing the panel without touching the machine.
    pub fn detached() -> (Self, Receiver<Request>) {
        let (sender, requests) = mpsc::channel();
        let system = Self {
            hardware: sender.clone(),
            wifi: sender.clone(),
            bluetooth: sender.clone(),
            sound: sender.clone(),
            settings: sender,
        };
        (system, requests)
    }

    /// Hand a request to the worker for its domain. Never blocks.
    pub fn request(&self, request: Request) {
        let worker = match request {
            Request::Backlight(_) | Request::Profile(_) => &self.hardware,
            Request::Volume(_)
            | Request::Mute(_)
            | Request::AppVolume(..)
            | Request::AppMute(..)
            | Request::Output(_)
            | Request::Input(_)
            | Request::MicVolume(_)
            | Request::MicMute(_) => &self.sound,
            Request::WifiPower(_)
            | Request::WifiScan
            | Request::WifiConnect { .. }
            | Request::WifiDisconnect => &self.wifi,
            Request::BluetoothPower(_)
            | Request::BluetoothDiscover(_)
            | Request::BluetoothConnect(_)
            | Request::BluetoothDisconnect(_) => &self.bluetooth,
            Request::Keep { .. } => &self.settings,
        };
        // A worker that is gone has already said why, or the panel is closing.
        let _ = worker.send(request);
    }
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(err) = thread::Builder::new().name(name.to_owned()).spawn(work) {
        eprintln!("enso-ctl: cannot start the {name} thread: {err}");
    }
}

/// Keeps what's chosen in Settings, in the order it was chosen, so the
/// interface never waits on the disk. Of a run of changes to one setting —
/// a switch flipped and flipped back — only the last is written.
pub(crate) fn settings_worker(
    paths: &Paths,
    requests: &Receiver<Request>,
    report: &impl Fn(Update) -> bool,
) {
    while let Ok(first) = requests.recv() {
        let mut pending: Vec<(&str, &str, Setting)> = Vec::new();
        for request in std::iter::once(first).chain(requests.try_iter()) {
            if let Request::Keep {
                section,
                key,
                value,
            } = request
            {
                pending.retain(|(s, k, _)| (*s, *k) != (section, key));
                pending.push((section, key, value));
            }
        }
        for (section, key, value) in pending {
            let kept = match value {
                Setting::Seconds(seconds) => chosen::set(
                    paths,
                    section,
                    key,
                    i64::try_from(seconds).unwrap_or(i64::MAX),
                ),
                Setting::Switch(on) => chosen::set(paths, section, key, on),
            };
            if let Err(err) = kept
                && !report(Update::Failed(format!("can't keep the setting: {err}")))
            {
                return;
            }
        }
    }
}

/// Polls `/sys` and applies backlight and profile changes.
fn hardware_worker(requests: &Receiver<Request>, report: &impl Fn(Update) -> bool) {
    let sysfs = Sysfs::system();
    let mut last = None;
    loop {
        let hardware = Hardware::read(&sysfs);
        if last.as_ref() != Some(&hardware) {
            if !report(Update::Hardware(hardware.clone())) {
                return;
            }
            last = Some(hardware.clone());
        }

        let first = match requests.recv_timeout(POLL) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        for request in newest(first, requests) {
            let result = match request {
                Request::Backlight(level) => match &hardware.screen {
                    Some(backlight) => set_backlight(&sysfs, backlight, level),
                    None => Ok(()),
                },
                Request::Profile(profile) => set_profile(&sysfs, &profile),
                _ => Ok(()),
            };
            if let Err(message) = result
                && !report(Update::Failed(message))
            {
                return;
            }
        }
    }
}

/// `first` and everything queued behind it, in order — except that of the
/// requests a dragged slider sends in a stream, only the newest for each
/// slider is kept: the ones before it are already out of date.
fn newest(first: Request, requests: &Receiver<Request>) -> Vec<Request> {
    let slider = |request: &Request| match request {
        Request::Backlight(_) => Some(String::new()),
        Request::Volume(_) | Request::MicVolume(_) => Some(String::new()),
        Request::AppVolume(app, _) => Some(format!("app {app}")),
        _ => None,
    };
    let mut pending: Vec<Request> = Vec::new();
    for request in std::iter::once(first).chain(requests.try_iter()) {
        if let Some(key) = slider(&request) {
            pending.retain(|queued| {
                std::mem::discriminant(queued) != std::mem::discriminant(&request)
                    || slider(queued).as_ref() != Some(&key)
            });
        }
        pending.push(request);
    }
    pending
}

/// Serves volume requests; a second thread republishes on every change.
fn sound_worker(
    requests: &Receiver<Request>,
    report: impl Fn(Update) -> bool + Clone + Send + 'static,
) {
    let Ok(mixer) = Mixer::connect() else {
        report(Update::Sound(None));
        return;
    };
    let publish = {
        let (mixer, report) = (mixer.clone(), report.clone());
        move || report(Update::Sound(mixer.state().ok()))
    };
    if !publish() {
        return;
    }
    spawn("enso-sound-watch", {
        let (mixer, publish) = (mixer.clone(), publish.clone());
        move || {
            let _ = mixer.watch(publish);
        }
    });

    // The watcher republishes after each change, ours included.
    while let Ok(first) = requests.recv() {
        for request in newest(first, requests) {
            let result = match request {
                Request::Volume(volume) => mixer.set_volume(volume),
                Request::Mute(muted) => mixer.set_muted(muted),
                Request::AppVolume(app, volume) => mixer.set_app_volume(&app, volume),
                Request::AppMute(app, muted) => mixer.set_app_muted(&app, muted),
                Request::Output(device) => mixer.set_output(&device),
                Request::Input(device) => mixer.set_input(&device),
                Request::MicVolume(volume) => mixer.set_input_volume(volume),
                Request::MicMute(muted) => mixer.set_input_muted(muted),
                _ => Ok(()),
            };
            if let Err(err) = result
                && !report(Update::Failed(err.to_string()))
            {
                return;
            }
        }
    }
}

/// Serves Wi-Fi requests; a second thread republishes on every change.
fn wifi_worker(
    requests: Receiver<Request>,
    report: impl Fn(Update) -> bool + Clone + Send + 'static,
) {
    let Ok(iwd) = Iwd::connect() else {
        report(Update::Wifi(None));
        return;
    };
    let publish = {
        let (iwd, report) = (iwd.clone(), report.clone());
        move || report(Update::Wifi(iwd.state().ok().flatten()))
    };
    if !publish() {
        return;
    }
    spawn("enso-wifi-watch", {
        let (iwd, publish) = (iwd.clone(), publish.clone());
        move || {
            // Ends when the panel does; an error just means no live updates.
            let _ = iwd.watch(publish);
        }
    });

    for request in requests {
        let result = match request {
            Request::WifiPower(on) => iwd.set_powered(on),
            Request::WifiScan => iwd.scan(),
            Request::WifiDisconnect => iwd.disconnect(),
            Request::WifiConnect {
                network,
                passphrase,
            } => {
                let result =
                    iwd.connect_to(&network, passphrase.as_ref().map(|text| text.as_str()));
                if !(publish() && report(Update::WifiConnect { network, result })) {
                    return;
                }
                continue;
            }
            _ => Ok(()),
        };
        if let Err(err) = result
            && !report(Update::Failed(err.to_string()))
        {
            return;
        }
        if !publish() {
            return;
        }
    }
}

/// Serves Bluetooth requests; a second thread republishes on every change.
fn bluetooth_worker(
    requests: Receiver<Request>,
    report: impl Fn(Update) -> bool + Clone + Send + 'static,
) {
    let Ok(bluez) = Bluez::connect() else {
        report(Update::Bluetooth(None));
        return;
    };
    let publish = {
        let (bluez, report) = (bluez.clone(), report.clone());
        move || report(Update::Bluetooth(bluez.state().ok().flatten()))
    };
    if !publish() {
        return;
    }
    spawn("enso-bluetooth-watch", {
        let (bluez, publish) = (bluez.clone(), publish.clone());
        move || {
            let _ = bluez.watch(publish);
        }
    });

    for request in requests {
        let result = match request {
            Request::BluetoothPower(on) => bluez.set_powered(on),
            Request::BluetoothDiscover(on) => bluez.set_discovering(on),
            Request::BluetoothDisconnect(device) => bluez.disconnect(&device),
            Request::BluetoothConnect(device) => {
                let result = bluez.connect_to(&device).map_err(|err| err.to_string());
                if !(publish() && report(Update::BluetoothConnect { device, result })) {
                    return;
                }
                continue;
            }
            _ => Ok(()),
        };
        if let Err(err) = result
            && !report(Update::Failed(err.to_string()))
        {
            return;
        }
        if !publish() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backlight(kind: BacklightKind, brightness: u32, max: u32) -> Backlight {
        Backlight {
            kind,
            name: "test".into(),
            brightness,
            max,
        }
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(45), "45 min");
        assert_eq!(duration(120), "2 h");
        assert_eq!(duration(135), "2 h 15 min");
    }

    #[test]
    fn steps_stay_in_range_and_keep_the_screen_lit() {
        use BacklightKind::{Keyboard, Screen};
        let screen = |brightness| backlight(Screen, brightness, 1000);
        assert_eq!(step(&screen(500), None, true), 550);
        assert_eq!(step(&screen(500), Some(10), false), 400);
        assert_eq!(step(&screen(980), None, true), 1000);
        assert_eq!(step(&screen(20), None, false), 1);
        // A coarse panel still moves by at least one level.
        assert_eq!(step(&backlight(Screen, 3, 7), None, true), 4);

        let keys = |brightness| backlight(Keyboard, brightness, 3);
        assert_eq!(step(&keys(1), None, true), 2);
        assert_eq!(step(&keys(3), None, true), 3);
        assert_eq!(step(&keys(1), None, false), 0);
        assert_eq!(step(&keys(0), None, false), 0);
    }

    #[test]
    fn requests_reach_a_detached_system() {
        let (system, requests) = System::detached();
        system.request(Request::WifiScan);
        system.request(Request::Backlight(5));
        system.request(Request::AppVolume("Firefox".into(), 40));
        system.request(Request::BluetoothPower(false));
        assert_eq!(
            requests.try_iter().collect::<Vec<_>>(),
            [
                Request::WifiScan,
                Request::Backlight(5),
                Request::AppVolume("Firefox".into(), 40),
                Request::BluetoothPower(false),
            ]
        );
    }

    #[test]
    fn a_dragged_slider_only_sends_its_newest_level() {
        let (sender, requests) = mpsc::channel();
        let queue = [
            Request::Volume(41),
            Request::AppVolume("Firefox".into(), 10),
            Request::Mute(true),
            Request::AppVolume("mpv".into(), 70),
            Request::Volume(42),
            Request::AppVolume("Firefox".into(), 12),
            Request::Mute(false),
        ];
        for request in queue {
            sender.send(request).unwrap();
        }
        assert_eq!(
            newest(Request::Volume(40), &requests),
            [
                // Mutes are deliberate, each of them.
                Request::Mute(true),
                Request::AppVolume("mpv".into(), 70),
                Request::Volume(42),
                Request::AppVolume("Firefox".into(), 12),
                Request::Mute(false),
            ]
        );
    }
}
