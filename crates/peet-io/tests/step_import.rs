//! STEP import: PeetCAD's own exports read back as the same solids, files written the way
//! other programs write them (inch units, edges against their curves, unmarked outer
//! bounds, periodic faces without seams, vertex loops, degrees) come out as valid kernel
//! solids, and broken files give errors rather than panics.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fmt::Write as _;

use peet_io::step::{self, StepOptions, StepSchema};
use peet_io::step_import::{StepImport, read};
use peet_kernel::boolean::{BooleanOp, boolean};
use peet_kernel::extrude::extrude;
use peet_kernel::revolve::{RevolveAxis, revolve};
use peet_kernel::validate::{Counts, measure, validate};
use peet_kernel::{Solid, Surface};
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
    assert_close(volume, measure::volume(solid));
    assert_close(total_area, area(solid));
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
            .map(|f| f.surface)
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
fn freeform_and_other_unsupported_geometry() {
    let text = exported_plate();
    let spline = "'',1,1,((#1,#2),(#3,#4)),.UNSPECIFIED.,.F.,.F.,.F.,(2,2),(2,2),(0.,1.),(0.,1.),\
                  .UNSPECIFIED.";
    let message = import_error(&with_params(&text, "CYLINDRICAL_SURFACE", "X").replace(
        "=CYLINDRICAL_SURFACE(X)",
        &format!("=B_SPLINE_SURFACE_WITH_KNOTS({spline})"),
    ));
    assert!(
        message.contains("freeform (NURBS) surfaces, which PeetCAD can't import yet"),
        "{message}"
    );
    assert!(message.contains("B_SPLINE_SURFACE_WITH_KNOTS") && message.contains("(entity #"));
    // A rational B-spline is a complex entity.
    let message = import_error(&with_params(&text, "CIRCLE", "X").replace(
        "=CIRCLE(X)",
        "=(BOUNDED_CURVE()B_SPLINE_CURVE(2,(#1,#2,#3),.UNSPECIFIED.,.F.,.F.)\
         B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.)CURVE()\
         GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE((1.,0.7,1.))\
         REPRESENTATION_ITEM(''))",
    ));
    assert!(message.contains("freeform (NURBS) curves"), "{message}");
    assert!(message.contains("B_SPLINE_CURVE"), "{message}");
    let message = import_error(&with_params(&text, "CYLINDRICAL_SURFACE", "X").replace(
        "=CYLINDRICAL_SURFACE(X)",
        "=SURFACE_OF_REVOLUTION('',#1,#2)",
    ));
    assert!(
        message.contains("a surface of a kind PeetCAD can't import yet"),
        "{message}"
    );
    assert!(message.contains("SURFACE_OF_REVOLUTION"), "{message}");

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
    let sources = [
        exported_plate(),
        step::write(
            &[("Ball", &ball), ("Cone", &cone), ("Ring", &ring)],
            &options(StepSchema::Ap242),
        ),
        seamless_cylinder(1.0, UnitStyle::Inches),
    ];
    let mut random = Random(0x5EED);
    let (mut ok, mut failed) = (0, 0);
    for source in &sources {
        let bytes = source.as_bytes();
        for round in 0..1000 {
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
