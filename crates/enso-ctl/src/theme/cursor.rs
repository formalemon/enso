//! The pointer: one cursor theme, at one size, for every program.
//!
//! A cursor theme is an XCursor theme — a directory holding `cursors/` —
//! under one of the `icons` directories (`~/.local/share/icons`,
//! `/usr/share/icons`, …). It is chosen in `config.toml` (`cursor`,
//! `cursor_size` under `[appearance]`) or with `enso-ctl cursor set`, which
//! records the choice in the state directory, beside the active theme.
//!
//! Programs learn which cursor to draw in three ways, and each is told:
//!
//! - **Niri** draws the pointer itself for its own surfaces and for every
//!   client that names a cursor shape rather than drawing one: enso's tools,
//!   Alacritty, Firefox, GTK 4. It reads the theme from `niri.kdl`, which
//!   [`niri`](super::niri) writes, and hands `XCURSOR_THEME` and
//!   `XCURSOR_SIZE` on to every program it starts.
//! - **GTK** reads `cursor-theme` and `cursor-size` from dconf (GTK 4 by way
//!   of the settings portal) and follows a change while running; GTK 3 off
//!   Wayland reads its `settings.ini` instead.
//! - **Everything else** that draws a cursor without being told falls back on
//!   the theme named `default`, so `~/.local/share/icons/default` is made to
//!   inherit the chosen one — unless that directory is somebody else's.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use enso_core::Context;

use super::{Host, Job, Outcome, Place, Report, keyfile, print_reports, replace};
use crate::cli::CursorCommand;

/// The choice made with `enso-ctl cursor set`, in the state directory.
pub const FILE: &str = "cursor";

/// The dconf keys, as GSettings' `org.gnome.desktop.interface` has them.
const DCONF_THEME: &str = "/org/gnome/desktop/interface/cursor-theme";
const DCONF_SIZE: &str = "/org/gnome/desktop/interface/cursor-size";

/// GTK 3's own keys, in `[Settings]` of its `settings.ini`.
const GTK_THEME: &str = "gtk-cursor-theme-name";
const GTK_SIZE: &str = "gtk-cursor-theme-size";

/// What marks the fallback theme as enso's to rewrite.
const OWN: &str = "enso-ctl";

/// Sizes a cursor can sensibly be drawn at, in logical pixels.
const SIZES: std::ops::RangeInclusive<u32> = 8..=256;

/// A cursor theme and the size to draw it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub name: String,
    pub size: u32,
}

impl Cursor {
    /// The cursor in use: the one chosen with `cursor set`, else the one in
    /// `config.toml`, else none.
    pub fn chosen(ctx: &Context) -> Option<Self> {
        fs::read_to_string(ctx.paths.state_dir.join(FILE))
            .ok()
            .and_then(|text| Self::parse(&text))
            .or_else(|| {
                let appearance = &ctx.config.appearance;
                let name = appearance.cursor.trim();
                (!name.is_empty()).then(|| Self {
                    name: name.to_owned(),
                    size: appearance.cursor_size,
                })
            })
    }

    /// `Bibata-Modern-Ice 24`: the name, and the size after the last space.
    fn parse(text: &str) -> Option<Self> {
        let (name, size) = text.trim().rsplit_once(' ')?;
        let size = size.parse().ok().filter(|size| SIZES.contains(size))?;
        let name = name.trim();
        (!name.is_empty()).then(|| Self {
            name: name.to_owned(),
            size,
        })
    }

    fn line(&self) -> String {
        format!("{} {}\n", self.name, self.size)
    }
}

/// The cursor themes installed on this host, by name, each with the `icons`
/// directory it is in. Where a name is in several, the first directory —
/// the user's own — wins, as it does for every program.
pub fn installed(host: &Host) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for dir in &host.icons {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `default` is not a theme of its own but a pointer to one.
            if name != "default"
                && entry.path().join("cursors").is_dir()
                && !found.iter().any(|(known, _)| *known == name)
            {
                found.push((name, dir.clone()));
            }
        }
    }
    found.sort_by_key(|(name, _)| name.to_lowercase());
    found
}

/// The installed theme of this name, spelled as installed: case is not
/// something to have to get right on the command line.
fn find(host: &Host, name: &str) -> Option<String> {
    let installed = installed(host);
    installed
        .iter()
        .find(|(known, _)| known == name)
        .or_else(|| {
            installed
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(name))
        })
        .map(|(known, _)| known.clone())
}

/// The cursor in use, if one is chosen and installed: what [`niri`] is told.
///
/// [`niri`]: super::niri
pub fn usable(job: &Job<'_>) -> Option<Cursor> {
    Cursor::chosen(job.ctx).filter(|cursor| find(job.host, &cursor.name).is_some())
}

