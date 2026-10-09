# Parity with the SolidWorks template library

What PeetCAD can't yet do that the SolidWorks component library needs. The aim is for
FabSys to drive PeetCAD in place of SolidWorks; parts come first, assemblies after.

**Status, 9 October 2026:** written from names and counts only. Nothing has been
imported yet, so every entry is either a gap read off the code and docs, or something
still to check against real geometry. Update an entry when it is confirmed, fixed or
decided against.

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
- [ ] **I1b. Export the library.** All 201 parts, then the 29 assemblies (the exporter
  has not been run on an assembly). About two minutes for a large part, so two to
  three hours for the parts.
- [ ] **I2. Tally the export.** Feature definition values and sketch relation types
  across the corpus, against what `peet` operations accept. This settles most of the
  **Check** entries below.
- [ ] **I3. Importer.** Replay an export as `peet` operations: sketches, features,
  configurations, equations.
- [ ] **I4. Verification.** Compare each imported part with SolidWorks' own numbers from
  the export: volume, bounding box, face count, flat size, positions of named faces.
- [ ] **I5. Fallback.** Parts the importer can't rebuild come in as STEP
  (`import_step`, `convert_to_sheet`), not drivable. Decide which parts that is
  acceptable for.

## What the pilot showed

From four parts, so these are sightings, not counts for the library.

- **Sketches lean on the model (P16, now a gap).** Dimensions and relations go to model
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

## Parts

168 of the 201 parts are sheet metal; 198 have one body. They average 34 features
(the Door has 94).

### Driving a part

| | Gap | Evidence | Mark |
|---|---|---|---|
| P1 | **No conditionals in expressions.** There is `min`, `max`, `round` and trigonometry, but no comparison and no `if`. | 89 parts have 386 equations; many are `iif(...)`: `"D1@LPattern2" = iif("D2@Sketch1" > 299, 3, 2)` | Gap |
| P2 | **Suppression can't follow an expression.** It is a flag per configuration. | `"Vent hole" = iif("D2@Sketch1" > 300, "unsuppressed", "suppressed")` | Gap |
| P3 | **No user names for faces and edges.** Faces are named by the feature that made them. FabSys mates and sketches by name. | 97 parts carry 324 named faces and edges | Gap |
| P4 | **Addressing by SolidWorks names.** FabSys sets `Width@Sketch1` and `D1@LPattern2`. A sketch dimension maps to `set_dimension`, a feature dimension to a field of `edit`; the importer has to keep a map from one to the other, or name things so none is needed. | About 5,100 dimensions | Decide |
| P5 | **Sketch text.** No text entity in a sketch. FabSys sets text for etched labels. | `SetSketchText`; features `Etch L`, `Etch R`, `EtchRodGL` | Gap |
| P6 | **Smart components and library features.** No equivalent. FabSys applies a smart feature about once per inserted component. | 43 parts hold smart components, 7 hold library features | Decide |

### Sheet metal features

