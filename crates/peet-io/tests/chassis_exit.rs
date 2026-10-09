//! Phase 5 exit criterion: a typical sheet metal chassis, built with the Phase 5
//! features, whose flat pattern DXF is dimensionally correct against hand calculation,
//! which passes the manufacturing checks, and which survives a save and reopen.
//! (Its STEP export is checked in `step_export.rs`.)
//!
//! The chassis (`peet_model::samples::chassis`): a 240 × 160 base, 1.5 mm thick, inner
//! bend radius 1.5 mm, K-factor 0.44. A mitre flange runs round the right, back and left
//! edges: a wall 40 high and a rim 12 wide turned in, both measured on the outside,
//! mitred at the two back corners. The front edge has a closed hem 10 long. On the base:
//! four Ø4.5 holes (one cut, patterned 2 × 2 at 200 × 110), ten louvers 40 × 8 × 3 (one,
//! patterned 2 × 5 at 80 × 14), two Ø10 dimples 2.5 high (one, mirrored). The back wall
//! has a 30 × 12 window.
//!
//! **Hand calculation** (90° bends, R = 1.5, t = 1.5, K = 0.44):
//!
//! - bend allowance BA = π/2 · (R + K·t) = π/2 · 2.16 = 3.392920 mm
//! - outside setback OSSB = R + t = 3 mm; bend deduction BD = 2·OSSB − BA = 2.607080 mm
//! - each rimmed side adds wall + rim − 2·BD = 52 − 5.214159 = 46.785841 mm to the flat
//! - flat width = 240 + 2 · 46.785841 = 333.571680 mm
//! - the hem: inner radius 0.01·t = 0.015 mm, so its fold reaches R + t = 1.515 mm past
//!   its start; its allowance is π · (0.015 + 0.44 · 1.5) = 2.120575 mm. Inside the
//!   outline, it takes 1.515 from the base and adds 2.120575 + (10 − 1.515).
//! - flat height = 160 + 46.785841 − 1.515 + 2.120575 + 8.485 = 215.876415 mm
//! - bend lines: the first bend of each side lies BA/2 outside its start, which is OSSB
//!   inside the base's edge; the second lies (40 − 2·OSSB) + BA further out.

use std::f64::consts::{FRAC_PI_2, PI};

use peet_io::dxf::{self, layer, read::Raw};
use peet_io::dxf_import::{ImportOptions, import};
use peet_kernel::validate::{measure, validate};
use peet_model::samples::{chassis, chassis_size as cs};
use peet_model::{FeatureId, Status};
use peet_sheetmetal::{CheckRules, Severity, check};
use peet_sketch::region::find_regions;

fn ba() -> f64 {
    FRAC_PI_2 * (cs::RADIUS + cs::K * cs::THICKNESS)
}
fn ossb() -> f64 {
    cs::RADIUS + cs::THICKNESS
}
fn bd() -> f64 {
    2.0 * ossb() - ba()
}
/// What one rimmed side adds to the flat pattern.
fn side() -> f64 {
    cs::WALL + cs::LIP - 2.0 * bd()
}
fn hem_radius() -> f64 {
    0.01 * cs::THICKNESS
}
fn hem_allowance() -> f64 {
    PI * (hem_radius() + cs::K * cs::THICKNESS)
}

