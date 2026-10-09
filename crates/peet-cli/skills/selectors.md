---
name: selectors
description: Pointing at existing geometry in peet - planes, faces, edges and vertices for a sketch on a face, a flange or fillet edge, a reference plane or a measurement.
---

# Selectors

A selector says which plane, face, edge or vertex of the part an operation means. It
must match **exactly one** thing. If it matches none or several, the operation is not
applied and the error lists the candidates with their positions: add a field to settle
it and run again.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 8}
{"op": "faces"}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "draw": [{"type": "circle", "center": [40, 25], "radius": 5}]}
{"op": "fillet", "size": 3, "edges": [{"between": [[0, 0, 0], [0, 0, 8]]}, {"at": [80, 0, 4]}]}
{"op": "plane", "from": {"at": [10, 10, 8], "normal": [0, 0, 1]}, "distance": 20}
{"op": "measure", "a": {"face": {"normal": [1, 0, 0]}}, "b": {"face": {"normal": [-1, 0, 0]}}}
```

## What each field takes

| Field wants | Write |
|---|---|
| a plane | `"top"`, `"front"`, `"right"`, a reference plane's name, or a face selector for a flat face |
| an axis | `"x"`, `"y"`, `"z"`, a reference axis's name, or an edge selector |
| a point | `"origin"`, a reference point's name, or a vertex selector |
| a face | `{"feature": "Extrude1", "side": "end"}` · `{"at": [x, y, z]}` · `{"normal": [x, y, z]}` · `{"body": 0, "index": 5}` |
| an edge | `{"between": [[x, y, z], [x, y, z]]}` · `{"at": [x, y, z]}` · `{"faces": [face, face]}` · `{"body": 0, "index": 7}` |
| a vertex | `{"at": [x, y, z]}` · `{"body": 0, "index": 2}` |

Coordinates are **model** coordinates, in the part's units. Fields combine: every one
given must match.

## Choosing a selector

1. **By what made it**, when a feature made the face: `{"feature": "Extrude1", "side":
   "end"}`. It reads clearly and survives size changes. Sides: `start`, `end`, `side`
   (an extrusion's two caps and its walls); `top`, `bottom`, `bend`, `wall` (sheet
   metal); `blend` (a fillet's or chamfer's own face); `inner` (the inside of a shell).
   A revolve all the way round has no caps: **every** face of it is a `side`, flat
   ones included, so add `normal` or `at` (`{"feature": "Revolve1", "normal": [0, 0,
   1]}`). When `feature` and `side` match nothing, the error lists the faces that
   feature made, with the side of each.
2. **By position**, when you know where it is: a face `at` a point on it, an edge
   `between` its two ends.
3. **By listing**, when you don't: `peet faces` and `peet edges` give every face and
   edge with its normal and where it is. Each face has `what` (in words: "the top face
   of Wall3") and `made_by` (the same as `feature` and `side` values to put in a
   selector). Or pick from the list by `body` and `index`, in the same call or the next
   (an index is only good until the part changes). A face's `center` there is a point on or near the face for telling faces
   apart, not its centroid: on a ring-shaped face it is off to one side.

## Things that go wrong

- **A point on an edge is on two faces.** `{"at": [10, 0, 8]}` is ambiguous: add
  `"normal"` (`[0, 0, 1]` for the face that looks up), or use a point inside the face.
- **A point on a corner or a seam is on several edges.** A round face has a seam: a
  straight edge down it, which meets the circles at its ends. An `at` point there
  matches both the circle and the seam. Use a point further round the circle, such as
  the opposite side (`[0, 22, 63]` in place of `[22, 0, 63]`).
- **`normal` alone** matches flat faces only, and every flat face that looks that way. On
  a body with several, add `at` or `feature`. For a round or freeform face use `at`,
  `feature` and `side`, or `index`.
- **`side` matches every face of that kind**: an extrusion of a rectangle has four
  `side` faces. Add `at` or `normal`.
- **A sketch needs a flat face.** A curved one is refused, saying so.
- **Ends moved.** After a fillet or a neighbouring flange, an edge's ends are no longer
  at the old corner. List the edges again and use the new ends, or use `at` with a point
  in the middle of the edge.

A selector is resolved once, when the operation is applied, and stored as a reference
that follows the geometry: changing a size later moves the sketch or the flange with its
face or edge.

## User names

In the UI, select one face or edge in a part's viewport and use Properties > Names to
add or remove names. With nothing selected, Properties lists all names so unavailable
bindings can be removed too. These actions run the operations below.

Assign names when later scripts need a particular face or edge, such as a mating face
or a hinge hole. First build the geometry and select it with the usual selectors.
`name_face` accepts flat and curved faces; `name_edge` accepts edges. Use
`{"name": "Front"}` wherever a face selector is accepted, including sketch planes
and mate ends. A named curved face still cannot be a sketch plane. Names are
case-sensitive and unique across faces and edges in a part. Several names can point
to the same geometry. `faces` and `edges` report the names in their `names` arrays.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 8}
{"op": "name_face", "face": {"feature": "Extrude1", "side": "end"}, "name": "Front"}
{"op": "name_edge", "edge": {"between": [[0, 0, 0], [0, 0, 8]]}, "name": "Corner"}
{"op": "edit", "feature": "Extrude1", "depth": 12}
{"op": "sketch", "on": {"name": "Front"}, "draw": [{"type": "circle", "center": [40, 25], "radius": 5}]}
{"op": "measure", "a": {"edge": {"name": "Corner"}}}
{"op": "faces"}
{"op": "delete_name", "name": "Corner"}
```

Names are saved with the part, shared by all configurations, and support undo and
redo. They use the existing persistent references: feature origins, neighbouring
faces and a position hint. After a split, the reference chooses the best matching
remaining face or edge. A name does not label every fragment or pattern copy.
If the target disappears, is suppressed or lies below rollback, selection fails.
Restore the geometry, or inspect `faces` and `edges`, remove the binding with
`delete_name`, and assign it again. Removing a name leaves geometry and references
already made by sketches or mates intact.
