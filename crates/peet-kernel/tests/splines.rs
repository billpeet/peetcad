//! Solids from sketch profiles with splines in them: extruded, revolved, lofted and cut.
//! Volumes are checked against the regions' own areas, which are exact integrals of the
//! same curves.

use std::f64::consts::{PI, TAU};

use peet_kernel::boolean::{BooleanOp, boolean};
use peet_kernel::extrude::{ExtrudeFace, extrude, extrude_traced};
use peet_kernel::loft::{LoftSection, loft};
use peet_kernel::revolve::{RevolveAxis, revolve, revolve_traced};
use peet_kernel::tessellate::tessellate;
use peet_kernel::validate::{measure, validate};
use peet_kernel::{Curve3, KernelError, Solid, Surface};
use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::region::{Region, find_regions};
use peet_sketch::{Curve, Sketch};

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn check(solid: &Solid) {
    if let Err(problems) = validate(solid) {
        panic!("invalid solid: {problems:#?}");
    }
}

fn blob() -> [DVec2; 5] {
    [
        v(10.0, 0.0),
        v(4.0, 9.0),
        v(-8.0, 5.0),
        v(-9.0, -6.0),
        v(3.0, -8.0),
    ]
}

/// A rectangle 30 wide whose top edge is a spline from (30, 10) to (0, 10).
fn wavy_plate() -> Sketch {
    let mut s = Sketch::new();
    s.add_line(v(0.0, 10.0), v(0.0, 0.0));
    s.add_line(v(0.0, 0.0), v(30.0, 0.0));
    s.add_line(v(30.0, 0.0), v(30.0, 10.0));
    s.add_spline(
        &[v(30.0, 10.0), v(20.0, 14.0), v(10.0, 7.0), v(0.0, 10.0)],
        false,
    )
    .unwrap();
    s
}

fn only_region(s: &Sketch) -> Region {
    let mut regions = find_regions(s).regions;
    assert_eq!(regions.len(), 1);
    regions.remove(0)
}

#[test]
fn extruded_spline_side_is_an_exact_freeform_face() {
    let sketch = wavy_plate();
    let region = only_region(&sketch);
    let plane = Plane::front();
    let (solid, faces) = extrude_traced(&plane, std::slice::from_ref(&region), 0.0, 6.0).unwrap();
    check(&solid);
    assert!(
        (measure::volume(&solid) - region.area() * 6.0).abs() < 0.05,
        "{} vs {}",
        measure::volume(&solid),
        region.area() * 6.0
    );
    let freeform: Vec<usize> = (0..solid.faces.len())
        .filter(|i| matches!(solid.faces[*i].surface, Surface::Nurbs(_)))
        .collect();
    assert_eq!(freeform.len(), 1);
    // The wall is named after the spline's loop edge, like any other wall.
    let ExtrudeFace::Side { edge, .. } = faces[freeform[0]] else {
        panic!("the freeform face is a side")
    };
    assert!(matches!(region.outer.edges[edge].curve, Curve::Spline(_)));
    // Its cap edges are the sketch's spline, to modelling tolerance.
    let drawn = region.outer.edges[edge].curve.clone();
    let mut spline_edges = 0;
    for e in &solid.edges {
        let Curve3::Nurbs(curve) = &e.curve else {
            continue;
        };
        spline_edges += 1;
        let (lo, hi) = curve.domain();
        for i in 0..=100 {
            let p = curve.point(lo + (hi - lo) * f64::from(i) / 100.0);
            let uv = plane.to_plane_coords(p);
            assert!(drawn.distance(uv) < 1e-9, "{}", drawn.distance(uv));
            let height = plane.signed_distance(p);
            assert!(height.abs() < 1e-9 || (height - 6.0).abs() < 1e-9);
        }
    }
    assert_eq!(spline_edges, 2);
    assert!(!tessellate(&solid, 0.05).unwrap().faces.is_empty());
}

#[test]
fn closed_spline_extrudes_to_two_freeform_walls() {
    let mut s = Sketch::new();
    s.add_spline(&blob(), true).unwrap();
    let region = only_region(&s);
    let solid = extrude(&Plane::TOP, std::slice::from_ref(&region), -2.0, 3.0).unwrap();
    check(&solid);
    assert!(
        (measure::volume(&solid) - region.area() * 5.0).abs() < 0.05,
        "{} vs {}",
        measure::volume(&solid),
        region.area() * 5.0
    );
    let walls = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
        .count();
    assert_eq!(walls, region.outer.edges.len());
    // A single whole closed spline, as a hand-made loop, is cut in two.
    let whole = Region {
        outer: peet_sketch::region::Loop {
            edges: vec![peet_sketch::region::LoopEdge {
                entity: peet_sketch::EntityId(1),
                curve: Curve::spline_through(&blob(), true),
                reversed: false,
            }],
            signed_area: region.outer.signed_area,
        },
        holes: Vec::new(),
    };
    let again = extrude(&Plane::TOP, &[whole], -2.0, 3.0).unwrap();
    check(&again);
    assert!((measure::volume(&again) - measure::volume(&solid)).abs() < 0.05);
}