/// Tell GTK, and the `default` theme, which cursor to use.
pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let host = job.host;
    let Some(cursor) = Cursor::chosen(job.ctx) else {
        return Ok(Outcome::absent("none chosen"));
    };
    if find(host, &cursor.name).is_none() {
        return Err(format!(
            "{} is not installed (looked in {})",
            cursor.name,
            searched(host)
        ));
    }
    let Cursor { name, size } = &cursor;

    if host.session {
        let size = i32::try_from(*size).unwrap_or(24);
        match enso_sys::dconf::set(&[
            (DCONF_THEME, enso_sys::dconf::Value::Str(name)),
            (DCONF_SIZE, enso_sys::dconf::Value::Int(size)),
        ]) {
            Ok(()) => {}
            Err(err) if err.is_unavailable() => {}
            Err(err) => return Err(format!("dconf: {err}")),
        }
    }

    let gtk_3 = host.config.join("gtk-3.0");
    if gtk_3.is_dir() {
        let file = gtk_3.join("settings.ini");
        let text = fs::read_to_string(&file).unwrap_or_default();
        let text = keyfile::set(&text, "Settings", GTK_THEME, name);
        let text = keyfile::set(&text, "Settings", GTK_SIZE, &size.to_string());
        replace(&file, &text)?;
    }

    let fallback = host.data.join("icons/default/index.theme");
    let outcome = Outcome::done(format!("{name}, {size} px"));
    Ok(if point_default(&fallback, name)? {
        outcome
    } else {
        outcome.step(
            format!(
                "{} is not enso's; for programs that ask for no theme, set in it:",
                host.tilde(&fallback)
            ),
            [format!("Inherits={name}")],
        )
    })
}

/// Make the `default` theme inherit `name`, if that file is enso's to write
/// (or does not exist yet). Whether it now does.
fn point_default(file: &Path, name: &str) -> Result<bool, String> {
    let current = fs::read_to_string(file).ok();
    if current.as_deref().is_some_and(|text| !text.contains(OWN)) {
        return Ok(keyfile::get(
            current.as_deref().unwrap_or_default(),
            "Icon Theme",
            "Inherits",
        ) == Some(name));
    }
    replace(
        file,
        &format!(
            "# Written by {OWN} for programs that ask for no cursor theme: it\n\
             # follows `{OWN} cursor set`. Edits are overwritten.\n\
             [Icon Theme]\n\
             Name=Default\n\
             Comment=The cursor chosen with {OWN}\n\
             Inherits={name}\n"
        ),
    )?;
    Ok(true)
}

/// The directories looked in, as a person writes them.
fn searched(host: &Host) -> String {
    let dirs: Vec<String> = host.icons.iter().map(|dir| host.tilde(dir)).collect();
    dirs.join(", ")
}

/// Run a `cursor` subcommand, writing to `out`. `places` are the configured
/// theme targets: the cursor goes to those of them that take one.
pub fn run(
    ctx: &Context,
    places: &[Place],
    command: &CursorCommand,
    out: &mut impl Write,
) -> Result<(), String> {
    run_on(&Host::of(ctx), ctx, places, command, out)
}

fn run_on(
    host: &Host,
    ctx: &Context,
    places: &[Place],
    command: &CursorCommand,
    out: &mut impl Write,
) -> Result<(), String> {
    let io = |err: std::io::Error| err.to_string();
    let chosen = Cursor::chosen(ctx);
    let wanted = match command {
        CursorCommand::List => return list(host, chosen.as_ref(), out).map_err(io),
        CursorCommand::Set(name, size) => {
            let size = size
                .or(chosen.as_ref().map(|cursor| cursor.size))
                .unwrap_or(ctx.config.appearance.cursor_size);
            let name = find(host, name).ok_or_else(|| {
                let known: Vec<String> =
                    installed(host).into_iter().map(|(name, _)| name).collect();
                if known.is_empty() {
                    format!(
                        "`{name}` is not installed, and no cursor theme is (looked in {})",
                        searched(host)
                    )
                } else {
                    format!("`{name}` is not installed; installed: {}", known.join(", "))
                }
            })?;
            Cursor { name, size }
        }
        CursorCommand::Size(size) => Cursor {
            size: *size,
            ..chosen.ok_or("no cursor chosen yet: `enso-ctl cursor set <NAME>` first")?
        },
    };
    if !SIZES.contains(&wanted.size) {
        return Err(format!(
            "a cursor size is from {} to {} pixels",
            SIZES.start(),
            SIZES.end()
        ));
    }
    replace(&ctx.paths.state_dir.join(FILE), &wanted.line())?;

    // Niri learns the cursor from its generated file; the rest from here.
    let theme = &ctx.theme;
    let reports: Vec<Report> = super::apply_on(
        host,
        ctx,
        theme,
        &places
            .iter()
            .copied()
            .filter(|place| matches!(place, Place::Niri | Place::Cursor))
            .collect::<Vec<_>>(),
    )
    .into_iter()
    // Setting the theme again is how the files are rewritten, not news.
    .filter(|report| report.place != "enso")
    .collect();
    print_reports(
        &format!("{}, {} px", wanted.name, wanted.size),
        &reports,
        false,
        out,
    )
    .map_err(io)?;
    match reports
        .iter()
        .filter(|report| report.result.is_err())
        .count()
    {
        0 => Ok(()),
        1 => Err("one place could not be given the cursor".to_owned()),
        n => Err(format!("{n} places could not be given the cursor")),
    }
}

