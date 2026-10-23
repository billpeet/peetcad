//! Laying a string out in a font: each glyph's outline, hinted as SolidWorks draws it,
//! placed along the line and turned into lines and arcs.

use peet_math::DVec2;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, HintingInstance, HintingOptions, OutlineGlyphCollection, OutlinePen,
    Target,
};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use super::arcs::{self, Bezier};
use super::{GlyphCurve, SketchText, TextAlign};

/// The size, in pixels to the em, that SolidWorks draws glyphs at before scaling them to
/// the text's height. Their outlines are hinted at this size (as Windows hints them for
/// black-and-white pixels), so stems and edges sit on its pixel grid, and the capital
/// height is a whole number of pixels: SolidWorks' text has them there.
pub const HINTED_PPEM: f32 = 108.0;

/// A glyph's contours as Bézier pieces, in pixels.
#[derive(Default)]
struct Pieces {
    start: DVec2,
    at: DVec2,
    contours: Vec<Vec<Bezier>>,
}

fn pt(x: f32, y: f32) -> DVec2 {
    DVec2::new(f64::from(x), f64::from(y))
}

impl OutlinePen for Pieces {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = pt(x, y);
        self.at = self.start;
        self.contours.push(Vec::new());
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(vec![self.at, pt(x, y)]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.push(vec![self.at, pt(x1, y1), pt(x, y)]);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.push(vec![self.at, pt(x1, y1), pt(x2, y2), pt(x, y)]);
    }

    fn close(&mut self) {
        if self.at != self.start {
            self.push(vec![self.at, self.start]);
        }
    }
}

impl Pieces {
    fn push(&mut self, points: Vec<DVec2>) {
        self.at = *points.last().unwrap();
        if let Some(c) = self.contours.last_mut() {
            c.push(Bezier(points));
        }
    }

    fn top(&self) -> f64 {
        self.contours
            .iter()
            .flatten()
            .flat_map(|b| b.0.iter().map(|p| p.y))
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// A font's glyphs at a size, hinted if asked and the font can be.
struct Glyphs<'a> {
    font: FontRef<'a>,
    ppem: f32,
    outlines: OutlineGlyphCollection<'a>,
    hinting: Option<HintingInstance>,
}

impl<'a> Glyphs<'a> {
    fn new(font: FontRef<'a>, ppem: f32, hinted: bool) -> Self {
        let outlines = font.outline_glyphs();
        let hinting = hinted
            .then(|| {
                HintingInstance::new(
                    &outlines,
                    Size::new(ppem),
                    LocationRef::default(),
                    HintingOptions {
                        engine: Engine::Interpreter,
                        target: Target::Mono,
                    },
                )
                .ok()
            })
            .flatten();
        Self {
            font,
            ppem,
            outlines,
            hinting,
        }
    }

    fn size(&self) -> Size {
        Size::new(self.ppem)
    }

    fn glyph_id(&self, c: char) -> GlyphId {
        self.font.charmap().map(c).unwrap_or(GlyphId::NOTDEF)
    }

    /// The glyph's outline and advance, in pixels.
    fn draw(&self, g: GlyphId) -> (Pieces, f64) {
        let linear = || {
            self.font
                .glyph_metrics(self.size(), LocationRef::default())
                .advance_width(g)
                .map_or(0.0, f64::from)
        };
        let mut pieces = Pieces::default();
        let Some(glyph) = self.outlines.get(g) else {
            return (pieces, linear());
        };
        if let Some(h) = &self.hinting
            && let Ok(m) = glyph.draw(DrawSettings::hinted(h, false), &mut pieces)
        {
            return (pieces, m.advance_width.map_or_else(linear, f64::from));
        }
        pieces = Pieces::default();
        let _ = glyph.draw(
            DrawSettings::unhinted(self.size(), LocationRef::default()),
            &mut pieces,
        );
        (pieces, linear())
    }

    /// The height of a capital letter in pixels: the top of `H` as drawn, else the
    /// font's own figure.
    fn cap_height(&self) -> f64 {
        let top = self.draw(self.glyph_id('H')).0.top();
        if top.is_finite() && top > 0.0 {
            return top;
        }
        let m = self.font.metrics(self.size(), LocationRef::default());
        m.cap_height
            .map(f64::from)
            .filter(|h| *h > 0.0)
            .unwrap_or(0.7 * f64::from(self.ppem))
    }

    fn line_height(&self) -> f64 {
        let m = self.font.metrics(self.size(), LocationRef::default());
        f64::from(m.ascent - m.descent + m.leading)
    }
}

/// The text's outline in its own coordinates (millimetres from its point, unturned):
/// hinted at [`HINTED_PPEM`] as SolidWorks draws it, or as the font is designed.
pub(super) fn outline(face: FontRef<'_>, t: &SketchText, hinted: bool) -> Vec<GlyphCurve> {
    outline_at(face, t, HINTED_PPEM, hinted)
}

pub(super) fn outline_at(
    face: FontRef<'_>,
    t: &SketchText,
    ppem: f32,
    hinted: bool,
) -> Vec<GlyphCurve> {
    let glyphs = Glyphs::new(face, ppem, hinted);
    let scale = t.height / glyphs.cap_height();
    let line_height = glyphs.line_height() * scale;
    // Fine enough that the area is right to a few parts in a million.
    let tol = (t.height * 1e-4).min(1e-3);
    let lines: Vec<&str> = t
        .string
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    let last = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    let mut out = Vec::new();
    for (row, line) in lines[..last].iter().enumerate() {
        let drawn: Vec<(Pieces, f64)> = line
            .chars()
            .map(|c| glyphs.draw(glyphs.glyph_id(c)))
            .collect();
        let step = |advance: f64| advance * scale * t.width * t.spacing;
        let total: f64 = drawn.iter().map(|(_, a)| step(*a)).sum();
        let mut pen = match t.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => -total / 2.0,
            TextAlign::Right => -total,
        };
        let baseline = -(row as f64) * line_height;
        for (pieces, advance) in drawn {
            let map = |p: &DVec2| DVec2::new(pen + p.x * scale * t.width, baseline + p.y * scale);
            for piece in pieces.contours.iter().flatten() {
                let placed = Bezier(piece.0.iter().map(map).collect());
                arcs::fit(&placed, tol, &mut out);
            }
            pen += step(advance);
        }
    }
    out
}
