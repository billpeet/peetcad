# peet-sw-export

Writes what a SolidWorks part or assembly is made of as JSON (`<file name>.swx.json`), for
a PeetCAD importer to read and for finding out what PeetCAD can't yet rebuild. It needs
SolidWorks, so it is not part of the Cargo workspace or of CI.

It opens each file read-only in its own SolidWorks instance and never saves it. It refuses
to run if the instance it gets is under user control or has documents open.

```
peet-sw-export.exe <input-list.txt> <output-directory> [--step] [--dxf] [--no-selections]
```

- `input-list.txt`: one path per line (`.sldprt` or `.sldasm`).
- `--step`: also save each configuration of a part as STEP, to compare a rebuilt part with.
- `--dxf`: also save the flat pattern of each configuration of a sheet metal part.
- `--no-selections`: don't read the edges, faces and sketches that feature definitions
  refer to. Reading them rolls the model back to each feature in turn, which is the slow
  part and the one most likely to upset SolidWorks. Use it if an export hangs or fails.
- Output: the JSON files, `export.log` and `summary.json`. A document that could not be
  fully exported still gets a file, with `completed` false and the reason.
- Exit code: 0 when every document was fully exported, 1 when one was not, 2 for wrong
  arguments.

## What is in a file

Lengths are metres and angles radians, as the SolidWorks API gives them.

For each configuration:

- `features`: the tree in order, each with its type, suppression, parents and children,
  and its `dimensions` with their values in this configuration.
  - `sketch`: points, segments (lines, arcs, ellipses, splines, text), relations with the
    entities and the dimension each one ties (`defined` holds the model edge, vertex or
    plane a relation refers to outside the sketch), what the sketch is on, and the
    transform from model space to the sketch.
  - `definition`: every value the feature's data interface offers (`IExtrudeFeatureData2`,
    `IEdgeFlangeFeatureData`, …), read by reflection, so the names are the API's own and
    are documented in the SolidWorks API help. Methods that take a flag are read both
    ways: `GetDepth(true)`, `GetDepth(false)`. `unreadable` lists what threw.
  - `planeTransform` for a reference plane and `mate` (type, alignment, the entities
    mated) for a mate.
- `equations`, with their values.
- `bodies` (parts): volume, area, centre of gravity, bounding box, and every face with its
  surface, the feature that made it and its name if it has one. Named edges too.
- `components` (assemblies): each instance's file, configuration and transform.

A configuration SolidWorks made for the flat pattern (`…SM-FLAT-PATTERN`) has its bodies
only: their box is the flat size.

Faces, edges and sketch entities that something refers to are written as objects with a
`kind`. A sketch entity is identified by its `id` (two numbers) and its sketch's name.

## Building

```powershell
dotnet build tools/sw-export/SwExport.csproj -c Release
```

The SolidWorks interop assemblies (`SolidWorks.Interop.sldworks.dll` and `swconst.dll`,
from `api\redist` in a SolidWorks installation) are looked for in the folder given by the
`SolidWorksInteropDir` environment variable or `-p:SolidWorksInteropDir=...`.

## Running it on a SolidWorks machine

SolidWorks cannot be started from an SSH session. `RunInDesktopSession.ps1` (copied
beside the executable) starts the exporter as a one-off scheduled task in the account's
logged-in desktop, waits for it and removes the task. The account needs an open desktop
session there, and the paths in the input list must be readable from it.

```powershell
scp tools/sw-export/bin/Release/net48/* USER@HOST:C:/Users/USER/AppData/Local/PeetSwExport/bin/
ssh USER@HOST powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `
  C:\Users\USER\AppData\Local\PeetSwExport\bin\RunInDesktopSession.ps1 `
  -InputList C:\Users\USER\AppData\Local\PeetSwExport\inputs.txt `
  -OutputDirectory C:\Users\USER\AppData\Local\PeetSwExport\output -Step -Dxf
scp USER@HOST:C:/Users/USER/AppData/Local/PeetSwExport/output/* <local folder>
```

## Status

Run against SolidWorks 2026 on the whole template library (201 parts, 29 assemblies):
all complete, in about 80 minutes, twice. The flat sizes in the DXF files agree with the
flat-pattern configurations' bodies and with the bend deduction by hand.

`formatVersion` is 2. Since 1, a definition also has its `Is…` methods (`IsBossFeature`
is the only thing that tells a boss-extrude from a cut-extrude) and its values per
reference (`Constraint[0]`); a relation's entities say where they are, because the id
a relation gives for a segment is not the id the sketch lists it under; and every face
has a `point` that is on it.

`tally.js` counts what a folder of exports uses: see
[docs/solidworks-library-tally.md](../../docs/solidworks-library-tally.md).

Still missing: whether a distance dimension is horizontal, vertical or aligned (the
importer works it out from the points), and the sketch a sketched bend is made from.

Known behaviour:

- **Flat patterns.** On a document opened read-only, SolidWorks writes the whole DXF,
  then faults and closes the document. The exporter takes the file if it is there
  (`flatDxfFault` records the fault) and opens the document again for the next
  configuration. Every entity is on layer 0, bend lines included.
- **A flat-pattern configuration is not always flat.** `unfolded` says whether it is;
  the Door's is not, so its flat size comes only from the DXF.
- **A crash.** If a document takes SolidWorks down, what was read is written and the
  rest of the list gets a new instance (five times at most).
