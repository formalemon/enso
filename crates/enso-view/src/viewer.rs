//! The viewer's window, made as the file manager's is: a title row with
//! back and forward, what's shown and where it sits among the rest, and
//! its buttons; the picture (or the video) on the window's ground; a strip
//! of the others along the foot, to see where you are and jump; a status
//! line with its details; and, with `i`, a pane of everything known about
//! it. Full-screen, all of that goes, and the picture has the screen.
//!
//! Two fingers sideways step through them: the one shown follows the
//! fingers, the next coming in beside it. A video plays where it is, its
//! controls floating over its foot while the pointer moves.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use enso_core::{Context, Transition, Tween, style::space};
use enso_media::announce::Asked;
use enso_ui::{
    App, Button, Canvas, Clicks, Clip, Cursor, Cx, Event, Image, Key, KeyEvent, Paint,
    PointerEvent, Proxy, Rect, SurfaceSpec, WindowSpec,
    swipe::{self, Direction, Slide, Swiped},
    watch::watch_dir,
    widget::{Look, Tooltip},
};

use crate::{
    album::Album,
    item::{self, Item, Thumbnailer},
    photo::Photo,
    video::{self, Video},
};

/// The title row, the status line, and the details pane: the file
/// manager's own sizes.
pub(crate) const HEADER: f32 = 52.0;
pub(crate) const STATUS: f32 = 32.0;
pub(crate) const PANE: f32 = 320.0;
/// The window needs this width before the details pane fits beside it.
pub(crate) const PANE_FROM: f32 = 720.0;
/// The strip of the others along the foot.
pub(crate) const STRIP: f32 = 76.0;
pub(crate) const TILE: f32 = 48.0;
pub(crate) const TILE_GAP: f32 = 8.0;
/// How long the controls over a video stay after the pointer stops.
const LINGER: Duration = Duration::from_millis(1800);
/// One zoom step, by key or by a wheel's notch.
const STEP: f32 = 1.25;
/// The most a picture is enlarged: this many screen pixels to one of its own.
const MOST: f32 = 32.0;
/// How far a key moves within a video.
const SEEK: Duration = Duration::from_secs(5);
/// Thumbnails asked for either side of the one shown.
const STRIP_REACH: usize = 14;
/// A new size for a video's pictures waits until the window has been this
/// still: a resize being dragged doesn't restart them at every step.
pub(crate) const RESIZE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum Message {
    /// A picture or a video was read, or couldn't be.
    Read(PathBuf, Result<Arc<Item>, String>),
    /// A thumbnail for the strip.
    Thumb(PathBuf, Option<Image>),
    /// The desktop asked something of the video playing.
    Asked(Asked),
    ThemeChanged,
}

/// How a picture is shown: fitted, or so many output pixels to each of its
/// own, around a point of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub zoom: Option<f32>,
    /// The picture's point at the stage's middle, in its own pixels.
    pub centre: Option<(f32, f32)>,
}

impl View {
    pub const FIT: Self = Self {
        zoom: None,
        centre: None,
    };
}

/// What answers the pointer, where it was drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Hit {
    Back,
    Forward,
    Turn,
    Info,
    Reveal,
    Trash,
    Zoom,
    Thumb(usize),
    /// The big button over a still video.
    Start,
    PlayPause,
    Timeline,
    Mute,
    /// The controls' bar, between its buttons.
    Controls,
    #[default]
    Stage,
}

impl Hit {
    /// What a button does, for its tooltip.
    pub fn tip(self) -> Option<&'static str> {
        Some(match self {
            Self::Back => "Previous  ←",
            Self::Forward => "Next  →",
            Self::Turn => "Turn  r",
            Self::Info => "Details  i",
            Self::Reveal => "Show in Files  e",
            Self::Trash => "Move to the trash  Delete",
            Self::Mute => "Sound on or off  m",
            _ => return None,
        })
    }
}

