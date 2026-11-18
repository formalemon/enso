//! The grid: each entry a tile, pictures as their thumbnails, everything
//! else as a large figure in its kind's colour. Four sizes (`Ctrl+=`,
//! `Ctrl+-`, `Ctrl+0`, `Ctrl`+wheel).
//!
//! Thumbnails are asked for as tiles come into view, newest first, and kept
//! in memory while the folder is shown; the freedesktop cache keeps them
//! between runs.

use std::{collections::HashMap, path::PathBuf, time::Instant};

use enso_core::style::{icon, opacity, radius, space, type_scale};
use enso_fs::thumbs::{Size, Thumb};
use enso_ui::{Align, Canvas, Image, Rect, Text};

use super::{Files, Hit};
use crate::{config::View, kind::Kind};

/// Tile sizes: the picture's square, in logical pixels.
pub(super) const ZOOMS: [f32; 4] = [88.0, 120.0, 168.0, 232.0];
/// The default size.
pub(super) const ZOOM: usize = 1;

/// Thumbnails in memory: each with the size it was made at, or `None` if
/// there's none to be had.
#[derive(Debug, Default)]
pub(super) struct Thumbs {
    ready: HashMap<PathBuf, (Size, Option<Image>)>,
    /// Asked for, not yet back.
    asked: HashMap<PathBuf, Size>,
}

impl Thumbs {
    /// Forget everything not in `folder`: thumbnails are for what's shown.
    pub fn keep_only(&mut self, folder: &std::path::Path) {
        self.ready.retain(|path, _| path.parent() == Some(folder));
        self.asked.clear();
    }

    pub fn get(&self, path: &std::path::Path) -> Option<&Image> {
        self.ready.get(path).and_then(|(_, image)| image.as_ref())
    }

    /// Whether `path` needs asking for at `size`.
    fn wanted(&self, path: &std::path::Path, size: Size) -> bool {
        let made = |have: Size| have.pixels() >= size.pixels();
        !self.asked.get(path).is_some_and(|asked| made(*asked))
            && !self.ready.get(path).is_some_and(|(have, _)| made(*have))
    }

    pub fn arrived(&mut self, path: PathBuf, thumb: Option<Thumb>) {
        let size = self.asked.remove(&path).unwrap_or(Size::Normal);
        let image = thumb.and_then(|thumb| {
            Image::from_raw(
                thumb.width,
                thumb.height,
                thumb.width as usize * 4,
                true,
                &thumb.rgba,
            )
        });
        self.ready.insert(path, (size, image));
    }
}

impl Files {
    pub(super) fn tile(&self) -> (f32, f32) {
        let zoom = ZOOMS[self.zoom];
        (zoom + 24.0, zoom + 56.0)
    }

    /// Tiles across.
    pub(super) fn columns(&self) -> usize {
        let (width, _) = self.tile();
        (((self.list.w - 2.0 * space::LG) / width).floor() as usize).max(1)
    }

    /// Where row `row` is, down the view, and how tall: a list row, or the
    /// grid row the entry is in.
    pub(super) fn span_of(&self, index: usize) -> (f32, f32) {
        match self.config.view {
            View::List | View::Columns => (index as f32 * self.row_height(), self.row_height()),
            View::Grid => {
                let (_, height) = self.tile();
                ((index / self.columns()) as f32 * height + space::SM, height)
            }
        }
    }

    pub(super) fn zoom_by(&mut self, step: isize) {
        self.zoom = self.zoom.saturating_add_signed(step).min(ZOOMS.len() - 1);
    }

    /// Ask for the thumbnail of `path`, if it's of a kind that has one and
    /// it isn't in hand or on its way.
    fn want_thumb(&mut self, path: PathBuf, kind: Kind) {
        if !matches!(kind, Kind::Image | Kind::Video) {
            return;
        }
        let size = Size::for_pixels(ZOOMS[self.zoom]);
        if !self.thumbs.wanted(&path, size) {
            return;
        }
        // Pictures are made here; for the rest, a thumbnail another program
        // made is used if there is one.
        if let Some(thumbnailer) = &self.thumbnailer {
            self.thumbs.asked.insert(path.clone(), size);
            thumbnailer.request(path, size);
        }
    }

