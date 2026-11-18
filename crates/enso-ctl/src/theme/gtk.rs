//! GTK, and with it the system's dark-or-light preference.
//!
//! Three things, for three kinds of program:
//!
//! - **The colour scheme** — `color-scheme` in dconf, `prefer-dark` or
//!   `prefer-light`. This is the setting the desktop portal hands on, so it
//!   is how GTK 4, libadwaita, Qt 6, Firefox, and Electron all learn whether
//!   the desktop is dark, and they follow it while running.
//! - **GTK 3**, which does not ask the portal: `gtk-application-prefer-dark-theme`
//!   in its `settings.ini`, the one key of that file enso sets. GTK 3
//!   programs read it when they start.
//! - **The palette** — a stylesheet per GTK version in enso's state
//!   directory, naming the colours libadwaita builds itself from. A program
//!   takes them once the user's own `gtk.css` imports the file:
//!
//!   ```css
//!   @import url("file:///home/me/.local/state/enso/gtk-4.0.css");
//!   ```
//!
//!   GTK 3's stock theme has its colours built in, so its stylesheet also
//!   recolours the stock widgets one by one — enough for a file chooser or
//!   a settings dialog to belong to the desktop.
//!
//! GTK 3 reads both only as it starts, and one GTK 3 program lives as long
//! as the session: the GTK portal, which draws the file chooser Firefox and
//! every other portal user opens. So it is restarted, if it is running; a
//! chooser open at that moment closes, as if cancelled.

use std::{fs, path::Path};

use enso_core::{Palette, Rgba};

use super::{Job, Outcome, files, keyfile, mentions, replace};

/// The generated stylesheets, inside the state directory.
pub const CSS_3: &str = "gtk-3.0.css";
pub const CSS_4: &str = "gtk-4.0.css";

/// Rules that recolour GTK 3's stock widgets from the names above.
const WIDGETS_3: &str = include_str!("gtk3.css");

/// The preference in dconf, as GSettings' `org.gnome.desktop.interface` has it.
const COLOR_SCHEME: &str = "/org/gnome/desktop/interface/color-scheme";
/// GTK 3's own switch, in `[Settings]` of its `settings.ini`.
const PREFER_DARK: &str = "gtk-application-prefer-dark-theme";
/// The GTK 3 program that outlives a theme change: the file chooser's.
const PORTAL: &str = "xdg-desktop-portal-gtk.service";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    job.generate(CSS_3, &render_3(job))?;
    let css_4 = job.generate(CSS_4, &render_4(job))?;
    let host = job.host;
    let dark = job.theme.is_dark();
    let scheme = if dark { "dark" } else { "light" };

    // The preference itself, where there is a session to hold it.
    let told = host.session
        && match enso_sys::dconf::set_strings(&[(COLOR_SCHEME, &format!("prefer-{scheme}"))]) {
            Ok(()) => true,
            // No dconf on this machine: nothing keeps such a setting.
            Err(err) if err.is_unavailable() => false,
            Err(err) => return Err(format!("dconf: {err}")),
        };

    // GTK 3, if it has ever run here.
    let gtk_3 = host.config.join("gtk-3.0");
    if gtk_3.is_dir() {
        prefer_dark(&gtk_3.join("settings.ini"), dark)?;
        // Best effort: without systemd, or without the portal, there is no
        // long-lived GTK 3 program to bring up to date.
        if host.session {
            let _ = enso_sys::systemd::try_restart(PORTAL);
        }
    }

    let gtk_4 = host.config.join("gtk-4.0");
    if !told && !gtk_3.is_dir() && !gtk_4.is_dir() {
        return Ok(Outcome::absent("no dconf and no GTK settings"));
    }
    let outcome = Outcome::done(format!("{scheme} colour scheme set"));
    // The palette is only worth a line to those with GTK 4 programs; GTK 3's
    // stylesheet is documented for those whose theme can use it.
    let imported = mentions(&files(&gtk_4, &["css"], 0), "/*", &format!("enso/{CSS_4}"));
    Ok(if gtk_4.is_dir() && !imported {
        outcome.step(
            format!(
                "for the palette too, add to {}:",
                host.tilde(&gtk_4.join("gtk.css"))
            ),
            [format!("@import url(\"{}\");", file_url(&css_4))],
        )
    } else {
        outcome
    })
}

/// Set GTK 3's dark switch in its `settings.ini`, touching nothing else.
fn prefer_dark(file: &Path, dark: bool) -> Result<(), String> {
    let current = fs::read_to_string(file).unwrap_or_default();
    let value = if dark { "true" } else { "false" };
    replace(
        file,
        &keyfile::set(&current, "Settings", PREFER_DARK, value),
    )
}

/// A path as a `file://` URL, which is what a stylesheet imports.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                url.push(char::from(byte));
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

