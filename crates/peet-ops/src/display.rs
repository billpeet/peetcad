//! Operations on an assembly's display states (SolidWorks' display states): named looks of
//! the assembly, each with every component shown or hidden, coloured and see-through as it
//! has them. See [`peet_model::DisplayState`].
//!
//! `show`, `isolate`, `show_all`, `set_color` and `set_transparency` change the display
//! state shown; given `display_states`, `show`, `set_color` and `set_transparency` change
//! those instead.

use peet_document::Document;
use peet_model::{Assembly, CompId, DisplayStateId, Look, Model};
use serde_json::{Map, Value, json};

use crate::args::{Args, integer, list, text};
use crate::assembly::CompSel;
use crate::op::Op;

/// A display state, by name (`"Display State-1"`) or id. A name is looked for among the
/// active configuration's display states first, then among every configuration's.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayStateSel {
    Id(DisplayStateId),
    Name(String),
}

impl From<DisplayStateId> for DisplayStateSel {
    fn from(id: DisplayStateId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for DisplayStateSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model.assembly().ok_or_else(|| {
        format!(
            "{} is a part: display states are an assembly's (a part's look is its colour, set_color).",
            doc.title()
        )
    })
}

impl DisplayStateSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(DisplayStateId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!(
                "expected a display state's name or id, not {other}"
            )),
        }
    }

    /// The display state this means in `doc`, or why there is none.
    pub fn resolve(&self, doc: &Document) -> Result<DisplayStateId, String> {
        the_assembly(doc)?;
        let model = &doc.model;
        let names = || {
            let names: Vec<&str> = model
                .display_states()
                .iter()
                .map(|s| s.name.as_str())
                .collect();
            if names.is_empty() {
                "The assembly has no display states yet: add one with add_display_state.".to_owned()
            } else {
                format!(
                    "The active configuration's display states are: {}.",
                    names.join(", ")
                )
            }
        };
        match self {
            Self::Id(id) => model.display_state(*id).map(|s| s.id).ok_or_else(|| {
                format!(
                    "There is no display state with the id {}. {}",
                    id.0,
                    names()
                )
            }),
            Self::Name(name) => {
                if let Some(s) = model.display_states().iter().find(|s| s.name == *name) {
                    return Ok(s.id);
                }
                let all: Vec<_> = model
                    .all_display_states()
                    .into_iter()
                    .filter(|s| s.name == *name)
                    .collect();
                match all.as_slice() {
                    [one] => Ok(one.id),
                    [] => Err(format!(
                        "There is no display state called '{name}'. {}",
                        names()
                    )),
                    _ => Err(format!(
                        "Several configurations have a display state called '{name}': make the one you mean active first (configuration), or give its id (display_states lists them)."
                    )),
                }
            }
        }
    }
}

/// Which display states a change to a component's look is made in.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayScope {
    /// Every display state the active configuration has.
    All,
    Named(Vec<DisplayStateSel>),
}

impl DisplayScope {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::String(s) if s == "all" => Ok(Self::All),
            Value::Array(_) => list(v)?
                .iter()
                .map(DisplayStateSel::parse)
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Named),
            other => DisplayStateSel::parse(other).map(|s| Self::Named(vec![s])),
        }
    }

    fn resolve(&self, doc: &Document) -> Result<Vec<DisplayStateId>, String> {
        let ids: Vec<DisplayStateId> = match self {
            Self::All => doc.model.display_states().iter().map(|s| s.id).collect(),
            Self::Named(sels) => sels
                .iter()
                .map(|s| s.resolve(doc))
                .collect::<Result<_, _>>()?,
        };
        if ids.is_empty() {
            return Err(
                "The assembly has no display states yet: leave 'display_states' out to change the component itself, or add one with add_display_state."
                    .to_owned(),
            );
        }
        Ok(ids)
    }
}