#[test]
fn spline_hole_through_a_plate() {
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(&mut s, v(-20.0, -20.0), v(20.0, 20.0));
    s.add_spline(&blob(), true).unwrap();
    let profile = find_regions(&s);
    let plate = &profile.regions[profile.region_at(v(18.0, 18.0)).unwrap()];
    assert_eq!(plate.holes.len(), 1);
    let solid = extrude(&Plane::TOP, std::slice::from_ref(plate), 0.0, 4.0).unwrap();
    check(&solid);
    assert!((measure::volume(&solid) - plate.area() * 4.0).abs() < 0.05);
}

#[test]
fn cutting_a_round_hole_through_a_spline_sided_block() {
    let sketch = wavy_plate();
    let region = only_region(&sketch);
    let block = extrude(&Plane::TOP, std::slice::from_ref(&region), 0.0, 6.0).unwrap();
    // A round hole straight through, clear of the walls.
    let mut s = Sketch::new();
    s.add_circle(v(15.0, 5.0), 2.5);
    let drill = extrude(&Plane::TOP, &find_regions(&s).regions, -1.0, 7.0).unwrap();
    let cut = boolean(&block, &drill, BooleanOp::Subtract).unwrap();
    check(&cut);
    let expected = (region.area() - PI * 2.5 * 2.5) * 6.0;
    assert!((measure::volume(&cut) - expected).abs() < 0.05);

    // A slot across the block, through the freeform wall: front to back along y.
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(&mut s, v(12.0, 2.0), v(18.0, 4.0));
    let slot = extrude(&Plane::front(), &find_regions(&s).regions, -20.0, 20.0).unwrap();
    let cut = boolean(&block, &slot, BooleanOp::Subtract).unwrap();
    check(&cut);
    // The slot is 6 wide and 2 high, and as long as the block is deep where it passes:
    // the area of the profile between x = 12 and x = 18.
    let mut s = wavy_plate();
    s.add_line(v(12.0, -5.0), v(12.0, 20.0));
    s.add_line(v(18.0, -5.0), v(18.0, 20.0));
    let strips = find_regions(&s);
    let middle = &strips.regions[strips.region_at(v(15.0, 5.0)).unwrap()];
    let expected = region.area() * 6.0 - middle.area() * 2.0;
    assert!(
        (measure::volume(&cut) - expected).abs() < 0.05,
        "{} vs {expected}",
        measure::volume(&cut)
    );
    assert!(!tessellate(&cut, 0.05).unwrap().faces.is_empty());
}

/// A vase wall: a spline outside, straight lines inside, clear of the axis (the sketch's
/// y axis).
fn vase() -> Sketch {
    let mut s = Sketch::new();
    s.add_line(v(5.0, 0.0), v(12.0, 0.0));
    s.add_spline(
        &[v(12.0, 0.0), v(18.0, 10.0), v(11.0, 22.0), v(14.0, 30.0)],
        false,
    )
    .unwrap();
    s.add_line(v(14.0, 30.0), v(5.0, 30.0));
    s.add_line(v(5.0, 30.0), v(5.0, 0.0));
    s
}

/// The volume a region sweeps turned by `angle` about the sketch's y axis: Pappus, with
/// the region's first moment about the axis taken from a fine polygon.
fn turned_volume(region: &Region, angle: f64) -> f64 {
    let mut pts: Vec<DVec2> = Vec::new();
    for e in &region.outer.edges {
        let n = 20_000;
        for i in 0..n {
            let t = f64::from(i) / f64::from(n);
            pts.push(e.curve.point_at(if e.reversed { 1.0 - t } else { t }));
        }
    }
    // ∫∫ x dA = ⅙ Σ (x_i + x_j) cross(p_i, p_j)
    let n = pts.len();
    let moment: f64 = (0..n)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            (p.x + q.x) * p.perp_dot(q) / 6.0
        })
        .sum();
    moment.abs() * angle
}

#[test]
fn revolved_spline_partial_and_full_turn() {
    let sketch = vase();
    let region = only_region(&sketch);
    let axis = RevolveAxis {
        origin: DVec2::ZERO,
        dir: DVec2::Y,
    };
    for angle in [1.2, PI, TAU] {
        let (solid, faces) = revolve_traced(
            &Plane::front(),
            std::slice::from_ref(&region),
            &axis,
            0.0,
            angle,
        )
        .unwrap();
        check(&solid);
        let expected = turned_volume(&region, angle);
        let volume = measure::volume(&solid);
        assert!(
            (volume - expected).abs() < 1e-4 * expected,
            "{angle}: {volume} vs {expected}"
        );
        let freeform = solid
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
            .count();
        // All the way round, the spline's face is in two halves.
        assert_eq!(freeform, if angle == TAU { 2 } else { 1 }, "{angle}");
        assert_eq!(faces.len(), solid.faces.len());
        assert!(!tessellate(&solid, 0.05).unwrap().faces.is_empty());
    }
    // The other way about the axis, and from a negative angle.
    let down = RevolveAxis {
        origin: v(0.0, 3.0),
        dir: -DVec2::Y,
    };
    let solid = revolve(
        &Plane::front(),
        std::slice::from_ref(&region),
        &down,
        -0.5,
        1.0,
    )
    .unwrap();
    check(&solid);
    let expected = turned_volume(&region, 1.5);
    assert!((measure::volume(&solid) - expected).abs() < 1e-4 * expected);
}

