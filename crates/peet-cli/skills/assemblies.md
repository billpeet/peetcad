---
name: assemblies
description: Assemblies in peet - starting one, inserting parts as components, placing and turning them, mates that hold components together (coincident, concentric, distance, angle, parallel, fasten; a point kept on a path), dragging a mated component to pose a mechanism, component patterns (rows of copies, copies round an axis, counts and spacings from expressions), assembly cuts (cutting components in the assembly only, with a sketch on a plane or face and edges converted from components), configurations (a component showing one of its part's configurations, and an assembly's own configurations: left and right hinged, with components and mates suppressed, configured and placed per configuration, equations that differ per configuration, derived configurations), equations that size and suppress components from the assembly (SolidWorks addresses like Height@Sketch1@Door-1, reference dimensions, making a component independent), linked parts that follow their own files, interference between components, the bill of materials and the mass of the whole, hiding, isolating and colouring components, exploded views, STEP files with their assembly structure (import and export), replacing and deleting, changing a part of an assembly, sub-assemblies, saving and exporting.
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
  component, unless it is made flexible: see "Flexible sub-assemblies").
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

- **An end** is `{"component": NAME, "face": SELECTOR}` (or `"edge"`, `"vertex"`, or `"plane"`).
  The selector is in the **part's own coordinates**, as if the part were open by itself,
  not where the component is in the assembly (`peet skills selectors`). To see a part's
  faces, `open_component` it and run `faces`.
- **What an end stands for:** a flat face is a plane; a round face (a hole, a shaft) or
  a round edge is its axis; a straight edge is a line; a vertex is a point. A round edge
  `coincident` with a plane is the exception: the circle lies in the plane (the edge of
  a disc flat on a face), as SolidWorks has it. `"origin": true` (with or without a
  `component`) and sketch entities are ends too: see below.
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
- **`align`** says which way round in SolidWorks' terms: `aligned` (what `flip` does),
  `anti_aligned` or `closest` (whichever way the components are nearer now). Give `flip`
  or `align`, not both. Two axes (a pin in a hole) go whichever way is nearer unless
  `align` says: `aligned` points the axes alike, `anti_aligned` apart. An axis's
  direction is the `axis` that `faces` or `edges` gives for it, in its part.
- **`flip_dimension`** measures a distance (or an angle) the other way. A distance from
  a flat face goes out along its outward normal, from the first end if it is a flat face:
  the other end is in front of the face. With `"flip_dimension": true` it is behind the
  face, the same distance away. This is SolidWorks' "Flip dimension", and independent of
  `flip`.
- **`"suppressed": true`** adds the mate without solving it: nothing moves. It is solved
  when unsuppressed (`{"op": "suppress", "mate": NAME, "on": false}`), as any mate is,
  and fails then if it can't hold. Use it for a mate that should hold only sometimes.
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
- `edit_mate` changes a `distance`, an `angle`, `flip` or `align`, or `flip_dimension`. `rename`, `suppress` and
  `delete` take `mate`. Deleting a component deletes its mates.
- A mate's `distance` or `angle` can be an expression over the assembly's parameters
  (`set_parameter`): the components follow when the parameter changes.
