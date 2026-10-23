//! Sketch text: a string drawn in a font, whose glyph outlines are closed shapes that
//! features take like any other (a cut of a label, an embossed number).
//!
//! As in SolidWorks, a text has a position (a point of the sketch, which relations and
//! dimensions can hold), a height (of its capital letters), an angle, a font, a width
//! factor, a character spacing and an alignment. The point is on the baseline of the
//! first line: at its left end, its middle or its right end as the text is aligned left,
//! centre or right. Further lines go below, a line height apart.
//!
//! **Outlines.** The glyphs are read from the TrueType or OpenType font of that name
//! installed on the machine ([`font`]); where it isn't installed, from a built-in one
//! ([`font::FALLBACK`]). Their curves become lines and arcs within a small tolerance
//! ([`arcs`]), because sheet metal takes nothing else. The outline is kept with the text,
//! in its own coordinates, so a part looks the same on a machine without the font; it is
//! drawn again only when the string or the style changes ([`SketchText::layout`]). Moving
//! or turning the text moves its outline ([`SketchText::curves`]).

pub mod arcs;
pub mod font;
mod glyphs;

use peet_math::DVec2;
use serde::{Deserialize, Serialize};

use crate::curve::Curve;
pub use glyphs::HINTED_PPEM;

/// Where a text's point is along its lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    /// The point is at the left end of each line.
    #[default]
    Left,
    /// The point is at the middle of each line.
    Center,
    /// The point is at the right end of each line.
    Right,
}

impl TextAlign {
    pub fn word(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "left" => Some(Self::Left),
            "center" | "centre" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// A piece of a glyph outline, in the text's own coordinates (millimetres from its point,
/// before it is turned).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GlyphCurve {
    Line {
        a: DVec2,
        b: DVec2,
    },
    /// Counter-clockwise, `sweep` in `(0, 2π)`.
    Arc {
        center: DVec2,
        radius: f64,
        start_angle: f64,
        sweep: f64,
    },
}

impl GlyphCurve {
    /// The curve moved to `at` after turning it by `angle` (radians) about the origin.
    pub fn placed(&self, at: DVec2, angle: f64) -> Curve {
        let rot = DVec2::from_angle(angle);
        let map = |p: DVec2| at + rot.rotate(p);
        match *self {
            Self::Line { a, b } => Curve::Line {
                a: map(a),
                b: map(b),
            },
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => Curve::Arc {
                center: map(center),
                radius,
                start_angle: crate::curve::normalize_angle(start_angle + angle),
                sweep,
            },
        }
    }

    fn distance(&self, p: DVec2) -> f64 {
        self.placed(DVec2::ZERO, 0.0).distance(p)
    }
}

/// A sketch text: its string, its style, and its outline as last drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchText {
    /// The text. A line feed starts a new line.
    pub string: String,
    /// The font's name: a family ("Century Gothic") or a face ("Century Gothic Bold").
    pub font: String,
    /// The height of a capital letter, in millimetres.
    pub height: f64,
    /// Counter-clockwise from the sketch's x axis, in radians.
    pub angle: f64,
    /// Width factor: 1 as the font has it, 2 twice as wide.
    pub width: f64,
    /// Character spacing: 1 as the font has it, 2 twice as far apart.
    pub spacing: f64,
    pub align: TextAlign,
    /// The font the outline was drawn in: `font`, or the built-in one if `font` was not
    /// installed.
    pub drawn_in: String,
    /// The glyph outlines, in the text's own coordinates.
    pub outline: Vec<GlyphCurve>,
}

/// SolidWorks' default font for sketch text.
pub const DEFAULT_FONT: &str = "Century Gothic";

impl SketchText {
    /// A text in the default style (Century Gothic, 5 mm high), not yet drawn.
    pub fn new(string: &str) -> Self {
        Self {
            string: string.to_owned(),
            font: DEFAULT_FONT.to_owned(),
            height: 5.0,
            angle: 0.0,
            width: 1.0,
            spacing: 1.0,
            align: TextAlign::Left,
            drawn_in: String::new(),
            outline: Vec::new(),
        }
    }

    /// The outline placed with the text's point at `at` and turned by its angle.
    pub fn curves(&self, at: DVec2) -> impl Iterator<Item = Curve> + '_ {
        self.outline.iter().map(move |c| c.placed(at, self.angle))
    }

    /// Draws the outline again from the string and the style. Returns a warning when the
    /// font named is not installed and the built-in one was used.
    pub fn layout(&mut self) -> Result<Option<String>, String> {
        if !(self.height.is_finite() && self.height > 0.0) {
            return Err("A text's height must be greater than zero.".to_owned());
        }
        if !(self.width.is_finite() && self.width > 0.0) {
            return Err("A text's width factor must be greater than zero.".to_owned());
        }
        if !(self.spacing.is_finite() && self.spacing > 0.0) {
            return Err("A text's spacing must be greater than zero.".to_owned());
        }
        let (data, found) = font::font(&self.font);
        let face = data
            .face()
            .ok_or_else(|| format!("The font file of {} can't be read.", data.name))?;
        // An installed font is drawn as SolidWorks draws it; the built-in one, which
        // SolidWorks doesn't have, as it is designed.
        self.outline = glyphs::outline(face, self, data.name != font::FALLBACK);
        self.drawn_in.clone_from(&data.name);
        Ok((!found).then(|| {
            format!(
                "The font '{}' is not installed: the text is drawn in {}.",
                self.font,
                font::FALLBACK
            )
        }))
    }
}

#[cfg(test)]
mod tests;