    /// The grid. Returns whether anything is moving.
    pub(super) fn draw_grid(&mut self, canvas: &mut Canvas<'_>, list: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let small = self.font(type_scale::SMALL);
        let (cell_w, cell_h) = self.tile();
        let columns = self.columns();
        let rows = self.pane.len().div_ceil(columns);
        self.scroll
            .set_extent(rows as f32 * cell_h + 2.0 * space::SM, list.h);
        let scroll = self.scroll.get(now);
        let first = ((scroll / cell_h).floor().max(0.0) as usize) * columns;
        let last =
            ((((scroll + list.h) / cell_h).ceil() as usize + 1) * columns).min(self.pane.len());
        // Tiles are spread across the width, the gaps shared out evenly.
        let spare = (list.w - 2.0 * space::LG - columns as f32 * cell_w) / columns as f32;
        let x0 = list.x + space::LG + spare / 2.0;
        let cursor = self.pane.cursor();
        let zoom = ZOOMS[self.zoom];
        let mut wanted = Vec::new();
        let mut hits = Vec::new();

        canvas.with_clip(list, |canvas| {
            for index in first..last {
                let (col, row) = (index % columns, index / columns);
                let x = x0 + col as f32 * (cell_w + spare);
                let y = list.y + space::SM + row as f32 * cell_h - scroll;
                let tile = Rect::new(x, y, cell_w, cell_h - space::SM);
                let path = self.pane.path(index);
                let entry = *self.pane.entry(index);
                let selected = self.selection.contains(&path);
                if cursor == Some(index) && self.behind {
                    canvas.stroke(tile, radius::CARD, 1.0, colors.highlight_high);
                } else if cursor == Some(index) {
                    canvas.fill(tile, radius::CARD, accent.alpha(opacity::TINT));
                    canvas.stroke(tile, radius::CARD, 1.5, accent.alpha(0.6));
                } else if selected {
                    canvas.fill(tile, radius::CARD, colors.iris.alpha(opacity::TINT));
                } else if self.hover == Some(Hit::Row(index)) {
                    canvas.fill(tile, radius::CARD, colors.highlight_low);
                }
                let executable = entry.meta.is_some_and(|meta| meta.mode & 0o111 != 0);
                let full = self.pane.name(index);
                // A gathered list's names are paths: the tile shows its own.
                let name = full.rsplit('/').next().unwrap_or(&full).to_owned();
                let kind = Kind::of(&name, entry.kind.is_dir(), executable);
                let picture = Rect::new(x + (cell_w - zoom) / 2.0, y + space::SM, zoom, zoom);
                match self.thumbs.get(&path) {
                    Some(image) => {
                        // Contained, not cropped: the whole picture shows.
                        let (w, h) = image.size();
                        let scale = (zoom / w as f32).min(zoom / h as f32).min(4.0);
                        let (dw, dh) = (w as f32 * scale, h as f32 * scale);
                        let fitted = Rect::new(
                            picture.x + (zoom - dw) / 2.0,
                            picture.y + (zoom - dh) / 2.0,
                            dw,
                            dh,
                        );
                        canvas.image(image, fitted, 6.0);
                    }
                    None => {
                        let figure = if entry.kind.is_dir() {
                            icon::FOLDER
                        } else {
                            icon::FILE
                        };
                        canvas.icon(
                            figure,
                            zoom * 0.5,
                            kind.color(&colors, accent),
                            picture,
                            Align::Center,
                        );
                    }
                }
                if selected {
                    let check = Rect::new(tile.right() - 26.0, tile.y + 6.0, 20.0, 20.0);
                    canvas.circle(check.center().0, check.center().1, 10.0, colors.iris);
                    canvas.icon(icon::CHECK, 14.0, colors.base, check, Align::Center);
                }
                let label = Rect::new(
                    x + 4.0,
                    picture.bottom() + space::XS,
                    cell_w - 8.0,
                    cell_h - zoom - 2.0 * space::SM,
                );
                let text = Text::new(&name, small, colors.text)
                    .max_width(label.w)
                    .max_lines(2);
                let size = canvas.measure(&text);
                canvas.text(&text, label.x + (label.w - size.w) / 2.0, label.y);
                wanted.push((path, kind));
                hits.push((
                    Rect::new(
                        tile.x,
                        tile.y.max(list.y),
                        tile.w,
                        (tile.bottom().min(list.bottom()) - tile.y.max(list.y)).max(0.0),
                    ),
                    Hit::Row(index),
                ));
            }
        });
        // Newest last, so the first tiles come first.
        for (path, kind) in wanted.into_iter().rev() {
            self.want_thumb(path, kind);
        }
        self.hits
            .extend(hits.into_iter().filter(|(rect, _)| rect.h > 0.0));
        let hovered_bar = self.hover == Some(Hit::ScrollBar);
        self.scroll
            .draw_bar(canvas, list, colors.subtle, hovered_bar, now);
        if self.scroll.max() > 0.0 {
            self.hits.push((
                Rect::new(list.right() - 14.0, list.y, 14.0, list.h),
                Hit::ScrollBar,
            ));
        }
        self.scroll.is_animating(now)
    }
}
