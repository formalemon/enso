//! Qt as a theme target, through qt5ct and qt6ct.
//!
//! Qt 6 follows the system's dark-or-light preference by itself (see
//! [`gtk`](super::gtk), which sets it). A palette is the business of a
//! platform theme, and the plain one is qt5ct / qt6ct: each takes a colour
//! scheme — three lists of colours, one per widget state — from a file in
//! its `colors` directory. enso writes `enso.conf` there; choosing it, once,
//! in the tool's Appearance tab is what brings it in.
//!
//! Both tools watch their config directory and re-read everything a few
//! seconds after something in it changes — but only that directory, not
//! `colors` below it. So the new scheme is written in the config directory
//! and moved down into place: the write is atomic, and the move is the
//! nudge that has running Qt programs recolour.

use std::fs;

use enso_core::{Palette, Rgba};

use super::{Job, Outcome, keyfile, replace};

/// Name of the colour scheme, as the tools list it.
const SCHEME: &str = "enso.conf";

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
        name: "qt6ct",
        roles: 22,
    },
];

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let host = job.host;
    let mut found = 0;
    let mut to_choose = Vec::new();
    for tool in &TOOLS {
        let dir = host.config.join(tool.name);
        if !dir.is_dir() && !host.has_program(tool.name) {
            continue;
        }
        found += 1;
        let colors = dir.join("colors");
        fs::create_dir_all(&colors).map_err(|err| format!("{}: {err}", colors.display()))?;
        replace(&colors.join(SCHEME), &render(job, tool.roles))?;
        if !chosen(&fs::read_to_string(dir.join(format!("{}.conf", tool.name))).unwrap_or_default())
        {
            to_choose.push(tool.name);
        }
    }
    Ok(match (found, to_choose.as_slice()) {
        (0, _) => Outcome::absent("no qt5ct or qt6ct"),
        (_, []) => Outcome::done("palette updated"),
        (_, tools) => Outcome::done("palette written").step(
            format!(
                "to use it, choose the custom colour scheme `enso` in {} (Appearance)",
                tools.join(" and ")
            ),
            Vec::<String>::new(),
        ),
    })
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
mod tests {
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
