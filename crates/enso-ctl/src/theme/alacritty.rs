//! Alacritty as a theme target: the palette as a file of Alacritty colours.
//!
//! Alacritty reloads its config, and every file that config imports, as soon
//! as one changes. So theming it is one generated file in enso's state
//! directory and one import in `alacritty.toml`:
//!
//! ```toml
//! [general]
//! import = ["~/.local/state/enso/alacritty.toml"]
//! ```
//!
//! Alacritty's own config wins over what it imports, so colours a person
//! sets there stay theirs. An import that did not exist when a terminal
//! started is not watched: terminals opened before the first `theme set`
//! pick the colours up when next started.

use std::{fs, path::PathBuf};

use enso_core::Palette;

use super::{Job, Outcome, files, mentions};

/// Name of the generated file inside the state directory.
const FILE: &str = "alacritty.toml";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let file = job.generate(FILE, &render(job))?;
    let host = job.host;
    let dir = host.config.join("alacritty");
    // Where Alacritty looks for its config, first found first used.
    let configs = [
        dir.join("alacritty.toml"),
        host.config.join("alacritty.toml"),
        host.home.join(".alacritty.toml"),
    ];
    let config = configs.iter().find(|path| path.is_file());
    if config.is_none() && !dir.is_dir() && !host.has_program("alacritty") {
        return Ok(Outcome::absent("not installed"));
    }

    // An import may sit in a file that is itself imported.
    let mut candidates: Vec<PathBuf> = files(&dir, &["toml"], 0);
    candidates.extend(configs.iter().skip(1).cloned());
    if mentions(&candidates, "#", &format!("enso/{FILE}")) {
        return Ok(Outcome::done("colours updated"));
    }

    let import = format!("import = [\"{}\"]", host.tilde(&file));
    let config = config.unwrap_or(&configs[0]);
    // A second `[general]` table would be an error: name the one there is.
    let has_general = fs::read_to_string(config)
        .is_ok_and(|text| text.lines().any(|line| line.trim() == "[general]"));
    Ok(if has_general {
        Outcome::done("colours written").step(
            format!(
                "to use them, add under [general] in {}:",
                host.tilde(config)
            ),
            [import],
        )
    } else {
        Outcome::done("colours written").step(
            format!("to use them, add to {}:", host.tilde(config)),
            ["[general]".to_owned(), import],
        )
    })
}

/// The palette as Alacritty's `[colors]`, mapped as Rosé Pine's own
/// Alacritty theme maps it.
pub fn render(job: &Job<'_>) -> String {
    let Palette {
        base,
        surface,
        overlay,
        muted,
        subtle,
        text,
        love,
        gold,
        rose,
        pine,
        foam,
        iris,
        highlight_med,
        highlight_high,
        ..
    } = job.colors();
    // The eight terminal colours; only black differs between the sets.
    let ansi = |black| {
        format!(
            "black = \"{black}\"\n\
             red = \"{love}\"\n\
             green = \"{pine}\"\n\
             yellow = \"{gold}\"\n\
             blue = \"{foam}\"\n\
             magenta = \"{iris}\"\n\
             cyan = \"{rose}\"\n\
             white = \"{text}\"\n"
        )
    };
    format!(
        r##"# {banner}

[colors.primary]
foreground = "{text}"
background = "{base}"
dim_foreground = "{subtle}"
bright_foreground = "{text}"

[colors.cursor]
text = "{text}"
cursor = "{highlight_high}"

[colors.vi_mode_cursor]
text = "{text}"
cursor = "{highlight_high}"

[colors.search.matches]
foreground = "{subtle}"
background = "{overlay}"

[colors.search.focused_match]
foreground = "{base}"
background = "{rose}"

[colors.hints.start]
foreground = "{subtle}"
background = "{surface}"

[colors.hints.end]
foreground = "{muted}"
background = "{surface}"

[colors.line_indicator]
foreground = "None"
background = "None"

[colors.footer_bar]
foreground = "{text}"
background = "{surface}"

[colors.selection]
text = "{text}"
background = "{highlight_med}"

[colors.normal]
{normal}
[colors.bright]
{bright}
[colors.dim]
{dim}"##,
        banner = job.banner(),
        normal = ansi(overlay),
        bright = ansi(muted),
        dim = ansi(muted),
    )
}

#[cfg(test)]
mod tests {
    use enso_core::Config;

    use super::{super::sandbox::Sandbox, *};

