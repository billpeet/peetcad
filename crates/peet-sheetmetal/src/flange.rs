//! Flanges added on an edge: edge flanges, hems and flanges with a sketched profile.
//!
//! All of them are a *profile* laid out from a straight boundary edge of a flange: a chain
//! of bends, each followed by a flat segment, running away from the edge in the flat
//! pattern. An edge flange is one bend and one flat. A hem is a bend of 180° (or more)
//! and a flat that comes back over the sheet; a rolled hem is a bend alone. A flange
//! profile from a sketch is any number of bends and flats.
//!
//! Every piece of a profile is a strip as long as the flange's span along the edge. Each
//! bend folds from the piece before it, so the placement of the last flat is the edge
//! flange's placement composed with every bend's fold.
//!
//! The edge's parent gives up a strip along the edge (a *trim*) when the flange's
//! position puts the bend inside the original outline, and a relief is cut where the
//! bend stops short of an end of the edge.

use std::f64::consts::{PI, TAU};

use peet_math::DVec2;
use peet_sketch::Curve;

use crate::layout::{
    Area, Bend, CurveTag, Cut, Edge2, EdgeSite, Layout, MIN_LENGTH, Origin, Piece, PieceKind, rect,
    wall,
};
use crate::settings::{BendValues, FlangePosition, ReliefType};

/// An edge flange's values, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeFlangeSpec {
    /// Outside length: from the outer virtual sharp to the flange's end.
    pub length: f64,
    /// Bend angle in degrees.
    pub angle: f64,
    pub position: FlangePosition,
    /// How far each end of the flange is set back from the ends of the edge.
    pub offsets: [f64; 2],
    /// Bend to the other side.
    pub flip: bool,
    /// Inner radius, if not the body's default.
    pub radius: Option<f64>,
}

/// The kinds of hem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HemKind {
    /// Folded flat onto the sheet (a very small inner radius).
    Closed,
    /// Folded back with a gap between the sheet and the hem.
    Open,
    /// Folded past 180° so the end comes back towards the sheet.
    Teardrop,
    /// A curl: a bend of more than 180° with no flat after it.
    Rolled,
}

impl HemKind {
    pub const ALL: [Self; 4] = [Self::Closed, Self::Open, Self::Teardrop, Self::Rolled];

    pub fn label(self) -> &'static str {
        match self {
            Self::Closed => "Closed",
            Self::Open => "Open",
            Self::Teardrop => "Teardrop",
            Self::Rolled => "Rolled",
        }
    }
}

/// A hem's values, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HemSpec {
    pub kind: HemKind,
    /// Closed and open hems: from the outside of the fold to the end of the hem. Teardrop:
    /// the flat length after the bend. Not used for rolled hems.
    pub length: f64,
    /// Open hems: the gap between the sheet and the hem.
    pub gap: f64,
    /// Teardrop and rolled hems: the inner radius.
    pub radius: f64,
    /// Teardrop and rolled hems: the angle turned through, in degrees (more than 180).
    pub angle: f64,
    /// The outside of the fold is flush with the original edge (else the fold starts at
    /// the edge and the part grows).
    pub inside: bool,
    pub offsets: [f64; 2],
    /// Fold to the other side.
    pub flip: bool,
}

/// The inner radius of a closed hem, as a fraction of the thickness: small enough to
/// look and unfold like a flattened hem, large enough to be a sound solid.
pub const CLOSED_HEM_RADIUS: f64 = 0.01;

/// One step of a profile: a bend, then a flat segment (none after the last bend of a
/// rolled hem).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub values: BendValues,
    /// Folds towards the top side of the sheet.
    pub up: bool,
    /// Length of the flat after the bend, in the flat pattern.
    pub flat: f64,
}

/// A flange, hem or profile added on an edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub owner: u32,
    /// The flange piece the edge is on.
    pub parent: usize,
    /// The edge, with the parent's material on its left.
    pub a: DVec2,
    pub b: DVec2,
    /// Where the profile starts and ends along the edge (distances from `a`).
    pub span: [f64; 2],
    /// How far into the parent the first bend starts.
    pub trim: f64,
    /// The profile's pieces in order: bend, flat, bend, flat…
    pub pieces: Vec<usize>,
}

