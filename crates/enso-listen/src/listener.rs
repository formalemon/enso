//! The listener's window: the cover large, warming the window with its
//! colour; the title and who it's by; the timeline (a wave for what has
//! played, as on the now-playing card); the buttons; and the folder's other
//! tracks below, the one playing marked by its moving bars.
//!
//! It plays the folder through, one after another. The desktop hears of it
//! as a player, so the now-playing card under the bar's clock shows it, and
//! the media keys reach it.

use std::{
    collections::HashMap,
    f32::consts::TAU,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Icon, Rgba, Transition, Tween,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_media::{
    Playback,
    announce::{Announcer, Asked, Playing, Status},
    clock,
};
use enso_ui::{
    Align, App, Button, Canvas, Cursor, Cx, Event, Key, KeyEvent, Paint, PointerEvent, Proxy, Rect,
    SurfaceSpec, Text, WindowSpec,
    swipe::{self, Direction, Slide, Swiped},
    watch::watch_dir,
    widget::{
        Scroll, toast,
        wave::{self, Wave},
    },
};

use crate::{
    cover::{self, Cover},
    queue::{Queue, Track},
};

/// What it listens as, and holds its one-copy lock by.
pub const NAME: &str = "enso-listen";

const PAD: f32 = space::XL;
const COVER_MOST: f32 = 340.0;
const TRACK: f32 = 24.0;
const TIMES: f32 = 16.0;
const CONTROLS: f32 = 56.0;
const PLAY: f32 = 52.0;
const BUTTON: f32 = 40.0;
const ROW: f32 = 40.0;
/// How far a key moves within a track.
const SEEK: Duration = Duration::from_secs(5);
/// Back this far into a track, "previous" starts it again instead.
const RESTART: Duration = Duration::from_secs(3);
/// How much the cover's colour warms the window.
const WARMTH: f32 = 0.10;
/// The wave rolls on slowly: crests a second.
const ROLL: f32 = 0.5;

#[derive(Debug)]
pub enum Message {
    /// What ffprobe read of a track.
    Probed(PathBuf, Result<enso_media::Media, String>),
    Cover(PathBuf, Option<Cover>),
    /// Another `enso-listen` was asked to play this: play it here instead.
    Open(PathBuf),
    Asked(Asked),
    ThemeChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Previous,
    PlayPause,
    Next,
    Timeline,
    Row(usize),
    Cover,
}

#[derive(Clone, Copy, Debug)]
enum Job {
    Probe,
    Cover,
}

/// Where everything sits.
#[derive(Clone, Debug)]
struct Layout {
    cover: Rect,
    title: Rect,
    byline: Rect,
    track: Rect,
    times: Rect,
    previous: Rect,
    play: Rect,
    next: Rect,
    list: Option<Rect>,
}

pub struct Listener {
    ctx: Context,
    queue: Queue,
    playback: Option<Playback>,
    covers: HashMap<PathBuf, Option<Cover>>,
    jobs: Option<mpsc::Sender<(Job, PathBuf)>>,
    asked_covers: Vec<PathBuf>,
    announcer: Option<Announcer>,
    /// What the desktop was last told: whether it played, and which.
    told: Option<(Status, usize)>,
    /// Which play this has been: a new one is a new track to the desktop.
    plays: u64,
    /// Play as soon as the track's been read.
    wants_play: bool,
    waving: Tween<f32>,
    held: Option<f32>,
    glow: [Tween<f32>; 3],
    hover: Option<Hit>,
    scroll: Scroll,
    slide: Slide,
    toast: Option<(String, Instant)>,
    bounds: Rect,
    opened: Instant,
    proxy: Option<Proxy<Message>>,
}

impl Listener {
    pub fn new(ctx: Context, queue: Queue) -> Self {
        Self {
            slide: Slide::new(&ctx.motion),
            ctx,
            queue,
            playback: None,
            covers: HashMap::new(),
            jobs: None,
            asked_covers: Vec::new(),
            announcer: None,
            told: None,
            plays: 0,
            wants_play: true,
            waving: Tween::new(0.0),
            held: None,
            glow: [Tween::new(0.0), Tween::new(0.0), Tween::new(0.0)],
            hover: None,
            scroll: Scroll::default(),
            toast: None,
            bounds: Rect::new(0.0, 0.0, 440.0, 720.0),
            opened: Instant::now(),
            proxy: None,
        }
    }

    fn transition(&self, transition: Transition) -> Transition {
        self.ctx.motion.apply(transition)
    }

    fn cover_of(&self, path: &Path) -> Option<&Cover> {
        self.covers.get(path)?.as_ref()
    }

    fn playing(&self) -> bool {
        self.playback.as_ref().is_some_and(Playback::is_playing)
    }

    // ---- Playing -----------------------------------------------------------

    /// Ready the current track, playing it if `play`.
    fn start(&mut self, play: bool) {
        self.playback = None;
        self.held = None;
        self.wants_play = play;
        let Some(track) = self.queue.current().cloned() else {
            return;
        };
        self.want_covers();
        let Some(media) = track.media else {
            // It plays once it's been read.
            self.ask(Job::Probe, track.path);
            return;
        };
        let mut playback = Playback::open(&track.path, media, (0, 0));
        if play {
            playback.play(Instant::now());
        }
        self.plays += 1;
        self.playback = Some(playback);
        self.wants_play = false;
    }

    fn go(&mut self, index: usize, play: bool, arrive: bool) {
        let from = self.queue.at;
        if !self.queue.go(index) {
            return;
        }
        if arrive {
            let direction = if index > from {
                Direction::Forward
            } else {
                Direction::Back
            };
            self.slide.arrive(direction, Instant::now());
        }
        self.start(play);
        self.reveal_current();
    }

    fn skip(&mut self, forward: bool) {
        let now = Instant::now();
        // Back, a little way in: from its start again.
        if !forward
            && let Some(playback) = &mut self.playback
            && playback.position(now) > RESTART
        {
            playback.seek(Duration::ZERO, now);
            self.told = None;
            return;
        }
        let playing = self.playing() || self.playback.is_none();
        let to = if forward {
            self.queue.at + 1
        } else {
            self.queue.at.saturating_sub(1)
        };
        if to < self.queue.len() {
            self.go(to, playing, true);
        }
    }

    fn toggle(&mut self) {
        let now = Instant::now();
        match &mut self.playback {
            Some(playback) => playback.toggle(now),
            None => self.start(true),
        }
    }

    fn seek_by(&mut self, forward: bool) {
        let now = Instant::now();
        if let Some(playback) = &mut self.playback {
            let at = playback.position(now);
            let to = if forward {
                at + SEEK
            } else {
                at.saturating_sub(SEEK)
            };
            playback.seek(to, now);
            if let Some(announcer) = &self.announcer {
                announcer.seeked(to);
            }
        }
    }

    fn seek_to(&mut self, fraction: f32) {
        let now = Instant::now();
        if let Some(playback) = &mut self.playback
            && let Some(length) = playback.media().duration
        {
            let to = length.mul_f32(fraction.clamp(0.0, 1.0));
            playback.seek(to, now);
            if let Some(announcer) = &self.announcer {
                announcer.seeked(to);
            }
        }
    }

    fn toggle_mute(&mut self) {
        if let Some(playback) = &mut self.playback {
            let muted = !playback.is_muted();
            playback.set_muted(muted);
            self.toast = Some((
                if muted { "Sound off" } else { "Sound on" }.to_owned(),
                Instant::now(),
            ));
        }
    }

    /// The track played out: on to the next, or stop at the last.
    fn follow_on(&mut self) {
        if self.playback.as_ref().is_some_and(Playback::has_ended) && self.queue.neighbours().1 {
            let next = self.queue.at + 1;
            self.go(next, true, true);
        }
    }

    fn open(&mut self, path: &Path) {
        match self.queue.index_of(path) {
            Some(index) if index == self.queue.at => {
                if let Some(playback) = &mut self.playback {
                    playback.play(Instant::now());
                }
            }
            Some(index) => self.go(index, true, true),
            None => {
                self.queue = Queue::open(path);
                self.scroll = Scroll::default();
                self.probe_all();
                self.start(true);
            }
        }
    }

    fn asked(&mut self, asked: Asked, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match asked {
            Asked::PlayPause => self.toggle(),
            Asked::Play => match &mut self.playback {
                Some(playback) => playback.play(now),
                None => self.start(true),
            },
            Asked::Pause | Asked::Stop => {
                if let Some(playback) = &mut self.playback {
                    playback.pause(now);
                }
            }
            Asked::Next => self.skip(true),
            Asked::Previous => self.skip(false),
            Asked::Seek(micros) => {
                if let Some(playback) = &mut self.playback {
                    let at = playback.position(now);
                    let by = Duration::from_micros(micros.unsigned_abs());
                    let to = if micros >= 0 {
                        at + by
                    } else {
                        at.saturating_sub(by)
                    };
                    playback.seek(to, now);
                    if let Some(announcer) = &self.announcer {
                        announcer.seeked(to);
                    }
                }
            }
            Asked::SetPosition(to) => {
                if let Some(playback) = &mut self.playback {
                    playback.seek(to, now);
                    if let Some(announcer) = &self.announcer {
                        announcer.seeked(to);
                    }
                }
            }
            Asked::Raise => {}
            Asked::Quit => cx.exit(),
        }
    }

    // ---- Reading -----------------------------------------------------------

    fn ask(&self, job: Job, path: PathBuf) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send((job, path));
        }
    }

    /// Read every track (the current one first): titles, lengths.
    fn probe_all(&mut self) {
        let at = self.queue.at;
        let mut order: Vec<usize> = (0..self.queue.len()).collect();
        order.sort_by_key(|index| index.abs_diff(at));
        for index in order {
            if self.queue.tracks[index].media.is_none() {
                self.ask(Job::Probe, self.queue.tracks[index].path.clone());
            }
        }
    }

    /// Covers for the one playing and those either side.
    fn want_covers(&mut self) {
        let around: Vec<PathBuf> = [0isize, 1, -1]
            .into_iter()
            .filter_map(|step| self.queue.at.checked_add_signed(step))
            .filter_map(|index| self.queue.tracks.get(index))
            .map(|track| track.path.clone())
            .collect();
        for path in around {
            if !self.covers.contains_key(&path) && !self.asked_covers.contains(&path) {
                self.asked_covers.push(path.clone());
                self.ask(Job::Cover, path);
            }
        }
    }

    /// Tell the desktop how it stands, if that changed.
    fn announce(&mut self) {
        let Some(playback) = &self.playback else {
            return;
        };
        let status = if playback.is_playing() {
            Status::Playing
        } else if playback.has_ended() {
            Status::Stopped
        } else {
            Status::Paused
        };
        if self.told == Some((status, self.queue.at)) {
            return;
        }
        if self.announcer.is_none() {
            let Some(proxy) = self.proxy.clone() else {
                return;
            };
            match Announcer::start("enso_listen", "Listen", NAME, move |asked| {
                proxy.send(Message::Asked(asked));
            }) {
                Ok(announcer) => self.announcer = Some(announcer),
                Err(err) => {
                    eprintln!("enso-listen: {err}");
                    self.told = Some((status, self.queue.at));
                    return;
                }
            }
        }
        self.told = Some((status, self.queue.at));
        let Some(track) = self.queue.current() else {
            return;
        };
        let (_, can_next) = self.queue.neighbours();
        let art = self
            .cover_of(&track.path)
            .and_then(|cover| cover.file.as_ref())
            .map(|file| enso_fs::uri::from_path(file));
        let playing = Playing {
            title: track.title(),
            artist: track.artist(),
            album: track.album(),
            art,
            length: track.length(),
            position: playback.position(Instant::now()),
            status,
            can_next,
            // Previous always does something: the one before, or this one
            // from its start.
            can_previous: true,
            track: self.plays,
        };
        if let Some(announcer) = &self.announcer {
            announcer.tell(playing);
        }
    }

    // ---- Layout ------------------------------------------------------------

    fn layout(&self, bounds: Rect) -> Layout {
        let size = self.ctx.config.appearance.font_size;
        let line = size * type_scale::LINE_HEIGHT;
        let inner = bounds.inset_xy(PAD, 0.0);
        let side = inner
            .w
            .min(COVER_MOST)
            .min(bounds.h * 0.42)
            .max(96.0)
            .round();
        let cover = Rect::new(
            (bounds.x + (bounds.w - side) / 2.0).round(),
            bounds.y + PAD,
            side,
            side,
        );
        let title = Rect::new(
            inner.x,
            cover.bottom() + space::LG,
            inner.w,
            (line * type_scale::HEADING).ceil(),
        );
        let byline = Rect::new(inner.x, title.bottom(), inner.w, line.ceil());
        let track = Rect::new(inner.x, byline.bottom() + space::LG, inner.w, TRACK);
        let times = Rect::new(inner.x, track.bottom(), inner.w, TIMES);
        let controls = Rect::new(inner.x, times.bottom() + space::SM, inner.w, CONTROLS);
        let centre = controls.x + controls.w / 2.0;
        let middle = controls.y + controls.h / 2.0;
        let around = |x: f32, d: f32| Rect::new(x - d / 2.0, middle - d / 2.0, d, d);
        let apart = PLAY / 2.0 + space::XL + BUTTON / 2.0;
        let top = controls.bottom() + space::MD;
        let list = (self.queue.len() > 1 && bounds.bottom() - top > ROW)
            .then(|| Rect::new(bounds.x, top, bounds.w, bounds.bottom() - top));
        Layout {
            cover,
            title,
            byline,
            track,
            times,
            previous: around(centre - apart, BUTTON),
            play: around(centre, PLAY),
            next: around(centre + apart, BUTTON),
            list,
        }
    }

    fn hit(&self, x: f32, y: f32, now: Instant) -> Option<Hit> {
        let layout = self.layout(self.bounds);
        let buttons = [
            (layout.previous, Hit::Previous),
            (layout.play, Hit::PlayPause),
            (layout.next, Hit::Next),
            (layout.track.inset_xy(0.0, -6.0), Hit::Timeline),
            (layout.cover, Hit::Cover),
        ];
        if let Some((_, hit)) = buttons.iter().find(|(rect, _)| rect.contains(x, y)) {
            return Some(*hit);
        }
        let list = layout.list?;
        if !list.contains(x, y) {
            return None;
        }
        let row = ((y - list.y - space::SM + self.scroll.get(now)) / ROW).floor();
        (row >= 0.0 && (row as usize) < self.queue.len()).then_some(Hit::Row(row as usize))
    }

    fn reveal_current(&mut self) {
        let now = Instant::now();
        let layout = self.layout(self.bounds);
        if let Some(list) = layout.list {
            let view = list.h - 2.0 * space::SM;
            self.scroll.set_extent(self.queue.len() as f32 * ROW, view);
            let transition = self.transition(Transition::SELECT);
            self.scroll
                .reveal(self.queue.at as f32 * ROW, ROW, transition, now);
        }
    }

    fn hover(&mut self, hit: Option<Hit>) {
        if hit == self.hover {
            return;
        }
        self.hover = hit;
        let now = Instant::now();
        let transition = self.transition(Transition::HOVER);
        for (i, which) in [Hit::Previous, Hit::PlayPause, Hit::Next]
            .iter()
            .enumerate()
        {
            self.glow[i].set(if hit == Some(*which) { 1.0 } else { 0.0 }, transition, now);
        }
    }

    // ---- Keys and the pointer ----------------------------------------------

    fn on_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>) {
        if !key.mods.is_plain() {
            return;
        }
        match key.key {
            Key::Escape | Key::Char('q') => cx.exit(),
            Key::Char(' ' | 'k') => self.toggle(),
            Key::Left | Key::Char('h') => self.seek_by(false),
            Key::Right | Key::Char('l') => self.seek_by(true),
            Key::Char('n') | Key::PageDown => self.skip(true),
            Key::Char('p') | Key::PageUp => self.skip(false),
            Key::Down | Key::Char('j') => self.skip(true),
            Key::Up => self.skip(false),
            Key::Home => self.go(0, true, true),
            Key::End => self.go(self.queue.len().saturating_sub(1), true, true),
            Key::Char('m') => self.toggle_mute(),
            Key::Char('e') => self.reveal(),
            _ => return,
        }
        cx.redraw();
    }

    fn reveal(&mut self) {
        let Some(track) = self.queue.current() else {
            return;
        };
        let argv = vec![
            enso_core::exe::companion("enso-files")
                .to_string_lossy()
                .into_owned(),
            "--select".to_owned(),
            track.path.to_string_lossy().into_owned(),
        ];
        if let Err(err) = enso_fs::apps::spawn_detached(&argv, None) {
            self.toast = Some((format!("Can't open Files: {err}"), Instant::now()));
        }
    }

    fn on_pointer(&mut self, event: PointerEvent, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                if self.held.is_some() {
                    let track = self.layout(self.bounds).track;
                    self.held = Some(wave::fraction_at(track, x));
                } else {
                    let hit = self.hit(x, y, now);
                    self.hover(hit);
                    cx.set_cursor(match hit {
                        Some(Hit::Cover) | None => Cursor::Default,
                        Some(_) => Cursor::Pointer,
                    });
                }
                cx.redraw();
            }
            PointerEvent::Leave => {
                self.hover(None);
                cx.redraw();
            }
            PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            } => {
                match self.hit(x, y, now) {
                    Some(Hit::PlayPause | Hit::Cover) => self.toggle(),
                    Some(Hit::Previous) => self.skip(false),
                    Some(Hit::Next) => self.skip(true),
                    Some(Hit::Timeline) => {
                        if self.playback.is_some() {
                            let track = self.layout(self.bounds).track;
                            self.held = Some(wave::fraction_at(track, x));
                        }
                    }
                    Some(Hit::Row(index)) if index == self.queue.at => self.toggle(),
                    Some(Hit::Row(index)) => self.go(index, true, true),
                    None => {}
                }
                cx.redraw();
            }
            PointerEvent::Release {
                button: Button::Left,
                ..
            } => {
                if let Some(held) = self.held.take() {
                    self.seek_to(held);
                    cx.redraw();
                }
            }
            PointerEvent::Scroll {
                x,
                y,
                dx,
                dy,
                wheel,
                stopped,
            } => {
                let layout = self.layout(self.bounds);
                let over_list = layout.list.is_some_and(|list| list.contains(x, y));
                if !over_list {
                    let can = self.queue.neighbours();
                    let width = self.bounds.w;
                    match self.slide.scroll((dx, dy, wheel, stopped), can, width, now) {
                        Swiped::Go(direction) => {
                            let to = self.queue.at.saturating_add_signed(direction.step());
                            let playing = self.playing();
                            if self.queue.go(to) {
                                self.start(playing);
                                self.reveal_current();
                            }
                        }
                        Swiped::Moved => cx.wake_in(swipe::QUIET),
                        Swiped::Scroll => {}
                    }
                } else {
                    let transition = self.transition(Transition::SELECT);
                    self.scroll.scroll(dy, wheel, ROW * 2.0, transition, now);
                }
                cx.redraw();
            }
            _ => {}
        }
    }

    // ---- Painting ----------------------------------------------------------

    fn paint(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        self.bounds = canvas.bounds();
        let bounds = self.bounds;
        let layout = self.layout(bounds);

        // The window, warmed by the cover's colour, and the next's as it
        // comes in.
        let offset = self.slide.offset(bounds.w, now);
        let beside = match offset {
            o if o > 0.0 => self.queue.tracks.get(self.queue.at + 1),
            o if o < 0.0 => self
                .queue
                .at
                .checked_sub(1)
                .and_then(|index| self.queue.tracks.get(index)),
            _ => None,
        }
        .cloned();
        let warm = |cover: Option<&Cover>| match cover {
            Some(cover) => colors.base.mix(cover.tint, WARMTH),
            None => colors.base,
        };
        let current = self.queue.current().cloned();
        let mut ground = warm(
            current
                .as_ref()
                .and_then(|track| self.cover_of(&track.path)),
        );
        if let Some(other) = &beside {
            ground = ground.mix(warm(self.cover_of(&other.path)), offset.abs().min(1.0));
        }
        canvas.clear(ground);

        let Some(track) = current else {
            canvas.text_in(
                &Text::new("Nothing to listen to here", size, colors.muted),
                bounds,
                Align::Center,
            );
            return Paint::Idle;
        };

        // The cover and the words, following the fingers; the next beside.
        let (arrive, faded) = self.slide.arrival(now);
        let shift = -offset * bounds.w + arrive;
        canvas.with_opacity(faded, |canvas| {
            self.paint_face(canvas, &layout, &track, shift);
        });
        if let Some(other) = &beside {
            self.paint_face(canvas, &layout, other, shift + offset.signum() * bounds.w);
        }

        // The timeline.
        let playing = self.playing();
        let target = if playing { 1.0 } else { 0.0 };
        if self.waving.target() != target {
            self.waving
                .set(target, self.transition(Transition::SCENE), now);
        }
        let length = track.length();
        let position = self
            .playback
            .as_ref()
            .map_or(Duration::ZERO, |playback| playback.position(now));
        let fraction = match (self.held, length) {
            (Some(held), _) => held,
            (None, Some(length)) if !length.is_zero() => {
                position.as_secs_f32() / length.as_secs_f32()
            }
            _ => 0.0,
        };
        let phase = now.saturating_duration_since(self.opened).as_secs_f32() * ROLL * TAU;
        wave::draw(
            canvas,
            layout.track,
            fraction,
            Wave::calm(self.waving.get(now), phase),
            wave::Look {
                played: accent,
                rest: colors.highlight_high,
            },
            if self.held.is_some() { 1.0 } else { 0.0 },
        );
        let at = match (self.held, length) {
            (Some(held), Some(length)) => length.mul_f32(held),
            _ => position,
        };
        canvas.text_in(
            &Text::new(&clock(at), small, colors.muted).tabular(),
            layout.times,
            Align::Start,
        );
        let end = length.map_or_else(String::new, clock);
        canvas.text_in(
            &Text::new(&end, small, colors.muted).tabular(),
            layout.times,
            Align::End,
        );

        // The buttons, as the now-playing card has them.
        let (before, after) = self.queue.neighbours();
        let icon_size = (size * 1.7).round();
        for (i, (rect, figure, enabled)) in [
            (layout.previous, icon::PREVIOUS, true),
            (layout.next, icon::NEXT, after),
        ]
        .into_iter()
        .enumerate()
        {
            let glow = self.glow[if i == 0 { 0 } else { 2 }].get(now);
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
        let _ = before;
        let glow = self.glow[1].get(now);
        let play = layout.play.inset(-1.5 * glow);
        canvas.circle(
            play.x + play.w / 2.0,
            play.y + play.h / 2.0,
            play.w / 2.0,
            colors.text.mix(colors.subtle, 0.1 * (1.0 - glow)),
        );
        let waving = self.waving.get(now);
        crossfade(
            canvas,
            (icon::PLAY, icon::PAUSE),
            waving,
            icon_size,
            colors.base,
            layout.play,
        );

        let mut animating = self.waving.is_animating(now)
            || self.slide.is_animating(now)
            || self.scroll.is_animating(now)
            || self.glow.iter().any(|glow| glow.is_animating(now));
        if let Some(list) = layout.list {
            animating |= self.paint_list(canvas, list, now);
        }
        if let Some((text, since)) = &self.toast
            && since.elapsed() < toast::STAYS
        {
            let look = enso_ui::widget::Look::new(&colors, accent, size);
            toast::draw(canvas, text, bounds.bottom(), &look);
        }
        // While it plays the wave rolls on: slowly, so a frame now and then.
        Paint::when(animating).or_after(playing, wave::TICK)
    }

    /// A track's cover and its words, moved `shift` across.
    fn paint_face(&self, canvas: &mut Canvas<'_>, layout: &Layout, track: &Track, shift: f32) {
        let colors = &self.ctx.theme.colors;
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        let cover = layout.cover.translate(shift, 0.0);
        let found = self.cover_of(&track.path);
        // A picture without see-through parts, or the plain face, covers
        // its square: the shadow need only go round it.
        if found.map_or(colors.surface.a == u8::MAX, |found| found.image.is_opaque()) {
            canvas.lift_opaque(cover, radius::CARD, 1.0);
        } else {
            canvas.lift(cover, radius::CARD, 1.0);
        }
        match found {
            Some(found) => canvas.image(&found.image, cover, radius::CARD),
            None => {
                canvas.fill(cover, radius::CARD, colors.surface);
                canvas.grain(cover, radius::CARD);
                canvas.icon(
                    icon::NOTE,
                    (cover.w * 0.32).round(),
                    colors.muted,
                    cover,
                    Align::Center,
                );
            }
        }
        canvas.rim(cover, radius::CARD, colors.highlight_med.alpha(0.6));
        let title = track.title();
        canvas.text_in(
            &Text::new(&title, size * type_scale::HEADING, colors.text)
                .medium()
                .max_width(layout.title.w),
            layout.title.translate(shift, 0.0),
            Align::Center,
        );
        let byline = match (track.artist(), track.album()) {
            (Some(artist), Some(album)) => format!("{artist}  ·  {album}"),
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => String::new(),
        };
        canvas.text_in(
            &Text::new(&byline, small, colors.subtle).max_width(layout.byline.w),
            layout.byline.translate(shift, 0.0),
            Align::Center,
        );
    }

    /// The folder's tracks. Returns whether anything in it moves.
    fn paint_list(&mut self, canvas: &mut Canvas<'_>, list: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        canvas.fill(
            Rect::new(list.x + PAD, list.y, list.w - 2.0 * PAD, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let view = Rect::new(list.x, list.y + space::SM, list.w, list.h - space::SM);
        self.scroll
            .set_extent(self.queue.len() as f32 * ROW + space::SM, view.h);
        let top = self.scroll.get(now);
        let playing = self.playing();
        let time = now.saturating_duration_since(self.opened).as_secs_f32();
        canvas.with_clip(view, |canvas| {
            let first = (top / ROW).floor().max(0.0) as usize;
            let last = (((top + view.h) / ROW).ceil() as usize + 1).min(self.queue.len());
            for index in first..last {
                let row = Rect::new(view.x, view.y + index as f32 * ROW - top, view.w, ROW);
                let band = row.inset_xy(space::MD, 2.0);
                let track = &self.queue.tracks[index];
                let current = index == self.queue.at;
                if current {
                    canvas.fill(band, radius::CONTROL, accent.alpha(opacity::TINT));
                } else if self.hover == Some(Hit::Row(index)) {
                    canvas.fill(band, radius::CONTROL, colors.highlight_low);
                }
                let number = Rect::new(band.x + space::SM, band.y, 28.0, band.h);
                if current {
                    bars(
                        canvas,
                        number,
                        accent,
                        if playing { Some(time) } else { None },
                    );
                } else {
                    canvas.text_in(
                        &Text::new(&(index + 1).to_string(), small, colors.muted).tabular(),
                        number,
                        Align::Center,
                    );
                }
                let length = track.length().map_or_else(String::new, clock);
                let length_width = 56.0;
                let words = Rect::new(
                    number.right() + space::SM,
                    band.y,
                    band.right() - number.right() - space::SM - length_width - space::SM,
                    band.h,
                );
                let name = track.title();
                let title = Text::new(&name, size, colors.text).max_width(words.w);
                let title = if current { title.medium() } else { title };
                canvas.text_in(&title, words, Align::Start);
                canvas.text_in(
                    &Text::new(&length, small, colors.muted).tabular(),
                    Rect::new(
                        band.right() - length_width - space::SM,
                        band.y,
                        length_width,
                        band.h,
                    ),
                    Align::End,
                );
            }
        });
        playing
    }
}

/// The one playing's mark: three small bars, rising and falling while it
/// plays (`time`), resting low while it doesn't.
fn bars(canvas: &mut Canvas<'_>, area: Rect, color: Rgba, time: Option<f32>) {
    let (width, gap, tall) = (3.0, 2.5, 12.0);
    let total = 3.0 * width + 2.0 * gap;
    let x = area.x + (area.w - total) / 2.0;
    let bottom = area.y + area.h / 2.0 + tall / 2.0;
    for i in 0..3 {
        let height = match time {
            Some(time) => {
                let wobble = (time * (4.1 + i as f32 * 1.3) + i as f32 * 1.7).sin();
                tall * (0.45 + 0.4 * wobble.abs() + 0.15 * (time * 2.3 + i as f32).sin())
            }
            None => tall * [0.45, 0.7, 0.55][i],
        };
        let height = height.clamp(width, tall);
        canvas.fill(
            Rect::new(x + i as f32 * (width + gap), bottom - height, width, height),
            width / 2.0,
            color,
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

impl App for Listener {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::window(
            WindowSpec {
                app_id: NAME.to_owned(),
                title: self
                    .queue
                    .current()
                    .map_or_else(|| "Listen".to_owned(), Track::title),
                min_size: (340, 460),
                ..WindowSpec::default()
            },
            440,
            720,
        )
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let (jobs, inbox) = mpsc::channel::<(Job, PathBuf)>();
        let worker = proxy.clone();
        let cache = self.ctx.paths.cache_dir.clone();
        let spawned = thread::Builder::new()
            .name("enso-listen-read".to_owned())
            .spawn(move || {
                for (job, path) in inbox {
                    let sent = match job {
                        Job::Probe => {
                            let media = enso_media::probe(&path).map_err(|err| err.to_string());
                            worker.send(Message::Probed(path, media))
                        }
                        Job::Cover => {
                            let found = cover::find(&path, &cache);
                            worker.send(Message::Cover(path, found))
                        }
                    };
                    if !sent {
                        break;
                    }
                }
            });
        if spawned.is_ok() {
            self.jobs = Some(jobs);
        }
        if let Err(err) = enso_core::ipc::listen(NAME, {
            let proxy = proxy.clone();
            move |word| match word.strip_prefix("open ") {
                Some(path) => proxy.send(Message::Open(PathBuf::from(path))),
                None => true,
            }
        }) {
            eprintln!("enso-listen: another can't hand its songs to this one: {err}");
        }
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-listen: live theme switching unavailable: {err}");
        }
        self.proxy = Some(proxy);
        if !enso_media::available() {
            self.toast = Some((
                "Playing needs ffmpeg, which isn't installed".to_owned(),
                Instant::now(),
            ));
        }
        self.start(true);
        self.probe_all();
        cx.redraw();
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Probed(path, media)) => {
                match media {
                    Ok(media) => {
                        self.queue.learn(&path, media);
                        let current = self.queue.current().is_some_and(|track| track.path == path);
                        if current && self.playback.is_none() {
                            let play = self.wants_play;
                            self.start(play);
                        }
                    }
                    Err(err) => {
                        if self.queue.current().is_some_and(|track| track.path == path) {
                            self.toast = Some((format!("Can't play it: {err}"), Instant::now()));
                        }
                    }
                }
                cx.redraw();
            }
            Event::Message(Message::Cover(path, found)) => {
                self.asked_covers.retain(|asked| *asked != path);
                self.covers.insert(path, found);
                // The card's cover changes with it.
                self.told = None;
                cx.redraw();
            }
            Event::Message(Message::Open(path)) => {
                self.open(&path);
                cx.redraw();
            }
            Event::Message(Message::Asked(asked)) => {
                self.asked(asked, cx);
                cx.redraw();
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Key(key) => self.on_key(key, cx),
            Event::Pointer(pointer) => self.on_pointer(pointer, cx),
            Event::Resized { .. } => {
                self.reveal_current();
                cx.redraw();
            }
            Event::Close => cx.exit(),
            Event::Wake => {
                let now = Instant::now();
                if let Some(direction) = self.slide.expire(self.bounds.w, now) {
                    let to = self.queue.at.saturating_add_signed(direction.step());
                    let playing = self.playing();
                    if self.queue.go(to) {
                        self.start(playing);
                        self.reveal_current();
                    }
                }
                if let Some(playback) = &mut self.playback {
                    playback.frame(now);
                }
                self.follow_on();
                cx.redraw();
            }
            _ => {}
        }
        // Played out, or nothing left to do: look again in a moment.
        if let Some(playback) = &mut self.playback {
            playback.frame(Instant::now());
            if let Some(due) = playback.due(Instant::now()) {
                cx.wake_in(due);
            }
        }
        self.follow_on();
        self.announce();
        if let Some(track) = self.queue.current() {
            cx.set_title(&track.title());
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.paint(canvas, now)
    }
}

#[cfg(test)]
mod tests;
