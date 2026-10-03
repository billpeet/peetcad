//! Validity checks for solids: run after every kernel operation in debug builds and tests.
//!
//! [`validate`] checks, in order (later checks are skipped when earlier ones fail, since
//! they would only report follow-on noise or could not run safely):
//!
//! 1. **References**: every id stored in the arenas is in range.
//! 2. **Topology**: each edge has exactly two coedges, in opposite directions; coedge
//!    `next`/`prev` links agree and every loop is closed (each coedge ends where the next
//!    starts); every face has a loop and belongs to the shell it names; twins lie in the
//!    same shell; each shell is connected through its edges; every vertex is used, belongs
//!    to one shell, and its faces form a single fan (no "bow tie" vertices); the
//!    Euler–Poincaré formula gives a non-negative integer genus for every shell.
//! 3. **Geometry**: vertices lie on their edges' curves at `t0`/`t1`; points along each edge
//!    lie on both adjacent faces' surfaces; each loop's winding in its face's parameter
//!    space matches the face orientation (outer loops counter-clockwise seen from outside,
//!    holes clockwise, no loop wrapping around a cylinder); each shell encloses a positive
//!    volume, or a negative one if it is a void inside another shell.

pub mod measure;
#[cfg(test)]
pub(crate) mod test_solids;

use std::collections::HashSet;

use peet_math::tolerance;

use crate::Solid;
use crate::geom::{Curve3, Surface};
use crate::topo::{CoedgeId, EdgeId, FaceId, LoopId, ShellId, VertexId};

/// Vertices must lie this close to their edges' curves (mm).
pub const VERTEX_ON_CURVE: f64 = 100.0 * tolerance::LINEAR;

/// Edges must lie this close to their faces' surfaces (mm).
pub const EDGE_ON_SURFACE: f64 = 100.0 * tolerance::LINEAR;

/// A closed edge must span one period of its curve to within this (radians).
const PERIOD_SLACK: f64 = 1e-9;

/// Slack (radians) when checking that a cylinder face's holes stay clear of its seam.
const SEAM_SLACK: f64 = 1e-6;

/// Fractions of each edge's parameter range checked against the adjacent faces.
const EDGE_SAMPLES: [f64; 3] = [0.25, 0.5, 0.75];

/// One problem found in a solid.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Summary of a solid's topology.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub vertices: usize,
    pub edges: usize,
    pub faces: usize,
    /// Inner loops (holes in faces).
    pub rings: usize,
    pub shells: usize,
    /// Genus from the Euler–Poincaré formula `V − E + F − R = 2 (S − G)`.
    pub genus: i64,
}

/// Checks topology (closed manifold shells, consistent coedge orientation, loops closed,
/// Euler–Poincaré with a non-negative integer genus) and geometry (vertices on their edges'
/// curves, edges on their faces' surfaces, loop orientation matches face normals).
/// Returns the counts, or every problem found.
pub fn validate(solid: &Solid) -> Result<Counts, Vec<Problem>> {
    let mut v = Validator {
        s: solid,
        problems: Vec::new(),
    };
    v.references();
    if !v.problems.is_empty() {
        return Err(v.problems);
    }
    let counts = v.topology();
    if !v.problems.is_empty() {
        return Err(v.problems);
    }
    v.geometry();
    if v.problems.is_empty() {
        Ok(counts)
    } else {
        Err(v.problems)
    }
}

/// Panics with every problem if `solid` is invalid. For tests and debug assertions.
#[track_caller]
pub fn assert_valid(solid: &Solid) -> Counts {
    match validate(solid) {
        Ok(c) => c,
        Err(problems) => {
            let list: Vec<String> = problems.iter().map(|p| p.message.clone()).collect();
            panic!("invalid solid:\n  {}", list.join("\n  "));
        }
    }
}

struct Validator<'a> {
    s: &'a Solid,
    problems: Vec<Problem>,
}