impl Attachment {
    /// Unit direction of the edge.
    pub fn along(&self) -> DVec2 {
        (self.b - self.a).normalize_or_zero()
    }

    /// Unit direction away from the parent's material.
    pub fn out(&self) -> DVec2 {
        let d = self.along();
        DVec2::new(d.y, -d.x)
    }
}

impl Layout {
    /// Adds an edge flange on a straight boundary edge of a flange.
    pub fn add_edge_flange(
        &mut self,
        owner: u32,
        site: &EdgeSite,
        spec: &EdgeFlangeSpec,
    ) -> Result<(), String> {
        let t = self.settings.thickness;
        let radius = spec.radius.unwrap_or(self.settings.radius);
        if !(radius.is_finite() && radius >= 0.0) {
            return Err("The bend radius can't be negative.".to_owned());
        }
        if !spec.length.is_finite() {
            return Err("The length is not a number.".to_owned());
        }
        let values = BendValues::new(self.settings.model, spec.angle.to_radians(), radius, t)?;
        let ossb = values.outside_setback();
        let straight = spec.length - ossb;
        if straight <= MIN_LENGTH {
            return Err(format!(
                "The flange is too short for its bend: make it longer than {ossb:.3} mm (measured on the outside)."
            ));
        }
        let trim = match spec.position {
            FlangePosition::MaterialInside => ossb,
            FlangePosition::MaterialOutside => values.inside_setback(),
            FlangePosition::BendOutside => 0.0,
        };
        let segments = [Segment {
            values,
            up: site.top != spec.flip,
            flat: straight,
        }];
        self.add_profile(owner, site, spec.offsets, trim, &segments, 0)
            .map(|_| ())
    }

