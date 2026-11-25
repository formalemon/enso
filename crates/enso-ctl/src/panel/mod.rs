//! The control center panel: a floating card in the corner of the screen.
//!
//! The first page is a glance and the quick controls — the power profile
//! and temperature, brightness, volume, and a row each for Wi-Fi and
//! Bluetooth. A row opens a page of its own: networks in range, devices —
//! and the volume opens Sound: which output, each application's level, and
//! the microphone.
//!
//! Beside it, a swipe away (or Tab, or the dots at the foot), is Settings:
//! the theme, Do not disturb, and when the screen locks, when the lock
//! screen turns the screens off and puts the machine to sleep, and whether
//! it locks before sleeping. What's chosen there is kept with enso's state,
//! over `config.toml` ([`enso_core::chosen`]), and the notification daemon
//! and the lock watcher follow it.
//!
//! This module is the panel's state and behaviour; `draw` paints it. It never
//! talks to the system itself: it sends [`Request`]s to the workers in
//! [`crate::system`] and redraws from the [`Update`]s that come back, so
//! nothing here can block the interface.
//!
//! The panel has two presentations. Opened on purpose (`enso-ctl`), it takes
//! the keyboard and stays until dismissed. Opened by `enso-bar` under the
//! pointer (`--dropdown`), it hangs below the bar and goes away when the
//! pointer does; it leaves the keyboard with whatever had it, borrowing it
//! only for as long as a passphrase is being typed.

mod displays;
mod draw;

use std::{
    io::BufRead,
    thread,
    time::{Duration, Instant},
};

use enso_core::{Context, Transition, Tween, chosen, style::space};
use enso_sys::{
    audio::Sound,
    bluetooth::{Bluetooth, Device},
    wifi::{ConnectError, Link, Network, Security, Wifi},
};
use enso_ui::{
    Anchor, App, Button, Canvas, Cx, Event, Key, KeyEvent, Keyboard, Layer, Nav, Paint,
    PointerEvent, Presence, Proxy, Rect, SurfaceSpec,
    watch::watch_dir,
    widget::{SecretField, secret},
};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::{
    cli::{Edge, PanelMode},
    system::{self, Hardware, Request, Setting, System, Update},
    theme,
};

/// The `[ctl]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CtlConfig {
    /// Panel width in logical pixels.
    pub width: u32,
    /// The programs a theme switch reaches besides enso's own tools.
    pub theme_targets: Vec<theme::Place>,
}

impl Default for CtlConfig {
    fn default() -> Self {
        Self {
            width: 340,
            theme_targets: theme::Place::ALL.to_vec(),
        }
    }
}

/// Gap between the card and the screen edge: the bar's, so they line up.
const MARGIN: f32 = enso_core::style::edge::SCREEN;
/// Gap between the bar and a dropdown card.
const DROP_GAP: f32 = enso_core::style::edge::BELOW_BAR;
/// Room a dropdown's surface keeps around the card for its shadow.
const SHADOW_ROOM: f32 = 28.0;
const PAD: f32 = space::LG;
/// Room between the card's edge and what's on it — tiles, and the rows'
/// highlights — and between the tiles and the rows: one line all round.
const EDGE: f32 = space::SM;
const TILE_HEIGHT: f32 = 60.0;
const ROW_HEIGHT: f32 = 44.0;
const NOTICE_HEIGHT: f32 = 28.0;
/// The dots at the foot of the pages a swipe moves between.
const DOTS_HEIGHT: f32 = 18.0;
/// How far fingers travel across a touchpad to turn the page.
const SWIPE: f32 = 60.0;
/// Rows a list shows before it scrolls.
const MAX_ROWS: usize = 7;
/// Rows the Sound page shows before it scrolls: it has three short lists.
const MAX_SOUND_ROWS: usize = 12;
/// Applications the Sound page lists at most.
const MAX_APPS: usize = 6;
/// How far one key press moves a volume, in percent.
const VOLUME_STEP: u8 = 5;
/// Scroll distance that moves a list by one row.
const SCROLL_STEP: f32 = 30.0;
/// The name the control center listens under, and holds its one-copy lock
/// by (see [`enso_core::ipc`]).
pub const NAME: &str = "enso-ctl";

/// How long a dropdown outlives the pointer leaving it, so that crossing the
/// gap between the bar and the card does not close it.
const GRACE: Duration = Duration::from_millis(300);
/// Characters a passphrase may have: more than WPA's 63, for WPA3.
const MAX_PASSPHRASE: usize = 256;

#[derive(Debug)]
pub enum Message {
    System(Box<Update>),
    ThemeChanged,
    /// From the bar: whether the pointer is on the island that opened us.
    Owner(bool),
    /// From the bar, a second `enso-ctl`, or the notification history:
    /// something else is opening in this panel's place, or the key that
    /// opened it was pressed again.
    Dismiss,
    /// From a second `enso-ctl --page`: show this page.
    Page(Page),
}

/// The pages `--page` can open, by name.
pub const PAGE_NAMES: [(&str, Page); 7] = [
    ("main", Page::Main),
    ("settings", Page::Settings),
    ("wifi", Page::Wifi),
    ("bluetooth", Page::Bluetooth),
    ("sound", Page::Sound),
    ("displays", Page::Displays),
    ("theme", Page::Theme),
];

impl Page {
    pub fn named(name: &str) -> Option<Self> {
        PAGE_NAMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, page)| *page)
    }

    pub fn name(self) -> &'static str {
        PAGE_NAMES
            .iter()
            .find(|(_, page)| *page == self)
            .map_or("main", |(name, _)| name)
    }
}

/// What the panel is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    /// The screens: size, refresh, scale, on or off.
    Displays,
    /// Beside the first page: the theme, Do not disturb, and when to lock,
    /// darken, and sleep.
    Settings,
    Sound,
    Wifi,
    Bluetooth,
    Theme,
}

/// Something a slider sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dial {
    Brightness,
    /// The output's volume.
    Volume,
    /// The volume of an application, by index into those playing.
    App(usize),
    /// The microphone's level.
    Mic,
}

/// How many choices the Do not disturb row steps through.
const QUIET_CHOICES: usize = 5;
const HOUR: u64 = 3600;

/// The next 8 in the morning, in seconds since the epoch: today's, if it
/// hasn't come yet.
fn next_morning(now: u64) -> u64 {
    let morning = || -> Option<u64> {
        let zone = jiff::tz::TimeZone::system();
        let at = jiff::Timestamp::from_second(i64::try_from(now).ok()?)
            .ok()?
            .to_zoned(zone);
        let mut morning = at.date().at(8, 0, 0, 0).to_zoned(at.time_zone().clone()).ok()?;
        if morning <= at {
            morning = morning.tomorrow().ok()?;
        }
        u64::try_from(morning.timestamp().as_second()).ok()
    };
    morning().unwrap_or(now + 8 * HOUR)
}

/// The pages side by side, which a swipe moves between.
const SIDE_BY_SIDE: [Page; 2] = [Page::Main, Page::Settings];

/// A time on the Screen & sleep page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timer {
    /// Idle time before the screen locks.
    Lock,
    /// Time on the lock screen before the screens go dark.
    ScreenOff,
    /// Time on the lock screen before the machine sleeps.
    Sleep,
}

impl Timer {
    /// Its key in `[lock]`.
    fn key(self) -> &'static str {
        match self {
            Self::Lock => "idle_after",
            Self::ScreenOff => "screen_off_after",
            Self::Sleep => "suspend_after",
        }
    }

    /// The times to choose between, in seconds, shortest first; `0`, never,
    /// comes last.
    fn choices(self) -> &'static [u64] {
        match self {
            Self::Lock => &[60, 120, 300, 600, 900, 1800, 0],
            Self::ScreenOff => &[10, 30, 60, 120, 300, 0],
            Self::Sleep => &[300, 600, 900, 1800, 3600, 0],
        }
    }

    /// The choice `step` places along from `now`, which may be a time
    /// from the config file that isn't among the choices; `wrap` goes
    /// round from the last to the first.
    fn step(self, now: u64, step: isize, wrap: bool) -> u64 {
        let choices = self.choices();
        // Never sorts after every time.
        let rank = |seconds: u64| if seconds == 0 { u64::MAX } else { seconds };
        let at = choices
            .iter()
            .position(|&choice| rank(choice) >= rank(now))
            .unwrap_or(choices.len() - 1);
        let exact = choices[at] == now;
        let last = choices.len() as isize - 1;
        // Between two choices, `at` is the next one up: a step up lands
        // there, a step down on the one before it.
        let to = at as isize + if step > 0 && !exact { step - 1 } else { step };
        let to = if wrap {
            to.rem_euclid(last + 1)
        } else {
            to.clamp(0, last)
        };
        choices[to as usize]
    }
}

