//! The window: a title row with the way back and the path, the folder's
//! entries in rows, and a status line.
//!
//! Layout is worked out while drawing, and every place that answers a click
//! is written down then ([`Hit`]), so the pointer asks "what is under me"
//! of the last frame instead of laying everything out again.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use enso_core::{
    Context, Icon, Rgba,
    motion::{Transition, Tween},
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_fs::{format, places};
use enso_ui::{
    Align, App, BUTTON_BACK, BUTTON_FORWARD, Button, Canvas, Clicks, Clip, Cursor, Cx, Event, Key,
    KeyEvent, Nav, Paint, PointerEvent, Proxy, Rect, SurfaceSpec, TEXT_TYPES, Text, WindowSpec,
    watch::watch_dir,
    widget::{TextField, field},
};

use crate::{
    config::FilesConfig,
    jobs::{Loader, Message},
    pane::{Load, Open, Pane, Request},
};

const TITLE_HEIGHT: f32 = 52.0;
const HEADER_HEIGHT: f32 = 30.0;
const STATUS_HEIGHT: f32 = 32.0;
/// Room for the size and date columns.
const SIZE_WIDTH: f32 = 96.0;
const DATE_WIDTH: f32 = 128.0;
/// Rows a wheel notch scrolls.
const WHEEL_ROWS: f32 = 3.0;
/// What a compositor reports for one notch, in logical pixels.
const NOTCH: f32 = 15.0;

/// Something on screen that answers the pointer.
#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Row(usize),
    /// A segment of the path: go there.
    Crumb(PathBuf),
    Back,
    Forward,
    /// Empty title row: grab it to move the window.
    Title,
    /// The filter field, whose text starts at this `x`.
    Filter(f32),
}

pub struct Files {
    ctx: Context,
    config: FilesConfig,
    pane: Pane,
    /// The first folder's request, until there is a proxy to load it with.
    pending: Option<Request>,
    loader: Loader,
    proxy: Option<Proxy<Message>>,
    size: (f32, f32),
    /// The list's scroll offset, in logical pixels from its top.
    scroll: Tween<f32>,
    /// Where the cursor's highlight is drawn, gliding between rows.
    highlight: Tween<f32>,
    hover: Option<Hit>,
    hits: Vec<(Rect, Hit)>,
    clicks: Clicks,
    /// The list's rectangle in the last frame.
    list: Rect,
    /// `g` was pressed: the next key completes a `g` command.
    pending_g: bool,
    /// The filter-as-you-type field, while it is open.
    filter: Option<TextField>,
    maximized: bool,
    home: PathBuf,
}

impl Files {
    pub fn new(ctx: Context, config: FilesConfig, location: PathBuf) -> Self {
        let (pane, request) = Pane::new(location, config.sort(), config.hidden);
        Self {
            ctx,
            config,
            pane,
            pending: Some(request),
            loader: Loader::default(),
            proxy: None,
            size: (960.0, 640.0),
            scroll: Tween::new(0.0),
            highlight: Tween::new(0.0),
            hover: None,
            hits: Vec::new(),
            clicks: Clicks::default(),
            list: Rect::default(),
            pending_g: false,
            filter: None,
            maximized: false,
            home: places::home(),
        }
    }

    fn row_height(&self) -> f32 {
        self.config.density.row_height()
    }

    fn font(&self, scale: f32) -> f32 {
        self.ctx.config.appearance.font_size * scale
    }

    fn transition(&self, transition: Transition) -> Transition {
        self.ctx.motion.apply(transition)
    }

    // Moving about.

    fn load(&mut self, request: Option<Request>) {
        let Some(request) = request else { return };
        // A new folder starts at its top; the highlight doesn't glide in
        // from where the last one's was.
        self.scroll.jump(0.0);
        self.highlight.jump(0.0);
        self.hover = None;
        // The pane starts the new folder unfiltered; so does the field.
        self.filter = None;
        match &self.proxy {
            Some(proxy) => self.loader.load(request, proxy),
            None => self.pending = Some(request),
        }
    }

    fn open(&mut self) {
        match self.pane.open() {
            Some(Open::Dir(path)) => {
                let request = self.pane.navigate(path);
                self.load(request);
            }
            Some(Open::File(path)) => {
                if let Err(err) = enso_fs::open::open(&path, "alacritty") {
                    eprintln!("enso-files: cannot open {}: {err}", path.display());
                }
            }
            None => {}
        }
    }