#[test]
fn a_spline_on_the_axis_is_refused() {
    // A dome: the spline ends on the axis.
    let mut s = Sketch::new();
    s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    s.add_spline(&[v(10.0, 0.0), v(8.0, 6.0), v(0.0, 9.0)], false)
        .unwrap();
    s.add_line(v(0.0, 9.0), v(0.0, 0.0));
    let region = only_region(&s);
    let axis = RevolveAxis {
        origin: DVec2::ZERO,
        dir: DVec2::Y,
    };
    let err = revolve(&Plane::front(), &[region], &axis, 0.0, TAU).unwrap_err();
    assert!(matches!(err, KernelError::Unsupported(_)));
    assert!(err.to_string().contains("spline"), "{err}");
    assert!(err.to_string().contains("line"), "{err}");
}

fn plane_at(z: f64) -> Plane {
    Plane::from_origin_normal_x(DVec3::new(0.0, 0.0, z), DVec3::Z, DVec3::X).unwrap()
}

#[test]
fn loft_between_spline_profiles() {
    let region_of = |scale: f64| {
        let mut s = Sketch::new();
        s.add_spline(&blob().map(|p| p * scale), true).unwrap();
        // Cut the closed spline into the same two sides in every profile.
        only_region(&s)
    };
    let (low, mid, high) = (region_of(1.0), region_of(0.7), region_of(1.1));
    assert_eq!(low.outer.edges.len(), mid.outer.edges.len());
    let (p0, p1, p2) = (plane_at(0.0), plane_at(12.0), plane_at(25.0));
    let two = loft(&[
        LoftSection {
            plane: &p0,
            region: &low,
        },
        LoftSection {
            plane: &p1,
            region: &mid,
        },
    ])
    .unwrap();
    check(&two);
    // A ruled loft between similar outlines: a frustum of the outline.
    let (a0, a1) = (low.area(), mid.area());
    let expected = 12.0 / 3.0 * (a0 + a1 + (a0 * a1).sqrt());
    assert!(
        (measure::volume(&two) - expected).abs() < 1e-3 * expected,
        "{} vs {expected}",
        measure::volume(&two)
    );
    let three = loft(&[
        LoftSection {
            plane: &p0,
            region: &low,
        },
        LoftSection {
            plane: &p1,
            region: &mid,
        },
        LoftSection {
            plane: &p2,
            region: &high,
        },
    ])
    .unwrap();
    check(&three);
    assert!(measure::volume(&three) > measure::volume(&two));
}

#[test]
fn loft_joins_a_spline_side_to_a_straight_one_but_not_to_an_arc() {
    let straight = {
        let mut s = Sketch::new();
        s.add_line(v(0.0, 10.0), v(0.0, 0.0));
        s.add_line(v(0.0, 0.0), v(30.0, 0.0));
        s.add_line(v(30.0, 0.0), v(30.0, 10.0));
        s.add_line(v(30.0, 10.0), v(0.0, 10.0));
        only_region(&s)
    };
    let wavy = only_region(&wavy_plate());
    let (p0, p1) = (plane_at(0.0), plane_at(15.0));
    let solid = loft(&[
        LoftSection {
            plane: &p0,
            region: &straight,
        },
        LoftSection {
            plane: &p1,
            region: &wavy,
        },
    ])
    .unwrap();
    check(&solid);
    // Ruled between profiles of the same width that differ only in their top edge: the
    // cross-section's area changes linearly.
    let expected = 15.0 * 0.5 * (straight.area() + wavy.area());
    assert!((measure::volume(&solid) - expected).abs() < 1e-3 * expected);

    let arched = {
        let mut s = Sketch::new();
        s.add_line(v(0.0, 10.0), v(0.0, 0.0));
        s.add_line(v(0.0, 0.0), v(30.0, 0.0));
        s.add_line(v(30.0, 0.0), v(30.0, 10.0));
        s.add_arc(v(15.0, 10.0), v(30.0, 10.0), v(0.0, 10.0));
        only_region(&s)
    };
    let err = loft(&[
        LoftSection {
            plane: &p0,
            region: &arched,
        },
        LoftSection {
            plane: &p1,
            region: &wavy,
        },
    ])
    .unwrap_err();
    assert!(matches!(err, KernelError::Unsupported(_)), "{err}");
    assert!(err.to_string().contains("spline") && err.to_string().contains("arc"));
}
