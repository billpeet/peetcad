---
name: configurations
description: Configurations in peet - several versions of one part in one file (sizes, with and without features), switching between them, and exporting each.
---

# Configurations

A part has one or more **configurations**: versions of it that differ in which features
are suppressed and in the values of its parameters. A new part has one, `Default`.
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
- Only two things can differ: whether a feature is suppressed, and a parameter's value.
  To make a size differ, drive it by a parameter (`"depth": "t"`), then give the
  parameter another value. A size typed as a number is the same in every configuration.
- Everything else is the part's, in every configuration: a new feature, an `edit`, a
  sketch, a new parameter.

## Which configurations a change applies to

`suppress` and `set_parameter` take `configurations`:

- left out, or `"this"`: the active configuration only;
- `"all"`: every configuration, and the value no longer differs between them;
- a name or a list of names: those, without making them active.

With one configuration there is nothing to choose.

## What is suppressed with a feature

Everything built on a suppressed feature is suppressed with it (the sketch on its face,
the cut from that sketch) and comes back when it is unsuppressed. `features` shows those
with `suppressed_by`. They are not failures.

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
