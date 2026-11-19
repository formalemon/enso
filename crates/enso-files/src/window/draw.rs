//! Drawing the window, and recording where everything that answers the
//! pointer went.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use enso_core::{
    Rgba,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_fs::format;
use enso_ui::{
    Align, Canvas, Paint, Rect, Text,
    widget::{Button, Form, Interaction, Kind, Part, controls, field},
};

use super::overlay::{Overlay, Reach};
use super::{Files, Hit, preview, tabs};
use crate::pane::Source;
use crate::{pane::Load, pick::Choice, picker::Under};

pub(super) const TITLE_HEIGHT: f32 = 52.0;
const HEADER_HEIGHT: f32 = 30.0;
const STATUS_HEIGHT: f32 = 32.0;
const BAR_HEIGHT: f32 = 64.0;
/// Room for the size and date columns.
const SIZE_WIDTH: f32 = 96.0;
const DATE_WIDTH: f32 = 128.0;
/// The window needs this much width before the sidebar is shown.
const SIDEBAR_FROM: f32 = 640.0;

impl Files {
    pub(super) fn draw_window(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let bounds = canvas.bounds();
        canvas.clear(self.ctx.theme.colors.base);
        self.hits.clear();
        let title = Rect::new(0.0, 0.0, bounds.w, TITLE_HEIGHT);
        let status = Rect::new(0.0, bounds.h - STATUS_HEIGHT, bounds.w, STATUS_HEIGHT);
        let bar = self
            .picker
            .is_some()
            .then(|| Rect::new(0.0, status.y - BAR_HEIGHT, bounds.w, BAR_HEIGHT));
        let body_bottom = bar.map_or(status.y, |bar| bar.y);
        let body = Rect::new(0.0, title.bottom(), bounds.w, body_bottom - title.bottom());

        let mut animating = false;
        self.draw_title(canvas, title);
        let list = if self.sidebar_open && bounds.w >= SIDEBAR_FROM {
            let width = self.sidebar.at.min(bounds.w * 0.4);
            self.draw_sidebar(canvas, Rect::new(body.x, body.y, width, body.h));
            Rect::new(width, body.y, body.w - width, body.h)
        } else {
            body
        };
        // More than one tab: their strip along the top.
        let list = if self.tabs.len() > 1 {
            let strip = Rect::new(list.x, list.y, list.w, tabs::STRIP);
            self.draw_tabs(canvas, strip);
            Rect::new(list.x, strip.bottom(), list.w, list.h - tabs::STRIP)
        } else {
            list
        };
        animating |= self.draw_split(canvas, list, now);
        self.draw_drop(canvas);
        if let Some(bar) = bar {
            self.draw_bar(canvas, bar);
        }
        self.draw_status(canvas, status);
        self.draw_toast(canvas, bar.map_or(status.y, |bar| bar.y));
        if self.overlay.is_some() {
            // Only what's over the window answers the pointer: what's under
            // it is out of reach.
            self.hits.clear();
            animating |= self.draw_overlay(canvas, bounds, now);
        }
        Paint::when(animating)
    }

    /// What's over the window. Returns whether anything in it is moving.
    fn draw_overlay(&mut self, canvas: &mut Canvas<'_>, bounds: Rect, now: Instant) -> bool {
        let look = self.look();
        if self.overlay.settings().is_some() {
            return self.draw_settings(canvas, bounds, now);
        }
        match &mut self.overlay {
            None => {}
            Some(Overlay::Menu((menu, _))) => menu.draw(canvas, bounds, &look.menu()),
            Some(Overlay::Help) => self.draw_help(canvas, bounds),
            Some(Overlay::Peek) => self.draw_peek(canvas, bounds),
            Some(Overlay::Palette(_)) => self.draw_palette(canvas, bounds),
            Some(Overlay::Renamer(_)) => self.draw_renamer(canvas, bounds),
            Some(Overlay::Packing(_)) => self.draw_packing(canvas, bounds),
            Some(Overlay::Properties(_)) => self.draw_properties(canvas, bounds),
            // Drawn above.
            Some(Overlay::Settings(_)) => {}
            Some(Overlay::Vcs(_)) => self.draw_vcs(canvas, bounds),
            Some(Overlay::Asked(_) | Overlay::ConfirmDelete(_)) => {
                self.draw_questions(canvas, bounds);
            }
            Some(Overlay::Replace(_)) => self.draw_confirm(canvas, bounds),
        }
        false
    }

