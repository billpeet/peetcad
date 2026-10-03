# Scripting and agent access: the plan

**Goal.** An agent (or a script) can drive everything a user can do in PeetCAD, in two
ways that use the same commands:

- **Headless**: nothing is running; a command line run builds or edits a `.peet` file in
  the background and exports from it.
- **Live**: the part is open in a running PeetCAD; the same command line run is applied
  to that session, and the model updates on screen as it goes.

A scripting language for users comes later and sits on top of the same foundation.

## Where things stand

- **The core is already headless.** `peet-model` (model, rebuild engine, undo) and
  `peet-io` (file format, STL, DXF, STEP) have no UI dependency, and the sample parts are
  built entirely in code.
- **The document lives in the UI crate.** `Document` (model + engine + undo + "apply a
  change as one undo step") is in `peet-ui/src/document.rs`, mixed with display meshes.
- **Commands carry no arguments.** `CommandId` means "the user clicked Extrude"; what
  gets extruded comes from the current selection, and the logic sits in `peet-ui/src/app.rs`.
- **Geometry references need a pick.** A flange needs an `EdgeRef`, which today comes
  from a mouse click. The samples and the tests each hand-roll "the face with this normal
  through this point".
- **The command line is only `dump` and `pack`.** The release exe is built as a Windows
  GUI app, so it has no console.
- **There is no IPC or single-instance mechanism.**

## Stages

### 1. Headless session crate (small to medium)

Move `Document` out of `peet-ui` into a new crate with no egui or wgpu dependency. The
display meshes stay behind as a view layered on top. Headless runs and the running app
then use the same object.

### 2. Operation vocabulary (large: this is the real work)

A serialisable set of operations with explicit arguments: add a sketch, draw and
constrain entities, extrude, base flange, edge flange, set a parameter, edit, suppress,
reorder, delete, undo, save, export. Each is applied through one
`apply(op) -> result` entry point, as one undo step. Three things come with it:

- **Selectors** for faces and edges that resolve to the existing persistent references:
  "the end face of Extrude1", "the edge between these two points", "the planar face with
  normal +Z through this point".
- **Queries**: feature tree, parameters, per-feature status and error text, body bounds
  and volume, sketch degrees of freedom, bend table, manufacturing checks.
- **Structured results**: every operation returns the ids and names it created plus any
  rebuild failures, so an agent can correct itself.

### 3. Route the UI through the same operations (medium to large, incremental)

Move the `start_extrude` / `start_edge_flange` / export logic out of `app.rs`, so that
buttons emit the same operations. This keeps all functionality exposed as features are
added, and gives macro recording for free. It can run alongside the other stages.

### 4. Headless command line (small once stage 2 exists)

A console binary, for example:

```sh
peet run script.jsonl --file part.peet
peet query features --file part.peet --json
peet export dxf --file part.peet -o flat.dxf
```

JSON in and out, meaningful exit codes, no window.

### 5. Live attach (medium)

The running app opens a local named pipe and writes a small registry entry (process id,
open file path). The command line checks the registry: if the file is open in an
instance, it sends the operations there; otherwise it runs headless. The app applies
them between frames on the UI thread and repaints, so the model updates live and each
agent edit is a normal undo step. Native only: the web build doesn't get this.

### 6. Scripting language and agent packaging (later)

Rhai or Lua (the roadmap's open question) becomes a thin layer that emits the same
operations, so nothing above is thrown away. An MCP server wrapper and a screenshot
operation for visual checking fit here too.

Stages 1, 2 and 4 give headless builds; stage 5 adds live update.

## Decisions

| Question | Decision |
|---|---|
| Wire format or language first | JSON operations are the foundation; a language is added on top later |
| An agent edits a sketch that is open in the app | Refused with a clear "busy" error at first (the open sketch is a working copy that isn't in the model yet), rather than merged |
| Saving in live mode | Agent edits leave the document unsaved unless the script explicitly saves, as if the user had made them |
| One exe or two | A separate `peet` console binary next to the GUI exe |

## Progress

- [x] Stage 1: headless session crate (`peet-document`: `Document` moved out of `peet-ui`; bodies are tessellated on demand, so a run that only edits and saves does no display work; the GPU mesh conversion stays in `peet-ui`)
- [x] Stage 2: operation vocabulary, selectors, queries (`peet-ops`: a typed `Op` with JSON as one way of writing it; [reference](scripting.md), [ADR 0007](adr/0007-operations.md)). Every command of the application has an operation: `peet_ui::coverage` is checked by the compiler and by a test
- [ ] Stage 3: UI routed through operations. Done so far: `PeetApp::apply_op` applies any operation to the running application, including the application's own commands (views, toggles, windows, sketch editing). Still to do: the application's buttons call the operations
- [ ] Stage 4: headless command line
- [ ] Stage 5: live attach. `PeetApp::apply_op` is the entry point; what is missing is the transport (the pipe and the registry). The "sketch open for editing" rule is already enforced there
- [ ] Stage 6: scripting language, MCP, screenshots