    /// After the cursor moved: glide the highlight there, and scroll just
    /// enough to keep it in view.
    fn follow_cursor(&mut self, now: Instant) {
        let Some(row) = self.pane.cursor() else {
            return;
        };
        let row_height = self.row_height();
        let top = row as f32 * row_height;
        self.highlight
            .set(top, self.transition(Transition::SELECT), now);
        let view = self.list.h.max(row_height);
        let scroll = self.scroll.target();
        let target = if top < scroll {
            top
        } else if top + row_height > scroll + view {
            top + row_height - view
        } else {
            scroll
        };
        self.scroll_to(target, true, now);
    }

    fn max_scroll(&self) -> f32 {
        (self.pane.len() as f32 * self.row_height() - self.list.h).max(0.0)
    }

    fn scroll_to(&mut self, target: f32, animate: bool, now: Instant) {
        let target = target.clamp(0.0, self.max_scroll());
        if animate {
            self.scroll
                .set(target, self.transition(Transition::SELECT), now);
        } else {
            self.scroll.jump(target);
        }
    }

    fn visible_rows(&self) -> isize {
        ((self.list.h / self.row_height()).floor() as isize).max(1)
    }

    // Input.

    /// Keys for the filter field, while it is open. Returns whether the
    /// key was used.
    fn filter_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) -> bool {
        let Some(field) = &mut self.filter else {
            return false;
        };
        // The list keeps moving under arrows while typing; j and k are
        // letters here.
        if let Some(nav) = key.nav(false) {
            if self.pane.move_cursor(match nav {
                Nav::Up => -1,
                Nav::Down => 1,
            }) {
                self.follow_cursor(now);
            }
            cx.redraw();
            return true;
        }
        match field.key(&key) {
            field::Response::Ignored => return false,
            field::Response::Cancel => {
                self.filter = None;
                self.pane.set_filter("");
                self.follow_cursor(now);
            }
            field::Response::Submit => {
                // Leave the field; the folder stays filtered until Esc or
                // a new folder.
                self.filter = None;
                self.open();
            }
            field::Response::Changed => {
                let text = field.text().to_owned();
                self.pane.set_filter(&text);
                self.follow_cursor(now);
            }
            field::Response::Copy(text) => cx.copy(Clip::text(&text)),
            field::Response::Cut(text) => {
                cx.copy(Clip::text(&text));
                let text = field.text().to_owned();
                self.pane.set_filter(&text);
                self.follow_cursor(now);
            }
            field::Response::Paste => cx.paste(&TEXT_TYPES),
            field::Response::Moved => {}
        }
        cx.redraw();
        true
    }

    fn open_filter(&mut self, cx: &mut Cx<'_>) {
        if self.filter.is_none() {
            self.filter = Some(TextField::new(self.pane.filter()));
        }
        cx.redraw();
    }

    fn key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) {
        if self.filter_key(key, cx, now) {
            return;
        }
        let mods = key.mods;
        let g = std::mem::take(&mut self.pending_g);
        let rows = self.visible_rows();
        let moved = if let Some(nav) = key.nav(true) {
            self.pane.move_cursor(match nav {
                Nav::Up => -1,
                Nav::Down => 1,
            })
        } else {
            match key.key {
                Key::PageUp => self.pane.move_cursor(-rows),
                Key::PageDown => self.pane.move_cursor(rows),
                Key::Home => self.pane.set_cursor(0),
                Key::End | Key::Char('G') => self.pane.set_cursor(usize::MAX),
                Key::Char('g') if mods.is_plain() && g => self.pane.set_cursor(0),
                Key::Char('g') if mods.is_plain() => {
                    self.pending_g = true;
                    return;
                }
                Key::Char('h') if g && mods.is_plain() => {
                    let request = self.pane.navigate(self.home.clone());
                    self.load(request);
                    false
                }
                Key::Enter | Key::Right | Key::Char('l') if !mods.alt => {
                    self.open();
                    false
                }
                Key::Left | Key::Char('h') | Key::Backspace if !mods.alt && !mods.ctrl => {
                    let request = self.pane.parent();
                    self.load(request);
                    false
                }
                Key::Up if mods.alt => {
                    let request = self.pane.parent();
                    self.load(request);
                    false
                }
                Key::Left | Key::Char('H') if mods.alt || key.key == Key::Char('H') => {
                    let request = self.pane.back();
                    self.load(request);
                    false
                }
                Key::Right | Key::Char('L') if mods.alt || key.key == Key::Char('L') => {
                    let request = self.pane.forward();
                    self.load(request);
                    false
                }
                Key::Char('/') if mods.is_plain() => {
                    self.open_filter(cx);
                    return;
                }
                Key::Char('f') if mods.ctrl => {
                    self.open_filter(cx);
                    return;
                }
                // A filter left on from before: Esc takes it off.
                Key::Escape if !self.pane.filter().is_empty() => {
                    self.pane.set_filter("");
                    true
                }
                Key::Char('~') => {
                    let request = self.pane.navigate(self.home.clone());
                    self.load(request);
                    false
                }
                Key::Char('h') if mods.ctrl => {
                    self.pane.toggle_hidden();
                    true
                }
                Key::Char('.') if mods.is_plain() => {
                    self.pane.toggle_hidden();
                    true
                }
                Key::Char('r') if mods.ctrl => {
                    let request = Some(self.pane.reload());
                    if let (Some(request), Some(proxy)) = (request, &self.proxy) {
                        self.loader.load(request, proxy);
                    }
                    false
                }
                // Ctrl+Shift+C: the path under the cursor, as text.
                Key::Char('c' | 'C') if mods.ctrl && mods.shift => {
                    if let Some(row) = self.pane.cursor() {
                        let path = self.pane.path(row);
                        cx.copy(Clip::text(&path.to_string_lossy()));
                    }
                    return;
                }
                Key::Char('q' | 'w') if mods.ctrl => {
                    cx.exit();
                    return;
                }
                _ => return,
            }
        };
        if moved {
            self.follow_cursor(now);
        }
        cx.redraw();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| hit.clone())
    }

    fn pointer(&mut self, event: PointerEvent, cx: &mut Cx<'_>, now: Instant) {
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                let hover = self.hit(x, y);
                cx.set_cursor(match hover {
                    Some(Hit::Crumb(_) | Hit::Back | Hit::Forward) => Cursor::Pointer,
                    Some(Hit::Filter(_)) => Cursor::Text,
                    _ => Cursor::Default,
                });
                if hover != self.hover {
                    self.hover = hover;
                    cx.redraw();
                }
            }
            PointerEvent::Leave => {
                if self.hover.take().is_some() {
                    cx.redraw();
                }
            }
            PointerEvent::Press { x, y, button } => {
                let clicks = self.clicks.press(x, y, button, now);
                let request = match (button, self.hit(x, y)) {
                    (b, _) if b == BUTTON_BACK => self.pane.back(),
                    (b, _) if b == BUTTON_FORWARD => self.pane.forward(),
                    (Button::Left, Some(Hit::Row(row))) => {
                        if self.pane.set_cursor(row) {
                            self.follow_cursor(now);
                        }
                        if clicks >= 2 {
                            self.clicks.reset();
                            self.open();
                        }
                        None
                    }
                    (Button::Left, Some(Hit::Crumb(path))) => self.pane.navigate(path),
                    (Button::Left, Some(Hit::Back)) => self.pane.back(),
                    (Button::Left, Some(Hit::Forward)) => self.pane.forward(),
                    (Button::Left, Some(Hit::Filter(text_x))) => {
                        self.open_filter(cx);
                        let size = self.font(type_scale::BODY);
                        if let Some(field) = &mut self.filter {
                            field.click(
                                |text| cx.measure(&Text::new(text, size, Rgba::TRANSPARENT)).w,
                                x - text_x,
                                false,
                            );
                            if clicks >= 2 {
                                field.select_word();
                            }
                        }
                        None
                    }
                    (Button::Left, Some(Hit::Title)) => {
                        if clicks >= 2 {
                            self.clicks.reset();
                            cx.toggle_maximized();
                        } else {
                            cx.begin_move();
                        }
                        None
                    }
                    _ => None,
                };
                self.load(request);
                cx.redraw();
            }
            PointerEvent::Scroll {
                x, y, dy, wheel, ..
            } => {
                if !self.list.contains(x, y) {
                    return;
                }
                if wheel {
                    let notches = (dy / NOTCH).round();
                    let notches = if notches == 0.0 { dy.signum() } else { notches };
                    let target = self.scroll.target() + notches * WHEEL_ROWS * self.row_height();
                    self.scroll_to(target, true, now);
                } else {
                    // Fingers on a touchpad: follow them exactly.
                    let target = self.scroll.get(now) + dy;
                    self.scroll_to(target, false, now);
                }
                cx.redraw();
            }
            PointerEvent::Release { .. } => {}
        }
    }

    // Drawing.

    fn draw_title(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        self.hits.push((area, Hit::Title));

        // Back and forward.
        let button = 32.0;
        let mut x = area.x + space::MD;
        for (icon, hit, enabled) in [
            (icon::CHEVRON_LEFT, Hit::Back, self.pane.can_go_back()),
            (
                icon::CHEVRON_RIGHT,
                Hit::Forward,
                self.pane.can_go_forward(),
            ),
        ] {
            let rect = Rect::new(x, area.y + (area.h - button) / 2.0, button, button);
            if enabled && self.hover.as_ref() == Some(&hit) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let color = if enabled {
                colors.text
            } else {
                colors.muted.alpha(opacity::DISABLED)
            };
            canvas.icon(icon, 18.0, color, rect, Align::Center);
            if enabled {
                self.hits.push((rect, hit));
            }
            x += button + space::XS;
        }
        x += space::SM;

        // The path, as crumbs: home is "~", and what's above the last few
        // folders is folded away when there isn't room.
        let crumbs = crumbs(self.pane.location(), &self.home);
        let size = self.font(type_scale::BODY);
        let separator = Text::new("›", size, colors.muted);
        let gap = canvas.measure(&separator).w + space::SM * 2.0;
        let right = self.draw_filter(canvas, area) - space::LG;
        let widths: Vec<f32> = crumbs
            .iter()
            .enumerate()
            .map(|(i, (label, _))| {
                let text = Text::new(label, size, colors.text);
                let text = if i + 1 == crumbs.len() {
                    text.medium()
                } else {
                    text
                };
                canvas.measure(&text).w + space::SM * 2.0
            })
            .collect();
        // Fold from the left until the rest fits.
        let mut first = 0;
        while first + 1 < crumbs.len()
            && widths[first..].iter().sum::<f32>() + gap * (crumbs.len() - first) as f32 > right - x
        {
            first += 1;
        }
        if first > 0 {
            let ellipsis = Text::new("…", size, colors.muted);
            x += canvas
                .text_in(&ellipsis, Rect::new(x, area.y, 24.0, area.h), Align::Start)
                .w;
            x += space::SM;
        }
        let pill = 30.0;
        for (i, (label, path)) in crumbs.iter().enumerate().skip(first) {
            let last = i + 1 == crumbs.len();
            if i > first {
                canvas.text_in(&separator, Rect::new(x, area.y, gap, area.h), Align::Center);
                x += gap;
            }
            // The last crumb may have to stop short of the edge.
            let width = widths[i].min((right - x).max(0.0));
            let rect = Rect::new(x, area.y + (area.h - pill) / 2.0, width, pill);
            let hit = Hit::Crumb(path.clone());
            if last {
                canvas.fill(rect, radius::pill(pill), accent.alpha(opacity::TINT));
            } else if self.hover.as_ref() == Some(&hit) {
                canvas.fill(rect, radius::pill(pill), colors.highlight_low);
            }
            let text = Text::new(label, size, if last { colors.text } else { colors.subtle });
            let text = if last { text.medium() } else { text };
            canvas.text_in(&text, rect.inset_xy(space::SM, 0.0), Align::Center);
            if !last {
                self.hits.push((rect, hit));
            }
            x += widths[i];
        }
    }

    fn draw_list(&mut self, canvas: &mut Canvas<'_>, area: Rect, now: Instant) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let row_height = self.row_height();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);

        // Column headings.
        let header = Rect::new(area.x, area.y, area.w, HEADER_HEIGHT);
        let columns = Columns::new(header.inset_xy(space::LG, 0.0));
        let heading = |label| Text::new(label, small, colors.muted).medium();
        canvas.text_in(&heading("Name"), columns.name, Align::Start);
        canvas.text_in(&heading("Size"), columns.size, Align::End);
        canvas.text_in(&heading("Modified"), columns.date, Align::Start);
        canvas.fill(
            Rect::new(
                area.x + space::LG,
                header.bottom() - 1.0,
                area.w - 2.0 * space::LG,
                stroke::HAIRLINE,
            ),
            0.0,
            colors.highlight_med,
        );

        let list = Rect::new(area.x, header.bottom(), area.w, area.h - HEADER_HEIGHT);
        self.list = list;
        // The window may have grown since the scroll was set.
        if self.scroll.target() > self.max_scroll() {
            self.scroll.jump(self.max_scroll());
        }

        match self.pane.load().clone() {
            Load::Loading if self.pane.is_empty() => return,
            Load::Failed(why) => {
                let text = Text::new(&why, body, colors.subtle)
                    .max_width(list.w * 0.6)
                    .max_lines(3);
                canvas.text_in(&text, list.centered(list.w * 0.6, 80.0), Align::Center);
                return;
            }
            _ if self.pane.is_empty() => {
                let message = match self.pane.hidden_count() {
                    0 => "Nothing here yet".to_owned(),
                    n => format!(
                        "Nothing here but {} — Ctrl+H shows {}",
                        format::count(n, "hidden file", "hidden files"),
                        if n == 1 { "it" } else { "them" }
                    ),
                };
                let text = Text::new(&message, body, colors.muted);
                canvas.text_in(&text, list, Align::Center);
                return;
            }
            _ => {}
        }

        let scroll = self.scroll.get(now);
        let first = (scroll / row_height).floor().max(0.0) as usize;
        let last = (((scroll + list.h) / row_height).ceil() as usize).min(self.pane.len());
        let zone = jiff::Zoned::now();
        let cursor = self.pane.cursor();
        let highlight = self.highlight.get(now);

        canvas.with_clip(list, |canvas| {
            // The cursor's highlight glides on its own, under the rows.
            if cursor.is_some() {
                let y = list.y + highlight - scroll;
                let rect = Rect::new(
                    list.x + space::SM,
                    y + 1.0,
                    list.w - space::SM * 2.0,
                    row_height - 2.0,
                );
                canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
                canvas.pill(
                    Rect::new(
                        rect.x + space::XS,
                        rect.y + rect.h * 0.25,
                        3.0,
                        rect.h * 0.5,
                    ),
                    accent,
                );
            }
            for row in first..last {
                let y = list.y + row as f32 * row_height - scroll;
                let rect = Rect::new(list.x, y, list.w, row_height);
                let columns = Columns::new(rect.inset_xy(space::LG, 0.0));
                if self.hover == Some(Hit::Row(row)) && cursor != Some(row) {
                    canvas.fill(
                        Rect::new(
                            list.x + space::SM,
                            y + 1.0,
                            list.w - space::SM * 2.0,
                            row_height - 2.0,
                        ),
                        radius::CONTROL,
                        colors.highlight_low,
                    );
                }
                let entry = *self.pane.entry(row);
                let dir = entry.kind.is_dir();
                let faded = |color: Rgba| {
                    if self.pane.is_hidden(row) {
                        color.alpha(0.7)
                    } else {
                        color
                    }
                };
                let (icon, icon_color): (Icon, Rgba) = if dir {
                    (icon::FOLDER, accent)
                } else {
                    (icon::FILE, colors.subtle)
                };
                canvas.icon(icon, 18.0, faded(icon_color), columns.icon, Align::Center);
                let name = self.pane.name(row);
                let name_text = Text::new(&name, body, faded(colors.text));
                canvas.text_in(&name_text, columns.name, Align::Start);
                if let Some(meta) = entry.meta {
                    if !dir {
                        let size = format::size(meta.size);
                        canvas.text_in(
                            &Text::new(&size, small, colors.subtle),
                            columns.size,
                            Align::End,
                        );
                    }
                    let when = format::when(meta.modified, &zone);
                    canvas.text_in(
                        &Text::new(&when, small, colors.subtle),
                        columns.date,
                        Align::Start,
                    );
                }
            }
        });
        for row in first..last {
            let y = list.y + row as f32 * row_height - scroll;
            let rect = Rect::new(
                list.x,
                y.max(list.y),
                list.w,
                (y + row_height).min(list.bottom()) - y.max(list.y),
            );
            if rect.h > 0.0 {
                self.hits.push((rect, Hit::Row(row)));
            }
        }
    }

    /// The filter at the right of the title row: a field while it's open
    /// or a filter is on, a quiet magnifier otherwise. Returns where it
    /// starts, for the path to stop short of.
    fn draw_filter(&mut self, canvas: &mut Canvas<'_>, area: Rect) -> f32 {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.font(type_scale::BODY);
        let height = 32.0;
        let open = self.filter.is_some() || !self.pane.filter().is_empty();
        let width = if open {
            (area.w * 0.32).clamp(160.0, 280.0)
        } else {
            height
        };
        let rect = Rect::new(
            area.right() - space::MD - width,
            area.y + (area.h - height) / 2.0,
            width,
            height,
        );
        let hovered = matches!(self.hover, Some(Hit::Filter(_)));
        if open {
            canvas.fill(rect, radius::pill(height), colors.surface);
            let ring = if self.filter.is_some() {
                accent
            } else {
                colors.highlight_med
            };
            canvas.stroke(rect, radius::pill(height), stroke::HAIRLINE, ring);
        } else if hovered {
            canvas.fill(rect, radius::pill(height), colors.highlight_low);
        }
        let icon_box = Rect::new(rect.x + space::SM, rect.y, 18.0, rect.h);
        canvas.icon(
            icon::SEARCH,
            16.0,
            colors.subtle,
            if open { icon_box } else { rect },
            if open { Align::Start } else { Align::Center },
        );
        let text_rect = Rect::new(
            icon_box.right() + space::SM,
            rect.y,
            (rect.right() - space::MD - icon_box.right() - space::SM).max(0.0),
            rect.h,
        );
        if open {
            let style = field::Style {
                size,
                color: colors.text,
                placeholder: "Filter",
                placeholder_color: colors.muted,
                caret: accent,
                selection: accent.alpha(opacity::TINT * 2.0),
            };
            let focused = self.filter.is_some();
            match &mut self.filter {
                Some(field) => field.draw(canvas, text_rect, &style, focused),
                None => {
                    canvas.text_in(
                        &Text::new(self.pane.filter(), size, colors.subtle),
                        text_rect,
                        Align::Start,
                    );
                }
            }
        }
        self.hits.push((rect, Hit::Filter(text_rect.x)));
        rect.x
    }

    fn draw_status(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = &self.ctx.theme.colors;
        let small = self.font(type_scale::SMALL);
        canvas.fill(
            Rect::new(area.x, area.y, area.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let inner = area.inset_xy(space::LG, 0.0);
        let mut left = match self.pane.load() {
            Load::Loading => "Loading…".to_owned(),
            Load::Failed(_) => String::new(),
            Load::Ready if !self.pane.filter().is_empty() => format!(
                "{} of {} match",
                self.pane.len(),
                format::count(self.pane.unfiltered_len(), "item", "items")
            ),
            Load::Ready => format::count(self.pane.len(), "item", "items"),
        };
        if self.pane.hidden_count() > 0 {
            left += &format!(" · {} hidden", self.pane.hidden_count());
        }
        canvas.text_in(&Text::new(&left, small, colors.subtle), inner, Align::Start);
        if let Ok(space) = places::space(self.pane.location()) {
            let free = format!("{} free", format::size(space.free));
            canvas.text_in(&Text::new(&free, small, colors.subtle), inner, Align::End);
        }
    }
}

/// Where each column of a row goes.
struct Columns {
    icon: Rect,
    name: Rect,
    size: Rect,
    date: Rect,
}

impl Columns {
    fn new(row: Rect) -> Self {
        let icon = Rect::new(row.x, row.y, 24.0, row.h);
        let date = Rect::new(row.right() - DATE_WIDTH, row.y, DATE_WIDTH, row.h);
        let size = Rect::new(date.x - space::XL - SIZE_WIDTH, row.y, SIZE_WIDTH, row.h);
        let name_x = icon.right() + space::MD;
        let name = Rect::new(name_x, row.y, (size.x - space::XL - name_x).max(0.0), row.h);
        Self {
            icon,
            name,
            size,
            date,
        }
    }
}

/// The crumbs of `path`: a label and where it goes, from the root (or from
/// home, shown as `~`) down.
fn crumbs(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs = Vec::new();
    let mut at = Some(path);
    while let Some(dir) = at {
        if dir == home {
            crumbs.push(("~".to_owned(), dir.to_owned()));
            break;
        }
        let label = match dir.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => "/".to_owned(),
        };
        crumbs.push((label, dir.to_owned()));
        at = dir.parent();
    }
    crumbs.reverse();
    crumbs
}