| | Gap | Evidence | Mark |
|---|---|---|---|
| P7 | **Fold and unfold.** No feature pair to unfold, cut, and fold again. A sheet cut crosses a bend only parallel or square to it, and a round hole across a bend is refused. | 10 parts | Gap |
| P8 | **Break corner.** `corner` is about where flanges meet; whether it or `fillet`/`chamfer` can round or clip a sheet's outline corner without losing the sheet body is not known. | 55 parts (68 features) | Check |
| P9 | **Patterns and mirrors of flanges.** Patterns copy extrusions, cuts, sheet cuts, forms, revolves and holes, not flanges, and a sheet metal copy must stay on its original's face. | 44 parts have linear patterns, 26 have mirrors; what they copy comes from the export | Check |
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
| P16 | **Relations and dimensions to the model.** A sketch can't be dimensioned or constrained to a model edge, a vertex or a reference plane, and a face's edges aren't projected into a sketch on it. Collinear, coradial and the other relations listed under the pilot need a match each. | 2,306 sketches; in the pilot, most sketches after the first | Gap |
| P25 | **Slots from the hole wizard.** `hole` drills round holes only. | Seen in the pilot; count from the full export | Gap |
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
| A1 | **A mate can't end on a reference plane.** An end is a face, an edge or a vertex. | Every assembly has reference planes (99); templates carry mating planes by name (`MatePlane`, `MidPlane`, `LockMatePlane`); more than half of the scripts' mates name a plane | Gap |
| A2 | **A script can't mate to a part inside a sub-assembly.** The model and the solve handle it; an operation can't describe it. | About 30% of the scripts' mates | Gap |
| A3 | **Width mate.** | 31 mates in 16 assemblies | Gap |
| A4 | **Other mates:** slot, limit angle, symmetric, path, coordinate. | 7, 2, 2, 1 and 1 mates | Gap |
| A5 | **Assemblies have no configurations**, and a component can't choose one of its part's. | 10 assemblies have two (9 are left and right hinge), swapping component configurations and suppressing a different 40 to 50 mates; 24 of 202 instances use a part configuration other than the default | Gap |
| A6 | **A part can't read another part's or the assembly's values.** | 25 of 50 assembly equations set one part's dimension from another's: `Weather Shield width = Door width - 50` | Decide |
| A7 | **Suppressing a component by condition.** Needs P1 and P2 at assembly level. | 7 equations: the third hinge when the door is over 1500 | Gap |
| A8 | **Pattern count and spacing from an expression.** Whether `component_pattern` takes expressions is not known. | 2 equations (Gear Tray Assembly) | Check |
| A9 | **Flexible sub-assemblies.** Sub-assemblies are rigid, by decision (ADR 0009). | The scripts mark sub-assemblies flexible (hinges, door stays) | Decide |
| A10 | **In-context features.** Ruled out by decision (ADR 0009): each becomes an ordinary part feature driven by shared values. | 24 in 9 assemblies; one assembly-level cut; three layout sketches | Decide |
| A11 | **Alignment and flip.** FabSys sets an alignment and a flip on a mate; PeetCAD has one `flip`. Whether it says which side a distance is measured to is not known. | Over half the scripts' mates set a distance; about a quarter flip | Check |
| A12 | **What the templates' own mates join** (planes, faces, edges), and so how much of A1 an import hits. | From the export's mate entities | Check |

## FabSys as the driver

Not yet examined in detail: `ISolidWorksManager` has about 40 methods, and each needs a
`peet` operation or a decision that it is not needed. Known so far:

| | Gap | Mark |
|---|---|---|
| F1 | `ApplySmartFeatures` (see P6) | Decide |
| F2 | `SetSketchText` (see P5) | Gap |
| F3 | `MarkSubAssemblyFlexible` (see A9) | Decide |
| F4 | `AddMate` with planes and sub-assembly parts (see A1, A2) | Gap |
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
| 1 | **P16** Sketch relations and dimensions to model edges, vertices and reference planes | Most sketches after a part's first use them, so almost no template sketch can be rebuilt without it | `peet-sketch`, the sketch feature in `peet-model` |
| 2 | **P1, P2** Conditionals in expressions (`if`, comparisons, `int`), and suppression that follows an expression | 63 `iif` equations in 23 parts, 41 of them suppressing a feature; 7 more in assemblies (A7) | `peet-sketch/src/expr.rs`, regeneration in `peet-model` |
| 3 | **P3** User names for faces and edges, usable as selectors | 324 names in 97 parts; FabSys mates and sketches by them | naming in `peet-model`, `peet-ops/src/select.rs` |
| 4 | **A1, A2** Mate ends on reference planes, and on parts inside sub-assemblies from a script | Over half of FabSys's mates name a plane and about 30% reach into a sub-assembly; 64 parts carry 142 planes of their own | `peet-model/src/mate.rs`, `peet-ops/src/mate.rs` |
| 5 | **P25** Slots in `hole` | 32 slots in 28 parts | `peet-model/src/hole.rs` |

Where they meet: 3 and 4 both add kinds of selector (a named face should work as a mate
end), 1 and 3 both extend references to model geometry, and all five add to
`docs/scripting.md` and the skills, so those files will need merging.

Next in line: the width mate (A3, 16 assemblies; left out because it is in the same
files as 4) and break corner (P8, 55 parts; first needs a check of whether `fillet` or
`chamfer` on a sheet body already does it).

## A working order

1. I1 and I2: the pilot export and its tally, which turn the **Check** entries into
   gaps or non-issues.
2. P1, P2, P3: without them an imported part can't be driven as FabSys drives it.
3. I3 and I4 on the common sheet metal features, with P7 to P11 as they come up.
4. A1 and A2, which nearly every FabSys mate depends on; then A3 and A5.
5. The **Decide** entries: P4, P6, A6, A9, A10.
