//! `enso-greet enable` and `disable`: what making it the login screen
//! takes.
//!
//! The greeter runs before anyone has logged in, as greetd's own user, so
//! everything it reads has to be outside any home: the program, its niri
//! configuration, and its settings. Those are root's to put in place. This
//! writes them where the user can read them first, then says exactly what
//! to run as root; it never runs anything as root itself.

use std::{
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

/// Where root puts the program: the `greeter` user can't read a home.
pub const PROGRAM: &str = "/usr/local/bin/enso-greet";
/// The greeter's niri configuration.
pub const NIRI_CONFIG: &str = "/etc/greetd/enso-greet.kdl";
/// greetd's configuration.
pub const GREETD_CONFIG: &str = "/etc/greetd/config.toml";
/// The greeter's own settings, beside the suite's system-wide config.
pub const SYSTEM_DIR: &str = "/etc/enso";
pub const GREET_CONFIG: &str = "/etc/enso/greet.toml";
pub const SYSTEM_CONFIG: &str = "/etc/enso/config.toml";
/// The terraces, quietened, drawn once by `enable`.
pub const BACKDROP: &str = "/usr/local/share/enso/greet/backdrop.png";
/// What moves in it: water, stars, petals, mist.
pub const BACKDROP_LIFE: &str = "/usr/local/share/enso/greet/backdrop.life";
/// The boot splash (Plymouth) theme: the same backdrop while the machine
/// starts, so the screen is never bare between the boot menu and the login.
pub const PLYMOUTH_THEME: &str = "/usr/share/plymouth/themes/enso";
/// Plymouth leaves its last frame up when it goes, for the login screen.
pub const PLYMOUTH_DROP_IN: &str = "/etc/systemd/system/plymouth-quit.service.d/enso.conf";
/// Fonts every user, the greeter too, can read.
const SYSTEM_FONTS: &str = "/usr/local/share/fonts";

/// What `enable` is given to work with.
#[derive(Clone, Debug)]
pub struct Setup {
    /// Where the files are written for root to copy.
    pub dir: PathBuf,
    /// This program, as built.
    pub program: PathBuf,
    /// Who the greeter offers first.
    pub user: String,
    /// The theme's base colour, for niri to show before the greeter draws.
    pub base: String,
    /// The interface font, if it is one only this user has: its family and
    /// the folder its files are in.
    pub private_font: Option<(String, PathBuf)>,
    /// The terraces, quietened: drawn here, since drawing them takes
    /// seconds the login screen shouldn't spend.
    pub backdrop: enso_ui::Picture,
    /// What moves in the backdrop.
    pub life: enso_scene::Life,
}

/// The greeter's niri: nothing in it but enso-greet.
fn niri_config(base: &str) -> String {
    format!(
        r#"// The login screen's own niri, started by greetd as the `greeter` user
// to run enso-greet and nothing else. Written by `enso-greet enable`.

input {{
    keyboard {{
        numlock
    }}
    touchpad {{
        tap
    }}
}}

cursor {{
    hide-when-typing
    hide-after-inactive-ms 2000
}}

layout {{
    background-color "{base}"
}}

hotkey-overlay {{
    skip-at-startup
}}

prefer-no-csd
screenshot-path null

// Nothing here is an X11 program.
xwayland-satellite {{
    off
}}

spawn-at-startup "{PROGRAM}"

binds {{
    // Should the greeter ever stop answering: leave, and greetd starts it
    // again.
    Ctrl+Alt+Delete {{ quit skip-confirmation=true; }}
}}
"#
    )
}

fn plymouth_theme() -> String {
    format!(
        "# The login screen's backdrop while the machine starts. Written by\n\
         # `enso-greet enable`.\n\n\
         [Plymouth Theme]\n\
         Name=enso\n\
         Description=The terraces at dusk, quietened, from the boot menu to the login\n\
         ModuleName=script\n\n\
         [script]\n\
         ImageDir={PLYMOUTH_THEME}\n\
         ScriptFile={PLYMOUTH_THEME}/enso.script\n"
    )
}

/// The splash: the backdrop, still, from the boot menu's last frame to the
/// login screen's first. Nothing on it moves: Plymouth only keeps the
/// screen while the machine starts.
fn plymouth_script(base: &str) -> String {
    let channel = |at: usize| {
        u8::from_str_radix(base.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0
    };
    let (r, g, b) = (channel(1), channel(3), channel(5));
    format!(
        "// The login screen's backdrop, still, while the machine starts: the\n\
         // terraces at dusk, quietened. Written by `enso-greet enable`.\n\n\
         Window.SetBackgroundTopColor({r:.4}, {g:.4}, {b:.4});\n\
         Window.SetBackgroundBottomColor({r:.4}, {g:.4}, {b:.4});\n\n\
         backdrop = Image(\"backdrop.png\");\n\
         width = Window.GetWidth();\n\
         height = Window.GetHeight();\n\
         if (backdrop.GetWidth() != width || backdrop.GetHeight() != height) {{\n\
         \x20   scale = Math.Max(width / backdrop.GetWidth(), height / backdrop.GetHeight());\n\
         \x20   backdrop = backdrop.Scale(backdrop.GetWidth() * scale, backdrop.GetHeight() * scale);\n\
         }}\n\
         wall = Sprite(backdrop);\n\
         wall.SetPosition(Math.Int((width - backdrop.GetWidth()) / 2), Math.Int((height - backdrop.GetHeight()) / 2), -100);\n"
    )
}

/// Plymouth goes when the system is up, as always, but leaves its last
/// frame on the screen: the login screen draws over it the same picture.
fn plymouth_drop_in() -> String {
    "# Written by `enso-greet enable`: the boot splash's last frame stays up\n\
     # until the login screen draws over it.\n\
     [Service]\n\
     ExecStart=\n\
     ExecStart=-/usr/bin/plymouth quit --retain-splash\n"
        .to_owned()
}

fn greetd_config() -> String {
    format!(
        r#"# greetd, with enso-greet as the login screen. Written by `enso-greet
# enable`.

[terminal]
vt = 1

[general]
# enso-greet starts the session through the user's login shell, which
# reads their profile itself.
source_profile = false

[default_session]
# niri's log goes to the journal (`journalctl -b -t enso-greet`), not over
# the login screen's console.
command = "systemd-cat --identifier=enso-greet niri -c {NIRI_CONFIG}"
user = "greeter"
"#
    )
}

fn greet_config(user: &str) -> String {
    format!(
        "# enso-greet's settings. Written by `enso-greet enable`.\n\n\
         # Who is offered first; the others are a Tab away.\n\
         default_user = {user:?}\n\
         # Started, once logged in, by the user's login shell.\n\
         command = \"niri-session\"\n"
    )
}

fn system_config(font: &str) -> String {
    format!(
        "# Settings for enso where nobody has logged in yet: the login screen.\n\
         # Written by `enso-greet enable`.\n\n\
         [appearance]\n\
         font = {font:?}\n"
    )
}

/// Write the files, and return what root has to run.
pub fn enable(setup: &Setup) -> io::Result<String> {
    fs::create_dir_all(&setup.dir)?;
    let write = |name: &str, text: String| -> io::Result<PathBuf> {
        let path = setup.dir.join(name);
        enso_core::disk::write_atomic(&path, text.as_bytes(), 0o644)?;
        Ok(path)
    };
    let niri = write("enso-greet.kdl", niri_config(&setup.base))?;
    let greetd = write("config.toml", greetd_config())?;
    let greet = write("greet.toml", greet_config(&setup.user))?;
    let backdrop = setup.dir.join("backdrop.png");
    setup.backdrop.save_png(&backdrop)?;
    let life = setup.dir.join("backdrop.life");
    enso_core::disk::write_atomic(&life, &setup.life.encode(), 0o644)?;
    // The splash theme, whole, in a folder of its own: copied as it is.
    let theme = setup.dir.join("plymouth");
    match fs::remove_dir_all(&theme) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    fs::create_dir_all(&theme)?;
    let put = |name: &str, text: String| {
        enso_core::disk::write_atomic(&theme.join(name), text.as_bytes(), 0o644)
    };
    put("enso.plymouth", plymouth_theme())?;
    put("enso.script", plymouth_script(&setup.base))?;
    setup.backdrop.save_png(theme.join("backdrop.png"))?;
    let drop_in = write("plymouth-quit.conf", plymouth_drop_in())?;
    let system = match &setup.private_font {
        Some((family, _)) => Some(write("enso.toml", system_config(family))?),
        None => None,
    };

    let mut out = String::new();
    let shown = |path: &Path| path.display().to_string();
    let _ = writeln!(
        out,
        "The login screen's files are ready in {}.\n",
        shown(&setup.dir)
    );
    let _ = writeln!(out, "Put them in place as root:\n");
    let _ = writeln!(
        out,
        "  sudo install -Dm755 {} {PROGRAM}",
        shown(&setup.program)
    );
    let _ = writeln!(out, "  sudo install -Dm644 {} {NIRI_CONFIG}", shown(&niri));
    let _ = writeln!(out, "  sudo install -Dm644 {} {BACKDROP}", shown(&backdrop));
    let _ = writeln!(
        out,
        "  sudo install -Dm644 {} {BACKDROP_LIFE}",
        shown(&life)
    );
    let _ = writeln!(
        out,
        "  sudo install -Dm644 {} {GREET_CONFIG}",
        shown(&greet)
    );
    if let (Some(system), Some((family, fonts))) = (&system, &setup.private_font) {
        let _ = writeln!(
            out,
            "  sudo install -Dm644 {} {SYSTEM_CONFIG}",
            shown(system)
        );
        let _ = writeln!(
            out,
            "  sudo cp -r {} {SYSTEM_FONTS}/    # {family}, for the greeter",
            shown(fonts)
        );
    }
    let _ = writeln!(
        out,
        "  sudo install -Dm644 {} {GREETD_CONFIG}",
        shown(&greetd)
    );
    let _ = writeln!(
        out,
        "\nThen make it the login and restart. It has to start with the machine:\n\
         while a desktop is running, that desktop holds the screen and the\n\
         login screen can't have it.\n\n\
         \x20 sudo systemctl enable greetd && systemctl reboot\n\n\
         Should it not come up: Ctrl+Alt+F2, log in there, and\n\n\
         \x20 sudo systemctl disable greetd && systemctl reboot\n\n\
         brings the console login back. What it said: journalctl -b -t enso-greet\n\n\
         `enso-greet disable` says how to undo all of this. After rebuilding,\n\
         run the first line again."
    );
    let _ = writeln!(
        out,
        "\nThe boot splash: the same backdrop, still, from the boot menu to the\n\
         login screen while the machine starts (needs plymouth: pacman -S\n\
         plymouth):\n\n\
         \x20 sudo rm -rf {PLYMOUTH_THEME} && sudo cp -r {theme} {PLYMOUTH_THEME}\n\
         \x20 sudo install -Dm644 {drop_in} {PLYMOUTH_DROP_IN}\n\
         \x20 sudo plymouth-set-default-theme enso\n\n\
         Then Plymouth in the initramfs, after `systemd` in HOOKS, and rebuilt:\n\n\
         \x20 sudo sed -i 's/^HOOKS=(base systemd /HOOKS=(base systemd plymouth /' /etc/mkinitcpio.conf\n\
         \x20 sudo mkinitcpio -P    # again whenever the splash changes\n\n\
         And `splash` among the kernel's options (with `quiet`), on the options\n\
         line of the boot entry in /boot/loader/entries/.",
        theme = shown(&theme),
        drop_in = shown(&drop_in),
    );
    Ok(out)
}

/// Remove the staged files, and return what root has to run.
pub fn disable(dir: &Path) -> io::Result<String> {
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    Ok(format!(
        "To log in on the console again, as root:\n\n\
         \x20 sudo systemctl disable greetd    # from the next start\n\n\
         And to remove the login screen's files:\n\n\
         \x20 sudo rm {PROGRAM} {NIRI_CONFIG} {GREET_CONFIG} {BACKDROP} {BACKDROP_LIFE}\n\
         \x20 sudo rm -r {PLYMOUTH_THEME} {PLYMOUTH_DROP_IN}    # and plymouth out of HOOKS\n\
\n\
         {GREETD_CONFIG} can stay: greetd reads it only when enabled.\n\
         {SYSTEM_CONFIG} and any font copied to {SYSTEM_FONTS} are left: remove them\n\
         too if nothing else uses them.\n"
    ))
}

/// The folder `family` was fetched into, when it is one of this user's own
/// fonts: `~/.local/share/fonts/<Family without spaces>`, as sumi's
/// `install` lays them out.
pub fn private_font(fonts: &Path, family: &str) -> Option<PathBuf> {
    let dir = fonts.join(family.replace(' ', ""));
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("enso-greet-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn enable_writes_the_files_and_says_what_root_runs() {
        let root = scratch("enable");
        let fonts = root.join("fonts");
        fs::create_dir_all(fonts.join("GoogleSansFlex")).unwrap();
        let setup = Setup {
            dir: root.join("greet"),
            program: PathBuf::from("/build/enso-greet"),
            user: "kai".to_owned(),
            base: "#232136".to_owned(),
            private_font: private_font(&fonts, "Google Sans Flex")
                .map(|dir| ("Google Sans Flex".to_owned(), dir)),
            backdrop: enso_ui::Picture::solid(4, 3, enso_core::Rgba::rgb(1, 2, 3)),
            life: enso_scene::Life::default(),
        };
        // Left from a splash that painted the ensō: gone once enabled again.
        fs::create_dir_all(setup.dir.join("plymouth")).unwrap();
        fs::write(setup.dir.join("plymouth/stroke-0.png"), b"old").unwrap();
        assert!(private_font(&fonts, "Inter").is_none());
        let said = enable(&setup).unwrap();

        let read = |name: &str| fs::read_to_string(setup.dir.join(name)).unwrap();
        let niri = read("enso-greet.kdl");
        assert!(niri.contains(&format!("spawn-at-startup \"{PROGRAM}\"")));
        assert!(niri.contains("background-color \"#232136\""));
        let greetd = read("config.toml");
        assert!(greetd.contains(&format!("niri -c {NIRI_CONFIG}\"")));
        assert!(greetd.contains("systemd-cat --identifier=enso-greet"));
        assert!(greetd.contains("user = \"greeter\""));
        let greet: crate::greet::GreetConfig = toml::from_str(&read("greet.toml")).unwrap();
        assert_eq!(greet.default_user, "kai");
        let system = enso_core::Config::parse(&read("enso.toml"), "enso.toml").unwrap();
        assert_eq!(system.appearance.font, "Google Sans Flex");

        assert!(said.contains("sudo install -Dm755 /build/enso-greet /usr/local/bin/enso-greet"));
        assert!(said.contains("GoogleSansFlex /usr/local/share/fonts/"));
        assert!(said.contains(&format!("backdrop.png {BACKDROP}")));
        assert!(said.contains("plymouth-set-default-theme enso"));
        let script = read("plymouth/enso.script");
        assert!(script.contains("Image(\"backdrop.png\")"));
        assert!(script.contains("SetBackgroundTopColor(0.1373, 0.1294, 0.2118)"));
        // Still: nothing on it is redrawn.
        assert!(!script.contains("SetRefreshFunction"));
        assert!(read("plymouth/enso.plymouth").contains("ModuleName=script"));
        let files: Vec<_> = fs::read_dir(setup.dir.join("plymouth"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        let mut files = files;
        files.sort();
        assert_eq!(files, ["backdrop.png", "enso.plymouth", "enso.script"]);
        assert!(said.contains("sudo cp -r"));
        assert!(read("plymouth-quit.conf").contains("quit --retain-splash"));
        let saved = enso_ui::Picture::open(setup.dir.join("backdrop.png")).unwrap();
        assert!(saved == setup.backdrop);
        assert!(said.contains("sudo systemctl enable greetd && systemctl reboot"));
        assert!(!said.contains("systemctl start greetd"));

        let undone = disable(&setup.dir).unwrap();
        assert!(!setup.dir.exists());
        assert!(undone.contains("sudo systemctl disable greetd"));
        // Twice is fine.
        disable(&setup.dir).unwrap();
        let _ = fs::remove_dir_all(&root);
    }
}
