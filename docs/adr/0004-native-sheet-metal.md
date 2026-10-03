# ADR 0004: Native sheet metal, flat first

**Status:** accepted (Phase 4)

## Context

The roadmap makes sheet metal the first vertical to get right, from sketch to a DXF for
the laser cutter. Most CAD tools model a solid and then recognise bends to unfold it,
which is fragile. The kernel handles planes and cylinders, and its booleans are tested
for extrude-style geometry. They were not built for gluing many thin pieces along
tangent edges.

## Decision

A sheet metal body carries its own definition, a **layout in flat-pattern coordinates**
(`peet-sheetmetal`). Both the folded solid and the flat pattern are derived from it.

- **Layout.** Pieces are *flanges* (each with a rigid placement in the model) and
  *bends* (strips one bend allowance wide that wrap around a cylinder). Each bend knows
  its parent and child flange. The child's placement is the parent's composed with the
  bend's fold, so the part is a tree unrolled into the plane. Cuts and trims are closed
  areas of tagged lines and arcs. A cut applies only to the pieces that existed when it
  was made.
- **Building** (`build.rs`). Every curve goes into one planar arrangement (the
  sketcher's `find_regions`, opened up as `regions_of_curves`). Each face of the
  arrangement belongs to exactly one piece or to none; a face claimed by two pieces is
  reported as an overlapping flat pattern. Faces of one piece merge, and a bend joins its
  flanges along its lines. Any other contact is a slit, which is how a tear relief works.
  The glued faces are thickened into one B-rep, with vertices made per fan of faces.
- **Folding** keeps the topology and only maps geometry. Flange elements move rigidly.
  In a bend, the top and bottom faces become cylinders, lines along the bend stay lines,
  lines across it become arcs, and walls become radial or end planes. A curve crossing a
  bend at any other angle has no analytic folded form, and the feature fails with a
  message naming it.
- **Same topology, same ids.** The flat and folded solids share face, edge and vertex
  ids. Names, references, picking and selection work the same in either view. The flat
  pattern is exact by construction and comes for free with every rebuild.
- **Features** (`peet-model/src/sheet.rs`): Base Flange (a plate from a closed sketch, or
  a profile of lines with a bend at each corner), Edge Flange (length to the outer
  virtual sharp, angle, position, offsets, reliefs, custom radius) and Sheet Metal Cut
  (a sketch on a flat face mapped into the flat pattern through that face's placement).
  Each one rebuilds the sheet from its layout. Face names follow ADR 0001: a piece's
  top or bottom side, walls from sketch curves, and walls numbered within the feature.
- **Bend math** (`settings.rs`): K-factor, fixed bend allowance or fixed bend
  deduction. The bend table reports BA, BD and the effective K for every bend.

## Consequences

- Unfolding never fails and costs nothing. A K = 0.5 part keeps its volume exactly
  (checked by a property test over random flanges).
- No booleans are used, so sheet metal is independent of the general boolean's
  robustness.
- Cuts across bends are limited to straight edges parallel or square to the bend line.
  Round holes across a bend are refused with a clear message.
- Corners are left open: two flanges whose bend zones meet are reported as an overlap.
  Closed corners, miters and hems (Phase 5) need new piece kinds, but no new machinery.
- A solid feature (Extrude, Cut-Extrude) applied to a sheet body turns it into a plain
  solid, with a warning that the flat pattern is lost.
