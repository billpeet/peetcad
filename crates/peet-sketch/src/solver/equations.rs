//! Residual equations: evaluation with analytic gradients, and generation from constraints.
//!
//! Every equation reads its inputs from a flat value array through *slots*: a point uses
//! two consecutive slots (x, y), a circle radius one slot. Slots below the number of
//! variables are unknowns, the rest are constants (locked points). Coincident points share
//! slots (aliasing), so an equation may list the same slot twice; gradients are summed
//! when the Jacobian is assembled.
//!
//! An arc's radius is implicit, `|start - centre|` (as in the sketch model), and each arc
//! adds one internal equation `|end - centre| = |start - centre|`. Equations that involve
//! radii carry up to two *radius terms*, each either a circle's radius slot or an arc's
//! `|start - centre|`.
//!
//! **Formulation.** Length-like residuals are in mm, angular ones are dimensionless (a
//! sine or an angle in radians), so every Jacobian row has entries of order one:
//!
//! | constraint | residual |
//! |---|---|
//! | coincident, concentric, fix, midpoint | coordinate differences (aliased where possible) |
//! | horizontal / vertical | `b.y - a.y` / `b.x - a.x` |
//! | point on line | signed distance to the infinite line |
//! | point on circle/arc | `|p - c| - r` |
//! | parallel / perpendicular | `sin` / `cos` of the angle between the directions |
//! | angle | signed angle minus target (wrapped), sign kept from the current geometry |
//! | tangent line–circle | signed distance of the centre from the line `∓ r` (side kept) |
//! | tangent circle–circle | `|c1 - c2| - (r1 + r2)` or `- |r1 - r2|`, whichever is closer |
//! | tangent with a known contact point | `(p - c) · line direction`, or `p` on the centre line |
//! | symmetric | midpoint on the axis, and `(b - a) · axis direction` |
//! | equal | lengths or radii |
//! | distance, length, radius, … | measured value minus target, signed where needed |

use std::f64::consts::{PI, TAU};

use peet_math::DVec2;

use crate::sketch::{ConstraintKind, EntityId, Geometry, Sketch};

use peet_solve::{Equation, MAX_SLOTS, NONE};

/// Below this length (mm) a direction is considered undefined and a fixed fallback is used.
const TINY: f64 = 1e-12;

/// The core of an equation (before radius terms and the constant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EqKind {
    /// Zero (only radius terms and the constant).
    Zero,
    /// `Σ c[k] v[s_k]` over the core slots (up to four).
    Linear,
    /// `|q - p|`: p = slots 0-1, q = slots 2-3.
    Dist,
    /// Signed distance of p (0-1) from the line a (2-3) → b (4-5), positive on the left.
    LineDist,
    /// Signed distance of the midpoint of p (0-1) and q (2-3) from the line a (4-5) → b (6-7).
    SymMid,
    /// `dot(unit(b - a), q - p)` with the same slots as `SymMid`.
    SymPerp,
    /// `sin(θ2 - θ1)` for the lines a1 → b1 (0-3) and a2 → b2 (4-7).
    Parallel,
    /// `cos(θ2 - θ1)`.
    Perpendicular,
    /// `θ2 - θ1 - k0`, wrapped to (-π, π] (the constant is inside the wrap).
    Angle,
    /// `|b1 - a1| - |b2 - a2|`.
    EqualLength,
}

/// Who an equation belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A driving constraint (index = `ConstraintId.0`).
    Constraint(u32),
    /// The implicit "end on the same circle as start" equation of an arc.
    Arc(u32),
    /// A soft drag target.
    Soft,
}

/// A radius: a circle's radius slot, or an arc's implicit `|start - centre|`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Radius {
    Slot(u32),
    Arc { center: u32, start: u32 },
}

