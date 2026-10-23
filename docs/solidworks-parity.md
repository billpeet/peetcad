# Parity with the SolidWorks template library

What PeetCAD can't yet do that the SolidWorks component library needs. The aim is for
FabSys to drive PeetCAD in place of SolidWorks; parts come first, assemblies after.

**Status, 23 October 2026:** first written on 9 October from names and counts; since then
the whole library has been exported and imported, and the entries below say what that
found. Update an entry when it is confirmed, fixed or decided against.

## Where the numbers come from

- **The corpus:** `\\nas\Design\Solidworks\Templates`, 201 parts and 29 assemblies, as
  described by FabSys's manifests in `Manifests\` (feature names and types, dimension
  names, equations, configurations, components). The manifests hold no geometry, no
  values and not what a mate joins. Counts are for each document's active configuration.
- **The FabSys scripts:** 34 enclosure scripts under `\\nas\Design\Solidworks` (many are
  variants of one another, so their counts are given as shares).
- **PeetCAD:** `docs/scripting.md`, the skills, `ROADMAP.md` and ADR 0009, at commit
  `7920105` (Phase 7 merged).

Marks: **Gap** is known missing. **Check** needs a real import or a closer read to
settle. **Decide** is a choice to make, not just work to do.

## The import pipeline

- [x] **I1. Run the exporter.** `tools/sw-export` ran on SolidWorks-PC on the pilot
  (Pipe, b Tophat side, Battery Bracket, Door): all four complete, with STEP and flat
  DXF per configuration. See "What the pilot showed" below.
- [x] **I1b. Export the library.** Done on 9 October 2026: all 201 parts and 29 assemblies
  complete, in 76 minutes (18 seconds a file on average). 415 configurations, 3,466 sketches,
  287 STEP files and a flat DXF for every sheet metal configuration (249). The one thing not
  read: the definitions of 33 composite curves in `Laptop Shelf Assembly`. The output is on
  SolidWorks-PC in `C:\Users\will\AppData\Local\PeetSwExport\out-all` (82 MB).
- [x] **I2. Tally the export.** In [solidworks-library-tally.md](solidworks-library-tally.md):
  every feature, definition value, sketch relation and mate the library uses, with counts,
  against what PeetCAD takes. `node tools/sw-export/tally.js` makes the numbers.
- [ ] **I3. Importer.** `tools/sw-import` rebuilds a part from its export and compares it
  with SolidWorks: geometry, sketch relations and dimensions, face names, suppressed
  features, equations and the other configurations. What is left is in "What the
  importer found" below: mostly features it can't build yet, which every other number
  depends on.
- [ ] **I4. Verification.** Compare each imported part with SolidWorks' own numbers from
  the export: volume, bounding box, face count, flat size, positions of named faces.
- [ ] **I5. Fallback.** Parts the importer can't rebuild come in as STEP
  (`import_step`, `convert_to_sheet`), not drivable. Decide which parts that is
  acceptable for.

## What the pilot showed

From four parts, so these are sightings, not counts for the library.

- **Sketches lean on the model (P16, since done).** Dimensions and relations go to model
  edges, vertices and reference planes, not only to sketch geometry: in the Door, 25
  distances to edges, 6 to vertices and 4 relations to named planes (`Mid Plane`,
  `MidVerticalPlane`); converted edges in the Battery Bracket.
- **Relations in use:** coincident, horizontal, vertical, parallel, tangent, symmetric
  and fixed, which have matches, and collinear, coradial, at-midpoint of an edge,
  equal length, horizontal and vertical between two points, a doubled distance (to a
  centreline) and converted edges, which need checking one by one.
- **Slots from the hole wizard (new, P25).** `b Tophat side` has hole wizard slots.
  `hole` drills round holes.
- **Bend deduction everywhere.** All three sheet metal parts use a deduction of 3.325 mm
  at 2 mm thick with a 0.1 mm radius. PeetCAD has deductions; the flat sizes to match
  are in the export.
- **Named faces include holes.** The Door names flat faces (`Front`, `HingeFace`) and
  four hinge holes (round faces), so P3 has to cover both.
- **Edge flanges with their own profile sketch**, not fully defined (P10).
- **Library features** in the Door (3), on top of the 7 parts the manifests showed (P6).
- **Equations can be disabled**: two of the three in `b Tophat side` are.

## What the importer found

All 201 parts were last run through `tools/sw-import` on 23 October 2026, from the third
export (which adds what is inside sketch blocks, the way each dimension measures and
the selections of most suppressed features). The parts, the operations that made them
and a report each are in `C:\Users\marti\source\peetcad-imports\run-r5`. The run of
12 October, before that day's changes, is in `run-base` beside it, and
`scratch\compare.js` sets two runs against each other part by part.

