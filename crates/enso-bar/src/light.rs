//! The screen's brightness, and the keyboard's light, for the level the
//! island shows as they change.
//!
//! The kernel announces each new screen level on the backlight's
//! `actual_brightness` file, and each change the keyboard's own keys make
//! to its light on `brightness_hw_changed`; a background thread waits on
//! each file and reports — whoever changed it: the keys, the control center,
//! the firmware. Without a backlight (a desktop) there is nothing to follow.

use std::{fs, io, os::unix::fs::FileExt, path::Path, thread};

use enso_sys::power::{Backlight, Sysfs};
use rustix::event::{PollFd, PollFlags, poll};

/// Which light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lamp {
    Screen,
    Keyboard,
}

/// Follow the screen's brightness and the keyboard's light on background
/// threads, calling `on_change` with each new level in percent; each stops
/// when it returns `false`.
pub fn watch(on_change: impl FnMut(Lamp, u8) -> bool + Clone + Send + 'static) {
    for lamp in [Lamp::Screen, Lamp::Keyboard] {
        let mut on_change = on_change.clone();
        let spawned = thread::Builder::new()
            .name("enso-light".to_owned())
            .spawn(move || {
                if let Err(err) = follow(&Sysfs::system(), lamp, |percent| on_change(lamp, percent))
                    && err.kind() != io::ErrorKind::NotFound
                {
                    eprintln!("enso-bar: cannot follow the {lamp:?} light: {err}");
                }
            });
        if let Err(err) = spawned {
            eprintln!("enso-bar: cannot follow the {lamp:?} light: {err}");
        }
    }
}

fn follow(sysfs: &Sysfs, lamp: Lamp, mut on_change: impl FnMut(u8) -> bool) -> io::Result<()> {
    let light = match lamp {
        Lamp::Screen => sysfs.screen_backlight(),
        Lamp::Keyboard => sysfs.keyboard_backlight(),
    }
    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let dir = sysfs.backlight_dir(&light);
    // What announces a change, and where the level is read.
    let (announced, level) = match lamp {
        Lamp::Screen => (dir.join("actual_brightness"), dir.join("actual_brightness")),
        // A keyboard light the keys can't change (no `brightness_hw_changed`)
        // only changes when asked, by the control center.
        Lamp::Keyboard => (dir.join("brightness_hw_changed"), dir.join("brightness")),
    };
    let file = fs::File::open(&announced)?;
    let mut last = None;
    loop {
        // Reading it from the start is what lets the next change wake us.
        // Before the keys have changed the keyboard's light, there is
        // nothing to read: the level is read from its own file anyway.
        let mut buf = [0u8; 32];
        let _ = file.read_at(&mut buf, 0);
        if let Some(level) = read_level(&level) {
            let percent = Backlight {
                brightness: level,
                ..light.clone()
            }
            .percent();
            if last != Some(percent) {
                last = Some(percent);
                if !on_change(percent) {
                    return Ok(());
                }
            }
        }
        let mut fds = [PollFd::new(&file, PollFlags::PRI | PollFlags::ERR)];
        poll(&mut fds, None)?;
    }
}

fn read_level(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}