/// A radius term `coeff · R` subtracted from the residual; its slots start at `at`.
#[derive(Clone, Copy, Debug, Default)]
struct RadTerm {
    arc: bool,
    at: u8,
    coeff: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Eq {
    pub kind: EqKind,
    /// Total slots used.
    pub n: u8,
    /// Core slots (radius term slots follow).
    nc: u8,
    /// Radius terms used.
    nr: u8,
    pub slots: [u32; MAX_SLOTS],
    /// Linear coefficients (core slots).
    c: [f64; 4],
    rad: [RadTerm; 2],
    pub k0: f64,
    pub owner: Owner,
}

#[inline]
fn pt(v: &[f64], s: u32) -> DVec2 {
    DVec2::new(v[s as usize], v[s as usize + 1])
}

#[inline]
fn put(g: &mut [f64; MAX_SLOTS], i: usize, d: DVec2) {
    g[i] = d.x;
    g[i + 1] = d.y;
}

impl Eq {
    fn new(kind: EqKind, slots: &[u32], owner: Owner) -> Self {
        let mut s = [NONE; MAX_SLOTS];
        s[..slots.len()].copy_from_slice(slots);
        Self {
            kind,
            n: slots.len() as u8,
            nc: slots.len() as u8,
            nr: 0,
            slots: s,
            c: [0.0; 4],
            rad: [RadTerm::default(); 2],
            k0: 0.0,
            owner,
        }
    }

    /// A linear equation `Σ coeffs[k] v[slots[k]] - k0`.
    pub fn linear(terms: &[(u32, f64)], k0: f64, owner: Owner) -> Self {
        let slots: Vec<u32> = terms.iter().map(|t| t.0).collect();
        let mut e = Self::new(EqKind::Linear, &slots, owner);
        for (k, t) in terms.iter().enumerate() {
            e.c[k] = t.1;
        }
        e.k0 = k0;
        e
    }

    /// Subtracts `coeff · r` from the residual.
    fn minus_radius(mut self, r: Radius, coeff: f64) -> Self {
        let at = self.n as usize;
        let arc = match r {
            Radius::Slot(s) => {
                self.slots[at] = s;
                self.n += 1;
                false
            }
            Radius::Arc { center, start } => {
                self.slots[at..at + 4].copy_from_slice(&[center, center + 1, start, start + 1]);
                self.n += 4;
                true
            }
        };
        self.rad[self.nr as usize] = RadTerm {
            arc,
            at: at as u8,
            coeff,
        };
        self.nr += 1;
        self
    }

    fn with_k0(mut self, k0: f64) -> Self {
        self.k0 = k0;
        self
    }
}

impl Equation for Eq {
    fn slots(&self) -> &[u32] {
        &self.slots[..self.n as usize]
    }

