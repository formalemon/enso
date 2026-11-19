use std::sync::mpsc::Receiver;

use enso_core::Paths;
use enso_sys::{
    audio::{App as AudioApp, Device as SoundDevice, Output},
    power::{Backlight, BacklightKind, Battery, BatteryStatus, Fan, PlatformProfile},
};
use enso_ui::{Mods, Offscreen, TextEngine};

use super::*;

#[test]
fn example_config_documents_the_defaults() {
    let example = include_str!("../../../../config.example.toml");
    let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
    assert_eq!(
        config.section::<CtlConfig>("ctl").unwrap(),
        CtlConfig::default()
    );
}

// ---- Fixtures ----------------------------------------------------------------

fn hardware() -> Hardware {
    Hardware {
        battery: Some(Battery {
            name: "BAT1".into(),
            percent: 82,
            status: BatteryStatus::Discharging,
            power_mw: Some(9_400),
            minutes_left: Some(135),
        }),
        profile: Some(PlatformProfile {
            current: "balanced".into(),
            choices: vec!["quiet".into(), "balanced".into(), "performance".into()],
            writable: true,
        }),
        temperature: Some(54.0),
        fans: vec![Fan {
            label: "cpu_fan".into(),
            rpm: 2400,
        }],
        screen: Some(Backlight {
            kind: BacklightKind::Screen,
            name: "intel_backlight".into(),
            brightness: 400,
            max: 1000,
        }),
    }
}

fn app(name: &str, volume: u8, muted: bool) -> AudioApp {
    AudioApp {
        name: name.to_owned(),
        volume,
        muted,
    }
}

fn sound() -> Sound {
    Sound {
        output: Some(Output {
            name: "Speakers".into(),
            volume: 60,
            muted: false,
        }),
        outputs: vec![
            sound_device("Laptop Speakers", true),
            sound_device("Soft Buds", false),
        ],
        apps: vec![
            app("Firefox", 80, false),
            app("Music Player", 45, true),
            app("Telegram Desktop", 100, false),
        ],
        input: Some(Output {
            name: "Laptop Microphone".into(),
            volume: 70,
            muted: false,
        }),
        inputs: vec![
            sound_device("Laptop Microphone", true),
            sound_device("Soft Buds", false),
        ],
    }
}

fn sound_device(name: &str, current: bool) -> SoundDevice {
    SoundDevice {
        id: name.to_lowercase().replace(' ', "-"),
        name: name.to_owned(),
        current,
    }
}

fn network(name: &str, security: Security, signal_dbm: i16) -> Network {
    Network {
        id: format!("/net/{}", name.to_lowercase().replace(' ', "-")),
        name: name.to_owned(),
        security,
        signal_dbm,
        connected: false,
        known: false,
    }
}

/// Nine networks — more than fit — the first one connected.
fn wifi() -> Wifi {
    use Security::{Enterprise, Open, Passphrase, Wep};
    let mut networks = vec![
        network("Tea House", Passphrase, -52),
        network("Loft 5G", Passphrase, -58),
        network("Bakery Guest", Open, -64),
        network("Studio Mira", Passphrase, -70),
        network("Corner Office", Enterprise, -74),
        network("Garden Shed", Passphrase, -79),
        network("Old Router", Wep, -83),
        network("Moss", Passphrase, -86),
        network("Far Away", Passphrase, -91),
    ];
    networks[0].connected = true;
    networks[0].known = true;
    networks[3].known = true;
    Wifi {
        interface: "wlan0".into(),
        powered: true,
        link: Link::Connected,
        scanning: false,
        networks,
    }
}

fn device(name: &str, paired: bool, connected: bool, battery: Option<u8>) -> Device {
    Device {
        id: format!("/dev/{}", name.to_lowercase().replace(' ', "-")),
        name: name.to_owned(),
        address: "AA:BB:CC:00:00:01".into(),
        kind: None,
        paired,
        connected,
        battery,
    }
}

fn bluetooth() -> Bluetooth {
    Bluetooth {
        powered: true,
        discovering: true,
        devices: vec![
            device("Soft Buds", true, true, Some(80)),
            device("Trackball", true, false, None),
            device("Kitchen Speaker", false, false, None),
        ],
    }
}

/// A panel with a full machine behind it, and the requests it sends.
fn panel_in(name: &str, mode: PanelMode) -> (Panel, Receiver<Request>) {
    let root = std::env::temp_dir().join(format!("enso-ctl-panel-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut panel = Panel::new(
        Context::load_from(Paths::rooted(root)),
        CtlConfig::default(),
        mode,
    );
    let (system, requests) = System::detached();
    panel.system = system;
    panel.apply(Update::Hardware(hardware()));
    panel.apply(Update::Sound(Some(sound())));
    panel.apply(Update::Wifi(Some(wifi())));
    panel.apply(Update::Bluetooth(Some(bluetooth())));
    (panel, requests)
}

fn panel(name: &str) -> (Panel, Receiver<Request>) {
    panel_in(name, PanelMode::Modal)
}

fn sent(requests: &Receiver<Request>) -> Vec<Request> {
    requests.try_iter().collect()
}

fn key(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        mods: Mods::default(),
        repeat: false,
    }
}

fn press(panel: &mut Panel, key_: Key) -> Outcome {
    panel.on_key(key(key_))
}

