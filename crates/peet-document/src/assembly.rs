//! What an assembly is asked about as a whole: where its components run into each
//! other, what it is made of (its bill of materials), and what it weighs.
//!
//! All three work from the assembly as it is built: the bodies of its components where
//! they are ([`Document::bodies`] with [`Document::placed`]). A body is measured once
//! however many components show it, since they share its view.

use peet_kernel::boolean::{BooleanOp, boolean};
use peet_kernel::query::{MassProperties, mass_properties};
use peet_math::{Aabb, DMat3, DVec2, DVec3, Frame, tolerance};
use std::collections::HashMap;
use std::sync::Arc;

use peet_io::step_import::StepNode;
use peet_model::{Assembly, CompId, DefId, Definition, ImportedSolid, Model};

use crate::document::Document;
use crate::solids::symmetric_eigenvalues;

/// A component of an assembly, through the sub-assemblies it is in.
#[derive(Clone, Debug)]
pub struct Leaf<'a> {
    /// The components from the assembly's own down to this one.
    pub path: Vec<CompId>,
    /// Their names, joined: `Gearbox-1/Shaft-2`.
    pub name: String,
    pub definition: &'a Definition,
}

/// The components of `model`. With `parts_only`, a sub-assembly is replaced by the
/// components in it (and so on down): what is left are parts. Suppressed components are
/// left out.
pub fn leaves(model: &Model, parts_only: bool) -> Vec<Leaf<'_>> {
    fn collect<'a>(
        model: &'a Model,
        parts_only: bool,
        path: &[CompId],
        name: &str,
        out: &mut Vec<Leaf<'a>>,
    ) {
        let Some(assembly) = model.assembly() else {
            return;
        };
        for c in assembly.components().filter(|c| !c.suppressed) {
            let Some(definition) = assembly.definition(c.definition) else {
                continue;
            };
            let mut path = path.to_vec();
            path.push(c.id);
            let name = if name.is_empty() {
                c.name.clone()
            } else {
                format!("{name}/{}", c.name)
            };
            if parts_only && definition.model.is_assembly() {
                collect(&definition.model, true, &path, &name, out);
            } else {
                out.push(Leaf {
                    path,
                    name,
                    definition,
                });
            }
        }
    }
    let mut out = Vec::new();
    collect(model, parts_only, &[], "", &mut out);
    out
}

/// The eight corners of a box, placed, as a box.
fn placed_box(bounds: &Aabb, frame: &Frame) -> Aabb {
    let mut out = Aabb::EMPTY;
    for i in 0..8 {
        let pick = |bit: usize, lo: f64, hi: f64| if i >> bit & 1 == 0 { lo } else { hi };
        out.extend(frame.to_world(DVec3::new(
            pick(0, bounds.min.x, bounds.max.x),
            pick(1, bounds.min.y, bounds.max.y),
            pick(2, bounds.min.z, bounds.max.z),
        )));
    }
    out
}

/// A body's mass properties where the body is placed.
fn placed_mass(m: &MassProperties, frame: &Frame) -> MassProperties {
    let turn = DMat3::from_quat(frame.rotation);
    MassProperties {
        centroid: frame.to_world(m.centroid),
        inertia: turn * m.inertia * turn.transpose(),
        bounds: placed_box(&m.bounds, frame),
        ..*m
    }
}

// ---- Interference ----

/// Two components that occupy the same space.
#[derive(Clone, Debug, PartialEq)]
pub struct Interference {
    /// The two components, by their names through sub-assemblies.
    pub a: String,
    pub b: String,
    /// The components of the assembly itself they belong to.
    pub top: [CompId; 2],
    /// How much of them overlaps, mm³.
    pub volume: f64,
    /// Where: the box around the overlap.
    pub bounds: Aabb,
}

/// What a check for interference found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interferences {
    pub found: Vec<Interference>,
    /// Pairs whose boxes overlap but that couldn't be compared, with why.
    pub unchecked: Vec<(String, String, String)>,
    /// How many pairs of bodies were close enough to compare.
    pub compared: usize,
}

/// Less overlap than this (mm³) is two faces touching, not two bodies in one place.
const TOUCHING: f64 = 1e-6;

impl Document {
    /// The name of the component a shown body belongs to, through sub-assemblies.
    fn body_owner(&self, index: usize) -> String {
        let mut model = &self.model;
        let mut names = Vec::new();
        for id in &self.placed[index].path {
            let Some(assembly) = model.assembly() else {
                break;
            };
            names.push(assembly.name_of(*id).to_owned());
            match assembly.definition_of(*id) {
                Some(d) => model = &d.model,
                None => break,
            }
        }
        names.join("/")
    }

