//! Changing files from the window: renaming in place, new folders, the
//! trash, copy and paste through the clipboard other file managers use,
//! longer jobs on a worker with their progress in the status line, the
//! questions they ask, and undo.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use enso_core::style::{Shadow, opacity, radius, space, stroke, type_scale};
use enso_fs::{
    format,
    ops::{self, Done, Progress, Resolve},
    uri,
};
use enso_ui::{
    Align, Canvas, Clip, Cx, Key, KeyEvent, Rect, TEXT_TYPES, Text,
    widget::{Form, Interaction, Kind, Part, TextField, field},
};

use super::overlay::{Overlay, Reach};
use super::{Files, Hit};
use crate::jobs::{self, Op};

/// How long a toast stays.
const TOAST: Duration = Duration::from_secs(4);
/// The clipboard type GNOME's Files, Dolphin, and others use for files:
/// "copy" or "cut", then a URI per line.
pub(super) const COPIED_FILES: &str = "x-special/gnome-copied-files";
const URI_LIST: &str = "text/uri-list";

/// An operation under way.
#[derive(Debug)]
pub(super) struct Running {
    pub label: String,
    pub progress: Progress,
    pub stop: Arc<AtomicBool>,
}

/// A question an operation is waiting on.
#[derive(Debug)]
pub(super) struct Asked {
    pub source: PathBuf,
    pub target: PathBuf,
    pub reply: mpsc::Sender<(Resolve, bool)>,
    /// "Do the same for the rest."
    pub all: bool,
}

/// A name being edited in place.
#[derive(Debug)]
pub(super) struct Renaming {
    pub path: PathBuf,
    pub field: TextField,
    pub error: Option<&'static str>,
}

impl Files {
    /// What an action acts on: what's chosen here, else the cursor.
    pub(super) fn targets(&self) -> Vec<PathBuf> {
        let here: Vec<PathBuf> = self
            .selection
            .paths()
            .iter()
            .filter(|path| path.parent() == Some(self.pane.location()))
            .cloned()
            .collect();
        if !here.is_empty() {
            return here;
        }
        if !self.selection.is_empty() {
            return self.selection.paths().to_vec();
        }
        self.pane
            .cursor()
            .map(|row| vec![self.pane.path(row)])
            .unwrap_or_default()
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, cx: &mut Cx<'_>) {
        self.toast = Some((text.into(), Instant::now()));
        cx.wake_in(TOAST);
        cx.redraw();
    }

    /// Drop the toast once its time is up.
    pub(super) fn wake(&mut self, cx: &mut Cx<'_>) {
        if self.search_if_due(Instant::now()) {
            cx.redraw();
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() >= TOAST - Duration::from_millis(20))
        {
            self.toast = None;
            cx.redraw();
        }
    }

    // Jobs.

    pub(super) fn start_op(&mut self, op: Op, cx: &mut Cx<'_>) {
        if self.running.is_some() {
            self.toast("One thing at a time: something is still going", cx);
            return;
        }
        let Some(proxy) = &self.proxy else { return };
        let label = op.describe();
        let stop = jobs::start(op, proxy);
        self.running = Some(Running {
            label,
            progress: Progress::default(),
            stop,
        });
        cx.redraw();
    }

    pub(super) fn op_progress(&mut self, progress: Progress, cx: &mut Cx<'_>) {
        if let Some(running) = &mut self.running {
            running.progress = progress;
            cx.redraw();
        }
    }