fn type_text(panel: &mut Panel, text: &str) {
    for c in text.chars() {
        assert_eq!(press(panel, Key::Char(c)), Outcome::Changed);
    }
}

const SIZE: (u32, u32) = (420, 520);

/// Draw the panel at rest: once to start its animations, then long after.
fn render(panel: &mut Panel) -> Offscreen {
    let later = Instant::now() + Duration::from_secs(2);
    let mut text = TextEngine::new(&panel.ctx.config.appearance);
    Offscreen::render(&mut text, SIZE, 2, |canvas| {
        panel.draw(canvas, Instant::now());
        canvas.clear(enso_core::Rgba::TRANSPARENT);
        assert_eq!(panel.draw(canvas, later), Paint::Idle);
    })
}

/// The pictures held to a reference in `snapshots/`; the rest are
/// only written, to look at.
const CHECKED: &[&str] = &["ctl-panel-rose-pine-moon"];

fn snapshot(frame: &Offscreen, name: &str) {
    if CHECKED.contains(&name) {
        frame.check(env!("CARGO_MANIFEST_DIR"), name);
    } else {
        frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
    }
}

/// Centre of the first hit area for `hit`.
fn centre_of(panel: &Panel, wanted: impl Fn(&Hit) -> bool) -> (f32, f32) {
    panel
        .hits
        .iter()
        .find(|(_, hit)| wanted(hit))
        .map(|(rect, _)| rect.center())
        .expect("hit area")
}

fn pixel(frame: &Offscreen, x: f32, y: f32) -> [u8; 4] {
    frame.pixel((x * 2.0) as u32, (y * 2.0) as u32)
}

// ---- The first page ------------------------------------------------------------

#[test]
fn the_first_page_shows_readings_sliders_and_entries() {
    for theme in ["rose-pine-moon", "rose-pine-dawn"] {
        let (mut panel, _requests) = panel(&format!("main-{theme}"));
        enso_core::theme::set_active(&panel.ctx.paths, theme).unwrap();
        panel.ctx.reload_theme();
        let frame = render(&mut panel);

        assert_eq!(
            panel.items(),
            [
                Item::Slider(Dial::Brightness),
                Item::Slider(Dial::Volume),
                Item::Open(Page::Wifi),
                Item::Open(Page::Bluetooth),
                Item::Open(Page::Theme),
            ]
        );
        // Everything fits inside the card, which sits in the corner.
        assert_eq!(panel.card.h, panel.height_target());
        assert!(panel.hits.iter().all(|(rect, _)| {
            panel.card.contains(rect.x, rect.y)
                && panel.card.contains(rect.right() - 1.0, rect.bottom() - 1.0)
        }));
        assert_eq!(panel.card.right(), SIZE.0 as f32 - MARGIN);
        assert_eq!(pixel(&frame, 4.0, 300.0)[3], 0, "outside the card");
        let surface = panel.ctx.theme.colors.surface;
        assert_eq!(
            pixel(&frame, panel.card.x + 4.0, panel.card.y + 40.0),
            [surface.r, surface.g, surface.b, 255]
        );
        // Both radios have a switch ahead of their row.
        let switches: Vec<Page> = panel
            .hits
            .iter()
            .filter_map(|(_, hit)| match hit {
                Hit::Switch(page) => Some(*page),
                _ => None,
            })
            .collect();
        assert_eq!(switches, [Page::Wifi, Page::Bluetooth]);

        snapshot(&frame, &format!("ctl-panel-{theme}"));
    }
}

#[test]
fn tiles_say_what_matters() {
    let read = |hardware: &Hardware| -> Vec<(String, String)> {
        draw::tiles(hardware)
            .into_iter()
            .map(|tile| {
                let debug = format!("{tile:?}");
                let field = |name: &str| {
                    let start = debug.find(&format!("{name}: \"")).unwrap() + name.len() + 3;
                    debug[start..].split('"').next().unwrap().to_owned()
                };
                (field("value"), field("label"))
            })
            .collect()
    };
    let pair = |value: &str, label: &str| (value.to_owned(), label.to_owned());

    let mut hardware = hardware();
    assert_eq!(
        read(&hardware),
        [
            pair("82%", "2h 15m left"),
            pair("Balanced", "Profile"),
            pair("54°", "2400 rpm"),
        ]
    );

    let battery = hardware.battery.as_mut().unwrap();
    battery.status = BatteryStatus::Charging;
    battery.minutes_left = Some(40);
    hardware.fans[0].rpm = 0;
    assert_eq!(read(&hardware)[0], pair("82%", "40m to full"));
    assert_eq!(read(&hardware)[2], pair("54°", "CPU"));

    // A desktop: no battery, no profile, nothing to show for them.
    assert!(draw::tiles(&Hardware::default()).is_empty());
}

