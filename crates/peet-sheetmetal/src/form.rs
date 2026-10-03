//! Forming features: dimples, embosses and louvers pressed into a flange.
//!
//! A form is a raised (or sunk) plateau on a flat face. The kernel has planes and
//! cylinders, so forms are made with straight walls and sharp corners: the sheet steps
//! up square at the form's outline, a wall one thickness thick runs up to the plateau,
//! and the plateau is the sheet again, `height` higher. A dimple has a round outline
//! (cylinder walls); an emboss has a convex polygon (plane walls). A louver is an emboss
//! with one side lanced open: the sheet is cut along that side and the plateau and the
//! two walls next to it stop there, so it looks into the hood.
//!
//! **In the flat pattern** the form is a hole in its flange's material that the build
//! fills with the form's own faces ([`crate::build`]), so the folded and the flat solids
//! keep the same topology. The flat pattern reports each form ([`FormMark`]) for the
//! drawing: the punch's outline and centre, and the lance a louver needs cut.
//!
//! **Measures.** The outline is the outside of the form's wall, where it leaves the sheet
//! (the punch's size plus the thickness, as a punch press would quote it). `height` is
//! how far the plateau's outer face stands above the sheet's face.

use peet_math::DVec2;
use peet_sketch::Curve;

use crate::layout::{Area, CurveTag, Edge2, Layout, MIN_LENGTH, PieceKind, loop_winding};

/// The outline of a form.
#[derive(Clone, Debug, PartialEq)]
pub enum FormShape {
    Circle {
        center: DVec2,
        radius: f64,
    },
    /// A convex polygon, counter-clockwise.
    Polygon(Vec<DVec2>),
}

/// What kind of form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FormKind {
    /// A round plateau.
    Dimple,
    /// A plateau with a polygon outline.
    Emboss,
    /// A hood lanced open along one side.
    Louver,
}

impl FormKind {
    pub const ALL: [Self; 3] = [Self::Dimple, Self::Emboss, Self::Louver];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dimple => "Dimple",
            Self::Emboss => "Emboss",
            Self::Louver => "Louver",
        }
    }
}

/// A form on a flange, in flat coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub owner: u32,
    /// Tells the forms of one feature apart.
    pub part: u32,
    pub piece: usize,
    pub kind: FormKind,
    pub shape: FormShape,
    pub height: f64,
    /// Stands out of the top side of the sheet (else the bottom side).
    pub up: bool,
    /// Louvers: the side (index into the polygon's edges) that is lanced open.
    pub open_side: Option<usize>,
}

/// Wall numbers of a form's faces (the owner is the form's feature, the part the form).
pub mod face {
    /// Where the wall leaves the sheet: the ring between the outline and the wall's inside.
    pub const FOOT: u8 = 0;
    /// The plateau's outer face.
    pub const TOP: u8 = 1;
    /// The plateau's inner face.
    pub const UNDER: u8 = 2;
    /// A louver's open end: the plateau's edge and the ends of its side walls.
    pub const MOUTH: u8 = 3;
    /// The outline's curves in the flat pattern (the lance of a louver is a wall).
    pub const EDGE: u8 = 8;
    /// The outside of the wall, per side of the outline (two halves for a circle).
    pub const OUTER: u8 = 72;
    /// The inside of the wall, per side.
    pub const INNER: u8 = 136;
    /// How many sides a form can have.
    pub const MAX_SIDES: usize = 60;
}

/// What the flat pattern shows of a form.
#[derive(Clone, Debug, PartialEq)]
pub struct FormMark {
    pub owner: u32,
    pub part: u32,
    pub kind: FormKind,
    pub up: bool,
    pub height: f64,
    /// The outline, in flat coordinates, counter-clockwise.
    pub outline: Vec<(Curve, bool)>,
    pub center: DVec2,
    /// A louver's lance: the line the sheet is cut along.
    pub lance: Option<[DVec2; 2]>,
}

