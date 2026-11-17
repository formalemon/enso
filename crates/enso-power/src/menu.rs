//! The power menu overlay: a short row of choices, one keypress each.
//!
//! The key for each is a letter of its own name, marked in the name itself
//! in the choice's colour: the L of Lock, the o of Log out. Landing on a
//! choice sends a glint of light across its letter, and a small sparkle
//! twinkles at its shoulder — once, then it rests.

use std::{
    f32::consts::PI,
    os::unix::net::UnixListener,
    thread,
    time::{Duration, Instant},
};

use enso_core::{
    Context, Easing, Rgba, Transition, Tween,
    style::{Shadow, opacity, radius, space, stroke, type_scale},
};
use enso_ui::{
    Align, App, Button, Canvas, Cx, Event, Key, KeyEvent, Nav, Paint, PointerEvent, Presence,
    Proxy, Rect, SurfaceSpec, Text,
};
use serde::Deserialize;

use crate::action::{Action, Error};

/// The `[power]` table of `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PowerConfig {
    /// The choices shown, left to right.
    pub actions: Vec<Action>,
    /// Ask for a second press before logging out, restarting, or shutting
    /// down — the three that take unsaved work with them.
    pub confirm: bool,
    /// The program that locks the screen, with its arguments.
    pub lock_command: Vec<String>,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            actions: vec![
                Action::Lock,
                Action::Logout,
                Action::Restart,
                Action::Shutdown,
            ],
            confirm: true,
            lock_command: vec!["enso-lock".to_owned()],
        }
    }
}

const TILE_WIDTH: f32 = 104.0;
/// The disc and the name, with as much room below as above.
const TILE_HEIGHT: f32 = 112.0;
/// Height of the line the name sits on.
const LABEL_HEIGHT: f32 = 20.0;
/// How long the glint takes to cross a letter.
const GLINT: Duration = Duration::from_millis(900);
/// Diameter of the tinted disc behind each icon.
const DISC: f32 = 52.0;
const CAPTION_HEIGHT: f32 = 28.0;

/// Messages from the background thread.
#[derive(Debug)]
pub enum Message {
    /// The menu was invoked again: that closes this one.
    Dismiss,
}

/// What handling an input comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// Nothing changed.
    Ignored,
    Redraw,
    /// Ease out, then exit.
    Close,
    /// Exit without ceremony.
    Exit,
}

/// How an action is carried out; tests put a recorder here.
type Runner = fn(Action, &[String]) -> Result<(), Error>;

#[derive(Debug)]
pub struct Menu {
    ctx: Context,
    config: PowerConfig,
    presence: Presence,
    run: Runner,
    /// Where a second invocation knocks to close this one.
    toggle: Option<UnixListener>,
    selected: usize,
    /// The choice waiting for its second press.
    armed: Option<usize>,
    /// Why the last choice could not be carried out.
    problem: Option<String>,
    /// Animated position of the selection highlight, in tiles.
    highlight: Tween<f32>,
    /// Animated height of the line under the tiles.
    caption: Tween<f32>,
    /// The key for each action: a letter of its name.
    keys: Vec<Option<char>>,
    /// When the glint last set out across the selected choice's letter.
    glint: Option<Instant>,
    /// Geometry from the last frame, for hit-testing.
    card: Rect,
    tiles: Vec<Rect>,
}

impl Menu {
    pub fn new(ctx: Context, config: PowerConfig, toggle: Option<UnixListener>) -> Self {
        Self::with_runner(ctx, config, toggle, |action, lock| action.run(lock))
    }

