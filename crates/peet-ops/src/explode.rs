//! Operations on an assembly's exploded view, and on which of its components are shown.
//!
//! An exploded view is a list of **steps**, each moving some components by a distance.
//! The steps are part of the assembly (stored, undone and redone like anything else);
//! whether the assembly is *shown* exploded is a view, like a sheet metal part shown
//! flat. Either way the components stay where their placements and mates have them:
//! what is measured, checked and exported is the assembly as it is put together.

use peet_document::Document;
use peet_model::{Assembly, CompId, ExplodeId, Model};
use serde_json::{Map, Value, json};

use crate::args::{Args, coordinates, integer, list, point3_out, text};
use crate::assembly::{CompSel, Point3};
use crate::op::Op;

/// A step of the exploded view, by name (`"Explode1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplodeSel {
    Id(ExplodeId),
    Name(String),
}

impl From<ExplodeId> for ExplodeSel {
    fn from(id: ExplodeId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for ExplodeSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no exploded view.", doc.title()))
}

impl ExplodeSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(ExplodeId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!(
                "expected an explode step's name or id, not {other}"
            )),
        }
    }

    /// The step this means in `doc`, or why there is none.
    pub fn resolve(&self, doc: &Document) -> Result<ExplodeId, String> {
        let assembly = the_assembly(doc)?;
        let names = || {
            let names: Vec<&str> = assembly.explode_steps().map(|s| s.name.as_str()).collect();
            if names.is_empty() {
                "The assembly has no explode steps yet: add one with explode_step.".to_owned()
            } else {
                format!("The steps are: {}.", names.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly.explode_step(*id).map(|s| s.id).ok_or_else(|| {
                format!("There is no explode step with the id {}. {}", id.0, names())
            }),
            Self::Name(name) => assembly
                .explode_steps()
                .find(|s| s.name == *name)
                .map(|s| s.id)
                .ok_or_else(|| format!("There is no explode step called '{name}'. {}", names())),
        }
    }
}

/// What to do to a step of the exploded view.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplodeChange {
    /// How far it moves its components, and which they are: the ones given.
    Edit {
        by: Option<Point3>,
        components: Option<Vec<CompSel>>,
    },
    Rename(String),
    Delete,
}

impl ExplodeChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit { .. } => "edit_explode_step",
            Self::Rename(_) => "rename",
            Self::Delete => "delete",
        }
    }
}

/// The operations of the exploded view and of what is shown, with their fields, for
/// `help`.
pub(crate) const EXPLODE_OPS: &[(&str, &str, &str)] = &[
    (
        "explode_step",
        "components (names or ids), by ([x, y, z]: how far, in the assembly's directions), name",
        "Add a step to the assembly's exploded view: these components are shown moved by this much when the assembly is shown exploded. The components themselves, and the mates, stay as they are.",
    ),
    (
        "edit_explode_step",
        "explode_step, by, components",
        "Change how far a step of the exploded view moves its components, or which they are.",
    ),
    (
        "rename",
        "explode_step, name",
        "Rename a step of the exploded view.",
    ),
    (
        "delete",
        "explode_step",
        "Delete a step of the exploded view.",
    ),
    (
        "explode",
        "on (left out: the other way)",
        "Show the assembly exploded (every explode step taken), or as it is. A view, not an undo step: positions, measurements, checks and exports always mean the assembly as it is put together.",
    ),
    (
        "explode_steps",
        "",
        "The steps of the exploded view (components, how far), where each component is shown when exploded, and whether the assembly is shown exploded now.",
    ),
    ("show_all", "", "Show every hidden component."),
    (
        "isolate",
        "components (names or ids)",
        "Show these components and hide every other.",
    ),
];

fn components(v: &Value) -> Result<Vec<CompSel>, String> {
    match v {
        Value::Array(_) => list(v)?.iter().map(CompSel::parse).collect(),
        // One component, given alone.
        other => Ok(vec![CompSel::parse(other)?]),
    }
}

