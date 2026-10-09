//! Operations on an assembly: its components, where they are, and the parts they are
//! instances of.
//!
//! An assembly document has components where a part has features, so most operations
//! are for one kind of document or the other ([`wrong_kind`] says which, and what to do
//! instead). A component is inserted from a file, a sample, another open document or
//! another component (one more instance of the same part); the part is copied into the
//! assembly, which is then complete in itself.

use std::sync::Arc;

use peet_document::Document;
use peet_math::{DQuat, DVec3, Frame};
use peet_model::{Assembly, CompId, DefId, Model, Status};
use peet_sketch::expr::Units;
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, coordinates, integer, number, point3_out, round, text};
use crate::op::{Op, Query, Sample, Source};
use crate::session::DocSel;

/// A component of the assembly, by name (`"Bracket-1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum CompSel {
    Id(CompId),
    Name(String),
}

impl From<CompId> for CompSel {
    fn from(id: CompId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for CompSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no components.", doc.title()))
}

impl CompSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(CompId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!("expected a component's name or id, not {other}")),
        }
    }

    /// The component this means in `doc`, or why there is none.
    pub fn resolve(&self, doc: &Document) -> Result<CompId, String> {
        let assembly = the_assembly(doc)?;
        let names = || {
            let names: Vec<&str> = assembly.components().map(|c| c.name.as_str()).collect();
            if names.is_empty() {
                "The assembly has no components yet: add one with insert.".to_owned()
            } else {
                format!("The components are: {}.", names.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly
                .component(*id)
                .map(|c| c.id)
                .ok_or_else(|| format!("There is no component with the id {}. {}", id.0, names())),
            Self::Name(name) => assembly
                .components()
                .find(|c| c.name == *name)
                .map(|c| c.id)
                .ok_or_else(|| format!("There is no component called '{name}'. {}", names())),
        }
    }
}

/// Where a component goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Placing {
    /// Exactly here (in mm): what the application passes.
    Frame(Frame),
    /// As a script writes it. The parts left out keep what the component has (a new
    /// component: the assembly's origin, not turned).
    Described {
        /// Where the part's origin goes, in document units.
        at: Option<[f64; 3]>,
        /// Turned about this axis through the part's origin, by this many degrees, from
        /// the part's own orientation.
        rotate: Option<([f64; 3], f64)>,
    },
}

impl Placing {
    fn parse(a: &mut Args) -> Result<Option<Self>, String> {
        let at = a.parsed("at", coordinates::<3>)?;
        let rotate = a.parsed("rotate", |v| {
            let wrong = || {
                format!(
                    "expected {{\"axis\": \"x\", \"y\", \"z\" or [x, y, z], \"angle\": degrees}}, not {v}"
                )
            };
            let map = v.as_object().ok_or_else(wrong)?;
            if map.keys().any(|k| k != "axis" && k != "angle") {
                return Err(wrong());
            }
            let axis = match map.get("axis").ok_or_else(wrong)? {
                Value::String(s) if s == "x" => [1.0, 0.0, 0.0],
                Value::String(s) if s == "y" => [0.0, 1.0, 0.0],
                Value::String(s) if s == "z" => [0.0, 0.0, 1.0],
                other => coordinates::<3>(other).map_err(|_| wrong())?,
            };
            let angle = number(map.get("angle").ok_or_else(wrong)?).map_err(|_| wrong())?;
            Ok((axis, angle))
        })?;
        Ok((at.is_some() || rotate.is_some()).then_some(Self::Described { at, rotate }))
    }

    /// The placement this means, starting from `from`.
    fn frame(&self, from: Frame, units: &Units) -> Result<Frame, String> {
        match self {
            Self::Frame(f) => Ok(*f),
            Self::Described { at, rotate } => {
                let mut frame = from;
                if let Some(p) = at {
                    frame.origin = crate::args::mm3(*p, units);
                }
                if let Some((axis, angle)) = rotate {
                    let axis = DVec3::from_array(*axis).try_normalize().ok_or_else(|| {
                        "rotate: the axis has no direction (it is [0, 0, 0]).".to_owned()
                    })?;
                    frame.rotation = DQuat::from_axis_angle(axis, angle.to_radians()).normalize();
                }
                Ok(frame)
            }
        }
    }
}

