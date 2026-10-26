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
- `thickness`, `radius` (the default inner bend radius; 0 is a sharp bend, as in
  SolidWorks) and `bend` (`{"k_factor":
  0.44}`, or an `allowance` or `deduction`) apply to the whole body. Defaults: 1.5 mm,
  1.5 mm, K 0.44. Or take them from the material tables: `peet materials`, then
  `{"op": "apply_material", "material": ..., "gauge": ...}`. That also makes it the
  part's material, so `mass` gives `mass_kg`.
- `relief` and `relief_ratio` are also set here, for the whole body. A **relief** is the
  small cut at each end of a bend that stops short of the end of its edge, so the sheet
  doesn't tear. `relief`: `rectangular` (the default), `obround` or `tear` (no cut: the
  sheet is slit). `relief_ratio`: its width, and how far it reaches past the bend, as a
  multiple of the thickness (default 0.5).

## Tabs

A tab adds a flat piece to a sheet metal body: a closed sketch on a flat face of the
sheet (or on a plane in that face's plane), `base_flange` with `tab: true`. It is part of
that face's flange, with the body's thickness (its own `thickness`, `radius`, `reverse`
are ignored), so it folds and unfolds with it. It is what SolidWorks' base flange makes
on a part that has sheet metal already ("Tab1").

- The sketch must touch the sheet: overlap the face or share an edge with it. One that
  touches nothing fails ("A tab adds to a sheet metal body ..."): move the sketch, or
  leave out `tab` for a body of its own.
- It fills in what earlier cuts took: a tab drawn across a slot bridges it. Later cuts
  cut it like the rest of the face.
- An inside corner it makes with the sheet takes a `break_corner` like any other.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [300, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "sketch", "name": "Slot", "on": {"feature": "Base-Flange1", "side": "top"}, "draw": [{"type": "rectangle", "from": [20, 20], "to": [280, 40]}]}
{"op": "sheet_cut", "sketch": "Slot"}
{"op": "sketch", "name": "Bridge", "on": {"at": [0, 0, 2], "normal": [0, 0, 1]}, "draw": [{"type": "rectangle", "from": [40, 20], "to": [60, 40]}]}
{"op": "base_flange", "name": "Tab1", "sketch": "Bridge", "tab": true}
{"op": "break_corner", "name": "Round", "size": 3, "edges": [{"between": [[60, 20, 0], [60, 20, 2]]}, {"between": [[60, 40, 0], [60, 40, 2]]}]}
{"op": "linear_pattern", "features": ["Tab1", "Round"], "direction": "x", "spacing": 100, "count": 3}
{"op": "bodies"}
```

The slot is cut first and bridged three times: the tab, then two copies of it and of its
rounded corners.

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
- `thickness` makes a flat body sheet of another thickness, hanging from `face` into the
  solid, as SolidWorks' Convert to Sheet Metal does with the thickness it is given (a
  3 mm plate converted at 2 mm keeps its top face and loses a millimetre underneath). A
  body with bends can only be converted at the thickness it has.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [70, 210]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 3}
{"op": "convert_to_sheet", "face": {"normal": [0, 0, 1]}, "thickness": 2}
{"op": "bodies"}
```
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
  `material_outside`, `bend_outside`). They mean what they mean in SolidWorks: at an
  angle other than 90, a `material_inside` flange's outer face goes through the edge on
  the far face of the sheet, so a shallow flange is set well back from the edge.
- `position_offset` sets the whole flange out from its edge (SolidWorks' "Offset" in a
  flange's position): the sheet runs on flat that far past the edge before the bend.
  Negative sets it in. It adds to `position`, so a `bend_outside` flange set out 7 bends
  7 beyond where it otherwise would.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "edge_flange", "length": 20, "position": "bend_outside", "position_offset": 7, "edge": {"between": [[0, 60, 2], [100, 60, 2]]}}