#[track_caller]
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn chassis_builds_and_folds_to_size() {
    let (model, engine) = chassis();
    let eval = engine.evaluation();
    for f in model.features() {
        assert!(
            matches!(eval.status(f.id), Some(Status::Ok)),
            "{}: {:?}",
            f.name,
            eval.status(f.id)
        );
    }
    assert_eq!(eval.bodies.len(), 1);
    let body = &eval.bodies[0];
    validate(&body.solid).expect("a valid folded solid");
    let sheet = body.sheet.as_ref().expect("a sheet metal body");
    validate(&sheet.flat).expect("a valid flat solid");
    assert_eq!(body.solid.faces.len(), sheet.flat.faces.len());
    // Folded: the tray keeps the base's outline, and stands the wall height.
    let b = body.solid.bounds();
    assert!(b.min.abs().max_element() < 1e-9, "{:?}", b.min);
    close(b.max.x, cs::WIDTH);
    close(b.max.y, cs::DEPTH);
    close(b.max.z, cs::WALL);
    // Flat size.
    let report = sheet.report();
    close(report.flat_size.x, cs::WIDTH + 2.0 * side());
    close(
        report.flat_size.y,
        cs::DEPTH + side() - (hem_radius() + cs::THICKNESS) + hem_allowance() + cs::HEM
            - (hem_radius() + cs::THICKNESS),
    );
    close(report.flat_size.x, 333.571_680_263_507_9);
    close(report.flat_size.y, 215.876_415_172_927);
    // Seven bends: two on each rimmed side, and the hem.
    assert_eq!(report.bends.len(), 7);
    assert_eq!(report.bends.iter().filter(|b| b.angle > 179.0).count(), 1);
    assert_eq!(report.pieces, 1);
    // Four holes and the window are cut; louvers and dimples are formed.
    assert_eq!(report.cutouts, 5);
    assert_eq!(sheet.forms.len(), 12);
    // Flat area: the blank less the corner reliefs and mitres is hard to do by hand,
    // but the cutouts are exact: compare with and without them through the outline.
    let holes: f64 = 4.0 * PI * cs::HOLE_RADIUS * cs::HOLE_RADIUS + cs::WINDOW.0 * cs::WINDOW.1;
    let outer = sheet
        .outline
        .iter()
        .filter(|l| l.outer)
        .map(loop_area)
        .sum::<f64>();
    let inner = sheet
        .outline
        .iter()
        .filter(|l| !l.outer)
        .map(loop_area)
        .sum::<f64>();
    close(inner.abs(), holes);
    // The flat solid is the blank less the cutouts, plus what the forms add: the ring
    // of each form's wall (outline area less inside area) times its height.
    let (ll, lw, lh) = cs::LOUVER;
    let t = cs::THICKNESS;
    let louver = (ll * lw - (ll - 2.0 * t) * (lw - t)) * lh;
    let (dr, dh) = cs::DIMPLE;
    let dimple = PI * (dr * dr - (dr - t) * (dr - t)) * dh;
    close(
        measure::volume(&sheet.flat),
        (outer - holes) * t + 10.0 * louver + 2.0 * dimple,
    );
    // It rebuilds well inside the budget (100 ms for a 20-feature part).
    assert!(eval.stats.ms < 100.0, "{} ms", eval.stats.ms);
}

/// Signed area of a flat loop.
fn loop_area(l: &peet_sheetmetal::FlatLoop) -> f64 {
    use peet_sketch::Curve;
    l.edges
        .iter()
        .map(|(c, reversed)| {
            let a = match *c {
                Curve::Circle { radius, .. } => PI * radius * radius,
                Curve::Arc { radius, sweep, .. } => {
                    0.5 * c.start().perp_dot(c.end())
                        + 0.5 * radius * radius * (sweep - sweep.sin())
                }
                Curve::Line { a, b } => 0.5 * a.perp_dot(b),
                Curve::Spline(_) => c.area_term(),
            };
            if *reversed { -a } else { a }
        })
        .sum()
}