pub struct Viewer {
    pub(crate) ctx: Context,
    pub(crate) album: Album,
    pub(crate) read: HashMap<PathBuf, Result<Arc<Item>, String>>,
    reader: Option<mpsc::Sender<PathBuf>>,
    asked: Vec<PathBuf>,
    /// The one on screen, kept while the next is read.
    pub(crate) shown: Option<(PathBuf, Arc<Item>)>,
    pub(crate) view: View,
    /// Where the picture goes, from the last frame: the window less its
    /// title row, strip, status line, and pane.
    pub(crate) stage: Rect,
    pub(crate) scale: f32,
    pub(crate) pointer: Option<(f32, f32)>,
    /// Being dragged about: where the pointer was, and the centre then.
    dragging: Option<((f32, f32), (f32, f32))>,
    clicks: Clicks,
    /// The controls over a video show until then.
    pub(crate) chrome_until: Option<Instant>,
    /// Something said, since when.
    pub(crate) toast: Option<(String, Instant)>,
    /// What was thrown away here, last first, to put back.
    trashed: Vec<(enso_fs::trash::Item, usize)>,
    title: String,
    pub(crate) slide: Slide,
    /// The details pane is open.
    pub(crate) info: bool,
    pub(crate) fullscreen: bool,
    pub(crate) maximized: bool,
    /// The keys are shown.
    pub(crate) help: bool,
    pub(crate) hover: Option<Hit>,
    pub(crate) hits: Vec<(Rect, Hit)>,
    pub(crate) tooltip: Tooltip<Hit>,
    /// When the tooltip waited for is due.
    tooltip_at: Option<Instant>,
    pub(crate) thumbs: HashMap<PathBuf, Option<Image>>,
    thumbnailer: Option<Thumbnailer>,
    /// Where along the strip the one shown is, gliding as it changes.
    pub(crate) strip_at: Tween<f32>,
    pub(crate) video: Option<Video>,
    /// How much the controls over a video show, `0..=1`.
    pub(crate) controls: Tween<f32>,
    /// Play this one as soon as it's on show: what was opened, if a video.
    autoplay: Option<PathBuf>,
    /// The stage changed size: a video decodes at the new one once it has
    /// been still a moment.
    pub(crate) resizing: Option<((u32, u32), Instant)>,
    /// How much the timeline waves: `1` while the video plays.
    pub(crate) waving: Tween<f32>,
    /// When the window opened: the wave rolls from here.
    pub(crate) opened: Instant,
    /// The current file's size and when it last changed, for the details.
    pub(crate) file: Option<(PathBuf, u64, Option<std::time::SystemTime>)>,
    proxy: Option<Proxy<Message>>,
}

impl Viewer {
    pub fn new(ctx: Context, album: Album) -> Self {
        let autoplay = album
            .current()
            .filter(|path| crate::album::is_video(path))
            .cloned();
        let at = album.at as f32;
        Self {
            slide: Slide::new(&ctx.motion),
            ctx,
            album,
            read: HashMap::new(),
            reader: None,
            asked: Vec::new(),
            shown: None,
            view: View::FIT,
            stage: Rect::new(0.0, HEADER, 1100.0, 760.0 - HEADER - STATUS - STRIP),
            scale: 1.0,
            pointer: None,
            dragging: None,
            clicks: Clicks::default(),
            chrome_until: None,
            toast: None,
            trashed: Vec::new(),
            title: String::new(),
            info: false,
            fullscreen: false,
            maximized: false,
            help: false,
            hover: None,
            hits: Vec::new(),
            tooltip: Tooltip::default(),
            tooltip_at: None,
            thumbs: HashMap::new(),
            thumbnailer: None,
            strip_at: Tween::new(at),
            video: None,
            controls: Tween::new(1.0),
            autoplay,
            resizing: None,
            waving: Tween::new(0.0),
            opened: Instant::now(),
            file: None,
            proxy: None,
        }
    }

    pub(crate) fn name(path: &Path) -> String {
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }

    pub(crate) fn look(&self) -> Look {
        Look::new(
            &self.ctx.theme.colors,
            self.ctx.accent(),
            self.ctx.config.appearance.font_size,
        )
    }

    pub(crate) fn transition(&self, transition: Transition) -> Transition {
        self.ctx.motion.apply(transition)
    }

    // ---- What's shown ------------------------------------------------------