/// libadwaita's interface colours, each filled from the palette. The names
/// are libadwaita's, written with underscores; the stylesheets spell them
/// both ways.
fn interface(job: &Job<'_>) -> Vec<(&'static str, Rgba)> {
    let Palette {
        base,
        surface,
        overlay,
        text,
        love,
        gold,
        pine,
        ..
    } = job.colors();
    let accent = job.accent();
    vec![
        ("accent_bg_color", accent),
        ("accent_fg_color", base),
        ("accent_color", accent),
        ("destructive_bg_color", love),
        ("destructive_fg_color", base),
        ("destructive_color", love),
        ("success_bg_color", pine),
        ("success_fg_color", base),
        ("success_color", pine),
        ("warning_bg_color", gold),
        ("warning_fg_color", base),
        ("warning_color", gold),
        ("error_bg_color", love),
        ("error_fg_color", base),
        ("error_color", love),
        ("window_bg_color", base),
        ("window_fg_color", text),
        ("view_bg_color", surface),
        ("view_fg_color", text),
        ("headerbar_bg_color", base),
        ("headerbar_fg_color", text),
        ("headerbar_border_color", text),
        ("headerbar_backdrop_color", base),
        ("sidebar_bg_color", surface),
        ("sidebar_fg_color", text),
        ("sidebar_backdrop_color", surface),
        ("sidebar_border_color", text),
        ("secondary_sidebar_bg_color", surface),
        ("secondary_sidebar_fg_color", text),
        ("secondary_sidebar_backdrop_color", surface),
        ("secondary_sidebar_border_color", text),
        ("card_bg_color", surface),
        ("card_fg_color", text),
        ("dialog_bg_color", surface),
        ("dialog_fg_color", text),
        ("popover_bg_color", overlay),
        ("popover_fg_color", text),
        ("overview_bg_color", surface),
        ("overview_fg_color", text),
        ("thumbnail_bg_color", overlay),
        ("thumbnail_fg_color", text),
    ]
}

fn define(colors: &[(&str, Rgba)]) -> String {
    colors
        .iter()
        .map(|(name, color)| format!("@define-color {name} {color};\n"))
        .collect()
}

/// The stylesheet for GTK 4: libadwaita's colours as the CSS variables it
/// has read since 1.6, and as the named colours it read before.
pub fn render_4(job: &Job<'_>) -> String {
    let colors = interface(job);
    let variables: String = colors
        .iter()
        .map(|(name, color)| format!("  --{}: {color};\n", name.replace('_', "-")))
        .collect();
    format!(
        "/* {banner} */\n\n:root {{\n{variables}}}\n\n/* libadwaita before 1.6. */\n{named}",
        banner = job.banner(),
        named = define(&colors),
    )
}

