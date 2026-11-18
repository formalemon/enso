//! The window: a title row with the way back, the path, and the filter; the
//! places down the side; the folder's entries in rows; and a status line.
//! As a picker it gains a bar at the bottom: what's chosen, the file types
//! on offer, the application's questions, and the buttons to finish.
//!
//! Layout is worked out while drawing ([`draw`]), and every place that
//! answers the pointer is written down then ([`Hit`]), so input
//! ([`input`]) asks "what is under me" of the last frame instead of laying
//! everything out again.

mod accounts;
mod archives;
mod band;
mod columns;
mod dnd;
mod draw;
mod grid;
mod input;
mod machine;
mod ops;
mod overlay;
mod palette;
mod preview;
mod properties;
mod renamer;
mod seek;
mod settings;
mod sidebar;
mod tabs;
#[cfg(test)]
mod tests;
mod vcs;

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
    widget::{Look, Scroll, Splitter, TextField},
};

use crate::{
    config::FilesConfig,
    jobs::{Loader, Message},
    pane::{Open, Pane, Request, Source},
    pick,
    picker::{Accept, Outcome, Picker, Under},
    recall::{self, Recall},
    selection::Selection,
};
use overlay::Overlay;

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
    /// Stop the operation under way.
    StopOp,
    /// An answer to "this name is taken".
    Resolve(enso_fs::ops::Resolve),
    /// "Do the same for the rest."
    ForAll,
    /// The answers to "delete for good?".
    DeleteForGood,
    KeepFiles,
    /// The branch pill.
    Branch,
    /// "Use version control?": yes, or not now.
    GitYes,
    GitNo,
    /// A form's field (by index), and its buttons.
    FormField(usize),
    /// One of a form's choices: which forge.
    FormChoice(usize),
    FormSave,
    FormCancel,
    /// A drive, and its ⏏.
    Volume(usize),
    Eject(usize),
    TrashPlace,
    EmptyTrash,
    /// The sidebar's Recent.
    Recent,
    /// An entry of a side column: go to it, or (`true`) show it chosen.
    Side(PathBuf, bool),
    /// A line of the settings panel, and its sort choices.
    Setting(usize),
    SortPill(crate::config::SortBy),
    ViewPill(crate::config::View),
    /// The tab strip.
    Tabs(tabs::TabHit),
    /// The palette: a line of it, and the rest of its card.
    PaletteLine(usize),
    PaletteField,
    /// Renaming many.
    Rename(renamer::RenameHit),
    /// The Compress… question.
    Pack(archives::PackHit),
    /// A permission box (its bit), and the rest of the properties card.
    Permission(u32),
    PropertiesCard,
    /// Something on the split's other side: it comes in front first.
    Other(Box<Hit>),
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
    Rename,
    NewFolder,
    Copy,
    Cut,
    Paste,
    Duplicate,
    Trash,
    Delete,
    Terminal,
    Undo,
    InitRepo,
    Settings,
    Restore,
    EmptyTrash,
    OpenWith(PathBuf),
    /// Start a program, in its folder.
    Run(PathBuf),
    /// Go to where a link leads.
    RealPath(PathBuf),
    /// Take everything out of this archive, beside it.
    Extract(PathBuf),
    /// Ask for a repository's address, to clone here.
    Clone,
    Compress,
    /// Go to a file's folder, with the cursor on it.
    Reveal(PathBuf),
    NewTab(PathBuf),
    /// Bookmark a folder, or stop.
    Bookmark(PathBuf),
    /// Move a bookmark up or down the list.
    ShiftBookmark(PathBuf, isize),
}

