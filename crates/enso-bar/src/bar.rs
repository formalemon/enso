//! The bar itself: floating islands — windows, clock, status, and the bell.
//!
//! At the right, each island has one card of its own. The status island —
//! network and volume glyphs, the battery where there is one, whatever is
//! out of the ordinary — warms a shade under the pointer, because resting
//! there (or clicking) opens the control center. The bell sits on an island
//! of its own at the very end: resting there opens the notification
//! history, and a click hushes it (Do not disturb) or wakes it again.
//! Reading what came in is never by way of the controls. Each card grows
//! out of its own island (see [`enso_ui::emerge`]).
//!
//! Their icons are drawn, not taken from a font (see [`icons`]): Wi-Fi or a
//! cable, the battery — a pill holding its percentage, filled
//! as far as it is charged — and the bell, which rings when something new
//! arrives. They are sized from the bar's height, so a taller or shorter bar
//! keeps the same islands, larger or smaller.
//!
//! The left island is workspace-centric. At rest it shows one dot per window
//! on the current workspace, the focused one stretched into a pill. Resting
//! the pointer on it opens it out to every workspace, each a group of dots,
//! with a soft capsule behind the current one.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use enso_core::{
    Awake, Context, Easing, Figure, Notices, Rgba, Transition, Tween, awake, island,
    style::{edge, space, type_scale},
};
use enso_sys::{
    net::{self, Link},
    power::{Battery, BatteryStatus, Sysfs},
};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Layer, Paint, PointerEvent, Proxy, Rect, Shape,
    SurfaceSpec, Text, watch::watch_dir,
};

use crate::{
    config::{BarConfig, DEFAULT_CLOCK, Position},
    dropdown::{Dropdown, History},
    icons::{self, Charge},
    light,
    music::Music,
    niri::{self, Around, Seen, Workspace},
    osd::{Gauge, Osd},
    sound::{self, Heard, Level, Mic},
    water,
};

/// Gap between the islands and the screen edges.
const EDGE: f32 = edge::SCREEN;
/// Vertical inset of the islands inside the bar.
const INSET: f32 = edge::ISLAND_INSET;
/// Between the status island and the bell's: what lies between an island
/// and the screen's edge, or the card under it.
const APART: f32 = edge::SCREEN;
/// Diameter of a window dot, and height of the focused window's pill.
const DOT: f32 = 8.0;
/// Width of the focused window's pill.
const ACTIVE_WIDTH: f32 = 24.0;
/// Space between neighbouring dots.
const DOT_GAP: f32 = space::SM;
/// Padding around the dots of one workspace; the current workspace's capsule
/// fills it.
const GROUP_PAD: f32 = space::SM;
/// Padding between the island's ends and its outermost groups. Together with
/// [`GROUP_PAD`] it gives the resting island `space::MD` either side.
const ISLAND_PAD: f32 = space::MD - GROUP_PAD;
/// Extra space between two workspaces' groups.
const GROUP_GAP: f32 = space::XS;
/// How long the pointer rests on an island before what it opens — the
/// control center, what's playing — opens: long enough that passing over
/// it opens nothing.
const HOVER_INTENT: Duration = Duration::from_millis(150);
/// How often the readings nothing announces (network, battery) are looked at.
const POLL: Duration = Duration::from_secs(5);
/// Messages from background threads.
#[derive(Debug)]
pub enum Message {
    Workspaces(Vec<Workspace>),
    /// Sharing, the keyboard layout, the windows' titles.
    Around(Around),
    /// The sound server's news: the output's level, the microphone.
    Heard(Heard),
    /// The screen's brightness, or the keyboard's light, changed to this
    /// many percent.
    Light(light::Lamp, u8),
    ThemeChanged,
    /// Whether anything is playing, from enso-play.
    Playing(bool),
}

/// How slowly the clock's water fills and drains.
const POUR: Transition = Transition::new(900, Easing::InOutCubic);

/// The widest a window's name gets in the clock island.
const MAX_TITLE: f32 = 420.0;

/// How often the steam over the cup moves on while the machine is kept
/// awake: slow wisps need no more.
const STEAM_TICK: Duration = Duration::from_millis(50);
/// While the battery charges, a light runs along its charge for `SHINE`
/// seconds, once every `SHINE_EVERY`, a frame each `SHINE_TICK`.
const SHINE: f32 = 1.4;
const SHINE_EVERY: f32 = 4.5;
const SHINE_TICK: Duration = Duration::from_millis(33);

/// How long the bell rings when something new arrives.
const RING: Duration = Duration::from_millis(900);

/// One thing the status island shows, left to right.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Item {
    /// The screen (or a window) is being shared; a click stops it.
    Share,
    /// Someone is listening to the microphone; `true` while it's muted.
    Mic(bool),
    /// The output is muted.
    Muted,
    /// Kept awake: the idle lock is held off.
    Awake,
    /// The keyboard layout, when there are several.
    Layout,
    Network(Link),
    Battery(Charge),
    Bell,
}

/// What a click on the workspace island does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Window(u64),
    Workspace(u64),
}

/// One dot: a window, or the stand-in for an empty workspace.
#[derive(Clone, Copy, Debug)]
struct Dot {
    rect: Rect,
    color: Rgba,
    window: Option<u64>,
}

/// One workspace's dots.
#[derive(Clone, Debug)]
struct Group {
    workspace: u64,
    rect: Rect,
    /// How much of the group is showing: `0` folded away, `1` fully open.
    open: f32,
    /// How much this is the workspace being looked at (`0..=1`).
    current: f32,
    dots: Vec<Dot>,
}

/// Geometry of the workspace island at one moment.
#[derive(Clone, Debug)]
struct Island {
    rect: Rect,
    groups: Vec<Group>,
}

#[derive(Debug)]
pub struct Bar {
    ctx: Context,
    config: BarConfig,
    sysfs: Sysfs,
    workspaces: Vec<Workspace>,
    /// How much each workspace is the current one (`0..=1`), animated, by
    /// workspace id.
    current: HashMap<u64, Tween<f32>>,
    /// How "focused" each window looks (`0..=1`), animated, by window id.
    activity: HashMap<u64, Tween<f32>>,
    /// `0` at rest, `1` opened out to every workspace.
    expand: Tween<f32>,
    /// The pointer is resting on the workspace island.
    hovered: bool,
    /// Last known pointer position, while it is over the bar.
    pointer: Option<(f32, f32)>,
    /// The strip the islands sit in, from the last frame.
    strip: Rect,
    /// Click targets from the last frame; the first match wins.
    hits: Vec<(Rect, Target)>,
    /// Output this bar is on; workspaces of other outputs are hidden.
    output: Option<String>,
    clock: String,
    battery: Option<Battery>,
    /// How the machine is online, if it has the hardware to be.
    link: Option<Link>,
    /// The output's volume, if there is an output: what scrolling on the
    /// status island steps from.
    sound: Option<Level>,
    /// Scrolling on the status island, on its way to a volume step.
    scroll: sound::Scroll,
    /// When the clock next needs redrawing.
    clock_due: Instant,
    /// When to look at the network and battery again.
    poll_due: Instant,
    /// How much the status island is lit by the pointer (`0..=1`).
    status_glow: Tween<f32>,
    /// The status island, from the last frame, if there is one.
    status: Option<Rect>,
    /// The status island at rest, without a level widening it: where the
    /// control center grows out of.
    status_rest: Option<Rect>,
    /// The pointer is resting on the status island.
    status_hovered: bool,
    /// The bell's island, from the last frame.
    bell: Option<Rect>,
    /// How much the bell's island is lit by the pointer (`0..=1`).
    bell_glow: Tween<f32>,
    /// The pointer is on the bell.
    bell_hovered: bool,
    /// When the pointer will have rested on the bell long enough to mean it.
    bell_due: Option<Instant>,
    /// How hushed the bell is (`0..=1`): its strike drawing across as Do
    /// not disturb comes on, and back as it goes.
    hush: Tween<f32>,
    /// When the pointer will have rested there long enough to mean it.
    intent_due: Option<Instant>,
    /// The control center, opened from the status island.
    dropdown: Dropdown,
    /// The notification history, opened by clicking the bell.
    history: History,
    /// Where the clock island comes to rest.
    clock_rest: Option<Rect>,
    /// The clock island, from the last frame.
    clock_island: Option<Rect>,
    /// The pointer is on the clock.
    clock_hovered: bool,
    /// When the pointer will have rested on the clock long enough to mean it.
    playing_due: Option<Instant>,
    /// What's playing, opened from the clock.
    playing: Dropdown,
    /// Whether anything plays, and a click on the clock to play or pause.
    music: Music,
    /// How full of water the clock is (`0..=1`): full while playing.
    water: Tween<f32>,
    /// When the water next moves on, while it moves.
    water_due: Option<Instant>,
    /// When the bar started: the water's clock.
    epoch: Instant,
    /// What the notification daemon last reported.
    notices: Notices,
    /// When the bell began ringing, if it is.
    ringing: Option<Instant>,
    /// The level the island shows as it changes.
    osd: Osd,
    /// The microphone, as last heard.
    mic: Mic,
    /// The sound server has been heard from: from then on a change is news.
    heard: bool,
    /// The screen's brightness, once known.
    light: Option<u8>,
    /// The keyboard's light, in percent, as last heard.
    keyboard_light: Option<u8>,
    /// Sharing, the keyboard layout, the windows' titles.
    around: Around,
    /// Keeping awake, as chosen.
    awake: Awake,
    /// Do not disturb is on: the bell is quietened.
    quiet: bool,
    /// The window whose dot is under the pointer: the clock says its name.
    pointed: Option<u64>,
    /// The clock island's width, gliding as what it says changes length.
    clock_width: Option<Tween<f32>>,
    /// The sharing mark, from the last frame: a click there stops sharing.
    share: Option<Rect>,
    /// The calendar, opened from the clock when nothing plays.
    calendar: Dropdown,
    /// Something is playing, as enso-play last said.
    is_playing: bool,
    /// Whether to keep a note of where the islands are, for what grows out
    /// of them: the bar on a screen of its own does.
    notes: bool,
    /// The note last kept, and for which screen.
    noted: Option<(String, island::Island)>,
    /// When the small motions — steam, the battery's light — next move on.
    ambient_due: Option<Instant>,
}

impl Bar {
    /// The bar for screen `output`: its workspaces, on it.
    #[must_use]
    pub fn on(mut self, output: String) -> Self {
        self.output = Some(output);
        self.notes = true;
        self
    }

