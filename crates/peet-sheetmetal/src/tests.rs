//! Scenario tests: plates, flanges, reliefs, cuts and open profiles, checked for validity
//! and against hand-calculated sizes.

use std::f64::consts::{FRAC_PI_2, PI};

use peet_kernel::Solid;
use peet_kernel::validate::{measure, validate};
use peet_math::{Aabb, DVec2, DVec3, Plane};
use peet_sketch::region::find_regions;
use peet_sketch::{Curve, Sketch, shapes};

use super::*;
use crate::corner;
use crate::flange::CLOSED_HEM_RADIUS;
use crate::layout::wall;

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

fn plate_layout(s: SheetSettings) -> Layout {
    let mut sk = Sketch::new();
    shapes::rectangle(&mut sk, DVec2::ZERO, DVec2::new(W, H));
    let regions = find_regions(&sk).regions;
    Layout::plate(s, 1, &Plane::TOP, &regions, false).unwrap()
}

#[track_caller]
fn built(layout: Layout) -> (Solid, SheetBody) {
    let (solid, body) = match build(layout) {
        Ok(b) => b,
        Err(e) => panic!("build failed: {}", e.message(|o| format!("owner {o}"))),
    };
    if let Err(p) = validate(&solid) {
        panic!("folded invalid: {p:#?}");
    }
    if let Err(p) = validate(&body.flat) {
        panic!("flat invalid: {p:#?}");
    }
    assert_eq!(solid.faces.len(), body.flat.faces.len());
    assert_eq!(solid.faces.len(), body.faces.len());
    (solid, body)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[track_caller]
fn assert_close(a: f64, b: f64, tol: f64) {
    assert!(close(a, b, tol), "{a} != {b} (±{tol})");
}

/// The plate's edge on the line `y = H` (the top edge in the sketch), material below it.
fn top_edge(top: bool) -> EdgeSite {
    EdgeSite {
        piece: 0,
        a: DVec2::new(W, H),
        b: DVec2::new(0.0, H),
        top,
    }
}

fn flange(length: f64, position: FlangePosition) -> EdgeFlangeSpec {
    EdgeFlangeSpec {
        length,
        angle: 90.0,
        position,
        offsets: [0.0, 0.0],
        flip: false,
        radius: None,
    }
}

/// Exact bounds from the vertices plus dense samples of curved edges.
fn bounds(s: &Solid) -> Aabb {
    let mut b = Aabb::EMPTY;
    for v in &s.vertices {
        b.extend(v.point);
    }
    for e in &s.edges {
        for i in 0..=256 {
            b.extend(e.point_at_fraction(f64::from(i) / 256.0));
        }
    }
    b
}

fn flat_area(body: &SheetBody) -> f64 {
    measure::volume(&body.flat) / T
}

#[test]
fn plate() {
    let (solid, body) = built(plate_layout(settings(0.44)));
    assert_eq!(solid.faces.len(), 6);
    assert_close(measure::volume(&solid), W * H * T, 1e-9);
    assert_eq!(solid, body.flat);
    assert_eq!(body.outline.len(), 1);
    assert!(body.outline[0].outer);
    let (lo, hi) = body.flat_bounds();
    assert!(lo.abs_diff_eq(DVec2::ZERO, 1e-12) && hi.abs_diff_eq(DVec2::new(W, H), 1e-12));
}

#[test]
fn material_inside_flange_keeps_the_outside_size() {
    let s = settings(0.44);
    let mut layout = plate_layout(s);
    let l = 20.0;
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(l, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-9), "{:?}", b.min);
    assert!(b.max.abs_diff_eq(DVec3::new(W, H, l), 1e-9), "{:?}", b.max);
    // Flat: H + L - BD along y.
    let v = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap();
    let (lo, hi) = body.flat_bounds();
    assert_close(hi.y - lo.y, H + l - v.deduction(), 1e-9);
    assert_close(hi.x - lo.x, W, 1e-9);
    // One bend line, across the full width, in the middle of the bend region.
    assert_eq!(body.bend_lines.len(), 1);
    let seg = body.bend_lines[0].segments.clone();
    assert_eq!(seg.len(), 1);
    assert_close(seg[0][0].distance(seg[0][1]), W, 1e-9);
    assert_close(
        seg[0][0].y,
        H - v.outside_setback() + v.allowance / 2.0,
        1e-9,
    );
}

#[test]
fn positions() {
    for (position, y_max) in [
        (FlangePosition::MaterialInside, H),
        (FlangePosition::MaterialOutside, H + T),
        (FlangePosition::BendOutside, H + R + T),
    ] {
        let mut layout = plate_layout(settings(0.44));
        layout
            .add_edge_flange(2, &top_edge(true), &flange(15.0, position))
            .unwrap();
        let (solid, _) = built(layout);
        let b = bounds(&solid);
        assert_close(b.max.y, y_max, 1e-9);
        assert_close(b.max.z, 15.0, 1e-9);
    }
}

#[test]
fn downward_flange() {
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_edge_flange(
            2,
            &top_edge(false),
            &flange(12.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, _) = built(layout);
    let b = bounds(&solid);
    assert_close(b.min.z, T - 12.0, 1e-9);
    assert_close(b.max.z, T, 1e-9);
    assert_close(b.max.y, H, 1e-9);
}

#[test]
fn half_k_factor_keeps_the_volume() {
    // With K = 0.5 the neutral axis is the mid-surface, so folding keeps the volume.
    for angle in [30.0, 90.0, 135.0] {
        let mut layout = plate_layout(settings(0.5));
        let mut spec = flange(25.0, FlangePosition::MaterialOutside);
        spec.angle = angle;
        layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
        let (solid, body) = built(layout);
        let folded = measure::volume(&solid);
        let flat = measure::volume(&body.flat);
        assert_close(folded, flat, 1e-6 * flat);
    }
}

#[test]
fn offsets_with_each_relief() {
    let s = settings(0.44);
    let v = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap();
    let (l, off) = (20.0, 10.0);
    let span = W - 2.0 * off;
    let trim = v.outside_setback();
    let straight = l - trim;
    let base = W * H - span * trim + span * (v.allowance + straight);
    let rw = 0.5 * T;
    let depth = trim + 0.5 * T;
    for (relief, removed) in [
        (ReliefType::Tear, 0.0),
        (ReliefType::Rectangular, 2.0 * rw * depth),
        (
            ReliefType::Obround,
            2.0 * (rw * (depth - rw / 2.0) + PI * (rw / 2.0).powi(2) / 2.0),
        ),
    ] {
        let mut st = s;
        st.relief = relief;
        let mut layout = plate_layout(st);
        let mut spec = flange(l, FlangePosition::MaterialInside);
        spec.offsets = [off, off];
        layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
        let (solid, body) = built(layout);
        assert_close(flat_area(&body), base - removed, 1e-6);
        let b = bounds(&solid);
        assert_close(b.max.z, l, 1e-9);
        // The flange is narrower than the plate.
        let tip: Vec<&DVec3> = solid
            .vertices
            .iter()
            .map(|v| &v.point)
            .filter(|p| close(p.z, l, 1e-9))
            .collect();
        assert!(
            tip.iter()
                .all(|p| p.x >= off - 1e-9 && p.x <= W - off + 1e-9)
        );
    }
}

#[test]
fn flange_names_its_faces() {
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_edge_flange(
            7,
            &top_edge(true),
            &flange(20.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, body) = built(layout);
    // Two cylinders (the bend's two sides), a tip wall, and the plate's faces.
    let cylinders = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, peet_kernel::Surface::Cylinder(_)))
        .count();
    assert_eq!(cylinders, 2);
    let tips: Vec<usize> = body
        .faces
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            matches!(
                t,
                FaceTag::Wall {
                    tag: CurveTag::Generated {
                        owner: 7,
                        index: wall::TIP,
                        ..
                    },
                    ..
                }
            )
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(tips.len(), 1);
    // The tip is at the top of the flange, facing up.
    let f = peet_kernel::FaceId(tips[0] as u32);
    let p = solid.vertices[solid
        .edge(solid.coedge(solid.loop_(solid.face(f).loops[0]).first).edge)
        .start
        .index()]
    .point;
    assert_close(p.z, 20.0, 1e-9);
    assert!(solid.face_normal_at(f, p).abs_diff_eq(DVec3::Z, 1e-9));
}

#[test]
fn flange_on_a_flange() {
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(20.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, body) = built(layout.clone());
    // The tip edge on the inner side (the top side of the sheet faces the plate).
    let site = solid
        .edge_ids()
        .filter_map(|e| body.edge_site(&solid, e).ok())
        .find(|s| {
            let fl = &body.layout.pieces[2];
            let (p, q) = (
                fl.frame.to_world(s.a.extend(0.0)),
                fl.frame.to_world(s.b.extend(0.0)),
            );
            s.piece == 2 && s.top && close(p.z, 20.0, 1e-9) && close(q.z, 20.0, 1e-9)
        })
        .expect("the flange's tip edge");
    layout
        .add_edge_flange(3, &site, &flange(10.0, FlangePosition::MaterialInside))
        .unwrap();
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    // The second flange turns back over the plate (towards -y), staying under z = 20.
    assert_close(b.max.z, 20.0, 1e-9);
    assert_close(b.max.y, H, 1e-9);
    assert_close(b.min.y, 0.0, 1e-9);
    assert_eq!(body.bend_lines.len(), 2);
    let v = BendValues::new(BendModel::KFactor(0.44), FRAC_PI_2, R, T).unwrap();
    let (lo, hi) = body.flat_bounds();
    assert_close(hi.y - lo.y, H + 20.0 + 10.0 - 2.0 * v.deduction(), 1e-9);
}

#[test]
fn cut_across_a_bend() {
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    // A slot square to the bend, from the plate across the bend into the flange.
    let slot = Area::polygon(
        &[
            DVec2::new(40.0, 45.0),
            DVec2::new(50.0, 45.0),
            DVec2::new(50.0, 70.0),
            DVec2::new(40.0, 70.0),
        ],
        |i| CurveTag::Sketch {
            owner: 3,
            entity: i as u32,
        },
    );
    let mut before = layout.clone();
    layout.add_cut(slot.clone());
    let (solid, body) = built(layout);
    let (_, full) = built(before.clone());
    assert_close(flat_area(&full) - flat_area(&body), 250.0, 1e-6);
    assert_close(measure::volume(&solid), measure::volume(&body.flat), 1e-6);
    // The bend line is split in two by the slot.
    assert_eq!(body.bend_lines[0].segments.len(), 2);
    // A cut made before the flange doesn't cut the flange.
    let mut early = plate_layout(settings(0.5));
    early.add_cut(slot);
    early
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (_, e) = built(early);
    assert!(flat_area(&e) > flat_area(&body) + 1.0);
    before.cuts.clear();
}

