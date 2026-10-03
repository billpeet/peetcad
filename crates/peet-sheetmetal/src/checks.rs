//! Manufacturing checks: problems a fabricator would reject, found on a built body.
//!
//! [`check`] looks at a [`SheetBody`] the way a press brake operator or laser
//! programmer would, and returns a list of [`Finding`]s for a checks panel. Every finding
//! says what is wrong, where (a point in flat coordinates for a marker, and the pieces
//! involved), the size found against the size needed, and how to fix it.
//!
//! **What is measured, and where from.** All distances are measured in the flat pattern,
//! which is where the rules of thumb are stated and where the geometry is exact:
//!
//! - *Flange length* is the straight (flat) length of the material next to a bend,
//!   measured square to the bend from its *tangent line* (where the bend region ends and
//!   the flat material starts) to the farthest material of that flange, looking only at
//!   the stretch alongside the bend. Both sides of every bend are measured: an edge
//!   flange's own flange and the face it was added to, and every segment of an open
//!   profile, including the ones between two bends. A flange that is short next to two
//!   bends is reported once, with its shortest length.
//! - *Distance to a bend* is from a hole, or from a run of cut edges on the outside of the
//!   blank, to the bend region (the strip between the bend's two tangent lines, over the
//!   bend's length). A cut that touches or crosses the bend region is taken to cross it
//!   on purpose (a slot across a bend, a hand-made relief) and is not reported. Edges the
//!   features made themselves (bend reliefs, tagged [`CurveTag::Generated`]) are designed
//!   with their bends and are never reported.
//! - *Holes* are the inner loops of the flat pattern. Their distance to the outside of
//!   the blank and to each other is the shortest distance between their edges. A round
//!   hole's size is its diameter; any other hole's size is the smaller side of its
//!   bounding box.
//! - *Collisions* in the folded part are found cheaply: each piece's folded bounding box,
//!   in the fixed flange's coordinates, is compared with every other piece's, except
//!   pieces joined directly by a bend (which always touch). Boxes that overlap by more
//!   than a rule depth in every direction are reported as a possible collision. Boxes are
//!   loose around slanted pieces, so this is a warning, not an error.
//!
//! The flat pattern overlapping itself stops the build ([`SheetError::Overlap`]), so it
//! can't be found on a body; [`error_finding`] turns that error (and any other build
//! error) into a finding, so the checks panel can list it with the rest.
//!
//! **Rules** ([`CheckRules`]) are each `a·t + b·R + c` ([`Rule`]): a multiple of the
//! thickness, plus a multiple of the inner bend radius, plus a constant in mm. The defaults
//! are common rules of thumb from fabricators' design guides (see each field). Shops vary
//! with their tooling and materials, so the rules are settings, not constants. A rule of
//! zero turns its check off.

use std::collections::{HashMap, HashSet};

use peet_math::{DVec2, DVec3, Frame};
use peet_sketch::Curve;
use serde::{Deserialize, Serialize};

use crate::build::{FaceTag, SheetBody, SheetError};
use crate::layout::{Bend, CurveTag, Origin};

/// Lengths closer than this are equal (mm): the arrangement's merging tolerance.
const EPS: f64 = 1e-6;

/// A rule distance: `thickness·t + radius·R + constant` (mm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Multiple of the sheet thickness `t`.
    pub thickness: f64,
    /// Multiple of the inner bend radius `R` (the bend's own radius where there is one,
    /// else the body's default radius).
    pub radius: f64,
    /// A fixed amount in mm.
    pub constant: f64,
}

impl Rule {
    pub const fn new(thickness: f64, radius: f64, constant: f64) -> Self {
        Self {
            thickness,
            radius,
            constant,
        }
    }

    /// The distance for thickness `t` and inner radius `r`.
    pub fn value(&self, t: f64, r: f64) -> f64 {
        self.thickness * t + self.radius * r + self.constant
    }

    /// The rule written out, such as `2.5 × t + R` or `4 × t`.
    pub fn formula(&self) -> String {
        let term = |k: f64, sym: &str| {
            if k == 1.0 {
                sym.to_owned()
            } else {
                format!("{k} × {sym}")
            }
        };
        let mut parts = Vec::new();
        if self.thickness != 0.0 {
            parts.push(term(self.thickness, "t"));
        }
        if self.radius != 0.0 {
            parts.push(term(self.radius, "R"));
        }
        if self.constant != 0.0 || parts.is_empty() {
            parts.push(format!("{} mm", self.constant));
        }
        parts.join(" + ")
    }
}

/// The thresholds of the checks. Each is a [`Rule`]; set one to zero to turn its check
/// off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckRules {
    /// Shortest straight flange next to a bend, from the bend's tangent line.
    ///
    /// Default `4 × t`. Protolabs' sheet metal design guidelines: "Minimum flange length on
    /// sheet metal parts must be at least 4 times the material thickness." It comes from
    /// the press brake die: air bending uses a V-die opening of about 8 × t, and the
    /// flange has to reach across half of it to rest on the die's shoulder. We measure
    /// the straight part only (from the tangent line, not from the outer virtual sharp),
    /// which is a little stricter than the guide and leaves room for the shoulder.
    pub min_flange: Rule,
    /// Shortest distance from a hole or cut to a bend region (from its tangent line).
    ///
    /// Default `2.5 × t + R`, the usual design-guide rule for holes and slots near bends
    /// (for example Protolabs' and SMLease's sheet metal design guides). Closer than this,
    /// the hole sits in the material that stretches as the bend forms, and comes out
    /// oval or pulls the bend line out of true.
    pub hole_to_bend: Rule,
    /// Shortest distance from a hole to the outside edge of the blank.
    ///
    /// Default `2 × t`. Design guides give 1.5 to 2 × t so the web of material does not
    /// bulge or tear when the hole is punched (Protolabs gives fixed values: 1.57 mm for
    /// sheet up to 0.91 mm and 3.18 mm above, which is about 2 × t for common gauges).
    pub hole_to_edge: Rule,
    /// Shortest distance between two holes.
    ///
    /// Default `2 × t`, the common design-guide value, for the same reason as
    /// `hole_to_edge`.
    pub hole_to_hole: Rule,
    /// Smallest hole (diameter, or the narrow side of a slot or other shape).
    ///
    /// Default `1 × t`. Protolabs: "Holes and slots should be a minimum of material
    /// thickness in diameter." Smaller holes break punches and come out rough when laser
    /// cut.
    pub min_hole: Rule,
    /// How deep two folded pieces' bounding boxes may overlap (in every direction) before
    /// they are reported as a possible collision.
    ///
    /// Default `0.5 × t`. Two plates crossing each other overlap by about one thickness in
    /// the thin direction, so the limit has to be below `t`; half of it keeps pieces that
    /// only touch (closed corners, small gaps) quiet.
    pub collision: Rule,
}

