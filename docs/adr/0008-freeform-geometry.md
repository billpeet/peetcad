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
- **STEP import of freeform faces.** `B_SPLINE_CURVE_WITH_KNOTS`,
  `B_SPLINE_SURFACE_WITH_KNOTS` and their rational forms are read into the same
  variants, with unclamped knot vectors clamped exactly by knot insertion. A closed
  surface never reaches the kernel: each face's surface is trimmed to the stretch the
  face covers, and a face that goes all the way round is cut in two. Swept surfaces
  (`SURFACE_OF_LINEAR_EXTRUSION`, `SURFACE_OF_REVOLUTION`) become the analytic surface
  when they are one, and an exact NURBS surface otherwise.

## Consequences

- A freeform boolean costs a few tenths of a second where an analytic one costs
  microseconds; the marching is where the time goes. Content-keyed regeneration means it
  is paid only when that feature's inputs change.
- Marched curves are approximations to tolerance, so a solid that has been through a
  freeform boolean has edges that lie on their faces to about 2 µm rather than exactly.
  Validation and the STEP writer allow for it.
- **Pinched faces are refused.** A freeform face that comes to a point where its
  surface collapses (the tip of a dome written as a B-spline, a three-sided patch) is
  refused on import: the kernel's nearest-parameter search is ambiguous there, and such
  faces tessellate and measure wrongly. Analytic poles (a sphere's, a cone's apex) are
  unaffected.
- **Not done.** Surfaces that touch along a curve instead of crossing (a loft tangent to
  a cylinder) can't be intersected by marching and are refused. Fillets, chamfers,
  shells and offsets are refused on freeform faces and edges: they need offset surfaces
  and rolling-ball blends, which are the next piece of freeform work. Draft doesn't tilt
  freeform faces. The sketcher has no splines yet, so a sweep path is still lines and
  arcs, and a loft's profiles are lines, arcs and circles: freeform curves reach a model
  through lofts, intersections and import, not yet through a sketch. Lofts have no guide
  curves and no end tangency conditions, and their profiles must have the same number
  of sides.
