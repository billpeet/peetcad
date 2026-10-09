# Configurations: the plan

**Goal.** A part can hold any number of named configurations, as in SolidWorks: versions
of the same part that differ in which features are suppressed, in their dimensions and in
anything else a feature or the part can set. One file is then a whole family of parts,
and each configuration can be shown, measured, checked and exported.

The final state is everything in the stages below. **Stage 1 is built**
([ADR 0009](adr/0009-configurations.md)); it waited for the application's changes to go
through the operations ([scripting plan](scripting-plan.md), stage 3).

## Where things stood before stage 1

- **Rebuilds are keyed by content.** The engine hashes each feature's inputs and
  recomputes only what differs ([ADR 0002](adr/0002-incremental-regeneration.md)). Given
  a different model it rebuilds exactly the features that differ, so switching
  configuration needs nothing new from the engine.
- **A model is cheap to copy.** Features are behind shared pointers, so a second model
  that differs in three features costs three features.
- **Suppression is one flag per feature** (`Feature::suppressed`), with a tree command
  and a `suppress` operation. A feature that uses a suppressed one fails, with a message
  saying to unsuppress it.
- **Every value can be an expression.** Feature values are `Scalar`s and sketch
  dimensions have names (`d1`) and expressions, both over the parameter table. Counts
  (`u32`) and choices (enums) are plain values.
- **A feature's fields are listed once**, in the `feature_args!` tables of
  `peet-ops/src/fields.rs`, with how each is read, written and described.
- **Solved sketches are written back into the model**, so a sketch holds the positions
  of its last solution.
- **References break ties with a point** recorded when the face or edge was picked
  ([ADR 0001](adr/0001-persistent-naming.md)).
- **The model section of the file can only grow by new enum variants so far.** It is
  `postcard`, which is not self-describing; no file has yet needed converting
  ([ADR 0003](adr/0003-native-file-format.md)).
- **The flat pattern is a view**, not a second set of features, so it needs no
  configuration of its own.
- **Several things a configuration would set don't exist yet**: a part's material and
  density (the density is an application setting), body colours, custom properties,
  display states, assemblies and drawings.

## The design

**The model holds the active configuration.** Every part has at least one configuration
(`Default`), and exactly one is active. The features and the parameter table are what
the active configuration says, so the engine, the kernel, the queries, the exports and
the panels read an ordinary part and don't know configurations exist.

**What differs is kept on the side**: for each value that differs between
configurations (this feature is suppressed, this parameter has another value, later this
field has another value), its value in every other configuration. Making another
configuration active swaps them in. Any configuration can be had as an ordinary model
and built without making it active (to export it, or to check it).

**Every edit has a scope**: this configuration, all configurations, or the ones named.
"All" makes the value the same everywhere; the others make it differ. With a single
configuration the scopes are the same thing and the value is stored where it always was.

**Operations carry the scope.** Operations that change something configurable take a
`configurations` field (`"this"`, `"all"`, a name or a list of names; `"this"` if left
out). The application sends the same operations.

This is not the layout first planned (shared values in the model, each configuration's
differences resolved into a second model before a rebuild): every reader of the model
would have had to read the resolved one instead. [ADR 0009](adr/0009-configurations.md)
records the layout, the reasons and the answers to the open questions of stage 1.

## Stages

### 1. Core: suppression and parameters (medium)

What a user gets: named configurations, switching between them, a feature suppressed in
some and not in others, and parameters with a value per configuration. Because any
dimension can be tied to a parameter, this is already a working part family.

- **Model** (`peet-model`): configurations in the `Model` with a name, a comment, the
  features whose suppression differs and the parameters whose expression differs; the
  active configuration; add, copy, rename, delete (never the last one) and activate;
  the resolve step. Renaming or deleting a parameter or a feature keeps the
  configurations in step.
- **Suppressed parents.** A feature that uses a suppressed feature is itself treated as
  suppressed (and shown so, naming the parent) instead of failing. Different features
  suppressed per configuration makes this the normal case.
- **New features** exist in every configuration.
- **File format** (`peet-io`): model schema 5. Version 4 models are read through a
  frozen copy of the old layout and converted: the first migration, with tests against
  version 4 files of the samples kept in the repository. The B-rep and mesh caches hold
  the active configuration. The text form (`dump` / `pack`) includes configurations.
