//! The sidebar: your places, the drives — every other filesystem, mounted
//! or waiting to be — and the trash.
//!
//! Linux has no drive letters: a disk, a partition, or a USB stick is
//! attached ("mounted") at a folder in the one tree, `/run/media/<you>/…`
//! for the ones you plug in. udisks2 knows them all; a worker follows it, so
//! a stick appears the moment it's plugged in and goes when it's pulled.
//! Clicking one that isn't mounted mounts it and goes there; its ⏏ unmounts
//! it again — and powers a USB disk down, so it's safe to pull.
//!
//! The trash is a place like the others: in it, Delete deletes for good,
//! the menu puts things back, and the status line offers to empty it.

use std::{
    path::{Path, PathBuf},
    thread,
};

use enso_core::style::{icon, opacity, radius, space, stroke, type_scale};
use enso_fs::{
    bookmarks::{self, Bookmarks},
    format, places, trash,
};
use enso_sys::udisks::{self, Volume};
use enso_ui::{Align, Canvas, Cx, Rect, Text, widget::controls};

use super::overlay::Overlay;
use super::{Files, Hit};
use crate::{jobs::Message, pane::Source};

const PLACE_HEIGHT: f32 = 32.0;

impl Files {
    /// Follow udisks for the life of the window: the drives now, and again
    /// whenever one comes, goes, or is mounted.
    pub(super) fn watch_drives(&self) {
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("enso-files-drives".to_owned())
            .spawn(move || {
                let send = |proxy: &enso_ui::Proxy<Message>| {
                    proxy.send(Message::Volumes(udisks::volumes().unwrap_or_default()))
                };
                if !send(&proxy) {
                    return;
                }
                // No udisks here: the drives stay as they are (none).
                let _ = udisks::watch(|| send(&proxy));
            });
    }

    /// What a volume is called: its label, else its size and kind.
    pub(super) fn volume_name(volume: &Volume) -> String {
        match volume.name() {
            Some(name) => name.to_owned(),
            None => format!(
                "{} {}",
                format::size(volume.size),
                volume.kind.to_uppercase()
            ),
        }
    }

    /// A drive was clicked: go there, mounting it first if need be.
    pub(super) fn open_volume(&mut self, index: usize, cx: &mut Cx<'_>) {
        let Some(volume) = self.volumes.get(index).cloned() else {
            return;
        };
        if let Some(mount) = &volume.mount {
            return self.navigate(mount.clone());
        }
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        self.mounting = Some(volume.object.clone());
        let name = Self::volume_name(&volume);
        let _ = thread::Builder::new()
            .name("enso-files-mount".to_owned())
            .spawn(move || {
                let result = udisks::mount(&volume).map_err(|err| {
                    if udisks::is_dismissed(&err) {
                        format!("{name} stays unmounted")
                    } else if udisks::is_denied(&err) {
                        format!(
                            "Mounting {name} needs an administrator's permission, and nothing is running to ask for it · enso-auth asks; or enso's contrib/50-enso-udisks.rules lets the wheel group mount disks"
                        )
                    } else {
                        format!("Can't mount {name}: {err}")
                    }
                });
                proxy.send(Message::Mounted(result));
            });
        cx.redraw();
    }

    pub(super) fn mounted(&mut self, result: Result<PathBuf, String>, cx: &mut Cx<'_>) {
        self.mounting = None;
        match result {
            Ok(path) => self.navigate(path),
            Err(why) => self.toast(why, cx),
        }
        cx.redraw();
    }

