//! Revolving sketch regions about an axis in the sketch plane.
//!
//! **Surfaces.** Turned about the axis, a line of the profile sweeps a plane (square to the
//! axis), a cylinder (parallel to it) or a cone; an arc sweeps a sphere (centred on the
//! axis) or a torus. A line lying on the axis sweeps nothing.
//!
//! **Coordinates.** The profile is described in the half-plane `(ρ, z)`: `ρ` is the
//! distance from the axis, `z` the height along it. The axis direction is taken so that
//! this chart is right-handed in the sketch plane, which makes a counter-clockwise loop of
//! the sketch counter-clockwise in `(ρ, z)`; the region is then to the left of its loops
//! and the outside of every swept face is to the right. The angle `u` about the axis
//! (the surfaces' own parameter) starts at the sketch plane.
//!
//! **Topology.** A profile vertex off the axis sweeps a circle edge; one on the axis stays
//! a single vertex (a cone's apex, a sphere's pole). Seen from outside, the face swept by a
//! profile piece from `A` to `B` is bounded by `A`'s circle forwards, the piece at the end
//! angle from `A` to `B`, `B`'s circle backwards, and the piece at the start angle back.
//!
//! - *A full turn* closes each face on itself: the piece at the start and at the end is
//!   one seam edge, used twice (see [`crate::topo`]). Flat faces need no seam: they are
//!   discs or rings bounded by their circles alone. A vertex on the axis belongs to one
//!   face only (each face wraps all the way round it), so faces meeting there get a
//!   vertex each. Every loop of the region gives a closed shell of its own.
//! - *A partial turn* adds the two flat caps, the profile at the start and at the end.

use std::f64::consts::{PI, TAU};

use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::Curve;
use peet_sketch::region::Region;

use crate::extrude::{Piece, PreparedLoop, prepare_loop};
use crate::geom::{Circle3, Cone, Curve3, Cylinder, Sphere, Surface, Torus};
use crate::topo::{EdgeId, ShellId, VertexId};
use crate::{KernelError, Solid};

/// Profile points this close to the axis (mm) are on it.
const ON_AXIS: f64 = 10.0 * tolerance::LINEAR;

/// A turn within this of 2π (radians) is a full turn.
const FULL_TURN: f64 = 1e-9;

/// The axis of a revolution, in the sketch plane's 2D coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevolveAxis {
    pub origin: DVec2,
    /// Direction (any length). Angles are counted about it by the right-hand rule.
    pub dir: DVec2,
}

/// What a face of a revolution is, in terms of the input that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevolveFace {
    /// The flat cap at the angle `from`, of the region with this index in the input.
    Start { region: usize },
    /// The flat cap at the angle `to`.
    End { region: usize },
    /// The face swept by one loop edge. `loop_index` 0 is the region's outer loop and
    /// `1..` are its holes; `edge` indexes that loop's `edges`. A circle given as several
    /// arcs is one face, reported with its first arc.
    Side {
        region: usize,
        loop_index: usize,
        edge: usize,
    },
}

/// Revolves `regions` (in `plane`'s 2D coordinates) about `axis` from the angle `from` to
/// the angle `to` (radians, `from < to`, at most a full turn apart; 0 is the sketch plane).
/// The regions must lie on one side of the axis; they may touch it.
pub fn revolve(
    plane: &Plane,
    regions: &[Region],
    axis: &RevolveAxis,
    from: f64,
    to: f64,
) -> Result<Solid, KernelError> {
    revolve_traced(plane, regions, axis, from, to).map(|(solid, _)| solid)
}

