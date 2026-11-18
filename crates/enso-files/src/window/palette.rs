//! The command palette (`Ctrl+P`, `Ctrl+Shift+P`, or `:`): everything the
//! window can do, the folders you go to most, and ways to choose by kind
//! or date — one fuzzy list, each command with its key beside it, so the
//! palette is also how the keys are learnt.
//!
//! A command is run by pressing its key on your behalf, so what the
//! palette does and what the key does can never drift apart.

use std::path::PathBuf;

use enso_core::style::{icon, opacity, radius, space, stroke, type_scale};
use enso_fs::fuzzy::Pattern;
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Mods, Nav, Rect, Text,
    widget::{TextField, field},
};

use super::overlay::{Overlay, Reach};
use super::{Files, Hit};
use crate::query::Query;

/// Rows shown at once.
const ROWS: usize = 9;
const ROW: f32 = 36.0;

/// What a line of the palette does.
#[derive(Clone, Debug, PartialEq)]
enum Do {
    /// Press this key.
    Press(Key, Mods),
    Recent,
    Trash,
    Compress,
    /// Go to this folder.
    Go(PathBuf),
    /// Choose what in this folder these query words admit.
    Choose(&'static str),
}

#[derive(Clone, Debug)]
pub(super) struct Line {
    pub(super) label: String,
    /// The key, or where a folder is, shown at the right.
    pub(super) aside: String,
    what: Do,
}

#[derive(Debug)]
pub(super) struct Palette {
    pub(super) field: TextField,
    pub(super) lines: Vec<Line>,
    /// The lines that match, best first.
    pub(super) shown: Vec<usize>,
    cursor: usize,
    first: usize,
}

const CTRL: Mods = Mods {
    ctrl: true,
    alt: false,
    shift: false,
    logo: false,
    caps_lock: false,
};
const CTRL_SHIFT: Mods = Mods {
    shift: true,
    ..CTRL
};
const SHIFT: Mods = Mods {
    ctrl: false,
    ..CTRL_SHIFT
};
const ALT: Mods = Mods {
    alt: true,
    ctrl: false,
    ..CTRL
};
const PLAIN: Mods = Mods {
    ctrl: false,
    ..CTRL
};

/// The commands: what they're called, their key as written, the key.
/// Those after the line only make sense in the file manager.
const COMMANDS: &[(&str, &str, Key, Mods)] = &[
    ("Search below here", "Ctrl+F", Key::Char('f'), CTRL),
    (
        "Search inside files",
        "Ctrl+Shift+F",
        Key::Char('F'),
        CTRL_SHIFT,
    ),
    ("Filter this folder", "/", Key::Char('/'), PLAIN),
    ("Type a location", "Ctrl+L", Key::Char('l'), CTRL),
    ("Go back", "Alt+←", Key::Left, ALT),
    ("Go forward", "Alt+→", Key::Right, ALT),
    ("Go up a folder", "Alt+↑", Key::Up, ALT),
    ("Go home", "~", Key::Char('~'), PLAIN),
    ("Select all", "Ctrl+A", Key::Char('a'), CTRL),
    ("Turn the selection round", "*", Key::Char('*'), PLAIN),
    ("Show or hide dotfiles", "Ctrl+H", Key::Char('h'), CTRL),
    ("Show as a list", "Ctrl+1", Key::Char('1'), CTRL),
    ("Show as a grid", "Ctrl+2", Key::Char('2'), CTRL),
    ("Show as columns", "Ctrl+3", Key::Char('3'), CTRL),
    ("Preview pane", "i", Key::Char('i'), PLAIN),
    ("Quick look", "Shift+Space", Key::Char(' '), SHIFT),
    ("Sidebar", "Ctrl+B", Key::Char('b'), CTRL),
    ("Copy the path", "Ctrl+Shift+C", Key::Char('C'), CTRL_SHIFT),
    ("Reload", "Ctrl+R", Key::Char('r'), CTRL),
    ("Keys", "?", Key::Char('?'), PLAIN),
    ("New tab", "Ctrl+T", Key::Char('t'), CTRL),
    ("Next tab", "Ctrl+Tab", Key::Tab, CTRL),
    ("Split view", "F3", Key::F(3), PLAIN),
];

const MANAGING: &[(&str, &str, Key, Mods)] = &[
    ("Rename", "F2", Key::F(2), PLAIN),
    ("New folder", "Ctrl+Shift+N", Key::Char('N'), CTRL_SHIFT),
    ("Copy", "Ctrl+C", Key::Char('c'), CTRL),
    ("Cut", "Ctrl+X", Key::Char('x'), CTRL),
    ("Paste", "Ctrl+V", Key::Char('v'), CTRL),
    ("Duplicate", "Ctrl+Shift+D", Key::Char('D'), CTRL_SHIFT),
    ("Move to the trash", "Delete", Key::Delete, PLAIN),
    ("Delete for good", "Shift+Delete", Key::Delete, SHIFT),
    ("Undo", "Ctrl+Z", Key::Char('z'), CTRL),
    ("Redo", "Ctrl+Shift+Z", Key::Char('Z'), CTRL_SHIFT),
    ("Bookmark this folder", "Ctrl+D", Key::Char('d'), CTRL),
    ("Terminal here", "F4", Key::F(4), PLAIN),
    ("Properties", "Ctrl+I", Key::Char('i'), CTRL),
    (
        "Run as a program, or not",
        "Ctrl+Shift+X",
        Key::Char('X'),
        CTRL_SHIFT,
    ),
    ("Settings", "Ctrl+,", Key::Char(','), CTRL),
];

/// Choosing by what things are, or when they changed.
const CHOOSE: &[(&str, &str)] = &[
    ("Select the pictures here", "type:image"),
    ("Select the videos here", "type:video"),
    ("Select the music and sound here", "type:audio"),
    ("Select the documents here", "type:document"),
    ("Select the code here", "type:code"),
    ("Select the archives here", "type:archive"),
    ("Select the folders here", "type:folder"),
    ("Select what changed today", "modified:<1d"),
    ("Select what changed this week", "modified:<7d"),
    ("Select what's over 100 MB", "size:>100M"),
];

impl Palette {
    /// Run the query against every line.
    pub(super) fn refilter(&mut self) {
        let pattern = Pattern::new(self.field.text());
        let mut scored: Vec<(i32, usize)> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let haystack = match &line.what {
                    Do::Go(_) => format!("{} {}", line.label, line.aside),
                    _ => line.label.clone(),
                };
                pattern.score(&haystack).map(|score| (score, index))
            })
            .collect();
        if !self.field.text().is_empty() {
            // Best first; for equals, the order they were listed in.
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.shown = scored.into_iter().map(|(_, index)| index).collect();
        self.cursor = 0;
        self.first = 0;
    }

