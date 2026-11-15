//! Text shaping and rasterisation (cosmic-text), including Nerd Font icons.

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Weight as FontWeight, Wrap,
};
use enso_core::{
    Rgba,
    config::{Appearance, IconMode},
    style::type_scale,
};

use crate::geom::Size;

/// Font weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Weight {
    #[default]
    Regular,
    Medium,
    Bold,
}

/// Which font a run of text uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Font {
    /// The configured interface font.
    #[default]
    Ui,
    /// The system monospace font.
    Mono,
    /// The detected Nerd Font, for icon glyphs.
    Icon,
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
        }
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
pub struct TextEngine {
    fonts: FontSystem,
    cache: SwashCache,
    family: String,
    icon_family: Option<String>,
    glyphs: bool,
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine")
            .field("family", &self.family)
            .field("icon_family", &self.icon_family)
            .field("glyphs", &self.glyphs)
            .finish_non_exhaustive()
    }
}

impl TextEngine {
    /// Load system fonts and resolve the configured interface font and icon mode.
    pub fn new(appearance: &Appearance) -> Self {
        let mut fonts = FontSystem::new();
        resolve_generic_families(&mut fonts);
        let icon_family = find_nerd_font(&fonts);
        let glyphs = match appearance.icons {
            IconMode::Auto => icon_family.is_some(),
            IconMode::Glyphs => true,
            IconMode::Text => false,
        };
        Self {
            fonts,
            cache: SwashCache::new(),
            family: appearance.font.clone(),
            icon_family,
            glyphs,
        }
    }

    /// Whether icons render as Nerd Font glyphs (as opposed to text stand-ins).
    pub fn glyphs(&self) -> bool {
        self.glyphs
    }

    /// Family name of the Nerd Font used for icons, if one is installed.
    pub fn icon_family(&self) -> Option<&str> {
        self.icon_family.as_deref()
    }

    /// Size the text would occupy, in logical pixels.
    pub fn measure(&mut self, text: &Text<'_>) -> Size {
        let buffer = self.layout(text, 1.0);
        extent(&buffer)
    }

    /// Shape `text` at `scale` (physical pixels per logical pixel).
    fn layout(&mut self, text: &Text<'_>, scale: f32) -> Buffer {
        let Some(width) = text.max_width else {
            return self.shape(text, scale, None, false);
        };
        let width = (width * scale).max(1.0);
        if text.max_lines == 1 {
            return self.shape(text, scale, Some(width), true);
        }
        // cosmic-text's multi-line ellipsis path keeps the space a wrapped
        // last line starts with, so only use it when truncation is needed.
        let wrapped = self.shape(text, scale, Some(width), false);
        if wrapped.layout_runs().count() <= text.max_lines {
            wrapped
        } else {
            self.shape(text, scale, Some(width), true)
        }
    }

    fn shape(
        &mut self,
        text: &Text<'_>,
        scale: f32,
        width: Option<f32>,
        ellipsize: bool,
    ) -> Buffer {
        let metrics = Metrics::new(text.size * scale, text.line_height() * scale);
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_wrap(if width.is_some() && text.max_lines > 1 {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        if ellipsize {
            buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(text.max_lines)));
        }
        buffer.set_size(width, None);

        let family = match text.font {
            Font::Ui => family_of(&self.family),
            Font::Mono => Family::Monospace,
            Font::Icon => self
                .icon_family
                .as_deref()
                .map_or(Family::SansSerif, Family::Name),
        };
        let weight = match text.weight {
            Weight::Regular => FontWeight::NORMAL,
            Weight::Medium => FontWeight::MEDIUM,
            Weight::Bold => FontWeight::BOLD,
        };
        let attrs = Attrs::new().family(family).weight(weight);
        buffer.set_text(text.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
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
        let mut buffer = self.layout(text, scale);
        let Rgba { r, g, b, a } = text.color;
        buffer.draw(
            &mut self.fonts,
            &mut self.cache,
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
        let size = extent(&buffer);
        Size {
            w: size.w / scale,
            h: size.h / scale,
        }
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

/// Pick the best installed Nerd Font family for icon glyphs: the symbols-only
/// font if present, otherwise any patched font.
fn find_nerd_font(fonts: &FontSystem) -> Option<String> {
    let mut best: Option<&str> = None;
    for face in fonts.db().faces() {
        for (family, _) in &face.families {
            if !family.contains("Nerd Font") {
                continue;
            }
            if family.starts_with("Symbols Nerd Font") {
                return Some(family.clone());
            }
            best.get_or_insert(family);
        }
    }
    best.map(str::to_owned)
}