    pub(super) fn op_finished(&mut self, op: Op, report: ops::Report, cx: &mut Cx<'_>) {
        self.running = None;
        // A question about this operation has no more to ask.
        self.overlay.take_asked();
        let done: Vec<(PathBuf, PathBuf)> = report.done.clone();
        let record = match &op {
            Op::Copy { .. } | Op::Duplicate(_) | Op::Extract { .. } | Op::Compress { .. } => Some(
                Done::Created(done.iter().map(|(_, to)| to.clone()).collect()),
            ),
            Op::Move { .. } => Some(Done::Moved(done.clone())),
            Op::Delete(_) => None,
        };
        if let Some(record) = record.filter(|_| !done.is_empty()) {
            self.remember(record);
        }
        // What arrived here is what's chosen now.
        let arrived: Vec<PathBuf> = done
            .iter()
            .map(|(_, to)| to.clone())
            .filter(|to| to.parent() == Some(self.pane.location()))
            .collect();
        if !arrived.is_empty() {
            self.selection.clear_in(self.pane.location());
            for path in arrived {
                self.selection.insert(path);
            }
        }
        let verb = match &op {
            Op::Copy { .. } => "Copied",
            Op::Move { .. } => "Moved",
            Op::Duplicate(_) => "Duplicated",
            Op::Delete(_) => "Deleted",
            Op::Extract { .. } => "Took out",
            Op::Compress { .. } => "Made",
        };
        // Taking out or making an archive is one thing, named.
        let what = match (&op, done.first()) {
            (Op::Extract { .. } | Op::Compress { .. }, Some((_, to))) => {
                let name = to
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let mut what = format!("“{name}”");
                if !report.skipped.is_empty() {
                    what += &format!(
                        " · left out {} that could reach outside it",
                        format::count(report.skipped.len(), "item", "items")
                    );
                }
                what
            }
            _ => format::count(done.len(), "item", "items"),
        };
        let mut message = format!("{verb} {what}");
        if report.stopped {
            message = format!("Stopped · {}", message.to_lowercase());
        }
        if !report.failed.is_empty() {
            let (path, why) = &report.failed[0];
            let name = path
                .file_name()
                .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
            message += &format!(" · “{name}” failed: {why}");
            if report.failed.len() > 1 {
                message += &format!(" (and {} more)", report.failed.len() - 1);
            }
        }
        self.toast(message, cx);
    }

    pub(super) fn ask(
        &mut self,
        source: PathBuf,
        target: PathBuf,
        reply: mpsc::Sender<(Resolve, bool)>,
        cx: &mut Cx<'_>,
    ) {
        self.overlay = Some(Overlay::Asked(super::ops::Asked {
            source,
            target,
            reply,
            all: false,
        }));
        cx.redraw();
    }

    fn answer(&mut self, resolve: Resolve, cx: &mut Cx<'_>) {
        if let Some(asked) = self.overlay.take_asked() {
            let _ = asked.reply.send((resolve, asked.all));
        }
        cx.redraw();
    }

    // Undo.

    pub(super) fn remember(&mut self, done: Done) {
        self.undo.push(done);
        // Something new: what was undone is no longer in reach.
        self.redo.clear();
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }

    pub(super) fn undo_last(&mut self, cx: &mut Cx<'_>) {
        let Some(done) = self.undo.pop() else {
            return self.toast("Nothing to undo", cx);
        };
        match ops::undo(&done) {
            Ok(redo) => {
                self.toast(format!("Undid the {}", done.describe()), cx);
                self.redo.push(redo);
            }
            Err(err) => {
                self.toast(format!("Can't undo the {}: {err}", done.describe()), cx);
            }
        }
    }

    pub(super) fn redo_last(&mut self, cx: &mut Cx<'_>) {
        let Some(done) = self.redo.pop() else {
            return self.toast("Nothing to redo", cx);
        };
        match ops::undo(&done) {
            Ok(again) => {
                self.toast("Redone", cx);
                self.undo.push(again);
            }
            Err(err) => self.toast(format!("Can't redo: {err}"), cx),
        }
    }

    // Renaming and new folders.

    pub(super) fn begin_rename(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut field = TextField::new(&name);
        field.select_stem();
        self.filter = None;
        self.location = None;
        self.renaming = Some(Renaming {
            path,
            field,
            error: None,
        });
    }

