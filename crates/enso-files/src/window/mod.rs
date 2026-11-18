//! The window: a title row with the way back, the path, and the filter; the
//! places down the side; the folder's entries in rows; and a status line.
//! As a picker it gains a bar at the bottom: what's chosen, the file types
//! on offer, the application's questions, and the buttons to finish.
//!
//! Layout is worked out while drawing ([`draw`]), and every place that
//! answers the pointer is written down then ([`Hit`]), so input
//! ([`input`]) asks "what is under me" of the last frame instead of laying
//! everything out again.

mod draw;
mod input;

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use enso_core::{
    Context, Icon,
    motion::{Transition, Tween},
    style::{icon, type_scale},
};
use enso_fs::{
    places::{self, UserDir, UserDirs},
    sort::By,
};
use enso_ui::{
    App, Canvas, Clicks, Cx, Event, Paint, Proxy, Rect, SurfaceSpec, WindowSpec,
    watch::watch_dir,
    widget::{Look, Menu, Scroll, Splitter, TextField},
};

use crate::{
    config::FilesConfig,
    jobs::{Loader, Message},
    pane::{Open, Pane, Request},
    pick,
    picker::{Accept, Outcome, Picker, Under},
    recall::{self, Recall},
    selection::Selection,
};

/// Something on screen that answers the pointer.
#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Row(usize),
    /// The list below its last row.
    Empty,
    /// A segment of the path: go there.
    Crumb(PathBuf),
    Back,
    Forward,
    /// Empty title row: grab it to move the window.
    Title,
    /// The filter field, whose text starts at this `x`.
    Filter(f32),
    /// The location typed as text, whose text starts at this `x`.
    Location(f32),
    Place(PathBuf),
    /// The edge between the sidebar and the list.
    SidebarEdge,
    ScrollBar,
    /// A chosen file in the picker's bar: show it.
    Chip(PathBuf),
    /// Its ✕: unchoose it.
    Unchoose(PathBuf),
    /// The name being saved as, whose text starts at this `x`.
    Name(f32),
    FilterMenu,
    Choice(usize),
    Accept,
    Cancel,
    /// The question about replacing a file: yes, or no.
    Replace,
    KeepFile,
}

/// What a menu entry does.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Open,
    Toggle(PathBuf),
    CopyPath(PathBuf),
    SelectAll,
    Unselect,
    Sort(By),
    Descending,
    FoldersFirst,
    Hidden,
    Reload,
}

/// What an open menu is for.
#[derive(Clone, Debug)]
enum MenuFor {
    /// A right-click: each item's action.
    Context(Vec<Option<Action>>),
    Filters,
    Choice(usize),
}

/// A place in the sidebar.
#[derive(Clone, Debug)]
struct Place {
    label: String,
    icon: Icon,
    path: PathBuf,
}

const SIDEBAR: f32 = 200.0;

pub struct Files {
    ctx: Context,
    config: FilesConfig,
    pane: Pane,
    /// The first folder's request, until there is a proxy to load it with.
    pending: Option<Request>,
    loader: Loader,
    proxy: Option<Proxy<Message>>,
    size: (f32, f32),
    scroll: Scroll,
    /// Where the cursor's highlight is drawn, gliding between rows.
    highlight: Tween<f32>,
    hover: Option<Hit>,
    hits: Vec<(Rect, Hit)>,
    clicks: Clicks,
    /// The list's rectangle in the last frame.
    list: Rect,
    /// `g` was pressed: the next key completes a `g` command.
    pending_g: bool,
    /// The filter-as-you-type field, while it is open.
    filter: Option<TextField>,
    maximized: bool,
    home: PathBuf,
    selection: Selection,
    /// Where a Shift-selection starts, as a row of this folder.
    anchor: Option<usize>,
    sidebar: Splitter,
    sidebar_open: bool,
    places: Vec<Place>,
    picker: Option<Picker>,
    menu: Option<(Menu, MenuFor)>,
    /// The location as text (`Ctrl+L`), while it's being typed, and
    /// whether Enter found nothing there.
    location: Option<TextField>,
    location_missing: bool,
    /// The keys, shown over everything (`?`, `F1`).
    help: bool,
    /// Following the folder shown, so it updates as it changes.
    watch: Option<enso_fs::watch::Watch>,
}