- `set` on an assembly takes a mate's value by SolidWorks' name for it, `D1@` the mate's
  name (`D1@Distance1`, in any case), as FabSys sets it, or an assembly parameter by its
  name; with `configurations`, in some of the assembly's configurations (see
  "Configurations"). A dimension of a component's part is set from the assembly too: see
  "Equations" below.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Base"}
{"op": "insert", "part": 1, "name": "Lid", "at": [0, 0, 30]}
{"op": "mate", "type": "distance", "distance": 10, "a": {"component": "Base", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Lid", "face": {"normal": [0, 0, -1]}}}
{"op": "set", "name": "D1@Distance1", "value": 20}
{"op": "components"}
{"op": "edit_mate", "mate": "Distance1", "flip_dimension": true}
{"op": "components"}
{"op": "insert", "part": 1, "name": "Side", "at": [80, 0, 0]}
{"op": "mate", "type": "coincident", "align": "aligned", "suppressed": true, "a": {"component": "Base", "face": {"normal": [1, 0, 0]}}, "b": {"component": "Side", "face": {"normal": [1, 0, 0]}}}
{"op": "suppress", "mate": "Coincident1", "on": false}
{"op": "mates"}
```

The lid's underside goes 20 above the base's top, then 20 below it once the dimension is
flipped. `Coincident1` moves nothing until it is unsuppressed; then `Side`'s right face is
flush with the base's.

### Reference planes and parts inside sub-assemblies

Use a plane when the mate locates a part by its datum rather than a solid face.
`plane` takes the same selector as a sketch's `on`. Create named reference planes in
the part before inserting it. A plane mate follows edits to that plane and keeps its
reference when the plane is renamed. Standard planes work even in a part with no bodies.

To reach a part inside a sub-assembly, give `component` a nonempty array from the
assembly's own component down to the part. Each entry is a name or id in that level's
assembly. Geometry selectors still use the final part's own coordinates.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "plane", "from": "top", "distance": 7, "name": "MatePlane"}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Plate", "at": [0, 0, 100]}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Base"}
{"op": "insert", "part": 2, "name": "Stand", "at": [80, 0, 50]}
{"op": "mate", "type": "distance", "distance": 10, "flip": true, "a": {"component": "Base", "plane": "MatePlane"}, "b": {"component": ["Stand", "Plate"], "plane": "MatePlane"}}
{"op": "component_pattern", "components": ["Stand"], "type": "linear", "direction": {"component": ["Stand", "Plate"], "plane": "top"}, "spacing": 50, "count": 2}
{"op": "mates"}
{"op": "components"}
```

The example moves `Stand` to Z = -90. Its plate stays 100 above the sub-assembly's
origin, leaving 10 between the two mating planes. The sub-assembly moves as one rigid
component. Two ends inside the same rigid top-level component cannot be joined here. Add
that mate inside the sub-assembly instead. `fasten` holds the top-level components
as wholes even when its ends name nested paths.

`mates` reports a nested end's full `path` as names and a plane end's current `plane`
name. An invalid path fails before adding a mate and names the level that could not
be found. A stored mate fails if its plane or nested component is deleted or suppressed.
Restore it, or delete the mate and select another end.

### Flexible sub-assemblies

A sub-assembly is **rigid** by default: it moves as one thing, posed as its own file has
it. Make it **flexible** when something in it has to move in this assembly: a hinge that
this assembly's mates open, a slide, a door stay. This is SolidWorks' "Solve as:
Flexible".

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
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-50, -50], "to": [50, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 4, "name": "Base"}
{"op": "insert", "part": 3, "name": "Hinge", "at": [0, 0, 5], "fixed": true}
{"op": "flexible", "component": "Hinge"}
{"op": "place", "component": ["Hinge", "Arm"], "rotate": {"axis": "z", "angle": 60}}
{"op": "mate", "type": "parallel", "align": "closest", "a": {"component": "Base", "face": {"normal": [0, 1, 0]}}, "b": {"component": ["Hinge", "Arm"], "face": {"normal": [1, 0, 0]}}}
{"op": "components"}
```

Document 3 is a hinge: the post fixed, the arm free to turn round it. In the assembly
the arm is turned to 60° and then mated to turn to 90°, along Y.

- **`flexible`** (`component`, `on`, default true) solves the sub-assembly's own mates
  here, together with this assembly's. What is fixed in the sub-assembly moves with it;
  each component that is not fixed there moves as those mates and this assembly's let
  it. `"on": false` makes it rigid again, posed as its file has it.
- **Mate into it** with a component path (`["Hinge", "Arm"]`), as for any part inside a
  sub-assembly. Then a mate that a rigid sub-assembly would fail ("can't hold") can
  turn the arm instead.
- **Each instance is posed on its own**: two instances of one hinge can be open to
  different angles. The sub-assembly's file is not changed.
- **`place`** takes a path to a component that moves in a flexible sub-assembly:
  `{"op": "place", "component": ["Hinge", "Arm"], "rotate": ...}` (`at` and `rotate` in
  this assembly's coordinates). Use it to start the arm near where it should end, so the
  mates take it there and not to the other of two solutions. A component fixed in its
  sub-assembly, or one in a rigid sub-assembly, is refused: place the sub-assembly.
- **Read `components`**: a flexible component has `"flexible": true` and `moving`: each
  component that moves in it, by its `path` below it, with where it is (`at`,
  `rotate`) and its own `freedom`. The assembly's `freedom` counts them too: the
  hinge above has 1 until the parallel mate holds the arm.
- A flexible sub-assembly in a flexible one is taken apart too: reach it with a longer
  path (`["Door", "Hinge", "Arm"]`). Make the inner one flexible in its own assembly
  (the door's), and the outer one here.
- A pattern's copy of a flexible sub-assembly is placed as one thing.
- When the sub-assembly's mates and this assembly's can't all hold, the sub-assembly's
  win and this assembly's mate fails, as a hinge can't come apart.

### Width mates

Use `width` to locate a tab between two faces without creating midpoint planes in
each part. Select parallel, distinct planar faces or reference planes for each pair.
The width pair is `a`, the tab is `b`. Each pair is usually on one top-level component
(the ends may use different nested paths within it); see below for a pair on two. A single tab end
can be a cylindrical face or an edge used as an axis. A cylinder uses its diameter for
clearance; an edge used as an axis has zero thickness.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Width"}
{"op": "insert", "part": 2, "name": "Tab", "at": [60, 0, 0]}
{"op": "mate", "type": "width", "name": "Locate tab", "a": [{"component": "Width", "face": {"normal": [-1, 0, 0]}}, {"component": "Width", "face": {"normal": [1, 0, 0]}}], "b": [{"component": "Tab", "face": {"normal": [-1, 0, 0]}}, {"component": "Tab", "face": {"normal": [1, 0, 0]}}]}
{"op": "components"}
{"op": "edit_mate", "mate": "Locate tab", "mode": "free"}
{"op": "place", "component": "Tab", "at": [-30, 0, 0]}
{"op": "components"}
{"op": "edit_mate", "mate": "Locate tab", "mode": "dimension", "distance": 4}
{"op": "components"}
{"op": "set_parameter", "name": "ratio", "value": 25}
{"op": "edit_mate", "mate": "Locate tab", "mode": "percent", "percent": "ratio"}
{"op": "components"}
{"op": "set_parameter", "name": "ratio", "value": 75}
{"op": "components"}
{"op": "edit_mate", "mate": "Locate tab", "mode": "centered"}
{"op": "components"}
{"op": "mates"}
```

The tab's X positions in the example are 10, 0, 4, 5, 15, then 10. The available
clearance is the width span minus the tab span, here 20. `centered` divides it equally.
`free` leaves sliding across the gap available; `place` and `drag` stop at either end.
`dimension` sets the surface gap from the first width end to the nearest tab surface.
`percent` divides the available clearance: 0 touches the first width end, 100 touches
the second, and 50 centers the tab. Reversing the width pair reverses the side used by
Dimension and Percent. Values can be expressions over the assembly's parameters.

A planar tab keeps three freedoms in Centered, Dimension and Percent, or four in
Free when clearance is positive. It can slide along the mating planes and rotate
about their normal. A cylindrical tab keeps its axis in the midplane and its spin
free. Width mates keep the nearest orientation and do not take `flip`, `align` or
`flip_dimension`.

Centred, the tab may be wider than the width pair: their midplanes coincide, as in
SolidWorks. Nonparallel or coincident pairs, a tab wider than the gap in another mode,
an out-of-range distance or a percent outside 0 to 100 fail the mate with a message. Correct the geometry or value;
the stored mate recovers on rebuild. Drafted and angular width pairs are not supported.
Changing either selected face or plane recomputes the midpoint and clearance. If an
end is gone, restore it or delete the mate and select new ends. `mates` lists both ends
of each pair and the current mode and value. The properties panel can edit the mode
and value too.

A pair may also be on **two components** (a tab centred between two separate side
plates, or a gap between two parts that a third fills). Give the four ends as before;
each face then counts by where it is along the first width face's normal, and a tab
face is kept parallel to that face. Here the tab goes to X = 25, 17.5, then 14:

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [10, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Left"}
{"op": "insert", "part": 1, "name": "Right", "at": [60, 0, 0], "fixed": true}
{"op": "insert", "part": 2, "name": "Tab", "at": [15, 0, 0]}
{"op": "mate", "type": "width", "a": [{"component": "Left", "face": {"normal": [1, 0, 0]}}, {"component": "Right", "face": {"normal": [-1, 0, 0]}}], "b": [{"component": "Tab", "face": {"normal": [-1, 0, 0]}}, {"component": "Tab", "face": {"normal": [1, 0, 0]}}]}
{"op": "components"}
{"op": "edit_mate", "mate": "Width1", "mode": "percent", "percent": 25}
{"op": "edit_mate", "mate": "Width1", "mode": "dimension", "distance": 4}
{"op": "components"}
```

### The assembly's own planes and origin, origins and sketches

- **An end with no `component`** is the assembly's own standard plane or origin:
  `{"plane": "top"}` (or `front`, `right`), `{"origin": true}`. They don't move, so a
  part mated to them is placed in the assembly as SolidWorks places a part on the
  assembly's planes. A mate needs a component at one end at least.
- **`{"component": C, "origin": true}`** is the part's origin (with a path, the origin
  of the part or sub-assembly the path ends at). Two origins `coincident` put the points
  together and leave the part free to turn: add plane mates for its axes.
- **`{"component": C, "sketch": "Sketch1", "entity": 13}`** is a point, line, circle or
  arc of a sketch of the part: a point, a line, or a round edge (its axis, or in a plane
  for `coincident` with one). Find the entity's id with
  `{"op": "feature", "feature": "Sketch1"}` in the part (`open_component`, or before
  inserting it); a line's ends and a circle's centre are points too (`start`, `end`,
  `center`). It follows the sketch when it is edited.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}, {"type": "point", "at": [10, 5]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 4}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 10}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Block", "at": [3, 4, 50], "fixed": false}
{"op": "mate", "type": "coincident", "a": {"plane": "top"}, "b": {"component": "Block", "face": {"normal": [0, 0, -1]}}}
{"op": "mate", "type": "coincident", "a": {"origin": true}, "b": {"component": "Block", "origin": true}}
{"op": "insert", "part": 2, "name": "Disc", "at": [70, 3, 20]}
{"op": "mate", "type": "coincident", "a": {"component": "Block", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Disc", "edge": {"at": [-5, 0, 0]}}}
{"op": "mate", "type": "coincident", "a": {"component": "Block", "sketch": "Sketch1", "entity": 13}, "b": {"component": "Disc", "face": {"at": [5, 0, 5]}}}
{"op": "components"}
```

The block's underside goes onto the assembly's top plane with its origin at the
assembly's; the disc's bottom edge lies on the block's top and its axis goes through the
sketch point (10, 5).

### Slot, limit and symmetric mates

- **`slot`**: a pin in a straight slot. `a` and `b` are a face of the slot (one of its
  round ends or a flat side) and the pin's round face or edge, either way round. The pin
  stays parallel to the slot's ends, on the line between their axes. `mode`: `free` (the
  default: anywhere between the ends, so `drag` slides it), `centered`, `dimension` with
  `distance` from the first end's axis (the round end you picked, else the one the part
  lists first; `flip_dimension` measures from the other), or `percent`. It fails if
  neither end is a face of a straight slot (two round ends of one radius joined by flat
  sides): pick a face of the slot itself, not of the plate around it.
