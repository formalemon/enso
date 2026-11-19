//! `enso-dusk` — night light: the screens warm at sunset and cool at
//! sunrise, worked out from the timezone; and, if asked, a day theme and a
//! night theme to go with them.
//!
//! Started with the session, it holds every screen's gamma
//! (`wlr-gamma-control`) and changes it slowly — half an hour around
//! sunset and sunrise — and otherwise sleeps until the light next turns.
//! The control center's moon, the launcher's "Night light", and
//! `enso-dusk toggle` turn it on or off by hand until then.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod gamma;
mod plan;
mod state;
mod sun;

use std::{process::ExitCode, time::Duration};

use calloop::{
    EventLoop, LoopHandle, RegistrationToken,
    channel::{self, Event as ChannelEvent},
    timer::{TimeoutAction, Timer},
};
use calloop_wayland_source::WaylandSource;
use enso_core::{Context, Paths, ipc};
use jiff::Timestamp;
use serde::Deserialize;
use smithay_client_toolkit::reexports::{
    calloop, calloop_wayland_source,
    client::{Connection, globals::registry_queue_init, protocol::wl_output::WlOutput},
};

use crate::{
    gamma::Gamma,
    plan::{Choice, Mode},
    state::State,
    sun::Place,
};

const NAME: &str = "enso-dusk";

const HELP: &str = "\
enso-dusk — night light for Niri

Usage: enso-dusk [COMMAND]

With no command, runs: start it with the session.

Commands:
  status            Whether the screens are warm, and today's sunrise and sunset
  on | off          Warm the screens, or don't, until told otherwise
  sun               Follow the sun again: warm from sunset to sunrise
  toggle            The other way from now, until the next sunset or sunrise

Configuration is optional: the [dusk] table of ~/.config/enso/config.toml.
  warmth      = 3500    how warm at night, in kelvin (6500 is no change)
  follow_sun  = true    warm from sunset to sunrise by itself
  fade        = 30      minutes the change takes, centred on sunset
  latitude    = 22.57   where the sun is reckoned for; without them, the
  longitude   = 88.36   place of the timezone (from zone1970.tab)
  day_theme   = \"\"      themes to switch to at sunrise and sunset (both,
  night_theme = \"\"      or neither): rose-pine-dawn and rose-pine-moon, say
";

/// The `[dusk]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DuskConfig {
    pub warmth: u32,
    pub follow_sun: bool,
    pub fade: i64,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub day_theme: String,
    pub night_theme: String,
}

impl Default for DuskConfig {
    fn default() -> Self {
        Self {
            warmth: 3500,
            follow_sun: true,
            fade: 30,
            latitude: None,
            longitude: None,
            day_theme: String::new(),
            night_theme: String::new(),
        }
    }
}

impl DuskConfig {
    fn place(&self) -> Option<Place> {
        match (self.latitude, self.longitude) {
            (Some(latitude), Some(longitude)) => Some(Place {
                latitude,
                longitude,
            }),
            _ => sun::place_of_timezone(),
        }
    }
}

/// The daemon.
#[allow(missing_debug_implementations)]
pub struct Dusk {
    gamma: Gamma,
    config: DuskConfig,
    paths: Paths,
    place: Option<Place>,
    state: State,
    /// How warm the screens are now, gliding to what's wanted.
    shown: f64,
    /// Night, at the last look: a change turns the theme.
    night: Option<bool>,
    handle: Option<LoopHandle<'static, Dusk>>,
    timer: Option<RegistrationToken>,
}

/// While gliding by hand, how often the gamma moves, and how far.
const GLIDE: Duration = Duration::from_millis(50);
const GLIDE_STEP: f64 = 0.06;

