//! 3D sketches: points and lines in space, as SolidWorks' 3D sketch has them, held where
//! they are by relations and dimensions solved in space. A structural member can follow
//! one's lines (Weldment 30x30x2's frame runs along 3DSketch1).
//!
//! **What it holds.** Points (each where it was last put), lines between two of them, and
//! relations: two points together, a point on a line or at its middle, a line along an
//! axis of the model (SolidWorks' Along X, Y and Z), two lines parallel, square or as long
//! as each other, a point fixed where it is, and distances: between two points, a point
//! and a line, or two parallel lines. A distance is a dimension with a name (`D1`) and a
//! value that can be an expression; it is one of the feature's numeric values, so it has
//! an address (`D1@3DSketch1`), differs between configurations and is set by `set`.
//!
//! **Solving.** Every rebuild solves the relations for the points from where they are,
//! moving them as little as it can (minimum-norm steps), with every line along an axis of
//! the model keeping its direction where that can be: what a new value doesn't ask to
//! change stays as it is, as in SolidWorks, and isn't turned to share the change out.
//! Nothing is stored back: the drawing is kept as it was drawn, and solved again from
//! there.

use peet_math::DVec3;
use peet_solve::{Equation, MAX_SLOTS, Options, Problem, Space};
use serde::{Deserialize, Serialize};

use crate::feature::Scalar;

/// The most dimensions a 3D sketch can have: each is one of its numeric values, named
/// `d1` to `d64` (and given a name of its own, `D1`, as an address).
pub const MAX_DIMENSIONS: usize = 64;

/// The field names of the dimensions, in order.
pub const DIMENSION_FIELDS: [&str; MAX_DIMENSIONS] = [
    "d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8", "d9", "d10", "d11", "d12", "d13", "d14", "d15",
    "d16", "d17", "d18", "d19", "d20", "d21", "d22", "d23", "d24", "d25", "d26", "d27", "d28",
    "d29", "d30", "d31", "d32", "d33", "d34", "d35", "d36", "d37", "d38", "d39", "d40", "d41",
    "d42", "d43", "d44", "d45", "d46", "d47", "d48", "d49", "d50", "d51", "d52", "d53", "d54",
    "d55", "d56", "d57", "d58", "d59", "d60", "d61", "d62", "d63", "d64",
];

/// An axis of the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis3 {
    X,
    Y,
    Z,
}

impl Axis3 {
    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

/// A line between two points (indices into [`Sketch3dFeature::points`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line3 {
    pub start: u32,
    pub end: u32,
    /// Drawn to help: a structural member doesn't follow it.
    pub construction: bool,
}

/// What a distance is measured between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ref3 {
    Point(u32),
    Line(u32),
}

/// A relation of a 3D sketch. Points and lines are indices into its lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Relation3 {
    /// Two points in one place.
    Coincident(u32, u32),
    /// A point on a line (or the line carried on).
    OnLine(u32, u32),
    /// A point at the middle of a line.
    Midpoint(u32, u32),
    /// A line along an axis of the model.
    Along(Axis3, u32),
    Parallel(u32, u32),
    Perpendicular(u32, u32),
    /// Two lines as long as each other.
    Equal(u32, u32),
    /// A point kept where it is.
    Fixed(u32),
    /// A distance: between two points, a point and a line, or two parallel lines. The
    /// `dimension`th of the sketch's dimensions (see [`DIMENSION_FIELDS`]).
    Distance {
        a: Ref3,
        b: Ref3,
        dimension: u32,
    },
}

/// A sketch in space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sketch3dFeature {
    pub points: Vec<[f64; 3]>,
    pub lines: Vec<Line3>,
    pub relations: Vec<Relation3>,
    /// The dimensions' values, in order (their names are the feature's value names).
    pub dimensions: Vec<Scalar>,
}

