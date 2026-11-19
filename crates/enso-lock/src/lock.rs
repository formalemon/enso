//! The lock screen: a clock, a password field, and as little else as a
//! locked machine needs.
//!
//! Everything here waits. Nothing is drawn unless a key was pressed or the
//! minute turned; once the screens have gone dark not even the minute is
//! followed, and the process sleeps on its socket until someone comes back.

use std::{
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Transition, Tween,
    style::{icon, opacity, space, type_scale},
    wallpaper::{self, Chosen, Fit, WallConfig},
};
use enso_sys::{
    auth,
    logind::{self, Machine},
};
use enso_ui::{
    Align, Canvas, Cx, Event, KeyEvent, Paint, Picture, Proxy, Rect, Role, Screen, Screens, Text,
    mark::{Mark, Pace},
    widget::{Look, Note, SecretField, secret},
};
use niri_ipc::{Action, Request, socket::Socket};
use serde::Deserialize;
use zeroize::Zeroizing;

/// The `[lock]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LockConfig {
    /// strftime format of the large clock.
    pub clock: String,
    /// strftime format of the line under it; empty for none.
    pub date: String,
    /// Show the theme's wallpaper, blurred, behind the clock. `false` is the
    /// theme's base colour alone.
    pub wallpaper: bool,
    /// Seconds without a key or the pointer before the screens are powered
    /// off. `0` leaves them on.
    pub screen_off_after: u64,
    /// Seconds without a key or the pointer before the machine is suspended.
    /// `0` never suspends.
    pub suspend_after: u64,
    /// Hold the machine in its low-power profile while it is locked.
    pub power_saver: bool,
    /// For `enso-lock watch`: seconds without a key or the pointer before
    /// the screen is locked. `0` never locks on idle.
    pub idle_after: u64,
    /// For `enso-lock watch`: lock just before the machine sleeps.
    pub before_sleep: bool,
}

use enso_core::chosen;

pub const DEFAULT_CLOCK: &str = "%H:%M";

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            clock: DEFAULT_CLOCK.to_owned(),
            date: "%A, %-d %B".to_owned(),
            wallpaper: true,
            screen_off_after: chosen::lock::SCREEN_OFF_AFTER,
            suspend_after: chosen::lock::SUSPEND_AFTER,
            power_saver: true,
            idle_after: chosen::lock::IDLE_AFTER,
            before_sleep: chosen::lock::BEFORE_SLEEP,
        }
    }
}

const FIELD_WIDTH: f32 = 280.0;
const FIELD_HEIGHT: f32 = 48.0;
/// Characters accepted; PAM takes no more than this many bytes anyway.
const MAX_PASSWORD: usize = 256;
/// Longest side of the wallpaper as it is kept for the backdrop. It is
/// blurred beyond recognising detail, so this is all there is to keep.
const BACKDROP_SIDE: u32 = 256;
/// Wrong attempts allowed before each further one costs a wait.
const FREE_ATTEMPTS: u32 = 3;
/// The ensō's ring, as a share of the screen's shorter side.
const MARK_SIZE: f32 = 0.075;
/// How long "Wrong password" stays up.
const NOTICE: Duration = Duration::from_secs(3);

/// Messages from the background threads.
#[derive(Debug)]
pub enum Message {
    /// The answer to a password check.
    Verdict(Result<bool, String>),
    /// The wallpaper, small and blurred, ready to put behind the clock.
    Backdrop(Picture),
}

/// Where the lock screen is in its one conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for a password.
    Asking,
    /// A password is being checked.
    Checking,
    /// The password was right; fading out before letting go.
    Leaving,
}

/// What handling an input comes to.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ignored,
    Redraw,
    /// Check this password.
    Submit(Zeroizing<String>),
}

/// What has come due on the lock screen's own clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Due {
    redraw: bool,
    /// The fade after the right password has finished: let the session go.
    unlock: bool,
    /// Nobody has been here for a while: power the screens off.
    screens_off: bool,
    /// Nobody has been here for a long while: suspend the machine.
    suspend: bool,
}

