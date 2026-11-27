//! Drawing the viewer's window, and noting where what answers the pointer
//! went.

use std::{f32::consts::TAU, path::Path, time::Instant};

use enso_core::{
    Rgba, Transition,
    style::{icon, opacity, radius, space, stroke, type_scale},
};
use enso_media::clock;
use enso_ui::{
    Align, Canvas, Paint, Rect, Text,
    widget::{
        Form, Part, controls, toast,
        wave::{self, Wave},
    },
};

use crate::{
    item::Item,
    video,
    viewer::{HEADER, Hit, PANE, PANE_FROM, RESIZE, STATUS, STRIP, TILE, TILE_GAP, Viewer},
};

/// The wave rolls on slowly: crests a second.
const ROLL: f32 = 0.5;
/// The controls' bar over a video.
const BAR: f32 = 52.0;
const BAR_WIDTH: f32 = 680.0;
/// The big button over a still video.
const START: f32 = 68.0;

impl Viewer {
    pub(crate) fn draw_window(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let colors = self.ctx.theme.colors.clone();
        self.scale = canvas.scale();
        let bounds = canvas.bounds();
        self.hits.clear();
        let full = self.fullscreen;
        // Full-screen, the ground darkens towards black, as a cinema's.
        let ground = if full {
            colors.base.mix(Rgba::rgb(0, 0, 0), 0.6)
        } else {
            colors.base
        };
        canvas.clear(ground);

        let header = (!full).then(|| Rect::new(0.0, 0.0, bounds.w, HEADER));
        let status = (!full).then(|| Rect::new(0.0, bounds.h - STATUS, bounds.w, STATUS));
        let top = header.map_or(0.0, |header| header.bottom());
        let bottom = status.map_or(bounds.h, |status| status.y);
        let body = Rect::new(0.0, top, bounds.w, bottom - top);
        let pane = (self.info && !full && bounds.w >= PANE_FROM)
            .then(|| Rect::new(body.right() - PANE, body.y, PANE, body.h));
        let left = Rect::new(
            body.x,
            body.y,
            body.w - pane.map_or(0.0, |pane| pane.w),
            body.h,
        );
        let strip = (!full && self.album.len() > 1)
            .then(|| Rect::new(left.x, left.bottom() - STRIP, left.w, STRIP));
        let stage = Rect::new(
            left.x,
            left.y,
            left.w,
            left.h - strip.map_or(0.0, |strip| strip.h),
        );
        self.stage = stage;
        self.follow_stage(now);

        let mut animating = self.draw_stage(canvas, stage, now);
        if let Some(strip) = strip {
            animating |= self.draw_strip(canvas, strip, now);
        }
        if let Some(pane) = pane {
            self.draw_info(canvas, pane);
        }
        if let Some(header) = header {
            self.draw_header(canvas, header);
        }
        if let Some(status) = status {
            self.draw_status(canvas, status);
        }
        let look = self.look();
        if let Some((text, since)) = &self.toast
            && since.elapsed() < toast::STAYS
        {
            let above = strip.or(status).map_or(bounds.h, |rect| rect.y);
            toast::draw(canvas, text, above, &look);
        }
        if let Some(hit) = self.tooltip.due(now).copied()
            && let Some(tip) = hit.tip()
            && let Some((anchor, _)) = self.hits.iter().find(|(_, found)| *found == hit)
        {
            controls::tooltip(canvas, tip, *anchor, bounds, &look);
        }
        if self.help {
            self.draw_help(canvas, bounds);
        }
        Paint::when(animating)
    }

    /// A video decodes at the stage's size, once that has settled.
    fn follow_stage(&mut self, now: Instant) {
        let room = self.room();
        let area = (room.w * self.scale, room.h * self.scale);
        let Some(video) = &mut self.video else {
            self.resizing = None;
            return;
        };
        let Some(own) = &video.media().video else {
            return;
        };
        let wanted = video::fit(own.width, own.height, area);
        if wanted == video.playback.size() {
            self.resizing = None;
            return;
        }
        match self.resizing {
            Some((size, since)) if size == wanted => {
                if now.saturating_duration_since(since) >= RESIZE {
                    video.playback.set_size(wanted, now);
                    self.resizing = None;
                }
            }
            _ => self.resizing = Some((wanted, now)),
        }
    }

    // ---- The stage ---------------------------------------------------------

