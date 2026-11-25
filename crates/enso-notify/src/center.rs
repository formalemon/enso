//! The notification history: a card that opens where the toasts appear —
//! out of the bar's bell, when there is a bar along the top — and lists what
//! has come in, newest first.
//!
//! A handful of rows show at once; the rest are reached by scrolling, which
//! glides rather than steps and shows no scroll bar — the list simply fades
//! at whichever edge has more beyond it. Rows arrive one after another when
//! the card opens, and close ranks smoothly when one is removed.
//!
//! This module is the card's state, choreography, and drawing. Showing and
//! hiding the surface is the daemon's business.

use std::time::{Duration, Instant};

use enso_core::{
    Easing, Motion, Palette, Rgba, Transition, Tween,
    style::{radius, space, type_scale},
};
use enso_ui::{Align, Canvas, Nav, Rect, Text, emerge::Perch};

use crate::stack::{Notification, Urgency};

/// Height of the heading above the list.
const HEADER: f32 = 46.0;
/// Height of one notification's row.
const ROW: f32 = 68.0;
/// Space between rows.
const GAP: f32 = 6.0;
/// Space between the list and the card's sides and bottom.
const INSET: f32 = space::SM;
/// Padding inside a row.
const ROW_PAD: f32 = 10.0;
/// Side of a notification's picture.
const IMAGE: f32 = 36.0;
/// How far a row rises into place as it arrives.
const RISE: f32 = 14.0;
/// How far the card drops into place as it opens.
const DROP: f32 = 10.0;
/// Height of the fade at an edge with more beyond it.
const FADE: f32 = 18.0;
/// Scroll distance per unit the pointer reports: about a row per wheel notch.
const SCROLL_SPEED: f32 = 3.0;
/// How long after the pointer leaves the card it closes.
const LINGER: Duration = Duration::from_millis(600);

/// How the card is to look, from the daemon's theme and configuration.
#[derive(Clone, Debug)]
pub struct Look<'a> {
    pub colors: &'a Palette,
    pub accent: Rgba,
    /// Base font size.
    pub size: f32,
    /// Card width.
    pub width: f32,
    /// Where the card's top-left corner rests on the surface.
    pub origin: (f32, f32),
    /// The bar's bell, when the card grows out of it.
    pub perch: Perch,
}

/// What a click on the card asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Nothing: the heading, a gap.
    Nothing,
    /// Remove this notification from the history.
    Remove(u32),
    /// A notification whose sender is still waiting to hear: open it —
    /// what a click on its toast would have done — or just remove it.
    Open(u32),
    /// Empty the history.
    Clear,
}

#[derive(Clone, Debug)]
struct Item {
    note: Notification,
    /// The body on one line.
    body: String,
    received: Instant,
    /// How old it already was when received: the age a notification from
    /// before a restart came back with.
    before: Duration,
    /// Its sender has not been told it is over, and offers something to
    /// open: clicking the row can still do what clicking the toast did.
    live: bool,
    /// Position in the list, in rows; glides when the list changes.
    slot: Tween<f32>,
    /// When the row starts arriving.
    arrives: Instant,
}

#[derive(Debug)]
pub struct Center {
    motion: Motion,
    /// Rows shown at once.
    rows: usize,
    /// Notifications remembered.
    capacity: usize,
    /// Newest first.
    items: Vec<Item>,
    /// Notifications that went by unattended since the card was last open.
    unseen: u32,
    open: bool,
    /// The card grows out of the bar's bell, rather than dropping in.
    emerging: bool,
    /// `0` hidden, `1` fully shown.
    presence: Tween<f32>,
    /// While closing: when the card will be gone.
    gone_at: Option<Instant>,
    /// How far the list is scrolled, in pixels.
    scroll: Tween<f32>,
    /// Height of the list's viewport.
    viewport: Tween<f32>,
    pointer: Option<(f32, f32)>,
    /// The pointer has been on the card since it opened.
    visited: bool,
    /// When to close because the pointer left.
    leave_at: Option<Instant>,
    /// Geometry from the last frame, for the pointer.
    card: Rect,
    hot_rows: Vec<(Rect, u32)>,
    hot_clear: Option<Rect>,
    /// The list changed since it was last saved.
    dirty: bool,
}