    /// ⏏: unmount it (and everything else on its disk), and power the disk
    /// down if it can be.
    pub(super) fn eject(&mut self, index: usize, cx: &mut Cx<'_>) {
        let Some(volume) = self.volumes.get(index).cloned() else {
            return;
        };
        // Not from inside it: the window would hold it busy.
        if let Some(mount) = &volume.mount
            && self.pane.location().starts_with(mount)
        {
            self.navigate(places::home());
        }
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let all = self.volumes.clone();
        let name = Self::volume_name(&volume);
        let _ = thread::Builder::new()
            .name("enso-files-eject".to_owned())
            .spawn(move || {
                let result = if volume.can_eject {
                    udisks::eject(&volume, &all).map(|()| format!("{name} can be taken out now"))
                } else {
                    udisks::unmount(&volume).map(|()| format!("{name} is unmounted"))
                };
                proxy.send(Message::Ejected(
                    result.map_err(|err| format!("Can't eject {name}: {err}")),
                ));
            });
        cx.redraw();
    }

    // The trash.

    pub(super) fn trash_dir() -> PathBuf {
        trash::home_trash().dir.join("files")
    }

    pub(super) fn in_trash(&self) -> bool {
        self.pane.location() == Self::trash_dir()
    }

    pub(super) fn open_trash(&mut self) {
        let dir = Self::trash_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.navigate(dir);
    }

    /// The trash's items for `paths` in it.
    fn trash_items(paths: &[PathBuf]) -> Vec<trash::Item> {
        trash::list(&trash::home_trash())
            .into_iter()
            .filter(|item| paths.contains(&item.path()))
            .collect()
    }

    /// Put what's chosen in the trash back where it came from.
    pub(super) fn restore(&mut self, cx: &mut Cx<'_>) {
        let items = Self::trash_items(&self.targets());
        let (mut back, mut failed) = (0, None);
        for item in &items {
            match trash::restore(item) {
                Ok(_) => back += 1,
                Err(err) => failed = Some(err.to_string()),
            }
            self.selection.remove(&item.path());
        }
        let mut message = format!("Put back {}", format::count(back, "item", "items"));
        if let Some(why) = failed {
            message += &format!(" · one couldn't be: {why}");
        }
        self.toast(message, cx);
    }

    /// Delete things in the trash for good (with their records).
    pub(super) fn delete_from_trash(&mut self, paths: &[PathBuf], cx: &mut Cx<'_>) {
        let items = Self::trash_items(paths);
        let failed = items
            .iter()
            .filter(|item| trash::delete(item).is_err())
            .count();
        let done = items.len() - failed;
        self.toast(
            format!("Deleted {} for good", format::count(done, "item", "items")),
            cx,
        );
    }

    /// Ask to empty the trash.
    pub(super) fn ask_empty(&mut self, cx: &mut Cx<'_>) {
        let paths: Vec<PathBuf> = trash::list(&trash::home_trash())
            .iter()
            .map(trash::Item::path)
            .collect();
        if paths.is_empty() {
            return self.toast("The trash is empty", cx);
        }
        self.overlay = Some(Overlay::ConfirmDelete(paths));
        cx.redraw();
    }

    // Drawing.

