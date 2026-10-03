---
name: peet
description: Build, edit, inspect and export PeetCAD parts (.peet files) with the `peet` command line. Read before running peet. More with `peet skills sketching`, `selectors`, `solids`, `sheet-metal`.
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

## More, when the task reaches it

- **Drawing or changing a sketch** (shapes, relations, dimensions, entity labels and
  ids, which way the sketch axes point): `peet skills sketching`.
- **Pointing at existing geometry** (a sketch on a face, the edge for a flange or a
  fillet, anything taking a plane, face, edge or vertex): `peet skills selectors`.
- **Solid features** (extrude and cut directions, revolve, sweep, loft, holes, fillets,
  shell, patterns, freeform faces, bodies from a STEP file): `peet skills solids`.
- **Sheet metal** (base flange, flanges, cuts, converting a solid to sheet metal, flat
  pattern, DXF, materials, checks): `peet skills sheet-metal`.
