//! What the boot menu does, apart from the firmware: which choice is
//! highlighted, how the pictures are opened and put together, and how a
//! systemd-boot entry becomes a kernel command line. Nothing here touches
//! the firmware, so all of it is tested on the build machine.

use alloc::{string::String, vec::Vec};

use enso_scene::life::Rect;

/// What can be started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Arch,
    Windows,
}

/// The choices in the order they are shown.
pub const CHOICES: [Choice; 2] = [Choice::Arch, Choice::Windows];

/// The keys the menu listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Escape,
    Other,
}

/// What a key comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Nothing,
    /// The highlight moved: draw again.
    Moved,
    Start(Choice),
    /// Leave the menu: the firmware goes on to its next boot option.
    Leave,
}

/// The menu: which choice is highlighted. Arch first, and nothing is ever
/// started without a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub chosen: usize,
}

impl Menu {
    pub fn key(&mut self, key: Key) -> Act {
        let count = CHOICES.len();
        match key {
            Key::Up => {
                self.chosen = (self.chosen + count - 1) % count;
                Act::Moved
            }
            Key::Down => {
                self.chosen = (self.chosen + 1) % count;
                Act::Moved
            }
            Key::Enter => Act::Start(CHOICES[self.chosen]),
            Key::Escape => Act::Leave,
            Key::Other => Act::Nothing,
        }
    }
}

/// A picture as the program keeps it: premultiplied B, G, R, A bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// The start of a kept picture: then its width and height (little-endian
/// `u32`s), then its pixels, deflated.
pub const MAGIC: &[u8; 4] = b"ENSO";

/// Open a kept picture.
pub fn open(blob: &[u8]) -> Option<Picture> {
    let (head, body) = blob.split_at_checked(12)?;
    if &head[..4] != MAGIC {
        return None;
    }
    let number = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = head.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(bytes)).ok()
    };
    let (width, height) = (number(4)?, number(8)?);
    let size = width.checked_mul(height)?.checked_mul(4)?;
    let bgra = miniz_oxide::inflate::decompress_to_vec_with_limit(body, size).ok()?;
    (bgra.len() == size).then_some(Picture {
        width,
        height,
        bgra,
    })
}

/// Keep a picture: the other half of [`open`].
#[cfg_attr(target_os = "uefi", allow(dead_code))] // the build script keeps them
pub fn keep(picture: &Picture) -> Vec<u8> {
    let mut blob = Vec::from(&MAGIC[..]);
    for number in [picture.width, picture.height] {
        blob.extend_from_slice(&(number as u32).to_le_bytes());
    }
    blob.extend(miniz_oxide::deflate::compress_to_vec(&picture.bgra, 9));
    blob
}

/// A screen being put together: B, G, R, A bytes, opaque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

impl Screen {
    /// A screen of `width × height` filled with `bgra`.
    pub fn new(width: usize, height: usize, bgra: [u8; 4]) -> Self {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&bgra);
        }
        Self {
            width,
            height,
            bgra: pixels,
        }
    }

    /// Fill the screen with `picture`, scaled to cover it and centred:
    /// pixel for pixel when the sizes match.
    pub fn cover(&mut self, picture: &Picture) {
        if picture.width == 0 || picture.height == 0 {
            return;
        }
        // Source pixels per screen pixel, in 1/65536ths: the smaller, so
        // the picture covers the screen.
        let step = (picture.width << 16)
            .checked_div(self.width)
            .unwrap_or(1 << 16)
            .min(
                (picture.height << 16)
                    .checked_div(self.height)
                    .unwrap_or(1 << 16),
            );
        let left = (picture.width << 16).saturating_sub(step * self.width) / 2;
        let top = (picture.height << 16).saturating_sub(step * self.height) / 2;
        for y in 0..self.height {
            let sy = ((top + y * step) >> 16).min(picture.height - 1);
            for x in 0..self.width {
                let sx = ((left + x * step) >> 16).min(picture.width - 1);
                let from = (sy * picture.width + sx) * 4;
                let to = (y * self.width + x) * 4;
                self.bgra[to..to + 4].copy_from_slice(&picture.bgra[from..from + 4]);
            }
        }
    }

    /// Put `picture` over the screen with its top left at (`x`, `y`),
    /// mixed by its own alpha. What falls outside is left out.
    pub fn over(&mut self, picture: &Picture, x: usize, y: usize) {
        for row in 0..picture.height {
            let sy = y + row;
            if sy >= self.height {
                break;
            }
            for col in 0..picture.width {
                let sx = x + col;
                if sx >= self.width {
                    break;
                }
                let from = (row * picture.width + col) * 4;
                let to = (sy * self.width + sx) * 4;
                let src = &picture.bgra[from..from + 4];
                let keep = 255 - u32::from(src[3]);
                for c in 0..3 {
                    let dst = u32::from(self.bgra[to + c]);
                    self.bgra[to + c] =
                        (u32::from(src[c]) + (dst * keep + 127) / 255).min(255) as u8;
                }
                self.bgra[to + 3] = 255;
            }
        }
    }

    /// Like [`Screen::over`], but only within `rect`.
    pub fn over_part(&mut self, picture: &Picture, x: usize, y: usize, rect: Rect) {
        let (rx, ry) = (rect.x as usize, rect.y as usize);
        let (rx1, ry1) = (rx + rect.w as usize, ry + rect.h as usize);
        let rows = y.max(ry)..(y + picture.height).min(ry1).min(self.height);
        let cols = x.max(rx)..(x + picture.width).min(rx1).min(self.width);
        for sy in rows {
            for sx in cols.clone() {
                let from = ((sy - y) * picture.width + (sx - x)) * 4;
                let to = (sy * self.width + sx) * 4;
                let src = &picture.bgra[from..from + 4];
                let keep = 255 - u32::from(src[3]);
                for c in 0..3 {
                    let dst = u32::from(self.bgra[to + c]);
                    self.bgra[to + c] =
                        (u32::from(src[c]) + (dst * keep + 127) / 255).min(255) as u8;
                }
                self.bgra[to + 3] = 255;
            }
        }
    }

    /// Put `picture` in the middle of the screen, unscaled.
    pub fn centre(&mut self, picture: &Picture) {
        let x = self.width.saturating_sub(picture.width) / 2;
        let y = self.height.saturating_sub(picture.height) / 2;
        self.over(picture, x, y);
    }
}

