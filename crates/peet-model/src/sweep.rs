//! Sweep features: a sketch's regions carried along a path of lines and arcs.
//!
//! **Analytic sweeps.** The path is another sketch: straight lines and arcs joined end to
//! end, smoothly (each piece leaves in the direction the last one arrived). It starts on
//! the profile's plane, square to it. Along a line the profile is extruded; round an arc
//! it is revolved about the arc's axis; the pieces are joined. Every face is then a plane,
//! a cylinder, a cone, a sphere or a torus, exact like the rest of the kernel. A path that
//! is one full circle makes a ring.
//!
//! A corner between two straight pieces is mitred: each piece runs on past the corner
//! and is cut back with the plane that halves the angle, as a picture frame or a welded
//! pipe elbow is made. A corner at an arc is refused (make the pieces tangent), and so
//! is a path that leaves its sketch plane.

use std::sync::Arc;

use peet_kernel::Solid;
use peet_kernel::boolean::{BooleanOp, boolean_traced};
use peet_kernel::extrude::{ExtrudeFace, extrude_traced};
use peet_kernel::revolve::{RevolveAxis, RevolveFace, revolve_traced};
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::region::Region;
use peet_sketch::{Curve, Sketch};
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::{
    Combine, Operation, RegionSelection, combine, result_names, selected_regions,
};
use crate::feature::FeatureId;
use crate::naming::{Body, FaceName, FaceRole};

/// A sweep of a sketch's regions along the path drawn in another sketch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SweepFeature {
    /// The sketch whose regions are swept.
    pub profile: FeatureId,
    /// The sketch whose lines and arcs are the path. `None` until picked.
    pub path: Option<FeatureId>,
    pub operation: Operation,
    pub regions: RegionSelection,
}

impl SweepFeature {
    pub fn new(profile: FeatureId, path: Option<FeatureId>, operation: Operation) -> Self {
        Self {
            profile,
            path,
            operation,
            regions: RegionSelection::Auto,
        }
    }
}

/// Everything a sweep is built from, with references already resolved.
pub struct SweepInput<'a> {
    pub feature: FeatureId,
    pub bodies: &'a [Arc<Body>],
    pub profile_plane: &'a Plane,
    pub profile: &'a Sketch,
    pub path_plane: &'a Plane,
    pub path: &'a Sketch,
    pub def: &'a SweepFeature,
    pub stamp: u64,
}

/// Points this close (mm) are the same point of the path.
const JOIN: f64 = 100.0 * tolerance::LINEAR;

/// Directions within this of each other (as 1 − cosine) continue smoothly.
const SMOOTH: f64 = 1e-9;

/// The cosine of the sharpest corner that is mitred (150° between the directions).
const SHARPEST: f64 = -0.866;

/// One piece of the path, in model space, in the direction it is followed.
#[derive(Clone, Copy, Debug)]
enum Piece {
    Line {
        from: DVec3,
        to: DVec3,
    },
    /// An arc from `from`, turning by `angle` (radians, positive) about the axis through
    /// `center` along `axis` (right-hand rule).
    Arc {
        from: DVec3,
        center: DVec3,
        axis: DVec3,
        angle: f64,
    },
}

impl Piece {
    fn start(&self) -> DVec3 {
        match *self {
            Self::Line { from, .. } | Self::Arc { from, .. } => from,
        }
    }

    fn end(&self) -> DVec3 {
        match *self {
            Self::Line { to, .. } => to,
            Self::Arc {
                from,
                center,
                axis,
                angle,
            } => center + DQuat::from_axis_angle(axis, angle) * (from - center),
        }
    }

    fn start_dir(&self) -> DVec3 {
        match *self {
            Self::Line { from, to } => (to - from).normalize_or_zero(),
            Self::Arc {
                from, center, axis, ..
            } => axis.cross(from - center).normalize_or_zero(),
        }
    }

    fn end_dir(&self) -> DVec3 {
        match *self {
            Self::Line { .. } => self.start_dir(),
            Self::Arc { center, axis, .. } => axis.cross(self.end() - center).normalize_or_zero(),
        }
    }

