//! `enso-files portal enable | disable | status`: make enso-files the file
//! chooser every application gets through the desktop portal, or stop.
//!
//! Everything goes in the user's own directories; nothing needs root, and
//! `disable` takes all of it away again:
//!
//! - **The service**, `$XDG_DATA_HOME/dbus-1/services/…desktop.enso.service`,
//!   so the session bus starts `enso-files --service` (this very binary)
//!   when the portal first asks.
//! - **A portals directory**, `$XDG_DATA_HOME/enso/portals`. The portal
//!   reads its `.portal` files from one directory — the system's, or
//!   `$XDG_DESKTOP_PORTAL_DIR` — so this one holds a link to every system
//!   `.portal` file and enso's own, and with it the routing: the desktop's
//!   own `<desktop>-portals.conf`, with file choosers sent to enso. (With
//!   its directory moved, the portal reads its routing from there too.)
//! - **A drop-in** for `xdg-desktop-portal.service` setting
//!   `XDG_DESKTOP_PORTAL_DIR`.
//!
//! The session bus is then told to reload, systemd to reread its units, and
//! the portal is restarted, so the next "choose a file" is enso's.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use crate::service::{FILE_MANAGER, NAME};

/// What tells the portal enso-files is a file chooser.
const PORTAL_FILE: &str = "[portal]
DBusName=org.freedesktop.impl.portal.desktop.enso
Interfaces=org.freedesktop.impl.portal.FileChooser;
";

const FILE_CHOOSER: &str = "org.freedesktop.impl.portal.FileChooser";
const SYSTEM_PORTALS: &str = "/usr/share/xdg-desktop-portal";

/// Where everything goes.
#[derive(Clone, Debug)]
struct Places {
    portals: PathBuf,
    service: PathBuf,
    /// The same service under the name "Show in folder" asks for.
    file_manager: PathBuf,
    dropin: PathBuf,
    /// The system's portal directory and routing files.
    system: PathBuf,
    /// The user's own routing, which wins over the system's when present.
    user_config: PathBuf,
}

impl Places {
    fn from_env() -> Self {
        let home = enso_fs::places::home();
        let dir = |var: &str, fallback: &str| {
            env::var_os(var)
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let data = dir("XDG_DATA_HOME", ".local/share");
        let config = dir("XDG_CONFIG_HOME", ".config");
        Self {
            portals: data.join("enso/portals"),
            service: data.join(format!("dbus-1/services/{NAME}.service")),
            file_manager: data.join(format!("dbus-1/services/{FILE_MANAGER}.service")),
            dropin: config.join("systemd/user/xdg-desktop-portal.service.d/enso.conf"),
            system: PathBuf::from(SYSTEM_PORTALS),
            user_config: config.join("xdg-desktop-portal"),
        }
    }
}

pub fn run(command: &str) -> ExitCode {
    let places = Places::from_env();
    let done = match command {
        "enable" => {
            let exe = enso_core::exe::installed();
            enable(&places, &exe, &desktops()).and_then(|()| {
                let taken = enso_fs::open::adopt(ENTRY, &entry(&exe), &["inode/directory"])?;
                println!("enso-files is the file chooser now.");
                if !taken.is_empty() {
                    println!("Folders open in it too.");
                }
                Ok(())
            }).map(|()| {
                println!("  portal files  {}", places.portals.display());
                println!("  service       {}", places.service.display());
                println!("  drop-in       {}", places.dropin.display());
                println!("`enso-files portal disable` gives the choosing back.");
            })
        }
        "disable" => disable(&places).and_then(|()| enso_fs::open::abandon(ENTRY)).map(|()| {
            println!("Files are chosen as before enso-files: its portal set-up is gone.");
        }),
        "status" => {
            status(&places);
            return ExitCode::SUCCESS;
        }
        // Run by the portal itself as it starts (the drop-in's
        // ExecStartPre): the system's portal files linked afresh, so one
        // installed since — xdg-desktop-portal-gnome, for sharing the
        // screen — is seen without enabling again.
        "refresh" => {
            return match link_portals(&places, &desktops()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("enso-files: cannot refresh the portal files: {err}");
                    ExitCode::FAILURE
                }
            };
        }
        other => {
            eprintln!("enso-files: `portal {other}`? It's enable, disable, status, or refresh.");
            return ExitCode::from(2);
        }
    };
    if let Err(err) = done {
        eprintln!("enso-files: {err}");
        return ExitCode::FAILURE;
    }
    restart();
    ExitCode::SUCCESS
}

