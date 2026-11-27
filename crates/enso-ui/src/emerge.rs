//! Cards that grow out of the bar. When there is a bar along the top of
//! the screen, the launcher, the clipboard history and the keyring's card
//! open out of its clock island and settle under it in the middle; the control center opens out
//! of the status island and the notification history out of the bell, and
//! each hangs under the end of its island. Elsewhere the pickers settle a
//! fifth of the way down. Nothing is dimmed behind them either way.
//!
//! The bar keeps a note of where its islands are ([`enso_core::island`]);
//! [`Perch::find_from`] reads it for the screen the card is for — the one
//! named in `$ENSO_OUTPUT` when the bar opened it, otherwise the one niri
//! says has the focus. Without a bar, or without niri, a card simply opens
//! the other way.

use enso_core::{
    Easing, Motion, Palette, Rgba, Transition,
    island::{self, Island, Which},
    style::{edge, radius, space},
};
use niri_ipc::{Request, Response, socket::Socket};

use crate::{Canvas, Presence, Rect, Shape, SurfaceSpec};

/// How much the clock island swells as the card leaves it, either side
/// and above and below.
const SWELL: (f32, f32) = (7.0, 3.0);

/// Where an overlay card opens.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Perch {
    /// The bar's island on the card's screen, when a bar runs along that
    /// screen's top, in the card's surface's coordinates: the card grows
    /// out of it.
    pub island: Option<Island>,
    /// Which island that is: a card from the clock hangs in the middle of
    /// the screen; one from an island at the right end, under that end.
    pub from: Which,
    /// The island was lit by the pointer when last looked at, so its swell
    /// is drawn lit too.
    pub lit: bool,
}

/// The screen with the focus, by name, from niri. `None` outside niri,
/// which then puts a surface where it likes.
pub fn focused_output() -> Option<String> {
    let reply = Socket::connect().ok()?.send(Request::FocusedOutput).ok()?;
    match reply {
        Ok(Response::FocusedOutput(output)) => output.map(|output| output.name),
        _ => None,
    }
}

/// The screen a card is for: the bar's, when the bar opened it (it says
/// so in `$ENSO_OUTPUT`), otherwise the one with the focus.
pub fn screen() -> Option<String> {
    std::env::var("ENSO_OUTPUT")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(focused_output)
}

impl Perch {
    /// The bar's clock on the card's screen, if it has one along its top.
    pub fn find() -> (Option<String>, Self) {
        Self::find_from(Which::Clock)
    }

    /// The island `from` of the bar on the card's screen.
    pub fn find_from(from: Which) -> (Option<String>, Self) {
        let screen = screen();
        let perch = Self::on(screen.as_deref(), from);
        (screen, perch)
    }

    /// The island `from` of the bar on `screen`, if that bar runs along its
    /// top. One at the right end is only of use if the bar said how wide it
    /// is: a card's surface finds the island from the screen's right edge.
    pub fn on(screen: Option<&str>, from: Which) -> Self {
        let note = screen.and_then(island::find).filter(|note| note.top);
        let island = note
            .and_then(|note| note.at(from))
            .filter(|island| from == Which::Clock || island.screen > 0.0);
        Self {
            island,
            from,
            lit: note.is_some_and(|note| note.lit == Some(from)),
        }
    }

    /// Look again at whether the island is lit: as the card goes back into
    /// it, the pointer may have come or gone.
    pub fn relight(&mut self, screen: Option<&str>) {
        if self.island.is_some() {
            let note = screen.and_then(island::find);
            self.lit = note.is_some_and(|note| note.lit == Some(self.from));
        }
    }

    /// How far from the screen's right edge a surface anchored there must
    /// reach to hold a card `width` wide under this island, and the island
    /// as it swells. `None` without an island.
    pub fn reach(&self, width: f32) -> Option<f32> {
        let island = self.island?;
        let under = island.screen - (island.x + island.w) + width;
        let swollen = island.screen - island.x + SWELL.0 + 1.0;
        Some(under.max(swollen).ceil())
    }

