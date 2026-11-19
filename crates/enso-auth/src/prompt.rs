//! The prompt: one card over a dimmed screen, asking one thing.
//!
//! For polkit, a password: typed as dots, checked by polkit's helper when
//! Enter is pressed, the field shaking if it was wrong. For Bluetooth, a
//! code to compare or type, a PIN or passkey to enter, or a yes or no.
//! Enter answers yes, Esc no; the mouse works too. It takes the keyboard
//! while it's up and isn't dismissed by a stray click outside.
//!
//! The answer is one line on standard output (see [`crate::request`]).

use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
    thread,
    time::Instant,
};

use enso_core::{
    Context,
    style::{icon, type_scale},
};
use enso_ui::{
    App, Canvas, Cx, Event, Key, KeyEvent, Paint, PointerEvent, Presence, Proxy, Rect, SurfaceSpec,
    widget::{Form, Interaction, Kind, Look, Note, Part, SecretField, Shown, secret},
};

use zeroize::Zeroizing;

use crate::{
    helper::{self, Said, Verdict},
    request::{Answer, Request},
};

/// Characters taken: PAM takes no more than this anyway.
const MAX_SECRET: usize = 256;
const MAX_PIN: usize = 16;
const PASSKEY_DIGITS: usize = 6;

#[derive(Debug)]
pub enum Message {
    /// How the password check went.
    Verdict(Result<Verdict, String>),
    /// PAM says something along the way, or asks something more.
    Said(Said),
    /// The agent took the question back.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Primary,
    Secondary,
    User(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Asking,
    /// The helper has the password.
    Checking,
    /// Answered; fading out.
    Leaving,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ignored,
    Redraw,
    Close,
}

pub struct Prompt {
    ctx: Context,
    request: Request,
    /// Shown by `enso-auth try`: nothing is checked or sent anywhere.
    preview: bool,
    out: Option<Box<dyn Write>>,
    answered: Option<Answer>,
    presence: Presence,
    phase: Phase,
    field: SecretField,
    /// Which of the users is answering.
    user: usize,
    /// A line under the field: what went wrong (`true`), or a note.
    notice: Option<(String, bool)>,
    /// A further question from PAM: its words, and whether typing shows.
    further: Option<(String, bool)>,
    reply: Option<mpsc::Sender<Option<Zeroizing<String>>>>,
    proxy: Option<Proxy<Message>>,
    hits: Vec<(Rect, Hit)>,
    hover: Option<Hit>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never what's typed.
        f.debug_struct("Prompt")
            .field("request", &self.request.is_bluetooth())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl Prompt {
    pub fn new(ctx: Context, request: Request, out: Option<Box<dyn Write>>, preview: bool) -> Self {
        let mut field = SecretField::new(MAX_SECRET);
        if let Some((shown, max)) = wanted(&request, None) {
            field.ask(shown, max, &ctx.motion, std::time::Instant::now());
        }
        Self {
            presence: enso_ui::Presence::new(&ctx.motion),
            ctx,
            request,
            preview,
            out,
            answered: None,
            phase: Phase::Asking,
            field,
            user: 0,
            notice: None,
            further: None,
            reply: None,
            proxy: None,
            hits: Vec::new(),
            hover: None,
        }
    }

    fn font(&self, scale: f32) -> f32 {
        (self.ctx.config.appearance.font_size * scale).round()
    }

    fn look(&self) -> Look {
        Look::new(
            &self.ctx.theme.colors,
            self.ctx.accent(),
            self.font(type_scale::BODY),
        )
    }

    /// What the field asks for now, if there is a field.
    fn wanted(&self) -> Option<(Shown, usize)> {
        wanted(&self.request, self.further.as_ref())
    }

    fn users(&self) -> &[String] {
        match &self.request {
            Request::Password { users, .. } => users,
            _ => &[],
        }
    }

    /// The two buttons' words: the "no", and the "yes" (if there is one).
    fn buttons(&self) -> (&'static str, Option<&'static str>) {
        match &self.request {
            Request::Password { .. } if self.phase == Phase::Checking => {
                ("Cancel", Some("Checking…"))
            }
            Request::Password { .. } => ("Cancel", Some("Allow")),
            Request::Confirm { .. } => ("They differ", Some("Pair")),
            Request::Authorize { .. } => ("Don't pair", Some("Pair")),
            Request::Service { .. } => ("Don't allow", Some("Allow")),
            Request::Pin { .. } | Request::Passkey { .. } => ("Cancel", Some("Pair")),
            Request::Show { .. } => ("Cancel", None),
        }
    }

    /// Empty the field, asking for what's wanted now.
    fn clear(&mut self, now: Instant) {
        if let Some((shown, max)) = self.wanted() {
            self.field.ask(shown, max, &self.ctx.motion, now);
        }
    }

    /// Say `answer`, once, and go.
    fn answer(&mut self, answer: Answer) -> Outcome {
        if self.answered.is_some() {
            return Outcome::Ignored;
        }
        if let Some(out) = &mut self.out {
            let _ = writeln!(out, "{}", answer.line());
            let _ = out.flush();
        }
        // A conversation still waiting on a further question is given up.
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(None);
        }
        self.answered = Some(answer);
        self.phase = Phase::Leaving;
        Outcome::Close
    }

