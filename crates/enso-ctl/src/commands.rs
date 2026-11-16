//! The hardware and network subcommands: plain text in, plain text out, so
//! they suit key bindings and scripts as well as a terminal.

use std::{
    io::{self, BufRead, IsTerminal, Write},
    thread,
    time::{Duration, Instant},
};

use enso_sys::{
    audio::{Device as SoundDevice, Mixer, Sound},
    bluetooth::{Bluetooth, Bluez, Device},
    power::{BatteryStatus, Sysfs},
    wifi::{ConnectError, Iwd, Link, Network, Security, Wifi},
};

use crate::{
    cli::{BluetoothCommand, Level, Light, ProfileCommand, VolumeChange, WifiCommand},
    system,
};

type Outcome = Result<(), String>;

fn io_error(err: io::Error) -> String {
    err.to_string()
}

/// `enso-ctl power`: a summary read from `/sys`.
pub fn power(out: &mut impl Write) -> Outcome {
    let sysfs = Sysfs::system();
    let mut line =
        |label: &str, value: String| writeln!(out, "{label:<12} {value}").map_err(io_error);

    match sysfs.battery() {
        Some(battery) => {
            let status = match battery.status {
                BatteryStatus::Charging => "charging",
                BatteryStatus::Discharging => "discharging",
                BatteryStatus::Full => "full",
                BatteryStatus::NotCharging => "plugged in, not charging",
                BatteryStatus::Unknown => "unknown",
            };
            let mut details = vec![status.to_owned()];
            if let Some(minutes) = battery.minutes_left {
                let what = if battery.status == BatteryStatus::Charging {
                    "until full"
                } else {
                    "left"
                };
                details.push(format!("{} {what}", system::duration(minutes)));
            }
            if let Some(milliwatts) = battery.power_mw.filter(|mw| *mw > 0) {
                details.push(format!("{:.1} W", f64::from(milliwatts) / 1000.0));
            }
            line(
                "battery",
                format!("{}% ({})", battery.percent, details.join(", ")),
            )?;
        }
        None => line("battery", "none".to_owned())?,
    }
    if let Some(profile) = sysfs.platform_profile() {
        line(
            "profile",
            format!("{} (of: {})", profile.current, profile.choices.join(", ")),
        )?;
    }
    if let Some(temperature) = sysfs.cpu_temperature() {
        line("temperature", format!("{temperature:.0}°C"))?;
    }
    let fans = sysfs.fans();
    if !fans.is_empty() {
        let fans: Vec<String> = fans
            .iter()
            .map(|fan| format!("{} {} rpm", fan.label, fan.rpm))
            .collect();
        line("fans", fans.join(", "))?;
    }
    if let Some(screen) = sysfs.screen_backlight() {
        line("brightness", format!("{}%", screen.percent()))?;
    }
    if let Some(keyboard) = sysfs.keyboard_backlight() {
        line(
            "keyboard",
            format!("{} of {}", keyboard.brightness, keyboard.max),
        )?;
    }
    Ok(())
}

/// `enso-ctl brightness …`: show or change a backlight, then print where it is.
pub fn brightness(light: Light, level: Level, out: &mut impl Write) -> Outcome {
    let sysfs = Sysfs::system();
    let backlight = match light {
        Light::Screen => sysfs
            .screen_backlight()
            .ok_or("no screen backlight to control")?,
        Light::Keyboard => sysfs
            .keyboard_backlight()
            .ok_or("no keyboard light to control")?,
    };
    let target = match level {
        Level::Show => None,
        Level::Set(percent) => Some(backlight.level(percent)),
        Level::Up(step) => Some(system::step(&backlight, step, true)),
        Level::Down(step) => Some(system::step(&backlight, step, false)),
    };
    if let Some(target) = target {
        system::set_backlight(&sysfs, &backlight, target)?;
    }
    let now = match light {
        Light::Screen => sysfs.screen_backlight(),
        Light::Keyboard => sysfs.keyboard_backlight(),
    }
    .unwrap_or(backlight);
    writeln!(out, "{}%", now.percent()).map_err(io_error)
}