/// A point or a place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Point3 {
    /// As a script writes it: in document units.
    Units([f64; 3]),
    /// In mm: what the application passes.
    Mm(DVec3),
}

impl Point3 {
    pub(crate) fn mm(&self, units: &Units) -> DVec3 {
        match self {
            Self::Units(p) => crate::args::mm3(*p, units),
            Self::Mm(p) => *p,
        }
    }
}

/// The part a component is an instance of.
#[derive(Clone, Debug, PartialEq)]
pub enum InsertSource {
    /// A `.peet` file: a part, or an assembly (which becomes a sub-assembly).
    File(Source),
    Sample(Sample),
    /// Another document of the session, as it is now. Only [`crate::apply_session`]
    /// can resolve it.
    Document(DocSel),
    /// One more instance of the part a component is an instance of.
    Component(CompSel),
    /// This model: what the application passes, and what the others come to.
    Model(Arc<Model>),
}

impl InsertSource {
    fn parse(op: &str, a: &mut Args, component: bool) -> Result<Self, String> {
        let mut found = Vec::new();
        if let Some(path) = a.string("path")? {
            found.push(Self::File(Source::path(path)));
        }
        if let Some(sample) = a.parsed("sample", Sample::parse)? {
            found.push(Self::Sample(sample));
        }
        if let Some(part) = a.parsed("part", DocSel::parse)? {
            found.push(Self::Document(part));
        }
        if component && let Some(c) = a.parsed("component", CompSel::parse)? {
            found.push(Self::Component(c));
        }
        let choices = if component {
            "'path' (a .peet file), 'sample', 'part' (an open document) or 'component' (another instance of its part)"
        } else {
            "'path' (a .peet file), 'sample' or 'part' (an open document)"
        };
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(format!("'{op}' needs one of {choices}.")),
            _ => Err(format!("'{op}' takes only one of {choices}.")),
        }
    }

    /// The model to make a definition of.
    fn model(&self, doc: &Document) -> Result<Arc<Model>, String> {
        match self {
            Self::Model(model) => Ok(model.clone()),
            Self::File(file) => {
                let bytes = file.read()?;
                let opened = peet_io::document::open(&bytes)
                    .map_err(|e| format!("Couldn't open {}: {}", file.shown(), e.message))?;
                let mut model = opened.model;
                // A part is known by its file's name, as in the application's title.
                let stem = file.name.strip_suffix(".peet").unwrap_or(&file.name);
                if !stem.is_empty() && matches!(model.name.as_str(), "Part1" | "Assembly1") {
                    stem.clone_into(&mut model.name);
                }
                Ok(Arc::new(model))
            }
            Self::Sample(sample) => Ok(Arc::new(sample.model())),
            Self::Component(c) => {
                let id = c.resolve(doc)?;
                the_assembly(doc)?
                    .definition_of(id)
                    .map(|d| d.model.clone())
                    .ok_or_else(|| "The component's part is missing.".to_owned())
            }
            Self::Document(_) => Err(crate::session::needs_session("insert")),
        }
    }
}

/// What to do to a component.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentChange {
    Rename(String),
    /// Leave it out of the assembly without deleting it, or put it back.
    Suppress(bool),
    Show(bool),
    /// Hold it where it is, or let it go.
    Fix(bool),
    Place(Placing),
    /// Make it an instance of another part, where it is.
    Replace(InsertSource),
    /// A colour of its own, or (`None`) its part's again.
    Color(Option<[u8; 3]>),
    Delete,
}

impl ComponentChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Rename(_) => "rename",
            Self::Suppress(_) => "suppress",
            Self::Show(_) => "show",
            Self::Fix(_) => "fix",
            Self::Place(_) => "place",
            Self::Replace(_) => "replace",
            Self::Color(_) => "set_color",
            Self::Delete => "delete",
        }
    }
}

