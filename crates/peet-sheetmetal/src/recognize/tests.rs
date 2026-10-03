//! Recognition tests. Most parts are made natively first (a layout, built), and their
//! folded solid is handed to [`recognize`] as if it came from elsewhere: the native body
//! is then the expected answer for the flat pattern. Parts extruded from a profile check
//! the topology other CAD systems produce (one edge face running along several walls).

use std::f64::consts::FRAC_PI_2;

use peet_kernel::extrude::extrude;
use peet_kernel::geom::{Cone, Torus};
use peet_kernel::validate::validate;
use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::region::{Region, find_regions, regions_of_curves};
use peet_sketch::{EntityId, Sketch, shapes};

use super::*;
use crate::build::FaceTag;
use crate::flange::{EdgeFlangeSpec, HemKind, HemSpec};
use crate::layout::{ChainLine, EdgeSite};
use crate::settings::{BendModel, FlangePosition, ReliefType};

const W: f64 = 100.0;
const H: f64 = 60.0;
const T: f64 = 2.0;
const R: f64 = 3.0;
const K: f64 = 0.44;
const OWNER: u32 = 9;

fn settings() -> SheetSettings {
    SheetSettings {
        thickness: T,
        radius: R,
        model: BendModel::KFactor(K),
        relief: ReliefType::Rectangular,
        relief_ratio: 0.5,
    }
}

/// What recognition is told: the bend model, with a thickness and radius it must replace.
fn hint() -> SheetSettings {
    SheetSettings {
        thickness: 0.7,
        radius: 0.3,
        ..settings()
    }
}

fn plate() -> Layout {
    let mut sk = Sketch::new();
    shapes::rectangle(&mut sk, DVec2::ZERO, DVec2::new(W, H));
    let regions = find_regions(&sk).regions;
    Layout::plate(settings(), 1, &Plane::TOP, &regions, false).unwrap()
}

/// The plate's four edges, in order: `y = 0`, `x = W`, `y = H`, `x = 0`.
fn edge(i: usize, top: bool) -> EdgeSite {
    let c = [
        DVec2::ZERO,
        DVec2::new(W, 0.0),
        DVec2::new(W, H),
        DVec2::new(0.0, H),
    ];
    EdgeSite {
        piece: 0,
        a: c[i],
        b: c[(i + 1) % 4],
        top,
    }
}

fn flange(length: f64, angle: f64) -> EdgeFlangeSpec {
    EdgeFlangeSpec {
        length,
        angle,
        position: FlangePosition::MaterialInside,
        offsets: [0.0, 0.0],
        flip: false,
        radius: None,
    }
}

#[track_caller]
fn native(layout: Layout) -> (Solid, SheetBody) {
    match build(layout) {
        Ok(b) => b,
        Err(e) => panic!("build failed: {}", e.message(|o| format!("owner {o}"))),
    }
}

/// The top face of the native body's first flange.
fn fixed_face(body: &SheetBody) -> FaceId {
    let i = body
        .faces
        .iter()
        .position(|f| matches!(f, FaceTag::Top { piece: 0 }))
        .unwrap();
    FaceId(i as u32)
}

#[track_caller]
fn recognized(solid: &Solid, fixed: Option<FaceId>) -> Recognized {
    match recognize(solid, fixed, &hint(), OWNER) {
        Ok(r) => {
            validate(&r.solid).expect("folded solid is valid");
            validate(&r.body.flat).expect("flat solid is valid");
            r
        }
        Err(e) => panic!("not recognised: {}", e.message(|o| format!("owner {o}"))),
    }
}

#[track_caller]
fn refusal(solid: &Solid, fixed: Option<FaceId>) -> String {
    match recognize(solid, fixed, &hint(), OWNER) {
        Ok(_) => panic!("recognised a part that should be refused"),
        Err(e) => e.message(|o| format!("owner {o}")),
    }
}

#[track_caller]
fn assert_close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} != {b} (±{tol})");
}

/// The flat blank's size, smaller side first.
fn blank(body: &SheetBody) -> (f64, f64) {
    let (lo, hi) = body.flat_bounds();
    let s = hi - lo;
    (s.x.min(s.y), s.x.max(s.y))
}

fn sorted(mut points: Vec<DVec3>) -> Vec<DVec3> {
    let key = |p: &DVec3| {
        (
            (p.x * 1e3).round(),
            (p.y * 1e3).round(),
            (p.z * 1e3).round(),
        )
    };
    points.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
    points
}

