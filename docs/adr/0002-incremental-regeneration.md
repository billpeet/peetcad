# ADR 0002: Incremental regeneration by content keys

**Status:** accepted (Phase 3)

## Context

A parametric model is rebuilt after every edit. The budget is 100 ms for a 20-feature
part after a change to its first sketch, and edits further down should cost almost
nothing. Tracking "what changed" by hand (dirty flags set by every editing operation)
is fragile: one forgotten flag gives a stale model.

## Decision

`peet_model::Engine` hashes each feature's *inputs* into a key and remembers each
feature's last key and result. Implemented in `peet-model/src/regen.rs`.

- A feature's inputs are its own definition, the parameter table, the output keys of the
  features it refers to, and for solid features the key of the body state it builds on
  (the key of the last solid feature that changed the bodies).
- Keys are FNV-1a hashes of the `postcard` encoding, so they depend only on content and
  are the same in every run and on every platform.
- A feature whose key is unchanged reuses its result. Nobody has to say what changed, and
  undo or redo to a state the engine has seen is free for the unchanged features.
- Body states are shared (`Arc<Body>`): a feature that doesn't touch a body passes the
  same pointer on, and each body carries a content stamp that the UI uses to cache
  tessellations and GPU meshes.
- Sketch solutions and resolved sketch planes are written back into the model, so the
  solver always starts from the last solution and a sketch whose face is gone can still
  be drawn and moved.
- Undo is snapshot based. The model keeps its features behind `Arc`s, so a snapshot
  costs a few pointers plus the features that changed (`peet_model::History`).

## Consequences

- Measured on the 20-feature sample bracket (`cargo bench -p peet-model`, release):
  about 5 ms after a first-sketch change, 0.02 ms when nothing changed.
- Hashing a large sketch on every rebuild costs microseconds. If it ever shows up in
  profiles, a per-feature revision counter can short-cut it without changing the design.
- The cache keeps one result per feature. Alternating between two states recomputes;
  a small per-feature LRU would fix that if it matters.