    fn draw_stage(&mut self, canvas: &mut Canvas<'_>, stage: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let size = self.ctx.config.appearance.font_size;
        self.hits.push((stage, Hit::Stage));
        let offset = self.slide.offset(stage.w, now);
        let (arrive, faded) = self.slide.arrival(now);
        let shift = -offset * stage.w + arrive;
        let mut animating = self.slide.is_animating(now);

        let current = self.album.current().cloned();
        let read = current
            .as_ref()
            .and_then(|path| self.read.get(path).cloned());
        match (self.item().cloned(), read) {
            (Some(item), _) => {
                canvas.with_opacity(faded, |canvas| {
                    self.draw_item(canvas, &item, shift, now);
                });
            }
            (None, Some(Err(err))) => {
                let said = format!("Can't show it: {err}");
                canvas.text_in(
                    &Text::new(&said, size, colors.muted).max_width(stage.w - 2.0 * space::XL),
                    stage.translate(shift, 0.0),
                    Align::Center,
                );
            }
            (None, _) if current.is_none() => {
                canvas.text_in(
                    &Text::new("No pictures or videos here", size, colors.muted),
                    stage,
                    Align::Center,
                );
            }
            // Being read: the one before stays a moment, faint.
            (None, _) => {
                if let Some((_, item)) = self.shown.clone() {
                    canvas.with_opacity(0.4, |canvas| {
                        self.draw_still(canvas, &item, stage, shift);
                    });
                }
            }
        }
        // The one coming in beside it, while swiping.
        let beside = if offset > 0.0 {
            self.album.neighbour(1)
        } else if offset < 0.0 {
            self.album.neighbour(-1)
        } else {
            None
        };
        if let Some(path) = beside
            && let Some(Ok(item)) = self.read.get(path).cloned()
        {
            let side = shift + offset.signum() * stage.w;
            self.draw_still(canvas, &item, stage, side);
        }
        if offset.abs() < 0.001 && self.video.is_some() {
            animating |= self.draw_controls(canvas, stage, now);
        }
        animating
    }

    /// The current one, as it is: a picture where the view puts it, a video
    /// playing (or its still frame).
    fn draw_item(&mut self, canvas: &mut Canvas<'_>, item: &Item, shift: f32, now: Instant) {
        match item {
            Item::Picture { photo, .. } => {
                let placed = self.placement(photo).translate(shift, 0.0);
                let level = photo.level_for(placed.w * self.scale);
                canvas.pixels(&level.rgba, level.width, level.height, placed);
            }
            Item::Video { poster, media } => {
                let Some(own) = &media.video else { return };
                let room = self.room();
                let rect =
                    fitted(room, (own.width, own.height), self.scale, true).translate(shift, 0.0);
                let frame = self
                    .video
                    .as_mut()
                    .and_then(|video| video.playback.frame(now));
                match (frame, poster) {
                    // Decoded at the size it's shown: placed on whole
                    // pixels at that size, it's copied as it is. (While a
                    // new size is on its way, it's scaled to fit.)
                    (Some(frame), _) => {
                        let scale = self.scale;
                        let own = (frame.width as f32, frame.height as f32);
                        let exact = (own.0 - rect.w * scale).abs() <= 2.0
                            && (own.1 - rect.h * scale).abs() <= 2.0;
                        let rect = if exact {
                            let x = ((rect.x + rect.w / 2.0) * scale - own.0 / 2.0).round();
                            let y = ((rect.y + rect.h / 2.0) * scale - own.1 / 2.0).round();
                            Rect::new(x / scale, y / scale, own.0 / scale, own.1 / scale)
                        } else {
                            rect
                        };
                        canvas.frame(&frame.rgba, frame.width, frame.height, rect);
                    }
                    (None, Some(poster)) => {
                        let level = poster.level_for(rect.w * self.scale);
                        canvas.pixels(&level.rgba, level.width, level.height, rect);
                    }
                    (None, None) => {}
                }
            }
        }
    }

    /// Something still, fitted to the stage: a neighbour coming in, or the
    /// one before while the next is read.
    fn draw_still(&self, canvas: &mut Canvas<'_>, item: &Item, stage: Rect, shift: f32) {
        let Some(still) = item.still() else { return };
        let Some(size) = item.size() else { return };
        let room = if self.fullscreen {
            stage
        } else {
            stage.inset(space::LG)
        };
        let rect = fitted(room, size, self.scale, item.is_video()).translate(shift, 0.0);
        let level = still.level_for(rect.w * self.scale);
        canvas.pixels(&level.rgba, level.width, level.height, rect);
    }

