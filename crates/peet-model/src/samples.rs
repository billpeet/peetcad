//! Sample parts, built through the same API as the UI uses. They serve the exit-criterion
//! tests, the rebuild benchmark and "open a sample" in the app.

use peet_kernel::Surface;
use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::{ConstraintKind, Sketch, shapes};

use crate::feature::{FeatureKind, PlaneDef, PlaneRef, Scalar, StdPlane};
use crate::{EndCondition, Engine, FeatureId, Model, Operation};

/// A bracket with 20 features: a dimensioned base plate (its width is `d1` of Sketch1),
/// a pocket with a hole in its floor, four mounting holes, a rib on an offset plane with a
/// hole in its top, a slot in the end face, a boss underneath with a hole through it, and
/// a reference plane. Sketches sit on faces of earlier features wherever that makes sense,
/// so a change upstream exercises persistent naming everywhere.
pub fn bracket() -> (Model, Engine) {
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Bracket".to_owned();
    let top_plane = PlaneRef::Standard(StdPlane::Top);
    let base = b.sketch(top_plane, |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(120.0, 80.0));
        let (bottom, right) = (shape.curves[0], shape.curves[1]);
        let corner = s.endpoints(bottom).expect("a line").0;
        let _ = s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN));
        let _ = s.add_dimension(ConstraintKind::Length(bottom), 120.0);
        let _ = s.add_dimension(ConstraintKind::Length(right), 80.0);
    });
    b.extrude(base, Operation::Add, |e| e.depth = Scalar::new(8.0));

    let top = b.face(DVec3::Z, DVec3::new(1.0, 1.0, 8.0));
    let pocket = b.sketch(top.clone(), |s| {
        shapes::rectangle(s, DVec2::new(20.0, 20.0), DVec2::new(60.0, 60.0));
    });
    b.extrude(pocket, Operation::Cut, |e| e.depth = Scalar::new(3.0));

    let floor = b.face(DVec3::Z, DVec3::new(40.0, 40.0, 5.0));
    let drain = b.sketch(floor, |s| {
        s.add_circle(DVec2::new(40.0, 40.0), 6.0);
    });
    b.extrude(drain, Operation::Cut, |e| e.end = EndCondition::ThroughAll);

    let holes = b.sketch(top, |s| {
        for (x, y) in [(80.0, 15.0), (110.0, 15.0), (80.0, 65.0), (110.0, 65.0)] {
            s.add_circle(DVec2::new(x, y), 3.0);
        }
    });
    b.extrude(holes, Operation::Cut, |e| e.end = EndCondition::ThroughAll);

    let rib_plane = b.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Front),
        distance: Scalar::new(40.0),
        flip: true,
    }));
    let rib = b.sketch(PlaneRef::Feature(rib_plane), |s| {
        shapes::rectangle(s, DVec2::new(70.0, 8.0), DVec2::new(110.0, 28.0));
    });
    b.extrude(rib, Operation::Add, |e| {
        e.end = EndCondition::Symmetric;
        e.depth = Scalar::new(6.0);
    });

    let rib_top = b.face(DVec3::Z, DVec3::new(90.0, 40.0, 28.0));
    let rib_hole = b.sketch(rib_top, |s| {
        s.add_circle(DVec2::new(90.0, 40.0), 2.0);
    });
    b.extrude(rib_hole, Operation::Cut, |e| e.depth = Scalar::new(10.0));

    let end = b.face(DVec3::X, DVec3::new(120.0, 1.0, 1.0));
    let slot = b.sketch(end, |s| {
        shapes::rectangle(s, DVec2::new(30.0, 2.0), DVec2::new(50.0, 6.0));
    });
    b.extrude(slot, Operation::Cut, |e| e.depth = Scalar::new(15.0));

    let bottom = b.face(-DVec3::Z, DVec3::new(1.0, 1.0, 0.0));
    let boss = b.sketch(bottom, |s| {
        s.add_circle(DVec2::new(15.0, -70.0), 5.0);
    });
    b.extrude(boss, Operation::Add, |e| e.depth = Scalar::new(5.0));

    let boss_end = b.face(-DVec3::Z, DVec3::new(15.0, 70.0, -5.0));
    let boss_hole = b.sketch(boss_end, |s| {
        s.add_circle(DVec2::new(15.0, -70.0), 2.0);
    });
    b.extrude(boss_hole, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll
    });

    let top = b.face(DVec3::Z, DVec3::new(1.0, 1.0, 8.0));
    b.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: top,
        distance: Scalar::new(20.0),
        flip: false,
    }));
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// A sheet metal enclosure panel (the Phase 4 exit criterion part): a 200 × 150 plate,
/// 1.5 mm thick (the `thickness` parameter), with a 25 mm flange on each edge set back
/// 10 mm from the corners (with reliefs), an 80 × 40 window, four Ø5 mounting holes, a
/// slot running across the right-hand bend and a Ø8 hole in the front flange.
pub fn enclosure() -> (Model, Engine) {
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Enclosure Panel".to_owned();
    let _ = b.model.parameters.set("thickness", "1.5mm");
    let _ = b.model.parameters.set("flange", "25mm");
    let base = b.sketch(PlaneRef::Standard(StdPlane::Top), |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(200.0, 150.0));
        let (bottom, right) = (shape.curves[0], shape.curves[1]);
        let corner = s.endpoints(bottom).expect("a line").0;
        let _ = s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN));
        let _ = s.add_dimension(ConstraintKind::Length(bottom), 200.0);
        let _ = s.add_dimension(ConstraintKind::Length(right), 150.0);
    });
    let flange = b.model.add_base_flange(base);
    if let Some(f) = b.model.feature_mut(flange)
        && let FeatureKind::BaseFlange(def) = &mut f.kind
    {
        def.settings.thickness = Scalar {
            value: 1.5,
            expression: Some("thickness".to_owned()),
        };
        def.settings.radius = Scalar::new(2.0);
    }
    let t = 1.5;
    let corners = [
        DVec3::new(0.0, 0.0, t),
        DVec3::new(200.0, 0.0, t),
        DVec3::new(200.0, 150.0, t),
        DVec3::new(0.0, 150.0, t),
    ];
    for i in 0..4 {
        let edge = b.edge(corners[i], corners[(i + 1) % 4]);
        let id = b.model.add_edge_flange(Some(edge));
        if let Some(f) = b.model.feature_mut(id)
            && let FeatureKind::EdgeFlange(e) = &mut f.kind
        {
            e.length = Scalar {
                value: 25.0,
                expression: Some("flange".to_owned()),
            };
            e.offset_start = Scalar::new(10.0);
            e.offset_end = Scalar::new(10.0);
        }
    }
    let top = b.face(DVec3::Z, DVec3::new(50.0, 50.0, t));
    let cutouts = b.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(60.0, 55.0), DVec2::new(140.0, 95.0));
        for (x, y) in [(20.0, 20.0), (180.0, 20.0), (20.0, 130.0), (180.0, 130.0)] {
            s.add_circle(DVec2::new(x, y), 2.5);
        }
        shapes::rectangle(s, DVec2::new(190.0, 70.0), DVec2::new(210.0, 80.0));
    });
    b.model.add_sheet_cut(cutouts);
    let front = b.face(-DVec3::Y, DVec3::new(100.0, 0.0, 15.0));
    let hole = b.sketch(front, |s| {
        s.add_circle(DVec2::new(100.0, 15.0), 4.0);
    });
    b.model.add_sheet_cut(hole);
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// The sizes of the [`chassis`] sample, for tests and hand calculations.
pub mod chassis_size {
    pub const WIDTH: f64 = 240.0;
    pub const DEPTH: f64 = 160.0;
    pub const THICKNESS: f64 = 1.5;
    pub const RADIUS: f64 = 1.5;
    pub const K: f64 = 0.44;
    /// Height of the walls, to the top of the rim.
    pub const WALL: f64 = 40.0;
    /// Width of the rim, from the outside of the wall.
    pub const LIP: f64 = 12.0;
    /// Length of the hem on the front edge.
    pub const HEM: f64 = 10.0;
    pub const HOLE_RADIUS: f64 = 2.25;
    pub const LOUVER: (f64, f64, f64) = (40.0, 8.0, 3.0);
    pub const DIMPLE: (f64, f64) = (5.0, 2.5);
    pub const WINDOW: (f64, f64) = (30.0, 12.0);
}