    /// Adds a hem on a straight boundary edge of a flange.
    pub fn add_hem(&mut self, owner: u32, site: &EdgeSite, spec: &HemSpec) -> Result<(), String> {
        let t = self.settings.thickness;
        let model = self.settings.model;
        let finite = |v: f64, what: &str| {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("The {what} is not a number."))
            }
        };
        let (values, flat) = match spec.kind {
            HemKind::Closed | HemKind::Open => {
                let radius = if spec.kind == HemKind::Closed {
                    CLOSED_HEM_RADIUS * t
                } else {
                    let gap = finite(spec.gap, "gap")?;
                    if gap <= MIN_LENGTH {
                        return Err(
                            "The gap of an open hem must be greater than zero (use a closed hem for none)."
                                .to_owned(),
                        );
                    }
                    gap / 2.0
                };
                let values = BendValues::hem(model, PI, radius, t)?;
                let reach = radius + t;
                let flat = finite(spec.length, "length")? - reach;
                if flat <= MIN_LENGTH {
                    return Err(format!(
                        "The hem is too short for its fold: make it longer than {reach:.3} mm (measured from the outside of the fold)."
                    ));
                }
                (values, flat)
            }
            HemKind::Teardrop | HemKind::Rolled => {
                let angle = finite(spec.angle, "angle")?;
                if angle <= 180.0 + 1e-6 {
                    return Err(format!(
                        "A {} hem turns through more than 180° (this one is {angle:.3}°).",
                        spec.kind.label().to_lowercase()
                    ));
                }
                let radius = finite(spec.radius, "radius")?;
                let values = BendValues::hem(model, angle.to_radians(), radius, t)?;
                // Past this angle the outside of the curl comes back into the sheet.
                let most = TAU - (radius / (radius + t)).acos();
                if values.angle > most - 1e-9 {
                    return Err(format!(
                        "The hem curls back into the sheet: with this radius it can turn through at most {:.1}°.",
                        most.to_degrees()
                    ));
                }
                let flat = if spec.kind == HemKind::Rolled {
                    0.0
                } else {
                    let flat = finite(spec.length, "length")?;
                    if flat <= MIN_LENGTH {
                        return Err(
                            "The length of a teardrop hem's flat end must be greater than zero."
                                .to_owned(),
                        );
                    }
                    // The end heads back down towards the sheet: its inner face reaches the
                    // sheet after this length.
                    let a = values.angle;
                    let reach = radius * (1.0 - a.cos()) / -a.sin();
                    if flat > reach + 1e-9 {
                        return Err(format!(
                            "The teardrop's end runs into the sheet: make it at most {reach:.3} mm long, or use a smaller angle."
                        ));
                    }
                    flat
                };
                (values, flat)
            }
        };
        let trim = if spec.inside {
            values.outside_reach()
        } else {
            0.0
        };
        let segments = [Segment {
            values,
            up: site.top != spec.flip,
            flat,
        }];
        self.add_profile(owner, site, spec.offsets, trim, &segments, 0)
            .map(|_| ())
    }

    /// Lays out a profile on an edge: its bends and flats, the strip the parent gives up
    /// (`trim` deep) and reliefs where it stops short of the edge's ends. Returns the
    /// index of its [`Attachment`].
    pub fn add_profile(
        &mut self,
        owner: u32,
        site: &EdgeSite,
        offsets: [f64; 2],
        trim: f64,
        segments: &[Segment],
        part_base: u32,
    ) -> Result<usize, String> {
        // Hems end where their edge does; flanges are carried on to the face's corners.
        let carried = if segments.first().is_some_and(|s| !s.values.is_hem()) {
            self.carried_to_corners(site, offsets)
        } else {
            *site
        };
        let site = &carried;
        let parent = self
            .pieces
            .get(site.piece)
            .ok_or("The edge is not on this body.")?;
        if !parent.is_flange() {
            return Err("Flanges go on flat faces, not on bends.".to_owned());
        }
        if segments.is_empty() {
            return Err("The profile has no bends.".to_owned());
        }
        for (k, s) in segments.iter().enumerate() {
            let last = k + 1 == segments.len();
            if !(s.flat.is_finite() && (s.flat > MIN_LENGTH || (last && s.flat == 0.0))) {
                return Err(format!(
                    "Segment {} of the profile is too short for its bends.",
                    k + 1
                ));
            }
        }
        let t = self.settings.thickness;
        let len = site.a.distance(site.b);
        let [o0, o1] = offsets;
        if !(o0.is_finite() && o1.is_finite()) || o0 < 0.0 || o1 < 0.0 {
            return Err("The offsets from the ends of the edge can't be negative.".to_owned());
        }
        let span = [o0, len - o1];
        if span[1] - span[0] <= MIN_LENGTH {
            return Err(format!(
                "The offsets ({o0:.3} + {o1:.3} mm) leave nothing of the {len:.3} mm edge."
            ));
        }
        let d = (site.b - site.a) / len;
        let out = DVec2::new(d.y, -d.x); // right of the edge: away from the material
        let inward = -out;
        let origin = site.a + d * span[0] + inward * trim;
        let w = span[1] - span[0];

        let mut frame = parent.frame;
        let mut parent_index = site.piece;
        let mut x = 0.0;
        let mut new_pieces = Vec::new();
        for (k, seg) in segments.iter().enumerate() {
            let part = part_base + k as u32;
            let tag = |index: u8| CurveTag::Generated { owner, part, index };
            let ba = seg.values.allowance;
            let bend_index = self.pieces.len();
            let has_flat = seg.flat > 0.0;
            let bend = Bend {
                parent: parent_index,
                child: has_flat.then_some(bend_index + 1),
                origin: origin + out * x,
                along: d,
                across: out,
                length: w,
                values: seg.values,
                up: seg.up,
            };
            let far = if has_flat {
                wall::BEND_CHILD
            } else {
                wall::TIP
            };
            self.pieces.push(Piece {
                origin: Origin { owner, part },
                kind: PieceKind::Bend(bend),
                outline: rect(
                    origin,
                    d,
                    out,
                    [0.0, w],
                    [x, x + ba],
                    [
                        tag(wall::BEND_PARENT),
                        tag(wall::BEND_END),
                        tag(far),
                        tag(wall::BEND_START),
                    ],
                ),
                trims: Vec::new(),
                frame,
            });
            new_pieces.push(bend_index);
            x += ba;
            if !has_flat {
                break;
            }
            frame = frame.compose(&bend.fold_frame());
            parent_index = bend_index + 1;
            self.pieces.push(Piece {
                origin: Origin { owner, part },
                kind: PieceKind::Flange,
                outline: rect(
                    origin,
                    d,
                    out,
                    [0.0, w],
                    [x, x + seg.flat],
                    [
                        tag(wall::FLANGE_FOLD),
                        tag(wall::FLANGE_END),
                        tag(wall::TIP),
                        tag(wall::FLANGE_START),
                    ],
                ),
                trims: Vec::new(),
                frame,
            });
            new_pieces.push(parent_index);
            x += seg.flat;
        }

        // Reliefs where the bend stops short of an end of the edge. They cut the parent
        // (and whatever else was there), not the new flange.
        let tag = |index: u8| CurveTag::Generated {
            owner,
            part: part_base,
            index,
        };
        let mut reliefs = Vec::new();
        if self.settings.relief != ReliefType::Tear {
            let rw = self.settings.relief_ratio * t;
            let depth = trim + self.settings.relief_ratio * t;
            if o0 > MIN_LENGTH {
                reliefs.push(relief(
                    self.settings.relief,
                    site.a,
                    d,
                    inward,
                    [(o0 - rw).max(0.0), o0],
                    depth,
                    |i| tag(wall::RELIEF_START + i),
                ));
            }
            if o1 > MIN_LENGTH {
                reliefs.push(relief(
                    self.settings.relief,
                    site.a,
                    d,
                    inward,
                    [span[1], (span[1] + rw).min(len)],
                    depth,
                    |i| tag(wall::RELIEF_END + i),
                ));
            }
        }
        if trim > MIN_LENGTH {
            let trim_area = rect(
                site.a,
                d,
                inward,
                [span[0], span[1]],
                [0.0, trim],
                [
                    tag(wall::TRIM),
                    tag(wall::TRIM + 1),
                    tag(wall::TRIM + 2),
                    tag(wall::TRIM + 3),
                ],
            );
            self.pieces[site.piece].trims.push(trim_area);
        }
        let before: Vec<usize> = (0..new_pieces[0]).collect();
        for area in reliefs.into_iter().flatten() {
            self.cuts.push(Cut {
                area,
                pieces: before.clone(),
            });
        }
        self.attachments.push(Attachment {
            owner,
            parent: site.piece,
            a: site.a,
            b: site.b,
            span,
            trim,
            pieces: new_pieces,
        });
        let index = self.attachments.len() - 1;
        self.find_corners(index);
        Ok(index)
    }
}

