# Parity with the SolidWorks template library

What PeetCAD can't yet do that the SolidWorks component library needs. The aim is for
FabSys to drive PeetCAD in place of SolidWorks; parts come first, assemblies after.

**Status, 27 October 2026 (after round 4):** first written on 9 October from names and counts; since then
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

All 201 parts were last run through `tools/sw-import` on 26 October 2026 (`run-m6`, after the
second round of parallel work below; `run-m4` before it), from
the fourth export (`exports\out-all-v4`, format 4: what is in a library feature, a body's
tight box, a corner relief's dimensions) with its STEP files as the reference. The five
"b Tophat" parts are no longer on the NAS and come from the third export. On 25 October
(`run-m3`) and 23 October (`run-r6`) they were run from the third export (which adds
what is inside sketch blocks, the way each dimension measures and the selections of most
suppressed features). The parts, the operations that made them and a report each are in
`C:\Users\marti\source\peetcad-imports\run-m6`. The run of
12 October, before that day's changes, is in `run-base` beside it, and
`scratch\compare.js` sets two runs against each other part by part.

**Shape, in the active configuration**, on 12, 23, 25 and 26 October (from the fourth export, then after round 2):

| Result | 12 Oct | 23 Oct | 25 Oct | 26 Oct | Round 2 |
|---|---|---|---|---|---|
| Matches SolidWorks: volume, bounding box and flat size | 118 | 138 | 182 | 185 | 198 |
| Same size and flat pattern, volume within 5% | 17 | 16 | 12 | 12 | 2 |
| Everything imported, but it differs | 13 | 13 | 5 | 2 | 0 |
| A feature could not be imported | 47 | 30 | 1 | 1 | 1 |
| Nothing built | 6 | 4 | 1 | 1 | 0 |

No part came out worse on 25 October than on 23 October, nor on 26 October than on 25 October.
On the 26th, the tight boxes put Box Roof End Left and Right's size right, and STEP import
heals SolidWorks' tolerant edges, so Weather Shield Door is measured against its STEP body
and matches; Battery Bracket and Elsteel Plinth match too. "Matches" counts the two parts
that match with a feature left out (a weldment's structural member, a mirror of a body).
After round 2 none of the 198 leaves a feature out, and none got worse.

"Matches" is about shape only. On 23 October 113 of the 138 use no stand-in. The other 25 match
because the importer stands in for something: a flange whose length runs up to a vertex
is given the length it had (10), hole wizard slots on several faces are cut to the shape
read off SolidWorks' body (8), fillets on a sheet's corners likewise (5), sketch text is
left out (2); a part can be in more than one of these. Those have the right shape and do
not follow the part when it changes.

**Beyond shape**, on 23 October, after round 3 (`run-m9`) and after round 4 (`run-m10`, both
from the fifth export; `scratch\beyond.js` counts them):

| | 23 Oct | Round 3 | Round 4 | Of (round 4) |
|---|---|---|---|---|
| Sketch relations and dimensions kept | 5,869 (81%) | 8,130 (95%) | 8,364 (97.8%) | 8,554; every one kept in 850 of 950 sketches |
| Names of faces and edges | 311 | | | 320 |
| Equations in force imported | 48 | 82 | 85 | 91 |
| Other configurations that match SolidWorks | 42 | 84 | 86 | 86, in 51 parts |
| FabSys `SetDimension` on template parts that exist | | 60 | 66 | 81 calls (the rest are on assemblies, retired documents and a 3D sketch) |

In 49 of the 51 parts with more than one configuration, every configuration matches; the
other two are Box Roof End Left and Right, whose active configuration is SolidWorks' error.
(The counts of 27 October include hole wizard sketches, now imported with their relations.)

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
| P29 | **A cut sketched on the sheet's edge face**, or on a plane that is not a face of the sheet. Done with P40: `sheet_cut` takes a sketch on any plane and cuts as far as its end condition reaches. | Cableway Cover Bracket LH and RH and Door match; Louvre Filter Bracket is the right size, 0.02% out | Done |
| P30 | **A cut across a bend at an angle or with a curve.** Done (`edeaf9c`): any edge of a sheet metal cut can cross a bend. One that crosses on a slant or as a curve folds to a freeform curve, with a ruled wall through the sheet; the blank is unchanged. A round hole on a bend and a triangle across one keep the blank's volume to 2 parts in 100,000 at K = 0.5. | None of the 11 parts is refused for it any more; they are held back by other things now | Done |
| P31 | **The vent hoods and box roofs** (flanges at other angles than 90°). Not an angle convention. Three things, all done: (1) the hood's last flange has a bend deduction of its own in SolidWorks, so `edge_flange` takes `bend` as the base flange does (15 flanges in 12 parts); (2) the side walls' outlines are trapezoids, four lines that the importer took for a rectangle: it now cuts them to shape, on the mirrored side too, and makes the flange wider than its edge where the outline is; (3) a sheet metal cut only went through the flange its sketch is on, and now goes through every flange with a face in the sketch's plane, so holes drawn over two lips cut both. Between flange features where SolidWorks cuts nothing, the importer now also sets PeetCAD's own corner with a `tear` relief. | All 10 vent hoods match SolidWorks: 8 were 0.03 to 0.35% out and the 2 SS2 hoods did not build. The box roofs are unchanged (Box Roof Top and the Ends are 0.1 to 0.18 mm out in size) | Done for the hoods; box roofs open |
| P32 | **A bend deduction that makes the allowance negative** is refused. For edge flanges, P31's `bend` on the flange gives one its own. For an open-profile base flange, done: `base_flange` takes `bends`, a bend model or null for each bend along the profile, as SolidWorks' sketched bends have their own; the importer gives each bend its SolidWorks sketched bend's | SS2 Vent Hood 250mm and 300mm build; Baffle (VSD) is 0.9% out where it was 11.9%; Peaked Angled Roof builds, the right size, where nothing did (one of its flanges' flat pattern overlaps the base's, which is still to look into) | Done |
| P33 | **A base flange outline with two lines in line** is refused ("join them into one line"). The importer joins them: SolidWorks takes a bend of nothing between them. With it, open-profile base flanges are imported as SolidWorks has them (a depth, to both sides for the mid plane), which the importer did not do before | Peaked Angled Roof now reaches P32. Battery Clamp Long, the other open profile, matches; it was 158% out | Done in the importer |
| P34 | **"The sheet metal body came out invalid. This is a bug."** from a sheet metal cut. Two cuts, or a cut and the sheet's outline, that nearly meet (a cut drawn a micron off the edge it runs along, an arc that dips a micron past a line) left a boundary edge of the flat pattern a few microns long, whose wall through the sheet encloses no area. Fixed: the flat pattern drops boundary edges shorter than the tolerance it matches points with, and joins their ends | skinny door Stay Slide matches; Meter Bracket is the right size, 0.03% out; Laptop Bracket's corner cuts are made | Done |
| P37 | **An angled flange with its material inside.** Done: PeetCAD's `material_inside` now means what SolidWorks' does. The flange's outer face goes through the edge on the far face of the sheet, t / tan θ further back than the outer virtual sharp. At 90° nothing changes; angled flanges in existing parts move | The four Weather Shields (30°), Baffle (VSD) (60°), Box Roof Top (88° and 92°) and Roof Front Flange (93°) were out by exactly t / tan θ. Box Roof Top and three Weather Shields match now; the others are held back by other things | Done |
| P38 | **A fillet on an inside corner of a sheet metal face.** Done: `break_corner` on an inside corner adds the sheet that one on an outside corner takes away, as SolidWorks' fillet does there. A piece of the flat pattern can now have sheet added to its outline | MS3 Forklift Tine lifting plate matches; Escutcheon Surround's volume is SolidWorks' (its flat size is not: SolidWorks' DXF is the size of its base alone) | Done |
| P39 | **An edge flange set out from its edge** (SolidWorks' "Offset" in the flange's position). Done: `edge_flange` takes `position_offset`; the sheet runs on flat that far before the bend | Roof Angled matches; Radio Bracket is the right size, 0.1% out where it was 3.5%; Peaked Angled Roof is held back by P32 | Done |
| P40 | **A sheet metal cut through every flange in its way.** Done: `sheet_cut` takes SolidWorks' end conditions, `end` (`sheet`, `blind` with `depth`, `through_all`), `reverse` and `both`. It cuts the flanges parallel to its sketch that lie wholly within its reach, and across the flanges square to it, on across the bend at the end of one it goes past; flanges at other angles are left. Patterns and mirrors of such a cut reach as far. The importer maps SolidWorks' end conditions onto it. It takes through next and up to a surface or a vertex as deep as SolidWorks' walls of the cut reach, and the direction from where those walls are | 18 parts have such cuts. Wall Left, Wall Right, the MS2 Esc Extensions, Battery Clamp Long and Mullion H and V match, with the cut a feature of their own | Done |
| P41 | **A bend radius of 0.** Done: a sharp bend, as SolidWorks allows, is folded round a radius of a tenth of a micron, which moves its setbacks by less than that. It used to come out invalid, so the importer used 0.01 mm, which put the flat pattern 0.01 mm out wherever a bend allowance was used | 9 parts, the Weather Shields among them | Done |
| P35 | **A flange whose length runs up to a vertex, one with its own outline that is not a rectangle, and one wider than its edge.** Wider than its edge is done: a negative `offset_start` or `offset_end` runs an edge flange past that end of its edge. An outline of its own is done: `edge_flange` takes `outline`, a sketch of the flange's shape on a plane turned about its edge; the flange is made as wide and as long as it reaches and cut to it from its foot out, and follows it. The importer turns a plane about the edge to where SolidWorks' profile sketch is and draws the profile there; where that fails, or the outline is not where the flange is (Top Hat), it falls back to a rectangle and a cut. Up to a vertex is done: `edge_flange` takes `up_to`, a vertex its length runs to, and follows it. | Outline: none of the matching parts relies on the cut stand-in now; Radio Bracket and Escutcheon match with it, Flow Meter Fixed Bracket matches. Up to a vertex: 12 flanges, none given a fixed length now | Done |
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

**What the work of 25 October gained**, done as seven pieces at once, each in a worktree
of its own, and merged; besides what P5, P9, P12, P29 and P35 say:

- **Solid features.** Cuts through a torus and exact boxes round spheres and tori;
  `extrude` and `cut` take `draft` (with `draft_outward`), `flip_side`, `through_next`
  and up to a face; `convert_to_sheet` takes the sheet's thickness; sweeps along more
  kinds of path. Plinth bolt, Locking Rod Behind Door, Stay Bracket, Limit switch base,
  HingeMale, HingeFemale and SS2 DoorStay Rod match.
- **Corners as SolidWorks makes them**, at any angle between flanges, with the reliefs
  and rounds SolidWorks puts there, and fillets on a sheet's corners imported as
  `break_corner` wherever the corner is found.
- **Library features** are imported as the features inside them, and hole wizard holes
  placed on a 3D sketch are placed at its points.
- **SolidWorks' STEP files as the reference** (`--step`): its mass properties are a few
  parts in 100,000 out round freeform faces, and its box can be loose by a hundredth of
  a millimetre; the STEP body measured by PeetCAD is not. A part that nothing could be
  built of is imported from its STEP file; the only such part, ABB Flow Meter, has a
  STEP file PeetCAD finds invalid, so it is still not built.

**Still to do:** see "Next" below.

**The fourth export** (26 October) writes what is in a library feature, a body's tight
box and a corner relief's dimensions. It runs in `will`'s desktop session on
SolidWorks-PC, so he must be logged in.

**STEP import heals tolerant edges** (26 October): SolidWorks writes edges whose curve
strays from its faces by up to a few ten-thousandths of a millimetre (Parasolid's
tolerant edges). `import_step` fits such an edge, up to 0.01 mm out, onto both faces
and warns; further out it still refuses the body.

**What round 2 gained** (26 October), seven pieces of work at once, each in a worktree:

- **Sheet metal cuts** with an end condition cut only the sheet their depth reaches, the
  sketch's own included (Roof Front Flange); one that ends in a bend ends square to the
  sheet where the straight cut leaves its far face (Box Roof Bottom Front and Back); a
  shape over another flange in the sketch's plane cuts that flange alone (Top Hat); a
  copied cut that lands on no flange cuts only those in line with the original's
  (Wall Left Right Middle). An arc seen edge-on projects to the line it covers.
- **Flanges.** Flanges on in-line edges of two faces joined by a bend are mitred round it
  (Laptop Bracket); a flange gets a relief past its edge's end where its face carries on;
  a flange on a wall's side edge runs its bend on through the wall's bend, and an outlined
  flange ends where its outline does (Louvre Filter Bracket, Laptop Shelf).
- **Solids.** `move_body` (Move/Copy Body), `insert_part` (Insert and Mirror Part) and
  `structural_member` (weldments); drafts copied by patterns; a fillet beside a band
  narrower than itself keeps the band's far edge (Lifting Lug); tight boxes round domes
  with a recess in them. M6 Nutsert's screw is placed by SolidWorks' mates, which PeetCAD
  has no body mates for: it is moved where SolidWorks has it, a stand-in.
- **The importer** draws splines (Plinth PFC Long B's lettering), places outlines on the
  face where SolidWorks' profile plane is and carries their dimensions per configuration
  (Top Hat in all eleven), reads which way a pattern goes when its copies tie, and
  measures a flat DXF that SolidWorks drew turned (Peaked Angled Roof's blank was right).
- **STEP bodies SolidWorks stitched with tolerance** are repaired on import (sliver faces
  dropped, vertices up to 1 mm off moved or split, gaps closed with patches), and a body
  that still can't be made a solid is left out with a warning, not the whole file. A
  STEP file with a body left out is not used as the reference volume.
- **Booleans** where a traced curve touches another edge: five tangency cases the
  property tests found, now fixed and tested; 50,000 random cases pass.

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
| P4 | **Addressing by SolidWorks names.** Done (round 3): every sketch and feature dimension has an address `NAME@FEATURE`, as SolidWorks has (`Width@Sketch1`, `D1@LPattern2`; a feature's values are `d1`, `d2`, … or their field names unless named); expressions read them, `set` sets one by its address (per configuration too), `name_dimension` renames one and `dimensions` lists them. The importer gives every dimension SolidWorks' own name. Of the 70 addresses the FabSys scripts set on template parts, 60 exist in the imported parts. **Left:** global variables with spaces in their names become identifiers (`Base Height` is `Base_Height`). | About 5,100 dimensions | Done |
| P5 | **Sketch text.** Done: a sketch takes `text` (a string, a height, a position and an angle), drawn from the font's glyphs as closed profiles a cut or extrusion can use, and `set_text` changes it as FabSys's `SetSketchText` does. **Left:** the export does not say which font, so the default is used | `SetSketchText`; features `Etch L`, `Etch R`, `EtchRodGL`; A3 Geartray and Tophat CB Covers match with their text cut | Done |
| P6 | **Smart components and library features.** No equivalent. FabSys applies a smart feature about once per inserted component. | 43 parts hold smart components, 7 hold library features | Decide |

### Sheet metal features

| | Gap | Evidence | Mark |
|---|---|---|---|
| P7 | **Fold and unfold.** Not a gap: SolidWorks unfolds, cuts flat and folds again, and PeetCAD's sheet metal cut is made in the flat pattern to begin with. The importer treats Unfold and Fold as nothing to do and puts the cuts between them on the face that stays put. With P30 such a cut can have any shape. | 10 parts, all imported past their folds now | Not needed |
| P8 | **Break corner.** Done: `break_corner` rounds (`fillet`) or chamfers corners of a sheet metal body by `size`, each picked by its edge through the sheet. It is made in the flat pattern, so the body stays sheet metal, and it follows the part. Outside corners between two straight sides only; no toolbar command yet (an operation and a panel). The importer uses it where it finds the corner's edge in the part and keeps its cut of the corner's shape otherwise; a fillet on a solid part becomes PeetCAD's `fillet`. **Left:** SolidWorks' Corner Relief feature (3 parts) has no definition in the export. | 55 parts (68 features). Double Door Seal matches (its corners are on a flange at 45°, which the cut could not place); Door Stop's fillets import | Done; Corner Relief needs the exporter |
| P9 | **Patterns and mirrors of flanges.** Done for edge flanges: `mirror`, `linear_pattern` and `circular_pattern` copy one as SolidWorks does. Each copy is the same flange on the edge the motion takes the original's edge to, set back the other way round for a mirror. A copy of a sheet metal cut can land on another flange's face, and copies are built in model order, so a flange on a copied flange follows it. Mitre flanges, base flange tabs (`base_flange` with `tab: true`), break corners and patterns and mirrors themselves are copied too. | The ten vent hoods and Top Hat mirror their flanges with PeetCAD's mirror. Door Lock Bracket Behind Door (a mirror of a mirror) matches; Wall Left Right Middle (a mitre flange) and Gear tray bracket (a tab) are the right size, 0.2% and 1% out | Done |
| P10 | **Edge flange options.** One edge per flange; profile from a sketch exists. Whether every position, offset and relief the templates use has a match is not known. | 142 parts (309 flanges) | Check |
| P11 | **Holes, fillets and cuts on a sheet body.** Extrude and cut-extrude on a sheet body turn it into a plain solid; whether `hole` and `fillet` do too is not known. | Hole wizard in 80 parts, fillets in 35, cuts in over 140 | Check |
| P12 | **Corner relief.** Done: `corner` takes the reliefs `circular` and `square` as well, sized as SolidWorks sizes them. | 3 parts. The exporter now writes corner reliefs; the library has not been exported again with it | Done; export to run |
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

### Importing the template assemblies (first run, 27 October)

`tools/sw-import/import-asm.js` rebuilds the exported assemblies from the parts a run of
`import.js` made (`run-m10`); `summarise-asm.js` counts what it left out and why. Each component
is inserted where SolidWorks has it, then the mates are added one at a time, each end found among
its part's faces and edges by its geometry (a plane, an axis and radius, a circle, a line). A mate
that agrees with SolidWorks moves nothing, so the report says, after each mate, how far the
components are from SolidWorks' placement. The run is in `peetcad-imports\scratch\asm\run`.

Of the 29 assembly exports, 26 have content (3 are documents gone from the NAS). They hold 121
top-level components and sub-assemblies (143 part instances in all), 303 live mates and 114
suppressed ones. First run:

| Result | Assemblies |
|---|---|
| Matches: every component where SolidWorks has it, every mate imported | 4 (Baffle Duct, Dummy, Escutcheon Surround Sealed, Hinge) |
| Placed as SolidWorks has them; a part differs (Box Roof End, SolidWorks' error) | 1 |
| Placed as SolidWorks has them, with components or mates left out | 12 |
| Components placed differently | 9 |

117 of 121 components are inserted (the other 4: two parts outside Templates, one sub-assembly
SolidWorks itself can't load); 204 of 303 live mates and 1 of 114 suppressed mates are imported.

**What it takes, by where the work is** (counts are mates unless said):

PeetCAD:

| | What | Evidence | Parity |
|---|---|---|---|
| AI1 | **Which side a distance mate is measured to.** Done (`flip_dimension`). SolidWorks' "flip dimension"; `move_body`'s mates have it (`reversed` in `mate::solve`), assembly mates pass none. The main cause of components placed differently: Sliding Doorstay's Distance3 puts parts 344 mm off, Perforated Louver Filter's Distance1 222 mm | Most of the 9 assemblies placed differently | A11 |
| AI2 | **A mate added suppressed.** Done (`suppressed`), without solving it: `mate` has no `suppressed`, so adding one of SolidWorks' suppressed mates moves components first | 114 suppressed mates; FabSys unsuppresses them | |
| AI3 | **SolidWorks' alignment.** Done (`align`) (aligned, anti-aligned, closest) for coincident and concentric: Plinth Folded's pipes come out turned end for end (8 mm) | 1 assembly so far | A11 |
| AI4 | **Mates to the assembly's own planes and origin** | 27 in 6 assemblies | A12 |
| AI5 | **Mate kinds**: slot (6), limit angle (2), symmetric (2), coordinate (1), path (1); mate ends on sketch points and lines (4); width mates with a round tab or ends on more than two components (6) | | A4, A3 |
| AI6 | **Assembly configurations**, each with its components' configurations and suppression, and its mates' suppression. Now only the active configuration is imported, and a part used in another configuration than its file's is a copy with that configuration active (a stand-in) | 8 assemblies with two configurations (left and right hinged); 24 instances in a non-default configuration | A5 |
| AI7 | **Flexible sub-assemblies**: four Hinge Assembly instances (Meter Panel Movement, Laptop Shelf Assembly) are opened to another angle than the sub-assembly's own | 2 assemblies | A9 |
| AI8 | **Assembly equations**: reading and setting a component's dimension (`"Height@Sketch1@Door<1>.Part"`), suppressing a component by condition (`"Hinge Assembly-3.Assembly" = IIF(...)`, 5 assemblies), global variables (`SectionID`), reference dimensions in the assembly (`RD1@Annotations`) | 50 equations in 11 assemblies, most of those that set part sizes disabled | A6, A7 |
| AI9 | **Assembly features**: a cut with its sketch (Escutcheon Assembly), layout sketches (3), a component pattern driven by equations (Gear Tray Assembly's LocalLPattern1, count and spacing from Gear tray's height), composite curves (33, Laptop Shelf); in-context features in parts (24 holders in 9 assemblies) | | A8, A10 |
| AI10 | **FabSys reaching into components**: `SetDimension` and suppression in a component (`Height@Sketch1@<part>-1`), `SuppressComponent`, `ResolveComponent`, a component's configuration, a component's sheet metal | Most of what FabSys does that PeetCAD can't take | F |

**After AI1–AI3** (27 October, `agents\M1\run`):

| Result | First run | Now |
|---|---|---|
| Matches | 4 | 6 (Gland Plate Frame and Panel Mount Battery Bracket join) |
| Placed as SolidWorks has them; a part differs | 1 | 1 |
| Placed as SolidWorks has them, with components or mates left out | 12 | 13 |
| Components placed differently | 9 | 6 |
| Live mates imported | 204 of 303 | 231 of 303 |
| Suppressed mates imported | 1 of 114 | 67 of 114 (56 of them solve when unsuppressed) |

- **AI1, done.** In every live distance mate the second end is the distance along the first end's
  outward normal, or against it when SolidWorks flips the dimension. PeetCAD's mates take
  `flip_dimension` (and `move_body`'s use the same field).
- **AI2, done.** `mate` takes `suppressed`: added without solving, solved when unsuppressed. A mate on
  a suppressed component stays live and waits for it, as in SolidWorks.
- **AI3, done.** Mates take `align`: `aligned`, `anti_aligned` or `closest` (`flip` still works).
  SolidWorks' alignment agrees with its own ends in every live mate; "closest" is on 9, none of
  which needs resolving. Plinth Folded's pipes are not an alignment matter: an assembly equation
  makes them 308 mm long where the part has 300 (AI8).
- Width mates: SolidWorks' centred width holds a tab wider than the gap; PeetCAD's now does too.
- The six still placed differently all have a mate PeetCAD can't take yet: the assembly's own
  planes (AI4) and hinges opened to another angle (AI7) in the Door and Escutcheon Door ones and
  Laptop Shelf and Meter Panel Movement, a limit angle and sketch points (AI5), Plinth Folded's
  equation (AI8). Also: a round edge coincident with a plane (the circle lies in the plane), which
  PeetCAD takes as the edge's axis (Rod Doorstay).

The importer: width mates (which pair is the gap and which the tab), mates on components that are
suppressed (19, not failures), sub-assemblies in another configuration than their file's, and the
ends it has only SolidWorks' numbers for (a guessed plane can be the wrong one of two coplanar faces).

The exporter: assembly STEP files and mass properties, to check the assembled geometry and not
only placements; composite curve definitions; what a mate joins when its component isn't loaded.

The templates: Escutcheon Assembly uses `\\NAS\Metroid\Design\Solidworks\Templates\Escutcheon
Surround.SLDASM`, which doesn't exist (SolidWorks lists the component with nothing in it); Battery
Floor Mount Assembly uses `C:\Users\Dave\Desktop\Northern Views Estate SPS\Battery test.SLDPRT`
(twice) and Limit switch assy a job's part under `\\nas\JobSys`.

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

## Round 3 (27 October): driving the parts

Seven pieces of work, each in a worktree; what each changed:

- **Sketch solver.** A relation that the others already imply is accepted, as in
  SolidWorks, where no sketch in the library is over-defined: only conflicting ones are
  refused, and `peet feature` marks each `redundant` or `conflicting` (P27, done). A new
  value for a dimension of a sketch that isn't fully defined moves the sketch as SolidWorks
  does: the whole sketch by one offset if that does, else what the fewest relations hold.
  `midpoint` takes an arc; `patterned` ties a linear sketch pattern's copies to the seed.
- **The importer's relations.** SolidWorks numbers lines and arcs apart, and a line
  overwrote an arc with its id; relations to model edges and vertices the sketch holds a
  hidden copy of are projected from the model, with SolidWorks' ends; a line converted
  from an edge takes the edge's ends; blocks are placed by their instance; texts' points.
- **Dimension names and equations** (P4 above). PeetCAD's expressions read functions
  in any case (`IIF`), an empty global variable is 0 and a disabled one keeps its value,
  as SolidWorks has them; a pattern count from a formula is rounded.
- **Other configurations.** A sketch that projects from a suppressed feature keeps what it
  projected, with a warning, as SolidWorks keeps a dangling relation; simple holes;
  hole wizard sketches keep their dimensions, so holes move per configuration;
  `break_corner` takes faces; copies of a cut that runs off its flange cut as it does.
- **Moved bodies by mates.** `move_body` takes `mates` (coincident, concentric,
  parallel, distance, angle), solved at every rebuild: M6 Nutsert's screw is placed by
  SolidWorks' two mates and follows them.
- **Patterns up to a reference.** `linear_pattern` takes `up_to` (a plane or point,
  with an offset), with SolidWorks' rule: count = floor(span / spacing) + 1, which gives
  all ten of Top Hat's counts. The exporter can't read the reference: SolidWorks-PC runs
  SolidWorks 2018 SP5 (26.5), whose API has no "up to reference" members (they came in
  2019) and no corner relief data. So Top Hat's counts are still set per configuration.
- **The exporter, format 5.** Members newer than its 2018 interop are read by name where
  SolidWorks has them; on 2018 that is each pattern instance's transform. Sketch texts
  carry their font, height and spacing.

The fifth export (`exports\out-all-v5`, format 5, 27 October) changes nothing for the worse
(`run-m9` against `run-m8`). Meter Arm, Meter Bracket, Meter Panel and the Meter Panel
Movement assembly had been edited on the NAS since the fourth (Meter Bracket is 2% bigger)
and still match; Meter Bracket_old and two Meter Movement assemblies are gone from it.
Run against `exports\step-v5`, the fifth export's STEP files.

## Round 4 (27 October): what could be closed

- **Unfold and fold** (`unfold`, `fold`), as SolidWorks has them: between the two the body
  is flat and cuts made there are carried back round the bends; with them suppressed, a
  cut goes straight through the folded part. Gear tray / No Mounting Holes matches, and the
  ten other parts with them still do.
- **Faces picked at a point** keep to the piece there when a later cut splits the face.
  Weather Shield Door / HingeRight's break corner rounded the wrong pieces; it matches.
- **Sketches SolidWorks left unsolved.** A sketch suppressed in the active configuration is
  in the export as SolidWorks last solved it, often long ago; its relations to the model and
  its dimensions are kept, and move it onto the model as SolidWorks does when it comes back.
- **Relations to another sketch's entities** (a `project` of a `sketch`'s entity, which
  follows it), **Offset Entities** (`offset_distance`), pierce points, polygons, and features
  built on suppressed parents (Esc Surround's slot).
- **FabSys names.** Global variables keep SolidWorks' names, spaces included (`"Base
  Height"`), and disabled ones their value; addresses are found in any case, as
  SolidWorks' `Parameter()` finds them; a moved body's mate values are `D1@Distance2`;
  `set` reaches an assembly's mates; the sheet's thickness is `Thickness@Sheet-Metal`; a
  flange's set-backs are named by the end of the edge they measure from.

## Next, after round 4

1. **Assemblies.** What FabSys does that PeetCAD can't take is now almost all on
   assemblies: dimensions and suppression in a component (`Height@Sketch1@<part>-1`),
   `SuppressComponent`, `ResolveComponent`, a component's configuration, sheet metal
   settings of a component's part, and global variables on assemblies. The 29 template
   assemblies are exported and not imported.
2. **3D sketches** (P15): Weldment 30x30x2's structural member is swept along planar
   stand-ins, so FabSys's `D1`–`D3@3DSketch1` drive nothing.
3. **Relations still dropped** (190): stale hidden copies in SolidWorks' own data (sketches
   last solved in another configuration), 6 symmetric relations the exporter writes empty,
   the 32 of Esc Surround R Top and Bottom (an edge flange imported 3.8 mm long), 2 to the
   near or far side of an arc (the export doesn't say which), sketch pattern counts.
4. **Box Roof End Left and Right** and **ABB Flow Meter**: SolidWorks' or the file's errors.
5. **SolidWorks 2018's API** has no "up to reference" for patterns and no corner relief
   data; a newer SolidWorks would let Top Hat's counts follow the part.
