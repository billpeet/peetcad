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
peet sessions                      # the running PeetCADs and the part each has open
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
| `--live` | apply to a running PeetCAD, and fail if there is none to apply to |
| `--headless` | work on the file, even if it is open in a running PeetCAD |
| `--pid N` | apply to the running PeetCAD with this process id |
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
`assemblies`, `configurations`, `live`) for when a
task reaches them. They are in `crates/peet-cli/skills/` and are built into the binary,
so they describe the version being run. Every script in them is run by a test, and
[AGENTS.md](../AGENTS.md) asks that a new feature is added to them.

| Exit code | |
|---|---|
| 0 | every operation was applied |
| 1 | an operation was not applied, or `--strict` found features that can't be built. Nothing was saved |
| 2 | the command line or a file was the problem. Nothing was applied |

### A part that is open in PeetCAD

If the part given with `--file` is open in a running PeetCAD, `peet` applies the
operations there and not to the file: the part changes on screen, and each operation is
an undo step of the application ([ADR 0010](adr/0010-live-attach.md)).

```sh
peet sessions                          # {"sessions": [{"pid", "file", "name", "modified", "version"}]}
peet features -f bracket.peet          # goes to PeetCAD if bracket.peet is open there
peet run build.jsonl --live            # the one running PeetCAD, whatever it has open
peet view to=front --live
```

- **Which.** `--file` open in a PeetCAD: that one. `--live` without `--file`: the only
  PeetCAD running (a part never saved has no file, so this is how to reach it). `--pid`:
  that process. `--headless`: the file, whatever is open. `--live` fails if there is
  nothing to attach to; without it a part open nowhere is worked on as a file.
- **Saving.** Nothing is saved: the part is left modified, as if the user had changed
  it. A script saves with the `save` operation.
- **Failure.** A run stops at an operation that can't be applied, and the ones before it
  stay applied.
- **`--new`, `--out` and `--materials`** are about files and are refused on a part that
  is open. The material tables and check limits are the application's settings.
- **Paths** in operations are relative to where `peet` is run.
- **Busy.** While a sketch is open for editing in the application, operations that change
  the part are refused. If the application takes no operation for 20 seconds (a file
  dialog is open), the operation is refused and not applied.
- The application's own operations (`view`, `toggle`, `window`, …) work only here.
  Without a running PeetCAD they are refused.

A running PeetCAD listens on a local socket (a named pipe on Windows) and lists itself
in `sessions/` in its data folder. Only the same user's processes can connect. Native
only: the web build does not listen.

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

Expressions support `if(condition, yes, no)` and the alias `iif`. Comparisons `<`,
`<=`, `>`, `>=`, `==` or `=`, and `!=` or `<>` return a plain 1 or 0. Arithmetic binds
before comparisons; chained comparisons are refused. Zero is false, any finite
nonzero plain number is true. Only the selected branch is evaluated, and its units
must suit the field using it. Both branches' names must exist and their dependencies
must be free of cycles. Comparisons use the same unit adoption as addition: a bare
number adopts the other operand's units in the document's units. `int(x)` is `floor(x)`,
rounding down in document units. Part pattern counts also accept expressions; a count
an expression gives is rounded to the nearest whole number, as SolidWorks rounds one.
Function names are not case sensitive (`IIF`, `INT`).

```jsonl
{"op": "set_parameter", "name": "width", "value": "300mm"}
{"op": "set_parameter", "name": "depth", "value": "iif(width > 299mm, 8mm, 4mm)"}
```

### Dimensions by name

Every number of a part has an address `NAME@FEATURE`, as SolidWorks names its
dimensions: a sketch dimension by its name (`Width@Sketch1`, `d1@Sketch1`), and any other
feature's numeric value as `d1`, `d2`, … in the order of its fields (`d1@Extrude1`), by
a name it was given, or by its field (`depth@Extrude1`). A feature's value can be given a
whole address as its name (`D2@Sketch10`: SolidWorks keeps an edge flange's length in its
profile sketch). Expressions read dimensions by address; a name that is not `word@word`
is quoted, as in SolidWorks' equations (`"Dia.@Sketch1" / 4`). A `"` after a number or
a `)` is inches. Names are the same in every configuration; an address reads and sets the
active configuration's value, and `set` takes `configurations`. Before each rebuild every
dimension's value is worked out (a sketch's reference dimension has the value the last
rebuild measured).

| Operation | Fields |
|---|---|
| `set` | `name` (an address, or a parameter), `value`, `configurations` (as for `edit`; a parameter: as for `set_parameter`). A reference dimension can't be set. An address's case doesn't matter where only one dimension matches it, as in SolidWorks. On an assembly: a mate's value `D1@MATE` (with `configurations`, of the assembly), a parameter, or a component's dimension `NAME@FEATURE@COMPONENT` (see Assemblies; `configurations` of the assembly, for its equation) |
| `name_dimension` | `feature`, `dimension` (its name or field), `name`. A sketch dimension's name has no `"` or `@`; another feature's value may take a whole address |
| `dimensions` | `feature`, `name`: every address, its feature, field, value, `driven` for a reference dimension, and each configuration's value where they differ |

```jsonl
{"op": "set", "name": "Width@Sketch1", "value": 450, "configurations": "all"}
{"op": "set", "name": "D1@LPattern2", "value": "int((\"Width@Sketch1\" - 25) / 175mm + 0.999999) + 1"}
{"op": "name_dimension", "feature": "Edge-Flange1", "dimension": "length", "name": "D2@Sketch10"}
```

## Selectors

In the UI, select a face or edge in a part's viewport and use **Properties > Names** to
add a name or remove an existing binding. With nothing selected, Properties lists
all names, including unavailable targets, so you can remove stale bindings. The same
actions are operations.

Use `name_face` with `face` and `name`, or `name_edge` with `edge` and `name`, to
bind a user name to geometry already built. `delete_name` with `name` removes the
binding. Names are case-sensitive, unique across faces and edges in each part, saved
with the model and shared by all configurations. Select them with `{"name": "Front"}`;
other selector fields can still restrict the match. `faces` and `edges` include a
`names` array for each item. Curved faces, including hole walls, can be named and
used in mates; sketch planes still require flat faces. Names follow the existing
persistent references through rebuilds, selecting one best match after a split.
Suppression, rollback or deletion can leave a name unresolved. Restore its geometry
or remove and reassign the name. Existing sketch and mate references survive removal
of the user name.

A selector must match exactly one thing. If it matches none or several, the error lists
the candidates. All the fields given must match.

