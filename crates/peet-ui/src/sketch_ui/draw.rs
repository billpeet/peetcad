//! Painting the sketch being edited: geometry coloured by constraint state, relation
//! glyphs, dimensions, tool previews and inference hints.

use std::collections::HashMap;

use egui::{Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use peet_math::DVec2;
use peet_sketch::infer::Inferred;
use peet_sketch::solver::{ConstraintStatus, DofStatus};
use peet_sketch::{ConstraintId, ConstraintKind, Curve, EntityId, EntityKind, Geometry, Sketch};

use super::{Sel, SketchEditor, SketchView, Tool, dof_color, format_value};

const SELECTED: Color32 = Color32::from_rgb(255, 150, 30);
const PREVIEW: Color32 = Color32::from_rgb(255, 170, 60);
const RELATION: Color32 = Color32::from_rgb(40, 160, 70);
const CONFLICT: Color32 = Color32::from_rgb(229, 72, 77);
const GLYPH_SIZE: f32 = 16.0;

/// The small pictures used for relation glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Tangent,
    Equal,
    Concentric,
    Midpoint,
    Symmetric,
    Fix,
    OnCurve,
    Coincident,
    Center,
    AlignH,
    AlignV,
}

struct Palette {
    fg_under: Color32,
    fg_fully: Color32,
    dimension: Color32,
    driven: Color32,
    glyph_bg: Color32,
    label_bg: Color32,
}

impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                fg_under: dof_color(DofStatus::Under, true),
                fg_fully: dof_color(DofStatus::Fully, true),
                dimension: Color32::from_rgb(214, 218, 226),
                driven: Color32::from_rgb(140, 146, 156),
                glyph_bg: Color32::from_rgba_unmultiplied(34, 38, 44, 225),
                label_bg: Color32::from_rgba_unmultiplied(30, 33, 38, 200),
            }
        } else {
            Self {
                fg_under: dof_color(DofStatus::Under, false),
                fg_fully: dof_color(DofStatus::Fully, false),
                dimension: Color32::from_rgb(52, 56, 64),
                driven: Color32::from_rgb(128, 132, 140),
                glyph_bg: Color32::from_rgba_unmultiplied(250, 250, 252, 235),
                label_bg: Color32::from_rgba_unmultiplied(248, 249, 251, 210),
            }
        }
    }
}

