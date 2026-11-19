//! The viewer's window: the picture, as large as fits, on the theme's base
//! colour; a quiet line under it saying which it is, there while you move
//! and gone when you look.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context,
    style::{space, type_scale},
};
use enso_ui::{
    Align, App, Button, Canvas, Clip, Cursor, Cx, Event, Key, KeyEvent, Paint, PointerEvent,
    Proxy, Rect, SurfaceSpec, Text, WindowSpec,
};

use crate::{album::Album, photo::Photo};

/// How long the line under the picture stays after the last move.
const LINGER: Duration = Duration::from_millis(1800);
/// Two clicks this close are a double click.
const DOUBLE: Duration = Duration::from_millis(400);
/// One zoom step, by key or by a wheel's notch.
const STEP: f32 = 1.25;
/// The most a picture is enlarged: this many screen pixels to one of its own.
const MOST: f32 = 32.0;

#[derive(Debug)]
pub enum Message {
    /// A picture was read, or couldn't be.
    Read(PathBuf, Result<Arc<Photo>, String>),
}

/// How the picture is shown: zoomed to fit, or so many output pixels to
/// each of its own, around a point of it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    zoom: Option<f32>,
    /// The picture's point at the window's middle, in its own pixels.
    centre: Option<(f32, f32)>,
}

impl View {
    const FIT: Self = Self {
        zoom: None,
        centre: None,
    };
}

pub struct Viewer {
    ctx: Context,
    album: Album,
    read: HashMap<PathBuf, Result<Arc<Photo>, String>>,
    reader: Option<mpsc::Sender<PathBuf>>,
    asked: Vec<PathBuf>,
    /// The one on screen, kept while the next is read.
    shown: Option<(PathBuf, Arc<Photo>)>,
    view: View,
    size: (f32, f32),
    scale: f32,
    pointer: Option<(f32, f32)>,
    /// Being dragged: where the pointer was, and the centre then.
    dragging: Option<((f32, f32), (f32, f32))>,
    last_click: Option<Instant>,
    /// The line under the picture shows until then.
    chrome_until: Option<Instant>,
    /// Something said, until when.
    notice: Option<(String, Instant)>,
    /// What was thrown away here, last first, to put back.
    trashed: Vec<(enso_fs::trash::Item, usize)>,
    title: String,
}

impl Viewer {
    pub fn new(ctx: Context, album: Album) -> Self {
        Self {
            ctx,
            album,
            read: HashMap::new(),
            reader: None,
            asked: Vec::new(),
            shown: None,
            view: View::FIT,
            size: (1100.0, 760.0),
            scale: 1.0,
            pointer: None,
            dragging: None,
            last_click: None,
            chrome_until: None,
            notice: None,
            trashed: Vec::new(),
            title: String::new(),
        }
    }

    fn name(path: &Path) -> String {
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }

