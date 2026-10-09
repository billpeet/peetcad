//! Drives the sketch editor with simulated mouse and keyboard input (no GPU needed), to
//! check the tool state machines end to end: clicks become geometry with the right
//! relations, dimensions can be placed and typed in, dragging solves, conflicts are
//! rejected.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Sense};
use egui_kittest::Harness;
use peet_math::{DVec2, Plane};
use peet_render::{Camera, Projection, StandardView};
use peet_sketch::expr::Parameters;
use peet_sketch::{ConstraintKind, EntityKind, Sketch};

use super::{SketchEditor, SketchView, Tool};
use crate::commands::CommandId;
use crate::document::{ItemId, SketchItem, SketchStatus};

struct State {
    editor: SketchEditor,
    item: SketchItem,
    params: Parameters,
    camera: Camera,
    rect: Rect,
}

fn harness() -> Harness<'static, State> {
    harness_stepping(0.25)
}

/// A harness whose frames are `step_dt` seconds apart (short enough, a double-click fits).
fn harness_stepping(step_dt: f32) -> Harness<'static, State> {
    let mut item = SketchItem {
        plane: Plane::TOP,
        plane_name: "Top Plane".to_owned(),
        sketch: Sketch::new(),
        status: SketchStatus::Under,
    };
    let editor = SketchEditor::new(ItemId::Feature(peet_model::FeatureId(1000)), &mut item);
    let camera = Camera {
        rotation: StandardView::Top.rotation(),
        projection: Projection::Orthographic,
        distance: 200.0,
        ..Camera::default()
    };
    let state = State {
        editor,
        item,
        params: Parameters::default(),
        camera,
        rect: Rect::NOTHING,
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .with_step_dt(step_dt)
        .build_ui_state(
            |ui, s: &mut State| {
                let rect = ui.max_rect();
                s.rect = rect;
                let response = ui.allocate_rect(rect, Sense::click_and_drag());
                s.editor
                    .show(ui, &response, &s.camera, &mut s.item, &s.params, false);
            },
            state,
        );
    h.step();
    h
}

fn to_screen(h: &Harness<'_, State>, p: DVec2) -> Pos2 {
    let s = h.state();
    SketchView::new(&s.camera, s.rect, s.item.plane).to_screen(p)
}