/// [`revolve`], also reporting what each face of the result is (by face index).
pub fn revolve_traced(
    plane: &Plane,
    regions: &[Region],
    axis: &RevolveAxis,
    from: f64,
    to: f64,
) -> Result<(Solid, Vec<RevolveFace>), KernelError> {
    let invalid = |m: &str| Err(KernelError::InvalidInput(m.to_owned()));
    if !from.is_finite() || !to.is_finite() {
        return invalid("the revolve angle is not a finite number");
    }
    let span = to - from;
    if span <= 1e-9 {
        return invalid("the revolve angle must be greater than zero");
    }
    if span > TAU + FULL_TURN {
        return invalid("the revolve angle can't be more than a full turn (360°)");
    }
    if regions.is_empty() {
        return invalid("nothing to revolve: select at least one closed region");
    }
    let Some(dir) = axis.dir.try_normalize().filter(|_| axis.origin.is_finite()) else {
        return invalid("the revolve axis has no direction");
    };

    let mut loops: Vec<Vec<PreparedLoop>> = Vec::with_capacity(regions.len());
    for (ri, region) in regions.iter().enumerate() {
        let mut list =
            vec![prepare_loop(&region.outer.edges, true).map_err(|m| {
                KernelError::InvalidInput(format!("region {ri}: outer boundary {m}"))
            })?];
        for (hi, h) in region.holes.iter().enumerate() {
            list.push(
                prepare_loop(&h.edges, false).map_err(|m| {
                    KernelError::InvalidInput(format!("region {ri}: hole {hi} {m}"))
                })?,
            );
        }
        loops.push(list);
    }

    // Which side of the axis the profile is on: the side of its farthest point.
    let left = dir.perp();
    let mut lowest = f64::INFINITY;
    let mut highest = f64::NEG_INFINITY;
    for l in loops.iter().flatten() {
        for (lo, hi) in extent(l, axis.origin, left) {
            lowest = lowest.min(lo);
            highest = highest.max(hi);
        }
    }
    let side = if highest.abs() >= lowest.abs() {
        1.0
    } else {
        -1.0
    };
    if (side > 0.0 && lowest < -ON_AXIS) || (side < 0.0 && highest > ON_AXIS) {
        return invalid(
            "the profile crosses the axis: everything revolved must be on one side of it",
        );
    }
    if highest.abs().max(lowest.abs()) <= ON_AXIS {
        return invalid("the profile lies on the axis");
    }
    // The (ρ, z) chart: ρ towards the profile, z so that the chart is right-handed in
    // the sketch plane. That z is the given direction, or its opposite.
    let rho_dir = left * side;
    let z_dir = rho_dir.perp();
    let flipped = z_dir.dot(dir) < 0.0;
    let (u0, u1) = if flipped { (-to, -from) } else { (from, to) };
    let full = span >= TAU - FULL_TURN;

    let origin = plane.from_plane_coords(axis.origin);
    let to_3d = |v: DVec2| plane.frame.vector_to_world(v.extend(0.0));
    let base = Frame::from_origin_z_x(origin, to_3d(z_dir), to_3d(rho_dir))
        .ok_or_else(|| KernelError::InvalidInput("the revolve axis is degenerate".to_owned()))?;
    let mut b = Builder {
        solid: Solid::new(),
        faces: Vec::new(),
        base,
        chart: Chart {
            origin: axis.origin,
            rho_dir,
            z_dir,
        },
        u0,
        u1: if full { u0 + TAU } else { u1 },
        full,
        flipped,
    };
    for (ri, list) in loops.iter().enumerate() {
        b.add_region(ri, list)?;
    }
    debug_assert_eq!(b.faces.len(), b.solid.faces.len());
    if let Err(problems) = crate::validate::validate(&b.solid) {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        return Err(KernelError::InvalidResult(format!(
            "The revolve produced an invalid solid (does the profile cross itself?): {}.",
            list.join("; ")
        )));
    }
    Ok((b.solid, b.faces))
}

/// The `(ρ, z)` chart of the profile plane.
#[derive(Clone, Copy, Debug)]
struct Chart {
    origin: DVec2,
    rho_dir: DVec2,
    z_dir: DVec2,
}

impl Chart {
    /// `(ρ, z)` of a sketch point, with `ρ` snapped to the axis.
    fn map(&self, p: DVec2) -> DVec2 {
        let w = p - self.origin;
        let rho = w.dot(self.rho_dir);
        DVec2::new(
            if rho.abs() <= ON_AXIS { 0.0 } else { rho },
            w.dot(self.z_dir),
        )
    }

    /// A sketch angle as an angle in the chart (counter-clockwise from the `ρ` axis).
    fn angle(&self, sketch_angle: f64) -> f64 {
        sketch_angle - self.rho_dir.to_angle()
    }
}