impl Sketch3dFeature {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            lines: Vec::new(),
            relations: Vec::new(),
            dimensions: Vec::new(),
        }
    }

    /// Whether every point, line and dimension a relation names is there.
    pub fn check(&self) -> Result<(), String> {
        let (np, nl) = (self.points.len() as u32, self.lines.len() as u32);
        if self.dimensions.len() > MAX_DIMENSIONS {
            return Err(format!(
                "A 3D sketch has at most {MAX_DIMENSIONS} dimensions."
            ));
        }
        for l in &self.lines {
            if l.start >= np || l.end >= np || l.start == l.end {
                return Err("A line of the 3D sketch has a missing end.".to_owned());
            }
        }
        let point = |p: u32| p < np;
        let line = |l: u32| l < nl;
        let r#ref = |r: &Ref3| match *r {
            Ref3::Point(p) => point(p),
            Ref3::Line(l) => line(l),
        };
        for r in &self.relations {
            let ok = match r {
                Relation3::Coincident(a, b) => point(*a) && point(*b),
                Relation3::OnLine(p, l) | Relation3::Midpoint(p, l) => point(*p) && line(*l),
                Relation3::Along(_, l) => line(*l),
                Relation3::Parallel(a, b)
                | Relation3::Perpendicular(a, b)
                | Relation3::Equal(a, b) => line(*a) && line(*b),
                Relation3::Fixed(p) => point(*p),
                Relation3::Distance { a, b, dimension } => {
                    r#ref(a) && r#ref(b) && (*dimension as usize) < self.dimensions.len()
                }
            };
            if !ok {
                return Err(
                    "A relation of the 3D sketch names something it doesn't have.".to_owned(),
                );
            }
        }
        Ok(())
    }

    /// The points solved for the relations, with the dimensions' `values` (in order, mm).
    /// `Err` says what can't be met.
    pub fn solve(&self, values: &[f64]) -> Result<Vec<DVec3>, String> {
        self.check()?;
        let mut eqs: Vec<Eq3> = Vec::new();
        // Each distance's first end and its equation
        let mut distances: Vec<(Ref3, Ref3, usize)> = Vec::new();
        let slots = |p: u32| [3 * p, 3 * p + 1, 3 * p + 2];
        let line = |l: u32| {
            let l = self.lines[l as usize];
            [slots(l.start), slots(l.end)].concat()
        };
        for r in &self.relations {
            match r {
                Relation3::Coincident(a, b) => {
                    for c in 0..3 {
                        eqs.push(Eq3::new(
                            Kind::Coincident(c),
                            [slots(*a), slots(*b)].concat(),
                        ));
                    }
                }
                Relation3::OnLine(p, l) => {
                    for c in 0..3 {
                        eqs.push(Eq3::new(
                            Kind::OnLine(c),
                            [slots(*p).to_vec(), line(*l)].concat(),
                        ));
                    }
                }
                Relation3::Midpoint(p, l) => {
                    for c in 0..3 {
                        eqs.push(Eq3::new(
                            Kind::Midpoint(c),
                            [slots(*p).to_vec(), line(*l)].concat(),
                        ));
                    }
                }
                Relation3::Along(axis, l) => {
                    for c in (0..3).filter(|c| *c != axis.index()) {
                        eqs.push(Eq3::new(Kind::Along(c), line(*l)));
                    }
                }
                Relation3::Parallel(a, b) => {
                    for c in 0..3 {
                        eqs.push(Eq3::new(Kind::Parallel(c), [line(*a), line(*b)].concat()));
                    }
                }
                Relation3::Perpendicular(a, b) => {
                    eqs.push(Eq3::new(Kind::Perpendicular, [line(*a), line(*b)].concat()));
                }
                Relation3::Equal(a, b) => {
                    eqs.push(Eq3::new(Kind::Equal, [line(*a), line(*b)].concat()));
                }
                Relation3::Fixed(p) => {
                    let at = self.points[*p as usize];
                    for (c, &v) in at.iter().enumerate() {
                        eqs.push(Eq3::new(Kind::Fixed(c, v), slots(*p).to_vec()));
                    }
                }
                Relation3::Distance { a, b, dimension } => {
                    let value = values
                        .get(*dimension as usize)
                        .copied()
                        .unwrap_or_else(|| self.dimensions[*dimension as usize].value);
                    let (kind, s) = match (*a, *b) {
                        (Ref3::Point(p), Ref3::Point(q)) => {
                            (Kind::PointPoint(value), [slots(p), slots(q)].concat())
                        }
                        (Ref3::Point(p), Ref3::Line(l)) | (Ref3::Line(l), Ref3::Point(p)) => (
                            Kind::PointLine(value),
                            [slots(p).to_vec(), line(l)].concat(),
                        ),
                        // Two parallel lines: from a point of the second to the first
                        (Ref3::Line(l), Ref3::Line(m)) => (
                            Kind::PointLine(value),
                            [line(m)[..3].to_vec(), line(l)].concat(),
                        ),
                    };
                    distances.push((*a, *b, eqs.len()));
                    eqs.push(Eq3::new(kind, s));
                }
            }
        }
        let start: Vec<f64> = self.points.iter().flatten().copied().collect();
        if eqs.is_empty() {
            return Ok(points(&start));
        }
        let run = |eqs: &[Eq3]| {
            let mut vals = start.clone();
            let mut used: Vec<u32> = eqs.iter().flat_map(|e| e.slots.iter().copied()).collect();
            used.sort_unstable();
            used.dedup();
            let rows: Vec<u32> = (0..eqs.len() as u32).collect();
            let mut problem = Problem::new(used, rows, eqs, Space::Equations);
            let out = problem.solve(
                eqs,
                &mut vals,
                Options {
                    max_iter: 200,
                    hard: true,
                    dogleg: true,
                },
            );
            (out, vals)
        };
        // What the dimensions don't ask to change stays as it is, as in SolidWorks: first
        // with every line along an axis of the model keeping its direction (moving
        // everything as little as can be would also turn what the relations leave free to
        // turn, to share a change out; a slanting line, a frame's diagonal, turns as the
        // sizes change), then, if a new value needs such a line to turn, without.
        let changed = distances
            .iter()
            .any(|&(_, _, k)| eqs[k].residual_at(&start).abs() > 1e-9);
        if changed {
            let mut kept = eqs.clone();
            for l in &self.lines {
                let at = |p: u32| DVec3::from_array(self.points[p as usize]);
                let Some(d) = (at(l.end) - at(l.start)).try_normalize() else {
                    continue;
                };
                if d.abs().max_element() < 1.0 - 1e-9 {
                    continue;
                }
                let s = [slots(l.start), slots(l.end)].concat();
                for c in 0..3 {
                    kept.push(Eq3::new(Kind::Direction(c, d.to_array()), s.clone()));
                }
            }
            let (out, vals) = run(&kept);
            if out.converged {
                return Ok(points(&vals));
            }
        }
        let (out, vals) = run(&eqs);
        if !out.converged {
            return Err(format!(
                "Its relations and dimensions can't all be met (they are {:.3} mm out at worst): check the dimensions' values.",
                out.max_residual
            ));
        }
        Ok(points(&vals))
    }

    /// The lines a structural member follows (not construction lines), solved: each with
    /// its index.
    pub fn path_lines(&self, points: &[DVec3]) -> Vec<(u32, DVec3, DVec3)> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.construction)
            .map(|(i, l)| (i as u32, points[l.start as usize], points[l.end as usize]))
            .collect()
    }
}

