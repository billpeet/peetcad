//! Point-in-solid classification by ray casting.

use peet_math::tolerance::LINEAR;
use peet_math::{Aabb, DVec3};

use super::domain::{FaceGeom, Where};
use crate::geom::Surface;

/// A ray meeting a surface at less than this sine of the angle is grazing: the hit is
/// ill-conditioned, so another direction is tried.
const GRAZING: f64 = 1e-4;

/// Ray directions tried in turn. None is aligned with anything a part is likely to have.
const DIRECTIONS: [[f64; 3]; 6] = [
    [0.531_726_1, 0.277_350_9, 0.800_210_3],
    [-0.713_908_1, 0.412_337_7, 0.566_113_9],
    [0.193_649_2, -0.887_412_1, 0.418_330_7],
    [-0.344_124_3, -0.601_337_9, -0.721_110_3],
    [0.846_153_8, 0.119_402_9, -0.519_399_1],
    [0.068_279_3, 0.756_802_5, -0.650_066_3],
];

/// The faces of one input solid, prepared for queries.
pub(crate) struct Body {
    pub faces: Vec<FaceGeom>,
    pub bounds: Aabb,
}

impl Body {
    /// Whether `p` is inside the solid. `p` must not lie on its boundary.
    pub fn contains(&self, p: DVec3) -> bool {
        if !self.bounds.contains(p) {
            return false;
        }
        let mut votes = 0i32;
        for d in DIRECTIONS {
            let dir = DVec3::from_array(d).normalize();
            match self.crossings(p, dir) {
                Some(n) => return n % 2 == 1,
                None => continue,
            }
        }
        // Every direction was degenerate: fall back to a vote that ignores the doubts.
        for d in DIRECTIONS {
            let dir = DVec3::from_array(d).normalize();
            votes += if self.crossings_lenient(p, dir) % 2 == 1 {
                1
            } else {
                -1
            };
        }
        votes > 0
    }

    /// Whether `p` lies on the solid's boundary (within tolerance).
    pub fn touches(&self, p: DVec3) -> bool {
        self.bounds.contains(p)
            && self.faces.iter().any(|f| {
                f.bounds.contains(p)
                    && f.surface.signed_distance(p).abs() <= LINEAR
                    && f.locate(p) != Where::Outside
            })
    }

    /// Number of times the ray from `p` crosses the boundary, or `None` if a hit was
    /// degenerate (grazing, or on a face's boundary).
    fn crossings(&self, p: DVec3, dir: DVec3) -> Option<u32> {
        let mut count = 0;
        for f in &self.faces {
            if !ray_hits_box(p, dir, &f.bounds) {
                continue;
            }
            for hit in surface_hits(&f.surface, p, dir) {
                let t = hit?;
                if t <= LINEAR {
                    continue;
                }
                match f.locate(p + dir * t) {
                    Where::Inside => count += 1,
                    Where::Boundary => return None,
                    Where::Outside => {}
                }
            }
        }
        Some(count)
    }

    fn crossings_lenient(&self, p: DVec3, dir: DVec3) -> u32 {
        let mut count = 0;
        for f in &self.faces {
            for t in surface_hits(&f.surface, p, dir).into_iter().flatten() {
                if t > LINEAR && f.locate(p + dir * t) == Where::Inside {
                    count += 1;
                }
            }
        }
        count
    }
}