/// How a password is checked; tests put a known answer here.
type Verify = fn(&[u8]) -> Result<bool, auth::Error>;

#[derive(Debug)]
pub struct Lock {
    ctx: Context,
    config: LockConfig,
    verify: Verify,
    proxy: Option<Proxy<Message>>,
    phase: Phase,
    field: SecretField,
    /// Wrong attempts in a row.
    failures: u32,
    /// No attempt is taken before this.
    locked_out_until: Option<Instant>,
    /// What went wrong, and until when to say so.
    notice: Option<(String, Instant)>,
    time: String,
    date: String,
    /// When the minute turns next.
    clock_due: Instant,
    last_input: Instant,
    /// The screens were powered off for lack of input.
    asleep: bool,
    /// Suspend was asked for since the last input.
    suspended: bool,
    /// How present the clock and field are: `0` hidden, `1` shown.
    presence: Tween<f32>,
    revealed: bool,
    /// The blurred wallpaper, small; and how far it has faded in.
    backdrop: Option<Picture>,
    backdrop_shown: Tween<f32>,
    /// The backdrop fitted to each screen size in use.
    fitted: Vec<Picture>,
    /// The ensō above the clock: painted in as the lock appears.
    mark: Mark,
}

impl Lock {
    pub fn new(ctx: Context, config: LockConfig) -> Self {
        Self::with_verify(ctx, config, auth::verify)
    }

    fn with_verify(ctx: Context, config: LockConfig, verify: Verify) -> Self {
        let now = Instant::now();
        let mark = Mark::new(ctx.theme.colors.rose, ctx.theme.colors.text);
        let mut lock = Self {
            ctx,
            config,
            verify,
            proxy: None,
            phase: Phase::Asking,
            field: SecretField::new(MAX_PASSWORD),
            failures: 0,
            locked_out_until: None,
            notice: None,
            time: String::new(),
            date: String::new(),
            clock_due: now,
            last_input: now,
            asleep: false,
            suspended: false,
            presence: Tween::new(0.0),
            revealed: false,
            backdrop: None,
            backdrop_shown: Tween::new(0.0),
            fitted: Vec::new(),
            mark,
        };
        lock.refresh_clock();
        lock
    }

    /// Re-read the clock, and note when the minute turns next.
    fn refresh_clock(&mut self) {
        let now = jiff::Zoned::now();
        let format = |pattern: &str| jiff::fmt::strtime::format(pattern, &now);
        self.time = format(&self.config.clock)
            .or_else(|_| format(DEFAULT_CLOCK))
            .unwrap_or_default();
        self.date = format(&self.config.date).unwrap_or_default();
        let into_minute = Duration::new(
            now.second().unsigned_abs().into(),
            now.subsec_nanosecond().unsigned_abs(),
        );
        let next_minute =
            Duration::from_secs(60).saturating_sub(into_minute) + Duration::from_millis(20);
        self.clock_due = Instant::now() + next_minute;
    }

    fn locked_out(&self, now: Instant) -> Option<Duration> {
        self.locked_out_until
            .map(|until| until.saturating_duration_since(now))
            .filter(|left| !left.is_zero())
    }

    /// The line under the field, and whether it is a complaint.
    fn caption(&self, now: Instant) -> Option<(String, bool)> {
        if let Some(left) = self.locked_out(now) {
            let seconds = left.as_secs_f32().ceil() as u64;
            return Some((format!("Try again in {seconds} s"), true));
        }
        if let Some((notice, until)) = &self.notice
            && now < *until
        {
            return Some((notice.clone(), true));
        }
        match self.phase {
            Phase::Checking => Some(("Checking…".to_owned(), false)),
            _ => None,
        }
    }

    /// Forget what was typed, overwriting it first.
    fn clear_password(&mut self, now: Instant) {
        self.field.clear(&self.ctx.motion, now);
    }

    /// Someone is here: note it, and if the screens were off, they are
    /// coming back on (the compositor does that on any input) with a clock
    /// that has not been followed since. Returns whether to redraw.
    fn stir(&mut self, now: Instant) -> bool {
        self.last_input = now;
        self.suspended = false;
        if !std::mem::take(&mut self.asleep) {
            return false;
        }
        self.refresh_clock();
        self.mark.show(&self.ctx.motion, now);
        true
    }