```

- `up_to` runs a flange up to a point in place of its `length` (SolidWorks' "Up To
  Vertex"): a vertex selector (`{"at": [x, y, z]}`), a reference point's name or
  `"origin"`. The flange's end goes through the point, square to the flange, and follows
  it when the part changes; `length` is kept but not used (`null` goes back to it). The
  point need not be on the flange: here the side ends level with the top of the wall.
  "The point it runs up to is behind the flange's bend" means it is on the other side
  of the bend: flip the flange or pick another point.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "edge_flange", "name": "Wall", "length": 30, "edge": {"between": [[0, 0, 2], [100, 0, 2]]}}
{"op": "edge_flange", "name": "Side", "length": 10, "up_to": {"at": [100, 0, 30]}, "edge": {"at": [100, 30, 2]}}
{"op": "bodies"}
```

- `outline` shapes a flange to a sketch of its own, as SolidWorks' "Edit Flange
  Profile" does: a trapezoid, a tab with a round top, a flange wider at its tip. Draw it
  first, on a plane turned about the flange's edge by the flange's angle (from the face
  the edge is on, `about` the edge), with its foot along the edge. The flange is then as
  wide as the sketch's region along the edge and as long as it reaches, which overrides
  `length`, `offset_start` and `offset_end`. Everything outside the region, from its
  foot out, is cut from the flange and its bend. It follows the sketch: change a
  dimension and the flange changes.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
{"op": "plane", "from": {"feature": "Base-Flange1", "side": "top"}, "about": {"between": [[0, 60, 2], [100, 60, 2]]}, "angle": 90, "name": "Lip plane"}
{"op": "sketch", "name": "Lip shape", "on": "Lip plane", "draw": [{"type": "polyline", "closed": true, "points": [[10, 62], [90, 62], [70, 32], [30, 32]]}]}
{"op": "edge_flange", "name": "Lip", "length": 10, "outline": "Lip shape", "edge": {"between": [[0, 60, 2], [100, 60, 2]]}}
```

  Look at the plane before drawing (`{"op": "sketch", "on": "Lip plane", "draw": []}`
  replies with its `origin`, `x` and `y`). Here x runs along the edge and y down the
  flange from 62 above the sheet, so the trapezoid is 80 wide at the bend and 40 wide 30
  up.
- `offset_start` and `offset_end` set a flange back from the two ends of its edge, with
  a relief at each end: a wall on a 180 edge with both offsets 5 has a bend 170 long
  (`bend_table` gives each bend's `length`). Where two flanges are set back a little
  from the same corner, their reliefs cut the corner of the sheet off: the corner is
  notched, which is normal. Flanges that run right to a corner (no offset) get a butt
  corner with a small gap; `corner` changes how those are treated.
- The reliefs at a set-back flange's ends are the body's (the base flange's `relief`
  and `relief_ratio`) unless the edge flange has its own: `"relief": "tear"` cuts
  nothing, which is right when the bend takes little of the sheet (`material_outside`)
  and the body's rectangular relief would only notch it; `"relief_ratio": 1` makes the
  slot one thickness wide and deep past the bend. `null` goes back to the body's. Hems
  and miter flanges always use the body's. A flange whose bend is wholly outside the
  sheet (`bend_outside`, or set out by `position_offset`) gets no relief at all, as in
  SolidWorks: it takes nothing from the sheet.

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.2}
  {"op": "edge_flange", "name": "Lip", "length": 25, "position": "material_outside", "offset_start": 40, "offset_end": 40, "relief": "tear", "edges": [{"between": [[0, 0, 2], [200, 0, 2]]}]}
  {"op": "edge_flange", "name": "Tab", "length": 25, "offset_start": 60, "offset_end": 60, "relief": "obround", "relief_ratio": 1, "edges": [{"between": [[200, 150, 2], [0, 150, 2]]}]}
  {"op": "bodies"}
  ```