/// Reads the operation called `op`, if `op` is one of these for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let change = |a: &mut Args, change: Result<ExplodeChange, String>| {
        Ok(Op::EditExplodeStep {
            step: a.required("explode_step", ExplodeSel::parse)?,
            change: change?,
        })
    };
    let by = |v: &Value| coordinates::<3>(v).map(Point3::Units);
    Some(match op {
        "explode_step" => (|| {
            Ok(Op::ExplodeStep {
                components: a.required("components", components)?,
                by: a.required("by", by)?,
                name: a.string("name")?,
            })
        })(),
        "edit_explode_step" => {
            let edit = (|| {
                let by = a.parsed("by", by)?;
                let components = a.parsed("components", components)?;
                if by.is_none() && components.is_none() {
                    return Err(
                        "'edit_explode_step' needs a 'by' or a 'components' field.".to_owned()
                    );
                }
                Ok(ExplodeChange::Edit { by, components })
            })();
            change(a, edit)
        }
        "rename" if a.has("explode_step") => {
            let to = a
                .required("name", |v| text(v).map(str::to_owned))
                .map(ExplodeChange::Rename);
            change(a, to)
        }
        "delete" if a.has("explode_step") => change(a, Ok(ExplodeChange::Delete)),
        "explode" => a
            .parsed("on", crate::args::boolean)
            .map(|on| Op::Explode { on }),
        "explode_steps" => Ok(Op::Query(crate::op::Query::ExplodeSteps)),
        "show_all" => Ok(Op::ShowAll),
        "isolate" => a
            .required("components", components)
            .map(|components| Op::Isolate { components }),
        _ => return None,
    })
}

/// A step in a few fields: what replies and `explode_steps` give.
pub(crate) fn step_out(doc: &Document, id: ExplodeId) -> Value {
    let Some((assembly, step)) = doc
        .model
        .assembly()
        .and_then(|a| Some((a, a.explode_step(id)?)))
    else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let names: Vec<&str> = step
        .components
        .iter()
        .map(|c| assembly.name_of(*c))
        .collect();
    json!({
        "id": step.id.0,
        "name": step.name,
        "components": names,
        "by": point3_out(step.offset, &doc.model.parameters.units),
    })
}

/// The steps of the exploded view, and where it puts each component it moves.
pub(crate) fn steps(doc: &Document) -> Result<Map<String, Value>, String> {
    let assembly = the_assembly(doc)?;
    let units = &doc.model.parameters.units;
    let list: Vec<Value> = assembly
        .explode_steps()
        .map(|s| step_out(doc, s.id))
        .collect();
    let moved: Vec<Value> = assembly
        .components()
        .filter(|c| {
            assembly
                .explode_steps()
                .any(|s| s.components.contains(&c.id))
        })
        .map(|c| {
            let offset = assembly.explode_offset(c.id);
            json!({
                "component": c.name,
                "by": point3_out(offset, units),
                "at": point3_out(c.placement.origin + offset, units),
            })
        })
        .collect();
    let mut out = Map::new();
    out.insert("steps".to_owned(), json!(list));
    out.insert("exploded".to_owned(), json!(doc.explode() > 0.0));
    out.insert("moved".to_owned(), json!(moved));
    Ok(out)
}

