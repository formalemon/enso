//! The settings panel (`Ctrl+,`): the file manager's own settings, on a
//! card over the window. What's changed here is written to the state
//! directory, over `config.toml` ([`crate::settings`]); each change shows
//! at once.

use enso_core::style::{radius, space, stroke, type_scale};
use enso_fs::git::{self, Scope};
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Nav, Rect, Text,
    widget::{Button, Interaction, Kind, Toggle, controls},
};

use super::{Files, Hit};
use crate::{
    config::{Density, SortBy},
    settings,
};

/// A line of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Hidden,
    FoldersFirst,
    Compact,
    Reversed,
    Sort,
    Git,
    /// Who commits everywhere.
    Global,
    /// Who commits in this repository.
    Repository,
}

const SORTS: [(SortBy, &str); 4] = [
    (SortBy::Name, "Name"),
    (SortBy::Size, "Size"),
    (SortBy::Modified, "Date"),
    (SortBy::Extension, "Type"),
];

impl Files {
    /// The panel's lines, as things stand.
    pub(super) fn setting_rows(&self) -> Vec<Row> {
        let mut rows = vec![
            Row::Hidden,
            Row::FoldersFirst,
            Row::Compact,
            Row::Reversed,
            Row::Sort,
            Row::Git,
        ];
        if self.git_on() {
            rows.push(Row::Global);
            if git::root(self.pane.location()).is_some() {
                rows.push(Row::Repository);
            }
        }
        rows
    }

    pub(super) fn save_overrides(&mut self, cx: &mut Cx<'_>) {
        let path = self.ctx.paths.state_dir.join(settings::FILE);
        if let Err(err) = self.overrides.save(&path) {
            self.toast(format!("Can't keep the setting: {err}"), cx);
        }
        self.apply_config();
    }

    /// Make the window match the settings: config.toml with the changes
    /// over it.
    pub(super) fn apply_config(&mut self) {
        self.config = self.overrides.over(&self.base_config);
        self.pane.set_sort(self.config.sort());
        if self.pane.shows_hidden() != self.config.hidden {
            self.pane.toggle_hidden();
        }
    }

    /// Change the setting on `row` (`step` moves the sort left or right).
    pub(super) fn change(&mut self, row: Row, step: isize, cx: &mut Cx<'_>) {
        let config = self.config.clone();
        match row {
            Row::Hidden => self.overrides.hidden = Some(!config.hidden),
            Row::FoldersFirst => self.overrides.folders_first = Some(!config.folders_first),
            Row::Compact => {
                self.overrides.density = Some(match config.density {
                    Density::Compact => Density::Comfortable,
                    Density::Comfortable => Density::Compact,
                });
            }
            Row::Reversed => self.overrides.descending = Some(!config.descending),
            Row::Sort => {
                let at = SORTS
                    .iter()
                    .position(|(by, _)| *by == config.sort)
                    .unwrap_or(0) as isize;
                let next = (at + if step == 0 { 1 } else { step }).rem_euclid(SORTS.len() as isize);
                self.overrides.sort = Some(SORTS[next as usize].0);
            }
            Row::Git => {
                if git::available() {
                    self.set_git(!self.git_on(), cx);
                    return;
                }
            }
            Row::Global => {
                self.settings = None;
                self.edit_identity(Scope::Global);
                return;
            }
            Row::Repository => {
                if let Some(root) = git::root(self.pane.location()) {
                    self.settings = None;
                    self.edit_identity(Scope::Repository(root));
                }
                return;
            }
        }
        self.save_overrides(cx);
        cx.redraw();
    }

