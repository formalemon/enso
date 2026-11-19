//! The login screen: the ensō, who is logging in, and what PAM asks them.
//!
//! It looks like enso-lock on purpose, so that logging in and unlocking
//! are the same gesture. What differs is underneath: here nothing checks a
//! password. Every question is greetd's (and so PAM's), every answer goes
//! back to greetd, and the screen only waits, shows and forwards.

use std::{
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Transition, Tween,
    style::{icon, space, type_scale},
    wallpaper::Fit,
};
use enso_sys::accounts::User;
use enso_ui::{
    Align, Canvas, Cx, Event, Key, KeyEvent, Paint, Picture, Proxy, Rect, Role, Screen, Screens,
    Text,
    mark::{self, Mark, Pace},
    widget::{Look, Note, SecretField, secret},
};
use serde::Deserialize;

use crate::greetd::{self, Command, ErrorKind, Greetd, Heard, Prompt, Response};

/// `/etc/enso/greet.toml`. Every key is optional.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GreetConfig {
    /// Who is offered first; empty for the first who can log in.
    pub default_user: String,
    /// The session, run by the user's login shell so that their profile
    /// (`~/.bash_profile` and the like) sets its environment.
    pub command: String,
}

impl Default for GreetConfig {
    fn default() -> Self {
        Self {
            default_user: String::new(),
            command: "niri-session".to_owned(),
        }
    }
}

const FIELD_WIDTH: f32 = 280.0;
const FIELD_HEIGHT: f32 = 48.0;
/// Characters accepted; PAM takes no more than this many bytes anyway.
const MAX_ANSWER: usize = 256;
/// How long a passing message stays up.
const NOTICE: Duration = Duration::from_secs(4);
/// What greetd's goodbye looks like when the greeter has no reason to stay
/// up for it: the screen fades and the session starts.
const LEAVE: Transition = Transition::DISMISS;

/// Where the greeter is in its conversation with greetd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// A request is out; its reply is awaited.
    Waiting,
    /// A question is on screen.
    Asking,
    /// Let in, and the session asked for: fading out before going.
    Leaving,
    /// greetd can't be reached. Going, so that greetd starts a new greeter.
    Lost,
}

/// What handling an input or a reply comes to.
#[derive(Debug, PartialEq)]
enum Outcome {
    Ignored,
    Redraw,
    /// Have the conversation thread do this.
    Send(Command),
    /// Done: leave now.
    Exit,
}

impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Begin(a), Self::Begin(b)) => a == b,
            (Self::Answer(a), Self::Answer(b)) => {
                a.as_ref().map(|a| a.as_str()) == b.as_ref().map(|b| b.as_str())
            }
            (Self::Start { cmd: a, env: c }, Self::Start { cmd: b, env: d }) => a == b && c == d,
            _ => false,
        }
    }
}

pub struct Greet {
    ctx: Context,
    config: GreetConfig,
    users: Vec<User>,
    /// Who is logging in: an index into `users`.
    chosen: usize,
    /// Until [`Screens::start`] hands it to the conversation thread.
    greetd: Option<Box<dyn Greetd>>,
    commands: Option<Sender<Command>>,
    /// A preview leaves with Esc, and never quits anyone's compositor.
    preview: bool,
    phase: Phase,
    /// The session has been asked for: the next success means it's started.
    starting: bool,
    /// An answer is with greetd: "Checking…".
    answering: bool,
    field: SecretField,
    /// PAM's question, as the field's placeholder.
    question: String,
    /// What PAM said this attempt, kept over the plainer "Wrong password".
    said: Option<String>,
    /// What to show under the field, and until when.
    notice: Option<(String, Option<Instant>)>,
    /// How present the name and field are: `0` hidden, `1` shown.
    presence: Tween<f32>,
    revealed: bool,
    /// The ensō, as the boot left it: whole, then breathing.
    mark: Mark,
    /// The scene behind it, quietened, as the boot left it too; and fitted
    /// to the screen if the screen is another size.
    backdrop: Option<Picture>,
    fitted: Option<Picture>,
    /// When the ensō is painted and the name and field are let in.
    reveal_at: Option<Instant>,
}

