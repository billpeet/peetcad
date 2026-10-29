# What the SolidWorks template library uses

A count of what is in the 201 parts and 29 assemblies of
`\\nas\Design\Solidworks\Templates`, from the full export of 9 October 2026, set against
what PeetCAD takes. It answers I2 of [solidworks-parity.md](solidworks-parity.md).

Made by `node tools/sw-export/tally.js <folder of exports>`, which prints every count as
JSON. Counts are for each document's active configuration; "in N" is the number of
documents.

## Parts

201 parts: 172 sheet metal, 198 with one body (the others have 2, 12 and 14). 93 have
equations and 51 have more than one configuration besides the flat-pattern ones.

### Features

| Feature | Count (suppressed) | Parts | PeetCAD |
|---|---|---|---|
| Sketch | 1,661 (149) | 200 | `sketch` |
| Cut-extrude | 460 (187) | 160 | `sheet_cut` on sheet metal, `cut` on a solid |
| Edge flange | 325 (54) | 146 | `edge_flange` |
| Base flange | 174 (2) | 171 | `base_flange` |
| Hole wizard | 165 (57) | 80 | `sheet_cut` or `hole`; **slots (32 in 28 parts) have no match (P25)** |
| Reference plane (not a standard one) | 142 (5) | 64 | `plane`, see below |
| Break corner | 68 (25) | 55 | **none (P8)** |
| Linear pattern | 65 (17) | 44 | `linear_pattern` |
| Fillet | 52 (13) | 35 | `fillet`, but **not on a sheet metal body (P11)** |
| Smart component | 47 | 47 | **none (P6)**: the marker only, it has no geometry |
| Mirror | 38 (9) | 30 | `mirror`, but **not of flanges (P9)** |
| Boss-extrude | 22 | 22 | `extrude` |
| Sketched bend | 19 | 7 | `sketched_bend` |
| Library feature | 12 (5) | 7 | **none (P6)** |
| Fold and unfold | 10 pairs | 10 | **none (P7)** |
| Revolve | 6, and 1 cut | 3 | `revolve` |
| Mitre flange | 5 | 5 | `miter_flange` |
| Corner relief | 3 | 3 | `corner`, to check (P12) |
| 3D sketch | 3 | 3 | **none (P15)** |
| Sweep, cut sweep | 2, 1 | 3 | `sweep` |
| One each | | | weldment, helix, combine, split, move/copy body, mirrored stock, convert to sheet metal, imported body, circular pattern, chamfer, draft, thin extrude, reference axis (2) |

### Sheet metal settings

| | Values |
|---|---|
| Thickness | 2 mm in 115 parts, 3 mm in 46, 6 mm in 6; 1, 1.6, 2.5, 4 and 5 mm in one each |
| Bend radius | 0.1 mm in 123 parts, 0.2 in 21, 1 in 10, **0 in 9**, 0.01 in 7 |
| Bend allowance | a bend deduction in 166 parts, a bend allowance in 5, a K-factor in 1. No bend tables |
| Relief | one type and a ratio of 0.5 in nearly all (171 parts) |

Every base flange puts the sheet behind its sketch plane.

### Edge flanges (325)

| | Values | PeetCAD |
|---|---|---|
| Position | material inside 181, material outside 113, bend outside 31 | all three |
| Angle | 90° for 290; 30° to 105° for the other 35 | `angle` |
| Edges in one flange | one for 216, two to four for 109 | `edges` makes one flange each |
| Length measured from | the outer corner 312, the inner corner 11, the bend 2 | the outer corner: the others are converted |
| Length up to a vertex | 12 | **no**: the distance it had is used |
| Its own relief | 79 | the body's only |
| Its own bend allowance | 16 | the body's only |
| Its own bend radius | 2 | `radius` |
| Mirrored | 24 in 13 parts | **no (P9)** |

### Cuts (460)

| | Values |
|---|---|
| End | blind 248, through the next 156, through all 42, up to a surface 5, up to a vertex 4, through both ways 5 |
| Normal to the sheet | 336 |
| Depth linked to the thickness | 101 |
| In both directions | 10 |
| With draft | 2 |

A blind cut shallower than the sheet is an etch mark, which a sheet metal cut can't
make. The importer counts them as it meets them.

### Patterns and mirrors

- **Linear pattern (65):** of hole wizard holes 35, cuts 30, sketch holes 2, a base
  flange 1 and a fillet 1. Eight are grids. The direction is an edge in 47 and was not
  exported in 17 (suppressed). Between 2 and 30 copies. None skips copies or patterns
  faces or bodies.