#[test]
fn cuts_through_a_whole_bend_leave_its_strips() {
    // Two cuts that each take the bend away over its whole width leave it in three
    // strips. Each still joins the plate to the flange: none is scrap.
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let bend = layout
        .pieces
        .iter()
        .find(|p| !p.is_flange())
        .expect("the bend");
    let ys: Vec<f64> = bend
        .outline
        .edges()
        .flat_map(|e| [e.curve.start().y, e.curve.end().y])
        .collect();
    let (y0, y1) = ys
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &y| {
            (a.min(y), b.max(y))
        });
    let (_, full) = built(layout.clone());
    for (k, x) in [20.0, 60.0].into_iter().enumerate() {
        layout.add_cut(Area::polygon(
            &[
                DVec2::new(x, y0),
                DVec2::new(x + 10.0, y0),
                DVec2::new(x + 10.0, y1),
                DVec2::new(x, y1),
            ],
            |i| CurveTag::Sketch {
                owner: 3 + k as u32,
                entity: i as u32,
            },
        ));
    }
    let (solid, body) = built(layout);
    assert_close(
        flat_area(&full) - flat_area(&body),
        2.0 * 10.0 * (y1 - y0),
        1e-6,
    );
    let flat = measure::volume(&body.flat);
    assert_close(measure::volume(&solid), flat, flat * 2e-5);
    assert_eq!(body.bend_lines[0].segments.len(), 3);
}

/// A plate with a flange on its top edge and `cut` made after it, at K = 0.5.
fn cut_across_the_bend(cut: Area) -> (Solid, SheetBody) {
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    layout.add_cut(cut);
    built(layout)
}

#[test]
fn round_hole_across_a_bend() {
    // A hole that straddles the bend line: part of its rim is on the plate, part on the
    // bend and part on the flange. On the bend the rim is neither a line nor a circle
    // once folded, and its wall is not flat.
    let hole = Area {
        loops: vec![vec![Edge2 {
            curve: Curve::Circle {
                center: DVec2::new(30.0, H - 3.0),
                radius: 4.0,
            },
            reversed: false,
            tag: CurveTag::Sketch {
                owner: 9,
                entity: 1,
            },
        }]],
    };
    let (solid, body) = cut_across_the_bend(hole);
    // With K = 0.5 the neutral axis is the mid-surface, so folding keeps the volume: the
    // folded part has what the blank has, hole and all.
    let flat = measure::volume(&body.flat);
    assert_close(measure::volume(&solid), flat, flat * 2e-5);
    // The blank has the hole whole.
    let holes: Vec<_> = body.outline.iter().filter(|l| !l.outer).collect();
    assert_eq!(holes.len(), 1);
    // And some of the folded part's edges are freeform now.
    assert!(
        solid
            .edges
            .iter()
            .any(|e| matches!(e.curve, peet_kernel::geom::Curve3::Nurbs(_)))
    );
}

#[test]
fn slanted_cut_across_a_bend() {
    // A triangle with one corner on the plate and a side on the flange: two of its sides
    // cross the bend on a slant.
    let tag = |entity| CurveTag::Sketch { owner: 9, entity };
    let (a, b, c) = (
        DVec2::new(30.0, H - 8.0),
        DVec2::new(44.0, H + 12.0),
        DVec2::new(20.0, H + 12.0),
    );
    let triangle = Area {
        loops: vec![vec![
            Edge2::line(a, b, tag(1)),
            Edge2::line(b, c, tag(2)),
            Edge2::line(c, a, tag(3)),
        ]],
    };
    let (solid, body) = cut_across_the_bend(triangle);
    let flat = measure::volume(&body.flat);
    assert_close(measure::volume(&solid), flat, flat * 2e-5);
    assert_eq!(body.outline.iter().filter(|l| !l.outer).count(), 1);
}

#[test]
fn flange_past_the_end_of_its_edge() {
    // Set back 5 from the start of its edge and running 10 past the end: the bend is on
    // the edge, from 5 to its end, and the flange is 10 wider than that.
    let mut layout = plate_layout(settings(0.5));
    let mut spec = flange(25.0, FlangePosition::MaterialInside);
    spec.offsets = [5.0, -10.0];
    layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    // The top edge runs from x = W to x = 0, so its end is at x = 0.
    assert_close(b.min.x, -10.0, 1e-9);
    assert_close(b.max.x, W, 1e-9);
    assert_eq!(body.bend_lines.len(), 1);
    // One blank, and folding keeps its volume (K = 0.5).
    assert_eq!(body.outline.iter().filter(|l| l.outer).count(), 1);
    let flat = measure::volume(&body.flat);
    assert_close(measure::volume(&solid), flat, flat * 1e-9);
    // The flange is the edge's length less the set-back plus the overrun; the bend is
    // only the part on the edge.
    let flanges: Vec<usize> = (0..body.layout.pieces.len())
        .filter(|&i| body.layout.pieces[i].is_flange() && i > 0)
        .collect();
    let width = |i: usize| {
        let p = piece_bounds(&solid, &body, i);
        p.max.x - p.min.x
    };
    assert_close(width(flanges[0]), W - 5.0 + 10.0, 1e-9);
}

#[test]
fn a_bend_ends_on_a_slant_at_a_sharp_corner() {
    // A right triangle with a flange on its long side, material outside: the bend starts
    // inside the sheet, where the triangle is narrower than the side is long. Its ends
    // run from the triangle's corners there to the ends of the flange, as SolidWorks
    // makes them: the blank has no step at the ends of the bend, and folding keeps its
    // volume (K = 0.5).
    let mut sk = Sketch::new();
    let pts = [DVec2::ZERO, DVec2::new(100.0, 0.0), DVec2::new(0.0, 100.0)];
    for i in 0..3 {
        sk.add_line(pts[i], pts[(i + 1) % 3]);
    }
    let regions = find_regions(&sk).regions;
    let mut layout = Layout::plate(settings(0.5), 1, &Plane::TOP, &regions, false).unwrap();
    let site = EdgeSite {
        piece: 0,
        a: pts[1],
        b: pts[2],
        top: true,
    };
    let own = FlangeOwn {
        relief: Some((ReliefType::Tear, 0.5)),
        ..Default::default()
    };
    layout
        .add_edge_flange_with(
            2,
            &site,
            &flange(20.0, FlangePosition::MaterialOutside),
            own,
        )
        .unwrap();
    let (solid, body) = built(layout);
    let slanted = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, peet_kernel::geom::Surface::Nurbs(_)))
        .count();
    assert_eq!(slanted, 2);
    // The blank's outline: the triangle's two short sides, two slanted bend ends and the
    // flange's three sides; nothing else.
    assert_eq!(body.outline.len(), 1);
    assert_eq!(body.outline[0].edges.len(), 7);
    assert_close(
        measure::volume(&solid),
        measure::volume(&body.flat),
        2e-5 * measure::volume(&body.flat),
    );
}

#[test]
fn a_flange_with_its_own_relief() {
    // Set back 10 from both ends, material outside, so that the bend takes a strip of
    // the plate. With the body's rectangular relief the plate is notched at each end of
    // the bend; with a tear relief of the flange's own nothing is cut from it, and with a
    // ratio of its own the notches are that much bigger. Bend outside, the bend takes
    // nothing from the plate and no relief is cut, as in SolidWorks.
    let area = |relief: Option<(ReliefType, f64)>, position: FlangePosition| {
        let mut layout = plate_layout(settings(0.44));
        let mut spec = flange(25.0, position);
        spec.offsets = [10.0, 10.0];
        layout
            .add_edge_flange_with(
                2,
                &top_edge(true),
                &spec,
                FlangeOwn {
                    relief,
                    ..Default::default()
                },
            )
            .unwrap();
        let (_, body) = built(layout);
        flat_area(&body)
    };
    let s = settings(0.44);
    let ratio = s.relief_ratio;
    let trim = BendValues::new(s.model, FRAC_PI_2, R, T)
        .unwrap()
        .inside_setback();
    let inside = FlangePosition::MaterialOutside;
    let body_default = area(None, inside);
    let tear = area(Some((ReliefType::Tear, ratio)), inside);
    let wide = area(Some((ReliefType::Rectangular, 2.0 * ratio)), inside);
    // Two notches, each ratio·t wide and ratio·t past the strip the bend takes.
    assert_close(
        tear - body_default,
        2.0 * (ratio * T) * (trim + ratio * T),
        1e-9,
    );
    assert_close(
        tear - wide,
        2.0 * (2.0 * ratio * T) * (trim + 2.0 * ratio * T),
        1e-9,
    );
    let outside = FlangePosition::BendOutside;
    assert_close(
        area(None, outside),
        area(Some((ReliefType::Tear, ratio)), outside),
        1e-9,
    );
    let mut layout = plate_layout(settings(0.44));
    let spec = flange(25.0, FlangePosition::BendOutside);
    let own = FlangeOwn {
        relief: Some((ReliefType::Tear, 0.0)),
        ..Default::default()
    };
    let e = layout
        .add_edge_flange_with(2, &top_edge(true), &spec, own)
        .unwrap_err();
    assert!(e.contains("ratio"), "{e}");
}

#[test]
fn a_flange_with_its_own_bend_model() {
    // The body's bends are worked out with a K-factor. A flange with a bend deduction of
    // its own folds to the same part, from a blank that much shorter or longer.
    let size = |model: Option<BendModel>| {
        let mut layout = plate_layout(settings(0.44));
        let spec = flange(25.0, FlangePosition::MaterialInside);
        let own = FlangeOwn {
            model,
            ..Default::default()
        };
        layout
            .add_edge_flange_with(2, &top_edge(true), &spec, own)
            .unwrap();
        let (solid, body) = built(layout);
        let allowance = body.layout.bends().next().unwrap().1.values.allowance;
        (bounds(&solid), allowance)
    };
    let (plain, by_k) = size(None);
    let deduction = 2.0 * (R + T) - by_k;
    let (same, a) = size(Some(BendModel::Deduction(deduction)));
    assert_close(a, by_k, 1e-12);
    let (other, b) = size(Some(BendModel::Deduction(deduction - 0.5)));
    assert_close(b, by_k + 0.5, 1e-12);
    for folded in [same, other] {
        assert!(folded.min.distance(plain.min) < 1e-9 && folded.max.distance(plain.max) < 1e-9);
    }
}

