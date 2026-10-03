//! The hole wizard: drilled, counterbored and countersunk holes at the points of a
//! sketch, in standard sizes.
//!
//! **Positions.** Every free-standing point of the sketch is a hole (the sketch origin
//! and the points that belong to curves are not). A sketch without such points uses the
//! centres of its circles instead, so a sketch of circles drawn for another purpose works
//! too.
//!
//! **Shape.** A hole is a profile turned about its axis ([`peet_kernel::revolve`]): the
//! bore, with a counterbore or a countersink at its mouth and a flat or a drill-point
//! bottom, cut from the bodies it reaches. Holes drill into the face the sketch is on
//! (against the sketch plane's normal).
//!
//! **Threads** are cosmetic: a tapped hole is drilled to its tap drill size and carries
//! the thread's designation ("M6x1"), which drawings and exports can show; no helix is
//! modelled.
//!
//! **Sizes.** [`METRIC`] lists ISO metric coarse threads with their clearance holes
//! (ISO 273), tap drills, counterbores for socket head cap screws (after DIN 974-1) and
//! countersinks for 90° countersunk screws (after ISO 10642). They are starting values:
//! check them against the fasteners you buy.

use std::sync::Arc;

use peet_kernel::revolve::{RevolveAxis, RevolveFace, revolve_traced};
use peet_math::{DVec2, DVec3, Plane, tolerance};
use peet_sketch::region::{Loop, LoopEdge, Region};
use peet_sketch::sketch::Geometry;
use peet_sketch::{Curve, EntityId, Sketch};
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::dressup::sentence;
use crate::extrude::{Combine, Operation, combine};
use crate::feature::{FeatureId, Scalar};
use crate::naming::{Body, FaceName, FaceRole};
use crate::sheet::Applied;

/// What the mouth of a hole looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoleKind {
    /// A plain bore.
    Simple,
    /// A wider, flat-bottomed recess for a screw head.
    Counterbore,
    /// A conical recess for a countersunk screw head.
    Countersink,
}

impl HoleKind {
    pub const ALL: [Self; 3] = [Self::Simple, Self::Counterbore, Self::Countersink];

    pub fn label(self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Counterbore => "Counterbore",
            Self::Countersink => "Countersink",
        }
    }
}

/// How deep a hole goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoleEnd {
    /// To a given depth, measured to the end of the full diameter.
    Blind,
    /// Through every body in its way.
    ThroughAll,
}

/// How a standard size fits its screw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoleFit {
    /// A clearance hole, close fit (ISO 273 fine).
    Close,
    /// A clearance hole, normal fit (ISO 273 medium).
    Normal,
    /// A clearance hole, loose fit (ISO 273 coarse).
    Loose,
    /// Drilled for tapping: the hole carries the thread, cosmetically.
    Tapped,
}

impl HoleFit {
    pub const ALL: [Self; 4] = [Self::Close, Self::Normal, Self::Loose, Self::Tapped];

    pub fn label(self) -> &'static str {
        match self {
            Self::Close => "Clearance, close",
            Self::Normal => "Clearance, normal",
            Self::Loose => "Clearance, loose",
            Self::Tapped => "Tapped",
        }
    }
}

/// A metric screw size and the holes that go with it (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricSize {
    pub name: &'static str,
    /// Nominal thread diameter.
    pub diameter: f64,
    /// Coarse pitch.
    pub pitch: f64,
    pub tap_drill: f64,
    /// Clearance hole diameters: close, normal, loose.
    pub clearance: [f64; 3],
    /// Counterbore for a socket head cap screw.
    pub counterbore_diameter: f64,
    pub counterbore_depth: f64,
    /// Mouth diameter of a 90° countersink for a countersunk screw.
    pub countersink_diameter: f64,
}

impl MetricSize {
    /// The thread's designation, as on a drawing: "M6x1".
    pub fn thread(&self) -> String {
        format!(
            "{}x{}",
            self.name,
            crate::feature::format_number(self.pitch)
        )
    }
}