| Wanted | Selector |
|---|---|
| plane | `"top"`, `"front"`, `"right"`, a reference plane's name, or a face selector (a flat face) |
| axis | `"x"`, `"y"`, `"z"`, a reference axis's name, or an edge selector |
| point | `"origin"`, a reference point's name, or a vertex selector |
| face | `{"name": "Front"}` a user name · `{"at": [x,y,z]}` a point on it · `{"normal": [x,y,z]}` its outward normal · `{"feature": "Extrude1", "side": "end"}` what made it · `{"body": 0, "index": 5}` from `faces` |
| edge | `{"name": "HingeEdge"}` a user name · `{"between": [[x,y,z],[x,y,z]]}` its ends · `{"at": [x,y,z]}` a point on it · `{"faces": [face, face]}` where two faces meet · `{"body": 0, "index": 7}` from `edges` |
| vertex | `{"at": [x,y,z]}` · `{"body": 0, "index": 2}` |

Sides: `start`, `end`, `side` for extrusions, revolves, sweeps and holes; `blend` for a
fillet's or chamfer's own faces; `inner` for the inside of a shell; `top`, `bottom`, `bend`, `wall` for sheet
metal. A point on an edge is on two faces: add `normal` to say which. Indices are only
valid until the part changes; every other selector is stored as a reference that follows
the geometry through later edits. A face picked with `at` keeps that point: where the face
is in pieces later (a cut before it unsuppressed, or another configuration), it is the
flat piece at that point, as SolidWorks keeps each piece picked.

## Operations

### Sketches