#[test]
fn round_hole_in_a_flange() {
    let mut layout = plate_layout(settings(0.44));
    layout.add_cut(Area {
        loops: vec![vec![Edge2 {
            curve: Curve::Circle {
                center: DVec2::new(30.0, 20.0),
                radius: 4.0,
            },
            reversed: false,
            tag: CurveTag::Sketch {
                owner: 9,
                entity: 1,
            },
        }]],
    });
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, body) = built(layout);
    let holes = body.outline.iter().filter(|l| !l.outer).count();
    assert_eq!(holes, 1);
    assert!(solid.faces.len() > 10);
}

#[test]
fn two_flanges_on_one_edge_overlap() {
    let mut layout = plate_layout(settings(0.44));
    let spec = flange(20.0, FlangePosition::BendOutside);
    layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
    layout.add_edge_flange(3, &top_edge(true), &spec).unwrap();
    let err = build(layout).unwrap_err();
    assert!(matches!(err, SheetError::Overlap { .. }), "{err:?}");
}

#[test]
fn four_flanges_with_open_corners() {
    // An enclosure panel: flanges on every edge, set back at the ends so the corners stay
    // open, with reliefs.
    let s = settings(0.44);
    let edges = [
        (DVec2::new(0.0, 0.0), DVec2::new(W, 0.0)),
        (DVec2::new(W, 0.0), DVec2::new(W, H)),
        (DVec2::new(W, H), DVec2::new(0.0, H)),
        (DVec2::new(0.0, H), DVec2::new(0.0, 0.0)),
    ];
    let panel = |off: f64| {
        let mut layout = plate_layout(s);
        let mut spec = flange(20.0, FlangePosition::MaterialInside);
        spec.offsets = [off, off];
        for (i, (a, b)) in edges.into_iter().enumerate() {
            layout
                .add_edge_flange(
                    10 + i as u32,
                    &EdgeSite {
                        piece: 0,
                        a,
                        b,
                        top: true,
                    },
                    &spec,
                )
                .unwrap();
        }
        layout
    };
    // Set back less than the bend zone (R + t): the bends collide in the corners.
    let err = build(panel(T + 1.0)).unwrap_err();
    assert!(matches!(err, SheetError::Overlap { .. }), "{err:?}");
    // Set back just past the bend zone: the reliefs of neighbouring flanges meet and cut
    // the corner of the sheet off. That scrap is left out: the corners are notched, the
    // sheet is one piece, and every bend is as long as its edge less the two set-backs.
    for off in [R + T + 1.0, 5.0] {
        let (solid, notched) = built(panel(off));
        assert_eq!(
            notched.outline.iter().filter(|l| l.outer).count(),
            1,
            "{off}"
        );
        validate(&solid).expect("a valid solid");
        let mut lengths: Vec<f64> = notched
            .bend_lines
            .iter()
            .map(|l| l.segments.iter().map(|s| s[0].distance(s[1])).sum())
            .collect();
        lengths.sort_by(f64::total_cmp);
        let (short, long) = (H - 2.0 * off, W - 2.0 * off);
        for (got, want) in lengths.iter().zip([short, short, long, long]) {
            assert_close(*got, want, 1e-9);
        }
    }
    let (solid, body) = built(panel(10.0));
    let b = bounds(&solid);
    assert!(
        b.min.abs_diff_eq(DVec3::ZERO, 1e-9) && b.max.abs_diff_eq(DVec3::new(W, H, 20.0), 1e-9)
    );
    assert_eq!(body.bend_lines.len(), 4);
    let v = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap();
    let (lo, hi) = body.flat_bounds();
    assert_close(hi.x - lo.x, W + 40.0 - 2.0 * v.deduction(), 1e-9);
    assert_close(hi.y - lo.y, H + 40.0 - 2.0 * v.deduction(), 1e-9);
    assert_eq!(body.outline.iter().filter(|l| l.outer).count(), 1);
}

#[test]
fn u_channel_from_an_open_profile() {
    let s = settings(0.5);
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
    let plane = Plane::front();
    let layout = Layout::open_profile(s, 1, &plane, &chain, [0.0, 100.0], false).unwrap();
    let (solid, body) = built(layout);
    // The lines are the outside of the channel: it fits in the 60 × 40 box.
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    let mut depth = (f64::INFINITY, f64::NEG_INFINITY);
    for e in &solid.edges {
        for i in 0..=64 {
            let p = e.point_at_fraction(f64::from(i) / 64.0);
            let q = plane.to_plane_coords(p);
            lo = lo.min(q);
            hi = hi.max(q);
            let d = plane.signed_distance(p);
            depth = (depth.0.min(d), depth.1.max(d));
        }
    }
    assert!(lo.abs_diff_eq(DVec2::ZERO, 1e-9), "{lo}");
    assert!(hi.abs_diff_eq(DVec2::new(60.0, 40.0), 1e-9), "{hi}");
    assert_close(depth.0, 0.0, 1e-9);
    assert_close(depth.1, 100.0, 1e-9);
    let v = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap();
    let (flo, fhi) = body.flat_bounds();
    assert_close(fhi.x - flo.x, 140.0 - 2.0 * v.deduction(), 1e-9);
    assert_close(measure::volume(&solid), measure::volume(&body.flat), 1e-6);
    // Flipped: the lines are the inside.
    let layout = Layout::open_profile(s, 1, &plane, &chain, [0.0, 100.0], true).unwrap();
    let (solid, _) = built(layout);
    let b = bounds(&solid);
    let size = b.max - b.min;
    assert_close(size.x, 60.0 + 2.0 * T, 1e-9);
}

#[test]
fn open_profile_errors() {
    let s = settings(0.44);
    let short = [
        ChainLine {
            entity: 1,
            a: DVec2::new(0.0, 2.0),
            b: DVec2::ZERO,
        },
        ChainLine {
            entity: 2,
            a: DVec2::ZERO,
            b: DVec2::new(50.0, 0.0),
        },
    ];
    let err = Layout::open_profile(s, 1, &Plane::TOP, &short, [0.0, 10.0], false).unwrap_err();
    assert!(err.contains("too short"), "{err}");
    let straight = [
        ChainLine {
            entity: 1,
            a: DVec2::ZERO,
            b: DVec2::new(10.0, 0.0),
        },
        ChainLine {
            entity: 2,
            a: DVec2::new(10.0, 0.0),
            b: DVec2::new(20.0, 0.0),
        },
    ];
    let err = Layout::open_profile(s, 1, &Plane::TOP, &straight, [0.0, 10.0], false).unwrap_err();
    assert!(err.contains("in line"), "{err}");
}

#[test]
fn flange_errors() {
    let mut layout = plate_layout(settings(0.44));
    let err = layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(3.0, FlangePosition::MaterialInside),
        )
        .unwrap_err();
    assert!(err.contains("too short"), "{err}");
    let mut spec = flange(20.0, FlangePosition::MaterialInside);
    spec.offsets = [60.0, 50.0];
    let err = layout
        .add_edge_flange(2, &top_edge(true), &spec)
        .unwrap_err();
    assert!(err.contains("leave nothing"), "{err}");
    spec.offsets = [0.0, 0.0];
    spec.angle = 180.0;
    assert!(layout.add_edge_flange(2, &top_edge(true), &spec).is_err());
}

#[test]
fn folded_bodies_tessellate() {
    let mut layout = plate_layout(settings(0.44));
    let mut spec = flange(20.0, FlangePosition::MaterialInside);
    spec.offsets = [10.0, 5.0];
    layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
    let (solid, body) = built(layout);
    for s in [&solid, &body.flat] {
        let mesh = peet_kernel::tessellate::tessellate(s, 0.01).unwrap();
        let area: f64 = mesh.faces.iter().map(|f| f.area()).sum();
        let exact: f64 = s.face_ids().map(|f| measure::face_area(s, f)).sum();
        assert_close(area, exact, 1e-3 * exact);
    }
}

mod random {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Any flange either builds a valid body or fails with a message; with K = 0.5 the
        /// folded volume equals the flat one.
        #[test]
        fn flanges_build_or_fail_cleanly(
            length in 1.0f64..60.0,
            angle in 1.0f64..179.0,
            pos in 0usize..3,
            off0 in 0.0f64..40.0,
            off1 in 0.0f64..40.0,
            top in any::<bool>(),
            flip in any::<bool>(),
            relief in 0usize..3,
            radius in 0.0f64..8.0,
            edge in 0usize..4,
        ) {
            let mut s = settings(0.5);
            s.relief = ReliefType::ALL[relief];
            let mut layout = plate_layout(s);
            let edges = [
                (DVec2::new(0.0, 0.0), DVec2::new(W, 0.0)),
                (DVec2::new(W, 0.0), DVec2::new(W, H)),
                (DVec2::new(W, H), DVec2::new(0.0, H)),
                (DVec2::new(0.0, H), DVec2::new(0.0, 0.0)),
            ];
            let (a, b) = edges[edge];
            let spec = EdgeFlangeSpec {
                length,
                angle,
                position: FlangePosition::ALL[pos],
                offsets: [off0, off1],
                flip,
                radius: Some(radius),
            };
            if layout.add_edge_flange(2, &EdgeSite { piece: 0, a, b, top }, &spec).is_err() {
                return Ok(());
            }
            match build(layout) {
                Ok((solid, body)) => {
                    prop_assert!(validate(&solid).is_ok());
                    prop_assert!(validate(&body.flat).is_ok());
                    let (f, v) = (measure::volume(&body.flat), measure::volume(&solid));
                    prop_assert!((f - v).abs() <= 1e-6 * f, "{} vs {}", f, v);
                }
                Err(SheetError::Invalid(m)) => prop_assert!(false, "invalid: {}", m),
                Err(_) => {}
            }
        }
    }
}

fn hem(kind: HemKind, length: f64) -> HemSpec {
    HemSpec {
        kind,
        length,
        gap: 2.0,
        radius: 1.5,
        angle: 270.0,
        inside: false,
        offsets: [0.0, 0.0],
        flip: false,
    }
}