/// The stylesheet for GTK 3: the colours GTK 3 themes name, and libadwaita's
/// for the themes that follow it.
pub fn render_3(job: &Job<'_>) -> String {
    let Palette {
        base,
        surface,
        muted,
        text,
        highlight_med,
        ..
    } = job.colors();
    let accent = job.accent();
    // `warning_color`, `error_color`, and `success_color` are names in both
    // vocabularies, and come with libadwaita's below.
    let legacy = [
        ("theme_bg_color", base),
        ("theme_fg_color", text),
        ("theme_base_color", surface),
        ("theme_text_color", text),
        ("theme_selected_bg_color", accent),
        ("theme_selected_fg_color", base),
        ("theme_unfocused_bg_color", base),
        ("theme_unfocused_fg_color", text),
        ("theme_unfocused_base_color", surface),
        ("theme_unfocused_text_color", text),
        ("theme_unfocused_selected_bg_color", accent),
        ("theme_unfocused_selected_fg_color", base),
        ("insensitive_bg_color", base),
        ("insensitive_fg_color", muted),
        ("insensitive_base_color", surface),
        ("borders", highlight_med),
        ("unfocused_borders", highlight_med),
    ];
    // Every role under its own name, for the widget rules and for a
    // `gtk.css` of the user's own.
    let mut roles: Vec<(String, Rgba)> = job
        .colors()
        .iter()
        .map(|(role, color)| (format!("enso_{}", role.name().replace('-', "_")), color))
        .collect();
    roles.push(("enso_accent".to_owned(), accent));
    let roles: String = roles
        .iter()
        .map(|(name, color)| format!("@define-color {name} {color};\n"))
        .collect();
    format!(
        "/* {banner} */\n\n{roles}\n{legacy}\n/* libadwaita's names, for themes that follow \
         it. */\n{named}{WIDGETS_3}",
        banner = job.banner(),
        legacy = define(&legacy),
        named = define(&interface(job)),
    )
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    #[test]
    fn stylesheets_name_the_palette() {
        let sandbox = Sandbox::new("gtk-render");
        let moon = render_4(&sandbox.job("rose-pine-moon"));
        assert!(moon.starts_with("/* Written by `enso-ctl theme set` — Rosé Pine Moon"));
        assert!(moon.contains("  --window-bg-color: #232136;\n"), "{moon}");
        assert!(moon.contains("  --window-fg-color: #e0def4;\n"));
        // The accent is the configured one: rose, unless config says otherwise.
        assert!(moon.contains("  --accent-bg-color: #ea9a97;\n"));
        assert!(moon.contains("  --secondary-sidebar-bg-color: #2a273f;\n"));
        assert!(moon.contains("@define-color window_bg_color #232136;\n"));
        assert_eq!(moon.matches('{').count(), moon.matches('}').count());
        // Every colour is said both ways.
        assert_eq!(
            moon.matches("  --").count(),
            moon.matches("@define-color ").count()
        );

        let dawn = render_3(&sandbox.job("rose-pine-dawn"));
        assert!(
            dawn.contains("@define-color theme_bg_color #faf4ed;\n"),
            "{dawn}"
        );
        assert!(dawn.contains("@define-color theme_selected_bg_color #d7827e;\n"));
        assert!(dawn.contains("@define-color window_bg_color #faf4ed;\n"));
        // GTK 3 knows no CSS variables.
        assert!(!dawn.contains("--"), "{dawn}");
        // The roles by name, and widget rules that use only names defined.
        assert!(dawn.contains("@define-color enso_highlight_med #dfdad9;\n"));
        assert!(dawn.contains("@define-color enso_accent #d7827e;\n"));
        for used in dawn
            .split('@')
            .skip(1)
            .filter(|rest| rest.starts_with("enso_"))
        {
            let name: String = used
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            assert!(dawn.contains(&format!("@define-color {name} ")), "{name}");
        }
        // No name is defined twice.
        let mut names: Vec<&str> = dawn
            .lines()
            .filter_map(|line| line.strip_prefix("@define-color ")?.split(' ').next())
            .collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn a_machine_without_gtk_is_passed_over() {
        let sandbox = Sandbox::new("gtk-absent");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.absent, "{outcome:?}");
        // The stylesheets are there for the day it is not.
        assert!(
            sandbox
                .read(".local/state/enso/gtk-3.0.css")
                .contains("#232136")
        );
        assert!(
            sandbox
                .read(".local/state/enso/gtk-4.0.css")
                .contains("#232136")
        );
        assert!(!sandbox.root.join(".config/gtk-3.0").exists());
    }

    #[test]
    fn gtk_3_is_told_in_its_settings_file() {
        let sandbox = Sandbox::new("gtk-3");
        let ini = ".config/gtk-3.0/settings.ini";
        std::fs::create_dir_all(sandbox.root.join(".config/gtk-3.0")).unwrap();

        // No file yet: one is made, holding only the switch.
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(outcome, Outcome::done("dark colour scheme set"));
        assert_eq!(
            sandbox.read(ini),
            "[Settings]\ngtk-application-prefer-dark-theme=true\n"
        );

        // A file with settings of the user's own keeps them.
        sandbox.write(
            ini,
            "[Settings]\ngtk-font-name=Inter 11\ngtk-application-prefer-dark-theme=true\n\
             gtk-cursor-theme-size=24\n",
        );
        let outcome = apply(&sandbox.job("rose-pine-dawn")).unwrap();
        assert_eq!(outcome, Outcome::done("light colour scheme set"));
        assert_eq!(
            sandbox.read(ini),
            "[Settings]\ngtk-font-name=Inter 11\ngtk-application-prefer-dark-theme=false\n\
             gtk-cursor-theme-size=24\n"
        );
    }

    #[test]
    fn gtk_4_is_offered_the_palette_until_it_imports_it() {
        let sandbox = Sandbox::new("gtk-4");
        std::fs::create_dir_all(sandbox.root.join(".config/gtk-4.0")).unwrap();
        let css = sandbox.root.join(".local/state/enso/gtk-4.0.css");
        let import = format!("@import url(\"file://{}\");", css.display());

        let step = apply(&sandbox.job("rose-pine-moon")).unwrap().step.unwrap();
        assert_eq!(
            step.what,
            "for the palette too, add to ~/.config/gtk-4.0/gtk.css:"
        );
        assert_eq!(step.lines, std::slice::from_ref(&import));
        // GTK 4 follows the portal: its settings file is not written.
        assert!(!sandbox.root.join(".config/gtk-4.0/settings.ini").exists());

        sandbox.write(".config/gtk-4.0/gtk.css", &format!("{import}\n"));
        assert_eq!(
            apply(&sandbox.job("rose-pine-moon")).unwrap(),
            Outcome::done("dark colour scheme set")
        );
    }

    #[test]
    fn paths_become_urls() {
        assert_eq!(
            file_url(Path::new("/home/me/.local/state/enso/gtk-4.0.css")),
            "file:///home/me/.local/state/enso/gtk-4.0.css"
        );
        assert_eq!(
            file_url(Path::new("/home/a b/é.css")),
            "file:///home/a%20b/%C3%A9.css"
        );
    }
}
