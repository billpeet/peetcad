# ADR 0009: Assemblies

**Status:** accepted (Phase 7). Stage 1 (groundwork), most of stage 2 (assembly
documents) and most of stage 3 (mates) are implemented; the rest is the plan.

## Context

Until Phase 7 a document is one part: `Document` wraps one `Model`, the application
holds one document, an operation is applied to it, and `peet --file` names one part. An
assembly places several parts (and other assemblies) relative to each other, holds them
together with mates, and reports on the whole: interference, mass, a bill of materials.

Both Tier 1 platforms have to work the same way. The browser has no file paths, so a
design that depends on a folder of part files next to the assembly doesn't carry over.

## Decision

**An assembly is a second kind of document.** A part's source of truth is its features;
an assembly's is its components, mates and explode steps. Both are a `Model`: an
assembly's has no features and an `Assembly` in their place (`Model::assembly`). So
everything built on a model holds for both without a second copy: the document, undo by
snapshots that share what didn't change, the content hash that says whether a file is
modified, the parameters and units, and the file's model section. A model that has both
features and an assembly is refused as damaged. Operations are for one kind or the
other, and say so when sent to the wrong one. The model section's schema version went up
with it (6), so an older version says the file is from a newer one instead of
misreading it.

**Parts are embedded first, linked second.** An assembly file holds the parts it uses:
each is a complete part (its model, and its caches) stored in the assembly's file. One
file opens anywhere, headless or in the browser. A component can later be *linked* to a part file instead
(stage 2b): the assembly then stores the path, the hash of the part it was built
against, and that part's caches so it still opens when the file is missing. Linked files
are found through the `Host`, as the material tables are, so the browser can answer
from what the user has opened.

**A component is an instance of a definition.** The assembly lists its definitions
(parts, sub-assemblies) once; a component names a definition and has a placement (a
rigid `Frame`), a name, and whether it is fixed. Any number of components share one
definition, and so one rebuild, one tessellation and one GPU mesh. A definition is a
whole `Model`, held by the assembly; inserting a model the assembly already has (the
same content) uses the definition that is there. A definition no component uses is
dropped.

**The engine rebuilds an assembly's parts with engines of its own**, one per definition,
and only when a definition's model is another than last time: moving a component
rebuilds nothing. The result is a list of instances (a body, where it is, the path of
components down to it), not bodies of the assembly's own; the document shows one entry
per instance, the instances of one body sharing a view.

**Sub-assemblies are rigid.** A sub-assembly is solved on its own and placed as one
body in its parent. Flexible sub-assemblies are left for later.

**A part is edited on its own, not in context.** Opening a component for editing shows
the part alone. A part's features can't refer to its neighbours in the assembly.
In-context references are where most of the complexity of assemblies in other systems
comes from (circular updates, references that dangle when a component is replaced), and
nothing in the phase needs them.

**Mates refer to geometry by instance and persistent reference.** A mate's ends are a
component plus the `FaceRef` / `EdgeRef` / `VertexRef` a pick or a selector gives
([ADR 0001](0001-persistent-naming.md)), so mates follow the geometry through edits to
the part, and a mate whose reference is lost is flagged as a failed feature is. The
kinds are those of the roadmap: coincident, concentric, distance, angle, parallel, and
*fasten*, which makes components one rigid group.

**Mates are solved by the sketch solver's numerical core.** The core (damped least
squares with minimum-norm steps, the dogleg fallback, the sparse Cholesky, row dropping
for redundant equations) moves out of `peet-sketch` into `peet-solve`, behind a trait
for a system of equations over a flat array of values. The sketcher's equations are one
implementation, unchanged in behaviour; the assembly's are another, with six unknowns
per free component (a translation and a rotation vector about the current orientation)
and residuals in millimetres or dimensionless, as in the sketcher. Dragging a component
uses the same two-phase drag, so everything else moves as little as it can.

**A part says what it is made of and what colour it is.** The bill of materials and the
mass of an assembly need a material and a density per part, and components need
colours. These are properties of the part's `Model` (not of the application's settings,
where the density was until now), with the application's material tables supplying the
values when a material is chosen from them.