/// A notification as the history keeps it across restarts: its words and
/// its age. Pictures and actions belong to the session that sent them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub urgency: Urgency,
    /// When it was received, in seconds since the Unix epoch.
    pub at: u64,
}

impl Center {
    pub fn new(rows: usize, capacity: usize, motion: Motion) -> Self {
        Self {
            motion,
            rows: rows.max(1),
            capacity: capacity.max(1),
            items: Vec::new(),
            unseen: 0,
            open: false,
            emerging: false,
            presence: Tween::new(0.0),
            gone_at: None,
            scroll: Tween::new(0.0),
            viewport: Tween::new(ROW),
            pointer: None,
            visited: false,
            leave_at: None,
            card: Rect::default(),
            hot_rows: Vec::new(),
            hot_clear: None,
            dirty: false,
        }
    }

    /// Remember a notification. One with the id of an entry already there
    /// updates it in place. Returns the ids of live notifications pushed
    /// out of a full history: their senders are owed a word.
    pub fn record(&mut self, note: Notification, now: Instant) -> Vec<u32> {
        let body = note.body.split_whitespace().collect::<Vec<_>>().join(" ");
        let live = note.has_default_action();
        self.dirty = true;
        if let Some(item) = self.items.iter_mut().find(|item| item.note.id == note.id) {
            item.note = note;
            item.body = body;
            item.received = now;
            item.before = Duration::ZERO;
            item.live = live;
            return Vec::new();
        }
        self.items.insert(
            0,
            Item {
                note,
                body,
                received: now,
                before: Duration::ZERO,
                live,
                slot: Tween::new(0.0),
                arrives: now,
            },
        );
        let dropped = self
            .items
            .drain(self.capacity.min(self.items.len())..)
            .filter(|item| item.live)
            .map(|item| item.note.id)
            .collect();
        self.reflow(now);
        dropped
    }

    /// Whether this notification's sender is still waiting to hear of it.
    pub fn is_live(&self, id: u32) -> bool {
        self.items
            .iter()
            .any(|item| item.note.id == id && item.live)
    }

    /// The sender has been told this notification is over: its row stays,
    /// as something to read.
    pub fn settle(&mut self, id: u32) {
        for item in &mut self.items {
            if item.note.id == id {
                item.live = false;
            }
        }
    }