- A flange on an edge that runs to a corner of its face **sharper than square** (a
  triangle's side), with its bend inside the sheet (`material_inside`,
  `material_outside`): the face is narrower where the bend starts than the edge is
  long, and the bend ends on a slant from the face's corner to the end of its flange,
  as SolidWorks' does. Nothing to set; the bend's ends are freeform faces.

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "polyline", "closed": true, "points": [[0, 0], [100, 0], [0, 100]]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
  {"op": "edge_flange", "name": "Lip", "length": 20, "position": "material_outside", "relief": "tear", "edges": [{"between": [[100, 0, 2], [0, 100, 2]]}]}
  {"op": "bodies"}
  ```
- A lip on a wall's **side edge**, which runs down to the bend the wall hangs from: with
  its bend inside the sheet, the strip that bend takes from the wall runs on through the
  wall's bend, which is cut away there, as SolidWorks does. Nothing to set; the wall's
  bend starts the lip's setback (radius + thickness) in from the side.

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
  {"op": "edge_flange", "name": "Wall", "length": 30, "edge": {"between": [[0, 0, 2], [100, 0, 2]]}}
  {"op": "edge_flange", "name": "Lip", "length": 15, "edge": {"at": [0, 2, 20]}}
  {"op": "bodies"}
  ```