**A session holds several documents.** The application and the command line work on a
`Session` (`peet-document`): the open documents, one of them current. An operation is
applied to the current document unless it names another (`"document"`), so every
existing script still means what it meant. A session dereferences to its current
document, so code written for one document works on a session unchanged. Opening a
component for editing will make its part current.

**A part of an assembly is edited as a working copy.** `open_component` opens the
definition's model as a document of the session, marked as embedded in its assembly.
It has its own undo history while it is open. `save` on it stores its model back as the
definition's, which is one undo step of the assembly ("Edit Bracket") and reaches every
instance; until then the assembly is unchanged. This is how a sketch is edited too: a
working copy, written back when it is finished. Closing the assembly closes the parts
opened from it, and refuses while one has changes that were not stored.

**Operations, skills and the reference grow with it**, as `AGENTS.md` requires: every
assembly command is an operation, and a new `assemblies` skill covers them.

## Stages

1. **Groundwork.** Material, density and colour on the part; `peet-solve`; the session.
2. **Assembly document and instancing.** The assembly model, its file section, the
   tree, drawing with shared meshes, picking by instance, operations and the skill, STEP
   import that keeps the structure, STEP export of an assembly. Then linked parts.
3. **Mates.** The equations, the solve, dragging, degrees of freedom and conflicts.
4. **Interference, bill of materials, mass.** Bounding boxes first, then the kernel's
   intersection; quantities, material, mass and flat sizes; CSV.
5. **Visibility, colours, exploded views.** Explode steps are stored per assembly and
   don't affect mates.
6. **Large assemblies.** GPU instancing, culling, loading from the mesh cache on demand,
   coarser meshes at a distance.

## What stage 1 built

- `peet_model::Material` and `Model::material`, `Model::color` (model schema 5; older
  files are read as `ModelV4`). Densities in the material tables. Operations
  `set_material` and `set_color`; `mass` gives `mass_kg`. The mass properties window
  sets the part's material and colour.
- `peet-solve`: `Problem`, `Options`, `Space` and the sparse Cholesky, generic over an
  `Equation` trait. `peet-sketch` implements it for its equations; its tests pass
  unchanged and its drag benchmark is the same to within noise (bracket 0.009 ms, 180-line
  truss 0.29 ms, before and after).
- `peet_document::Session`, `peet_ops::apply_session`, the operations `documents`,
  `switch`, `close` and `keep` on `new`, `open`, `open_sample`. The command line and the
  application hold a session; the application shows a tab per document when more than
  one is open.

## What stage 2 built so far

- `peet_model::Assembly` (definitions, components), `Model::assembly`, model schema 6
  (version 5 files are read as `ModelV5`), instances in the `Evaluation`.
- `Document::placed` (where each shown body is, and whose), `Document::embedded`.
- Operations: `new` with `assembly`, `insert` (from a file, a sample, an open document
  or another component), `place`, `fix`, `replace`, `rename` / `suppress` / `show` /
  `delete` with `component`, `open_component`, `components`; `status` and `failures`
  for assemblies; STEP and STL export of every component where it is. The application's
  own changes to an assembly are translated to these, like its changes to a part.
- The `assemblies` skill and the reference.
- In the application: New Assembly, Insert Part and Edit Part; a component tree and a
  component's properties (name, position, quarter turns, fixed); components drawn
  where they are, the instances of a part sharing a GPU mesh; a click picks a component.

Left of stage 2: STEP import that keeps a file's assembly structure (it still makes one
body per occurrence, in a part) and STEP export with structure (each component is
written as a solid of its own, without the product tree); linked parts; dragging a
component in the view (its position is typed, for now). Also open: mass properties
and measurements in an assembly (stage 4), an assembly's caches in its file (it is
rebuilt when opened), selections on a component's faces (stage 3 needs them for mates),
autosave of documents other than the current one, and a colour per component (a part's
colour is baked into its mesh, so it needs a colour per drawn object).

## What stage 3 built so far

- `peet_model::Mate`: a kind (coincident, concentric, parallel, distance, angle,
  fasten), two ends, `flip`. An end is the path of components down to a part and a
  `FaceRef`, `EdgeRef` or `VertexRef` in it. A flat face stands for its plane, a round
  face or a round edge for its axis, a straight edge for a line, a vertex (or a sphere)
  for a point.
