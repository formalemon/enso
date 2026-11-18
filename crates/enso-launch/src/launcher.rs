//! The launcher overlay: a search field, and a ranked list once you type.
//!
//! It opens as a bare field. Nothing is listed until the query has something
//! in it, so the first frame has almost nothing to draw, and the applications
//! themselves are gathered on a background thread while the field is already
//! on screen.

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use enso_core::{
    Context, Role, Transition, Tween,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App as UiApp, Button, Canvas, Cx, Event, Key, KeyEvent, Nav, Paint, PointerEvent,
    Presence, Proxy, Rect, SurfaceSpec, Text,
};
use serde::Deserialize;

use enso_fs::{
    apps::{self, App},
    fuzzy, open,
    search::{self as files, Hit, Job, Scope},
};

use crate::history::{self, History};

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
}

/// Background work that should be allowed to finish after the window has
/// closed — the file walk, so that its result is saved for next time.
pub type Background = Arc<Mutex<Vec<JoinHandle<()>>>>;

/// One row of the result list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// Index into `apps`.
    App(usize),
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
    query: String,
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
            generation: 0,
            jobs: None,
            background,
            toggle,
            ctx,
            config,
            apps: Vec::new(),
            loaded: false,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            offset: 0,
            highlight: Tween::new(0.0),
            list_height: Tween::new(0.0),
            card: Rect::default(),
            rows: Vec::new(),
        }
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
            Entry::App(index) => app_key(&self.apps[*index].id),
            Entry::File(hit) => file_key(&hit.path),
        })
    }

    /// Rebuild the list from its two halves. The row with identity `keep`
    /// stays selected if it is still listed; otherwise the selection stays
    /// where it is, or returns to the top when there was none to keep.
    fn compose(&mut self, keep: Option<&str>) {
        self.results = self
            .app_results
            .iter()
            .map(|index| Entry::App(*index))
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

    /// Nothing has been typed yet (spaces alone do not count).
    fn is_idle(&self) -> bool {
        self.query.trim().is_empty()
    }

    /// The query matched nothing, and that is final.
    fn has_no_matches(&self) -> bool {
        self.loaded && !self.is_idle() && self.results.is_empty()
    }

    fn visible_rows(&self) -> usize {
        self.results.len().min(self.config.max_results.max(1))
    }

    fn list_target(&self) -> f32 {
        match self.visible_rows() {
            // Room for the "no matches" line.
            0 if self.has_no_matches() => ROW_HEIGHT + space::SM,
            // Just the search field.
            0 => 0.0,
            rows => rows as f32 * ROW_HEIGHT + space::SM,
        }
    }

    /// Rank the applications against the current query. An empty query
    /// lists nothing.
    fn rank_apps(&mut self) {
        // Looking inside files is for files alone.
        if self.inside().is_some() {
            self.app_results.clear();
            return;
        }
        let now = history::now();
        let pattern = fuzzy::Pattern::new(&self.query);
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
    }

    /// What to look for inside files, if the query asks that: it starts
    /// with a quote (`"green tea`, or `"/te+a/"` for a pattern).
    fn inside(&self) -> Option<&str> {
        let text = self.query.trim().strip_prefix('"')?;
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
                query: self.query.trim().to_owned(),
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
    fn edit(&mut self, key: &KeyEvent) -> bool {
        match key.key {
            Key::Backspace => {
                self.query.pop();
            }
            Key::Char('u') if key.mods.ctrl => self.query.clear(),
            Key::Char('w') if key.mods.ctrl => {
                let trimmed = self.query.trim_end();
                let cut = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                self.query.truncate(cut);
            }
            _ => match key.text() {
                Some(c) => self.query.push(c),
                None => return false,
            },
        }
        self.search();
        true
    }

    /// Launch the selected application or open the selected file — or,
    /// with `reveal`, the folder the file is in.
    fn launch_selected(&mut self, reveal: bool, cx: &mut Cx<'_>) {
        let Some(key) = self.selected_key() else {
            return;
        };
        match &self.results[self.selected] {
            Entry::App(index) => {
                let app = &self.apps[*index];
                if let Err(err) = app.launch(&self.ctx.config.appearance.terminal_program()) {
                    eprintln!("enso-launch: cannot start {}: {err}", app.name);
                }
            }
            Entry::File(hit) => {
                let path = Path::new(&hit.path);
                let target = match path.parent() {
                    Some(parent) if reveal => parent,
                    _ => path,
                };
                if let Err(err) = open::open(target, &self.ctx.config.appearance.terminal_program())
                {
                    eprintln!("enso-launch: cannot open {}: {err}", target.display());
                }
            }
        }
        self.history.record(&key, history::now());
        self.presence.close(cx);
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

impl UiApp for Launcher {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-launch")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        // Gathering applications never holds up the first frame: nothing is
        // listed before a key is pressed, and that is ages away.
        if self.config.files {
            self.start_files(proxy.clone());
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
            Event::Key(key) => {
                if let Some(nav) = key.nav(false) {
                    self.move_selection(nav);
                } else {
                    match key.key {
                        Key::Escape => return self.presence.close(cx),
                        Key::Enter => return self.launch_selected(key.mods.ctrl, cx),
                        _ if !self.edit(&key) => return,
                        _ => {}
                    }
                }
                cx.redraw();
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
                    self.launch_selected(false, cx);
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

        // Dim what is behind so the card is the only thing asking for attention.
        // The scrim covers the whole screen, so it is written straight over
        // the empty canvas rather than blended in: on every frame of the
        // reveal that is most of the pixels there are.
        canvas.clear(colors.base.alpha(opacity::SCRIM * presence));

        let list_height = self.list_height.get(now);
        let width = (self.config.width as f32).min(bounds.w - 2.0 * space::LG);
        let card = Rect::new(
            (bounds.w - width) / 2.0,
            (bounds.h * 0.2).round(),
            width,
            SEARCH_HEIGHT + list_height,
        )
        .translate(0.0, (presence - 1.0) * space::MD);
        self.card = card;

        canvas.set_opacity(presence);
        canvas.card(card, colors.surface, colors.highlight_med);

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
        let typed = if self.query.is_empty() {
            canvas.text_in(
                &Text::new(self.placeholder(), title, colors.muted),
                field,
                Align::Start,
            );
            0.0
        } else {
            canvas
                .text_in(
                    &Text::new(&self.query, title, colors.text),
                    field,
                    Align::Start,
                )
                .w
        };
        let caret = Rect::new(
            field.x + typed + 1.0,
            field.y + (field.h - title * 1.3) / 2.0,
            2.0,
            title * 1.3,
        );
        canvas.fill(caret, 1.0, accent);

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
                let (name, description) = match entry {
                    Entry::App(index) => {
                        let app = &self.apps[*index];
                        // A letter chip stands in for an icon: no icon themes needed.
                        let tint = colors.get(chip_role(&app.name));
                        canvas.fill(chip, radius::CONTROL, tint.alpha(opacity::TINT + 0.06));
                        let initial: String = app
                            .name
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().collect())
                            .unwrap_or_default();
                        canvas.text_in(
                            &Text::new(&initial, base_size, tint).bold(),
                            chip,
                            Align::Center,
                        );
                        (app.name.as_str(), app.description.as_deref())
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
                        (hit.name(), Some(location.as_str()))
                    }
                };

                let text_x = chip.right() + space::MD;
                let available = content.right() - text_x;
                let name = Text::new(name, base_size, colors.text)
                    .medium()
                    .max_width(available);
                let name_width = canvas
                    .text_in(
                        &name,
                        Rect::new(text_x, content.y, available, content.h),
                        Align::Start,
                    )
                    .w;
                if let Some(description) = description {
                    let x = text_x + name_width + space::MD;
                    let room = content.right() - x;
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

        canvas.set_opacity(1.0);
        Paint::when(
            self.presence.is_animating(now)
                || self.highlight.is_animating(now)
                || self.list_height.is_animating(now),
        )
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

    /// A launcher whose applications have not arrived yet.
    fn unloaded() -> Launcher {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-launch-test"));
        Launcher::new(
            ctx,
            LaunchConfig {
                max_results: 6,
                ..LaunchConfig::default()
            },
            Background::default(),
            None,
        )
    }

    fn selected_name(launcher: &Launcher) -> Option<&str> {
        launcher
            .results
            .get(launcher.selected)
            .map(|entry| match entry {
                Entry::App(index) => launcher.apps[*index].name.as_str(),
                Entry::File(hit) => hit.name(),
            })
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

    fn snapshot(frame: &Offscreen, name: &str) {
        frame
            .save_png(format!(
                "{}/../../target/snapshots/{name}.png",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
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
            assert!(launcher.edit(&key(Key::Char(c), false)));
        }
    }

    fn names(launcher: &Launcher) -> Vec<&str> {
        launcher
            .results
            .iter()
            .map(|entry| match entry {
                Entry::App(index) => launcher.apps[*index].name.as_str(),
                Entry::File(hit) => hit.name(),
            })
            .collect()
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
        launcher.edit(&key(Key::Char('u'), true));
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
        assert_eq!(launcher.query, "fire");
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
        assert_eq!(launcher.query, "fi");
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
        launcher.edit(&key(Key::Char('u'), true));
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
        launcher.edit(&key(Key::Char('w'), true));
        assert_eq!(launcher.query, "web ");
        launcher.edit(&key(Key::Backspace, false));
        assert_eq!(launcher.query, "web");
        launcher.edit(&key(Key::Char('u'), true));
        assert_eq!(launcher.query, "");
        // Shortcuts are not typed into the field.
        assert!(!launcher.edit(&key(Key::Char('x'), true)));
        assert!(!launcher.edit(&key(Key::Tab, false)));
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
        // The scrim tints everything outside the card.
        assert!(frame.pixel(10, 10)[3] > 0 && frame.pixel(10, 10)[3] < 255);
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
        launcher.edit(&key(Key::Backspace, false));
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
