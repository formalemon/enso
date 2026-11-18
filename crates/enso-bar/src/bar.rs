//! The bar itself: three floating islands — windows, clock, status.
//!
//! The right island is a small cluster — network and volume glyphs, the
//! battery where there is one, and a bell — that warms a shade under the
//! pointer, because resting there opens the control center and clicking it
//! opens the notification history. The bell is always there, so a desktop
//! with no battery and no Wi-Fi still has an island to rest on.
//!
//! Its icons are drawn, not taken from a font (see [`icons`]): Wi-Fi or a
//! cable, the battery — a pill holding its percentage, filled
//! as far as it is charged — and the bell, which rings when something new
//! arrives. They are sized from the bar's height, so a taller or shorter bar
//! keeps the same island, larger or smaller.
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
    Context, Figure, Notices, Rgba, Transition, Tween,
    style::{edge, space, type_scale},
};
use enso_sys::{
    net::{self, Link},
    power::{Battery, Sysfs},
};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Layer, Paint, PointerEvent, Proxy, Rect,
    SurfaceSpec, Text, watch::watch_dir,
};

use crate::{
    config::{BarConfig, DEFAULT_CLOCK, Position},
    dropdown::{Dropdown, History},
    icons::{self, Charge},
    niri::{self, Workspace},
    sound::{self, Level},
};

/// Gap between the islands and the screen edges.
const EDGE: f32 = edge::SCREEN;
/// Vertical inset of the islands inside the bar.
const INSET: f32 = edge::ISLAND_INSET;
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
/// How long the pointer rests on the status island before the control
/// center opens: long enough that passing over it opens nothing.
const HOVER_INTENT: Duration = Duration::from_millis(150);
/// How often the readings nothing announces (network, battery) are looked at.
const POLL: Duration = Duration::from_secs(5);
/// Messages from background threads.
#[derive(Debug)]
pub enum Message {
    Workspaces(Vec<Workspace>),
    /// The output's volume changed (`None`: no output).
    Sound(Option<Level>),
    ThemeChanged,
}

/// How long the bell rings when something new arrives.
const RING: Duration = Duration::from_millis(900);

/// One thing the status island shows, left to right.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Item {
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
    /// The pointer is resting on the status island.
    status_hovered: bool,
    /// When the pointer will have rested there long enough to mean it.
    intent_due: Option<Instant>,
    /// The control center, opened from the status island.
    dropdown: Dropdown,
    /// The notification history, opened by clicking the island.
    history: History,
    /// What the notification daemon last reported.
    notices: Notices,
    /// The island was clicked: resting on it opens nothing more until the
    /// pointer has left and come back.
    clicked: bool,
    /// When the bell began ringing, if it is.
    ringing: Option<Instant>,
}