    fn reveal_cursor(&mut self) {
        if self.cursor < self.first {
            self.first = self.cursor;
        } else if self.cursor >= self.first + ROWS {
            self.first = self.cursor + 1 - ROWS;
        }
    }
}

impl Files {
    pub(super) fn open_palette(&mut self) {
        let managing = self.picker.is_none();
        let mut lines: Vec<Line> = COMMANDS
            .iter()
            .chain(if managing { MANAGING } else { &[] })
            .map(|&(label, keys, key, mods)| Line {
                label: label.to_owned(),
                aside: keys.to_owned(),
                what: Do::Press(key, mods),
            })
            .collect();
        lines.push(Line {
            label: "Recent files".to_owned(),
            aside: String::new(),
            what: Do::Recent,
        });
        if managing {
            lines.push(Line {
                label: "Compress…".to_owned(),
                aside: "zip · tar.gz · tar.xz".to_owned(),
                what: Do::Compress,
            });
            lines.push(Line {
                label: "Trash".to_owned(),
                aside: String::new(),
                what: Do::Trash,
            });
        }
        let multiple = self
            .picker
            .as_ref()
            .is_none_or(|picker| picker.request.multiple());
        if multiple && self.pane.source().is_folder() {
            lines.extend(CHOOSE.iter().map(|&(label, words)| Line {
                label: label.to_owned(),
                aside: words.to_owned(),
                what: Do::Choose(words),
            }));
        }
        // The folders: bookmarks, then those gone to most.
        let file = self.ctx.paths.state_dir.join(crate::frecent::FILE);
        let now = jiff::Timestamp::now().as_second();
        let mut folders: Vec<PathBuf> = self
            .bookmarks
            .iter()
            .map(|mark| mark.path.clone())
            .collect();
        for folder in crate::frecent::Visits::load(&file).best(40, now) {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        lines.extend(folders.into_iter().map(|folder| {
            let name = folder.file_name().map_or_else(
                || "/".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            Line {
                label: format!("Go to {name}"),
                aside: super::tabs::short(&folder, &self.home),
                what: Do::Go(folder),
            }
        }));
        let mut palette = Palette {
            field: TextField::new(""),
            lines,
            shown: Vec::new(),
            cursor: 0,
            first: 0,
        };
        palette.refilter();
        // In place of whatever was up: a menu, say.
        self.overlay = Some(Overlay::Palette(palette));
    }

    /// Keys while the palette is open. Returns whether it was.
    pub(super) fn palette_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(palette) = self.overlay.palette_mut() else {
            return false;
        };
        if let Some(nav) = key.nav(false) {
            let count = palette.shown.len();
            if count > 0 {
                palette.cursor = match nav {
                    Nav::Up => palette.cursor.checked_sub(1).unwrap_or(count - 1),
                    Nav::Down => (palette.cursor + 1) % count,
                };
                palette.reveal_cursor();
            }
            cx.redraw();
            return true;
        }
        match palette.field.key(key) {
            field::Response::Cancel => self.overlay = None,
            field::Response::Submit => {
                let chosen = palette.shown.get(palette.cursor).copied();
                self.run_line(chosen, cx);
            }
            field::Response::Changed | field::Response::Cut(_) => palette.refilter(),
            field::Response::Paste => cx.paste(&enso_ui::TEXT_TYPES),
            _ => {}
        }
        cx.redraw();
        true
    }

    /// A press while the palette is open. Returns whether it was.
    pub(super) fn palette_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.overlay.palette().is_none() {
            return false;
        }
        match hit {
            Some(Hit::PaletteLine(index)) => self.run_line(Some(*index), cx),
            Some(Hit::PaletteField) => {}
            // Anywhere else puts it away.
            _ => self.overlay = None,
        }
        cx.redraw();
        true
    }