/// The centres of the round holes of the flat pattern, in model space.
fn hole_centers(body: &SheetBody) -> Vec<DVec3> {
    let frame = body.layout.pieces[0].frame;
    sorted(
        body.outline
            .iter()
            .filter(|l| !l.outer)
            .filter_map(|l| l.edges[0].0.center())
            .map(|c| frame.to_world(c.extend(0.0)))
            .collect(),
    )
}

/// The middles of the bend lines of the flat pattern, in model space.
fn bend_line_middles(body: &SheetBody) -> Vec<DVec3> {
    let frame = body.layout.pieces[0].frame;
    sorted(
        body.bend_lines
            .iter()
            .flat_map(|l| &l.segments)
            .map(|s| frame.to_world(((s[0] + s[1]) / 2.0).extend(0.0)))
            .collect(),
    )
}

/// Checks a recognised part against the native body it was made from: the same folded
/// solid, and the same flat pattern in the same place (the same wall fixed).
#[track_caller]
fn assert_same(rec: &Recognized, solid: &Solid, body: &SheetBody) {
    let s = rec.body.layout.settings;
    assert_close(s.thickness, T, 1e-9);
    assert_eq!(s.model, BendModel::KFactor(K));
    assert_close(
        measure::volume(&rec.solid),
        measure::volume(solid),
        1e-6 * measure::volume(solid),
    );
    assert_close(
        measure::volume(&rec.body.flat),
        measure::volume(&body.flat),
        1e-6 * measure::volume(&body.flat),
    );
    let (a, b) = (bounds(&rec.body.flat), bounds(&body.flat));
    assert!(
        a.min.abs_diff_eq(b.min, 1e-6) && a.max.abs_diff_eq(b.max, 1e-6),
        "flat pattern at {a:?}, expected {b:?}"
    );
    let (got, want) = (blank(&rec.body), blank(body));
    assert_close(got.0, want.0, 1e-6);
    assert_close(got.1, want.1, 1e-6);
    let (got, want) = (bend_line_middles(&rec.body), bend_line_middles(body));
    assert_eq!(got.len(), want.len(), "bend lines");
    for (g, w) in got.iter().zip(&want) {
        assert!(g.abs_diff_eq(*w, 1e-6), "bend line at {g}, expected {w}");
    }
    let (got, want) = (hole_centers(&rec.body), hole_centers(body));
    assert_eq!(got.len(), want.len(), "holes");
    for (g, w) in got.iter().zip(&want) {
        assert!(g.abs_diff_eq(*w, 1e-6), "hole at {g}, expected {w}");
    }
    let report = (rec.body.report(), body.report());
    assert_eq!(report.0.cutouts, report.1.cutouts);
    assert_eq!(report.0.pieces, 1);
}

/// The bends of a layout: `(angle in degrees, inner radius, up)`.
fn bends(layout: &Layout) -> Vec<(f64, f64, bool)> {
    layout
        .bends()
        .map(|(_, b)| (b.values.angle.to_degrees(), b.values.radius, b.up))
        .collect()
}

fn circle(center: DVec2, radius: f64, entity: u32) -> Area {
    Area {
        loops: vec![vec![Edge2 {
            curve: Curve::Circle { center, radius },
            reversed: false,
            tag: CurveTag::Sketch { owner: 2, entity },
        }]],
    }
}

#[test]
fn flat_plate() {
    let (solid, body) = native(plate());
    let rec = recognized(&solid, None);
    assert_eq!(rec.body.layout.pieces.len(), 1);
    assert_close(rec.body.layout.settings.thickness, T, 1e-12);
    // No bends to take a radius from: the hint's stays.
    assert_close(rec.body.layout.settings.radius, hint().radius, 1e-12);
    let (a, b) = blank(&rec.body);
    assert_close(a, H, 1e-9);
    assert_close(b, W, 1e-9);
    assert_close(measure::volume(&rec.solid), W * H * T, 1e-9);
    assert_eq!(body.report().cutouts, 0);
    // The fixed face is the largest one, and the flat pattern lies along its long edge.
    assert_close(rec.placement.x_axis().dot(DVec3::X).abs(), 1.0, 1e-12);
}