    fn reversed(&self) -> Self {
        match *self {
            Self::Line { from, to } => Self::Line { from: to, to: from },
            Self::Arc {
                center,
                axis,
                angle,
                ..
            } => Self::Arc {
                from: self.end(),
                center,
                axis: -axis,
                angle,
            },
        }
    }
}

/// The path's pieces in order, starting on `start` (the profile's plane).
fn path_pieces(path: &Sketch, plane: &Plane, start: &Plane) -> Result<Vec<Piece>, FeatureError> {
    let err = |m: &str| Err(FeatureError(m.to_owned()));
    let at = |p| plane.from_plane_coords(p);
    let mut loose: Vec<Piece> = Vec::new();
    for (id, e) in path.entities() {
        if e.construction {
            continue;
        }
        match path.curve(id) {
            Some(Curve::Line { a, b }) if a.distance(b) > JOIN => loose.push(Piece::Line {
                from: at(a),
                to: at(b),
            }),
            Some(Curve::Arc {
                center,
                radius,
                start_angle,
                sweep,
            }) if radius > JOIN && sweep > 1e-9 => loose.push(Piece::Arc {
                from: at(center + peet_math::DVec2::from_angle(start_angle) * radius),
                center: at(center),
                axis: plane.normal(),
                angle: sweep,
            }),
            Some(Curve::Circle { center, radius }) if radius > JOIN => {
                // A whole circle: start it where it crosses the profile's plane.
                let c = at(center);
                let n = plane.normal();
                let across = n.cross(start.normal());
                if across.length() <= 1e-9 || start.signed_distance(c).abs() > JOIN {
                    return err(
                        "A circular path must be square to the profile's plane, with its \
                         centre on that plane.",
                    );
                }
                // The crossing on the side the profile is on: nearest the sketch origin.
                let dir = across.normalize();
                let candidates = [c + dir * radius, c - dir * radius];
                let from = if candidates[0].distance(start.origin())
                    <= candidates[1].distance(start.origin())
                {
                    candidates[0]
                } else {
                    candidates[1]
                };
                loose.push(Piece::Arc {
                    from,
                    center: c,
                    axis: n,
                    angle: std::f64::consts::TAU,
                });
            }
            _ => {}
        }
    }
    if loose.is_empty() {
        return err(
            "The path sketch has no lines or arcs. Draw the path the profile should follow.",
        );
    }
    // The piece that starts on the profile's plane, heading square out of it.
    let on_plane = |p: DVec3| start.signed_distance(p).abs() <= JOIN;
    let square = |d: DVec3| d.cross(start.normal()).length() <= 1e-6;
    let first = loose.iter().enumerate().find_map(|(i, piece)| {
        if on_plane(piece.start()) && square(piece.start_dir()) {
            Some((i, *piece))
        } else if on_plane(piece.end()) && square(piece.end_dir()) {
            Some((i, piece.reversed()))
        } else {
            None
        }
    });
    let Some((i, first)) = first else {
        return err(
            "The path must start on the profile's plane and leave it squarely. Sketch the \
             profile on a plane square to the start of the path (a reference plane helps), \
             or move the path's start onto the profile's plane.",
        );
    };
    loose.swap_remove(i);
    let mut pieces = vec![first];
    loop {
        let last = pieces[pieces.len() - 1];
        let (end, dir) = (last.end(), last.end_dir());
        let next = loose.iter().enumerate().find_map(|(i, piece)| {
            if piece.start().distance(end) <= JOIN {
                Some((i, *piece))
            } else if piece.end().distance(end) <= JOIN {
                Some((i, piece.reversed()))
            } else {
                None
            }
        });
        let Some((i, next)) = next else {
            break;
        };
        if next.start_dir().dot(dir) < 1.0 - SMOOTH {
            // A corner between two straight pieces is mitred; others can't be.
            let straight = matches!(last, Piece::Line { .. }) && matches!(next, Piece::Line { .. });
            if !straight {
                return err(
                    "The path has a corner where an arc meets it at an angle. A sweep can \
                     mitre a corner between two straight pieces; where an arc is involved, \
                     make the pieces tangent.",
                );
            }
            if next.start_dir().dot(dir) < SHARPEST {
                return err(
                    "The path doubles back on itself at a corner too sharp to mitre. Open \
                     the corner up, or round it with an arc.",
                );
            }
        }
        loose.swap_remove(i);
        pieces.push(next);
    }
    if !loose.is_empty() {
        return err(
            "The path sketch has curves that aren't connected to the path. Remove them, \
             join them to it, or make them construction geometry.",
        );
    }
    Ok(pieces)
}

