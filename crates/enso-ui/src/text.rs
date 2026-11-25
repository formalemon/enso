//! Text shaping and rasterisation (cosmic-text).

use std::{
    cell::OnceCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    mem,
};

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FeatureTag, FontFeatures,
    FontSystem, Metrics, Shaping, SwashCache, Weight as FontWeight, Wrap,
};
use enso_core::{
    Rgba,
    config::{Appearance, IconMode},
    style::type_scale,
};

use crate::geom::Size;

/// Font weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Weight {
    #[default]
    Regular,
    Medium,
    Bold,
}

/// Which font a run of text uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Font {
    /// The configured interface font.
    #[default]
    Ui,
    /// The configured monospace font.
    Mono,
}

/// A run of text and how to draw it.
#[derive(Clone, Copy, Debug)]
pub struct Text<'a> {
    pub text: &'a str,
    /// Font size in logical pixels.
    pub size: f32,
    pub color: Rgba,
    pub weight: Weight,
    pub font: Font,
    /// Wrap or truncate (with an ellipsis) beyond this width.
    pub max_width: Option<f32>,
    /// Lines to show before truncating. Only meaningful with `max_width`.
    pub max_lines: usize,
    /// Figures all the same width (OpenType `tnum`), where the font has
    /// them: a clock or a count keeps its width as its digits change.
    pub tabular: bool,
}

impl<'a> Text<'a> {
    pub fn new(text: &'a str, size: f32, color: Rgba) -> Self {
        Self {
            text,
            size,
            color,
            weight: Weight::Regular,
            font: Font::Ui,
            max_width: None,
            max_lines: 1,
            tabular: false,
        }
    }

    /// Figures all the same width, so the words keep their width as the
    /// digits in them change: clocks, percentages, counts.
    #[must_use]
    pub fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }

    #[must_use]
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    #[must_use]
    pub fn bold(self) -> Self {
        self.weight(Weight::Bold)
    }

    #[must_use]
    pub fn medium(self) -> Self {
        self.weight(Weight::Medium)
    }

    #[must_use]
    pub fn font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    /// Truncate with an ellipsis beyond `width`.
    #[must_use]
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }

    /// Wrap within `max_width`, truncating after `lines` lines.
    #[must_use]
    pub fn max_lines(mut self, lines: usize) -> Self {
        self.max_lines = lines.max(1);
        self
    }

    /// Height of one line box.
    pub fn line_height(&self) -> f32 {
        (self.size * type_scale::LINE_HEIGHT).round()
    }
}

/// Owns the font database and glyph cache. One per process.
///
/// The system's fonts are only looked at when text is first measured or
/// drawn, so an interface with no words in it — a wallpaper — never pays
/// for them.
pub struct TextEngine {
    family: String,
    mono: String,
    icons: IconMode,
    loaded: OnceCell<Loaded>,
}

/// The fonts, once something needed them.
struct Loaded {
    fonts: FontSystem,
    cache: SwashCache,
    layouts: Layouts,
    families: Families,
}

/// The families each [`Font`] stands for, resolved against what is
/// installed: a configured family that is not falls back to its generic.
struct Families {
    ui: String,
    mono: String,
}

impl Families {
    fn of(&self, font: Font) -> Family<'_> {
        match font {
            Font::Ui => family_of(&self.ui),
            Font::Mono => family_of(&self.mono),
        }
    }
}

impl Loaded {
    fn new(family: &str, mono: &str) -> Self {
        let mut fonts = FontSystem::new();
        resolve_generic_families(&mut fonts);
        let installed = |name: &str, generic: &str| {
            let known = matches!(family_of(name), Family::Name(_)).then(|| {
                fonts
                    .db()
                    .faces()
                    .any(|face| face.families.iter().any(|(family, _)| family == name))
            });
            if known == Some(false) {
                eprintln!("enso: font \"{name}\" is not installed; using {generic}");
                generic.to_owned()
            } else {
                name.to_owned()
            }
        };
        let families = Families {
            ui: installed(family, "sans-serif"),
            mono: installed(mono, "monospace"),
        };
        Self {
            fonts,
            cache: SwashCache::new(),
            layouts: Layouts::default(),
            families,
        }
    }
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine")
            .field("family", &self.family)
            .field("mono", &self.mono)
            .field("loaded", &self.loaded.get().is_some())
            .finish_non_exhaustive()
    }
}

