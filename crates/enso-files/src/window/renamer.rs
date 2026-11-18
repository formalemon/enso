//! Renaming many at once (`F2` with several chosen, when Settings has it
//! on): find and replace, a template with numbering and dates, a change of
//! case — every name shown before and after as you type, with what won't
//! do marked — or the names in `$EDITOR`, one a line.
//!
//! It's done in one step, swaps and shifted numbers included, and undone
//! in one (`Ctrl+Z`).

use std::{path::PathBuf, thread};

use enso_core::style::{radius, space, type_scale};
use enso_fs::ops;
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Rect, Text,
    widget::{
        Button, Form, Interaction, Kind, Note, Part, TextField, controls, field, field::Frame, form,
    },
};

use super::overlay::{Overlay, Reach};
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
        self.overlay = Some(Overlay::Renamer(renamer));
        true
    }

    /// Keys while renaming many. Returns whether it was open.
    pub(super) fn renamer_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(renamer) = self.overlay.renamer_mut() else {
            return false;
        };
        if renamer.editing {
            return true;
        }
        match key.key {
            Key::Escape => self.overlay = None,
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
        let Some(renamer) = self.overlay.renamer_mut() else {
            return false;
        };
        let focus = renamer.focus;
        renamer.fields[focus].insert(text);
        renamer.replan();
        true
    }

    /// A press while renaming many. Returns whether it was open.
    pub(super) fn renamer_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        let Some(renamer) = self.overlay.renamer_mut() else {
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
            Some(Hit::Rename(RenameHit::Cancel)) => self.overlay = None,
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
        let pairs = self.overlay.renamer().and_then(Renamer::ready)?;
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
                self.overlay.take_renamer();
                Ok(pairs.len())
            }
            Err(err) => Err(err.to_string()),
        })
    }

    /// The names in `$EDITOR`, one a line, in a terminal; what's saved
    /// comes back as the new names to look over.
    fn rename_in_editor(&mut self, cx: &mut Cx<'_>) {
        let (Some(renamer), Some(proxy)) = (self.overlay.renamer_mut(), self.proxy.clone()) else {
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
        let terminal = self.ctx.config.appearance.terminal_program();
        let _ = thread::Builder::new()
            .name("enso-files-editor".to_owned())
            .spawn(move || {
                let file = std::env::temp_dir()
                    .join(format!("enso-files-rename-{}.txt", std::process::id()));
                let result = std::fs::write(&file, names)
                    .and_then(|()| {
                        // The title by escape code, which every terminal
                        // takes, unlike a flag for it.
                        std::process::Command::new(&terminal)
                            .args(["-e", "sh", "-c"])
                            .arg(
                                "printf '\\033]2;%s\\007' 'Rename — save and close to finish'; \
                                 exec ${VISUAL:-${EDITOR:-nvim}} \"$1\"",
                            )
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
        let Some(renamer) = self.overlay.renamer_mut() else {
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
        let Some(renamer) = self.overlay.renamer() else {
            return;
        };
        let look = self.look();
        let small = self.font(type_scale::SMALL);
        let count = renamer.items.len();
        let rows = count.min(ROWS);
        let title = format!("Rename {}", enso_fs::format::count(count, "item", "items"));
        let hint = if renamer.editing {
            "Editing in a terminal: save and close the editor to come back"
        } else if renamer.edited.is_some() {
            "Names from the editor · change the rule to start again"
        } else {
            "{n} counts, {n:03} pads · {name} {ext} {date} {folder} · Pattern takes $1"
        };
        let (problems, changing, planned) = match renamer.shown() {
            Err(_) => (1, 0, false),
            Ok(planned) => (
                planned.iter().filter(|item| item.problem.is_some()).count(),
                planned.iter().filter(|item| item.changes()).count(),
                true,
            ),
        };
        // What it comes to.
        let mut summary = format!("{changing} of {count} change");
        if count > ROWS {
            summary += &format!(" · first {ROWS} shown");
        }
        if problems > 0 && planned {
            summary += &format!(
                " · {}",
                enso_fs::format::count(problems, "won't do", "won't do")
            );
        }
        let ready = renamer.ready().is_some() && !renamer.editing;
        let hovered = |hit: RenameHit| Interaction {
            hovered: self.hover == Some(Hit::Rename(hit)),
            ..Interaction::default()
        };
        let label_h = (small * 1.6).round();
        let placed = Form::new(&title)
            .width(660.0)
            .about(hint)
            .part(Part::Space(label_h + field::HEIGHT))
            .part(Part::Space(controls::HEIGHT))
            .part(Part::Space(rows as f32 * ROW))
            .note(if problems > 0 {
                Note::Complains(&summary)
            } else {
                Note::Says(&summary)
            })
            .button("Cancel", Kind::Secondary, hovered(RenameHit::Cancel))
            .button(
                "Rename",
                Kind::Primary,
                Interaction {
                    disabled: !ready,
                    ..hovered(RenameHit::Apply)
                },
            )
            .draw(canvas, bounds, &look, 1.0);
        let mut hits = Vec::new();

        // The two fields, side by side.
        let place = placed.parts[0];
        let half = (place.w - space::MD) / 2.0;
        let editing = renamer.editing;
        let focus = renamer.focus;
        for (index, (label, placeholder)) in [
            ("Find (empty: the whole name)", "IMG_"),
            ("Replace with", "Holiday {n:02}"),
        ]
        .into_iter()
        .enumerate()
        {
            let x = place.x + index as f32 * (half + space::MD);
            canvas.text(&Text::new(label, small, look.muted), x, place.y);
            let rect = Rect::new(x, place.y + label_h, half, field::HEIGHT);
            let focused = focus == index && !editing;
            let frame = Frame {
                focused,
                filled: true,
                ..Frame::default()
            };
            let text = frame.draw(canvas, rect, &look, look.base);
            if let Some(renamer) = self.overlay.renamer_mut() {
                renamer.fields[index].draw(
                    canvas,
                    text,
                    &field::Style::new(&look, placeholder),
                    focused,
                );
            }
            hits.push((rect, Hit::Rename(RenameHit::Field(index))));
        }
        let Some(renamer) = self.overlay.renamer() else {
            return;
        };

        // Pattern, case, editor.
        let row = placed.parts[1];
        let y = row.y + (row.h - 26.0) / 2.0;
        let lit = |hit: RenameHit| self.hover == Some(Hit::Rename(hit));
        let rect = form::pill(
            canvas,
            (row.x, y),
            "Pattern",
            renamer.regex,
            lit(RenameHit::Pattern),
            &look,
        );
        hits.push((rect, Hit::Rename(RenameHit::Pattern)));
        let mut x = rect.right() + space::LG;
        for (case, label) in [
            (Case::AsIs, "As is"),
            (Case::Lower, "lower"),
            (Case::Upper, "UPPER"),
            (Case::Title, "Title"),
        ] {
            let on = renamer.case == case;
            let rect = form::pill(canvas, (x, y), label, on, lit(RenameHit::Case(case)), &look);
            hits.push((rect, Hit::Rename(RenameHit::Case(case))));
            x = rect.right() + space::XS;
        }
        let editor = Button::new("In $EDITOR…", Kind::Quiet);
        let w = editor.width(canvas, &look);
        let rect = Rect::new(row.right() - w, row.y, w, controls::HEIGHT);
        editor.draw(canvas, rect, &look, hovered(RenameHit::Editor));
        hits.push((rect, Hit::Rename(RenameHit::Editor)));

        // Before and after.
        let list = placed.parts[2];
        canvas.fill(
            list.inset_xy(-space::SM, -space::XS),
            radius::CONTROL,
            look.base.alpha(0.5),
        );
        match renamer.shown() {
            Err(why) => {
                canvas.text_in(&Text::new(why, look.size, look.danger), list, Align::Center);
            }
            Ok(planned) => {
                let column = (list.w - 28.0) / 2.0;
                for (index, item) in planned.iter().take(ROWS).enumerate() {
                    let row = Rect::new(list.x, list.y + index as f32 * ROW, list.w, ROW);
                    let old_name = item.old_name();
                    let old = Text::new(&old_name, small, look.subtle).max_width(column);
                    canvas.text_in(&old, Rect::new(row.x, row.y, column, row.h), Align::Start);
                    canvas.text_in(
                        &Text::new("→", small, look.muted),
                        Rect::new(row.x + column, row.y, 28.0, row.h),
                        Align::Center,
                    );
                    let (name, ink) = match item.problem {
                        Some(problem) => (
                            format!("{} · {}", item.name, problem.describe()),
                            look.danger,
                        ),
                        None if item.changes() => (item.name.clone(), look.text),
                        None => (item.name.clone(), look.muted),
                    };
                    let new = Text::new(&name, small, ink).max_width(column);
                    canvas.text_in(
                        &new,
                        Rect::new(row.x + column + 28.0, row.y, column, row.h),
                        Align::Start,
                    );
                }
            }
        }
        hits.push((placed.buttons[0], Hit::Rename(RenameHit::Cancel)));
        hits.push((placed.buttons[1], Hit::Rename(RenameHit::Apply)));
        self.hits.extend(hits);
    }
}