#[test]
fn chassis_flat_pattern_dxf_is_dimensionally_correct() {
    let (_, engine) = chassis();
    let sheet = engine.evaluation().bodies[0].sheet.clone().unwrap();
    let text = dxf::flat_pattern(&sheet);
    let entities: Vec<Raw> = dxf::read::entities(&text).expect("a readable DXF");
    let on = |l: &str| {
        entities
            .iter()
            .filter(|e| e.layer() == l)
            .collect::<Vec<_>>()
    };

    // The outline's extents are the flat size.
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for e in on(layer::OUTLINE) {
        assert_eq!(e.kind, "LINE", "the blank's outline is all straight");
        for (cx, cy) in [(10, 20), (11, 21)] {
            let (x, y) = (e.num(cx), e.num(cy));
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
    }
    close(hi.0 - lo.0, cs::WIDTH + 2.0 * side());
    // The base keeps its sketch coordinates: the left side unfolds to −side().
    close(lo.0, -side());
    close(hi.0, cs::WIDTH + side());
    close(hi.1, cs::DEPTH + side());
    // The hem unfolds to the front: its start is R + t inside the front edge.
    let reach = hem_radius() + cs::THICKNESS;
    close(lo.1, reach - hem_allowance() - (cs::HEM - reach));

    // Cutouts: four Ø4.5 holes at the grid's corners, the window as four lines, and the
    // ten louvers' lances.
    let cut = on(layer::CUTOUTS);
    let circles: Vec<_> = cut.iter().filter(|e| e.kind == "CIRCLE").collect();
    assert_eq!(circles.len(), 4);
    let mut centres: Vec<(f64, f64)> = circles.iter().map(|e| (e.num(10), e.num(20))).collect();
    centres.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let expected = [(20.0, 25.0), (20.0, 135.0), (220.0, 25.0), (220.0, 135.0)];
    for (c, e) in centres.iter().zip(expected) {
        close(c.0, e.0);
        close(c.1, e.1);
    }
    for c in &circles {
        close(c.num(40), cs::HOLE_RADIUS);
    }
    let lines: Vec<_> = cut.iter().filter(|e| e.kind == "LINE").collect();
    assert_eq!(lines.len(), 4 + 10, "the window and ten lances");
    // The window, in the back wall: 14 to 26 above the base's bottom. In the flat, a
    // point of the wall's outer face at height h lies DEPTH − OSSB + BA + (h − OSSB).
    let flat_y = |h: f64| cs::DEPTH - ossb() + ba() + (h - ossb());
    let window: Vec<_> = lines
        .iter()
        .filter(|e| e.num(20) > cs::DEPTH && e.num(21) > cs::DEPTH)
        .collect();
    assert_eq!(window.len(), 4);
    let ys: Vec<f64> = window.iter().flat_map(|e| [e.num(20), e.num(21)]).collect();
    let xs: Vec<f64> = window.iter().flat_map(|e| [e.num(10), e.num(11)]).collect();
    close(
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        flat_y(14.0),
    );
    close(
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        flat_y(14.0 + cs::WINDOW.1),
    );
    close(
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        cs::WIDTH / 2.0 - cs::WINDOW.0 / 2.0,
    );
    close(
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        cs::WIDTH / 2.0 + cs::WINDOW.0 / 2.0,
    );
    // The lances: each a louver's length, on the louvers' first side.
    let lances: Vec<_> = lines
        .iter()
        .filter(|e| e.num(20) < cs::DEPTH && e.num(21) < cs::DEPTH)
        .collect();
    assert_eq!(lances.len(), 10);
    for l in &lances {
        close((l.num(11) - l.num(10)).abs(), cs::LOUVER.0);
        close(l.num(20), l.num(21));
    }

    // Bend lines: seven, dashed, where the hand calculation puts them.
    let bends = on(layer::BEND);
    assert_eq!(bends.len(), 7);
    let first = cs::WIDTH - ossb() + ba() / 2.0;
    let second = first + (cs::WALL - 2.0 * ossb()) + ba();
    let mut right: Vec<f64> = bends
        .iter()
        .filter(|e| (e.num(10) - e.num(11)).abs() < 1e-9 && e.num(10) > cs::WIDTH / 2.0)
        .map(|e| e.num(10))
        .collect();
    right.sort_by(f64::total_cmp);
    assert_eq!(right.len(), 2);
    close(right[0], first);
    close(right[1], second);
    // The hem's bend line: BA/2 outside its start.
    let hem: Vec<_> = bends
        .iter()
        .filter(|e| (e.num(20) - e.num(21)).abs() < 1e-9 && e.num(20) < cs::DEPTH / 2.0)
        .collect();
    assert_eq!(hem.len(), 1);
    close(hem[0].num(20), reach - hem_allowance() / 2.0);
    // It runs between the two walls' bends.
    close(
        (hem[0].num(11) - hem[0].num(10)).abs(),
        cs::WIDTH - 2.0 * ossb(),
    );

    // Notes: one per bend, one per form.
    let notes = on(layer::BEND_NOTES);
    assert_eq!(notes.len(), 7);
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.get(1).is_some_and(|t| t.contains("180")))
            .count(),
        1
    );
    let form_notes = on(layer::FORM_NOTES);
    assert_eq!(form_notes.len(), 12);
    let louvers = form_notes
        .iter()
        .filter(|n| n.get(1).is_some_and(|t| t.starts_with("LOUVER UP 3")))
        .count();
    let dimples = form_notes
        .iter()
        .filter(|n| n.get(1).is_some_and(|t| t.starts_with("DIMPLE UP 2.5")))
        .count();
    assert_eq!((louvers, dimples), (10, 2));
    // The dimples are marked as circles on the forms layer, mirrored about x = 120.
    let marks: Vec<_> = on(layer::FORMS)
        .into_iter()
        .filter(|e| e.kind == "CIRCLE")
        .collect();
    assert_eq!(marks.len(), 2);
    let mut xs: Vec<f64> = marks.iter().map(|e| e.num(10)).collect();
    xs.sort_by(f64::total_cmp);
    close(xs[0], 30.0);
    close(xs[1], 210.0);
}