- **Document** (`peet-document`): rebuilds the resolved model; carries solved sketches
  back; undo and the modified mark cover changes to configurations.
- **Operations** (`peet-ops`): `add_configuration` (optionally a copy of another),
  `edit_configuration` (its name and comment), `delete_configuration`, `configuration`
  (make one active), a `configurations` query, and the `configurations` scope on
  `suppress` and `set_parameter`. The `features` and `parameters` queries answer for the
  active configuration.
- **Application** (`peet-ui`): a configurations list beside the feature tree (add, copy,
  rename, delete, double-click to make active); the active one named in the title and
  status bar; the tree's suppress command offers "this configuration" and "all
  configurations"; the parameters window shows a column per configuration.

**Exit criterion:** the sample enclosure panel with three configurations (two
thicknesses with their flange lengths, and one with its cutouts suppressed): each
configuration's flat pattern DXF matches its hand calculation, the part survives save
and reopen with every configuration intact, and a version 4 file opens as a part with
one configuration.

**Status:** implemented. In `peet-model`: `config.rs` (configurations, scopes, the swap
on activation), suppression that carries to what is built on a feature, and the version 4
layout for reading old files. In `peet-io`: model schema 5 and the migration. In
`peet-ops`: the operations, the scope and the query, and the translation of a tool's
change into a scoped operation. In `peet-ui`: the configurations list above the feature
tree, "in all configurations" in the tree's menu, the active configuration in the title,
and a column per configuration in the parameters window.

`crates/peet-ops/tests/configurations.rs` runs the exit criterion: the enclosure with
`Default` (1.5 mm, 25 mm flanges), `Thick` (2 mm, 30 mm) and `Blank` (cutouts
suppressed), each flat pattern DXF read back and checked against the hand calculation
(244.356636 × 194.356636 and 253.047787 × 203.047787 mm), going round the configurations
twice, then saved, reopened and checked again. `crates/peet-io/tests/migration.rs` opens
the four samples as saved with model schema 4. Further tests cover the scopes, the
queries, undo, suppressed parents, a tool's change translated to its scope, and the
list, the tree and the title driven in the application without a window.

Still to do: a hands-on pass in the running app. The list, the tree's menu and the
parameters window's columns have been compile-checked and driven through their actions
in tests, but not exercised by hand. Known limits: a configuration's comment can only be
set by an operation; the list has no drag to reorder; a feature's row doesn't yet show
that its suppression differs between configurations (the `features` query does); in the
parameters window a new parameter is added to every configuration, and the table edits
expressions only (suppression per configuration is edited in the tree, one
configuration at a time or all).

**The open questions** were settled as follows ([ADR 0009](adr/0009-configurations.md)):
solved sketches stay in the model, which holds the active configuration, so switching
re-solves exactly as a parameter edit does; references resolve as they do after a
parameter edit; and making a configuration active is not an undo step, while undoing a
change returns to the configuration it was made in.

### 2. Any dimension (large, mostly in the application)

