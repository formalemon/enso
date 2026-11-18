//! Properties (`Ctrl+I`, `Alt+Enter`): what something is, where, how big
//! all the way down (counted as you watch, for a folder), when it changed,
//! whose it is, and who may read, write, and run it — three rows of boxes
//! to tick, and the number they make. For several at once: their total,
//! and the permissions they share, set for all with one click.
//!
//! `Ctrl+Shift+X` makes what's chosen runnable as a program, or not.

use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use enso_core::style::{icon, radius, space, stroke, type_scale};
use enso_fs::{format, tally::Tally};
use enso_ui::{
    Align, Canvas, Cx, Rect, Text,
    widget::{Button, Interaction, Kind as ButtonKind, controls, form},
};

use super::{Files, Hit};
use crate::{jobs::Message, kind::Kind};

/// Who, in the permission grid: owner, group, others.
const WHO: [&str; 3] = ["Owner", "Group", "Others"];
/// What: the bit for each, owner's (shifted right 3 per row below).
const WHAT: [(&str, u32); 3] = [("Read", 0o400), ("Write", 0o200), ("Run", 0o100)];

#[derive(Debug)]
pub(super) struct Properties {
    paths: Vec<PathBuf>,
    /// The count so far, and whether it's finished.
    tally: Option<(Tally, bool)>,
    cancel: Arc<AtomicBool>,
    /// Which count this is, so a late one for an earlier window is ignored.
    token: u64,
}

impl Drop for Properties {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The modes of these paths, without following links.
fn modes(paths: &[PathBuf]) -> Vec<u32> {
    paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.permissions().mode() & 0o7777)
        .collect()
}

/// A user's or group's name, from `/etc/passwd` or `/etc/group`.
fn name_of(file: &str, id: u32) -> String {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let _ = fields.next()?;
            (fields.next()?.parse::<u32>().ok()? == id).then(|| name.to_owned())
        })
        .unwrap_or_else(|| id.to_string())
}

