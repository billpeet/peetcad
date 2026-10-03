//! The sheet definition: flanges and bends laid out in flat-pattern coordinates.
//!
//! **Flat coordinates.** Everything is described in one 2D coordinate system, the flat
//! pattern's, plus a thickness coordinate `z` from 0 (the *bottom* side of the sheet) to
//! `t` (the *top* side). A [`Layout`] is a list of [`Piece`]s: flat regions that are
//! either *flanges* (flat in the folded part too) or *bends* (strips that wrap around a
//! cylinder when folded). Each piece knows where its flat coordinates go in the model:
//!
//! - a flange carries a rigid placement (`frame`: model from flat);
//! - a bend carries its strip (an origin on the parent-side line, the direction along the
//!   bend line, the direction across it into the bend, its length and its width, which is
//!   the bend allowance) and folds from its parent flange's placement.
//!
//! The first piece is the fixed flange: its placement is also where the flat pattern is
//! shown. A bend's child flange is placed by the parent's placement composed with the
//! bend's fold ([`Bend::fold_frame`]), so the whole part is a tree unrolled into the plane.
//!
//! **Areas.** Piece outlines, trims (parts of a piece given up to a bend) and cuts are
//! [`Area`]s: closed loops of tagged lines and arcs. Overlaps are resolved when the body is
//! built ([`crate::build`]), so a flange only has to say "this strip of my parent is mine
//! now" rather than edit its parent's outline.

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::Curve;
use peet_sketch::region::{Loop, LoopEdge, Region};

use crate::corner::Corner;
use crate::flange::Attachment;
use crate::form::Form;
use crate::settings::{BendModel, BendValues, SheetSettings};

/// Who made a piece: an owner (the feature) and a key within it, for naming its faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Origin {
    pub owner: u32,
    pub part: u32,
}

/// What a boundary curve of the flat pattern is, which names the wall it becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CurveTag {
    /// A curve of the owner's sketch (a base flange's outline, a cut).
    Sketch { owner: u32, entity: u32 },
    /// An edge the owner made (a flange's tip and sides, a relief), numbered within it.
    Generated { owner: u32, part: u32, index: u8 },
    /// A copy of a sketch curve made by a pattern or a mirror (the owner): which copy,
    /// and the curve's entity in the copied feature's sketch.
    Copy { owner: u32, copy: u32, entity: u32 },
}

impl CurveTag {
    pub fn owner(self) -> u32 {
        match self {
            Self::Sketch { owner, .. }
            | Self::Generated { owner, .. }
            | Self::Copy { owner, .. } => owner,
        }
    }
}

impl Area {
    /// The area with every curve's tag changed by `f`.
    pub fn retagged(mut self, f: impl Fn(CurveTag) -> CurveTag) -> Self {
        for e in self.loops.iter_mut().flatten() {
            e.tag = f(e.tag);
        }
        self
    }
}

/// One edge of an area's loop.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge2 {
    pub curve: Curve,
    /// Traversed against the curve's own direction (arcs are always counter-clockwise).
    pub reversed: bool,
    pub tag: CurveTag,
}

impl Edge2 {
    pub fn line(a: DVec2, b: DVec2, tag: CurveTag) -> Self {
        Self {
            curve: Curve::Line { a, b },
            reversed: false,
            tag,
        }
    }
}

/// A region of the flat pattern: closed loops. A point is inside when an odd number of
/// loops enclose it, so an outer loop with hole loops is a plate with holes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Area {
    pub loops: Vec<Vec<Edge2>>,
}

impl Area {
    /// A polygon through `points`, edge `i` (from point `i` to `i + 1`) tagged `tag(i)`.
    pub fn polygon(points: &[DVec2], tag: impl Fn(usize) -> CurveTag) -> Self {
        let n = points.len();
        let edges = (0..n)
            .map(|i| Edge2::line(points[i], points[(i + 1) % n], tag(i)))
            .collect();
        Self { loops: vec![edges] }
    }

    /// The outer and hole loops of sketch regions, tagged with their sketch curves.
    pub fn from_regions(regions: &[Region], owner: u32) -> Self {
        let mut loops = Vec::new();
        for r in regions {
            for l in std::iter::once(&r.outer).chain(&r.holes) {
                loops.push(
                    l.edges
                        .iter()
                        .map(|e| Edge2 {
                            curve: e.curve.clone(),
                            reversed: e.reversed,
                            tag: CurveTag::Sketch {
                                owner,
                                entity: e.entity.0,
                            },
                        })
                        .collect(),
                );
            }
        }
        Self { loops }
    }

