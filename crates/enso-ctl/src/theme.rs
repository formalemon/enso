//! `enso-ctl theme …` and the theme-application pipeline.
//!
//! Switching theme means applying one palette to several places. Each place
//! is a [`Target`]. The scaffold ships the first one — enso's own tools,
//! which follow a state file live — and lists the rest as planned so the
//! extension point is explicit.

use std::io::{IsTerminal, Write};

use enso_core::{Context, Rgba, Theme, theme};

use crate::cli::ThemeCommand;

/// Somewhere a theme gets applied.
pub trait Target {
    fn name(&self) -> &'static str;
    /// Apply `theme`, returning a short note on what changed.
    fn apply(&self, ctx: &Context, theme: &Theme) -> Result<String, String>;
}

/// enso's own tools: they watch the state directory and recolour instantly.
struct EnsoTools;

impl Target for EnsoTools {
    fn name(&self) -> &'static str {
        "enso"
    }

    fn apply(&self, ctx: &Context, theme: &Theme) -> Result<String, String> {
        theme::set_active(&ctx.paths, &theme.id).map_err(|err| err.to_string())?;
        Ok("running enso tools updated".to_owned())
    }
}

/// Targets that are implemented.
fn targets() -> Vec<Box<dyn Target>> {
    vec![Box::new(EnsoTools)]
}

/// Targets on the roadmap, in the order they will be built.
pub const PLANNED_TARGETS: &[&str] = &["niri", "alacritty", "neovim", "gtk", "qt", "flatpak"];

/// Apply a theme everywhere it can go. Returns one line per target.
pub fn apply(ctx: &Context, theme: &Theme) -> Result<Vec<String>, String> {
    targets()
        .iter()
        .map(|target| {
            target
                .apply(ctx, theme)
                .map(|note| format!("{}: {note}", target.name()))
                .map_err(|err| format!("{}: {err}", target.name()))
        })
        .collect()
}

/// Run a `theme` subcommand, writing to `out`.
pub fn run(ctx: &Context, command: &ThemeCommand, out: &mut impl Write) -> Result<(), String> {
    let color = std::io::stdout().is_terminal();
    let io = |err: std::io::Error| err.to_string();
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
        ThemeCommand::Set(id) => {
            let theme = lookup(ctx, id)?;
            set(ctx, theme, out)?;
        }
        ThemeCommand::Next => {
            let ids: Vec<&str> = ctx.themes.iter().map(|theme| theme.id.as_str()).collect();
            let current = ids.iter().position(|id| *id == ctx.theme.id).unwrap_or(0);
            let theme = lookup(ctx, ids[(current + 1) % ids.len()])?;
            set(ctx, theme, out)?;
        }
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

fn set(ctx: &Context, theme: &Theme, out: &mut impl Write) -> Result<(), String> {
    let notes = apply(ctx, theme)?;
    writeln!(out, "{} ({})", theme.name, theme.id).map_err(|err| err.to_string())?;
    for note in notes {
        writeln!(out, "  {note}").map_err(|err| err.to_string())?;
    }
    writeln!(out, "  not yet themed: {}", PLANNED_TARGETS.join(", "))
        .map_err(|err| err.to_string())?;
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

#[cfg(test)]
mod tests {
    use enso_core::Paths;

    use super::*;

    fn context(name: &str) -> Context {
        let root = std::env::temp_dir().join(format!("enso-ctl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Context::load_from(Paths::rooted(root))
    }

    fn run_to_string(ctx: &Context, command: ThemeCommand) -> Result<String, String> {
        let mut out = Vec::new();
        run(ctx, &command, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn lists_themes_and_marks_the_active_one() {
        let ctx = context("list");
        let out = run_to_string(&ctx, ThemeCommand::List).unwrap();
        assert_eq!(out.lines().count(), 3);
        let active: Vec<&str> = out.lines().filter(|line| line.starts_with('●')).collect();
        assert_eq!(active.len(), 1);
        assert!(active[0].contains("rose-pine-moon"));
    }

    #[test]
    fn set_persists_and_next_cycles() {
        let ctx = context("set");
        run_to_string(&ctx, ThemeCommand::Set("rose-pine-dawn".into())).unwrap();
        assert_eq!(theme::active(&ctx.paths).as_deref(), Some("rose-pine-dawn"));

        // `next` moves on from whatever is active, wrapping around.
        let ctx = Context::load_from(ctx.paths.clone());
        assert_eq!(
            run_to_string(&ctx, ThemeCommand::Current).unwrap().trim(),
            "rose-pine-dawn"
        );
        run_to_string(&ctx, ThemeCommand::Next).unwrap();
        assert_eq!(theme::active(&ctx.paths).as_deref(), Some("rose-pine-main"));
    }

    #[test]
    fn unknown_theme_is_an_error_that_lists_alternatives() {
        let ctx = context("unknown");
        let err = run_to_string(&ctx, ThemeCommand::Set("nope".into())).unwrap_err();
        assert!(err.contains("rose-pine-moon"), "{err}");
        assert_eq!(theme::active(&ctx.paths), None);
    }

    #[test]
    fn show_prints_every_role() {
        let ctx = context("show");
        let out = run_to_string(&ctx, ThemeCommand::Show(Some("rose-pine-main".into()))).unwrap();
        assert_eq!(out.lines().count(), 16);
        assert!(out.contains("highlight-high"));
        assert!(out.contains("#191724"));
    }
}
