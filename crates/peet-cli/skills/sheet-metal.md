---
name: sheet-metal
description: Sheet metal in peet - base flange, edge flanges, hems, cuts, forms, converting a solid or an imported body to sheet metal, the flat pattern and its DXF, bend table, materials and gauges, manufacturing checks.
---

# Sheet metal

A sheet metal body starts with a `base_flange` and is then bent and cut by sheet metal
features. It always has an exact **flat pattern**, which is what the DXF export writes.

```jsonl
{"op": "set_parameter", "name": "t", "value": "1.5mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": "t", "radius": 2}
{"op": "edge_flange", "length": 25, "offset_start": 10, "offset_end": 10, "edges": [{"between": [[0, 0, 1.5], [200, 0, 1.5]]}, {"between": [[200, 0, 1.5], [200, 150, 1.5]]}]}
{"op": "sketch", "on": {"feature": "Base-Flange1", "side": "top", "at": [100, 75, 1.5]}, "name": "Holes", "draw": [{"type": "circle", "center": [30, 30], "radius": 2.5}, {"type": "circle", "center": [170, 30], "radius": 2.5}]}
{"op": "sheet_cut", "sketch": "Holes"}
{"op": "bend_table"}
{"op": "checks"}
```

## Base flange

- From a **closed** sketch: a flat plate, `thickness` thick, on the sketch plane.
- From an **open** chain of connected lines: a bent profile, with a bend at every
  corner, extruded by `depth`.
- `thickness`, `radius` (the default inner bend radius) and `bend` (`{"k_factor":
  0.44}`, or an `allowance` or `deduction`) apply to the whole body. Defaults: 1.5 mm,
  1.5 mm, K 0.44. Or take them from the material tables: `peet materials`, then
  `{"op": "apply_material", "material": ..., "gauge": ...}`.
- `relief` and `relief_ratio` are also set here, for the whole body. A **relief** is the
  small cut at each end of a bend that stops short of the end of its edge, so the sheet
  doesn't tear. `relief`: `rectangular` (the default), `obround` or `tear` (no cut: the
  sheet is slit). `relief_ratio`: its width, and how far it reaches past the bend, as a
  multiple of the thickness (default 0.5).

## From a solid

`convert_to_sheet` turns a solid that is already shaped like sheet metal (one wall
thickness everywhere, flat walls, rounded bends) into a sheet metal body, so it gets a
flat pattern and takes flanges. Use it on a shelled solid or on a body from
`import_step`.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 2}
{"op": "convert_to_sheet", "face": {"normal": [0, 0, 1]}, "bend": {"k_factor": 0.4}}
{"op": "edge_flange", "length": 15, "edge": {"between": [[0, 0, 2], [60, 0, 2]]}}
{"op": "bend_table"}
```

- `face` is the flat face that stays fixed in the flat pattern. Left out, it is the
  largest flat face of the only body.
- The thickness and the bend radii are read from the solid; `bend` (K-factor, allowance
  or deduction) says how its bends unfold.
- A solid that is not one thickness, or whose bends are sharp corners, fails to build
  and says what it found: fix the solid, or model the part with `base_flange` and
  flanges.

## Flanges and hems

A tray: four walls, a hem on the front wall's free edge, a slot through the back wall
and a louver in the base.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [180, 120]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 1.5, "radius": 2}
{"op": "edge_flange", "name": "Wall", "length": 30, "offset_start": 5, "offset_end": 5, "edges": [{"between": [[0, 0, 1.5], [180, 0, 1.5]]}, {"between": [[180, 0, 1.5], [180, 120, 1.5]]}, {"between": [[180, 120, 1.5], [0, 120, 1.5]]}, {"between": [[0, 120, 1.5], [0, 0, 1.5]]}]}
{"op": "hem", "length": 8, "edge": {"between": [[5, 1.5, 30], [175, 1.5, 30]]}}
{"op": "sketch", "on": {"feature": "Wall3", "side": "top"}, "name": "Slot", "draw": [{"type": "rectangle", "from": [80, 12], "to": [100, 20]}]}
{"op": "sheet_cut", "sketch": "Slot"}
{"op": "sketch", "on": {"feature": "Base-Flange1", "side": "top"}, "name": "Vent", "draw": [{"type": "rectangle", "from": [120, 45], "to": [126, 75]}]}
{"op": "louver", "sketch": "Vent", "height": 3}
{"op": "bend_table"}
{"op": "checks"}
```

The slot's sketch is on the back wall's inside face, whose plane has x along the model's
X and y along its Z, so `[80, 12]` to `[100, 20]` is x 80 to 100, z 12 to 20.

- `edge_flange` and `hem` go on an edge **of a flat face of the sheet**: the long edges
  where a flat face meets the sheet's cut side, not the short edges across the
  thickness. On a plate on `top` of thickness 1.5, the top face's edges are at z = 1.5
  and the bottom's at z = 0.
