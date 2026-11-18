//! `enso-ctl theme …` and the theme-application pipeline.
//!
//! Switching theme means taking one palette to several places. enso's own
//! tools come first: they follow a state file live. Every other program is
//! a [`Place`], and each is reached the way that program itself offers:
//!
//! - **Niri, Alacritty, Neovim** read a generated file — `niri.kdl`,
//!   `alacritty.toml`, `nvim.lua` in enso's state directory — which one line
//!   of their own config includes. Each notices when it changes.
//! - **GTK** is told dark or light through dconf, which is also what the
//!   settings portal hands to Qt 6, Firefox, and Electron; its palette is a
//!   generated stylesheet to import.
//! - **Fonts**, when families are named, go to fontconfig through a
//!   generated `fonts.conf` to include, and to GTK through dconf.
//! - **The cursor**, when one is chosen, goes to GTK through dconf and to
//!   whatever asks for no theme through the `default` one; Niri has it in
//!   `niri.kdl`, and passes it on to what it starts.
//! - **Firefox** gets a stylesheet for its own interface, to import from a
//!   profile's `userChrome.css`.
//! - **Qt** gets a colour scheme for qt5ct and qt6ct to pick.
//! - **Flatpak** is let read what GTK was given.
//!
//! enso never edits a config file a person writes by hand: it writes files of
//! its own and says which line brings them in. A program that is not
//! installed is passed over.

mod alacritty;
pub mod cursor;
mod firefox;
mod flatpak;
mod fonts;
mod gtk;
mod keyfile;
mod neovim;
mod niri;
mod qt;

use std::{
    env, fs,
    io::{IsTerminal, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use enso_core::{Context, Palette, Paths, Rgba, Theme, theme};
use serde::Deserialize;

use crate::cli::ThemeCommand;

/// A program a theme is taken to, besides enso's own tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Niri,
    Alacritty,
    Neovim,
    Gtk,
    Fonts,
    Cursor,
    Firefox,
    Qt,
    Flatpak,
}

impl Place {
    /// Every place, in the order they are applied: Flatpak passes on what
    /// GTK was given, so it comes after.
    pub const ALL: [Self; 9] = [
        Self::Niri,
        Self::Alacritty,
        Self::Neovim,
        Self::Gtk,
        Self::Fonts,
        Self::Cursor,
        Self::Firefox,
        Self::Qt,
        Self::Flatpak,
    ];

    /// The name used in `config.toml` and in reports.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Niri => "niri",
            Self::Alacritty => "alacritty",
            Self::Neovim => "neovim",
            Self::Gtk => "gtk",
            Self::Fonts => "fonts",
            Self::Cursor => "cursor",
            Self::Firefox => "firefox",
            Self::Qt => "qt",
            Self::Flatpak => "flatpak",
        }
    }

    fn apply(self, job: &Job<'_>) -> Result<Outcome, String> {
        match self {
            Self::Niri => niri::apply(job),
            Self::Alacritty => alacritty::apply(job),
            Self::Neovim => neovim::apply(job),
            Self::Gtk => gtk::apply(job),
            Self::Fonts => fonts::apply(job),
            Self::Cursor => cursor::apply(job),
            Self::Firefox => firefox::apply(job),
            Self::Qt => qt::apply(job),
            Self::Flatpak => flatpak::apply(job),
        }
    }
}