impl Validator<'_> {
    fn report(&mut self, message: String) {
        self.problems.push(Problem { message });
    }

    // ---- 1. References ----

    fn references(&mut self) {
        let s = self.s;
        let (nv, ne, nc, nl, nf, ns) = (
            s.vertices.len(),
            s.edges.len(),
            s.coedges.len(),
            s.loops.len(),
            s.faces.len(),
            s.shells.len(),
        );
        for (i, e) in s.edges.iter().enumerate() {
            if e.start.index() >= nv || e.end.index() >= nv {
                self.report(format!("edge {i} refers to a missing vertex"));
            }
            if e.coedges.iter().any(|c| c.index() >= nc) {
                self.report(format!("edge {i} refers to a missing coedge"));
            }
        }
        for (i, c) in s.coedges.iter().enumerate() {
            if c.edge.index() >= ne
                || c.loop_id.index() >= nl
                || c.next.index() >= nc
                || c.prev.index() >= nc
            {
                self.report(format!(
                    "coedge {i} refers to a missing edge, loop or coedge"
                ));
            }
        }
        for (i, l) in s.loops.iter().enumerate() {
            if l.face.index() >= nf || l.first.index() >= nc {
                self.report(format!("loop {i} refers to a missing face or coedge"));
            }
        }
        for (i, f) in s.faces.iter().enumerate() {
            if f.shell.index() >= ns || f.loops.iter().any(|l| l.index() >= nl) {
                self.report(format!("face {i} refers to a missing shell or loop"));
            }
        }
        for (i, sh) in s.shells.iter().enumerate() {
            if sh.faces.iter().any(|f| f.index() >= nf) {
                self.report(format!("shell {i} refers to a missing face"));
            }
        }
    }

    // ---- 2. Topology ----

    fn topology(&mut self) -> Counts {
        self.edges_and_coedges();
        self.loops_and_faces();
        if !self.problems.is_empty() {
            return Counts::default();
        }
        self.shells_and_vertices()
    }

    fn edges_and_coedges(&mut self) {
        let s = self.s;
        for (i, e) in s.edges.iter().enumerate() {
            let faces: Vec<String> = e
                .coedges
                .iter()
                .map(|&c| s.loop_(s.coedge(c).loop_id).face.0.to_string())
                .collect();
            if e.coedges.len() != 2 {
                self.report(format!(
                    "edge {i} has {} coedge{} (face{} {}); a closed solid needs exactly 2",
                    e.coedges.len(),
                    if e.coedges.len() == 1 { "" } else { "s" },
                    if faces.len() == 1 { "" } else { "s" },
                    faces.join(", ")
                ));
            } else {
                let (a, b) = (s.coedge(e.coedges[0]), s.coedge(e.coedges[1]));
                if a.reversed == b.reversed {
                    self.report(format!(
                        "edge {i} is used in the same direction by both its coedges (faces {}); \
                         one of the faces is flipped",
                        faces.join(", ")
                    ));
                }
            }
            for &c in &e.coedges {
                if s.coedge(c).edge.index() != i {
                    self.report(format!(
                        "edge {i} lists coedge {} which belongs to edge {}",
                        c.0,
                        s.coedge(c).edge.0
                    ));
                }
            }
        }
        for (i, c) in s.coedges.iter().enumerate() {
            let id = CoedgeId(i as u32);
            if !s.edge(c.edge).coedges.contains(&id) {
                self.report(format!("coedge {i} is not listed by its edge {}", c.edge.0));
            }
            if s.coedge(c.next).prev != id {
                self.report(format!(
                    "coedge {i}: next is {} but that coedge's prev is {}",
                    c.next.0,
                    s.coedge(c.next).prev.0
                ));
            }
            if s.coedge(c.next).loop_id != c.loop_id {
                self.report(format!(
                    "coedge {i} (loop {}) links to coedge {} of loop {}",
                    c.loop_id.0,
                    c.next.0,
                    s.coedge(c.next).loop_id.0
                ));
            }
            let end = s.coedge_end(id);
            let next_start = s.coedge_start(c.next);
            if end != next_start {
                self.report(format!(
                    "loop {} of face {} is not closed: coedge {i} ends at vertex {} but the \
                     next coedge {} starts at vertex {}",
                    c.loop_id.0,
                    s.loop_(c.loop_id).face.0,
                    end.0,
                    c.next.0,
                    next_start.0
                ));
            }
        }
    }

    fn loops_and_faces(&mut self) {
        let s = self.s;
        let mut seen = vec![0usize; s.loops.len()];
        for c in &s.coedges {
            seen[c.loop_id.index()] += 1;
        }
        for (i, l) in s.loops.iter().enumerate() {
            let id = LoopId(i as u32);
            if s.coedge(l.first).loop_id != id {
                self.report(format!(
                    "loop {i} starts at coedge {} which belongs to loop {}",
                    l.first.0,
                    s.coedge(l.first).loop_id.0
                ));
                continue;
            }
            let walked = s.loop_coedges(id).len();
            if walked != seen[i] {
                self.report(format!(
                    "loop {i} of face {}: walking it visits {walked} coedges, but {} coedges \
                     belong to it",
                    l.face.0, seen[i]
                ));
            }
            if !s.face(l.face).loops.contains(&id) {
                self.report(format!("loop {i} is not listed by its face {}", l.face.0));
            }
        }
        let mut in_shells = vec![0usize; s.faces.len()];
        for sh in &s.shells {
            for f in &sh.faces {
                in_shells[f.index()] += 1;
            }
        }
        for (i, f) in s.faces.iter().enumerate() {
            if f.loops.is_empty() {
                self.report(format!("face {i} has no loops"));
            }
            for &l in &f.loops {
                if s.loop_(l).face.index() != i {
                    self.report(format!(
                        "face {i} lists loop {} which belongs to face {}",
                        l.0,
                        s.loop_(l).face.0
                    ));
                }
            }
            if !s.shell(f.shell).faces.contains(&FaceId(i as u32)) {
                self.report(format!(
                    "face {i} names shell {} but that shell doesn't list it",
                    f.shell.0
                ));
            }
            if in_shells[i] != 1 {
                self.report(format!(
                    "face {i} is listed by {} shells (should be exactly 1)",
                    in_shells[i]
                ));
            }
        }
    }

    fn shells_and_vertices(&mut self) -> Counts {
        let s = self.s;
        let mut counts = Counts {
            shells: s.shells.len(),
            ..Counts::default()
        };
        let mut vertex_shell: Vec<Option<ShellId>> = vec![None; s.vertices.len()];
        for (si, sh) in s.shells.iter().enumerate() {
            let shell = ShellId(si as u32);
            if sh.faces.is_empty() {
                self.report(format!("shell {si} has no faces"));
                continue;
            }
            let mut edges: HashSet<EdgeId> = HashSet::new();
            let mut vertices: HashSet<VertexId> = HashSet::new();
            let mut rings = 0usize;
            // Union-find over the shell's faces.
            let local: std::collections::HashMap<FaceId, usize> =
                sh.faces.iter().enumerate().map(|(k, &f)| (f, k)).collect();
            let mut parent: Vec<usize> = (0..sh.faces.len()).collect();
            fn find(p: &mut [usize], mut x: usize) -> usize {
                while p[x] != x {
                    p[x] = p[p[x]];
                    x = p[x];
                }
                x
            }
            for &f in &sh.faces {
                let face = s.face(f);
                rings += face.loops.len().saturating_sub(1);
                for &l in &face.loops {
                    for c in s.loop_coedges(l) {
                        let e = s.coedge(c).edge;
                        edges.insert(e);
                        vertices.insert(s.edge(e).start);
                        vertices.insert(s.edge(e).end);
                        let Some(t) = s.twin(c) else { continue };
                        let other = s.coedge_face(t);
                        match local.get(&other) {
                            Some(&k) => {
                                let (a, b) = (find(&mut parent, local[&f]), find(&mut parent, k));
                                parent[a] = b;
                            }
                            None => self.report(format!(
                                "edge {} joins face {} (shell {si}) to face {} in another shell",
                                e.0, f.0, other.0
                            )),
                        }
                    }
                }
            }
            let roots: HashSet<usize> = (0..sh.faces.len()).map(|k| find(&mut parent, k)).collect();
            if roots.len() > 1 {
                self.report(format!(
                    "shell {si} falls apart into {} pieces that share no edges",
                    roots.len()
                ));
            }
            for &v in &vertices {
                match vertex_shell[v.index()] {
                    Some(other) if other != shell => self.report(format!(
                        "vertex {} is shared by shells {} and {si}",
                        v.0, other.0
                    )),
                    _ => vertex_shell[v.index()] = Some(shell),
                }
            }
            let (nv, ne, nf) = (vertices.len(), edges.len(), sh.faces.len());
            let chi = nv as i64 - ne as i64 + nf as i64 - rings as i64;
            let twice_genus = 2 - chi;
            if twice_genus < 0 || twice_genus % 2 != 0 {
                self.report(format!(
                    "shell {si} fails the Euler–Poincaré check: V − E + F − R = {nv} − {ne} + \
                     {nf} − {rings} = {chi}, which gives no non-negative integer genus"
                ));
            }
            counts.vertices += nv;
            counts.edges += ne;
            counts.faces += nf;
            counts.rings += rings;
            counts.genus += twice_genus / 2;
        }
        for (i, owner) in vertex_shell.iter().enumerate() {
            if owner.is_none() {
                self.report(format!("vertex {i} is not used by any edge of a shell"));
            }
        }
        for (i, e) in s.edges.iter().enumerate() {
            if e.coedges.is_empty() {
                self.report(format!("edge {i} is not used by any face"));
            }
        }
        self.vertex_fans();
        counts
    }

    /// Around each vertex, the faces must form one fan: following `twin(next(c))` from an
    /// incoming coedge visits every incoming coedge of the vertex in one cycle.
    fn vertex_fans(&mut self) {
        let s = self.s;
        let mut incoming: Vec<Vec<CoedgeId>> = vec![Vec::new(); s.vertices.len()];
        for c in 0..s.coedges.len() as u32 {
            let c = CoedgeId(c);
            incoming[s.coedge_end(c).index()].push(c);
        }
        for (v, list) in incoming.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let mut visited: HashSet<CoedgeId> = HashSet::new();
            let mut c = list[0];
            while visited.insert(c) {
                let Some(t) = s.twin(s.coedge(c).next) else {
                    return; // already reported as a non-manifold edge
                };
                c = t;
            }
            if visited.len() != list.len() {
                self.report(format!(
                    "vertex {v} is non-manifold: its faces form more than one fan \
                     ({} of {} incoming coedges reached)",
                    visited.len(),
                    list.len()
                ));
            }
        }
    }

    // ---- 3. Geometry ----

    fn geometry(&mut self) {
        let s = self.s;
        for (i, e) in s.edges.iter().enumerate() {
            if !(e.t0.is_finite() && e.t1.is_finite() && e.t0 < e.t1) {
                self.report(format!(
                    "edge {i} has an empty or invalid parameter range {}..{}",
                    e.t0, e.t1
                ));
                continue;
            }
            if let Some((lo, hi)) = e.curve.domain()
                && (e.t0 < lo - PERIOD_SLACK || e.t1 > hi + PERIOD_SLACK)
            {
                self.report(format!(
                    "edge {i} runs from {} to {}, past the ends of its curve ({lo} to {hi})",
                    e.t0, e.t1
                ));
                continue;
            }
            if let Some(problem) = curve_problem(&e.curve) {
                self.report(format!("edge {i}: {problem}"));
                continue;
            }
            match (e.is_closed(), e.curve.period()) {
                (true, None) if matches!(e.curve, Curve3::Line(_)) => {
                    self.report(format!("edge {i} is a closed line"));
                }
                (true, Some(p)) if ((e.t1 - e.t0) - p).abs() > PERIOD_SLACK => {
                    self.report(format!(
                        "closed edge {i} spans {} rad instead of one full period",
                        e.t1 - e.t0
                    ));
                }
                (false, Some(p)) if e.t1 - e.t0 >= p - PERIOD_SLACK => {
                    self.report(format!(
                        "edge {i} spans a full period but has two different vertices"
                    ));
                }
                _ => {}
            }
            for (t, v, which) in [(e.t0, e.start, "start"), (e.t1, e.end, "end")] {
                let d = e.curve.point(t).distance(s.vertex(v).point);
                if d > VERTEX_ON_CURVE {
                    self.report(format!(
                        "edge {i}'s {which} vertex {} is {d:.3e} mm from the curve at t = {t}",
                        v.0
                    ));
                }
            }
            for &c in &e.coedges {
                let f = s.coedge_face(c);
                let surface = &s.face(f).surface;
                let worst = EDGE_SAMPLES
                    .iter()
                    .map(|&k| surface.signed_distance(e.point_at_fraction(k)).abs())
                    .fold(0.0, f64::max);
                if worst > EDGE_ON_SURFACE {
                    self.report(format!(
                        "edge {i} leaves the surface of face {} by {worst:.3e} mm",
                        f.0
                    ));
                }
            }
        }
        for (i, f) in s.faces.iter().enumerate() {
            if let Some(problem) = surface_problem(&f.surface) {
                self.report(format!("face {i}: {problem}"));
            }
        }
        if !self.problems.is_empty() {
            return;
        }
        self.orientation();
    }

    fn orientation(&mut self) {
        let s = self.s;
        let bounds = s.bounds();
        let size = bounds.size().length().max(tolerance::LINEAR);
        let min_area = tolerance::LINEAR * size;
        let mut volumes = Vec::with_capacity(s.shells.len());
        for (si, sh) in s.shells.iter().enumerate() {
            let reference = measure::shell_reference(s, ShellId(si as u32));
            let mut volume = 0.0;
            for &f in &sh.faces {
                let face = s.face(f);
                let sign = if face.reversed { -1.0 } else { 1.0 };
                let mut outer_range = (f64::NEG_INFINITY, f64::INFINITY);
                for (k, &l) in face.loops.iter().enumerate() {
                    let m = measure::loop_integrals(s, l, reference);
                    volume += m.volume;
                    if face.surface.is_periodic_u() {
                        if k == 0 {
                            outer_range = m.theta_range;
                        } else if !fits_in_turns(m.theta_range, outer_range) {
                            self.report(format!(
                                "hole {} of face {} crosses the face's seam (or leaves its                                  angular range); split the hole's edges at the seam",
                                l.0, f.0
                            ));
                        }
                    }
                    if m.winding.abs() > std::f64::consts::PI
                        || m.winding_v.abs() > std::f64::consts::PI
                    {
                        let kind = surface_name(&face.surface);
                        self.report(format!(
                            "loop {} of face {} wraps around the {kind}; a full {kind} \
                             needs a seam edge",
                            l.0, f.0
                        ));
                        continue;
                    }
                    let area = m.signed_area * sign;
                    let what = if k == 0 { "outer loop" } else { "hole" };
                    if area.abs() <= min_area {
                        self.report(format!("{what} {} of face {} encloses no area", l.0, f.0));
                    } else if (area > 0.0) != (k == 0) {
                        self.report(format!(
                            "{what} {} of face {} runs {} seen from outside (should be {}); \
                             the loop or the face's `reversed` flag is wrong",
                            l.0,
                            f.0,
                            if area > 0.0 {
                                "counter-clockwise"
                            } else {
                                "clockwise"
                            },
                            if k == 0 {
                                "counter-clockwise"
                            } else {
                                "clockwise"
                            },
                        ));
                    }
                }
            }
            volumes.push(volume);
        }
        let min_volume = tolerance::LINEAR * size * size;
        for (si, &vol) in volumes.iter().enumerate() {
            if vol.abs() <= min_volume {
                self.report(format!("shell {si} encloses no volume ({vol:.3e} mm³)"));
            } else if vol < 0.0 && !self.is_void(si, &volumes) {
                self.report(format!(
                    "shell {si} is inside out: its faces point inwards (volume {vol:.3e} mm³)"
                ));
            }
        }
    }

    /// Whether negative shell `si` lies inside a positive shell (by bounding boxes).
    fn is_void(&self, si: usize, volumes: &[f64]) -> bool {
        let s = self.s;
        let shell_bounds = |k: usize| {
            let mut b = peet_math::Aabb::EMPTY;
            for &f in &s.shell(ShellId(k as u32)).faces {
                b = b.union(&measure::bulge_bounds(s, f));
                for &l in &s.face(f).loops {
                    for c in s.loop_coedges(l) {
                        let e = s.edge(s.coedge(c).edge);
                        for i in 0..=8 {
                            b.extend(e.point_at_fraction(f64::from(i) / 8.0));
                        }
                    }
                }
            }
            b
        };
        let inner = shell_bounds(si);
        (0..volumes.len()).any(|k| {
            k != si && volumes[k] > 0.0 && {
                let outer = shell_bounds(k);
                // A void may touch the outer shell from inside (a hole tangent to a wall).
                let slack = peet_math::DVec3::splat(tolerance::LINEAR);
                (outer.min - slack).cmple(inner.min).all()
                    && inner.max.cmple(outer.max + slack).all()
            }
        })
    }
}