    pub fn new(ctx: Context, config: BarConfig) -> Self {
        let strip = Rect::new(0.0, 0.0, 0.0, config.height as f32).inset_xy(EDGE, INSET);
        let mut bar = Self {
            ctx,
            config,
            sysfs: Sysfs::system(),
            workspaces: Vec::new(),
            current: HashMap::new(),
            activity: HashMap::new(),
            expand: Tween::new(0.0),
            hovered: false,
            pointer: None,
            strip,
            hits: Vec::new(),
            output: None,
            clock: String::new(),
            battery: None,
            link: None,
            sound: None,
            scroll: sound::Scroll::default(),
            clock_due: Instant::now(),
            poll_due: Instant::now(),
            status_glow: Tween::new(0.0),
            status: None,
            status_rest: None,
            status_hovered: false,
            bell: None,
            bell_glow: Tween::new(0.0),
            bell_hovered: false,
            bell_due: None,
            hush: Tween::new(0.0),
            intent_due: None,
            dropdown: Dropdown::control_center(),
            history: History::new(),
            clock_rest: None,
            clock_island: None,
            clock_hovered: false,
            playing_due: None,
            playing: Dropdown::now_playing(),
            music: Music::new(),
            water: Tween::new(0.0),
            water_due: None,
            epoch: Instant::now(),
            notices: Notices::default(),
            ringing: None,
            osd: Osd::new(),
            mic: Mic::default(),
            heard: false,
            light: None,
            keyboard_light: None,
            around: Around::default(),
            awake: Awake::Off,
            quiet: false,
            pointed: None,
            clock_width: None,
            share: None,
            calendar: Dropdown::calendar(),
            is_playing: false,
            notes: false,
            noted: None,
            ambient_due: None,
        };
        bar.notices = Notices::read(&bar.ctx.paths);
        bar.read_settings();
        // As it is, not on its way there.
        bar.hush = Tween::new(if bar.quiet { 1.0 } else { 0.0 });
        bar.refresh_clock();
        bar.poll();
        bar
    }

    /// Re-read what's chosen elsewhere that the island shows: keeping awake,
    /// and Do not disturb. Returns whether either changed.
    fn read_settings(&mut self) -> bool {
        let awake = Awake::read(&self.ctx.paths);
        let quiet = enso_core::chosen::Quiet::read(&self.ctx.paths).is_on(awake::unix_now());
        let changed = awake != self.awake || quiet != self.quiet;
        self.awake = awake;
        self.hush_to(quiet, Instant::now());
        changed
    }

    /// Do not disturb comes on or goes: the strike draws across the bell,
    /// or leaves it, and a bell woken again rings once.
    fn hush_to(&mut self, quiet: bool, now: Instant) {
        if quiet == self.quiet {
            return;
        }
        self.quiet = quiet;
        let hush = self.ctx.motion.apply(Transition::SELECT);
        self.hush.set(if quiet { 1.0 } else { 0.0 }, hush, now);
        if !quiet && self.ctx.motion.is_enabled() {
            self.ringing = Some(now);
        }
    }

    /// The sound server's news: the output's level for scrolling, and —
    /// after the first word, which is only how things are — the level the
    /// island shows when it changes.
    fn hear(&mut self, heard: Heard) {
        let now = Instant::now();
        let motion = self.ctx.motion;
        if self.heard {
            if let Some(level) = heard.output
                && self.sound != Some(level)
                && self.sound.is_some()
            {
                self.osd
                    .show(Gauge::Volume, level.volume, level.muted, now, &motion);
            } else if heard.mic.muted != self.mic.muted && heard.mic != Mic::default() {
                self.osd.show(
                    Gauge::Microphone,
                    heard.mic.volume,
                    heard.mic.muted,
                    now,
                    &motion,
                );
            }
        }
        self.heard = true;
        self.sound = heard.output;
        self.mic = heard.mic;
    }

    /// A light changed: show it, unless this is the first reading.
    fn brighten(&mut self, lamp: light::Lamp, percent: u8) {
        let (before, gauge) = match lamp {
            light::Lamp::Screen => (&mut self.light, Gauge::Brightness),
            light::Lamp::Keyboard => (&mut self.keyboard_light, Gauge::Keyboard),
        };
        let shown = before.is_some_and(|before| before != percent);
        *before = Some(percent);
        if shown {
            // A keyboard light turned all the way down is off.
            let off = gauge == Gauge::Keyboard && percent == 0;
            self.osd
                .show(gauge, percent, off, Instant::now(), &self.ctx.motion);
        }
    }

    /// Re-read the network and battery. Returns whether either changed.
    fn poll(&mut self) -> bool {
        let (battery, link) = (self.sysfs.battery(), net::link());
        let changed = battery != self.battery || link != self.link;
        self.battery = battery;
        self.link = link;
        self.poll_due = Instant::now() + POLL;
        changed
    }

    /// Re-read the clock, and note when the minute turns next.
    fn refresh_clock(&mut self) {
        let now = jiff::Zoned::now();
        self.clock = jiff::fmt::strtime::format(&self.config.clock, &now)
            .or_else(|_| jiff::fmt::strtime::format(DEFAULT_CLOCK, &now))
            .unwrap_or_default();
        let into_minute = Duration::new(
            now.second().unsigned_abs().into(),
            now.subsec_nanosecond().unsigned_abs(),
        );
        let next_minute =
            Duration::from_secs(60).saturating_sub(into_minute) + Duration::from_millis(20);
        self.clock_due = Instant::now() + next_minute;
    }

    /// Ask to be woken for whichever comes first: the minute turning, the
    /// next look at the network and battery, or the pointer having rested on
    /// the status island long enough.
    fn arm(&self, cx: &mut Cx<'_>) {
        let due = self.clock_due.min(self.poll_due);
        let due = self.intent_due.map_or(due, |intent| intent.min(due));
        let due = self.playing_due.map_or(due, |intent| intent.min(due));
        let due = self.bell_due.map_or(due, |intent| intent.min(due));
        let due = self.water_due.map_or(due, |tick| tick.min(due));
        let due = self.osd.due().map_or(due, |hide| hide.min(due));
        let due = self.ambient_due.map_or(due, |tick| tick.min(due));
        cx.wake_in(due.saturating_duration_since(Instant::now()));
    }

    /// Track the pointer against the status island: from its near end to
    /// halfway to the bell. Returns the new state when it changes.
    fn status_hover(&mut self, pointer: Option<(f32, f32)>) -> Option<bool> {
        let split = self.bell.map_or(f32::INFINITY, |bell| bell.x - APART / 2.0);
        let hovered = pointer.is_some_and(|(x, _)| {
            self.status
                .is_some_and(|island| x >= island.x && x < split)
        });
        (hovered != self.status_hovered).then(|| {
            self.status_hovered = hovered;
            self.status_glow.set(
                if hovered { 1.0 } else { 0.0 },
                self.ctx.motion.apply(Transition::HOVER),
                Instant::now(),
            );
            hovered
        })
    }

    /// Track the pointer against the bell: from halfway between it and the
    /// status island to the screen's corner. Returns whether that changed.
    fn bell_hover(&mut self, pointer: Option<(f32, f32)>) -> bool {
        let hovered = pointer
            .is_some_and(|(x, _)| self.bell.is_some_and(|bell| x >= bell.x - APART / 2.0));
        let changed = hovered != self.bell_hovered;
        if changed {
            self.bell_hovered = hovered;
            self.bell_glow.set(
                if hovered { 1.0 } else { 0.0 },
                self.ctx.motion.apply(Transition::HOVER),
                Instant::now(),
            );
        }
        changed
    }

    /// Follow the pointer for the control center's sake: keep an open panel
    /// informed, and start (or abandon) the wait before opening one. And
    /// light the bell under it.
    fn follow_status(&mut self, pointer: Option<(f32, f32)>, cx: &mut Cx<'_>) {
        if let Some(hovering) = self.status_hover(pointer) {
            self.dropdown.tell(hovering);
            cx.redraw();
        }
        if self.bell_hover(pointer) {
            if self.bell_hovered {
                // Back on the bell while the history is up: it stays at once.
                if self.notices.open {
                    self.history.hover();
                } else {
                    self.bell_due = Some(Instant::now() + HOVER_INTENT);
                }
            } else {
                self.bell_due = None;
                self.history.leave();
            }
            cx.redraw();
        }
        if !self.status_hovered {
            self.intent_due = None;
        } else if self.intent_due.is_none() && self.may_open_on_hover() && !self.dropdown.is_open()
        {
            self.intent_due = Some(Instant::now() + HOVER_INTENT);
        }
        self.arm(cx);
    }

    /// Follow the pointer for what's playing: tell an open card whether the
    /// pointer is on the clock, and start (or abandon) the wait before
    /// opening one.
    fn follow_clock(&mut self, pointer: Option<(f32, f32)>, cx: &mut Cx<'_>) {
        self.clock_hover(pointer);
        self.arm(cx);
    }

    /// Track the pointer against the clock island, and the wait before what's
    /// playing opens.
    fn clock_hover(&mut self, pointer: Option<(f32, f32)>) {
        let hovered = pointer.is_some_and(|(x, _)| {
            self.clock_island
                .is_some_and(|island| x >= island.x && x < island.right())
        });
        if hovered != self.clock_hovered {
            self.clock_hovered = hovered;
            self.playing.tell(hovered);
            self.calendar.tell(hovered);
        }
        let opens = self.config.now_playing || self.config.calendar;
        if !hovered {
            self.playing_due = None;
        } else if self.playing_due.is_none()
            && opens
            && !self.playing.is_open()
            && !self.calendar.is_open()
        {
            self.playing_due = Some(Instant::now() + HOVER_INTENT);
        }
    }

    /// Whether the water is moving: filling, draining, or rippling while it
    /// plays (and still, when motion is turned down).
    /// Whether the battery is filling.
    fn charging(&self) -> bool {
        self.battery
            .as_ref()
            .is_some_and(|battery| battery.status == BatteryStatus::Charging)
    }

    /// How far the light has run along the charging battery (`0..1`), while
    /// it runs.
    fn shine_at(&self, now: Instant) -> Option<f32> {
        if !self.charging() || !self.ctx.motion.is_enabled() {
            return None;
        }
        let t = now.saturating_duration_since(self.epoch).as_secs_f32();
        let into = t % SHINE_EVERY;
        (into < SHINE).then_some(into / SHINE)
    }

    /// When the small motions next move on: the steam over the cup while
    /// the machine is kept awake, the light along the battery while it
    /// charges. Nothing, when nothing moves.
    fn ambient_next(&self, now: Instant) -> Option<Instant> {
        if !self.ctx.motion.is_enabled() {
            return None;
        }
        if self.status_items().contains(&Item::Awake) {
            return Some(now + STEAM_TICK);
        }
        if self.charging() {
            let t = now.saturating_duration_since(self.epoch).as_secs_f32();
            let into = t % SHINE_EVERY;
            return Some(if into < SHINE {
                now + SHINE_TICK
            } else {
                now + Duration::from_secs_f32(SHINE_EVERY - into)
            });
        }
        None
    }