/// Ray parameters where the ray meets the surface; `None` entries are degenerate hits.
fn surface_hits(surface: &Surface, p: DVec3, dir: DVec3) -> Vec<Option<f64>> {
    match surface {
        Surface::Plane(pl) => {
            let denom = dir.dot(pl.normal());
            let dist = pl.signed_distance(p);
            if denom.abs() < GRAZING {
                // Parallel: no hit, unless the ray runs within the plane.
                return if dist.abs() <= LINEAR {
                    vec![None]
                } else if dist * denom < 0.0 {
                    // Slowly approaching: a real but ill-conditioned hit.
                    vec![None]
                } else {
                    Vec::new()
                };
            }
            vec![Some(-dist / denom)]
        }
        Surface::Cylinder(c) => {
            let o = c.frame.to_local(p);
            let d = c.frame.vector_to_local(dir);
            let a = d.x * d.x + d.y * d.y;
            if a < GRAZING * GRAZING {
                // Along the axis: never crosses, unless it runs along the wall.
                let off = o.x.hypot(o.y) - c.radius;
                return if off.abs() <= LINEAR {
                    vec![None]
                } else {
                    Vec::new()
                };
            }
            let b = o.x * d.x + o.y * d.y;
            // Closest approach of the ray's line to the axis.
            let tc = -b / a;
            let h = (o.x + d.x * tc).hypot(o.y + d.y * tc);
            if h > c.radius + LINEAR {
                return Vec::new();
            }
            let reach2 = (c.radius * c.radius - h * h) / a;
            // Tangent, or meeting the wall at a shallow angle.
            if h >= c.radius * (1.0 - GRAZING * GRAZING) || reach2 <= 0.0 {
                return if tc + reach2.max(0.0).sqrt() > LINEAR {
                    vec![None]
                } else {
                    Vec::new()
                };
            }
            let w = reach2.sqrt();
            vec![Some(tc - w), Some(tc + w)]
        }
        Surface::Sphere(s) => {
            let o = p - s.frame.origin;
            let tc = -o.dot(dir);
            let h = (o + dir * tc).length();
            if h > s.radius + LINEAR {
                return Vec::new();
            }
            let reach2 = s.radius * s.radius - h * h;
            if h >= s.radius * (1.0 - GRAZING * GRAZING) || reach2 <= 0.0 {
                return if tc + reach2.max(0.0).sqrt() > LINEAR {
                    vec![None]
                } else {
                    Vec::new()
                };
            }
            let w = reach2.sqrt();
            vec![Some(tc - w), Some(tc + w)]
        }
        Surface::Cone(c) => {
            // The quadric x² + y² = k² z² about the apex; only the nappe with a
            // non-negative radius is the surface.
            let k = c.slope();
            let apex = DVec3::new(0.0, 0.0, c.apex_v());
            let o = c.frame.to_local(p) - apex;
            let d = c.frame.vector_to_local(dir);
            let qa = d.x * d.x + d.y * d.y - k * k * d.z * d.z;
            let qb = o.x * d.x + o.y * d.y - k * k * o.z * d.z;
            let qc = o.x * o.x + o.y * o.y - k * k * o.z * o.z;
            // Along a ruling's direction the ray meets the cone once, far away.
            if qa.abs() < GRAZING * GRAZING {
                return vec![None];
            }
            let disc = qb * qb - qa * qc;
            let scale = qb * qb + (qa * qc).abs();
            if disc < -GRAZING * GRAZING * scale {
                return Vec::new();
            }
            let on_nappe = |t: f64| (o.z + d.z * t) * k >= 0.0;
            if disc <= GRAZING * GRAZING * scale {
                let t = -qb / qa;
                return if t > LINEAR && on_nappe(t) {
                    vec![None]
                } else {
                    Vec::new()
                };
            }
            let w = disc.sqrt();
            [(-qb - w) / qa, (-qb + w) / qa]
                .into_iter()
                .filter(|&t| on_nappe(t))
                .map(|t| {
                    let shallow = surface.normal_at(p + dir * t).dot(dir).abs() < GRAZING;
                    (!shallow).then_some(t)
                })
                .collect()
        }
        Surface::Torus(t) => torus_hits(surface, t, p, dir),
        Surface::Nurbs(s) => nurbs_hits(s, p, dir),
    }
}

/// Hits of a ray on a freeform surface: Newton's method on `S(u, v) = p + t·dir`, started
/// from every grid point of the surface that is close to the ray for its cell's size.
fn nurbs_hits(s: &crate::nurbs::NurbsSurface, p: DVec3, dir: DVec3) -> Vec<Option<f64>> {
    use peet_math::{DMat3, DVec2};
    let (lo, hi) = s.domain();
    let (samples, cell) = s.samples();
    let stretch = s.stretch();
    // A grid point can start a search if the ray passes within about a cell of it.
    let reach = 1.5 * (stretch * cell).length();
    let mut hits: Vec<(f64, bool)> = Vec::new();
    {
        for &(start, q) in samples {
            let along = (q - p).dot(dir);
            if (q - p - dir * along).length() > reach {
                continue;
            }
            let (mut uv, mut t) = (start, along);
            let mut converged = false;
            for _ in 0..24 {
                let [at, su, sv, ..] = s.evaluate(uv);
                let f = at - p - dir * t;
                if f.length() <= 1e-11 {
                    converged = true;
                    break;
                }
                let jacobian = DMat3::from_cols(su, sv, -dir);
                if jacobian.determinant().abs() <= 1e-14 * su.length() * sv.length() {
                    break;
                }
                let step = jacobian.inverse() * f;
                uv -= DVec2::new(step.x, step.y);
                t -= step.z;
                // A hit outside the surface's rectangle is not on the surface.
                if uv.cmplt(lo - cell).any() || uv.cmpgt(hi + cell).any() {
                    break;
                }
            }
            if !converged || uv.cmplt(lo).any() || uv.cmpgt(hi).any() {
                continue;
            }
            if hits.iter().any(|(known, _)| (known - t).abs() <= LINEAR) {
                continue;
            }
            let shallow = s.normal(uv).dot(dir).abs() < GRAZING;
            hits.push((t, shallow));
        }
    }
    hits.into_iter()
        .map(|(t, shallow)| (!shallow).then_some(t))
        .collect()
}

