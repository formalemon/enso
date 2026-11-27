//! Two fingers sideways on a touchpad.
//!
//! A sideways glide of two fingers is how a touchpad says back and forward:
//! a browser's history, a gallery's next picture. [`Swipe`] tells one from
//! scrolling (it starts sideways, and stays so until the fingers lift),
//! follows it, and says when the fingers lift how far they went and how
//! fast. What that means is the interface's own; [`decide`] is the usual
//! rule.
//!
//! Directions follow the scroll's own sign, so the touchpad's
//! natural-scrolling setting carries over: with it, fingers moving left go
//! forward, as on a phone; without it, fingers moving right do.
//!
//! Two ways of showing one, so every interface shows it alike:
//!
//! - [`Pull`], for going back and forward between places (the file
//!   manager's folders): an arrow drawn in from the edge being pulled from,
//!   filling with the accent once letting go would go.
//! - [`Slide`], for stepping through things side by side (pictures, the
//!   players on the now-playing card): what's shown follows the fingers, the
//!   next one coming in beside it, and settles on whichever is meant.

use std::{
    collections::VecDeque,
    f32::consts::{FRAC_PI_2, TAU},
    time::{Duration, Instant},
};

use enso_core::{
    Motion, Transition, Tween,
    style::{icon, space, stroke},
};

use crate::{Align, Canvas, Rect, Shape, widget::Look};

/// Which way a swipe went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Towards the start: back, the one before.
    Back,
    /// Towards the end: forward, the next one.
    Forward,
}

impl Direction {
    /// `-1` back, `1` forward: a step along a list.
    pub fn step(self) -> isize {
        match self {
            Self::Back => -1,
            Self::Forward => 1,
        }
    }

    fn sign(self) -> f32 {
        match self {
            Self::Back => -1.0,
            Self::Forward => 1.0,
        }
    }
}

/// What a scroll event was, to a swipe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glide {
    /// Not a swipe's: scroll as usual.
    Scroll,
    /// A swipe moved on; draw again.
    Moved,
    /// The fingers lifted at the end of a swipe that went `travel` logical
    /// pixels (signed as the scroll), moving at `speed` pixels a second.
    Lifted { travel: f32, speed: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Not far enough yet to tell which way it's going.
    Deciding,
    Sideways,
    /// Up and down: a scroll, until the fingers lift.
    Scrolling,
}

/// How far the fingers move before it's clear which way they're going.
const SLOP: f32 = 12.0;
/// Sideways is this many times more across than up and down.
const SIDEWAYS: f32 = 1.5;
/// A glide this quiet is over, even if no lift was reported. Wake an
/// interface this long after a swipe moves, and [`Swipe::expire`] it.
pub const QUIET: Duration = Duration::from_millis(180);
/// The speed at the lift is measured over the end of the glide.
const RECENT: Duration = Duration::from_millis(90);
/// A flick: fast enough to go on a quarter of the distance.
const FLICK: f32 = 900.0;

/// One touchpad glide, followed.
#[derive(Clone, Debug)]
pub struct Swipe {
    phase: Phase,
    /// Across and down since the glide began.
    travel: (f32, f32),
    /// The latest moves across, for the speed at the lift.
    recent: VecDeque<(Instant, f32)>,
    last: Option<Instant>,
}

impl Default for Swipe {
    fn default() -> Self {
        Self::new()
    }
}