/// `[lock]`'s times, as the Screen & sleep page shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct LockTimes {
    idle_after: u64,
    screen_off_after: u64,
    suspend_after: u64,
    before_sleep: bool,
}

impl Default for LockTimes {
    fn default() -> Self {
        Self {
            idle_after: chosen::lock::IDLE_AFTER,
            screen_off_after: chosen::lock::SCREEN_OFF_AFTER,
            suspend_after: chosen::lock::SUSPEND_AFTER,
            before_sleep: chosen::lock::BEFORE_SLEEP,
        }
    }
}

impl LockTimes {
    fn get(&self, timer: Timer) -> u64 {
        match timer {
            Timer::Lock => self.idle_after,
            Timer::ScreenOff => self.screen_off_after,
            Timer::Sleep => self.suspend_after,
        }
    }

    fn set(&mut self, timer: Timer, seconds: u64) {
        match timer {
            Timer::Lock => self.idle_after = seconds,
            Timer::ScreenOff => self.screen_off_after = seconds,
            Timer::Sleep => self.suspend_after = seconds,
        }
    }
}

/// A time as the page says it: `30 s`, `5 min`, `1 h`, `Never`.
fn duration(seconds: u64) -> String {
    match seconds {
        0 => "Never".to_owned(),
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => format!("{} h", s / 3600),
    }
}

/// A part of the Sound page, or of Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Output,
    Apps,
    Microphone,
    /// Settings' times and the lock.
    Screen,
}

/// Something on the current page that can be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Slider(Dial),
    /// The heading of a part of the Sound page. Not selectable.
    Heading(Section),
    /// The line that says no application is playing. Not selectable.
    Silence,
    /// An output device, by index: choose it to play through it.
    Output(usize),
    /// A microphone, by index: choose it to record from it.
    Input(usize),
    Open(Page),
    /// Index into the networks in range.
    Network(usize),
    /// Index into the Bluetooth devices.
    Device(usize),
    /// Index into the themes.
    Theme(usize),
    /// A time on the Screen & sleep page.
    Timer(Timer),
    /// Whether to lock before sleeping.
    BeforeSleep,
    /// Notifications wait quietly in the history.
    DoNotDisturb,
    /// One row about a screen: index into the displays, and which.
    Display(usize, Screen),
}

/// A row on the Displays page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    /// Its name, heading its rows. Not selectable.
    Name,
    Power,
    Mode,
    Scale,
    Vrr,
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    /// An item, by index on the current page.
    Item(usize),
    /// A slider's item index, what it sets, and the track to measure
    /// against.
    Slider(usize, Dial, Rect),
    /// The speaker at the start of a volume slider: mute.
    Mute(Dial),
    /// The volume row away from its track and its speaker: the Sound page.
    /// The row's item index.
    OpenSound(usize),
    /// The switch on the Sound page's microphone heading.
    MicSwitch,
    /// The on/off switch of Wi-Fi or Bluetooth.
    Switch(Page),
    /// The header of a page: back to the first one.
    Back,
    /// The power profile tile.
    Profile,
    /// The Do not disturb tile.
    Quiet,
    /// The Keep awake tile.
    Awake,
    /// One of a display row's arrows: its item index, and the step.
    DisplayStep(usize, isize),
    /// The moon at the end of the screen's slider: night light.
    NightLight,
    /// One of a time's arrows: a step shorter (`-1`) or longer (`1`).
    Step(usize, Timer, isize),
    /// One of the arrows of how long Do not disturb lasts.
    QuietStep(usize, isize),
    /// A dot at the foot: go to that page.
    Dot(Page),
}

/// A passphrase being typed for a network.
#[derive(Debug)]
struct Prompt {
    /// The network's id.
    network: String,
    field: SecretField,
    /// The last passphrase tried was not accepted.
    rejected: bool,
}

/// What handling an input came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Ignored,
    /// Something changed: redraw.
    Changed,
    /// The panel should close.
    Close,
}

#[derive(Debug)]
pub struct Panel {
    ctx: Context,
    config: CtlConfig,
    mode: PanelMode,
    presence: Presence,
    system: System,

    hardware: Hardware,
    wifi: Option<Wifi>,
    bluetooth: Option<Bluetooth>,
    sound: Option<Sound>,

    page: Page,
    /// Selected item on the current page.
    selected: usize,
    /// Where the selection was on each page side by side, to return to.
    home_selected: [usize; SIDE_BY_SIDE.len()],
    /// The page side by side that the page showing was opened from, and
    /// goes back to.
    home: Page,
    /// First visible item of a scrolling list.
    offset: usize,
    /// Scroll distance not yet turned into rows.
    scroll: f32,
    prompt: Option<Prompt>,
    /// Id of the network or device a connection is being made to.
    busy: Option<String>,
    /// The last thing that went wrong, shown quietly at the bottom.
    notice: Option<String>,
    /// The slider being dragged, and its track.
    dragging: Option<(Dial, Rect)>,

    /// Position of the selection highlight, in rows.
    highlight: Tween<f32>,
    /// Height of the card.
    height: Tween<f32>,
    /// `1` as a page is opened, easing to `0` as it settles into place.
    slide: Tween<f32>,
    /// Which way the page slid in: `1` from the right, `-1` from the left.
    slide_from: f32,
    wifi_switch: Tween<f32>,
    bluetooth_switch: Tween<f32>,
    /// On while the microphone is live.
    mic_switch: Tween<f32>,
    before_sleep_switch: Tween<f32>,
    quiet_switch: Tween<f32>,
    /// Keeping awake is on and its tile shows: the steam over its cup
    /// drifts, a frame at a time.
    steaming: bool,
    /// Do not disturb, as it stands now.
    quiet: bool,
    /// The screens, as niri last said, while the Displays page is open.
    displays: Vec<displays::Display>,
    /// Night light: whether the screens are warm, as enso-dusk last said;
    /// `None` without enso-dusk.
    night: Option<bool>,
    /// When Do not disturb ends by itself (0: when it's turned off).
    quiet_until: u64,
    /// Keeping awake, as chosen.
    awake: enso_core::Awake,

    /// When to lock, darken, and sleep.
    lock: LockTimes,
    /// Sideways travel on the touchpad not yet turned into a page.
    swipe: f32,

    /// Dropdown only: the pointer is over the card.
    pointer_inside: bool,
    /// Dropdown only: the pointer is over the bar island that opened us.
    owner_hovering: bool,
    /// Dropdown only: when to close unless the pointer comes back.
    leave_at: Option<Instant>,

    /// A page asked for (`--page`) and not shown yet: Wi-Fi, Bluetooth,
    /// and Sound wait until their service has answered.
    asked: Option<Page>,

    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    hits: Vec<(Rect, Hit)>,
}