/// Whether angular range `inner`, shifted by some whole number of turns, lies inside `outer`.
fn fits_in_turns(inner: (f64, f64), outer: (f64, f64)) -> bool {
    let turn = std::f64::consts::TAU;
    let k = ((outer.0 - inner.0 - SEAM_SLACK) / turn).ceil() * turn;
    inner.0 + k >= outer.0 - SEAM_SLACK && inner.1 + k <= outer.1 + SEAM_SLACK
}

fn curve_problem(c: &Curve3) -> Option<String> {
    match c {
        Curve3::Line(l) => ((l.dir.length() - 1.0).abs() > 1e-9 || !l.origin.is_finite())
            .then(|| "line direction is not a unit vector".to_owned()),
        Curve3::Circle(c) => (!(c.radius > tolerance::LINEAR && c.radius.is_finite()))
            .then(|| format!("circle radius {} is not positive", c.radius)),
        Curve3::Ellipse(e) => {
            (!(e.minor > tolerance::LINEAR && e.major >= e.minor && e.major.is_finite()))
                .then(|| format!("ellipse axes {} / {} are invalid", e.major, e.minor))
        }
        // Checked when it was made.
        Curve3::Nurbs(_) => None,
    }
}

fn surface_problem(s: &Surface) -> Option<String> {
    match s {
        Surface::Plane(p) => (!p.origin().is_finite()).then(|| "plane is not finite".to_owned()),
        Surface::Cylinder(c) => (!(c.radius > tolerance::LINEAR && c.radius.is_finite()))
            .then(|| format!("cylinder radius {} is not positive", c.radius)),
        Surface::Cone(c) => {
            let angle = c.half_angle.abs();
            (!(c.radius >= 0.0
                && c.radius.is_finite()
                && angle > tolerance::ANGULAR
                && angle < std::f64::consts::FRAC_PI_2 - tolerance::ANGULAR))
                .then(|| {
                    format!(
                        "cone radius {} or half angle {} is invalid",
                        c.radius, c.half_angle
                    )
                })
        }
        Surface::Sphere(s) => (!(s.radius > tolerance::LINEAR && s.radius.is_finite()))
            .then(|| format!("sphere radius {} is not positive", s.radius)),
        Surface::Torus(t) => (!(t.minor > tolerance::LINEAR
            && t.major > tolerance::LINEAR
            && t.major.is_finite()
            && t.minor.is_finite()))
        .then(|| format!("torus radii {} / {} are invalid", t.major, t.minor)),
        Surface::Nurbs(_) => None,
    }
}