impl Default for CheckRules {
    fn default() -> Self {
        Self {
            min_flange: Rule::new(4.0, 0.0, 0.0),
            hole_to_bend: Rule::new(2.5, 1.0, 0.0),
            hole_to_edge: Rule::new(2.0, 0.0, 0.0),
            hole_to_hole: Rule::new(2.0, 0.0, 0.0),
            min_hole: Rule::new(1.0, 0.0, 0.0),
            collision: Rule::new(0.5, 0.0, 0.0),
        }
    }
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// The part can't be made as drawn (or a fabricator will send it back).
    Error,
    /// The part can probably be made, but with poorer quality or extra work.
    Warning,
    /// Worth knowing.
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

/// Which check a finding comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckKind {
    FlangeTooShort,
    HoleNearBend,
    HoleNearEdge,
    HolesTooClose,
    SmallHole,
    /// The flat pattern overlaps itself (from a build error).
    Overlap,
    /// Two pieces of the folded part may run into each other.
    Collision,
    /// Some other build error.
    BuildFailed,
}

impl CheckKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::FlangeTooShort => "Flange too short",
            Self::HoleNearBend => "Hole or cut too close to a bend",
            Self::HoleNearEdge => "Hole too close to an edge",
            Self::HolesTooClose => "Holes too close together",
            Self::SmallHole => "Hole too small",
            Self::Overlap => "Flat pattern overlaps",
            Self::Collision => "Possible collision when folded",
            Self::BuildFailed => "Can't build",
        }
    }
}

/// One problem found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub kind: CheckKind,
    /// What is wrong, where, and how to fix it, in plain English.
    pub message: String,
    /// Where to show a marker, in flat coordinates.
    pub at: Option<DVec2>,
    /// The pieces involved (a piece's origin names its feature).
    pub pieces: Vec<Origin>,
    /// The size measured (mm), if the check measures one.
    pub found: Option<f64>,
    /// The size the rule asks for (mm).
    pub required: Option<f64>,
}

/// Turns a build error into a finding, so a checks panel can list it. Owners are named by
/// `name`, as in [`SheetError::message`].
pub fn error_finding(err: &SheetError, name: impl Fn(u32) -> String) -> Finding {
    let (kind, pieces) = match err {
        SheetError::Overlap { a, b } => (CheckKind::Overlap, vec![*a, *b]),
        SheetError::AcrossBend { tag, bend, .. } => (
            CheckKind::BuildFailed,
            vec![
                Origin {
                    owner: tag.owner(),
                    part: 0,
                },
                *bend,
            ],
        ),
        _ => (CheckKind::BuildFailed, Vec::new()),
    };
    Finding {
        severity: Severity::Error,
        kind,
        message: err.message(name),
        at: None,
        pieces,
        found: None,
        required: None,
    }
}

/// Runs every check on a built body. Owners in messages are named by `name`, as in
/// [`SheetError::message`]. Errors come first, then warnings, then notes.
pub fn check(body: &SheetBody, rules: &CheckRules, name: impl Fn(u32) -> String) -> Vec<Finding> {
    let ctx = Ctx::new(body);
    let mut out = Vec::new();
    ctx.flange_lengths(rules, &name, &mut out);
    ctx.near_bends(rules, &name, &mut out);
    ctx.holes(rules, &name, &mut out);
    ctx.collisions(rules, &name, &mut out);
    out.sort_by_key(|f| f.severity);
    out
}

// ---- The flat pattern, with tags ----

/// A boundary edge of the flat pattern, with the walls on it.
struct BEdge {
    curve: Curve,
    tags: Vec<CurveTag>,
    pieces: Vec<usize>,
}

/// A hole, or a run of cut edges on the outside of the blank.
struct Feature {
    curves: Vec<Curve>,
    pieces: Vec<usize>,
    /// The feature that made it (the first edge not generated by a flange).
    owner: u32,
    hole: bool,
}

struct Ctx<'a> {
    body: &'a SheetBody,
    t: f64,
    /// Outer boundary edges of the blank.
    outer: Vec<Curve>,
    features: Vec<Feature>,
    /// Wall curves of each piece.
    walls: HashMap<usize, Vec<Curve>>,
}

/// Whether `part` lies on `whole` (both ends and the middle).
fn lies_on(part: &Curve, whole: &Curve) -> bool {
    [0.0, 0.5, 1.0]
        .iter()
        .all(|&s| whole.distance(part.point_at(s)) <= EPS)
}

impl<'a> Ctx<'a> {
    fn new(body: &'a SheetBody) -> Self {
        let mut walls: HashMap<usize, Vec<Curve>> = HashMap::new();
        let mut wall_list = Vec::new();
        for f in &body.faces {
            if let FaceTag::Wall {
                piece, tag, curve, ..
            } = f
            {
                walls.entry(*piece).or_default().push(curve.clone());
                wall_list.push((*piece, *tag, curve.clone()));
            }
        }
        // The cut edges a user drew (reliefs are generated and left out).
        let cut_tags: HashSet<CurveTag> = body
            .layout
            .cuts
            .iter()
            .flat_map(|c| c.area.edges())
            .map(|e| e.tag)
            .filter(|t| matches!(t, CurveTag::Sketch { .. } | CurveTag::Copy { .. }))
            .collect();

        let mut outer = Vec::new();
        let mut features = Vec::new();
        for lp in &body.outline {
            let edges: Vec<BEdge> = lp
                .edges
                .iter()
                .map(|(curve, _)| {
                    let mut tags = Vec::new();
                    let mut pieces = Vec::new();
                    for (piece, tag, wc) in &wall_list {
                        if lies_on(wc, curve) {
                            if !tags.contains(tag) {
                                tags.push(*tag);
                            }
                            if !pieces.contains(piece) {
                                pieces.push(*piece);
                            }
                        }
                    }
                    BEdge {
                        curve: curve.clone(),
                        tags,
                        pieces,
                    }
                })
                .collect();
            if lp.outer {
                outer.extend(edges.iter().map(|e| e.curve.clone()));
                let is_cut = |e: &BEdge| e.tags.iter().any(|t| cut_tags.contains(t));
                for run in runs(&edges, is_cut) {
                    if let Some(f) = feature(run.iter().map(|&i| &edges[i]), false) {
                        features.push(f);
                    }
                }
            } else if let Some(f) = feature(edges.iter(), true) {
                features.push(f);
            }
        }
        Self {
            body,
            t: body.layout.settings.thickness,
            outer,
            features,
            walls,
        }
    }

