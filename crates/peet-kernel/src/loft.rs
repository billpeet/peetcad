//! Lofts: a solid through a series of profiles on different planes.
//!
//! **Matching.** Each profile is the outline of a sketch region: lines, arcs and splines
//! joined end to end. Every profile must have the same number of pieces, so that piece `k` of
//! one profile is joined to piece `k` of the next. A full circle has no corners of its
//! own and takes them from its neighbours: it is cut into arcs at the directions of the
//! nearest cornered profile's corners (four quarter arcs if every profile is a circle),
//! which is what makes a round-to-square transition come out straight. Profiles are
//! walked the same way round as seen along the loft, and each is started at the corner
//! that lines up best with the profile before it.
//!
//! **Surfaces.** Piece `k` of every profile is skinned into one freeform surface
//! ([`NurbsSurface::skin_at`]): ruled between two profiles, a smooth cubic through more.
//! All the side surfaces place the profiles at the same parameters, so neighbours meet
//! exactly along the rail through their shared corners. A side between two straight
//! pieces that lie in one plane is made a plane, so a loft between similar polygons on
//! parallel planes has flat sides like any other prism or frustum. A spline piece can be
//! joined to lines and to other splines, not yet to an arc (their degrees differ).
//!
//! **Topology** is that of an extrusion: a cap on the first and the last profile, a side
//! face per piece, a rail edge per corner. The caps' edges are the profiles' own lines
//! and circles.

use std::f64::consts::TAU;
use std::sync::Arc;

use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::region::Region;
use peet_sketch::{Curve, SplinePiece};

use crate::extrude::{PreparedLoop, prepare_loop};
use crate::geom::{Circle3, Curve3, Surface};
use crate::nurbs::{NurbsCurve, NurbsSurface};
use crate::profile::spline_curve;
use crate::topo::{EdgeId, VertexId};
use crate::{KernelError, Solid};

/// One profile of a loft: a region in its plane's 2D coordinates.
#[derive(Clone, Copy, Debug)]
pub struct LoftSection<'a> {
    pub plane: &'a Plane,
    pub region: &'a Region,
}

/// What a face of a loft is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoftFace {
    /// The cap on the first profile.
    Start,
    /// The cap on the last profile.
    End,
    /// The side through one piece of each profile: `edge` indexes the first profile's
    /// outer loop edges.
    Side { edge: usize },
}

/// One piece of a profile, in its plane, in the direction the profile is walked.
#[derive(Clone, Debug)]
enum Piece {
    Line {
        a: DVec2,
        b: DVec2,
    },
    /// From angle `from` about `center`, turning by `sweep` (negative: clockwise).
    Arc {
        center: DVec2,
        radius: f64,
        from: f64,
        sweep: f64,
    },
    /// A stretch of a sketch spline, walked against its own direction if `backwards`.
    Spline {
        piece: SplinePiece,
        backwards: bool,
    },
}

impl Piece {
    fn start(&self) -> DVec2 {
        match *self {
            Self::Spline {
                ref piece,
                backwards,
            } => piece.point_at(if backwards { 1.0 } else { 0.0 }),
            Self::Line { a, .. } => a,
            Self::Arc {
                center,
                radius,
                from,
                ..
            } => center + DVec2::from_angle(from) * radius,
        }
    }

    fn reversed(&self) -> Self {
        match *self {
            Self::Spline {
                ref piece,
                backwards,
            } => Self::Spline {
                piece: piece.clone(),
                backwards: !backwards,
            },
            Self::Line { a, b } => Self::Line { a: b, b: a },
            Self::Arc {
                center,
                radius,
                from,
                sweep,
            } => Self::Arc {
                center,
                radius,
                from: from + sweep,
                sweep: -sweep,
            },
        }
    }
}

/// A profile being matched to the others.
enum Outline {
    /// Pieces in order, each with the index of the loop edge it came from.
    Pieces(Vec<(Piece, usize)>),
    /// A full circle, to be cut where its neighbours have corners.
    Circle {
        center: DVec2,
        radius: f64,
        source: usize,
    },
}