impl Files {
    /// The file manager, opened on `location`.
    pub fn new(ctx: Context, config: FilesConfig, location: PathBuf) -> Self {
        let (pane, request) = Pane::new(location, config.sort(), config.hidden);
        let home = places::home();
        Self {
            ctx,
            config,
            pane,
            pending: Some(request),
            loader: Loader::default(),
            proxy: None,
            size: (960.0, 640.0),
            scroll: Scroll::default(),
            highlight: Tween::new(0.0),
            hover: None,
            hits: Vec::new(),
            clicks: Clicks::default(),
            list: Rect::default(),
            pending_g: false,
            filter: None,
            maximized: false,
            places: places_for(&home, &UserDirs::load()),
            home,
            selection: Selection::default(),
            anchor: None,
            sidebar: Splitter::new(SIDEBAR, 150.0, 320.0),
            sidebar_open: true,
            picker: None,
            menu: None,
            location: None,
            location_missing: false,
            help: false,
            watch: None,
        }
    }

    /// The window as a picker, answering `request` into `outcome`.
    ///
    /// It opens where this application last picked from; else where the
    /// application asked; else in Downloads (Documents, to save).
    pub fn picker(
        ctx: Context,
        config: FilesConfig,
        mut request: pick::Request,
        outcome: Outcome,
    ) -> Self {
        let recall = Recall::load(&ctx.paths.state_dir.join(recall::FILE));
        if let Some(folder) = recall.folder(&request.app_id) {
            request.folder = Some(folder.to_owned());
        }
        let dirs = UserDirs::load();
        let usual = if request.is_save() {
            UserDir::Documents
        } else {
            UserDir::Downloads
        };
        let fallback = dirs
            .get(usual)
            .filter(|dir| dir.is_dir())
            .map_or_else(places::home, Path::to_owned);
        let start = request.start(&fallback);
        let mut files = Self::new(ctx, config, start);
        files.size = (900.0, 600.0);
        let picker = Picker::new(request, outcome);
        files.pane.set_keep(picker.keep());
        files.picker = Some(picker);
        files
    }

    fn row_height(&self) -> f32 {
        self.config.density.row_height()
    }

    fn font(&self, scale: f32) -> f32 {
        self.ctx.config.appearance.font_size * scale
    }

    fn transition(&self, transition: Transition) -> Transition {
        self.ctx.motion.apply(transition)
    }

    fn look(&self) -> Look {
        Look::new(
            &self.ctx.theme.colors,
            self.ctx.accent(),
            self.font(type_scale::BODY),
        )
    }

    // Moving about.

