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
    }
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