    /// Finds the components that run into each other: every pair of bodies of different
    /// components whose boxes overlap is intersected, and those that share volume are
    /// reported. With `only`, just the pairs that component is one of.
    ///
    /// Bodies that only touch (faces against each other, a pin in a hole of its size) do
    /// not interfere.
    pub fn interferences(&self, only: Option<CompId>) -> Interferences {
        let mut out = Interferences::default();
        let boxes: Vec<Aabb> = self
            .bodies
            .iter()
            .zip(&self.placed)
            .map(|(b, p)| placed_box(&b.source.solid.bounds(), &p.frame))
            .collect();
        // Placed solids, made when first needed.
        let mut placed: Vec<Option<peet_kernel::Solid>> = vec![None; self.bodies.len()];
        let apart = tolerance::LINEAR;
        for i in 0..self.bodies.len() {
            for j in i + 1..self.bodies.len() {
                let (pi, pj) = (&self.placed[i], &self.placed[j]);
                // Bodies of one component (a part with several) are that part's business.
                if pi.path == pj.path || pi.path.is_empty() || pj.path.is_empty() {
                    continue;
                }
                if only.is_some_and(|c| pi.component() != Some(c) && pj.component() != Some(c)) {
                    continue;
                }
                let (a, b) = (&boxes[i], &boxes[j]);
                let overlap = a
                    .min
                    .max(b.min)
                    .cmplt(a.max.min(b.max) - DVec3::splat(apart));
                if !overlap.all() {
                    continue;
                }
                out.compared += 1;
                for k in [i, j] {
                    if placed[k].is_none() {
                        placed[k] = Some(peet_kernel::transform::solid(
                            &self.bodies[k].source.solid,
                            &self.placed[k].frame,
                        ));
                    }
                }
                let (Some(sa), Some(sb)) = (&placed[i], &placed[j]) else {
                    continue;
                };
                let names = || (self.body_owner(i), self.body_owner(j));
                let shared = boolean(sa, sb, BooleanOp::Intersect).and_then(|common| {
                    if common.faces.is_empty() {
                        Ok(None)
                    } else {
                        mass_properties(&common).map(Some)
                    }
                });
                match shared {
                    Ok(Some(m)) if m.volume > TOUCHING => {
                        let (a, b) = names();
                        // A component with several bodies in the other: one finding.
                        let (ti, tj) = (pi.component(), pj.component());
                        let same = out.found.iter_mut().find(|f| f.a == a && f.b == b);
                        match (same, ti, tj) {
                            (Some(f), ..) => {
                                f.volume += m.volume;
                                f.bounds = f.bounds.union(&m.bounds);
                            }
                            (None, Some(ti), Some(tj)) => out.found.push(Interference {
                                a,
                                b,
                                top: [ti, tj],
                                volume: m.volume,
                                bounds: m.bounds,
                            }),
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let (a, b) = names();
                        out.unchecked.push((a, b, e.to_string()));
                    }
                }
            }
        }
        out
    }
}

// ---- Bill of materials ----

/// What a sheet metal part is cut from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetStock {
    /// mm.
    pub thickness: f64,
    /// The flat pattern's size, mm.
    pub flat_size: DVec2,
    pub bends: usize,
}

/// One line of a bill of materials: a part (or a sub-assembly), and how many.
#[derive(Clone, Debug, PartialEq)]
pub struct BomRow {
    /// Counted from 1, in the order the parts first appear.
    pub item: usize,
    pub part: String,
    /// A sub-assembly, listed as one thing (a bill of materials of the top level only).
    pub is_assembly: bool,
    pub quantity: usize,
    pub material: Option<String>,
    /// Of one, mm³.
    pub volume: f64,
    /// Of one, kg: known if the part (every part, for a sub-assembly) has a material.
    pub mass: Option<f64>,
    /// For a sheet metal part.
    pub sheet: Option<SheetStock>,
    /// The components that are this part.
    pub components: Vec<String>,
}

impl Document {
    /// The volume and the mass (if every part in it has a material) of what a component
    /// shows: its bodies, and those of the components in it.
    fn weigh(&self, path: &[CompId]) -> (f64, Option<f64>) {
        let mut volume = 0.0;
        let mut mass = Some(0.0);
        let parts = leaves(&self.model, true);
        for (i, placed) in self.placed.iter().enumerate() {
            if !placed.path.starts_with(path) {
                continue;
            }
            let Ok(m) = self.bodies[i].mass_properties() else {
                continue;
            };
            volume += m.volume;
            let material = parts
                .iter()
                .find(|l| l.path == placed.path)
                .and_then(|l| l.definition.model.material.as_ref());
            mass = match (mass, material) {
                (Some(sum), Some(material)) => Some(sum + material.mass_kg(m.volume)),
                _ => None,
            };
        }
        (volume, mass)
    }

