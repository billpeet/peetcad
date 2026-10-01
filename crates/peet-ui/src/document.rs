//! A placeholder document, until the parametric model (`peet-model`, Phase 3) exists.
//!
//! It holds what every new part starts with in a CAD tool (the origin and the three
//! standard reference planes), a demo body, and the user's sketches, so the feature tree,
//! properties panel, selection highlighting, viewport and sketcher can be built and
//! tested end to end now.

use peet_math::{Aabb, Plane};
use peet_render::MeshData;
use peet_sketch::Sketch;
use peet_sketch::expr::Parameters;
use peet_sketch::solver::DofStatus;

use crate::bodies::Body;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefPlane {
    Front,
    Top,
    Right,
}

impl RefPlane {
    pub const ALL: [Self; 3] = [Self::Front, Self::Top, Self::Right];

    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front Plane",
            Self::Top => "Top Plane",
            Self::Right => "Right Plane",
        }
    }

    pub fn plane(self) -> Plane {
        match self {
            Self::Front => Plane::front(),
            Self::Top => Plane::TOP,
            Self::Right => Plane::right(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    /// The origin point and axes.
    Origin,
    ReferencePlane(RefPlane),
    /// A body with a display mesh.
    Body {
        mesh: MeshData,
    },
    Sketch(Box<SketchItem>),
    Extrude(Box<ExtrudeItem>),
}

/// An extrude (or cut-extrude) feature.
#[derive(Clone, Debug)]
pub struct ExtrudeItem {
    /// The sketch whose regions are extruded.
    pub sketch: ItemId,
    pub feature: peet_model::Extrude,
    /// Why the last rebuild of this feature failed, if it did.
    pub error: Option<String>,
}

/// A sketch placed on a plane.
#[derive(Clone, Debug)]
pub struct SketchItem {
    pub plane: Plane,
    /// Name of the plane the sketch lies on, for the properties panel.
    pub plane_name: String,
    pub sketch: Sketch,
    /// Summary of the last analysis, for the feature tree marker.
    pub status: SketchStatus,
}

/// How well defined a sketch is, as shown in the feature tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SketchStatus {
    #[default]
    Under,
    Fully,
    Over,
}

impl SketchStatus {
    /// Tree prefix, as in SolidWorks: "(-)" under defined, "(+)" over defined.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Under => "(-) ",
            Self::Fully => "",
            Self::Over => "(+) ",
        }
    }
}

impl From<DofStatus> for SketchStatus {
    fn from(s: DofStatus) -> Self {
        match s {
            DofStatus::Under => Self::Under,
            DofStatus::Fully => Self::Fully,
            DofStatus::Over => Self::Over,
        }
    }
}

impl ItemKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Origin => "Origin",
            Self::ReferencePlane(_) => "Reference plane",
            Self::Body { .. } => "Solid body",
            Self::Sketch(_) => "Sketch",
            Self::Extrude(e) if e.feature.operation == peet_model::Operation::Cut => "Cut-extrude",
            Self::Extrude(_) => "Extrude",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub kind: ItemKind,
    pub visible: bool,
}

#[derive(Clone, Debug)]
pub struct Document {
    pub name: String,
    pub items: Vec<Item>,
    /// Named parameters usable in dimension expressions.
    pub parameters: Parameters,
    /// The solids the features build, in feature order (rebuilt by [`Document::rebuild`]).
    pub bodies: Vec<Body>,
    /// Incremented on every rebuild, so the viewport knows to re-upload meshes.
    pub revision: u64,
    next_id: u32,
    next_sketch_number: u32,
    next_extrude_number: u32,
}

pub const DEMO_BODY: ItemId = ItemId(100);

impl Default for Document {
    fn default() -> Self {
        let item = |id, name: &str, kind| Item {
            id: ItemId(id),
            name: name.to_owned(),
            kind,
            visible: true,
        };
        Self {
            name: "Part1".to_owned(),
            items: vec![
                item(0, "Origin", ItemKind::Origin),
                item(1, "Front Plane", ItemKind::ReferencePlane(RefPlane::Front)),
                item(2, "Top Plane", ItemKind::ReferencePlane(RefPlane::Top)),
                item(3, "Right Plane", ItemKind::ReferencePlane(RefPlane::Right)),
                Item {
                    visible: false,
                    ..item(
                        DEMO_BODY.0,
                        "Demo U-Channel",
                        ItemKind::Body {
                            mesh: peet_render::mesh::demo::u_channel(),
                        },
                    )
                },
            ],
            parameters: Parameters::default(),
            bodies: Vec::new(),
            revision: 0,
            next_id: 1000,
            next_sketch_number: 1,
            next_extrude_number: 1,
        }
    }
}

