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

    // The rim: a wall and a lip, drawn on the base's front end face at the right-hand
    // corner, run along the right, back and left edges.
    let end = b.face(-DVec3::Y, DVec3::new(w / 2.0, 0.0, t / 2.0));
    let profile = b.sketch_at(end, |s, to| {
        let p0 = to(DVec3::new(w, 0.0, 0.0));
        let p1 = to(DVec3::new(w, 0.0, cs::WALL));
        let p2 = to(DVec3::new(w - cs::LIP, 0.0, cs::WALL));
        let wall = s.add_line(p0, p1);
        let lip = s.add_line(p1, p2);
        if let (Some((_, wall_end)), Some((lip_start, _))) = (s.endpoints(wall), s.endpoints(lip)) {
            let _ = s.add_constraint(ConstraintKind::Coincident(wall_end, lip_start));
        }
        let _ = s.add_dimension(ConstraintKind::Length(wall), cs::WALL);
        let _ = s.add_dimension(ConstraintKind::Length(lip), cs::LIP);
    });
    let corners = [
        DVec3::new(w, 0.0, t),
        DVec3::new(w, d, t),
        DVec3::new(0.0, d, t),
        DVec3::new(0.0, 0.0, t),
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
            count: nx,
            flip: false,
        },
        second: Some(LinearDirection {
            direction: AxisRef::Standard(StdAxis::Y),
            spacing: Scalar::new(y),
            count: ny,
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
            count: hs::BOLTS,
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