    fn with_runner(
        ctx: Context,
        mut config: PowerConfig,
        toggle: Option<UnixListener>,
        run: Runner,
    ) -> Self {
        // A menu of nothing is no menu; and one of each is enough.
        let mut seen = Vec::new();
        config.actions.retain(|action| {
            let fresh = !seen.contains(action);
            seen.push(*action);
            fresh
        });
        if config.actions.is_empty() {
            config.actions = PowerConfig::default().actions;
        }
        Self {
            presence: Presence::new(&ctx.motion),
            keys: Action::keys(&config.actions),
            // The first choice glints as the menu arrives.
            glint: Some(Instant::now()),
            ctx,
            config,
            run,
            toggle,
            selected: 0,
            armed: None,
            problem: None,
            highlight: Tween::new(0.0),
            caption: Tween::new(0.0),
            card: Rect::default(),
            tiles: Vec::new(),
        }
    }

    fn actions(&self) -> &[Action] {
        &self.config.actions
    }

    /// The line under the tiles, when there is something to say.
    fn caption_text(&self) -> Option<String> {
        if let Some(problem) = &self.problem {
            return Some(problem.clone());
        }
        let action = self.actions()[self.armed?];
        Some(format!("Press again to {}", action.verb()))
    }

    fn sync_caption(&mut self) {
        let target = if self.caption_text().is_some() {
            CAPTION_HEIGHT
        } else {
            0.0
        };
        self.caption.set(
            target,
            self.ctx.motion.apply(Transition::REFLOW),
            Instant::now(),
        );
    }

    /// Move the selection. Looking elsewhere withdraws a pending question.
    fn select(&mut self, index: usize) {
        let index = index.min(self.actions().len() - 1);
        if index == self.selected {
            return;
        }
        self.selected = index;
        self.armed = None;
        self.problem = None;
        self.glint = Some(Instant::now());
        self.highlight.set(
            index as f32,
            self.ctx.motion.apply(Transition::SELECT),
            Instant::now(),
        );
        self.sync_caption();
    }

    fn step(&mut self, forward: bool) {
        let count = self.actions().len();
        let next = if forward {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
        self.select(next);
    }

    /// Choose the action at `index`: ask first if it is one that ends the
    /// session, otherwise — or on the second press — do it.
    fn choose(&mut self, index: usize) -> Outcome {
        self.select(index);
        let action = self.actions()[self.selected];
        self.problem = None;
        if self.config.confirm && action.is_final() && self.armed != Some(self.selected) {
            self.armed = Some(self.selected);
            self.sync_caption();
            return Outcome::Redraw;
        }
        self.armed = None;
        match (self.run)(action, &self.config.lock_command) {
            // The lock screen is about to cover everything; get out of its
            // way at once rather than fade underneath it.
            Ok(()) if action == Action::Lock => Outcome::Exit,
            Ok(()) => Outcome::Close,
            Err(err) => {
                self.problem = Some(err.to_string());
                self.sync_caption();
                Outcome::Redraw
            }
        }
    }

    fn key(&mut self, key: KeyEvent) -> Outcome {
        if let Some(nav) = key.nav(true) {
            self.step(nav == Nav::Down);
            return Outcome::Redraw;
        }
        match key.key {
            // The first Esc takes back a pending question; the next closes.
            Key::Escape if self.armed.is_some() => {
                self.armed = None;
                self.sync_caption();
            }
            Key::Escape => return Outcome::Close,
            Key::Enter | Key::Char(' ') => return self.choose(self.selected),
            Key::Left => self.step(false),
            Key::Right => self.step(true),
            Key::Tab => self.step(!key.mods.shift),
            Key::Home => self.select(0),
            Key::End => self.select(usize::MAX),
            Key::Char(c) if key.mods.is_plain() && !key.repeat => {
                let typed = c.to_ascii_lowercase();
                return match self.keys.iter().position(|key| *key == Some(typed)) {
                    Some(index) => self.choose(index),
                    None => Outcome::Ignored,
                };
            }
            _ => return Outcome::Ignored,
        }
        Outcome::Redraw
    }

    fn pointer(&mut self, event: PointerEvent) -> Outcome {
        match event {
            PointerEvent::Move { x, y } => match self.tile_at(x, y) {
                Some(index) if index != self.selected => {
                    self.select(index);
                    Outcome::Redraw
                }
                _ => Outcome::Ignored,
            },
            PointerEvent::Press {
                x,
                y,
                button: Button::Left,
            } => match self.tile_at(x, y) {
                Some(index) => self.choose(index),
                None if self.card.contains(x, y) => Outcome::Ignored,
                None => Outcome::Close,
            },
            _ => Outcome::Ignored,
        }
    }

    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        self.tiles.iter().position(|tile| tile.contains(x, y))
    }
}

