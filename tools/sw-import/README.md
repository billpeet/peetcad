# sw-import

Rebuilds SolidWorks parts in PeetCAD from their exports (`tools/sw-export`), and says how
close each came out.

```
node tools/sw-import/import.js <export.swx.json | folder of exports> <output folder> [--peet <peet>] [--only <text>] [--jobs <n>] [--step <folder>] [--no-relations]
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
text. `--jobs` imports that many parts at once, each in a process of its own: the library takes about half an hour with 16,
most of it Gear tray. It uses `target/release/peet.exe` if there is one, then the debug build; give a copy with `--peet`
for a long run, or a build in the meantime can't replace it.

`--step` takes a folder of SolidWorks' STEP files (`<Name>.SLDPRT.<configuration>.step`). Where a part has
one, the volume and box to match are its STEP body's as PeetCAD measures it, not SolidWorks' mass properties,
which are a few parts in 100,000 out round freeform faces such as the ends of slanted bends. A part nothing
can be built of is imported from its STEP file instead, and says so in its report.

The library's export is on SolidWorks-PC in `C:\Users\will\AppData\Local\PeetSwExport\out-all`, and a copy
in `peetcad-imports\exports\out-all`, with SolidWorks' STEP files in `peetcad-imports\exports\step`. Keep copies out of `%TEMP%`, which Windows empties.

It is a tool for the migration, not a feature of PeetCAD: the export is SolidWorks' own
feature data under SolidWorks' own names, and an importer inside `peet` would tie
PeetCAD to that. What it finds missing goes to
[docs/solidworks-parity.md](../../docs/solidworks-parity.md).

## Assemblies

```
node tools/sw-import/import-asm.js <export.swx.json | folder of exports> <parts run folder> <output folder> [--peet <peet>] [--only <text>]
node tools/sw-import/summarise-asm.js <output folder> [--assemblies]
```

`import-asm.js` rebuilds assemblies from their exports, using the `.peet` parts a run of `import.js`
made. It writes `<name>.peet`, `<name>.jsonl` and `<name>.report.json` for each, sub-assemblies under
`sub-assemblies\` (from their own export, or from what their parent lists under them) and parts used in
another configuration than their file's under `parts\` (a copy with that configuration active: a
stand-in until PeetCAD's components can choose a configuration).

Each component is inserted where SolidWorks has it, then each mate is added; an end is found among its
part's faces and edges by its geometry (SolidWorks' plane, axis and radius, circle or line, or its
numbers for the end in the assembly). A mate that agrees with SolidWorks moves nothing: the report gives,
for each mate, how far the components are from SolidWorks' placement after it (`moved`), and for each
component how far it ended up (`off_mm`) and how free it still is. The verdict is `matches` when every
component is within a micron of SolidWorks' placement and every live mate and component is imported.

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
| Reference plane | `plane`: an offset of a standard plane, or of one turned about an axis, where SolidWorks has it (its transform runs from the plane to the model, the other way from a sketch's) |
| Base flange | `base_flange`, with the body's thickness, radius (0 too), bend deduction and relief, and each bend's own bend model from SolidWorks' sketched bends. From an open profile: as deep as SolidWorks has it, to both sides for its mid-plane end condition, with lines that are in line joined; which side the sheet is on is tried both ways and the one kept whose faces are where SolidWorks' are |
| Edge flange | `edge_flange`: length converted to the outer corner, position (SolidWorks' meaning, which PeetCAD's has), position offset, angle, set-backs read off its outline sketch (negative where the outline runs past the edge), its own relief and bend allowance where it has them. An outline that is not a rectangle is its `outline`, drawn on a plane turned about the edge to where SolidWorks' profile sketch is |
| Mitre flange | `miter_flange` on the same edges with the same gap and offsets. The profile is tried as drawn and with it, or its start, moved a thickness across the sketch: PeetCAD puts the sheet on the side of the profile that follows from the corner of the edge it starts at, SolidWorks from the flange's position. The one kept has its flat faces in the most planes SolidWorks' has |
| Cut-extrude | `sheet_cut` on sheet metal, with SolidWorks' end condition as its `end`, `depth` and `both`: through next and up to a surface or a vertex as deep as SolidWorks' walls of the cut reach. `cut` on a solid. Which way either goes is the way that makes SolidWorks' walls where they are. A cut of nothing but text is left out (P5) |
| Boss-extrude | `extrude`; up to a surface or a vertex, as deep as SolidWorks' faces of it go |
| Hole wizard | `sheet_cut` with circles on sheet metal (with the hole's end condition), `hole` on a solid. A slot: a sketch slot on the line its sketch has from tip to tip |
| Linear pattern, mirror | `linear_pattern`, `mirror`, of edge flanges too. A pattern along a flat face goes along its normal; one whose direction was not exported goes the way SolidWorks' body has the copies from the original |
| Break corner | `break_corner`, where the corner's edge is in the part |

These are stood in for, with a warning in the report. Each has the right shape but does
not follow the part when it changes:

- **A break corner whose edge is not found, and a fillet on a sheet's corner:** a sheet
  metal cut of the shape SolidWorks removed, read off the faces the feature left in
  SolidWorks' body.
- **Slots from the hole wizard on several faces**, which its one sketch does not show: a
  sheet metal cut of a slot, read off its round ends, on each face the feature has
  slots on.
- **A mirrored flange PeetCAD's mirror can't copy** (a mitre flange, or one whose edge
  across the plane is not there yet): a flange of its own on the edge across the plane,
  a mitre flange with its profile moved across the plane.
- **A flange whose outline can't be its own** (no plane can be put where the profile is,
  or the outline is not where the flange is): a rectangular flange, and a sheet metal
  cut of everything between the outline and a box around it. Where the outline reaches
  further than the flange's length (a round top), the flange is made long enough first.

**Corners** are set after each flange feature (edge or mitre), as SolidWorks leaves them:

- between the pieces of one flange feature on several edges, `corner` with `open`, the
  feature's gap over 1.414 and `slanted`, which is what SolidWorks' folded part has
  (its flat pattern has square ends there, which is `flush`);
- between the new flanges and earlier ones, `corner` with `plain` and the bend radius,
  kept only if it brings the flanges' faces nearer SolidWorks' than PeetCAD's own corner
  does (it does not where flanges are at an angle). Failing that, PeetCAD's own corner
  with a `tear` relief: SolidWorks cuts nothing where the bends share nothing.

A flange's set-backs come from its outline sketch: the line of the outline that runs
along the edge, which may be a thickness away from it.

Besides the features, it brings over:

- **Sketch relations and dimensions**, so that a sketch can be driven. They are added all
  at once; if that moves the geometry or over-defines the sketch they are taken back and
  added one at a time, keeping those that leave the sketch as drawn. What a relation
  refers to outside the sketch (an edge, a vertex, a reference plane) is projected in
  first. Segments are matched by where they are: the ids a relation gives are not the
  ids the sketch lists its segments under.
- **Suppressed features.** One that is live in another configuration is left for that
  configuration and built there, where SolidWorks' body has it to check against. One
  that is live nowhere is built and then suppressed in every configuration, so that an
  equation or FabSys can bring it back; it can't be checked. One that can't be built is
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
- **Sketched bends**, which PeetCAD has; **corner reliefs** and **library features**,
  whose definitions the export does not have.
- **A cut that removes nothing in SolidWorks** because it points away from the part:
  it can't be told from one whose faces later features took away, and is cut both ways
  to see (Plinth bolt).
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
