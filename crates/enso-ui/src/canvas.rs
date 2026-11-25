//! Software canvas: rounded shapes, soft shadows, text, and icons drawn in
//! logical pixels onto a premultiplied pixel buffer (tiny-skia).

use enso_core::{
    Icon, Rgba,
    style::{Shadow, radius},
};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient,
    Mask, Paint, Path, PathBuilder, Pattern, Pixmap, PixmapMut, PixmapRef, Point, RadialGradient,
    SpreadMode, Stroke, Transform,
};

use crate::{
    geom::{Align, Rect, Size},
    image::Image,
    ime::Preedit,
    picture::Picture,
    text::{Text, TextEngine},
};

/// Drawing surface handed to [`App::draw`](crate::App::draw).
///
/// Coordinates are logical pixels; the canvas applies the output scale.
pub struct Canvas<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
    scale: f32,
    /// Byte order is B, G, R, A (Wayland `argb8888`) instead of R, G, B, A.
    bgra: bool,
    opacity: f32,
    /// Where drawing is confined to, if anywhere.
    clip: Option<Clip>,
    text: &'a mut TextEngine,
    /// What an input method is composing, for the focused field to show.
    preedit: Option<Preedit>,
    /// Where the focused text field's caret was drawn, if one was.
    text_input: Option<Rect>,
}

/// A clip: a mask that shapes and text are drawn through, and the physical
/// pixel bounds (`x0, y0, x1, y1`, end-exclusive) it lies within.
struct Clip {
    mask: Mask,
    bounds: (i32, i32, i32, i32),
}

impl std::fmt::Debug for Canvas<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}