    /// Residual at `v`, writing the gradient with respect to each slot into `g[..n]`.
    fn eval(&self, v: &[f64], g: &mut [f64; MAX_SLOTS]) -> f64 {
        let s = &self.slots;
        let mut r = match self.kind {
            EqKind::Zero => 0.0,
            EqKind::Linear => {
                let mut r = 0.0;
                for k in 0..self.nc as usize {
                    r += self.c[k] * v[s[k] as usize];
                    g[k] = self.c[k];
                }
                r
            }
            EqKind::Dist => {
                let d = pt(v, s[2]) - pt(v, s[0]);
                let len = d.length();
                let u = if len > TINY { d / len } else { DVec2::X };
                put(g, 0, -u);
                put(g, 2, u);
                len
            }
            EqKind::LineDist | EqKind::SymMid => {
                let mid = self.kind == EqKind::SymMid;
                let (p, a, b) = if mid {
                    ((pt(v, s[0]) + pt(v, s[2])) * 0.5, pt(v, s[4]), pt(v, s[6]))
                } else {
                    (pt(v, s[0]), pt(v, s[2]), pt(v, s[4]))
                };
                let d = b - a;
                let len = d.length().max(TINY);
                let w = p - a;
                let dist = d.perp_dot(w) / len;
                let n = d.perp() / len;
                let gd = (-w.perp() - d * (dist / len)) / len;
                if mid {
                    put(g, 0, n * 0.5);
                    put(g, 2, n * 0.5);
                    put(g, 4, -n - gd);
                    put(g, 6, gd);
                } else {
                    put(g, 0, n);
                    put(g, 2, -n - gd);
                    put(g, 4, gd);
                }
                dist
            }
            EqKind::SymPerp => {
                let q_p = pt(v, s[2]) - pt(v, s[0]);
                let d = pt(v, s[6]) - pt(v, s[4]);
                let len = d.length();
                let (u, len) = if len > TINY {
                    (d / len, len)
                } else {
                    (DVec2::X, TINY)
                };
                let t = u.dot(q_p);
                let gd = (q_p - u * t) / len;
                put(g, 0, -u);
                put(g, 2, u);
                put(g, 4, -gd);
                put(g, 6, gd);
                t
            }
            EqKind::Parallel | EqKind::Perpendicular | EqKind::Angle => {
                let d1 = pt(v, s[2]) - pt(v, s[0]);
                let d2 = pt(v, s[6]) - pt(v, s[4]);
                let l1 = d1.length_squared().max(TINY * TINY);
                let l2 = d2.length_squared().max(TINY * TINY);
                // dθ/dd1 and dθ/dd2 for θ = θ2 - θ1.
                let t1 = -d1.perp() / l1;
                let t2 = d2.perp() / l2;
                let theta = d1.perp_dot(d2).atan2(d1.dot(d2));
                let (r, scale) = match self.kind {
                    EqKind::Parallel => (theta.sin(), theta.cos()),
                    EqKind::Perpendicular => (theta.cos(), -theta.sin()),
                    // The constant goes inside the wrap; cancel the `- k0` below.
                    _ => (wrap_angle(theta - self.k0) + self.k0, 1.0),
                };
                put(g, 0, -t1 * scale);
                put(g, 2, t1 * scale);
                put(g, 4, -t2 * scale);
                put(g, 6, t2 * scale);
                r
            }
            EqKind::EqualLength => {
                let d1 = pt(v, s[2]) - pt(v, s[0]);
                let d2 = pt(v, s[6]) - pt(v, s[4]);
                let (n1, n2) = (d1.length(), d2.length());
                let u1 = if n1 > TINY { d1 / n1 } else { DVec2::X };
                let u2 = if n2 > TINY { d2 / n2 } else { DVec2::X };
                put(g, 0, -u1);
                put(g, 2, u1);
                put(g, 4, u2);
                put(g, 6, -u2);
                n1 - n2
            }
        };
        r -= self.k0;
        for t in &self.rad[..self.nr as usize] {
            let at = t.at as usize;
            if t.arc {
                // R = |start - centre|
                let d = pt(v, s[at + 2]) - pt(v, s[at]);
                let len = d.length();
                let u = if len > TINY { d / len } else { DVec2::X };
                r -= t.coeff * len;
                put(g, at, u * t.coeff);
                put(g, at + 2, -u * t.coeff);
            } else {
                r -= t.coeff * v[s[at] as usize];
                g[at] = -t.coeff;
            }
        }
        r
    }
}

/// Wraps an angle to (-π, π].
pub(crate) fn wrap_angle(a: f64) -> f64 {
    let r = (a + PI).rem_euclid(TAU) - PI;
    if r <= -PI { r + TAU } else { r }
}

/// Deterministic sign: +1 for zero.
#[inline]
fn sign(x: f64) -> f64 {
    if x < 0.0 { -1.0 } else { 1.0 }
}

/// Where each entity's values live.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlotMap {
    /// x slot of each point entity (y is the next slot); `NONE` for other entities.
    pub point: Vec<u32>,
    /// Radius slot of each circle; `NONE` otherwise (arc radii are implicit).
    pub radius: Vec<u32>,
    /// Per constraint id: it is valid and driving, and not absorbed by aliasing.
    pub emit: Vec<bool>,
    /// Per tangent constraint id: the x slot of a point known to lie on both curves (a
    /// shared endpoint, or a point related to both by coincidence or point-on-curve), or
    /// `NONE`. With a contact point, tangency is written as "radius ⟂ line" (line–circle)
    /// or "contact collinear with the centres" (circle–circle). The distance form would be
    /// satisfied only to second order along the curve there, making the Jacobian rank
    /// deficient and the analysis report false redundancy.
    pub contact: Vec<u32>,
}