    /// Keys while the panel is open. Returns whether it was.
    pub(super) fn settings_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(focus) = self.settings else {
            return false;
        };
        let rows = self.setting_rows();
        let focus = focus.min(rows.len() - 1);
        if let Some(nav) = key.nav(true) {
            let next = match nav {
                Nav::Up => focus.checked_sub(1).unwrap_or(rows.len() - 1),
                Nav::Down => (focus + 1) % rows.len(),
            };
            self.settings = Some(next);
        } else {
            match key.key {
                Key::Escape => self.settings = None,
                Key::Char(',') if key.mods.ctrl => self.settings = None,
                Key::Enter | Key::Char(' ') => self.change(rows[focus], 0, cx),
                Key::Left if rows[focus] == Row::Sort => self.change(Row::Sort, -1, cx),
                Key::Right if rows[focus] == Row::Sort => self.change(Row::Sort, 1, cx),
                Key::Tab => self.settings = Some((focus + 1) % rows.len()),
                _ => {}
            }
        }
        cx.redraw();
        true
    }

    /// A press while the panel is open. Returns whether it was.
    pub(super) fn settings_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.settings.is_none() {
            return false;
        }
        let rows = self.setting_rows();
        match hit {
            Some(Hit::Setting(index)) => {
                self.settings = Some(*index);
                if let Some(row) = rows.get(*index) {
                    self.change(*row, 0, cx);
                }
            }
            Some(Hit::SortPill(by)) => {
                self.overrides.sort = Some(*by);
                self.save_overrides(cx);
            }
            Some(Hit::FormCancel) => self.settings = None,
            Some(_) => {}
            None => self.settings = None,
        }
        cx.redraw();
        true
    }

    pub(super) fn draw_settings(
        &mut self,
        canvas: &mut Canvas<'_>,
        bounds: Rect,
        now: std::time::Instant,
    ) {
        let Some(focus) = self.settings else { return };
        let rows = self.setting_rows();
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        self.hits.clear();
        let line = 44.0;
        let sections = 2.0;
        let height = 64.0 + rows.len() as f32 * line + sections * 34.0 + controls::HEIGHT + 40.0;
        let inner = controls::dialog(canvas, bounds, 560.0, height, &look, 1.0);
        canvas.text(
            &Text::new("Settings", self.font(type_scale::HEADING), colors.text).medium(),
            inner.x,
            inner.y,
        );
        let mut y = inner.y + 48.0;
        let config = self.config.clone();
        let facts = self.git_facts.clone();
        let (global, own) = (facts.global, facts.own);
        for (index, row) in rows.iter().enumerate() {
            if matches!(row, Row::Hidden | Row::Git) {
                let heading = if *row == Row::Hidden {
                    "View"
                } else {
                    "Version control"
                };
                canvas.text(
                    &Text::new(heading, small, colors.muted).medium(),
                    inner.x,
                    y + 6.0,
                );
                y += 34.0;
            }
            let rect = Rect::new(
                inner.x - space::SM,
                y,
                inner.w + 2.0 * space::SM,
                line - 4.0,
            );
            if index == focus {
                canvas.fill(rect, radius::CONTROL, accent.alpha(0.10));
                canvas.stroke(rect, radius::CONTROL, stroke::HAIRLINE, accent.alpha(0.5));
            } else if self.hover == Some(Hit::Setting(index)) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let label_rect = Rect::new(inner.x + space::SM, y, inner.w * 0.55, line - 4.0);
            let (label, note): (&str, Option<String>) = match row {
                Row::Hidden => ("Show dotfiles", None),
                Row::FoldersFirst => ("Folders before files", None),
                Row::Compact => ("Compact rows", None),
                Row::Reversed => ("Reverse the order", None),
                Row::Sort => ("Sort by", None),
                Row::Git => (
                    "Use version control",
                    (!facts.available).then(|| "git isn't installed".to_owned()),
                ),
                Row::Global => (
                    "Commit as, everywhere",
                    Some(if global.is_set() {
                        global.label()
                    } else {
                        "not set".to_owned()
                    }),
                ),
                Row::Repository => (
                    "Commit as, in this repository",
                    Some(match &own {
                        Some(identity) if identity.is_set() => identity.label(),
                        _ => "the global identity".to_owned(),
                    }),
                ),
            };
            canvas.text_in(
                &Text::new(label, body, colors.text),
                label_rect,
                Align::Start,
            );
            let control = Rect::new(
                inner.x + inner.w * 0.55,
                y,
                inner.w * 0.45 - space::SM,
                line - 4.0,
            );
            let toggle_at = Rect::new(
                control.right() - controls::TOGGLE.0,
                y + (line - 4.0 - controls::TOGGLE.1) / 2.0,
                controls::TOGGLE.0,
                controls::TOGGLE.1,
            );
            let on = match row {
                Row::Hidden => Some(config.hidden),
                Row::FoldersFirst => Some(config.folders_first),
                Row::Compact => Some(config.density == Density::Compact),
                Row::Reversed => Some(config.descending),
                Row::Git => Some(self.git_on()),
                _ => None,
            };
            if let Some(on) = on {
                let disabled = *row == Row::Git && !facts.available;
                Toggle::new(on).draw(
                    canvas,
                    toggle_at,
                    &look,
                    Interaction {
                        disabled,
                        ..Interaction::default()
                    },
                    now,
                );
                if let Some(note) = &note {
                    canvas.text_in(
                        &Text::new(note, small, colors.muted),
                        Rect::new(
                            control.x,
                            y,
                            toggle_at.x - control.x - space::SM,
                            line - 4.0,
                        ),
                        Align::End,
                    );
                }
            } else if *row == Row::Sort {
                let mut x = control.right();
                for (by, name) in SORTS.iter().rev() {
                    let text = Text::new(
                        name,
                        small,
                        if config.sort == *by {
                            look.base
                        } else {
                            colors.subtle
                        },
                    );
                    let width = canvas.measure(&text).w + 2.0 * space::MD;
                    let pill = Rect::new(x - width, y + 8.0, width, line - 20.0);
                    let fill = if config.sort == *by {
                        accent
                    } else {
                        colors.overlay
                    };
                    canvas.fill(pill, radius::pill(pill.h), fill);
                    canvas.text_in(&text, pill, Align::Center);
                    self.hits.push((pill, Hit::SortPill(*by)));
                    x = pill.x - space::XS;
                }
            } else if let Some(note) = &note {
                canvas.text_in(
                    &Text::new(&format!("{note}  ›"), small, colors.subtle).max_width(control.w),
                    control,
                    Align::End,
                );
            }
            self.hits.insert(0, (rect, Hit::Setting(index)));
            y += line;
        }
        let path = self.ctx.paths.state_dir.join(settings::FILE);
        let footer = format!("Kept in {}, over config.toml's [files].", path.display());
        canvas.text(
            &Text::new(&footer, small, colors.muted).max_width(inner.w - 120.0),
            inner.x,
            inner.bottom() - controls::HEIGHT + 8.0,
        );
        let done = Button::new("Done", Kind::Secondary);
        let width = done.width(canvas, &look);
        let rect = Rect::new(
            inner.right() - width,
            inner.bottom() - controls::HEIGHT,
            width,
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