impl<'a> Canvas<'a> {
    /// Wrap a `width × height` buffer of premultiplied 4-byte pixels.
    pub(crate) fn new(
        pixels: &'a mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        bgra: bool,
        text: &'a mut TextEngine,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            scale,
            bgra,
            opacity: 1.0,
            clip: None,
            text,
            preedit: None,
            text_input: None,
        }
    }

    /// What an input method is composing, shown by the focused text field.
    /// The runtime sets it before each frame; tests may set it too.
    pub fn set_preedit(&mut self, preedit: Option<Preedit>) {
        self.preedit = preedit;
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Say that a focused text field is in this frame, its caret at
    /// `caret`: an input method may compose into it, and its candidates
    /// hang from there. Fields that take passwords never say so.
    pub fn text_input_at(&mut self, caret: Rect) {
        self.text_input = Some(caret);
    }

    /// Where a focused text field's caret was drawn this frame, if one was.
    pub fn text_input(&self) -> Option<Rect> {
        self.text_input
    }

    /// Canvas size in logical pixels.
    pub fn size(&self) -> Size {
        Size {
            w: self.width as f32 / self.scale,
            h: self.height as f32 / self.scale,
        }
    }

    /// The whole canvas as a rectangle.
    /// Output pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn bounds(&self) -> Rect {
        let Size { w, h } = self.size();
        Rect::new(0.0, 0.0, w, h)
    }

    /// Whether icons are drawn (as opposed to stood in for by words).
    pub fn glyphs(&self) -> bool {
        self.text.glyphs()
    }

    /// Multiply the opacity of everything drawn from now on. Used for fades.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// Run `draw` with a temporary opacity.
    pub fn with_opacity(&mut self, opacity: f32, draw: impl FnOnce(&mut Self)) {
        let previous = self.opacity;
        self.set_opacity(previous * opacity);
        draw(self);
        self.opacity = previous;
    }

    /// Replace every pixel with `color` (ignores the current opacity).
    pub fn clear(&mut self, color: Rgba) {
        let px = premultiply(color, self.bgra);
        for chunk in self.pixels.as_chunks_mut::<4>().0 {
            *chunk = px;
        }
    }

    /// Confine everything drawn from now on to `rect`, or lift the limit
    /// with `None`. What falls outside is simply not drawn — the way a
    /// scrolling list keeps to its viewport. [`Canvas::clear`] ignores it.
    pub fn set_clip(&mut self, rect: Option<Rect>) {
        self.clip = rect.and_then(|rect| {
            let scale = self.scale;
            let x0 = ((rect.x * scale).round() as i32).clamp(0, self.width as i32);
            let y0 = ((rect.y * scale).round() as i32).clamp(0, self.height as i32);
            let x1 = ((rect.right() * scale).round() as i32).clamp(x0, self.width as i32);
            let y1 = ((rect.bottom() * scale).round() as i32).clamp(y0, self.height as i32);
            let mut mask = Mask::new(self.width, self.height)?;
            // An empty clip hides everything: the mask stays blank.
            if let Some(area) =
                tiny_skia::Rect::from_ltrb(x0 as f32, y0 as f32, x1 as f32, y1 as f32)
            {
                let path = PathBuilder::from_rect(area);
                mask.fill_path(&path, FillRule::Winding, false, Transform::identity());
            }
            Some(Clip {
                mask,
                bounds: (x0, y0, x1, y1),
            })
        });
    }

    /// Run `draw` confined to `rect`, then restore the previous limit.
    pub fn with_clip(&mut self, rect: Rect, draw: impl FnOnce(&mut Self)) {
        let previous = self.clip.take();
        self.set_clip(Some(rect));
        draw(self);
        self.clip = previous;
    }

    /// Run `draw` confined to `shape` — or, with `inside` false, to all but
    /// it — within whatever limit there already is, then restore that
    /// limit. Its edge is soft, so what is split along it is split cleanly:
    /// words drawn once on each side in two colours read as one.
    pub fn with_clip_shape(&mut self, shape: &Shape, inside: bool, draw: impl FnOnce(&mut Self)) {
        let Some(path) = shape.path() else {
            // Nothing: inside it is nowhere, outside it everywhere.
            if !inside {
                draw(self);
            }
            return;
        };
        let Some(mut mask) = Mask::new(self.width, self.height) else {
            return;
        };
        mask.fill_path(&path, FillRule::Winding, true, self.transform());
        if !inside {
            mask.invert();
        }
        let bounds = match &self.clip {
            Some(clip) => {
                for (cover, limit) in mask.data_mut().iter_mut().zip(clip.mask.data()) {
                    *cover = (u16::from(*cover) * u16::from(*limit) / 255) as u8;
                }
                clip.bounds
            }
            None => (0, 0, self.width as i32, self.height as i32),
        };
        let previous = self.clip.replace(Clip { mask, bounds });
        draw(self);
        self.clip = previous;
    }

    /// Fill a rectangle with rounded corners. `radius` 0 gives square corners.
    pub fn fill(&mut self, rect: Rect, radius: f32, color: Rgba) {
        let Some(path) = rounded_rect(rect, radius) else {
            return;
        };
        let paint = self.paint(color);
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Fill a shape with fully rounded ends.
    pub fn pill(&mut self, rect: Rect, color: Rgba) {
        self.fill(rect, radius::pill(rect.h.min(rect.w)), color);
    }

    /// Outline a rounded rectangle; the stroke sits inside `rect`.
    pub fn stroke(&mut self, rect: Rect, radius: f32, width: f32, color: Rgba) {
        let Some(path) = rounded_rect(rect.inset(width / 2.0), (radius - width / 2.0).max(0.0))
        else {
            return;
        };
        let paint = self.paint(color);
        let transform = self.transform();
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
        }
    }

    /// Fill the closed shape through `points`, in logical pixels: the way a
    /// small icon of its own — a lightning bolt — is drawn without a font
    /// or an image file.
    pub fn polygon(&mut self, points: &[(f32, f32)], color: Rgba) {
        let Some((&(x, y), rest)) = points.split_first() else {
            return;
        };
        let mut path = PathBuilder::new();
        path.move_to(x, y);
        for &(x, y) in rest {
            path.line_to(x, y);
        }
        path.close();
        let Some(path) = path.finish() else {
            return;
        };
        let paint = self.paint(color);
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Fill a [`Shape`].
    pub fn fill_shape(&mut self, shape: &Shape, color: Rgba) {
        let Some(path) = shape.path() else {
            return;
        };
        let paint = self.paint(color);
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Draw the lines of a [`Shape`], `width` thick, with round ends and
    /// round corners: soft, like everything else here.
    pub fn stroke_shape(&mut self, shape: &Shape, width: f32, color: Rgba) {
        let Some(path) = shape.path() else {
            return;
        };
        let paint = self.paint(color);
        let transform = self.transform();
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
        }
    }

    /// Fill a circle.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgba) {
        self.fill(Rect::new(cx - r, cy - r, r * 2.0, r * 2.0), r, color);
    }

    /// A floating card — the launcher, the panels, toasts, menus, and
    /// dialogs: lifted off what is behind it, filled, grained, and edged.
    pub fn card(&mut self, rect: Rect, fill: Rgba, edge: Rgba) {
        use enso_core::style::radius;
        self.lift(rect, radius::CARD, 1.0);
        self.fill(rect, radius::CARD, fill);
        self.grain(rect, radius::CARD);
        self.rim(rect, radius::CARD, edge);
    }

    /// The shadow of something floating `amount` (`0..=1`) of the way off
    /// the page: a tight, darker one where it nearly touches, and a faint,
    /// wide one under it — as light falls, rather than one soft haze.
    /// Draw it before the shape.
    pub fn lift(&mut self, rect: Rect, radius: f32, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        for shadow in [Shadow::AMBIENT, Shadow::CONTACT] {
            let shadow = Shadow {
                opacity: shadow.opacity * amount.min(1.0),
                ..shadow
            };
            self.shadow(rect, radius, shadow);
        }
    }

    /// A card's edge: a hairline of `edge` that catches the light along
    /// the top and fades into the shadow at the bottom.
    pub fn rim(&mut self, rect: Rect, radius: f32, edge: Rgba) {
        use enso_core::style::stroke;
        let width = stroke::HAIRLINE;
        let Some(path) = rounded_rect(rect.inset(width / 2.0), (radius - width / 2.0).max(0.0))
        else {
            return;
        };
        let lit = edge.mix(Rgba::rgb(255, 255, 255), 0.14);
        let stops = vec![
            GradientStop::new(0.0, self.color(lit)),
            GradientStop::new(0.35, self.color(edge)),
            GradientStop::new(1.0, self.color(edge.alpha(0.45))),
        ];
        let Some(shader) = LinearGradient::new(
            Point::from_xy(0.0, rect.y),
            Point::from_xy(0.0, rect.bottom()),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return self.stroke(rect, radius, width, edge);
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
        }
    }

    /// A fine grain over a surface, a pixel of the screen to a speck: the
    /// tooth of paper rather than the flat of a screen. It also breaks up
    /// the steps a dark gradient falls into.
    pub fn grain(&mut self, rect: Rect, radius: f32) {
        let Some(path) = rounded_rect(rect, radius) else {
            return;
        };
        let scale = self.scale;
        let paint = Paint {
            shader: Pattern::new(
                noise().as_ref(),
                SpreadMode::Repeat,
                FilterQuality::Nearest,
                self.opacity,
                Transform::from_scale(1.0 / scale, 1.0 / scale),
            ),
            anti_alias: true,
            ..Paint::default()
        };
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Soft drop shadow for a rounded rectangle. Draw it before the shape.
    pub fn shadow(&mut self, rect: Rect, radius: f32, shadow: Shadow) {
        let color = shadow.color;
        // Stacked, progressively larger translucent shapes approximate a blur
        // closely enough at these sizes and cost far less than a real one.
        const LAYERS: usize = 8;
        let rect = rect.translate(0.0, shadow.offset_y);
        let layer = color.alpha(shadow.opacity / LAYERS as f32 * 1.6);
        for i in 0..LAYERS {
            let grow = shadow.blur * (i as f32 + 1.0) / LAYERS as f32;
            self.fill(
                rect.inset(-grow * 0.6),
                radius + grow * 0.6,
                layer.alpha(1.0 - i as f32 / LAYERS as f32),
            );
        }
    }

    /// Size `text` would occupy.
    pub fn measure(&mut self, text: &Text<'_>) -> Size {
        self.text.measure(text)
    }

    /// Draw text with its top-left corner at `(x, y)`. Returns its size.
    pub fn text(&mut self, text: &Text<'_>, x: f32, y: f32) -> Size {
        if text.text.is_empty() || self.opacity <= 0.0 {
            return Size::default();
        }
        let origin = (
            (x * self.scale).round() as i32,
            (y * self.scale).round() as i32,
        );
        let width = self.width as i32;
        let (x0, y0, x1, y1) = self
            .clip
            .as_ref()
            .map_or((0, 0, width, self.height as i32), |clip| clip.bounds);
        // Within the bounds, as much as the clip lets through: all of it for
        // a rectangle, a soft edge for a shape.
        let cover = self.clip.as_ref().map(|clip| clip.mask.data());
        let (bgra, opacity) = (self.bgra, self.opacity);
        let pixels = &mut *self.pixels;
        self.text
            .rasterize(text, self.scale, origin, |px, py, color| {
                if px < x0 || py < y0 || px >= x1 || py >= y1 {
                    return;
                }
                let at = py as usize * width as usize + px as usize;
                let through = cover.map_or(1.0, |cover| f32::from(cover[at]) / 255.0);
                if through <= 0.0 {
                    return;
                }
                let i = at * 4;
                blend(
                    &mut pixels[i..i + 4],
                    premultiply(color.alpha(opacity * through), bgra),
                );
            })
    }

    /// Draw single-line text aligned horizontally in `rect` and centred vertically.
    pub fn text_in(&mut self, text: &Text<'_>, rect: Rect, align: Align) -> Size {
        let text = Text {
            max_width: text.max_width.or(Some(rect.w)),
            ..*text
        };
        let size = self.measure(&text);
        let x = match align {
            Align::Start => rect.x,
            Align::Center => rect.x + (rect.w - size.w) / 2.0,
            Align::End => rect.right() - size.w,
        };
        self.text(&text, x, rect.y + (rect.h - size.h) / 2.0)
    }

    /// Draw an icon `size` logical pixels square, aligned across `rect` and
    /// centred down it: drawn (see [`figure`](crate::figure)), or its few
    /// letters where icons are words. Returns the space it took.
    pub fn icon(&mut self, icon: Icon, size: f32, color: Rgba, rect: Rect, align: Align) -> Size {
        if !self.glyphs() {
            let text = Text::new(icon.text, size * 0.8, color);
            return self.text_in(&text, rect, align);
        }
        let x = match align {
            Align::Start => rect.x,
            Align::Center => rect.x + (rect.w - size) / 2.0,
            Align::End => rect.right() - size,
        };
        let y = rect.y + (rect.h - size) / 2.0;
        // On whole pixels, so the pen falls the same way in every icon.
        let snap = |v: f32| (v * self.scale).round() / self.scale;
        crate::figure::draw(
            self,
            icon.figure,
            (snap(x), snap(y)),
            size,
            crate::figure::Look::new(color),
        );
        Size { w: size, h: size }
    }

    /// [`Canvas::icon`], drawn at a moment or showing an amount (see
    /// [`figure::Live`](crate::figure::Live)): steam that drifts, a sun as
    /// bright as the screen.
    pub fn icon_live(
        &mut self,
        icon: Icon,
        size: f32,
        color: Rgba,
        rect: Rect,
        align: Align,
        live: crate::figure::Live,
    ) -> Size {
        if !self.glyphs() {
            return self.icon(icon, size, color, rect, align);
        }
        let x = match align {
            Align::Start => rect.x,
            Align::Center => rect.x + (rect.w - size) / 2.0,
            Align::End => rect.right() - size,
        };
        let y = rect.y + (rect.h - size) / 2.0;
        let snap = |v: f32| (v * self.scale).round() / self.scale;
        crate::figure::draw_live(
            self,
            icon.figure,
            (snap(x), snap(y)),
            size,
            crate::figure::Look::new(color),
            live,
        );
        Size { w: size, h: size }
    }

    /// Draw `image` so that it covers `rect`, centred and cropped to fit,
    /// with rounded corners.
    pub fn image(&mut self, image: &Image, rect: Rect, radius: f32) {
        let Some(path) = rounded_rect(rect, radius) else {
            return;
        };
        let bytes = if self.bgra { &image.bgra } else { &image.rgba };
        let Some(source) = PixmapRef::from_bytes(bytes, image.width, image.height) else {
            return;
        };
        let (w, h) = (image.width as f32, image.height as f32);
        let scale = (rect.w / w).max(rect.h / h);
        let placement = Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            rect.x + (rect.w - w * scale) / 2.0,
            rect.y + (rect.h - h * scale) / 2.0,
        );
        let paint = Paint {
            shader: Pattern::new(
                source,
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                self.opacity,
                placement,
            ),
            anti_alias: true,
            ..Paint::default()
        };
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Draw a picture of any size scaled into `dest` (logical pixels, and
    /// as far off the canvas as it likes: only what shows is drawn). The
    /// pixels are premultiplied R, G, B, A, `width × height`; smooth while
    /// shrinking or gently enlarged, square pixels when enlarged a lot, as
    /// an image viewer zooms in. Returns `false` if the numbers don't
    /// describe `rgba`.
    pub fn pixels(&mut self, rgba: &[u8], width: u32, height: u32, dest: Rect) -> bool {
        let Some(source) = PixmapRef::from_bytes(rgba, width, height) else {
            return false;
        };
        if dest.w <= 0.0 || dest.h <= 0.0 {
            return true;
        }
        // The part that shows, in output pixels.
        let s = self.scale;
        let x0 = (dest.x * s).floor().max(0.0);
        let y0 = (dest.y * s).floor().max(0.0);
        let x1 = (dest.right() * s).ceil().min(self.width as f32);
        let y1 = (dest.bottom() * s).ceil().min(self.height as f32);
        if x1 <= x0 || y1 <= y0 {
            return true;
        }
        let (tw, th) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let Some(mut tile) = tiny_skia::Pixmap::new(tw, th) else {
            return true;
        };
        let zoom = dest.w * s / width as f32;
        let quality = if zoom > 3.0 {
            FilterQuality::Nearest
        } else {
            FilterQuality::Bilinear
        };
        let placement = Transform::from_row(
            zoom,
            0.0,
            0.0,
            dest.h * s / height as f32,
            dest.x * s - x0,
            dest.y * s - y0,
        );
        let paint = Paint {
            shader: Pattern::new(source, SpreadMode::Pad, quality, 1.0, placement),
            ..Paint::default()
        };
        let area = tiny_skia::Rect::from_xywh(
            (dest.x * s - x0).max(0.0),
            (dest.y * s - y0).max(0.0),
            (dest.w * s).min(tw as f32),
            (dest.h * s).min(th as f32),
        );
        if let Some(area) = area {
            tile.fill_rect(area, &paint, Transform::identity(), None);
        }
        // The canvas may be B, G, R, A; the picture is R, G, B, A.
        if self.bgra {
            for pixel in tile.data_mut().as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let opacity = self.opacity;
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.draw_pixmap(
                x0 as i32,
                y0 as i32,
                tile.as_ref(),
                &tiny_skia::PixmapPaint {
                    opacity,
                    blend_mode: BlendMode::SourceOver,
                    quality: FilterQuality::Nearest,
                },
                Transform::identity(),
                mask,
            );
        }
        true
    }

    /// Copy `picture` onto the canvas pixel for pixel — its pixels are the
    /// output's, not logical ones — centred, and cropped where it is larger.
    /// At full opacity that is a plain copy; below it the picture is mixed
    /// over what is there, which is how one wallpaper fades into the next.
    /// Like [`Canvas::clear`], it ignores the clip.
    pub fn picture(&mut self, picture: &Picture) {
        let (cw, ch) = (self.width as usize, self.height as usize);
        let (pw, ph) = (picture.width as usize, picture.height as usize);
        let (w, h) = (cw.min(pw), ch.min(ph));
        let (cx, cy) = ((cw - w) / 2, (ch - h) / 2);
        let (px, py) = ((pw - w) / 2, (ph - h) / 2);
        let mix = (self.opacity * 256.0).round() as u32;
        if mix == 0 {
            return;
        }
        let swap = self.bgra;
        for row in 0..h {
            let to = ((cy + row) * cw + cx) * 4;
            let from = ((py + row) * pw + px) * 4;
            let target = &mut self.pixels[to..to + w * 4];
            let source = &picture.rgba[from..from + w * 4];
            if mix >= 256 && !swap {
                target.copy_from_slice(source);
                continue;
            }
            let pairs = target
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(source.as_chunks::<4>().0);
            for (dst, src) in pairs {
                let src = if swap {
                    [src[2], src[1], src[0], src[3]]
                } else {
                    *src
                };
                if mix >= 256 {
                    *dst = src;
                } else {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = ((u32::from(*d) * (256 - mix) + u32::from(s) * mix) >> 8) as u8;
                    }
                }
            }
        }
    }

    /// A soft round light centred on (`cx`, `cy`): `color` at `alpha` out to
    /// `inner`, fading to nothing at `outer`. Logical pixels.
    pub fn glow(&mut self, (cx, cy): (f32, f32), inner: f32, outer: f32, color: Rgba, alpha: f32) {
        let alpha = (alpha * self.opacity * f32::from(color.a) / 255.0).clamp(0.0, 1.0);
        if alpha <= 0.0 || outer <= inner {
            return;
        }
        let (r, b) = if self.bgra {
            (color.b, color.r)
        } else {
            (color.r, color.b)
        };
        let channel = |c: u8| f32::from(c) / 255.0;
        let stop = |at: f32, alpha: f32| {
            Color::from_rgba(channel(r), channel(color.g), channel(b), alpha)
                .map(|color| GradientStop::new(at, color))
        };
        let (Some(bright), Some(gone)) = (stop(0.0, alpha), stop(1.0, 0.0)) else {
            return;
        };
        let centre = Point::from_xy(cx, cy);
        let Some(shader) = RadialGradient::new(
            centre,
            inner.max(0.0),
            centre,
            outer,
            vec![bright, gone],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let Some(path) = rounded_rect(
            Rect::new(cx - outer, cy - outer, outer * 2.0, outer * 2.0),
            0.0,
        ) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
        }
    }

    /// Draw premultiplied pixels — `rgba`, or `bgra` when the canvas is in
    /// that order — of `size` (the output's own pixels) with their centre at
    /// the logical point `centre`, `zoom` times their size. At `zoom` 1 they
    /// land on whole pixels and are copied exactly, not resampled.
    pub(crate) fn pixels_at(
        &mut self,
        rgba: &[u8],
        bgra: &[u8],
        size: (u32, u32),
        centre: (f32, f32),
        zoom: f32,
    ) {
        let bytes = if self.bgra { bgra } else { rgba };
        let Some(source) = PixmapRef::from_bytes(bytes, size.0, size.1) else {
            return;
        };
        let (w, h) = (size.0 as f32 * zoom, size.1 as f32 * zoom);
        let exact = (zoom - 1.0).abs() < f32::EPSILON;
        // In the output's pixels, so the corner can be put on a whole one.
        let mut x = centre.0 * self.scale - w / 2.0;
        let mut y = centre.1 * self.scale - h / 2.0;
        if exact {
            x = x.round();
            y = y.round();
        }
        let placement = Transform::from_row(zoom, 0.0, 0.0, zoom, x, y);
        let paint = Paint {
            shader: Pattern::new(
                source,
                SpreadMode::Pad,
                if exact {
                    FilterQuality::Nearest
                } else {
                    FilterQuality::Bilinear
                },
                self.opacity,
                placement,
            ),
            anti_alias: false,
            ..Paint::default()
        };
        let Some(path) = rounded_rect(Rect::new(x, y, w, h), 0.0) else {
            return;
        };
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                mask,
            );
        }
    }

    /// Draw the line through `points`, `width` thick with square-cut ends
    /// (so that lines meeting end to end don't overlap), in `color` at
    /// `alpha` (unrounded, unlike a colour's own). Logical pixels.
    pub(crate) fn line(&mut self, points: &[(f32, f32)], width: f32, color: Rgba, alpha: f32) {
        let mut builder = PathBuilder::new();
        let Some((first, rest)) = points.split_first() else {
            return;
        };
        builder.move_to(first.0, first.1);
        for point in rest {
            builder.line_to(point.0, point.1);
        }
        let Some(path) = builder.finish() else {
            return;
        };
        let (r, b) = if self.bgra {
            (color.b, color.r)
        } else {
            (color.r, color.b)
        };
        let channel = |c: u8| f32::from(c) / 255.0;
        let alpha = (alpha * self.opacity).clamp(0.0, 1.0);
        let Some(color) = Color::from_rgba(channel(r), channel(color.g), channel(b), alpha) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        let transform = self.transform();
        let (pixmap, mask) = self.target();
        if let Some(mut pixmap) = pixmap {
            pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
        }
    }

    /// The pixels to draw on, and the clip mask to draw through.
    fn target(&mut self) -> (Option<PixmapMut<'_>>, Option<&Mask>) {
        (
            PixmapMut::from_bytes(self.pixels, self.width, self.height),
            self.clip.as_ref().map(|clip| &clip.mask),
        )
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    /// `color` as tiny-skia takes it: in the canvas's channel order, and
    /// as see-through as the canvas is drawing.
    fn color(&self, color: Rgba) -> Color {
        let color = color.alpha(self.opacity);
        let (r, b) = if self.bgra {
            (color.b, color.r)
        } else {
            (color.r, color.b)
        };
        Color::from_rgba8(r, color.g, b, color.a)
    }

    fn paint(&self, color: Rgba) -> Paint<'static> {
        let color = color.alpha(self.opacity);
        let (r, b) = if self.bgra {
            (color.b, color.r)
        } else {
            (color.r, color.b)
        };
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(r, color.g, b, color.a));
        paint.anti_alias = true;
        paint.blend_mode = BlendMode::SourceOver;
        paint
    }
}