    fn load(&mut self, request: Option<Request>) {
        let Some(request) = request else { return };
        // A new folder starts at its top; the highlight doesn't glide in
        // from where the last one's was.
        self.scroll.to(0.0, None, Instant::now());
        self.highlight.jump(0.0);
        self.hover = None;
        self.anchor = None;
        // The pane starts the new folder unfiltered; so does the field.
        self.filter = None;
        match &self.proxy {
            Some(proxy) => self.loader.load(request, proxy),
            None => self.pending = Some(request),
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        let request = self.pane.navigate(path);
        self.load(request);
    }

    /// Go to the folder `path` is in, with the cursor on it.
    fn reveal(&mut self, path: &Path) {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        let request = self
            .pane
            .navigate_to(folder.to_owned(), name.as_encoded_bytes());
        self.load(request);
    }

    /// What is under the cursor, for a picker deciding what Enter means.
    fn under(&self) -> Under {
        match self.pane.open() {
            Some(Open::Dir(path)) => Under::Dir(path),
            Some(Open::File(path)) => Under::File(path),
            None => Under::Nothing,
        }
    }

    /// Enter, or a double-click: go into a folder; open a file, or, in a
    /// picker, choose it.
    fn open(&mut self, cx: &mut Cx<'_>) {
        match (self.pane.open(), self.picker.is_some()) {
            (Some(Open::Dir(path)), _) => self.navigate(path),
            (Some(Open::File(path)), false) => {
                if let Err(err) = enso_fs::open::open(&path, "alacritty") {
                    eprintln!("enso-files: cannot open {}: {err}", path.display());
                }
            }
            (Some(Open::File(path)), true) => {
                let under = Under::File(path);
                self.accept(under, cx);
            }
            (None, _) => {}
        }
    }

    /// Accept in a picker, with `under` the pointer or cursor (or
    /// [`Under::Nothing`] for the button).
    fn accept(&mut self, under: Under, cx: &mut Cx<'_>) {
        let Some(picker) = &self.picker else { return };
        // Choosing a file by double-click or Enter adds it to what's
        // chosen; with a selection, Enter means the selection.
        let mut selection = self.selection.clone();
        if let Under::File(path) = &under
            && picker.request.multiple()
            && !selection.is_empty()
        {
            selection.insert(path.clone());
        }
        match picker.accept(self.pane.location(), &selection, &under) {
            Accept::Answer(paths) => self.finish(paths, cx),
            Accept::Enter(path) => {
                // Typing a folder's name went there; the name to save as
                // goes back to the one suggested.
                if let Some(picker) = &mut self.picker
                    && let (pick::Mode::Save { name: suggested }, Some(field)) =
                        (&picker.request.mode, &mut picker.name)
                {
                    field.set_text(suggested);
                    field.select_stem();
                }
                self.navigate(path);
            }
            Accept::Confirm(path) => {
                if let Some(picker) = &mut self.picker {
                    picker.confirm = Some(path);
                }
            }
            Accept::Nothing => {}
        }
        cx.redraw();
    }

    /// Hand back `paths` and close, remembering where they were chosen.
    fn finish(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_>) {
        let Some(picker) = &self.picker else { return };
        let app = picker.request.app_id.clone();
        picker.answer(paths);
        if !app.is_empty() {
            let file = self.ctx.paths.state_dir.join(recall::FILE);
            let mut recall = Recall::load(&file);
            recall.remember(&app, self.pane.location());
            if let Err(err) = recall.save(&file) {
                eprintln!("enso-files: cannot remember the folder: {err}");
            }
        }
        cx.exit();
    }

    /// Close without choosing anything.
    fn cancel(&mut self, cx: &mut Cx<'_>) {
        cx.exit();
    }

    /// After the cursor moved: glide the highlight there, and scroll just
    /// enough to keep it in view.
    fn follow_cursor(&mut self, now: Instant) {
        let Some(row) = self.pane.cursor() else {
            return;
        };
        let row_height = self.row_height();
        let top = row as f32 * row_height;
        self.highlight
            .set(top, self.transition(Transition::SELECT), now);
        self.scroll
            .reveal(top, row_height, self.transition(Transition::SELECT), now);
    }

    fn visible_rows(&self) -> isize {
        ((self.list.h / self.row_height()).floor() as isize).max(1)
    }

    /// Choose `row`, or unchoose it. A picker that takes one thing chooses
    /// only it; a picker of files doesn't choose folders, nor one of
    /// folders files.
    fn toggle(&mut self, row: usize) {
        let path = self.pane.path(row);
        let is_dir = self.pane.entry(row).kind.is_dir();
        if let Some(picker) = &self.picker {
            if picker.request.is_save() || picker.request.directory() != is_dir {
                return;
            }
            if !picker.request.multiple() {
                if self.selection.contains(&path) {
                    self.selection.clear();
                } else {
                    self.selection.only(path);
                }
                return;
            }
        }
        self.selection.toggle(path);
    }

    /// Choose the rows from the anchor to the cursor, and nothing else in
    /// this folder.
    fn select_range(&mut self) {
        let (Some(anchor), Some(cursor)) = (self.anchor, self.pane.cursor()) else {
            return;
        };
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| !picker.request.multiple())
        {
            return;
        }
        self.selection.clear_in(self.pane.location());
        for row in anchor.min(cursor)..=anchor.max(cursor) {
            if self.can_choose(row) {
                self.selection.insert(self.pane.path(row));
            }
        }
    }

    fn can_choose(&self, row: usize) -> bool {
        let is_dir = self.pane.entry(row).kind.is_dir();
        match &self.picker {
            None => true,
            Some(picker) => !picker.request.is_save() && picker.request.directory() == is_dir,
        }
    }

    fn select_all(&mut self) {
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| !picker.request.multiple())
        {
            return;
        }
        for row in 0..self.pane.len() {
            if self.can_choose(row) {
                self.selection.insert(self.pane.path(row));
            }
        }
    }
}

