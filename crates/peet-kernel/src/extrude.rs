//! Extrusion of sketch regions into prisms.
//!
//! **Construction.** Every region loop is turned into a ring of side faces. Loop vertex `i`
//! (the start of loop edge `i`, in loop order) becomes a bottom vertex `B_i` (at `from`) and
//! a top vertex `T_i` (at `to`) joined by a vertical line edge shared by the two side faces
//! that meet there. Each loop edge gives a bottom and a top cap edge (stored in the 2D
//! curve's own direction, so arcs run counter-clockwise about the plane normal) and a side
//! face whose loop is, seen from outside: bottom edge along the loop direction, vertical edge
//! up, top edge back, vertical edge down.
//!
//! A loop made of a single circle (or of co-circular arcs covering the whole circle, which is
//! what `find_regions` produces for a circle) becomes one closed circle edge per cap and one
//! cylinder face with a seam line.
//!
//! **Orientation.** The region lies to the left of every loop edge (outer loops run
//! counter-clockwise, holes clockwise), so the outward side of a side face is to the right of
//! the loop direction. The top cap uses the plane's normal and walks the loops as given; the
//! bottom cap is the same plane offset to `from` with `reversed = true` and walks the loops
//! backwards. A cylinder side face is `reversed` exactly when its loop edge is traversed
//! clockwise (the material is outside the circle, as for a round hole).

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::Curve;
use peet_sketch::region::{LoopEdge, Region};

use crate::geom::{Circle3, Curve3, Cylinder, Surface};
use crate::topo::{EdgeId, ShellId, VertexId};
use crate::{KernelError, Solid};

/// Loop edges shorter than this are dropped (their end merges into the next edge's start).
const MIN_EDGE_LENGTH: f64 = tolerance::LINEAR;

/// Arcs whose centres and radii agree within this are treated as pieces of one circle.
const SAME_CIRCLE: f64 = 10.0 * tolerance::LINEAR;

/// Extrudes `regions` (in `plane`'s 2D coordinates, as found by
/// `peet_sketch::region::find_regions`) along the plane normal, from offset `from` to offset
/// `to` (mm along the normal, `from < to`). Each region becomes one closed shell: planar caps
/// at both ends, a planar side face per line and a cylindrical side face per arc or circle.
/// Holes in regions become inner loops on the caps and inward-facing side walls. Disjoint
/// regions give a solid with several shells (lumps).
///
/// Topology follows [`crate::topo`]'s conventions: shared edges between neighbouring side
/// faces, full circles as closed edges plus a seam line on the cylinder, outward normals.
/// Loops given with the wrong winding (an outer loop clockwise, a hole counter-clockwise)
/// are reversed rather than rejected.
pub fn extrude(
    plane: &Plane,
    regions: &[Region],
    from: f64,
    to: f64,
) -> Result<Solid, KernelError> {
    extrude_traced(plane, regions, from, to).map(|(solid, _)| solid)
}

/// What a face of an extrusion is, in terms of the input that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtrudeFace {
    /// The cap at `from`, of the region with this index in the input.
    Start { region: usize },
    /// The cap at `to`.
    End { region: usize },
    /// The wall swept by one loop edge. `loop_index` 0 is the region's outer loop and
    /// `1..` are its holes; `edge` indexes that loop's `edges`. A circle given as several
    /// arcs is one wall, reported with its first arc.
    Side {
        region: usize,
        loop_index: usize,
        edge: usize,
    },
}

