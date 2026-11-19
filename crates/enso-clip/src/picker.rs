//! The card `enso-clip pick` opens (Mod+V): what was copied, newest
//! first, with a field to narrow it down. Enter puts the chosen one back on
//! the clipboard, ready to paste; Delete forgets it.

use std::{path::PathBuf, time::Instant};

use enso_core::{
    Context, Transition, Tween, ipc,
    style::{opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App, Button, Canvas, Cx, Event, Image, Key, Nav, Paint, PointerEvent, Presence, Proxy,
    Rect, SurfaceSpec, TEXT_TYPES, Text,
    widget::{TextField, field},
};

use crate::{
    daemon::NAME as DAEMON,
    store::{self, Clip},
};

/// The name the card listens under, so a second Mod+V closes it.
pub const NAME: &str = "enso-clip-pick";

const WIDTH: f32 = 560.0;
const SEARCH_HEIGHT: f32 = 56.0;
const ROW_HEIGHT: f32 = 48.0;
const ROWS: usize = 8;
const THUMB: f32 = 32.0;

#[derive(Debug)]
pub enum Message {
    Dismiss,
}

/// One thing in the history, ready to show.
#[derive(Debug)]
struct Entry {
    id: u64,
    /// The first line of a text, or what a picture is.
    title: String,
    /// More about it: how many lines, how big.
    detail: String,
    /// Lowercased, for matching.
    haystack: String,
    thumb: Option<Image>,
}

#[derive(Debug)]
pub struct Picker {
    ctx: Context,
    presence: Presence,
    field: TextField,
    entries: Vec<Entry>,
    /// Indices into `entries` that match what's typed.
    shown: Vec<usize>,
    selected: usize,
    offset: usize,
    highlight: Tween<f32>,
    /// The daemon is running.
    running: bool,
    card: Rect,
    rows: Vec<Rect>,
}

fn entry(dir: &std::path::Path, id: u64) -> Option<Entry> {
    match store::read(dir, id)? {
        Clip::Text(text) => {
            let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
            let title: String = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            let detail = match lines {
                0 | 1 => String::new(),
                n => format!("{n} lines"),
            };
            Some(Entry {
                id,
                haystack: text.to_lowercase(),
                title,
                detail,
                thumb: None,
            })
        }
        Clip::Png(_) => {
            let path: PathBuf = dir.join(format!("{id}.png"));
            let pixels = enso_fs::picture::open(&path, 128).ok();
            let size = enso_fs::picture::open(&path, u32::MAX)
                .ok()
                .map(|full| format!("{}×{}", full.width, full.height))
                .unwrap_or_default();
            let thumb = pixels.and_then(|p| {
                Image::from_raw(p.width, p.height, p.width as usize * 4, true, &p.rgba)
            });
            Some(Entry {
                id,
                title: "Picture".to_owned(),
                detail: size,
                haystack: "picture image screenshot".to_owned(),
                thumb,
            })
        }
    }
}

impl Picker {
    pub fn new(ctx: Context) -> Self {
        let entries = store::dir()
            .map(|dir| {
                store::list(&dir)
                    .into_iter()
                    .filter_map(|id| entry(&dir, id))
                    .collect()
            })
            .unwrap_or_default();
        let mut picker = Self {
            presence: Presence::new(&ctx.motion),
            ctx,
            field: TextField::default(),
            entries,
            shown: Vec::new(),
            selected: 0,
            offset: 0,
            highlight: Tween::new(0.0),
            running: ipc::is_listening(DAEMON),
            card: Rect::default(),
            rows: Vec::new(),
        };
        picker.filter();
        picker
    }

    fn filter(&mut self) {
        let query = self.field.text().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        self.shown = (0..self.entries.len())
            .filter(|&i| {
                words
                    .iter()
                    .all(|word| self.entries[i].haystack.contains(word))
            })
            .collect();
        self.select(0);
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.shown.len().saturating_sub(1));
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + ROWS {
            self.offset = self.selected + 1 - ROWS;
        }
        self.highlight.set(
            (self.selected - self.offset) as f32,
            self.ctx.motion.apply(Transition::SELECT),
            Instant::now(),
        );
    }

    fn chosen(&self) -> Option<&Entry> {
        self.shown.get(self.selected).map(|&i| &self.entries[i])
    }

    /// Put the chosen one on the clipboard, by way of the daemon.
    fn choose(&mut self, cx: &mut Cx<'_>) {
        if let Some(entry) = self.chosen() {
            ipc::tell(DAEMON, &format!("use {}", entry.id));
            self.presence.close(cx);
        }
    }

    /// Forget the chosen one.
    fn forget(&mut self) {
        let Some(&index) = self.shown.get(self.selected) else {
            return;
        };
        let entry = self.entries.remove(index);
        ipc::tell(DAEMON, &format!("delete {}", entry.id));
        let selected = self.selected;
        self.filter();
        self.select(selected);
    }

    fn list_height(&self) -> f32 {
        ROW_HEIGHT * self.shown.len().clamp(1, ROWS) as f32 + space::SM
    }
}

