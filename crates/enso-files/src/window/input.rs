//! Keys and the pointer.
//!
//! Each goes to the first thing that wants it: the question about
//! replacing a file, then an open menu, then whichever field has the
//! keyboard (the filter, the name being saved as), then the list.

use std::time::Instant;

use enso_core::{Rgba, motion::Transition, style::type_scale};
use enso_fs::sort::{By, Sort};
use enso_ui::{
    BUTTON_BACK, BUTTON_FORWARD, Button, Clip, Cursor, Cx, Key, KeyEvent, Nav, PointerEvent,
    TEXT_TYPES, Text,
    widget::{Item, Menu, TextField, field, menu},
};

use super::{Action, Files, Hit, MenuFor, tabs};
use crate::{location, pane::Open, pick::Choice, picker::Under};

/// Rows a wheel notch scrolls.
const WHEEL_ROWS: f32 = 3.0;

impl Files {
    pub(super) fn key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) {
        if self.help {
            // Any key puts the keys away.
            self.help = false;
            cx.redraw();
            return;
        }
        if self.peek_key(&key, cx, now)
            || self.palette_key(&key, cx)
            || self.renamer_key(&key, cx)
            || self.properties_key(&key, cx)
        {
            return;
        }
        if self.settings_key(&key, cx)
            || self.vcs_key(&key, cx)
            || self.ops_key(&key, cx)
            || self.confirm_key(&key, cx)
            || self.menu_key(&key, cx)
        {
            return;
        }
        if self.location_key(key, cx)
            || self.filter_key(key, cx, now)
            || self.name_key(key, cx, now)
        {
            return;
        }
        self.list_key(key, cx, now);
    }

    /// Keys while the quick look is up: moving shows the next file in it;
    /// Space or Esc puts it away.
    fn peek_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>, now: Instant) -> bool {
        if !self.peek {
            return false;
        }
        let across = match self.config.view {
            crate::config::View::Grid => self.columns() as isize,
            crate::config::View::List | crate::config::View::Columns => 1,
        };
        let delta = match key.key {
            Key::Escape | Key::Char(' ') | Key::Enter => {
                self.peek = false;
                None
            }
            Key::Left | Key::Char('h') => Some(-1),
            Key::Right | Key::Char('l') => Some(1),
            _ => key.nav(true).map(|nav| step(nav) * across),
        };
        if let Some(delta) = delta
            && self.pane.move_cursor(delta)
        {
            self.follow_cursor(now);
        }
        cx.redraw();
        true
    }

    /// Keys for the question about replacing a file.
    fn confirm_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(picker) = &mut self.picker else {
            return false;
        };
        let Some(path) = picker.confirm.clone() else {
            return false;
        };
        match key.key {
            Key::Enter => {
                self.finish(vec![path], cx);
                return true;
            }
            Key::Escape => picker.confirm = None,
            _ => {}
        }
        cx.redraw();
        true
    }

    fn menu_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some((menu, _)) = &mut self.menu else {
            return false;
        };
        let response = menu.key(key);
        self.menu_response(response, cx);
        true
    }

    fn menu_response(&mut self, response: menu::Response, cx: &mut Cx<'_>) {
        match response {
            menu::Response::Ignored => return,
            menu::Response::Changed => {}
            menu::Response::Closed => self.menu = None,
            menu::Response::Chosen(index) => {
                if let Some((menu, purpose)) = self.menu.take() {
                    self.menu_at = (menu.rect().x, menu.rect().y);
                    self.chosen(purpose, index, cx);
                }
            }
        }
        cx.redraw();
    }

    fn chosen(&mut self, purpose: MenuFor, index: usize, cx: &mut Cx<'_>) {
        match purpose {
            MenuFor::Context(actions) => {
                if let Some(Some(action)) = actions.into_iter().nth(index) {
                    self.act(action, cx);
                }
            }
            MenuFor::Filters => {
                if let Some(picker) = &mut self.picker {
                    picker.set_filter(index);
                    let keep = picker.keep();
                    self.pane.set_keep(keep);
                }
            }
            MenuFor::Branches(branches) => self.branch_chosen(branches, index, cx),
            MenuFor::OpenWith(path, choices) => match choices.get(index) {
                Some(choice) => {
                    let db = enso_fs::open::Database::from_env();
                    match db.open_with(&choice.id, &path, "alacritty") {
                        Ok(()) => super::used(vec![path.clone()]),
                        Err(err) => {
                            self.toast(format!("Can't open it with {}: {err}", choice.name), cx);
                        }
                    }
                }
                // Past the line: "Always use…".
                None if index == choices.len() + 1 => {
                    let items = choices
                        .iter()
                        .map(|choice| Item::new(&choice.name).checked(choice.default))
                        .collect();
                    let (x, y) = self.menu_at;
                    self.menu = Some((Menu::new(items, x, y), MenuFor::SetDefault(path, choices)));
                }
                None => {}
            },
            MenuFor::SetDefault(path, choices) => {
                if let Some(choice) = choices.get(index) {
                    let db = enso_fs::open::Database::from_env();
                    match db.set_default(&path, &choice.id) {
                        Ok(mime) => {
                            self.toast(format!("{mime} files open with {} now", choice.name), cx)
                        }
                        Err(err) => self.toast(format!("Can't change it: {err}"), cx),
                    }
                }
            }
            MenuFor::Choice(which) => {
                if let Some(picker) = &mut self.picker
                    && let Some(Choice::Options { options, .. }) = picker.choices.get(which)
                    && let Some((id, _)) = options.get(index).cloned()
                {
                    picker.set_choice(which, Some(&id));
                }
            }
        }
    }

    fn act(&mut self, action: Action, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match action {
            Action::Open => self.open(cx),
            Action::Toggle(path) => {
                if let Some(row) = (0..self.pane.len()).find(|&row| self.pane.path(row) == path) {
                    self.toggle(row);
                }
            }
            Action::CopyPath(path) => cx.copy(Clip::text(&path.to_string_lossy())),
            Action::SelectAll => self.select_all(),
            Action::Unselect => self.selection.clear(),
            Action::Sort(by) => {
                let sort = Sort {
                    by,
                    ..self.pane.sort()
                };
                self.sort_here(sort, cx);
            }
            Action::Descending => {
                let sort = self.pane.sort();
                self.sort_here(
                    Sort {
                        descending: !sort.descending,
                        ..sort
                    },
                    cx,
                );
            }
            Action::FoldersFirst => {
                let sort = self.pane.sort();
                self.sort_here(
                    Sort {
                        folders_first: !sort.folders_first,
                        ..sort
                    },
                    cx,
                );
            }
            Action::Hidden => self.toggle_hidden_here(cx),
            Action::Reload => {
                let request = self.pane.reload();
                if let Some(proxy) = &self.proxy {
                    self.loader.load(request, proxy);
                }
            }
            Action::Rename => {
                if let Some(row) = self.pane.cursor() {
                    self.begin_rename(self.pane.path(row));
                }
            }
            Action::NewFolder => self.new_folder(cx),
            Action::Copy => self.copy_files(false, cx),
            Action::Cut => self.copy_files(true, cx),
            Action::Paste => self.ask_paste(cx),
            Action::Duplicate => {
                if let Some(row) = self.pane.cursor() {
                    let path = self.pane.path(row);
                    self.start_op(crate::jobs::Op::Duplicate(path), cx);
                }
            }
            Action::Trash => self.trash_targets(cx),
            Action::Delete => self.ask_delete(cx),
            Action::Terminal => self.terminal_here(cx),
            Action::Undo => self.undo_last(cx),
            Action::InitRepo => self.init_repository(cx),
            Action::Restore => self.restore(cx),
            Action::Run(path) => {
                let dir = path.parent().map(std::path::Path::to_owned);
                let argv = vec![path.to_string_lossy().into_owned()];
                if let Err(err) = enso_fs::apps::spawn_detached(&argv, dir.as_deref()) {
                    self.toast(format!("Can't run it: {err}"), cx);
                }
            }
            Action::RealPath(path) => match std::fs::canonicalize(&path) {
                Ok(real) => self.reveal(&real),
                Err(_) => self.toast("It leads nowhere: what it points to is gone", cx),
            },
            Action::OpenWith(path) => {
                let choices = enso_fs::open::Database::from_env().choices(&path);
                if choices.is_empty() {
                    self.toast("Nothing here opens this kind of file", cx);
                } else {
                    let mut items: Vec<Item> = choices
                        .iter()
                        .map(|choice| Item::new(&choice.name).checked(choice.default))
                        .collect();
                    items.push(Item::Separator);
                    items.push(Item::new("Always use…"));
                    let (x, y) = self.menu_at;
                    self.menu = Some((Menu::new(items, x, y), MenuFor::OpenWith(path, choices)));
                }
            }
            Action::EmptyTrash => self.ask_empty(cx),
            Action::Reveal(path) => self.reveal(&path),
            Action::NewTab(path) => self.open_tab(path, false, cx),
            Action::Bookmark(path) => self.toggle_bookmark(&path, cx),
            Action::ShiftBookmark(path, step) => self.shift_bookmark(&path, step, cx),
            Action::Settings => {
                self.learn_git();
                self.settings = Some(0);
            }
        }
        self.follow_cursor(now);
        cx.redraw();
    }

    /// Open the menu for a right-click on `row` (or on empty space).
    fn context_menu(&mut self, row: Option<usize>, x: f32, y: f32) {
        let mut items = Vec::new();
        let mut actions = Vec::new();
        let mut add = |item: Item, action: Option<Action>| {
            items.push(item);
            actions.push(action);
        };
        if let Some(row) = row {
            let path = self.pane.path(row);
            let is_dir = self.pane.entry(row).kind.is_dir();
            let open = match (&self.picker, is_dir) {
                (_, true) => "Open",
                (Some(_), false) => "Choose",
                (None, false) => "Open",
            };
            add(Item::new(open).keys("Enter"), Some(Action::Open));
            if !is_dir && self.picker.is_none() {
                add(
                    Item::new("Open with…"),
                    Some(Action::OpenWith(path.clone())),
                );
                if super::is_program(&path) {
                    add(Item::new("Run"), Some(Action::Run(path.clone())));
                }
            }
            if is_dir && self.picker.is_none() {
                add(
                    Item::new("Open in a new tab").keys("Ctrl+Enter"),
                    Some(Action::NewTab(path.clone())),
                );
                let marked = self.bookmarks.iter().any(|mark| mark.path == path);
                add(
                    Item::new(if marked {
                        "Remove the bookmark"
                    } else {
                        "Bookmark"
                    }),
                    Some(Action::Bookmark(path.clone())),
                );
            }
            // In a gathered list, where it really is.
            if !self.pane.source().is_folder() {
                add(
                    Item::new("Show in its folder"),
                    Some(Action::Reveal(path.clone())),
                );
            }
            if self.pane.entry(row).kind.is_link() {
                add(
                    Item::new("Go to the real one").keys("Ctrl+Shift+Enter"),
                    Some(Action::RealPath(path.clone())),
                );
            }
            if self.can_choose(row) && !self.picker.as_ref().is_some_and(|p| p.request.is_save()) {
                let label = if self.selection.contains(&path) {
                    "Unselect"
                } else {
                    "Select"
                };
                add(
                    Item::new(label).keys("Space"),
                    Some(Action::Toggle(path.clone())),
                );
            }
            let managing = self.picker.is_none();
            if managing && self.in_trash() {
                add(Item::new("Put back"), Some(Action::Restore));
                add(
                    Item::new("Delete for good…").keys("Delete"),
                    Some(Action::Delete),
                );
            } else {
                add(Item::new("Rename").keys("F2"), Some(Action::Rename));
                if managing {
                    add(Item::new("Copy").keys("Ctrl+C"), Some(Action::Copy));
                    add(Item::new("Cut").keys("Ctrl+X"), Some(Action::Cut));
                    add(
                        Item::new("Duplicate").keys("Ctrl+Shift+D"),
                        Some(Action::Duplicate),
                    );
                }
                add(
                    Item::new("Copy path").keys("Ctrl+Shift+C"),
                    Some(Action::CopyPath(path)),
                );
                if managing {
                    add(
                        Item::new("Move to the trash").keys("Delete"),
                        Some(Action::Trash),
                    );
                    add(
                        Item::new("Delete for good…").keys("Shift+Delete"),
                        Some(Action::Delete),
                    );
                }
            }
            add(Item::Separator, None);
        }
        let folder = self.pane.source().is_folder();
        if self.picker.is_none() && self.in_trash() {
            add(Item::new("Empty the trash…"), Some(Action::EmptyTrash));
        } else if folder {
            add(
                Item::new("New folder").keys("Ctrl+Shift+N"),
                Some(Action::NewFolder),
            );
        }
        if self.git_on() && enso_fs::git::root(self.pane.location()).is_none() {
            add(
                Item::new("Start a Git repository here"),
                Some(Action::InitRepo),
            );
        }
        if self.picker.is_none() && folder && row.is_none() {
            let here = self.pane.location().to_owned();
            let marked = self.bookmarks.iter().any(|mark| mark.path == here);
            add(
                Item::new(if marked {
                    "Remove the bookmark"
                } else {
                    "Bookmark this folder"
                })
                .keys("Ctrl+D"),
                Some(Action::Bookmark(here)),
            );
        }
        if self.picker.is_none() && folder {
            add(Item::new("Paste").keys("Ctrl+V"), Some(Action::Paste));
            add(
                Item::new("Terminal here").keys("F4"),
                Some(Action::Terminal),
            );
            add(
                Item::new("Undo")
                    .keys("Ctrl+Z")
                    .enabled(!self.undo.is_empty()),
                Some(Action::Undo),
            );
        }
        add(Item::Separator, None);
        let multiple = self
            .picker
            .as_ref()
            .is_none_or(|picker| picker.request.multiple());
        if multiple {
            add(
                Item::new("Select all").keys("Ctrl+A"),
                Some(Action::SelectAll),
            );
        }
        if !self.selection.is_empty() {
            add(
                Item::new("Unselect all").keys("Esc"),
                Some(Action::Unselect),
            );
        }
        if multiple || !self.selection.is_empty() {
            add(Item::Separator, None);
        }
        let sort = self.pane.sort();
        for (by, label) in [
            (By::Name, "Sort by name"),
            (By::Size, "Sort by size"),
            (By::Modified, "Sort by date"),
            (By::Extension, "Sort by type"),
        ] {
            add(
                Item::new(label).checked(sort.by == by),
                Some(Action::Sort(by)),
            );
        }
        add(
            Item::new("Reversed").checked(sort.descending),
            Some(Action::Descending),
        );
        add(
            Item::new("Folders first").checked(sort.folders_first),
            Some(Action::FoldersFirst),
        );
        add(Item::Separator, None);
        add(
            Item::new("Show hidden files")
                .checked(self.pane.shows_hidden())
                .keys("Ctrl+H"),
            Some(Action::Hidden),
        );
        add(Item::new("Reload").keys("Ctrl+R"), Some(Action::Reload));
        if self.picker.is_none() {
            add(
                Item::new("Settings…").keys("Ctrl+,"),
                Some(Action::Settings),
            );
        }
        self.menu = Some((Menu::new(items, x, y), MenuFor::Context(actions)));
    }

    /// Keys for the filter field, while it is open. Returns whether the
    /// key was used.
    fn filter_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) -> bool {
        let Some(field) = &mut self.filter else {
            return false;
        };
        // The list keeps moving under arrows while typing; j and k are
        // letters here.
        if let Some(nav) = key.nav(false) {
            if self.pane.move_cursor(step(nav)) {
                self.follow_cursor(now);
            }
            cx.redraw();
            return true;
        }
        let seeking = self.seek.is_some();
        match field.key(&key) {
            field::Response::Ignored => return false,
            field::Response::Cancel if seeking => self.close_search(),
            field::Response::Cancel => {
                self.filter = None;
                self.pane.set_filter("");
                self.follow_cursor(now);
            }
            field::Response::Submit if seeking => {
                // Run it now, whichever kind; the field goes, the results
                // stay.
                self.seek_due = None;
                self.run_search();
                self.close_search();
            }
            field::Response::Submit => {
                // Leave the field; the folder stays filtered until Esc or
                // a new folder.
                self.filter = None;
                self.open(cx);
            }
            field::Response::Changed if seeking => self.search_typed(now, cx),
            // `//`: the filter becomes a search below.
            field::Response::Changed if field.text() == "/" => {
                self.open_search(super::seek::Seek::Names, cx);
                if let Some(field) = &mut self.filter {
                    field.set_text("");
                }
            }
            field::Response::Changed => {
                let text = field.text().to_owned();
                self.pane.set_filter(&text);
                self.follow_cursor(now);
            }
            field::Response::Copy(text) => cx.copy(Clip::text(&text)),
            field::Response::Cut(text) => {
                cx.copy(Clip::text(&text));
                let text = field.text().to_owned();
                self.pane.set_filter(&text);
                self.follow_cursor(now);
            }
            field::Response::Paste => cx.paste(&TEXT_TYPES),
            field::Response::Moved => {}
        }
        cx.redraw();
        true
    }

    /// Keys for the name being saved as, while it has the keyboard.
    fn name_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) -> bool {
        let Some(picker) = &mut self.picker else {
            return false;
        };
        if !picker.typing_name {
            return false;
        }
        let Some(field) = &mut picker.name else {
            return false;
        };
        if let Some(nav) = key.nav(false) {
            if self.pane.move_cursor(step(nav)) {
                self.follow_cursor(now);
            }
            cx.redraw();
            return true;
        }
        if key.key == Key::Tab {
            picker.typing_name = false;
            cx.redraw();
            return true;
        }
        match field.key(&key) {
            field::Response::Ignored => return false,
            field::Response::Submit => self.accept(Under::Nothing, cx),
            field::Response::Cancel => self.cancel(cx),
            field::Response::Copy(text) | field::Response::Cut(text) => cx.copy(Clip::text(&text)),
            field::Response::Paste => cx.paste(&TEXT_TYPES),
            field::Response::Changed | field::Response::Moved => {}
        }
        cx.redraw();
        true
    }

    /// Keys for the location typed as text, while it's open.
    fn location_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(field) = &mut self.location else {
            return false;
        };
        if key.key == Key::Tab {
            let files = self
                .picker
                .as_ref()
                .is_none_or(|picker| !picker.request.directory());
            let completed = location::complete(
                field.text(),
                self.pane.location(),
                &self.home,
                files,
                self.pane.shows_hidden(),
            );
            if let Some(completed) = completed {
                field.replace_all(&completed);
                self.location_missing = false;
            }
            cx.redraw();
            return true;
        }
        match field.key(&key) {
            field::Response::Ignored => return false,
            field::Response::Cancel => self.location = None,
            field::Response::Submit => {
                match location::target(field.text(), self.pane.location(), &self.home) {
                    location::Target::Folder(path) => {
                        self.location = None;
                        self.navigate(path);
                    }
                    location::Target::File(path) => {
                        self.location = None;
                        self.reveal(&path);
                    }
                    location::Target::Missing => self.location_missing = true,
                }
            }
            field::Response::Changed => self.location_missing = false,
            field::Response::Copy(text) | field::Response::Cut(text) => {
                cx.copy(Clip::text(&text));
            }
            field::Response::Paste => cx.paste(&TEXT_TYPES),
            field::Response::Moved => {}
        }
        cx.redraw();
        true
    }

    /// Open the location as text, all of it selected to type over.
    fn open_location(&mut self, cx: &mut Cx<'_>) {
        let mut field = TextField::new(&location::display(self.pane.location(), &self.home));
        field.select_all();
        self.location = Some(field);
        self.location_missing = false;
        self.filter = None;
        if let Some(picker) = &mut self.picker {
            picker.typing_name = false;
        }
        cx.redraw();
    }

    /// Text from the clipboard, for whichever field asked.
    pub(super) fn paste(&mut self, text: &str, now: Instant, cx: &mut Cx<'_>) {
        if let Some(field) = &mut self.location {
            field.insert(text);
            self.location_missing = false;
        } else if let Some(field) = &mut self.filter {
            field.insert(text);
            let text = field.text().to_owned();
            self.pane.set_filter(&text);
            self.follow_cursor(now);
        } else if let Some(field) = self
            .picker
            .as_mut()
            .filter(|picker| picker.typing_name)
            .and_then(|picker| picker.name.as_mut())
        {
            field.insert(text);
        }
        cx.redraw();
    }

    fn open_filter(&mut self, cx: &mut Cx<'_>) {
        if self.filter.is_none() {
            self.filter = Some(TextField::new(self.pane.filter()));
        }
        if let Some(picker) = &mut self.picker {
            picker.typing_name = false;
        }
        cx.redraw();
    }

    fn list_key(&mut self, key: KeyEvent, cx: &mut Cx<'_>, now: Instant) {
        let mods = key.mods;
        let g = std::mem::take(&mut self.pending_g);
        let rows = self.visible_rows();
        let picking = self.picker.is_some();
        // Shift with a movement extends the selection from where it started.
        let grid = self.config.view == crate::config::View::Grid;
        let across = if grid { self.columns() as isize } else { 1 };
        let movement = match key.key {
            // In the grid, up and down go a row of tiles; left and right
            // (and h, l) a tile.
            Key::Left | Key::Char('h') if grid && !mods.alt && !mods.ctrl => Some(-1),
            Key::Right | Key::Char('l') if grid && !mods.alt && !mods.ctrl => Some(1),
            _ if key.nav(true).is_some() => key.nav(true).map(|nav| step(nav) * across),
            Key::PageUp => Some(-rows),
            Key::PageDown => Some(rows),
            Key::Home => Some(isize::MIN / 2),
            Key::End => Some(isize::MAX / 2),
            _ => None,
        };
        if let Some(delta) = movement {
            if mods.shift {
                self.anchor = self.anchor.or(self.pane.cursor());
            }
            let moved = self.pane.move_cursor(delta);
            if mods.shift {
                self.select_range();
            } else {
                self.anchor = None;
            }
            if moved {
                self.follow_cursor(now);
            }
            cx.redraw();
            return;
        }
        let moved = match key.key {
            Key::Char('G') => self.pane.set_cursor(usize::MAX),
            Key::Char('g') if mods.is_plain() && g => self.pane.set_cursor(0),
            Key::Char('g') if mods.is_plain() => {
                self.pending_g = true;
                return;
            }
            Key::Char('h') if g && mods.is_plain() => {
                self.navigate(self.home.clone());
                false
            }
            Key::Enter if mods.ctrl && mods.shift => {
                if let Some(row) = self.pane.cursor() {
                    let path = self.pane.path(row);
                    self.act(super::Action::RealPath(path), cx);
                }
                return;
            }
            Key::Enter if mods.ctrl && picking => {
                self.accept(Under::Nothing, cx);
                return;
            }
            Key::Enter if picking => {
                match self.under() {
                    Under::File(path)
                        if self.picker.as_ref().is_some_and(|p| p.request.is_save()) =>
                    {
                        self.name_from(&path);
                    }
                    Under::Dir(path) => self.navigate(path),
                    under => self.accept(under, cx),
                }
                false
            }
            Key::Enter if mods.ctrl && !mods.shift => {
                // A folder, in a new tab behind this one.
                if let Some(Open::Dir(path)) = self.pane.open() {
                    self.open_tab(path, false, cx);
                }
                false
            }
            Key::Char('d') if mods.ctrl && !mods.shift && self.picker.is_none() => {
                if self.pane.source().is_folder() {
                    let here = self.pane.location().to_owned();
                    self.toggle_bookmark(&here, cx);
                }
                false
            }
            Key::Char('p' | 'P') if mods.ctrl => {
                self.open_palette();
                false
            }
            Key::Char(':') if mods.is_plain() => {
                self.open_palette();
                false
            }
            Key::Char('i' | 'I') if mods.ctrl && !mods.shift => {
                self.open_properties();
                false
            }
            Key::Enter if mods.alt => {
                self.open_properties();
                false
            }
            Key::Char('x' | 'X') if mods.ctrl && mods.shift && !picking => {
                self.toggle_runnable(cx);
                false
            }
            Key::Char('t') if mods.ctrl => {
                let here = self.pane.location().to_owned();
                self.open_tab(here, true, cx);
                false
            }
            Key::Tab if mods.ctrl => {
                self.step_tab(if mods.shift { -1 } else { 1 });
                false
            }
            Key::PageDown if mods.ctrl => {
                self.step_tab(1);
                false
            }
            Key::PageUp if mods.ctrl => {
                self.step_tab(-1);
                false
            }
            Key::Char(digit @ '1'..='9') if mods.alt => {
                let index = (digit as usize - '1' as usize).min(self.tabs.len() - 1);
                // Alt+9 is always the last, as in browsers.
                self.switch_tab(if digit == '9' {
                    self.tabs.len() - 1
                } else {
                    index
                });
                false
            }
            Key::F(3) => {
                self.toggle_split();
                false
            }
            Key::F(5) if self.split.is_some() => {
                self.send_across(false, cx);
                false
            }
            Key::F(6) if self.split.is_some() => {
                self.send_across(true, cx);
                false
            }
            Key::Tab
                if self.split.is_some()
                    && self.picker.as_ref().is_none_or(|p| p.name.is_none()) =>
            {
                self.other_side();
                false
            }
            Key::Enter | Key::Right | Key::Char('l') if !mods.alt && !mods.ctrl => {
                // Right and l only go into folders; they never open files.
                if key.key == Key::Enter || matches!(self.pane.open(), Some(Open::Dir(_))) {
                    self.open(cx);
                }
                false
            }
            Key::Left | Key::Char('h') | Key::Backspace if !mods.alt && !mods.ctrl => {
                let request = self.pane.parent();
                self.load(request);
                false
            }
            Key::Up if mods.alt => {
                let request = self.pane.parent();
                self.load(request);
                false
            }
            Key::Left | Key::Char('H') if mods.alt || key.key == Key::Char('H') => {
                let request = self.pane.back();
                self.load(request);
                false
            }
            Key::Right | Key::Char('L') if mods.alt || key.key == Key::Char('L') => {
                let request = self.pane.forward();
                self.load(request);
                false
            }
            Key::Char(' ') if mods.shift && !mods.ctrl => {
                self.peek = true;
                cx.redraw();
                return;
            }
            Key::Char('i') if mods.is_plain() => {
                self.previewing = !self.previewing;
                true
            }
            Key::Char(' ') if mods.is_plain() => {
                if let Some(row) = self.pane.cursor() {
                    self.toggle(row);
                    self.anchor = Some(row);
                }
                self.pane.move_cursor(1)
            }
            Key::Char('a') if mods.ctrl => {
                self.select_all();
                false
            }
            Key::Char('*') => {
                // Invert what's chosen in this folder.
                for row in 0..self.pane.len() {
                    if self.can_choose(row) {
                        self.toggle(row);
                    }
                }
                false
            }
            Key::Char('/') if mods.is_plain() => {
                self.open_filter(cx);
                return;
            }
            Key::Char('f' | 'F') if mods.ctrl => {
                let seek = if mods.shift {
                    super::seek::Seek::Contents
                } else {
                    super::seek::Seek::Names
                };
                self.open_search(seek, cx);
                return;
            }
            Key::Char('l') if mods.ctrl => {
                self.open_location(cx);
                return;
            }
            Key::Char(digit @ ('1' | '2' | '3')) if mods.ctrl => {
                let view = match digit {
                    '1' => crate::config::View::List,
                    '2' => crate::config::View::Grid,
                    _ => crate::config::View::Columns,
                };
                // This folder; Settings sets the rest.
                self.config.view = view;
                self.remember_here(|own| own.view = Some(view), cx);
                true
            }
            Key::Char('=' | '+') if mods.ctrl => {
                self.zoom_by(1);
                self.remember_zoom(cx);
                true
            }
            Key::Char('-') if mods.ctrl => {
                self.zoom_by(-1);
                self.remember_zoom(cx);
                true
            }
            Key::Char('0') if mods.ctrl => {
                self.zoom = super::grid::ZOOM;
                self.remember_zoom(cx);
                true
            }
            Key::Char('?') | Key::F(1) => {
                self.help = true;
                cx.redraw();
                return;
            }
            Key::Tab if self.picker.as_ref().is_some_and(|p| p.name.is_some()) => {
                if let Some(picker) = &mut self.picker {
                    picker.typing_name = true;
                }
                false
            }
            // Esc takes off one thing at a time: the filter, then what's
            // chosen, then — in a picker — the picker itself.
            Key::Escape if !self.pane.filter().is_empty() => {
                self.pane.set_filter("");
                true
            }
            Key::Escape if !self.selection.is_empty() => {
                self.selection.clear();
                false
            }
            // From results, back to the folder searched.
            Key::Escape if matches!(self.pane.source(), crate::pane::Source::Search { .. }) => {
                let folder = self.pane.location().to_owned();
                self.navigate(folder);
                false
            }
            Key::Escape if picking => {
                self.cancel(cx);
                return;
            }
            Key::Char('~') => {
                self.navigate(self.home.clone());
                false
            }
            Key::Char('h') if mods.ctrl => {
                self.toggle_hidden_here(cx);
                true
            }
            Key::Char('.') if mods.is_plain() => {
                self.toggle_hidden_here(cx);
                true
            }
            Key::Char('b') if mods.ctrl => {
                self.sidebar_open = !self.sidebar_open;
                false
            }
            Key::Char('r') if mods.ctrl => {
                self.act(super::Action::Reload, cx);
                return;
            }
            // Ctrl+Shift+C: the path under the cursor, as text.
            Key::Char('c' | 'C') if mods.ctrl && mods.shift => {
                if let Some(row) = self.pane.cursor() {
                    cx.copy(Clip::text(&self.pane.path(row).to_string_lossy()));
                }
                return;
            }
            Key::Menu => {
                let row = self.pane.cursor();
                let y = row.map_or(self.list.y, |row| {
                    self.list.y + (row as f32 + 1.0) * self.row_height() - self.scroll.get(now)
                });
                self.context_menu(row, self.list.x + 48.0, y);
                false
            }
            Key::F(2) => {
                if !self.rename_targets() {
                    self.act(super::Action::Rename, cx);
                }
                cx.redraw();
                return;
            }
            Key::Char('n' | 'N') if mods.ctrl && mods.shift => {
                self.new_folder(cx);
                return;
            }
            Key::Delete if mods.shift && !picking => {
                self.ask_delete(cx);
                return;
            }
            Key::Delete if !picking && self.in_trash() => {
                self.ask_delete(cx);
                return;
            }
            Key::Delete if !picking => {
                self.trash_targets(cx);
                return;
            }
            Key::Char('c') if mods.ctrl && !mods.shift && !picking => {
                self.copy_files(false, cx);
                return;
            }
            Key::Char('x') if mods.ctrl && !picking => {
                self.copy_files(true, cx);
                return;
            }
            Key::Char('v') if mods.ctrl && !picking => {
                self.ask_paste(cx);
                return;
            }
            Key::Char('d' | 'D') if mods.ctrl && mods.shift && !picking => {
                self.act(super::Action::Duplicate, cx);
                return;
            }
            Key::Char('z' | 'Z') if mods.ctrl && mods.shift && !picking => {
                self.redo_last(cx);
                return;
            }
            Key::Char('y') if mods.ctrl && !picking => {
                self.redo_last(cx);
                return;
            }
            Key::Char('z') if mods.ctrl && !picking => {
                self.undo_last(cx);
                return;
            }
            Key::Char(',') if mods.ctrl => {
                self.learn_git();
                self.settings = Some(0);
                cx.redraw();
                return;
            }
            Key::F(4) => {
                self.terminal_here(cx);
                return;
            }
            Key::Char('w') if mods.ctrl && self.tabs.len() > 1 => {
                self.close_tab(self.active);
                false
            }
            Key::Char('q' | 'w') if mods.ctrl => {
                self.cancel(cx);
                return;
            }
            // Saving: typing in the list types the name.
            _ if self.picker.as_ref().is_some_and(|p| p.name.is_some()) && key.text().is_some() => {
                if let Some(picker) = &mut self.picker {
                    picker.typing_name = true;
                }
                self.name_key(key, cx, now);
                return;
            }
            _ => return,
        };
        if moved {
            self.follow_cursor(now);
        }
        cx.redraw();
    }

    /// Saving: take a file's name as the name to save as.
    fn name_from(&mut self, path: &std::path::Path) {
        if let Some(picker) = &mut self.picker
            && let Some(field) = &mut picker.name
            && let Some(name) = path.file_name()
        {
            field.set_text(&name.to_string_lossy());
            field.select_stem();
            picker.typing_name = true;
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| hit.clone())
    }

    fn hit_rect(&self, wanted: &Hit) -> Option<enso_ui::Rect> {
        self.hits
            .iter()
            .find(|(_, hit)| hit == wanted)
            .map(|(rect, _)| *rect)
    }

    pub(super) fn pointer(&mut self, event: PointerEvent, cx: &mut Cx<'_>, now: Instant) {
        if let Some((menu, _)) = &mut self.menu {
            let response = menu.pointer(&event);
            // A press outside closes the menu and goes no further.
            if response != menu::Response::Ignored || matches!(event, PointerEvent::Press { .. }) {
                self.menu_response(response, cx);
                return;
            }
        }
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                if self.drag_band(x, y, now) {
                    cx.redraw();
                    return;
                }
                if self.sidebar.is_dragging() {
                    if self.sidebar.drag(x) {
                        cx.redraw();
                    }
                    return;
                }
                if self.scroll.is_dragging() {
                    if self.scroll.drag(self.list, y, now) {
                        cx.redraw();
                    }
                    return;
                }
                let hover = self.hit(x, y);
                cx.set_cursor(match hover {
                    Some(Hit::SidebarEdge) => Cursor::ColResize,
                    Some(Hit::Filter(_) | Hit::Name(_) | Hit::Location(_)) => Cursor::Text,
                    Some(
                        Hit::Crumb(_)
                        | Hit::Back
                        | Hit::Forward
                        | Hit::Place(_)
                        | Hit::Chip(_)
                        | Hit::Unchoose(_)
                        | Hit::FilterMenu
                        | Hit::Choice(_)
                        | Hit::Accept
                        | Hit::Cancel
                        | Hit::Replace
                        | Hit::KeepFile
                        | Hit::StopOp
                        | Hit::Resolve(_)
                        | Hit::ForAll
                        | Hit::DeleteForGood
                        | Hit::KeepFiles
                        | Hit::Branch
                        | Hit::Volume(_)
                        | Hit::Side(..)
                        | Hit::Eject(_)
                        | Hit::TrashPlace
                        | Hit::Recent
                        | Hit::Tabs(_)
                        | Hit::PaletteLine(_)
                        | Hit::Rename(_)
                        | Hit::Permission(_)
                        | Hit::EmptyTrash
                        | Hit::GitYes
                        | Hit::GitNo
                        | Hit::FormSave
                        | Hit::FormCancel
                        | Hit::Setting(_)
                        | Hit::SortPill(_)
                        | Hit::ViewPill(_),
                    ) => Cursor::Pointer,
                    Some(Hit::FormField(_)) => Cursor::Text,
                    _ => Cursor::Default,
                });
                if hover != self.hover {
                    self.hover = hover;
                    cx.redraw();
                }
            }
            PointerEvent::Leave => {
                if self.hover.take().is_some() {
                    cx.redraw();
                }
            }
            PointerEvent::Press { x, y, button } => {
                let clicks = self.clicks.press(x, y, button, now);
                self.press(x, y, button, clicks, cx, now);
                cx.redraw();
            }
            PointerEvent::Release { .. } => {
                if self.end_band() {
                    cx.redraw();
                }
                if self.sidebar.is_dragging() || self.scroll.is_dragging() {
                    self.sidebar.release();
                    self.scroll.release();
                    cx.redraw();
                }
            }
            PointerEvent::Scroll {
                x, y, dy, wheel, ..
            } => {
                if !self.list.contains(x, y) {
                    // The split's other side scrolls where it is.
                    if let Some(other) = self.split
                        && self.split_rect.contains(x, y)
                    {
                        self.with_tab(other, |files| files.wheel(dy, wheel, now));
                        cx.redraw();
                    }
                    return;
                }
                // Ctrl and the wheel: the grid's tiles grow and shrink.
                if cx.mods().ctrl {
                    if self.config.view == crate::config::View::Grid && dy.abs() > 0.0 {
                        self.zoom_by(if dy < 0.0 { 1 } else { -1 });
                        self.remember_zoom(cx);
                        self.follow_cursor(now);
                        cx.redraw();
                    }
                    return;
                }
                self.wheel(dy, wheel, now);
                cx.redraw();
            }
        }
    }

    /// The wheel over the list: scroll it.
    fn wheel(&mut self, dy: f32, wheel: bool, now: Instant) {
        let (_, height) = self.span_of(0);
        let step = if self.config.view == crate::config::View::Grid {
            height
        } else {
            WHEEL_ROWS * height
        };
        self.scroll
            .scroll(dy, wheel, step, self.transition(Transition::SELECT), now);
    }

    fn press(&mut self, x: f32, y: f32, button: Button, clicks: u8, cx: &mut Cx<'_>, now: Instant) {
        if self.help || self.peek {
            self.help = false;
            self.peek = false;
            return;
        }
        let hit = self.hit(x, y);
        // The split's other side comes in front; the press is then its.
        let hit = match hit {
            Some(Hit::Other(inner)) if self.settings.is_none() && self.vcs.is_none() => {
                self.other_side();
                Some(*inner)
            }
            hit => hit,
        };
        if self.palette_press(hit.as_ref(), cx)
            || self.renamer_press(hit.as_ref(), cx)
            || self.properties_press(hit.as_ref(), cx)
            || self.settings_press(hit.as_ref(), cx)
            || self.vcs_press(hit.as_ref(), cx)
            || self.ops_press(hit.as_ref(), cx)
        {
            return;
        }
        // A click away from the name being edited leaves it as it was.
        if self.renaming.is_some() {
            self.renaming = None;
        }
        // Clicking anywhere but the location puts it away.
        if self.location.is_some() && !matches!(hit, Some(Hit::Location(_))) {
            self.location = None;
        }
        // A question is open: only its answers count.
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.confirm.is_some())
        {
            match hit {
                Some(Hit::Replace) => {
                    if let Some(path) = self
                        .picker
                        .as_ref()
                        .and_then(|picker| picker.confirm.clone())
                    {
                        self.finish(vec![path], cx);
                    }
                }
                Some(Hit::KeepFile) | None => {
                    if let Some(picker) = &mut self.picker {
                        picker.confirm = None;
                    }
                }
                _ => {}
            }
            return;
        }
        if button == BUTTON_BACK {
            let request = self.pane.back();
            return self.load(request);
        }
        if button == BUTTON_FORWARD {
            let request = self.pane.forward();
            return self.load(request);
        }
        if button == Button::Right {
            match hit {
                Some(Hit::Row(row)) => {
                    if self.pane.set_cursor(row) {
                        self.follow_cursor(now);
                    }
                    self.context_menu(Some(row), x, y);
                }
                Some(Hit::Empty) => self.context_menu(None, x, y),
                Some(Hit::Place(path)) if self.bookmarks.iter().any(|mark| mark.path == path) => {
                    self.bookmark_menu(path, x, y);
                }
                _ => {}
            }
            return;
        }
        if button == Button::Middle {
            // A folder opens in a new tab; a tab closes.
            match hit {
                Some(Hit::Row(row)) if self.pane.entry(row).kind.is_dir() => {
                    let path = self.pane.path(row);
                    self.open_tab(path, false, cx);
                }
                Some(Hit::Tabs(tabs::TabHit::Tab(index))) => {
                    self.close_tab(index);
                }
                _ => {}
            }
            return;
        }
        let mods = cx.mods();
        match hit {
            // Unwrapped above; a nested one is nothing.
            Some(Hit::Other(_)) => {}
            // The overlays took their own presses above.
            Some(
                Hit::PaletteLine(_)
                | Hit::PaletteField
                | Hit::Rename(_)
                | Hit::Permission(_)
                | Hit::PropertiesCard,
            ) => {}
            Some(Hit::Tabs(tabs::TabHit::Tab(index))) => self.switch_tab(index),
            Some(Hit::Tabs(tabs::TabHit::Close(index))) => {
                self.close_tab(index);
            }
            Some(Hit::SidebarEdge) => {
                self.sidebar.press(x);
            }
            Some(Hit::ScrollBar) => {
                self.scroll
                    .press(self.list, x, y, self.transition(Transition::SELECT), now);
            }
            Some(Hit::Row(row)) => {
                if let Some(picker) = &mut self.picker {
                    picker.typing_name = false;
                }
                if self.pane.set_cursor(row) {
                    self.follow_cursor(now);
                }
                if mods.ctrl {
                    self.toggle(row);
                    self.anchor = Some(row);
                } else if mods.shift {
                    self.anchor = self.anchor.or(Some(row));
                    self.select_range();
                } else {
                    self.anchor = Some(row);
                    let is_dir = self.pane.entry(row).kind.is_dir();
                    if self.picker.as_ref().is_some_and(|p| p.request.is_save()) && !is_dir {
                        self.name_from(&self.pane.path(row));
                    } else {
                        // A plain click starts over in this folder; what was
                        // chosen elsewhere stays chosen.
                        self.selection.clear_in(self.pane.location());
                    }
                    if clicks >= 2 {
                        self.clicks.reset();
                        self.open(cx);
                    }
                }
            }
            Some(Hit::Empty) => {
                let single = self
                    .picker
                    .as_ref()
                    .is_some_and(|picker| !picker.request.multiple());
                if single {
                    self.selection.clear_in(self.pane.location());
                } else {
                    self.begin_band(x, y, mods.ctrl, now);
                }
            }
            Some(Hit::Crumb(path) | Hit::Place(path)) => self.navigate(path),
            Some(Hit::Back) => {
                let request = self.pane.back();
                self.load(request);
            }
            Some(Hit::Forward) => {
                let request = self.pane.forward();
                self.load(request);
            }
            Some(Hit::Filter(text_x)) => {
                self.open_filter(cx);
                let size = self.font(type_scale::BODY);
                if let Some(field) = &mut self.filter {
                    field.click(
                        |text| cx.measure(&Text::new(text, size, Rgba::TRANSPARENT)).w,
                        x - text_x,
                        mods.shift,
                    );
                    if clicks >= 2 {
                        field.select_word();
                    }
                }
            }
            Some(Hit::Location(text_x)) => {
                let size = self.font(type_scale::BODY);
                if let Some(field) = &mut self.location {
                    field.click(
                        |text| cx.measure(&Text::new(text, size, Rgba::TRANSPARENT)).w,
                        x - text_x,
                        mods.shift,
                    );
                    if clicks >= 2 {
                        field.select_word();
                    }
                }
            }
            Some(Hit::Name(text_x)) => {
                self.filter = None;
                let size = self.font(type_scale::BODY);
                if let Some(picker) = &mut self.picker {
                    picker.typing_name = true;
                    if let Some(field) = &mut picker.name {
                        field.click(
                            |text| cx.measure(&Text::new(text, size, Rgba::TRANSPARENT)).w,
                            x - text_x,
                            mods.shift,
                        );
                        if clicks >= 2 {
                            field.select_word();
                        }
                    }
                }
            }
            Some(Hit::Chip(path)) => self.reveal(&path),
            Some(Hit::Unchoose(path)) => self.selection.remove(&path),
            Some(Hit::FilterMenu) => self.filter_menu(),
            Some(Hit::Choice(index)) => self.choice_pressed(index),
            Some(Hit::Accept) => self.accept(Under::Nothing, cx),
            Some(Hit::Cancel) => self.cancel(cx),
            Some(Hit::Title) => {
                if clicks >= 2 {
                    self.clicks.reset();
                    cx.toggle_maximized();
                } else {
                    cx.begin_move();
                }
            }
            Some(Hit::Side(path, reveal)) => self.side_clicked(path, reveal),
            Some(Hit::Volume(index)) => self.open_volume(index, cx),
            Some(Hit::Eject(index)) => self.eject(index, cx),
            Some(Hit::TrashPlace) => self.open_trash(),
            Some(Hit::Recent) => self.open_recent(),
            Some(Hit::EmptyTrash) => self.ask_empty(cx),
            Some(Hit::Branch) => {
                if let Some(rect) = self.hit_rect(&Hit::Branch) {
                    self.branch_menu(rect);
                }
            }
            Some(
                Hit::Replace
                | Hit::KeepFile
                | Hit::StopOp
                | Hit::Resolve(_)
                | Hit::ForAll
                | Hit::DeleteForGood
                | Hit::KeepFiles
                | Hit::GitYes
                | Hit::GitNo
                | Hit::FormField(_)
                | Hit::FormSave
                | Hit::FormCancel
                | Hit::Setting(_)
                | Hit::SortPill(_)
                | Hit::ViewPill(_),
            )
            | None => {}
        }
    }

    fn filter_menu(&mut self) {
        let Some(picker) = &self.picker else { return };
        let Some(rect) = self.hit_rect(&Hit::FilterMenu) else {
            return;
        };
        let items = picker
            .request
            .filters
            .iter()
            .enumerate()
            .map(|(index, filter)| Item::new(&filter.name).checked(picker.filter == Some(index)))
            .collect();
        let menu =
            Menu::new(items, rect.x, rect.bottom()).with_selected(picker.filter.unwrap_or(0));
        self.menu = Some((menu, MenuFor::Filters));
    }

    fn choice_pressed(&mut self, index: usize) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.choices.get(index) {
            Some(Choice::Toggle { .. }) => picker.set_choice(index, None),
            Some(Choice::Options {
                options, selected, ..
            }) => {
                let Some(rect) = self
                    .hits
                    .iter()
                    .find(|(_, hit)| *hit == Hit::Choice(index))
                    .map(|(rect, _)| *rect)
                else {
                    return;
                };
                let current = options
                    .iter()
                    .position(|(id, _)| id == selected)
                    .unwrap_or(0);
                let items = options
                    .iter()
                    .map(|(id, label)| Item::new(label).checked(id == selected))
                    .collect();
                self.menu = Some((
                    Menu::new(items, rect.x, rect.bottom()).with_selected(current),
                    MenuFor::Choice(index),
                ));
            }
            None => {}
        }
    }
}

fn step(nav: Nav) -> isize {
    match nav {
        Nav::Up => -1,
        Nav::Down => 1,
    }
}