#[test]
fn open_hem_folds_back_over_the_sheet() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    let spec = hem(HemKind::Open, 12.0);
    layout.add_hem(2, &top_edge(true), &spec).unwrap();
    let (solid, body) = built(layout);
    // Gap 2: inner radius 1. The hem lies on top of the sheet, 2 above it.
    let r = 1.0;
    let b = bounds(&solid);
    assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-9), "{:?}", b.min);
    assert!(
        b.max
            .abs_diff_eq(DVec3::new(W, H + r + T, 2.0 * T + 2.0 * r), 1e-9),
        "{:?}",
        b.max
    );
    // The hem's end: 12 from the outside of the fold.
    let lowest_hem_y = solid
        .vertices
        .iter()
        .filter(|v| v.point.z > T + 1e-6)
        .map(|v| v.point.y)
        .fold(f64::INFINITY, f64::min);
    assert_close(lowest_hem_y, H + r + T - 12.0, 1e-9);
    // Flat: the plate, the bend allowance (π · (R + t/2)) and the flat end.
    let ba = PI * (r + T / 2.0);
    let (lo, hi) = body.flat_bounds();
    assert_close(hi.y - lo.y, H + ba + 12.0 - (r + T), 1e-9);
    // K = 0.5 keeps the volume.
    assert_close(measure::volume(&solid), flat_area(&body) * T, 1e-6);
    let report = body.report();
    assert_eq!(report.bends.len(), 1);
    assert_close(report.bends[0].angle, 180.0, 1e-9);
    assert!(report.bends[0].deduction.is_nan());
}

#[test]
fn closed_hem_inside_keeps_the_outline() {
    let mut layout = plate_layout(settings(0.44));
    let mut spec = hem(HemKind::Closed, 10.0);
    spec.inside = true;
    layout.add_hem(2, &top_edge(true), &spec).unwrap();
    let (solid, _) = built(layout);
    let b = bounds(&solid);
    assert_close(b.max.y, H, 1e-9);
    // Flattened onto the sheet: twice the thickness plus a hair.
    let r = CLOSED_HEM_RADIUS * T;
    assert_close(b.max.z, 2.0 * T + 2.0 * r, 1e-9);
}

#[test]
fn every_hem_kind_builds_and_keeps_the_volume() {
    for kind in HemKind::ALL {
        for top in [true, false] {
            for inside in [false, true] {
                let mut layout = plate_layout(settings(0.5));
                let length = if kind == HemKind::Teardrop { 3.0 } else { 10.0 };
                let mut spec = hem(kind, length);
                spec.inside = inside;
                spec.angle = if kind == HemKind::Teardrop {
                    220.0
                } else {
                    270.0
                };
                spec.offsets = [5.0, 7.0];
                layout.add_hem(2, &top_edge(top), &spec).unwrap();
                let (solid, body) = built(layout);
                assert_close(measure::volume(&solid), flat_area(&body) * T, 1e-6);
                let b = bounds(&solid);
                if top {
                    assert!(b.min.z > -1e-9, "{kind:?}: {:?}", b.min);
                } else {
                    assert!(b.max.z < T + 1e-9, "{kind:?}: {:?}", b.max);
                }
            }
        }
    }
}

#[test]
fn hem_errors() {
    let mut layout = plate_layout(settings(0.44));
    let short = hem(HemKind::Open, 2.0); // shorter than the fold (R + t = 3)
    assert!(layout.add_hem(2, &top_edge(true), &short).is_err());
    let mut low = hem(HemKind::Rolled, 0.0);
    low.angle = 170.0;
    assert!(layout.add_hem(2, &top_edge(true), &low).is_err());
    let mut curl = hem(HemKind::Rolled, 0.0);
    curl.angle = 330.0;
    let err = layout.add_hem(2, &top_edge(true), &curl).unwrap_err();
    assert!(err.contains("curls back"), "{err}");
    let mut long = hem(HemKind::Teardrop, 10.0);
    long.angle = 220.0;
    let err = layout.add_hem(2, &top_edge(true), &long).unwrap_err();
    assert!(err.contains("runs into the sheet"), "{err}");
    let mut s = settings(0.44);
    s.model = BendModel::Deduction(3.0);
    let mut layout = plate_layout(s);
    let err = layout
        .add_hem(2, &top_edge(true), &hem(HemKind::Closed, 10.0))
        .unwrap_err();
    assert!(err.contains("deduction"), "{err}");
}

fn sketched(angle: f64, position: BendLinePosition, up: bool) -> SketchedBendSpec {
    SketchedBendSpec {
        angle,
        radius: None,
        position,
        up,
        flip_fixed: false,
    }
}

/// The vertical line x = 40 across the plate.
const LINE: [DVec2; 2] = [DVec2::new(40.0, 0.0), DVec2::new(40.0, H)];

#[test]
fn sketched_bend_folds_the_smaller_side() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    let split = layout
        .add_sketched_bend(
            2,
            0,
            0,
            LINE,
            &sketched(90.0, BendLinePosition::Centerline, true),
        )
        .unwrap();
    assert_eq!(split, Split { bend: 1, child: 2 });
    let (solid, body) = built(layout);
    // The flat pattern is the plate, untouched.
    let (lo, hi) = body.flat_bounds();
    assert!(lo.abs_diff_eq(DVec2::ZERO, 1e-9) && hi.abs_diff_eq(DVec2::new(W, H), 1e-9));
    assert_close(measure::volume(&solid), W * H * T, 1e-6);
    // The left 40 (less half the bend) stands up; the right side stays.
    let ba = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap().allowance;
    let b = bounds(&solid);
    assert_close(b.max.x, W, 1e-9);
    assert_close(b.min.x, 40.0 + ba / 2.0 - R - T, 1e-9);
    assert_close(b.max.z, T + R + 40.0 - ba / 2.0, 1e-9);
    assert_eq!(body.bend_lines.len(), 1);
    let seg = &body.bend_lines[0].segments;
    assert_eq!(seg.len(), 1);
    assert_close(seg[0][0].x, 40.0, 1e-9);
    assert_close(seg[0][0].distance(seg[0][1]), H, 1e-9);
}

#[test]
fn sketched_bend_positions_and_flip() {
    let s = settings(0.44);
    let v = BendValues::new(s.model, FRAC_PI_2, R, T).unwrap();
    for (position, start) in [
        (BendLinePosition::Centerline, -v.allowance / 2.0),
        (BendLinePosition::MaterialInside, -v.outside_setback()),
        (BendLinePosition::MaterialOutside, -v.inside_setback()),
        (BendLinePosition::BendOutside, 0.0),
    ] {
        for flip in [false, true] {
            let mut layout = plate_layout(s);
            let mut spec = sketched(90.0, position, false);
            spec.flip_fixed = flip;
            layout.add_sketched_bend(2, 0, 0, LINE, &spec).unwrap();
            let bend = *layout.pieces[1].bend().unwrap();
            // The bend starts `start` past the line, on the moving side.
            let moving = if flip { 1.0 } else { -1.0 };
            assert_close(bend.across.x, moving, 1e-12);
            assert_close(bend.origin.x, 40.0 + moving * start, 1e-9);
            let (solid, _) = built(layout);
            let b = bounds(&solid);
            // Downward: everything at or below the sheet's top.
            assert!(b.max.z <= T + 1e-9, "{position:?} {flip}: {:?}", b.max);
            if position == BendLinePosition::MaterialInside && !flip {
                // The outer face of the moved side stands on the line.
                assert_close(b.min.x, 40.0, 1e-9);
            }
        }
    }
}

#[test]
fn sketched_bend_carries_holes_and_flanges() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    // A hole on each side of the line and a flange on the moving side's edge.
    for c in [DVec2::new(15.0, 30.0), DVec2::new(70.0, 30.0)] {
        layout.add_cut(Area {
            loops: vec![vec![Edge2 {
                curve: Curve::Circle {
                    center: c,
                    radius: 4.0,
                },
                reversed: true,
                tag: CurveTag::Sketch {
                    owner: 9,
                    entity: 1,
                },
            }]],
        });
    }
    let left = EdgeSite {
        piece: 0,
        a: DVec2::new(0.0, H),
        b: DVec2::new(0.0, 0.0),
        top: true,
    };
    layout
        .add_edge_flange(3, &left, &flange(15.0, FlangePosition::BendOutside))
        .unwrap();
    let before = layout.clone();
    layout
        .add_sketched_bend(
            4,
            0,
            0,
            LINE,
            &sketched(90.0, BendLinePosition::Centerline, true),
        )
        .unwrap();
    let (flat_solid, flat_body) = built(before);
    let (solid, body) = built(layout);
    // Same flat pattern, same volume (K = 0.5), one more bend.
    assert_close(flat_area(&body), flat_area(&flat_body), 1e-6);
    assert_close(measure::volume(&solid), measure::volume(&flat_solid), 1e-6);
    assert_eq!(body.outline.iter().filter(|l| !l.outer).count(), 2);
    assert_eq!(body.bend_lines.len(), 2);
    // The edge flange moved with the left side: it now hangs from the new flange.
    let flange_bend = layout_bend_of(&body.layout, 3);
    assert_eq!(flange_bend.parent, 4);
}

fn layout_bend_of(layout: &Layout, owner: u32) -> Bend {
    *layout
        .pieces
        .iter()
        .find(|p| p.origin.owner == owner && p.bend().is_some())
        .and_then(|p| p.bend())
        .unwrap()
}

#[test]
fn sketched_bend_errors() {
    let s = settings(0.44);
    let mut layout = plate_layout(s);
    let spec = sketched(90.0, BendLinePosition::Centerline, true);
    // Off the plate.
    let off = [DVec2::new(-10.0, 0.0), DVec2::new(-10.0, H)];
    assert!(layout.add_sketched_bend(2, 0, 0, off, &spec).is_err());
    // Across a flange's bend.
    layout
        .add_edge_flange(
            3,
            &top_edge(true),
            &flange(15.0, FlangePosition::BendOutside),
        )
        .unwrap();
    let err = layout.add_sketched_bend(2, 0, 0, LINE, &spec).unwrap_err();
    assert!(err.contains("runs into"), "{err}");
    // On a flange that hangs from a bend, the fixed side can't be chosen.
    let mut layout = plate_layout(s);
    layout
        .add_edge_flange(
            3,
            &top_edge(true),
            &flange(40.0, FlangePosition::BendOutside),
        )
        .unwrap();
    let across = [DVec2::new(0.0, H + 25.0), DVec2::new(W, H + 25.0)];
    let mut flipped = spec;
    flipped.flip_fixed = true;
    assert!(layout.add_sketched_bend(2, 0, 2, across, &flipped).is_err());
    let split = layout.add_sketched_bend(2, 0, 2, across, &spec).unwrap();
    // The part beyond the line (away from the plate) moves.
    assert!(layout.pieces[split.bend].bend().unwrap().across.y > 0.0);
    built(layout);
}

