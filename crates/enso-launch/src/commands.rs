//! Things to do rather than open, typed by name: lock, sleep, a theme,
//! Wi-Fi off, Do not disturb, keep awake — and the control center's pages,
//! Settings, Wi-Fi, Sound, Displays, by name.
//!
//! Each is one of the suite's own commands (`enso-ctl`, `enso-power`, …),
//! found next to the launcher or on `PATH`, so a tool that isn't installed
//! simply has nothing to run. They rank below applications. Those that end
//! the session — log out, restart, shut down — are listed only once most of
//! their name is typed, so a stray Enter never takes the session with it.

use enso_core::{Figure, Registry};

/// One command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    /// More words it answers to.
    pub words: Vec<&'static str>,
    pub figure: Figure,
    /// The suite's program to run, and its arguments.
    pub program: &'static str,
    pub args: Vec<String>,
    /// It ends the session: listed only for a deliberate query.
    pub drastic: bool,
}

fn command(
    name: &str,
    words: &[&'static str],
    figure: Figure,
    program: &'static str,
    args: &[&str],
) -> Command {
    Command {
        name: name.to_owned(),
        words: words.to_vec(),
        figure,
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        drastic: false,
    }
}

/// Every command, the themes among them.
pub fn all(themes: &Registry) -> Vec<Command> {
    let drastic = |command: Command| Command {
        drastic: true,
        ..command
    };
    let mut commands = vec![
        // The control center's pages, by what they're for.
        command(
            "Settings",
            &["control center", "preferences", "lock after", "sleep"],
            Figure::Sliders,
            "enso-ctl",
            &["--page", "settings"],
        ),
        command(
            "Wi-Fi",
            &["wifi", "network", "wireless", "internet"],
            Figure::Wifi(4),
            "enso-ctl",
            &["--page", "wifi"],
        ),
        command(
            "Bluetooth",
            &["bt", "headphones", "pair"],
            Figure::Bluetooth,
            "enso-ctl",
            &["--page", "bluetooth"],
        ),
        command(
            "Sound",
            &["volume", "audio", "speakers", "microphone", "output"],
            Figure::Speaker(3),
            "enso-ctl",
            &["--page", "sound"],
        ),
        command(
            "Displays",
            &["screens", "monitor", "resolution", "scale", "refresh"],
            Figure::Display,
            "enso-ctl",
            &["--page", "displays"],
        ),
        command(
            "Appearance",
            &["theme", "colours", "colors", "dark", "light"],
            Figure::Palette,
            "enso-ctl",
            &["--page", "theme"],
        ),
        command("Lock screen", &["lock"], Figure::Lock, "enso-lock", &[]),
        command(
            "Sleep",
            &["suspend"],
            Figure::Sleep,
            "enso-power",
            &["suspend"],
        ),
        drastic(command(
            "Log out",
            &["logout", "sign out"],
            Figure::Logout,
            "enso-power",
            &["logout"],
        )),
        drastic(command(
            "Restart",
            &["reboot"],
            Figure::Restart,
            "enso-power",
            &["restart"],
        )),
        drastic(command(
            "Shut down",
            &["power off", "shutdown"],
            Figure::Power,
            "enso-power",
            &["shutdown"],
        )),
        command(
            "Do not disturb",
            &["dnd", "quiet", "focus"],
            Figure::BellOff,
            "enso-ctl",
            &["dnd", "toggle"],
        ),
        command(
            "Do not disturb for an hour",
            &["dnd", "quiet", "focus"],
            Figure::BellOff,
            "enso-ctl",
            &["dnd", "1h"],
        ),
        command(
            "Keep awake",
            &["caffeine", "awake", "no sleep"],
            Figure::Cup,
            "enso-ctl",
            &["awake", "toggle"],
        ),
        command(
            "Keep awake for an hour",
            &["caffeine", "awake", "no sleep"],
            Figure::Cup,
            "enso-ctl",
            &["awake", "1h"],
        ),
        command(
            "Wi-Fi on",
            &["wifi", "wireless"],
            Figure::Wifi(4),
            "enso-ctl",
            &["wifi", "on"],
        ),
        command(
            "Wi-Fi off",
            &["wifi", "wireless", "airplane"],
            Figure::WifiOff,
            "enso-ctl",
            &["wifi", "off"],
        ),
        command(
            "Bluetooth on",
            &["bt"],
            Figure::Bluetooth,
            "enso-ctl",
            &["bluetooth", "on"],
        ),
        command(
            "Bluetooth off",
            &["bt"],
            Figure::Bluetooth,
            "enso-ctl",
            &["bluetooth", "off"],
        ),
        command(
            "Pick a colour",
            &["color", "colour", "picker", "eyedropper"],
            Figure::Palette,
            "enso-ctl",
            &["colour"],
        ),
        command(
            "Night light",
            &["warm", "dusk", "blue light", "night shift"],
            Figure::Sleep,
            "enso-dusk",
            &["toggle"],
        ),
        command(
            "Night light: follow the sun",
            &["warm", "dusk", "night shift"],
            Figure::Sleep,
            "enso-dusk",
            &["sun"],
        ),
        command(
            "Notifications",
            &["history", "dnd"],
            Figure::Bell,
            "enso-notify",
            &["history", "open"],
        ),
        command(
            "Next theme",
            &["dark", "light", "style"],
            Figure::Palette,
            "enso-ctl",
            &["theme", "next"],
        ),
    ];
    for theme in themes.iter() {
        let mut theme_command = command(
            &format!("Theme: {}", theme.name),
            &["theme", "dark", "light", "style"],
            Figure::Palette,
            "enso-ctl",
            &["theme", "set"],
        );
        theme_command.args.push(theme.id.clone());
        commands.push(theme_command);
    }
    commands
}

impl Command {
    /// Whether a query this long and this like the name may list it.
    pub fn allowed(&self, query: &str) -> bool {
        if !self.drastic {
            return true;
        }
        let query = query.trim().to_lowercase();
        query.chars().count() >= 4
            && std::iter::once(self.name.to_lowercase())
                .chain(self.words.iter().map(|word| (*word).to_owned()))
                .any(|name| name.starts_with(&query))
    }

    /// Run it, detached; it is the suite's own program, next to this one.
    pub fn run(&self) -> std::io::Result<()> {
        let program = enso_core::exe::companion(self.program);
        let mut argv = vec![program.to_string_lossy().into_owned()];
        argv.extend(self.args.iter().cloned());
        enso_fs::apps::spawn_detached(&argv, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drastic_commands_wait_for_most_of_their_name() {
        let themes = Registry::builtin();
        let commands = all(&themes);
        let restart = commands.iter().find(|c| c.name == "Restart").unwrap();
        assert!(!restart.allowed("re"));
        assert!(!restart.allowed("res"));
        assert!(restart.allowed("rest"));
        assert!(restart.allowed("reboot"));
        let sleep = commands.iter().find(|c| c.name == "Sleep").unwrap();
        assert!(sleep.allowed("s"));
        // The control center's pages are found by name and by what they do.
        let page = |name: &str| commands.iter().find(|c| c.name == name).unwrap();
        assert_eq!(page("Displays").args, ["--page", "displays"]);
        assert!(page("Sound").words.contains(&"volume"));
        // Every theme is a command too.
        assert!(
            commands
                .iter()
                .any(|c| c.name.starts_with("Theme: Rosé Pine"))
        );
    }
}