impl Panel {
    pub fn new(ctx: Context, config: CtlConfig, mode: PanelMode) -> Self {
        let mut panel = Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            config,
            mode,
            // The workers start with the interface, in `start`; until then
            // there is nobody to ask and nothing to show.
            system: System::detached().0,
            hardware: Hardware::default(),
            wifi: None,
            bluetooth: None,
            sound: None,
            page: Page::Main,
            selected: 0,
            home_selected: [0; SIDE_BY_SIDE.len()],
            home: Page::Main,
            offset: 0,
            scroll: 0.0,
            prompt: None,
            busy: None,
            notice: None,
            dragging: None,
            highlight: Tween::new(0.0),
            height: Tween::new(0.0),
            slide: Tween::new(0.0),
            slide_from: 1.0,
            mic_switch: Tween::new(0.0),
            before_sleep_switch: Tween::new(0.0),
            quiet_switch: Tween::new(0.0),
            steaming: false,
            quiet: false,
            displays: Vec::new(),
            night: None,
            quiet_until: 0,
            awake: enso_core::Awake::Off,
            lock: LockTimes::default(),
            swipe: 0.0,
            wifi_switch: Tween::new(0.0),
            bluetooth_switch: Tween::new(0.0),
            pointer_inside: false,
            // Whoever opened a dropdown is hovering until they say otherwise.
            owner_hovering: matches!(mode, PanelMode::Dropdown(_)),
            leave_at: None,
            asked: None,
            card: Rect::default(),
            hits: Vec::new(),
        };
        panel.read_settings();
        panel
            .before_sleep_switch
            .jump(panel.before_sleep_switch.target());
        panel.quiet_switch.jump(panel.quiet_switch.target());
        // The card opens at its size instead of growing into it.
        panel.height.jump(panel.height_target());
        panel
    }

    // ---- What is on the page ------------------------------------------------

    /// Networks in range; none while the radio is off.
    fn networks(&self) -> &[Network] {
        match &self.wifi {
            Some(wifi) if wifi.powered => &wifi.networks,
            _ => &[],
        }
    }

    /// Bluetooth devices; none while the radio is off.
    fn devices(&self) -> &[Device] {
        match &self.bluetooth {
            Some(bluetooth) if bluetooth.powered => &bluetooth.devices,
            _ => &[],
        }
    }

    /// Applications playing sound, as many as the panel shows.
    fn apps(&self) -> &[enso_sys::audio::App] {
        let apps = self.sound.as_ref().map_or(&[][..], |sound| &sound.apps);
        &apps[..apps.len().min(MAX_APPS)]
    }

    /// Where a slider stands: percent, and whether it is muted. Brightness
    /// is never muted.
    fn dial(&self, dial: Dial) -> Option<(u8, bool)> {
        match dial {
            Dial::Brightness => Some((self.hardware.screen.as_ref()?.percent(), false)),
            Dial::Volume => {
                let output = self.sound.as_ref()?.output.as_ref()?;
                Some((output.volume, output.muted))
            }
            Dial::App(index) => {
                let app = self.apps().get(index)?;
                Some((app.volume, app.muted))
            }
            Dial::Mic => {
                let input = self.sound.as_ref()?.input.as_ref()?;
                Some((input.volume, input.muted))
            }
        }
    }

    /// Output devices, or microphones.
    fn sound_devices(&self, inputs: bool) -> &[enso_sys::audio::Device] {
        match &self.sound {
            Some(sound) if inputs => &sound.inputs,
            Some(sound) => &sound.outputs,
            None => &[],
        }
    }

    /// Whether the selection can rest on an item.
    fn selectable(item: Item) -> bool {
        !matches!(
            item,
            Item::Heading(_) | Item::Silence | Item::Display(_, Screen::Name)
        )
    }

    fn items(&self) -> Vec<Item> {
        match self.page {
            Page::Main => {
                let mut items = Vec::new();
                if self.hardware.screen.is_some() {
                    items.push(Item::Slider(Dial::Brightness));
                }
                if self.sound.as_ref().is_some_and(|s| s.output.is_some()) {
                    items.push(Item::Slider(Dial::Volume));
                }
                items.extend([Page::Wifi, Page::Bluetooth].map(Item::Open));
                items
            }
            Page::Sound => {
                // Where sound goes, first: its level, then the devices to
                // choose between.
                let mut items = vec![Item::Heading(Section::Output)];
                if self.dial(Dial::Volume).is_some() {
                    items.push(Item::Slider(Dial::Volume));
                }
                items.extend((0..self.sound_devices(false).len()).map(Item::Output));

                items.push(Item::Heading(Section::Apps));
                match self.apps().len() {
                    0 => items.push(Item::Silence),
                    count => items.extend((0..count).map(Dial::App).map(Item::Slider)),
                }

                if !self.sound_devices(true).is_empty() {
                    items.push(Item::Heading(Section::Microphone));
                    if self.dial(Dial::Mic).is_some() {
                        items.push(Item::Slider(Dial::Mic));
                    }
                    items.extend((0..self.sound_devices(true).len()).map(Item::Input));
                }
                items
            }
            Page::Wifi => (0..self.networks().len()).map(Item::Network).collect(),
            Page::Bluetooth => (0..self.devices().len()).map(Item::Device).collect(),
            Page::Theme => (0..self.ctx.themes.len()).map(Item::Theme).collect(),
            Page::Displays => {
                let many = self.displays.len() > 1;
                let mut items = Vec::new();
                for (index, display) in self.displays.iter().enumerate() {
                    items.push(Item::Display(index, Screen::Name));
                    // The only screen can't be turned off from here.
                    if many {
                        items.push(Item::Display(index, Screen::Power));
                    }
                    if display.on {
                        items.push(Item::Display(index, Screen::Mode));
                        items.push(Item::Display(index, Screen::Scale));
                        if display.vrr_supported {
                            items.push(Item::Display(index, Screen::Vrr));
                        }
                    }
                }
                items
            }
            Page::Settings => vec![
                Item::Open(Page::Theme),
                Item::Open(Page::Displays),
                Item::DoNotDisturb,
                Item::Heading(Section::Screen),
                Item::Timer(Timer::Lock),
                Item::Timer(Timer::ScreenOff),
                Item::Timer(Timer::Sleep),
                Item::BeforeSleep,
            ],
        }
    }

    /// Whether the page is one of those side by side, with dots.
    fn side_by_side(&self) -> bool {
        SIDE_BY_SIDE.contains(&self.page)
    }

    /// The id of the selected network or device, to find it again after the
    /// list changes under the selection.
    fn selected_id(&self) -> Option<String> {
        match self.items().get(self.selected)? {
            Item::Network(index) => Some(self.networks().get(*index)?.id.clone()),
            Item::Device(index) => Some(self.devices().get(*index)?.id.clone()),
            _ => None,
        }
    }

    /// Index of the network a passphrase is being typed for, if it is listed.
    fn prompt_index(&self) -> Option<usize> {
        let prompt = self.prompt.as_ref()?;
        self.networks()
            .iter()
            .position(|network| network.id == prompt.network)
    }

    /// Rows of the list that fit at once.
    fn visible_rows(&self) -> usize {
        match self.page {
            Page::Main | Page::Settings => self.items().len(),
            Page::Sound => self.items().len().min(MAX_SOUND_ROWS),
            _ => self.items().len().min(MAX_ROWS),
        }
    }

    /// Which row an item is drawn in, counting from the top of the list. The
    /// passphrase field takes the row under its network.
    fn slot(&self, index: usize) -> usize {
        let pushed = self.prompt_index().is_some_and(|prompt| prompt < index);
        index.saturating_sub(self.offset) + usize::from(pushed)
    }

    /// Height the card should have for what it shows.
    fn height_target(&self) -> f32 {
        let notice = if self.notice.is_some() {
            NOTICE_HEIGHT
        } else {
            0.0
        };
        let rows = |count: usize| count as f32 * ROW_HEIGHT;
        match self.page {
            Page::Main => {
                let tile_rows = draw::tiles(&self.hardware, self.switches()).len();
                let tiles = tile_rows as f32 * (TILE_HEIGHT + EDGE);
                EDGE + tiles + rows(self.visible_rows()) + notice + DOTS_HEIGHT + space::SM
            }
            Page::Settings => EDGE + rows(self.visible_rows()) + notice + DOTS_HEIGHT + space::SM,
            _ => {
                // An empty list still says why it is empty, on one row.
                let list = self.visible_rows().max(1) + usize::from(self.prompt_index().is_some());
                space::SM + ROW_HEIGHT + space::XS + rows(list) + notice + space::SM
            }
        }
    }

    /// The tallest the card can get, which sizes a dropdown's surface.
    fn max_height() -> f32 {
        let longest = MAX_SOUND_ROWS.max(MAX_ROWS + 1);
        let list = space::SM + ROW_HEIGHT + space::XS + longest as f32 * ROW_HEIGHT;
        // Tiles, two sliders, three rows.
        let main = EDGE + TILE_HEIGHT + EDGE + 5.0 * ROW_HEIGHT + DOTS_HEIGHT;
        list.max(main) + NOTICE_HEIGHT + space::SM
    }

    /// Where the card rests in a surface of `bounds`, at a given height.
    fn card_rect(&self, bounds: Rect, height: f32) -> Rect {
        let width = (self.config.width as f32).min(bounds.w - 2.0 * MARGIN);
        let x = bounds.right() - width - MARGIN;
        let y = match self.mode {
            PanelMode::Modal => MARGIN,
            PanelMode::Dropdown(Edge::Top) => DROP_GAP,
            PanelMode::Dropdown(Edge::Bottom) => bounds.bottom() - DROP_GAP - height,
        };
        Rect::new(x, y, width, height)
    }

    /// Bring everything derived from the state back in line with it. Called
    /// after every change.
    fn settle(&mut self) {
        let now = Instant::now();
        let items = self.items();
        let count = items.len();
        self.selected = self.selected.min(count.saturating_sub(1));
        // The selection never rests on a heading: it takes the next thing
        // that can be chosen, or failing that the one before.
        if items
            .get(self.selected)
            .is_some_and(|item| !Self::selectable(*item))
        {
            let after = (self.selected..count).find(|i| Self::selectable(items[*i]));
            let before = (0..self.selected)
                .rev()
                .find(|i| Self::selectable(items[*i]));
            if let Some(index) = after.or(before) {
                self.selected = index;
            }
        }

        // Keep the selection on screen.
        let rows = self.visible_rows().max(1);
        self.offset = self.offset.min(count.saturating_sub(rows));
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        // A heading just above the top row comes into view with it.
        if self.offset > 0
            && self.offset == self.selected
            && rows > 1
            && !Self::selectable(items[self.offset - 1])
        {
            self.offset -= 1;
        }

        let motion = self.ctx.motion;
        self.highlight.set(
            self.slot(self.selected) as f32,
            motion.apply(Transition::SELECT),
            now,
        );
        self.height
            .set(self.height_target(), motion.apply(Transition::REFLOW), now);
        let toggle = motion.apply(Transition::TOGGLE);
        let on = |powered: Option<bool>| if powered == Some(true) { 1.0 } else { 0.0 };
        self.wifi_switch
            .set(on(self.wifi.as_ref().map(|w| w.powered)), toggle, now);
        self.bluetooth_switch
            .set(on(self.bluetooth.as_ref().map(|b| b.powered)), toggle, now);
        let live = self.dial(Dial::Mic).map(|(_, muted)| !muted);
        self.mic_switch.set(on(live), toggle, now);
        self.before_sleep_switch
            .set(on(Some(self.lock.before_sleep)), toggle, now);
        self.quiet_switch.set(on(Some(self.quiet)), toggle, now);
    }

    // ---- News from the system ------------------------------------------------

    /// Take in news from the workers.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Hardware(mut hardware) => {
                // A slider being dragged is ahead of what the system reports.
                if let Some((Dial::Brightness, _)) = self.dragging
                    && let (Some(new), Some(old)) = (&mut hardware.screen, &self.hardware.screen)
                {
                    new.brightness = old.brightness;
                }
                self.hardware = hardware;
            }
            Update::Sound(mut sound) => {
                match (self.dragging, &mut sound, &self.sound) {
                    (Some((Dial::Volume, _)), Some(new), Some(old)) => {
                        if let (Some(new), Some(old)) = (&mut new.output, &old.output) {
                            new.volume = old.volume;
                        }
                    }
                    (Some((Dial::Mic, _)), Some(new), Some(old)) => {
                        if let (Some(new), Some(old)) = (&mut new.input, &old.input) {
                            new.volume = old.volume;
                        }
                    }
                    (Some((Dial::App(index), _)), Some(new), Some(old)) => {
                        match (new.apps.get_mut(index), old.apps.get(index)) {
                            (Some(new), Some(old)) if new.name == old.name => {
                                new.volume = old.volume;
                            }
                            // The list moved under the pointer: let go.
                            _ => self.dragging = None,
                        }
                    }
                    _ => {}
                }
                let first = self.sound.is_none();
                // Devices come and go: keep the selection on what it was on.
                let selected = self.items().get(self.selected).copied();
                self.sound = sound;
                if self.page == Page::Sound {
                    if self.sound.is_none() {
                        self.open(self.home);
                    } else if let Some(index) =
                        selected.and_then(|item| self.items().iter().position(|now| *now == item))
                    {
                        self.selected = index;
                    }
                }
                if first {
                    self.settle();
                    self.mic_switch.jump(self.mic_switch.target());
                }
            }
            Update::Wifi(wifi) => {
                let first = self.wifi.is_none();
                let selected = self.selected_id();
                self.wifi = wifi;
                if self.page == Page::Wifi {
                    self.reselect(selected);
                    if self.prompt.is_some() && self.prompt_index().is_none() {
                        self.prompt = None;
                    }
                    if self.wifi.is_none() {
                        self.open(self.home);
                    }
                }
                if first {
                    self.settle();
                    self.wifi_switch.jump(self.wifi_switch.target());
                }
            }
            Update::Bluetooth(bluetooth) => {
                let first = self.bluetooth.is_none();
                let selected = self.selected_id();
                self.bluetooth = bluetooth;
                if self.page == Page::Bluetooth {
                    self.reselect(selected);
                    if self.bluetooth.is_none() {
                        self.open(self.home);
                    }
                }
                if first {
                    self.settle();
                    self.bluetooth_switch.jump(self.bluetooth_switch.target());
                }
            }
            Update::WifiConnect { network, result } => {
                if self.busy.as_deref() == Some(network.as_str()) {
                    self.busy = None;
                }
                match result {
                    Ok(()) => self.notice = None,
                    // Ask (again) for the passphrase, if the page is still up.
                    Err(ConnectError::NeedsPassphrase | ConnectError::Rejected)
                        if self.page == Page::Wifi =>
                    {
                        let rejected = result == Err(ConnectError::Rejected);
                        self.ask_passphrase(network, rejected);
                    }
                    Err(err) => self.notice = Some(sentence(&err.to_string())),
                }
            }
            Update::BluetoothConnect { device, result } => {
                if self.busy.as_deref() == Some(device.as_str()) {
                    self.busy = None;
                }
                self.notice = result.err().map(|err| sentence(&err));
            }
            Update::Failed(message) => self.notice = Some(sentence(&message)),
        }
        self.settle();
    }

    /// Select the item with this id again, wherever it has moved to.
    fn reselect(&mut self, id: Option<String>) {
        let Some(id) = id else { return };
        let found = match self.page {
            Page::Wifi => self.networks().iter().position(|n| n.id == id),
            Page::Bluetooth => self.devices().iter().position(|d| d.id == id),
            _ => None,
        };
        if let Some(index) = found {
            self.selected = index;
        }
    }

    // ---- Things the user does -----------------------------------------------

    fn request(&self, request: Request) {
        self.system.request(request);
    }

    /// Go to a page.
    /// Open on `page` rather than the first, as soon as it can be shown.
    pub fn asking(mut self, page: Option<Page>) -> Self {
        self.asked = page;
        self
    }

    /// Show the page asked for, if it can be shown now.
    fn go_asked(&mut self, at_once: bool) {
        let Some(page) = self.asked else {
            return;
        };
        self.open(page);
        if self.page == page {
            self.asked = None;
            if at_once {
                // Opening on it: no slide, and the card at its size.
                self.slide.jump(0.0);
                self.height.jump(self.height_target());
            }
        }
    }

    fn open(&mut self, page: Page) {
        let unavailable = match page {
            Page::Sound => self.sound.is_none(),
            Page::Wifi => self.wifi.is_none(),
            Page::Bluetooth => self.bluetooth.is_none(),
            _ => false,
        };
        if page == self.page || unavailable {
            return;
        }
        let side = |page: Page| SIDE_BY_SIDE.iter().position(|side| *side == page);
        let (from, to) = (side(self.page), side(page));
        if let Some(at) = from {
            self.home_selected[at] = self.selected;
            // A page opened from here comes back here.
            if to.is_none() {
                self.home = self.page;
            }
        }
        // Only look for devices while someone is looking at them.
        if self.page == Page::Bluetooth {
            self.request(Request::BluetoothDiscover(false));
        }
        self.page = page;
        self.swipe = 0.0;
        self.prompt = None;
        self.notice = None;
        self.offset = 0;
        self.scroll = 0.0;
        self.selected = match page {
            Page::Main | Page::Settings => to.map_or(0, |at| self.home_selected[at]),
            Page::Theme => self
                .ctx
                .themes
                .iter()
                .position(|theme| theme.id == self.ctx.theme.id)
                .unwrap_or(0),
            Page::Wifi | Page::Bluetooth | Page::Sound => 0,
            Page::Displays => {
                self.read_displays();
                1
            }
        };
        match page {
            Page::Wifi if self.wifi.as_ref().is_some_and(|w| w.powered) => {
                self.request(Request::WifiScan);
            }
            Page::Bluetooth if self.bluetooth.as_ref().is_some_and(|b| b.powered) => {
                self.request(Request::BluetoothDiscover(true));
            }
            _ => {}
        }

        // The new page slides in from the side it lives on — the right for
        // one further along or opened from here, the left going back — and
        // the highlight is simply there, not carried over.
        self.slide_from = match (from, to) {
            (Some(from), Some(to)) if to < from => -1.0,
            (None, Some(_)) => -1.0,
            _ => 1.0,
        };
        self.slide = Tween::new(1.0);
        self.slide.set(
            0.0,
            self.ctx.motion.apply(Transition::REFLOW),
            Instant::now(),
        );
        self.settle();
        self.highlight.jump(self.highlight.target());
    }

    /// Flip the Wi-Fi or Bluetooth radio.
    fn toggle(&mut self, page: Page) {
        match page {
            Page::Wifi => {
                let Some(wifi) = &mut self.wifi else { return };
                // Shown at once; the next snapshot confirms or corrects it.
                wifi.powered = !wifi.powered;
                let on = wifi.powered;
                self.prompt = None;
                self.request(Request::WifiPower(on));
            }
            Page::Bluetooth => {
                let Some(bluetooth) = &mut self.bluetooth else {
                    return;
                };
                bluetooth.powered = !bluetooth.powered;
                let on = bluetooth.powered;
                self.request(Request::BluetoothPower(on));
                if on && self.page == Page::Bluetooth {
                    self.request(Request::BluetoothDiscover(true));
                }
            }
            _ => return,
        }
        self.notice = None;
        self.settle();
    }

    /// Do what the item is for.
    fn activate(&mut self, index: usize) {
        match self.items().get(index).copied() {
            Some(Item::Open(page)) => self.open(page),
            Some(Item::Network(index)) => self.activate_network(index),
            Some(Item::Device(index)) => self.activate_device(index),
            Some(Item::Theme(index)) => self.apply_theme(index),
            Some(Item::Timer(timer)) => self.step_timer(timer, 1, true),
            Some(Item::BeforeSleep) => self.toggle_before_sleep(),
            Some(Item::DoNotDisturb) => self.step_quiet(1, true),
            Some(Item::Display(display, screen)) => self.change_display(display, screen, 1),
            Some(Item::Output(index)) => self.choose_device(false, index),
            Some(Item::Input(index)) => self.choose_device(true, index),
            Some(Item::Slider(_) | Item::Heading(_) | Item::Silence) | None => {}
        }
        self.settle();
    }

    fn activate_network(&mut self, index: usize) {
        let Some(network) = self.networks().get(index).cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.notice = None;
        if network.connected {
            return self.request(Request::WifiDisconnect);
        }
        match network.security {
            Security::Wep => {
                self.notice = Some("WEP networks are not supported.".to_owned());
            }
            Security::Enterprise if !network.known => {
                self.notice = Some("Enterprise network: give iwd a profile first.".to_owned());
            }
            Security::Passphrase if !network.known => self.ask_passphrase(network.id, false),
            _ => self.connect(network.id, None),
        }
    }

    fn ask_passphrase(&mut self, network: String, rejected: bool) {
        let Some(index) = self.networks().iter().position(|n| n.id == network) else {
            return;
        };
        self.selected = index;
        let mut field = SecretField::new(MAX_PASSPHRASE);
        if rejected {
            field.refuse(&self.ctx.motion, Instant::now());
        }
        self.prompt = Some(Prompt {
            network,
            field,
            rejected,
        });
    }

    fn connect(&mut self, network: String, passphrase: Option<Zeroizing<String>>) {
        self.busy = Some(network.clone());
        self.request(Request::WifiConnect {
            network,
            passphrase,
        });
    }

    fn activate_device(&mut self, index: usize) {
        let Some(device) = self.devices().get(index).cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.notice = None;
        if device.connected {
            self.request(Request::BluetoothDisconnect(device.id));
        } else {
            self.busy = Some(device.id.clone());
            self.request(Request::BluetoothConnect(device.id));
        }
    }

    /// Apply a theme everywhere and recolour the panel itself.
    fn apply_theme(&mut self, index: usize) {
        let Some(theme) = self.ctx.themes.iter().nth(index).cloned() else {
            return;
        };
        let reports = theme::apply(&self.ctx, &theme, &self.config.theme_targets);
        if let Some(err) = theme::failure(&reports) {
            self.notice = Some(sentence(&err));
        }
        self.ctx.reload_theme();
    }

    /// Set the screen's backlight to a raw level: shown at once, applied by
    /// the worker.
    fn set_backlight(&mut self, level: u32) {
        let Some(backlight) = &mut self.hardware.screen else {
            return;
        };
        let level = level.min(backlight.max);
        if backlight.brightness != level {
            backlight.brightness = level;
            self.request(Request::Backlight(level));
        }
    }

    /// Set a slider to `percent`: shown at once, applied by a worker.
    fn set_dial(&mut self, dial: Dial, percent: u8) {
        let percent = percent.min(100);
        match dial {
            Dial::Brightness => {
                if let Some(backlight) = &self.hardware.screen {
                    self.set_backlight(backlight.level(percent));
                }
            }
            Dial::Volume => {
                let output = self.sound.as_mut().and_then(|s| s.output.as_mut());
                if let Some(output) = output.filter(|output| output.volume != percent) {
                    output.volume = percent;
                    self.request(Request::Volume(percent));
                }
            }
            Dial::App(index) => {
                let app = self.sound.as_mut().and_then(|s| s.apps.get_mut(index));
                if let Some(app) = app.filter(|app| app.volume != percent) {
                    app.volume = percent;
                    let name = app.name.clone();
                    self.request(Request::AppVolume(name, percent));
                }
            }
            Dial::Mic => {
                let input = self.sound.as_mut().and_then(|s| s.input.as_mut());
                if let Some(input) = input.filter(|input| input.volume != percent) {
                    input.volume = percent;
                    self.request(Request::MicVolume(percent));
                }
            }
        }
    }

    /// One keyboard step of a slider.
    fn nudge(&mut self, dial: Dial, up: bool) {
        match (dial, &self.hardware.screen) {
            (Dial::Brightness, Some(backlight)) => {
                self.set_backlight(system::step(backlight, None, up));
            }
            (Dial::Brightness, None) => {}
            _ => {
                let Some((volume, _)) = self.dial(dial) else {
                    return;
                };
                let volume = volume.min(100);
                self.set_dial(
                    dial,
                    if up {
                        volume + VOLUME_STEP.min(100 - volume)
                    } else {
                        volume.saturating_sub(VOLUME_STEP)
                    },
                );
            }
        }
    }

    /// Follow the pointer along a slider's track.
    fn drag_to(&mut self, dial: Dial, track: Rect, x: f32) {
        let fraction = ((x - track.x) / track.w).clamp(0.0, 1.0);
        self.set_dial(dial, (fraction * 100.0).round() as u8);
    }

    /// Mute or unmute the output or an application.
    fn toggle_mute(&mut self, dial: Dial) {
        match dial {
            Dial::Brightness => {}
            Dial::Volume => {
                if let Some(output) = self.sound.as_mut().and_then(|s| s.output.as_mut()) {
                    output.muted = !output.muted;
                    let muted = output.muted;
                    self.request(Request::Mute(muted));
                }
            }
            Dial::App(index) => {
                if let Some(app) = self.sound.as_mut().and_then(|s| s.apps.get_mut(index)) {
                    app.muted = !app.muted;
                    let (name, muted) = (app.name.clone(), app.muted);
                    self.request(Request::AppMute(name, muted));
                }
            }
            Dial::Mic => {
                if let Some(input) = self.sound.as_mut().and_then(|s| s.input.as_mut()) {
                    input.muted = !input.muted;
                    let muted = input.muted;
                    self.request(Request::MicMute(muted));
                }
            }
        }
    }

    /// Make a device the output, or the microphone: shown at once, applied
    /// by the worker.
    fn choose_device(&mut self, input: bool, index: usize) {
        let Some(sound) = &mut self.sound else { return };
        let devices = if input {
            &mut sound.inputs
        } else {
            &mut sound.outputs
        };
        let Some(id) = devices.get(index).map(|device| device.id.clone()) else {
            return;
        };
        if devices[index].current {
            return;
        }
        for (i, device) in devices.iter_mut().enumerate() {
            device.current = i == index;
        }
        self.notice = None;
        self.request(if input {
            Request::Input(id)
        } else {
            Request::Output(id)
        });
    }

    /// Read what Settings shows: the config file's `[lock]` and `[notify]`,
    /// with what was chosen here over them.
    fn read_settings(&mut self) {
        self.lock = chosen::section(&self.ctx.paths, "lock");
        let now = enso_core::awake::unix_now();
        let quiet = chosen::Quiet::read(&self.ctx.paths);
        self.quiet = quiet.is_on(now);
        self.quiet_until = quiet.ends(now).unwrap_or(0);
        self.awake = enso_core::Awake::read(&self.ctx.paths);
        self.night = night_light(&self.ctx.paths);
    }

    /// Night light the other way, until the next sunset or sunrise.
    fn toggle_night(&mut self) {
        if self.night.is_none() {
            return;
        }
        // Shown at once; enso-dusk confirms it in its state file.
        self.night = self.night.map(|warm| !warm);
        let dusk = enso_core::exe::companion("enso-dusk");
        let _ = std::thread::Builder::new()
            .name("enso-dusk-toggle".to_owned())
            .spawn(move || {
                if let Err(err) = std::process::Command::new(dusk)
                    .arg("toggle")
                    .stdout(std::process::Stdio::null())
                    .status()
                {
                    eprintln!("enso-ctl: cannot reach enso-dusk: {err}");
                }
            });
        self.settle();
    }

    /// What the switch tiles stand at.
    fn switches(&self) -> draw::Switches {
        let now = enso_core::awake::unix_now();
        draw::Switches {
            quiet: self.quiet.then_some(self.quiet_until),
            awake: self
                .awake
                .is_on(now)
                .then(|| self.awake.ends(now).unwrap_or(0)),
        }
    }

    /// Where Do not disturb stands among the Settings row's choices: off,
    /// an hour, three hours, until morning, on. A time set elsewhere counts
    /// as the shortest choice that covers what's left of it.
    fn quiet_choice(&self, now: u64) -> usize {
        match (self.quiet, self.quiet_until) {
            (false, _) => 0,
            (true, 0) => QUIET_CHOICES - 1,
            (true, until) => match until.saturating_sub(now) {
                left if left <= HOUR => 1,
                left if left <= 3 * HOUR => 2,
                _ => 3,
            },
        }
    }

    /// Step how long Do not disturb lasts, and keep it; `wrap` goes round
    /// from on to off.
    fn step_quiet(&mut self, step: isize, wrap: bool) {
        let now = enso_core::awake::unix_now();
        let to = self.quiet_choice(now) as isize + step;
        let last = QUIET_CHOICES as isize - 1;
        let to = if wrap {
            to.rem_euclid(last + 1)
        } else {
            to.clamp(0, last)
        };
        let until = match to {
            0 => None,
            1 => Some(now + HOUR),
            2 => Some(now + 3 * HOUR),
            3 => Some(next_morning(now)),
            _ => Some(0),
        };
        self.quiet = until.is_some();
        self.quiet_until = until.unwrap_or(0);
        self.keep(
            "notify",
            "do_not_disturb_until",
            Setting::Seconds(self.quiet_until),
        );
        self.keep("notify", "do_not_disturb", Setting::Switch(self.quiet));
    }

    /// Do not disturb, on or off, and keep it. Turned on here it lasts
    /// until it's turned off.
    fn toggle_quiet(&mut self) {
        self.quiet = !self.quiet;
        self.quiet_until = 0;
        self.keep("notify", "do_not_disturb_until", Setting::Seconds(0));
        self.keep("notify", "do_not_disturb", Setting::Switch(self.quiet));
    }

    /// Ask niri for the screens again.
    fn read_displays(&mut self) {
        match displays::read() {
            Ok(displays) => self.displays = displays,
            Err(err) => {
                self.displays.clear();
                self.notice = Some(format!("can't ask niri about the screens: {err}"));
            }
        }
    }

    /// Change one thing about a screen: flip its power or variable
    /// refresh, or step its size or scale `step` along. niri does it at
    /// once; it lasts until niri restarts.
    fn change_display(&mut self, index: usize, screen: Screen, step: isize) {
        let Some(display) = self.displays.get(index).cloned() else {
            return;
        };
        let done = match screen {
            Screen::Name => return,
            Screen::Power => displays::set_on(&display.name, !display.on),
            Screen::Vrr => displays::set_vrr(&display.name, !display.vrr),
            Screen::Mode => match display.mode_step(step) {
                Some(mode) => displays::set_mode(&display.name, mode),
                None => return,
            },
            Screen::Scale => match display.scale_step(step) {
                Some(scale) => displays::set_scale(&display.name, scale),
                None => return,
            },
        };
        let selected = self.items().get(self.selected).copied();
        self.read_displays();
        // Kept as it now is, so niri starts with it next time.
        let kept = match &done {
            Ok(()) => self
                .displays
                .iter()
                .find(|shown| shown.name == display.name)
                .map(|shown| displays::keep(&self.ctx.paths.state_dir, shown))
                .transpose(),
            Err(_) => Ok(None),
        };
        // The same row stays selected, though rows come and go.
        if let Some(position) =
            selected.and_then(|item| self.items().iter().position(|i| *i == item))
        {
            self.selected = position;
        }
        self.notice = match (done, kept) {
            (Ok(()), Ok(_)) => None,
            (Ok(()), Err(err)) => Some(format!("Changed, but can't keep it for next time: {err}")),
            (Err(err), _) => Some(format!("niri says: {err}")),
        };
        self.settle();
    }

    /// Keep the machine awake, or stop.
    fn toggle_awake(&mut self) {
        let now = enso_core::awake::unix_now();
        self.awake = if self.awake.is_on(now) {
            enso_core::Awake::Off
        } else {
            enso_core::Awake::On
        };
        if let Err(err) = self.awake.write(&self.ctx.paths) {
            self.notice = Some(format!("can't keep the setting: {err}"));
        }
        self.settle();
    }

    /// Move a time `step` choices along, and keep it.
    fn step_timer(&mut self, timer: Timer, step: isize, wrap: bool) {
        let seconds = timer.step(self.lock.get(timer), step, wrap);
        if seconds != self.lock.get(timer) {
            self.lock.set(timer, seconds);
            self.keep("lock", timer.key(), Setting::Seconds(seconds));
        }
    }

    /// Lock before sleeping, or don't, and keep it.
    fn toggle_before_sleep(&mut self) {
        self.lock.before_sleep = !self.lock.before_sleep;
        self.keep(
            "lock",
            "before_sleep",
            Setting::Switch(self.lock.before_sleep),
        );
    }

    /// A choice shows at once; a worker writes it where the lock screen,
    /// its watcher, and the notification daemon read it, and says so if it
    /// can't.
    fn keep(&mut self, section: &'static str, key: &'static str, value: Setting) {
        self.notice = None;
        self.request(Request::Keep {
            section,
            key,
            value,
        });
        self.settle();
    }

    /// Turn to the page beside this one: `1` to the right, `-1` to the left.
    fn turn(&mut self, step: isize) -> Outcome {
        let Some(at) = SIDE_BY_SIDE.iter().position(|page| *page == self.page) else {
            return Outcome::Ignored;
        };
        let to = at as isize + step;
        match usize::try_from(to).ok().and_then(|to| SIDE_BY_SIDE.get(to)) {
            Some(&page) => {
                self.open(page);
                Outcome::Changed
            }
            None => Outcome::Ignored,
        }
    }

    /// Switch to the next power profile.
    fn cycle_profile(&mut self) {
        let Some(profile) = &mut self.hardware.profile else {
            return;
        };
        if !profile.writable {
            self.notice = Some("Only root may switch the power profile here.".to_owned());
        } else if let Some(next) = profile.next().map(str::to_owned) {
            profile.current.clone_from(&next);
            self.notice = None;
            self.request(Request::Profile(next));
        }
        self.settle();
    }

    fn move_selection(&mut self, nav: Nav) {
        let count = self.items().len();
        if count == 0 {
            return;
        }
        // Step over headings, wrapping at the ends.
        let items = self.items();
        let mut next = self.selected;
        for _ in 0..count {
            next = match nav {
                Nav::Down => (next + 1) % count,
                Nav::Up => (next + count - 1) % count,
            };
            if Self::selectable(items[next]) {
                self.selected = next;
                break;
            }
        }
        self.settle();
    }

    // ---- Input ---------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) -> Outcome {
        if self.prompt.is_some() {
            return self.on_prompt_key(key);
        }
        if let Some(nav) = key.nav(true) {
            self.move_selection(nav);
            return Outcome::Changed;
        }
        // Letters only count as shortcuts when typed plainly.
        let pressed = match key.key {
            Key::Char(_) if !key.mods.is_plain() => return Outcome::Ignored,
            other => other,
        };
        let item = self.items().get(self.selected).copied();
        let on_main = self.page == Page::Main;
        let on_home = self.side_by_side();
        // The pages side by side turn with Tab, the page keys, and brackets.
        if self.side_by_side() {
            let step = match pressed {
                Key::Tab if key.mods.shift => Some(-1),
                Key::Tab | Key::PageDown | Key::Char(']') => Some(1),
                Key::PageUp | Key::Char('[') => Some(-1),
                _ => None,
            };
            if let Some(step) = step {
                // At either end Tab goes round; the others stop.
                let step = if matches!(pressed, Key::Tab) && self.turn(step) == Outcome::Ignored {
                    -step
                } else {
                    step
                };
                self.turn(step);
                return Outcome::Changed;
            }
        }
        match (pressed, item) {
            (Key::Char('q'), _) => return Outcome::Close,
            // A time steps with left and right, and Enter goes round.
            (Key::Left | Key::Char('h'), Some(Item::Timer(timer))) => {
                self.step_timer(timer, -1, false);
            }
            (Key::Right | Key::Char('l'), Some(Item::Timer(timer))) => {
                self.step_timer(timer, 1, false);
            }
            (Key::Left | Key::Char('h'), Some(Item::DoNotDisturb)) => self.step_quiet(-1, false),
            (Key::Right | Key::Char('l'), Some(Item::DoNotDisturb)) => self.step_quiet(1, false),
            (Key::Char(' '), Some(Item::BeforeSleep | Item::DoNotDisturb | Item::Timer(_))) => {
                self.activate(self.selected);
            }
            // A screen's size and scale step with left and right.
            (
                Key::Left | Key::Char('h'),
                Some(Item::Display(display, screen @ (Screen::Mode | Screen::Scale))),
            ) => {
                self.change_display(display, screen, -1);
            }
            (
                Key::Right | Key::Char('l'),
                Some(Item::Display(display, screen @ (Screen::Mode | Screen::Scale))),
            ) => {
                self.change_display(display, screen, 1);
            }
            (Key::Char(' '), Some(Item::Display(..))) => self.activate(self.selected),
            (Key::Escape, _) if on_home => return Outcome::Close,
            // On a slider, left and right adjust it — also on a page, where
            // they would otherwise lead back.
            (Key::Left | Key::Char('h'), Some(Item::Slider(dial))) => self.nudge(dial, false),
            (Key::Right | Key::Char('l'), Some(Item::Slider(dial))) => self.nudge(dial, true),
            (Key::Escape | Key::Backspace | Key::Left | Key::Char('h'), _) if !on_home => {
                self.open(self.home);
            }
            // On Settings, left is the first page.
            (Key::Left | Key::Char('h'), _) if self.page == Page::Settings => {
                self.turn(-1);
            }
            // The volume leads to the Sound page; m mutes what is selected.
            (Key::Enter, Some(Item::Slider(Dial::Volume))) if on_main => self.open(Page::Sound),
            (Key::Char('m'), Some(Item::Slider(dial))) => self.toggle_mute(dial),
            (Key::Char('m'), _) if on_main || self.page == Page::Sound => {
                self.toggle_mute(Dial::Volume);
            }
            (Key::Enter | Key::Right | Key::Char('l'), Some(Item::Open(page))) => self.open(page),
            (Key::Enter, Some(_)) => self.activate(self.selected),
            // Space flips the radio of the row or page it is pressed on.
            (Key::Char(' '), Some(Item::Open(page @ (Page::Wifi | Page::Bluetooth)))) => {
                self.toggle(page);
            }
            (Key::Char(' '), _) if matches!(self.page, Page::Wifi | Page::Bluetooth) => {
                self.toggle(self.page);
            }
            (Key::Char(' '), Some(Item::Theme(_))) => self.activate(self.selected),
            (Key::Char('p'), _) if on_main => self.cycle_profile(),
            (Key::Char('d'), _) if on_main => self.toggle_quiet(),
            (Key::Char('a'), _) if on_main => self.toggle_awake(),
            (Key::Char('n'), _) if on_main => self.toggle_night(),
            _ => return Outcome::Ignored,
        }
        Outcome::Changed
    }

    /// Keys while a passphrase is being typed: they are text.
    fn on_prompt_key(&mut self, key: KeyEvent) -> Outcome {
        let Some(prompt) = &mut self.prompt else {
            return Outcome::Ignored;
        };
        let now = Instant::now();
        prompt.field.note_mods(key.mods);
        match prompt.field.key(&key, &self.ctx.motion, now) {
            secret::Response::Cancel => self.prompt = None,
            secret::Response::Submit if prompt.field.is_empty() => return Outcome::Ignored,
            secret::Response::Submit => {
                if let Some(Prompt {
                    network, mut field, ..
                }) = self.prompt.take()
                {
                    let passphrase = field.take(&self.ctx.motion, now);
                    self.connect(network, Some(passphrase));
                }
            }
            secret::Response::Changed => {}
            // Keys not for the field still aren't the panel's while it asks.
            secret::Response::Ignored => return Outcome::Ignored,
        }
        self.settle();
        Outcome::Changed
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| *hit)
    }

    fn on_press(&mut self, x: f32, y: f32) -> Outcome {
        match self.hit(x, y) {
            Some(Hit::Slider(index, dial, track)) => {
                self.selected = index;
                self.dragging = Some((dial, track));
                self.drag_to(dial, track, x);
                self.settle();
            }
            Some(Hit::Mute(dial)) => self.toggle_mute(dial),
            Some(Hit::OpenSound(_)) => self.open(Page::Sound),
            Some(Hit::MicSwitch) => self.toggle_mute(Dial::Mic),
            Some(Hit::Item(index)) => {
                // Clicking elsewhere while a passphrase is asked for drops it.
                if self.prompt_index().is_some_and(|prompt| prompt != index) {
                    self.prompt = None;
                }
                self.selected = index;
                self.activate(index);
            }
            Some(Hit::Switch(page)) => self.toggle(page),
            Some(Hit::Back) => self.open(self.home),
            Some(Hit::Profile) => self.cycle_profile(),
            Some(Hit::Quiet) => self.toggle_quiet(),
            Some(Hit::NightLight) => self.toggle_night(),
            Some(Hit::DisplayStep(index, step)) => {
                self.selected = index;
                if let Some(Item::Display(display, screen)) = self.items().get(index).copied() {
                    self.change_display(display, screen, step);
                }
            }
            Some(Hit::Awake) => self.toggle_awake(),
            Some(Hit::Step(index, timer, step)) => {
                self.selected = index;
                self.step_timer(timer, step, false);
            }
            Some(Hit::QuietStep(index, step)) => {
                self.selected = index;
                self.step_quiet(step, false);
            }
            Some(Hit::Dot(page)) => self.open(page),
            None if self.card.contains(x, y) => return Outcome::Ignored,
            // A click outside dismisses a panel that covers the screen; a
            // dropdown never sees one.
            None => return Outcome::Close,
        }
        Outcome::Changed
    }

    fn on_move(&mut self, x: f32, y: f32) -> Outcome {
        if let Some((dial, track)) = self.dragging {
            self.drag_to(dial, track, x);
            return Outcome::Changed;
        }
        let index = match self.hit(x, y) {
            Some(
                Hit::Item(index)
                | Hit::Slider(index, ..)
                | Hit::Step(index, ..)
                | Hit::QuietStep(index, _)
                | Hit::OpenSound(index),
            ) => index,
            _ => return Outcome::Ignored,
        };
        // The pointer selects what it rests on — but not out from under a
        // passphrase being typed.
        if index == self.selected || self.prompt.is_some() {
            return Outcome::Ignored;
        }
        self.selected = index;
        self.settle();
        Outcome::Changed
    }

    /// A sideways swipe on the touchpad, or a tilt of the wheel, turns the
    /// pages that sit side by side.
    fn on_swipe(&mut self, dx: f32, wheel: bool) -> Outcome {
        if !self.side_by_side() || self.prompt.is_some() || dx == 0.0 {
            return Outcome::Ignored;
        }
        // A swipe back the other way starts afresh.
        if self.swipe * dx < 0.0 {
            self.swipe = 0.0;
        }
        self.swipe += dx;
        if !wheel && self.swipe.abs() < SWIPE {
            return Outcome::Ignored;
        }
        let step = if self.swipe > 0.0 { 1 } else { -1 };
        self.swipe = 0.0;
        self.turn(step)
    }

    fn on_scroll(&mut self, dy: f32) -> Outcome {
        let count = self.items().len();
        let rows = self.visible_rows();
        if self.page == Page::Main || count <= rows || self.prompt.is_some() {
            return Outcome::Ignored;
        }
        self.scroll += dy;
        let steps = (self.scroll / SCROLL_STEP).trunc();
        if steps == 0.0 {
            return Outcome::Ignored;
        }
        self.scroll -= steps * SCROLL_STEP;
        let last = (count - rows) as f32;
        self.offset = (self.offset as f32 + steps).clamp(0.0, last) as usize;
        // The selection stays where it can be seen.
        self.selected = self.selected.clamp(self.offset, self.offset + rows - 1);
        self.settle();
        Outcome::Changed
    }

    // ---- Dropdown lifetime ---------------------------------------------------

    /// A dropdown stays while the pointer is on it or on the island that
    /// opened it, while a slider is held, or while a passphrase is being
    /// typed (the pointer wanders off during that).
    fn wanted(&self) -> bool {
        self.pointer_inside
            || self.owner_hovering
            || self.dragging.is_some()
            || self.prompt.is_some()
    }

    /// Start or stop the countdown to closing a dropdown nobody is on.
    fn watch_pointer(&mut self, cx: &mut Cx<'_>) {
        if self.mode == PanelMode::Modal || self.presence.is_closing() {
            return;
        }
        if self.wanted() {
            self.leave_at = None;
        } else if self.leave_at.is_none() {
            self.leave_at = Some(Instant::now() + GRACE);
            cx.wake_in(GRACE);
        }
    }

    /// Limit a dropdown's input to its card and the gap above (or below) it,
    /// so the rest of its surface neither blocks clicks nor counts as hover.
    /// And keep its hands off the keyboard: hovering the bar must never take
    /// typing away from a window. Only a passphrase field borrows it.
    fn update_input(&self, cx: &mut Cx<'_>) {
        let PanelMode::Dropdown(edge) = self.mode else {
            return;
        };
        cx.set_keyboard(if self.prompt.is_some() {
            Keyboard::Exclusive
        } else {
            Keyboard::None
        });
        let (width, height) = cx.size();
        let bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
        let card = self.card_rect(bounds, self.height.target());
        let region = match edge {
            Edge::Top => Rect::new(card.x, 0.0, card.w, card.bottom()),
            Edge::Bottom => Rect::new(card.x, card.y, card.w, bounds.h - card.y),
        };
        cx.set_input_region(Some(region));
    }

    fn finish(&mut self, outcome: Outcome, cx: &mut Cx<'_>) {
        match outcome {
            Outcome::Ignored => {}
            Outcome::Changed => {
                self.update_input(cx);
                cx.redraw();
            }
            Outcome::Close => self.presence.close(cx),
        }
    }
}