/// Premultiplied pixel bytes for `color` in the canvas's channel order.
fn premultiply(color: Rgba, bgra: bool) -> [u8; 4] {
    let a = u32::from(color.a);
    let mul = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
    let (r, g, b) = (mul(color.r), mul(color.g), mul(color.b));
    if bgra {
        [b, g, r, color.a]
    } else {
        [r, g, b, color.a]
    }
}

/// Source-over blend of a premultiplied pixel onto another.
fn blend(dst: &mut [u8], src: [u8; 4]) {
    let inverse = 255 - u32::from(src[3]);
    for (d, s) in dst.iter_mut().zip(src) {
        *d = (u32::from(s) + (u32::from(*d) * inverse + 127) / 255).min(255) as u8;
    }
}

/// A figure of an interface's own — lines, curves, arcs, circles — to fill
/// or stroke on a [`Canvas`]: how an icon is drawn without a font or an
/// image file. Coordinates are logical pixels; the canvas scales them, so a
/// shape is as sharp on a HiDPI screen as anywhere.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    builder: PathBuilder,
}

impl Shape {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in `other`'s pieces too: filled, the two together.
    pub fn add(&mut self, other: &Shape) -> &mut Self {
        if let Some(path) = other.path() {
            self.builder.push_path(&path);
        }
        self
    }