impl Swipe {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            travel: (0.0, 0.0),
            recent: VecDeque::new(),
            last: None,
        }
    }

    /// Take a scroll event, as [`crate::PointerEvent::Scroll`] has it.
    pub fn scroll(&mut self, dx: f32, dy: f32, wheel: bool, stopped: bool, now: Instant) -> Glide {
        // A wheel never swipes, and ends a glide that went unfinished.
        if wheel {
            if self.phase == Phase::Sideways {
                return self.lift(now);
            }
            self.reset();
            return Glide::Scroll;
        }
        // After a quiet spell, a new glide: the last one's lift went
        // unreported.
        if self.phase != Phase::Sideways
            && self
                .last
                .is_some_and(|last| now.saturating_duration_since(last) > QUIET)
        {
            self.reset();
        }
        self.last = Some(now);
        match self.phase {
            Phase::Idle | Phase::Deciding => {
                if stopped {
                    self.reset();
                    return Glide::Scroll;
                }
                self.phase = Phase::Deciding;
                self.travel.0 += dx;
                self.travel.1 += dy;
                self.note(dx, now);
                let (across, down) = self.travel;
                if across.hypot(down) < SLOP {
                    return Glide::Scroll;
                }
                if across.abs() > down.abs() * SIDEWAYS {
                    self.phase = Phase::Sideways;
                    Glide::Moved
                } else {
                    self.phase = Phase::Scrolling;
                    Glide::Scroll
                }
            }
            Phase::Sideways if stopped => self.lift(now),
            Phase::Sideways => {
                self.travel.0 += dx;
                self.note(dx, now);
                Glide::Moved
            }
            Phase::Scrolling => {
                if stopped {
                    self.reset();
                }
                Glide::Scroll
            }
        }
    }

    /// Whether a sideways swipe is under way.
    pub fn is_sideways(&self) -> bool {
        self.phase == Phase::Sideways
    }

    /// How far the swipe under way has gone, signed as the scroll: positive
    /// is towards [`Direction::Forward`]. `0` with none under way.
    pub fn travel(&self) -> f32 {
        if self.is_sideways() {
            self.travel.0
        } else {
            0.0
        }
    }

    /// End a glide whose fingers went quiet without a lift being reported.
    /// Returns the lift, for a sideways one.
    pub fn expire(&mut self, now: Instant) -> Option<Glide> {
        let quiet = self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) >= QUIET);
        match self.phase {
            Phase::Sideways if quiet => Some(self.lift(now)),
            Phase::Deciding | Phase::Scrolling if quiet => {
                self.reset();
                None
            }
            _ => None,
        }
    }

    fn note(&mut self, dx: f32, now: Instant) {
        self.recent.push_back((now, dx));
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > RECENT)
        {
            self.recent.pop_front();
        }
    }

    fn lift(&mut self, now: Instant) -> Glide {
        let travel = self.travel.0;
        let recent = self
            .recent
            .iter()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= RECENT);
        let (mut moved, mut first) = (0.0, now);
        for (at, dx) in recent {
            moved += dx;
            first = first.min(*at);
        }
        let span = now
            .saturating_duration_since(first)
            .max(Duration::from_millis(16));
        self.reset();
        Glide::Lifted {
            travel,
            speed: moved / span.as_secs_f32(),
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

/// The usual rule for a swipe's meaning: it goes if it went `distance`,
/// or was flicked a quarter of that the same way.
pub fn decide(travel: f32, speed: f32, distance: f32) -> Option<Direction> {
    let flicked =
        speed.abs() >= FLICK && speed.signum() == travel.signum() && travel.abs() >= distance / 4.0;
    (travel.abs() >= distance || flicked).then_some(if travel > 0.0 {
        Direction::Forward
    } else {
        Direction::Back
    })
}

/// Past where it would go, a swipe meets resistance: `t` (`1` there)
/// carries on ever more slowly, never past `1 + give`.
fn resist(t: f32, give: f32) -> f32 {
    if t <= 1.0 {
        t
    } else {
        1.0 + give * (1.0 - 1.0 / (1.0 + (t - 1.0) / give))
    }
}

/// What a scroll event did to a pull or a slide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swiped {
    /// Not a swipe's: scroll as usual.
    Scroll,
    /// It moved (or settles back); draw again, and wake in [`QUIET`].
    Moved,
    /// Let go far enough: go this way.
    Go(Direction),
}

/// How far the fingers go before a pull goes.
pub const PULL: f32 = 150.0;
/// The arrow's diameter.
const KNOB: f32 = 40.0;
/// How much of the way a pull towards nowhere comes.
const NOWHERE: f32 = 0.35;

/// Back and forward between places, shown as a browser shows it: an arrow
/// drawn in from the edge being pulled from (the left for back, the right
/// for forward), a ring round it filling as the fingers go, the arrow
/// filling with the accent once letting go would go. Where there is nowhere
/// to go it comes only part way and stays hollow.
#[derive(Clone, Debug)]
pub struct Pull {
    swipe: Swipe,
    /// Whether back and forward lead anywhere, as last told.
    can: (bool, bool),
    /// After the lift: where the arrow is, gliding home.
    after: Tween<f32>,
    /// How much it shows: faded where it is once it went.
    shown: Tween<f32>,
    /// Lit: letting go now would go.
    lit: Tween<f32>,
    back: Transition,
    fade: Transition,
    hover: Transition,
}

