# Working in this repository

## Every feature is scriptable, and the agent skills say how

PeetCAD is driven by agents through `peet` (`crates/peet-cli`), which applies
**operations** (`crates/peet-ops`). The skills that teach an agent to use it are in
`crates/peet-cli/skills/` and are built into the binary (`peet skills`). A feature that
an agent can't reach, or doesn't know about, is not finished.

When you add or change a feature, a command, or anything a user can do to a part:

1. **Operation.** The compiler makes you give a new feature kind a field table
   (`crates/peet-ops/src/fields.rs`) and a new command an operation
   (`coverage` in `crates/peet-ui/src/app/scripting.rs`). Anything else a user can do
   (a new option, a new query, a new file format) gets an operation or a field too.
2. **Skill.** Add it to the skill it belongs to in `crates/peet-cli/skills/` (`solids`,
   `sheet-metal`, `sketching`, `selectors`; `core` for anything every run needs). Write
   what `peet ops NAME` can't tell an agent: when to use it, what it needs first, which
   way it goes, how it fails and what to do then. Include a `jsonl` script that uses it:
   a test runs every script in the skills from an empty part.
   If it opens a new area, add a skill file, list it in `SKILLS`
   (`crates/peet-cli/src/lib.rs`) and point to it from `core.md`.
3. **Reference.** Add it to `docs/scripting.md`.

Done when `cargo test -p peet-cli -p peet-ops -p peet-ui` passes and
`peet skills <name>` shows the new feature with a script that uses it. One of those
tests fails if an operation that adds a feature is named in no skill.

Writing a skill: state what to do, keep each fact in one place, and leave field lists to
`peet ops`, which is generated from the code and can't go stale.