/// A sheet metal chassis tray (the Phase 5 exit criterion part): a 240 × 160 base,
/// 1.5 mm thick, with a mitre flange round the right, back and left edges (a 40 mm wall
/// and a 12 mm rim turned in, mitred at the back corners), a closed hem on the front
/// edge, four Ø4.5 mounting holes (one cut, patterned in a grid), ten louvers (one,
/// patterned in a grid), two dimples (one, mirrored) and a window in the back wall.
pub fn chassis() -> (Model, Engine) {
    use chassis_size as cs;
    use peet_sheetmetal::FormKind;

    use crate::feature::{AxisRef, LinearDirection, PatternDef, StdAxis};

    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Chassis".to_owned();
    let _ = b.model.parameters.set("thickness", "1.5mm");
    let (w, d, t) = (cs::WIDTH, cs::DEPTH, cs::THICKNESS);
    let base = b.sketch(PlaneRef::Standard(StdPlane::Top), |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(w, d));
        let (bottom, right) = (shape.curves[0], shape.curves[1]);
        let corner = s.endpoints(bottom).expect("a line").0;
        let _ = s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN));
        let _ = s.add_dimension(ConstraintKind::Length(bottom), w);
        let _ = s.add_dimension(ConstraintKind::Length(right), d);
    });
    let flange = b.model.add_base_flange(base);
    if let Some(f) = b.model.feature_mut(flange)
        && let FeatureKind::BaseFlange(def) = &mut f.kind
    {
        def.settings.thickness = Scalar {
            value: t,
            expression: Some("thickness".to_owned()),
        };
        def.settings.radius = Scalar::new(cs::RADIUS);
    }

    // The rim: a wall and a lip, drawn on the base's front end face at the left-hand
    // corner (which stays where it is when the base is made wider, so the rim follows
    // the width), run along the left, back and right edges.
    let end = b.face(-DVec3::Y, DVec3::new(w / 2.0, 0.0, t / 2.0));
    let profile = b.sketch_at(end, |s, to| {
        let p0 = to(DVec3::new(0.0, 0.0, 0.0));
        let p1 = to(DVec3::new(0.0, 0.0, cs::WALL));
        let p2 = to(DVec3::new(cs::LIP, 0.0, cs::WALL));
        let wall = s.add_line(p0, p1);
        let lip = s.add_line(p1, p2);
        if let (Some((_, wall_end)), Some((lip_start, _))) = (s.endpoints(wall), s.endpoints(lip)) {
            let _ = s.add_constraint(ConstraintKind::Coincident(wall_end, lip_start));
        }
        let _ = s.add_dimension(ConstraintKind::Length(wall), cs::WALL);
        let _ = s.add_dimension(ConstraintKind::Length(lip), cs::LIP);
    });
    let corners = [
        DVec3::new(0.0, 0.0, t),
        DVec3::new(0.0, d, t),
        DVec3::new(w, d, t),
        DVec3::new(w, 0.0, t),
    ];
    let edges = (0..3).map(|i| b.edge(corners[i], corners[i + 1])).collect();
    b.model.add_miter_flange(profile, edges);

    // A closed hem on the front edge, which now runs between the two walls' bends.
    let inset = (cs::RADIUS + t) * 1.0; // the outside setback of a square bend
    let front = b.edge(DVec3::new(inset, 0.0, t), DVec3::new(w - inset, 0.0, t));
    let hem = b.model.add_hem(Some(front));
    if let Some(f) = b.model.feature_mut(hem)
        && let FeatureKind::Hem(h) = &mut f.kind
    {
        h.length = Scalar::new(cs::HEM);
    }

    // Mounting holes: one, patterned to the four corners.
    let top = b.face(DVec3::Z, DVec3::new(w / 2.0, d / 2.0, t));
    let hole = b.sketch_at(top.clone(), |s, to| {
        s.add_circle(to(DVec3::new(20.0, 25.0, t)), cs::HOLE_RADIUS);
    });
    let hole_cut = b.model.add_sheet_cut(hole);
    let grid = |x: f64, nx: u32, y: f64, ny: u32| PatternDef::Linear {
        first: LinearDirection {
            direction: AxisRef::Standard(StdAxis::X),
            spacing: Scalar::new(x),
            count: Scalar::new(f64::from(nx)),
            flip: false,
        },
        second: Some(LinearDirection {
            direction: AxisRef::Standard(StdAxis::Y),
            spacing: Scalar::new(y),
            count: Scalar::new(f64::from(ny)),
            flip: false,
        }),
    };
    b.model
        .add_pattern(vec![hole_cut], grid(200.0, 2, 110.0, 2));

    // Louvers: one, patterned into two columns of five.
    let (ll, lw, lh) = cs::LOUVER;
    let louver_sketch = b.sketch_at(top.clone(), |s, to| {
        let a = to(DVec3::new(60.0, 50.0, t));
        let c = to(DVec3::new(60.0 + ll, 50.0 + lw, t));
        shapes::rectangle(s, a.min(c), a.max(c));
    });
    let louver = b.model.add_form(louver_sketch, FormKind::Louver);
    if let Some(f) = b.model.feature_mut(louver)
        && let FeatureKind::Form(def) = &mut f.kind
    {
        def.height = Scalar::new(lh);
    }
    b.model.add_pattern(vec![louver], grid(80.0, 2, 14.0, 5));

    // Stand-off dimples: one, mirrored across the middle of the tray.
    let (dr, dh) = cs::DIMPLE;
    let dimple_sketch = b.sketch_at(top, |s, to| {
        s.add_circle(to(DVec3::new(30.0, 80.0, t)), dr);
    });
    let dimple = b.model.add_form(dimple_sketch, FormKind::Dimple);
    if let Some(f) = b.model.feature_mut(dimple)
        && let FeatureKind::Form(def) = &mut f.kind
    {
        def.height = Scalar::new(dh);
    }
    let middle = b.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Right),
        distance: Scalar::new(w / 2.0),
        flip: false,
    }));
    b.model.add_mirror(vec![dimple], PlaneRef::Feature(middle));

    // A window in the back wall, sketched on its outer face.
    let back = b.face(DVec3::Y, DVec3::new(w / 2.0, d, cs::WALL / 2.0));
    let (ww, wh) = cs::WINDOW;
    let window = b.sketch_at(back, |s, to| {
        let a = to(DVec3::new(w / 2.0 - ww / 2.0, d, 14.0));
        let c = to(DVec3::new(w / 2.0 + ww / 2.0, d, 14.0 + wh));
        shapes::rectangle(s, a.min(c), a.max(c));
    });
    b.model.add_sheet_cut(window);
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// The bracket's exact volume for a base plate `width` wide.
/// Sizes of [`housing`], for its tests.
pub mod housing_size {
    /// Radius of the flange, the hub and the bore.
    pub const FLANGE_R: f64 = 50.0;
    pub const HUB_R: f64 = 28.0;
    pub const BORE_R: f64 = 15.0;
    /// Thickness of the flange and overall height.
    pub const FLANGE_T: f64 = 12.0;
    pub const HEIGHT: f64 = 40.0;
    /// The fillet at the foot of the hub and the chamfers on its top rims.
    pub const FILLET: f64 = 4.0;
    pub const CHAMFER: f64 = 1.5;
    /// The bolt circle: six counterbored holes for M8 cap screws.
    pub const BOLT_CIRCLE_R: f64 = 40.0;
    pub const BOLTS: u32 = 6;
}

