//! STEP import: PeetCAD's own exports read back as the same solids, files written the way
//! other programs write them (inch units, edges against their curves, unmarked outer
//! bounds, periodic faces without seams, vertex loops, degrees; B-splines with every kind
//! of knot vector, rational ones, closed ones, swept surfaces) come out as valid kernel
//! solids, and broken files give errors rather than panics.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fmt::Write as _;
use std::sync::Arc;

use peet_io::step::{self, StepOptions, StepSchema};
use peet_io::step_import::{StepImport, read};
use peet_kernel::boolean::{BooleanOp, boolean};
use peet_kernel::extrude::extrude;
use peet_kernel::loft::{LoftSection, loft};
use peet_kernel::nurbs::{NurbsCurve, NurbsSurface};
use peet_kernel::revolve::{RevolveAxis, revolve};
use peet_kernel::validate::{Counts, measure, validate};
use peet_kernel::{Curve3, Solid, Surface};
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::region::{Region, find_regions};
use peet_sketch::{Sketch, shapes};

// ---- Helpers ----

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

#[track_caller]
fn import(text: &str) -> StepImport {
    match read(text) {
        Ok(i) => i,
        Err(e) => panic!("import failed: {e}"),
    }
}

#[track_caller]
fn import_error(text: &str) -> String {
    match read(text) {
        Ok(i) => panic!("import should fail, but gave {} bodies", i.bodies.len()),
        Err(e) => {
            // `Display` and the message agree, and the message is a sentence.
            assert_eq!(e.to_string(), e.message);
            assert!(e.message.len() > 20 && e.message.ends_with('.'), "{e}");
            e.message
        }
    }
}

#[track_caller]
fn valid(solid: &Solid) -> Counts {
    match validate(solid) {
        Ok(c) => c,
        Err(problems) => panic!("invalid solid: {problems:#?}"),
    }
}

fn tuple(c: Counts) -> (usize, usize, usize, usize, usize, i64) {
    (c.vertices, c.edges, c.faces, c.rings, c.shells, c.genus)
}

fn area(s: &Solid) -> f64 {
    s.face_ids().map(|f| measure::face_area(s, f)).sum()
}

#[track_caller]
fn assert_close(got: f64, expected: f64) {
    assert!(
        (got - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

fn options(schema: StepSchema) -> StepOptions {
    StepOptions {
        schema,
        product_name: "Test part".to_owned(),
        author: "Bill".to_owned(),
        organization: String::new(),
        timestamp: "2026-10-03T12:00:00".to_owned(),
    }
}

/// Exports `solid`, imports the file and checks that the same solid comes back: the same
/// numbers of everything, the same volume and area, valid, named as it was. A solid of
/// several lumps comes back as a body per lump; the first is returned.
#[track_caller]
fn round_trip_as(solid: &Solid, schema: StepSchema) -> Solid {
    round_trip_within(solid, schema, 1e-9)
}

/// [`round_trip_as`] with the volume and the area the same to within `tolerance`
/// (relative).
#[track_caller]
fn round_trip_within(solid: &Solid, schema: StepSchema, tolerance: f64) -> Solid {
    valid(solid);
    let text = step::write(&[("Part", solid)], &options(schema));
    let imported = import(&text);
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.product_name, "Test part");
    let lumps = (0..solid.shells.len() as u32)
        .filter(|&k| measure::shell_volume(solid, peet_kernel::ShellId(k)) > 0.0)
        .count();
    assert_eq!(imported.bodies.len(), lumps);
    let mut sizes = [0usize; 6];
    let (mut volume, mut total_area) = (0.0, 0.0);
    for body in &imported.bodies {
        assert_eq!(body.name, "Part");
        valid(&body.solid);
        let s = &body.solid;
        let counts = [
            s.vertices.len(),
            s.edges.len(),
            s.coedges.len(),
            s.loops.len(),
            s.faces.len(),
            s.shells.len(),
        ];
        for (total, n) in sizes.iter_mut().zip(counts) {
            *total += n;
        }
        volume += measure::volume(s);
        total_area += area(s);
    }
    assert_eq!(
        sizes,
        [
            solid.vertices.len(),
            solid.edges.len(),
            solid.coedges.len(),
            solid.loops.len(),
            solid.faces.len(),
            solid.shells.len(),
        ],
        "vertices, edges, coedges, loops, faces, shells"
    );
    for (got, expected) in [(volume, measure::volume(solid)), (total_area, area(solid))] {
        assert!(
            (got - expected).abs() <= tolerance * expected.abs().max(1.0),
            "{got} instead of {expected}"
        );
    }
    imported.bodies[0].solid.clone()
}

#[track_caller]
fn round_trip(solid: &Solid) -> Solid {
    round_trip_as(solid, StepSchema::Ap242);
    round_trip_as(solid, StepSchema::Ap214)
}

fn regions(s: &Sketch) -> Vec<Region> {
    find_regions(s).regions
}

fn block(plane: &Plane, a: DVec2, b: DVec2, from: f64, to: f64) -> Solid {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, a, b);
    extrude(plane, &regions(&s), from, to).unwrap()
}

/// A closed polygon.
fn polygon(points: &[DVec2]) -> Sketch {
    let mut s = Sketch::new();
    for i in 0..points.len() {
        s.add_line(points[i], points[(i + 1) % points.len()]);
    }
    s
}

/// Revolves the region at `at` about the sketch's Y axis on the front plane (the world Z
/// axis).
fn turned(s: &Sketch, at: DVec2, from: f64, to: f64) -> Solid {
    let profile = find_regions(s);
    let i = profile.region_at(at).expect("a region at the pick point");
    let axis = RevolveAxis {
        origin: DVec2::ZERO,
        dir: DVec2::Y,
    };
    revolve(
        &Plane::front(),
        &[profile.regions[i].clone()],
        &axis,
        from,
        to,
    )
    .unwrap()
}

// ---- Round trips through PeetCAD's own exporter ----

#[test]
fn box_round_trips() {
    let solid = block(&Plane::TOP, DVec2::ZERO, v2(40.0, 20.0), 0.0, 5.0);
    let back = round_trip(&solid);
    assert_eq!(tuple(valid(&back)), (8, 12, 6, 0, 1, 0));
    assert_close(measure::volume(&back), 4000.0);
    let b = back.bounds();
    assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-12) && b.max.abs_diff_eq(v3(40.0, 20.0, 5.0), 1e-12));
}

#[test]
fn plate_with_holes_and_a_slot_round_trips() {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, v2(60.0, 30.0));
    s.add_circle(v2(10.0, 10.0), 3.0);
    s.add_circle(v2(50.0, 20.0), 4.5);
    shapes::slot(&mut s, v2(22.0, 15.0), v2(38.0, 15.0), 2.5);
    let plate: Vec<Region> = regions(&s)
        .into_iter()
        .filter(|r| r.holes.len() == 3)
        .collect();
    assert_eq!(plate.len(), 1);
    // On a tilted plane, so nothing lines up with the world axes.
    let plane = Plane {
        frame: Frame {
            origin: v3(5.0, -3.0, 2.0),
            rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3, -0.7, 1.9),
        },
    };
    let solid = extrude(&plane, &plate, 0.0, 2.0).unwrap();
    let back = round_trip(&solid);
    let c = valid(&back);
    assert_eq!((c.rings, c.genus), (6, 3));
    // The hole walls face into the holes.
    let walls: Vec<_> = back
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Cylinder(_)))
        .collect();
    assert!(walls.len() >= 4 && walls.iter().all(|f| f.reversed));
}