    fn origin(&self, piece: usize) -> Origin {
        self.body.layout.pieces[piece].origin
    }

    fn origins(&self, pieces: impl IntoIterator<Item = usize>) -> Vec<Origin> {
        let mut out: Vec<Origin> = Vec::new();
        for p in pieces {
            let o = self.origin(p);
            if !out.contains(&o) {
                out.push(o);
            }
        }
        out
    }

    // ---- Flange length ----

    fn flange_lengths(
        &self,
        rules: &CheckRules,
        name: &impl Fn(u32) -> String,
        out: &mut Vec<Finding>,
    ) {
        let layout = &self.body.layout;
        // Shortest length per flange piece: (length, needed, bend piece, marker).
        let mut short: Vec<(usize, f64, f64, usize, DVec2)> = Vec::new();
        for (bi, bend) in layout.bends() {
            let need = rules.min_flange.value(self.t, bend.values.radius);
            for child in [false, true] {
                let Some(fi) = (if child { bend.child } else { Some(bend.parent) }) else {
                    continue;
                };
                if !layout.pieces.get(fi).is_some_and(|p| p.is_flange()) {
                    continue;
                }
                let (base, dir) = if child {
                    (bend.origin + bend.across * bend.width(), bend.across)
                } else {
                    (bend.origin, -bend.across)
                };
                let Some(len) = self.reach(fi, base, dir, bend) else {
                    continue;
                };
                if len >= need - EPS {
                    continue;
                }
                let at = base + dir * (len / 2.0) + bend.along * (bend.length / 2.0);
                match short.iter_mut().find(|s| s.0 == fi) {
                    Some(s) if s.1 <= len => {}
                    Some(s) => *s = (fi, len, need, bi, at),
                    None => short.push((fi, len, need, bi, at)),
                }
            }
        }
        for (fi, len, need, bi, at) in short {
            let (flange, bend) = (self.origin(fi), self.origin(bi));
            let is_child = layout.pieces[bi].bend().and_then(|b| b.child) == Some(fi);
            let what = if flange.owner == bend.owner && is_child && flange.part == bend.part {
                format!(
                    "The flange of {} is only {len:.2} mm long past its bend",
                    name(bend.owner)
                )
            } else {
                format!(
                    "The flat face of {} next to the bend of {} is only {len:.2} mm long",
                    name(flange.owner),
                    name(bend.owner)
                )
            };
            out.push(Finding {
                severity: Severity::Error,
                kind: CheckKind::FlangeTooShort,
                message: format!(
                    "{what}; a press brake die needs at least {need:.2} mm ({}) of straight material to hold it. Make it at least {:.2} mm longer (measured from where the bend ends), or leave out the bend.",
                    rules.min_flange.formula(),
                    need - len
                ),
                at: Some(at),
                pieces: self.origins([fi, bi]),
                found: Some(len),
                required: Some(need),
            });
        }
    }

    /// How far the material of flange `fi` reaches from the line through `base` along
    /// the bend, in direction `dir`, alongside the bend.
    fn reach(&self, fi: usize, base: DVec2, dir: DVec2, bend: &Bend) -> Option<f64> {
        let layout = &self.body.layout;
        let mut curves: Vec<Curve> = self.walls.get(&fi).cloned().unwrap_or_default();
        // The flange's edges along bend lines have no walls: add the bend lines.
        for (_, b) in layout.bends() {
            let o = if b.parent == fi {
                b.origin
            } else if b.child == Some(fi) {
                b.origin + b.across * b.width()
            } else {
                continue;
            };
            curves.push(Curve::Line {
                a: o,
                b: o + b.along * b.length,
            });
        }
        let w = |p: DVec2| (p - bend.origin).dot(bend.along);
        let range = -EPS..=bend.length + EPS;
        let mut best: Option<f64> = None;
        let mut take = |p: DVec2| {
            let s = (p - base).dot(dir);
            best = Some(best.map_or(s, |b: f64| b.max(s)));
        };
        for c in &curves {
            match *c {
                Curve::Line { a, b } => {
                    // Clip the line to the stretch alongside the bend.
                    let (wa, wb) = (w(a), w(b));
                    let (mut t0, mut t1) = (0.0f64, 1.0f64);
                    if (wb - wa).abs() <= 1e-12 {
                        if !range.contains(&wa) {
                            continue;
                        }
                    } else {
                        let ta = (range.start() - wa) / (wb - wa);
                        let tb = (range.end() - wa) / (wb - wa);
                        t0 = t0.max(ta.min(tb));
                        t1 = t1.min(ta.max(tb));
                        if t0 > t1 {
                            continue;
                        }
                    }
                    take(a + (b - a) * t0);
                    take(a + (b - a) * t1);
                }
                _ => {
                    for p in c.tessellate(1e-4) {
                        if range.contains(&w(p)) {
                            take(p);
                        }
                    }
                }
            }
        }
        best
    }

    // ---- Holes and cuts near bends ----