    fn key(&mut self, key: KeyEvent, now: Instant) -> Outcome {
        let woke = self.stir(now);
        let caps_changed = self.field.note_mods(key.mods);
        let quiet = if woke || caps_changed {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        if self.phase != Phase::Asking || self.locked_out(now).is_some() {
            return quiet;
        }
        match self.field.key(&key, &self.ctx.motion, now) {
            secret::Response::Submit if self.field.is_empty() => quiet,
            secret::Response::Submit => {
                self.phase = Phase::Checking;
                self.notice = None;
                Outcome::Submit(self.field.take(&self.ctx.motion, now))
            }
            // Nothing to cancel on a lock screen: Esc starts over.
            secret::Response::Cancel => {
                self.clear_password(now);
                self.notice = None;
                Outcome::Redraw
            }
            secret::Response::Changed => {
                self.notice = None;
                Outcome::Redraw
            }
            secret::Response::Ignored => quiet,
        }
    }

    /// Take the answer to a password check. Returns whether it was right.
    fn verdict(&mut self, verdict: Result<bool, String>, now: Instant) -> bool {
        match verdict {
            Ok(true) => {
                self.phase = Phase::Leaving;
                self.notice = None;
                self.presence
                    .set(0.0, self.ctx.motion.apply(Transition::DISMISS), now);
                return true;
            }
            Ok(false) => {
                self.failures += 1;
                self.notice = Some(("Wrong password".to_owned(), now + NOTICE));
                // After a few misses each further one costs a wait, doubling
                // up to half a minute: patience for a person, a wall for a
                // guessing script.
                if let Some(extra) = self.failures.checked_sub(FREE_ATTEMPTS) {
                    let wait = Duration::from_secs((1u64 << extra.min(5)).min(30));
                    self.locked_out_until = Some(now + wait);
                }
            }
            Err(problem) => {
                self.notice = Some((
                    format!("Cannot check the password: {problem}"),
                    now + NOTICE,
                ));
            }
        }
        self.phase = Phase::Asking;
        self.field.refuse(&self.ctx.motion, now);
        false
    }

    /// Check `password` on another thread: hashing it takes a deliberate
    /// moment, and the screen should not freeze for it.
    fn submit(&mut self, password: Zeroizing<String>) {
        let verify = self.verify;
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let check = move || {
            let verdict = verify(password.as_bytes()).map_err(|err| err.to_string());
            drop(password);
            proxy.send(Message::Verdict(verdict));
        };
        if let Err(err) = thread::Builder::new()
            .name("enso-auth".to_owned())
            .spawn(check)
        {
            self.verdict(Err(err.to_string()), Instant::now());
        }
    }

    /// Whatever has come due. Deciding is all this does; the caller acts.
    fn tick(&mut self, now: Instant) -> Due {
        let mut due = Due::default();
        if self.phase == Phase::Leaving {
            due.unlock = !self.presence.is_animating(now);
            return due;
        }
        if !self.asleep && now >= self.clock_due {
            self.refresh_clock();
            due.redraw = true;
        }
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
            due.redraw = true;
        }
        // The mark breathing, or the light passing over it.
        due.redraw |= !self.asleep && self.mark.pace(now) != Pace::Rest;
        if let Some(until) = self.locked_out_until {
            if now >= until {
                self.locked_out_until = None;
            }
            // The countdown is on screen, a second at a time.
            due.redraw |= !self.asleep;
        }
        let idle = now.saturating_duration_since(self.last_input);
        let after = |seconds: u64| seconds > 0 && idle >= Duration::from_secs(seconds);
        let waiting = self.phase == Phase::Asking;
        if waiting && !self.asleep && after(self.config.screen_off_after) {
            self.asleep = true;
            due.screens_off = true;
            // Nobody to breathe for.
            self.mark.rest();
            // What was half typed and left is not a password any more.
            self.clear_password(now);
            // Nor is there anyone to show the backdrop to: its full-size
            // copies go, and are remade from the small one in a blink.
            self.fitted = Vec::new();
        }
        if waiting && !self.suspended && after(self.config.suspend_after) {
            self.suspended = true;
            due.suspend = true;
        }
        due
    }