impl App for Menu {
    type Message = Message;

    fn surface(&self) -> SurfaceSpec {
        SurfaceSpec::overlay("enso-power")
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Some(listener) = self.toggle.take() {
            // Any connection at all is the knock. Not being able to listen
            // only costs the toggle.
            let _ = thread::Builder::new()
                .name("enso-toggle".to_owned())
                .spawn(move || {
                    if listener.accept().is_ok() {
                        proxy.send(Message::Dismiss);
                    }
                });
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        let closing = self.presence.is_closing();
        let outcome = match event {
            Event::Wake if closing => Outcome::Exit,
            Event::Message(Message::Dismiss) => Outcome::Close,
            // Once it is leaving, the menu has stopped listening.
            Event::Key(key) if !closing => self.key(key),
            Event::Pointer(event) if !closing => self.pointer(event),
            _ => Outcome::Ignored,
        };
        match outcome {
            Outcome::Ignored => {}
            Outcome::Redraw => cx.redraw(),
            Outcome::Close => self.presence.close(cx),
            Outcome::Exit => cx.exit(),
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let colors = self.ctx.theme.colors.clone();
        let base_size = self.ctx.config.appearance.font_size;
        let bounds = canvas.bounds();

        // Dim what is behind so the card is the only thing asking for attention.
        canvas.clear(colors.base.alpha(opacity::SCRIM * presence));

        let count = self.actions().len() as f32;
        let caption_height = self.caption.get(now);
        let row_width = count * TILE_WIDTH + (count - 1.0) * space::SM;
        let card = bounds
            .centered(
                row_width + 2.0 * space::LG,
                TILE_HEIGHT + 2.0 * space::LG + caption_height,
            )
            .translate(0.0, (presence - 1.0) * space::MD);
        self.card = card;

        canvas.set_opacity(presence);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, colors.surface);
        canvas.stroke(card, radius::CARD, stroke::HAIRLINE, colors.highlight_med);

        let tile = |index: f32| {
            Rect::new(
                card.x + space::LG + index * (TILE_WIDTH + space::SM),
                card.y + space::LG,
                TILE_WIDTH,
                TILE_HEIGHT,
            )
        };

        // The highlight glides between tiles, and warms while it waits for
        // the second press.
        let armed = self.armed == Some(self.selected);
        let highlight = if armed {
            colors.love.alpha(opacity::TINT)
        } else {
            colors.overlay
        };
        canvas.fill(tile(self.highlight.get(now)), radius::PANEL, highlight);

        self.tiles.clear();
        for (index, action) in self.config.actions.iter().enumerate() {
            let rect = tile(index as f32);
            self.tiles.push(rect);
            let selected = index == self.selected;
            let tint = colors.get(action.role());

            let disc = Rect::new(
                rect.x + (rect.w - DISC) / 2.0,
                rect.y + space::LG,
                DISC,
                DISC,
            );
            canvas.pill(disc, tint.alpha(opacity::TINT + 0.06));
            // About half the disc, as on any round button: room all round.
            let glyph = if canvas.glyphs() {
                (DISC * 0.5).round()
            } else {
                base_size
            };
            canvas.icon(action.icon(), glyph, tint, disc, Align::Center);

            let label = Text::new(
                action.label(),
                base_size,
                if selected { colors.text } else { colors.subtle },
            )
            .medium();
            let label_row = Rect::new(rect.x, disc.bottom() + space::SM, rect.w, LABEL_HEIGHT);
            let mark = self.keys[index].and_then(|key| action.mark(key));
            // The letter is the choice's own colour: full where the
            // selection is, quieter elsewhere.
            let letter = if selected {
                tint
            } else {
                tint.mix(colors.subtle, 0.35)
            };
            let glint = if selected { self.glint_at(now) } else { None };
            paint_label(canvas, &label, mark, label_row, letter, glint, &colors);
        }

        // The caption arrives with the room made for it.
        if let Some(text) = self.caption_text() {
            let shown = (caption_height / CAPTION_HEIGHT).clamp(0.0, 1.0);
            let line = Rect::new(
                card.x + space::LG,
                card.bottom() - space::LG - caption_height + space::XS,
                card.w - 2.0 * space::LG,
                CAPTION_HEIGHT,
            );
            canvas.with_opacity(shown, |canvas| {
                canvas.text_in(
                    &Text::new(&text, base_size * type_scale::SMALL, colors.love),
                    line,
                    Align::Center,
                );
            });
        }
        canvas.set_opacity(1.0);

        Paint::when(
            self.presence.is_animating(now)
                || self.highlight.is_animating(now)
                || self.caption.is_animating(now)
                || self.glint_at(now).is_some(),
        )
    }
}

impl Menu {
    /// How far the glint has crossed the selected letter (`0..1`), while it
    /// is crossing. Never with motion turned off.
    fn glint_at(&self, now: Instant) -> Option<f32> {
        let length = self
            .ctx
            .motion
            .apply(Transition::new(GLINT.as_millis() as u64, Easing::Linear))
            .duration;
        let elapsed = now.saturating_duration_since(self.glint?);
        (!length.is_zero() && elapsed < length)
            .then(|| elapsed.as_secs_f32() / length.as_secs_f32())
    }
}

/// A choice's name, centred in `row`, with the letter that picks it — the
/// byte range `mark` — in its own colour. While `glint` (`0..1`) is under
/// way, a band of light crosses that letter, and a small four-pointed
/// sparkle grows and fades at its shoulder.
fn paint_label(
    canvas: &mut Canvas<'_>,
    label: &Text<'_>,
    mark: Option<std::ops::Range<usize>>,
    row: Rect,
    letter: Rgba,
    glint: Option<f32>,
    colors: &enso_core::Palette,
) {
    let Some(mark) = mark else {
        canvas.text_in(label, row, Align::Center);
        return;
    };
    let words = label.text;
    let (before, key, after) = (
        &words[..mark.start],
        &words[mark.clone()],
        &words[mark.end..],
    );
    let width = |canvas: &mut Canvas<'_>, text: &str| canvas.measure(&Text { text, ..*label }).w;
    // Each piece starts where the name up to it ends, so the spacing is the
    // name's own.
    let whole = width(canvas, words);
    let start = row.x + (row.w - whole) / 2.0;
    let to_key = width(canvas, before);
    let through_key = width(canvas, &words[..mark.end]);
    let piece = |canvas: &mut Canvas<'_>, text: &str, x: f32, w: f32, color: Rgba| {
        if !text.is_empty() {
            let text = Text {
                text,
                color,
                ..*label
            };
            canvas.text_in(&text, Rect::new(x, row.y, w + 2.0, row.h), Align::Start);
        }
    };
    piece(canvas, before, start, to_key, label.color);
    piece(
        canvas,
        after,
        start + through_key,
        whole - through_key,
        label.color,
    );
    let cell = Rect::new(start + to_key, row.y, through_key - to_key, row.h);
    piece(canvas, key, cell.x, cell.w, letter);

