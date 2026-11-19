//! Drag and drop, both ways. Files dragged out go as `text/uri-list`
//! (what file managers, browsers' upload fields, and editors take) and as
//! their paths, quoted, for terminals. Files dropped in land in the folder
//! under the pointer — a folder in the list, a place in the sidebar, a
//! tab, a crumb of the path, the split's other side — or the folder shown;
//! moved if the drag says move (one from this window), else copied. In a
//! picker, they join what's chosen.

use std::path::{Path, PathBuf};

use enso_core::{
    Icon, Rgba,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_fs::uri;
use enso_ui::{Align, Canvas, Clip, Cx, DragIcon, Rect, Text};

use super::{Files, Hit, tabs::TabHit};
use crate::{jobs::Op, pane::Source};

/// How far the pointer moves with the button held before it's a drag.
const DRAG_FROM: f32 = 6.0;

/// What's dragged, under the pointer: a pill this tall, at most this wide.
const ICON_HEIGHT: f32 = 34.0;
const ICON_WIDTH: f32 = 280.0;

/// What a drag's pill says, how big, and in which colours.
#[derive(Clone, Debug)]
pub(super) struct Held {
    pub(super) label: String,
    pub(super) icon: Icon,
    pub(super) size: f32,
    pub(super) text: Rgba,
    pub(super) fill: Rgba,
    pub(super) edge: Rgba,
    pub(super) accent: Rgba,
}

impl Held {
    /// The pill's width for a label `label_width` wide.
    pub(super) fn width(&self, label_width: f32) -> f32 {
        // The edge, the room at each end, the icon and the gap after it.
        let around = 2.0 + 2.0 * space::MD + self.size + space::SM;
        (around + label_width + 1.0).min(ICON_WIDTH)
    }

    /// Draw the pill filling `canvas`.
    pub(super) fn draw(&self, canvas: &mut Canvas<'_>) {
        let pill = canvas.bounds().inset(1.0);
        canvas.fill(pill, radius::pill(pill.h), self.fill);
        canvas.stroke(pill, radius::pill(pill.h), stroke::HAIRLINE, self.edge);
        let inner = pill.inset_xy(space::MD, 0.0);
        canvas.icon(self.icon, self.size, self.accent, inner, Align::Start);
        let start = inner.x + self.size + space::SM;
        let label = Rect::new(start, inner.y, inner.right() - start, inner.h);
        canvas.text_in(
            &Text::new(&self.label, self.size * 0.9, self.text).max_width(label.w),
            label,
            Align::Start,
        );
    }
}

impl Files {
    /// The button went down on `row`: a drag may start from here.
    pub(super) fn may_drag(&mut self, x: f32, y: f32) {
        self.drag_from = Some((x, y));
    }

    /// The pointer moved with the button down: past a little way, the
    /// files under it start being dragged. Returns whether they did.
    pub(super) fn maybe_drag(&mut self, x: f32, y: f32, cx: &mut Cx<'_>) -> bool {
        let Some((from_x, from_y)) = self.drag_from else {
            return false;
        };
        if (x - from_x).hypot(y - from_y) < DRAG_FROM {
            return false;
        }
        self.drag_from = None;
        // An archive's insides aren't files to hand over.
        if matches!(self.pane.source(), Source::Archive { .. }) {
            return false;
        }
        let paths = self.dragged();
        if paths.is_empty() {
            return false;
        }
        let held = self.held(&paths);
        let label = cx
            .measure(&Text::new(&held.label, held.size * 0.9, held.text))
            .w;
        let width = held.width(label).ceil() as u32;
        let picture = DragIcon::new((width, ICON_HEIGHT as u32), move |canvas| {
            held.draw(canvas);
        });
        cx.start_drag_showing(clip_of(&paths), picture);
        true
    }

    /// What the pill under the pointer shows for `paths`: the one file's
    /// name, or how many there are.
    pub(super) fn held(&self, paths: &[PathBuf]) -> Held {
        let colors = &self.ctx.theme.colors;
        let (label, figure) = match paths {
            [one] => (
                one.file_name().map_or_else(
                    || one.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                ),
                if one.is_dir() {
                    icon::FOLDER
                } else {
                    icon::FILE
                },
            ),
            many => (format!("{} items", many.len()), icon::FILE),
        };
        Held {
            label,
            icon: figure,
            size: self.font(type_scale::BODY),
            text: colors.text,
            fill: colors.overlay,
            edge: colors.highlight_high,
            accent: self.ctx.accent(),
        }
    }

    /// What a drag takes: what's chosen, if the cursor's row is among it;
    /// else the cursor's row.
    fn dragged(&self) -> Vec<PathBuf> {
        let Some(row) = self.pane.cursor() else {
            return Vec::new();
        };
        let under = self.pane.path(row);
        if self.selection.contains(&under) {
            self.selection.paths().to_vec()
        } else {
            vec![under]
        }
    }

    /// Where a drop at `x, y` would go, and the place to light up (`None`:
    /// the whole list).
    fn drop_target(&self, x: f32, y: f32) -> Option<(PathBuf, Option<Rect>)> {
        let found = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(rect, hit)| (*rect, hit.clone()));
        let here = || {
            self.pane
                .source()
                .is_folder()
                .then(|| (self.pane.location().to_owned(), None))
        };
        match found {
            Some((rect, Hit::Row(row))) if self.pane.entry(row).kind.is_dir() => {
                Some((self.pane.path(row), Some(rect)))
            }
            Some((rect, Hit::Place(path) | Hit::Crumb(path))) => Some((path, Some(rect))),
            Some((rect, Hit::Tabs(TabHit::Tab(index)))) => {
                Some((self.tab_location(index).to_owned(), Some(rect)))
            }
            Some((_, Hit::Other(_))) => self
                .split
                .map(|other| (self.tab_location(other).to_owned(), Some(self.split_rect))),
            _ => here(),
        }
    }

    #[cfg(test)]
    pub(super) fn drop_target_for_tests(&self, x: f32, y: f32) -> Option<(PathBuf, Option<Rect>)> {
        self.drop_target(x, y)
    }

    /// Something droppable is over the window.
    pub(super) fn drag_over(&mut self, x: f32, y: f32, cx: &mut Cx<'_>) {
        let target = if self.picker.is_some() {
            // A picker takes what's dropped anywhere, into what's chosen.
            Some((self.pane.location().to_owned(), None))
        } else {
            self.drop_target(x, y)
        };
        if target != self.drop_at {
            self.drop_at = target;
            cx.redraw();
        }
    }

    pub(super) fn drag_leave(&mut self, cx: &mut Cx<'_>) {
        if self.drop_at.take().is_some() {
            cx.redraw();
        }
    }

    pub(super) fn dropped(&mut self, data: &[u8], x: f32, y: f32, moving: bool, cx: &mut Cx<'_>) {
        let target = if self.picker.is_some() {
            None
        } else {
            self.drop_target(x, y).map(|(path, _)| path)
        };
        self.drop_at = None;
        cx.redraw();
        let paths = paths_of(data);
        if paths.is_empty() {
            return;
        }
        if self.picker.is_some() {
            return self.drop_into_picker(paths, cx);
        }
        let Some(into) = target else {
            self.toast("Nothing can be dropped here: this isn't a folder", cx);
            return;
        };
        // Already where they'd go: nothing to do.
        let sources: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| path.parent() != Some(into.as_path()))
            .collect();
        if sources.is_empty() {
            return;
        }
        if sources.iter().any(|source| into.starts_with(source)) {
            self.toast("A folder can't go inside itself", cx);
            return;
        }
        let op = if moving {
            Op::Move { sources, into }
        } else {
            Op::Copy { sources, into }
        };
        self.start_op(op, cx);
    }

    /// Dropped on a picker: what can be chosen joins what's chosen.
    fn drop_into_picker(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_>) {
        let Some(picker) = &self.picker else { return };
        let (directory, save, multiple) = (
            picker.request.directory(),
            picker.request.is_save(),
            picker.request.multiple(),
        );
        if save {
            return;
        }
        let fitting: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| path.is_dir() == directory && path.exists())
            .collect();
        if fitting.is_empty() {
            self.toast(
                if directory {
                    "Only folders can be chosen here"
                } else {
                    "Only files can be chosen here"
                },
                cx,
            );
            return;
        }
        if !multiple {
            self.selection.only(fitting[0].clone());
        } else {
            for path in &fitting {
                self.selection.insert(path.clone());
            }
        }
        let count = if multiple { fitting.len() } else { 1 };
        self.toast(
            format!(
                "Chose {}",
                enso_fs::format::count(count, "dropped item", "dropped items")
            ),
            cx,
        );
    }

    /// Light up where a drop would land.
    pub(super) fn draw_drop(&self, canvas: &mut Canvas<'_>) {
        let Some((_, at)) = &self.drop_at else { return };
        let accent = self.ctx.accent();
        let rect = at.unwrap_or_else(|| self.list.inset(3.0));
        canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
        canvas.stroke(rect, radius::CONTROL, stroke::FOCUS, accent.alpha(0.8));
    }
}