    /// Begin a new piece of the figure at `(x, y)`.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.builder.move_to(x, y);
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.builder.line_to(x, y);
        self
    }

    /// A cubic Bézier curve to `(x, y)`, pulled toward the two controls.
    pub fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) -> &mut Self {
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
        self
    }

    /// Join the current piece back to where it began.
    pub fn close(&mut self) -> &mut Self {
        self.builder.close();
        self
    }

    /// A quadratic Bézier curve to `(x, y)`, pulled toward the control.
    pub fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) -> &mut Self {
        self.builder.quad_to(x1, y1, x, y);
        self
    }

    /// A circular arc around `(cx, cy)`, as a piece of its own: from the
    /// angle `start`, turning through `sweep` (radians; positive turns
    /// clockwise on screen, and 0 points right).
    pub fn arc(&mut self, centre: (f32, f32), r: f32, start: f32, sweep: f32) -> &mut Self {
        self.arc_from(centre, r, start, sweep, false)
    }

    /// [`arc`](Self::arc), carrying on the current piece: a straight line
    /// is drawn to where the arc begins.
    pub fn arc_to(&mut self, centre: (f32, f32), r: f32, start: f32, sweep: f32) -> &mut Self {
        self.arc_from(centre, r, start, sweep, true)
    }

    fn arc_from(
        &mut self,
        (cx, cy): (f32, f32),
        r: f32,
        start: f32,
        sweep: f32,
        joined: bool,
    ) -> &mut Self {
        // Each quarter turn or less is one cubic, whose controls sit
        // 4/3·tan(θ/4) of the radius along the tangents.
        let pieces = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0);
        let step = sweep / pieces;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let at = |angle: f32| (cx + r * angle.cos(), cy + r * angle.sin());
        let (x, y) = at(start);
        if joined && self.builder.last_point().is_some() {
            self.builder.line_to(x, y);
        } else {
            self.builder.move_to(x, y);
        }
        let mut angle = start;
        for _ in 0..pieces as usize {
            let next = angle + step;
            let (x0, y0) = at(angle);
            let (x3, y3) = at(next);
            self.builder.cubic_to(
                x0 - k * r * angle.sin(),
                y0 + k * r * angle.cos(),
                x3 + k * r * next.sin(),
                y3 - k * r * next.cos(),
                x3,
                y3,
            );
            angle = next;
        }
        self
    }

    /// A whole circle, as a piece of its own.
    pub fn circle(&mut self, (cx, cy): (f32, f32), r: f32) -> &mut Self {
        self.builder.push_circle(cx, cy, r);
        self
    }

    /// A rectangle with rounded corners, as a piece of its own.
    pub fn rounded_rect(&mut self, rect: Rect, radius: f32) -> &mut Self {
        if let Some(path) = rounded_rect(rect, radius) {
            self.builder.push_path(&path);
        }
        self
    }

    /// The smallest rectangle around what the shape covers — curves at
    /// their true extent, not their control points — before any stroke.
    pub fn bounds(&self) -> Option<Rect> {
        let bounds = self.path()?.compute_tight_bounds()?;
        Some(Rect::new(
            bounds.x(),
            bounds.y(),
            bounds.width(),
            bounds.height(),
        ))
    }

    fn path(&self) -> Option<Path> {
        self.builder.clone().finish()
    }
}