    let Some(t) = glint else {
        return;
    };
    // The light: a narrow band, brighter than the letter, swept across it
    // and seen only where the letter is.
    let shine = letter.mix(colors.text, 0.7);
    let band = (cell.w * 0.55).max(3.0);
    let eased = 1.0 - (1.0 - t).powi(3);
    let centre = cell.x - band + eased * (cell.w + 2.0 * band);
    canvas.with_clip(
        Rect::new(centre - band / 2.0, row.y, band, row.h),
        |canvas| {
            piece(canvas, key, cell.x, cell.w, shine);
        },
    );
    // The sparkle: it swells and fades over the glint, turning a little.
    let size = (PI * t).sin();
    if size > 0.01 {
        let (x, y) = (cell.right() + 1.5, row.y + row.h * 0.22);
        let (long, short) = (3.6 * size, 1.0 * size);
        let turn = t * 0.6;
        let points: Vec<(f32, f32)> = (0..8)
            .map(|i| {
                let reach = if i % 2 == 0 { long } else { short };
                let angle = turn + i as f32 * PI / 4.0;
                (x + reach * angle.cos(), y + reach * angle.sin())
            })
            .collect();
        canvas.polygon(&points, shine.alpha(size.min(1.0)));
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, time::Duration};

    use enso_core::Paths;
    use enso_ui::{Mods, Offscreen, TextEngine};