/// `enso-ctl volume …`: the output's volume, or one application's.
pub fn volume(app: Option<&str>, change: VolumeChange, out: &mut impl Write) -> Outcome {
    let mixer = Mixer::connect().map_err(|err| err.to_string())?;
    let sound = mixer.state().map_err(|err| err.to_string())?;
    if app.is_none() && change == VolumeChange::Level(Level::Show) {
        return print_sound(&sound, out);
    }

    // Applications are named as they are listed, give or take case.
    let app = match app {
        Some(wanted) => Some(
            sound
                .apps
                .iter()
                .find(|app| app.name.eq_ignore_ascii_case(wanted))
                .ok_or_else(|| {
                    format!("`{wanted}` is not playing anything (see `enso-ctl volume`)")
                })?,
        ),
        None => None,
    };
    let (now, muted) = match (app, &sound.output) {
        (Some(app), _) => (app.volume, app.muted),
        (None, Some(output)) => (output.volume, output.muted),
        (None, None) => return Err("there is no sound output".to_owned()),
    };

    let result = match change {
        VolumeChange::Level(level) => {
            let target = next_volume(now, level);
            match app {
                Some(app) => mixer.set_app_volume(&app.name, target),
                None => mixer.set_volume(target),
            }
        }
        VolumeChange::Mute(_) | VolumeChange::ToggleMute => {
            let mute = match change {
                VolumeChange::Mute(mute) => mute,
                _ => !muted,
            };
            match app {
                Some(app) => mixer.set_app_muted(&app.name, mute),
                None => mixer.set_muted(mute),
            }
        }
    };
    result.map_err(|err| err.to_string())?;

    // Say where it ended up, as the server has it.
    let sound = mixer.state().map_err(|err| err.to_string())?;
    let after = match app {
        Some(app) => sound
            .apps
            .iter()
            .find(|now| now.name == app.name)
            .map(|app| (app.volume, app.muted)),
        None => sound.output.map(|output| (output.volume, output.muted)),
    };
    match after {
        Some((volume, true)) => writeln!(out, "{volume}% muted"),
        Some((volume, false)) => writeln!(out, "{volume}%"),
        None => Ok(()),
    }
    .map_err(io_error)
}

/// `enso-ctl mic …`: the microphone's level and mute.
pub fn mic(change: VolumeChange, out: &mut impl Write) -> Outcome {
    let mixer = Mixer::connect().map_err(|err| err.to_string())?;
    let sound = mixer.state().map_err(|err| err.to_string())?;
    if change == VolumeChange::Level(Level::Show) {
        match &sound.input {
            Some(input) => writeln!(
                out,
                "microphone   {}%{}",
                input.volume,
                if input.muted { "  muted" } else { "" }
            ),
            None => writeln!(out, "microphone   none"),
        }
        .map_err(io_error)?;
        return print_devices(&sound.inputs, out);
    }
    let input = sound.input.ok_or("there is no microphone")?;
    match change {
        VolumeChange::Level(level) => mixer.set_input_volume(next_volume(input.volume, level)),
        VolumeChange::Mute(mute) => mixer.set_input_muted(mute),
        VolumeChange::ToggleMute => mixer.set_input_muted(!input.muted),
    }
    .map_err(|err| err.to_string())?;
    match mixer.state().map_err(|err| err.to_string())?.input {
        Some(input) if input.muted => writeln!(out, "{}% muted", input.volume),
        Some(input) => writeln!(out, "{}%", input.volume),
        None => Ok(()),
    }
    .map_err(io_error)
}

/// `enso-ctl volume output [NAME]` and `enso-ctl mic input [NAME]`: list the
/// devices, or switch to one.
pub fn sound_device(input: bool, name: Option<&str>, out: &mut impl Write) -> Outcome {
    let mixer = Mixer::connect().map_err(|err| err.to_string())?;
    let sound = mixer.state().map_err(|err| err.to_string())?;
    let devices = if input { &sound.inputs } else { &sound.outputs };
    let Some(wanted) = name else {
        return print_devices(devices, out);
    };
    let device = find_sound_device(devices, wanted)?;
    if input {
        mixer.set_input(&device.id)
    } else {
        mixer.set_output(&device.id)
    }
    .map_err(|err| err.to_string())?;
    writeln!(out, "{}", device.name).map_err(io_error)
}

