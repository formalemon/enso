//! The launcher overlay: a search field, and a ranked list once you type.
//!
//! It opens as a bare field. Nothing is listed until the query has something
//! in it, so the first frame has almost nothing to draw, and the applications
//! themselves are gathered on a background thread while the field is already
//! on screen.
//!
//! Nothing is dimmed behind it. Where the bar runs along the top of the
//! screen, the card grows out of the bar's clock island and hangs under the
//! bar in the middle; elsewhere it settles into place a fifth of the way
//! down ([`enso_ui::emerge`]).

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use enso_core::{
    Context, Icon,
    style::{self, icon, opacity, radius, space, type_scale},
};
use enso_ui::{
    Align, App as UiApp, Button, Canvas, Clip, Cx, Event, Key, KeyEvent, Nav, Paint, PointerEvent,
    Presence, Proxy, Rect, SurfaceSpec, TEXT_TYPES, Text,
    emerge::Perch,
    widget::{
        TextField, field,
        search::{self, List},
    },
};
use serde::Deserialize;

use enso_fs::{
    apps::{self, App},
    fuzzy, open,
    search::{self as files, Hit, Job, Scope},
};

use crate::{
    calc,
    commands::{self, Command},
    history::{self, History},
    windows::{self, Window},
};

/// The `[launch]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    /// Card width in logical pixels.
    pub width: u32,
    /// Rows visible at once; the list scrolls beyond this.
    pub max_results: usize,
    /// Search files as well as applications.
    pub files: bool,
    /// Directories whose files are searched; `~` is the home directory.
    pub file_roots: Vec<String>,
    /// Include dotfiles and dot-directories.
    pub hidden: bool,
    /// Names, globs, or paths to leave out of the file search; `!pattern`
    /// keeps something that would otherwise be left out.
    pub ignore: Vec<String>,
    /// Honour `.gitignore` and `.ignore` files.
    pub gitignore: bool,
    /// Leave out build output, dependency trees, and caches by name.
    pub default_ignores: bool,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            width: style::search::WIDTH as u32,
            max_results: 8,
            files: true,
            file_roots: vec!["~".to_owned(), "~/.config".to_owned()],
            hidden: false,
            ignore: Vec::new(),
            gitignore: true,
            default_ignores: true,
        }
    }
}

impl LaunchConfig {
    /// What the file search covers.
    pub fn scope(&self, home: &Path) -> Scope {
        let mut scope = Scope::new(&self.file_roots, self.hidden, &self.ignore, home);
        scope.gitignore = self.gitignore;
        scope.defaults = self.default_ignores;
        scope
    }
}

// The search card's measures, shared with the clipboard history.
const SEARCH_HEIGHT: f32 = style::search::FIELD;
const ROW_HEIGHT: f32 = style::search::ROW;
/// File name of the application index inside the cache directory.
const INDEX: &str = "apps";
/// File name of the file index inside the cache directory.
pub const FILE_INDEX: &str = "files";
/// File name of the launch history inside the state directory.
const HISTORY: &str = "launch-history";
/// File matches listed under the applications.
const FILE_LIMIT: usize = 40;

/// Messages from the background threads.
#[derive(Debug)]
pub enum Message {
    /// The applications, from the saved index or from a fresh scan.
    Apps(Vec<App>),
    /// The files matching the query that carried this generation.
    Files { generation: u64, hits: Vec<Hit> },
    /// The launcher was invoked again: that closes this one.
    Dismiss,
    /// The windows open now, from niri.
    Windows(Vec<Window>),
}

/// Something the launcher copied, to be kept on the clipboard after it
/// closes: `main` holds it there until something else is copied.
pub type Copied = Arc<Mutex<Option<String>>>;

/// Background work that should be allowed to finish after the window has
/// closed — the file walk, so that its result is saved for next time.
pub type Background = Arc<Mutex<Vec<JoinHandle<()>>>>;

/// One row of the result list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// The answer to a sum.
    Answer(String),
    /// Index into `apps`.
    App(usize),
    /// Index into `windows`: found by its title.
    Window(usize),
    /// One of an application's desktop actions: indices into `apps`, and
    /// into its actions.
    Action(usize, usize),
    /// Index into `commands`.
    Command(usize),
    File(Hit),
}

#[derive(Debug)]
pub struct Launcher {
    ctx: Context,
    config: LaunchConfig,
    presence: Presence,
    apps: Vec<App>,
    /// Whether `apps` has arrived; until then an empty result is not "no
    /// matches", just not known yet.
    loaded: bool,
    /// What's typed: a field like every other, with a caret, a selection,
    /// and the clipboard.
    field: TextField,
    /// Matching applications as indices into `apps`, best first.
    app_results: Vec<usize>,
    /// Matching files, best first, as last answered by the search thread.
    file_results: Vec<Hit>,
    /// Counts queries, so a late answer to an old one can be told apart.
    generation: u64,
    /// The file search thread, once started.
    jobs: Option<Sender<Job>>,
    background: Background,
    /// Where a second invocation knocks to close this one.
    toggle: Option<UnixListener>,
    history: History,
    /// The home directory, shown as `~` in file locations.
    home: String,
    /// Applications first, then files.
    results: Vec<Entry>,
    /// Which result is chosen and which are in view, as every search card
    /// has it.
    list: List,
    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    /// The answer, when the query is a sum.
    answer: Option<String>,
    /// The windows open, from niri.
    windows: Vec<Window>,
    /// Windows whose title matches, as indices into `windows`.
    window_results: Vec<usize>,
    /// Matching desktop actions: (app, action).
    action_results: Vec<(usize, usize)>,
    commands: Vec<Command>,
    /// Matching commands, as indices into `commands`.
    command_results: Vec<usize>,
    /// What went wrong with the last choice, said under the field.
    error: Option<String>,
    /// What was copied, for `main` to keep on the clipboard.
    copied: Copied,
    /// The screen to open on: the one with the focus.
    screen: Option<String>,
    /// Out of the bar's clock, when there is a bar along that screen's top.
    perch: Perch,
}

