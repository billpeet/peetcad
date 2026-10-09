# ADR 0008: Freeform geometry as one more kind of surface

**Status:** accepted (Phase 6)

## Context

[ADR 0006](0006-general-solid-modelling.md) built general solid modelling on planes and
surfaces of revolution and left three things to "the freeform kernel": lofts, curved
faces that cross without a common axis, and other systems' freeform faces. This records
how NURBS were added, and what still isn't there.

The constraint was that nothing analytic should get worse. A cylinder must stay a
cylinder (exact, cheap, recognised by sheet metal and by STEP), and a model without a
freeform face must rebuild exactly as fast as before.

## Decision

- **NURBS are one more variant, not a new foundation.** `Curve3::Nurbs` and
  `Surface::Nurbs` (`peet-kernel/src/nurbs.rs`) sit beside the line, circle, plane and
  cylinder. Everything that already worked on a surface through its `(u, v)`
  parametrisation (lifting loops, tessellation, validation, area and volume by Green's
  theorem, ray casts, transforms, STEP export) gained one arm. Nothing analytic is ever
  converted to NURBS on the way through a boolean; a result is freeform only where an
  input was, or where two curved faces met in a curve that has no closed form.
- **Clamped, non-periodic, shared.** Curves and surfaces have clamped knot vectors, so
  they start and end at their end control points, and a closed section is a clamped
  curve whose ends coincide, with a seam, as a full cylinder has. Their data is behind an
  `Arc`, so `Curve3` and `Surface` stopped being `Copy` but cloning stays cheap; the
  values derived from them (bounds, how sharply they bend, a grid of samples for
  nearest-point starts and ray casts) are computed once per surface and cached with it.
- **Lofts are skinned sections.** `loft` (`peet-kernel/src/loft.rs`) takes closed
  profiles on planes, one after another. Each profile is cut into the same number of
  sides: a polygon's edges are its sides, and a circle is cut where its neighbours'
  corners point, so a rectangle lofts to a circle without a twist. Sides are made
  compatible (one degree, one knot vector) and a surface is interpolated through them.
  Two profiles give ruled sides; more give sides smooth through every profile. Sides
  that come out flat are made planes, so a loft between two rectangles is as analytic as
  an extrusion with draft.
- **Intersections with a freeform face are marched.** Where neither closed form applies
  (`boolean/freeform.rs`), a grid of starting points is relaxed onto both surfaces, each
  curve is traced by predictor and corrector until it closes or leaves a face, and the
  points are fitted with a cubic NURBS curve to a fifth of the modelling tolerance. The
  rest of the boolean (clipping to the faces, splitting, classifying, assembling) is the
  one written for analytic curves and doesn't know the difference. This also covers the
  analytic pairs ADR 0006 refused: cylinders that cross at different radii, a hole
  drilled across a cone.
- **Measurements stay exact where they were.** A freeform face's area and volume
  integrals are done by Gauss quadrature over its parameter region instead of in closed
  form; analytic faces keep their closed forms.
- **In the model, a loft is a feature like a sweep.** `LoftFeature` lists profile
  sketches in order with an operation (new body, add, cut). Sides are named after the
  first profile's curves, so a reference to a side survives edits to the other profiles.
- **Sweeps mitre at corners.** Not NURBS, but it removed the commonest reason to want a
  general path: a corner between two straight pieces of a sweep path is cut along the
  plane that halves it, so frames and pipe runs with square corners sweep directly.
- **Draft on round walls.** A cylinder along the pull direction drafts to a cone, by the
  same re-solve as flat faces.
- **Analytic faces with no closed-form crossing are traced too.** Cylinders and cones
  that cross askew (a hole drilled across a hole of another size), and a plane along a
  cone, are intersected by writing the part of one of them near the crossing exactly as
  a NURBS patch and tracing on that. Both faces stay analytic; only the curve between
  them is freeform. Tracing can't follow faces that only graze each other or cross
  exactly at an edge, so a boolean that fails after tracing is reported as unsupported,
  with advice, instead of as whatever went wrong downstream.
- **Splines in the sketcher** are curves through fit points (natural cubics; closed ones
  periodic), and the fit points are ordinary sketch points: the spline adds no equations
  to the solver. The curve is held in `peet-sketch` as degree, knots and control points,
  and the kernel builds its edge from exactly those, so the sketch and the solid agree.
  A spline edge extrudes to an exact ruled NURBS face and revolves to an exact rational
  one.
