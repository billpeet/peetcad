//! Operations that reach into an assembly's components, as SolidWorks' assembly equations
//! and FabSys do: setting a dimension of a component's part (`set` with
//! `Height@Sketch1@Door-1`), equations that set one (`set` with an expression), suppressing
//! a component by an expression, suppressing a feature of a component's part, reference
//! dimensions the equations read (`RD1@Annotations`), and giving a component a part of its
//! own (SolidWorks' Make Independent, FabSys's `ResolveComponent`).
//!
//! A component's part is shared by all its instances, so what is set in it reaches every
//! instance, as setting a part's dimension from a SolidWorks assembly changes the part's
//! document. A linked part's file is written too.

use std::path::PathBuf;
use std::sync::Arc;

use peet_document::Document;
use peet_model::{Assembly, CompId, DefId, EquationTarget, Model, ReferenceDimension, ScalarKind};
use serde_json::{Map, Value, json};

use crate::args::{Args, length_out, text};
use crate::assembly::CompSel;
use crate::mate::MateEndSel;
use crate::op::{Op, Query};
use crate::value::{Configs, Input};

/// The second part of a reference dimension's address: `RD1@Annotations`.
const ANNOTATIONS: &str = peet_model::equations::ANNOTATIONS;

/// The operations of this module, with the form they are listed under, for `help`.
pub(crate) const EQUATION_OPS: &[(&str, &str, &str, &str)] = &[
    (
        "assembly",
        "set",
        "name (an address that reaches into a component: NAME@FEATURE@COMPONENT, \"Height@Sketch1@Door-1\", or SolidWorks' \"Height@Sketch1@Door<1>.Part\"; a parameter of its part: NAME@COMPONENT), value (null: no equation), configurations (\"this\", \"all\", a name or a list of names; left out: this one if the equation already differs between configurations, else all)",
        "Set a dimension or parameter of a component's part from the assembly: a number sets the part (every instance of it, and a linked part's file); an expression is an equation of the assembly, worked out at every rebuild. With configurations (or where the equation differs between them), the value is an equation of those configurations.",
    ),
    (
        "component",
        "set_suppression_expression",
        "component, value (expression text, or null to clear), configurations (as for set)",
        "Suppress a component by an expression of the assembly: nonzero suppresses, zero unsuppresses (SolidWorks' suppressed and unsuppressed). It decides at every rebuild, over 'suppress'. It can differ between the assembly's configurations.",
    ),
    (
        "assembly",
        "suppress",
        "feature (FEATURE@COMPONENT), on (default true)",
        "Suppress a feature of a component's part (every instance of it), or unsuppress it.",
    ),
    (
        "assembly",
        "dimensions",
        "component, name",
        "An assembly's mate values (D1@Distance1), reference dimensions (RD1@Annotations) and equations; with component, its part's dimensions by their address from the assembly; with name, the one at that address.",
    ),
    (
        "assembly",
        "reference_dimension",
        "a, b (ends as a mate's), name (default RD1, RD2, ...)",
        "Measure the distance between two components' geometry, for expressions to read as NAME@Annotations. It follows the components.",
    ),
    (
        "reference_dimension",
        "delete",
        "reference_dimension",
        "Delete a reference dimension.",
    ),
    (
        "assembly",
        "make_independent",
        "component, name",
        "Give a component a copy of its part of its own, called name, and name the component after it (name-1): what is set in it then reaches only this component. A linked part's copy is written beside its file.",
    ),
];