fn rounded_rect(rect: Rect, radius: f32) -> Option<Path> {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let Rect { x, y, w, h } = rect;
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    let mut path = PathBuilder::new();
    if r <= 0.0 {
        path.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h)?);
        return path.finish();
    }
    // Each corner eases into its sides rather than meeting them at the end
    // of a circle's arc: the continuous corner of a phone's own windows. A
    // corner with no room for the easing — a pill's ends — is a circle's.
    let corner = Corner::new(r, CORNER_SMOOTHING, w.min(h) / 2.0);
    let p = corner.p;
    path.move_to(x + p, y);
    // Top right, bottom right, bottom left, top left: each turns a quarter.
    path.line_to(x + w - p, y);
    corner.add(&mut path, (x + w - p, y), (1.0, 0.0), (0.0, 1.0));
    path.line_to(x + w, y + h - p);
    corner.add(&mut path, (x + w, y + h - p), (0.0, 1.0), (-1.0, 0.0));
    path.line_to(x + p, y + h);
    corner.add(&mut path, (x + p, y + h), (-1.0, 0.0), (0.0, -1.0));
    path.line_to(x, y + p);
    corner.add(&mut path, (x, y + p), (0.0, -1.0), (1.0, 0.0));
    path.close();
    path.finish()
}

/// How much a corner eases into its sides, `0` a circle's arc. Six tenths
/// is the smoothing a phone's own corners have.
const CORNER_SMOOTHING: f32 = 0.6;