#[test]
fn keys_move_adjust_and_open() {
    let (mut panel, requests) = panel("keys");
    render(&mut panel);
    assert_eq!(panel.selected, 0);

    // On a slider, left and right (or h and l) adjust it: 5 % of the screen…
    assert_eq!(press(&mut panel, Key::Char('l')), Outcome::Changed);
    assert_eq!(sent(&requests), [Request::Backlight(450)]);
    assert_eq!(panel.hardware.screen.as_ref().unwrap().brightness, 450);
    press(&mut panel, Key::Left);
    assert_eq!(sent(&requests), [Request::Backlight(400)]);

    // …and five points of the volume, which m mutes.
    press(&mut panel, Key::Char('j'));
    press(&mut panel, Key::Right);
    press(&mut panel, Key::Char('m'));
    assert_eq!(sent(&requests), [Request::Volume(65), Request::Mute(true)]);
    assert_eq!(panel.dial(Dial::Volume), Some((65, true)));
    press(&mut panel, Key::Char('m'));
    press(&mut panel, Key::Char('h'));
    assert_eq!(sent(&requests), [Request::Mute(false), Request::Volume(60)]);

    // Navigation wraps.
    press(&mut panel, Key::Char('k'));
    press(&mut panel, Key::Char('k'));
    assert_eq!(panel.items()[panel.selected], Item::Open(Page::Theme));
    press(&mut panel, Key::Down);
    assert_eq!(panel.selected, 0);

    // Enter (or l) on an entry opens its page; Esc comes back to the entry.
    for _ in 0..2 {
        press(&mut panel, Key::Down);
    }
    assert_eq!(press(&mut panel, Key::Enter), Outcome::Changed);
    assert_eq!(panel.page, Page::Wifi);
    assert_eq!(sent(&requests), [Request::WifiScan]);
    assert_eq!(press(&mut panel, Key::Escape), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);
    assert_eq!(panel.items()[panel.selected], Item::Open(Page::Wifi));

    // Shortcuts need to be typed plainly, and Esc on the first page closes.
    let ctrl_q = KeyEvent {
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
        ..key(Key::Char('q'))
    };
    assert_eq!(panel.on_key(ctrl_q), Outcome::Ignored);
    assert_eq!(press(&mut panel, Key::Escape), Outcome::Close);
    assert_eq!(press(&mut panel, Key::Char('q')), Outcome::Close);
}

#[test]
fn a_slider_follows_the_pointer_and_stays_ahead_of_the_system() {
    let (mut panel, requests) = panel("drag");
    render(&mut panel);
    let track = panel
        .hits
        .iter()
        .find_map(|(_, hit)| match hit {
            Hit::Slider(_, Dial::Brightness, track) => Some(*track),
            _ => None,
        })
        .unwrap();
    let (_, y) = track.center();

    assert_eq!(
        panel.on_press(track.x + track.w * 0.75, y),
        Outcome::Changed
    );
    assert_eq!(sent(&requests), [Request::Backlight(750)]);
    // The reading that was true a moment ago does not pull the knob back.
    panel.apply(Update::Hardware(hardware()));
    assert_eq!(panel.hardware.screen.as_ref().unwrap().brightness, 750);

    // Past the end is the end; a screen's lowest step is still lit.
    assert_eq!(
        panel.on_move(track.right() + 80.0, y + 60.0),
        Outcome::Changed
    );
    assert_eq!(panel.on_move(track.x - 80.0, y), Outcome::Changed);
    assert_eq!(
        sent(&requests),
        [Request::Backlight(1000), Request::Backlight(1),]
    );

    // Released, the pointer goes back to selecting rows.
    panel.dragging = None;
    let (x, y) = centre_of(&panel, |hit| matches!(hit, Hit::Item(_)));
    assert_eq!(panel.on_move(x, y), Outcome::Changed);
    assert!(sent(&requests).is_empty());
    assert_eq!(panel.items()[panel.selected], Item::Open(Page::Wifi));
}

// ---- Sound ---------------------------------------------------------------------

fn item_at(panel: &Panel, item: Item) -> usize {
    panel
        .items()
        .iter()
        .position(|candidate| *candidate == item)
        .unwrap_or_else(|| panic!("no {item:?} on this page"))
}

#[test]
fn the_volume_leads_to_a_sound_page() {
    let (mut panel, requests) = panel("sound");
    render(&mut panel);

    // The chevron at the end of the volume row opens it, as does Enter.
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::OpenSound);
    assert_eq!(panel.on_press(x, y), Outcome::Changed);
    assert_eq!(panel.page, Page::Sound);
    press(&mut panel, Key::Escape);
    panel.selected = item_at(&panel, Item::Slider(Dial::Volume));
    press(&mut panel, Key::Enter);
    assert_eq!(panel.page, Page::Sound);

    // Outputs first, then applications, then the microphone.
    use Item::{Heading, Input, Output, Slider};
    assert_eq!(
        panel.items(),
        [
            Heading(Section::Output),
            Slider(Dial::Volume),
            Output(0),
            Output(1),
            Heading(Section::Apps),
            Slider(Dial::App(0)),
            Slider(Dial::App(1)),
            Slider(Dial::App(2)),
            Heading(Section::Microphone),
            Slider(Dial::Mic),
            Input(0),
            Input(1),
        ]
    );
    // The selection starts on the first thing that can be chosen, and the
    // heading above it is in view.
    assert_eq!((panel.selected, panel.offset), (1, 0));
    let frame = render(&mut panel);
    assert_eq!(panel.card.h, panel.height_target());
    snapshot(&frame, "ctl-sound");

    // Moving steps over headings, and wraps over them too.
    press(&mut panel, Key::Char('k'));
    assert_eq!(panel.items()[panel.selected], Input(1));
    press(&mut panel, Key::Char('j'));
    assert_eq!(panel.items()[panel.selected], Slider(Dial::Volume));
    for _ in 0..3 {
        press(&mut panel, Key::Char('j'));
    }
    assert_eq!(panel.items()[panel.selected], Slider(Dial::App(0)));

    // On a slider h and l adjust rather than go back; m mutes it.
    press(&mut panel, Key::Char('h'));
    press(&mut panel, Key::Char('m'));
    assert_eq!(panel.page, Page::Sound);
    assert_eq!(
        sent(&requests),
        [
            Request::AppVolume("Firefox".into(), 75),
            Request::AppMute("Firefox".into(), true),
        ]
    );
    // Anywhere else h goes back.
    panel.selected = item_at(&panel, Output(1));
    press(&mut panel, Key::Char('h'));
    assert_eq!(panel.page, Page::Main);
}

