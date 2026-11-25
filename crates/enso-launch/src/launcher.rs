//! The launcher overlay: a search field, and a ranked list once you type.
//!
//! It opens as a bare field. Nothing is listed until the query has something
//! in it, so the first frame has almost nothing to draw, and the applications
//! themselves are gathered on a background thread while the field is already
//! on screen.
//!
//! Nothing is dimmed behind it. Where the bar runs along the top of the
//! screen, the card grows out of the bar's clock island ([`Emergence`]) and
//! hangs under the bar in the middle; elsewhere it settles into place a
//! fifth of the way down.

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use enso_core::{
    Context, Easing, Icon, Rgba, Role, Transition, Tween,
    island::Island,
    style::{Shadow, edge, icon, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App as UiApp, Button, Canvas, Clip, Cx, Event, Key, KeyEvent, Nav, Paint, PointerEvent,
    Presence, Proxy, Rect, Shape, SurfaceSpec, TEXT_TYPES, Text,
    widget::{TextField, field},
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
            width: 560,
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

const SEARCH_HEIGHT: f32 = 56.0;
const ROW_HEIGHT: f32 = 44.0;
const CHIP: f32 = 28.0;
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
    selected: usize,
    /// First visible result.
    offset: usize,
    /// Animated position of the selection highlight, in visible rows.
    highlight: Tween<f32>,
    /// Animated height of the result list.
    list_height: Tween<f32>,
    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    rows: Vec<Rect>,
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
    /// The bar's clock island on that screen, when there is a bar along its
    /// top: the card grows out of it.
    island: Option<Island>,
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
            selected: 0,
            offset: 0,
            highlight: Tween::new(0.0),
            list_height: Tween::new(0.0),
            card: Rect::default(),
            rows: Vec::new(),
            screen: None,
            island: None,
        }
    }

    /// Open on `screen`, out of the bar's clock `island` there if it has
    /// one along its top.
    #[must_use]
    pub fn on(mut self, screen: Option<String>, island: Option<Island>) -> Self {
        self.screen = screen;
        self.island = island.filter(|island| island.top);
        if self.island.is_some() {
            self.presence = Presence::with(&self.ctx.motion, EMERGE, RETURN);
        }
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
        self.results.get(self.selected).map(|entry| match entry {
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
            (0..self.results.len()).find(|index| {
                self.selected = *index;
                self.selected_key().as_deref() == Some(key)
            })
        });
        self.selected = 0;
        self.offset = 0;
        let now = Instant::now();
        self.highlight
            .set(0.0, self.ctx.motion.apply(Transition::SELECT), now);
        if let Some(index) = kept {
            self.select(index);
        }
        self.list_height.set(
            self.list_target(),
            self.ctx.motion.apply(Transition::REFLOW),
            now,
        );
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
        self.results.len().min(self.config.max_results.max(1))
    }

    fn list_target(&self) -> f32 {
        let error = if self.error.is_some() {
            ROW_HEIGHT
        } else {
            0.0
        };
        error
            + match self.visible_rows() {
                // Room for the "no matches" line.
                0 if self.has_no_matches() => ROW_HEIGHT + space::SM,
                // Just the search field.
                0 => 0.0,
                rows => rows as f32 * ROW_HEIGHT + space::SM,
            }
    }

    /// Make room for what's said under the list, or take it away.
    fn reflow(&mut self) {
        self.list_height.set(
            self.list_target(),
            self.ctx.motion.apply(Transition::REFLOW),
            Instant::now(),
        );
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

    fn select(&mut self, index: usize) {
        if self.results.is_empty() {
            return;
        }
        self.selected = index.min(self.results.len() - 1);
        // Scroll just enough to keep the selection visible.
        let rows = self.visible_rows();
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + rows {
            self.offset = self.selected + 1 - rows;
        }
        self.highlight.set(
            (self.selected - self.offset) as f32,
            self.ctx.motion.apply(Transition::SELECT),
            Instant::now(),
        );
    }

    fn move_selection(&mut self, nav: Nav) {
        let count = self.results.len();
        if count == 0 {
            return;
        }
        self.select(match nav {
            Nav::Down => (self.selected + 1) % count,
            Nav::Up => (self.selected + count - 1) % count,
        });
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
        match self.results.get(self.selected)? {
            Entry::Answer(answer) => Some(answer.clone()),
            Entry::File(hit) => Some(hit.path.clone()),
            _ => None,
        }
    }

    /// Launch the selected application or open the selected file — or,
    /// with `reveal`, the folder the file is in. A running application's
    /// window is gone to rather than another opened, unless `fresh`.
    fn launch_selected(&mut self, reveal: bool, fresh: bool, cx: &mut Cx<'_>) {
        let Some(key) = self.selected_key() else {
            return;
        };
        let terminal = self.ctx.config.appearance.terminal_program();
        let done = match &self.results[self.selected] {
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

/// A stable accent for an application's letter chip, picked by name.
fn chip_role(name: &str) -> Role {
    let hash = name.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Role::ACCENTS[hash as usize % Role::ACCENTS.len()]
}

/// An application's letter chip: a stand-in for an icon, needing no icon
/// theme.
fn letter_chip(
    canvas: &mut Canvas<'_>,
    chip: Rect,
    name: &str,
    size: f32,
    colors: &enso_core::Palette,
) {
    let tint = colors.get(chip_role(name));
    canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT + 0.06));
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    canvas.text_in(&Text::new(&initial, size, tint).bold(), chip, Align::Center);
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
        SurfaceSpec {
            // Over the bar too, which the card grows out of.
            exclusive_zone: -1,
            output: self.screen.clone(),
            ..SurfaceSpec::overlay("enso-launch")
        }
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
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y))
                    && self.offset + row != self.selected
                {
                    self.select(self.offset + row);
                    cx.redraw();
                }
            }
            Event::Pointer(PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            }) => {
                if let Some(row) = self.rows.iter().position(|row| row.contains(x, y)) {
                    self.select(self.offset + row);
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
        let title = base_size * type_scale::TITLE;
        let bounds = canvas.bounds();

        // Nothing behind the card: the desktop stays as it is, and only the
        // card asks for attention. (The canvas starts each frame clear.)

        let list_height = self.list_height.get(now);
        let width = (self.config.width as f32).min(bounds.w - 2.0 * space::LG);
        let height = SEARCH_HEIGHT + list_height;
        let card = match self.island {
            // Out of the bar's clock, to hang under the bar in the middle.
            Some(island) => {
                let card = Rect::new(
                    ((bounds.w - width) / 2.0).round(),
                    island.bar + edge::BELOW_BAR,
                    width,
                    height,
                );
                let stage = Emergence::at(presence);
                emerge(canvas, &island, card, &stage, &colors);
                canvas.set_opacity(stage.content);
                card
            }
            // Without a bar, a fifth of the way down, settling into place.
            None => {
                let top = (bounds.h * 0.2).round();
                let card = Rect::new((bounds.w - width) / 2.0, top, width, height)
                    .translate(0.0, (presence - 1.0) * space::MD);
                canvas.set_opacity(presence);
                canvas.card(card, colors.surface, colors.highlight_med);
                card
            }
        };
        self.card = card;

        // Search field.
        let search = Rect::new(card.x, card.y, card.w, SEARCH_HEIGHT).inset_xy(space::LG, 0.0);
        // The magnifier stands as tall as the words typed after it.
        let magnifier = (title * 1.25).round();
        let icon_width = if canvas.glyphs() {
            magnifier + space::SM
        } else {
            space::LG
        };
        canvas.icon(icon::SEARCH, magnifier, colors.subtle, search, Align::Start);
        let field = Rect::new(
            search.x + icon_width + space::XS,
            search.y,
            search.w - icon_width - space::XS,
            search.h,
        );
        // The field: a caret to move, a selection, what an input method is
        // composing — as in every enso text box.
        let style = field::Style {
            size: title,
            color: colors.text,
            placeholder: self.placeholder(),
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        self.field.draw(canvas, field, &style, true);

        // The rule under the field belongs to the list: it fades in with it.
        let list_open = (list_height / ROW_HEIGHT).min(1.0);
        if list_open > 0.0 {
            canvas.fill(
                Rect::new(
                    card.x + space::LG,
                    card.y + SEARCH_HEIGHT - 0.5,
                    card.w - 2.0 * space::LG,
                    stroke::HAIRLINE,
                ),
                0.0,
                colors.highlight_med.alpha(list_open),
            );
        }

        // Results.
        let list = Rect::new(
            card.x + space::SM,
            card.y + SEARCH_HEIGHT + space::XS,
            card.w - 2.0 * space::SM,
            0.0,
        );
        self.rows.clear();
        if self.results.is_empty() {
            let row = list.row(0, ROW_HEIGHT, 0.0).inset_xy(space::SM, 0.0);
            // Like the rows, the line waits until the card has grown past it.
            if self.has_no_matches() && row.bottom() <= card.bottom() {
                canvas.text_in(
                    &Text::new("No matches", base_size, colors.muted),
                    row,
                    Align::Start,
                );
            }
        } else {
            let highlight = list
                .row(0, ROW_HEIGHT, 0.0)
                .translate(0.0, self.highlight.get(now) * ROW_HEIGHT);
            canvas.fill(highlight, radius::CONTROL, colors.overlay);

            let visible = self.visible_rows();
            let glyphs = canvas.glyphs();
            let pattern = fuzzy::Pattern::new(self.query());
            for (row_index, entry) in self
                .results
                .iter()
                .skip(self.offset)
                .take(visible)
                .enumerate()
            {
                let row = list.row(row_index, ROW_HEIGHT, 0.0);
                // While the card is still growing, rows below its edge wait their turn.
                if row.bottom() > card.bottom() {
                    break;
                }
                self.rows.push(row);
                let content = row.inset_xy(space::SM, 0.0);
                let chip = Rect::new(content.x, content.y + (content.h - CHIP) / 2.0, CHIP, CHIP);

                let location;
                let (name, description, hint): (&str, Option<&str>, Option<&str>) = match entry {
                    Entry::Answer(answer) => {
                        canvas.fill(chip, radius::CONTROL, accent.alpha(opacity::TINT + 0.06));
                        canvas.text_in(
                            &Text::new("=", base_size, accent).bold(),
                            chip,
                            Align::Center,
                        );
                        (answer.as_str(), Some(self.query().trim()), Some("Copy"))
                    }
                    Entry::App(index) => {
                        let app = &self.apps[*index];
                        letter_chip(canvas, chip, &app.name, base_size, &colors);
                        // Running: a dot on its chip, and Enter goes there.
                        let running = self.running(app).is_some();
                        if running {
                            canvas.circle(
                                chip.right() - 1.0,
                                chip.bottom() - 1.0,
                                4.0,
                                colors.surface,
                            );
                            canvas.circle(chip.right() - 1.0, chip.bottom() - 1.0, 2.6, accent);
                        }
                        let hint = running.then_some("Go to window · Shift+Enter new");
                        (app.name.as_str(), app.description.as_deref(), hint)
                    }
                    Entry::Window(index) => {
                        let window = &self.windows[*index];
                        let owner = self.apps.iter().find(|app| app.owns_window(&window.app_id));
                        let app_name =
                            owner.map_or(window.app_id.as_str(), |app| app.name.as_str());
                        letter_chip(canvas, chip, app_name, base_size, &colors);
                        (window.title.as_str(), Some(app_name), Some("Go to window"))
                    }
                    Entry::Action(app, action) => {
                        let app = &self.apps[*app];
                        letter_chip(canvas, chip, &app.name, base_size, &colors);
                        (
                            app.actions[*action].name.as_str(),
                            Some(app.name.as_str()),
                            None,
                        )
                    }
                    Entry::Command(index) => {
                        let command = &self.commands[*index];
                        canvas.fill(chip, radius::CONTROL, colors.overlay);
                        let size = if glyphs { base_size + 3.0 } else { base_size };
                        canvas.icon(
                            Icon::new(command.figure, ""),
                            size,
                            colors.subtle,
                            chip,
                            Align::Center,
                        );
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
                        canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT));
                        let size = if glyphs { base_size + 5.0 } else { base_size };
                        canvas.icon(glyph, size, tint, chip, Align::Center);
                        // Found inside: the line that has it says more
                        // than where the file is.
                        location = match &hit.line {
                            Some(line) => line.clone(),
                            None => hit.location(&self.home),
                        };
                        (hit.name(), Some(location.as_str()), None)
                    }
                };

                // What Enter does, on the selected row only.
                let selected = self.offset + row_index == self.selected;
                let mut right = content.right();
                if let Some(hint) = hint.filter(|_| selected) {
                    let text = Text::new(hint, base_size * type_scale::SMALL, colors.subtle);
                    let width = canvas.measure(&text).w;
                    if width < content.w * 0.45 {
                        canvas.text_in(&text, content, Align::End);
                        right -= width + space::MD;
                    }
                }

                let text_x = chip.right() + space::MD;
                let available = right - text_x;
                let name_text = Text::new(name, base_size, colors.text)
                    .medium()
                    .max_width(available);
                let name_width = canvas
                    .text_in(
                        &name_text,
                        Rect::new(text_x, content.y, available, content.h),
                        Align::Start,
                    )
                    .w;
                // What was typed, picked out where it matched the name.
                if !matches!(entry, Entry::Answer(_)) {
                    highlight_matches(
                        canvas, &pattern, name, base_size, accent, text_x, content, name_width,
                    );
                }
                if let Some(description) = description {
                    let x = text_x + name_width + space::MD;
                    let room = right - x;
                    if room > 60.0 {
                        let text =
                            Text::new(description, base_size * type_scale::SMALL, colors.muted)
                                .max_width(room);
                        canvas.text_in(
                            &text,
                            Rect::new(x, content.y, room, content.h),
                            Align::Start,
                        );
                    }
                }
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
        Paint::when(
            self.presence.is_animating(now)
                || self.highlight.is_animating(now)
                || self.list_height.is_animating(now),
        )
    }
}

/// How long the card takes to grow out of the bar's clock, and to go back
/// into it. Linear: [`Emergence`] shapes each part of the way.
const EMERGE: Transition = Transition::new(420, Easing::Linear);
const RETURN: Transition = Transition::new(300, Easing::Linear);
/// How much the clock island swells as the card leaves it, either side
/// and above and below.
const SWELL: (f32, f32) = (7.0, 3.0);

/// The card on its way out of the bar's clock island, at `p` of the way
/// there (`0` the island alone, `1` the card at rest). Going back, the
/// same way in reverse: the card gathers itself, then is drawn back in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Emergence {
    /// How far the island has swollen, `0..=1`: up as the card slides out
    /// from under it, and back as it goes.
    swell: f32,
    /// How far the card has dropped from under the island to its place:
    /// a little past `1` and back, as it settles.
    drop: f32,
    /// How far it has spread from the island's width to its own, the same.
    spread: f32,
    /// How solid the card is.
    solid: f32,
    /// How far it has turned from the island's colour to its own.
    tint: f32,
    /// How much of what the card holds shows.
    content: f32,
}