impl App for Files {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::window(
            WindowSpec {
                app_id: "enso-files".to_owned(),
                title: title(self.pane.location(), &self.home),
                min_size: (420, 300),
                parent: None,
            },
            self.size.0 as u32,
            self.size.1 as u32,
        )
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Some(request) = self.pending.take() {
            self.loader.load(request, &proxy);
        }
        let theme = proxy.clone();
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            theme.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-files: live theme switching unavailable: {err}");
        }
        self.proxy = Some(proxy);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let location = self.pane.location().to_owned();
        match event {
            Event::Message(Message::Listed { generation, result }) => {
                if self.pane.set_listing(generation, result) {
                    self.follow_cursor(now);
                    // Landing on a remembered row: be there, don't glide.
                    if let Some(row) = self.pane.cursor() {
                        let top = row as f32 * self.row_height();
                        self.highlight.jump(top);
                        self.scroll.jump(self.scroll.target());
                    }
                    cx.redraw();
                }
            }
            Event::Message(Message::Stats {
                generation,
                start,
                metas,
            }) => {
                if self.pane.apply_stats(generation, start, &metas) {
                    cx.redraw();
                }
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Key(key) => self.key(key, cx, now),
            Event::Pointer(pointer) => self.pointer(pointer, cx, now),
            Event::Resized { width, height } => {
                self.size = (width as f32, height as f32);
                cx.redraw();
            }
            Event::WindowState { maximized, .. } => {
                self.maximized = maximized;
                cx.redraw();
            }
            Event::Paste { data, .. } => {
                if let Some(field) = &mut self.filter {
                    field.insert(&String::from_utf8_lossy(&data));
                    let text = field.text().to_owned();
                    self.pane.set_filter(&text);
                    self.follow_cursor(now);
                    cx.redraw();
                }
            }
            Event::Close => cx.exit(),
            _ => {}
        }
        if self.pane.location() != location {
            cx.set_title(&title(self.pane.location(), &self.home));
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let bounds = canvas.bounds();
        canvas.clear(self.ctx.theme.colors.base);
        self.hits.clear();
        let title = Rect::new(0.0, 0.0, bounds.w, TITLE_HEIGHT);
        let status = Rect::new(0.0, bounds.h - STATUS_HEIGHT, bounds.w, STATUS_HEIGHT);
        let list = Rect::new(0.0, title.bottom(), bounds.w, status.y - title.bottom());
        self.draw_title(canvas, title);
        self.draw_list(canvas, list, now);
        self.draw_status(canvas, status);
        Paint::when(self.scroll.is_animating(now) || self.highlight.is_animating(now))
    }
}