impl Launcher {
    pub fn new(
        ctx: Context,
        config: LaunchConfig,
        background: Background,
        toggle: Option<UnixListener>,
    ) -> Self {
        Self {
            presence: Presence::new(&ctx.motion),
            list: List::new(config.max_results, &ctx.motion),
            history: History::load(&ctx.paths.state_dir.join(HISTORY)),
            home: std::env::var("HOME").unwrap_or_default(),
            app_results: Vec::new(),
            file_results: Vec::new(),
            answer: None,
            windows: Vec::new(),
            window_results: Vec::new(),
            action_results: Vec::new(),
            commands: commands::all(&ctx.themes),
            command_results: Vec::new(),
            error: None,
            copied: Copied::default(),
            generation: 0,
            jobs: None,
            background,
            toggle,
            ctx,
            config,
            apps: Vec::new(),
            loaded: false,
            field: TextField::default(),
            results: Vec::new(),
            card: Rect::default(),
            screen: None,
            perch: Perch::default(),
        }
    }

    /// Open on `screen`, out of the bar's clock there if `perch` has it.
    #[must_use]
    pub fn on(mut self, screen: Option<String>, perch: Perch) -> Self {
        self.screen = screen;
        self.presence = perch.presence(&self.ctx.motion);
        self.perch = perch;
        self
    }

    /// Open with `text` already typed: `=` for a sum, from a calculator key.
    #[must_use]
    pub fn typed(mut self, text: &str) -> Self {
        self.field.insert(text);
        self
    }

    /// Take the applications to search, keeping the query and — where the
    /// same entry is still a result — the selection.
    pub fn set_apps(&mut self, apps: Vec<App>) {
        let selected = self.selected_key();
        self.apps = apps;
        self.loaded = true;
        self.rank_apps();
        self.compose(selected.as_deref());
    }

    /// Take the answer to a file query. Answers to queries since typed
    /// past are dropped. Returns whether anything changed.
    pub fn set_files(&mut self, generation: u64, mut hits: Vec<Hit>) -> bool {
        if generation != self.generation {
            return false;
        }
        let now = history::now();
        for hit in &mut hits {
            hit.score += self.history.boost(&file_key(&hit.path), now);
        }
        // Stable: equal scores keep the order the search gave them.
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        if hits == self.file_results {
            return false;
        }
        let selected = self.selected_key();
        self.file_results = hits;
        self.compose(selected.as_deref());
        true
    }

    /// Identity of the selected row, to find it again after the list changes.
    fn selected_key(&self) -> Option<String> {
        self.key_of(self.list.selected()?)
    }

    /// Identity of result `index`.
    fn key_of(&self, index: usize) -> Option<String> {
        self.results.get(index).map(|entry| match entry {
            Entry::Answer(_) => "answer".to_owned(),
            Entry::App(index) => app_key(&self.apps[*index].id),
            Entry::Window(index) => format!("window:{}", self.windows[*index].id),
            Entry::Action(app, action) => {
                format!("action:{}:{action}", self.apps[*app].id)
            }
            Entry::Command(index) => format!("command:{}", self.commands[*index].name),
            Entry::File(hit) => file_key(&hit.path),
        })
    }

    /// Rebuild the list from its two halves. The row with identity `keep`
    /// stays selected if it is still listed; otherwise the selection stays
    /// where it is, or returns to the top when there was none to keep.
    fn compose(&mut self, keep: Option<&str>) {
        // A sum's answer first; then what to open or go to; then what to
        // do; then files.
        self.results = self
            .answer
            .iter()
            .cloned()
            .map(Entry::Answer)
            .chain(self.app_results.iter().map(|index| Entry::App(*index)))
            .chain(
                self.window_results
                    .iter()
                    .map(|index| Entry::Window(*index)),
            )
            .chain(
                self.action_results
                    .iter()
                    .map(|(app, action)| Entry::Action(*app, *action)),
            )
            .chain(
                self.command_results
                    .iter()
                    .map(|index| Entry::Command(*index)),
            )
            .chain(self.file_results.iter().cloned().map(Entry::File))
            .collect();
        let kept = keep.and_then(|key| {
            (0..self.results.len()).find(|&index| self.key_of(index).as_deref() == Some(key))
        });
        self.list.reset(self.results.len());
        if let Some(index) = kept {
            self.list.select(index);
        }
        self.reflow();
    }