    /// Forget one notification. Returns whether it was there.
    pub fn forget(&mut self, id: u32, now: Instant) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.note.id != id);
        let removed = self.items.len() != before;
        if removed {
            self.dirty = true;
            self.reflow(now);
        }
        removed
    }

    /// Empty the history. Returns the ids of the live notifications in it.
    pub fn clear(&mut self, now: Instant) -> Vec<u32> {
        let live = self
            .items
            .drain(..)
            .filter(|item| item.live)
            .map(|item| item.note.id)
            .collect();
        self.dirty = true;
        self.reflow(now);
        live
    }

    /// The history as it is to be kept, if it changed since last asked.
    /// `unix_now` is the wall clock at `now`.
    pub fn save(&mut self, now: Instant, unix_now: u64) -> Option<Vec<Saved>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        Some(
            self.items
                .iter()
                .map(|item| Saved {
                    app: item.note.app_name.clone(),
                    summary: item.note.summary.clone(),
                    body: item.note.body.clone(),
                    urgency: item.note.urgency,
                    at: unix_now.saturating_sub(item.age(now).as_secs()),
                })
                .collect(),
        )
    }

    /// Bring back a kept history, newest first. Ids are taken from the top
    /// of the range, where no sender's notification will be for a long time.
    pub fn restore(&mut self, saved: Vec<Saved>, now: Instant, unix_now: u64) {
        self.items = saved
            .into_iter()
            .take(self.capacity)
            .enumerate()
            .map(|(index, saved)| Item {
                body: saved.body.split_whitespace().collect::<Vec<_>>().join(" "),
                note: Notification {
                    id: u32::MAX - index as u32,
                    app_name: saved.app,
                    summary: saved.summary,
                    body: saved.body,
                    urgency: saved.urgency,
                    ..Notification::default()
                },
                received: now,
                before: Duration::from_secs(unix_now.saturating_sub(saved.at)),
                live: false,
                slot: Tween::new(index as f32),
                arrives: now,
            })
            .collect();
        self.viewport.jump(self.viewport_target());
    }

    /// Note that a notification went by without being attended to.
    pub fn missed(&mut self) {
        self.unseen = self.unseen.saturating_add(1);
    }

    /// Notifications that went by unattended since the card was last open.
    pub fn unseen(&self) -> u32 {
        self.unseen
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The card is open (and not on its way out).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The card is on screen: open, or still closing.
    pub fn is_showing(&self) -> bool {
        self.open || self.gone_at.is_some()
    }

    /// Grow the card out of the bar's bell from now on (see
    /// [`enso_ui::emerge`]), or drop it in.
    pub fn set_emerging(&mut self, emerging: bool) {
        self.emerging = emerging;
    }

    /// Open the card: everything unseen is now seen.
    pub fn open(&mut self, now: Instant) {
        if self.open {
            return;
        }
        self.open = true;
        self.gone_at = None;
        self.unseen = 0;
        self.visited = false;
        self.leave_at = None;
        self.pointer = None;
        let (reveal, before) = if self.emerging {
            // Out of the bell: the rows wait until the card is out.
            let emerge = self.motion.apply(Transition::EMERGE);
            (emerge, emerge.duration.mul_f32(0.6))
        } else {
            (self.motion.apply(Transition::REVEAL), Duration::ZERO)
        };
        self.presence.set(1.0, reveal, now);
        self.scroll.jump(0.0);
        self.viewport.jump(self.viewport_target());
        // The rows follow the card in, one after another.
        let step = self.motion.apply(Transition::REVEAL).duration / 5;
        for (index, item) in self.items.iter_mut().enumerate() {
            item.slot.jump(index as f32);
            item.arrives = now + before + step * (index.min(self.rows) as u32 + 1);
        }
    }

    pub fn close(&mut self, now: Instant) {
        if !self.open {
            return;
        }
        self.open = false;
        self.leave_at = None;
        let dismiss = self.motion.apply(if self.emerging {
            Transition::RETURN
        } else {
            Transition::DISMISS
        });
        self.presence.set(0.0, dismiss, now);
        self.gone_at = Some(now + dismiss.duration);
    }

    /// Advance time. Returns `true` on the tick the card finishes closing.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.leave_at.is_some_and(|at| at <= now) {
            self.close(now);
        }
        if self.gone_at.is_some_and(|at| at <= now) {
            self.gone_at = None;
            return true;
        }
        false
    }

    /// Time until [`Center::tick`] next has something to do.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        [self.leave_at, self.gone_at]
            .into_iter()
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// Height of the surface that holds the card at its tallest, so the
    /// card can grow and shrink without the surface being resized.
    pub fn full_height(&self) -> f32 {
        HEADER + self.rows as f32 * (ROW + GAP) - GAP + INSET
    }

    /// The card as it rests, for the surface's input region.
    pub fn card_at_rest(&self, look: &Look<'_>) -> Rect {
        Rect::new(
            look.origin.0,
            look.origin.1,
            look.width,
            HEADER + self.viewport.target() + INSET,
        )
    }

    fn viewport_target(&self) -> f32 {
        match self.items.len().min(self.rows) {
            // Room for the line that says there is nothing.
            0 => ROW,
            shown => shown as f32 * (ROW + GAP) - GAP,
        }
    }

    /// The furthest the list scrolls.
    fn scroll_limit(&self) -> f32 {
        let content = self.items.len() as f32 * (ROW + GAP) - GAP;
        (content - self.viewport_target()).max(0.0)
    }

    /// Give every row its place after the list changed.
    fn reflow(&mut self, now: Instant) {
        let reflow = self.motion.apply(Transition::REFLOW);
        for (index, item) in self.items.iter_mut().enumerate() {
            item.slot.set(index as f32, reflow, now);
        }
        self.viewport.set(self.viewport_target(), reflow, now);
        let limit = self.scroll_limit();
        if self.scroll.target() > limit {
            self.scroll.set(limit, reflow, now);
        }
    }

    fn scroll_to(&mut self, target: f32, now: Instant) {
        let target = target.clamp(0.0, self.scroll_limit());
        self.scroll
            .set(target, self.motion.apply(Transition::SELECT), now);
    }

    /// The pointer scrolled by `dy`. Returns whether the list will move.
    pub fn on_scroll(&mut self, dy: f32, now: Instant) -> bool {
        let before = self.scroll.target();
        self.scroll_to(before + dy * SCROLL_SPEED, now);
        self.scroll.target() != before
    }

    /// A navigation key: one row at a time.
    pub fn on_nav(&mut self, nav: Nav, now: Instant) {
        let step = match nav {
            Nav::Down => ROW + GAP,
            Nav::Up => -(ROW + GAP),
        };
        self.scroll_to(self.scroll.target() + step, now);
    }

    pub fn on_pointer(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
        self.visited = true;
        self.leave_at = None;
    }

    /// The pointer left the card. Having been looked at, it closes shortly.
    pub fn on_leave(&mut self, now: Instant) {
        self.pointer = None;
        if self.open && self.visited {
            self.leave_at = Some(now + LINGER);
        }
    }

    pub fn on_press(&self, x: f32, y: f32) -> Click {
        if self.hot_clear.is_some_and(|rect| rect.contains(x, y)) {
            return Click::Clear;
        }
        self.hot_rows
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .map_or(Click::Nothing, |(_, id)| {
                if self.is_live(*id) {
                    Click::Open(*id)
                } else {
                    Click::Remove(*id)
                }
            })
    }

    /// How far a row that started arriving at `arrives` has come: `0..=1`.
    fn arrival(&self, arrives: Instant, now: Instant) -> f32 {
        let reveal = self.motion.apply(Transition::REVEAL);
        if reveal.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(arrives);
        Easing::OutCubic.apply(elapsed.as_secs_f32() / reveal.duration.as_secs_f32())
    }

    /// Draw the card. Returns whether anything is still moving.
    pub fn draw(&mut self, canvas: &mut Canvas<'_>, look: &Look<'_>, now: Instant) -> bool {
        let colors = look.colors;
        let presence = self.presence.get(now);
        let list_height = self.viewport.get(now);
        let scroll = self.scroll.get(now);
        let small = look.size * type_scale::SMALL;

        let rest = Rect::new(
            look.origin.0,
            look.origin.1,
            look.width,
            HEADER + list_height + INSET,
        );
        let (card, presence) = if self.emerging && look.perch.island.is_some() {
            // Out of the bell; what it holds shows once it is out.
            let card = look.perch.card(canvas, rest, presence, colors);
            (card, look.perch.shown(presence))
        } else {
            let card = rest.translate(0.0, (presence - 1.0) * DROP);
            canvas.set_opacity(presence);
            canvas.card(card, colors.surface, colors.highlight_med);
            (card, presence)
        };
        self.card = card;

        // Heading, and the way to empty the list.
        let header = Rect::new(card.x, card.y, card.w, HEADER).inset_xy(space::LG, 0.0);
        canvas.text_in(
            &Text::new("Notifications", look.size, colors.text).medium(),
            header,
            Align::Start,
        );
        self.hot_clear = None;
        if !self.items.is_empty() {
            let label = Text::new("Clear", small, colors.muted);
            let width = canvas.measure(&label).w;
            let button = Rect::new(header.right() - width, header.y, width, header.h)
                .inset_xy(-space::SM, 10.0);
            let hovered = self.pointer.is_some_and(|(x, y)| button.contains(x, y));
            if hovered {
                canvas.fill(button, radius::CONTROL, colors.overlay);
            }
            let tint = if hovered { colors.text } else { colors.muted };
            canvas.text_in(&Text::new("Clear", small, tint), button, Align::Center);
            self.hot_clear = Some(button);
        }

        let viewport = Rect::new(
            card.x + INSET,
            card.y + HEADER,
            card.w - 2.0 * INSET,
            list_height,
        );
        self.hot_rows.clear();
        let mut moving = false;
        if self.items.is_empty() {
            canvas.text_in(
                &Text::new("Nothing has come in", small, colors.muted),
                viewport,
                Align::Center,
            );
        } else {
            // Where each row is, and how far it has arrived.
            let placed: Vec<(usize, Rect, f32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let arrived = self.arrival(item.arrives, now);
                    moving |= arrived < 1.0 || item.slot.is_animating(now);
                    let y = viewport.y + item.slot.get(now) * (ROW + GAP) - scroll
                        + (1.0 - arrived) * RISE;
                    let row = Rect::new(viewport.x, y, viewport.w, ROW);
                    (row.bottom() > viewport.y && row.y < viewport.bottom())
                        .then_some((index, row, arrived))
                })
                .collect();
            let pointer = self.pointer.filter(|(x, y)| viewport.contains(*x, *y));
            canvas.with_clip(viewport, |canvas| {
                for (index, row, arrived) in &placed {
                    // Not yet arrived: nothing of it shows.
                    if *arrived <= 0.0 {
                        continue;
                    }
                    let hovered = pointer.is_some_and(|(x, y)| row.contains(x, y));
                    canvas.set_opacity(presence * arrived);
                    draw_row(canvas, &self.items[*index], *row, hovered, look, now);
                }
            });
            canvas.set_opacity(presence);
            // Only the part of a row inside the viewport takes clicks.
            for (index, row, _) in placed {
                let top = row.y.max(viewport.y);
                let visible =
                    Rect::new(row.x, top, row.w, row.bottom().min(viewport.bottom()) - top);
                self.hot_rows.push((visible, self.items[index].note.id));
            }

            // No scroll bar: the list fades toward whatever lies beyond.
            let limit = self.scroll_limit();
            let strips = 6;
            let strip = FADE / strips as f32;
            for step in 0..strips {
                let cover = colors
                    .surface
                    .alpha(0.92 * (1.0 - step as f32 / strips as f32));
                let offset = step as f32 * strip;
                if scroll > 0.5 {
                    let band = Rect::new(viewport.x, viewport.y + offset, viewport.w, strip);
                    canvas.fill(band, 0.0, cover);
                }
                if scroll < limit - 0.5 {
                    let y = viewport.bottom() - offset - strip;
                    canvas.fill(Rect::new(viewport.x, y, viewport.w, strip), 0.0, cover);
                }
            }
        }
        canvas.set_opacity(1.0);

        moving
            || self.presence.is_animating(now)
            || self.viewport.is_animating(now)
            || self.scroll.is_animating(now)
    }
}