/// Signed distances from the axis (positive to its left) of a loop's pieces: the range of
/// each.
fn extent(l: &PreparedLoop, origin: DVec2, left: DVec2) -> Vec<(f64, f64)> {
    let d = |p: DVec2| (p - origin).dot(left);
    match l {
        PreparedLoop::Circle { center, radius, .. } => {
            vec![(d(*center) - radius, d(*center) + radius)]
        }
        PreparedLoop::Pieces(pieces) => pieces
            .iter()
            .map(|p| {
                let (a, b) = (d(p.curve.start()), d(p.curve.end()));
                let (mut lo, mut hi) = (a.min(b), a.max(b));
                if let Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep,
                } = p.curve
                {
                    // The arc's extremes across the axis, where it reaches them.
                    let toward = left.to_angle();
                    if arc_contains(start_angle, sweep, toward) {
                        hi = d(center) + radius;
                    }
                    if arc_contains(start_angle, sweep, toward + PI) {
                        lo = d(center) - radius;
                    }
                }
                (lo, hi)
            })
            .collect(),
    }
}

/// Whether the arc from `start` over `sweep` (counter-clockwise) passes `angle`.
fn arc_contains(start: f64, sweep: f64, angle: f64) -> bool {
    (angle - start).rem_euclid(TAU) < sweep
}

/// One piece of a profile loop, in the chart.
#[derive(Clone, Copy, Debug)]
struct Swept {
    /// What the piece sweeps.
    kind: Kind,
    /// Whether the piece's own direction (a line: from its first point in loop order;
    /// an arc: counter-clockwise) is the loop's direction.
    forward: bool,
    /// Index of the loop edge the piece is reported as.
    source: usize,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// A line on the axis: no face.
    Axis,
    /// A line square to the axis, with the outward normal along +z or −z.
    Flat { up: bool },
    /// A line: a cylinder or a cone.
    Ruled,
    /// An arc with centre `center` (chart coordinates): a sphere or a torus. `v0` is the
    /// chart angle where the arc starts (counter-clockwise).
    Round {
        center: DVec2,
        radius: f64,
        v0: f64,
        sweep: f64,
    },
}

struct Builder {
    solid: Solid,
    /// What each face of `solid` is, in face order.
    faces: Vec<RevolveFace>,
    /// Origin on the axis, Z along it, X towards the profile.
    base: Frame,
    chart: Chart,
    u0: f64,
    u1: f64,
    full: bool,
    /// The chart's z runs against the given axis direction: the angles were negated, so
    /// the cap at `u0` is the one at the angle `to`.
    flipped: bool,
}

/// A profile vertex, swept.
#[derive(Clone, Copy, Debug)]
enum Ring {
    /// On the axis. In a partial turn every face shares the vertex.
    Pole(Option<VertexId>),
    /// Off the axis: the vertices at the start and end angles (the same one for a full
    /// turn) and the circle edge between them.
    Circle {
        start: VertexId,
        end: VertexId,
        edge: EdgeId,
    },
}

impl Builder {
    fn radial(&self, u: f64) -> DVec3 {
        let (s, c) = u.sin_cos();
        self.base.x_axis() * c + self.base.y_axis() * s
    }

    /// The point `q = (ρ, z)` of the profile at the angle `u`.
    fn point(&self, q: DVec2, u: f64) -> DVec3 {
        self.base.origin + self.radial(u) * q.x + self.base.z_axis() * q.y
    }

    /// The surfaces' frame with its origin at height `z` on the axis.
    fn frame_at(&self, z: f64) -> Frame {
        Frame {
            origin: self.base.origin + self.base.z_axis() * z,
            rotation: self.base.rotation,
        }
    }