    /// A folder's part of the window: its list (or grid, or columns), and
    /// the preview pane beside it when that's open and this is the side in
    /// `front`. Returns whether anything is moving.
    pub(super) fn draw_body(
        &mut self,
        canvas: &mut Canvas<'_>,
        list: Rect,
        now: Instant,
        front: bool,
    ) -> bool {
        // The preview pane, where there's room for it beside the list (the
        // columns have their own).
        let columns = self.config.view == crate::config::View::Columns;
        let list = if front && self.previewing && !columns && list.w > preview::PANE * 2.0 {
            let pane = Rect::new(list.right() - preview::PANE, list.y, preview::PANE, list.h);
            self.draw_preview(canvas, pane);
            Rect::new(list.x, list.y, list.w - preview::PANE, list.h)
        } else {
            list
        };
        // A folder just arrived slides in the way we went, and fades up.
        let arrival = self.arrival.get(now);
        let shifted = list.translate((1.0 - arrival) * 18.0 * self.sliding, 0.0);
        let columns_view = columns && list.w > 640.0;
        let mut animating = false;
        self.behind = !front;
        canvas.with_opacity(0.35 + 0.65 * arrival, |canvas| {
            animating = if columns_view {
                self.draw_columns(canvas, shifted, now)
            } else {
                self.draw_list(canvas, shifted, now)
            };
        });
        self.behind = false;
        self.draw_band(canvas, now);
        animating || self.arrival.is_animating(now)
    }

    fn draw_title(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        self.hits.push((area, Hit::Title));

        // Back and forward.
        let button = 32.0;
        let mut x = area.x + space::MD;
        for (icon, hit, enabled) in [
            (icon::CHEVRON_LEFT, Hit::Back, self.pane.can_go_back()),
            (
                icon::CHEVRON_RIGHT,
                Hit::Forward,
                self.pane.can_go_forward(),
            ),
        ] {
            let rect = Rect::new(x, area.y + (area.h - button) / 2.0, button, button);
            if enabled && self.hover.as_ref() == Some(&hit) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let color = if enabled {
                colors.text
            } else {
                colors.muted.alpha(opacity::DISABLED)
            };
            canvas.icon(icon, 18.0, color, rect, Align::Center);
            if enabled {
                self.hits.push((rect, hit));
            }
            x += button + space::XS;
        }
        x += space::SM;

        // The path, as crumbs: home is "~", and what's above the last few
        // folders is folded away when there isn't room.
        // A gathered list says what it is: Recent, or the folder searched
        // and what for.
        let mut crumbs = match self.pane.source() {
            Source::Recent => vec![("Recent".to_owned(), PathBuf::from("/"))],
            _ => crumbs(self.pane.location(), &self.home),
        };
        // Inside an archive: on through its folders.
        if let Source::Archive { inner } = self.pane.source() {
            let mut at = self.pane.location().to_owned();
            for part in inner.split('/').filter(|part| !part.is_empty()) {
                at = at.join(part);
                crumbs.push((part.to_owned(), at.clone()));
            }
        }
        if let Source::Search { query, content } = self.pane.source() {
            let label = if *content {
                format!("“{query}” in files")
            } else {
                format!("“{query}”")
            };
            crumbs.push((label, self.pane.location().to_owned()));
        }
        let size = self.font(type_scale::BODY);
        let separator = Text::new("›", size, colors.muted);
        let gap = canvas.measure(&separator).w + space::SM * 2.0;
        let right = self.draw_filter(canvas, area) - space::LG;
        if self.location.is_some() {
            self.draw_location(canvas, Rect::new(x, area.y, (right - x).max(0.0), area.h));
            return;
        }
        let widths: Vec<f32> = crumbs
            .iter()
            .enumerate()
            .map(|(i, (label, _))| {
                let text = Text::new(label, size, colors.text);
                let text = if i + 1 == crumbs.len() {
                    text.medium()
                } else {
                    text
                };
                // A pixel to spare, so rounding never turns a short name into "…".
                canvas.measure(&text).w.ceil() + 1.0 + space::SM * 2.0
            })
            .collect();
        // Fold from the left until the rest fits.
        let mut first = 0;
        while first + 1 < crumbs.len()
            && widths[first..].iter().sum::<f32>() + gap * (crumbs.len() - first) as f32 > right - x
        {
            first += 1;
        }
        if first > 0 {
            let ellipsis = Text::new("…", size, colors.muted);
            x += canvas
                .text_in(&ellipsis, Rect::new(x, area.y, 24.0, area.h), Align::Start)
                .w;
            x += space::SM;
        }
        let pill = 30.0;
        for (i, (label, path)) in crumbs.iter().enumerate().skip(first) {
            let last = i + 1 == crumbs.len();
            if i > first {
                canvas.text_in(&separator, Rect::new(x, area.y, gap, area.h), Align::Center);
                x += gap;
            }
            // The last crumb may have to stop short of the edge.
            let width = widths[i].min((right - x).max(0.0));
            let rect = Rect::new(x, area.y + (area.h - pill) / 2.0, width, pill);
            let hit = Hit::Crumb(path.clone());
            if last {
                canvas.fill(rect, radius::pill(pill), accent.alpha(opacity::TINT));
            } else if self.hover.as_ref() == Some(&hit) {
                canvas.fill(rect, radius::pill(pill), colors.highlight_low);
            }
            let text = Text::new(label, size, if last { colors.text } else { colors.subtle });
            let text = if last { text.medium() } else { text };
            canvas.text_in(&text, rect.inset_xy(space::SM, 0.0), Align::Center);
            if !last {
                self.hits.push((rect, hit));
            }
            x += widths[i];
        }
    }