/// Files as a drag carries them: a URI list, and their paths quoted for
/// a shell.
fn clip_of(paths: &[PathBuf]) -> Clip {
    let uris: String = paths
        .iter()
        .map(|path| format!("{}\r\n", uri::from_path(path)))
        .collect();
    let quoted: Vec<String> = paths.iter().map(|path| quote(path)).collect();
    Clip::default()
        .with("text/uri-list", uris.into_bytes())
        .with("text/plain;charset=utf-8", quoted.join(" ").into_bytes())
        .with("text/plain", quoted.join(" ").into_bytes())
}

/// `path` as a shell reads it: in single quotes, any inside escaped.
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The local files in a `text/uri-list`.
fn paths_of(data: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(data)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(uri::to_path)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_travel_as_uris_and_quoted_paths() {
        let paths = [
            PathBuf::from("/home/me/a b.txt"),
            PathBuf::from("/tmp/it's"),
        ];
        let clip = clip_of(&paths);
        let (_, uris) = &clip.formats[0];
        assert_eq!(paths_of(uris), paths);
        assert_eq!(
            String::from_utf8_lossy(&clip.formats[1].1),
            r"'/home/me/a b.txt' '/tmp/it'\''s'"
        );
        assert_eq!(
            paths_of(b"# a comment\r\nfile:///x\r\nhttps://example.com/\r\n"),
            [PathBuf::from("/x")]
        );
    }
}
