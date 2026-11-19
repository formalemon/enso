//! The settings panel (`Ctrl+,`): the file manager's own settings, on a
//! card over the window. What's changed here is written to the state
//! directory, over `config.toml` ([`crate::settings`]); each change shows
//! at once.

use enso_core::style::{radius, space, stroke, type_scale};
use enso_fs::git::{self, Scope};
use std::time::Instant;

use enso_core::motion::Transition;
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Nav, Rect, Text,
    widget::{Button, Interaction, Kind, Scroll, Toggle, controls, form},
};

use super::overlay::Reach;
use super::{Files, Hit};
use crate::forge::Site;
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
    /// The account on a forge.
    Account(Site),
    /// git pushes and pulls with it.
    AccountGit(Site),
}

/// The height of a line, and of a section's heading above its first.
const LINE: f32 = 44.0;
const HEADING: f32 = 34.0;

/// The panel's state: which line has the keyboard, and how far the lines
/// are scrolled when the window is too short to show them all.
#[derive(Clone, Debug, Default)]
pub(super) struct Panel {
    focus: usize,
    scroll: Scroll,
    /// Bring the focused line into view at the next frame.
    follow: bool,
    /// Where the lines were last drawn: the wheel scrolls them there.
    view: Rect,
}

impl Panel {
    /// The panel with the keyboard on line `focus`.
    #[cfg(test)]
    pub(super) fn at(focus: usize) -> Self {
        Self {
            focus,
            follow: true,
            ..Self::default()
        }
    }

    fn focus(&mut self, line: usize) {
        self.focus = line;
        self.follow = true;
    }
}

/// Whether `row` starts a section, and under which heading.
fn heading(row: Row) -> Option<&'static str> {
    match row {
        Row::Hidden => Some("View"),
        Row::BulkRename => Some("Renaming"),
        Row::Git => Some("Version control"),
        _ => None,
    }
}

