//! Phase 2 exit criterion: sketch → extrude → sketch on a face → cut, with valid topology.

use peet_kernel::validate::validate;
use peet_kernel::{FaceId, Solid, Surface};
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{EndCondition, Extrude, Operation, apply_extrude, face_sketch_plane};
use peet_sketch::{Sketch, shapes};

/// The planar face whose outward normal is `n` and which lies furthest along it.
fn face_facing(solid: &Solid, n: DVec3) -> FaceId {
    solid
        .face_ids()
        .filter(|f| {
            let face = solid.face(*f);
            matches!(face.surface, Surface::Plane(_))
                && solid.face_normal_at(*f, DVec3::ZERO).dot(n) > 0.999
        })
        .max_by(|a, b| {
            let d = |f: FaceId| match solid.face(f).surface {
                Surface::Plane(p) => p.origin().dot(n),
                _ => f64::MIN,
            };
            d(*a).total_cmp(&d(*b))
        })
        .expect("a face facing that way")
}

fn assert_valid(solid: &Solid) -> peet_kernel::validate::Counts {
    match validate(solid) {
        Ok(c) => c,
        Err(problems) => panic!("invalid solid: {problems:#?}"),
    }
}

/// Checks the exact volume against a hand-calculated value, and that the display mesh
/// is closed and encloses the same volume (within the tessellation tolerance).
fn assert_volume(solid: &Solid, expected: f64) {
    let exact = peet_kernel::validate::measure::volume(solid);
    assert!(
        (exact - expected).abs() < 1e-6,
        "volume {exact} vs {expected}"
    );
    let mesh = peet_kernel::tessellate::tessellate(solid, 0.01).expect("tessellates");
    let mut mesh_volume = 0.0;
    for f in &mesh.faces {
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            mesh_volume += a.dot(b.cross(c)) / 6.0;
        }
    }
    assert!(
        (mesh_volume - expected).abs() < expected * 1e-3,
        "mesh volume {mesh_volume} vs {expected}"
    );
    assert_eq!(mesh.faces.len(), solid.faces.len());
}

#[test]
fn plate_with_pocket_and_through_hole() {
    // 1. A 100 × 60 rectangle on the Top plane, extruded 10 mm up.
    let mut base = Sketch::new();
    shapes::rectangle(&mut base, DVec2::new(-50.0, -30.0), DVec2::new(50.0, 30.0));
    let mut bodies = Vec::new();
    let mut ex = Extrude::new(Operation::Add);
    ex.depth = 10.0;
    apply_extrude(&mut bodies, &Plane::TOP, &base, &ex).expect("base extrude");
    assert_eq!(bodies.len(), 1);
    let counts = assert_valid(&bodies[0]);
    assert_eq!((counts.vertices, counts.edges, counts.faces), (8, 12, 6));
    let pi = std::f64::consts::PI;
    let mut volume = 100.0 * 60.0 * 10.0;
    assert_volume(&bodies[0], volume);

    // 2. Sketch on the top face: a 40 × 20 pocket, 4 mm deep.
    let top = face_facing(&bodies[0], DVec3::Z);
    let plane = face_sketch_plane(&bodies[0], top).expect("planar face");
    assert!((plane.origin().z - 10.0).abs() < 1e-9);
    let mut pocket = Sketch::new();
    shapes::rectangle(
        &mut pocket,
        DVec2::new(-20.0, -10.0),
        DVec2::new(20.0, 10.0),
    );
    let mut cut = Extrude::new(Operation::Cut);
    cut.depth = 4.0;
    apply_extrude(&mut bodies, &plane, &pocket, &cut).expect("pocket");
    assert_eq!(bodies.len(), 1);
    let counts = assert_valid(&bodies[0]);
    assert_eq!(counts.faces, 11, "6 box faces + pocket floor and 4 walls");
    assert_eq!(counts.genus, 0);
    volume -= 40.0 * 20.0 * 4.0;
    assert_volume(&bodies[0], volume);

    // 3. Sketch on the pocket floor: a through-all hole.
    let floor = face_at(&bodies[0], DVec3::Z, DVec3::new(0.0, 0.0, 6.0));
    let plane = face_sketch_plane(&bodies[0], floor).expect("planar");
    let mut hole = Sketch::new();
    hole.add_circle(DVec2::ZERO, 5.0);
    let mut through = Extrude::new(Operation::Cut);
    through.end = EndCondition::ThroughAll;
    apply_extrude(&mut bodies, &plane, &hole, &through).expect("hole");
    let counts = assert_valid(&bodies[0]);
    assert_eq!(counts.genus, 1, "a through hole makes a handle");
    assert_eq!(counts.faces, 12);
    volume -= pi * 25.0 * 6.0;
    assert_volume(&bodies[0], volume);

    // 4. Adding a boss on the bottom face grows the same body.
    let bottom = face_facing(&bodies[0], -DVec3::Z);
    let plane = face_sketch_plane(&bodies[0], bottom).expect("planar");
    let mut boss = Sketch::new();
    boss.add_circle(DVec2::new(30.0, 0.0), 8.0);
    apply_extrude(&mut bodies, &plane, &boss, &Extrude::new(Operation::Add)).expect("boss");
    assert_eq!(bodies.len(), 1);
    assert_valid(&bodies[0]);
    volume += pi * 64.0 * 10.0;
    assert_volume(&bodies[0], volume);
}