/// What an open menu is for.
#[derive(Clone, Debug)]
enum MenuFor {
    /// A right-click: each item's action.
    Context(Vec<Option<Action>>),
    Filters,
    Choice(usize),
    /// The branch menu: what each line does (`None` for a separator).
    Branches(Vec<Option<vcs::BranchLine>>),
    /// The applications that open this file, then "Always use…".
    OpenWith(PathBuf, Vec<enso_fs::open::Choice>),
    /// The same applications, to make one the default for the file's type.
    SetDefault(PathBuf, Vec<enso_fs::open::Choice>),
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
    /// The settings in effect: `base_config` with `overrides` over it.
    config: FilesConfig,
    /// `config.toml`'s `[files]`.
    base_config: FilesConfig,
    overrides: crate::settings::Overrides,
    /// The repository the folder is in, and how it stands.
    repo: Option<(PathBuf, enso_fs::git::Status)>,
    /// Whether "use version control?" was considered this session.
    asked_git: bool,
    git_facts: vcs::GitFacts,
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
    /// What's over the window, if anything: one thing at a time.
    overlay: Option<Overlay>,
    /// The time, the disks, the trash: asked of the machine.
    machine: machine::Machine,
    /// The location as text (`Ctrl+L`), while it's being typed, and
    /// whether Enter found nothing there.
    location: Option<TextField>,
    location_missing: bool,
    /// Following the folder shown, so it updates as it changes.
    watch: Option<enso_fs::watch::Watch>,
    running: Option<ops::Running>,
    renaming: Option<ops::Renaming>,
    /// A new folder to name as soon as it's listed.
    rename_after: Option<PathBuf>,
    undo: Vec<enso_fs::ops::Done>,
    redo: Vec<enso_fs::ops::Done>,
    toast: Option<(String, Instant)>,
    /// The drives udisks knows, and the one being mounted.
    volumes: Vec<enso_sys::udisks::Volume>,
    mounting: Option<String>,
    /// Where the last menu was, for one that follows it.
    menu_at: (f32, f32),
    /// The grid's tile size, as an index into its sizes.
    zoom: usize,
    thumbs: grid::Thumbs,
    thumbnailer: Option<crate::jobs::Thumbnailer>,
    /// A box being dragged to choose with.
    band: Option<band::Band>,
    /// A new folder's arrival, `0 → 1`, and from which side: `1` from the
    /// right (going in), `-1` from the left (going out), `0` not at all.
    arrival: Tween<f32>,
    sliding: f32,
    /// A navigation is on its way: its listing slides in. (A reload
    /// doesn't.)
    slide_pending: bool,
    /// The columns view's side columns.
    side_parent: columns::Side,
    side_child: columns::Side,
    /// The preview pane.
    previewing: bool,
    shown: preview::Shown,
    previewer: Option<crate::jobs::Previewer>,
    /// The tabs, in order: `None` is the one in front, whose folder is the
    /// window's own; the rest are parked.
    tabs: Vec<Option<tabs::Tab>>,
    active: usize,
    /// The tab on the split's other side, and where it was drawn.
    split: Option<usize>,
    split_rect: Rect,
    /// How each folder is shown, where that was changed.
    folder_views: crate::views::FolderViews,
    tallies: u64,
    /// Where a press on a row was, while it may become a drag.
    drag_from: Option<(f32, f32)>,
    /// Where something dragged over the window would land, and what to
    /// light up (`None`: the list).
    drop_at: Option<(PathBuf, Option<Rect>)>,
    /// Your GitHub repositories, for the clone form, and the one picked.
    repos: Vec<crate::forge::Repo>,
    repo_pick: usize,
    /// Signing in to GitHub: set to stop waiting.
    signing_in: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Files opened out of archives, for a fresh folder each.
    opened: u64,
    /// The field searching instead of filtering, and when the search
    /// typed is due to run.
    seek: Option<seek::Seek>,
    seek_due: Option<Instant>,
    /// The bookmarked folders, as GTK's file has them.
    bookmarks: Vec<enso_fs::bookmarks::Bookmark>,
    /// Drawing the split's other side.
    behind: bool,
    /// The folder whose listing is on screen, which the pane has left
    /// once it's asked for another.
    shown_at: PathBuf,
}

