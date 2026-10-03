//! Corners: where two flanges on neighbouring edges of the same face meet.
//!
//! When a flange is added on an edge that ends where another flange's edge ends (and
//! neither is set back from that end), the two meet in a corner. Left alone they would
//! collide twice: their bends claim the same corner of the flat pattern, and their flanges
//! fill the same column of material in the folded part. A [`Corner`] records the meeting
//! with how to treat it ([`CornerSpec`]), and is *resolved* when the body is built
//! ([`Layout::resolved`]): the layout itself only says "these two meet, like this", so a
//! later feature can change the treatment and the corner is worked out again.
//!
//! **Relief.** A region around the corner is cut from the face and from both bends (not
//! from the flanges). Its two edges are square to each bend line, so the bends end
//! cleanly and fold exactly: a line across a bend must be square or parallel to it. The
//! region reaches past both bend zones and the strips the face gave up to them, plus a
//! margin (`relief_size`, none for a tear relief).
//!
//! **Flange ends.** Each flange's end at the corner is extended a little and then cut
//! back against a plane of the other flange, in the folded part:
//! - *butt*: the earlier flange runs to the later one's outer face, and the later one
//!   stops `gap` short of the earlier one's inner face;
//! - *overlap*: the other way round;
//! - *open*: both stop `gap` short of the other's inner face, leaving the corner open.
//!
//! Walls through the sheet are square to it, so a cut is the line where the plane meets
//! the flange, at whichever face of the sheet is the stricter. For the usual square
//! corner (square edges, square bends) that is a square end, so the flat pattern stays
//! simple. The cut is made in the flat pattern, so the flange keeps its exact shape there.

use peet_math::{DVec2, DVec3};

use crate::layout::{Area, CurveTag, Cut, Edge2, Layout, MIN_LENGTH, PieceKind, wall};

/// How the flanges at a corner meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CornerKind {
    /// The later flange butts against the earlier one, which runs to the corner.
    Butt,
    /// The earlier flange butts against the later one.
    Overlap,
    /// Neither reaches the corner: both stop short of the other's inner face.
    Open,
}

impl CornerKind {
    pub const ALL: [Self; 3] = [Self::Butt, Self::Overlap, Self::Open];

    pub fn label(self) -> &'static str {
        match self {
            Self::Butt => "Butt",
            Self::Overlap => "Overlap",
            Self::Open => "Open",
        }
    }
}

/// The relief cut where two bends meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CornerRelief {
    /// A rectangle square to both bends, reaching `relief_size` past them.
    Rectangular,
    /// Only what the bends would otherwise share: the corner tears there.
    Tear,
}

impl CornerRelief {
    pub const ALL: [Self; 2] = [Self::Rectangular, Self::Tear];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rectangular => "Rectangular",
            Self::Tear => "Tear",
        }
    }
}

/// How a corner is treated, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerSpec {
    pub kind: CornerKind,
    /// The gap left between the flanges.
    pub gap: f64,
    pub relief: CornerRelief,
    /// How far a rectangular relief reaches past the bends.
    pub relief_size: f64,
}

/// The gap a corner gets unless a corner feature says otherwise, in mm.
pub const DEFAULT_GAP: f64 = 0.1;

/// Two flanges meeting at a corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    /// The attachments (earlier first) and which end of each meets the corner (0: the
    /// start of its span, 1: the end).
    pub attachments: [usize; 2],
    pub ends: [usize; 2],
    /// The face's corner point.
    pub at: DVec2,
    pub spec: CornerSpec,
}

/// Name numbers for the walls of corner reliefs: `part` is this plus the end of the
/// later flange that meets the corner.
pub const CORNER_PART: u32 = 0x100;

impl Layout {
    /// The corner treatment a new corner gets from the body's settings.
    pub fn default_corner(&self) -> CornerSpec {
        let tear = self.settings.relief == crate::ReliefType::Tear;
        CornerSpec {
            kind: CornerKind::Butt,
            gap: DEFAULT_GAP,
            relief: if tear {
                CornerRelief::Tear
            } else {
                CornerRelief::Rectangular
            },
            relief_size: self.settings.relief_ratio * self.settings.thickness,
        }
    }

