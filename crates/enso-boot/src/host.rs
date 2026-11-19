//! `enso-boot` on Linux: the installer. It changes nothing itself (the
//! boot partition and the firmware's boot entries are root's): it finds
//! the menu built for the firmware and the boot partition, and prints the
//! lines to run.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

const HELP: &str = "\
enso-boot — the boot menu, for the firmware

Usage: enso-boot install     say how to put the menu in place
       enso-boot remove      say how to take it away again

Options:
  -h, --help       Print this help
  -V, --version    Print the version

The menu itself is built for the firmware, from a checkout:

  cargo build --release -p enso-boot --target x86_64-unknown-uefi

It shows Arch and Windows over the terraces; nothing starts until Enter.
Arch starts as systemd-boot's entry for it says (/loader/entries), so
kernel options are changed there, for both. Esc in the menu, or anything
going wrong, hands over to the firmware's next boot option: systemd-boot,
which stays installed.
";

/// Where the menu goes on the boot partition.
const ON_BOOT: &str = "EFI/enso/enso-boot.efi";

pub fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let word = args.next();
    if args.next().is_some() {
        eprintln!("enso-boot: one command at a time\nTry 'enso-boot --help'.");
        return ExitCode::from(2);
    }
    match word.as_deref() {
        Some("install") => install(),
        Some("remove") => {
            print!("{}", remove_steps());
            ExitCode::SUCCESS
        }
        Some("-h" | "--help") | None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("enso-boot {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("enso-boot: unknown command {other:?}\nTry 'enso-boot --help'.");
            ExitCode::from(2)
        }
    }
}

/// The menu built for the firmware: beside this program, or in the
/// checkout it was built in.
fn built() -> Option<PathBuf> {
    let exe = env::current_exe().and_then(fs::canonicalize).ok()?;
    let dir = exe.parent()?;
    // Beside it, as a release has it; or, in a checkout,
    // target/release/enso-boot → target/x86_64-unknown-uefi/release/.
    [
        dir.join("enso-boot.efi"),
        dir.parent()?
            .join("x86_64-unknown-uefi/release/enso-boot.efi"),
    ]
    .into_iter()
    .find(|efi| efi.is_file())
}

/// The boot partition: the device mounted at `/boot` (or `/efi`), its
/// disk, and its number on that disk.
#[derive(Debug, PartialEq, Eq)]
struct Partition {
    mount: String,
    disk: String,
    number: u32,
}

fn boot_partition(mounts: &str, sys_block: &Path) -> Option<Partition> {
    let (device, mount) = mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (device, mount, kind) = (fields.next()?, fields.next()?, fields.next()?);
        (matches!(mount, "/boot" | "/efi" | "/boot/efi") && kind == "vfat")
            .then(|| (device.to_owned(), mount.to_owned()))
    })?;
    let name = Path::new(&device).file_name()?.to_str()?;
    // /sys/class/block/<partition> is a link into its disk's directory.
    let link = fs::canonicalize(sys_block.join(name)).ok()?;
    let disk = link.parent()?.file_name()?.to_str()?.to_owned();
    let number = fs::read_to_string(link.join("partition"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Partition {
        mount,
        disk: format!("/dev/{disk}"),
        number,
    })
}

fn install() -> ExitCode {
    let Some(efi) = built() else {
        eprintln!(
            "enso-boot: the menu isn't built yet. From the checkout:\n\n  \
             rustup target add x86_64-unknown-uefi\n  \
             cargo build --release -p enso-boot --target x86_64-unknown-uefi\n"
        );
        return ExitCode::FAILURE;
    };
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let Some(boot) = boot_partition(&mounts, Path::new("/sys/class/block")) else {
        eprintln!("enso-boot: no boot partition (vfat) mounted at /boot or /efi.");
        return ExitCode::FAILURE;
    };
    print!("{}", install_steps(&efi, &boot));
    ExitCode::SUCCESS
}

fn install_steps(efi: &Path, boot: &Partition) -> String {
    let loader = format!("\\{}", ON_BOOT.replace('/', "\\"));
    format!(
        "The menu is built: {efi}\n\n\
         1. Put it on the boot partition, as root:\n\n\
         \x20  sudo install -Dm644 {efi} {mount}/{ON_BOOT}\n\n\
         2. Try it from systemd-boot's menu first, changing nothing else:\n\n\
         \x20  printf 'title enso\\nefi /{ON_BOOT}\\n' | sudo tee {mount}/loader/entries/enso.conf\n\n\
         \x20  Restart and choose \"enso\". Esc in the menu goes back to systemd-boot's.\n\n\
         3. Then make it what the machine starts, one of two ways:\n\n\
         \x20  a. systemd-boot starts it at once, its own menu hidden (nothing to\n\
         \x20     install). In {mount}/loader/loader.conf (sudoedit):\n\n\
         \x20       default enso.conf\n\
         \x20       timeout 0\n\n\
         \x20     Holding Space as the machine starts shows systemd-boot's menu.\n\n\
         \x20  b. A firmware boot entry of its own, first in the order (takes\n\
         \x20     efibootmgr: pacman -S efibootmgr):\n\n\
         \x20       sudo efibootmgr --create --disk {disk} --part {part} \\\n\
         \x20           --loader '{loader}' --label 'enso'\n\n\
         \x20     systemd-boot stays next in the order: Esc in the menu, or anything\n\
         \x20     going wrong, goes on to it.\n\n\
         Either way Esc at the maker's logo still opens the firmware's boot menu.\n\
         After rebuilding, only step 1 again.\n",
        efi = efi.display(),
        mount = boot.mount,
        disk = boot.disk,
        part = boot.number,
    )
}