#[test]
fn turned_shapes_round_trip() {
    // A rod: a cylinder wall with a seam between two discs.
    let rod = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(5.0, 10.0), v2(0.0, 10.0)]);
    let back = round_trip(&turned(&rod, v2(2.0, 5.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(&back), PI * 250.0);

    // A tube: the bore faces the axis.
    let tube = polygon(&[v2(3.0, 0.0), v2(5.0, 0.0), v2(5.0, 10.0), v2(3.0, 10.0)]);
    let back = round_trip(&turned(&tube, v2(4.0, 5.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (4, 6, 4, 2, 1, 1));

    // A cone with its apex, upright and upside down (a negative half angle, which is
    // written about the opposite axis), and a frustum.
    let cone = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(0.0, 10.0)]);
    let back = round_trip(&turned(&cone, v2(1.0, 1.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (2, 2, 2, 0, 1, 0));
    assert_close(measure::volume(&back), PI * 250.0 / 3.0);
    let cone = polygon(&[v2(0.0, 0.0), v2(5.0, 10.0), v2(0.0, 10.0)]);
    round_trip(&turned(&cone, v2(1.0, 8.0), 0.0, TAU));
    let frustum = polygon(&[v2(0.0, 0.0), v2(6.0, 0.0), v2(3.0, 4.0), v2(0.0, 4.0)]);
    round_trip(&turned(&frustum, v2(1.0, 1.0), 0.0, TAU));
    // Two cones tip to tip: a vertex each at the tip.
    let groove = polygon(&[v2(0.0, 0.0), v2(4.0, -4.0), v2(4.0, 4.0)]);
    let back = round_trip(&turned(&groove, v2(3.0, 0.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (4, 5, 3, 0, 1, 0));

    // A ball: one face, one seam from pole to pole.
    let mut ball = Sketch::new();
    ball.add_arc(DVec2::ZERO, v2(0.0, -5.0), v2(0.0, 5.0));
    ball.add_line(v2(0.0, 5.0), v2(0.0, -5.0));
    let back = round_trip(&turned(&ball, v2(2.0, 0.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (2, 1, 1, 0, 1, 0));
    assert_close(measure::volume(&back), 4.0 / 3.0 * PI * 125.0);
    assert_close(area(&back), 4.0 * PI * 25.0);

    // A ring: one face, two seams.
    let mut ring = Sketch::new();
    ring.add_circle(v2(10.0, 0.0), 3.0);
    let back = round_trip(&turned(&ring, v2(10.5, 0.5), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (1, 2, 1, 0, 1, 1));
    assert_close(measure::volume(&back), 2.0 * PI * PI * 90.0);

    // A puck with a rounded rim: flat, torus, cylinder, torus, flat.
    let mut puck = Sketch::new();
    puck.add_line(v2(0.0, 0.0), v2(8.0, 0.0));
    puck.add_arc(v2(8.0, 2.0), v2(8.0, 0.0), v2(10.0, 2.0));
    puck.add_line(v2(10.0, 2.0), v2(10.0, 4.0));
    puck.add_arc(v2(8.0, 4.0), v2(10.0, 4.0), v2(8.0, 6.0));
    puck.add_line(v2(8.0, 6.0), v2(0.0, 6.0));
    puck.add_line(v2(0.0, 6.0), v2(0.0, 0.0));
    let back = round_trip(&turned(&puck, v2(4.0, 3.0), 0.0, TAU));
    assert_eq!(tuple(valid(&back)), (4, 7, 5, 0, 1, 0));
}

#[test]
fn partial_turns_round_trip() {
    let tube = polygon(&[v2(2.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(2.0, 4.0)]);
    let back = round_trip(&turned(&tube, v2(3.0, 2.0), 0.0, FRAC_PI_2));
    assert_eq!(tuple(valid(&back)), (8, 12, 6, 0, 1, 0));
    assert_close(measure::volume(&back), PI * 21.0);
    // A wedge with an edge on the axis, a slice of a cone, of a ball and of a ring.
    let rod = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(0.0, 4.0)]);
    round_trip(&turned(&rod, v2(3.0, 2.0), -0.5, 1.0));
    let cone = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(0.0, 10.0)]);
    round_trip(&turned(&cone, v2(1.0, 1.0), 0.3, 4.0));
    let mut ball = Sketch::new();
    ball.add_arc(DVec2::ZERO, v2(0.0, -5.0), v2(0.0, 5.0));
    ball.add_line(v2(0.0, 5.0), v2(0.0, -5.0));
    let back = round_trip(&turned(&ball, v2(2.0, 0.0), 0.0, 2.0));
    assert_eq!(tuple(valid(&back)), (2, 3, 3, 0, 1, 0));
    let mut ring = Sketch::new();
    ring.add_circle(v2(10.0, 0.0), 3.0);
    let back = round_trip(&turned(&ring, v2(10.5, 0.5), 1.0, 4.0));
    assert_eq!(tuple(valid(&back)), (2, 3, 3, 0, 1, 0));
}

#[test]
fn voids_and_lumps_round_trip() {
    let outer = block(&Plane::TOP, DVec2::ZERO, DVec2::splat(10.0), 0.0, 10.0);
    let inner = block(&Plane::TOP, DVec2::splat(3.0), DVec2::splat(7.0), 3.0, 7.0);
    let hollow = boolean(&outer, &inner, BooleanOp::Subtract).unwrap();
    assert_eq!(hollow.shells.len(), 2);
    let back = round_trip(&hollow);
    assert_eq!(back.shells.len(), 2);
    assert_close(measure::volume(&back), 1000.0 - 64.0);
    assert!(measure::shell_volume(&back, peet_kernel::ShellId(0)) > 0.0);
    assert!(measure::shell_volume(&back, peet_kernel::ShellId(1)) < 0.0);

    // A hollow turned part: a full turn makes the hole of the profile a void.
    let mut s = polygon(&[v2(2.0, 0.0), v2(10.0, 0.0), v2(10.0, 8.0), v2(2.0, 8.0)]);
    s.add_circle(v2(6.0, 4.0), 2.0);
    let hollow = turned(&s, v2(3.0, 1.0), 0.0, TAU);
    assert_eq!(hollow.shells.len(), 2);
    round_trip(&hollow);

    // Two lumps in one solid come back as two bodies.
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, DVec2::splat(10.0));
    s.add_circle(v2(30.0, 5.0), 4.0);
    let two = extrude(&Plane::front(), &regions(&s), 0.0, 1.0).unwrap();
    assert_eq!(two.shells.len(), 2);
    round_trip(&two);
}

#[test]
fn slanted_cut_with_an_ellipse_round_trips() {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, 5.0);
    let rod = extrude(&Plane::TOP, &regions(&s), 0.0, 20.0).unwrap();
    let plane = Plane::from_origin_normal_x(
        v3(0.0, 0.0, 12.0),
        DQuat::from_rotation_x(PI / 6.0) * DVec3::Z,
        DVec3::X,
    )
    .unwrap();
    let cutter = block(&plane, DVec2::splat(-20.0), DVec2::splat(20.0), 0.0, 30.0);
    let cut = boolean(&rod, &cutter, BooleanOp::Subtract).unwrap();
    let back = round_trip(&cut);
    assert!(
        back.edges
            .iter()
            .any(|e| matches!(e.curve, peet_kernel::Curve3::Ellipse(_)))
    );
}

#[test]
fn chassis_round_trips() {
    // The Phase 5 sheet metal sample: mitred rims, a hem, louvers and dimples.
    let (_, engine) = peet_model::samples::chassis();
    let body = &engine.evaluation().bodies[0];
    round_trip(&body.solid);
    let sheet = body.sheet.as_ref().unwrap();
    round_trip_as(&sheet.flat, StepSchema::Ap214);
}

#[test]
fn names_and_several_bodies() {
    let a = block(&Plane::TOP, DVec2::ZERO, DVec2::ONE, 0.0, 1.0);
    let b = block(&Plane::TOP, v2(5.0, 0.0), v2(7.0, 1.0), 0.0, 1.0);
    let mut o = options(StepSchema::Ap214);
    o.product_name = "Bill's Größe".to_owned();
    let text = step::write(&[("Körper", &a), ("", &b)], &o);
    let imported = import(&text);
    assert_eq!(imported.product_name, "Bill's Größe");
    let names: Vec<&str> = imported.bodies.iter().map(|b| b.name.as_str()).collect();
    // A brep without a name takes its product's.
    assert_eq!(names, ["Körper", "Bill's Größe"]);
    assert_close(measure::volume(&imported.bodies[1].solid), 2.0);
}

// ---- Freeform solids through PeetCAD's own exporter ----

fn plane_at(z: f64) -> Plane {
    Plane::from_origin_normal_x(v3(0.0, 0.0, z), DVec3::Z, DVec3::X).unwrap()
}

fn rectangle(w: f64, h: f64) -> Region {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, v2(-w / 2.0, -h / 2.0), v2(w / 2.0, h / 2.0));
    regions(&s).remove(0)
}

fn disc(r: f64) -> Region {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, r);
    regions(&s).remove(0)
}

#[track_caller]
fn lofted(sections: &[(Plane, Region)]) -> Solid {
    let list: Vec<LoftSection<'_>> = sections
        .iter()
        .map(|(plane, region)| LoftSection { plane, region })
        .collect();
    match loft(&list) {
        Ok(solid) => solid,
        Err(e) => panic!("loft failed: {e}"),
    }
}

fn freeform_faces(s: &Solid) -> usize {
    s.faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
        .count()
}

fn freeform_edges(s: &Solid) -> usize {
    s.edges
        .iter()
        .filter(|e| matches!(e.curve, Curve3::Nurbs(_)))
        .count()
}

/// [`round_trip`] for a solid with freeform faces, whose measures come from numerical
/// integration: the same to seven digits. The freeform faces and edges come back as
/// freeform ones, on the same surfaces and curves.
#[track_caller]
fn freeform_round_trip(solid: &Solid) -> Solid {
    for schema in [StepSchema::Ap242, StepSchema::Ap214] {
        let back = round_trip_within(solid, schema, 1e-7);
        assert_eq!(freeform_faces(&back), freeform_faces(solid));
        assert_eq!(freeform_edges(&back), freeform_edges(solid));
        for (a, b) in solid.faces.iter().zip(&back.faces) {
            if matches!(a.surface, Surface::Nurbs(_)) {
                assert_eq!(a.surface, b.surface);
            }
            assert_eq!(a.reversed, b.reversed);
        }
        // (The importer numbers edges in the order the faces use them.)
        for a in solid.edges.iter().filter(|e| e.curve.domain().is_some()) {
            let same = |b: &&peet_kernel::topo::Edge| {
                b.curve == a.curve && (a.t0 - b.t0).abs() < 1e-9 && (a.t1 - b.t1).abs() < 1e-9
            };
            assert!(back.edges.iter().any(|b| same(&b)), "{:?}", a.curve);
        }
    }
    round_trip_within(solid, StepSchema::Ap214, 1e-7)
}

#[test]
fn twisted_loft_round_trips() {
    // A rectangle turned by 30 degrees on the way up: four ruled freeform sides.
    let top = Plane {
        frame: Frame {
            origin: v3(0.0, 0.0, 10.0),
            rotation: DQuat::from_rotation_z(30f64.to_radians()),
        },
    };
    let solid = lofted(&[
        (plane_at(0.0), rectangle(12.0, 8.0)),
        (top, rectangle(12.0, 8.0)),
    ]);
    assert_eq!(freeform_faces(&solid), 4);
    let back = freeform_round_trip(&solid);
    assert_eq!(tuple(valid(&back)), (8, 12, 6, 0, 1, 0));
}

#[test]
fn lofts_between_circles_and_squares_round_trip() {
    // A cone frustum between two circles: rational sides.
    let frustum = lofted(&[(plane_at(0.0), disc(10.0)), (plane_at(15.0), disc(4.0))]);
    assert_eq!(freeform_faces(&frustum), 4);
    assert!(frustum.faces.iter().any(|f| match &f.surface {
        Surface::Nurbs(s) => s.weights().is_some(),
        _ => false,
    }));
    let back = freeform_round_trip(&frustum);
    assert!(
        (measure::volume(&back) - PI * 15.0 / 3.0 * (100.0 + 40.0 + 16.0)).abs() < 1e-5,
        "{}",
        measure::volume(&back)
    );
    // A square duct that becomes round.
    let duct = lofted(&[
        (plane_at(0.0), rectangle(20.0, 20.0)),
        (plane_at(25.0), disc(8.0)),
    ]);
    freeform_round_trip(&duct);
}

#[test]
fn smooth_lofts_round_trip() {
    // A vase through four circles: cubic in the direction of the loft, with freeform
    // rails for edges.
    let vase = lofted(&[
        (plane_at(0.0), disc(6.0)),
        (plane_at(10.0), disc(10.0)),
        (plane_at(22.0), disc(5.0)),
        (plane_at(30.0), disc(7.0)),
    ]);
    assert_eq!(freeform_edges(&vase), 4);
    freeform_round_trip(&vase);
    // Three pentagons, the middle one bigger and turned.
    let pentagon = |r: f64, turn: f64| {
        let points: Vec<DVec2> = (0..5)
            .map(|k| DVec2::from_angle(turn + f64::from(k) * TAU / 5.0) * r)
            .collect();
        regions(&polygon(&points)).remove(0)
    };
    let solid = lofted(&[
        (plane_at(0.0), pentagon(8.0, 0.0)),
        (plane_at(12.0), pentagon(12.0, 0.3)),
        (plane_at(20.0), pentagon(6.0, 0.6)),
    ]);
    assert_eq!(solid.faces.len(), 7);
    freeform_round_trip(&solid);
}

// ---- Files written by hand, the way other programs write them ----

/// Writes numbered entities for hand-made test files.
#[derive(Default)]
struct Step {
    data: String,
    n: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum UnitStyle {
    Millimetres,
    Inches,
    /// Centimetres and degrees.
    CentimetresDegrees,
    /// No units at all.
    Unstated,
}

/// An edge as a loop uses it: the `EDGE_CURVE` and the `ORIENTED_EDGE` orientation.
type EdgeUse = (u32, bool);

fn logical(b: bool) -> &'static str {
    if b { ".T." } else { ".F." }
}

fn refs(ids: &[u32]) -> String {
    let list: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
    format!("({})", list.join(","))
}

impl Step {
    fn add(&mut self, entity: impl AsRef<str>) -> u32 {
        self.n += 1;
        let _ = writeln!(self.data, "#{} = {} ;", self.n, entity.as_ref());
        self.n
    }

    fn point(&mut self, p: DVec3) -> u32 {
        self.add(format!(
            "CARTESIAN_POINT ( 'NONE', ( {}, {}, {} ) )",
            step::real(p.x),
            step::real(p.y),
            step::real(p.z)
        ))
    }

    fn dir(&mut self, d: DVec3) -> u32 {
        self.add(format!(
            "DIRECTION('',({},{},{}))",
            step::real(d.x),
            step::real(d.y),
            step::real(d.z)
        ))
    }

    fn axis(&mut self, origin: DVec3, z: DVec3, x: DVec3) -> u32 {
        let (o, z, x) = (self.point(origin), self.dir(z), self.dir(x));
        self.add(format!("AXIS2_PLACEMENT_3D('',#{o},#{z},#{x})"))
    }

    fn vertex(&mut self, p: DVec3) -> u32 {
        let p = self.point(p);
        self.add(format!("VERTEX_POINT('',#{p})"))
    }

    /// The line from `a` towards `b`, with a vector as long as the stretch between them.
    fn line(&mut self, a: DVec3, b: DVec3) -> u32 {
        let (o, d) = (self.point(a), self.dir((b - a).normalize()));
        let v = self.add(format!("VECTOR('',#{d},{})", step::real(a.distance(b))));
        self.add(format!("LINE('',#{o},#{v})"))
    }

    fn circle(&mut self, centre: DVec3, z: DVec3, x: DVec3, radius: f64) -> u32 {
        let axis = self.axis(centre, z, x);
        self.add(format!("CIRCLE('',#{axis},{})", step::real(radius)))
    }

    fn edge(&mut self, start: u32, end: u32, curve: u32, same_sense: bool) -> u32 {
        self.add(format!(
            "EDGE_CURVE('',#{start},#{end},#{curve},{})",
            logical(same_sense)
        ))
    }

    fn bound(&mut self, kind: &str, uses: &[EdgeUse], orientation: bool) -> u32 {
        let oriented: Vec<u32> = uses
            .iter()
            .map(|&(e, o)| self.add(format!("ORIENTED_EDGE('',*,*,#{e},{})", logical(o))))
            .collect();
        let l = self.add(format!("EDGE_LOOP('',{})", refs(&oriented)));
        self.add(format!("{kind}('',#{l},{})", logical(orientation)))
    }

    fn vertex_loop(&mut self, at: DVec3) -> u32 {
        let v = self.vertex(at);
        let l = self.add(format!("VERTEX_LOOP('',#{v})"));
        self.add(format!("FACE_BOUND('',#{l},.T.)"))
    }

    fn face(&mut self, bounds: &[u32], surface: u32, same_sense: bool) -> u32 {
        self.add(format!(
            "ADVANCED_FACE('',{},#{surface},{})",
            refs(bounds),
            logical(same_sense)
        ))
    }

    fn plane(&mut self, origin: DVec3, normal: DVec3, x: DVec3) -> u32 {
        let axis = self.axis(origin, normal, x);
        self.add(format!("PLANE('',#{axis})"))
    }

    fn shell(&mut self, faces: &[u32]) -> u32 {
        self.add(format!("CLOSED_SHELL('',{})", refs(faces)))
    }

    fn solid(&mut self, name: &str, faces: &[u32]) -> u32 {
        let shell = self.shell(faces);
        self.add(format!("MANIFOLD_SOLID_BREP('{name}',#{shell})"))
    }

    /// The units and the representation context, as complex entities.
    fn context(&mut self, units: UnitStyle) -> u32 {
        let metre = |w: &mut Self, prefix: &str| {
            w.add(format!(
                "( LENGTH_UNIT ( ) NAMED_UNIT ( * ) SI_UNIT ( {prefix}, .METRE. ) )"
            ))
        };
        let radian = self.add("( NAMED_UNIT ( * ) PLANE_ANGLE_UNIT ( ) SI_UNIT ( $, .RADIAN. ) )");
        let steradian =
            self.add("( NAMED_UNIT ( * ) SI_UNIT ( $, .STERADIAN. ) SOLID_ANGLE_UNIT ( ) )");
        let (length, angle) = match units {
            UnitStyle::Millimetres => (metre(self, ".MILLI."), radian),
            UnitStyle::Inches => {
                let mm = metre(self, ".MILLI.");
                let measure = self.add(format!(
                    "LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#{mm})"
                ));
                let dimensions = self.add("DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.)");
                let inch = self.add(format!(
                    "( CONVERSION_BASED_UNIT ( 'INCH', #{measure} ) LENGTH_UNIT ( ) \
                     NAMED_UNIT ( #{dimensions} ) )"
                ));
                (inch, radian)
            }
            UnitStyle::CentimetresDegrees => {
                let cm = metre(self, ".CENTI.");
                let measure = self.add(format!(
                    "PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.0174532925),#{radian})"
                ));
                let dimensions = self.add("DIMENSIONAL_EXPONENTS(0.,0.,0.,0.,0.,0.,0.)");
                let degree = self.add(format!(
                    "(CONVERSION_BASED_UNIT('DEGREE',#{measure})NAMED_UNIT(#{dimensions})\
                     PLANE_ANGLE_UNIT())"
                ));
                (cm, degree)
            }
            UnitStyle::Unstated => {
                return self
                    .add("( GEOMETRIC_REPRESENTATION_CONTEXT(3) REPRESENTATION_CONTEXT('','') )");
            }
        };
        let uncertainty = self.add(format!(
            "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-05),#{length},\
             'distance_accuracy_value','NONE')"
        ));
        self.add(format!(
            "( GEOMETRIC_REPRESENTATION_CONTEXT ( 3 ) \
             GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT ( ( #{uncertainty} ) ) \
             GLOBAL_UNIT_ASSIGNED_CONTEXT ( ( #{length}, #{angle}, #{steradian} ) ) \
             REPRESENTATION_CONTEXT ( 'NONE', 'WORKASPACE' ) )"
        ))
    }

    /// A product called `name` whose shape is the representation `rep`. Returns the
    /// product definition.
    fn product(&mut self, name: &str, rep: u32) -> u32 {
        let app = self.add("APPLICATION_CONTEXT('automotive_design')");
        let context = self.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
        let product = self.add(format!("PRODUCT('{name}','{name}','',(#{context}))"));
        let formation = self.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
        let definition_context = self.add(format!(
            "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
        ));
        let definition = self.add(format!(
            "PRODUCT_DEFINITION('design','',#{formation},#{definition_context})"
        ));
        let shape = self.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
        self.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
        definition
    }

    /// A representation of `items` in `context`.
    fn representation(&mut self, items: &[u32], context: u32) -> u32 {
        let origin = self.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
        let mut all = vec![origin];
        all.extend(items);
        self.add(format!(
            "ADVANCED_BREP_SHAPE_REPRESENTATION('',{},#{context})",
            refs(&all)
        ))
    }

    fn file(self) -> String {
        format!(
            "ISO-10303-21;\nHEADER;\n/* made by hand; a comment with a ' and DATA; in it */\n\
             FILE_DESCRIPTION (( 'STEP AP214' ),\n    '1' );\n\
             FILE_NAME ('widget.STEP', '2026-10-03T12:00:00', ( '' ), ( '' ), 'by hand', \
             'DATA; ENDSEC;', '' );\nFILE_SCHEMA (( 'AUTOMOTIVE_DESIGN' ));\nENDSEC;\n\nDATA;\n\
             {}ENDSEC;\nEND-ISO-10303-21;\n",
            self.data
        )
    }

    /// One part called "Widget" holding `breps`.
    fn part(mut self, breps: &[u32], units: UnitStyle) -> String {
        let context = self.context(units);
        let rep = self.representation(breps, context);
        self.product("Widget", rep);
        self.file()
    }
}

/// The faces of a box. With `sloppy`, the way careless writers do it: every other edge is
/// written from its far end with `same_sense = .F.`, every other bound backwards with
/// orientation `.F.`, and no bound is marked as the outer one. With `inward`, the faces
/// point into the box. `holes` are more bounds for the bottom (−Z) and the top (+Z) face,
/// written before the outer bound.
fn box_faces(
    w: &mut Step,
    min: DVec3,
    max: DVec3,
    sloppy: bool,
    inward: bool,
    holes: [&[Vec<EdgeUse>]; 2],
) -> Vec<u32> {
    let c = |i: usize| {
        v3(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    };
    let v: Vec<u32> = (0..8).map(|i| w.vertex(c(i))).collect();
    // Four corners, counter-clockwise seen from outside, and the normal.
    let sides: [([usize; 4], DVec3); 6] = [
        ([0, 2, 3, 1], -DVec3::Z),
        ([4, 5, 7, 6], DVec3::Z),
        ([0, 1, 5, 4], -DVec3::Y),
        ([2, 6, 7, 3], DVec3::Y),
        ([0, 4, 6, 2], -DVec3::X),
        ([1, 3, 7, 5], DVec3::X),
    ];
    // Per pair of corners: the edge and the corner it starts from.
    let mut edges: HashMap<(usize, usize), (u32, usize)> = HashMap::new();
    let mut faces = Vec::new();
    for (k, (corners, normal)) in sides.into_iter().enumerate() {
        let mut uses: Vec<EdgeUse> = Vec::new();
        for j in 0..4 {
            let (a, b) = (corners[j], corners[(j + 1) % 4]);
            let key = (a.min(b), a.max(b));
            let count = edges.len();
            let (edge, from) = *edges.entry(key).or_insert_with(|| {
                let line = w.line(c(key.0), c(key.1));
                if sloppy && count % 2 == 1 {
                    (w.edge(v[key.1], v[key.0], line, false), key.1)
                } else {
                    (w.edge(v[key.0], v[key.1], line, true), key.0)
                }
            });
            uses.push((edge, from == a));
        }
        let mut orientation = true;
        if sloppy && k % 2 == 0 {
            uses.reverse();
            for u in &mut uses {
                u.1 = !u.1;
            }
            orientation = false;
        }
        let mut bounds = Vec::new();
        if k < 2 {
            for hole in holes[k] {
                bounds.push(w.bound("FACE_BOUND", hole, !inward));
            }
        }
        let kind = if sloppy {
            "FACE_BOUND"
        } else {
            "FACE_OUTER_BOUND"
        };
        bounds.push(w.bound(kind, &uses, orientation != inward));
        let plane = w.plane(c(corners[0]), normal, c(corners[1]) - c(corners[0]));
        faces.push(w.face(&bounds, plane, !inward));
    }
    faces
}

const NO_HOLES: [&[Vec<EdgeUse>]; 2] = [&[], &[]];

#[test]
fn inch_box_with_reversed_edges_and_unmarked_bounds() {
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        true,
        false,
        NO_HOLES,
    );
    let brep = w.solid("", &faces);
    let text = w.part(&[brep], UnitStyle::Inches);
    assert!(text.contains(".F.") && !text.contains("FACE_OUTER_BOUND"));
    let imported = import(&text);
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.product_name, "Widget");
    assert_eq!(imported.bodies.len(), 1);
    assert_eq!(imported.bodies[0].name, "Widget");
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (8, 12, 6, 0, 1, 0));
    assert_close(measure::volume(s), 6.0 * 25.4f64.powi(3));
    let b = s.bounds();
    assert!(b.min.abs_diff_eq(DVec3::ZERO, 1e-9));
    assert!(b.max.abs_diff_eq(v3(25.4, 50.8, 76.2), 1e-9), "{b:?}");
    // Every edge runs from its start to its end along its line.
    for e in &s.edges {
        let (a, b) = (s.vertex(e.start).point, s.vertex(e.end).point);
        assert!(e.t1 > e.t0 && (b - a).dot(e.curve.tangent(e.t0)) > 0.0);
    }
}

#[test]
fn file_without_units_is_taken_as_millimetres() {
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Block", &faces);
    let imported = import(&w.part(&[brep], UnitStyle::Unstated));
    assert_eq!(imported.bodies[0].name, "Block");
    assert_close(measure::volume(&imported.bodies[0].solid), 6.0);
    assert_eq!(imported.warnings.len(), 1);
    assert!(
        imported.warnings[0].contains("millimetres"),
        "{:?}",
        imported.warnings
    );
}

#[test]
fn inside_out_body_is_turned_round() {
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        true,
        NO_HOLES,
    );
    let brep = w.solid("Block", &faces);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    assert_close(measure::volume(&imported.bodies[0].solid), 6.0);
    valid(&imported.bodies[0].solid);
    assert!(
        imported.warnings[0].contains("inside out"),
        "{:?}",
        imported.warnings
    );
}

/// A 40 × 20 × 5 plate with a Ø6 hole whose wall is two half cylinders (the way SolidWorks
/// and others split periodic faces). The hole's bounds come before the outer bounds, all
/// plain `FACE_BOUND`s, and half of the arcs run against their circles.
#[test]
fn plate_with_a_split_hole_and_outer_bounds_listed_last() {
    let mut w = Step::default();
    let (centre, r, h) = (v3(10.0, 10.0, 0.0), 3.0, 5.0);
    let up = DVec3::Z * h;
    let (p0, p1) = (centre + DVec3::X * r, centre - DVec3::X * r);
    let (vp0, vp1, vq0, vq1) = (
        w.vertex(p0),
        w.vertex(p1),
        w.vertex(p0 + up),
        w.vertex(p1 + up),
    );
    let bottom_circle = w.circle(centre, DVec3::Z, DVec3::X, r);
    let top_circle = w.circle(centre + up, DVec3::Z, DVec3::X, r);
    // The far halves run counter-clockwise; the near halves are written clockwise.
    let b_far = w.edge(vp0, vp1, bottom_circle, true);
    let b_near = w.edge(vp0, vp1, bottom_circle, false);
    let t_far = w.edge(vq0, vq1, top_circle, true);
    let t_near = w.edge(vq0, vq1, top_circle, false);
    let line0 = w.line(p0, p0 + up);
    let line1 = w.line(p1, p1 + up);
    let l0 = w.edge(vp0, vq0, line0, true);
    let l1 = w.edge(vp1, vq1, line1, true);
    let bottom_hole = vec![(b_far, true), (b_near, false)];
    let top_hole = vec![(t_near, true), (t_far, false)];
    let mut faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(40.0, 20.0, h),
        true,
        false,
        [&[bottom_hole], &[top_hole]],
    );
    // The wall faces the hole's axis: against the cylinder's normal.
    let axis = w.axis(centre, DVec3::Z, DVec3::X);
    let cylinder = w.add(format!("CYLINDRICAL_SURFACE('',#{axis},3.0)"));
    let far = w.bound(
        "FACE_BOUND",
        &[(l0, true), (t_far, true), (l1, false), (b_far, false)],
        true,
    );
    faces.push(w.face(&[far], cylinder, false));
    let near = w.bound(
        "FACE_BOUND",
        &[(l1, true), (t_near, false), (l0, false), (b_near, true)],
        true,
    );
    faces.push(w.face(&[near], cylinder, false));
    let brep = w.solid("Plate", &faces);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (12, 18, 8, 2, 1, 1));
    assert_close(measure::volume(s), 40.0 * 20.0 * 5.0 - PI * 9.0 * 5.0);
    // The arcs written clockwise run counter-clockwise about a flipped axis now.
    let flipped = s
        .edges
        .iter()
        .filter(|e| matches!(e.curve, peet_kernel::Curve3::Circle(c) if c.frame.z_axis().z < 0.0))
        .count();
    assert_eq!(flipped, 2);
    for e in &s.edges {
        assert!(e.t1 > e.t0);
        if !matches!(e.curve, peet_kernel::Curve3::Line(_)) {
            assert!((e.t1 - e.t0 - PI).abs() < 1e-9, "half circles");
        }
    }
}