/// Pick a device by its name, or by any part of it that only one has.
/// Without regard to case.
fn find_sound_device<'a>(
    devices: &'a [SoundDevice],
    wanted: &str,
) -> Result<&'a SoundDevice, String> {
    let lower = wanted.to_lowercase();
    if let Some(device) = devices.iter().find(|d| d.name.to_lowercase() == lower) {
        return Ok(device);
    }
    let matching: Vec<&SoundDevice> = devices
        .iter()
        .filter(|device| device.name.to_lowercase().contains(&lower))
        .collect();
    match matching.as_slice() {
        [device] => Ok(device),
        [] => Err(format!("no device called `{wanted}`")),
        several => Err(format!(
            "`{wanted}` could be any of: {}",
            several
                .iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn print_devices(devices: &[SoundDevice], out: &mut impl Write) -> Outcome {
    for device in devices {
        writeln!(
            out,
            "  {} {}",
            if device.current { '*' } else { ' ' },
            device.name
        )
        .map_err(io_error)?;
    }
    Ok(())
}

/// Where a volume at `now` percent goes for `level`. Steps stop at 100;
/// setting it outright may go no further either.
fn next_volume(now: u8, level: Level) -> u8 {
    const STEP: u8 = 5;
    let now = now.min(100);
    match level {
        Level::Show => now,
        Level::Set(percent) => percent.min(100),
        Level::Up(step) => now + step.unwrap_or(STEP).min(100 - now),
        Level::Down(step) => now.saturating_sub(step.unwrap_or(STEP)),
    }
}

fn print_sound(sound: &Sound, out: &mut impl Write) -> Outcome {
    let muted = |muted: bool| if muted { "  muted" } else { "" };
    match &sound.output {
        Some(output) => writeln!(
            out,
            "volume       {}%{} ({})",
            output.volume,
            muted(output.muted),
            output.name
        ),
        None => writeln!(out, "volume       no output"),
    }
    .map_err(io_error)?;
    let width = sound
        .apps
        .iter()
        .map(|app| app.name.chars().count())
        .max()
        .unwrap_or(0);
    for app in &sound.apps {
        writeln!(
            out,
            "  {:<width$}  {:>3}%{}",
            app.name,
            app.volume,
            muted(app.muted)
        )
        .map_err(io_error)?;
    }
    // With a choice of outputs, say what the choice is.
    if sound.outputs.len() > 1 {
        writeln!(out, "outputs").map_err(io_error)?;
        print_devices(&sound.outputs, out)?;
    }
    Ok(())
}

/// `enso-ctl profile …`.
pub fn profile(command: &ProfileCommand, out: &mut impl Write) -> Outcome {
    let sysfs = Sysfs::system();
    let profile = sysfs
        .platform_profile()
        .ok_or("this machine has no power profiles")?;
    let target = match command {
        ProfileCommand::Show => {
            return writeln!(
                out,
                "{} (of: {})",
                profile.current,
                profile.choices.join(", ")
            )
            .map_err(io_error);
        }
        ProfileCommand::Set(name) => {
            if !profile.choices.contains(name) {
                return Err(format!(
                    "no profile `{name}` here; choose from: {}",
                    profile.choices.join(", ")
                ));
            }
            name.as_str()
        }
        ProfileCommand::Next => profile.next().ok_or("there is no other profile")?,
    };
    system::set_profile(&sysfs, target)?;
    writeln!(out, "{target}").map_err(io_error)
}

/// `enso-ctl wifi …`.
pub fn wifi(command: &WifiCommand, out: &mut impl Write) -> Outcome {
    let iwd = Iwd::connect().map_err(|err| err.to_string())?;
    let state = || -> Result<Wifi, String> {
        iwd.state().map_err(|err| err.to_string())?.ok_or_else(|| {
            "Wi-Fi is unavailable: iwd is not running, or there is no wireless device".to_owned()
        })
    };
    let find = |name: &str| -> Result<Network, String> {
        state()?
            .networks
            .into_iter()
            .find(|network| network.name == name)
            .ok_or_else(|| format!("no network named `{name}` in range (try `enso-ctl wifi scan`)"))
    };

    match command {
        WifiCommand::Status => print_wifi(&state()?, out),
        WifiCommand::Power(on) => {
            // Checked first so a missing radio is reported as such.
            state()?;
            iwd.set_powered(*on).map_err(|err| err.to_string())?;
            writeln!(out, "wifi {}", if *on { "on" } else { "off" }).map_err(io_error)
        }
        WifiCommand::Scan => {
            if !state()?.powered {
                return Err("Wi-Fi is off (`enso-ctl wifi on`)".to_owned());
            }
            iwd.scan().map_err(|err| err.to_string())?;
            // Results are in once iwd stops scanning; give it a moment to start.
            let deadline = Instant::now() + Duration::from_secs(15);
            thread::sleep(Duration::from_millis(300));
            while state()?.scanning && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(200));
            }
            print_wifi(&state()?, out)
        }
        WifiCommand::Connect(name) => {
            let network = find(name)?;
            if network.connected {
                return writeln!(out, "already connected to {name}").map_err(io_error);
            }
            match network.security {
                Security::Wep => return Err("iwd does not connect to WEP networks".to_owned()),
                Security::Enterprise if !network.known => {
                    return Err(
                        "this is an enterprise (802.1X) network: give iwd a profile for it first"
                            .to_owned(),
                    );
                }
                _ => {}
            }
            let result = match iwd.connect_to(&network.id, None) {
                Err(ConnectError::NeedsPassphrase) => {
                    let passphrase = read_passphrase(name)?;
                    iwd.connect_to(&network.id, Some(&passphrase))
                }
                other => other,
            };
            result.map_err(|err| format!("cannot connect to {name}: {err}"))?;
            writeln!(out, "connected to {name}").map_err(io_error)
        }
        WifiCommand::Disconnect => {
            if state()?.connected().is_none() {
                return writeln!(out, "not connected").map_err(io_error);
            }
            iwd.disconnect().map_err(|err| err.to_string())?;
            writeln!(out, "disconnected").map_err(io_error)
        }
        WifiCommand::Forget(name) => {
            let network = find(name)?;
            iwd.forget(&network.id).map_err(|err| err.to_string())?;
            writeln!(out, "forgot {name}").map_err(io_error)
        }
    }
}

fn print_wifi(wifi: &Wifi, out: &mut impl Write) -> Outcome {
    let summary = match (wifi.powered, wifi.link, wifi.connected()) {
        (false, ..) => "off".to_owned(),
        (true, _, Some(network)) => format!("connected to {}", network.name),
        (true, Link::Connecting, None) => "connecting".to_owned(),
        (true, ..) => "not connected".to_owned(),
    };
    writeln!(out, "wifi         {summary} ({})", wifi.interface).map_err(io_error)?;

    let width = wifi
        .networks
        .iter()
        .map(|network| network.name.chars().count())
        .max()
        .unwrap_or(0);
    for network in &wifi.networks {
        let bars: String = "▂▄▆█"
            .chars()
            .enumerate()
            .map(|(i, bar)| {
                if i < usize::from(network.bars()) {
                    bar
                } else {
                    '·'
                }
            })
            .collect();
        let security = match network.security {
            Security::Open => "open",
            Security::Passphrase => "secured",
            Security::Enterprise => "enterprise",
            Security::Wep => "wep",
        };
        writeln!(
            out,
            "  {} {:<width$}  {bars}  {:>4} dBm  {security}{}",
            if network.connected { '*' } else { ' ' },
            network.name,
            network.signal_dbm,
            if network.known { ", saved" } else { "" },
        )
        .map_err(io_error)?;
    }
    Ok(())
}

/// Ask for a passphrase: typed unseen at a terminal, or one line of piped
/// input. Never taken from the command line, where other users could read it.
fn read_passphrase(network: &str) -> Result<String, String> {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};

    let stdin = io::stdin();
    let mut line = String::new();
    if stdin.is_terminal() {
        eprint!("Passphrase for {network}: ");
        let saved = tcgetattr(&stdin).map_err(|err| err.to_string())?;
        let mut quiet = saved.clone();
        quiet.local_modes.remove(LocalModes::ECHO);
        tcsetattr(&stdin, OptionalActions::Now, &quiet).map_err(|err| err.to_string())?;
        let read = stdin.lock().read_line(&mut line);
        // Whatever happened, give the terminal its echo back.
        let restored = tcsetattr(&stdin, OptionalActions::Now, &saved);
        eprintln!();
        read.map_err(io_error)?;
        restored.map_err(|err| err.to_string())?;
    } else {
        stdin.lock().read_line(&mut line).map_err(io_error)?;
    }
    let passphrase = line.trim_end_matches(['\n', '\r']);
    if passphrase.is_empty() {
        return Err(format!("{network} needs a passphrase"));
    }
    Ok(passphrase.to_owned())
}

