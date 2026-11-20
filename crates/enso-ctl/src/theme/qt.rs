//! Qt as a theme target.
//!
//! Qt's colours are the business of a platform theme, and the one Qt picks
//! on a desktop it does not know (Niri) is its plain one: Fusion's light
//! palette, whatever the system prefers, and Qt's own file dialog. So enso
//! names a better one in `QT_QPA_PLATFORMTHEME`, which goes in the
//! `niri.kdl` it writes: Niri hands its `environment` to every program it
//! starts (see [`platform_theme`]). In order of preference:
//!
//! - **qt6ct**, when it is installed. enso writes a colour scheme for it and
//!   sets the keys of `qt6ct.conf` that choose it: the scheme, the fonts,
//!   and the desktop portal for file dialogs — so a Qt program opens the
//!   same file chooser as everything else. Every other line of the file is
//!   left as it was. qt6ct watches its config directory, so running Qt
//!   programs recolour within a few seconds of a switch.
//! - **gtk3**, which comes with Qt: it asks GTK 3 for the palette and the
//!   font, and GTK 3 already has enso's (see [`gtk`](super::gtk)). Its file
//!   dialog is GTK's own, though, and a running program keeps its colours
//!   until restarted.
//!
//! A session that names a platform theme of its own keeps it. For qt5ct —
//! and qt6ct, when Qt is not pointed at it — enso writes the colour scheme,
//! `enso.conf`, and says to choose it, once, in the tool's Appearance tab.
//!
//! Both tools watch their config directory and re-read everything a few
//! seconds after something in it changes — but only that directory, not
//! `colors` below it. So the new scheme is written in the config directory
//! and moved down into place: the write is atomic, and the move is the
//! nudge that has running Qt programs recolour.

use std::{fs, path::Path};

use enso_core::{Palette, Rgba};

use super::{Job, Outcome, Place, fonts, keyfile, replace};

/// Name of the colour scheme, as the tools list it.
const SCHEME: &str = "enso.conf";

/// qt6ct's platform theme.
pub const QT6CT: &str = "qt6ct";
/// Qt's own platform theme that takes its palette from GTK 3.
pub const GTK: &str = "gtk3";
/// The platform themes enso may name, best first, with their plugins.
const PLATFORM_THEMES: [(&str, &str); 2] = [(QT6CT, "libqt6ct.so"), (GTK, "libqgtk3.so")];
/// The file dialogs qt6ct is told to use: the desktop portal's.
const DIALOGS: &str = "xdgdesktopportal";

/// The platform theme for Niri to give the programs it starts, if any: the
/// best one Qt has, when Qt and Niri are both themed and the session has
/// not named another of its own.
pub fn platform_theme(job: &Job<'_>) -> Option<&'static str> {
    let host = job.host;
    let themed = [Place::Qt, Place::Niri]
        .iter()
        .all(|place| job.places.contains(place));
    let own = host
        .qt_platform_theme
        .as_deref()
        .is_some_and(|name| !PLATFORM_THEMES.iter().any(|(ours, _)| *ours == name));
    if !themed || own {
        return None;
    }
    PLATFORM_THEMES
        .iter()
        .find(|(_, plugin)| {
            host.qt_plugins
                .iter()
                .any(|dir| dir.join("platformthemes").join(plugin).is_file())
        })
        .map(|(name, _)| *name)
}

/// One of the two tools.
struct Tool {
    name: &'static str,
    /// How many colour roles its Qt has. Qt 5 wants exactly its 21; Qt 6
    /// added `Accent`, and qt6ct takes that many or more.
    roles: usize,
}

const TOOLS: [Tool; 2] = [
    Tool {
        name: "qt5ct",
        roles: 21,
    },
    Tool {
        name: QT6CT,
        roles: 22,
    },
];

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let host = job.host;
    let platform = platform_theme(job);
    let mut found = 0;
    let mut to_choose = Vec::new();
    for tool in &TOOLS {
        let dir = host.config.join(tool.name);
        let ours = platform == Some(tool.name);
        if !ours && !dir.is_dir() && !host.has_program(tool.name) {
            continue;
        }
        found += 1;
        let colors = dir.join("colors");
        fs::create_dir_all(&colors).map_err(|err| format!("{}: {err}", colors.display()))?;
        let scheme = colors.join(SCHEME);
        replace(&scheme, &render(job, tool.roles))?;
        let conf = dir.join(format!("{}.conf", tool.name));
        let text = fs::read_to_string(&conf).unwrap_or_default();
        if ours {
            replace(&conf, &configure(job, &text, &scheme))?;
        } else if platform.is_none() && !chosen(&text) {
            to_choose.push(tool.name);
        }
    }
    Ok(match (platform, found, to_choose.as_slice()) {
        (Some(QT6CT), ..) => Outcome::done(format!(
            "palette, fonts, and portal dialogs through qt6ct \
             (QT_QPA_PLATFORMTHEME={QT6CT}, in niri.kdl)"
        )),
        (Some(name), ..) => Outcome::done(format!(
            "takes GTK's palette and font (QT_QPA_PLATFORMTHEME={name}, in niri.kdl)"
        )),
        (None, 0, _) => Outcome::absent("no qt5ct or qt6ct"),
        (None, _, []) => Outcome::done("palette updated"),
        (None, _, tools) => Outcome::done("palette written").step(
            format!(
                "to use it, choose the custom colour scheme `enso` in {} (Appearance)",
                tools.join(" and ")
            ),
            Vec::<String>::new(),
        ),
    })
}