impl Document {
    /// Adds an empty sketch on `plane` and returns its id.
    pub fn add_sketch(&mut self, plane: Plane, plane_name: &str) -> ItemId {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        let name = format!("Sketch{}", self.next_sketch_number);
        self.next_sketch_number += 1;
        self.items.push(Item {
            id,
            name,
            kind: ItemKind::Sketch(Box::new(SketchItem {
                plane,
                plane_name: plane_name.to_owned(),
                sketch: Sketch::new(),
                status: SketchStatus::Under,
            })),
            visible: true,
        });
        id
    }

    /// Adds an extrude feature using `sketch`, after the existing items, and rebuilds.
    pub fn add_extrude(&mut self, sketch: ItemId, operation: peet_model::Operation) -> ItemId {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        let prefix = if operation == peet_model::Operation::Cut {
            "Cut-Extrude"
        } else {
            "Extrude"
        };
        let name = format!("{prefix}{}", self.next_extrude_number);
        self.next_extrude_number += 1;
        let mut feature = peet_model::Extrude::new(operation);
        if operation == peet_model::Operation::Add && self.bodies.is_empty() {
            feature.operation = peet_model::Operation::NewBody;
        }
        self.items.push(Item {
            id,
            name,
            kind: ItemKind::Extrude(Box::new(ExtrudeItem {
                sketch,
                feature,
                error: None,
            })),
            visible: true,
        });
        // A sketch used by a feature is hidden, as in other CAD tools.
        if let Some(item) = self.item_mut(sketch) {
            item.visible = false;
        }
        self.rebuild();
        id
    }

    pub fn extrude_mut(&mut self, id: ItemId) -> Option<&mut ExtrudeItem> {
        match &mut self.item_mut(id)?.kind {
            ItemKind::Extrude(e) => Some(e),
            _ => None,
        }
    }