/// The operations of assemblies, with their fields, for `help`.
pub(crate) const ASSEMBLY_OPS: &[(&str, &str, &str)] = &[
    (
        "insert",
        "path (a .peet file), sample, part (an open document) or component (another instance of its part); name, at ([x, y, z]), rotate ({axis: \"x\"|\"y\"|\"z\"|[x, y, z], angle}), fixed, link (with path: follow the file instead of copying the part), absolute (with link: keep the link as a full path, not relative to the assembly's folder)",
        "Add a component to the assembly: an instance of a part, which is copied into the assembly (or, with link, read from its file whenever the assembly is opened).",
    ),
    (
        "place",
        "component, at ([x, y, z]), rotate ({axis, angle})",
        "Put a component somewhere: its part's origin at a point, turned about an axis from the part's own orientation.",
    ),
    (
        "drag",
        "component, to ([x, y, z]), point ([x, y, z] in the component's own coordinates; default its origin)",
        "Pull a point of a component towards a place, as with the mouse: it goes as far as its mates let it, sliding if it can and turning if it must, and what it is mated to comes along.",
    ),
    (
        "fix",
        "component, on (default true)",
        "Hold a component where it is, or let it go.",
    ),
    (
        "replace",
        "component, and path, sample or part (an open document)",
        "Make a component an instance of another part, where it is.",
    ),
    ("rename", "component, name", "Rename a component."),
    (
        "suppress",
        "component, on (default true)",
        "Leave a component out of the assembly without deleting it, or put it back.",
    ),
    (
        "show",
        "component, on (default true)",
        "Show or hide a component.",
    ),
    (
        "set_color",
        "component, color (\"#rrggbb\" or [r, g, b]; null for its part's colour)",
        "Give a component a colour of its own, in place of its part's.",
    ),
    (
        "delete",
        "component",
        "Delete a component (and its part, if no other component uses it).",
    ),
    (
        "open_component",
        "component",
        "Open a component's part as a document of its own, to change it. 'save' on that document stores it back in the assembly, for every instance.",
    ),
    (
        "interference",
        "component (left out: every pair)",
        "Where components run into each other: the pairs that share space, with how much (mm³) and where. Components that only touch do not interfere.",
    ),
    (
        "bom",
        "level (parts: every part, through sub-assemblies; top: a sub-assembly is one line; default parts)",
        "The bill of materials: each part with how many, its material, its mass, and for sheet metal its thickness and flat size.",
    ),
    (
        "components",
        "",
        "The components of the assembly (where each is, its part and its status) and its parts.",
    ),
];

/// Reads the assembly operation called `op`, if `op` is one for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let component = |a: &mut Args| a.required("component", CompSel::parse);
    let name = |v: &Value| text(v).map(str::to_owned);
    let change = |a: &mut Args, change: Result<ComponentChange, String>| {
        Ok(Op::Component {
            component: component(a)?,
            change: change?,
        })
    };
    Some(match op {
        "insert" => (|| {
            Ok(Op::Insert {
                from: InsertSource::parse("insert", a, true)?,
                name: a.string("name")?,
                placing: Placing::parse(a)?,
                fixed: a.parsed("fixed", boolean)?,
                link: a.flag("link", false)?,
                absolute: a.flag("absolute", false)?,
            })
        })(),
        "components" => Ok(Op::Query(Query::Components)),
        "interference" => a
            .parsed("component", CompSel::parse)
            .map(|component| Op::Query(Query::Interference { component })),
        "bom" => a
            .parsed("level", |v| match text(v)? {
                "parts" => Ok(false),
                "top" => Ok(true),
                other => Err(format!(
                    "'{other}' is not a level: use \"parts\" (every part, through sub-assemblies) or \"top\" (a sub-assembly is one line)"
                )),
            })
            .map(|top| {
                Op::Query(Query::Bom {
                    top_level: top.unwrap_or(false),
                })
            }),
        "open_component" => component(a).map(|component| Op::OpenComponent { component }),
        "place" => {
            let placing = Placing::parse(a).and_then(|p| {
                p.ok_or_else(|| "'place' needs an 'at' or a 'rotate' field.".to_owned())
            });
            change(a, placing.map(ComponentChange::Place))
        }
        "drag" => (|| {
            Ok(Op::Drag {
                component: component(a)?,
                point: a.parsed("point", coordinates::<3>)?.map(Point3::Units),
                to: Point3::Units(a.required("to", coordinates::<3>)?),
            })
        })(),
        "fix" => {
            let on = a.flag("on", true).map(ComponentChange::Fix);
            change(a, on)
        }
        "replace" => {
            let from = InsertSource::parse("replace", a, false).map(ComponentChange::Replace);
            change(a, from)
        }
        // These words are a feature's too: they are a component's when given one.
        "rename" if a.has("component") => {
            let to = a.required("name", name).map(ComponentChange::Rename);
            change(a, to)
        }
        "suppress" if a.has("component") => {
            let on = a.flag("on", true).map(ComponentChange::Suppress);
            change(a, on)
        }
        "set_color" if a.has("component") => {
            let color = match a.take_nullable("color") {
                None => Err(
                    "'set_color' needs a 'color' field (null for the part's colour).".to_owned(),
                ),
                Some(Value::Null) => Ok(None),
                Some(v) => crate::library::color(&v)
                    .map(Some)
                    .map_err(|e| format!("color: {e}")),
            };
            change(a, color.map(ComponentChange::Color))
        }
        "show" if a.has("component") => {
            let on = a.flag("on", true).map(ComponentChange::Show);
            change(a, on)
        }
        "delete" if a.has("component") => change(a, Ok(ComponentChange::Delete)),
        _ => return None,
    })
}