/// The machine around enso: where other programs keep their settings, and
/// which programs there are.
#[derive(Clone, Debug)]
pub struct Host {
    /// `$HOME`.
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`: every program's config directory.
    pub config: PathBuf,
    /// `$XDG_DATA_HOME`.
    pub data: PathBuf,
    /// The directories of `$PATH`.
    pub path: Vec<PathBuf>,
    /// Where icon and cursor themes are installed, the user's own first:
    /// `$XDG_DATA_HOME/icons`, `~/.icons`, then each of `$XDG_DATA_DIRS`.
    pub icons: Vec<PathBuf>,
    /// Whether this is the user's real session, whose services (dconf) may
    /// be spoken to.
    pub session: bool,
}

impl Host {
    /// The host a context lives on. A context loaded from somewhere other
    /// than the environment's own directories — a sandbox, a test — is on no
    /// host in particular: it finds no programs and reaches no services.
    pub fn of(ctx: &Context) -> Self {
        let parent = |dir: &Path| dir.parent().unwrap_or(dir).to_owned();
        let config = parent(&ctx.paths.config_dir);
        let data = ctx
            .paths
            .data_dirs
            .first()
            .map_or_else(|| config.clone(), |dir| parent(dir));
        let session = ctx.paths == Paths::from_env();
        let (home, path, shared) = if session {
            let shared = env::var_os("XDG_DATA_DIRS")
                .filter(|dirs| !dirs.is_empty())
                .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            (
                env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
                env::var_os("PATH")
                    .map(|path| env::split_paths(&path).collect())
                    .unwrap_or_default(),
                env::split_paths(&shared).collect(),
            )
        } else {
            (parent(&config), Vec::new(), Vec::new())
        };
        let icons = [data.join("icons"), home.join(".icons")]
            .into_iter()
            .chain(shared.into_iter().map(|dir: PathBuf| dir.join("icons")))
            .collect();
        Self {
            home,
            config,
            data,
            path,
            icons,
            session,
        }
    }

    /// Whether a program of this name is on `$PATH`.
    pub fn has_program(&self, name: &str) -> bool {
        self.path.iter().any(|dir| {
            fs::metadata(dir.join(name))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    }

    /// `path` as a person writes it: `~/…` when it is under the home
    /// directory. Every program themed here expands the `~` itself.
    pub fn tilde(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if !self.home.as_os_str().is_empty() => format!("~/{}", rest.display()),
            _ => path.display().to_string(),
        }
    }
}

/// One theme on its way to one host.
#[derive(Clone, Copy, Debug)]
pub struct Job<'a> {
    pub ctx: &'a Context,
    pub theme: &'a Theme,
    pub host: &'a Host,
}

impl Job<'_> {
    /// The theme's colours, opaque: other programs take `#rrggbb`.
    fn colors(&self) -> Palette {
        let mut colors = self.theme.colors.clone();
        for color in [
            &mut colors.base,
            &mut colors.surface,
            &mut colors.overlay,
            &mut colors.muted,
            &mut colors.subtle,
            &mut colors.text,
            &mut colors.love,
            &mut colors.gold,
            &mut colors.rose,
            &mut colors.pine,
            &mut colors.foam,
            &mut colors.iris,
            &mut colors.highlight_low,
            &mut colors.highlight_med,
            &mut colors.highlight_high,
        ] {
            color.a = 0xff;
        }
        colors
    }

    /// The configured accent, in this theme.
    fn accent(&self) -> Rgba {
        self.colors().get(self.ctx.config.appearance.accent)
    }

    /// Where a generated file of this name lives: the state directory,
    /// beside the active theme.
    fn generated(&self, name: &str) -> PathBuf {
        self.ctx.paths.state_dir.join(name)
    }

    /// Write a generated file, returning its path.
    fn generate(&self, name: &str, text: &str) -> Result<PathBuf, String> {
        let file = self.generated(name);
        replace(&file, text)?;
        Ok(file)
    }

    /// The first line of every generated file, less its comment marker.
    fn banner(&self) -> String {
        format!(
            "Written by `enso-ctl theme set` — {} ({}). Edits are overwritten.",
            self.theme.name, self.theme.id
        )
    }
}

/// Make `path` hold `text`, creating its directory. A reader never sees half
/// a file — it is written beside the target, then swapped in — and a file
/// that already says `text` is left alone, so nothing reloads for nothing.
fn replace(path: &Path, text: &str) -> Result<(), String> {
    if fs::read_to_string(path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    enso_core::disk::write_atomic(path, text.as_bytes(), enso_core::disk::SHARED)
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// The files directly in `dir` with one of these extensions, and — `depth`
/// levels down — those of its subdirectories.
fn files(dir: &Path, extensions: &[&str], depth: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            if depth > 0 {
                found.extend(files(&path, extensions, depth - 1));
            }
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.contains(&ext))
        {
            found.push(path);
        }
    }
    found
}

