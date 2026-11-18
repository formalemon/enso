//! Disks, partitions, and USB sticks, through udisks2.
//!
//! Linux has no drive letters: every filesystem is attached ("mounted") at a
//! folder somewhere in one tree. udisks2 is the system service that knows
//! which filesystems there are, mounts them where the user can reach them
//! (`/run/media/<user>/<label>`), and powers removable disks down safely.
//! It decides who may do what with polkit: a USB stick plugged in at the
//! active session mounts without a question; an internal disk usually needs
//! an administrator's say ([`is_denied`]).

use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use zbus::wire::Variant;

use crate::bus::{self, Objects, Props, Result, Service};

const NAME: &str = "org.freedesktop.UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
/// Where udisks lists its objects.
const MANAGER: &str = "/org/freedesktop/UDisks2";

/// A filesystem that can be shown and mounted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// udisks' object for it, which every operation names.
    pub object: String,
    /// `/dev/sdb1`.
    pub device: PathBuf,
    /// Its own name, if it has one (`STICK`, `Data`).
    pub label: Option<String>,
    /// `ext4`, `vfat`, `ntfs`, …
    pub kind: String,
    /// In bytes.
    pub size: u64,
    /// Where it's mounted, if it is.
    pub mount: Option<PathBuf>,
    /// On a disk meant to come and go: a USB stick, a card.
    pub removable: bool,
    /// Part of the machine (an internal disk): mounting needs permission.
    pub system: bool,
    /// The disk it's on, for ejecting.
    pub drive: Option<String>,
    /// The disk can be ejected or powered off when done.
    pub can_eject: bool,
}

impl Volume {
    /// The name to show: its label, or what udisks suggests, else `None`
    /// (the caller makes one from the size and kind).
    pub fn name(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

/// Mount points that are the system itself, shown elsewhere or not at all.
const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/efi", "/boot/efi", "/usr", "/var", "/home"];

/// The filesystems worth showing: not swap or partition-table bits, not
/// hidden by udev rules, not the system's own mounts.
pub fn volumes() -> Result<Vec<Volume>> {
    let objects = Service::connect(NAME)?.objects_at(MANAGER)?;
    Ok(parse(&objects))
}

fn parse(objects: &Objects) -> Vec<Volume> {
    let drives: HashMap<String, &Props> = objects
        .iter()
        .filter_map(|(path, interfaces)| Some((path.to_string(), interfaces.get(DRIVE)?)))
        .collect();
    let mut volumes: Vec<Volume> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let block = interfaces.get(BLOCK)?;
            let filesystem = interfaces.get(FILESYSTEM)?;
            if bus::string(block, "IdUsage").as_deref() != Some("filesystem")
                || bus::prop(block, "HintIgnore").unwrap_or(false)
            {
                return None;
            }
            let mount = owned::<Vec<Vec<u8>>>(filesystem, "MountPoints")
                .unwrap_or_default()
                .first()
                .map(|bytes| bytes_path(bytes));
            if mount.as_ref().is_some_and(|mount| {
                SYSTEM_MOUNTS
                    .iter()
                    .any(|system| mount == Path::new(system))
            }) {
                return None;
            }
            let drive = bus::path(block, "Drive").filter(|drive| drive != "/");
            let drive_props = drive.as_ref().and_then(|drive| drives.get(drive));
            let flag = |name| {
                drive_props
                    .and_then(|props| bus::prop::<bool>(props, name))
                    .unwrap_or(false)
            };
            let label = bus::string(block, "IdLabel")
                .or_else(|| bus::string(block, "HintName"))
                .filter(|label| !label.is_empty());
            Some(Volume {
                object: path.to_string(),
                device: owned::<Vec<u8>>(block, "Device")
                    .map(|bytes| bytes_path(&bytes))
                    .unwrap_or_default(),
                label,
                kind: bus::string(block, "IdType").unwrap_or_default(),
                size: bus::prop(block, "Size").unwrap_or(0),
                mount,
                removable: flag("Removable") || flag("MediaRemovable"),
                system: bus::prop(block, "HintSystem").unwrap_or(false),
                can_eject: flag("Ejectable") || flag("CanPowerOff"),
                drive,
            })
        })
        .collect();
    // Removable first, then by device: the order disks were found in.
    volumes.sort_by(|a, b| {
        b.removable
            .cmp(&a.removable)
            .then_with(|| a.device.cmp(&b.device))
    });
    volumes
}

/// A property that only converts owned (arrays: paths, mount points).
fn owned<T: TryFrom<zbus::wire::OwnedVariant>>(props: &Props, name: &str) -> Option<T> {
    T::try_from(props.get(name)?.try_clone().ok()?).ok()
}

/// A path from udisks' NUL-terminated byte strings.
fn bytes_path(bytes: &[u8]) -> PathBuf {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    PathBuf::from(OsStr::from_bytes(bytes))
}

fn no_options() -> HashMap<&'static str, Variant<'static>> {
    HashMap::new()
}

/// Mount `volume`; where it went.
pub fn mount(volume: &Volume) -> Result<PathBuf> {
    let path: String =
        Service::connect(NAME)?.call(&volume.object, FILESYSTEM, "Mount", &(no_options(),))?;
    Ok(PathBuf::from(path))
}

pub fn unmount(volume: &Volume) -> Result<()> {
    Service::connect(NAME)?.run(&volume.object, FILESYSTEM, "Unmount", &(no_options(),))
}

