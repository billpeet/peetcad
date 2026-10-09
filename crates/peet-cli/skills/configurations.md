---
name: configurations
description: Configurations in peet - several versions of one part in one file (sizes, with and without features), giving a size, a sketch dimension or a parameter another value in some versions, switching between them, and exporting each.
---

# Configurations

A part has one or more **configurations**: versions of it that differ in which features
are suppressed and in its numbers: parameters, the numeric values of features (a depth, an
angle, a hole's diameter, a pattern's count) and the dimensions of sketches. A new part has one, `Default`.
Exactly one is **active**, and every other operation reads, builds, measures and exports
the active one.

```jsonl
{"op": "set_parameter", "name": "t", "value": "8mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": "t"}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Holes", "draw": [{"type": "circle", "center": [15, 25], "radius": 4}]}
{"op": "cut", "sketch": "Holes", "end": "through_all"}
{"op": "add_configuration", "name": "Thick"}
{"op": "set_parameter", "name": "t", "value": "12mm"}
{"op": "add_configuration", "name": "Blank", "copy": "Default"}
{"op": "suppress", "feature": "Cut-Extrude1"}
{"op": "configurations"}
{"op": "configuration", "configuration": "Default"}
{"op": "bodies"}
```

## Making one

- `add_configuration` makes a copy of the active configuration (or of `copy`) **and
  makes it active**, so the operations after it change the new one.
- What can differ: whether a feature is suppressed, a parameter's value, a feature's
  numeric values, and a sketch's dimensions.
- Everything else is the part's, in every configuration: a new feature, what is drawn in
  a sketch, a new parameter, and the fields of a feature that aren't numbers (a
  direction, an end condition, the edges of a fillet).

## Which configurations a change applies to

`suppress`, `set_parameter`, `edit` and `set_dimension` take `configurations`:

- `"this"`: the active configuration only;
- `"all"`: every configuration, and the value no longer differs between them;
- a name or a list of names: those, without making them active.

**Left out, it depends on the operation.** `suppress` and `set_parameter` mean `"this"`.
`edit` and `set_dimension` change a value that already differs between configurations in
the active one, and a value that doesn't in all of them: so say `"this"` (or name the
configurations) the first time a size is to differ, and nothing after that.

With one configuration there is nothing to choose.

## Sizes that differ

A size differs in one of two ways. Use a parameter when several sizes go together (a
thickness that features and sketches use); set the value itself when it is one number.

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 80, "name": "width"}, {"type": "length", "of": ["r.right"], "value": 50}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 8}
{"op": "add_configuration", "name": "Long"}
{"op": "set_dimension", "sketch": "Sketch1", "name": "width", "value": 120, "configurations": "this"}
{"op": "edit", "feature": "Extrude1", "depth": 12, "configurations": ["Long"]}
{"op": "configurations"}
{"op": "bodies"}
```

- A sketch dimension can only be set if it has a name to call it by: give dimensions a
  `name` when drawing, or read the automatic ones (`d1`, `d2`) with `feature`.
- `edit` with `configurations` other than `"all"` takes numeric fields only. A field
  that isn't a number is refused there with the reason: change it in an `edit` of its
  own, without `configurations`.
- `configurations` shows what differs under `values`, by feature and by the value's
  name, as it would be typed (`"2 * t"` for an expression). An expression is evaluated
  with each configuration's own parameters.

## What is suppressed with a feature

Everything built on a suppressed feature is suppressed with it (the sketch on its face,
the cut from that sketch) and comes back when it is unsuppressed. `features` shows those
with `suppressed_by`. They are not failures.

## Suppression driven by a size

Use `set_suppression_expression` when a feature should appear or disappear as a
parameter changes. The rule belongs to the part and reads the active configuration's
parameters on every rebuild. Zero builds the feature; a finite nonzero plain number
suppresses it and its dependents. This rule controls suppression while it is set.
Manual flags, including changes made with `suppress`, stay saved for use after clearing
it with `value: null`. `suppressed_in` lists those saved manual flags.

```jsonl
{"op": "set_parameter", "name": "width", "value": "300mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 10]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 4}
{"op": "sketch", "name": "Vent", "on": {"feature": "Extrude1", "side": "end"}, "draw": [{"type": "circle", "center": [5, 5], "radius": 1}]}
{"op": "cut", "sketch": "Vent", "end": "through_all"}
{"op": "set_suppression_expression", "feature": "Vent", "value": "iif(width > 300mm, 0, 1)"}
{"op": "add_configuration", "name": "Wide"}
{"op": "set_parameter", "name": "width", "value": "400mm"}
{"op": "features"}
{"op": "configuration", "configuration": "Default"}
{"op": "features"}
{"op": "set_suppression_expression", "feature": "Vent", "value": null}
```

Inspect the rule and effective state with `feature`; `manual_suppressed` is the saved
flag. A malformed expression is refused. An unknown name, invalid unit or evaluation
error makes the feature fail at rebuild, and appears in `failures`. Fix the rule or
its parameter, or clear it. Check each configuration, since their parameter values
can select different branches. Translate SolidWorks suppression strings to numeric
results: `"suppressed"` becomes 1 and `"unsuppressed"` becomes 0. Use a named parameter
for a sketch size that a suppression rule needs to read.

## Checking and exporting each

`configurations` lists them, the active one, and what differs. To check or export one,
make it active first, in the same script:

```jsonl
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "set_parameter", "name": "t", "value": "3mm"}
{"op": "extrude", "sketch": "Sketch1", "depth": "t"}
{"op": "add_configuration", "name": "Thick"}
{"op": "set_parameter", "name": "t", "value": "6mm"}
{"op": "bodies"}
{"op": "configuration", "configuration": "Default"}
{"op": "bodies"}
```

Put an `export` where each `bodies` is to write a file per configuration.

## What the replies don't say

- **The file remembers the active configuration.** A call that ends in another
  configuration saves the part that way, and the next call starts there. Check with
  `status` (`configuration`), and end a script in the configuration the part should
  open in.
- **`configuration` is not an undo step.** `undo` takes back the last change to the
  part, and goes back to the configuration that change was made in.
- **`failures` are the active configuration's.** A part is right when every
  configuration builds: switch to each and read the reply.
- **A parameter must evaluate in every configuration it is set in.** An error names the
  configuration (`in configuration Thick`): an expression may use a parameter that has
  another value there.
- **A part keeps at least one configuration.** Deleting the active one makes its
  neighbour active; the reply's `configuration` says which.
