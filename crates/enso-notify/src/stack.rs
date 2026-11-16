//! The toast stack: which notifications are showing, where each one sits,
//! and how they move. Pure state and timing — no Wayland, no D-Bus — so the
//! choreography is unit-tested.
//!
//! Oldest toasts sit at the top. A new toast slides in at the bottom; a
//! dismissed or expired one glides up toward the bar while the rest close
//! the gap.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use enso_core::{Motion, Transition, Tween};
use enso_ui::Image;

/// How urgent the sender says a notification is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

/// How long a notification stays up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Timeout {
    /// Use the configured default.
    #[default]
    Default,
    /// Stay until dismissed.
    Never,
    After(Duration),
}

/// Something the sender offers to do about a notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// What the sender calls it; sent back when it is chosen.
    pub key: String,
    /// What the button says.
    pub label: String,
}

/// The key of the action meant by activating the notification itself.
pub const DEFAULT_ACTION: &str = "default";

/// A notification as received over D-Bus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    /// Plain text; markup has been reduced already.
    pub body: String,
    pub urgency: Urgency,
    pub timeout: Timeout,
    pub actions: Vec<Action>,
    /// A picture: an avatar, album art, a screenshot's thumbnail.
    pub image: Option<Image>,
    /// A level to show as a bar, in percent — volume and brightness
    /// pop-ups send one.
    pub progress: Option<u8>,
}

impl Notification {
    /// Whether clicking the toast itself does something at the sender.
    pub fn has_default_action(&self) -> bool {
        self.actions
            .iter()
            .any(|action| action.key == DEFAULT_ACTION)
    }

    /// The actions that get a button of their own.
    pub fn buttons(&self) -> impl Iterator<Item = &Action> {
        self.actions
            .iter()
            .filter(|action| action.key != DEFAULT_ACTION && !action.label.trim().is_empty())
            .take(3)
    }
}

/// Why a notification went away, as reported in `NotificationClosed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

/// A notification on screen.
#[derive(Clone, Debug)]
pub struct Toast {
    pub note: Notification,
    /// Card height in logical pixels.
    pub height: f32,
    /// Top edge within the stack, animated as neighbours come and go.
    y: Tween<f32>,
    /// `0` invisible, `1` fully shown.
    presence: Tween<f32>,
    expires: Option<Instant>,
    /// Set once the toast starts leaving: when to drop it.
    gone_at: Option<Instant>,
}

impl Toast {
    pub fn y(&self, now: Instant) -> f32 {
        self.y.get(now)
    }

    pub fn presence(&self, now: Instant) -> f32 {
        self.presence.get(now)
    }

    pub fn is_leaving(&self) -> bool {
        self.gone_at.is_some()
    }

    fn is_animating(&self, now: Instant) -> bool {
        self.y.is_animating(now) || self.presence.is_animating(now)
    }
}

/// Stack behaviour, from the `[notify]` config table.
#[derive(Clone, Copy, Debug)]
pub struct StackOptions {
    /// Space between toasts.
    pub gap: f32,
    /// Toasts shown at once; the rest wait their turn.
    pub max_visible: usize,
    /// Lifetime of notifications that do not ask for one.
    pub default_timeout: Duration,
}

#[derive(Debug)]
pub struct Stack {
    options: StackOptions,
    motion: Motion,
    toasts: Vec<Toast>,
    /// Notifications (with their measured heights) waiting for a free slot.
    queue: VecDeque<(Notification, f32)>,
    /// Since when nothing expires: the pointer is on the toasts, so they
    /// are being read.
    paused: Option<Instant>,
}

impl Stack {
    pub fn new(options: StackOptions, motion: Motion) -> Self {
        Self {
            options,
            motion,
            toasts: Vec::new(),
            queue: VecDeque::new(),
            paused: None,
        }
    }

    /// Stop the clocks: nothing expires until [`Stack::resume`].
    pub fn pause(&mut self, now: Instant) {
        self.paused.get_or_insert(now);
    }

    /// Start the clocks again. Every toast gets back the time it had left
    /// when they stopped.
    pub fn resume(&mut self, now: Instant) {
        if let Some(since) = self.paused.take() {
            let held = now.saturating_duration_since(since);
            for toast in &mut self.toasts {
                toast.expires = toast.expires.map(|at| at + held);
            }
        }
    }

    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    /// Drop everything at once, showing and queued, without ceremony: the
    /// history card is taking the toasts' place. Returns the ids dropped.
    pub fn clear(&mut self) -> Vec<u32> {
        let ids = self
            .toasts
            .drain(..)
            .filter(|toast| !toast.is_leaving())
            .map(|toast| toast.note.id)
            .chain(self.queue.drain(..).map(|(note, _)| note.id))
            .collect();
        self.paused = None;
        ids
    }

