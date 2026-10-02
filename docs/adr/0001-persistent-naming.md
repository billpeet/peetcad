# ADR 0001: Persistent naming by feature origin

**Status:** accepted (Phase 3)

## Context

Features refer to geometry made by earlier features: a sketch sits on a face, an
extrusion goes "up to" a face, an axis runs through a hole. Kernel face indices change
whenever anything upstream changes, so a reference stored as "face 7 of body 0" breaks
on the first edit. This is the classic weak spot of FreeCAD, and the roadmap requires it
to be designed in, not patched later.

## Decision

Name faces by *where they came from*, never by index, and resolve references by name
plus a few disambiguators. Implemented in `peet-model/src/naming.rs`.

- **Face names.** Every body face carries a `FaceName`: a sorted set of
  `(feature id, role)` origins. An extrusion names its caps `NearCap` / `FarCap` and each
  wall `Side(sketch entity id)`. Sketch entity ids are never reused, so "the wall from
  that line" survives redimensioning, curves added elsewhere and loops coming out in a
  different order.
- **Booleans propagate names.** The kernel reports, for each result face, the input
  faces it is part of (`peet_kernel::boolean::boolean_traced`; `extrude_traced` does the
  same for extrusions). A result face inherits the union of its sources' names. A face
  merged from two keeps both origins; a face split in two gives two faces with one name.
- **Edges and vertices** are not named separately: an edge is where two named faces
  meet, a vertex is where its faces meet.
- **References** (`FaceRef`, `EdgeRef`, `VertexRef`) store the name plus what told the
  candidates apart when the user picked: the neighbouring faces' names, and a point on
  the geometry. Resolution is deterministic: exact name, else most shared origins; then
  best neighbour match; then nearest to the point.
- **No silent jumps.** If no face shares an origin with the name, the reference is
  *missing* and the feature fails with a message that names what it lost ("the end face
  of Cut-Extrude1 no longer exists").
- References are not "healed" during rebuilds. They only change when the user picks
  again, so a model never edits itself.

## Consequences

- Kernel operations must report provenance. Every future operation (fillet, sheet metal
  flange, hem) has to name its new faces by role and pass old names through.
- Ambiguity is possible only between faces with the same name *and* the same neighbours,
  which is resolved by the stored point. That case is rare and the outcome is stable.
- Tested in `peet-model/tests/parametric.rs`: references survive a feature inserted
  upstream, split faces resolve to the correct half, merged faces are found by either
  origin, edges and vertices follow a resized plate, and lost references fail with a
  message.