#[test]
fn chassis_passes_the_manufacturing_checks() {
    let (model, engine) = chassis();
    let sheet = engine.evaluation().bodies[0].sheet.clone().unwrap();
    let findings = check(&sheet, &CheckRules::default(), |o| {
        model.name_of(FeatureId(o)).to_owned()
    });
    let problems: Vec<_> = findings
        .iter()
        .filter(|f| f.severity != Severity::Info)
        .map(|f| &f.message)
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn chassis_flat_pattern_imports_back_as_a_sketch() {
    // A fabricator's view of the DXF: the blank is one closed profile with five holes.
    let (_, engine) = chassis();
    let sheet = engine.evaluation().bodies[0].sheet.clone().unwrap();
    let text = dxf::flat_pattern(&sheet);
    let imported = import(text.as_bytes(), &ImportOptions::flat_pattern()).unwrap();
    let profile = find_regions(&imported.sketch);
    let blank = profile
        .regions
        .iter()
        .max_by(|a, b| {
            a.outer
                .signed_area
                .abs()
                .total_cmp(&b.outer.signed_area.abs())
        })
        .unwrap();
    // Open ends: the ten lances (cut lines, not loops), and the two short tears where
    // the hem's fold meets the side walls' bends (the sheet rips there, 1.5 mm).
    assert_eq!(profile.open_ends.len(), 2 * 10 + 2);
    assert_eq!(blank.holes.len(), 5);
}

#[test]
fn chassis_survives_save_and_reopen() {
    let (model, engine) = chassis();
    let bodies = engine.evaluation().bodies.clone();
    let caches = peet_io::document::Caches {
        bodies: &bodies,
        meshes: Vec::new(),
    };
    let bytes = peet_io::document::save(
        &model,
        &peet_io::document::Metadata::default(),
        Some(caches),
    )
    .expect("the chassis saves");
    let opened = peet_io::document::open(&bytes).expect("the chassis opens");
    assert_eq!(opened.model, model);
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    // Rebuilt from the file, it is the same part.
    let mut model = opened.model;
    let mut engine = peet_model::Engine::new();
    let eval = engine.regenerate(&mut model);
    assert_eq!(eval.bodies.len(), 1);
    assert_eq!(eval.bodies[0].solid, bodies[0].solid);
    assert_eq!(eval.bodies[0].face_names, bodies[0].face_names);
}
