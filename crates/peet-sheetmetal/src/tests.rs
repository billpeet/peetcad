//! Scenario tests: plates, flanges, reliefs, cuts and open profiles, checked for validity
//! and against hand-calculated sizes.

use std::f64::consts::{FRAC_PI_2, PI};

use peet_kernel::Solid;
use peet_kernel::validate::{measure, validate};
use peet_math::{Aabb, DVec2, DVec3, Plane};
use peet_sketch::region::find_regions;
use peet_sketch::{Curve, Sketch, shapes};

use super::*;
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
fn round_hole_across_a_bend_is_refused() {
    let mut layout = plate_layout(settings(0.44));
    layout
        .add_edge_flange(
            2,
            &top_edge(true),
            &flange(25.0, FlangePosition::MaterialInside),
        )
        .unwrap();
    layout.add_cut(Area {
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
    });
    let err = build(layout).unwrap_err();
    assert!(
        matches!(err, SheetError::AcrossBend { curved: true, tag, .. } if tag.owner() == 9),
        "{err:?}"
    );
    let msg = err.message(|o| format!("F{o}"));
    assert!(msg.contains("F9") && msg.contains("F2"), "{msg}");
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
    // the corner tabs free (five separate outlines).
    let (_, loose) = built(panel(R + T + 1.0));
    assert_eq!(loose.outline.iter().filter(|l| l.outer).count(), 5);
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