    /// Whether `p` is inside (points on the boundary give either answer).
    pub fn contains(&self, p: DVec2) -> bool {
        let mut inside = false;
        for l in &self.loops {
            if loop_winding(l, p) != 0 {
                inside = !inside;
            }
        }
        inside
    }

    /// Every curve, for building the arrangement.
    pub fn edges(&self) -> impl Iterator<Item = &Edge2> {
        self.loops.iter().flatten()
    }

    /// The area moved by `f` (a 2D rigid motion or reflection).
    pub fn mapped(&self, f: &impl Fn(DVec2) -> DVec2) -> Self {
        Self {
            loops: self
                .loops
                .iter()
                .map(|l| l.iter().map(|e| map_edge(e, f)).collect())
                .collect(),
        }
    }
}

/// Winding number of a loop of edges around `p`.
pub(crate) fn loop_winding(edges: &[Edge2], p: DVec2) -> i32 {
    let l = Loop {
        edges: edges
            .iter()
            .map(|e| LoopEdge {
                entity: peet_sketch::EntityId(0),
                curve: e.curve.clone(),
                reversed: e.reversed,
            })
            .collect(),
        signed_area: 0.0,
    };
    l.winding_number(p)
}

/// An edge moved by a 2D rigid motion or a reflection (arcs stay counter-clockwise).
fn map_edge(e: &Edge2, f: &impl Fn(DVec2) -> DVec2) -> Edge2 {
    let curve = match e.curve {
        // A rigid motion or a reflection maps control points to control points; the
        // curve keeps its own direction, as a line does.
        Curve::Spline(ref s) => Curve::Spline(s.mapped(f)),
        Curve::Line { a, b } => Curve::Line { a: f(a), b: f(b) },
        Curve::Circle { center, radius } => Curve::Circle {
            center: f(center),
            radius,
        },
        Curve::Arc { center, sweep, .. } => {
            let c = f(center);
            let start = f(e.curve.start());
            let end = f(e.curve.end());
            // The middle is less than half a turn from the start, so the turn from start
            // to middle tells whether the motion kept the arc counter-clockwise.
            let mid = f(e.curve.point_at(0.5));
            let ccw = (start - c).perp_dot(mid - c) > 0.0;
            // A reflection makes it clockwise: start it at the other end and traverse it
            // the other way.
            let s = if ccw { start } else { end };
            return Edge2 {
                curve: Curve::Arc {
                    center: c,
                    radius: s.distance(c),
                    start_angle: (s - c).to_angle(),
                    sweep,
                },
                reversed: e.reversed == ccw,
                tag: e.tag,
            };
        }
    };
    Edge2 { curve, ..*e }
}

/// A bend: a strip of the flat pattern that wraps around a cylinder when folded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bend {
    /// The flange the bend starts from, and the one it carries (none at the end of a
    /// rolled hem, which ends in the bend).
    pub parent: usize,
    pub child: Option<usize>,
    /// The start of the bend's parent-side line, in flat coordinates.
    pub origin: DVec2,
    /// Unit direction along the bend line.
    pub along: DVec2,
    /// Unit direction across the strip, from the parent into the bend.
    pub across: DVec2,
    /// Length along the bend line.
    pub length: f64,
    pub values: BendValues,
    /// Folds towards the top side (`z = t`), which is then the inside of the bend.
    pub up: bool,
}

impl Bend {
    /// Width of the strip: the bend allowance.
    pub fn width(&self) -> f64 {
        self.values.allowance
    }

    fn along3(&self) -> DVec3 {
        self.along.extend(0.0)
    }

    fn across3(&self) -> DVec3 {
        self.across.extend(0.0)
    }

    /// Height of the bend axis above the bottom side of the sheet.
    pub fn axis_z(&self) -> f64 {
        if self.up {
            self.values.thickness + self.values.radius
        } else {
            -self.values.radius
        }
    }

    /// Distance from the axis of a point at height `z`.
    pub fn radius_at(&self, z: f64) -> f64 {
        if self.up {
            self.values.thickness + self.values.radius - z
        } else {
            self.values.radius + z
        }
    }