/// A turned bearing housing: a flange and a hub revolved from one half section, the
/// foot of the hub filleted, the top rims chamfered, and a bolt circle of counterbored
/// holes patterned round the axis. The Phase 6 exit part.
pub fn housing() -> (Model, Engine) {
    use housing_size as hs;
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Housing".to_owned();
    // Half the section on the front plane: sketch x is the radius, y the height.
    let section = b.sketch(PlaneRef::Standard(StdPlane::Front), |s| {
        let pts = [
            DVec2::new(hs::BORE_R, 0.0),
            DVec2::new(hs::FLANGE_R, 0.0),
            DVec2::new(hs::FLANGE_R, hs::FLANGE_T),
            DVec2::new(hs::HUB_R, hs::FLANGE_T),
            DVec2::new(hs::HUB_R, hs::HEIGHT),
            DVec2::new(hs::BORE_R, hs::HEIGHT),
        ];
        for i in 0..pts.len() {
            s.add_line(pts[i], pts[(i + 1) % pts.len()]);
        }
        let axis = s.add_line(DVec2::new(0.0, -10.0), DVec2::new(0.0, hs::HEIGHT + 10.0));
        s.set_construction(axis, true);
    });
    b.model.add_revolve(section, Operation::NewBody);

    let foot = b.circle_edge(hs::HUB_R, hs::FLANGE_T);
    let fillet = b.model.add_blend(crate::BlendKind::Fillet, vec![foot]);
    if let Some(FeatureKind::Blend(f)) = b.model.feature_mut(fillet).map(|f| &mut f.kind) {
        f.size = Scalar::new(hs::FILLET);
    }
    let rims = vec![
        b.circle_edge(hs::HUB_R, hs::HEIGHT),
        b.circle_edge(hs::BORE_R, hs::HEIGHT),
    ];
    let chamfer = b.model.add_blend(crate::BlendKind::Chamfer, rims);
    if let Some(FeatureKind::Blend(c)) = b.model.feature_mut(chamfer).map(|f| &mut f.kind) {
        c.size = Scalar::new(hs::CHAMFER);
    }

    let top = b.face(DVec3::Z, DVec3::new(hs::BOLT_CIRCLE_R, 0.0, hs::FLANGE_T));
    let at = b.sketch_at(top, |s, to| {
        s.add_point(to(DVec3::new(hs::BOLT_CIRCLE_R, 0.0, hs::FLANGE_T)));
    });
    let hole = b.model.add_hole(at);
    if let Some(FeatureKind::Hole(h)) = b.model.feature_mut(hole).map(|f| &mut f.kind) {
        let m8 = crate::METRIC
            .iter()
            .find(|s| s.name == "M8")
            .expect("M8 is in the table");
        h.set_standard(m8, crate::HoleFit::Normal);
        h.kind = crate::HoleKind::Counterbore;
    }
    b.model.add_pattern(
        vec![hole],
        crate::PatternDef::Circular {
            axis: crate::AxisRef::Standard(crate::StdAxis::Z),
            count: Scalar::new(f64::from(hs::BOLTS)),
            angle: Scalar::new(360.0),
            flip: false,
        },
    );
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// Volume of [`housing`], from its sizes.
pub fn housing_volume() -> f64 {
    use housing_size as hs;
    use std::f64::consts::{PI, TAU};
    let bore2 = hs::BORE_R * hs::BORE_R;
    let turned = PI
        * ((hs::FLANGE_R * hs::FLANGE_R - bore2) * hs::FLANGE_T
            + (hs::HUB_R * hs::HUB_R - bore2) * (hs::HEIGHT - hs::FLANGE_T));
    // Pappus: a section's area times the distance its centroid travels. A fillet's
    // sliver (a square less a quarter disc) has its centroid this far from its corner.
    let r = hs::FILLET;
    let sliver = r * r * (1.0 - PI / 4.0);
    let sliver_centroid = r * (5.0 / 6.0 - PI / 4.0) / (1.0 - PI / 4.0);
    let fillet = TAU * (hs::HUB_R + sliver_centroid) * sliver;
    let d = hs::CHAMFER;
    let chamfers = TAU * (hs::HUB_R - d / 3.0 + hs::BORE_R + d / 3.0) * (d * d / 2.0);
    // M8, normal clearance: a Ø9 hole with a Ø15 counterbore 8.6 deep.
    let hole = PI * (7.5 * 7.5 * 8.6 + 4.5 * 4.5 * (hs::FLANGE_T - 8.6));
    turned + fillet - chamfers - f64::from(hs::BOLTS) * hole
}

pub fn bracket_volume(width: f64) -> f64 {
    let pi = std::f64::consts::PI;
    width * 80.0 * 8.0 // plate
        - 40.0 * 40.0 * 3.0 // pocket
        - pi * 36.0 * 5.0 // hole in the pocket floor
        - 4.0 * pi * 9.0 * 8.0 // mounting holes
        + 40.0 * 6.0 * 20.0 // rib
        - pi * 4.0 * 10.0 // hole in the rib
        - 20.0 * 4.0 * 15.0 // slot in the end face
        + pi * 25.0 * 5.0 // boss
        - pi * 4.0 * 13.0 // hole through the boss and the plate
}

// ---- The Phase 7 exit assembly ----

/// Sizes of the parts made for [`enclosure_assembly`], for its tests.
pub mod enclosure_size {
    /// The cover: a flat sheet the size of the chassis, with an opening for the shaft
    /// and holes for the housing's bolts.
    pub const COVER_T: f64 = 1.5;
    pub const COVER_OPENING_R: f64 = 16.0;
    pub const COVER_HOLE_R: f64 = 4.5;
    /// A socket head screw: (thread radius, length under the head, head radius, head
    /// height, socket across flats, socket depth).
    pub const M8: (f64, f64, f64, f64, f64, f64) = (4.0, 8.0, 6.5, 8.0, 6.0, 4.0);
    pub const M4: (f64, f64, f64, f64, f64, f64) = (2.0, 8.0, 3.5, 4.0, 3.0, 2.0);
    /// Where the housing's axis is on the cover, from the cover's corner.
    pub const HOUSING_AT: (f64, f64) = (120.0, 80.0);
    /// The chassis's mounting holes: the first, and the steps to the others.
    pub const MOUNT_AT: (f64, f64) = (20.0, 25.0);
    pub const MOUNT_STEP: (f64, f64) = (200.0, 110.0);
    /// Densities, kg/m³.
    pub const STEEL: f64 = 7850.0;
    pub const ALUMINIUM: f64 = 2700.0;
}

/// The cover of [`enclosure_assembly`]: a sheet of the chassis's size and thickness
/// with an opening for the housing's shaft and a hole for each of its bolts.
pub fn cover() -> (Model, Engine) {
    use chassis_size as cs;
    use enclosure_size as es;
    use housing_size as hs;
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Cover".to_owned();
    let base = b.sketch(PlaneRef::Standard(StdPlane::Top), |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(cs::WIDTH, cs::DEPTH));
        let (bottom, right) = (shape.curves[0], shape.curves[1]);
        let corner = s.endpoints(bottom).expect("a line").0;
        let _ = s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN));
        let _ = s.add_dimension(ConstraintKind::Length(bottom), cs::WIDTH);
        let _ = s.add_dimension(ConstraintKind::Length(right), cs::DEPTH);
    });
    let flange = b.model.add_base_flange(base);
    if let Some(f) = b.model.feature_mut(flange)
        && let FeatureKind::BaseFlange(def) = &mut f.kind
    {
        def.settings.thickness = Scalar::new(es::COVER_T);
    }
    let (cx, cy) = es::HOUSING_AT;
    let top = b.face(DVec3::Z, DVec3::new(1.0, 1.0, es::COVER_T));
    let holes = b.sketch_at(top, |s, to| {
        s.add_circle(to(DVec3::new(cx, cy, es::COVER_T)), es::COVER_OPENING_R);
        for i in 0..hs::BOLTS {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(hs::BOLTS);
            let at = DVec3::new(
                cx + hs::BOLT_CIRCLE_R * a.cos(),
                cy + hs::BOLT_CIRCLE_R * a.sin(),
                es::COVER_T,
            );
            s.add_circle(to(at), es::COVER_HOLE_R);
        }
    });
    b.model.add_sheet_cut(holes);
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// The cover's volume, from its sizes.
pub fn cover_volume() -> f64 {
    use chassis_size as cs;
    use enclosure_size as es;
    let pi = std::f64::consts::PI;
    let holes = pi * es::COVER_OPENING_R * es::COVER_OPENING_R
        + f64::from(housing_size::BOLTS) * pi * es::COVER_HOLE_R * es::COVER_HOLE_R;
    (cs::WIDTH * cs::DEPTH - holes) * es::COVER_T
}

