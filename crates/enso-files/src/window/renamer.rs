//! Renaming many at once (`F2` with several chosen, when Settings has it
//! on): find and replace, a template with numbering and dates, a change of
//! case — every name shown before and after as you type, with what won't
//! do marked — or the names in `$EDITOR`, one a line.
//!
//! It's done in one step, swaps and shifted numbers included, and undone
//! in one (`Ctrl+Z`).

use std::{path::PathBuf, thread};

use enso_core::style::{opacity, radius, space, stroke, type_scale};
use enso_fs::ops;
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Rect, Text,
    widget::{Button, Interaction, Kind, TextField, controls, field},
};

use super::{Files, Hit};
use crate::{
    bulk::{self, Case, Item, Planned, Rule},
    jobs::Message,
};

/// Preview rows shown at once.
const ROWS: usize = 8;
const ROW: f32 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenameHit {
    Field(usize),
    Pattern,
    Case(Case),
    Editor,
    Apply,
    Cancel,
}

#[derive(Debug)]
pub(super) struct Renamer {
    items: Vec<Item>,
    /// Find, and replace with.
    pub(super) fields: [TextField; 2],
    focus: usize,
    regex: bool,
    case: Case,
    /// Names from the editor, in place of the rule until the rule changes.
    edited: Option<Vec<Planned>>,
    planned: Result<Vec<Planned>, String>,
    /// In `$EDITOR` now.
    editing: bool,
}

impl Renamer {
    fn rule(&self) -> Rule {
        Rule {
            find: self.fields[0].text().to_owned(),
            replace: self.fields[1].text().to_owned(),
            regex: self.regex,
            case: self.case,
        }
    }

    pub(super) fn replan(&mut self) {
        self.edited = None;
        self.planned = bulk::plan(&self.items, &self.rule());
    }

    fn shown(&self) -> Result<&[Planned], &str> {
        match (&self.edited, &self.planned) {
            (Some(edited), _) => Ok(edited),
            (None, Ok(planned)) => Ok(planned),
            (None, Err(why)) => Err(why),
        }
    }

    /// The renames to make, if every name will do and something changes.
    fn ready(&self) -> Option<Vec<(PathBuf, PathBuf)>> {
        let planned = self.shown().ok()?;
        if planned.iter().any(|item| item.problem.is_some()) {
            return None;
        }
        let pairs: Vec<_> = planned
            .iter()
            .filter(|item| item.changes())
            .map(|item| (item.from.clone(), item.to()))
            .collect();
        (!pairs.is_empty()).then_some(pairs)
    }
}

