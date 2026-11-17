//! Fonts: the families enso draws with, for every other program too.
//!
//! enso's own tools read `font` and `mono_font` under `[appearance]`. The
//! rest of the desktop is told in the ways it listens:
//!
//! - **fontconfig** answers every program that asks for a kind of font
//!   rather than one by name — Alacritty's `monospace`, Firefox and web
//!   pages, most toolkits. enso writes `fonts.conf` in its state directory,
//!   putting the configured families first for `sans-serif`, `system-ui`, and
//!   `monospace` (with Symbols Nerd Font behind each, for glyphs), and one
//!   line of the user's own `~/.config/fontconfig/fonts.conf` includes it:
//!
//!   ```xml
//!   <include ignore_missing="yes">~/.local/state/enso/fonts.conf</include>
//!   ```
//!
//!   Preferences of the user's own after that line come after enso's: the
//!   place for fallbacks.
//! - **GTK** reads `font-name` and `monospace-font-name` from dconf (GTK 4
//!   by way of the portal) and follows them while running; GTK 3 off Wayland
//!   reads `gtk-font-name` in its `settings.ini`.
//!
//! A family left generic — `sans-serif`, `monospace` — is the system's
//! choice, and is left to the system.

use std::fs;

use super::{Job, Outcome, files, keyfile, mentions, replace};

/// Name of the generated file inside the state directory.
const FILE: &str = "fonts.conf";

/// The dconf keys, as GSettings' `org.gnome.desktop.interface` has them.
const DCONF_UI: &str = "/org/gnome/desktop/interface/font-name";
const DCONF_MONO: &str = "/org/gnome/desktop/interface/monospace-font-name";

/// The glyph font put behind each family, for icons and symbols.
const GLYPHS: &str = "Symbols Nerd Font";

/// The configured families that name a font: generic ones are `None`.
struct Families<'a> {
    ui: Option<&'a str>,
    mono: Option<&'a str>,
}

fn named(family: &str) -> Option<&str> {
    let family = family.trim();
    (!matches!(
        family,
        "" | "sans-serif" | "sans" | "serif" | "monospace" | "mono" | "system-ui"
    ))
    .then_some(family)
}

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let appearance = &job.ctx.config.appearance;
    let families = Families {
        ui: named(&appearance.font),
        mono: named(&appearance.mono_font),
    };
    let file = job.generate(FILE, &render(job, &families))?;
    if families.ui.is_none() && families.mono.is_none() {
        return Ok(Outcome::absent("the system's own"));
    }
    let host = job.host;
    // GTK sizes fonts in points, enso in pixels: 3/4 of a point each.
    let points = (appearance.font_size * 0.75).round().max(6.0);
    let with_size = |family: &str| format!("{family} {points}");

    if host.session {
        let ui = families.ui.map(with_size);
        let mono = families.mono.map(with_size);
        let entries: Vec<(&str, &str)> = [(DCONF_UI, ui.as_deref()), (DCONF_MONO, mono.as_deref())]
            .into_iter()
            .filter_map(|(key, value)| Some((key, value?)))
            .collect();
        match enso_sys::dconf::set_strings(&entries) {
            Ok(()) => {}
            Err(err) if err.is_unavailable() => {}
            Err(err) => return Err(format!("dconf: {err}")),
        }
    }

    let gtk_3 = host.config.join("gtk-3.0");
    if gtk_3.is_dir()
        && let Some(ui) = families.ui
    {
        let ini = gtk_3.join("settings.ini");
        let text = fs::read_to_string(&ini).unwrap_or_default();
        replace(
            &ini,
            &keyfile::set(&text, "Settings", "gtk-font-name", &with_size(ui)),
        )?;
    }

    let note = [families.ui, families.mono]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let outcome = Outcome::done(note);
    let dir = host.config.join("fontconfig");
    let included = mentions(&files(&dir, &["conf"], 1), "<!--", &format!("enso/{FILE}"));
    Ok(if included {
        outcome
    } else {
        outcome.step(
            format!(
                "for Alacritty, Firefox, and the rest, add inside <fontconfig> in {}:",
                host.tilde(&dir.join("fonts.conf"))
            ),
            [format!(
                "<include ignore_missing=\"yes\">{}</include>",
                host.tilde(&file)
            )],
        )
    })
}