/// A socket head screw standing on the top plane: its head above (its underside at
/// z = 0), its thread below, a hexagon socket in the top of the head. `size` is as
/// [`enclosure_size::M8`].
pub fn socket_screw(name: &str, size: (f64, f64, f64, f64, f64, f64)) -> (Model, Engine) {
    let (thread_r, length, head_r, head_h, socket, socket_depth) = size;
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    name.clone_into(&mut b.model.name);
    let head = b.sketch(PlaneRef::Standard(StdPlane::Top), |s| {
        s.add_circle(DVec2::ZERO, head_r);
    });
    b.extrude(head, Operation::Add, |e| e.depth = Scalar::new(head_h));
    let under = b.face(-DVec3::Z, DVec3::ZERO);
    let thread = b.sketch(under, |s| {
        s.add_circle(DVec2::ZERO, thread_r);
    });
    b.extrude(thread, Operation::Add, |e| e.depth = Scalar::new(length));
    let top = b.face(DVec3::Z, DVec3::new(0.0, 0.0, head_h));
    let hexagon = b.sketch_at(top, |s, to| {
        // Across flats `socket`: corners at this radius, a flat square to x.
        let r = socket / 3f64.sqrt();
        let corner = |i: u32| {
            let a = std::f64::consts::TAU * (f64::from(i) + 0.5) / 6.0;
            to(DVec3::new(r * a.cos(), r * a.sin(), head_h))
        };
        for i in 0..6 {
            s.add_line(corner(i), corner(i + 1));
        }
    });
    b.extrude(hexagon, Operation::Cut, |e| {
        e.depth = Scalar::new(socket_depth)
    });
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// A socket screw's volume, from its sizes.
pub fn socket_screw_volume(size: (f64, f64, f64, f64, f64, f64)) -> f64 {
    let (thread_r, length, head_r, head_h, socket, socket_depth) = size;
    let pi = std::f64::consts::PI;
    // A hexagon across flats s has the area (√3 / 2) s².
    pi * head_r * head_r * head_h + pi * thread_r * thread_r * length
        - 3f64.sqrt() / 2.0 * socket * socket * socket_depth
}

/// The faces of a built part, for mates.
struct Faces(std::sync::Arc<crate::Body>);

impl Faces {
    fn of(model: &Model) -> Self {
        let mut model = model.clone();
        let body = Engine::new()
            .regenerate(&mut model)
            .bodies
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("{} has no body", model.name));
        Self(body)
    }

    /// The flat face with the outward normal `n` whose plane goes through `at`.
    fn flat(&self, n: DVec3, at: DVec3) -> crate::MateGeom {
        let solid = &self.0.solid;
        let found = solid.face_ids().find(|f| {
            matches!(solid.face(*f).surface, Surface::Plane(p)
                if solid.face_normal_at(*f, at).dot(n) > 0.999 && p.signed_distance(at).abs() < 1e-9)
        });
        let found = found.unwrap_or_else(|| panic!("no flat face {n} through {at}"));
        crate::MateGeom::Face(self.0.face_ref(found))
    }

    /// The round face of this radius whose axis goes through `on`.
    fn round(&self, radius: f64, on: DVec3) -> crate::MateGeom {
        let solid = &self.0.solid;
        let found = solid.face_ids().find(|f| {
            matches!(&solid.face(*f).surface, Surface::Cylinder(c)
                if (c.radius - radius).abs() < 1e-9
                    && (on - c.axis_origin()).cross(c.axis()).length() < 1e-9)
        });
        let found = found.unwrap_or_else(|| panic!("no round face of radius {radius} at {on}"));
        crate::MateGeom::Face(self.0.face_ref(found))
    }
}