/// A solid through `sections`, in order. Each must be a region without holes; all must
/// have the same number of edges (a full circle adapts).
pub fn loft(sections: &[LoftSection<'_>]) -> Result<Solid, KernelError> {
    loft_traced(sections).map(|(solid, _)| solid)
}

/// [`loft`], also reporting what each face of the result is (by face index).
pub fn loft_traced(sections: &[LoftSection<'_>]) -> Result<(Solid, Vec<LoftFace>), KernelError> {
    let (solid, faces) = loft_unchecked(sections)?;
    if let Err(problems) = crate::validate::validate(&solid) {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        return Err(KernelError::InvalidResult(format!(
            "The loft produced an invalid solid (do the profiles twist, or cross each \
             other?): {}.",
            list.join("; ")
        )));
    }
    Ok((solid, faces))
}

/// [`loft_traced`] without the check that the result is a valid solid, for a caller that
/// checks what it makes of it.
pub(crate) fn loft_unchecked(
    sections: &[LoftSection<'_>],
) -> Result<(Solid, Vec<LoftFace>), KernelError> {
    let invalid = |m: String| KernelError::InvalidInput(m);
    if sections.len() < 2 {
        return Err(invalid("a loft needs at least two profiles".to_owned()));
    }
    // The profiles' outlines, counter-clockwise in their own planes.
    let mut outlines = Vec::with_capacity(sections.len());
    for (i, s) in sections.iter().enumerate() {
        if !s.region.holes.is_empty() {
            return Err(KernelError::Unsupported(format!(
                "profile {} has a hole; a loft joins plain outlines",
                i + 1
            )));
        }
        let prepared = prepare_loop(&s.region.outer.edges, true)
            .map_err(|m| invalid(format!("profile {}: its outline {m}", i + 1)))?;
        outlines.push(match prepared {
            PreparedLoop::Circle {
                center,
                radius,
                source,
                ..
            } => Outline::Circle {
                center,
                radius,
                source,
            },
            PreparedLoop::Pieces(pieces) => Outline::Pieces(
                pieces
                    .iter()
                    .map(|p| {
                        let piece = match p.curve {
                            Curve::Line { .. } => {
                                let (a, b) = p.ends();
                                Piece::Line { a, b }
                            }
                            Curve::Arc {
                                center,
                                radius,
                                start_angle,
                                sweep,
                            } => {
                                let arc = Piece::Arc {
                                    center,
                                    radius,
                                    from: start_angle,
                                    sweep,
                                };
                                if p.reversed { arc.reversed() } else { arc }
                            }
                            Curve::Spline(ref s) => Piece::Spline {
                                piece: s.clone(),
                                backwards: p.reversed,
                            },
                            Curve::Circle { .. } => {
                                unreachable!("full circles are handled as circle loops")
                            }
                        };
                        (piece, p.source)
                    })
                    .collect(),
            ),
        });
    }

    // Where each profile is, and which way the loft runs past it.
    let centers: Vec<DVec3> = sections
        .iter()
        .zip(&outlines)
        .map(|(s, o)| {
            let c = match o {
                Outline::Circle { center, .. } => *center,
                Outline::Pieces(pieces) => {
                    pieces.iter().map(|(p, _)| p.start()).sum::<DVec2>() / pieces.len() as f64
                }
            };
            s.plane.from_plane_coords(c)
        })
        .collect();
    let count = sections.len();
    let mut along = Vec::with_capacity(count);
    for i in 0..count {
        let (a, b) = (i.saturating_sub(1), (i + 1).min(count - 1));
        let d = (centers[b] - centers[a]).try_normalize().ok_or_else(|| {
            invalid(format!(
                "profiles {} and {} are in the same place",
                a + 1,
                b + 1
            ))
        })?;
        along.push(d);
    }
    // Seen along the loft, every profile runs counter-clockwise: a profile whose plane
    // faces the other way is walked backwards.
    let mut facing = Vec::with_capacity(count);
    for i in 0..count {
        let dot = sections[i].plane.normal().dot(along[i]);
        if dot.abs() <= 1e-6 {
            return Err(invalid(format!(
                "profile {} is edge-on to the loft: its plane runs along the way to its \
                 neighbours",
                i + 1
            )));
        }
        facing.push(dot > 0.0);
        if !facing[i]
            && let Outline::Pieces(pieces) = &mut outlines[i]
        {
            pieces.reverse();
            for (p, _) in pieces.iter_mut() {
                *p = p.reversed();
            }
        }
    }

    // The number of pieces, and the corners circles are cut at.
    let cornered = outlines
        .iter()
        .position(|o| matches!(o, Outline::Pieces(_)));
    let pieces_in = |o: &Outline| match o {
        Outline::Pieces(p) => Some(p.len()),
        Outline::Circle { .. } => None,
    };
    let n = cornered.and_then(|i| pieces_in(&outlines[i])).unwrap_or(4);
    for (i, o) in outlines.iter().enumerate() {
        if let Some(have) = pieces_in(o)
            && have != n
        {
            return Err(invalid(format!(
                "the profiles need the same number of edges to be joined one to one: profile \
                 {} has {n} and profile {} has {have}. Split an edge of the one with fewer",
                cornered.unwrap_or(0) + 1,
                i + 1
            )));
        }
    }
    // Directions (in model space, from a profile's middle) to cut circles at.
    let directions: Vec<DVec3> = match cornered {
        Some(i) => {
            let Outline::Pieces(pieces) = &outlines[i] else {
                unreachable!("a cornered profile has pieces")
            };
            pieces
                .iter()
                .map(|(p, _)| sections[i].plane.from_plane_coords(p.start()) - centers[i])
                .collect()
        }
        None => {
            // Every profile is a circle: quarter them, going round the way the loft does.
            let frame = &sections[0].plane.frame;
            let turn = if facing[0] { 1.0 } else { -1.0 };
            (0..4)
                .map(|k| {
                    let (s, c) = (TAU * f64::from(k) / 4.0).sin_cos();
                    frame.x_axis() * c + frame.y_axis() * (s * turn)
                })
                .collect()
        }
    };
    let mut profiles: Vec<Vec<(Piece, usize)>> = Vec::with_capacity(count);
    // Each circle is cut where the circle before it was, so that a row of circles on
    // planes that turn a long way (a swept pipe) keeps its corners in line.
    let mut directions = directions;
    for (i, o) in outlines.into_iter().enumerate() {
        profiles.push(match o {
            Outline::Pieces(p) => p,
            Outline::Circle {
                center,
                radius,
                source,
            } => {
                let frame = &sections[i].plane.frame;
                let turn = if facing[i] { 1.0 } else { -1.0 };
                let mut angles: Vec<f64> = directions
                    .iter()
                    .map(|d| {
                        let local = frame.vector_to_local(*d);
                        local.y.atan2(local.x)
                    })
                    .collect();
                // Unwrap them into one turn in the profile's direction.
                for k in 1..angles.len() {
                    let step = ((angles[k] - angles[k - 1]) * turn).rem_euclid(TAU);
                    angles[k] = angles[k - 1] + step * turn;
                }
                let total = (angles[n - 1] - angles[0]).abs();
                if total >= TAU - 1e-9 || angles.windows(2).any(|w| (w[1] - w[0]).abs() < 1e-6) {
                    return Err(invalid(format!(
                        "profile {} (a circle) can't be matched to the corners of the others: \
                         seen from it they don't go round in order",
                        i + 1
                    )));
                }
                directions = angles
                    .iter()
                    .map(|a| {
                        let (s, c) = a.sin_cos();
                        (frame.x_axis() * c + frame.y_axis() * s) * radius
                    })
                    .collect();
                (0..n)
                    .map(|k| {
                        let from = angles[k];
                        let to = if k + 1 < n {
                            angles[k + 1]
                        } else {
                            angles[0] + TAU * turn
                        };
                        (
                            Piece::Arc {
                                center,
                                radius,
                                from,
                                sweep: to - from,
                            },
                            source,
                        )
                    })
                    .collect()
            }
        });
    }

    // Start each profile at the corner that lines up best with the one before.
    let corners = |i: usize, profile: &[(Piece, usize)]| -> Vec<DVec3> {
        profile
            .iter()
            .map(|(p, _)| sections[i].plane.from_plane_coords(p.start()))
            .collect()
    };
    for i in 1..count {
        let before: Vec<DVec3> = corners(i - 1, &profiles[i - 1])
            .iter()
            .map(|p| *p - centers[i - 1])
            .collect();
        let here: Vec<DVec3> = corners(i, &profiles[i])
            .iter()
            .map(|p| *p - centers[i])
            .collect();
        let cost = |shift: usize| -> f64 {
            (0..n)
                .map(|k| here[(k + shift) % n].distance_squared(before[k]))
                .sum()
        };
        let best = (0..n)
            .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
            .unwrap_or(0);
        profiles[i].rotate_left(best);
    }

    // A side must be of one degree all the way: a spline (cubic) and an arc (a rational
    // quadratic) can't be made so yet.
    for k in 0..n {
        let has = |f: fn(&Piece) -> bool| profiles.iter().position(|p| f(&p[k].0));
        let spline = has(|p| matches!(p, Piece::Spline { .. }));
        let arc = has(|p| matches!(p, Piece::Arc { .. }));
        if let (Some(spline), Some(arc)) = (spline, arc) {
            return Err(KernelError::Unsupported(format!(
                "a loft side that joins a spline (profile {}) to an arc or a circle (profile \
                 {}). Draw that arc as a spline through three points, or the spline as lines \
                 and arcs",
                spline + 1,
                arc + 1
            )));
        }
    }

    // The geometry: each corner's rail, each piece's surface.
    let rails: Vec<Vec<DVec3>> = (0..n)
        .map(|k| {
            (0..count)
                .map(|i| {
                    sections[i]
                        .plane
                        .from_plane_coords(profiles[i][k].0.start())
                })
                .collect()
        })
        .collect();
    let nurbs = |m: crate::nurbs::NurbsError| invalid(format!("the loft can't be made: {m}"));
    let params = NurbsSurface::section_params(&rails).map_err(nurbs)?;
    let curve_3d = |i: usize, piece: &Piece| -> Result<NurbsCurve, KernelError> {
        let plane = sections[i].plane;
        Ok(match *piece {
            Piece::Spline {
                ref piece,
                backwards,
            } => {
                let curve = spline_curve(piece, |p| plane.from_plane_coords(p))?;
                if backwards { curve.reversed() } else { curve }
            }
            Piece::Line { a, b } => {
                NurbsCurve::line(plane.from_plane_coords(a), plane.from_plane_coords(b), 1)
            }
            Piece::Arc {
                center,
                radius,
                from,
                sweep,
            } => NurbsCurve::arc(
                &Frame {
                    origin: plane.from_plane_coords(center),
                    rotation: plane.frame.rotation,
                },
                radius,
                from,
                sweep,
            ),
        })
    };

    let mut solid = Solid::new();
    let mut faces = Vec::with_capacity(n + 2);
    let shell = solid.add_shell();
    let last = count - 1;
    let bottom_v: Vec<VertexId> = (0..n).map(|k| solid.add_vertex(rails[k][0])).collect();
    let top_v: Vec<VertexId> = (0..n).map(|k| solid.add_vertex(rails[k][last])).collect();
    // The caps' edges: the first and last profiles' own curves. Each is stored in its
    // curve's own direction; the flag says whether the profile runs against it.
    let cap_edges =
        |solid: &mut Solid, i: usize, v: &[VertexId]| -> Result<Vec<(EdgeId, bool)>, KernelError> {
            let plane = sections[i].plane;
            (0..n)
                .map(|k| {
                    let (from, to) = (v[k], v[(k + 1) % n]);
                    Ok(match profiles[i][k].0 {
                        Piece::Spline { .. } => {
                            let curve = curve_3d(i, &profiles[i][k].0)?;
                            let (lo, hi) = curve.domain();
                            let curve = Curve3::Nurbs(Arc::new(curve));
                            (solid.add_edge(curve, from, to, lo, hi), false)
                        }
                        Piece::Line { .. } => (solid.add_line_edge(from, to), false),
                        Piece::Arc {
                            center,
                            radius,
                            from: a0,
                            sweep,
                        } => {
                            let circle = Curve3::Circle(Circle3 {
                                frame: Frame {
                                    origin: plane.from_plane_coords(center),
                                    rotation: plane.frame.rotation,
                                },
                                radius,
                            });
                            if sweep > 0.0 {
                                (solid.add_edge(circle, from, to, a0, a0 + sweep), false)
                            } else {
                                (solid.add_edge(circle, to, from, a0 + sweep, a0), true)
                            }
                        }
                    })
                })
                .collect()
        };
    let bottom_e = cap_edges(&mut solid, 0, &bottom_v)?;
    let top_e = cap_edges(&mut solid, last, &top_v)?;

    // Sides and rails. A rail is the edge of its two neighbouring surfaces.
    let mut surfaces: Vec<Surface> = Vec::with_capacity(n);
    for k in 0..n {
        let section_curves: Vec<NurbsCurve> = (0..count)
            .map(|i| curve_3d(i, &profiles[i][k].0))
            .collect::<Result<_, _>>()?;

        let flat = flat_side(&profiles, &rails, k, count);
        surfaces.push(match flat {
            Some(plane) => Surface::Plane(plane),
            None => Surface::Nurbs(Arc::new(
                NurbsSurface::skin_at(&section_curves, &params).map_err(nurbs)?,
            )),
        });
    }
    let rail_e: Vec<EdgeId> = (0..n)
        .map(|k| {
            if count == 2 {
                return solid.add_line_edge(bottom_v[k], top_v[k]);
            }
            // The curve through the corners, as the surface that starts there has it.
            let curve = match &surfaces[k] {
                Surface::Nurbs(s) => s.iso_curve(false, 0.0),
                _ => match &surfaces[(k + n - 1) % n] {
                    Surface::Nurbs(s) => s.iso_curve(false, 1.0),
                    _ => unreachable!("flat sides only come with two profiles"),
                },
            };
            let (lo, hi) = curve.domain();
            solid.add_edge(
                Curve3::Nurbs(Arc::new(curve)),
                bottom_v[k],
                top_v[k],
                lo,
                hi,
            )
        })
        .collect();
    for (k, surface) in surfaces.into_iter().enumerate() {
        let next = (k + 1) % n;
        let face = solid.add_face(shell, surface, false);
        faces.push(LoftFace::Side {
            edge: profiles[0][k].1,
        });
        solid.add_loop(
            face,
            &[
                (bottom_e[k].0, bottom_e[k].1),
                (rail_e[next], false),
                (top_e[k].0, !top_e[k].1),
                (rail_e[k], true),
            ],
        );
    }

    // Caps. The profiles run counter-clockwise seen along the loft: the last cap shows
    // them that way from outside, the first cap the other way round.
    let end = solid.add_face(shell, Surface::Plane(*sections[last].plane), !facing[last]);
    faces.push(LoftFace::End);
    solid.add_loop(end, &top_e);
    let start = solid.add_face(shell, Surface::Plane(*sections[0].plane), facing[0]);
    faces.push(LoftFace::Start);
    let backwards: Vec<(EdgeId, bool)> = bottom_e.iter().rev().map(|&(e, r)| (e, !r)).collect();
    solid.add_loop(start, &backwards);
    Ok((solid, faces))
}

/// The plane of side `k`, if it is flat: two profiles, straight pieces, four corners in
/// one plane. Its normal points out of the loft.
fn flat_side(
    profiles: &[Vec<(Piece, usize)>],
    rails: &[Vec<DVec3>],
    k: usize,
    count: usize,
) -> Option<Plane> {
    if count != 2
        || !profiles
            .iter()
            .all(|p| matches!(p[k].0, Piece::Line { .. }))
    {
        return None;
    }
    let n = rails.len();
    let (a, b) = (rails[k][0], rails[(k + 1) % n][0]);
    let (c, d) = (rails[k][1], rails[(k + 1) % n][1]);
    // Out of the loft: along the profile, then up the loft.
    let normal = (b - a).cross(c - a).try_normalize()?;
    let plane = Plane::from_origin_normal_x(a, normal, b - a)?;
    (plane.signed_distance(d).abs() <= tolerance::LINEAR).then_some(plane)
}

#[cfg(test)]
mod tests;