impl TextEngine {
    /// Resolve the configured interface fonts and icon mode against the
    /// system's fonts — on first use.
    pub fn new(appearance: &Appearance) -> Self {
        Self {
            family: appearance.font.clone(),
            mono: appearance.mono_font.clone(),
            icons: appearance.icons,
            loaded: OnceCell::new(),
        }
    }

    fn loaded(&self) -> &Loaded {
        self.loaded
            .get_or_init(|| Loaded::new(&self.family, &self.mono))
    }

    fn loaded_mut(&mut self) -> &mut Loaded {
        self.loaded();
        self.loaded
            .get_mut()
            .expect("the fonts were loaded on the line above")
    }

    /// Whether icons are drawn (as opposed to stood in for by words). Icons
    /// need no font, so this does not load any.
    pub fn glyphs(&self) -> bool {
        self.icons != IconMode::Text
    }

    /// Size the text would occupy, in logical pixels.
    pub fn measure(&mut self, text: &Text<'_>) -> Size {
        let Loaded {
            fonts,
            layouts,
            families,
            ..
        } = self.loaded_mut();
        extent(layout(fonts, layouts, families, text, 1.0))
    }

    /// Rasterise `text` with its top-left corner at physical pixel `(x, y)`,
    /// calling `put(px, py, colour)` for every covered pixel.
    pub(crate) fn rasterize(
        &mut self,
        text: &Text<'_>,
        scale: f32,
        (x, y): (i32, i32),
        mut put: impl FnMut(i32, i32, Rgba),
    ) -> Size {
        let Loaded {
            fonts,
            cache,
            layouts,
            families,
            ..
        } = self.loaded_mut();
        let buffer = layout(fonts, layouts, families, text, scale);
        let Rgba { r, g, b, a } = text.color;
        buffer.draw(
            fonts,
            cache,
            Color::rgba(r, g, b, a),
            |gx, gy, w, h, color| {
                let color = Rgba::new(color.r(), color.g(), color.b(), color.a());
                if color.a == 0 {
                    return;
                }
                for dy in 0..h as i32 {
                    for dx in 0..w as i32 {
                        put(x + gx + dx, y + gy + dy, color);
                    }
                }
            },
        );
        let size = extent(buffer);
        Size {
            w: size.w / scale,
            h: size.h / scale,
        }
    }
}

/// `text` shaped at `scale` (physical pixels per logical pixel): from the
/// cache when it was shaped lately, shaped now when not.
fn layout<'a>(
    fonts: &mut FontSystem,
    layouts: &'a mut Layouts,
    families: &Families,
    text: &Text<'_>,
    scale: f32,
) -> &'a mut Buffer {
    let form = Form::of(text, scale);
    let key = form.key(text.text);
    if layouts.has(key, text.text, form) {
        return layouts.get(key, text.text, form);
    }
    let buffer = shape_fitted(fonts, families, text, scale);
    layouts.insert(key, text.text, form, buffer)
}