#[test]
fn l_bracket() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(25.0, 90.0))
        .unwrap();
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    let found = bends(&rec.body.layout);
    assert_eq!(found.len(), 1);
    assert_close(found[0].0, 90.0, 1e-9);
    assert_close(found[0].1, R, 1e-9);
    assert!(found[0].2, "the flange bends up");
    assert_close(rec.body.layout.settings.radius, R, 1e-9);
    // Hand calculation: the outside sizes less one bend deduction.
    let v = BendValues::new(BendModel::KFactor(K), FRAC_PI_2, R, T).unwrap();
    let (a, b) = blank(&rec.body);
    assert_close(a, H + 25.0 - v.deduction(), 1e-9);
    assert_close(b, W, 1e-9);
    // Three pieces: wall, bend, wall, named by the converting owner.
    assert_eq!(rec.body.layout.pieces.len(), 3);
    assert!(
        rec.body
            .layout
            .pieces
            .iter()
            .all(|p| p.origin.owner == OWNER)
    );
    // Without a fixed face the largest wall is the base: the same blank.
    let auto = recognized(&solid, None);
    let (a2, b2) = blank(&auto.body);
    assert_close(a2, a, 1e-9);
    assert_close(b2, b, 1e-9);
}

#[test]
fn fixing_the_other_side_turns_the_bend_down() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(25.0, 90.0))
        .unwrap();
    let (solid, body) = native(layout);
    let bottom = body
        .faces
        .iter()
        .position(|f| matches!(f, FaceTag::Bottom { piece: 0 }))
        .unwrap();
    let rec = recognized(&solid, Some(FaceId(bottom as u32)));
    let found = bends(&rec.body.layout);
    assert_eq!(found.len(), 1);
    assert!(
        !found[0].2,
        "seen from the other side the flange bends down"
    );
    assert_close(found[0].1, R, 1e-9);
    let (want, got) = (blank(&body), blank(&rec.body));
    assert_close(got.0, want.0, 1e-9);
    assert_close(got.1, want.1, 1e-9);
}

#[test]
fn fixing_the_flange_unfolds_the_base() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(25.0, 90.0))
        .unwrap();
    let (solid, body) = native(layout);
    let wall = body
        .faces
        .iter()
        .position(|f| matches!(f, FaceTag::Top { piece: 2 }))
        .unwrap();
    let rec = recognized(&solid, Some(FaceId(wall as u32)));
    assert_eq!(rec.fixed, FaceId(wall as u32));
    let (want, got) = (blank(&body), blank(&rec.body));
    assert_close(got.0, want.0, 1e-9);
    assert_close(got.1, want.1, 1e-9);
    // The flat pattern now lies in the flange's plane (the plane y = const).
    assert_close(rec.placement.z_axis().dot(DVec3::Z), 0.0, 1e-12);
}

#[test]
fn u_channel() {
    let chain = [
        ChainLine {
            entity: 1,
            a: DVec2::new(0.0, 40.0),
            b: DVec2::new(0.0, 0.0),
        },
        ChainLine {
            entity: 4,
            a: DVec2::new(0.0, 0.0),
            b: DVec2::new(60.0, 0.0),
        },
        ChainLine {
            entity: 7,
            a: DVec2::new(60.0, 0.0),
            b: DVec2::new(60.0, 40.0),
        },
    ];
    let layout =
        Layout::open_profile(settings(), 1, &Plane::front(), &chain, [0.0, 100.0], false).unwrap();
    let (solid, body) = native(layout);
    // The native part unfolds from its first leg; fix the same one.
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    let v = BendValues::new(BendModel::KFactor(K), FRAC_PI_2, R, T).unwrap();
    let (a, b) = blank(&rec.body);
    assert_close(a, 100.0, 1e-9);
    assert_close(b, 140.0 - 2.0 * v.deduction(), 1e-9);
    assert_eq!(bends(&rec.body.layout).len(), 2);
    // Unfolded from the web (the largest face) instead: the same blank.
    let auto = recognized(&solid, None);
    let (a2, b2) = blank(&auto.body);
    assert_close(a2, a, 1e-9);
    assert_close(b2, b, 1e-9);
    assert_eq!(auto.body.layout.pieces.len(), 5);
}

