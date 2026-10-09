//! Sweeps along a general path: a profile carried along any smooth curve.
//!
//! **Path.** Pieces of curve joined end to end, each leaving in the direction the last
//! one arrived: lines, arcs, ellipses and freeform curves, in a plane or not.
//!
//! **Frames.** The profile is carried without twisting about the path: its frame along
//! the path is the *rotation-minimising* one, which turns only as much as the path's
//! direction does. On a path in a plane that is the turn of the tangent in that plane,
//! as a revolve round each bend would give; on a path in space it is what a pipe bent
//! without torsion does.
//!
//! **Surfaces.** The profile is placed at stations along the path and the stations are
//! joined with a loft ([`crate::loft`]), smooth through every station. Stations are
//! added until the lofted surface is within [`TRUE_TO`] of the true sweep between them,
//! so the sides are freeform faces that follow the sweep closely, not exactly; the two
//! ends are the profile itself, exactly. Holes of the profile are swept on their own,
//! through the same stations, and become the inside of the solid.
//!
//! A sweep along lines and tangent arcs alone is better made piece by piece with
//! [`crate::extrude`] and [`crate::revolve`], which give exact cylinders and tori.

use peet_math::{DMat3, DQuat, DVec3, Frame, Plane};
use peet_sketch::region::{Loop, Region};

use crate::geom::Curve3;
use crate::loft::{LoftFace, LoftSection, loft_unchecked};
use crate::nurbs::NurbsCurve;
use crate::{KernelError, Solid};

/// How far a swept surface may be from the true sweep between its stations (mm).
pub const TRUE_TO: f64 = 1e-3;

/// The most stations a sweep is given before it is refused as too intricate.
const MAX_STATIONS: usize = 400;

/// Steps of the frame's integration between two stations.
const FRAME_STEPS: usize = 12;

/// Directions within this of each other (as 1 − cosine) continue smoothly.
const SMOOTH: f64 = 1e-7;

/// One piece of a sweep's path: `curve` between two of its parameters, run from `from`
/// to `to` (which may be the smaller one).
#[derive(Clone, Debug, PartialEq)]
pub struct PathPiece {
    pub curve: Curve3,
    pub from: f64,
    pub to: f64,
}

impl PathPiece {
    /// The straight piece from `a` to `b`. `None` if they are the same point.
    pub fn line(a: DVec3, b: DVec3) -> Option<Self> {
        let curve = Curve3::line_through(a, b)?;
        Some(Self {
            from: curve.param(a),
            to: curve.param(b),
            curve,
        })
    }

    /// The whole of a freeform curve.
    pub fn nurbs(curve: NurbsCurve) -> Self {
        let (from, to) = curve.domain();
        Self {
            curve: Curve3::Nurbs(std::sync::Arc::new(curve)),
            from,
            to,
        }
    }

    fn param(&self, s: f64) -> f64 {
        self.from + (self.to - self.from) * s
    }

    /// The point a fraction `s` of the way along the piece.
    pub fn point(&self, s: f64) -> DVec3 {
        self.curve.point(self.param(s))
    }

    /// The direction of travel there.
    pub fn direction(&self, s: f64) -> DVec3 {
        self.curve.tangent(self.param(s)) * (self.to - self.from).signum()
    }

    /// The curvature vector there: towards the centre of the bend, one over its radius
    /// long.
    fn curvature(&self, s: f64) -> DVec3 {
        let t = self.param(s);
        let d1 = self.curve.derivative(t);
        let d2 = self.curve.second_derivative(t);
        let speed = d1.length_squared();
        if speed <= 1e-300 {
            return DVec3::ZERO;
        }
        (d2 - d1 * (d2.dot(d1) / speed)) / speed
    }
}

/// What a face of a sweep is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SweepFace {
    /// The profile at the start of the path.
    Start,
    /// The profile at the end of the path.
    End,
    /// The side swept by one edge of the profile: loop 0 is the outer loop, loop `h + 1`
    /// hole `h`, and `edge` indexes that loop's edges.
    Side { loop_index: usize, edge: usize },
}

/// A place on the path: a fraction of the way along one of its pieces.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Station {
    piece: usize,
    s: f64,
}