    fn add_region(&mut self, region: usize, loops: &[PreparedLoop]) -> Result<(), KernelError> {
        let shell = self.solid.add_shell();
        // Per loop: the profile pieces as edges at the start and at the end angle.
        let mut caps: Vec<Vec<(EdgeId, EdgeId, bool)>> = Vec::with_capacity(loops.len());
        for (li, l) in loops.iter().enumerate() {
            // A full turn closes every loop's faces on themselves: a shell each.
            let shell = if self.full && li > 0 {
                self.solid.add_shell()
            } else {
                shell
            };
            caps.push(self.add_loop(shell, l, region, li)?);
        }
        if self.full {
            return Ok(());
        }
        // The caps. Seen from outside, the cap at the start angle shows the chart as it
        // is, so its loops run as given; the cap at the end angle shows it mirrored.
        let axis = self.base.z_axis();
        let cap_plane = |b: &Self, u: f64, sign: f64| {
            Plane::from_origin_normal_x(b.base.origin, axis.cross(b.radial(u)) * sign, axis)
                .map(Surface::Plane)
                .ok_or_else(|| KernelError::InvalidInput("the revolve axis is degenerate".into()))
        };
        let start = cap_plane(self, self.u0, -1.0)?;
        let start = self.solid.add_face(shell, start, false);
        let (at_from, at_to) = (RevolveFace::Start { region }, RevolveFace::End { region });
        self.faces.push(if self.flipped { at_to } else { at_from });
        for l in &caps {
            let uses: Vec<(EdgeId, bool)> = l.iter().map(|&(e, _, r)| (e, r)).collect();
            self.solid.add_loop(start, &uses);
        }
        let end = cap_plane(self, self.u1, 1.0)?;
        let end = self.solid.add_face(shell, end, false);
        self.faces.push(if self.flipped { at_from } else { at_to });
        for l in &caps {
            let uses: Vec<(EdgeId, bool)> = l.iter().rev().map(|&(_, e, r)| (e, !r)).collect();
            self.solid.add_loop(end, &uses);
        }
        Ok(())
    }

