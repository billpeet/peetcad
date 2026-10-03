# Scripting reference

PeetCAD's operations are JSON objects: an `op` name and its fields. A script is a JSON
list of them, or one per line (blank lines and lines starting with `#` or `//` are
skipped). `{"op": "help"}` returns this reference as data, with every field of every
feature.

> From a terminal, `peet` applies them to a part file: see [The command line](#the-command-line).
> Rust code builds the same operations as typed values (`peet_ops::Op`) and applies them
> with `peet_ops::apply`, so the compiler checks their fields; JSON goes through
> `peet_ops::apply_json`. The design is in [ADR 0007](adr/0007-operations.md).

## Example

```json
{"op": "set_parameter", "name": "thickness", "value": "1.5mm"}
{"op": "sketch", "on": "top", "draw": [
  {"type": "rectangle", "from": [0, 0], "to": [200, 150], "as": "r"},
  {"type": "coincident", "of": ["r.bottom.start", "origin"]},
  {"type": "length", "of": ["r.bottom"], "value": 200},
  {"type": "length", "of": ["r.right"], "value": 150}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": "thickness", "radius": 2}
{"op": "edge_flange", "length": 25, "offset_start": 10, "offset_end": 10,
 "edges": [{"between": [[0, 0, 1.5], [200, 0, 1.5]]}, {"between": [[200, 0, 1.5], [200, 150, 1.5]]}]}
{"op": "sketch", "on": {"at": [50, 50, 1.5], "normal": [0, 0, 1]},
 "draw": [{"type": "circle", "center": [20, 20], "radius": 2.5}]}
{"op": "sheet_cut", "sketch": "Sketch2"}
{"op": "bend_table"}
{"op": "export", "path": "panel.dxf"}
```

(Shown wrapped; in a line-per-operation script each operation is on one line.)

## The command line

`peet` applies operations to a part without a window.

```sh
peet run [SCRIPT...] [options]     # the operations of scripts (- or none: standard input)
peet op JSON... [options]          # operations given as JSON
peet OPERATION [field=value...]    # one operation, by name
peet ops [OPERATION]               # the operations, or one with its fields
peet skills [NAME]                 # how to use peet, for an agent
peet new PART.peet                 # an empty part file
peet dump PART.peet [OUT.ron]      # a part as readable text
peet pack IN.ron PART.peet         # and back
```

| Option | |
|---|---|
| `-f`, `--file PART.peet` | the part to work on. Without it: a new, empty part, in memory |
| `-o`, `--out PART.peet` | save the result here, not over `--file` |
| `--new` | start from an empty part even if `--file` exists (it is overwritten) |
| `--no-save` | don't save the part, even if it changed |
| `--no-caches` | save without the geometry caches (smaller; the application rebuilds on open) |
| `--keep-going` | carry on after an operation that can't be applied |
| `--strict` | fail if the part ends with features that can't be built |
| `--materials CSV` | use these material and gauge tables, not the built-in ones |
| `--pretty` | indent the replies |
| `-q`, `--quiet` | say nothing on standard error |

- **Replies** go to standard output, one line of JSON per operation, in order. What was
  saved, and problems outside an operation, go to standard error.
- **A field** written `field=value` is JSON if it reads as JSON (`depth=8`, `flip=true`,
  `edge={"between":[[0,0,8],[80,0,8]]}`) and text otherwise (`value=2mm`,
  `sketch=Sketch1`, `path=flat.dxf`). To pass text that reads as JSON, quote it as JSON:
  `name="8"`.
- **Saving.** The part is saved to `--out`, or else back to `--file`, if it changed and
  every operation was applied. If an operation was not applied, nothing is saved, even
  with `--keep-going`. A script can also save and export where it likes with the `save`
  and `export` operations.
- **Features that can't be built** are not failed operations: the reply lists them under
  `failures` and the part is saved. `--strict` makes them a failure.
- **Other documents.** A script can open more documents beside the part
  ([several documents](#several-documents)); it saves those itself.
- **Material tables and check limits** are the built-in ones (or `--materials`), and
  changes to them last for the run. The command line does not read or change the
  application's settings.

### Skills for agents

`peet skills` lists instructions written for an agent that is going to use `peet`, and
`peet skills NAME` prints one. `core` is the one to start with: it gives the working loop
and points to the others (`sketching`, `selectors`, `solids`, `sheet-metal`) for when a
task reaches them. They are in `crates/peet-cli/skills/` and are built into the binary,
so they describe the version being run. Every script in them is run by a test, and
[AGENTS.md](../AGENTS.md) asks that a new feature is added to them.

| Exit code | |
|---|---|
| 0 | every operation was applied |
| 1 | an operation was not applied, or `--strict` found features that can't be built. Nothing was saved |
| 2 | the command line or a file was the problem. Nothing was applied |

The application's own operations (`view`, `toggle`, `window`, …) need a running PeetCAD,
so `peet` refuses them. Applying operations to a part that is open in the application is
stage 5 of [the plan](scripting-plan.md).

## Replies

```json
{"ok": true, "op": "extrude", "created": [{"id": 2, "name": "Extrude1", "type": "Extrude", "status": "ok"}]}
{"ok": false, "op": "edit", "error": "'edit' has no field 'depht'. Its fields are: depth, end, …"}
```

- `ok: false` means the operation was not applied and nothing changed; `error` says why.
- `created` lists new features; `feature` is the one an operation was about.
- `failures` lists every feature that currently can't be built, with the reason. A
  feature that fails to build is still added (`"status": "failed"`), as in the
  application: edit or delete it.
- Queries add their data to the reply.

## Values

| Kind | Written as |
|---|---|
| length | a number in document units, or text: `"25mm"`, `"1in"`, `"2 * thickness"` |
| angle | a number in degrees, or an expression |
| feature | its name (`"Extrude1"`) or id (`3`) |
| point in a sketch | `[x, y]` in the sketch plane |
| point in the model | `[x, y, z]` |

## Selectors

A selector must match exactly one thing. If it matches none or several, the error lists
the candidates. All the fields given must match.

| Wanted | Selector |
|---|---|
| plane | `"top"`, `"front"`, `"right"`, a reference plane's name, or a face selector (a flat face) |
| axis | `"x"`, `"y"`, `"z"`, a reference axis's name, or an edge selector |
| point | `"origin"`, a reference point's name, or a vertex selector |
| face | `{"at": [x,y,z]}` a point on it · `{"normal": [x,y,z]}` its outward normal · `{"feature": "Extrude1", "side": "end"}` what made it · `{"body": 0, "index": 5}` from `faces` |
| edge | `{"between": [[x,y,z],[x,y,z]]}` its ends · `{"at": [x,y,z]}` a point on it · `{"faces": [face, face]}` where two faces meet · `{"body": 0, "index": 7}` from `edges` |
| vertex | `{"at": [x,y,z]}` · `{"body": 0, "index": 2}` |

Sides: `start`, `end`, `side` for extrusions, revolves, sweeps and holes; `blend` for a
fillet's or chamfer's own faces; `inner` for the inside of a shell; `top`, `bottom`, `bend`, `wall` for sheet
metal. A point on an edge is on two faces: add `normal` to say which. Indices are only
valid until the part changes; every other selector is stored as a reference that follows
the geometry through later edits.

## Operations

### Sketches

| Operation | Fields |
|---|---|
| `sketch` | `on` (plane), `name`, `draw` |
| `draw` | `sketch`, `draw` |
| `set_dimension` | `sketch`, `name` (`"d1"`), `value` |

A `draw` list holds items with a `type`:

| Types | Fields |
|---|---|
| `point` | `at` |
| `line` | `from`, `to` |
| `polyline` | `points`, `closed` |
| `circle` | `center`, `radius` |
| `arc` | `center`, `start`, `end` (counter-clockwise) |
| `rectangle` | `from`, `to` (parts `bottom`, `right`, `top`, `left`) |
| `center_rectangle` | `center`, `corner` |
| `slot` | `from`, `to`, `radius` |
| `polygon` | `center`, `vertex`, `sides` |
| `coincident`, `horizontal`, `vertical`, `parallel`, `perpendicular`, `tangent`, `equal`, `concentric`, `midpoint`, `symmetric`, `fix` | `of` (entities) |
| `distance`, `length`, `horizontal_distance`, `vertical_distance`, `radius`, `diameter`, `angle` | `of`, `value`, `name`, `driven` |
| `fillet` | `corner`, `radius` |
| `trim`, `extend` | `curve`, `near` |
| `offset` | `of`, `distance` |
| `mirror` | `of`, `axis` |
| `construction` | `of`, `on` |
| `delete` | `of` (entities) |
| `remove` | `relations` (ids), `dimensions` (names) |

Geometry takes `as` (a label) and `construction`. An entity is an id, a label from the
same list, or a part of one: `"a.start"`, `"a.end"`, `"c.center"`, `"r.bottom"`,
`"p.2"` (a shape's curve by number), `"origin"`. Labels last for one operation; the reply
and the `feature` query give the ids.

### Features

Each adds a feature and takes `name` plus the feature's own fields (`help` lists them all
with their defaults' types):

`extrude`, `cut`, `revolve`, `cut_revolve`, `sweep`, `cut_sweep`, `loft`, `cut_loft`,
`hole`, `fillet`, `chamfer`, `shell`, `draft`, `convert_to_sheet`, `plane`, `axis`, `point`, `coordinate_system`, `base_flange`,
`edge_flange`, `sheet_cut`, `hem`, `sketched_bend`, `jog`, `miter_flange`, `corner`,
`dimple`, `emboss`, `louver`, `linear_pattern`, `circular_pattern`, `mirror`.

- `plane`: `from` + `distance` (offset), `from` + `about` + `angle` (angled), or `a` + `b`
  (halfway between).
- `axis`: `edge`, `face` (a round face) or `a` + `b` (two planes). `point`: `x`, `y`, `z`
  or `vertex`.
- `edge_flange` and `hem` take `edges` to make one feature per edge.
- `linear_pattern` becomes a grid with `direction2`, `spacing2`, `count2`; `"direction2":
  null` makes it a row again.
- A reference that the application lets the user pick afterwards (a flange's or hem's
  `edge`, a draft's `neutral`, a sweep's `path`) can be `null`: the feature is added
  waiting for it. Leaving it out is an error.
- `revolve` takes `axis`: `"sketch_x"`, `"sketch_y"`, `{"line": id}` (a line of its sketch)
  or an axis selector. Left out, it is the sketch's first construction line, else
  `sketch_y`.
- `sweep` takes `profile` and `path` (two sketches). Corners between straight pieces of
  the path are mitred.
- `loft` takes `profiles`: two or more sketches in order, each with the same number of
  edges (a circle adapts).
- `convert_to_sheet` turns a solid of one wall thickness into a sheet metal body. It takes
  `face` (the flat face that stays fixed; left out, the largest flat face of the only
  body), `bend`, `relief` and `relief_ratio`.
- `hole` drills at every point of its sketch. `standard` (`{"size": "M6", "fit": "close"
  | "normal" | "loose" | "tapped"}`) sets every size; sizes given as well win.
- `fillet` and `chamfer` take `edges` and `size`; `shell` takes `open` (faces to remove)
  and `thickness`; `draft` takes `faces` (flat, or round along
  the pull), `neutral` (a plane) and `angle`.

| Operation | Fields |
|---|---|
| `edit` | `feature`, then any of its fields |
| `rename` | `feature`, `name` |
| `suppress`, `show` | `feature`, `on` (default true) |
| `show` | `datum` (`origin`, `front`, `top`, `right`, `planes`), `on`: the built-in reference geometry |
| `delete` | `feature` or `features` |
| `move` | `feature`, and `before`, `after` or `index` |
| `rollback` | `to` (a feature, `"end"` or `"start"`) |

### Parameters and history

| Operation | Fields |
|---|---|
| `set_parameter` | `name`, `value` |
| `delete_parameter` | `name` |
| `set_units` | `length` (`mm`, `cm`, `m`, `in`, `ft`) |
| `set_material` | `material` (a name, or `null` for none), `density` in kg/m³ (left out: the material tables' density for that material). What the part is made of: saved with it, and what `mass` weighs it with |
| `set_color` | `color` (`"#rrggbb"` or `[r, g, b]`, or `null` for the usual colour): what the part is drawn in |
| `undo`, `redo` | |

### Queries

| Operation | Gives |
|---|---|
| `status` | name, file, units, material, colour, counts, failures, undo and redo labels |
| `features` | the tree, each feature's status and what it uses |
| `feature` (`feature`) | its fields; a sketch's plane, definition, entities, relations and dimensions |
| `parameters` | named values and units |
| `bodies` | bounds, volume, counts; flat size and thickness for sheet metal |
| `faces`, `edges` (`body`) | what made each face, normals, centres; edge ends and the faces they join |
| `bend_table` (`body`) | flat size, area, and each bend |
| `checks` (`body`) | manufacturing checks, and the limits they were run with |
| `materials` (`material`) | the material and gauge tables |
| `mass` (`body`) | volume, area, centre of gravity, principal moments (for a density of 1), and `mass_kg` if the part has a material; each body and the total |
| `measure` (`a`, `b`) | the exact size of a face, edge or vertex, or with `b` the distance and angle between two. Each is `{"face": selector}`, `{"edge": selector}` or `{"vertex": selector}` |
| `help` | this reference as data |

### Files

| Operation | Fields |
|---|---|
| `new` | `discard` |
| `open` | `path`, `discard` |
| `open_sample` | `sample` (`bracket`, `enclosure`, `chassis`, `housing`), `discard` |
| `save` | `path` (optional once the part has a file), `caches` (default true) |
| `import_step` | `path`: the file's solids become bodies, in one feature named after the file |
| `import_dxf` | `path`, and `sketch` (an existing one) or `on` with `name` (a new one; default the top plane); `unit`, `placement` (`keep`, `centred`, `lower_left`) |
| `export` | `path`, `format` (`stl`, `dxf`, `step`; taken from the path if absent), `body`, `schema` (`ap214`, `ap242`) |

`new`, `open` and `open_sample` replace the document. They are refused while the part has
unsaved changes, unless `discard` is true. With `"keep": true` they open a document
beside the one that is open instead (see below), and there is nothing to discard.

### Several documents

The application and the command line hold a *session*: the documents that are open
together, one of them *current*. An operation goes to the current document. Any
operation can take `"document"` (a name or an id, as `documents` lists them): it is then
applied to that document, which does not become current.

| Operation | Fields |
|---|---|
| `new`, `open`, `open_sample` with `"keep": true` | opens a document beside the others and makes it current. The reply's `document` is its id |
| `documents` | the open documents: `id`, `name`, `file`, `modified`, `current` |
| `switch` | `document`: make it current. The reply has its `status` |
| `close` | `document` (default the current one), `discard` (default false). Refused if it has unsaved changes, unless `discard` is true. Closing the last one leaves a new, empty part |

From the command line, the part of `--file` is the first document and the one a run
saves by itself, whichever is current at the end. A document opened with `keep` is saved
by a `save` operation sent to it; a run that leaves one changed and unsaved says so.
Undo is per document. `apply` and `apply_json` on a `Document` alone (Rust) refuse these:
a session is applied to with `apply_session` and `apply_session_json`.

### Sheet metal: materials, checks, flat pattern

| Operation | Fields |
|---|---|
| `flat_pattern` | `on` (left out: the other way). A view, not an undo step; selectors and exports always mean the folded part |
| `apply_material` | `material`, and `gauge` or `thickness` (the nearest gauge); `feature` (a base flange; default the first). Also makes it the part's material, if the table has a density |
| `set_gauge` | `material`, `gauge`, `thickness`, `radius`, `bend` (`{"k_factor": …}`, `{"allowance": …}` or `{"deduction": …}`), `notes`: adds a row, or changes one. `density` (kg/m³) is the material's, not the row's |
| `delete_gauge` | `material`, `gauge` (left out: the whole table) |
| `import_materials`, `export_materials` | `path` (CSV) |
| `set_check_rule` | `rule` (`min_flange`, `hole_to_bend`, `hole_to_edge`, `hole_to_hole`, `min_hole`, `collision`), `thickness`, `radius`, `constant`: the limit is that many thicknesses plus that many bend radii plus a constant in mm |

The material tables and the check limits are the user's, not the part's. In the
application they are its settings. Without it they start as the built-in ones and last
for one script run.

### The application

These are about the application's window, not the part, so they need a running PeetCAD.
Without one they are refused.

| Operation | Fields |
|---|---|
| `view` | `to` (`isometric`, `front`, `back`, `left`, `right`, `top`, `bottom`) |
| `zoom_to_fit` | |
| `toggle` | `what` (`perspective`, `grid`, `view_cube`, `feature_tree`, `properties`, `performance_overlay`, `relations`, `construction`), `on` (left out: the other way) |
| `window` | `open` (`command_palette`, `settings`, `keyboard_shortcuts`, `about`, `parameters`, `bend_table`, `checks`, `materials`, `mass`) |
| `edit_sketch` | `sketch`: open it in the sketch editor |
| `exit_sketch` | finish the open sketch, writing it to the part |
| `tool` | `tool` (`select`, `line`, `rectangle`, `center_rectangle`, `circle`, `arc`, `slot`, `polygon`, `point`, `trim`, `extend`, `fillet`, `offset`, `mirror`, `dimension`) |
| `quit` | |

While a sketch is open for editing in the application, operations that change the part
are refused until it is finished (`exit_sketch`); queries are not.

## Every command has an operation

`peet_ui::coverage` names the operation that covers each command of the application. It
matches every command, so a new command does not compile until it has one, and a test
checks that the operation named exists.

The application itself changes a part only through operations, and keeps the ones it has
applied (`PeetApp::journal`): see [ADR 0007](adr/0007-operations.md).
