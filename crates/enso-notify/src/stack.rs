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

/// How urgent the sender says a notification is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// A notification as received over D-Bus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub timeout: Timeout,
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
}

impl Stack {
    pub fn new(options: StackOptions, motion: Motion) -> Self {
        Self {
            options,
            motion,
            toasts: Vec::new(),
            queue: VecDeque::new(),
        }
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
        let expired: Vec<u32> = self
            .toasts
            .iter()
            .filter(|t| !t.is_leaving() && t.expires.is_some_and(|at| at <= now))
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
            .filter_map(|toast| toast.gone_at.or(toast.expires))
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