#[test]
fn holes_in_the_walls() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(30.0, 90.0))
        .unwrap();
    // Two holes in the base, one in the flange (flat coordinates: beyond the bend) and a
    // square window.
    layout.add_cut(circle(DVec2::new(20.0, 20.0), 4.0, 1));
    layout.add_cut(circle(DVec2::new(70.0, 30.0), 6.0, 2));
    layout.add_cut(circle(DVec2::new(50.0, H + 18.0), 3.0, 3));
    layout.add_cut(Area::polygon(
        &[
            DVec2::new(40.0, 10.0),
            DVec2::new(55.0, 10.0),
            DVec2::new(55.0, 20.0),
            DVec2::new(40.0, 20.0),
        ],
        |i| CurveTag::Sketch {
            owner: 2,
            entity: 10 + i as u32,
        },
    ));
    let (solid, body) = native(layout);
    assert_eq!(body.report().cutouts, 4);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    assert_eq!(hole_centers(&rec.body).len(), 3);
    assert_eq!(rec.body.report().cutouts, 4);
    // Holes are outline loops of their walls, not cuts.
    assert!(rec.body.layout.cuts.is_empty());
    assert_eq!(rec.body.layout.pieces[0].outline.loops.len(), 4);
}

#[test]
fn z_profile_bends_up_and_down() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(25.0, 90.0))
        .unwrap();
    layout
        .add_edge_flange(3, &edge(0, false), &flange(20.0, 90.0))
        .unwrap();
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    let mut ups: Vec<bool> = bends(&rec.body.layout).iter().map(|b| b.2).collect();
    ups.sort_unstable();
    assert_eq!(ups, [false, true]);
    let v = BendValues::new(BendModel::KFactor(K), FRAC_PI_2, R, T).unwrap();
    let (a, _) = blank(&rec.body);
    assert_close(a, H + 25.0 + 20.0 - 2.0 * v.deduction(), 1e-9);
}

#[test]
fn bends_of_135_and_45_degrees() {
    for angle in [135.0, 45.0, 30.0] {
        let mut layout = plate();
        layout
            .add_edge_flange(2, &edge(2, true), &flange(40.0, angle))
            .unwrap();
        layout
            .add_edge_flange(3, &edge(0, true), &flange(40.0, angle))
            .unwrap();
        let (solid, body) = native(layout);
        let rec = recognized(&solid, Some(fixed_face(&body)));
        assert_same(&rec, &solid, &body);
        let found = bends(&rec.body.layout);
        assert_eq!(found.len(), 2);
        for b in found {
            assert_close(b.0, angle, 1e-7);
            assert_close(b.1, R, 1e-9);
        }
    }
}

#[test]
fn custom_radius_and_the_default() {
    let mut layout = plate();
    for (i, radius) in [(0, None), (1, Some(5.0)), (2, None)] {
        let spec = EdgeFlangeSpec {
            offsets: [12.0, 12.0],
            radius,
            ..flange(20.0, 90.0)
        };
        layout
            .add_edge_flange(2 + i as u32, &edge(i, true), &spec)
            .unwrap();
    }
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    // Two bends of R, one of 5: R is the body's radius.
    assert_close(rec.body.layout.settings.radius, R, 1e-9);
    let mut radii: Vec<f64> = bends(&rec.body.layout).iter().map(|b| b.1).collect();
    radii.sort_by(f64::total_cmp);
    assert_close(radii[0], R, 1e-9);
    assert_close(radii[2], 5.0, 1e-9);
}

#[test]
fn flanges_with_reliefs_and_a_slot_across_a_bend() {
    // An enclosure panel in small: flanges set back from the corners with reliefs, and a
    // slot that runs from the base across a bend into the flange.
    let mut layout = plate();
    for i in 0..4 {
        let spec = EdgeFlangeSpec {
            offsets: [10.0, 10.0],
            ..flange(20.0, 90.0)
        };
        layout
            .add_edge_flange(2 + i as u32, &edge(i, true), &spec)
            .unwrap();
    }
    layout.add_cut(Area::polygon(
        &[
            DVec2::new(W - 12.0, 25.0),
            DVec2::new(W + 8.0, 25.0),
            DVec2::new(W + 8.0, 35.0),
            DVec2::new(W - 12.0, 35.0),
        ],
        |i| CurveTag::Sketch {
            owner: 7,
            entity: i as u32,
        },
    ));
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    // The slot splits the bend's face in two, but it is still one bend.
    assert_eq!(bends(&rec.body.layout).len(), 4);
    assert_eq!(rec.body.report().bends.len(), 4);
    let lengths = |b: &SheetBody| {
        let mut l: Vec<f64> = b.report().bends.iter().map(|r| r.length).collect();
        l.sort_by(f64::total_cmp);
        l
    };
    for (g, w) in lengths(&rec.body).iter().zip(lengths(&body)) {
        assert_close(*g, w, 1e-6);
    }
}