/// Reads the operation called `op`, if it is one of this module's for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    Some(match op {
        "set_suppression_expression" if a.has("component") => (|| {
            Ok(Op::ComponentSuppression {
                component: a.required("component", CompSel::parse)?,
                configurations: a.parsed("configurations", Configs::parse)?,
                value: match a.take_nullable("value") {
                    Some(Value::Null) => None,
                    Some(v) => Some(text(&v).map_err(|e| format!("value: {e}"))?.to_owned()),
                    None => {
                        return Err(
                            "'set_suppression_expression' needs a 'value' field (null to clear)."
                                .to_owned(),
                        );
                    }
                },
            })
        })(),
        "reference_dimension" => (|| {
            Ok(Op::ReferenceDimension {
                a: a.required("a", MateEndSel::parse)?,
                b: a.required("b", MateEndSel::parse)?,
                name: a.string("name")?,
            })
        })(),
        "delete" if a.has("reference_dimension") => a
            .required("reference_dimension", |v| text(v).map(str::to_owned))
            .map(|name| Op::DeleteReference { name }),
        "make_independent" => (|| {
            Ok(Op::MakeIndependent {
                component: a.required("component", CompSel::parse)?,
                name: a.required("name", |v| text(v).map(str::to_owned))?,
            })
        })(),
        "dimensions" if a.has("component") => (|| {
            Ok(Op::Query(Query::Dimensions {
                feature: None,
                name: a.string("name")?,
                component: Some(a.required("component", CompSel::parse)?),
            }))
        })(),
        _ => return None,
    })
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no components.", doc.title()))
}

/// Whether `address` names a reference dimension (`RD1@Annotations`).
fn is_reference(address: &str) -> bool {
    address
        .rsplit_once('@')
        .is_some_and(|(_, rest)| rest.trim().eq_ignore_ascii_case(ANNOTATIONS))
}