    /// Adds the faces swept by one loop. Returns, per piece in loop order, its edge at
    /// the start angle, its edge at the end angle and whether the loop runs against them.
    fn add_loop(
        &mut self,
        shell: ShellId,
        l: &PreparedLoop,
        region: usize,
        loop_index: usize,
    ) -> Result<Vec<(EdgeId, EdgeId, bool)>, KernelError> {
        let unsupported = |m: &str| Err(KernelError::Unsupported(m.to_owned()));
        // The loop as pieces between chart points; a full circle is one closed piece.
        let (points, pieces): (Vec<DVec2>, Vec<Swept>) = match *l {
            PreparedLoop::Circle {
                center,
                radius,
                start_angle,
                reversed,
                source,
            } => {
                let c = self.chart.map(center);
                if c.x - radius <= ON_AXIS {
                    return unsupported(
                        "a circle that touches the axis; draw it clear of the axis, or use a \
                         half circle closed along the axis for a ball",
                    );
                }
                let v0 = self.chart.angle(start_angle);
                (
                    vec![c + DVec2::from_angle(v0) * radius],
                    vec![Swept {
                        kind: Kind::Round {
                            center: c,
                            radius,
                            v0: wrap(v0),
                            sweep: TAU,
                        },
                        forward: !reversed,
                        source,
                    }],
                )
            }
            PreparedLoop::Pieces(ref list) => {
                let points: Vec<DVec2> = list.iter().map(|p| self.chart.map(p.ends().0)).collect();
                let mut pieces = Vec::with_capacity(list.len());
                for (i, p) in list.iter().enumerate() {
                    let (a, b) = (points[i], points[(i + 1) % points.len()]);
                    pieces.push(self.classify(p, a, b)?);
                }
                (points, pieces)
            }
        };
        let n = pieces.len();

        // The profile's vertices, swept.
        let mut rings: Vec<Ring> = Vec::with_capacity(points.len());
        for &q in &points {
            rings.push(if q.x <= 0.0 {
                // A full turn gives each face its own vertex there (made with the face).
                Ring::Pole((!self.full).then(|| self.solid.add_vertex(self.point(q, 0.0))))
            } else {
                let start = self.solid.add_vertex(self.point(q, self.u0));
                let end = if self.full {
                    start
                } else {
                    self.solid.add_vertex(self.point(q, self.u1))
                };
                let circle = Curve3::Circle(Circle3 {
                    frame: self.frame_at(q.y),
                    radius: q.x,
                });
                let edge = self.solid.add_edge(circle, start, end, self.u0, self.u1);
                Ring::Circle { start, end, edge }
            });
        }

        let mut caps = Vec::with_capacity(n);
        for (i, piece) in pieces.iter().enumerate() {
            let j = (i + 1) % points.len();
            let (a, b) = (points[i], points[j]);
            let tag = RevolveFace::Side {
                region,
                loop_index,
                edge: piece.source,
            };
            // A flat face of a full turn is bounded by its circles alone.
            if let (true, Kind::Flat { up }) = (self.full, piece.kind) {
                let normal = self.base.z_axis() * if up { 1.0 } else { -1.0 };
                let surface = Plane::from_origin_normal_x(
                    self.frame_at(a.y).origin,
                    normal,
                    self.base.x_axis(),
                )
                .map(Surface::Plane)
                .ok_or_else(|| {
                    KernelError::InvalidInput("the revolve axis is degenerate".into())
                })?;
                let face = self.solid.add_face(shell, surface, false);
                self.faces.push(tag);
                // Seen from outside, A's circle runs forwards and B's backwards; the
                // one farther from the axis is the outer loop.
                let (first, second) = if a.x > b.x { (i, j) } else { (j, i) };
                for k in [first, second] {
                    if let Ring::Circle { edge, .. } = rings[k] {
                        self.solid.add_loop(face, &[(edge, k == j)]);
                    }
                }
                continue;
            }
            if self.full && matches!(piece.kind, Kind::Axis) {
                continue;
            }
            // The piece's vertices at the start and at the end angle.
            let ends = |me: &mut Self, ring: Ring, q: DVec2| match ring {
                Ring::Circle { start, end, .. } => (start, end),
                Ring::Pole(Some(v)) => (v, v),
                Ring::Pole(None) => {
                    let v = me.solid.add_vertex(me.point(q, 0.0));
                    (v, v)
                }
            };
            let (a0, a1) = ends(self, rings[i], a);
            let (b0, b1) = ends(self, rings[j], b);
            // Its edges, in the piece's own direction.
            let (m0, m1) = {
                let (s0, e0, s1, e1) = if piece.forward {
                    (a0, b0, a1, b1)
                } else {
                    (b0, a0, b1, a1)
                };
                let (from, to) = if piece.forward { (a, b) } else { (b, a) };
                let m0 = self.meridian(piece, from, to, self.u0, s0, e0);
                let m1 = if self.full || matches!(piece.kind, Kind::Axis) {
                    m0
                } else {
                    self.meridian(piece, from, to, self.u1, s1, e1)
                };
                (m0, m1)
            };
            caps.push((m0, m1, !piece.forward));
            let Some((surface, reversed)) = self.surface(piece, a, b)? else {
                continue;
            };
            let face = self.solid.add_face(shell, surface, reversed);
            self.faces.push(tag);
            let mut uses = Vec::with_capacity(4);
            if let Ring::Circle { edge, .. } = rings[i] {
                uses.push((edge, false));
            }
            uses.push((m1, !piece.forward));
            if let Ring::Circle { edge, .. } = rings[j] {
                uses.push((edge, true));
            }
            uses.push((m0, piece.forward));
            self.solid.add_loop(face, &uses);
        }
        Ok(caps)
    }

    /// What a loop piece from `a` to `b` (chart points, in loop order) sweeps.
    fn classify(&self, p: &Piece, a: DVec2, b: DVec2) -> Result<Swept, KernelError> {
        let unsupported = |m: &str| Err(KernelError::Unsupported(m.to_owned()));
        let kind = match p.curve {
            Curve::Line { .. } => {
                if a.x <= 0.0 && b.x <= 0.0 {
                    Kind::Axis
                } else if (b.y - a.y).abs() <= tolerance::LINEAR {
                    // The outside is to the right of the loop's direction.
                    Kind::Flat { up: b.x < a.x }
                } else {
                    Kind::Ruled
                }
            }
            Curve::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => {
                let c = self.chart.map(center);
                let v0 = wrap(self.chart.angle(start_angle));
                if c.x == 0.0 {
                    // A sphere: the arc is on the profile's side of the axis, so its
                    // angles are latitudes.
                    if v0 < -PI / 2.0 - 1e-6 || v0 + sweep > PI / 2.0 + 1e-6 {
                        return Err(KernelError::InvalidInput(
                            "the profile crosses the axis: everything revolved must be on \
                             one side of it"
                                .to_owned(),
                        ));
                    }
                } else if c.x < 0.0 {
                    return unsupported(
                        "an arc whose centre is on the other side of the axis from the profile",
                    );
                } else if a.x <= 0.0
                    || b.x <= 0.0
                    || (arc_contains(v0, sweep, PI) && c.x - radius <= ON_AXIS)
                {
                    return unsupported("an arc that touches the axis without being centred on it");
                }
                Kind::Round {
                    center: c,
                    radius,
                    v0,
                    sweep,
                }
            }
            Curve::Circle { .. } => unreachable!("full circles are handled as circle loops"),
        };
        Ok(Swept {
            kind,
            // Lines are stored in loop direction, arcs counter-clockwise.
            forward: matches!(p.curve, Curve::Line { .. }) || !p.reversed,
            source: p.source,
        })
    }

