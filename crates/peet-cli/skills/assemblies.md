---
name: assemblies
description: Assemblies in peet - starting one, inserting parts as components, placing and turning them, mates that hold components together (coincident, concentric, distance, angle, parallel, fasten), dragging a mated component to pose a mechanism, linked parts that follow their own files, interference between components, the bill of materials and the mass of the whole, STEP files with their assembly structure (import and export), replacing and deleting, changing a part of an assembly, sub-assemblies, saving and exporting.
---

# Assemblies

An assembly is a document with **components** where a part has features. A component is
one placed instance of a **part**; the part is copied into the assembly, once, however
many components use it. So an assembly file is complete in itself.

```jsonl
{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket"}
{"op": "insert", "component": "Bracket-1", "at": [0, 120, 0]}
{"op": "insert", "sample": "housing", "name": "Bearing", "at": [60, 40, 30], "rotate": {"axis": "x", "angle": 90}}
{"op": "components"}
```

A file becomes an assembly by its script starting with `{"op": "new", "assembly": true}`:
`peet run build.jsonl --new -f ASM.peet`. Later calls work on it like on any file
(`peet components -f ASM.peet`). `peet status` says `"kind": "assembly"` or `"part"`.

## Inserting

`insert` takes the part from one of:

- `path`: a `.peet` file. A part, or an assembly (it becomes a sub-assembly: one rigid
  component).
- `component`: one more instance of that component's part. Use this, not `path` again,
  after changing the part inside the assembly.
- `part`: a document that is open in the same run (`peet skills core`, several
  documents).
- `sample`: a sample part.

Components are named after their part: `Bracket-1`, `Bracket-2`. Pass `name` to choose.
The reply's `component` has its name, where it is, and `min` and `max`: the box it
occupies in the assembly. Check those against where it should be.

## Placing

`at` is where the **part's origin** goes, in the assembly's units. `rotate` turns the
part about an axis through its origin, from the part's own orientation (not from where
it is now): `{"axis": "z", "angle": 90}`, or `"axis": [x, y, z]`.

```jsonl
{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket"}
{"op": "insert", "component": "Bracket-1", "name": "Lid"}
{"op": "place", "component": "Lid", "at": [0, 0, 40], "rotate": {"axis": "x", "angle": 180}}
{"op": "place", "component": "Lid", "at": [0, 80, 40]}
```

- `place` changes only what it is given: `at` alone keeps the turn, `rotate` alone
  keeps the position.
- To know where a part's origin is relative to its shape, read the part's `bodies`
  (`min`, `max`) before inserting it, or the component's `min` and `max` after.
- The first component is **fixed** and the others are not: mates move the others to
  it. `fix` changes which are.
- On a component that has mates, `place` is a suggestion: the component goes to the
  nearest place its mates allow. The reply's `component` says where it ended up.

## Mates

