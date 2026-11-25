//! The card `enso-clip pick` opens (Mod+V): what was copied, newest
//! first, with a field to narrow it down. Enter puts the chosen one back on
//! the clipboard, ready to paste; Delete forgets it.
//!
//! Like the launcher, it grows out of the bar's clock when there is a bar
//! along the top of the screen, with nothing dimmed behind it
//! ([`enso_ui::emerge`]).

use std::{path::PathBuf, time::Instant};

use enso_core::{
    Context, ipc,
    style::{self, opacity, radius, space, type_scale},
};
use enso_ui::{
    Align, App, Button, Canvas, Cx, Event, Image, Key, Paint, PointerEvent, Presence, Proxy, Rect,
    SurfaceSpec, TEXT_TYPES, Text,
    emerge::Perch,
    widget::{
        TextField, field,
        search::{self, List},
    },
};

use crate::{
    daemon::NAME as DAEMON,
    store::{self, Clip},
};

/// The name the card listens under, so a second Mod+V closes it.
pub const NAME: &str = "enso-clip-pick";

// The launcher's measures: the same card.
const WIDTH: f32 = style::search::WIDTH;
const SEARCH_HEIGHT: f32 = style::search::FIELD;
const ROWS: usize = 8;
const THUMB: f32 = style::search::CHIP;

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
    /// Which of them is chosen and which are in view, as every search card
    /// has it.
    list: List,
    /// The daemon is running.
    running: bool,
    card: Rect,
    /// The screen to open on: the one with the focus.
    screen: Option<String>,
    /// Out of the bar's clock, when there is a bar along that screen's top.
    perch: Perch,
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
        Self::with(ctx, entries, ipc::is_listening(DAEMON))
    }

    /// The card over `entries`; `running` says whether the daemon is.
    fn with(ctx: Context, entries: Vec<Entry>, running: bool) -> Self {
        let mut picker = Self {
            presence: Presence::new(&ctx.motion),
            list: List::new(ROWS, &ctx.motion),
            ctx,
            field: TextField::default(),
            entries,
            shown: Vec::new(),
            running,
            card: Rect::default(),
            screen: None,
            perch: Perch::default(),
        };
        picker.filter();
        picker
    }

    /// Open on `screen`, out of the bar's clock there if `perch` has it.
    #[must_use]
    pub fn on(mut self, screen: Option<String>, perch: Perch) -> Self {
        self.screen = screen;
        self.presence = perch.presence(&self.ctx.motion);
        self.perch = perch;
        self
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
        self.list.reset(self.shown.len());
        // Always room for a line, if only to say there's nothing.
        self.list.grow(search::height(self.list.visible().max(1)));
    }

    fn chosen(&self) -> Option<&Entry> {
        let &index = self.shown.get(self.list.selected()?)?;
        self.entries.get(index)
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
        let Some(selected) = self.list.selected() else {
            return;
        };
        let entry = self.entries.remove(self.shown[selected]);
        ipc::tell(DAEMON, &format!("delete {}", entry.id));
        self.filter();
        self.list.select(selected);
    }
}