    fn run_line(&mut self, index: Option<usize>, cx: &mut Cx<'_>) {
        let Some(palette) = self.overlay.take_palette() else {
            return;
        };
        let Some(line) = index.and_then(|index| palette.lines.get(index)) else {
            return;
        };
        match line.what.clone() {
            Do::Press(key, mods) => {
                let event = KeyEvent {
                    key,
                    mods,
                    repeat: false,
                };
                self.key(event, cx, std::time::Instant::now());
            }
            Do::Recent => self.open_recent(),
            Do::Trash => self.open_trash(),
            Do::Compress => self.ask_compress(cx),
            Do::Go(folder) => self.navigate(folder),
            Do::Choose(words) => {
                let chosen = self.choose_where(words);
                let text = if chosen == 0 {
                    "Nothing here is like that".to_owned()
                } else {
                    format!(
                        "Selected {}",
                        enso_fs::format::count(chosen, "item", "items")
                    )
                };
                self.toast(text, cx);
            }
        }
    }

    /// Choose every row the query words admit. Returns how many.
    pub(super) fn choose_where(&mut self, words: &str) -> usize {
        let query = Query::parse(words);
        let now = jiff::Timestamp::now().as_nanosecond() as i64;
        let mut chosen = 0;
        for row in 0..self.pane.len() {
            let entry = *self.pane.entry(row);
            let name = self.pane.name(row).into_owned();
            if self.can_choose(row)
                && query.admits(&name, entry.kind.is_dir(), entry.meta.as_ref(), now)
            {
                self.selection.insert(self.pane.path(row));
                chosen += 1;
            }
        }
        chosen
    }

