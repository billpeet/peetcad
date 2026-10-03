---
name: sheet-metal
description: Sheet metal in peet - base flange, edge flanges, hems, cuts, forms, the flat pattern and its DXF, bend table, materials and gauges, manufacturing checks.
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

## Flanges and hems

- `edge_flange` and `hem` go on an edge **of the sheet's top or bottom face**: the long
  edges where a flat face meets the sheet's cut side. On a plate on `top` of thickness
  1.5, the top face's edges are at z = 1.5 and the bottom's at z = 0. An edge on the top
  face bends the flange up; `flip` bends it the other way.
- `edges` (a list) makes one flange per edge in one operation.
- `length` is measured on the **outside**, from the outer corner to the flange's end.
  `position` says where the bend sits relative to the edge (`material_inside`,
  `material_outside`, `bend_outside`).
- Flanges on edges that meet at a corner are given a butt corner with a relief and a
  small gap. `offset_start` and `offset_end` set a flange back from the ends of its
  edge; `corner` changes how corners are treated.
- After a flange is added, the ends of the neighbouring edges may have moved. For the
  next flange select the edge with `{"at": [x, y, z]}` at its middle, or list the edges
  again (`peet edges`). Flanges given together in one `edges` list don't have this
  problem.
- `miter_flange` runs a sketched profile along several edges of one face;
  `sketched_bend` and `jog` bend a flat face along sketched lines; `corner` sets how
  flanges meet.

## Cuts and forms

- Use **`sheet_cut`**, with the sketch on a flat face of the sheet. It cuts in the flat
  pattern, so it can run across bends, and the body stays sheet metal.
- `cut` and `extrude` on a sheet metal body turn it into a plain solid with no flat
  pattern: the feature's status is `warning` and says so. If that happened, `delete` it
  and use `sheet_cut`.
- `dimple`, `emboss` and `louver` press forms from a sketch's shapes; they are marked in
  the flat pattern, not cut.
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
