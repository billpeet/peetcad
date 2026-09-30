//! Hit testing: what is under the cursor.

use egui::Pos2;
use peet_math::DVec2;
use peet_sketch::{EntityId, EntityKind, Sketch};

use super::{Sel, SketchEditor, SketchView};

/// Pick radius for points, in screen points.
pub const POINT_PICK_PX: f64 = 7.0;
/// Pick distance for curves, in screen points.
pub const CURVE_PICK_PX: f64 = 5.0;

/// Which kinds of things a hit test may return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitFilter {
    /// Geometry, dimension labels and relation glyphs.
    Any,
    /// Points and curves only.
    Geometry,
    /// Lines, arcs and circles only.
    Curves,
    /// Points only.
    Points,
}

impl SketchEditor {
    pub(super) fn hit(
        &self,
        sketch: &Sketch,
        view: &SketchView,
        pos: Pos2,
        filter: HitFilter,
    ) -> Option<Sel> {
        if filter == HitFilter::Any
            && let Some((sel, _)) = self
                .hit_rects
                .iter()
                .rev()
                .find(|(_, r)| r.expand(2.0).contains(pos))
        {
            return Some(*sel);
        }
        let cursor = view.to_sketch(pos)?;
        if filter != HitFilter::Curves
            && let Some(p) = nearest_point(sketch, view, pos)
        {
            return Some(Sel::Entity(p));
        }
        if filter != HitFilter::Points {
            return nearest_curve(sketch, cursor, CURVE_PICK_PX * view.px()).map(Sel::Entity);
        }
        None
    }
}

/// The point entity nearest `pos` within the pick radius. Standalone points and endpoints
/// win over centre points at equal distance, so a centre never hides an endpoint on top of it.
pub fn nearest_point(sketch: &Sketch, view: &SketchView, pos: Pos2) -> Option<EntityId> {
    let mut best: Option<(f64, EntityId)> = None;
    for (id, e) in sketch.entities() {
        let peet_sketch::Geometry::Point { pos: p } = e.geometry else {
            continue;
        };
        let d = f64::from(view.to_screen(p).distance(pos));
        let is_centre = e.owner.is_some_and(|o| sketch.center(o) == Some(id));
        let d = if is_centre { d + 0.5 } else { d };
        if d <= POINT_PICK_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id)
}

/// The curve nearest `p` (sketch coordinates) within `max_dist` (sketch units).
pub fn nearest_curve(sketch: &Sketch, p: DVec2, max_dist: f64) -> Option<EntityId> {
    let mut best: Option<(f64, EntityId)> = None;
    for (id, e) in sketch.entities() {
        if e.kind() == EntityKind::Point {
            continue;
        }
        let Some(curve) = sketch.curve(id) else {
            continue;
        };
        let d = curve.distance(p);
        if d <= max_dist && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id)
}
