---
name: solids
description: Solid features in peet - extrude and cut directions and end conditions, revolve, sweep, loft, holes, fillets and chamfers, shell, draft, patterns and mirrors, moving and copying bodies, inserting or mirroring another part, structural members (weldment frames), freeform faces and imported STEP bodies.
---

# Solid features

Each feature takes a sketch (by name) or existing geometry (by selector: `peet skills
selectors`). `peet ops NAME` gives a feature's fields. Change one later with
`{"op": "edit", "feature": "Extrude1", ...}`, giving only the fields that change.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 10}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Centres", "draw": [{"type": "point", "at": [15, 25]}]}
{"op": "hole", "sketch": "Centres", "standard": {"size": "M6", "fit": "normal"}, "kind": "counterbore"}
{"op": "linear_pattern", "features": ["Hole1"], "direction": "x", "spacing": 25, "count": 3}
{"op": "fillet", "size": 4, "edges": [{"between": [[0, 0, 0], [0, 0, 10]]}, {"between": [[80, 0, 0], [80, 0, 10]]}]}
{"op": "shell", "open": [{"normal": [0, 0, -1]}], "thickness": 2}
{"op": "mass"}
```

## Extrude and cut

- `extrude` goes **along** the sketch's normal; `cut` goes **against** it, into the face
  the sketch is on. `"reverse": true` turns either round. If a cut "doesn't reach any
  body", it went the wrong way or its sketch is not on the body.
- `end`: `"blind"` (uses `depth`), `"mid_plane"` (`depth` split to both sides),
  `"through_all"`, `"through_next"`, or `{"up_to": plane}`.
- `"through_next"` is SolidWorks' "up to next": a `cut` goes on until it has come out of
  the first material in its way, so holes through one wall of a channel stop there and
  leave the wall across from it; an `extrude` goes until it meets material. If there is
  nothing in its way it fails: reverse it, or use `through_all`.

```jsonl
{"op": "sketch", "on": "top", "name": "Channel", "draw": [{"type": "polyline", "closed": true, "points": [[0, 0], [60, 0], [60, 40], [54, 40], [54, 6], [6, 6], [6, 40], [0, 40]]}]}
{"op": "extrude", "sketch": "Channel", "depth": 100}
{"op": "sketch", "on": {"at": [0, 20, 50], "normal": [-1, 0, 0]}, "name": "Bolt holes", "draw": [{"type": "circle", "center": [20, -30], "radius": 4}, {"type": "circle", "center": [20, -70], "radius": 4}]}
{"op": "cut", "sketch": "Bolt holes", "end": "through_next"}
```
- `operation`: `"add"` joins the bodies it touches (and is a new body if it touches
  none), `"new_body"` always makes a separate body, `"cut"` removes.
- A sketch with several regions extrudes all the outer ones, with regions inside them as
  holes. `regions` picks others: a list of sketch points, one inside each region wanted.
  Regions picked side by side are extruded as one shape.
- `draft` leans the walls by that many degrees from the direction of extrusion, pivoting
  on the sketch plane: what is added or removed narrows as it goes
  (`"draft_outward": true` widens it). A draft too steep for the depth fails to build;
  it can't be used with `mid_plane`.
- `"flip_side": true` on a cut removes everything **outside** its regions within its
  reach instead of inside. With a draft, the material removed still narrows, so the
  regions grow with depth: that is how a bolt head's corners are chamfered.

```jsonl
{"op": "sketch", "on": "right", "draw": [{"type": "polygon", "center": [0, 0], "vertex": [0, 7.5], "sides": 6}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5.3}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Chamfer", "draw": [{"type": "circle", "center": [0, 0], "radius": 6.5}]}
{"op": "cut", "sketch": "Chamfer", "end": "through_all", "flip_side": true, "draft": 60}
{"op": "sketch", "on": "top", "name": "Boss", "draw": [{"type": "rectangle", "from": [20, -5], "to": [30, 5]}]}
{"op": "extrude", "sketch": "Boss", "depth": 10, "draft": 5}
{"op": "bodies"}
```

## Revolve

`revolve` turns the sketch's regions about an axis in the sketch plane. Left out, the
axis is the sketch's first **construction line**, else the sketch's vertical axis. So
draw the profile on one side of a construction centreline. `axis` can also be
`"sketch_x"`, `"sketch_y"`, `{"line": id}` or an axis selector. `angle` defaults to 360.
`cut_revolve` removes the same shape.

## Sweep

`sweep` takes two sketches: `profile` (closed regions) and `path` (connected lines and
arcs in another sketch, starting on the profile's plane and square to it). A corner
between two straight pieces of the path is mitred. `cut_sweep` removes the same shape.

- The profile can also sit **part way along** a straight piece of the path, square to
  it: the path is swept both ways from it, end to end.
- A path that leaves the profile's plane **at a slant** carries the profile along as
  drawn, without turning it (a slanted cylinder from a circle). Such a path, like a
  spline, can't have corners.

```jsonl
{"op": "sketch", "on": "top", "name": "Section", "draw": [{"type": "circle", "center": [0, 0], "radius": 3}]}
{"op": "sketch", "on": "front", "name": "Rod", "draw": [{"type": "polyline", "points": [[0, 10], [0, 0], [0, -40], [20, -40]]}]}
{"op": "sweep", "profile": "Section", "path": "Rod"}
{"op": "bodies"}
```

The path can be a **spline** (see the sketching skill), for a pipe or a handle that
curves freely:

```jsonl
{"op": "sketch", "on": "right", "name": "Section", "draw": [{"type": "circle", "center": [0, 0], "radius": 4}, {"type": "circle", "center": [0, 0], "radius": 2.5}]}
{"op": "sketch", "on": "top", "name": "Route", "draw": [{"type": "spline", "points": [[0, 0], [20, 5], [40, -5], [60, 0]]}]}
{"op": "sweep", "profile": "Section", "path": "Route"}
```

- One end of the spline must lie **on the profile's plane**, heading away from it. It
  need not leave squarely (a spline's end direction can't be set): the profile keeps
  the attitude to the path it starts with, and is carried along without twisting.
- A path with a spline has **no corners**: every piece must carry on the way the last
  one ended, so draw the whole route as one spline. A closed spline is refused.
- "The profile doesn't fit round a bend of the path" means the path bends more tightly
  than the profile reaches towards the inside of the bend: move the spline's points
  apart, or make the profile smaller.
- The sides are freeform faces (below); it takes about a second to build.

## Loft

`loft` joins the closed profiles of two or more sketches, in the order given, into one
body. `cut_loft` removes the same shape.

```jsonl
{"op": "sketch", "on": "top", "name": "Base", "draw": [{"type": "rectangle", "from": [-20, -15], "to": [20, 15]}]}
{"op": "plane", "from": "top", "distance": 30, "name": "Up"}
{"op": "sketch", "on": "Up", "name": "Neck", "draw": [{"type": "circle", "center": [0, 0], "radius": 8}]}
{"op": "loft", "profiles": ["Base", "Neck"]}
{"op": "faces"}
```

- Each profile is one closed shape, in its own sketch, on its own plane: make the planes
  with `plane` (`from` and `distance`).
- Profiles need the **same number of sides**. A circle adapts to its neighbours, so a
  rectangle lofts to a circle. A rectangle to a triangle fails to build: split an edge
  of the profile with fewer sides so the counts match.
- Two profiles give straight (ruled) sides; three or more give sides that pass smoothly
  through every profile. There are no guide curves.

## Freeform faces

A loft's sides, the walls made from a sketch spline, a sweep along a spline and faces of
an imported STEP body (`import_step`) can be **freeform**: `faces` reports them with
`"surface": "freeform"`. They can be cut, added to, shelled, filleted, drafted, measured
and exported like any face, with these limits:

- `fillet` and `chamfer` roll along freeform edges, and along edges of curved faces (the
  rim of a hole drilled across another). A run of edges must either close on itself or
  end on **flat** faces, and the faces along it must meet smoothly: where fillets would
  have to mitre into each other at a corner (the rectangular end of a rectangle-to-
  circle loft) it fails to build and says so. Fillet such corners in the sketch.
- `shell` and `draft` work on freeform faces. A wall thicker than the tightest curve of
  a face fails ("would fold over itself"): use a thinner wall. A draft replaces a
  freeform face with straight lines tilted from the pull, through the curve where the
  face crosses the `neutral` plane, so the face must cross that plane along its whole
  width.
- These take from a few tenths of a second to about a second each.
- A cut or an add that crosses a freeform face takes a few tenths of a second, and fails
  if the two surfaces only touch along a curve instead of crossing: move one so they
  cross properly. The same goes for round faces that cross askew (a hole drilled across
  a hole of another size): they work where they cross cleanly.
- To select one, use `at`, `feature` and `side`, or `index`: `normal` alone finds flat
  faces only.
- An imported body has no fields to edit. Build on it with new features.

## Holes

`hole` drills at every **point** of its sketch: draw `point` items where the holes go
(the sketch is on the face to drill into). `standard` sets every size from a screw size
(`M2` to `M24`) and a fit (`close`, `normal`, `loose`, `tapped`); any size given as well
wins. `kind`: `simple`, `counterbore`, `countersink`. `end`: `through_all` or `blind`
with `depth`. The counterbore or countersink is at the face the sketch is on, and the
hole goes into the body from there. `peet feature feature=Hole1` shows the sizes a
standard gave (an M5 `normal` hole: 5.5 through, counterbore 10 wide and 5.4 deep).
Several points in one sketch make several holes in one feature; fix their positions
with dimensions or `fix`, as in any sketch.

## Fillet, chamfer, shell, draft

- `fillet` and `chamfer` take `edges` and `size`. A size too large for the faces next to
  the edge fails to build: reduce it. They are one kind of feature: `kind` says which,
  already set by the operation's name, so leave it out.
- `shell` hollows every body to `thickness`, removing the faces in `open` (none: a
  closed hollow).
- `draft` tapers `faces` (flat ones, round ones along the pull, or freeform ones) about
  a `neutral` plane by `angle`.

Each works on the bodies as they are at its place in the tree: a hole added after a
shell goes through the wall, one added before it is shelled with the body.

## Patterns and mirrors

`linear_pattern`, `circular_pattern` and `mirror` copy **features**, by name, not
bodies. They copy extrudes, cuts, revolves, holes, sheet metal cuts, forms, edge and
mitre flanges, tabs and break corners (see the `sheet-metal` skill); to repeat
a fillet, fillet all the edges in one feature. To copy whole bodies, use `move_body`
(below). `count` includes the original. A linear pattern becomes a grid with
`direction2`, `spacing2`, `count2`.

A **draft** is copied with the features whose faces it drafts: list those features and
then the draft, and each copy drafts the same faces of the copies. A draft of a face the
pattern doesn't copy fails ("which the pattern doesn't copy"): add the feature that made
that face to `features`.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 30}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 10}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Slot", "draw": [{"type": "rectangle", "from": [10, -3], "to": [20, 3]}]}
{"op": "cut", "sketch": "Slot", "depth": 6}
{"op": "draft", "faces": [{"at": [15, -3, 7]}, {"at": [15, 3, 7]}], "neutral": {"feature": "Extrude1", "side": "end"}, "angle": 5}
{"op": "circular_pattern", "features": ["Cut-Extrude1", "Draft1"], "axis": "z", "count": 4}
{"op": "bodies"}
```

**A pattern or a mirror can be copied too**, as SolidWorks copies one: a mirror of
`Mirror1` copies everything `Mirror1` has, its originals and their copies, across the
new plane, and a pattern of a pattern repeats the whole pattern. Name the pattern or
mirror in `features`, not what it copies (naming both copies the originals twice). A copy
that lands where nothing can be made (a cut off the part, a flange with no edge to go on)
is left out with a warning, as for any other copy.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 100]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "sketch", "name": "Seed", "on": {"feature": "Extrude1", "side": "end"}, "draw": [{"type": "circle", "center": [20, 15], "radius": 4}]}
{"op": "cut", "sketch": "Seed", "end": "through_all"}
{"op": "plane", "from": "right", "distance": 100, "name": "Across"}
{"op": "plane", "from": "front", "distance": 50, "flip": true, "name": "Along"}
{"op": "mirror", "name": "Pair", "features": ["Cut-Extrude1"], "plane": "Across"}
{"op": "mirror", "name": "Four", "features": ["Pair"], "plane": "Along"}
{"op": "linear_pattern", "name": "Middle row", "features": ["Pair"], "direction": "y", "spacing": 35, "count": 2}
{"op": "bodies"}
```

Here `Four` makes two holes (the mirrors of the seed and of its copy), so the plate has
four; `Middle row` copies both holes of `Pair` 35 along y, making six. Read the copies
off `faces`: each copy's faces are named after the feature that made it.

`count` and a grid's `count2` can follow parameters: use an expression when the
number of copies changes with a part size. Expressions return a unitless whole number
from 2 to 10000, including the original. A grid has at most 10000 instances in total.
Use `int` to round down deliberately; fractional results fail instead of being rounded.
Use `set_suppression_expression` when there should be no copies below a threshold.
A pattern with count 1 fails because it has no copies to build.

```jsonl
{"op": "set_parameter", "name": "width", "value": "299mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-50, -50], "to": [50, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 4}
{"op": "sketch", "name": "Seed", "on": {"feature": "Extrude1", "side": "end"}, "draw": [{"type": "circle", "center": [15, 0], "radius": 1}]}
{"op": "cut", "sketch": "Seed", "end": "through_all"}
{"op": "linear_pattern", "features": ["Cut-Extrude1"], "direction": "x", "spacing": 5, "count": "iif(width > 299mm, 3, 2)"}
{"op": "set_parameter", "name": "width", "value": "300mm"}
{"op": "edit", "feature": "LPattern1", "direction2": "y", "spacing2": 5, "count2": "int(width / 100mm)"}
{"op": "feature", "feature": "LPattern1"}
{"op": "bodies"}
```

`circular_pattern` accepts the same count expressions. Read `feature` for the stored
expression and its evaluated value; a failed count appears in `failures`. Fix its
parameter or `edit` the count, then check `bodies` again. Counts can differ between
configurations with `edit`'s `configurations`, just like spacing or an angle. A numeric
count clears its expression.

## Moving and copying bodies

`move_body` is SolidWorks' Move/Copy Body: it moves whole bodies, where the patterns
copy features. Use it to put a body made in a convenient place (a screw drawn on its own
axis) where it belongs, or to repeat a body that is not one feature.

- `bodies`: a face of each body (any selector); none moves every body. A moved body keeps
  its faces' names, so features after it still find what they picked.
- It turns first, by `rx`, `ry`, `rz` degrees about the X, Y and Z axes through `center`
  (the origin if absent; a reference point or a vertex), in that order, and then moves by
  `dx`, `dy`, `dz`. Positive angles turn anticlockwise, looking down the axis towards the
  origin. Check the result with `bodies`.
- `"copy": true` leaves the bodies where they are and adds `copies` moved copies (default
  1), the second moved twice as far, and so on. Each copy is a new body made by the move.
- A sheet metal body that is moved is a plain solid afterwards (the feature warns): move
  the sketches it is made from instead if it must keep its flat pattern.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
{"op": "sketch", "on": "front", "name": "Pin", "draw": [{"type": "circle", "center": [10, 30], "radius": 3}]}
{"op": "extrude", "sketch": "Pin", "depth": 12, "operation": "new_body"}
{"op": "move_body", "bodies": [{"feature": "Extrude2", "side": "end"}], "rx": -90, "dz": 10}
{"op": "move_body", "bodies": [{"feature": "Extrude1", "side": "end"}], "copy": true, "copies": 2, "dx": 50}
{"op": "bodies"}
```

Here the pin, drawn along -Y, is turned to stand along +Z (a turn of -90 about X takes
-Y to +Z) and lifted 10; the plate gets two copies, 50 and 100 along X.

## Another part's bodies

`insert_part` puts the bodies of another part's file into this one: SolidWorks' Insert
Part, and with `mirror` (a plane) its Mirror Part, the left-hand part made from the
right-hand one. `file` is the other part's `.peet` file; a relative path is looked for
from the working folder, then from this part's folder.

```sh
peet run bracket.jsonl --new -f bracket-rh.peet
peet op '{"op": "insert_part", "file": "bracket-rh.peet", "mirror": "right", "name": "Bracket LH"}' --new -f bracket-lh.peet
peet edit feature=Extrude1 depth=9 -f bracket-rh.peet    # change the original
peet op '{"op": "update_links"}' -f bracket-lh.peet       # the mirror image follows
```

- The bodies are read when `file` is set, as the other part builds them, and kept in this
  part: it still opens if the file is gone. `update_links` reads the file again (its
  reply's `updated` names the features that changed).
- Insert a part that builds without failures: one that doesn't is refused, with its
  failures.
- The inserted bodies have no fields but `file` and `mirror`: build on them with new
  features, or move them with `move_body`.
- Files on disk only: refused in the browser build.

## Structural members (weldments)

`structural_member` is SolidWorks' weldment structural member: a tube's or a beam's
section (`profile`, a sketch) swept along every line of the `paths` sketches, each line a
body of its own. Build a frame from lines, not from extrusions.

- Draw the **profile** with its origin where the line should run through it (the middle
  of a tube). It is turned square to each line by the smallest turn from its sketch plane:
  draw it on a plane square to the first line and that member has it as drawn; lines that
  run the same way get it the same way round. `rotate` turns it about every line.
- Draw the **paths** as lines (construction lines are left out) in as many sketches as the
  frame's planes need. Lines in other planes need a `plane` to sketch on.
- Where two lines of one feature meet end to end at an angle, the members are mitred
  (`corner`: `miter`, the default, or `plain` for square ends at the lines' ends).
- Make each **group** a feature of its own: a ring, then the legs. A line that ends on a
  member of an earlier structural member is trimmed to that member's outside (`trim`,
  default true), as SolidWorks trims one group against another. Three lines of one
  feature meeting at a point are not mitred: put them in different features.
- Check with `bodies`: one body per line. "trimmed to nothing" means a line lies inside an
  earlier member: leave it out, or give the feature `"trim": false`.

```jsonl
{"op": "sketch", "on": "right", "name": "SHS 40", "draw": [{"type": "rectangle", "from": [-20, -20], "to": [20, 20]}, {"type": "rectangle", "from": [-18, -18], "to": [18, 18]}]}
{"op": "sketch", "on": "top", "name": "Frame", "draw": [{"type": "rectangle", "from": [0, 0], "to": [400, 300]}]}
{"op": "structural_member", "profile": "SHS 40", "paths": ["Frame"]}
{"op": "sketch", "on": "front", "name": "Legs", "draw": [{"type": "line", "from": [0, 0], "to": [0, -300]}, {"type": "line", "from": [400, 0], "to": [400, -300]}]}
{"op": "structural_member", "profile": "SHS 40", "paths": ["Legs"]}
{"op": "bodies"}
```

Here the frame's four members meet in mitres at its corners, and the two legs (drawn
down from two corners on the front plane) start under the frame, at z = -20.

## Reference geometry

`plane` (offset `from` a plane by `distance`; or `from` + `about` + `angle`; or halfway
between `a` and `b`), `axis`, `point`, `coordinate_system`. Use a reference plane to
sketch where there is no face.

## Material and colour

A part has one material: a name and a density. `mass` gives `mass_kg` only once it has
one.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 100]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 10}
{"op": "set_material", "material": "Mild steel"}
{"op": "set_color", "color": "#3366cc"}
{"op": "mass"}
```

- A material of the tables (`peet materials` lists them, with `density_kg_m3`) needs
  only its name. Any other needs its density in kg/m³:
  `{"op": "set_material", "material": "Brass", "density": 8500}`. Without one the
  operation fails and lists the materials that have a density.
- On a sheet metal part use `apply_material` instead (`peet skills sheet-metal`): it sets
  the thickness and bends from a gauge and the part's material in one step.
- `"material": null` and `"color": null` clear them.

## Checking

`bodies` (size, volume), `mass` (centre of gravity, and the mass once the part has a
material), `measure` (one face, edge or vertex, or the distance and angle between two). Features are built in tree order, each
on the result of those above it: `move` and `rollback` change that order.
