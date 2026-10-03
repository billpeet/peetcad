# ADR 0009: Configurations

**Status:** accepted (configurations stages 1 and 2, see [the plan](../configurations-plan.md))

## Context

A part should hold named configurations, as in SolidWorks: versions that differ in which
features are suppressed, in their dimensions and, in the end, in anything a feature can
set. Stage 1 covers suppression and the values of parameters.

Two facts about the code decided the layout. Everything reads the model directly: the
rebuild engine, the queries, the exports and every panel of the application read
`doc.model`, its features and its parameter table. And the application's tools change
the part by editing a copy of the model, which is then translated into operations
([ADR 0007](0007-operations.md)).

## Decision

**The model always holds the active configuration.** A feature's `suppressed` flag and
the parameter table are what the active configuration says. Nothing that reads the model
knows configurations exist, and the engine is unchanged: it is handed an ordinary part.
Implemented in `peet-model/src/config.rs`.

**What differs is kept on the side.** For each feature whose suppression differs between
configurations, and each parameter whose expression does, the model keeps the value in
every configuration *other than* the active one. Making another configuration active
swaps those values with the model's. A value that isn't listed is the same everywhere,
and a value that becomes the same everywhere is no longer listed. A part without
configurations is a part with one (`Default`) and nothing listed.

The plan first proposed the other way round: shared values in the model and each
configuration's differences resolved into a second model before a rebuild. That needs
every reader of `doc.model` to read the resolved model instead, and a missed one shows
another configuration's value without any error. Holding the active configuration needs
no reader to change.

**Any configuration is an ordinary part.** `Model::with_configuration` gives the model
with another configuration active, without touching the one shown. It costs a few
pointers (features are shared), and is what exports of other configurations will use.

**A change has a scope**: this configuration (the default), all of them, or the ones
named. `suppress` and `set_parameter` take it as `configurations`. With one configuration
every scope is the same thing and nothing is listed. A new parameter and a new feature
exist in every configuration.

**A change made to the model itself** (a tool's edit of its copy) applies to the active
configuration where the value already differs, and to every configuration where it
doesn't; the translation into operations says so explicitly (`"this"` or `"all"`). The
application's own suppress commands and the configuration columns of the parameters
window send operations with their scope directly.

**What is built on a suppressed feature is suppressed with it**, and comes back with it
(`Status::SuppressedBy`). Before, it failed with a message. With features suppressed in
some configurations and not others this is the normal case, not an error.

**Making a configuration active is not an undo step**, like the flat pattern view. Undo
steps are snapshots of the whole model, which includes the active configuration, so
undoing a change returns to the configuration it was made in.

**The file format** is model schema 5. The model's layout changed, which `postcard`
can't absorb, so earlier models are read as `ModelV4` (a frozen copy of the old layout)
and converted. This is the first migration; `peet-io/tests/migration.rs` opens the four
samples as saved with schema 4. The caches hold the active configuration.

## Stage 2: values of features and dimensions of sketches

**A value that can differ is a slot**: a numeric field of a feature, by the name its
operations give it (`depth`), or a driving dimension of a sketch, by its id. The model
lists a feature's slots (`FeatureKind::slots`, written once for reading and for writing)
and keeps the slots that differ in one more table, swapped on activation like the
others. Slots are addressed in the model, not through the field tables of `peet-ops`,
because the swap happens in the model.

Whole definitions of a feature per configuration were considered and rejected: a change
to a field that doesn't differ would then reach the active configuration only, unless
every edit knew to copy it to the others. With a table per value, a value that isn't
listed is the same everywhere by construction, as in stage 1. It is also what a sketch
needs: what is drawn is the part's, and only its dimensions differ.

**The table's values are an enum** (`Held`), with one kind so far. Stage 3 adds kinds
(a count, a choice) at its end without changing the layout again.

**Values are compared as entered**: two expressions are the same if their text is, even
if they evaluate differently because the parameters differ.

**The default scope of `edit` and `set_dimension`** is that of a change to the model:
the active configuration for a value that already differs, all of them for one that
doesn't. An edit of a part with configurations then does what it did before there were
any, until a value is deliberately made to differ. `suppress` and `set_parameter` keep
`"this"`, as in stage 1 and as SolidWorks suppresses.

**Only numeric fields differ.** An `edit` with a scope other than all that changes
another field is refused, rather than applied everywhere without saying so. A tool's
change is split by the translation: its values with the scope asked for, its other
fields without one.

**A value given is set where it was asked**, even if the active configuration already
has it: the operation works out which values its fields set, not only which changed.

**The file format** is model schema 6 (one more table in the configurations). Schema 5
is read through `ModelV5`; `fixtures/v5` holds a part with three configurations.

## The open questions of the plan

1. *Where solved sketches go.* Into the model, as before: it holds the active
   configuration, so its sketches hold that configuration's solution. Switching is the
   same as editing the parameters by hand, which already re-solved from the last
   solution. The exit test switches round the enclosure's configurations twice and gets
   the same flat patterns each time. A sketch whose solver flips on a large change of
   size would do so on a parameter edit too; it is not a matter for configurations.
2. *Reference tie-breaks.* The same: a reference picked at one size resolves at another
   exactly as it does after a parameter edit. Nothing new was needed.
3. *Switching and undo.* As proposed, above.

## Consequences

- Switching rebuilds only what differs, through the engine's keys. The engine remembers
  one result per feature, so switching back recomputes; the plan's stage 4 adds a small
  cache.
- A solved sketch and its placement are stored for the active configuration only, so the
  model of one configuration made from another with `with_configuration` has the other's
  sketch positions until it is rebuilt.
- Switching marks the document modified: the active configuration is saved with it.
- A file with caches saved by an earlier version rebuilds once on opening (its caches are
  tagged with the hash of the old layout).
- Stages 2 and 3 add more kinds of value to what is kept on the side (a feature's
  fields, a sketch's dimensions). The swap on activation and the scopes stay as they are.
- A tool that edits a value which differs changes this configuration only. The
  properties panel says which configurations a change of value goes to, and the table
  shows what differs; the panels themselves don't mark a value that differs.
- A value that ends up the same in every configuration is no longer listed, so it is
  not remembered as "configured": the next change with the default scope is to all.
