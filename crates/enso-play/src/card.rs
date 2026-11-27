//! The card: the cover, the title and who it's by, the timeline — a wave
//! for what has played, rolling slowly while it plays and lying flat when
//! paused — and three buttons. The card is warmed a touch by the cover's
//! colour.
//!
//! It has two presentations, as the control center does. Opened by
//! `enso-bar` while the pointer rests on the clock (`--dropdown`), it hangs
//! under the bar and goes when the pointer does, never taking the keyboard.
//! Opened on purpose (`enso-play`), it takes the keyboard and stays until
//! dismissed.
//!
//! With more than one player, dots at the foot choose between them: click
//! one, swipe two fingers sideways across the card (the player shown
//! following them, the next coming in beside it), or `Tab` from the
//! keyboard. Otherwise it follows whichever is playing.

use std::{
    collections::HashMap,
    f32::consts::TAU,
    io::BufRead,
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Icon, Rgba, Transition, Tween,
    style::{edge, icon, opacity, radius, space, stroke, type_scale},
};
use enso_sys::mpris::{Mpris, Player, Status};
use enso_ui::{
    Align, Anchor, App, Button, Canvas, Cx, Event, Key, KeyEvent, Keyboard, Layer, Paint,
    PointerEvent, Presence, Proxy, Rect, SurfaceSpec, Text,
    swipe::{self, Direction, Slide, Swiped},
    watch::watch_dir,
};

use crate::{
    art::Cover,
    player::{Inbox, Request, Update, Worker},
};
use enso_ui::widget::wave::{self, Wave};

/// The name the card listens under, and holds its one-copy lock by.
pub const NAME: &str = "enso-play";

const WIDTH: f32 = 340.0;
/// Gap between the card and the screen edge, as for every card.
const MARGIN: f32 = edge::SCREEN;
/// Gap between the bar and a dropdown card.
const DROP_GAP: f32 = edge::BELOW_BAR;
/// Room the surface keeps around the card for its shadow.
const SHADOW_ROOM: f32 = 28.0;
const PAD: f32 = space::LG;
const COVER: f32 = 64.0;
const TRACK: f32 = 24.0;
const TIMES: f32 = 16.0;
const CONTROLS: f32 = 48.0;
/// The play button's diameter, and the other buttons' glow.
const PLAY: f32 = 44.0;
const BUTTON: f32 = 38.0;
const DOTS: f32 = 16.0;
/// How far a key moves within the track.
const STEP: Duration = Duration::from_secs(5);
/// How long a dropdown outlives the pointer leaving it, so that crossing
/// the gap between the bar and the card does not close it.
const GRACE: Duration = Duration::from_millis(300);
/// How quickly the wave rolls on, in crests a second: slowly.
const ROLL: f32 = 0.5;
/// How much the cover's colour warms the card.
const WARMTH: f32 = 0.12;

/// Which screen edge a dropdown hangs from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// How the card was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// On purpose: it takes the keyboard and stays.
    Modal,
    /// From the bar, under the pointer.
    Dropdown(Edge),
}

#[derive(Debug)]
pub enum Message {
    Update(Update),
    /// From the bar: whether the pointer is on the clock.
    Owner(bool),
    /// Something wants this card gone: the key that opened it, again.
    Dismiss,
    ThemeChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Previous,
    PlayPause,
    Next,
    Track,
    Player(usize),
}

/// Where everything sits on the card.
#[derive(Clone, Debug, PartialEq)]
struct Layout {
    cover: Rect,
    title: Rect,
    byline: Rect,
    track: Rect,
    times: Rect,
    previous: Rect,
    play: Rect,
    next: Rect,
    dots: Vec<Rect>,
}