**Shape, in the active configuration**, on 12 October and on 23 October:

| Result | 12 Oct | 23 Oct |
|---|---|---|
| Matches SolidWorks: volume, bounding box and flat size | 118 | 136 |
| Same size and flat pattern, volume within 5% | 17 | 17 |
| Everything imported, but it differs | 13 | 13 |
| A feature could not be imported | 47 | 30 |
| Nothing built | 6 | 5 |

"Matches" is about shape only. 97 of the 136 are built from PeetCAD features alone (80
of the 118 were). The other 39 match because the importer stands in for something with
sheet metal cuts of its own: flange outlines that are not rectangles in 15, flanges up
to a vertex in 10, hole wizard slots on several faces in 8, fillets on a sheet's corners
in 5 (a part can be in more than one of these). Those have the right shape and do not
follow the part when it changes (P35, P25).

**Beyond shape**, on 23 October:

| | Imported | Of |
|---|---|---|
| Sketch relations and dimensions | 5,869 | 7,275 (81%); every one kept in 273 of 759 sketches |
| Names of faces and edges | 311 | 320 |
| Equations in force | 48 | 90 |
| Other configurations that match SolidWorks | 42 | 85, in 50 parts; 14 more are the right size with the volume within 5% |

In 22 of the 50 parts with more than one configuration, every configuration matches.
What holds most of the others back is what holds back their active configuration.

- **Relations that were dropped.** Coincident relations PeetCAD refused (most join
  points the importer had already joined); redundant ones, which SolidWorks accepts and
  PeetCAD calls over-defined (P27); ones to something outside the sketch that could not
  be projected; sketch patterns and at-intersection, which have no match.
- **Suppressed features.** One that is live in another configuration is built there,
  where SolidWorks' body has it and it can be checked (a flange built where it is
  suppressed came out on the wrong side as often as not), and suppressed elsewhere.
  One that is live nowhere is built and then suppressed in every configuration. 243 could not be
  built: there is nothing to compare a suppressed feature with, so the side of a flange
  or the direction of a pattern is not checked, and many are old leftovers.
- **Equations.** A global variable becomes a parameter; a dimension set by a formula
  gets the formula; a feature suppressed by a condition gets a suppression rule; and a
  sketch dimension or a pattern's count that a formula reads is made to follow a
  parameter (`Sketch1_Length`), which is the way round P4. `"Thickness"` becomes a
  parameter the base flange follows. Each is kept only if the part is the same
  afterwards. Of the 46 not imported, 18 set a size of a feature that was not imported,
  13 read or suppress something that was not imported, 9 were refused, and 3 set a
  size with no match.
- **Equation-set dimensions are not reference dimensions.** SolidWorks reports a
  dimension as driven both when it is for reference and when an equation sets it. The
  second kind drives the sketch and is imported so.
- **Configurations.** What still differs is mostly what differs in the active
  configuration (the corners of P28, features left out), and sizes of features
  themselves (a flange's length, a pattern's count), which are not carried over yet.

**New gaps it turned up:**