    /// Ask for the current one and its neighbours; forget the rest. Ask for
    /// the strip's thumbnails, nearest first.
    fn want(&mut self) {
        let around: Vec<PathBuf> = [0, 1, -1]
            .into_iter()
            .filter_map(|step| self.album.neighbour(step).cloned())
            .collect();
        self.read.retain(|path, _| around.contains(path));
        self.asked.retain(|path| around.contains(path));
        if let Some(reader) = &self.reader {
            for path in around {
                if !self.read.contains_key(&path) && !self.asked.contains(&path) {
                    let _ = reader.send(path.clone());
                    self.asked.push(path);
                }
            }
        }
        if let Some(thumbnailer) = &self.thumbnailer {
            let mut wanted = Vec::new();
            for distance in 0..=STRIP_REACH as isize {
                for step in [distance, -distance] {
                    if let Some(path) = self.album.neighbour(step)
                        && !self.thumbs.contains_key(path)
                        && !wanted.contains(path)
                    {
                        wanted.push(path.clone());
                    }
                }
            }
            thumbnailer.want(wanted);
        }
        self.show_current();
    }

    /// Put the current one on screen, if it has been read; and if it's a
    /// video, ready it to play.
    fn show_current(&mut self) {
        let Some(path) = self.album.current().cloned() else {
            self.shown = None;
            self.video = None;
            return;
        };
        if let Some(Ok(item)) = self.read.get(&path)
            && self.shown.as_ref().is_none_or(|(shown, _)| *shown != path)
        {
            self.shown = Some((path.clone(), Arc::clone(item)));
            self.view = View::FIT;
        }
        if self.video.as_ref().is_some_and(|video| video.path != path) {
            self.video = None;
        }
        if self.video.is_none()
            && let Some(Ok(item)) = self.read.get(&path)
            && let Item::Video { media, .. } = item.as_ref()
            && let Some(own) = &media.video
        {
            let room = self.room();
            let size = video::fit(
                own.width,
                own.height,
                (room.w * self.scale, room.h * self.scale),
            );
            let mut video = Video::open(&path, media.clone(), size);
            if self.autoplay.take().is_some_and(|wanted| wanted == path) {
                video.play(Instant::now());
            }
            self.video = Some(video);
        }
    }

    /// The item on screen, if it's the current one.
    pub(crate) fn item(&self) -> Option<&Arc<Item>> {
        let (path, item) = self.shown.as_ref()?;
        (Some(path) == self.album.current()).then_some(item)
    }

    /// The picture on screen, if a picture is.
    pub(crate) fn photo(&self) -> Option<Arc<Photo>> {
        match self.item()?.as_ref() {
            Item::Picture { photo, .. } => Some(Arc::clone(photo)),
            Item::Video { .. } => None,
        }
    }

    /// Moved to another, by `direction` if by a step (a key, a click): the
    /// one arriving slides in from that side. A swipe carries its own.
    fn moved(&mut self, direction: Option<Direction>, cx: &mut Cx<'_>) {
        if let Some(direction) = direction {
            self.slide.arrive(direction, Instant::now());
        }
        self.toast = None;
        self.dragging = None;
        self.view = View::FIT;
        // Stepped, the strip glides along; swiped, the fingers' offset
        // already carries it, and it goes on from there.
        let at = self.album.at as f32;
        if direction.is_some() {
            let glide = self.transition(Transition::SELECT);
            self.strip_at.set(at, glide, Instant::now());
        } else {
            self.strip_at.jump(at);
        }
        self.want();
        self.awaken(cx);
    }

    fn step(&mut self, step: isize, cx: &mut Cx<'_>) {
        if self.album.step(step) {
            let direction = if step > 0 {
                Direction::Forward
            } else {
                Direction::Back
            };
            self.moved(Some(direction), cx);
        }
    }

    fn go(&mut self, index: usize, cx: &mut Cx<'_>) {
        let from = self.album.at;
        if self.album.go(index) {
            let direction = if index > from {
                Direction::Forward
            } else {
                Direction::Back
            };
            self.moved(Some(direction), cx);
        }
    }

    /// Show the controls for a while.
    fn awaken(&mut self, cx: &mut Cx<'_>) {
        self.chrome_until = Some(Instant::now() + LINGER);
        cx.redraw();
    }

    pub(crate) fn say(&mut self, text: impl Into<String>, cx: &mut Cx<'_>) {
        self.toast = Some((text.into(), Instant::now()));
        cx.redraw();
    }

    // ---- Pictures: fitting, zooming, turning -------------------------------