    /// The same, seen from a surface whose left edge is `left` along the
    /// screen.
    pub fn seen_from(self, left: f32) -> Self {
        Self {
            island: self.island.map(|island| Island {
                x: island.x - left,
                ..island
            }),
            ..self
        }
    }

    /// Easing in and out: the stages of [`Emergence`] out of the bar, or
    /// the usual reveal.
    pub fn presence(&self, motion: &Motion) -> Presence {
        match self.island {
            Some(_) => Presence::with(motion, Transition::EMERGE, Transition::RETURN),
            None => Presence::new(motion),
        }
    }

    /// A full-screen overlay on `screen`, over the bar too: the card grows
    /// out of it.
    pub fn surface(namespace: &str, screen: Option<String>) -> SurfaceSpec {
        SurfaceSpec {
            exclusive_zone: -1,
            output: screen,
            ..SurfaceSpec::overlay(namespace)
        }
    }

    /// Where a card `width` × `height` rests on a screen of `bounds`:
    /// hanging under the bar in the middle, or under the end of its island
    /// at the right; or, without a bar, a fifth of the way down.
    pub fn rest(&self, bounds: Rect, width: f32, height: f32) -> Rect {
        match self.island {
            Some(island) if self.from != Which::Clock => Rect::new(
                island.x + island.w - width,
                island.bar + edge::BELOW_BAR,
                width,
                height,
            ),
            Some(island) => Rect::new(
                ((bounds.w - width) / 2.0).round(),
                island.bar + edge::BELOW_BAR,
                width,
                height,
            ),
            None => Rect::new(
                (bounds.w - width) / 2.0,
                (bounds.h * 0.2).round(),
                width,
                height,
            ),
        }
    }

    /// How much of what a card holds shows, `p` of the way open: once it
    /// has come out of the island, or as it eases in.
    pub fn shown(&self, p: f32) -> f32 {
        match self.island {
            Some(_) => Emergence::at(p).content,
            None => p,
        }
    }

    /// Draw the card's body `p` of the way open (as its [`Presence`] says),
    /// to rest at `card`. Returns where its contents go, and leaves the
    /// canvas's opacity at how much of them shows; the caller sets it back.
    pub fn card(&self, canvas: &mut Canvas<'_>, card: Rect, p: f32, colors: &Palette) -> Rect {
        match self.island {
            Some(island) => {
                let stage = Emergence::at(p);
                emerge(canvas, &island, self.lit, card, &stage, colors);
                canvas.set_opacity(stage.content);
                card
            }
            None => {
                let card = card.translate(0.0, (p - 1.0) * space::MD);
                canvas.set_opacity(p);
                canvas.card(card, colors.surface, colors.highlight_med);
                card
            }
        }
    }
}

/// A card on its way out of the bar's clock island, at `p` of the way
/// there (`0` the island alone, `1` the card at rest). Going back, the
/// same way in reverse: the card gathers itself, then is drawn back in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emergence {
    /// How far the island has swollen, `0..=1`: up as the card slides out
    /// from under it, and back as it goes.
    pub swell: f32,
    /// How far the card has dropped from under the island to its place:
    /// a little past `1` and back, as it settles.
    pub drop: f32,
    /// How far it has spread from the island's width to its own, the same.
    pub spread: f32,
    /// How solid the card is.
    pub solid: f32,
    /// How far it has turned from the island's colour to its own.
    pub tint: f32,
    /// How much of what the card holds shows.
    pub content: f32,
}

impl Emergence {
    pub fn at(p: f32) -> Self {
        let span = |from: f32, to: f32| ((p - from) / (to - from)).clamp(0.0, 1.0);
        Self {
            swell: Easing::OutCubic.apply(span(0.0, 0.25))
                * (1.0 - Easing::InOutCubic.apply(span(0.3, 0.7))),
            drop: settle(span(0.05, 0.7)),
            spread: settle(span(0.2, 0.95)),
            solid: Easing::OutCubic.apply(span(0.05, 0.15)),
            tint: Easing::InOutCubic.apply(span(0.2, 0.65)),
            content: Easing::OutCubic.apply(span(0.6, 1.0)),
        }
    }
}

