//! Structural members: SolidWorks' weldment structural member. A profile (a tube's or a
//! beam's section, drawn in a sketch) swept along every line of one or more path sketches,
//! each line a body of its own.
//!
//! **Where the profile sits.** The profile sketch's origin is put on the line (it is the
//! pierce point), and the sketch is turned square to the line by the smallest turn that
//! takes its plane's normal to the line's direction (half a turn about the sketch's Y axis
//! for a line running against the normal). Lines that run the same way get the profile
//! the same way round; `rotate` turns it about the line.
//!
//! **Ends.** Two lines of the feature that meet end to end at an angle are mitred: each
//! member is cut where the plane halving the angle crosses it. A line that ends on a
//! member of an earlier structural member is trimmed to that member's outside: to the side
//! of its profile the line meets it from (SolidWorks' trim between groups). Any other end
//! is square, at the end of the line.
//!
//! **Names.** Each member's faces carry [`FaceRole::Instance`] with the member's number
//! (its path sketch's place in the list times 2^20, plus its line's id): the walls are
//! [`FaceRole::Side`] of the profile's curves, the ends [`FaceRole::NearCap`] (at the
//! line's start) and [`FaceRole::FarCap`].

use std::sync::Arc;

use peet_kernel::boolean::{BooleanOp, boolean_traced};
use peet_kernel::extrude::{ExtrudeFace, extrude_traced};
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::region::Region;
use peet_sketch::{Curve, Sketch};
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::stamp;
use crate::feature::{FeatureId, Scalar};
use crate::naming::{Body, FaceName, FaceRole};
use crate::sheet::Applied;

/// How two members of one feature end where their lines meet at an angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberCorner {
    /// Both cut on the plane halving the angle.
    Miter,
    /// Both square, at the end of their lines.
    Plain,
}

impl MemberCorner {
    pub const ALL: [Self; 2] = [Self::Miter, Self::Plain];
}

/// A profile swept along the lines of path sketches, a body for each line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuralMemberFeature {
    /// The section, drawn with its origin where the line goes through it.
    pub profile: FeatureId,
    /// Sketches whose lines (not construction lines) the members follow.
    pub paths: Vec<FeatureId>,
    pub corner: MemberCorner,
    /// Trim members that end on members of earlier structural members to their outside.
    pub trim: bool,
    /// Degrees the profile is turned about each line.
    pub rotate: Scalar,
}

impl StructuralMemberFeature {
    pub fn new(profile: FeatureId, paths: Vec<FeatureId>) -> Self {
        Self {
            profile,
            paths,
            corner: MemberCorner::Miter,
            trim: true,
            rotate: Scalar::new(0.0),
        }
    }
}

/// One member: its line and how its profile sits on it.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub start: DVec3,
    pub end: DVec3,
    /// At `start`: X and Y are the profile's, Z runs along the line.
    pub frame: Frame,
    /// The member's number, for its faces' names.
    pub number: u32,
}

impl Member {
    fn dir(&self) -> DVec3 {
        self.frame.z_axis()
    }

    fn length(&self) -> f64 {
        self.start.distance(self.end)
    }
}

/// A feature's members and its profile, laid out.
pub(crate) struct Layout {
    pub members: Vec<Member>,
    pub regions: Vec<Region>,
    /// How far the profile reaches from its origin.
    pub reach: f64,
}

/// The lines of a path sketch (not its construction lines), in the model, with their ids.
pub(crate) fn sketch_lines(plane: &Plane, sketch: &Sketch) -> Vec<(u32, DVec3, DVec3)> {
    sketch
        .entities()
        .filter(|(_, e)| !e.construction)
        .filter_map(|(id, _)| match sketch.curve(id) {
            Some(Curve::Line { a, b }) => Some((
                id.0,
                plane.frame.to_world(a.extend(0.0)),
                plane.frame.to_world(b.extend(0.0)),
            )),
            _ => None,
        })
        .collect()
}

