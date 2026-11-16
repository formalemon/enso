//! The control center panel: a floating card in the corner of the screen.
//!
//! The first page is a glance and the quick controls — power readings,
//! brightness, volume, and one row each for Wi-Fi, Bluetooth, and the theme.
//! A row opens a page of its own: networks in range, devices, themes — and
//! the volume opens Sound: which output, each application's level, and the
//! microphone.
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

mod draw;

use std::{
    io::BufRead,
    thread,
    time::{Duration, Instant},
};

use enso_core::{Context, Transition, Tween, style::space};
use enso_sys::{
    audio::Sound,
    bluetooth::{Bluetooth, Device},
    wifi::{ConnectError, Link, Network, Security, Wifi},
};
use enso_ui::{
    Anchor, App, Button, Canvas, Cx, Event, Key, KeyEvent, Keyboard, Layer, Nav, Paint,
    PointerEvent, Presence, Proxy, Rect, SurfaceSpec, watch::watch_dir,
};
use serde::Deserialize;

use crate::{
    cli::{Edge, PanelMode},
    system::{self, Hardware, Request, System, Update},
    theme,
};

/// The `[ctl]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CtlConfig {
    /// Panel width in logical pixels.
    pub width: u32,
}

impl Default for CtlConfig {
    fn default() -> Self {
        Self { width: 340 }
    }
}

/// Gap between the card and the screen edge.
const MARGIN: f32 = space::SM;
/// Gap between the bar and a dropdown card.
const DROP_GAP: f32 = space::XS;
/// Room a dropdown's surface keeps around the card for its shadow.
const SHADOW_ROOM: f32 = 28.0;
const PAD: f32 = space::LG;
const TILE_HEIGHT: f32 = 60.0;
const ROW_HEIGHT: f32 = 44.0;
const NOTICE_HEIGHT: f32 = 28.0;
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
/// How long a dropdown outlives the pointer leaving it, so that crossing the
/// gap between the bar and the card does not close it.
const GRACE: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub enum Message {
    System(Box<Update>),
    ThemeChanged,
    /// From the bar: whether the pointer is on the island that opened us.
    Owner(bool),
}

/// What the panel is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
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

/// A part of the Sound page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Output,
    Apps,
    Microphone,
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
    /// The chevron at the end of the volume row: the Sound page.
    OpenSound,
    /// The switch on the Sound page's microphone heading.
    MicSwitch,
    /// The on/off switch of Wi-Fi or Bluetooth.
    Switch(Page),
    /// The header of a page: back to the first one.
    Back,
    /// The power profile tile.
    Profile,
}