/// The components a list of selectors means, each once.
fn resolve(doc: &Document, components: &[CompSel]) -> Result<Vec<CompId>, String> {
    let mut out = Vec::new();
    for c in components {
        let id = c.resolve(doc)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() {
        return Err(
            "'components' is empty: name at least one component (as 'components' lists them)."
                .to_owned(),
        );
    }
    Ok(out)
}

fn rename(assembly: &mut Assembly, id: ExplodeId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if assembly
        .explode_steps()
        .any(|s| s.id != id && s.name == name)
    {
        return Err(format!("Another explode step is already called '{name}'."));
    }
    if let Some(s) = assembly.explode_step_mut(id) {
        name.clone_into(&mut s.name);
    }
    Ok(())
}

/// How far a step moves its components. A new step must move them somewhere; one that
/// is being changed may pass through nothing on its way.
fn distance(doc: &Document, by: &Point3, new: bool) -> Result<peet_math::DVec3, String> {
    let offset = by.mm(&doc.model.parameters.units);
    if !offset.is_finite() {
        return Err("'by' is not a distance.".to_owned());
    }
    if new && offset.length() < 1e-9 {
        return Err(
            "'by' is [0, 0, 0], which moves nothing: give how far the components go, such as [0, 0, 50]."
                .to_owned(),
        );
    }
    Ok(offset)
}

/// Adds a step. Returns it, with the undo label.
pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    components: &[CompSel],
    by: &Point3,
    name: Option<&str>,
) -> Result<(ExplodeId, String), String> {
    the_assembly(doc)?;
    let components = resolve(doc, components)?;
    let offset = distance(doc, by, true)?;
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let id = assembly.add_explode_step(&components, offset);
    if let Some(name) = name {
        rename(assembly, id, name)?;
    }
    let label = format!(
        "Add {}",
        assembly.explode_step(id).map_or("", |s| s.name.as_str())
    );
    Ok((id, label))
}

/// Changes a step. Returns the undo label.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: ExplodeId,
    change: &ExplodeChange,
) -> Result<String, String> {
    let gone = || "The explode step no longer exists.".to_owned();
    let was = the_assembly(doc)?
        .explode_step(id)
        .map(|s| s.name.clone())
        .ok_or_else(gone)?;
    // Read what is given against the document before anything changes.
    let edit = match change {
        ExplodeChange::Edit { by, components } => Some((
            by.as_ref().map(|by| distance(doc, by, false)).transpose()?,
            components.as_ref().map(|c| resolve(doc, c)).transpose()?,
        )),
        _ => None,
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    Ok(match change {
        ExplodeChange::Edit { .. } => {
            let step = assembly.explode_step_mut(id).ok_or_else(gone)?;
            if let Some((by, components)) = edit {
                if let Some(by) = by {
                    step.offset = by;
                }
                if let Some(components) = components {
                    step.components = components;
                }
            }
            format!("Edit {was}")
        }
        ExplodeChange::Rename(name) => {
            rename(assembly, id, name)?;
            format!("Rename {was}")
        }
        ExplodeChange::Delete => {
            assembly.remove_explode_step(id).ok_or_else(gone)?;
            format!("Delete {was}")
        }
    })
}

/// Shows every hidden component. Returns their names, with the undo label.
pub(crate) fn show_all(doc: &Document, model: &mut Model) -> Result<(Vec<String>, String), String> {
    let hidden: Vec<(CompId, String)> = the_assembly(doc)?
        .components()
        .filter(|c| !c.visible)
        .map(|c| (c.id, c.name.clone()))
        .collect();
    if let Some(assembly) = model.assembly_mut() {
        for (id, _) in &hidden {
            if let Some(c) = assembly.component_mut(*id) {
                c.visible = true;
            }
        }
    }
    Ok((
        hidden.into_iter().map(|(_, name)| name).collect(),
        "Show All".to_owned(),
    ))
}

/// Shows `components` and hides every other. Returns the names of those hidden, with
/// the undo label.
pub(crate) fn isolate(
    doc: &Document,
    model: &mut Model,
    components: &[CompSel],
) -> Result<(Vec<String>, String), String> {
    let assembly = the_assembly(doc)?;
    let keep = resolve(doc, components)?;
    let hidden: Vec<String> = assembly
        .components()
        .filter(|c| !keep.contains(&c.id))
        .map(|c| c.name.clone())
        .collect();
    let label = match keep.as_slice() {
        [one] => format!("Isolate {}", assembly.name_of(*one)),
        _ => "Isolate".to_owned(),
    };
    if let Some(assembly) = model.assembly_mut() {
        let ids: Vec<CompId> = assembly.components().map(|c| c.id).collect();
        for id in ids {
            let show = keep.contains(&id);
            if assembly.component(id).is_some_and(|c| c.visible != show)
                && let Some(c) = assembly.component_mut(id)
            {
                c.visible = show;
            }
        }
    }
    Ok((hidden, label))
}