/// Lays out a structural member: the profile (its plane and sketch), the lines of each
/// path (a sketch's or a 3D sketch's, with their ids), and the turn about each line in
/// degrees.
pub(crate) fn layout(
    profile: (&Plane, &Sketch),
    paths: &[Vec<(u32, DVec3, DVec3)>],
    rotate: f64,
) -> Result<Layout, FeatureError> {
    let regions = crate::extrude::selected_regions(profile.1, &crate::RegionSelection::Auto, false)
        .map_err(|e| FeatureError(format!("The profile: {}", e.0)))?;
    let reach = regions
        .iter()
        .flat_map(|r| &r.outer.edges)
        .flat_map(|e| curve_points(&e.curve))
        .map(DVec2::length)
        .fold(0.0, f64::max);
    let normal = profile.0.normal();
    let (xs, ys) = (profile.0.frame.x_axis(), profile.0.frame.y_axis());
    let mut members = Vec::new();
    for (pi, lines) in paths.iter().enumerate() {
        for &(id, start, end) in lines {
            let Some(dir) = (end - start).try_normalize() else {
                continue;
            };
            // The smallest turn that takes the profile's normal to the line
            let turn = if normal.dot(dir) < -1.0 + 1e-12 {
                DQuat::from_axis_angle(ys, std::f64::consts::PI)
            } else {
                DQuat::from_rotation_arc(normal, dir)
            };
            let about = DQuat::from_axis_angle(dir, rotate.to_radians());
            let x = about * (turn * xs);
            let y = about * (turn * ys);
            let frame = Frame::from_origin_z_x(start, x.cross(y), x)
                .ok_or_else(|| FeatureError("A line's direction is degenerate.".to_owned()))?;
            members.push(Member {
                start,
                end,
                frame,
                number: ((pi as u32) << 20) | (id & 0xfffff),
            });
        }
    }
    if members.is_empty() {
        return Err(FeatureError(
            "The path sketches have no lines: draw the lines the members follow.".to_owned(),
        ));
    }
    Ok(Layout {
        members,
        regions,
        reach,
    })
}

fn curve_points(c: &Curve) -> Vec<DVec2> {
    match *c {
        Curve::Line { a, b } => vec![a, b],
        // The point of the circle furthest from the origin
        Curve::Circle { center, radius } | Curve::Arc { center, radius, .. } => {
            vec![center.normalize_or(DVec2::X) * (center.length() + radius.abs())]
        }
        Curve::Spline(_) => (0..=16).map(|k| c.point_at(f64::from(k) / 16.0)).collect(),
    }
}

/// Where a member is cut at one end.
#[derive(Clone, Copy, Debug)]
enum Cut {
    /// Square to the line, this far along it from its start.
    Square(f64),
    /// On a plane through `point`; the member keeps the side `normal` points away from.
    Plane { point: DVec3, normal: DVec3 },
}

/// How far along the ray from `from` in `dir` (unit) it last leaves the profile's outer
/// loops, with the direction across the curve it leaves by (`None` for a round one).
fn exit(regions: &[Region], from: DVec2, dir: DVec2) -> Option<(f64, Option<DVec2>)> {
    let mut best: Option<(f64, Option<DVec2>)> = None;
    let mut take = |t: f64, across: Option<DVec2>| {
        if t > 1e-9 && best.is_none_or(|(b, _)| t > b) {
            best = Some((t, across));
        }
    };
    for r in regions {
        for e in &r.outer.edges {
            match e.curve {
                Curve::Line { a, b } => {
                    let d = b - a;
                    let den = dir.perp_dot(d);
                    if den.abs() < 1e-12 {
                        continue;
                    }
                    let t = (a - from).perp_dot(d) / den;
                    let s = (a - from).perp_dot(dir) / den;
                    if (-1e-9..=1.0 + 1e-9).contains(&s) {
                        take(t, d.try_normalize().map(DVec2::perp));
                    }
                }
                Curve::Arc { .. } | Curve::Circle { .. } => {
                    let (center, radius) = match e.curve {
                        Curve::Arc { center, radius, .. } | Curve::Circle { center, radius } => {
                            (center, radius)
                        }
                        _ => unreachable!(),
                    };
                    // |from + t dir − center| = radius
                    let w = from - center;
                    let (b, c) = (w.dot(dir), w.length_squared() - radius * radius);
                    let disc = b * b - c;
                    if disc < 0.0 {
                        continue;
                    }
                    for t in [-b - disc.sqrt(), -b + disc.sqrt()] {
                        let p = from + dir * t;
                        if e.curve.distance(p) < 1e-7 {
                            take(t, None);
                        }
                    }
                }
                Curve::Spline(_) => {}
            }
        }
    }
    best
}