    /// Paste into the palette's field. Returns whether it was open.
    pub(super) fn palette_paste(&mut self, text: &str) -> bool {
        let Some(palette) = self.overlay.palette_mut() else {
            return false;
        };
        palette.field.insert(text);
        palette.refilter();
        true
    }

    pub(super) fn draw_palette(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(palette) = self.overlay.palette_mut() else {
            return;
        };
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.ctx.config.appearance.font_size * type_scale::BODY;
        let small = self.ctx.config.appearance.font_size * type_scale::SMALL;
        canvas.fill(bounds, 0.0, colors.base.alpha(opacity::SCRIM * 0.6));
        let width = (bounds.w - 2.0 * space::LG).min(620.0);
        let rows = palette.shown.len().min(ROWS);
        let field_h = 52.0;
        let height = field_h
            + if rows > 0 {
                rows as f32 * ROW + 2.0 * space::SM
            } else {
                44.0
            };
        let card = Rect::new(
            (bounds.w - width) / 2.0,
            bounds.y + 64.0_f32.min(bounds.h * 0.12),
            width,
            height,
        );
        canvas.card(card, colors.surface, colors.highlight_med);

        // The field.
        let row = Rect::new(card.x, card.y, card.w, field_h);
        canvas.icon(
            icon::SEARCH,
            18.0,
            colors.subtle,
            Rect::new(row.x + space::LG, row.y, 20.0, row.h),
            Align::Start,
        );
        let text = Rect::new(
            row.x + space::LG + 20.0 + space::MD,
            row.y,
            row.w - 2.0 * space::LG - 32.0,
            row.h,
        );
        let style = field::Style {
            size: body,
            color: colors.text,
            placeholder: "Do something, or go somewhere…",
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        palette.field.draw(canvas, text, &style, true);
        self.hits.push((card, Hit::PaletteField));
        canvas.fill(
            Rect::new(card.x, row.bottom(), card.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );

        if rows == 0 {
            let empty = Rect::new(card.x, row.bottom(), card.w, 44.0);
            canvas.text_in(
                &Text::new("Nothing by that name", small, colors.muted),
                empty,
                Align::Center,
            );
            return;
        }
        let top = row.bottom() + space::SM;
        let hover = self.hover.clone();
        for (offset, &index) in palette
            .shown
            .iter()
            .skip(palette.first)
            .take(ROWS)
            .enumerate()
        {
            let line = &palette.lines[index];
            let at = palette.first + offset;
            let rect = Rect::new(
                card.x + space::SM,
                top + offset as f32 * ROW,
                card.w - 2.0 * space::SM,
                ROW,
            );
            if at == palette.cursor {
                canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
            } else if hover == Some(Hit::PaletteLine(index)) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let inner = rect.inset_xy(space::MD, 0.0);
            let (figure, tint) = match &line.what {
                Do::Go(_) => (Some(icon::FOLDER), accent),
                Do::Recent => (Some(icon::CLOCK), colors.subtle),
                Do::Trash => (Some(icon::TRASH), colors.subtle),
                Do::Compress => (None, colors.subtle),
                Do::Choose(_) => (Some(icon::CHECK), colors.iris),
                Do::Press(..) => (None, colors.subtle),
            };
            let label_x = inner.x + 26.0;
            if let Some(figure) = figure {
                canvas.icon(
                    figure,
                    16.0,
                    tint,
                    Rect::new(inner.x, inner.y, 18.0, inner.h),
                    Align::Start,
                );
            }
            let aside = Text::new(&line.aside, small, colors.muted).max_width(inner.w * 0.45);
            let aside_w = canvas.measure(&aside).w;
            canvas.text_in(&aside, inner, Align::End);
            let label = Text::new(&line.label, body, colors.text)
                .max_width(inner.right() - label_x - aside_w - space::MD);
            canvas.text_in(
                &label,
                Rect::new(label_x, inner.y, inner.w, inner.h),
                Align::Start,
            );
            self.hits.push((rect, Hit::PaletteLine(index)));
        }
    }
}
