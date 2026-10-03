//! Changes made to the model directly, as the application's tools make them, applied as
//! operations: the part must end up exactly as it would have.

use peet_document::Document;
use peet_math::{DVec2, Plane};
use peet_model::{
    AxisRef, BlendKind, Datum, FeatureId, FeatureKind, HoleFit, LinearDirection, METRIC, Model,
    Operation, PatternDef, PlaneDef, PlaneRef, Scalar, StdAxis, StdPlane,
};
use peet_ops::{Headless, Op, apply_model, diff};

/// Makes the same change two ways: directly (as before operations existed), and through
/// the operations. Both documents must end up with the same model.
struct Pair {
    direct: Document,
    through_ops: Document,
    host: Headless,
    key: u64,
    /// The operations of the last change.
    last: Vec<Op>,
}

impl Pair {
    fn new() -> Self {
        Self {
            direct: Document::default(),
            through_ops: Document::default(),
            host: Headless::default(),
            key: 0,
            last: Vec::new(),
        }
    }

    fn change(&mut self, label: &str, f: impl Fn(&mut Model)) -> &[Op] {
        self.direct.change(label, &f);
        let mut new = self.through_ops.model.clone();
        f(&mut new);
        self.key += 1;
        let done = apply_model(&mut self.host, &mut self.through_ops, new, label, self.key);
        self.through_ops.seal_history();
        assert_eq!(done.untranslated, None, "{label}: {:?}", done.ops);
        assert_eq!(self.through_ops.model, self.direct.model, "{label}");
        assert_eq!(
            self.through_ops.undo_label(),
            self.direct.undo_label(),
            "{label}"
        );
        self.last = done.ops;
        &self.last
    }

    fn id(&self, name: &str) -> FeatureId {
        self.direct
            .model
            .features()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .id
    }

    /// The first edge whose ends satisfy `wanted`.
    fn edge(
        &self,
        wanted: impl Fn(peet_math::DVec3, peet_math::DVec3) -> bool,
    ) -> peet_model::EdgeRef {
        for body in &self.direct.evaluation().bodies {
            for e in body.solid.edge_ids() {
                let edge = body.solid.edge(e);
                let (s, t) = (
                    body.solid.vertex(edge.start).point,
                    body.solid.vertex(edge.end).point,
                );
                if (wanted(s, t) || wanted(t, s))
                    && let Some(r) = body.edge_ref(e)
                {
                    return r;
                }
            }
        }
        panic!("no such edge");
    }

    /// The edge of the sheet's top face that runs from `a` to `b` (x and y).
    fn top_edge(&self, a: [f64; 2], b: [f64; 2]) -> peet_model::EdgeRef {
        let at = |p: peet_math::DVec3, q: [f64; 2]| {
            (p.x - q[0]).abs() < 1e-9 && (p.y - q[1]).abs() < 1e-9
        };
        self.edge(|s, t| at(s, a) && at(t, b) && (s.z - t.z).abs() < 1e-9)
    }

    fn top_face(&self) -> peet_model::FaceRef {
        let body = &self.direct.evaluation().bodies[0];
        let f = body
            .solid
            .face_ids()
            .max_by(|a, b| body.face_center(*a).z.total_cmp(&body.face_center(*b).z))
            .unwrap();
        body.face_ref(f)
    }
}

fn rectangle(m: &mut Model, w: f64, h: f64) -> FeatureId {
    let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
        peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(w, h));
    }
    s
}

#[test]
fn the_samples_are_rebuilt_from_nothing_by_operations() {
    for (name, (sample, _)) in [
        ("bracket", peet_model::samples::bracket()),
        ("enclosure", peet_model::samples::enclosure()),
        ("chassis", peet_model::samples::chassis()),
        ("housing", peet_model::samples::housing()),
    ] {
        // The samples are named; an empty part is not, and no operation renames a part.
        let mut doc = Document::default();
        doc.model.name.clone_from(&sample.name);
        let ops = diff(&doc, &sample).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(ops.len() >= sample.len() / 2, "{name}");
        let done = apply_model(
            &mut Headless::default(),
            &mut doc,
            sample.clone(),
            "Build",
            1,
        );
        assert_eq!(done.untranslated, None, "{name}");
        assert_eq!(doc.evaluation().failures().count(), 0, "{name}");
        // Applied a second time, there is nothing to do.
        assert_eq!(
            diff(&doc, &doc.model.clone()).unwrap(),
            Vec::new(),
            "{name}"
        );
        // One undo step, as one change to the model is.
        doc.seal_history();
        assert!(doc.undo().is_some());
        assert!(doc.model.is_empty(), "{name}");
    }
}