const fn metric(
    name: &'static str,
    diameter: f64,
    pitch: f64,
    tap_drill: f64,
    clearance: [f64; 3],
    counterbore: (f64, f64),
    countersink_diameter: f64,
) -> MetricSize {
    MetricSize {
        name,
        diameter,
        pitch,
        tap_drill,
        clearance,
        counterbore_diameter: counterbore.0,
        counterbore_depth: counterbore.1,
        countersink_diameter,
    }
}

/// ISO metric coarse sizes, M2 to M24.
pub const METRIC: [MetricSize; 13] = [
    metric("M2", 2.0, 0.4, 1.6, [2.2, 2.4, 2.6], (4.3, 2.3), 4.4),
    metric("M2.5", 2.5, 0.45, 2.05, [2.7, 2.9, 3.1], (5.0, 2.9), 5.5),
    metric("M3", 3.0, 0.5, 2.5, [3.2, 3.4, 3.6], (6.5, 3.4), 6.72),
    metric("M4", 4.0, 0.7, 3.3, [4.3, 4.5, 4.8], (8.0, 4.4), 8.96),
    metric("M5", 5.0, 0.8, 4.2, [5.3, 5.5, 5.8], (10.0, 5.4), 11.2),
    metric("M6", 6.0, 1.0, 5.0, [6.4, 6.6, 7.0], (11.0, 6.4), 13.44),
    metric("M8", 8.0, 1.25, 6.8, [8.4, 9.0, 10.0], (15.0, 8.6), 17.92),
    metric(
        "M10",
        10.0,
        1.5,
        8.5,
        [10.5, 11.0, 12.0],
        (18.0, 10.6),
        22.4,
    ),
    metric(
        "M12",
        12.0,
        1.75,
        10.2,
        [13.0, 13.5, 14.5],
        (20.0, 12.6),
        26.88,
    ),
    metric(
        "M14",
        14.0,
        2.0,
        12.0,
        [15.0, 15.5, 16.5],
        (24.0, 14.6),
        30.8,
    ),
    metric(
        "M16",
        16.0,
        2.0,
        14.0,
        [17.0, 17.5, 18.5],
        (26.0, 16.6),
        33.6,
    ),
    metric(
        "M20",
        20.0,
        2.5,
        17.5,
        [21.0, 22.0, 24.0],
        (33.0, 20.6),
        40.32,
    ),
    metric(
        "M24",
        24.0,
        3.0,
        21.0,
        [25.0, 26.0, 28.0],
        (40.0, 24.8),
        48.0,
    ),
];

/// Holes at the points of a sketch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleFeature {
    /// The sketch whose points are the holes' centres.
    pub sketch: FeatureId,
    pub kind: HoleKind,
    pub diameter: Scalar,
    pub end: HoleEnd,
    /// Blind holes: the depth of the full diameter, from the face.
    pub depth: Scalar,
    /// Blind holes: the angle of the drill's point, in degrees (118 for a twist drill,
    /// 180 for a flat bottom).
    pub tip_angle: Scalar,
    pub counterbore_diameter: Scalar,
    pub counterbore_depth: Scalar,
    /// The countersink's diameter at the face.
    pub countersink_diameter: Scalar,
    /// The countersink's full angle, in degrees (90 for metric countersunk screws).
    pub countersink_angle: Scalar,
    /// Drill the other way (along the sketch plane's normal).
    pub reverse: bool,
    /// A cosmetic thread: its designation ("M6x1").
    pub thread: Option<String>,
    /// The standard size the hole was last set from: its name ("M6") and fit.
    pub standard: Option<(String, HoleFit)>,
}

impl HoleFeature {
    /// An M6 normal clearance hole through everything.
    pub fn new(sketch: FeatureId) -> Self {
        let mut hole = Self {
            sketch,
            kind: HoleKind::Simple,
            diameter: Scalar::new(6.6),
            end: HoleEnd::ThroughAll,
            depth: Scalar::new(10.0),
            tip_angle: Scalar::new(118.0),
            counterbore_diameter: Scalar::new(11.0),
            counterbore_depth: Scalar::new(6.4),
            countersink_diameter: Scalar::new(13.44),
            countersink_angle: Scalar::new(90.0),
            reverse: false,
            thread: None,
            standard: None,
        };
        hole.set_standard(&METRIC[5], HoleFit::Normal);
        hole
    }