impl SketchEditor {
    pub(super) fn paint(
        &mut self,
        painter: &Painter,
        view: &SketchView,
        sketch: &Sketch,
        dark: bool,
    ) {
        self.hit_rects.clear();
        let pal = Palette::new(dark);
        let tol = view.px() * 0.35;

        // Entities that should look highlighted: selected, hovered, or tied to a hovered
        // or selected relation.
        let mut highlighted: HashMap<EntityId, Color32> = HashMap::new();
        let hover_color = SELECTED.gamma_multiply(0.75);
        let mut mark = |sel: Sel, color: Color32| match sel {
            Sel::Entity(e) => {
                highlighted.insert(e, color);
            }
            Sel::Constraint(c) => {
                if let Some(c) = sketch.constraint(c) {
                    for e in c.kind.entities() {
                        highlighted.entry(e).or_insert(color);
                    }
                }
            }
        };
        if let Some(h) = self.hover {
            let color = if self.tool == Tool::Trim {
                CONFLICT
            } else {
                hover_color
            };
            mark(h, color);
        }
        for s in &self.selection {
            mark(*s, SELECTED);
        }
        for e in &self.dim_picks {
            mark(Sel::Entity(*e), SELECTED);
        }

        // Closed regions, lightly shaded.
        let fill = if dark {
            Color32::from_rgba_unmultiplied(110, 168, 255, 26)
        } else {
            Color32::from_rgba_unmultiplied(28, 104, 226, 22)
        };
        let mut mesh = egui::Mesh::default();
        for t in &self.fills {
            let base = mesh.vertices.len() as u32;
            for p in &t.points {
                mesh.colored_vertex(view.to_screen(*p), fill);
            }
            for tri in &t.triangles {
                mesh.add_triangle(base + tri[0], base + tri[1], base + tri[2]);
            }
        }
        if !mesh.is_empty() {
            painter.add(Shape::mesh(mesh));
        }

        // Curves first, then points on top.
        for (id, e) in sketch.entities() {
            let Some(curve) = sketch.curve(id) else {
                continue;
            };
            let base = self.entity_color(id, &pal);
            let (color, width) = match highlighted.get(&id) {
                Some(c) => (*c, 2.6),
                None => (base, 1.6),
            };
            let pts: Vec<Pos2> = curve
                .tessellate(tol)
                .into_iter()
                .map(|p| view.to_screen(p))
                .collect();
            let stroke = Stroke::new(width, color);
            if e.construction {
                painter.extend(Shape::dashed_line(&pts, stroke, 7.0, 4.0));
            } else {
                painter.add(Shape::line(pts, stroke));
            }
        }
        for (id, e) in sketch.entities() {
            let Geometry::Point { pos } = e.geometry else {
                continue;
            };
            let p = view.to_screen(pos);
            if id == Sketch::ORIGIN {
                let c = Color32::from_rgb(214, 70, 70);
                painter.line_segment(
                    [p - vec2(6.0, 0.0), p + vec2(6.0, 0.0)],
                    Stroke::new(1.5, c),
                );
                painter.line_segment(
                    [p - vec2(0.0, 6.0), p + vec2(0.0, 6.0)],
                    Stroke::new(1.5, c),
                );
                painter.circle_stroke(p, 3.5, Stroke::new(1.2, c));
                if let Some(h) = highlighted.get(&id) {
                    painter.circle_stroke(p, 6.0, Stroke::new(1.5, *h));
                }
                continue;
            }
            let base = self.entity_color(id, &pal);
            let is_centre = e.owner.is_some_and(|o| sketch.center(o) == Some(id));
            let r = if e.owner.is_none() { 3.2 } else { 2.4 };
            match highlighted.get(&id) {
                Some(c) => {
                    painter.circle_filled(p, r + 1.6, *c);
                }
                None if is_centre => {
                    painter.line_segment(
                        [p - vec2(3.0, 0.0), p + vec2(3.0, 0.0)],
                        Stroke::new(1.2, base),
                    );
                    painter.line_segment(
                        [p - vec2(0.0, 3.0), p + vec2(0.0, 3.0)],
                        Stroke::new(1.2, base),
                    );
                }
                None => {
                    painter.circle_filled(p, r, base);
                }
            }
        }

        if self.options.show_relations {
            self.paint_relations(painter, view, sketch, &pal);
        }
        self.paint_dimensions(painter, view, sketch, &pal);
        self.paint_tool_overlay(painter, view, sketch, &pal);
    }

    fn entity_color(&self, id: EntityId, pal: &Palette) -> Color32 {
        match self.analysis.entity_status(id) {
            DofStatus::Under => pal.fg_under,
            DofStatus::Fully => pal.fg_fully,
            DofStatus::Over => CONFLICT,
        }
    }

    // ---- Relation glyphs ----

    fn paint_relations(
        &mut self,
        painter: &Painter,
        view: &SketchView,
        sketch: &Sketch,
        pal: &Palette,
    ) {
        // Glyphs are grouped by the entity they sit next to, then laid out in a row.
        let mut rows: HashMap<EntityId, Vec<(ConstraintId, Icon)>> = HashMap::new();
        let mut order: Vec<EntityId> = Vec::new();
        for (cid, c) in sketch.constraints() {
            if c.dimension.is_some() {
                continue;
            }
            let (icon, at): (Icon, Vec<EntityId>) = match c.kind {
                ConstraintKind::Horizontal(l) => (Icon::Horizontal, vec![l]),
                ConstraintKind::Vertical(l) => (Icon::Vertical, vec![l]),
                ConstraintKind::HorizontalPoints(a, b) => (Icon::Horizontal, vec![a, b]),
                ConstraintKind::VerticalPoints(a, b) => (Icon::Vertical, vec![a, b]),
                ConstraintKind::Parallel(a, b) => (Icon::Parallel, vec![a, b]),
                ConstraintKind::Perpendicular(a, b) => (Icon::Perpendicular, vec![a, b]),
                ConstraintKind::Tangent(a, b) => (Icon::Tangent, vec![a, b]),
                ConstraintKind::Equal(a, b) => (Icon::Equal, vec![a, b]),
                ConstraintKind::Concentric(a, b) => (Icon::Concentric, vec![a, b]),
                ConstraintKind::Midpoint { point, .. } => (Icon::Midpoint, vec![point]),
                ConstraintKind::Symmetric { a, b, .. } => (Icon::Symmetric, vec![a, b]),
                ConstraintKind::Fix { point, .. } => (Icon::Fix, vec![point]),
                ConstraintKind::PointOnCurve { point, .. } => (Icon::OnCurve, vec![point]),
                // Coincident points are visibly joined; showing a glyph for each would
                // clutter every corner. They're listed in the properties panel instead.
                _ => continue,
            };
            for e in at {
                if !rows.contains_key(&e) {
                    order.push(e);
                }
                rows.entry(e).or_default().push((cid, icon));
            }
        }
        for e in order {
            let Some(anchor) = glyph_anchor(sketch, view, e) else {
                continue;
            };
            for (i, (cid, icon)) in rows[&e].iter().enumerate() {
                let center = anchor + vec2(i as f32 * (GLYPH_SIZE + 2.0), 0.0);
                let rect = Rect::from_center_size(center, Vec2::splat(GLYPH_SIZE));
                let status = self.analysis.constraint_status(*cid);
                let sel = Sel::Constraint(*cid);
                let color = if status != ConstraintStatus::Ok {
                    CONFLICT
                } else if self.selection.contains(&sel) || self.hover == Some(sel) {
                    SELECTED
                } else {
                    RELATION
                };
                draw_glyph(painter, rect, *icon, color, pal.glyph_bg);
                self.hit_rects.push((sel, rect));
            }
        }
    }