/// A change to how a component looks.
#[derive(Clone, Debug, PartialEq)]
pub enum LookChange {
    Show(bool),
    Color(Option<[u8; 3]>),
    Transparency(f32),
}

impl LookChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Show(_) => "show",
            Self::Color(_) => "set_color",
            Self::Transparency(_) => "set_transparency",
        }
    }

    fn apply(&self, look: &mut Look) {
        match self {
            Self::Show(on) => look.visible = *on,
            Self::Color(c) => look.color = *c,
            Self::Transparency(t) => look.transparency = *t,
        }
    }
}

/// What to do to a display state.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayChange {
    Rename(String),
    Delete,
}

impl DisplayChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Rename(_) => "rename",
            Self::Delete => "delete",
        }
    }
}

/// The operations of display states, with their fields, for `help`.
pub(crate) const DISPLAY_OPS: &[(&str, &str, &str)] = &[
    (
        "add_display_state",
        "name, configuration (with display states linked: the configuration it is for; default the active one)",
        "Add a display state (SolidWorks' Add Display State): a named look of the assembly, each component shown or hidden, coloured and see-through as it is now. It becomes the one shown (in another configuration, that configuration's if it is its first).",
    ),
    (
        "display_state",
        "display_state (a name or id)",
        "Show a display state: every component takes the look it has there. Not an undo step, like 'configuration'.",
    ),
    ("rename", "display_state, name", "Rename a display state."),
    (
        "delete",
        "display_state",
        "Delete a display state. Each configuration (or the assembly, when they are not linked) keeps one.",
    ),
    (
        "link_display_states",
        "on (default true)",
        "Link display states to configurations (SolidWorks' default: each configuration has display states of its own, and shows the one it last showed), or make them independent (every configuration can show every one).",
    ),
    (
        "display_states",
        "",
        "The display states: which configuration each is for (when linked), which is shown, and how each component looks in each where it is not shown, uncoloured and opaque.",
    ),
];

/// Reads the operation called `op`, if `op` is one of these for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let state = |a: &mut Args| a.required("display_state", DisplayStateSel::parse);
    let change = |a: &mut Args, change: Result<DisplayChange, String>| {
        Ok(Op::EditDisplayState {
            state: state(a)?,
            change: change?,
        })
    };
    let look = |a: &mut Args, change: Result<LookChange, String>| {
        Ok(Op::Look {
            component: a.required("component", CompSel::parse)?,
            change: change?,
            display_states: a.required("display_states", DisplayScope::parse)?,
        })
    };
    Some(match op {
        "add_display_state" => (|| {
            Ok(Op::AddDisplayState {
                name: a.required("name", |v| text(v).map(str::to_owned))?,
                configuration: a.string("configuration")?,
            })
        })(),
        "display_state" => state(a).map(|state| Op::DisplayState { state }),
        "rename" if a.has("display_state") => {
            let to = a
                .required("name", |v| text(v).map(str::to_owned))
                .map(DisplayChange::Rename);
            change(a, to)
        }
        "delete" if a.has("display_state") => change(a, Ok(DisplayChange::Delete)),
        "link_display_states" => a.flag("on", true).map(|on| Op::LinkDisplayStates { on }),
        "display_states" => Ok(Op::Query(crate::op::Query::DisplayStates)),
        // A component's look in display states other than the one shown.
        "show" if a.has("component") && a.has("display_states") => {
            let on = a.flag("on", true).map(LookChange::Show);
            look(a, on)
        }
        "set_color" if a.has("component") && a.has("display_states") => {
            let color = crate::assembly::component_color(a).map(LookChange::Color);
            look(a, color)
        }
        "set_transparency" if a.has("display_states") => {
            let t = crate::assembly::transparency(a).map(LookChange::Transparency);
            look(a, t)
        }
        _ => return None,
    })
}