- **Mirror (38):** of edge flanges 24, cuts 16, holes 10, another mirror 2, a fillet 1
  and a mitre flange 1. Always about a reference plane.

### Reference planes

142 in 64 parts, defined from flat faces (171 selections), straight edges (16), other
planes (12), round faces (10) and a few points and axes. The export has where each
plane is but not its offset or angle (both read as 0: the planes are defined by
constraints, which the API reports differently). So the importer can put a plane where
it is, as an offset of a standard plane, but can't yet make it follow what it was
defined from.

### Sketches (1,661)

| | Values | PeetCAD |
|---|---|---|
| On | a flat face 679, a flange's own plane 695, a standard plane 217, a reference plane 8, a round face 2 | all but the round faces |
| Lines | 5,784 (900 construction) | yes |
| Circles and arcs | 942 and 516 | yes |
| Splines | 9, in one part | yes, without tangency |
| Text | 7, in 7 parts | **no (P5)** |
| Blocks | in 43 sketches of 21 parts | **no (P14)** |
| Fully defined | 979; under-defined 683 | builds either way |

**Relations.** All of these have a match since P16: distance (2,704), vertical (2,252),
horizontal (1,720), coincident (959), at a midpoint (719), converted edge (471),
collinear (466), diameter (423), fixed (410), equal length (368), symmetric (323),
horizontal and vertical between points (597), tangent (261), doubled distance (173),
radius (152), angle (134), coradial (48), parallel (29), perpendicular (12),
concentric (7).

These have none:

| Relation | Count | Parts |
|---|---|---|
| Sketch pattern | 96, and 11 pattern counts | 9 |
| At an intersection | 47 | 20 |
| Offset of an edge, sketch offset | 22 | 5 |
| Pierce | 2 | 2 |
| Along an axis (3D sketches) | 12 | 1 |

**References to the model (1,693).** Straight edges 1,481, vertices 63, flat faces 62,
round edges 31, round faces 26, reference planes 15, torus faces 2, another kind of edge
1. `project` takes the edges, vertices and planes. To check: a relation to a flat face
seen edge-on (62: collinear with it or coincident to it), and to a round face (26).

**Dimensions (3,603).** Linear 2,761, diameter 428, radial 154, angular 134, and 126 of
three other kinds the exporter has no name for yet. 596 have a name of their own, in 114
parts: those are the ones FabSys sets. 308 are driven.

### Equations (396 in 93 parts)

| Kind | Count | PeetCAD |
|---|---|---|
| A dimension by a formula | 246 | an expression |
| A global variable | 86 | a parameter |
| Suppressing a feature by a condition | 41 | `set_suppression_expression` |
| A dimension by a condition | 23 | `iif` |

Only `iif`, `if` and `int` are used. **305 of the 396 are disabled**: only 91 are in
force, which an importer has to respect.

### Configurations

51 parts have more than one. They differ in which features are suppressed (in 38 parts
a sketch, 25 a cut, 14 a bend, 14 a hole, 9 an edge flange, 9 a reference plane) and in
dimension values (a sketch dimension in 25 parts, a feature dimension in 17). All of
that PeetCAD's configurations hold. Four parts differ in something the export does not
show.

### Names and faces

317 named faces in 97 parts (313 flat, 4 round, the Door's hinge holes) and named edges
in 5 parts. Faces in the library: 5,334 flat, 3,714 cylinders, 174 freeform in 24
parts, 122 tori, 80 cones, 53 spheres.

## Assemblies

29 assemblies, 202 component instances (140 at the top, 56 one level down, 6 two levels
down), 17 suppressed, 23 fixed.

| Mate | Live | Suppressed | PeetCAD |
|---|---|---|---|
| Coincident | 169 | 46 | yes |
| Concentric | 87 | 73 | yes |
| Distance | 40 | 28 | yes |
| Width | 25 | 6 | yes (A3) |
| Parallel | 25 | 13 | yes |
| Slot | 5 | 2 | **no (A4)** |
| Limit angle | 2 | | **no (A4)** |
| Symmetric | 2 | | **no (A4)** |
| Coordinate, path | 1 each | | **no (A4)** |

- **Ends:** flat faces 638, round faces 165, round edges 80, reference planes 23,
  sketch lines 12, straight edges 6, sketch points 3. 188 were exported empty, 95 of
  them on suppressed mates.
- **219 of the 525 mates reach into a sub-assembly**, in 9 assemblies.
- **Alignment:** 313 anti-aligned, 202 aligned, 10 closest.
- Ten assemblies have two configurations.
- Also in assemblies: 24 in-context features in 9, 3 layout sketches, 1 cut, 1
  component pattern, 33 composite curves in one.