#[test]
fn the_tools_changes_are_made_by_operations() {
    let mut p = Pair::new();
    // What New Sketch, drawing and Extrude do.
    p.change("New Sketch", |m| {
        m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    });
    let sketch = p.id("Sketch1");
    let ops = p.change("Edit Sketch1", |m| {
        let f = m.feature_mut(sketch).unwrap();
        peet_sketch::shapes::rectangle(
            &mut f.sketch_mut().unwrap().sketch,
            DVec2::ZERO,
            DVec2::new(60.0, 40.0),
        );
        f.visible = true;
    });
    assert!(matches!(ops, [Op::SetSketch { .. }]), "{ops:?}");
    let ops = p.change("Add Extrude", |m| {
        let e = m.add_extrude(sketch, Operation::Add);
        let x = m.feature_mut(e).unwrap().extrude_mut().unwrap();
        x.params.operation = Operation::NewBody;
    });
    assert!(matches!(ops, [Op::Add(new)] if new.len() == 1), "{ops:?}");
    let extrude = p.id("Extrude1");

    // The properties panel: a value, then an expression, then a toggle.
    let ops = p.change("Edit Extrude1", |m| {
        let x = m.feature_mut(extrude).unwrap().extrude_mut().unwrap();
        x.params.depth = Scalar::new(20.0);
    });
    assert!(matches!(ops, [Op::Edit { .. }]), "{ops:?}");
    p.change("Edit Parameters", |m| {
        m.parameters.set("deep", "25mm").unwrap();
        m.parameters.set("wide", "2 * deep").unwrap();
    });
    p.change("Edit Extrude1", |m| {
        let e = m.feature_mut(extrude).unwrap().extrude_mut().unwrap();
        e.params.depth = Scalar {
            value: 25.0,
            expression: Some("deep".to_owned()),
        };
        e.params.reverse = true;
    });
    p.change("Edit Parameters", |m| {
        m.parameters.set("deep", "30mm").unwrap();
        m.parameters.remove("wide");
    });
    for unit in [
        peet_sketch::expr::LengthUnit::Inch,
        peet_sketch::expr::LengthUnit::Mm,
    ] {
        p.change("Change Units", |m| {
            m.parameters.set_units(peet_sketch::expr::Units::new(unit));
        });
    }

    // The tree: rename, hide, suppress, the planes.
    let ops = p.change("Rename Extrude1", |m| {
        m.feature_mut(extrude).unwrap().name = "Block".to_owned();
    });
    assert!(matches!(ops, [Op::Rename { .. }]), "{ops:?}");
    p.change("Show Sketch1", |m| {
        m.feature_mut(sketch).unwrap().visible = true;
    });
    p.change("Suppress Block", |m| {
        m.feature_mut(extrude).unwrap().suppressed = true;
    });
    p.change("Unsuppress Block", |m| {
        m.feature_mut(extrude).unwrap().suppressed = false;
    });
    let ops = p.change("Hide Planes", |m| {
        for plane in StdPlane::ALL {
            m.set_datum_visible(Datum::Plane(plane), false);
        }
    });
    assert_eq!(ops.len(), 3);
    p.change("Hide Origin", |m| m.set_datum_visible(Datum::Origin, false));

    // Reference geometry built on the body, a second sketch on a face, a hole.
    let top = p.top_face();
    p.change("Add Reference plane", |m| {
        m.add(FeatureKind::Plane(PlaneDef::Offset {
            from: PlaneRef::Face(top.clone()),
            distance: Scalar::new(10.0),
            flip: false,
        }));
    });
    let plane = p.id("Plane1");
    // Another form of the same feature: an offset plane made an angled one.
    let ops = p.change("Edit Plane1", |m| {
        m.feature_mut(plane).unwrap().kind = FeatureKind::Plane(PlaneDef::Angled {
            from: PlaneRef::Standard(StdPlane::Top),
            about: AxisRef::Standard(StdAxis::X),
            angle: Scalar::new(30.0),
        });
    });
    assert!(matches!(ops, [Op::Edit { .. }]), "{ops:?}");
    p.change("New Sketch", |m| {
        let s = m.add_sketch(PlaneRef::Face(top.clone()), Plane::TOP);
        let f = m.feature_mut(s).unwrap().sketch_mut().unwrap();
        f.sketch.add_point(DVec2::new(15.0, 20.0));
    });
    let centres = p.id("Sketch2");
    p.change("Add Hole", |m| {
        m.add_hole(centres);
    });
    let hole = p.id("Hole1");
    p.change("Edit Hole1", |m| {
        if let FeatureKind::Hole(h) = &mut m.feature_mut(hole).unwrap().kind {
            h.set_standard(&METRIC[7], HoleFit::Tapped);
        }
    });
    p.change("Edit Hole1", |m| {
        if let FeatureKind::Hole(h) = &mut m.feature_mut(hole).unwrap().kind {
            h.diameter = Scalar::new(7.3);
            h.standard = None;
            h.thread = None;
        }
    });
    assert!(p.direct.evaluation().failures().next().is_none());

    // A pattern, made a grid and a row again.
    p.change("Add Linear Pattern", |m| {
        m.add_pattern(
            vec![hole],
            PatternDef::Linear {
                first: LinearDirection {
                    direction: AxisRef::Standard(StdAxis::X),
                    spacing: Scalar::new(20.0),
                    count: 2,
                    flip: false,
                },
                second: None,
            },
        );
    });
    let pattern = p.id("LPattern1");
    let set_second = |m: &mut Model, second: Option<LinearDirection>| {
        if let FeatureKind::Pattern(pat) = &mut m.feature_mut(pattern).unwrap().kind
            && let PatternDef::Linear { second: s, .. } = &mut pat.def
        {
            *s = second;
        }
    };
    p.change("Edit LPattern1", |m| {
        set_second(
            m,
            Some(LinearDirection {
                direction: AxisRef::Standard(StdAxis::Y),
                spacing: Scalar::new(12.0),
                count: 2,
                flip: true,
            }),
        );
    });
    p.change("Edit LPattern1", |m| set_second(m, None));

    // Features waiting for a pick: a fillet with no edges, then given one.
    p.change("Add Fillet", |m| {
        m.add_blend(BlendKind::Fillet, Vec::new());
    });
    let fillet = p.id("Fillet1");
    // An upright edge of the block.
    let edge = p.edge(|s, t| s.x == t.x && s.y == t.y && s.z != t.z);
    p.change("Edit Fillet1", |m| {
        if let FeatureKind::Blend(b) = &mut m.feature_mut(fillet).unwrap().kind {
            b.edges.push(edge.clone());
        }
    });
    assert!(p.direct.evaluation().failures().next().is_none());

    // The tree again: the rollback bar, moving, deleting.
    p.change("Roll Back", |m| m.set_rollback(Some(2)));
    p.change("Add Reference plane", |m| {
        m.add(FeatureKind::Plane(PlaneDef::Offset {
            from: PlaneRef::Standard(StdPlane::Front),
            distance: Scalar::new(5.0),
            flip: true,
        }));
    });
    assert_eq!(p.direct.model.rollback_index(), 3);
    p.change("Roll Back", |m| m.set_rollback(Some(0)));
    p.change("Roll to End", |m| m.set_rollback(None));
    let moved = p.id("Plane2");
    let ops = p.change("Move Plane2", |m| m.move_to(moved, 0).unwrap());
    assert!(matches!(ops, [Op::Move { .. }]), "{ops:?}");
    let ops = p.change("Delete Fillet1, LPattern1", |m| {
        m.remove(fillet);
        m.remove(pattern);
    });
    assert!(
        matches!(ops, [Op::Delete { features }] if features.len() == 2),
        "{ops:?}"
    );
    // A change that changes nothing is no operation and no undo step.
    let before = p.through_ops.undo_label().map(str::to_owned);
    assert!(p.change("Nothing", |_| {}).is_empty());
    assert_eq!(p.through_ops.undo_label().map(str::to_owned), before);
}

