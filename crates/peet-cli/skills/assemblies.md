---
name: assemblies
description: Assemblies in peet - starting one, inserting parts as components, placing and turning them, replacing and deleting, changing a part of an assembly, sub-assemblies, saving and exporting.
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
- The first component is **fixed** and the others are not. Nothing moves by itself yet
  (there are no mates), so this only records which component is the base. `fix` changes
  it.

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

## Checking, saving, exporting

- `components` lists every component (`status`, `message`) and every part. A component
  with `"status": "warning"` has a part that has no bodies or has features that can't be
  built: open the part and fix it. `status` lists the same under `failures`.
- The run saves the assembly like any document. A part opened with `open_component` is
  not a file: it is saved by storing it back.
- `export` writes every component where it is, as STEP or STL. For a flat pattern DXF,
  open the sheet metal part and export there.

Done when `components` shows every component with `"status": "ok"` and the `min` and
`max` that were asked for.