/// A passphrase being typed for a network.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Prompt {
    /// The network's id.
    network: String,
    text: String,
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
    /// Where the selection was on the first page, to return to.
    main_selected: usize,
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

    /// Dropdown only: the pointer is over the card.
    pointer_inside: bool,
    /// Dropdown only: the pointer is over the bar island that opened us.
    owner_hovering: bool,
    /// Dropdown only: when to close unless the pointer comes back.
    leave_at: Option<Instant>,

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
            main_selected: 0,
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
            wifi_switch: Tween::new(0.0),
            bluetooth_switch: Tween::new(0.0),
            pointer_inside: false,
            // Whoever opened a dropdown is hovering until they say otherwise.
            owner_hovering: matches!(mode, PanelMode::Dropdown(_)),
            leave_at: None,
            card: Rect::default(),
            hits: Vec::new(),
        };
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
        !matches!(item, Item::Heading(_) | Item::Silence)
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
                items.extend([Page::Wifi, Page::Bluetooth, Page::Theme].map(Item::Open));
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
        }
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
            Page::Main => self.items().len(),
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
                let tiles = if draw::tiles(&self.hardware).is_empty() {
                    0.0
                } else {
                    TILE_HEIGHT + space::MD
                };
                PAD + tiles + rows(self.visible_rows()) + notice + space::SM
            }
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
        let main = PAD + TILE_HEIGHT + space::MD + 5.0 * ROW_HEIGHT;
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
                        self.open(Page::Main);
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
                        self.open(Page::Main);
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
                        self.open(Page::Main);
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
        match self.page {
            Page::Main => self.main_selected = self.selected,
            // Only look for devices while someone is looking at them.
            Page::Bluetooth => self.request(Request::BluetoothDiscover(false)),
            _ => {}
        }
        self.page = page;
        self.prompt = None;
        self.notice = None;
        self.offset = 0;
        self.scroll = 0.0;
        self.selected = match page {
            Page::Main => self.main_selected,
            Page::Theme => self
                .ctx
                .themes
                .iter()
                .position(|theme| theme.id == self.ctx.theme.id)
                .unwrap_or(0),
            Page::Wifi | Page::Bluetooth | Page::Sound => 0,
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

        // The new page slides in from the side it lives on; the highlight is
        // simply there, not carried over from the page before.
        self.slide_from = if page == Page::Main { -1.0 } else { 1.0 };
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
        self.prompt = Some(Prompt {
            network,
            text: String::new(),
            rejected,
        });
    }

    fn connect(&mut self, network: String, passphrase: Option<String>) {
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
        if let Err(err) = theme::apply(&self.ctx, &theme) {
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
        match (pressed, item) {
            (Key::Char('q'), _) => return Outcome::Close,
            (Key::Escape, _) if on_main => return Outcome::Close,
            // On a slider, left and right adjust it — also on a page, where
            // they would otherwise lead back.
            (Key::Left | Key::Char('h'), Some(Item::Slider(dial))) => self.nudge(dial, false),
            (Key::Right | Key::Char('l'), Some(Item::Slider(dial))) => self.nudge(dial, true),
            (Key::Escape | Key::Backspace | Key::Left | Key::Char('h'), _) if !on_main => {
                self.open(Page::Main);
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
            _ => return Outcome::Ignored,
        }
        Outcome::Changed
    }

    /// Keys while a passphrase is being typed: they are text.
    fn on_prompt_key(&mut self, key: KeyEvent) -> Outcome {
        let Some(prompt) = &mut self.prompt else {
            return Outcome::Ignored;
        };
        match key.key {
            Key::Escape => self.prompt = None,
            Key::Enter if !prompt.text.is_empty() => {
                let Prompt { network, text, .. } = prompt.clone();
                self.prompt = None;
                self.connect(network, Some(text));
            }
            Key::Backspace => {
                prompt.text.pop();
            }
            Key::Char('u') if key.mods.ctrl => prompt.text.clear(),
            _ => match key.text() {
                Some(c) => prompt.text.push(c),
                None => return Outcome::Ignored,
            },
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
            Some(Hit::OpenSound) => self.open(Page::Sound),
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
            Some(Hit::Back) => self.open(Page::Main),
            Some(Hit::Profile) => self.cycle_profile(),
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
            Some(Hit::Item(index) | Hit::Slider(index, ..)) => index,
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
        self.system = System::start({
            let proxy = proxy.clone();
            move |update| proxy.send(Message::System(Box::new(update)))
        });
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-ctl: live theme switching unavailable: {err}");
        }
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
                Outcome::Changed
            }
            Event::Message(Message::ThemeChanged) => {
                self.ctx.reload_theme();
                Outcome::Changed
            }
            Event::Message(Message::Owner(hovering)) => {
                self.owner_hovering = hovering;
                Outcome::Ignored
            }
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
            Event::Pointer(PointerEvent::Scroll { dy, .. }) => self.on_scroll(dy),
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
/// dropdown. The bar going away counts as leaving. Returns whether anyone
/// is listening.
fn follow_owner(proxy: Proxy<Message>) -> bool {
    thread::Builder::new()
        .name("enso-owner".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let hovering = match line.as_deref().map(str::trim) {
                    Ok("enter") => true,
                    Ok("leave") => false,
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

#[cfg(test)]
mod tests;
