//! The flat pattern report: one row per bend, plus the flat size.

use peet_math::DVec2;

use crate::build::SheetBody;
use crate::layout::Origin;
use crate::settings::BendModel;

/// One bend of the flat pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendRow {
    /// Index of the bend's piece in the layout.
    pub piece: usize,
    /// The feature (and part of it) that made the bend.
    pub origin: Origin,
    /// Towards the top side of the sheet (the side the flat pattern is seen from).
    pub up: bool,
    /// The angle the flange turns through, in degrees.
    pub angle: f64,
    /// Inner radius.
    pub radius: f64,
    pub k_factor: f64,
    pub allowance: f64,
    pub deduction: f64,
    /// Length of the bend line where there is material.
    pub length: f64,
}

/// The flat pattern's numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub thickness: f64,
    pub model: BendModel,
    /// Bends in the order they were made (a sensible first guess at a bending order).
    pub bends: Vec<BendRow>,
    /// Size of the flat blank's bounding box (flat X and Y).
    pub flat_size: DVec2,
    /// Area of the flat blank (cutouts removed).
    pub flat_area: f64,
    /// Separate pieces of the blank (1 for a sound part).
    pub pieces: usize,
    pub cutouts: usize,
}

impl SheetBody {
    pub fn report(&self) -> Report {
        let t = self.layout.settings.thickness;
        let bends = self
            .layout
            .bends()
            .map(|(piece, b)| {
                let length = self
                    .bend_lines
                    .iter()
                    .find(|l| l.piece == piece)
                    .map_or(0.0, |l| {
                        l.segments.iter().map(|s| s[0].distance(s[1])).sum()
                    });
                BendRow {
                    piece,
                    origin: self.layout.pieces[piece].origin,
                    up: b.up,
                    angle: b.values.angle.to_degrees(),
                    radius: b.values.radius,
                    k_factor: b.values.k_factor(),
                    allowance: b.values.allowance,
                    deduction: b.values.deduction(),
                    length,
                }
            })
            .collect();
        let (lo, hi) = self.flat_bounds();
        Report {
            thickness: t,
            model: self.layout.settings.model,
            bends,
            flat_size: if lo.x <= hi.x { hi - lo } else { DVec2::ZERO },
            flat_area: peet_kernel::validate::measure::volume(&self.flat) / t,
            pieces: self.outline.iter().filter(|l| l.outer).count(),
            cutouts: self.outline.iter().filter(|l| !l.outer).count(),
        }
    }
}

impl Report {
    /// The report as tab-separated text (pastes into a spreadsheet), with owners named by
    /// `name`.
    pub fn to_text(&self, name: impl Fn(Origin) -> String) -> String {
        let mut s = format!(
            "Flat size\t{:.3} x {:.3} mm\nArea\t{:.1} mm²\nThickness\t{:.3} mm\nCutouts\t{}\n\n",
            self.flat_size.x, self.flat_size.y, self.flat_area, self.thickness, self.cutouts
        );
        s.push_str("Bend\tFeature\tDirection\tAngle (°)\tInner radius (mm)\tK-factor\tBend allowance (mm)\tBend deduction (mm)\tLength (mm)\n");
        for (i, b) in self.bends.iter().enumerate() {
            s.push_str(&format!(
                "{}\t{}\t{}\t{:.2}\t{:.3}\t{:.4}\t{:.4}\t{:.4}\t{:.3}\n",
                i + 1,
                name(b.origin),
                if b.up { "Up" } else { "Down" },
                b.angle,
                b.radius,
                b.k_factor,
                b.allowance,
                b.deduction,
                b.length
            ));
        }
        s
    }
}