/// `enso-ctl bluetooth …`.
pub fn bluetooth(command: &BluetoothCommand, out: &mut impl Write) -> Outcome {
    let bluez = Bluez::connect().map_err(|err| err.to_string())?;
    let state = || -> Result<Bluetooth, String> {
        bluez
            .state()
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "Bluetooth is unavailable: BlueZ is not running, or there is no adapter".to_owned()
            })
    };
    let find = |wanted: &str| -> Result<Device, String> { find_device(&state()?.devices, wanted) };

    match command {
        BluetoothCommand::Status => print_bluetooth(&state()?, out),
        BluetoothCommand::Power(on) => {
            state()?;
            bluez.set_powered(*on).map_err(|err| err.to_string())?;
            writeln!(out, "bluetooth {}", if *on { "on" } else { "off" }).map_err(io_error)
        }
        BluetoothCommand::Scan(seconds) => {
            if !state()?.powered {
                return Err("Bluetooth is off (`enso-ctl bluetooth on`)".to_owned());
            }
            bluez.set_discovering(true).map_err(|err| err.to_string())?;
            thread::sleep(Duration::from_secs(*seconds));
            let found = state();
            // Discovery would end with this process anyway; be tidy.
            let _ = bluez.set_discovering(false);
            print_bluetooth(&found?, out)
        }
        BluetoothCommand::Connect(wanted) => {
            let device = find(wanted)?;
            bluez
                .connect_to(&device.id)
                .map_err(|err| format!("cannot connect to {}: {err}", device.name))?;
            writeln!(out, "connected to {}", device.name).map_err(io_error)
        }
        BluetoothCommand::Disconnect(wanted) => {
            let device = find(wanted)?;
            bluez
                .disconnect(&device.id)
                .map_err(|err| err.to_string())?;
            writeln!(out, "disconnected {}", device.name).map_err(io_error)
        }
        BluetoothCommand::Forget(wanted) => {
            let device = find(wanted)?;
            bluez.remove(&device.id).map_err(|err| err.to_string())?;
            writeln!(out, "forgot {}", device.name).map_err(io_error)
        }
    }
}

