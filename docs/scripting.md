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
and points to the others (`sketching`, `selectors`, `solids`, `sheet-metal`,
`assemblies`) for when a task reaches them. They are in `crates/peet-cli/skills/` and are built into the binary,
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
| `mass` (`body`) | volume, area, centre of gravity, principal moments (for a density of 1), and `mass_kg` if the part has a material; each body and the total. In an assembly: see [Assemblies](#assemblies) |
| `measure` (`a`, `b`) | the exact size of a face, edge or vertex, or with `b` the distance and angle between two. Each is `{"face": selector}`, `{"edge": selector}` or `{"vertex": selector}` |
| `help` | this reference as data |

### Files

| Operation | Fields |
|---|---|
| `new` | `discard` |
| `open` | `path`, `discard` |
| `open_sample` | `sample` (`bracket`, `enclosure`, `chassis`, `housing`, `cover`, `bolt`, `screw`, or `assembly`: an assembly of the last five, mated), `discard` |
| `save` | `path` (optional once the part has a file), `caches` (default true: a part's file keeps its built bodies and their display meshes, so it opens without rebuilding; an assembly's keeps the display meshes the application has drawn) |
| `import_step` | `path`. In a part: the file's solids become bodies, in one feature named after the file. In an assembly: the file's parts and assemblies become parts and sub-assemblies, placed as components where the file has them (fixed); replies `components`, `parts`, `bodies` |
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

### Assemblies

An assembly is a document with components in place of features: `status` gives
`"kind": "assembly"`. A component is a placed instance of a part, and the part is copied
into the assembly (once, however many components use it). The operations on features and
bodies are refused in an assembly, and these are refused in a part; each says what to do
instead. The design is in [ADR 0009](adr/0009-assemblies.md).

| Operation | Fields |
|---|---|
| `new` with `"assembly": true` | starts an empty assembly |
| `insert` | the part: `path` (a `.peet` file; an assembly becomes a sub-assembly), `sample`, `part` (an open document, by name or id) or `component` (another instance of its part). `name`, `at` (`[x, y, z]`: where the part's origin goes), `rotate` (`{"axis": "x"`, `"y"`, `"z"` or `[x, y, z]`, `"angle": degrees}`, from the part's own orientation), `fixed` (default: only the first component) |
| `insert` with `"link": true` | with `path`: the component follows the part's file instead of the assembly keeping a copy. The link is relative to the assembly's folder unless `"absolute": true` |
| `update_links` | reads every linked part from its file again. Replies `updated` (the parts that changed) and `warnings` (files that can't be found or read) |
| `link` | `part` (a part's name or id, as `components` lists the parts), `path`, `absolute` (default false): links the part to the file if there is one (the part becomes what is in it), else writes the part there first |
| `unlink` | `part`: the assembly keeps the part as it is and no longer follows the file |
| `place` | `component`, `at`, `rotate`: the ones given are set, the other is kept |
| `drag` | `component`, `to` (`[x, y, z]`, in the assembly), `point` (`[x, y, z]` on the component, in its own coordinates; default its origin): pulls the point towards the place as far as the component's mates allow. `short_by` in the reply is how far it still is |
| `fix` | `component`, `on` (default true) |
| `replace` | `component`, and `path`, `sample` or `part`: another part, in the same place |
| `rename`, `suppress`, `show`, `delete` | `component` in place of `feature` |
| `component_pattern` | `components` (the originals), `type` (`linear`, `circular`), `name`. Linear: `direction`, `spacing`, `count` (the original included), `flip`, `second` (`{direction, spacing, count, flip}`). Circular: `axis`, `angle` (default 360), `count`, `flip`. A direction or axis is `"x"`, `"y"`, `"z"`, `[x, y, z]`, `{"origin": …, "direction": …}`, or `{"component": …, "edge" or "face": selector}` |
| `edit_component_pattern` | `pattern`, and any of `count`, `spacing`, `angle`, `flip`, `direction`, `axis`, `second` (`null` removes it) |
| `rename`, `delete` | `pattern` in place of `feature` |
| `set_color` with `component` | `color` (`"#rrggbb"`, `[r, g, b]`, or `null` for its part's): a colour for that component alone |
| `show_all` | shows every hidden component; replies `shown` |
| `isolate` | `components` (names or ids): shows these and hides every other; replies `hidden` |
| `explode_step` | `components`, `by` (`[x, y, z]`: how far, along the assembly's axes), `name`: adds a step to the exploded view |
| `edit_explode_step` | `explode_step`, and `by` or `components` |
| `rename`, `delete` | `explode_step` in place of `feature` |
| `explode` | `on` (left out: the other way): shows the assembly exploded, or as it is. A view, not an undo step |
| `explode_steps` | every step (`id`, `name`, `components`, `by`), whether the assembly is shown `exploded`, and under `moved` each component the steps move with its total `by` and where it is drawn exploded (`at`) |
| `open_component` | `component`: opens its part as a document of its own and makes it current. `save` without a `path` on that document stores it back, as one undo step of the assembly, for every component of the part. Needs a session |
| `components` | every component (`id`, `name`, `part`, `at`, `rotate`, `fixed`, `freedom`, `mates`, `status`, `message`, `min`, `max`) and every part (`id`, `name`, `kind`, how many `components`, `material`); how many `mates`, and the `freedom` left |
| `mate` | `type` (`coincident`, `concentric`, `parallel`, `distance`, `angle`, `fasten`), `a`, `b`, `distance` or `angle` for those types, `flip`, `name`. An end is `{"component": …, "face": selector}` (or `"edge"`, `"vertex"`), the selector in the part's own coordinates; for `fasten` it is the component alone |
| `edit_mate` | `mate`, and `distance` or `angle`, `flip` |
| `rename`, `suppress`, `delete` | `mate` in place of `feature` |
| `mates` | every mate (`id`, `name`, `type`, `a`, `b`, `distance` or `angle`, `flip`, `status`, `message`) and the `freedom` left |

| `interference` | `component` (left out: every pair): the pairs of components that share space, each with `a`, `b`, `volume_mm3`, `min`, `max`; `clear`; how many pairs were `compared`; `unchecked` pairs that couldn't be |
| `bom` | `level` (`parts`, the default: every part, through sub-assemblies; `top`: a sub-assembly is one line): `rows`, each with `item`, `part`, `quantity`, `material`, `volume_mm3`, `mass_kg`, `total_mass_kg`, the `components`, and for sheet metal `thickness`, `flat_size`, `bends`; the total `quantity` and `mass_kg`, or `without_mass` |
| `mass` | in an assembly: each component's `volume_mm3`, `mass_kg` and `center_of_gravity`, and the `total` (with `center_of_gravity_of`: `mass` if every part has a material, else `volume`; `principal_moments_kg_mm2` or `principal_moments_mm5`); `without_material` |
| `export` to a `.csv` | the bill of materials (every part) as a table: `item`, `part`, `quantity`, `material`, `mass_kg`, `total_mass_kg`, `thickness_`, `flat_width_`, `flat_height_` (with the document's unit), `bends`, `components` |

**Component patterns** copy components in rows (one direction or two) or round an axis.
The copies are components of the same part, and `components` lists each with the
`pattern` that made it, and the patterns themselves under `patterns` (`id`, `name`,
`type`, `components`, `copies`, and the fields above, `status`, `message`). A copy is
placed by its pattern after the mates are solved: where its original is, moved by its
place in the pattern. So it has no freedom, and `place`, `drag`, `fix`, `mate`,
`replace` and `delete` on it are refused; hiding, colouring, renaming and suppressing
it are not. A direction or axis taken from a component's geometry follows that
component. Changing a pattern's count adds and removes copies; deleting a pattern
deletes its copies; deleting an original deletes its pattern.

**Visibility and the exploded view** change what is drawn, not the assembly. A hidden
component is still mated, checked, counted, weighed and exported (a suppressed one is
not). An exploded view is a list of steps stored in the assembly, each moving some of
its components by a distance; a component in several steps moves by their sum. Whether
the assembly is shown exploded is a view, like `flat_pattern`: it is not saved and not
undone, and every other operation means the assembly as it is put together.

**Interference** compares every two bodies of different components whose boxes overlap,
by intersecting them: what they share is the interference. Bodies that only touch share
nothing. **Mass** weighs each part with its own material; if a part has none, the whole
has no mass, and the centre of gravity given is that of the volume.

**Mates.** A flat face stands for its plane, a round face or a round edge for its axis,
a straight edge for a line, a vertex for a point. Two planes are put against each other
unless `flip` is true. The mates are solved whenever the assembly is rebuilt, and the
components move as little as they can from where they are: so `place` on a mated
component is a suggestion, and where a component is placed before it is mated decides
which of several positions it takes. A reply to an operation that moved other components
lists them under `moved`, and gives the `freedom` left: how many ways the components can
still move (six for each that is not fixed, less what the mates hold). Each component
has a `freedom` of its own, 0 to 6: how many ways it can still move, by itself or along
with what it is mated to (so the ones above 0 are the ones not yet held). A mate that
can't be solved is not a failed operation: it is added and listed under `failures` with
the reason, as a feature that can't be built is. `drag` moves what the mates leave free:
the component slides to the place if it can, turns only if sliding can't get it nearer,
and takes what it is mated to along. When mates contradict each other, the
earlier ones hold and the later one is flagged.

**Linked parts** (desktop only: they are files on disk, and are refused in the browser).
A linked part is read from its file when the assembly is opened, by `update_links`, and
when the part's document is saved in the same session (that `save` replies `updated_in`
with the assemblies that followed). The assembly keeps the part as it last read it, so
it opens whole when the file is missing, with a warning. A link is written in the
assembly's file relative to the folder that file is in (`../parts/pin.peet`), so an
assembly and its parts can be moved or copied together; with `"absolute": true` it is a
full path. Saving an assembly somewhere else keeps it pointing at the same part files
(the relative links are rewritten from the new place). If nothing is where a link says,
a file of the same name next to the assembly's is used. `components` gives a linked
part's `link` (as the file has it), `link_file` (the file it means now) and
`link_status` (`current`, `changed`, `missing`). In the application, an open assembly
is told by the system when a linked part's file is changed by another program, and
reads the part again a moment later (this can be turned off in the settings); the
command line reads them at each call. `open_component` on a linked part opens its file as a document; on a part
of the assembly's own, a working copy that `save` stores back.

`failures` (in `status` and in replies) lists the components that need attention (a part
with no bodies, or with features that can't be built) and the mates that don't hold. `export` writes every component's
bodies where they are, as STEP or STL. A STEP file has the assembly's structure: each
part and sub-assembly once, as a product, and each component as an occurrence of it
(suppressed components are left out); the reply counts `parts`, `components` and
`bodies`. `import_step` in an assembly reads such a file back with its structure. A reply about a component gives it under `component`.

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
