//! Columns (`Ctrl+3`), as in Finder, ranger, and yazi: the folder above on
//! the left with this one marked, this folder in the middle — the list, with
//! all its keys — and on the right what's under the cursor: a folder's
//! contents, or a file's preview. `h` and `l` (and the arrows) move along
//! them.
//!
//! The side columns are names only, read when the folder or the cursor
//! changes and kept until they change again; a very large folder shows its
//! first few hundred.

use std::path::{Path, PathBuf};

use enso_core::style::{icon, opacity, radius, space, stroke, type_scale};
use enso_fs::{
    list::Listing,
    sort::{self, Keys, Sort},
};
use enso_ui::{Align, Canvas, Rect, Text};

use super::{Files, Hit};
use crate::kind::Kind;

/// Entries a side column shows at most.
const SHOWN: usize = 500;

/// A side column's folder, read: each entry's name and whether it's a folder.
#[derive(Debug, Default)]
pub(super) struct Side {
    folder: Option<PathBuf>,
    entries: Vec<(String, bool)>,
}

impl Side {
    /// Read `folder` if it isn't the one in hand.
    fn read(&mut self, folder: &Path, hidden: bool, sort: Sort) {
        if self.folder.as_deref() == Some(folder) {
            return;
        }
        self.folder = Some(folder.to_owned());
        self.entries = names(folder, hidden, sort);
    }
}

fn names(folder: &Path, hidden: bool, sort: Sort) -> Vec<(String, bool)> {
    let Ok(listing) = Listing::read(folder) else {
        return Vec::new();
    };
    let keys = Keys::new(&listing);
    sort::order(&listing, &keys, sort)
        .into_iter()
        .map(|index| index as usize)
        .filter(|&index| hidden || !listing.is_hidden(index))
        .take(SHOWN)
        .map(|index| {
            (
                listing.display_name(index).into_owned(),
                listing.entry(index).kind.is_dir(),
            )
        })
        .collect()
}

impl Files {
    /// The columns: `area` split three ways, the middle drawn as the list.
    /// Returns whether anything is moving.
    pub(super) fn draw_columns(
        &mut self,
        canvas: &mut Canvas<'_>,
        area: Rect,
        now: std::time::Instant,
    ) -> bool {
        let left_w = (area.w * 0.24).round();
        let right_w = (area.w * 0.34).round();
        let left = Rect::new(area.x, area.y, left_w, area.h);
        let middle = Rect::new(left.right(), area.y, area.w - left_w - right_w, area.h);
        let right = Rect::new(middle.right(), area.y, right_w, area.h);

        let colors = self.ctx.theme.colors.clone();
        canvas.fill(left, 0.0, colors.surface.mix(colors.base, 0.5));
        let (hidden, sort) = (self.pane.shows_hidden(), self.pane.sort());
        // The folder above, this one marked.
        if let Some(parent) = self.pane.location().parent().map(Path::to_owned) {
            self.side_parent.read(&parent, hidden, sort);
            let here = self
                .pane
                .location()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            let entries = self.side_parent.entries.clone();
            self.draw_side(canvas, left, &parent, &entries, here.as_deref(), true);
        }
        let animating = self.draw_list(canvas, middle, now);
        // What's under the cursor.
        canvas.fill(
            Rect::new(right.x, right.y, stroke::HAIRLINE, right.h),
            0.0,
            colors.highlight_low,
        );
        if let Some(row) = self.pane.cursor() {
            if self.pane.entry(row).kind.is_dir() {
                let folder = self.pane.path(row);
                self.side_child.read(&folder, hidden, sort);
                let entries = self.side_child.entries.clone();
                if entries.is_empty() {
                    let text = Text::new("Empty", self.font(type_scale::SMALL), colors.muted);
                    canvas.text_in(&text, right, Align::Center);
                } else {
                    self.draw_side(canvas, right, &folder, &entries, None, false);
                }
            } else {
                self.draw_preview(canvas, right);
            }
        }
        canvas.fill(
            Rect::new(left.right() - 1.0, left.y, stroke::HAIRLINE, left.h),
            0.0,
            colors.highlight_low,
        );
        animating
    }

    /// A side column: names in `folder`, `marked` highlighted. Clicking one
    /// goes there (from the left, its folder; from the right, into it).
    fn draw_side(
        &mut self,
        canvas: &mut Canvas<'_>,
        area: Rect,
        folder: &Path,
        entries: &[(String, bool)],
        marked: Option<&str>,
        left: bool,
    ) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let row_height = self.row_height();
        let inner = area.inset_xy(space::SM, space::SM);
        // Keep the marked one in view.
        let marked_at = marked.and_then(|name| entries.iter().position(|(entry, _)| entry == name));
        let visible = ((inner.h / row_height).floor() as usize).max(1);
        let first = marked_at
            .map_or(0, |at| at.saturating_sub(visible / 2))
            .min(entries.len().saturating_sub(visible));
        canvas.with_clip(area, |canvas| {
            for (offset, (name, is_dir)) in entries.iter().skip(first).take(visible + 1).enumerate()
            {
                let y = inner.y + offset as f32 * row_height;
                let rect = Rect::new(inner.x, y, inner.w, row_height - 2.0);
                let is_marked = marked == Some(name.as_str());
                let path = folder.join(name);
                // From the left: go to it. From the right: into the
                // cursor's folder, with it chosen.
                let hit = Hit::Side(path.clone(), !left);
                if is_marked {
                    canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
                } else if self.hover.as_ref() == Some(&hit) {
                    canvas.fill(rect, radius::CONTROL, colors.highlight_low);
                }
                let kind = Kind::of(name, *is_dir, false);
                let figure = if *is_dir { icon::FOLDER } else { icon::FILE };
                canvas.icon(
                    figure,
                    16.0,
                    kind.color(&colors, accent),
                    Rect::new(rect.x + space::SM, rect.y, 18.0, rect.h),
                    Align::Start,
                );
                let ink = if is_marked {
                    colors.text
                } else {
                    colors.subtle
                };
                let label = Rect::new(
                    rect.x + space::SM + 18.0 + space::SM,
                    rect.y,
                    rect.w - 18.0 - 3.0 * space::SM,
                    rect.h,
                );
                canvas.text_in(&Text::new(name, body * 0.95, ink), label, Align::Start);
                self.hits.push((rect, hit));
            }
        });
    }

    /// A side column was clicked: go into `path` if it's a folder from the
    /// left; else to where it is, with the cursor on it.
    pub(super) fn side_clicked(&mut self, path: PathBuf, reveal: bool) {
        if !reveal && path.is_dir() {
            self.navigate(path);
        } else {
            self.reveal(&path);
        }
    }
}