#[test]
fn flange_on_a_flange_and_a_hem() {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(30.0, 90.0))
        .unwrap();
    let (solid, body) = native(layout.clone());
    // A second flange on the first one's tip, and an open hem on the opposite edge.
    let tip = (0..solid.edges.len() as u32)
        .filter_map(|e| body.edge_site(&solid, peet_kernel::EdgeId(e)).ok())
        .find(|s| s.piece == 2 && s.top && (s.a.y - s.b.y).abs() < 1e-9 && s.a.y > H + 10.0)
        .expect("the flange's tip edge");
    layout
        .add_edge_flange(3, &tip, &flange(15.0, 90.0))
        .unwrap();
    layout
        .add_hem(
            4,
            &edge(0, true),
            &HemSpec {
                kind: HemKind::Open,
                length: 8.0,
                gap: 1.0,
                radius: 1.0,
                angle: 270.0,
                inside: true,
                offsets: [0.0, 0.0],
                flip: false,
            },
        )
        .unwrap();
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    let found = bends(&rec.body.layout);
    assert_eq!(found.len(), 3);
    assert!(found.iter().any(|b| (b.0 - 180.0).abs() < 1e-7));
}

#[test]
fn box_with_closed_corners() {
    // Four flanges that meet at the corners: the corner treatment cuts reliefs and butts
    // the flanges' ends, all of which come back as plain outline edges.
    let mut layout = plate();
    for i in 0..4 {
        layout
            .add_edge_flange(2 + i as u32, &edge(i, true), &flange(20.0, 90.0))
            .unwrap();
    }
    assert_eq!(layout.corners.len(), 4);
    let (solid, body) = native(layout);
    let rec = recognized(&solid, Some(fixed_face(&body)));
    assert_same(&rec, &solid, &body);
    assert_eq!(bends(&rec.body.layout).len(), 4);
    // The recognised layout has no corners or cuts of its own: only outlines.
    assert!(rec.body.layout.corners.is_empty() && rec.body.layout.cuts.is_empty());
}

/// A profile of lines and arcs as regions.
fn regions(curves: &[Curve]) -> Vec<Region> {
    let inputs: Vec<(EntityId, Curve)> = curves
        .iter()
        .enumerate()
        .map(|(i, c)| (EntityId(i as u32), c.clone()))
        .collect();
    regions_of_curves(&inputs).regions
}

fn line(a: (f64, f64), b: (f64, f64)) -> Curve {
    Curve::Line {
        a: DVec2::new(a.0, a.1),
        b: DVec2::new(b.0, b.1),
    }
}

fn quarter(center: (f64, f64), radius: f64, start_angle: f64) -> Curve {
    Curve::Arc {
        center: DVec2::new(center.0, center.1),
        radius,
        start_angle,
        sweep: FRAC_PI_2,
    }
}

/// An L profile: a base `x` long and `t` thick along the x axis, a leg `y` high and
/// `leg` thick at its end. With `round`, the corner has an inside radius `R` and an
/// outside radius `R + t` (concentric when `leg == t`).
fn l_profile(x: f64, y: f64, t: f64, leg: f64, round: bool) -> Vec<Curve> {
    if !round {
        return vec![
            line((0.0, 0.0), (x, 0.0)),
            line((x, 0.0), (x, y)),
            line((x, y), (x - leg, y)),
            line((x - leg, y), (x - leg, t)),
            line((x - leg, t), (0.0, t)),
            line((0.0, t), (0.0, 0.0)),
        ];
    }
    let outer = (x - R - t, R + t);
    let inner = (x - leg - R, t + R);
    vec![
        line((0.0, 0.0), (outer.0, 0.0)),
        quarter(outer, R + t, -FRAC_PI_2),
        line((x, outer.1), (x, y)),
        line((x, y), (x - leg, y)),
        line((x - leg, y), (x - leg, inner.1)),
        quarter(inner, R, -FRAC_PI_2),
        line((inner.0, t), (0.0, t)),
        line((0.0, t), (0.0, 0.0)),
    ]
}

