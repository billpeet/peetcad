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

`formatVersion` is 4. What each version added:

- **2:** a definition has its `Is…` methods (`IsBossFeature` is the only thing that tells a
  boss-extrude from a cut-extrude; a part's first extrude is `IsBaseExtrude`) and its
  values per reference (a plane's `Constraint[0]`); a relation's entities say where they
  are, because the id a relation gives for a segment is not the id the sketch lists it
  under; and every face has a `point` that is on it.
- **3:** a sketch has its `blocks`: each instance with its definition's segments, in the
  definition's own coordinates, and `blockToSketch`, the transform that places them (the
  sketch's own list of segments leaves out what is inside a block). A dimension has
  `lineDirection`, the way it measures, in the model's coordinates: two points can be
  dimensioned along x, along y or straight across. And the selections of suppressed
  features are read where SolidWorks allows it, so that a suppressed flange has its
  edges and a suppressed pattern its direction.
- **4:** a library feature has `subFeatures`, the names of what is in it. The walk of the
  tree meets those first, as features of their own just above it (with their sketches
  and definitions), so they are written there and only named under it. A body has
  `tightBox`, from its extreme points: `box` (GetBodyBox) can be loose by a hundredth of
  a millimetre round a freeform face. A corner relief has `namedDimensions` if any of
  `D1` to `D6` answers by name.

A corner relief has no definition: the API has no data interface for one, only the calls
that make one (`AddCornerReliefType`, `AddCornerReliefCorner`, `FinishCornerRelief`). Its
shape is in the faces it leaves in the body (each face says which feature made it). A
3D sketch's points and segments are in the model's coordinates (its `modelToSketch` is
the identity).

`tally.js` counts what a folder of exports uses: see
[docs/solidworks-library-tally.md](../../docs/solidworks-library-tally.md).

Still missing: the definitions of composite curves (33, in one assembly); what a mate
joins when its component is not loaded (the end is written with its numbers and no
entity); and the selections of some suppressed features, which SolidWorks will not
give.

Known behaviour:

- **No desktop session, no SolidWorks.** With the account logged off, the scheduled task
  never runs (result 267011, "has not run"); one that runs whether the account is logged
  on or not (S4U) gets no SolidWorks (`CO_E_SERVER_EXEC_FAILURE` after two minutes), and
  has no network credentials for the library's share anyway.

- **Flat patterns.** On a document opened read-only, SolidWorks writes the whole DXF,
  then faults and closes the document. The exporter takes the file if it is there
  (`flatDxfFault` records the fault) and opens the document again for the next
  configuration. Every entity is on layer 0, bend lines included.
- **A flat-pattern configuration is not always flat.** `unfolded` says whether it is;
  the Door's is not, so its flat size comes only from the DXF.
- **A crash.** If a document takes SolidWorks down, what was read is written and the
  rest of the list gets a new instance (five times at most).
