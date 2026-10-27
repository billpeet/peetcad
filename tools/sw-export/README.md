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
  - `sketch`: points, segments (lines, arcs, ellipses, splines, and text with its
    `format`: font, bold, italic, height, width factor, spacing), relations with the
    entities and the dimension each one ties (`defined` holds the model edge, vertex or
    plane a relation refers to outside the sketch), what the sketch is on, and the
    transform from model space to the sketch.
  - `definition`: every value the feature's data interface offers (`IExtrudeFeatureData2`,
    `IEdgeFlangeFeatureData`, …), read by reflection, so the names are the API's own and
    are documented in the SolidWorks API help. Methods that take a flag are read both
    ways: `GetDepth(true)`, `GetDepth(false)`. `unreadable` lists what threw.
    The interop assemblies can be older than the SolidWorks the exporter runs on (the
    ones in use are SolidWorks 2018's, revision 26): what they don't know is read by
    name, from the type information the running SolidWorks gives (`dispatchType`), and
    listed in `lateBound`. So a member SolidWorks added later (a linear pattern's
    `D1EndCondition`, from 2019) is written under its own name with no change here, and a
    feature with no data interface in the interop (a corner relief) gets a definition if
    this SolidWorks has one. A few methods are read for each index, from 0 until one
    fails: `GetTransform[i]` (a pattern's instances) and `Get…AtIndex[i]` (add a name to
    `IndexedReads` for more); `GetFaces[i]` for each of a fillet's face sets, 0 to 5
    (`FixedIndexes`). A method whose one argument is a second answer gives it as
    `Name.out` (a reference axis's `GetSelections`). An object with no case of its own is
    written with its `type` and its `values`, two levels deep (a structural member's
    `Groups`).
  - `planeTransform` for a reference plane and `mate` (type, alignment, the entities
    mated) for a mate.
- `equations`, with their values.
- `bodies` (parts): volume, area, centre of gravity, bounding box, and every face with its
  surface, the feature that made it and its name if it has one. Named edges too.
- `components` (assemblies): each instance's file, configuration and transform, and for a
  sub-assembly its `solving` ("rigid" or "flexible", SolidWorks' "Solve as"; added
  without a new format version, so an older export has none).

Added in format 6:

- A dimension's `arcEndConditions`: where on a circle or arc each of its two ends is
  measured (`IDimension.GetArcEndCondition`, swArcEndCondition_e: 0 none, 1 centre, 2 min,
  3 max). A mate's `firstArcCondition` and `secondArcCondition` likewise for a distance mate.
- A sketch's `slots` (centre, centre point, slot points, length, width), and each relation's
  `entityTypes` (`GetEntitiesType`). SolidWorks gives the slots of a symmetric relation
  between two slots as null entities: where two slots' centres are mirror images about the
  relation's line, they are written in their place as `sketchSlot`s, with `resolved` saying so.
- A path mate's `pathSelections`: what editing the mate selects (its point and the path's
  edges or sketch segments, each with its component and selection mark). SolidWorks 2018
  has no `IPathMateFeatureData` (it is from 2019) and gives the path as an empty mate entity,
  so the mate is edited (Edit Feature), the selections read, and the edit cancelled. Skipped
  with `--no-selections`.
- A composite curve's `curve`: its edges (`IReferenceCurve.GetSegments`). Its definition is
  no longer read in an assembly, where it faults SolidWorks 2018 (RPC_E_SERVERFAULT).
- Assemblies, per configuration: `suppressedChildren`, the components of each suppressed
  sub-assembly (which SolidWorks doesn't list, though mates can name them), read from the
  sub-assembly's own document in the configuration the component shows, with `transform`
  (in this assembly, by the component's transform) and `ownTransform`;
  `massProperties` (volume, area, mass, centre of mass, SolidWorks' `box`, and `tightBox`
  from the bodies' extreme points); and with `--step`, a STEP file of the configuration
  (`<file>.<configuration>.step`), as for parts.

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

`formatVersion` is 5. What each version added:

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
- **5:** what the running SolidWorks has and the interop assemblies don't is read by name
  (`lateBound`, above): a linear pattern's "up to reference" (`D1EndCondition` 1, with
  `D1EndReference`, `D1EndRefOffset`, `D1EndRefReverseOffset`, `D1EndUseSeedReference`,
  `D1EndSeedReference`, `D1EndUseSpacing`), each instance's `GetTransform[i]`, a base
  flange's end conditions and relief, an edge flange's angle reference. A sketch text
  has its `format` (`typeFace`, `bold`, `italic`, `charHeight` in metres, `widthFactor`,
  `charSpacingFactor`, `escapement`, …). An object with no case of its own has its
  `type` and `values` in place of `"kind": "unknown"`. None of this is checked against
  SolidWorks yet: a value that can't be read is left out (or listed in `unreadable`),
  and never stops the document.

In the interop assemblies (SolidWorks 2018) a corner relief has no definition: there is
no data interface for one, only the calls that make one (`AddCornerReliefType`,
`AddCornerReliefCorner`, `FinishCornerRelief`). SolidWorks 2026's API has
`ICornerReliefFeatureData` (`CornerCount`, `CornerType`, `GetCornerAtIndex`), which the
fifth version reads by name if the running SolidWorks gives it. Its shape is also in the
faces it leaves in the body (each face says which feature made it). A
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