impl Pull {
    pub fn new(motion: &Motion) -> Self {
        Self {
            swipe: Swipe::new(),
            can: (false, false),
            after: Tween::new(0.0),
            shown: Tween::new(1.0),
            lit: Tween::new(0.0),
            back: motion.apply(Transition::SELECT),
            fade: motion.apply(Transition::DISMISS),
            hover: motion.apply(Transition::HOVER),
        }
    }

    /// Take a scroll event. `can` says whether back and forward lead
    /// anywhere.
    pub fn scroll(
        &mut self,
        (dx, dy, wheel, stopped): (f32, f32, bool, bool),
        can: (bool, bool),
        now: Instant,
    ) -> Swiped {
        self.can = can;
        match self.swipe.scroll(dx, dy, wheel, stopped, now) {
            Glide::Scroll => Swiped::Scroll,
            Glide::Moved => {
                if self.swipe.travel().abs() < SLOP * 2.0 {
                    // A new pull: whatever was fading is gone.
                    self.after.jump(0.0);
                    self.shown.jump(1.0);
                }
                let lit = self.going(self.swipe.travel()).is_some();
                self.lit.set(if lit { 1.0 } else { 0.0 }, self.hover, now);
                Swiped::Moved
            }
            Glide::Lifted { travel, speed } => self.finish(travel, speed, now),
        }
    }

