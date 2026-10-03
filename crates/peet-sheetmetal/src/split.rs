//! Bends along a line drawn on a flange: sketched bends and jogs.
//!
//! A sketched bend splits a flange in two along a line. One side stays where it is (the
//! *fixed* side); the other becomes a new flange that folds about the line. In the flat
//! pattern nothing moves: the flange keeps its outline, gives up everything past the
//! start of the bend (a trim), and the bend and the new flange take their share of the
//! same outline through trims of their own. So holes, notches and the outline's shape
//! carry over exactly, and the cuts that applied to the flange apply to both new pieces.
//!
//! The fixed side is the side the flange is attached on (where its own bend joins it).
//! The first flange of a part isn't attached to anything; there the larger side stays,
//! unless the bend says otherwise.
//!
//! Everything attached to the flange on the moving side (edge flanges, hems, other
//! bends) moves with it: it is re-parented onto the new flange and its placement is
//! composed with the new fold. Anything attached across the line is an error.
//!
//! A jog is two sketched bends in opposite directions with a short flat between them,
//! sized so the far side ends up offset by a given distance.

use std::collections::HashMap;

use peet_math::DVec2;

use crate::layout::{
    Area, Bend, CurveTag, Layout, MIN_LENGTH, Origin, Piece, PieceKind, loop_winding, rect,
};
use crate::settings::BendValues;

/// Where a sketched bend sits relative to its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BendLinePosition {
    /// The line is the middle of the bend.
    Centerline,
    /// The line is where the outer faces of the two sides meet (the outer virtual sharp).
    MaterialInside,
    /// The line is where the inner faces meet (the inner virtual sharp).
    MaterialOutside,
    /// The bend starts at the line, on the moving side.
    BendOutside,
}

impl BendLinePosition {
    pub const ALL: [Self; 4] = [
        Self::Centerline,
        Self::MaterialInside,
        Self::MaterialOutside,
        Self::BendOutside,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Centerline => "Bend centre line",
            Self::MaterialInside => "Material inside",
            Self::MaterialOutside => "Material outside",
            Self::BendOutside => "Bend outside",
        }
    }

    /// Where the bend starts, measured from the line towards the moving side.
    fn start(self, v: &BendValues) -> f64 {
        match self {
            Self::Centerline => -v.allowance / 2.0,
            Self::MaterialInside => -v.outside_setback(),
            Self::MaterialOutside => -v.inside_setback(),
            Self::BendOutside => 0.0,
        }
    }
}

/// A sketched bend's values, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SketchedBendSpec {
    /// Bend angle in degrees.
    pub angle: f64,
    /// Inner radius, if not the body's default.
    pub radius: Option<f64>,
    pub position: BendLinePosition,
    /// Fold towards the top side of the sheet.
    pub up: bool,
    /// On the part's first flange: keep the other side fixed.
    pub flip_fixed: bool,
}

/// How a jog's offset is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum JogDimension {
    /// Between the same faces of the two sides (bottom to bottom): how far the far side
    /// moves.
    Overall,
    /// Between the facing faces: the clear height inside the step.
    Inside,
    /// Between the outermost faces of the step.
    Outside,
}

impl JogDimension {
    pub const ALL: [Self; 3] = [Self::Overall, Self::Inside, Self::Outside];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overall => "Face to same face",
            Self::Inside => "Inside",
            Self::Outside => "Outside",
        }
    }
}

/// A jog's values, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JogSpec {
    /// How far the far side is offset, measured as `dimension` says.
    pub offset: f64,
    pub dimension: JogDimension,
    /// Angle of both bends, in degrees (90 for a square step).
    pub angle: f64,
    pub radius: Option<f64>,
    /// Where the first bend sits relative to the line.
    pub position: BendLinePosition,
    /// The far side steps towards the top side of the sheet.
    pub up: bool,
    pub flip_fixed: bool,
}

/// The pieces a sketched bend made: the bend and the flange that moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub bend: usize,
    pub child: usize,
}