    fn placeholder(&self) -> &'static str {
        if self.config.files {
            "Search apps and files · \" looks inside them"
        } else {
            "Search apps"
        }
    }

    /// What's typed.
    fn query(&self) -> &str {
        self.field.text()
    }

    /// Nothing has been typed yet (spaces alone do not count).
    fn is_idle(&self) -> bool {
        self.query().trim().is_empty()
    }

    /// The query matched nothing, and that is final.
    fn has_no_matches(&self) -> bool {
        self.loaded && !self.is_idle() && self.results.is_empty()
    }

    fn visible_rows(&self) -> usize {
        self.list.visible()
    }

    fn list_target(&self) -> f32 {
        let error = if self.error.is_some() {
            ROW_HEIGHT
        } else {
            0.0
        };
        error
            + search::height(match self.visible_rows() {
                // Room for the "no matches" line.
                0 if self.has_no_matches() => 1,
                // Just the search field, or the rows.
                rows => rows,
            })
    }

    /// Make room for what's said under the list, or take it away.
    fn reflow(&mut self) {
        self.list.grow(self.list_target());
    }

    /// Rank the applications against the current query. An empty query
    /// lists nothing.
    fn rank_apps(&mut self) {
        // Looking inside files is for files alone.
        self.window_results.clear();
        self.action_results.clear();
        self.command_results.clear();
        self.answer = calc::answer(self.query());
        if self.inside().is_some() || self.answer.is_some() {
            self.app_results.clear();
            return;
        }
        let now = history::now();
        let pattern = fuzzy::Pattern::new(self.query());
        let mut ranked: Vec<(i32, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter(|_| !self.is_idle())
            .filter_map(|(index, app)| {
                let boost = self.history.boost(&app_key(&app.id), now);
                rank(&pattern, app).map(|score| (score + boost, index))
            })
            .collect();
        // Stable sort: ties keep the alphabetical order of `apps`.
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        self.app_results = ranked.into_iter().map(|(_, index)| index).collect();
        if self.is_idle() {
            return;
        }

        // Windows by their title — a page, a folder, a document — when the
        // query is more than a letter or two.
        if self.query().trim().chars().count() >= 3 {
            let mut windows: Vec<(i32, usize)> = self
                .windows
                .iter()
                .enumerate()
                .filter_map(|(index, window)| {
                    let score = pattern.score(&window.title)?;
                    // An app listed already says its window; a title that
                    // only matches as the app's name adds nothing.
                    let named = self.app_results.iter().any(|app| {
                        self.apps[*app].owns_window(&window.app_id)
                            && pattern.score(&self.apps[*app].name).is_some()
                    });
                    (!named).then_some((score, index))
                })
                .collect();
            windows.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            self.window_results = windows.into_iter().take(3).map(|(_, i)| i).collect();
        }

        // An application's actions, by their names or "app action".
        let mut actions: Vec<(i32, (usize, usize))> = Vec::new();
        for (app_index, app) in self.apps.iter().enumerate() {
            for (action_index, action) in app.actions.iter().enumerate() {
                let both = format!("{} {}", app.name, action.name);
                let score = pattern
                    .score(&action.name)
                    .max(pattern.score(&both).map(|s| s - 10));
                if let Some(score) = score {
                    actions.push((score - 20, (app_index, action_index)));
                }
            }
        }
        actions.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        self.action_results = actions.into_iter().take(3).map(|(_, a)| a).collect();

        // Commands, below what's opened.
        let query = self.query().to_owned();
        let mut commands: Vec<(i32, usize)> = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, command)| command.allowed(&query))
            .filter_map(|(index, command)| {
                let score = std::iter::once(command.name.as_str())
                    .chain(command.words.iter().copied())
                    .filter_map(|name| pattern.score(name))
                    .max()?;
                Some((score, index))
            })
            .collect();
        commands.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        self.command_results = commands.into_iter().take(4).map(|(_, i)| i).collect();
    }

    /// The most recently focused window of an application, if it's running.
    /// The window to go to for `app`, rather than opening another; never
    /// one of a terminal's, which is asked for to have another terminal.
    fn running(&self, app: &App) -> Option<&Window> {
        if app.fresh {
            return None;
        }
        self.windows
            .iter()
            .filter(|window| app.owns_window(&window.app_id))
            .max_by_key(|window| window.focused_at)
    }

    /// What to look for inside files, if the query asks that: it starts
    /// with a quote (`"green tea`, or `"/te+a/"` for a pattern).
    fn inside(&self) -> Option<&str> {
        let text = self.query().trim().strip_prefix('"')?;
        Some(text.strip_suffix('"').unwrap_or(text))
    }

    /// Search again for the current query and reset the selection.
    fn search(&mut self) {
        self.rank_apps();
        self.generation += 1;
        let generation = self.generation;
        let job = match self.inside() {
            Some(text) => Job::Inside {
                generation,
                text: text.to_owned(),
            },
            None => Job::Query {
                generation,
                query: self.query().trim().to_owned(),
            },
        };
        let wanted = match &job {
            Job::Inside { text, .. } => text,
            Job::Query { query, .. } => query,
            Job::Index(_) => "",
        };
        let asked = self.config.files
            && wanted.chars().count() >= files::MIN_QUERY
            && self
                .jobs
                .as_ref()
                .is_some_and(|jobs| jobs.send(job).is_ok());
        // While the answer is on its way — a few milliseconds — the files
        // for the previous query stay rather than blink out and back.
        if !asked {
            self.file_results.clear();
        }
        self.compose(None);
    }

    fn move_selection(&mut self, nav: Nav) {
        self.list.step(nav);
    }

    /// Edit the query. Returns whether the key was consumed.
    fn edit(&mut self, key: &KeyEvent, cx: Option<&mut Cx<'_>>) -> bool {
        match self.field.key(key) {
            field::Response::Ignored | field::Response::Submit | field::Response::Cancel => false,
            field::Response::Moved => true,
            field::Response::Changed => {
                self.error = None;
                self.search();
                true
            }
            field::Response::Copy(text) => {
                if let Some(cx) = cx {
                    cx.copy(Clip::text(&text));
                }
                true
            }
            field::Response::Cut(text) => {
                if let Some(cx) = cx {
                    cx.copy(Clip::text(&text));
                }
                self.search();
                true
            }
            field::Response::Paste => {
                if let Some(cx) = cx {
                    cx.paste(&TEXT_TYPES);
                }
                true
            }
        }
    }

    /// What Ctrl+C copies when nothing in the field is selected: a file's
    /// path, or a sum's answer.
    fn copyable(&self) -> Option<String> {
        match self.results.get(self.list.selected()?)? {
            Entry::Answer(answer) => Some(answer.clone()),
            Entry::File(hit) => Some(hit.path.clone()),
            _ => None,
        }
    }

    /// Launch the selected application or open the selected file — or,
    /// with `reveal`, the folder the file is in. A running application's
    /// window is gone to rather than another opened, unless `fresh`.
    fn launch_selected(&mut self, reveal: bool, fresh: bool, cx: &mut Cx<'_>) {
        let Some((index, key)) = self
            .list
            .selected()
            .and_then(|index| Some((index, self.key_of(index)?)))
        else {
            return;
        };
        let terminal = self.ctx.config.appearance.terminal_program();
        let done = match &self.results[index] {
            Entry::Answer(answer) => {
                // Kept on the clipboard by `main` once the window is gone.
                if let Ok(mut copied) = self.copied.lock() {
                    *copied = Some(answer.clone());
                }
                Ok(())
            }
            Entry::App(index) => {
                let app = &self.apps[*index];
                match self.running(app).filter(|_| !fresh) {
                    Some(window) => windows::focus(window.id)
                        .map_err(|err| format!("Can't go to {}: {err}", app.name)),
                    None => app
                        .launch(&terminal)
                        .map_err(|err| format!("Can't start {}: {err}", app.name)),
                }
            }
            Entry::Window(index) => windows::focus(self.windows[*index].id)
                .map_err(|err| format!("Can't go to that window: {err}")),
            Entry::Action(app, action) => {
                let app = &self.apps[*app];
                app.launch_action(*action, &terminal)
                    .map_err(|err| format!("Can't start {}: {err}", app.name))
            }
            Entry::Command(index) => {
                let command = &self.commands[*index];
                command
                    .run()
                    .map_err(|err| format!("Can't {}: {err}", command.name.to_lowercase()))
            }
            Entry::File(hit) => {
                let path = Path::new(&hit.path);
                let target = match path.parent() {
                    Some(parent) if reveal => parent,
                    _ => path,
                };
                open::open(target, &terminal)
                    .map_err(|err| format!("Can't open {}: {err}", target.display()))
            }
        };
        match done {
            Ok(()) => {
                self.history.record(&key, history::now());
                self.presence.close(cx);
            }
            // Said here, where it was asked for, rather than to a log
            // nobody reads; the card stays to try something else.
            Err(err) => {
                eprintln!("enso-launch: {err}");
                self.error = Some(err);
                self.reflow();
                cx.redraw();
            }
        }
    }

    /// What was copied, for keeping after the window has gone.
    pub fn copied(&self) -> Copied {
        Arc::clone(&self.copied)
    }

    /// Start indexing and searching files in the background.
    fn start_files(&mut self, proxy: Proxy<Message>) {
        let scope = self.config.scope(Path::new(&self.home));
        let cache: PathBuf = self.ctx.paths.cache_dir.join(FILE_INDEX);
        let answer = move |generation, hits| {
            proxy.send(Message::Files { generation, hits });
        };
        match files::serve(scope, cache, FILE_LIMIT, answer) {
            Ok((jobs, scan)) => {
                self.jobs = Some(jobs);
                if let Ok(mut background) = self.background.lock() {
                    background.push(scan);
                }
            }
            Err(err) => eprintln!("enso-launch: file search is unavailable: {err}"),
        }
    }
}