impl Emergence {
    fn at(p: f32) -> Self {
        let span = |from: f32, to: f32| ((p - from) / (to - from)).clamp(0.0, 1.0);
        Self {
            swell: Easing::OutCubic.apply(span(0.0, 0.25))
                * (1.0 - Easing::InOutCubic.apply(span(0.3, 0.7))),
            drop: settle(span(0.05, 0.7)),
            spread: settle(span(0.2, 0.95)),
            solid: Easing::OutCubic.apply(span(0.05, 0.15)),
            tint: Easing::InOutCubic.apply(span(0.2, 0.65)),
            content: Easing::OutCubic.apply(span(0.6, 1.0)),
        }
    }
}

/// Quick out, a few hundredths past the end, and back: how something
/// thrown settles into its place.
fn settle(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.0 * u.powi(3) + u.powi(2)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Draw the card coming out of `island` towards `card`, as `stage` has it:
/// the island swelling round its clock as the card slides out from under
/// it, drops into place and spreads, turning from the island's colour to
/// its own.
fn emerge(
    canvas: &mut Canvas<'_>,
    island: &Island,
    card: Rect,
    stage: &Emergence,
    colors: &enso_core::Palette,
) {
    let shore = colors.base.alpha(island.opacity);
    let clock = Rect::new(island.x, island.y, island.w, island.h);
    let swollen = Rect::new(
        clock.x - SWELL.0 * stage.swell,
        clock.y - SWELL.1 * stage.swell,
        clock.w + 2.0 * SWELL.0 * stage.swell,
        clock.h + 2.0 * SWELL.1 * stage.swell,
    );

    // The island, swollen: a ring round the bar's own, in its colour, so
    // its clock still shows. It reaches a pixel inside, so no seam shows
    // where the two meet.
    if stage.swell > 0.0 {
        let mut ring = Shape::new();
        pill(&mut ring, swollen, true);
        pill(&mut ring, clock.inset(1.0), false);
        canvas.fill_shape(&ring, shore);
    }
    if stage.solid <= 0.0 {
        return;
    }

    // It starts hidden under the island, as wide as its flat foot, so it
    // comes out from under the middle of it and not past its round ends.
    let foot = Rect::new(clock.x + clock.h / 2.0, clock.y, clock.w - clock.h, clock.h);
    let (across, down) = (stage.spread, stage.drop);
    let now = Rect::new(
        lerp(foot.x, card.x, across),
        lerp(foot.y, card.y, down),
        lerp(foot.w, card.w, across),
        lerp(foot.h, card.h, down),
    );
    let corner = lerp(foot.h / 2.0, radius::CARD, across.min(1.0)).min(now.h / 2.0);
    let alpha = lerp(f32::from(shore.a) / 255.0, 1.0, stage.tint);
    let fill = Rgba {
        a: 255,
        ..shore.mix(colors.surface, stage.tint)
    }
    .alpha(alpha);
    // Under the island: nothing of the card shows above its foot.
    let under = swollen.bottom();
    let hidden = now.y < under;
    if hidden {
        canvas.set_clip(Some(Rect::new(0.0, under, canvas.bounds().w, canvas.bounds().h)));
    }
    canvas.with_opacity(stage.solid, |canvas| {
        // Its shadow comes as it leaves the bar.
        let shadow = Shadow {
            opacity: Shadow::FLOATING.opacity * stage.tint,
            ..Shadow::FLOATING
        };
        canvas.shadow(now, corner, shadow);
        canvas.fill(now, corner, fill);
        canvas.stroke(
            now,
            corner,
            stroke::HAIRLINE,
            colors.highlight_med.alpha(stage.tint),
        );
    });
    if hidden {
        canvas.set_clip(None);
    }
}

/// Add a pill filling `rect` to `shape`, round clockwise or the other way:
/// a pill inside another, turning the other way, is a hole in it.
fn pill(shape: &mut Shape, rect: Rect, clockwise: bool) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = (rect.h.min(rect.w) / 2.0).max(0.0);
    let (left, right) = ((rect.x + r, rect.y + r), (rect.right() - r, rect.y + r));
    if clockwise {
        shape.move_to(left.0, rect.y);
        shape.arc_to(right, r, -FRAC_PI_2, PI);
        shape.arc_to(left, r, FRAC_PI_2, PI);
    } else {
        shape.move_to(left.0, rect.y);
        shape.arc_to(left, r, -FRAC_PI_2, -PI);
        shape.arc_to(right, r, FRAC_PI_2, -PI);
    }
    shape.close();
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

    #[test]
    fn the_card_grows_out_of_the_clock_and_settles_under_the_bar() {
        let rest = Emergence::at(1.0);
        assert_eq!(
            (rest.swell, rest.drop, rest.spread, rest.solid, rest.tint, rest.content),
            (0.0, 1.0, 1.0, 1.0, 1.0, 1.0)
        );
        let hidden = Emergence::at(0.0);
        assert_eq!(
            (hidden.swell, hidden.drop, hidden.spread, hidden.solid, hidden.content),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        let stages: Vec<Emergence> = (0..=100).map(|i| Emergence::at(i as f32 / 100.0)).collect();
        // It goes a little past its place, and only a little.
        for furthest in [
            stages.iter().map(|s| s.drop).fold(0.0, f32::max),
            stages.iter().map(|s| s.spread).fold(0.0, f32::max),
        ] {
            assert!(furthest > 1.0 && furthest < 1.05, "{furthest}");
        }
        // It drops before it spreads.
        assert!(stages[30].drop > stages[30].spread + 0.3);
        // The island swells while the card leaves it, then settles.
        assert!(stages[25].swell > 0.95);
        // What the card holds shows only once it has arrived.
        assert!(stages[..60].iter().all(|s| s.content == 0.0));
        assert!(stages[60..].iter().all(|s| s.drop >= 0.999 && s.spread >= 0.999));
    }

    /// The opening drawn over a bar, a frame at a time, to look at:
    /// `target/snapshots/launch-emerge.png`.
    #[test]
    fn the_card_opens_out_of_the_bars_clock() {
        let (width, height) = (800u32, 200u32);
        let island = Island {
            pid: 1,
            top: true,
            bar: 36.0,
            x: 270.0,
            y: 4.0,
            w: 260.0,
            h: 28.0,
            opacity: 0.9,
        };
        let mut launcher = launcher().on(None, Some(island));
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
            .results
            .get(launcher.selected)
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
        assert!(launcher.rows.is_empty());
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
        assert_eq!((launcher.selected, launcher.offset), (6, 1));
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.selected, launcher.offset), (5, 1));
        launcher.select(0);
        launcher.move_selection(Nav::Up);
        assert_eq!((launcher.selected, launcher.offset), (8, 3));
        launcher.move_selection(Nav::Down);
        assert_eq!((launcher.selected, launcher.offset), (0, 0));
    }

    #[test]
    fn renders_card_and_rows() {
        let mut launcher = launcher();
        type_text(&mut launcher, "e");
        launcher.move_selection(Nav::Down);
        let frame = settled(&mut launcher);

        assert_eq!(launcher.rows.len(), 6);
        assert!(
            launcher
                .rows
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
        launcher.select(3);
        launcher.set_apps(vec![app("Files", "")]);
        assert_eq!(selected_name(&launcher), Some("fish"));
        assert_eq!(launcher.selected, 2);

        let frame = settled(&mut launcher);
        assert_eq!(launcher.rows.len(), 4);
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
        assert!(launcher.rows.is_empty());
        assert_eq!(launcher.card.h, SEARCH_HEIGHT + ROW_HEIGHT + space::SM);
        snapshot(&frame, "launch-no-matches");
    }
}