    /// Records the corners the newest attachment makes with earlier ones on the same face.
    pub(crate) fn find_corners(&mut self, new: usize) {
        let spec = self.default_corner();
        let n = &self.attachments[new];
        if !self.makes_corners(n) {
            return;
        }
        let ends = |a: &crate::flange::Attachment| {
            let d = a.along();
            [a.a + d * a.span[0], a.a + d * a.span[1]]
        };
        let new_ends = ends(n);
        let mut found = Vec::new();
        for (i, old) in self.attachments[..new].iter().enumerate() {
            if old.parent != n.parent || !self.makes_corners(old) {
                continue;
            }
            // Edges in line meet end to end, not in a corner.
            if old.along().perp_dot(n.along()).abs() < 1e-6 {
                continue;
            }
            let old_ends = ends(old);
            for (eo, po) in old_ends.iter().enumerate() {
                for (en, pn) in new_ends.iter().enumerate() {
                    if po.distance(*pn) <= MIN_LENGTH {
                        found.push(Corner {
                            attachments: [i, new],
                            ends: [eo, en],
                            at: *po,
                            spec,
                        });
                    }
                }
            }
        }
        self.corners.extend(found);
    }

    /// Whether an attachment meets its neighbours in corners: flanges do; hems don't
    /// (a hem lies on the sheet, so it ends where its edge does and needs no fitting).
    pub(crate) fn makes_corners(&self, a: &crate::flange::Attachment) -> bool {
        a.pieces.len() >= 2
            && self.pieces[a.pieces[0]]
                .bend()
                .is_some_and(|b| !b.values.is_hem())
    }