    /// Re-runs every feature in order, rebuilding the bodies and their display meshes.
    /// A failing feature is skipped (and keeps its error), so the rest still builds.
    pub fn rebuild(&mut self) {
        let mut solids: Vec<peet_kernel::Solid> = Vec::new();
        let mut errors: Vec<(ItemId, Option<String>)> = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            let ItemKind::Extrude(ex) = &item.kind else {
                continue;
            };
            let sketch = self.items[..index].iter().find_map(|i| match &i.kind {
                ItemKind::Sketch(s) if i.id == ex.sketch => Some(s),
                _ => None,
            });
            let result = match sketch {
                None => Err("Its sketch was deleted, or comes after it in the tree.".to_owned()),
                Some(s) => peet_model::apply_extrude(&mut solids, &s.plane, &s.sketch, &ex.feature)
                    .map_err(|e| e.0),
            };
            errors.push((item.id, result.err()));
        }
        for (id, error) in errors {
            if let Some(ex) = self.extrude_mut(id) {
                ex.error = error;
            }
        }
        self.bodies = solids.into_iter().map(Body::new).collect();
        self.revision += 1;
        // A body that can't be displayed is a bug, but must not go unnoticed.
        if let Some(e) = self.bodies.iter().find_map(|b| b.error.clone())
            && let Some(last) = self.items.iter_mut().rev().find_map(|i| match &mut i.kind {
                ItemKind::Extrude(x) if x.error.is_none() => Some(x),
                _ => None,
            })
        {
            last.error = Some(format!("The result can't be displayed: {e}"));
        }
    }

    pub fn sketch(&self, id: ItemId) -> Option<&SketchItem> {
        match &self.item(id)?.kind {
            ItemKind::Sketch(s) => Some(s),
            _ => None,
        }
    }

    pub fn remove_item(&mut self, id: ItemId) {
        self.items.retain(|i| i.id != id);
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Bounds of all visible bodies (reference geometry is excluded, as in other CAD tools'
    /// "zoom to fit").
    pub fn visible_body_bounds(&self) -> Aabb {
        let meshes = self
            .items
            .iter()
            .filter(|i| i.visible)
            .filter_map(|i| match &i.kind {
                ItemKind::Body { mesh } => Some(mesh.bounds()),
                _ => None,
            });
        let solids = self.bodies.iter().map(|b| b.solid.bounds());
        meshes.chain(solids).fold(Aabb::EMPTY, |a, b| a.union(&b))
    }

    /// Bounds of all visible bodies and sketches: what "zoom to fit" frames.
    pub fn visible_bounds(&self) -> Aabb {
        let mut bounds = self.visible_body_bounds();
        for item in self.items.iter().filter(|i| i.visible) {
            if let ItemKind::Sketch(s) = &item.kind {
                bounds = bounds.union(&sketch_bounds(s));
            }
        }
        bounds
    }

    /// Whether all three reference planes are visible.
    pub fn planes_visible(&self) -> bool {
        self.items
            .iter()
            .filter(|i| matches!(i.kind, ItemKind::ReferencePlane(_)))
            .all(|i| i.visible)
    }

    pub fn set_planes_visible(&mut self, visible: bool) {
        for item in &mut self.items {
            if matches!(item.kind, ItemKind::ReferencePlane(_)) {
                item.visible = visible;
            }
        }
    }
}

/// Model-space bounds of a sketch's geometry (the origin point alone counts as empty).
pub fn sketch_bounds(item: &SketchItem) -> Aabb {
    let mut bounds = Aabb::EMPTY;
    for (id, e) in item.sketch.entities() {
        if id == Sketch::ORIGIN {
            continue;
        }
        let (lo, hi) = match item.sketch.curve(id) {
            Some(c) => c.bounds(),
            None => match e.geometry {
                peet_sketch::Geometry::Point { pos } => (pos, pos),
                _ => continue,
            },
        };
        for corner in [
            lo,
            hi,
            peet_math::DVec2::new(lo.x, hi.y),
            peet_math::DVec2::new(hi.x, lo.y),
        ] {
            bounds.extend(item.plane.from_plane_coords(corner));
        }
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::DVec2;

    fn doc_with_extruded_rectangle() -> (Document, ItemId, ItemId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(Plane::TOP, "Top Plane");
        if let Some(Item {
            kind: ItemKind::Sketch(s),
            ..
        }) = doc.item_mut(sketch)
        {
            peet_sketch::shapes::rectangle(&mut s.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
        }
        let ex = doc.add_extrude(sketch, peet_model::Operation::Add);
        (doc, sketch, ex)
    }

    #[test]
    fn extrude_builds_a_body_and_hides_its_sketch() {
        let (doc, sketch, ex) = doc_with_extruded_rectangle();
        assert_eq!(doc.bodies.len(), 1);
        let ItemKind::Extrude(e) = &doc.item(ex).unwrap().kind else {
            panic!()
        };
        assert_eq!(e.error, None);
        assert_eq!(
            e.feature.operation,
            peet_model::Operation::NewBody,
            "first body"
        );
        assert!(!doc.item(sketch).unwrap().visible);
        let body = &doc.bodies[0];
        assert!(!body.mesh.indices.is_empty());
        assert_eq!(body.mesh.pick_ids.len(), body.mesh.vertices.len());
        assert_eq!(body.mesh.edge_pick_ids.len() * 2, body.mesh.edges.len());
        let size = doc.visible_body_bounds().size();
        assert!((size.x - 40.0).abs() < 1e-9 && (size.z - 10.0).abs() < 1e-9);
    }

    #[test]
    fn failing_feature_keeps_the_rest() {
        let (mut doc, sketch, ex) = doc_with_extruded_rectangle();
        let rev = doc.revision;
        doc.remove_item(sketch);
        doc.rebuild();
        assert!(doc.revision > rev);
        let ItemKind::Extrude(e) = &doc.item(ex).unwrap().kind else {
            panic!()
        };
        assert!(e.error.as_deref().unwrap().contains("sketch"));
        assert!(doc.bodies.is_empty());
        // Zero depth explains itself.
        let (mut doc, _, ex) = doc_with_extruded_rectangle();
        doc.extrude_mut(ex).unwrap().feature.depth = 0.0;
        doc.rebuild();
        let e = doc.extrude_mut(ex).unwrap();
        assert!(
            e.error.as_deref().unwrap().contains("depth"),
            "{:?}",
            e.error
        );
    }
}