/// Why `op` can't be applied to this kind of document, if it can't: an operation on
/// features and bodies to an assembly, or one on components to a part.
pub(crate) fn wrong_kind(doc: &Document, op: &Op) -> Option<String> {
    let for_assembly = matches!(
        op,
        Op::Insert { .. }
            | Op::Component { .. }
            | Op::SetPart { .. }
            | Op::OpenComponent { .. }
            | Op::UpdateLinks
            | Op::Link { .. }
            | Op::Unlink { .. }
            | Op::Drag { .. }
            | Op::Mate { .. }
            | Op::EditMate { .. }
            | Op::ComponentPattern { .. }
            | Op::EditComponentPattern { .. }
            | Op::ExplodeStep { .. }
            | Op::EditExplodeStep { .. }
            | Op::Explode { .. }
            | Op::ShowAll
            | Op::Isolate { .. }
            | Op::Query(
                Query::Components
                    | Query::Mates
                    | Query::ExplodeSteps
                    | Query::Interference { .. }
                    | Query::Bom { .. }
            )
    );
    let for_part = match op {
        Op::Add(_)
        | Op::Edit { .. }
        | Op::Sketch { .. }
        | Op::Draw { .. }
        | Op::SetDimension { .. }
        | Op::Rename { .. }
        | Op::Suppress { .. }
        | Op::Show { .. }
        | Op::Delete { .. }
        | Op::Move { .. }
        | Op::Rollback { .. }
        | Op::SetSketch { .. }
        | Op::ImportDxf { .. }
        | Op::ShowDatum { .. }
        | Op::FlatPattern { .. }
        | Op::ApplyMaterial { .. }
        | Op::SetMaterial { .. }
        | Op::SetColor { .. }
        // An assembly has no configurations of its own: each part has its own.
        | Op::AddConfiguration { .. }
        | Op::EditConfiguration { .. }
        | Op::DeleteConfiguration { .. }
        | Op::Configuration { .. } => true,
        Op::Query(q) => matches!(
            q,
            Query::Features
                | Query::Configurations
                | Query::Feature(_)
                | Query::Bodies
                | Query::Faces { .. }
                | Query::Edges { .. }
                | Query::BendTable { .. }
                | Query::Checks { .. }
                | Query::Measure { .. }
        ),
        _ => false,
    };
    let word = op.word();
    if doc.is_assembly() && for_part {
        Some(format!(
            "{} is an assembly, and '{word}' works on a part. Its components are listed by 'components'; to change one of its parts, open it with 'open_component'.",
            doc.title()
        ))
    } else if !doc.is_assembly() && for_assembly {
        Some(format!(
            "{} is a part, and '{word}' works on an assembly. Start one with {{\"op\": \"new\", \"assembly\": true}}.",
            doc.title()
        ))
    } else {
        None
    }
}

fn status_word(status: Option<&Status>) -> &'static str {
    match status {
        Some(Status::Ok) => "ok",
        Some(Status::Warning(_)) => "warning",
        Some(Status::Failed(_)) => "failed",
        Some(Status::Suppressed | Status::SuppressedBy(_)) => "suppressed",
        Some(Status::RolledBack) | None => "not_built",
    }
}

