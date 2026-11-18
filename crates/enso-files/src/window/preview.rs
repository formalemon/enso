//! Seeing a file without opening it: the preview pane at the side (`i`),
//! and a quick look over everything (`Shift+Space`), both following the
//! cursor. Pictures are shown whole, text files as their first lines in
//! the monospace face, folders as how much is in them; below, the details
//! — kind, size, dates, permissions, and where a link leads.

use std::path::PathBuf;

use enso_core::style::{icon, opacity, radius, space, stroke, type_scale};
use enso_fs::format;
use enso_ui::{Align, Canvas, Font, Image, Rect, Text};

use super::Files;
use crate::{jobs::Preview, kind::Kind};

/// How wide the pane is.
pub(super) const PANE: f32 = 320.0;

/// The preview in hand: for which file, and at what size.
#[derive(Debug, Default)]
pub(super) struct Shown {
    pub path: Option<PathBuf>,
    /// The longest side asked for: a quick look wants more than the pane.
    pub side: u32,
    pub preview: Option<Preview>,
    /// The picture, ready to draw.
    pub image: Option<Image>,
}

impl Files {
    /// Ask for the cursor's preview, if it isn't the one in hand (or is,
    /// but smaller than now wanted).
    fn want_preview(&mut self, side: u32) {
        let Some(row) = self.pane.cursor() else {
            return;
        };
        let path = self.pane.path(row);
        if self.shown.path.as_ref() == Some(&path) && self.shown.side >= side {
            return;
        }
        // Moved on: the old preview goes, so it can't sit under a new name.
        if self.shown.path.as_ref() != Some(&path) {
            self.shown = Shown::default();
        }
        self.shown.path = Some(path.clone());
        self.shown.side = side;
        if let Some(previewer) = &self.previewer {
            previewer.request(path, side);
        }
    }

    pub(super) fn preview_arrived(&mut self, path: PathBuf, preview: Preview) {
        if self.shown.path.as_ref() != Some(&path) {
            return;
        }
        self.shown.image = match &preview {
            Preview::Picture(picture) => Image::from_raw(
                picture.width,
                picture.height,
                picture.width as usize * 4,
                true,
                &picture.rgba,
            ),
            _ => None,
        };
        self.shown.preview = Some(preview);
    }

    /// The preview pane, in `area` at the right of the list.
    pub(super) fn draw_preview(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        canvas.fill(area, 0.0, colors.surface);
        canvas.fill(
            Rect::new(area.x, area.y, stroke::HAIRLINE, area.h),
            0.0,
            colors.highlight_low,
        );
        self.want_preview(((area.w - 2.0 * space::LG) * 2.0) as u32);
        let inner = area.inset(space::LG);
        let view = Rect::new(
            inner.x,
            inner.y,
            inner.w,
            (inner.w * 0.85).min(inner.h * 0.55),
        );
        self.draw_seen(canvas, view, false);
        let details = Rect::new(
            inner.x,
            view.bottom() + space::LG,
            inner.w,
            inner.bottom() - view.bottom() - space::LG,
        );
        self.draw_details(canvas, details);
    }

    /// The quick look: the preview large, on a card over everything.
    pub(super) fn draw_peek(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let colors = self.ctx.theme.colors.clone();
        canvas.fill(bounds, 0.0, colors.base.alpha(opacity::SCRIM));
        let card = bounds.centered(bounds.w * 0.8, bounds.h * 0.84);
        canvas.card(card, colors.surface, colors.highlight_med);
        let inner = card.inset(space::XL);
        self.want_preview((inner.w.max(inner.h) * 2.0) as u32);
        let details_width = (inner.w * 0.3).clamp(200.0, 300.0);
        let view = Rect::new(
            inner.x,
            inner.y,
            inner.w - details_width - space::XL,
            inner.h,
        );
        self.draw_seen(canvas, view, true);
        let details = Rect::new(view.right() + space::XL, inner.y, details_width, inner.h);
        self.draw_details(canvas, details);
    }

