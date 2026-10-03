# ADR 0005: Completing sheet metal on the flat-first layout

**Status:** accepted (Phase 5)

## Context

Phase 5 adds the everyday sheet metal features (hems, sketched bends, jogs, mitre
flanges, corners, forms), patterns and mirrors, and interop (STEP export, DXF import).
[ADR 0004](0004-native-sheet-metal.md) predicted that these "need new piece kinds, but no
new machinery". That mostly held: the layout of flanges and bends in flat coordinates,
built into two solids with the same topology, carries all of it. This records what was
added and where the model had to stretch.

## Decision

- **A flange is a profile.** Everything added on an edge is a chain of bends, each
  followed by a flat (`peet-sheetmetal/src/flange.rs`). An edge flange is one bend and
  one flat; a hem is a bend of 180° or more with a flat coming back over the sheet (a
  rolled hem has no flat); a mitre flange is a sketched profile of any number of bends
  and flats. A bend may now turn through up to just under 360°: nothing in folding
  depended on the old 180° limit, only the setback formulas do, and hems don't use them.
- **Sketched bends split a flange in place** (`split.rs`). The flange keeps its outline
  and gives up everything past the bend line as a trim; the bend and the new flange take
  the same outline with trims of their own. Nothing moves in the flat pattern, so holes
  and outline carry over exactly. Cuts list the pieces they apply to (they used to apply
  to "the first n pieces"), so a split can hand a flange's cuts on to its halves.
  Whatever hangs from the moving side is re-parented and re-placed. A jog is two such
  bends, with the flat between them solved from the offset asked for.
- **Corners are declared, then resolved at build time** (`corner.rs`). A flange added on
  an edge that ends where another flange's edge ends records a corner with a treatment
  (butt, overlap or open; a gap; a relief). The layout stays a description; `build`
  works out the geometry: a relief with edges square to both bend lines (so the bends
  still fold exactly), and each flange's end cut against a plane of the other flange.
  A corner feature changes the treatment later without touching the flanges. Walls
  through the sheet stay square to it, so a "mitre" between flats in one plane (the lips
  of a rim) is exact, while two walls meet in a butt joint.
  A flange whose edge was shortened by a neighbour's trim is carried on to the true
  corner, so two flanges picked one after the other meet as expected. Hems don't make
  corners: they end where their edge does.
- **Forms are holes the build fills** (`form.rs`, `build.rs`). The kernel has planes and
  cylinders, so a dimple, emboss or louver is a plateau with square walls one thickness
  thick: exact, analytic, and a valid solid. Its outline is a hole in its flange's
  material that the thickening step closes with the form's own faces, in both the flat
  and the folded solid, so the two keep the same topology and ids. The flat pattern
  reports forms as marks (outline, centre, a louver's lance), not as cutouts.
- **Patterns and mirrors copy features, not geometry** (`peet-model/src/pattern.rs`).
  Each copy is the copied feature built again from a moved or reflected sketch, so a
  copied sheet metal cut is still made in the flat pattern. Copies are named as the
  pattern feature, and every face a copy makes also carries `FaceRole::Instance(n)`:
  a face name is a set of origins, so this keeps the faces of different copies apart
  without a new naming scheme.
- **Manufacturing checks read the built body** (`checks.rs`): flange lengths, hole to
  bend, hole to edge, hole to hole, small holes, and collisions when folded. Collisions
  between flat pieces are tested as convex prisms along their own face and edge normals,
  from the outlines of the built flat solid, so mitred lips don't raise false alarms.
- **Gauge tables are a tool, not a link** (`gauge.rs`). Applying a gauge copies its
  thickness, radius and bend model into the part. The tables live in the app's settings
  and travel as CSV.
- **File format.** Every addition to the model is a new variant at the end of its enum
  (feature kinds, `AxisRef::Edge`, `FaceRole::Instance`), so files from earlier versions
  decode unchanged. The model and B-rep cache schemas go to 3, so older versions refuse
  new files with a clear message.
- **STEP** (`peet-io/src/step.rs`) writes the kernel's B-rep as it is: the kernel's
  loop and face orientation conventions match AP214/AP242's. **DXF import**
  (`dxf_import.rs`) reads lines, arcs, circles and polylines into a sketch, joining
  coincident ends.

## Consequences

- Unfolding is still exact and free for every new feature. The Phase 5 exit part (a
  chassis with a mitred rim, a hem, patterned holes and louvers, mirrored dimples)
  rebuilds in about 4 ms.
- Because the flat pattern is one planar arrangement, a part whose flat pattern would
  overlap itself can't be modelled (it couldn't be cut from one blank either). Lips
  turned *outwards* round a corner are the usual case; they are reported as an overlap.
- Corner geometry is exact for square corners with square bends, which is nearly every
  box. At other angles the relief's edges can cross a bend at a slant, which the build
  refuses with a message. Circular corner reliefs aren't offered: their arcs would cross
  the bends.
- Forms have sharp corners and vertical walls. Rounded forms need cones and tori, which
  come with the Phase 6 surfaces.
- Patterns and mirrors copy extrusions, cuts, sheet metal cuts and forms. Flanges and
  bends aren't copied, and a sheet metal copy must stay on the face of its original.
- A mitre flange's edges must be on one face.
