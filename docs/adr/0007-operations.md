# ADR 0007: Operations as data

**Status:** accepted (scripting stage 2, see [the plan](../scripting-plan.md))

## Context

An agent or a script must be able to do what a user does, headless and (later) against a
running session. Before this, a user action was a `CommandId` with no arguments: what it
acted on came from the selection, and the logic sat in the UI. Geometry references came
only from mouse clicks.

## Decision

**An operation is a value of one type, `peet_ops::Op`**, applied by one function:
`apply(&mut Document, &Op, Undo) -> Reply`. The crate (`peet-ops`) sits on
`peet-document` and has no UI dependency.

**JSON is one way of writing an `Op`.** A script or an agent writes
`{"op": "extrude", "sketch": "Sketch1", "depth": 8}`; `apply_json` reads it into the same
`Op` and applies it. [The reference](../scripting.md) lists the operations as JSON. Rust
callers (the application, tests, later plugins) build the `Op` directly, so a wrong field
name or type is a compile error:

```rust
Op::add(FeatureArgs::EdgeFlange(EdgeFlange {
    edge: Some(clicked_edge.into()),
    length: Some("flange".into()),
    ..Default::default()
}))
```

JSON is read by our own code, not by `serde` derives, to keep the messages specific: an
unknown field lists the fields the operation has, and an unknown operation lists the
operations.

**A feature's fields are listed once.** Each kind of feature has one table
(`feature_args!` in `fields.rs`) naming its fields, the kind of value each takes and
where it is stored. A macro turns the table into the typed arguments (`EdgeFlange`
above), reading them from JSON, creating the feature, editing it, reading it back and
`help`. There is no second copy of a feature's fields to keep in step. Each table
destructures its feature exhaustively, so a field added to a feature in `peet-model` does
not compile until it is listed (or left out on purpose). Every kind of feature is likewise
matched in one place, so a new kind does not compile until it is given a table (or is said
to have no fields, as an imported body is).

The same arguments serve creating and editing: every field is optional, and one left out
keeps its default in a new feature and its value in an edit.

**A selector is either a description or a reference.** A description (`{"at": …}`,
`{"feature": "Extrude1", "side": "end"}`, `{"between": [p, q]}`) is matched against the
part as it is now, and stored as the `FaceRef` / `EdgeRef` / `VertexRef` a click would
have made ([ADR 0001](0001-persistent-naming.md)), so a scripted reference follows the
geometry through later edits exactly as a picked one does. A description must match
exactly one thing; none or several is an error listing the candidates. It never picks the
nearest of several silently. A reference that is already resolved (what the application
holds after a click) is stored as it is, without being described and found again. Scripts
write descriptions; the application will pass references.

**An operation is atomic.** It works on a copy of the model (cheap: features are shared),
and the copy replaces the model only once every field has been read and accepted. A
misspelt field, a selector that matches nothing or an expression that doesn't evaluate
changes nothing. File writes and undo check their fields first for the same reason.

**A feature that fails to rebuild is not an error.** The operation was applied; the reply
lists the feature under `failures` with the reason, as the feature tree would show it.
The script can then edit or delete it. This matches the application and keeps "the part
is in the state my operations describe" true.

**Undo.** Each operation is one undo step with a label ("Add Extrude1"), through
`Document::change`. A script run can group its operations into one step
(`Undo::Group`), using the merging that drags already use.

**Units.** A number is in document units (degrees for angles); text is an expression
(`"2 * thickness"`, `"1in"`). Coordinates in selectors are model coordinates; in `draw`
lists, sketch coordinates. Replies use document units, except areas and volumes
(`_mm2`, `_mm3`).

**Sketch entities** are referred to by id. Within one `draw` list they can be given
labels (`"as": "a"`, then `"a.end"`); labels are not stored in the sketch, so later
operations use the ids that replies and the `feature` query give.

**Every command has an operation.** `peet_ui::coverage` maps each `CommandId` to the
operation that does what it does. The match is exhaustive, so a new command does not
compile until it is covered, and a test checks that each operation named exists.

**Operations run in a host.** Some things an operation needs are not the part's: the
material tables and the limits of the manufacturing checks are the user's settings.
`apply_in(host, …)` takes them from a `Host`: the application (its settings), or a
`Headless` host that starts with the built-in ones and lasts for a script run. `apply`
is `apply_in` with a fresh `Headless`.