impl std::fmt::Debug for Greet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Greet")
            .field("users", &self.users.len())
            .field("chosen", &self.chosen)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl Greet {
    pub fn new(
        ctx: Context,
        config: GreetConfig,
        users: Vec<User>,
        greetd: Box<dyn Greetd>,
        preview: bool,
        backdrop: Option<Picture>,
    ) -> Self {
        let mark = Mark::new(ctx.theme.colors.rose, ctx.theme.colors.text);
        let chosen = users
            .iter()
            .position(|user| user.name == config.default_user)
            .unwrap_or(0);
        Self {
            ctx,
            config,
            users,
            chosen,
            greetd: Some(greetd),
            commands: None,
            preview,
            phase: Phase::Waiting,
            starting: false,
            answering: false,
            field: SecretField::new(MAX_ANSWER),
            question: "Password".to_owned(),
            said: None,
            notice: None,
            presence: Tween::new(0.0),
            revealed: false,
            mark,
            backdrop,
            fitted: None,
            reveal_at: None,
        }
    }

    fn user(&self) -> Option<&User> {
        self.users.get(self.chosen)
    }

    /// The first thing to ask greetd: log the chosen user in.
    fn begin(&mut self, now: Instant) -> Outcome {
        self.phase = Phase::Waiting;
        self.starting = false;
        self.said = None;
        self.field.clear(&self.ctx.motion, now);
        match self.user() {
            Some(user) => Outcome::Send(Command::Begin(user.name.clone())),
            None => {
                self.notice = Some(("Nobody here can log in".to_owned(), None));
                Outcome::Redraw
            }
        }
    }

    /// The session to start for `user`: their login shell runs the command,
    /// so that their profile sets its environment as a console login would.
    ///
    /// greetd runs a session as `sh -c "exec <its words joined by spaces>"`,
    /// so each word is quoted for that shell: unquoted, `-c "exec
    /// niri-session"` would come apart into `-c exec niri-session`, a bare
    /// `exec`, and a session that ends as soon as it starts.
    fn session(&self, user: &User) -> Command {
        let words = [
            user.login_shell(),
            "-l",
            "-c",
            &format!("exec {}", self.config.command),
        ];
        Command::Start {
            cmd: words.iter().map(|word| quoted(word)).collect(),
            env: vec![
                "XDG_SESSION_TYPE=wayland".to_owned(),
                "XDG_SESSION_DESKTOP=niri".to_owned(),
                "XDG_CURRENT_DESKTOP=niri".to_owned(),
            ],
        }
    }

    fn notify(&mut self, words: impl Into<String>, lasting: bool, now: Instant) {
        let until = (!lasting).then(|| now + NOTICE);
        self.notice = Some((words.into(), until));
    }

    /// The line under the field, and whether it is a complaint.
    fn caption(&self, now: Instant) -> Option<(&str, bool)> {
        if let Some((words, until)) = &self.notice
            && until.is_none_or(|until| now < until)
        {
            return Some((words, true));
        }
        self.answering.then_some(("Checking…", false))
    }

    fn key(&mut self, key: KeyEvent, now: Instant) -> Outcome {
        let quiet = if self.field.note_mods(key.mods) {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        if self.preview && key.key == Key::Escape && self.field.is_empty() {
            return Outcome::Exit;
        }
        if self.phase != Phase::Asking {
            return quiet;
        }
        // Another user: Tab and the arrows go round those who can log in.
        let step = match key.key {
            Key::Tab if key.mods.shift => -1,
            Key::Tab | Key::Down => 1,
            Key::Up => -1,
            _ => 0,
        };
        if step != 0 && key.mods.is_plain() {
            if self.users.len() < 2 {
                return quiet;
            }
            let count = self.users.len() as isize;
            self.chosen = (self.chosen as isize + step).rem_euclid(count) as usize;
            self.notice = None;
            return self.begin(now);
        }
        match self.field.key(&key, &self.ctx.motion, now) {
            secret::Response::Submit
                if self.field.is_empty() && self.field.shown() == secret::Shown::Dots =>
            {
                quiet
            }
            secret::Response::Submit => {
                self.phase = Phase::Waiting;
                self.answering = true;
                self.notice = None;
                Outcome::Send(Command::Answer(Some(
                    self.field.take(&self.ctx.motion, now),
                )))
            }
            // Esc starts the answer over.
            secret::Response::Cancel => {
                self.field.clear(&self.ctx.motion, now);
                self.notice = None;
                Outcome::Redraw
            }
            secret::Response::Changed => {
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|(_, until)| until.is_some())
                {
                    self.notice = None;
                }
                Outcome::Redraw
            }
            secret::Response::Ignored => quiet,
        }
    }

    /// Take what the conversation thread heard.
    fn heard(&mut self, heard: Heard, now: Instant) -> Outcome {
        match heard {
            Heard::Reply(reply) => self.reply(reply, now),
            Heard::Lost(why) => {
                self.phase = Phase::Lost;
                self.notify(format!("Lost greetd: {why}"), true, now);
                Outcome::Redraw
            }
        }
    }

    fn reply(&mut self, reply: Response, now: Instant) -> Outcome {
        if matches!(self.phase, Phase::Leaving | Phase::Lost) {
            return Outcome::Ignored;
        }
        self.answering = false;
        match reply {
            Response::AuthMessage {
                auth_message_type: kind @ (Prompt::Secret | Prompt::Visible),
                auth_message,
            } => {
                let shown = if kind == Prompt::Secret {
                    secret::Shown::Dots
                } else {
                    secret::Shown::Text
                };
                if self.field.shown() != shown {
                    self.field.ask(shown, MAX_ANSWER, &self.ctx.motion, now);
                }
                self.question = question(&auth_message);
                self.phase = Phase::Asking;
                Outcome::Redraw
            }
            // Something to read: shown, and acknowledged at once.
            Response::AuthMessage { auth_message, .. } => {
                let words = auth_message.trim().to_owned();
                if !words.is_empty() {
                    self.notify(words.clone(), true, now);
                    self.said = Some(words);
                }
                Outcome::Send(Command::Answer(None))
            }
            Response::Success if self.starting => {
                self.phase = Phase::Leaving;
                self.presence.set(0.0, self.ctx.motion.apply(LEAVE), now);
                Outcome::Redraw
            }
            Response::Success => match self.user().map(|user| self.session(user)) {
                Some(session) => {
                    self.starting = true;
                    Outcome::Send(session)
                }
                None => Outcome::Ignored,
            },
            Response::Error {
                error_type,
                description,
            } => {
                let words = match (error_type, self.said.take()) {
                    // PAM said why, the account locked say: that, not this.
                    (ErrorKind::AuthError, Some(said)) => said,
                    (ErrorKind::AuthError, None) => "Wrong password".to_owned(),
                    (ErrorKind::Error, _) if self.starting => {
                        format!("Cannot start the session: {description}")
                    }
                    (ErrorKind::Error, _) => description,
                };
                self.notify(words, false, now);
                self.field.refuse(&self.ctx.motion, now);
                let notice = self.notice.take();
                let next = self.begin(now);
                self.notice = notice;
                next
            }
        }
    }

    /// Whatever has come due: returns whether to redraw, and whether to go.
    fn tick(&mut self, now: Instant) -> (bool, bool) {
        let mut redraw = false;
        if let Some((_, Some(until))) = &self.notice
            && now >= *until
        {
            self.notice = None;
            redraw = true;
        }
        let leave = match self.phase {
            Phase::Leaving => !self.presence.is_animating(now),
            // Long enough to read why.
            Phase::Lost => true,
            _ => false,
        };
        (redraw, leave)
    }

    fn next_wake(&self, now: Instant) -> Option<Duration> {
        let due = match self.phase {
            Phase::Leaving => Some(now + self.ctx.motion.apply(LEAVE).duration),
            Phase::Lost => Some(now + NOTICE),
            _ => self.notice.as_ref().and_then(|(_, until)| *until),
        };
        let due = due.map(|due| due.saturating_duration_since(now) + Duration::from_millis(5));
        // The mark's next breath, unless frames are coming anyway.
        let breath = match self.mark.pace(now) {
            Pace::After(after) => Some(after),
            Pace::Frames | Pace::Rest => None,
        };
        due.into_iter().chain(breath).min()
    }

    fn send(&mut self, command: Command) {
        let sent = self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.send(command).is_ok());
        if !sent {
            self.phase = Phase::Lost;
            self.notify("Lost greetd", true, Instant::now());
        }
    }

    fn act(&mut self, outcome: Outcome, cx: &mut Cx<'_>) {
        match outcome {
            Outcome::Ignored => {}
            Outcome::Redraw => cx.redraw(),
            Outcome::Send(command) => {
                self.send(command);
                cx.redraw();
            }
            Outcome::Exit => cx.exit(),
        }
    }
}

