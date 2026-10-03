# Scripting reference

PeetCAD's operations are JSON objects: an `op` name and its fields. A script is a JSON
list of them, or one per line (blank lines and lines starting with `#` or `//` are
skipped). `{"op": "help"}` returns this reference as data, with every field of every
feature.

> JSON operations are applied with `peet_ops::apply_json`. Rust code builds the same
> operations as typed values (`peet_ops::Op`) and applies them with `peet_ops::apply`, so
> the compiler checks their fields. The command line and live attach to a running session
> are stages 4 and 5 of [the plan](scripting-plan.md); the design is in
> [ADR 0007](adr/0007-operations.md).

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

Sides: `start`, `end`, `side` for extrusions; `top`, `bottom`, `bend`, `wall` for sheet
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
| `delete` | `of` |

Geometry takes `as` (a label) and `construction`. An entity is an id, a label from the
same list, or a part of one: `"a.start"`, `"a.end"`, `"c.center"`, `"r.bottom"`,
`"p.2"` (a shape's curve by number), `"origin"`. Labels last for one operation; the reply
and the `feature` query give the ids.

### Features

Each adds a feature and takes `name` plus the feature's own fields (`help` lists them all
with their defaults' types):

`extrude`, `cut`, `plane`, `axis`, `point`, `coordinate_system`, `base_flange`,
`edge_flange`, `sheet_cut`, `hem`, `sketched_bend`, `jog`, `miter_flange`, `corner`,
`dimple`, `emboss`, `louver`, `linear_pattern`, `circular_pattern`, `mirror`.

- `plane`: `from` + `distance` (offset), `from` + `about` + `angle` (angled), or `a` + `b`
  (halfway between).
- `axis`: `edge`, `face` (a round face) or `a` + `b` (two planes). `point`: `x`, `y`, `z`
  or `vertex`.
- `edge_flange` and `hem` take `edges` to make one feature per edge.
- `linear_pattern` becomes a grid with `direction2`, `spacing2`, `count2`.

| Operation | Fields |
|---|---|
| `edit` | `feature`, then any of its fields |
| `rename` | `feature`, `name` |
| `suppress`, `show` | `feature`, `on` (default true) |
| `delete` | `feature` or `features` |
| `move` | `feature`, and `before`, `after` or `index` |
| `rollback` | `to` (a feature, or `"end"`) |

### Parameters and history

| Operation | Fields |
|---|---|
| `set_parameter` | `name`, `value` |
| `delete_parameter` | `name` |
| `set_units` | `length` (`mm`, `cm`, `m`, `in`, `ft`) |
| `undo`, `redo` | |

### Queries

| Operation | Gives |
|---|---|
| `status` | name, file, units, counts, failures, undo and redo labels |
| `features` | the tree, each feature's status and what it uses |
| `feature` (`feature`) | its fields; a sketch's plane, definition, entities, relations and dimensions |
| `parameters` | named values and units |
| `bodies` | bounds, volume, counts; flat size and thickness for sheet metal |
| `faces`, `edges` (`body`) | what made each face, normals, centres; edge ends and the faces they join |
| `bend_table` (`body`) | flat size, area, and each bend |
| `checks` (`body`) | manufacturing checks |
| `help` | this reference as data |

### Files

| Operation | Fields |
|---|---|
| `save` | `path` (optional once the part has a file), `caches` (default true) |
| `export` | `path`, `format` (`stl`, `dxf`, `step`; taken from the path if absent), `body`, `schema` (`ap214`, `ap242`) |