- **Sweeps along a spline are skinned.** `sweep` (`peet-kernel/src/sweep.rs`) carries
  the profile along any smooth path with a rotation-minimising frame (no twist about
  the path), places it at stations, and joins the stations with the loft's skinning.
  Stations are added until the surface is within a micron of the true sweep between
  them. The ends are the profile exactly. Holes of the profile are swept through the
  same stations and stitched in as inner faces, with no boolean. Paths of lines and arcs
  alone keep their exact extrude-and-revolve construction.
- **Offsets are fitted, and edges re-solved numerically.** The offset of a NURBS surface
  is not one, so it is fitted to a fifth of the modelling tolerance
  (`nurbs/fit.rs`), carried on a little past its edges so that neighbours that moved
  apart still meet. Shell, offset and draft keep the "same topology, new surfaces"
  method of ADR 0006: where two new surfaces have no closed-form intersection, the old
  edge's points are moved onto both and a curve is fitted through them. Analytic edges
  stay analytic. A wall thicker than a face's tightest curvature is refused before it
  can fold. A freeform body is hollowed by assembling the body, its offset turned inside
  out and the rims, without the boolean the analytic shell uses.
- **Draft of a freeform face is a ruled surface.** The face is replaced by the lines
  through its neutral curve (where it crosses the neutral plane) tilted from the pull
  direction by the draft angle, which is what a mould needs and what makes the wall of
  an extruded spline draftable.
- **Freeform fillets are rolled, and put in by editing the topology.** For an edge the
  analytic tools can't do (a freeform edge, an ellipse, the rim where two round faces
  cross), the ball's centre and its two contact points are solved at stations along the
  edge; the fillet is skinned from exact circular arcs through them, refined until it
  is within half the modelling tolerance of the true surface and tangent to both faces.
  The edge is then replaced by that face and its two neighbours are cut back to the
  contact curves. Booleans can't do this: the fillet touches both faces without crossing
  them. A chamfer is the ruled surface between the two curves a set distance from the
  edge. Runs close on themselves or end on flat faces. Edges the analytic tools can do
  still give exact cylinders, cones and tori.
- **STEP import of freeform faces.** `B_SPLINE_CURVE_WITH_KNOTS`,
  `B_SPLINE_SURFACE_WITH_KNOTS` and their rational forms are read into the same
  variants, with unclamped knot vectors clamped exactly by knot insertion. A closed
  surface never reaches the kernel: each face's surface is trimmed to the stretch the
  face covers, and a face that goes all the way round is cut in two. Swept surfaces
  (`SURFACE_OF_LINEAR_EXTRUSION`, `SURFACE_OF_REVOLUTION`) become the analytic surface
  when they are one, and an exact NURBS surface otherwise.

## Consequences

- A freeform boolean, shell, draft, fillet or sweep costs from a few tenths of a second
  to about a second where an analytic one costs microseconds; tracing, fitting and
  validating many-piece surfaces is where the time goes. Content-keyed regeneration means it
  is paid only when that feature's inputs change.
- Marched curves are approximations to tolerance, so a solid that has been through a
  freeform boolean has edges that lie on their faces to a fraction of the modelling tolerance (about
  2 × 10⁻⁷ mm) rather than exactly, and volumes known to about a part in 10⁵.
  Validation and the STEP writer allow for it.
- **Pinched faces are refused.** A freeform face that comes to a point where its
  surface collapses (the tip of a dome written as a B-spline, a three-sided patch) is
  refused on import: the kernel's nearest-parameter search is ambiguous there, and such
  faces tessellate and measure wrongly. Analytic poles (a sphere's, a cone's apex) are
  unaffected.
- **Not done.** Surfaces that touch along a curve instead of crossing (a loft tangent to
  a cylinder) can't be intersected by tracing and are refused. Fillets that would have
  to mitre into each other at a corner with a freeform edge, and fillets that end on a
  curved face, are refused. A fillet that runs into a distant part of the body is not
  noticed. A closed freeform face with a seam can't be offset (no feature makes one
  today: closed splines extrude to two faces). The sketcher's splines have no tangent
  handles and take no tangent relation, so a spline can't be made to leave a line or a
  plane exactly squarely. Sketches are flat, so a path in space reaches the kernel's
  sweep only from code. Lofts have no guide curves and no end tangency conditions, and
  their profiles must have the same number of sides.
