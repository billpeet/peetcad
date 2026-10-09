# sw-import

Rebuilds SolidWorks parts in PeetCAD from their exports (`tools/sw-export`), and says how
close each came out.

```
node tools/sw-import/import.js <export.swx.json | folder of exports> <output folder> [--peet <peet>] [--only <text>]
node tools/sw-import/summarise.js <output folder> [--parts]
```

For each part it writes:

- `<name>.peet`: the part.
- `<name>.jsonl`: the operations that were applied, which `peet run <name>.jsonl --new -f x.peet`
  applies again.
- `<name>.report.json`: every SolidWorks feature and what became of it, SolidWorks' and
  PeetCAD's volume, area, bounding box and flat size, and a verdict.

A folder of exports also gets `summary.json`. `summarise.js` counts the verdicts and the
reasons features were left out. `diagnose.js <export> <part.peet>` says where one part
differs: the area of its flat faces against SolidWorks', feature by feature. `--only` takes the parts whose file name contains the
text. It uses `target/release/peet.exe` if there is one, then the debug build.

It is a tool for the migration, not a feature of PeetCAD: the export is SolidWorks' own
feature data under SolidWorks' own names, and an importer inside `peet` would tie
PeetCAD to that. What it finds missing goes to
[docs/solidworks-parity.md](../../docs/solidworks-parity.md).

## How it works

It applies one operation at a time, one `peet` call each, because the next operation
often depends on the reply: where PeetCAD put a sketch's plane, which edge of the part
is the one SolidWorks means, whether a flange came out on the right side. An operation
that fails, or a feature that can't be built, is taken back and recorded.

Coordinates are SolidWorks', in millimetres. SolidWorks' Front Plane is the XY plane,
which is PeetCAD's `top`, so a sketch goes on the PeetCAD plane or face that lies where
its SolidWorks plane does, and its points are converted through the two planes' axes.

| SolidWorks | PeetCAD |
|---|---|
| Sketch | `sketch` with its lines, arcs and circles where they are, ends that meet joined |
| Reference plane | `plane`: an offset of a standard plane, where SolidWorks has it |
| Base flange | `base_flange`, with the body's thickness, radius, bend deduction and relief |
| Edge flange | `edge_flange`: length converted to the outer corner, position, angle, set-backs read off its outline sketch |
| Cut-extrude | `sheet_cut` on sheet metal, `cut` on a solid |
| Boss-extrude | `extrude` |
| Hole wizard | `sheet_cut` with circles on sheet metal, `hole` on a solid |
| Linear pattern, mirror | `linear_pattern`, `mirror` |

Three things PeetCAD has no feature for are stood in for, with a warning in the report.
Each has the right shape but does not follow the part when it changes:

- **Break corner, and a fillet on a sheet's corner:** a sheet metal cut of the shape
  SolidWorks removed, read off the faces the feature left in SolidWorks' body.
- **A slot from the hole wizard:** a sheet metal cut of a slot, read off its round ends,
  on each face the feature has slots on.
- **A mirrored flange:** a flange of its own on the edge across the mirror plane.

**Corners** are set after each flange feature, as SolidWorks leaves them:

- between the pieces of one flange feature on several edges, `corner` with `open`, the
  feature's gap over 1.414 and `flush`;
- between the new flanges and earlier ones, `corner` with `plain` and the bend radius,
  kept only if it brings the flanges' faces nearer SolidWorks' than PeetCAD's own corner
  does (it does not where flanges are at an angle).

A flange's set-backs come from its outline sketch: the line of the outline that runs
along the edge, which may be a thickness away from it.

Besides the features, it brings over:

- **Sketch relations and dimensions**, so that a sketch can be driven. They are added all
  at once; if that moves the geometry or over-defines the sketch they are taken back and
  added one at a time, keeping those that leave the sketch as drawn. What a relation
  refers to outside the sketch (an edge, a vertex, a reference plane) is projected in
  first. Segments are matched by where they are: the ids a relation gives are not the
  ids the sketch lists its segments under.
- **Suppressed features**: built, then suppressed in every configuration, so that a
  configuration, an equation or FabSys can bring them back. One that can't be built is
  recorded as suppressed, with the reason.
- **The names of faces and edges**, with `name_face` and `name_edge`.
- **Equations in force.** A global variable becomes a parameter, a dimension set by a
  formula gets the formula, a feature suppressed by a condition gets a suppression rule.
  A formula can't read a sketch dimension from outside its sketch, so one that a formula
  reads is made to follow a parameter of its own (`Sketch1_Width`), as is a pattern's
  count or spacing. `"Thickness"` becomes a parameter the base flange follows. An
  equation is kept only if the part is the same afterwards.
- **The other configurations**: each is added, given the sketch dimensions and
  suppression that differ, has the features imported that are live only there, and is
  compared with SolidWorks' body in that configuration.

The report counts each of these, and says why whatever was dropped was dropped.
`--no-relations` leaves the relations and dimensions out, which is several times
quicker and enough to check shapes.

## What it does not do yet

- **Sizes of features** (a flange's length, a pattern's count) that differ between
  configurations or are set by an equation, except a linear pattern's.
- **Sketched bends, mitre flanges, corner reliefs**, which PeetCAD has.
- **Reference planes that follow** what they were defined from: they are fixed offsets.
- **Checking a suppressed feature.** It is built without anything to compare it with.
- Assemblies.

It reads every format of export. The first does not say whether an extrude is a boss or
a cut, nor where a relation's entities are, so from it the name decides the first and
few relations are found. Before the third, what is inside a sketch block is missing and
the way a distance is measured is worked out from the points.

A sketch block is drawn as plain geometry where each instance puts it: PeetCAD sketches
have no blocks, so the grouping is lost.

## Verdicts

| Verdict | Means |
|---|---|
| matches | Volume within a part in 100,000, bounding box within a micron, flat size within 0.01 mm, every feature imported |
| matches, with features left out | The same, though something was left out (one that changes nothing, like a suppressed-in-effect cut) |
| same size and flat pattern, volume differs | Box and flat size match and the volume is within 5%: in practice, the corners where flanges meet |
| differs | Everything was imported and the result is not the same |
| differs: features left out | Something could not be imported |
| nothing built | The part's first solid feature could not be imported |