/// A cylinder whose wall is bounded by its two circles and nothing else. `turn` is the
/// angle of the top circle's vertex; the bottom one's is at 0.
fn seamless_cylinder(turn: f64, units: UnitStyle) -> String {
    let mut w = Step::default();
    let (r, h) = (5.0, 10.0);
    let top = DVec3::Z * h;
    let top_x = DQuat::from_rotation_z(turn) * DVec3::X;
    let vb = w.vertex(DVec3::X * r);
    let vt = w.vertex(top + top_x * r);
    let bottom_circle = w.circle(DVec3::ZERO, DVec3::Z, DVec3::X, r);
    let top_circle = w.circle(top, DVec3::Z, top_x, r);
    let eb = w.edge(vb, vb, bottom_circle, true);
    let et = w.edge(vt, vt, top_circle, true);
    let axis = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let cylinder = w.add(format!("CYLINDRICAL_SURFACE('',#{axis},5.)"));
    let wall = [
        w.bound("FACE_BOUND", &[(et, false)], true),
        w.bound("FACE_BOUND", &[(eb, true)], true),
    ];
    let wall = w.face(&wall, cylinder, true);
    let bound = w.bound("FACE_OUTER_BOUND", &[(eb, false)], true);
    let plane = w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X);
    let bottom = w.face(&[bound], plane, true);
    let bound = w.bound("FACE_OUTER_BOUND", &[(et, true)], true);
    let plane = w.plane(top, DVec3::Z, DVec3::X);
    let top = w.face(&[bound], plane, true);
    let brep = w.solid("Rod", &[wall, bottom, top]);
    w.part(&[brep], units)
}

#[test]
fn cylinder_between_two_circles_gets_a_seam() {
    // The circles' vertices line up: the seam joins them.
    let imported = import(&seamless_cylinder(0.0, UnitStyle::Millimetres));
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(s), PI * 250.0);
    assert_close(area(s), 2.0 * PI * 25.0 + TAU * 50.0);
    let wall = s
        .face_ids()
        .find(|&f| matches!(s.face(f).surface, Surface::Cylinder(_)))
        .unwrap();
    assert_eq!(s.face(wall).loops.len(), 1);
    assert_eq!(s.loop_coedges(s.face(wall).loops[0]).len(), 4);
    assert_close(measure::face_area(s, wall), TAU * 50.0);

    // They don't: the seam starts at one and splits the other circle, in the cap too.
    for turn in [FRAC_PI_2, 2.5, -1.0] {
        let imported = import(&seamless_cylinder(turn, UnitStyle::Inches));
        let s = &imported.bodies[0].solid;
        assert_eq!(tuple(valid(s)), (3, 4, 3, 0, 1, 0), "turn {turn}");
        assert_close(measure::volume(s), PI * 250.0 * 25.4f64.powi(3));
    }
}

#[test]
fn tube_between_circles_gets_seams_inside_and_out() {
    // A tube: two walls without seams, the bore against its cylinder's normal, and flat
    // rings whose two bounds are both plain FACE_BOUNDs with the hole first.
    let mut w = Step::default();
    let (ri, ro, h) = (3.0, 5.0, 10.0);
    let top = DVec3::Z * h;
    let circle = |w: &mut Step, at: DVec3, r: f64, turn: f64| {
        let x = DQuat::from_rotation_z(turn) * DVec3::X;
        let v = w.vertex(at + x * r);
        let c = w.circle(at, DVec3::Z, x, r);
        w.edge(v, v, c, true)
    };
    let (bo, to) = (
        circle(&mut w, DVec3::ZERO, ro, 0.3),
        circle(&mut w, top, ro, 0.3),
    );
    let (bi, ti) = (
        circle(&mut w, DVec3::ZERO, ri, -2.0),
        circle(&mut w, top, ri, 1.0),
    );
    let axis = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let outer = w.add(format!("CYLINDRICAL_SURFACE('',#{axis},5.0)"));
    let bore = w.add(format!("CYLINDRICAL_SURFACE('',#{axis},3.0)"));
    let bounds = [
        w.bound("FACE_BOUND", &[(bo, true)], true),
        w.bound("FACE_BOUND", &[(to, false)], true),
    ];
    let outer = w.face(&bounds, outer, true);
    let bounds = [
        w.bound("FACE_BOUND", &[(ti, true)], true),
        w.bound("FACE_BOUND", &[(bi, false)], true),
    ];
    let bore = w.face(&bounds, bore, false);
    let bounds = [
        w.bound("FACE_BOUND", &[(bi, true)], true),
        w.bound("FACE_BOUND", &[(bo, false)], true),
    ];
    let plane = w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X);
    let bottom = w.face(&bounds, plane, true);
    let bounds = [
        w.bound("FACE_BOUND", &[(ti, false)], true),
        w.bound("FACE_BOUND", &[(to, true)], true),
    ];
    let plane = w.plane(top, DVec3::Z, DVec3::X);
    let top = w.face(&bounds, plane, true);
    let brep = w.solid("Tube", &[outer, bore, bottom, top]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    let s = &imported.bodies[0].solid;
    // The outer circles' vertices line up; the bore's don't, so one is split.
    assert_eq!(tuple(valid(s)), (5, 7, 4, 2, 1, 1));
    assert_close(measure::volume(s), PI * 160.0);
    assert_close(area(s), 2.0 * PI * 16.0 + TAU * 80.0);
}

#[test]
fn whole_sphere_with_a_vertex_loop_or_no_bounds() {
    for vertex_loop in [true, false] {
        let mut w = Step::default();
        let centre = v3(3.0, -2.0, 7.0);
        // A frame that points nowhere in particular: the poles go where its Z puts them.
        let z = v3(1.0, 2.0, -2.0) / 3.0;
        let x = v3(2.0, 1.0, 2.0) / 3.0;
        let axis = w.axis(centre, z, x);
        let sphere = w.add(format!("SPHERICAL_SURFACE('',#{axis},4.0)"));
        let bounds = if vertex_loop {
            vec![w.vertex_loop(centre + DVec3::X * 4.0)]
        } else {
            Vec::new()
        };
        let face = w.face(&bounds, sphere, true);
        let brep = w.solid("Ball", &[face]);
        let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
        assert_eq!(imported.warnings, Vec::<String>::new());
        let s = &imported.bodies[0].solid;
        assert_eq!(tuple(valid(s)), (2, 1, 1, 0, 1, 0));
        assert_close(measure::volume(s), 4.0 / 3.0 * PI * 64.0);
        assert_close(area(s), 4.0 * PI * 16.0);
        let poles: Vec<DVec3> = s.vertices.iter().map(|v| v.point).collect();
        assert!(poles[0].abs_diff_eq(centre - z * 4.0, 1e-9), "{poles:?}");
        assert!(poles[1].abs_diff_eq(centre + z * 4.0, 1e-9), "{poles:?}");
    }
}

#[test]
fn cone_with_a_vertex_loop_at_its_apex_in_degrees() {
    for vertex_loop in [true, false] {
        let mut w = Step::default();
        // In centimetres: base radius 2, height 2, so the half angle is 45°.
        let apex = DVec3::Z * 2.0;
        let v = w.vertex(DVec3::X * 2.0);
        let circle = w.circle(DVec3::ZERO, DVec3::Z, DVec3::X, 2.0);
        let base = w.edge(v, v, circle, true);
        // STEP cones widen along their axis: this one's points down from the base.
        let axis = w.axis(DVec3::ZERO, -DVec3::Z, DVec3::X);
        let cone = w.add(format!("CONICAL_SURFACE('',#{axis},2.0,45.0)"));
        let mut bounds = vec![w.bound("FACE_BOUND", &[(base, true)], true)];
        if vertex_loop {
            bounds.insert(0, w.vertex_loop(apex));
        }
        let side = w.face(&bounds, cone, true);
        let bound = w.bound("FACE_BOUND", &[(base, false)], true);
        let plane = w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X);
        let bottom = w.face(&[bound], plane, true);
        let brep = w.solid("Cone", &[side, bottom]);
        let imported = import(&w.part(&[brep], UnitStyle::CentimetresDegrees));
        assert_eq!(imported.warnings, Vec::<String>::new());
        let s = &imported.bodies[0].solid;
        assert_eq!(tuple(valid(s)), (2, 2, 2, 0, 1, 0));
        // In millimetres: radius 20, height 20.
        assert_close(measure::volume(s), PI * 400.0 * 20.0 / 3.0);
        assert_close(area(s), PI * 400.0 + PI * 20.0 * 800f64.sqrt());
        assert!(
            s.vertices
                .iter()
                .any(|v| v.point.abs_diff_eq(v3(0.0, 0.0, 20.0), 1e-9))
        );
        let Some(Surface::Cone(c)) = s
            .faces
            .iter()
            .map(|f| f.surface.clone())
            .find(|s| matches!(s, Surface::Cone(_)))
        else {
            panic!("no cone");
        };
        assert!((c.half_angle - PI / 4.0).abs() < 1e-12, "{}", c.half_angle);
    }
}

#[test]
fn spherical_cap_closes_onto_its_pole() {
    // A dome: the part of a sphere above a plane, bounded by one circle. With the face
    // against the sphere's normal it is a bowl-shaped void's wall, so use the dome.
    let mut w = Step::default();
    let (r, cut) = (5.0, 3.0);
    let rim = f64::sqrt(r * r - cut * cut);
    let v = w.vertex(v3(rim, 0.0, cut));
    let circle = w.circle(DVec3::Z * cut, DVec3::Z, DVec3::X, rim);
    let edge = w.edge(v, v, circle, true);
    let axis = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let sphere = w.add(format!("SPHERICAL_SURFACE('',#{axis},5.0)"));
    let bound = w.bound("FACE_BOUND", &[(edge, true)], true);
    let dome = w.face(&[bound], sphere, true);
    let bound = w.bound("FACE_BOUND", &[(edge, false)], true);
    let plane = w.plane(DVec3::Z * cut, -DVec3::Z, DVec3::X);
    let flat = w.face(&[bound], plane, true);
    let brep = w.solid("Dome", &[dome, flat]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (2, 2, 2, 0, 1, 0));
    let height = r - cut;
    assert_close(
        measure::volume(s),
        PI * height * height * (3.0 * r - height) / 3.0,
    );
    assert_close(area(s), TAU * r * height + PI * rim * rim);
    assert!(
        s.vertices
            .iter()
            .any(|v| v.point.abs_diff_eq(v3(0.0, 0.0, 5.0), 1e-9))
    );
}

#[test]
fn sphere_in_two_halves_split_through_its_poles() {
    // Two hemispheres that meet in one circle through both poles, with its only vertex
    // on the equator: the circle is split at the poles.
    let mut w = Step::default();
    let r = 4.0;
    let v = w.vertex(DVec3::Y * r);
    let circle = w.circle(DVec3::ZERO, DVec3::X, DVec3::Y, r);
    let edge = w.edge(v, v, circle, true);
    let axis = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let sphere = w.add(format!("SPHERICAL_SURFACE('',#{axis},4.0)"));
    let bound = w.bound("FACE_BOUND", &[(edge, true)], true);
    let front = w.face(&[bound], sphere, true);
    let bound = w.bound("FACE_BOUND", &[(edge, false)], true);
    let back = w.face(&[bound], sphere, true);
    let brep = w.solid("Ball", &[front, back]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (3, 3, 2, 0, 1, 0));
    assert_close(measure::volume(s), 4.0 / 3.0 * PI * 64.0);
    for f in s.face_ids() {
        assert_close(measure::face_area(s, f), TAU * 16.0);
    }
    for pole in [DVec3::Z * r, -DVec3::Z * r] {
        assert!(s.vertices.iter().any(|v| v.point.abs_diff_eq(pole, 1e-9)));
    }
}

/// A cylinder the way OCCT (FreeCAD) writes it: a seam as a `SEAM_CURVE` with its pcurves,
/// circles as `SURFACE_CURVE`s, a plain `FACE_BOUND` on every face.
const OCCT_CYLINDER: &str = "ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('FreeCAD Model'),'2;1');
FILE_NAME('Open CASCADE Shape Model','2026-10-03T12:00:00',(''),(''),
  'Open CASCADE STEP processor 7.6','FreeCAD','Unknown');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));
ENDSEC;
DATA;
#1 = APPLICATION_PROTOCOL_DEFINITION('international standard',
  'automotive_design',2000,#2);
#2 = APPLICATION_CONTEXT(
  'core data for automotive mechanical design processes');
#3 = SHAPE_DEFINITION_REPRESENTATION(#4,#10);
#4 = PRODUCT_DEFINITION_SHAPE('','',#5);
#5 = PRODUCT_DEFINITION('design','',#6,#9);
#6 = PRODUCT_DEFINITION_FORMATION('','',#7);
#7 = PRODUCT('Cylinder','Cylinder','',(#8));
#8 = PRODUCT_CONTEXT('',#2,'mechanical');
#9 = PRODUCT_DEFINITION_CONTEXT('part definition',#2,'design');
#10 = ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#15),#90);
#11 = AXIS2_PLACEMENT_3D('',#12,#13,#14);
#12 = CARTESIAN_POINT('',(0.,0.,0.));
#13 = DIRECTION('',(0.,0.,1.));
#14 = DIRECTION('',(1.,0.,-0.));
#15 = MANIFOLD_SOLID_BREP('',#16);
#16 = CLOSED_SHELL('',(#17,#50,#60));
#17 = ADVANCED_FACE('',(#18),#40,.T.);
#18 = FACE_BOUND('',#19,.T.);
#19 = EDGE_LOOP('',(#20,#30,#34,#35));
#20 = ORIENTED_EDGE('',*,*,#21,.T.);
#21 = EDGE_CURVE('',#22,#24,#26,.T.);
#22 = VERTEX_POINT('',#23);
#23 = CARTESIAN_POINT('',(5.,-1.224646799147E-15,0.));
#24 = VERTEX_POINT('',#25);
#25 = CARTESIAN_POINT('',(5.,-1.224646799147E-15,10.));
#26 = SEAM_CURVE('',#27,(#70,#74),.PCURVE_S1.);
#27 = LINE('',#28,#29);
#28 = CARTESIAN_POINT('',(5.,-1.224646799147E-15,0.));
#29 = VECTOR('',#13,1.);
#30 = ORIENTED_EDGE('',*,*,#31,.F.);
#31 = EDGE_CURVE('',#24,#24,#32,.T.);
#32 = SURFACE_CURVE('',#33,(#78,#82),.PCURVE_S1.);
#33 = CIRCLE('',#36,5.);
#34 = ORIENTED_EDGE('',*,*,#21,.F.);
#35 = ORIENTED_EDGE('',*,*,#37,.T.);
#36 = AXIS2_PLACEMENT_3D('',#44,#13,#14);
#37 = EDGE_CURVE('',#22,#22,#38,.T.);
#38 = SURFACE_CURVE('',#39,(#78,#82),.PCURVE_S1.);
#39 = CIRCLE('',#11,5.);
#40 = CYLINDRICAL_SURFACE('',#41,5.);
#41 = AXIS2_PLACEMENT_3D('',#12,#13,$);
#44 = CARTESIAN_POINT('',(0.,0.,10.));
#50 = ADVANCED_FACE('',(#51),#54,.F.);
#51 = FACE_BOUND('',#52,.F.);
#52 = EDGE_LOOP('',(#53));
#53 = ORIENTED_EDGE('',*,*,#37,.T.);
#54 = PLANE('',#55);
#55 = AXIS2_PLACEMENT_3D('',#12,$,$);
#60 = ADVANCED_FACE('',(#61),#64,.T.);
#61 = FACE_BOUND('',#62,.T.);
#62 = EDGE_LOOP('',(#63));
#63 = ORIENTED_EDGE('',*,*,#31,.T.);
#64 = PLANE('',#36);
#70 = PCURVE('',#40,#71);
#71 = DEFINITIONAL_REPRESENTATION('',(#72),#77);
#72 = LINE('',#73,#75);
#73 = CARTESIAN_POINT('',(6.28318530718,-0.));
#74 = PCURVE('',#40,#71);
#75 = VECTOR('',#76,1.);
#76 = DIRECTION('',(0.,1.));
#77 = ( GEOMETRIC_REPRESENTATION_CONTEXT(2)
PARAMETRIC_REPRESENTATION_CONTEXT() REPRESENTATION_CONTEXT('2D SPACE',''
  ) );
#78 = PCURVE('',#40,#71);
#82 = PCURVE('',#54,#71);
#90 = ( GEOMETRIC_REPRESENTATION_CONTEXT(3)
GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#94)) GLOBAL_UNIT_ASSIGNED_CONTEXT(
(#91,#92,#93)) REPRESENTATION_CONTEXT('Context #1',
  '3D Context with UNIT and UNCERTAINTY') );
#91 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );
#92 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );
#93 = ( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );
#94 = UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#91,
  'distance_accuracy_value','confusion accuracy');
#95 = PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#7));
ENDSEC;
END-ISO-10303-21;
";

#[test]
fn occt_style_cylinder() {
    let imported = import(OCCT_CYLINDER);
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.product_name, "Cylinder");
    assert_eq!(imported.bodies.len(), 1);
    // The brep has no name of its own: the product's.
    assert_eq!(imported.bodies[0].name, "Cylinder");
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(s), PI * 250.0);
    assert_close(area(s), 2.0 * PI * 25.0 + TAU * 50.0);
    // The bottom is a plane along +Z used against its normal.
    let bottom = s
        .faces
        .iter()
        .find(|f| f.reversed)
        .expect("a reversed face");
    assert!(matches!(bottom.surface, Surface::Plane(p) if p.normal().abs_diff_eq(DVec3::Z, 1e-12)));
}