/// Sweeps `region` (in `plane`'s coordinates) along `path`.
pub fn sweep(plane: &Plane, region: &Region, path: &[PathPiece]) -> Result<Solid, KernelError> {
    sweep_traced(plane, region, path).map(|(solid, _)| solid)
}

/// [`sweep`], also reporting what each face of the result is (by face index).
///
/// The profile keeps its place relative to the start of the path; its plane should cross
/// the path there, squarely or not, but not run along it.
pub fn sweep_traced(
    plane: &Plane,
    region: &Region,
    path: &[PathPiece],
) -> Result<(Solid, Vec<SweepFace>), KernelError> {
    let invalid = |m: &str| KernelError::InvalidInput(m.to_owned());
    if path.is_empty() {
        return Err(invalid("the sweep has no path"));
    }
    for (i, piece) in path.iter().enumerate() {
        if (piece.to - piece.from).abs() <= 1e-12 {
            return Err(invalid("a piece of the path has no length"));
        }
        if let Some(next) = path.get(i + 1) {
            if piece.point(1.0).distance(next.point(0.0)) > 1e-6 {
                return Err(invalid("the pieces of the path aren't joined end to end"));
            }
            if piece.direction(1.0).dot(next.direction(0.0)) < 1.0 - SMOOTH {
                return Err(invalid(
                    "the path has a corner: a sweep along a freeform path needs every piece \
                     to carry on in the direction the last one ended",
                ));
            }
        }
    }
    let start = path[0].point(0.0);
    let end = path[path.len() - 1].point(1.0);
    if start.distance(end) <= 1e-6 {
        return Err(KernelError::Unsupported(
            "the path closes on itself; a sweep along a freeform path needs a path with two \
             ends (leave a gap, or sweep round a circle instead)"
                .to_owned(),
        ));
    }
    if plane.normal().dot(path[0].direction(0.0)).abs() < 1e-3 {
        return Err(invalid(
            "the path starts along the profile's plane; it has to leave the plane",
        ));
    }

    // The profile's outline, to judge how well the stations follow the sweep and whether
    // the profile fits round the path's bends.
    let outline: Vec<DVec3> = region
        .outer
        .edges
        .iter()
        .flat_map(|e| (0..8).map(|k| e.curve.point_at(f64::from(k) / 8.0)))
        .map(|p| plane.from_plane_coords(p) - start)
        .collect();
    if outline.is_empty() {
        return Err(invalid("the profile has no outline"));
    }

    // Stations: the ends of every piece, bends divided, then more where needed.
    let mut stations: Vec<Station> = vec![Station { piece: 0, s: 0.0 }];
    for (i, piece) in path.iter().enumerate() {
        let parts = if matches!(piece.curve, Curve3::Line(_)) {
            1
        } else {
            4
        };
        for k in 1..=parts {
            stations.push(Station {
                piece: i,
                s: f64::from(k) / f64::from(parts),
            });
        }
    }
    let turns = loop {
        let turns = frames(path, &stations);
        check_fit(path, &stations, &outline)?;
        if stations.len() == 2 {
            break turns;
        }
        let split = coarse(path, &stations, &turns, &outline)?;
        if split.is_empty() {
            break turns;
        }
        if stations.len() + split.len() > MAX_STATIONS {
            return Err(KernelError::Unsupported(
                "the path is too intricate to sweep along: it has more twists and turns \
                 than the sweep can follow. Simplify the path, or sweep it in parts"
                    .to_owned(),
            ));
        }
        // Later intervals first, so the indices stay good.
        for &i in split.iter().rev() {
            let (a, b) = (stations[i], stations[i + 1]);
            let from = if a.piece == b.piece { a.s } else { 0.0 };
            stations.insert(
                i + 1,
                Station {
                    piece: b.piece,
                    s: 0.5 * (from + b.s),
                },
            );
        }
    };

    // The profile's plane at each station.
    let planes: Vec<Plane> = stations
        .iter()
        .zip(&turns)
        .map(|(st, turn)| Plane {
            frame: Frame {
                origin: path[st.piece].point(st.s) + *turn * (plane.origin() - start),
                rotation: (*turn * plane.frame.rotation).normalize(),
            },
        })
        .collect();

    // The outer loop, then each hole taken out of it.
    let lofted = |profile: &Loop, loop_index: usize| {
        let region = Region {
            outer: profile.clone(),
            holes: Vec::new(),
        };
        let sections: Vec<LoftSection<'_>> = planes
            .iter()
            .map(|plane| LoftSection {
                plane,
                region: &region,
            })
            .collect();
        let (solid, faces) = loft_unchecked(&sections)?;
        let faces: Vec<SweepFace> = faces
            .into_iter()
            .map(|f| match f {
                LoftFace::Start => SweepFace::Start,
                LoftFace::End => SweepFace::End,
                LoftFace::Side { edge } => SweepFace::Side { loop_index, edge },
            })
            .collect();
        Ok::<_, KernelError>((solid, faces))
    };
    let (mut solid, mut faces) = lofted(&region.outer, 0)?;
    for (h, hole) in region.holes.iter().enumerate() {
        let (tool, tool_faces) = lofted(hole, h + 1)?;
        bore(&mut solid, &mut faces, &tool, &tool_faces);
    }
    if let Err(problems) = crate::validate::validate(&solid) {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        return Err(KernelError::InvalidResult(format!(
            "the sweep crosses itself: the profile is too big for a bend of the path, the              path comes back through what it has swept, or a hole of the profile doesn't              fit inside it ({})",
            list.join("; ")
        )));
    }
    Ok((solid, faces))
}