impl Layout {
    /// The player's part moved `dx` across; the dots stay.
    fn shifted(&self, dx: f32) -> Self {
        let by = |rect: Rect| rect.translate(dx, 0.0);
        Self {
            cover: by(self.cover),
            title: by(self.title),
            byline: by(self.byline),
            track: by(self.track),
            times: by(self.times),
            previous: by(self.previous),
            play: by(self.play),
            next: by(self.next),
            dots: self.dots.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Card {
    ctx: Context,
    mode: Mode,
    players: Vec<Player>,
    /// The player chosen with the dots or `Tab`, by bus name; otherwise the
    /// card follows whichever plays.
    chosen: Option<String>,
    /// Covers by address; `None` while loading, or when there's none to have.
    covers: HashMap<String, Option<Cover>>,
    worker: Worker,
    /// The players and the requests' inbox, until the worker is started.
    live: Option<(Mpris, Inbox)>,
    presence: Presence,
    /// `1` while playing: the wave's height, and the play button's figure.
    playing: Tween<f32>,
    /// How lit each button is by the pointer.
    glow: [Tween<f32>; 3],
    hovered: Option<Hit>,
    /// Where the thumb is held, `0..=1`, while it's being dragged.
    dragging: Option<f32>,
    held: Tween<f32>,
    /// The players side by side, swiped through.
    slide: Slide,
    pointer_inside: bool,
    owner_hovering: bool,
    leave_at: Option<Instant>,
    /// The surface, from the last frame or resize.
    bounds: Rect,
    /// When the card opened: the wave rolls from here.
    opened: Instant,
}

impl Card {
    /// A card showing `players`; with `live`, it starts the worker that
    /// follows them once it's running.
    pub fn new(
        ctx: Context,
        mode: Mode,
        players: Vec<Player>,
        worker: Worker,
        live: Option<(Mpris, Inbox)>,
    ) -> Self {
        let playing = players
            .first()
            .is_some_and(|player| player.status == Status::Playing);
        let slide = Slide::new(&ctx.motion);
        let mut card = Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            mode,
            players: Vec::new(),
            chosen: None,
            covers: HashMap::new(),
            worker,
            live,
            playing: Tween::new(if playing { 1.0 } else { 0.0 }),
            glow: [Tween::new(0.0), Tween::new(0.0), Tween::new(0.0)],
            hovered: None,
            dragging: None,
            held: Tween::new(0.0),
            slide,
            pointer_inside: false,
            // Whoever opened a dropdown is hovering until they say otherwise.
            owner_hovering: matches!(mode, Mode::Dropdown(_)),
            leave_at: None,
            bounds: Rect::new(0.0, 0.0, Self::surface_width(), Self::surface_height()),
            opened: Instant::now(),
        };
        card.set_players(players);
        card
    }

    fn surface_width() -> f32 {
        WIDTH + 2.0 * (MARGIN + SHADOW_ROOM)
    }

    fn surface_height() -> f32 {
        Self::height_for(2) + DROP_GAP + SHADOW_ROOM
    }

    /// The player shown.
    fn current(&self) -> Option<&Player> {
        self.chosen
            .as_ref()
            .and_then(|bus| self.players.iter().find(|player| &player.bus == bus))
            .or_else(|| self.players.first())
    }

    fn current_index(&self) -> usize {
        self.current()
            .and_then(|current| {
                self.players
                    .iter()
                    .position(|player| player.bus == current.bus)
            })
            .unwrap_or(0)
    }

    fn set_players(&mut self, players: Vec<Player>) {
        self.players = players;
        if self
            .chosen
            .as_ref()
            .is_some_and(|bus| !self.players.iter().any(|player| &player.bus == bus))
        {
            self.chosen = None;
        }
        self.follow_current();
    }

    /// Bring the wave and the cover in line with the player shown.
    fn follow_current(&mut self) {
        let (playing, art) = match self.current() {
            Some(player) => (player.status == Status::Playing, player.track.art.clone()),
            None => (false, None),
        };
        let target = if playing { 1.0 } else { 0.0 };
        if self.playing.target() != target {
            self.playing.set(
                target,
                self.ctx.motion.apply(Transition::SCENE),
                Instant::now(),
            );
        }
        // Every player's cover, so one swiped to comes in with its own.
        let wanted: Vec<String> = art
            .into_iter()
            .chain(self.players.iter().filter_map(|player| player.track.art.clone()))
            .collect();
        for url in wanted {
            if !self.covers.contains_key(&url) {
                self.covers.insert(url.clone(), None);
                self.worker.send(Request::Cover(url));
            }
        }
    }

    fn cover(&self) -> Option<&Cover> {
        self.cover_of(self.current()?)
    }

    fn cover_of(&self, player: &Player) -> Option<&Cover> {
        let url = player.track.art.as_ref()?;
        self.covers.get(url)?.as_ref()
    }

    fn height_for(players: usize) -> f32 {
        if players == 0 {
            return PAD + COVER + PAD;
        }
        let dots = if players > 1 { DOTS } else { 0.0 };
        PAD + COVER + space::MD + TRACK + TIMES + space::SM + CONTROLS + dots + PAD
    }

    /// Where the card rests in a surface of `bounds`: across the middle,
    /// hanging from the bar's edge.
    fn card_rect(&self, bounds: Rect) -> Rect {
        let height = Self::height_for(self.players.len());
        let width = WIDTH.min(bounds.w - 2.0 * MARGIN);
        let x = bounds.x + ((bounds.w - width) / 2.0).round();
        let y = match self.mode {
            Mode::Modal => MARGIN,
            Mode::Dropdown(Edge::Top) => DROP_GAP,
            Mode::Dropdown(Edge::Bottom) => bounds.bottom() - DROP_GAP - height,
        };
        Rect::new(x, y, width, height)
    }

    fn layout(&self, card: Rect) -> Layout {
        let inner = card.inset_xy(PAD, 0.0);
        let cover = Rect::new(inner.x, card.y + PAD, COVER, COVER);
        let text_x = cover.right() + space::MD;
        let text_w = inner.right() - text_x;
        let line = self.ctx.config.appearance.font_size * type_scale::LINE_HEIGHT;
        let middle = cover.y + COVER / 2.0;
        let title = Rect::new(text_x, middle - line - 1.0, text_w, line);
        let byline = Rect::new(text_x, middle + 1.0, text_w, line);

        let track = Rect::new(inner.x, cover.bottom() + space::MD, inner.w, TRACK);
        let times = Rect::new(inner.x, track.bottom(), inner.w, TIMES);
        let controls = Rect::new(inner.x, times.bottom() + space::SM, inner.w, CONTROLS);
        let centre = controls.x + controls.w / 2.0;
        let mid_y = controls.y + controls.h / 2.0;
        let around = |x: f32, d: f32| Rect::new(x - d / 2.0, mid_y - d / 2.0, d, d);
        let apart = PLAY / 2.0 + space::LG + BUTTON / 2.0;

        let count = self.players.len();
        let dots = if count > 1 {
            let (size, gap) = (6.0, space::SM);
            let total = count as f32 * size + (count - 1) as f32 * gap;
            let y = controls.bottom() + (DOTS - size) / 2.0;
            (0..count)
                .map(|i| {
                    // A little larger to hit than to see.
                    Rect::new(
                        centre - total / 2.0 + i as f32 * (size + gap),
                        y,
                        size,
                        size,
                    )
                    .inset(-3.0)
                })
                .collect()
        } else {
            Vec::new()
        };
        Layout {
            cover,
            title,
            byline,
            track,
            times,
            previous: around(centre - apart, BUTTON),
            play: around(centre, PLAY),
            next: around(centre + apart, BUTTON),
            dots,
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let card = self.card_rect(self.bounds);
        if !card.contains(x, y) || self.players.is_empty() {
            return None;
        }
        let layout = self.layout(card);
        let track = layout.track.inset_xy(0.0, -4.0);
        [
            (layout.previous, Hit::Previous),
            (layout.play, Hit::PlayPause),
            (layout.next, Hit::Next),
            (track, Hit::Track),
        ]
        .into_iter()
        .chain(
            layout
                .dots
                .iter()
                .enumerate()
                .map(|(i, rect)| (*rect, Hit::Player(i))),
        )
        .find(|(rect, _)| rect.contains(x, y))
        .map(|(_, hit)| hit)
    }

    // ---- Doing ---------------------------------------------------------------

    fn play_pause(&mut self) {
        let Some(player) = self.current() else { return };
        let bus = player.bus.clone();
        // Answer at once; the player confirms shortly.
        let target = if player.status == Status::Playing {
            0.0
        } else {
            1.0
        };
        self.playing.set(
            target,
            self.ctx.motion.apply(Transition::SCENE),
            Instant::now(),
        );
        if let Some(player) = self.players.iter_mut().find(|player| player.bus == bus) {
            let now = Instant::now();
            player.position = player.position_at(now);
            player.read = now;
            player.status = if target > 0.5 {
                Status::Playing
            } else {
                Status::Paused
            };
        }
        self.worker.send(Request::PlayPause(bus));
    }

    fn skip(&mut self, forward: bool) {
        let Some(player) = self.current() else { return };
        let can = if forward {
            player.can_next
        } else {
            player.can_previous
        };
        if can {
            let bus = player.bus.clone();
            self.worker.send(if forward {
                Request::Next(bus)
            } else {
                Request::Previous(bus)
            });
        }
    }

    /// Move to `fraction` of the way through the track.
    fn seek_to(&mut self, fraction: f32) {
        let Some(player) = self.current() else { return };
        let Some(length) = player.track.length.filter(|_| player.seekable()) else {
            return;
        };
        let to = length.mul_f32(fraction.clamp(0.0, 1.0));
        let request = Request::Seek(Box::new(player.clone()), to);
        let bus = player.bus.clone();
        if let Some(player) = self.players.iter_mut().find(|player| player.bus == bus) {
            player.position = to;
            player.read = Instant::now();
        }
        self.worker.send(request);
    }

    /// Move `by` forwards (or back) from where it is.
    fn seek_by(&mut self, forward: bool) {
        let Some(player) = self.current() else { return };
        let Some(length) = player.track.length else {
            return;
        };
        let now = player.position_at(Instant::now());
        let to = if forward {
            (now + STEP).min(length)
        } else {
            now.saturating_sub(STEP)
        };
        self.seek_to(to.as_secs_f32() / length.as_secs_f32().max(f32::EPSILON));
    }

    /// Show the player at `index`, sliding in from its side.
    fn choose(&mut self, index: usize) {
        let from = self.current_index();
        if index == from || index >= self.players.len() {
            return;
        }
        self.slide.arrive(
            if index > from {
                Direction::Forward
            } else {
                Direction::Back
            },
            Instant::now(),
        );
        self.show(index);
    }

    fn show(&mut self, index: usize) {
        if let Some(player) = self.players.get(index) {
            self.chosen = Some(player.bus.clone());
            self.follow_current();
        }
    }

    /// Swiped there: its wave as it stands, since it came in drawn so.
    fn swiped(&mut self, direction: Direction) {
        let Some(index) = self.current_index().checked_add_signed(direction.step()) else {
            return;
        };
        self.show(index);
        let playing = self
            .current()
            .is_some_and(|player| player.status == Status::Playing);
        self.playing.jump(if playing { 1.0 } else { 0.0 });
    }

    /// Whether there's a player before the one shown, and one after.
    fn neighbours(&self) -> (bool, bool) {
        let index = self.current_index();
        (index > 0, index + 1 < self.players.len())
    }

    fn on_key(&mut self, key: KeyEvent) -> bool {
        if !key.mods.is_plain() {
            return false;
        }
        match key.key {
            Key::Escape | Key::Char('q') => return true,
            Key::Char(' ' | 'k') => self.play_pause(),
            Key::Left | Key::Char('h') => self.seek_by(false),
            Key::Right | Key::Char('l') => self.seek_by(true),
            Key::Char('n') => self.skip(true),
            Key::Char('p') => self.skip(false),
            Key::Tab if key.mods.shift && !self.players.is_empty() => {
                let count = self.players.len();
                self.choose((self.current_index() + count - 1) % count);
            }
            Key::Tab if !self.players.is_empty() => {
                self.choose((self.current_index() + 1) % self.players.len());
            }
            _ => {}
        }
        false
    }

    fn hover(&mut self, hit: Option<Hit>) {
        if hit == self.hovered {
            return;
        }
        self.hovered = hit;
        let now = Instant::now();
        let transition = self.ctx.motion.apply(Transition::HOVER);
        for (i, which) in [Hit::Previous, Hit::PlayPause, Hit::Next]
            .iter()
            .enumerate()
        {
            self.glow[i].set(if hit == Some(*which) { 1.0 } else { 0.0 }, transition, now);
        }
    }

    fn hold(&mut self, fraction: Option<f32>) {
        let held = fraction.is_some();
        if held != self.dragging.is_some() {
            self.held.set(
                if held { 1.0 } else { 0.0 },
                self.ctx.motion.apply(Transition::HOVER),
                Instant::now(),
            );
        }
        self.dragging = fraction;
    }

    // ---- Dropdown lifetime ---------------------------------------------------

    /// A dropdown stays while the pointer is on it or on the clock, or while
    /// the thumb is held.
    fn wanted(&self) -> bool {
        self.pointer_inside || self.owner_hovering || self.dragging.is_some()
    }

    fn watch_pointer(&mut self, cx: &mut Cx<'_>) {
        if self.mode == Mode::Modal || self.presence.is_closing() {
            return;
        }
        if self.wanted() {
            self.leave_at = None;
        } else if self.leave_at.is_none() {
            self.leave_at = Some(Instant::now() + GRACE);
            cx.wake_in(GRACE);
        }
    }

    /// Limit a dropdown's input to its card and the gap to the bar, so the
    /// rest of its surface neither blocks clicks nor counts as hovering.
    fn update_input(&self, cx: &mut Cx<'_>) {
        let Mode::Dropdown(edge) = self.mode else {
            return;
        };
        let card = self.card_rect(self.bounds);
        let region = match edge {
            Edge::Top => Rect::new(card.x, 0.0, card.w, card.bottom()),
            Edge::Bottom => Rect::new(card.x, card.y, card.w, self.bounds.h - card.y),
        };
        cx.set_input_region(Some(region));
    }

    // ---- Painting --------------------------------------------------------------

    fn paint(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let base = self.ctx.config.appearance.font_size;
        self.bounds = canvas.bounds();

        // The card eases in from the edge it hangs off.
        let from = match self.mode {
            Mode::Dropdown(Edge::Bottom) => 1.0,
            _ => -1.0,
        };
        let card = self
            .card_rect(self.bounds)
            .translate(0.0, from * (1.0 - presence) * space::MD);
        let index = self.current_index();
        let offset = self.slide.offset(card.w, now);
        // The player coming in beside the one shown, while swiping.
        let beside = match offset {
            o if o > 0.0 => self.players.get(index + 1),
            o if o < 0.0 => index.checked_sub(1).and_then(|i| self.players.get(i)),
            _ => None,
        }
        .cloned();
        // Warmed by the cover shown, and by the next as it comes in.
        let warm = |cover: Option<&Cover>| match cover {
            Some(cover) => colors.surface.mix(cover.tint, WARMTH),
            None => colors.surface,
        };
        let mut fill = warm(self.cover());
        if let Some(other) = &beside {
            fill = fill.mix(warm(self.cover_of(other)), offset.abs().min(1.0));
        }
        canvas.set_opacity(presence);
        canvas.card(card, fill, colors.highlight_med);

        let Some(player) = self.current().cloned() else {
            let quiet = colors.muted;
            let row = card.inset(PAD);
            let width = canvas.measure(&Text::new("Nothing playing", base, quiet)).w;
            let glyph = base * 1.4;
            let start = row.x + (row.w - glyph - space::SM - width) / 2.0;
            canvas.icon(
                icon::PLAY,
                glyph,
                quiet,
                Rect::new(start, row.y, glyph, row.h),
                Align::Center,
            );
            canvas.text_in(
                &Text::new("Nothing playing", base, quiet),
                Rect::new(start + glyph + space::SM, row.y, width + 1.0, row.h),
                Align::Start,
            );
            canvas.set_opacity(1.0);
            return Paint::when(self.presence.is_animating(now));
        };
        let layout = self.layout(card);

        // The player shown, following the fingers (or arriving, stepped to
        // by a key or a dot), and the one beside it coming in.
        let (arrive, faded) = self.slide.arrival(now);
        let shift = -offset * card.w + arrive;
        canvas.with_clip(card.inset_xy(stroke::HAIRLINE, 0.0), |canvas| {
            canvas.with_opacity(faded, |canvas| {
                self.paint_player(canvas, &layout.shifted(shift), &player, true, now);
            });
            if let Some(other) = &beside {
                let side = shift + offset.signum() * card.w;
                self.paint_player(canvas, &layout.shifted(side), other, false, now);
            }
        });

        // Which player, when there's a choice: the one shown is a longer
        // dot, gliding along with the fingers.
        if let (Some(first), Some(last)) = (layout.dots.first(), layout.dots.last()) {
            let (first, last) = (first.inset(3.0), last.inset(3.0));
            for rect in &layout.dots {
                let dot = rect.inset(3.0);
                canvas.circle(
                    dot.x + dot.w / 2.0,
                    dot.y + dot.h / 2.0,
                    dot.w / 2.0,
                    colors.highlight_high,
                );
            }
            let pitch = if layout.dots.len() > 1 {
                (last.x - first.x) / (layout.dots.len() - 1) as f32
            } else {
                0.0
            };
            let at = (index as f32 + offset).clamp(0.0, (layout.dots.len() - 1) as f32);
            let x = first.x + at * pitch;
            let stretch = offset.abs().min(1.0) * pitch.min(first.w * 2.0);
            canvas.pill(
                Rect::new(x - stretch / 2.0, first.y, first.w + stretch, first.h),
                colors.subtle,
            );
        }
        canvas.set_opacity(1.0);

        // While something plays its wave rolls on: slowly, so a frame now
        // and then; arriving, sliding, and lighting up take every frame.
        let rolling = self.playing.get(now) > 0.0
            || beside
                .as_ref()
                .is_some_and(|other| other.status == Status::Playing);
        Paint::when(
            self.presence.is_animating(now)
                || self.playing.is_animating(now)
                || self.held.is_animating(now)
                || self.slide.is_animating(now)
                || self.glow.iter().any(|glow| glow.is_animating(now)),
        )
        .or_after(rolling, wave::TICK)
    }

    /// One player's part of the card: the cover, the words, the timeline,
    /// the buttons. The one `shown` takes the pointer's glow and the held
    /// thumb; one sliding in beside it is drawn as it stands.
    fn paint_player(
        &self,
        canvas: &mut Canvas<'_>,
        layout: &Layout,
        player: &Player,
        shown: bool,
        now: Instant,
    ) {
        let colors = &self.ctx.theme.colors;
        let accent = self.ctx.accent();
        let base = self.ctx.config.appearance.font_size;
        let small = base * type_scale::SMALL;

        // The cover, or a quiet square where it would be.
        match self.cover_of(player) {
            Some(cover) => canvas.image(&cover.image, layout.cover, radius::CONTROL + 2.0),
            None => {
                canvas.fill(layout.cover, radius::CONTROL + 2.0, colors.highlight_med);
                canvas.icon(
                    icon::PLAY,
                    COVER * 0.4,
                    colors.muted,
                    layout.cover,
                    Align::Center,
                );
            }
        }

        let title = if player.track.title.is_empty() {
            &player.name
        } else {
            &player.track.title
        };
        canvas.text_in(
            &Text::new(title, base, colors.text)
                .medium()
                .max_width(layout.title.w),
            layout.title,
            Align::Start,
        );
        let byline = match player.track.artist.as_str() {
            "" => player.name.clone(),
            artist => format!("{artist}  ·  {}", player.name),
        };
        canvas.text_in(
            &Text::new(&byline, small, colors.subtle).max_width(layout.byline.w),
            layout.byline,
            Align::Start,
        );

        // The timeline.
        let dragging = self.dragging.filter(|_| shown);
        let position = player.position_at(now);
        let fraction = match (dragging, player.track.length) {
            (Some(held), _) => held,
            (None, Some(length)) => position.as_secs_f32() / length.as_secs_f32().max(f32::EPSILON),
            // A live stream: all of it is now.
            (None, None) => 1.0,
        };
        let playing = if shown {
            self.playing.get(now)
        } else if player.status == Status::Playing {
            1.0
        } else {
            0.0
        };
        let phase = now.saturating_duration_since(self.opened).as_secs_f32() * ROLL * TAU;
        wave::draw(
            canvas,
            layout.track,
            fraction,
            Wave::calm(playing, phase),
            wave::Look {
                played: accent,
                rest: colors.highlight_high,
            },
            if shown { self.held.get(now) } else { 0.0 },
        );
        let at = match (dragging, player.track.length) {
            (Some(held), Some(length)) => length.mul_f32(held),
            _ => position,
        };
        canvas.text_in(
            &Text::new(&clock(at), small, colors.muted).tabular(),
            layout.times,
            Align::Start,
        );
        let end = player.track.length.map_or_else(|| "Live".to_owned(), clock);
        canvas.text_in(
            &Text::new(&end, small, colors.muted),
            layout.times,
            Align::End,
        );

        // The buttons.
        let glow = |i: usize| if shown { self.glow[i].get(now) } else { 0.0 };
        let icon_size = (base * 1.6).round();
        for (i, (rect, figure, enabled)) in [
            (layout.previous, icon::PREVIOUS, player.can_previous),
            (layout.next, icon::NEXT, player.can_next),
        ]
        .into_iter()
        .enumerate()
        {
            let glow = glow(if i == 0 { 0 } else { 2 });
            if enabled && glow > 0.0 {
                canvas.circle(
                    rect.x + rect.w / 2.0,
                    rect.y + rect.h / 2.0,
                    rect.w / 2.0,
                    colors.highlight_low.alpha(glow),
                );
            }
            let color = if enabled {
                colors.text
            } else {
                colors.text.alpha(opacity::DISABLED * 0.6)
            };
            canvas.icon(figure, icon_size, color, rect, Align::Center);
        }
        let glow = glow(1);
        let play = layout.play.inset(-1.5 * glow);
        // The one big button: the lightest thing on the card, its glyph cut
        // in the ground's colour. The accent is the wave's — what has
        // played — not the button's.
        canvas.circle(
            play.x + play.w / 2.0,
            play.y + play.h / 2.0,
            play.w / 2.0,
            colors.text.mix(colors.subtle, 0.1 * (1.0 - glow)),
        );
        // Play turns into pause as the wave rises, and back.
        crossfade(
            canvas,
            (icon::PLAY, icon::PAUSE),
            playing,
            icon_size,
            colors.base,
            layout.play,
        );
    }
}

/// Draw `figures.0` fading into `figures.1` as `t` goes from 0 to 1.
fn crossfade(
    canvas: &mut Canvas<'_>,
    figures: (Icon, Icon),
    t: f32,
    size: f32,
    color: Rgba,
    rect: Rect,
) {
    let t = t.clamp(0.0, 1.0);
    for (figure, amount) in [(figures.0, 1.0 - t), (figures.1, t)] {
        if amount > 0.01 {
            canvas.with_opacity(amount, |canvas| {
                canvas.icon(figure, size, color, rect, Align::Center);
            });
        }
    }
}

/// `3:07`, or `1:02:45` for something longer than an hour.
pub fn clock(duration: Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

impl App for Card {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        match self.mode {
            Mode::Modal => SurfaceSpec::overlay(NAME),
            // Hangs off the bar's edge across the middle — under the clock
            // — without the keyboard.
            Mode::Dropdown(edge) => SurfaceSpec {
                width: Self::surface_width().ceil() as u32,
                height: Self::surface_height().ceil() as u32,
                keyboard: Keyboard::None,
                ..SurfaceSpec::new(
                    NAME,
                    Layer::Overlay,
                    match edge {
                        Edge::Top => Anchor::TOP,
                        Edge::Bottom => Anchor::BOTTOM,
                    },
                )
            },
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        if let Some((mpris, inbox)) = self.live.take() {
            let proxy = proxy.clone();
            inbox.serve(mpris, self.ctx.paths.cache_dir.clone(), move |update| {
                proxy.send(Message::Update(update))
            });
        }
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-play: live theme switching unavailable: {err}");
        }
        // Asked to go by the key that opened it, pressed again.
        if let Err(err) = enso_core::ipc::listen(NAME, {
            let proxy = proxy.clone();
            move |word| word != "close" || proxy.send(Message::Dismiss)
        }) {
            eprintln!("enso-play: a second press cannot close this one: {err}");
        }
        if self.mode != Mode::Modal {
            // Without the bar's word, the pointer on the card decides alone.
            self.owner_hovering = follow_owner(proxy);
            self.update_input(cx);
            self.watch_pointer(cx);
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let mut close = false;
        match event {
            Event::Message(Message::Update(Update::Players(players))) => {
                self.set_players(players);
                // A dropdown with nothing left to show goes.
                close = self.players.is_empty() && self.mode != Mode::Modal;
                self.update_input(cx);
                cx.redraw();
            }
            Event::Message(Message::Update(Update::Cover(url, cover))) => {
                self.covers.insert(url, cover);
                cx.redraw();
            }
            Event::Message(Message::Owner(hovering)) => self.owner_hovering = hovering,
            Event::Message(Message::Dismiss) => close = true,
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Wake if self.presence.is_closing() => return cx.exit(),
            Event::Wake => {
                let width = self.card_rect(self.bounds).w;
                if let Some(direction) = self.slide.expire(width, Instant::now()) {
                    self.swiped(direction);
                }
                cx.redraw();
                close = self
                    .leave_at
                    .is_some_and(|leave_at| Instant::now() >= leave_at)
                    && !self.wanted();
            }
            Event::Resized { .. } => {
                let (width, height) = cx.size();
                self.bounds = Rect::new(0.0, 0.0, width as f32, height as f32);
                self.update_input(cx);
                cx.redraw();
            }
            Event::Key(key) => {
                close = self.on_key(key);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Enter { x, y } | PointerEvent::Move { x, y }) => {
                self.pointer_inside = true;
                if self.dragging.is_some() {
                    let track = self.layout(self.card_rect(self.bounds)).track;
                    self.hold(Some(wave::fraction_at(track, x)));
                    cx.redraw();
                } else {
                    self.hover(self.hit(x, y));
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Scroll {
                dx,
                dy,
                wheel,
                stopped,
                ..
            }) if self.players.len() > 1 => {
                let width = self.card_rect(self.bounds).w;
                let now = Instant::now();
                let can = self.neighbours();
                match self.slide.scroll((dx, dy, wheel, stopped), can, width, now) {
                    Swiped::Go(direction) => self.swiped(direction),
                    Swiped::Moved => cx.wake_in(swipe::QUIET),
                    Swiped::Scroll => {}
                }
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Leave) => {
                self.pointer_inside = false;
                self.hover(None);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                match self.hit(x, y) {
                    Some(Hit::PlayPause) => self.play_pause(),
                    Some(Hit::Previous) => self.skip(false),
                    Some(Hit::Next) => self.skip(true),
                    Some(Hit::Track) => {
                        if self.current().is_some_and(Player::seekable) {
                            let track = self.layout(self.card_rect(self.bounds)).track;
                            self.hold(Some(wave::fraction_at(track, x)));
                        }
                    }
                    Some(Hit::Player(i)) => self.choose(i),
                    // A click away from a card opened on purpose dismisses it.
                    None => {
                        close =
                            self.mode == Mode::Modal && !self.card_rect(self.bounds).contains(x, y);
                    }
                }
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Release {
                button: Button::Left,
                ..
            }) => {
                if let Some(fraction) = self.dragging {
                    self.seek_to(fraction);
                    self.hold(None);
                    cx.redraw();
                }
            }
            _ => {}
        }
        if close {
            self.presence.close(cx);
            return;
        }
        self.watch_pointer(cx);
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.paint(canvas, now)
    }
}

/// Follow the bar on standard input: `enter` and `leave` as the pointer
/// comes and goes from the clock, `close` when it wants the card gone. The
/// bar going away counts as leaving. Returns whether anyone is listening.
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

#[cfg(test)]
mod tests;