    fn water_moving(&self, now: Instant) -> bool {
        self.water.is_animating(now) || (self.water.target() > 0.0 && self.ctx.motion.is_enabled())
    }

    /// Whether resting on the status island opens the control center: not
    /// when that is switched off. Over the notification history too: the
    /// pointer has gone from the bell to the controls, and the one card
    /// gives way to the other.
    fn may_open_on_hover(&self) -> bool {
        self.config.control_center
    }

    /// A click on the status island opens the control center at once, not
    /// waiting for the pointer to rest. One that is up stays: the click
    /// meant it.
    fn click_status(&mut self) {
        self.intent_due = None;
        if !self.dropdown.is_open() {
            self.dropdown.open(self.config.position);
        }
    }

    /// A click on the bell hushes it — Do not disturb — or wakes it again.
    fn click_bell(&mut self) {
        let quiet = !self.quiet;
        match enso_core::chosen::Quiet::set(&self.ctx.paths, quiet, 0) {
            // Shown at once; the setting's own news, when it comes, agrees.
            Ok(()) => self.hush_to(quiet, Instant::now()),
            Err(err) => eprintln!("enso-bar: cannot change Do not disturb: {err}"),
        }
    }

    fn set_workspaces(&mut self, workspaces: Vec<Workspace>) {
        let now = Instant::now();
        let select = self.ctx.motion.apply(Transition::SELECT);
        // Something seen for the first time appears in its final state.
        let settle = |tweens: &mut HashMap<u64, Tween<f32>>, id: u64, on: bool| {
            let target = if on { 1.0 } else { 0.0 };
            tweens
                .entry(id)
                .or_insert_with(|| Tween::new(target))
                .set(target, select, now);
        };
        for ws in &workspaces {
            settle(&mut self.current, ws.id, ws.active);
            for window in &ws.windows {
                // Only the workspace being looked at shows its focused window
                // as a pill; elsewhere a dot is just a count.
                settle(&mut self.activity, window.id, ws.active && window.active);
            }
        }
        // Forget what has gone: one pass to collect what is here, one to
        // drop the rest — not a search of every workspace per entry.
        let here: HashSet<u64> = workspaces.iter().map(|ws| ws.id).collect();
        self.current.retain(|id, _| here.contains(id));
        let here: HashSet<u64> = workspaces
            .iter()
            .flat_map(|ws| ws.windows.iter().map(|window| window.id))
            .collect();
        self.activity.retain(|id, _| here.contains(id));
        self.workspaces = workspaces;
        // The island may have changed size under a pointer that is not moving.
        self.hover(self.pointer);
    }

    /// An island's fill: its colour, as see-through as the config asks.
    fn island_color(&self, color: Rgba) -> Rgba {
        color.alpha(self.opacity())
    }

    /// How solid the islands are.
    fn opacity(&self) -> f32 {
        if self.config.opacity.is_finite() {
            self.config.opacity
        } else {
            1.0
        }
    }

    /// Keep a note of where the islands come to rest, and which one the
    /// pointer lights, when that has changed: what grows out of them reads
    /// it (see [`enso_core::island`]). `screen` is the bar's width.
    fn note_islands(&mut self, screen: f32) {
        let Some(output) = self.output.clone().filter(|_| self.notes) else {
            return;
        };
        let Some(clock) = self.clock_rest else {
            return;
        };
        let spot = |rect: Rect| island::Spot {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        let lit = if self.bell_hovered {
            Some(island::Which::Bell)
        } else if self.status_hovered {
            Some(island::Which::Status)
        } else {
            None
        };
        let note = island::Island {
            pid: std::process::id(),
            top: self.config.position == Position::Top,
            bar: self.config.height as f32,
            x: clock.x,
            y: clock.y,
            w: clock.w,
            h: clock.h,
            opacity: self.opacity(),
            screen,
            status: self.status_rest.map(spot),
            bell: self.bell.map(spot),
            lit,
        };
        if self.noted.as_ref() == Some(&(output.clone(), note)) {
            return;
        }
        // Said once for each change, not on every frame.
        if let Err(err) = island::publish(&output, &note) {
            eprintln!("enso-bar: cannot say where the islands are: {err}");
        }
        self.noted = Some((output, note));
    }

    /// Workspaces on this bar's output (all of them until the output is known).
    fn visible_workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces
            .iter()
            .filter(|ws| self.output.is_none() || ws.output.is_none() || ws.output == self.output)
    }

    /// Lay out the workspace island inside `strip`, opened out by `expand`
    /// (`0..=1`), reading each animated value through `sample` — the value
    /// right now for drawing, its target for where things will come to rest.
    ///
    /// Every workspace is a group of dots. A group is as open as it is
    /// current or the island is expanded, and everything about it — width,
    /// dot size, the gaps to its neighbours — scales with that, so groups
    /// bloom in and fold away without anything jumping.
    fn island(
        &self,
        strip: Rect,
        expand: f32,
        sample: impl Fn(&Tween<f32>) -> f32,
    ) -> Option<Island> {
        let colors = &self.ctx.theme.colors;
        let accent = self.ctx.accent();
        let value =
            |tweens: &HashMap<u64, Tween<f32>>, id: u64| tweens.get(&id).map_or(0.0, &sample);

        let mut groups: Vec<Group> = Vec::new();
        let mut x = strip.x + ISLAND_PAD;
        for ws in self.visible_workspaces() {
            let current = value(&self.current, ws.id);
            let open = 1.0 - (1.0 - expand) * (1.0 - current);
            if let Some(previous) = groups.last() {
                x += GROUP_GAP * previous.open.min(open);
            }

            // Natural widths first: an empty workspace still gets one quiet
            // dot, so it can be seen and clicked.
            let empty = ws
                .windows
                .is_empty()
                .then_some((DOT, colors.highlight_high, None));
            let windows = ws.windows.iter().map(|window| {
                let activity = value(&self.activity, window.id);
                let idle = if window.urgent {
                    colors.love
                } else {
                    colors.subtle
                };
                (
                    DOT + (ACTIVE_WIDTH - DOT) * activity,
                    idle.mix(accent, activity),
                    Some(window.id),
                )
            });

            let size = DOT * open;
            let mut offset = GROUP_PAD;
            let dots: Vec<Dot> = empty
                .into_iter()
                .chain(windows)
                .map(|(width, color, window)| {
                    let rect = Rect::new(
                        x + offset * open,
                        strip.y + (strip.h - size) / 2.0,
                        width * open,
                        size,
                    );
                    offset += width + DOT_GAP;
                    Dot {
                        rect,
                        color,
                        window,
                    }
                })
                .collect();

            let width = (offset - DOT_GAP + GROUP_PAD) * open;
            groups.push(Group {
                workspace: ws.id,
                rect: Rect::new(x, strip.y, width, strip.h),
                open,
                current,
                dots,
            });
            x += width;
        }

        (!groups.is_empty()).then(|| Island {
            rect: Rect::new(strip.x, strip.y, x + ISLAND_PAD - strip.x, strip.h),
            groups,
        })
    }

    /// Note which window's dot is under the pointer, for the clock to name.
    /// Returns whether that changed.
    fn point(&mut self, pointer: Option<(f32, f32)>) -> bool {
        let pointed = pointer.and_then(|(x, y)| {
            self.hits.iter().find_map(|(rect, target)| match target {
                Target::Window(id) if rect.contains(x, y) => Some(*id),
                _ => None,
            })
        });
        let changed = pointed != self.pointed;
        self.pointed = pointed;
        changed
    }

    /// Track the pointer (`None` once it leaves the bar) and open or close
    /// the island to match. Returns whether that changed, i.e. a redraw is due.
    fn hover(&mut self, pointer: Option<(f32, f32)>) -> bool {
        self.pointer = pointer;
        // Measured against where the island will rest in its present state,
        // so it does not flicker while it grows or shrinks under the pointer.
        // Everything from the screen corner to the island's far end counts.
        let expand = if self.hovered { 1.0 } else { 0.0 };
        let hovered = pointer.is_some_and(|(x, _)| {
            self.island(self.strip, expand, Tween::target)
                .is_some_and(|island| x < island.rect.right())
        });
        if hovered == self.hovered {
            return false;
        }
        self.hovered = hovered;
        self.expand.set(
            if hovered { 1.0 } else { 0.0 },
            self.ctx.motion.apply(Transition::REFLOW),
            Instant::now(),
        );
        true
    }

    fn draw_workspaces(&mut self, canvas: &mut Canvas<'_>, strip: Rect, now: Instant) -> bool {
        self.strip = strip;
        self.hits.clear();
        let expand = self.expand.get(now);
        let Some(island) = self.island(strip, expand, |tween| tween.get(now)) else {
            return false;
        };
        let colors = &self.ctx.theme.colors;
        canvas.pill(island.rect, self.island_color(colors.base));

        for group in &island.groups {
            // A soft capsule marks the current workspace once there are
            // others to tell it apart from.
            let highlight = expand * group.current;
            if highlight > 0.0 {
                canvas.pill(
                    group.rect.inset_xy(0.0, space::XS),
                    colors.overlay.alpha(highlight),
                );
            }
            for dot in &group.dots {
                canvas.pill(dot.rect, dot.color.alpha(group.open));
            }

            // Clickable once it is open enough to aim at. Targets are
            // generous: the full bar height, and half the gap either side of
            // a dot. Dots come first so they win over their group.
            if group.open > 0.5 {
                let tall = |rect: Rect| rect.inset_xy(0.0, -INSET);
                for dot in &group.dots {
                    if let Some(id) = dot.window {
                        let rect = Rect::new(
                            dot.rect.x - DOT_GAP / 2.0,
                            group.rect.y,
                            dot.rect.w + DOT_GAP,
                            group.rect.h,
                        );
                        self.hits.push((tall(rect), Target::Window(id)));
                    }
                }
                self.hits
                    .push((tall(group.rect), Target::Workspace(group.workspace)));
            }
        }

        self.expand.is_animating(now)
            || self
                .current
                .values()
                .chain(self.activity.values())
                .any(|tween| tween.is_animating(now))
    }

    /// What the clock island says: the time — or, while the pointer is on
    /// a window's dot, what that window is.
    fn clock_says(&self) -> &str {
        self.pointed
            .and_then(|id| self.around.titles.get(&id))
            .map_or(&self.clock, String::as_str)
    }