| | Gap | Evidence | Mark |
|---|---|---|---|
| P28 | **Corners where edge flanges meet.** Done, in two parts. (1) Between the pieces of one flange feature on several edges SolidWorks stops both short and ends each bend in line with its flange: `corner` with `open`, its gap over 1.414 and the new relief `flush` gives the same blank, edge for edge (a tray with four 23 mm flanges: 12 lines, the same vertices). (2) Between two flange features SolidWorks runs nothing on: the earlier flange ends where its own edge does. That is the new kind `plain` (PeetCAD's butt corner runs the earlier flange on to the later one's outer face when the later one stands outside its edge); with it Double Door Seal's volume and area are SolidWorks' to the last digit given. **Left:** SolidWorks trims the ends of the bends at a corner of kind (1) on a slant in the folded solid and writes its flat pattern with square ends, so its solid and its blank do not agree; PeetCAD's do, and the volume stays a few hundredths of a percent out. And `plain` is not what SolidWorks does where flanges are at an angle (the vent hoods): the importer keeps it only where it brings the flanges nearer. **Slanted bend ends, done:** SolidWorks' folded part ends each bend at such a corner on the line from the face's corner to the end of its flange (a freeform face), and leaves the face its corner; its flat pattern has square ends. The corner relief `slanted` does the same, and the importer uses it for corners of kind (1): Flush Handle's area is SolidWorks' and its volume 0.002% out where it was 0.058%. 19 parts have such corners; in the others the change is under 0.01% either way and no verdict changes, because something else holds them back. | The tray: volume 0.06% out where it was 2.5%. Double Door Seal: 0 where it was 4.3% | Done; angled flanges' corners to look into |
| P29 | **A cut sketched on the sheet's edge face**, or on a plane that is not a face of the sheet. `sheet_cut` wants the sketch on a flange's top or bottom. | 4 parts (Cableway Cover Bracket LH and RH, Door, Louvre Filter Bracket) | Gap |
| P30 | **A cut across a bend at an angle or with a curve.** Done (`edeaf9c`): any edge of a sheet metal cut can cross a bend. One that crosses on a slant or as a curve folds to a freeform curve, with a ruled wall through the sheet; the blank is unchanged. A round hole on a bend and a triangle across one keep the blank's volume to 2 parts in 100,000 at K = 0.5. | None of the 11 parts is refused for it any more; they are held back by other things now | Done |
| P31 | **The vent hoods and box roofs** (flanges at other angles than 90°). Not an angle convention. Three things, all done: (1) the hood's last flange has a bend deduction of its own in SolidWorks, so `edge_flange` takes `bend` as the base flange does (15 flanges in 12 parts); (2) the side walls' outlines are trapezoids, four lines that the importer took for a rectangle: it now cuts them to shape, on the mirrored side too, and makes the flange wider than its edge where the outline is; (3) a sheet metal cut only went through the flange its sketch is on, and now goes through every flange with a face in the sketch's plane, so holes drawn over two lips cut both. Between flange features where SolidWorks cuts nothing, the importer now also sets PeetCAD's own corner with a `tear` relief. | All 10 vent hoods match SolidWorks: 8 were 0.03 to 0.35% out and the 2 SS2 hoods did not build. The box roofs are unchanged (Box Roof Top and the Ends are 0.1 to 0.18 mm out in size) | Done for the hoods; box roofs open |
| P32 | **A bend deduction that makes the allowance negative** is refused. In the library these are flanges with a smaller deduction of their own, which P31's `bend` on the flange gives them. | SS2 Vent Hood 250mm and 300mm build now; Baffle (VSD) is 0.9% out where it was 11.9%. **One more:** Peaked Angled Roof's base flange is an open profile with shallow bends, and the body's deduction makes one of them -2.9 mm ("Corner 2"); SolidWorks builds it | Open for Peaked Angled Roof |
| P33 | **A base flange outline with two lines in line** is refused ("join them into one line"). The importer joins them: SolidWorks takes a bend of nothing between them. With it, open-profile base flanges are imported as SolidWorks has them (a depth, to both sides for the mid plane), which the importer did not do before | Peaked Angled Roof now reaches P32. Battery Clamp Long, the other open profile, matches; it was 158% out | Done in the importer |
| P34 | **"The sheet metal body came out invalid. This is a bug."** from a sheet metal cut. Two cuts, or a cut and the sheet's outline, that nearly meet (a cut drawn a micron off the edge it runs along, an arc that dips a micron past a line) left a boundary edge of the flat pattern a few microns long, whose wall through the sheet encloses no area. Fixed: the flat pattern drops boundary edges shorter than the tolerance it matches points with, and joins their ends | skinny door Stay Slide matches; Meter Bracket is the right size, 0.03% out; Laptop Bracket's corner cuts are made | Done |
| P37 | **An angled flange with its material inside.** Done: PeetCAD's `material_inside` now means what SolidWorks' does. The flange's outer face goes through the edge on the far face of the sheet, t / tan θ further back than the outer virtual sharp. At 90° nothing changes; angled flanges in existing parts move | The four Weather Shields (30°), Baffle (VSD) (60°), Box Roof Top (88° and 92°) and Roof Front Flange (93°) were out by exactly t / tan θ. Box Roof Top and three Weather Shields match now; the others are held back by other things | Done |
| P38 | **A fillet on an inside corner of a sheet metal face.** Done: `break_corner` on an inside corner adds the sheet that one on an outside corner takes away, as SolidWorks' fillet does there. A piece of the flat pattern can now have sheet added to its outline | MS3 Forklift Tine lifting plate matches; Escutcheon Surround's volume is SolidWorks' (its flat size is not: SolidWorks' DXF is the size of its base alone) | Done |
| P39 | **An edge flange set out from its edge** (SolidWorks' "Offset" in the flange's position). Done: `edge_flange` takes `position_offset`; the sheet runs on flat that far before the bend | Roof Angled matches; Radio Bracket is the right size, 0.1% out where it was 3.5%; Peaked Angled Roof is held back by P32 | Done |
| P40 | **A sheet metal cut through every flange in its way.** Done: `sheet_cut` takes SolidWorks' end conditions, `end` (`sheet`, `blind` with `depth`, `through_all`), `reverse` and `both`. It cuts the flanges parallel to its sketch that lie wholly within its reach, and across the flanges square to it, on across the bend at the end of one it goes past; flanges at other angles are left. Patterns and mirrors of such a cut reach as far. The importer maps SolidWorks' end conditions onto it. It takes through next and up to a surface or a vertex as deep as SolidWorks' walls of the cut reach, and the direction from where those walls are | 18 parts have such cuts. Wall Left, Wall Right, the MS2 Esc Extensions, Battery Clamp Long and Mullion H and V match, with the cut a feature of their own | Done |
| P41 | **A bend radius of 0.** Done: a sharp bend, as SolidWorks allows, is folded round a radius of a tenth of a micron, which moves its setbacks by less than that. It used to come out invalid, so the importer used 0.01 mm, which put the flat pattern 0.01 mm out wherever a bend allowance was used | 9 parts, the Weather Shields among them | Done |
| P35 | **A flange whose length runs up to a vertex, one with its own outline that is not a rectangle, and one wider than its edge.** Wider than its edge is done: a negative `offset_start` or `offset_end` runs an edge flange or a hem past that end of its edge, with the bend staying on the edge. An outline that is not a rectangle is still a gap in PeetCAD; the importer stands in for it with a rectangular flange and a sheet metal cut of everything outside the outline, which does not follow the part. Up to a vertex: the importer uses the length the flange had. | Wider: the Floor parts are 0.005% from SolidWorks' volume where they were 0.06 to 0.09%. Outline: 11 parts have one; the cut is made in 9, bringing Flow Meter Moving Bracket from 0.75% to 0.03%, Laptop Shelf from 5.8% to 0.6% and the Schneider radio brackets from 0.1% to 0.01%. In 2 parts (Radio Bracket, and Flow Meter Fixed Bracket big hole at top) the flange is not where the outline is, so it is left a rectangle. 12 run up to a vertex | Partly done |
| P36 | **A flange with its own relief.** Done: `edge_flange` takes `relief` (`rectangular`, `obround`, `tear`) and `relief_ratio` of its own, the body's without them. Two more fields of the feature, so the model schema is 12; schema 11 files are migrated on open. Where SolidWorks' flange had a tear relief and the body's is rectangular, PeetCAD cut notches SolidWorks doesn't (Top Bracket: two of 1 x 1.1 mm). A relief given as a width and a depth is not there: the importer uses the width for both. | 58 flanges in 46 parts. 12 more parts match SolidWorks (Top Bracket to 8 digits of volume, the Esc Surrounds, ElSteel Geartray, Floor, Wall Inner Vertical, the Schneider radio brackets). 3 flanges have a width and a depth | Done |