/// Bytes kept deflated in the program: the pagoda.
#[cfg_attr(target_os = "uefi", allow(dead_code))] // the build script packs them
pub fn pack(bytes: &[u8]) -> Vec<u8> {
    let mut blob = Vec::from(&b"ENSZ"[..]);
    blob.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    blob.extend(miniz_oxide::deflate::compress_to_vec(bytes, 9));
    blob
}

/// The other half of [`pack`].
pub fn unpack(blob: &[u8]) -> Option<Vec<u8>> {
    let (head, body) = blob.split_at_checked(8)?;
    (&head[..4] == b"ENSZ").then_some(())?;
    let size = usize::try_from(u32::from_le_bytes(head[4..8].try_into().ok()?)).ok()?;
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(body, size).ok()?;
    (bytes.len() == size).then_some(bytes)
}

/// Where the menu's card goes on a screen: centred across, its middle five
/// sixths of the way down, below the island, over the sea of cloud.
pub fn card_at(screen: (usize, usize), card: (usize, usize)) -> (usize, usize) {
    let x = screen.0.saturating_sub(card.0) / 2;
    let y = (screen.1 * 5 / 6).saturating_sub(card.1 / 2);
    (x, y.min(screen.1.saturating_sub(card.1)))
}

/// A systemd-boot entry (`/loader/entries/*.conf`): what it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    /// The kernel, as a path on the boot partition (`/vmlinuz-linux`).
    pub linux: String,
    pub initrd: Vec<String>,
    pub options: String,
}

/// Read an entry. `None` if it starts no Linux kernel.
pub fn entry(text: &str) -> Option<Entry> {
    let mut entry = Entry::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(|c: char| c.is_ascii_whitespace())
            .map_or((line, ""), |(key, value)| (key, value.trim()));
        match key {
            "title" => entry.title = value.into(),
            "linux" => entry.linux = value.into(),
            "initrd" => entry
                .initrd
                .extend(value.split_ascii_whitespace().map(String::from)),
            // Several options lines add up, as systemd-boot has it.
            "options" if !value.is_empty() => {
                if !entry.options.is_empty() {
                    entry.options.push(' ');
                }
                entry.options.push_str(value);
            }
            _ => {}
        }
    }
    (!entry.linux.is_empty()).then_some(entry)
}

/// Of the entries (file name, text), the one Arch starts from: the first
/// by name that starts a kernel and isn't a fallback.
pub fn arch_entry<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<Entry> {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .filter(|(name, _)| !name.contains("fallback"))
        .find_map(|(_, text)| entry(text))
}

/// A path on the boot partition as the firmware writes it: `\` between
/// the parts.
pub fn firmware_path(path: &str) -> String {
    let path = path.replace('/', "\\");
    if path.starts_with('\\') {
        path
    } else {
        let mut rooted = String::from("\\");
        rooted.push_str(&path);
        rooted
    }
}