/// Where each line starts, from the top of the first heading, and how
/// tall they are all together.
fn layout(rows: &[Row]) -> (Vec<f32>, f32) {
    let mut y = 0.0;
    let tops = rows
        .iter()
        .map(|&row| {
            if heading(row).is_some() {
                y += HEADING;
            }
            let top = y;
            y += LINE;
            top
        })
        .collect();
    (tops, y)
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
            for site in Site::ALL {
                rows.push(Row::Account(site));
                if self.account(site).is_some() {
                    rows.push(Row::AccountGit(site));
                }
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
            Row::Account(site) => {
                self.overlay = None;
                self.account_row(site, cx);
                return;
            }
            Row::AccountGit(site) => {
                self.toggle_forge_git(site, cx);
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
        let Some(panel) = self.overlay.settings() else {
            return false;
        };
        let rows = self.setting_rows();
        let focus = panel.focus.min(rows.len() - 1);
        let mut to = None;
        if let Some(nav) = key.nav(true) {
            to = Some(match nav {
                Nav::Up => focus.checked_sub(1).unwrap_or(rows.len() - 1),
                Nav::Down => (focus + 1) % rows.len(),
            });
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
                Key::Tab => to = Some((focus + 1) % rows.len()),
                _ => {}
            }
        }
        if let (Some(line), Some(panel)) = (to, self.overlay.settings_mut()) {
            panel.focus(line);
        }
        cx.redraw();
        true
    }

    /// The wheel, or fingers on a touchpad, over the panel's lines: scroll
    /// them. Returns whether the panel is open.
    pub(super) fn settings_wheel(
        &mut self,
        x: f32,
        y: f32,
        dy: f32,
        wheel: bool,
        now: Instant,
    ) -> bool {
        let transition = self.transition(Transition::SELECT);
        let Some(panel) = self.overlay.settings_mut() else {
            return false;
        };
        if panel.view.contains(x, y) {
            panel.scroll.scroll(dy, wheel, LINE, transition, now);
        }
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
                if let Some(panel) = self.overlay.settings_mut() {
                    panel.focus(*index);
                }
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

    /// Draw the panel. Returns whether its lines are still scrolling.
    pub(super) fn draw_settings(
        &mut self,
        canvas: &mut Canvas<'_>,
        bounds: Rect,
        now: Instant,
    ) -> bool {
        let rows = self.setting_rows();
        let transition = self.transition(Transition::SELECT);
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        let line = LINE;
        let (tops, content) = layout(&rows);
        let height = 64.0 + content + controls::HEIGHT + 40.0;
        let inner = controls::dialog(canvas, bounds, 560.0, height, &look, 1.0);
        canvas.text(
            &Text::new("Settings", form::title_size(&look), colors.text).medium(),
            inner.x,
            inner.y,
        );
        // The lines, between the title and the footer: all of them where
        // the window is tall enough, else as many as fit, scrolled. The
        // view reaches into the card's margin, where its scrollbar goes.
        let top = inner.y + 48.0;
        let view = Rect::new(
            inner.x - space::SM - stroke::FOCUS,
            top,
            inner.w + 2.0 * space::SM + space::LG,
            inner.bottom() - controls::HEIGHT - space::MD - top,
        );
        let Some(panel) = self.overlay.settings_mut() else {
            return false;
        };
        let focus = panel.focus.min(rows.len() - 1);
        panel.view = view;
        panel.scroll.set_extent(content, view.h);
        if std::mem::take(&mut panel.follow) {
            // The heading over a section's first line comes into view too.
            let above = if heading(rows[focus]).is_some() {
                HEADING
            } else {
                0.0
            };
            panel
                .scroll
                .reveal(tops[focus] - above, line + above, transition, now);
        }
        let scrolled = panel.scroll.get(now);
        let animating = panel.scroll.is_animating(now);
        let scroll = panel.scroll.clone();
        let config = self.config.clone();
        let facts = self.git_facts.clone();
        let (global, own) = (facts.global, facts.own);
        // Lines first, so a pill on one wins over the line itself.
        let mut lines = Vec::new();
        let mut pill_hits = Vec::new();
        canvas.set_clip(Some(view));
        for (index, row) in rows.iter().enumerate() {
            let y = view.y + tops[index] - scrolled;
            if let Some(heading) = heading(*row) {
                canvas.text(
                    &Text::new(heading, small, colors.muted).medium(),
                    inner.x,
                    y - HEADING + 6.0,
                );
            }
            if y + line < view.y || y > view.bottom() {
                continue;
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
                Row::Account(site) => (
                    site.name(),
                    Some(match self.account(*site) {
                        Some(login) => login.to_owned(),
                        None => "Sign in".to_owned(),
                    }),
                ),
                Row::AccountGit(_) => ("git pushes and pulls with it", None),
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
                Row::AccountGit(site) => Some(facts.forge_git[usize::from(*site == Site::GitLab)]),
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
                    pill_hits.push((pill, hit));
                    x = pill.x - space::XS;
                }
            } else if let Some(note) = &note {
                canvas.text_in(
                    &Text::new(&format!("{note}  ›"), small, colors.subtle).max_width(control.w),
                    control,
                    Align::End,
                );
            }
            lines.push((rect, Hit::Setting(index)));
        }
        canvas.set_clip(None);
        // A line half out of view answers only where it shows.
        let shown = |rect: Rect| {
            let y = rect.y.max(view.y);
            let bottom = rect.bottom().min(view.bottom());
            Rect::new(rect.x, y, rect.w, (bottom - y).max(0.0))
        };
        let pills: Vec<(Rect, Hit)> = pill_hits
            .into_iter()
            .map(|(rect, hit)| (shown(rect), hit))
            .collect();
        self.hits.splice(
            0..0,
            lines.into_iter().map(|(rect, hit)| (shown(rect), hit)),
        );
        self.hits.extend(pills);
        scroll.draw_bar(canvas, view, colors.subtle, false, now);
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
        animating
    }
}