    /// Sets every size from a standard screw size.
    pub fn set_standard(&mut self, size: &MetricSize, fit: HoleFit) {
        let diameter = match fit {
            HoleFit::Close => size.clearance[0],
            HoleFit::Normal => size.clearance[1],
            HoleFit::Loose => size.clearance[2],
            HoleFit::Tapped => size.tap_drill,
        };
        self.diameter = Scalar::new(diameter);
        self.counterbore_diameter = Scalar::new(size.counterbore_diameter);
        self.counterbore_depth = Scalar::new(size.counterbore_depth);
        self.countersink_diameter = Scalar::new(size.countersink_diameter);
        self.countersink_angle = Scalar::new(90.0);
        self.thread = (fit == HoleFit::Tapped).then(|| size.thread());
        self.standard = Some((size.name.to_owned(), fit));
    }

    /// The hole in a few words: "M6x1 tapped hole", "Ø6.6 counterbored hole".
    pub fn summary(&self) -> String {
        let size = match &self.thread {
            Some(t) => format!("{t} tapped"),
            None => format!("Ø{}", crate::feature::format_number(self.diameter.value)),
        };
        let kind = match self.kind {
            HoleKind::Simple => "hole",
            HoleKind::Counterbore => "counterbored hole",
            HoleKind::Countersink => "countersunk hole",
        };
        format!("{size} {kind}")
    }
}

/// A hole feature's values, evaluated (mm and degrees).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct HoleSizes {
    pub diameter: f64,
    pub depth: f64,
    pub tip_angle: f64,
    pub counterbore_diameter: f64,
    pub counterbore_depth: f64,
    pub countersink_diameter: f64,
    pub countersink_angle: f64,
}

/// Where the holes of `sketch` go, in sketch coordinates, in a stable order.
pub fn hole_positions(sketch: &Sketch) -> Vec<DVec2> {
    let points: Vec<DVec2> = sketch
        .entities()
        .filter(|(_, e)| {
            matches!(e.geometry, Geometry::Point { .. })
                && e.owner.is_none()
                && !e.locked
                && !e.construction
        })
        .map(|(id, _)| sketch.point(id))
        .collect();
    if !points.is_empty() {
        return points;
    }
    sketch
        .entities()
        .filter(|(_, e)| !e.construction)
        .filter_map(|(_, e)| match e.geometry {
            Geometry::Circle { center, .. } => Some(sketch.point(center)),
            _ => None,
        })
        .collect()
}

/// The parts of a hole's profile, as the ids of the lines that sweep them.
mod part {
    /// The mouth, on the face (it vanishes there).
    pub const MOUTH: u32 = 0;
    /// The counterbore's wall, or the countersink's cone.
    pub const RECESS: u32 = 1;
    /// The counterbore's floor.
    pub const FLOOR: u32 = 2;
    pub const BORE: u32 = 3;
    /// The bottom: flat, or the cone of the drill's point.
    pub const BOTTOM: u32 = 4;
    pub const AXIS: u32 = 5;
}