- **`limit_angle`** with `min` and `max` (degrees), and **`limit_distance`** with `min`
  and `max` (lengths): the angle or distance, measured as `angle` and `distance` measure
  it (`flip_dimension` too), kept anywhere between them. Nothing else is held, so add the
  hinge or slide they limit. A component outside the range is brought just inside it.
- **`symmetric`**: `a` and `b` (two planes, points or lines) mirror images in `plane`,
  an end like any other (`{"plane": "right"}` for the assembly's). Both ends on one
  component centre it on the plane: two opposite faces of a part about the assembly's
  planes. The plane must be on one of the two components, a fixed one, or the
  assembly; it fails otherwise.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 40]}, {"type": "slot", "from": [20, 20], "to": [60, 20], "radius": 4}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 3}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 20}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Base"}
{"op": "insert", "part": 2, "name": "Pin", "at": [30, 25, -5]}
{"op": "mate", "type": "slot", "a": {"component": "Base", "face": {"at": [40, 16, 2.5]}}, "b": {"component": "Pin", "face": {"at": [3, 0, 10]}}}
{"op": "edit_mate", "mate": "Slot1", "mode": "percent", "percent": 75}
{"op": "insert", "part": 1, "name": "Lid", "at": [0, 0, 30]}
{"op": "mate", "type": "symmetric", "a": {"component": "Lid", "face": {"normal": [-1, 0, 0]}}, "b": {"component": "Lid", "face": {"normal": [1, 0, 0]}}, "plane": {"plane": "right"}}
{"op": "mate", "type": "concentric", "a": {"component": "Base", "edge": {"at": [40, 0, 5]}}, "b": {"component": "Lid", "edge": {"at": [40, 0, 0]}}}
{"op": "mate", "type": "limit_angle", "min": 10, "max": 60, "a": {"component": "Base", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Lid", "face": {"normal": [0, 0, 1]}}}
{"op": "components"}
{"op": "mates"}
```

The pin goes to X = 50, three quarters along the slot. The lid is centred on the
assembly's right plane, hinged on the base's front top edge, and opened to between
10° and 60°.

### Path mates

**`path`** keeps a point of one component on a path of another: a stay's pin in a curved
track, a slider on a rail that bends. `a` is the point (a vertex, a sketch point, or
`"origin": true`); `b` is the path, one end or a list of them joined end to end, all on one
component: straight and round edges, or a sketch's lines, arcs and circles (sketch the
track's centreline in the part and use that). The path starts at the first piece's end away
from the others (a path of one piece runs the way it was drawn).

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "sketch", "on": "top", "name": "Track", "draw": [{"type": "line", "from": [10, 20], "to": [60, 20]}, {"type": "arc", "center": [60, 40], "start": [60, 20], "end": [80, 40]}]}
{"op": "feature", "feature": "Track"}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-3, -2], "to": [12, 2]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 4}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Plate"}
{"op": "insert", "part": 2, "name": "Shoe", "at": [30, 25, 0]}
{"op": "mate", "type": "path", "a": {"component": "Shoe", "origin": true}, "b": [{"component": "Plate", "sketch": "Track", "entity": 1}, {"component": "Plate", "sketch": "Track", "entity": 4}]}
{"op": "edit_mate", "mate": "PathMate1", "mode": "dimension", "distance": 20}
{"op": "edit_mate", "mate": "PathMate1", "mode": "percent", "percent": 80}
{"op": "components"}
{"op": "mates"}
```

The shoe's origin goes onto the track at (30, 20), then 20 along it, then 80% of the way:
round the bend. (The track's line is entity 1 and its arc 4: `feature` lists them.)

- **Where along it:** `mode` `free` (the default: anywhere on the path, so `drag` slides it,
  stopping at each joint between pieces), `dimension` with `distance` from the path's start
  (`flip_dimension` measures from its end), `percent`, or `centered`. `edit_mate` changes
  them.
- **Which way it faces:** the point's component turns freely unless `follow` (`"x"`, `"y"`
  or `"z"`: that axis of its own coordinates kept along the path, either way round:
  SolidWorks' pitch and yaw "follow path") and `roll` (`{"axis": "z", "up": END}`: that axis
  kept along a direction: a flat face's or plane's normal, a straight edge, an axis;
  SolidWorks' "up vector") say. Both: the component is held but for its way along the path.
- It fails if the path's pieces don't join end to end, aren't all on one component, or are
  splines; or if `a` isn't a point. A distance longer than the path fails with its length.
- `follow` and `roll` are given when the mate is added: to change them, delete the mate and
  add it again.

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

## Component patterns

A pattern copies components in rows or round an axis. Use it for fasteners and anything
else that repeats: **mate the original once, and the copies are in place**.

```jsonl
{"op": "new", "assembly": true}
{"op": "insert", "sample": "chassis", "name": "Chassis"}
{"op": "insert", "sample": "cover", "name": "Cover", "at": [0, 0, 40]}
{"op": "mate", "type": "coincident", "a": {"component": "Chassis", "face": {"at": [234, 80, 40]}}, "b": {"component": "Cover", "face": {"at": [10, 10, 0]}}}
{"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Chassis", "face": {"at": [240, 80, 20]}}, "b": {"component": "Cover", "face": {"at": [240, 80, 0.75]}}}
{"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Chassis", "face": {"at": [200, 160, 4]}}, "b": {"component": "Cover", "face": {"at": [120, 160, 0.75]}}}
{"op": "insert", "sample": "screw", "name": "Screw", "at": [20, 25, 1.5]}
{"op": "mate", "type": "concentric", "a": {"component": "Chassis", "face": {"at": [20, 27.25, 0.75]}}, "b": {"component": "Screw", "face": {"at": [0, 2, -4]}}}
{"op": "mate", "type": "coincident", "a": {"component": "Chassis", "face": {"at": [30, 25, 1.5]}}, "b": {"component": "Screw", "face": {"at": [0, 3, 0]}}}
{"op": "component_pattern", "components": ["Screw"], "type": "linear", "direction": {"component": "Chassis", "face": {"at": [240, 80, 20]}}, "spacing": 200, "count": 2, "second": {"direction": "y", "spacing": 110, "count": 2}, "name": "Screws"}
{"op": "components"}
```

- **`count` includes the original**: `"count": 2` with a second direction of 2 makes
  three copies. A linear pattern takes `direction`, `spacing`, `count` (and `flip`),
  and the same again under `second` for rows and columns. A circular one takes `axis`,
  `count` and `angle` (left out: 360, evenly all the way round; less: from the first to
  the last).
- **A direction or an axis** is one of the assembly's own (`"x"`, `"y"`, `"z"`,
  `[x, y, z]`, or for an axis that is not through the origin
  `{"origin": [x, y, z], "direction": "z"}`), or geometry of a component:
  `{"component": ..., "edge": selector}` for a straight edge, `"face"` for a round
  face (its axis) or a flat face (its normal), the selector in the part's own
  coordinates as for a mate. Take it from a component when the pattern should follow
  that component: bolts round a housing use the housing's bore, so they stay on its
  bolt circle wherever the housing goes.
- A component pattern's direction also accepts `plane` for its normal and a nested
  component path, using the same selectors as a mate end.
- **The copies are components** of the original's part: `components` lists them (with
  `"pattern": name`), the bill of materials counts them, and they are weighed,
  checked for interference and exported. A copy can be hidden, coloured, renamed or
  suppressed like any other.
- **But a copy is the pattern's to place**: `place`, `drag`, `fix`, `mate`, `replace`
  and `delete` on a copy fail and say which pattern made it and which component is
  its original. Move or mate the original, or change the pattern.
- `edit_component_pattern` (`pattern`, and any of `count`, `spacing`, `angle`, `flip`,
  `direction`, `axis`, `second`; `"second": null` takes the second direction away)
  adds and removes copies to match, and the ones that stay keep their names. `rename`
  and `delete` take `pattern` in place of `feature`; deleting a pattern deletes its
  copies and leaves the original, and deleting the original deletes its pattern.
- A pattern whose direction can't be found (the face it was taken from is gone, the
  component is suppressed) is not a failed operation: it is listed under `failures`
  with the reason, and its copies stay where they are.