impl Dusk {
    /// What the warmth should be now, and when to look again.
    fn wanted(&mut self, now: Timestamp) -> (f64, Duration, Option<bool>) {
        let seconds = now.as_second();
        // A choice by hand that has run out gives way to the sun.
        if self.state.choice.until > 0 && seconds >= self.state.choice.until {
            self.state.choice = Choice::default();
        }
        let sky = self
            .place
            .filter(|_| self.config.follow_sun)
            .map(|place| plan::sky(now, place, self.config.fade));
        let target = match self.state.choice.mode_at(seconds) {
            Mode::On => 1.0,
            Mode::Off => 0.0,
            Mode::Sun => sky.map_or(0.0, |sky| sky.warm),
        };
        // Mid-fade, a look every twenty seconds; otherwise sleep until the
        // next fade begins (and at least look every ten minutes).
        let next = match sky {
            Some(sky) if sky.warm > 0.0 && sky.warm < 1.0 => Duration::from_secs(20),
            Some(Sky {
                turn: Some(turn), ..
            }) => {
                let begins = turn.as_second() - self.config.fade.max(1) * 30;
                Duration::from_secs(u64::try_from(begins - seconds).unwrap_or(1).clamp(1, 600))
            }
            _ => Duration::from_secs(600),
        };
        let next = match self.state.choice.until {
            0 => next,
            until => next.min(Duration::from_secs(
                u64::try_from(until - seconds).unwrap_or(1).max(1),
            )),
        };
        (target, next, sky.map(|sky| sky.night))
    }

    /// Look at the time, move the gamma, and say when to look again.
    fn tick(&mut self) -> Duration {
        let (target, next, night) = self.wanted(Timestamp::now());
        let gliding = (self.shown - target).abs() > GLIDE_STEP;
        self.shown = if gliding {
            self.shown + GLIDE_STEP * (target - self.shown).signum()
        } else {
            target
        };
        self.gamma
            .set(plan::channels(plan::kelvin(self.shown, self.config.warmth)));
        let warm = self.shown > 0.5;
        if warm != self.state.warm {
            self.state.warm = warm;
            self.save();
        }
        if let Some(night) = night {
            self.turn_theme(night);
        }
        if gliding { GLIDE } else { next }
    }

    /// Day or night has come: the theme for it, if there's a pair and the
    /// one in use is one of them (a theme chosen otherwise is left alone).
    fn turn_theme(&mut self, night: bool) {
        if self.night == Some(night) {
            return;
        }
        self.night = Some(night);
        let (day, dark) = (&self.config.day_theme, &self.config.night_theme);
        if day.is_empty() || dark.is_empty() {
            return;
        }
        let wanted = if night { dark } else { day };
        let current = Context::load().theme.id;
        if current == *wanted || (current != *day && current != *dark) {
            return;
        }
        let ctl = enso_core::exe::companion("enso-ctl");
        if let Err(err) = std::process::Command::new(&ctl)
            .args(["theme", "set", wanted])
            .stdout(std::process::Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait())
        {
            eprintln!("enso-dusk: cannot switch the theme: {err}");
        }
    }

    fn save(&self) {
        if let Err(err) = self.state.write(&self.paths) {
            eprintln!("enso-dusk: cannot keep the state: {err}");
        }
    }

    /// A word from outside: on, off, sun, toggle.
    fn command(&mut self, word: &str) {
        let now = Timestamp::now();
        let sky = self
            .place
            .filter(|_| self.config.follow_sun)
            .map(|place| plan::sky(now, place, self.config.fade));
        // By hand while following the sun: until the next turn.
        let until = sky
            .and_then(|sky| sky.turn)
            .map_or(0, |turn| turn.as_second());
        let warm_now = self.shown > 0.5;
        self.state.choice = match word {
            "on" => Choice {
                mode: Mode::On,
                until: 0,
            },
            "off" => Choice {
                mode: Mode::Off,
                until: 0,
            },
            "sun" => Choice::default(),
            "toggle" if warm_now => Choice {
                mode: Mode::Off,
                until,
            },
            "toggle" => Choice {
                mode: Mode::On,
                until,
            },
            _ => return,
        };
        self.save();
        self.reschedule(Duration::ZERO);
    }

    /// Look again in `after`, whatever was planned.
    fn reschedule(&mut self, after: Duration) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        if let Some(token) = self.timer.take() {
            handle.remove(token);
        }
        self.timer = handle
            .insert_source(Timer::from_duration(after), |_, (), dusk| {
                TimeoutAction::ToDuration(dusk.tick())
            })
            .ok();
    }
}

use plan::Sky;