/// The Phase 7 exit assembly: the chassis with a cover on its rim, the bearing housing
/// bolted to the cover over an opening for its shaft, and screws in the chassis's
/// mounting holes. Thirteen components of five parts, each with a material. The
/// chassis is fixed and everything else is held by mates to it, to the cover or to
/// the housing: nothing is left free. The housing's six bolts are one bolt and a
/// circular pattern round the housing's axis; the four mounting screws one screw and
/// a pattern in two directions taken from the chassis's walls. Three explode steps take
/// it apart.
pub fn enclosure_assembly() -> (Model, Engine) {
    use std::sync::Arc;

    use chassis_size as cs;
    use enclosure_size as es;
    use housing_size as hs;
    use peet_math::Frame;

    use crate::{
        CompId, MateEnd, MateGeom, MateKind, Material, PatternKind, PatternLine, PatternStep,
    };

    let with = |mut part: Model, material: &str, density: f64| {
        part.material = Material::new(material, density).ok();
        Arc::new(part)
    };
    let chassis = with(chassis().0, "Mild steel", es::STEEL);
    let cover = with(cover().0, "Mild steel", es::STEEL);
    let housing = with(housing().0, "Aluminium 6061", es::ALUMINIUM);
    let bolt = with(socket_screw("Bolt M8", es::M8).0, "Alloy steel", es::STEEL);
    let screw = with(socket_screw("Screw M4", es::M4).0, "Alloy steel", es::STEEL);
    let faces = [&chassis, &cover, &housing, &bolt, &screw].map(|part| Faces::of(part));
    let [f_chassis, f_cover, f_housing, f_bolt, f_screw] = &faces;

    let mut model = Model::new_assembly();
    model.name = "Enclosure".to_owned();
    let at = |x: f64, y: f64, z: f64| Frame {
        origin: DVec3::new(x, y, z),
        ..Frame::WORLD
    };
    let (hx, hy) = es::HOUSING_AT;
    let (mx, my) = es::MOUNT_AT;
    let t = cs::THICKNESS;
    // The floor of the housing's counterbores: an M8 counterbore is 8.6 deep.
    let seat = hs::FLANGE_T - 8.6;
    let a = model.assembly_mut().expect("an assembly");
    // Near where the mates will have them: a solve goes to the nearest answer.
    let place = |a: &mut crate::Assembly, part: &Arc<Model>, frame, name: &str| -> CompId {
        let definition = a.define(part.clone());
        let id = a
            .insert(definition, frame)
            .expect("the part was just defined");
        if let Some(c) = a.component_mut(id) {
            name.clone_into(&mut c.name);
        }
        id
    };
    let c_chassis = place(a, &chassis, Frame::WORLD, "Chassis");
    let c_cover = place(a, &cover, at(0.0, 0.0, cs::WALL), "Cover");
    let c_housing = place(a, &housing, at(hx, hy, cs::WALL + es::COVER_T), "Housing");
    let c_bolt = place(
        a,
        &bolt,
        at(hx + hs::BOLT_CIRCLE_R, hy, cs::WALL + es::COVER_T + seat),
        "Bolt-1",
    );
    let c_screw = place(a, &screw, at(mx, my, t), "Screw-1");

    let end = |component: CompId, geom: MateGeom| MateEnd {
        path: vec![component],
        geom: Some(geom),
    };
    let mate = |a: &mut crate::Assembly, kind: MateKind, x: MateEnd, y: MateEnd, flip: bool| {
        let id = a.add_mate(kind, x, y);
        if let Some(m) = a.mate_mut(id) {
            m.flip = flip;
        }
    };
    let (x, y, z) = (DVec3::X, DVec3::Y, DVec3::Z);
    let right_wall = f_chassis.flat(x, DVec3::new(cs::WIDTH, cs::DEPTH / 2.0, cs::WALL / 2.0));
    let back_wall = f_chassis.flat(y, DVec3::new(cs::WIDTH / 2.0, cs::DEPTH, 4.0));

    // The cover: on the rim, flush with the right and the back walls.
    mate(
        a,
        MateKind::Coincident,
        end(
            c_chassis,
            f_chassis.flat(z, DVec3::new(cs::WIDTH - 6.0, cs::DEPTH / 2.0, cs::WALL)),
        ),
        end(c_cover, f_cover.flat(-z, DVec3::ZERO)),
        false,
    );
    mate(
        a,
        MateKind::Coincident,
        end(c_chassis, right_wall.clone()),
        end(c_cover, f_cover.flat(x, DVec3::new(cs::WIDTH, 1.0, 0.5))),
        true,
    );
    mate(
        a,
        MateKind::Coincident,
        end(c_chassis, back_wall.clone()),
        end(c_cover, f_cover.flat(y, DVec3::new(1.0, cs::DEPTH, 0.5))),
        true,
    );

    // The housing: on the cover, over the opening, a bolt hole over a hole.
    let hole = DVec3::new(hs::BOLT_CIRCLE_R, 0.0, 0.0);
    mate(
        a,
        MateKind::Coincident,
        end(c_cover, f_cover.flat(z, DVec3::new(1.0, 1.0, es::COVER_T))),
        end(
            c_housing,
            f_housing.flat(-z, DVec3::new(hs::FLANGE_R - 1.0, 0.0, 0.0)),
        ),
        false,
    );
    mate(
        a,
        MateKind::Concentric,
        end(
            c_cover,
            f_cover.round(es::COVER_OPENING_R, DVec3::new(hx, hy, 0.0)),
        ),
        end(c_housing, f_housing.round(hs::BORE_R, DVec3::ZERO)),
        false,
    );
    mate(
        a,
        MateKind::Concentric,
        end(
            c_cover,
            f_cover.round(es::COVER_HOLE_R, DVec3::new(hx, hy, 0.0) + hole),
        ),
        end(c_housing, f_housing.round(4.5, hole)),
        false,
    );

    // A screw: in its hole, its head down on its seat, and a flat of its socket square
    // to the right wall so that it can't spin.
    let fasten = |a: &mut crate::Assembly,
                  screw: CompId,
                  f: &Faces,
                  size: (f64, f64, f64, f64, f64, f64),
                  hole: MateEnd,
                  seat: MateEnd| {
        mate(
            a,
            MateKind::Concentric,
            hole,
            end(screw, f.round(size.0, DVec3::ZERO)),
            false,
        );
        mate(
            a,
            MateKind::Coincident,
            seat,
            end(screw, f.flat(-z, DVec3::ZERO)),
            false,
        );
        mate(
            a,
            MateKind::Parallel,
            end(c_chassis, right_wall.clone()),
            end(
                screw,
                f.flat(-x, DVec3::new(size.4 / 2.0, 0.0, size.3 - 0.5)),
            ),
            false,
        );
    };
    fasten(
        a,
        c_bolt,
        f_bolt,
        es::M8,
        end(c_housing, f_housing.round(4.5, hole)),
        end(c_housing, f_housing.flat(z, hole + z * seat)),
    );
    fasten(
        a,
        c_screw,
        f_screw,
        es::M4,
        end(
            c_chassis,
            f_chassis.round(cs::HOLE_RADIUS, DVec3::new(mx, my, 0.0)),
        ),
        end(c_chassis, f_chassis.flat(z, DVec3::new(mx + 10.0, my, t))),
    );

    // The other bolts round the housing's axis; the other screws along the walls.
    let bolts = a.add_pattern(
        &[c_bolt],
        PatternKind::Circular {
            axis: PatternLine::Geom(end(c_housing, f_housing.round(hs::BORE_R, DVec3::ZERO))),
            angle: Scalar::new(360.0),
            count: hs::BOLTS,
            flip: false,
        },
    );
    let along = |wall: &MateGeom, spacing: f64| PatternStep {
        direction: PatternLine::Geom(end(c_chassis, wall.clone())),
        spacing: Scalar::new(spacing),
        count: 2,
        flip: false,
    };
    let screws = a.add_pattern(
        &[c_screw],
        PatternKind::Linear {
            first: along(&right_wall, es::MOUNT_STEP.0),
            second: Some(along(&back_wall, es::MOUNT_STEP.1)),
        },
    );
    for (pattern, name) in [(bolts, "Bolts"), (screws, "Screws")] {
        if let Some(p) = pattern.and_then(|id| a.pattern_mut(id)) {
            name.clone_into(&mut p.name);
        }
    }

    // Taken apart: the cover with what is on it, then the housing, then the bolts.
    let copies = |a: &crate::Assembly, pattern: Option<crate::PatternId>, seed: CompId| {
        let mut all = vec![seed];
        if let Some(p) = pattern.and_then(|id| a.pattern(id)) {
            all.extend(p.instances.iter().map(|i| i.component));
        }
        all
    };
    let all_bolts = copies(a, bolts, c_bolt);
    let mut lifted = vec![c_cover, c_housing];
    lifted.extend(&all_bolts);
    a.add_explode_step(&lifted, DVec3::new(0.0, 0.0, 60.0));
    lifted.remove(0);
    a.add_explode_step(&lifted, DVec3::new(0.0, 0.0, 40.0));
    a.add_explode_step(&all_bolts, DVec3::new(0.0, 0.0, 30.0));
    let all_screws = copies(a, screws, c_screw);
    a.add_explode_step(&all_screws, DVec3::new(0.0, 0.0, 30.0));

    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    (model, engine)
}