    /// Returns whether the island is still changing width.
    fn draw_clock(&mut self, canvas: &mut Canvas<'_>, bar: Rect, size: f32, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let says = self.clock_says().to_owned();
        let longest = (bar.w / 3.0).min(MAX_TITLE);
        // The date quieter than the time, where the clock says both — a
        // double space between them, as the default has. A window's name
        // is one thing, and said as one.
        let parts = (says == self.clock)
            .then(|| says.rsplit_once("  "))
            .flatten()
            .map(|(date, time)| (date.trim_end().to_owned(), time.trim_start().to_owned()));
        let words = match &parts {
            Some((date, time)) => {
                let date = Text::new(date, size, colors.subtle).tabular();
                let time = Text::new(time, size, colors.text).medium().tabular();
                let (d, t) = (canvas.measure(&date).w, canvas.measure(&time).w);
                d + space::SM + t
            }
            None => {
                canvas
                    .measure(
                        &Text::new(&says, size, colors.text)
                            .medium()
                            .tabular()
                            .max_width(longest),
                    )
                    .w
            }
        };
        let natural = words + 2.0 * space::LG;
        let reflow = self.ctx.motion.apply(Transition::REFLOW);
        let tween = self.clock_width.get_or_insert_with(|| Tween::new(natural));
        if tween.target() != natural {
            tween.set(natural, reflow, now);
        }
        let width = tween.get(now);
        let moving = tween.is_animating(now);
        self.clock_rest = Some(bar.centered(natural, bar.h));
        // Laid out at the width it is heading for, and clipped to the
        // island as it grows: the words never squeeze or spill.
        let text = Text::new(&says, size, colors.text)
            .medium()
            .tabular()
            .max_width((natural - 2.0 * space::LG).max(1.0));
        let island = bar.centered(width, bar.h);
        self.clock_island = Some(island);
        canvas.pill(island, self.island_color(colors.base));
        let fill = self.water.get(now);
        let t = now.saturating_duration_since(self.epoch).as_secs_f32();
        let sea = water::draw(
            canvas,
            island,
            fill,
            t,
            water::Look {
                // Dusk: iris deep water, rising to a rose wave.
                deep: self.island_color(colors.base.mix(colors.iris, 0.35)),
                shallow: self.island_color(colors.rose.mix(colors.base, 0.1)),
                foam: colors.rose.mix(colors.text, 0.45),
            },
        );
        // Where the water is too light for light words, they are cut dark
        // out of it, as the battery's figures are out of its fill; on the
        // deep water they stay light. The date can't be the quieter colour
        // on the sea — there's no telling it from the water — so it comes
        // up to the time's as the tide does; the time keeps its weight.
        let dark = colors.base;
        let mut shallows = Shape::new();
        for (water, color) in &sea {
            if color.legible(colors.text, dark) == dark {
                shallows.add(water);
            }
        }
        let date_color = colors.subtle.mix(colors.text, fill.clamp(0.0, 1.0));
        let say = |canvas: &mut Canvas<'_>, quiet: Rgba, ink: Rgba| match &parts {
            Some((date, time)) => {
                let date = Text::new(date, size, quiet).tabular();
                let time = Text::new(time, size, ink).medium().tabular();
                let left = island.x + (island.w - words) / 2.0;
                let d = canvas.measure(&date).w;
                canvas.text_in(&date, Rect::new(left, island.y, d, island.h), Align::Start);
                let at = left + d + space::SM;
                canvas.text_in(&time, Rect::new(at, island.y, words - d, island.h), Align::Start);
            }
            None => {
                canvas.text_in(&Text { color: ink, ..text }, island, Align::Center);
            }
        };
        let inside = island.inset_xy(space::SM, 0.0);
        canvas.with_clip(inside, |canvas| {
            canvas.with_clip_shape(&shallows, false, |canvas| say(canvas, date_color, colors.text));
            canvas.with_clip_shape(&shallows, true, |canvas| say(canvas, dark, dark));
        });
        moving
    }

    /// The battery, if there is one: on power it fills in foam, and as it
    /// runs low in gold and then love.
    fn charge(&self) -> Option<Charge> {
        let colors = &self.ctx.theme.colors;
        let battery = self.battery.as_ref()?;
        let plugged = battery.is_plugged_in();
        let fill = match (plugged, battery.percent) {
            (true, _) => colors.foam,
            (false, ..=15) => colors.love,
            (false, ..=30) => colors.gold,
            // Bright, as the figures cut out of it must read at a glance.
            (false, _) => colors.text,
        };
        Some(Charge {
            percent: battery.percent.min(100),
            plugged,
            fill,
        })
    }

    /// What the status island shows, left to right: what's out of the
    /// ordinary, while it lasts; the network where there is such, and the
    /// battery where there is one. The volume shows only as it changes (see
    /// [`Osd`]), and scrolling on the island turns it. The bell has an
    /// island of its own.
    fn status_items(&self) -> Vec<Item> {
        let mut items = Vec::with_capacity(8);
        // What's out of the ordinary comes first, and only while it is.
        if !self.around.casts.is_empty() {
            items.push(Item::Share);
        }
        if self.mic.listened {
            items.push(Item::Mic(self.mic.muted));
        }
        if self.sound.is_some_and(|level| level.muted) {
            items.push(Item::Muted);
        }
        if self.awake.is_on(awake::unix_now()) {
            items.push(Item::Awake);
        }
        if self.around.layout.is_some() {
            items.push(Item::Layout);
        }
        items.extend(self.link.map(Item::Network));
        items.extend(self.charge().map(Item::Battery));
        items
    }

    /// An item's colour: quiet, and the bell bright when something went by
    /// unattended. Offline is told by the drawing — every arc faint, a
    /// slash across — not by a darker colour, which would leave too little
    /// to see.
    fn tint(&self, item: Item) -> Rgba {
        let colors = &self.ctx.theme.colors;
        match item {
            Item::Bell if self.notices.unseen > 0 => colors.text,
            Item::Battery(charge) => charge.fill,
            // Being heard or seen is worth noticing.
            Item::Share | Item::Mic(false) => colors.love,
            Item::Layout => colors.text,
            Item::Network(_) | Item::Bell | Item::Mic(true) | Item::Muted | Item::Awake => {
                colors.subtle
            }
        }
    }

    /// Where the bell is in its ring, and whether it is still ringing.
    fn swing(&self, now: Instant) -> (f32, bool) {
        let Some(started) = self.ringing else {
            return (0.0, false);
        };
        let length = self.ctx.motion.apply(Transition::new(
            RING.as_millis() as u64,
            enso_core::Easing::Linear,
        ));
        let elapsed = now.saturating_duration_since(started);
        if length.duration.is_zero() || elapsed >= length.duration {
            return (0.0, false);
        }
        let t = elapsed.as_secs_f32() / length.duration.as_secs_f32();
        (icons::ring(t), true)
    }

    /// The figure an item is drawn with, if it is one.
    fn figure(&self, item: Item) -> Option<Figure> {
        match item {
            Item::Share => Some(Figure::Screencast),
            Item::Mic(muted) => Some(if muted {
                Figure::MicrophoneOff
            } else {
                Figure::Microphone
            }),
            Item::Muted => Some(Figure::SpeakerMuted),
            Item::Awake => Some(Figure::Cup),
            Item::Network(link) => Some(network(link)),
            Item::Bell => Some(if self.quiet {
                Figure::BellOff
            } else {
                Figure::Bell
            }),
            Item::Battery(_) | Item::Layout => None,
        }
    }

    /// The bell's island, at the very end of the bar: a round one when the
    /// bell fits, which it does. Returns where the next island ends, and
    /// whether the bell is still moving.
    fn draw_bell(&mut self, canvas: &mut Canvas<'_>, bar: Rect, now: Instant) -> (f32, bool) {
        let scale = icons::Scale::new(bar.h);
        let colors = &self.ctx.theme.colors;
        let hush = self.hush.get(now);
        let figure = if hush > 0.0 {
            Figure::BellOff
        } else {
            Figure::Bell
        };
        let figure_w = scale.width(figure);
        let width = (figure_w + 2.0 * scale.pad()).min(bar.h).max(figure_w + scale.gap());
        let island = Rect::new(bar.right() - width, bar.y, width, bar.h);
        self.bell = Some(island);

        let glow = self.bell_glow.get(now);
        let ground = colors.base.mix(colors.overlay, glow);
        canvas.pill(island, self.island_color(ground));
        let (swing, ringing) = self.swing(now);
        let ink = icons::Ink {
            on: self.tint(Item::Bell).mix(colors.text, glow * 0.5),
            ground,
        };
        let icon_h = scale.icon();
        let icon = Rect::new(
            island.x + ((width - figure_w) / 2.0).round(),
            island.y + ((island.h - icon_h) / 2.0).round(),
            figure_w,
            icon_h,
        );
        if figure == Figure::Bell {
            let dot = (self.notices.unseen > 0).then_some(self.ctx.accent());
            icons::bell(canvas, icon, swing, dot, ink);
        } else {
            let live = enso_ui::figure::Live {
                time: None,
                amount: Some(hush),
            };
            icons::draw_live(canvas, figure, icon, ink, live);
        }
        let moving = ringing || self.bell_glow.is_animating(now) || self.hush.is_animating(now);
        (island.x - APART, moving)
    }

    /// The status island: what's out of the ordinary, Wi-Fi (or a cable),
    /// and the battery, every one drawn at the bar's scale, ending where
    /// the bell's begins — or, for a moment after a level changes, that
    /// level. Returns whether it is still animating.
    fn draw_status(&mut self, canvas: &mut Canvas<'_>, bar: Rect, now: Instant) -> bool {
        self.status = None;
        self.status_rest = None;
        self.share = None;
        let (right, bell_moving) = self.draw_bell(canvas, bar, now);
        let scale = icons::Scale::new(bar.h);
        let battery = icons::Battery::new(scale);
        let items = self.status_items();
        let colors = self.ctx.theme.colors.clone();

        let figures = items.iter().find_map(|item| match item {
            Item::Battery(charge) => Some(charge.percent.to_string()),
            _ => None,
        });
        let label = figures
            .as_deref()
            .map(|figures| Text::new(figures, battery.figures(), colors.text).medium().tabular());
        let layout = self
            .around
            .layout
            .as_deref()
            .map(|name| Text::new(name, battery.figures(), self.tint(Item::Layout)).medium());
        let widths: Vec<f32> = items
            .iter()
            .map(|item| match (item, self.figure(*item)) {
                (_, Some(figure)) => scale.width(figure),
                (Item::Battery(charge), None) => label.as_ref().map_or(0.0, |label| {
                    battery.width(canvas.measure(label).w, charge.plugged)
                }),
                (_, None) => layout
                    .as_ref()
                    .map_or(0.0, |layout| canvas.measure(layout).w.ceil()),
            })
            .collect();
        let resting = if items.is_empty() {
            0.0
        } else {
            widths.iter().sum::<f32>() + (items.len() - 1) as f32 * scale.gap() + 2.0 * scale.pad()
        };

        // A level just changed: the island widens to the left to show it.
        let size = self.ctx.config.appearance.font_size * type_scale::SMALL;
        let shown = self.osd.presence(now);
        let level_width = self.osd.width(canvas, scale, size).max(resting);
        let width = resting + (level_width - resting) * shown;
        // Nothing to say and no level to show: no island, only the bell.
        if width < 1.0 {
            return bell_moving || self.status_glow.is_animating(now);
        }
        let island = Rect::new(right - width, bar.y, width, bar.h);
        self.status = Some(island);
        if resting > 0.0 {
            self.status_rest = Some(Rect::new(right - resting, bar.y, resting, bar.h));
        }

        // It warms a shade under the pointer: there is something behind it.
        let glow = self.status_glow.get(now);
        let ground = colors.base.mix(colors.overlay, glow);
        canvas.pill(island, self.island_color(ground));
        let t = now.saturating_duration_since(self.epoch).as_secs_f32();

        if shown > 0.0 {
            let osd = &self.osd;
            canvas.with_opacity(shown, |canvas| {
                let look = crate::osd::Look {
                    scale,
                    size,
                    colors: &colors,
                };
                osd.draw(canvas, island, look, now);
            });
        }

        let icon_h = scale.icon();
        let mut x = island.right() - resting + scale.pad();
        let mut share = None;
        canvas.with_opacity(1.0 - shown, |canvas| {
            for (item, width) in items.into_iter().zip(widths) {
                let tint = self.tint(item);
                let ink = icons::Ink {
                    on: tint.mix(colors.text, glow * 0.5),
                    ground,
                };
                let slot = Rect::new(x, island.y, width, island.h);
                let icon = Rect::new(
                    x,
                    island.y + ((island.h - icon_h) / 2.0).round(),
                    width,
                    icon_h,
                );
                match (item, self.figure(item)) {
                    (Item::Share, Some(figure)) => {
                        share = Some(slot.inset_xy(-scale.gap() / 2.0, 0.0));
                        icons::draw(canvas, figure, icon, ink);
                    }
                    // Kept awake: the steam over the cup drifts.
                    (Item::Awake, Some(figure)) if self.ctx.motion.is_enabled() => {
                        icons::draw_at(canvas, figure, icon, ink, t)
                    }
                    (_, Some(figure)) => icons::draw(canvas, figure, icon, ink),
                    (Item::Battery(charge), None) => {
                        if let Some(label) = &label {
                            let shine = self.shine_at(now);
                            battery.draw(canvas, slot, charge, label, glow, shine, &colors);
                        }
                    }
                    (_, None) => {
                        if let Some(layout) = &layout {
                            canvas.text_in(layout, slot, Align::Center);
                        }
                    }
                }
                x += width + scale.gap();
            }
        });
        // Only once the island has its icons back is the mark a button.
        if shown < 0.5 {
            self.share = share;
        }
        self.status_glow.is_animating(now) || bell_moving || self.osd.is_animating(now)
    }
}