impl Layout {
    /// Bends flange `piece` along the line through `a` and `b` (flat coordinates).
    pub fn add_sketched_bend(
        &mut self,
        owner: u32,
        part: u32,
        piece: usize,
        line: [DVec2; 2],
        spec: &SketchedBendSpec,
    ) -> Result<Split, String> {
        let radius = spec.radius.unwrap_or(self.settings.radius);
        if !(radius.is_finite() && radius >= 0.0) {
            return Err("The bend radius can't be negative.".to_owned());
        }
        let values = BendValues::new(
            self.settings.model,
            spec.angle.to_radians(),
            radius,
            self.settings.thickness,
        )?;
        let normal = self.moving_side(piece, line, spec.flip_fixed)?;
        let start = spec.position.start(&values);
        self.split(owner, part, piece, line[0], normal, start, values, spec.up)
    }

    /// A jog on flange `piece` along the line through `a` and `b`.
    pub fn add_jog(
        &mut self,
        owner: u32,
        piece: usize,
        line: [DVec2; 2],
        spec: &JogSpec,
    ) -> Result<[Split; 2], String> {
        let t = self.settings.thickness;
        let radius = spec.radius.unwrap_or(self.settings.radius);
        if !(radius.is_finite() && radius >= 0.0) {
            return Err("The bend radius can't be negative.".to_owned());
        }
        if !spec.offset.is_finite() {
            return Err("The offset is not a number.".to_owned());
        }
        let values = BendValues::new(self.settings.model, spec.angle.to_radians(), radius, t)?;
        // How far the far side has to move, face to same face.
        let wanted = match spec.dimension {
            JogDimension::Overall => spec.offset,
            JogDimension::Inside => spec.offset + t,
            JogDimension::Outside => spec.offset - t,
        };
        let normal = self.moving_side(piece, line, spec.flip_fixed)?;
        let s0 = spec.position.start(&values);
        // The offset grows with the flat between the bends at sin(angle) per unit: work
        // out the offset with no flat, then the flat that gives the one asked for.
        let rise = |flat: f64| -> f64 {
            let first = jog_bend(line[0], normal, s0, values, spec.up);
            let second = jog_bend(
                line[0],
                normal,
                s0 + values.allowance + flat,
                values,
                !spec.up,
            );
            let far = first.fold_frame().compose(&second.fold_frame());
            let p = second.origin + second.across * (values.allowance + 1.0);
            far.to_world(p.extend(0.0)).z.abs()
        };
        let (h0, h1) = (rise(0.0), rise(1.0));
        let flat = (wanted - h0) / (h1 - h0);
        if flat.is_nan() || flat <= MIN_LENGTH {
            let least = match spec.dimension {
                JogDimension::Overall => h0,
                JogDimension::Inside => h0 - t,
                JogDimension::Outside => h0 + t,
            };
            return Err(format!(
                "The offset is too small for two bends: with this radius and angle it must be more than {least:.3} mm."
            ));
        }
        let first = self.split(owner, 0, piece, line[0], normal, s0, values, spec.up)?;
        let second = self.split(
            owner,
            1,
            first.child,
            line[0],
            normal,
            s0 + values.allowance + flat,
            values,
            !spec.up,
        )?;
        Ok([first, second])
    }

    /// The unit normal of the line pointing to the side of `piece` that moves.
    fn moving_side(&self, piece: usize, line: [DVec2; 2], flip: bool) -> Result<DVec2, String> {
        let p = self
            .pieces
            .get(piece)
            .ok_or("The face is not on this body.")?;
        if !p.is_flange() {
            return Err("Sketched bends go on flat faces, not on bends.".to_owned());
        }
        let d = (line[1] - line[0]).normalize_or_zero();
        if line[0].distance(line[1]) <= MIN_LENGTH || d == DVec2::ZERO {
            return Err("The bend line has no length.".to_owned());
        }
        let left = DVec2::new(-d.y, d.x);
        let side = |q: DVec2| (q - line[0]).dot(left);
        // Attached through its own bend: that side stays.
        let attached = self.bends().find(|(_, b)| b.child == Some(piece));
        let normal = if let Some((_, bend)) = attached {
            let w = bend.width();
            let ends = [
                bend.origin + bend.across * w,
                bend.origin + bend.across * w + bend.along * bend.length,
            ];
            let (s0, s1) = (side(ends[0]), side(ends[1]));
            if s0 * s1 < 0.0 && s0.abs().min(s1.abs()) > MIN_LENGTH {
                return Err(
                    "The bend line crosses the bend this face hangs from. Draw it clear of that bend."
                        .to_owned(),
                );
            }
            let stay = if (s0 + s1).abs() > MIN_LENGTH {
                s0 + s1
            } else {
                1.0
            };
            if flip {
                return Err(
                    "Only the part's first face can choose which side stays fixed: this face stays fixed where it hangs from its bend. Flip the bend direction instead."
                        .to_owned(),
                );
            }
            if stay > 0.0 { -left } else { left }
        } else {
            // The larger side stays.
            let area = |sign: f64| clipped_area(&p.outline, line[0], left * sign);
            let left_bigger = area(1.0) >= area(-1.0);
            let moving_left = !left_bigger != flip;
            if moving_left { left } else { -left }
        };
        Ok(normal)
    }

