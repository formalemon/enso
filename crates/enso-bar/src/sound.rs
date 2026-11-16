//! The output's volume, for the status island's speaker.
//!
//! A background thread follows the sound server and reports the level
//! whenever it changes. Without a sound server the island simply has no
//! speaker.

use std::{thread, time::Duration};

use enso_sys::audio::Mixer;

/// The output's volume in percent, and whether it is muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub volume: u8,
    pub muted: bool,
}

/// Follow the output's volume on a background thread, calling `on_change`
/// with each new level (`None` while there is no output). Keeps trying if
/// the sound server is not up yet, or goes away; stops when `on_change`
/// returns `false`.
pub fn watch(mut on_change: impl FnMut(Option<Level>) -> bool + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-sound".to_owned())
        .spawn(move || {
            let mut last = None;
            let mut report = |level: Option<Level>| {
                if last == Some(level) {
                    return true;
                }
                last = Some(level);
                on_change(level)
            };
            loop {
                if let Ok(mixer) = Mixer::connect() {
                    let level = || {
                        let output = mixer.state().ok()?.output?;
                        Some(Level {
                            volume: output.volume,
                            muted: output.muted,
                        })
                    };
                    let mut listening = report(level());
                    if listening {
                        // Returns when the listener is gone or the server is.
                        let _ = mixer.watch(|| {
                            listening = report(level());
                            listening
                        });
                    }
                    if !listening {
                        return;
                    }
                }
                if !report(None) {
                    return;
                }
                thread::sleep(Duration::from_secs(5));
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-bar: cannot follow the volume: {err}");
    }
}