    /// The notification with this id, if it is showing and not on its way out.
    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.toasts
            .iter()
            .find(|toast| toast.note.id == id && !toast.is_leaving())
            .map(|toast| &toast.note)
    }

    /// Show a notification whose card is `height` tall. A notification with
    /// the id of one already showing replaces it in place.
    pub fn push(&mut self, note: Notification, height: f32, now: Instant) {
        let expires = self.expiry(&note, now);
        if let Some(toast) = self
            .toasts
            .iter_mut()
            .find(|t| t.note.id == note.id && !t.is_leaving())
        {
            toast.note = note;
            toast.height = height;
            toast.expires = expires;
        } else if let Some(queued) = self
            .queue
            .iter_mut()
            .find(|(queued, _)| queued.id == note.id)
        {
            *queued = (note, height);
        } else if self.showing() < self.options.max_visible.max(1) {
            self.show(note, height, now);
        } else {
            self.queue.push_back((note, height));
        }
        self.layout(now);
    }

    /// Start dismissing a notification. Returns whether it was showing or queued.
    pub fn close(&mut self, id: u32, now: Instant) -> bool {
        if let Some(index) = self.queue.iter().position(|(note, _)| note.id == id) {
            self.queue.remove(index);
            return true;
        }
        let dismiss = self.motion.apply(Transition::DISMISS);
        let Some(toast) = self
            .toasts
            .iter_mut()
            .find(|t| t.note.id == id && !t.is_leaving())
        else {
            return false;
        };
        toast.presence.set(0.0, dismiss, now);
        toast.gone_at = Some(now + dismiss.duration);
        self.promote(now);
        self.layout(now);
        true
    }

    /// Advance time: expire what is due and drop toasts that finished
    /// leaving. Returns the ids that expired.
    pub fn tick(&mut self, now: Instant) -> Vec<u32> {
        self.toasts
            .retain(|toast| toast.gone_at.is_none_or(|at| at > now));
        let running = self.paused.is_none();
        let expired: Vec<u32> = self
            .toasts
            .iter()
            .filter(|t| running && !t.is_leaving() && t.expires.is_some_and(|at| at <= now))
            .map(|t| t.note.id)
            .collect();
        for id in &expired {
            self.close(*id, now);
        }
        self.promote(now);
        self.layout(now);
        expired
    }

    /// Time until [`Stack::tick`] next has something to do.
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.toasts
            .iter()
            .filter_map(|toast| {
                // While paused only departures in progress are due.
                toast
                    .gone_at
                    .or(toast.expires.filter(|_| self.paused.is_none()))
            })
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// Height needed to show every toast where it is now and where it is
    /// heading, so nothing is clipped while toasts glide out or close a gap.
    pub fn height(&self, now: Instant) -> f32 {
        self.toasts
            .iter()
            .map(|toast| toast.y(now).max(toast.y.target()) + toast.height)
            .fold(0.0, f32::max)
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.toasts.iter().any(|toast| toast.is_animating(now))
    }

    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// The notification under a point in stack coordinates, if any.
    pub fn hit(&self, y: f32, now: Instant) -> Option<u32> {
        self.toasts
            .iter()
            .find(|t| !t.is_leaving() && y >= t.y(now) && y < t.y(now) + t.height)
            .map(|t| t.note.id)
    }

    fn showing(&self) -> usize {
        self.toasts
            .iter()
            .filter(|toast| !toast.is_leaving())
            .count()
    }

    fn expiry(&self, note: &Notification, now: Instant) -> Option<Instant> {
        match (note.timeout, note.urgency) {
            (Timeout::Never, _) | (Timeout::Default, Urgency::Critical) => None,
            (Timeout::Default, _) => Some(now + self.options.default_timeout),
            (Timeout::After(duration), _) => Some(now + duration),
        }
    }

    fn show(&mut self, note: Notification, height: f32, now: Instant) {
        let expires = self.expiry(&note, now);
        let mut presence = Tween::new(0.0);
        presence.set(1.0, self.motion.apply(Transition::REVEAL), now);
        // Appear directly in the slot below the toasts already showing.
        let y = self.slot_after_showing();
        self.toasts.push(Toast {
            note,
            height,
            y: Tween::new(y),
            presence,
            expires,
            gone_at: None,
        });
    }

    /// Move queued notifications into free slots.
    fn promote(&mut self, now: Instant) {
        while self.showing() < self.options.max_visible.max(1) {
            let Some((note, height)) = self.queue.pop_front() else {
                break;
            };
            self.show(note, height, now);
        }
    }

    fn slot_after_showing(&self) -> f32 {
        self.toasts
            .iter()
            .filter(|toast| !toast.is_leaving())
            .map(|toast| toast.height + self.options.gap)
            .sum()
    }

    /// Give every staying toast its slot; leaving toasts keep theirs.
    fn layout(&mut self, now: Instant) {
        let reflow = self.motion.apply(Transition::REFLOW);
        let mut y = 0.0;
        for toast in self.toasts.iter_mut().filter(|toast| !toast.is_leaving()) {
            toast.y.set(y, reflow, now);
            y += toast.height + self.options.gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: f32 = 10.0;
    const H: f32 = 70.0;

    fn stack(max_visible: usize) -> Stack {
        Stack::new(
            StackOptions {
                gap: GAP,
                max_visible,
                default_timeout: Duration::from_secs(5),
            },
            Motion::default(),
        )
    }

    fn note(id: u32) -> Notification {
        Notification {
            id,
            summary: format!("note {id}"),
            ..Notification::default()
        }
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn ys(stack: &Stack, now: Instant) -> Vec<f32> {
        stack.toasts().iter().map(|t| t.y(now)).collect()
    }

    #[test]
    fn toasts_stack_downward_with_even_gaps() {
        let now = Instant::now();
        let mut stack = stack(5);
        for id in 1..=3 {
            stack.push(note(id), H, now);
        }
        assert_eq!(ys(&stack, now), [0.0, H + GAP, 2.0 * (H + GAP)]);
        assert_eq!(stack.height(now), 3.0 * H + 2.0 * GAP);
        assert_eq!(stack.hit(H + GAP + 1.0, now), Some(2));
        assert_eq!(stack.hit(H + 1.0, now), None, "the gap belongs to no toast");
    }

    #[test]
    fn new_toasts_fade_in() {
        let now = Instant::now();
        let mut stack = stack(5);
        stack.push(note(1), H, now);
        assert_eq!(stack.toasts()[0].presence(now), 0.0);
        assert!(stack.is_animating(now));
        assert_eq!(stack.toasts()[0].presence(now + ms(400)), 1.0);
        assert!(!stack.is_animating(now + ms(400)));
    }

    #[test]
    fn expired_toast_glides_out_and_the_rest_close_the_gap() {
        let t0 = Instant::now();
        let mut stack = stack(5);
        stack.push(
            Notification {
                timeout: Timeout::After(ms(1000)),
                ..note(1)
            },
            H,
            t0,
        );
        stack.push(
            Notification {
                timeout: Timeout::Never,
                ..note(2)
            },
            H,
            t0,
        );
        assert_eq!(stack.next_deadline(t0), Some(ms(1000)));

        let t1 = t0 + ms(1000);
        assert_eq!(stack.tick(t1), [1]);
        // Still on screen, fading, while its neighbour starts moving up.
        assert!(stack.toasts()[0].is_leaving());
        assert_eq!(stack.toasts().len(), 2);
        assert_eq!(stack.toasts()[1].y(t1), H + GAP);
        let mid = stack.toasts()[1].y(t1 + ms(100));
        assert!(
            mid > 0.0 && mid < H + GAP,
            "neighbour should be mid-glide, was {mid}"
        );
        assert!(stack.toasts()[0].presence(t1 + ms(100)) < 1.0);

        // The surface stays tall enough for the leaving toast until it is gone.
        assert_eq!(stack.height(t1), 2.0 * H + GAP);
        let gone = t1 + stack.next_deadline(t1).unwrap();
        assert!(stack.tick(gone).is_empty());
        assert_eq!(stack.toasts().len(), 1);
        assert_eq!(ys(&stack, gone + ms(300)), [0.0]);
        assert_eq!(stack.next_deadline(gone), None);
    }

    #[test]
    fn critical_notifications_wait_for_the_user() {
        let now = Instant::now();
        let mut stack = stack(5);
        stack.push(
            Notification {
                urgency: Urgency::Critical,
                ..note(1)
            },
            H,
            now,
        );
        stack.push(note(2), H, now);
        assert_eq!(stack.next_deadline(now), Some(Duration::from_secs(5)));
        assert_eq!(stack.tick(now + Duration::from_secs(6)), [2]);
        assert!(stack.close(1, now));
        assert!(!stack.close(1, now), "already leaving");
        assert!(!stack.close(99, now));
    }

    #[test]
    fn clearing_drops_showing_and_queued_alike() {
        let now = Instant::now();
        let mut stack = stack(2);
        for id in 1..=4 {
            stack.push(note(id), H, now);
        }
        stack.close(1, now);
        stack.pause(now);
        // 1 was already leaving and has been reported; 3 was promoted.
        assert_eq!(stack.clear(), [2, 3, 4]);
        assert!(stack.is_empty() && !stack.is_paused());
        assert_eq!(stack.next_deadline(now), None);
    }

    #[test]
    fn nothing_expires_while_paused() {
        let t0 = Instant::now();
        let mut stack = stack(5);
        stack.push(note(1), H, t0);
        assert_eq!(stack.next_deadline(t0), Some(Duration::from_secs(5)));

        // Four seconds in, the pointer arrives and stays a minute.
        let t1 = t0 + Duration::from_secs(4);
        stack.pause(t1);
        assert!(stack.is_paused());
        assert_eq!(stack.next_deadline(t1), None);
        let t2 = t1 + Duration::from_secs(60);
        assert!(stack.tick(t2).is_empty());
        // Pausing again does not restart the hold.
        stack.pause(t2);

        // It leaves: the toast has the one second it had left.
        stack.resume(t2);
        assert_eq!(stack.next_deadline(t2), Some(Duration::from_secs(1)));
        assert!(stack.tick(t2 + ms(900)).is_empty());
        assert_eq!(stack.tick(t2 + ms(1000)), [1]);

        // A dismissal still completes while paused.
        stack.push(note(2), H, t2);
        stack.pause(t2);
        assert!(stack.close(2, t2));
        assert!(stack.next_deadline(t2).is_some());
    }

    #[test]
    fn default_action_and_buttons_are_told_apart() {
        let action = |key: &str, label: &str| Action {
            key: key.into(),
            label: label.into(),
        };
        let plain = note(1);
        assert!(!plain.has_default_action());
        assert_eq!(plain.buttons().count(), 0);

        let rich = Notification {
            actions: vec![
                action("default", "Open"),
                action("reply", "Reply"),
                action("blank", " "),
                action("mute", "Mute"),
                action("a", "A"),
                action("b", "B"),
            ],
            ..note(2)
        };
        assert!(rich.has_default_action());
        let labels: Vec<&str> = rich.buttons().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Reply", "Mute", "A"]);

        let now = Instant::now();
        let mut stack = stack(5);
        stack.push(rich, H, now);
        assert!(stack.get(2).is_some());
        assert!(stack.get(3).is_none());
        stack.close(2, now);
        assert!(stack.get(2).is_none(), "leaving toasts take no more input");
    }

    #[test]
    fn overflow_queues_and_is_promoted_when_a_slot_frees() {
        let now = Instant::now();
        let mut stack = stack(2);
        for id in 1..=4 {
            stack.push(note(id), H, now);
        }
        assert_eq!(stack.showing(), 2);
        assert!(stack.close(1, now));
        let showing: Vec<u32> = stack
            .toasts()
            .iter()
            .filter(|t| !t.is_leaving())
            .map(|t| t.note.id)
            .collect();
        assert_eq!(showing, [2, 3]);
        // A queued notification can be withdrawn before it is ever shown.
        assert!(stack.close(4, now));
        assert!(stack.queue.is_empty());
    }

    #[test]
    fn same_id_replaces_in_place() {
        let now = Instant::now();
        let mut stack = stack(5);
        stack.push(note(1), H, now);
        stack.push(note(2), H, now);
        stack.push(
            Notification {
                summary: "updated".into(),
                ..note(1)
            },
            H + 20.0,
            now,
        );
        assert_eq!(stack.toasts().len(), 2);
        assert_eq!(stack.toasts()[0].note.summary, "updated");
        assert_eq!(stack.toasts()[1].y.target(), H + 20.0 + GAP);
    }
}