    /// What a picture is fitted into: the stage, less a margin that keeps
    /// it off the window's edges and the chrome (none, full screen).
    pub(crate) fn room(&self) -> Rect {
        if self.fullscreen {
            self.stage
        } else {
            self.stage.inset(space::LG)
        }
    }

    /// Output pixels per picture pixel that fit the stage, never enlarging.
    fn fit(&self, photo: &Photo) -> f32 {
        let (w, h) = photo.size();
        let room = self.room();
        let (sw, sh) = (room.w * self.scale, room.h * self.scale);
        (sw / w as f32).min(sh / h as f32).min(1.0)
    }

    pub(crate) fn zoom(&self, photo: &Photo) -> f32 {
        self.view.zoom.unwrap_or_else(|| self.fit(photo))
    }

    /// Where the picture goes, in logical pixels.
    pub(crate) fn placement(&self, photo: &Photo) -> Rect {
        let (w, h) = photo.size();
        let zoom = self.zoom(photo);
        let (dw, dh) = (w as f32 * zoom / self.scale, h as f32 * zoom / self.scale);
        let (cx, cy) = self.view.centre.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        let along = |start: f32, window: f32, size: f32, centre: f32| {
            start
                + if size <= window {
                    (window - size) / 2.0
                } else {
                    // Its edges never come inside the stage's.
                    (window / 2.0 - centre * zoom / self.scale).clamp(window - size, 0.0)
                }
        };
        Rect::new(
            along(self.stage.x, self.stage.w, dw, cx).round(),
            along(self.stage.y, self.stage.h, dh, cy).round(),
            dw,
            dh,
        )
    }

    /// Zoom to `zoom` (output pixels per picture pixel), keeping the point
    /// under `at` (logical, in the window) where it is.
    pub(crate) fn zoom_to(&mut self, zoom: f32, at: Option<(f32, f32)>) {
        let Some(photo) = self.photo() else {
            return;
        };
        let fit = self.fit(&photo);
        let zoom = zoom.clamp(fit.min(1.0) / 4.0, MOST);
        let before = self.placement(&photo);
        let old = self.zoom(&photo);
        let middle = (
            self.stage.x + self.stage.w / 2.0,
            self.stage.y + self.stage.h / 2.0,
        );
        let at = at.unwrap_or(middle);
        // The picture's point under `at`, and where the middle must go to
        // keep it there.
        let px = (at.0 - before.x) * self.scale / old;
        let py = (at.1 - before.y) * self.scale / old;
        let centre = (
            px + (middle.0 - at.0) * self.scale / zoom,
            py + (middle.1 - at.1) * self.scale / zoom,
        );
        self.view = if (zoom - fit).abs() < 0.001 {
            View::FIT
        } else {
            View {
                zoom: Some(zoom),
                centre: Some(centre),
            }
        };
    }

    fn step_zoom(&mut self, by: f32, at: Option<(f32, f32)>) {
        if let Some(photo) = self.photo() {
            let now = self.zoom(&photo);
            let fit = self.fit(&photo);
            let mut next = now * by;
            // Passing the fit, stop there on the way.
            if (now < fit && next > fit) || (now > fit && next < fit) {
                next = fit;
            }
            self.zoom_to(next, at);
        }
    }

    /// Fit, or every pixel of it, at the pointer.
    pub(crate) fn toggle_actual(&mut self, at: Option<(f32, f32)>) {
        if self.view.zoom.is_some() {
            self.view = View::FIT;
        } else {
            self.zoom_to(1.0, at);
        }
    }

    /// Move a zoomed picture by `(dx, dy)` logical pixels, as far as it goes.
    fn pan(&mut self, dx: f32, dy: f32) {
        let Some(photo) = self.photo() else { return };
        let (w, h) = photo.size();
        let zoom = self.zoom(&photo) / self.scale;
        let (cx, cy) = self.view.centre.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        self.view.centre = Some((cx + dx / zoom, cy + dy / zoom));
        self.keep_in_reach(&photo);
    }