impl App for Picker {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        Perch::surface(NAME, self.screen.clone())
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
                    self.list.step(nav);
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
                if let Some(index) = self.list.at(x, y)
                    && Some(index) != self.list.selected()
                {
                    self.list.select(index);
                    cx.redraw();
                }
            }
            // The wheel moves the choice, a row a turn.
            Event::Pointer(PointerEvent::Scroll { dy, .. }) => {
                self.list.scroll(dy);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(index) = self.list.at(x, y) {
                    self.list.select(index);
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
        let look = search::Look {
            colors: colors.clone(),
            accent: self.ctx.accent(),
            size: self.ctx.config.appearance.font_size,
        };
        let bounds = canvas.bounds();
        // Nothing behind the card: the canvas starts each frame clear.
        let width = WIDTH.min(bounds.w - 2.0 * space::LG);
        let list_height = self.list.height(now);
        let rest = self.perch.rest(bounds, width, SEARCH_HEIGHT + list_height);
        let card = self.perch.card(canvas, rest, presence, &colors);
        self.card = card;

        let placeholder = "Search what was copied · Delete forgets";
        search::head(canvas, card, &mut self.field, placeholder, list_height, &look);

        let rows = self.list.draw(canvas, card, now, &look);
        if self.shown.is_empty() {
            let said = if !self.running {
                "enso-clip isn't running: start it with the session"
            } else if self.entries.is_empty() {
                "Nothing copied yet"
            } else {
                "Nothing like that was copied"
            };
            search::note(canvas, card, said, &look);
        }
        for row in rows {
            let entry = &self.entries[self.shown[row.index]];
            let chip = search::chip(row.content);
            // A picture shows itself where a row's chip goes, as wide as it
            // needs, up to two chips; words get a chip of their own.
            let from = match &entry.thumb {
                Some(thumb) => {
                    let (w, h) = thumb.size();
                    let aspect = w as f32 / h.max(1) as f32;
                    let wide = (THUMB * aspect).min(THUMB * 2.0);
                    canvas.image(thumb, Rect::new(chip.x, chip.y, wide, chip.h), radius::CONTROL);
                    chip.x + wide
                }
                None => {
                    canvas.fill(chip, radius::CONTROL, colors.subtle.alpha(opacity::TINT));
                    let mark = Text::new("Aa", look.size * type_scale::SMALL, colors.subtle);
                    canvas.text_in(&mark.medium(), chip, Align::Center);
                    chip.right()
                }
            } + space::MD;
            let detail = Some(entry.detail.as_str());
            let (title, chosen) = (entry.title.as_str(), row.chosen);
            search::words(canvas, row.content, from, title, detail, Some("Copy"), chosen, &look);
        }
        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now) || self.list.is_animating(now))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_core::{Paths, island::Island};
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    fn text(id: u64, title: &str, detail: &str) -> Entry {
        Entry {
            id,
            title: title.to_owned(),
            detail: detail.to_owned(),
            haystack: title.to_lowercase(),
            thumb: None,
        }
    }

    /// The history opening out of the bar's clock, a frame at a time, to
    /// look at: `target/snapshots/clip-emerge.png`.
    #[test]
    fn the_history_opens_out_of_the_bars_clock() {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-clip-test"));
        let entries = vec![
            text(3, "cargo build --release -p enso-boot", ""),
            text(2, "https://github.com/formalemon/enso", ""),
            text(1, "fn main() {", "3 lines"),
        ];
        let island = Island {
            pid: 1,
            top: true,
            bar: 36.0,
            x: 330.0,
            y: 4.0,
            w: 140.0,
            h: 28.0,
            opacity: 0.9,
        };
        let perch = Perch {
            island: Some(island),
        };
        let mut picker = Picker::with(ctx, entries, true).on(None, perch);
        let colors = picker.ctx.theme.colors.clone();
        let mut engine = TextEngine::new(&picker.ctx.config.appearance);
        let (width, height) = (800u32, 260u32);
        let desk = Offscreen::render(&mut engine, (width, height), 1, |canvas| {
            canvas.clear(colors.pine.mix(colors.overlay, 0.6));
            let clock = Rect::new(island.x, island.y, island.w, island.h);
            canvas.pill(clock, colors.base.alpha(island.opacity));
            let words = Text::new("16:52", 13.0, colors.text).medium();
            canvas.text_in(&words, clock, Align::Center);
        });
        let start = Instant::now();
        let moments = [0, 40, 80, 130, 200, 300, 420];
        let mut sheet = Vec::new();
        for ms in moments {
            let frame = Offscreen::render(&mut engine, (width, height), 1, |canvas| {
                picker.draw(canvas, start + Duration::from_millis(ms));
            });
            let mut over = desk.rgba.clone();
            for (dst, src) in over.chunks_mut(4).zip(frame.rgba.chunks(4)) {
                let keep = 255 - u32::from(src[3]);
                for c in 0..4 {
                    dst[c] = (u32::from(src[c]) + (u32::from(dst[c]) * keep + 127) / 255) as u8;
                }
            }
            sheet.extend(over);
        }
        Offscreen {
            width,
            height: height * moments.len() as u32,
            rgba: sheet,
        }
        .snapshot(env!("CARGO_MANIFEST_DIR"), "clip-emerge");
        // At rest it hangs under the bar, in the middle, three rows long.
        assert_eq!(picker.card.y, 40.0);
        assert_eq!(picker.card.center().0, 400.0);
        assert_eq!(picker.list.drawn().len(), 3);
    }
}
