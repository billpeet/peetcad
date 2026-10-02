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