    /// The signed rotation about `along` that turns `across` towards the folding side by
    /// `angle`.
    fn rotation(&self, angle: f64) -> DQuat {
        let c = self.along.perp_dot(self.across); // (along × across)·z, ±1
        let side = if self.up { 1.0 } else { -1.0 };
        DQuat::from_axis_angle(self.along3(), side * c * angle)
    }

    /// The fold angle reached at distance `s` across the strip.
    pub fn angle_at(&self, s: f64) -> f64 {
        s / self.width() * self.values.angle
    }

    /// Distance across the strip and along the bend line of a flat point.
    pub fn strip_coords(&self, p: DVec2) -> (f64, f64) {
        let q = p - self.origin;
        (q.dot(self.across), q.dot(self.along))
    }

    /// Folds a flat point of the strip into the parent flange's flat coordinates
    /// (continued past its edge).
    pub fn fold_point(&self, p: DVec3) -> DVec3 {
        let (s, w) = self.strip_coords(p.truncate());
        let base = self.origin.extend(0.0) + self.along3() * w;
        let axis = base + DVec3::Z * self.axis_z();
        let at_line = base + DVec3::Z * p.z;
        axis + self.rotation(self.angle_at(s)) * (at_line - axis)
    }

    /// A flat direction at distance `s` across the strip, folded (for wall normals).
    pub fn fold_vector(&self, s: f64, v: DVec3) -> DVec3 {
        self.rotation(self.angle_at(s)) * v
    }

    /// The fold axis in the parent's flat coordinates: a point and the unit direction.
    pub fn axis(&self) -> (DVec3, DVec3) {
        (
            self.origin.extend(0.0) + DVec3::Z * self.axis_z(),
            self.along3(),
        )
    }

    /// The rigid motion that places the child flange's flat coordinates (beyond the
    /// strip) in the parent's: shift back by the strip width, then turn about the axis.
    pub fn fold_frame(&self) -> Frame {
        let rot = self.rotation(self.values.angle);
        let c = self.axis().0;
        Frame {
            origin: c - rot * (c + self.across3() * self.width()),
            rotation: rot,
        }
    }

    /// The direction the bend goes, seen from the top side: "up" or "down".
    pub fn direction_label(&self) -> &'static str {
        if self.up { "Up" } else { "Down" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PieceKind {
    Flange,
    Bend(Bend),
}

/// A flange or a bend of the flat pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub origin: Origin,
    pub kind: PieceKind,
    pub outline: Area,
    /// Parts of the outline given up (to a bend that starts inside the flange).
    pub trims: Vec<Area>,
    /// Model from flat: where this flange is (for a bend, its parent's placement).
    pub frame: Frame,
}

impl Piece {
    pub fn bend(&self) -> Option<&Bend> {
        match &self.kind {
            PieceKind::Bend(b) => Some(b),
            PieceKind::Flange => None,
        }
    }

    pub fn is_flange(&self) -> bool {
        matches!(self.kind, PieceKind::Flange)
    }
}

/// Material removed through the thickness.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub area: Area,
    /// The pieces the cut applies to: those that existed when it was made, plus the pieces
    /// later split off them (by a sketched bend), which keep their holes.
    pub pieces: Vec<usize>,
}

impl Cut {
    pub fn applies_to(&self, piece: usize) -> bool {
        self.pieces.contains(&piece)
    }
}

/// An open profile's line, in sketch coordinates, in chain order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainLine {
    pub entity: u32,
    pub a: DVec2,
    pub b: DVec2,
}

/// Where an edge flange goes: a straight boundary edge of a flange, in flat coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSite {
    pub piece: usize,
    /// The edge runs from `a` to `b` with the flange's material on its left.
    pub a: DVec2,
    pub b: DVec2,
    /// Whether the edge was picked on the top side of the sheet (the flange then bends
    /// towards that side).
    pub top: bool,
}