- An edge flange's bend is worked out as the body's are unless it has a `bend` of its
  own (`{"deduction": 3.5}`, or `k_factor` or `allowance`): for the one bend the press
  brake makes differently, such as a bend at another angle. The folded part is the
  same; the blank is that much longer or shorter. `null` goes back to the body's. "The
  bend allowance works out at ... it must be greater than zero" on one flange means
  the body's deduction is too big for that flange's angle or radius: give it its own.

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.2, "bend": {"deduction": 3.3}}
  {"op": "edge_flange", "name": "Hood", "length": 60, "angle": 105, "bend": {"deduction": 3.5}, "edges": [{"between": [[0, 0, 2], [200, 0, 2]]}]}
  {"op": "bend_table"}
  ```
- The bends of a base flange from an open profile take `bends`: a bend model or `null`
  for each, in order along the profile, as SolidWorks' sketched bends can have their
  own. Give a shallow bend a smaller deduction when "Corner N: The bend allowance works
  out at ..." says the body's is too big for it.

  ```jsonl
  {"op": "sketch", "on": "front", "draw": [{"type": "polyline", "points": [[0, 0], [100, 0], [200, 20], [200, 40]]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.1, "bend": {"deduction": 2.5}, "depth": 50, "bends": [{"deduction": 0.1}, null]}
  {"op": "bend_table"}
  ```
- A **negative** offset runs an edge flange or a hem past that end of its edge, for one
  wider than the sheet it is bent from: `"offset_end": -12` ends it 12 beyond the
  edge. The bend stays on the edge and the flange overhangs it, with no relief. The
  start of an edge is the end from which the sheet is on your left, looking down on
  the sheet's top face (the order of a `between` does not change it); if in doubt, try
  one end and look at `bodies` (`min`, `max`) to see which way it went. The overhang must not
  land on another flange: "The sheet overlaps itself" means it does.

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 1.5, "radius": 2}
  {"op": "edge_flange", "name": "Wide", "length": 20, "offset_start": -12, "offset_end": 5, "edges": [{"between": [[200, 150, 1.5], [0, 150, 1.5]]}]}
  {"op": "bodies"}
  ```
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
  `sketched_bend` and `jog` bend a flat face along sketched lines.

## Corners

Where two flanges run to the same corner of a face, one of them has to give way and
the two bends can't both have the corner of the flat pattern. Left alone, the earlier
flange runs to the corner, the later one stops 0.1 short of it (a butt corner), and a
rectangle is cut out of the blank where the bends meet. `corner` changes that: add it
after the flanges it is about.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [120, 80]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.2}
{"op": "edge_flange", "name": "Wall", "length": 25, "edges": [{"between": [[0, 0, 2], [120, 0, 2]]}, {"between": [[120, 0, 2], [120, 80, 2]]}, {"between": [[120, 80, 2], [0, 80, 2]]}, {"between": [[0, 80, 2], [0, 0, 2]]}]}
{"op": "corner", "kind": "open", "gap": 0.7, "relief": "flush"}
{"op": "bend_table"}
```

- `kind`: `butt` (the earlier flange runs to the corner), `overlap` (the later one does),
  `open` (neither does: both stop short of the other's inside face) or `plain` (nothing
  is made longer to close the corner: the earlier flange ends where its own edge does
  and the later one stops short of it). `plain` differs from `butt` when the later
  flange stands outside its edge (`material_outside`, `bend_outside`): a butt corner
  runs the earlier flange on to meet it, a plain one doesn't.
- `gap` is how far a flange that stops short stays from the other's inside face. In an
  `open` corner it is measured from the flange's own inside face, so where the face's
  corner is not square the flange's outside face comes a little nearer the other
  flange (or stays a little further off).
- `relief` is what is cut from the blank where the two bends meet: `rectangular` (a
  rectangle reaching `relief_size` past the bends), `tear` (only what the bends would
  share), `flush` (back to where the flanges end, so each bend ends in line with its
  flange and the blank has one inside corner there, with no notch) or `slanted`
  (nothing is cut from the face: each bend ends on the line from the face's corner to
  the end of its flange, so the bends' ends are freeform faces and the blank has two
  short slanted edges at each corner).
- **To match a part made in SolidWorks:** for the corners of one of its flange features
  on several edges, `open` with `flush` and its gap divided by 1.414 (SolidWorks
  measures the gap between the two flanges corner to corner, and both stop short; at a
  corner of the face that is not square, its gap times the cosine of half the face's
  angle there, one `corner` for each gap); for
  a corner between two of its flange features, `plain` with the bend radius as the gap.
  The blank is then the same, edge for edge. SolidWorks' folded part is not quite its
  blank folded: there the bends end on a slant and the face keeps its corner. `slanted`
  in place of `flush` gives that solid (the same area, the volume within 0.003%), with
  a blank that has the slants in it:

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [120, 80]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.2}
  {"op": "edge_flange", "name": "Wall", "length": 25, "edges": [{"between": [[0, 0, 2], [120, 0, 2]]}, {"between": [[120, 0, 2], [120, 80, 2]]}, {"between": [[120, 80, 2], [0, 80, 2]]}, {"between": [[0, 80, 2], [0, 0, 2]]}]}
  {"op": "corner", "kind": "open", "gap": 0.7, "relief": "slanted"}
  {"op": "bodies"}
  ```
- `flush` is never less than a tear, so with a gap smaller than the bend radius the
  bends still end before their flanges do.
- `circular` and `square` are SolidWorks' corner relief feature: a circle, or a square
  with its sides along the two edges, `relief_size` across, centred where the two
  flanges' ends meet in the blank, cut from the face, both bends and both flanges. They
  change only the relief, so give the corner the `kind` and `gap` it had. Use one where
  the part needs a drilled or punched hole at each corner. Outside the shape the blank
  is cut as `flush` cuts it, so no bend runs past its flange. To bring a SolidWorks
  Corner Relief over, apply it to the same corners after the flange's own `corner`:

  ```jsonl
  {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [120, 80]}]}
  {"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 0.2}
  {"op": "edge_flange", "name": "Wall", "length": 25, "edges": [{"between": [[0, 0, 2], [120, 0, 2]]}, {"between": [[120, 0, 2], [120, 80, 2]]}, {"between": [[120, 80, 2], [0, 80, 2]]}, {"between": [[0, 80, 2], [0, 0, 2]]}]}
  {"op": "corner", "kind": "open", "gap": 0.7, "relief": "slanted"}
  {"op": "corner", "name": "Corner Relief1", "kind": "open", "gap": 0.7, "relief": "circular", "relief_size": 6}
  {"op": "bodies"}
  ```
- Without `faces` it is every corner the part has when it is applied; corners made by
  flanges added later keep the default. `faces` picks corners: the end face of a flange
  at a corner picks that corner, a flange's top or bottom face picks the corners at both
  its ends.
- "The part has no corners where two flanges meet": the flanges are set back from the
  corner (an `offset_start` or `offset_end`), or are on different faces. Nothing to do.

## Breaking corners

`break_corner` rounds or chamfers corners of the sheet's outline: of the plate, of a
flange, of a cutout's edge with the outside. It works in the flat pattern, so the body
stays sheet metal (`fillet` and `chamfer` turn it into a plain solid).

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
{"op": "edge_flange", "name": "Wall", "length": 40, "offset_start": 20, "offset_end": 20, "edges": [{"between": [[0, 0, 2], [200, 0, 2]]}]}
{"op": "break_corner", "name": "Rounds", "size": 10, "edges": [{"between": [[0, 150, 0], [0, 150, 2]]}, {"between": [[200, 150, 0], [200, 150, 2]]}]}
{"op": "break_corner", "name": "Nips", "kind": "chamfer", "size": 3, "edges": [{"at": [20, 1, 40]}, {"at": [180, 1, 40]}]}
{"op": "bodies"}
```

- `edges` are the corners: each the short edge **through the sheet** where two sides
  of a flat face meet, one sheet thickness long. `between` its two ends, or `at`
  its middle, not an end, where other edges meet it (`peet edges` lists them; a corner edge has both ends at one place on
  the face, a thickness apart). An edge along a face is not a corner.
- `kind`: `fillet` (the default; `size` is the radius) or `chamfer` (`size` is the
  distance along each side). A chamfer needs two straight sides; a fillet also rounds
  a corner between a straight side and an arc, or two arcs (where a cut's round end
  meets a straight edge): the round touches both, as SolidWorks' fillet does.
- An outside corner loses sheet; an inside corner (of a notch, an L or a cutout) gains
  it, as a fillet there does in SolidWorks. A cut made later still cuts the round. A
  corner next to a bend is not a corner of a flat face: break the flange's far
  corners, or cut the shape with `sheet_cut`.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "polyline", "closed": true, "points": [[0, 0], [100, 0], [100, 30], [50, 30], [50, 60], [0, 60]]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "break_corner", "size": 5, "edges": [{"between": [[50, 30, 0], [50, 30, 2]]}, {"between": [[100, 0, 0], [100, 0, 2]]}]}
```

A cutout's inside corners, and the corners where a slot's round end meets the plate's
edge:

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 100]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "sketch", "on": {"at": [100, 50, 2], "normal": [0, 0, 1]}, "name": "Window", "draw": [{"type": "rectangle", "from": [20, 20], "to": [80, 80]}]}
{"op": "sheet_cut", "sketch": "Window"}
{"op": "break_corner", "name": "Window rounds", "size": 5, "edges": [{"between": [[20, 20, 0], [20, 20, 2]]}, {"between": [[80, 80, 0], [80, 80, 2]]}]}
{"op": "sketch", "on": {"at": [100, 50, 2], "normal": [0, 0, 1]}, "name": "Slot", "draw": [{"type": "slot", "from": [130, 95], "to": [170, 95], "radius": 10}]}
{"op": "sheet_cut", "sketch": "Slot"}
{"op": "break_corner", "name": "Slot rounds", "size": 3, "edges": [{"at": [121.339746, 100, 1]}, {"at": [178.660254, 100, 1]}]}
{"op": "bodies"}
```
- "The corner's sides are too short" gives the largest size that fits there. Two
  corners at the ends of one side share it: their sizes together can't exceed it.
- It follows the part: a flange made longer takes its broken corners with it.

## Cuts and forms

- Use **`sheet_cut`**, with the sketch on a flat face of the sheet. It cuts in the flat
  pattern, so it can run across bends, and the body stays sheet metal.
- It cuts every flange that has a face in the sketch's plane, not only the one the
  sketch is on: holes drawn over two tabs or two lips that lie in one plane go through
  both, in one `sheet_cut`.
- **A cut through more than its own sheet**, as SolidWorks' cut-extrude goes: `end`
  `through_all`, or `blind` with a `depth` from the sketch's face. It goes into the
  sheet from that face (`reverse` the other way, `both` both ways) and cuts every flange
  in its reach: one parallel to the sketch if the reach takes in its whole thickness,
  and one square to it where the sketch's outline crosses it, on across the bend at its
  end if the reach goes past it. That is how to notch the ends of a channel's sides from
  its base, or put one hole through both sides of a U. Flanges at other angles are left.
  The default `end` is `sheet`: its own sheet and the flanges in its sketch's plane.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
{"op": "edge_flange", "name": "Side", "length": 20, "edges": [{"between": [[0, 0, 2], [100, 0, 2]]}, {"between": [[100, 60, 2], [0, 60, 2]]}]}
{"op": "sketch", "name": "Notch", "on": {"at": [50, 30, 2], "normal": [0, 0, 1]}, "draw": [{"type": "rectangle", "from": [-5, -5], "to": [10, 65]}]}
{"op": "sheet_cut", "sketch": "Notch", "end": "through_all", "reverse": true}
```

