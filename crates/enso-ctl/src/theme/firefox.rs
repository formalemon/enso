//! Firefox as a theme target: the palette for the browser's own interface.
//!
//! Firefox follows dark and light through the settings portal (see
//! [`gtk`](super::gtk)). Its tabs, toolbars, address bar, and menus are
//! coloured by CSS variables, and a profile may restyle them from
//! `chrome/userChrome.css` — once `about:config` has
//! `toolkit.legacyUserProfileCustomizations.stylesheets` set to `true`.
//! enso writes a stylesheet that sets those variables from the palette; the
//! profile's `userChrome.css` imports it:
//!
//! ```css
//! @import url("file:///home/me/.local/state/enso/firefox.css");
//! ```
//!
//! Firefox reads it when it starts, so a switch shows on the next start.
//! Pages are not touched: they are the web's to colour.

use std::path::{Path, PathBuf};

use enso_core::Palette;

use super::{Job, Outcome, files, gtk::file_url, mentions};

/// Name of the generated file inside the state directory.
const FILE: &str = "firefox.css";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let file = job.generate(FILE, &render(job))?;
    let host = job.host;
    // Where Firefox keeps profiles: the XDG place, and the one before it.
    let roots = [
        host.config.join("mozilla/firefox"),
        host.home.join(".mozilla/firefox"),
    ];
    let Some(root) = roots.iter().find(|root| root.is_dir()) else {
        return Ok(if host.has_program("firefox") {
            Outcome::done("colours written; Firefox has no profile yet")
        } else {
            Outcome::absent("not installed")
        });
    };
    // Every profile's own stylesheets.
    let sheets: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| files(root, &["css"], 2))
        .filter(|path| path.parent().is_some_and(|dir| dir.ends_with("chrome")))
        .collect();
    let imported = sheets.iter().any(|sheet| imports(sheet, &file));
    Ok(if imported || mentions(&sheets, "/*", &format!("enso/{FILE}")) {
        Outcome::done("colours updated; shown when Firefox next starts")
    } else {
        Outcome::done("colours written").step(
            format!(
                "to use them, turn on toolkit.legacyUserProfileCustomizations.stylesheets \
                 in about:config, and add to chrome/userChrome.css of a profile in {}:",
                host.tilde(root)
            ),
            [format!("@import url(\"{}\");", file_url(&file))],
        )
    })
}

/// Whether `sheet` imports `target` by any name: a relative `enso.css`
/// linked to it beside the sheet counts as much as its full path.
fn imports(sheet: &Path, target: &Path) -> bool {
    let (Ok(text), Some(dir), Ok(target)) = (
        std::fs::read_to_string(sheet),
        sheet.parent(),
        target.canonicalize(),
    ) else {
        return false;
    };
    text.lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("@import"))
        .filter_map(|line| {
            let start = line.find(['"', '\''])?;
            let quote = line[start..].chars().next()?;
            let rest = &line[start + 1..];
            Some(rest[..rest.find(quote)?].to_owned())
        })
        .any(|url| {
            let path = url.strip_prefix("file://").unwrap_or(&url);
            dir.join(path)
                .canonicalize()
                .is_ok_and(|path| path == target)
        })
}

/// A `userChrome.css` that colours the browser's interface from the palette.
pub fn render(job: &Job<'_>) -> String {
    let Palette {
        base,
        surface,
        overlay,
        muted,
        subtle,
        text,
        love,
        highlight_med,
        highlight_high,
        ..
    } = job.colors();
    let accent = job.accent();
    format!(
        r##"/* {banner} */

:root {{
  /* The window: the tab strip and what frames the toolbars. */
  --lwt-accent-color: {base} !important;
  --lwt-accent-color-inactive: {base} !important;
  --lwt-text-color: {text} !important;
  --toolbox-bgcolor: {base} !important;
  --toolbox-bgcolor-inactive: {base} !important;
  --toolbox-textcolor: {text} !important;
  --toolbox-textcolor-inactive: {subtle} !important;
  --tabpanel-background-color: {base} !important;

  /* Toolbars and the selected tab, one surface. */
  --toolbar-bgcolor: {surface} !important;
  --toolbar-color: {text} !important;
  --toolbarbutton-icon-fill: {subtle} !important;
  --toolbarbutton-icon-fill-attention: {accent} !important;
  --toolbarbutton-hover-background: {overlay} !important;
  --toolbarbutton-active-background: {highlight_med} !important;
  --tab-selected-bgcolor: {surface} !important;
  --tab-selected-textcolor: {text} !important;
  --tab-hover-background-color: {overlay} !important;
  --tab-loading-fill: {accent} !important;
  --chrome-content-separator-color: {highlight_med} !important;

  /* The address bar and other fields. */
  --toolbar-field-background-color: {base} !important;
  --toolbar-field-color: {text} !important;
  --toolbar-field-border-color: {highlight_med} !important;
  --toolbar-field-focus-background-color: {base} !important;
  --toolbar-field-focus-color: {text} !important;
  --toolbar-field-focus-border-color: {accent} !important;
  --urlbar-box-bgcolor: {overlay} !important;
  --urlbar-box-focus-bgcolor: {overlay} !important;
  --urlbar-box-hover-bgcolor: {highlight_med} !important;
  --urlbar-box-text-color: {text} !important;
  --urlbar-popup-url-color: {accent} !important;
  --urlbarView-highlight-background: {accent} !important;
  --urlbarView-highlight-color: {base} !important;
  --urlbarView-hover-background: {overlay} !important;
  --urlbarView-separator-color: {highlight_med} !important;

  /* Menus and panels. */
  --arrowpanel-background: {overlay} !important;
  --arrowpanel-color: {text} !important;
  --arrowpanel-border-color: {highlight_med} !important;
  --arrowpanel-dimmed: {highlight_med} !important;
  --arrowpanel-dimmed-further: {highlight_high} !important;
  --panel-separator-color: {highlight_med} !important;
  --panel-disabled-color: {muted} !important;
  --menu-background-color: {overlay} !important;
  --menu-color: {text} !important;
  --menu-border-color: {highlight_med} !important;
  --menuitem-hover-background-color: {highlight_med} !important;
  --menuitem-disabled-color: {muted} !important;

  /* The sidebar. */
  --sidebar-background-color: {base} !important;
  --sidebar-text-color: {text} !important;
  --sidebar-border-color: {highlight_med} !important;

  /* What is focused, chosen, or wrong. */
  --focus-outline-color: {accent} !important;
  --color-accent-primary: {accent} !important;
  --color-accent-primary-hover: {accent} !important;
  --color-accent-primary-active: {accent} !important;
  --button-primary-bgcolor: {accent} !important;
  --button-primary-color: {base} !important;
  --attention-dot-color: {accent} !important;
  --link-color: {accent} !important;
  --text-color-error: {love} !important;
}}

#navigator-toolbox {{
  background-color: {base} !important;
  color: {text} !important;
  border-bottom-color: {highlight_med} !important;
}}