/// The component an address reaches into, its part's address, and the definition.
fn reach(doc: &Document, address: &str) -> Result<(CompId, String, DefId), String> {
    let assembly = the_assembly(doc)?;
    let (c, rest) = doc.model.component_address(address).ok_or_else(|| {
        format!(
            "'{address}' names no component of {} at its end (NAME@FEATURE@COMPONENT, as \"Height@Sketch1@Door-1\"). The components are: {}.",
            doc.title(),
            assembly
                .components()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let def = assembly
        .definition_of(c)
        .ok_or_else(|| "The component's part is gone.".to_owned())?;
    Ok((c, rest, def.id))
}

/// Whether `set` on an assembly means a component's dimension: an address that ends in a
/// component's name.
pub(crate) fn reaches_component(doc: &Document, address: &str) -> bool {
    doc.model.component_address(address).is_some() || is_reference(address)
}

/// Writes a linked part's file after it was changed from the assembly, as SolidWorks
/// saves a part changed in an assembly with it. Returns the file written, if any.
pub(crate) fn write_link(
    doc: &Document,
    model: &Model,
    def: DefId,
) -> Result<Option<PathBuf>, String> {
    let Some(d) = model.assembly().and_then(|a| a.definition(def)) else {
        return Ok(None);
    };
    let Some(link) = &d.link else {
        return Ok(None);
    };
    crate::links::unavailable()?;
    let path = crate::links::find(link, crate::links::folder(doc).as_deref())
        .unwrap_or_else(|| PathBuf::from(&link.path));
    let bytes = peet_io::document::save(
        &d.model.for_file(path.parent()),
        &peet_io::document::Metadata::default(),
        None,
    )
    .map_err(|e| e.message)?;
    peet_platform::write_file(&path, &bytes)?;
    Ok(Some(path))
}

/// The configurations an equation that sets `target` is changed in: those given, else the
/// active one if it already differs between configurations, else all of them.
fn equation_scope(
    doc: &Document,
    target: &EquationTarget,
    configurations: Option<&Configs>,
) -> Result<peet_model::Scope, String> {
    Ok(match configurations {
        Some(c) => c.resolve(doc)?,
        None if doc.model.equation_differs(target) => peet_model::Scope::This,
        None => peet_model::Scope::All,
    })
}

/// `set` with an address that reaches into a component. Returns the undo label. `value`
/// `None` removes the equation that sets it.
pub(crate) fn set(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    target: &str,
    value: Option<&Input>,
    configurations: Option<&Configs>,
) -> Result<String, String> {
    if is_reference(target) {
        return Err(format!(
            "{target} is a reference dimension: it measures the assembly and can't be set."
        ));
    }
    let (c, rest, def) = reach(doc, target)?;
    let assembly = the_assembly(doc)?;
    let name = assembly.name_of(c).to_owned();
    let part = &assembly
        .definition(def)
        .ok_or_else(|| "The component's part is gone.".to_owned())?
        .model;
    let kind = part.kind_at(&rest).ok_or_else(|| {
        format!(
            "{} (the part of {name}) has no dimension or parameter '{rest}'. 'dimensions' with \"component\": \"{name}\" lists them.",
            part.name
        )
    })?;
    let own = EquationTarget::Dimension {
        component: c,
        address: rest.clone(),
    };
    // An equation of the assembly that sets the same value of the same part through another
    // component, in any configuration: this one takes its place.
    let same = |t: &EquationTarget| match t {
        EquationTarget::Dimension { component, address } => {
            assembly.component(*component).map(|o| o.definition) == Some(def)
                && address.eq_ignore_ascii_case(&rest)
        }
        EquationTarget::Suppression(_) => false,
    };
    let mut existing: Vec<EquationTarget> = doc
        .model
        .equation_targets()
        .into_iter()
        .filter(|t| same(t))
        .collect();
    // Its own address, as the equation that is there has it.
    let own = existing
        .iter()
        .find(|t| t.component() == c)
        .cloned()
        .unwrap_or(own);
    existing.retain(|t| *t != own);
    let scope = equation_scope(doc, &own, configurations)?;
    let full = format!("{rest}@{name}");
    data.insert("dimension".to_owned(), json!(full));
    for other in &existing {
        model.set_equation(other, None, &peet_model::Scope::All)?;
    }
    let Some(value) = value else {
        model.set_equation(&own, None, &scope)?;
        return Ok(format!("Clear equation {full}"));
    };
    let text = value.text();
    let constant = peet_sketch::expr::Expr::parse(&text)
        .map_err(|e| format!("{target}: {e}"))?
        .names()
        .is_empty();
    // A number for every configuration sets the part, where nothing differs; otherwise it
    // is an equation of the configurations it is for.
    let per_configuration =
        configurations.is_some_and(|c| *c != Configs::All) || doc.model.equation_differs(&own);
    if constant && !(per_configuration && doc.model.configurations().len() > 1) {
        let value = value
            .scalar(kind, doc)
            .map_err(|e| format!("{target}: {e}"))?
            .value;
        model.set_equation(&own, None, &peet_model::Scope::All)?;
        model.set_value_at(&full, value)?;
        if let Some(file) = write_link(doc, model, def)? {
            data.insert("written".to_owned(), json!(file.display().to_string()));
        }
    } else {
        // Checked against the assembly as it is now.
        let check = peet_model::Scalar {
            value: 0.0,
            expression: Some(text.clone()),
        }
        .evaluate(kind, &doc.model.parameters);
        if let Err(e) = check
            && !(kind == ScalarKind::Number
                && doc.model.parameters.evaluate_expression(&text).is_ok())
        {
            return Err(format!("{target}: {e}"));
        }
        model.set_equation(&own, Some(text), &scope)?;
    }
    Ok(format!("Set {full}"))
}

/// Sets or clears the equation that suppresses a component. Returns the undo label.
pub(crate) fn suppression(
    doc: &Document,
    model: &mut Model,
    component: &CompSel,
    value: Option<&str>,
    configurations: Option<&Configs>,
) -> Result<String, String> {
    let c = component.resolve(doc)?;
    let name = the_assembly(doc)?.name_of(c).to_owned();
    if let Some(source) = value {
        let q = doc
            .model
            .parameters
            .evaluate_expression(source)
            .map_err(|e| format!("value: {e}"))?;
        if !q.dim.is_none() {
            return Err(
                "value: a suppression expression gives a plain number: 0 unsuppresses, anything else suppresses."
                    .to_owned(),
            );
        }
    }
    let target = EquationTarget::Suppression(c);
    let scope = equation_scope(doc, &target, configurations)?;
    model.set_equation(&target, value.map(|v| v.trim().to_owned()), &scope)?;
    Ok(format!("Set suppression expression {name}"))
}

/// `suppress` with a feature of a component's part (`FEATURE@COMPONENT`). Returns the
/// undo label.
pub(crate) fn suppress_feature(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    feature: &str,
    on: bool,
) -> Result<String, String> {
    let (c, rest, def) = reach(doc, feature)?;
    let assembly = the_assembly(doc)?;
    let mut part = (*assembly
        .definition(def)
        .ok_or_else(|| "The component's part is gone.".to_owned())?
        .model)
        .clone();
    if part.is_assembly() {
        return Err(format!(
            "{} is a sub-assembly: suppress its components with 'suppress' and 'component' in it.",
            assembly.name_of(c)
        ));
    }
    let id = part
        .features()
        .find(|f| f.name == rest)
        .or_else(|| {
            let mut alike = part
                .features()
                .filter(|f| f.name.eq_ignore_ascii_case(&rest));
            match (alike.next(), alike.next()) {
                (Some(f), None) => Some(f),
                _ => None,
            }
        })
        .map(|f| f.id)
        .ok_or_else(|| {
            format!(
                "{} (the part of {}) has no feature called '{rest}'.",
                part.name,
                assembly.name_of(c)
            )
        })?;
    part.set_suppressed(id, on, &peet_model::Scope::default())?;
    let label = format!(
        "{} {} of {}",
        if on { "Suppress" } else { "Unsuppress" },
        part.name_of(id),
        assembly.name_of(c)
    );
    model
        .assembly_mut()
        .expect("an assembly")
        .set_model(def, Arc::new(part));
    if let Some(file) = write_link(doc, model, def)? {
        data.insert("written".to_owned(), json!(file.display().to_string()));
    }
    Ok(label)
}

/// Adds (or replaces) a reference dimension. Returns its name and the undo label.
pub(crate) fn add_reference(
    doc: &Document,
    model: &mut Model,
    a: &MateEndSel,
    b: &MateEndSel,
    name: Option<&str>,
) -> Result<(String, String), String> {
    let (a, b) = (a.resolve(doc)?, b.resolve(doc)?);
    let assembly = the_assembly(doc)?;
    for end in [&a, &b] {
        if end.geom.is_none() {
            return Err(
                "A reference dimension is measured between geometry: give each end a face, an edge, a vertex or a plane."
                    .to_owned(),
            );
        }
    }
    let name = match name.map(str::trim) {
        Some(n) if n.is_empty() || n.contains('@') || n.contains('"') => {
            return Err(format!(
                "'{n}' can't name a reference dimension: give a name without '@' or '\"' (RD1)."
            ));
        }
        Some(n) => n.to_owned(),
        None => (1..)
            .map(|n| format!("RD{n}"))
            .find(|n| assembly.reference(n).is_none())
            .unwrap_or_default(),
    };
    model
        .assembly_mut()
        .expect("an assembly")
        .set_reference(ReferenceDimension {
            name: name.clone(),
            a,
            b,
            value: None,
        });
    let label = format!("Add {name}");
    Ok((name, label))
}

/// Deletes a reference dimension. Returns the undo label.
pub(crate) fn delete_reference(
    doc: &Document,
    model: &mut Model,
    name: &str,
) -> Result<String, String> {
    let assembly = the_assembly(doc)?;
    let found = assembly
        .reference(name.split('@').next().unwrap_or(name))
        .map(|r| r.name.clone())
        .ok_or_else(|| {
            let names: Vec<&str> = assembly.references().map(|r| r.name.as_str()).collect();
            format!(
                "There is no reference dimension called '{name}'. They are: {}.",
                if names.is_empty() {
                    "(none)".to_owned()
                } else {
                    names.join(", ")
                }
            )
        })?;
    model
        .assembly_mut()
        .expect("an assembly")
        .remove_reference(&found);
    Ok(format!("Delete {found}"))
}

/// Gives a component a copy of its part of its own. Returns the undo label.
pub(crate) fn make_independent(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: &CompSel,
    name: &str,
) -> Result<String, String> {
    let c = component.resolve(doc)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("'name' is the new part's name: it can't be empty.".to_owned());
    }
    let assembly = the_assembly(doc)?;
    crate::pattern::placed_by_pattern(assembly, c, "made independent")?;
    let was = assembly.name_of(c).to_owned();
    let def = assembly
        .definition_of(c)
        .ok_or_else(|| "The component's part is gone.".to_owned())?;
    if assembly.definitions().any(|d| d.name() == name) {
        return Err(format!(
            "The assembly already has a part called '{name}': give the copy another name."
        ));
    }
    let mut copy = (*def.model).clone();
    copy.name = name.to_owned();
    let copy = Arc::new(copy);
    // A linked part's copy is a file of its own beside the part's, as SolidWorks saves
    // the copy.
    let link = match &def.link {
        Some(link) => {
            crate::links::unavailable()?;
            let file = crate::links::find(link, crate::links::folder(doc).as_deref())
                .unwrap_or_else(|| PathBuf::from(&link.path));
            let path = file.with_file_name(format!("{name}.peet"));
            if path.exists() {
                return Err(format!(
                    "{} already exists: give the copy another name.",
                    path.display()
                ));
            }
            let bytes = peet_io::document::save(
                &copy.for_file(path.parent()),
                &peet_io::document::Metadata::default(),
                None,
            )
            .map_err(|e| e.message)?;
            peet_platform::write_file(&path, &bytes)?;
            data.insert("written".to_owned(), json!(path.display().to_string()));
            Some(peet_model::Link {
                path: crate::links::full(&path),
                relative: link.relative,
            })
        }
        None => None,
    };
    let new_name = (1..)
        .map(|n| format!("{name}-{n}"))
        .find(|n| !assembly.components().any(|o| o.id != c && o.name == *n))
        .unwrap_or_default();
    let assembly = model.assembly_mut().expect("an assembly");
    let id = match link {
        Some(link) => assembly.define_linked(copy, link),
        None => assembly.define(copy),
    };
    assembly.replace(c, id);
    if let Some(comp) = assembly.component_mut(c) {
        comp.name = new_name;
    }
    Ok(format!("Make {was} independent"))
}

/// What a reference dimension measured, for a reply.
pub(crate) fn reference_out(doc: &Document, name: &str) -> Value {
    let units = &doc.model.parameters.units;
    let mut out = Map::new();
    out.insert("name".to_owned(), json!(name));
    out.insert("address".to_owned(), json!(format!("{name}@{ANNOTATIONS}")));
    match doc.evaluation().references.iter().find(|(n, _)| n == name) {
        Some((_, Ok(v))) => {
            out.insert("value".to_owned(), length_out(*v, units));
            out.insert("status".to_owned(), json!("ok"));
        }
        Some((_, Err(why))) => {
            out.insert("status".to_owned(), json!("failed"));
            out.insert("message".to_owned(), json!(why));
        }
        None => {}
    }
    Value::Object(out)
}

/// The text of an equation's target, as an address.
fn target_text(assembly: &Assembly, target: &EquationTarget) -> String {
    match target {
        EquationTarget::Dimension { component, address } => {
            format!("{address}@{}", assembly.name_of(*component))
        }
        EquationTarget::Suppression(c) => assembly.name_of(*c).to_owned(),
    }
}

/// An equation's expression in every configuration of the assembly (null where it has
/// none).
pub(crate) fn equation_configurations(doc: &Document, target: &EquationTarget) -> Value {
    Value::Object(
        doc.model
            .configurations()
            .iter()
            .map(|c| (c.name.clone(), json!(doc.model.equation_in(target, c.id))))
            .collect(),
    )
}

/// What every equation that differs between the assembly's configurations sets, by its
/// address, with its expression in each.
pub(crate) fn equations_that_differ(doc: &Document) -> Map<String, Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Map::new();
    };
    doc.model
        .equation_targets()
        .into_iter()
        .filter(|t| doc.model.equation_differs(t))
        .map(|t| (target_text(assembly, &t), equation_configurations(doc, &t)))
        .collect()
}