#[test]
fn jog_offsets_the_far_side() {
    for dimension in JogDimension::ALL {
        for up in [true, false] {
            let s = settings(0.5);
            let mut layout = plate_layout(s);
            let spec = JogSpec {
                offset: 20.0,
                dimension,
                angle: 90.0,
                radius: None,
                position: BendLinePosition::BendOutside,
                up,
                flip_fixed: false,
            };
            let [_, second] = layout.add_jog(2, 0, LINE, &spec).unwrap();
            let far = layout.pieces[second.child].frame;
            // The far side's bottom face, in the model.
            let z = far.to_world(DVec3::new(5.0, 5.0, 0.0)).z;
            let rise = match dimension {
                JogDimension::Overall => 20.0,
                JogDimension::Inside => 20.0 + T,
                JogDimension::Outside => 20.0 - T,
            };
            assert_close(z, if up { rise } else { -rise }, 1e-9);
            // And parallel to the plate.
            assert!(far.z_axis().abs_diff_eq(DVec3::Z, 1e-12));
            let (solid, body) = built(layout);
            assert_close(measure::volume(&solid), W * H * T, 1e-6);
            assert_eq!(body.bend_lines.len(), 2);
        }
    }
    // Too small for two bends: with no flat between them, they rise 2R + t.
    let mut layout = plate_layout(settings(0.5));
    let spec = JogSpec {
        offset: 1.0,
        dimension: JogDimension::Overall,
        angle: 90.0,
        radius: None,
        position: BendLinePosition::Centerline,
        up: true,
        flip_fixed: false,
    };
    let err = layout.add_jog(2, 0, LINE, &spec).unwrap_err();
    assert!(err.contains("too small"), "{err}");
}

/// The plate's four edges, counter-clockwise from the bottom, material on the left.
const EDGES: [(DVec2, DVec2); 4] = [
    (DVec2::new(0.0, 0.0), DVec2::new(W, 0.0)),
    (DVec2::new(W, 0.0), DVec2::new(W, H)),
    (DVec2::new(W, H), DVec2::new(0.0, H)),
    (DVec2::new(0.0, H), DVec2::new(0.0, 0.0)),
];

/// A box: a flange on every edge, with no set-back, so they meet in four corners.
fn closed_box(s: SheetSettings, position: FlangePosition, length: f64) -> Layout {
    let mut layout = plate_layout(s);
    for (i, (a, b)) in EDGES.into_iter().enumerate() {
        layout
            .add_edge_flange(
                10 + i as u32,
                &EdgeSite {
                    piece: 0,
                    a,
                    b,
                    top: true,
                },
                &flange(length, position),
            )
            .unwrap();
    }
    layout
}

/// The folded bounds of the faces of one piece.
fn piece_bounds(solid: &Solid, body: &SheetBody, piece: usize) -> Aabb {
    let mut b = Aabb::EMPTY;
    for (id, tag) in solid.face_ids().zip(&body.faces) {
        if tag.piece() != piece {
            continue;
        }
        for &l in &solid.face(id).loops {
            for c in solid.loop_coedges(l) {
                b.extend(solid.vertex(solid.coedge_start(c)).point);
            }
        }
    }
    b
}

/// How much two boxes overlap in volume.
fn overlap(a: &Aabb, b: &Aabb) -> f64 {
    let lo = a.min.max(b.min);
    let hi = a.max.min(b.max);
    let d = (hi - lo).max(DVec3::ZERO);
    d.x * d.y * d.z
}

/// The first flat of each flange of the box, in edge order.
fn box_flanges(body: &SheetBody) -> Vec<usize> {
    body.layout
        .attachments
        .iter()
        .map(|a| a.pieces[1])
        .collect()
}

#[test]
fn flanges_meeting_at_corners_get_corners() {
    let layout = closed_box(settings(0.44), FlangePosition::MaterialInside, 20.0);
    assert_eq!(layout.corners.len(), 4);
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    assert!(
        b.min.abs_diff_eq(DVec3::ZERO, 1e-9) && b.max.abs_diff_eq(DVec3::new(W, H, 20.0), 1e-9),
        "{b:?}"
    );
    // No two flanges share any volume.
    let flanges = box_flanges(&body);
    let boxes: Vec<Aabb> = flanges
        .iter()
        .map(|&f| piece_bounds(&solid, &body, f))
        .collect();
    for i in 0..4 {
        for j in i + 1..4 {
            assert!(overlap(&boxes[i], &boxes[j]) < 1e-9, "{i} {j}");
        }
    }
    // Butt: the later flange (the right one) stops the gap short of the earlier (bottom)
    // one's inner face; the bottom one runs to the right one's outer face.
    let gap = corner::DEFAULT_GAP;
    assert_close(boxes[1].min.y, T + gap, 1e-9);
    assert_close(boxes[0].max.x, W, 1e-9);
    // The last flange (left) butts against both its neighbours.
    assert_close(boxes[3].min.y, T + gap, 1e-9);
    assert_close(boxes[3].max.y, H - T - gap, 1e-9);
    // One blank, with the four corners relieved.
    assert_eq!(body.outline.iter().filter(|l| l.outer).count(), 1);
    assert_eq!(body.bend_lines.len(), 4);
}

#[test]
fn corner_kinds() {
    let s = settings(0.44);
    let gap = 0.5;
    for kind in CornerKind::ALL {
        for relief in CornerRelief::ALL {
            let mut layout = closed_box(s, FlangePosition::MaterialInside, 20.0);
            for c in &mut layout.corners {
                c.spec = CornerSpec {
                    kind,
                    gap,
                    relief,
                    relief_size: 1.0,
                };
            }
            let (solid, body) = built(layout);
            let flanges = box_flanges(&body);
            let bottom = piece_bounds(&solid, &body, flanges[0]);
            let right = piece_bounds(&solid, &body, flanges[1]);
            assert!(overlap(&bottom, &right) < 1e-9, "{kind:?}");
            match kind {
                CornerKind::Butt => {
                    assert_close(bottom.max.x, W, 1e-9);
                    assert_close(right.min.y, T + gap, 1e-9);
                }
                CornerKind::Overlap => {
                    assert_close(bottom.max.x, W - T - gap, 1e-9);
                    assert_close(right.min.y, 0.0, 1e-9);
                }
                CornerKind::Open => {
                    assert_close(bottom.max.x, W - T - gap, 1e-9);
                    assert_close(right.min.y, T + gap, 1e-9);
                }
                // With the flanges inside the edges a plain corner is a butt corner.
                CornerKind::Plain => {
                    assert_close(bottom.max.x, W, 1e-9);
                    assert_close(right.min.y, T + gap, 1e-9);
                }
            }
        }
    }
}

#[test]
fn flush_corner_relief_ends_with_the_flanges() {
    // Open corners with a flush relief: each bend ends in line with its flange, so the
    // blank is a plain cross, twelve straight edges, with one inside corner at each corner
    // of the plate. A rectangular relief notches each corner further in. The gap is wider
    // than the bend radius: a flush relief is never less than a tear, which reaches past
    // the bends, so with a narrower gap the bends would end before their flanges do.
    let gap = R + 0.5;
    let blank = |relief: CornerRelief| {
        let mut layout = closed_box(settings(0.44), FlangePosition::MaterialInside, 20.0);
        for c in &mut layout.corners {
            c.spec = CornerSpec {
                kind: CornerKind::Open,
                gap,
                relief,
                relief_size: 1.0,
            };
        }
        let (solid, body) = built(layout);
        let flanges = box_flanges(&body);
        let bottom = piece_bounds(&solid, &body, flanges[0]);
        let right = piece_bounds(&solid, &body, flanges[1]);
        // The flanges are where an open corner puts them, whatever the relief.
        assert_close(bottom.max.x, W - T - gap, 1e-9);
        assert_close(right.min.y, T + gap, 1e-9);
        let outer: Vec<_> = body.outline.iter().filter(|l| l.outer).collect();
        assert_eq!(outer.len(), 1);
        (outer[0].edges.len(), flat_area(&body))
    };
    let (flush_edges, flush_area) = blank(CornerRelief::Flush);
    let (square_edges, square_area) = blank(CornerRelief::Rectangular);
    assert_eq!(flush_edges, 12);
    assert!(square_edges > flush_edges);
    // A flush relief keeps what a rectangular one cuts from the plate and the bends.
    assert!(flush_area > square_area);
}

#[test]
fn slanted_corner_relief_keeps_the_face() {
    // Open corners with a slanted relief: nothing is cut from the plate, and each bend
    // ends on the line from the plate's corner to the end of its flange. So the blank has
    // more in it than with a flush relief, and folding keeps its volume (K = 0.5).
    let gap = R + 0.5;
    let blank = |relief: CornerRelief| {
        let mut layout = closed_box(settings(0.5), FlangePosition::MaterialInside, 20.0);
        for c in &mut layout.corners {
            c.spec = CornerSpec {
                kind: CornerKind::Open,
                gap,
                relief,
                relief_size: 1.0,
            };
        }
        let (solid, body) = built(layout);
        let flanges = box_flanges(&body);
        let bottom = piece_bounds(&solid, &body, flanges[0]);
        assert_close(bottom.max.x, W - T - gap, 1e-9);
        let outer: Vec<_> = body.outline.iter().filter(|l| l.outer).collect();
        assert_eq!(outer.len(), 1);
        let flat = measure::volume(&body.flat);
        assert_close(measure::volume(&solid), flat, flat * 2e-5);
        (outer[0].edges.len(), flat_area(&body))
    };
    let (slanted_edges, slanted_area) = blank(CornerRelief::Slanted);
    let (flush_edges, flush_area) = blank(CornerRelief::Flush);
    // Each corner of the cross has two slanted edges where a flush one has a notch.
    assert_eq!(flush_edges, 12);
    assert_eq!(slanted_edges, 20);
    assert!(slanted_area > flush_area);
}