    fn near_bends(
        &self,
        rules: &CheckRules,
        name: &impl Fn(u32) -> String,
        out: &mut Vec<Finding>,
    ) {
        for (bi, bend) in self.body.layout.bends() {
            let need = rules.hole_to_bend.value(self.t, bend.values.radius);
            let a = bend.origin;
            let corners = [
                a,
                a + bend.along * bend.length,
                a + bend.along * bend.length + bend.across * bend.width(),
                a + bend.across * bend.width(),
            ];
            let zone: Vec<Curve> = (0..4)
                .map(|i| Curve::Line {
                    a: corners[i],
                    b: corners[(i + 1) % 4],
                })
                .collect();
            let inside = |p: DVec2| {
                let (s, w) = bend.strip_coords(p);
                (0.0..=bend.width()).contains(&s) && (0.0..=bend.length).contains(&w)
            };
            for f in &self.features {
                if f.curves.iter().any(|c| inside(c.point_at(0.0))) {
                    continue; // crosses the bend on purpose
                }
                let Some((d, p, _)) = set_distance(&f.curves, &zone) else {
                    continue;
                };
                if d <= EPS || d >= need - EPS {
                    continue;
                }
                let what = if f.hole {
                    format!("A hole made by {}", name(f.owner))
                } else {
                    format!("A cut edge made by {}", name(f.owner))
                };
                out.push(Finding {
                    severity: Severity::Error,
                    kind: CheckKind::HoleNearBend,
                    message: format!(
                        "{what} is {d:.2} mm from the bend of {}; it needs to be at least {need:.2} mm ({}) away, or it will stretch out of shape when the part is bent. Move it {:.2} mm further from the bend, or extend the cut across the bend.",
                        name(self.origin(bi).owner),
                        rules.hole_to_bend.formula(),
                        need - d
                    ),
                    at: Some(p),
                    pieces: self.origins(f.pieces.iter().copied().chain([bi])),
                    found: Some(d),
                    required: Some(need),
                });
            }
        }
    }

    // ---- Holes: edges, spacing, size ----

    fn holes(&self, rules: &CheckRules, name: &impl Fn(u32) -> String, out: &mut Vec<Finding>) {
        let r = self.body.layout.settings.radius;
        let holes: Vec<&Feature> = self.features.iter().filter(|f| f.hole).collect();
        let (edge_need, gap_need, size_need) = (
            rules.hole_to_edge.value(self.t, r),
            rules.hole_to_hole.value(self.t, r),
            rules.min_hole.value(self.t, r),
        );
        for (i, h) in holes.iter().enumerate() {
            let (lo, hi) = bounds(&h.curves);
            // Size.
            let size = circle_diameter(&h.curves).unwrap_or_else(|| (hi - lo).min_element());
            if size < size_need - EPS {
                out.push(Finding {
                    severity: Severity::Warning,
                    kind: CheckKind::SmallHole,
                    message: format!(
                        "A hole made by {} is {size:.2} mm across, less than the {size_need:.2} mm ({}) that can be punched or laser cut cleanly in this sheet. Make it at least {size_need:.2} mm across, or drill it after forming.",
                        name(h.owner),
                        rules.min_hole.formula()
                    ),
                    at: Some((lo + hi) / 2.0),
                    pieces: self.origins(h.pieces.iter().copied()),
                    found: Some(size),
                    required: Some(size_need),
                });
            }
            // Distance to the outside edge.
            if let Some((d, p, _)) = set_distance(&h.curves, &self.outer)
                && d < edge_need - EPS
            {
                out.push(Finding {
                    severity: Severity::Warning,
                    kind: CheckKind::HoleNearEdge,
                    message: format!(
                        "A hole made by {} is {d:.2} mm from the edge of the sheet; keep at least {edge_need:.2} mm ({}) of material so the edge doesn't bulge or tear. Move it {:.2} mm away from the edge.",
                        name(h.owner),
                        rules.hole_to_edge.formula(),
                        edge_need - d
                    ),
                    at: Some(p),
                    pieces: self.origins(h.pieces.iter().copied()),
                    found: Some(d),
                    required: Some(edge_need),
                });
            }
            // Distance to the other holes.
            for g in &holes[i + 1..] {
                let (glo, ghi) = bounds(&g.curves);
                let gap = (glo - hi).max(lo - ghi).max(DVec2::ZERO).length();
                if gap >= gap_need {
                    continue;
                }
                let Some((d, p, q)) = set_distance(&h.curves, &g.curves) else {
                    continue;
                };
                if d >= gap_need - EPS {
                    continue;
                }
                let who = if h.owner == g.owner {
                    format!("Two holes made by {}", name(h.owner))
                } else {
                    format!(
                        "A hole made by {} and one made by {}",
                        name(h.owner),
                        name(g.owner)
                    )
                };
                out.push(Finding {
                    severity: Severity::Warning,
                    kind: CheckKind::HolesTooClose,
                    message: format!(
                        "{who} are {d:.2} mm apart; leave at least {gap_need:.2} mm ({}) between them so the web between them doesn't distort. Move them {:.2} mm further apart.",
                        rules.hole_to_hole.formula(),
                        gap_need - d
                    ),
                    at: Some((p + q) / 2.0),
                    pieces: self.origins(h.pieces.iter().chain(&g.pieces).copied()),
                    found: Some(d),
                    required: Some(gap_need),
                });
            }
        }
    }

    // ---- Collisions when folded ----