#[test]
fn torus_faces_without_seams() {
    // A whole ring: one face with no bounds at all.
    let mut w = Step::default();
    let axis = w.axis(v3(1.0, 2.0, 3.0), DVec3::X, DVec3::Y);
    let torus = w.add(format!("TOROIDAL_SURFACE('',#{axis},10.0,3.0)"));
    let face = w.face(&[], torus, true);
    let brep = w.solid("Ring", &[face]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (1, 2, 1, 0, 1, 1));
    assert_close(measure::volume(s), 2.0 * PI * PI * 90.0);
    assert_close(area(s), 4.0 * PI * PI * 30.0);

    // The outer half of a ring, closed by a cylinder through the tube's middle: both
    // faces are bands between the same two circles.
    let mut w = Step::default();
    let (major, minor) = (10.0, 3.0);
    let circle = |w: &mut Step, z: f64, turn: f64| {
        let x = DQuat::from_rotation_z(turn) * DVec3::X;
        let v = w.vertex(v3(0.0, 0.0, z) + x * major);
        let c = w.circle(v3(0.0, 0.0, z), DVec3::Z, x, major);
        w.edge(v, v, c, true)
    };
    let (low, high) = (circle(&mut w, -minor, 0.0), circle(&mut w, minor, 2.0));
    let axis = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let torus = w.add(format!("TOROIDAL_SURFACE('',#{axis},10.0,3.0)"));
    let cylinder = w.add(format!("CYLINDRICAL_SURFACE('',#{axis},10.0)"));
    let bounds = [
        w.bound("FACE_BOUND", &[(low, true)], true),
        w.bound("FACE_BOUND", &[(high, false)], true),
    ];
    let outside = w.face(&bounds, torus, true);
    let bounds = [
        w.bound("FACE_BOUND", &[(high, true)], true),
        w.bound("FACE_BOUND", &[(low, false)], true),
    ];
    let inside = w.face(&bounds, cylinder, false);
    let brep = w.solid("Tyre", &[outside, inside]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    let s = &imported.bodies[0].solid;
    // The torus's seam splits the upper circle; the cylinder's seam then joins the two
    // vertices that are in line. The body is a ring with a D-shaped section.
    assert_eq!(tuple(valid(s)), (3, 5, 2, 0, 1, 1));
    // Pappus: a half disc of radius 3 whose centroid is 4r/3π outside the tube's middle.
    let half_disc = PI * minor * minor / 2.0;
    assert_close(
        measure::volume(s),
        TAU * (major + 4.0 * minor / (3.0 * PI)) * half_disc,
    );
    // The torus face is the outer half: it passes through the outer equator.
    let outer = s
        .face_ids()
        .find(|&f| matches!(s.face(f).surface, Surface::Torus(_)))
        .unwrap();
    assert_close(
        measure::face_area(s, outer),
        TAU * (PI * minor * major + 2.0 * minor * minor),
    );
}

#[test]
fn brep_with_voids_either_way_round() {
    // OCCT writes a void as an outward-facing shell used with `.F.`; others write the
    // shell already inside out. Both give a void with a negative volume.
    for (inward, flag) in [(false, ".F."), (true, ".T."), (false, ".T."), (true, ".F.")] {
        let mut w = Step::default();
        let outer = box_faces(
            &mut w,
            DVec3::ZERO,
            DVec3::splat(10.0),
            false,
            false,
            NO_HOLES,
        );
        let outer = w.shell(&outer);
        let inner = box_faces(
            &mut w,
            DVec3::splat(3.0),
            DVec3::splat(7.0),
            true,
            inward,
            NO_HOLES,
        );
        let inner = w.shell(&inner);
        let void = w.add(format!("ORIENTED_CLOSED_SHELL('',*,#{inner},{flag})"));
        let brep = w.add(format!("BREP_WITH_VOIDS('Hollow',#{outer},(#{void}))"));
        let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
        assert_eq!(imported.warnings, Vec::<String>::new());
        assert_eq!(imported.bodies.len(), 1);
        let s = &imported.bodies[0].solid;
        assert_eq!(tuple(valid(s)), (16, 24, 12, 0, 2, 0));
        assert_close(measure::volume(s), 1000.0 - 64.0);
        assert_close(measure::shell_volume(s, peet_kernel::ShellId(1)), -64.0);
    }
}

#[test]
fn duplicate_vertices_are_merged_with_a_warning() {
    // Each face of a box gets its own copy of one corner.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Block", &faces);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    // Vertex #2 is the corner at the origin; give the first edge that uses it a twin.
    let first = text
        .find("EDGE_CURVE('',#2,")
        .expect("an edge from the first vertex");
    let mut text = text.replacen("EDGE_CURVE('',#2,", "EDGE_CURVE('',#9002,", 1);
    assert!(first > 0);
    text = text.replace(
        "ENDSEC;\nEND-ISO",
        "#9001 = CARTESIAN_POINT('',(0.0,0.0,1.E-08));\n#9002 = VERTEX_POINT('',#9001);\n\
         ENDSEC;\nEND-ISO",
    );
    let imported = import(&text);
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (8, 12, 6, 0, 1, 0));
    assert_eq!(imported.warnings.len(), 1);
    assert!(
        imported.warnings[0].contains("merged"),
        "{:?}",
        imported.warnings
    );
}

#[test]
fn syntax_oddities_are_read() {
    // Lower-case keywords, odd spacing and line breaks, comments between tokens, reals
    // without digits after the point, integers for reals, typed measures.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("It''s a \\X2\\00E400F6\\X0\\ \\X\\FC box", &faces);
    let text = w
        .part(&[brep], UnitStyle::Millimetres)
        .replace(
            "CARTESIAN_POINT ( 'NONE', ( 0.0, 0.0, 0.0 ) )",
            "cartesian_point('',(0,0.,0.E+00))",
        )
        .replace("3.0 ) )", "\n3.,\n) /* three */ )")
        .replace("3.0 ) )", "3.0))")
        .replace("ADVANCED_FACE", "\r\n  ADVANCED_FACE")
        .replace("EDGE_LOOP('',", "EDGE_LOOP ( 'a ; in a name (' , ");
    assert!(text.contains("cartesian_point"));
    // `3.,\n)` is not valid Part 21 (a trailing comma): that one must be refused.
    assert!(read(&text).is_err());
    let text = text.replace("\n3.,\n)", "\n3.\n)");
    let imported = import(&text);
    assert_eq!(imported.bodies[0].name, "It's a äö ü box");
    assert_close(measure::volume(&imported.bodies[0].solid), 6.0);
}

#[test]
fn assembly_places_each_occurrence() {
    // A 1 × 2 × 3 inch block used twice in an assembly in millimetres: moved 100 along X,
    // and turned a quarter turn about Z and moved 50 along Y.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("", &faces);
    let inch = w.context(UnitStyle::Inches);
    let part = w.representation(&[brep], inch);
    let part_definition = w.product("Block", part);
    let part_origin = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);

    let mm = w.context(UnitStyle::Millimetres);
    let origin = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let first = w.axis(v3(100.0, 0.0, 0.0), DVec3::Z, DVec3::X);
    let second = w.axis(v3(0.0, 50.0, 0.0), DVec3::Z, DVec3::Y);
    let assembly = w.add(format!(
        "SHAPE_REPRESENTATION('',(#{origin},#{first},#{second}),#{mm})"
    ));
    let assembly_definition = w.product("Machine", assembly);
    for (k, target) in [first, second].into_iter().enumerate() {
        let transformation = w.add(format!(
            "ITEM_DEFINED_TRANSFORMATION('','',#{part_origin},#{target})"
        ));
        // One the usual way round, one with the assembly first (as some writers do),
        // which the assembly usage sorts out.
        let relationship = if k == 0 {
            w.add(format!(
                "( REPRESENTATION_RELATIONSHIP('','',#{part},#{assembly}) \
                 REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{transformation}) \
                 SHAPE_REPRESENTATION_RELATIONSHIP() )"
            ))
        } else {
            let swapped = w.add(format!(
                "ITEM_DEFINED_TRANSFORMATION('','',#{target},#{part_origin})"
            ));
            w.add(format!(
                "( REPRESENTATION_RELATIONSHIP('','',#{assembly},#{part}) \
                 REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{swapped}) \
                 SHAPE_REPRESENTATION_RELATIONSHIP() )"
            ))
        };
        let usage = w.add(format!(
            "NEXT_ASSEMBLY_USAGE_OCCURRENCE('{k}','','',#{assembly_definition},\
             #{part_definition},$)"
        ));
        let shape = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{usage})"));
        w.add(format!(
            "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{relationship},#{shape})"
        ));
    }
    let imported = import(&w.file());
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.product_name, "Block");
    assert_eq!(imported.bodies.len(), 2);
    for body in &imported.bodies {
        assert_eq!(body.name, "Block");
        valid(&body.solid);
        assert_close(measure::volume(&body.solid), 6.0 * 25.4f64.powi(3));
    }
    let b = imported.bodies[0].solid.bounds();
    assert!(b.min.abs_diff_eq(v3(100.0, 0.0, 0.0), 1e-9), "{b:?}");
    assert!(b.max.abs_diff_eq(v3(125.4, 50.8, 76.2), 1e-9), "{b:?}");
    // A quarter turn about Z: the block's X runs along Y and its Y along −X.
    let b = imported.bodies[1].solid.bounds();
    assert!(b.min.abs_diff_eq(v3(-50.8, 50.0, 0.0), 1e-9), "{b:?}");
    assert!(b.max.abs_diff_eq(v3(0.0, 75.4, 76.2), 1e-9), "{b:?}");
}

#[test]
fn mapped_item_places_a_part() {
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Block", &faces);
    let mm = w.context(UnitStyle::Millimetres);
    let part = w.representation(&[brep], mm);
    let origin = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let map = w.add(format!("REPRESENTATION_MAP(#{origin},#{part})"));
    let target = w.axis(v3(0.0, 0.0, 10.0), DVec3::Z, DVec3::X);
    let mapped = w.add(format!("MAPPED_ITEM('',#{map},#{target})"));
    let top = w.add(format!("SHAPE_REPRESENTATION('',(#{mapped}),#{mm})"));
    w.product("Top", top);
    let imported = import(&w.file());
    assert_eq!(imported.bodies.len(), 1);
    let b = imported.bodies[0].solid.bounds();
    assert!(b.min.abs_diff_eq(v3(0.0, 0.0, 10.0), 1e-9), "{b:?}");
}

#[test]
fn brep_in_a_related_representation_takes_the_product_name_and_units() {
    // The product's shape is a plain SHAPE_REPRESENTATION; the brep sits in an
    // ADVANCED_BREP_SHAPE_REPRESENTATION tied to it by a relationship (as several
    // writers do), through a chain of three.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("", &faces);
    let inch = w.context(UnitStyle::Inches);
    let breps = w.representation(&[brep], inch);
    let middle = w.representation(&[], inch);
    let shape = w.representation(&[], inch);
    w.add(format!(
        "SHAPE_REPRESENTATION_RELATIONSHIP('','',#{middle},#{breps})"
    ));
    w.add(format!(
        "SHAPE_REPRESENTATION_RELATIONSHIP('','',#{shape},#{middle})"
    ));
    w.product("Gadget", shape);
    let imported = import(&w.file());
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.bodies.len(), 1);
    assert_eq!(imported.bodies[0].name, "Gadget");
    assert_close(
        measure::volume(&imported.bodies[0].solid),
        6.0 * 25.4f64.powi(3),
    );
}

// ---- Freeform files written by hand ----

/// `( a, b, c )`.
fn real_list(values: &[f64]) -> String {
    let list: Vec<String> = values.iter().map(|v| step::real(*v)).collect();
    format!("( {} )", list.join(", "))
}

/// A knot vector as STEP writes it: how often each distinct knot is repeated, and the
/// distinct knots.
fn knot_lists(knots: &[f64]) -> (String, String) {
    let mut distinct: Vec<f64> = Vec::new();
    let mut repeats: Vec<usize> = Vec::new();
    for &k in knots {
        if distinct.last() == Some(&k) {
            *repeats.last_mut().unwrap() += 1;
        } else {
            distinct.push(k);
            repeats.push(1);
        }
    }
    let repeats: Vec<String> = repeats.iter().map(usize::to_string).collect();
    (format!("( {} )", repeats.join(", ")), real_list(&distinct))
}

impl Step {
    fn points(&mut self, points: &[DVec3], scale: f64) -> String {
        let ids: Vec<u32> = points.iter().map(|p| self.point(*p * scale)).collect();
        refs(&ids)
    }

    /// A B-spline curve with its knots written out; a rational one as a complex instance
    /// (with its parts in an order of its own, and "unknown" for the flags).
    fn spline_curve(&mut self, c: &NurbsCurve, scale: f64) -> u32 {
        let points = self.points(c.control_points(), scale);
        let (repeats, knots) = knot_lists(c.knots());
        let degree = c.degree();
        match c.weights() {
            None => self.add(format!(
                "B_SPLINE_CURVE_WITH_KNOTS ( 'NONE', {degree}, {points}, .UNSPECIFIED., .F., \
                 .F., {repeats}, {knots}, .UNSPECIFIED. )"
            )),
            Some(w) => self.add(format!(
                "( BOUNDED_CURVE ( ) B_SPLINE_CURVE ( {degree}, {points}, .CIRCULAR_ARC., .U., \
                 .U. ) CURVE ( ) GEOMETRIC_REPRESENTATION_ITEM ( ) RATIONAL_B_SPLINE_CURVE ( {} \
                 ) REPRESENTATION_ITEM ( 'arc' ) B_SPLINE_CURVE_WITH_KNOTS ( {repeats}, {knots}, \
                 .PIECEWISE_BEZIER_KNOTS. ) )",
                real_list(w)
            )),
        }
    }

    /// The control points of a surface as STEP lists them: a row along `v` for each step
    /// in `u`.
    fn net(&mut self, s: &NurbsSurface, scale: f64) -> String {
        let (_, count_v) = s.counts();
        let rows: Vec<String> = s
            .control_points()
            .chunks(count_v)
            .map(|row| self.points(row, scale))
            .collect();
        format!("( {} )", rows.join(", "))
    }

    fn spline_surface(&mut self, s: &NurbsSurface, scale: f64) -> u32 {
        let net = self.net(s, scale);
        let (degree_u, degree_v) = s.degrees();
        let (knots_u, knots_v) = s.knots();
        let (repeats_u, knots_u) = knot_lists(knots_u);
        let (repeats_v, knots_v) = knot_lists(knots_v);
        let knots = format!("{repeats_u}, {repeats_v}, {knots_u}, {knots_v}, .UNSPECIFIED.");
        match s.weights() {
            None => self.add(format!(
                "B_SPLINE_SURFACE_WITH_KNOTS ( 'NONE', {degree_u}, {degree_v}, {net}, \
                 .UNSPECIFIED., .F., .F., .F., {knots} )"
            )),
            Some(w) => {
                let (_, count_v) = s.counts();
                let rows: Vec<String> = w.chunks(count_v).map(real_list).collect();
                self.add(format!(
                    "( B_SPLINE_SURFACE ( {degree_u}, {degree_v}, {net}, .UNSPECIFIED., .U., \
                     .U., .U. ) B_SPLINE_SURFACE_WITH_KNOTS ( {knots} ) BOUNDED_SURFACE ( ) \
                     GEOMETRIC_REPRESENTATION_ITEM ( ) RATIONAL_B_SPLINE_SURFACE ( ( {} ) ) \
                     REPRESENTATION_ITEM ( '' ) SURFACE ( ) )",
                    rows.join(", ")
                ))
            }
        }
    }
}

type CurveWriter<'a> = &'a dyn Fn(&mut Step, &Curve3) -> Option<u32>;
type SurfaceWriter<'a> = &'a dyn Fn(&mut Step, &Surface) -> Option<u32>;

/// How a kernel solid is written out by hand.
#[derive(Clone, Copy)]
struct Foreign<'a> {
    /// File units per millimetre.
    scale: f64,
    /// Every other edge is written from its far end, with `same_sense = .F.`.
    backwards: bool,
    /// Writes the curves it wants to write its own way.
    curve: CurveWriter<'a>,
    surface: SurfaceWriter<'a>,
}

const PLAIN: Foreign = Foreign {
    scale: 1.0,
    backwards: false,
    curve: &|_, _| None,
    surface: &|_, _| None,
};

/// Writes a solid of planes and freeform faces with the test's own writer: the entities
/// spaced and named the way other exporters do, no bound marked as the outer one and the
/// outer one listed last.
fn foreign_brep(w: &mut Step, name: &str, solid: &Solid, style: &Foreign) -> u32 {
    let scale = style.scale;
    let vertices: Vec<u32> = solid
        .vertices
        .iter()
        .map(|v| w.vertex(v.point * scale))
        .collect();
    let edges: Vec<(u32, bool)> = solid
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let curve = (style.curve)(w, &e.curve).unwrap_or_else(|| match &e.curve {
                Curve3::Line(l) => w.line(l.origin * scale, (l.origin + l.dir) * scale),
                Curve3::Circle(c) => w.circle(
                    c.frame.origin * scale,
                    c.frame.z_axis(),
                    c.frame.x_axis(),
                    c.radius * scale,
                ),
                Curve3::Nurbs(c) => w.spline_curve(c, scale),
                Curve3::Ellipse(_) => unimplemented!("no test writes an ellipse this way"),
            });
            let backwards = style.backwards && i % 2 == 1;
            let (a, b) = (vertices[e.start.index()], vertices[e.end.index()]);
            if backwards {
                (w.edge(b, a, curve, false), true)
            } else {
                (w.edge(a, b, curve, true), false)
            }
        })
        .collect();
    let mut faces = Vec::new();
    for f in solid.face_ids() {
        let face = solid.face(f);
        let surface = (style.surface)(w, &face.surface).unwrap_or_else(|| match &face.surface {
            Surface::Plane(p) => w.plane(p.origin() * scale, p.normal(), p.frame.x_axis()),
            Surface::Nurbs(s) => w.spline_surface(s, scale),
            Surface::Cylinder(c) => {
                let axis = w.axis(c.frame.origin * scale, c.frame.z_axis(), c.frame.x_axis());
                w.add(format!(
                    "CYLINDRICAL_SURFACE('',#{axis},{})",
                    step::real(c.radius * scale)
                ))
            }
            _ => unimplemented!("no test writes this surface this way"),
        });
        let bounds: Vec<u32> = face
            .loops
            .iter()
            .map(|&l| {
                let uses: Vec<EdgeUse> = solid
                    .loop_coedges(l)
                    .into_iter()
                    .map(|c| {
                        let c = solid.coedge(c);
                        let (edge, backwards) = edges[c.edge.index()];
                        (edge, c.reversed == backwards)
                    })
                    .collect();
                w.bound("FACE_BOUND", &uses, true)
            })
            .rev()
            .collect();
        faces.push(w.face(&bounds, surface, !face.reversed));
    }
    w.solid(name, &faces)
}

