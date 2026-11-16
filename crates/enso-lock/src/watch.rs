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

use std::{
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

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
    let program = std::env::current_exe().unwrap_or_else(|_| "enso-lock".into());
    match Command::new(program).spawn() {
        Ok(child) => *running = Some(child),
        Err(err) => eprintln!("enso-lock: cannot start the lock screen: {err}"),
    }
}

/// Watch until the session ends.
pub fn run(config: &LockConfig) -> Result<(), String> {
    if config.idle_after == 0 && !config.before_sleep {
        return Err("nothing to watch for: idle_after is 0 and before_sleep is false".to_owned());
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