struct Builder {
    model: Model,
    engine: Engine,
}

impl Builder {
    fn sketch(&mut self, plane: PlaneRef, draw: impl FnOnce(&mut Sketch)) -> FeatureId {
        let id = self.model.add_sketch(plane, Plane::TOP);
        if let Some(s) = self.model.feature_mut(id).and_then(|f| f.sketch_mut()) {
            draw(&mut s.sketch);
        }
        id
    }

    fn extrude(
        &mut self,
        sketch: FeatureId,
        operation: Operation,
        edit: impl FnOnce(&mut crate::Extrude),
    ) -> FeatureId {
        let id = self.model.add_extrude(sketch, operation);
        if let Some(e) = self.model.feature_mut(id).and_then(|f| f.extrude_mut()) {
            edit(&mut e.params);
        }
        id
    }

    /// A sketch on `plane`, drawn in model coordinates: `draw` gets a map from model
    /// points to the sketch's own.
    fn sketch_at(
        &mut self,
        plane: PlaneRef,
        draw: impl FnOnce(&mut Sketch, &dyn Fn(DVec3) -> DVec2),
    ) -> FeatureId {
        let id = self.model.add_sketch(plane, Plane::TOP);
        let at = match self.engine.regenerate(&mut self.model).output(id) {
            crate::Output::Sketch { plane, .. } => plane,
            _ => Plane::TOP,
        };
        if let Some(s) = self.model.feature_mut(id).and_then(|f| f.sketch_mut()) {
            draw(&mut s.sketch, &|p| at.to_plane_coords(p));
        }
        id
    }