    /// Splits `piece` with a bend whose strip starts `start` past the line through `at`
    /// (along `normal`, towards the moving side).
    #[allow(clippy::too_many_arguments)]
    fn split(
        &mut self,
        owner: u32,
        part: u32,
        piece: usize,
        at: DVec2,
        normal: DVec2,
        start: f64,
        values: BendValues,
        up: bool,
    ) -> Result<Split, String> {
        let p = &self.pieces[piece];
        let along = DVec2::new(normal.y, -normal.x);
        // Everything of the piece, generously: the strip and the trims reach past it.
        let (lo, hi) = area_bounds(&p.outline);
        let size = lo.distance(hi) + 1.0;
        let base = at + normal * start;
        let corners = [lo, hi, DVec2::new(lo.x, hi.y), DVec2::new(hi.x, lo.y)];
        let u = |q: DVec2| (q - base).dot(along);
        let u0 = corners.iter().map(|&c| u(c)).fold(f64::INFINITY, f64::min) - 1.0;
        let u1 = corners
            .iter()
            .map(|&c| u(c))
            .fold(f64::NEG_INFINITY, f64::max)
            + 1.0;
        let ba = values.allowance;
        let tag = |index: u8| CurveTag::Generated {
            owner,
            part,
            index: SPLIT_TAG + index,
        };
        let tags = |k: u8| [tag(k), tag(k + 1), tag(k + 2), tag(k + 3)];
        let band = |s: [f64; 2], k: u8| rect(base, along, normal, [u0, u1], s, tags(k));
        let before = band([-size, 0.0], 0);
        let after_start = band([0.0, size + ba], 4);
        let after_bend = band([ba, size + ba], 8);

        // Is there material on both sides of the bend?
        let has = |sign: f64, offset: f64| {
            clipped_area(&p.outline, base + normal * offset, normal * sign) > MIN_LENGTH
        };
        if !has(-1.0, 0.0) || !has(1.0, ba) {
            return Err(
                "The bend line doesn't cross the face: draw it right across the face.".to_owned(),
            );
        }

        let bend = Bend {
            parent: piece,
            child: Some(self.pieces.len() + 1),
            origin: base + along * u0,
            along,
            across: normal,
            length: u1 - u0,
            values,
            up,
        };
        // Bends hanging from the piece: which side are they on?
        let s = |q: DVec2| (q - base).dot(normal);
        let mut moving = Vec::new();
        for (i, b) in self.bends() {
            if b.parent != piece {
                continue;
            }
            let ends = [b.origin, b.origin + b.along * b.length];
            let (a0, a1) = (s(ends[0]), s(ends[1]));
            if a0.max(a1) <= MIN_LENGTH {
                continue;
            }
            if a0.min(a1) >= ba - MIN_LENGTH {
                moving.push(i);
                continue;
            }
            return Err(
                "The bend runs into a flange or bend attached to this face. Move the line clear of it."
                    .to_owned(),
            );
        }

        // Forms on the piece go with their side; one the bend runs through can't be.
        let mut moving_forms = Vec::new();
        for (i, f) in self.forms.iter().enumerate() {
            if f.piece != piece {
                continue;
            }
            let reach = match &f.shape {
                crate::form::FormShape::Circle { center, radius } => {
                    [s(*center) - radius, s(*center) + radius]
                }
                crate::form::FormShape::Polygon(p) => p
                    .iter()
                    .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], &q| {
                        [lo.min(s(q)), hi.max(s(q))]
                    }),
            };
            if reach[1] <= 0.0 {
                continue;
            }
            if reach[0] >= ba {
                moving_forms.push(i);
                continue;
            }
            return Err(format!(
                "The bend runs through a {}. Move the line clear of it.",
                f.kind.label().to_lowercase()
            ));
        }

        let parent = self.pieces[piece].clone();
        let bend_index = self.pieces.len();
        let child_index = bend_index + 1;
        let child_frame = parent.frame.compose(&bend.fold_frame());
        let mut bend_trims = parent.trims.clone();
        bend_trims.extend([before.clone(), after_bend.clone()]);
        let mut child_trims = parent.trims.clone();
        child_trims.push(band([-size, ba], 12));
        self.pieces.push(Piece {
            origin: Origin { owner, part },
            kind: PieceKind::Bend(bend),
            outline: parent.outline.clone(),
            trims: bend_trims,
            frame: parent.frame,
        });
        self.pieces.push(Piece {
            origin: Origin { owner, part },
            kind: PieceKind::Flange,
            outline: parent.outline.clone(),
            trims: child_trims,
            frame: child_frame,
        });
        self.pieces[piece].trims.push(after_start);
        for cut in &mut self.cuts {
            if cut.applies_to(piece) {
                cut.pieces.extend([bend_index, child_index]);
            }
        }

        // Move what hangs from the moving side.
        let motion = child_frame.compose(&parent.frame.inverse());
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, b) in self.bends() {
            children.entry(b.parent).or_default().push(i);
        }
        let mut stack = moving.clone();
        while let Some(i) = stack.pop() {
            let frame = self.pieces[i].frame;
            self.pieces[i].frame = motion.compose(&frame);
            if let Some(c) = self.pieces[i].bend().and_then(|b| b.child) {
                let frame = self.pieces[c].frame;
                self.pieces[c].frame = motion.compose(&frame);
                stack.extend(children.get(&c).into_iter().flatten().copied());
            }
        }
        for &i in &moving {
            if let PieceKind::Bend(b) = &mut self.pieces[i].kind {
                b.parent = child_index;
            }
        }
        for i in moving_forms {
            self.forms[i].piece = child_index;
        }
        for a in &mut self.attachments {
            if a.parent == piece && a.pieces.first().is_some_and(|f| moving.contains(f)) {
                a.parent = child_index;
            }
        }
        Ok(Split {
            bend: bend_index,
            child: child_index,
        })
    }
}