    pub(super) fn draw_sidebar(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let small = self.font(type_scale::SMALL);
        canvas.fill(area, 0.0, colors.surface);
        let edge = Rect::new(
            area.right() - controls::SPLITTER_GRAB,
            area.y,
            controls::SPLITTER_GRAB * 2.0,
            area.h,
        );
        let edge_color = if self.sidebar.is_dragging() || self.hover == Some(Hit::SidebarEdge) {
            accent.alpha(0.5)
        } else {
            colors.highlight_low
        };
        canvas.fill(
            Rect::new(area.right() - 1.0, area.y, stroke::HAIRLINE, area.h),
            0.0,
            edge_color,
        );

        let location = self.pane.location().to_owned();
        let in_folder = self.pane.source().is_folder();
        let mut y = area.y + space::SM;
        for (index, place) in self.places.clone().into_iter().enumerate() {
            if y + PLACE_HEIGHT > area.bottom() {
                return;
            }
            let here = in_folder && place.path == location;
            self.place_row(
                canvas,
                area,
                &mut y,
                &place.label,
                place.icon,
                here,
                Hit::Place(place.path.clone()),
                false,
            );
            // Recent, after Home.
            if index == 0 {
                let here = *self.pane.source() == Source::Recent;
                self.place_row(
                    canvas,
                    area,
                    &mut y,
                    "Recent",
                    icon::CLOCK,
                    here,
                    Hit::Recent,
                    false,
                );
            }
        }

        // Bookmarks, when there are any.
        if !self.bookmarks.is_empty() {
            y += space::SM;
            canvas.text(
                &Text::new("Bookmarks", small, colors.muted).medium(),
                area.x + space::LG,
                y,
            );
            y += 24.0;
            for mark in self.bookmarks.clone() {
                if y + PLACE_HEIGHT > area.bottom() - 3.0 * PLACE_HEIGHT {
                    break;
                }
                let here = in_folder && mark.path == location;
                self.place_row(
                    canvas,
                    area,
                    &mut y,
                    &mark.name(),
                    icon::BOOKMARK,
                    here,
                    Hit::Place(mark.path.clone()),
                    false,
                );
            }
        }

        // Drives.
        y += space::SM;
        canvas.text(
            &Text::new("Drives", small, colors.muted).medium(),
            area.x + space::LG,
            y,
        );
        y += 24.0;
        let root = PathBuf::from("/");
        let rect = self.place_row(
            canvas,
            area,
            &mut y,
            "Computer",
            icon::DRIVE,
            location == root,
            Hit::Place(root.clone()),
            false,
        );
        self.draw_space(canvas, rect, &root);
        let volumes = self.volumes.clone();
        for (index, volume) in volumes.iter().enumerate() {
            if y + PLACE_HEIGHT > area.bottom() - PLACE_HEIGHT {
                break;
            }
            let name = Self::volume_name(volume);
            let here = volume
                .mount
                .as_ref()
                .is_some_and(|mount| location.starts_with(mount));
            let busy = self.mounting.as_deref() == Some(volume.object.as_str());
            let rect = self.place_row(
                canvas,
                area,
                &mut y,
                &name,
                icon::DRIVE,
                here,
                Hit::Volume(index),
                busy,
            );
            if let Some(mount) = &volume.mount {
                self.draw_space(canvas, rect, mount);
                // ⏏ at the right.
                let eject = Rect::new(rect.right() - 28.0, rect.y, 28.0, rect.h);
                let hovered = self.hover == Some(Hit::Eject(index));
                if hovered {
                    canvas.fill(eject.inset(4.0), radius::CONTROL, colors.highlight_med);
                }
                canvas.icon(
                    icon::EJECT,
                    14.0,
                    if hovered { colors.text } else { colors.muted },
                    eject,
                    Align::Center,
                );
                self.hits.push((eject, Hit::Eject(index)));
            }
        }

        // The trash, last.
        y += space::SM;
        let full = (self.machine.trash_full)();
        let here = self.in_trash();
        let label = if full { "Trash" } else { "Trash (empty)" };
        self.place_row(
            canvas,
            area,
            &mut y,
            label,
            icon::TRASH,
            here,
            Hit::TrashPlace,
            false,
        );
        self.hits.push((edge, Hit::SidebarEdge));
    }

    /// The files used lately, here and in other programs.
    pub(super) fn open_recent(&mut self) {
        let request = self.pane.show(PathBuf::from("/"), Source::Recent);
        self.load(request);
    }

    fn save_bookmarks(&mut self, marks: &Bookmarks, cx: &mut Cx<'_>) {
        if let Err(err) = marks.save(&bookmarks::file()) {
            self.toast(format!("Can't keep the bookmarks: {err}"), cx);
        }
        self.bookmarks = marks.folders();
    }

    /// Bookmark `path`, or take its bookmark away.
    pub(super) fn toggle_bookmark(&mut self, path: &Path, cx: &mut Cx<'_>) {
        let mut marks = Bookmarks::load(&bookmarks::file());
        let name = path.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let text = if marks.remove(path) {
            format!("{name} is no longer bookmarked")
        } else {
            marks.add(path);
            format!("Bookmarked {name}")
        };
        self.save_bookmarks(&marks, cx);
        self.toast(text, cx);
    }

