# ADR 0009: Assemblies

**Status:** accepted (Phase 7). Stage 1 (groundwork) is implemented; the rest is the plan.

## Context

Until Phase 7 a document is one part: `Document` wraps one `Model`, the application
holds one document, an operation is applied to it, and `peet --file` names one part. An
assembly places several parts (and other assemblies) relative to each other, holds them
together with mates, and reports on the whole: interference, mass, a bill of materials.

Both Tier 1 platforms have to work the same way. The browser has no file paths, so a
design that depends on a folder of part files next to the assembly doesn't carry over.

## Decision

**An assembly is a second kind of document.** A part's source of truth is its `Model`
(features); an assembly's is its components, mates and explode steps. Both are saved in
the same `.peet` container: an assembly has an assembly section where a part has a model
section, so an older version reports a file it can't use instead of misreading it.

**Parts are embedded first, linked second.** An assembly file holds the parts it uses:
each is a complete part (its model, and its caches) stored in the assembly's file. One
file opens anywhere, headless or in the browser, and one undo history covers the
assembly and its parts. A component can later be *linked* to a part file instead
(stage 2b): the assembly then stores the path, the hash of the part it was built
against, and that part's caches so it still opens when the file is missing. Linked files
are found through the `Host`, as the material tables are, so the browser can answer
from what the user has opened.

**A component is an instance of a definition.** The assembly lists its definitions
(parts, sub-assemblies) once; a component names a definition and has a placement (a
rigid `Frame`), a name, and whether it is fixed. Any number of components share one
definition, and so one rebuild, one tessellation and one GPU mesh.

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

In stage 1 every document of a session is a part with its own file and its own undo
history. How the parts embedded in an assembly's file appear in the session, and
whether a change to one is an undo step of the assembly, is decided in stage 2 with the
assembly document.

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

Left for later stages: the application has no command that opens a second document (a
script does it with `keep`; inserting and opening components will, in stage 2); autosave
and crash recovery cover the current document only; a component's colour will need a
colour per drawn object, since a part's colour is baked into its mesh.

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