impl Files {
    /// The file manager, opened on `location`.
    pub fn new(ctx: Context, base_config: FilesConfig, location: PathBuf) -> Self {
        let overrides =
            crate::settings::Overrides::load(&ctx.paths.state_dir.join(crate::settings::FILE));
        let config = overrides.over(&base_config);
        let folder_views =
            crate::views::FolderViews::load(&ctx.paths.state_dir.join(crate::views::FILE));
        let start = location.clone();
        let (pane, request) = Pane::new(location, config.sort(), config.hidden);
        let home = places::home();
        let mut files = Self {
            ctx,
            config,
            base_config,
            overrides,
            repo: None,
            asked_git: false,
            git_facts: vcs::GitFacts::default(),
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
            overlay: None,
            machine: machine::Machine::real(),
            location: None,
            location_missing: false,
            watch: None,
            running: None,
            renaming: None,
            rename_after: None,
            undo: Vec::new(),
            redo: Vec::new(),
            toast: None,
            volumes: Vec::new(),
            mounting: None,
            menu_at: (0.0, 0.0),
            zoom: grid::ZOOM,
            thumbs: grid::Thumbs::default(),
            thumbnailer: None,
            band: None,
            arrival: Tween::new(1.0),
            sliding: 0.0,
            slide_pending: false,
            side_parent: columns::Side::default(),
            side_child: columns::Side::default(),
            previewing: false,
            shown: preview::Shown::default(),
            previewer: None,
            tabs: vec![None],
            active: 0,
            split: None,
            split_rect: Rect::default(),
            drag_from: None,
            drop_at: None,
            signing_in: None,
            repos: Vec::new(),
            repo_pick: 0,
            opened: 0,
            tallies: 0,
            folder_views,
            seek: None,
            seek_due: None,
            bookmarks: enso_fs::bookmarks::Bookmarks::load(&enso_fs::bookmarks::file()).folders(),
            behind: false,
            shown_at: PathBuf::new(),
        };
        // The first folder shown its own way, too.
        files.apply_folder_view(&start);
        files
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
        // Into a folder below: from the right; out to one above: from the
        // left; elsewhere, or reloading, or a gathered list: no slide. (The
        // pane has moved on already: what's on screen is `shown_at`.)
        let from = self.shown_at.as_path();
        self.sliding = if request.path == from || !request.source.is_folder() {
            0.0
        } else if request.path.starts_with(from) {
            1.0
        } else if from.starts_with(&request.path) {
            -1.0
        } else {
            0.0
        };
        self.slide_pending = true;
        // A folder arrives shown its own way.
        if request.source.is_folder() && request.path != self.shown_at {
            self.apply_folder_view(&request.path);
        }
        // Thumbnails and previews are for what's shown.
        self.thumbs.keep_only(&request.path);
        self.shown = preview::Shown::default();
        if let Some(thumbnailer) = &self.thumbnailer {
            thumbnailer.clear();
        }
        match &self.proxy {
            Some(proxy) => self.loader.load(request, proxy),
            None => self.pending = Some(request),
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        // Into an archive, or a folder inside one: its insides.
        if let Some((archive, inner)) = archives::inside_archive(&path) {
            let request = self.pane.show(archive, Source::Archive { inner });
            return self.load(request);
        }
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

    /// Show these selected, the cursor on the first: "show in folder".
    pub fn preselect(&mut self, paths: &[PathBuf]) {
        let Some(first) = paths.first() else { return };
        for path in paths.iter().filter(|path| path.parent() == first.parent()) {
            self.selection.insert(path.clone());
        }
        if let (Some(folder), Some(name)) = (first.parent(), first.file_name()) {
            self.pending = self
                .pane
                .navigate_to(folder.to_owned(), name.as_encoded_bytes())
                .or(self.pending.take());
        }
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
            // A file inside an archive: a copy, taken out.
            (Some(Open::File(path)), false)
                if matches!(self.pane.source(), Source::Archive { .. }) =>
            {
                self.open_member(&path);
            }
            // An archive: inside it.
            (Some(Open::File(path)), false)
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| enso_fs::archive::Format::of(name).is_some()) =>
            {
                self.navigate(path);
            }
            (Some(Open::File(path)), false) => {
                let opened =
                    enso_fs::open::open(&path, &self.ctx.config.appearance.terminal_program());
                if opened.is_ok() {
                    used(vec![path.clone()]);
                }
                if let Err(err) = opened {
                    let name = path
                        .file_name()
                        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
                    // A program is never run by opening it: that's Run's.
                    let hint = if is_program(&path) {
                        " · it's a program: Run is in its menu"
                    } else {
                        ""
                    };
                    self.toast(format!("Can't open “{name}”: {err}{hint}"), cx);
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
            Accept::Confirm(path) => self.overlay = Some(Overlay::Replace(path)),
            Accept::Nothing => {}
        }
        cx.redraw();
    }

    /// Hand back `paths` and close, remembering where they were chosen.
    fn finish(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_>) {
        let Some(picker) = &self.picker else { return };
        let app = picker.request.app_id.clone();
        used(paths.clone());
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
        let (top, height) = self.span_of(row);
        self.highlight
            .set(top, self.transition(Transition::SELECT), now);
        self.scroll
            .reveal(top, height, self.transition(Transition::SELECT), now);
    }

    /// How many entries a page down moves: rows of the list, or of the grid.
    fn visible_rows(&self) -> isize {
        let (_, height) = self.span_of(0);
        let rows = ((self.list.h / height).floor() as isize).max(1);
        match self.config.view {
            crate::config::View::List | crate::config::View::Columns => rows,
            crate::config::View::Grid => rows * self.columns() as isize,
        }
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
    /// A listing arrived for the tab in front. Returns whether it was the
    /// one waited for.
    fn listed_now(
        &mut self,
        generation: u64,
        result: Result<enso_fs::list::Listing, String>,
        now: Instant,
    ) -> bool {
        if !self.pane.set_listing(generation, result) {
            return false;
        }
        self.follow(generation);
        self.listed();
        // Arriving somewhere new (not a reload): a visit, for the palette.
        if self.pane.source().is_folder() && self.shown_at != self.pane.location() {
            self.shown_at = self.pane.location().to_owned();
            self.visited(self.shown_at.clone());
        }
        if !self.pane.source().is_folder() {
            self.shown_at = PathBuf::new();
        }
        // The side columns read again, in case they changed too.
        self.side_parent = columns::Side::default();
        self.side_child = columns::Side::default();
        // A new folder arrives sliding the way we went.
        if std::mem::take(&mut self.slide_pending) && self.sliding != 0.0 {
            self.arrival.jump(0.0);
            self.arrival
                .set(1.0, self.transition(Transition::SELECT), now);
        }
        self.refresh_git(generation);
        self.maybe_ask_git();
        // Landing on a remembered row: be there, don't glide.
        if let Some(row) = self.pane.cursor() {
            let (top, height) = self.span_of(row);
            self.highlight.jump(top);
            let (_, last) = self.span_of(self.pane.len().saturating_sub(1));
            self.scroll.set_extent(last + height, self.list.h);
            self.scroll.reveal(top, height, Transition::INSTANT, now);
        }
        true
    }

    /// The folder in front changed on disk, or went away.
    fn changed(&mut self, gone: bool) {
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

    /// Note a visit to `folder`, off the drawing thread.
    fn visited(&self, folder: PathBuf) {
        let file = self.ctx.paths.state_dir.join(crate::frecent::FILE);
        // Tests leave no trace.
        if !file.starts_with("/nonexistent") {
            let _ = std::thread::Builder::new()
                .name("enso-files-visit".to_owned())
                .spawn(move || {
                    let now = jiff::Timestamp::now().as_second();
                    let mut visits = crate::frecent::Visits::load(&file);
                    visits.visit(&folder, now);
                    let _ = visits.save(&file);
                });
        }
    }

    /// Watch the folder just listed, so it updates as it changes.
    fn follow(&mut self, generation: u64) {
        // The old watch stops as it's replaced. A gathered list isn't
        // followed: a change would run the search again under the cursor.
        self.watch = None;
        if !self.pane.source().is_folder() {
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        self.watch = enso_fs::watch::Watch::start(self.pane.location(), move |gone| {
            proxy.send(Message::Changed { generation, gone });
        })
        .ok();
    }
}

/// Note that these files were just used, in the list every program shares
/// (Recent), off the drawing thread.
pub(super) fn used(paths: Vec<PathBuf>) {
    let _ = std::thread::Builder::new()
        .name("enso-files-recent".to_owned())
        .spawn(move || {
            let mime = enso_fs::mime::MimeDb::load();
            let file = enso_fs::recent::file();
            for path in paths {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
                let kind = name
                    .as_deref()
                    .and_then(|name| mime.by_name(name))
                    .unwrap_or("application/octet-stream");
                if let Err(err) = enso_fs::recent::add(&file, &path, kind) {
                    eprintln!("enso-files: cannot note {} as used: {err}", path.display());
                }
            }
        });
}

/// Whether `path` is a file that can be run: executable by someone, and
/// not a folder.
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The sidebar's places: home, the user folders that exist, the root.
fn places_for(home: &Path, dirs: &UserDirs) -> Vec<Place> {
    let mut places = vec![Place {
        label: "Home".to_owned(),
        icon: icon::HOME,
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
                // Files, from here or anywhere.
                drops: vec!["text/uri-list".to_owned()],
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
        self.thumbnailer = Some(crate::jobs::Thumbnailer::start(&proxy));
        self.previewer = Some(crate::jobs::Previewer::start(&proxy));
        self.proxy = Some(proxy);
        self.watch_drives();
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let location = self.pane.location().to_owned();
        match event {
            Event::Message(Message::Listed {
                generation,
                result,
                notes,
            }) => {
                // Whichever tab asked for it, in front or not.
                if let Some(tab) = self.tab_of(generation)
                    && self.with_tab(tab, |files| {
                        if files.pane.generation() == generation {
                            files.pane.set_notes(notes);
                        }
                        files.listed_now(generation, result, now)
                    })
                {
                    cx.redraw();
                }
            }
            Event::Message(Message::Stats {
                generation,
                start,
                metas,
            }) => {
                if let Some(tab) = self.tab_of(generation)
                    && self.with_tab(tab, |files| {
                        files.pane.apply_stats(generation, start, &metas)
                    })
                {
                    cx.redraw();
                }
            }
            Event::Message(Message::Changed { generation, gone }) => {
                if let Some(tab) = self.tab_of(generation) {
                    self.with_tab(tab, |files| files.changed(gone));
                }
            }
            Event::Message(Message::Git { generation, status }) => {
                if let Some(tab) = self.tab_of(generation) {
                    self.with_tab(tab, |files| files.git_arrived(generation, status, cx));
                }
            }
            Event::Message(Message::Preview { path, preview }) => {
                self.preview_arrived(path, preview);
                cx.redraw();
            }
            Event::Message(Message::Thumb { path, thumb }) => {
                let mut tabs = path
                    .parent()
                    .map(|folder| self.tabs_at(folder))
                    .unwrap_or_default();
                // A gathered list's thumbnails are from all over: the tab
                // in front asked for them.
                if tabs.is_empty() && !self.pane.source().is_folder() {
                    tabs.push(self.active);
                }
                for tab in tabs {
                    let (path, thumb) = (path.clone(), thumb.clone());
                    self.with_tab(tab, |files| files.thumbs.arrived(path, thumb));
                    cx.redraw();
                }
            }
            Event::Message(Message::Edited(result)) => self.edited(result, cx),
            Event::Message(Message::Toast(text)) => self.toast(text, cx),
            Event::Message(Message::Device(code, page)) => self.device_arrived(code, page, cx),
            Event::Message(Message::SignedIn(site, result)) => self.signed_in(site, result, cx),
            Event::Message(Message::Repos(site, result)) => {
                match result {
                    // Each forge's in place of what it said before.
                    Ok(repos) => {
                        self.repos.retain(|repo| repo.forge != site.name());
                        self.repos.extend(repos);
                    }
                    Err(why) => self.toast(why, cx),
                }
                cx.redraw();
            }
            Event::Message(Message::Remote { said, result }) => self.remote_done(said, result, cx),
            Event::DragOver { x, y } => self.drag_over(x, y, cx),
            Event::DragLeave => self.drag_leave(cx),
            Event::Drop {
                data, x, y, moving, ..
            } => self.dropped(&data, x, y, moving, cx),
            Event::Message(Message::Tallied { token, tally, done }) => {
                self.tallied(token, tally, done);
                cx.redraw();
            }
            Event::Message(Message::Volumes(volumes)) => {
                self.volumes = volumes;
                cx.redraw();
            }
            Event::Message(Message::Mounted(result)) => self.mounted(result, cx),
            Event::Message(Message::Ejected(result)) => {
                let text = result.unwrap_or_else(|why| why);
                self.toast(text, cx);
            }
            Event::Message(Message::Progress(progress)) => self.op_progress(progress, cx),
            Event::Message(Message::Conflict {
                source,
                target,
                reply,
            }) => self.ask(source, target, reply, cx),
            Event::Message(Message::Finished { op, report }) => self.op_finished(op, report, cx),
            Event::Wake => self.wake(cx),
            Event::Message(Message::ThemeChanged) => {
                // The state directory changed: the theme, or the settings
                // (from another window).
                let overrides = crate::settings::Overrides::load(
                    &self.ctx.paths.state_dir.join(crate::settings::FILE),
                );
                if overrides != self.overrides {
                    self.overrides = overrides;
                    self.apply_config();
                    cx.redraw();
                }
                if self.ctx.reload_theme() {
                    cx.redraw();
                }
            }
            Event::Key(key) => self.key(key, cx, now),
            Event::Pointer(pointer) => self.pointer(pointer, cx, now),
            Event::Paste { mime, data } => {
                let text = String::from_utf8_lossy(&data).into_owned();
                let typing = self.location.is_some()
                    || self.filter.is_some()
                    || self
                        .picker
                        .as_ref()
                        .is_some_and(|picker| picker.typing_name);
                if self.palette_paste(&text)
                    || self.packing_paste(&text)
                    || self.renamer_paste(&text)
                    || self.vcs_paste(&text)
                {
                    cx.redraw();
                } else if let Some(renaming) = &mut self.renaming {
                    renaming.field.insert(&text);
                    cx.redraw();
                } else if typing {
                    self.paste(&text, now, cx);
                } else if self.picker.is_none() && !self.paste_files(&mime, &data, cx) {
                    self.toast("Nothing to paste here: the clipboard holds no files", cx);
                }
            }
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