impl Form {
    /// The outline as an area, its sides tagged for the build.
    pub fn area(&self) -> Area {
        let tag = |side: usize| CurveTag::Generated {
            owner: self.owner,
            part: self.part,
            index: face::EDGE + side as u8,
        };
        let edges = match &self.shape {
            FormShape::Circle { center, radius } => vec![Edge2 {
                curve: Curve::Circle {
                    center: *center,
                    radius: *radius,
                },
                reversed: false,
                tag: tag(0),
            }],
            FormShape::Polygon(p) => (0..p.len())
                .map(|i| Edge2::line(p[i], p[(i + 1) % p.len()], tag(i)))
                .collect(),
        };
        Area { loops: vec![edges] }
    }

    /// Which side of the outline a tag belongs to, if it is one of this form's.
    pub fn side_of(&self, tag: CurveTag) -> Option<usize> {
        match tag {
            CurveTag::Generated { owner, part, index }
                if owner == self.owner
                    && part == self.part
                    && (face::EDGE..face::EDGE + face::MAX_SIDES as u8).contains(&index) =>
            {
                Some(usize::from(index - face::EDGE))
            }
            _ => None,
        }
    }

    pub fn center(&self) -> DVec2 {
        match &self.shape {
            FormShape::Circle { center, .. } => *center,
            FormShape::Polygon(p) => p.iter().copied().sum::<DVec2>() / p.len().max(1) as f64,
        }
    }

    /// Points around the outline, `margin` outside it.
    fn ring(&self, margin: f64) -> Vec<DVec2> {
        match &self.shape {
            FormShape::Circle { center, radius } => (0..64)
                .map(|i| {
                    let a = f64::from(i) / 64.0 * std::f64::consts::TAU;
                    *center + DVec2::from_angle(a) * (radius + margin)
                })
                .collect(),
            FormShape::Polygon(p) => {
                let c = self.center();
                let mut out = Vec::new();
                for i in 0..p.len() {
                    let (a, b) = (p[i], p[(i + 1) % p.len()]);
                    for k in 0..8 {
                        let q = a + (b - a) * (f64::from(k) / 8.0);
                        let away = (q - c).normalize_or_zero();
                        out.push(q + away * margin);
                    }
                }
                out
            }
        }
    }
}

