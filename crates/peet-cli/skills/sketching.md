---
name: sketching
description: Drawing in a PeetCAD sketch with peet - shapes, relations, dimensions, entity labels and ids, and which way the sketch axes point.
---

# Sketching

A sketch lies on a plane and holds 2D geometry. Features (extrude, base flange, hole)
are made from it. `sketch` adds one and draws in it; `draw` adds to an existing one.
Both take a `draw` list, applied in order. `peet ops draw` lists every item type.

```jsonl
{"op": "sketch", "on": "top", "name": "Profile", "draw": [{"type": "polyline", "points": [[0, 0], [60, 0], [60, 40], [0, 40]], "closed": true, "as": "p"}, {"type": "horizontal", "of": ["p.0"]}, {"type": "vertical", "of": ["p.1"]}, {"type": "length", "of": ["p.0"], "value": 60}, {"type": "circle", "center": [30, 20], "radius": 6, "as": "hole"}, {"type": "diameter", "of": ["hole"], "value": 12}]}
{"op": "draw", "sketch": "Profile", "draw": [{"type": "line", "from": [30, -5], "to": [30, 45], "construction": true}]}
{"op": "set_dimension", "sketch": "Profile", "name": "d1", "value": 70}
{"op": "feature", "feature": "Profile"}
```

## Coordinates

Points are `[x, y]` **in the sketch plane**, not in the model. The reply to `sketch`
gives the plane: its `origin`, and where its `x`, `y` and `normal` point in the model.

| Sketch on | x is model | y is model | normal |
|---|---|---|---|
| `"top"` | X | Y | Z |
| `"front"` | X | Z | −Y |
| `"right"` | Y | Z | X |
| a face | read `plane` in the reply | | out of the material |

A sketch on a face has its origin where the model's origin projects onto the face. On a
face that looks up (normal +Z) a model point `[x, y, z]` is sketch point `[x, y]`; on
one that looks down, y is reversed. For any other face, work it out from `plane`.

## Referring to entities

- Give geometry a label with `"as"`, then use it later **in the same list**: `"a"`,
  `"a.start"`, `"a.end"`, `"c.center"`, `"origin"`.
- A shape's parts: a rectangle's `r.bottom`, `r.right`, `r.top`, `r.left`; any shape's
  curves by number, `p.0`, `p.1`; then `.start` and `.end` of those (`r.bottom.start`).
- Labels end with the operation. Afterwards use **ids**: each reply's `drawn` gives the
  ids of what an item made, and `peet feature feature=SKETCH` lists every entity with
  its id and coordinates. Ids work with parts too: `"12.start"`.

## Closed shapes

Extrude, cut and base flange use the sketch's **closed regions**.

- `rectangle`, `center_rectangle`, `slot`, `polygon`, `circle`, a `polyline` with
  `"closed": true` and a `spline` with `"closed": true` are closed and stay closed.
- Separate `line` items that happen to meet are not joined. Join their ends with
  `coincident`, or draw a `polyline`.
- A region inside a region is a hole in it (a circle inside a rectangle).
- `"construction": true` geometry helps relations and is ignored by regions: use it for
  centrelines and mirror axes.

## Splines