A mate holds two components together by geometry of their parts. Place the components
roughly first, then add mates: each moves the components as little as it can, so a
rough placement decides which of several possible positions is taken.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}, {"type": "circle", "center": [30, 20], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 25}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Plate"}
{"op": "insert", "part": 2, "name": "Pin", "at": [100, 0, 50]}
{"op": "mate", "type": "concentric", "a": {"component": "Plate", "face": {"at": [35, 20, 3]}}, "b": {"component": "Pin", "face": {"at": [5, 0, 10]}}}
{"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Plate", "face": {"normal": [0, 0, -1]}}, "b": {"component": "Pin", "face": {"normal": [0, 0, -1]}}}
{"op": "mates"}
```

- **An end** is `{"component": NAME, "face": SELECTOR}` (or `"edge"`, or `"vertex"`).
  The selector is in the **part's own coordinates**, as if the part were open by itself,
  not where the component is in the assembly (`peet skills selectors`). To see a part's
  faces, `open_component` it and run `faces`.
- **What an end stands for:** a flat face is a plane; a round face (a hole, a shaft) or
  a round edge is its axis; a straight edge is a line; a vertex is a point.
- **Types:**
  - `coincident`: two planes against each other, two lines in line, two points
    together, or a point or line in a plane.
  - `concentric`: two axes in line (a pin in a hole).
  - `parallel`: planes or lines.
  - `distance` (with `distance`): the gap between two planes, or between points, lines
    and planes.
  - `angle` (with `angle`, in degrees, between 0 and 180): between two planes or lines.
  - `fasten`: `a` and `b` are components alone (`"a": "Plate"`). `b` keeps its place
    relative to `a` as it is now.
- **`flip`**: two flat faces are put **against each other** (their outsides facing).
  `"flip": true` puts them the same way round instead (two side faces flush). If a
  component ends up inside another or upside down, this is what to change:
  `{"op": "edit_mate", "mate": "Coincident1", "flip": true}`.
- **Read the reply.** `mate.status` is `ok` or `failed` with a `message`; `moved` lists
  the components the mate moved, with where they are now (`at`, `min`, `max`);
  `freedom` is how many ways the assembly's components can still move (0: fully held).
  Check `moved` against where the component should be.
- **Finding what is loose.** Each component in `components` has its own `freedom` (0 to
  6): how many ways it can still move, by itself or along with what it is mated to. The
  ones above 0 are the ones that still need a mate, or the one they are mated to does.
  A pin in a hole with its end flush has 1 (it turns); two components fastened together
  and to nothing else each have 6.
- **A mate that fails** is still added, like a feature that can't be built. "It can't
  hold together with the mates above it" means it contradicts earlier mates: change its
  value, its `flip` or its faces (delete it and add another), or delete the one it
  contradicts. A message about a face that "is gone" means the part was changed: delete
  the mate and add it again on the part as it is now.
- `edit_mate` changes a `distance`, an `angle` or `flip`. `rename`, `suppress` and
  `delete` take `mate`. Deleting a component deletes its mates.
- A mate's `distance` or `angle` can be an expression over the assembly's parameters
  (`set_parameter`): the components follow when the parameter changes.

## Posing what the mates leave free

When `freedom` is not 0, something can still turn or slide. `drag` moves it there as the
mouse would: it pulls a point of a component towards a place, and the component goes as
far as its mates let it. Use it to open a lid, swing an arm, or slide a part along.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 25}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-10, -10], "to": [70, 10]}, {"type": "circle", "center": [0, 0], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Post"}
{"op": "insert", "part": 2, "name": "Arm"}
{"op": "mate", "type": "concentric", "a": {"component": "Post", "face": {"at": [5, 0, 10]}}, "b": {"component": "Arm", "face": {"at": [5, 0, 3]}}}
{"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Post", "face": {"normal": [0, 0, -1]}}, "b": {"component": "Arm", "face": {"normal": [0, 0, -1]}}}
{"op": "drag", "component": "Arm", "point": [70, 0, 0], "to": [0, 70, 0]}
```

- `point` is on the component, in its part's own coordinates (its origin if left out).
  `to` is in the assembly's coordinates.
- The component **slides** there if it can, and **turns** only if sliding can't get it
  nearer. What it is mated to comes along.
- `short_by` in the reply is how far the point still is from `to`: the mates held it
  back. No `short_by` means it got there.
- A fixed component can't be dragged: drag another, or `fix` it with `"on": false`.
- Prefer a mate when the position is meant to be kept (an `angle` mate holds a lid open
  at 30 degrees; a drag only leaves it there until something else moves it).

## Changing components

`rename`, `suppress`, `show` and `delete` take `component` instead of `feature`.
`replace` makes a component an instance of another part, where it is. Deleting the last
component of a part removes the part from the assembly.

## Changing a part of an assembly

The parts live in the assembly. To change one:

1. `{"op": "open_component", "component": "Bracket-1"}` opens its part as a document of
   its own and makes it current. Now every part operation works (`features`, `edit`,
   `sketch`, ...).
2. `{"op": "save"}` (no `path`) stores it back in the assembly. Every component of that
   part changes.
3. `{"op": "close"}` returns to the assembly.

```jsonl
{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket"}
{"op": "insert", "component": "Bracket-1", "at": [0, 0, 60]}
{"op": "open_component", "component": "Bracket-1"}
{"op": "set_parameter", "name": "extra", "value": "2mm"}
{"op": "save"}
{"op": "close"}
{"op": "components"}
```

Until step 2 the assembly is unchanged, and `close` refuses to drop the edit. A part
operation sent to the assembly itself fails with "is an assembly": it means step 1 was
skipped.

## Linked parts

By default `insert` copies the part into the assembly. With `"link": true` the component
**follows the part's file** instead: use it when one part file is shared by several
assemblies, or will keep changing.

```sh
peet run plate.jsonl --new -f plate.peet
peet op '{"op": "new", "assembly": true}' '{"op": "insert", "path": "plate.peet", "link": true}' --new -f frame.peet
peet edit feature=Extrude1 depth=9 -f plate.peet     # change the part in its own file
peet components -f frame.peet                        # the assembly has the new plate
```

- Linking needs `path`: a `.peet` file on disk. Give more instances of a linked part
  with `{"op": "insert", "component": "plate-1"}`, as for any part.
- **Links are relative** to the folder the assembly's file is in (`plate.peet`,
  `../parts/plate.peet`), so an assembly and its parts can be moved or copied together
  and still find each other. Keep the folder layout when you move them. Add
  `"absolute": true` for a part that stays in one fixed place whatever happens to the
  assembly (a shared library folder).
- **When the file is read:** every time the assembly is opened (so every `peet` call),
  and by `update_links`. `components` gives each linked part's `link` and
  `link_status`: `current`, `changed` (the file is newer than what the assembly shows:
  run `update_links`) or `missing`. `link` is the path as the assembly's file has it;
  `link_file` is the file that means now.