#[test]
fn the_sound_page_chooses_devices() {
    let (mut panel, requests) = panel("sound-devices");
    open(&mut panel, Page::Sound);
    use Item::{Input, Output};

    // Choosing another output switches to it, shown at once.
    panel.selected = item_at(&panel, Output(1));
    press(&mut panel, Key::Enter);
    assert_eq!(sent(&requests), [Request::Output("soft-buds".into())]);
    let current = |panel: &Panel, inputs: bool| -> Vec<bool> {
        panel
            .sound_devices(inputs)
            .iter()
            .map(|device| device.current)
            .collect()
    };
    assert_eq!(current(&panel, false), [false, true]);
    // Choosing the one in use does nothing.
    press(&mut panel, Key::Enter);
    assert!(sent(&requests).is_empty());

    // The same for microphones, with a click.
    render(&mut panel);
    panel.selected = item_at(&panel, Input(1));
    panel.settle();
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Item(item_at(&panel, Input(1))));
    panel.on_press(x, y);
    assert_eq!(sent(&requests), [Request::Input("soft-buds".into())]);
    assert_eq!(current(&panel, true), [false, true]);

    // A device that appears does not move the selection off its row.
    let mut more = sound();
    more.outputs.insert(0, sound_device("HDMI", false));
    panel.apply(Update::Sound(Some(more)));
    assert_eq!(panel.items()[panel.selected], Input(1));

    // Without a sound server there is no page to be on.
    panel.apply(Update::Sound(None));
    assert_eq!(panel.page, Page::Main);
    assert!(!panel.items().contains(&Item::Slider(Dial::Volume)));
}

#[test]
fn the_sound_page_has_a_slider_for_each_application() {
    let (mut panel, requests) = panel("sound-apps");
    open(&mut panel, Page::Sound);
    render(&mut panel);

    let track = panel
        .hits
        .iter()
        .find_map(|(_, hit)| match hit {
            Hit::Slider(_, Dial::App(0), track) => Some(*track),
            _ => None,
        })
        .unwrap();
    let (_, y) = track.center();
    panel.on_press(track.x + track.w * 0.25, y);
    assert_eq!(sent(&requests), [Request::AppVolume("Firefox".into(), 25)]);
    // It stays put under the pointer while the system catches up…
    panel.apply(Update::Sound(Some(sound())));
    assert_eq!(panel.dial(Dial::App(0)), Some((25, false)));
    // …and lets go if the list changes under it.
    let mut fewer = sound();
    fewer.apps.remove(0);
    panel.apply(Update::Sound(Some(fewer)));
    assert_eq!(panel.dragging, None);
    assert_eq!(panel.dial(Dial::App(0)), Some((45, true)));
    panel.apply(Update::Sound(Some(sound())));

    // Each has its own mute, on the speaker at its start.
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Mute(Dial::App(1)));
    panel.on_press(x, y);
    assert_eq!(
        sent(&requests),
        [Request::AppMute("Music Player".into(), false)]
    );

    // With nothing playing the page says so, and the line is just a line.
    let mut quiet = sound();
    quiet.apps.clear();
    panel.apply(Update::Sound(Some(quiet)));
    let silence = item_at(&panel, Item::Silence);
    panel.selected = silence;
    panel.settle();
    assert_ne!(panel.selected, silence);
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-sound-quiet");

    // More applications than fit are capped, and the page scrolls.
    let mut busy = sound();
    busy.apps = (0..10)
        .map(|i| app(&format!("App {i}"), 50, false))
        .collect();
    panel.apply(Update::Sound(Some(busy)));
    assert_eq!(panel.apps().len(), MAX_APPS);
    assert_eq!(panel.visible_rows(), MAX_SOUND_ROWS);
    assert!(panel.height_target() <= Panel::max_height());
    panel.selected = item_at(&panel, Item::Input(1));
    panel.settle();
    assert_eq!(panel.offset, panel.items().len() - MAX_SOUND_ROWS);
    render(&mut panel);
    assert!(panel.hits.iter().all(|(rect, _)| {
        panel.card.contains(rect.x, rect.y)
            && panel.card.contains(rect.right() - 1.0, rect.bottom() - 1.0)
    }));
}

#[test]
fn the_microphone_has_a_switch_a_level_and_a_choice() {
    let (mut panel, requests) = panel("mic");
    open(&mut panel, Page::Sound);
    render(&mut panel);

    // The switch on its heading turns it off and on.
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::MicSwitch);
    assert_eq!(panel.on_press(x, y), Outcome::Changed);
    assert_eq!(sent(&requests), [Request::MicMute(true)]);
    assert_eq!(panel.dial(Dial::Mic), Some((70, true)));
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-sound-mic-off");
    panel.on_press(x, y);
    assert_eq!(sent(&requests), [Request::MicMute(false)]);

    // Its level is a slider like the others.
    panel.selected = item_at(&panel, Item::Slider(Dial::Mic));
    press(&mut panel, Key::Char('l'));
    press(&mut panel, Key::Char('m'));
    assert_eq!(
        sent(&requests),
        [Request::MicVolume(75), Request::MicMute(true)]
    );

    // A machine with no microphone has no such section.
    let mut deaf = sound();
    deaf.input = None;
    deaf.inputs.clear();
    panel.apply(Update::Sound(Some(deaf)));
    assert!(!panel.items().contains(&Item::Heading(Section::Microphone)));
}