/// A smoothed corner of radius `r`: a curve easing off the side, a stretch
/// of circle, and a curve easing onto the next side, spread over `p` of
/// each side (Figma's construction of the continuous corner).
#[derive(Clone, Copy, Debug)]
struct Corner {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    p: f32,
    r: f32,
    /// The arc's angle, in radians.
    arc: f32,
    /// How far the arc's ends lie apart along each side.
    arc_len: f32,
    /// How far the arc's ends are turned from the sides, in radians.
    beta: f32,
}

impl Corner {
    fn new(r: f32, smoothing: f32, budget: f32) -> Self {
        // Where the sides are too short for the whole easing, it eases less.
        let smoothing = smoothing.min(budget / r - 1.0).max(0.0);
        let p = ((1.0 + smoothing) * r).min(budget);
        let arc_deg = 90.0 * (1.0 - smoothing);
        let arc_len = (arc_deg / 2.0).to_radians().sin() * r * std::f32::consts::SQRT_2;
        let alpha = (90.0 - arc_deg) / 2.0;
        let p3_to_p4 = r * (alpha / 2.0).to_radians().tan();
        let beta = (45.0 * smoothing).to_radians();
        let c = p3_to_p4 * beta.cos();
        let d = c * beta.tan();
        let b = (p - arc_len - c - d) / 3.0;
        Self {
            a: 2.0 * b,
            b,
            c,
            d,
            p,
            r,
            arc: arc_deg.to_radians(),
            arc_len,
            beta,
        }
    }

    /// Add the corner starting at `from`, heading along `along` and
    /// turning to head along `turn` (unit vectors along the axes).
    fn add(&self, path: &mut PathBuilder, from: (f32, f32), along: (f32, f32), turn: (f32, f32)) {
        let at = |u: f32, v: f32| {
            (
                from.0 + along.0 * u + turn.0 * v,
                from.1 + along.1 * u + turn.1 * v,
            )
        };
        let Self { a, b, c, d, p, .. } = *self;
        // Easing off the side.
        let (x1, y1) = at(a, 0.0);
        let (x2, y2) = at(a + b, 0.0);
        let (x3, y3) = at(a + b + c, d);
        path.cubic_to(x1, y1, x2, y2, x3, y3);
        // The stretch of circle, as one cubic: its controls along the
        // tangents, 4/3·tan(θ/4) of the radius out.
        // The arc leaves the first curve turned `beta` from the side, and
        // meets the second turned `beta` from the next.
        let k = 4.0 / 3.0 * (self.arc / 4.0).tan() * self.r;
        let (tu, tv) = (self.beta.cos(), self.beta.sin());
        let (eu, ev) = (a + b + c + self.arc_len, d + self.arc_len);
        let (su, sv) = (self.beta.sin(), self.beta.cos());
        let (cx1, cy1) = at(a + b + c + tu * k, d + tv * k);
        let (cx2, cy2) = at(eu - su * k, ev - sv * k);
        let (ex, ey) = at(eu, ev);
        path.cubic_to(cx1, cy1, cx2, cy2, ex, ey);
        // Easing onto the next side, which it meets `p` along.
        let (x1, y1) = at(eu + d, ev + c);
        let (x2, y2) = at(eu + d, ev + b + c);
        let (x3, y3) = at(p, p);
        path.cubic_to(x1, y1, x2, y2, x3, y3);
    }
}

/// A tile of fine noise, made once: specks of light and of dark, each a
/// few hundredths opaque, at random.
fn noise() -> &'static Pixmap {
    static NOISE: std::sync::OnceLock<Pixmap> = std::sync::OnceLock::new();
    NOISE.get_or_init(|| {
        const SIDE: u32 = 96;
        // At most this opaque, of 255: a whisper.
        const MOST: u32 = 9;
        let mut tile = Pixmap::new(SIDE, SIDE)
            .unwrap_or_else(|| Pixmap::new(1, 1).unwrap_or_else(|| unreachable!("a 1 × 1 pixmap")));
        let mut seed: u32 = 0x9e37_79b9;
        for px in tile.data_mut().as_chunks_mut::<4>().0 {
            // xorshift: the same grain every time.
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let alpha = (seed >> 8) % (MOST + 1);
            let light = seed & 1 == 0;
            let v = if light { alpha as u8 } else { 0 };
            px.copy_from_slice(&[v, v, v, alpha as u8]);
        }
        tile
    })
}