- **Which way it bends:** towards the side of the face whose edge you gave. An edge of
  the plate's top face bends the flange up, one of its bottom face bends it down. `flip`
  turns it the other way.
- **A wall's faces:** each flange has a `top` and a `bottom` face of its own. Its `top`
  continues the top of the face it was bent from, so on walls bent up from the plate's
  top face, `top` is the face that looks **into** the tray and `bottom` the one that
  looks out. A hem on an edge of a wall's `top` face folds to the inside; on its
  `bottom` face, to the outside.
- **A wall's free edge** is at the wall's height: for a wall on the y = 0 edge, 30 high,
  set back 5, its inside top edge runs between `[5, 1.5, 30]` and `[175, 1.5, 30]`. Use
  `peet edges` if in doubt.
- **A sketch on a wall:** its plane is the wall's, so read `plane` in the reply (or
  `peet feature feature=SKETCH`) to see where its origin is and which way x and y point
  before drawing. Add the sketch empty first, look, then `draw`.
- `edges` (a list) makes one flange or hem per edge in one operation. With a `name` they
  are numbered: `Wall1`, `Wall2`, …
- `length` is measured on the **outside**, from the outer corner to the flange's end.
  `position` says where the bend sits relative to the edge (`material_inside`,
  `material_outside`, `bend_outside`).
- `offset_start` and `offset_end` set a flange back from the two ends of its edge, with
  a relief at each end: a wall on a 180 edge with both offsets 5 has a bend 170 long
  (`bend_table` gives each bend's `length`). Where two flanges are set back a little
  from the same corner, their reliefs cut the corner of the sheet off: the corner is
  notched, which is normal. Flanges that run right to a corner (no offset) get a butt
  corner with a small gap; `corner` changes how those are treated.
- `hem`: `kind` is `closed` (folded flat, the default), `open` (folded leaving `gap`),
  `teardrop` or `rolled` (these use `radius` and `angle`); `length` is the hem's length;
  `inside` (default true) keeps the fold's outside flush with the original edge, so the
  part doesn't get taller. `peet feature` shows `gap`, `radius` and `angle` for every
  hem, but a closed hem doesn't use them.
- If a feature's status is `warning` and says the sheet is in **separate pieces**, a
  relief or a cut has cut part of the sheet off: make the offsets larger, or set the
  base flange's `relief` to `tear`.
- Selecting an edge after flanges exist: a flange that runs to a corner moves the end of
  the next edge, so `between` with the plate's original corners may match nothing.
  Select with `{"at": [x, y, z]}` at the middle of the edge, or list the edges again
  (`peet edges`). The edges of one `edges` list are all found before any flange is
  added, so original corners always work there.
- `miter_flange` runs a sketched profile along several edges of one face;
  `sketched_bend` and `jog` bend a flat face along sketched lines; `corner` sets how
  flanges meet.

## Cuts and forms

- Use **`sheet_cut`**, with the sketch on a flat face of the sheet. It cuts in the flat
  pattern, so it can run across bends, and the body stays sheet metal.
- `cut` and `extrude` on a sheet metal body turn it into a plain solid with no flat
  pattern: the feature's status is `warning` and says so. If that happened, `delete` it
  and use `sheet_cut`.
- `dimple`, `emboss` and `louver` press forms from a sketch's shapes: circles for
  dimples, closed outlines for embosses and louvers, one form per shape. They are marked
  in the flat pattern, not cut. `height` is how far the form stands out of the face the
  sketch is on (default 3); `flip` presses it into that face instead.
- A louver is closed on three sides and open on one: `open_side` says which edge of the
  outline is the opening, counting the outline's edges from 0 in the order they were
  drawn (for a `rectangle`: 0 bottom, 1 right, 2 top, 3 left).
- `dimple`, `emboss` and `louver` are one kind of feature: each has a `kind` field that
  says which, already set by the operation's name. Leave it out.
- To repeat a hole or a form, make one and use `linear_pattern` on the feature.
- A face selector for the sketch: `{"feature": "Base-Flange1", "side": "top"}` matches
  the plate's top face. Once flanges exist they have `top` faces of their own (made by
  the flange features), so name the feature that made the face you want.

## Flat pattern and export

- `bend_table`: the flat size, and each bend's direction, angle, radius and allowance.
  Check the flat size against the stock.
- `{"op": "export", "path": "part.dxf"}` writes the flat pattern: outline, cutouts,
  bend lines and notes on separate layers, in mm. `.step` and `.stl` write the folded
  part.
- `flat_pattern` only changes what the application shows: exports don't need it.

## Checks

`checks` reports flanges too short to bend, holes too close to a bend or an edge, holes
too small, and parts that collide when folded. `"passed": true` means none. The limits
are multiples of the thickness and bend radius: `set_check_rule` changes one for the run.
Run `checks` before exporting for manufacture, and fix each finding by moving or
resizing what it names.