impl Item {
    fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.received) + self.before
    }
}

fn draw_row(
    canvas: &mut Canvas<'_>,
    item: &Item,
    row: Rect,
    hovered: bool,
    look: &Look<'_>,
    now: Instant,
) {
    let colors = look.colors;
    let note = &item.note;
    let small = look.size * type_scale::SMALL;
    let fill = if hovered {
        colors.overlay
    } else {
        colors.overlay.alpha(0.55)
    };
    canvas.fill(row, radius::CONTROL, fill);

    let content = row.inset_xy(ROW_PAD, 0.0);
    let mut x = content.x;
    if let Some(image) = &note.image {
        let picture = Rect::new(x, row.y + (ROW - IMAGE) / 2.0, IMAGE, IMAGE);
        canvas.image(image, picture, radius::CONTROL);
        x += IMAGE + ROW_PAD;
    }
    let width = content.right() - x;

    let caption = Text::new(&note.app_name, small, colors.subtle);
    let summary = Text::new(&note.summary, look.size, colors.text)
        .medium()
        .max_width(width);
    let body = Text::new(&item.body, small, colors.muted).max_width(width);
    let mut block = caption.line_height() + summary.line_height();
    if !item.body.is_empty() {
        block += body.line_height();
    }
    let mut y = row.y + (ROW - block) / 2.0;

    // First line: who it is from, and how long ago.
    let line = caption.line_height();
    // A row that can still be opened says so under the pointer.
    let age = if hovered && item.live {
        "Open".to_owned()
    } else {
        ago(item.age(now))
    };
    let age_width = canvas
        .text_in(
            &Text::new(&age, small, colors.muted),
            Rect::new(x, y, width, line),
            Align::End,
        )
        .w;
    let dot = match note.urgency {
        Urgency::Critical => colors.love,
        Urgency::Normal => look.accent,
        Urgency::Low => colors.muted,
    };
    canvas.circle(x + 3.0, y + line / 2.0, 3.0, dot);
    if !note.app_name.is_empty() {
        let room = width - 12.0 - age_width - space::SM;
        canvas.text_in(&caption, Rect::new(x + 12.0, y, room, line), Align::Start);
    }
    y += line;
    canvas.text(&summary, x, y);
    y += summary.line_height();
    if !item.body.is_empty() {
        canvas.text(&body, x, y);
    }
}