/// A placement as scripts write it: where the origin is, and the turn as an axis and an
/// angle in degrees (left out if it is not turned).
fn placement_out(m: &mut Map<String, Value>, frame: &Frame, units: &Units) {
    m.insert("at".to_owned(), point3_out(frame.origin, units));
    let (axis, angle) = frame.rotation.to_axis_angle();
    // The shorter way round, and nothing for no turn at all.
    let (axis, angle) = if angle > std::f64::consts::PI {
        (-axis, std::f64::consts::TAU - angle)
    } else {
        (axis, angle)
    };
    if angle.abs() > 1e-12 {
        m.insert(
            "rotate".to_owned(),
            json!({
                "axis": [round(axis.x), round(axis.y), round(axis.z)],
                "angle": round(angle.to_degrees()),
            }),
        );
    }
}

/// A component in a few fields: what replies and `components` give.
pub(crate) fn component_out(doc: &Document, id: CompId) -> Value {
    let Some(assembly) = doc.model.assembly() else {
        return Value::Null;
    };
    let Some(c) = assembly.component(id) else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let units = &doc.model.parameters.units;
    let status = doc.evaluation().component_status(id);
    let mut m = Map::new();
    m.insert("id".to_owned(), json!(id.0));
    m.insert("name".to_owned(), json!(c.name));
    if let Some(d) = assembly.definition(c.definition) {
        m.insert("part".to_owned(), json!(d.name()));
    }
    placement_out(&mut m, &c.placement, units);
    m.insert("fixed".to_owned(), json!(c.fixed));
    // How many ways it can still move, on its own or along with what it is mated to.
    if let Some(freedom) = doc.evaluation().component_freedom(id) {
        m.insert("freedom".to_owned(), json!(freedom));
    }
    let mates: Vec<&str> = assembly.mates_of(id).map(|m| m.name.as_str()).collect();
    if !mates.is_empty() {
        m.insert("mates".to_owned(), json!(mates));
    }
    if !c.visible {
        m.insert("hidden".to_owned(), json!(true));
    }
    if let Some(color) = c.color {
        m.insert("color".to_owned(), crate::library::color_out(color));
    }
    // A copy made by a pattern, which places it.
    if let Some(pattern) = assembly.pattern_of(id) {
        m.insert("pattern".to_owned(), json!(pattern.name));
    }
    m.insert("status".to_owned(), json!(status_word(status)));
    if let Some(message) = status.and_then(Status::message) {
        m.insert("message".to_owned(), json!(message));
    }
    let bounds = doc.component_bounds(id);
    if bounds.min.cmple(bounds.max).all() {
        m.insert("min".to_owned(), point3_out(bounds.min, units));
        m.insert("max".to_owned(), point3_out(bounds.max, units));
    }
    Value::Object(m)
}

/// The components that need attention: what `failures` is for an assembly.
pub(crate) fn problems(doc: &Document) -> Vec<Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Vec::new();
    };
    assembly
        .components()
        .filter(|c| {
            doc.evaluation()
                .component_status(c.id)
                .is_some_and(|s| s.message().is_some())
        })
        .map(|c| component_out(doc, c.id))
        .chain(crate::mate::failures(doc))
        .chain(crate::pattern::failures(doc))
        .collect()
}

/// The components and the parts of the assembly.
pub(crate) fn components(doc: &Document) -> Result<Map<String, Value>, String> {
    let assembly = the_assembly(doc)?;
    let list: Vec<Value> = assembly
        .components()
        .map(|c| component_out(doc, c.id))
        .collect();
    let parts: Vec<Value> = assembly
        .definitions()
        .map(|d| {
            let mut m = Map::new();
            m.insert("id".to_owned(), json!(d.id.0));
            m.insert("name".to_owned(), json!(d.name()));
            m.insert(
                "kind".to_owned(),
                json!(if d.model.is_assembly() {
                    "assembly"
                } else {
                    "part"
                }),
            );
            m.insert("components".to_owned(), json!(assembly.uses(d.id)));
            if let Some(material) = &d.model.material {
                m.insert("material".to_owned(), json!(material.name));
            }
            if let Some(link) = &d.link {
                crate::links::link_out(&mut m, &d.model, link, doc);
            }
            Value::Object(m)
        })
        .collect();
    let mut out = Map::new();
    out.insert("components".to_owned(), json!(list));
    out.insert("parts".to_owned(), json!(parts));
    if assembly.patterns().len() > 0 {
        let patterns: Vec<Value> = assembly
            .patterns()
            .map(|p| crate::pattern::pattern_out(doc, p.id))
            .collect();
        out.insert("patterns".to_owned(), json!(patterns));
    }
    out.insert("mates".to_owned(), json!(assembly.mates().count()));
    out.insert("freedom".to_owned(), json!(doc.evaluation().freedom));
    Ok(out)
}