/// `conf`, qt6ct's config, with the keys that choose enso's colour scheme
/// at `scheme`, its fonts, and the portal's file dialogs. Fonts left to the
/// system are left to qt6ct too.
fn configure(job: &Job<'_>, conf: &str, scheme: &Path) -> String {
    let appearance = &job.ctx.config.appearance;
    let mut text = conf.to_owned();
    for (key, value) in [
        ("custom_palette", "true"),
        ("color_scheme_path", &*scheme.to_string_lossy()),
        ("standard_dialogs", DIALOGS),
    ] {
        text = keyfile::set(&text, "Appearance", key, value);
    }
    let points = fonts::points(appearance.font_size);
    for (key, family) in [("general", &appearance.font), ("fixed", &appearance.mono_font)] {
        // QFont's own description, as qt6ct writes it: family, size in points,
        // no pixel size, any style, normal weight, then nothing set. Quoted,
        // or Qt would read the commas as a list.
        let plain = |family: &&str| !family.contains([',', '"', '\\']);
        if let Some(family) = fonts::named(family).filter(plain) {
            let value = format!("\"{family},{points},-1,5,400,0,0,0,0,0,0,0,0,0,0,1\"");
            text = keyfile::set(&text, "Fonts", key, &value);
        }
    }
    text
}

/// Whether a tool's config has the custom palette on and pointing at ours.
fn chosen(conf: &str) -> bool {
    let get = |key| keyfile::get(conf, "Appearance", key);
    get("custom_palette") == Some("true")
        && get("color_scheme_path").is_some_and(|path| {
            path.trim_matches('"')
                .ends_with(&format!("colors/{SCHEME}"))
        })
}