    /// The assembly's bill of materials: its parts, with how many of each. With
    /// `parts_only` the parts of sub-assemblies are counted with the rest; without, a
    /// sub-assembly is one line.
    pub fn bill_of_materials(&self, parts_only: bool) -> Vec<BomRow> {
        let mut rows: Vec<BomRow> = Vec::new();
        let mut models: Vec<&Model> = Vec::new();
        for leaf in leaves(&self.model, parts_only) {
            let model = &*leaf.definition.model;
            // The same part, wherever it is used (in sub-assemblies too).
            let known = models
                .iter()
                .position(|m| std::ptr::eq(*m, model) || **m == *model);
            if let Some(row) = known {
                rows[row].quantity += 1;
                rows[row].components.push(leaf.name);
                continue;
            }
            let (volume, mass) = self.weigh(&leaf.path);
            let sheet = self
                .placed
                .iter()
                .zip(&self.bodies)
                .filter(|(p, _)| p.path == leaf.path)
                .find_map(|(_, b)| b.source.sheet.as_ref())
                .map(|sheet| {
                    let report = sheet.report();
                    SheetStock {
                        thickness: sheet.layout.settings.thickness,
                        flat_size: report.flat_size,
                        bends: report.bends.len(),
                    }
                });
            models.push(model);
            rows.push(BomRow {
                item: rows.len() + 1,
                part: model.name.clone(),
                is_assembly: model.is_assembly(),
                quantity: 1,
                material: model.material.as_ref().map(|m| m.name.clone()),
                volume,
                mass,
                sheet,
                components: vec![leaf.name],
            });
        }
        rows
    }
}

// ---- Mass ----

/// What one component of an assembly weighs, and where its centre of gravity is.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentMass {
    pub component: CompId,
    pub name: String,
    /// mm³.
    pub volume: f64,
    /// kg, if every part in it has a material.
    pub mass: Option<f64>,
    /// In the assembly's coordinates.
    pub centroid: DVec3,
}

/// The mass properties of an assembly.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyMass {
    pub components: Vec<ComponentMass>,
    /// mm³ and mm².
    pub volume: f64,
    pub area: f64,
    /// kg: known if every part has a material.
    pub mass: Option<f64>,
    /// The parts that have no material, so that the mass is not known.
    pub without_material: Vec<String>,
    /// The centre of gravity: of the mass if it is known, else of the volume (as if
    /// everything were made of one material).
    pub centroid: DVec3,
    /// The principal moments of inertia about the centre of gravity, ascending: in
    /// kg·mm² if the mass is known, else in mm⁵ (for a density of 1).
    pub principal_moments: [f64; 3],
    pub bounds: Aabb,
    /// Bodies that couldn't be measured, with why.
    pub problems: Vec<String>,
}