/// The profile of a hole in the half-plane `(ρ, z)`, `z` being the depth below the face:
/// its corners in order from the centre of the mouth, each with the id of the line that
/// leaves it.
fn profile(
    def: &HoleFeature,
    s: &HoleSizes,
    depth: f64,
    flat: bool,
) -> Result<Vec<(DVec2, u32)>, FeatureError> {
    let err = |m: &str| Err(FeatureError(m.to_owned()));
    let r = s.diameter / 2.0;
    if !(r.is_finite() && r > 10.0 * tolerance::LINEAR) {
        return err("The hole's diameter must be greater than zero.");
    }
    if !(depth.is_finite() && depth > 10.0 * tolerance::LINEAR) {
        return err("The hole's depth must be greater than zero.");
    }
    let mut points = vec![(DVec2::ZERO, part::MOUTH)];
    match def.kind {
        HoleKind::Simple => points.push((DVec2::new(r, 0.0), part::BORE)),
        HoleKind::Counterbore => {
            let (cr, cd) = (s.counterbore_diameter / 2.0, s.counterbore_depth);
            if cr <= r + 10.0 * tolerance::LINEAR {
                return err(
                    "The counterbore must be wider than the hole. Enter a larger counterbore diameter.",
                );
            }
            if cd <= 10.0 * tolerance::LINEAR {
                return err("The counterbore's depth must be greater than zero.");
            }
            if cd >= depth - 10.0 * tolerance::LINEAR {
                return err(
                    "The counterbore is as deep as the hole. Make the hole deeper or the counterbore shallower.",
                );
            }
            points.push((DVec2::new(cr, 0.0), part::RECESS));
            points.push((DVec2::new(cr, cd), part::FLOOR));
            points.push((DVec2::new(r, cd), part::BORE));
        }
        HoleKind::Countersink => {
            let cr = s.countersink_diameter / 2.0;
            let angle = s.countersink_angle;
            if cr <= r + 10.0 * tolerance::LINEAR {
                return err(
                    "The countersink must be wider than the hole. Enter a larger countersink diameter.",
                );
            }
            if !(angle > 1.0 && angle < 179.0) {
                return err("The countersink angle must be between 1° and 179°.");
            }
            let cd = (cr - r) / (angle.to_radians() / 2.0).tan();
            if cd >= depth - 10.0 * tolerance::LINEAR {
                return err(
                    "The countersink is as deep as the hole. Make the hole deeper or the countersink smaller.",
                );
            }
            points.push((DVec2::new(cr, 0.0), part::RECESS));
            points.push((DVec2::new(r, cd), part::BORE));
        }
    }
    points.push((DVec2::new(r, depth), part::BOTTOM));
    let tip = s.tip_angle;
    if flat || tip >= 179.9 {
        points.push((DVec2::new(0.0, depth), part::AXIS));
    } else {
        if tip < 30.0 {
            return err("The drill point angle must be between 30° and 180°.");
        }
        let point = r / (tip.to_radians() / 2.0).tan();
        points.push((DVec2::new(0.0, depth + point), part::AXIS));
    }
    Ok(points)
}

pub(crate) struct HoleInput<'a> {
    pub feature: FeatureId,
    pub bodies: &'a [Arc<Body>],
    pub plane: &'a Plane,
    pub sketch: &'a Sketch,
    pub def: &'a HoleFeature,
    pub sizes: HoleSizes,
    pub stamp: u64,
    /// Added to each hole's number in its faces' names: 0 for the feature itself, a
    /// number of their own for the copies a pattern makes.
    pub instance: u32,
    /// Mirror the sketch (y to −y): for mirrored copies.
    pub mirror: bool,
}