impl Layout {
    /// The edge carried on to the face's corners where it stops short of them.
    ///
    /// A flange that takes its bend out of the face (material inside) shortens the
    /// neighbouring edges by the strip it took. A flange then added on such an edge
    /// would stop short of the corner and leave a notch. If an end of the edge (with no
    /// offset) lies in line with the end of an earlier flange's edge and within the
    /// strip that flange took, the edge is carried on to that corner, where the two
    /// flanges then meet (see [`crate::corner`]).
    fn carried_to_corners(&self, site: &EdgeSite, offsets: [f64; 2]) -> EdgeSite {
        let mut site = *site;
        let len = site.a.distance(site.b);
        if len <= MIN_LENGTH {
            return site;
        }
        let d = (site.b - site.a) / len;
        for (end, offset) in offsets.into_iter().enumerate() {
            if offset.abs() > MIN_LENGTH {
                continue;
            }
            let (q, outward) = if end == 0 { (site.a, -d) } else { (site.b, d) };
            let corner = self
                .attachments
                .iter()
                .filter(|o| {
                    o.parent == site.piece
                        && self.makes_corners(o)
                        && o.along().perp_dot(d).abs() > 1e-6
                })
                .flat_map(|o| {
                    let od = o.along();
                    [o.a + od * o.span[0], o.a + od * o.span[1]].map(|p| (p, o.trim))
                })
                .find(|&(p, trim)| {
                    let v = p - q;
                    let along = v.dot(outward);
                    v.perp_dot(d).abs() <= MIN_LENGTH
                        && along > MIN_LENGTH
                        && along <= trim + MIN_LENGTH
                });
            if let Some((p, _)) = corner {
                if end == 0 {
                    site.a = p;
                } else {
                    site.b = p;
                }
            }
        }
        site
    }