/// How long ago, in as few characters as say it.
fn ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..60 => "now".to_owned(),
        secs @ 60..3600 => format!("{}m", secs / 60),
        secs @ 3600..86_400 => format!("{}h", secs / 3600),
        secs => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u32) -> Notification {
        Notification {
            id,
            app_name: "Messages".into(),
            summary: format!("note {id}"),
            body: "first line\nsecond   line".into(),
            ..Notification::default()
        }
    }

    fn center(rows: usize) -> Center {
        Center::new(rows, 50, Motion::default())
    }

    fn ids(center: &Center) -> Vec<u32> {
        center.items.iter().map(|item| item.note.id).collect()
    }

    fn secs(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    fn openable(id: u32) -> Notification {
        Notification {
            actions: vec![crate::stack::Action {
                key: crate::stack::DEFAULT_ACTION.into(),
                label: "Open".into(),
            }],
            ..note(id)
        }
    }

    /// A notification that offers something to open stays live in the
    /// history until its sender has been told it is over.
    #[test]
    fn rows_that_can_be_opened_are_live() {
        let now = Instant::now();
        let mut center = Center::new(3, 2, Motion::default());
        assert!(center.record(openable(1), now).is_empty());
        assert!(center.record(note(2), now).is_empty());
        assert!(center.is_live(1));
        assert!(!center.is_live(2));
        // Pushed out of a full history: its sender is owed a word.
        assert_eq!(center.record(openable(3), now), [1]);
        // Dismissed as a toast: the row stays, only to be read.
        center.settle(3);
        assert!(!center.is_live(3));
        center.record(openable(4), now);
        assert_eq!(center.clear(now), [4]);
        assert!(ids(&center).is_empty());
    }

    #[test]
    fn the_history_survives_a_restart() {
        let now = Instant::now();
        let unix = 1_800_000_000;
        let mut center = center(3);
        center.record(note(1), now - secs(7200));
        center.record(openable(2), now - secs(60));
        let saved = center.save(now, unix).unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!((saved[0].at, saved[1].at), (unix - 60, unix - 7200));
        assert_eq!(saved[1].summary, "note 1");
        // Unchanged since: nothing to write.
        assert_eq!(center.save(now, unix), None);

        // Through the file format and into a new daemon, ten minutes on.
        #[derive(serde::Serialize, serde::Deserialize)]
        struct File {
            note: Vec<Saved>,
        }
        let text = toml::to_string(&File { note: saved }).unwrap();
        let saved = toml::from_str::<File>(&text).unwrap().note;
        let later = Instant::now();
        let mut restored = center_of(3);
        restored.restore(saved, later, unix + 600);
        let ages: Vec<String> = restored
            .items
            .iter()
            .map(|item| ago(item.age(later)))
            .collect();
        assert_eq!(ages, ["11m", "2h"]);
        assert_eq!(restored.items[0].note.summary, "note 2");
        // Whoever sent them is gone: nothing to open, and no id a new
        // notification will be given.
        assert!(ids(&restored).iter().all(|id| *id > u32::MAX - 10));
        assert!(!restored.is_live(ids(&restored)[0]));
        assert_eq!(restored.save(later, unix + 600), None);
    }

    fn center_of(rows: usize) -> Center {
        center(rows)
    }

    #[test]
    fn newest_first_updates_in_place_and_forgets_the_oldest() {
        let now = Instant::now();
        let mut center = Center::new(3, 4, Motion::default());
        for id in 1..=4 {
            center.record(note(id), now);
        }
        assert_eq!(ids(&center), [4, 3, 2, 1]);
        // The body is flattened to one line.
        assert_eq!(center.items[0].body, "first line second line");

        // A replacement keeps its place.
        center.record(
            Notification {
                summary: "updated".into(),
                ..note(2)
            },
            now,
        );
        assert_eq!(ids(&center), [4, 3, 2, 1]);
        assert_eq!(center.items[2].note.summary, "updated");

        // Past capacity the oldest goes.
        center.record(note(5), now);
        assert_eq!(ids(&center), [5, 4, 3, 2]);

        assert!(center.forget(3, now));
        assert!(!center.forget(3, now));
        assert_eq!(ids(&center), [5, 4, 2]);
        center.clear(now);
        assert_eq!(center.len(), 0);
    }

    #[test]
    fn opening_marks_everything_seen() {
        let now = Instant::now();
        let mut center = center(5);
        center.record(note(1), now);
        center.missed();
        center.missed();
        assert_eq!(center.unseen(), 2);
        center.open(now);
        assert!(center.is_open() && center.is_showing());
        assert_eq!(center.unseen(), 0);

        // Closing takes a moment; then it is gone, exactly once.
        center.close(now);
        assert!(!center.is_open() && center.is_showing());
        let gone = now + center.next_deadline(now).unwrap();
        assert!(!center.tick(now));
        assert!(center.tick(gone));
        assert!(!center.is_showing());
        assert!(!center.tick(gone));
    }

    #[test]
    fn the_card_fits_its_rows_up_to_the_limit() {
        let now = Instant::now();
        let mut center = center(3);
        // Empty: room for one line of text.
        assert_eq!(center.viewport_target(), ROW);
        assert_eq!(center.scroll_limit(), 0.0);
        center.record(note(1), now);
        center.record(note(2), now);
        assert_eq!(center.viewport_target(), 2.0 * ROW + GAP);
        assert_eq!(center.scroll_limit(), 0.0);
        for id in 3..=7 {
            center.record(note(id), now);
        }
        // Three show; four more are a scroll away.
        assert_eq!(center.viewport_target(), 3.0 * ROW + 2.0 * GAP);
        assert_eq!(center.scroll_limit(), 4.0 * (ROW + GAP));
        assert_eq!(center.full_height(), HEADER + 3.0 * ROW + 2.0 * GAP + INSET);
    }

    #[test]
    fn scrolling_glides_and_stops_at_the_ends() {
        let t0 = Instant::now();
        let mut center = center(3);
        for id in 1..=7 {
            center.record(note(id), t0);
        }
        center.open(t0);
        let limit = center.scroll_limit();

        // Nothing above the top.
        assert!(!center.on_scroll(-15.0, t0));
        // A notch moves the list — over time, not at once.
        assert!(center.on_scroll(15.0, t0));
        assert_eq!(center.scroll.target(), 15.0 * SCROLL_SPEED);
        assert_eq!(center.scroll.get(t0), 0.0);
        let mid = center.scroll.get(t0 + Duration::from_millis(60));
        assert!(
            mid > 0.0 && mid < 15.0 * SCROLL_SPEED,
            "mid-glide, was {mid}"
        );
        assert_eq!(center.scroll.get(t0 + secs(1)), 15.0 * SCROLL_SPEED);

        // Keys move a row at a time; the end is the end.
        center.scroll_to(0.0, t0);
        center.on_nav(Nav::Down, t0);
        assert_eq!(center.scroll.target(), ROW + GAP);
        for _ in 0..20 {
            center.on_nav(Nav::Down, t0);
        }
        assert_eq!(center.scroll.target(), limit);
        assert!(!center.on_scroll(15.0, t0));

        // Removing rows while scrolled to the end pulls the list back.
        center.forget(1, t0);
        center.forget(2, t0);
        assert_eq!(center.scroll.target(), center.scroll_limit());
        assert_eq!(center.scroll_limit(), 2.0 * (ROW + GAP));
    }

    #[test]
    fn rows_arrive_one_after_another_and_close_ranks() {
        let t0 = Instant::now();
        let mut center = center(3);
        for id in 1..=3 {
            center.record(note(id), t0);
        }
        center.open(t0);
        let arrivals: Vec<Instant> = center.items.iter().map(|item| item.arrives).collect();
        assert!(arrivals[0] > t0 && arrivals[0] < arrivals[1] && arrivals[1] < arrivals[2]);
        assert_eq!(center.arrival(arrivals[1], arrivals[1]), 0.0);
        assert_eq!(center.arrival(arrivals[1], t0 + secs(2)), 1.0);

        // The middle row goes; the last glides up into its place.
        let t1 = t0 + secs(2);
        center.forget(2, t1);
        let last = &center.items[1];
        assert_eq!(last.slot.get(t1), 2.0);
        let mid = last.slot.get(t1 + Duration::from_millis(80));
        assert!(mid < 2.0 && mid > 1.0, "mid-glide, was {mid}");
        assert_eq!(last.slot.get(t1 + secs(1)), 1.0);

        // One arriving while the card is open takes the top; the rest make room.
        center.record(note(9), t1);
        assert_eq!(ids(&center), [9, 3, 1]);
        assert_eq!(center.items[0].slot.get(t1), 0.0);
        assert_eq!(center.items[1].slot.target(), 1.0);
    }

    #[test]
    fn leaving_closes_only_after_a_visit() {
        let now = Instant::now();
        let mut center = center(3);
        center.open(now);
        // Opened from the bar, the pointer has not been here yet.
        center.on_leave(now);
        assert_eq!(center.next_deadline(now), None);

        center.on_pointer(40.0, 40.0);
        center.on_leave(now);
        assert_eq!(center.next_deadline(now), Some(LINGER));
        // Coming back cancels it.
        center.on_pointer(40.0, 40.0);
        assert_eq!(center.next_deadline(now), None);
        center.on_leave(now);
        assert!(!center.tick(now + LINGER));
        assert!(!center.is_open() && center.is_showing());
    }

    #[test]
    fn ages_are_short() {
        assert_eq!(ago(secs(0)), "now");
        assert_eq!(ago(secs(59)), "now");
        assert_eq!(ago(secs(60)), "1m");
        assert_eq!(ago(secs(3599)), "59m");
        assert_eq!(ago(secs(7200)), "2h");
        assert_eq!(ago(secs(3 * 86_400 + 5)), "3d");
    }
}