    fn primary(&mut self, now: Instant) -> Outcome {
        if self.phase != Phase::Asking {
            return Outcome::Ignored;
        }
        match &self.request {
            Request::Password { users, cookie, .. } => {
                let (user, cookie) = (users.get(self.user).cloned(), cookie.clone());
                if self.field.is_empty() {
                    return Outcome::Ignored;
                }
                let typed = self.field.take(&self.ctx.motion, now);
                self.notice = None;
                self.phase = Phase::Checking;
                if self.further.take().is_some() {
                    // The answer to PAM's further question.
                    if let Some(reply) = &self.reply {
                        let _ = reply.send(Some(typed));
                    }
                    return Outcome::Redraw;
                }
                if self.preview {
                    return self.answer(Answer::Allow);
                }
                let (Some(user), Some(proxy)) = (user, self.proxy.clone()) else {
                    return self.answer(Answer::Deny);
                };
                let (reply, replies) = mpsc::channel();
                self.reply = Some(reply);
                thread::spawn(move || {
                    let verdict = helper::check(&user, &cookie, typed, |said| {
                        let asks = matches!(said, Said::Ask { .. });
                        proxy.send(Message::Said(said));
                        if asks {
                            replies.recv().ok().flatten()
                        } else {
                            None
                        }
                    });
                    proxy.send(Message::Verdict(verdict.map_err(|err| err.to_string())));
                });
                Outcome::Redraw
            }
            Request::Pin { .. } if !self.field.is_empty() => {
                let pin = self.field.take(&self.ctx.motion, now);
                self.answer(Answer::Pin(pin.as_str().to_owned()))
            }
            Request::Passkey { .. } => match self.field.text().parse() {
                Ok(passkey) => self.answer(Answer::Passkey(passkey)),
                Err(_) => Outcome::Ignored,
            },
            Request::Confirm { .. } | Request::Authorize { .. } | Request::Service { .. } => {
                self.answer(Answer::Allow)
            }
            _ => Outcome::Ignored,
        }
    }

    fn message(&mut self, message: Message, now: Instant) -> Outcome {
        match message {
            Message::Close => self.answer(Answer::Deny),
            Message::Said(Said::Ask { text, echo }) => {
                self.further = Some((text.trim().trim_end_matches(':').to_owned(), echo));
                self.phase = Phase::Asking;
                self.notice = None;
                self.clear(now);
                Outcome::Redraw
            }
            Message::Said(Said::Info(text)) => {
                self.notice = Some((text, false));
                Outcome::Redraw
            }
            Message::Said(Said::Error(text)) => {
                self.notice = Some((text, true));
                Outcome::Redraw
            }
            Message::Verdict(verdict) => {
                self.reply = None;
                self.further = None;
                if self.phase == Phase::Leaving {
                    return Outcome::Ignored;
                }
                match verdict {
                    Ok(Verdict::Accepted) => return self.answer(Answer::Allow),
                    Ok(Verdict::Abandoned) => return self.answer(Answer::Deny),
                    Ok(Verdict::Refused) => {
                        // PAM's own words win: "the account is locked…".
                        if !self
                            .notice
                            .as_ref()
                            .is_some_and(|(_, complaint)| *complaint)
                        {
                            self.notice =
                                Some(("That's not the password. Try again.".to_owned(), true));
                        }
                        self.field.refuse(&self.ctx.motion, now);
                    }
                    Err(err) => self.notice = Some((format!("Can't check it: {err}"), true)),
                }
                self.phase = Phase::Asking;
                self.clear(now);
                Outcome::Redraw
            }
        }
    }