/// The equations and reference dimensions that can't be worked out, for `failures`.
pub(crate) fn failures(doc: &Document) -> Vec<Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Vec::new();
    };
    let built = doc.evaluation();
    let mut out: Vec<Value> = built
        .equation_failures
        .iter()
        .filter_map(|(i, why)| {
            let e = assembly.equations().get(*i)?;
            Some(json!({
                "equation": target_text(assembly, &e.target),
                "expression": e.expression,
                "status": "failed",
                "message": why,
            }))
        })
        .collect();
    out.extend(
        built
            .references
            .iter()
            .filter(|(_, r)| r.is_err())
            .map(|(name, _)| reference_out(doc, name)),
    );
    out
}

/// `dimensions` on an assembly: its own values and equations, or a component's part's
/// dimensions by their address from the assembly.
pub(crate) fn dimensions(
    doc: &Document,
    component: Option<&CompSel>,
    name: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let assembly = the_assembly(doc)?;
    let units = &doc.model.parameters.units;
    let value_out = |q: peet_sketch::expr::Quantity| -> Value {
        match q.kind() {
            Some(peet_sketch::expr::QuantityKind::Length) => length_out(q.value, units),
            _ => json!(crate::args::round(q.value)),
        }
    };
    let table = &doc.model.parameters.dimensions;
    let mut out = Map::new();
    if let Some(name) = name {
        let q = table.get(name.trim()).ok_or_else(|| {
            format!(
                "Nothing in {} is at '{name}'. 'dimensions' lists the assembly's own; with \"component\" a component's.",
                doc.title()
            )
        })?;
        let address = peet_sketch::expr::solidworks_address(name.trim());
        out.insert(
            "dimensions".to_owned(),
            json!([{ "address": address, "value": value_out(q) }]),
        );
        return Ok(out);
    }
    let list: Vec<Value> = match component {
        Some(sel) => {
            let c = sel.resolve(doc)?;
            let comp = assembly.component(c).ok_or("The component is gone.")?;
            let suffix = format!("@{}", comp.name);
            let driven: Vec<String> = assembly
                .equations()
                .iter()
                .filter_map(|e| match &e.target {
                    EquationTarget::Dimension { component, address }
                        if assembly.component(*component).map(|o| o.definition)
                            == Some(comp.definition) =>
                    {
                        Some(address.to_ascii_lowercase())
                    }
                    _ => None,
                })
                .collect();
            table
                .0
                .iter()
                .filter_map(|(a, q)| {
                    let inner = a.strip_suffix(&suffix)?;
                    // A part's own address, not one of a component further down.
                    let mut v = json!({ "address": a, "value": value_out(*q) });
                    if driven.contains(&inner.to_ascii_lowercase()) {
                        v["equation"] = json!(true);
                    }
                    Some(v)
                })
                .collect()
        }
        None => table
            .0
            .iter()
            .filter(|(a, _)| doc.model.component_address(a).is_none() || is_reference(a))
            .map(|(a, q)| json!({ "address": a, "value": value_out(*q) }))
            .collect(),
    };
    out.insert("dimensions".to_owned(), json!(list));
    if component.is_none() {
        let built = doc.evaluation();
        // Those of the active configuration, then those only others have, with their
        // expression in every configuration where it differs.
        let equations: Vec<Value> = doc
            .model
            .equation_targets()
            .into_iter()
            .map(|t| {
                let at = assembly.equations().iter().position(|e| e.target == t);
                let mut v = json!({ "target": target_text(assembly, &t) });
                match at {
                    Some(i) => v["expression"] = json!(assembly.equations()[i].expression),
                    None => v["expression"] = Value::Null,
                }
                if matches!(t, EquationTarget::Suppression(_)) {
                    v["suppresses"] = json!(true);
                }
                if doc.model.equation_differs(&t) {
                    v["configurations"] = equation_configurations(doc, &t);
                }
                if let Some((_, why)) =
                    at.and_then(|i| built.equation_failures.iter().find(|(n, _)| *n == i))
                {
                    v["status"] = json!("failed");
                    v["message"] = json!(why);
                }
                v
            })
            .collect();
        out.insert("equations".to_owned(), json!(equations));
        let references: Vec<Value> = assembly
            .references()
            .map(|r| reference_out(doc, &r.name))
            .collect();
        out.insert("references".to_owned(), json!(references));
    }
    Ok(out)
}