/// Its desktop entry: the launcher lists it as Files, and folders opened
/// anywhere (the launcher's Ctrl+Enter, a browser's "show in folder") open
/// in it, unless something else was chosen for them.
const ENTRY: &str = "enso-files.desktop";

fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Files\n\
         GenericName=File Manager\n\
         Comment=Browse, find, and tidy your files\n\
         Keywords=folder;explorer;manager;browse;\n\
         Exec={} %f\n\
         Icon=system-file-manager\n\
         Terminal=false\n\
         Categories=System;FileTools;FileManager;\n\
         MimeType=inode/directory;\n\
         StartupWMClass=enso-files\n",
        exe.display()
    )
}

/// The desktops the portal routes for: `$XDG_CURRENT_DESKTOP`, lower-cased.
fn desktops() -> Vec<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|name| !name.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn enable(places: &Places, exe: &Path, desktops: &[String]) -> io::Result<()> {
    link_portals(places, desktops)?;

    let service = format!(
        "[D-BUS Service]\nName={NAME}\nExec={} --service\n",
        exe.display()
    );
    write_new(&places.service, &service)?;
    // "Show in folder" — unless something else already answers it.
    if !places.file_manager.exists() || is_ours(&places.file_manager) {
        let file_manager = format!(
            "[D-BUS Service]\nName={FILE_MANAGER}\nExec={} --service\n",
            exe.display()
        );
        write_new(&places.file_manager, &file_manager)?;
    }
    // The portal links the system's portal files again each time it
    // starts, so a portal package installed later is seen.
    let dropin = format!(
        "# Written by `enso-files portal enable`; `enso-files portal disable` removes it.\n\
         [Service]\nEnvironment=XDG_DESKTOP_PORTAL_DIR={}\nExecStartPre=-{} portal refresh\n",
        places.portals.display(),
        exe.display()
    );
    write_new(&places.dropin, &dropin)
}

/// The portal files — the system's as links, renewed so packages that came
/// or went are seen, and enso's own — and the routing.
fn link_portals(places: &Places, desktops: &[String]) -> io::Result<()> {
    fs::create_dir_all(&places.portals)?;
    for entry in fs::read_dir(&places.portals)? {
        let path = entry?.path();
        if path.is_symlink()
            || path
                .extension()
                .is_some_and(|ext| ext == "portal" || ext == "conf")
        {
            fs::remove_file(&path)?;
        }
    }
    if let Ok(system) = fs::read_dir(places.system.join("portals")) {
        for entry in system {
            let path = entry?.path();
            let name = path.file_name().unwrap_or_default();
            if path.extension().is_some_and(|ext| ext == "portal") && name != "enso.portal" {
                std::os::unix::fs::symlink(&path, places.portals.join(name))?;
            }
        }
    }
    fs::write(places.portals.join("enso.portal"), PORTAL_FILE)?;

    // The routing: for each desktop and for any, what the user or the
    // system already says, with file choosers sent to enso.
    let names: Vec<String> = desktops
        .iter()
        .map(|desktop| format!("{desktop}-portals.conf"))
        .chain(["portals.conf".to_owned()])
        .collect();
    for name in names {
        let base = [places.user_config.join(&name), places.system.join(&name)]
            .iter()
            .find_map(|path| fs::read_to_string(path).ok())
            .unwrap_or_default();
        fs::write(places.portals.join(&name), route(&base))?;
    }
    Ok(())
}

/// Whether a service file is one `enable` wrote.
fn is_ours(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|text| text.contains("enso-files"))
}

fn write_new(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, text)
}

/// What only GNOME's portal does on niri: sharing the screen, screenshots
/// for programs that ask, and remote desktop — niri speaks GNOME's own
/// screen-casting interface, which xdg-desktop-portal-gnome turns into
/// these.
const SCREEN: [&str; 3] = [
    "org.freedesktop.impl.portal.ScreenCast",
    "org.freedesktop.impl.portal.Screenshot",
    "org.freedesktop.impl.portal.RemoteDesktop",
];