/// The palette as a colour scheme of `roles` roles.
pub fn render(job: &Job<'_>, roles: usize) -> String {
    let Palette {
        base,
        surface,
        overlay,
        muted,
        text,
        love,
        iris,
        highlight_med,
        highlight_high,
        ..
    } = job.colors();
    let accent = job.accent();
    // Qt shades a button with four colours ordered by brightness; which end
    // of the palette is the bright one depends on the theme.
    let (light, midlight, mid, dark) = if job.theme.is_dark() {
        (highlight_high, highlight_med, surface, base)
    } else {
        (surface, base, highlight_med, highlight_high)
    };
    // In the order of `QPalette::ColorRole`.
    let active = [
        text,                 // WindowText
        overlay,              // Button
        light,                // Light
        midlight,             // Midlight
        dark,                 // Dark
        mid,                  // Mid
        text,                 // Text
        love,                 // BrightText
        text,                 // ButtonText
        surface,              // Base
        base,                 // Window
        Rgba::rgb(0, 0, 0),   // Shadow
        accent,               // Highlight
        base,                 // HighlightedText
        iris,                 // Link
        iris.mix(muted, 0.5), // LinkVisited
        overlay,              // AlternateBase
        text,                 // NoRole
        overlay,              // ToolTipBase
        text,                 // ToolTipText
        muted,                // PlaceholderText
        accent,               // Accent
    ];
    // A disabled widget: its words fade, and so does what it selects.
    let mut disabled = active;
    for role in [0, 6, 8, 13] {
        disabled[role] = muted;
    }
    disabled[12] = highlight_med;

    let list = |colors: &[Rgba]| {
        colors[..roles]
            .iter()
            .map(|c| format!("#ff{:02x}{:02x}{:02x}", c.r, c.g, c.b))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "; {banner}\n[ColorScheme]\nactive_colors={active}\ndisabled_colors={disabled}\n\
         inactive_colors={active}\n",
        banner = job.banner(),
        active = list(&active),
        disabled = list(&disabled),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{super::sandbox::Sandbox, *};

    fn colors<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
        keyfile::get(text, "ColorScheme", key)
            .unwrap()
            .split(", ")
            .collect()
    }

    #[test]
    fn renders_one_colour_per_role_and_state() {
        let sandbox = Sandbox::new("qt-render");
        let job = sandbox.job("rose-pine-moon");
        for roles in [21, 22] {
            let text = render(&job, roles);
            for key in ["active_colors", "inactive_colors", "disabled_colors"] {
                let colors = colors(&text, key);
                assert_eq!(colors.len(), roles, "{key}");
                assert!(
                    colors
                        .iter()
                        .all(|color| color.len() == 9 && color.starts_with("#ff")),
                    "{colors:?}"
                );
            }
        }
        let text = render(&job, 22);
        let active = colors(&text, "active_colors");
        assert_eq!(active[0], "#ffe0def4"); // WindowText: text
        assert_eq!(active[9], "#ff2a273f"); // Base: surface
        assert_eq!(active[10], "#ff232136"); // Window: base
        assert_eq!(active[12], "#ffea9a97"); // Highlight: the accent
        assert_eq!(active[21], "#ffea9a97"); // Accent
        assert_eq!(colors(&text, "inactive_colors"), active);
        let disabled = colors(&text, "disabled_colors");
        assert_eq!(disabled[0], "#ff6e6a86"); // muted
        assert_eq!(disabled[10], active[10]);
    }

    /// Light, Midlight, Mid, Dark run from bright to dim in either kind of
    /// theme.
    #[test]
    fn button_shades_are_ordered_by_brightness() {
        let sandbox = Sandbox::new("qt-shades");
        for theme in sandbox.ctx.themes.iter() {
            let text = render(&sandbox.job(&theme.id), 21);
            let active = colors(&text, "active_colors");
            let luminance = |index: usize| {
                format!("#{}", &active[index][3..])
                    .parse::<Rgba>()
                    .unwrap()
                    .luminance()
            };
            // Light > Midlight > Mid > Dark.
            let shades = [2, 3, 5, 4].map(luminance);
            assert!(
                shades.windows(2).all(|pair| pair[0] > pair[1]),
                "{}: {shades:?}",
                theme.id
            );
        }
    }

    #[test]
    fn writes_a_scheme_for_each_tool_there_is() {
        let mut sandbox = Sandbox::new("qt-apply");
        assert!(apply(&sandbox.job("rose-pine-moon")).unwrap().absent);
        assert!(!sandbox.root.join(".config/qt6ct").exists());

        // qt6ct installed but never run: its directory is made.
        sandbox.install("qt6ct");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(outcome.note, "palette written");
        assert_eq!(
            outcome.step.unwrap().what,
            "to use it, choose the custom colour scheme `enso` in qt6ct (Appearance)"
        );
        let scheme = sandbox.read(".config/qt6ct/colors/enso.conf");
        assert_eq!(colors(&scheme, "active_colors").len(), 22);
        assert!(!sandbox.root.join(".config/qt5ct").exists());
        // Nothing is left in the directory the tool watches.
        assert!(!sandbox.root.join(".config/qt6ct/.enso.conf.tmp").exists());

        // qt5ct known by its config directory; Qt 5 gets its 21 roles.
        sandbox.write(".config/qt5ct/qt5ct.conf", "[Appearance]\nstyle=Fusion\n");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.step.unwrap().what.contains("qt5ct and qt6ct"));
        let scheme = sandbox.read(".config/qt5ct/colors/enso.conf");
        assert_eq!(colors(&scheme, "active_colors").len(), 21);

        // Chosen in both: nothing left to do, and a switch rewrites them.
        for tool in ["qt5ct", "qt6ct"] {
            let path = sandbox
                .root
                .join(format!(".config/{tool}/colors/enso.conf"));
            sandbox.write(
                &format!(".config/{tool}/{tool}.conf"),
                &format!(
                    "[Appearance]\ncolor_scheme_path={}\ncustom_palette=true\nstyle=Fusion\n",
                    path.display()
                ),
            );
        }
        assert_eq!(
            apply(&sandbox.job("rose-pine-dawn")).unwrap(),
            Outcome::done("palette updated")
        );
        assert!(
            sandbox
                .read(".config/qt6ct/colors/enso.conf")
                .contains("#fffaf4ed")
        );
    }

    /// Put a platform theme's plugin where the sandbox's Qt finds plugins.
    pub(crate) fn install_plugin(sandbox: &mut Sandbox, file: &str) {
        let dir = sandbox.root.join("qt6/plugins");
        sandbox.write(&format!("qt6/plugins/platformthemes/{file}"), "");
        if !sandbox.host.qt_plugins.contains(&dir) {
            sandbox.host.qt_plugins.push(dir);
        }
    }

    #[test]
    fn qt_is_pointed_at_gtk_when_it_can_be() {
        let mut sandbox = Sandbox::new("qt-gtk");
        // Without a plugin there is nothing to point at.
        assert_eq!(platform_theme(&sandbox.job("rose-pine-moon")), None);

        install_plugin(&mut sandbox, "libqgtk3.so");
        assert_eq!(platform_theme(&sandbox.job("rose-pine-moon")), Some("gtk3"));
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(!outcome.absent);
        assert!(outcome.note.contains("QT_QPA_PLATFORMTHEME=gtk3"), "{outcome:?}");

        // qt5ct about: its scheme is written, but nothing asked of the
        // user, since Qt is not using it.
        sandbox.install("qt5ct");
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert_eq!(outcome.step, None);
        assert!(sandbox.root.join(".config/qt5ct/colors/enso.conf").is_file());

        // Qt or Niri left out of the targets: Niri's file says nothing.
        for places in [[Place::Niri, Place::Gtk], [Place::Qt, Place::Gtk]] {
            let job = Job {
                places: &places,
                ..sandbox.job("rose-pine-moon")
            };
            assert_eq!(platform_theme(&job), None);
        }

        // The session's own choice stays; one enso gives is no choice.
        sandbox.host.qt_platform_theme = Some("kde".into());
        assert_eq!(platform_theme(&sandbox.job("rose-pine-moon")), None);
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.step.unwrap().what.contains("qt5ct"));
        sandbox.host.qt_platform_theme = Some("gtk3".into());
        assert_eq!(platform_theme(&sandbox.job("rose-pine-moon")), Some("gtk3"));
    }

    #[test]
    fn qt6ct_is_preferred_and_set_up() {
        let mut sandbox = Sandbox::new("qt-qt6ct");
        install_plugin(&mut sandbox, "libqgtk3.so");
        install_plugin(&mut sandbox, "libqt6ct.so");
        sandbox.ctx.config.appearance.font = "Google Sans Flex".into();
        sandbox.ctx.config.appearance.mono_font = "monospace".into();
        sandbox.ctx.config.appearance.font_size = 14.0;
        assert_eq!(platform_theme(&sandbox.job("rose-pine-moon")), Some("qt6ct"));

        // Never run: the config is made, with everything enso needs.
        let outcome = apply(&sandbox.job("rose-pine-moon")).unwrap();
        assert!(outcome.note.contains("QT_QPA_PLATFORMTHEME=qt6ct"), "{outcome:?}");
        assert_eq!(outcome.step, None);
        let scheme = sandbox.root.join(".config/qt6ct/colors/enso.conf");
        let conf = sandbox.read(".config/qt6ct/qt6ct.conf");
        let get = |group, key| keyfile::get(&conf, group, key).map(str::to_owned);
        assert!(chosen(&conf), "{conf}");
        assert_eq!(get("Appearance", "color_scheme_path"), Some(scheme.display().to_string()));
        assert_eq!(get("Appearance", "standard_dialogs").as_deref(), Some("xdgdesktopportal"));
        // 14 px is 10.5 pt: 11. A generic monospace is left to the system.
        assert_eq!(
            get("Fonts", "general").as_deref(),
            Some("\"Google Sans Flex,11,-1,5,400,0,0,0,0,0,0,0,0,0,0,1\"")
        );
        assert_eq!(get("Fonts", "fixed"), None);

        // A config of the user's own: only enso's keys change.
        sandbox.write(
            ".config/qt6ct/qt6ct.conf",
            "[Appearance]\nstyle=kvantum\nstandard_dialogs=gtk3\n\n\
             [Interface]\ndouble_click_interval=400\n",
        );
        apply(&sandbox.job("rose-pine-dawn")).unwrap();
        let conf = sandbox.read(".config/qt6ct/qt6ct.conf");
        let get = |group, key| keyfile::get(&conf, group, key);
        assert_eq!(get("Appearance", "style"), Some("kvantum"));
        assert_eq!(get("Appearance", "standard_dialogs"), Some("xdgdesktopportal"));
        assert_eq!(get("Interface", "double_click_interval"), Some("400"));
        assert!(sandbox.read(".config/qt6ct/colors/enso.conf").contains("#fffaf4ed"));
    }

    #[test]
    fn a_scheme_is_chosen_only_when_switched_on() {
        let path = "color_scheme_path=/home/me/.config/qt6ct/colors/enso.conf";
        assert!(chosen(&format!(
            "[Appearance]\n{path}\ncustom_palette=true\n"
        )));
        assert!(!chosen(&format!(
            "[Appearance]\n{path}\ncustom_palette=false\n"
        )));
        assert!(!chosen(&format!("[Appearance]\n{path}\n")));
        assert!(!chosen(
            "[Appearance]\ncolor_scheme_path=/usr/share/qt6ct/colors/darker.conf\n\
             custom_palette=true\n"
        ));
        assert!(!chosen(""));
    }
}