    use super::*;

    thread_local! {
        /// What the recording runner was asked to do, and whether it refuses.
        static RAN: RefCell<Vec<Action>> = const { RefCell::new(Vec::new()) };
        static REFUSE: RefCell<bool> = const { RefCell::new(false) };
    }

    fn record(action: Action, _: &[String]) -> Result<(), Error> {
        if REFUSE.with(|refuse| *refuse.borrow()) {
            return Action::Lock.run(&[]);
        }
        RAN.with(|ran| ran.borrow_mut().push(action));
        Ok(())
    }

    fn ran() -> Vec<Action> {
        RAN.with(|ran| ran.borrow().clone())
    }

    fn menu(config: PowerConfig) -> Menu {
        RAN.with(|ran| ran.borrow_mut().clear());
        REFUSE.with(|refuse| *refuse.borrow_mut() = false);
        let ctx = Context::load_from(Paths::rooted("/nonexistent/enso-power-test"));
        Menu::with_runner(ctx, config, None, record)
    }

    fn press(menu: &mut Menu, key: Key) -> Outcome {
        menu.key(KeyEvent {
            key,
            mods: Mods::default(),
            repeat: false,
        })
    }

    #[test]
    fn example_config_documents_the_defaults() {
        let example = include_str!("../../../config.example.toml");
        let config = enso_core::Config::parse(example, "config.example.toml").unwrap();
        assert_eq!(
            config.section::<PowerConfig>("power").unwrap(),
            PowerConfig::default()
        );
    }

    #[test]
    fn locking_is_one_press_and_shutting_down_is_two() {
        let mut menu = menu(PowerConfig::default());

        // `s` goes to "Shut down" and asks; nothing has happened yet.
        press(&mut menu, Key::Char('s'));
        assert_eq!(menu.selected, 3);
        assert_eq!(menu.armed, Some(3));
        assert_eq!(
            menu.caption_text().as_deref(),
            Some("Press again to shut down")
        );
        assert!(ran().is_empty());

        // Looking elsewhere withdraws the question.
        press(&mut menu, Key::Left);
        assert_eq!((menu.selected, menu.armed), (2, None));
        // So does Esc, without closing the menu.
        press(&mut menu, Key::Enter);
        assert_eq!(menu.armed, Some(2));
        assert_eq!(press(&mut menu, Key::Escape), Outcome::Redraw);
        assert!(menu.armed.is_none() && ran().is_empty());
        // With nothing pending, Esc closes.
        assert_eq!(press(&mut menu, Key::Escape), Outcome::Close);

        // Twice does it.
        assert_eq!(press(&mut menu, Key::Char('s')), Outcome::Redraw);
        assert_eq!(press(&mut menu, Key::Char('s')), Outcome::Close);
        assert_eq!(ran(), [Action::Shutdown]);

        // Lock needs no second press.
        let mut menu = self::menu(PowerConfig::default());
        assert_eq!(press(&mut menu, Key::Char('L')), Outcome::Exit);
        assert_eq!(ran(), [Action::Lock]);
    }