    /// Rosé Pine's published Alacritty theme for Moon, less its comments.
    const UPSTREAM_MOON: &str = r##"
[colors.primary]
foreground = "#e0def4"
background = "#232136"
dim_foreground = "#908caa"
bright_foreground = "#e0def4"

[colors.cursor]
text = "#e0def4"
cursor = "#56526e"

[colors.vi_mode_cursor]
text = "#e0def4"
cursor = "#56526e"

[colors.search.matches]
foreground = "#908caa"
background = "#393552"

[colors.search.focused_match]
foreground = "#232136"
background = "#ea9a97"

[colors.hints.start]
foreground = "#908caa"
background = "#2a273f"

[colors.hints.end]
foreground = "#6e6a86"
background = "#2a273f"

[colors.line_indicator]
foreground = "None"
background = "None"

[colors.footer_bar]
foreground = "#e0def4"
background = "#2a273f"

[colors.selection]
text = "#e0def4"
background = "#44415a"

[colors.normal]
black = "#393552"
red = "#eb6f92"
green = "#3e8fb0"
yellow = "#f6c177"
blue = "#9ccfd8"
magenta = "#c4a7e7"
cyan = "#ea9a97"
white = "#e0def4"

[colors.bright]
black = "#6e6a86"
red = "#eb6f92"
green = "#3e8fb0"
yellow = "#f6c177"
blue = "#9ccfd8"
magenta = "#c4a7e7"
cyan = "#ea9a97"
white = "#e0def4"

[colors.dim]
black = "#6e6a86"
red = "#eb6f92"
green = "#3e8fb0"
yellow = "#f6c177"
blue = "#9ccfd8"
magenta = "#c4a7e7"
cyan = "#ea9a97"
white = "#e0def4"
"##;

    #[test]
    fn moon_is_rose_pines_own_alacritty_theme() {
        let sandbox = Sandbox::new("alacritty-render");
        let moon = render(&sandbox.job("rose-pine-moon"));
        let (banner, colors) = moon.split_once('\n').unwrap();
        assert_eq!(
            banner,
            "# Written by `enso-ctl theme set` — Rosé Pine Moon (rose-pine-moon). \
             Edits are overwritten."
        );
        assert_eq!(colors, UPSTREAM_MOON);
    }

    /// Every theme renders to TOML — which is what Alacritty must parse —
    /// with nothing left unfilled.
    #[test]
    fn every_theme_renders_valid_toml() {
        let sandbox = Sandbox::new("alacritty-toml");
        for theme in sandbox.ctx.themes.iter() {
            let text = render(&sandbox.job(&theme.id));
            // enso's own config parser keeps unknown tables, so it will do
            // as a TOML parser here.
            Config::parse(&text, &theme.id).unwrap();
            assert!(!text.contains('{'), "{text}");
            assert!(text.contains(&format!("background = \"{}\"", theme.colors.base)));
        }
    }

    #[test]
    fn says_which_lines_to_add_until_it_is_imported() {
        let mut sandbox = Sandbox::new("alacritty-apply");
        let generated = ".local/state/enso/alacritty.toml";

        // Not installed: written, and nothing to say.
        assert!(apply(&sandbox.job("rose-pine-dawn")).unwrap().absent);
        assert_eq!(
            sandbox.read(generated),
            render(&sandbox.job("rose-pine-dawn"))
        );

        // Installed, never configured: the whole table, in the file
        // Alacritty would read.
        sandbox.install("alacritty");
        let step = apply(&sandbox.job("rose-pine-dawn")).unwrap().step.unwrap();
        assert_eq!(
            step.what,
            "to use them, add to ~/.config/alacritty/alacritty.toml:"
        );
        assert_eq!(
            step.lines,
            [
                "[general]",
                "import = [\"~/.local/state/enso/alacritty.toml\"]"
            ]
        );

        // A config that already has the table: only the key.
        sandbox.write(
            ".config/alacritty/alacritty.toml",
            "[general]\nlive_config_reload = true\n\
             # import = [\"~/.local/state/enso/alacritty.toml\"]\n",
        );
        let step = apply(&sandbox.job("rose-pine-dawn")).unwrap().step.unwrap();
        assert_eq!(
            step.what,
            "to use them, add under [general] in ~/.config/alacritty/alacritty.toml:"
        );
        assert_eq!(
            step.lines,
            ["import = [\"~/.local/state/enso/alacritty.toml\"]"]
        );

        // Imported, even from a file that is itself imported.
        sandbox.write(
            ".config/alacritty/look.toml",
            "[general]\nimport = [\"~/.local/state/enso/alacritty.toml\"]\n",
        );
        assert_eq!(
            apply(&sandbox.job("rose-pine-dawn")).unwrap(),
            Outcome::done("colours updated")
        );
    }
}