impl Document {
    /// The mass properties of the assembly: every component's bodies where they are,
    /// each weighed with its own part's material. `None` if nothing in it has volume.
    pub fn assembly_mass(&self) -> Option<AssemblyMass> {
        let parts = leaves(&self.model, true);
        let assembly = self.model.assembly()?;
        let mut without_material: Vec<String> = Vec::new();
        let mut problems = Vec::new();
        // Each body placed, with its density (kg/mm³) if its part has a material.
        let mut bodies: Vec<(MassProperties, Option<f64>, CompId)> = Vec::new();
        for (i, placed) in self.placed.iter().enumerate() {
            let Some(top) = placed.component() else {
                continue;
            };
            let part = parts.iter().find(|l| l.path == placed.path);
            let material = part.and_then(|l| l.definition.model.material.as_ref());
            match self.bodies[i].mass_properties() {
                Ok(m) => bodies.push((
                    placed_mass(m, &placed.frame),
                    material.map(|m| m.density * 1e-9),
                    top,
                )),
                Err(e) => problems.push(format!("{}: {e}", self.body_owner(i))),
            }
            if let (None, Some(part)) = (material, part) {
                let name = part.definition.name().to_owned();
                if !without_material.contains(&name) {
                    without_material.push(name);
                }
            }
        }
        let volume: f64 = bodies.iter().map(|(m, ..)| m.volume).sum();
        if bodies.is_empty() || volume < f64::MIN_POSITIVE {
            return None;
        }
        let known = without_material.is_empty();
        // What each body counts for: its mass if all are known, else its volume.
        let weight = |m: &MassProperties, density: &Option<f64>| match density {
            Some(d) if known => m.volume * d,
            _ => m.volume,
        };
        let total: f64 = bodies.iter().map(|(m, d, _)| weight(m, d)).sum();
        let centroid = bodies
            .iter()
            .map(|(m, d, _)| m.centroid * weight(m, d))
            .sum::<DVec3>()
            / total;
        // Each body's inertia moved to the common centre (the parallel axis theorem).
        let mut inertia = DMat3::ZERO;
        for (m, d, _) in &bodies {
            let per_volume = weight(m, d) / m.volume;
            let offset = m.centroid - centroid;
            let shift = DMat3::from_diagonal(DVec3::splat(offset.length_squared()))
                - DMat3::from_cols(offset * offset.x, offset * offset.y, offset * offset.z);
            inertia += (m.inertia + shift * m.volume) * per_volume;
        }
        let components = assembly
            .components()
            .filter(|c| !c.suppressed)
            .filter_map(|c| {
                let own: Vec<&(MassProperties, Option<f64>, CompId)> =
                    bodies.iter().filter(|(_, _, top)| *top == c.id).collect();
                let volume: f64 = own.iter().map(|(m, ..)| m.volume).sum();
                if volume < f64::MIN_POSITIVE {
                    return None;
                }
                let mass = own
                    .iter()
                    .map(|(m, d, _)| d.map(|d| m.volume * d))
                    .sum::<Option<f64>>();
                // Of its mass if that is known, else of its volume.
                let of = |m: &MassProperties, d: &Option<f64>| match (mass, d) {
                    (Some(_), Some(d)) => m.volume * d,
                    _ => m.volume,
                };
                let sum: f64 = own.iter().map(|(m, d, _)| of(m, d)).sum();
                Some(ComponentMass {
                    component: c.id,
                    name: c.name.clone(),
                    volume,
                    mass,
                    centroid: own
                        .iter()
                        .map(|(m, d, _)| m.centroid * of(m, d))
                        .sum::<DVec3>()
                        / sum,
                })
            })
            .collect();
        Some(AssemblyMass {
            components,
            volume,
            area: bodies.iter().map(|(m, ..)| m.area).sum(),
            mass: known.then_some(total),
            without_material,
            centroid,
            principal_moments: symmetric_eigenvalues(&inertia),
            bounds: bodies
                .iter()
                .fold(Aabb::EMPTY, |all, (m, ..)| all.union(&m.bounds)),
            problems,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use peet_math::{DQuat, Plane};
    use peet_model::{Engine, Material, Operation, PlaneRef, Scalar, StdPlane};

    use super::*;

    /// A plate from (0, 0, 0) to (w, h, t), with a hole of radius `hole` through its
    /// middle if that is not zero.
    fn plate(name: &str, w: f64, h: f64, t: f64, hole: f64) -> Model {
        let mut m = Model::new();
        name.clone_into(&mut m.name);
        let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        let sketch = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
        peet_sketch::shapes::rectangle(sketch, DVec2::ZERO, DVec2::new(w, h));
        if hole > 0.0 {
            sketch.add_circle(DVec2::new(w / 2.0, h / 2.0), hole);
        }
        let e = m.add_extrude(s, Operation::Add);
        m.feature_mut(e)
            .unwrap()
            .extrude_mut()
            .unwrap()
            .params
            .depth = Scalar::new(t);
        Engine::new().regenerate(&mut m);
        m
    }

    /// A pin of radius `r` and length `l` standing on the top plane at the origin.
    fn pin(r: f64, l: f64) -> Model {
        let mut m = Model::new();
        m.name = "Pin".to_owned();
        let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        m.feature_mut(s)
            .unwrap()
            .sketch_mut()
            .unwrap()
            .sketch
            .add_circle(DVec2::ZERO, r);
        let e = m.add_extrude(s, Operation::Add);
        m.feature_mut(e)
            .unwrap()
            .extrude_mut()
            .unwrap()
            .params
            .depth = Scalar::new(l);
        Engine::new().regenerate(&mut m);
        m
    }

    fn at(x: f64, y: f64, z: f64) -> Frame {
        Frame {
            origin: DVec3::new(x, y, z),
            ..Frame::WORLD
        }
    }

    /// An assembly of `parts`, each placed.
    fn assembly(parts: &[(&Model, Frame)]) -> Document {
        let mut model = Model::new_assembly();
        let a = model.assembly_mut().unwrap();
        for (part, frame) in parts {
            let d = a.define(Arc::new((*part).clone()));
            a.insert(d, *frame);
        }
        Document::from_model(model, None)
    }

    const PI: f64 = std::f64::consts::PI;

    #[test]
    fn components_that_share_space_interfere_and_those_that_touch_do_not() {
        let holed = plate("Holed", 40.0, 30.0, 5.0, 4.0);
        let solid = plate("Solid", 40.0, 30.0, 5.0, 0.0);
        let snug = pin(4.0, 20.0);
        let fat = pin(5.0, 20.0);
        let through = at(20.0, 15.0, -5.0);

        // A pin in a hole of its size, standing on a plate that the holed one lies on:
        // they touch all round, and nothing interferes.
        let doc = assembly(&[
            (&holed, Frame::WORLD),
            (&snug, at(20.0, 15.0, 0.0)),
            (&solid, at(0.0, 0.0, -5.0)),
        ]);
        let found = doc.interferences(None);
        assert_eq!(found.found, [], "{:?}", found.unchecked);
        assert_eq!(found.unchecked, []);
        // Only the pin and the plate it is in are in each other's space at all.
        assert_eq!(found.compared, 1);
        // Pushed down into the plate underneath, it is in the way there.
        let doc = assembly(&[
            (&holed, Frame::WORLD),
            (&snug, at(20.0, 15.0, -2.0)),
            (&solid, at(0.0, 0.0, -5.0)),
        ]);
        let found = doc.interferences(None);
        assert_eq!(found.found.len(), 1, "{found:?}");
        assert_eq!(found.found[0].b, "Solid-1");
        assert!((found.found[0].volume - PI * 16.0 * 2.0).abs() < 1e-6);

        // The same pin through a plate with no hole: a slug of the thickness of the plate.
        let doc = assembly(&[
            (&solid, Frame::WORLD),
            (&snug, through),
            (&solid, at(500.0, 0.0, 0.0)),
        ]);
        let found = doc.interferences(None);
        assert_eq!(
            found.compared, 1,
            "the far plate is not compared with anything"
        );
        assert_eq!(found.found.len(), 1, "{found:?}");
        let hit = &found.found[0];
        assert_eq!((hit.a.as_str(), hit.b.as_str()), ("Solid-1", "Pin-1"));
        assert_eq!(hit.top, [CompId(1), CompId(2)]);
        assert!(
            (hit.volume - PI * 16.0 * 5.0).abs() < 1e-6,
            "{}",
            hit.volume
        );
        assert!(
            hit.bounds
                .min
                .abs_diff_eq(DVec3::new(16.0, 11.0, 0.0), 1e-6)
        );
        assert!(
            hit.bounds
                .max
                .abs_diff_eq(DVec3::new(24.0, 19.0, 5.0), 1e-6)
        );
        // Only the pairs one component is in.
        assert_eq!(doc.interferences(Some(CompId(3))).compared, 0);
        assert_eq!(doc.interferences(Some(CompId(2))).found.len(), 1);

        // A pin too fat for its hole: the ring it would have to push out of the way.
        let doc = assembly(&[(&holed, Frame::WORLD), (&fat, through)]);
        let found = doc.interferences(None);
        assert_eq!(found.found.len(), 1, "{found:?}");
        assert!((found.found[0].volume - PI * 9.0 * 5.0).abs() < 1e-6);

        // Two plates overlapping at a corner, one of them turned: a box 10 x 30 x 3.
        let turned = Frame {
            origin: DVec3::new(70.0, 30.0, 2.0),
            rotation: DQuat::from_rotation_z(PI),
        };
        let doc = assembly(&[(&solid, Frame::WORLD), (&solid, turned)]);
        let found = doc.interferences(None);
        assert_eq!(found.found.len(), 1, "{found:?}");
        assert!(
            (found.found[0].volume - 900.0).abs() < 1e-6,
            "{}",
            found.found[0].volume
        );

        // In a sub-assembly: named through it.
        let mut sub = Model::new_assembly();
        sub.name = "Pair".to_owned();
        {
            let a = sub.assembly_mut().unwrap();
            let d = a.define(Arc::new(snug.clone()));
            a.insert(d, through);
        }
        let doc = assembly(&[(&solid, Frame::WORLD), (&sub, Frame::WORLD)]);
        let found = doc.interferences(None);
        assert_eq!(found.found[0].b, "Pair-1/Pin-1");
    }

    #[test]
    fn an_assemblys_meshes_are_made_on_demand_and_kept_in_its_file() {
        let (a, b) = (plate("Plate", 40.0, 30.0, 5.0, 4.0), pin(4.0, 20.0));
        let doc = assembly(&[
            (&a, at(0.0, 0.0, 0.0)),
            (&a, at(0.0, 0.0, 10.0)),
            (&b, at(0.0, 0.0, 0.0)),
        ]);
        // Nothing is tessellated until it is shown, and instances share what is made.
        assert!(doc.bodies.iter().all(|b| !b.is_tessellated()));
        let triangles = doc.bodies[0].tess().triangles().len();
        assert!(doc.bodies[1].is_tessellated() && !doc.bodies[2].is_tessellated());
        assert!(Arc::ptr_eq(&doc.bodies[0], &doc.bodies[1]));

        // A coarser mesh for drawing it small: the same faces and edges, fewer triangles
        // round the hole.
        let coarse = doc.bodies[0].coarse();
        assert_eq!(coarse.faces.len(), doc.bodies[0].tess().faces.len());
        assert_eq!(coarse.edges.len(), doc.bodies[0].tess().edges.len());
        assert!(coarse.triangles().len() < triangles, "{triangles}");

        // Saved: the mesh that was made, and not the one that was not.
        let bytes = doc.save_bytes(true).unwrap();
        let opened = peet_io::document::open(&bytes).unwrap();
        assert_eq!(opened.meshes.len(), 1);
        assert_eq!(opened.meshes[0].stamp, doc.bodies[0].stamp);
        let again = Document::from_opened(opened, None);
        assert_eq!(again.model, doc.model);
        // Opened: the plate's mesh is the file's (it is there without being made), the
        // pin's is still to be made.
        assert!(again.bodies[0].is_tessellated() && again.bodies[1].is_tessellated());
        assert!(!again.bodies[2].is_tessellated());
        assert_eq!(again.bodies[0].tess().triangles().len(), triangles);
        // Saved again without having been drawn, it keeps what it has.
        let kept = peet_io::document::open(&again.save_bytes(true).unwrap()).unwrap();
        assert_eq!(kept.meshes.len(), 1);
        // And without caches there are none.
        let bare = peet_io::document::open(&doc.save_bytes(false).unwrap()).unwrap();
        assert!(bare.meshes.is_empty());
    }

    #[test]
    fn a_bill_of_materials_counts_the_parts() {
        let mut holed = plate("Plate", 40.0, 30.0, 5.0, 4.0);
        holed.material = Some(Material::new("Mild steel", 7850.0).unwrap());
        let pin = pin(4.0, 20.0);
        let panel = peet_model::samples::enclosure().0;
        // A sub-assembly of a plate and two pins.
        let mut sub = Model::new_assembly();
        sub.name = "Pinned plate".to_owned();
        {
            let a = sub.assembly_mut().unwrap();
            let p = a.define(Arc::new(holed.clone()));
            let n = a.define(Arc::new(pin.clone()));
            a.insert(p, Frame::WORLD);
            a.insert(n, at(100.0, 0.0, 0.0));
            a.insert(n, at(120.0, 0.0, 0.0));
        }
        let mut doc = assembly(&[
            (&holed, Frame::WORLD),
            (&holed, at(0.0, 100.0, 0.0)),
            (&sub, at(0.0, 0.0, 200.0)),
            (&sub, at(0.0, 0.0, 400.0)),
            (&panel, at(300.0, 0.0, 0.0)),
            (&pin, at(-50.0, 0.0, 0.0)),
        ]);

        // Parts only: the parts of the sub-assemblies are counted with the rest.
        let rows = doc.bill_of_materials(true);
        let summary: Vec<(usize, &str, usize)> = rows
            .iter()
            .map(|r| (r.item, r.part.as_str(), r.quantity))
            .collect();
        assert_eq!(
            summary,
            [(1, "Plate", 4), (2, "Pin", 5), (3, "Enclosure Panel", 1)]
        );
        let plate_volume = 40.0 * 30.0 * 5.0 - PI * 16.0 * 5.0;
        assert!((rows[0].volume - plate_volume).abs() < 1e-6);
        assert_eq!(rows[0].material.as_deref(), Some("Mild steel"));
        assert!((rows[0].mass.unwrap() - plate_volume * 7850e-9).abs() < 1e-9);
        assert_eq!(rows[0].sheet, None);
        assert_eq!(
            rows[0].components,
            [
                "Plate-1",
                "Plate-2",
                "Pinned plate-1/Plate-1",
                "Pinned plate-2/Plate-1"
            ]
        );
        assert_eq!((rows[1].material.as_deref(), rows[1].mass), (None, None));
        // The sheet metal part: what it is cut from (the blank of the Phase 4 exit part).
        let stock = rows[2].sheet.unwrap();
        assert_eq!(stock.thickness, 1.5);
        assert!((stock.flat_size - DVec2::new(244.356_636, 194.356_636)).length() < 1e-5);
        assert_eq!(stock.bends, 4);

        // The top level only: a sub-assembly is one line, weighed if all its parts are.
        let rows = doc.bill_of_materials(false);
        let summary: Vec<(&str, usize, bool)> = rows
            .iter()
            .map(|r| (r.part.as_str(), r.quantity, r.is_assembly))
            .collect();
        assert_eq!(
            summary,
            [
                ("Plate", 2, false),
                ("Pinned plate", 2, true),
                ("Enclosure Panel", 1, false),
                ("Pin", 1, false)
            ]
        );
        assert_eq!(rows[1].mass, None, "its pins have no material");
        assert!((rows[1].volume - (plate_volume + 2.0 * PI * 16.0 * 20.0)).abs() < 1e-5);

        // A suppressed component is not in the bill.
        doc.change("Suppress", |m| {
            let a = m.assembly_mut().unwrap();
            a.component_mut(CompId(6)).unwrap().suppressed = true;
        });
        assert_eq!(doc.bill_of_materials(true)[1].quantity, 4);
    }

    #[test]
    fn an_assembly_is_weighed_part_by_part() {
        let mut steel = plate("Plate", 40.0, 30.0, 5.0, 0.0);
        steel.material = Some(Material::new("Mild steel", 7850.0).unwrap());
        let mut light = pin(4.0, 20.0);
        light.material = Some(Material::new("Aluminium", 2700.0).unwrap());
        // A plate at the origin, another turned up on end far away, and a pin.
        let on_end = Frame {
            origin: DVec3::new(200.0, 0.0, 0.0),
            rotation: DQuat::from_rotation_x(PI / 2.0),
        };
        let doc = assembly(&[
            (&steel, Frame::WORLD),
            (&steel, on_end),
            (&light, at(0.0, 0.0, 100.0)),
        ]);
        let mass = doc.assembly_mass().unwrap();
        let plate_kg = 6000.0 * 7850e-9;
        let pin_kg = PI * 16.0 * 20.0 * 2700e-9;
        assert!((mass.volume - (12000.0 + PI * 320.0)).abs() < 1e-6);
        assert!((mass.mass.unwrap() - (2.0 * plate_kg + pin_kg)).abs() < 1e-9);
        assert!(mass.without_material.is_empty());
        // Centres: that of the first plate; that of the second, turned (y becomes z, and
        // z becomes minus y).
        let c1 = DVec3::new(20.0, 15.0, 2.5);
        let c2 = DVec3::new(220.0, -2.5, 15.0);
        let c3 = DVec3::new(0.0, 0.0, 110.0);
        assert!(
            mass.components[1].centroid.abs_diff_eq(c2, 1e-6),
            "{:?}",
            mass.components[1]
        );
        assert!((mass.components[2].mass.unwrap() - pin_kg).abs() < 1e-9);
        let expected = (c1 * plate_kg + c2 * plate_kg + c3 * pin_kg) / (2.0 * plate_kg + pin_kg);
        assert!(
            mass.centroid.abs_diff_eq(expected, 1e-4),
            "{} / {expected}",
            mass.centroid
        );
        assert!(
            mass.bounds
                .max
                .abs_diff_eq(DVec3::new(240.0, 30.0, 120.0), 1e-6)
        );
        // The moments are those of the masses (kg mm²): the largest is at least what two
        // point masses at the centres of the plates would have about the axis between
        // them through the common centre.
        assert!(mass.principal_moments[0] > 0.0);
        assert!(mass.principal_moments[2] > 2.0 * plate_kg * 100.0 * 100.0);

        // One part without a material: the mass is not known, and says which; the
        // centre is then that of the volume.
        let bare = pin(4.0, 20.0);
        let doc = assembly(&[(&steel, Frame::WORLD), (&bare, at(0.0, 0.0, 100.0))]);
        let mass = doc.assembly_mass().unwrap();
        assert_eq!(mass.mass, None);
        assert_eq!(mass.without_material, ["Pin"]);
        assert!((mass.components[0].mass.unwrap() - plate_kg).abs() < 1e-9);
        assert_eq!(mass.components[1].mass, None);
        let (v1, v3) = (6000.0, PI * 320.0);
        let by_volume = (c1 * v1 + c3 * v3) / (v1 + v3);
        assert!(mass.centroid.abs_diff_eq(by_volume, 1e-4));

        // An assembly with nothing in it weighs nothing.
        assert_eq!(assembly(&[]).assembly_mass(), None);
    }
}

// ---- STEP import ----

/// What a STEP import added to an assembly.
#[derive(Clone, Debug, PartialEq)]
pub struct StepAssemblyImported {
    /// The new components of the assembly itself.
    pub components: Vec<CompId>,
    /// How many different parts came in (each once, however often it is used).
    pub parts: usize,
    /// How many bodies they show, counting each where it is used.
    pub bodies: usize,
    /// The importer's notes for the user: assumptions made and what was left out.
    pub warnings: Vec<String>,
}

/// The parts and sub-assemblies made from a STEP file's product structure.
struct StepModels<'a> {
    nodes: &'a [StepNode],
    /// The file's name, for the import features of the parts.
    source: &'a str,
    /// The model of each node that has one yet.
    built: HashMap<usize, Arc<Model>>,
    parts: usize,
}

impl StepModels<'_> {
    /// A part holding `bodies`.
    fn part(&mut self, name: &str, bodies: &[peet_io::step_import::ImportedBody]) -> Arc<Model> {
        let mut model = Model::new();
        name.clone_into(&mut model.name);
        model.add_import(
            self.source.to_owned(),
            bodies
                .iter()
                .map(|b| ImportedSolid {
                    name: b.name.clone(),
                    solid: b.solid.clone(),
                })
                .collect(),
        );
        self.parts += 1;
        Arc::new(model)
    }

    /// The model of a node: a part for one with solids only, an assembly for one with
    /// things placed in it. None for a node with nothing in it.
    fn model(&mut self, index: usize) -> Option<Arc<Model>> {
        if let Some(model) = self.built.get(&index) {
            return Some(model.clone());
        }
        let node = self.nodes.get(index)?;
        let model = if node.children.is_empty() {
            if node.bodies.is_empty() {
                return None;
            }
            self.part(&node.name, &node.bodies)
        } else {
            let mut model = Model::new_assembly();
            node.name.clone_into(&mut model.name);
            let added = self.fill(model.assembly_mut()?, index, &mut HashMap::new());
            if added.is_empty() {
                return None;
            }
            Arc::new(model)
        };
        self.built.insert(index, model.clone());
        Some(model)
    }

    /// Puts what a node holds into `assembly`: its own solids as a part, and what is
    /// placed in it as components, held where the file has them. `defined` is the nodes
    /// that are parts of the assembly already.
    fn fill(
        &mut self,
        assembly: &mut Assembly,
        index: usize,
        defined: &mut HashMap<usize, DefId>,
    ) -> Vec<CompId> {
        let Some(node) = self.nodes.get(index) else {
            return Vec::new();
        };
        let mut added = Vec::new();
        let mut place = |assembly: &mut Assembly, definition: DefId, name: &str, frame| {
            if let Some(id) = assembly.insert(definition, frame) {
                // Called what the file calls it, if that is a name no other has.
                let name = name.trim();
                let free = !name.is_empty()
                    && !name.contains('/')
                    && !assembly.components().any(|c| c.name == name);
                if let Some(c) = assembly.component_mut(id) {
                    c.fixed = true;
                    if free {
                        name.clone_into(&mut c.name);
                    }
                }
                added.push(id);
            }
        };
        if !node.bodies.is_empty() {
            let part = self.part(&node.name, &node.bodies);
            let definition = assembly.define(part);
            place(assembly, definition, "", peet_math::Frame::WORLD);
        }
        for (child, name, frame) in &node.children {
            let definition = match defined.get(child) {
                Some(definition) => *definition,
                None => {
                    let Some(model) = self.model(*child) else {
                        continue;
                    };
                    let definition = assembly.define(model);
                    defined.insert(*child, definition);
                    definition
                }
            };
            place(assembly, definition, name, *frame);
        }
        added
    }
}

impl Document {
    /// Adds what a STEP file (its text) holds to the assembly, keeping the file's
    /// structure: each of its parts becomes a part of the assembly once, each of its
    /// assemblies a sub-assembly, and what is placed in its top level become components
    /// here, fixed where the file has them. A file of parts alone gives a component for
    /// each. The error says, in plain words, why nothing was imported.
    pub fn import_step_assembly(
        &mut self,
        file_name: &str,
        text: &str,
    ) -> Result<StepAssemblyImported, String> {
        if !self.is_assembly() {
            return Err(format!("{} is not an assembly.", self.title()));
        }
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
        let source = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
        let mut models = StepModels {
            nodes: &read.nodes,
            source,
            built: HashMap::new(),
            parts: 0,
        };
        let mut model = self.model.clone();
        let assembly = model
            .assembly_mut()
            .ok_or_else(|| "The document is not an assembly.".to_owned())?;
        let mut components = Vec::new();
        let mut defined = HashMap::new();
        for root in &read.roots {
            let node = &read.nodes[*root];
            if node.children.is_empty() {
                // A part at the top of the file: a component of its own.
                if let Some(part) = models.model(*root) {
                    let definition = assembly.define(part);
                    if let Some(id) = assembly.insert(definition, peet_math::Frame::WORLD) {
                        if let Some(c) = assembly.component_mut(id) {
                            c.fixed = true;
                        }
                        components.push(id);
                    }
                }
            } else {
                components.extend(models.fill(assembly, *root, &mut defined));
            }
        }
        if components.is_empty() {
            return Err(
                "No solid in this STEP file could be imported, so nothing was added.".to_owned(),
            );
        }
        let parts = models.parts;
        let stem = crate::solids::file_stem(file_name).trim();
        let label = if stem.is_empty() {
            "Import STEP".to_owned()
        } else {
            format!("Import {stem}")
        };
        self.change(&label, |m| *m = model);
        Ok(StepAssemblyImported {
            components,
            parts,
            bodies: read.bodies.len(),
            warnings: read.warnings,
        })
    }
}