    fn collisions(
        &self,
        rules: &CheckRules,
        name: &impl Fn(u32) -> String,
        out: &mut Vec<Finding>,
    ) {
        let layout = &self.body.layout;
        let limit = rules.collision.value(self.t, layout.settings.radius);
        let Some(fixed) = layout.pieces.first().map(|p| p.frame.inverse()) else {
            return;
        };
        // Each piece folded (in the fixed flange's coordinates): its points, the
        // directions it can be told apart from another piece along, and its flat centre.
        // A flange is a flat prism: its own normal and the normals of its outline's hull
        // separate it exactly from another flat piece (two mitred lips, say). A bend has
        // no such directions and is told apart by its box alone.
        let mut shapes: Vec<Option<Folded>> = Vec::new();
        for (pi, piece) in layout.pieces.iter().enumerate() {
            // The piece's outline as built: the edges of its top faces in the flat solid
            // (which has the corners, reliefs and cuts worked out).
            let mut flat: Vec<DVec2> = Vec::new();
            for (id, tag) in self.body.flat.face_ids().zip(&self.body.faces) {
                if !matches!(tag, FaceTag::Top { piece } if *piece == pi) {
                    continue;
                }
                for &l in &self.body.flat.face(id).loops {
                    for c in self.body.flat.loop_coedges(l) {
                        let e = self.body.flat.edge(self.body.flat.coedge(c).edge);
                        let n = match e.curve {
                            peet_kernel::Curve3::Line(_) => 1,
                            _ => 8,
                        };
                        for k in 0..=n {
                            let q = e.point_at_fraction(f64::from(k) / f64::from(n));
                            flat.push(fixed.to_world(q).truncate());
                        }
                    }
                }
            }
            if flat.is_empty() {
                shapes.push(None);
                continue;
            }
            let frame: Frame = fixed.compose(&piece.frame);
            let lo = flat
                .iter()
                .fold(DVec2::splat(f64::INFINITY), |m, p| m.min(*p));
            let hi = flat
                .iter()
                .fold(DVec2::splat(f64::NEG_INFINITY), |m, p| m.max(*p));
            let mut points = Vec::new();
            let mut axes = Vec::new();
            match piece.bend() {
                Some(b) => {
                    for p in &flat {
                        for z in [0.0, self.t] {
                            points.push(frame.to_world(b.fold_point(p.extend(z))));
                        }
                    }
                }
                None => {
                    let hull = convex_hull(&flat);
                    for (k, p) in hull.iter().enumerate() {
                        for z in [0.0, self.t] {
                            points.push(frame.to_world(p.extend(z)));
                        }
                        let d = hull[(k + 1) % hull.len()] - *p;
                        if d.length() > EPS {
                            let n = DVec2::new(d.y, -d.x).normalize();
                            axes.push(frame.vector_to_world(n.extend(0.0)));
                        }
                    }
                    axes.push(frame.z_axis());
                }
            }
            shapes.push(Some((points, axes, (lo + hi) / 2.0)));
        }
        // How deep two pieces run into each other: the least overlap along any of the
        // directions (and the three axes, which is the box test).
        let depth_of = |a: &Folded, b: &Folded| {
            let extent = |pts: &[DVec3], axis: DVec3| {
                pts.iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                        let d = p.dot(axis);
                        (lo.min(d), hi.max(d))
                    })
            };
            [DVec3::X, DVec3::Y, DVec3::Z]
                .iter()
                .chain(&a.1)
                .chain(&b.1)
                .map(|&axis| {
                    let ((a0, a1), (b0, b1)) = (extent(&a.0, axis), extent(&b.0, axis));
                    a1.min(b1) - a0.max(b0)
                })
                .fold(f64::INFINITY, f64::min)
        };
        let joined = |i: usize, j: usize| {
            layout.bends().any(|(bi, b)| {
                let ends = [Some(b.parent), b.child];
                (bi == i && ends.contains(&Some(j)))
                    || (bi == j && ends.contains(&Some(i)))
                    || (ends.contains(&Some(i)) && ends.contains(&Some(j)))
            })
        };
        let mut seen: Vec<(Origin, Origin)> = Vec::new();
        for (i, si) in shapes.iter().enumerate() {
            let Some(a) = si else { continue };
            for (j, sj) in shapes.iter().enumerate().skip(i + 1) {
                let Some(b) = sj else { continue };
                let at = b.2;
                if joined(i, j) {
                    continue;
                }
                let (oi, oj) = (self.origin(i), self.origin(j));
                let key = if oi <= oj { (oi, oj) } else { (oj, oi) };
                if oi == oj || seen.contains(&key) {
                    continue;
                }
                let depth = depth_of(a, b);
                if depth <= limit + EPS {
                    continue;
                }
                seen.push(key);
                let who = if oi.owner == oj.owner {
                    format!("Two parts of {}", name(oi.owner))
                } else {
                    format!("{} and {}", name(oi.owner), name(oj.owner))
                };
                out.push(Finding {
                    severity: Severity::Warning,
                    kind: CheckKind::Collision,
                    message: format!(
                        "{who} may run into each other when the part is folded: they overlap by {depth:.2} mm. Check the folded part, and shorten or move one of them if they collide."
                    ),
                    at: Some(at),
                    pieces: vec![oi, oj],
                    found: Some(depth),
                    required: Some(limit),
                });
            }
        }
    }
}