- A sketch dimension or any feature value can differ per configuration without a
  parameter in between. The model addresses a value by feature and field name (and a
  sketch's dimension by its name), following the field tables in `peet-ops`.
- Every value field in the property panels and the sketch dimension editor offers the
  scope, and marks values that differ between configurations.
- `edit` and `set_dimension` take the `configurations` scope.
- A table editor (SolidWorks' *Modify Configurations*): configurations down, the chosen
  values and features across, edited in place.

**Exit criterion:** a plate with a hole in three configurations that differ in two
sketch dimensions, the hole's diameter and the extrusion's depth, with no parameter in
between: each configuration's volume matches its hand calculation, the part survives
save and reopen, and a schema 5 file opens with its configurations.

**Status:** implemented. In `peet-model`: every numeric value of a feature has a name
(`FeatureKind::slots`, the names the operations use), and a value of a feature or a
driving dimension of a sketch is a `Slot` that can differ, kept and swapped like the
stage 1 tables. In `peet-io`: model schema 6, with schema 5 read through its old layout.
In `peet-ops`: `configurations` on `edit`, `set_dimension` and `set_sketch`; the
`values` of the `configurations` query; a tool's change translated with the scope the
application gives it. In `peet-ui`: "Values change in: all / this configuration" at the
top of the properties panel (for feature values and for the dimensions of the sketch
being edited), and the Configurations Table window.

`crates/peet-ops/tests/configured_values.rs` runs the exit criterion (31 095.22,
47 095.22 and 44 230.09 mm³, round the configurations twice, saved, reopened, and through
the text form), and covers the scopes, the default, expressions that follow each
configuration's parameters, the refusal of non-numeric fields and a tool's change given
its scope. `crates/peet-io/tests/migration.rs` opens schema 5 files.

How it differs from the list above: the scope is one switch for the panel, not a control
beside every value, and values that differ are marked in the table, not in the panels.
The table lists what differs, plus every value of one chosen feature, with
configurations across and values down.

Still to do: a hands-on pass in the running app (the switch, the table and the window
have been driven through their operations in tests, not by hand). Known limits: the
panels don't mark a value that differs; a value that is given the same number in every
configuration stops differing, so "all" then changes it everywhere again; a sketch
dimension changed while the sketch is open takes its scope when the sketch is finished;
an optional value (a flange's own bend radius) is there or not in every configuration;
changing a base flange's bend model drops what was kept for the old one; an expression
typed for another configuration is checked against the active one's parameters.

### 3. Fields that aren't numbers (medium to large)

- Counts (pattern instances), choices (end condition, operation, flange position, hem
  kind, hole standard and size, relief type, bend model) and flags per configuration.
- References per configuration where it makes sense: a sketch's plane, an "up to" face,
  a pattern's direction.
- Sketch relations and dimensions suppressed per configuration, which needs a suppress
  flag on sketch constraints first.
- Parameters that exist only in some configurations.

### 4. Part families (medium)

- **Derived configurations**: a configuration with a parent takes the parent's
  differences and adds its own.
- **Design table**: the table editor's contents as CSV, out and in, so a family can be
  made or changed in a spreadsheet. In the application and CSV only: no Excel files, as
  the gauge tables already do.
- **Export by configuration**: `export` takes a `configuration` (the active one if left
  out) or all of them, writing one file each (a DXF or a STEP file per configuration).
- Per-configuration options: whether features added while another configuration is
  active start suppressed in this one.
- The engine remembers more than one result per feature, so switching back and forth
  between configurations doesn't recompute (the small cache ADR 0002 mentions).

### 5. Properties of the part (each small once its base exists)

Each needs the property itself first; making it differ per configuration is then the
same mechanism as stages 1 to 3.

- Material and density of the part.
- Body and face colours.
- Custom properties (part number, description), for a later bill of materials.
- Display states.

### 6. Assemblies and drawings (with roadmap Phases 7 and 8)

- A component of an assembly names the configuration of its part.
- Assembly configurations: components and mates suppressed, mate values, the
  configuration each component uses.
- A drawing view names the configuration it shows.
- Bills of materials list configurations as separate parts.

## Decisions

| Question | Decision |
|---|---|
| Scope | Everything SolidWorks configurations do is the final state; stage 1 is built first |
| How a configuration is stored | The model holds the active configuration; values that differ are kept on the side for the others and swapped in on activation ([ADR 0009](adr/0009-configurations.md)). Not a per-configuration value in every field |
| File format | Bump the model schema (to 5) and write the first migration; not a separate section, which older versions would drop silently on save |
| When to start | After the application is routed through the operations (scripting stage 3), so the scope of an edit is implemented once |
| Design tables | A table in the application plus CSV; no Excel files |
| Flat patterns | Stay a view of each configuration; no derived flat-pattern configurations |

## Progress

- [x] Before starting: scripting stage 3 (the application's changes are applied as operations)
- [x] Stage 1: core (suppression and parameters per configuration)
- [x] Stage 2: any dimension, the table editor
- [ ] Stage 3: counts, choices, references, sketch relations
- [ ] Stage 4: derived configurations, CSV design tables, export by configuration
- [ ] Stage 5: material, colours, custom properties, display states
- [ ] Stage 6: assemblies and drawings