/// A prism on a "D": the curve `profile` (in the plane z = 0, bulging to the right of
/// the way it runs) closed by its chord, `height` tall. One freeform side, one flat one,
/// and two caps with a freeform edge each.
fn d_prism(profile: &NurbsCurve, height: f64) -> Solid {
    let up = DVec3::Z * height;
    let raised = profile.mapped(|p| p + up);
    let (lo, hi) = profile.domain();
    let (a, b) = (profile.point(lo), profile.point(hi));
    let corners = [a, b, a + up, b + up];
    let mut s = Solid::new();
    let shell = s.add_shell();
    let v = corners.map(|p| s.add_vertex(p));
    let line = |s: &mut Solid, from: usize, to: usize| {
        let (p, q) = (corners[from], corners[to]);
        let curve = Curve3::line_through(p, q).unwrap();
        s.add_edge(curve, v[from], v[to], 0.0, p.distance(q))
    };
    let spline = |c: &NurbsCurve| Curve3::Nurbs(Arc::new(c.clone()));
    let bottom_curve = s.add_edge(spline(profile), v[0], v[1], lo, hi);
    let top_curve = s.add_edge(spline(&raised), v[2], v[3], lo, hi);
    let bottom_chord = line(&mut s, 1, 0);
    let top_chord = line(&mut s, 3, 2);
    let up_a = line(&mut s, 0, 2);
    let up_b = line(&mut s, 1, 3);
    let along = (b - a).normalize();
    let plane = |origin: DVec3, normal: DVec3| {
        Surface::Plane(Plane::from_origin_normal_x(origin, normal, along).unwrap())
    };
    let side = NurbsSurface::skin(&[profile.clone(), raised]).unwrap();
    let f = s.add_face(shell, plane(a, -DVec3::Z), false);
    s.add_loop(f, &[(bottom_chord, true), (bottom_curve, true)]);
    let f = s.add_face(shell, plane(a + up, DVec3::Z), false);
    s.add_loop(f, &[(top_curve, false), (top_chord, false)]);
    let f = s.add_face(shell, plane(a, DVec3::Z.cross(along)), false);
    s.add_loop(
        f,
        &[
            (bottom_chord, false),
            (up_a, false),
            (top_chord, true),
            (up_b, true),
        ],
    );
    let f = s.add_face(shell, Surface::Nurbs(Arc::new(side)), false);
    s.add_loop(
        f,
        &[
            (bottom_curve, false),
            (up_b, false),
            (top_curve, true),
            (up_a, true),
        ],
    );
    valid(&s);
    s
}

fn flat(points: &[(f64, f64)]) -> Vec<DVec3> {
    points.iter().map(|&(x, y)| v3(x, y, 0.0)).collect()
}

/// A cubic with two knots inside, from the origin to (10, 0), bulging towards −Y.
fn wavy_profile() -> NurbsCurve {
    NurbsCurve::new(
        3,
        vec![0.0, 0.0, 0.0, 0.0, 0.4, 0.7, 1.0, 1.0, 1.0, 1.0],
        flat(&[
            (0.0, 0.0),
            (2.0, -4.0),
            (4.0, -5.0),
            (6.0, -2.0),
            (8.0, -5.0),
            (10.0, 0.0),
        ]),
        None,
    )
    .unwrap()
}

/// Half a disc of radius 5 as a prism: its profile is an exact (rational) half circle.
fn half_disc() -> Solid {
    let centre = Frame {
        origin: v3(5.0, 0.0, 0.0),
        ..Frame::WORLD
    };
    d_prism(&NurbsCurve::arc(&centre, 5.0, PI, PI), 4.0)
}

/// Imports a one-body file and checks the body against the solid it was written from:
/// valid, of the same size, with the same numbers of everything.
#[track_caller]
fn same_body(text: &str, expected: &Solid) -> Solid {
    let imported = import(text);
    assert_eq!(imported.warnings, Vec::<String>::new());
    assert_eq!(imported.bodies.len(), 1);
    let s = imported.bodies[0].solid.clone();
    assert_eq!(tuple(valid(&s)), tuple(valid(expected)));
    for (got, expected) in [
        (measure::volume(&s), measure::volume(expected)),
        (area(&s), area(expected)),
    ] {
        assert!(
            (got - expected).abs() <= 1e-7 * expected.abs().max(1.0),
            "{got} instead of {expected}"
        );
    }
    s
}

#[test]
fn spline_face_with_spline_edges() {
    let solid = d_prism(&wavy_profile(), 6.0);
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "D", &solid, &PLAIN);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert!(text.contains("B_SPLINE_SURFACE_WITH_KNOTS ( 'NONE', 3, 1,"));
    assert!(text.contains("( 4, 1, 1, 4 ), ( 0.0, 0.4, 0.7, 1.0 )"));
    let s = same_body(&text, &solid);
    assert_eq!((freeform_faces(&s), freeform_edges(&s)), (1, 2));
    // The surface and the curves are the file's, as they are.
    assert_eq!(s.faces[3].surface, solid.faces[3].surface);
    assert!(s.edges.iter().any(|e| e.curve == solid.edges[0].curve));

    // Every other edge from its far end, with same_sense = .F., in inches: the curves
    // are turned round and scaled.
    let backwards = Foreign {
        scale: 1.0 / 25.4,
        backwards: true,
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "D", &solid, &backwards);
    let text = w.part(&[brep], UnitStyle::Inches);
    let s = same_body(&text, &solid);
    assert_eq!(freeform_edges(&s), 2);
    for e in &s.edges {
        let (a, b) = (s.vertex(e.start).point, s.vertex(e.end).point);
        assert!(e.t1 > e.t0);
        assert!(e.curve.point(e.t0).distance(a) < 1e-9);
        assert!(e.curve.point(e.t1).distance(b) < 1e-9);
    }
    // The top curve (edge 1 of the solid) was written backwards: it comes back running
    // from (10, 0, 6) to (0, 0, 6).
    assert!(s.edges.iter().any(|e| {
        e.curve.domain().is_some()
            && e.curve.point(e.t0).abs_diff_eq(v3(10.0, 0.0, 6.0), 1e-9)
            && e.curve.point(e.t1).abs_diff_eq(v3(0.0, 0.0, 6.0), 1e-9)
    }));
    let far = s.vertices.iter().fold(DVec3::ZERO, |m, v| m.max(v.point));
    assert!(far.abs_diff_eq(v3(10.0, 0.0, 6.0), 1e-9), "{far}");
}

