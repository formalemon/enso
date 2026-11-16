//! Niri as a theme target: the palette, written as a piece of Niri config.
//!
//! Niri reads its colours from its own config file, and reloads that file —
//! and everything it includes — when any of them changes. So theming Niri is
//! one generated file in enso's state directory and one line in Niri's
//! config that includes it:
//!
//! ```kdl
//! include optional=true "~/.local/state/enso/niri.kdl"
//! ```
//!
//! Put the line after any colours of your own: later settings win, and
//! `optional` keeps the config valid before the first `theme set`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use enso_core::{Context, Theme, style::radius};

use crate::theme::Target;

/// Name of the generated file inside the state directory.
const FILE: &str = "niri.kdl";

pub struct Niri;

impl Target for Niri {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn apply(&self, ctx: &Context, theme: &Theme) -> Result<String, String> {
        let file = ctx.paths.state_dir.join(FILE);
        let io = |path: &Path, err: std::io::Error| format!("{}: {err}", path.display());
        fs::create_dir_all(&ctx.paths.state_dir).map_err(|err| io(&ctx.paths.state_dir, err))?;
        // Niri must never read half a file: write beside it, then swap.
        let tmp = ctx.paths.state_dir.join(".niri.kdl.tmp");
        fs::write(&tmp, render(theme)).map_err(|err| io(&tmp, err))?;
        fs::rename(&tmp, &file).map_err(|err| io(&file, err))?;

        Ok(match config_dir(ctx) {
            Some(dir) if includes(&dir) => "colours updated".to_owned(),
            Some(_) => format!(
                "colours written; to use them, add to Niri's config:\n      \
                 include optional=true \"{}\"",
                file.display()
            ),
            None => format!(
                "no Niri config found; colours written to {}",
                file.display()
            ),
        })
    }
}

/// Niri's config directory: enso's sibling under `$XDG_CONFIG_HOME`.
fn config_dir(ctx: &Context) -> Option<PathBuf> {
    let dir = ctx.paths.config_dir.parent()?.join("niri");
    dir.is_dir().then_some(dir)
}

/// Whether some file of Niri's config already includes the generated one.
/// The path can be spelled several ways (`~`, absolute), so only its tail is
/// looked for.
fn includes(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "kdl"))
        .filter_map(|path| fs::read_to_string(path).ok())
        .any(|text| {
            text.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("include") && line.contains(&format!("enso/{FILE}"))
            })
        })
}

/// The palette as Niri config: only colours, so that gaps, widths, and
/// everything else stay the user's own.
pub fn render(theme: &Theme) -> String {
    let c = &theme.colors;
    let (base, accent, quiet, urgent) = (c.base, c.iris, c.highlight_med, c.love);
    // A shadow that reads on a dark desktop is heavy on a light one.
    let shadow = if theme.is_dark() {
        "#00000066"
    } else {
        "#00000033"
    };
    let corner = radius::PANEL;
    format!(
        r#"// Written by `enso-ctl theme set` — {name} ({id}). Edits are overwritten.

layout {{
    background-color "{base}"

    focus-ring {{
        active-color "{accent}"
        inactive-color "{quiet}"
        urgent-color "{urgent}"
    }}

    border {{
        active-color "{accent}"
        inactive-color "{quiet}"
        urgent-color "{urgent}"
    }}

    shadow {{
        color "{shadow}"
    }}

    tab-indicator {{
        active-color "{accent}"
        inactive-color "{quiet}"
        urgent-color "{urgent}"
    }}

    insert-hint {{
        color "{accent}66"
    }}
}}

overview {{
    backdrop-color "{base}"
}}

recent-windows {{
    highlight {{
        active-color "{accent}"
        urgent-color "{urgent}"
        corner-radius {corner}
    }}
}}
"#,
        name = theme.name,
        id = theme.id,
    )
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;

    use super::*;

    fn context(name: &str) -> (PathBuf, Context) {
        let root = std::env::temp_dir().join(format!("enso-niri-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // `Paths::rooted` puts enso's config at `<root>/config`; give it the
        // shape of a real `~/.config/enso` so Niri's has somewhere to sit.
        let mut paths = Paths::rooted(&root);
        paths.config_dir = root.join("config/enso");
        (root, Context::load_from(paths))
    }

    #[test]
    fn renders_the_palette() {
        let (_root, ctx) = context("render");
        let moon = render(ctx.themes.get("rose-pine-moon").unwrap());
        assert!(moon.contains(r##"background-color "#232136""##), "{moon}");
        assert!(moon.contains(r##"active-color "#c4a7e7""##));
        assert!(moon.contains(r##"color "#c4a7e766""##));
        assert!(moon.contains(r##"color "#00000066""##));
        let dawn = render(ctx.themes.get("rose-pine-dawn").unwrap());
        assert!(dawn.contains(r##"background-color "#faf4ed""##), "{dawn}");
        assert!(dawn.contains(r##"color "#00000033""##));
        // Balanced braces: the cheapest check that Niri can read it.
        assert_eq!(moon.matches('{').count(), moon.matches('}').count());
    }

    #[test]
    fn writes_the_file_and_says_what_is_left_to_do() {
        let (root, ctx) = context("apply");
        let theme = ctx.themes.get("rose-pine-dawn").unwrap();
        let file = ctx.paths.state_dir.join(FILE);

        // No Niri config at all: the file is written all the same.
        let note = Niri.apply(&ctx, theme).unwrap();
        assert!(note.starts_with("no Niri config found"), "{note}");
        assert_eq!(fs::read_to_string(&file).unwrap(), render(theme));

        // A config that does not include it yet: say which line to add.
        let niri = root.join("config/niri");
        fs::create_dir_all(&niri).unwrap();
        fs::write(niri.join("config.kdl"), "// include \"enso/niri.kdl\"\n").unwrap();
        let note = Niri.apply(&ctx, theme).unwrap();
        assert!(note.contains("include optional=true"), "{note}");

        // Included from any file of the config: nothing more to say.
        fs::write(
            niri.join("theme.kdl"),
            "include optional=true \"~/.local/state/enso/niri.kdl\"\n",
        )
        .unwrap();
        assert_eq!(Niri.apply(&ctx, theme).unwrap(), "colours updated");
        fs::remove_dir_all(root).unwrap();
    }
}