/// The figure for how the machine is online.
fn network(link: Link) -> Figure {
    match link {
        Link::Wired => Figure::Ethernet,
        Link::Offline => Figure::WifiOff,
        Link::Wifi { bars } => Figure::Wifi(bars),
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        if let Some((output, _)) = &self.noted {
            island::withdraw(output);
        }
    }
}

impl App for Bar {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        let edge = match self.config.position {
            Position::Top => Anchor::TOP,
            Position::Bottom => Anchor::BOTTOM,
        };
        SurfaceSpec {
            height: self.config.height,
            exclusive_zone: self.config.height as i32,
            output: self.output.clone(),
            ..SurfaceSpec::new("enso-bar", Layer::Top, edge | Anchor::LEFT | Anchor::RIGHT)
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let workspaces = proxy.clone();
        niri::watch(move |seen| match seen {
            Seen::Workspaces(snapshot) => workspaces.send(Message::Workspaces(snapshot)),
            Seen::Around(around) => workspaces.send(Message::Around(around)),
        });
        let volume = proxy.clone();
        sound::watch(move |heard| volume.send(Message::Heard(heard)));
        let light = proxy.clone();
        light::watch(move |lamp, percent| light.send(Message::Light(lamp, percent)));
        if self.config.now_playing {
            let music = proxy.clone();
            self.music
                .follow(move |playing| music.send(Message::Playing(playing)));
        }

        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-bar: live theme switching unavailable: {err}");
        }
        self.refresh_clock();
        self.ambient_due = self.ambient_next(Instant::now());
        self.arm(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Workspaces(workspaces)) => {
                self.set_workspaces(workspaces);
                cx.redraw();
            }
            Event::Message(Message::Around(around)) => {
                // Titles change all the time; only what shows is redrawn for.
                let shown = |around: &Around, pointed: Option<u64>| {
                    (
                        around.casts.is_empty(),
                        around.layout.clone(),
                        pointed.and_then(|id| around.titles.get(&id).cloned()),
                    )
                };
                let redraw = shown(&self.around, self.pointed) != shown(&around, self.pointed);
                self.around = around;
                if redraw {
                    cx.redraw();
                }
            }
            Event::Message(Message::Heard(heard)) => {
                self.hear(heard);
                self.arm(cx);
                cx.redraw();
            }
            Event::Message(Message::Light(lamp, percent)) => {
                self.brighten(lamp, percent);
                self.arm(cx);
                cx.redraw();
            }
            Event::Message(Message::Playing(playing)) => {
                self.is_playing = playing;
                let now = Instant::now();
                let target = if playing { 1.0 } else { 0.0 };
                if self.water.target() != target {
                    self.water.set(target, self.ctx.motion.apply(POUR), now);
                    self.water_due = Some(now);
                    self.arm(cx);
                    cx.redraw();
                }
            }
            // Anything written to the state directory: the theme choice, or
            // the notification daemon's count.
            Event::Message(Message::ThemeChanged) => {
                let notices = Notices::read(&self.ctx.paths);
                let changed = notices != self.notices;
                // Something new came in unattended: the bell rings.
                if notices.unseen > self.notices.unseen && self.ctx.motion.is_enabled() {
                    self.ringing = Some(Instant::now());
                }
                self.notices = notices;
                let chosen = self.read_settings();
                if self.ctx.reload_theme() || changed || chosen {
                    cx.redraw();
                }
            }
            Event::Wake => {
                let now = Instant::now();
                if now >= self.clock_due {
                    self.refresh_clock();
                    cx.redraw();
                }
                if self.osd.tick(now, &self.ctx.motion) {
                    cx.redraw();
                }
                if now >= self.poll_due {
                    // Kept awake, or quiet, for a while: the cup goes, and
                    // the bell rings again, once that's up.
                    let awake = self.awake.is_on(awake::unix_now());
                    let read = self.read_settings();
                    if self.poll() || read || awake != self.status_items().contains(&Item::Awake) {
                        cx.redraw();
                    }
                    // A card that has closed is cleaned up after here,
                    // rather than left until the next hover.
                    self.dropdown.is_open();
                    self.playing.is_open();
                    self.calendar.is_open();
                }
                if self.water_due.is_some_and(|due| now >= due) {
                    cx.redraw();
                    self.water_due = self.water_moving(now).then(|| now + water::TICK);
                }
                if self.ambient_due.is_some_and(|due| now >= due) {
                    cx.redraw();
                }
                self.ambient_due = self.ambient_next(now);
                if self.playing_due.is_some_and(|due| now >= due) {
                    self.playing_due = None;
                    // What's playing while something plays; otherwise the
                    // month.
                    if self.clock_hovered {
                        if self.is_playing && self.config.now_playing {
                            self.playing.open(self.config.position);
                        } else if self.config.calendar {
                            self.calendar.open(self.config.position);
                        }
                    }
                }
                if self.bell_due.is_some_and(|due| now >= due) {
                    self.bell_due = None;
                    if self.bell_hovered {
                        self.history.hover();
                    }
                }
                if self.intent_due.is_some_and(|due| now >= due) {
                    self.intent_due = None;
                    if self.status_hovered && self.may_open_on_hover() {
                        self.dropdown.open(self.config.position);
                    }
                }
                self.arm(cx);
            }
            Event::Output(name) => {
                self.output = Some(name);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Enter { x, y } | PointerEvent::Move { x, y }) => {
                if self.hover(Some((x, y))) {
                    cx.redraw();
                }
                if self.point(Some((x, y))) {
                    cx.redraw();
                }
                self.follow_status(Some((x, y)), cx);
                self.follow_clock(Some((x, y)), cx);
            }
            // Scrolling on the status island turns the volume.
            Event::Pointer(PointerEvent::Scroll { dy, .. }) if self.status_hovered => {
                if let Some(level) = self.sound
                    && let Some(volume) = self.scroll.level(dy, level.volume)
                {
                    // Counted from at once; the sound server confirms it shortly.
                    self.sound = Some(Level { volume, ..level });
                    sound::set(volume);
                }
            }
            Event::Pointer(PointerEvent::Leave) => {
                self.scroll.reset();
                if self.hover(None) | self.point(None) {
                    cx.redraw();
                }
                self.follow_status(None, cx);
                self.follow_clock(None, cx);
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(share) = self.share
                    && share.contains(x, y)
                {
                    // The sharing mark: stop sharing, and nothing else.
                    if let Err(err) = niri::stop_casts(&self.around.casts) {
                        eprintln!("enso-bar: cannot stop sharing the screen: {err}");
                    }
                    return;
                }
                if self.bell_hovered {
                    self.click_bell();
                } else if self.status_hovered {
                    self.click_status();
                }
                // A click on the clock plays or pauses.
                if self.clock_hovered && self.config.now_playing {
                    self.music.play_pause();
                }
                let target = self.hits.iter().find(|(rect, _)| rect.contains(x, y));
                let focused = match target {
                    Some((_, Target::Window(id))) => niri::focus_window(*id),
                    Some((_, Target::Workspace(id))) => niri::focus_workspace(*id),
                    None => Ok(()),
                };
                if let Err(err) = focused {
                    eprintln!("enso-bar: cannot switch focus: {err}");
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let strip = canvas.bounds().inset_xy(EDGE, INSET);
        let size = self.ctx.config.appearance.font_size * type_scale::SMALL;

        let animating = self.draw_workspaces(canvas, strip, now);
        let reflowing = self.draw_clock(canvas, strip, size, now);
        let glowing = self.draw_status(canvas, strip, now);
        self.note_islands(canvas.bounds().w);
        Paint::when(animating || reflowing || glowing)
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_sys::power::BatteryStatus;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;
    use crate::niri::Window;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<BarConfig>("bar").unwrap(),
            BarConfig::default()
        );
    }

