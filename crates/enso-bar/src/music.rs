//! What the clock knows of music: whether anything is playing, which turns
//! its pill to sea, and a click there playing or pausing it. Both come
//! from `enso-play` — `follow` says `playing` or `idle` as that changes, for
//! as long as the bar keeps its standard input open — so the bar itself
//! speaks no D-Bus. Without enso-play the clock is just a clock.

use std::{
    io::{self, BufRead, BufReader},
    path::PathBuf,
    process::{ChildStdin, Command, Stdio},
    thread,
};

#[derive(Debug)]
pub struct Music {
    program: PathBuf,
    /// Held open while following; the follower ends when it closes.
    follower: Option<ChildStdin>,
    /// The program could not be started; do not keep trying.
    missing: bool,
}

impl Music {
    /// The `enso-play` next to this bar, or else on `PATH`.
    pub fn new() -> Self {
        Self::with_program(enso_core::exe::companion("enso-play"))
    }

    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            follower: None,
            missing: false,
        }
    }

    /// Start hearing whether anything plays: `heard` is called with each
    /// change, and says whether anyone still listens.
    pub fn follow(&mut self, heard: impl Fn(bool) -> bool + Send + 'static) {
        if self.missing || self.follower.is_some() {
            return;
        }
        let spawned = Command::new(&self.program)
            .arg("follow")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => return self.cannot(&err),
        };
        self.follower = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        let reading = thread::Builder::new()
            .name("enso-music".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let playing = match line.as_deref().map(str::trim) {
                        Ok("playing") => true,
                        Ok("idle") => false,
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                    if !heard(playing) {
                        break;
                    }
                }
                // Gone quiet: nothing plays as far as the clock knows.
                heard(false);
                let _ = child.wait();
            });
        if let Err(err) = reading {
            eprintln!("enso-bar: cannot follow what's playing: {err}");
        }
    }

    /// Play if paused, pause if playing: whichever player matters most.
    pub fn play_pause(&mut self) {
        if self.missing {
            return;
        }
        let spawned = Command::new(&self.program)
            .arg("play-pause")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn();
        match spawned {
            // Waited for off to the side, so it's not left as a zombie.
            Ok(mut child) => {
                let _ = thread::Builder::new()
                    .name("enso-play-pause".to_owned())
                    .spawn(move || {
                        let _ = child.wait();
                    });
            }
            Err(err) => self.cannot(&err),
        }
    }

    fn cannot(&mut self, err: &io::Error) {
        self.missing = true;
        if err.kind() == io::ErrorKind::NotFound {
            eprintln!("enso-bar: enso-play is not installed, so the clock is just a clock");
        } else {
            eprintln!("enso-bar: cannot run enso-play: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::*;

    #[test]
    fn a_missing_player_is_tried_once() {
        let mut music = Music::with_program("/nonexistent/enso-play");
        music.follow(|_| true);
        assert!(music.missing);
        music.play_pause();
        music.follow(|_| true);
        assert!(music.follower.is_none());
    }

    #[test]
    fn what_the_follower_says_is_heard_and_its_end_is_quiet() {
        // `sh follow` stands in for a follower that fails at once: its end
        // is still heard, as nothing playing.
        let mut music = Music::with_program("sh");
        let (sent, got) = mpsc::channel();
        music.follow(move |playing| sent.send(playing).is_ok());
        assert_eq!(got.recv_timeout(Duration::from_secs(5)), Ok(false));
    }
}