/// Runs of consecutive edges (indices, cyclic) for which `keep` holds.
fn runs(edges: &[BEdge], keep: impl Fn(&BEdge) -> bool) -> Vec<Vec<usize>> {
    let n = edges.len();
    let Some(start) = (0..n).find(|&i| !keep(&edges[i])) else {
        return if n > 0 {
            vec![(0..n).collect()]
        } else {
            Vec::new()
        };
    };
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    for k in 1..=n {
        let i = (start + k) % n;
        if keep(&edges[i]) {
            cur.push(i);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    out
}

/// A feature from boundary edges, unless every edge was generated by a flange (a relief).
fn feature<'e>(edges: impl Iterator<Item = &'e BEdge>, hole: bool) -> Option<Feature> {
    let mut curves = Vec::new();
    let mut pieces: Vec<usize> = Vec::new();
    let mut owner = None;
    for e in edges {
        curves.push(e.curve.clone());
        for &p in &e.pieces {
            if !pieces.contains(&p) {
                pieces.push(p);
            }
        }
        if owner.is_none() {
            owner = e
                .tags
                .iter()
                .find(|t| matches!(t, CurveTag::Sketch { .. } | CurveTag::Copy { .. }))
                .map(|t| t.owner());
        }
    }
    Some(Feature {
        curves,
        pieces,
        owner: owner?,
        hole,
    })
}

/// Bounding box of curves.
fn bounds(curves: &[Curve]) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for c in curves {
        let (a, b) = c.bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    (lo, hi)
}

/// The diameter, if the curves make one full circle.
fn circle_diameter(curves: &[Curve]) -> Option<f64> {
    let (c0, r0) = (curves.first()?.center()?, curves.first()?.radius()?);
    let mut sweep = 0.0;
    for c in curves {
        match *c {
            Curve::Circle { center, radius }
                if center.distance(c0) <= EPS && (radius - r0).abs() <= EPS =>
            {
                sweep += std::f64::consts::TAU;
            }
            Curve::Arc {
                center,
                radius,
                sweep: s,
                ..
            } if center.distance(c0) <= EPS && (radius - r0).abs() <= EPS => sweep += s,
            _ => return None,
        }
    }
    ((sweep - std::f64::consts::TAU).abs() <= 1e-6).then_some(2.0 * r0)
}

/// Shortest distance between two sets of curves: `(distance, point on a, point on b)`.
fn set_distance(a: &[Curve], b: &[Curve]) -> Option<(f64, DVec2, DVec2)> {
    let mut best: Option<(f64, DVec2, DVec2)> = None;
    for ca in a {
        for cb in b {
            let d = curve_distance(ca, cb);
            if best.is_none_or(|x| d.0 < x.0) {
                best = Some(d);
            }
        }
    }
    best
}

/// Shortest distance between two bounded curves: `(distance, point on a, point on b)`.
///
/// For curves that don't meet, the closest points are at an end of one of them, or where
/// the joining segment is square to both: through the centre of an arc and square to a
/// line, or along the line between two centres. Every such candidate is tried.
fn curve_distance(a: &Curve, b: &Curve) -> (f64, DVec2, DVec2) {
    if let Some(x) = a.intersect(b).first() {
        return (0.0, x.point, x.point);
    }
    let mut cands: Vec<(DVec2, DVec2)> = Vec::new();
    for p in ends(a) {
        cands.push((p, b.closest_point(p).1));
    }
    for q in ends(b) {
        cands.push((a.closest_point(q).1, q));
    }
    let round = |c: &Curve| c.center().zip(c.radius());
    // Points of a round curve in direction ±n from its centre, where they are on it.
    let toward = |c: &Curve, n: DVec2| -> Vec<DVec2> {
        let Some((center, r)) = round(c) else {
            return Vec::new();
        };
        let Some(n) = n.try_normalize() else {
            return Vec::new();
        };
        [center + n * r, center - n * r]
            .into_iter()
            .filter(|&p| c.contains_param(c.project(p)))
            .collect()
    };
    match (a, b) {
        (Curve::Line { a: p, b: q }, _) if round(b).is_some() => {
            let n = (*q - *p).perp();
            for s in toward(b, n) {
                cands.push((a.closest_point(s).1, s));
            }
            let f = a.closest_point(b.center().unwrap_or(*p)).1;
            cands.push((f, b.closest_point(f).1));
        }
        (_, Curve::Line { a: p, b: q }) if round(a).is_some() => {
            let n = (*q - *p).perp();
            for s in toward(a, n) {
                cands.push((s, b.closest_point(s).1));
            }
            let f = b.closest_point(a.center().unwrap_or(*p)).1;
            cands.push((a.closest_point(f).1, f));
        }
        _ => {
            if let (Some((ca, _)), Some((cb, _))) = (round(a), round(b)) {
                for s in toward(a, cb - ca) {
                    cands.push((s, b.closest_point(s).1));
                }
                for s in toward(b, cb - ca) {
                    cands.push((a.closest_point(s).1, s));
                }
            }
        }
    }
    cands
        .into_iter()
        .map(|(p, q)| (p.distance(q), p, q))
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap_or((f64::INFINITY, DVec2::ZERO, DVec2::ZERO))
}

/// The ends of an open curve (a circle has none; its start stands in).
fn ends(c: &Curve) -> Vec<DVec2> {
    match c {
        Curve::Circle { .. } => vec![c.point_at(0.0)],
        _ => vec![c.start(), c.end()],
    }
}

/// A piece as folded: its points, the directions it can be told apart from another
/// piece along, and its centre in the flat pattern.
type Folded = (Vec<DVec3>, Vec<DVec3>, DVec2);

/// The convex hull of points, counter-clockwise (Andrew's monotone chain).
fn convex_hull(points: &[DVec2]) -> Vec<DVec2> {
    let mut pts: Vec<DVec2> = points.to_vec();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance(*b) <= EPS);
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<DVec2> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &DVec2>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2 {
                let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
                if (b - a).perp_dot(p - b) <= EPS {
                    hull.pop();
                } else {
                    break;
                }
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build;
    use crate::flange::EdgeFlangeSpec;
    use crate::layout::{Area, ChainLine, Edge2, EdgeSite, Layout};
    use crate::settings::{BendModel, FlangePosition, ReliefType, SheetSettings};
    use peet_math::Plane;
    use peet_sketch::region::find_regions;
    use peet_sketch::{Sketch, shapes};

    const W: f64 = 100.0;
    const H: f64 = 60.0;
    const T: f64 = 2.0;
    const R: f64 = 3.0;

    fn settings(k: f64) -> SheetSettings {
        SheetSettings {
            thickness: T,
            radius: R,
            model: BendModel::KFactor(k),
            relief: ReliefType::Rectangular,
            relief_ratio: 0.5,
        }
    }

    fn plate() -> Layout {
        let mut sk = Sketch::new();
        shapes::rectangle(&mut sk, DVec2::ZERO, DVec2::new(W, H));
        let regions = find_regions(&sk).regions;
        Layout::plate(settings(0.44), 1, &Plane::TOP, &regions, false).unwrap()
    }

    fn flange(length: f64) -> EdgeFlangeSpec {
        EdgeFlangeSpec {
            length,
            angle: 90.0,
            position: FlangePosition::MaterialInside,
            offsets: [0.0, 0.0],
            flip: false,
            radius: None,
        }
    }

    /// The plate's edge at `y = H`.
    fn top_edge() -> EdgeSite {
        EdgeSite {
            piece: 0,
            a: DVec2::new(W, H),
            b: DVec2::new(0.0, H),
            top: true,
        }
    }

    fn hole(center: DVec2, radius: f64, owner: u32) -> Area {
        Area {
            loops: vec![vec![Edge2 {
                curve: Curve::Circle { center, radius },
                reversed: false,
                tag: CurveTag::Sketch { owner, entity: 1 },
            }]],
        }
    }

    fn rect(lo: DVec2, hi: DVec2, owner: u32) -> Area {
        Area::polygon(
            &[lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)],
            |i| CurveTag::Sketch {
                owner,
                entity: i as u32,
            },
        )
    }

    #[track_caller]
    fn run(layout: Layout) -> Vec<Finding> {
        let (_, body) = match build(layout) {
            Ok(b) => b,
            Err(e) => panic!("build failed: {}", e.message(|o| format!("F{o}"))),
        };
        check(&body, &CheckRules::default(), |o| format!("F{o}"))
    }

    fn of(findings: &[Finding], kind: CheckKind) -> Vec<&Finding> {
        findings.iter().filter(|f| f.kind == kind).collect()
    }

    #[test]
    fn short_flange_is_flagged_and_a_long_one_is_not() {
        let mut layout = plate();
        // Outside length 9, setback R + t = 5: 4 mm of straight flange, less than 4·t = 8.
        layout
            .add_edge_flange(2, &top_edge(), &flange(9.0))
            .unwrap();
        let found = run(layout);
        let short = of(&found, CheckKind::FlangeTooShort);
        assert_eq!(short.len(), 1, "{found:#?}");
        let f = short[0];
        assert_eq!(f.severity, Severity::Error);
        assert!((f.found.unwrap() - 4.0).abs() < 1e-9, "{f:?}");
        assert!((f.required.unwrap() - 8.0).abs() < 1e-9);
        assert!(f.pieces.contains(&Origin { owner: 2, part: 0 }));
        assert!(
            f.message.contains("F2") && f.message.contains("4.00 mm longer"),
            "{}",
            f.message
        );

        let mut layout = plate();
        layout
            .add_edge_flange(2, &top_edge(), &flange(20.0))
            .unwrap();
        let found = run(layout);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn short_profile_segments_are_flagged() {
        let s = settings(0.5);
        let line = |entity, a: DVec2, b: DVec2| ChainLine { entity, a, b };
        // An L: the 10 mm leg has 10 − 5 = 5 mm of straight material.
        let chain = [
            line(1, DVec2::new(0.0, 10.0), DVec2::ZERO),
            line(2, DVec2::ZERO, DVec2::new(60.0, 0.0)),
        ];
        let layout =
            Layout::open_profile(s, 1, &Plane::front(), &chain, [0.0, 50.0], false).unwrap();
        let found = run(layout);
        let short = of(&found, CheckKind::FlangeTooShort);
        assert_eq!(short.len(), 1, "{found:#?}");
        assert!((short[0].found.unwrap() - 5.0).abs() < 1e-9);
        assert!(short[0].pieces.contains(&Origin { owner: 1, part: 1 }));
        // A narrow U: the base between two bends has 14 − 2·5 = 4 mm, reported once.
        let chain = [
            line(1, DVec2::new(0.0, 30.0), DVec2::ZERO),
            line(2, DVec2::ZERO, DVec2::new(14.0, 0.0)),
            line(3, DVec2::new(14.0, 0.0), DVec2::new(14.0, 30.0)),
        ];
        let layout =
            Layout::open_profile(s, 1, &Plane::front(), &chain, [0.0, 50.0], false).unwrap();
        let found = run(layout);
        let short = of(&found, CheckKind::FlangeTooShort);
        assert_eq!(short.len(), 1, "{found:#?}");
        assert!((short[0].found.unwrap() - 4.0).abs() < 1e-9);
        assert!(short[0].pieces.contains(&Origin { owner: 1, part: 2 }));
    }

    #[test]
    fn hole_near_a_bend_is_flagged_with_its_distance() {
        let mut layout = plate();
        layout
            .add_edge_flange(2, &top_edge(), &flange(20.0))
            .unwrap();
        // The bend region starts R + t = 5 below the edge, at y = 55. The near hole's top
        // is at y = 52: 3 mm away, less than 2.5·t + R = 8.
        layout.add_cut(hole(DVec2::new(30.0, 50.0), 2.0, 5));
        layout.add_cut(hole(DVec2::new(70.0, 20.0), 2.0, 6));
        let found = run(layout);
        let near = of(&found, CheckKind::HoleNearBend);
        assert_eq!(near.len(), 1, "{found:#?}");
        let f = near[0];
        assert!((f.found.unwrap() - 3.0).abs() < 1e-9, "{f:?}");
        assert!((f.required.unwrap() - 8.0).abs() < 1e-9);
        assert!(f.at.unwrap().abs_diff_eq(DVec2::new(30.0, 52.0), 1e-9));
        assert!(
            f.message.contains("F5") && f.message.contains("F2"),
            "{}",
            f.message
        );
        assert!(f.pieces.contains(&Origin { owner: 1, part: 0 }));
        assert!(f.pieces.contains(&Origin { owner: 2, part: 0 }));
        assert_eq!(found.len(), 1, "{found:#?}");
    }

    #[test]
    fn cut_edge_near_a_bend_is_flagged() {
        let mut layout = plate();
        layout
            .add_edge_flange(2, &top_edge(), &flange(20.0))
            .unwrap();
        // A notch in the side of the plate, its top edge 3 mm below the bend region.
        layout.add_cut(rect(DVec2::new(95.0, 48.0), DVec2::new(105.0, 52.0), 5));
        let found = run(layout);
        let near = of(&found, CheckKind::HoleNearBend);
        assert_eq!(near.len(), 1, "{found:#?}");
        assert!((near[0].found.unwrap() - 3.0).abs() < 1e-9);
        assert!(near[0].message.starts_with("A cut edge made by F5"));
    }

    #[test]
    fn reliefs_and_a_good_enclosure_pass() {
        // Four flanges set back from the corners, with reliefs: nothing to report.
        let edges = [
            (DVec2::new(0.0, 0.0), DVec2::new(W, 0.0)),
            (DVec2::new(W, 0.0), DVec2::new(W, H)),
            (DVec2::new(W, H), DVec2::new(0.0, H)),
            (DVec2::new(0.0, H), DVec2::new(0.0, 0.0)),
        ];
        for relief in ReliefType::ALL {
            let mut layout = plate();
            layout.settings.relief = relief;
            let mut spec = flange(20.0);
            spec.offsets = [10.0, 10.0];
            for (i, (a, b)) in edges.into_iter().enumerate() {
                let site = EdgeSite {
                    piece: 0,
                    a,
                    b,
                    top: true,
                };
                layout.add_edge_flange(10 + i as u32, &site, &spec).unwrap();
            }
            let found = run(layout);
            assert!(found.is_empty(), "{relief:?}: {found:#?}");
        }
    }

    #[test]
    fn slot_across_a_bend_is_not_too_close() {
        let mut layout = plate();
        layout.settings.model = BendModel::KFactor(0.5);
        layout
            .add_edge_flange(2, &top_edge(), &flange(25.0))
            .unwrap();
        layout.add_cut(rect(DVec2::new(40.0, 45.0), DVec2::new(50.0, 70.0), 3));
        let found = run(layout);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn small_holes_and_holes_near_edges_or_each_other() {
        let mut layout = plate();
        layout.add_cut(hole(DVec2::new(20.0, 30.0), 0.5, 3)); // 1 mm across, t = 2
        layout.add_cut(hole(DVec2::new(3.0, 30.0), 1.0, 4)); // 2 mm from the edge
        layout.add_cut(hole(DVec2::new(50.0, 30.0), 2.0, 5));
        layout.add_cut(hole(DVec2::new(55.0, 30.0), 2.0, 6)); // 1 mm from the last
        let found = run(layout);
        let small = of(&found, CheckKind::SmallHole);
        assert_eq!(small.len(), 1, "{found:#?}");
        assert!((small[0].found.unwrap() - 1.0).abs() < 1e-9);
        let edge = of(&found, CheckKind::HoleNearEdge);
        assert_eq!(edge.len(), 1, "{found:#?}");
        assert!((edge[0].found.unwrap() - 2.0).abs() < 1e-9);
        let close = of(&found, CheckKind::HolesTooClose);
        assert_eq!(close.len(), 1, "{found:#?}");
        assert!((close[0].found.unwrap() - 1.0).abs() < 1e-9);
        assert!(close[0].message.contains("F5") && close[0].message.contains("F6"));
        assert_eq!(found.len(), 3, "{found:#?}");
        assert!(found.iter().all(|f| f.severity == Severity::Warning));
    }

    #[test]
    fn folded_collision_is_a_warning() {
        let mut layout = plate();
        layout
            .add_edge_flange(2, &top_edge(), &flange(20.0))
            .unwrap();
        let (solid, body) = build(layout.clone()).unwrap();
        // The top flange's tip edge, on the side facing the plate.
        let site = solid
            .edge_ids()
            .filter_map(|e| body.edge_site(&solid, e).ok())
            .find(|s| {
                let fl = &body.layout.pieces[2];
                let (p, q) = (
                    fl.frame.to_world(s.a.extend(0.0)),
                    fl.frame.to_world(s.b.extend(0.0)),
                );
                s.piece == 2 && s.top && (p.z - 20.0).abs() < 1e-9 && (q.z - 20.0).abs() < 1e-9
            })
            .unwrap();
        let bottom = EdgeSite {
            piece: 0,
            a: DVec2::ZERO,
            b: DVec2::new(W, 0.0),
            top: true,
        };
        layout.add_edge_flange(3, &bottom, &flange(20.0)).unwrap();
        // A return flange over the plate: 30 mm clears the far flange, 70 mm runs
        // through it.
        let mut good = layout.clone();
        good.add_edge_flange(4, &site, &flange(30.0)).unwrap();
        let found = run(good);
        assert!(found.is_empty(), "{found:#?}");
        layout.add_edge_flange(4, &site, &flange(70.0)).unwrap();
        let found = run(layout);
        let hits = of(&found, CheckKind::Collision);
        assert_eq!(hits.len(), 1, "{found:#?}");
        assert_eq!(hits[0].severity, Severity::Warning);
        let mut owners: Vec<u32> = hits[0].pieces.iter().map(|o| o.owner).collect();
        owners.sort_unstable();
        assert_eq!(owners, [3, 4]);
    }

    #[test]
    fn build_errors_become_findings() {
        let mut layout = plate();
        let mut spec = flange(20.0);
        spec.position = FlangePosition::BendOutside;
        layout.add_edge_flange(2, &top_edge(), &spec).unwrap();
        layout.add_edge_flange(3, &top_edge(), &spec).unwrap();
        let err = build(layout).unwrap_err();
        let f = error_finding(&err, |o| format!("F{o}"));
        assert_eq!(f.kind, CheckKind::Overlap);
        assert_eq!(f.severity, Severity::Error);
        assert_eq!(f.pieces.len(), 2);
        assert!(
            f.message.contains("F2") && f.message.contains("F3"),
            "{}",
            f.message
        );
    }

    #[test]
    fn distances_between_curves() {
        let line = Curve::Line {
            a: DVec2::new(-10.0, 0.0),
            b: DVec2::new(10.0, 0.0),
        };
        let circle = Curve::Circle {
            center: DVec2::new(1.0, 5.0),
            radius: 2.0,
        };
        let (d, p, q) = curve_distance(&circle, &line);
        assert!((d - 3.0).abs() < 1e-12, "{d}");
        assert!(p.abs_diff_eq(DVec2::new(1.0, 3.0), 1e-12));
        assert!(q.abs_diff_eq(DVec2::new(1.0, 0.0), 1e-12));
        let (d, ..) = curve_distance(&line, &circle);
        assert!((d - 3.0).abs() < 1e-12, "{d}");
        // Past the end of the line.
        let far = Curve::Circle {
            center: DVec2::new(14.0, 3.0),
            radius: 1.0,
        };
        let (d, ..) = curve_distance(&line, &far);
        assert!((d - 4.0).abs() < 1e-12, "{d}");
        // Two circles.
        let (d, ..) = curve_distance(&circle, &far);
        assert!(
            (d - (DVec2::new(13.0, -2.0).length() - 3.0)).abs() < 1e-12,
            "{d}"
        );
        // Crossing.
        let (d, ..) = curve_distance(
            &line,
            &Curve::Circle {
                center: DVec2::ZERO,
                radius: 1.0,
            },
        );
        assert_eq!(d, 0.0);
    }

    #[test]
    fn rule_formulas() {
        assert_eq!(Rule::new(2.5, 1.0, 0.0).formula(), "2.5 × t + R");
        assert_eq!(Rule::new(4.0, 0.0, 0.0).formula(), "4 × t");
        assert_eq!(Rule::new(0.0, 0.0, 3.0).formula(), "3 mm");
        assert_eq!(Rule::new(1.0, 0.0, 0.5).formula(), "t + 0.5 mm");
        assert!((Rule::new(2.5, 1.0, 0.5).value(2.0, 3.0) - 8.5).abs() < 1e-12);
    }
}