    /// The bends and flats of a profile drawn square to an edge, and how far into the
    /// face the first bend starts.
    ///
    /// `points` are in the edge's profile plane: `x` along the face away from its
    /// material (0 at the edge), `y` through the sheet (0 at the bottom face, the
    /// thickness at the top). The profile starts on the edge, at the bottom or the top
    /// face, and is that face of the sheet carried on: the thickness stays on the same
    /// side of it (above a profile that starts at the bottom, below one that starts at
    /// the top). Its lines are measured to the virtual sharps, and a bend with the body's
    /// radius goes at every corner, including the one at the edge.
    pub fn profile_segments(&self, points: &[DVec2]) -> Result<(Vec<Segment>, f64), String> {
        let t = self.settings.thickness;
        let tol = 1e-6_f64.max(1e-6 * t);
        let Some(&first) = points.first() else {
            return Err("The profile has no lines.".to_owned());
        };
        if points.len() < 2 {
            return Err("The profile has no lines.".to_owned());
        }
        if first.x.abs() > 1e-4 {
            return Err(
                "The profile must start on the edge: draw its first line from the edge's end."
                    .to_owned(),
            );
        }
        let material_above = if first.y.abs() <= 1e-4 {
            true
        } else if (first.y - t).abs() <= 1e-4 {
            false
        } else {
            return Err(
                "The profile must start at the top or the bottom face of the sheet, at the edge."
                    .to_owned(),
            );
        };
        // The face the profile carries on, then the profile's lines.
        let mut dirs = vec![DVec2::X];
        let mut lengths = Vec::new();
        for w in points.windows(2) {
            let d = w[1] - w[0];
            if d.length() <= tol {
                return Err("The profile has a line of zero length.".to_owned());
            }
            dirs.push(d.normalize());
            lengths.push(d.length());
        }
        let mut bends = Vec::new();
        for k in 0..lengths.len() {
            let (d0, d1) = (dirs[k], dirs[k + 1]);
            let cross = d0.perp_dot(d1);
            let angle = cross.abs().atan2(d0.dot(d1));
            if angle < 1e-6 {
                return Err(if k == 0 {
                    "The profile's first line runs along the face: start it at an angle to the face.".to_owned()
                } else {
                    format!(
                        "Lines {} and {} of the profile are in line: join them into one line.",
                        k,
                        k + 1
                    )
                });
            }
            // A left turn heads towards the top side of the sheet.
            let up = cross > 0.0;
            let values = BendValues::new(self.settings.model, angle, self.settings.radius, t)
                .map_err(|m| format!("Corner {}: {m}", k + 1))?;
            // The profile is the outside of the bend when the bend turns towards its
            // material.
            let setback = if up == material_above {
                values.outside_setback()
            } else {
                values.inside_setback()
            };
            bends.push((values, up, setback));
        }
        let mut segments = Vec::new();
        for (k, &len) in lengths.iter().enumerate() {
            let after = bends.get(k + 1).map_or(0.0, |b| b.2);
            let flat = len - bends[k].2 - after;
            if flat <= MIN_LENGTH {
                return Err(format!(
                    "Line {} of the profile is too short for its bends: it needs to be longer than {:.3} mm.",
                    k + 1,
                    bends[k].2 + after
                ));
            }
            segments.push(Segment {
                values: bends[k].0,
                up: bends[k].1,
                flat,
            });
        }
        Ok((segments, bends[0].2))
    }

