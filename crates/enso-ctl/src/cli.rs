//! Command-line grammar for `enso-ctl`.

use std::ffi::OsString;

pub const HELP: &str = "\
enso-ctl — control center for the enso desktop

Usage: enso-ctl [COMMAND]

With no command, opens the control center panel.

Wi-Fi (iwd):
  wifi                      Radio, connection, and the networks in range
  wifi on | off             Turn the radio on or off
  wifi scan                 Look for networks, then list them
  wifi connect <NAME>       Connect; asks for the passphrase if one is needed
  wifi disconnect           Drop the current connection
  wifi forget <NAME>        Forget a saved network

Bluetooth (BlueZ):
  bluetooth                 Radio and known devices
  bluetooth on | off        Turn the radio on or off
  bluetooth scan [SECONDS]  Look for nearby devices (8 seconds), then list
  bluetooth connect <DEVICE>     Connect, pairing first if the device is new
  bluetooth disconnect <DEVICE>  Disconnect; it stays paired
  bluetooth forget <DEVICE>      Unpair and forget
                            <DEVICE> is a name (or the start of one) or an address

Sound (PipeWire):
  volume                    The output's volume, and each application's
  volume N | up | down [STEP]
                            Set it to N percent, or step it (5 unless STEP)
  volume mute | unmute | toggle
  volume app <NAME> <N | up | down | mute | unmute | toggle>
                            The same for one application that is playing
  volume output [NAME]      List the outputs, or play through the one named
  mic                       The microphone's level, and the microphones there are
  mic N | up | down [STEP] | mute | unmute | toggle
  mic input <NAME>          Record from the microphone named
                            A device NAME may be any distinctive part of it

Hardware:
  power                     Battery, power profile, temperature, fans, brightness
  brightness [N | up | down [STEP]]
                            Show the screen brightness, set it to N percent,
                            or step it (5 percent unless STEP is given)
  brightness keyboard [N | up | down]
                            The same for the keyboard light, one level a step
  profile                   Show the power profile and the choices
  profile set <NAME> | next Switch power profile

Theme:
  theme [list]              List available themes
  theme current             Print the active theme's id
  theme set <ID>            Switch theme: enso at once, and with it Niri,
                            Alacritty, Neovim, GTK, the cursor, Firefox, Qt,
                            and Flatpak
  theme next                Switch to the next theme
  theme apply               Apply the active theme again, and say how each
                            of those programs stands
  theme show [ID]           Print a theme's palette
  theme dirs                Print where themes are loaded from

Cursor:
  cursor [list]             List installed cursor themes, the one in use marked
  cursor set <NAME> [SIZE]  Use this cursor theme everywhere: Niri, GTK, and
                            programs that ask for none (SIZE in pixels, kept
                            unless given)
  cursor size <N>           Change only the size

Options:
  -h, --help                Print this help
  -V, --version             Print the version
";

/// Which screen edge a dropdown panel hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// How the panel is presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelMode {
    /// Opened on purpose (a key binding): takes the keyboard, and stays
    /// until dismissed.
    Modal,
    /// Opened by `enso-bar` under the pointer: follows the pointer, and
    /// leaves the keyboard alone until clicked.
    Dropdown(Edge),
}

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the control center panel.
    Panel(PanelMode),
    Help,
    Version,
    Theme(ThemeCommand),
    Cursor(CursorCommand),
    Power,
    Brightness(Light, Level),
    Profile(ProfileCommand),
    Wifi(WifiCommand),
    Bluetooth(BluetoothCommand),
    /// Change the volume of the output, or of the application named.
    Volume(Option<String>, VolumeChange),
    /// Change the microphone's level.
    Mic(VolumeChange),
    /// List the outputs (or, for `input`, the microphones), or switch to
    /// the one named.
    SoundDevice {
        input: bool,
        name: Option<String>,
    },
}