    /// A wake-up after [`QUIET`]: a pull whose lift went unreported ends.
    pub fn expire(&mut self, now: Instant) -> Option<Direction> {
        match self.swipe.expire(now)? {
            Glide::Lifted { travel, speed } => match self.finish(travel, speed, now) {
                Swiped::Go(direction) => Some(direction),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether a pull is under way: the fingers are on it.
    pub fn is_pulling(&self) -> bool {
        self.swipe.is_sideways()
    }

    /// Whether letting go at `travel` would go, and where.
    fn going(&self, travel: f32) -> Option<Direction> {
        decide(travel, 0.0, PULL).filter(|direction| self.allowed(*direction))
    }

    fn allowed(&self, direction: Direction) -> bool {
        match direction {
            Direction::Back => self.can.0,
            Direction::Forward => self.can.1,
        }
    }

    /// How far in the arrow is for `travel`: `±1` where letting go goes.
    fn reach(&self, travel: f32) -> f32 {
        let towards = if travel >= 0.0 {
            Direction::Forward
        } else {
            Direction::Back
        };
        let t = travel.abs() / PULL;
        let t = if self.allowed(towards) {
            resist(t, 0.12)
        } else {
            NOWHERE * (1.0 - (-t).exp())
        };
        t * towards.sign()
    }

    fn finish(&mut self, travel: f32, speed: f32, now: Instant) -> Swiped {
        let reach = self.reach(travel);
        let gone = decide(travel, speed, PULL).filter(|direction| self.allowed(*direction));
        self.after.jump(reach);
        match gone {
            Some(direction) => {
                // It went: the arrow, lit, fades where it is.
                self.after.jump(reach.abs().max(1.0) * direction.sign());
                self.lit.set(1.0, self.hover, now);
                self.shown.jump(1.0);
                self.shown.set(0.0, self.fade, now);
                Swiped::Go(direction)
            }
            None => {
                self.after.set(0.0, self.back, now);
                self.lit.set(0.0, self.hover, now);
                Swiped::Moved
            }
        }
    }

    /// Whether it is still moving on its own, after the lift.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.after.is_animating(now) || self.shown.is_animating(now) || self.lit.is_animating(now)
    }

    /// Draw the arrow at the edge of `area` it is pulled from. Returns
    /// whether another frame is needed.
    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect, look: &Look, now: Instant) -> bool {
        let animating = self.is_animating(now);
        let (reach, shown) = if self.swipe.is_sideways() {
            (self.reach(self.swipe.travel()), 1.0)
        } else {
            (self.after.get(now), self.shown.get(now))
        };
        let t = reach.abs();
        if t < 0.01 || shown <= 0.01 {
            return animating;
        }
        let forward = reach > 0.0;
        // In from beyond the edge, eased; a margin inside it when there.
        let eased = 1.0 - (1.0 - t.min(1.0)).powi(2) + (t - 1.0).max(0.0);
        let way = KNOB + space::LG;
        let centre_x = if forward {
            area.right() + KNOB / 2.0 - eased * way
        } else {
            area.x - KNOB / 2.0 + eased * way
        };
        let centre_y = area.y + area.h / 2.0;
        let knob = Rect::new(centre_x - KNOB / 2.0, centre_y - KNOB / 2.0, KNOB, KNOB);
        let lit = self.lit.get(now);
        let opacity = shown * t.min(1.0).sqrt();
        canvas.with_clip(area, |canvas| {
            canvas.with_opacity(opacity, |canvas| {
                canvas.lift(knob, KNOB / 2.0, 0.8);
                canvas.circle(
                    centre_x,
                    centre_y,
                    KNOB / 2.0,
                    look.overlay.mix(look.accent, lit),
                );
                canvas.rim(knob, KNOB / 2.0, look.highlight_med.mix(look.accent, lit));
                // The ring fills as the fingers go, from the top.
                let ring = t.min(1.0);
                if ring > 0.02 && lit < 1.0 {
                    let mut arc = Shape::new();
                    arc.arc(
                        (centre_x, centre_y),
                        KNOB / 2.0 - stroke::FOCUS,
                        -FRAC_PI_2,
                        ring * TAU,
                    );
                    canvas.stroke_shape(&arc, stroke::FOCUS, look.accent.alpha(1.0 - lit));
                }
                let glyph = if forward {
                    icon::CHEVRON_RIGHT
                } else {
                    icon::CHEVRON_LEFT
                };
                canvas.icon(
                    glyph,
                    18.0,
                    look.text.mix(look.base, lit),
                    knob,
                    Align::Center,
                );
            });
        });
        animating
    }
}

/// Things side by side, stepped through by swiping: what's shown follows
/// the fingers, the next one coming in beside it; let go, it settles on
/// whichever is meant. Stepped through by other means (keys, a click), the
/// one arriving slides in a little the way it came and fades up, as a
/// folder arrives in the file manager.
///
/// [`Slide::offset`] is where what's shown is, in widths: draw the current
/// one moved left by `offset` widths, and its neighbours a width either
/// side of it.
#[derive(Clone, Debug)]
pub struct Slide {
    swipe: Swipe,
    /// Whether there is one before, and one after, as last told.
    can: (bool, bool),
    /// After a lift or a step: the offset gliding to rest, in widths.
    settle: Tween<f32>,
    /// A step by other means: how far it has arrived (`0..=1`), and from
    /// which side.
    arrival: Tween<f32>,
    from: f32,
    glide: Transition,
    arrive: Transition,
}

/// How far a stepped-to one slides in, in logical pixels.
pub const ARRIVE: f32 = 18.0;

impl Slide {
    pub fn new(motion: &Motion) -> Self {
        Self {
            swipe: Swipe::new(),
            can: (false, false),
            settle: Tween::new(0.0),
            arrival: Tween::new(1.0),
            from: 0.0,
            glide: motion.apply(Transition::REFLOW),
            arrive: motion.apply(Transition::REVEAL),
        }
    }

    /// Take a scroll event over things `width` logical pixels wide. `can`
    /// says whether there's one before, and one after. On
    /// [`Swiped::Go`], step there at once: the offset is carried over,
    /// so the one arriving goes on from where the fingers left it.
    pub fn scroll(
        &mut self,
        (dx, dy, wheel, stopped): (f32, f32, bool, bool),
        can: (bool, bool),
        width: f32,
        now: Instant,
    ) -> Swiped {
        self.can = can;
        match self.swipe.scroll(dx, dy, wheel, stopped, now) {
            Glide::Scroll => Swiped::Scroll,
            Glide::Moved => {
                self.arrival.jump(1.0);
                Swiped::Moved
            }
            Glide::Lifted { travel, speed } => self.finish(travel, speed, width, now),
        }
    }

