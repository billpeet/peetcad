//! Bodies as the document shows them: the model's body (or its flat pattern) plus its
//! tessellation, made on demand.

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use peet_kernel::query::{MassProperties, mass_properties};
use peet_kernel::tessellate::{Silhouettes, SolidMesh, tessellate, tessellate_with};
use peet_kernel::{EdgeId, FaceId, VertexId};

/// A face, edge or vertex of a body, for picking and selection. Only valid for the
/// current rebuild: across rebuilds, selections go through persistent references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeomRef {
    Face { body: usize, face: FaceId },
    Edge { body: usize, edge: EdgeId },
    Vertex { body: usize, vertex: VertexId },
}

impl GeomRef {
    pub fn body(self) -> usize {
        match self {
            Self::Face { body, .. } | Self::Edge { body, .. } | Self::Vertex { body, .. } => body,
        }
    }
}

/// What is made from a body the first time its triangles are needed.
#[derive(Debug)]
struct Display {
    tess: SolidMesh,
    silhouettes: Silhouettes,
    error: Option<String>,
}

#[derive(Debug)]
pub struct BodyView {
    /// What is shown: the model's body, or for a sheet metal body in the flat pattern
    /// view, its flat pattern (same topology, so face and edge ids are the same).
    pub body: Arc<peet_model::Body>,
    /// The model's body. References (to faces, edges, vertices) are made from this one,
    /// so they are the same whichever view they were picked in.
    pub source: Arc<peet_model::Body>,
    /// Showing the flat pattern.
    pub flat: bool,
    display: OnceLock<Display>,
    /// A coarser mesh for when the body is small on screen, made the first time one is
    /// asked for. `None` if it couldn't be made (the fine one is used then).
    coarse: OnceLock<Option<SolidMesh>>,
    /// The box around what is shown, worked out the first time it is asked for.
    bounds: OnceLock<peet_math::Aabb>,
    /// The mass properties of `source`, worked out the first time they are asked for.
    mass: OnceLock<Result<MassProperties, String>>,
}

impl Deref for BodyView {
    type Target = peet_model::Body;

    fn deref(&self) -> &Self::Target {
        &self.body
    }
}

/// The tessellation tolerance for a solid: finer for small parts.
pub fn tolerance(solid: &peet_kernel::Solid) -> f64 {
    let size = solid.bounds().size().length();
    (size * 5e-4).clamp(0.005, 0.1)
}

/// The largest arc a facet of the coarse mesh spans: three times the usual.
const COARSE_ANGLE: f64 = 30.0 * std::f64::consts::PI / 180.0;

/// The tolerance of the coarse mesh: a hundredth of the body's size, which is under a
/// pixel while the body is drawn smaller than a hundred pixels.
pub fn coarse_tolerance(solid: &peet_kernel::Solid) -> f64 {
    let size = solid.bounds().size().length();
    (size * 0.01).max(tolerance(solid) * 4.0)
}

/// Distinguishes a flat pattern's display stamp from its body's.
const FLAT_STAMP: u64 = 0x9e37_79b9_7f4a_7c15;

impl BodyView {
    /// The view of a model body: its flat pattern if `flat` is set and it is sheet metal,
    /// else the body itself.
    pub fn of(source: Arc<peet_model::Body>, flat: bool) -> Self {
        Self::of_cached(source, flat, None)
    }

    /// [`BodyView::of`], with a mesh of the model body from a file to use in place of
    /// tessellating it (not for a flat pattern, which the file has no mesh of).
    pub fn of_cached(source: Arc<peet_model::Body>, flat: bool, cached: Option<SolidMesh>) -> Self {
        match (&source.sheet, flat) {
            (Some(sheet), true) => {
                let shown = Arc::new(peet_model::Body {
                    solid: sheet.flat.clone(),
                    face_names: source.face_names.clone(),
                    origin: source.origin,
                    stamp: source.stamp ^ FLAT_STAMP,
                    sheet: source.sheet.clone(),
                });
                let mut v = Self::new(shown, None);
                v.source = source;
                v.flat = true;
                v
            }
            _ => Self::new(source, cached),
        }
    }

    /// The stamp the view is cached by: the model body's, marked when flat.
    pub fn key(source: &peet_model::Body, flat: bool) -> u64 {
        if flat && source.sheet.is_some() {
            source.stamp ^ FLAT_STAMP
        } else {
            source.stamp
        }
    }

    /// The view of `body`. `cached` is a mesh of this exact body from a file, used in
    /// place of tessellating it if it fits.
    pub fn new(body: Arc<peet_model::Body>, cached: Option<SolidMesh>) -> Self {
        let display = OnceLock::new();
        if let Some(tess) = cached.filter(|t| t.faces.len() == body.solid.faces.len()) {
            let _ = display.set(Display {
                tess,
                silhouettes: Silhouettes::new(&body.solid),
                error: None,
            });
        }
        Self {
            source: body.clone(),
            flat: false,
            body,
            display,
            coarse: OnceLock::new(),
            bounds: OnceLock::new(),
            mass: OnceLock::new(),
        }
    }

    fn display(&self) -> &Display {
        self.display.get_or_init(|| {
            let solid = &self.body.solid;
            let (tess, error) = match tessellate(solid, tolerance(solid)) {
                Ok(t) => (t, None),
                Err(e) => (SolidMesh::default(), Some(e.to_string())),
            };
            Display {
                tess,
                silhouettes: Silhouettes::new(solid),
                error,
            }
        })
    }

    /// The body's triangles and edge polylines, by face and edge. Made on first use and
    /// kept; empty if tessellation failed (see [`BodyView::error`]).
    pub fn tess(&self) -> &SolidMesh {
        &self.display().tess
    }

    /// The box around what is shown, in the body's own coordinates. Kept: an assembly
    /// asks for the boxes of all its bodies every frame.
    pub fn bounds(&self) -> peet_math::Aabb {
        *self.bounds.get_or_init(|| self.body.solid.bounds())
    }

    /// A coarser mesh of the body, with the same faces and edges, for drawing it small:
    /// made on first use and kept. The fine mesh if a coarser one can't be made.
    pub fn coarse(&self) -> &SolidMesh {
        self.coarse
            .get_or_init(|| {
                let solid = &self.body.solid;
                tessellate_with(solid, coarse_tolerance(solid), COARSE_ANGLE).ok()
            })
            .as_ref()
            .unwrap_or_else(|| self.tess())
    }

    /// Precomputed data for the view-dependent silhouette lines of curved faces.
    pub fn silhouettes(&self) -> &Silhouettes {
        &self.display().silhouettes
    }

    /// Why the body couldn't be tessellated, if it couldn't.
    pub fn error(&self) -> Option<&str> {
        self.display().error.as_deref()
    }

    /// Whether the body has been tessellated yet.
    pub fn is_tessellated(&self) -> bool {
        self.display.get().is_some()
    }

    /// Volume, area, centre of gravity and inertia of the model's body (the folded part,
    /// also while its flat pattern is shown), for a density of 1 and in millimetres.
    /// Worked out on first use and kept: views are reused for as long as the body's stamp
    /// stays the same, so a body is measured once per change to it. It can take a tenth
    /// of a second for a curved body.
    pub fn mass_properties(&self) -> Result<&MassProperties, &str> {
        self.mass
            .get_or_init(|| mass_properties(&self.source.solid).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(String::as_str)
    }

    /// Whether [`BodyView::mass_properties`] has its answer already.
    pub fn is_measured(&self) -> bool {
        self.mass.get().is_some()
    }
}