impl Files {
    /// Watch the folder just listed, so it updates as it changes.
    fn follow(&mut self, generation: u64) {
        // The old watch stops as it's replaced.
        self.watch = None;
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        self.watch = enso_fs::watch::Watch::start(self.pane.location(), move |gone| {
            proxy.send(Message::Changed { generation, gone });
        })
        .ok();
    }
}

/// The sidebar's places: home, the user folders that exist, the root.
fn places_for(home: &Path, dirs: &UserDirs) -> Vec<Place> {
    let mut places = vec![Place {
        label: "Home".to_owned(),
        icon: icon::FOLDER,
        path: home.to_owned(),
    }];
    for (dir, path) in dirs.existing() {
        // The desktop folder means little on a desktop without icons.
        if dir == UserDir::Desktop {
            continue;
        }
        places.push(Place {
            label: path.file_name().map_or_else(
                || dir.label().to_owned(),
                |name| name.to_string_lossy().into_owned(),
            ),
            icon: icon::FOLDER,
            path: path.to_owned(),
        });
    }
    places.push(Place {
        label: "Computer".to_owned(),
        icon: icon::FOLDER,
        path: PathBuf::from("/"),
    });
    places
}

impl App for Files {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        let (title, parent) = match &self.picker {
            Some(picker) => (picker.request.title(), picker.request.parent.clone()),
            None => (draw::title(self.pane.location(), &self.home), None),
        };
        SurfaceSpec::window(
            WindowSpec {
                // Pickers are dialogs: their own id lets the compositor float
                // them (sumi's niri config does).
                app_id: if self.picker.is_some() {
                    "enso-files-picker"
                } else {
                    "enso-files"
                }
                .to_owned(),
                title,
                min_size: (460, 320),
                parent,
            },
            self.size.0 as u32,
            self.size.1 as u32,
        )
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Some(request) = self.pending.take() {
            self.loader.load(request, &proxy);
        }
        let theme = proxy.clone();
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            theme.send(Message::ThemeChanged)
        }) {
            eprintln!("enso-files: live theme switching unavailable: {err}");
        }
        self.proxy = Some(proxy);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let location = self.pane.location().to_owned();
        match event {
            Event::Message(Message::Listed { generation, result }) => {
                if self.pane.set_listing(generation, result) {
                    self.follow(generation);
                    // Landing on a remembered row: be there, don't glide.
                    if let Some(row) = self.pane.cursor() {
                        let top = row as f32 * self.row_height();
                        self.highlight.jump(top);
                        self.scroll
                            .set_extent(self.pane.len() as f32 * self.row_height(), self.list.h);
                        self.scroll
                            .reveal(top, self.row_height(), Transition::INSTANT, now);
                    }
                    cx.redraw();
                }
            }
            Event::Message(Message::Stats {
                generation,
                start,
                metas,
            }) => {
                if self.pane.apply_stats(generation, start, &metas) {
                    cx.redraw();
                }
            }
            Event::Message(Message::Changed { generation, gone }) => {
                if generation == self.pane.generation() {
                    if gone {
                        self.watch = None;
                        // Up to the nearest folder that's still there.
                        let mut up = self.pane.location().to_owned();
                        while !up.is_dir() && up.pop() {}
                        self.navigate(up);
                    } else {
                        let request = self.pane.reload();
                        if let Some(proxy) = &self.proxy {
                            self.loader.load(request, proxy);
                        }
                    }
                }
            }
            Event::Message(Message::ThemeChanged) => {
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Key(key) => self.key(key, cx, now),
            Event::Pointer(pointer) => self.pointer(pointer, cx, now),
            Event::Paste { data, .. } => self.paste(&String::from_utf8_lossy(&data), now, cx),
            Event::Resized { width, height } => {
                self.size = (width as f32, height as f32);
                cx.redraw();
            }
            Event::WindowState { maximized, .. } => {
                self.maximized = maximized;
                cx.redraw();
            }
            Event::Close => self.cancel(cx),
            _ => {}
        }
        if self.picker.is_none() && self.pane.location() != location {
            cx.set_title(&draw::title(self.pane.location(), &self.home));
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.draw_window(canvas, now)
    }
}