    /// A wake-up after [`QUIET`]: a swipe whose lift went unreported ends.
    pub fn expire(&mut self, width: f32, now: Instant) -> Option<Direction> {
        match self.swipe.expire(now)? {
            Glide::Lifted { travel, speed } => match self.finish(travel, speed, width, now) {
                Swiped::Go(direction) => Some(direction),
                _ => None,
            },
            _ => None,
        }
    }

    fn finish(&mut self, travel: f32, speed: f32, width: f32, now: Instant) -> Swiped {
        let offset = self.follow(travel, width);
        let gone =
            decide(travel, speed, width.max(1.0) * 0.25).filter(|direction| match direction {
                Direction::Back => self.can.0,
                Direction::Forward => self.can.1,
            });
        match gone {
            Some(direction) => {
                // The one arriving is a width the other side of the one
                // leaving, and goes on from there.
                self.settle.jump(offset - direction.sign());
                self.settle.set(0.0, self.glide, now);
                Swiped::Go(direction)
            }
            None => {
                self.settle.jump(offset);
                self.settle.set(0.0, self.glide, now);
                Swiped::Moved
            }
        }
    }

    /// The offset for a swipe `travel` across things `width` wide: with the
    /// fingers, and resisting where there's nothing beside.
    fn follow(&self, travel: f32, width: f32) -> f32 {
        let t = travel / width.max(1.0);
        let open = if t >= 0.0 { self.can.1 } else { self.can.0 };
        if open {
            t.clamp(-1.0, 1.0)
        } else {
            let pull = t.abs();
            (0.12 * (1.0 - 1.0 / (1.0 + pull * 3.0))).copysign(t)
        }
    }

    /// Stepped by other means, `direction` that way: the one arriving
    /// slides in a little from that side, and fades up.
    pub fn arrive(&mut self, direction: Direction, now: Instant) {
        self.settle.jump(0.0);
        self.from = direction.sign();
        self.arrival.jump(0.0);
        self.arrival.set(1.0, self.arrive, now);
    }

    /// Where what's shown is, in widths: positive while the next one is
    /// coming in (what's shown moving left).
    pub fn offset(&self, width: f32, now: Instant) -> f32 {
        if self.swipe.is_sideways() {
            self.follow(self.swipe.travel(), width)
        } else {
            self.settle.get(now)
        }
    }

    /// A step's arrival: how far in from its side (logical pixels, to add
    /// to its place) and how faded up (`0..=1`).
    pub fn arrival(&self, now: Instant) -> (f32, f32) {
        let t = self.arrival.get(now);
        if t >= 1.0 {
            // Arrived: exactly in place, and wholly there.
            return (0.0, 1.0);
        }
        (self.from * (1.0 - t) * ARRIVE, 0.35 + 0.65 * t)
    }

    /// Whether the fingers are on it.
    pub fn is_swiping(&self) -> bool {
        self.swipe.is_sideways()
    }