/// `word` as one word to a POSIX shell: in single quotes, any single quote
/// in it closed, escaped and reopened.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// PAM's question as a placeholder: "Password: " is "Password".
fn question(asked: &str) -> String {
    let asked = asked.trim().trim_end_matches(':').trim_end();
    if asked.is_empty() {
        "Password".to_owned()
    } else {
        asked.to_owned()
    }
}

impl Screens for Greet {
    type Message = Heard;

    fn role(&self) -> Role {
        Role::Greeter
    }

    fn start(&mut self, proxy: Proxy<Heard>, cx: &mut Cx<'_>) {
        if let Some(greetd) = self.greetd.take() {
            match greetd::converse(greetd, move |heard| proxy.send(heard)) {
                Ok(commands) => self.commands = Some(commands),
                Err(err) => {
                    self.phase = Phase::Lost;
                    self.notify(
                        format!("Cannot talk to greetd: {err}"),
                        true,
                        Instant::now(),
                    );
                }
            }
        }
        let outcome = self.begin(Instant::now());
        self.act(outcome, cx);
        if let Some(delay) = self.next_wake(Instant::now()) {
            cx.wake_in(delay);
        }
    }

    fn event(&mut self, event: Event<Heard>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let outcome = match event {
            Event::Key(key) => self.key(key, now),
            Event::Message(heard) => self.heard(heard, now),
            Event::Wake => {
                let (redraw, leave) = self.tick(now);
                if leave {
                    return cx.exit();
                }
                if redraw || self.mark.pace(now) != Pace::Rest {
                    Outcome::Redraw
                } else {
                    Outcome::Ignored
                }
            }
            _ => return,
        };
        self.act(outcome, cx);
        if let Some(delay) = self.next_wake(now) {
            cx.wake_in(delay);
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, screen: &Screen, now: Instant) -> Paint {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let base_size = self.ctx.config.appearance.font_size;
        let bounds = canvas.bounds();

        // The boot leaves the stage bare; the ensō is painted on it, and
        // only then do the name and the field come in.
        if !self.revealed {
            self.revealed = true;
            self.mark.paint(&self.ctx.motion, now);
            self.reveal_at = Some(now + self.ctx.motion.apply(Transition::BRUSH).duration);
        }
        if let Some(at) = self.reveal_at
            && now >= at
        {
            self.reveal_at = None;
            self.presence
                .set(1.0, self.ctx.motion.apply(Transition::REVEAL), now);
        }
        let presence = self.presence.get(now);

        // The frame the boot hands over, exactly: then the rest fades in.
        canvas.clear(colors.base);
        if let Some(backdrop) = &self.backdrop {
            let size = screen.pixels();
            if backdrop.size() == size {
                canvas.picture(backdrop);
            } else {
                let fitted = self.fitted.get_or_insert_with(|| {
                    backdrop.fitted(size.0, size.1, Fit::Cover, colors.base)
                });
                if fitted.size() != size {
                    *fitted = backdrop.fitted(size.0, size.1, Fit::Cover, colors.base);
                }
                canvas.picture(fitted);
            }
        }
        let (centre, radius) = mark::place(bounds);
        self.mark.draw(canvas, centre, radius, now);

        canvas.set_opacity(presence);
        let drift = (1.0 - presence) * space::MD;

        // The name, between the arrows when there is someone else to choose.
        let name = self.user().map_or("", User::display_name);
        let shown_name = if self.users.len() > 1 {
            format!("‹  {name}  ›")
        } else {
            name.to_owned()
        };
        let name_text =
            Text::new(&shown_name, base_size * type_scale::HEADING, colors.text).medium();
        let name_height = name_text.line_height();

        // Under the ring, clear of the brush lifting off at its top right.
        let name_top = centre.1 + radius * 1.25 + space::XXL + drift;
        canvas.text_in(
            &name_text,
            Rect::new(0.0, name_top, bounds.w, name_height),
            Align::Center,
        );

        let look = Look::new(&colors, accent, base_size);
        let field = Rect::new(
            (bounds.w - FIELD_WIDTH) / 2.0,
            name_top + name_height + space::LG,
            FIELD_WIDTH.min(bounds.w - 2.0 * space::LG),
            FIELD_HEIGHT,
        );
        let said = self.caption(now);
        let style = secret::Style {
            fill: colors.surface.alpha(0.82),
            icon: Some(icon::LOCK),
            align: Align::Center,
            busy: self.answering,
            complaint: said.is_some_and(|(_, complaint)| complaint),
            ..secret::Style::new(&look, &self.question)
        };
        self.field.draw(canvas, field, &style, now);
        let line = Rect::new(0.0, field.bottom() + space::MD, bounds.w, 22.0);
        Note::under(&self.field, said, style.busy).draw(
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
                || self.reveal_at.is_some()
                || self.mark.pace(now) == Pace::Frames,
        )
    }
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;
    use crate::greetd::{Pretend, carry};

    fn people() -> Vec<User> {
        enso_sys::accounts::login_users_in(
            "kai:x:1000:1000:Kai Lin:/home/kai:/bin/bash\nmo:x:1001:1001::/home/mo:/usr/bin/fish\n",
            "/bin/bash\n/usr/bin/fish\n",
            "",
        )
    }

    fn greet(config: GreetConfig) -> Greet {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-greet-test"));
        Greet::new(
            ctx,
            config,
            people(),
            Box::new(Pretend::default()),
            false,
            None,
        )
    }

    fn press(greet: &mut Greet, key: Key, now: Instant) -> Outcome {
        greet.key(
            KeyEvent {
                key,
                mods: Mods::default(),
                repeat: false,
            },
            now,
        )
    }

    /// Run commands against `pretend` the way the thread would, feeding the
    /// replies back, until the greeter wants nothing more.
    fn settle(greet: &mut Greet, pretend: &mut Pretend, mut outcome: Outcome, now: Instant) {
        while let Outcome::Send(command) = outcome {
            outcome = greet.heard(carry(pretend, command), now);
        }
    }

    fn type_answer(greet: &mut Greet, pretend: &mut Pretend, text: &str, now: Instant) {
        for c in text.chars() {
            assert_eq!(press(greet, Key::Char(c), now), Outcome::Redraw);
        }
        let outcome = press(greet, Key::Enter, now);
        assert!(matches!(outcome, Outcome::Send(Command::Answer(Some(_)))));
        settle(greet, pretend, outcome, now);
    }

    fn render(greet: &mut Greet, now: Instant) -> (Offscreen, Paint) {
        let screen = Screen {
            name: None,
            width: 960,
            height: 540,
            scale: 2.0,
        };
        let mut text = TextEngine::new(&greet.ctx.config.appearance);
        let mut paint = Paint::Idle;
        let frame = Offscreen::render(&mut text, (960, 540), 2, |canvas| {
            paint = greet.draw(canvas, &screen, now);
        });
        (frame, paint)
    }

    #[test]
    fn logs_in_and_starts_the_session_through_the_login_shell() {
        let mut greet = greet(GreetConfig {
            default_user: "mo".to_owned(),
            ..GreetConfig::default()
        });
        let mut pretend = Pretend::default();
        let now = Instant::now();
        // As if it had been on screen for a while.
        greet.presence.jump(1.0);
        let first = greet.begin(now);
        assert_eq!(first, Outcome::Send(Command::Begin("mo".to_owned())));
        settle(&mut greet, &mut pretend, first, now);
        assert_eq!(greet.phase, Phase::Asking);
        assert_eq!(greet.question, "Password");

        // Enter on nothing asks nothing.
        assert_eq!(press(&mut greet, Key::Enter, now), Outcome::Ignored);
        type_answer(&mut greet, &mut pretend, greetd::PRETEND_PASSWORD, now);
        assert_eq!(greet.phase, Phase::Leaving);
        assert_eq!(
            pretend.started.as_deref().unwrap(),
            ["'/usr/bin/fish'", "'-l'", "'-c'", "'exec niri-session'"]
        );
        // Keys do nothing more; the greeter goes once the fade has run.
        assert_eq!(press(&mut greet, Key::Char('x'), now), Outcome::Ignored);
        assert!(!greet.tick(now + Duration::from_millis(50)).1);
        assert!(greet.tick(now + Duration::from_secs(1)).1);
    }

    /// What greetd runs, as greetd runs it: the words joined, behind
    /// `exec`, by `sh -c`. The session gets its command whole.
    #[test]
    fn the_session_survives_greetds_shell() {
        let greet = greet(GreetConfig {
            command: r#"printf '%s|' "it's""#.to_owned(),
            ..GreetConfig::default()
        });
        let user = enso_sys::accounts::login_users_in(
            "kai:x:1000:1000::/home/kai:/bin/sh\n",
            "/bin/sh\n",
            "",
        )
        .remove(0);
        let Command::Start { cmd, .. } = greet.session(&user) else {
            panic!("not a session");
        };
        let script = format!("exec {}", cmd.join(" "));
        // The login shell's profile may print; only the last line counts.
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", "/nonexistent")
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&out.stdout);
        assert_eq!(printed.lines().last(), Some("it's|"));
    }