    /// The preview itself, in `view`.
    fn draw_seen(&mut self, canvas: &mut Canvas<'_>, view: Rect, large: bool) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let Some(row) = self.pane.cursor() else {
            return;
        };
        let entry = *self.pane.entry(row);
        let name = self.pane.name(row).into_owned();
        let executable = entry.meta.is_some_and(|meta| meta.mode & 0o111 != 0);
        let kind = Kind::of(&name, entry.kind.is_dir(), executable);
        let figure = |canvas: &mut Canvas<'_>| {
            let figure = if entry.kind.is_dir() {
                icon::FOLDER
            } else {
                icon::FILE
            };
            canvas.icon(
                figure,
                view.w.min(view.h) * 0.4,
                kind.color(&colors, accent),
                view,
                Align::Center,
            );
        };
        match (&self.shown.preview, &self.shown.image) {
            (Some(Preview::Picture(_)), Some(image)) => {
                let (w, h) = image.size();
                let scale =
                    (view.w / w as f32)
                        .min(view.h / h as f32)
                        .min(if large { 4.0 } else { 2.0 });
                let (dw, dh) = (w as f32 * scale, h as f32 * scale);
                let fitted = Rect::new(
                    view.x + (view.w - dw) / 2.0,
                    view.y + (view.h - dh) / 2.0,
                    dw,
                    dh,
                );
                canvas.image(image, fitted, radius::CONTROL);
            }
            (Some(Preview::Text(lines)), _) => {
                canvas.fill(view, radius::CONTROL, colors.base);
                let size = self.font(
                    if large {
                        type_scale::BODY
                    } else {
                        type_scale::SMALL
                    } * 0.92,
                );
                let line = (size * 1.45).round();
                let text = view.inset(space::MD);
                canvas.with_clip(text, |canvas| {
                    for (index, content) in lines.iter().enumerate() {
                        let y = text.y + index as f32 * line;
                        if y > text.bottom() {
                            break;
                        }
                        let label = Text::new(content, size, colors.text)
                            .font(Font::Mono)
                            .max_width(text.w);
                        canvas.text(&label, text.x, y);
                    }
                });
            }
            (Some(Preview::Folder(count)), _) => {
                figure(canvas);
                let label = format::count(*count, "item", "items");
                let below = Rect::new(view.x, view.bottom() - 28.0, view.w, 24.0);
                canvas.text_in(
                    &Text::new(&label, self.font(type_scale::SMALL), colors.subtle),
                    below,
                    Align::Center,
                );
            }
            _ => figure(canvas),
        }
    }

    /// The details: name, kind, size, dates, permissions, a link's target.
    fn draw_details(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let Some(row) = self.pane.cursor() else {
            return;
        };
        let colors = &self.ctx.theme.colors;
        let small = self.font(type_scale::SMALL);
        let entry = *self.pane.entry(row);
        let name = self.pane.name(row);
        let path = self.pane.path(row);
        let title = Text::new(&name, self.font(type_scale::TITLE), colors.text)
            .medium()
            .max_width(area.w)
            .max_lines(3);
        let height = canvas.text(&title, area.x, area.y).h;
        let mut y = area.y + height + space::MD;
        let zone = jiff::Zoned::now().time_zone().clone();
        let executable = entry.meta.is_some_and(|meta| meta.mode & 0o111 != 0);
        let kind = Kind::of(&name, entry.kind.is_dir(), executable);
        let mut rows: Vec<(&str, String)> = vec![("Kind", describe(kind, entry.kind.is_link()))];
        if let Some(meta) = entry.meta {
            if !entry.kind.is_dir() {
                rows.push((
                    "Size",
                    format!("{} ({} bytes)", format::size(meta.size), group(meta.size)),
                ));
            }
            rows.push(("Modified", format::when_exactly(meta.modified, &zone)));
            if let Some(created) = meta.created {
                rows.push(("Created", format::when_exactly(created, &zone)));
            }
            rows.push(("Permissions", permissions(meta.mode)));
        }
        if entry.kind.is_link()
            && let Ok(target) = std::fs::read_link(&path)
        {
            rows.push(("Leads to", target.to_string_lossy().into_owned()));
        }
        let label_width = 92.0;
        for (label, value) in rows {
            if y > area.bottom() - 20.0 {
                break;
            }
            canvas.text(&Text::new(label, small, colors.muted), area.x, y);
            let value = Text::new(&value, small, colors.subtle)
                .max_width(area.w - label_width)
                .max_lines(2);
            let height = canvas.text(&value, area.x + label_width, y).h;
            y += height.max(small * 1.4) + space::XS;
        }
    }
}

fn describe(kind: Kind, link: bool) -> String {
    let word = match kind {
        Kind::Folder => "Folder",
        Kind::Image => "Picture",
        Kind::Video => "Video",
        Kind::Audio => "Sound",
        Kind::Archive => "Archive",
        Kind::Code => "Code or settings",
        Kind::Document => "Document",
        Kind::Program => "Program",
        Kind::Other => "File",
    };
    if link {
        format!("{word} (a link)")
    } else {
        word.to_owned()
    }
}

/// `rwxr-x---` and the number: who may read, write, and run it.
fn permissions(mode: u32) -> String {
    let bits = ['r', 'w', 'x'];
    let text: String = (0..9)
        .map(|i| {
            if mode & (0o400 >> i) != 0 {
                bits[i % 3]
            } else {
                '-'
            }
        })
        .collect();
    format!("{text}  {:o}", mode & 0o7777)
}

/// `1234567` as `1 234 567`.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_and_numbers_read_plainly() {
        assert_eq!(permissions(0o755), "rwxr-xr-x  755");
        assert_eq!(permissions(0o640), "rw-r-----  640");
        assert_eq!(group(1_234_567), "1\u{202f}234\u{202f}567");
        assert_eq!(group(999), "999");
    }
}