impl Bar {
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
            status_hovered: false,
            intent_due: None,
            dropdown: Dropdown::new(),
            history: History::new(),
            notices: Notices::default(),
            clicked: false,
            ringing: None,
        };
        bar.notices = Notices::read(&bar.ctx.paths);
        bar.refresh_clock();
        bar.poll();
        bar
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
        cx.wake_in(due.saturating_duration_since(Instant::now()));
    }

    /// Track the pointer against the status island. Everything from the
    /// island's near end to the screen corner counts. Returns the new state
    /// when it changes.
    fn status_hover(&mut self, pointer: Option<(f32, f32)>) -> Option<bool> {
        let hovered = pointer.is_some_and(|(x, _)| self.status.is_some_and(|island| x >= island.x));
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

    /// Follow the pointer for the control center's sake: keep an open panel
    /// informed, and start (or abandon) the wait before opening one.
    fn follow_status(&mut self, pointer: Option<(f32, f32)>, cx: &mut Cx<'_>) {
        if let Some(hovering) = self.status_hover(pointer) {
            self.dropdown.tell(hovering);
            cx.redraw();
        }
        if !self.status_hovered {
            self.intent_due = None;
            self.clicked = false;
        } else if self.intent_due.is_none() && self.may_open_on_hover() && !self.dropdown.is_open()
        {
            self.intent_due = Some(Instant::now() + HOVER_INTENT);
        }
        self.arm(cx);
    }

    /// Whether resting on the island opens the control center: not when
    /// that is switched off, and not on top of the notification history.
    fn may_open_on_hover(&self) -> bool {
        self.config.control_center && !self.clicked && !self.notices.open
    }

    /// A click on the island: the notification history takes the control
    /// center's place (or closes, if it was open).
    fn click_status(&mut self) {
        self.intent_due = None;
        self.clicked = true;
        self.dropdown.close();
        self.history.toggle();
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
        color.alpha(if self.config.opacity.is_finite() {
            self.config.opacity
        } else {
            1.0
        })
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

    fn draw_clock(&self, canvas: &mut Canvas<'_>, bar: Rect, size: f32) {
        let colors = &self.ctx.theme.colors;
        let text = Text::new(&self.clock, size, colors.text).medium();
        let width = canvas.measure(&text).w + 2.0 * space::LG;
        let island = bar.centered(width, bar.h);
        canvas.pill(island, self.island_color(colors.base));
        canvas.text_in(&text, island, Align::Center);
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
            (false, _) => colors.subtle,
        };
        Some(Charge {
            percent: battery.percent.min(100),
            plugged,
            fill,
        })
    }

    /// What the status island shows, left to right: the network where there
    /// is such, the battery where there is one, and the bell, always — so
    /// there is always an island to rest on. The volume is not shown (the
    /// control center has it), but scrolling on the island still turns it.
    fn status_items(&self) -> Vec<Item> {
        let mut items = Vec::with_capacity(3);
        items.extend(self.link.map(Item::Network));
        items.extend(self.charge().map(Item::Battery));
        items.push(Item::Bell);
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
            Item::Network(_) | Item::Bell => colors.subtle,
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

    /// The status island: Wi-Fi (or a cable), the battery,
    /// and the bell, every one drawn at the bar's scale. Returns whether it
    /// is still animating.
    fn draw_status(&mut self, canvas: &mut Canvas<'_>, bar: Rect, now: Instant) -> bool {
        self.status = None;
        let scale = icons::Scale::new(bar.h);
        let battery = icons::Battery::new(scale);
        let items = self.status_items();
        let colors = &self.ctx.theme.colors;

        let figures = items.iter().find_map(|item| match item {
            Item::Battery(charge) => Some(charge.percent.to_string()),
            _ => None,
        });
        let label = figures
            .as_deref()
            .map(|figures| Text::new(figures, battery.figures(), colors.text).bold());
        let width_of = |item: &Item, canvas: &mut Canvas<'_>| match item {
            Item::Network(link) => scale.width(network(*link)),
            Item::Bell => scale.width(Figure::Bell),
            Item::Battery(charge) => label.as_ref().map_or(0.0, |label| {
                battery.width(canvas.measure(label).w, charge.plugged)
            }),
        };
        let widths: Vec<f32> = items.iter().map(|item| width_of(item, canvas)).collect();
        let width =
            widths.iter().sum::<f32>() + (items.len() - 1) as f32 * scale.gap() + 2.0 * scale.pad();
        let island = Rect::new(bar.right() - width, bar.y, width, bar.h);
        self.status = Some(island);

        // It warms a shade under the pointer: there is something behind it.
        let glow = self.status_glow.get(now);
        let ground = colors.base.mix(colors.overlay, glow);
        canvas.pill(island, self.island_color(ground));
        let (swing, ringing) = self.swing(now);
        let accent = self.ctx.accent();

        let icon_h = scale.icon();
        let mut x = island.x + scale.pad();
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
            match item {
                Item::Network(link) => icons::draw(canvas, network(link), icon, ink),
                Item::Battery(charge) => {
                    if let Some(label) = &label {
                        battery.draw(canvas, slot, charge, label, glow, colors);
                    }
                }
                Item::Bell => {
                    let dot = (self.notices.unseen > 0).then_some(accent);
                    icons::bell(canvas, icon, swing, dot, ink);
                }
            }
            x += width + scale.gap();
        }
        self.status_glow.is_animating(now) || ringing
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
            ..SurfaceSpec::new("enso-bar", Layer::Top, edge | Anchor::LEFT | Anchor::RIGHT)
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let workspaces = proxy.clone();
        niri::watch(move |snapshot| workspaces.send(Message::Workspaces(snapshot)));
        let volume = proxy.clone();
        sound::watch(move |level| volume.send(Message::Sound(level)));

        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-bar: live theme switching unavailable: {err}");
        }
        self.refresh_clock();
        self.arm(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Workspaces(workspaces)) => {
                self.set_workspaces(workspaces);
                cx.redraw();
            }
            // Kept for scrolling on the island; nothing shows it.
            Event::Message(Message::Sound(level)) => self.sound = level,
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
                if self.ctx.reload_theme() || changed {
                    cx.redraw();
                }
            }
            Event::Wake => {
                let now = Instant::now();
                if now >= self.clock_due {
                    self.refresh_clock();
                    cx.redraw();
                }
                if now >= self.poll_due {
                    if self.poll() {
                        cx.redraw();
                    }
                    // A control center that has closed is cleaned up after
                    // here, rather than left until the next hover.
                    self.dropdown.is_open();
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
                self.follow_status(Some((x, y)), cx);
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
                if self.hover(None) {
                    cx.redraw();
                }
                self.follow_status(None, cx);
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if self.status_hovered {
                    self.click_status();
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
        self.draw_clock(canvas, strip, size);
        let glowing = self.draw_status(canvas, strip, now);
        Paint::when(animating || glowing)
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
        // Each call gets its own directory so parallel tests never collide.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let paths = Paths::rooted(
            std::env::temp_dir().join(format!("enso-bar-test-{}-{unique}", std::process::id())),
        );
        enso_core::theme::set_active(&paths, theme).expect("write theme state");
        let mut bar = Bar::new(Context::load_from(paths), BarConfig::default());
        bar.clock = "Sun 15 Nov  22:46".into();
        bar.battery = Some(Battery {
            name: "BAT1".into(),
            percent: 82,
            status: BatteryStatus::Discharging,
            power_mw: None,
            minutes_left: None,
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
    fn the_status_island_is_hovered_from_its_near_end_to_the_corner() {
        let mut bar = sample("rose-pine-moon");
        // Nothing to hover before the island has been drawn.
        assert_eq!(bar.status_hover(Some((890.0, 18.0))), None);
        frame(&mut bar, Instant::now());
        let island = bar.status.expect("a status island");
        assert!(island.right() <= 900.0 - EDGE + 0.5);

        assert_eq!(bar.status_hover(Some((island.x - 1.0, 18.0))), None);
        assert_eq!(bar.status_hover(Some((island.x + 1.0, 18.0))), Some(true));
        // Still on it: no news. The corner past its end counts too.
        assert_eq!(bar.status_hover(Some((899.0, 0.0))), None);
        assert_eq!(bar.status_hover(Some((450.0, 18.0))), Some(false));
        assert_eq!(bar.status_hover(Some((island.x + 1.0, 18.0))), Some(true));
        assert_eq!(bar.status_hover(None), Some(false));

        // A desktop has no battery — nor, perhaps, Wi-Fi or sound — but
        // always the bell, so always an island to rest on.
        let with_battery = island.w;
        bar.battery = None;
        bar.link = None;
        bar.sound = None;
        frame(&mut bar, later());
        let island = bar.status.expect("an island with only the bell");
        assert!(island.w < with_battery / 2.0);
        assert_eq!(bar.status_hover(Some((island.x + 1.0, 18.0))), Some(true));
        snapshot(&frame(&mut bar, later()).0, "bar-status-desktop");
    }

    #[test]
    fn resting_opens_the_control_center_and_a_click_the_history() {
        let mut bar = sample("rose-pine-moon");
        bar.dropdown = Dropdown::with_program("/nonexistent/enso-ctl");
        bar.history = History::with_program("/nonexistent/enso-notify");
        assert!(bar.may_open_on_hover());

        // After a click, resting there opens nothing more…
        bar.click_status();
        assert!(bar.clicked && !bar.may_open_on_hover());
        assert_eq!(bar.intent_due, None);
        // …and neither does it while the history is open, whoever opened it.
        bar.clicked = false;
        bar.notices.open = true;
        assert!(!bar.may_open_on_hover());
        bar.notices.open = false;
        assert!(bar.may_open_on_hover());

        // Switched off in the config, the island still takes the click.
        bar.config.control_center = false;
        assert!(!bar.may_open_on_hover());
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

        // The dot is drawn in the accent, somewhere on the island's end.
        let island = bar.status.unwrap();
        let accent = bar.ctx.accent();
        let has_accent = |image: &Offscreen| {
            let (x0, x1) = (
                ((island.right() - 40.0) * 2.0) as u32,
                (island.right() * 2.0) as u32,
            );
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

    #[test]
    fn the_status_island_shows_network_battery_and_bell() {
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
                })
                .collect()
        };
        // The volume is known (for scrolling) but not shown.
        assert!(bar.sound.is_some());
        assert_eq!(kinds(&bar), ["wifi", "battery", "bell"]);
        let tints = |bar: &Bar| -> Vec<Rgba> {
            bar.status_items()
                .into_iter()
                .map(|item| bar.tint(item))
                .collect()
        };
        assert_eq!(tints(&bar), [colors.subtle, colors.subtle, colors.subtle]);

        bar.link = Some(Link::Offline);
        bar.battery.as_mut().unwrap().percent = 12;
        assert_eq!(tints(&bar), [colors.subtle, colors.love, colors.subtle]);
        snapshot(&frame(&mut bar, later()).0, "bar-status-offline");

        bar.link = Some(Link::Wired);
        bar.battery = None;
        assert_eq!(kinds(&bar), ["cable", "bell"]);

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
        assert_eq!(bar.charge().unwrap().fill, colors.subtle);
        assert!(count(&pixels, colors.subtle) > 20, "no fill");
        assert!(count(&pixels, colors.highlight_med) > 4, "no track");
        let width = bar.status.unwrap().w;
        snapshot(&discharging, "bar-battery-82");

        // On power: foam, and a bolt that widens the pill.
        bar.battery.as_mut().unwrap().status = BatteryStatus::Charging;
        let (charging, _) = frame(&mut bar, later());
        assert_eq!(bar.charge().unwrap().fill, colors.foam);
        assert!(count(&row(&charging, &bar), colors.foam) > 20);
        assert!(bar.status.unwrap().w > width);
        snapshot(&charging, "bar-battery-charging");

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