- **A count or spacing from an expression**, as SolidWorks' equations drive a component
  pattern: `count` takes an expression, and `set` takes a pattern's values by SolidWorks'
  names, `D1@PATTERN` (the count; `D2` the second direction's) and `D3@PATTERN` (the
  spacing; `D4` the second's; a circular pattern's angle is `D3`). The expression is
  worked out at every rebuild, and copies are made or removed as the count changes.
  Expressions read the values by the same addresses. A count must work out to a whole
  number; one that doesn't is refused when set, and leaves the pattern failed (its copies
  as they were) when a later change makes it so.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [1200, 40], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 1200, "name": "Length"}, {"type": "length", "of": ["r.left"], "value": 40}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 2}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 20}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Tray-1", "fixed": true}
{"op": "insert", "part": 2, "name": "Bracket-1", "at": [0, 0, 2], "fixed": true}
{"op": "component_pattern", "components": ["Bracket-1"], "type": "linear", "direction": "x", "spacing": 590, "count": "iif(\"Length@Sketch1@Tray-1\" > 1100, 3, 2)", "name": "Brackets"}
{"op": "set", "name": "D3@Brackets", "value": "(\"Length@Sketch1@Tray-1\" - 20) / (\"D1@Brackets\" - 1)"}
{"op": "set", "name": "Length@Sketch1@Tray-1", "value": 900}
{"op": "components"}
```

  The tray made 900 long leaves two brackets, 880 apart.
- `{"op": "open_sample", "sample": "assembly"}` opens a finished example: the chassis,
  a cover, the housing on the cover with six bolts (a circular pattern round the
  housing's bore) and four screws (a linear pattern along the chassis's walls), every
  component held, with an exploded view. The parts are samples too: `chassis`,
  `cover`, `housing`, `bolt`, `screw`.

## Assembly cuts

An assembly cut is SolidWorks' cut made in an assembly: an extruded sketch that removes
material from **the components in the assembly only**. Their parts, and every other
assembly that uses them, are unchanged (`open_component` shows the part whole). Use it
for holes drilled through parts once they are put together. To change the part itself,
cut it in the part.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Base", "fixed": true}
{"op": "insert", "part": 1, "name": "Cover", "at": [0, 0, 5]}
{"op": "assembly_cut", "on": "top", "draw": [{"type": "circle", "center": [50, 30], "radius": 10}], "end": "through_all", "flip": true, "components": ["Cover"]}
{"op": "assembly_cut", "on": {"component": "Cover", "face": {"normal": [0, 0, 1]}}, "convert": [{"component": "Base", "edge": {"at": [100, 30, 5]}}], "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 10]}], "depth": 2, "name": "Notch"}
{"op": "mass"}
{"op": "components"}
```