/// The result of drawing without a compositor: premultiplied RGBA pixels.
#[derive(Clone, Debug)]
pub struct Offscreen {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Premultiplied RGBA, row-major.
    pub rgba: Vec<u8>,
}

impl Offscreen {
    /// Draw into a fresh transparent buffer of `width × height` logical pixels.
    /// Lets tools render (and snapshot-test) their interface headlessly.
    pub fn render(
        text: &mut TextEngine,
        (width, height): (u32, u32),
        scale: u32,
        draw: impl FnOnce(&mut Canvas<'_>),
    ) -> Self {
        let (width, height) = (width * scale, height * scale);
        let mut rgba = vec![0; width as usize * height as usize * 4];
        draw(&mut Canvas::new(
            &mut rgba,
            width,
            height,
            scale as f32,
            false,
            text,
        ));
        Self {
            width,
            height,
            rgba,
        }
    }

    /// Premultiplied RGBA of the pixel at physical `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// Write the image as a PNG.
    #[cfg(feature = "png")]
    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let size = tiny_skia::IntSize::from_wh(self.width, self.height)
            .ok_or_else(|| std::io::Error::other("empty image"))?;
        let pixmap = tiny_skia::Pixmap::from_vec(self.rgba.clone(), size)
            .ok_or_else(|| std::io::Error::other("pixel buffer does not match its size"))?;
        if let Some(dir) = path.as_ref().parent() {
            std::fs::create_dir_all(dir)?;
        }
        pixmap.save_png(path).map_err(std::io::Error::other)
    }

    /// Write the image to the workspace's `target/snapshots/<name>.png`,
    /// to look at. `crate_dir` is the calling crate's
    /// `env!("CARGO_MANIFEST_DIR")`.
    #[cfg(feature = "png")]
    pub fn snapshot(&self, crate_dir: &str, name: &str) {
        let path = std::path::Path::new(crate_dir)
            .join("../../target/snapshots")
            .join(format!("{name}.png"));
        if let Err(err) = self.save_png(&path) {
            panic!("can't write {}: {err}", path.display());
        }
    }

    /// [`snapshot`](Self::snapshot), and then hold the image to the
    /// crate's reference picture, `<crate_dir>/snapshots/<name>.png`:
    /// panics if more than a sliver of it has changed, writing a picture
    /// of where beside the snapshot. With `ENSO_BLESS=1` set, the image
    /// becomes the reference instead.
    #[cfg(feature = "png")]
    pub fn check(&self, crate_dir: &str, name: &str) {
        self.snapshot(crate_dir, name);
        let reference = std::path::Path::new(crate_dir)
            .join("snapshots")
            .join(format!("{name}.png"));
        if std::env::var_os("ENSO_BLESS").is_some_and(|value| value != "0") {
            if let Err(err) = self.save_png(&reference) {
                panic!("can't write {}: {err}", reference.display());
            }
            return;
        }
        let Ok(expected) = tiny_skia::Pixmap::load_png(&reference) else {
            panic!(
                "no reference picture for {name} ({}): run the tests with ENSO_BLESS=1 to make it",
                reference.display()
            );
        };
        let changed = snapshot::compare(self, expected.width(), expected.height(), expected.data());
        if let Err(problem) = changed {
            let diff = std::path::Path::new(crate_dir)
                .join("../../target/snapshots")
                .join(format!("{name}.diff.png"));
            if let Some(marked) = snapshot::marked(self, expected.data()) {
                let _ = marked.save_png(&diff);
            }
            panic!(
                "{name} looks different: {problem}. Compare target/snapshots/{name}.png with \
                 {} (changes marked in {name}.diff.png); if it's meant, run with ENSO_BLESS=1",
                reference.display()
            );
        }
    }
}

/// Holding an image to its reference.
#[cfg(feature = "png")]
mod snapshot {
    use super::Offscreen;

    /// How far a channel may drift before the pixel counts as changed: what
    /// antialiasing and font hinting move, not what a change of colour does.
    const DRIFT: u8 = 40;
    /// How many pixels may change, in parts per ten thousand: enough for
    /// a glyph's edge to land differently, not for a word to change size.
    const CHANGED: usize = 5;

    fn differs(a: &[u8], b: &[u8]) -> bool {
        a.iter().zip(b).any(|(a, b)| a.abs_diff(*b) > DRIFT)
    }

    /// Whether `image` is close enough to the reference `expected`.
    pub fn compare(
        image: &Offscreen,
        width: u32,
        height: u32,
        expected: &[u8],
    ) -> Result<(), String> {
        if (image.width, image.height) != (width, height) {
            return Err(format!(
                "it's {}×{} where it was {width}×{height}",
                image.width, image.height
            ));
        }
        let pixels = image.rgba.len() / 4;
        let changed = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
            .filter(|(a, b)| differs(*a, *b))
            .count();
        if changed * 10_000 > pixels * CHANGED {
            return Err(format!("{changed} of {pixels} pixels changed"));
        }
        Ok(())
    }