- **A missing file** is not an error: the assembly opens with the part as it last read
  it, and warns. Put the file back where the link says (or next to the assembly's
  file), or keep the part as it is with `{"op": "unlink", "part": ...}`.
- **To change a linked part**, change its file: with `peet ... -f plate.peet`, or from
  the assembly with `open_component`, which opens that file. `save` there writes the
  file, and the assembly follows (the reply's `updated_in`).
- `{"op": "link", "part": ..., "path": ...}` links a part the assembly already has: to
  the file if it exists (the part becomes what is in the file), else the part is written
  there. `part` is a part's name or id from `components` (use the id when two parts
  have the same name). It takes `"absolute": true` too.
- Linked parts are for files on disk: they are refused in the browser build.

## Interference, bill of materials, mass

Run these when the assembly is put together, before calling it done.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}, {"type": "circle", "center": [30, 20], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "set_material", "material": "Mild steel"}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 25}
{"op": "set_material", "material": "Aluminium 5052-H32"}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Plate"}
{"op": "insert", "part": 2, "name": "Pin", "at": [30, 20, 0]}
{"op": "interference"}
{"op": "bom"}
{"op": "mass"}
```

- **`interference`** lists the pairs of components that occupy the same space, with
  `volume_mm3` and the box (`min`, `max`) where they overlap. `"clear": true` means
  none do. Components that only **touch** (faces against each other, a pin in a hole of
  exactly its size) do not interfere. To fix one: move a component (`place`, `drag`, a
  mate's `flip` or `distance`), or change a part so that it makes room. An entry under
  `unchecked` is a pair that could not be compared: treat it as not known, not as
  clear. `"component": NAME` checks just that component against the others.
- **`bom`** lists each part once with its `quantity`, `material`, `mass_kg` (of one)
  and `total_mass_kg`, and for a sheet metal part its `thickness`, `flat_size` and
  `bends`: what to order and cut. By default the parts inside sub-assemblies are counted
  with the rest; `"level": "top"` lists a sub-assembly as one line.
  `{"op": "export", "path": "bom.csv"}` writes it as a table.
- **`mass`** gives each component's mass and centre of gravity, and the whole's under
  `total`.
- **A part with no material has no mass**, and then neither has the assembly:
  `without_material` (in `mass`) and `without_mass` (in `bom`) name the parts. Give each
  a material in its own document: `open_component`, `set_material` (or, for sheet
  metal, `apply_material`), `save`, `close`. A linked part: `set_material` in its file.

## Checking, saving, exporting

- `components` lists every component (`status`, `message`) and every part. A component
  with `"status": "warning"` has a part that has no bodies or has features that can't be
  built: open the part and fix it. `status` lists the same under `failures`.
- The run saves the assembly like any document. A part opened with `open_component` is
  not a file: it is saved by storing it back.
- `export` writes every component where it is, as STEP or STL. For a flat pattern DXF,
  open the sheet metal part and export there.

## STEP files

A STEP file carries an assembly's structure both ways: its parts, each once, and where
each component is.

```sh
peet export path=gearbox.step -f gearbox.peet
peet op '{"op": "new", "assembly": true}' '{"op": "import_step", "path": "gearbox.step"}' --new -f received.peet
peet components -f received.peet
```

- **`export` of an assembly** writes each part and sub-assembly as a product of its
  own and each component as an occurrence of it, named after the component. The reply
  counts `parts`, `components` and `bodies`. Suppressed components are left out; hidden
  ones are written.
- **`import_step` in an assembly** keeps the file's structure: each of its parts becomes
  a part of the assembly, each of its assemblies a sub-assembly, and what the file's
  top level holds become components here (the reply's `components`), called what the
  file calls them where it names them. A file with no
  assembly in it gives one component for each part.
- **Imported components are fixed** where the file has them, and have no mates: a STEP
  file has none. To move one, `place` it; to let mates move it,
  `{"op": "fix", "component": ..., "on": false}`.
- The parts are imported bodies (one `import_step` feature each): change one with
  `open_component`, as any part. Their face names are the importer's, so pick mate ends
  with `faces` first.
- `import_step` adds to what the assembly has. To bring a file in as **one**
  sub-assembly, import it into a new assembly document and `insert` that.
- `import_step` in a **part** takes the same file as bodies where they are, without
  the structure: use it when the whole is to be one part.
- `warnings` in the reply says what was left out or assumed (a face that can't be
  read, units not given).

Done when `components` shows every component with `"status": "ok"` and the `min` and
`max` that were asked for, `mates` shows every mate `"ok"`, `freedom` is what was
intended (0 for a rigid assembly; more if something is meant to turn or slide), and
`interference` is `"clear": true`.