#[test]
fn switches_flip_at_once_and_ask_the_system() {
    let (mut panel, requests) = panel("switches");
    render(&mut panel);

    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Switch(Page::Wifi));
    assert_eq!(panel.on_press(x, y), Outcome::Changed);
    assert_eq!(sent(&requests), [Request::WifiPower(false)]);
    assert!(!panel.wifi.as_ref().unwrap().powered);
    assert_eq!(wifi_summary(panel.wifi.as_ref()), "Off");
    // The click was on the switch, not the row behind it.
    assert_eq!(panel.page, Page::Main);

    // Space does the same from the keyboard.
    panel.selected = panel
        .items()
        .iter()
        .position(|item| *item == Item::Open(Page::Bluetooth))
        .unwrap();
    assert_eq!(press(&mut panel, Key::Char(' ')), Outcome::Changed);
    assert_eq!(sent(&requests), [Request::BluetoothPower(false)]);
    assert_eq!(bluetooth_summary(panel.bluetooth.as_ref()), "Off");

    // The system has the last word.
    panel.apply(Update::Bluetooth(Some(bluetooth())));
    assert_eq!(bluetooth_summary(panel.bluetooth.as_ref()), "Soft Buds");
}

#[test]
fn the_profile_tile_cycles_only_where_allowed() {
    let (mut panel, requests) = panel("profile");
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Profile);
    assert_eq!(panel.on_press(x, y), Outcome::Changed);
    assert_eq!(sent(&requests), [Request::Profile("performance".into())]);
    assert_eq!(
        panel.hardware.profile.as_ref().unwrap().current,
        "performance"
    );

    // `p` does it from the keyboard, wrapping around.
    press(&mut panel, Key::Char('p'));
    assert_eq!(sent(&requests), [Request::Profile("quiet".into())]);

    panel.hardware.profile.as_mut().unwrap().writable = false;
    press(&mut panel, Key::Char('p'));
    assert!(sent(&requests).is_empty());
    assert!(panel.notice.as_deref().unwrap().contains("root"));
    // The notice gets a line of its own.
    let frame = render(&mut panel);
    assert_eq!(panel.card.h, panel.height_target());
    snapshot(&frame, "ctl-panel-notice");
}

#[test]
fn a_missing_service_dims_its_row_and_cannot_be_opened() {
    let (mut panel, requests) = panel("unavailable");
    panel.apply(Update::Wifi(None));
    panel.apply(Update::Hardware(Hardware::default()));
    panel.apply(Update::Sound(None));
    let frame = render(&mut panel);

    // No backlight, no sound: the sliders are gone, and so are the tiles.
    assert_eq!(panel.items().len(), 3);
    assert_eq!(wifi_summary(panel.wifi.as_ref()), "Unavailable");
    panel.selected = 0;
    assert_eq!(press(&mut panel, Key::Enter), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);
    press(&mut panel, Key::Char(' '));
    assert!(sent(&requests).is_empty());
    assert!(
        !panel
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Switch(Page::Wifi))
    );
    snapshot(&frame, "ctl-panel-bare");
}

// ---- Wi-Fi -----------------------------------------------------------------------

fn open(panel: &mut Panel, page: Page) {
    panel.open(page);
    assert_eq!(panel.page, page);
}

fn select_network(panel: &mut Panel, name: &str) {
    panel.selected = panel
        .networks()
        .iter()
        .position(|network| network.name == name)
        .unwrap();
    panel.settle();
}

#[test]
fn the_wifi_page_lists_networks_and_scrolls() {
    let (mut panel, requests) = panel("wifi");
    open(&mut panel, Page::Wifi);
    assert_eq!(sent(&requests), [Request::WifiScan]);
    let frame = render(&mut panel);

    // Seven of the nine fit; the header leads back.
    let rows = panel
        .hits
        .iter()
        .filter(|(_, hit)| matches!(hit, Hit::Item(_)))
        .count();
    assert_eq!(rows, MAX_ROWS);
    assert!(panel.hits.iter().any(|(_, hit)| *hit == Hit::Back));
    snapshot(&frame, "ctl-wifi");

    // Moving past the last visible row scrolls; the list wraps at its ends.
    for _ in 0..7 {
        press(&mut panel, Key::Char('j'));
    }
    assert_eq!((panel.selected, panel.offset), (7, 1));
    press(&mut panel, Key::Char('j'));
    press(&mut panel, Key::Char('j'));
    assert_eq!((panel.selected, panel.offset), (0, 0));

    // The wheel moves the list a row at a time and keeps the selection in view.
    assert_eq!(panel.on_scroll(10.0), Outcome::Ignored);
    assert_eq!(panel.on_scroll(25.0), Outcome::Changed);
    assert_eq!((panel.selected, panel.offset), (1, 1));
    assert_eq!(panel.on_scroll(200.0), Outcome::Changed);
    assert_eq!((panel.selected, panel.offset), (2, 2));
    panel.on_scroll(-500.0);
    assert_eq!(panel.offset, 0);

    // h, Backspace, or a click on the header go back.
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Back);
    assert_eq!(panel.on_press(x - 60.0, y), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);
}

