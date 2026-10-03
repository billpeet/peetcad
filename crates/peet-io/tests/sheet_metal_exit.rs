//! Phase 4 exit criterion: model an enclosure panel with four flanges, reliefs and
//! cutouts, export the flat pattern as DXF, and check it against hand-calculated values.
//!
//! The panel: a 200 × 150 plate, 1.5 mm thick, inner bend radius 2 mm, K-factor 0.44.
//! Four 25 mm flanges (outside height) at 90°, material inside, each set back 10 mm from
//! both ends of its edge, with rectangular reliefs. Cutouts: an 80 × 40 window and four
//! Ø5 holes in the plate, a Ø8 hole in the front flange, and a 10 mm slot that runs from
//! the plate across the right-hand bend into the flange.
//!
//! **Hand calculation** (90° bends, R = 2, t = 1.5, K = 0.44):
//!
//! - bend allowance BA = π/2 · (R + K·t) = π/2 · 2.66 = 4.178318 mm
//! - outside setback OSSB = (R + t) · tan 45° = 3.5 mm
//! - bend deduction BD = 2·OSSB − BA = 2.821682 mm
//! - flat width = 200 + 2·25 − 2·BD = 244.356636 mm; flat height = 150 + 50 − 2·BD
//!   = 194.356636 mm
//! - each bend line runs 200 − 2·10 = 180 mm (or 130 mm) along the middle of its bend
//!   region, BA/2 outside the bend's start line, which is OSSB inside the plate's edge
//! - a point on the front flange's outer face at height h above the bottom of the plate
//!   lies OSSB − BA − (h − OSSB) from the plate's front edge in the flat (negative: outside)

use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use peet_io::dxf::{self, layer, read::Raw};
use peet_kernel::validate::validate;
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{
    BendModelDef, Body, EdgeRef, Engine, FeatureId, FeatureKind, Model, PlaneRef, Scalar, Status,
    StdPlane,
};
use peet_sketch::{ConstraintKind, Sketch, shapes};

const W: f64 = 200.0;
const H: f64 = 150.0;
const T: f64 = 1.5;
const R: f64 = 2.0;
const K: f64 = 0.44;
const L: f64 = 25.0;
const OFFSET: f64 = 10.0;

fn ba() -> f64 {
    FRAC_PI_2 * (R + K * T)
}
fn ossb() -> f64 {
    R + T
}
fn bd() -> f64 {
    2.0 * ossb() - ba()
}

struct Part {
    model: Model,
    engine: Engine,
}

impl Part {
    fn rebuild(&mut self) {
        self.engine.regenerate(&mut self.model);
        for f in self.model.features() {
            if let Some(Status::Failed(m)) = self.engine.evaluation().status(f.id) {
                panic!("{} failed: {m}", f.name);
            }
        }
    }

    fn body(&self) -> &Arc<Body> {
        &self.engine.evaluation().bodies[0]
    }

    fn sketch(&mut self, plane: PlaneRef, draw: impl FnOnce(&mut Sketch)) -> FeatureId {
        let id = self.model.add_sketch(plane, Plane::TOP);
        let s = &mut self
            .model
            .feature_mut(id)
            .unwrap()
            .sketch_mut()
            .unwrap()
            .sketch;
        draw(s);
        id
    }

    /// The edge between two points (either way round).
    fn edge(&self, a: DVec3, b: DVec3) -> EdgeRef {
        let body = self.body();
        for e in body.solid.edge_ids() {
            let edge = body.solid.edge(e);
            let (s, t) = (
                body.solid.vertex(edge.start).point,
                body.solid.vertex(edge.end).point,
            );
            if (s.distance(a) < 1e-9 && t.distance(b) < 1e-9)
                || (s.distance(b) < 1e-9 && t.distance(a) < 1e-9)
            {
                return body.edge_ref(e).unwrap();
            }
        }
        panic!("no edge from {a} to {b}");
    }

    /// The flat face with outward normal `n` through `p`.
    fn face(&self, n: DVec3, p: DVec3) -> PlaneRef {
        let body = self.body();
        for f in body.solid.face_ids() {
            if let peet_kernel::Surface::Plane(pl) = body.solid.face(f).surface
                && body.solid.face_normal_at(f, p).dot(n) > 0.999
                && pl.signed_distance(p).abs() < 1e-9
            {
                return PlaneRef::Face(body.face_ref(f));
            }
        }
        panic!("no face with normal {n} through {p}");
    }
}