    /// Over a video: the big button while it's still, and the controls'
    /// bar while the pointer moves or it's paused. Returns whether either
    /// is moving.
    fn draw_controls(&mut self, canvas: &mut Canvas<'_>, stage: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let look = self.look();
        let small = look.size * type_scale::SMALL;
        let Some(video) = &self.video else {
            return false;
        };
        let playing = video.playback.is_playing();
        let ended = video.playback.has_ended();
        let started = video.started && !ended;
        let wanted = !playing
            || video.held.is_some()
            || self.chrome_until.is_some_and(|until| now < until)
            || matches!(
                self.hover,
                Some(Hit::PlayPause | Hit::Timeline | Hit::Mute | Hit::Controls)
            );
        let target = if wanted { 1.0 } else { 0.0 };
        if self.controls.target() != target {
            let transition = if wanted {
                Transition::REVEAL
            } else {
                Transition::DISMISS
            };
            self.controls.set(target, self.transition(transition), now);
        }
        let waving = if playing { 1.0 } else { 0.0 };
        if self.waving.target() != waving {
            self.waving
                .set(waving, self.transition(Transition::SCENE), now);
        }
        let shown = self.controls.get(now);
        let animating = self.controls.is_animating(now) || self.waving.is_animating(now);

        // Not started (or played out): one big button, the lightest thing
        // on the stage.
        if !started {
            let button = Rect::new(
                stage.x + (stage.w - START) / 2.0,
                stage.y + (stage.h - START) / 2.0,
                START,
                START,
            );
            let glow = if self.hover == Some(Hit::Start) {
                1.0
            } else {
                0.0
            };
            let button = button.inset(-2.0 * glow);
            canvas.lift(button, button.w / 2.0, 1.0);
            canvas.circle(
                button.x + button.w / 2.0,
                button.y + button.h / 2.0,
                button.w / 2.0,
                colors.text.mix(colors.subtle, 0.1 * (1.0 - glow)),
            );
            canvas.icon(
                icon::PLAY,
                30.0,
                colors.base,
                button.translate(2.0, 0.0),
                Align::Center,
            );
            self.hits.push((button, Hit::Start));
            return animating;
        }
        if shown <= 0.01 {
            return animating;
        }

        let Some(video) = &self.video else {
            return animating;
        };
        let width = BAR_WIDTH.min(stage.w - 2.0 * space::XL);
        let bar = Rect::new(
            stage.x + (stage.w - width) / 2.0,
            stage.bottom() - space::XL - BAR + (1.0 - shown) * space::SM,
            width,
            BAR,
        );
        let pill = bar.h / 2.0;
        let fraction = video.fraction(now);
        let length = video.media().duration;
        let at = match (video.held, length) {
            (Some(held), Some(length)) => length.mul_f32(held),
            _ => video.playback.position(now),
        };
        let end = length.map_or_else(|| "Live".to_owned(), clock);
        let muted = video.playback.is_muted();
        let held = video.held.is_some();
        let mut hits = Vec::new();
        canvas.with_opacity(shown, |canvas| {
            canvas.lift(bar, pill, 1.0);
            canvas.fill(bar, pill, colors.surface);
            canvas.grain(bar, pill);
            canvas.rim(bar, pill, colors.highlight_med);
            hits.push((bar, Hit::Controls));

            // Play and pause: the lightest thing on the bar.
            let play = Rect::new(bar.x + 8.0, bar.y + (bar.h - 36.0) / 2.0, 36.0, 36.0);
            let glow = if self.hover == Some(Hit::PlayPause) {
                1.0
            } else {
                0.0
            };
            canvas.circle(
                play.x + play.w / 2.0,
                play.y + play.h / 2.0,
                play.w / 2.0 + glow,
                colors.text.mix(colors.subtle, 0.1 * (1.0 - glow)),
            );
            let figure = if playing { icon::PAUSE } else { icon::PLAY };
            canvas.icon(figure, 18.0, colors.base, play, Align::Center);
            hits.push((play, Hit::PlayPause));

            // Sound on or off, at the other end.
            let mute = Rect::new(bar.right() - 8.0 - 36.0, play.y, 36.0, 36.0);
            if self.hover == Some(Hit::Mute) {
                canvas.circle(
                    mute.x + mute.w / 2.0,
                    mute.y + mute.h / 2.0,
                    mute.w / 2.0,
                    colors.highlight_low,
                );
            }
            let speaker = if muted {
                icon::VOLUME_MUTED
            } else {
                icon::VOLUME
            };
            canvas.icon(speaker, 18.0, colors.subtle, mute, Align::Center);
            hits.push((mute, Hit::Mute));

            // The times, either side of the wave.
            let time_width = canvas
                .measure(&Text::new(&end, small, colors.muted).tabular())
                .w
                .ceil()
                + 2.0;
            let start = Rect::new(play.right() + space::MD, bar.y, time_width, bar.h);
            canvas.text_in(
                &Text::new(&clock(at), small, colors.muted).tabular(),
                start,
                Align::End,
            );
            let finish = Rect::new(mute.x - space::SM - time_width, bar.y, time_width, bar.h);
            canvas.text_in(
                &Text::new(&end, small, colors.muted).tabular(),
                finish,
                Align::Start,
            );
            let track = Rect::new(
                start.right() + space::MD,
                bar.y + (bar.h - 24.0) / 2.0,
                (finish.x - space::MD - start.right() - space::MD).max(8.0),
                24.0,
            );
            let phase = now.saturating_duration_since(self.opened).as_secs_f32() * ROLL * TAU;
            wave::draw(
                canvas,
                track,
                fraction,
                Wave::calm(self.waving.get(now), phase),
                wave::Look {
                    played: accent,
                    rest: colors.highlight_high,
                },
                if held { 1.0 } else { 0.0 },
            );
            hits.push((track.inset_xy(0.0, -6.0), Hit::Timeline));
        });
        self.hits.extend(hits);
        animating
    }