The cover has a hole through it and a notch 2 deep in its top; the base is whole.

- **What it lies on.** `on` is the assembly's own plane (`"top"`, `"front"`, `"right"`),
  a flat face or a plane of a component (`{"component": ..., "face": ...}`), which the
  sketch then follows when the component moves, or a plane at a place
  (`{"origin": [x, y, z], "normal": [x, y, z], "x": [x, y, z]}`). `draw` is as for
  `sketch`, in the plane's coordinates.
- **Converted edges.** `convert` lists components' edges (`{"component": name or path,
  "edge": selector}`) taken into the sketch where they are seen on its plane, as
  SolidWorks' Convert Entities: they follow the components at every rebuild. A circle
  converted from a bolt hole cuts that hole on through the parts it is bolted to. The
  reply's `converted` gives each one's sketch `entity`.
- **Which way, how far.** A cut goes against the plane's normal (into the face it is on);
  `flip` turns it. `end` is `blind` (with `depth`), `mid_plane` or `through_all`.
- **Scope.** Left out, it cuts every component it reaches (SolidWorks' feature scope
  with every component); `components` limits it to those. A sub-assembly in the scope
  is cut through all its parts.
- **It cuts nothing?** Its `status` is `warning` with why: check the sketch, `flip`, the
  depth and the scope. A cut whose plane or converted edge is gone fails, and the
  components are whole until it is fixed.
- The cut bodies are what is drawn, weighed (`mass`), checked for `interference` and
  exported. Mates are solved on the parts as they are: mate to the part's own faces.
- `edit_assembly_cut` (`cut`, and any of `on`, `draw` and `convert` (added), `end`,
  `depth`, `flip`, `flip_side`, `components` (`null`: every one), `suppressed`);
  `rename` and `delete` take `cut`. `components` lists the cuts under `cuts`.

## Changing components

`rename`, `suppress`, `show` and `delete` take `component` instead of `feature`.
`replace` makes a component an instance of another part, where it is. Deleting the last
component of a part removes the part from the assembly.

## Showing, hiding, colouring, exploding

None of these moves a component or changes a mate: they are about what is seen.

```jsonl
{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket", "name": "Base"}
{"op": "insert", "component": "Base", "at": [0, 0, 40], "name": "Top"}
{"op": "insert", "sample": "housing", "name": "Bearing", "at": [60, 40, 80]}
{"op": "set_color", "component": "Top", "color": "#c82828"}
{"op": "isolate", "components": ["Bearing"]}
{"op": "show_all"}
{"op": "explode_step", "components": ["Top", "Bearing"], "by": [0, 0, 60]}
{"op": "explode_step", "components": ["Bearing"], "by": [0, 0, 50], "name": "Lift bearing"}
{"op": "explode"}
{"op": "explode_steps"}
```

- **Hidden is not suppressed.** `show` with `"on": false` (or `isolate`, which hides
  every component but the ones named) only stops a component being drawn: it is still
  mated, checked for interference, counted in the bill of materials, weighed and
  exported. To take a component out of the assembly without deleting it, `suppress` it.
  `show_all` shows everything again; `components` marks hidden ones `"hidden": true`.
- **`set_color` with `component`** colours that one component, in place of its part's
  colour: use it to tell instances of one part apart. `"color": null` goes back to the
  part's. Without `component`, `set_color` is the part's own colour, set in the part's
  document (`open_component`), and it reaches every instance.
- **An exploded view is a list of steps**, stored in the assembly. `explode_step` names
  components and how far they go (`by`, along the assembly's axes, in its units). A
  component in several steps is moved by their sum, so pull a group away first and then
  one of them further, as above: `Bearing` ends 110 above where it is.
- **`explode` only changes what is drawn.** `components`, `mates`, `interference`,
  `mass` and `export` always mean the assembly as it is put together, whether it is
  shown exploded or not. `explode_steps` lists the steps and, under `moved`, where each
  component is drawn when exploded (`at`). `explode` is not an undo step and is not
  saved: a file opens put together.
- Steps move the assembly's own components. A sub-assembly moves as one thing; to
  take it apart, give it steps in its own document.
- `edit_explode_step` (`explode_step`, and `by` or `components`) changes a step;
  `rename` and `delete` take `explode_step` in place of `feature`. Deleting a component
  takes it out of its steps, and a step left with none goes too.
- `explode` with no steps fails: add one first.

## Equations: sizing and suppressing components from the assembly

An assembly reads and sets its components' dimensions as SolidWorks' assembly equations
and FabSys do. Use it when one part's size follows another's (a shield 50 narrower than
its door), when a component is there only at some sizes (a third hinge on a tall door),
or to set a part's size while building the assembly.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [800, 40], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 800, "name": "Width"}, {"type": "length", "of": ["r.left"], "value": 40, "name": "Height"}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 2}
{"op": "new", "keep": true}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [700, 30], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 700, "name": "Width"}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 2}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Door-1"}
{"op": "insert", "part": 2, "name": "Shield-1", "at": [0, 0, 20]}
{"op": "insert", "part": 2, "name": "Stiffener-1", "at": [0, 0, 40]}
{"op": "make_independent", "component": "Stiffener-1", "name": "Stiffener"}
{"op": "set", "name": "Height@Sketch1@Door-1", "value": 1600}
{"op": "set", "name": "Width@Sketch1@Shield-1", "value": "\"Width@Sketch1@Door-1\" - 50"}
{"op": "set_suppression_expression", "component": "Stiffener-1", "value": "iif(\"Height@Sketch1@Door-1\" > 1500, 0, 1)"}
{"op": "reference_dimension", "a": {"component": "Door-1", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Shield-1", "face": {"normal": [0, 0, -1]}}}
{"op": "set_parameter", "name": "gap", "value": "\"RD1@Annotations\""}
{"op": "suppress", "feature": "Extrude1@Stiffener-1"}
{"op": "suppress", "feature": "Extrude1@Stiffener-1", "on": false}
{"op": "dimensions"}
{"op": "dimensions", "component": "Shield-1"}
{"op": "set", "name": "Height@Sketch1@Door-1", "value": 1200}
{"op": "components"}
```

The shield ends 750 wide and follows the door's width; the stiffener is there while the
door is over 1500 high, so the last `set` suppresses it.

- **An address that reaches into a component** is the part's address followed by the
  component's name: `Height@Sketch1@Door-1` (a dimension), `Thickness@Door-1` (a
  parameter of its part), `Height@Sketch1@Door-1@Door Assembly-1` (through a
  sub-assembly). SolidWorks' forms are taken too: `Height@Sketch1@Door<1>.Part`,
  `Door-1.Part`. As in SolidWorks, a component's name is found in any case where only
  one component matches. Quote it in an expression: `"Height@Sketch1@Door-1" - 20`.