    fn key(&mut self, key: &KeyEvent, now: Instant) -> Outcome {
        let quiet = if self.field.note_mods(key.mods) {
            Outcome::Redraw
        } else {
            Outcome::Ignored
        };
        match key.key {
            Key::Escape => return self.answer(Answer::Deny),
            Key::Enter => return self.primary(now),
            _ if self.phase != Phase::Asking => return quiet,
            Key::Tab | Key::Left | Key::Right
                if self.users().len() > 1 && self.further.is_none() =>
            {
                let count = self.users().len();
                self.user = if key.key == Key::Left || (key.key == Key::Tab && key.mods.shift) {
                    (self.user + count - 1) % count
                } else {
                    (self.user + 1) % count
                };
                self.notice = None;
                self.clear(now);
                return Outcome::Redraw;
            }
            _ => {}
        }
        if self.wanted().is_none()
            || self.field.key(key, &self.ctx.motion, now) != secret::Response::Changed
        {
            return quiet;
        }
        // Typing again takes back the complaint.
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, complaint)| *complaint)
        {
            self.notice = None;
        }
        Outcome::Redraw
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map(|(_, hit)| *hit)
    }

    fn pointer(&mut self, event: PointerEvent, now: Instant) -> Outcome {
        match event {
            PointerEvent::Enter { x, y } | PointerEvent::Move { x, y } => {
                let hover = self.hit(x, y);
                if std::mem::replace(&mut self.hover, hover) == hover {
                    Outcome::Ignored
                } else {
                    Outcome::Redraw
                }
            }
            PointerEvent::Leave => {
                self.hover = None;
                Outcome::Redraw
            }
            PointerEvent::Press { x, y, .. } => match self.hit(x, y) {
                Some(Hit::Primary) => self.primary(now),
                Some(Hit::Secondary) => self.answer(Answer::Deny),
                Some(Hit::User(index)) if self.phase == Phase::Asking && self.further.is_none() => {
                    self.user = index;
                    self.clear(now);
                    Outcome::Redraw
                }
                _ => Outcome::Ignored,
            },
            _ => Outcome::Ignored,
        }
    }

    /// The words: title, what it's about, and who's asking.
    fn words(&self) -> (String, String, Option<String>) {
        match &self.request {
            Request::Password {
                message,
                action,
                program,
                ..
            } => (
                "Confirm it's you".to_owned(),
                message.clone(),
                Some(match program {
                    Some(program) => format!("Asked by {}", program_name(program)),
                    None => action.clone(),
                }),
            ),
            Request::Confirm { device, .. } => (
                format!("Pair with {device}?"),
                format!("Pair if {device} shows the same number."),
                None,
            ),
            Request::Authorize { device } => (
                format!("Pair with {device}?"),
                "It wants to pair without a code. Pair only if you started this.".to_owned(),
                None,
            ),
            Request::Service { device, service } => (
                format!("Let {device} use {service}?"),
                "It isn't trusted yet. Allow it only if you know this device.".to_owned(),
                None,
            ),
            Request::Pin { device } => (
                format!("PIN for {device}"),
                format!("Type the PIN {device} wants. Older headsets and car kits often use 0000."),
                None,
            ),
            Request::Passkey { device } => (
                format!("Code from {device}"),
                format!("Type the six-digit number {device} shows."),
                None,
            ),
            Request::Show { device, .. } => (
                format!("Type this on {device}"),
                "Then press Enter on it. This goes away once it's paired.".to_owned(),
                None,
            ),
        }
    }

    /// The big number to compare or type, spaced in threes.
    fn code(&self) -> Option<String> {
        let digits = match &self.request {
            Request::Confirm { passkey, .. } => format!("{passkey:06}"),
            Request::Show { code, .. } => code.clone(),
            _ => return None,
        };
        let chars: Vec<char> = digits.chars().collect();
        Some(
            chars
                .chunks(3)
                .map(|chunk| chunk.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn render(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let look = self.look();
        self.hits.clear();

        let (title, about, aside) = self.words();
        let (badge, tint) = if self.request.is_bluetooth() {
            (icon::BLUETOOTH, colors.foam)
        } else {
            (icon::LOCK, look.accent)
        };
        let users: Vec<&str> = self.users().iter().map(String::as_str).collect();
        let code = self.code();
        let wanted = self.wanted();
        let busy = self.phase != Phase::Asking;
        let said = self
            .notice
            .as_ref()
            .map(|(words, complaint)| (words.as_str(), *complaint));
        let (no, yes) = self.buttons();

        let mut form = Form::new(&title)
            .badge(badge, tint)
            .about(&about)
            .aside(aside.as_deref());
        if !users.is_empty() {
            form = form.part(Part::Choices {
                label: Some("As"),
                options: &users,
                chosen: self.user,
                hovered: match self.hover {
                    Some(Hit::User(index)) => Some(index),
                    _ => None,
                },
                enabled: users.len() > 1 && self.further.is_none(),
            });
        }
        if let Some(code) = &code {
            form = form.part(Part::Code(code));
        }
        let field_at = wanted.map(|_| form_len(&users, &code));
        if wanted.is_some() {
            form = form.field();
        }
        form = form
            .note(Note::under(
                &self.field,
                said,
                self.phase == Phase::Checking,
            ))
            .button(
                no,
                Kind::Secondary,
                Interaction {
                    hovered: self.hover == Some(Hit::Secondary),
                    ..Interaction::default()
                },
            );
        if let Some(yes) = yes {
            form = form.button(
                yes,
                Kind::Primary,
                Interaction {
                    hovered: self.hover == Some(Hit::Primary),
                    disabled: busy || (wanted.is_some() && self.field.is_empty()),
                    ..Interaction::default()
                },
            );
        }
        let placed = form.draw(canvas, canvas.bounds(), &look, presence);

        if let (Some(at), Some((shown, _))) = (field_at, wanted) {
            let hint = if self.phase == Phase::Checking {
                "Checking…".to_owned()
            } else {
                match (&self.further, shown) {
                    (Some((question, _)), _) => question.clone(),
                    (None, Shown::Dots) => match self.users().get(self.user) {
                        Some(user) => format!("{user}'s password"),
                        None => "Password".to_owned(),
                    },
                    (None, Shown::Digits) => "000000".to_owned(),
                    (None, Shown::Text) => "PIN".to_owned(),
                }
            };
            let style = secret::Style {
                busy,
                complaint: said.is_some_and(|(_, complaint)| complaint),
                ..secret::Style::new(&look, &hint)
            };
            self.field.draw(canvas, placed.parts[at], &style, now);
        }
        for &(_, option, pill) in &placed.choices {
            self.hits.push((pill, Hit::User(option)));
        }
        self.hits.push((placed.buttons[0], Hit::Secondary));
        if let Some(&rect) = placed.buttons.get(1) {
            self.hits.push((rect, Hit::Primary));
        }
        canvas.set_opacity(1.0);

        Paint::when(self.presence.is_animating(now) || self.field.is_animating(now))
    }
}

/// Where the field comes among the form's parts: after the users and the
/// code, when there are those.
fn form_len(users: &[&str], code: &Option<String>) -> usize {
    usize::from(!users.is_empty()) + usize::from(code.is_some())
}

/// What the field asks for, for `request` (and PAM's `further` question,
/// with whether its answer shows): how it's shown, and how long it may be.
fn wanted(request: &Request, further: Option<&(String, bool)>) -> Option<(Shown, usize)> {
    match (request, further) {
        (Request::Password { .. }, Some((_, true))) => Some((Shown::Text, MAX_SECRET)),
        (Request::Password { .. }, _) => Some((Shown::Dots, MAX_SECRET)),
        (Request::Pin { .. }, _) => Some((Shown::Text, MAX_PIN)),
        (Request::Passkey { .. }, _) => Some((Shown::Digits, PASSKEY_DIGITS)),
        _ => None,
    }
}

impl App for Prompt {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-auth")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if !self.preview {
            // The agent may take the question back; if it goes away, so does
            // the question.
            let closer = proxy.clone();
            thread::spawn(move || {
                let stdin = io::stdin();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdin.lock().read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line.trim() == "close" => break,
                        Ok(_) => {}
                    }
                }
                closer.send(Message::Close);
            });
        }
        self.proxy = Some(proxy);
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let now = Instant::now();
        let leaving = self.phase == Phase::Leaving;
        let outcome = match event {
            Event::Wake if leaving && self.presence.is_closing() => {
                cx.exit();
                return;
            }
            Event::Message(message) => self.message(message, now),
            Event::Key(key) if !leaving => self.key(&key, now),
            Event::Pointer(event) if !leaving => self.pointer(event, now),
            Event::Close => self.answer(Answer::Deny),
            _ => Outcome::Ignored,
        };
        match outcome {
            Outcome::Ignored => {}
            Outcome::Redraw => cx.redraw(),
            Outcome::Close => self.presence.close(cx),
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        self.render(canvas, now)
    }
}