    // ---- Dimensions ----

    fn paint_dimensions(
        &mut self,
        painter: &Painter,
        view: &SketchView,
        sketch: &Sketch,
        pal: &Palette,
    ) {
        for (cid, c) in sketch.constraints() {
            let Some(d) = &c.dimension else {
                continue;
            };
            let Some(anchor) = dimension_anchor(sketch, &c.kind) else {
                continue;
            };
            let sel = Sel::Constraint(cid);
            let status = self.analysis.constraint_status(cid);
            let color = if status != ConstraintStatus::Ok {
                CONFLICT
            } else if self.selection.contains(&sel) || self.hover == Some(sel) {
                SELECTED
            } else if d.driving {
                pal.dimension
            } else {
                pal.driven
            };
            let label = anchor + d.label_offset;
            let mut text = dimension_text(&c.kind, d.value);
            if d.expression.is_some() {
                text = format!("ƒ {text}");
            }
            if !d.driving {
                text = format!("({text})");
            }
            // Hide the label while its value is being edited (the editor sits there).
            let editing = self.edit.as_ref().is_some_and(|e| e.dimension == cid);
            if let Some(rect) = draw_dimension(
                painter, view, sketch, &c.kind, label, &text, color, pal, editing,
            ) {
                self.hit_rects.push((sel, rect));
            }
        }
    }

    // ---- Tool previews and inference ----