/// A component's look in a few fields: what is not the default.
fn look_out(look: &Look) -> Map<String, Value> {
    let mut m = Map::new();
    if !look.visible {
        m.insert("hidden".to_owned(), json!(true));
    }
    if let Some(c) = look.color {
        m.insert("color".to_owned(), crate::library::color_out(c));
    }
    if look.transparency > 0.0 {
        m.insert(
            "transparency".to_owned(),
            json!(crate::args::round(f64::from(look.transparency))),
        );
    }
    m
}

/// A display state in a few fields: what replies and `display_states` give.
pub(crate) fn state_out(doc: &Document, id: DisplayStateId) -> Value {
    let model = &doc.model;
    let (Some(assembly), Some(s)) = (model.assembly(), model.display_state(id)) else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let mut m = Map::new();
    m.insert("id".to_owned(), json!(s.id.0));
    m.insert("name".to_owned(), json!(s.name));
    if let Some(c) = s.configuration.and_then(|c| model.configuration(c)) {
        m.insert("configuration".to_owned(), json!(c.name));
    }
    let shown = model.active_display_state().is_some_and(|a| a.id == id);
    m.insert("shown".to_owned(), json!(shown));
    let looks: Vec<Value> = assembly
        .components()
        .filter_map(|c| {
            let look = model.look_in(c.id, id)?;
            let mut o = look_out(&look);
            if o.is_empty() {
                return None;
            }
            o.insert("component".to_owned(), json!(c.name));
            Some(Value::Object(o))
        })
        .collect();
    m.insert("looks".to_owned(), Value::Array(looks));
    Value::Object(m)
}

/// The display states, which one is shown, and whether they are linked.
pub(crate) fn states(doc: &Document) -> Result<Map<String, Value>, String> {
    the_assembly(doc)?;
    let model = &doc.model;
    let mut out = Map::new();
    out.insert("linked".to_owned(), json!(model.display_states_linked()));
    out.insert(
        "shown".to_owned(),
        json!(model.active_display_state().map(|s| s.name.clone())),
    );
    let list: Vec<Value> = model
        .all_display_states()
        .iter()
        .map(|s| state_out(doc, s.id))
        .collect();
    out.insert("display_states".to_owned(), Value::Array(list));
    Ok(out)
}

/// Adds a display state. Returns it, with the undo label.
pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    name: &str,
    configuration: Option<&str>,
) -> Result<(DisplayStateId, String), String> {
    the_assembly(doc)?;
    let config = configuration
        .map(|c| crate::value::configuration(doc, c))
        .transpose()?;
    let id = model.add_display_state(name, config)?;
    Ok((id, format!("Add Display State {}", name.trim())))
}

/// Renames or deletes a display state. Returns the undo label.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: DisplayStateId,
    change: &DisplayChange,
) -> Result<String, String> {
    let was = doc
        .model
        .display_state(id)
        .map(|s| s.name.clone())
        .ok_or_else(|| "The display state no longer exists.".to_owned())?;
    Ok(match change {
        DisplayChange::Rename(name) => {
            model.rename_display_state(id, name)?;
            format!("Rename {was}")
        }
        DisplayChange::Delete => {
            model.remove_display_state(id)?;
            format!("Delete {was}")
        }
    })
}

/// Changes a component's look in some display states. Returns the undo label.
pub(crate) fn look(
    doc: &Document,
    model: &mut Model,
    component: &CompSel,
    change: &LookChange,
    states: &DisplayScope,
) -> Result<String, String> {
    let id: CompId = component.resolve(doc)?;
    let ids = states.resolve(doc)?;
    model.set_look(id, Some(&ids), |l| change.apply(l))?;
    let name = the_assembly(doc)?.name_of(id).to_owned();
    Ok(match change {
        LookChange::Show(true) => format!("Show {name}"),
        LookChange::Show(false) => format!("Hide {name}"),
        LookChange::Color(_) => format!("Colour {name}"),
        LookChange::Transparency(_) => format!("Change Transparency of {name}"),
    })
}