/// Every installed cursor theme, the one in use marked.
fn list(host: &Host, chosen: Option<&Cursor>, out: &mut impl Write) -> std::io::Result<()> {
    let installed = installed(host);
    if installed.is_empty() {
        return writeln!(
            out,
            "no cursor themes installed (looked in {})",
            searched(host)
        );
    }
    let width = installed
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (name, dir) in &installed {
        let current = chosen.filter(|cursor| cursor.name == *name);
        let marker = if current.is_some() { "●" } else { " " };
        let size = current.map_or_else(String::new, |cursor| format!("  {} px", cursor.size));
        writeln!(out, "{marker} {name:<width$}  {}{size}", host.tilde(dir))?;
    }
    match chosen {
        None => writeln!(out, "\nnone chosen: each program draws its own default"),
        Some(cursor) if !installed.iter().any(|(name, _)| *name == cursor.name) => writeln!(
            out,
            "\n{} is chosen but not installed; programs draw their own default",
            cursor.name
        ),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    /// A cursor theme with nothing in it but the directory that makes it one.
    fn install_cursor(sandbox: &Sandbox, dir: &str, name: &str) {
        fs::create_dir_all(sandbox.root.join(dir).join(name).join("cursors")).unwrap();
    }

    fn run_to_string(sandbox: &Sandbox, command: CursorCommand) -> Result<String, String> {
        let mut out = Vec::new();
        run_on(&sandbox.host, &sandbox.ctx, &Place::ALL, &command, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn choices_are_read_back() {
        assert_eq!(
            Cursor::parse("Bibata-Modern-Ice 24\n"),
            Some(Cursor {
                name: "Bibata-Modern-Ice".into(),
                size: 24
            })
        );
        // A name may have spaces; the size is after the last.
        assert_eq!(
            Cursor::parse("My Cursors 32").map(|cursor| cursor.name),
            Some("My Cursors".into())
        );
        for bad in ["", "Bibata", "Bibata big", "Bibata 0", " 24"] {
            assert_eq!(Cursor::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nothing_chosen_is_nothing_done() {
        let sandbox = Sandbox::new("cursor-none");
        assert_eq!(Cursor::chosen(&sandbox.ctx), None);
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.absent, "{outcome:?}");
        assert!(!sandbox.root.join(".local/share/icons/default").exists());
    }

    #[test]
    fn lists_what_is_installed_the_users_own_first() {
        let sandbox = Sandbox::new("cursor-list");
        assert!(
            run_to_string(&sandbox, CursorCommand::List)
                .unwrap()
                .starts_with("no cursor themes installed")
        );
        install_cursor(&sandbox, ".local/share/icons", "Bibata-Modern-Ice");
        install_cursor(&sandbox, ".icons", "Bibata-Modern-Ice");
        install_cursor(&sandbox, ".icons", "Adwaita");
        // An icon theme without cursors, and the `default` pointer, are not
        // cursor themes to choose.
        fs::create_dir_all(sandbox.root.join(".local/share/icons/Papirus")).unwrap();
        install_cursor(&sandbox, ".local/share/icons", "default");

        let names: Vec<(String, String)> = installed(&sandbox.host)
            .into_iter()
            .map(|(name, dir)| (name, sandbox.host.tilde(&dir)))
            .collect();
        assert_eq!(
            names,
            [
                ("Adwaita".into(), "~/.icons".into()),
                ("Bibata-Modern-Ice".into(), "~/.local/share/icons".into()),
            ]
        );
        let out = run_to_string(&sandbox, CursorCommand::List).unwrap();
        assert!(
            out.ends_with("\nnone chosen: each program draws its own default\n"),
            "{out}"
        );
    }

    #[test]
    fn set_records_the_choice_and_tells_everyone() {
        let sandbox = Sandbox::new("cursor-set");
        install_cursor(&sandbox, ".local/share/icons", "Bibata-Modern-Ice");
        sandbox.write(
            ".config/gtk-3.0/settings.ini",
            "[Settings]\ngtk-application-prefer-dark-theme=true\n",
        );
        sandbox.write(
            ".config/niri/config.kdl",
            "include optional=true \"~/.local/state/enso/niri.kdl\"\n",
        );

        // Any case will do; the size is the configured one until given.
        let out = run_to_string(
            &sandbox,
            CursorCommand::Set("bibata-modern-ice".into(), None),
        )
        .unwrap();
        assert_eq!(
            out,
            "Bibata-Modern-Ice, 24 px\n  niri       colours updated\n  \
             cursor     Bibata-Modern-Ice, 24 px\n"
        );
        assert_eq!(
            sandbox.read(".local/state/enso/cursor"),
            "Bibata-Modern-Ice 24\n"
        );
        let niri = sandbox.read(".local/state/enso/niri.kdl");
        assert!(
            niri.contains(
                "cursor {\n    xcursor-theme \"Bibata-Modern-Ice\"\n    xcursor-size 24\n}"
            ),
            "{niri}"
        );
        assert_eq!(
            sandbox.read(".config/gtk-3.0/settings.ini"),
            "[Settings]\ngtk-application-prefer-dark-theme=true\n\
             gtk-cursor-theme-name=Bibata-Modern-Ice\ngtk-cursor-theme-size=24\n"
        );
        let fallback = sandbox.read(".local/share/icons/default/index.theme");
        assert!(
            fallback.contains("\nInherits=Bibata-Modern-Ice\n"),
            "{fallback}"
        );

        // The size alone, keeping the theme.
        run_to_string(&sandbox, CursorCommand::Size(32)).unwrap();
        assert_eq!(
            sandbox.read(".local/state/enso/cursor"),
            "Bibata-Modern-Ice 32\n"
        );
        assert!(
            sandbox
                .read(".local/state/enso/niri.kdl")
                .contains("xcursor-size 32")
        );
        assert!(
            sandbox
                .read(".config/gtk-3.0/settings.ini")
                .contains("gtk-cursor-theme-size=32")
        );

        let out = run_to_string(&sandbox, CursorCommand::List).unwrap();
        assert!(
            out.starts_with("● Bibata-Modern-Ice  ~/.local/share/icons  32 px"),
            "{out}"
        );
    }

    #[test]
    fn what_is_not_installed_is_refused() {
        let sandbox = Sandbox::new("cursor-missing");
        install_cursor(&sandbox, ".local/share/icons", "Adwaita");
        let err = run_to_string(&sandbox, CursorCommand::Set("Bibata".into(), None)).unwrap_err();
        assert_eq!(err, "`Bibata` is not installed; installed: Adwaita");
        assert!(!sandbox.root.join(".local/state/enso/cursor").exists());
        let err = run_to_string(&sandbox, CursorCommand::Size(32)).unwrap_err();
        assert!(err.starts_with("no cursor chosen yet"), "{err}");
        let err =
            run_to_string(&sandbox, CursorCommand::Set("Adwaita".into(), Some(1000))).unwrap_err();
        assert_eq!(err, "a cursor size is from 8 to 256 pixels");

        // Chosen in the config but gone from the disk: said, and Niri is not
        // sent looking for it.
        let mut sandbox = sandbox;
        sandbox.ctx.config.appearance.cursor = "Gone".into();
        let err = apply(&sandbox.job("rose-pine-moon")).unwrap_err();
        assert!(
            err.starts_with("Gone is not installed (looked in ~/.local/share/icons"),
            "{err}"
        );
        assert_eq!(usable(&sandbox.job("rose-pine-moon")), None);
    }

    #[test]
    fn a_default_theme_of_the_users_own_is_left_alone() {
        let mut sandbox = Sandbox::new("cursor-default");
        install_cursor(&sandbox, ".local/share/icons", "Bibata-Modern-Ice");
        sandbox.ctx.config.appearance.cursor = "Bibata-Modern-Ice".into();
        let mine = "[Icon Theme]\nInherits=Adwaita\n";
        sandbox.write(".local/share/icons/default/index.theme", mine);

        let step = apply(&sandbox.job("rose-pine-moon")).unwrap().step.unwrap();
        assert!(
            step.what
                .starts_with("~/.local/share/icons/default/index.theme is not enso's")
        );
        assert_eq!(step.lines, ["Inherits=Bibata-Modern-Ice"]);
        assert_eq!(sandbox.read(".local/share/icons/default/index.theme"), mine);

        // Once it says the same, there is nothing to ask.
        sandbox.write(
            ".local/share/icons/default/index.theme",
            "[Icon Theme]\nInherits=Bibata-Modern-Ice\n",
        );
        assert_eq!(apply(&sandbox.job("rose-pine-moon")).unwrap().step, None);
    }
}