/// Samples along the chord of a torus's bounding sphere.
const TORUS_SAMPLES: usize = 64;

/// Hits of a ray on a torus (a quartic), found numerically: the signed distance along the
/// ray is sampled across the torus's bounding sphere, sign changes are bisected, and
/// every dip towards the surface between samples is followed to its lowest point, so a
/// ray that only just enters the tube is not missed. A dip that touches the surface
/// within tolerance, or a crossing at a shallow angle, is a degenerate hit.
fn torus_hits(
    surface: &Surface,
    torus: &crate::geom::Torus,
    p: DVec3,
    dir: DVec3,
) -> Vec<Option<f64>> {
    let o = p - torus.frame.origin;
    let tc = -o.dot(dir);
    let reach = torus.major + torus.minor + LINEAR;
    let h2 = (o + dir * tc).length_squared();
    if h2 > reach * reach {
        return Vec::new();
    }
    let w = (reach * reach - h2).sqrt();
    let (lo, hi) = (tc - w, tc + w);
    let f = |t: f64| surface.signed_distance(p + dir * t);
    let n = TORUS_SAMPLES;
    let step = (hi - lo) / n as f64;
    let at = |i: usize| lo + step * i as f64;
    let values: Vec<f64> = (0..=n).map(|i| f(at(i))).collect();
    let bisect = |mut x0: f64, mut x1: f64| {
        let f0 = f(x0);
        for _ in 0..70 {
            let m = 0.5 * (x0 + x1);
            if (f(m) < 0.0) == (f0 < 0.0) {
                x0 = m;
            } else {
                x1 = m;
            }
        }
        0.5 * (x0 + x1)
    };
    let mut hits = Vec::new();
    let crossing = |t: f64, hits: &mut Vec<Option<f64>>| {
        let shallow = surface.normal_at(p + dir * t).dot(dir).abs() < GRAZING;
        hits.push((!shallow).then_some(t));
    };
    for i in 0..n {
        let (a, b) = (values[i], values[i + 1]);
        if a * b < 0.0 || (a == 0.0 && b != 0.0) {
            crossing(bisect(at(i), at(i + 1)), &mut hits);
            continue;
        }
        if i == 0 {
            continue;
        }
        // The same sign on both sides: look for a dip (or a bump) towards the surface.
        let prev = values[i - 1];
        if prev * a < 0.0 || !(a.abs() <= prev.abs() && a.abs() <= b.abs()) {
            continue;
        }
        let (mut x0, mut x1) = (at(i) - step, at(i) + step);
        const GOLD: f64 = 0.618_033_988_749_894_8;
        let toward = |t: f64| f(t) * a.signum();
        for _ in 0..60 {
            let (m0, m1) = (x1 - GOLD * (x1 - x0), x0 + GOLD * (x1 - x0));
            if toward(m0) < toward(m1) {
                x1 = m1;
            } else {
                x0 = m0;
            }
        }
        let t = 0.5 * (x0 + x1);
        let lowest = toward(t);
        if lowest.abs() <= LINEAR {
            hits.push(None);
        } else if lowest < 0.0 {
            // The ray dips through the surface and comes back between two samples.
            crossing(bisect(at(i) - step, t), &mut hits);
            crossing(bisect(t, at(i) + step), &mut hits);
        }
    }
    hits
}

fn ray_hits_box(p: DVec3, dir: DVec3, b: &Aabb) -> bool {
    let (mut lo, mut hi) = (0.0_f64, f64::INFINITY);
    for k in 0..3 {
        let (o, d, min, max) = (p[k], dir[k], b.min[k], b.max[k]);
        if d.abs() < 1e-300 {
            if o < min || o > max {
                return false;
            }
        } else {
            let (t0, t1) = ((min - o) / d, (max - o) / d);
            lo = lo.max(t0.min(t1));
            hi = hi.min(t0.max(t1));
            if lo > hi {
                return false;
            }
        }
    }
    true
}