#[test]
fn connecting_asks_for_a_passphrase_only_when_one_is_needed() {
    let (mut panel, requests) = panel("wifi-connect");
    open(&mut panel, Page::Wifi);
    sent(&requests);

    // The network in use: Enter disconnects.
    select_network(&mut panel, "Tea House");
    press(&mut panel, Key::Enter);
    assert_eq!(sent(&requests), [Request::WifiDisconnect]);

    // A saved one, and an open one, connect straight away.
    for name in ["Studio Mira", "Bakery Guest"] {
        select_network(&mut panel, name);
        press(&mut panel, Key::Enter);
        let id = panel.networks()[panel.selected].id.clone();
        assert_eq!(
            sent(&requests),
            [Request::WifiConnect {
                network: id.clone(),
                passphrase: None
            }]
        );
        // One connection at a time.
        press(&mut panel, Key::Enter);
        assert!(sent(&requests).is_empty());
        panel.apply(Update::WifiConnect {
            network: id,
            result: Ok(()),
        });
        assert_eq!(panel.busy, None);
    }

    // Ones this cannot join say so instead.
    select_network(&mut panel, "Old Router");
    press(&mut panel, Key::Enter);
    assert!(panel.notice.as_deref().unwrap().contains("WEP"));
    select_network(&mut panel, "Corner Office");
    press(&mut panel, Key::Enter);
    assert!(panel.notice.as_deref().unwrap().contains("Enterprise"));
    assert!(sent(&requests).is_empty());
}

#[test]
fn a_passphrase_is_typed_unseen_and_asked_again_if_refused() {
    let (mut panel, requests) = panel("wifi-prompt");
    open(&mut panel, Page::Wifi);
    sent(&requests);
    select_network(&mut panel, "Loft 5G");
    let loft = panel.networks()[panel.selected].id.clone();
    let height = panel.height_target();

    press(&mut panel, Key::Enter);
    assert!(panel.prompt.is_some());
    assert!(sent(&requests).is_empty());
    // The field takes a row of its own, pushing the rest down.
    assert_eq!(panel.height_target(), height + ROW_HEIGHT);
    assert_eq!((panel.slot(1), panel.slot(2)), (1, 3));

    // Letters are text now — j, k, q, and space included — and nothing moves.
    type_text(&mut panel, "jk q matcha");
    assert_eq!(panel.prompt.as_ref().unwrap().field.text(), "jk q matcha");
    assert_eq!(panel.networks()[panel.selected].name, "Loft 5G");
    press(&mut panel, Key::Backspace);
    let clear = KeyEvent {
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
        ..key(Key::Char('u'))
    };
    assert_eq!(panel.on_key(clear), Outcome::Changed);
    assert_eq!(panel.prompt.as_ref().unwrap().field.text(), "");
    // Nothing typed, nothing sent.
    press(&mut panel, Key::Enter);
    assert!(sent(&requests).is_empty());

    type_text(&mut panel, "hunter2");
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-wifi-passphrase");
    // The pointer does not take the selection away mid-typing.
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Item(4));
    assert_eq!(panel.on_move(x, y), Outcome::Ignored);

    press(&mut panel, Key::Enter);
    assert_eq!(
        sent(&requests),
        [Request::WifiConnect {
            network: loft.clone(),
            passphrase: Some(Zeroizing::new("hunter2".into()))
        }]
    );
    assert!(panel.prompt.is_none());
    assert_eq!(panel.busy.as_deref(), Some(loft.as_str()));

    // Refused: the field comes back, empty, saying so.
    panel.apply(Update::WifiConnect {
        network: loft.clone(),
        result: Err(ConnectError::Rejected),
    });
    let prompt = panel.prompt.as_ref().unwrap();
    assert_eq!(prompt.network, loft);
    assert!(prompt.field.is_empty() && prompt.rejected);
    assert_eq!(panel.busy, None);

    // Esc gives up on the passphrase, not on the page.
    assert_eq!(press(&mut panel, Key::Escape), Outcome::Changed);
    assert!(panel.prompt.is_none() && panel.page == Page::Wifi);

    // A saved network whose passphrase no longer works asks too.
    panel.apply(Update::WifiConnect {
        network: loft.clone(),
        result: Err(ConnectError::NeedsPassphrase),
    });
    assert!(panel.prompt.as_ref().is_some_and(|prompt| !prompt.rejected));
    // If the network drops out of range, so does the question.
    let mut fewer = wifi();
    fewer.networks.retain(|network| network.id != loft);
    panel.apply(Update::Wifi(Some(fewer)));
    assert!(panel.prompt.is_none());
}