    /// Keep the centre where the picture can actually go.
    fn keep_in_reach(&mut self, photo: &Photo) {
        let zoom = self.zoom(photo) / self.scale;
        let placed = self.placement(photo);
        let (w, h) = photo.size();
        let clamp = |start: f32, window: f32, size: f32, placed: f32, total: u32| {
            if size <= window {
                total as f32 / 2.0
            } else {
                (start + window / 2.0 - placed) / zoom
            }
        };
        self.view.centre = Some((
            clamp(self.stage.x, self.stage.w, placed.w, placed.x, w),
            clamp(self.stage.y, self.stage.h, placed.h, placed.y, h),
        ));
    }

    pub(crate) fn turn(&mut self, clockwise: bool) {
        let Some((path, item)) = self.shown.clone() else {
            return;
        };
        let Item::Picture { photo, details } = item.as_ref() else {
            return;
        };
        let turned = Arc::new(Item::Picture {
            photo: Arc::new(photo.turned(clockwise)),
            details: details.clone(),
        });
        self.read.insert(path.clone(), Ok(Arc::clone(&turned)));
        self.shown = Some((path, turned));
        self.view = View::FIT;
    }

    // ---- Doing -------------------------------------------------------------

    fn copy(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        let uri = format!("file://{}\r\n", path.display());
        if crate::album::is_video(&path) {
            cx.copy(Clip::default().with("text/uri-list", uri.into_bytes()));
            self.say("Copied", cx);
            return;
        }
        let mime = match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/jpeg",
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                cx.copy(
                    Clip::default()
                        .with(mime, bytes)
                        .with("text/uri-list", uri.into_bytes()),
                );
                self.say("Copied", cx);
            }
            Err(err) => self.say(format!("Can't copy it: {err}"), cx),
        }
    }

    fn throw_away(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        // A video being played lets go of its file first.
        self.video = None;
        match enso_fs::trash::trash(&path) {
            Ok(item) => {
                if let Some((_, at)) = self.album.take() {
                    self.trashed.push((item, at));
                }
                self.read.remove(&path);
                self.thumbs.remove(&path);
                self.shown = None;
                self.moved(Some(Direction::Forward), cx);
                self.say("Moved to the trash · Ctrl+Z puts it back", cx);
            }
            Err(err) => self.say(format!("Can't move it to the trash: {err}"), cx),
        }
    }

    fn put_back(&mut self, cx: &mut Cx<'_>) {
        let Some((item, at)) = self.trashed.pop() else {
            return;
        };
        match enso_fs::trash::restore(&item) {
            Ok(path) => {
                self.album.put_back(path, at);
                self.moved(Some(Direction::Back), cx);
                self.say("Put back", cx);
            }
            Err(err) => self.say(format!("Can't put it back: {err}"), cx),
        }
    }

    fn wallpaper(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        if crate::album::is_video(&path) {
            return;
        }
        let done = Command::new(enso_core::exe::companion("enso-wall"))
            .arg("set")
            .arg(&path)
            .output();
        let said = match done {
            Ok(out) if out.status.success() => "The wallpaper now".to_owned(),
            Ok(out) => format!(
                "Can't make it the wallpaper: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(err) => format!("Can't make it the wallpaper: enso-wall: {err}"),
        };
        self.say(said, cx);
    }

    fn reveal(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current() else {
            return;
        };
        let argv = vec![
            enso_core::exe::companion("enso-files")
                .to_string_lossy()
                .into_owned(),
            "--select".to_owned(),
            path.to_string_lossy().into_owned(),
        ];
        if let Err(err) = enso_fs::apps::spawn_detached(&argv, None) {
            self.say(format!("Can't open Files: {err}"), cx);
        }
    }

    fn toggle_play(&mut self, cx: &mut Cx<'_>) {
        if let Some(video) = &mut self.video {
            video.toggle(Instant::now());
            self.awaken(cx);
        }
    }

    fn toggle_mute(&mut self, cx: &mut Cx<'_>) {
        if let Some(video) = &mut self.video {
            let muted = !video.playback.is_muted();
            video.playback.set_muted(muted);
            self.say(if muted { "Sound off" } else { "Sound on" }, cx);
        }
    }

    fn asked(&mut self, asked: Asked, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match asked {
            Asked::Next => self.step(1, cx),
            Asked::Previous => self.step(-1, cx),
            Asked::Quit => cx.exit(),
            Asked::Raise => {}
            _ => {
                let Some(video) = &mut self.video else { return };
                match asked {
                    Asked::PlayPause => video.toggle(now),
                    Asked::Play => video.play(now),
                    Asked::Pause | Asked::Stop => video.playback.pause(now),
                    Asked::Seek(micros) => video.seek_by(
                        Duration::from_micros(micros.unsigned_abs()),
                        micros >= 0,
                        now,
                    ),
                    Asked::SetPosition(to) => video.seek_to(to, now),
                    _ => {}
                }
                self.awaken(cx);
            }
        }
    }

    // ---- Keys and the pointer ----------------------------------------------

    fn on_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>) {
        if self.help {
            self.help = false;
            cx.redraw();
            return;
        }
        let (ctrl, shift) = (key.mods.ctrl, key.mods.shift);
        let video = self.video.is_some();
        let now = Instant::now();
        match key.key {
            Key::Escape if self.fullscreen => cx.toggle_fullscreen(),
            Key::Escape | Key::Char('q') => cx.exit(),
            Key::Char('c') if ctrl => self.copy(cx),
            Key::Char('z') if ctrl => self.put_back(cx),
            Key::Right | Key::Left if shift && video => {
                if let Some(video) = &mut self.video {
                    video.seek_by(SEEK, key.key == Key::Right, now);
                }
            }
            Key::Char('.' | ',') if video => {
                if let Some(video) = &mut self.video {
                    video.seek_by(SEEK, key.key == Key::Char('.'), now);
                }
            }
            Key::Char(' ' | 'k') if video => self.toggle_play(cx),
            Key::Char('m') if video => self.toggle_mute(cx),
            Key::Right | Key::Char('l' | ' ' | 'n') | Key::PageDown | Key::Down => {
                self.step(1, cx);
            }
            Key::Left | Key::Char('h' | 'p') | Key::Backspace | Key::PageUp | Key::Up => {
                self.step(-1, cx);
            }
            Key::Home => self.go(0, cx),
            Key::End => self.go(self.album.len().saturating_sub(1), cx),
            Key::Char('+' | '=') => self.step_zoom(STEP, None),
            Key::Char('-') => self.step_zoom(1.0 / STEP, None),
            Key::Char('0') => self.view = View::FIT,
            Key::Char('1') => self.zoom_to(1.0, None),
            Key::Char('r') => self.turn(true),
            Key::Char('R') => self.turn(false),
            Key::Char('i') => self.info = !self.info,
            Key::Char('f') | Key::F(11) => cx.toggle_fullscreen(),
            Key::Char('w') => self.wallpaper(cx),
            Key::Char('e') => self.reveal(cx),
            Key::Char('?') | Key::F(1) => self.help = true,
            Key::Delete => self.throw_away(cx),
            _ => return,
        }
        cx.redraw();
    }

    pub(crate) fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| *hit)
    }

    fn on_pointer(&mut self, event: PointerEvent, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                self.pointer = Some((x, y));
                let hit = self.hit(x, y);
                if hit != self.hover {
                    self.hover = hit;
                    let tip = hit.filter(|hit| hit.tip().is_some());
                    self.tooltip_at = self.tooltip.hover(tip, now).map(|delay| now + delay);
                }
                let track = self.timeline();
                if let Some(video) = &mut self.video
                    && video.held.is_some()
                {
                    video.held = Some(enso_ui::widget::wave::fraction_at(track, x));
                }
                if let Some((from, centre)) = self.dragging
                    && let Some(photo) = self.photo()
                {
                    let zoom = self.zoom(&photo) / self.scale;
                    self.view.centre = Some((
                        centre.0 - (x - from.0) / zoom,
                        centre.1 - (y - from.1) / zoom,
                    ));
                    self.keep_in_reach(&photo);
                }
                let cursor = match hit {
                    _ if self.dragging.is_some() => Cursor::Grabbing,
                    Some(Hit::Stage | Hit::Controls | Hit::Zoom) | None => Cursor::Default,
                    Some(_) => Cursor::Pointer,
                };
                cx.set_cursor(cursor);
                self.awaken(cx);
            }
            PointerEvent::Leave => {
                self.pointer = None;
                self.hover = None;
                self.tooltip.hover(None, now);
                cx.redraw();
            }
            PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            } => {
                let clicks = self.clicks.press(x, y, Button::Left, now);
                self.tooltip.hover(None, now);
                match self.hit(x, y) {
                    Some(Hit::Back) => self.step(-1, cx),
                    Some(Hit::Forward) => self.step(1, cx),
                    Some(Hit::Turn) => self.turn(true),
                    Some(Hit::Info) => self.info = !self.info,
                    Some(Hit::Reveal) => self.reveal(cx),
                    Some(Hit::Trash) => self.throw_away(cx),
                    Some(Hit::Zoom) => self.toggle_actual(None),
                    Some(Hit::Thumb(index)) => self.go(index, cx),
                    Some(Hit::Start | Hit::PlayPause) => self.toggle_play(cx),
                    Some(Hit::Mute) => self.toggle_mute(cx),
                    Some(Hit::Timeline) => {
                        let track = self.timeline();
                        if let Some(video) = &mut self.video {
                            video.held = Some(enso_ui::widget::wave::fraction_at(track, x));
                        }
                    }
                    Some(Hit::Controls) => {}
                    Some(Hit::Stage) | None if self.video.is_some() => {
                        if clicks >= 2 {
                            // The first click of the two played it, or
                            // paused it: the second puts that back.
                            self.toggle_play(cx);
                            cx.toggle_fullscreen();
                        } else {
                            self.toggle_play(cx);
                        }
                    }
                    Some(Hit::Stage) | None => {
                        if clicks >= 2 {
                            self.clicks.reset();
                            self.toggle_actual(Some((x, y)));
                        } else if let Some(photo) = self.photo() {
                            let (w, h) = photo.size();
                            let centre =
                                self.view.centre.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
                            self.dragging = Some(((x, y), centre));
                            cx.set_cursor(Cursor::Grabbing);
                        }
                    }
                }
                cx.redraw();
            }
            PointerEvent::Release {
                button: Button::Left,
                ..
            } => {
                self.dragging = None;
                cx.set_cursor(Cursor::Default);
                if let Some(video) = &mut self.video
                    && let Some(held) = video.held.take()
                    && let Some(length) = video.media().duration
                {
                    video.seek_to(length.mul_f32(held.clamp(0.0, 1.0)), now);
                }
                cx.redraw();
            }
            // The side buttons: back and forward.
            PointerEvent::Press {
                button: enso_ui::BUTTON_BACK,
                ..
            } => self.step(-1, cx),
            PointerEvent::Press {
                button: enso_ui::BUTTON_FORWARD,
                ..
            } => self.step(1, cx),
            PointerEvent::Scroll {
                x,
                y,
                dx,
                dy,
                wheel,
                stopped,
            } => self.on_scroll((x, y), (dx, dy), wheel, stopped, cx),
            _ => {}
        }
    }

    fn on_scroll(
        &mut self,
        at: (f32, f32),
        (dx, dy): (f32, f32),
        wheel: bool,
        stopped: bool,
        cx: &mut Cx<'_>,
    ) {
        let now = Instant::now();
        let zoomed = self.view.zoom.is_some() && self.photo().is_some();
        let ctrl = cx.mods().ctrl;
        // A zoomed picture: the fingers move about it.
        if zoomed && !wheel && !ctrl {
            self.pan(dx, dy);
            cx.redraw();
            return;
        }
        if !ctrl {
            let can = self.album.neighbours();
            match self
                .slide
                .scroll((dx, dy, wheel, stopped), can, self.stage.w, now)
            {
                Swiped::Go(direction) => {
                    self.album.step(direction.step());
                    self.moved(None, cx);
                    return;
                }
                Swiped::Moved => {
                    cx.redraw();
                    return;
                }
                Swiped::Scroll => {}
            }
        }
        // Over the strip, a wheel steps along it.
        if wheel
            && self
                .hit(at.0, at.1)
                .is_some_and(|hit| matches!(hit, Hit::Thumb(_)))
        {
            self.step(if dy > 0.0 { 1 } else { -1 }, cx);
            return;
        }
        if self.photo().is_some() {
            let notches = if wheel { dy / 15.0 } else { dy / 60.0 };
            if notches != 0.0 {
                self.step_zoom(STEP.powf(-notches), Some(at));
                cx.redraw();
            }
        }
    }

    /// The video's timeline, where it was drawn.
    fn timeline(&self) -> Rect {
        self.hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Timeline)
            .map_or(Rect::new(0.0, 0.0, 1.0, 1.0), |(rect, _)| *rect)
    }

    /// Ask to be woken for whatever comes next on its own: the video's next
    /// frame, the controls going, a toast going, a swipe going quiet.
    fn schedule(&mut self, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let mut next: Option<Duration> = None;
        let mut soon = |after: Duration| {
            next = Some(next.map_or(after, |next| next.min(after)));
        };
        if let Some(until) = self.chrome_until
            && until > now
        {
            soon(until - now);
        }
        if let Some((_, since)) = &self.toast {
            soon((*since + enso_ui::widget::toast::STAYS).saturating_duration_since(now));
        }
        if self.slide.is_swiping() {
            soon(swipe::QUIET);
        }
        if let Some(video) = &self.video
            && let Some(due) = video.playback.due(now)
        {
            soon(due);
        }
        if let Some(at) = self.tooltip_at
            && at > now
        {
            soon(at - now);
        }
        if let Some((_, since)) = self.resizing {
            soon((since + RESIZE).saturating_duration_since(now));
        }
        if let Some(next) = next {
            cx.wake_in(next.max(Duration::from_millis(1)));
        }
    }

    /// Tell the desktop how the video stands.
    fn announce(&mut self) {
        let can = self.album.neighbours();
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        if let Some(video) = &mut self.video {
            video.announce(can, Instant::now(), move |asked| {
                proxy.send(Message::Asked(asked));
            });
        }
    }
}