/// Make `volume`'s disk safe to pull out: unmount everything on it, then
/// power it off (a USB disk) or eject it (a card, a disc).
pub fn eject(volume: &Volume, all: &[Volume]) -> Result<()> {
    let service = Service::connect(NAME)?;
    for other in all
        .iter()
        .filter(|other| other.drive == volume.drive && other.mount.is_some())
    {
        service.run(&other.object, FILESYSTEM, "Unmount", &(no_options(),))?;
    }
    let Some(drive) = &volume.drive else {
        return Ok(());
    };
    let objects = service.objects_at(MANAGER)?;
    let props = objects
        .iter()
        .find(|(path, _)| path.as_str() == drive)
        .and_then(|(_, interfaces)| interfaces.get(DRIVE));
    let can = |name| {
        props
            .and_then(|props| bus::prop::<bool>(props, name))
            .unwrap_or(false)
    };
    if can("CanPowerOff") {
        service.run(drive, DRIVE, "PowerOff", &(no_options(),))
    } else if can("Ejectable") {
        service.run(drive, DRIVE, "Eject", &(no_options(),))
    } else {
        Ok(())
    }
}

/// Whether `err` is polkit saying no — this user may not do that here
/// without an administrator, and no one was there to ask.
pub fn is_denied(err: &crate::Error) -> bool {
    err.name()
        .is_some_and(|name| name.contains("NotAuthorized"))
}

/// Block, calling `on_change` whenever disks come, go, or are mounted,
/// until it returns `false`.
pub fn watch(on_change: impl FnMut() -> bool) -> Result<()> {
    Service::connect(NAME)?.watch(on_change)
}

#[cfg(test)]
mod tests {
    use zbus::wire::{OwnedObjectPath, OwnedVariant};

    use super::*;

    fn props(pairs: Vec<(&str, Variant<'static>)>) -> Props {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), OwnedVariant::try_from(value).unwrap()))
            .collect()
    }

    fn path(text: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(text.to_owned()).unwrap()
    }

    fn block(
        device: &str,
        label: &str,
        usage: &str,
        ignore: bool,
        system: bool,
        drive: &str,
        mount: Option<&str>,
    ) -> HashMap<String, Props> {
        let mut interfaces = HashMap::new();
        interfaces.insert(
            BLOCK.to_owned(),
            props(vec![
                ("Device", Variant::from(format!("{device}\0").into_bytes())),
                ("IdLabel", Variant::from(label.to_owned())),
                ("IdUsage", Variant::from(usage.to_owned())),
                ("IdType", Variant::from("ext4".to_owned())),
                ("HintIgnore", Variant::from(ignore)),
                ("HintSystem", Variant::from(system)),
                ("Size", Variant::from(64_000_000_000u64)),
                (
                    "Drive",
                    Variant::from(zbus::wire::ObjectPath::try_from(drive.to_owned()).unwrap()),
                ),
            ]),
        );
        let mounts: Vec<Vec<u8>> = mount
            .map(|m| format!("{m}\0").into_bytes())
            .into_iter()
            .collect();
        interfaces.insert(
            FILESYSTEM.to_owned(),
            props(vec![("MountPoints", Variant::from(mounts))]),
        );
        interfaces
    }

    #[test]
    fn what_is_shown_and_in_what_order() {
        let mut objects: Objects = HashMap::new();
        let stick = "/org/freedesktop/UDisks2/drives/Stick";
        let internal = "/org/freedesktop/UDisks2/drives/Nvme";
        objects.insert(path(stick), {
            let mut drive = HashMap::new();
            drive.insert(
                DRIVE.to_owned(),
                props(vec![
                    ("Removable", Variant::from(true)),
                    ("CanPowerOff", Variant::from(true)),
                ]),
            );
            drive
        });
        let root = "/org/freedesktop/UDisks2/block_devices/";
        objects.insert(
            path(&format!("{root}nvme0n1p2")),
            block(
                "/dev/nvme0n1p2",
                "",
                "filesystem",
                false,
                true,
                internal,
                Some("/"),
            ),
        );
        objects.insert(
            path(&format!("{root}nvme1n1p1")),
            block(
                "/dev/nvme1n1p1",
                "",
                "filesystem",
                true,
                true,
                internal,
                None,
            ),
        );
        objects.insert(
            path(&format!("{root}nvme1n1p3")),
            block(
                "/dev/nvme1n1p3",
                "",
                "filesystem",
                false,
                true,
                internal,
                None,
            ),
        );
        objects.insert(
            path(&format!("{root}sdb1")),
            block(
                "/dev/sdb1",
                "STICK",
                "filesystem",
                false,
                false,
                stick,
                Some("/run/media/me/STICK"),
            ),
        );
        objects.insert(
            path(&format!("{root}nvme0n1p3")),
            block("/dev/nvme0n1p3", "", "other", false, true, internal, None),
        );
        let volumes = parse(&objects);
        let devices: Vec<&Path> = volumes
            .iter()
            .map(|volume| volume.device.as_path())
            .collect();
        // The stick first; the root and hidden and swap left out.
        assert_eq!(
            devices,
            [Path::new("/dev/sdb1"), Path::new("/dev/nvme1n1p3")]
        );
        assert_eq!(volumes[0].name(), Some("STICK"));
        assert_eq!(
            volumes[0].mount.as_deref(),
            Some(Path::new("/run/media/me/STICK"))
        );
        assert!(volumes[0].removable && volumes[0].can_eject);
        assert!(volumes[1].system && !volumes[1].removable);
        assert_eq!(volumes[1].name(), None);
        assert_eq!(volumes[1].size, 64_000_000_000);
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p enso-sys --features udisks -- --ignored live`: what
    /// this machine's udisks reports.
    #[test]
    #[ignore = "talks to the real system bus"]
    fn live_volumes() {
        for volume in super::volumes().unwrap() {
            eprintln!("{volume:?}");
        }
    }
}
