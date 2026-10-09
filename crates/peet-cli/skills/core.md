---
name: peet
description: Build, edit, inspect and export PeetCAD parts and assemblies (.peet files) with the `peet` command line. Read before running peet. More with `peet skills sketching`, `selectors`, `solids`, `sheet-metal`, `assemblies`, `configurations`, `live`.
---

# peet

`peet` applies **operations** to a part file and prints one JSON **reply** per operation.
An operation is a name and fields: `{"op": "extrude", "sketch": "Sketch1", "depth": 8}`.

## The loop

1. **Look.** `peet status -f PART.peet`, then `peet features -f PART.peet` for the tree.
   A new part starts with `--new`.
2. **Find the operation.** `peet ops` lists every operation; `peet ops NAME` gives its
   fields and what each takes. That list is the reference: read it, don't guess fields.
3. **Apply.** Operations that belong together go in one script, so they are saved together
   or not at all:

   ```sh
   peet run build.jsonl --new -f plate.peet    # a script: one operation per line
   peet edit feature=Extrude1 depth=12 -f plate.peet    # one operation, by name
   ```

4. **Read every reply.**
   - `"ok": false`: the operation was not applied and `error` says why, usually with the
     valid choices. The run stops there and nothing is saved. Fix that operation and run
     the whole script again.
   - `created[].status` and `feature.status`: `ok`, `warning` or `failed`, with a
     `message`. A feature that can't be built is still added and saved.
   - `failures`: every feature that currently can't be built. Fix each with `edit`, or
     remove it with `delete`.
5. **Check the geometry.** `peet bodies -f PART.peet` gives each body's size and volume;
   `measure` gives exact sizes and distances. Compare them with what was asked for.

Done when `status` shows `"failures": []` and `bodies` has the sizes that were asked for.

## A complete example

```jsonl
{"op": "set_parameter", "name": "t", "value": "8mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 80, "name": "width"}, {"type": "length", "of": ["r.right"], "value": 50}]}
{"op": "extrude", "sketch": "Sketch1", "depth": "t"}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Holes", "draw": [{"type": "circle", "center": [15, 25], "radius": 4}, {"type": "circle", "center": [65, 25], "radius": 4}]}
{"op": "cut", "sketch": "Holes", "end": "through_all"}
{"op": "bodies"}
```

## What the replies and `peet ops` don't say

- **A part that is open in PeetCAD is changed there**, on screen, and not saved: `peet`
  says "Applying to PeetCAD" on standard error when it does. Read `peet skills live`
  before going on.
- **Each `peet` call is separate.** It opens the file, applies, and saves if the part
  changed and every operation was applied. `undo` only reaches back within one call: to
  take back an earlier call, `edit` or `delete`.
- **Without `-f`** the part exists only for that call. Use it to try a script, or have
  the script `export`.
- **Values.** A number is in the part's units (mm unless `set_units` changed them), or
  degrees. Text is an expression: `"25mm"`, `"1in"`, `"2 * t"`. Prefer parameters
  (`set_parameter`) for sizes that may change: features follow them.
- **Names.** Features are named as they are added (`Sketch1`, `Extrude1`, `Cut-Extrude1`,
  `Edge-Flange1`); the reply's `created` gives the name. Pass `name` to choose one, and
  refer to a feature by name.
- **`field=value` on the command line** is JSON if it reads as JSON (`depth=8`,
  `flip=true`) and text otherwise (`depth=2mm`). Operations with nested JSON (`draw`
  lists, selectors) go in a script file or standard input (`peet run - -f PART.peet`),
  where no shell quoting applies.
- **`--strict`** makes a part that ends with features that can't be built exit 1 and
  not save. Use it when a broken part must not be written.
- **Exit code:** 0 applied, 1 an operation was not applied, 2 the command line or a file
  was wrong.

## Conditional sizes

Use `if(condition, yes, no)` or `iif` when a size changes at a threshold. Comparisons
`<`, `<=`, `>`, `>=`, `==` or `=`, and `!=` or `<>` return 1 for true and 0 for false.
Zero chooses `no`; any nonzero plain number chooses `yes`. Only the selected branch
is evaluated, so it can guard a division by zero. Names in both branches must exist,
and dependencies in both branches must be free of cycles.

Compare like units. A bare number adopts the other operand's kind in document units;
write `300mm` to keep a threshold fixed when the document units change. The selected
branch must suit the consuming field. `int` rounds down like `floor`, including for
negative numbers. Part pattern counts accept expressions too; read `peet skills solids`
for their limits and an example.

```jsonl
{"op": "set_parameter", "name": "width", "value": "300mm"}
{"op": "set_parameter", "name": "depth", "value": "iif(width > 299mm, 8mm, 4mm)"}
{"op": "set_parameter", "name": "bands", "value": "int(width / 100mm)"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 10]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": "depth"}
{"op": "set_parameter", "name": "width", "value": "200mm"}
{"op": "bodies"}
```

For features that appear only at certain sizes, read `peet skills configurations`
for `set_suppression_expression`.

## Several documents in one run

Use this to read from one part while building another, or to make several parts in one
script. Otherwise one `peet` call per part is simpler.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "open_sample", "sample": "bracket", "keep": true}
{"op": "documents"}
{"op": "bodies", "document": 1}
{"op": "close"}
```

- `"keep": true` on `new`, `open` or `open_sample` opens a document **beside** the one
  that is open and makes it current. Without it they replace the current document.
- Operations go to the current document. `"document": NAME_OR_ID` on any operation
  sends it to another without leaving the current one; `switch` changes which is
  current. `documents` lists them with their ids.
- The run saves only the part of `-f`. Save any other with
  `{"op": "save", "path": ..., "document": ...}`, or its changes are lost (the run says
  so on standard error).
- `close` refuses a document with unsaved changes: save it, or add `"discard": true`.

## More, when the task reaches it

- **Drawing or changing a sketch** (shapes, relations, dimensions, entity labels and
  ids, which way the sketch axes point): `peet skills sketching`.
- **Pointing at existing geometry** (a sketch on a face, the edge for a flange or a
  fillet, anything taking a plane, face, edge or vertex): `peet skills selectors`.
- **Solid features** (extrude and cut directions, revolve, sweep, loft, holes, fillets,
  shell, patterns, freeform faces, bodies from a STEP file): `peet skills solids`.
- **Sheet metal** (base flange, flanges, cuts, converting a solid to sheet metal, flat
  pattern, DXF, materials, checks): `peet skills sheet-metal`.
- **Putting parts together** (an assembly: inserting parts as components, placing them,
  changing a part of an assembly): `peet skills assemblies`.
- **Several versions of one part in one file** (sizes, with and without features;
  `status` shows a `configuration` other than `Default`, or `suppress` and
  `set_parameter` should apply to some versions only): `peet skills configurations`.
- **A part the user has open in PeetCAD**, or showing them something (turning the view,
  opening a window): `peet skills live`.