#[test]
fn spline_face_with_a_hole() {
    // A plate with a hole whose flat faces are written as B-spline patches (flat ones,
    // bigger than the plate, their control points unevenly spaced): faces with two
    // bounds, neither marked as the outer one, the hole listed first.
    let mut sketch = Sketch::new();
    shapes::rectangle(&mut sketch, DVec2::ZERO, v2(40.0, 20.0));
    sketch.add_circle(v2(10.0, 10.0), 3.0);
    let plate: Vec<Region> = regions(&sketch)
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    let solid = extrude(&Plane::TOP, &plate, 0.0, 2.0).unwrap();
    let patches = Foreign {
        surface: &|w, s| match s {
            Surface::Plane(p) if p.normal().z.abs() > 0.5 => {
                let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
                let mut net = Vec::new();
                for x in [-60.0, -10.0, 70.0] {
                    for y in [-50.0, 5.0, 60.0] {
                        net.push(p.from_plane_coords(v2(x, y)));
                    }
                }
                let patch = NurbsSurface::new(2, 2, knots.clone(), knots, net, None).unwrap();
                Some(w.spline_surface(&patch, 1.0))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Plate", &solid, &patches);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert_eq!(text.matches("B_SPLINE_SURFACE_WITH_KNOTS").count(), 2);
    let s = same_body(&text, &solid);
    assert_eq!(freeform_faces(&s), 2);
    for face in s
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
    {
        // The outer loop (four lines) comes first, the hole (a circle) second.
        let sizes: Vec<usize> = face
            .loops
            .iter()
            .map(|&l| s.loop_coedges(l).len())
            .collect();
        assert_eq!(sizes, [4, 1]);
    }
    assert_close(measure::volume(&s), (800.0 - PI * 9.0) * 2.0);
}

#[test]
fn rational_splines_as_complex_instances() {
    let solid = half_disc();
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Half", &solid, &PLAIN);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert!(text.contains("RATIONAL_B_SPLINE_CURVE"));
    assert!(text.contains("RATIONAL_B_SPLINE_SURFACE"));
    let s = same_body(&text, &solid);
    let volume = PI * 25.0 / 2.0 * 4.0;
    let got = measure::volume(&s);
    assert!(
        (got - volume).abs() < 1e-7 * volume,
        "{got} instead of {volume}"
    );
    assert!(s.faces.iter().any(|f| match &f.surface {
        Surface::Nurbs(n) => n.weights().is_some(),
        _ => false,
    }));
    // In centimetres, edges backwards.
    let mut w = Step::default();
    let style = Foreign {
        scale: 0.1,
        backwards: true,
        ..PLAIN
    };
    let brep = foreign_brep(&mut w, "Half", &solid, &style);
    same_body(&w.part(&[brep], UnitStyle::CentimetresDegrees), &solid);
}

#[test]
fn bezier_and_quasi_uniform_splines() {
    // One cubic Bézier piece: no knots are written at all.
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let points = flat(&[(0.0, 0.0), (2.0, -6.0), (8.0, -6.0), (10.0, 0.0)]);
    let solid = d_prism(&NurbsCurve::new(3, knots, points, None).unwrap(), 5.0);
    let bezier = Foreign {
        curve: &|w, c| match c {
            Curve3::Nurbs(c) => {
                let points = w.points(c.control_points(), 1.0);
                Some(w.add(format!(
                    "BEZIER_CURVE('',{},{points},.UNSPECIFIED.,.F.,.F.)",
                    c.degree()
                )))
            }
            _ => None,
        },
        surface: &|w, s| match s {
            Surface::Nurbs(s) => {
                let net = w.net(s, 1.0);
                Some(w.add(format!(
                    "BEZIER_SURFACE('',3,1,{net},.UNSPECIFIED.,.F.,.F.,.F.)"
                )))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Bezier", &solid, &bezier);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert!(text.contains("= BEZIER_SURFACE('',3,1,"));
    assert!(!text.contains("WITH_KNOTS"));
    let s = same_body(&text, &solid);
    assert_eq!(s.faces[3].surface, solid.faces[3].surface);

    // Two Bézier pieces end to end, and the rational kind as a complex instance.
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 2.0];
    let points = flat(&[
        (0.0, 0.0),
        (1.0, -4.0),
        (5.0, -5.0),
        (9.0, -6.0),
        (10.0, 0.0),
    ]);
    let weights = vec![1.0, 0.8, 1.0, 1.3, 1.0];
    let solid = d_prism(
        &NurbsCurve::new(2, knots, points, Some(weights)).unwrap(),
        5.0,
    );
    let pieces = Foreign {
        curve: &|w, c| match c {
            Curve3::Nurbs(c) => {
                let points = w.points(c.control_points(), 1.0);
                Some(w.add(format!(
                    "(BEZIER_CURVE()B_SPLINE_CURVE(2,{points},.UNSPECIFIED.,.F.,.F.)\
                     BOUNDED_CURVE()CURVE()GEOMETRIC_REPRESENTATION_ITEM()\
                     RATIONAL_B_SPLINE_CURVE({})REPRESENTATION_ITEM(''))",
                    real_list(c.weights().unwrap())
                )))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Pieces", &solid, &pieces);
    same_body(&w.part(&[brep], UnitStyle::Millimetres), &solid);

    // Quasi-uniform: clamped, with the whole numbers between as knots.
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 3.0, 3.0, 3.0];
    let points = wavy_profile().control_points().to_vec();
    let solid = d_prism(&NurbsCurve::new(3, knots, points, None).unwrap(), 5.0);
    let quasi = Foreign {
        curve: &|w, c| match c {
            Curve3::Nurbs(c) => {
                let points = w.points(c.control_points(), 1.0);
                Some(w.add(format!(
                    "QUASI_UNIFORM_CURVE('',3,{points},.UNSPECIFIED.,.F.,.F.)"
                )))
            }
            _ => None,
        },
        surface: &|w, s| match s {
            Surface::Nurbs(s) => {
                let net = w.net(s, 1.0);
                Some(w.add(format!(
                    "QUASI_UNIFORM_SURFACE('',3,1,{net},.UNSPECIFIED.,.F.,.F.,.F.)"
                )))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Quasi", &solid, &quasi);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert!(text.contains("QUASI_UNIFORM_SURFACE") && text.contains("QUASI_UNIFORM_CURVE"));
    same_body(&text, &solid);
}

#[test]
fn unclamped_knot_vectors() {
    // A uniform cubic: no knot repeated, so the curve starts and ends away from its
    // control points. The kernel's curve is the clamped one that is the same shape.
    let net = flat(&[
        (-2.0, 3.0),
        (0.0, -1.5),
        (2.0, -4.0),
        (5.0, -6.0),
        (8.0, -4.0),
        (10.0, -1.5),
        (12.0, 3.0),
    ]);
    let uniform: Vec<f64> = (0..11).map(|k| f64::from(k) - 3.0).collect();
    let profile = NurbsCurve::from_unclamped(3, uniform.clone(), net.clone(), None).unwrap();
    assert_eq!(profile.domain(), (0.0, 4.0));
    let height = 5.0;
    let solid = d_prism(&profile, height);
    // The file has the unclamped definitions: the curves with their knots written out
    // (each once), or as a UNIFORM_CURVE; the surface as a UNIFORM_SURFACE.
    let raised = |z: f64| -> Vec<DVec3> { net.iter().map(|p| *p + DVec3::Z * z).collect() };
    let (repeats, values) = knot_lists(&uniform);
    let style = Foreign {
        curve: &|w, c| match c {
            Curve3::Nurbs(c) if c.control_points()[0].z == 0.0 => {
                let points = w.points(&raised(0.0), 1.0);
                Some(w.add(format!(
                    "B_SPLINE_CURVE_WITH_KNOTS('',3,{points},.UNSPECIFIED.,.F.,.F.,{repeats},\
                     {values},.UNIFORM_KNOTS.)"
                )))
            }
            Curve3::Nurbs(_) => {
                let points = w.points(&raised(height), 1.0);
                Some(w.add(format!(
                    "UNIFORM_CURVE('',3,{points},.UNSPECIFIED.,.F.,.F.)"
                )))
            }
            _ => None,
        },
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let rows: Vec<String> = net
                    .iter()
                    .map(|p| w.points(&[*p, *p + DVec3::Z * height], 1.0))
                    .collect();
                // In v, the two rows of a uniform surface of degree 1 are its ends.
                Some(w.add(format!(
                    "UNIFORM_SURFACE('',3,1,({}),.UNSPECIFIED.,.F.,.F.,.F.)",
                    rows.join(",")
                )))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Uniform", &solid, &style);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert!(
        text.contains("( 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1 )"),
        "{text}"
    );
    let s = same_body(&text, &solid);
    // Clamped on the way in.
    for e in s.edges.iter().filter(|e| e.curve.domain().is_some()) {
        let Curve3::Nurbs(c) = &e.curve else {
            unreachable!()
        };
        assert_eq!(c.knots()[..4], [0.0; 4]);
        assert_eq!((e.t0, e.t1), (0.0, 4.0));
    }
}

/// The area a closed curve in a plane of constant z encloses.
fn enclosed_area(c: &NurbsCurve) -> f64 {
    let (lo, hi) = c.domain();
    let n = 20_000;
    let at = |k: usize| c.point(lo + (hi - lo) * k as f64 / n as f64);
    (0..n)
        .map(|k| {
            let (p, q) = (at(k), at(k + 1));
            0.5 * (p.x * q.y - q.x * p.y)
        })
        .sum()
}

/// How the side of [`spline_tube`] is written.
#[derive(Clone, Copy, PartialEq)]
enum Tube {
    /// One face all the way round, with a seam edge used twice.
    Seam,
    /// One face between its two closed edges, with no seam.
    Seamless,
    /// Two faces, each half the way round.
    Halves,
}

/// A tapering tube with a closed, periodic B-spline surface for a side, written the way
/// exporters with periodic surfaces write it: uniform knots, the first control points
/// repeated at the end, `closed` flags set. The vertices are at the closed direction's
/// parameter `at` (0 is where the curves and the surface start and end; the range is 0
/// to 8). With `across`, the surface's `u` and `v` are swapped (it closes in `v`).
/// Returns the file and the volume.
fn spline_tube(kind: Tube, at: f64, across: bool) -> (String, f64) {
    let (height, taper) = (10.0, 0.5);
    let ring = |z: f64, scale: f64| -> Vec<DVec3> {
        let mut points: Vec<DVec3> = (0..8)
            .map(|k| {
                let a = f64::from(k) * TAU / 8.0;
                v3(6.0 * a.cos() * scale, 4.0 * a.sin() * scale, z)
            })
            .collect();
        points.extend_from_within(..3);
        points
    };
    let (bottom, top) = (ring(0.0, 1.0), ring(height, taper));
    let uniform: Vec<f64> = (0..15).map(|k| f64::from(k) - 3.0).collect();
    let curve = |points: &[DVec3]| {
        NurbsCurve::from_unclamped(3, uniform.clone(), points.to_vec(), None).unwrap()
    };
    let (c0, c1) = (curve(&bottom), curve(&top));
    assert!(c0.is_closed(1e-12));
    let volume = enclosed_area(&c0) * height * (1.0 + taper + taper * taper) / 3.0;

    let mut w = Step::default();
    let (repeats, values) = knot_lists(&uniform);
    let closed_curve = |w: &mut Step, points: &[DVec3]| {
        let points = w.points(points, 1.0);
        w.add(format!(
            "B_SPLINE_CURVE_WITH_KNOTS('',3,{points},.UNSPECIFIED.,.T.,.F.,{repeats},{values},\
             .UNIFORM_KNOTS.)"
        ))
    };
    let (curve0, curve1) = (closed_curve(&mut w, &bottom), closed_curve(&mut w, &top));
    // The natural normal is outwards when u goes round and v up, inwards when swapped.
    let surface = if across {
        let rows = [w.points(&bottom, 1.0), w.points(&top, 1.0)];
        w.add(format!(
            "B_SPLINE_SURFACE_WITH_KNOTS('',1,3,({}),.UNSPECIFIED.,.F.,.T.,.F.,(2,2),{repeats},\
             (0.,1.),{values},.UNSPECIFIED.)",
            rows.join(",")
        ))
    } else {
        let rows: Vec<String> = (0..11)
            .map(|i| w.points(&[bottom[i], top[i]], 1.0))
            .collect();
        w.add(format!(
            "B_SPLINE_SURFACE_WITH_KNOTS('',3,1,({}),.UNSPECIFIED.,.T.,.F.,.F.,{repeats},(2,2),\
             {values},(0.,1.),.UNSPECIFIED.)",
            rows.join(",")
        ))
    };
    let below = w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X);
    let above = w.plane(v3(0.0, 0.0, height), DVec3::Z, DVec3::X);
    let mut faces = Vec::new();
    if kind == Tube::Halves {
        let other = (at + 4.0) % 8.0;
        let (a0, a1) = (w.vertex(c0.point(at)), w.vertex(c1.point(at)));
        let (b0, b1) = (w.vertex(c0.point(other)), w.vertex(c1.point(other)));
        // Each ring in two edges: one of them runs across the curve's start.
        let e0 = [w.edge(a0, b0, curve0, true), w.edge(b0, a0, curve0, true)];
        let e1 = [w.edge(a1, b1, curve1, true), w.edge(b1, a1, curve1, true)];
        let seam_a = w.line(c0.point(at), c1.point(at));
        let seam_a = w.edge(a0, a1, seam_a, true);
        let seam_b = w.line(c0.point(other), c1.point(other));
        let seam_b = w.edge(b0, b1, seam_b, true);
        let bound = w.bound("FACE_BOUND", &[(e0[0], false), (e0[1], false)], true);
        faces.push(w.face(&[bound], below, true));
        let bound = w.bound("FACE_BOUND", &[(e1[0], true), (e1[1], true)], true);
        faces.push(w.face(&[bound], above, true));
        for (k, (first, second)) in [(seam_a, seam_b), (seam_b, seam_a)].into_iter().enumerate() {
            let uses = [
                (e0[k], true),
                (second, true),
                (e1[k], false),
                (first, false),
            ];
            let bound = w.bound("FACE_OUTER_BOUND", &uses, true);
            faces.push(w.face(&[bound], surface, !across));
        }
    } else {
        let (v0, v1) = (w.vertex(c0.point(at)), w.vertex(c1.point(at)));
        let e0 = w.edge(v0, v0, curve0, true);
        let e1 = w.edge(v1, v1, curve1, true);
        let bound = w.bound("FACE_BOUND", &[(e0, false)], true);
        faces.push(w.face(&[bound], below, true));
        let bound = w.bound("FACE_BOUND", &[(e1, true)], true);
        faces.push(w.face(&[bound], above, true));
        let bounds = if kind == Tube::Seam {
            let seam = w.line(c0.point(at), c1.point(at));
            let seam = w.edge(v0, v1, seam, true);
            let uses = [(e0, true), (seam, true), (e1, false), (seam, false)];
            vec![w.bound("FACE_OUTER_BOUND", &uses, true)]
        } else {
            vec![
                w.bound("FACE_BOUND", &[(e0, true)], true),
                w.bound("FACE_BOUND", &[(e1, false)], true),
            ]
        };
        faces.push(w.face(&bounds, surface, !across));
    }
    let brep = w.solid("Tube", &faces);
    (w.part(&[brep], UnitStyle::Millimetres), volume)
}

#[test]
fn closed_spline_surface_with_a_seam() {
    // The seam where the surface's parameter starts and ends, and elsewhere; closed in u
    // and in v.
    for across in [false, true] {
        for at in [0.0, 2.6, 7.5] {
            let (text, volume) = spline_tube(Tube::Seam, at, across);
            let imported = import(&text);
            assert_eq!(imported.warnings, Vec::<String>::new());
            let s = &imported.bodies[0].solid;
            // The side comes in two halves: the kernel's freeform faces don't go all
            // the way round. Each cap's edge is split where the cut meets it.
            assert_eq!(tuple(valid(s)), (4, 6, 4, 0, 1, 0), "at {at}");
            assert_eq!(freeform_faces(s), 2);
            let got = measure::volume(s);
            assert!(
                (got - volume).abs() < 1e-6 * volume,
                "{got} instead of {volume}"
            );
            for face in &s.faces {
                if let Surface::Nurbs(surface) = &face.surface {
                    assert!(!surface.is_closed(true, 1e-6) && !surface.is_closed(false, 1e-6));
                }
            }
            for e in &s.edges {
                let (a, b) = (s.vertex(e.start).point, s.vertex(e.end).point);
                assert!(e.t1 > e.t0);
                assert!(e.curve.point(e.t0).distance(a) < 1e-7, "at {at}");
                assert!(e.curve.point(e.t1).distance(b) < 1e-7, "at {at}");
                if let Some((lo, hi)) = e.curve.domain() {
                    assert!(lo <= e.t0 && e.t1 <= hi);
                }
            }
        }
    }
}

#[test]
fn closed_spline_surface_in_halves() {
    // Faces that stop at seams of their own: one of the two runs across the surface's
    // start, and so do its edges across their curves'.
    for across in [false, true] {
        for at in [0.0, 1.3, 6.0] {
            let (text, volume) = spline_tube(Tube::Halves, at, across);
            let imported = import(&text);
            let s = &imported.bodies[0].solid;
            assert_eq!(tuple(valid(s)), (4, 6, 4, 0, 1, 0), "at {at}");
            let got = measure::volume(s);
            assert!(
                (got - volume).abs() < 1e-6 * volume,
                "{got} instead of {volume}"
            );
            for e in &s.edges {
                if let Some((lo, hi)) = e.curve.domain() {
                    assert!(lo <= e.t0 && e.t0 < e.t1 && e.t1 <= hi);
                }
            }
        }
    }
}

#[test]
fn closed_spline_surface_without_a_seam_is_refused() {
    for across in [false, true] {
        let (text, _) = spline_tube(Tube::Seamless, 1.0, across);
        let message = import_error(&text);
        assert!(
            message.contains("a freeform face that wraps all the way round its surface"),
            "{message}"
        );
        assert!(message.contains("(entity #"), "{message}");
    }
}

/// A torus as one rational B-spline face that closes both ways, with its two seams.
fn spline_torus(major: f64, minor: f64) -> String {
    let tube_frame = Frame::from_origin_z_x(v3(major, 0.0, 0.0), -DVec3::Y, DVec3::X).unwrap();
    let tube = NurbsCurve::arc(&tube_frame, minor, 0.0, TAU);
    let round = NurbsCurve::arc(&Frame::WORLD, 1.0, 0.0, TAU);
    let (tube_weights, round_weights) = (tube.weights().unwrap(), round.weights().unwrap());
    let mut points = Vec::new();
    let mut weights = Vec::new();
    for (i, corner) in round.control_points().iter().enumerate() {
        for (j, p) in tube.control_points().iter().enumerate() {
            points.push(v3(p.x * corner.x, p.x * corner.y, p.z));
            weights.push(round_weights[i] * tube_weights[j]);
        }
    }
    let surface = NurbsSurface::new(
        2,
        2,
        round.knots().to_vec(),
        tube.knots().to_vec(),
        points,
        Some(weights),
    )
    .unwrap();
    let mut w = Step::default();
    let surface = w.spline_surface(&surface, 1.0);
    let corner = v3(major + minor, 0.0, 0.0);
    let v = w.vertex(corner);
    let around = w.circle(DVec3::ZERO, DVec3::Z, DVec3::X, major + minor);
    let around = w.edge(v, v, around, true);
    let through = w.spline_curve(&tube, 1.0);
    let through = w.edge(v, v, through, true);
    let uses = [
        (around, true),
        (through, true),
        (around, false),
        (through, false),
    ];
    let bound = w.bound("FACE_OUTER_BOUND", &uses, true);
    let face = w.face(&[bound], surface, true);
    let brep = w.solid("Ring", &[face]);
    w.part(&[brep], UnitStyle::Millimetres)
}

#[test]
fn spline_torus_is_cut_into_four() {
    let imported = import(&spline_torus(10.0, 3.0));
    let s = &imported.bodies[0].solid;
    // Cut once each way: four faces round four vertices.
    assert_eq!(tuple(valid(s)), (4, 8, 4, 0, 1, 1));
    assert_eq!(freeform_faces(s), 4);
    let volume = 2.0 * PI * PI * 10.0 * 9.0;
    let got = measure::volume(s);
    assert!(
        (got - volume).abs() < 1e-6 * volume,
        "{got} instead of {volume}"
    );
    let surface = 4.0 * PI * PI * 10.0 * 3.0;
    assert!((area(s) - surface).abs() < 1e-6 * surface, "{}", area(s));
}

#[test]
fn broken_splines_give_errors() {
    let solid = d_prism(&wavy_profile(), 6.0);
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "D", &solid, &PLAIN);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    import(&text);
    let curve_knots = "( 4, 1, 1, 4 ), ( 0.0, 0.4, 0.7, 1.0 ), .UNSPECIFIED. )";
    assert_eq!(text.matches(curve_knots).count(), 2);
    let curve = "B_SPLINE_CURVE_WITH_KNOTS ( 'NONE', 3,";
    let cases: [(&str, &str, &str); 11] = [
        // Knots against control points.
        (
            curve_knots,
            "( 4, 1, 4 ), ( 0.0, 0.4, 1.0 ), .UNSPECIFIED. )",
            "has 9 knots",
        ),
        (
            curve_knots,
            "( 4, 1, 1, 4 ), ( 0.0, 0.4, 1.0 ), .UNSPECIFIED. )",
            "lists 4 knot multiplicities for 3 knots",
        ),
        (
            curve_knots,
            "( 4, 2, 1, 4 ), ( 0.0, 0.4, 0.7, 1.0 ), .UNSPECIFIED. )",
            "has 11 knots",
        ),
        (
            curve_knots,
            "( 4, 1, 999999999999, 4 ), ( 0.0, 0.4, 0.7, 1.0 ), .UNSPECIFIED. )",
            "where its 6 control points of degree 3 need 10",
        ),
        (
            curve_knots,
            "( 4, 1, 0, 5 ), ( 0.0, 0.4, 0.7, 1.0 ), .UNSPECIFIED. )",
            "whole numbers, 1 or more",
        ),
        (
            curve_knots,
            "( 4, 1, 1, 4 ), ( 0.0, 0.7, 0.4, 1.0 ), .UNSPECIFIED. )",
            "the knots must be finite and not decrease",
        ),
        (
            curve_knots,
            "( 4, 1, 1, 4 ), ( 0.0, 0.4, 'x', 1.0 ), .UNSPECIFIED. )",
            "a list of numbers",
        ),
        // The degree.
        (
            curve,
            "B_SPLINE_CURVE_WITH_KNOTS ( 'NONE', 0,",
            "a B-spline of degree 0",
        ),
        (
            curve,
            "B_SPLINE_CURVE_WITH_KNOTS ( 'NONE', 7,",
            "too few for its degree of 7",
        ),
        (
            curve,
            "B_SPLINE_CURVE_WITH_KNOTS ( 'NONE', 3.5,",
            "should be a whole number",
        ),
        (
            "B_SPLINE_SURFACE_WITH_KNOTS ( 'NONE', 3, 1,",
            "B_SPLINE_SURFACE_WITH_KNOTS ( 'NONE', 3, 99,",
            "a B-spline of degree 99 in v",
        ),
    ];
    for (from, to, expected) in cases {
        assert!(text.contains(from), "{from}");
        let message = import_error(&text.replacen(from, to, 1));
        assert!(message.contains(expected), "{to}: {message}");
        assert!(message.contains("B_SPLINE_"), "{message}");
    }
    let surface_knots = "( 4, 1, 1, 4 ), ( 2, 2 ), ( 0.0,";
    assert!(text.contains(surface_knots), "{text}");
    let message =
        import_error(&text.replacen(surface_knots, "( 4, 1, 1, 4 ), ( 2, 3 ), ( 0.0,", 1));
    assert!(message.contains("has 5 knots in v"), "{message}");
    let message = import_error(&text.replacen(surface_knots, "( 4, 1, 4 ), ( 2, 2 ), ( 0.0,", 1));
    assert!(
        message.contains("lists 3 knot multiplicities for 4 knots in u"),
        "{message}"
    );
    // A row of control points short of one.
    let net = text
        .find("B_SPLINE_SURFACE_WITH_KNOTS ( 'NONE', 3, 1, ( (")
        .unwrap();
    let comma = net + text[net..].find(",#").unwrap();
    let end = comma + text[comma..].find(')').unwrap();
    let message = import_error(&format!("{}{}", &text[..comma], &text[end..]));
    assert!(message.contains("not all the same length"), "{message}");

    // Weights against control points, and weights that are no use.
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Half", &half_disc(), &PLAIN);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    import(&text);
    let half = "0.7071067811865476";
    let curve_weights = format!("RATIONAL_B_SPLINE_CURVE ( ( 1.0, {half}, 1.0, {half}, 1.0 ) )");
    let surface_weights =
        format!("RATIONAL_B_SPLINE_SURFACE ( ( ( 1.0, 1.0 ), ( {half}, {half} ),");
    let rows = "RATIONAL_B_SPLINE_SURFACE ( ( ( 1.0, 1.0 ),";
    let cases: [(&str, String, &str); 7] = [
        (
            &curve_weights,
            format!("RATIONAL_B_SPLINE_CURVE ( ( 1.0, {half}, 1.0, 1.0 ) )"),
            "has 4 weights for 5 control points",
        ),
        (
            &curve_weights,
            "RATIONAL_B_SPLINE_CURVE ( ( 1.0, 0.7, 1.0, -0.7, 1.0 ) )".to_owned(),
            "one positive weight per control point",
        ),
        (
            &curve_weights,
            "RATIONAL_B_SPLINE_CURVE ( ( 1.0, 0.7, 1.0, #5, 1.0 ) )".to_owned(),
            "a list of numbers",
        ),
        (
            &curve_weights,
            "RATIONAL_B_SPLINE_CURVE ( 1.0 )".to_owned(),
            "should be a list",
        ),
        (
            &surface_weights,
            rows.to_owned(),
            "are not one for each of its 5 by 2 control points",
        ),
        (
            rows,
            "RATIONAL_B_SPLINE_SURFACE ( ( ( 1.0, 1.0, 1.0 ),".to_owned(),
            "are not one for each",
        ),
        (
            rows,
            "RATIONAL_B_SPLINE_SURFACE ( ( ( 1.0, 0.0 ),".to_owned(),
            "one positive weight per control point",
        ),
    ];
    for (from, to, expected) in cases {
        assert!(text.contains(from), "{from}: {text}");
        let message = import_error(&text.replacen(from, &to, 1));
        assert!(message.contains(expected), "{to}: {message}");
    }
    // A B-spline with no knots to go by, and Bézier pieces that don't come out even.
    let knots = "B_SPLINE_CURVE_WITH_KNOTS ( ( 3, 2, 3 ), ( 0.0, 0.5, 1.0 ), \
                 .PIECEWISE_BEZIER_KNOTS. )";
    assert!(text.contains(knots), "{text}");
    let message = import_error(&text.replacen(knots, "", 1));
    assert!(message.contains("without a knot vector"), "{message}");
    // (Five control points of degree 2 are two Bézier pieces: the same curve.)
    import(&text.replacen(knots, "BEZIER_CURVE ( )", 1));
    let odd = text
        .replacen("B_SPLINE_CURVE ( 2, (", "B_SPLINE_CURVE ( 3, (", 1)
        .replacen(knots, "BEZIER_CURVE ( )", 1);
    let message = import_error(&odd);
    assert!(
        message.contains("don't make whole Bézier pieces"),
        "{message}"
    );
}

// ---- Swept surfaces ----

impl Step {
    fn extrusion(&mut self, curve: u32, dir: DVec3) -> u32 {
        let d = self.dir(dir);
        let vector = self.add(format!("VECTOR('',#{d},1.)"));
        self.add(format!(
            "SURFACE_OF_LINEAR_EXTRUSION('',#{curve},#{vector})"
        ))
    }

    /// The surface `curve` sweeps when turned about the world's Z axis.
    fn revolution(&mut self, curve: u32) -> u32 {
        let origin = self.point(DVec3::ZERO);
        // Without a direction, the axis is along Z.
        let axis = self.add(format!("AXIS1_PLACEMENT('',#{origin},$)"));
        self.add(format!("SURFACE_OF_REVOLUTION('',#{curve},#{axis})"))
    }
}

#[test]
fn extruded_surfaces() {
    // The D prism's sides as swept surfaces: the B-spline swept up is a freeform
    // surface, the chord swept up is a plane.
    let profile = wavy_profile();
    let solid = d_prism(&profile, 6.0);
    let swept = Foreign {
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let curve = w.spline_curve(&profile, 1.0);
                Some(w.extrusion(curve, DVec3::Z))
            }
            Surface::Plane(p) if p.normal().abs_diff_eq(DVec3::Y, 1e-12) => {
                // From the far end: the surface then faces +Y, as the plane does.
                let chord = w.line(v3(10.0, 0.0, 0.0), DVec3::ZERO);
                Some(w.extrusion(chord, DVec3::Z))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Swept", &solid, &swept);
    let text = w.part(&[brep], UnitStyle::Millimetres);
    assert_eq!(text.matches("SURFACE_OF_LINEAR_EXTRUSION").count(), 2);
    let s = same_body(&text, &solid);
    assert_eq!(freeform_faces(&s), 1);
    let Surface::Nurbs(side) = &s.faces[3].surface else {
        panic!("the swept B-spline should be a freeform surface");
    };
    assert_eq!(side.degrees(), (3, 1));
    // Swept the other way the surface faces inwards: the face says so, or the body
    // would come out wrong.
    let down = Foreign {
        scale: 1.0 / 25.4,
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let curve = w.spline_curve(&profile, 1.0 / 25.4);
                Some(w.extrusion(curve, -DVec3::Z))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut inwards = solid.clone();
    inwards.faces[3].reversed = true;
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Swept", &inwards, &down);
    same_body(&w.part(&[brep], UnitStyle::Inches), &solid);

    // A circle swept along its axis is a cylinder.
    let solid = half_disc();
    let round = Foreign {
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let circle = w.circle(v3(5.0, 0.0, 0.0), DVec3::Z, DVec3::X, 5.0);
                Some(w.extrusion(circle, DVec3::Z))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Half", &solid, &round);
    let s = same_body(&w.part(&[brep], UnitStyle::Millimetres), &solid);
    assert!(matches!(s.faces[3].surface, Surface::Cylinder(c) if c.radius == 5.0));
    // A circle that runs the other way round sweeps a surface that faces inwards; the
    // kernel's cylinder faces outwards, so the face is turned over.
    let clockwise = Foreign {
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let circle = w.circle(v3(5.0, 0.0, 0.0), -DVec3::Z, DVec3::X, 5.0);
                Some(w.extrusion(circle, DVec3::Z))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut inwards = solid.clone();
    inwards.faces[3].reversed = true;
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Half", &inwards, &clockwise);
    let s = same_body(&w.part(&[brep], UnitStyle::Millimetres), &solid);
    assert!(matches!(s.faces[3].surface, Surface::Cylinder(_)) && !s.faces[3].reversed);
    // Swept askew it is not: a freeform surface, cut down from the whole tube to the
    // half the face is on. (The body is no longer closed: the caps don't fit.)
    let askew = Foreign {
        surface: &|w, s| match s {
            Surface::Nurbs(_) => {
                let circle = w.circle(v3(5.0, 0.0, 0.0), DVec3::Z, DVec3::X, 5.0);
                Some(w.extrusion(circle, v3(0.0, 0.3, 1.0)))
            }
            _ => None,
        },
        ..PLAIN
    };
    let mut w = Step::default();
    let brep = foreign_brep(&mut w, "Askew", &solid, &askew);
    let message = import_error(&w.part(&[brep], UnitStyle::Millimetres));
    assert!(message.contains("not a closed, valid solid"), "{message}");
}

/// The volume of the solid a curve in the XZ plane sweeps when turned about Z, between
/// the planes through its ends.
fn turned_volume(c: &NurbsCurve) -> f64 {
    let (lo, hi) = c.domain();
    let n = 20_000;
    let at = |k: usize| c.point(lo + (hi - lo) * k as f64 / n as f64);
    (0..n)
        .map(|k| {
            let (p, q) = (at(k), at(k + 1));
            PI * (p.x * p.x + p.x * q.x + q.x * q.x) / 3.0 * (q.z - p.z)
        })
        .sum()
}

/// A body of revolution: `profile` (from the bottom to the top, in the XZ plane) turned
/// about Z, with a seam along it and flat ends. `side` writes the profile's curve.
fn turned_body(a: DVec3, b: DVec3, side: &dyn Fn(&mut Step) -> u32, flat_ends: bool) -> String {
    let mut w = Step::default();
    let curve = side(&mut w);
    let surface = w.revolution(curve);
    let (below, above) = if flat_ends {
        (
            w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X),
            w.plane(v3(0.0, 0.0, b.z), DVec3::Z, DVec3::X),
        )
    } else {
        // Lines square to the axis, turned: planes that face −Z.
        let low = w.line(v3(1.0, 0.0, a.z), v3(3.0, 0.0, a.z));
        let high = w.line(v3(1.0, 0.0, b.z), v3(3.0, 0.0, b.z));
        (w.revolution(low), w.revolution(high))
    };
    let (v0, v1) = (w.vertex(a), w.vertex(b));
    let c0 = w.circle(v3(0.0, 0.0, a.z), DVec3::Z, DVec3::X, a.x);
    let c1 = w.circle(v3(0.0, 0.0, b.z), DVec3::Z, DVec3::X, b.x);
    let (e0, e1) = (w.edge(v0, v0, c0, true), w.edge(v1, v1, c1, true));
    let seam = side(&mut w);
    let seam = w.edge(v0, v1, seam, true);
    let mut faces = Vec::new();
    let bound = w.bound("FACE_BOUND", &[(e0, false)], true);
    faces.push(w.face(&[bound], below, true));
    let bound = w.bound("FACE_BOUND", &[(e1, true)], true);
    faces.push(w.face(&[bound], above, flat_ends));
    let uses = [(e0, true), (seam, true), (e1, false), (seam, false)];
    let bound = w.bound("FACE_BOUND", &uses, true);
    faces.push(w.face(&[bound], surface, true));
    let brep = w.solid("Turned", &faces);
    w.part(&[brep], UnitStyle::Millimetres)
}

fn vase_profile() -> NurbsCurve {
    NurbsCurve::new(
        3,
        vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        vec![
            v3(4.0, 0.0, 0.0),
            v3(8.0, 0.0, 3.0),
            v3(1.0, 0.0, 8.0),
            v3(3.0, 0.0, 12.0),
        ],
        None,
    )
    .unwrap()
}

fn turned_vase() -> String {
    let profile = vase_profile();
    turned_body(
        v3(4.0, 0.0, 0.0),
        v3(3.0, 0.0, 12.0),
        &|w| w.spline_curve(&profile, 1.0),
        true,
    )
}

#[test]
fn revolved_surfaces() {
    // Lines turned about an axis: a cone from the slanted one, planes from the ones
    // square to it.
    let (a, b) = (v3(5.0, 0.0, 0.0), v3(2.0, 0.0, 10.0));
    let text = turned_body(a, b, &|w| w.line(a, b), false);
    assert_eq!(text.matches("SURFACE_OF_REVOLUTION").count(), 3);
    let imported = import(&text);
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(s), PI * 10.0 / 3.0 * (25.0 + 10.0 + 4.0));
    let kinds: Vec<&str> = s
        .faces
        .iter()
        .map(|f| match f.surface {
            Surface::Plane(_) => "plane",
            Surface::Cone(_) => "cone",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["plane", "plane", "cone"]);
    // A line along the axis: a cylinder.
    let (a, b) = (v3(5.0, 0.0, 0.0), v3(5.0, 0.0, 10.0));
    let imported = import(&turned_body(a, b, &|w| w.line(a, b), true));
    let s = &imported.bodies[0].solid;
    assert!(matches!(s.faces[2].surface, Surface::Cylinder(c) if (c.radius - 5.0).abs() < 1e-12));
    assert_close(measure::volume(s), PI * 250.0);

    // A B-spline turned: a rational freeform surface, closed round the axis, so the
    // face is cut in two.
    let profile = vase_profile();
    let imported = import(&turned_vase());
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (4, 6, 4, 0, 1, 0));
    assert_eq!(freeform_faces(s), 2);
    let (got, volume) = (measure::volume(s), turned_volume(&profile));
    assert!(
        (got - volume).abs() < 1e-6 * volume,
        "{got} instead of {volume}"
    );
}

#[test]
fn faces_at_pinched_points_are_refused() {
    // A dome: the profile ends on the axis, where the surface is pinched to a point.
    // The side's loop is the base circle, the seam up to the tip and the seam back. The
    // kernel's freeform faces can't have a corner there yet.
    let profile = NurbsCurve::new(
        3,
        vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        vec![
            v3(4.0, 0.0, 0.0),
            v3(5.0, 0.0, 3.0),
            v3(2.0, 0.0, 6.0),
            v3(0.0, 0.0, 6.0),
        ],
        None,
    )
    .unwrap();
    let mut w = Step::default();
    let curve = w.spline_curve(&profile, 1.0);
    let surface = w.revolution(curve);
    let below = w.plane(DVec3::ZERO, -DVec3::Z, DVec3::X);
    let (v0, tip) = (w.vertex(v3(4.0, 0.0, 0.0)), w.vertex(v3(0.0, 0.0, 6.0)));
    let c0 = w.circle(DVec3::ZERO, DVec3::Z, DVec3::X, 4.0);
    let e0 = w.edge(v0, v0, c0, true);
    let seam = w.edge(v0, tip, curve, true);
    let bound = w.bound("FACE_BOUND", &[(e0, false)], true);
    let bottom = w.face(&[bound], below, true);
    let uses = [(e0, true), (seam, true), (seam, false)];
    let bound = w.bound("FACE_BOUND", &uses, true);
    let side = w.face(&[bound], surface, true);
    let brep = w.solid("Dome", &[bottom, side]);
    let message = import_error(&w.part(&[brep], UnitStyle::Millimetres));
    assert!(
        message.contains("comes to a point where its surface is pinched together"),
        "{message}"
    );
    assert!(message.contains("(entity #"), "{message}");

    // A three-sided patch: a B-spline surface with one edge squeezed to a point, as the
    // slanted side of a pyramid on the rectangle a, b, b1, a1 with its tip c above a.
    let (a, b, c) = (DVec3::ZERO, v3(4.0, 0.0, 0.0), v3(0.0, 0.0, 3.0));
    let (a1, b1) = (a + DVec3::Y * 5.0, b + DVec3::Y * 5.0);
    let corners = [a, b, c, a1, b1];
    // u from b to b1, v from that edge up to the tip.
    let knots = vec![0.0, 0.0, 1.0, 1.0];
    let patch = NurbsSurface::new(1, 1, knots.clone(), knots, vec![b, c, b1, c], None).unwrap();
    assert!(patch.normal(v2(0.5, 0.5)).x > 0.0);
    let mut w = Step::default();
    let slanted = w.spline_surface(&patch, 1.0);
    let ids = corners.map(|p| w.vertex(p));
    let line = |w: &mut Step, from: usize, to: usize| {
        let curve = w.line(corners[from], corners[to]);
        w.edge(ids[from], ids[to], curve, true)
    };
    let (ab, bb1, b1a1, a1a) = (
        line(&mut w, 0, 1),
        line(&mut w, 1, 4),
        line(&mut w, 4, 3),
        line(&mut w, 3, 0),
    );
    let (ac, bc, b1c, a1c) = (
        line(&mut w, 0, 2),
        line(&mut w, 1, 2),
        line(&mut w, 4, 2),
        line(&mut w, 3, 2),
    );
    let sides: [(u32, Vec<EdgeUse>); 5] = [
        (
            w.plane(a, -DVec3::Z, DVec3::X),
            vec![(a1a, false), (b1a1, false), (bb1, false), (ab, false)],
        ),
        (
            w.plane(a, -DVec3::Y, DVec3::X),
            vec![(ab, true), (bc, true), (ac, false)],
        ),
        (
            w.plane(a, -DVec3::X, DVec3::Y),
            vec![(ac, true), (a1c, false), (a1a, true)],
        ),
        (
            w.plane(a1, (c - a1).cross(b1 - a1), DVec3::X),
            vec![(b1a1, true), (a1c, true), (b1c, false)],
        ),
        (slanted, vec![(bb1, true), (b1c, true), (bc, false)]),
    ];
    let faces: Vec<u32> = sides
        .iter()
        .map(|(surface, uses)| {
            let bound = w.bound("FACE_BOUND", uses, true);
            w.face(&[bound], *surface, true)
        })
        .collect();
    let brep = w.solid("Pyramid", &faces);
    let message = import_error(&w.part(&[brep], UnitStyle::Millimetres));
    assert!(
        message.contains("comes to a point where its surface is pinched together"),
        "{message}"
    );
    // With a plane for the slanted side, the same file is a pyramid.
    let mut w = Step::default();
    let ids = corners.map(|p| w.vertex(p));
    let line = |w: &mut Step, from: usize, to: usize| {
        let curve = w.line(corners[from], corners[to]);
        w.edge(ids[from], ids[to], curve, true)
    };
    let (ab, bb1, b1a1, a1a) = (
        line(&mut w, 0, 1),
        line(&mut w, 1, 4),
        line(&mut w, 4, 3),
        line(&mut w, 3, 0),
    );
    let (ac, bc, b1c, a1c) = (
        line(&mut w, 0, 2),
        line(&mut w, 1, 2),
        line(&mut w, 4, 2),
        line(&mut w, 3, 2),
    );
    let sides: [(u32, Vec<EdgeUse>); 5] = [
        (
            w.plane(a, -DVec3::Z, DVec3::X),
            vec![(a1a, false), (b1a1, false), (bb1, false), (ab, false)],
        ),
        (
            w.plane(a, -DVec3::Y, DVec3::X),
            vec![(ab, true), (bc, true), (ac, false)],
        ),
        (
            w.plane(a, -DVec3::X, DVec3::Y),
            vec![(ac, true), (a1c, false), (a1a, true)],
        ),
        (
            w.plane(a1, (c - a1).cross(b1 - a1), DVec3::X),
            vec![(b1a1, true), (a1c, true), (b1c, false)],
        ),
        (
            w.plane(b, (b1 - b).cross(c - b), DVec3::Y),
            vec![(bb1, true), (b1c, true), (bc, false)],
        ),
    ];
    let faces: Vec<u32> = sides
        .iter()
        .map(|(surface, uses)| {
            let bound = w.bound("FACE_BOUND", uses, true);
            w.face(&[bound], *surface, true)
        })
        .collect();
    let brep = w.solid("Pyramid", &faces);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    assert_close(measure::volume(&imported.bodies[0].solid), 20.0);
}

#[test]
fn revolved_circles() {
    // A circle beside the axis, turned: a torus, as the kernel has it, with its seams.
    let (major, minor) = (10.0, 3.0);
    let mut w = Step::default();
    let tube = w.circle(v3(major, 0.0, 0.0), -DVec3::Y, DVec3::X, minor);
    let surface = w.revolution(tube);
    let v = w.vertex(v3(major + minor, 0.0, 0.0));
    let around = w.circle(DVec3::ZERO, DVec3::Z, DVec3::X, major + minor);
    let around = w.edge(v, v, around, true);
    let through = w.edge(v, v, tube, true);
    let uses = [
        (around, true),
        (through, true),
        (around, false),
        (through, false),
    ];
    let bound = w.bound("FACE_OUTER_BOUND", &uses, true);
    let face = w.face(&[bound], surface, true);
    let brep = w.solid("Ring", &[face]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert_eq!(tuple(valid(s)), (1, 2, 1, 0, 1, 1));
    assert!(matches!(s.faces[0].surface, Surface::Torus(t) if t.major == major));
    assert_close(measure::volume(s), 2.0 * PI * PI * major * minor * minor);

    // A circle about a point of the axis, turned: a sphere. With no bounds at all it
    // gets the kernel's seam.
    let mut w = Step::default();
    let circle = w.circle(v3(0.0, 0.0, 2.0), -DVec3::Y, DVec3::X, 5.0);
    let surface = w.revolution(circle);
    let face = w.face(&[], surface, true);
    let brep = w.solid("Ball", &[face]);
    let imported = import(&w.part(&[brep], UnitStyle::Millimetres));
    assert_eq!(imported.warnings, Vec::<String>::new());
    let s = &imported.bodies[0].solid;
    assert!(matches!(s.faces[0].surface, Surface::Sphere(b) if b.radius == 5.0));
    valid(s);
    assert_close(measure::volume(s), 4.0 / 3.0 * PI * 125.0);
    let b = s.bounds();
    assert!((0.5 * (b.min + b.max)).abs_diff_eq(v3(0.0, 0.0, 2.0), 1e-9));

    // A line swept along itself, or turned about itself, is no surface.
    let mut w = Step::default();
    let line = w.line(DVec3::ZERO, DVec3::Z);
    let surface = w.revolution(line);
    let face = w.face(&[], surface, true);
    let brep = w.solid("Nothing", &[face]);
    let message = import_error(&w.part(&[brep], UnitStyle::Millimetres));
    assert!(
        message.contains("a swept surface PeetCAD can't import"),
        "{message}"
    );
    assert!(message.contains("SURFACE_OF_REVOLUTION"), "{message}");
}

// ---- Files that can't be imported ----

fn exported_plate() -> String {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, v2(40.0, 20.0));
    s.add_circle(v2(10.0, 10.0), 3.0);
    let plate: Vec<Region> = regions(&s)
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    let solid = extrude(&Plane::TOP, &plate, 0.0, 2.0).unwrap();
    step::write(&[("Plate", &solid)], &options(StepSchema::Ap214))
}

#[test]
fn empty_and_foreign_files() {
    assert!(import_error("").contains("empty"));
    assert!(import_error("  \r\n\t ").contains("empty"));
    for text in [
        "solid ascii\nfacet normal 0 0 1\nendsolid",
        "PK\u{3}\u{4}\u{14}\u{0}\u{0}\u{0}\u{8}\u{0}\u{fffd}\u{fffd}",
        "\u{0}\u{1}\u{2}ISO-10303-21;",
        "ISO-10303-22;",
        "'ISO-10303-21';",
    ] {
        assert!(import_error(text).contains("not a STEP file"), "{text:?}");
    }
    assert!(import_error("ISO-10303-21;").contains("no DATA section"));
    assert!(import_error("ISO-10303-21;HEADER;ENDSEC;").contains("no DATA section"));
    let no_solids = "ISO-10303-21;HEADER;ENDSEC;DATA;#1=CARTESIAN_POINT('',(0.,0.,0.));ENDSEC;\
                     END-ISO-10303-21;";
    assert!(import_error(no_solids).contains("no solids"));
    let empty_data = "ISO-10303-21;HEADER;ENDSEC;DATA;ENDSEC;END-ISO-10303-21;";
    assert!(import_error(empty_data).contains("no solids"));
}

#[test]
fn truncated_files() {
    let text = exported_plate();
    let data = text.find("DATA;").unwrap();
    let end = text.find("ENDSEC;\nEND-ISO").unwrap();
    // Cut anywhere in the data: an error that says so, whatever the cut lands in.
    let mut cut = data + 6;
    while cut < end {
        let message = import_error(&text[..cut]);
        assert!(message.contains("cut off"), "cut at {cut}: {message}");
        cut += 37;
    }
    // Cut in the header.
    for cut in [5, 13, 40, data, data + 4] {
        import_error(&text[..cut]);
    }
    // Without the closing line the model is all there.
    assert_eq!(import(&text[..end + 8]).bodies.len(), 1);
    // An unterminated comment or string.
    assert!(import_error(&text.replace("#30=", "/* #30=")).contains("cut off"));
    import_error(&text.replace("MANIFOLD_SOLID_BREP('Plate'", "MANIFOLD_SOLID_BREP('Plate"));
    assert!(import_error(&format!("{}'", &text[..end])).contains("cut off"));
}

/// Replaces the parameters of the first entity called `name`.
fn with_params(text: &str, name: &str, params: &str) -> String {
    with_params_of(text, name, 0, params)
}

/// Replaces the parameters of the entity called `name` that comes after `skip` others.
fn with_params_of(text: &str, name: &str, skip: usize, params: &str) -> String {
    let (at, _) = text
        .match_indices(&format!("={name}("))
        .nth(skip)
        .unwrap_or_else(|| panic!("no {name}"));
    let end = at + text[at..].find(";\n").unwrap();
    format!("{}={name}({params}){}", &text[..at], &text[end..])
}

#[test]
fn dangling_references_and_wrong_parameters() {
    let text = exported_plate();
    let cases: [(&str, &str, &str); 14] = [
        (
            "EDGE_CURVE",
            "'',#99999,#99999,#99998,.T.",
            "#99999, which the file doesn't define",
        ),
        ("MANIFOLD_SOLID_BREP", "'Plate',#99999", "doesn't define"),
        ("CLOSED_SHELL", "'',(#99999)", "doesn't define"),
        (
            "VERTEX_POINT",
            "''",
            "has 1 parameter where at least 2 are needed",
        ),
        (
            "VERTEX_POINT",
            "'','not a reference'",
            "should be a reference",
        ),
        ("ADVANCED_FACE", "'',#1,#2,.T.", "should be a list"),
        ("ADVANCED_FACE", "'',(1.0),#2,.T.", "list of references"),
        ("CLOSED_SHELL", "'',()", "has no faces"),
        ("EDGE_LOOP", "'',()", "has no edges"),
        ("ORIENTED_EDGE", "'',*,*,#1,.MAYBE.", ".T. or .F."),
        (
            "CLOSED_SHELL",
            "'',(#1,#2)",
            "where a ADVANCED_FACE or FACE_SURFACE should be",
        ),
        (
            "CYLINDRICAL_SURFACE",
            "'',#1,3.0",
            "where a AXIS2_PLACEMENT_3D should be",
        ),
        ("CIRCLE", "'',$,'three'", "should be a reference"),
        ("MANIFOLD_SOLID_BREP", "'Plate'", "has 1 parameter"),
    ];
    for (name, params, expected) in cases {
        let message = import_error(&with_params(&text, name, params));
        assert!(message.contains(expected), "{name}({params}): {message}");
    }
    // Geometry that makes no sense. (The file's first point and two directions are the
    // world placement, which no solid uses.)
    let geometry: [(&str, usize, &str, &str); 7] = [
        ("CARTESIAN_POINT", 1, "'',(0.0,0.0)", "three coordinates"),
        ("CARTESIAN_POINT", 1, "'',(0.0,'x',0.0)", "three numbers"),
        (
            "CARTESIAN_POINT",
            1,
            "'',(1.0E+30,0.0,0.0)",
            "from the origin",
        ),
        ("DIRECTION", 2, "'',(0.0,0.0,0.0)", "has no length"),
        ("CYLINDRICAL_SURFACE", 0, "'',#99999,3.0", "doesn't define"),
        ("CIRCLE", 0, "'',#99999,-3.0", "doesn't define"),
        ("VECTOR", 0, "'',#99999,1.0", "doesn't define"),
    ];
    for (name, skip, params, expected) in geometry {
        let message = import_error(&with_params_of(&text, name, skip, params));
        assert!(message.contains(expected), "{name}({params}): {message}");
    }
    // Broken entities that no solid uses don't matter.
    import(&with_params(&text, "CARTESIAN_POINT", "'',(0.0,0.0)"));
    // A radius of nothing, a number that isn't one, an entity defined twice.
    let cylinder = text.find("=CYLINDRICAL_SURFACE(").unwrap();
    let end = cylinder + text[cylinder..].find(";\n").unwrap();
    let zero = format!(
        "{},0.0){}",
        &text[..text[..end].rfind(',').unwrap()],
        &text[end..]
    );
    assert!(import_error(&zero).contains("not a usable shape"));
    assert!(import_error(&text.replace("#30=", "#29=")).contains("twice"));
    assert!(
        import_error(&text.replace("=VERTEX_POINT('',", "=VERTEX_POINT('',1.0E+999,"))
            .contains("number")
    );
    // A plate whose cap has lost its hole: no longer a closed solid.
    let (face, list_end) = text
        .match_indices("=ADVANCED_FACE('',(#")
        .map(|(at, _)| (at, at + text[at..].find(')').unwrap()))
        .find(|&(at, end)| text[at..end].matches('#').count() == 2)
        .expect("a face with two bounds");
    let comma = face + text[face..list_end].rfind(',').unwrap();
    let broken = format!("{}{}", &text[..comma], &text[list_end..]);
    let message = import_error(&broken);
    assert!(message.contains("not a closed, valid solid"), "{message}");
    assert!(message.contains("\"Plate\""), "{message}");
}

#[test]
fn reference_cycles() {
    let text = exported_plate();
    // A curve defined by itself, directly and through a second entity.
    let own = |text: &str, body: &str| {
        let at = text.find("=CIRCLE(").unwrap();
        let id: u32 = text[text[..at].rfind('#').unwrap() + 1..at]
            .parse()
            .unwrap();
        with_params(text, "CIRCLE", "X")
            .replace("=CIRCLE(X)", &body.replace("{id}", &id.to_string()))
    };
    let message = import_error(&own(&text, "=SURFACE_CURVE('',#{id},(),.CURVE_3D.)"));
    assert!(message.contains("defined in terms of itself"), "{message}");
    let message = import_error(&own(
        &text,
        "=TRIMMED_CURVE('',#{id},(0.0),(1.0),.T.,.PARAMETER.)",
    ));
    assert!(message.contains("defined in terms of itself"), "{message}");
    // A loop that lists itself, a shell that is its own face, a brep that is its own shell.
    for (name, expected) in [
        ("EDGE_LOOP", "ORIENTED_EDGE should be"),
        ("CLOSED_SHELL", "should be"),
        ("MANIFOLD_SOLID_BREP", "should be"),
    ] {
        let at = text.find(&format!("={name}(")).unwrap();
        let id: u32 = text[text[..at].rfind('#').unwrap() + 1..at]
            .parse()
            .unwrap();
        let params = if name == "MANIFOLD_SOLID_BREP" {
            format!("'',#{id}")
        } else {
            format!("'',(#{id})")
        };
        let message = import_error(&with_params(&text, name, &params));
        assert!(message.contains(expected), "{name}: {message}");
    }
    // Units that go round in circles are ignored, and an assembly that contains itself
    // leaves the part where it is.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Block", &faces);
    let n = w.n;
    let unit = w.add(format!(
        "(CONVERSION_BASED_UNIT('X',#{})LENGTH_UNIT()NAMED_UNIT(*))",
        n + 2
    ));
    w.add(format!(
        "LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(2.0),#{unit})"
    ));
    let context = w.add(format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#{unit}))\
         REPRESENTATION_CONTEXT('',''))"
    ));
    let rep = w.representation(&[brep], context);
    let origin = w.axis(DVec3::ZERO, DVec3::Z, DVec3::X);
    let moved = w.axis(DVec3::X, DVec3::Z, DVec3::X);
    let transformation = w.add(format!(
        "ITEM_DEFINED_TRANSFORMATION('','',#{origin},#{moved})"
    ));
    let other = w.add(format!("SHAPE_REPRESENTATION('',(#{origin}),#{context})"));
    for (a, b) in [(rep, other), (other, rep)] {
        w.add(format!(
            "(REPRESENTATION_RELATIONSHIP('','',#{a},#{b})\
             REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{transformation})\
             SHAPE_REPRESENTATION_RELATIONSHIP())"
        ));
    }
    let imported = import(&w.file());
    assert_eq!(imported.bodies.len(), 1);
    assert_close(measure::volume(&imported.bodies[0].solid), 6.0);
    assert_eq!(imported.warnings.len(), 2, "{:?}", imported.warnings);
    assert!(imported.warnings[0].contains("millimetres"));
    assert!(imported.warnings[1].contains("placements"));
}

#[test]
fn unsupported_geometry() {
    let text = exported_plate();
    // Surfaces and curves the kernel has nothing for.
    for surface in [
        "OFFSET_SURFACE('',#1,1.0,.F.)",
        "CURVE_BOUNDED_SURFACE('',#1,(#2),.T.)",
        "RECTANGULAR_TRIMMED_SURFACE('',#1,0.,1.,0.,1.,.T.,.T.)",
    ] {
        let message = import_error(
            &with_params(&text, "CYLINDRICAL_SURFACE", "X")
                .replace("=CYLINDRICAL_SURFACE(X)", &format!("={surface}")),
        );
        assert!(
            message.contains("a surface of a kind PeetCAD can't import yet"),
            "{message}"
        );
        let name = surface.split('(').next().unwrap();
        assert!(
            message.contains(name) && message.contains("(entity #"),
            "{message}"
        );
        assert!(message.contains("B-spline surfaces"), "{message}");
    }
    let message = import_error(
        &with_params(&text, "CIRCLE", "X")
            .replace("=CIRCLE(X)", "=OFFSET_CURVE_3D('',#1,1.0,.F.,#2)"),
    );
    assert!(
        message.contains("a curve of a kind PeetCAD can't import yet"),
        "{message}"
    );
    assert!(message.contains("OFFSET_CURVE_3D"), "{message}");
    // An edge with only a curve in its surface's parameters.
    let message =
        import_error(&with_params(&text, "CIRCLE", "X").replace("=CIRCLE(X)", "=PCURVE('',#1,#2)"));
    assert!(
        message.contains("only as a curve in its surface's parameters"),
        "{message}"
    );
    assert!(message.contains("PCURVE") && message.contains("(entity #"));
    // B-splines are read, so what is wrong with one is said: here its control points
    // are not points.
    let spline = "'',1,1,((#1,#2),(#3,#4)),.UNSPECIFIED.,.F.,.F.,.F.,(2,2),(2,2),(0.,1.),(0.,1.),\
                  .UNSPECIFIED.";
    let message = import_error(&with_params(&text, "CYLINDRICAL_SURFACE", "X").replace(
        "=CYLINDRICAL_SURFACE(X)",
        &format!("=B_SPLINE_SURFACE_WITH_KNOTS({spline})"),
    ));
    assert!(
        message.contains("where a CARTESIAN_POINT should be"),
        "{message}"
    );
    assert!(message.contains("B_SPLINE_SURFACE_WITH_KNOTS"), "{message}");
    // A rational B-spline is a complex entity.
    let message = import_error(&with_params(&text, "CIRCLE", "X").replace(
        "=CIRCLE(X)",
        "=(BOUNDED_CURVE()B_SPLINE_CURVE(2,(#1,#2,#3),.UNSPECIFIED.,.F.,.F.)\
         B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.)CURVE()\
         GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE((1.,0.7,1.))\
         REPRESENTATION_ITEM(''))",
    ));
    assert!(
        message.contains("where a CARTESIAN_POINT should be"),
        "{message}"
    );
    assert!(message.contains("B_SPLINE_CURVE"), "{message}");
    // A surface swept from something that is no curve.
    let message = import_error(&with_params(&text, "CYLINDRICAL_SURFACE", "X").replace(
        "=CYLINDRICAL_SURFACE(X)",
        "=SURFACE_OF_REVOLUTION('',#1,#2)",
    ));
    assert!(
        message.contains("a curve of a kind PeetCAD can't import yet"),
        "{message}"
    );

    // Surface bodies and faceted bodies.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let shell = w.add(format!("OPEN_SHELL('',{})", refs(&faces[..5])));
    let model = w.add(format!("SHELL_BASED_SURFACE_MODEL('',(#{shell}))"));
    let surfaces_only = w.part(&[model], UnitStyle::Millimetres);
    assert!(import_error(&surfaces_only).contains("surface bodies"));
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let shell = w.add(format!("OPEN_SHELL('',{})", refs(&faces[..5])));
    let model = w.add(format!("SHELL_BASED_SURFACE_MODEL('',(#{shell}))"));
    let more = box_faces(
        &mut w,
        DVec3::splat(5.0),
        DVec3::splat(6.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Cube", &more);
    let imported = import(&w.part(&[model, brep], UnitStyle::Millimetres));
    assert_eq!(imported.bodies.len(), 1);
    assert!(
        imported.warnings[0].contains("surface body was skipped"),
        "{:?}",
        imported.warnings
    );
    // A brep over an open shell.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let shell = w.add(format!("OPEN_SHELL('',{})", refs(&faces)));
    let brep = w.add(format!("MANIFOLD_SOLID_BREP('',#{shell})"));
    assert!(import_error(&w.part(&[brep], UnitStyle::Millimetres)).contains("open shell"));
    // A box with a face missing is not a solid.
    let mut w = Step::default();
    let faces = box_faces(
        &mut w,
        DVec3::ZERO,
        v3(1.0, 2.0, 3.0),
        false,
        false,
        NO_HOLES,
    );
    let brep = w.solid("Lidless", &faces[..5]);
    let message = import_error(&w.part(&[brep], UnitStyle::Millimetres));
    assert!(
        message.contains("\"Lidless\"") && message.contains("not a closed, valid solid"),
        "{message}"
    );
}

#[test]
fn hostile_nesting_and_sizes() {
    // Parentheses nested far too deep must not overflow the stack.
    let deep = format!(
        "ISO-10303-21;HEADER;ENDSEC;DATA;#1=A({}{});ENDSEC;END-ISO-10303-21;",
        "(".repeat(200_000),
        ")".repeat(200_000)
    );
    assert!(import_error(&deep).contains("nested too deeply"));
    let deep = format!(
        "ISO-10303-21;HEADER;ENDSEC;DATA;#1=A({}",
        "B(".repeat(200_000)
    );
    import_error(&deep);
    // Entity numbers and numbers that don't fit.
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#99999999999999999999=A();ENDSEC;");
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#1=A(1E999);ENDSEC;");
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#=A();ENDSEC;");
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#1=();ENDSEC;");
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#1=A(.T);ENDSEC;");
    import_error("ISO-10303-21;HEADER;ENDSEC;DATA;#1=A(~);ENDSEC;");
}

/// A small deterministic generator (an LCG), so the mutations are the same every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn mutated_files_never_panic() {
    // Files of every kind the importer has special handling for.
    let mut ball = Sketch::new();
    ball.add_arc(DVec2::ZERO, v2(0.0, -5.0), v2(0.0, 5.0));
    ball.add_line(v2(0.0, 5.0), v2(0.0, -5.0));
    let ball = turned(&ball, v2(2.0, 0.0), 0.0, TAU);
    let cone = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(0.0, 10.0)]);
    let cone = turned(&cone, v2(1.0, 1.0), 0.0, TAU);
    let mut ring = Sketch::new();
    ring.add_circle(v2(10.0, 0.0), 3.0);
    let ring = turned(&ring, v2(10.5, 0.5), 0.0, 4.0);
    let top = Plane {
        frame: Frame {
            origin: v3(0.0, 0.0, 10.0),
            rotation: DQuat::from_rotation_z(0.5),
        },
    };
    let twisted = lofted(&[
        (plane_at(0.0), rectangle(12.0, 8.0)),
        (top, rectangle(12.0, 8.0)),
    ]);
    let mut w = Step::default();
    let style = Foreign {
        backwards: true,
        ..PLAIN
    };
    let brep = foreign_brep(&mut w, "Half", &half_disc(), &style);
    let rational = w.part(&[brep], UnitStyle::Inches);
    let sources = [
        exported_plate(),
        step::write(
            &[("Ball", &ball), ("Cone", &cone), ("Ring", &ring)],
            &options(StepSchema::Ap242),
        ),
        seamless_cylinder(1.0, UnitStyle::Inches),
        // Freeform faces and edges: PeetCAD's own, other exporters', closed surfaces
        // that are cut and trimmed, swept surfaces.
        step::write(&[("Loft", &twisted)], &options(StepSchema::Ap214)),
        rational,
        spline_tube(Tube::Seam, 2.6, false).0,
        spline_tube(Tube::Halves, 1.3, true).0,
        spline_torus(10.0, 3.0),
        turned_vase(),
    ];
    let mut random = Random(0x5EED);
    let (mut ok, mut failed) = (0, 0);
    for source in &sources {
        let bytes = source.as_bytes();
        // The files with freeform faces take longer to read: fewer rounds each.
        let rounds = if source.contains("B_SPLINE") {
            150
        } else {
            1000
        };
        for round in 0..rounds {
            let mut out = bytes.to_vec();
            for _ in 0..1 + random.below(3) {
                let at = random.below(out.len());
                let len = 1 + random.below(if round % 4 == 0 { 200 } else { 6 });
                let end = (at + len).min(out.len());
                match random.below(5) {
                    // Delete a stretch.
                    0 => {
                        out.drain(at..end);
                    }
                    // Duplicate a stretch.
                    1 => {
                        let copy = out[at..end].to_vec();
                        let to = random.below(out.len());
                        out.splice(to..to, copy);
                    }
                    // Change a byte to another printable one.
                    2 => out[at] = b' ' + (random.below(95) as u8),
                    // Swap a digit for another: other references and other numbers.
                    3 => {
                        if let Some(i) = (at..out.len()).find(|&i| out[i].is_ascii_digit()) {
                            out[i] = b'0' + random.below(10) as u8;
                        }
                    }
                    // Flip a flag.
                    _ => {
                        if let Some(i) = (at..out.len().saturating_sub(2))
                            .find(|&i| out[i] == b'.' && matches!(out[i + 1], b'T' | b'F'))
                        {
                            out[i + 1] = if out[i + 1] == b'T' { b'F' } else { b'T' };
                        }
                    }
                }
            }
            let text = String::from_utf8_lossy(&out);
            match read(&text) {
                Ok(imported) => {
                    // Whatever comes back is a valid solid.
                    for body in &imported.bodies {
                        valid(&body.solid);
                    }
                    ok += 1;
                }
                Err(e) => {
                    assert!(!e.message.is_empty());
                    failed += 1;
                }
            }
        }
    }
    // Both happen: mutations in names and unused entities are harmless.
    assert!(ok > 0 && failed > 0, "{ok} read, {failed} refused");
}