/// Builds the panel through the model, as a user would.
fn panel() -> Part {
    let mut p = Part {
        model: Model::new(),
        engine: Engine::new(),
    };
    let base = p.sketch(PlaneRef::Standard(StdPlane::Top), |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(W, H));
        let corner = s.endpoints(shape.curves[0]).unwrap().0;
        s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN))
            .unwrap();
        s.add_dimension(ConstraintKind::Length(shape.curves[0]), W)
            .unwrap();
        s.add_dimension(ConstraintKind::Length(shape.curves[1]), H)
            .unwrap();
    });
    let flange = p.model.add_base_flange(base);
    let FeatureKind::BaseFlange(b) = &mut p.model.feature_mut(flange).unwrap().kind else {
        unreachable!()
    };
    b.settings.thickness = Scalar::new(T);
    b.settings.radius = Scalar::new(R);
    b.settings.model = BendModelDef::KFactor(Scalar::new(K));
    p.rebuild();

    // Four flanges, picked on the top side of each edge of the plate.
    let corners = [
        DVec3::new(0.0, 0.0, T),
        DVec3::new(W, 0.0, T),
        DVec3::new(W, H, T),
        DVec3::new(0.0, H, T),
    ];
    for i in 0..4 {
        let e = p.edge(corners[i], corners[(i + 1) % 4]);
        let id = p.model.add_edge_flange(Some(e));
        let FeatureKind::EdgeFlange(f) = &mut p.model.feature_mut(id).unwrap().kind else {
            unreachable!()
        };
        f.length = Scalar::new(L);
        f.offset_start = Scalar::new(OFFSET);
        f.offset_end = Scalar::new(OFFSET);
        p.rebuild();
    }

    // Cutouts in the plate: a window, four holes, and a slot across the right-hand bend.
    let top = p.face(DVec3::Z, DVec3::new(50.0, 50.0, T));
    let cutouts = p.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(60.0, 55.0), DVec2::new(140.0, 95.0));
        for (x, y) in [(20.0, 20.0), (180.0, 20.0), (20.0, 130.0), (180.0, 130.0)] {
            s.add_circle(DVec2::new(x, y), 2.5);
        }
        shapes::rectangle(s, DVec2::new(190.0, 70.0), DVec2::new(210.0, 80.0));
    });
    p.model.add_sheet_cut(cutouts);
    p.rebuild();

    // A Ø8 hole in the front flange, 15 mm above the bottom of the plate.
    let front = p.face(-DVec3::Y, DVec3::new(100.0, 0.0, 15.0));
    let hole = p.sketch(front, |s| {
        s.add_circle(DVec2::new(100.0, 15.0), 4.0);
    });
    p.model.add_sheet_cut(hole);
    p.rebuild();
    p
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6
}

#[track_caller]
fn assert_close(a: f64, b: f64) {
    assert!(close(a, b), "{a} != {b}");
}

fn extent(entities: &[&Raw]) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in entities {
        let pts = match e.kind.as_str() {
            "LINE" => vec![
                DVec2::new(e.num(10), e.num(20)),
                DVec2::new(e.num(11), e.num(21)),
            ],
            "ARC" | "CIRCLE" => {
                let (c, r) = (DVec2::new(e.num(10), e.num(20)), e.num(40));
                vec![c - DVec2::splat(r), c + DVec2::splat(r)]
            }
            _ => vec![],
        };
        for p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo, hi)
}