fn app_key(id: &str) -> String {
    format!("app:{id}")
}

fn file_key(path: &str) -> String {
    format!("file:{path}")
}

/// Best score for an application across its searchable fields. The name
/// counts most; description, keywords, and id are progressively discounted.
fn rank(pattern: &fuzzy::Pattern, app: &App) -> Option<i32> {
    let name = pattern.score(&app.name);
    let description = app
        .description
        .as_deref()
        .and_then(|d| pattern.score(d))
        .map(|s| s - 25);
    let keywords = app
        .keywords
        .iter()
        .filter_map(|k| pattern.score(k))
        .max()
        .map(|s| s - 30);
    let id = pattern.score(&app.id).map(|s| s - 35);
    [name, description, keywords, id]
        .into_iter()
        .flatten()
        .max()
}

/// Go over the letters of `name` that matched in the accent, each where it
/// stands in the name as drawn (and no further than `shown` of it).
#[allow(clippy::too_many_arguments)]
fn highlight_matches(
    canvas: &mut Canvas<'_>,
    pattern: &fuzzy::Pattern,
    name: &str,
    size: f32,
    accent: enso_core::Rgba,
    x: f32,
    row: Rect,
    shown: f32,
) {
    let Some(positions) = pattern.positions(name) else {
        return;
    };
    let chars: Vec<(usize, char)> = name.char_indices().collect();
    for index in positions {
        let Some(&(at, c)) = chars.get(index) else {
            continue;
        };
        let before = canvas
            .measure(&Text::new(&name[..at], size, accent).medium())
            .w;
        let mut buf = [0u8; 4];
        let letter = Text::new(c.encode_utf8(&mut buf), size, accent).medium();
        let width = canvas.measure(&letter).w;
        // Past an ellipsis, a letter isn't there to mark.
        if before + width > shown - 1.0 {
            break;
        }
        canvas.text_in(
            &letter,
            Rect::new(x + before, row.y, width + 1.0, row.h),
            Align::Start,
        );
    }
}