- **`set` with a number changes the part**, and so every component of it, as setting a
  part's dimension in a SolidWorks assembly changes the part's file. A linked part's file
  is written (the reply's `written`). To change one component alone, `make_independent`
  it first: it gets a copy of the part called `name`, and is renamed `name-1` (FabSys's
  `ResolveComponent`). Address it by its new name afterwards.
- **`set` with an expression** that reads anything (`"Width@Sketch1@Door-1" - 50`) makes
  an **equation** of the assembly: worked out again at every rebuild, so the part follows.
  A number set later replaces it. An equation lives in the assembly: opened on its own,
  the part has the value the equation last gave it.
- **`set_suppression_expression` with `component`** suppresses the component while the
  expression is not 0, and unsuppresses it while it is 0 (SolidWorks' `suppressed` is 1,
  `unsuppressed` 0). It decides over `suppress` until cleared with `"value": null`.
- **`suppress` with `"feature": "FEATURE@COMPONENT"`** suppresses (or with `"on": false`
  unsuppresses) a feature of the component's part, in every instance of it.
- **`reference_dimension`** measures the distance between two ends (as a mate's: two
  parallel flat faces or planes, a point and a plane, points, lines) where the mates put
  the components. Expressions read it as `RD1@Annotations`, as SolidWorks does, so a pipe
  can be as long as the gap it spans. The reply's `reference.value` is what it measures.
  `{"op": "delete", "reference_dimension": "RD1"}` removes it.
- **`dimensions`** on an assembly lists its own values (`D1@` each distance mate,
  `RD1@Annotations`), its `equations` and `references`; with `component`, the addresses
  of that component's part from the assembly. An equation that can't be worked out (a
  dimension that is gone, a component renamed) is listed under `failures` with why: fix
  the address with `set` again.

## Configurations

Two things, as in SolidWorks, and they go together:

- **A component shows one configuration of its part** (SolidWorks' referenced
  configuration). `insert` takes `configuration`; `component_configuration` changes it
  (FabSys's `SetSubComponentConfig`). Two components of one part can show different
  configurations: a door in "R Hinges" beside one in "Default". Without one, a component
  shows whichever configuration its part has active.
- **An assembly has configurations of its own**, made with the part operations
  (`add_configuration`, `configuration`, `edit_configuration`, `delete_configuration`,
  `configurations`). In each, a component can be suppressed or not, show another
  configuration of its part, be fixed or not, and be somewhere else; a mate can be
  suppressed or not and have another value. Use them for variants of one assembly: left
  and right hinged, a long and a short duct.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "add_configuration", "name": "Thick"}
{"op": "edit", "feature": "Extrude1", "depth": 20, "configurations": "this"}
{"op": "configuration", "configuration": "Default"}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Base"}
{"op": "insert", "part": 1, "name": "Lid", "configuration": "Thick", "at": [0, 0, 40]}
{"op": "mate", "type": "distance", "distance": 10, "a": {"component": "Base", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Lid", "face": {"normal": [0, 0, -1]}}}
{"op": "insert", "component": "Base", "name": "Spare", "at": [100, 0, 0]}
{"op": "edit_configuration", "configuration": "Default", "name": "Closed"}
{"op": "add_configuration", "name": "Open"}
{"op": "set", "name": "D1@Distance1", "value": 50, "configurations": "this"}
{"op": "component_configuration", "component": "Lid", "configuration": "Default"}
{"op": "suppress", "component": "Spare", "configurations": ["Closed"]}
{"op": "configurations"}
{"op": "components"}
{"op": "configuration", "configuration": "Closed"}
{"op": "components"}
{"op": "bom"}
```

In "Closed" the 20 thick lid is 10 above the base and the spare is suppressed; in "Open"
the 6 thick lid is 50 above it and the spare is there.

- **What changes where.** `suppress`, `fix`, `place` and `component_configuration` on a
  component, and `suppress` on a mate, change the active configuration only, unless
  `configurations` says which (`"all"`, a name, a list). A mate's value (`edit_mate`,
  `set` `D1@MATE`) and an equation (`set` with a component's address,
  `set_suppression_expression` with a component) change in all of them until they
  differ, then in the active one: give `"configurations": "this"` to make them differ (see
  "Equations that differ between configurations"). Parameters differ with
  `set_parameter`'s `configurations`, as in a part. Everything else (names, colours, ends,
  `flexible`, reference dimensions, patterns) is the same in every configuration.
- **Where a component is belongs to the configuration.** Mates solve each configuration
  from its own placements, and a solve or a `place` moves a component in the active one
  only. So place components where they go in each configuration before relying on the
  mates to choose between two solutions there.
- **A new configuration is a copy** of the active one (or of `copy`). A component or mate
  added later is in every configuration: suppress it where it does not belong.
- **Read it back.** `configurations` lists, for each component and mate that differs,
  what it is in every configuration; `components` gives each component's `configuration`
  (where its part has several), and each part its `configurations`. The `bom` gives a
  part in two configurations as two rows.
- **A configuration that is not there.** `insert` and `component_configuration` with a
  name the part doesn't have fail and list its configurations; `configuration` with a
  name the assembly doesn't have fails the same way. SolidWorks silently ignores a missing
  name; PeetCAD doesn't, on purpose: a script that asks for a configuration that isn't
  there has a mistake in it, so fix the name rather than ignore the error. A part that loses the configuration a
  component shows leaves the component showing its active one, with a warning in
  `components`.
- A sub-assembly is a component like a part: it shows one of its configurations, and its
  own components and mates are as that configuration has them.
- **`open_component` opens the part in the configuration the component shows** (the
  reply's `configuration`), as SolidWorks does: a change made there with
  `"configurations": "this"` is that configuration's. `save` stores it back with the
  part's own active configuration as it was, so components that name no configuration
  still show the same one.
- **Derived configurations** (`add_configuration` with `parent`) work in an assembly as in
  a part (`peet skills configurations`): each field of a component, each mate and each
  equation follows the parent unless the derived one has its own.

### Equations that differ between configurations

An equation of the assembly (`set` with a component's address, `set_suppression_expression`
with a component) can have another expression, or none, in some configurations: SolidWorks'
"This configuration" in the equation manager. A global variable is a parameter, and differs
with `set_parameter`'s `configurations`.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 6}
{"op": "new", "assembly": true, "keep": true}
{"op": "insert", "part": 1, "name": "Door"}
{"op": "insert", "part": 1, "name": "Hinge", "at": [100, 0, 0]}
{"op": "set_parameter", "name": "tall", "value": 0}
{"op": "set", "name": "depth@Extrude1@Door", "value": "6 + 4 * tall"}
{"op": "edit_configuration", "configuration": "Default", "name": "Short"}
{"op": "add_configuration", "name": "Tall"}
{"op": "set_parameter", "name": "tall", "value": 1, "configurations": "this"}
{"op": "set", "name": "depth@Extrude1@Door", "value": "12mm", "configurations": "this"}
{"op": "set_suppression_expression", "component": "Hinge", "value": "iif(tall > 0, 0, 1)", "configurations": "this"}
{"op": "configurations"}
{"op": "dimensions"}
{"op": "configuration", "configuration": "Short"}
{"op": "components"}
```

In "Tall" the door is 12 thick and the hinge is there; in "Short" the door is 6 thick and
the hinge is as it was set there (not suppressed), as no equation sets it.

- **Left out, `configurations` is "all"** for an equation that is the same everywhere,
  and the active one for one that already differs, as for a mate's value.
- `"value": null` (with `set`, or `set_suppression_expression`) removes the equation in
  those configurations: what it set keeps its last value there.
- **A number for some configurations is an equation of theirs** (`"12mm"`). A number for
  all of them sets the part, as before.
- **Suppression by an equation of some configurations** is that configuration's:
  the component keeps its own suppression in the others.
- **Read them back.** `configurations` lists under `equations` each one that differs, by
  what it sets (`depth@Extrude1@Door`, or the component's name for suppression), with its
  expression in every configuration (null: none). `dimensions` lists the active
  configuration's equations, then those only others have (`expression` null), each that
  differs with its `configurations`.
- One equation per thing set, per configuration: setting it again replaces it there.
  An equation reached through another instance of the same part (`Door-2`) replaces it
  in every configuration.

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
- `warnings` in the reply says what was left out, assumed or mended (a face that can't
  be read, units not given). "lay up to … mm off its faces … and was moved onto them"
  means the file gave edges a tolerance, as SolidWorks does, and PeetCAD refitted them
  onto their faces: the shape moved by at most that much, and the body is sound.
  "was repaired: … vertices moved … split … faces added to close the gaps" means the
  file gave vertices a tolerance too (SolidWorks does, up to a millimetre, for a body
  it imported from elsewhere: a small fillet squeezed into an edge): the body is sound,
  but differs from the file by up to the distance named, so check its volume if that
  matters. "The body #… was left out, and the rest imported without it" means a body
  could not be made a valid solid: the reply's `bodies` is fewer than the file has.
  Export that body again with a finer tolerance, or model it. If no body can be made
  valid the import fails with the reason ("leaves the surface of face", "doesn't
  close up").

Done when `components` shows every component with `"status": "ok"` and the `min` and
`max` that were asked for, `mates` shows every mate `"ok"`, `freedom` is what was
intended (0 for a rigid assembly; more if something is meant to turn or slide), and
`interference` is `"clear": true`.