/// Quick out, a few hundredths past the end, and back: how something
/// thrown settles into its place.
fn settle(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.0 * u.powi(3) + u.powi(2)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Draw the card coming out of `island` towards `card`, as `stage` has it:
/// the island swelling round what it shows as the card slides out from
/// under it, drops into place and spreads, turning from the island's colour
/// to its own. A `lit` island is the shade the bar lights it under the
/// pointer, and so is its swell.
pub fn emerge(
    canvas: &mut Canvas<'_>,
    island: &Island,
    lit: bool,
    card: Rect,
    stage: &Emergence,
    colors: &Palette,
) {
    let ground = if lit { colors.overlay } else { colors.base };
    let shore = ground.alpha(island.opacity);
    let clock = Rect::new(island.x, island.y, island.w, island.h);
    let swollen = Rect::new(
        clock.x - SWELL.0 * stage.swell,
        clock.y - SWELL.1 * stage.swell,
        clock.w + 2.0 * SWELL.0 * stage.swell,
        clock.h + 2.0 * SWELL.1 * stage.swell,
    );

    // The island, swollen: a ring round the bar's own, in its colour, so
    // its clock still shows. It reaches a pixel inside, so no seam shows
    // where the two meet.
    if stage.swell > 0.0 {
        let mut ring = Shape::new();
        pill(&mut ring, swollen, true);
        pill(&mut ring, clock.inset(1.0), false);
        canvas.fill_shape(&ring, shore);
    }
    if stage.solid <= 0.0 {
        return;
    }

    // It starts hidden under the island, as wide as its flat foot, so it
    // comes out from under the middle of it and not past its round ends.
    let foot = Rect::new(clock.x + clock.h / 2.0, clock.y, clock.w - clock.h, clock.h);
    let (across, down) = (stage.spread, stage.drop);
    let now = Rect::new(
        lerp(foot.x, card.x, across),
        lerp(foot.y, card.y, down),
        lerp(foot.w, card.w, across),
        lerp(foot.h, card.h, down),
    );
    let corner = lerp(foot.h / 2.0, radius::CARD, across.min(1.0)).min(now.h / 2.0);
    let alpha = lerp(f32::from(shore.a) / 255.0, 1.0, stage.tint);
    let fill = Rgba {
        a: 255,
        ..shore.mix(colors.surface, stage.tint)
    }
    .alpha(alpha);
    // Under the island: nothing of the card shows above its foot.
    let under = swollen.bottom();
    let hidden = now.y < under;
    if hidden {
        let bounds = canvas.bounds();
        canvas.set_clip(Some(Rect::new(0.0, under, bounds.w, bounds.h)));
    }
    canvas.with_opacity(stage.solid, |canvas| {
        // Its depth comes as it leaves the bar: the shadow, the grain, and
        // the light along its rim, as every card has them.
        canvas.lift(now, corner, stage.tint);
        canvas.fill(now, corner, fill);
        canvas.with_opacity(stage.tint, |canvas| {
            canvas.grain(now, corner);
            canvas.rim(now, corner, colors.highlight_med);
        });
    });
    if hidden {
        canvas.set_clip(None);
    }
}

/// Add a pill filling `rect` to `shape`, round clockwise or the other way:
/// a pill inside another, turning the other way, is a hole in it.
fn pill(shape: &mut Shape, rect: Rect, clockwise: bool) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = (rect.h.min(rect.w) / 2.0).max(0.0);
    let (left, right) = ((rect.x + r, rect.y + r), (rect.right() - r, rect.y + r));
    shape.move_to(left.0, rect.y);
    if clockwise {
        shape.arc_to(right, r, -FRAC_PI_2, PI);
        shape.arc_to(left, r, FRAC_PI_2, PI);
    } else {
        shape.arc_to(left, r, -FRAC_PI_2, -PI);
        shape.arc_to(right, r, FRAC_PI_2, -PI);
    }
    shape.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_grows_out_of_the_clock_and_settles_under_the_bar() {
        let rest = Emergence::at(1.0);
        assert_eq!(
            (
                rest.swell,
                rest.drop,
                rest.spread,
                rest.solid,
                rest.tint,
                rest.content
            ),
            (0.0, 1.0, 1.0, 1.0, 1.0, 1.0)
        );
        let hidden = Emergence::at(0.0);
        assert_eq!(
            (
                hidden.swell,
                hidden.drop,
                hidden.spread,
                hidden.solid,
                hidden.content
            ),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        let stages: Vec<Emergence> = (0..=100).map(|i| Emergence::at(i as f32 / 100.0)).collect();
        // It goes a little past its place, and only a little.
        for furthest in [
            stages.iter().map(|s| s.drop).fold(0.0, f32::max),
            stages.iter().map(|s| s.spread).fold(0.0, f32::max),
        ] {
            assert!(furthest > 1.0 && furthest < 1.05, "{furthest}");
        }
        // It drops before it spreads.
        assert!(stages[30].drop > stages[30].spread + 0.3);
        // The island swells while the card leaves it, then settles.
        assert!(stages[25].swell > 0.95);
        // What the card holds shows only once it has arrived.
        assert!(stages[..60].iter().all(|s| s.content == 0.0));
        assert!(
            stages[60..]
                .iter()
                .all(|s| s.drop >= 0.999 && s.spread >= 0.999)
        );
    }

    #[test]
    fn a_card_rests_under_the_bar_or_a_fifth_of_the_way_down() {
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let bar = Perch {
            island: Some(Island {
                pid: 1,
                top: true,
                bar: 36.0,
                x: 430.0,
                y: 4.0,
                w: 140.0,
                h: 28.0,
                opacity: 0.9,
                ..Island::default()
            }),
            ..Perch::default()
        };
        assert_eq!(
            bar.rest(screen, 560.0, 56.0),
            Rect::new(220.0, 40.0, 560.0, 56.0)
        );
        let alone = Perch::default();
        assert_eq!(
            alone.rest(screen, 560.0, 56.0),
            Rect::new(220.0, 160.0, 560.0, 56.0)
        );
        assert_eq!(Perch::surface("x", None).exclusive_zone, -1);
    }

    /// A card from the bell or the status island hangs under the end of its
    /// island, and its surface, anchored at the screen's right, reaches far
    /// enough for both the card and the island's swell.
    #[test]
    fn a_card_from_the_right_hangs_under_its_island() {
        let note = Island {
            pid: 1,
            top: true,
            bar: 36.0,
            x: 880.0,
            y: 4.0,
            w: 160.0,
            h: 28.0,
            opacity: 0.9,
            screen: 1920.0,
            status: Some(island::Spot {
                x: 1700.0,
                y: 4.0,
                w: 168.0,
                h: 28.0,
            }),
            bell: Some(island::Spot {
                x: 1876.0,
                y: 4.0,
                w: 36.0,
                h: 28.0,
            }),
            lit: None,
        };
        let status = Perch {
            island: note.at(Which::Status),
            from: Which::Status,
            lit: false,
        };
        // Wider than the island: as far as the card goes.
        let reach = status.reach(340.0).expect("an island");
        assert_eq!(reach, 1920.0 - 1868.0 + 340.0);
        let seen = status.seen_from(1920.0 - reach);
        let surface = Rect::new(0.0, 0.0, reach, 600.0);
        let card = seen.rest(surface, 340.0, 200.0);
        assert_eq!((card.x, card.right(), card.y), (0.0, 340.0, 40.0));
        // Narrower than the island: as far as its swell.
        let bell = Perch {
            island: note.at(Which::Bell),
            from: Which::Bell,
            lit: false,
        };
        assert_eq!(bell.reach(20.0), Some(1920.0 - 1876.0 + SWELL.0 + 1.0));
        // The clock still hangs in the middle.
        let clock = Perch {
            island: Some(note),
            ..Perch::default()
        };
        let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(clock.rest(screen, 560.0, 56.0).x, 680.0);
    }
}