impl Default for Sketch3dFeature {
    fn default() -> Self {
        Self::new()
    }
}

fn points(vals: &[f64]) -> Vec<DVec3> {
    vals.chunks(3)
        .map(|c| DVec3::new(c[0], c[1], c[2]))
        .collect()
}

/// One equation's residual, by component `c` where it has several. Its slots are the
/// points it reads, three each, in the order its relation names them.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Coincident(usize),
    OnLine(usize),
    Midpoint(usize),
    Along(usize),
    Parallel(usize),
    Perpendicular,
    Equal,
    Fixed(usize, f64),
    /// A line's direction kept
    Direction(usize, [f64; 3]),
    PointPoint(f64),
    PointLine(f64),
}

#[derive(Clone)]
struct Eq3 {
    kind: Kind,
    slots: Vec<u32>,
}

impl Eq3 {
    fn new(kind: Kind, slots: Vec<u32>) -> Self {
        Self { kind, slots }
    }

    /// The residual with every point's coordinates `vals`.
    fn residual_at(&self, vals: &[f64]) -> f64 {
        let x: Vec<f64> = self.slots.iter().map(|s| vals[*s as usize]).collect();
        self.residual(&x)
    }

    /// The residual with the slots' values `x`, in their order.
    fn residual(&self, x: &[f64]) -> f64 {
        let p = |k: usize| DVec3::new(x[3 * k], x[3 * k + 1], x[3 * k + 2]);
        let unit = |d: DVec3| d / d.length().max(1e-12);
        match self.kind {
            Kind::Coincident(c) => p(0)[c] - p(1)[c],
            Kind::OnLine(c) => {
                let d = unit(p(2) - p(1));
                (p(0) - p(1)).cross(d)[c]
            }
            Kind::Midpoint(c) => p(0)[c] - 0.5 * (p(1)[c] + p(2)[c]),
            Kind::Along(c) => p(1)[c] - p(0)[c],
            Kind::Parallel(c) => {
                let (a, b) = (unit(p(1) - p(0)), unit(p(3) - p(2)));
                // Scaled to a length, as the other residuals are
                a.cross(b)[c] * (p(1) - p(0)).length().max(1.0)
            }
            Kind::Perpendicular => {
                let (a, b) = (unit(p(1) - p(0)), unit(p(3) - p(2)));
                a.dot(b) * (p(1) - p(0)).length().max(1.0)
            }
            Kind::Equal => (p(1) - p(0)).length() - (p(3) - p(2)).length(),
            Kind::Fixed(c, at) => p(0)[c] - at,
            Kind::Direction(c, d) => {
                unit(p(1) - p(0)).cross(DVec3::from_array(d))[c] * (p(1) - p(0)).length().max(1.0)
            }
            Kind::PointPoint(v) => (p(1) - p(0)).length() - v,
            Kind::PointLine(v) => {
                let d = unit(p(2) - p(1));
                (p(0) - p(1)).cross(d).length() - v
            }
        }
    }
}