/// The window title: the folder's name, and where it is.
fn title(path: &Path, home: &Path) -> String {
    let name = if path == home {
        "Home".to_owned()
    } else {
        path.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    format!("{name} — Files")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_core::Paths;
    use enso_fs::list::{Kind, Listing, Meta, Target};
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    /// A window onto a made-up folder, its listing and sizes arrived.
    fn files() -> Files {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-files-test"));
        let mut files = Files::new(ctx, FilesConfig::default(), "/home/me/dev/enso".into());
        files.home = "/home/me".into();
        let request = files.pending.take().unwrap();
        let entries = [
            ("crates", Kind::Dir),
            ("docs", Kind::Dir),
            ("target", Kind::Dir),
            ("latest", Kind::Link(Target::Dir)),
            (".git", Kind::Dir),
            ("Cargo.lock", Kind::File),
            ("Cargo.toml", Kind::File),
            (
                "a file with a name far too long to fit in its column at all.txt",
                Kind::File,
            ),
            ("README.md", Kind::File),
        ];
        let listing = Listing::from_entries("/home/me/dev/enso", entries);
        let now = jiff::Timestamp::now().as_nanosecond() as i64;
        let metas: Vec<Option<Meta>> = (0..entries.len())
            .map(|i| {
                Some(Meta {
                    size: 1500 * (i as u64 + 1).pow(4),
                    modified: now - (i as i64) * 7_200_000_000_000,
                    created: None,
                    accessed: now,
                    mode: 0o644,
                    uid: 1000,
                    gid: 1000,
                    link_size: None,
                })
            })
            .collect();
        assert!(files.pane.set_listing(request.generation, Ok(listing)));
        assert!(files.pane.apply_stats(request.generation, 0, &metas));
        files
    }

    fn render(files: &mut Files, size: (u32, u32)) -> Offscreen {
        let later = Instant::now() + Duration::from_secs(2);
        let mut text = TextEngine::new(&files.ctx.config.appearance);
        Offscreen::render(&mut text, size, 2, |canvas| {
            assert_eq!(files.draw(canvas, later), Paint::Idle);
        })
    }

    fn snapshot(frame: &Offscreen, name: &str) {
        frame
            .save_png(format!(
                "{}/../../target/snapshots/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
    }

    #[test]
    fn renders_a_folder() {
        let mut files = files();
        files.pane.set_cursor(1);
        let frame = render(&mut files, (900, 420));
        snapshot(&frame, "enso-files");
        // Every row on screen answers the pointer, below the title row.
        let rows: Vec<&Rect> = files
            .hits
            .iter()
            .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
            .map(|(rect, _)| rect)
            .collect();
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|rect| rect.y >= TITLE_HEIGHT));
        // The path's crumbs lead home and to dev; the last one is where we are.
        let crumbs: Vec<&Hit> = files
            .hits
            .iter()
            .map(|(_, hit)| hit)
            .filter(|hit| matches!(hit, Hit::Crumb(_)))
            .collect();
        assert_eq!(
            crumbs,
            [
                &Hit::Crumb("/home/me".into()),
                &Hit::Crumb("/home/me/dev".into())
            ]
        );
    }

    #[test]
    fn renders_the_filter() {
        let mut files = files();
        let mut field = TextField::new("carg");
        field.select(1..3);
        files.filter = Some(field);
        files.pane.set_filter("carg");
        let frame = render(&mut files, (900, 300));
        snapshot(&frame, "enso-files-filter");
        assert_eq!(files.pane.len(), 2);
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Filter(_)))
        );
    }

    #[test]
    fn renders_narrow_and_empty() {
        let mut files = files();
        let narrow = render(&mut files, (420, 300));
        snapshot(&narrow, "enso-files-narrow");

        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-files-test"));
        let mut empty = Files::new(ctx, FilesConfig::default(), "/home/me/new".into());
        let request = empty.pending.take().unwrap();
        let only_hidden = Listing::from_entries("/home/me/new", [(".keep", Kind::File)]);
        assert!(empty.pane.set_listing(request.generation, Ok(only_hidden)));
        snapshot(&render(&mut empty, (600, 300)), "enso-files-empty");
        assert!(!empty.hits.iter().any(|(_, hit)| matches!(hit, Hit::Row(_))));
    }

    #[test]
    fn crumbs_start_at_home_or_the_root() {
        let home = Path::new("/home/me");
        let labels = |path: &str| -> Vec<String> {
            crumbs(Path::new(path), home)
                .into_iter()
                .map(|(label, _)| label)
                .collect()
        };
        assert_eq!(labels("/home/me/dev/enso"), ["~", "dev", "enso"]);
        assert_eq!(labels("/home/me"), ["~"]);
        assert_eq!(labels("/usr/lib"), ["/", "usr", "lib"]);
        assert_eq!(labels("/"), ["/"]);
        assert_eq!(
            crumbs(Path::new("/home/me/dev"), home)[1].1,
            Path::new("/home/me/dev")
        );
    }

    #[test]
    fn titles_name_the_folder() {
        let home = Path::new("/home/me");
        assert_eq!(title(Path::new("/home/me"), home), "Home — Files");
        assert_eq!(title(Path::new("/home/me/dev"), home), "dev — Files");
        assert_eq!(title(Path::new("/"), home), "/ — Files");
    }
}