    /// The corners an attachment's flange meets, by index into `corners`.
    pub fn corners_of(&self, attachment: usize) -> impl Iterator<Item = usize> + '_ {
        self.corners
            .iter()
            .enumerate()
            .filter(move |(_, c)| c.attachments.contains(&attachment))
            .map(|(i, _)| i)
    }

    /// The layout with its corners worked out: reliefs added as cuts, flange ends cut
    /// to fit. Pieces keep their indices.
    pub fn resolved(&self) -> Layout {
        let mut out = self.clone();
        if self.corners.is_empty() {
            return out;
        }
        let t = self.settings.thickness;
        let size = self.extent() + 10.0;

        // Per flat: the half-planes each of its ends must keep to. Per later bend: how far
        // each end may reach along its edge.
        struct EndCut {
            end: usize,
            planes: Vec<(DVec2, f64)>,
        }
        let mut per_flat: Vec<(usize, Vec<EndCut>)> = Vec::new();
        let mut bend_ends: Vec<(usize, usize, f64)> = Vec::new();
        let mut push =
            |flat: usize, cut: EndCut| match per_flat.iter_mut().find(|(f, _)| *f == flat) {
                Some(s) => s.1.push(cut),
                None => per_flat.push((flat, vec![cut])),
            };
        for (ci, c) in self.corners.iter().enumerate() {
            let [ia, ib] = c.attachments;
            let (a, b) = (&self.attachments[ia], &self.attachments[ib]);
            let gap = c.spec.gap.max(0.0);
            let flats = (a.pieces.len() / 2).min(b.pieces.len() / 2);
            let mut planes_of: Vec<[Vec<(DVec2, f64)>; 2]> = Vec::new();
            for k in 0..flats {
                let (fa, fb) = (a.pieces[2 * k + 1], b.pieces[2 * k + 1]);
                let planes = if k > 0 && self.coplanar(fa, fb) {
                    // Lips in one plane: mitred along the line that halves the corner.
                    match self.miter(c, k) {
                        Some(p) => p,
                        None => break,
                    }
                } else {
                    // (keep beyond the other's outer face (true) or inner face, gap)
                    let rules = match c.spec.kind {
                        CornerKind::Butt => [(true, 0.0), (false, gap)],
                        CornerKind::Overlap => [(false, gap), (true, 0.0)],
                        CornerKind::Open => [(false, gap), (false, gap)],
                    };
                    [
                        self.keep_clear(fa, fb, rules[0].0, rules[0].1),
                        self.keep_clear(fb, fa, rules[1].0, rules[1].1),
                    ]
                };
                push(
                    fa,
                    EndCut {
                        end: c.ends[0],
                        planes: planes[0].clone(),
                    },
                );
                push(
                    fb,
                    EndCut {
                        end: c.ends[1],
                        planes: planes[1].clone(),
                    },
                );
                planes_of.push(planes);
            }
            // Later bends end square, where the tighter of the flats on either side of
            // them ends along their shared line.
            for k in 1..planes_of.len() {
                for (side, att) in [a, b].into_iter().enumerate() {
                    let bend = att.pieces[2 * k];
                    let Some(bv) = self.pieces[bend].bend() else {
                        continue;
                    };
                    let end = c.ends[side];
                    let lines = [bv.origin, bv.origin + bv.across * bv.width()];
                    let mut reach = if end == 0 {
                        f64::NEG_INFINITY
                    } else {
                        f64::INFINITY
                    };
                    for (line, planes) in lines.iter().zip([&planes_of[k - 1], &planes_of[k]]) {
                        for &(n, kk) in &planes[side] {
                            // n·(line + along·u) ≥ kk.
                            let slope = n.dot(bv.along);
                            if slope.abs() < 1e-12 {
                                continue;
                            }
                            let u = (kk - n.dot(*line)) / slope;
                            reach = if end == 0 { reach.max(u) } else { reach.min(u) };
                        }
                    }
                    if reach.is_finite() {
                        bend_ends.push((bend, end, reach));
                    }
                }
            }

            // The relief, cut from the face and both first bends.
            let (sa, sb) = (a.pieces[0], b.pieces[0]);
            if let Some(area) = self.corner_relief(c, ci, size) {
                out.cuts.push(Cut {
                    area,
                    pieces: vec![a.parent, sa, sb],
                });
            }
        }
        let grow = 2.0 * (self.max_radius() + t) + 4.0 * t + 1.0;
        for (flat, cuts) in per_flat {
            let piece = &self.pieces[flat];
            let Some(att) = self.attachment_of(flat) else {
                continue;
            };
            let d = att.along();
            let o = att.out();
            // The flat's rectangle, in (along, across) from the edge's start.
            let (lo, hi) = rect_extent(&piece.outline, att.a, d, o);
            let mut u = [lo.x, hi.x];
            for c in &cuts {
                if c.end == 0 {
                    u[0] -= grow;
                } else {
                    u[1] += grow;
                }
            }
            let tag = |index: u8| CurveTag::Generated {
                owner: piece.origin.owner,
                part: piece.origin.part,
                index,
            };
            let p = |x: f64, y: f64| att.a + d * x + o * y;
            // Counter-clockwise: (along, out) turns clockwise, so go round the other way.
            let mut poly: Vec<(DVec2, CurveTag)> = vec![
                (p(u[0], lo.y), tag(wall::FLANGE_START)),
                (p(u[0], hi.y), tag(wall::TIP)),
                (p(u[1], hi.y), tag(wall::FLANGE_END)),
                (p(u[1], lo.y), tag(wall::FLANGE_FOLD)),
            ];
            for c in &cuts {
                let end_tag = tag(if c.end == 0 {
                    wall::FLANGE_START
                } else {
                    wall::FLANGE_END
                });
                for &(n, k) in &c.planes {
                    poly = clip(&poly, n, k, end_tag);
                }
            }
            if poly.len() >= 3 {
                out.pieces[flat].outline = polygon_area(&poly);
            }
        }
        for (bend, end, reach) in bend_ends {
            let piece = &out.pieces[bend];
            let Some(bv) = piece.bend().copied() else {
                continue;
            };
            let (lo, hi) = rect_extent(&piece.outline, bv.origin, bv.along, bv.across);
            let mut u = [lo.x, hi.x];
            if end == 0 {
                u[0] = u[0].max(reach);
            } else {
                u[1] = u[1].min(reach);
            }
            if u[1] - u[0] <= MIN_LENGTH {
                continue;
            }
            let tag = |index: u8| CurveTag::Generated {
                owner: piece.origin.owner,
                part: piece.origin.part,
                index,
            };
            let far = if bv.child.is_some() {
                wall::BEND_CHILD
            } else {
                wall::TIP
            };
            // The same rectangle as `rect` makes, with the ends moved.
            out.pieces[bend].outline = crate::layout::rect(
                bv.origin,
                bv.along,
                bv.across,
                u,
                [lo.y, hi.y],
                [
                    tag(wall::BEND_PARENT),
                    tag(wall::BEND_END),
                    tag(far),
                    tag(wall::BEND_START),
                ],
            );
        }
        out
    }

    /// The attachment a flat belongs to.
    fn attachment_of(&self, flat: usize) -> Option<&crate::flange::Attachment> {
        self.attachments.iter().find(|a| a.pieces.contains(&flat))
    }

    /// Whether two flats lie in one plane in the folded part.
    fn coplanar(&self, a: usize, b: usize) -> bool {
        let (fa, fb) = (self.pieces[a].frame, self.pieces[b].frame);
        let (na, nb) = (fa.z_axis(), fb.z_axis());
        na.cross(nb).length() < 1e-9 && na.dot(fb.origin - fa.origin).abs() < 1e-6
    }

    /// The half-planes of a mitre between the `k`-th flats of a corner's two flanges,
    /// which lie in one plane: each keeps to its side of the line through the meeting
    /// points of their fold lines and of their tip lines, half the gap clear.
    fn miter(&self, c: &Corner, k: usize) -> Option<[Vec<(DVec2, f64)>; 2]> {
        let t = self.settings.thickness;
        let atts = c.attachments.map(|i| &self.attachments[i]);
        let flats = atts.map(|a| a.pieces[2 * k + 1]);
        // Fold and tip lines of each flat in the model, and a point inside it.
        let lines = |i: usize| {
            let att = atts[i];
            let piece = &self.pieces[flats[i]];
            let (d, o) = (att.along(), att.out());
            let (lo, hi) = rect_extent(&piece.outline, att.a, d, o);
            let at = |x: f64, y: f64| {
                piece
                    .frame
                    .to_world((att.a + d * x + o * y).extend(t / 2.0))
            };
            (
                (at(lo.x, lo.y), at(hi.x, lo.y)),
                (at(lo.x, hi.y), at(hi.x, hi.y)),
                at((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0),
            )
        };
        let (la, lb) = (lines(0), lines(1));
        let meet = |p: (DVec3, DVec3), q: (DVec3, DVec3)| -> Option<DVec3> {
            // The point of line p closest to line q (coplanar lines meet there).
            let (d1, d2) = (p.1 - p.0, q.1 - q.0);
            let r = p.0 - q.0;
            let (a, b, e) = (d1.dot(d1), d1.dot(d2), d2.dot(d2));
            let (cc, f) = (d1.dot(r), d2.dot(r));
            let den = a * e - b * b;
            if den.abs() < 1e-12 * a * e {
                return None;
            }
            let s = (b * f - cc * e) / den;
            Some(p.0 + d1 * s)
        };
        let x1 = meet(la.0, lb.0)?;
        let x2 = meet(la.1, lb.1)?;
        let normal = self.pieces[flats[0]].frame.z_axis();
        let along = x2 - x1;
        if along.length() < 1e-9 {
            return None;
        }
        // The mitre plane holds the line and the sheet normal.
        let n = along.cross(normal).normalize();
        let gap = c.spec.gap.max(0.0) / 2.0;
        let mut out: [Vec<(DVec2, f64)>; 2] = [Vec::new(), Vec::new()];
        for (i, inside) in [la.2, lb.2].into_iter().enumerate() {
            let n = if n.dot(inside - x1) >= 0.0 { n } else { -n };
            let frame = self.pieces[flats[i]].frame;
            let g = frame.vector_to_local(n);
            for level in [0.0, t] {
                let k = gap - g.z * level - n.dot(frame.origin - x1);
                out[i].push((g.truncate(), k));
            }
        }
        Some(out)
    }

    /// Half-planes `n·p ≥ k` (flat coordinates) that keep flange `flange` on the far side
    /// of flange `other`'s outer (or inner) face, `gap` clear, at both faces of the sheet.
    fn keep_clear(&self, flange: usize, other: usize, outer: bool, gap: f64) -> Vec<(DVec2, f64)> {
        let t = self.settings.thickness;
        let (fp, op) = (&self.pieces[flange], &self.pieces[other]);
        let up_of = |piece: usize| {
            self.bends()
                .find(|(_, b)| b.child == Some(piece))
                .is_some_and(|(_, b)| b.up)
        };
        // The other flange's inside direction (from its outer face to its inner face).
        let inside = op
            .frame
            .vector_to_world(if up_of(other) { DVec3::Z } else { -DVec3::Z });
        let z = match (up_of(other), outer) {
            (true, true) | (false, false) => 0.0,
            (true, false) | (false, true) => t,
        };
        let face_point = op.frame.to_world(DVec3::new(0.0, 0.0, z));
        let mut out = Vec::new();
        for level in [0.0, t] {
            // Signed distance from the face of a flat point p at this level:
            // inside · (R·(p, level) + origin − face_point).
            let g = fp.frame.vector_to_local(inside);
            let n = g.truncate();
            if n.length() < 1e-9 {
                continue; // parallel faces: nothing to cut
            }
            let k = gap - g.z * level - inside.dot(fp.frame.origin - face_point);
            out.push((n, k));
        }
        out
    }

    /// The relief region of corner `c`, or `None` if it has no area.
    fn corner_relief(&self, c: &Corner, index: usize, size: f64) -> Option<Area> {
        let [ia, ib] = c.attachments;
        let (a, b) = (&self.attachments[ia], &self.attachments[ib]);
        let margin = match c.spec.relief {
            CornerRelief::Rectangular => c.spec.relief_size.max(0.0),
            CornerRelief::Tear => 0.0,
        };
        // Each edge's band: the trim and the bend strip, as distances into the face.
        let band = |att: &crate::flange::Attachment| -> [f64; 2] {
            let ba = match self.pieces[att.pieces[0]].kind {
                PieceKind::Bend(b) => b.width(),
                PieceKind::Flange => 0.0,
            };
            [(att.trim - ba).min(0.0), att.trim.max(0.0)]
        };
        // Direction along each edge away from the corner, and into the face.
        let away = |att: &crate::flange::Attachment, end: usize| {
            if end == 0 { att.along() } else { -att.along() }
        };
        let (da, db) = (away(a, c.ends[0]), away(b, c.ends[1]));
        let (ina, inb) = (-a.out(), -b.out());
        let (ba, bb) = (band(a), band(b));
        // Corners of the parallelogram where the bands cross: p = at + ina·s + inb·r.
        // Solve for the point at depth s into face A's band and r into B's band.
        let solve = |s: f64, r: f64| -> Option<DVec2> {
            // p·ina = at·ina + s, p·inb = at·inb + r (unit normals of the two edges).
            let det = ina.perp_dot(inb);
            if det.abs() < 1e-9 {
                return None;
            }
            let (ka, kb) = (s, r);
            // p - at = x: x·ina = ka, x·inb = kb.
            let x = DVec2::new(
                (ka * inb.y - kb * ina.y) / (ina.x * inb.y - ina.y * inb.x),
                (ina.x * kb - inb.x * ka) / (ina.x * inb.y - ina.y * inb.x),
            );
            Some(c.at + x)
        };
        let mut reach_a = f64::NEG_INFINITY;
        let mut reach_b = f64::NEG_INFINITY;
        for s in ba {
            for r in bb {
                let p = solve(s, r)?;
                reach_a = reach_a.max((p - c.at).dot(da));
                reach_b = reach_b.max((p - c.at).dot(db));
            }
        }
        let (ea, eb) = (reach_a + margin, reach_b + margin);
        if ea <= MIN_LENGTH && eb <= MIN_LENGTH {
            return None;
        }
        let tag = |i: u8| CurveTag::Generated {
            owner: b.owner,
            part: CORNER_PART + c.ends[1] as u32,
            index: i + 4 * (index % 32) as u8,
        };
        // A big square around the corner, cut down to the wedge in front of both edges.
        let s = size;
        let mut poly: Vec<(DVec2, CurveTag)> = vec![
            (c.at + DVec2::new(-s, -s), tag(0)),
            (c.at + DVec2::new(s, -s), tag(0)),
            (c.at + DVec2::new(s, s), tag(0)),
            (c.at + DVec2::new(-s, s), tag(0)),
        ];
        // Keep (p − at)·da ≤ ea, i.e. (−da)·p ≥ −(ea + at·da).
        poly = clip(&poly, -da, -(ea + c.at.dot(da)), tag(1));
        poly = clip(&poly, -db, -(eb + c.at.dot(db)), tag(2));
        if poly.len() < 3 {
            return None;
        }
        Some(polygon_area(&poly))
    }

    /// The size of the flat pattern (the diagonal of every outline's bounds).
    fn extent(&self) -> f64 {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for p in &self.pieces {
            for e in p.outline.edges() {
                let (a, b) = e.curve.bounds();
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if lo.x > hi.x { 0.0 } else { lo.distance(hi) }
    }

    fn max_radius(&self) -> f64 {
        self.bends()
            .map(|(_, b)| b.values.radius)
            .fold(self.settings.radius, f64::max)
    }
}

/// The bounds of an area's points in the frame `(o, d, n)`: `(min, max)` of (along,
/// across).
fn rect_extent(area: &Area, o: DVec2, d: DVec2, n: DVec2) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in area.edges() {
        for p in [e.curve.start(), e.curve.end()] {
            let q = DVec2::new((p - o).dot(d), (p - o).dot(n));
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    (lo, hi)
}

/// A polygon of tagged points (each tags the edge from it to the next) as an area.
fn polygon_area(poly: &[(DVec2, CurveTag)]) -> Area {
    Area {
        loops: vec![
            (0..poly.len())
                .map(|i| Edge2::line(poly[i].0, poly[(i + 1) % poly.len()].0, poly[i].1))
                .collect(),
        ],
    }
}

/// Clips a convex polygon (points with the tag of the edge that starts there) to the
/// half-plane `n·p ≥ k`. The new edge along the line gets `tag`.
fn clip(poly: &[(DVec2, CurveTag)], n: DVec2, k: f64, tag: CurveTag) -> Vec<(DVec2, CurveTag)> {
    let inside = |p: DVec2| n.dot(p) >= k - 1e-12;
    let mut out: Vec<(DVec2, CurveTag)> = Vec::new();
    for i in 0..poly.len() {
        let (p, tp) = poly[i];
        let (q, _) = poly[(i + 1) % poly.len()];
        let (ip, iq) = (inside(p), inside(q));
        if ip {
            out.push((p, tp));
        }
        if ip != iq {
            let (dp, dq) = (n.dot(p) - k, n.dot(q) - k);
            let x = p + (q - p) * (dp / (dp - dq));
            // Entering: the edge from here runs along the old edge; leaving: along the
            // line.
            out.push((x, if ip { tag } else { tp }));
        }
    }
    // Drop points closer than the tolerance to the one before.
    let mut cleaned: Vec<(DVec2, CurveTag)> = Vec::new();
    for v in out {
        if cleaned
            .last()
            .is_none_or(|l: &(DVec2, CurveTag)| l.0.distance(v.0) > MIN_LENGTH)
        {
            cleaned.push(v);
        }
    }
    while cleaned.len() > 1 && cleaned[0].0.distance(cleaned[cleaned.len() - 1].0) <= MIN_LENGTH {
        cleaned.pop();
    }
    cleaned
}