    /// A reference to the edge between two points, in the model as built so far.
    fn edge(&mut self, a: DVec3, b: DVec3) -> crate::EdgeRef {
        let eval = self.engine.regenerate(&mut self.model);
        for body in &eval.bodies {
            for e in body.solid.edge_ids() {
                let edge = body.solid.edge(e);
                let (s, t) = (
                    body.solid.vertex(edge.start).point,
                    body.solid.vertex(edge.end).point,
                );
                let same = |p: DVec3, q: DVec3| p.distance(q) < 1e-9;
                if ((same(s, a) && same(t, b)) || (same(s, b) && same(t, a)))
                    && let Some(r) = body.edge_ref(e)
                {
                    return r;
                }
            }
        }
        panic!("the sample has no edge from {a} to {b}");
    }

    /// A reference to the planar face with outward normal `n` through `at`, in the model
    /// as built so far.
    /// The round edge of this radius about the Z axis at height `z`.
    fn circle_edge(&mut self, radius: f64, z: f64) -> crate::EdgeRef {
        let eval = self.engine.regenerate(&mut self.model);
        for body in &eval.bodies {
            for e in body.solid.edge_ids() {
                if let peet_kernel::Curve3::Circle(c) = body.solid.edge(e).curve
                    && (c.radius - radius).abs() < 1e-9
                    && (c.frame.origin.z - z).abs() < 1e-9
                    && let Some(r) = body.edge_ref(e)
                {
                    return r;
                }
            }
        }
        panic!("no round edge of radius {radius} at z = {z}");
    }

    fn face(&mut self, n: DVec3, at: DVec3) -> PlaneRef {
        let eval = self.engine.regenerate(&mut self.model);
        for body in &eval.bodies {
            for f in body.solid.face_ids() {
                if let Surface::Plane(p) = body.solid.face(f).surface
                    && body.solid.face_normal_at(f, at).dot(n) > 0.999
                    && p.signed_distance(at).abs() < 1e-9
                {
                    return PlaneRef::Face(body.face_ref(f));
                }
            }
        }
        panic!("the sample has no face with normal {n} through {at}");
    }
}
