//! Flatpak as a theme target: letting sandboxed programs see the theme.
//!
//! A Flatpak program learns dark-or-light from the settings portal like any
//! other, so that much needs nothing. What it cannot do is read the files
//! that carry the palette — its sandbox has a config directory of its own.
//! Flatpak's answer is an override: a key file per program, and one named
//! `global` for all of them, listing what the sandbox may see.
//!
//! enso adds four read-only entries to the user's global override — GTK's
//! two config directories, and the two stylesheets [`gtk`](super::gtk)
//! writes, which the `gtk.css` in those directories imports. Nothing else of
//! the file is touched; an entry the user already has, or has denied with a
//! `!`, is left exactly as it is.

use std::fs;

use super::{Job, Outcome, gtk, keyfile, replace};

/// The key of the `[Context]` group that lists what a sandbox may see.
const KEY: &str = "filesystems";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let host = job.host;
    if !host.has_program("flatpak") {
        return Ok(Outcome::absent("not installed"));
    }
    let wanted: Vec<String> = [
        "xdg-config/gtk-3.0".to_owned(),
        "xdg-config/gtk-4.0".to_owned(),
        host.tilde(&job.generated(gtk::CSS_3)),
        host.tilde(&job.generated(gtk::CSS_4)),
    ]
    .into_iter()
    .map(|location| format!("{location}:ro"))
    .collect();

    let file = host.data.join("flatpak/overrides/global");
    let current = fs::read_to_string(&file).unwrap_or_default();
    replace(&file, &grant(&current, &wanted))?;
    Ok(Outcome::done("sandboxed programs can read the GTK theme"))
}

/// An override file that grants each of `wanted`, unless it already has a
/// say on that location.
fn grant(text: &str, wanted: &[String]) -> String {
    let current = keyfile::get(text, "Context", KEY).unwrap_or_default();
    let mut entries: Vec<&str> = current
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect();
    let before = entries.len();
    for entry in wanted {
        if !entries.iter().any(|have| location(have) == location(entry)) {
            entries.push(entry);
        }
    }
    if entries.len() == before {
        return text.to_owned();
    }
    // A list in a key file ends each item, the last included, with `;`.
    keyfile::set(text, "Context", KEY, &format!("{};", entries.join(";")))
}

/// What an entry is about: the path, less a `!` that denies it and the mode
/// it is granted in.
fn location(entry: &str) -> &str {
    let entry = entry.strip_prefix('!').unwrap_or(entry);
    [":ro", ":rw", ":create"]
        .iter()
        .find_map(|mode| entry.strip_suffix(mode))
        .unwrap_or(entry)
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    const GLOBAL: &str = ".local/share/flatpak/overrides/global";
    const GRANTS: &str = "xdg-config/gtk-3.0:ro;xdg-config/gtk-4.0:ro;\
                          ~/.local/state/enso/gtk-3.0.css:ro;~/.local/state/enso/gtk-4.0.css:ro;";

    #[test]
    fn nothing_is_written_without_flatpak() {
        let sandbox = Sandbox::new("flatpak-absent");
        assert!(apply(&sandbox.job("rose-pine-moon")).unwrap().absent);
        assert!(!sandbox.root.join(".local/share/flatpak").exists());
    }

    #[test]
    fn grants_the_theme_once() {
        let mut sandbox = Sandbox::new("flatpak-grant");
        sandbox.install("flatpak");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(
            outcome,
            Outcome::done("sandboxed programs can read the GTK theme")
        );
        let written = sandbox.read(GLOBAL);
        assert_eq!(written, format!("[Context]\nfilesystems={GRANTS}\n"));

        // A second switch finds its entries there and leaves the file be.
        apply(&sandbox.job("rose-pine-dawn")).unwrap();
        assert_eq!(sandbox.read(GLOBAL), written);
    }

    #[test]
    fn keeps_what_the_user_set() {
        let mut sandbox = Sandbox::new("flatpak-merge");
        sandbox.install("flatpak");
        sandbox.write(
            GLOBAL,
            "[Context]\nsockets=wayland;\nfilesystems=~/Games;!xdg-config/gtk-3.0;\
             xdg-config/gtk-4.0;\n\n[Environment]\nGTK_THEME=Adwaita\n",
        );
        apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(
            sandbox.read(GLOBAL),
            // gtk-3.0 stays denied, gtk-4.0 stays as it was granted; only
            // the two stylesheets are new.
            "[Context]\nsockets=wayland;\nfilesystems=~/Games;!xdg-config/gtk-3.0;\
             xdg-config/gtk-4.0;~/.local/state/enso/gtk-3.0.css:ro;\
             ~/.local/state/enso/gtk-4.0.css:ro;\n\n[Environment]\nGTK_THEME=Adwaita\n"
        );
    }

    #[test]
    fn adds_the_key_to_a_file_without_it() {
        let wanted = ["a:ro".to_owned()];
        assert_eq!(
            grant("[Environment]\nA=1\n", &wanted),
            "[Environment]\nA=1\n\n[Context]\nfilesystems=a:ro;\n"
        );
        assert_eq!(
            grant("[Context]\nsockets=x11;\n", &wanted),
            "[Context]\nsockets=x11;\nfilesystems=a:ro;\n"
        );
        assert_eq!(location("!xdg-config/gtk-3.0"), "xdg-config/gtk-3.0");
        assert_eq!(location("~/Games:create"), "~/Games");
        assert_eq!(location("home"), "home");
    }
}