    fn paint_tool_overlay(
        &self,
        painter: &Painter,
        view: &SketchView,
        sketch: &Sketch,
        pal: &Palette,
    ) {
        let tol = view.px() * 0.35;
        let stroke = Stroke::new(1.6, PREVIEW);
        for curve in self.preview(sketch) {
            let pts: Vec<Pos2> = curve
                .tessellate(tol)
                .into_iter()
                .map(|p| view.to_screen(p))
                .collect();
            if self.options.construction {
                painter.extend(Shape::dashed_line(&pts, stroke, 7.0, 4.0));
            } else {
                painter.add(Shape::line(pts, stroke));
            }
        }
        for c in &self.clicks {
            painter.circle_filled(view.to_screen(c.pos), 3.0, PREVIEW);
        }

        if let Some(inf) = &self.inference {
            let p = view.to_screen(inf.pos);
            let mut icons = Vec::new();
            for r in &inf.relations {
                let (icon, guide_from) = match *r {
                    Inferred::Coincident(_) => (Icon::Coincident, None),
                    Inferred::OnCurve(_) => (Icon::OnCurve, None),
                    Inferred::Midpoint(_) => (Icon::Midpoint, None),
                    Inferred::Center(_) => (Icon::Center, None),
                    Inferred::Horizontal => (Icon::Horizontal, None),
                    Inferred::Vertical => (Icon::Vertical, None),
                    Inferred::Tangent(_) => (Icon::Tangent, None),
                    Inferred::Perpendicular(_) => (Icon::Perpendicular, None),
                    Inferred::Parallel(_) => (Icon::Parallel, None),
                    Inferred::AlignedHorizontally(q) => (Icon::AlignH, sketch.try_point(q)),
                    Inferred::AlignedVertically(q) => (Icon::AlignV, sketch.try_point(q)),
                };
                if let Some(from) = guide_from {
                    let guide = Stroke::new(1.0, PREVIEW.gamma_multiply(0.8));
                    painter.extend(Shape::dashed_line(
                        &[view.to_screen(from), p],
                        guide,
                        4.0,
                        4.0,
                    ));
                }
                if matches!(
                    r,
                    Inferred::Coincident(_) | Inferred::Midpoint(_) | Inferred::Center(_)
                ) {
                    painter.circle_stroke(p, 6.0, Stroke::new(1.5, PREVIEW));
                }
                icons.push(icon);
            }
            for (i, icon) in icons.into_iter().enumerate() {
                let c = p + vec2(18.0 + i as f32 * (GLYPH_SIZE + 2.0), 18.0);
                draw_glyph(
                    painter,
                    Rect::from_center_size(c, Vec2::splat(GLYPH_SIZE)),
                    icon,
                    PREVIEW,
                    pal.glyph_bg,
                );
            }
        }

        // Smart dimension preview while picking.
        if self.tool == Tool::Dimension
            && let (Some(kind), Some(cursor)) = (self.pending_dimension(sketch), self.cursor)
            && let Some(value) = sketch.measure(&kind)
        {
            let text = dimension_text(&kind, value);
            draw_dimension(
                painter, view, sketch, &kind, cursor, &text, PREVIEW, pal, false,
            );
        }

        if let Some(super::DragState {
            kind: super::DragKind::Box { start },
            ..
        }) = &self.drag
            && let Some(end) = painter.ctx().input(|i| i.pointer.latest_pos())
        {
            let rect = Rect::from_two_pos(*start, end);
            painter.rect_filled(rect, 0.0, SELECTED.gamma_multiply(0.08));
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, SELECTED), StrokeKind::Inside);
        }
    }
}

/// Where a relation glyph row for an entity starts, on screen.
fn glyph_anchor(sketch: &Sketch, view: &SketchView, e: EntityId) -> Option<Pos2> {
    match sketch.kind(e)? {
        EntityKind::Point => Some(view.to_screen(sketch.point(e)) + vec2(12.0, -16.0)),
        _ => {
            let curve = sketch.curve(e)?;
            let (at, t) = match curve {
                Curve::Line { .. } | Curve::Arc { .. } => (curve.point_at(0.5), 0.5),
                Curve::Circle { .. } => (curve.point_at(0.125), 0.125),
            };
            let dir = view.screen_dir(at, curve.tangent_at(t));
            // Offset to the side of the curve, towards screen up/right.
            let mut normal = vec2(-dir.y, dir.x);
            if normal.y > 0.0 || (normal.y == 0.0 && normal.x < 0.0) {
                normal = -normal;
            }
            Some(view.to_screen(at) + normal * 16.0 + dir * 4.0)
        }
    }
}

/// The point a dimension's label offset is measured from.
pub fn dimension_anchor(sketch: &Sketch, kind: &ConstraintKind) -> Option<DVec2> {
    use ConstraintKind::*;
    match *kind {
        Distance(a, b) => {
            let (p, q) = distance_points(sketch, a, b)?;
            Some((p + q) * 0.5)
        }
        HorizontalDistance(a, b) | VerticalDistance(a, b) => {
            Some((sketch.try_point(a)? + sketch.try_point(b)?) * 0.5)
        }
        Length(l) => {
            let c = sketch.curve(l)?;
            Some(c.point_at(0.5))
        }
        Radius(c) | Diameter(c) => sketch.curve(c)?.center(),
        Angle(l1, l2) => {
            let a = sketch.curve(l1)?;
            let b = sketch.curve(l2)?;
            Some(
                a.intersect_unbounded(&b)
                    .first()
                    .map_or_else(|| a.point_at(0.5), |i| i.point),
            )
        }
        _ => None,
    }
}

/// The two points a distance dimension measures between (a point–line distance measures
/// to the foot of the perpendicular).
fn distance_points(sketch: &Sketch, a: EntityId, b: EntityId) -> Option<(DVec2, DVec2)> {
    match (sketch.kind(a)?, sketch.kind(b)?) {
        (EntityKind::Point, EntityKind::Point) => Some((sketch.point(a), sketch.point(b))),
        (EntityKind::Point, EntityKind::Line) => {
            let p = sketch.point(a);
            let l = sketch.curve(b)?;
            Some((p, l.point_at(l.project(p))))
        }
        (EntityKind::Line, EntityKind::Point) => {
            let p = sketch.point(b);
            let l = sketch.curve(a)?;
            Some((l.point_at(l.project(p)), p))
        }
        _ => None,
    }
}