#[test]
fn failures_explain_themselves() {
    let mut bodies = Vec::new();
    let mut open = Sketch::new();
    open.add_line(DVec2::ZERO, DVec2::X * 10.0);
    let err = apply_extrude(
        &mut bodies,
        &Plane::TOP,
        &open,
        &Extrude::new(Operation::Add),
    )
    .unwrap_err();
    assert!(err.0.contains("closed"), "{err}");

    let mut square = Sketch::new();
    shapes::rectangle(&mut square, DVec2::ZERO, DVec2::ONE * 10.0);
    let err = apply_extrude(
        &mut bodies,
        &Plane::TOP,
        &square,
        &Extrude::new(Operation::Cut),
    )
    .unwrap_err();
    assert!(err.0.contains("doesn't reach"), "{err}");
}

/// The planar face with outward normal `n` whose plane passes through `through`.
fn face_at(solid: &Solid, n: DVec3, through: DVec3) -> FaceId {
    solid
        .face_ids()
        .find(|f| match solid.face(*f).surface {
            Surface::Plane(p) => {
                solid.face_normal_at(*f, through).dot(n) > 0.999
                    && p.signed_distance(through).abs() < 1e-9
            }
            _ => false,
        })
        .expect("a face there")
}

#[test]
fn l_bracket_with_features_from_three_directions() {
    let pi = std::f64::consts::PI;
    // An L profile on the Front plane (sketch X = world X, sketch Y = world Z), 40 mm wide.
    let mut profile = Sketch::new();
    let pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(80.0, 0.0),
        DVec2::new(80.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(10.0, 60.0),
        DVec2::new(0.0, 60.0),
    ];
    for i in 0..pts.len() {
        profile.add_line(pts[i], pts[(i + 1) % pts.len()]);
    }
    let mut bodies = Vec::new();
    let mut ex = Extrude::new(Operation::Add);
    ex.end = EndCondition::Symmetric;
    ex.depth = 40.0;
    apply_extrude(&mut bodies, &Plane::front(), &profile, &ex).expect("L extrude");
    let mut volume = 1300.0 * 40.0;
    assert_valid(&bodies[0]);
    assert_volume(&bodies[0], volume);

    // Two through holes from the top of the horizontal leg (cylinders along Z).
    let step = face_at(&bodies[0], DVec3::Z, DVec3::new(40.0, 0.0, 10.0));
    let plane = face_sketch_plane(&bodies[0], step).unwrap();
    let mut holes = Sketch::new();
    holes.add_circle(DVec2::new(30.0, 0.0), 3.0);
    holes.add_circle(DVec2::new(60.0, 0.0), 3.0);
    let mut cut = Extrude::new(Operation::Cut);
    cut.end = EndCondition::ThroughAll;
    apply_extrude(&mut bodies, &plane, &holes, &cut).expect("holes");
    volume -= 2.0 * pi * 9.0 * 10.0;
    assert_eq!(assert_valid(&bodies[0]).genus, 2);
    assert_volume(&bodies[0], volume);

    // A blind rectangular pocket from the end face (cut along X).
    let end = face_at(&bodies[0], DVec3::X, DVec3::new(80.0, 0.0, 5.0));
    let plane = face_sketch_plane(&bodies[0], end).unwrap();
    assert!(plane.frame.x_axis().abs_diff_eq(DVec3::Y, 1e-12));
    assert!(plane.frame.y_axis().abs_diff_eq(DVec3::Z, 1e-12));
    let mut slot = Sketch::new();
    shapes::rectangle(&mut slot, DVec2::new(-5.0, 3.0), DVec2::new(5.0, 7.0));
    let mut cut = Extrude::new(Operation::Cut);
    cut.depth = 5.0;
    apply_extrude(&mut bodies, &plane, &slot, &cut).expect("end pocket");
    volume -= 10.0 * 4.0 * 5.0;
    assert_eq!(assert_valid(&bodies[0]).genus, 2);
    assert_volume(&bodies[0], volume);

    // A round boss on the back of the upright, up to a plane 15 mm away.
    let back = face_at(&bodies[0], -DVec3::X, DVec3::new(0.0, 0.0, 30.0));
    let plane = face_sketch_plane(&bodies[0], back).unwrap();
    let mut boss = Sketch::new();
    let center = plane.to_plane_coords(DVec3::new(0.0, 0.0, 40.0));
    boss.add_circle(center, 4.0);
    let mut add = Extrude::new(Operation::Add);
    add.end = EndCondition::UpTo(
        Plane::from_origin_normal_x(DVec3::new(-15.0, 0.0, 0.0), -DVec3::X, DVec3::Y).unwrap(),
    );
    apply_extrude(&mut bodies, &plane, &boss, &add).expect("boss");
    volume += pi * 16.0 * 15.0;
    assert_eq!(bodies.len(), 1);
    assert_valid(&bodies[0]);
    assert_volume(&bodies[0], volume);
}