impl UiApp for Launcher {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        Perch::surface("enso-launch", self.screen.clone())
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        // Gathering applications never holds up the first frame: nothing is
        // listed before a key is pressed, and that is ages away.
        if self.config.files {
            self.start_files(proxy.clone());
        }
        // Opened with something typed: look for it as if it had been.
        if !self.query().is_empty() {
            self.search();
        }
        if let Some(listener) = self.toggle.take() {
            let proxy = proxy.clone();
            // Any connection at all is the knock. Not being able to listen
            // only costs the toggle.
            let _ = thread::Builder::new()
                .name("enso-toggle".to_owned())
                .spawn(move || {
                    if listener.accept().is_ok() {
                        proxy.send(Message::Dismiss);
                    }
                });
        }
        let windows = proxy.clone();
        windows::load(move |open| {
            windows.send(Message::Windows(open));
        });
        let index = self.ctx.paths.cache_dir.join(INDEX);
        let load = move || {
            apps::load_indexed(&index, |apps| {
                proxy.send(Message::Apps(apps));
            });
        };
        if let Err(err) = thread::Builder::new()
            .name("enso-apps".to_owned())
            .spawn(load)
        {
            eprintln!("enso-launch: cannot load applications in the background: {err}");
            self.set_apps(apps::load());
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        match event {
            Event::Wake if self.presence.is_closing() => cx.exit(),
            Event::Message(Message::Apps(apps)) => {
                self.set_apps(apps);
                cx.redraw();
            }
            Event::Message(Message::Files { generation, hits }) => {
                if self.set_files(generation, hits) {
                    cx.redraw();
                }
            }
            Event::Message(Message::Dismiss) => self.presence.close(cx),
            Event::Message(Message::Windows(windows)) => {
                let selected = self.selected_key();
                self.windows = windows;
                self.rank_apps();
                self.compose(selected.as_deref());
                cx.redraw();
            }
            Event::Key(key) => {
                if let Some(nav) = key.nav(false) {
                    self.move_selection(nav);
                } else {
                    match key.key {
                        Key::Escape => return self.presence.close(cx),
                        Key::Enter => {
                            return self.launch_selected(key.mods.ctrl, key.mods.shift, cx);
                        }
                        // Nothing selected in the field: copy the row's path
                        // or answer.
                        Key::Char('c' | 'C')
                            if key.mods.ctrl && self.field.selection().is_none() =>
                        {
                            if let Some(text) = self.copyable() {
                                cx.copy(Clip::text(&text));
                                if let Ok(mut copied) = self.copied.lock() {
                                    *copied = Some(text);
                                }
                            }
                            return;
                        }
                        _ if !self.edit(&key, Some(&mut *cx)) => return,
                        _ => {}
                    }
                }
                cx.redraw();
            }
            Event::Paste { data, .. } => {
                // One line: a pasted paragraph is a search for its words.
                let text = String::from_utf8_lossy(&data);
                let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !line.is_empty() {
                    self.field.insert(&line);
                    self.error = None;
                    self.search();
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Move { x, y }) => {
                if let Some(index) = self.list.at(x, y)
                    && Some(index) != self.list.selected()
                {
                    self.list.select(index);
                    cx.redraw();
                }
            }
            // The wheel moves the choice, a row a turn.
            Event::Pointer(PointerEvent::Scroll { dy, .. }) => {
                self.list.scroll(dy);
                cx.redraw();
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(index) = self.list.at(x, y) {
                    self.list.select(index);
                    self.launch_selected(false, false, cx);
                } else if !self.card.contains(x, y) {
                    self.presence.close(cx);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let base_size = self.ctx.config.appearance.font_size;
        let bounds = canvas.bounds();

        // Nothing behind the card: the desktop stays as it is, and only the
        // card asks for attention. (The canvas starts each frame clear.)

        let look = search::Look {
            colors: colors.clone(),
            accent,
            size: base_size,
        };
        let list_height = self.list.height(now);
        let width = (self.config.width as f32).min(bounds.w - 2.0 * space::LG);
        let height = SEARCH_HEIGHT + list_height;
        let rest = self.perch.rest(bounds, width, height);
        let card = self.perch.card(canvas, rest, presence, &colors);
        self.card = card;

        let placeholder = self.placeholder();
        search::head(canvas, card, &mut self.field, placeholder, list_height, &look);

        let rows = self.list.draw(canvas, card, now, &look);
        if self.results.is_empty() && self.has_no_matches() {
            search::note(canvas, card, "No matches", &look);
        }
        let pattern = fuzzy::Pattern::new(self.query());
        for row in rows {
            let entry = &self.results[row.index];
            let chip = search::chip(row.content);
            let location;
            let (name, description, hint): (&str, Option<&str>, Option<&str>) = match entry {
                Entry::Answer(answer) => {
                    canvas.fill(chip, radius::CONTROL, accent.alpha(opacity::TINT + 0.06));
                    canvas.text_in(
                        &Text::new("=", base_size, accent).medium(),
                        chip,
                        Align::Center,
                    );
                    (answer.as_str(), Some(self.query().trim()), Some("Copy"))
                }
                Entry::App(index) => {
                    let app = &self.apps[*index];
                    search::letter_chip(canvas, chip, &app.name, &look);
                    // Running: a dot on its chip, and Enter goes there.
                    let running = self.running(app).is_some();
                    if running {
                        let (x, y) = (chip.right() - 1.0, chip.bottom() - 1.0);
                        canvas.circle(x, y, 4.0, colors.surface);
                        canvas.circle(x, y, 2.6, accent);
                    }
                    let hint = running.then_some("Go to window · Shift+Enter new");
                    (app.name.as_str(), app.description.as_deref(), hint)
                }
                Entry::Window(index) => {
                    let window = &self.windows[*index];
                    let owner = self.apps.iter().find(|app| app.owns_window(&window.app_id));
                    let app_name = owner.map_or(window.app_id.as_str(), |app| app.name.as_str());
                    search::letter_chip(canvas, chip, app_name, &look);
                    (window.title.as_str(), Some(app_name), Some("Go to window"))
                }
                Entry::Action(app, action) => {
                    let app = &self.apps[*app];
                    search::letter_chip(canvas, chip, &app.name, &look);
                    let action = app.actions[*action].name.as_str();
                    (action, Some(app.name.as_str()), None)
                }
                Entry::Command(index) => {
                    let command = &self.commands[*index];
                    let figure = Icon::new(command.figure, "");
                    search::icon_chip(canvas, chip, figure, colors.subtle, &look);
                    (command.name.as_str(), None, Some("Run"))
                }
                Entry::File(hit) => {
                    // Files are quieter than applications: one glyph,
                    // folders in the accent's cooler neighbour.
                    let (glyph, tint) = if hit.is_dir {
                        (icon::FOLDER, colors.foam)
                    } else {
                        (icon::FILE, colors.subtle)
                    };
                    search::icon_chip(canvas, chip, glyph, tint, &look);
                    // Found inside: the line that has it says more than
                    // where the file is.
                    location = match &hit.line {
                        Some(line) => line.clone(),
                        None => hit.location(&self.home),
                    };
                    (hit.name(), Some(location.as_str()), None)
                }
            };
            let from = chip.right() + space::MD;
            let chosen = row.chosen;
            let (x, shown) =
                search::words(canvas, row.content, from, name, description, hint, chosen, &look);
            // What was typed, picked out where it matched the name.
            if !matches!(entry, Entry::Answer(_)) {
                highlight_matches(canvas, &pattern, name, base_size, accent, x, row.content, shown);
            }
        }

        // What went wrong with the last choice, at the foot of the card.
        if let Some(error) = &self.error {
            let line = Rect::new(card.x, card.bottom() - ROW_HEIGHT, card.w, ROW_HEIGHT)
                .inset_xy(space::LG, 0.0);
            if line.y >= card.y + SEARCH_HEIGHT {
                canvas.text_in(
                    &Text::new(error, base_size * type_scale::SMALL, colors.love).max_width(line.w),
                    line,
                    Align::Start,
                );
            }
        }

        canvas.set_opacity(1.0);
        Paint::when(self.presence.is_animating(now) || self.list.is_animating(now))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<LaunchConfig>("launch").unwrap(),
            LaunchConfig::default()
        );
    }

    fn app(name: &str, description: &str) -> App {
        App {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.to_owned(),
            description: (!description.is_empty()).then(|| description.to_owned()),
            keywords: Vec::new(),
            command: vec!["true".to_owned()],
            terminal: false,
            fresh: false,
            wm_class: None,
            actions: Vec::new(),
        }
    }

    /// What a row is called, as the tests read it.
    fn name_of<'a>(launcher: &'a Launcher, entry: &'a Entry) -> &'a str {
        match entry {
            Entry::Answer(answer) => answer,
            Entry::App(index) => launcher.apps[*index].name.as_str(),
            Entry::Window(index) => launcher.windows[*index].title.as_str(),
            Entry::Action(app, action) => launcher.apps[*app].actions[*action].name.as_str(),
            Entry::Command(index) => launcher.commands[*index].name.as_str(),
            Entry::File(hit) => hit.name(),
        }
    }

    fn launcher() -> Launcher {
        let apps = vec![
            app("Alacritty", "Terminal"),
            app("Calculator", ""),
            app("Files", "File Manager"),
            app("Firefox", "Web Browser"),
            app("Image Viewer", ""),
            app("Neovim", "Text Editor"),
            app("Settings", ""),
            app("Text Editor", ""),
            app("Videos", "Media Player"),
            app("Zathura", "Document Viewer"),
        ];
        let mut launcher = unloaded();
        launcher.set_apps(apps);
        launcher
    }

    /// A launcher whose applications have not arrived yet. Its commands
    /// are left out, so the tests of ranking see applications and files;
    /// the commands have tests of their own.
    fn unloaded() -> Launcher {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-launch-test"));
        let mut launcher = Launcher::new(
            ctx,
            LaunchConfig {
                max_results: 6,
                ..LaunchConfig::default()
            },
            Background::default(),
            None,
        );
        launcher.commands.clear();
        launcher
    }

    fn window(id: u64, app_id: &str, title: &str, focused_at: u64) -> Window {
        Window {
            id,
            app_id: app_id.to_owned(),
            title: title.to_owned(),
            focused_at: Some((focused_at, 0)),
        }
    }

    #[test]
    fn a_sum_is_answered_first() {
        let mut launcher = launcher();
        type_text(&mut launcher, "12*7.5");
        assert_eq!(names(&launcher), ["90"]);
        assert_eq!(launcher.copyable().as_deref(), Some("90"));
        let frame = settled(&mut launcher);
        snapshot(&frame, "launch-sum");
    }

    /// The opening drawn over a bar, a frame at a time, to look at:
    /// `target/snapshots/launch-emerge.png`.
    #[test]
    fn the_card_opens_out_of_the_bars_clock() {
        let (width, height) = (800u32, 200u32);
        let island = enso_core::island::Island {
            pid: 1,
            top: true,
            bar: 36.0,
            x: 270.0,
            y: 4.0,
            w: 260.0,
            h: 28.0,
            opacity: 0.9,
            ..enso_core::island::Island::default()
        };
        let mut launcher = launcher().on(
            None,
            Perch {
                island: Some(island),
                ..Perch::default()
            },
        );
        let colors = launcher.ctx.theme.colors.clone();
        let mut text = TextEngine::new(&launcher.ctx.config.appearance);
        // The desktop: something to see the card against, and the bar's
        // clock island.
        let desk = Offscreen::render(&mut text, (width, height), 1, |canvas| {
            canvas.clear(colors.pine.mix(colors.overlay, 0.6));
            let clock = Rect::new(island.x, island.y, island.w, island.h);
            canvas.pill(clock, colors.base.alpha(island.opacity));
            let words = Text::new("Wed 25 Nov  15:44", 13.0, colors.text).medium();
            canvas.text_in(&words, clock, Align::Center);
        });
        let start = Instant::now();
        let moments = [0, 30, 60, 90, 130, 180, 250, 330, 420, 700];
        let mut sheet = Vec::new();
        for (n, ms) in moments.into_iter().enumerate() {
            let frame = Offscreen::render(&mut text, (width, height), 1, |canvas| {
                let painted = launcher.draw(canvas, start + Duration::from_millis(ms));
                assert_eq!(painted == Paint::Idle, ms >= 420, "at {ms} ms");
            });
            if n == 0 {
                // Nothing at all before the reveal has begun.
                assert!(frame.rgba.iter().all(|&byte| byte == 0));
            }
            // The frame over the desk: premultiplied, source over.
            let mut over = desk.rgba.clone();
            for (dst, src) in over.chunks_mut(4).zip(frame.rgba.chunks(4)) {
                let keep = 255 - u32::from(src[3]);
                for c in 0..4 {
                    dst[c] = (u32::from(src[c]) + (u32::from(dst[c]) * keep + 127) / 255) as u8;
                }
            }
            sheet.extend(over);
        }
        let sheet = Offscreen {
            width,
            height: height * moments.len() as u32,
            rgba: sheet,
        };
        sheet.snapshot(env!("CARGO_MANIFEST_DIR"), "launch-emerge");
        // At rest: under the bar, in the middle, and the clock left clear.
        assert_eq!(launcher.card.y, 40.0);
        assert_eq!(launcher.card.center().0, 400.0);
        let last = (moments.len() as u32 - 1) * height;
        assert_eq!(sheet.pixel(400, last + 18), desk.pixel(400, 18));
        assert_ne!(sheet.pixel(400, last + 70), desk.pixel(400, 70));
    }

    #[test]
    fn a_running_application_is_gone_to_and_windows_are_found_by_title() {
        let mut launcher = launcher();
        launcher.windows = vec![
            window(1, "firefox", "Quiet Hours — Mozilla Firefox", 10),
            window(2, "firefox", "Inbox — Mozilla Firefox", 20),
            window(3, "Alacritty", "~/dev/enso", 5),
        ];
        type_text(&mut launcher, "fire");
        let app = launcher
            .apps
            .iter()
            .find(|app| app.name == "Firefox")
            .unwrap();
        // The most recently used of its windows.
        assert_eq!(launcher.running(app).map(|w| w.id), Some(2));
        // Its windows aren't listed again by title: the app says them.
        assert_eq!(names(&launcher), ["Firefox"]);

        launcher.field.replace_all("");
        type_text(&mut launcher, "dev/enso");
        assert_eq!(names(&launcher), ["~/dev/enso"]);
        let frame = settled(&mut launcher);
        snapshot(&frame, "launch-window");
    }

    #[test]
    fn actions_and_commands_are_found_by_name() {
        let mut launcher = launcher();
        let mut apps = launcher.apps.clone();
        let firefox = apps.iter_mut().find(|app| app.name == "Firefox").unwrap();
        firefox.actions.push(enso_fs::apps::Shortcut {
            name: "New Private Window".into(),
            command: vec!["true".into()],
        });
        launcher.set_apps(apps);
        launcher.commands = commands::all(&launcher.ctx.themes);
        type_text(&mut launcher, "private");
        assert_eq!(names(&launcher)[0], "New Private Window");

        launcher.field.replace_all("");
        type_text(&mut launcher, "lock");
        assert!(names(&launcher).contains(&"Lock screen"));
        // Ending the session wants most of the word.
        launcher.field.replace_all("");
        type_text(&mut launcher, "re");
        assert!(!names(&launcher).contains(&"Restart"));
        type_text(&mut launcher, "st");
        assert!(names(&launcher).contains(&"Restart"));
        let frame = settled(&mut launcher);
        snapshot(&frame, "launch-commands");
    }

    #[test]
    fn pasting_and_moving_the_caret_edit_the_query() {
        let mut launcher = launcher();
        launcher.field.insert("fox");
        launcher.edit(&key(Key::Home, false), None);
        type_text(&mut launcher, "fire");
        assert_eq!(launcher.query(), "firefox");
        assert_eq!(names(&launcher), ["Firefox"]);
    }

    #[test]
    fn what_went_wrong_is_said_on_the_card() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fire");
        launcher.error = Some("Can't start Firefox: No such file or directory".into());
        launcher.reflow();
        let frame = settled(&mut launcher);
        snapshot(&frame, "launch-error");
    }

    fn selected_name(launcher: &Launcher) -> Option<&str> {
        launcher
            .list
            .selected()
            .and_then(|index| launcher.results.get(index))
            .map(|entry| name_of(launcher, entry))
    }

    fn hit(path: &str, score: i32) -> Hit {
        Hit {
            path: path.strip_suffix('/').unwrap_or(path).to_owned(),
            is_dir: path.ends_with('/'),
            score,
            line: None,
        }
    }

    /// Draw the launcher at rest and return the frame.
    fn settled(launcher: &mut Launcher) -> Offscreen {
        let later = Instant::now() + Duration::from_secs(2);
        let mut text = TextEngine::new(&launcher.ctx.config.appearance);
        // Draw once to start the reveal, then again at rest.
        Offscreen::render(&mut text, (800, 520), 2, |canvas| {
            assert_eq!(launcher.draw(canvas, Instant::now()), Paint::Animating);
            canvas.clear(enso_core::Rgba::TRANSPARENT);
            assert_eq!(launcher.draw(canvas, later), Paint::Idle);
        })
    }

    /// The pictures held to a reference in `snapshots/`; the rest are
    /// only written, to look at.
    const CHECKED: &[&str] = &["launch"];

    fn snapshot(frame: &Offscreen, name: &str) {
        if CHECKED.contains(&name) {
            frame.check(env!("CARGO_MANIFEST_DIR"), name);
        } else {
            frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
        }
    }

    fn key(key: Key, ctrl: bool) -> KeyEvent {
        KeyEvent {
            key,
            mods: Mods {
                ctrl,
                ..Mods::default()
            },
            repeat: false,
        }
    }

    fn type_text(launcher: &mut Launcher, text: &str) {
        for c in text.chars() {
            assert!(launcher.edit(&key(Key::Char(c), false), None));
        }
    }

    fn names(launcher: &Launcher) -> Vec<&str> {
        launcher
            .results
            .iter()
            .map(|entry| name_of(launcher, entry))
            .collect()
    }

    #[test]
    fn an_input_method_composes_after_the_query() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fi");
        let later = Instant::now() + Duration::from_secs(2);
        let mut text = TextEngine::new(&launcher.ctx.config.appearance);
        let mut carets = Vec::new();
        let frame = Offscreen::render(&mut text, (800, 520), 2, |canvas| {
            launcher.draw(canvas, Instant::now());
            canvas.clear(enso_core::Rgba::TRANSPARENT);
            launcher.draw(canvas, later);
            carets.push(canvas.text_input());
            canvas.clear(enso_core::Rgba::TRANSPARENT);
            canvas.set_preedit(Some(enso_ui::Preedit {
                text: "re".into(),
                cursor: Some(2..2),
            }));
            launcher.draw(canvas, later);
            carets.push(canvas.text_input());
        });
        snapshot(&frame, "launch-preedit");
        let (typed, composing) = (carets[0].unwrap_or_default(), carets[1].unwrap_or_default());
        assert!(typed.x > 0.0);
        // The caret, and the candidates with it, sit after what's composed.
        assert!(composing.x > typed.x + 8.0, "{typed:?} {composing:?}");
        // Composing isn't typing: the query waits for the commit.
        assert_eq!(launcher.query(), "fi");
    }

    #[test]
    fn nothing_is_listed_until_typing_starts() {
        let mut launcher = launcher();
        assert!(launcher.results.is_empty());
        assert!(selected_name(&launcher).is_none());
        assert_eq!(launcher.list_target(), 0.0);

        // The card is just the search field: no rows, no "no matches".
        let frame = settled(&mut launcher);
        assert!(launcher.list.drawn().is_empty());
        assert_eq!(launcher.card.h, SEARCH_HEIGHT);
        snapshot(&frame, "launch-empty");

        // A space is not a query.
        type_text(&mut launcher, " ");
        assert!(launcher.results.is_empty());
        assert_eq!(launcher.list_target(), 0.0);

        type_text(&mut launcher, "f");
        assert_eq!(names(&launcher)[..2], ["Files", "Firefox"]);
        assert!(launcher.list_target() > 0.0);

        // Clearing the query folds the list away again.
        launcher.edit(&key(Key::Char('u'), true), None);
        assert!(launcher.results.is_empty());
        assert_eq!(launcher.list_target(), 0.0);
    }

    #[test]
    fn typing_before_the_applications_arrive_is_kept() {
        let mut launcher = unloaded();
        type_text(&mut launcher, "fire");
        // Not "no matches" — the answer is simply not in yet.
        assert!(launcher.results.is_empty());
        assert!(!launcher.has_no_matches());
        assert_eq!(launcher.list_target(), 0.0);

        launcher.set_apps(vec![app("Files", ""), app("Firefox", "Web Browser")]);
        assert_eq!(launcher.query(), "fire");
        assert_eq!(names(&launcher), ["Firefox"]);

        type_text(&mut launcher, "zzz");
        assert!(launcher.has_no_matches());
        assert_eq!(launcher.list_target(), ROW_HEIGHT + space::SM);
    }

    #[test]
    fn a_corrected_application_list_keeps_the_selection() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fi");
        launcher.move_selection(Nav::Down);
        assert_eq!(selected_name(&launcher).unwrap(), "Firefox");

        // The fresh scan found something the saved index did not have.
        launcher.set_apps(vec![
            app("Files", ""),
            app("Filelight", ""),
            app("Firefox", "Web Browser"),
        ]);
        assert_eq!(launcher.query(), "fi");
        assert_eq!(launcher.results.len(), 3);
        assert_eq!(selected_name(&launcher).unwrap(), "Firefox");

        // If the selected application is gone, the best match is selected.
        launcher.set_apps(vec![app("Files", "")]);
        assert_eq!(selected_name(&launcher).unwrap(), "Files");
    }