impl SlotMap {
    #[inline]
    fn p(&self, id: EntityId) -> u32 {
        self.point[id.0 as usize]
    }
}

/// Generates the hard equations: the driving constraints in id order, then the implicit
/// arc equations (last, so the redundancy analysis treats them as the dependent ones when
/// user constraints already imply them). Sign and branch choices come from `vals`.
pub(crate) fn generate(sketch: &Sketch, map: &SlotMap, vals: &[f64], out: &mut Vec<Eq>) {
    out.clear();
    for (cid, con) in sketch.constraints() {
        if !map.emit.get(cid.0 as usize).copied().unwrap_or(false) {
            continue;
        }
        let value = con.dimension.as_ref().map_or(0.0, |d| d.value);
        let contact = map.contact.get(cid.0 as usize).copied().unwrap_or(NONE);
        let ctx = Ctx {
            sketch,
            map,
            vals,
            owner: Owner::Constraint(cid.0),
        };
        ctx.emit(&con.kind, value, contact, out);
    }
    for (id, e) in sketch.entities() {
        if let Geometry::Arc { center, start, end } = e.geometry {
            let (c, s, en) = (map.p(center), map.p(start), map.p(end));
            if [c, s, en].contains(&NONE) {
                continue;
            }
            // |end - centre| - |start - centre|
            out.push(
                Eq::new(EqKind::Dist, &[c, c + 1, en, en + 1], Owner::Arc(id.0)).minus_radius(
                    Radius::Arc {
                        center: c,
                        start: s,
                    },
                    1.0,
                ),
            );
        }
    }
}

struct Ctx<'a> {
    sketch: &'a Sketch,
    map: &'a SlotMap,
    vals: &'a [f64],
    owner: Owner,
}

