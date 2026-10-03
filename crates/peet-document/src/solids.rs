//! General solid modelling on the document: revolves, sweeps, fillets and chamfers, shells,
//! draft, holes and imported bodies, each added as one undo step, and the measurements taken
//! from the bodies (what a face or an edge is, the distance and angle between two, mass
//! properties).
//!
//! Nothing here knows about a user interface: a command of the application and a script
//! call the same functions. What is picked is passed in as [`GeomRef`]s (the current
//! rebuild's faces and edges), which are turned into references that survive rebuilds.

use peet_kernel::Surface;
use peet_kernel::query::{self, Between, Description, Entity, MassProperties};
use peet_math::{Aabb, DMat3, DVec3};
use peet_model::{
    BlendKind, EdgeRef, FaceRef, FeatureId, FeatureKind, ImportedSolid, Operation, PlaneRef,
};

use crate::body::GeomRef;
use crate::document::Document;

/// What a STEP import added to the document.
#[derive(Clone, Debug, PartialEq)]
pub struct StepImported {
    /// The new feature that holds the imported bodies.
    pub feature: FeatureId,
    /// Its name: the file's name without the extension.
    pub name: String,
    /// How many solids came in.
    pub bodies: usize,
    /// The importer's notes for the user: assumptions made and what was left out.
    pub warnings: Vec<String>,
}

/// Mass properties of several bodies taken together, for a density of 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassTotal {
    /// mm³.
    pub volume: f64,
    /// mm².
    pub area: f64,
    /// The centre of gravity of all the bodies.
    pub centroid: DVec3,
    /// The principal moments of inertia about that centre (ascending), mm⁵.
    pub principal_moments: [f64; 3],
    pub bounds: Aabb,
}

impl MassTotal {
    /// The bodies of `parts` as one. `None` if there are none, or they have no volume.
    pub fn of<'a>(parts: impl IntoIterator<Item = &'a MassProperties>) -> Option<Self> {
        let parts: Vec<&MassProperties> = parts.into_iter().collect();
        let volume: f64 = parts.iter().map(|p| p.volume).sum();
        if parts.is_empty() || volume.abs() < f64::MIN_POSITIVE {
            return None;
        }
        let centroid = parts.iter().map(|p| p.centroid * p.volume).sum::<DVec3>() / volume;
        // Each body's inertia moved to the common centre (the parallel axis theorem).
        let mut inertia = DMat3::ZERO;
        for p in &parts {
            let d = p.centroid - centroid;
            let shift = DMat3::from_diagonal(DVec3::splat(d.length_squared()))
                - DMat3::from_cols(d * d.x, d * d.y, d * d.z);
            inertia += p.inertia + shift * p.volume;
        }
        Some(Self {
            volume,
            area: parts.iter().map(|p| p.area).sum(),
            centroid,
            principal_moments: symmetric_eigenvalues(&inertia),
            bounds: parts
                .iter()
                .fold(Aabb::EMPTY, |all, p| all.union(&p.bounds)),
        })
    }
}

/// Eigenvalues of a symmetric 3×3 matrix, ascending (the trigonometric solution of its
/// characteristic cubic).
fn symmetric_eigenvalues(m: &DMat3) -> [f64; 3] {
    let a = m.to_cols_array_2d();
    let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
    let trace = a[0][0] + a[1][1] + a[2][2];
    let mut values = if off <= 1e-30 * trace * trace {
        [a[0][0], a[1][1], a[2][2]]
    } else {
        let q = trace / 3.0;
        let spread = (a[0][0] - q).powi(2) + (a[1][1] - q).powi(2) + (a[2][2] - q).powi(2);
        let p = ((spread + 2.0 * off) / 6.0).sqrt();
        let b = (*m - DMat3::from_diagonal(DVec3::splat(q))) * (1.0 / p);
        let phi = (b.determinant() / 2.0).clamp(-1.0, 1.0).acos() / 3.0;
        let largest = q + 2.0 * p * phi.cos();
        let smallest = q + 2.0 * p * (phi + std::f64::consts::TAU / 3.0).cos();
        [smallest, 3.0 * q - largest - smallest, largest]
    };
    values.sort_by(f64::total_cmp);
    values
}