impl Files {
    /// F2: rename what's chosen — all at once if there are several and
    /// Settings says so; else the one under the cursor.
    pub(super) fn rename_targets(&mut self) -> bool {
        let targets = self.targets();
        if targets.len() < 2 || !self.config.bulk_rename || self.picker.is_some() || self.in_trash()
        {
            return false;
        }
        // In the order they're shown, so numbering follows the list.
        let mut ordered: Vec<PathBuf> = (0..self.pane.len())
            .map(|row| self.pane.path(row))
            .filter(|path| targets.contains(path))
            .collect();
        for path in targets {
            if !ordered.contains(&path) {
                ordered.push(path);
            }
        }
        let zone = jiff::tz::TimeZone::system();
        let items: Vec<Item> = ordered
            .into_iter()
            .map(|path| {
                let meta = std::fs::symlink_metadata(&path).ok();
                let date = meta
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|time| jiff::Timestamp::try_from(time).ok())
                    .map(|time| time.to_zoned(zone.clone()).strftime("%Y-%m-%d").to_string());
                Item {
                    is_dir: meta.is_some_and(|meta| meta.is_dir()),
                    path,
                    date,
                }
            })
            .collect();
        let mut renamer = Renamer {
            items,
            fields: [TextField::new(""), TextField::new("")],
            focus: 0,
            regex: false,
            case: Case::AsIs,
            edited: None,
            planned: Ok(Vec::new()),
            editing: false,
        };
        renamer.replan();
        self.renamer = Some(renamer);
        true
    }

    /// Keys while renaming many. Returns whether it was open.
    pub(super) fn renamer_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(renamer) = &mut self.renamer else {
            return false;
        };
        if renamer.editing {
            return true;
        }
        match key.key {
            Key::Escape => self.renamer = None,
            Key::Enter => self.apply_rename(cx),
            Key::Tab => renamer.focus = 1 - renamer.focus,
            Key::Char('e') if key.mods.ctrl => self.rename_in_editor(cx),
            Key::Char('r') if key.mods.ctrl => {
                renamer.regex = !renamer.regex;
                renamer.replan();
            }
            _ => {
                let focus = renamer.focus;
                match renamer.fields[focus].key(key) {
                    field::Response::Changed | field::Response::Cut(_) => renamer.replan(),
                    field::Response::Copy(text) => cx.copy(enso_ui::Clip::text(&text)),
                    field::Response::Paste => cx.paste(&enso_ui::TEXT_TYPES),
                    _ => {}
                }
            }
        }
        cx.redraw();
        true
    }

    pub(super) fn renamer_paste(&mut self, text: &str) -> bool {
        let Some(renamer) = &mut self.renamer else {
            return false;
        };
        let focus = renamer.focus;
        renamer.fields[focus].insert(text);
        renamer.replan();
        true
    }

    /// A press while renaming many. Returns whether it was open.
    pub(super) fn renamer_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        let Some(renamer) = &mut self.renamer else {
            return false;
        };
        if renamer.editing {
            return true;
        }
        match hit {
            Some(Hit::Rename(RenameHit::Field(index))) => renamer.focus = *index,
            Some(Hit::Rename(RenameHit::Pattern)) => {
                renamer.regex = !renamer.regex;
                renamer.replan();
            }
            Some(Hit::Rename(RenameHit::Case(case))) => {
                renamer.case = *case;
                renamer.replan();
            }
            Some(Hit::Rename(RenameHit::Editor)) => self.rename_in_editor(cx),
            Some(Hit::Rename(RenameHit::Apply)) => self.apply_rename(cx),
            Some(Hit::Rename(RenameHit::Cancel)) => self.renamer = None,
            _ => {}
        }
        cx.redraw();
        true
    }

    fn apply_rename(&mut self, cx: &mut Cx<'_>) {
        match self.commit_renames() {
            Some(Ok(count)) => {
                let count = enso_fs::format::count(count, "item", "items");
                self.toast(format!("Renamed {count} · Ctrl+Z puts the names back"), cx);
            }
            Some(Err(err)) => self.toast(format!("Nothing was renamed: {err}"), cx),
            None => {}
        }
    }

    /// Rename, if every name will do. Returns how many, or why not.
    pub(super) fn commit_renames(&mut self) -> Option<Result<usize, String>> {
        let pairs = self.renamer.as_ref().and_then(Renamer::ready)?;
        Some(match ops::rename_all(&pairs) {
            Ok(done) => {
                // What was chosen stays chosen, under its new name.
                for (from, to) in &pairs {
                    if self.selection.contains(from) {
                        self.selection.remove(from);
                        self.selection.insert(to.clone());
                    }
                }
                self.remember(done);
                self.renamer = None;
                Ok(pairs.len())
            }
            Err(err) => Err(err.to_string()),
        })
    }

    /// The names in `$EDITOR`, one a line, in a terminal; what's saved
    /// comes back as the new names to look over.
    fn rename_in_editor(&mut self, cx: &mut Cx<'_>) {
        let (Some(renamer), Some(proxy)) = (&mut self.renamer, self.proxy.clone()) else {
            return;
        };
        let names: String = renamer
            .shown()
            .unwrap_or_default()
            .iter()
            .map(|item| format!("{}\n", item.name))
            .collect();
        // Before anything was planned: the names as they are.
        let names = if names.is_empty() {
            renamer
                .items
                .iter()
                .map(|item| {
                    format!(
                        "{}\n",
                        item.path.file_name().unwrap_or_default().to_string_lossy()
                    )
                })
                .collect()
        } else {
            names
        };
        renamer.editing = true;
        cx.redraw();
        let _ = thread::Builder::new()
            .name("enso-files-editor".to_owned())
            .spawn(move || {
                let file = std::env::temp_dir()
                    .join(format!("enso-files-rename-{}.txt", std::process::id()));
                let result = std::fs::write(&file, names)
                    .and_then(|()| {
                        std::process::Command::new("alacritty")
                            .args([
                                "--title",
                                "Rename — save and close to finish",
                                "-e",
                                "sh",
                                "-c",
                            ])
                            .arg("exec ${VISUAL:-${EDITOR:-nvim}} \"$1\"")
                            .arg("sh")
                            .arg(&file)
                            .status()
                    })
                    .and_then(|_| std::fs::read_to_string(&file))
                    .map_err(|err| format!("The editor didn't open: {err}"));
                let _ = std::fs::remove_file(&file);
                proxy.send(Message::Edited(result));
            });
    }

    /// The editor closed: its names, to look over before renaming.
    pub(super) fn edited(&mut self, result: Result<String, String>, cx: &mut Cx<'_>) {
        let Some(renamer) = &mut self.renamer else {
            return;
        };
        renamer.editing = false;
        match result.and_then(|text| bulk::from_lines(&renamer.items, &text)) {
            Ok(planned) => renamer.edited = Some(planned),
            Err(why) => self.toast(why, cx),
        }
        cx.redraw();
    }

    pub(super) fn draw_renamer(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        if self.renamer.is_none() {
            return;
        }
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        self.hits.clear();
        let Some(renamer) = &mut self.renamer else {
            return;
        };
        let count = renamer.items.len();
        let rows = count.min(ROWS);
        let height = 44.0
            + 66.0
            + 40.0
            + 24.0
            + rows as f32 * ROW
            + space::LG
            + controls::HEIGHT
            + 2.0 * space::XL;
        let inner = controls::dialog(canvas, bounds, 660.0, height, &look, 1.0);
        let title = format!("Rename {}", enso_fs::format::count(count, "item", "items"));
        canvas.text(
            &Text::new(
                &title,
                self.ctx.config.appearance.font_size * type_scale::HEADING,
                colors.text,
            )
            .medium(),
            inner.x,
            inner.y,
        );
        let mut y = inner.y + 44.0;
        let mut hits = Vec::new();

        // The two fields, side by side.
        let half = (inner.w - space::MD) / 2.0;
        for (index, label) in ["Find (empty: the whole name)", "Replace with"]
            .into_iter()
            .enumerate()
        {
            let x = inner.x + index as f32 * (half + space::MD);
            canvas.text(&Text::new(label, small, colors.muted), x, y);
            let rect = Rect::new(x, y + 22.0, half, 38.0);
            let focused = renamer.focus == index && !renamer.editing;
            canvas.fill(rect, radius::CONTROL, colors.base);
            canvas.stroke(
                rect,
                radius::CONTROL,
                if focused {
                    stroke::FOCUS
                } else {
                    stroke::HAIRLINE
                },
                if focused {
                    accent
                } else {
                    colors.highlight_med
                },
            );
            let style = field::Style {
                size: body,
                color: colors.text,
                placeholder: if index == 0 { "IMG_" } else { "Holiday {n:02}" },
                placeholder_color: colors.muted,
                caret: accent,
                selection: accent.alpha(opacity::TINT * 2.0),
            };
            renamer.fields[index].draw(canvas, rect.inset_xy(space::MD, 0.0), &style, focused);
            hits.push((rect, Hit::Rename(RenameHit::Field(index))));
        }
        y += 66.0;

        // Pattern, case, editor.
        let pill = |canvas: &mut Canvas<'_>, x: f32, label: &str, on: bool| -> Rect {
            let text = Text::new(label, small, if on { look.base } else { colors.subtle });
            let w = canvas.measure(&text).w + 2.0 * space::MD;
            let rect = Rect::new(x, y + 4.0, w, 28.0);
            canvas.fill(
                rect,
                radius::pill(rect.h),
                if on { accent } else { colors.overlay },
            );
            canvas.text_in(&text, rect, Align::Center);
            rect
        };
        let rect = pill(canvas, inner.x, "Pattern", renamer.regex);
        hits.push((rect, Hit::Rename(RenameHit::Pattern)));
        let mut x = rect.right() + space::LG;
        for (case, label) in [
            (Case::AsIs, "As is"),
            (Case::Lower, "lower"),
            (Case::Upper, "UPPER"),
            (Case::Title, "Title"),
        ] {
            let rect = pill(canvas, x, label, renamer.case == case);
            hits.push((rect, Hit::Rename(RenameHit::Case(case))));
            x = rect.right() + space::XS;
        }
        let editor = Button::new("In $EDITOR…", Kind::Quiet);
        let w = editor.width(canvas, &look);
        let rect = Rect::new(inner.right() - w, y + 2.0, w, 32.0);
        editor.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Rename(RenameHit::Editor)),
                ..Interaction::default()
            },
        );
        hits.push((rect, Hit::Rename(RenameHit::Editor)));
        y += 40.0;
        let hint = if renamer.editing {
            "Editing in a terminal: save and close the editor to come back".to_owned()
        } else if renamer.edited.is_some() {
            "Names from the editor · change the rule to start again".to_owned()
        } else {
            "{n} counts, {n:03} pads · {name} {ext} {date} {folder} · Pattern takes $1".to_owned()
        };
        canvas.text(
            &Text::new(&hint, small, colors.muted).max_width(inner.w),
            inner.x,
            y,
        );
        y += 24.0;

        // Before and after.
        let list = Rect::new(inner.x, y, inner.w, rows as f32 * ROW);
        canvas.fill(
            list.inset_xy(-space::SM, -space::XS),
            radius::CONTROL,
            colors.base.alpha(0.5),
        );
        let (problems, changing) = match renamer.shown() {
            Err(why) => {
                canvas.text_in(&Text::new(why, body, colors.love), list, Align::Center);
                (1, 0)
            }
            Ok(planned) => {
                let column = (inner.w - 28.0) / 2.0;
                for (index, item) in planned.iter().take(ROWS).enumerate() {
                    let row = Rect::new(inner.x, y + index as f32 * ROW, inner.w, ROW);
                    let old_name = item.old_name();
                    let old = Text::new(&old_name, small, colors.subtle).max_width(column);
                    canvas.text_in(&old, Rect::new(row.x, row.y, column, row.h), Align::Start);
                    canvas.text_in(
                        &Text::new("→", small, colors.muted),
                        Rect::new(row.x + column, row.y, 28.0, row.h),
                        Align::Center,
                    );
                    let (name, ink) = match item.problem {
                        Some(problem) => (
                            format!("{} · {}", item.name, problem.describe()),
                            colors.love,
                        ),
                        None if item.changes() => (item.name.clone(), colors.text),
                        None => (item.name.clone(), colors.muted),
                    };
                    let new = Text::new(&name, small, ink).max_width(column);
                    canvas.text_in(
                        &new,
                        Rect::new(row.x + column + 28.0, row.y, column, row.h),
                        Align::Start,
                    );
                }
                (
                    planned.iter().filter(|item| item.problem.is_some()).count(),
                    planned.iter().filter(|item| item.changes()).count(),
                )
            }
        };

        // What it comes to, and the buttons.
        let row_y = inner.bottom() - controls::HEIGHT;
        let mut summary = format!("{changing} of {count} change");
        if count > ROWS {
            summary += &format!(" · first {ROWS} shown");
        }
        if problems > 0 && renamer.shown().is_ok() {
            summary += &format!(
                " · {}",
                enso_fs::format::count(problems, "won't do", "won't do")
            );
        }
        canvas.text_in(
            &Text::new(
                &summary,
                small,
                if problems > 0 {
                    colors.love
                } else {
                    colors.subtle
                },
            ),
            Rect::new(inner.x, row_y, inner.w * 0.5, controls::HEIGHT),
            Align::Start,
        );
        let ready = renamer.ready().is_some() && !renamer.editing;
        let apply = Button::new("Rename", Kind::Primary);
        let w = apply.width(canvas, &look);
        let apply_rect = Rect::new(inner.right() - w, row_y, w, controls::HEIGHT);
        apply.draw(
            canvas,
            apply_rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Rename(RenameHit::Apply)),
                disabled: !ready,
                ..Interaction::default()
            },
        );
        hits.push((apply_rect, Hit::Rename(RenameHit::Apply)));
        let cancel = Button::new("Cancel", Kind::Secondary);
        let w = cancel.width(canvas, &look);
        let rect = Rect::new(apply_rect.x - space::SM - w, row_y, w, controls::HEIGHT);
        cancel.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Rename(RenameHit::Cancel)),
                ..Interaction::default()
            },
        );
        hits.push((rect, Hit::Rename(RenameHit::Cancel)));
        self.hits.extend(hits);
    }
}
