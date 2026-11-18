//! The settings panel (`Ctrl+,`): the file manager's own settings, on a
//! card over the window. What's changed here is written to the state
//! directory, over `config.toml` ([`crate::settings`]); each change shows
//! at once.

use enso_core::style::{radius, space, stroke, type_scale};
use enso_fs::git::{self, Scope};
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Nav, Rect, Text,
    widget::{Button, Interaction, Kind, Toggle, controls, form},
};

use super::overlay::{Overlay, Reach};
use super::{Files, Hit};
use crate::{
    config::{Density, SortBy, View},
    settings,
};

/// A line of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Hidden,
    FoldersFirst,
    View,
    Compact,
    BulkRename,
    Reversed,
    Sort,
    Git,
    /// Who commits everywhere.
    Global,
    /// Who commits in this repository.
    Repository,
    /// The GitHub account.
    GitHub,
    /// git pushes and pulls with it.
    GitHubGit,
}

const VIEWS: [(View, &str); 3] = [
    (View::List, "List"),
    (View::Grid, "Grid"),
    (View::Columns, "Columns"),
];

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
            Row::View,
            Row::Compact,
            Row::Reversed,
            Row::Sort,
            Row::BulkRename,
            Row::Git,
        ];
        if self.git_on() {
            rows.push(Row::Global);
            if git::root(self.pane.location()).is_some() {
                rows.push(Row::Repository);
            }
            rows.push(Row::GitHub);
            if self.github_account().is_some() {
                rows.push(Row::GitHubGit);
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
    /// over it — and over those, this folder's own view, if it has one.
    pub(super) fn apply_config(&mut self) {
        let here = self.pane.location().to_owned();
        self.apply_folder_view(&here);
    }

    /// Show `folder` as it was last shown, where that was changed; else
    /// as Settings says.
    pub(super) fn apply_folder_view(&mut self, folder: &std::path::Path) {
        let default = self.overrides.over(&self.base_config);
        let own = if self.pane.source().is_folder() {
            self.folder_views.get(folder)
        } else {
            crate::views::FolderView::default()
        };
        self.config = own.over(&default);
        self.pane.set_sort(self.config.sort());
        if self.pane.shows_hidden() != self.config.hidden {
            self.pane.toggle_hidden();
        }
        self.zoom = own.zoom.unwrap_or(super::grid::ZOOM);
    }

    pub(super) fn remember_zoom(&mut self, cx: &mut Cx<'_>) {
        let zoom = self.zoom;
        self.remember_here(|own| own.zoom = Some(zoom), cx);
    }

    pub(super) fn toggle_hidden_here(&mut self, cx: &mut Cx<'_>) {
        self.pane.toggle_hidden();
        self.config.hidden = self.pane.shows_hidden();
        let hidden = self.config.hidden;
        self.remember_here(|own| own.hidden = Some(hidden), cx);
    }

    pub(super) fn sort_here(&mut self, sort: enso_fs::sort::Sort, cx: &mut Cx<'_>) {
        self.pane.set_sort(sort);
        let by: crate::config::SortBy = sort.by.into();
        self.config.sort = by;
        self.config.descending = sort.descending;
        self.config.folders_first = sort.folders_first;
        self.remember_here(
            |own| {
                own.sort = Some(by);
                own.descending = Some(sort.descending);
                own.folders_first = Some(sort.folders_first);
            },
            cx,
        );
    }

    /// Remember a change to how this folder is shown. The first time a
    /// folder keeps its own view, say so: Settings decides the rest.
    pub(super) fn remember_here(
        &mut self,
        change: impl FnOnce(&mut crate::views::FolderView),
        cx: &mut Cx<'_>,
    ) {
        if !self.pane.source().is_folder() {
            return;
        }
        let here = self.pane.location().to_owned();
        if self.folder_views.change(&here, change) {
            self.toast(
                "This folder keeps its own view · Settings sets the rest",
                cx,
            );
        }
        let file = self.ctx.paths.state_dir.join(crate::views::FILE);
        let _ = self.folder_views.save(&file);
    }

    /// Change the setting on `row` (`step` moves the sort left or right).
    pub(super) fn change(&mut self, row: Row, step: isize, cx: &mut Cx<'_>) {
        let config = self.config.clone();
        match row {
            Row::Hidden => self.overrides.hidden = Some(!config.hidden),
            Row::FoldersFirst => self.overrides.folders_first = Some(!config.folders_first),
            Row::View => {
                let at = VIEWS
                    .iter()
                    .position(|(view, _)| *view == config.view)
                    .unwrap_or(0) as isize;
                let next = (at + if step == 0 { 1 } else { step }).rem_euclid(VIEWS.len() as isize);
                self.overrides.view = Some(VIEWS[next as usize].0);
            }
            Row::Compact => {
                self.overrides.density = Some(match config.density {
                    Density::Compact => Density::Comfortable,
                    Density::Comfortable => Density::Compact,
                });
            }
            Row::Reversed => self.overrides.descending = Some(!config.descending),
            Row::BulkRename => self.overrides.bulk_rename = Some(!config.bulk_rename),
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
                self.overlay = None;
                self.edit_identity(Scope::Global);
                return;
            }
            Row::GitHub => {
                self.overlay = None;
                self.github_row(cx);
                return;
            }
            Row::GitHubGit => {
                self.toggle_github_git(cx);
                cx.redraw();
                return;
            }
            Row::Repository => {
                if let Some(root) = git::root(self.pane.location()) {
                    self.overlay = None;
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
        let Some(&focus) = self.overlay.settings() else {
            return false;
        };
        let rows = self.setting_rows();
        let focus = focus.min(rows.len() - 1);
        if let Some(nav) = key.nav(true) {
            let next = match nav {
                Nav::Up => focus.checked_sub(1).unwrap_or(rows.len() - 1),
                Nav::Down => (focus + 1) % rows.len(),
            };
            self.overlay = Some(Overlay::Settings(next));
        } else {
            match key.key {
                Key::Escape => self.overlay = None,
                Key::Char(',') if key.mods.ctrl => self.overlay = None,
                Key::Enter | Key::Char(' ') => self.change(rows[focus], 0, cx),
                Key::Left | Key::Char('h') if matches!(rows[focus], Row::Sort | Row::View) => {
                    self.change(rows[focus], -1, cx);
                }
                Key::Right | Key::Char('l') if matches!(rows[focus], Row::Sort | Row::View) => {
                    self.change(rows[focus], 1, cx);
                }
                Key::Tab => self.overlay = Some(Overlay::Settings((focus + 1) % rows.len())),
                _ => {}
            }
        }
        cx.redraw();
        true
    }

    /// A press while the panel is open. Returns whether it was.
    pub(super) fn settings_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.overlay.settings().is_none() {
            return false;
        }
        let rows = self.setting_rows();
        match hit {
            Some(Hit::Setting(index)) => {
                self.overlay = Some(Overlay::Settings(*index));
                if let Some(row) = rows.get(*index) {
                    self.change(*row, 0, cx);
                }
            }
            Some(Hit::SortPill(by)) => {
                self.overrides.sort = Some(*by);
                self.save_overrides(cx);
            }
            Some(Hit::ViewPill(view)) => {
                self.overrides.view = Some(*view);
                self.save_overrides(cx);
            }
            Some(Hit::FormCancel) => self.overlay = None,
            Some(_) => {}
            None => self.overlay = None,
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
        let Some(&focus) = self.overlay.settings() else {
            return;
        };
        let rows = self.setting_rows();
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        let line = 44.0;
        let sections = 3.0;
        let height = 64.0 + rows.len() as f32 * line + sections * 34.0 + controls::HEIGHT + 40.0;
        let inner = controls::dialog(canvas, bounds, 560.0, height, &look, 1.0);
        canvas.text(
            &Text::new("Settings", form::title_size(&look), colors.text).medium(),
            inner.x,
            inner.y,
        );
        let mut y = inner.y + 48.0;
        let config = self.config.clone();
        let facts = self.git_facts.clone();
        let (global, own) = (facts.global, facts.own);
        for (index, row) in rows.iter().enumerate() {
            if matches!(row, Row::Hidden | Row::BulkRename | Row::Git) {
                let heading = match row {
                    Row::Hidden => "View",
                    Row::BulkRename => "Renaming",
                    _ => "Version control",
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
                Row::View => ("Show as", None),
                Row::Compact => ("Compact rows", None),
                Row::Reversed => ("Reverse the order", None),
                Row::Sort => ("Sort by", None),
                Row::BulkRename => ("Rename several at once (F2)", None),
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
                Row::GitHub => (
                    "GitHub",
                    Some(match self.github_account() {
                        Some(login) => login.to_owned(),
                        None => "Sign in".to_owned(),
                    }),
                ),
                Row::GitHubGit => ("git pushes and pulls with it", None),
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
                Row::BulkRename => Some(config.bulk_rename),
                Row::Git => Some(self.git_on()),
                Row::GitHubGit => Some(facts.github_git),
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
            } else if matches!(row, Row::Sort | Row::View) {
                // A row of pills, the chosen one filled.
                let pills: Vec<(&str, bool, Hit)> = if *row == Row::Sort {
                    SORTS
                        .iter()
                        .map(|(by, name)| (*name, config.sort == *by, Hit::SortPill(*by)))
                        .collect()
                } else {
                    VIEWS
                        .iter()
                        .map(|(view, name)| (*name, config.view == *view, Hit::ViewPill(*view)))
                        .collect()
                };
                let mut x = control.right();
                for (name, chosen, hit) in pills.into_iter().rev() {
                    let ink = if chosen { look.base } else { colors.subtle };
                    let text = Text::new(name, small, ink);
                    let width = canvas.measure(&text).w + 2.0 * space::MD;
                    let pill = Rect::new(x - width, y + 8.0, width, line - 20.0);
                    let fill = if chosen {
                        accent
                    } else if self.hover.as_ref() == Some(&hit) {
                        colors.highlight_med
                    } else {
                        colors.overlay
                    };
                    canvas.fill(pill, radius::pill(pill.h), fill);
                    canvas.text_in(&text, pill, Align::Center);
                    self.hits.push((pill, hit));
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