    /// A flange with the same profile along each edge of a chain (a mitre flange): the
    /// edges are on one face, in order, each starting where the one before it ends.
    /// `offsets` set the flange back from the chain's two ends. Where the edges meet,
    /// the flanges meet in corners that butt their walls, mitre the flats that lie in
    /// one plane, and leave `gap` between them.
    pub fn add_miter_flange(
        &mut self,
        owner: u32,
        sites: &[EdgeSite],
        points: &[DVec2],
        offsets: [f64; 2],
        gap: f64,
    ) -> Result<(), String> {
        if sites.is_empty() {
            return Err("Pick the edges for the flange to run along.".to_owned());
        }
        let parent = sites[0].piece;
        for w in sites.windows(2) {
            if w[1].piece != parent {
                return Err("The edges of a mitre flange must all be on one face.".to_owned());
            }
            if w[0].b.distance(w[1].a) > MIN_LENGTH {
                return Err(
                    "The edges must form a connected chain: each starting where the one before ends."
                        .to_owned(),
                );
            }
        }
        if !(gap.is_finite() && gap >= 0.0) {
            return Err("The gap can't be negative.".to_owned());
        }
        let (segments, trim) = self.profile_segments(points)?;
        let first_corner = self.corners.len();
        let last = sites.len() - 1;
        for (i, site) in sites.iter().enumerate() {
            let off = [
                if i == 0 { offsets[0] } else { 0.0 },
                if i == last { offsets[1] } else { 0.0 },
            ];
            self.add_profile(owner, site, off, trim, &segments, (i as u32) << 8)
                .map_err(|m| format!("Edge {}: {m}", i + 1))?;
        }
        let me = self.attachments.len() - sites.len();
        for c in &mut self.corners[first_corner..] {
            if c.attachments.iter().all(|&a| a >= me) {
                c.spec.gap = gap;
            }
        }
        Ok(())
    }
}

/// A relief slot next to the end of a bend: along the edge over `u`, reaching `depth`
/// into the material. `None` if it has no width.
pub(crate) fn relief(
    kind: ReliefType,
    a: DVec2,
    d: DVec2,
    inward: DVec2,
    u: [f64; 2],
    depth: f64,
    tag: impl Fn(u8) -> CurveTag,
) -> Option<Area> {
    let width = u[1] - u[0];
    if width <= MIN_LENGTH || depth <= MIN_LENGTH {
        return None;
    }
    let p = |x: f64, y: f64| a + d * x + inward * y;
    match kind {
        ReliefType::Rectangular | ReliefType::Tear => Some(rect(
            a,
            d,
            inward,
            u,
            [0.0, depth],
            [tag(0), tag(1), tag(2), tag(3)],
        )),
        ReliefType::Obround => {
            let r = width / 2.0;
            if depth <= r + MIN_LENGTH {
                // Too shallow for a round end: a rectangle.
                return relief(ReliefType::Rectangular, a, d, inward, u, depth, tag);
            }
            let yc = depth - r;
            let center = p(u[0] + r, yc);
            // (d, inward) is counter-clockwise, so the arc from the right side to the left
            // side through the far end runs counter-clockwise.
            let arc = Curve::arc_from_points(center, p(u[1], yc), p(u[0], yc));
            let arc = match arc {
                Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    ..
                } => Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep: PI,
                },
                other => other,
            };
            Some(Area {
                loops: vec![vec![
                    Edge2::line(p(u[0], 0.0), p(u[1], 0.0), tag(0)),
                    Edge2::line(p(u[1], 0.0), p(u[1], yc), tag(1)),
                    Edge2 {
                        curve: arc,
                        reversed: false,
                        tag: tag(2),
                    },
                    Edge2::line(p(u[0], yc), p(u[0], 0.0), tag(3)),
                ]],
            })
        }
    }
}
