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
//!
//! While the machine is kept awake (`enso_core::Awake`, the control
//! center's switch) being left alone doesn't lock it. If that ends while
//! nobody is there — its time runs out, or it's turned off — the lock comes
//! then, as it would have.

use std::{
    os::unix::process::CommandExt,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use enso_core::{Awake, Paths, awake};
use enso_sys::logind;

use crate::lock::LockConfig;

/// How long the lock screen is given to cover the screens before the
/// machine is let go to sleep.
const COVER: Duration = Duration::from_millis(600);

/// What the watcher knows, shared by its threads.
#[derive(Debug, Default)]
struct Watch {
    /// The lock screen this watcher started is up.
    running: bool,
    /// Nobody has been at the machine for `idle_after`.
    idle: bool,
    /// Bumped each time the machine goes idle, so a wait for keeping awake
    /// to end knows whether it is still the same stretch.
    stretch: u64,
}

type Shared = Arc<Mutex<Watch>>;

/// Run the lock screen, unless the one started earlier is still up. A lock
/// screen started by hand is no trouble either: a second one bows out.
fn lock(shared: &Shared) {
    let Ok(mut watch) = shared.lock() else {
        return;
    };
    if watch.running {
        return;
    }
    match enso_core::exe::again().spawn() {
        Ok(mut child) => {
            watch.running = true;
            // Waited for as soon as it ends, so no finished lock screen
            // lingers as a zombie until the next one.
            let shared = Arc::clone(shared);
            let reaped = thread::Builder::new()
                .name("enso-lock-reap".to_owned())
                .spawn(move || {
                    let _ = child.wait();
                    if let Ok(mut watch) = shared.lock() {
                        watch.running = false;
                    }
                });
            if let Err(err) = reaped {
                eprintln!("enso-lock: cannot wait for the lock screen: {err}");
            }
        }
        Err(err) => eprintln!("enso-lock: cannot start the lock screen: {err}"),
    }
}

/// The machine has been idle long enough: lock it, unless it's being kept
/// awake — then lock when that ends, if nobody has come back by then.
fn idle_lock(shared: &Shared, paths: &Paths) {
    let now = awake::unix_now();
    let awake = Awake::read(paths);
    if !awake.is_on(now) {
        return lock(shared);
    }
    let Some(ends) = awake.ends(now) else {
        // Until turned off: the state directory's watch hears that.
        return;
    };
    let stretch = shared.lock().map_or(0, |watch| watch.stretch);
    let (shared, paths) = (Arc::clone(shared), paths.clone());
    let waited = thread::Builder::new()
        .name("enso-lock-awake".to_owned())
        .spawn(move || {
            thread::sleep(Duration::from_secs(ends.saturating_sub(now)));
            let still = shared
                .lock()
                .is_ok_and(|watch| watch.idle && watch.stretch == stretch);
            if still {
                idle_lock(&shared, &paths);
            }
        });
    if let Err(err) = waited {
        eprintln!("enso-lock: cannot wait for keeping awake to end: {err}");
    }
}

/// What the watcher does, from the `[lock]` settings.
fn watching(config: &LockConfig) -> (u64, bool) {
    (config.idle_after, config.before_sleep)
}

/// Run again in this process's place once the settings it watches by have
/// changed: in the config file, or chosen in the control center. And if
/// keeping awake was turned off while nobody is here, lock now.
fn follow_settings(paths: &Paths, now: (u64, bool), shared: &Shared) {
    let file = paths.config_file();
    let follow = |paths: Paths, shared: Shared| {
        move || {
            let config: LockConfig = enso_core::chosen::section(&paths, "lock");
            if watching(&config) != now {
                let err = enso_core::exe::again().arg("watch").exec();
                eprintln!("enso-lock: cannot start again with the new settings: {err}");
            }
            let idle = shared.lock().is_ok_and(|watch| watch.idle);
            if idle && now.0 > 0 && !Awake::read(&paths).is_on(awake::unix_now()) {
                lock(&shared);
            }
            true
        }
    };
    let state = follow(paths.clone(), Arc::clone(shared));
    if let Err(err) = enso_ui::watch::watch_dir(&paths.state_dir, state) {
        eprintln!("enso-lock: changes to when to lock wait for a restart: {err}");
    }
    if let Some(dir) = file.parent()
        && let Err(err) = enso_ui::watch::watch_dir(dir, follow(paths.clone(), Arc::clone(shared)))
    {
        eprintln!("enso-lock: changes to config.toml wait for a restart: {err}");
    }
}

/// Watch until the session ends.
pub fn run(config: &LockConfig, paths: &Paths) -> Result<(), String> {
    let shared = Shared::default();
    follow_settings(paths, watching(config), &shared);
    if config.idle_after == 0 && !config.before_sleep {
        // Nothing to do until the settings say otherwise.
        loop {
            thread::park();
        }
    }
    if config.before_sleep {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("enso-lock-sleep".to_owned())
            .spawn(move || {
                let result = logind::before_sleep(|| {
                    lock(&shared);
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
    let paths = paths.clone();
    enso_ui::idle::watch_idle(Duration::from_secs(config.idle_after), move |idle| {
        if let Ok(mut watch) = shared.lock() {
            watch.idle = idle;
            watch.stretch += u64::from(idle);
        }
        if idle {
            idle_lock(&shared, &paths);
        }
    })
    .map_err(|err| err.to_string())
}