    fn commit_rename(&mut self, cx: &mut Cx<'_>) {
        let Some(renaming) = &mut self.renaming else {
            return;
        };
        let name = renaming.field.text().trim().to_owned();
        if let Err(why) = ops::check_name(&name) {
            renaming.error = Some(why);
            cx.redraw();
            return;
        }
        let path = renaming.path.clone();
        match ops::rename(&path, &name) {
            Ok(done) => {
                self.renaming = None;
                if let Done::Renamed { to, .. } = &done {
                    if self.selection.contains(&path) {
                        self.selection.remove(&path);
                        self.selection.insert(to.clone());
                    }
                    self.reveal(to);
                }
                self.remember(done);
            }
            Err(err) => {
                self.renaming = None;
                self.toast(format!("Can't rename: {err}"), cx);
            }
        }
        cx.redraw();
    }

    /// Whether there's a folder in front to put things in; if not, say so.
    pub(super) fn in_a_folder(&mut self, cx: &mut Cx<'_>) -> bool {
        let folder = self.pane.source().is_folder();
        if !folder {
            self.toast(
                "This is a list, not a folder: go to one to put things in",
                cx,
            );
        }
        folder
    }

    pub(super) fn new_folder(&mut self, cx: &mut Cx<'_>) {
        if !self.in_a_folder(cx) {
            return;
        }
        match ops::new_folder(self.pane.location(), "New folder") {
            Ok(path) => {
                self.remember(Done::Created(vec![path.clone()]));
                // Name it once it's listed.
                self.rename_after = Some(path.clone());
                self.reveal(&path);
            }
            Err(err) => self.toast(format!("Can't make a folder here: {err}"), cx),
        }
    }

    /// The listing arrived: if a new folder is waiting to be named, name it.
    pub(super) fn listed(&mut self) {
        if let Some(path) = self.rename_after.take()
            && (0..self.pane.len()).any(|row| self.pane.path(row) == path)
        {
            self.begin_rename(path);
        }
    }

    // The trash and deleting.

    pub(super) fn trash_targets(&mut self, cx: &mut Cx<'_>) {
        if self.read_only(cx) {
            return;
        }
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let (trashed, failed) = ops::trash_all(&targets);
        for item in &trashed {
            self.selection.remove(&item.original);
        }
        let mut message = format!(
            "Moved {} to the trash · Ctrl+Z puts {} back",
            format::count(trashed.len(), "item", "items"),
            if trashed.len() == 1 { "it" } else { "them" }
        );
        if let Some((path, why)) = failed.first() {
            let name = path
                .file_name()
                .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
            message = if trashed.is_empty() {
                format!("“{name}” can't go to the trash: {why} · Shift+Delete deletes it for good")
            } else {
                format!("{message} · “{name}” couldn't: {why}")
            };
        }
        if !trashed.is_empty() {
            self.remember(Done::Trashed(trashed));
        }
        self.toast(message, cx);
    }

    /// Delete `paths` for good — from the trash with their records, else
    /// as a job.
    fn delete_for_good(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_>) {
        for path in &paths {
            self.selection.remove(path);
        }
        if self.in_trash() {
            self.delete_from_trash(&paths, cx);
        } else {
            self.start_op(Op::Delete(paths), cx);
        }
    }

    pub(super) fn ask_delete(&mut self, cx: &mut Cx<'_>) {
        if self.read_only(cx) {
            return;
        }
        let targets = self.targets();
        if !targets.is_empty() {
            self.overlay = Some(Overlay::ConfirmDelete(targets));
            cx.redraw();
        }
    }

    // The clipboard.