/// Shape `text`, fitted to its width and lines.
fn shape_fitted(
    fonts: &mut FontSystem,
    families: &Families,
    text: &Text<'_>,
    scale: f32,
) -> Buffer {
    let Some(width) = text.max_width else {
        return shape(fonts, families, text, scale, None, false);
    };
    if text.max_lines == 1 {
        // Whether one line fits is decided where text is measured — a pixel
        // to a logical pixel — and with half a pixel to spare. Text laid out
        // in the width it was measured at is then never cut short: not by a
        // rounding in whoever worked the width out (padding added and taken
        // off again), nor by the same words coming out a little wider at
        // another scale, where letters are hinted to other pixels.
        let natural = shape(fonts, families, text, 1.0, None, false);
        if extent(&natural).w <= width + 0.5 {
            return if scale == 1.0 {
                natural
            } else {
                shape(fonts, families, text, scale, None, false)
            };
        }
        let width = (width * scale).max(1.0);
        return shape(fonts, families, text, scale, Some(width), true);
    }
    let width = (width * scale).max(1.0);
    // cosmic-text's multi-line ellipsis path keeps the space a wrapped
    // last line starts with, so only use it when truncation is needed.
    let wrapped = shape(fonts, families, text, scale, Some(width), false);
    if wrapped.layout_runs().count() <= text.max_lines {
        wrapped
    } else {
        shape(fonts, families, text, scale, Some(width), true)
    }
}

fn shape(
    fonts: &mut FontSystem,
    families: &Families,
    text: &Text<'_>,
    scale: f32,
    width: Option<f32>,
    ellipsize: bool,
) -> Buffer {
    let metrics = Metrics::new(text.size * scale, text.line_height() * scale);
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_wrap(if width.is_some() && text.max_lines > 1 {
        Wrap::WordOrGlyph
    } else {
        Wrap::None
    });
    if ellipsize {
        buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(text.max_lines)));
    }
    buffer.set_size(width, None);
    let weight = match text.weight {
        Weight::Regular => FontWeight::NORMAL,
        Weight::Medium => FontWeight::MEDIUM,
        Weight::Bold => FontWeight::BOLD,
    };
    let mut attrs = Attrs::new().family(families.of(text.font)).weight(weight);
    if text.tabular {
        let mut features = FontFeatures::new();
        features.enable(FeatureTag::new(b"tnum"));
        attrs = attrs.font_features(features);
    }
    buffer.set_text(text.text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts, false);
    buffer
}

/// Everything that decides how a run of text is shaped, but its words: not
/// its colour, which only matters when the shape is painted. Sizes are kept
/// as bits, so the form can be hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Form {
    size: u32,
    scale: u32,
    max_width: Option<u32>,
    max_lines: usize,
    weight: Weight,
    font: Font,
    tabular: bool,
}

impl Form {
    fn of(text: &Text<'_>, scale: f32) -> Self {
        Self {
            size: text.size.to_bits(),
            scale: scale.to_bits(),
            max_width: text.max_width.map(f32::to_bits),
            max_lines: text.max_lines,
            weight: text.weight,
            font: text.font,
            tabular: text.tabular,
        }
    }