    /// The marked letters are the keys: `o` for Log out, which asks first.
    #[test]
    fn the_marked_letter_is_the_key() {
        let mut menu = menu(PowerConfig::default());
        assert_eq!(menu.keys, [Some('l'), Some('o'), Some('r'), Some('s')]);
        assert_eq!(press(&mut menu, Key::Char('o')), Outcome::Redraw);
        assert_eq!((menu.selected, menu.armed), (1, Some(1)));
        // The old hint keys mean nothing now.
        assert_eq!(press(&mut menu, Key::Char('e')), Outcome::Ignored);
        assert!(ran().is_empty());
    }

    /// Landing on a choice sends the glint across its letter once; then all
    /// is still. With motion off there is no glint at all.
    #[test]
    fn the_glint_crosses_once_on_arrival() {
        let mut menu = menu(PowerConfig::default());
        let now = Instant::now();
        assert!(menu.glint_at(now).is_some(), "it glints as the menu opens");
        assert_eq!(menu.glint_at(now + GLINT), None);
        press(&mut menu, Key::Right);
        let landed = menu.glint.unwrap();
        assert!(
            menu.glint_at(landed + GLINT / 2)
                .is_some_and(|t| (0.4..0.6).contains(&t))
        );
        assert_eq!(menu.glint_at(landed + GLINT), None);

        menu.ctx.motion = enso_core::Motion::none();
        press(&mut menu, Key::Right);
        assert_eq!(menu.glint_at(Instant::now()), None);
    }

    #[test]
    fn navigation_wraps_and_confirmation_can_be_turned_off() {
        let mut menu = menu(PowerConfig {
            confirm: false,
            ..PowerConfig::default()
        });
        press(&mut menu, Key::Char('k'));
        assert_eq!(menu.selected, 3);
        press(&mut menu, Key::Char('j'));
        assert_eq!(menu.selected, 0);
        press(&mut menu, Key::Right);
        press(&mut menu, Key::Tab);
        assert_eq!(menu.selected, 2);
        // A key for nothing on the menu does nothing.
        assert_eq!(press(&mut menu, Key::Char('z')), Outcome::Ignored);
        assert!(ran().is_empty());
        assert_eq!(press(&mut menu, Key::Enter), Outcome::Close);
        assert_eq!(ran(), [Action::Restart]);
    }

    #[test]
    fn a_refusal_is_shown_and_the_menu_stays() {
        let mut menu = menu(PowerConfig::default());
        REFUSE.with(|refuse| *refuse.borrow_mut() = true);
        assert_eq!(press(&mut menu, Key::Enter), Outcome::Redraw);
        assert!(menu.problem.is_some() && ran().is_empty());

        // A click on a tile chooses it; one outside the card closes.
        let mut menu = self::menu(PowerConfig::default());
        let mut text = TextEngine::new(&menu.ctx.config.appearance);
        Offscreen::render(&mut text, (900, 420), 1, |canvas| {
            menu.draw(canvas, Instant::now());
        });
        let (x, y) = menu.tiles[0].center();
        let click = |x, y| PointerEvent::Press {
            x,
            y,
            button: Button::Left,
        };
        assert_eq!(menu.pointer(click(x, y)), Outcome::Exit);
        assert_eq!(ran(), [Action::Lock]);
        assert_eq!(menu.pointer(click(4.0, 4.0)), Outcome::Close);
    }