#[test]
fn circular_and_square_reliefs_are_centred_where_the_flanges_end() {
    // Open corners, the flanges stopping well short of the corner: a flush relief cuts the
    // blank back to the corner of the two flanges' ends. A circular or square relief is
    // centred there, so it takes three quarters of its shape more from the face, the
    // bends and the flanges at each of the four corners. Folding keeps the volume.
    let gap = R + T + 4.0;
    let size = 4.0;
    let blank = |relief: CornerRelief| {
        let mut layout = closed_box(settings(0.5), FlangePosition::MaterialInside, 20.0);
        for c in &mut layout.corners {
            c.spec = CornerSpec {
                kind: CornerKind::Open,
                gap,
                relief,
                relief_size: size,
            };
        }
        let (solid, body) = built(layout);
        let flat = measure::volume(&body.flat);
        assert_close(measure::volume(&solid), flat, flat * 2e-5);
        flat_area(&body)
    };
    let flush = blank(CornerRelief::Flush);
    let square = blank(CornerRelief::Square);
    let circular = blank(CornerRelief::Circular);
    assert_close(flush - square, 4.0 * 0.75 * size * size, 1e-6);
    let disc = std::f64::consts::PI * size * size / 4.0;
    assert_close(flush - circular, 4.0 * 0.75 * disc, 1e-3);
}

#[test]
fn a_plain_corner_runs_nothing_on() {
    // Bend outside: the flanges stand R + t outside the plate. A butt corner runs the
    // earlier flange on to the later one's outer face; a plain corner leaves it ending
    // where its own edge does, at the plate's corner.
    let mut layout = closed_box(settings(0.44), FlangePosition::BendOutside, 20.0);
    for c in &mut layout.corners {
        c.spec.kind = CornerKind::Plain;
    }
    let (solid, body) = built(layout);
    let flanges = box_flanges(&body);
    let bottom = piece_bounds(&solid, &body, flanges[0]);
    let right = piece_bounds(&solid, &body, flanges[1]);
    assert_close(bottom.max.x, W, 1e-9);
    assert_close(bottom.min.x, 0.0, 1e-9);
    assert!(overlap(&bottom, &right) < 1e-9);
}

#[test]
fn closed_corners_with_flanges_outside() {
    // Bend outside: the flanges stand R + t outside the plate. A butt corner closes the
    // gap between them: the earlier flange runs on to the later one's outer face.
    let (solid, body) = built(closed_box(
        settings(0.44),
        FlangePosition::BendOutside,
        20.0,
    ));
    let flanges = box_flanges(&body);
    let bottom = piece_bounds(&solid, &body, flanges[0]);
    let right = piece_bounds(&solid, &body, flanges[1]);
    assert_close(bottom.max.x, W + R + T, 1e-9);
    assert_close(right.min.y, -R + corner::DEFAULT_GAP, 1e-9);
    assert!(overlap(&bottom, &right) < 1e-9);
    let b = bounds(&solid);
    assert!(
        b.min.abs_diff_eq(DVec3::new(-R - T, -R - T, 0.0), 1e-9),
        "{b:?}"
    );
}

#[test]
fn flanges_set_back_make_no_corner() {
    let s = settings(0.44);
    let mut layout = plate_layout(s);
    let mut spec = flange(20.0, FlangePosition::MaterialInside);
    spec.offsets = [10.0, 10.0];
    for (i, (a, b)) in EDGES.into_iter().enumerate() {
        let site = EdgeSite {
            piece: 0,
            a,
            b,
            top: true,
        };
        layout.add_edge_flange(10 + i as u32, &site, &spec).unwrap();
    }
    assert!(layout.corners.is_empty());
}

/// The plate's edges as a closed chain (each starts where the one before ends).
fn chain() -> Vec<EdgeSite> {
    EDGES
        .iter()
        .map(|&(a, b)| EdgeSite {
            piece: 0,
            a,
            b,
            top: true,
        })
        .collect()
}

/// Points of a piece's faces in the folded part.
fn piece_points(solid: &Solid, body: &SheetBody, piece: usize) -> Vec<DVec3> {
    let mut out = Vec::new();
    for (id, tag) in solid.face_ids().zip(&body.faces) {
        if tag.piece() != piece {
            continue;
        }
        for &l in &solid.face(id).loops {
            for c in solid.loop_coedges(l) {
                out.push(solid.vertex(solid.coedge_start(c)).point);
            }
        }
    }
    out
}

#[test]
fn miter_flange_makes_a_rimmed_tray() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    // Up 20 from the bottom face, then a lip 8 back over the plate (to the virtual
    // sharps, measured on the outside).
    let profile = [
        DVec2::new(0.0, 0.0),
        DVec2::new(0.0, 20.0),
        DVec2::new(-8.0, 20.0),
    ];
    let gap = 0.4;
    layout
        .add_miter_flange(5, &chain(), &profile, [0.0, 0.0], gap)
        .unwrap();
    assert_eq!(layout.attachments.len(), 4);
    assert_eq!(layout.corners.len(), 4);
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    assert!(
        b.min.abs_diff_eq(DVec3::ZERO, 1e-9) && b.max.abs_diff_eq(DVec3::new(W, H, 20.0), 1e-9),
        "{b:?}"
    );
    assert_eq!(body.bend_lines.len(), 8);
    assert_eq!(body.outline.iter().filter(|l| l.outer).count(), 1);
    // The walls butt: no two share volume.
    let walls: Vec<Aabb> = body
        .layout
        .attachments
        .iter()
        .map(|a| piece_bounds(&solid, &body, a.pieces[1]))
        .collect();
    for i in 0..4 {
        for j in i + 1..4 {
            assert!(overlap(&walls[i], &walls[j]) < 1e-9, "walls {i} {j}");
        }
    }
    // The lips are mitred along the corner's diagonal, half the gap each side. At the
    // corner at the origin: the bottom lip (edge 0) keeps x − y ≥ gap/√2, the left lip
    // (edge 3) y − x ≥ gap/√2.
    let lips: Vec<usize> = body
        .layout
        .attachments
        .iter()
        .map(|a| a.pieces[3])
        .collect();
    let d = gap / 2.0 * std::f64::consts::SQRT_2;
    for p in piece_points(&solid, &body, lips[0]) {
        if p.x < W / 2.0 {
            assert!(p.x - p.y >= d - 1e-9, "bottom lip {p}");
        }
    }
    for p in piece_points(&solid, &body, lips[3]) {
        if p.y < H / 2.0 {
            assert!(p.y - p.x >= d - 1e-9, "left lip {p}");
        }
    }
    // And they do reach the diagonal (a mitre, not a gap).
    let closest = piece_points(&solid, &body, lips[0])
        .into_iter()
        .filter(|p| p.x < W / 2.0)
        .map(|p| p.x - p.y)
        .fold(f64::INFINITY, f64::min);
    assert_close(closest, d, 1e-9);
}

#[test]
fn single_edge_profile_flange() {
    // A Z: up 15, then out 10 away from the plate.
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    let profile = [
        DVec2::new(0.0, T),
        DVec2::new(0.0, T + 15.0),
        DVec2::new(10.0, T + 15.0),
    ];
    let site = top_edge(true);
    layout
        .add_miter_flange(5, &[site], &profile, [0.0, 0.0], 0.0)
        .unwrap();
    let (solid, body) = built(layout);
    assert_close(measure::volume(&solid), flat_area(&body) * T, 1e-6);
    let b = bounds(&solid);
    // Starting at the top face: the profile is the wall's inner face, so the wall's
    // outer face stands t outside the edge, and the lip's top is at t + 15.
    assert_close(b.max.z, T + 15.0, 1e-9);
    assert_close(b.max.y, H + 10.0, 1e-9);
    assert_eq!(body.bend_lines.len(), 2);
    let rows = body.report().bends;
    assert!(rows[0].up && !rows[1].up);
}

#[test]
fn profile_errors() {
    let layout = plate_layout(settings(0.44));
    let off_edge = [DVec2::new(3.0, 0.0), DVec2::new(3.0, 10.0)];
    assert!(layout.profile_segments(&off_edge).is_err());
    let mid = [DVec2::new(0.0, 1.0), DVec2::new(0.0, 10.0)];
    assert!(layout.profile_segments(&mid).is_err());
    let along = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
    assert!(layout.profile_segments(&along).is_err());
    let short = [
        DVec2::new(0.0, 0.0),
        DVec2::new(0.0, 4.0),
        DVec2::new(-3.0, 4.0),
    ];
    assert!(layout.profile_segments(&short).is_err());
    let mut layout = plate_layout(settings(0.44));
    let mut sites = chain();
    sites.swap(1, 2);
    let profile = [DVec2::new(0.0, 0.0), DVec2::new(0.0, 20.0)];
    assert!(
        layout
            .add_miter_flange(5, &sites, &profile, [0.0, 0.0], 0.1)
            .is_err()
    );
}

fn dimple(center: DVec2, radius: f64, height: f64, up: bool) -> Form {
    Form {
        owner: 7,
        part: 0,
        piece: 0,
        kind: FormKind::Dimple,
        shape: FormShape::Circle { center, radius },
        height,
        up,
        open_side: None,
    }
}

fn rectangle(o: DVec2, w: f64, h: f64) -> Vec<DVec2> {
    vec![
        o,
        o + DVec2::new(w, 0.0),
        o + DVec2::new(w, h),
        o + DVec2::new(0.0, h),
    ]
}

#[test]
fn dimples_up_and_down() {
    let (r, h) = (10.0, 3.0);
    for up in [true, false] {
        let mut layout = plate_layout(settings(0.44));
        layout
            .add_form(dimple(DVec2::new(50.0, 30.0), r, h, up))
            .unwrap();
        let (solid, body) = built(layout);
        // The sheet, plus the wall's ring lifted by the height.
        let ring = PI * (r * r - (r - T) * (r - T));
        assert_close(measure::volume(&solid), W * H * T + ring * h, 1e-6);
        assert_close(measure::volume(&body.flat), W * H * T + ring * h, 1e-6);
        let b = bounds(&solid);
        if up {
            assert_close(b.max.z, h + T, 1e-9);
            assert_close(b.min.z, 0.0, 1e-9);
        } else {
            assert_close(b.min.z, -h, 1e-9);
            assert_close(b.max.z, T, 1e-9);
        }
        // The form isn't a cutout of the blank: it is marked.
        assert_eq!(body.outline.len(), 1);
        assert_eq!(body.forms.len(), 1);
        assert!(body.forms[0].lance.is_none());
        assert!(
            body.forms[0]
                .center
                .abs_diff_eq(DVec2::new(50.0, 30.0), 1e-12)
        );
    }
}