/// What to call the program asking: the name its application goes by
/// (`Files`), with the program itself (`enso-files`) beside it — or just
/// the program, when no application owns it.
fn program_name(program: &str) -> String {
    let base = program.rsplit('/').next().unwrap_or(program);
    match application_named(base) {
        Some(name) if !name.eq_ignore_ascii_case(base) => format!("{name} ({base})"),
        _ => base.to_owned(),
    }
}

/// The name of the application whose desktop entry runs `program`. (Not
/// in tests, whose pictures mustn't depend on what's installed.)
fn application_named(program: &str) -> Option<String> {
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let shared =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    let dirs = std::iter::once(home.join(".local/share"))
        .chain(shared.split(':').map(std::path::PathBuf::from))
        .map(|dir| dir.join("applications"));
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().is_none_or(|ext| ext != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let (mut name, mut runs) = (None, false);
            let mut in_entry = false;
            for line in text.lines().map(str::trim) {
                if line.starts_with('[') {
                    in_entry = line == "[Desktop Entry]";
                } else if in_entry && let Some(value) = line.strip_prefix("Name=") {
                    name = Some(value.to_owned());
                } else if in_entry && let Some(exec) = line.strip_prefix("Exec=") {
                    let first = exec.split_whitespace().next().unwrap_or_default();
                    runs = first.trim_matches('"').rsplit('/').next() == Some(program);
                }
            }
            if runs {
                return name;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    fn context() -> Context {
        Context::load_from(Paths::rooted("/nonexistent/enso-auth-test"))
    }

    fn prompt(request: Request) -> Prompt {
        Prompt::new(context(), request, None, true)
    }

    fn password() -> Request {
        Request::Password {
            message: "Authentication is required to mount Windows (1.0 TB NTFS).".into(),
            action: "org.freedesktop.udisks2.filesystem-mount-system".into(),
            program: Some("/usr/bin/enso-files".into()),
            users: vec!["ren".into(), "root".into()],
            cookie: "cookie".into(),
        }
    }

    fn press(prompt: &mut Prompt, key: Key) -> Outcome {
        let event = KeyEvent {
            key,
            mods: Mods::default(),
            repeat: false,
        };
        prompt.key(&event, Instant::now())
    }

    fn type_text(prompt: &mut Prompt, text: &str) {
        for c in text.chars() {
            press(prompt, Key::Char(c));
        }
    }

    fn render(prompt: &mut Prompt, name: &str) {
        let mut text = TextEngine::new(&prompt.ctx.config.appearance);
        // The first frame starts the reveal; the snapshot is after it.
        let now = Instant::now();
        Offscreen::render(&mut text, (760, 520), 1, |canvas| {
            prompt.render(canvas, now);
        });
        let later = now + std::time::Duration::from_secs(2);
        let frame = Offscreen::render(&mut text, (760, 520), 2, |canvas| {
            prompt.render(canvas, later);
        });
        // The one held to a reference in `snapshots/`; the rest are only
        // written, to look at.
        if name == "enso-auth-password" {
            frame.check(env!("CARGO_MANIFEST_DIR"), name);
        } else {
            frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
        }
    }

    #[test]
    fn a_password_is_typed_and_sent() {
        let mut prompt = prompt(password());
        assert_eq!(
            press(&mut prompt, Key::Enter),
            Outcome::Ignored,
            "nothing typed"
        );
        type_text(&mut prompt, "hunter2");
        press(&mut prompt, Key::Backspace);
        assert_eq!(prompt.field.text(), "hunter");
        assert_eq!(press(&mut prompt, Key::Enter), Outcome::Close);
        assert_eq!(prompt.answered, Some(Answer::Allow));
        assert!(prompt.field.is_empty());
    }

    #[test]
    fn a_wrong_password_shakes_and_is_cleared() {
        let mut prompt = prompt(password());
        prompt.preview = false;
        prompt.phase = Phase::Checking;
        let now = Instant::now();
        prompt.message(Message::Verdict(Ok(Verdict::Refused)), now);
        assert_eq!(prompt.phase, Phase::Asking);
        assert!(prompt.field.is_animating(now));
        assert!(prompt.notice.as_ref().unwrap().1);
        // PAM's own complaint is kept over the plain one.
        prompt.phase = Phase::Checking;
        prompt.message(
            Message::Said(Said::Error("The account is locked.".into())),
            now,
        );
        prompt.message(Message::Verdict(Ok(Verdict::Refused)), now);
        assert_eq!(prompt.notice.as_ref().unwrap().0, "The account is locked.");
        render(&mut prompt, "enso-auth-password");
        // Typing takes it back.
        type_text(&mut prompt, "x");
        assert!(prompt.notice.is_none());
    }

    #[test]
    fn users_can_be_chosen_and_esc_says_no() {
        let mut prompt = prompt(password());
        type_text(&mut prompt, "abc");
        press(&mut prompt, Key::Tab);
        assert_eq!(prompt.user, 1);
        assert!(prompt.field.is_empty(), "a new user starts afresh");
        press(&mut prompt, Key::Left);
        assert_eq!(prompt.user, 0);
        assert_eq!(press(&mut prompt, Key::Escape), Outcome::Close);
        assert_eq!(prompt.answered, Some(Answer::Deny));
        assert_eq!(
            press(&mut prompt, Key::Enter),
            Outcome::Ignored,
            "once only"
        );
    }

    #[test]
    fn a_further_question_is_asked_in_the_field() {
        let mut prompt = prompt(password());
        prompt.phase = Phase::Checking;
        let now = Instant::now();
        prompt.message(
            Message::Said(Said::Ask {
                text: "Verification code: ".into(),
                echo: true,
            }),
            now,
        );
        assert_eq!(prompt.wanted(), Some((Shown::Text, MAX_SECRET)));
        let (reply, replies) = mpsc::channel();
        prompt.reply = Some(reply);
        type_text(&mut prompt, "123456");
        prompt.primary(now);
        assert_eq!(
            replies
                .recv()
                .unwrap()
                .as_ref()
                .map(|answer| answer.as_str()),
            Some("123456")
        );
        assert_eq!(prompt.phase, Phase::Checking);
    }

    #[test]
    fn bluetooth_codes() {
        let mut passkey = prompt(Request::Passkey {
            device: "Keyboard".into(),
        });
        type_text(&mut passkey, "12a34567");
        assert_eq!(passkey.field.text(), "123456", "digits only, six of them");
        assert_eq!(press(&mut passkey, Key::Enter), Outcome::Close);
        assert_eq!(passkey.answered, Some(Answer::Passkey(123_456)));

        let mut pin = prompt(Request::Pin {
            device: "Car".into(),
        });
        type_text(&mut pin, "0000");
        press(&mut pin, Key::Enter);
        assert_eq!(pin.answered, Some(Answer::Pin("0000".into())));

        let mut confirm = prompt(Request::Confirm {
            device: "Pixel 9".into(),
            passkey: 42,
        });
        assert_eq!(confirm.code().as_deref(), Some("000 042"));
        press(&mut confirm, Key::Enter);
        assert_eq!(confirm.answered, Some(Answer::Allow));

        let mut show = prompt(Request::Show {
            device: "Keyboard".into(),
            code: "482913".into(),
        });
        assert_eq!(
            press(&mut show, Key::Enter),
            Outcome::Ignored,
            "nothing to say yes to"
        );
        assert_eq!(show.message(Message::Close, Instant::now()), Outcome::Close);
    }

    #[test]
    fn renders_each_kind() {
        let mut fresh = prompt(password());
        fresh.field.note_mods(enso_ui::Mods {
            caps_lock: true,
            ..enso_ui::Mods::default()
        });
        render(&mut fresh, "enso-auth-password-empty");
        let mut confirm = prompt(Request::Confirm {
            device: "Sony WH-1000XM5".into(),
            passkey: 482_913,
        });
        render(&mut confirm, "enso-auth-confirm");
        let mut pin = prompt(Request::Pin {
            device: "Car kit".into(),
        });
        type_text(&mut pin, "00");
        render(&mut pin, "enso-auth-pin");
        let mut show = prompt(Request::Show {
            device: "MX Keys".into(),
            code: "039264".into(),
        });
        render(&mut show, "enso-auth-show");
        let mut service = prompt(Request::Service {
            device: "Pixel 9".into(),
            service: "the network".into(),
        });
        render(&mut service, "enso-auth-service");
    }
}