    /// `image` faded, with the pixels that changed from `expected` in
    /// magenta.
    pub fn marked(image: &Offscreen, expected: &[u8]) -> Option<Offscreen> {
        if image.rgba.len() != expected.len() {
            return None;
        }
        let mut rgba = Vec::with_capacity(image.rgba.len());
        for (a, b) in image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
        {
            if differs(a, b) {
                rgba.extend_from_slice(&[255, 0, 255, 255]);
            } else {
                rgba.extend(a.iter().map(|channel| channel / 3));
            }
        }
        Some(Offscreen {
            width: image.width,
            height: image.height,
            rgba,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn image(rgba: Vec<u8>) -> Offscreen {
            Offscreen {
                width: (rgba.len() / 4) as u32,
                height: 1,
                rgba,
            }
        }

        #[test]
        fn drift_is_forgiven_and_change_is_not() {
            let reference = vec![100; 4 * 1000];
            let mut drifted = reference.clone();
            drifted[0] = 130;
            assert!(compare(&image(drifted), 1000, 1, &reference).is_ok());
            let mut changed = reference.clone();
            for pixel in 0..1 {
                changed[pixel * 4] = 250;
            }
            assert!(compare(&image(changed.clone()), 1000, 1, &reference).is_err());
            assert!(compare(&image(reference.clone()), 999, 1, &reference).is_err());
            let marked = marked(&image(changed), &reference).expect("same size");
            assert_eq!(marked.pixel(0, 0), [255, 0, 255, 255]);
            assert_eq!(marked.pixel(10, 0), [33, 33, 33, 33]);
        }
    }
}

#[cfg(test)]
mod clip_tests {
    use super::*;
    use crate::{Offscreen, TextEngine};

    /// A shape clip keeps shapes and text to one side of it, softly, and
    /// within any rectangle there already was.
    #[test]
    fn shapes_and_text_keep_to_a_shape() {
        let mut text = TextEngine::new(&enso_core::Config::default().appearance);
        let red = Rgba::rgb(255, 0, 0);
        let mut left = Shape::new();
        left.move_to(0.0, 0.0)
            .line_to(50.0, 0.0)
            .line_to(50.0, 40.0)
            .line_to(0.0, 40.0)
            .close();
        let frame = Offscreen::render(&mut text, (100, 40), 1, |canvas| {
            canvas.with_clip(Rect::new(0.0, 0.0, 100.0, 20.0), |canvas| {
                canvas.with_clip_shape(&left, true, |canvas| {
                    canvas.fill(Rect::new(0.0, 0.0, 100.0, 40.0), 0.0, red);
                });
                canvas.with_clip_shape(&left, false, |canvas| {
                    let words = Text::new("WWWWWWWWWWWWWWW", 16.0, red);
                    canvas.text(&words, 0.0, 0.0);
                });
            });
        });
        let is_red = |x: u32, y: u32| frame.pixel(x, y)[0] > 200 && frame.pixel(x, y)[1] < 60;
        assert!(is_red(10, 10), "inside the shape and the rectangle");
        assert!(!is_red(10, 30), "below the rectangle");
        // Right of the shape, only text, and only above the rectangle's edge.
        assert!((50..100).any(|x| (0..20).any(|y| frame.pixel(x, y)[3] > 0)));
        assert!((50..100).all(|x| (20..40).all(|y| frame.pixel(x, y)[3] == 0)));
        // An empty shape: inside it nothing, outside it everything.
        let empty = Shape::new();
        let frame = Offscreen::render(&mut text, (10, 10), 1, |canvas| {
            canvas.with_clip_shape(&empty, true, |canvas| {
                canvas.fill(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, red);
            });
        });
        assert!(!is_red_at(&frame, 5, 5));
        let frame = Offscreen::render(&mut text, (10, 10), 1, |canvas| {
            canvas.with_clip_shape(&empty, false, |canvas| {
                canvas.fill(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, red);
            });
        });
        assert!(is_red_at(&frame, 5, 5));
    }

    fn is_red_at(frame: &Offscreen, x: u32, y: u32) -> bool {
        frame.pixel(x, y)[0] > 200 && frame.pixel(x, y)[1] < 60
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod corner_tests {
    use super::*;

    /// Points along a path's curves, to look at where it goes.
    fn outline(path: &Path) -> Vec<(f32, f32)> {
        let mut points = Vec::new();
        let mut last = (0.0, 0.0);
        for segment in path.segments() {
            match segment {
                tiny_skia::PathSegment::MoveTo(p) | tiny_skia::PathSegment::LineTo(p) => {
                    last = (p.x, p.y);
                    points.push(last);
                }
                tiny_skia::PathSegment::CubicTo(c1, c2, p) => {
                    for i in 1..=16 {
                        let t = i as f32 / 16.0;
                        let u = 1.0 - t;
                        let x = u * u * u * last.0
                            + 3.0 * u * u * t * c1.x
                            + 3.0 * u * t * t * c2.x
                            + t * t * t * p.x;
                        let y = u * u * u * last.1
                            + 3.0 * u * u * t * c1.y
                            + 3.0 * u * t * t * c2.y
                            + t * t * t * p.y;
                        points.push((x, y));
                    }
                    last = (p.x, p.y);
                }
                _ => {}
            }
        }
        points
    }

    #[test]
    fn a_pill_keeps_its_round_ends() {
        let path = rounded_rect(Rect::new(0.0, 0.0, 100.0, 28.0), 14.0).unwrap();
        for (x, y) in outline(&path) {
            if x < 13.99 || x > 86.01 {
                let cx = if x < 50.0 { 14.0 } else { 86.0 };
                let d = ((x - cx).powi(2) + (y - 14.0).powi(2)).sqrt();
                assert!(
                    (d - 14.0).abs() < 0.1,
                    "({x}, {y}) is {d} from its end's middle"
                );
            }
        }
    }

    #[test]
    fn a_smoothed_corner_is_softer_than_a_circle_and_stays_in_bounds() {
        let rect = Rect::new(10.0, 20.0, 200.0, 120.0);
        let points = outline(&rounded_rect(rect, 14.0).unwrap());
        for &(x, y) in &points {
            assert!(x >= rect.x - 0.01 && x <= rect.right() + 0.01, "{x}");
            assert!(y >= rect.y - 0.01 && y <= rect.bottom() + 0.01, "{y}");
        }
        // Across its middle it is the circle's arc...
        let near = |want: f32| {
            points
                .iter()
                .filter(|(x, y)| *x > rect.right() - 30.0 && *y < rect.y + 30.0)
                .min_by(|a, b| {
                    (rect.right() - a.0 - want)
                        .abs()
                        .total_cmp(&(rect.right() - b.0 - want).abs())
                })
                .map(|(_, y)| y - rect.y)
                .unwrap()
        };
        let circle = 14.0 * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        assert!(
            (near(circle) - circle).abs() < 0.2,
            "{} vs {circle}",
            near(circle)
        );
        // ...but it starts to turn further from the corner than a circle,
        // which only leaves the side one radius from it: the curve comes on
        // gradually instead of all at once.
        assert!(near(14.0 * 1.3) > 0.01, "{}", near(14.0 * 1.3));
        assert!(near(14.0 * 1.7) < 0.001, "{}", near(14.0 * 1.7));
    }
}