    // ---- The strip ---------------------------------------------------------

    fn draw_strip(&mut self, canvas: &mut Canvas<'_>, area: Rect, now: Instant) -> bool {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        canvas.fill(area, 0.0, colors.surface);
        canvas.fill(
            Rect::new(area.x, area.y, area.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let pitch = TILE + TILE_GAP;
        // It follows the fingers while they swipe.
        let at = self.strip_at.get(now) + self.slide.offset(self.stage.w, now);
        let middle = area.x + area.w / 2.0;
        let y = area.y + (area.h - TILE) / 2.0;
        let reach = (area.w / 2.0 / pitch).ceil() + 1.0;
        let first = (at - reach).floor().max(0.0) as usize;
        let last = ((at + reach).ceil().max(0.0) as usize + 1).min(self.album.len());
        let mut hits = Vec::new();
        // No clip: the window's edge and the pane (drawn after) bound the
        // strip, and a clip is a mask the size of the window each frame.
        {
            for index in first..last {
                let x = middle + (index as f32 - at) * pitch - TILE / 2.0;
                let tile = Rect::new(x.round(), y, TILE, TILE);
                // Fading towards the strip's ends.
                let room = (tile.x - area.x).min(area.right() - tile.right());
                let alpha = 0.2 + 0.8 * (room / (pitch * 1.5)).clamp(0.0, 1.0);
                let path = &self.album.items[index];
                let video = crate::album::is_video(path);
                canvas.with_opacity(alpha, |canvas| {
                    match self.thumbs.get(path) {
                        Some(Some(image)) => canvas.image(image, tile, radius::CONTROL),
                        _ => {
                            canvas.fill(tile, radius::CONTROL, colors.highlight_low);
                            let figure = if video { icon::PLAY } else { icon::FILE };
                            canvas.icon(figure, 18.0, colors.muted, tile, Align::Center);
                        }
                    }
                    if video {
                        // A little play mark in the corner.
                        let badge =
                            Rect::new(tile.right() - 19.0, tile.bottom() - 19.0, 15.0, 15.0);
                        canvas.circle(
                            badge.x + badge.w / 2.0,
                            badge.y + badge.h / 2.0,
                            badge.w / 2.0,
                            Rgba::rgb(0, 0, 0).alpha(0.55),
                        );
                        canvas.icon(
                            icon::PLAY,
                            9.0,
                            Rgba::rgb(255, 255, 255),
                            badge.translate(0.5, 0.0),
                            Align::Center,
                        );
                    }
                });
                let ring = tile.inset(-3.0);
                if index == self.album.at {
                    canvas.stroke(ring, radius::CONTROL + 3.0, stroke::FOCUS, accent);
                } else if self.hover == Some(Hit::Thumb(index)) {
                    canvas.stroke(
                        ring,
                        radius::CONTROL + 3.0,
                        stroke::HAIRLINE,
                        colors.highlight_high,
                    );
                }
                hits.push((tile.inset(-TILE_GAP / 2.0), Hit::Thumb(index)));
            }
        }
        self.hits.extend(hits);
        self.strip_at.is_animating(now)
    }

    // ---- The title row, the status line, the details -----------------------

    fn draw_header(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        canvas.fill(area, 0.0, colors.base);
        // Back and forward, as the file manager's.
        let button = 32.0;
        let mut x = area.x + space::MD;
        let (before, after) = self.album.neighbours();
        for (figure, hit, enabled) in [
            (icon::CHEVRON_LEFT, Hit::Back, before),
            (icon::CHEVRON_RIGHT, Hit::Forward, after),
        ] {
            let rect = Rect::new(x, area.y + (area.h - button) / 2.0, button, button);
            if enabled && self.hover == Some(hit) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let color = if enabled {
                colors.text
            } else {
                colors.muted.alpha(opacity::DISABLED)
            };
            canvas.icon(figure, 18.0, color, rect, Align::Center);
            if enabled {
                self.hits.push((rect, hit));
            }
            x += button + space::XS;
        }
        x += space::MD;

        // Its buttons, from the right.
        let picture = self.photo().is_some();
        let mut right = area.right() - space::MD;
        let buttons = [
            (icon::TRASH, Hit::Trash, false, true),
            (icon::FOLDER, Hit::Reveal, false, true),
            (icon::INFO, Hit::Info, self.info, true),
            (icon::TURN, Hit::Turn, false, picture),
        ];
        for (figure, hit, on, shown) in buttons {
            if !shown {
                continue;
            }
            let rect = Rect::new(
                right - button,
                area.y + (area.h - button) / 2.0,
                button,
                button,
            );
            // On is the accent's: the details are open.
            if on {
                canvas.fill(rect, radius::CONTROL, accent.alpha(opacity::TINT));
            } else if self.hover == Some(hit) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let color = if on { accent } else { colors.subtle };
            canvas.icon(figure, 18.0, color, rect, Align::Center);
            self.hits.push((rect, hit));
            right = rect.x - space::XS;
        }

        // What's shown, and where among the rest.
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        let room = right - space::LG - x;
        let place = (self.album.len() > 1)
            .then(|| format!("{} of {}", self.album.at + 1, self.album.len()));
        let place_width = place.as_ref().map_or(0.0, |place| {
            canvas
                .measure(&Text::new(place, small, colors.muted).tabular())
                .w
                .ceil()
                + space::MD
        });
        let name = Self::name(&path);
        let title = Text::new(&name, size, colors.text)
            .medium()
            .max_width((room - place_width).max(40.0));
        let drawn = canvas.text_in(&title, Rect::new(x, area.y, room, area.h), Align::Start);
        if let Some(place) = place {
            canvas.text_in(
                &Text::new(&place, small, colors.muted).tabular(),
                Rect::new(x + drawn.w + space::MD, area.y, place_width, area.h),
                Align::Start,
            );
        }
    }

    fn draw_status(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let small = self.ctx.config.appearance.font_size * type_scale::SMALL;
        canvas.fill(area, 0.0, colors.base);
        canvas.fill(
            Rect::new(area.x, area.y, area.w, stroke::HAIRLINE),
            0.0,
            colors.highlight_low,
        );
        let inner = area.inset_xy(space::LG, 0.0);
        self.know_file();
        let bytes = self.file.as_ref().map(|(_, bytes, _)| *bytes);
        let mut parts = Vec::new();
        let mut right = None;
        match self.item().map(AsRef::as_ref) {
            Some(Item::Picture { photo, .. }) => {
                let (w, h) = photo.size();
                parts.push(format!("{w} × {h}"));
                let percent = self.zoom(photo) * 100.0 / self.scale.max(0.01);
                right = Some(if self.view.zoom.is_none() {
                    format!("Fit · {percent:.0}%")
                } else {
                    format!("{percent:.0}%")
                });
            }
            Some(Item::Video { media, .. }) => {
                if let Some(own) = &media.video {
                    parts.push(format!("{} × {}", own.width, own.height));
                }
                if let Some(length) = media.duration {
                    parts.push(clock(length));
                }
            }
            None => {}
        }
        if let Some(bytes) = bytes {
            parts.push(enso_fs::format::size(bytes));
        }
        if let Some(path) = self.album.current() {
            parts.push(kind_word(path, self.item().map(AsRef::as_ref)));
        }
        canvas.text_in(
            &Text::new(&parts.join(" · "), small, colors.subtle).max_width(inner.w * 0.7),
            inner,
            Align::Start,
        );
        if let Some(right) = right {
            let text = Text::new(&right, small, colors.subtle).tabular();
            let width = canvas.measure(&text).w.ceil() + 2.0 * space::SM;
            let rect = Rect::new(
                inner.right() - width + space::SM,
                area.y + 4.0,
                width,
                area.h - 8.0,
            );
            if self.hover == Some(Hit::Zoom) {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            canvas.text_in(&text, rect, Align::Center);
            self.hits.push((rect, Hit::Zoom));
        }
    }

    /// Note the current file's size and date, once each time it changes.
    fn know_file(&mut self) {
        let Some(path) = self.album.current() else {
            self.file = None;
            return;
        };
        if self.file.as_ref().is_some_and(|(known, ..)| known == path) {
            return;
        }
        let meta = std::fs::metadata(path).ok();
        self.file = Some((
            path.clone(),
            meta.as_ref().map_or(0, std::fs::Metadata::len),
            meta.and_then(|meta| meta.modified().ok()),
        ));
    }

    /// The details pane: everything known about it, as the file manager's
    /// preview pane lays it out.
    fn draw_info(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let size = self.ctx.config.appearance.font_size;
        let small = size * type_scale::SMALL;
        canvas.fill(area, 0.0, colors.surface);
        canvas.fill(
            Rect::new(area.x, area.y, stroke::HAIRLINE, area.h),
            0.0,
            colors.highlight_low,
        );
        let Some(path) = self.album.current().cloned() else {
            return;
        };
        self.know_file();
        let inner = area.inset(space::LG);
        let name = Self::name(&path);
        let title = Text::new(&name, size * type_scale::TITLE, colors.text)
            .medium()
            .max_width(inner.w)
            .max_lines(3);
        let height = canvas.text(&title, inner.x, inner.y).h;
        let mut y = inner.y + height + space::MD;
        let mut rows: Vec<(&str, String)> =
            vec![("Kind", kind_word(&path, self.item().map(AsRef::as_ref)))];
        match self.item().map(AsRef::as_ref) {
            Some(Item::Picture { photo, details }) => {
                let (w, h) = photo.size();
                rows.push(("Size", format!("{w} × {h}")));
                if let Some(taken) = &details.taken {
                    rows.push(("Taken", taken_when(taken)));
                }
                if let Some(camera) = &details.camera {
                    rows.push(("Camera", camera.clone()));
                }
            }
            Some(Item::Video { media, .. }) => {
                if let Some(own) = &media.video {
                    rows.push(("Size", format!("{} × {}", own.width, own.height)));
                    rows.push(("Frames", format!("{:.0} a second", own.rate)));
                }
                if let Some(length) = media.duration {
                    rows.push(("Length", clock(length)));
                }
                let codecs: Vec<String> = media
                    .video
                    .iter()
                    .map(|own| codec(&own.codec))
                    .chain(media.audio.iter().map(|sound| codec(&sound.codec)))
                    .collect();
                if !codecs.is_empty() {
                    rows.push(("Coded", codecs.join(" · ")));
                }
                if media.audio.is_none() {
                    rows.push(("Sound", "None".to_owned()));
                }
            }
            None => {}
        }
        if let Some((_, bytes, modified)) = &self.file {
            rows.push(("On disk", enso_fs::format::size(*bytes)));
            if let Some(modified) = modified {
                rows.push(("Modified", modified_when(*modified)));
            }
        }
        if let Some(folder) = path.parent() {
            let home = enso_fs::places::home();
            let shown = match folder.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => folder.display().to_string(),
            };
            rows.push(("Folder", shown));
        }
        let label_width = 92.0;
        for (label, value) in rows {
            if y > inner.bottom() - 20.0 {
                break;
            }
            canvas.text(&Text::new(label, small, colors.muted), inner.x, y);
            let value = Text::new(&value, small, colors.subtle)
                .max_width(inner.w - label_width)
                .max_lines(2);
            let height = canvas.text(&value, inner.x + label_width, y).h;
            y += height.max(small * 1.4) + space::XS;
        }
    }

    fn draw_help(&self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let look = self.look();
        let small = look.size * type_scale::SMALL;
        canvas.fill(
            bounds,
            0.0,
            self.ctx.theme.colors.base.alpha(opacity::SCRIM),
        );
        let keys: [(&str, &str); 14] = [
            ("←  →  ·  two fingers", "the one before, the next"),
            ("Home  ·  End", "the first, the last"),
            ("Space  ·  k", "play, pause"),
            ("Shift+← →  ·  ,  .", "back, on 5 seconds"),
            ("m", "sound on, off"),
            ("+  -  ·  wheel", "zoom"),
            ("0  ·  1  ·  double-click", "fit · every pixel"),
            ("r  ·  R", "turn right · left"),
            ("i", "details"),
            ("f  ·  F11", "full screen"),
            ("Ctrl+C  ·  w", "copy · make the wallpaper"),
            ("e", "show in Files"),
            ("Delete  ·  Ctrl+Z", "to the trash · put back"),
            ("Esc  ·  q", "close"),
        ];
        let rows = keys.len().div_ceil(2);
        let line = (small * 1.9).round();
        let placed = Form::new("Keys")
            .width(640.0)
            .about("A picture steps on with Space; a video plays and pauses with it.")
            .part(Part::Space(rows as f32 * line))
            .draw(canvas, bounds, &look, 1.0);
        let grid = placed.parts[0];
        let column = (grid.w - space::XL) / 2.0;
        let key_width = column * 0.5;
        for (index, (key, what)) in keys.iter().enumerate() {
            let (col, row) = (index / rows, index % rows);
            let x = grid.x + col as f32 * (column + space::XL);
            let y = grid.y + row as f32 * line;
            canvas.text_in(
                &Text::new(key, small, look.accent).medium(),
                Rect::new(x, y, key_width, line),
                Align::Start,
            );
            canvas.text_in(
                &Text::new(what, small, look.subtle),
                Rect::new(x + key_width, y, column - key_width, line),
                Align::Start,
            );
        }
    }
}

/// Where something `size` (its own pixels) goes, fitted to `stage`: never
/// enlarged, unless `enlarge` (a video, which fills its stage).
fn fitted(stage: Rect, (w, h): (u32, u32), scale: f32, enlarge: bool) -> Rect {
    let (sw, sh) = (stage.w * scale, stage.h * scale);
    let mut zoom = (sw / w.max(1) as f32).min(sh / h.max(1) as f32);
    if !enlarge {
        zoom = zoom.min(1.0);
    }
    let (dw, dh) = (w as f32 * zoom / scale, h as f32 * zoom / scale);
    Rect::new(
        (stage.x + (stage.w - dw) / 2.0).round(),
        (stage.y + (stage.h - dh) / 2.0).round(),
        dw,
        dh,
    )
}

/// What it is, in words: `JPEG picture`, `MP4 video`.
fn kind_word(path: &Path, item: Option<&Item>) -> String {
    if let Some(Item::Video { media, .. }) = item
        && !media.format.is_empty()
    {
        return format!("{} video", media.format);
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_uppercase)
        .unwrap_or_default();
    let ext = match ext.as_str() {
        "JPG" | "JPE" => "JPEG".to_owned(),
        _ => ext,
    };
    if crate::album::is_video(path) {
        format!("{ext} video")
    } else {
        format!("{ext} picture")
    }
}

/// A codec's name as people know it.
fn codec(name: &str) -> String {
    match name {
        "h264" => "H.264".to_owned(),
        "hevc" => "HEVC".to_owned(),
        "av1" => "AV1".to_owned(),
        "vp8" | "vp9" | "aac" | "mp3" | "flac" => name.to_uppercase(),
        "opus" => "Opus".to_owned(),
        "vorbis" => "Vorbis".to_owned(),
        other => other.to_owned(),
    }
}

/// `2026:11:02 10:00:00` as `Mon 2 Nov 2026, 10:00`.
fn taken_when(exif: &str) -> String {
    jiff::civil::DateTime::strptime("%Y:%m:%d %H:%M:%S", exif)
        .map(|when| when.strftime("%a %-d %b %Y, %H:%M").to_string())
        .unwrap_or_else(|_| exif.to_owned())
}

fn modified_when(time: std::time::SystemTime) -> String {
    jiff::Timestamp::try_from(time)
        .map(|stamp| {
            stamp
                .to_zoned(jiff::tz::TimeZone::system())
                .strftime("%a %-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