/// An error message as a sentence for the notice line.
fn sentence(message: &str) -> String {
    let mut chars = message.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut sentence: String = first.to_uppercase().chain(chars).collect();
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

/// One line saying where Wi-Fi stands.
fn wifi_summary(wifi: Option<&Wifi>) -> String {
    match wifi {
        None => "Unavailable".to_owned(),
        Some(wifi) if !wifi.powered => "Off".to_owned(),
        Some(wifi) => match (wifi.connected(), wifi.link) {
            (Some(network), _) => network.name.clone(),
            (None, Link::Connecting) => "Connecting…".to_owned(),
            (None, _) => "Not connected".to_owned(),
        },
    }
}

/// One line saying where Bluetooth stands.
fn bluetooth_summary(bluetooth: Option<&Bluetooth>) -> String {
    match bluetooth {
        None => "Unavailable".to_owned(),
        Some(bluetooth) if !bluetooth.powered => "Off".to_owned(),
        Some(bluetooth) => {
            let connected: Vec<&Device> = bluetooth.connected().collect();
            match connected.as_slice() {
                [] => "Not connected".to_owned(),
                [device] => device.name.clone(),
                several => format!("{} devices", several.len()),
            }
        }
    }
}

impl App for Panel {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        match self.mode {
            PanelMode::Modal => SurfaceSpec::overlay("enso-ctl"),
            // Hangs off the bar's edge of the screen, below (or above) the
            // bar itself, without the keyboard (see `update_input`).
            PanelMode::Dropdown(edge) => SurfaceSpec {
                width: self.config.width + (MARGIN + SHADOW_ROOM) as u32,
                height: (Self::max_height() + DROP_GAP + SHADOW_ROOM).ceil() as u32,
                keyboard: Keyboard::None,
                ..SurfaceSpec::new(
                    "enso-ctl",
                    Layer::Overlay,
                    Anchor::RIGHT
                        | match edge {
                            Edge::Top => Anchor::TOP,
                            Edge::Bottom => Anchor::BOTTOM,
                        },
                )
            },
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        self.system = System::start(self.ctx.paths.clone(), {
            let proxy = proxy.clone();
            move |update| proxy.send(Message::System(Box::new(update)))
        });
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-ctl: live theme switching unavailable: {err}");
        }
        // Told to go by whoever wants its place: see `main::open_panel`.
        if let Err(err) = enso_core::ipc::listen(NAME, {
            let proxy = proxy.clone();
            move |word| match word.strip_prefix("page ") {
                Some(name) => Page::named(name).is_none_or(|page| proxy.send(Message::Page(page))),
                None => word != "close" || proxy.send(Message::Dismiss),
            }
        }) {
            eprintln!("enso-ctl: a second Super+C cannot close this one: {err}");
        }
        self.go_asked(true);
        if self.mode != PanelMode::Modal {
            // Without the bar's word, the pointer on the card decides alone.
            self.owner_hovering = follow_owner(proxy);
            self.update_input(cx);
            self.watch_pointer(cx);
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let outcome = match event {
            Event::Message(Message::System(update)) => {
                self.apply(*update);
                // Its service has answered: a page asked for may show now.
                self.go_asked(false);
                Outcome::Changed
            }
            Event::Message(Message::Page(page)) => {
                self.asked = Some(page);
                self.go_asked(false);
                Outcome::Changed
            }
            Event::Message(Message::ThemeChanged) => {
                // Settings are read as the panel opens, not again: what it
                // keeps itself lands here too, and mustn't move a switch
                // back while the panel's own write is on its way.
                self.ctx.reload_theme();
                Outcome::Changed
            }
            Event::Message(Message::Owner(hovering)) => {
                self.owner_hovering = hovering;
                Outcome::Ignored
            }
            Event::Message(Message::Dismiss) => Outcome::Close,
            Event::Wake if self.presence.is_closing() => return cx.exit(),
            Event::Wake => {
                let due = self
                    .leave_at
                    .is_some_and(|leave_at| Instant::now() >= leave_at);
                if due && !self.wanted() {
                    Outcome::Close
                } else {
                    Outcome::Ignored
                }
            }
            Event::Resized { .. } => Outcome::Changed,
            Event::Key(key) => self.on_key(key),
            Event::Pointer(PointerEvent::Enter { x, y }) => {
                self.pointer_inside = true;
                self.on_move(x, y)
            }
            Event::Pointer(PointerEvent::Leave) => {
                self.pointer_inside = false;
                Outcome::Ignored
            }
            Event::Pointer(PointerEvent::Move { x, y }) => self.on_move(x, y),
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => self.on_press(x, y),
            Event::Pointer(PointerEvent::Release {
                button: Button::Left,
                ..
            }) => {
                self.dragging = None;
                Outcome::Ignored
            }
            Event::Pointer(PointerEvent::Scroll { dx, dy, wheel, .. }) => {
                if dx.abs() > dy.abs() {
                    self.on_swipe(dx, wheel)
                } else {
                    self.on_scroll(dy)
                }
            }
            _ => Outcome::Ignored,
        };
        self.finish(outcome, cx);
        self.watch_pointer(cx);
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.paint(canvas, now)
    }
}

/// Follow the bar's side of the conversation on standard input: `enter` and
/// `leave` as the pointer comes and goes from the island that opened this
/// dropdown, and `close` when the bar wants the space for something else.
/// The bar going away counts as leaving. Returns whether anyone
/// is listening.
fn follow_owner(proxy: Proxy<Message>) -> bool {
    thread::Builder::new()
        .name("enso-owner".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let hovering = match line.as_deref().map(str::trim) {
                    Ok("enter") => true,
                    Ok("leave") => false,
                    Ok("close") => {
                        proxy.send(Message::Dismiss);
                        return;
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                };
                if !proxy.send(Message::Owner(hovering)) {
                    return;
                }
            }
            proxy.send(Message::Owner(false));
        })
        .is_ok()
}

/// Whether enso-dusk has the screens warm: its state file's `warm` line.
/// `None` when enso-dusk isn't installed.
fn night_light(paths: &enso_core::Paths) -> Option<bool> {
    let text = std::fs::read_to_string(paths.state_dir.join("dusk")).ok();
    let installed = text.is_some() || {
        let dusk = enso_core::exe::companion("enso-dusk");
        dusk.is_file()
            || std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| dir.join("enso-dusk").is_file())
            })
    };
    installed.then(|| text.is_some_and(|text| text.lines().any(|line| line.trim() == "warm 1")))
}

#[cfg(test)]
mod tests;