#[test]
fn enclosure_panel_flat_pattern_is_dimensionally_correct() {
    let p = panel();
    let body = p.body().clone();
    validate(&body.solid).expect("a valid folded body");
    let sheet = body.sheet.clone().expect("a sheet metal body");

    // The folded panel keeps its outside size.
    let b = body.solid.bounds();
    assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-9), "{:?}", b.min);
    assert!(b.max.abs_diff_eq(DVec3::new(W, H, L), 1e-9), "{:?}", b.max);

    let text = dxf::flat_pattern(&sheet);
    let entities = dxf::read::entities(&text).expect("a well-formed DXF");
    let on = |l: &str| -> Vec<&Raw> { entities.iter().filter(|e| e.layer() == l).collect() };

    // Outer profile: the flat blank's size.
    let outline = on(layer::OUTLINE);
    let (lo, hi) = extent(&outline);
    assert_close(hi.x - lo.x, W + 2.0 * L - 2.0 * bd());
    assert_close(hi.y - lo.y, H + 2.0 * L - 2.0 * bd());
    assert!(
        (hi.x - lo.x - 244.356636).abs() < 1e-6,
        "the hand-calculated width"
    );
    assert!(
        (hi.y - lo.y - 194.356636).abs() < 1e-6,
        "the hand-calculated height"
    );
    // The plate keeps its sketch coordinates: the front flange's flat ends at
    // OSSB − BA − (L − OSSB) below the plate's front edge.
    assert_close(lo.y, ossb() - ba() - (L - ossb()));
    assert_close(lo.x, ossb() - ba() - (L - ossb()));
    assert!(
        outline.iter().all(|e| e.kind == "LINE"),
        "the outline is straight lines"
    );

    // Cutouts: the window and the slot as lines, five round holes as circles.
    let cutouts = on(layer::CUTOUTS);
    let circles: Vec<&Raw> = cutouts
        .iter()
        .copied()
        .filter(|e| e.kind == "CIRCLE")
        .collect();
    assert_eq!(circles.len(), 5);
    let centres: Vec<(f64, f64, f64)> = circles
        .iter()
        .map(|c| (c.num(10), c.num(20), c.num(40)))
        .collect();
    for (x, y) in [(20.0, 20.0), (180.0, 20.0), (20.0, 130.0), (180.0, 130.0)] {
        assert!(
            centres
                .iter()
                .any(|&(cx, cy, r)| close(cx, x) && close(cy, y) && close(r, 2.5)),
            "hole at ({x}, {y}) in {centres:?}"
        );
    }
    let flange_hole_y = ossb() - ba() - (15.0 - ossb());
    assert!(
        centres
            .iter()
            .any(|&(cx, cy, r)| close(cx, 100.0) && close(cy, flange_hole_y) && close(r, 4.0)),
        "flange hole at (100, {flange_hole_y}) in {centres:?}"
    );
    let lines: Vec<&Raw> = cutouts
        .iter()
        .copied()
        .filter(|e| e.kind == "LINE")
        .collect();
    assert_eq!(lines.len(), 8, "window and slot: four lines each");
    // The slot keeps its flat length of 20 mm across the bend.
    let slot: Vec<&Raw> = lines
        .iter()
        .copied()
        .filter(|e| e.num(10).min(e.num(11)) >= 189.0)
        .collect();
    assert_eq!(slot.len(), 4);
    let (slo, shi) = extent(&slot);
    assert_close(shi.x - slo.x, 20.0);
    assert_close(shi.y - slo.y, 10.0);

    // Bend lines: one per flange, the length of the bend, in the middle of the bend region.
    let bends = on(layer::BEND);
    let line_y0: Vec<&Raw> = bends
        .iter()
        .copied()
        .filter(|e| close(e.num(20), ossb() - ba() / 2.0) && close(e.num(21), ossb() - ba() / 2.0))
        .collect();
    assert_eq!(line_y0.len(), 1, "the front bend line");
    assert_close(
        (line_y0[0].num(11) - line_y0[0].num(10)).abs(),
        W - 2.0 * OFFSET,
    );
    // The right-hand bend line is cut in two by the slot.
    let right_x = W - ossb() + ba() / 2.0;
    let right: Vec<&Raw> = bends
        .iter()
        .copied()
        .filter(|e| close(e.num(10), right_x) && close(e.num(11), right_x))
        .collect();
    assert_eq!(right.len(), 2);
    let right_len: f64 = right.iter().map(|e| (e.num(21) - e.num(20)).abs()).sum();
    assert_close(right_len, H - 2.0 * OFFSET - 10.0);
    assert_eq!(bends.len(), 5);
    assert!(bends.iter().all(|e| e.get(6) == Some("DASHED")));

    // Bend notes: direction, angle and radius for each bend.
    let notes = on(layer::BEND_NOTES);
    assert_eq!(notes.len(), 4);
    assert!(
        notes.iter().all(|n| n.get(1) == Some("UP 90%%d R2")),
        "{notes:?}"
    );

    // Reliefs: the corners are notched, so the outline has more than the 4 + 4·4 lines
    // of a plain plate with four flanges.
    assert!(outline.len() > 20);

    // The report agrees.
    let report = sheet.report();
    assert_eq!(report.bends.len(), 4);
    assert_eq!(report.pieces, 1);
    assert_eq!(report.cutouts, 7);
    for row in &report.bends {
        assert_close(row.allowance, ba());
        assert_close(row.deduction, bd());
        assert_close(row.k_factor, K);
        assert_close(row.angle, 90.0);
    }
    assert_close(report.flat_size.x, W + 2.0 * L - 2.0 * bd());
}

#[test]
fn panel_rebuilds_within_budget() {
    // Unfolding is free (the flat pattern is built with the body); a full rebuild of the
    // panel after a change to the plate's width must stay interactive.
    let mut p = panel();
    let base = p.model.features().next().unwrap().id;
    let s = &mut p
        .model
        .feature_mut(base)
        .unwrap()
        .sketch_mut()
        .unwrap()
        .sketch;
    let d1 = s.dimension_by_name("d1").unwrap();
    peet_sketch::expr::set_dimension_input(s, &peet_sketch::expr::Parameters::default(), d1, "220")
        .unwrap();
    let start = std::time::Instant::now();
    p.rebuild();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let b = p.body().solid.bounds();
    assert!(
        b.max.abs_diff_eq(DVec3::new(220.0, H, L), 1e-9),
        "{:?}",
        b.max
    );
    // Generous in debug builds; release builds take a few milliseconds.
    assert!(ms < 2000.0, "{ms} ms");
}