/// Takes the swept hole `tool` out of `solid`. Both were swept through the same
/// stations, so the hole's ends lie in the solid's end faces and its sides inside the
/// solid: the sides become inward-facing faces of the solid, and the ends' outlines
/// holes in the solid's ends. Nothing is intersected.
fn bore(solid: &mut Solid, faces: &mut Vec<SweepFace>, tool: &Solid, tool_faces: &[SweepFace]) {
    let shell = solid.faces[0].shell;
    let vertices: Vec<_> = tool
        .vertices
        .iter()
        .map(|v| solid.add_vertex(v.point))
        .collect();
    let edges: Vec<_> = tool
        .edges
        .iter()
        .map(|e| {
            solid.add_edge(
                e.curve.clone(),
                vertices[e.start.index()],
                vertices[e.end.index()],
                e.t0,
                e.t1,
            )
        })
        .collect();
    for id in tool.face_ids() {
        let face = tool.face(id);
        let what = tool_faces[id.index()];
        // Seen from the solid, each of the hole's faces is walked the other way round.
        let target = match what {
            SweepFace::Side { .. } => {
                faces.push(what);
                solid.add_face(shell, face.surface.clone(), !face.reversed)
            }
            end => {
                let at = faces
                    .iter()
                    .position(|f| *f == end)
                    .expect("a sweep has both its ends");
                crate::topo::FaceId(at as u32)
            }
        };
        for &l in &face.loops {
            let uses: Vec<_> = tool
                .loop_coedges(l)
                .into_iter()
                .rev()
                .map(|c| {
                    let co = tool.coedge(c);
                    (edges[co.edge.index()], !co.reversed)
                })
                .collect();
            solid.add_loop(target, &uses);
        }
    }
}

/// The turn of the profile at each station, from its attitude at the start of the path:
/// the rotation-minimising frame, by double reflection in small steps.
fn frames(path: &[PathPiece], stations: &[Station]) -> Vec<DQuat> {
    let t0 = path[0].direction(0.0);
    let r0 = t0.any_orthonormal_vector();
    let basis = |t: DVec3, r: DVec3| DMat3::from_cols(r, t.cross(r), t);
    let home = basis(t0, r0).transpose();
    let (mut x, mut t, mut r) = (path[0].point(0.0), t0, r0);
    let mut out = Vec::with_capacity(stations.len());
    out.push(DQuat::IDENTITY);
    for w in stations.windows(2) {
        let (a, b) = (w[0], w[1]);
        let from = if a.piece == b.piece { a.s } else { 0.0 };
        let piece = &path[b.piece];
        for k in 1..=FRAME_STEPS {
            let s = from + (b.s - from) * (k as f64 / FRAME_STEPS as f64);
            let (x1, t1) = (piece.point(s), piece.direction(s));
            let v1 = x1 - x;
            let c1 = v1.length_squared();
            if c1 > 1e-300 {
                let rl = r - v1 * (2.0 / c1 * v1.dot(r));
                let tl = t - v1 * (2.0 / c1 * v1.dot(t));
                let v2 = t1 - tl;
                let c2 = v2.length_squared();
                r = if c2 > 1e-300 {
                    rl - v2 * (2.0 / c2 * v2.dot(rl))
                } else {
                    rl
                };
            }
            // Keep it square to the direction and a unit long.
            r = (r - t1 * r.dot(t1)).normalize_or(t1.any_orthonormal_vector());
            (x, t) = (x1, t1);
        }
        out.push(DQuat::from_mat3(&(basis(t, r) * home)).normalize());
    }
    out
}