    /// Whether it moves on its own: settling, or arriving.
    pub fn is_animating(&self, now: Instant) -> bool {
        self.settle.is_animating(now) || self.arrival.is_animating(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn sideways_is_a_swipe_and_up_and_down_a_scroll() {
        let start = Instant::now();
        let mut swipe = Swipe::new();
        assert_eq!(
            swipe.scroll(4.0, 0.5, false, false, at(start, 0)),
            Glide::Scroll
        );
        assert_eq!(
            swipe.scroll(10.0, 1.0, false, false, at(start, 10)),
            Glide::Moved
        );
        assert!(swipe.is_sideways());
        // Once sideways, up and down is ignored, and it keeps the count.
        assert_eq!(
            swipe.scroll(20.0, 30.0, false, false, at(start, 20)),
            Glide::Moved
        );
        assert_eq!(swipe.travel(), 34.0);
        match swipe.scroll(0.0, 0.0, false, true, at(start, 30)) {
            Glide::Lifted { travel, speed } => {
                assert_eq!(travel, 34.0);
                assert!(speed > 0.0);
            }
            other => panic!("{other:?}"),
        }
        assert!(!swipe.is_sideways());

        // Down first: a scroll until the fingers lift, however it turns.
        assert_eq!(
            swipe.scroll(1.0, 14.0, false, false, at(start, 100)),
            Glide::Scroll
        );
        assert_eq!(
            swipe.scroll(50.0, 0.0, false, false, at(start, 110)),
            Glide::Scroll
        );
        assert_eq!(
            swipe.scroll(0.0, 0.0, false, true, at(start, 120)),
            Glide::Scroll
        );
        assert_eq!(
            swipe.scroll(14.0, 0.0, false, false, at(start, 130)),
            Glide::Moved
        );
    }

    #[test]
    fn a_wheel_never_swipes_and_a_quiet_glide_ends() {
        let start = Instant::now();
        let mut swipe = Swipe::new();
        assert_eq!(
            swipe.scroll(40.0, 0.0, true, false, at(start, 0)),
            Glide::Scroll
        );
        assert_eq!(
            swipe.scroll(-20.0, 0.0, false, false, at(start, 10)),
            Glide::Moved
        );
        assert_eq!(swipe.expire(at(start, 100)), None);
        assert!(matches!(
            swipe.expire(at(start, 400)),
            Some(Glide::Lifted { travel, .. }) if travel == -20.0
        ));
    }

    #[test]
    fn far_enough_or_a_flick_goes() {
        assert_eq!(decide(160.0, 0.0, 150.0), Some(Direction::Forward));
        assert_eq!(decide(-150.0, 0.0, 150.0), Some(Direction::Back));
        assert_eq!(decide(100.0, 200.0, 150.0), None);
        assert_eq!(decide(60.0, 1500.0, 150.0), Some(Direction::Forward));
        // A flick back on a swipe forward: not a flick forward.
        assert_eq!(decide(60.0, -1500.0, 150.0), None);
        assert_eq!(decide(20.0, 1500.0, 150.0), None);
    }

    #[test]
    fn a_pull_goes_only_where_there_is_somewhere_to_go() {
        let motion = Motion::default();
        let start = Instant::now();
        let mut pull = Pull::new(&motion);
        let mut t = 0;
        let mut step = |pull: &mut Pull, dx: f32, can| {
            t += 8;
            pull.scroll((dx, 0.0, false, false), can, at(start, t))
        };
        for _ in 0..12 {
            step(&mut pull, -20.0, (false, true));
        }
        // Nowhere back: part way, and letting go goes nowhere.
        assert!(pull.reach(pull.swipe.travel()).abs() <= NOWHERE);
        assert_eq!(
            pull.scroll((0.0, 0.0, false, true), (false, true), at(start, 200)),
            Swiped::Moved
        );
        for _ in 0..12 {
            step(&mut pull, -20.0, (true, true));
        }
        assert!((pull.reach(pull.swipe.travel()) + 1.0).abs() < 0.2);
        assert_eq!(
            pull.scroll((0.0, 0.0, false, true), (true, true), at(start, 400)),
            Swiped::Go(Direction::Back)
        );
    }

    #[test]
    fn a_slide_carries_the_offset_over_to_the_one_arriving() {
        let motion = Motion::default();
        let start = Instant::now();
        let mut slide = Slide::new(&motion);
        for i in 0..10 {
            slide.scroll(
                (30.0, 0.0, false, false),
                (true, true),
                600.0,
                at(start, i * 10),
            );
        }
        assert!((slide.offset(600.0, at(start, 100)) - 0.5).abs() < 0.01);
        let went = slide.scroll((0.0, 0.0, false, true), (true, true), 600.0, at(start, 105));
        assert_eq!(went, Swiped::Go(Direction::Forward));
        // Now the next is shown, half a width to the right, going home.
        assert!((slide.offset(600.0, at(start, 105)) + 0.5).abs() < 0.01);
        assert_eq!(slide.offset(600.0, at(start, 2000)), 0.0);
        // Nothing before: it barely moves, and stays.
        for i in 0..10 {
            slide.scroll(
                (-60.0, 0.0, false, false),
                (false, true),
                600.0,
                at(start, 3000 + i),
            );
        }
        assert!(slide.offset(600.0, at(start, 3010)) > -0.13);
        let stayed = slide.scroll(
            (0.0, 0.0, false, true),
            (false, true),
            600.0,
            at(start, 3020),
        );
        assert_eq!(stayed, Swiped::Moved);
    }
}
