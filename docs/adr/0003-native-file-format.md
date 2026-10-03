# ADR 0003: The `.peet` file format

**Status:** accepted (Phase 3). Format version 1.

## Context

The roadmap asks for a compact file that opens instantly, degrades gracefully across
versions and never crashes on bad input, with a readable form for diffs.

## Decision

- **Container** (`peet-io/src/peet.rs`, byte layout documented there): a 16-byte header
  (magic `PEET`, format version, table checksum) and a table of typed sections. Each
  section is `postcard` encoded and LZ4 compressed on its own (stored raw if that is
  smaller), has its own schema version and a CRC-32. Sections are decoded lazily, unknown
  kinds are skipped, and every length and offset is checked so damaged files give an
  error rather than a panic or a huge allocation (property tested with mutated files).
- **Contents** (`peet-io/src/document.rs`): metadata, the parametric model (the source
  of truth), and optional B-rep and mesh caches tagged with the hash of the model they
  were built from. Caches that don't match are ignored. With caches the part is shown at
  once and rebuilt on the next frame; the cached meshes save tessellating.
- **Text form:** `peetcad dump part.peet part.ron` and `peetcad pack part.ron part.peet`
  convert to and from RON (metadata and model only).
- **Autosave** writes the part without caches to the platform blob store (a file under
  the user's data folder natively, IndexedDB in the browser) while there are unsaved
  changes, and offers it at the next start if the app closed without saving.

## Consequences

- The 20-feature sample bracket is 1.3 KB without caches and 23 KB with them.
- Any change to a serialized model type is a format change: bump that section's schema
  version and keep reading the old one. Migration tests start with the first released
  file.
- The first such change was model schema 5 (configurations): earlier models are read
  through a frozen copy of their layout and converted, with tests against files of the
  samples saved with schema 4 ([ADR 0009](0009-configurations.md)).
- The model section uses `postcard`, which is not self-describing, so fields can't be
  added without a schema version bump. That is deliberate: the versioning is explicit.