fn frames(h: &mut Harness<'_, State>, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

fn button(h: &Harness<'_, State>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// Moves to a sketch position and clicks there.
fn click(h: &mut Harness<'_, State>, p: DVec2) {
    let pos = to_screen(h, p);
    h.hover_at(pos);
    frames(h, 2);
    button(h, pos, true);
    h.step();
    button(h, pos, false);
    frames(h, 2);
}

/// Drags from one sketch position to another in a few steps.
fn drag(h: &mut Harness<'_, State>, from: DVec2, to: DVec2) {
    let a = to_screen(h, from);
    let b = to_screen(h, to);
    h.hover_at(a);
    frames(h, 2);
    button(h, a, true);
    h.step();
    for i in 1..=8 {
        h.hover_at(a + (b - a) * (i as f32 / 8.0));
        h.step();
    }
    button(h, b, false);
    frames(h, 2);
}

fn key(h: &mut Harness<'_, State>, k: Key) {
    h.key_press(k);
    frames(h, 2);
}

fn command(h: &mut Harness<'_, State>, cmd: CommandId) {
    let s = h.state_mut();
    s.editor.command(cmd, &mut s.item);
    frames(h, 1);
}

fn count(s: &Sketch, kind: EntityKind) -> usize {
    s.entities().filter(|(_, e)| e.kind() == kind).count()
}

fn has(s: &Sketch, f: impl Fn(&ConstraintKind) -> bool) -> bool {
    s.constraints().any(|(_, c)| f(&c.kind))
}

#[test]
fn line_chain_infers_relations() {
    let mut h = harness();
    command(&mut h, CommandId::SketchLine);
    click(&mut h, DVec2::ZERO); // on the origin
    click(&mut h, DVec2::new(30.0, 0.4)); // nearly horizontal: snaps
    click(&mut h, DVec2::new(30.3, 20.0)); // nearly vertical: snaps
    key(&mut h, Key::Escape);

    let s = &h.state().item.sketch;
    assert_eq!(count(s, EntityKind::Line), 2);
    assert!(has(s, |k| matches!(k, ConstraintKind::Horizontal(_))));
    assert!(has(s, |k| matches!(k, ConstraintKind::Vertical(_))));
    assert!(has(
        s,
        |k| matches!(k, ConstraintKind::Coincident(a, b) if *a == Sketch::ORIGIN || *b == Sketch::ORIGIN)
    ));
    // The chain's corner is joined.
    let lines: Vec<_> = s
        .entities()
        .filter(|(_, e)| e.kind() == EntityKind::Line)
        .map(|(id, _)| id)
        .collect();
    let (_, end0) = s.endpoints(lines[0]).unwrap();
    let (start1, _) = s.endpoints(lines[1]).unwrap();
    assert!(s.point(end0).distance(s.point(start1)) < 1e-9);
    assert!(
        s.point(end0).abs_diff_eq(DVec2::new(30.0, 0.0), 0.2),
        "{:?}",
        s.point(end0)
    );
    let st = &h.state().editor;
    assert_eq!(st.tool, Tool::Line, "Esc ends the chain but keeps the tool");
    assert_eq!(st.analysis.dof, 2, "two lengths remain free");
}

#[test]
fn rectangle_then_dimension_and_undo() {
    let mut h = harness();
    command(&mut h, CommandId::SketchRectangle);
    click(&mut h, DVec2::new(-20.0, -10.0));
    click(&mut h, DVec2::new(20.0, 15.0));
    {
        let s = &h.state().item.sketch;
        assert_eq!(count(s, EntityKind::Line), 4);
        assert_eq!(h.state().editor.analysis.dof, 4);
    }

    // Smart dimension on the bottom line, placed below it, then typed in.
    command(&mut h, CommandId::SmartDimension);
    click(&mut h, DVec2::new(0.0, -10.0));
    click(&mut h, DVec2::new(0.0, -25.0));
    assert!(
        h.state().editor.edit.is_some(),
        "value editor opens after placing"
    );
    {
        let s = h.state_mut();
        if let Some(edit) = &mut s.editor.edit {
            edit.text = "50".to_owned();
        }
    }
    key(&mut h, Key::Enter);
    let s = &h.state().item.sketch;
    let dims: Vec<_> = s
        .constraints()
        .filter(|(_, c)| c.dimension.is_some())
        .collect();
    assert_eq!(dims.len(), 1);
    assert_eq!(dims[0].1.kind.label(), "Length");
    assert!((dims[0].1.dimension.as_ref().unwrap().value - 50.0).abs() < 1e-12);
    let line = dims[0].1.kind.entities()[0];
    assert!((s.curve(line).unwrap().length() - 50.0).abs() < 1e-9);
    assert_eq!(h.state().editor.analysis.dof, 3);

    // One undo step removes the dimension (added and set in one step).
    command(&mut h, CommandId::Undo);
    let s = &h.state().item.sketch;
    assert_eq!(
        s.constraints()
            .filter(|(_, c)| c.dimension.is_some())
            .count(),
        0
    );
    command(&mut h, CommandId::Undo);
    assert_eq!(count(&h.state().item.sketch, EntityKind::Line), 0);
    command(&mut h, CommandId::Redo);
    assert_eq!(count(&h.state().item.sketch, EntityKind::Line), 4);
}

#[test]
fn dragging_a_point_solves() {
    let mut h = harness();
    command(&mut h, CommandId::SketchRectangle);
    click(&mut h, DVec2::new(10.0, 10.0));
    click(&mut h, DVec2::new(40.0, 30.0));
    command(&mut h, CommandId::SketchSelect);
    drag(&mut h, DVec2::new(40.0, 30.0), DVec2::new(50.0, 35.0));
    let s = &h.state().item.sketch;
    // Still a rectangle, now reaching the new corner.
    let (lo, hi) = s
        .entities()
        .filter_map(|(id, _)| s.curve(id))
        .map(|c| c.bounds())
        .fold(
            (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN)),
            |(a, b), (c, d)| (a.min(c), b.max(d)),
        );
    assert!(lo.abs_diff_eq(DVec2::new(10.0, 10.0), 0.3), "{lo:?}");
    assert!(hi.abs_diff_eq(DVec2::new(50.0, 35.0), 0.3), "{hi:?}");
    assert!(h.state().editor.last_report.converged);
    assert!(h.state().editor.can_undo());
}

#[test]
fn conflicting_relation_is_rejected_with_explanation() {
    let mut h = harness();
    command(&mut h, CommandId::SketchLine);
    click(&mut h, DVec2::new(5.0, 5.0));
    click(&mut h, DVec2::new(35.0, 5.0)); // horizontal
    key(&mut h, Key::Escape);
    key(&mut h, Key::Escape); // back to select
    assert_eq!(h.state().editor.tool, Tool::Select);
    click(&mut h, DVec2::new(20.0, 5.0)); // select the line
    assert_eq!(h.state().editor.selected_entities().len(), 1);
    let before = h.state().item.sketch.constraints().count();
    command(&mut h, CommandId::RelVertical);
    let st = h.state();
    assert_eq!(st.item.sketch.constraints().count(), before, "rejected");
    let msg = st.editor.message.as_ref().expect("explains why");
    assert!(msg.error);
    assert!(msg.text.contains("Horizontal"), "{}", msg.text);
}

#[test]
fn delete_removes_selection() {
    let mut h = harness();
    command(&mut h, CommandId::SketchCircle);
    click(&mut h, DVec2::new(0.0, 20.0));
    click(&mut h, DVec2::new(10.0, 20.0));
    command(&mut h, CommandId::SketchSelect);
    click(&mut h, DVec2::new(10.0, 20.0));
    command(&mut h, CommandId::DeleteSelection);
    assert_eq!(count(&h.state().item.sketch, EntityKind::Circle), 0);
}

#[test]
fn redundant_dimension_becomes_driven() {
    let mut h = harness();
    command(&mut h, CommandId::SketchRectangle);
    click(&mut h, DVec2::new(-20.0, -10.0));
    click(&mut h, DVec2::new(20.0, 15.0));
    command(&mut h, CommandId::SmartDimension);
    click(&mut h, DVec2::new(0.0, -10.0)); // bottom
    click(&mut h, DVec2::new(0.0, -20.0));
    key(&mut h, Key::Enter);
    click(&mut h, DVec2::new(0.0, 15.0)); // top: same length as the bottom
    click(&mut h, DVec2::new(0.0, 25.0));
    let s = &h.state().item.sketch;
    let dims: Vec<_> = s
        .constraints()
        .filter_map(|(_, c)| c.dimension.clone())
        .collect();
    assert_eq!(dims.len(), 2);
    assert!(dims[0].driving);
    assert!(!dims[1].driving, "the second width only measures");
    assert!(h.state().editor.analysis.diagnoses.is_empty());
    assert!(
        h.state().editor.edit.is_none(),
        "nothing to type for a driven dimension"
    );
}

#[test]
fn dimension_expression_uses_parameters() {
    let mut h = harness();
    h.state_mut().params.set("width", "40").unwrap();
    command(&mut h, CommandId::SketchCircle);
    click(&mut h, DVec2::ZERO);
    click(&mut h, DVec2::new(10.0, 0.0));
    command(&mut h, CommandId::SmartDimension);
    click(&mut h, DVec2::new(10.0, 0.0));
    click(&mut h, DVec2::new(20.0, 10.0));
    {
        let s = h.state_mut();
        s.editor.edit.as_mut().expect("editor open").text = "width / 2".to_owned();
    }
    key(&mut h, Key::Enter);
    let s = &h.state().item.sketch;
    let (id, c) = s
        .constraints()
        .find(|(_, c)| c.dimension.is_some())
        .unwrap();
    let d = c.dimension.as_ref().unwrap();
    assert_eq!(d.expression.as_deref(), Some("width / 2"));
    assert!((d.value - 20.0).abs() < 1e-12, "diameter {}", d.value);
    let circle = c.kind.entities()[0];
    assert!((s.curve(circle).unwrap().radius().unwrap() - 10.0).abs() < 1e-9);

    // Changing the parameter updates the sketch.
    let st = h.state_mut();
    st.params.set("width", "60").unwrap();
    st.editor.refresh(&mut st.item, &st.params);
    let s = &st.item.sketch;
    assert!((s.constraint(id).unwrap().dimension.as_ref().unwrap().value - 30.0).abs() < 1e-12);
    assert!((s.curve(circle).unwrap().radius().unwrap() - 15.0).abs() < 1e-9);
}

#[test]
fn trim_tool_removes_the_clicked_piece() {
    let mut h = harness();
    {
        let s = h.state_mut();
        s.item
            .sketch
            .add_line(DVec2::new(-30.0, 0.0), DVec2::new(30.0, 0.0));
        s.item
            .sketch
            .add_line(DVec2::new(0.0, -20.0), DVec2::new(0.0, 20.0));
    }
    command(&mut h, CommandId::SketchTrim);
    click(&mut h, DVec2::new(20.0, 0.0));
    let s = &h.state().item.sketch;
    let longest_x = s
        .entities()
        .filter_map(|(id, _)| s.curve(id))
        .map(|c| c.bounds().1.x)
        .fold(f64::MIN, f64::max);
    assert!(
        longest_x.abs() < 1e-9,
        "right half of the horizontal line is gone"
    );
    assert_eq!(count(s, EntityKind::Line), 2);
}

#[test]
fn arc_direction_follows_the_cursor() {
    let mut h = harness();
    command(&mut h, CommandId::SketchArc);
    click(&mut h, DVec2::ZERO);
    click(&mut h, DVec2::new(20.0, 0.0));
    // Sweep clockwise (downwards) to the end.
    for a in [-0.3f64, -0.8, -1.3] {
        h.hover_at(to_screen(&h, DVec2::from_angle(a) * 20.0));
        frames(&mut h, 1);
    }
    click(&mut h, DVec2::new(0.0, -20.0));
    let s = &h.state().item.sketch;
    let (id, _) = s
        .entities()
        .find(|(_, e)| e.kind() == EntityKind::Arc)
        .expect("an arc");
    let arc = s.curve(id).unwrap();
    // A quarter arc in the fourth quadrant.
    assert!(
        (arc.length() - std::f64::consts::FRAC_PI_2 * 20.0).abs() < 0.5,
        "{}",
        arc.length()
    );
    assert!(arc.point_at(0.5).y < 0.0 && arc.point_at(0.5).x > 0.0);
}

// ---- The spline tool ----

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn splines(s: &Sketch) -> Vec<peet_sketch::EntityId> {
    s.entities()
        .filter(|(_, e)| e.kind() == EntityKind::Spline)
        .map(|(id, _)| id)
        .collect()
}

fn right_click(h: &mut Harness<'_, State>, p: DVec2) {
    let pos = to_screen(h, p);
    h.hover_at(pos);
    frames(h, 2);
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
    frames(h, 2);
}

/// Two clicks in quick succession at a sketch position.
fn double_click(h: &mut Harness<'_, State>, p: DVec2) {
    let pos = to_screen(h, p);
    h.hover_at(pos);
    frames(h, 2);
    for _ in 0..2 {
        button(h, pos, true);
        h.step();
        button(h, pos, false);
        h.step();
    }
    frames(h, 2);
}

#[test]
fn spline_tool_places_points_until_enter() {
    let mut h = harness();
    command(&mut h, CommandId::SketchSpline);
    assert_eq!(h.state().editor.tool, Tool::Spline);
    click(&mut h, DVec2::ZERO); // on the origin
    click(&mut h, v(20.0, 15.0));
    click(&mut h, v(40.0, -5.0));
    assert!(
        splines(&h.state().item.sketch).is_empty(),
        "not finished yet"
    );
    assert!(h.state().editor.hint().contains("Enter"));

    // The preview is the spline through the clicks and the cursor.
    h.hover_at(to_screen(&h, v(60.0, 10.0)));
    frames(&mut h, 2);
    {
        let s = h.state();
        let preview = s.editor.preview(&s.item.sketch);
        assert_eq!(preview.len(), 1);
        assert!(matches!(preview[0], peet_sketch::Curve::Spline(_)));
        for p in [DVec2::ZERO, v(20.0, 15.0), v(40.0, -5.0)] {
            assert!(preview[0].distance(p) < 0.5, "{p}");
        }
        assert!(preview[0].end().distance(v(60.0, 10.0)) < 0.5);
    }

    click(&mut h, v(60.0, 10.0));
    key(&mut h, Key::Enter);
    {
        let s = &h.state().item.sketch;
        let found = splines(s);
        assert_eq!(found.len(), 1);
        let (points, closed) = s.spline_points(found[0]).unwrap();
        assert!(!closed);
        assert_eq!(points.len(), 4);
        assert!(s.point(points[3]).distance(v(60.0, 10.0)) < 0.5);
        // The first click's snap to the origin became a relation on the first point.
        assert!(has(s, |k| matches!(k, ConstraintKind::Coincident(a, b)
            if (*a == points[0] && *b == Sketch::ORIGIN)
                || (*b == points[0] && *a == Sketch::ORIGIN))));
        assert_eq!(s.point(points[0]), DVec2::ZERO);
    }
    // The tool stays active for the next spline, with nothing pending.
    assert_eq!(h.state().editor.tool, Tool::Spline);
    assert!(h.state().editor.clicks.is_empty());
    // One undo step takes the whole spline away.
    command(&mut h, CommandId::Undo);
    assert!(splines(&h.state().item.sketch).is_empty());
    assert_eq!(count(&h.state().item.sketch, EntityKind::Point), 1);
}

#[test]
fn spline_closes_on_its_first_point() {
    let mut h = harness();
    command(&mut h, CommandId::SketchSpline);
    for p in [v(30.0, 0.0), v(0.0, 20.0), v(-30.0, 0.0), v(0.0, -20.0)] {
        click(&mut h, p);
    }
    // Hovering the first point shows the closed curve.
    h.hover_at(to_screen(&h, v(30.4, 0.3)));
    frames(&mut h, 2);
    {
        let s = h.state();
        let preview = s.editor.preview(&s.item.sketch);
        assert!(preview[0].is_closed());
    }
    click(&mut h, v(30.4, 0.3));
    let s = &h.state().item.sketch;
    let found = splines(s);
    assert_eq!(found.len(), 1);
    let (points, closed) = s.spline_points(found[0]).unwrap();
    assert!(closed);
    assert_eq!(points.len(), 4, "the closing click adds no point");
    assert!(s.curve(found[0]).unwrap().is_closed());
    assert!(h.state().editor.clicks.is_empty());
    // A closed spline is a region to extrude.
    assert_eq!(peet_sketch::region::find_regions(s).regions.len(), 1);
}

#[test]
fn spline_finishes_on_double_click_and_right_click_and_escape_cancels() {
    let mut h = harness_stepping(0.02);

    command(&mut h, CommandId::SketchSpline);
    // Esc drops the points placed so far and keeps the tool.
    click(&mut h, v(-40.0, 20.0));
    click(&mut h, v(-20.0, 30.0));
    click(&mut h, v(0.0, 20.0));
    key(&mut h, Key::Escape);
    assert!(splines(&h.state().item.sketch).is_empty());
    assert!(h.state().editor.clicks.is_empty());
    assert_eq!(h.state().editor.tool, Tool::Spline);

    // A double-click places the last point and finishes. Quick clicks in different
    // places before it are points like any other.
    click(&mut h, v(-40.0, -20.0));
    click(&mut h, v(-20.0, -30.0));
    assert_eq!(h.state().editor.clicks.len(), 2);
    frames(&mut h, 40);
    double_click(&mut h, v(0.0, -20.0));
    {
        let s = &h.state().item.sketch;
        let found = splines(s);
        assert_eq!(found.len(), 1);
        assert_eq!(s.spline_points(found[0]).unwrap().0.len(), 3);
    }
    assert!(h.state().editor.clicks.is_empty());

    // A right-click finishes without adding a point. Two points make a straight spline.
    frames(&mut h, 40); // let the double-click window pass

    click(&mut h, v(20.0, 40.0));
    click(&mut h, v(50.0, 40.0));
    right_click(&mut h, v(60.0, 30.0));
    {
        let s = &h.state().item.sketch;
        let found = splines(s);
        assert_eq!(found.len(), 2);
        assert_eq!(s.spline_points(found[1]).unwrap().0.len(), 2);
        assert!((s.curve(found[1]).unwrap().length() - 30.0).abs() < 0.5);
    }
    // With one point placed there is nothing to finish: Enter drops it.
    click(&mut h, v(20.0, 55.0));
    key(&mut h, Key::Enter);
    assert_eq!(splines(&h.state().item.sketch).len(), 2);
    assert!(h.state().editor.clicks.is_empty());
}

#[test]
fn spline_points_drag_and_the_curve_is_picked() {
    let mut h = harness();
    command(&mut h, CommandId::SketchSpline);
    for p in [v(-40.0, 0.0), v(-10.0, 20.0), v(20.0, -10.0), v(50.0, 10.0)] {
        click(&mut h, p);
    }
    key(&mut h, Key::Enter);
    command(&mut h, CommandId::SketchSelect);
    let spline = splines(&h.state().item.sketch)[0];
    let points = h
        .state()
        .item
        .sketch
        .spline_points(spline)
        .unwrap()
        .0
        .to_vec();

    // Hovering the curve between fit points picks the spline; hovering a fit point, it.
    let on_curve = h.state().item.sketch.curve(spline).unwrap().point_at(0.5);
    h.hover_at(to_screen(&h, on_curve));
    frames(&mut h, 2);
    assert_eq!(h.state().editor.hover, Some(super::Sel::Entity(spline)));
    h.hover_at(to_screen(&h, v(-10.0, 20.0)));
    frames(&mut h, 2);
    assert_eq!(h.state().editor.hover, Some(super::Sel::Entity(points[1])));
    assert_eq!(
        super::describe_entity(&h.state().item.sketch, points[1]),
        format!("Spline {} point 2", spline.0)
    );

    // Dragging a fit point moves it, and the curve with it; the other points stay.
    drag(&mut h, v(-10.0, 20.0), v(-5.0, 35.0));
    {
        let s = &h.state().item.sketch;
        assert!(s.point(points[1]).distance(v(-5.0, 35.0)) < 0.5);
        assert!(s.point(points[0]).distance(v(-40.0, 0.0)) < 0.5);
        assert!(s.curve(spline).unwrap().distance(s.point(points[1])) < 1e-9);
        assert!(h.state().editor.last_report.converged);
    }
    // Clicking the curve selects it, and Delete removes the spline and its points.
    let on_curve = h.state().item.sketch.curve(spline).unwrap().point_at(0.8);
    click(&mut h, on_curve);
    assert_eq!(h.state().editor.selection, vec![super::Sel::Entity(spline)]);
    command(&mut h, CommandId::DeleteSelection);
    let s = &h.state().item.sketch;
    assert!(splines(s).is_empty());
    assert_eq!(count(s, EntityKind::Point), 1, "only the origin is left");
}