fn remove_steps() -> String {
    format!(
        "To take the menu away, as root, undo whichever was done:\n\n\
         \x20 sudoedit /boot/loader/loader.conf     # default and timeout as they were\n\
         \x20 sudo efibootmgr                       # an entry labelled enso? Boot000N:\n\
         \x20 sudo efibootmgr --delete-bootnum --bootnum 000N\n\
         \x20 sudo rm /boot/loader/entries/enso.conf /boot/{ON_BOOT}\n\n\
         The firmware then starts systemd-boot, as before.\n"
    )
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use enso_ui::TextEngine;

    use super::*;
    use crate::{
        art,
        menu::{self, Picture, Screen},
    };

    #[test]
    fn finds_the_boot_partition() {
        let root = env::temp_dir().join(format!("enso-boot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let disk = root.join("devices/nvme0n1");
        fs::create_dir_all(disk.join("nvme0n1p1")).unwrap();
        fs::write(disk.join("nvme0n1p1/partition"), "1\n").unwrap();
        let block = root.join("class/block");
        fs::create_dir_all(&block).unwrap();
        std::os::unix::fs::symlink(disk.join("nvme0n1p1"), block.join("nvme0n1p1")).unwrap();
        let mounts = "/dev/nvme0n1p2 / ext4 rw 0 0\n/dev/nvme0n1p1 /boot vfat rw 0 0\n";
        let found = boot_partition(mounts, &block).unwrap();
        assert_eq!(
            found,
            Partition {
                mount: "/boot".to_owned(),
                disk: "/dev/nvme0n1".to_owned(),
                number: 1
            }
        );
        let steps = install_steps(Path::new("/b/enso-boot.efi"), &found);
        assert!(steps.contains("--disk /dev/nvme0n1 --part 1"));
        assert!(steps.contains("'\\EFI\\enso\\enso-boot.efi'"));
        assert!(steps.contains("/boot/EFI/enso/enso-boot.efi"));
        assert!(steps.contains("/boot/loader/entries/enso.conf"));
        assert!(boot_partition("/dev/sda1 /boot ext4 rw 0 0\n", &block).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    fn picture(width: u32, height: u32, mut rgba: Vec<u8>) -> Picture {
        art::to_bgra(&mut rgba);
        Picture {
            width: width as usize,
            height: height as usize,
            bgra: rgba,
        }
    }

    fn save(name: &str, screen: &Screen) {
        let mut rgba = screen.bgra.clone();
        art::to_bgra(&mut rgba);
        let frame = enso_ui::Offscreen {
            width: screen.width as u32,
            height: screen.height as u32,
            rgba,
        };
        frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
    }

    /// The menu as the firmware puts it together, at a quarter of the
    /// pixels, and the frame Arch starts on — the login screen's first.
    #[test]
    fn the_menu_put_together() {
        let ctx = art::context(None);
        let mut text = TextEngine::new(&ctx.config.appearance);
        let (width, height) = (art::SCREEN.0 / 2, art::SCREEN.1 / 2);
        let rendered = art::scene(&ctx, (width, height));
        let scene = picture(width, height, rendered.rgba().to_vec());
        for chosen in 0..art::LABELS.len() {
            let card = art::card(&ctx, &mut text, chosen);
            let card = picture(card.width, card.height, card.rgba);
            // Kept and opened as the program does.
            let card = menu::open(&menu::keep(&card)).unwrap();
            let mut screen = Screen::new(width as usize, height as usize, [0, 0, 0, 255]);
            screen.cover(&scene);
            let (x, y) = menu::card_at((screen.width, screen.height), (card.width, card.height));
            screen.over(&card, x, y);
            save(&format!("boot-menu-{chosen}"), &screen);
        }

        let frame = art::frame(&ctx, &mut text, &rendered);
        let colors = ctx.theme.colors.clone();
        let backdrop = enso_ui::mark::backdrop(rendered.clone(), colors.base);
        let greeter = enso_ui::Offscreen::render(&mut text, (width, height), 1, |canvas| {
            canvas.clear(colors.base);
            canvas.picture(&backdrop);
            let (centre, radius) = enso_ui::mark::place(canvas.bounds());
            enso_ui::mark::Mark::new(colors.rose, colors.text).draw(
                canvas,
                centre,
                radius,
                Instant::now(),
            );
        });
        assert!(frame.rgba == greeter.rgba);
    }
}