/// Whether some line of some file names `needle` without being a comment.
/// This is how a generated file is known to be included: its path can be
/// spelled several ways (`~`, absolute), so only its tail is looked for.
fn mentions(files: &[PathBuf], comment: &str, needle: &str) -> bool {
    files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .any(|text| {
            text.lines().any(|line| {
                let line = line.trim_start();
                !line.starts_with(comment) && line.contains(needle)
            })
        })
}

/// What became of a theme in one place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// What was done, in a few words.
    pub note: String,
    /// What is left for the user to do, once.
    pub step: Option<Step>,
    /// The program is not on this machine, so nothing needed doing.
    pub absent: bool,
}

/// One thing only the user can do: a line to add to a config of their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// What to do and where.
    pub what: String,
    /// The lines to add, if it comes down to lines.
    pub lines: Vec<String>,
}

impl Outcome {
    fn done(note: impl Into<String>) -> Self {
        Self {
            note: note.into(),
            ..Self::default()
        }
    }

    fn absent(note: impl Into<String>) -> Self {
        Self {
            note: note.into(),
            absent: true,
            ..Self::default()
        }
    }

    fn step<L: Into<String>>(
        mut self,
        what: impl Into<String>,
        lines: impl IntoIterator<Item = L>,
    ) -> Self {
        self.step = Some(Step {
            what: what.into(),
            lines: lines.into_iter().map(Into::into).collect(),
        });
        self
    }
}

/// How one place fared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub place: &'static str,
    pub result: Result<Outcome, String>,
}

/// Apply a theme to enso and to each of `places`. One place failing does not
/// stop the rest: every one is tried and reported.
pub fn apply(ctx: &Context, theme: &Theme, places: &[Place]) -> Vec<Report> {
    apply_on(&Host::of(ctx), ctx, theme, places)
}

fn apply_on(host: &Host, ctx: &Context, theme: &Theme, places: &[Place]) -> Vec<Report> {
    let job = Job { ctx, theme, host };
    // enso's own tools watch the state directory and recolour at once.
    let mut reports = vec![Report {
        place: "enso",
        result: theme::set_active(&ctx.paths, &theme.id)
            .map(|()| Outcome::done("running tools recoloured"))
            .map_err(|err| err.to_string()),
    }];
    // Each place once, however the list was written.
    let mut seen = Vec::new();
    for &place in places {
        if !seen.contains(&place) {
            seen.push(place);
            reports.push(Report {
                place: place.name(),
                result: place.apply(&job),
            });
        }
    }
    reports
}

/// The places that could not be themed, as one message.
pub fn failure(reports: &[Report]) -> Option<String> {
    let failed: Vec<String> = reports
        .iter()
        .filter_map(|report| {
            let err = report.result.as_ref().err()?;
            Some(format!("{}: {err}", report.place))
        })
        .collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}

/// Run a `theme` subcommand, writing to `out`.
pub fn run(
    ctx: &Context,
    places: &[Place],
    command: &ThemeCommand,
    out: &mut impl Write,
) -> Result<(), String> {
    run_on(&Host::of(ctx), ctx, places, command, out)
}