/// Refuses a profile that doesn't fit round a bend of the path: where the path bends
/// more tightly than the profile reaches towards the inside of the bend, the sweep would
/// fold over itself.
fn check_fit(
    path: &[PathPiece],
    stations: &[Station],
    outline: &[DVec3],
) -> Result<(), KernelError> {
    // Looked at between the stations too.
    let mut fine: Vec<Station> = vec![stations[0]];
    for w in stations.windows(2) {
        let from = if w[0].piece == w[1].piece {
            w[0].s
        } else {
            0.0
        };
        fine.extend((1..=4).map(|k| Station {
            piece: w[1].piece,
            s: from + (w[1].s - from) * f64::from(k) / 4.0,
        }));
    }
    let turns = frames(path, &fine);
    for (st, turn) in fine.iter().zip(&turns) {
        let bend = path[st.piece].curvature(st.s);
        if bend.length_squared() <= 1e-24 {
            continue;
        }
        let reach = outline
            .iter()
            .map(|q| (*turn * *q).dot(bend))
            .fold(0.0, f64::max);
        if reach >= 0.999 {
            return Err(KernelError::InvalidInput(format!(
                "the profile doesn't fit round a bend of the path: the path bends with a \
                 radius of {:.3} mm there, and the profile reaches further than that \
                 towards the inside of the bend. Open the bend up, or make the profile \
                 smaller",
                1.0 / bend.length()
            )));
        }
    }
    Ok(())
}

/// The intervals between stations (by the index of their first station) where a smooth
/// curve through the stations strays from the true sweep by more than [`TRUE_TO`].
fn coarse(
    path: &[PathPiece],
    stations: &[Station],
    turns: &[DQuat],
    outline: &[DVec3],
) -> Result<Vec<usize>, KernelError> {
    // The outline's points that go furthest from the path: they move the most.
    let mut tracked: Vec<DVec3> = Vec::new();
    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
        for sign in [1.0, -1.0] {
            if let Some(p) = outline
                .iter()
                .max_by(|a, b| (a.dot(axis) * sign).total_cmp(&(b.dot(axis) * sign)))
                && !tracked.contains(p)
            {
                tracked.push(*p);
            }
        }
    }
    // Where each interval's middle really is.
    let middles: Vec<Station> = stations
        .windows(2)
        .map(|w| {
            let from = if w[0].piece == w[1].piece {
                w[0].s
            } else {
                0.0
            };
            Station {
                piece: w[1].piece,
                s: 0.5 * (from + w[1].s),
            }
        })
        .collect();
    let mut with_middles: Vec<Station> = Vec::with_capacity(2 * stations.len());
    for (i, st) in stations.iter().enumerate() {
        with_middles.push(*st);
        if let Some(m) = middles.get(i) {
            with_middles.push(*m);
        }
    }
    let all_turns = frames(path, &with_middles);
    let mut split = vec![false; middles.len()];
    for q in &tracked {
        let through: Vec<DVec3> = stations
            .iter()
            .zip(turns)
            .map(|(st, turn)| path[st.piece].point(st.s) + *turn * *q)
            .collect();
        let curve = NurbsCurve::interpolate(&through).map_err(|e| {
            KernelError::InvalidInput(format!("the sweep can't follow the path: {e}"))
        })?;
        for (i, m) in middles.iter().enumerate() {
            if split[i] {
                continue;
            }
            let truth = path[m.piece].point(m.s) + all_turns[2 * i + 1] * *q;
            let near = curve.point(curve.param(truth));
            if near.distance(truth) > TRUE_TO {
                split[i] = true;
            }
        }
    }
    Ok(split
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.then_some(i))
        .collect())
}

#[cfg(test)]
mod tests;