    pub(super) fn shift_bookmark(&mut self, path: &Path, step: isize, cx: &mut Cx<'_>) {
        let mut marks = Bookmarks::load(&bookmarks::file());
        if marks.shift(path, step) {
            self.save_bookmarks(&marks, cx);
        }
    }

    /// The menu for a right-click on a bookmark.
    pub(super) fn bookmark_menu(&mut self, path: PathBuf, x: f32, y: f32) {
        use enso_ui::widget::{Item, Menu};

        use super::{Action, MenuFor};
        let at = self.bookmarks.iter().position(|mark| mark.path == path);
        let last = self.bookmarks.len().saturating_sub(1);
        let entries = [
            (Item::new("Open in a new tab"), Action::NewTab(path.clone())),
            (
                Item::new("Move up").enabled(at.is_some_and(|at| at > 0)),
                Action::ShiftBookmark(path.clone(), -1),
            ),
            (
                Item::new("Move down").enabled(at.is_some_and(|at| at < last)),
                Action::ShiftBookmark(path.clone(), 1),
            ),
            (Item::new("Remove the bookmark"), Action::Bookmark(path)),
        ];
        let (items, actions): (Vec<Item>, Vec<Option<Action>>) = entries
            .into_iter()
            .map(|(item, action)| (item, Some(action)))
            .unzip();
        self.menu_at = (x, y);
        self.overlay = Some(Overlay::Menu((
            Menu::new(items, x, y),
            MenuFor::Context(actions),
        )));
    }

    /// One row of the sidebar at `y`, which moves on past it: highlighted
    /// where we are, faint under the pointer, quiet while `busy`.
    #[allow(clippy::too_many_arguments)]
    fn place_row(
        &mut self,
        canvas: &mut Canvas<'_>,
        area: Rect,
        y: &mut f32,
        label: &str,
        icon: enso_core::Icon,
        here: bool,
        hit: Hit,
        busy: bool,
    ) -> Rect {
        let colors = &self.ctx.theme.colors;
        let accent = self.ctx.accent();
        let size = self.font(type_scale::BODY);
        let rect = Rect::new(
            area.x + space::SM,
            *y,
            area.w - 2.0 * space::SM,
            PLACE_HEIGHT,
        );
        if here {
            canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
        } else if self.hover.as_ref() == Some(&hit) {
            canvas.fill(rect, radius::CONTROL, colors.highlight_low);
        }
        let inner = rect.inset_xy(space::MD, 0.0);
        let color = if here { accent } else { colors.subtle };
        canvas.icon(
            icon,
            16.0,
            color,
            Rect::new(inner.x, inner.y, 18.0, inner.h),
            Align::Start,
        );
        let ink = if busy {
            colors.muted
        } else if here {
            colors.text
        } else {
            colors.subtle
        };
        let text = Text::new(label, size, ink);
        let text = if here { text.medium() } else { text };
        canvas.text_in(
            &text,
            Rect::new(
                inner.x + 18.0 + space::MD,
                inner.y,
                inner.w - 18.0 - space::MD,
                inner.h,
            ),
            Align::Start,
        );
        self.hits.push((rect, hit));
        *y += PLACE_HEIGHT + 2.0;
        rect
    }

    /// How full a filesystem is: a hairline under its row.
    fn draw_space(&self, canvas: &mut Canvas<'_>, row: Rect, mount: &std::path::Path) {
        let Ok(space) = (self.machine.space)(mount) else {
            return;
        };
        let colors = &self.ctx.theme.colors;
        let used = space.used_fraction();
        let bar = Rect::new(
            row.x + space::MD + 18.0 + space::MD,
            row.bottom() - 2.5,
            row.w - 2.0 * space::MD - 18.0 - space::MD - 30.0,
            2.0,
        );
        canvas.pill(bar, colors.highlight_med);
        let fill = if used > 0.9 {
            colors.love
        } else {
            colors.subtle
        };
        canvas.pill(
            Rect::new(bar.x, bar.y, (bar.w * used).max(2.0), bar.h),
            fill,
        );
    }
}