fn run_on(
    host: &Host,
    ctx: &Context,
    places: &[Place],
    command: &ThemeCommand,
    out: &mut impl Write,
) -> Result<(), String> {
    let color = std::io::stdout().is_terminal();
    let io = |err: std::io::Error| err.to_string();
    let set = |theme: &Theme, everything: bool, out: &mut dyn Write| {
        let reports = apply_on(host, ctx, theme, places);
        let title = format!("{} ({})", theme.name, theme.id);
        print_reports(&title, &reports, everything, out).map_err(io)?;
        match reports
            .iter()
            .filter(|report| report.result.is_err())
            .count()
        {
            0 => Ok(()),
            1 => Err("one place could not be themed".to_owned()),
            n => Err(format!("{n} places could not be themed")),
        }
    };
    match command {
        ThemeCommand::List => {
            for theme in ctx.themes.iter() {
                let marker = if theme.id == ctx.theme.id { "●" } else { " " };
                let preview = preview(theme, color);
                writeln!(
                    out,
                    "{marker} {:<18} {preview}{}  ({})",
                    theme.id,
                    theme.name,
                    theme.variant.name()
                )
                .map_err(io)?;
            }
        }
        ThemeCommand::Current => writeln!(out, "{}", ctx.theme.id).map_err(io)?,
        ThemeCommand::Set(id) => set(lookup(ctx, id)?, false, out)?,
        ThemeCommand::Next => {
            let ids: Vec<&str> = ctx.themes.iter().map(|theme| theme.id.as_str()).collect();
            let current = ids.iter().position(|id| *id == ctx.theme.id).unwrap_or(0);
            set(lookup(ctx, ids[(current + 1) % ids.len()])?, false, out)?;
        }
        // The same as setting the active theme again, and a look at every
        // place: the ones passed over are listed too.
        ThemeCommand::Apply => set(&ctx.theme, true, out)?,
        ThemeCommand::Show(id) => {
            let theme = match id {
                Some(id) => lookup(ctx, id)?,
                None => &ctx.theme,
            };
            writeln!(
                out,
                "{} ({}, {})",
                theme.name,
                theme.id,
                theme.variant.name()
            )
            .map_err(io)?;
            for (role, rgba) in theme.colors.iter() {
                writeln!(out, "  {}{:<15} {rgba}", swatch(rgba, color), role.name()).map_err(io)?;
            }
        }
        ThemeCommand::Dirs => {
            writeln!(out, "built-in themes are compiled into every enso tool").map_err(io)?;
            for dir in ctx.paths.theme_dirs().iter().rev() {
                let note = if dir.is_dir() { "" } else { "  (not created)" };
                writeln!(out, "{}{note}", dir.display()).map_err(io)?;
            }
        }
    }
    Ok(())
}

/// Width of the column of place names.
const NAMES: usize = 10;