    /// Put what's chosen on the clipboard, to copy or (with `cut`) to move.
    pub(super) fn copy_files(&mut self, cut: bool, cx: &mut Cx<'_>) {
        if self.read_only(cx) {
            return;
        }
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let uris: Vec<String> = paths.iter().map(|path| uri::from_path(path)).collect();
        let copied = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
        let text = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let mut clip = Clip::default()
            .with(COPIED_FILES, copied.into_bytes())
            .with(
                URI_LIST,
                uri::format_list(paths.iter().map(PathBuf::as_path)).into_bytes(),
            );
        for mime in TEXT_TYPES {
            clip = clip.with(mime, text.clone().into_bytes());
        }
        cx.copy(clip);
        let verb = if cut { "Cut" } else { "Copied" };
        self.toast(
            format!(
                "{verb} {} · Ctrl+V puts {} here",
                format::count(paths.len(), "item", "items"),
                if paths.len() == 1 { "it" } else { "them" }
            ),
            cx,
        );
    }

    pub(super) fn ask_paste(&mut self, cx: &mut Cx<'_>) {
        if !self.in_a_folder(cx) {
            return;
        }
        let mut mimes = vec![COPIED_FILES, URI_LIST];
        mimes.extend(TEXT_TYPES);
        cx.paste(&mimes);
    }

    /// Files from the clipboard: copy (or move) them here. Returns whether
    /// the clipboard held files.
    pub(super) fn paste_files(&mut self, mime: &str, data: &[u8], cx: &mut Cx<'_>) -> bool {
        let text = String::from_utf8_lossy(data);
        let (cut, paths) = if mime == COPIED_FILES {
            let mut lines = text.lines();
            let cut = lines.next() == Some("cut");
            (cut, lines.filter_map(uri::to_path).collect())
        } else if mime == URI_LIST {
            (false, uri::parse_list(&text))
        } else {
            // Plain text: paths, one per line, if every line is one.
            let paths: Vec<PathBuf> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect();
            if paths.is_empty() || !paths.iter().all(|path| path.is_absolute() && path.exists()) {
                return false;
            }
            (false, paths)
        };
        if paths.is_empty() {
            return false;
        }
        let into = self.pane.location().to_owned();
        let op = if cut {
            Op::Move {
                sources: paths,
                into,
            }
        } else {
            Op::Copy {
                sources: paths,
                into,
            }
        };
        self.start_op(op, cx);
        true
    }

    pub(super) fn terminal_here(&mut self, cx: &mut Cx<'_>) {
        let dir = self.pane.location().to_owned();
        let terminal = self.ctx.config.appearance.terminal_program();
        if let Err(err) = enso_fs::apps::spawn_detached(&[terminal], Some(&dir)) {
            self.toast(format!("Can't open a terminal: {err}"), cx);
        }
    }

    // Keys.