/// A `portals.conf` that says what `base` says, with file choosers sent to
/// enso. Without a `[preferred]` section, everything else goes to GTK's
/// portal, the usual fallback — and the screen to GNOME's, the one portal
/// that shares a niri screen. (niri's own routing already says gnome first.)
fn route(base: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_preferred = false;
    let mut seen_preferred = false;
    for line in base.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_preferred {
                lines.push(format!("{FILE_CHOOSER}=enso"));
            }
            in_preferred = trimmed == "[preferred]";
            seen_preferred |= in_preferred;
        } else if in_preferred
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == FILE_CHOOSER)
        {
            continue;
        }
        lines.push(line.to_owned());
    }
    if in_preferred {
        lines.push(format!("{FILE_CHOOSER}=enso"));
    }
    if !seen_preferred {
        lines.push("[preferred]".to_owned());
        lines.push("default=gtk;".to_owned());
        lines.push(format!("{FILE_CHOOSER}=enso"));
        lines.extend(SCREEN.iter().map(|interface| format!("{interface}=gnome;")));
    }
    let mut text = "# Written by `enso-files portal enable`: the routing this desktop had,\n\
                    # with file choosers sent to enso-files.\n"
        .to_owned();
    text += &lines.join("\n");
    text.push('\n');
    text
}