/// [`extrude`], also reporting what each face of the result is (by face index). Persistent
/// naming builds on this.
pub fn extrude_traced(
    plane: &Plane,
    regions: &[Region],
    from: f64,
    to: f64,
) -> Result<(Solid, Vec<ExtrudeFace>), KernelError> {
    if !from.is_finite() || !to.is_finite() {
        return Err(KernelError::InvalidInput(
            "extrude depth is not a finite number".to_owned(),
        ));
    }
    if to - from <= tolerance::LINEAR {
        return Err(KernelError::InvalidInput(format!(
            "extrude depth must be positive (from {from} mm to {to} mm)"
        )));
    }
    if regions.is_empty() {
        return Err(KernelError::InvalidInput(
            "nothing to extrude: select at least one closed region".to_owned(),
        ));
    }

    let mut builder = Builder {
        solid: Solid::new(),
        plane: *plane,
        from,
        to,
        faces: Vec::new(),
    };
    for (ri, region) in regions.iter().enumerate() {
        let outer = prepare_loop(&region.outer.edges, true)
            .map_err(|m| KernelError::InvalidInput(format!("region {ri}: outer boundary {m}")))?;
        let holes = region
            .holes
            .iter()
            .enumerate()
            .map(|(hi, h)| {
                prepare_loop(&h.edges, false)
                    .map_err(|m| KernelError::InvalidInput(format!("region {ri}: hole {hi} {m}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        builder.add_region(ri, &outer, &holes);
    }
    debug_assert_eq!(builder.faces.len(), builder.solid.faces.len());
    Ok((builder.solid, builder.faces))
}

/// A loop edge after cleaning: geometry plus traversal direction.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub curve: Curve,
    pub reversed: bool,
    /// Index of the edge in the loop as given.
    pub source: usize,
}

impl Piece {
    /// Start and end in loop order.
    pub fn ends(&self) -> (DVec2, DVec2) {
        let (a, b) = (self.curve.start(), self.curve.end());
        if self.reversed { (b, a) } else { (a, b) }
    }
}

/// A cleaned loop.
pub(crate) enum PreparedLoop {
    /// A general loop of lines and arcs, at least two pieces.
    Pieces(Vec<Piece>),
    /// A full circle; `reversed` means clockwise. `start_angle` is where the seam goes.
    Circle {
        center: DVec2,
        radius: f64,
        start_angle: f64,
        reversed: bool,
        /// Index of the loop edge the circle is reported as.
        source: usize,
    },
}

/// Drops degenerate pieces, detects full circles and fixes the winding (counter-clockwise
/// for outer loops, clockwise for holes). Errors describe the problem for the user.
pub(crate) fn prepare_loop(edges: &[LoopEdge], outer: bool) -> Result<PreparedLoop, String> {
    let mut pieces: Vec<Piece> = edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.curve.length() > MIN_EDGE_LENGTH)
        .map(|(source, e)| Piece {
            curve: e.curve,
            reversed: e.reversed,
            source,
        })
        .collect();
    if pieces.is_empty() {
        return Err("has no edges".to_owned());
    }
    for p in &pieces {
        if !curve_is_finite(&p.curve) {
            return Err("has an edge with non-finite coordinates".to_owned());
        }
    }
    if let Some(circle) = full_circle(&pieces, outer) {
        return Ok(circle);
    }
    if pieces.iter().any(|p| p.curve.is_closed()) {
        return Err("mixes a full circle with other edges".to_owned());
    }
    if pieces.len() < 2 {
        return Err("is not closed".to_owned());
    }
    for i in 0..pieces.len() {
        let (_, end) = pieces[i].ends();
        let (start, _) = pieces[(i + 1) % pieces.len()].ends();
        if end.distance(start) > 100.0 * tolerance::LINEAR {
            return Err(format!(
                "is not closed: a gap of {:.3e} mm between edges {i} and {}",
                end.distance(start),
                (i + 1) % pieces.len()
            ));
        }
    }
    let area: f64 = pieces.iter().map(signed_area_contribution).sum();
    let size = pieces.iter().map(|p| p.curve.length()).sum::<f64>();
    if area.abs() <= tolerance::LINEAR * size {
        return Err("encloses no area".to_owned());
    }
    if (area > 0.0) != outer {
        pieces.reverse();
        for p in &mut pieces {
            p.reversed = !p.reversed;
        }
    }
    Ok(PreparedLoop::Pieces(pieces))
}

fn curve_is_finite(c: &Curve) -> bool {
    match *c {
        Curve::Line { a, b } => a.is_finite() && b.is_finite(),
        Curve::Circle { center, radius } => center.is_finite() && radius.is_finite(),
        Curve::Arc {
            center,
            radius,
            start_angle,
            sweep,
        } => {
            center.is_finite() && radius.is_finite() && start_angle.is_finite() && sweep.is_finite()
        }
    }
}

/// `½ ∮ (x dy − y dx)` of a piece in loop direction (exact for arcs).
fn signed_area_contribution(p: &Piece) -> f64 {
    let c = &p.curve;
    let a = match *c {
        Curve::Circle { radius, .. } => std::f64::consts::PI * radius * radius,
        Curve::Arc { radius, sweep, .. } => {
            0.5 * c.start().perp_dot(c.end()) + 0.5 * radius * radius * (sweep - sweep.sin())
        }
        Curve::Line { a, b } => 0.5 * a.perp_dot(b),
    };
    if p.reversed { -a } else { a }
}

/// A full circle: a single circle piece, or arcs of one circle in one direction covering it.
fn full_circle(pieces: &[Piece], outer: bool) -> Option<PreparedLoop> {
    let (center, radius) = match pieces[0].curve {
        Curve::Circle { center, radius } | Curve::Arc { center, radius, .. } => (center, radius),
        Curve::Line { .. } => return None,
    };
    let mut total = 0.0;
    for p in pieces {
        match p.curve {
            Curve::Circle {
                center: c,
                radius: r,
            } if pieces.len() == 1 => {
                debug_assert!(c == center && r == radius);
                total = TAU;
            }
            Curve::Arc {
                center: c,
                radius: r,
                sweep,
                ..
            } if c.distance(center) <= SAME_CIRCLE
                && (r - radius).abs() <= SAME_CIRCLE
                && p.reversed == pieces[0].reversed =>
            {
                total += sweep;
            }
            _ => return None,
        }
    }
    if (total - TAU).abs() * radius > 100.0 * tolerance::LINEAR {
        return None;
    }
    let start = pieces[0].ends().0;
    // The winding is fixed by the loop's role, whatever the input says.
    Some(PreparedLoop::Circle {
        center,
        radius,
        start_angle: (start - center).to_angle(),
        reversed: !outer,
        source: pieces[0].source,
    })
}

struct Builder {
    solid: Solid,
    plane: Plane,
    from: f64,
    to: f64,
    /// What each face of `solid` is, in face order.
    faces: Vec<ExtrudeFace>,
}

/// Bottom and top edge of one loop piece, plus its side surface.
struct SideEdges {
    bottom: EdgeId,
    top: EdgeId,
    surface: Surface,
    face_reversed: bool,
    /// Cap edges are stored in the 2D curve direction; this is the loop's direction on them.
    reversed: bool,
}

/// Everything a cap needs from one loop: its edges in loop order.
struct BuiltLoop {
    /// `(bottom edge, top edge, traversed reversed)` per loop piece, in loop order.
    caps: Vec<(EdgeId, EdgeId, bool)>,
}

impl Builder {
    fn normal(&self) -> DVec3 {
        self.plane.normal()
    }

    fn point(&self, uv: DVec2, h: f64) -> DVec3 {
        self.plane.from_plane_coords(uv) + self.normal() * h
    }

    /// A frame with the sketch plane's axes, at 2D point `uv` lifted by `h`.
    fn frame_at(&self, uv: DVec2, h: f64) -> Frame {
        Frame {
            origin: self.point(uv, h),
            rotation: self.plane.frame.rotation,
        }
    }

    fn add_region(&mut self, region: usize, outer: &PreparedLoop, holes: &[PreparedLoop]) {
        let shell = self.solid.add_shell();
        let mut built = vec![self.add_loop_sides(shell, outer, region, 0)];
        for (hi, h) in holes.iter().enumerate() {
            built.push(self.add_loop_sides(shell, h, region, hi + 1));
        }
        // Caps: top walks the loops as given, bottom walks them backwards.
        let top_plane = Plane {
            frame: self.frame_at(DVec2::ZERO, self.to),
        };
        let bottom_plane = Plane {
            frame: self.frame_at(DVec2::ZERO, self.from),
        };
        let top = self.solid.add_face(shell, Surface::Plane(top_plane), false);
        let bottom = self
            .solid
            .add_face(shell, Surface::Plane(bottom_plane), true);
        self.faces.push(ExtrudeFace::End { region });
        self.faces.push(ExtrudeFace::Start { region });
        for l in &built {
            let uses: Vec<(EdgeId, bool)> = l.caps.iter().map(|&(_, t, r)| (t, r)).collect();
            self.solid.add_loop(top, &uses);
            let uses: Vec<(EdgeId, bool)> = l.caps.iter().rev().map(|&(b, _, r)| (b, !r)).collect();
            self.solid.add_loop(bottom, &uses);
        }
    }

    /// Adds the side faces of one loop and returns its cap edges.
    fn add_loop_sides(
        &mut self,
        shell: ShellId,
        l: &PreparedLoop,
        region: usize,
        loop_index: usize,
    ) -> BuiltLoop {
        let side = |edge: usize| ExtrudeFace::Side {
            region,
            loop_index,
            edge,
        };
        match *l {
            PreparedLoop::Circle {
                center,
                radius,
                start_angle,
                reversed,
                source,
            } => {
                let seam_uv = center + DVec2::from_angle(start_angle) * radius;
                let vb = self.solid.add_vertex(self.point(seam_uv, self.from));
                let vt = self.solid.add_vertex(self.point(seam_uv, self.to));
                let seam = self.solid.add_line_edge(vb, vt);
                let circle = |h: f64| {
                    Curve3::Circle(Circle3 {
                        frame: self.frame_at(center, h),
                        radius,
                    })
                };
                let (cb, ct) = (circle(self.from), circle(self.to));
                let bottom = self
                    .solid
                    .add_edge(cb, vb, vb, start_angle, start_angle + TAU);
                let top = self
                    .solid
                    .add_edge(ct, vt, vt, start_angle, start_angle + TAU);
                let surface = Surface::Cylinder(Cylinder {
                    frame: self.frame_at(center, 0.0),
                    radius,
                });
                let face = self.solid.add_face(shell, surface, reversed);
                self.faces.push(side(source));
                self.solid.add_loop(
                    face,
                    &[
                        (bottom, reversed),
                        (seam, false),
                        (top, !reversed),
                        (seam, true),
                    ],
                );
                BuiltLoop {
                    caps: vec![(bottom, top, reversed)],
                }
            }
            PreparedLoop::Pieces(ref pieces) => {
                let n = pieces.len();
                // Loop vertex i is the start of piece i (in loop order).
                let bottom_v: Vec<VertexId> = pieces
                    .iter()
                    .map(|p| self.solid.add_vertex(self.point(p.ends().0, self.from)))
                    .collect();
                let top_v: Vec<VertexId> = pieces
                    .iter()
                    .map(|p| self.solid.add_vertex(self.point(p.ends().0, self.to)))
                    .collect();
                let vertical: Vec<EdgeId> = (0..n)
                    .map(|i| self.solid.add_line_edge(bottom_v[i], top_v[i]))
                    .collect();
                let mut caps = Vec::with_capacity(n);
                for (i, p) in pieces.iter().enumerate() {
                    let j = (i + 1) % n;
                    let wall =
                        self.add_piece_edges(p, [bottom_v[i], bottom_v[j]], [top_v[i], top_v[j]]);
                    let face = self.solid.add_face(shell, wall.surface, wall.face_reversed);
                    self.faces.push(side(p.source));
                    self.solid.add_loop(
                        face,
                        &[
                            (wall.bottom, wall.reversed),
                            (vertical[j], false),
                            (wall.top, !wall.reversed),
                            (vertical[i], true),
                        ],
                    );
                    caps.push((wall.bottom, wall.top, wall.reversed));
                }
                BuiltLoop { caps }
            }
        }
    }

    /// Cap edges and side surface of a line or arc piece running (in loop order) from
    /// `bottom[0]` to `bottom[1]` (and `top[0]` to `top[1]`).
    fn add_piece_edges(
        &mut self,
        p: &Piece,
        bottom: [VertexId; 2],
        top: [VertexId; 2],
    ) -> SideEdges {
        // Vertices in the curve's own direction.
        let (b0, b1, t0, t1) = if p.reversed {
            (bottom[1], bottom[0], top[1], top[0])
        } else {
            (bottom[0], bottom[1], top[0], top[1])
        };
        match p.curve {
            Curve::Line { .. } => {
                let be = self.solid.add_line_edge(b0, b1);
                let te = self.solid.add_line_edge(t0, t1);
                // Outward normal: right of the loop direction, i.e. direction × normal.
                let (a, b) = p.ends();
                let a3 = self.point(a, self.from);
                let dir = self.point(b, self.from) - a3;
                let n = self.normal();
                let surface = Plane::from_origin_normal_x(a3, dir.cross(n), dir)
                    .map(Surface::Plane)
                    .expect("line pieces are longer than the merge tolerance");
                SideEdges {
                    bottom: be,
                    top: te,
                    surface,
                    face_reversed: false,
                    reversed: p.reversed,
                }
            }
            Curve::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => {
                let (from, to) = (self.from, self.to);
                let mut arc_edge = |h: f64, s: VertexId, e: VertexId| {
                    let curve = Curve3::Circle(Circle3 {
                        frame: self.frame_at(center, h),
                        radius,
                    });
                    self.solid
                        .add_edge(curve, s, e, start_angle, start_angle + sweep)
                };
                let be = arc_edge(from, b0, b1);
                let te = arc_edge(to, t0, t1);
                SideEdges {
                    bottom: be,
                    top: te,
                    surface: Surface::Cylinder(Cylinder {
                        frame: self.frame_at(center, 0.0),
                        radius,
                    }),
                    face_reversed: p.reversed,
                    reversed: p.reversed,
                }
            }
            Curve::Circle { .. } => unreachable!("full circles are handled as circle loops"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::validate::{Counts, measure, validate};
    use peet_math::DQuat;
    use peet_sketch::Sketch;
    use peet_sketch::region::find_regions;
    use peet_sketch::shapes;

    fn regions_of(s: &Sketch) -> Vec<Region> {
        find_regions(s).regions
    }

    fn check(solid: &Solid) -> Counts {
        match validate(solid) {
            Ok(c) => c,
            Err(problems) => panic!("invalid solid: {problems:#?}"),
        }
    }

    fn counts(v: usize, e: usize, f: usize, rings: usize, shells: usize, genus: i64) -> Counts {
        Counts {
            vertices: v,
            edges: e,
            faces: f,
            rings,
            shells,
            genus,
        }
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn rectangle_block() {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(40.0, 20.0));
        let solid = extrude(&Plane::TOP, &regions_of(&s), 0.0, 5.0).unwrap();
        assert_eq!(check(&solid), counts(8, 12, 6, 0, 1, 0));
        assert!(close(measure::volume(&solid), 4000.0, 1e-9));
        let b = solid.bounds();
        assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-12));
        assert!(b.max.abs_diff_eq(DVec3::new(40.0, 20.0, 5.0), 1e-12));
    }

    #[test]
    fn plate_with_round_hole() {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(40.0, 20.0));
        s.add_circle(DVec2::new(10.0, 10.0), 3.0);
        let regions = regions_of(&s);
        let plate: Vec<Region> = regions.into_iter().filter(|r| r.holes.len() == 1).collect();
        assert_eq!(plate.len(), 1);
        let solid = extrude(&Plane::TOP, &plate, 0.0, 2.0).unwrap();
        assert_eq!(check(&solid), counts(10, 15, 7, 2, 1, 1));
        let expected = (800.0 - PI * 9.0) * 2.0;
        assert!(close(measure::volume(&solid), expected, 1e-9));
        // The hole's wall faces into the hole.
        let wall = solid
            .faces
            .iter()
            .find(|f| matches!(f.surface, Surface::Cylinder(_)))
            .unwrap();
        assert!(wall.reversed);
    }

    #[test]
    fn slot() {
        let mut s = Sketch::new();
        shapes::slot(&mut s, DVec2::ZERO, DVec2::new(10.0, 0.0), 2.0);
        let regions = regions_of(&s);
        assert_eq!(regions.len(), 1);
        let solid = extrude(&Plane::TOP, &regions, -1.0, 3.0).unwrap();
        // 4 side faces (2 planes, 2 half cylinders) + 2 caps.
        assert_eq!(check(&solid), counts(8, 12, 6, 0, 1, 0));
        let cylinders = solid
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Cylinder(_)))
            .count();
        assert_eq!(cylinders, 2);
        let expected = (40.0 + PI * 4.0) * 4.0;
        assert!(close(measure::volume(&solid), expected, 1e-9));
    }

    #[test]
    fn cylinder_from_circle() {
        let mut s = Sketch::new();
        s.add_circle(DVec2::new(1.0, 2.0), 5.0);
        let solid = extrude(&Plane::front(), &regions_of(&s), 0.0, 10.0).unwrap();
        assert_eq!(check(&solid), counts(2, 3, 3, 0, 1, 0));
        assert!(close(measure::volume(&solid), PI * 250.0, 1e-9));
        assert!(solid.edges.iter().filter(|e| e.is_closed()).count() == 2);
    }

    #[test]
    fn single_circle_loop() {
        // A hand-built region with one circle piece, wound the wrong way.
        let lp = peet_sketch::region::Loop {
            edges: vec![LoopEdge {
                entity: peet_sketch::EntityId(0),
                curve: Curve::Circle {
                    center: DVec2::ZERO,
                    radius: 2.0,
                },
                reversed: true,
            }],
            signed_area: -4.0 * PI,
        };
        let region = Region {
            outer: lp,
            holes: Vec::new(),
        };
        let solid = extrude(&Plane::TOP, &[region], 0.0, 1.0).unwrap();
        assert_eq!(check(&solid), counts(2, 3, 3, 0, 1, 0));
        assert!(close(measure::volume(&solid), 4.0 * PI, 1e-9));
    }

    #[test]
    fn l_shape() {
        let mut s = Sketch::new();
        let pts = [
            DVec2::ZERO,
            DVec2::new(80.0, 0.0),
            DVec2::new(80.0, 10.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(10.0, 60.0),
            DVec2::new(0.0, 60.0),
        ];
        for i in 0..pts.len() {
            s.add_line(pts[i], pts[(i + 1) % pts.len()]);
        }
        let solid = extrude(&Plane::right(), &regions_of(&s), 0.0, 3.0).unwrap();
        assert_eq!(check(&solid), counts(12, 18, 8, 0, 1, 0));
        assert!(close(measure::volume(&solid), 1300.0 * 3.0, 1e-9));
    }

    #[test]
    fn two_disjoint_regions() {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(10.0, 10.0));
        s.add_circle(DVec2::new(30.0, 5.0), 4.0);
        let solid = extrude(&Plane::TOP, &regions_of(&s), 0.0, 1.0).unwrap();
        assert_eq!(check(&solid), counts(10, 15, 9, 0, 2, 0));
        assert!(close(measure::volume(&solid), 100.0 + 16.0 * PI, 1e-9));
    }

    #[test]
    fn symmetric_on_tilted_plane() {
        let plane = Plane {
            frame: Frame {
                origin: DVec3::new(5.0, -3.0, 2.0),
                rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3, -0.7, 1.9),
            },
        };
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(30.0, 30.0));
        shapes::slot(&mut s, DVec2::new(8.0, 15.0), DVec2::new(22.0, 15.0), 3.0);
        let plate: Vec<Region> = regions_of(&s)
            .into_iter()
            .filter(|r| r.holes.len() == 1)
            .collect();
        let solid = extrude(&plane, &plate, -2.5, 2.5).unwrap();
        // 8 + 8 vertices, 12 + 12 edges, 6 + 4 faces, a hole in each cap.
        assert_eq!(check(&solid), counts(16, 24, 10, 2, 1, 1));
        let expected = (900.0 - (14.0 * 6.0 + PI * 9.0)) * 5.0;
        assert!(close(measure::volume(&solid), expected, 1e-8));
        // Symmetric about the sketch plane.
        for v in &solid.vertices {
            let d = plane.signed_distance(v.point);
            assert!(close(d.abs(), 2.5, 1e-9), "{d}");
        }
    }

    #[test]
    fn invalid_inputs() {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(10.0, 10.0));
        let r = regions_of(&s);
        for (from, to) in [(0.0, 0.0), (1.0, 0.0), (0.0, f64::NAN)] {
            assert!(matches!(
                extrude(&Plane::TOP, &r, from, to),
                Err(KernelError::InvalidInput(_))
            ));
        }
        let err = extrude(&Plane::TOP, &[], 0.0, 1.0).unwrap_err();
        assert!(err.to_string().contains("region"), "{err}");
    }
}