/// What to do with a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeChange {
    Level(Level),
    Mute(bool),
    ToggleMute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeCommand {
    List,
    Current,
    Set(String),
    Next,
    Apply,
    Show(Option<String>),
    Dirs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorCommand {
    List,
    /// A theme by name, at this size or the one in use.
    Set(String, Option<u32>),
    Size(u32),
}

/// Which backlight `brightness` is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Light {
    Screen,
    Keyboard,
}

/// What to do with a backlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Show,
    /// Set to this many percent.
    Set(u8),
    /// Raise by this many percent; `None` is the light's natural step.
    Up(Option<u8>),
    Down(Option<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileCommand {
    Show,
    Set(String),
    Next,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiCommand {
    Status,
    Power(bool),
    Scan,
    Connect(String),
    Disconnect,
    Forget(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BluetoothCommand {
    Status,
    Power(bool),
    /// Look around for this many seconds.
    Scan(u64),
    Connect(String),
    Disconnect(String),
    Forget(String),
}

/// How long `bluetooth scan` looks when not told.
const DEFAULT_SCAN_SECONDS: u64 = 8;

/// Parse arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = impl Into<OsString>>) -> Result<Command, String> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut words: Vec<String> = Vec::new();
    let mut mode = PanelMode::Modal;
    while let Some(arg) = parser.next().map_err(|err| err.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            // Used by enso-bar; `--dropdown=bottom` for a bar at the bottom.
            Long("dropdown") => {
                let edge = parser
                    .optional_value()
                    .map(|value| value.string())
                    .transpose()
                    .map_err(|err| err.to_string())?;
                mode = PanelMode::Dropdown(match edge.as_deref() {
                    None | Some("top") => Edge::Top,
                    Some("bottom") => Edge::Bottom,
                    Some(other) => return Err(format!("unknown edge `{other}`")),
                });
            }
            Value(value) => words.push(value.string().map_err(|err| err.to_string())?),
            other => return Err(other.unexpected().to_string()),
        }
    }

    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    // Names with spaces need not be quoted: `wifi connect My Network`.
    let name = |words: &[&str]| words.join(" ");
    match words.as_slice() {
        [] => Ok(Command::Panel(mode)),
        _ if mode != PanelMode::Modal => Err("`--dropdown` only opens the panel".to_owned()),

        ["theme"] | ["theme", "list"] => Ok(Command::Theme(ThemeCommand::List)),
        ["theme", "current"] => Ok(Command::Theme(ThemeCommand::Current)),
        ["theme", "set", id] => Ok(Command::Theme(ThemeCommand::Set((*id).to_owned()))),
        ["theme", "set"] => {
            Err("`theme set` needs a theme id (see `enso-ctl theme list`)".to_owned())
        }
        ["theme", "next"] => Ok(Command::Theme(ThemeCommand::Next)),
        ["theme", "apply"] => Ok(Command::Theme(ThemeCommand::Apply)),
        ["theme", "show"] => Ok(Command::Theme(ThemeCommand::Show(None))),
        ["theme", "show", id] => Ok(Command::Theme(ThemeCommand::Show(Some((*id).to_owned())))),
        ["theme", "dirs"] => Ok(Command::Theme(ThemeCommand::Dirs)),
        ["theme", other, ..] => Err(format!("unknown theme command `{other}`")),
        ["cursor"] | ["cursor", "list"] => Ok(Command::Cursor(CursorCommand::List)),
        ["cursor", "set", name] => Ok(Command::Cursor(CursorCommand::Set(
            (*name).to_owned(),
            None,
        ))),
        ["cursor", "set", name, size] => cursor_size(size)
            .map(|size| Command::Cursor(CursorCommand::Set((*name).to_owned(), Some(size)))),
        ["cursor", "set"] => {
            Err("`cursor set` needs a cursor theme (see `enso-ctl cursor list`)".to_owned())
        }
        ["cursor", "size", size] => {
            cursor_size(size).map(|size| Command::Cursor(CursorCommand::Size(size)))
        }
        ["cursor", other, ..] => Err(format!("unknown cursor command `{other}`")),

        ["power"] => Ok(Command::Power),
        ["brightness", "keyboard", rest @ ..] => {
            level(rest).map(|level| Command::Brightness(Light::Keyboard, level))
        }
        ["brightness", rest @ ..] => {
            level(rest).map(|level| Command::Brightness(Light::Screen, level))
        }

        ["profile"] => Ok(Command::Profile(ProfileCommand::Show)),
        ["profile", "next"] => Ok(Command::Profile(ProfileCommand::Next)),
        ["profile", "set", profile] => {
            Ok(Command::Profile(ProfileCommand::Set((*profile).to_owned())))
        }
        ["profile", "set"] => {
            Err("`profile set` needs a profile (see `enso-ctl profile`)".to_owned())
        }
        ["profile", other, ..] => Err(format!("unknown profile command `{other}`")),

        ["wifi"] | ["wifi", "status" | "list"] => Ok(Command::Wifi(WifiCommand::Status)),
        ["wifi", "on"] => Ok(Command::Wifi(WifiCommand::Power(true))),
        ["wifi", "off"] => Ok(Command::Wifi(WifiCommand::Power(false))),
        ["wifi", "scan"] => Ok(Command::Wifi(WifiCommand::Scan)),
        ["wifi", "disconnect"] => Ok(Command::Wifi(WifiCommand::Disconnect)),
        ["wifi", "connect", network @ ..] if !network.is_empty() => {
            Ok(Command::Wifi(WifiCommand::Connect(name(network))))
        }
        ["wifi", "forget", network @ ..] if !network.is_empty() => {
            Ok(Command::Wifi(WifiCommand::Forget(name(network))))
        }
        ["wifi", action @ ("connect" | "forget")] => Err(format!(
            "`wifi {action}` needs a network name (see `enso-ctl wifi`)"
        )),
        ["wifi", other, ..] => Err(format!("unknown wifi command `{other}`")),

        ["bluetooth"] | ["bluetooth", "status" | "list"] => {
            Ok(Command::Bluetooth(BluetoothCommand::Status))
        }
        ["bluetooth", "on"] => Ok(Command::Bluetooth(BluetoothCommand::Power(true))),
        ["bluetooth", "off"] => Ok(Command::Bluetooth(BluetoothCommand::Power(false))),
        ["bluetooth", "scan"] => Ok(Command::Bluetooth(BluetoothCommand::Scan(
            DEFAULT_SCAN_SECONDS,
        ))),
        ["bluetooth", "scan", seconds] => seconds
            .parse()
            .map(|seconds| Command::Bluetooth(BluetoothCommand::Scan(seconds)))
            .map_err(|_| format!("`{seconds}` is not a number of seconds")),
        [
            "bluetooth",
            action @ ("connect" | "disconnect" | "forget"),
            device @ ..,
        ] => {
            if device.is_empty() {
                return Err(format!(
                    "`bluetooth {action}` needs a device (see `enso-ctl bluetooth`)"
                ));
            }
            let device = name(device);
            Ok(Command::Bluetooth(match *action {
                "connect" => BluetoothCommand::Connect(device),
                "disconnect" => BluetoothCommand::Disconnect(device),
                _ => BluetoothCommand::Forget(device),
            }))
        }
        ["bluetooth", other, ..] => Err(format!("unknown bluetooth command `{other}`")),

        ["volume", "app"] => Err("`volume app` needs an application and what to do".to_owned()),
        ["volume", "app", name @ .., action] if !name.is_empty() => {
            volume(&[action]).map(|change| Command::Volume(Some(name.join(" ")), change))
        }
        ["volume", "app", name] => Err(format!(
            "what should happen to `{name}`? (a percentage, up, down, mute, unmute, toggle)"
        )),
        ["volume", "output", device @ ..] => Ok(Command::SoundDevice {
            input: false,
            name: (!device.is_empty()).then(|| name(device)),
        }),
        ["volume", rest @ ..] => volume(rest).map(|change| Command::Volume(None, change)),
        ["mic", "input", device @ ..] => Ok(Command::SoundDevice {
            input: true,
            name: (!device.is_empty()).then(|| name(device)),
        }),
        ["mic", rest @ ..] => volume(rest).map(Command::Mic),
        [other, ..] => Err(format!("unknown command `{other}`")),
    }
}

/// The part of a `volume` command that says what to do.
/// A cursor size: whole pixels. Whether it is a sensible one is the
/// cursor module's to say.
fn cursor_size(word: &str) -> Result<u32, String> {
    word.parse()
        .map_err(|_| format!("`{word}` is not a cursor size in pixels"))
}

fn volume(words: &[&str]) -> Result<VolumeChange, String> {
    match words {
        ["mute"] => Ok(VolumeChange::Mute(true)),
        ["unmute"] => Ok(VolumeChange::Mute(false)),
        ["toggle"] => Ok(VolumeChange::ToggleMute),
        other => level(other)
            .map(VolumeChange::Level)
            .map_err(|err| format!("{err}, `mute`, `unmute`, or `toggle`")),
    }
}

/// The part of a `brightness` command after the light.
fn level(words: &[&str]) -> Result<Level, String> {
    let percent = |word: &str| {
        word.trim_end_matches('%')
            .parse::<u8>()
            .ok()
            .filter(|percent| *percent <= 100)
            .ok_or_else(|| format!("`{word}` is not a percentage from 0 to 100"))
    };
    match words {
        [] | ["get"] => Ok(Level::Show),
        ["up"] => Ok(Level::Up(None)),
        ["down"] => Ok(Level::Down(None)),
        ["up", step] => percent(step).map(|step| Level::Up(Some(step))),
        ["down", step] => percent(step).map(|step| Level::Down(Some(step))),
        ["set", value] | [value] => percent(value).map(Level::Set),
        _ => Err("expected a percentage, `up`, or `down`".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Command, String> {
        parse(words.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_opens_the_panel() {
        assert_eq!(parse_words(&[]), Ok(Command::Panel(PanelMode::Modal)));
        assert_eq!(
            parse_words(&["--dropdown"]),
            Ok(Command::Panel(PanelMode::Dropdown(Edge::Top)))
        );
        assert_eq!(
            parse_words(&["--dropdown=bottom"]),
            Ok(Command::Panel(PanelMode::Dropdown(Edge::Bottom)))
        );
        assert!(parse_words(&["--dropdown=left"]).is_err());
        assert!(parse_words(&["--dropdown", "wifi"]).is_err());
    }

    #[test]
    fn parses_cursor_commands() {
        use CursorCommand::*;
        assert_eq!(parse_words(&["cursor"]), Ok(Command::Cursor(List)));
        assert_eq!(parse_words(&["cursor", "list"]), Ok(Command::Cursor(List)));
        assert_eq!(
            parse_words(&["cursor", "set", "Bibata-Modern-Ice"]),
            Ok(Command::Cursor(Set("Bibata-Modern-Ice".into(), None)))
        );
        assert_eq!(
            parse_words(&["cursor", "set", "Bibata-Modern-Ice", "32"]),
            Ok(Command::Cursor(Set("Bibata-Modern-Ice".into(), Some(32))))
        );
        assert_eq!(
            parse_words(&["cursor", "size", "28"]),
            Ok(Command::Cursor(Size(28)))
        );
        assert!(parse_words(&["cursor", "set"]).is_err());
        assert!(parse_words(&["cursor", "size", "big"]).is_err());
        assert!(parse_words(&["cursor", "set", "Bibata", "-1"]).is_err());
        assert!(parse_words(&["cursor", "spin"]).is_err());
    }

    #[test]
    fn parses_theme_commands() {
        use ThemeCommand::*;
        assert_eq!(parse_words(&["theme"]), Ok(Command::Theme(List)));
        assert_eq!(parse_words(&["theme", "list"]), Ok(Command::Theme(List)));
        assert_eq!(
            parse_words(&["theme", "current"]),
            Ok(Command::Theme(Current))
        );
        assert_eq!(parse_words(&["theme", "next"]), Ok(Command::Theme(Next)));
        assert_eq!(parse_words(&["theme", "apply"]), Ok(Command::Theme(Apply)));
        assert_eq!(
            parse_words(&["theme", "set", "rose-pine-dawn"]),
            Ok(Command::Theme(Set("rose-pine-dawn".into())))
        );
        assert_eq!(
            parse_words(&["theme", "show"]),
            Ok(Command::Theme(Show(None)))
        );
        assert_eq!(
            parse_words(&["theme", "show", "x"]),
            Ok(Command::Theme(Show(Some("x".into()))))
        );
    }

    #[test]
    fn parses_hardware_commands() {
        use Light::*;
        let brightness = |words: &[&str]| parse_words(&[&["brightness"], words].concat());
        assert_eq!(parse_words(&["power"]), Ok(Command::Power));
        assert_eq!(
            brightness(&[]),
            Ok(Command::Brightness(Screen, Level::Show))
        );
        assert_eq!(
            brightness(&["40"]),
            Ok(Command::Brightness(Screen, Level::Set(40)))
        );
        assert_eq!(
            brightness(&["set", "40%"]),
            Ok(Command::Brightness(Screen, Level::Set(40)))
        );
        assert_eq!(
            brightness(&["up"]),
            Ok(Command::Brightness(Screen, Level::Up(None)))
        );
        assert_eq!(
            brightness(&["down", "10"]),
            Ok(Command::Brightness(Screen, Level::Down(Some(10))))
        );
        assert_eq!(
            brightness(&["keyboard"]),
            Ok(Command::Brightness(Keyboard, Level::Show))
        );
        assert_eq!(
            brightness(&["keyboard", "up"]),
            Ok(Command::Brightness(Keyboard, Level::Up(None)))
        );
        assert!(brightness(&["101"]).is_err());
        assert!(brightness(&["bright"]).is_err());

        assert_eq!(
            parse_words(&["profile"]),
            Ok(Command::Profile(ProfileCommand::Show))
        );
        assert_eq!(
            parse_words(&["profile", "next"]),
            Ok(Command::Profile(ProfileCommand::Next))
        );
        assert_eq!(
            parse_words(&["profile", "set", "quiet"]),
            Ok(Command::Profile(ProfileCommand::Set("quiet".into())))
        );
        assert!(parse_words(&["profile", "set"]).is_err());
    }

    #[test]
    fn parses_network_commands() {
        use BluetoothCommand as Bt;
        use WifiCommand as Wifi;
        let wifi = |words: &[&str]| parse_words(&[&["wifi"], words].concat());
        assert_eq!(wifi(&[]), Ok(Command::Wifi(Wifi::Status)));
        assert_eq!(wifi(&["list"]), Ok(Command::Wifi(Wifi::Status)));
        assert_eq!(wifi(&["on"]), Ok(Command::Wifi(Wifi::Power(true))));
        assert_eq!(wifi(&["off"]), Ok(Command::Wifi(Wifi::Power(false))));
        assert_eq!(wifi(&["scan"]), Ok(Command::Wifi(Wifi::Scan)));
        assert_eq!(wifi(&["disconnect"]), Ok(Command::Wifi(Wifi::Disconnect)));
        // A name with spaces works quoted or not.
        assert_eq!(
            wifi(&["connect", "Tea House 5G"]),
            Ok(Command::Wifi(Wifi::Connect("Tea House 5G".into())))
        );
        assert_eq!(
            wifi(&["connect", "Tea", "House", "5G"]),
            Ok(Command::Wifi(Wifi::Connect("Tea House 5G".into())))
        );
        assert_eq!(
            wifi(&["forget", "Old"]),
            Ok(Command::Wifi(Wifi::Forget("Old".into())))
        );
        assert!(wifi(&["connect"]).is_err());
        assert!(wifi(&["dance"]).is_err());

        let bluetooth = |words: &[&str]| parse_words(&[&["bluetooth"], words].concat());
        assert_eq!(bluetooth(&[]), Ok(Command::Bluetooth(Bt::Status)));
        assert_eq!(
            bluetooth(&["off"]),
            Ok(Command::Bluetooth(Bt::Power(false)))
        );
        assert_eq!(bluetooth(&["scan"]), Ok(Command::Bluetooth(Bt::Scan(8))));
        assert_eq!(
            bluetooth(&["scan", "3"]),
            Ok(Command::Bluetooth(Bt::Scan(3)))
        );
        assert_eq!(
            bluetooth(&["connect", "Soft", "Buds"]),
            Ok(Command::Bluetooth(Bt::Connect("Soft Buds".into())))
        );
        assert_eq!(
            bluetooth(&["disconnect", "AA:BB:CC:DD:EE:FF"]),
            Ok(Command::Bluetooth(Bt::Disconnect(
                "AA:BB:CC:DD:EE:FF".into()
            )))
        );
        assert_eq!(
            bluetooth(&["forget", "Buds"]),
            Ok(Command::Bluetooth(Bt::Forget("Buds".into())))
        );
        assert!(bluetooth(&["connect"]).is_err());
        assert!(bluetooth(&["scan", "soon"]).is_err());
    }

    #[test]
    fn parses_volume_commands() {
        use VolumeChange::*;
        let volume = |words: &[&str]| parse_words(&[&["volume"], words].concat());
        assert_eq!(
            volume(&[]),
            Ok(Command::Volume(None, Level(self::Level::Show)))
        );
        assert_eq!(
            volume(&["35"]),
            Ok(Command::Volume(None, Level(self::Level::Set(35))))
        );
        assert_eq!(
            volume(&["up"]),
            Ok(Command::Volume(None, Level(self::Level::Up(None))))
        );
        assert_eq!(
            volume(&["down", "2"]),
            Ok(Command::Volume(None, Level(self::Level::Down(Some(2)))))
        );
        assert_eq!(volume(&["mute"]), Ok(Command::Volume(None, Mute(true))));
        assert_eq!(volume(&["unmute"]), Ok(Command::Volume(None, Mute(false))));
        assert_eq!(volume(&["toggle"]), Ok(Command::Volume(None, ToggleMute)));
        // An application's name may have spaces; the last word is what to do.
        assert_eq!(
            volume(&["app", "Telegram", "Desktop", "40"]),
            Ok(Command::Volume(
                Some("Telegram Desktop".into()),
                Level(self::Level::Set(40))
            ))
        );
        assert_eq!(
            volume(&["app", "Firefox", "toggle"]),
            Ok(Command::Volume(Some("Firefox".into()), ToggleMute))
        );
        assert_eq!(
            volume(&["output"]),
            Ok(Command::SoundDevice {
                input: false,
                name: None
            })
        );
        assert_eq!(
            volume(&["output", "Soft", "Buds"]),
            Ok(Command::SoundDevice {
                input: false,
                name: Some("Soft Buds".into())
            })
        );
        assert_eq!(
            parse_words(&["mic"]),
            Ok(Command::Mic(Level(self::Level::Show)))
        );
        assert_eq!(
            parse_words(&["mic", "toggle"]),
            Ok(Command::Mic(ToggleMute))
        );
        assert_eq!(
            parse_words(&["mic", "70"]),
            Ok(Command::Mic(Level(self::Level::Set(70))))
        );
        assert_eq!(
            parse_words(&["mic", "input", "webcam"]),
            Ok(Command::SoundDevice {
                input: true,
                name: Some("webcam".into())
            })
        );
        assert!(parse_words(&["mic", "loud"]).is_err());
        assert!(volume(&["app"]).is_err());
        assert!(volume(&["app", "Firefox"]).is_err());
        assert!(volume(&["loud"]).is_err());
    }

    #[test]
    fn flags_and_errors() {
        assert_eq!(parse_words(&["--help"]), Ok(Command::Help));
        assert_eq!(parse_words(&["theme", "-h"]), Ok(Command::Help));
        assert_eq!(parse_words(&["-V"]), Ok(Command::Version));
        assert!(parse_words(&["audio", "up"]).is_err());
        assert!(parse_words(&["theme", "set"]).is_err());
        assert!(parse_words(&["theme", "paint"]).is_err());
        assert!(parse_words(&["nope"]).is_err());
        assert!(parse_words(&["--nope"]).is_err());
    }
}
