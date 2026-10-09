---
name: live
description: Working on a part that is open in a running PeetCAD with peet - the user watches it change, each change is an undo step, and the view and windows can be driven too.
---

# Live: a part that is open in PeetCAD

When the part given with `-f` is open in a running PeetCAD, `peet` applies the
operations **there**, not to the file. The same operations, the same replies. The user
sees the part change, and each operation is one undo step in the application.

```sh
peet sessions                             # which PeetCADs are running, and what each has open
peet features -f bracket.peet             # goes to PeetCAD if bracket.peet is open there
peet run build.jsonl --live               # the one running PeetCAD, whatever it has open
peet view to=front --live
```

`peet` says on standard error when it does this: "Applying to PeetCAD, where …".

## Which part the operations go to

| Given | Goes to |
|---|---|
| `-f PART`, open in a PeetCAD | that PeetCAD |
| `-f PART`, open nowhere | the file |
| `--live` and no `-f` | the one running PeetCAD (a part that has never been saved has no file: this is how to reach it) |
| `--live -f PART` | that PeetCAD, and an error if the part is open nowhere |
| `--pid N` | the PeetCAD with that process id (from `peet sessions`) |
| `--headless` | the file, even if it is open: PeetCAD will not see the change, and saving there later overwrites it |

## What is different from working on a file

- **Nothing is saved for you.** The part is changed in the application and left unsaved,
  as if the user had done it. Save with `{"op": "save"}` if that is what was asked for;
  otherwise leave it to the user. `sessions` and `status` show `modified`.
- **A failed operation does not take the run back.** The run stops, and the operations
  before it stay applied. Fix the script from that operation on, or send `undo` once
  for each one applied.
- **Calls share the part.** `undo` reaches back over earlier calls, and over what the
  user did.
- **`--new`, `--out` and `--materials` are refused.** Start a new part with
  `{"op": "new"}`, save somewhere else with `{"op": "save", "path": …}`. The material
  tables and check limits are the user's settings, and `set_gauge` and `set_check_rule`
  change them for good.
- **The user is working too.** Look before changing (`status`, `features`): the part may
  not be as the last call left it. While the user has a sketch open for editing,
  operations that change the part are refused, saying so: ask them to finish the sketch,
  or send `exit_sketch` if that is yours to do. Queries always work.
- **Paths** in `save`, `export` and the imports are relative to where `peet` is run, as
  they are for a file.
- **The application's own operations** work only here: `view`, `zoom_to_fit`, `toggle`,
  `window`, `edit_sketch`, `exit_sketch`, `tool`, `quit` (`peet ops` marks them "needs a
  running PeetCAD"). Use `view` and `zoom_to_fit` to show the user what was done.
- If PeetCAD doesn't take an operation within 20 seconds (a file dialog is open in it),
  the reply is `"ok": false` saying so, and the operation is not applied.