    /// The edge of a piece at the angle `u`, from the vertex `start` to `end` (the piece's
    /// own direction, `from` and `to` being its ends in the chart).
    fn meridian(
        &mut self,
        piece: &Swept,
        from: DVec2,
        to: DVec2,
        u: f64,
        start: VertexId,
        end: VertexId,
    ) -> EdgeId {
        match piece.kind {
            Kind::Round {
                center,
                radius,
                v0,
                sweep,
            } => {
                let radial = self.radial(u);
                let axis = self.base.z_axis();
                let frame =
                    Frame::from_origin_z_x(self.point(center, u), radial.cross(axis), radial)
                        .expect("the radial direction is square to the axis");
                self.solid.add_edge(
                    Curve3::Circle(Circle3 { frame, radius }),
                    start,
                    end,
                    v0,
                    v0 + sweep,
                )
            }
            _ => {
                let (p, q) = (self.point(from, u), self.point(to, u));
                let curve = Curve3::line_through(p, q).expect("profile lines have a length");
                self.solid.add_edge(curve, start, end, 0.0, p.distance(q))
            }
        }
    }

    /// The surface a piece from `a` to `b` sweeps, and whether its outside is against the
    /// surface's natural normal. `None` for a piece on the axis.
    fn surface(
        &self,
        piece: &Swept,
        a: DVec2,
        b: DVec2,
    ) -> Result<Option<(Surface, bool)>, KernelError> {
        Ok(Some(match piece.kind {
            Kind::Axis => return Ok(None),
            Kind::Flat { up } => {
                // A partial turn: a sector of the plane.
                let normal = self.base.z_axis() * if up { 1.0 } else { -1.0 };
                let plane = Plane::from_origin_normal_x(
                    self.frame_at(a.y).origin,
                    normal,
                    self.base.x_axis(),
                )
                .ok_or_else(|| {
                    KernelError::InvalidInput("the revolve axis is degenerate".to_owned())
                })?;
                (Surface::Plane(plane), false)
            }
            Kind::Ruled => {
                // The outside is to the right of the loop's direction: away from the
                // axis when the loop climbs.
                let reversed = b.y < a.y;
                if (b.x - a.x).abs() <= tolerance::LINEAR {
                    (
                        Surface::Cylinder(Cylinder {
                            frame: self.base,
                            radius: a.x,
                        }),
                        reversed,
                    )
                } else {
                    (
                        Surface::Cone(Cone {
                            frame: self.frame_at(a.y),
                            radius: a.x,
                            half_angle: ((b.x - a.x) / (b.y - a.y)).atan(),
                        }),
                        reversed,
                    )
                }
            }
            Kind::Round { center, radius, .. } => {
                let frame = self.frame_at(center.y);
                let surface = if center.x == 0.0 {
                    Surface::Sphere(Sphere { frame, radius })
                } else {
                    Surface::Torus(Torus {
                        frame,
                        major: center.x,
                        minor: radius,
                    })
                };
                // A counter-clockwise arc has its outside away from its centre.
                (surface, !piece.forward)
            }
        }))
    }
}

/// An angle brought into `(−π, π]`.
fn wrap(a: f64) -> f64 {
    let w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI { w + TAU } else { w }
}

#[cfg(test)]
mod tests;