| Operation | Fields |
|---|---|
| `sketch` | `on` (plane), `name`, `draw` |
| `draw` | `sketch`, `draw` |
| `set_dimension` | `sketch`, `name` (`"d1"`), `value`, `configurations` |
| `set_text` | `sketch`, `text` (its id; left out: the sketch's only text), `string`, `font`, `height`, `angle`, `width`, `spacing`, `align` |

A `draw` list holds items with a `type`:

| Types | Fields |
|---|---|
| `project` | exactly one of `edge`, `vertex`, `plane`, `face` selectors, or `sketch` with `entity` (another sketch's entity); `convert` includes projected edges in profiles |
| `point` | `at` |
| `line` | `from`, `to` |
| `polyline` | `points`, `closed` |
| `circle` | `center`, `radius` |
| `arc` | `center`, `start`, `end` (counter-clockwise) |
| `spline` | `points`, `closed` (a smooth curve through the points; a closed one returns to the first, which is not repeated) |
| `rectangle` | `from`, `to` (parts `bottom`, `right`, `top`, `left`) |
| `center_rectangle` | `center`, `corner` |
| `slot` | `from`, `to`, `radius` |
| `polygon` | `center`, `vertex`, `sides` |
| `text` | `at`, `string`, `height`, `font`, `angle`, `width`, `spacing`, `align` (part `point`) |
| `coincident`, `horizontal`, `vertical`, `parallel`, `perpendicular`, `tangent`, `equal`, `concentric`, `collinear`, `coradial`, `midpoint` (of a line or an arc), `symmetric`, `fix` | `of` (entities) |
| `patterned` | `of`: a seed, a copy of it in a linear sketch pattern, and the pattern's first copy where that is another (points, lines or circles) |
| `distance`, `doubled_distance`, `length`, `horizontal_distance`, `vertical_distance`, `radius`, `diameter`, `angle`, `offset_distance` | `of`, `value`, `name`, `driven` (`offset_distance`: a curve, then the curve it is offset from) |
| `fillet` | `corner`, `radius` |
| `trim`, `extend` | `curve`, `near` |
| `offset` | `of`, `distance` |
| `mirror` | `of`, `axis` |
| `construction` | `of`, `on` |
| `delete` | `of` (entities) |
| `remove` | `relations` (ids), `dimensions` (names) |

Geometry takes `as` (a label) and `construction`. An entity is an id, a label from the
same list, or a part of one: `"a.start"`, `"a.end"`, `"c.center"`, `"r.bottom"`,
`"p.2"` (a shape's curve by number), `"s.2"` (a spline's fit point by number, from 0),
`"t.point"` (a text's point), `"origin"`. Labels last for one operation; the reply and the `feature` query give the ids.

`project` stores a persistent model reference and refreshes locked geometry before
solving. It defaults to construction geometry. `convert: true` makes projected edges
profile geometry; `face` projects every boundary edge, addressed as `label.0`, etc.
Vertices project to points and planes to their intersection line with the sketch.
`sketch` with `entity` projects an entity of an earlier sketch (its id there): points and
lines into any sketch, circles and arcs into a parallel one; it follows that sketch.
Straight edges and circles/arcs parallel to the sketch plane are supported; a circle or
arc seen edge-on (its plane square to the sketch's) projects to the line it covers. Parallel
planes, collapsed edges and other projected curve types fail with a diagnostic. A
projection whose source is suppressed (in a configuration, say) or gone stays where it
was and the sketch gets a warning, as SolidWorks leaves a dangling relation; the sketch
is not suppressed with its source unless it is drawn on it.
Relations and dimensions use the projected ids like other sketch entities. Delete a
projection by its root entity id to remove its source and attached relations.
The `feature` query returns `projections` and marks reference entities `locked`.
The application adds the supporting face's boundary references when opening a sketch
for editing, so Smart Dimension and relations can select its edges and corners.
Scripts obtain those references with an explicit `project` item.

A new dimension value (`set_dimension`, a parameter, a configuration) moves an
under-defined sketch as SolidWorks does: moved whole, if that satisfies it (a profile
placed by a distance to the model keeps its shape); otherwise the points held by the most
relations and dimensions stay put and the rest moves; only if that can't satisfy the sketch is the
change spread over everything by least squares.

`collinear` creates parallel plus point-on-line relations; `coradial` creates concentric
plus equal-radius relations. Each returns both relation ids. `doubled_distance` is twice
the perpendicular distance from a point to a line, with its own editable dimension name.
`offset_distance` keeps a curve offset from another, as SolidWorks' Offset Entities does: two lines
parallel and the value apart, two circles or arcs concentric with radii the value apart.

A relation or dimension that the others already imply is accepted, as in SolidWorks: the
sketch stays `under_defined` or `fully_defined`, and the `feature` query marks it
`redundant`. Only relations and dimensions that contradict each other make a sketch
`over_defined`; the `feature` query marks those `conflicting`.

A spline passes through its points and has no other shape of its own: its points take
`coincident`, `fix` and dimensions like a line's ends, and moving them moves the curve.
There are no tangent handles yet, and a spline takes no `tangent` relation; nothing can
be made `coincident` with the curve itself, only with its points. `trim`, `extend`,
`offset` and `fillet` refuse splines; `mirror` copies them. A profile with a spline can
be extruded, revolved (the spline must stay clear of the axis) and lofted (a spline side
joins lines and other splines, not arcs), and a spline can be a sweep's path; sheet metal
features refuse it.

A `text` is a string drawn in a font, as SolidWorks' sketch text is. Its glyph outlines
are closed regions like any other, made of lines and arcs (within a micron), so `cut`,
`extrude` and `sheet_cut` take them. `at` is its point, on the baseline: at the left end,
the middle or the right end of each line for `align` `left` (default), `center` or
`right`; the point takes relations and dimensions as `"t.point"`. `height` is a capital
letter's height (default 5 mm), `angle` is in degrees from the sketch's x axis, `width`
is a width factor and `spacing` a character spacing (both 1 by default). `font` names an
installed TrueType or OpenType font, by family (`"Century Gothic"`, the default) or face
(`"Century Gothic Bold"`); glyphs are hinted as SolidWorks draws them, so a SolidWorks
part's text comes out the same. A font that isn't installed is replaced by the built-in
Didact Gothic, with a warning. The outline is stored with the part, so it looks the same
on a machine without the font. A line feed starts a new line below. `set_text` changes
the string (SolidWorks' `SetSketchText`) or the style and draws the text again; the reply
and the `feature` query give its `bounds`. A text is not mirrored by `mirror`.


### Features

Each adds a feature and takes `name` plus the feature's own fields (`help` lists them all
with their defaults' types):

`extrude`, `cut`, `revolve`, `cut_revolve`, `sweep`, `cut_sweep`, `loft`, `cut_loft`,
`hole`, `fillet`, `chamfer`, `shell`, `draft`, `convert_to_sheet`, `plane`, `axis`, `point`, `coordinate_system`, `base_flange`,
`edge_flange`, `sheet_cut`, `hem`, `sketched_bend`, `jog`, `miter_flange`, `corner`, `break_corner`,
`unfold`, `fold`, `dimple`, `emboss`, `louver`, `linear_pattern`, `circular_pattern`, `mirror`.

- `plane`: `from` + `distance` (offset), `from` + `about` + `angle` (angled), or `a` + `b`
  (halfway between).
- `extrude` and `cut` take `draft`: degrees the walls lean from the direction of
  extrusion, from the sketch plane, so that what is added or removed narrows away from the
  sketch (`draft_outward`: widens). Not with `mid_plane`. A `cut` with `flip_side`
  removes everything outside its regions within its reach (SolidWorks' "flip side to
  cut"); with a draft the removed part still narrows, so the regions grow. Regions picked
  side by side (`regions`) are extruded as one shape.
- `axis`: `edge`, `face` (a round face) or `a` + `b` (two planes). `point`: `x`, `y`, `z`
  or `vertex`.
- `base_flange` from an open profile takes `bends`: a bend model (as `bend`) or `null` for
  each bend in order along the profile, as SolidWorks' sketched bends can have their own.
- `edge_flange` and `hem` take `edges` to make one feature per edge. `offset_start` and
  `offset_end` set it back from the ends of its edge; a negative one runs it past that
  end, with the bend staying on the edge. An `edge_flange` can have a `relief`
  (`rectangular`, `obround`, `tear`) and a `relief_ratio` of its own for where it is set
  back, and a `bend` of its own (as the base flange's); without them, or with `null`,
  it uses the body's. A flange whose bend lies outside the sheet (`bend_outside`, or set
  out by `position_offset`) takes nothing from it and gets no relief, as in SolidWorks.
  Where its edge ends but its face goes on past the end (on in a curve, or round an inside
  corner), the bend is relieved there too, past the end of the edge, as SolidWorks does.
  Where a flange's edge runs to a corner of its face sharper than square and its bend
  starts inside the sheet, the bend ends on a slant from the face's corner to the
  flange's end, as SolidWorks' does. Where a flange's edge runs down to the bend its face
  hangs from (a lip on a wall's side) and its bend starts inside the sheet, the strip its
  bend takes runs on through that bend, which is cut away there, as in SolidWorks. `outline` is a sketch of the flange's shape (on a plane turned about
  the edge): the flange is as wide and as long as its region and cut to it, from its foot
  out; an outline that reaches the end of its edge meets a flange at that corner as a
  flange without one does. `up_to` (a vertex, a reference point or `"origin"`) runs the flange up to that point
  in place of its `length`, its end through the point square to the flange, as
  SolidWorks' "Up To Vertex"; copies of it are as long as it came out. `position_offset` sets the flange out from its edge, the sheet
  running on flat to the bend (negative: in). `position` is SolidWorks': at an angle other than 90 a
  `material_inside` flange's outer face goes through the edge on the far face of the
  sheet, `t / tan(angle)` back from where the outer faces meet.
- `sheet_cut` cuts every flange with a face in its sketch's plane, each through its own
  place in the flat pattern. A closed shape that lies over another such flange and not
  over the sketch's own cuts that flange only (holes drawn over both lips of a hat
  section from one lip go through the lips, not the base). With `end` `blind` (and `depth`) or `through_all` it reaches
  further, as SolidWorks' cut-extrude does: into the sheet from the sketch's face
  (`reverse` away from it, `both` both ways), through the flanges parallel to the sketch
  that lie wholly within the reach, and across the flanges square to it where the
  sketch's outline crosses them, on across the bend at the end of one it goes past. Its
  own sheet is cut only if the reach goes into it (`reverse` alone leaves it), and an
  outline that ends over a bend ends in it where the straight cut leaves the sheet's far
  face, square to the sheet (SolidWorks' normal cut). The default `end`, `sheet`, is
  through its own sheet only, carried across bends in the flat pattern. Its sketch can also be on a plane
  or on the sheet's edge (a face through the thickness): with a reach it then cuts from
  there, against the sketch's normal, whatever its reach takes in; without one, only the
  flanges with a face in the sketch's plane. Its copies in a pattern or a mirror cut each
  flange with a face in their plane, and one that lands off the sheet is left out with a
  warning.
- `unfold` lays bends of a sheet metal body flat, as SolidWorks' unfold: `fixed` (a
  flange's top or bottom face; left out, the largest flat face) stays where it is, and
  `bends` (a face of each bend; left out, every bend) open out flat, what lies beyond
  them turning into their parent's plane. The features after it are made on the part as it lies, until a `fold` (`bends`;
  left out, every bend laid flat) folds them back round, carrying what was cut while they
  were flat. A `sheet_cut` with an `end` between them cuts straight through the part as
  it lies; suppress the unfold and the fold and the same cut goes straight through the
  folded part, as SolidWorks' cut-extrude does there.
- `extrude` and `cut` take `end` `through_next`, SolidWorks' "up to next": a cut goes until
  it comes out of the first material in its way, an extrusion until it meets material.
- `linear_pattern`, `circular_pattern` and `mirror` copy edge flanges too: each copy is the
  same flange on the edge the copy's motion takes the original's edge to, set back from its
  ends the other way round for a mirror. They copy mitre flanges (the profile along the
  moved edges, in the other order for a mirror), tabs (the sketch moved) and break corners
  (the moved corner) the same way.
- A pattern or a mirror can be copied by another: the copy is of everything it has, its
  originals and their copies, each moved on, as SolidWorks copies a mirror of a mirror.
- `base_flange` with `tab: true` is a tab: its closed sketch, on a flat face of a sheet
  metal body or in its plane and touching the sheet, is added to that face's flange with
  the body's thickness, filling in what earlier cuts took.
- `break_corner` rounds (`kind` `fillet`, the default) or chamfers (`chamfer`) corners of a
  sheet metal body by `size`. `edges` are the corners: the edges through the sheet where
  two straight sides of a flat face meet (for a fillet, also a straight side and an arc,
  or two arcs: the round touches both). The body stays sheet metal. An inside corner,
  of the outline or of a cutout, gains the sheet an outside one loses. `faces` (flat faces)
  breaks every corner of each, found again at each rebuild, as SolidWorks' break corner on
  a face does.
- `corner` sets how flanges that run to one corner meet: `kind` (`butt`, `overlap`,
  `open`, `plain`), `gap` (for `open`, measured from each flange's inner face), and
  `relief`, what is cut from the blank where their bends meet
  (`rectangular` with `relief_size`, `tear`, `flush`: back to where the flanges end, or
  `slanted`: the face keeps its corner and each bend ends on a slant to its flange's end;
  `circular` and `square`, SolidWorks' corner relief: `relief_size` across, centred where
  the flanges' ends meet in the blank, cut from the face, the bends and the flanges).
  It applies to the corners of `faces`, or to every corner there is when it is added.
  Flanges on in-line edges of two faces that a bend joins (the lips round the side of a
  channel), each running to the bend, are mitred where they meet in one plane, as
  SolidWorks mitres them: half the `gap` (0.1 unless a `corner` sets it) each side of the
  line that halves the corner, with their bends ending on the mitre too. A `corner` on a
  mitre's face sets only that mitre's `gap`.
- `linear_pattern` becomes a grid with `direction2`, `spacing2`, `count2`; `"direction2":
  null` makes it a row again.
- `linear_pattern`'s `up_to` (`{"plane": ...}` or `{"point": ...}`) runs its first
  direction up to a reference, as SolidWorks' "Up to reference": the copies span the
  distance from the seed's centre (the middle of the box round its faces, or `up_to_seed`)
  to the reference, less `up_to_offset` (more with `up_to_reverse`). `up_to_keeps`
  `"spacing"` (default) gives as many copies as fit, one exactly at the end included;
  `"count"` spreads `count` copies evenly. `"up_to": null` sets both again.
- `count` for linear and circular part patterns, and a grid's `count2`, accept a number
  or an expression such as `"iif(width > 299mm, 3, 2)"` or `"int(width / 100mm)"`.
  Counts include the original and must evaluate to unitless whole numbers from 2 to
  10000; a grid has at most 10000 instances in total. Fractional or out-of-range counts
  fail at rebuild. Unit mismatches, unknown names and expression errors reject an input;
  errors caused by later parameter changes fail the pattern at rebuild. Use
  `set_suppression_expression` when no copies should be made. Counts can differ between
  configurations with `edit`'s `configurations`. A numeric count clears its expression.
- A reference that the application lets the user pick afterwards (a flange's or hem's
  `edge`, a draft's `neutral`, a sweep's `path`) can be `null`: the feature is added
  waiting for it. Leaving it out is an error.
- `revolve` takes `axis`: `"sketch_x"`, `"sketch_y"`, `{"line": id}` (a line of its sketch)
  or an axis selector. Left out, it is the sketch's first construction line, else
  `sketch_y`.
- `sweep` takes `profile` and `path` (two sketches). Corners between straight pieces of
  the path are mitred. A path with a spline in it is followed as a whole: it has no
  corners, the profile's plane only has to cross its start (not squarely), and the sides
  are freeform. So is a path that leaves the profile's plane at a slant: the profile is
  carried along it as drawn. A profile part way along a straight piece of the path, square
  to it, is swept both ways along the whole path.
- `loft` takes `profiles`: two or more sketches in order, each with the same number of
  edges (a circle adapts).
- `convert_to_sheet` turns a solid of one wall thickness into a sheet metal body. It takes
  `face` (the flat face that stays fixed; left out, the largest flat face of the only
  body), `bend`, `relief` and `relief_ratio`. `thickness` gives a flat body a sheet
  thickness of its own, from `face` into the solid (`null`: the solid's own).
- `hole` drills at every point of its sketch. `standard` (`{"size": "M6", "fit": "close"
  | "normal" | "loose" | "tapped"}`) sets every size; sizes given as well win.
- `fillet` and `chamfer` take `edges` and `size`; `shell` takes `open` (faces to remove)
  and `thickness`; `draft` takes `faces` (flat, round along
  the pull, or freeform), `neutral` (a plane) and `angle`.
- A pattern or a mirror copies a `draft` of faces of features it copies too: list the
  draft after them, and each copy drafts the same faces of their copies.
- `move_body` (SolidWorks' move/copy body) takes `bodies` (a face of each body; none:
  every body), turns them by `rx`, `ry`, `rz` degrees about the X, Y and Z axes through
  `center` (default the origin), in that order, then moves them by `dx`, `dy`, `dz`. A
  moved body keeps its faces' names. With `"copy": true` the bodies stay and `copies` moved
  copies are made, each moved once more than the last. A sheet metal body moved this way
  loses its flat pattern. `mates` places the bodies instead by SolidWorks' constraints, solved
  at every rebuild from where the distances and angles put them: each is `{"type":
  coincident | concentric | parallel | perpendicular | distance | angle, "moved": end, "to":
  end}`, with `distance` or `angle`, `aligned` (two flat faces the same way), `flip` (the
  distance or angle the other way) and `name`; an end is `{"face" | "edge" | "vertex":
  selector}` on a moved body as it was before the move, or on another body, or
  `{"plane": plane}`. A mate that can't hold is left out with a warning. A distance or angle
  mate's value is the move's dimension `D1@NAME` (`D1@Distance2`), field `mate1`, `mate2`, …
  by the mate's place: `set` changes it, per configuration as any feature's value.
- `structural_member` (SolidWorks' weldment structural member) sweeps `profile` (a
  sketch, drawn with its origin on the line) along every line of the `paths` sketches, a
  body per line, the profile turned square to each line by the smallest turn from its
  sketch plane and then by `rotate` degrees about it. Lines of the feature meeting end to
  end are mitred (`corner`: `miter` or `plain`); a line ending on a member of an earlier
  structural member is trimmed to its outside (`trim`, default true).
- `insert_part` (SolidWorks' insert part and mirror part) takes `file`, a `.peet` part,
  and inserts its bodies as they are built in it, or their mirror image in `mirror` (a
  plane). The bodies are read when `file` is set and kept: `update_links` reads the file
  again. A relative path is looked for from the working folder, then from the part's own.

| Operation | Fields |
|---|---|
| `edit` | `feature`, then any of its fields; `configurations` for its numeric fields |
| `rename` | `feature`, `name` |
| `set_suppression_expression` | `feature`, `value`: expression text, or `null` to clear. A part-wide rule evaluated with the active configuration's parameters; zero builds, finite nonzero suppresses |
| `suppress` | `feature`, `on` (default true), `configurations`. What is built on a suppressed feature is suppressed with it |
| `show` | `feature`, `on` (default true) |
| `show` | `datum` (`origin`, `front`, `top`, `right`, `planes`), `on`: the built-in reference geometry |
| `delete` | `feature` or `features` |
| `move` | `feature`, and `before`, `after` or `index` |
| `rollback` | `to` (a feature, `"end"` or `"start"`) |

### Parameters and history

| Operation | Fields |
|---|---|
| `set_parameter` | `name`, `value`, `configurations`. A new parameter exists in every configuration. A name is a word, or any text without `"` or `@` as SolidWorks names global variables (`Base Height`), quoted in expressions (`"Base Height" / 2`) |
| `delete_parameter` | `name` |
| `set_units` | `length` (`mm`, `cm`, `m`, `in`, `ft`) |
| `set_material` | `material` (a name, or `null` for none), `density` in kg/m³ (left out: the material tables' density for that material). What the part is made of: saved with it, and what `mass` weighs it with |
| `set_color` | `color` (`"#rrggbb"` or `[r, g, b]`, or `null` for the usual colour): what the part is drawn in |
| `undo`, `redo` | |

#A suppression expression overrides the saved manual flag while it is set. Clearing
it restores the per-configuration manual flags. `suppress` changes those saved flags;
`feature` returns `suppression_expression`, effective `suppressed` or `null` on an
invalid rule, and `manual_suppressed`. `features` reports the effective rebuild status.
`suppressed_in` lists saved manual flags. Syntax errors reject the operation;
evaluation errors make the feature fail at rebuild and appear in `failures`.
Rules read parameters and dimensions by address (`"Height@Sketch1" > 1500`).
SolidWorks suppression strings must be translated to 1 for suppressed and 0 for
unsuppressed by the importer.

```jsonl
{"op": "set_suppression_expression", "feature": "Vent", "value": "iif(width > 300mm, 0, 1)"}
{"op": "set_suppression_expression", "feature": "Vent", "value": null}
```

## Configurations

A part has one or more configurations: versions of it that differ in which features are
suppressed and in its numbers: parameters, the numeric values of features and the
dimensions of sketches. One is active: it is what every other operation reads, builds
and exports. An assembly has configurations too, made with the same operations: see
[Configurations of an assembly](#configurations-of-an-assembly).

| Operation | Fields |
|---|---|
| `add_configuration` | `name`, `copy` (a configuration; default the active one), `comment`, `parent` (a configuration to derive it from, in place of `copy`). It becomes the active one |
| `edit_configuration` | `configuration`, `name`, `comment` |
| `delete_configuration` | `configuration`, with the configurations derived from it. A part or an assembly keeps at least one |
| `configuration` | `configuration`: make it the active one. Not an undo step |

`suppress`, `set_parameter`, `edit`, `set_dimension` and `set` take `configurations`: `"this"`
(the active one), `"all"`, a configuration's name, or a list of names. Left out,
`suppress` and `set_parameter` mean `"this"`; `edit`, `set_dimension` and `set` change a value
that already differs between configurations in the active one, and a value that doesn't
in all of them. Only the numeric fields of a feature can differ: an `edit` with
`configurations` other than `"all"` that changes another field is refused.

A **derived configuration** (`add_configuration` with `parent`, SolidWorks' derived
configuration) starts as a copy of its parent and follows it: a change made to the parent
is made to every configuration derived from it (at any depth) that has the parent's value
there; a value of its own (one that differs from the parent's) stays. In an assembly this
holds per field of a component, per mate and per equation. Deleting a parent deletes what
is derived from it. `configurations` gives each derived one's `parent`.

```json
{"op": "add_configuration", "name": "Thick"}
{"op": "set_parameter", "name": "thickness", "value": "2mm"}
{"op": "suppress", "feature": "Sheet-Cut1", "configurations": ["Thick", "Blank"]}
{"op": "edit", "feature": "Extrude1", "depth": 12, "configurations": "this"}
{"op": "set_dimension", "sketch": "Sketch1", "name": "width", "value": 120, "configurations": "Thick"}
{"op": "configuration", "configuration": "Default"}
```

### Queries

| Operation | Gives |
|---|---|
| `status` | name, file, units, material, colour, counts, failures, undo and redo labels |
| `features` | the tree, each feature's status and what it uses |
| `feature` (`feature`) | its fields; a sketch's plane, definition, entities, relations and dimensions |
| `parameters` | named values and units; a parameter that differs between configurations lists its expression in each |
| `dimensions` (`feature`, `name`) | every dimension's address `NAME@FEATURE` and value: see [Dimensions by name](#dimensions-by-name) |
| `configurations` | the configurations (a derived one with its `parent`), the active one, and what differs: the configurations each feature is suppressed in, each parameter's expression in every configuration, and under `values` each feature value and sketch dimension in every configuration. In an assembly also `components` (for each component that differs, in every configuration, the fields that differ: `suppressed`, `configuration`, `fixed`, `placement`), `mates` (for each mate that differs: `suppressed`, and its values) and `equations` (for each equation that differs, by what it sets, its expression in every configuration; null where there is none) |
| `bodies` | bounds, volume, counts; flat size and thickness for sheet metal |
| `faces`, `edges` (`body`) | what made each face, normals, centres, a round face's `radius`, `axis` and a point on it (`axis_point`); edge ends and the faces they join, a straight edge's `direction` (the one mates take) |
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
| `import_step` | `path`. In a part: the file's solids become bodies, in one feature named after the file (a closed shell of a surface model is a solid too; open shells are skipped with a warning). In an assembly: the file's parts and assemblies become parts and sub-assemblies, placed as components where the file has them (fixed); replies `components`, `parts`, `bodies`. An edge the file writes off its faces (a tolerant edge, as SolidWorks writes them) is moved onto them where they meet, with a warning; a body whose vertices are off where their faces meet (by up to 1 mm) is repaired, with a warning: vertices moved to where the faces meet, or split, with new faces closing the gaps where they don't. A body that still isn't a valid solid is left out, with a warning, and the rest imported; if none can be, the import fails |
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
| `insert` | the part: `path` (a `.peet` file; an assembly becomes a sub-assembly), `sample`, `part` (an open document, by name or id) or `component` (another instance of its part). `name`, `at` (`[x, y, z]`: where the part's origin goes), `rotate` (`{"axis": "x"`, `"y"`, `"z"` or `[x, y, z]`, `"angle": degrees}`, from the part's own orientation), `fixed` (default: only the first component), `configuration` (the configuration of the part it shows; default the part's active one, or with `component` the configuration that component shows) |
| `insert` with `"link": true` | with `path`: the component follows the part's file instead of the assembly keeping a copy. The link is relative to the assembly's folder unless `"absolute": true` |
| `update_links` | reads every linked part from its file again, and in a part every inserted part (`insert_part`). Replies `updated` (the parts or features that changed) and `warnings` (files that can't be found or read) |
| `link` | `part` (a part's name or id, as `components` lists the parts), `path`, `absolute` (default false): links the part to the file if there is one (the part becomes what is in it), else writes the part there first |
| `unlink` | `part`: the assembly keeps the part as it is and no longer follows the file |
| `place` | `component`, `at`, `rotate`: the ones given are set, the other is kept. `component` can be a path (`["Hinge", "Arm"]`) to a component that moves in a flexible sub-assembly: it is placed there, in this assembly's coordinates. `configurations` (of the assembly; default the active one) |
| `drag` | `component`, `to` (`[x, y, z]`, in the assembly), `point` (`[x, y, z]` on the component, in its own coordinates; default its origin): pulls the point towards the place as far as the component's mates allow. `short_by` in the reply is how far it still is |
| `fix` | `component`, `on` (default true), `configurations` (of the assembly; default the active one) |
| `component_configuration` | `component`, `configuration` (a configuration of its part, or `null` for whichever the part has active), `configurations` (of the assembly; default the active one): the configuration of its part the component shows, SolidWorks' referenced configuration (FabSys's `SetSubComponentConfig`). Two components of one part can show different configurations; each configuration is built once (model schema 12) |
| `flexible` | `component` (a sub-assembly), `on` (default true): solves the sub-assembly as flexible (SolidWorks' "Solve as: Flexible"): its own mates are solved in this assembly with this one's, so what is not fixed in it moves as they let it, each instance posed on its own. `components` then gives `"flexible": true` and `moving` (each moving component's `path`, `at`, `rotate`, `freedom`). Off: rigid, as its file has it (model schema 12) |
| `replace` | `component`, and `path`, `sample` or `part`: another part, in the same place |
| `rename`, `suppress`, `show`, `delete` | `component` in place of `feature`. `suppress` takes `configurations` (of the assembly; default the active one) |
| `component_pattern` | `components` (the originals), `type` (`linear`, `circular`), `name`. Linear: `direction`, `spacing`, `count` (the original included; a whole number or an expression, worked out at every rebuild: copies are made and removed as it changes), `flip`, `second` (`{direction, spacing, count, flip}`). Circular: `axis`, `angle` (default 360), `count`, `flip`. A direction or axis is `"x"`, `"y"`, `"z"`, `[x, y, z]`, `{"origin": …, "direction": …}`, or `{"component": …, "edge" or "face": selector}` |
| `edit_component_pattern` | `pattern`, and any of `count`, `spacing`, `angle`, `flip`, `direction`, `axis`, `second` (`null` removes it) |
| `set` on a pattern's value | `name` by SolidWorks' names for a component pattern's dimensions: a linear pattern's counts `D1@PATTERN` and `D2@PATTERN`, spacings `D3@PATTERN` and `D4@PATTERN`; a circular pattern's count `D1@PATTERN` and angle `D3@PATTERN`. `value` a number or an expression (`IIF("Height@Base@Gear tray<1>.Part" > 1100, 3, 2)`). Expressions read them by the same addresses |
| `assembly_cut` | `on` (`"top"`, `"front"`, `"right"`: the assembly's plane; `{"component", "face" or "plane"}`: a flat face or plane of a component, which the sketch follows; or `{"origin", "normal", "x"}`), `draw` (as for `sketch`, in the plane's coordinates), `convert` (list of `{"component", "edge"}`: components' edges converted into the sketch, which follow them), `end` (`blind`, `mid_plane`, `through_all`; default `blind`), `depth` (default 10), `flip` (along the plane's normal instead of against it), `flip_side`, `components` (the scope; left out: every component it reaches), `name` (default `Cut-Extrude1`), `sketch_name` (default `Sketch1`). SolidWorks' assembly cut: the components are cut in the assembly only, their parts are not changed. The reply's `cut` has `status` (`warning` when it cuts nothing), `converted` (each with its sketch `entity`) and `plane` |
| `edit_assembly_cut` | `cut`, and any of `on`, `draw` (added), `convert` (added), `end`, `depth`, `flip`, `flip_side`, `components` (`null`: every one it reaches), `suppressed`. `rename` and `delete` take `cut`; `components` lists the cuts under `cuts` |
| `rename`, `delete` | `pattern` in place of `feature` |
| `set_color` with `component` | `color` (`"#rrggbb"`, `[r, g, b]`, or `null` for its part's): a colour for that component alone |
| `show_all` | shows every hidden component; replies `shown` |
| `isolate` | `components` (names or ids): shows these and hides every other; replies `hidden` |
| `explode_step` | `components`, `by` (`[x, y, z]`: how far, along the assembly's axes), `name`: adds a step to the exploded view |
| `edit_explode_step` | `explode_step`, and `by` or `components` |
| `rename`, `delete` | `explode_step` in place of `feature` |
| `explode` | `on` (left out: the other way): shows the assembly exploded, or as it is. A view, not an undo step |
| `explode_steps` | every step (`id`, `name`, `components`, `by`), whether the assembly is shown `exploded`, and under `moved` each component the steps move with its total `by` and where it is drawn exploded (`at`) |
| `open_component` | `component`: opens its part as a document of its own, in the configuration the component shows (the reply's `configuration`), and makes it current. `save` without a `path` on that document stores it back, as one undo step of the assembly, for every component of the part, with the part's own active configuration as it was. Needs a session |
| `components` | every component (`id`, `name`, `part`, `configuration` (of its part, where the part has several), `at`, `rotate`, `fixed`, `freedom`, `mates`, `status`, `message`, `min`, `max`) and every part (`id`, `name`, `kind`, how many `components`, `material`, and where it has several `configurations` and its `active_configuration`); how many `mates`, and the `freedom` left |
| `mate` | `type` (`coincident`, `concentric`, `parallel`, `distance`, `angle`, `fasten`, `width`, `slot`, `limit_angle`, `limit_distance`, `symmetric`), `a`, `b`, `distance` or `angle` for those types, `flip` (`true`: aligned), `align` (`aligned`, `anti_aligned` or `closest`: which way round the ends go, SolidWorks' alignment; not with `flip`), `flip_dimension` (the distance or angle measured the other way: SolidWorks' "flip dimension"), `suppressed` (added without being solved: nothing moves until it is unsuppressed), `name`. An end is `{"component": …, "face": selector}` (or `"edge"`, `"vertex"`, `"plane"`, `"origin": true`, or `"sketch"` (name or id) with `"entity"` (the id `feature` gives a point, line, circle or arc)), the selector in the part's own coordinates. `component` is a name, id or nonempty path array from the top-level component down through sub-assemblies. `plane` takes `"top"`, `"front"`, `"right"`, a reference plane or coordinate system by name or id, or a planar face selector. With no `component`, the end is the assembly's own: `{"plane": "top"}` (or `front`, `right`) or `{"origin": true}`, fixed where the assembly is. For `fasten` the component alone |
| `mate`, `type: "slot"` | `a`, `b`: a face of a straight slot (one of its round ends or a flat side) and an axis (a round face or edge), either way round. The axis stays parallel to the slot's ends and on the line between their axes. `mode`: `free` by default (anywhere between the ends), `centered`, `dimension` with `distance` (from the first end's axis: the round end picked, else the end the part lists first), `percent` with `percent`; `flip_dimension` measures from the other end. No `flip` or `align` |
| `mate`, `type: "limit_angle"` / `"limit_distance"` | `a`, `b`, `min`, `max` (degrees, or lengths): the angle or distance, measured as `angle` and `distance` measure it (`flip_dimension` too), kept anywhere between them |
| `mate`, `type: "symmetric"` | `a`, `b`: two planes, points or lines, mirror images in `plane` (an end: a flat face or a reference plane, or the assembly's own). The plane is on one of the two components, on a fixed one, or the assembly's; `a` and `b` may be on one component (both faces of a part, centred on the plane) |
| `mate`, `type: "width"` | `a`: two planar ends, the width pair. `b`: two planar ends, the tab pair, or one cylindrical face / edge used as an axis. `mode`: `centered` by default, `free`, `dimension` with `distance`, or `percent` with `percent`. Values accept numbers or expressions. A pair on one top-level component is parallel and distinct; a pair may instead be on two components (each face is then placed by its own place along the first width face's normal, and a tab face kept parallel to it). Centred, the tab may be wider than the width pair (their midplanes coincide); the other modes need it to fit. No `flip`, `align` or `flip_dimension`. `name`, `suppressed` |
| `edit_mate` for width | `mate`, `mode`, and `distance` for Dimension or `percent` for Percent. `distance` alone edits a Dimension mate; `percent` alone selects/edits Percent. Rename, suppress and delete use the ordinary mate operations |
| `edit_mate` | `mate`, and `distance` or `angle`, `flip` or `align`, `flip_dimension`; a slot mate's `mode`, `distance`, `percent` as for width; a limit mate's `min` and `max`. `configurations` (of the assembly) for the value, mode and limits: left out, the active one if they already differ between configurations, else all |
| `set` | `name` `D1@MATE` (SolidWorks' name for a mate's value, `D1@Distance6`), `value`, `configurations` (as for `edit_mate`): a distance or angle mate's value; or an assembly parameter by name |
| `set` with a component | `name`: an address that reaches into a component, `NAME@FEATURE@COMPONENT` (`Height@Sketch1@Door-1`; SolidWorks' `Height@Sketch1@Door<1>.Part` and `Door-1.Part` too), a parameter of its part `NAME@COMPONENT`, through a sub-assembly `…@Door-1@Door Assembly-1`; `value`. A number sets the part, so every instance of it (a linked part's file is written: the reply's `written`); an expression is an equation of the assembly, worked out at every rebuild. A value replaces an equation on it. `configurations` (of the assembly; left out, all of them while the equation is the same everywhere, else the active one): the equation is theirs, a number too; `value` `null` removes it there |
| `set_suppression_expression` with `component` | `value`: an expression of the assembly (nonzero suppresses, 0 unsuppresses: SolidWorks' `suppressed` and `unsuppressed`), or `null` to clear. It decides over `suppress` while it is set. `configurations` as for `set` with a component |
| `suppress` with a component's feature | `feature`: `FEATURE@COMPONENT` (`Vent@Door-1`), `on`: suppresses the feature in the component's part (every instance; a linked part's file is written) |
| `reference_dimension` | `a`, `b` (ends as a mate's: flat faces, planes, edges, vertices), `name` (default `RD1`, `RD2`, …): the distance between them, measured after the mates are solved, which expressions read as `RD1@Annotations`. The reply's `reference` has its `value`. `delete` takes `reference_dimension` |
| `make_independent` | `component`, `name`: the component gets a copy of its part, called `name`, and is renamed `name-1` (SolidWorks' Make Independent, FabSys's `ResolveComponent`). A linked part's copy is written beside its file as `name.peet` |
| `dimensions` on an assembly | the assembly's own values (mates' `D1@MATE`, reference dimensions' `RD1@Annotations`), its `equations` (`target`, `expression`, `suppresses`, `status`, `message`; those of the active configuration, then those only others have with `expression` null; `configurations` with each one's expression in every configuration where it differs) and `references`; with `component`, that component's part's dimensions and parameters by their address from the assembly (`equation`: set by one); with `name`, the value at any address |
| `rename`, `suppress`, `delete` | `mate` in place of `feature`. `suppress` takes `configurations` (of the assembly; default the active one) |
| `mates` | every mate (`id`, `name`, `type`, `a`, `b`, `distance` or `angle`, `flip`, `align` if one was given, `flip_dimension`, `status`, `message`) and the `freedom` left |

| `interference` | `component` (left out: every pair): the pairs of components that share space, each with `a`, `b`, `volume_mm3`, `min`, `max`; `clear`; how many pairs were `compared`; `unchecked` pairs that couldn't be |
| `bom` | `level` (`parts`, the default: every part, through sub-assemblies; `top`: a sub-assembly is one line): `rows`, each with `item`, `part`, `configuration` (where the part has several: a part in two configurations is two rows), `quantity`, `material`, `volume_mm3`, `mass_kg`, `total_mass_kg`, the `components`, and for sheet metal `thickness`, `flat_size`, `bends`; the total `quantity` and `mass_kg`, or `without_mass` |
| `mass` | in an assembly: each component's `volume_mm3`, `mass_kg` and `center_of_gravity`, and the `total` (with `center_of_gravity_of`: `mass` if every part has a material, else `volume`; `principal_moments_kg_mm2` or `principal_moments_mm5`); `without_material` |
| `export` to a `.csv` | the bill of materials (every part) as a table: `item`, `part` (with its configuration in brackets where it has several: `Door (R Hinges)`), `quantity`, `material`, `mass_kg`, `total_mass_kg`, `thickness_`, `flat_width_`, `flat_height_` (with the document's unit), `bends`, `components` |

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

**Mates.** A flat face or a reference plane stands for its plane, a round face or a round edge for its axis
(but a round edge coincident with a plane lies in the plane, as in SolidWorks), a straight edge for a line
(its direction the `direction` `edges` gives), a vertex or an origin for a point, and a sketch's point, line,
circle or arc for a point, a line or a round edge. Two planes are put against each other
unless `flip` is true (`align: "aligned"`); `align: "closest"` keeps them whichever way they
are nearer. Two axes go whichever way is nearer unless `align` is `aligned` (pointing alike)
or `anti_aligned`; an axis's direction is the one `faces` and `edges` give. A distance from a
plane is measured along its outward normal (the first end's, if it is a plane), behind it
with `flip_dimension`. A mate added `suppressed` is solved when it is unsuppressed. The mates are solved whenever the assembly is rebuilt, and the
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

Reference-plane ends store persistent references and resolve the plane again at each
rebuild, including planes in definitions with no bodies. Nested ends use paths such as
`{"component": ["Stand", "Plate"], "plane": "MatePlane"}` or
`{"component": ["Stand", "Plate"], "face": {"name": "TopFace"}}`.
Names and ids can be mixed in a path; each belongs to the assembly at that level.
The mate moves the top-level component as a rigid whole and cannot join two ends
within the same top-level component. `fasten` holds those top-level components where
they are. A missing or suppressed nested plane/component makes its mate fail until
restored. `mates` reports nested ends' full `path` and plane ends' current `plane` name.
Component pattern directions also accept plane normals and nested paths through the
same end selectors.

**Width mates** align the tab pair's midplane with the width pair's midplane. The
available clearance is the width span minus the tab span; for a cylindrical tab the
span is its diameter, and an edge selected as an axis has zero thickness. Centered
divides the clearance equally. Free lets the tab slide within it, including during
`place` and `drag`. Dimension sets the nearest surface gap from the first width end;
Percent sets its share of the available clearance, from 0 at the first width end to
100 at the second. Pair ordering therefore selects the side used by Dimension and
Percent, without changing centering. Values follow assembly parameters. Parallel
planar pairs are supported; drafted/nonparallel and coincident pairs fail explicitly.
A tab wider than the gap or a value beyond the available clearance also fails and
recovers when corrected. Geometry edits recompute midpoint and clearance, and all
ends remain persistent references. Query replies give `a` and paired `b` as arrays,
plus `mode`, and `distance` or `percent` when applicable. A single-axis `b` stays an
object. Width mates keep the nearest orientation. Their properties show the pairs
and allow mode/value editing.

**Equations and reaching into components**, as SolidWorks' assembly equations and FabSys
do. An expression of the assembly (a parameter, a mate's value, an equation) reads a
component's dimension as `"Height@Sketch1@Door-1"` and a parameter of its part as
`"Thickness@Door-1"`. A component's part is shared by all its instances, so a value set
in it from the assembly reaches every instance, as SolidWorks changes the part's document
(use `make_independent` first to change one instance alone). Equations are worked out at
every rebuild, before the mates are solved: those that read what others set are worked
out again, and if one reads a reference dimension, the parts it changed are rebuilt and
the mates solved again until the values hold. An equation that can't be worked out is
not a failed operation: it is listed under `failures` (`equation`, `expression`,
`message`). An assembly's equations live in the assembly: the part keeps the value they
last set, and a linked part's file is not written by them.

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

#### Configurations of an assembly

An assembly has configurations as a part has (`add_configuration`, `edit_configuration`,
`delete_configuration`, `configuration`, `configurations`, and parameters that differ with
`set_parameter`'s `configurations`). What differs between them is what SolidWorks lets
differ in an assembly's configurations: for each component whether it is suppressed, the
configuration of its part it shows, whether it is fixed, and where it is; for each mate
whether it is suppressed and its values. `suppress`, `fix`, `place` and
`component_configuration` with a component, and `suppress` with a mate, change the active
configuration unless `configurations` says otherwise; a mate's value (`edit_mate`, `set`
`D1@MATE`) changes as a part's value does. Where a component is belongs to the
configuration: a solve that moves it, or `place`, moves it in the active one only, so each
configuration keeps its own placements (left and right hinged). A new component or mate is
in every configuration. A configuration made from another (`copy`) starts as it is.
An equation (`set` with a component's address, `set_suppression_expression` with a
component) can differ too, as SolidWorks' "This configuration": both take `configurations`
(left out: all of them while the equation is the same everywhere, the active one once it
differs), a number for some configurations is an equation of theirs, and `"value": null`
removes the equation there. A suppression equation of some configurations suppresses in the
active one only. `open_component` opens the part in the configuration the component shows;
`save` there stores it back with the part's own active configuration as it was. Reference
dimensions and component patterns are the same in every configuration. The model schema
is 12.

```json
{"op": "add_configuration", "name": "Right Hinged"}
{"op": "component_configuration", "component": "Door-1", "configuration": "R Hinges"}
{"op": "suppress", "component": "Left Hinge-1"}
{"op": "suppress", "mate": "Concentric4", "on": false}
{"op": "set", "name": "D1@Distance1", "value": 40, "configurations": "this"}
{"op": "configuration", "configuration": "Left Hinged"}
```

`configuration` with a name the assembly doesn't have fails and lists the names. SolidWorks
ignores a missing name silently; PeetCAD reports it, on purpose, so that a script asking for
a configuration that isn't there is fixed rather than going on with the wrong one.

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
| `window` | `open` (`command_palette`, `settings`, `keyboard_shortcuts`, `about`, `parameters`, `configurations`, `bend_table`, `checks`, `materials`, `mass`) |
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