/// Wall numbers within an edge flange (part 0), for naming its faces.
pub mod wall {
    pub const TIP: u8 = 0;
    pub const FLANGE_START: u8 = 1;
    pub const FLANGE_END: u8 = 2;
    pub const FLANGE_FOLD: u8 = 3;
    pub const BEND_START: u8 = 4;
    pub const BEND_END: u8 = 5;
    pub const BEND_PARENT: u8 = 6;
    pub const BEND_CHILD: u8 = 7;
    pub const TRIM: u8 = 8;
    pub const RELIEF_START: u8 = 12;
    pub const RELIEF_END: u8 = 16;
    /// Open profiles: the walls along the depth sides, the two ends, and bend sides.
    pub const PROFILE_SIDE_LOW: u8 = 0;
    pub const PROFILE_SIDE_HIGH: u8 = 1;
    pub const PROFILE_START: u8 = 2;
    pub const PROFILE_END: u8 = 3;
    pub const PROFILE_FOLD_START: u8 = 4;
    pub const PROFILE_FOLD_END: u8 = 5;
    pub const PROFILE_BEND_LOW: u8 = 6;
    pub const PROFILE_BEND_HIGH: u8 = 7;
    pub const PROFILE_BEND_FOLD_START: u8 = 8;
    pub const PROFILE_BEND_FOLD_END: u8 = 9;
}

/// A sheet metal body's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub settings: SheetSettings,
    pub pieces: Vec<Piece>,
    pub cuts: Vec<Cut>,
    /// The flanges and hems added on edges, in order: where they sit, for finding the
    /// corners where two of them meet.
    pub attachments: Vec<Attachment>,
    /// Where two of those flanges meet, and how the corner is treated.
    pub corners: Vec<Corner>,
    /// Dimples, embosses and louvers pressed into flanges.
    pub forms: Vec<Form>,
}

/// A rectangle in a 2D frame `(o, u, v)`: `o + x·u + y·v` for `x` in `x0..x1`, `y` in
/// `y0..y1`. Edges in order: `y = y0`, `x = x1`, `y = y1`, `x = x0`.
pub(crate) fn rect(
    o: DVec2,
    u: DVec2,
    v: DVec2,
    x: [f64; 2],
    y: [f64; 2],
    tags: [CurveTag; 4],
) -> Area {
    let p = |a: f64, b: f64| o + u * a + v * b;
    Area::polygon(
        &[p(x[0], y[0]), p(x[1], y[0]), p(x[1], y[1]), p(x[0], y[1])],
        |i| tags[i],
    )
}

pub(crate) const MIN_LENGTH: f64 = 100.0 * tolerance::LINEAR;

impl Layout {
    /// A flat plate: the regions of a closed sketch on `plane`, thickened along the plane
    /// normal (against it when `reverse` is set).
    pub fn plate(
        settings: SheetSettings,
        owner: u32,
        plane: &Plane,
        regions: &[Region],
        reverse: bool,
    ) -> Result<Self, String> {
        settings.check()?;
        if regions.is_empty() {
            return Err("The sketch has no closed region.".to_owned());
        }
        let mut frame = plane.frame;
        if reverse {
            frame.origin -= plane.normal() * settings.thickness;
        }
        Ok(Self {
            settings,
            pieces: vec![Piece {
                origin: Origin { owner, part: 0 },
                kind: PieceKind::Flange,
                outline: Area::from_regions(regions, owner),
                trims: Vec::new(),
                frame,
            }],
            cuts: Vec::new(),
            attachments: Vec::new(),
            corners: Vec::new(),
            forms: Vec::new(),
        })
    }