impl App for Picker {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay(NAME)
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Err(err) = ipc::listen(NAME, move |word| {
            word != "close" || proxy.send(Message::Dismiss)
        }) {
            eprintln!("enso-clip: a second press cannot close this one: {err}");
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Wake if self.presence.is_closing() => cx.exit(),
            Event::Message(Message::Dismiss) => self.presence.close(cx),
            Event::Key(key) => {
                if let Some(nav) = key.nav(false) {
                    let count = self.shown.len().max(1);
                    let next = match nav {
                        Nav::Down => (self.selected + 1) % count,
                        Nav::Up => (self.selected + count - 1) % count,
                    };
                    self.select(next);
                } else {
                    match key.key {
                        Key::Escape => return self.presence.close(cx),
                        Key::Enter => return self.choose(cx),
                        Key::Delete if self.field.is_empty() || key.mods.shift => self.forget(),
                        _ => match self.field.key(&key) {
                            field::Response::Changed | field::Response::Cut(_) => self.filter(),
                            field::Response::Paste => cx.paste(&TEXT_TYPES),
                            field::Response::Ignored => return,
                            _ => {}
                        },
                    }
                }
                cx.redraw();
            }
            Event::Paste { data, .. } => {
                let text = String::from_utf8_lossy(&data);
                self.field
                    .insert(&text.split_whitespace().collect::<Vec<_>>().join(" "));
                self.filter();
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Move { x, y }) => {
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y))
                    && self.offset + row != self.selected
                {
                    self.select(self.offset + row);
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Scroll { dy, .. }) => {
                let count = self.shown.len();
                if count > 0 {
                    let step = if dy > 0.0 { 1 } else { count - 1 };
                    self.select((self.selected + step) % count);
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y)) {
                    self.select(self.offset + row);
                    self.choose(cx);
                } else if !self.card.contains(x, y) {
                    self.presence.close(cx);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.ctx.config.appearance.font_size;
        let title = size * type_scale::TITLE;
        let bounds = canvas.bounds();
        canvas.clear(colors.base.alpha(opacity::SCRIM * presence));

        let width = WIDTH.min(bounds.w - 2.0 * space::LG);
        let card = Rect::new(
            (bounds.w - width) / 2.0,
            (bounds.h * 0.2).round(),
            width,
            SEARCH_HEIGHT + self.list_height(),
        )
        .translate(0.0, (presence - 1.0) * space::MD);
        self.card = card;
        canvas.set_opacity(presence);
        canvas.card(card, colors.surface, colors.highlight_med);

        let search = Rect::new(card.x, card.y, card.w, SEARCH_HEIGHT).inset_xy(space::LG, 0.0);
        let style = field::Style {
            size: title,
            color: colors.text,
            placeholder: "Search what was copied",
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        self.field.draw(canvas, search, &style, true);
        canvas.fill(
            Rect::new(
                card.x + space::LG,
                card.y + SEARCH_HEIGHT - 0.5,
                card.w - 2.0 * space::LG,
                stroke::HAIRLINE,
            ),
            0.0,
            colors.highlight_med,
        );

        let list = Rect::new(
            card.x + space::SM,
            card.y + SEARCH_HEIGHT + space::XS,
            card.w - 2.0 * space::SM,
            0.0,
        );
        self.rows.clear();
        if self.shown.is_empty() {
            let row = list.row(0, ROW_HEIGHT, 0.0).inset_xy(space::SM, 0.0);
            let said = if !self.running {
                "enso-clip isn't running: start it with the session"
            } else if self.entries.is_empty() {
                "Nothing copied yet"
            } else {
                "Nothing like that was copied"
            };
            canvas.text_in(&Text::new(said, size, colors.muted), row, Align::Start);
        } else {
            let highlight = list
                .row(0, ROW_HEIGHT, 0.0)
                .translate(0.0, self.highlight.get(now) * ROW_HEIGHT);
            canvas.fill(highlight, radius::CONTROL, colors.overlay);
            for (slot, &index) in self.shown.iter().skip(self.offset).take(ROWS).enumerate() {
                let row = list.row(slot, ROW_HEIGHT, 0.0);
                self.rows.push(row);
                let content = row.inset_xy(space::SM, 0.0);
                let entry = &self.entries[index];
                let mut x = content.x;
                if let Some(thumb) = &entry.thumb {
                    let (w, h) = thumb.size();
                    let aspect = w as f32 / h.max(1) as f32;
                    let w = (THUMB * aspect).min(THUMB * 2.0);
                    let rect = Rect::new(x, content.y + (content.h - THUMB) / 2.0, w, THUMB);
                    canvas.image(thumb, rect, radius::CONTROL / 2.0);
                    x += w + space::MD;
                }
                let selected = self.offset + slot == self.selected;
                let mut right = content.right();
                if selected {
                    let hint = Text::new(
                        "Enter copies · Delete forgets",
                        size * type_scale::SMALL,
                        colors.subtle,
                    );
                    let w = canvas.measure(&hint).w;
                    canvas.text_in(&hint, content, Align::End);
                    right -= w + space::MD;
                }
                let name =
                    Text::new(&entry.title, size, colors.text).max_width((right - x).max(1.0));
                let used = canvas
                    .text_in(
                        &name,
                        Rect::new(x, content.y, right - x, content.h),
                        Align::Start,
                    )
                    .w;
                if !entry.detail.is_empty() && right - (x + used + space::MD) > 40.0 {
                    let rest = Rect::new(
                        x + used + space::MD,
                        content.y,
                        right - x - used - space::MD,
                        content.h,
                    );
                    canvas.text_in(
                        &Text::new(&entry.detail, size * type_scale::SMALL, colors.muted),
                        rest,
                        Align::Start,
                    );
                }
            }
        }
        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now) || self.highlight.is_animating(now))
    }
}