    #[test]
    fn a_wrong_password_is_refused_and_asked_again() {
        let mut greet = greet(GreetConfig::default());
        let mut pretend = Pretend::default();
        let now = Instant::now();
        let first = greet.begin(now);
        settle(&mut greet, &mut pretend, first, now);
        type_answer(&mut greet, &mut pretend, "guess", now);
        assert_eq!(greet.phase, Phase::Asking);
        assert!(greet.field.is_empty());
        assert_eq!(greet.caption(now), Some(("Wrong password", true)));
        assert!(pretend.started.is_none());
        // Typing again puts the complaint away.
        press(&mut greet, Key::Char('e'), now);
        assert_eq!(greet.caption(now), None);
        for c in "nso".chars() {
            press(&mut greet, Key::Char(c), now);
        }
        let outcome = press(&mut greet, Key::Enter, now);
        settle(&mut greet, &mut pretend, outcome, now);
        assert_eq!(greet.phase, Phase::Leaving);
    }

    #[test]
    fn what_pam_says_is_shown_and_outlasts_the_plain_refusal() {
        let mut greet = greet(GreetConfig::default());
        let now = Instant::now();
        greet.phase = Phase::Waiting;
        let locked = "The account is locked due to 3 failed logins.";
        let reply = greet.reply(
            Response::AuthMessage {
                auth_message_type: Prompt::Error,
                auth_message: format!("{locked}\n"),
            },
            now,
        );
        assert_eq!(reply, Outcome::Send(Command::Answer(None)));
        assert_eq!(greet.caption(now), Some((locked, true)));
        let next = greet.reply(
            Response::Error {
                error_type: ErrorKind::AuthError,
                description: "pam_authenticate: AUTH_ERR".to_owned(),
            },
            now,
        );
        assert_eq!(next, Outcome::Send(Command::Begin("kai".to_owned())));
        assert_eq!(greet.caption(now), Some((locked, true)));
        // A visible question shows what's typed.
        greet.reply(
            Response::AuthMessage {
                auth_message_type: Prompt::Visible,
                auth_message: "Code: ".to_owned(),
            },
            now,
        );
        assert_eq!(greet.field.shown(), secret::Shown::Text);
        assert_eq!(greet.question, "Code");
    }