/// The cut at one end of member `m` (its start if `at_start`), against the other members
/// of the feature and the members of earlier ones.
fn end_cut(
    m: &Member,
    at_start: bool,
    own: &Layout,
    corner: MemberCorner,
    earlier: &[&Layout],
) -> Cut {
    let (p, out) = if at_start {
        (m.start, -m.dir())
    } else {
        (m.end, m.dir())
    };
    let square = Cut::Square(if at_start { 0.0 } else { m.length() });
    // Mitred to the one other line of the feature that ends here
    if corner == MemberCorner::Miter {
        let others: Vec<DVec3> = own
            .members
            .iter()
            .filter(|o| o.number != m.number)
            // Each the way out of the other member at this end
            .filter_map(|o| {
                if o.start.distance(p) < 1e-6 {
                    Some(-o.dir())
                } else if o.end.distance(p) < 1e-6 {
                    Some(o.dir())
                } else {
                    None
                }
            })
            .collect();
        if let [w] = others[..] {
            let n = out - w;
            if out.dot(w) > -1.0 + 1e-9 && n.length() > 1e-9 {
                return Cut::Plane {
                    point: p,
                    normal: n.normalize(),
                };
            }
            return square;
        }
    }
    // Trimmed to the outside of an earlier member it ends on: the one reaching furthest
    let mut best: Option<(f64, Cut)> = None;
    for l in earlier {
        for s in &l.members {
            let d = s.dir();
            let along = (p - s.start).dot(d);
            let off = (p - s.start) - d * along;
            if off.length() > 1e-6 || along < -l.reach - 1e-6 || along > s.length() + l.reach + 1e-6
            {
                continue;
            }
            // Back along the member, as seen in the other's section
            let back = -out - d * (-out).dot(d);
            let Some(back) = back.try_normalize() else {
                continue;
            };
            let to2 = |v: DVec3| DVec2::new(v.dot(s.frame.x_axis()), v.dot(s.frame.y_axis()));
            let Some((t, across)) = exit(&l.regions, to2(p - s.start), to2(back)) else {
                continue;
            };
            let point = p + back * t;
            let normal = match across {
                Some(n2) => {
                    let n = s.frame.x_axis() * n2.x + s.frame.y_axis() * n2.y;
                    // Pointing out of the member's way: towards where it was cut off
                    if n.dot(out) < 0.0 { -n } else { n }
                }
                None => out,
            };
            let reach = (point - p).dot(-out);
            if best.as_ref().is_none_or(|(b, _)| reach > *b) {
                best = Some((reach, Cut::Plane { point, normal }));
            }
        }
    }
    match best {
        Some((_, Cut::Plane { point, normal })) if normal.cross(out).length() < 1e-9 => {
            Cut::Square((point - m.start).dot(m.dir()))
        }
        Some((_, cut)) => cut,
        None => square,
    }
}

/// The members of `own`, made: a body each, after `bodies`.
pub(crate) fn apply_structural_member(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    own: &Layout,
    corner: MemberCorner,
    earlier: &[&Layout],
    seed: u64,
) -> Result<Applied, FeatureError> {
    let mut out = bodies.to_vec();
    for m in &own.members {
        let ends = [true, false].map(|s| end_cut(m, s, own, corner, earlier));
        // Long enough to be cut where an end is slanted
        let spare = 4.0 * own.reach + 1.0;
        let along = |c: &Cut, start: bool| match *c {
            Cut::Square(t) => t,
            Cut::Plane { .. } if start => -spare,
            Cut::Plane { .. } => m.length() + spare,
        };
        let (from, to) = (along(&ends[0], true), along(&ends[1], false));
        if to - from < 1e-6 {
            return Err(FeatureError(format!(
                "A member near ({:.3}, {:.3}, {:.3}) is trimmed to nothing.",
                m.start.x, m.start.y, m.start.z
            )));
        }
        let plane = Plane { frame: m.frame };
        let (mut solid, faces) = extrude_traced(&plane, &own.regions, from, to)?;
        let instance = FaceName::new(feature, FaceRole::Instance(m.number));
        let name = |role: FaceRole| FaceName::merged([&FaceName::new(feature, role), &instance]);
        let mut names: Vec<FaceName> = faces
            .iter()
            .map(|f| match *f {
                ExtrudeFace::Start { .. } => name(FaceRole::NearCap),
                ExtrudeFace::End { .. } => name(FaceRole::FarCap),
                ExtrudeFace::Side {
                    region,
                    loop_index,
                    edge,
                } => {
                    let r = &own.regions[region];
                    let l = if loop_index == 0 {
                        &r.outer
                    } else {
                        &r.holes[loop_index - 1]
                    };
                    name(FaceRole::Side(l.edges[edge].entity))
                }
            })
            .collect();
        for (k, c) in ends.iter().enumerate() {
            let Cut::Plane { point, normal } = *c else {
                continue;
            };
            let size = 2.0 * (m.length() + spare + own.reach) + 10.0;
            let local = peet_kernel::primitive::cuboid(
                DVec3::new(-size, -size, -size),
                DVec3::new(size, size, 0.0),
            );
            let frame = Frame::from_origin_z_x(point, normal, normal.any_orthonormal_vector())
                .ok_or_else(|| FeatureError("A member's end is degenerate.".to_owned()))?;
            let keep = peet_kernel::transform::solid(&local, &frame);
            let cap = name(if k == 0 {
                FaceRole::NearCap
            } else {
                FaceRole::FarCap
            });
            let keep_names = vec![cap; keep.faces.len()];
            let traced = boolean_traced(&solid, &keep, BooleanOp::Intersect)?;
            names = crate::extrude::result_names(&traced.sources, &names, &keep_names);
            solid = traced.solid;
        }
        let index = out.len();
        out.push(Arc::new(Body {
            solid,
            face_names: names,
            origin: feature,
            stamp: stamp(seed, index),
            sheet: None,
        }));
    }
    Ok((out, None))
}