    /// A workspace holding `windows`, the one with id `focus` focused.
    fn workspace(id: u64, active: bool, windows: &[u64], focus: u64) -> Workspace {
        Workspace {
            id,
            index: id as u8,
            output: Some("eDP-1".into()),
            active,
            windows: windows
                .iter()
                .map(|&id| Window {
                    id,
                    active: id == focus,
                    urgent: false,
                })
                .collect(),
        }
    }

    /// Four workspaces; the second is current, with three windows.
    fn desktop(current: u64, focus: u64) -> Vec<Workspace> {
        vec![
            workspace(
                1,
                current == 1,
                &[11, 12],
                if current == 1 { focus } else { 12 },
            ),
            workspace(
                2,
                current == 2,
                &[21, 22, 23],
                if current == 2 { focus } else { 22 },
            ),
            workspace(3, current == 3, &[31], 31),
            workspace(4, current == 4, &[], 0),
        ]
    }

    fn sample(theme: &str) -> Bar {
        // Each call gets its own directory so parallel tests never collide;
        // the next run clears and reuses them, so they don't pile up.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("enso-bar-test-{unique}"));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::rooted(root);
        enso_core::theme::set_active(&paths, theme).expect("write theme state");
        let mut bar = Bar::new(Context::load_from(paths), BarConfig::default());
        bar.clock = "Sun 15 Nov  22:46".into();
        bar.battery = Some(Battery {
            name: "BAT1".into(),
            percent: 82,
            status: BatteryStatus::Discharging,
            power_mw: None,
            minutes_left: None,
            limit: None,
        });
        bar.link = Some(Link::Wifi { bars: 3 });
        bar.sound = Some(Level {
            volume: 60,
            muted: false,
        });
        bar.set_workspaces(desktop(2, 22));
        bar
    }