- The solve (`peet-model/src/mate.rs`, where the formulation is written up). Each
  component that is not fixed has six unknowns: its origin, and a rotation vector from
  the orientation it had when the solve started, scaled by the component's size so that
  the Jacobian's entries are of order one. Every mate is made of four kinds of residual
  (two directions at an angle; a point at a distance along a direction of the other
  end; a distance between points; a distance from a line), with analytic gradients,
  checked against finite differences. `peet-solve` solves them with minimum norm steps,
  so the components move as little as they can.
- **Where a component is, is the suggestion.** The mates are solved whenever the
  assembly is rebuilt, from where the components are, and the placements they come to
  are stored in the components (as a sketch's solved points are stored in the sketch).
  So placing a mated component is a request, and there is no separate "solved" state to
  keep in step.
- **Groups.** Components joined by mates are solved as a group of their own; a fixed
  component joins nothing. A group whose mates already hold is not touched, so a solved
  assembly is bit for bit the same after any number of rebuilds (which matters for
  "is the file modified" and for undo), and a mate added in one place moves nothing in
  another.
- **Which way round** two planes go (against each other, or the same way with `flip`)
  can't be told apart by equations that are well behaved at the solution, so a
  component that is the wrong way round is turned over before the solve.
- **Conflicts.** If a group's mates can't all hold, they are added one at a time in
  their order; each that can't be satisfied with those before it is flagged and left
  out, and the rest hold. A mate that can't be set up (a face that is gone, two ends on
  one component, a flat face for a concentric mate) is flagged the same way, with what
  to do. Redundant mates that agree are not flagged.
- **Freedom**: six for each component that is not fixed, less the rank of the
  Jacobian. It is reported for the assembly as a whole.
- The solver's structure and the freedom are kept between solves of the same mates, so
  a drag costs only the numeric solve: 50 components held by 147 mates take about 3 ms
  per step in a release build, against the 4 ms budget
  (`crates/peet-model/tests/mates.rs`).
- Operations `mate`, `edit_mate`, `mates`, and `rename`, `suppress`, `delete` with a
  `mate`. A script describes an end with the selectors a part's operations take, in the
  part's own coordinates. Replies list the components an operation moved. In the
  application: a click in an assembly picks a face, an edge or a corner (and with it
  its component), six mate commands that work on two picked ends, the mates in the
  tree, and a mate's properties.

- **Dragging.** A drag pulls a point of a component towards a place (`peet_model::Drag`,
  the `drag` operation, the left mouse button on a component in the view). The
  component's group is moved in steps, as the sketcher drags: the least squares answer
  to the mates' equations, linearised, with weak equations that pull the point; then
  the mates solved exactly. A pull is taken a reach (10 mm) at a time, so that a far or
  unreachable place doesn't bend the mates in the step it asks for, and a step that
  overshoots round a curve is halved. It is done first without letting the dragged
  component turn, then letting it: a part that can slide to the place slides, a hinged
  one swings. In the view the point is the one under the pointer when the button went
  down, and it is pulled in the plane through it that faces the viewer; the steps of
  one drag are one undo step. Pulling a stack of 50 plates held by 147 mates by a
  corner takes about 3 ms a step in a release build (7 ms for the first, which sets the
  solver up).

Left of stage 3: freedom per component, to show
which ones are still loose; a script can't describe a face of a part inside a
sub-assembly (the application can pick one, and the model and the solve handle it);
tangent and other mates beyond the roadmap's list. Mates were added to model schema 6,
which no released version writes, without another version number.

## Consequences

- An assembly file grows with the parts in it, and a part used by two assemblies is two
  copies until linking exists.
- A part can't be shaped by its neighbours (a hole placed from the mating part). It has
  to be dimensioned by shared parameter values kept in step by hand.
- A mechanism inside a sub-assembly doesn't move in its parent.
- Moving the solver's core is a refactoring of the most heavily tested numerical code in
  the project; its tests and benchmark must pass unchanged, and the drag benchmark must
  not slow down.
- Operations that mean "the document" now mean "the current document", and the hosts
  (the application, the command line, tests) hold a session.
