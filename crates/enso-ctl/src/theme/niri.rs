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
//!
//! The file carries the cursor too, when one is chosen and installed (see
//! [`cursor`](super::cursor)): Niri draws it, and hands it on to every
//! program it starts. Niri merges the `cursor` section across files, so a
//! `hide-when-typing` of your own stays — but an `xcursor-theme` of your own
//! included after this file wins over it.

use std::path::Path;

use enso_core::{Theme, style::radius};

use super::{Job, Outcome, cursor, files, mentions};

/// Name of the generated file inside the state directory.
const FILE: &str = "niri.kdl";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let file = job.generate(FILE, &render(job))?;
    let host = job.host;
    let dir = host.config.join("niri");
    Ok(if includes(&dir) {
        Outcome::done("colours updated")
    } else if dir.is_dir() {
        Outcome::done("colours written").step(
            format!(
                "to use them, add to {}:",
                host.tilde(&dir.join("config.kdl"))
            ),
            [format!("include optional=true \"{}\"", host.tilde(&file))],
        )
    } else if host.has_program("niri") {
        // Niri is running on its built-in config; a config holding only an
        // include would take its key bindings away, so none is suggested.
        Outcome::done(format!(
            "no Niri config found; colours written to {}",
            host.tilde(&file)
        ))
    } else {
        Outcome::absent("not installed")
    })
}

/// Whether some file of Niri's config already includes the generated one.
fn includes(dir: &Path) -> bool {
    mentions(&files(dir, &["kdl"], 0), "//", &format!("enso/{FILE}"))
}

/// The palette as Niri config: only colours (and the cursor), so that gaps,
/// widths, and everything else stay the user's own.
pub fn render(job: &Job<'_>) -> String {
    let theme: &Theme = job.theme;
    let c = job.colors();
    let (base, accent, quiet, urgent) = (c.base, c.iris, c.highlight_med, c.love);
    // A shadow that reads on a dark desktop is heavy on a light one.
    let shadow = if theme.is_dark() {
        "#00000066"
    } else {
        "#00000033"
    };
    let corner = radius::PANEL;
    let pointer = cursor::usable(job).map_or_else(String::new, |cursor| {
        format!(
            "\ncursor {{\n    xcursor-theme \"{}\"\n    xcursor-size {}\n}}\n",
            cursor.name.replace('\\', "\\\\").replace('"', "\\\""),
            cursor.size
        )
    });
    format!(
        r#"// {banner}

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
{pointer}"#,
        banner = job.banner(),
    )
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    #[test]
    fn renders_the_palette() {
        let sandbox = Sandbox::new("niri-render");
        let moon = render(&sandbox.job("rose-pine-moon"));
        assert!(moon.starts_with("// Written by `enso-ctl theme set` — Rosé Pine Moon"));
        assert!(moon.contains(r##"background-color "#232136""##), "{moon}");
        assert!(moon.contains(r##"active-color "#c4a7e7""##));
        assert!(moon.contains(r##"color "#c4a7e766""##));
        assert!(moon.contains(r##"color "#00000066""##));
        let dawn = render(&sandbox.job("rose-pine-dawn"));
        assert!(dawn.contains(r##"background-color "#faf4ed""##), "{dawn}");
        assert!(dawn.contains(r##"color "#00000033""##));
        // Balanced braces: the cheapest check that Niri can read it.
        assert_eq!(moon.matches('{').count(), moon.matches('}').count());
    }

    #[test]
    fn writes_the_file_and_says_what_is_left_to_do() {
        let mut sandbox = Sandbox::new("niri-apply");
        let generated = ".local/state/enso/niri.kdl";

        // No Niri at all: the file is written all the same, and nothing said.
        let outcome = apply(&sandbox.job("rose-pine-dawn")).unwrap();
        assert!(outcome.absent);
        assert_eq!(
            sandbox.read(generated),
            render(&sandbox.job("rose-pine-dawn"))
        );

        // Niri without a config of the user's own: nowhere to add a line.
        sandbox.install("niri");
        let outcome = apply(&sandbox.job("rose-pine-dawn")).unwrap();
        assert!(
            outcome.note.starts_with("no Niri config found"),
            "{outcome:?}"
        );
        assert_eq!(outcome.step, None);

        // A config that does not include it yet: say which line to add. A
        // commented-out include is not one.
        sandbox.write(
            ".config/niri/config.kdl",
            "// include \"~/.local/state/enso/niri.kdl\"\n",
        );
        let outcome = apply(&sandbox.job("rose-pine-dawn")).unwrap();
        let step = outcome.step.unwrap();
        assert_eq!(step.what, "to use them, add to ~/.config/niri/config.kdl:");
        assert_eq!(
            step.lines,
            ["include optional=true \"~/.local/state/enso/niri.kdl\""]
        );

        // Included from any file of the config: nothing more to say.
        sandbox.write(
            ".config/niri/theme.kdl",
            "include optional=true \"~/.local/state/enso/niri.kdl\"\n",
        );
        assert_eq!(
            apply(&sandbox.job("rose-pine-dawn")).unwrap(),
            Outcome::done("colours updated")
        );
    }
}