    /// When [`Lock::tick`] next has something to do. `None` is the deepest
    /// sleep: nothing is scheduled, and only input wakes the process.
    fn next_wake(&self, now: Instant) -> Option<Duration> {
        let idle_due = |seconds: u64, done: bool| {
            (seconds > 0 && !done).then(|| self.last_input + Duration::from_secs(seconds))
        };
        let lockout = self
            .locked_out_until
            .map(|until| until.min(now + Duration::from_secs(1)));
        let breath = match self.mark.pace(now) {
            Pace::After(after) if !self.asleep => Some(now + after),
            _ => None,
        };
        [
            breath,
            (!self.asleep).then_some(self.clock_due),
            self.notice.as_ref().map(|(_, until)| *until),
            lockout,
            idle_due(self.config.screen_off_after, self.asleep),
            idle_due(self.config.suspend_after, self.suspended),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|due| due.saturating_duration_since(now) + Duration::from_millis(5))
    }

    fn arm(&self, cx: &mut Cx<'_>) {
        if let Some(delay) = self.next_wake(Instant::now()) {
            cx.wake_in(delay);
        }
    }

    /// The backdrop at `size`, fitted once per screen size and kept: every
    /// keystroke redraws over it.
    fn backdrop_for(&mut self, size: (u32, u32)) -> Option<&Picture> {
        let small = self.backdrop.as_ref()?;
        let at = match self.fitted.iter().position(|fit| fit.size() == size) {
            Some(at) => at,
            None => {
                let base = self.ctx.theme.colors.base;
                self.fitted.push(
                    small
                        .fitted(size.0, size.1, Fit::Cover, base)
                        .tinted(base, 1.0 - opacity::SCRIM),
                );
                self.fitted.len() - 1
            }
        };
        self.fitted.get(at)
    }
}

/// Ask Niri to power the screens off or on. Outside Niri this does nothing.
pub fn screens(action: Action) {
    if let Ok(mut socket) = Socket::connect() {
        let _ = socket.send(Request::Action(action));
    }
}

