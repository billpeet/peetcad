//! A placeholder document, until the parametric model (`peet-model`, Phase 3) exists.
//!
//! It holds what every new part starts with in a CAD tool (the origin and the three
//! standard reference planes) plus a demo body, so the feature tree, properties panel,
//! selection highlighting and viewport can be built and tested end to end now.

use peet_math::{Aabb, Plane};
use peet_render::MeshData;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefPlane {
    Front,
    Top,
    Right,
}

impl RefPlane {
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
}

impl ItemKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Origin => "Origin",
            Self::ReferencePlane(_) => "Reference plane",
            Self::Body { .. } => "Solid body",
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
        }
    }
}

impl Document {
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