`spline` draws a smooth curve **through** its `points`, in order; with `"closed": true`
it returns to the first point (don't repeat it).

```jsonl
{"op": "sketch", "on": "top", "name": "Blade", "draw": [{"type": "spline", "points": [[0, 0], [15, 12], [40, 14], [60, 0]], "as": "s"}, {"type": "line", "from": [60, 0], "to": [0, 0], "as": "l"}, {"type": "coincident", "of": ["s.end", "l.start"]}, {"type": "coincident", "of": ["s.start", "l.end"]}]}
{"op": "extrude", "sketch": "Blade", "depth": 5}
```

- The points are the spline's only handles: `s.start`, `s.end`, and `s.0`, `s.1`, … by
  number. They take `coincident`, `fix` and dimensions like a line's ends. To change the
  shape, move a point or add one; there are no tangent handles, and a spline takes no
  `tangent` relation.
- Nothing can be put on the curve between its points. To join another curve to it, make
  their end points `coincident`.
- `trim`, `extend`, `offset` and `fillet` refuse splines; `mirror` copies them.
- A profile with a spline extrudes, revolves (keep the spline clear of the axis) and
  lofts (against lines and other splines, not arcs), and a spline can be a sweep's path.
  Sheet metal features refuse it: flat patterns are lines and arcs.

## Relations and dimensions

- Relations and dimensions take `"of"`: a list of entities. `peet ops draw` says what
  each wants (a `length` takes a line, a `distance` two points or a point and a line).
- A dimension's `value` is a number or an expression (`"width / 2"`). It gets a name
  (`d1`, `d2`, or your `name`), which other dimensions and `set_dimension` use.
- **What is still free:** when a sketch is `under_defined`, `peet feature feature=SKETCH`
  marks each entity that can still move with `"free": true`. Dimension or relate those
  (often it is the far end of a construction line).
- A dimension is a size, not a coordinate: it has no sign. Geometry stays on the side
  it was drawn, so a point drawn at x = −42 with a `horizontal_distance` of 42 to the
  origin stays at −42.
- A dimension is greater than 0. To put two things in line, use a relation:
  `horizontal` or `vertical` of two points, or `coincident`. To centre a
  `center_rectangle` on the origin: `{"type": "coincident", "of": ["r.center",
  "origin"]}`.
- Geometry is drawn where you put it and a sketch need not be fully defined: an
  `under_defined` sketch builds as drawn. Add dimensions to what must stay exact or
  follow a parameter.
- The reply's `definition` must not be `over_defined`: that is two relations or
  dimensions that contradict each other (the sketch's status is then `warning`). Take
  one out with `{"type": "remove", "dimensions": ["d2"]}` (or `"relations": [ids]`; the
  ids are in the reply that made them and in `peet feature`).

## Editing what is drawn

`fillet` (a corner point and a radius), `trim` and `extend` (a curve and a point `near`
the piece), `offset`, `mirror` (entities and an `axis` line), `construction`, `delete`
(entities), `remove` (relations and dimensions). After an edit, ids of the pieces may be
new: read them from the reply.

## Relations to the model

Use `project` before relating or dimensioning sketch geometry to a model edge,
vertex, or reference plane. Build the source feature above the sketch first. Pick
edges and vertices with the selectors skill; select a plane by its feature name or
`front`, `top`, `right`. The projection is locked construction geometry and follows
upstream edits, configurations, and sketch placement changes. Existing relations
and dimensions accept it by label or id. A plane supplies its intersection line
with the sketch, so use `coincident` for a point on the plane and `distance` for its
offset. Parallel planes have no intersection and are refused.

For converted edges, set `convert` to true. The locked curves then participate in
profiles. Projecting a face gives its boundary edges, including hole loops; address
them by number, such as `outline.0.start`. The application exposes the supporting face's boundary as reference geometry when
opening a sketch for editing. Scripts use `project` explicitly after creating the sketch
on the face; those references work the same way.

Straight edges project unless viewed end-on. Circular edges must be parallel to the
sketch plane; tilted circles, ellipses and freeform edges are refused. Pick supported
edges individually when a face includes an unsupported boundary. A missing source
fails the sketch; suppression of a source suppresses its dependent sketches. Replace
a projection with `delete` of its entity id, then project the replacement and recreate
its relations. Use `feature` to inspect `projections`, locked entities, and failures.

`collinear` adds parallel and point-on-line relations; `coradial` adds concentric and
equal-radius relations. Remove both returned relation ids to remove either composite
relation. `equal` already covers equal line lengths, `midpoint` places a point at a
projected edge's midpoint, and `horizontal` or `vertical` accepts two points.
`doubled_distance` measures twice a point's perpendicular distance to a centreline;
its named value and expressions use the full doubled size.

```jsonl
{"op":"sketch","on":"top","name":"Stock profile","draw":[{"type":"rectangle","from":[0,0],"to":[40,30]}]}
{"op":"extrude","sketch":"Stock profile","name":"Stock","depth":5}
{"op":"sketch","on":{"feature":"Stock","side":"end"},"name":"Located","draw":[{"type":"project","edge":{"between":[[40,0,5],[40,30,5]]},"as":"edge"},{"type":"project","vertex":{"at":[40,30,5]},"as":"corner"},{"type":"project","plane":"right","as":"axis"},{"type":"point","at":[35,27],"as":"p"},{"type":"distance","of":["p","edge"],"value":5},{"type":"vertical_distance","of":["p","corner"],"value":3},{"type":"doubled_distance","of":["p","axis"],"driven":true,"name":"full_width"}]}
{"op":"sketch","on":{"feature":"Stock","side":"end"},"name":"Converted","draw":[{"type":"project","face":{"feature":"Stock","side":"end"},"convert":true,"as":"outline"}]}
{"op":"extrude","sketch":"Converted","depth":2}
{"op":"feature","feature":"Located"}
```

```jsonl
{"op":"sketch","on":"top","name":"Relations","draw":[{"type":"line","from":[0,0],"to":[10,0],"as":"a","construction":true},{"type":"fix","of":["a"]},{"type":"line","from":[2,2],"to":[8,3],"as":"b","construction":true},{"type":"collinear","of":["a","b"]},{"type":"circle","center":[20,20],"radius":3,"as":"c","construction":true},{"type":"fix","of":["c"]},{"type":"circle","center":[21,21],"radius":4,"as":"d","construction":true},{"type":"coradial","of":["c","d"]}]}
```