    #[test]
    fn another_user_starts_over() {
        let mut greet = greet(GreetConfig::default());
        let mut pretend = Pretend::default();
        let now = Instant::now();
        let first = greet.begin(now);
        settle(&mut greet, &mut pretend, first, now);
        assert_eq!(greet.user().unwrap().name, "kai");
        press(&mut greet, Key::Char('x'), now);
        let switched = press(&mut greet, Key::Tab, now);
        assert_eq!(switched, Outcome::Send(Command::Begin("mo".to_owned())));
        assert!(greet.field.is_empty());
        settle(&mut greet, &mut pretend, switched, now);
        // Round the end and back.
        assert_eq!(
            press(&mut greet, Key::Down, now),
            Outcome::Send(Command::Begin("kai".to_owned()))
        );
        greet.phase = Phase::Asking;
        assert_eq!(
            press(&mut greet, Key::Up, now),
            Outcome::Send(Command::Begin("mo".to_owned()))
        );
        // While waiting on greetd, no switching.
        assert_eq!(press(&mut greet, Key::Tab, now), Outcome::Ignored);
    }

    #[test]
    fn a_lost_greetd_ends_the_greeter() {
        let mut greet = greet(GreetConfig::default());
        let now = Instant::now();
        greet.heard(Heard::Lost("broken pipe".to_owned()), now);
        assert_eq!(greet.phase, Phase::Lost);
        assert!(greet.caption(now).unwrap().0.contains("broken pipe"));
        assert!(greet.tick(now).1);
        // Only a preview leaves on Esc.
        let mut greet = self::greet(GreetConfig::default());
        assert_eq!(press(&mut greet, Key::Escape, now), Outcome::Ignored);
        greet.preview = true;
        assert_eq!(press(&mut greet, Key::Escape, now), Outcome::Exit);
    }

