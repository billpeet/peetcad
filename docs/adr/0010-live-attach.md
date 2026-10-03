# ADR 0010: Live attach

Status: accepted (scripting stage 5)

## Context

`peet` works on part files. The goal of the [scripting plan](../scripting-plan.md) is
that the same commands also drive a part that is open in the running application, with
the model changing on screen. `PeetApp::apply_op` (stage 3) already applies an operation
to the application as if the user had done it. What was missing was a way for `peet` to
find a running application and send operations to it.

## Decision

**A local socket per application, and a folder that lists them** (`crates/peet-live`).

- A running application is a *session*. It listens on a local socket: a named pipe on
  Windows, a Unix domain socket elsewhere, through the `interprocess` crate, so there is
  no unsafe code of ours. Only the same user's processes can connect. Nothing listens on
  the network.
- Each session writes `sessions/<name>.json` in PeetCAD's data folder: process id,
  socket name, the file it has open, whether it is modified. It rewrites the entry when
  the open part changes and removes it on exit. `peet` reads the folder, and an entry
  whose socket no longer answers (a crash) is removed by whoever finds it.
- **The wire format is the operations.** Lines of JSON: a hello, answered with what the
  session has open now, then one operation per line, each answered with its reply.
  Nothing is translated: a script means the same thing headless and live.
- **Operations are applied on the UI thread**, in `eframe::App::logic`, which also runs
  while the window is minimised. The socket threads only carry lines: they queue each
  operation, wake the application (`request_repaint`) and wait for the answer. At most
  40 ms of operations are applied per frame, so the window keeps drawing through a long
  script.
- **A request is answered once.** If the application doesn't take an operation within 20
  seconds (it is blocked in a native file dialog), the connection answers "busy" and the
  operation is withdrawn, so it can't be applied later behind the client's back. An
  operation already being applied (a long rebuild) is waited for.
- **`peet` attaches by file.** If `--file` is open in a session, the operations go
  there. `--live` requires a session (and without `--file` means the only one), `--pid`
  names one, `--headless` never attaches. `peet sessions` lists them.
- **Live runs don't save, and aren't taken back.** The part is left modified, as if the
  user had made the changes; a script saves with the `save` operation. A failed
  operation stops the run and leaves the earlier ones applied (each is an undo step).
  `--new`, `--out` and `--materials` are about files and are refused on a live part.
- **Paths are made absolute by `peet`** before sending, so `path=flat.dxf` means the same
  place as it does headless and not the application's working directory.

## Alternatives

- **A loopback TCP port with a token in the session file.** No dependency, but any local
  user could reach the port, and the token would be the only protection.
- **Applying operations on the socket thread**, with the document behind a lock. The
  document, the selection and the open sketch are all the UI thread's; sharing them would
  touch most of `peet-ui`.
- **Rolling a failed live run back** with undo. It would also undo across anything the
  user did in between, so it is left to the script.

## Consequences

- Native only. The crate is empty on `wasm32`, and `peet-ui` only depends on it there.
- One operation per frame is the worst case for a client that waits for each reply: fine
  at 60 frames a second, about ten a second while the window is minimised (eframe
  throttles hidden windows to one pass per 100 ms).
- A whole run is many undo steps. Grouping a run into one (`Undo::Group` exists) is not
  exposed yet.
- The user and an agent can change the part at the same time. The only rule enforced is
  the one from the plan: while a sketch is open for editing, changes from outside are
  refused.