/// The profile extruded 50 mm from the front plane.
fn extruded(curves: &[Curve]) -> Solid {
    let all = regions(curves);
    // The material is the region with the largest outline that has the others as holes
    // (a tube), or the only region.
    let region = all
        .iter()
        .find(|r| !r.holes.is_empty())
        .unwrap_or(&all[0])
        .clone();
    extrude(&Plane::front(), &[region], 0.0, 50.0).unwrap()
}

#[test]
fn extruded_l_with_rounded_corner() {
    // One edge face runs along both walls and the bend, as in a solid from any CAD
    // system.
    let solid = extruded(&l_profile(40.0, 30.0, T, T, true));
    assert_eq!(solid.faces.len(), 10);
    let rec = recognized(&solid, None);
    let found = bends(&rec.body.layout);
    assert_eq!(found.len(), 1);
    assert_close(found[0].0, 90.0, 1e-9);
    assert_close(found[0].1, R, 1e-9);
    assert_close(rec.body.layout.settings.thickness, T, 1e-12);
    // Flat length: the two flats plus the bend allowance.
    let ba = FRAC_PI_2 * (R + K * T);
    let (a, b) = blank(&rec.body);
    assert_close(a, 50.0, 1e-9);
    assert_close(b, (40.0 - R - T) + ba + (30.0 - R - T), 1e-9);
    assert_close(measure::volume(&rec.solid), measure::volume(&solid), 1e-6);
    // The sheet metal body has its own faces for every wall of every piece.
    assert_eq!(rec.solid.faces.len(), 14);
}

#[test]
fn refuses_walls_of_different_thickness() {
    let solid = extruded(&l_profile(40.0, 30.0, T, 3.0, true));
    let m = refusal(&solid, None);
    assert!(m.contains("not all the same thickness"), "{m}");
    assert!(m.contains("2.000") && m.contains("3.000"), "{m}");
}

#[test]
fn refuses_sharp_corners() {
    let solid = extruded(&l_profile(40.0, 30.0, T, T, false));
    let m = refusal(&solid, None);
    assert!(m.contains("not a side of the sheet"), "{m}");
    assert!(m.contains("round them first"), "{m}");
}

#[test]
fn refuses_a_closed_box_section() {
    // A 40 × 30 tube with rounded corners: outside radius R + T, inside R.
    let ring = |inset: f64, r: f64| {
        let (x0, y0, x1, y1) = (inset, inset, 40.0 - inset, 30.0 - inset);
        vec![
            line((x0 + r, y0), (x1 - r, y0)),
            quarter((x1 - r, y0 + r), r, -FRAC_PI_2),
            line((x1, y0 + r), (x1, y1 - r)),
            quarter((x1 - r, y1 - r), r, 0.0),
            line((x1 - r, y1), (x0 + r, y1)),
            quarter((x0 + r, y1 - r), r, FRAC_PI_2),
            line((x0, y1 - r), (x0, y0 + r)),
            quarter((x0 + r, y0 + r), r, 2.0 * FRAC_PI_2),
        ]
    };
    let mut curves = ring(0.0, R + T);
    curves.extend(ring(T, R));
    let solid = extruded(&curves);
    let m = refusal(&solid, None);
    assert!(m.contains("closed loop"), "{m}");
    assert!(m.contains("Cut a seam"), "{m}");
}

#[test]
fn refuses_several_lumps() {
    let mut layout = plate();
    layout.add_cut(Area::polygon(
        &[
            DVec2::new(45.0, -5.0),
            DVec2::new(55.0, -5.0),
            DVec2::new(55.0, H + 5.0),
            DVec2::new(45.0, H + 5.0),
        ],
        |i| CurveTag::Sketch {
            owner: 2,
            entity: i as u32,
        },
    ));
    let (solid, _) = native(layout);
    assert_eq!(solid.shells.len(), 2);
    let m = refusal(&solid, None);
    assert!(m.contains("2 separate pieces"), "{m}");
}

/// An L bracket whose bend faces can be swapped for other surfaces.
fn bracket() -> (Solid, SheetBody) {
    let mut layout = plate();
    layout
        .add_edge_flange(2, &edge(2, true), &flange(25.0, 90.0))
        .unwrap();
    native(layout)
}