    #[test]
    fn nobody_to_log_in() {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-greet-test"));
        let mut greet = Greet::new(
            ctx,
            GreetConfig::default(),
            Vec::new(),
            Box::new(Pretend::default()),
            false,
            None,
        );
        let now = Instant::now();
        assert_eq!(greet.begin(now), Outcome::Redraw);
        assert!(greet.caption(now).is_some());
        render(&mut greet, now);
    }

    #[test]
    fn config_reads_and_refuses_strangers() {
        let config: GreetConfig =
            toml::from_str("default_user = \"ren\"\ncommand = \"sway\"\n").unwrap();
        assert_eq!(config.default_user, "ren");
        assert_eq!(config.command, "sway");
        assert_eq!(
            toml::from_str::<GreetConfig>("").unwrap(),
            GreetConfig::default()
        );
        assert!(toml::from_str::<GreetConfig>("colour = 1").is_err());
    }

    #[test]
    fn renders_the_greeter() {
        let mut greet = greet(GreetConfig::default());
        let mut pretend = Pretend::default();
        let start = Instant::now();
        // Past the ensō's painting and the field's arrival.
        let settled = start + Duration::from_secs(4);
        let base = greet.ctx.theme.colors.base;
        let first = greet.begin(start);
        settle(&mut greet, &mut pretend, first, start);

        let (frame, paint) = render(&mut greet, start);
        assert_eq!(paint, Paint::Animating);
        assert_eq!(frame.pixel(5, 5), [base.r, base.g, base.b, 255]);
        // The first frame is the one the boot hands over, pixel for pixel.
        let mut text = TextEngine::new(&greet.ctx.config.appearance);
        let colors = greet.ctx.theme.colors.clone();
        let handed = Offscreen::render(&mut text, (960, 540), 2, |canvas| {
            mark::stage(canvas, colors.base, None);
        });
        assert!(frame.rgba == handed.rgba);
        // Painted by now: the name and the field are let in.
        let (_, paint) = render(&mut greet, start + Duration::from_secs(3));
        assert_eq!(paint, Paint::Animating);
        let (frame, paint) = render(&mut greet, settled);
        assert_eq!(paint, Paint::Idle);
        assert!(frame.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
        frame.check(env!("CARGO_MANIFEST_DIR"), "greet");

        for c in "hunter2".chars() {
            press(&mut greet, Key::Char(c), settled);
        }
        let later = settled + Duration::from_secs(1);
        frame_named(&mut greet, later, "greet-typing");

        let outcome = press(&mut greet, Key::Enter, later);
        settle(&mut greet, &mut pretend, outcome, later);
        frame_named(
            &mut greet,
            later + Duration::from_millis(100),
            "greet-wrong",
        );
    }

    #[test]
    fn over_the_backdrop_it_starts_as_the_boot_ends() {
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-greet-test"));
        let colors = ctx.theme.colors.clone();
        // At the screen's own pixels, as enable draws it.
        let backdrop = mark::backdrop(mark::scene(&colors, (1920, 1080)), colors.base);
        let mut greet = Greet::new(
            ctx,
            GreetConfig::default(),
            people(),
            Box::new(Pretend::default()),
            false,
            Some(backdrop.clone()),
        );
        let now = Instant::now();
        let (first, _) = render(&mut greet, now);
        let mut text = TextEngine::new(&greet.ctx.config.appearance);
        let handed = Offscreen::render(&mut text, (960, 540), 2, |canvas| {
            mark::stage(canvas, colors.base, Some(&backdrop));
        });
        assert!(first.rgba == handed.rgba);
        render(&mut greet, now + Duration::from_secs(3));
        let (later, _) = render(&mut greet, now + Duration::from_secs(4));
        later.snapshot(env!("CARGO_MANIFEST_DIR"), "greet-backdrop");
    }

    fn frame_named(greet: &mut Greet, now: Instant, name: &str) {
        let (frame, _) = render(greet, now);
        frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
    }
}