#[test]
fn the_selection_stays_on_its_network_as_the_list_changes() {
    let (mut panel, _requests) = panel("wifi-reorder");
    open(&mut panel, Page::Wifi);
    select_network(&mut panel, "Bakery Guest");

    let mut changed = wifi();
    changed.networks.swap(2, 5);
    changed
        .networks
        .insert(0, network("Brand New", Security::Open, -40));
    panel.apply(Update::Wifi(Some(changed)));
    assert_eq!(panel.networks()[panel.selected].name, "Bakery Guest");

    // Other failures are reported in a line under the list.
    panel.apply(Update::Failed("operation failed".into()));
    assert_eq!(panel.notice.as_deref(), Some("Operation failed."));

    // Turning the radio off empties the list and says why.
    press(&mut panel, Key::Char(' '));
    assert!(panel.items().is_empty());
    let frame = render(&mut panel);
    assert_eq!(panel.card.h, panel.height_target());
    snapshot(&frame, "ctl-wifi-off");
}

// ---- Bluetooth -------------------------------------------------------------------

#[test]
fn the_bluetooth_page_searches_only_while_open() {
    let (mut panel, requests) = panel("bluetooth");
    open(&mut panel, Page::Bluetooth);
    assert_eq!(sent(&requests), [Request::BluetoothDiscover(true)]);
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-bluetooth");

    // Connected: Enter disconnects.
    press(&mut panel, Key::Enter);
    let buds = panel.devices()[0].id.clone();
    assert_eq!(sent(&requests), [Request::BluetoothDisconnect(buds)]);

    // Paired or new: Enter connects (pairing is the backend's business).
    press(&mut panel, Key::Char('j'));
    press(&mut panel, Key::Char('j'));
    press(&mut panel, Key::Enter);
    let speaker = panel.devices()[2].id.clone();
    assert_eq!(
        sent(&requests),
        [Request::BluetoothConnect(speaker.clone())]
    );
    assert_eq!(panel.busy.as_deref(), Some(speaker.as_str()));

    panel.apply(Update::BluetoothConnect {
        device: speaker,
        result: Err("page timeout".into()),
    });
    assert_eq!(panel.busy, None);
    assert_eq!(panel.notice.as_deref(), Some("Page timeout."));

    // Off and on again from the page resumes the search.
    press(&mut panel, Key::Char(' '));
    press(&mut panel, Key::Char(' '));
    assert_eq!(
        sent(&requests),
        [
            Request::BluetoothPower(false),
            Request::BluetoothPower(true),
            Request::BluetoothDiscover(true),
        ]
    );

    press(&mut panel, Key::Char('h'));
    assert_eq!(panel.page, Page::Main);
    assert_eq!(sent(&requests), [Request::BluetoothDiscover(false)]);
    // Leaving also clears what went wrong there.
    assert_eq!(panel.notice, None);
}

// ---- Theme -----------------------------------------------------------------------

#[test]
fn the_theme_page_applies_themes() {
    let (mut panel, _requests) = panel("theme");
    open(&mut panel, Page::Theme);
    // Built-ins sort as dawn, main, moon; moon is active, so selected.
    assert_eq!(panel.selected, 2);
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-theme");

    press(&mut panel, Key::Char('j'));
    assert_eq!(panel.selected, 0);
    press(&mut panel, Key::Enter);
    assert_eq!(panel.ctx.theme.id, "rose-pine-dawn");
    assert_eq!(
        enso_core::theme::active(&panel.ctx.paths).as_deref(),
        Some("rose-pine-dawn")
    );

    // A click applies too.
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Item(1));
    panel.on_press(x, y);
    assert_eq!(panel.ctx.theme.id, "rose-pine-main");
}

// ---- Presentation ------------------------------------------------------------------

#[test]
fn a_click_outside_closes_a_modal_panel() {
    let (mut panel, _requests) = panel("outside");
    render(&mut panel);
    // Empty space on the card is just empty space.
    assert_eq!(
        panel.on_press(panel.card.x + 2.0, panel.card.y + 2.0),
        Outcome::Ignored
    );
    assert_eq!(panel.on_press(10.0, 10.0), Outcome::Close);
}

#[test]
fn a_dropdown_hangs_from_the_bar_and_follows_the_pointer() {
    for edge in [Edge::Top, Edge::Bottom] {
        let (mut panel, _requests) =
            panel_in(&format!("dropdown-{edge:?}"), PanelMode::Dropdown(edge));
        let spec = panel.surface();
        // Hovering the bar must not take the keyboard from a window.
        assert_eq!(spec.keyboard, Keyboard::None);
        assert_eq!(spec.exclusive_zone, 0);

        // The surface has room for the tallest page at this panel's size.
        let bounds = Rect::new(0.0, 0.0, spec.width as f32, spec.height as f32);
        for page in [Page::Main, Page::Wifi, Page::Bluetooth, Page::Theme] {
            panel.open(page);
            if page == Page::Wifi {
                select_network(&mut panel, "Loft 5G");
                press(&mut panel, Key::Enter);
                panel.notice = Some("Something.".into());
                panel.settle();
            }
            let card = panel.card_rect(bounds, panel.height_target());
            assert!(card.x >= SHADOW_ROOM - 1.0 && card.right() == bounds.right() - MARGIN);
            assert!(
                card.y >= 0.0 && card.bottom() <= bounds.bottom(),
                "{page:?}"
            );
            // It grows away from the bar.
            match edge {
                Edge::Top => assert_eq!(card.y, DROP_GAP),
                Edge::Bottom => assert_eq!(card.bottom(), bounds.bottom() - DROP_GAP),
            }
        }

        // Wanted while the bar says the pointer is on its island…
        assert!(panel.wanted());
        panel.owner_hovering = false;
        assert!(!panel.wanted());
        // …or the pointer is on the card, or a slider is held, or a
        // passphrase is being typed.
        panel.pointer_inside = true;
        assert!(panel.wanted());
        panel.pointer_inside = false;
        panel.dragging = Some((Dial::Brightness, Rect::default()));
        assert!(panel.wanted());
        panel.dragging = None;
        assert!(!panel.wanted());
        panel.open(Page::Wifi);
        select_network(&mut panel, "Loft 5G");
        press(&mut panel, Key::Enter);
        assert!(panel.prompt.is_some() && panel.wanted());
        press(&mut panel, Key::Escape);
        assert!(!panel.wanted());
    }
}