- **A cut sketched off the sheet's faces**, as SolidWorks' cut-extrude can be: on a plane
  (`plane` first, where the cut starts), or on the sheet's edge, the face along a flange's
  end that is as wide as the sheet is thick. Give it an `end` (`blind` with `depth`, or
  `through_all`): it goes in against the sketch's normal (into the face, for an edge face;
  `reverse` the other way) and cuts what its reach takes in, as above. A rectangle drawn
  on a flange's end face and cut `blind` notches the flange edge-on, as deep as `depth`;
  `through_all` takes the flange and its bend away there. With the default `end` it cuts
  only flanges with a face in the sketch's plane, and fails if there are none.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2, "radius": 1}
{"op": "edge_flange", "name": "Side", "length": 20, "edge": {"between": [[0, 0, 2], [100, 0, 2]]}}
{"op": "sketch", "name": "Slot", "on": {"at": [50, 1, 20], "normal": [0, 0, 1]}, "draw": [{"type": "rectangle", "from": [40, -5], "to": [60, 5]}]}
{"op": "sheet_cut", "sketch": "Slot", "end": "blind", "depth": 8}
{"op": "plane", "from": "right", "distance": 90, "name": "End cut"}
{"op": "sketch", "name": "Trim", "on": "End cut", "draw": [{"type": "rectangle", "from": [-5, -5], "to": [70, 30]}]}
{"op": "sheet_cut", "sketch": "Trim", "end": "through_all", "reverse": true}
```

- **Mirrors and patterns of flanges.** `mirror`, `linear_pattern` and
  `circular_pattern` copy an `edge_flange` as SolidWorks does: each copy is the same
  flange on the edge the motion takes the original's edge to, bent the way the motion
  takes its bend. A mirror turns the edge round, so the copy's set-backs are the other
  way round and it lies across from the original. A copy whose edge is not an edge of
  the sheet (the edge it would go on has a flange already, or is not there) is left out
  with a warning.
- A `miter_flange` is copied the same way: its profile along the edges the motion takes
  its edges to, in the other order for a mirror (so `offset_start` and `offset_end`
  change ends). Every one of those edges must be an edge of the sheet. A tab's copy is
  its sketch moved, added to the flange whose face the moved sketch is on; a
  `break_corner`'s copy rounds (or adds to) the corner the motion takes the original's
  to. Copy a break corner with the tab or flange whose corners it breaks, after it in
  `features` order (copies are made in model order, so a corner of a copied tab is there
  when the corner is copied).

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "edge_flange", "name": "Lip", "length": 20, "offset_start": 10, "offset_end": 60, "edge": {"between": [[0, 60, 2], [100, 60, 2]]}}
{"op": "plane", "from": "front", "distance": 30, "flip": true, "name": "Middle"}
{"op": "mirror", "features": ["Lip"], "plane": "Middle"}
```

A gutter (a mitre flange) mirrored to the other end of a plate:

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 100]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 2}
{"op": "sketch", "name": "Profile", "on": {"at": [100, 0, 1], "normal": [0, -1, 0]}, "draw": [{"type": "polyline", "points": [[0, 0], [0, 20], [8, 20]]}]}
{"op": "miter_flange", "name": "Gutter", "sketch": "Profile", "edges": [{"between": [[0, 0, 2], [0, 100, 2]]}], "offset_start": 5}
{"op": "plane", "from": "right", "distance": 100, "name": "Middle"}
{"op": "mirror", "name": "Other gutter", "features": ["Gutter"], "plane": "Middle"}
{"op": "bend_table"}
```

- **A cut across a bend:** draw it on the flat face as it lies in the blank, running
  past the face's edge. Past that edge the sketch is over the bend and then the flange,
  laid flat: a point `d` beyond the bend line in the sketch is `d` along the sheet in the
  folded part. Any shape can cross: a round hole on a bend comes out as a hole whose rim
  winds round the bend. `bend_table` and `flat_pattern` show the blank, where the cut is
  as drawn.
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