    /// A profile of connected lines on `plane`, with a bend at every corner, extruded
    /// along the plane normal over `depth` (offsets along the normal, low then high).
    /// The lines are one face of the material; the thickness goes to their left (seen
    /// from the front of the plane, walking the chain) or to their right with `flip`.
    pub fn open_profile(
        settings: SheetSettings,
        owner: u32,
        plane: &Plane,
        chain: &[ChainLine],
        depth: [f64; 2],
        flip: bool,
    ) -> Result<Self, String> {
        settings.check()?;
        let t = settings.thickness;
        if chain.is_empty() {
            return Err("The profile has no lines.".to_owned());
        }
        let finite = depth[0].is_finite() && depth[1].is_finite();
        if !finite || depth[1] - depth[0] <= MIN_LENGTH {
            return Err("The depth must be greater than zero.".to_owned());
        }
        let dirs: Vec<DVec2> = chain
            .iter()
            .map(|l| (l.b - l.a).normalize_or_zero())
            .collect();
        let side = if flip { -1.0 } else { 1.0 };
        // Bends at the corners.
        let mut bends = Vec::new();
        for k in 0..chain.len().saturating_sub(1) {
            let (d0, d1) = (dirs[k], dirs[k + 1]);
            let cross = d0.perp_dot(d1);
            let angle = cross.abs().atan2(d0.dot(d1));
            if angle < 1e-6 {
                return Err(format!(
                    "Lines {} and {} of the profile are in line: join them into one line.",
                    k + 1,
                    k + 2
                ));
            }
            // Turning towards the material bends towards the top side (z = t).
            let up = (cross > 0.0) == (side > 0.0);
            let values = BendValues::new(settings.model, angle, settings.radius, t)
                .map_err(|m| format!("Corner {}: {m}", k + 1))?;
            // The sketch line is the outside of the bend when it turns towards the material.
            let setback = if up {
                values.outside_setback()
            } else {
                values.inside_setback()
            };
            bends.push((values, up, setback));
        }
        let mut flat_len = Vec::new();
        for (i, l) in chain.iter().enumerate() {
            let before = if i > 0 { bends[i - 1].2 } else { 0.0 };
            let after = bends.get(i).map_or(0.0, |b| b.2);
            let f = l.a.distance(l.b) - before - after;
            if f <= MIN_LENGTH {
                return Err(format!(
                    "Line {} of the profile is too short for its bends: it needs to be longer than {:.3} mm.",
                    i + 1,
                    before + after
                ));
            }
            flat_len.push(f);
        }

        // Placement of the first flange: x along the first line, z towards the material.
        let n = plane.normal();
        let x3 = plane.frame.vector_to_world(dirs[0].extend(0.0));
        let left = DVec2::new(-dirs[0].y, dirs[0].x) * side;
        let z3 = plane.frame.vector_to_world(left.extend(0.0));
        let origin = plane.from_plane_coords(chain[0].a);
        let mut frame = Frame::from_origin_z_x(origin, z3, x3)
            .ok_or("The profile's first line has no length.")?;
        let sigma = frame.y_axis().dot(n).signum();
        let y = if sigma > 0.0 {
            [depth[0], depth[1]]
        } else {
            [-depth[1], -depth[0]]
        };

        let gen_tag = |part: u32, index: u8| CurveTag::Generated { owner, part, index };
        let mut pieces = Vec::new();
        let mut x = 0.0;
        let (o, u, v) = (DVec2::ZERO, DVec2::X, DVec2::Y);
        for (i, l) in chain.iter().enumerate() {
            let part = l.entity;
            let last = i + 1 == chain.len();
            let tags = [
                gen_tag(part, wall::PROFILE_SIDE_LOW),
                gen_tag(
                    part,
                    if last {
                        wall::PROFILE_END
                    } else {
                        wall::PROFILE_FOLD_END
                    },
                ),
                gen_tag(part, wall::PROFILE_SIDE_HIGH),
                gen_tag(
                    part,
                    if i == 0 {
                        wall::PROFILE_START
                    } else {
                        wall::PROFILE_FOLD_START
                    },
                ),
            ];
            pieces.push(Piece {
                origin: Origin { owner, part },
                kind: PieceKind::Flange,
                outline: rect(o, u, v, [x, x + flat_len[i]], y, tags),
                trims: Vec::new(),
                frame,
            });
            x += flat_len[i];
            if last {
                break;
            }
            let (values, up, _) = bends[i];
            let flange = pieces.len() - 1;
            let bend = Bend {
                parent: flange,
                child: Some(flange + 2),
                origin: DVec2::new(x, y[0]),
                along: DVec2::Y,
                across: DVec2::X,
                length: y[1] - y[0],
                values,
                up,
            };
            let bpart = chain[i + 1].entity;
            let tags = [
                gen_tag(bpart, wall::PROFILE_BEND_LOW),
                gen_tag(bpart, wall::PROFILE_BEND_FOLD_END),
                gen_tag(bpart, wall::PROFILE_BEND_HIGH),
                gen_tag(bpart, wall::PROFILE_BEND_FOLD_START),
            ];
            pieces.push(Piece {
                origin: Origin { owner, part: bpart },
                kind: PieceKind::Bend(bend),
                outline: rect(o, u, v, [x, x + values.allowance], y, tags),
                trims: Vec::new(),
                frame,
            });
            frame = frame.compose(&bend.fold_frame());
            x += values.allowance;
        }
        Ok(Self {
            settings,
            pieces,
            cuts: Vec::new(),
            attachments: Vec::new(),
            corners: Vec::new(),
            forms: Vec::new(),
        })
    }