/// The kernel's command line for `entry`: its initrds, which the kernel
/// loads itself from the boot partition, then its options.
pub fn command_line(entry: &Entry) -> String {
    let mut line = String::new();
    for initrd in &entry.initrd {
        line.push_str("initrd=");
        line.push_str(&firmware_path(initrd));
        line.push(' ');
    }
    line.push_str(&entry.options);
    line.trim_end().into()
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn arrows_choose_and_enter_starts() {
        let mut menu = Menu::default();
        assert_eq!(menu.key(Key::Enter), Act::Start(Choice::Arch));
        assert_eq!(menu.key(Key::Down), Act::Moved);
        assert_eq!(menu.key(Key::Enter), Act::Start(Choice::Windows));
        assert_eq!(menu.key(Key::Down), Act::Moved);
        assert_eq!(menu.chosen, 0);
        assert_eq!(menu.key(Key::Up), Act::Moved);
        assert_eq!(menu.chosen, 1);
        assert_eq!(menu.key(Key::Other), Act::Nothing);
        assert_eq!(menu.key(Key::Escape), Act::Leave);
    }

    #[test]
    fn bytes_packed_and_unpacked() {
        let bytes: Vec<u8> = (0..5000u32).map(|n| (n % 7) as u8).collect();
        let blob = pack(&bytes);
        assert!(blob.len() < bytes.len() / 4);
        assert_eq!(unpack(&blob), Some(bytes));
        assert_eq!(unpack(&blob[..6]), None);
        let mut lying = blob;
        lying[4] = 9;
        assert_eq!(unpack(&lying), None);
    }

    #[test]
    fn pictures_kept_and_opened() {
        let picture = Picture {
            width: 3,
            height: 2,
            bgra: (0..24).collect(),
        };
        let blob = keep(&picture);
        assert_eq!(open(&blob), Some(picture));
        assert_eq!(open(&blob[..10]), None);
        let mut wrong = blob.clone();
        wrong[0] = b'X';
        assert_eq!(open(&wrong), None);
        // A size that doesn't match what's inside is refused.
        let mut lying = blob;
        lying[4] = 4;
        assert_eq!(open(&lying), None);
    }

    #[test]
    fn pictures_put_together() {
        let mut screen = Screen::new(4, 2, [10, 10, 10, 255]);
        // A 2 × 1 picture covers a 4 × 2 screen at twice its size.
        let wide = Picture {
            width: 2,
            height: 1,
            bgra: vec![1, 1, 1, 255, 2, 2, 2, 255],
        };
        screen.cover(&wide);
        let blues: Vec<u8> = screen.bgra.chunks(4).map(|px| px[0]).collect();
        assert_eq!(blues, [1, 1, 2, 2, 1, 1, 2, 2]);
        // Half-transparent white over it, premultiplied, off the edge.
        let veil = Picture {
            width: 2,
            height: 1,
            bgra: vec![128, 128, 128, 128, 0, 0, 0, 0],
        };
        screen.over(&veil, 3, 1);
        assert_eq!(&screen.bgra[28..32], &[128 + 1, 128 + 1, 128 + 1, 255]);
        assert_eq!(&screen.bgra[24..28], &[2, 2, 2, 255]);

        assert_eq!(card_at((1920, 1080), (440, 190)), (740, 805));

        // A frame smaller than the screen sits in its middle.
        let mut screen = Screen::new(4, 4, [9, 9, 9, 255]);
        let dot = Picture {
            width: 2,
            height: 2,
            bgra: [200, 200, 200, 255].repeat(4),
        };
        screen.centre(&dot);
        let middle: Vec<u8> = screen.bgra.chunks(4).map(|px| px[0]).collect();
        assert_eq!(
            middle,
            [9, 9, 9, 9, 9, 200, 200, 9, 9, 200, 200, 9, 9, 9, 9, 9]
        );
        assert_eq!(card_at((100, 50), (200, 100)), (0, 0));
    }

    #[test]
    fn a_systemd_boot_entry_becomes_a_command_line() {
        let arch = "\
title   Arch Linux
linux   /vmlinuz-linux
initrd  /intel-ucode.img
initrd  /initramfs-linux.img
options root=UUID=d61b rw nvidia_drm.modeset=1
options quiet
";
        let fallback =
            "title Arch (fallback)\nlinux /vmlinuz-linux\ninitrd /initramfs-linux-fallback.img\n";
        let found = arch_entry([
            ("arch-fallback.conf", fallback),
            (
                "windows.conf",
                "title Windows\nefi /EFI/Microsoft/Boot/bootmgfw.efi\n",
            ),
            ("arch.conf", arch),
        ])
        .unwrap();
        assert_eq!(found.title, "Arch Linux");
        assert_eq!(firmware_path(&found.linux), "\\vmlinuz-linux");
        assert_eq!(
            command_line(&found),
            "initrd=\\intel-ucode.img initrd=\\initramfs-linux.img root=UUID=d61b rw nvidia_drm.modeset=1 quiet"
        );
        assert_eq!(entry("title only\n"), None);
        assert_eq!(firmware_path("EFI/x.efi"), "\\EFI\\x.efi");
    }
}