fn dimension_text(kind: &ConstraintKind, value: f64) -> String {
    match kind {
        ConstraintKind::Radius(_) => format!("R{}", format_value(value)),
        ConstraintKind::Diameter(_) => format!("Ø{}", format_value(value)),
        ConstraintKind::Angle(..) => format!("{}°", format_value(value)),
        _ => format_value(value),
    }
}

/// Draws a dimension with its label at `label` (sketch coordinates). Returns the label's
/// screen rectangle.
#[allow(clippy::too_many_arguments)]
fn draw_dimension(
    painter: &Painter,
    view: &SketchView,
    sketch: &Sketch,
    kind: &ConstraintKind,
    label: DVec2,
    text: &str,
    color: Color32,
    pal: &Palette,
    hide_label: bool,
) -> Option<Rect> {
    use ConstraintKind::*;
    let s = |p: DVec2| view.to_screen(p);
    let stroke = Stroke::new(1.0, color);
    let px = view.px();
    match *kind {
        Distance(..) | Length(_) | HorizontalDistance(..) | VerticalDistance(..) => {
            let (p1, p2) = match *kind {
                Distance(a, b) => distance_points(sketch, a, b)?,
                Length(l) => {
                    let c = sketch.curve(l)?;
                    (c.start(), c.end())
                }
                HorizontalDistance(a, b) | VerticalDistance(a, b) => {
                    (sketch.try_point(a)?, sketch.try_point(b)?)
                }
                _ => unreachable!(),
            };
            let u = match *kind {
                HorizontalDistance(..) => DVec2::X,
                VerticalDistance(..) => DVec2::Y,
                _ => (p2 - p1).try_normalize().unwrap_or(DVec2::X),
            };
            let n = u.perp();
            let q1 = p1 + n * (label - p1).dot(n);
            let q2 = p2 + n * (label - p2).dot(n);
            // Extension lines from the geometry to just past the dimension line.
            for (p, q) in [(p1, q1), (p2, q2)] {
                let d = q - p;
                if d.length() > px * 3.0 {
                    let dir = d.normalize();
                    painter.line_segment([s(p + dir * px * 3.0), s(q + dir * px * 5.0)], stroke);
                }
            }
            // Dimension line, extended to reach the label if it sits outside.
            let t = (label - q1).dot(u);
            let span = (q2 - q1).dot(u);
            let (lo, hi) = (t.min(0.0).min(span), t.max(0.0).max(span));
            painter.line_segment([s(q1 + u * lo), s(q1 + u * hi)], stroke);
            let out = (q1 - q2).try_normalize().unwrap_or(-u);
            arrow(painter, view, q1, out, color);
            arrow(painter, view, q2, -out, color);
        }
        Radius(c) | Diameter(c) => {
            let curve = sketch.curve(c)?;
            let (center, r) = (curve.center()?, curve.radius()?);
            let d = (label - center).try_normalize().unwrap_or(DVec2::X);
            let tip = center + d * r;
            let far = if label.distance(center) > r {
                label
            } else {
                tip
            };
            if matches!(kind, Diameter(_)) {
                let other = center - d * r;
                painter.line_segment([s(other), s(far)], stroke);
                arrow(painter, view, tip, d, color);
                arrow(painter, view, other, -d, color);
            } else {
                painter.line_segment([s(center), s(far)], stroke);
                arrow(painter, view, tip, d, color);
            }
        }
        Angle(l1, l2) => {
            let a = sketch.curve(l1)?;
            let b = sketch.curve(l2)?;
            let v = dimension_anchor(sketch, kind)?;
            let (d1, d2) = (a.line_direction()?, b.line_direction()?);
            let rho = label.distance(v).max(px * 12.0);
            let a0 = d1.to_angle();
            let sweep = d1.angle_to(d2);
            let n = 32;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| s(v + DVec2::from_angle(a0 + sweep * f64::from(i) / f64::from(n)) * rho))
                .collect();
            painter.add(Shape::line(pts, stroke));
            let e0 = v + d1 * rho;
            let e1 = v + d2 * rho;
            let t0 = d1.perp() * -sweep.signum();
            let t1 = d2.perp() * sweep.signum();
            arrow(painter, view, e0, t0, color);
            arrow(painter, view, e1, t1, color);
        }
        _ => return None,
    }
    let pos = s(label);
    let font = FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let rect = Rect::from_center_size(pos, galley.size()).expand2(vec2(4.0, 1.0));
    if !hide_label {
        painter.rect_filled(rect, 3.0, pal.label_bg);
        painter.galley(rect.min + vec2(4.0, 1.0), galley, color);
    }
    Some(rect)
}