#[test]
fn emboss_and_louver() {
    let (w, l, h) = (30.0, 16.0, 4.0);
    let at = DVec2::new(20.0, 15.0);
    let a = w * l;
    // Emboss: the inside is moved in on every side.
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_form(Form {
            owner: 7,
            part: 0,
            piece: 0,
            kind: FormKind::Emboss,
            shape: FormShape::Polygon(rectangle(at, w, l)),
            height: h,
            up: true,
            open_side: None,
        })
        .unwrap();
    let (solid, body) = built(layout);
    let inside = (w - 2.0 * T) * (l - 2.0 * T);
    assert_close(measure::volume(&solid), W * H * T + (a - inside) * h, 1e-6);
    assert_eq!(body.outline.len(), 1);
    // Louver: open along its first side, which the sheet is lanced along.
    for up in [true, false] {
        let mut layout = plate_layout(settings(0.44));
        layout
            .add_form(Form {
                owner: 7,
                part: 0,
                piece: 0,
                kind: FormKind::Louver,
                shape: FormShape::Polygon(rectangle(at, w, l)),
                height: h,
                up,
                open_side: Some(0),
            })
            .unwrap();
        let (solid, body) = built(layout);
        let inside = (w - 2.0 * T) * (l - T);
        assert_close(measure::volume(&solid), W * H * T + (a - inside) * h, 1e-6);
        let lance = body.forms[0].lance.unwrap();
        assert!(lance[0].abs_diff_eq(at, 1e-12));
        assert!(lance[1].abs_diff_eq(at + DVec2::new(w, 0.0), 1e-12));
    }
}

#[test]
fn forms_fold_with_their_flange() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(40.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let flange_piece = 2;
    // On the flange, in flat coordinates: past the bend, beyond y = H.
    let mut d = dimple(DVec2::new(50.0, H + 20.0), 6.0, 2.0, true);
    d.piece = flange_piece;
    layout.add_form(d).unwrap();
    // A louver on the plate, near the flange.
    layout
        .add_form(Form {
            owner: 8,
            part: 0,
            piece: 0,
            kind: FormKind::Louver,
            shape: FormShape::Polygon(rectangle(DVec2::new(10.0, 10.0), 25.0, 10.0)),
            height: 3.0,
            up: false,
            open_side: Some(2),
        })
        .unwrap();
    let (solid, body) = built(layout);
    assert_eq!(body.forms.len(), 2);
    // The flange stands upright with its top side facing into the part, so the dimple's
    // plateau stands 2 inside the flange's inner face (y = H - t), up the flange.
    let plateau = solid
        .vertices
        .iter()
        .filter(|v| v.point.z > T + R)
        .map(|v| v.point.y)
        .fold(f64::INFINITY, f64::min);
    assert_close(plateau, H - T - 2.0, 1e-9);
    folded_bodies_tessellate_with(&solid);
}

fn folded_bodies_tessellate_with(s: &Solid) {
    let mesh = peet_kernel::tessellate::tessellate(s, 0.01).unwrap();
    let area: f64 = mesh.faces.iter().map(|f| f.area()).sum();
    let exact: f64 = s.face_ids().map(|f| measure::face_area(s, f)).sum();
    assert_close(area, exact, 1e-3 * exact);
}

#[test]
fn form_errors() {
    let mut layout = plate_layout(settings(0.44));
    // Too small for the sheet.
    assert!(
        layout
            .add_form(dimple(DVec2::new(50.0, 30.0), 1.5, 2.0, true))
            .is_err()
    );
    // Off the edge.
    let err = layout
        .add_form(dimple(DVec2::new(5.0, 30.0), 6.0, 2.0, true))
        .unwrap_err();
    assert!(err.contains("clear"), "{err}");
    // A concave emboss.
    let concave = vec![
        DVec2::new(20.0, 10.0),
        DVec2::new(60.0, 10.0),
        DVec2::new(40.0, 20.0),
        DVec2::new(60.0, 40.0),
        DVec2::new(20.0, 40.0),
    ];
    let mut e = dimple(DVec2::ZERO, 1.0, 2.0, true);
    e.kind = FormKind::Emboss;
    e.shape = FormShape::Polygon(concave);
    assert!(layout.add_form(e).is_err());
    // Two forms on top of each other.
    layout
        .add_form(dimple(DVec2::new(50.0, 30.0), 8.0, 2.0, true))
        .unwrap();
    assert!(
        layout
            .add_form(dimple(DVec2::new(55.0, 30.0), 8.0, 2.0, true))
            .is_err()
    );
}

#[test]
fn sketched_bend_carries_forms() {
    let s = settings(0.5);
    let mut layout = plate_layout(s);
    layout
        .add_form(dimple(DVec2::new(15.0, 30.0), 6.0, 2.0, true))
        .unwrap();
    let mut through = layout.clone();
    through
        .add_form(dimple(DVec2::new(70.0, 30.0), 6.0, 2.0, true))
        .unwrap();
    let spec = sketched(90.0, BendLinePosition::Centerline, true);
    layout.add_sketched_bend(2, 0, 0, LINE, &spec).unwrap();
    // The dimple was on the side that folds up.
    assert_eq!(layout.forms[0].piece, 2);
    let (solid, _) = built(layout);
    assert!(solid.vertices.iter().any(|v| v.point.z > 20.0));
    // A line through a form is refused.
    let line = [DVec2::new(70.0, 0.0), DVec2::new(70.0, H)];
    let err = through.add_sketched_bend(2, 0, 0, line, &spec).unwrap_err();
    assert!(err.contains("dimple"), "{err}");
}

#[test]
fn cut_that_grazes_an_edge() {
    // A cut that reaches three microns into the plate across its bottom edge, as an imported
    // cut drawn a hair off the edge it was meant to stop at does (P34). Its two sides inside
    // the plate are a few microns long, and the walls through the sheet along them would
    // enclose no area: they are left out and the notch with them.
    let mut layout = plate_layout(settings(0.5));
    layout.add_cut(Area::polygon(
        &[
            DVec2::new(45.0, -5.0),
            DVec2::new(55.0, -5.0),
            DVec2::new(50.0, 3e-6),
        ],
        |i| CurveTag::Sketch {
            owner: 3,
            entity: i as u32,
        },
    ));
    let (solid, body) = built(layout);
    assert_close(measure::volume(&solid), W * H * T, 1e-6);
    assert_eq!(body.outline.len(), 1);
}

#[test]
fn material_inside_at_an_angle() {
    // SolidWorks' material inside at an angle: the flange's outer face goes through the
    // original edge on the far face of the sheet. Bend outside starts the bend at the
    // edge, so the two differ by the outside setback and t / tan(angle) along the face.
    for angle in [30.0_f64, 60.0, 90.0, 120.0] {
        let max_y = |position| {
            let mut layout = plate_layout(settings(0.44));
            let mut spec = flange(20.0, position);
            spec.angle = angle;
            layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
            bounds(&built(layout).0).max.y
        };
        let a = angle.to_radians();
        let expected = (R + T) * (a / 2.0).tan() + T / a.tan();
        assert_close(
            max_y(FlangePosition::BendOutside) - max_y(FlangePosition::MaterialInside),
            expected,
            1e-6,
        );
    }
}

#[test]
fn a_flange_set_out_from_its_edge() {
    // SolidWorks' position offset: the sheet runs on flat past the edge before the bend.
    for (position, y_max) in [
        (FlangePosition::MaterialInside, H),
        (FlangePosition::BendOutside, H + R + T),
    ] {
        for set_out in [0.0, 7.0, -3.0] {
            let mut layout = plate_layout(settings(0.5));
            let own = FlangeOwn {
                set_out,
                ..Default::default()
            };
            layout
                .add_edge_flange_with(2, &top_edge(true), &flange(15.0, position), own)
                .unwrap();
            let (solid, body) = built(layout);
            assert_close(bounds(&solid).max.y, y_max + set_out, 1e-9);
            // K = 0.5 keeps the volume: the blank is the folded part laid flat.
            assert_close(measure::volume(&solid), measure::volume(&body.flat), 1e-6);
        }
    }
}

#[test]
fn a_sharp_bend() {
    // A bend radius of 0, as SolidWorks allows: the body is built, and its blank is the
    // one a radius of 0 gives, to well within a micron.
    let mut s = settings(0.5);
    s.radius = 0.0;
    let mut layout = plate_layout(s);
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(20.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (_, body) = built(layout);
    let v = BendValues::new(BendModel::KFactor(0.5), FRAC_PI_2, 0.0, T).unwrap();
    let blank = (body.flat_bounds().1 - body.flat_bounds().0).y;
    assert_close(
        blank,
        H - v.outside_setback() + v.allowance + 20.0 - v.outside_setback(),
        1e-6,
    );
    assert_close(v.outside_setback(), T, 1e-3);
}

#[test]
fn a_flange_shaped_to_an_outline() {
    // SolidWorks' flange profile: a trapezoid on the flange's plane, 80 wide at the edge
    // and 40 wide at the top, 30 up. The flange is made that wide and that long and cut to
    // it, and the bend below stays whole where the outline's foot is.
    let mut layout = plate_layout(settings(0.5));
    let world = [
        DVec3::new(10.0, H, 0.0),
        DVec3::new(90.0, H, 0.0),
        DVec3::new(70.0, H, 30.0),
        DVec3::new(30.0, H, 30.0),
    ];
    let outline = |frame: &peet_math::Frame| {
        let flat: Vec<DVec2> = world
            .iter()
            .map(|p| frame.to_local(*p).truncate())
            .collect();
        Area::polygon(&flat, |i| CurveTag::Sketch {
            owner: 9,
            entity: i as u32,
        })
    };
    layout
        .add_edge_flange_outlined(
            2,
            0,
            &top_edge(true),
            &flange(10.0, FlangePosition::MaterialInside),
            FlangeOwn::default(),
            &outline,
        )
        .unwrap();
    let (solid, body) = built(layout);
    let b = bounds(&solid);
    assert_close(b.max.z, 30.0, 1e-6);
    // The top is the outline's: 40 wide, from 30 to 70
    let top: Vec<f64> = solid
        .vertices
        .iter()
        .filter(|v| v.point.z > 30.0 - 1e-6)
        .map(|v| v.point.x)
        .collect();
    assert!(!top.is_empty());
    let (lo, hi) = top
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert_close(lo, 30.0, 1e-6);
    assert_close(hi, 70.0, 1e-6);
    // K = 0.5 keeps the volume: the blank is the folded part laid flat.
    assert_close(measure::volume(&solid), measure::volume(&body.flat), 1e-3);
}

#[test]
fn an_outline_to_the_corner_meets_the_flange_there() {
    // A wall outside the right edge takes the radius off the plate's top edge, which then
    // runs from x = 0 to W - R. A flange on it shaped to an outline that reaches the end of
    // that edge is carried on to the corner and meets the wall there, as one without an
    // outline does, and ends where the outline does (Louvre Filter Bracket).
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &EdgeSite {
                piece: 0,
                a: DVec2::new(W, 0.0),
                b: DVec2::new(W, H),
                top: true,
            },
            &flange(20.0, FlangePosition::MaterialOutside),
        )
        .unwrap();
    let world = [
        DVec3::new(10.0, H, 0.0),
        DVec3::new(W - R, H, 0.0),
        DVec3::new(W - R, H, 20.0),
        DVec3::new(10.0, H, 20.0),
    ];
    let outline = |frame: &peet_math::Frame| {
        let flat: Vec<DVec2> = world
            .iter()
            .map(|p| frame.to_local(*p).truncate())
            .collect();
        Area::polygon(&flat, |i| CurveTag::Sketch {
            owner: 9,
            entity: i as u32,
        })
    };
    let site = EdgeSite {
        piece: 0,
        a: DVec2::new(W - R, H),
        b: DVec2::new(0.0, H),
        top: true,
    };
    layout
        .add_edge_flange_outlined(
            3,
            0,
            &site,
            &flange(20.0, FlangePosition::MaterialInside),
            FlangeOwn::default(),
            &outline,
        )
        .unwrap();
    assert_eq!(layout.corners.len(), 1);
    let (solid, body) = built(layout);
    let lip = body.layout.attachments[1].pieces[1];
    let b = piece_bounds(&solid, &body, lip);
    // To the outline, not set back from the edge's end by the radius again
    assert_close(b.max.x, W - R, 1e-6);
    assert_close(b.min.x, 10.0, 1e-6);
}