    #[test]
    fn typing_ranks_names_above_descriptions() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fi");
        assert_eq!(names(&launcher)[..2], ["Files", "Firefox"]);

        // "Text Editor" by name beats Neovim, which only matches by description.
        launcher.edit(&key(Key::Char('u'), true), None);
        type_text(&mut launcher, "text ed");
        assert_eq!(names(&launcher), ["Text Editor", "Neovim"]);

        type_text(&mut launcher, "zzz");
        assert!(launcher.results.is_empty());
        assert!(selected_name(&launcher).is_none());
    }

    #[test]
    fn editing_keys() {
        let mut launcher = launcher();
        type_text(&mut launcher, "web brow");
        launcher.edit(&key(Key::Char('w'), true), None);
        assert_eq!(launcher.query(), "web ");
        launcher.edit(&key(Key::Backspace, false), None);
        assert_eq!(launcher.query(), "web");
        launcher.edit(&key(Key::Char('u'), true), None);
        assert_eq!(launcher.query(), "");
        // Shortcuts are not typed into the field.
        assert!(!launcher.edit(&key(Key::Char('x'), true), None));
        assert!(!launcher.edit(&key(Key::Tab, false), None));
    }

    #[test]
    fn selection_wraps_and_scrolls() {
        let mut launcher = launcher();
        // Nine of the ten match "e", by name or description; six fit.
        type_text(&mut launcher, "e");
        assert_eq!(launcher.results.len(), 9);
        assert_eq!(key(Key::Char('n'), true).nav(false), Some(Nav::Down));
        for _ in 0..6 {
            launcher.move_selection(Nav::Down);
        }
        assert_eq!((launcher.list.selected().unwrap(), launcher.list.offset()), (6, 1));
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.list.selected().unwrap(), launcher.list.offset()), (5, 1));
        launcher.list.select(0);
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.list.selected().unwrap(), launcher.list.offset()), (8, 3));
        launcher.move_selection(Nav::Down);
        assert_eq!((launcher.list.selected().unwrap(), launcher.list.offset()), (0, 0));
    }

    #[test]
    fn renders_card_and_rows() {
        let mut launcher = launcher();
        type_text(&mut launcher, "e");
        launcher.move_selection(Nav::Down);
        let frame = settled(&mut launcher);

        assert_eq!(launcher.list.drawn().len(), 6);
        assert!(
            launcher
                .list
                .drawn()
                .iter()
                .all(|row| launcher.card.contains(row.x, row.bottom() - 1.0))
        );
        // Nothing is drawn outside the card: the desktop shows as it is.
        assert_eq!(frame.pixel(10, 10), [0, 0, 0, 0]);
        let surface = launcher.ctx.theme.colors.surface;
        let (x, y) = (
            (launcher.card.x + 6.0) * 2.0,
            (launcher.card.y + 28.0) * 2.0,
        );
        assert_eq!(
            frame.pixel(x as u32, y as u32),
            [surface.r, surface.g, surface.b, 255]
        );
        snapshot(&frame, "launch");
    }

    #[test]
    fn files_are_listed_under_the_applications() {
        let mut launcher = launcher();
        launcher.home = "/home/mira".to_owned();
        type_text(&mut launcher, "fi");
        assert_eq!(names(&launcher), ["Files", "Firefox"]);

        // An answer to a query since typed past is ignored.
        let stale = launcher.generation - 1;
        assert!(!launcher.set_files(stale, vec![hit("/home/mira/old.txt", 9)]));
        assert_eq!(launcher.results.len(), 2);

        launcher.move_selection(Nav::Down);
        let hits = vec![
            hit("/home/mira/notes/first-ideas.md", 80),
            hit("/home/mira/dev/fish/", 70),
            hit("/home/mira/Pictures/fig.png", 60),
        ];
        assert!(launcher.set_files(launcher.generation, hits.clone()));
        assert_eq!(
            names(&launcher),
            ["Files", "Firefox", "first-ideas.md", "fish", "fig.png"]
        );
        // The list grew under the selection without moving it.
        assert_eq!(selected_name(&launcher), Some("Firefox"));
        // The same answer again changes nothing.
        assert!(!launcher.set_files(launcher.generation, hits));

        // A selected file stays selected when the applications change.
        launcher.list.select(3);
        launcher.set_apps(vec![app("Files", "")]);
        assert_eq!(selected_name(&launcher), Some("fish"));
        assert_eq!(launcher.list.selected(), Some(2));

        let frame = settled(&mut launcher);
        assert_eq!(launcher.list.drawn().len(), 4);
        snapshot(&frame, "launch-files");

        // Too short a query asks for no files and drops the old ones.
        launcher.edit(&key(Key::Backspace, false), None);
        assert_eq!(names(&launcher), ["Files"]);
    }

    #[test]
    fn a_quote_looks_inside_files_alone() {
        let mut launcher = launcher();
        launcher.home = "/home/mira".to_owned();
        type_text(&mut launcher, "\"fi");
        assert_eq!(launcher.inside(), Some("fi"));
        // Applications aren't what's looked inside.
        assert!(names(&launcher).is_empty());
        let mut found = hit("/home/mira/notes/tea.md", 0);
        found.line = Some("Find the first flush of the year".to_owned());
        assert!(launcher.set_files(launcher.generation, vec![found]));
        assert_eq!(names(&launcher), ["tea.md"]);
        let frame = settled(&mut launcher);
        snapshot(&frame, "launch-inside");
    }

    #[test]
    fn what_was_chosen_before_ranks_higher() {
        let mut launcher = launcher();
        type_text(&mut launcher, "fi");
        assert_eq!(names(&launcher), ["Files", "Firefox"]);
        let now = history::now();
        launcher.history.record("app:firefox", now);
        launcher.history.record("file:/home/mira/b.txt", now);
        launcher.search();
        assert_eq!(names(&launcher), ["Firefox", "Files"]);

        let hits = vec![hit("/home/mira/a.txt", 50), hit("/home/mira/b.txt", 40)];
        launcher.set_files(launcher.generation, hits);
        assert_eq!(names(&launcher)[2..], ["b.txt", "a.txt"]);
    }

    #[test]
    fn says_so_when_nothing_matches() {
        let mut launcher = launcher();
        type_text(&mut launcher, "qqq");
        let frame = settled(&mut launcher);
        assert!(launcher.list.drawn().is_empty());
        assert_eq!(launcher.card.h, SEARCH_HEIGHT + ROW_HEIGHT + space::SM);
        snapshot(&frame, "launch-no-matches");
    }
}