/// Wall numbers of the strips a sketched bend adds (they are never walls of the part:
/// they only ever run inside the face or outside the sheet).
const SPLIT_TAG: u8 = 32;

/// One of a jog's bends, laid out as [`Layout::split`] would.
fn jog_bend(at: DVec2, normal: DVec2, start: f64, values: BendValues, up: bool) -> Bend {
    Bend {
        parent: 0,
        child: None,
        origin: at + normal * start,
        along: DVec2::new(normal.y, -normal.x),
        across: normal,
        length: 1.0,
        values,
        up,
    }
}

/// The bounding box of an area's curves.
fn area_bounds(area: &Area) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in area.edges() {
        let (a, b) = e.curve.bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    (lo, hi)
}

/// The area of `area` on the side of the line through `at` that `normal` points to
/// (arcs are approximated by short chords). Loops inside an odd number of others are
/// holes.
fn clipped_area(area: &Area, at: DVec2, normal: DVec2) -> f64 {
    let mut total = 0.0;
    for (k, l) in area.loops.iter().enumerate() {
        let mut poly: Vec<DVec2> = Vec::new();
        for e in l {
            let n = match e.curve {
                peet_sketch::Curve::Line { .. } => 1,
                _ => 32,
            };
            for i in 0..n {
                let f = i as f64 / n as f64;
                let f = if e.reversed { 1.0 - f } else { f };
                poly.push(e.curve.point_at(f));
            }
        }
        let Some(&first) = poly.first() else { continue };
        let depth = area
            .loops
            .iter()
            .enumerate()
            .filter(|&(j, other)| j != k && loop_winding(other, first) != 0)
            .count();
        // Keep the side `normal` points to (Sutherland–Hodgman against one line).
        let inside = |q: DVec2| (q - at).dot(normal) >= 0.0;
        let mut kept = Vec::new();
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            if inside(p) {
                kept.push(p);
            }
            if inside(p) != inside(q) {
                let (dp, dq) = ((p - at).dot(normal), (q - at).dot(normal));
                kept.push(p + (q - p) * (dp / (dp - dq)));
            }
        }
        let signed: f64 = (0..kept.len())
            .map(|i| kept[i].perp_dot(kept[(i + 1) % kept.len()]))
            .sum::<f64>()
            / 2.0;
        total += if depth % 2 == 0 {
            signed.abs()
        } else {
            -signed.abs()
        };
    }
    total.max(0.0)
}