#[test]
fn refuses_conical_bends() {
    let (mut solid, body) = bracket();
    // Both sides of the bend become cones, so nearly cylindrical that they are still
    // smooth with the walls.
    for (i, tag) in body.faces.iter().enumerate() {
        if !matches!(
            tag,
            FaceTag::Top { piece: 1 } | FaceTag::Bottom { piece: 1 }
        ) {
            continue;
        }
        let Surface::Cylinder(c) = solid.faces[i].surface.clone() else {
            panic!("a bend face is a cylinder");
        };
        solid.faces[i].surface = Surface::Cone(Cone {
            frame: c.frame,
            radius: c.radius,
            half_angle: 1e-9,
        });
    }
    let m = refusal(&solid, Some(fixed_face(&body)));
    assert!(m.contains("conical"), "{m}");
    assert!(m.contains("cylindrical bends"), "{m}");
}

#[test]
fn refuses_faces_that_are_not_sheet() {
    // A doubly curved face where an edge face was.
    let (mut solid, body) = bracket();
    let wall = body
        .faces
        .iter()
        .position(|f| matches!(f, FaceTag::Wall { piece: 2, .. }))
        .unwrap();
    solid.faces[wall].surface = Surface::Torus(Torus {
        frame: Frame::WORLD,
        major: 500.0,
        minor: 100.0,
    });
    let m = refusal(&solid, Some(fixed_face(&body)));
    assert!(m.contains("doubly curved"), "{m}");

    // No flat face at all.
    let ball = peet_kernel::primitive::ball(&Frame::WORLD, 10.0).unwrap();
    let m = refusal(&ball, None);
    assert!(m.contains("no flat face"), "{m}");

    // A fixed face that is a bend, or not on the body.
    let (solid, body) = bracket();
    let bend = body
        .faces
        .iter()
        .position(|f| matches!(f, FaceTag::Top { piece: 1 }))
        .unwrap();
    let m = refusal(&solid, Some(FaceId(bend as u32)));
    assert!(m.contains("must be flat"), "{m}");
    let m = refusal(&solid, Some(FaceId(10_000)));
    assert!(m.contains("not on this body"), "{m}");
}

#[test]
fn a_block_is_a_thick_plate() {
    // Nothing says how thin sheet metal must be: a block is a plate as thick as the
    // distance behind its fixed face.
    let solid = peet_kernel::primitive::cuboid(DVec3::ZERO, DVec3::new(30.0, 20.0, 10.0));
    let rec = recognized(&solid, None);
    assert_close(rec.body.layout.settings.thickness, 10.0, 1e-12);
    let (a, b) = blank(&rec.body);
    assert_close(a, 20.0, 1e-9);
    assert_close(b, 30.0, 1e-9);
}

#[test]
fn names_are_unique_and_stable() {
    let (solid, body) = bracket();
    let a = recognized(&solid, Some(fixed_face(&body)));
    let b = recognized(&solid, Some(fixed_face(&body)));
    assert_eq!(a.body.layout, b.body.layout);
    // Every wall has a tag of its own.
    let mut tags: Vec<CurveTag> = a
        .body
        .faces
        .iter()
        .filter_map(|f| match f {
            FaceTag::Wall { tag, .. } => Some(*tag),
            _ => None,
        })
        .collect();
    let n = tags.len();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags.len(), n);
    assert!(tags.iter().all(|t| t.owner() == OWNER));
}

#[test]
fn the_k_factor_only_changes_the_flat_pattern() {
    let (solid, body) = bracket();
    let fixed = Some(fixed_face(&body));
    let size = |k: f64| {
        let hint = SheetSettings {
            model: BendModel::KFactor(k),
            ..hint()
        };
        let rec = recognize(&solid, fixed, &hint, OWNER).unwrap();
        assert_close(measure::volume(&rec.solid), measure::volume(&solid), 1e-6);
        blank(&rec.body).0
    };
    assert_close(size(0.5) - size(0.3), FRAC_PI_2 * 0.2 * T, 1e-9);
    // A fixed allowance or deduction works too.
    for model in [BendModel::Allowance(7.0), BendModel::Deduction(4.0)] {
        let hint = SheetSettings { model, ..hint() };
        let rec = recognize(&solid, fixed, &hint, OWNER).unwrap();
        let b = rec.body.layout.bends().next().unwrap().1;
        match model {
            BendModel::Allowance(v) => assert_close(b.values.allowance, v, 1e-12),
            BendModel::Deduction(v) => assert_close(b.values.deduction(), v, 1e-9),
            BendModel::KFactor(_) => unreachable!(),
        }
    }
}