    /// Draw one frame at `now` and report whether it is still animating.
    fn frame(bar: &mut Bar, now: Instant) -> (Offscreen, Paint) {
        let mut text = TextEngine::new(&bar.ctx.config.appearance);
        let mut paint = Paint::Idle;
        let frame = Offscreen::render(&mut text, (900, 36), 2, |canvas| {
            paint = bar.draw(canvas, now);
        });
        (frame, paint)
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    fn windows(bar: &Bar) -> Vec<u64> {
        bar.hits
            .iter()
            .filter_map(|(_, target)| match target {
                Target::Window(id) => Some(*id),
                Target::Workspace(_) => None,
            })
            .collect()
    }

    fn workspaces(bar: &Bar) -> Vec<u64> {
        bar.hits
            .iter()
            .filter_map(|(_, target)| match target {
                Target::Workspace(id) => Some(*id),
                Target::Window(_) => None,
            })
            .collect()
    }

    /// Where the island comes to rest, collapsed or expanded.
    fn resting(bar: &Bar, expanded: bool) -> Island {
        bar.island(bar.strip, if expanded { 1.0 } else { 0.0 }, Tween::target)
            .expect("an island")
    }

    /// `color` as an island shows it: premultiplied, as see-through as
    /// the bar's `opacity`.
    fn on_island(bar: &Bar, color: Rgba) -> [u8; 4] {
        let a = u32::from(bar.island_color(color).a);
        let mul = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
        [mul(color.r), mul(color.g), mul(color.b), a as u8]
    }

    /// The islands let a little of the wallpaper through, and only they:
    /// what sits on them — icons, the battery — stays solid.
    #[test]
    fn islands_are_a_little_see_through() {
        let mut bar = sample("rose-pine-moon");
        let colors = bar.ctx.theme.colors.clone();
        assert_eq!(BarConfig::default().opacity, 0.9);
        let (drawn, _) = frame(&mut bar, later());
        let island = bar.status.unwrap();
        let ground = drawn.pixel(((island.x + 4.0) * 2.0) as u32, 36);
        assert_eq!(ground, on_island(&bar, colors.base));
        assert_eq!(ground[3], 230);
        // The battery's fill is as solid as ever.
        let solid =
            |pixel: [u8; 4]| pixel == [colors.subtle.r, colors.subtle.g, colors.subtle.b, 255];
        let row = (0..drawn.width).map(|x| drawn.pixel(x, 36));
        assert!(row.filter(|pixel| solid(*pixel)).count() > 10);

        // Solid on request, and a nonsense value is solid too.
        bar.config.opacity = 1.0;
        let (solid_frame, _) = frame(&mut bar, later());
        assert_eq!(
            solid_frame.pixel(((island.x + 4.0) * 2.0) as u32, 36)[3],
            255
        );
        bar.config.opacity = f32::NAN;
        assert_eq!(bar.island_color(colors.base).a, 255);
    }

    /// The pictures held to a reference in `snapshots/`; the rest are
    /// only written, to look at.
    const CHECKED: &[&str] = &["bar-rose-pine-moon"];

    fn snapshot(frame: &Offscreen, name: &str) {
        if CHECKED.contains(&name) {
            frame.check(env!("CARGO_MANIFEST_DIR"), name);
        } else {
            frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
        }
    }

    #[test]
    fn renders_islands_for_each_builtin_theme() {
        for theme in ["rose-pine-moon", "rose-pine-main", "rose-pine-dawn"] {
            let mut bar = sample(theme);
            let (frame, paint) = frame(&mut bar, Instant::now());
            assert_eq!(paint, Paint::Idle);

            let base = bar.ctx.theme.colors.base;
            // The bar is transparent between islands, and nearly solid
            // inside them.
            assert_eq!(
                frame.pixel(400, 36)[3],
                0,
                "gap between islands should be transparent"
            );
            assert_eq!(frame.pixel(900, 20), on_island(&bar, base), "clock island");
            snapshot(&frame, &format!("bar-{theme}"));
        }
    }

    #[test]
    fn at_rest_the_island_shows_the_current_workspace_windows() {
        let mut bar = sample("rose-pine-moon");
        frame(&mut bar, Instant::now());

        // One target per window of workspace 2, left to right, and the
        // workspace itself behind them.
        assert_eq!(windows(&bar), [21, 22, 23]);
        assert_eq!(workspaces(&bar), [2]);
        assert!(
            bar.hits
                .windows(2)
                .take(2)
                .all(|pair| pair[0].0.x < pair[1].0.x)
        );
        // The focused window's pill is wider than the idle dots.
        assert!(bar.hits[1].0.w > bar.hits[0].0.w);

        // Same proportions as ever: dots a gap apart, `space::MD` at each end.
        let island = resting(&bar, false);
        let content = DOT + ACTIVE_WIDTH + DOT + 2.0 * DOT_GAP;
        assert_eq!(island.rect.w, content + 2.0 * space::MD);
        let first = island.groups[1].dots[0].rect;
        assert_eq!(first.x, island.rect.x + space::MD);
        assert_eq!((first.w, first.h), (DOT, DOT));
        assert_eq!(island.groups[1].dots[1].rect.w, ACTIVE_WIDTH);
    }

    #[test]
    fn an_empty_workspace_still_has_a_dot() {
        let mut bar = sample("rose-pine-moon");
        bar.set_workspaces(desktop(4, 0));
        let (frame, _) = frame(&mut bar, later());
        assert!(windows(&bar).is_empty());
        assert_eq!(workspaces(&bar), [4]);

        let dot = resting(&bar, false).groups[3].dots[0];
        assert_eq!(dot.window, None);
        let quiet = bar.ctx.theme.colors.highlight_high;
        let (x, y) = dot.rect.center();
        assert_eq!(
            frame.pixel((x * 2.0) as u32, (y * 2.0) as u32),
            [quiet.r, quiet.g, quiet.b, 255]
        );
    }

    #[test]
    fn hovering_opens_out_every_workspace() {
        for theme in ["rose-pine-moon", "rose-pine-main", "rose-pine-dawn"] {
            let mut bar = sample(theme);
            frame(&mut bar, Instant::now());
            let collapsed = resting(&bar, false).rect;

            assert!(bar.hover(Some((collapsed.x + 10.0, 18.0))));
            // Still over it: nothing new to do.
            assert!(!bar.hover(Some((collapsed.x + 12.0, 18.0))));
            assert_eq!(frame(&mut bar, Instant::now()).1, Paint::Animating);
            let (frame, paint) = frame(&mut bar, later());
            assert_eq!(paint, Paint::Idle);

            assert_eq!(windows(&bar), [11, 12, 21, 22, 23, 31]);
            assert_eq!(workspaces(&bar), [1, 2, 3, 4]);
            let island = resting(&bar, true);
            assert!(island.rect.w > collapsed.w);

            // A soft capsule sits behind the current workspace and no other.
            let colors = &bar.ctx.theme.colors;
            let behind = |group: &Group| {
                let (_, y) = group.rect.center();
                frame.pixel(((group.rect.x + 3.0) * 2.0) as u32, (y * 2.0) as u32)
            };
            let rgb = |color: Rgba| [color.r, color.g, color.b, 255];
            // The current workspace's capsule is solid on the island.
            assert_eq!(behind(&island.groups[1]), rgb(colors.overlay));
            assert_eq!(behind(&island.groups[0]), on_island(&bar, colors.base));
            assert_eq!(behind(&island.groups[2]), on_island(&bar, colors.base));
            // Only the current workspace shows its focused window as a pill.
            assert_eq!(island.groups[1].dots[1].rect.w, ACTIVE_WIDTH);
            assert!(island.groups[0].dots.iter().all(|dot| dot.rect.w == DOT));

            snapshot(&frame, &format!("bar-expanded-{theme}"));
        }
    }

    #[test]
    fn the_island_stays_open_until_the_pointer_leaves_it() {
        let mut bar = sample("rose-pine-moon");
        frame(&mut bar, Instant::now());
        let collapsed = resting(&bar, false).rect;
        let expanded = resting(&bar, true).rect;

        // Past the resting island there is nothing to hover.
        assert!(!bar.hover(Some((collapsed.right() + 4.0, 18.0))));
        // The screen corner counts, for an easy throw of the pointer.
        assert!(bar.hover(Some((0.0, 0.0))));
        // Once open, the whole opened island keeps it open…
        assert!(!bar.hover(Some((expanded.right() - 2.0, 18.0))));
        // …and moving off its end, or off the bar, closes it.
        assert!(bar.hover(Some((expanded.right() + 2.0, 18.0))));
        assert!(bar.hover(Some((collapsed.x + 10.0, 18.0))));
        assert!(bar.hover(None));

        assert_eq!(frame(&mut bar, Instant::now()).1, Paint::Animating);
        assert_eq!(frame(&mut bar, later()).1, Paint::Idle);
        assert_eq!(windows(&bar), [21, 22, 23]);
    }

    #[test]
    fn focus_changes_animate_then_settle() {
        let mut bar = sample("rose-pine-moon");
        // Another window of the same workspace takes focus.
        bar.set_workspaces(desktop(2, 23));
        assert_eq!(frame(&mut bar, Instant::now()).1, Paint::Animating);
        assert_eq!(frame(&mut bar, later()).1, Paint::Idle);
        assert!(bar.hits[2].0.w > bar.hits[1].0.w);

        // Switching workspace swaps the island's windows for the new ones.
        bar.set_workspaces(desktop(1, 11));
        assert_eq!(frame(&mut bar, Instant::now()).1, Paint::Animating);
        assert_eq!(frame(&mut bar, later()).1, Paint::Idle);
        assert_eq!(windows(&bar), [11, 12]);
        assert_eq!(workspaces(&bar), [1]);
    }

    #[test]
    fn closed_windows_and_workspaces_are_forgotten() {
        let mut bar = sample("rose-pine-moon");
        assert_eq!((bar.current.len(), bar.activity.len()), (4, 6));
        bar.set_workspaces(vec![workspace(2, true, &[22], 22)]);
        assert_eq!((bar.current.len(), bar.activity.len()), (1, 1));
    }

    #[test]
    fn the_status_island_and_the_bell_share_the_end_of_the_bar() {
        let mut bar = sample("rose-pine-moon");
        // Nothing to hover before the islands have been drawn.
        assert_eq!(bar.status_hover(Some((890.0, 18.0))), None);
        assert!(!bar.bell_hover(Some((899.0, 18.0))));
        frame(&mut bar, Instant::now());
        let island = bar.status.expect("a status island");
        let bell = bar.bell.expect("the bell's island");
        // The bell at the very end, round, and the status island before it.
        assert!((bell.right() - (900.0 - EDGE)).abs() < 0.5);
        assert_eq!(bell.w, bell.h);
        assert_eq!(island.right(), bell.x - APART);

        assert_eq!(bar.status_hover(Some((island.x - 1.0, 18.0))), None);
        assert_eq!(bar.status_hover(Some((island.x + 1.0, 18.0))), Some(true));
        // Still on it: no news. Up to halfway to the bell, it counts.
        assert_eq!(bar.status_hover(Some((bell.x - APART / 2.0 - 1.0, 0.0))), None);
        // On the bell, or past it in the corner: the bell's, not the
        // status island's.
        assert_eq!(bar.status_hover(Some((bell.x + 1.0, 18.0))), Some(false));
        assert!(bar.bell_hover(Some((bell.x + 1.0, 18.0))));
        assert!(!bar.bell_hover(Some((899.0, 0.0))));
        assert!(bar.bell_hovered);
        assert!(bar.bell_hover(Some((450.0, 18.0))));
        assert_eq!(bar.status_hover(Some((island.x + 1.0, 18.0))), Some(true));
        assert_eq!(bar.status_hover(None), Some(false));

        // A desktop has no battery — nor, perhaps, Wi-Fi or sound: then
        // there is only the bell.
        bar.battery = None;
        bar.link = None;
        bar.sound = None;
        frame(&mut bar, later());
        assert_eq!(bar.status, None);
        assert!(bar.bell.is_some());
        assert_eq!(bar.status_hover(Some((bell.x - 20.0, 18.0))), None);
        snapshot(&frame(&mut bar, later()).0, "bar-status-desktop");
    }

    #[test]
    fn the_status_island_opens_the_controls_and_the_bell_the_history() {
        let mut bar = sample("rose-pine-moon");
        bar.dropdown = Dropdown::with_program("enso-ctl", "the island", "/nonexistent/enso-ctl");
        bar.history = History::listening_as("enso-test-nobody");
        assert!(bar.may_open_on_hover());
        // Even over the history: the pointer has moved on to the controls.
        bar.notices.open = true;
        assert!(bar.may_open_on_hover());

        // A click opens at once, without the wait.
        bar.intent_due = Some(Instant::now());
        bar.click_status();
        assert_eq!(bar.intent_due, None);

        // Resting on the bell opens the history after the same wait; with
        // no daemon about, nothing is held, and leaving says nothing.
        frame(&mut bar, later());
        let bell = bar.bell.expect("a bell");
        bar.bell_hover(Some((bell.x + 2.0, 18.0)));
        assert!(bar.bell_hovered);
        bar.history.hover();
        assert!(!bar.history.is_held());

        // Switched off in the config, resting opens nothing; a click still
        // does.
        bar.config.control_center = false;
        assert!(!bar.may_open_on_hover());
    }

    /// The note says where every island rests, the bell and the status
    /// island with the clock, and which one the pointer lights.
    #[test]
    fn the_note_says_where_each_island_is_and_which_is_lit() {
        let mut bar = sample("rose-pine-moon");
        bar.output = Some("TEST-1".to_owned());
        bar.notes = true;
        // A note can't be kept here; what would have been is.
        frame(&mut bar, later());
        let (_, note) = bar.noted.clone().expect("a note");
        let bell = bar.bell.expect("a bell");
        assert_eq!(note.screen, 900.0);
        assert_eq!(note.bell.map(|spot| (spot.x, spot.w)), Some((bell.x, bell.w)));
        assert!(note.status.is_some_and(|spot| spot.x + spot.w == bell.x - APART));
        assert_eq!(note.lit, None);
        bar.bell_hover(Some((bell.x + 2.0, 18.0)));
        frame(&mut bar, later());
        assert_eq!(bar.noted.clone().map(|(_, note)| note.lit), Some(Some(island::Which::Bell)));
    }

    #[test]
    fn resting_on_the_clock_shows_whats_playing() {
        let mut bar = sample("rose-pine-moon");
        bar.playing = Dropdown::with_program("enso-play", "the clock", "/nonexistent/enso-play");
        frame(&mut bar, later());
        let clock = bar.clock_island.expect("the clock is drawn");
        // Resting on the clock starts the short wait before it opens…
        bar.clock_hover(Some((clock.x + clock.w / 2.0, 18.0)));
        assert!(bar.clock_hovered && bar.playing_due.is_some());
        // …and moving off abandons it. Either side of the clock is not it.
        bar.clock_hover(Some((clock.x - 1.0, 18.0)));
        assert!(!bar.clock_hovered && bar.playing_due.is_none());
        bar.clock_hover(Some((clock.right() + 1.0, 18.0)));
        assert!(!bar.clock_hovered);
        // Without what's playing, the calendar still waits to open…
        bar.config.now_playing = false;
        bar.clock_hover(Some((clock.x + 1.0, 18.0)));
        assert!(bar.clock_hovered && bar.playing_due.is_some());
        // …and with both switched off, resting there opens nothing.
        bar.clock_hover(None);
        bar.config.calendar = false;
        bar.clock_hover(Some((clock.x + 1.0, 18.0)));
        assert!(bar.clock_hovered && bar.playing_due.is_none());
    }

    #[test]
    fn the_clock_fills_with_the_sea_while_something_plays() {
        let mut bar = sample("rose-pine-moon");
        let (dry, _) = frame(&mut bar, later());
        let clock = bar.clock_island.expect("the clock is drawn");
        bar.water.jump(1.0);
        let (wet, _) = frame(&mut bar, later());
        snapshot(&wet, "bar-playing");
        // The sea fills the pill, and the time stays in front of it.
        let low = ((clock.bottom() - 3.0) * 2.0) as u32;
        let centre = ((clock.x + clock.w / 2.0) * 2.0) as u32;
        assert_ne!(wet.pixel(centre, low), dry.pixel(centre, low));
        let text = bar.ctx.theme.colors.text;
        let lit = (0..wet.height)
            .flat_map(|y| {
                ((clock.x * 2.0) as u32..(clock.right() * 2.0) as u32).map(move |x| (x, y))
            })
            .filter(|&(x, y)| wet.pixel(x, y)[..3] == [text.r, text.g, text.b])
            .count();
        assert!(lit > 50, "{lit}");
        // Playing, the sea moves; stopped, it ebbs and stills.
        bar.water.set(1.0, Transition::INSTANT, Instant::now());
        assert!(bar.water_moving(Instant::now()));
        bar.water.set(0.0, Transition::INSTANT, Instant::now());
        assert!(!bar.water_moving(later()));
    }

    #[test]
    fn the_bell_says_when_something_is_waiting() {
        let mut bar = sample("rose-pine-moon");
        let colors = bar.ctx.theme.colors.clone();
        assert_eq!(bar.tint(Item::Bell), colors.subtle);
        let (quiet, _) = frame(&mut bar, later());

        bar.notices.unseen = 2;
        assert_eq!(bar.tint(Item::Bell), colors.text);
        let (waiting, _) = frame(&mut bar, later());
        snapshot(&waiting, "bar-status-unseen");

        // The dot is drawn in the accent, on the bell's island.
        let island = bar.bell.unwrap();
        let accent = bar.ctx.accent();
        let has_accent = |image: &Offscreen| {
            let (x0, x1) = ((island.x * 2.0) as u32, (island.right() * 2.0) as u32);
            (x0..x1).any(|x| {
                (0..image.height).any(|y| image.pixel(x, y) == [accent.r, accent.g, accent.b, 255])
            })
        };
        assert!(has_accent(&waiting));
        assert!(!has_accent(&quiet));
    }

    /// Something new arriving rings the bell — a swing that dies away — and
    /// then the island is still again.
    #[test]
    fn the_bell_rings_when_something_new_arrives() {
        let mut bar = sample("rose-pine-moon");
        assert_eq!(bar.swing(Instant::now()), (0.0, false));
        let start = Instant::now();
        bar.ringing = Some(start);
        let (_, paint) = frame(&mut bar, start + Duration::from_millis(80));
        assert_eq!(paint, Paint::Animating);
        assert!(bar.swing(start + Duration::from_millis(80)).0.abs() > 0.05);
        assert_eq!(bar.swing(start + RING), (0.0, false));
        assert_eq!(frame(&mut bar, start + RING).1, Paint::Idle);

        // With motion switched off, it does not ring at all.
        bar.ctx.motion = enso_core::Motion::none();
        assert_eq!(bar.swing(start + Duration::from_millis(80)), (0.0, false));
    }

    /// What's out of the ordinary shows while it is, ahead of the rest.
    #[test]
    fn unusual_states_show_while_they_last() {
        let mut bar = sample("rose-pine-moon");
        let before = bar.status_items().len();
        bar.around.casts = vec![7];
        bar.mic = Mic {
            listened: true,
            muted: false,
            volume: 70,
        };
        bar.sound = Some(Level {
            volume: 60,
            muted: true,
        });
        bar.around.layout = Some("RU".into());
        let items = bar.status_items();
        assert_eq!(
            items[..4],
            [Item::Share, Item::Mic(false), Item::Muted, Item::Layout]
        );
        assert_eq!(items.len(), before + 4);
        // Sharing and a live microphone are in love; the rest stay quiet.
        let colors = bar.ctx.theme.colors.clone();
        assert_eq!(bar.tint(Item::Share), colors.love);
        assert_eq!(bar.tint(Item::Mic(true)), colors.subtle);
        let (frame, _) = frame(&mut bar, later());
        assert!(bar.share.is_some(), "the sharing mark is a button");
        snapshot(&frame, "bar-unusual");

        // Do not disturb quietens the bell.
        bar.quiet = true;
        assert_eq!(bar.figure(Item::Bell), Some(Figure::BellOff));
    }

    /// A change of level widens the island to show it, then it folds back;
    /// the first word from the sound server is only how things are.
    #[test]
    fn the_island_shows_a_level_as_it_changes() {
        let mut bar = sample("rose-pine-moon");
        bar.heard = false;
        bar.sound = None;
        let heard = |volume, muted| Heard {
            output: Some(Level { volume, muted }),
            mic: Mic::default(),
        };
        bar.hear(heard(40, false));
        assert_eq!(bar.osd.due(), None, "the first reading shows nothing");
        frame(&mut bar, later());
        let resting = bar.status.unwrap().w;

        bar.hear(heard(45, false));
        assert!(bar.osd.due().is_some());
        let (shown, _) = frame(&mut bar, later());
        assert!(bar.status.unwrap().w > resting, "the island widens");
        snapshot(&shown, "bar-level");

        // Brightness too, but not its first reading.
        bar.brighten(light::Lamp::Screen, 50);
        bar.brighten(light::Lamp::Screen, 55);
        assert!(bar.osd.due().is_some());
        // The keyboard's light the same way, on its own reading.
        bar.brighten(light::Lamp::Keyboard, 0);
        bar.brighten(light::Lamp::Keyboard, 33);
        assert!(bar.osd.due().is_some());
    }

    /// Resting on a window's dot, the clock says what the window is.
    #[test]
    fn the_clock_names_the_window_under_the_pointer() {
        let mut bar = sample("rose-pine-moon");
        frame(&mut bar, later());
        bar.around.titles.insert(22, "Quiet Hours — Firefox".into());
        let dot = bar
            .hits
            .iter()
            .find(|(_, target)| *target == Target::Window(22))
            .unwrap()
            .0;
        let (x, y) = dot.center();
        assert!(bar.point(Some((x, y))));
        assert_eq!(bar.clock_says(), "Quiet Hours — Firefox");
        // It glides to its new width.
        assert_eq!(frame(&mut bar, later()).1, Paint::Animating);
        let (named, _) = frame(&mut bar, later() + Duration::from_secs(1));
        snapshot(&named, "bar-window-named");
        assert!(bar.point(None));
        assert_eq!(bar.clock_says(), bar.clock);
    }

    #[test]
    fn the_status_island_shows_network_and_battery() {
        let mut bar = sample("rose-pine-moon");
        let colors = bar.ctx.theme.colors.clone();
        let kinds = |bar: &Bar| -> Vec<&'static str> {
            bar.status_items()
                .into_iter()
                .map(|item| match item {
                    Item::Network(Link::Wired) => "cable",
                    Item::Network(_) => "wifi",
                    Item::Battery(_) => "battery",
                    Item::Bell => "bell",
                    Item::Share => "share",
                    Item::Mic(true) => "mic off",
                    Item::Mic(false) => "mic",
                    Item::Muted => "muted",
                    Item::Awake => "awake",
                    Item::Layout => "layout",
                })
                .collect()
        };
        // The volume is known (for scrolling) but not shown.
        assert!(bar.sound.is_some());
        // The bell is on an island of its own.
        assert_eq!(kinds(&bar), ["wifi", "battery"]);
        let tints = |bar: &Bar| -> Vec<Rgba> {
            bar.status_items()
                .into_iter()
                .map(|item| bar.tint(item))
                .collect()
        };
        assert_eq!(tints(&bar), [colors.subtle, colors.text]);

        bar.link = Some(Link::Offline);
        bar.battery.as_mut().unwrap().percent = 12;
        assert_eq!(tints(&bar), [colors.subtle, colors.love]);
        snapshot(&frame(&mut bar, later()).0, "bar-status-offline");

        bar.link = Some(Link::Wired);
        bar.battery = None;
        assert_eq!(kinds(&bar), ["cable"]);

        // Under the pointer the island warms, then settles lit.
        frame(&mut bar, Instant::now());
        let island = bar.status.unwrap();
        assert_eq!(bar.status_hover(Some((island.x + 2.0, 18.0))), Some(true));
        assert_eq!(frame(&mut bar, Instant::now()).1, Paint::Animating);
        let (lit, paint) = frame(&mut bar, later());
        assert_eq!(paint, Paint::Idle);
        let (x, y) = ((island.x + 5.0) * 2.0, 36.0);
        assert_eq!(
            lit.pixel(x as u32, y as u32),
            on_island(&bar, colors.overlay)
        );
        snapshot(&lit, "bar-status-hovered");
    }