    /// The location as text, in place of the crumbs, while it's typed.
    fn draw_location(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let height = 32.0;
        let rect = Rect::new(area.x, area.y + (area.h - height) / 2.0, area.w, height);
        canvas.fill(rect, radius::pill(height), colors.surface);
        let ring = if self.location_missing {
            colors.love
        } else {
            accent
        };
        canvas.stroke(rect, radius::pill(height), stroke::HAIRLINE, ring);
        let text_rect = rect.inset_xy(space::LG, 0.0);
        let style = field::Style {
            size: self.font(type_scale::BODY),
            color: colors.text,
            placeholder: "Type a folder, ~ for home; Tab completes",
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        if let Some(field) = &mut self.location {
            field.draw(canvas, text_rect, &style, true);
        }
        self.hits.push((rect, Hit::Location(text_rect.x)));
    }

    /// The keys, on a card over everything (`?` or F1).
    /// The keys worth knowing by heart. The command palette (Ctrl+P) has
    /// every command with its keys; this is the short list.
    fn draw_help(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let look = self.look();
        let small = self.font(type_scale::SMALL);
        let mut keys: Vec<(&str, &str)> = vec![
            ("↑ ↓  j k", "move"),
            ("Enter  →  l", "open"),
            ("←  h  Backspace", "up a folder"),
            ("Alt+← →", "back, forward"),
            ("Space  ·  Ctrl+A", "choose · all"),
            ("/  ·  Ctrl+F", "filter · search"),
            ("Ctrl+L", "type a location"),
            ("i  ·  Shift+Space", "preview · quick look"),
            ("Ctrl+1 · 2 · 3", "list · grid · columns"),
            ("Ctrl+T  ·  F3", "new tab · split view"),
            ("F2  ·  Delete", "rename · to the trash"),
            ("Ctrl+Z", "undo"),
            ("Menu  ·  right-click", "more"),
        ];
        if let Some(picker) = &self.picker {
            keys.push(("Ctrl+Enter", "finish"));
            if picker.name.is_some() {
                keys.push(("Tab", "list, or the name"));
            }
            keys.push(("Esc", "cancel"));
        } else {
            keys.push(("Ctrl+Q", "close"));
        }
        let rows = keys.len().div_ceil(2);
        let line = (small * 1.9).round();
        let placed = Form::new("Keys")
            .width(640.0)
            .about("The ones worth knowing by heart. Everything else is in the command palette — Ctrl+P, or : — with its keys.")
            .part(Part::Space(rows as f32 * line))
            .draw(canvas, bounds, &look, 1.0);
        let grid = placed.parts[0];
        let column = (grid.w - space::XL) / 2.0;
        let key_width = column * 0.48;
        for (index, (key, what)) in keys.iter().enumerate() {
            let (col, row) = (index / rows, index % rows);
            let x = grid.x + col as f32 * (column + space::XL);
            let y = grid.y + row as f32 * line;
            let rect = Rect::new(x, y, key_width, line);
            canvas.text_in(
                &Text::new(key, small, look.accent).medium(),
                rect,
                Align::Start,
            );
            let rest = Rect::new(x + key_width, y, column - key_width, line);
            canvas.text_in(&Text::new(what, small, look.subtle), rest, Align::Start);
        }
    }

    /// The filter at the right of the title row: a field while it's open
    /// or a filter is on, a quiet magnifier otherwise. Returns where it
    /// starts, for the path to stop short of.
    fn draw_filter(&mut self, canvas: &mut Canvas<'_>, area: Rect) -> f32 {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.font(type_scale::BODY);
        let height = 32.0;
        let open = self.filter.is_some() || !self.pane.filter().is_empty();
        let width = if self.seek.is_some() {
            (area.w * 0.42).clamp(200.0, 400.0)
        } else if open {
            (area.w * 0.32).clamp(160.0, 280.0)
        } else {
            height
        };
        let rect = Rect::new(
            area.right() - space::MD - width,
            area.y + (area.h - height) / 2.0,
            width,
            height,
        );
        let hovered = matches!(self.hover, Some(Hit::Filter(_)));
        if open {
            canvas.fill(rect, radius::pill(height), colors.surface);
            let ring = if self.filter.is_some() {
                accent
            } else {
                colors.highlight_med
            };
            canvas.stroke(rect, radius::pill(height), stroke::HAIRLINE, ring);
        } else if hovered {
            canvas.fill(rect, radius::pill(height), colors.highlight_low);
        }
        let icon_box = Rect::new(rect.x + space::SM, rect.y, 18.0, rect.h);
        if open {
            canvas.icon(icon::SEARCH, 16.0, colors.subtle, icon_box, Align::Start);
        } else {
            canvas.icon(icon::SEARCH, 16.0, colors.subtle, rect, Align::Center);
        }
        let text_rect = Rect::new(
            icon_box.right() + space::SM,
            rect.y,
            (rect.right() - space::MD - icon_box.right() - space::SM).max(0.0),
            rect.h,
        );
        if open {
            let hint = self.search_hint();
            let style = field::Style {
                size,
                color: colors.text,
                placeholder: &hint,
                placeholder_color: colors.muted,
                caret: accent,
                selection: accent.alpha(opacity::TINT * 2.0),
            };
            let focused = self.filter.is_some();
            match &mut self.filter {
                Some(field) => field.draw(canvas, text_rect, &style, focused),
                None => {
                    canvas.text_in(
                        &Text::new(self.pane.filter(), size, colors.subtle),
                        text_rect,
                        Align::Start,
                    );
                }
            }
        }
        self.hits.push((rect, Hit::Filter(text_rect.x)));
        rect.x
    }

    /// The list, with its headings. Returns whether anything is moving.
    pub(super) fn draw_list(&mut self, canvas: &mut Canvas<'_>, area: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let row_height = self.row_height();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        let grid = self.config.view == crate::config::View::Grid;

        // Column headings (a grid has none).
        let header = Rect::new(
            area.x,
            area.y,
            area.w,
            if grid { 0.0 } else { HEADER_HEIGHT },
        );
        let columns = Columns::new(header.inset_xy(space::LG, 0.0));
        let heading = |label| Text::new(label, small, colors.muted).medium();
        if !grid {
            canvas.text_in(&heading("Name"), columns.name, Align::Start);
        }
        if columns.size.w > 0.0 && !grid {
            canvas.text_in(&heading("Size"), columns.size, Align::End);
            canvas.text_in(&heading("Modified"), columns.date, Align::Start);
        }
        if !grid {
            canvas.fill(
                Rect::new(
                    area.x + space::LG,
                    if grid {
                        area.y - 10.0
                    } else {
                        header.bottom() - 1.0
                    },
                    area.w - 2.0 * space::LG,
                    stroke::HAIRLINE,
                ),
                0.0,
                colors.highlight_med,
            );
        }

        let list = Rect::new(area.x, header.bottom(), area.w, area.h - header.h);
        self.list = list;
        self.scroll
            .set_extent(self.pane.len() as f32 * row_height, list.h);
        self.hits.push((list, Hit::Empty));

        match self.pane.load().clone() {
            Load::Loading if self.pane.is_empty() => {
                // A search can take a moment; a folder rarely does.
                if !self.pane.source().is_folder() {
                    let text = Text::new("Looking…", body, colors.muted);
                    canvas.text_in(&text, list, Align::Center);
                }
                return false;
            }
            Load::Failed(why) => {
                let text = Text::new(&why, body, colors.subtle)
                    .max_width(list.w * 0.6)
                    .max_lines(3);
                canvas.text_in(&text, list.centered(list.w * 0.6, 80.0), Align::Center);
                return false;
            }
            _ if self.pane.is_empty() => {
                let message = if let Source::Search { query, .. } = self.pane.source() {
                    format!("Nothing below here matches “{query}” — Esc goes back")
                } else if *self.pane.source() == Source::Recent {
                    "Nothing used lately".to_owned()
                } else if !self.pane.filter().is_empty() {
                    format!(
                        "Nothing here matches “{}” — Esc shows everything",
                        self.pane.filter()
                    )
                } else {
                    match self.pane.hidden_count() {
                        0 => "Nothing here yet".to_owned(),
                        n => format!(
                            "Nothing here but {} — Ctrl+H shows {}",
                            format::count(n, "hidden file", "hidden files"),
                            if n == 1 { "it" } else { "them" }
                        ),
                    }
                };
                canvas.text_in(
                    &Text::new(&message, body, colors.muted),
                    list,
                    Align::Center,
                );
                return false;
            }
            _ => {}
        }
        if grid {
            return self.draw_grid(canvas, list, now);
        }

        let scroll = self.scroll.get(now);
        let first = (scroll / row_height).floor().max(0.0) as usize;
        let last = (((scroll + list.h) / row_height).ceil() as usize).min(self.pane.len());
        let zone = (self.machine.now)();
        let cursor = self.pane.cursor();
        let highlight = self.highlight.get(now);
        let chosen = colors.iris;

        let renaming = self.renaming.as_ref().map(|renaming| renaming.path.clone());
        let mut rename_at = None;
        canvas.with_clip(list, |canvas| {
            // The cursor's highlight glides on its own, under the rows.
            if cursor.is_some() {
                let y = list.y + highlight - scroll;
                let rect = Rect::new(
                    list.x + space::SM,
                    y + 1.0,
                    list.w - space::SM * 2.0,
                    row_height - 2.0,
                );
                if self.behind {
                    // The split's other side: where its cursor is, quietly.
                    canvas.stroke(
                        rect,
                        radius::CONTROL,
                        stroke::HAIRLINE,
                        colors.highlight_high,
                    );
                } else {
                    canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
                    canvas.pill(
                        Rect::new(
                            rect.x + space::XS,
                            rect.y + rect.h * 0.25,
                            3.0,
                            rect.h * 0.5,
                        ),
                        accent,
                    );
                }
            }
            for row in first..last {
                let y = list.y + row as f32 * row_height - scroll;
                let rect = Rect::new(list.x, y, list.w, row_height);
                let band = Rect::new(
                    list.x + space::SM,
                    y + 1.0,
                    list.w - space::SM * 2.0,
                    row_height - 2.0,
                );
                let columns = Columns::new(rect.inset_xy(space::LG, 0.0));
                let path = self.pane.path(row);
                let selected = self.selection.contains(&path);
                if selected {
                    canvas.fill(band, radius::CONTROL, chosen.alpha(opacity::TINT));
                } else if self.hover == Some(Hit::Row(row)) && cursor != Some(row) {
                    canvas.fill(band, radius::CONTROL, colors.highlight_low);
                }
                let entry = *self.pane.entry(row);
                let dir = entry.kind.is_dir();
                let faded = |color: Rgba| {
                    if self.pane.is_hidden(row) {
                        color.alpha(0.7)
                    } else {
                        color
                    }
                };
                let executable = entry.meta.is_some_and(|meta| meta.mode & 0o111 != 0);
                let full = self.pane.name(row);
                // In a gathered list a name is a path from the root: the
                // file's own name leads, its folder follows, dimmer.
                let (folder, own) = match full.rsplit_once('/') {
                    Some((folder, own)) => (Some(folder.to_owned()), own.to_owned()),
                    None => (None, full.into_owned()),
                };
                let kind = crate::kind::Kind::of(&own, dir, executable);
                let (icon, icon_color) = if selected {
                    (icon::CHECK, chosen)
                } else if dir {
                    (icon::FOLDER, accent)
                } else {
                    (icon::FILE, kind.color(&colors, accent))
                };
                canvas.icon(icon, 18.0, faded(icon_color), columns.icon, Align::Center);
                if renaming.as_ref() == Some(&path) {
                    rename_at = Some(columns.name);
                } else {
                    let size = canvas.text_in(
                        &Text::new(&own, body, faded(colors.text)).max_width(columns.name.w),
                        columns.name,
                        Align::Start,
                    );
                    // After the name: a search's matching line, or the
                    // folder it's in.
                    let after = Rect::new(
                        columns.name.x + size.w + space::MD,
                        columns.name.y,
                        (columns.name.w - size.w - space::MD).max(0.0),
                        columns.name.h,
                    );
                    if after.w > 24.0 {
                        if let Some(note) = self.pane.note(row) {
                            let text = Text::new(note, small, colors.subtle)
                                .font(enso_ui::Font::Mono)
                                .max_width(after.w);
                            canvas.text_in(&text, after, Align::Start);
                        } else if let Some(folder) = &folder {
                            let shown = match self.pane.source() {
                                crate::pane::Source::Recent => {
                                    super::tabs::short(&Path::new("/").join(folder), &self.home)
                                }
                                _ => folder.clone(),
                            };
                            let text = Text::new(&shown, small, colors.muted).max_width(after.w);
                            canvas.text_in(&text, after, Align::Start);
                        }
                    }
                }
                if let Some(meta) = entry.meta
                    && columns.size.w > 0.0
                {
                    if !dir {
                        let size = format::size(meta.size);
                        canvas.text_in(
                            &Text::new(&size, small, colors.subtle),
                            columns.size,
                            Align::End,
                        );
                    }
                    let when = format::when(meta.modified, &zone);
                    canvas.text_in(
                        &Text::new(&when, small, colors.subtle),
                        columns.date,
                        Align::Start,
                    );
                }
            }
        });
        for row in first..last {
            let y = list.y + row as f32 * row_height - scroll;
            let top = y.max(list.y);
            let rect = Rect::new(
                list.x,
                top,
                list.w,
                (y + row_height).min(list.bottom()) - top,
            );
            if rect.h > 0.0 {
                self.hits.push((rect, Hit::Row(row)));
            }
        }
        if let Some(rect) = rename_at.filter(|rect| list.contains(rect.x, rect.center().1)) {
            self.draw_rename(canvas, rect);
        }
        let hovered_bar = self.hover == Some(Hit::ScrollBar);
        self.scroll
            .draw_bar(canvas, list, colors.subtle, hovered_bar, now);
        if self.scroll.max() > 0.0 {
            let strip = Rect::new(list.right() - 14.0, list.y, 14.0, list.h);
            self.hits.push((strip, Hit::ScrollBar));
        }
        self.scroll.is_animating(now) || self.highlight.is_animating(now)
    }

    /// The picker's bar: what's chosen (or the name to save as) on the
    /// left; the file types, the application's questions, and the buttons
    /// on the right.
    fn draw_bar(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let Some(picker) = &self.picker else { return };
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let small = self.font(type_scale::SMALL);
        canvas.fill(area, 0.0, colors.surface);
        canvas.fill(
            Rect::new(area.x, area.y, area.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let inner = area.inset_xy(space::LG, 0.0);
        let height = controls::HEIGHT;
        let y = inner.y + (inner.h - height) / 2.0;
        let mut right = inner.right();
        let mut hits = Vec::new();

        // Buttons, right to left.
        let under = match self.under() {
            Under::Dir(_) => Under::Nothing,
            under => under,
        };
        let ready = picker.ready(self.pane.location(), &self.selection, &under);
        let count = self.selection.len().max(1);
        let accept_label = picker.request.accept(count);
        let accept = Button::new(&accept_label, Kind::Primary);
        let width = accept.width(canvas, &look);
        let rect = Rect::new(right - width, y, width, height);
        accept.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Accept),
                disabled: !ready,
                ..Interaction::default()
            },
        );
        if ready {
            hits.push((rect, Hit::Accept));
        }
        right = rect.x - space::SM;
        let cancel = Button::new("Cancel", Kind::Secondary);
        let width = cancel.width(canvas, &look);
        let rect = Rect::new(right - width, y, width, height);
        cancel.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Cancel),
                ..Interaction::default()
            },
        );
        hits.push((rect, Hit::Cancel));
        right = rect.x - space::LG;

        // The application's questions.
        for (index, choice) in picker.choices.iter().enumerate().rev() {
            let hovered = self.hover == Some(Hit::Choice(index));
            let rect = match choice {
                Choice::Toggle { label, on, .. } => {
                    let text = Text::new(label, small, colors.subtle);
                    let width = canvas.measure(&text).w + controls::TOGGLE.0 + space::SM;
                    let rect = Rect::new(right - width, y, width, height);
                    let toggle = Rect::new(
                        rect.x,
                        y + (height - controls::TOGGLE.1) / 2.0,
                        controls::TOGGLE.0,
                        controls::TOGGLE.1,
                    );
                    enso_ui::widget::Toggle::new(*on).draw(
                        canvas,
                        toggle,
                        &look,
                        Interaction {
                            hovered,
                            ..Interaction::default()
                        },
                        Instant::now(),
                    );
                    canvas.text_in(
                        &text,
                        Rect::new(toggle.right() + space::SM, y, width, height),
                        Align::Start,
                    );
                    rect
                }
                Choice::Options {
                    label,
                    options,
                    selected,
                    ..
                } => {
                    let value = options
                        .iter()
                        .find(|(id, _)| id == selected)
                        .map_or(selected.as_str(), |(_, label)| label.as_str());
                    let text = format!("{label}: {value}");
                    dropdown(canvas, &look, &text, right, y, hovered)
                }
            };
            hits.push((rect, Hit::Choice(index)));
            right = rect.x - space::MD;
        }

        // The file types.
        if let Some(filter) = picker
            .filter
            .and_then(|index| picker.request.filters.get(index))
        {
            let hovered = self.hover == Some(Hit::FilterMenu);
            let rect = dropdown(canvas, &look, &filter.name, right, y, hovered);
            if picker.request.filters.len() > 1 {
                hits.push((rect, Hit::FilterMenu));
            }
            right = rect.x - space::LG;
        }

        // Left: the name to save as, or what's chosen.
        let left = Rect::new(inner.x, y, (right - inner.x).max(0.0), height);
        let typing = self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.typing_name);
        if let Some(name) = self.picker.as_mut().and_then(|picker| picker.name.as_mut()) {
            let field_rect = Rect::new(left.x, left.y, left.w.min(420.0), left.h);
            canvas.fill(field_rect, radius::pill(height), colors.base);
            canvas.stroke(
                field_rect,
                radius::pill(height),
                stroke::HAIRLINE,
                if typing {
                    look.accent
                } else {
                    colors.highlight_med
                },
            );
            let text_rect = field_rect.inset_xy(space::LG, 0.0);
            name.draw(
                canvas,
                text_rect,
                &field::Style {
                    size: look.size,
                    color: colors.text,
                    placeholder: "Name",
                    placeholder_color: colors.muted,
                    caret: look.accent,
                    selection: look.accent.alpha(opacity::TINT * 2.0),
                },
                typing,
            );
            hits.push((field_rect, Hit::Name(text_rect.x)));
        } else {
            self.draw_chips(canvas, left, &mut hits);
        }
        self.hits.extend(hits);
    }

    /// What's chosen, as removable chips; what doesn't fit is counted.
    fn draw_chips(&mut self, canvas: &mut Canvas<'_>, area: Rect, hits: &mut Vec<(Rect, Hit)>) {
        let colors = self.ctx.theme.colors.clone();
        let small = self.font(type_scale::SMALL);
        let chip = 28.0;
        let y = area.y + (area.h - chip) / 2.0;
        if self.selection.is_empty() {
            let hint = match &self.picker {
                Some(picker) if picker.request.directory() => {
                    "Enter goes in · Ctrl+Enter chooses this folder"
                }
                Some(picker) if picker.request.multiple() => {
                    "Space or Ctrl+click to choose · from any folder"
                }
                _ => "Double-click or Enter to choose",
            };
            canvas.text_in(&Text::new(hint, small, colors.muted), area, Align::Start);
            return;
        }
        let mut x = area.x;
        let paths: Vec<PathBuf> = self.selection.paths().to_vec();
        for (shown, path) in paths.iter().enumerate() {
            let name = path
                .file_name()
                .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
            let text = Text::new(&name, small, colors.text).max_width(160.0);
            let width = canvas.measure(&text).w + space::MD + 16.0 + space::SM;
            let more = paths.len() - shown;
            let room_for_more = if more > 1 { 64.0 } else { 0.0 };
            if x + width + room_for_more > area.right() {
                let label = format!("+{more} more");
                canvas.text_in(
                    &Text::new(&label, small, colors.subtle),
                    Rect::new(x, area.y, area.right() - x, area.h),
                    Align::Start,
                );
                break;
            }
            let rect = Rect::new(x, y, width, chip);
            let hovered = self.hover == Some(Hit::Chip(path.clone()));
            canvas.fill(
                rect,
                radius::pill(chip),
                if hovered {
                    colors.highlight_med
                } else {
                    colors.overlay
                },
            );
            canvas.text_in(
                &text,
                Rect::new(rect.x + space::MD, rect.y, width, chip),
                Align::Start,
            );
            let close = Rect::new(
                rect.right() - 16.0 - space::SM,
                rect.y,
                16.0 + space::SM,
                chip,
            );
            let close_hovered = self.hover == Some(Hit::Unchoose(path.clone()));
            canvas.icon(
                icon::CLOSE,
                12.0,
                if close_hovered {
                    colors.love
                } else {
                    colors.subtle
                },
                close,
                Align::Center,
            );
            hits.push((rect, Hit::Chip(path.clone())));
            hits.push((close, Hit::Unchoose(path.clone())));
            x = rect.right() + space::SM;
        }
    }

    fn draw_status(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let small = self.font(type_scale::SMALL);
        canvas.fill(
            Rect::new(area.x, area.y, area.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let inner = area.inset_xy(space::LG, 0.0);
        let mut left = match self.pane.load() {
            Load::Loading => "Loading…".to_owned(),
            Load::Failed(_) => String::new(),
            Load::Ready if !self.pane.filter().is_empty() => format!(
                "{} of {} match",
                self.pane.len(),
                format::count(self.pane.unfiltered_len(), "item", "items")
            ),
            Load::Ready => format::count(self.pane.len(), "item", "items"),
        };
        if self.pane.hidden_count() > 0 {
            left += &format!(" · {} hidden", self.pane.hidden_count());
        }
        if !self.selection.is_empty() {
            left += &format!(" · {} selected", self.selection.len());
            let folders = self.selection.folders();
            if folders > 1 {
                left += &format!(" in {folders} folders");
            }
        }
        // In a gathered list: where the one under the cursor is.
        if !self.pane.source().is_sorted()
            && let Some(row) = self.pane.cursor()
            && let Some(folder) = self.pane.path(row).parent()
        {
            left += &format!(" · in {}", super::tabs::short(folder, &self.home));
        }
        canvas.text_in(
            &Text::new(&left, small, colors.subtle).max_width(inner.w * 0.7),
            inner,
            Align::Start,
        );
        let right = if self.repo.is_some() {
            self.draw_branch(canvas, inner)
        } else {
            inner.right()
        };
        let inner = Rect::new(inner.x, inner.y, right - inner.x, inner.h);
        if self.in_trash() && self.picker.is_none() && !self.pane.is_empty() {
            let small = self.font(type_scale::SMALL);
            let label = Text::new("Empty the trash", small, colors.text);
            let width = canvas.measure(&label).w + 2.0 * space::MD;
            let rect = Rect::new(
                inner.right() - width,
                inner.y + (inner.h - 22.0) / 2.0,
                width,
                22.0,
            );
            let hovered = self.hover == Some(Hit::EmptyTrash);
            canvas.fill(
                rect,
                radius::pill(22.0),
                if hovered {
                    colors.love.alpha(0.3)
                } else {
                    colors.overlay
                },
            );
            canvas.text_in(&label, rect, Align::Center);
            self.hits.push((rect, Hit::EmptyTrash));
        } else if self.draw_running(canvas, inner) {
            // The job in hand says more than the free space.
        } else if let Ok(space) = (self.machine.space)(self.pane.location()) {
            let free = format!("{} free", format::size(space.free));
            canvas.text_in(&Text::new(&free, small, colors.subtle), inner, Align::End);
        }
    }

    /// The question about saving over a file, if it's being asked.
    fn draw_confirm(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(path) = self.overlay.replace().cloned() else {
            return;
        };
        let look = self.look();
        let name = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
        let title = format!("Replace “{name}”?");
        let folder = path
            .parent()
            .map(|folder| folder.to_string_lossy().into_owned())
            .unwrap_or_default();
        let detail =
            format!("A file with this name is already in {folder}. Saving replaces what's in it.");
        let hovered = |hit: Hit| Interaction {
            hovered: self.hover == Some(hit),
            ..Interaction::default()
        };
        let placed = Form::new(&title)
            .about(&detail)
            .button("Cancel", Kind::Secondary, hovered(Hit::KeepFile))
            .button(
                "Replace",
                Kind::Danger,
                Interaction {
                    focused: true,
                    ..hovered(Hit::Replace)
                },
            )
            .draw(canvas, bounds, &look, 1.0);
        self.hits.push((placed.buttons[1], Hit::Replace));
        self.hits.push((placed.buttons[0], Hit::KeepFile));
    }
}

/// A dropdown button whose right edge is at `right`: its value and a
/// chevron. Returns where it went.
fn dropdown(
    canvas: &mut Canvas<'_>,
    look: &enso_ui::widget::Look,
    text: &str,
    right: f32,
    y: f32,
    hovered: bool,
) -> Rect {
    let label = Text::new(text, look.size * type_scale::SMALL, look.text).max_width(220.0);
    let width = canvas.measure(&label).w + 2.0 * space::MD + 16.0 + space::XS;
    let rect = Rect::new(right - width, y, width, controls::HEIGHT);
    canvas.fill(
        rect,
        radius::pill(rect.h),
        if hovered {
            look.highlight_med
        } else {
            look.overlay
        },
    );
    canvas.text_in(&label, rect.inset_xy(space::MD, 0.0), Align::Start);
    canvas.icon(
        icon::CHEVRON_DOWN,
        14.0,
        look.subtle,
        Rect::new(rect.right() - space::MD - 16.0, y, 16.0, rect.h),
        Align::Center,
    );
    rect
}

/// Where each column of a row goes. Narrow lists drop the size and date.
struct Columns {
    icon: Rect,
    name: Rect,
    size: Rect,
    date: Rect,
}

impl Columns {
    fn new(row: Rect) -> Self {
        let icon = Rect::new(row.x, row.y, 24.0, row.h);
        let name_x = icon.right() + space::MD;
        if row.w < 420.0 {
            return Self {
                icon,
                name: Rect::new(name_x, row.y, (row.right() - name_x).max(0.0), row.h),
                size: Rect::new(row.right(), row.y, 0.0, row.h),
                date: Rect::new(row.right(), row.y, 0.0, row.h),
            };
        }
        let date = Rect::new(row.right() - DATE_WIDTH, row.y, DATE_WIDTH, row.h);
        let size = Rect::new(date.x - space::XL - SIZE_WIDTH, row.y, SIZE_WIDTH, row.h);
        let name = Rect::new(name_x, row.y, (size.x - space::XL - name_x).max(0.0), row.h);
        Self {
            icon,
            name,
            size,
            date,
        }
    }
}

/// The crumbs of `path`: a label and where it goes, from the root (or from
/// home, shown as `~`) down.
pub(super) fn crumbs(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs = Vec::new();
    let mut at = Some(path);
    while let Some(dir) = at {
        if dir == home {
            crumbs.push(("~".to_owned(), dir.to_owned()));
            break;
        }
        let label = match dir.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => "/".to_owned(),
        };
        crumbs.push((label, dir.to_owned()));
        at = dir.parent();
    }
    crumbs.reverse();
    crumbs
}

/// The window title: the folder's name, and where it is.
pub(super) fn title(path: &Path, home: &Path) -> String {
    let name = if path == home {
        "Home".to_owned()
    } else {
        path.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    format!("{name} — Files")
}