fn disable(places: &Places) -> io::Result<()> {
    let file_manager = Some(&places.file_manager).filter(|path| is_ours(path));
    for file in [Some(&places.dropin), Some(&places.service), file_manager]
        .into_iter()
        .flatten()
    {
        match fs::remove_file(file) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    if let Some(dir) = places.dropin.parent() {
        // Only if it's empty: other drop-ins aren't ours.
        let _ = fs::remove_dir(dir);
    }
    match fs::remove_dir_all(&places.portals) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

fn status(places: &Places) {
    let has = |path: &Path| if path.exists() { "present" } else { "absent" };
    let routed = desktops()
        .iter()
        .map(|desktop| format!("{desktop}-portals.conf"))
        .chain(["portals.conf".to_owned()])
        .any(|name| {
            fs::read_to_string(places.portals.join(name))
                .is_ok_and(|text| text.contains(&format!("{FILE_CHOOSER}=enso")))
        });
    println!(
        "enso-files is {}the file chooser.",
        if routed && places.service.exists() && places.dropin.exists() {
            ""
        } else {
            "not "
        }
    );
    println!(
        "  portal files  {} ({})",
        places.portals.display(),
        has(&places.portals)
    );
    println!(
        "  service       {} ({})",
        places.service.display(),
        has(&places.service)
    );
    println!(
        "  drop-in       {} ({})",
        places.dropin.display(),
        has(&places.dropin)
    );
    let gnome = places.system.join("portals/gnome.portal").exists();
    println!(
        "Sharing the screen {}",
        if gnome {
            "goes through xdg-desktop-portal-gnome."
        } else {
            "needs xdg-desktop-portal-gnome, which isn't installed."
        }
    );
}

/// Make the change take: the bus rereads its services, systemd its units,
/// and the portal starts again. Each is best effort, and says if it failed.
fn restart() {
    let reloaded = zbus::block_on(async {
        let bus = zbus::Connection::session().await?;
        bus.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ReloadConfig",
            &(),
        )
        .await
        .map(|_| ())
    });
    if let Err(err) = reloaded {
        eprintln!("enso-files: the session bus didn't reload ({err}); log out and in to finish");
    }
    if let Err(err) = enso_sys::systemd::reload() {
        eprintln!("enso-files: systemd didn't reload ({err}); log out and in to finish");
    }
    if let Err(err) = enso_sys::systemd::try_restart("xdg-desktop-portal.service") {
        eprintln!("enso-files: the portal didn't restart ({err}); log out and in to finish");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> Places {
        // Cleared and reused by the next run, so they don't pile up.
        let root = env::temp_dir().join(format!("enso-files-portal-{name}"));
        let _ = fs::remove_dir_all(&root);
        let system = root.join("usr/share/xdg-desktop-portal");
        fs::create_dir_all(system.join("portals")).unwrap();
        fs::write(system.join("portals/gtk.portal"), "[portal]\n").unwrap();
        fs::write(
            system.join("niri-portals.conf"),
            "[preferred]\ndefault=gnome;gtk;\norg.freedesktop.impl.portal.FileChooser=gtk;\norg.freedesktop.impl.portal.Secret=gnome-keyring;\n",
        )
        .unwrap();
        Places {
            portals: root.join("data/enso/portals"),
            service: root.join(format!("data/dbus-1/services/{NAME}.service")),
            file_manager: root.join(format!("data/dbus-1/services/{FILE_MANAGER}.service")),
            dropin: root.join("config/systemd/user/xdg-desktop-portal.service.d/enso.conf"),
            system,
            user_config: root.join("config/xdg-desktop-portal"),
        }
    }

    #[test]
    fn with_no_routing_the_screen_goes_to_gnome() {
        let routed = route("");
        assert!(routed.contains("org.freedesktop.impl.portal.ScreenCast=gnome;"));
        assert!(routed.contains("org.freedesktop.impl.portal.Screenshot=gnome;"));
        // Routing of one's own is kept as it is.
        let own = route("[preferred]\ndefault=gtk;\n");
        assert!(!own.contains("ScreenCast"));
    }

    #[test]
    fn routing_keeps_everything_but_file_choosers() {
        let routed = route(
            "[preferred]\ndefault=gnome;gtk;\norg.freedesktop.impl.portal.FileChooser=gtk;\n",
        );
        assert!(routed.contains("default=gnome;gtk;"));
        assert!(routed.contains("FileChooser=enso"));
        assert!(!routed.contains("FileChooser=gtk"));
        assert_eq!(routed.matches("FileChooser=").count(), 1);
        // No routing at all: GTK for the rest.
        let routed = route("");
        assert!(
            routed.contains(
                "[preferred]\ndefault=gtk;\norg.freedesktop.impl.portal.FileChooser=enso"
            )
        );
    }

    #[test]
    fn enable_then_disable_leaves_nothing() {
        let places = sandbox("cycle");
        let exe = Path::new("/opt/enso/enso-files");
        enable(&places, exe, &["niri".to_owned()]).unwrap();
        assert!(places.portals.join("gtk.portal").is_symlink());
        assert_eq!(
            fs::read_to_string(places.portals.join("enso.portal")).unwrap(),
            PORTAL_FILE
        );
        let niri = fs::read_to_string(places.portals.join("niri-portals.conf")).unwrap();
        assert!(niri.contains("Secret=gnome-keyring"));
        assert!(niri.contains("FileChooser=enso"));
        assert!(places.portals.join("portals.conf").exists());
        let service = fs::read_to_string(&places.service).unwrap();
        assert!(
            fs::read_to_string(&places.file_manager)
                .unwrap()
                .contains("Name=org.freedesktop.FileManager1")
        );
        assert!(service.contains("Exec=/opt/enso/enso-files --service"));
        let dropin = fs::read_to_string(&places.dropin).unwrap();
        assert!(dropin.contains(&format!(
            "XDG_DESKTOP_PORTAL_DIR={}",
            places.portals.display()
        )));

        // Again: the same, not doubled.
        enable(&places, exe, &["niri".to_owned()]).unwrap();
        assert_eq!(fs::read_dir(&places.portals).unwrap().count(), 4);

        disable(&places).unwrap();
        assert!(!places.portals.exists());
        assert!(!places.service.exists());
        assert!(!places.file_manager.exists());
        assert!(!places.dropin.exists());
        // Twice is fine.
        disable(&places).unwrap();
    }

    #[test]
    fn the_users_own_routing_wins() {
        let places = sandbox("user");
        fs::create_dir_all(&places.user_config).unwrap();
        fs::write(
            places.user_config.join("niri-portals.conf"),
            "[preferred]\ndefault=kde;\n",
        )
        .unwrap();
        enable(&places, Path::new("/x"), &["niri".to_owned()]).unwrap();
        let niri = fs::read_to_string(places.portals.join("niri-portals.conf")).unwrap();
        assert!(niri.contains("default=kde;"));
        assert!(!niri.contains("gnome"));
    }
}