**What looking into the near-misses found.** `tools/sw-import/diagnose.js` sets the area
of an imported part's faces against SolidWorks', feature by feature. Of the 31 parts
that were the right size with the wrong volume, most were the importer's mistakes: a
flange's set-back read wrongly when its outline is drawn from the other face of the
sheet, and a hole wizard feature's slots all cut on one face when they are on several.
The rest are P28, P35 and:

- **Sketch blocks** (P14): Castor Heavy Duty's holes are in a block. The third export
  reads blocks and the importer draws them as plain geometry, so it now matches.
- **Back Wall and Wall Middle** lack a pattern's holes on a flange. Its direction was not
  exported; the importer now reads it off where SolidWorks' body has the copies, which
  is how Perforated Sheet (a pattern along a face's normal) came to match too. The two
  are still 0.015% out for something else.

**What the importer gained on 23 October**, besides what P9, P25, P33, P34 and P37 to P41 say:

- **Mitre flanges.** `miter_flange`, with the profile placed so that the sheet is on the
  side of it SolidWorks has (PeetCAD decides that by the corner of the edge the profile
  starts at): Roof matches, Roof Left is the right size, Roof Middle and Roof Right are
  the right size with flat patterns 2 and 11 mm out. A mirrored mitre flange is made on
  the edges across the plane (Wall Left Right Middle: 0.9% where it was 42%).
- **Reference planes where SolidWorks has them.** A plane's transform runs from the plane
  to the model, the other way from a sketch's, and was read like a sketch's. Most planes
  came out right regardless; those turned about an axis did not (Wall Left Right
  Middle's mirror plane was at x = 0, not 500).
- **Flange outlines that reach past the flange's length** (a round top): the flange is
  made long enough to cut the outline from (3PT Rod Guide: the box matches where it was
  15 mm short).
- **A cut of nothing but text** is left out as text (P5), not reported as a failure.
- **Reference planes turned about an axis**: a standard plane turned to it and offset to
  where SolidWorks has it (Double Door Seal and Mullion Stiffener match with them).
- **Extrusions up to a surface or a vertex**, as deep as SolidWorks' faces of them go.
- **A sheet metal cut that removes nothing** because an earlier cut took the sheet
  already is kept, as SolidWorks keeps it, with a warning; a pattern of it can cut
  elsewhere (both Gland Plates match).

**Still the importer's to do**, not PeetCAD's: parts where a feature's sketch found no face
or plane to go on, four of them solid parts (Lifting Lug, Locking Rod Behind Door, Plinth
bolt, Weldment 30x30x2) and the rest sketched on a flange's edge (P29). Plinth bolt's
through-all cut removes nothing in SolidWorks (it points away from the part) and its
whole shaft in PeetCAD. Also sketched bends; sizes of features in configurations and
equations; Louvre Filter Bracket and Cableway Cover Bracket LH and RH, 42% and 15% out,
not looked into.

**Waiting on the exporter:** the definitions of library features (5 parts), corner
reliefs (3 parts) and 3D sketches that place hole wizard holes (HingeMale, HingeFemale).

## What converting four parts by hand showed

Pipe, A3 CT Cover Center, Baffle Duct B (two configurations) and Access Plate 1 were
rebuilt as `peet` scripts from their exports. Each matches SolidWorks in volume, surface
area, bounding box and flat size, to the digits the export gives. The scripts and parts
are in `C:\Users\marti\source\peetcad-imports`.

- **Worked as they are:** base flange with a bend deduction, edge flange (length,
  position, flat size 755.2 against SolidWorks' 755.2), sheet cuts, configurations by
  sketch dimension, sketch relations to projected edges (P16), a pattern whose count is
  an `iif` and whose spacing is an expression (P1). Changing the Access Plate's length
  to 450 moved the holes, the corners and the count correctly.
- **Break corner is a gap (P8).** `fillet` on the sheet's corners gives the right solid
  but it stops being sheet metal. The conversion cuts the corners with a sheet cut tied
  to projected edges: right shape, but four relations per corner where SolidWorks has
  one feature.
- **A value outside a sketch can't read a sketch dimension, and a dimension can't have a
  parameter's name (P4).** `"D3@LPattern2" = ("Length@Sketch1" - 25) / ...` needs
  `Length` as a parameter that the dimension follows, so FabSys has to set the
  parameter, not the dimension.
- **Which plane is which (new, P26).** SolidWorks' Front Plane is the XY plane, which is
  PeetCAD's `top`; its Top Plane is PeetCAD's `front`, facing the other way. The
  conversions keep SolidWorks' coordinates and use the plane that matches them, so a
  feature that names "Front Plane" needs translating.
- **Redundant relations (new, P27).** The cover's first sketch has an equal-length
  relation that the others already imply. SolidWorks accepts it; PeetCAD reports the
  sketch `over_defined` (and builds it).
- **Sketch text confirmed (P5):** Baffle Duct B has a sketch text "B", left out.
- **Face counts differ.** A hole is one round face in SolidWorks and two in PeetCAD, so
  verification (I4) compares volume, area, box and flat size, not face counts.
- **For the exporter:** it doesn't say whether a distance dimension is horizontal,
  vertical or aligned; a relation's outside entity is listed with an id that can match
  a sketch segment's (use `defined` instead); and a break corner's type (round or
  chamfer) isn't among its values. All three had to be worked out from the geometry.

## Parts

168 of the 201 parts are sheet metal; 198 have one body. They average 34 features
(the Door has 94).

### Driving a part

| | Gap | Evidence | Mark |
|---|---|---|---|
| P1 | **No conditionals in expressions.** There is `min`, `max`, `round` and trigonometry, but no comparison and no `if`. | 89 parts have 386 equations; many are `iif(...)`: `"D1@LPattern2" = iif("D2@Sketch1" > 299, 3, 2)` | Done (`689ab7c`): `if`, `iif`, comparisons, `int`; checked on Access Plate 1 |
| P2 | **Suppression can't follow an expression.** It is a flag per configuration. | `"Vent hole" = iif("D2@Sketch1" > 300, "unsuppressed", "suppressed")` | Done (`689ab7c`): `set_suppression_expression`; not yet tried on a template |
| P3 | **No user names for faces and edges.** Faces are named by the feature that made them. FabSys mates and sketches by name. | 97 parts carry 324 named faces and edges | Done (PR #13): `name_face`, `name_edge`, `{"name": ...}` as a selector. Checked on Access Plate 1 with its five SolidWorks names: they survive a change of length and a cut through the named face, and work as a sketch plane and as mate ends. Not yet tried: a named hole (the Door's hinge holes) or a named edge |
| P4 | **Addressing by SolidWorks names.** FabSys sets `Width@Sketch1` and `D1@LPattern2`. A sketch dimension maps to `set_dimension`, a feature dimension to a field of `edit`; the importer has to keep a map from one to the other, or name things so none is needed. | About 5,100 dimensions | Decide |
| P5 | **Sketch text.** No text entity in a sketch. FabSys sets text for etched labels. | `SetSketchText`; features `Etch L`, `Etch R`, `EtchRodGL` | Gap |
| P6 | **Smart components and library features.** No equivalent. FabSys applies a smart feature about once per inserted component. | 43 parts hold smart components, 7 hold library features | Decide |

### Sheet metal features

| | Gap | Evidence | Mark |
|---|---|---|---|
| P7 | **Fold and unfold.** Not a gap: SolidWorks unfolds, cuts flat and folds again, and PeetCAD's sheet metal cut is made in the flat pattern to begin with. The importer treats Unfold and Fold as nothing to do and puts the cuts between them on the face that stays put. With P30 such a cut can have any shape. | 10 parts, all imported past their folds now | Not needed |
| P8 | **Break corner.** Done: `break_corner` rounds (`fillet`) or chamfers corners of a sheet metal body by `size`, each picked by its edge through the sheet. It is made in the flat pattern, so the body stays sheet metal, and it follows the part. Outside corners between two straight sides only; no toolbar command yet (an operation and a panel). The importer uses it where it finds the corner's edge in the part and keeps its cut of the corner's shape otherwise; a fillet on a solid part becomes PeetCAD's `fillet`. **Left:** SolidWorks' Corner Relief feature (3 parts) has no definition in the export. | 55 parts (68 features). Double Door Seal matches (its corners are on a flange at 45°, which the cut could not place); Door Stop's fillets import | Done; Corner Relief needs the exporter |
| P9 | **Patterns and mirrors of flanges.** Done for edge flanges: `mirror`, `linear_pattern` and `circular_pattern` copy one as SolidWorks does. Each copy is the same flange on the edge the motion takes the original's edge to, set back the other way round for a mirror. A copy of a sheet metal cut can land on another flange's face, and copies are built in model order, so a flange on a copied flange follows it. **Left:** mitre flanges, base flange tabs and mirrors of mirrors can't be copied | The ten vent hoods and Top Hat now mirror their flanges with PeetCAD's mirror. Wall Left Right Middle (a mitre flange), Door Lock Bracket Behind Door (a mirror of a mirror) and Gear tray bracket (a tab) still can't | Done for edge flanges |
| P10 | **Edge flange options.** One edge per flange; profile from a sketch exists. Whether every position, offset and relief the templates use has a match is not known. | 142 parts (309 flanges) | Check |
| P11 | **Holes, fillets and cuts on a sheet body.** Extrude and cut-extrude on a sheet body turn it into a plain solid; whether `hole` and `fillet` do too is not known. | Hole wizard in 80 parts, fillets in 35, cuts in over 140 | Check |
| P12 | **Corner relief.** Reliefs are rectangular or tear; round is refused. | 3 parts | Check |
| P13 | **Multi-body sheet metal and mirrored stock.** | 3 parts with 2, 12 and 14 bodies; 1 mirror-stock part | Check |

Base flange (167 parts), mitre flange (5), sketched bend (7), convert to sheet metal (1)
and bend deduction per part have equivalents. No template uses a hem or a jog.

### Sketches

| | Gap | Evidence | Mark |
|---|---|---|---|
| P14 | **Sketch blocks.** None. An importer can explode them into plain geometry, losing the grouping. | 21 parts (52 instances) | Decide |
| P15 | **3D sketches.** Sketches are flat. | 3 parts | Gap |
| P16 | **Relations and dimensions to the model.** Done in `1c5c8f9`: `project` brings a model edge, vertex, reference plane or a face's boundary into a sketch, and `collinear`, `coradial` and `doubled_distance` were added. Every model reference in the four pilot parts is of a kind it takes (68 in all: straight edges, vertices, reference planes). Left to check on the full export: edges it refuses (circles tilted to the sketch, ellipses, freeform) and planes parallel to the sketch. | 2,306 sketches | Done; check limits |
| P25 | **Slots from the hole wizard.** The importer draws them as sketch slots from the hole wizard's own sketch (its line from tip to tip, the slot's length), cut with the hole's end condition, so they follow the sketch. That is what SolidWorks' slot is; `hole` itself still drills round holes only. Where one feature has slots on several faces, which its one sketch does not show, they are still read off SolidWorks' body | 32 slots in 28 parts; most are drawn from the sketch now | Done in the importer |
| P17 | **Splines** have no tangent handles and take no tangent relation. Whether any template sketch has a spline is not known. | | Check |

### One-off solid features

Each in one to three parts; likely STEP fallback (I5) unless something else needs them.

| | Feature | Parts | Mark |
|---|---|---|---|
| P18 | Weldment (structural member) | 1 | Gap |
| P19 | Helix | 1 (suppressed) | Gap |
| P20 | Combine bodies, split, move/copy body | 1 each | Gap |
| P21 | Thin-feature extrude | 1 (Pipe) | Check |
| P22 | Sweep, cut sweep, revolve, revolved cut, draft, chamfer | 1 to 3 each | Check: exist, with the limits in `ROADMAP.md` |

### Configurations

| | Gap | Evidence | Mark |
|---|---|---|---|
| P23 | **Flat-pattern configurations** are SolidWorks' own and need no import; PeetCAD's flat pattern is a view. | About 70 parts | None needed |
| P24 | **What differs between real configurations.** PeetCAD's can differ in suppression, parameters, feature numbers and sketch dimensions. Anything else (a different sketch, material) has no match. | 30 to 40 parts (Battery Bracket sizes, left and right hinge, Wide) | Check |

## Assemblies

29 assemblies: 202 component instances, 32 of them sub-assemblies, nested up to three
deep; 525 mates, 168 of them suppressed. Coincident, concentric, distance and parallel
are 481 of the 525, and components, sub-assemblies, shared definitions, linked parts,
suppression and linear patterns all have equivalents.

| | Gap | Evidence | Mark |
|---|---|---|---|
| A1 | **Mate ends on reference planes.** | Every assembly has reference planes (99); templates carry mating planes by name (`MatePlane`, `MidPlane`, `LockMatePlane`); more than half of the scripts' mates name a plane | Done: `plane` selects standard or named reference planes in mate ends. References follow plane edits and renames, including inside sub-assemblies. Tested through save/reopen; tried on two converted template parts (Baffle Duct B and Access Plate 1 in a sub-assembly): plane-to-plane distance and plane-to-named-face coincident put the component where the arithmetic says |
| A2 | **Scripted mates to parts inside sub-assemblies.** | About 30% of the scripts' mates | Done: `component` accepts a path array of names or ids, resolved at each level. Tested with named faces and planes through two rotated nesting levels. The top-level component moves rigidly; tried on the same two converted parts, with the plate inside a sub-assembly |
| A3 | **Width mate.** | 31 mates in 16 assemblies | Done for parallel pairs: `mate` with `type: "width"`, Centered, Free, Dimension and Percent, paired planar faces/reference planes or a cylindrical/axis tab, nested paths, expressions and mode editing. Tested with geometry changes, bounded dragging and save/reopen. Drafted/nonparallel pairs remain a gap; tried on the same two converted parts: the plate centred on the 3 mm sheet lands at x = -1.5 |
| A4 | **Other mates:** slot, limit angle, symmetric, path, coordinate. | 7, 2, 2, 1 and 1 mates | Gap |
| A5 | **Assemblies have no configurations**, and a component can't choose one of its part's. | 10 assemblies have two (9 are left and right hinge), swapping component configurations and suppressing a different 40 to 50 mates; 24 of 202 instances use a part configuration other than the default | Gap |
| A6 | **A part can't read another part's or the assembly's values.** | 25 of 50 assembly equations set one part's dimension from another's: `Weather Shield width = Door width - 50` | Decide |
| A7 | **Suppressing a component by condition.** Needs P1 and P2 at assembly level. | 7 equations: the third hinge when the door is over 1500 | Gap |
| A8 | **Pattern count and spacing from an expression.** Whether `component_pattern` takes expressions is not known. | 2 equations (Gear Tray Assembly) | Check |
| A9 | **Flexible sub-assemblies.** Sub-assemblies are rigid, by decision (ADR 0009). | The scripts mark sub-assemblies flexible (hinges, door stays) | Decide |
| A10 | **In-context features.** Ruled out by decision (ADR 0009): each becomes an ordinary part feature driven by shared values. | 24 in 9 assemblies; one assembly-level cut; three layout sketches | Decide |
| A11 | **Alignment and flip.** FabSys sets an alignment and a flip on a mate; PeetCAD has one `flip`. Whether it says which side a distance is measured to is not known. | Over half the scripts' mates set a distance; about a quarter flip | Check: on a distance mate between two reference planes, `flip` left the component on the same side (y = -25 both ways); on a coincident mate it turned it over. Which field picks the side of a distance is still to find |
| A12 | **What the templates' own mates join.** Of 1,117 mate ends: 639 flat faces, 165 round faces, 82 round edges, 23 reference planes, 12 sketch lines, 6 straight edges, 3 sketch points; 188 came back empty (half of them on suppressed mates). So the templates' mates are nearly all on faces, which PeetCAD takes; reference planes (A1) matter for the mates FabSys adds, not for importing these. | The full export | Answered; the 93 empty ends on live mates need a look |

## FabSys as the driver

Not yet examined in detail: `ISolidWorksManager` has about 40 methods, and each needs a
`peet` operation or a decision that it is not needed. Known so far:

| | Gap | Mark |
|---|---|---|
| F1 | `ApplySmartFeatures` (see P6) | Decide |
| F2 | `SetSketchText` (see P5) | Gap |
| F3 | `MarkSubAssemblyFlexible` (see A9) | Decide |
| F4 | `AddMate` with planes and sub-assembly parts (see A1, A2) | Done in `peet`; FabSys integration still to do |
| F5 | `CreateSketch`, `CreateLine`, `CreateDimension`, `AddConstraint`, `InsertPlane` in an assembly: sketching against a component's named face from the assembly | Check |
| F6 | `RunMacro` | Decide |
| F7 | The rest of the interface, method by method | Check |

## The templates themselves

Found while reading the manifests; worth fixing whichever CAD they end up in.

- [ ] Five components point outside the Templates folder: one at a file on a desktop
  (`Battery Floor Mount Assembly`), four at job folders (`Limit switch assy`,
  `Meter Movement Assembly Eltech`).
- [ ] 17 component instances have no model loaded (suppressed, so never checked).
- [ ] `Meter Movement Assembly_not used` and `Double Door Seal_old` look retired.
- [ ] The deployed manifests are from exporter version 3 and mark their feature lists
  partial; the FabSys repository is at version 4.

## The first five, in parallel

Chosen on 9 October 2026 from the manifests and the pilot, before the full export. Each
is in a different part of the code, so they can be worked on at once.

| | Gap | Why now | Mostly in |
|---|---|---|---|
| 1 | **P16** (done, `1c5c8f9`) Sketch relations and dimensions to model edges, vertices and reference planes | Most sketches after a part's first use them, so almost no template sketch can be rebuilt without it | `peet-sketch`, the sketch feature in `peet-model` |
| 2 | **P1, P2** (done, `689ab7c`) Conditionals in expressions (`if`, comparisons, `int`), and suppression that follows an expression | 63 `iif` equations in 23 parts, 41 of them suppressing a feature; 7 more in assemblies (A7) | `peet-sketch/src/expr.rs`, regeneration in `peet-model` |
| 3 | **P3** (done, PR #13) User names for faces and edges, usable as selectors | 324 names in 97 parts; FabSys mates and sketches by them | naming in `peet-model`, `peet-ops/src/select.rs` |
| 4 | **A1, A2** (done) Mate ends on reference planes, and on parts inside sub-assemblies from a script | Over half of FabSys's mates name a plane and about 30% reach into a sub-assembly; 64 parts carry 142 planes of their own | `peet-model/src/mate.rs`, `peet-ops/src/mate.rs` |
| 5 | **P25** Slots in `hole` | 32 slots in 28 parts | `peet-model/src/hole.rs` |

Where they meet: 3 and 4 both add kinds of selector (a named face should work as a mate
end), 1 and 3 both extend references to model geometry, and all five add to
`docs/scripting.md` and the skills, so those files will need merging.

Next in line: the remaining width-mate limits (A3, parallel pairs and all four modes
are done) and break corner (P8, 55 parts; first needs a check of whether `fillet` or
`chamfer` on a sheet body already does it).

## A working order

1. I1 and I2: the pilot export and its tally, which turn the **Check** entries into
   gaps or non-issues.
2. P1, P2, P3: without them an imported part can't be driven as FabSys drives it.
3. I3 and I4 on the common sheet metal features, with P7 to P11 as they come up.
4. A1 and A2, which nearly every FabSys mate depends on; then A3 and A5.
5. The **Decide** entries: P4, P6, A6, A9, A10.

## Next, as of 23 October

1. **The exporter**: library features, corner reliefs and the 3D sketches hole wizard holes
   are placed on. That is 10 parts held back by what the export lacks, not by PeetCAD.
2. **P35**, an edge flange with an outline of its own: the importer's stand-in for it is
   in 15 of the 39 matches that lean on one.
3. **P32** for open profiles: a bend allowance of each bend's own on a base flange, as
   SolidWorks' sketched bends have (Peaked Angled Roof).
4. **Copies of mitre flanges, tabs and mirrors of mirrors** (P9's remainder: Wall Left
   Right Middle, Gear tray bracket, Door Lock Bracket Behind Door).
5. **Driving the parts**: sizes of features that equations or configurations set (a
   flange's length, a pattern's spacing), which the importer does not carry over; then
   P4 and P27.