fn surface_name(s: &Surface) -> &'static str {
    match s {
        Surface::Plane(_) => "plane",
        Surface::Cylinder(_) => "cylinder",
        Surface::Cone(_) => "cone",
        Surface::Sphere(_) => "sphere",
        Surface::Torus(_) => "torus",
        Surface::Nurbs(_) => "freeform surface",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Cylinder;
    use crate::topo::test_shapes::cuboid;
    use peet_math::{DVec3, Frame};

    fn unit_box() -> Solid {
        cuboid(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0))
    }

    fn messages(s: &Solid) -> Vec<String> {
        validate(s)
            .expect_err("should be invalid")
            .into_iter()
            .map(|p| p.message)
            .collect()
    }

    fn any_contains(m: &[String], needle: &str) -> bool {
        m.iter().any(|x| x.contains(needle))
    }

    #[test]
    fn cuboid_is_valid() {
        let c = assert_valid(&unit_box());
        assert_eq!(
            c,
            Counts {
                vertices: 8,
                edges: 12,
                faces: 6,
                rings: 0,
                shells: 1,
                genus: 0
            }
        );
    }

    #[test]
    fn missing_face_leaves_open_edges() {
        let mut s = unit_box();
        // Drop the last face from its shell and detach its loop's coedges from the edges.
        let f = s.shells[0].faces.pop().unwrap();
        let lp = s.faces[f.index()].loops[0];
        for c in s.loop_coedges(lp) {
            let e = s.coedges[c.index()].edge;
            s.edges[e.index()].coedges.retain(|&x| x != c);
        }
        let m = messages(&s);
        assert!(any_contains(&m, "has 1 coedge (face"), "{m:#?}");
    }

    #[test]
    fn flipped_face() {
        let mut s = unit_box();
        // Reverse loop 0 by flipping every coedge and swapping next/prev.
        for c in s.loop_coedges(LoopId(0)) {
            let co = &mut s.coedges[c.index()];
            co.reversed = !co.reversed;
            std::mem::swap(&mut co.next, &mut co.prev);
        }
        let m = messages(&s);
        assert!(
            any_contains(&m, "same direction by both its coedges"),
            "{m:#?}"
        );
    }

    #[test]
    fn broken_loop_link() {
        let mut s = unit_box();
        let c0 = s.loops[0].first;
        let c1 = s.coedges[c0.index()].next;
        let c2 = s.coedges[c1.index()].next;
        s.coedges[c0.index()].next = c2;
        let m = messages(&s);
        assert!(any_contains(&m, "prev is"), "{m:#?}");
        assert!(any_contains(&m, "is not closed"), "{m:#?}");
    }

    #[test]
    fn reversed_flag_wrong() {
        let mut s = unit_box();
        s.faces[2].reversed = true;
        let m = messages(&s);
        assert!(any_contains(&m, "face 2"), "{m:#?}");
        assert!(any_contains(&m, "leaves the surface") || any_contains(&m, "runs clockwise"));
    }

    #[test]
    fn inside_out_solid() {
        let mut s = unit_box();
        // Flip every loop and every face: topology stays consistent, normals point in.
        for l in 0..s.loops.len() as u32 {
            for c in s.loop_coedges(LoopId(l)) {
                let co = &mut s.coedges[c.index()];
                co.reversed = !co.reversed;
                std::mem::swap(&mut co.next, &mut co.prev);
            }
        }
        for f in &mut s.faces {
            f.reversed = !f.reversed;
        }
        let m = messages(&s);
        assert!(any_contains(&m, "inside out"), "{m:#?}");
    }

    #[test]
    fn vertex_off_its_edge() {
        let mut s = unit_box();
        s.vertices[7].point += DVec3::new(0.0, 0.0, 0.01);
        let m = messages(&s);
        assert!(any_contains(&m, "vertex 7 is"), "{m:#?}");
    }

    #[test]
    fn edge_off_its_face() {
        let mut s = unit_box();
        // Swap a face's plane for a cylinder that doesn't contain its edges.
        s.faces[0].surface = Surface::Cylinder(Cylinder {
            frame: Frame::WORLD,
            radius: 5.0,
        });
        let m = messages(&s);
        assert!(any_contains(&m, "leaves the surface of face 0"), "{m:#?}");
    }

    #[test]
    fn two_separate_boxes_in_one_shell() {
        let mut a = unit_box();
        let b = cuboid(DVec3::splat(5.0), DVec3::splat(6.0));
        // Append b's elements into a's single shell.
        let (nv, ne, nc, nl, nf) = (
            a.vertices.len() as u32,
            a.edges.len() as u32,
            a.coedges.len() as u32,
            a.loops.len() as u32,
            a.faces.len() as u32,
        );
        a.vertices.extend(b.vertices.iter().cloned());
        for e in &b.edges {
            let mut e = e.clone();
            e.start.0 += nv;
            e.end.0 += nv;
            for c in &mut e.coedges {
                c.0 += nc;
            }
            a.edges.push(e);
        }
        for c in &b.coedges {
            let mut c = c.clone();
            c.edge.0 += ne;
            c.loop_id.0 += nl;
            c.next.0 += nc;
            c.prev.0 += nc;
            a.coedges.push(c);
        }
        for l in &b.loops {
            let mut l = l.clone();
            l.face.0 += nf;
            l.first.0 += nc;
            a.loops.push(l);
        }
        for f in &b.faces {
            let mut f = f.clone();
            for l in &mut f.loops {
                l.0 += nl;
            }
            a.faces.push(f);
            a.shells[0].faces.push(FaceId(a.faces.len() as u32 - 1));
        }
        let m = messages(&a);
        assert!(any_contains(&m, "falls apart into 2 pieces"), "{m:#?}");
        assert!(any_contains(&m, "Euler"), "{m:#?}");
    }

    fn extruded_cylinder() -> Solid {
        let lp = peet_sketch::region::Loop {
            edges: vec![peet_sketch::region::LoopEdge {
                entity: peet_sketch::EntityId(0),
                curve: peet_sketch::Curve::Circle {
                    center: peet_math::DVec2::ZERO,
                    radius: 2.0,
                },
                reversed: false,
            }],
            signed_area: 4.0 * std::f64::consts::PI,
        };
        let region = peet_sketch::region::Region {
            outer: lp,
            holes: Vec::new(),
        };
        crate::extrude::extrude(&peet_math::Plane::TOP, &[region], 0.0, 3.0).unwrap()
    }

    #[test]
    fn cylinder_measures_and_flips() {
        let s = extruded_cylinder();
        assert_valid(&s);
        let side = s
            .face_ids()
            .find(|&f| matches!(s.face(f).surface, Surface::Cylinder(_)))
            .unwrap();
        let area = measure::face_area(&s, side);
        assert!(
            (area - 2.0 * std::f64::consts::PI * 2.0 * 3.0).abs() < 1e-9,
            "{area}"
        );
        let mut bad = s.clone();
        bad.faces[side.index()].reversed = true;
        let m = messages(&bad);
        assert!(
            any_contains(&m, &format!("of face {} runs clockwise", side.0)),
            "{m:#?}"
        );
    }

    #[test]
    fn loop_wrapping_a_cylinder() {
        // A cylinder whose side face is bounded by its two circles without a seam: valid
        // topology (V − E + F − R = 2 − 2 + 3 − 1 = 2) but the face is not simply connected.
        use crate::geom::{Circle3, Curve3};
        use peet_math::Plane;
        let mut s = Solid::new();
        let shell = s.add_shell();
        let frame = |z: f64| Frame {
            origin: DVec3::new(0.0, 0.0, z),
            rotation: peet_math::DQuat::IDENTITY,
        };
        let vb = s.add_vertex(DVec3::new(2.0, 0.0, 0.0));
        let vt = s.add_vertex(DVec3::new(2.0, 0.0, 3.0));
        let circle = |z| {
            Curve3::Circle(Circle3 {
                frame: frame(z),
                radius: 2.0,
            })
        };
        let tau = std::f64::consts::TAU;
        let bottom = s.add_edge(circle(0.0), vb, vb, 0.0, tau);
        let top = s.add_edge(circle(3.0), vt, vt, 0.0, tau);
        let side = s.add_face(
            shell,
            Surface::Cylinder(Cylinder {
                frame: frame(0.0),
                radius: 2.0,
            }),
            false,
        );
        s.add_loop(side, &[(bottom, false)]);
        s.add_loop(side, &[(top, true)]);
        let t = s.add_face(shell, Surface::Plane(Plane { frame: frame(3.0) }), false);
        s.add_loop(t, &[(top, false)]);
        let b = s.add_face(shell, Surface::Plane(Plane { frame: frame(0.0) }), true);
        s.add_loop(b, &[(bottom, true)]);
        let m = messages(&s);
        assert!(any_contains(&m, "wraps around the cylinder"), "{m:#?}");
    }

    #[test]
    fn ellipse_edges_and_cylinder_holes() {
        let s = test_solids::oblique_cylinder(5.0, 10.0, 0.5);
        assert_eq!(
            assert_valid(&s),
            Counts {
                vertices: 2,
                edges: 3,
                faces: 3,
                rings: 0,
                shells: 1,
                genus: 0
            }
        );
        let pi = std::f64::consts::PI;
        assert!((measure::volume(&s) - pi * 250.0).abs() < 1e-8);
        let areas: Vec<f64> = s.face_ids().map(|f| measure::face_area(&s, f)).collect();
        assert!((areas[0] - 2.0 * pi * 50.0).abs() < 1e-8, "{areas:?}");
        assert!(
            (areas[1] - pi * 25.0 / 0.5f64.cos()).abs() < 1e-8,
            "{areas:?}"
        );
        assert!((areas[2] - pi * 25.0).abs() < 1e-8, "{areas:?}");

        let s = test_solids::pocketed_cylinder();
        let c = assert_valid(&s);
        assert_eq!(
            (c.vertices, c.edges, c.faces, c.rings, c.genus),
            (10, 15, 8, 1, 0)
        );
        assert!((measure::volume(&s) - test_solids::pocketed_cylinder_volume()).abs() < 1e-8);
    }

    #[test]
    fn hole_across_the_seam() {
        // Move the pocketed cylinder's seam into the pocket's angular range.
        let mut s = test_solids::pocketed_cylinder();
        for v in 0..2 {
            s.vertices[v].point.x = 5.0;
        }
        for e in 0..2 {
            s.edges[e].t0 = 0.0;
            s.edges[e].t1 = std::f64::consts::TAU;
        }
        s.edges[2].curve =
            crate::geom::Curve3::line_through(s.vertices[0].point, s.vertices[1].point).unwrap();
        let m = messages(&s);
        assert!(any_contains(&m, "crosses the face's seam"), "{m:#?}");
    }

    #[test]
    fn bad_reference() {
        let mut s = unit_box();
        s.coedges[3].next = CoedgeId(999);
        let m = messages(&s);
        assert!(any_contains(&m, "coedge 3 refers to a missing"), "{m:#?}");
    }
}