#nav-bar,
#PersonalToolbar,
findbar {{
  background-color: {surface} !important;
  color: {text} !important;
}}

#sidebar-box,
#sidebar-main {{
  background-color: {base} !important;
  color: {text} !important;
}}

menupopup,
panel {{
  --panel-background: {overlay} !important;
  --panel-color: {text} !important;
  --panel-border-color: {highlight_med} !important;
}}

::selection {{
  background-color: {accent};
  color: {base};
}}
"##,
        banner = job.banner(),
    )
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    #[test]
    fn colours_the_interface_from_the_palette() {
        let sandbox = Sandbox::new("firefox-render");
        let moon = render(&sandbox.job("rose-pine-moon"));
        assert!(moon.starts_with("/* Written by `enso-ctl theme set` — Rosé Pine Moon"));
        assert!(
            moon.contains("  --lwt-accent-color: #232136 !important;\n"),
            "{moon}"
        );
        assert!(moon.contains("  --toolbar-bgcolor: #2a273f !important;\n"));
        assert!(moon.contains("  --toolbar-field-focus-border-color: #ea9a97 !important;\n"));
        let dawn = render(&sandbox.job("rose-pine-dawn"));
        assert!(dawn.contains("  --toolbar-color: #464261 !important;\n"));
        for text in [&moon, &dawn] {
            assert_eq!(text.matches('{').count(), text.matches('}').count());
            // Every declaration got its colour.
            assert!(!text.contains(": ;") && !text.contains(":  !"), "{text}");
        }
    }

    #[test]
    fn says_what_to_add_until_a_profile_imports_it() {
        let mut sandbox = Sandbox::new("firefox-apply");
        assert!(apply(&sandbox.job("rose-pine-moon")).unwrap().absent);
        assert!(
            sandbox
                .read(".local/state/enso/firefox.css")
                .contains("#232136")
        );

        // Installed, never started.
        sandbox.install("firefox");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.note.contains("no profile yet"), "{outcome:?}");
        assert_eq!(outcome.step, None);

        // A profile that does not import it.
        sandbox.write(".config/mozilla/firefox/abc.default/prefs.js", "");
        let css = sandbox.root.join(".local/state/enso/firefox.css");
        let import = format!("@import url(\"file://{}\");", css.display());
        let step = apply(&sandbox.job("rose-pine-moon")).unwrap().step.unwrap();
        assert!(step.what.contains("about:config"), "{step:?}");
        assert!(step.what.contains("~/.config/mozilla/firefox"), "{step:?}");
        assert_eq!(step.lines, std::slice::from_ref(&import));

        // Imported by a profile's userChrome.css — and only that counts.
        sandbox.write(".config/mozilla/firefox/abc.default/notes.css", &import);
        assert!(
            apply(&sandbox.job("rose-pine-moon"))
                .unwrap()
                .step
                .is_some()
        );
        sandbox.write(
            ".config/mozilla/firefox/abc.default/chrome/userChrome.css",
            &format!("{import}\n"),
        );
        let outcome = apply(&sandbox.job("rose-pine-dawn")).unwrap();
        assert_eq!(outcome.step, None);
        assert!(outcome.note.starts_with("colours updated"), "{outcome:?}");
    }
}