impl Ctx<'_> {
    fn geometry(&self, id: EntityId) -> &Geometry {
        &self.sketch.entity(id).expect("validated entity").geometry
    }

    fn is_line(&self, id: EntityId) -> bool {
        matches!(self.geometry(id), Geometry::Line { .. })
    }

    fn is_point(&self, id: EntityId) -> bool {
        matches!(self.geometry(id), Geometry::Point { .. })
    }

    fn line_pts(&self, id: EntityId) -> (u32, u32) {
        match *self.geometry(id) {
            Geometry::Line { start, end } => (self.map.p(start), self.map.p(end)),
            _ => unreachable!("validated as a line"),
        }
    }

    fn center(&self, id: EntityId) -> u32 {
        self.map
            .p(self.sketch.center(id).expect("validated as circular"))
    }

    fn radius(&self, id: EntityId) -> Radius {
        match *self.geometry(id) {
            Geometry::Arc { center, start, .. } => Radius::Arc {
                center: self.map.p(center),
                start: self.map.p(start),
            },
            _ => Radius::Slot(self.map.radius[id.0 as usize]),
        }
    }

    fn radius_value(&self, id: EntityId) -> f64 {
        match self.radius(id) {
            Radius::Slot(s) => self.vals[s as usize],
            Radius::Arc { center, start } => pt(self.vals, start).distance(pt(self.vals, center)),
        }
    }

    fn pos(&self, slot: u32) -> DVec2 {
        pt(self.vals, slot)
    }

    /// Current signed distance of the point at `p` from the line `l`.
    fn signed_dist(&self, p: u32, l: EntityId) -> f64 {
        let (a, b) = self.line_pts(l);
        let (a, b) = (self.pos(a), self.pos(b));
        let d = b - a;
        d.perp_dot(self.pos(p) - a) / d.length().max(TINY)
    }

    fn line_dist(&self, p: u32, l: EntityId) -> Eq {
        let (a, b) = self.line_pts(l);
        Eq::new(
            EqKind::LineDist,
            &[p, p + 1, a, a + 1, b, b + 1],
            self.owner,
        )
    }

    fn four(&self, kind: EqKind, l1: EntityId, l2: EntityId) -> Eq {
        let (a1, b1) = self.line_pts(l1);
        let (a2, b2) = self.line_pts(l2);
        Eq::new(
            kind,
            &[a1, a1 + 1, b1, b1 + 1, a2, a2 + 1, b2, b2 + 1],
            self.owner,
        )
    }

    fn coincident(&self, a: u32, b: u32, out: &mut Vec<Eq>) {
        for k in 0..2 {
            out.push(Eq::linear(&[(b + k, 1.0), (a + k, -1.0)], 0.0, self.owner));
        }
    }

    fn emit(&self, kind: &ConstraintKind, value: f64, contact: u32, out: &mut Vec<Eq>) {
        use ConstraintKind as K;
        let owner = self.owner;
        let p = |id: EntityId| self.map.p(id);
        match *kind {
            K::Coincident(a, b) => self.coincident(p(a), p(b), out),
            K::Concentric(a, b) => self.coincident(self.center(a), self.center(b), out),
            K::PointOnCurve { point, curve } => {
                if self.is_line(curve) {
                    out.push(self.line_dist(p(point), curve));
                } else {
                    let (c, q) = (self.center(curve), p(point));
                    out.push(
                        Eq::new(EqKind::Dist, &[c, c + 1, q, q + 1], owner)
                            .minus_radius(self.radius(curve), 1.0),
                    );
                }
            }
            K::Horizontal(l) => {
                let (a, b) = self.line_pts(l);
                out.push(Eq::linear(&[(b + 1, 1.0), (a + 1, -1.0)], 0.0, owner));
            }
            K::Vertical(l) => {
                let (a, b) = self.line_pts(l);
                out.push(Eq::linear(&[(b, 1.0), (a, -1.0)], 0.0, owner));
            }
            K::HorizontalPoints(a, b) => {
                out.push(Eq::linear(&[(p(b) + 1, 1.0), (p(a) + 1, -1.0)], 0.0, owner))
            }
            K::VerticalPoints(a, b) => {
                out.push(Eq::linear(&[(p(b), 1.0), (p(a), -1.0)], 0.0, owner))
            }
            K::Parallel(l1, l2) => out.push(self.four(EqKind::Parallel, l1, l2)),
            K::Perpendicular(l1, l2) => out.push(self.four(EqKind::Perpendicular, l1, l2)),
            K::Tangent(a, b) => {
                let line = if self.is_line(a) {
                    Some((a, b))
                } else if self.is_line(b) {
                    Some((b, a))
                } else {
                    None
                };
                match (line, contact != NONE) {
                    (Some((l, circ)), true) => {
                        // (contact - centre) ⟂ line direction.
                        let (la, lb) = self.line_pts(l);
                        let c = self.center(circ);
                        out.push(Eq::new(
                            EqKind::SymPerp,
                            &[c, c + 1, contact, contact + 1, la, la + 1, lb, lb + 1],
                            owner,
                        ));
                    }
                    (Some((l, circ)), false) => {
                        // Signed distance of the centre from the line = ±r, side kept.
                        let c = self.center(circ);
                        let side = sign(self.signed_dist(c, l));
                        out.push(self.line_dist(c, l).minus_radius(self.radius(circ), side));
                    }
                    (None, true) => {
                        // Contact on the line through both centres.
                        let (c1, c2) = (self.center(a), self.center(b));
                        out.push(Eq::new(
                            EqKind::LineDist,
                            &[contact, contact + 1, c1, c1 + 1, c2, c2 + 1],
                            owner,
                        ));
                    }
                    (None, false) => {
                        let (c1, c2) = (self.center(a), self.center(b));
                        let (r1, r2) = (self.radius_value(a), self.radius_value(b));
                        let d = self.pos(c1).distance(self.pos(c2));
                        let external = (d - (r1 + r2)).abs() <= (d - (r1 - r2).abs()).abs();
                        let (k1, k2) = if external {
                            (1.0, 1.0)
                        } else {
                            let t = sign(r1 - r2);
                            (t, -t)
                        };
                        out.push(
                            Eq::new(EqKind::Dist, &[c1, c1 + 1, c2, c2 + 1], owner)
                                .minus_radius(self.radius(a), k1)
                                .minus_radius(self.radius(b), k2),
                        );
                    }
                }
            }
            K::Equal(a, b) => {
                if self.is_line(a) {
                    out.push(self.four(EqKind::EqualLength, a, b));
                } else {
                    out.push(
                        Eq::new(EqKind::Zero, &[], owner)
                            .minus_radius(self.radius(a), -1.0)
                            .minus_radius(self.radius(b), 1.0),
                    );
                }
            }
            K::Midpoint { point, line } => {
                let (a, b) = self.line_pts(line);
                let q = p(point);
                for k in 0..2 {
                    out.push(Eq::linear(
                        &[(q + k, 1.0), (a + k, -0.5), (b + k, -0.5)],
                        0.0,
                        owner,
                    ));
                }
            }
            K::Symmetric { a, b, axis } => {
                let (la, lb) = self.line_pts(axis);
                let (pa, pb) = (p(a), p(b));
                let slots = [pa, pa + 1, pb, pb + 1, la, la + 1, lb, lb + 1];
                out.push(Eq::new(EqKind::SymMid, &slots, owner));
                out.push(Eq::new(EqKind::SymPerp, &slots, owner));
            }
            K::Fix { point, at } => {
                let q = p(point);
                out.push(Eq::linear(&[(q, 1.0)], at.x, owner));
                out.push(Eq::linear(&[(q + 1, 1.0)], at.y, owner));
            }
            K::Distance(a, b) => {
                if self.is_point(a) && self.is_point(b) {
                    let (pa, pb) = (p(a), p(b));
                    out.push(
                        Eq::new(EqKind::Dist, &[pa, pa + 1, pb, pb + 1], owner).with_k0(value),
                    );
                } else {
                    let (q, l) = if self.is_point(a) { (a, b) } else { (b, a) };
                    let side = sign(self.signed_dist(p(q), l));
                    out.push(self.line_dist(p(q), l).with_k0(side * value));
                }
            }
            K::Length(l) => {
                let (a, b) = self.line_pts(l);
                out.push(Eq::new(EqKind::Dist, &[a, a + 1, b, b + 1], owner).with_k0(value));
            }
            K::HorizontalDistance(a, b) | K::VerticalDistance(a, b) => {
                let k = u32::from(matches!(kind, K::VerticalDistance(..)));
                let (pa, pb) = (p(a) + k, p(b) + k);
                let t = sign(self.vals[pb as usize] - self.vals[pa as usize]);
                out.push(Eq::linear(&[(pb, t), (pa, -t)], value, owner));
            }
            K::Radius(c) => out.push(
                Eq::new(EqKind::Zero, &[], owner)
                    .minus_radius(self.radius(c), -1.0)
                    .with_k0(value),
            ),
            K::Diameter(c) => out.push(
                Eq::new(EqKind::Zero, &[], owner)
                    .minus_radius(self.radius(c), -1.0)
                    .with_k0(value * 0.5),
            ),
            K::Angle(l1, l2) => {
                let (a1, b1) = self.line_pts(l1);
                let (a2, b2) = self.line_pts(l2);
                let d1 = self.pos(b1) - self.pos(a1);
                let d2 = self.pos(b2) - self.pos(a2);
                let current = d1.perp_dot(d2).atan2(d1.dot(d2));
                out.push(
                    self.four(EqKind::Angle, l1, l2)
                        .with_k0(sign(current) * value.to_radians()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_gradient(eq: &Eq, v: &[f64]) {
        let mut g = [0.0; MAX_SLOTS];
        eq.eval(v, &mut g);
        // Sum the analytic gradient per distinct slot (slots may repeat).
        for k in 0..eq.n as usize {
            let s = eq.slots[k] as usize;
            let analytic: f64 = (0..eq.n as usize)
                .filter(|&j| eq.slots[j] as usize == s)
                .map(|j| g[j])
                .sum();
            let h = 1e-6;
            let mut vp = v.to_vec();
            vp[s] += h;
            let mut vm = v.to_vec();
            vm[s] -= h;
            let numeric = (eq.residual(&vp) - eq.residual(&vm)) / (2.0 * h);
            assert!(
                (numeric - analytic).abs() < 1e-6,
                "{:?} slot {s}: numeric {numeric} analytic {analytic}",
                eq.kind
            );
        }
    }

    #[test]
    fn gradients_match_finite_differences() {
        let v = [
            0.3, -1.2, 4.1, 0.7, -2.5, 3.3, 1.9, -0.4, 2.2, 5.1, 0.8, 1.7,
        ];
        let o = Owner::Soft;
        let all8 = [0, 1, 2, 3, 4, 5, 6, 7];
        let arc = Radius::Arc {
            center: 8,
            start: 4,
        };
        // Keep the angle residuals away from their ±π wrap.
        let (d1, d2) = (DVec2::new(3.8, 1.9), DVec2::new(4.4, -3.7));
        let theta = d1.perp_dot(d2).atan2(d1.dot(d2));
        let eqs = vec![
            Eq::linear(&[(0, 1.0), (3, -0.5), (5, 2.0)], 1.0, o),
            Eq::new(EqKind::Dist, &[0, 1, 2, 3], o),
            Eq::new(EqKind::LineDist, &[0, 1, 2, 3, 4, 5], o),
            Eq::new(EqKind::LineDist, &[0, 1, 2, 3, 4, 5], o).minus_radius(Radius::Slot(10), -1.0),
            Eq::new(EqKind::LineDist, &[8, 9, 2, 3, 4, 5], o).minus_radius(arc, 1.0),
            Eq::new(EqKind::SymMid, &all8, o),
            Eq::new(EqKind::SymPerp, &all8, o),
            Eq::new(EqKind::Parallel, &all8, o),
            Eq::new(EqKind::Perpendicular, &all8, o),
            Eq::new(EqKind::Angle, &all8, o).with_k0(theta - 0.3),
            Eq::new(EqKind::EqualLength, &all8, o),
            Eq::new(EqKind::Dist, &[0, 1, 4, 5], o)
                .minus_radius(Radius::Slot(10), 1.0)
                .minus_radius(Radius::Slot(11), -1.0),
            Eq::new(EqKind::Dist, &[8, 9, 0, 1], o)
                .minus_radius(arc, 1.0)
                .minus_radius(
                    Radius::Arc {
                        center: 2,
                        start: 6,
                    },
                    -1.0,
                ),
            Eq::new(EqKind::Zero, &[], o)
                .minus_radius(arc, -1.0)
                .minus_radius(Radius::Slot(11), 1.0),
            // Shared slots (aliasing): the second line starts where the first ends; these two
            // directions are antiparallel, so keep k0 away from the wrap.
            Eq::new(EqKind::Angle, &[0, 1, 2, 3, 2, 3, 6, 7], o).with_k0(2.8),
        ];
        for eq in &eqs {
            check_gradient(eq, &v);
        }
    }

    #[test]
    fn wrap() {
        assert!((wrap_angle(3.0 * PI) - PI).abs() < 1e-12);
        assert!((wrap_angle(-PI) - PI).abs() < 1e-12);
        assert!((wrap_angle(0.5) - 0.5).abs() < 1e-12);
        assert!((wrap_angle(-0.5 - TAU) + 0.5).abs() < 1e-12);
    }
}