/// Suspend, off the interface thread: reaching logind can take a moment.
fn suspend() {
    let spawned = thread::Builder::new()
        .name("enso-suspend".to_owned())
        .spawn(|| {
            if let Err(err) = logind::machine(Machine::Suspend) {
                eprintln!("enso-lock: cannot suspend: {err}");
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-lock: cannot suspend: {err}");
    }
}

/// The theme's wallpaper as a backdrop: small, and blurred until it is only
/// colour and light. `None` when there is no wallpaper to use.
fn load_backdrop(ctx: &Context) -> Option<Picture> {
    let config: WallConfig = ctx.config.section("wall").unwrap_or_default();
    let chosen = Chosen::load(&ctx.paths);
    let (path, _) = wallpaper::resolve(&config, &chosen, &ctx.theme.id)?;
    let picture = Picture::open(path).ok()?;
    let (width, height) = picture.size();
    let scale = BACKDROP_SIDE as f32 / width.max(height) as f32;
    let small = picture.fitted(
        (width as f32 * scale).round().max(1.0) as u32,
        (height as f32 * scale).round().max(1.0) as u32,
        Fit::Cover,
        ctx.theme.colors.base,
    );
    Some(small.blurred(BACKDROP_SIDE / 40))
}

impl Screens for Lock {
    type Message = Message;

    fn role(&self) -> Role {
        Role::Lock
    }

    fn start(&mut self, proxy: Proxy<Message>, cx: &mut Cx<'_>) {
        self.proxy = Some(proxy.clone());
        if self.config.wallpaper {
            // The screen locks at once on the plain colour; the picture
            // fades in behind when it has been read.
            let ctx = self.ctx.clone();
            let _ = thread::Builder::new()
                .name("enso-backdrop".to_owned())
                .spawn(move || {
                    if let Some(backdrop) = load_backdrop(&ctx) {
                        proxy.send(Message::Backdrop(backdrop));
                    }
                });
        }
        self.last_input = Instant::now();
        self.arm(cx);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        match event {
            Event::Key(key) => match self.key(key, now) {
                Outcome::Ignored => {}
                Outcome::Redraw => cx.redraw(),
                Outcome::Submit(password) => {
                    self.submit(password);
                    cx.redraw();
                }
            },
            Event::Pointer(_) => {
                if self.stir(now) {
                    cx.redraw();
                }
                // The idle deadlines move with every twitch of the mouse;
                // the wake-up already set finds that out when it fires.
                return;
            }
            Event::Message(Message::Verdict(verdict)) => {
                cx.redraw();
                if self.verdict(verdict, now) {
                    // One last wake-up, when the fade has finished.
                    return cx.wake_in(self.ctx.motion.apply(Transition::DISMISS).duration);
                }
            }
            Event::Message(Message::Backdrop(backdrop)) => {
                self.backdrop = Some(backdrop);
                self.fitted.clear();
                self.backdrop_shown
                    .set(1.0, self.ctx.motion.apply(Transition::SCENE), now);
                cx.redraw();
            }
            Event::Wake => {
                let due = self.tick(now);
                if due.unlock {
                    return cx.exit();
                }
                if due.screens_off {
                    screens(Action::PowerOffMonitors {});
                }
                if due.suspend {
                    suspend();
                }
                if due.redraw {
                    cx.redraw();
                }
            }
            _ => return,
        }
        // Once leaving, the only thing still awaited is the end of the fade.
        if self.phase != Phase::Leaving {
            self.arm(cx);
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, screen: &Screen, now: Instant) -> Paint {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let base_size = self.ctx.config.appearance.font_size;
        let bounds = canvas.bounds();

        if !self.revealed {
            self.revealed = true;
            self.presence
                .set(1.0, self.ctx.motion.apply(Transition::REVEAL), now);
            self.mark.paint(&self.ctx.motion, now);
        }
        let presence = self.presence.get(now);

        // The backdrop is always whole: a lock screen must never be seen
        // through, whatever is fading on top of it.
        canvas.clear(colors.base);
        let shown = self.backdrop_shown.get(now);
        if shown > 0.0
            && let Some(backdrop) = self.backdrop_for(screen.pixels())
        {
            canvas.set_opacity(shown);
            canvas.picture(backdrop);
        }

        canvas.set_opacity(presence);
        let drift = (1.0 - presence) * space::MD;

        // Clock and date, a little above the middle.
        let clock = Text::new(&self.time, base_size * 6.0, colors.text).medium();
        let clock_height = clock.line_height();
        let date = Text::new(&self.date, base_size * type_scale::HEADING, colors.subtle);
        let date_height = if self.date.is_empty() {
            0.0
        } else {
            date.line_height()
        };
        // The ensō above it all, small: a ring and the room its brush takes.
        let ring = bounds.w.min(bounds.h) * MARK_SIZE;
        let mark_height = ring * 2.5 + space::LG;
        let column = mark_height + clock_height + date_height + space::XXL + FIELD_HEIGHT;
        let top = ((bounds.h - column) / 2.0 - bounds.h * 0.04).max(space::LG) + drift;
        // Whole as soon as the lock is, whatever fades in around it.
        canvas.set_opacity(1.0);
        self.mark
            .draw(canvas, (bounds.w / 2.0, top + ring * 1.3), ring, now);
        canvas.set_opacity(presence);
        let top = top + mark_height;
        canvas.text_in(
            &clock,
            Rect::new(0.0, top, bounds.w, clock_height),
            Align::Center,
        );
        canvas.text_in(
            &date,
            Rect::new(0.0, top + clock_height, bounds.w, date_height),
            Align::Center,
        );

        // The field: a pill that fills with dots.
        let look = Look::new(&colors, accent, base_size);
        let field = Rect::new(
            (bounds.w - FIELD_WIDTH) / 2.0,
            top + clock_height + date_height + space::XXL,
            FIELD_WIDTH.min(bounds.w - 2.0 * space::LG),
            FIELD_HEIGHT,
        );
        let caption = self.caption(now);
        let said = caption
            .as_ref()
            .map(|(words, complaint)| (words.as_str(), *complaint));
        let checking = self.phase == Phase::Checking;
        let style = secret::Style {
            fill: colors.surface.alpha(0.82),
            icon: Some(icon::LOCK),
            align: Align::Center,
            busy: checking,
            complaint: said.is_some_and(|(_, complaint)| complaint),
            ..secret::Style::new(&look, "Password")
        };
        self.field.draw(canvas, field, &style, now);
        let line = Rect::new(0.0, field.bottom() + space::MD, bounds.w, 22.0);
        Note::under(&self.field, said, checking).draw(
            canvas,
            line,
            &look,
            base_size * type_scale::SMALL,
            Align::Center,
        );
        canvas.set_opacity(1.0);

        Paint::when(
            self.presence.is_animating(now)
                || self.field.is_animating(now)
                || self.backdrop_shown.is_animating(now)
                || self.mark.pace(now) == Pace::Frames,
        )
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Key, Mods, Offscreen, TextEngine};

    use super::*;

    fn lock() -> Lock {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-lock-test"));
        let mut lock = Lock::with_verify(ctx, LockConfig::default(), |password| {
            Ok(password == b"correct horse")
        });
        lock.time = "09:41".to_owned();
        lock.date = "Monday, 16 November".to_owned();
        lock
    }

    fn press(lock: &mut Lock, key: Key, now: Instant) -> Outcome {
        lock.key(
            KeyEvent {
                key,
                mods: Mods::default(),
                repeat: false,
            },
            now,
        )
    }

    fn type_text(lock: &mut Lock, text: &str, now: Instant) {
        for c in text.chars() {
            assert_eq!(press(lock, Key::Char(c), now), Outcome::Redraw);
        }
    }

    /// Type a password and answer it the way the checking thread would.
    fn attempt(lock: &mut Lock, password: &str, now: Instant) -> bool {
        type_text(lock, password, now);
        let Outcome::Submit(typed) = press(lock, Key::Enter, now) else {
            panic!("Enter did not submit");
        };
        assert_eq!(lock.phase, Phase::Checking);
        let verdict = (lock.verify)(typed.as_bytes()).map_err(|err| err.to_string());
        lock.verdict(verdict, now)
    }

    fn render(lock: &mut Lock, now: Instant) -> (Offscreen, Paint) {
        let screen = Screen {
            name: None,
            width: 960,
            height: 540,
            scale: 2.0,
        };
        let mut text = TextEngine::new(&lock.ctx.config.appearance);
        let mut paint = Paint::Idle;
        let frame = Offscreen::render(&mut text, (960, 540), 2, |canvas| {
            paint = lock.draw(canvas, &screen, now);
        });
        (frame, paint)
    }

    /// The pictures held to a reference in `snapshots/`; the rest are
    /// only written, to look at.
    const CHECKED: &[&str] = &["lock-typing"];

    fn snapshot(frame: &Offscreen, name: &str) {
        if CHECKED.contains(&name) {
            frame.check(env!("CARGO_MANIFEST_DIR"), name);
        } else {
            frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
        }
    }

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<LockConfig>("lock").unwrap(),
            LockConfig::default()
        );
    }

    #[test]
    fn the_right_password_unlocks_after_the_fade() {
        let mut lock = lock();
        let now = Instant::now();
        // As if it had been on screen for a while.
        lock.presence.jump(1.0);
        // Enter on an empty field asks nothing.
        assert_eq!(press(&mut lock, Key::Enter, now), Outcome::Ignored);

        assert!(attempt(&mut lock, "correct horse", now));
        assert_eq!(lock.phase, Phase::Leaving);
        assert!(lock.field.is_empty());
        // While leaving, keys do nothing more.
        assert_eq!(press(&mut lock, Key::Char('x'), now), Outcome::Ignored);
        // The session is let go once the fade has run, not before.
        assert!(!lock.tick(now + Duration::from_millis(100)).unlock);
        assert!(lock.tick(now + Duration::from_millis(400)).unlock);
    }

    #[test]
    fn editing_the_field() {
        let mut lock = lock();
        let now = Instant::now();
        type_text(&mut lock, "abc", now);
        assert_eq!(press(&mut lock, Key::Backspace, now), Outcome::Redraw);
        assert_eq!(lock.field.text(), "ab");
        assert_eq!(press(&mut lock, Key::Escape, now), Outcome::Redraw);
        assert!(lock.field.is_empty());
        assert_eq!(press(&mut lock, Key::Backspace, now), Outcome::Ignored);

        // Ctrl+u clears; other shortcuts are not typed into the password.
        type_text(&mut lock, "abc", now);
        let ctrl = |key| KeyEvent {
            key,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            repeat: false,
        };
        assert_eq!(lock.key(ctrl(Key::Char('a')), now), Outcome::Ignored);
        assert_eq!(lock.field.text(), "abc");
        assert_eq!(lock.key(ctrl(Key::Char('u')), now), Outcome::Redraw);
        assert!(lock.field.is_empty());

        // Caps Lock changing is worth a redraw, and a word of warning.
        let caps = KeyEvent {
            key: Key::Other,
            mods: Mods {
                caps_lock: true,
                ..Mods::default()
            },
            repeat: false,
        };
        assert_eq!(lock.key(caps, now), Outcome::Redraw);
        assert_eq!(Note::under(&lock.field, None, false), Note::CapsLock);
    }

    #[test]
    fn wrong_passwords_are_refused_and_then_slowed_down() {
        let mut lock = lock();
        let mut now = Instant::now();
        for _ in 0..FREE_ATTEMPTS - 1 {
            assert!(!attempt(&mut lock, "guess", now));
            assert_eq!(lock.phase, Phase::Asking);
            assert_eq!(
                lock.caption(now).unwrap(),
                ("Wrong password".to_owned(), true)
            );
            assert!(lock.locked_out(now).is_none());
        }
        // The third miss costs a second; the next, two.
        assert!(!attempt(&mut lock, "guess", now));
        assert_eq!(lock.locked_out(now), Some(Duration::from_secs(1)));
        assert_eq!(lock.caption(now).unwrap().0, "Try again in 1 s");
        // Typing during the wait goes nowhere.
        assert_eq!(press(&mut lock, Key::Char('x'), now), Outcome::Ignored);
        assert!(lock.field.is_empty());
        // The countdown keeps the screen fresh, then ends.
        assert!(lock.next_wake(now).unwrap() <= Duration::from_millis(1005));
        now += Duration::from_millis(1100);
        assert!(lock.tick(now).redraw);
        assert!(lock.locked_out(now).is_none());

        assert!(!attempt(&mut lock, "guess", now));
        assert_eq!(lock.locked_out(now), Some(Duration::from_secs(2)));
        // The wait never grows past half a minute.
        lock.failures = 40;
        now += Duration::from_secs(3);
        lock.tick(now);
        assert!(!attempt(&mut lock, "guess", now));
        assert_eq!(lock.locked_out(now), Some(Duration::from_secs(30)));

        // The right password still works once the wait is over.
        now += Duration::from_secs(31);
        lock.tick(now);
        assert!(attempt(&mut lock, "correct horse", now));
    }

    #[test]
    fn a_check_that_cannot_be_made_is_not_a_wrong_password() {
        let mut lock = lock();
        let now = Instant::now();
        assert!(!lock.verdict(Err("no helper".to_owned()), now));
        assert_eq!(lock.failures, 0);
        assert!(lock.caption(now).unwrap().0.contains("no helper"));
        // The notice goes away by itself.
        assert!(lock.tick(now + NOTICE).redraw);
        assert!(lock.caption(now + NOTICE).is_none());
    }

    #[test]
    fn an_unattended_lock_goes_dark_and_then_schedules_nothing() {
        let mut lock = lock();
        lock.config.screen_off_after = 30;
        lock.config.suspend_after = 600;
        let start = Instant::now();
        lock.last_input = start;
        type_text(&mut lock, "half a pass", start);

        // Before the deadline there is something to wake for.
        assert!(lock.next_wake(start).unwrap() <= Duration::from_secs(61));
        assert!(!lock.asleep);

        let later = start + Duration::from_secs(31);
        lock.clock_due = later + Duration::from_secs(600);
        let due = lock.tick(later);
        assert!(due.screens_off && !due.suspend && lock.asleep);
        assert!(lock.fitted.is_empty());
        // What was half typed is gone, and it is asked for only once.
        assert!(lock.field.is_empty());
        assert_eq!(lock.tick(later + Duration::from_secs(1)), Due::default());

        // The one thing left to wake for is the suspend; after that,
        // nothing at all is scheduled, and the process sleeps until a key
        // or the pointer.
        let much_later = start + Duration::from_secs(601);
        assert!(lock.next_wake(later).unwrap() > Duration::from_secs(500));
        let due = lock.tick(much_later);
        assert!(due.suspend && !due.screens_off && !due.redraw);
        assert_eq!(lock.next_wake(much_later), None);
        assert_eq!(
            lock.tick(much_later + Duration::from_secs(3600)),
            Due::default()
        );

        // A key brings it back, with the clock read afresh.
        lock.time.clear();
        let back = later + Duration::from_secs(7200);
        assert_eq!(press(&mut lock, Key::Other, back), Outcome::Redraw);
        assert!(!lock.asleep && !lock.time.is_empty());
        assert!(lock.next_wake(back).is_some());
    }

    #[test]
    fn renders_the_lock_screen() {
        let mut lock = lock();
        let start = Instant::now();
        // Past the ensō's painting, too.
        let settled = start + Duration::from_secs(4);
        let base = lock.ctx.theme.colors.base;

        // The very first frame is already opaque: locked means covered.
        let (frame, paint) = render(&mut lock, start);
        assert_eq!(paint, Paint::Animating);
        assert_eq!(frame.pixel(5, 5), [base.r, base.g, base.b, 255]);
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));

        let (frame, paint) = render(&mut lock, settled);
        assert_eq!(paint, Paint::Idle);
        snapshot(&frame, "lock");

        // A wallpaper arrives and fades in behind; still opaque throughout.
        let mut rgba = Vec::new();
        for y in 0..90u32 {
            for x in 0..160u32 {
                rgba.extend([(x * 255 / 160) as u8, 120, (y * 255 / 90) as u8, 255]);
            }
        }
        lock.backdrop = Some(Picture::from_rgba(160, 90, rgba).unwrap().blurred(4));
        lock.backdrop_shown.jump(1.0);
        type_text(&mut lock, "hunter2", settled);
        let typed = settled + Duration::from_secs(1);
        let (frame, paint) = render(&mut lock, typed);
        assert_eq!(paint, Paint::Idle);
        assert_ne!(frame.pixel(5, 5), [base.r, base.g, base.b, 255]);
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
        assert_eq!(lock.fitted.len(), 1);
        snapshot(&frame, "lock-typing");

        assert!(!attempt(&mut lock, "", typed));
        let (frame, paint) = render(&mut lock, typed + Duration::from_millis(100));
        assert_eq!(paint, Paint::Animating);
        snapshot(&frame, "lock-wrong");

        // Light theme.
        let paths = Paths::rooted("/nonexistent/enso-lock-test");
        let mut ctx = Context::load_from(paths);
        ctx.theme = ctx.themes.get("rose-pine-dawn").unwrap().clone();
        let mut dawn = Lock::with_verify(ctx, LockConfig::default(), |_| Ok(false));
        dawn.time = "09:41".to_owned();
        dawn.date = "Monday, 16 November".to_owned();
        dawn.field.note_mods(Mods {
            caps_lock: true,
            ..Mods::default()
        });
        render(&mut dawn, start);
        snapshot(&render(&mut dawn, settled).0, "lock-rose-pine-dawn");
    }
}