    /// Ask for the current picture and its neighbours; forget the rest.
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
        self.show_current();
    }

    /// Put the current picture on screen, if it has been read.
    fn show_current(&mut self) {
        let Some(path) = self.album.current().cloned() else {
            self.shown = None;
            return;
        };
        if let Some(Ok(photo)) = self.read.get(&path)
            && self.shown.as_ref().is_none_or(|(shown, _)| *shown != path)
        {
            self.shown = Some((path, Arc::clone(photo)));
            self.view = View::FIT;
        }
    }

    fn go(&mut self, moved: bool, cx: &mut Cx<'_>) {
        if moved {
            self.notice = None;
            self.want();
            self.awaken(cx);
        }
    }

    /// Show the line under the picture for a while.
    fn awaken(&mut self, cx: &mut Cx<'_>) {
        self.chrome_until = Some(Instant::now() + LINGER);
        cx.wake_in(LINGER);
        cx.redraw();
    }

    fn say(&mut self, text: String, cx: &mut Cx<'_>) {
        self.notice = Some((text, Instant::now() + LINGER * 2));
        self.chrome_until = Some(Instant::now() + LINGER * 2);
        cx.wake_in(LINGER * 2);
        cx.redraw();
    }

    /// The photo on screen, if it is the current one.
    fn photo(&self) -> Option<&Arc<Photo>> {
        let (path, photo) = self.shown.as_ref()?;
        (Some(path) == self.album.current()).then_some(photo)
    }

    /// Output pixels per picture pixel that fit the window, never enlarging.
    fn fit(&self, photo: &Photo) -> f32 {
        let (w, h) = photo.size();
        let (ww, wh) = (self.size.0 * self.scale, self.size.1 * self.scale);
        (ww / w as f32).min(wh / h as f32).min(1.0)
    }

    fn zoom(&self, photo: &Photo) -> f32 {
        self.view.zoom.unwrap_or_else(|| self.fit(photo))
    }

    /// Where the picture goes, in logical pixels.
    fn placement(&self, photo: &Photo) -> Rect {
        let (w, h) = photo.size();
        let zoom = self.zoom(photo);
        let (dw, dh) = (w as f32 * zoom / self.scale, h as f32 * zoom / self.scale);
        let (cx, cy) = self
            .view
            .centre
            .unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        let along = |window: f32, size: f32, centre: f32| {
            if size <= window {
                (window - size) / 2.0
            } else {
                // Its edges never come inside the window's.
                (window / 2.0 - centre * zoom / self.scale).clamp(window - size, 0.0)
            }
        };
        Rect::new(
            along(self.size.0, dw, cx).round(),
            along(self.size.1, dh, cy).round(),
            dw,
            dh,
        )
    }

    /// Zoom to `zoom` (output pixels per picture pixel), keeping the point
    /// under `at` (logical, in the window) where it is.
    fn zoom_to(&mut self, zoom: f32, at: Option<(f32, f32)>) {
        let Some(photo) = self.photo().cloned() else {
            return;
        };
        let fit = self.fit(&photo);
        let zoom = zoom.clamp(fit.min(1.0) / 4.0, MOST);
        let before = self.placement(&photo);
        let old = self.zoom(&photo);
        let at = at.unwrap_or((self.size.0 / 2.0, self.size.1 / 2.0));
        // The picture's point under `at`, and where the middle must go to
        // keep it there.
        let px = (at.0 - before.x) * self.scale / old;
        let py = (at.1 - before.y) * self.scale / old;
        let centre = (
            px + (self.size.0 / 2.0 - at.0) * self.scale / zoom,
            py + (self.size.1 / 2.0 - at.1) * self.scale / zoom,
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
        if let Some(photo) = self.photo().cloned() {
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
    fn toggle_actual(&mut self, at: Option<(f32, f32)>) {
        if self.view.zoom.is_some() {
            self.view = View::FIT;
        } else {
            self.zoom_to(1.0, at);
        }
    }

    fn turn(&mut self, clockwise: bool) {
        let Some((path, photo)) = self.shown.clone() else {
            return;
        };
        let turned = Arc::new(photo.turned(clockwise));
        self.read.insert(path.clone(), Ok(Arc::clone(&turned)));
        self.shown = Some((path, turned));
        self.view = View::FIT;
    }

    fn copy(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
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
                let uri = format!("file://{}\r\n", path.display());
                cx.copy(
                    Clip::default()
                        .with(mime, bytes)
                        .with("text/uri-list", uri.into_bytes()),
                );
                self.say("Copied".to_owned(), cx);
            }
            Err(err) => self.say(format!("Can't copy it: {err}"), cx),
        }
    }

    fn throw_away(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        match enso_fs::trash::trash(&path) {
            Ok(item) => {
                if let Some((_, at)) = self.album.take() {
                    self.trashed.push((item, at));
                }
                self.read.remove(&path);
                self.shown = None;
                self.want();
                self.say("Moved to the trash · Ctrl+Z puts it back".to_owned(), cx);
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
                self.want();
                self.say("Put back".to_owned(), cx);
            }
            Err(err) => self.say(format!("Can't put it back: {err}"), cx),
        }
    }

    fn wallpaper(&mut self, cx: &mut Cx<'_>) {
        let Some(path) = self.album.current().cloned() else {
            return;
        };
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

    fn on_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>) {
        let ctrl = key.mods.ctrl;
        match key.key {
            Key::Escape | Key::Char('q') => cx.exit(),
            Key::Char('c') if ctrl => self.copy(cx),
            Key::Char('z') if ctrl => self.put_back(cx),
            Key::Right | Key::Char('l' | ' ' | 'n') | Key::PageDown | Key::Down => {
                let moved = self.album.step(1);
                self.go(moved, cx);
            }
            Key::Left | Key::Char('h' | 'p') | Key::Backspace | Key::PageUp | Key::Up => {
                let moved = self.album.step(-1);
                self.go(moved, cx);
            }
            Key::Home => {
                let moved = self.album.end(false);
                self.go(moved, cx);
            }
            Key::End => {
                let moved = self.album.end(true);
                self.go(moved, cx);
            }
            Key::Char('+' | '=') => self.step_zoom(STEP, None),
            Key::Char('-') => self.step_zoom(1.0 / STEP, None),
            Key::Char('0') => self.view = View::FIT,
            Key::Char('1') => self.zoom_to(1.0, None),
            Key::Char('r') => self.turn(true),
            Key::Char('R') => self.turn(false),
            Key::Char('f') | Key::F(11) => cx.toggle_maximized(),
            Key::Char('w') => self.wallpaper(cx),
            Key::Char('e') => self.reveal(cx),
            Key::Delete => self.throw_away(cx),
            _ => return,
        }
        self.awaken(cx);
    }

    fn on_pointer(&mut self, event: PointerEvent, cx: &mut Cx<'_>) {
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                self.pointer = Some((x, y));
                if let Some((from, centre)) = self.dragging
                    && let Some(photo) = self.photo().cloned()
                {
                    let zoom = self.zoom(&photo) / self.scale;
                    self.view.centre =
                        Some((centre.0 - (x - from.0) / zoom, centre.1 - (y - from.1) / zoom));
                    // Kept within reach: where the picture can actually go.
                    let placed = self.placement(&photo);
                    let (w, h) = photo.size();
                    let clamp = |window: f32, size: f32, placed: f32, total: u32| {
                        if size <= window {
                            total as f32 / 2.0
                        } else {
                            (window / 2.0 - placed) / zoom
                        }
                    };
                    self.view.centre = Some((
                        clamp(self.size.0, placed.w, placed.x, w),
                        clamp(self.size.1, placed.h, placed.y, h),
                    ));
                }
                self.awaken(cx);
            }
            PointerEvent::Leave => self.pointer = None,
            PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            } => {
                let now = Instant::now();
                if self
                    .last_click
                    .is_some_and(|last| now.duration_since(last) < DOUBLE)
                {
                    self.last_click = None;
                    self.toggle_actual(Some((x, y)));
                } else {
                    self.last_click = Some(now);
                    if let Some(photo) = self.photo().cloned() {
                        let (w, h) = photo.size();
                        let centre = self
                            .view
                            .centre
                            .unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
                        self.dragging = Some(((x, y), centre));
                        cx.set_cursor(Cursor::Grabbing);
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
            }
            // The side buttons: back and forward.
            PointerEvent::Press {
                button: Button::Other(code),
                ..
            } => {
                let step = match code {
                    0x113 => -1,
                    0x114 => 1,
                    _ => return,
                };
                let moved = self.album.step(step);
                self.go(moved, cx);
            }
            PointerEvent::Scroll { x, y, dy, wheel, .. } => {
                let notches = if wheel { dy / 15.0 } else { dy / 60.0 };
                if notches != 0.0 {
                    self.step_zoom(STEP.powf(-notches), Some((x, y)));
                    cx.redraw();
                }
            }
            _ => {}
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
                    .map_or_else(|| "Pictures".to_owned(), |path| Self::name(path)),
                min_size: (320, 240),
                ..WindowSpec::default()
            },
            1100,
            760,
        )
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        let (ask, asked) = mpsc::channel::<PathBuf>();
        let spawned = thread::Builder::new()
            .name("enso-view-read".to_owned())
            .spawn(move || {
                for path in asked {
                    let photo = Photo::open(&path).map(Arc::new);
                    if !proxy.send(Message::Read(path, photo)) {
                        break;
                    }
                }
            });
        if spawned.is_ok() {
            self.reader = Some(ask);
        }
        self.want();
        self.awaken(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Message(Message::Read(path, photo)) => {
                self.asked.retain(|asked| *asked != path);
                self.read.insert(path, photo);
                self.show_current();
                cx.redraw();
            }
            Event::Key(key) => self.on_key(key, cx),
            Event::Pointer(pointer) => self.on_pointer(pointer, cx),
            Event::Resized { width, height } => {
                self.size = (width as f32, height as f32);
                cx.redraw();
            }
            Event::Close => cx.exit(),
            Event::Wake => cx.redraw(),
            _ => {}
        }
        let title = self
            .album
            .current()
            .map_or_else(|| "Pictures".to_owned(), |path| Self::name(path));
        if title != self.title {
            cx.set_title(&title);
            self.title = title;
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let colors = self.ctx.theme.colors.clone();
        let size = self.ctx.config.appearance.font_size;
        self.scale = canvas.scale();
        let bounds = canvas.bounds();
        self.size = (bounds.w, bounds.h);
        canvas.clear(colors.base);

        let current = self.album.current().cloned();
        match (self.photo().cloned(), current.as_ref().and_then(|p| self.read.get(p))) {
            (Some(photo), _) => {
                let placed = self.placement(&photo);
                let level = photo.level_for(placed.w * self.scale);
                canvas.pixels(&level.rgba, level.width, level.height, placed);
            }
            (None, Some(Err(err))) => {
                let said = format!("Can't show it: {err}");
                canvas.text_in(&Text::new(&said, size, colors.muted), bounds, Align::Center);
            }
            (None, _) if current.is_none() => {
                canvas.text_in(
                    &Text::new("No pictures here", size, colors.muted),
                    bounds,
                    Align::Center,
                );
            }
            // Being read: the one before stays a moment.
            (None, _) => {
                if let Some((_, photo)) = self.shown.clone() {
                    let placed = self.placement(&photo);
                    let level = photo.level_for(placed.w * self.scale);
                    canvas.with_opacity(0.4, |canvas| {
                        canvas.pixels(&level.rgba, level.width, level.height, placed);
                    });
                }
            }
        }

        // The line under it: which picture, how big, how near.
        let notice = self
            .notice
            .as_ref()
            .filter(|(_, until)| now < *until)
            .map(|(text, _)| text.clone());
        let showing = self.chrome_until.is_some_and(|until| now < until) || notice.is_some();
        if showing && let Some(path) = &current {
            let mut parts = vec![Self::name(path)];
            if self.album.pictures.len() > 1 {
                parts.push(format!("{} of {}", self.album.at + 1, self.album.pictures.len()));
            }
            if let Some(photo) = self.photo() {
                let (w, h) = photo.size();
                parts.push(format!("{w} × {h}"));
                parts.push(format!("{:.0}%", self.zoom(photo) * 100.0 / self.scale.max(0.01)));
            }
            let line = notice.unwrap_or_else(|| parts.join("   ·   "));
            let text = Text::new(&line, size * type_scale::SMALL, colors.text);
            let measured = canvas.measure(&text);
            let pill = Rect::new(
                (bounds.w - measured.w) / 2.0 - space::LG,
                bounds.h - space::LG - measured.h - 2.0 * space::SM,
                measured.w + 2.0 * space::LG,
                measured.h + 2.0 * space::SM,
            );
            canvas.pill(pill, colors.surface.alpha(0.88));
            canvas.text_in(&text, pill, Align::Center);
        }
        Paint::Idle
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    /// A viewer showing an 800 × 400 picture, left half red, right half blue.
    fn viewer() -> Viewer {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-view-test"));
        let path = PathBuf::from("/pictures/wide.png");
        let mut rgba = Vec::new();
        for _ in 0..400 {
            for x in 0..800 {
                rgba.extend(if x < 400 { [200, 40, 40, 255] } else { [40, 40, 200, 255] });
            }
        }
        let mut viewer = Viewer::new(
            ctx,
            Album {
                pictures: vec![path.clone()],
                at: 0,
            },
        );
        viewer
            .read
            .insert(path, Ok(Arc::new(Photo::new(800, 400, rgba))));
        viewer.show_current();
        viewer
    }

    fn frame(viewer: &mut Viewer, size: (u32, u32)) -> Offscreen {
        let mut text = TextEngine::new(&viewer.ctx.config.appearance);
        viewer.chrome_until = Some(Instant::now() + LINGER);
        Offscreen::render(&mut text, size, 1, |canvas| {
            viewer.draw(canvas, Instant::now());
        })
    }

    #[test]
    fn fits_the_window_and_zooms_about_the_pointer() {
        let mut viewer = viewer();
        let shown = frame(&mut viewer, (600, 500));
        shown.snapshot(env!("CARGO_MANIFEST_DIR"), "view");
        let photo = viewer.photo().cloned().unwrap();
        // Shrunk to the window's width, in its middle.
        let placed = viewer.placement(&photo);
        assert_eq!((placed.w, placed.h), (600.0, 300.0));
        assert_eq!((placed.x, placed.y), (0.0, 100.0));
        assert_eq!(shown.pixel(100, 250)[..3], [200, 40, 40]);
        assert_eq!(shown.pixel(500, 250)[..3], [40, 40, 200]);

        // Every pixel, around a point on the blue half: it stays put.
        viewer.zoom_to(1.0, Some((450.0, 250.0)));
        let placed = viewer.placement(&photo);
        assert_eq!(placed.w, 800.0);
        let under = (450.0 - placed.x, 250.0 - placed.y);
        assert!((under.0 - 600.0).abs() < 1.5 && (under.1 - 200.0).abs() < 1.5, "{under:?}");
        // Back to fit; never enlarged past it by fitting.
        viewer.toggle_actual(None);
        assert_eq!(viewer.view, View::FIT);
        let big = frame(&mut viewer, (1600, 1000));
        assert_eq!(viewer.placement(&photo).w, 800.0);
        drop(big);
    }

    #[test]
    fn turning_swaps_the_sides() {
        let mut viewer = viewer();
        viewer.turn(true);
        assert_eq!(viewer.photo().unwrap().size(), (400, 800));
        // Still the one read, so stepping away and back keeps it turned.
        assert_eq!(
            viewer.read.values().next().unwrap().as_ref().unwrap().size(),
            (400, 800)
        );
    }
}
