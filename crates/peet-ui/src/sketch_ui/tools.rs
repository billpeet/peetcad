//! Mouse input for the sketch tools.

use std::f64::consts::{PI, TAU};

use egui::{PointerButton, Pos2, Rect, Response, Ui};
use peet_math::DVec2;
use peet_sketch::infer::{self, Anchor, Inference, InferenceSettings, Inferred};
use peet_sketch::{ConstraintKind, Drag, EntityId, EntityKind, Sketch, shapes};

use super::draw::dimension_anchor;
use super::hit::HitFilter;
use super::{Chain, Click, DimEdit, DragKind, DragState, Sel, SketchEditor, SketchView, Tool};
use crate::document::SketchItem;

/// Snap distance for inference, in screen points.
const SNAP_PX: f64 = 8.0;
/// Shapes smaller than this on screen are treated as accidental clicks.
const MIN_SIZE_PX: f64 = 2.0;

impl SketchEditor {
    pub(super) fn handle_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        view: &SketchView,
        item: &mut SketchItem,
    ) {
        self.hover = None;
        self.inference = None;
        let modifiers = ui.input(|i| i.modifiers);
        // Alt + left drag is navigation in every mouse preset.
        let primary_click = response.clicked_by(PointerButton::Primary) && !modifiers.alt;
        let pos = response.hover_pos();
        match self.tool {
            Tool::Select => self.select_input(ui, response, view, item, primary_click),
            Tool::Dimension => self.dimension_input(view, item, pos, primary_click),
            Tool::Trim | Tool::Extend | Tool::Fillet => {
                self.pick_tool_input(view, item, pos, primary_click);
            }
            _ => self.draw_input(ui, response, view, item, primary_click),
        }
    }

    // ---- Select tool: selection, dragging and box selection ----

    fn select_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        view: &SketchView,
        item: &mut SketchItem,
        primary_click: bool,
    ) {
        let modifiers = ui.input(|i| i.modifiers);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if self.drag.is_none()
            && let Some(p) = response.hover_pos()
        {
            self.hover = self.hit(&item.sketch, view, p, HitFilter::Any);
        }

        if response.drag_started_by(PointerButton::Primary) && !modifiers.alt {
            let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
            if let Some(origin) = origin {
                self.start_drag(view, item, origin);
            }
        }

        if let (Some(drag), Some(p)) = (&mut self.drag, pointer)
            && response.dragged_by(PointerButton::Primary)
        {
            let cursor = view.to_sketch(p);
            match &mut drag.kind {
                DragKind::Geometry(d) => {
                    if let Some(c) = cursor {
                        match d {
                            Drag::Point { target, .. } | Drag::Curve { target, .. } => {
                                if *target != c {
                                    *target = c;
                                    drag.moved = true;
                                }
                            }
                        }
                        let d = *d;
                        let start = peet_platform::Instant::now();
                        self.last_report = self.solver.solve_drag(&mut item.sketch, &[d]);
                        self.solve_ms = peet_platform::elapsed_ms(start);
                    }
                }
                DragKind::Label {
                    dimension,
                    start_offset,
                    grab,
                } => {
                    if let (Some(c), Some(dim)) = (
                        cursor,
                        item.sketch
                            .constraint_mut(*dimension)
                            .and_then(|c| c.dimension.as_mut()),
                    ) {
                        dim.label_offset = *start_offset + (c - *grab);
                        drag.moved = true;
                    }
                }
                DragKind::Box { .. } => drag.moved = true,
            }
        }

        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
        {
            match drag.kind {
                DragKind::Box { start } => {
                    if let Some(end) = pointer {
                        let rect = Rect::from_two_pos(start, end);
                        self.box_select(
                            &item.sketch,
                            view,
                            rect,
                            modifiers.shift || modifiers.command,
                        );
                    }
                }
                DragKind::Geometry(_) | DragKind::Label { .. } => {
                    if drag.moved {
                        self.push_undo(drag.before);
                        self.resolve(item);
                    }
                }
            }
        }

        if primary_click {
            let hit = response
                .hover_pos()
                .and_then(|p| self.hit(&item.sketch, view, p, HitFilter::Any));
            let additive = modifiers.shift || modifiers.command;
            match hit {
                Some(sel) if additive => {
                    if let Some(i) = self.selection.iter().position(|s| *s == sel) {
                        self.selection.remove(i);
                    } else {
                        self.selection.push(sel);
                    }
                }
                Some(sel) => self.selection = vec![sel],
                None if !additive => self.selection.clear(),
                None => {}
            }
        }

        if response.double_clicked()
            && let Some(Sel::Constraint(c)) = self.hover
            && item
                .sketch
                .constraint(c)
                .is_some_and(|c| c.dimension.is_some())
        {
            self.open_dimension_editor(&item.sketch, c, None);
        }
    }

    fn start_drag(&mut self, view: &SketchView, item: &mut SketchItem, origin: Pos2) {
        let before = item.sketch.clone();
        let kind = match self.hit(&item.sketch, view, origin, HitFilter::Any) {
            Some(Sel::Entity(e)) => {
                let entity = item.sketch.entity(e);
                if entity.is_some_and(|x| x.locked) {
                    return;
                }
                let grab = view.to_sketch(origin);
                match (item.sketch.kind(e), grab) {
                    (Some(EntityKind::Point), Some(target)) => {
                        DragKind::Geometry(Drag::Point { point: e, target })
                    }
                    (Some(_), Some(grab)) => DragKind::Geometry(Drag::Curve {
                        curve: e,
                        grab,
                        target: grab,
                    }),
                    _ => return,
                }
            }
            Some(Sel::Constraint(c)) => {
                let Some(dim) = item.sketch.constraint(c).and_then(|c| c.dimension.as_ref()) else {
                    return;
                };
                let Some(grab) = view.to_sketch(origin) else {
                    return;
                };
                DragKind::Label {
                    dimension: c,
                    start_offset: dim.label_offset,
                    grab,
                }
            }
            None => DragKind::Box { start: origin },
        };
        self.drag = Some(DragState {
            kind,
            before,
            moved: false,
        });
    }

    /// Selects everything whose on-screen shape lies entirely inside `rect`.
    fn box_select(&mut self, sketch: &Sketch, view: &SketchView, rect: Rect, additive: bool) {
        if !additive {
            self.selection.clear();
        }
        let tol = view.px() * 0.5;
        for (id, e) in sketch.entities() {
            if e.locked {
                continue;
            }
            let inside = match e.geometry {
                peet_sketch::Geometry::Point { pos } => {
                    // Curve endpoints come along with their curve; only free points on their own.
                    e.owner.is_none() && rect.contains(view.to_screen(pos))
                }
                _ => sketch.curve(id).is_some_and(|c| {
                    c.tessellate(tol)
                        .into_iter()
                        .all(|p| rect.contains(view.to_screen(p)))
                }),
            };
            if inside && !self.selection.contains(&Sel::Entity(id)) {
                self.selection.push(Sel::Entity(id));
            }
        }
    }

    // ---- Trim, extend and fillet: click on a curve or corner ----

    fn pick_tool_input(
        &mut self,
        view: &SketchView,
        item: &mut SketchItem,
        pos: Option<Pos2>,
        click: bool,
    ) {
        let filter = if self.tool == Tool::Fillet {
            HitFilter::Points
        } else {
            HitFilter::Curves
        };
        self.hover = pos.and_then(|p| self.hit(&item.sketch, view, p, filter));
        if !click {
            return;
        }
        let (Some(Sel::Entity(target)), Some(cursor)) = (self.hover, self.cursor) else {
            return;
        };
        match self.tool {
            Tool::Trim => {
                self.checked_edit(item, "Trim", |s| {
                    peet_sketch::ops::trim(s, target, cursor).map_err(|e| e.to_string())
                });
            }
            Tool::Extend => {
                self.checked_edit(item, "Extend", |s| {
                    peet_sketch::ops::extend(s, target, cursor).map_err(|e| e.to_string())
                });
            }
            Tool::Fillet => {
                let r = self.options.fillet_radius;
                self.checked_edit(item, "Fillet", |s| {
                    peet_sketch::ops::fillet(s, target, r).map_err(|e| e.to_string())
                });
            }
            _ => {}
        }
        self.hover = None;
    }

    // ---- Smart dimension ----

    fn dimension_input(
        &mut self,
        view: &SketchView,
        item: &mut SketchItem,
        pos: Option<Pos2>,
        click: bool,
    ) {
        self.hover = pos.and_then(|p| self.hit(&item.sketch, view, p, HitFilter::Geometry));
        self.dim_picks.retain(|e| item.sketch.entity(*e).is_some());
        if !click {
            return;
        }
        match self.hover {
            Some(Sel::Entity(e)) => {
                if self.dim_picks.contains(&e) {
                    return;
                }
                if self.dim_picks.len() >= 2 {
                    self.dim_picks.clear();
                }
                self.dim_picks.push(e);
            }
            _ => {
                let Some(cursor) = self.cursor else {
                    return;
                };
                if let Some(kind) = self.pending_dimension(&item.sketch) {
                    self.place_dimension(item, kind, cursor);
                }
                self.dim_picks.clear();
            }
        }
    }

    /// The dimension the current picks and cursor position would create.
    pub(super) fn pending_dimension(&self, sketch: &Sketch) -> Option<ConstraintKind> {
        use ConstraintKind as C;
        use EntityKind as K;
        let kinds: Vec<K> = self
            .dim_picks
            .iter()
            .filter_map(|e| sketch.kind(*e))
            .collect();
        let p = &self.dim_picks;
        match kinds.as_slice() {
            [K::Line] => Some(C::Length(p[0])),
            [K::Circle] => Some(C::Diameter(p[0])),
            [K::Arc] => Some(C::Radius(p[0])),
            [K::Point, K::Point] => {
                let (a, b) = (sketch.point(p[0]), sketch.point(p[1]));
                let c = self.cursor?;
                let (lo, hi) = (a.min(b), a.max(b));
                let outside_x = c.x < lo.x || c.x > hi.x;
                let outside_y = c.y < lo.y || c.y > hi.y;
                Some(if outside_y && !outside_x {
                    C::HorizontalDistance(p[0], p[1])
                } else if outside_x && !outside_y {
                    C::VerticalDistance(p[0], p[1])
                } else {
                    C::Distance(p[0], p[1])
                })
            }
            [K::Point, K::Line] => Some(C::Distance(p[0], p[1])),
            [K::Line, K::Point] => Some(C::Distance(p[1], p[0])),
            [K::Line, K::Line] => {
                let d1 = sketch.curve(p[0])?.line_direction()?;
                let d2 = sketch.curve(p[1])?.line_direction()?;
                if d1.perp_dot(d2).abs() < 1e-9 {
                    let (s, _) = sketch.endpoints(p[1])?;
                    Some(C::Distance(s, p[0]))
                } else {
                    Some(C::Angle(p[0], p[1]))
                }
            }
            [K::Point, k] if k.is_circular() => Some(C::Distance(p[0], sketch.center(p[1])?)),
            [k, K::Point] if k.is_circular() => Some(C::Distance(p[1], sketch.center(p[0])?)),
            [K::Line, k] if k.is_circular() => Some(C::Distance(sketch.center(p[1])?, p[0])),
            [k, K::Line] if k.is_circular() => Some(C::Distance(sketch.center(p[0])?, p[1])),
            [a, b] if a.is_circular() && b.is_circular() => {
                Some(C::Distance(sketch.center(p[0])?, sketch.center(p[1])?))
            }
            _ => None,
        }
    }

    fn place_dimension(&mut self, item: &mut SketchItem, kind: ConstraintKind, at: DVec2) {
        let before = item.sketch.clone();
        let Some(value) = item.sketch.measure(&kind) else {
            return;
        };
        let id = match item.sketch.add_dimension(kind, value) {
            Ok(id) => id,
            Err(e) => {
                self.error(format!("Dimension: {e}"));
                return;
            }
        };
        let anchor = dimension_anchor(&item.sketch, &kind).unwrap_or(at);
        if let Some(d) = item
            .sketch
            .constraint_mut(id)
            .and_then(|c| c.dimension.as_mut())
        {
            d.label_offset = at - anchor;
        }
        let previous = self.analysis.diagnoses.len();
        self.resolve(item);
        if self.analysis.diagnoses.len() > previous {
            // Already defined: keep it as a driven (reference) dimension, like SolidWorks.
            if let Some(d) = item
                .sketch
                .constraint_mut(id)
                .and_then(|c| c.dimension.as_mut())
            {
                d.driving = false;
            }
            self.resolve(item);
            self.info("That's already defined by other relations, so the dimension was added as a driven (reference) dimension.");
            self.push_undo(before);
        } else {
            self.open_dimension_editor(&item.sketch, id, Some(before));
        }
        self.selection = vec![Sel::Constraint(id)];
    }

    pub(super) fn open_dimension_editor(
        &mut self,
        sketch: &Sketch,
        id: peet_sketch::ConstraintId,
        before: Option<Sketch>,
    ) {
        let Some(d) = sketch.constraint(id).and_then(|c| c.dimension.as_ref()) else {
            return;
        };
        if !d.driving {
            self.info("Driven dimensions only measure. Make it driving in the properties panel to edit its value.");
            if let Some(b) = before {
                self.push_undo(b);
            }
            return;
        }
        let text = d.expression.clone().unwrap_or_else(|| {
            super::dim_value_text(&sketch.constraint(id).expect("checked above").kind, d.value)
        });
        self.edit = Some(DimEdit {
            dimension: id,
            text,
            focus: true,
            before,
        });
    }

    // ---- Drawing tools ----

    fn inference_settings(&self, ui: &Ui, view: &SketchView) -> InferenceSettings {
        InferenceSettings {
            snap_distance: SNAP_PX * view.px(),
            angle_tolerance_deg: 3.0,
            // Hold Ctrl to place a point without automatic relations.
            enabled: self.options.inference && !ui.input(|i| i.modifiers.command),
        }
    }

    /// Where the segment being drawn starts, for direction inference.
    fn anchor(&self, sketch: &Sketch) -> Option<Anchor> {
        if self.tool != Tool::Line {
            return None;
        }
        if let Some(chain) = self.chain {
            return Some(Anchor {
                pos: sketch.try_point(chain.last_end)?,
                point: Some(chain.last_end),
                curve: Some(chain.last_line),
            });
        }
        let first = self.clicks.first()?;
        Some(Anchor {
            pos: first.pos,
            point: first.inference.relations.iter().find_map(|r| match r {
                Inferred::Coincident(p) => Some(*p),
                _ => None,
            }),
            curve: None,
        })
    }

    fn draw_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        view: &SketchView,
        item: &mut SketchItem,
        click: bool,
    ) {
        let Some(cursor) = self.cursor.filter(|_| response.hover_pos().is_some()) else {
            return;
        };
        let settings = self.inference_settings(ui, view);
        let anchor = self.anchor(&item.sketch);
        let inference = infer::infer(&item.sketch, cursor, anchor, &settings, &[]);

        // Track which way the cursor travels around an arc's centre.
        if self.tool == Tool::Arc && self.clicks.len() == 2 {
            let center = self.clicks[0].pos;
            let angle = (inference.pos - center).to_angle();
            if let Some(last) = self.arc_last_angle {
                let mut d = angle - last;
                if d > PI {
                    d -= TAU;
                } else if d < -PI {
                    d += TAU;
                }
                self.arc_sweep = (self.arc_sweep + d).clamp(-TAU, TAU);
            }
            self.arc_last_angle = Some(angle);
        }

        if response.double_clicked() && self.tool == Tool::Line {
            self.cancel_tool();
            return;
        }
        if click {
            self.place_click(view, item, inference.clone());
        }
        self.inference = Some(inference);
    }

    fn place_click(&mut self, view: &SketchView, item: &mut SketchItem, inference: Inference) {
        let pos = inference.pos;
        let tiny = |a: DVec2, b: DVec2| a.distance(b) < MIN_SIZE_PX * view.px();
        if self.tool == Tool::Line {
            let start = match self.chain {
                Some(chain) => item.sketch.try_point(chain.last_end),
                None => self.clicks.first().map(|c| c.pos),
            };
            match start {
                None => self.clicks.push(Click { pos, inference }),
                Some(start) if tiny(start, pos) => {}
                Some(_) => self.commit_line(item, Click { pos, inference }),
            }
            return;
        }
        // Reject clicks that would make a degenerate shape.
        if let Some(last) = self.clicks.last()
            && tiny(last.pos, pos)
        {
            return;
        }
        if self.tool == Tool::Arc && self.clicks.len() == 1 {
            self.arc_sweep = 0.0;
            self.arc_last_angle = Some((pos - self.clicks[0].pos).to_angle());
        }
        self.clicks.push(Click { pos, inference });
        if self.clicks.len() >= self.tool.clicks_needed() {
            let clicks = std::mem::take(&mut self.clicks);
            self.commit_shape(view, item, &clicks);
        }
    }

    fn commit_line(&mut self, item: &mut SketchItem, end: Click) {
        let before = item.sketch.clone();
        let chain = self.chain;
        let start_click = if chain.is_none() {
            self.clicks.first().cloned()
        } else {
            None
        };
        let start_pos = match (chain, &start_click) {
            (Some(c), _) => item.sketch.point(c.last_end),
            (None, Some(c)) => c.pos,
            (None, None) => return,
        };
        let line = item.sketch.add_line(start_pos, end.pos);
        item.sketch
            .set_construction(line, self.options.construction);
        let (a, b) = item.sketch.endpoints(line).expect("a line");
        if let Some(c) = chain {
            let _ = item
                .sketch
                .add_constraint(ConstraintKind::Coincident(c.last_end, a));
        } else if let Some(click) = &start_click {
            self.apply_inferred(item, &click.inference.relations, a, None);
        }
        self.apply_inferred(item, &end.inference.relations, b, Some(line));
        self.push_undo(before);
        self.resolve(item);

        let first_point = chain.map_or(a, |c| c.first_point);
        let first_pos = item.sketch.try_point(first_point);
        let closed = end.inference.relations.iter().any(|r| {
            matches!(r, Inferred::Coincident(p)
                if *p == first_point || item.sketch.try_point(*p).zip(first_pos).is_some_and(|(q, f)| q.distance(f) < 1e-9))
        });
        self.clicks.clear();
        self.chain = if closed {
            None
        } else {
            Some(Chain {
                last_line: line,
                last_end: b,
                first_point,
            })
        };
    }

    fn commit_shape(&mut self, view: &SketchView, item: &mut SketchItem, clicks: &[Click]) {
        let before = item.sketch.clone();
        let s = &mut item.sketch;
        // Each click's inferred relations go to the new point nearest to where it was placed.
        let mut targets: Vec<(usize, Option<EntityId>, Option<EntityId>)> = Vec::new();
        let mut new_curves: Vec<EntityId> = Vec::new();
        match self.tool {
            Tool::Point => {
                let p = s.add_point(clicks[0].pos);
                targets.push((0, Some(p), None));
            }
            Tool::Rectangle | Tool::CenterRectangle => {
                let (a, b) = (clicks[0].pos, clicks[1].pos);
                let size = (b - a).abs();
                if size.min_element() < MIN_SIZE_PX * view.px() {
                    self.error("The rectangle needs a width and a height.");
                    return;
                }
                let shape = if self.tool == Tool::Rectangle {
                    shapes::rectangle(s, a, b)
                } else {
                    shapes::center_rectangle(s, a, b)
                };
                let first = if self.tool == Tool::CenterRectangle {
                    shape.points.first().copied()
                } else {
                    nearest_new_point(s, &shape.curves, a)
                };
                targets.push((0, first, None));
                targets.push((1, nearest_new_point(s, &shape.curves, b), None));
                new_curves = shape.curves;
            }
            Tool::Circle => {
                let c = s.add_circle(clicks[0].pos, clicks[0].pos.distance(clicks[1].pos));
                targets.push((0, s.center(c), None));
                // A circle drawn through an existing point stays on it.
                for r in &clicks[1].inference.relations {
                    if let Inferred::Coincident(p) = r {
                        let _ = s.add_constraint(ConstraintKind::PointOnCurve {
                            point: *p,
                            curve: c,
                        });
                    }
                }
                new_curves.push(c);
            }
            Tool::Arc => {
                let (c, p1, p2) = (clicks[0].pos, clicks[1].pos, clicks[2].pos);
                let arc = if self.arc_sweep >= 0.0 {
                    s.add_arc(c, p1, p2)
                } else {
                    s.add_arc(c, p2, p1)
                };
                targets.push((0, s.center(arc), None));
                targets.push((1, nearest_new_point(s, &[arc], p1), Some(arc)));
                targets.push((2, nearest_new_point(s, &[arc], p2), Some(arc)));
                new_curves.push(arc);
            }
            Tool::Slot => {
                let (c1, c2) = (clicks[0].pos, clicks[1].pos);
                let axis = peet_sketch::Curve::Line { a: c1, b: c2 };
                let r = axis.distance_to_line(clicks[2].pos);
                if r < MIN_SIZE_PX * view.px() {
                    self.error("The slot needs a width.");
                    return;
                }
                let shape = shapes::slot(s, c1, c2, r);
                targets.push((0, nearest_new_point(s, &shape.curves, c1), None));
                targets.push((1, nearest_new_point(s, &shape.curves, c2), None));
                new_curves = shape.curves;
            }
            Tool::Polygon => {
                let (c, v) = (clicks[0].pos, clicks[1].pos);
                let shape = shapes::polygon(s, c, v, self.options.polygon_sides);
                targets.push((0, nearest_new_point(s, &shape.curves, c), None));
                targets.push((1, nearest_new_point(s, &shape.curves, v), None));
                new_curves = shape.curves;
            }
            _ => return,
        }
        if self.options.construction {
            for c in &new_curves {
                item.sketch.set_construction(*c, true);
            }
        }
        for (i, point, segment) in targets {
            if let Some(point) = point {
                self.apply_inferred(item, &clicks[i].inference.relations, point, segment);
            }
        }
        self.push_undo(before);
        self.resolve(item);
    }

    /// Adds inferred relations one at a time, skipping any that would over-define the
    /// sketch (inference is a convenience; it must never create a broken sketch).
    fn apply_inferred(
        &mut self,
        item: &mut SketchItem,
        relations: &[Inferred],
        point: EntityId,
        segment: Option<EntityId>,
    ) {
        let baseline = self.analysis.diagnoses.len();
        for r in relations {
            let snapshot = item.sketch.clone();
            let added = infer::apply(&mut item.sketch, std::slice::from_ref(r), point, segment);
            if added.is_empty() {
                continue;
            }
            let report = self.solver.solve(&mut item.sketch);
            let analysis = self.solver.analyze(&item.sketch);
            if !report.converged || analysis.diagnoses.len() > baseline {
                item.sketch = snapshot;
            }
        }
    }

    /// Shape preview for the current tool from its clicks to the cursor, in sketch space.
    pub(super) fn preview(&self, sketch: &Sketch) -> Vec<peet_sketch::Curve> {
        use peet_sketch::Curve;
        let Some(inf) = &self.inference else {
            return Vec::new();
        };
        let cur = inf.pos;
        let line = |a, b| Curve::Line { a, b };
        let rect = |a: DVec2, b: DVec2| {
            let (lo, hi) = (a.min(b), a.max(b));
            let c = [lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)];
            (0..4)
                .map(|i| line(c[i], c[(i + 1) % 4]))
                .collect::<Vec<_>>()
        };
        let clicks: Vec<DVec2> = self.clicks.iter().map(|c| c.pos).collect();
        match (self.tool, clicks.as_slice()) {
            (Tool::Line, _) => {
                let start = self
                    .chain
                    .and_then(|c| sketch.try_point(c.last_end))
                    .or(clicks.first().copied());
                start.map(|s| vec![line(s, cur)]).unwrap_or_default()
            }
            (Tool::Rectangle, [a]) => rect(*a, cur),
            (Tool::CenterRectangle, [c]) => {
                let mut v = rect(*c * 2.0 - cur, cur);
                v.push(line(*c * 2.0 - cur, cur));
                v
            }
            (Tool::Circle, [c]) => vec![Curve::Circle {
                center: *c,
                radius: c.distance(cur),
            }],
            (Tool::Arc, [c]) => vec![line(*c, cur)],
            (Tool::Arc, [c, s]) => {
                let end = *c + (cur - *c).normalize_or(DVec2::X) * c.distance(*s);
                let arc = if self.arc_sweep >= 0.0 {
                    Curve::arc_from_points(*c, *s, end)
                } else {
                    Curve::arc_from_points(*c, end, *s)
                };
                vec![line(*c, *s), arc]
            }
            (Tool::Slot, [a]) => vec![line(*a, cur)],
            (Tool::Slot, [a, b]) => {
                let axis = line(*a, *b);
                let r = axis.distance_to_line(cur);
                let dir = (*b - *a).normalize_or(DVec2::X);
                let n = dir.perp() * r;
                let start = dir.perp().to_angle();
                vec![
                    line(*a - n, *b - n),
                    line(*a + n, *b + n),
                    Curve::Arc {
                        center: *b,
                        radius: r,
                        start_angle: start - PI,
                        sweep: PI,
                    },
                    Curve::Arc {
                        center: *a,
                        radius: r,
                        start_angle: start,
                        sweep: PI,
                    },
                ]
            }
            (Tool::Polygon, [c]) => {
                let n = self.options.polygon_sides.max(3);
                let r = c.distance(cur);
                let a0 = (cur - *c).to_angle();
                let pts: Vec<DVec2> = (0..n)
                    .map(|i| *c + DVec2::from_angle(a0 + TAU * i as f64 / n as f64) * r)
                    .collect();
                let mut v: Vec<Curve> = (0..n).map(|i| line(pts[i], pts[(i + 1) % n])).collect();
                v.push(Curve::Circle {
                    center: *c,
                    radius: r,
                });
                v
            }
            _ => Vec::new(),
        }
    }
}

/// The point of `curves` (their endpoints and centres) nearest `pos`.
fn nearest_new_point(sketch: &Sketch, curves: &[EntityId], pos: DVec2) -> Option<EntityId> {
    curves
        .iter()
        .filter_map(|c| sketch.entity(*c))
        .flat_map(|e| e.geometry.points())
        .min_by(|a, b| {
            let da = sketch.point(*a).distance_squared(pos);
            let db = sketch.point(*b).distance_squared(pos);
            da.total_cmp(&db)
        })
}