    /// Cuts `area` (in flat coordinates) out of every piece that exists now.
    pub fn add_cut(&mut self, area: Area) {
        self.cuts.push(Cut {
            area,
            pieces: (0..self.pieces.len()).collect(),
        });
    }

    /// The pieces that are bends, with their index.
    pub fn bends(&self) -> impl Iterator<Item = (usize, &Bend)> {
        self.pieces
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.bend().map(|b| (i, b)))
    }

    /// The bend model, for reports.
    pub fn model(&self) -> BendModel {
        self.settings.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn bend(up: bool) -> Bend {
        Bend {
            parent: 0,
            child: Some(2),
            origin: DVec2::new(10.0, 0.0),
            along: DVec2::Y,
            across: DVec2::X,
            length: 20.0,
            values: BendValues::new(BendModel::KFactor(0.5), FRAC_PI_2, 2.0, 1.0).unwrap(),
            up,
        }
    }

    #[test]
    fn fold_is_continuous_with_the_child_flange() {
        for up in [true, false] {
            let b = bend(up);
            let f = b.fold_frame();
            for z in [0.0, 0.4, 1.0] {
                for w in [0.0, 7.0] {
                    let at_end = DVec3::new(10.0 + b.width(), w, z);
                    let folded = b.fold_point(at_end);
                    assert!(
                        folded.abs_diff_eq(f.to_world(at_end), 1e-12),
                        "{up} {z} {w}"
                    );
                    // At the parent line nothing moves.
                    let start = DVec3::new(10.0, w, z);
                    assert!(b.fold_point(start).abs_diff_eq(start, 1e-12));
                }
            }
        }
    }

    #[test]
    fn right_angle_fold_up() {
        // R = 2, t = 1: inner face at R from the axis, which sits at z = 3 above x = 10.
        let b = bend(true);
        let f = b.fold_frame();
        // The child flange's bottom side (outside of the bend) is vertical at x = 10 + 3.
        let p = f.to_world(DVec3::new(10.0 + b.width() + 5.0, 0.0, 0.0));
        assert!(p.abs_diff_eq(DVec3::new(13.0, 0.0, 8.0), 1e-12), "{p}");
        let q = f.to_world(DVec3::new(10.0 + b.width() + 5.0, 0.0, 1.0));
        assert!(q.abs_diff_eq(DVec3::new(12.0, 0.0, 8.0), 1e-12), "{q}");
        // Down: mirrored about the sheet.
        let d = bend(false).fold_frame();
        let p = d.to_world(DVec3::new(10.0 + b.width() + 5.0, 0.0, 1.0));
        assert!(p.abs_diff_eq(DVec3::new(13.0, 0.0, -7.0), 1e-12), "{p}");
    }

    #[test]
    fn area_parity() {
        let outer = Area::polygon(
            &[
                DVec2::ZERO,
                DVec2::new(10.0, 0.0),
                DVec2::new(10.0, 10.0),
                DVec2::new(0.0, 10.0),
            ],
            |_| CurveTag::Generated {
                owner: 0,
                part: 0,
                index: 0,
            },
        );
        let mut a = outer.clone();
        a.loops.push(vec![Edge2 {
            curve: Curve::Circle {
                center: DVec2::new(5.0, 5.0),
                radius: 2.0,
            },
            reversed: true,
            tag: CurveTag::Sketch {
                owner: 0,
                entity: 1,
            },
        }]);
        assert!(a.contains(DVec2::new(1.0, 1.0)));
        assert!(!a.contains(DVec2::new(5.0, 5.0)));
        assert!(!a.contains(DVec2::new(11.0, 5.0)));
        // A reflection keeps arcs counter-clockwise.
        let arc = Edge2 {
            curve: Curve::arc_from_points(DVec2::ZERO, DVec2::X, DVec2::Y),
            reversed: false,
            tag: CurveTag::Sketch {
                owner: 0,
                entity: 0,
            },
        };
        let m = map_edge(&arc, &|p: DVec2| DVec2::new(p.x, -p.y));
        let Curve::Arc { sweep, .. } = m.curve else {
            panic!()
        };
        assert!((sweep - FRAC_PI_2).abs() < 1e-12);
        assert!(m.reversed);
        assert!(m.curve.start().abs_diff_eq(DVec2::new(0.0, -1.0), 1e-12));
    }
}