/// A filled arrowhead with its tip at `tip`, pointing along `dir` (sketch space).
fn arrow(painter: &Painter, view: &SketchView, tip: DVec2, dir: DVec2, color: Color32) {
    let t = view.to_screen(tip);
    let d = view.screen_dir(tip, dir);
    if d == Vec2::ZERO {
        return;
    }
    let back = t - d * 9.0;
    let side = vec2(-d.y, d.x) * 3.2;
    painter.add(Shape::convex_polygon(
        vec![t, back + side, back - side],
        color,
        Stroke::NONE,
    ));
}

/// Draws a relation glyph: a small rounded box with an icon.
fn draw_glyph(painter: &Painter, rect: Rect, icon: Icon, color: Color32, bg: Color32) {
    painter.rect_filled(rect, 3.0, bg);
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        StrokeKind::Inside,
    );
    let c = rect.center();
    let st = Stroke::new(1.4, color);
    let l = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([c + vec2(a.0, a.1), c + vec2(b.0, b.1)], st);
    };
    match icon {
        Icon::Horizontal => l((-5.0, 0.0), (5.0, 0.0)),
        Icon::Vertical => l((0.0, -5.0), (0.0, 5.0)),
        Icon::Parallel => {
            l((-4.0, 4.0), (-1.0, -4.0));
            l((1.0, 4.0), (4.0, -4.0));
        }
        Icon::Perpendicular => {
            l((0.0, -5.0), (0.0, 4.0));
            l((-5.0, 4.0), (5.0, 4.0));
        }
        Icon::Tangent => {
            painter.circle_stroke(c + vec2(0.0, 1.5), 3.5, st);
            l((-5.5, -2.0), (5.5, -2.0));
        }
        Icon::Equal => {
            l((-4.5, -2.0), (4.5, -2.0));
            l((-4.5, 2.0), (4.5, 2.0));
        }
        Icon::Concentric => {
            painter.circle_stroke(c, 2.0, st);
            painter.circle_stroke(c, 5.0, st);
        }
        Icon::Midpoint => {
            l((-5.5, 2.0), (5.5, 2.0));
            painter.add(Shape::convex_polygon(
                vec![c + vec2(0.0, -3.5), c + vec2(3.0, 2.0), c + vec2(-3.0, 2.0)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Symmetric => {
            l((-4.0, -5.0), (-4.0, 5.0));
            l((4.0, -5.0), (4.0, 5.0));
            painter.circle_filled(c, 1.5, color);
        }
        Icon::Fix => {
            painter.add(Shape::convex_polygon(
                vec![c + vec2(0.0, -4.0), c + vec2(3.5, 2.5), c + vec2(-3.5, 2.5)],
                Color32::TRANSPARENT,
                st,
            ));
            l((-5.0, 4.5), (5.0, 4.5));
        }
        Icon::OnCurve => {
            let pts: Vec<Pos2> = (0..=8)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.1 + 0.8 * i as f32 / 8.0);
                    c + vec2(a.cos(), -a.sin()) * -5.5 + vec2(0.0, 5.0)
                })
                .collect();
            painter.add(Shape::line(pts, st));
            painter.circle_filled(c + vec2(0.0, -0.5), 2.0, color);
        }
        Icon::Coincident => {
            painter.circle_stroke(c, 4.0, st);
            painter.circle_filled(c, 1.8, color);
        }
        Icon::Center => {
            painter.circle_stroke(c, 4.5, st);
            l((-2.0, 0.0), (2.0, 0.0));
            l((0.0, -2.0), (0.0, 2.0));
        }
        Icon::AlignH => {
            painter.extend(Shape::dashed_line(
                &[c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0)],
                st,
                2.0,
                2.0,
            ));
        }
        Icon::AlignV => {
            painter.extend(Shape::dashed_line(
                &[c + vec2(0.0, -6.0), c + vec2(0.0, 6.0)],
                st,
                2.0,
                2.0,
            ));
        }
    }
    let _ = pos2;
}
