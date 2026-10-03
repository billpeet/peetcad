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
not compile until it is listed (or left out on purpose).

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

## Consequences

- The application doesn't use the operations yet: its buttons still call the model
  directly (stage 3). Until then the two can drift, and there is no macro recording.
- There is no command line or live attach yet (stages 4 and 5): `apply` is callable from
  Rust and from tests only.
- What depends on the part is still checked when an operation is applied, typed or not:
  feature names, descriptions that match nothing, expressions that don't evaluate, and an
  edit whose fields are of another kind of feature than its target.
- The typed arguments are not `serde` types, so a JSON schema can't be derived from them.
  `help` is generated from the same tables and is the machine-readable reference; a
  schema for MCP tool definitions (stage 6) would be generated the same way.
- Not covered yet: DXF import, the gauge tables and applying a material, custom limits
  for the manufacturing checks, the flat/folded view toggle and other view state.
- `save` and `export` write files through `peet-platform`, so they fail cleanly in the
  browser. A web host that wants them needs a variant that returns the bytes.
- A face selector's `at` uses the body's tessellation to tell whether the point is on the
  face, so the first `at` on a body tessellates it (and only then).