/// The configured families as fontconfig preferences.
fn render(job: &Job<'_>, families: &Families<'_>) -> String {
    let alias = |generic: &str, family: Option<&str>| {
        family.map_or_else(String::new, |family| {
            format!(
                "  <alias binding=\"same\">\n    <family>{generic}</family>\n    <prefer>\n      \
                 <family>{}</family>\n      <family>{GLYPHS}</family>\n    </prefer>\n  </alias>\n",
                escape(family)
            )
        })
    };
    format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <!-- {} -->\n<fontconfig>\n{}{}{}</fontconfig>\n",
        escape(&job.banner()).replace("--", "—"),
        alias("sans-serif", families.ui),
        alias("system-ui", families.ui),
        alias("monospace", families.mono),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    fn google(sandbox: &mut Sandbox) {
        sandbox.ctx.config.appearance.font = "Google Sans Flex".into();
        sandbox.ctx.config.appearance.mono_font = "Google Sans Code".into();
    }

    #[test]
    fn generic_families_are_left_to_the_system() {
        let sandbox = Sandbox::new("fonts-generic");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.absent, "{outcome:?}");
        // The file is there, saying nothing, for whoever includes it.
        let conf = sandbox.read(".local/state/enso/fonts.conf");
        assert!(!conf.contains("<alias"), "{conf}");
    }

    #[test]
    fn named_families_go_to_fontconfig_and_gtk() {
        let mut sandbox = Sandbox::new("fonts-named");
        google(&mut sandbox);
        sandbox.write(".config/gtk-3.0/settings.ini", "[Settings]\n");

        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(outcome.note, "Google Sans Flex, Google Sans Code");
        let step = outcome.step.unwrap();
        assert_eq!(
            step.what,
            "for Alacritty, Firefox, and the rest, add inside <fontconfig> in \
             ~/.config/fontconfig/fonts.conf:"
        );
        assert_eq!(
            step.lines,
            ["<include ignore_missing=\"yes\">~/.local/state/enso/fonts.conf</include>"]
        );

        let conf = sandbox.read(".local/state/enso/fonts.conf");
        for generic in ["sans-serif", "system-ui"] {
            assert!(
                conf.contains(&format!(
                    "<family>{generic}</family>\n    <prefer>\n      <family>Google Sans \
                     Flex</family>\n      <family>Symbols Nerd Font</family>"
                )),
                "{conf}"
            );
        }
        assert!(conf.contains(
            "<family>monospace</family>\n    <prefer>\n      <family>Google Sans Code</family>"
        ));
        // A comment may not hold "--".
        let comment = conf.lines().find(|line| line.starts_with("<!--")).unwrap();
        assert!(!comment[4..comment.len() - 3].contains("--"), "{comment}");

        // 14 px is 10.5 pt, which GTK is given as 11.
        assert_eq!(
            sandbox.read(".config/gtk-3.0/settings.ini"),
            "[Settings]\ngtk-font-name=Google Sans Flex 11\n"
        );

        // Included (from a conf.d file, say): nothing more to ask.
        sandbox.write(
            ".config/fontconfig/conf.d/10-enso.conf",
            "<include ignore_missing=\"yes\">~/.local/state/enso/fonts.conf</include>\n",
        );
        assert_eq!(apply(&sandbox.job("rose-pine-moon")).unwrap().step, None);
    }

    #[test]
    fn names_are_escaped_for_xml() {
        let mut sandbox = Sandbox::new("fonts-escape");
        sandbox.ctx.config.appearance.font = "Fish & <Chips>".into();
        apply(&sandbox.job("rose-pine-moon")).unwrap();
        let conf = sandbox.read(".local/state/enso/fonts.conf");
        assert!(
            conf.contains("<family>Fish &amp; &lt;Chips&gt;</family>"),
            "{conf}"
        );
        // Only the monospace family left generic: no alias for it.
        assert!(!conf.contains("<family>monospace</family>"));
    }
}