/// Pick a device by address, exact name, or — if only one fits — the start
/// of its name. Names compare without regard to case.
fn find_device(devices: &[Device], wanted: &str) -> Result<Device, String> {
    let lower = wanted.to_lowercase();
    let exact = devices.iter().find(|device| {
        device.address.eq_ignore_ascii_case(wanted) || device.name.to_lowercase() == lower
    });
    if let Some(device) = exact {
        return Ok(device.clone());
    }
    let starting: Vec<&Device> = devices
        .iter()
        .filter(|device| device.name.to_lowercase().starts_with(&lower))
        .collect();
    match starting.as_slice() {
        [device] => Ok((*device).clone()),
        [] => Err(format!(
            "no device called `{wanted}` (see `enso-ctl bluetooth`, or try `enso-ctl bluetooth scan`)"
        )),
        several => Err(format!(
            "`{wanted}` could be any of: {}",
            several
                .iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn print_bluetooth(bluetooth: &Bluetooth, out: &mut impl Write) -> Outcome {
    writeln!(
        out,
        "bluetooth    {}",
        if bluetooth.powered { "on" } else { "off" }
    )
    .map_err(io_error)?;
    let width = bluetooth
        .devices
        .iter()
        .map(|device| device.name.chars().count())
        .max()
        .unwrap_or(0);
    for device in &bluetooth.devices {
        let state = match (device.connected, device.paired) {
            (true, _) => "connected",
            (false, true) => "paired",
            (false, false) => "nearby",
        };
        let battery = device
            .battery
            .map(|percent| format!("  {percent}%"))
            .unwrap_or_default();
        writeln!(
            out,
            "  {} {:<width$}  {}  {state}{battery}",
            if device.connected { '*' } else { ' ' },
            device.name,
            device.address,
        )
        .map_err(io_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, address: &str) -> Device {
        Device {
            id: format!("/org/bluez/hci0/dev_{}", address.replace(':', "_")),
            name: name.to_owned(),
            address: address.to_owned(),
            kind: None,
            paired: true,
            connected: false,
            battery: None,
        }
    }

    #[test]
    fn volumes_step_within_bounds_and_list_in_columns() {
        assert_eq!(next_volume(60, Level::Up(None)), 65);
        assert_eq!(next_volume(98, Level::Up(None)), 100);
        assert_eq!(next_volume(3, Level::Down(None)), 0);
        assert_eq!(next_volume(60, Level::Down(Some(20))), 40);
        assert_eq!(next_volume(60, Level::Set(30)), 30);
        // A volume boosted elsewhere comes back into range when stepped.
        assert_eq!(next_volume(130, Level::Down(None)), 95);

        let device = |name: &str| SoundDevice {
            id: name.to_lowercase(),
            name: name.to_owned(),
            current: false,
        };
        let devices = [
            device("Built-in Audio Analog Stereo"),
            device("Soft Buds"),
            device("Soft Speaker"),
        ];
        let found = |wanted: &str| find_sound_device(&devices, wanted).map(|d| d.name.as_str());
        assert_eq!(found("soft buds"), Ok("Soft Buds"));
        assert_eq!(found("built-in"), Ok("Built-in Audio Analog Stereo"));
        assert!(
            found("soft")
                .unwrap_err()
                .contains("Soft Buds, Soft Speaker")
        );
        assert!(found("hdmi").unwrap_err().contains("no device"));

        let sound = Sound {
            output: Some(enso_sys::audio::Output {
                name: "Speakers".into(),
                volume: 60,
                muted: false,
            }),
            apps: vec![
                enso_sys::audio::App {
                    name: "Firefox".into(),
                    volume: 80,
                    muted: false,
                },
                enso_sys::audio::App {
                    name: "Music Player".into(),
                    volume: 5,
                    muted: true,
                },
            ],
            ..Sound::default()
        };
        let mut out = Vec::new();
        print_sound(&sound, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "volume       60% (Speakers)\n\
             \x20 Firefox        80%\n\
             \x20 Music Player    5%  muted\n"
        );
    }

    #[test]
    fn devices_are_found_by_address_name_or_unique_prefix() {
        let devices = [
            device("Soft Buds", "AA:BB:CC:00:00:01"),
            device("Soft Speaker", "AA:BB:CC:00:00:02"),
            device("Mouse", "AA:BB:CC:00:00:03"),
        ];
        let found = |wanted: &str| find_device(&devices, wanted).map(|device| device.name);
        assert_eq!(found("aa:bb:cc:00:00:02").unwrap(), "Soft Speaker");
        assert_eq!(found("soft buds").unwrap(), "Soft Buds");
        assert_eq!(found("mou").unwrap(), "Mouse");
        assert!(
            found("soft")
                .unwrap_err()
                .contains("Soft Buds, Soft Speaker")
        );
        assert!(found("keyboard").unwrap_err().contains("no device"));
    }

    #[test]
    fn lists_networks_and_devices_in_aligned_columns() {
        let network = |name: &str, signal_dbm, connected| Network {
            id: String::new(),
            name: name.to_owned(),
            security: Security::Passphrase,
            signal_dbm,
            connected,
            known: connected,
        };
        let wifi = Wifi {
            interface: "wlan0".into(),
            powered: true,
            link: Link::Connected,
            scanning: false,
            networks: vec![network("Tea House", -52, true), network("Loft", -80, false)],
        };
        let mut out = Vec::new();
        print_wifi(&wifi, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "wifi         connected to Tea House (wlan0)\n\
             \x20 * Tea House  ▂▄▆█   -52 dBm  secured, saved\n\
             \x20   Loft       ▂···   -80 dBm  secured\n"
        );

        let mut buds = device("Buds", "AA:BB:CC:00:00:01");
        buds.connected = true;
        buds.battery = Some(80);
        let bluetooth = Bluetooth {
            powered: true,
            discovering: false,
            devices: vec![buds, device("Mouse", "AA:BB:CC:00:00:03")],
        };
        let mut out = Vec::new();
        print_bluetooth(&bluetooth, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "bluetooth    on\n\
             \x20 * Buds   AA:BB:CC:00:00:01  connected  80%\n\
             \x20   Mouse  AA:BB:CC:00:00:03  paired\n"
        );
    }
}
