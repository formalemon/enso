//! Whether the machine is online, and how — read from `/sys` and `/proc`,
//! for an indicator that must cost nothing. Managing connections is the
//! `wifi` module's business.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// How the machine is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// A cable.
    Wired,
    /// Wi-Fi, with its signal as bars, `0..=4`.
    Wifi { bars: u8 },
    /// There is a wireless interface, but it is not connected.
    Offline,
}

/// The connection worth showing: a cable if one carries traffic, else Wi-Fi,
/// else — on a machine with a wireless interface — the fact of being
/// offline. `None` if there is no network hardware to speak of.
pub fn link() -> Option<Link> {
    link_in(Path::new("/sys"), Path::new("/proc"))
}

fn link_in(sys: &Path, proc: &Path) -> Option<Link> {
    let mut interfaces: Vec<PathBuf> = fs::read_dir(sys.join("class/net"))
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    interfaces.sort();

    let read = |path: PathBuf| fs::read_to_string(path).ok().map(|s| s.trim().to_owned());
    let up = |dir: &Path| read(dir.join("operstate")).as_deref() == Some("up");
    let wireless = |dir: &Path| dir.join("wireless").exists() || dir.join("phy80211").exists();
    // ARPHRD_ETHER, on a real device: bridges and tunnels have no `device`.
    let wired = |dir: &Path| {
        !wireless(dir)
            && read(dir.join("type")).as_deref() == Some("1")
            && dir.join("device").exists()
    };

    if interfaces.iter().any(|dir| wired(dir) && up(dir)) {
        return Some(Link::Wired);
    }
    let radio = interfaces.iter().find(|dir| wireless(dir))?;
    if !up(radio) {
        return Some(Link::Offline);
    }
    let name = radio.file_name()?.to_string_lossy().into_owned();
    let dbm = fs::read_to_string(proc.join("net/wireless"))
        .ok()
        .and_then(|table| signal_dbm(&table, &name));
    Some(Link::Wifi {
        bars: dbm.map_or(4, bars),
    })
}

/// The signal level of `interface` in `/proc/net/wireless`, in dBm.
fn signal_dbm(table: &str, interface: &str) -> Option<i16> {
    table.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        if name.trim() != interface {
            return None;
        }
        // Columns: status, link quality, signal level, noise, …
        let level = rest.split_whitespace().nth(2)?;
        level
            .trim_end_matches('.')
            .parse::<f32>()
            .ok()
            .map(|dbm| dbm as i16)
    })
}

/// Signal strength as bars, on the same scale as `wifi::Network::bars`.
fn bars(dbm: i16) -> u8 {
    match dbm {
        -55.. => 4,
        -66.. => 3,
        -77.. => 2,
        -88.. => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRELESS: &str = "\
Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
 wlan0: 0000   49.  -61.  -256        0      0      0      1      0        0
";

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("enso-net-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, contents) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        root
    }

    fn link_of(root: &Path) -> Option<Link> {
        link_in(&root.join("sys"), &root.join("proc"))
    }

    #[test]
    fn reads_the_signal_table() {
        assert_eq!(signal_dbm(WIRELESS, "wlan0"), Some(-61));
        assert_eq!(signal_dbm(WIRELESS, "wlan1"), None);
        assert_eq!([-50, -61, -70, -80, -95].map(bars), [4, 3, 2, 1, 0]);
    }

    #[test]
    fn picks_the_connection_worth_showing() {
        let laptop = [
            ("sys/class/net/lo/operstate", "unknown\n"),
            ("sys/class/net/lo/type", "772\n"),
            ("sys/class/net/enp3s0/operstate", "down\n"),
            ("sys/class/net/enp3s0/type", "1\n"),
            ("sys/class/net/enp3s0/device/vendor", "0x10ec\n"),
            ("sys/class/net/wlan0/operstate", "up\n"),
            ("sys/class/net/wlan0/type", "1\n"),
            ("sys/class/net/wlan0/wireless/placeholder", ""),
            ("proc/net/wireless", WIRELESS),
        ];
        let root = tree("wifi", &laptop);
        assert_eq!(link_of(&root), Some(Link::Wifi { bars: 3 }));

        // Radio down: offline, but worth saying so.
        fs::write(root.join("sys/class/net/wlan0/operstate"), "down\n").unwrap();
        assert_eq!(link_of(&root), Some(Link::Offline));

        // A cable beats everything.
        fs::write(root.join("sys/class/net/enp3s0/operstate"), "up\n").unwrap();
        assert_eq!(link_of(&root), Some(Link::Wired));
        fs::remove_dir_all(root).unwrap();

        // A desktop with its cable out and no radio has nothing to show;
        // neither does a machine we cannot read.
        let root = tree(
            "none",
            &[
                ("sys/class/net/eno1/operstate", "down\n"),
                ("sys/class/net/eno1/type", "1\n"),
                ("sys/class/net/eno1/device/vendor", "0x8086\n"),
            ],
        );
        assert_eq!(link_of(&root), None);
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            link_in(Path::new("/nonexistent"), Path::new("/nonexistent")),
            None
        );
    }
}