impl Equation for Eq3 {
    fn slots(&self) -> &[u32] {
        &self.slots
    }

    fn eval(&self, vals: &[f64], grad: &mut [f64; MAX_SLOTS]) -> f64 {
        let mut x: Vec<f64> = self.slots.iter().map(|s| vals[*s as usize]).collect();
        let r = self.residual(&x);
        // Central differences: the residuals are smooth where they are met
        for k in 0..x.len() {
            let h = 1e-6 * x[k].abs().max(1.0);
            let was = x[k];
            x[k] = was + h;
            let up = self.residual(&x);
            x[k] = was - h;
            let down = self.residual(&x);
            x[k] = was;
            grad[k] = (up - down) / (2.0 * h);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Weldment 30x30x2's frame: a rectangle 970 by 370 about the origin in the plane
    /// y = 0, a second 170 below it, joined at the corners.
    fn frame() -> Sketch3dFeature {
        let mut s = Sketch3dFeature::new();
        let (w, d, h) = (485.0, 185.0, 170.0);
        for y in [0.0, -h] {
            for (x, z) in [(w, d), (-w, d), (-w, -d), (w, -d)] {
                s.points.push([x, y, z]);
            }
        }
        s.points.push([0.0, 0.0, 0.0]);
        let line = |a, b| Line3 {
            start: a,
            end: b,
            construction: false,
        };
        for k in 0..4 {
            s.lines.push(line(k, (k + 1) % 4));
            s.lines.push(line(4 + k, 4 + (k + 1) % 4));
            s.lines.push(line(k, 4 + k));
        }
        // The diagonals, through the origin
        s.lines.push(Line3 {
            start: 0,
            end: 2,
            construction: true,
        });
        s.relations.push(Relation3::Fixed(8));
        s.relations.push(Relation3::Midpoint(8, 12));
        for (k, axis) in [Axis3::X, Axis3::Z, Axis3::X, Axis3::Z]
            .into_iter()
            .enumerate()
        {
            s.relations.push(Relation3::Along(axis, 3 * k as u32));
            s.relations.push(Relation3::Along(axis, 3 * k as u32 + 1));
            s.relations
                .push(Relation3::Along(Axis3::Y, 3 * k as u32 + 2));
        }
        s.dimensions = vec![Scalar::new(370.0), Scalar::new(970.0), Scalar::new(170.0)];
        s.relations.push(Relation3::Distance {
            a: Ref3::Point(1),
            b: Ref3::Point(2),
            dimension: 0,
        });
        s.relations.push(Relation3::Distance {
            a: Ref3::Point(0),
            b: Ref3::Point(1),
            dimension: 1,
        });
        s.relations.push(Relation3::Distance {
            a: Ref3::Line(0),
            b: Ref3::Line(1),
            dimension: 2,
        });
        s
    }

    #[test]
    fn a_frame_is_where_it_is_drawn() {
        let s = frame();
        let p = s.solve(&[370.0, 970.0, 170.0]).unwrap();
        assert!(
            p[0].distance(DVec3::new(485.0, 0.0, 185.0)) < 1e-6,
            "{:?}",
            p[0]
        );
        assert!(p[6].distance(DVec3::new(-485.0, -170.0, -185.0)) < 1e-6);
    }

    #[test]
    fn its_dimensions_drive_it() {
        // Deeper, wider and taller: about the origin, which the diagonal's middle is held at
        let s = frame();
        let p = s.solve(&[400.0, 1000.0, 200.0]).unwrap();
        assert!(
            p[0].distance(DVec3::new(500.0, 0.0, 200.0)) < 1e-6,
            "{:?}",
            p[0]
        );
        assert!((p[4].y + 200.0).abs() < 1e-6, "{:?}", p[4]);
        assert!(
            p[6].distance(DVec3::new(-500.0, -200.0, -200.0)) < 1e-6,
            "{:?}",
            p[6]
        );
        assert_eq!(s.path_lines(&p).len(), 12);
    }

    #[test]
    fn its_height_moves_the_bottom_and_nothing_turns() {
        // Weldment 30x30x2's frame, all three sizes at once: the top stays in its plane
        let mut s = frame();
        // The top's long sides are only parallel and square to the short ones, free to turn
        // about the axis: as SolidWorks' export has them
        s.relations
            .retain(|r| !matches!(r, Relation3::Along(Axis3::X, 0)));
        let p = s.solve(&[500.0, 1200.0, 300.0]).unwrap();
        assert!(
            p[0].distance(DVec3::new(600.0, 0.0, 250.0)) < 1e-6,
            "{:?}",
            p[0]
        );
        assert!(
            p[1].distance(DVec3::new(-600.0, 0.0, 250.0)) < 1e-6,
            "{:?}",
            p[1]
        );
        assert!(
            p[6].distance(DVec3::new(-600.0, -300.0, -250.0)) < 1e-6,
            "{:?}",
            p[6]
        );
    }

    #[test]
    fn what_cant_be_met_says_so() {
        let mut s = frame();
        s.relations.push(Relation3::Equal(0, 2));
        assert!(s.solve(&[370.0, 970.0, 170.0]).is_err());
    }
}