#[test]
fn sheet_metal_tools_are_made_by_operations() {
    let mut p = Pair::new();
    p.change("New Sketch", |m| {
        rectangle(m, 200.0, 150.0);
    });
    let sketch = p.id("Sketch1");
    p.change("Add Base Flange", |m| {
        m.add_base_flange(sketch);
    });
    // Several flanges at once, and one waiting for its edge.
    let edges = [
        p.top_edge([0.0, 0.0], [200.0, 0.0]),
        p.top_edge([200.0, 0.0], [200.0, 150.0]),
    ];
    let ops = p.change("Add Edge Flanges", |m| {
        for e in &edges {
            m.add_edge_flange(Some(e.clone()));
        }
        m.add_edge_flange(None);
    });
    assert!(matches!(ops, [Op::Add(new)] if new.len() == 3), "{ops:?}");
    let waiting = p.id("Edge-Flange3");
    // The long edge at the back (its ends have moved in for the flange next to it).
    let third = p.edge(|s, t| {
        (s.y - 150.0).abs() < 1e-9
            && (t.y - 150.0).abs() < 1e-9
            && (s.z - t.z).abs() < 1e-9
            && (s.x - t.x).abs() > 100.0
    });
    p.change("Edit Edge-Flange3", |m| {
        if let FeatureKind::EdgeFlange(f) = &mut m.feature_mut(waiting).unwrap().kind {
            f.edge = Some(third.clone());
            f.radius = Some(Scalar::new(3.0));
        }
    });
    p.change("Edit Edge-Flange3", |m| {
        if let FeatureKind::EdgeFlange(f) = &mut m.feature_mut(waiting).unwrap().kind {
            f.radius = None;
        }
    });
    // Applying a material is an edit of the base flange.
    let base = p.id("Base-Flange1");
    p.change("Apply 2 mm to Base-Flange1", |m| {
        if let FeatureKind::BaseFlange(b) = &mut m.feature_mut(base).unwrap().kind {
            b.settings.thickness = Scalar::new(2.0);
            b.settings.radius = Scalar::new(2.5);
            b.settings.model = peet_model::BendModelDef::Allowance(Scalar::new(4.1));
        }
    });
    // A hem and a corner, added as the commands add them.
    p.change("Add Hem", |m| {
        m.add_hem(None);
    });
    p.change("Add Corner", |m| {
        let c = m.add_corner(Vec::new());
        if let FeatureKind::Corner(def) = &mut m.feature_mut(c).unwrap().kind {
            def.gap = Scalar::new(0.2);
        }
    });
}

#[test]
fn a_drag_is_one_undo_step() {
    let mut doc = Document::default();
    let mut host = Headless::default();
    let (model, _) = peet_model::samples::enclosure();
    doc.model.name.clone_from(&model.name);
    apply_model(&mut host, &mut doc, model, "Build", 1);
    doc.seal_history();
    let flange = doc
        .model
        .features()
        .find(|f| f.name == "Edge-Flange1")
        .unwrap()
        .id;
    for length in [26.0, 27.5, 31.0] {
        let mut new = doc.model.clone();
        if let FeatureKind::EdgeFlange(f) = &mut new.feature_mut(flange).unwrap().kind {
            f.length = Scalar::new(length);
        }
        let done = apply_model(&mut host, &mut doc, new, "Drag Edge-Flange1 Length", 7);
        assert_eq!(done.untranslated, None);
        assert!(done.changed);
    }
    doc.seal_history();
    assert_eq!(doc.undo().as_deref(), Some("Drag Edge-Flange1 Length"));
    assert_eq!(doc.undo_label(), Some("Build"));
}