    /// Signal lights the arcs from the inside out; the rest stay whole, in a
    /// whisper of the same colour.
    #[test]
    fn strength_lights_arcs_and_leaves_the_rest_faint() {
        let mut bar = sample("rose-pine-moon");
        let colors = bar.ctx.theme.colors.clone();
        let faint = colors.subtle.mix(colors.base, icons::UNLIT);
        let count = |frame: &Offscreen, bar: &Bar, color: Rgba| {
            let island = bar.status.unwrap();
            let xs = (island.x * 2.0) as u32..(island.right() * 2.0) as u32;
            xs.flat_map(|x| (0..frame.height).map(move |y| (x, y)))
                .filter(|&(x, y)| frame.pixel(x, y) == [color.r, color.g, color.b, 255])
                .count()
        };
        bar.link = Some(Link::Wifi { bars: 4 });
        let (full, _) = frame(&mut bar, later());
        // Fully lit, nothing is faint (but the odd edge pixel, whose blend
        // of icon and island happens to land on the same colour).
        let edges = count(&full, &bar, faint);
        assert!(
            edges < 10,
            "everything is lit, yet {edges} pixels are faint"
        );

        bar.link = Some(Link::Wifi { bars: 1 });
        let (weak, _) = frame(&mut bar, later());
        assert!(
            count(&weak, &bar, faint) > 40,
            "unlit arcs are drawn faintly"
        );
        assert!(count(&weak, &bar, colors.subtle) < count(&full, &bar, colors.subtle));
        snapshot(&weak, "bar-status-weak");
    }

    /// A taller bar (`height` under `[bar]`) gets a larger island: every
    /// icon, and the battery, scale with it.
    #[test]
    fn the_status_island_grows_with_the_bar() {
        let mut bar = sample("rose-pine-moon");
        frame(&mut bar, later());
        let normal = bar.status.unwrap();

        bar.config.height = 56;
        let mut text = TextEngine::new(&bar.ctx.config.appearance);
        let tall = Offscreen::render(&mut text, (900, 56), 2, |canvas| {
            bar.draw(canvas, later());
        });
        let large = bar.status.unwrap();
        let ratio = (56.0 - 2.0 * INSET) / (36.0 - 2.0 * INSET);
        assert!((large.h / normal.h - ratio).abs() < 0.01);
        assert!(
            (large.w / normal.w - ratio).abs() < 0.15,
            "{} vs {}",
            large.w,
            normal.w
        );
        snapshot(&tall, "bar-status-tall");
    }

    /// The battery is a pill filled to its charge, the figures cut out of
    /// the fill; on power it fills in foam and a bolt comes before them.
    #[test]
    fn the_battery_is_a_pill_filled_to_its_charge() {
        let mut bar = sample("rose-pine-moon");
        let colors = bar.ctx.theme.colors.clone();
        let rgba = |c: Rgba| [c.r, c.g, c.b, 255];
        // The pill's middle row, in physical pixels (the frame is at 2x).
        let row = |frame: &Offscreen, bar: &Bar| -> Vec<[u8; 4]> {
            let island = bar.status.unwrap();
            let y = (36.0 / 2.0 * 2.0) as u32;
            ((island.x * 2.0) as u32..(island.right() * 2.0) as u32)
                .map(|x| frame.pixel(x, y))
                .collect()
        };

        let (discharging, _) = frame(&mut bar, later());
        let pixels = row(&discharging, &bar);
        let count = |pixels: &[[u8; 4]], color: Rgba| {
            pixels.iter().filter(|pixel| **pixel == rgba(color)).count()
        };
        assert_eq!(bar.charge().unwrap().fill, colors.text);
        assert!(count(&pixels, colors.text) > 20, "no fill");
        assert!(count(&pixels, colors.highlight_med) > 4, "no track");
        let width = bar.status.unwrap().w;
        snapshot(&discharging, "bar-battery-82");

        // On power: foam, and a bolt that widens the pill. Between the
        // light's runs along it, foam all along.
        bar.battery.as_mut().unwrap().status = BatteryStatus::Charging;
        let between = bar.epoch + Duration::from_secs_f32(SHINE + 1.0);
        let (charging, _) = frame(&mut bar, between);
        assert_eq!(bar.charge().unwrap().fill, colors.foam);
        assert!(count(&row(&charging, &bar), colors.foam) > 20);
        assert!(bar.status.unwrap().w > width);
        snapshot(&charging, "bar-battery-charging");
        // Now and then, a light runs along the charge.
        let running = bar.epoch + Duration::from_secs_f32(SHINE * 0.5);
        let (shining, _) = frame(&mut bar, running);
        assert!(row(&shining, &bar) != row(&charging, &bar), "no light ran along it");

        // Nearly empty: a sliver of love, and the figures over the track.
        bar.battery.as_mut().unwrap().status = BatteryStatus::Discharging;
        bar.battery.as_mut().unwrap().percent = 9;
        let (low, _) = frame(&mut bar, later());
        assert_eq!(bar.charge().unwrap().fill, colors.love);
        let pixels = row(&low, &bar);
        assert!(count(&pixels, colors.love) > 0);
        assert!(count(&pixels, colors.highlight_med) > count(&pixels, colors.love));
        snapshot(&low, "bar-battery-low");

        // Without a battery, no pill: the island is glyphs and the bell.
        bar.battery = None;
        frame(&mut bar, later());
        assert!(bar.status.unwrap().w < width);
    }

    #[test]
    fn only_shows_workspaces_of_its_own_output() {
        let mut bar = sample("rose-pine-moon");
        bar.output = Some("HDMI-A-1".into());
        assert_eq!(bar.visible_workspaces().count(), 0);
        assert!(bar.island(bar.strip, 1.0, Tween::target).is_none());
        bar.output = Some("eDP-1".into());
        assert_eq!(bar.visible_workspaces().count(), 4);
    }
}