    #[test]
    fn an_empty_or_repetitive_list_is_repaired() {
        let empty = menu(PowerConfig {
            actions: Vec::new(),
            ..PowerConfig::default()
        });
        assert_eq!(empty.actions(), PowerConfig::default().actions);
        let doubled = menu(PowerConfig {
            actions: vec![Action::Lock, Action::Suspend, Action::Lock],
            ..PowerConfig::default()
        });
        assert_eq!(doubled.actions(), [Action::Lock, Action::Suspend]);
    }

    #[test]
    fn renders_the_menu() {
        let mut menu = menu(PowerConfig {
            actions: Action::ALL.to_vec(),
            ..PowerConfig::default()
        });
        let mut text = TextEngine::new(&menu.ctx.config.appearance);
        let start = Instant::now();
        let settled = start + Duration::from_secs(1);
        let mut draw = |menu: &mut Menu, now| {
            Offscreen::render(&mut text, (900, 420), 2, |canvas| {
                menu.draw(canvas, now);
            })
        };
        // The first frame starts the reveal; a second later it has settled.
        draw(&mut menu, start);
        let frame = draw(&mut menu, settled);
        let surface = menu.ctx.theme.colors.surface;
        let card = menu.card;
        assert_eq!(menu.tiles.len(), 5);
        assert!(card.w > 5.0 * TILE_WIDTH && card.h < 200.0);
        let inside = frame.pixel(
            ((card.x + 6.0) * 2.0) as u32,
            ((card.y + card.h / 2.0) * 2.0) as u32,
        );
        assert_eq!(inside, [surface.r, surface.g, surface.b, 255]);
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/power.png"
            ))
            .unwrap();

        // Armed: the card grows a line to ask on.
        assert_eq!(menu.choose(4), Outcome::Redraw);
        let later = Instant::now() + Duration::from_secs(1);
        let frame = draw(&mut menu, later);
        assert!(menu.card.h > card.h + CAPTION_HEIGHT - 1.0);
        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/power-confirm.png"
            ))
            .unwrap();
    }

    /// The selected name's key letter is in its choice's colour; caught
    /// halfway, the glint is on it and the sparkle beside it.
    #[test]
    fn the_key_letter_is_marked_and_glints() {
        let mut menu = menu(PowerConfig::default());
        let mut text = TextEngine::new(&menu.ctx.config.appearance);
        let colors = menu.ctx.theme.colors.clone();
        let mut draw = |menu: &mut Menu, now| {
            Offscreen::render(&mut text, (900, 420), 2, |canvas| {
                menu.draw(canvas, now);
            })
        };
        let start = Instant::now();
        draw(&mut menu, start);
        let settled = draw(&mut menu, start + Duration::from_secs(2));
        // Somewhere in Lock's name row, the letter in foam.
        let has = |frame: &Offscreen, menu: &Menu, index: usize, color: Rgba| {
            let tile = menu.tiles[index];
            let top = tile.y + space::LG + DISC + space::SM;
            let (x0, x1) = ((tile.x * 2.0) as u32, (tile.right() * 2.0) as u32);
            let (y0, y1) = ((top * 2.0) as u32, ((top + LABEL_HEIGHT) * 2.0) as u32);
            (x0..x1)
                .any(|x| (y0..y1).any(|y| frame.pixel(x, y) == [color.r, color.g, color.b, 255]))
        };
        assert!(has(&settled, &menu, 0, colors.foam));
        settled
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/power-keys.png"
            ))
            .unwrap();

        // On to Log out; halfway through its glint, the light is on the o.
        press(&mut menu, Key::Right);
        let landed = menu.glint.unwrap();
        draw(&mut menu, landed);
        let shining = draw(&mut menu, landed + GLINT * 2 / 5);
        let shine = colors.gold.mix(colors.text, 0.7);
        assert!(has(&shining, &menu, 1, shine), "the glint is on the letter");
        shining
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/power-glint.png"
            ))
            .unwrap();
    }
}