/// What was applied and, line by line, where it went. Places whose program
/// is not installed are left out unless `everything` is asked for.
fn print_reports(
    title: &str,
    reports: &[Report],
    everything: bool,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    writeln!(out, "{title}")?;
    for report in reports {
        let place = report.place;
        match &report.result {
            Err(err) => writeln!(out, "  {place:<NAMES$} failed: {err}")?,
            Ok(outcome) if outcome.absent && !everything => {}
            Ok(outcome) => {
                writeln!(out, "  {place:<NAMES$} {}", outcome.note)?;
                if let Some(step) = &outcome.step {
                    writeln!(out, "  {:<NAMES$} {}", "", step.what)?;
                    for line in &step.lines {
                        writeln!(out, "  {:<NAMES$}     {line}", "")?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn lookup<'a>(ctx: &'a Context, id: &str) -> Result<&'a Theme, String> {
    ctx.themes.get(id).ok_or_else(|| {
        let known: Vec<&str> = ctx.themes.iter().map(|theme| theme.id.as_str()).collect();
        format!("unknown theme `{id}`; available: {}", known.join(", "))
    })
}

/// A two-cell colour block using 24-bit ANSI colour, or nothing when piped.
fn swatch(color: Rgba, enabled: bool) -> String {
    if enabled {
        format!("\x1b[48;2;{};{};{}m  \x1b[0m ", color.r, color.g, color.b)
    } else {
        String::new()
    }
}

fn preview(theme: &Theme, enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    let c = &theme.colors;
    let blocks: String = [
        c.base, c.surface, c.love, c.gold, c.rose, c.pine, c.foam, c.iris,
    ]
    .into_iter()
    .map(|color| format!("\x1b[48;2;{};{};{}m  ", color.r, color.g, color.b))
    .collect();
    format!("{blocks}\x1b[0m  ")
}

/// A scratch host for the tests of this module and of each place: a home
/// directory with nothing installed and no session to speak to.
#[cfg(test)]
pub(crate) mod sandbox {
    use super::*;

    pub struct Sandbox {
        pub root: PathBuf,
        pub ctx: Context,
        pub host: Host,
    }

    impl Sandbox {
        /// An empty home under the temp directory, laid out like a real one:
        /// `.config/enso`, `.local/state/enso`, `.local/share/enso`.
        pub fn new(name: &str) -> Self {
            let root = env::temp_dir().join(format!("enso-theme-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let paths = Paths {
                config_dir: root.join(".config/enso"),
                state_dir: root.join(".local/state/enso"),
                cache_dir: root.join(".cache/enso"),
                data_dirs: vec![root.join(".local/share/enso")],
            };
            let ctx = Context::load_from(paths);
            let host = Host::of(&ctx);
            assert!(!host.session, "a sandbox must not reach the real session");
            let host = Host {
                home: root.clone(),
                icons: vec![root.join(".local/share/icons"), root.join(".icons")],
                ..host
            };
            Self { root, ctx, host }
        }

        pub fn theme(&self, id: &str) -> &Theme {
            self.ctx.themes.get(id).unwrap()
        }

        pub fn job<'a>(&'a self, id: &str) -> Job<'a> {
            Job {
                ctx: &self.ctx,
                theme: self.theme(id),
                host: &self.host,
            }
        }

        /// Put a program of this name on the sandbox's `$PATH`.
        pub fn install(&mut self, program: &str) {
            let bin = self.root.join("bin");
            fs::create_dir_all(&bin).unwrap();
            let file = bin.join(program);
            fs::write(&file, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
            if !self.host.path.contains(&bin) {
                self.host.path.push(bin);
            }
        }

        /// Write a file under the sandbox's home, creating its directory.
        pub fn write(&self, path: &str, text: &str) -> PathBuf {
            let file = self.root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, text).unwrap();
            file
        }

        pub fn read(&self, path: &str) -> String {
            fs::read_to_string(self.root.join(path)).unwrap()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{sandbox::Sandbox, *};

    fn run_to_string(sandbox: &Sandbox, command: ThemeCommand) -> Result<String, String> {
        let mut out = Vec::new();
        run_on(&sandbox.host, &sandbox.ctx, &Place::ALL, &command, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn lists_themes_and_marks_the_active_one() {
        let sandbox = Sandbox::new("list");
        let out = run_to_string(&sandbox, ThemeCommand::List).unwrap();
        assert_eq!(out.lines().count(), 3);
        let active: Vec<&str> = out.lines().filter(|line| line.starts_with('●')).collect();
        assert_eq!(active.len(), 1);
        assert!(active[0].contains("rose-pine-moon"));
    }

    #[test]
    fn set_persists_and_next_cycles() {
        let mut sandbox = Sandbox::new("set");
        run_to_string(&sandbox, ThemeCommand::Set("rose-pine-dawn".into())).unwrap();
        assert_eq!(
            theme::active(&sandbox.ctx.paths).as_deref(),
            Some("rose-pine-dawn")
        );

        // `next` moves on from whatever is active, wrapping around.
        sandbox.ctx = Context::load_from(sandbox.ctx.paths.clone());
        assert_eq!(
            run_to_string(&sandbox, ThemeCommand::Current)
                .unwrap()
                .trim(),
            "rose-pine-dawn"
        );
        run_to_string(&sandbox, ThemeCommand::Next).unwrap();
        assert_eq!(
            theme::active(&sandbox.ctx.paths).as_deref(),
            Some("rose-pine-main")
        );
    }

    #[test]
    fn unknown_theme_is_an_error_that_lists_alternatives() {
        let sandbox = Sandbox::new("unknown");
        let err = run_to_string(&sandbox, ThemeCommand::Set("nope".into())).unwrap_err();
        assert!(err.contains("rose-pine-moon"), "{err}");
        assert_eq!(theme::active(&sandbox.ctx.paths), None);
    }

    #[test]
    fn show_prints_every_role() {
        let sandbox = Sandbox::new("show");
        let out =
            run_to_string(&sandbox, ThemeCommand::Show(Some("rose-pine-main".into()))).unwrap();
        assert_eq!(out.lines().count(), 16);
        assert!(out.contains("highlight-high"));
        assert!(out.contains("#191724"));
    }

    /// On a machine with none of the programs, a switch says only that enso
    /// changed — and still leaves every generated file ready for the day one
    /// of them is installed.
    #[test]
    fn a_bare_machine_is_themed_quietly() {
        let sandbox = Sandbox::new("bare");
        let out = run_to_string(&sandbox, ThemeCommand::Set("rose-pine-dawn".into())).unwrap();
        assert_eq!(
            out,
            "Rosé Pine Dawn (rose-pine-dawn)\n  enso       running tools recoloured\n"
        );
        for file in [
            "theme",
            "niri.kdl",
            "alacritty.toml",
            "nvim.lua",
            "firefox.css",
        ] {
            assert!(
                sandbox.ctx.paths.state_dir.join(file).is_file(),
                "{file} was not written"
            );
        }
        // Nothing was left behind beside them.
        let stray: Vec<_> = files(&sandbox.ctx.paths.state_dir, &["tmp"], 0);
        assert!(stray.is_empty(), "{stray:?}");
    }

    /// `apply` goes over every place, installed or not, and says what each
    /// still needs.
    #[test]
    fn apply_reports_every_place() {
        let mut sandbox = Sandbox::new("apply");
        sandbox.install("alacritty");
        sandbox.write(".config/niri/config.kdl", "// nothing yet\n");
        let out = run_to_string(&sandbox, ThemeCommand::Apply).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Rosé Pine Moon (rose-pine-moon)");
        for place in Place::ALL {
            assert!(
                lines
                    .iter()
                    .any(|line| line.starts_with(&format!("  {:<10} ", place.name()))),
                "{} is missing from:\n{out}",
                place.name()
            );
        }
        assert!(out.contains("include optional=true \"~/.local/state/enso/niri.kdl\""));
        assert!(out.contains("import = [\"~/.local/state/enso/alacritty.toml\"]"));
        assert!(out.contains("  flatpak    not installed"), "{out}");
    }

    /// A place that cannot be written is reported, the others still go, and
    /// the command fails.
    #[test]
    fn one_failure_does_not_stop_the_rest() {
        let mut sandbox = Sandbox::new("failure");
        sandbox.install("qt6ct");
        // A file where qt6ct's config directory should be.
        sandbox.write(".config/qt6ct", "");
        sandbox.write(
            ".config/niri/config.kdl",
            "include optional=true \"~/.local/state/enso/niri.kdl\"\n",
        );

        let reports = apply_on(
            &sandbox.host,
            &sandbox.ctx,
            sandbox.theme("rose-pine-dawn"),
            &[Place::Qt, Place::Niri, Place::Niri],
        );
        let places: Vec<&str> = reports.iter().map(|report| report.place).collect();
        assert_eq!(places, ["enso", "qt", "niri"]);
        assert!(reports[1].result.is_err());
        assert_eq!(reports[2].result, Ok(Outcome::done("colours updated")));
        assert!(failure(&reports).unwrap().starts_with("qt: "));

        let mut out = Vec::new();
        let err = run_on(
            &sandbox.host,
            &sandbox.ctx,
            &[Place::Qt, Place::Niri],
            &ThemeCommand::Set("rose-pine-dawn".into()),
            &mut out,
        )
        .unwrap_err();
        assert_eq!(err, "one place could not be themed");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("  qt         failed: "), "{out}");
        assert!(out.contains("  niri       colours updated"), "{out}");
    }

    #[test]
    fn generated_files_are_left_alone_when_unchanged() {
        let sandbox = Sandbox::new("unchanged");
        let job = sandbox.job("rose-pine-moon");
        let file = job.generate("probe.txt", "one\n").unwrap();
        let written = fs::metadata(&file).unwrap().modified().unwrap();
        // The same text again does not touch the file.
        job.generate("probe.txt", "one\n").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), written);
        job.generate("probe.txt", "two\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two\n");
    }

    #[test]
    fn paths_are_spelled_from_home() {
        let sandbox = Sandbox::new("tilde");
        let host = &sandbox.host;
        assert_eq!(
            host.tilde(&sandbox.ctx.paths.state_dir.join("niri.kdl")),
            "~/.local/state/enso/niri.kdl"
        );
        assert_eq!(host.tilde(Path::new("/etc/xdg")), "/etc/xdg");
        assert!(!host.has_program("alacritty"));
    }

    #[test]
    fn colours_with_alpha_go_out_opaque() {
        let mut sandbox = Sandbox::new("opaque");
        let mut theme = sandbox.theme("rose-pine-moon").clone();
        theme.colors.rose = Rgba::new(0xea, 0x9a, 0x97, 0x80);
        sandbox.ctx.theme = theme.clone();
        let job = Job {
            ctx: &sandbox.ctx,
            theme: &theme,
            host: &sandbox.host,
        };
        assert_eq!(job.accent().to_string(), "#ea9a97");
    }
}