#[test]
fn error_messages_become_sentences() {
    assert_eq!(sentence("operation failed"), "Operation failed.");
    assert_eq!(sentence(" Already done. "), "Already done.");
    assert_eq!(sentence(""), "");
}

// ---- Screen & sleep ------------------------------------------------------------

/// What enso-lock will read for `[lock]`.
fn lock_as_read(panel: &Panel) -> LockTimes {
    Context::load_from(panel.ctx.paths.clone()).section("lock")
}

#[test]
fn screen_and_sleep_sits_beside_the_first_page() {
    let (mut panel, _requests) = panel("screen-turn");
    // Tab turns to it, and goes round back again.
    assert_eq!(press(&mut panel, Key::Tab), Outcome::Changed);
    assert_eq!(panel.page, Page::Screen);
    assert_eq!(press(&mut panel, Key::Tab), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);

    // A swipe turns it; a short one, or a wheel's up and down, doesn't.
    assert_eq!(panel.on_swipe(30.0, false), Outcome::Ignored);
    assert_eq!(panel.page, Page::Main);
    assert_eq!(panel.on_swipe(40.0, false), Outcome::Changed);
    assert_eq!(panel.page, Page::Screen);
    // Further along the same way there's nothing more.
    assert_eq!(panel.on_swipe(80.0, false), Outcome::Ignored);
    assert_eq!(panel.on_swipe(-80.0, false), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);
    // The wheel tilted sideways turns a page a notch.
    assert_eq!(panel.on_swipe(15.0, true), Outcome::Changed);
    assert_eq!(panel.page, Page::Screen);

    let frame = render(&mut panel);
    snapshot(&frame, "ctl-screen");
    assert_eq!(panel.card.h, panel.height_target());
    assert!(panel.hits.iter().all(|(rect, _)| {
        panel.card.contains(rect.x, rect.y)
            && panel.card.contains(rect.right() - 1.0, rect.bottom() - 1.0)
    }));
    // The dot of the first page leads back to it.
    let (x, y) = centre_of(&panel, |hit| *hit == Hit::Dot(Page::Main));
    assert_eq!(panel.on_press(x, y), Outcome::Changed);
    assert_eq!(panel.page, Page::Main);
    render(&mut panel);
    assert!(
        panel
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Dot(Page::Screen))
    );
}

#[test]
fn times_step_and_are_kept_for_the_lock_screen() {
    let (mut panel, _requests) = panel("screen-times");
    panel.open(Page::Screen);
    assert_eq!(panel.lock, LockTimes::default());
    assert_eq!(panel.items()[panel.selected], Item::Timer(Timer::Lock));

    // Five minutes, one step longer: ten.
    press(&mut panel, Key::Right);
    assert_eq!(panel.lock.idle_after, 600);
    assert_eq!(lock_as_read(&panel).idle_after, 600);
    // Back down two: two minutes.
    press(&mut panel, Key::Left);
    press(&mut panel, Key::Left);
    assert_eq!(lock_as_read(&panel).idle_after, 120);

    // The screen's time, by its arrow.
    render(&mut panel);
    let (x, y) = centre_of(&panel, |hit| {
        matches!(hit, Hit::Step(_, Timer::ScreenOff, 1))
    });
    panel.on_press(x, y);
    assert_eq!(lock_as_read(&panel).screen_off_after, 60);

    // The switch.
    press(&mut panel, Key::Down);
    press(&mut panel, Key::Down);
    assert_eq!(panel.items()[panel.selected], Item::BeforeSleep);
    press(&mut panel, Key::Char(' '));
    assert!(!lock_as_read(&panel).before_sleep);
    assert!(panel.notice.is_none());

    // What was kept is what a new panel shows.
    let again = Panel::new(
        Context::load_from(panel.ctx.paths.clone()),
        CtlConfig::default(),
        PanelMode::Modal,
    );
    assert_eq!(again.lock, panel.lock);
    let frame = render(&mut panel);
    snapshot(&frame, "ctl-screen-changed");
}

#[test]
fn times_step_between_choices_and_stop_at_the_ends() {
    // Never comes after the longest time.
    assert_eq!(Timer::Lock.step(1800, 1, false), 0);
    assert_eq!(Timer::Lock.step(0, 1, false), 0);
    assert_eq!(Timer::Lock.step(0, 1, true), 60);
    assert_eq!(Timer::Lock.step(60, -1, false), 60);
    // A time from the config file that isn't a choice steps to its
    // neighbours.
    assert_eq!(Timer::Lock.step(400, 1, false), 600);
    assert_eq!(Timer::Lock.step(400, -1, false), 300);
    assert_eq!(duration(0), "Never");
    assert_eq!(duration(30), "30 s");
    assert_eq!(duration(300), "5 min");
    assert_eq!(duration(3600), "1 h");
}