fn run() -> Result<(), String> {
    let _only = match ipc::only(NAME) {
        ipc::Only::Mine(guard) => guard,
        ipc::Only::Taken => return Err("already running".to_owned()),
    };
    let mut ctx = Context::load();
    let config: DuskConfig = ctx.section("dusk");
    ctx.report_warnings(NAME);
    let place = config.place();
    if place.is_none() && config.follow_sun {
        eprintln!("enso-dusk: no place for the sun (set latitude and longitude under [dusk])");
    }

    let conn =
        Connection::connect_to_env().map_err(|err| format!("cannot connect to Wayland: {err}"))?;
    let (globals, queue) =
        registry_queue_init::<Dusk>(&conn).map_err(|err| format!("Wayland registry: {err}"))?;
    let qh = queue.handle();
    let manager = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "the compositor lacks wlr-gamma-control".to_owned())?;
    let mut dusk = Dusk {
        gamma: Gamma::new(manager),
        state: State::read(&ctx.paths),
        paths: ctx.paths.clone(),
        config,
        place,
        shown: 0.0,
        night: None,
        handle: None,
        timer: None,
    };
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            let output: WlOutput =
                globals
                    .registry()
                    .bind(global.name, global.version.min(4), &qh, ());
            dusk.gamma.add(global.name, output, &qh);
        }
    }

    let mut event_loop: EventLoop<'static, Dusk> =
        EventLoop::try_new().map_err(|err| err.to_string())?;
    let handle = event_loop.handle();
    WaylandSource::new(conn, queue)
        .insert(handle.clone())
        .map_err(|err| err.to_string())?;
    let (sender, words) = channel::channel::<String>();
    handle
        .insert_source(words, |event, (), dusk| {
            if let ChannelEvent::Msg(word) = event {
                dusk.command(&word);
            }
        })
        .map_err(|err| err.to_string())?;
    if let Err(err) = ipc::listen(NAME, move |word| sender.send(word.to_owned()).is_ok()) {
        eprintln!("enso-dusk: commands will not reach this one: {err}");
    }
    dusk.handle = Some(handle);
    dusk.reschedule(Duration::ZERO);
    event_loop
        .run(None, &mut dusk, |_| {})
        .map_err(|err| err.to_string())
}

fn status() -> Result<(), String> {
    let ctx = Context::load();
    let config: DuskConfig = enso_core::chosen::section(&ctx.paths, "dusk");
    let state = State::read(&ctx.paths);
    let mode = match state.choice.mode_at(Timestamp::now().as_second()) {
        Mode::Sun => "following the sun",
        Mode::On => "on by hand",
        Mode::Off => "off by hand",
    };
    println!("{} — {mode}", if state.warm { "warm" } else { "as it is" });
    if let Some(place) = config.place() {
        let today = jiff::Zoned::now().date();
        let at = |t: Timestamp| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        };
        match sun::daylight(today, place) {
            sun::Daylight::Between { rise, set } => {
                println!("sunrise {}  sunset {}", at(rise), at(set));
            }
            sun::Daylight::AllDay => println!("the sun doesn't set today"),
            sun::Daylight::AllNight => println!("the sun doesn't rise today"),
        }
    }
    if !ipc::is_listening(NAME) {
        println!("(enso-dusk isn't running)");
    }
    Ok(())
}

/// A command for the running one; with none running, kept for when it starts.
fn tell(word: &str) -> Result<(), String> {
    if ipc::is_listening(NAME) {
        ipc::tell(NAME, word);
        return Ok(());
    }
    let paths = Paths::from_env();
    let mut state = State::read(&paths);
    state.choice = match word {
        "on" => Choice {
            mode: Mode::On,
            until: 0,
        },
        "off" => Choice {
            mode: Mode::Off,
            until: 0,
        },
        "toggle" if state.warm => Choice {
            mode: Mode::Off,
            until: 0,
        },
        "toggle" => Choice {
            mode: Mode::On,
            until: 0,
        },
        _ => Choice::default(),
    };
    state.write(&paths).map_err(|err| err.to_string())?;
    eprintln!("enso-dusk isn't running; it starts that way");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run(),
        ["-h" | "--help"] => {
            print!("{HELP}");
            Ok(())
        }
        ["-V" | "--version"] => {
            println!("enso-dusk {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["status"] => status(),
        [word @ ("on" | "off" | "sun" | "toggle")] => tell(word),
        [other, ..] => {
            eprintln!("enso-dusk: unknown command `{other}`\nTry 'enso-dusk --help'.");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("enso-dusk: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<DuskConfig>("dusk").unwrap(),
            DuskConfig::default()
        );
    }
}