impl Files {
    /// Open the properties of what's chosen (or under the cursor).
    pub(super) fn open_properties(&mut self) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        self.tallies += 1;
        let token = self.tallies;
        let cancel = Arc::new(AtomicBool::new(false));
        // Only folders take counting; a file's size is in hand.
        let counting = paths.len() > 1 || paths[0].is_dir();
        if counting && let Some(proxy) = self.proxy.clone() {
            let (paths, cancel) = (paths.clone(), cancel.clone());
            let _ = thread::Builder::new()
                .name("enso-files-tally".to_owned())
                .spawn(move || {
                    let total = enso_fs::tally::tally(&paths, &cancel, |tally| {
                        proxy.send(Message::Tallied {
                            token,
                            tally: *tally,
                            done: false,
                        });
                    });
                    proxy.send(Message::Tallied {
                        token,
                        tally: total,
                        done: true,
                    });
                });
        }
        self.properties = Some(Properties {
            paths,
            tally: None,
            cancel,
            token,
        });
    }

    pub(super) fn tallied(&mut self, token: u64, tally: Tally, done: bool) {
        if let Some(properties) = &mut self.properties
            && properties.token == token
        {
            properties.tally = Some((tally, done));
        }
    }

    /// Tick or untick `bit` for everything shown: on for all if any lacks
    /// it, else off for all.
    fn toggle_bit(&mut self, bit: u32, cx: &mut Cx<'_>) {
        let refused = self.flip_bit(bit);
        if refused > 0 {
            self.toast(
                format!(
                    "Only their owner can change that: {} weren't changed",
                    format::count(refused, "item", "items")
                ),
                cx,
            );
        }
    }

    /// The change itself. Returns how many refused it.
    pub(super) fn flip_bit(&mut self, bit: u32) -> usize {
        let Some(properties) = &self.properties else {
            return 0;
        };
        let paths = properties.paths.clone();
        let all = modes(&paths).iter().all(|mode| mode & bit != 0);
        let mut refused = 0;
        for path in &paths {
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            let mode = meta.permissions().mode();
            let mode = if all { mode & !bit } else { mode | bit };
            if std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).is_err() {
                refused += 1;
            }
        }
        refused
    }

    /// `Ctrl+Shift+X`: what's chosen can be run as a program — or, if it
    /// all could, no longer. Run is given to whoever may read.
    pub(super) fn toggle_runnable(&mut self, cx: &mut Cx<'_>) {
        let files: Vec<PathBuf> = self
            .targets()
            .into_iter()
            .filter(|path| path.is_file())
            .collect();
        if files.is_empty() {
            return;
        }
        let all = modes(&files).iter().all(|mode| mode & 0o111 != 0);
        let mut changed = 0;
        for path in &files {
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            let mode = meta.permissions().mode();
            // r-- for someone becomes r-x; taking it away takes every x.
            let mode = if all {
                mode & !0o111
            } else {
                mode | ((mode & 0o444) >> 2)
            };
            if std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).is_ok() {
                changed += 1;
            }
        }
        let what = format::count(changed, "file", "files");
        let text = match (changed, all) {
            (0, _) => "Only their owner can change that".to_owned(),
            (_, false) => format!("{what} can be run as programs now"),
            (_, true) => format!("{what} can't be run anymore"),
        };
        self.toast(text, cx);
    }

    /// Keys while the properties are up: Esc or Enter puts them away.
    /// Returns whether they were up.
    pub(super) fn properties_key(&mut self, key: &enso_ui::KeyEvent, cx: &mut Cx<'_>) -> bool {
        if self.properties.is_none() {
            return false;
        }
        if matches!(key.key, enso_ui::Key::Escape | enso_ui::Key::Enter) {
            self.properties = None;
            cx.redraw();
        }
        true
    }

    /// A press while the properties are up. Returns whether they were.
    pub(super) fn properties_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.properties.is_none() {
            return false;
        }
        match hit {
            Some(Hit::Permission(bit)) => self.toggle_bit(*bit, cx),
            Some(Hit::PropertiesCard) => {}
            _ => self.properties = None,
        }
        cx.redraw();
        true
    }

    pub(super) fn draw_properties(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(properties) = &self.properties else {
            return;
        };
        let paths = properties.paths.clone();
        let tally = properties.tally;
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        let zone = jiff::tz::TimeZone::system();
        self.hits.clear();

        let single = paths.len() == 1;
        let first = &paths[0];
        let meta = std::fs::symlink_metadata(first).ok();
        let target_meta = std::fs::metadata(first).ok();
        let is_link = meta
            .as_ref()
            .is_some_and(|meta| meta.file_type().is_symlink());
        let is_dir = target_meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
        let name = first.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );

        // The facts, as label and value.
        let mut rows: Vec<(&str, String)> = Vec::new();
        let counted = |tally: Option<(Tally, bool)>| match tally {
            None => "Counting…".to_owned(),
            Some((tally, done)) => {
                let items = format::count(
                    (tally.files + tally.folders).saturating_sub(paths.len() as u64) as usize,
                    "item",
                    "items",
                );
                let mut text = format!("{} in {items}", format::size(tally.bytes));
                if !done {
                    text += " · counting…";
                } else if tally.unreadable > 0 {
                    text += &format!(
                        " · {} not readable",
                        format::count(tally.unreadable as usize, "folder", "folders")
                    );
                }
                text
            }
        };
        let title;
        if single {
            title = name.clone();
            let executable = target_meta
                .as_ref()
                .is_some_and(|meta| meta.mode() & 0o111 != 0);
            let kind = Kind::of(&name, is_dir, executable);
            let mime = enso_fs::mime::MimeDb::load();
            let what = match (is_dir, mime.by_name(&name)) {
                (true, _) => "Folder".to_owned(),
                (false, Some(mime)) => format!("{} · {mime}", describe(kind)),
                (false, None) => describe(kind).to_owned(),
            };
            rows.push((
                "Kind",
                if is_link {
                    format!("{what} (a link)")
                } else {
                    what
                },
            ));
            if let Some(parent) = first.parent() {
                rows.push(("Where", super::tabs::short(parent, &self.home)));
            }
            if is_dir {
                rows.push(("Size", counted(tally)));
            } else if let Some(meta) = &target_meta {
                rows.push((
                    "Size",
                    format!("{} ({} bytes)", format::size(meta.len()), meta.len()),
                ));
            }
            if let Some(meta) = &target_meta {
                let time = |time: std::io::Result<std::time::SystemTime>| {
                    time.ok()
                        .and_then(|time| jiff::Timestamp::try_from(time).ok())
                        .map(|time| {
                            time.to_zoned(zone.clone())
                                .strftime("%a %-d %b %Y, %H:%M")
                                .to_string()
                        })
                };
                if let Some(when) = time(meta.modified()) {
                    rows.push(("Modified", when));
                }
                if let Some(when) = time(meta.created()) {
                    rows.push(("Created", when));
                }
                if let Some(when) = time(meta.accessed()) {
                    rows.push(("Opened", when));
                }
                rows.push((
                    "Owner",
                    format!(
                        "{}, group {}",
                        name_of("/etc/passwd", meta.uid()),
                        name_of("/etc/group", meta.gid())
                    ),
                ));
            }
            if is_link && let Ok(target) = std::fs::read_link(first) {
                rows.push(("Leads to", target.to_string_lossy().into_owned()));
            }
        } else {
            title = format::count(paths.len(), "item", "items");
            let folders = paths.iter().filter(|path| path.is_dir()).count();
            let files = paths.len() - folders;
            let mut what = Vec::new();
            if files > 0 {
                what.push(format::count(files, "file", "files"));
            }
            if folders > 0 {
                what.push(format::count(folders, "folder", "folders"));
            }
            rows.push(("Kind", what.join(" and ")));
            let parents: std::collections::HashSet<&Path> =
                paths.iter().filter_map(|path| path.parent()).collect();
            rows.push((
                "Where",
                match parents.iter().next() {
                    Some(parent) if parents.len() == 1 => super::tabs::short(parent, &self.home),
                    _ => format!("in {} folders", parents.len()),
                },
            ));
            rows.push(("Size", counted(tally)));
        }
        if let Ok(space) = enso_fs::places::space(first) {
            rows.push((
                "Disk",
                format!(
                    "{} free of {}",
                    format::size(space.free),
                    format::size(space.total)
                ),
            ));
        }

        let line = body * 1.6;
        let grid_h = 4.0 * 32.0 + space::SM;
        let height = 56.0
            + rows.len() as f32 * line
            + space::LG
            + 26.0
            + grid_h
            + space::LG
            + controls::HEIGHT
            + 2.0 * space::XL;
        let inner = controls::dialog(canvas, bounds, 520.0, height, &look, 1.0);
        let card = inner.inset(-space::XL);
        self.hits.push((card, Hit::PropertiesCard));

        // The title, with its kind's figure.
        let figure = if single && is_dir || !single {
            icon::FOLDER
        } else {
            icon::FILE
        };
        let kind_color = if single {
            let executable = target_meta
                .as_ref()
                .is_some_and(|meta| meta.mode() & 0o111 != 0);
            Kind::of(&name, is_dir, executable).color(&colors, accent)
        } else {
            accent
        };
        canvas.icon(
            figure,
            26.0,
            kind_color,
            Rect::new(inner.x, inner.y, 30.0, 40.0),
            Align::Start,
        );
        let heading = Text::new(&title, form::title_size(&look), colors.text)
            .medium()
            .max_width(inner.w - 44.0);
        canvas.text_in(
            &heading,
            Rect::new(inner.x + 44.0, inner.y, inner.w - 44.0, 40.0),
            Align::Start,
        );
        let mut y = inner.y + 56.0;
        let label_w = 96.0;
        for (label, value) in &rows {
            canvas.text(&Text::new(label, small, colors.muted), inner.x, y + 2.0);
            let value = Text::new(value, body, colors.text).max_width(inner.w - label_w);
            canvas.text(&value, inner.x + label_w, y);
            y += line;
        }

        // Permissions: who down the side, what across the top.
        y += space::LG;
        let modes = modes(&paths);
        let mode_text = match modes.as_slice() {
            [mode] => format!("{mode:o}"),
            _ => String::new(),
        };
        canvas.text(
            &Text::new("Permissions", small, colors.muted).medium(),
            inner.x,
            y,
        );
        canvas.text_in(
            &Text::new(&mode_text, small, colors.subtle).font(enso_ui::Font::Mono),
            Rect::new(inner.x, y, inner.w, 18.0),
            Align::End,
        );
        y += 26.0;
        let column = (inner.w - label_w) / 3.0;
        for (index, (what, _)) in WHAT.iter().enumerate() {
            let label = if index == 2 && (is_dir || !single) {
                "Run · open"
            } else {
                what
            };
            let rect = Rect::new(inner.x + label_w + index as f32 * column, y, column, 28.0);
            canvas.text_in(&Text::new(label, small, colors.subtle), rect, Align::Center);
        }
        y += 32.0;
        for (row, who) in WHO.iter().enumerate() {
            canvas.text_in(
                &Text::new(who, body, colors.text),
                Rect::new(inner.x, y, label_w, 28.0),
                Align::Start,
            );
            for (index, (_, owner_bit)) in WHAT.iter().enumerate() {
                let bit = owner_bit >> (3 * row);
                let have = modes.iter().filter(|mode| *mode & bit != 0).count();
                let cell = Rect::new(inner.x + label_w + index as f32 * column, y, column, 28.0);
                let tick = cell.centered(20.0, 20.0);
                let hovered = self.hover == Some(Hit::Permission(bit));
                if have == modes.len() && have > 0 {
                    canvas.fill(tick, 5.0, accent);
                    canvas.icon(icon::CHECK, 14.0, look.base, tick, Align::Center);
                } else if have > 0 {
                    // Some have it, some don't.
                    canvas.stroke(tick, 5.0, stroke::FOCUS, accent);
                    canvas.fill(tick.centered(8.0, 2.0), 1.0, accent);
                } else {
                    canvas.stroke(
                        tick,
                        5.0,
                        stroke::HAIRLINE,
                        if hovered {
                            colors.text
                        } else {
                            colors.highlight_high
                        },
                    );
                }
                if hovered {
                    canvas.fill(tick.inset(-4.0), radius::CONTROL, colors.highlight_low);
                }
                self.hits
                    .push((cell.centered(column.min(48.0), 28.0), Hit::Permission(bit)));
            }
            y += 32.0;
        }

        let done = Button::new("Done", ButtonKind::Secondary);
        let w = done.width(canvas, &look);
        let rect = Rect::new(
            inner.right() - w,
            inner.bottom() - controls::HEIGHT,
            w,
            controls::HEIGHT,
        );
        done.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::FormCancel),
                ..Interaction::default()
            },
        );
        self.hits.push((rect, Hit::FormCancel));
    }
}

fn describe(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => "Folder",
        Kind::Image => "Picture",
        Kind::Video => "Video",
        Kind::Audio => "Sound",
        Kind::Archive => "Archive",
        Kind::Code => "Code or settings",
        Kind::Document => "Document",
        Kind::Program => "Program",
        Kind::Other => "File",
    }
}