    /// Keys for the questions and the name being edited, which come first
    /// when they're open. Returns whether the key was theirs.
    pub(super) fn ops_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        if let Some(asked) = self.overlay.asked_mut() {
            match key.key {
                Key::Char('r') => self.answer(Resolve::Replace, cx),
                Key::Char('k') | Key::Enter => self.answer(Resolve::KeepBoth, cx),
                Key::Char('s') => self.answer(Resolve::Skip, cx),
                Key::Char('a') => {
                    asked.all = !asked.all;
                    cx.redraw();
                }
                Key::Escape => self.answer(Resolve::Cancel, cx),
                _ => {}
            }
            return true;
        }
        if let Some(paths) = self.overlay.confirm_delete() {
            match key.key {
                Key::Enter | Key::Delete => {
                    let paths = paths.clone();
                    self.overlay = None;
                    self.delete_for_good(paths, cx);
                }
                Key::Escape => self.overlay = None,
                _ => {}
            }
            cx.redraw();
            return true;
        }
        let Some(renaming) = &mut self.renaming else {
            return false;
        };
        match renaming.field.key(key) {
            field::Response::Ignored => return true,
            field::Response::Submit => self.commit_rename(cx),
            field::Response::Cancel => self.renaming = None,
            field::Response::Changed => renaming.error = None,
            field::Response::Copy(text) | field::Response::Cut(text) => cx.copy(Clip::text(&text)),
            field::Response::Paste => cx.paste(&TEXT_TYPES),
            field::Response::Moved => {}
        }
        cx.redraw();
        true
    }

    /// A press while a question is open: only its answers count. Returns
    /// whether a question was open.
    pub(super) fn ops_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.overlay.asked().is_some() {
            match hit {
                Some(Hit::Resolve(resolve)) => self.answer(*resolve, cx),
                Some(Hit::ForAll) => {
                    if let Some(asked) = self.overlay.asked_mut() {
                        asked.all = !asked.all;
                    }
                }
                _ => {}
            }
            return true;
        }
        if let Some(paths) = self.overlay.confirm_delete().cloned() {
            match hit {
                Some(Hit::DeleteForGood) => {
                    self.overlay = None;
                    self.delete_for_good(paths, cx);
                }
                Some(Hit::KeepFiles) | None => self.overlay = None,
                _ => {}
            }
            return true;
        }
        if matches!(hit, Some(Hit::StopOp))
            && let Some(running) = &self.running
        {
            running.stop.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    // Drawing.

    /// The toast, low in the middle of the list.
    pub(super) fn draw_toast(&mut self, canvas: &mut Canvas<'_>, above: f32) {
        let Some((text, _)) = &self.toast else { return };
        let colors = &self.ctx.theme.colors;
        let size = self.font(type_scale::SMALL);
        let bounds = canvas.bounds();
        let label =
            Text::new(text, size, colors.text).max_width((bounds.w - 4.0 * space::XL).max(100.0));
        let measured = canvas.measure(&label);
        let (w, h) = (measured.w + 2.0 * space::LG, measured.h + 2.0 * space::MD);
        let rect = Rect::new((bounds.w - w) / 2.0, above - h - space::LG, w, h);
        canvas.shadow(rect, radius::pill(h.min(40.0)), Shadow::FLOATING);
        canvas.fill(rect, radius::CONTROL.max(h.min(40.0) / 2.0), colors.overlay);
        canvas.text(&label, rect.x + space::LG, rect.y + space::MD);
    }

    /// The operation under way, at the right of the status line. Returns
    /// whether one is.
    pub(super) fn draw_running(&mut self, canvas: &mut Canvas<'_>, inner: Rect) -> bool {
        let Some(running) = &self.running else {
            return false;
        };
        let colors = &self.ctx.theme.colors;
        let accent = self.ctx.accent();
        let small = self.font(type_scale::SMALL);
        let progress = &running.progress;
        let fraction = if progress.total_bytes > 0 {
            progress.bytes as f32 / progress.total_bytes as f32
        } else if progress.total_items > 0 {
            progress.items as f32 / progress.total_items as f32
        } else {
            0.0
        };
        let label = format!(
            "{} · {:.0}%",
            running.label,
            fraction.clamp(0.0, 1.0) * 100.0
        );
        let stop = Rect::new(inner.right() - 20.0, inner.y, 20.0, inner.h);
        canvas.icon(
            enso_core::style::icon::CLOSE,
            12.0,
            if self.hover == Some(Hit::StopOp) {
                colors.love
            } else {
                colors.subtle
            },
            stop,
            Align::Center,
        );
        let bar = Rect::new(
            stop.x - space::SM - 120.0,
            inner.y + inner.h / 2.0 - 2.0,
            120.0,
            4.0,
        );
        canvas.pill(bar, colors.highlight_med);
        canvas.pill(
            Rect::new(bar.x, bar.y, (bar.w * fraction).max(4.0), bar.h),
            accent,
        );
        let text = Rect::new(inner.x, inner.y, bar.x - space::SM - inner.x, inner.h);
        canvas.text_in(&Text::new(&label, small, colors.subtle), text, Align::End);
        self.hits.push((stop, Hit::StopOp));
        true
    }

    /// The name being edited, in place of the row's name.
    pub(super) fn draw_rename(&mut self, canvas: &mut Canvas<'_>, rect: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.font(type_scale::BODY);
        let Some(renaming) = &mut self.renaming else {
            return;
        };
        let box_ = Rect::new(
            rect.x - space::SM,
            rect.y + 3.0,
            rect.w + space::SM,
            rect.h - 6.0,
        );
        canvas.fill(box_, radius::CONTROL, colors.base);
        let ring = if renaming.error.is_some() {
            colors.love
        } else {
            accent
        };
        canvas.stroke(box_, radius::CONTROL, stroke::FOCUS, ring);
        let style = field::Style {
            size,
            color: colors.text,
            placeholder: "Name",
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        renaming.field.draw(canvas, rect, &style, true);
    }

    /// The questions: a name taken, or deleting for good.
    pub(super) fn draw_questions(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let look = self.look();
        let name_of = |path: &Path| {
            path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        };
        let hovered = |hit: Hit| Interaction {
            hovered: self.hover == Some(hit),
            ..Interaction::default()
        };
        let now = self.machine.now;
        if let Some(asked) = self.overlay.asked() {
            let folder = asked.target.parent().map(&name_of).unwrap_or_default();
            let title = format!("“{}” is already in {folder}", name_of(&asked.target));
            let describe = |path: &Path| {
                std::fs::metadata(path).map_or_else(
                    |_| String::new(),
                    |meta| {
                        let when = meta
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
                        let zone = now();
                        let when = when.map_or_else(String::new, |since| {
                            format::when(since.as_nanos() as i64, &zone)
                        });
                        if meta.is_dir() {
                            format!("a folder, {when}")
                        } else {
                            format!("{}, {when}", format::size(meta.len()))
                        }
                    },
                )
            };
            let (there, arriving) = (describe(&asked.target), describe(&asked.source));
            let detail = if there.is_empty() && arriving.is_empty() {
                String::new()
            } else {
                format!("There: {there} · Arriving: {arriving}")
            };
            let placed = Form::new(&title)
                .width(480.0)
                .about(&detail)
                .part(Part::Toggle {
                    label: "Do the same for the rest (A)",
                    on: asked.all,
                })
                .button(
                    "Skip (S)",
                    Kind::Secondary,
                    hovered(Hit::Resolve(Resolve::Skip)),
                )
                .button(
                    "Keep both (K)",
                    Kind::Primary,
                    hovered(Hit::Resolve(Resolve::KeepBoth)),
                )
                .button(
                    "Replace (R)",
                    Kind::Danger,
                    hovered(Hit::Resolve(Resolve::Replace)),
                )
                .draw(canvas, bounds, &look, 1.0);
            self.hits.push((placed.parts[0], Hit::ForAll));
            for (rect, resolve) in
                placed
                    .buttons
                    .iter()
                    .zip([Resolve::Skip, Resolve::KeepBoth, Resolve::Replace])
            {
                self.hits.push((*rect, Hit::Resolve(resolve)));
            }
            return;
        }
        if let Some(paths) = self.overlay.confirm_delete() {
            let count = paths.len();
            let title = if count == 1 {
                format!("Delete “{}” for good?", name_of(&paths[0]))
            } else {
                format!("Delete {count} items for good?")
            };
            let about = if self.in_trash() {
                "They're in the trash; this takes them out of it, for good."
            } else {
                "They won't go to the trash, and can't be brought back."
            };
            let placed = Form::new(&title)
                .about(about)
                .button("Cancel", Kind::Secondary, hovered(Hit::KeepFiles))
                .button(
                    "Delete",
                    Kind::Danger,
                    Interaction {
                        focused: true,
                        ..hovered(Hit::DeleteForGood)
                    },
                )
                .draw(canvas, bounds, &look, 1.0);
            self.hits.push((placed.buttons[1], Hit::DeleteForGood));
            self.hits.push((placed.buttons[0], Hit::KeepFiles));
        }
    }
}