#[test]
fn a_lip_on_a_wall_cuts_through_the_walls_bend() {
    // A lip on the side edge of a wall, which runs down to the wall's bend: the strip the
    // lip's bend takes from the wall runs on through the wall's bend, which SolidWorks
    // cuts away there.
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(30.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (solid, body) = built(layout.clone());
    let wall = layout.attachments[0].pieces[1];
    let site = solid
        .edge_ids()
        .filter_map(|e| body.edge_site(&solid, e).ok())
        .find(|s| {
            let fl = &body.layout.pieces[wall];
            let (p, q) = (
                fl.frame.to_world(s.a.extend(0.0)),
                fl.frame.to_world(s.b.extend(0.0)),
            );
            s.piece == wall
                && close(p.x, 0.0, 1e-9)
                && close(q.x, 0.0, 1e-9)
                && (p.z - q.z).abs() > 1.0
        })
        .expect("the wall's side edge");
    let before = measure::volume(&solid);
    layout
        .add_edge_flange(3, &site, &flange(10.0, FlangePosition::MaterialInside))
        .unwrap();
    let (solid, body) = built(layout);
    // The wall's bend now starts where the lip's bend ends, R + T in from the side
    let bend = body.layout.attachments[0].pieces[0];
    assert_close(piece_bounds(&solid, &body, bend).min.x, R + T, 1e-6);
    // What the lip adds, less the end of the wall's bend it took away
    let section = PI / 4.0 * ((R + T).powi(2) - R * R);
    let wall_height = 30.0 - (R + T);
    let lip = wall_height * ((10.0 - (R + T)) * T + section);
    let taken = wall_height * (R + T) * T + (R + T) * section;
    assert_close(measure::volume(&solid), before + lip - taken, 1e-3);
}

#[test]
fn a_profile_bend_with_its_own_model() {
    // SolidWorks' sketched bends can each have a deduction of their own: a shallow bend
    // whose body deduction would make its allowance negative takes a smaller one.
    let mut s = settings(0.5);
    s.model = BendModel::Deduction(3.65);
    s.radius = 0.1;
    let chain = [
        ChainLine {
            entity: 1,
            a: DVec2::new(0.0, 0.0),
            b: DVec2::new(100.0, 0.0),
        },
        ChainLine {
            entity: 2,
            a: DVec2::new(100.0, 0.0),
            b: DVec2::new(200.0, 20.0),
        },
    ];
    let plane = Plane::front();
    let err = Layout::open_profile(s, 1, &plane, &chain, [0.0, 50.0], false).unwrap_err();
    assert!(err.contains("Corner 1"), "{err}");
    let layout = Layout::open_profile_with(
        s,
        1,
        &plane,
        &chain,
        [0.0, 50.0],
        false,
        &[Some(BendModel::Deduction(0.1))],
    )
    .unwrap();
    let bend = layout.bends().next().unwrap().1;
    assert_close(
        bend.values.allowance,
        2.0 * bend.values.outside_setback() - 0.1,
        1e-9,
    );
    built(layout);
}

#[test]
fn a_flange_up_to_a_point() {
    // SolidWorks' "up to vertex": the flange's end goes through the point, square to the
    // flange, whatever length it was given to start from.
    for (angle, start) in [(90.0_f64, 10.0), (105.0, 40.0), (60.0, 25.0)] {
        let mut layout = plate_layout(settings(0.5));
        let mut spec = flange(start, FlangePosition::MaterialInside);
        spec.angle = angle;
        let point = DVec3::new(50.0, H + 5.0, 37.0);
        let length = layout
            .edge_flange_length_to(2, 0, &top_edge(true), &spec, FlangeOwn::default(), point)
            .unwrap();
        spec.length = length;
        layout.add_edge_flange(2, &top_edge(true), &spec).unwrap();
        // It reads back as laid out
        assert_close(layout.edge_flange_length(0).unwrap(), length, 1e-9);
        let (solid, _) = built(layout);
        let a = angle.to_radians();
        let along = DVec3::new(0.0, a.cos(), a.sin());
        let end = solid
            .vertices
            .iter()
            .map(|v| v.point.dot(along))
            .fold(f64::NEG_INFINITY, f64::max);
        assert_close(end, point.dot(along), 1e-6);
    }
    // A point behind the bend is refused
    let layout = plate_layout(settings(0.5));
    let err = layout
        .edge_flange_length_to(
            2,
            0,
            &top_edge(true),
            &flange(10.0, FlangePosition::MaterialInside),
            FlangeOwn::default(),
            DVec3::new(50.0, H, 0.5),
        )
        .unwrap_err();
    assert!(err.contains("behind the flange's bend"), "{err}");
}

#[test]
fn a_flange_beside_a_cut_may_share_a_sliver() {
    // A cut takes a strip 3 wide off the plate's side, its end drawn a few microns off
    // square, and a flange goes on the edge it leaves, outwards. In the flat pattern the
    // flange's end runs a few microns into the sheet beyond the cut, as SolidWorks' does:
    // that sliver stays with the plate rather than counting as an overlap.
    let tag = |i: usize| CurveTag::Sketch {
        owner: 9,
        entity: i as u32,
    };
    let cut = |end: [f64; 2]| {
        Area::polygon(
            &[
                DVec2::new(W - 3.0, -1.0),
                DVec2::new(W + 1.0, -1.0),
                DVec2::new(W + 1.0, end[1]),
                DVec2::new(W, end[1]),
                DVec2::new(W - 3.0, end[0]),
            ],
            tag,
        )
    };
    let site = |end: f64| EdgeSite {
        piece: 0,
        a: DVec2::new(W - 3.0, 0.0),
        b: DVec2::new(W - 3.0, end),
        top: true,
    };
    let mut layout = plate_layout(settings(0.5));
    layout.add_cut(cut([40.004, 40.0]));
    layout
        .add_edge_flange(
            2,
            &site(40.004),
            &flange(10.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    built(layout);
    // A real overlap is still refused
    let mut long = plate_layout(settings(0.5));
    long.add_cut(cut([41.0, 39.0]));
    long.add_edge_flange(
        2,
        &site(41.0),
        &flange(10.0, FlangePosition::MaterialInside),
    )
    .unwrap();
    assert!(matches!(build(long), Err(SheetError::Overlap { .. })));
}

fn slot_across_the_bend() -> Area {
    Area::polygon(
        &[
            DVec2::new(40.0, 45.0),
            DVec2::new(50.0, 45.0),
            DVec2::new(50.0, 70.0),
            DVec2::new(40.0, 70.0),
        ],
        |i| CurveTag::Sketch {
            owner: 3,
            entity: i as u32,
        },
    )
}

#[test]
fn unfold_lays_bends_flat_and_fold_carries_a_cut_back() {
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    let (folded, _) = built(layout.clone());
    let mut flat = layout.clone();
    flat.unfold(0, &[]).unwrap();
    let (solid, body) = built(flat.clone());
    // Flat: one thickness deep, as long as the blank, the plate where it was
    let b = bounds(&solid);
    assert_close(b.min.z, 0.0, 1e-9);
    assert_close(b.max.z, T, 1e-9);
    let (lo, hi) = body.flat_bounds();
    assert_close(b.max.y - b.min.y, hi.y - lo.y, 1e-9);
    assert_close(b.min.y, 0.0, 1e-9);
    assert_close(measure::volume(&solid), measure::volume(&folded), 1e-6);
    // A slot across the flat bend, then folded again: as if cut in the flat pattern
    flat.add_cut(slot_across_the_bend());
    flat.fold(&[]);
    assert!(flat.flat_bends.is_empty());
    let (again, _) = built(flat);
    layout.add_cut(slot_across_the_bend());
    let (direct, _) = built(layout);
    assert_close(measure::volume(&again), measure::volume(&direct), 1e-6);
    let (a, d) = (bounds(&again), bounds(&direct));
    assert!(a.min.abs_diff_eq(d.min, 1e-9) && a.max.abs_diff_eq(d.max, 1e-9));
}

#[test]
fn unfold_keeps_the_fixed_flange_where_it_is() {
    let mut layout = plate_layout(settings(0.5));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    // Fixed on the flange: the plate turns into the flange's plane
    let (wall, bend) = (2, 1);
    let mut flat = layout.clone();
    let before = flat.pieces[wall].frame;
    flat.unfold(wall, &[bend]).unwrap();
    assert_eq!(flat.pieces[wall].frame, before);
    let (solid, _) = built(flat.clone());
    let b = bounds(&solid);
    assert!(b.max.z - b.min.z > H, "{b:?}");
    assert!(b.max.y - b.min.y < T + 1e-6, "{b:?}");
    flat.fold(&[bend]);
    let (again, _) = built(flat);
    let (direct, _) = built(layout);
    let (a, d) = (bounds(&again), bounds(&direct));
    assert!(a.min.abs_diff_eq(d.min, 1e-9) && a.max.abs_diff_eq(d.max, 1e-9));
    assert!(plate_layout(settings(0.5)).unfold(0, &[3]).is_err());
}