**Commands about the application are operations too** (`Op::App`): turn the view, toggle
the grid, open a window, open a sketch for editing, pick a sketch tool, quit. Only the
application can carry them out, through `PeetApp::apply_op`; a headless host refuses
them, saying why. Commands whose windows show data (parameters, bend table, checks,
materials, mass) are covered twice: an operation that returns the data, and `window` to
open the window.

**A sketch open for editing blocks changes from outside.** Its working copy is not in the
part yet, so `PeetApp::apply_op` refuses operations that change the part until the sketch
is finished. Queries and application commands are not refused.

**Replacing the document is guarded.** `new`, `open` and `open_sample` are refused while
the part has unsaved changes unless told to discard them, as the application asks first.

## Consequences

- `PeetApp::apply_op` is the way in for operations from outside, and is tested against
  the application without a window; nothing calls it yet, because there is no transport
  (stage 5).

## The application's own changes (stage 3)

**Everything the application does to a part is applied as operations.** Its tools
(start an extrusion from the selection, edit a value in the properties, drag a feature
up the tree) work out the change on a copy of the model, as they did before.
`peet_ops::apply_model` then finds the operations that make that change (`diff`) and
applies them as one undo step with the tool's label. So the tools were not rewritten,
and yet nothing reaches the part except through operations.

The translation is possible because a feature can be read back as the arguments that
would set it: each kind of field says how (`Field::arg`), so the one table per feature
also gives `FeatureArgs::of` (everything a feature has) and `FeatureArgs::changes` (what
differs between two). Values the application already holds go across exactly:
`Input::Base` is a value in base units, and a reference that is already resolved is
passed as it is.

**A gap in the operations can't go unnoticed.** If a change can't be expressed as
operations, or (in development builds) the operations arrive at a different model than
the tool asked for, the change is made directly so the user loses nothing, an error is
logged, and `PeetApp::untranslated` lists it. Tests rebuild all four sample parts from
nothing through `apply_model`, and make every kind of change the tools make, comparing
with the same change made directly.

**Features can be waiting for a pick.** The application adds a flange before its edge is
chosen. So the optional references (a flange's edge, a draft's neutral plane, a sweep's
path) can be given as `null`: "to be picked afterwards". Leaving one out is still an
error, so a script that forgets it is told.

**New operations for the application.** `Op::SetSketch` replaces what is drawn in a
sketch (how the sketch editor commits its work; a script draws with `draw`). Files can
be given as their contents (`Source`), since file dialogs and the browser hand over
contents, not paths. `rollback` can go above the first feature (`"start"`). An edit with
the fields of another form of the same feature (an offset plane made an angled one)
replaces its definition.

**The journal.** `PeetApp::journal` holds the operations applied, from the interface and
from outside: what the user did, as a script would do it.

What this leaves:

- The tools still build model changes, not operations. Moving each to build its
  operations (as the solid modelling commands, undo, the flat pattern and new, open and
  the samples now do) would make the translation unnecessary; until then a change costs
  one rebuild per operation it translates to (usually one), and development builds
  rebuild once more to check.
- The journal can't be written out as a script yet: operations are read from JSON, not
  written. A resolved reference would have to be written as a description that finds it
  again.
- Not through operations: saving (it goes through the platform's dialogs), STL export
  (it exports what is shown, flat pattern included), edits in the material tables window
  (they change the settings directly), importing a DXF into a sketch that is open for
  editing (it goes into the editor's working copy), and recovering unsaved work at
  startup.
- `peet_document`'s `add_revolve` and its siblings are no longer used by the
  application.
- There is no command line or live attach yet (stages 4 and 5): `apply` is callable from
  Rust and from tests only.
- What depends on the part is still checked when an operation is applied, typed or not:
  feature names, descriptions that match nothing, expressions that don't evaluate, and an
  edit whose fields are of another kind of feature than its target.
- The typed arguments are not `serde` types, so a JSON schema can't be derived from them.
  `help` is generated from the same tables and is the machine-readable reference; a
  schema for MCP tool definitions (stage 6) would be generated the same way.
- A sketch tool picked with `tool` still needs clicks to draw: the operation that draws
  is `draw`. There is no operation that selects things in the application, so commands
  that act on a selection are covered by operations that take what was selected as
  fields.
- Changes a headless script makes to the material tables or the check limits are not
  saved anywhere: the command line (stage 4) will have to decide whether to load and
  store the application's settings.
- Operations that read or write files (`open`, `save`, `export`, the imports) use the
  file system, so they fail cleanly in the
  browser. A web host that wants them needs a variant that returns the bytes.
- A face selector's `at` uses the body's tessellation to tell whether the point is on the
  face, so the first `at` on a body tessellates it (and only then).