pub(crate) fn apply_hole(input: &HoleInput<'_>) -> Result<Applied, FeatureError> {
    let (def, plane) = (input.def, input.plane);
    let positions = hole_positions(input.sketch);
    if positions.is_empty() {
        return Err(FeatureError(
            "The sketch has no points to drill at. Edit the sketch and place a point (or a \
             circle) at the centre of each hole."
                .to_owned(),
        ));
    }
    // Into the face the sketch is on.
    let dir = if def.reverse {
        plane.normal()
    } else {
        -plane.normal()
    };
    let mut bodies = input.bodies.to_vec();
    let mut made = 0;
    let mut skipped: Vec<String> = Vec::new();
    for (i, &at) in positions.iter().enumerate() {
        let at = if input.mirror {
            DVec2::new(at.x, -at.y)
        } else {
            at
        };
        let centre = plane.from_plane_coords(at);
        let (depth, flat) = match def.end {
            HoleEnd::Blind => (input.sizes.depth, false),
            HoleEnd::ThroughAll => {
                let reach = reach(&bodies, centre, dir);
                if reach <= tolerance::LINEAR {
                    skipped.push(format!("hole {} (there is no body in its way)", i + 1));
                    continue;
                }
                (reach + 1.0, true)
            }
        };
        let corners = profile(def, &input.sizes, depth, flat)?;
        let number = input.instance.wrapping_add(i as u32);
        let (tool, names) = tool(input.feature, plane, centre, dir, &corners, number)?;
        match combine(&Combine {
            feature: input.feature,
            bodies: &bodies,
            tool,
            tool_names: names,
            operation: Operation::Cut,
            stamp: crate::hash::combine(input.stamp, i as u64),
            hint: "Check which way it drills.",
        }) {
            Ok(out) => {
                bodies = out;
                made += 1;
            }
            Err(e) => skipped.push(format!("hole {} ({})", i + 1, e.0)),
        }
    }
    if made == 0 {
        return Err(FeatureError(format!(
            "None of the holes could be made: {}.",
            skipped.join("; ")
        )));
    }
    let warning =
        (!skipped.is_empty()).then(|| format!("Some holes were left out: {}.", skipped.join("; ")));
    Ok((bodies, warning))
}

/// How far the bodies reach from `from` along `dir`.
fn reach(bodies: &[Arc<Body>], from: DVec3, dir: DVec3) -> f64 {
    let mut reach = f64::NEG_INFINITY;
    for body in bodies {
        let bb = body.solid.bounds();
        if bb.is_empty() {
            continue;
        }
        for i in 0..8 {
            let corner = DVec3::new(
                if i & 1 == 0 { bb.min.x } else { bb.max.x },
                if i & 2 == 0 { bb.min.y } else { bb.max.y },
                if i & 4 == 0 { bb.min.z } else { bb.max.z },
            );
            reach = reach.max((corner - from).dot(dir));
        }
    }
    reach
}

/// The solid of one hole, with its faces' names.
fn tool(
    feature: FeatureId,
    plane: &Plane,
    centre: DVec3,
    dir: DVec3,
    corners: &[(DVec2, u32)],
    index: u32,
) -> Result<(peet_kernel::Solid, Vec<FaceName>), FeatureError> {
    // A plane through the hole's axis: x across the face, y down the hole.
    let x = plane.frame.x_axis();
    let half = Plane::from_origin_normal_x(centre, x.cross(dir), x)
        .ok_or_else(|| FeatureError("The hole's direction is degenerate.".to_owned()))?;
    let n = corners.len();
    let edges: Vec<LoopEdge> = (0..n)
        .map(|k| LoopEdge {
            entity: EntityId(corners[k].1),
            curve: Curve::Line {
                a: corners[k].0,
                b: corners[(k + 1) % n].0,
            },
            reversed: false,
        })
        .collect();
    let signed_area = 0.5
        * (0..n)
            .map(|k| corners[k].0.perp_dot(corners[(k + 1) % n].0))
            .sum::<f64>();
    let region = Region {
        outer: Loop { edges, signed_area },
        holes: Vec::new(),
    };
    let axis = RevolveAxis {
        origin: DVec2::ZERO,
        dir: DVec2::Y,
    };
    let (solid, faces) = revolve_traced(
        &half,
        std::slice::from_ref(&region),
        &axis,
        0.0,
        std::f64::consts::TAU,
    )
    .map_err(|e| sentence(&e))?;
    // The first hole's faces are named plainly; the others say which hole they are.
    let instance = FaceName::new(feature, FaceRole::Instance(index));
    let names = faces
        .iter()
        .map(|f| {
            let role = match *f {
                RevolveFace::Side { edge, .. } => FaceRole::Side(region.outer.edges[edge].entity),
                // A full turn has no caps.
                RevolveFace::Start { .. } | RevolveFace::End { .. } => FaceRole::NearCap,
            };
            let name = FaceName::new(feature, role);
            if index == 0 {
                name
            } else {
                FaceName::merged([&name, &instance])
            }
        })
        .collect();
    Ok((solid, names))
}
