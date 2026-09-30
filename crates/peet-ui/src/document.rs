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
    next_id: u32,
    next_sketch_number: u32,
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
                item(
                    DEMO_BODY.0,
                    "Demo U-Channel",
                    ItemKind::Body {
                        mesh: peet_render::mesh::demo::u_channel(),
                    },
                ),
            ],
            parameters: Parameters::default(),
            next_id: 1000,
            next_sketch_number: 1,
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
        self.items
            .iter()
            .filter(|i| i.visible)
            .filter_map(|i| match &i.kind {
                ItemKind::Body { mesh } => Some(mesh.bounds()),
                _ => None,
            })
            .fold(Aabb::EMPTY, |a, b| a.union(&b))
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