impl App for Viewer {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::window(
            WindowSpec {
                app_id: "enso-view".to_owned(),
                title: self
                    .album
                    .current()
                    .map_or_else(|| "Viewer".to_owned(), |path| Self::name(path)),
                min_size: (360, 280),
                ..WindowSpec::default()
            },
            1100,
            760,
        )
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let (ask, asked) = mpsc::channel::<PathBuf>();
        let reader = proxy.clone();
        let spawned = thread::Builder::new()
            .name("enso-view-read".to_owned())
            .spawn(move || {
                for path in asked {
                    let item = item::read(&path).map(Arc::new);
                    if !reader.send(Message::Read(path, item)) {
                        break;
                    }
                }
            });
        if spawned.is_ok() {
            self.reader = Some(ask);
        }
        let thumbs = proxy.clone();
        self.thumbnailer =
            Thumbnailer::start(move |path, thumb| thumbs.send(Message::Thumb(path, thumb)));
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, {
            let proxy = proxy.clone();
            move || proxy.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-view: live theme switching unavailable: {err}");
        }
        self.proxy = Some(proxy);
        self.want();
        self.awaken(cx);
        self.schedule(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Read(path, item)) => {
                self.asked.retain(|asked| *asked != path);
                self.read.insert(path, item);
                self.show_current();
                cx.redraw();
            }
            Event::Message(Message::Thumb(path, thumb)) => {
                if self.thumbs.len() > 400 {
                    self.thumbs.clear();
                }
                self.thumbs.insert(path, thumb);
                cx.redraw();
            }
            Event::Message(Message::Asked(asked)) => self.asked(asked, cx),
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Key(key) => self.on_key(key, cx),
            Event::Pointer(pointer) => self.on_pointer(pointer, cx),
            Event::Resized { .. } => cx.redraw(),
            Event::WindowState {
                maximized,
                fullscreen,
                ..
            } => {
                self.maximized = maximized;
                self.fullscreen = fullscreen;
                cx.redraw();
            }
            Event::Close => cx.exit(),
            Event::Wake => {
                let now = Instant::now();
                if let Some(direction) = self.slide.expire(self.stage.w, now) {
                    self.album.step(direction.step());
                    self.moved(None, cx);
                }
                if let Some(video) = &mut self.video {
                    video.playback.frame(now);
                }
                if self
                    .toast
                    .as_ref()
                    .is_some_and(|(_, since)| since.elapsed() >= enso_ui::widget::toast::STAYS)
                {
                    self.toast = None;
                }
                cx.redraw();
            }
            _ => {}
        }
        self.announce();
        self.schedule(cx);
        let title = self
            .album
            .current()
            .map_or_else(|| "Viewer".to_owned(), |path| Self::name(path));
        if title != self.title {
            cx.set_title(&title);
            self.title = title;
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.draw_window(canvas, now)
    }
}

#[cfg(test)]
mod tests;
