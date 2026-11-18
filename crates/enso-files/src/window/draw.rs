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
use enso_fs::{format, places};
use enso_ui::{
    Align, Canvas, Paint, Rect, Text,
    widget::{Button, Interaction, Kind, controls, field},
};

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
        if let Some(bar) = bar {
            self.draw_bar(canvas, bar);
        }
        self.draw_status(canvas, status);
        self.draw_toast(canvas, bar.map_or(status.y, |bar| bar.y));
        let look = self.look();
        if let Some((menu, _)) = &mut self.menu {
            menu.draw(canvas, bounds, &look.menu());
        }
        self.draw_confirm(canvas, bounds);
        self.draw_questions(canvas, bounds);
        self.draw_vcs(canvas, bounds);
        self.draw_settings(canvas, bounds, now);
        self.draw_palette(canvas, bounds);
        if self.peek {
            self.draw_peek(canvas, bounds);
        }
        if self.help {
            self.draw_help(canvas, bounds);
        }
        Paint::when(animating)
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
    fn draw_help(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let small = self.font(type_scale::SMALL);
        let mut keys: Vec<(&str, &str)> = vec![
            ("↑ ↓  j k", "move"),
            ("Enter  →  l", "open"),
            ("←  h  Backspace", "up a folder"),
            ("Alt+← →  H L", "back, forward"),
            ("Space", "choose, and move on"),
            ("Shift+↑ ↓", "choose a run"),
            ("Ctrl+A  ·  *", "choose all · turn round"),
            ("Esc", "clear filter, then choice"),
            ("/", "filter as you type"),
            ("Ctrl+F  ·  //", "search below here"),
            ("Ctrl+Shift+F", "search inside files"),
            ("Ctrl+D", "bookmark this folder"),
            ("Ctrl+P  ·  :", "everything else, by name"),
            ("Ctrl+L", "type a location"),
            ("i  ·  Shift+Space", "preview · quick look"),
            ("Ctrl+1 · 2 · 3", "list · grid · columns"),
            ("Ctrl+T  ·  Ctrl+W", "new tab · close it"),
            ("Ctrl+Tab  ·  Alt+1…9", "next tab · tab by number"),
            ("F3  ·  Tab", "split view · the other side"),
            ("F5  ·  F6", "copy · move across"),
            ("Ctrl+=  ·  Ctrl+−", "bigger · smaller tiles"),
            ("Ctrl+H  ·  .", "dotfiles"),
            ("~  ·  g h", "home"),
            ("g g  ·  G", "first · last"),
            ("Ctrl+B", "sidebar"),
            ("Ctrl+Shift+C", "copy the path"),
            ("Menu  ·  right-click", "more"),
            ("Ctrl+R", "reload"),
            ("Ctrl+Shift+Enter", "where a link leads"),
        ];
        if let Some(picker) = &self.picker {
            keys.push(("Ctrl+Enter", "finish"));
            if picker.name.is_some() {
                keys.push(("Tab", "list, or the name"));
            }
            keys.push(("Esc (nothing left)", "cancel"));
        } else {
            keys.push(("Ctrl+Q", "close"));
        }
        let rows = keys.len().div_ceil(2);
        let line = (small * 1.9).round();
        let height = 40.0 + rows as f32 * line + 2.0 * space::XL;
        let inner = controls::dialog(canvas, bounds, 640.0, height, &look, 1.0);
        canvas.text(
            &Text::new("Keys", self.font(type_scale::TITLE), colors.text).medium(),
            inner.x,
            inner.y,
        );
        let column = (inner.w - space::XL) / 2.0;
        let key_width = column * 0.48;
        for (index, (key, what)) in keys.iter().enumerate() {
            let (col, row) = (index / rows, index % rows);
            let x = inner.x + col as f32 * (column + space::XL);
            let y = inner.y + 40.0 + row as f32 * line;
            let rect = Rect::new(x, y, key_width, line);
            canvas.text_in(
                &Text::new(key, small, look.accent).medium(),
                rect,
                Align::Start,
            );
            let rest = Rect::new(x + key_width, y, column - key_width, line);
            canvas.text_in(&Text::new(what, small, colors.subtle), rest, Align::Start);
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
        let zone = jiff::Zoned::now();
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
        if !self.pane.source().is_folder()
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
        } else if let Ok(space) = places::space(self.pane.location()) {
            let free = format!("{} free", format::size(space.free));
            canvas.text_in(&Text::new(&free, small, colors.subtle), inner, Align::End);
        }
    }

    /// The question about saving over a file, if it's being asked.
    fn draw_confirm(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(path) = self
            .picker
            .as_ref()
            .and_then(|picker| picker.confirm.clone())
        else {
            return;
        };
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        // Everything under the question is out of reach.
        self.hits.clear();
        let inner = controls::dialog(canvas, bounds, 420.0, 168.0, &look, 1.0);
        let name = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
        let title = format!("Replace “{name}”?");
        canvas.text(
            &Text::new(&title, self.font(type_scale::TITLE), colors.text)
                .medium()
                .max_width(inner.w),
            inner.x,
            inner.y,
        );
        let folder = path
            .parent()
            .map(|folder| folder.to_string_lossy().into_owned())
            .unwrap_or_default();
        let detail =
            format!("A file with this name is already in {folder}. Saving replaces what's in it.");
        canvas.text(
            &Text::new(&detail, self.font(type_scale::SMALL), colors.subtle)
                .max_width(inner.w)
                .max_lines(2),
            inner.x,
            inner.y + 30.0,
        );
        let y = inner.bottom() - controls::HEIGHT;
        let replace = Button::new("Replace", Kind::Danger);
        let width = replace.width(canvas, &look);
        let replace_rect = Rect::new(inner.right() - width, y, width, controls::HEIGHT);
        replace.draw(
            canvas,
            replace_rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::Replace),
                focused: true,
                ..Interaction::default()
            },
        );
        let keep = Button::new("Cancel", Kind::Secondary);
        let width = keep.width(canvas, &look);
        let keep_rect = Rect::new(
            replace_rect.x - space::SM - width,
            y,
            width,
            controls::HEIGHT,
        );
        keep.draw(
            canvas,
            keep_rect,
            &look,
            Interaction {
                hovered: self.hover == Some(Hit::KeepFile),
                ..Interaction::default()
            },
        );
        self.hits.push((replace_rect, Hit::Replace));
        self.hits.push((keep_rect, Hit::KeepFile));
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
fn crumbs(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_core::{Context, Paths};
    use enso_fs::list::{Kind, Listing, Meta, Target};
    use enso_ui::{Offscreen, TextEngine, widget::TextField};

    use super::*;
    use crate::{
        config::FilesConfig,
        pick::{self, Filter, Mode},
        picker::Outcome,
    };

    fn context() -> Context {
        Context::load_from(Paths::rooted("/nonexistent/enso-files-test"))
    }

    fn fill(files: &mut Files) {
        let request = files.pending.take().unwrap();
        let entries = [
            ("crates", Kind::Dir),
            ("docs", Kind::Dir),
            ("target", Kind::Dir),
            ("latest", Kind::Link(Target::Dir)),
            (".git", Kind::Dir),
            ("Cargo.lock", Kind::File),
            ("Cargo.toml", Kind::File),
            (
                "a file with a name far too long to fit in its column at all.txt",
                Kind::File,
            ),
            ("README.md", Kind::File),
            ("photo.png", Kind::File),
        ];
        let listing = Listing::from_entries(request.path.clone(), entries);
        let now = jiff::Timestamp::now().as_nanosecond() as i64;
        let metas: Vec<Option<Meta>> = (0..entries.len())
            .map(|i| {
                Some(Meta {
                    size: 1500 * (i as u64 + 1).pow(4),
                    modified: now - (i as i64) * 7_200_000_000_000,
                    created: None,
                    accessed: now,
                    mode: 0o644,
                    uid: 1000,
                    gid: 1000,
                    link_size: None,
                })
            })
            .collect();
        assert!(files.pane.set_listing(request.generation, Ok(listing)));
        assert!(files.pane.apply_stats(request.generation, 0, &metas));
    }

    /// The manager onto a made-up folder, its listing and sizes arrived.
    fn files() -> Files {
        let mut files = Files::new(
            context(),
            FilesConfig::default(),
            "/home/me/dev/enso".into(),
        );
        files.home = "/home/me".into();
        files.places = Vec::new();
        fill(&mut files);
        files
    }

    fn render(files: &mut Files, size: (u32, u32)) -> Offscreen {
        let later = Instant::now() + Duration::from_secs(2);
        let mut text = TextEngine::new(&files.ctx.config.appearance);
        Offscreen::render(&mut text, size, 2, |canvas| {
            assert_eq!(files.draw_window(canvas, later), Paint::Idle);
        })
    }

    fn snapshot(frame: &Offscreen, name: &str) {
        frame
            .save_png(format!(
                "{}/../../target/snapshots/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
    }

    fn rows(files: &Files) -> usize {
        files
            .hits
            .iter()
            .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
            .count()
    }

    #[test]
    fn renders_a_folder() {
        let mut files = files();
        files.pane.set_cursor(1);
        files
            .selection
            .insert("/home/me/dev/enso/Cargo.toml".into());
        files.places = vec![super::super::Place {
            label: "Home".into(),
            icon: icon::FOLDER,
            path: "/home/me".into(),
        }];
        let frame = render(&mut files, (900, 420));
        snapshot(&frame, "enso-files");
        assert_eq!(rows(&files), 9);
        assert!(
            files
                .hits
                .iter()
                .all(|(rect, hit)| !matches!(hit, Hit::Row(_)) || rect.y >= TITLE_HEIGHT)
        );
        let crumbs: Vec<&Hit> = files
            .hits
            .iter()
            .map(|(_, hit)| hit)
            .filter(|hit| matches!(hit, Hit::Crumb(_)))
            .collect();
        assert_eq!(
            crumbs,
            [
                &Hit::Crumb("/home/me".into()),
                &Hit::Crumb("/home/me/dev".into())
            ]
        );
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| *hit == Hit::Place("/home/me".into()))
        );
    }

    #[test]
    fn renders_the_filter() {
        let mut files = files();
        let mut field = TextField::new("carg");
        field.select(1..3);
        files.filter = Some(field);
        files.pane.set_filter("carg");
        snapshot(&render(&mut files, (900, 300)), "enso-files-filter");
        assert_eq!(files.pane.len(), 2);
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Filter(_)))
        );
    }

    #[test]
    fn renders_narrow_and_empty() {
        let mut files = files();
        snapshot(&render(&mut files, (420, 300)), "enso-files-narrow");
        assert!(
            !files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Place(_))),
            "no sidebar when narrow"
        );

        let mut empty = Files::new(context(), FilesConfig::default(), "/home/me/new".into());
        let request = empty.pending.take().unwrap();
        let only_hidden = Listing::from_entries("/home/me/new", [(".keep", Kind::File)]);
        assert!(empty.pane.set_listing(request.generation, Ok(only_hidden)));
        snapshot(&render(&mut empty, (600, 300)), "enso-files-empty");
        assert_eq!(rows(&empty), 0);
    }

    fn picker(mode: Mode) -> Files {
        let mut request = pick::Request::new(mode);
        request.title = "Upload a file — file.io".into();
        request.folder = Some("/".into());
        request.filters = vec![
            Filter::parse("All files:*").unwrap(),
            Filter::parse("Images:*.png,*.jpg").unwrap(),
        ];
        request.choices = vec![pick::Choice::Toggle {
            id: "ro".into(),
            label: "Read-only".into(),
            on: true,
        }];
        let mut files = Files::picker(
            context(),
            FilesConfig::default(),
            request,
            Outcome::default(),
        );
        files.home = "/home/me".into();
        files.places = Vec::new();
        fill(&mut files);
        files
    }

    #[test]
    fn renders_a_picker_with_its_basket() {
        let mut files = picker(Mode::Open {
            multiple: true,
            directory: false,
        });
        files.selection.insert("/Cargo.toml".into());
        files
            .selection
            .insert("/home/me/Pictures/samurai_bebop.png".into());
        files.selection.insert("/home/me/notes.txt".into());
        snapshot(&render(&mut files, (980, 480)), "enso-files-picker");
        let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
        assert!(hits.contains(&&Hit::Accept), "three chosen: ready");
        assert!(hits.contains(&&Hit::Cancel));
        assert!(hits.contains(&&Hit::FilterMenu));
        assert!(hits.contains(&&Hit::Choice(0)));
        assert!(hits.contains(&&Hit::Unchoose("/home/me/notes.txt".into())));
    }

    #[test]
    fn renders_a_save_dialog_and_its_question() {
        let mut files = picker(Mode::Save {
            name: "report.pdf".into(),
        });
        snapshot(&render(&mut files, (900, 420)), "enso-files-save");
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Name(_)))
        );
        files.picker.as_mut().unwrap().confirm = Some("/Cargo.toml".into());
        snapshot(&render(&mut files, (900, 420)), "enso-files-replace");
        let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
        assert_eq!(
            hits,
            [&Hit::Replace, &Hit::KeepFile],
            "only the question answers"
        );
    }

    #[test]
    fn renders_the_location_and_the_keys() {
        let mut files = files();
        let mut field = TextField::new("~/dev/en");
        field.select(0..0);
        files.location = Some(field);
        snapshot(&render(&mut files, (900, 300)), "enso-files-location");
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Location(_)))
        );
        assert!(
            !files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Crumb(_)))
        );
        files.location = None;
        files.help = true;
        snapshot(&render(&mut files, (900, 560)), "enso-files-keys");
    }

    #[test]
    fn renders_operations_and_their_questions() {
        let mut files = files();
        files.begin_rename("/home/me/dev/enso/Cargo.toml".into());
        files.running = Some(super::super::ops::Running {
            label: "Copying 3 items".into(),
            progress: enso_fs::ops::Progress {
                bytes: 40,
                total_bytes: 100,
                ..Default::default()
            },
            stop: Default::default(),
        });
        files.toast = Some((
            "Moved 2 items to the trash · Ctrl+Z puts them back".into(),
            Instant::now(),
        ));
        snapshot(&render(&mut files, (900, 420)), "enso-files-rename");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::StopOp));

        let (reply, _answer) = std::sync::mpsc::channel();
        files.asked = Some(super::super::ops::Asked {
            source: "/tmp/notes.txt".into(),
            target: "/home/me/dev/enso/notes.txt".into(),
            reply,
            all: true,
        });
        snapshot(&render(&mut files, (900, 420)), "enso-files-conflict");
        let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
        assert!(hits.contains(&&Hit::Resolve(enso_fs::ops::Resolve::KeepBoth)));
        assert!(hits.contains(&&Hit::ForAll));
        assert!(
            !hits.iter().any(|hit| matches!(hit, Hit::Row(_))),
            "only the question answers"
        );

        files.asked = None;
        files.confirm_delete = Some(vec!["/home/me/dev/enso/target".into()]);
        snapshot(&render(&mut files, (900, 420)), "enso-files-delete");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::DeleteForGood));
    }

    #[test]
    fn renders_settings_and_version_control() {
        let mut files = files();
        files.repo = Some((
            "/home/me/dev/enso".into(),
            enso_fs::git::Status {
                branch: Some("enso-files".into()),
                ahead: 2,
                tracking: true,
                ..Default::default()
            },
        ));
        files.overrides.git = Some(true);
        files.git_facts = super::super::vcs::GitFacts {
            available: true,
            global: enso_fs::git::Identity {
                name: Some("Anshul".into()),
                email: Some("me@example.com".into()),
            },
            own: None,
        };
        files.settings = Some(4);
        snapshot(&render(&mut files, (900, 640)), "enso-files-settings");
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::SortPill(_)))
        );
        assert_eq!(
            files
                .hits
                .iter()
                .filter(|(_, hit)| matches!(hit, Hit::ViewPill(_)))
                .count(),
            3,
            "list, grid, and columns can be chosen in settings"
        );
        files.settings = None;
        snapshot(&render(&mut files, (900, 420)), "enso-files-branch");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::Branch));

        files.vcs = Some(super::super::vcs::Vcs::Ask);
        snapshot(&render(&mut files, (900, 420)), "enso-files-ask-git");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::GitYes));
        files.vcs = Some(super::super::vcs::Vcs::Identity(
            super::super::vcs::IdentityEdit {
                scope: enso_fs::git::Scope::Global,
                name: TextField::new("Anshul"),
                email: TextField::new("me@example.com"),
                focus: 1,
                error: None,
            },
        ));
        snapshot(&render(&mut files, (900, 420)), "enso-files-identity");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::FormField(1)));
    }

    #[test]
    fn renders_the_drives_and_the_trash() {
        let mut files = files();
        files.places = vec![super::super::Place {
            label: "Home".into(),
            icon: icon::HOME,
            path: "/home/me".into(),
        }];
        let volume =
            |label: Option<&str>, mount: Option<&str>, removable| enso_sys::udisks::Volume {
                object: format!(
                    "/org/freedesktop/UDisks2/block_devices/{}",
                    label.unwrap_or("x")
                ),
                device: "/dev/sdb1".into(),
                label: label.map(str::to_owned),
                kind: "ntfs".into(),
                size: 1_022_789_419_008,
                mount: mount.map(PathBuf::from),
                removable,
                system: !removable,
                drive: Some("/d".into()),
                can_eject: removable,
            };
        files.volumes = vec![
            volume(Some("STICK"), Some("/tmp"), true),
            volume(None, None, false),
        ];
        let frame = render(&mut files, (900, 520));
        snapshot(&frame, "enso-files-drives");
        let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
        assert!(hits.contains(&&Hit::Volume(1)));
        assert!(hits.contains(&&Hit::Eject(0)), "a mounted drive ejects");
        assert!(!hits.contains(&&Hit::Eject(1)), "an unmounted one doesn't");
        assert!(hits.contains(&&Hit::TrashPlace));
        assert!(hits.contains(&&Hit::Place("/".into())));
    }

    #[test]
    fn renders_a_grid_with_thumbnails() {
        let mut files = files();
        files.config.view = crate::config::View::Grid;
        let (w, h) = (160u32, 100u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, 200, 255]
            })
            .collect();
        files.thumbs.arrived(
            "/home/me/dev/enso/photo.png".into(),
            Some(enso_fs::thumbs::Thumb {
                width: w,
                height: h,
                rgba,
            }),
        );
        files.selection.insert("/home/me/dev/enso/README.md".into());
        files.pane.set_cursor(2);
        let frame = render(&mut files, (900, 520));
        snapshot(&frame, "enso-files-grid");
        let tiles = files
            .hits
            .iter()
            .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
            .count();
        assert_eq!(tiles, 9, "every tile answers the pointer");
        assert!(files.columns() >= 4);
    }

    #[test]
    fn renders_the_preview_and_the_quick_look() {
        let mut files = files();
        files.pane.set_cursor(6);
        files.previewing = true;
        let path = files.pane.path(6);
        files.shown.path = Some(path.clone());
        files.shown.side = 10_000;
        files.preview_arrived(
            path,
            crate::jobs::Preview::Text(vec![
                "[workspace]".into(),
                "resolver = \"3\"".into(),
                "members = [\"crates/*\"]".into(),
            ]),
        );
        snapshot(&render(&mut files, (1000, 480)), "enso-files-preview");

        files.pane.set_cursor(7);
        let picture = files.pane.path(7);
        assert!(picture.ends_with("photo.png"));
        files.shown = Default::default();
        files.shown.path = Some(picture.clone());
        files.shown.side = 10_000;
        let (w, h) = (300u32, 200u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w * 255 / w) as u8, 120, (i / w * 255 / h) as u8, 255])
            .collect();
        files.preview_arrived(
            picture,
            crate::jobs::Preview::Picture(enso_fs::thumbs::Thumb {
                width: w,
                height: h,
                rgba,
            }),
        );
        files.peek = true;
        snapshot(&render(&mut files, (1000, 600)), "enso-files-peek");
        assert!(files.hits.is_empty(), "the quick look takes every click");
    }

    #[test]
    fn a_dragged_box_chooses_what_it_touches() {
        let mut files = files();
        render(&mut files, (900, 520));
        let now = Instant::now();
        let list = files.list;
        let row = files.row_height();
        // From the empty space below the rows, up over the last three.
        files.begin_band(list.x + 300.0, list.bottom() - 4.0, false, now);
        assert!(files.drag_band(list.x + 40.0, list.y + row * 6.5, now));
        let chosen: Vec<String> = files
            .selection
            .paths()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(chosen, ["Cargo.toml", "photo.png", "README.md"]);
        // Smaller again: only what it still touches.
        files.drag_band(list.x + 40.0, list.y + row * 8.5, now);
        assert_eq!(files.selection.len(), 1);
        assert!(files.end_band());
        assert!(!files.end_band());
    }

    #[test]
    fn renders_columns() {
        let root = std::env::temp_dir().join(format!("enso-files-columns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "dev/enso/crates/enso-ui",
            "dev/enso/crates/enso-fs",
            "dev/enso/docs",
            "dev/sumi",
            "dev/notes",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("dev/enso/Cargo.toml"), "").unwrap();
        let here = root.join("dev/enso");
        let mut files = Files::new(context(), FilesConfig::default(), here.clone());
        files.places = Vec::new();
        files.config.view = crate::config::View::Columns;
        let request = files.pending.take().unwrap();
        assert!(
            files
                .pane
                .set_listing(request.generation, Ok(Listing::read(&here).unwrap()))
        );
        let frame = render(&mut files, (1100, 420));
        snapshot(&frame, "enso-files-columns");
        let sides: Vec<&Hit> = files
            .hits
            .iter()
            .map(|(_, hit)| hit)
            .filter(|hit| matches!(hit, Hit::Side(..)))
            .collect();
        // The folder above (enso, notes, sumi), and the cursor's folder
        // (crates: enso-fs, enso-ui).
        assert_eq!(sides.len(), 5, "{sides:?}");
        assert!(sides.contains(&&Hit::Side(root.join("dev/sumi"), false)));
        assert!(sides.contains(&&Hit::Side(here.join("crates/enso-ui"), true)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tabs_and_the_split() {
        let root = std::env::temp_dir().join(format!("enso-files-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["music/albums", "photos/2026", "photos/2025"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let now = Instant::now();
        let mut files = Files::new(context(), FilesConfig::default(), root.join("music"));
        files.places = Vec::new();
        files.asked_git = true;
        let first = files.pending.take().unwrap();
        assert!(files.listed_now(
            first.generation,
            Ok(Listing::read(root.join("music")).unwrap()),
            now
        ));

        // A second tab, behind: its listing finds it by its number.
        assert!(files.new_tab(root.join("photos"), false));
        assert_eq!((files.tabs.len(), files.active), (2, 0));
        let generation = files.tabs[1].as_ref().unwrap().generation();
        assert_ne!(
            generation,
            files.pane.generation(),
            "numbers are never shared"
        );
        assert_eq!(files.tab_of(generation), Some(1));
        let photos = Listing::read(root.join("photos")).unwrap();
        assert!(files.with_tab(1, |files| files.listed_now(generation, Ok(photos), now)));
        assert_eq!(
            files.pane.location(),
            root.join("music"),
            "the tab in front is untouched"
        );

        files.switch_tab(1);
        assert_eq!(files.pane.location(), root.join("photos"));
        assert_eq!(files.pane.len(), 2);
        assert_eq!(files.tab_location(0), root.join("music"));

        // Side by side: the other side's rows answer as the other side's.
        files.toggle_split();
        assert_eq!(files.split, Some(0));
        let frame = render(&mut files, (1100, 420));
        snapshot(&frame, "enso-files-split");
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Tabs(_)))
        );
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::Other(inner) if **inner == Hit::Row(0)))
        );
        files.other_side();
        assert_eq!((files.active, files.split), (0, Some(1)));
        assert_eq!(files.pane.location(), root.join("music"));

        // Closing the one in front leaves the other, and no split.
        assert!(files.close_tab(0));
        assert_eq!((files.tabs.len(), files.active, files.split), (1, 0, None));
        assert_eq!(files.pane.location(), root.join("photos"));
        assert!(!files.close_tab(0), "the last tab is the window");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renders_search_results() {
        use crate::pane::Source;
        let root = std::env::temp_dir().join(format!("enso-files-found-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/window")).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    let sky = \"blue\";\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/window/draw.rs"), "// the sky, drawn\n").unwrap();
        std::fs::write(root.join("notes.md"), "# Notes\nLook at the sky tonight.\n").unwrap();
        let now = Instant::now();
        let mut files = Files::new(context(), FilesConfig::default(), root.clone());
        files.asked_git = true;
        files.bookmarks = vec![enso_fs::bookmarks::Bookmark {
            path: root.join("src"),
            label: None,
        }];
        let first = files.pending.take().unwrap();
        files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);

        let source = Source::Search {
            query: "sky".into(),
            content: true,
        };
        let request = files.pane.show(root.clone(), source).unwrap();
        let none = std::sync::atomic::AtomicBool::new(false);
        let found = crate::gather::search(&root, false, "sky", true, &none).unwrap();
        files.pane.set_notes(found.notes);
        assert!(files.listed_now(request.generation, Ok(found.listing), now));
        assert_eq!(files.pane.len(), 3);
        assert!(files.pane.note(0).is_some_and(|note| note.contains("sky")));
        snapshot(&render(&mut files, (1000, 440)), "enso-files-search");
        assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::Recent));
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| *hit == Hit::Place(root.join("src")))
        );

        // Up from the results is the folder searched.
        let request = files.pane.parent().unwrap();
        assert_eq!(request.source, Source::Folder);
        assert_eq!(request.path, root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_palette_finds_commands_and_chooses_by_kind() {
        let mut files = files();
        files.open_palette();
        {
            let palette = files.palette.as_mut().unwrap();
            palette.field.insert("grid");
            palette.refilter();
        }
        let palette = files.palette.as_ref().unwrap();
        let first = &palette.lines[palette.shown[0]];
        assert_eq!(first.label, "Show as a grid");
        assert_eq!(first.aside, "Ctrl+2");
        {
            let palette = files.palette.as_mut().unwrap();
            palette.field.set_text("");
            palette.refilter();
        }
        snapshot(&render(&mut files, (900, 560)), "enso-files-palette");
        assert!(
            files
                .hits
                .iter()
                .any(|(_, hit)| matches!(hit, Hit::PaletteLine(_)))
        );
        files.palette = None;
        assert_eq!(files.choose_where("type:image"), 1);
        assert!(
            files
                .selection
                .paths()
                .iter()
                .any(|path| path.ends_with("photo.png"))
        );
    }

    #[test]
    fn folders_slide_in_the_way_you_went() {
        let root = std::env::temp_dir().join(format!("enso-files-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("inner")).unwrap();
        let now = Instant::now();
        let mut files = Files::new(context(), FilesConfig::default(), root.clone());
        files.asked_git = true;
        let first = files.pending.take().unwrap();
        files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);
        files.navigate(root.join("inner"));
        assert_eq!(files.sliding, 1.0, "in: from the right");
        let generation = files.pane.generation();
        files.listed_now(
            generation,
            Ok(Listing::read(root.join("inner")).unwrap()),
            now,
        );
        let request = files.pane.parent();
        files.load(request);
        assert_eq!(files.sliding, -1.0, "out: from the left");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crumbs_start_at_home_or_the_root() {
        let home = Path::new("/home/me");
        let labels = |path: &str| -> Vec<String> {
            crumbs(Path::new(path), home)
                .into_iter()
                .map(|(label, _)| label)
                .collect()
        };
        assert_eq!(labels("/home/me/dev/enso"), ["~", "dev", "enso"]);
        assert_eq!(labels("/home/me"), ["~"]);
        assert_eq!(labels("/usr/lib"), ["/", "usr", "lib"]);
        assert_eq!(labels("/"), ["/"]);
    }

    #[test]
    fn titles_name_the_folder() {
        let home = Path::new("/home/me");
        assert_eq!(title(Path::new("/home/me"), home), "Home — Files");
        assert_eq!(title(Path::new("/home/me/dev"), home), "dev — Files");
        assert_eq!(title(Path::new("/"), home), "/ — Files");
    }
}
