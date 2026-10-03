---
name: solids
description: Solid features in peet - extrude and cut directions and end conditions, revolve, sweep, holes, fillets and chamfers, shell, draft, patterns and mirrors.
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

## Sweep

`sweep` takes two sketches: `profile` (closed regions) and `path` (connected lines and
arcs in another sketch, starting on the profile's plane).

## Holes

`hole` drills at every **point** of its sketch: draw `point` items where the holes go
(the sketch is on the face to drill into). `standard` sets every size from a screw size
(`M2` to `M24`) and a fit (`close`, `normal`, `loose`, `tapped`); any size given as well
wins. `kind`: `simple`, `counterbore`, `countersink`. `end`: `through_all` or `blind`
with `depth`.

## Fillet, chamfer, shell, draft

- `fillet` and `chamfer` take `edges` and `size`. A size too large for the faces next to
  the edge fails to build: reduce it.
- `shell` hollows every body to `thickness`, removing the faces in `open` (none: a
  closed hollow).
- `draft` tapers flat `faces` about a `neutral` plane by `angle`.

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
