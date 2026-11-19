//! `enso-lock watch` — locking without being asked.
//!
//! A small program that sits with the session and runs the lock screen at
//! the two moments nobody presses the key: when the machine has been left
//! alone for a while, and just before it sleeps, so that it never wakes
//! unlocked. It draws nothing and wakes for nothing else.
//!
//! Idleness is the compositor's to know (`ext-idle-notify`, which does not
//! count time a film or a call keeps the screen awake); sleep is logind's
//! to announce, and it waits for the lock to be up before going.
//!
//! When the control center changes when to lock, the watcher starts itself
//! afresh in its own place (the same process, run again), so the new time
//! counts at once.

use std::{
    os::unix::process::CommandExt,
    process::Child,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use enso_core::{Context, Paths};
use enso_sys::logind;

use crate::lock::LockConfig;

/// How long the lock screen is given to cover the screens before the
/// machine is let go to sleep.
const COVER: Duration = Duration::from_millis(600);

/// The lock screen this watcher started, while it runs.
type Running = Arc<Mutex<Option<Child>>>;

/// Run the lock screen, unless the one started earlier is still up. A lock
/// screen started by hand is no trouble either: a second one bows out.
fn lock(running: &Running) {
    let Ok(mut running) = running.lock() else {
        return;
    };
    if let Some(child) = running.as_mut()
        && matches!(child.try_wait(), Ok(None))
    {
        return;
    }
    match enso_core::exe::again().spawn() {
        Ok(child) => *running = Some(child),
        Err(err) => eprintln!("enso-lock: cannot start the lock screen: {err}"),
    }
}

/// What the watcher does, from the `[lock]` settings.
fn watching(config: &LockConfig) -> (u64, bool) {
    (config.idle_after, config.before_sleep)
}

/// Run again in this process's place once the settings it watches by have
/// changed: in the config file, or chosen in the control center.
fn follow_settings(paths: &Paths, now: (u64, bool)) {
    let file = paths.config_file();
    let follow = |paths: Paths| {
        move || {
            let mut ctx = Context::load_from(paths.clone());
            let config: LockConfig = ctx.section("lock");
            if watching(&config) != now {
                let err = enso_core::exe::again().arg("watch").exec();
                eprintln!("enso-lock: cannot start again with the new settings: {err}");
            }
            true
        }
    };
    if let Err(err) = enso_ui::watch::watch_dir(&paths.state_dir, follow(paths.clone())) {
        eprintln!("enso-lock: changes to when to lock wait for a restart: {err}");
    }
    if let Some(dir) = file.parent()
        && let Err(err) = enso_ui::watch::watch_dir(dir, follow(paths.clone()))
    {
        eprintln!("enso-lock: changes to config.toml wait for a restart: {err}");
    }
}

/// Watch until the session ends.
pub fn run(config: &LockConfig, paths: &Paths) -> Result<(), String> {
    follow_settings(paths, watching(config));
    if config.idle_after == 0 && !config.before_sleep {
        // Nothing to do until the settings say otherwise.
        loop {
            thread::park();
        }
    }
    let running: Running = Arc::default();

    if config.before_sleep {
        let running = Arc::clone(&running);
        thread::Builder::new()
            .name("enso-lock-sleep".to_owned())
            .spawn(move || {
                let result = logind::before_sleep(|| {
                    lock(&running);
                    thread::sleep(COVER);
                });
                if let Err(err) = result {
                    eprintln!("enso-lock: not locking before sleep: {err}");
                }
            })
            .map_err(|err| err.to_string())?;
    }

    if config.idle_after == 0 {
        // Only sleep to wait for: the thread above does all there is to do.
        loop {
            thread::park();
        }
    }
    enso_ui::idle::watch_idle(Duration::from_secs(config.idle_after), move || {
        lock(&running)
    })
    .map_err(|err| err.to_string())
}