/// The swept tool solid with its face names.
fn tool(input: &SweepInput<'_>) -> Result<(Solid, Vec<FaceName>), FeatureError> {
    let regions: Vec<Region> = selected_regions(input.profile, &input.def.regions, false)?;
    let pieces = path_pieces(input.path, input.path_plane, input.profile_plane)?;
    let side = |region: usize, loop_index: usize, edge: usize| {
        let r = &regions[region];
        let l = if loop_index == 0 {
            &r.outer
        } else {
            &r.holes[loop_index - 1]
        };
        FaceRole::Side(l.edges[edge].entity)
    };
    // How far the profile reaches from the path, for the length of the mitres.
    let on_path = input.profile_plane.to_plane_coords(pieces[0].start());
    let reach = regions
        .iter()
        .map(|r| {
            let (lo, hi) = r.outer.bounds();
            [lo, hi, DVec2::new(lo.x, hi.y), DVec2::new(hi.x, lo.y)]
                .iter()
                .map(|c| c.distance(on_path))
                .fold(0.0, f64::max)
        })
        .fold(0.0, f64::max);
    // The profile's plane, carried along the path.
    let mut plane = *input.profile_plane;
    let mut swept: Option<(Solid, Vec<FaceName>)> = None;
    for (k, piece) in pieces.iter().enumerate() {
        // Faces of the first piece are named plainly; later pieces say which they are.
        let name = |role: FaceRole| {
            let name = FaceName::new(input.feature, role);
            if k == 0 {
                name
            } else {
                FaceName::merged([
                    &name,
                    &FaceName::new(input.feature, FaceRole::Instance(k as u32)),
                ])
            }
        };
        let forward = piece.start_dir().dot(plane.normal()) > 0.0;
        let (solid, names, moved) = match *piece {
            Piece::Line { from, to } => {
                let length = from.distance(to);
                let dir = piece.start_dir();
                // At a corner the piece runs on past it, to be cut back along the mitre.
                let corner_before = (k > 0)
                    .then(|| pieces[k - 1].end_dir())
                    .filter(|d| d.dot(dir) < 1.0 - SMOOTH);
                let corner_after = pieces
                    .get(k + 1)
                    .map(Piece::start_dir)
                    .filter(|d| d.dot(dir) < 1.0 - SMOOTH);
                let past = |other: DVec3| {
                    let half = 0.5 * other.dot(dir).clamp(-1.0, 1.0).acos();
                    reach * half.tan() * 1.5 + 1.0
                };
                let before = corner_before.map_or(0.0, past);
                let after = corner_after.map_or(0.0, past);
                let (lo, hi) = if forward {
                    (-before, length + after)
                } else {
                    (-length - after, before)
                };
                let (mut solid, faces) = extrude_traced(&plane, &regions, lo, hi)?;
                let mut names: Vec<FaceName> = faces
                    .iter()
                    .map(|f| {
                        name(match *f {
                            // The kernel's start cap is the lower one along the normal.
                            ExtrudeFace::Start { .. } if forward => FaceRole::NearCap,
                            ExtrudeFace::Start { .. } => FaceRole::FarCap,
                            ExtrudeFace::End { .. } if forward => FaceRole::FarCap,
                            ExtrudeFace::End { .. } => FaceRole::NearCap,
                            ExtrudeFace::Side {
                                region,
                                loop_index,
                                edge,
                            } => side(region, loop_index, edge),
                        })
                    })
                    .collect();
                // Cut back along each mitre: the plane through the corner that halves
                // the angle between the two pieces.
                let size = 4.0 * (reach + length + before + after + 1.0);
                for (at, normal, keep_behind) in [
                    (from, corner_before.map(|d| d + dir), false),
                    (to, corner_after.map(|d| dir + d), true),
                ] {
                    let Some(normal) = normal else {
                        continue;
                    };
                    let mitre =
                        Plane::from_origin_normal_x(at, normal, normal.any_orthonormal_vector())
                            .ok_or_else(|| {
                                FeatureError("The path has a degenerate corner.".to_owned())
                            })?;
                    let (z0, z1) = if keep_behind {
                        (-size, 0.0)
                    } else {
                        (0.0, size)
                    };
                    let block = peet_kernel::transform::solid(
                        &peet_kernel::primitive::cuboid(
                            DVec3::new(-size, -size, z0),
                            DVec3::new(size, size, z1),
                        ),
                        &mitre.frame,
                    );
                    // The mitre's face vanishes against the next piece's.
                    let cut = name(if keep_behind {
                        FaceRole::FarCap
                    } else {
                        FaceRole::NearCap
                    });
                    let block_names = vec![cut; block.faces.len()];
                    let traced = boolean_traced(&solid, &block, BooleanOp::Intersect)?;
                    names = result_names(&traced.sources, &names, &block_names);
                    solid = traced.solid;
                }
                // The profile's plane at the end of the piece, turned round the corner
                // if there is one.
                let mut moved = Frame {
                    origin: plane.origin() + (to - from),
                    rotation: plane.frame.rotation,
                };
                if let Some(next) = corner_after {
                    let turn = DQuat::from_rotation_arc(dir, next);
                    moved = Frame {
                        origin: to + turn * (moved.origin - to),
                        rotation: (turn * moved.rotation).normalize(),
                    };
                }
                (solid, names, moved)
            }
            Piece::Arc {
                center,
                axis,
                angle,
                ..
            } => {
                let turn = RevolveAxis {
                    origin: plane.to_plane_coords(center),
                    dir: plane.frame.vector_to_local(axis).truncate(),
                };
                let (solid, faces) =
                    revolve_traced(&plane, &regions, &turn, 0.0, angle).map_err(|e| {
                        let FeatureError(m) = FeatureError::from(e);
                        FeatureError(format!(
                            "The profile doesn't fit round a bend of the path (the bend's \
                             radius must be larger than the profile reaches towards its \
                             centre). {m}"
                        ))
                    })?;
                let names: Vec<FaceName> = faces
                    .iter()
                    .map(|f| {
                        name(match *f {
                            RevolveFace::Start { .. } => FaceRole::NearCap,
                            RevolveFace::End { .. } => FaceRole::FarCap,
                            RevolveFace::Side {
                                region,
                                loop_index,
                                edge,
                            } => side(region, loop_index, edge),
                        })
                    })
                    .collect();
                let rotation = DQuat::from_axis_angle(axis, angle);
                let moved = Frame {
                    origin: center + rotation * (plane.origin() - center),
                    rotation: (rotation * plane.frame.rotation).normalize(),
                };
                (solid, names, moved)
            }
        };
        plane = Plane { frame: moved };
        swept = Some(match swept {
            None => (solid, names),
            Some((so_far, so_far_names)) => {
                // The pieces share the profile's section: it vanishes between them.
                let traced = boolean_traced(&so_far, &solid, BooleanOp::Union)?;
                let names = result_names(&traced.sources, &so_far_names, &names);
                (traced.solid, names)
            }
        });
    }
    swept.ok_or_else(|| FeatureError("The path is empty.".to_owned()))
}

/// Applies a sweep feature to the bodies built so far and returns the new list of bodies.
pub fn apply_sweep(input: &SweepInput<'_>) -> Result<Vec<Arc<Body>>, FeatureError> {
    let (tool, tool_names) = tool(input)?;
    combine(&Combine {
        feature: input.feature,
        bodies: input.bodies,
        tool,
        tool_names,
        operation: input.def.operation,
        stamp: input.stamp,
        hint: "Check its profile and path.",
    })
}