fn rename(assembly: &mut Assembly, id: CompId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if assembly.components().any(|c| c.id != id && c.name == name) {
        return Err(format!("Another component is already called '{name}'."));
    }
    if let Some(c) = assembly.component_mut(id) {
        name.clone_into(&mut c.name);
    }
    Ok(())
}

/// A model can't hold itself: the part being inserted must not be this assembly.
fn check_not_itself(doc: &Document, model: &Model) -> Result<(), String> {
    if model.is_assembly() && *model == doc.model {
        return Err(format!("{} can't be a component of itself.", doc.title()));
    }
    Ok(())
}

/// Adds a component. Returns it, with the undo label.
pub(crate) fn insert(
    doc: &Document,
    model: &mut Model,
    from: &InsertSource,
    name: Option<&str>,
    placing: Option<&Placing>,
    fixed: Option<bool>,
    link: Option<bool>,
) -> Result<(CompId, String), String> {
    // `link`: whether to link, and if so whether the link is relative.
    let relative = link.unwrap_or(true);
    let link = link.is_some();
    // A linked part is read from its file, which the assembly then follows.
    let linked = if link {
        crate::links::unavailable()?;
        let path = match from {
            InsertSource::File(Source {
                path: Some(path), ..
            }) => path,
            _ => {
                return Err(
                    "'link' follows a file, so it needs the part's 'path' (a .peet file on disk)."
                        .to_owned(),
                );
            }
        };
        let own = doc.file.as_ref().and_then(|f| f.path.as_ref());
        if own.is_some_and(|own| crate::links::same_file(own, path)) {
            return Err(format!("{} can't be a component of itself.", doc.title()));
        }
        let model = crate::links::read(path, 0)?;
        Some((Arc::new(model), crate::links::full(path)))
    } else {
        None
    };
    let part = match &linked {
        Some((model, _)) => model.clone(),
        None => from.model(doc)?,
    };
    let same_part = match from {
        InsertSource::Component(c) => the_assembly(doc)?
            .component(c.resolve(doc)?)
            .map(|c| c.definition),
        _ => None,
    };
    check_not_itself(doc, &part)?;
    let units = doc.model.parameters.units;
    let frame = match placing {
        Some(p) => p.frame(Frame::WORLD, &units)?,
        None => Frame::WORLD,
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let definition = match (linked, same_part) {
        (Some((model, path)), _) => {
            assembly.define_linked(model, peet_model::Link { path, relative })
        }
        // One more instance of a component's part is of that very part, linked or not.
        (None, Some(definition)) => definition,
        (None, None) => assembly.define(part),
    };
    let id = assembly
        .insert(definition, frame)
        .ok_or_else(|| "The component's part is missing.".to_owned())?;
    if let Some(name) = name {
        rename(assembly, id, name)?;
    }
    if let (Some(fixed), Some(c)) = (fixed, assembly.component_mut(id)) {
        c.fixed = fixed;
    }
    let label = format!("Insert {}", assembly.name_of(id));
    Ok((id, label))
}

/// Changes a component. Returns the undo label.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: CompId,
    change: &ComponentChange,
) -> Result<String, String> {
    let units = doc.model.parameters.units;
    let was = the_assembly(doc)?.name_of(id).to_owned();
    // A copy a pattern made is the pattern's to place and to remove.
    let refused = match change {
        ComponentChange::Place(_) => Some("placed"),
        ComponentChange::Fix(_) => Some("fixed or let go"),
        ComponentChange::Replace(_) => Some("replaced"),
        ComponentChange::Delete => Some("deleted"),
        _ => None,
    };
    if let Some(what) = refused {
        crate::pattern::placed_by_pattern(the_assembly(doc)?, id, what)?;
    }
    // Read what is to be inserted before anything changes.
    let replacement = match change {
        ComponentChange::Replace(from) => {
            let part = from.model(doc)?;
            check_not_itself(doc, &part)?;
            Some(part)
        }
        _ => None,
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let gone = || "The component no longer exists.".to_owned();
    Ok(match change {
        ComponentChange::Rename(name) => {
            rename(assembly, id, name)?;
            format!("Rename {was}")
        }
        ComponentChange::Suppress(on) => {
            assembly.component_mut(id).ok_or_else(gone)?.suppressed = *on;
            format!("{} {was}", if *on { "Suppress" } else { "Unsuppress" })
        }
        ComponentChange::Show(on) => {
            assembly.component_mut(id).ok_or_else(gone)?.visible = *on;
            format!("{} {was}", if *on { "Show" } else { "Hide" })
        }
        ComponentChange::Color(color) => {
            assembly.component_mut(id).ok_or_else(gone)?.color = *color;
            format!("Colour {was}")
        }
        ComponentChange::Fix(on) => {
            assembly.component_mut(id).ok_or_else(gone)?.fixed = *on;
            format!("{} {was}", if *on { "Fix" } else { "Float" })
        }
        ComponentChange::Place(placing) => {
            let c = assembly.component_mut(id).ok_or_else(gone)?;
            c.placement = placing.frame(c.placement, &units)?;
            format!("Move {was}")
        }
        ComponentChange::Replace(_) => {
            let definition = assembly.define(replacement.ok_or_else(gone)?);
            if !assembly.replace(id, definition) {
                return Err(gone());
            }
            format!("Replace {was}")
        }
        // Its mates go with it.
        ComponentChange::Delete => {
            assembly.remove(id).ok_or_else(gone)?;
            format!("Delete {was}")
        }
    })
}

/// Links a part to a file: the file's part if there is one, else the part is written
/// there. Returns the undo label.
pub(crate) fn link(
    doc: &Document,
    model: &mut Model,
    part: DefId,
    path: &std::path::Path,
    relative: bool,
) -> Result<String, String> {
    crate::links::unavailable()?;
    let own = doc.file.as_ref().and_then(|f| f.path.as_ref());
    if own.is_some_and(|own| crate::links::same_file(own, path)) {
        return Err(format!(
            "{} is the assembly's own file: a part can't be linked to it.",
            path.display()
        ));
    }
    let stored = crate::links::full(path);
    let assembly = the_assembly(doc)?;
    let definition = assembly
        .definition(part)
        .ok_or_else(|| "The part is no longer in the assembly.".to_owned())?;
    if let Some(other) = assembly
        .definitions()
        .find(|d| d.id != part && d.link.as_ref().is_some_and(|l| l.path == stored))
    {
        return Err(format!(
            "{} is already linked to {}: make these components instances of it with 'replace'.",
            other.name(),
            path.display()
        ));
    }
    let now = if path.is_file() {
        Arc::new(crate::links::read(path, 0)?)
    } else {
        let bytes = peet_io::document::save(
            &definition.model,
            &peet_io::document::Metadata::default(),
            None,
        )
        .map_err(|e| e.message)?;
        peet_platform::write_file(path, &bytes)?;
        definition.model.clone()
    };
    let label = format!("Link {}", now.name);
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    assembly.set_model(part, now);
    assembly.set_link(
        part,
        Some(peet_model::Link {
            path: stored,
            relative,
        }),
    );
    Ok(label)
}

/// Makes a linked part the assembly's own again. Returns the undo label.
pub(crate) fn unlink(doc: &Document, model: &mut Model, part: DefId) -> Result<String, String> {
    let definition = the_assembly(doc)?
        .definition(part)
        .ok_or_else(|| "The part is no longer in the assembly.".to_owned())?;
    if definition.link.is_none() {
        return Err(format!(
            "{} is not linked: it is the assembly's own already.",
            definition.name()
        ));
    }
    let label = format!("Unlink {}", definition.name());
    if let Some(assembly) = model.assembly_mut() {
        assembly.set_link(part, None);
    }
    Ok(label)
}

/// What a drag operation asks of the solve, with the undo label.
pub(crate) fn drag(
    doc: &Document,
    component: &CompSel,
    point: Option<&Point3>,
    to: &Point3,
) -> Result<(peet_model::Drag, String), String> {
    let id = component.resolve(doc)?;
    let c = the_assembly(doc)?
        .component(id)
        .ok_or_else(|| "The component no longer exists.".to_owned())?;
    crate::pattern::placed_by_pattern(the_assembly(doc)?, id, "dragged")?;
    if c.fixed {
        return Err(format!(
            "{name} is fixed, so it stays where it is: drag another component, or let it go first with {{\"op\": \"fix\", \"component\": \"{name}\", \"on\": false}}.",
            name = c.name
        ));
    }
    if c.suppressed {
        return Err(format!(
            "{} is suppressed: there is nothing to drag.",
            c.name
        ));
    }
    let units = &doc.model.parameters.units;
    let drag = peet_model::Drag {
        component: id,
        point: point.map_or(DVec3::ZERO, |p| p.mm(units)),
        to: to.mm(units),
    };
    Ok((drag, format!("Drag {}", c.name)))
}

/// Stores a part's model as one of the assembly's parts. Returns the undo label.
pub(crate) fn set_part(model: &mut Model, part: DefId, new: &Arc<Model>) -> Result<String, String> {
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    if !assembly.set_model(part, new.clone()) {
        return Err(
            "That part is no longer in the assembly: its components were deleted or replaced."
                .to_owned(),
        );
    }
    Ok(format!("Edit {}", new.name))
}

/// An assembly as the products of a STEP file: every part and sub-assembly once, with
/// the components placed in each. Returns them with the index of the assembly itself and
/// how many bodies its components show. Suppressed components are left out, and so is a
/// part that has no bodies.
pub(crate) fn step_products(doc: &Document) -> (Vec<peet_io::step::StepProduct>, usize, usize) {
    use peet_io::step::StepProduct;

    struct Walk<'a> {
        instances: &'a [peet_model::Instance],
        products: Vec<StepProduct>,
        /// The product of each part and sub-assembly. None: nothing in it. Two parts
        /// that are the same in everything are one product, as in the bill of materials.
        of_model: Vec<(Arc<Model>, Option<usize>)>,
    }

    impl Walk<'_> {
        /// The children of an assembly: its components that have something to show.
        fn children(&mut self, model: &Model, path: &[CompId]) -> Vec<(usize, String, Frame)> {
            let Some(assembly) = model.assembly() else {
                return Vec::new();
            };
            let mut children = Vec::new();
            for c in assembly.components().filter(|c| !c.suppressed) {
                let Some(definition) = assembly.definition(c.definition) else {
                    continue;
                };
                let mut path = path.to_vec();
                path.push(c.id);
                if let Some(product) = self.product(&definition.model, &path) {
                    children.push((product, c.name.clone(), c.placement));
                }
            }
            children
        }

        /// The product of the part or sub-assembly at `path`.
        fn product(&mut self, model: &Arc<Model>, path: &[CompId]) -> Option<usize> {
            let known = self
                .of_model
                .iter()
                .find(|(m, _)| Arc::ptr_eq(m, model) || **m == **model);
            if let Some((_, known)) = known {
                return *known;
            }
            let (bodies, children) = if model.is_assembly() {
                (Vec::new(), self.children(model, path))
            } else {
                let solids: Vec<&peet_kernel::Solid> = self
                    .instances
                    .iter()
                    .filter(|i| i.path == path)
                    .map(|i| &i.body.solid)
                    .collect();
                let bodies = solids
                    .iter()
                    .enumerate()
                    .map(|(n, solid)| {
                        let name = if n == 0 {
                            model.name.clone()
                        } else {
                            format!("{} ({})", model.name, n + 1)
                        };
                        (name, (*solid).clone())
                    })
                    .collect();
                (bodies, Vec::new())
            };
            let index = (!bodies.is_empty() || !children.is_empty()).then(|| {
                self.products.push(StepProduct {
                    name: model.name.clone(),
                    bodies,
                    children,
                });
                self.products.len() - 1
            });
            self.of_model.push((model.clone(), index));
            index
        }
    }

    let evaluation = doc.evaluation();
    let mut walk = Walk {
        instances: &evaluation.instances,
        products: Vec::new(),
        of_model: Vec::new(),
    };
    let children = walk.children(&doc.model, &[]);
    walk.products.push(StepProduct {
        name: doc.model.name.clone(),
        bodies: Vec::new(),
        children,
    });
    let root = walk.products.len() - 1;
    // (Every body shown is of a component that is not suppressed.)
    (walk.products, root, evaluation.instances.len())
}