    fn key(self, text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// Text already shaped, kept between frames.
///
/// Shaping — finding each glyph, kerning, wrapping — is the dearest part of
/// drawing words, and an interface draws the same words frame after frame
/// while it animates: only their colour and place change, and neither is
/// part of the shape. Each label is also measured before it is drawn, at
/// two scales. So a shape is kept, by a hash of its words and [`Form`], and
/// checked against both on the way out; a lookup allocates nothing.
///
/// Two generations bound it: once the fresh one is full it becomes the
/// stale one, and the old stale one is dropped. A shape used since then is
/// moved back to fresh, so what is on screen stays; what is not goes within
/// two turnovers. Every step is O(1).
#[derive(Default)]
struct Layouts {
    fresh: HashMap<u64, Shaped>,
    stale: HashMap<u64, Shaped>,
}

struct Shaped {
    text: Box<str>,
    form: Form,
    buffer: Buffer,
}

impl Shaped {
    fn is(&self, text: &str, form: Form) -> bool {
        self.form == form && *self.text == *text
    }
}

/// Shapes in a generation: comfortably more than any one interface shows —
/// a file manager's full window of rows and columns among them — so
/// scrolling back over what was just on screen finds it still shaped.
const GENERATION: usize = 1024;

impl Layouts {
    /// Whether this is shaped, promoting it to fresh if it was stale.
    fn has(&mut self, key: u64, text: &str, form: Form) -> bool {
        if self
            .fresh
            .get(&key)
            .is_some_and(|shaped| shaped.is(text, form))
        {
            return true;
        }
        match self.stale.remove(&key) {
            Some(shaped) if shaped.is(text, form) => {
                self.keep(key, shaped);
                true
            }
            _ => false,
        }
    }

    /// A shape [`has`](Self::has) just found.
    fn get(&mut self, key: u64, text: &str, form: Form) -> &mut Buffer {
        let shaped = self
            .fresh
            .get_mut(&key)
            .filter(|shaped| shaped.is(text, form))
            .expect("`has` found it, and nothing has happened since");
        &mut shaped.buffer
    }

    fn insert(&mut self, key: u64, text: &str, form: Form, buffer: Buffer) -> &mut Buffer {
        let shaped = Shaped {
            text: text.into(),
            form,
            buffer,
        };
        &mut self.keep(key, shaped).buffer
    }

    fn keep(&mut self, key: u64, shaped: Shaped) -> &mut Shaped {
        if self.fresh.len() >= GENERATION && !self.fresh.contains_key(&key) {
            self.stale = mem::take(&mut self.fresh);
        }
        self.fresh.entry(key).insert_entry(shaped).into_mut()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.fresh.len() + self.stale.len()
    }
}

fn family_of(name: &str) -> Family<'_> {
    match name {
        "sans-serif" | "sans" | "" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" | "mono" => Family::Monospace,
        other => Family::Name(other),
    }
}

fn extent(buffer: &Buffer) -> Size {
    buffer
        .layout_runs()
        .fold(Size::default(), |size, run| Size {
            w: size.w.max(run.line_w),
            h: size.h.max(run.line_top + run.line_height),
        })
}

/// Point the generic `sans-serif` and `monospace` families at fonts that are
/// actually installed. The font database only knows hard-coded defaults for
/// them, and when those are missing every weight falls back to an arbitrary
/// face — so pick from a preference list instead.
fn resolve_generic_families(fonts: &mut FontSystem) {
    const SANS: &[&str] = &[
        "Inter",
        "Adwaita Sans",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
        "FreeSans",
    ];
    const MONO: &[&str] = &[
        "JetBrains Mono",
        "Adwaita Mono",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "FreeMono",
    ];
    let db = fonts.db_mut();
    if let Some(name) = missing_generic(db, Family::SansSerif, SANS) {
        db.set_sans_serif_family(name);
    }
    if let Some(name) = missing_generic(db, Family::Monospace, MONO) {
        db.set_monospace_family(name);
    }
}

/// If `generic` resolves to a family that is not installed, the first
/// installed family from `preferred`.
fn missing_generic(
    db: &cosmic_text::fontdb::Database,
    generic: Family<'_>,
    preferred: &[&'static str],
) -> Option<&'static str> {
    let installed = |name: &str| {
        db.faces()
            .any(|face| face.families.iter().any(|(family, _)| family == name))
    };
    if installed(db.family_name(&generic)) {
        return None;
    }
    preferred.iter().copied().find(|name| installed(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> TextEngine {
        TextEngine::new(&Appearance::default())
    }

    #[test]
    fn a_shape_is_made_once_and_kept() {
        let mut engine = engine();
        let text = Text::new("Soft pastel zen", 14.0, Rgba::rgb(0, 0, 0));
        let size = engine.measure(&text);
        assert_eq!(engine.loaded().layouts.len(), 1);
        // Colour is not part of the shape: the same entry serves.
        let again = engine.measure(&Text {
            color: Rgba::rgb(255, 0, 0),
            ..text
        });
        assert_eq!(again, size);
        assert_eq!(engine.loaded().layouts.len(), 1);
        // Anything that changes the shape is a shape of its own.
        let bold = engine.measure(&text.bold());
        assert!(bold.w > size.w, "{bold:?} {size:?}");
        let narrow = engine.measure(&text.max_width(40.0));
        assert!(narrow.w <= 40.0, "{narrow:?}");
        assert_eq!(engine.loaded().layouts.len(), 3);
    }

    #[test]
    fn the_cache_stays_bounded_and_keeps_what_is_in_use() {
        let mut engine = engine();
        let keep = Text::new("on screen", 14.0, Rgba::rgb(0, 0, 0));
        for i in 0..GENERATION * 5 {
            let label = i.to_string();
            engine.measure(&Text::new(&label, 14.0, Rgba::rgb(0, 0, 0)));
            // Drawn every "frame", so it survives every turnover.
            engine.measure(&keep);
        }
        let layouts = &mut engine.loaded_mut().layouts;
        assert!(layouts.len() <= 2 * GENERATION, "{}", layouts.len());
        let form = Form::of(&keep, 1.0);
        assert!(layouts.has(form.key(keep.text), keep.text, form));
    }

    /// Text laid out within the width it was measured at is never cut
    /// short: the bar's clock measures itself and is drawn at that width,
    /// at every minute of the day, at any scale.
    #[test]
    fn text_fits_the_width_it_was_measured_at() {
        let mut cut = Vec::new();
        for font in ["", "Google Sans Flex"] {
            let appearance = Appearance {
                font: if font.is_empty() {
                    Appearance::default().font
                } else {
                    font.into()
                },
                ..Appearance::default()
            };
            let mut engine = TextEngine::new(&appearance);
            for minute in 0..24 * 60 {
                let says = format!("Wed 25 Nov  {:02}:{:02}", minute / 60, minute % 60);
                // The clock's size, at the body sizes enso offers.
                for size in [12.04f32, 13.76] {
                    let text = Text::new(&says, size, Rgba::rgb(0, 0, 0)).medium();
                    let measured = engine.measure(&text).w;
                    for scale in [1.0f32, 1.25, 1.5] {
                        let Loaded {
                            fonts,
                            layouts,
                            families,
                            ..
                        } = engine.loaded_mut();
                        let whole = extent(layout(fonts, layouts, families, &text, scale)).w;
                        // The bar's own sum: padding added round the words
                        // for the island, and taken off again for them.
                        let padded = (measured + 2.0 * 16.0) - 2.0 * 16.0;
                        let fitted = text.max_width(padded);
                        let drawn = extent(layout(fonts, layouts, families, &fitted, scale)).w;
                        if drawn < whole - 0.5 {
                            cut.push(format!(
                                "{font:?} {says} {size}px at {scale}: {drawn} < {whole}"
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            cut.is_empty(),
            "{} cut short, e.g. {:?}",
            cut.len(),
            &cut[..cut.len().min(4)]
        );
    }

    #[test]
    fn tabular_figures_keep_a_clock_one_width() {
        for font in ["", "Google Sans Flex"] {
            let appearance = Appearance {
                font: if font.is_empty() {
                    Appearance::default().font
                } else {
                    font.into()
                },
                ..Appearance::default()
            };
            let mut engine = TextEngine::new(&appearance);
            let width = |engine: &mut TextEngine, says: &str| {
                let text = Text::new(says, 12.0, Rgba::rgb(0, 0, 0)).medium();
                engine.measure(&text.tabular()).w
            };
            let ones = width(&mut engine, "11:11");
            let eights = width(&mut engine, "08:58");
            assert!((ones - eights).abs() < 0.01, "{font:?}: {ones} {eights}");
        }
    }

    #[test]
    fn a_family_that_is_not_installed_falls_back_to_its_kind() {
        let appearance = Appearance {
            font: "No Such Font Anywhere".into(),
            mono_font: "Nor This One".into(),
            ..Appearance::default()
        };
        let engine = TextEngine::new(&appearance);
        let families = &engine.loaded().families;
        assert_eq!(families.ui, "sans-serif");
        assert_eq!(families.mono, "monospace");
    }
}
