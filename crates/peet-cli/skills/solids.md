---
name: solids
description: Solid features in peet - extrude and cut directions and end conditions, revolve, sweep, loft, holes, fillets and chamfers, shell, draft, patterns and mirrors, freeform faces and imported STEP bodies.
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
  `"through_all"`, or `{"up_to": plane}`.
- `operation`: `"add"` joins the bodies it touches (and is a new body if it touches
  none), `"new_body"` always makes a separate body, `"cut"` removes.
- A sketch with several regions extrudes all the outer ones, with regions inside them as
  holes. `regions` picks others: a list of sketch points, one inside each region wanted.

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
bodies. They copy extrudes, cuts, revolves, holes, sheet metal cuts and forms; to repeat
a fillet, fillet all the edges in one feature. `count` includes the original. A linear
pattern becomes a grid with `direction2`, `spacing2`, `count2`.

## Reference geometry

`plane` (offset `from` a plane by `distance`; or `from` + `about` + `angle`; or halfway
between `a` and `b`), `axis`, `point`, `coordinate_system`. Use a reference plane to
sketch where there is no face.

## Checking

`bodies` (size, volume), `mass` (centre of gravity), `measure` (one face, edge or
vertex, or the distance and angle between two). Features are built in tree order, each
on the result of those above it: `move` and `rollback` change that order.