impl Layout {
    /// Adds a form on flange `piece`.
    pub fn add_form(&mut self, mut form: Form) -> Result<(), String> {
        let t = self.settings.thickness;
        let what = form.kind.label().to_lowercase();
        let p = self
            .pieces
            .get(form.piece)
            .ok_or("The face is not on this body.")?;
        if !matches!(p.kind, PieceKind::Flange) {
            return Err(format!("Put the {what} on a flat face, not on a bend."));
        }
        if !(form.height.is_finite() && form.height > MIN_LENGTH) {
            return Err(format!("The {what}'s height must be greater than zero."));
        }
        match &mut form.shape {
            FormShape::Circle { radius, .. } => {
                if form.kind != FormKind::Dimple {
                    return Err(format!("A {what} needs a polygon outline, not a circle."));
                }
                if *radius <= t + MIN_LENGTH {
                    return Err(format!(
                        "The dimple is too small for the sheet: its diameter must be more than twice the thickness ({:.3} mm).",
                        2.0 * t
                    ));
                }
            }
            FormShape::Polygon(points) => {
                if form.kind == FormKind::Dimple {
                    return Err("A dimple needs a round outline.".to_owned());
                }
                let n = points.len();
                if !(3..=face::MAX_SIDES).contains(&n) {
                    return Err(format!(
                        "The {what}'s outline must have between 3 and {} sides.",
                        face::MAX_SIDES
                    ));
                }
                let area: f64 = (0..n)
                    .map(|i| points[i].perp_dot(points[(i + 1) % n]))
                    .sum::<f64>()
                    / 2.0;
                if area < 0.0 {
                    points.reverse();
                    if let Some(o) = form.open_side.as_mut() {
                        // Side i ran from point i to i + 1; reversed, it runs from
                        // n - 2 - i to n - 1 - i.
                        *o = (2 * n - 2 - *o) % n;
                    }
                }
                for i in 0..n {
                    let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
                    if (b - a).length() <= 2.0 * t + MIN_LENGTH {
                        return Err(format!(
                            "Side {} of the {what} is too short for the sheet: make it longer than twice the thickness ({:.3} mm).",
                            i + 1,
                            2.0 * t
                        ));
                    }
                    if (b - a).perp_dot(c - b) <= 1e-9 {
                        return Err(format!(
                            "The {what}'s outline must be convex, with no corners turning inwards."
                        ));
                    }
                }
                // The wall's inside must still be a polygon.
                let inner = inset(points, t, None);
                if inner.len() != n || polygon_area(&inner) <= 0.0 {
                    return Err(format!(
                        "The {what} is too small for the sheet's thickness."
                    ));
                }
            }
        }
        match (form.kind, form.open_side) {
            (FormKind::Louver, Some(o)) => {
                let FormShape::Polygon(points) = &form.shape else {
                    unreachable!()
                };
                if o >= points.len() {
                    return Err("The louver's open side isn't one of its sides.".to_owned());
                }
            }
            (FormKind::Louver, None) => {
                return Err("Say which side of the louver is open.".to_owned());
            }
            (_, Some(_)) => form.open_side = None,
            _ => {}
        }
        // Clear of the face's edges, its trims, cuts and other forms by a thickness.
        let material = |q: DVec2| {
            p.outline.contains(q)
                && !p.trims.iter().any(|a| a.contains(q))
                && !self
                    .cuts
                    .iter()
                    .any(|c| c.applies_to(form.piece) && c.area.contains(q))
        };
        if !form.ring(t).into_iter().all(material) {
            return Err(format!(
                "The {what} runs off the face, into a cut or too close to an edge or a bend: keep it at least the thickness ({t:.3} mm) clear."
            ));
        }
        for other in self.forms.iter().filter(|f| f.piece == form.piece) {
            let a = other.area();
            if form.ring(t).into_iter().any(|q| a.contains(q))
                || other.ring(t).into_iter().any(|q| form.area().contains(q))
            {
                return Err(format!(
                    "The {what} runs into another form: keep them at least the thickness apart."
                ));
            }
        }
        self.forms.push(form);
        Ok(())
    }
}

/// A convex counter-clockwise polygon moved in by `by` (each side, except `open`).
pub(crate) fn inset(points: &[DVec2], by: f64, open: Option<usize>) -> Vec<DVec2> {
    let n = points.len();
    let mut out = Vec::new();
    for i in 0..n {
        // The corner at point i, between side i - 1 and side i.
        let prev = (i + n - 1) % n;
        let line = |s: usize| {
            let (a, b) = (points[s], points[(s + 1) % n]);
            let d = (b - a).normalize_or_zero();
            let inward = DVec2::new(-d.y, d.x);
            let shift = if Some(s) == open { 0.0 } else { by };
            (a + inward * shift, d)
        };
        let ((p1, d1), (p2, d2)) = (line(prev), line(i));
        let den = d1.perp_dot(d2);
        if den.abs() < 1e-12 {
            out.push(p2);
            continue;
        }
        let s = (p2 - p1).perp_dot(d2) / den;
        out.push(p1 + d1 * s);
    }
    out
}

fn polygon_area(p: &[DVec2]) -> f64 {
    (0..p.len())
        .map(|i| p[i].perp_dot(p[(i + 1) % p.len()]))
        .sum::<f64>()
        / 2.0
}

/// Whether `q` is inside the closed loop of a mark's outline.
pub fn mark_contains(mark: &FormMark, q: DVec2) -> bool {
    let edges: Vec<Edge2> = mark
        .outline
        .iter()
        .map(|&(curve, reversed)| Edge2 {
            curve,
            reversed,
            tag: CurveTag::Generated {
                owner: 0,
                part: 0,
                index: 0,
            },
        })
        .collect();
    loop_winding(&edges, q) != 0
}