/// `stem`, or `stem` with the first number that makes it a name no feature has.
fn unique_name(model: &peet_model::Model, stem: &str) -> String {
    let taken = |name: &str| model.features().any(|f| f.name == name);
    if !taken(stem) {
        return stem.to_owned();
    }
    (2..)
        .map(|n| format!("{stem} ({n})"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| stem.to_owned())
}

/// A file's name without its folder and extension: "bracket" for "C:\parts\bracket.step".
fn file_stem(file_name: &str) -> &str {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

impl Document {
    fn entity(&self, g: GeomRef) -> Option<(&peet_kernel::Solid, Entity)> {
        let body = self.bodies.get(g.body())?;
        Some((
            &body.solid,
            match g {
                GeomRef::Face { face, .. } => Entity::Face(face),
                GeomRef::Edge { edge, .. } => Entity::Edge(edge),
                GeomRef::Vertex { vertex, .. } => Entity::Vertex(vertex),
            },
        ))
    }

    /// The exact measurements of a face, an edge or a vertex as it is shown: an edge's
    /// length (and radius and centre if it is round), a face's area (and radius), a
    /// vertex's position.
    pub fn describe(&self, g: GeomRef) -> Option<Description> {
        let (solid, entity) = self.entity(g)?;
        Some(query::describe(solid, entity))
    }

    /// The distance and the angle between two faces, edges or vertices, of one body or of
    /// two.
    pub fn measure_between(&self, a: GeomRef, b: GeomRef) -> Option<Between> {
        Some(query::between(self.entity(a)?, self.entity(b)?))
    }

    /// References to the edges among `picked`, for a fillet or a chamfer.
    pub fn edge_refs(&self, picked: &[GeomRef]) -> Vec<EdgeRef> {
        picked
            .iter()
            .filter_map(|g| match *g {
                GeomRef::Edge { body, edge } => self.bodies.get(body)?.source.edge_ref(edge),
                _ => None,
            })
            .collect()
    }

    /// References to the faces among `picked`: all of them, or only the flat ones.
    pub fn face_refs(&self, picked: &[GeomRef], flat_only: bool) -> Vec<FaceRef> {
        picked
            .iter()
            .filter_map(|g| match *g {
                GeomRef::Face { body, face } => {
                    let b = &self.bodies.get(body)?.source;
                    let flat = matches!(b.solid.face(face).surface, Surface::Plane(_));
                    (flat || !flat_only).then(|| b.face_ref(face))
                }
                _ => None,
            })
            .collect()
    }

    /// Adds a feature as one undo step.
    fn add_feature(
        &mut self,
        label: &str,
        add: impl FnOnce(&mut peet_model::Model) -> FeatureId,
    ) -> Option<FeatureId> {
        let mut id = None;
        self.change(label, |m| id = Some(add(m)));
        id
    }

    /// Revolves `sketch` about its centreline (its first construction line), or about its
    /// vertical axis if it has none: a full turn, to be changed afterwards. The first
    /// body of a part is always a new body.
    pub fn add_revolve(&mut self, sketch: FeatureId, operation: Operation) -> Option<FeatureId> {
        self.model.sketch(sketch)?;
        let operation = if operation == Operation::Add && self.evaluation().bodies.is_empty() {
            Operation::NewBody
        } else {
            operation
        };
        let label = if operation == Operation::Cut {
            "Add Cut-Revolve"
        } else {
            "Add Revolve"
        };
        self.add_feature(label, |m| m.add_revolve(sketch, operation))
    }

    /// Sweeps the sketch `profile` along the path drawn in the sketch `path` (none: to be
    /// chosen afterwards). The first body of a part is always a new body.
    pub fn add_sweep(
        &mut self,
        profile: FeatureId,
        path: Option<FeatureId>,
        operation: Operation,
    ) -> Option<FeatureId> {
        self.model.sketch(profile)?;
        let path = path.filter(|p| *p != profile && self.model.sketch(*p).is_some());
        let operation = if operation == Operation::Add && self.evaluation().bodies.is_empty() {
            Operation::NewBody
        } else {
            operation
        };
        let label = if operation == Operation::Cut {
            "Add Cut-Sweep"
        } else {
            "Add Sweep"
        };
        self.add_feature(label, |m| m.add_sweep(profile, path, operation))
    }

    /// The sketches a sweep's path can be: those before the sweep `feature` in the tree,
    /// other than its profile.
    pub fn sweep_path_choices(&self, feature: FeatureId) -> Vec<FeatureId> {
        let before = self.model.index_of(feature).unwrap_or(usize::MAX);
        let profile = match self.feature(feature).map(|f| &f.kind) {
            Some(FeatureKind::Sweep(s)) => Some(s.profile),
            _ => None,
        };
        self.model
            .features()
            .enumerate()
            .filter(|(i, f)| {
                *i < before && matches!(f.kind, FeatureKind::Sketch(_)) && Some(f.id) != profile
            })
            .map(|(_, f)| f.id)
            .collect()
    }

    /// Adds a fillet or a chamfer on `edges` (none: to be picked afterwards).
    pub fn add_blend(&mut self, kind: BlendKind, edges: Vec<EdgeRef>) -> Option<FeatureId> {
        let label = match kind {
            BlendKind::Fillet => "Add Fillet",
            BlendKind::Chamfer => "Add Chamfer",
        };
        self.add_feature(label, |m| m.add_blend(kind, edges))
    }

    /// Hollows the bodies, removing the faces `open` (none: closed hollows).
    pub fn add_shell(&mut self, open: Vec<FaceRef>) -> Option<FeatureId> {
        self.add_feature("Add Shell", |m| m.add_shell(open))
    }

    /// Tapers the flat `faces` about the plane `neutral` (none: to be picked afterwards).
    pub fn add_draft(
        &mut self,
        faces: Vec<FaceRef>,
        neutral: Option<PlaneRef>,
    ) -> Option<FeatureId> {
        self.add_feature("Add Draft", |m| m.add_draft(faces, neutral))
    }

    /// Drills a hole at every point of `sketch` (an M6 clearance hole through everything,
    /// to be changed afterwards).
    pub fn add_hole(&mut self, sketch: FeatureId) -> Option<FeatureId> {
        self.model.sketch(sketch)?;
        self.add_feature("Add Hole", |m| m.add_hole(sketch))
    }

    /// Adds the solids of a STEP file (its text) as bodies, in one feature named after
    /// the file. The error says, in plain words, why nothing was imported.
    pub fn import_step(&mut self, file_name: &str, text: &str) -> Result<StepImported, String> {
        let read = peet_io::step_import::read(text).map_err(|e| e.to_string())?;
        if read.bodies.is_empty() {
            let mut message =
                "No solid in this STEP file could be imported, so nothing was added.".to_owned();
            for w in &read.warnings {
                message.push(' ');
                message.push_str(w);
            }
            return Err(message);
        }
        let solids: Vec<ImportedSolid> = read
            .bodies
            .into_iter()
            .map(|b| ImportedSolid {
                name: b.name,
                solid: b.solid,
            })
            .collect();
        let bodies = solids.len();
        let source = file_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file_name)
            .to_owned();
        let stem = file_stem(file_name);
        let stem = if stem.trim().is_empty() {
            read.product_name.trim()
        } else {
            stem.trim()
        };
        let name = (!stem.is_empty()).then(|| unique_name(&self.model, stem));
        let mut added = None;
        self.change("Import STEP", |m| {
            let id = m.add_import(source, solids);
            if let (Some(name), Some(f)) = (&name, m.feature_mut(id)) {
                f.name.clone_from(name);
            }
            added = Some(id);
        });
        let feature = added.ok_or_else(|| "The import changed nothing.".to_owned())?;
        Ok(StepImported {
            feature,
            name: self.model.name_of(feature).to_owned(),
            bodies,
            warnings: read.warnings,
        })
    }

    /// The straight lines of a sketch that a revolve can turn about, construction lines
    /// (centrelines) first: each with whether it is a construction line and its length.
    pub fn revolve_axis_lines(&self, sketch: FeatureId) -> Vec<(peet_sketch::EntityId, bool, f64)> {
        let Some(s) = self.model.sketch(sketch) else {
            return Vec::new();
        };
        let mut lines: Vec<(peet_sketch::EntityId, bool, f64)> = s
            .sketch
            .entities()
            .filter_map(|(id, e)| match s.sketch.curve(id) {
                Some(peet_sketch::Curve::Line { a, b }) => {
                    Some((id, e.construction, a.distance(b)))
                }
                _ => None,
            })
            .collect();
        // Stable: within each group the lines stay in the order they were drawn.
        lines.sort_by_key(|(_, construction, _)| !construction);
        lines
    }

    /// Where the holes of a hole feature's sketch go: how many there are.
    pub fn hole_count(&self, sketch: FeatureId) -> usize {
        self.model
            .sketch(sketch)
            .map_or(0, |s| peet_model::hole::hole_positions(&s.sketch).len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::{DVec2, Plane};
    use peet_model::{Scalar, Status, StdPlane};

    /// A 40 × 20 × 10 block on the top plane.
    fn block() -> (Document, FeatureId) {
        let mut doc = Document::default();
        let mut ex = FeatureId(0);
        doc.change("Add", |m| {
            let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
            }
            ex = m.add_extrude(s, Operation::Add);
        });
        (doc, ex)
    }

    fn top_face(doc: &Document) -> GeomRef {
        let body = &doc.bodies[0];
        let face = body
            .solid
            .face_ids()
            .find(|f| body.face_center(*f).z > 9.0)
            .expect("the block has a top face");
        GeomRef::Face { body: 0, face }
    }

    /// An edge of the block's top face that runs along X.
    fn top_edge(doc: &Document) -> GeomRef {
        let body = &doc.bodies[0];
        let edge = body
            .solid
            .edge_ids()
            .find(|e| {
                let m = body.solid.edge(*e).point_at_fraction(0.5);
                (m.z - 10.0).abs() < 1e-9 && m.y.abs() < 1e-9
            })
            .expect("the block has a top front edge");
        GeomRef::Edge { body: 0, edge }
    }

    #[test]
    fn revolve_uses_the_centreline_and_starts_a_body() {
        let mut doc = Document::default();
        let mut sketch = FeatureId(0);
        let mut axis = None;
        doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
            if let Some(f) = m.feature_mut(sketch).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(
                    &mut f.sketch,
                    DVec2::new(10.0, 0.0),
                    DVec2::new(20.0, 30.0),
                );
                let line = f.sketch.add_line(DVec2::ZERO, DVec2::new(0.0, 30.0));
                f.sketch.set_construction(line, true);
                axis = Some(line);
            }
        });
        let lines = doc.revolve_axis_lines(sketch);
        assert_eq!(lines.len(), 5);
        assert_eq!((lines[0].0, lines[0].1), (axis.unwrap(), true));
        assert!((lines[0].2 - 30.0).abs() < 1e-9);

        let id = doc.add_revolve(sketch, Operation::Add).unwrap();
        let Some(FeatureKind::Revolve(r)) = doc.feature(id).map(|f| &f.kind) else {
            panic!("not a revolve");
        };
        assert_eq!(
            r.operation,
            Operation::NewBody,
            "the first body is a new body"
        );
        assert_eq!(
            r.axis,
            peet_model::RevolveAxisRef::SketchLine(axis.unwrap())
        );
        assert_eq!(doc.status(id), Some(&Status::Ok));
        assert_eq!(doc.bodies.len(), 1);
        // A ring 10 wide and 30 high, from radius 10 to 20.
        let mass = *doc.bodies[0].mass_properties().unwrap();
        let expected = std::f64::consts::PI * (400.0 - 100.0) * 30.0;
        assert!((mass.volume - expected).abs() < 1e-6 * expected);
        assert!(doc.bodies[0].is_measured());

        assert_eq!(doc.undo().as_deref(), Some("Add Revolve"));
        assert!(doc.feature(id).is_none());
        assert!(doc.bodies.is_empty());
        assert!(doc.add_revolve(FeatureId(9999), Operation::Add).is_none());
    }

    #[test]
    fn sweep_carries_a_profile_along_a_path_sketch() {
        let mut doc = Document::default();
        let (mut profile, mut path) = (FeatureId(0), FeatureId(0));
        doc.change("Sketches", |m| {
            // A 10 × 10 square on the top plane, and a path straight up from its middle.
            profile = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(profile).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(
                    &mut f.sketch,
                    DVec2::new(-5.0, -5.0),
                    DVec2::new(5.0, 5.0),
                );
            }
            let front = StdPlane::Front.plane();
            path = m.add_sketch(PlaneRef::Standard(StdPlane::Front), front);
            if let Some(f) = m.feature_mut(path).and_then(|f| f.sketch_mut()) {
                let a = front.to_plane_coords(DVec3::ZERO);
                let b = front.to_plane_coords(DVec3::new(0.0, 0.0, 30.0));
                f.sketch.add_line(a, b);
            }
        });
        // Without a path the feature waits for one.
        let waiting = doc.add_sweep(profile, None, Operation::Add).unwrap();
        assert!(doc.status(waiting).unwrap().message().is_some());
        assert_eq!(doc.sweep_path_choices(waiting), vec![path]);
        assert_eq!(doc.undo().as_deref(), Some("Add Sweep"));

        // The profile can't be its own path.
        let id = doc
            .add_sweep(profile, Some(profile), Operation::Add)
            .unwrap();
        let Some(FeatureKind::Sweep(s)) = doc.feature(id).map(|f| &f.kind) else {
            panic!("not a sweep");
        };
        assert_eq!((s.path, s.operation), (None, Operation::NewBody));
        doc.undo();

        let id = doc.add_sweep(profile, Some(path), Operation::Add).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        assert_eq!(doc.bodies.len(), 1);
        let volume = doc.bodies[0].mass_properties().unwrap().volume;
        assert!((volume - 3000.0).abs() < 1e-6, "{volume}");
        assert_eq!(doc.undo().as_deref(), Some("Add Sweep"));
        assert!(doc.bodies.is_empty());
    }

    #[test]
    fn fillet_and_chamfer_take_the_picked_edges() {
        let (mut doc, _) = block();
        let edge = top_edge(&doc);
        let before = *doc.bodies[0].mass_properties().unwrap();
        let refs = doc.edge_refs(&[edge, top_face(&doc)]);
        assert_eq!(refs.len(), 1, "faces are not edges");
        let id = doc.add_blend(BlendKind::Fillet, refs.clone()).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        assert_eq!(doc.feature(id).unwrap().kind.type_name(), "Fillet");
        // A 2 mm fillet along 40 mm removes (4 − π) × 40 mm³.
        let after = *doc.bodies[0].mass_properties().unwrap();
        let removed = (4.0 - std::f64::consts::PI) * 40.0;
        assert!((before.volume - after.volume - removed).abs() < 1e-6);
        assert_eq!(doc.undo().as_deref(), Some("Add Fillet"));
        assert!((doc.bodies[0].mass_properties().unwrap().volume - before.volume).abs() < 1e-9);

        let id = doc.add_blend(BlendKind::Chamfer, refs).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        let after = *doc.bodies[0].mass_properties().unwrap();
        assert!((before.volume - after.volume - 0.5 * 40.0).abs() < 1e-6);
        // With no edges the feature waits for them, and says so.
        let empty = doc.add_blend(BlendKind::Fillet, Vec::new()).unwrap();
        assert!(
            doc.status(empty)
                .unwrap()
                .message()
                .unwrap()
                .contains("Pick")
        );
    }

    #[test]
    fn shell_and_draft_take_the_picked_faces() {
        let (mut doc, _) = block();
        let top = top_face(&doc);
        let open = doc.face_refs(&[top, top_edge(&doc)], false);
        assert_eq!(open.len(), 1);
        let shell = doc.add_shell(open).unwrap();
        assert_eq!(doc.status(shell), Some(&Status::Ok));
        // Walls and floor 2 thick: the hollow is 36 × 16 × 8.
        let volume = doc.bodies[0].mass_properties().unwrap().volume;
        assert!((volume - (8000.0 - 36.0 * 16.0 * 8.0)).abs() < 1e-6);
        assert_eq!(doc.undo().as_deref(), Some("Add Shell"));

        let side = {
            let body = &doc.bodies[0];
            let face = body
                .solid
                .face_ids()
                .find(|f| body.face_center(*f).y.abs() < 1e-9)
                .unwrap();
            GeomRef::Face { body: 0, face }
        };
        let faces = doc.face_refs(&[side], true);
        assert_eq!(faces.len(), 1);
        let waiting = doc.add_draft(faces.clone(), None).unwrap();
        assert!(
            doc.status(waiting).unwrap().message().is_some(),
            "no neutral plane yet"
        );
        doc.undo();
        let draft = doc
            .add_draft(faces, Some(PlaneRef::Standard(StdPlane::Top)))
            .unwrap();
        assert_eq!(doc.status(draft), Some(&Status::Ok));
        assert!(doc.bodies[0].mass_properties().unwrap().volume < 8000.0 - 1.0);
    }

    #[test]
    fn hole_drills_at_the_sketch_points() {
        let (mut doc, _) = block();
        let GeomRef::Face { face, .. } = top_face(&doc) else {
            unreachable!()
        };
        let body = doc.bodies[0].source.clone();
        let plane = peet_model::face_sketch_plane(&body.solid, face).unwrap();
        let mut sketch = FeatureId(0);
        doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Face(body.face_ref(face)), plane);
            if let Some(f) = m.feature_mut(sketch).and_then(|f| f.sketch_mut()) {
                let at = plane.to_plane_coords(DVec3::new(20.0, 10.0, 10.0));
                f.sketch.add_point(at);
            }
        });
        assert_eq!(doc.hole_count(sketch), 1);
        let id = doc.add_hole(sketch).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        let volume = doc.bodies[0].mass_properties().unwrap().volume;
        let bore = std::f64::consts::PI * 3.3 * 3.3 * 10.0;
        assert!(
            (8000.0 - volume - bore).abs() < 1e-6,
            "an M6 clearance hole, through"
        );
        assert_eq!(doc.undo().as_deref(), Some("Add Hole"));
        assert!(doc.feature(id).is_none());
    }

    #[test]
    fn measurements_of_faces_edges_and_pairs() {
        let (doc, _) = block();
        let top = top_face(&doc);
        let edge = top_edge(&doc);
        assert_eq!(
            doc.describe(top),
            Some(Description::Face {
                area: 800.0,
                radius: None
            })
        );
        let Some(Description::Edge { length, radius, .. }) = doc.describe(edge) else {
            panic!("not an edge");
        };
        assert!((length - 40.0).abs() < 1e-9 && radius.is_none());
        let bottom = {
            let body = &doc.bodies[0];
            let face = body
                .solid
                .face_ids()
                .find(|f| body.face_center(*f).z < 1e-9)
                .unwrap();
            GeomRef::Face { body: 0, face }
        };
        let between = doc.measure_between(top, bottom).unwrap();
        assert!((between.distance.unwrap().0 - 10.0).abs() < 1e-9);
        assert!(between.angle.unwrap().abs() < 1e-9);
        assert!(
            doc.describe(GeomRef::Face {
                body: 7,
                face: peet_kernel::FaceId(0)
            })
            .is_none()
        );
    }

    #[test]
    fn step_import_adds_one_named_undoable_feature() {
        let (block_doc, _) = block();
        let solid = block_doc.evaluation().bodies[0].solid.clone();
        let options = peet_io::step::StepOptions {
            schema: peet_io::step::StepSchema::Ap214,
            product_name: "block".to_owned(),
            author: String::new(),
            organization: String::new(),
            timestamp: "2026-01-01T00:00:00".to_owned(),
        };
        let text = peet_io::step::write(&[("Block", &solid)], &options);

        let mut doc = Document::default();
        let done = doc.import_step("C:\\parts\\bracket.step", &text).unwrap();
        assert_eq!((done.name.as_str(), done.bodies), ("bracket", 1));
        assert_eq!(doc.status(done.feature), Some(&Status::Ok));
        assert_eq!(doc.bodies.len(), 1);
        let volume = doc.bodies[0].mass_properties().unwrap().volume;
        assert!((volume - 8000.0).abs() < 1e-6);
        let Some(FeatureKind::Import(i)) = doc.feature(done.feature).map(|f| &f.kind) else {
            panic!("not an import");
        };
        assert_eq!(i.source, "bracket.step");

        // A second import of the same file gets its own name.
        let again = doc.import_step("bracket.step", &text).unwrap();
        assert_eq!(again.name, "bracket (2)");
        assert_eq!(doc.undo().as_deref(), Some("Import STEP"));
        assert_eq!(doc.bodies.len(), 1);

        // A file that isn't STEP is refused in words, and changes nothing.
        let error = doc.import_step("notes.step", "hello").unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(doc.model.len(), 1);
        assert_eq!(doc.undo_label(), Some("Import STEP"));
    }

    #[test]
    fn totals_combine_bodies() {
        let (mut doc, ex) = block();
        let one = *doc.bodies[0].mass_properties().unwrap();
        assert!((one.volume - 8000.0).abs() < 1e-6);
        let alone = MassTotal::of([&one]).unwrap();
        assert!((alone.centroid - one.centroid).length() < 1e-9);
        for i in 0..3 {
            let (a, b) = (alone.principal_moments[i], one.principal_moments[i]);
            assert!((a - b).abs() < 1e-6 * b, "{a} vs {b}");
        }
        // The same block again, 100 further along X.
        let mut far = one;
        far.centroid.x += 100.0;
        let both = MassTotal::of([&one, &far]).unwrap();
        assert!((both.volume - 16000.0).abs() < 1e-6);
        assert!((both.centroid.x - (one.centroid.x + 50.0)).abs() < 1e-9);
        // About X nothing moved; about Y and Z each block is 50 from the common centre.
        let ixx = one.inertia.x_axis.x;
        let iyy = one.inertia.y_axis.y + 8000.0 * 2500.0;
        assert!((both.principal_moments[0] - 2.0 * ixx).abs() < 1e-6 * ixx);
        assert!((both.principal_moments[1] - 2.0 * iyy).abs() < 1e-6 * iyy);
        assert!(MassTotal::of([]).is_none());

        // The cache follows the body: a change measures the new body, undo finds the old.
        doc.change("Deeper", |m| {
            m.feature_mut(ex)
                .unwrap()
                .extrude_mut()
                .unwrap()
                .params
                .depth = Scalar::new(20.0);
        });
        assert!(!doc.bodies[0].is_measured());
        assert!((doc.bodies[0].mass_properties().unwrap().volume - 16000.0).abs() < 1e-6);
    }

    #[test]
    fn eigenvalues() {
        let m = DMat3::from_cols(
            DVec3::new(2.0, 1.0, 0.0),
            DVec3::new(1.0, 2.0, 0.0),
            DVec3::new(0.0, 0.0, 5.0),
        );
        let e = symmetric_eigenvalues(&m);
        assert!((e[0] - 1.0).abs() < 1e-12 && (e[1] - 3.0).abs() < 1e-12);
        assert!((e[2] - 5.0).abs() < 1e-12);
        let d = symmetric_eigenvalues(&DMat3::from_diagonal(DVec3::new(3.0, 1.0, 2.0)));
        assert_eq!(d, [1.0, 2.0, 3.0]);
    }
}
