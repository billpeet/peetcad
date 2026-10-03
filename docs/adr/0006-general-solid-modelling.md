# ADR 0006: General solid modelling on an analytic kernel

**Status:** accepted (Phase 6)

## Context

Phase 6 broadens PeetCAD beyond sheet metal: turned parts, holes, fillets and chamfers,
shells, draft, other CAD systems' files. The kernel so far held planes and cylinders,
which is all that extrusions and sheet metal need. The roadmap's plan was "cone, sphere,
torus, then NURBS". This records what was built on the analytic surfaces, how, and what
was left for the freeform kernel.

## Decision

- **Surfaces of revolution, one parametrisation.** Cones, spheres and tori join the
  cylinder as surfaces of revolution about their frame's Z axis
  (`peet-kernel/src/geom.rs`): `u` is the angle about the axis and `v` runs along the
  meridian `(ρ(v), z(v))`. Points, normals, areas and volumes are written once in terms
  of the meridian, so a new kind of surface is a few lines. Edge curves stay lines,
  circles and ellipses.
- **Faces are polygons in parameter space; poles are vertices.** A face on a curved
  surface is the region its loops enclose once they are followed continuously in
  `(u, v)` ("lifted"). Full turns carry a seam edge, as cylinders already did. A sphere's
  poles and a cone's apex, where every `u` is the same point, are always *vertices*, never
  inside an edge or a face: a loop through a pole runs along the pole's line in
  parameter space from the `u` it arrives with to the `u` it leaves with, towards the
  side that keeps the face on its left. One set of helpers (`param_toward`, `param_near`,
  `pole_exit`) does this lifting for the booleans, the tessellation, the measurements and
  the STEP importer, so they agree on what a face is. Booleans cut intersection curves
  at poles and never merge an edge across one.
- **Intersections stay exact, and say when they can't be.** Surfaces of revolution
  about one axis meet in circles, found where their meridians meet (which also gives
  every tangent contact a fillet makes). A plane meets a sphere in a circle, a cone in a
  circle, an ellipse or rulings, a torus in circles when square to its axis or through
  it. Two cylinders of one radius whose axes cross meet in two ellipses (the mitre of two
  fillets). Anything else (a hole drilled across a hole, a plane along a cone, curved
  faces without a common axis) is refused with a message that says what is and isn't
  possible, but only if the two *faces* really cross; surfaces that would meet somewhere
  the faces don't reach are no obstacle.
- **Revolve builds its topology directly** (`revolve.rs`), like extrude: a profile
  vertex sweeps a circle edge, a profile piece a face; a full turn closes each face on
  its own seam, a partial turn adds two caps.
- **Fillets and chamfers are booleans with a tool** (`blend.rs`). The sliver between an
  edge's two faces and the blend surface is built by sweeping its section along the
  edge (extrude for a straight edge, revolve for a round one) and is subtracted on a
  convex edge or added on a concave one. The boolean code already handles faces lying on
  one another and tangent contacts, so the tool's sides vanish and the blend face is
  left. Blended edges that meet at a corner overlap into a mitre; three fillets at a
  square corner get a ball first. Every edge of one feature takes one size.
- **Offset, shell and draft keep the topology and solve the geometry again**
  (`reshape.rs`). Faces get new surfaces (moved along their normals, or tilted about the
  neutral plane); each vertex is found where its faces' new surfaces meet (Newton from
  where it was), each edge is the intersection of its two faces' new surfaces through its
  new vertices. The result is validated: a change the topology can't survive is refused,
  not patched. A shell is the body less its own inward offset, with the opened faces
  left where they are.
- **Holes are revolved profiles** (`peet-model/src/hole.rs`): bore, counterbore or
  countersink and drill point are one half section turned about the hole's axis and cut
  from the body. Standard metric sizes are a table. Threads are cosmetic: a designation
  on the feature.
- **Names** follow ADR 0001. A revolve names its faces by sketch curve, like an
  extrusion. A blend names its faces by the place of its edge in the feature's list. A
  shell's inside wall is named as the face it stands behind plus an `Inner` origin (a
  name is a set of origins). Draft keeps every name. Imported faces are named by index.
- **Imported bodies are features** (`import.rs`): a STEP file's solids are stored in the
  model as they came and take their place in the tree; later features work on them.
  The importer (`peet-io/src/step_import.rs`) reads analytic B-reps, converts units,
  and rebuilds seams and poles to the kernel's conventions; every solid is validated.
- **Mass properties** (`query.rs`): volume and area are exact boundary integrals; the
  centre of gravity and inertia come from a fine mesh corrected to the exact volume.
- **File format.** New feature kinds, surface kinds and face roles are variants at the
  end of their enums, so earlier files decode unchanged. The model and B-rep cache
  schemas go to 4.

## Consequences

- Turned parts, holes, fillets, chamfers, shells and drafts are exact: the Phase 6 exit
  part (a bearing housing) matches its hand calculation to rounding, exports to STEP with
  a torus and cones, and reads back as the same solid.
- **No freeform geometry.** There are no NURBS curves or surfaces, so there is no loft,
  no sweep along a general path, no variable fillet, no draft of curved faces, and STEP
  files with B-spline faces are refused by name. That is the next kernel step, and it
  changes `Curve3` and `Surface` from small `Copy` values into something that owns data.
- **Blends have a shape of tool they can make.** Straight edges between flat faces and
  round edges between faces turned about the edge's axis. A straight edge on a cylinder,
  an elliptical edge, or an edge that ends on a curved face cutting across it is refused.
  Fillets of different radii can't meet at a corner (their mitre is a quartic).
- **Shell and draft need the topology to survive.** A wall thicker than a neighbouring
  fillet's radius, or a draft that pulls a tangent face off its neighbour, is refused.
  Draft works on flat faces.
- **A hole can't cross another curved face off its axis**: a cross-drilled hole, or a
  counterbore that runs into a fillet, is still the unsupported quartic.
- Mass properties of curved bodies are good to about one part in 10⁴ (the mesh), exact
  for flat-faced bodies; volume and area are exact for both.
