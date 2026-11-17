//! Software canvas: rounded shapes, soft shadows, text, and icons drawn in
//! logical pixels onto a premultiplied pixel buffer (tiny-skia).

use enso_core::{
    Icon, Rgba,
    style::{Shadow, radius},
};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder,
    Pattern, PixmapMut, PixmapRef, SpreadMode, Stroke, Transform,
};

use crate::{
    geom::{Align, Rect, Size},
    image::Image,
    picture::Picture,
    text::{Font, Text, TextEngine},
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
}

/// A clip rectangle: as a mask for shapes, and as physical pixel bounds
/// (`x0, y0, x1, y1`, end-exclusive) for text.
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
        }
    }

    /// Canvas size in logical pixels.
    pub fn size(&self) -> Size {
        Size {
            w: self.width as f32 / self.scale,
            h: self.height as f32 / self.scale,
        }
    }

    /// The whole canvas as a rectangle.
    pub fn bounds(&self) -> Rect {
        let Size { w, h } = self.size();
        Rect::new(0.0, 0.0, w, h)
    }

    /// Whether icons render as Nerd Font glyphs.
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

    /// Soft drop shadow for a rounded rectangle. Draw it before the shape.
    pub fn shadow(&mut self, rect: Rect, radius: f32, shadow: Shadow, color: Rgba) {
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
        let (bgra, opacity) = (self.bgra, self.opacity);
        let pixels = &mut *self.pixels;
        self.text
            .rasterize(text, self.scale, origin, |px, py, color| {
                if px < x0 || py < y0 || px >= x1 || py >= y1 {
                    return;
                }
                let i = (py as usize * width as usize + px as usize) * 4;
                blend(
                    &mut pixels[i..i + 4],
                    premultiply(color.alpha(opacity), bgra),
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

    /// Draw an icon aligned in `rect`: a Nerd Font glyph, or its text stand-in
    /// when no Nerd Font is available.
    pub fn icon(&mut self, icon: Icon, size: f32, color: Rgba, rect: Rect, align: Align) -> Size {
        let glyphs = self.glyphs();
        let text = Text::new(
            icon.get(glyphs),
            if glyphs { size } else { size * 0.8 },
            color,
        )
        .font(if glyphs { Font::Icon } else { Font::Ui });
        self.text_in(&text, rect, align)
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

    /// A circular arc around `(cx, cy)`, as a piece of its own: from the
    /// angle `start`, turning through `sweep` (radians; positive turns
    /// clockwise on screen, and 0 points right).
    pub fn arc(&mut self, (cx, cy): (f32, f32), r: f32, start: f32, sweep: f32) -> &mut Self {
        // Each quarter turn or less is one cubic, whose controls sit
        // 4/3·tan(θ/4) of the radius along the tangents.
        let pieces = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0);
        let step = sweep / pieces;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let at = |angle: f32| (cx + r * angle.cos(), cy + r * angle.sin());
        let (x, y) = at(start);
        self.builder.move_to(x, y);
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
    // Control-point offset that makes a cubic Bézier approximate a quarter circle.
    let k = r * 0.447_715_25;
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - k, y, x + w, y + k, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(x + w, y + h - k, x + w - k, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.cubic_to(x + k, y + h, x, y + h - k, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + k, x + k, y, x + r, y);
    path.close();
    path.finish()
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
}
