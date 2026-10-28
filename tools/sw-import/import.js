#!/usr/bin/env node
// Rebuilds a SolidWorks part in PeetCAD from its export (tools/sw-export), by applying peet operations one at
// a time and reading each reply. Writes the part, the operations that were applied (a script peet can run
// again) and a report that says what was imported, what was left out and whether the result matches
// SolidWorks' own numbers.
//
//   node import.js <export.swx.json | folder of exports> <output folder> [--peet <peet executable>] [--only <text>]
//
// Coordinates are kept as SolidWorks has them, in millimetres. SolidWorks' Front Plane is the XY plane,
// which is PeetCAD's "top", so a sketch goes on the PeetCAD plane that lies where its SolidWorks plane does.
"use strict";
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

// ---- Arguments ----

const args = process.argv.slice(2);
const option = name => {
  const i = args.indexOf(name);
  return i < 0 ? null : args.splice(i, 2)[1];
};
const repo = path.resolve(__dirname, "..", "..");
const peetExe = option("--peet")
  || [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe"), "peet"].find(p => p === "peet" || fs.existsSync(p));
const only = option("--only");
/** How many parts to import at once, each in a process of its own. */
const jobs = Math.max(1, parseInt(option("--jobs") || "1", 10));
const flag = name => { const i = args.indexOf(name); if (i >= 0) args.splice(i, 1); return i >= 0; };
/** Without them a part is the right shape and can't be driven. Leaving them out is much quicker. */
const relations = !flag("--no-relations");
/**
 * A folder of SolidWorks' STEP files (`<Name>.SLDPRT.<configuration>.step`, as the exporter writes them). Where
 * one is there, the volume to match is its body's as PeetCAD measures it: SolidWorks' own mass properties are
 * a few parts in 100,000 out for a body with freeform faces (the ends of slanted bends), its STEP body is not.
 * A part nothing can be built of is imported from its STEP file instead.
 */
const stepDir = option("--step");
if (args.length !== 2) {
  console.error("Usage: node import.js <export.swx.json | folder of exports> <output folder> [--peet <peet executable>] [--only <text>] [--step <folder of STEP files>]");
  process.exit(2);
}
const [input, outDir] = args.map(a => path.resolve(a));

// ---- Vectors ----

const MM = 1000;
const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const mul = (a, s) => [a[0] * s, a[1] * s, a[2] * s];
const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const len = a => Math.sqrt(dot(a, a));
const unit = a => mul(a, 1 / len(a));
const mid = (a, b) => mul(add(a, b), 0.5);
const mm = p => p.map(v => v * MM);
const tidy = v => Math.abs(v) < 5e-7 ? 0 : +v.toFixed(6);
const AXES = { x: [1, 0, 0], y: [0, 1, 0], z: [0, 0, 1] };

/** A SolidWorks transform from model space to a sketch or plane: its axes and origin in the model, in mm. */
function frameOf(a) {
  const x = [a[0], a[3], a[6]], y = [a[1], a[4], a[7]], n = [a[2], a[5], a[8]];
  const o = mul(add(add(mul(x, a[9]), mul(y, a[10])), mul(n, a[11])), -MM);
  return { x, y, n, o };
}
/**
 * A reference plane's transform, which runs the other way: from the plane to the model. Its rows are the
 * plane's axes and its normal, then its origin, in the model.
 */
function planeFrameOf(a) {
  return { x: [a[0], a[1], a[2]], y: [a[3], a[4], a[5]], n: [a[6], a[7], a[8]], o: mm([a[9], a[10], a[11]]) };
}
const fromSketch = (f, p) => add(add(add(f.o, mul(f.x, p.x * MM)), mul(f.y, p.y * MM)), mul(f.n, (p.z || 0) * MM));
const samePlane = (a, b) => Math.abs(Math.abs(dot(a.n, b.n)) - 1) < 1e-7 && Math.abs(dot(sub(a.o, b.o), b.n)) < 1e-4;
/** A sketch point's place as a key: two points a tenth of a micron apart are one. */
const spot = p => `${Math.round(p.x * 1e7)},${Math.round(p.y * 1e7)}`;
/** A sketch text's point: the sketch point with the text's id (SolidWorks lists it with the points). */
const textPoint = (s, g) => s.points.find(p => p.id && p.id.join() === g.id.join()) || null;
/** The fonts a text is fitted with, where the export does not say: SolidWorks' default and its styles. */
const TEXT_FONTS = ["Century Gothic", "Century Gothic Bold", "Century Gothic Italic", "Century Gothic Bold Italic"];
/** A sketch text's format as exports from the fifth on have it, or null. */
const textFormat = g => g.format && !g.format.error && g.format.typeFace ? g.format : null;
/** The PeetCAD font of a SolidWorks text format: its face, with " Bold" and " Italic" as SolidWorks has them. */
const fontOf = t => t.typeFace + (t.bold ? " Bold" : "") + (t.italic ? " Italic" : "");
/**
 * A sketch with the segments of its blocks among its own. A block is a group of segments drawn once and
 * placed by a transform; PeetCAD sketches have no blocks, so each instance's segments are drawn where it puts
 * them and the grouping is lost. (The third export has what is in a block; before it they are missing.)
 */
function withBlocks(sketch) {
  if (!sketch || !sketch.blocks || !sketch.blocks.some(b => (b.segments || []).length)) return sketch;
  const segments = [...sketch.segments];
  sketch.blocks.forEach((block, k) => (block.segments || []).forEach((g, i) => segments.push({ ...inBlock(block, g), id: ["block" + k, i] })));
  // Its points too (its origin, say), which relations and dimensions go to
  const points = [...sketch.points];
  sketch.blocks.forEach((block, k) => (block.points || []).forEach((p, i) => points.push({ ...inBlock(block, p), id: ["block" + k, i] })));
  return { ...sketch, segments, points, blockInstances: 0, blocksDrawn: sketch.blocks.length };
}
/** A point or segment of a block's definition where the block instance puts it in its sketch. */
function inBlock(block, g) {
  const a = block.blockToSketch;
  // The same layout as every SolidWorks transform: a rotation by rows, then the translation and the scale
  const place = p => !p ? p : !a ? p : {
    ...p,
    x: (p.x * a[0] + p.y * a[3] + (p.z || 0) * a[6]) * (a[12] || 1) + a[9],
    y: (p.x * a[1] + p.y * a[4] + (p.z || 0) * a[7]) * (a[12] || 1) + a[10],
  };
  if (g.x !== undefined) return place(g);
  return { ...g, start: place(g.start), end: place(g.end), center: place(g.center), radius: g.radius === undefined ? undefined : g.radius * ((a && a[12]) || 1) };
}
/** How far, in mm, an edge or a face may be from where SolidWorks has it and still be taken for the same one. */
const NEAR = 2;

/** Points along a sketch segment, in its sketch's coordinates: an arc's ends and points round it. */
function pointsAlong(g) {
  if (g.type !== "arc" || !g.center || g.circle) return [g.start, g.end].filter(Boolean);
  const c = g.center, r = g.radius;
  const a0 = Math.atan2(g.start.y - c.y, g.start.x - c.x), a1 = Math.atan2(g.end.y - c.y, g.end.x - c.x);
  // Counter-clockwise from start to end when the arc runs that way, else clockwise
  let sweep = a1 - a0;
  if (g.direction === 1) { while (sweep <= 0) sweep += 2 * Math.PI; } else { while (sweep >= 0) sweep -= 2 * Math.PI; }
  return Array.from({ length: 33 }, (_, i) => ({ x: c.x + r * Math.cos(a0 + sweep * i / 32), y: c.y + r * Math.sin(a0 + sweep * i / 32) }));
}

/** Whether a sketch's lines and arcs leave an end that meets no other: an open profile, not an outline. */
function isOpen(segments) {
  const ends = new Map();
  for (const g of segments) {
    if (g.type === "arc" && g.circle) continue;
    if (!g.start || !g.end) continue;
    for (const p of [g.start, g.end]) ends.set(spot(p), (ends.get(spot(p)) || 0) + 1);
  }
  return ends.size > 0 && [...ends.values()].some(n => n === 1);
}

/**
 * The lines of an open profile with each run of lines that are in line, end to end, made one line. Only for
 * profiles of lines alone: anything else is left as it is.
 */
function joinInLine(segments) {
  if (segments.some(g => g.type !== "line" || !g.start || !g.end)) return segments;
  const lines = segments.map(g => ({ start: g.start, end: g.end }));
  const dir = g => { const dx = g.end.x - g.start.x, dy = g.end.y - g.start.y, l = Math.hypot(dx, dy); return [dx / l, dy / l]; };
  for (let joined = true; joined;) {
    joined = false;
    for (let i = 0; i < lines.length && !joined; i++) {
      for (let j = 0; j < lines.length && !joined; j++) {
        if (i === j) continue;
        // a then b, end to end, each turned round as needed
        const turn = g => ({ start: g.end, end: g.start });
        const meet = spot(lines[i].end) === spot(lines[j].start) || spot(lines[i].end) === spot(lines[j].end);
        const a = meet ? lines[i] : turn(lines[i]);
        const b = spot(lines[j].start) === spot(a.end) ? lines[j] : turn(lines[j]);
        if (spot(b.start) !== spot(a.end)) continue;
        // Only where no third segment meets them there
        if (lines.some((g, k) => k !== i && k !== j && (spot(g.start) === spot(a.end) || spot(g.end) === spot(a.end)))) continue;
        // In line by PeetCAD's measure: a bend of under a microradian
        const [ax, ay] = dir(a), [bx, by] = dir(b);
        if (Math.atan2(Math.abs(ax * by - ay * bx), ax * bx + ay * by) >= 1e-6) continue;
        const bb = b;
        lines[i] = { start: a.start, end: bb.end };
        lines.splice(j, 1);
        joined = true;
      }
    }
  }
  return lines.length < segments.length ? lines.map(g => ({ type: "line", ...g })) : segments;
}

/**
 * The angles a profile of lines turns through at its corners, in order from one end: what each bend of a
 * base flange from it turns through.
 */
function profileAngles(lines) {
  const ends = new Map();
  for (const g of lines) for (const p of [g.start, g.end]) ends.set(spot(p), (ends.get(spot(p)) || 0) + 1);
  const first = lines.find(g => ends.get(spot(g.start)) === 1 || ends.get(spot(g.end)) === 1);
  if (!first) return [];
  const chain = [], left = [...lines];
  let at = ends.get(spot(first.start)) === 1 ? first.start : first.end;
  while (left.length) {
    const k = left.findIndex(g => spot(g.start) === spot(at) || spot(g.end) === spot(at));
    if (k < 0) break;
    const g = left.splice(k, 1)[0];
    const [a, b] = spot(g.start) === spot(at) ? [g.start, g.end] : [g.end, g.start];
    chain.push([b.x - a.x, b.y - a.y]);
    at = b;
  }
  const angles = [];
  for (let i = 0; i + 1 < chain.length; i++) {
    const [u, v] = [chain[i], chain[i + 1]];
    angles.push(Math.atan2(Math.abs(u[0] * v[1] - u[1] * v[0]), u[0] * v[0] + u[1] * v[1]));
  }
  return angles;
}

/** The standard plane that lies where this frame does, if one does. */
function standardPlane(f) {
  if (Math.abs(Math.abs(f.n[2]) - 1) < 1e-9 && Math.abs(f.o[2]) < 1e-6) return "top";
  if (Math.abs(Math.abs(f.n[1]) - 1) < 1e-9 && Math.abs(f.o[1]) < 1e-6) return "front";
  if (Math.abs(Math.abs(f.n[0]) - 1) < 1e-9 && Math.abs(f.o[0]) < 1e-6) return "right";
  return null;
}

// ---- peet ----

/** One part file, changed by one peet call per operation so that each reply can decide the next operation. */
class Peet {
  constructor(file) {
    this.file = file;
    this.script = [];
    if (fs.existsSync(file)) fs.unlinkSync(file);
    const made = spawnSync(peetExe, ["new", file], { encoding: "utf8" });
    if (made.status !== 0) throw new Error("peet new failed: " + made.stderr);
  }
  run(op, save) {
    const r = spawnSync(peetExe, ["run", "-", "-f", this.file, "-q", ...(save ? [] : ["--no-save"])],
      { input: JSON.stringify(op) + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
    const line = (r.stdout || "").split("\n").find(l => l.startsWith("{"));
    if (!line) return { ok: false, error: (r.stderr || "no reply").trim() };
    return JSON.parse(line);
  }
  /** Applies an operation and keeps it. A reply that is not ok changed nothing. */
  apply(op) {
    const reply = this.run(op, true);
    if (reply.ok) this.script.push(op);
    return reply;
  }
  /** Asks without changing the part: a query, or an operation to see what it would do. */
  ask(op) { return this.run(op, false); }
  note(text) { this.script.push("# " + text); }
}

// ---- Import ----

const STRUCTURE = /Folder$|Light$|Cabinet$|^AnnotationViewFeat$|^OriginProfileFeature$|^SelectionSetNode$|^Attribute$|^GridFeature$|^FlatPattern$|^OneBend$|^UiBend$|^SmartComponentFeature$|^Reference$|^SketchBlockDef$|^SketchBlockInst$|^FlatPatternFolder$|^TemplateSheetMetal$|^TemplateFlatPattern$/;
const SKETCH = /^(ProfileFeature|3DProfileFeature)$/;
/** For each kind of SolidWorks feature, which of its definition's values is which field of PeetCAD's feature. */
const DEPTH = { "GetDepth(true)": "depth" };
const DIMENSION_FIELDS = {
  EdgeFlange: { OffsetDistance: "length", BendAngle: "angle", PositionOffsetDistance: "position_offset" },
  LPattern: { D1Spacing: "spacing", D1TotalInstances: "count", D2Spacing: "spacing2", D2TotalInstances: "count2" },
  SMBaseFlange: { D1OffsetDistance: "depth" },
  Fillet: { DefaultRadius: "size" },
  BreakCorner: { Distance: "size" },
  ICE: DEPTH, Cut: DEPTH, Extrusion: DEPTH, Boss: DEPTH,
  Revolution: { "GetRevolutionAngle(true)": "angle" },
};

function importPart(doc, dir) {
  const config = doc.configurations.find(c => c.name === doc.activeConfiguration) || doc.configurations[0];
  const file = path.join(dir, doc.name + ".peet");
  const peet = new Peet(file);
  peet.note(`${path.basename(doc.sourcePath)}, configuration ${config.name}, imported from its SolidWorks export.`);

  const report = { name: doc.name, configuration: config.name, features: [], notes: [] };
  // The sketches SolidWorks has suppressed in the active configuration, or that are under a feature it has
  // suppressed there: SolidWorks does not solve them, and the export has them as they were last solved
  // (the export reads every configuration's sketches in the active one)
  const unsolvedSketches = new Set();
  (function walk(list, off) { for (const x of list || []) { const now = off || !!x.suppressed; if (now && SKETCH.test(x.type)) unsolvedSketches.add(x.name); walk(x.sub, now); } })(config.features, false);
  // A feature that fails to rebuild in a configuration (an error, not a warning) leaves nothing in
  // SolidWorks' body there, as if it were suppressed (Top Hat's Edge-Flange3, whose outline crosses itself
  // where its edge is short): it is treated as suppressed in that configuration, and built where it is not
  for (const c of doc.configurations) {
    const faced = new Set((c.bodies || []).flatMap(b => b.faces.map(face => face.feature)));
    for (const x of c.features || []) if (x.errorCode != null && !x.isWarning && !x.suppressed && !faced.has(x.name) && !STRUCTURE.test(x.type) && !SKETCH.test(x.type) && x.type !== "RefPlane") {
      x.suppressed = true;
      if (c === config) report.notes.push(`${x.name} fails to rebuild in SolidWorks (error ${x.errorCode}) and is not in its body: it is taken as suppressed here.`);
    }
  }
  // The features suppressed in some configuration, and so PeetCAD's features that stand for them
  const sometimesOff = new Set();
  for (const c of doc.configurations) (function walk(list) { for (const x of list || []) { if (x.suppressed) sometimesOff.add(x.name); walk(x.sub); } })(c.features);
  const switched = { has: name => !!name && [...sometimesOff].some(n => name === n || name.startsWith(n + " ")) };
  // Each configuration's suppressed features, with what is built on them (which SolidWorks suppresses with
  // them), and so those suppressed somewhere a feature is live
  const offIn = doc.configurations.filter(c => c.features).map(c => {
    const off = new Set();
    (function walk(list) { for (const x of list || []) { walk(x.sub); if (x.suppressed || (x.parents || []).some(p => off.has(p))) off.add(x.name); } })(c.features);
    return off;
  });
  const switchedWhereLive = feature => {
    const names = new Set(offIn.filter(off => !off.has(feature)).flatMap(off => [...off]));
    return { has: name => !!name && [...names].some(n => name === n || name.startsWith(n + " ")) };
  };
  /** A configuration's features by name, sub-features included. */
  const flatten = c => { const m = new Map(); (function walk(list) { for (const x of list || []) { if (!m.has(x.name)) m.set(x.name, x); walk(x.sub); } })(c.features); return m; };
  // The configuration being imported from: the active one first, then each other one for the features
  // that are only live there
  let cur = config;
  let byName = flatten(config);

  const planes = new Map();     // SolidWorks plane name -> PeetCAD plane selector
  const sketches = new Map();   // SolidWorks sketch name -> { name, toPeet, frame, partial }
  const made = new Map();       // SolidWorks feature name -> names of the PeetCAD features that stand for it
  const planeFrames = new Map(); // SolidWorks plane name -> where it is
  const upToPatterns = new Set(); // SolidWorks linear patterns made to run up to their reference
  const flanges = new Map();     // SolidWorks edge flange name -> what it was made with, for mirroring it
  const rectangular = new Set(); // SolidWorks edge flanges whose outline could not be their own
  const miters = new Map();      // SolidWorks mitre flange name -> what it was made with, for mirroring it
  const promoted = new Map();    // "D1@Sketch1" -> the parameter that sketch dimension now follows
  const dimensionsNamed = new Set(); // SolidWorks features whose sizes have been given SolidWorks' names
  const flangeEnds = new Map();  // "flange|outline sketch" -> the edge's ends and which is the PeetCAD flange's start
  let equationsLater = null;     // tries again the equations that set sizes of features imported later
  const formulas = new Set();    // the sketch dimensions (full names) whose value is a formula
  const asleep = new Set();      // SolidWorks features imported and then suppressed in every configuration
  const cornerOps = new Map();   // PeetCAD corner feature name -> the operation that made it, for corner reliefs
  const rejoined = new Map();    // SolidWorks CombineBodies name -> the Split whose pieces it joins again
  const moves = new Map();       // SolidWorks Move/Copy Body name -> the PeetCAD features whose faces its body has
  let unfolded = null;           // between an Unfold and its Fold: the face that stays where it is
  let sheet = null;             // the sheet metal settings in force
  let thickness = null;
  let asleepNow = false;         // while a feature suppressed in every configuration is built

  const record = (f, result, message) => report.features.push({ name: f.name, type: f.type, result, ...(message ? { message } : {}), ...(cur !== config ? { configuration: cur.name } : {}) });
  const sheetBody = () => (peet.ask({ op: "bodies" }).bodies || []).some(b => b.sheet_metal);

  /** Applies an operation that adds features. A feature that can't be built is taken out again. */
  function build(f, op) {
    const reply = peet.apply(op);
    if (!reply.ok) return { ok: false, message: reply.error };
    const created = reply.created || [];
    const failed = created.filter(c => c.status === "failed");
    // A feature suppressed everywhere in SolidWorks whose cut lies off the part at these sizes (Escutcheon's
    // third hinge's holes, below the part until it is 1500 tall) is kept, suppressed, as SolidWorks keeps it
    if (failed.length && asleepNow && failed.every(c => /doesn't remove any material: its shape is off/.test(c.message || ""))
      && created.every(c => peet.apply({ op: "suppress", feature: c.name, on: true, configurations: "all" }).ok)) {
      return { ok: true, names: created.map(c => c.name), warning: `it can't be built at these sizes (${failed[0].message}), as in SolidWorks, where it is suppressed` };
    }
    if (failed.length) {
      peet.apply({ op: "delete", features: created.map(c => c.name) });
      return { ok: false, message: failed[0].message || "it can't be built" };
    }
    const warning = created.find(c => c.status === "warning");
    return { ok: true, names: created.map(c => c.name), warning: warning && warning.message };
  }
  function finish(f, built) {
    if (!built.ok) return record(f, "failed", built.message);
    made.set(f.name, built.names);
    record(f, built.warning ? "imported with a warning" : "imported", built.warning);
  }


  /**
   * A SolidWorks 3D sketch as PeetCAD's (`sketch3d`): its points and lines where SolidWorks has them, its
   * relations (together, on a line, at the middle, along an axis, parallel, square, as long, its origin
   * fixed) and its distances as dimensions under SolidWorks' names (D1@3DSketch1). `path` is the lines a
   * structural member follows (their ids): the others are construction lines. `follow`, if given, is the 3D
   * sketch whose dimensions this one's follow (a copy for another group of a structural member). Returns
   * the PeetCAD name, or an error.
   */
  function import3dSketch(f, name, path, follow) {
    const s = f.sketch;
    const key = id => id.join(",");
    const index = new Map(), points = [];
    const add = q => { const k = key(q.id); if (!index.has(k)) { index.set(k, points.length); points.push(mm([q.x, q.y, q.z || 0]).map(tidy)); } return index.get(k); };
    for (const q of s.points || []) add(q);
    const lineIndex = new Map(), lines = [];
    for (const g of s.segments || []) {
      if (g.type !== "line" || !g.start || !g.end) continue;
      lineIndex.set(key(g.id), lines.length);
      const real = !g.construction && (!path || path.has(key(g.id)));
      lines.push(real ? [add(g.start), add(g.end)] : { from: add(g.start), to: add(g.end), construction: true });
    }
    const dims = (f.dimensions || []).filter(d => d.valueType === 0);
    const relations = [], dropped = {};
    const drop = why => { dropped[why] = (dropped[why] || 0) + 1; };
    const pointOf = e => e && e.kind === "sketchPoint" ? index.get(key(e.id)) : undefined;
    const lineOf = e => e && e.kind === "sketchSegment" && e.type === "line" ? lineIndex.get(key(e.id)) : undefined;
    const AXIS = { alongx: "along_x", alongx3d: "along_x", alongy: "along_y", alongy3d: "along_y", alongz: "along_z", alongz3d: "along_z" };
    // SolidWorks' 3D sketch origin is fixed where it is
    for (const q of s.points || []) if (key(q.id) === "0,1" && !q.x && !q.y && !q.z) relations.push({ type: "fix", of: [index.get("0,1")] });
    for (const r of s.relations || []) {
      const es = r.entities || [];
      const ps = es.map(pointOf), ls = es.map(lineOf);
      const pair = (a, b) => [ps.findIndex((x, i) => x !== undefined && ls[i] === undefined), ls.findIndex(x => x !== undefined)];
      if (r.type === "coincident" && es.length === 2 && ps.every(x => x !== undefined)) relations.push({ type: "coincident", of: ps });
      else if ((r.type === "coincident" || r.type === "atmiddle") && es.length === 2) {
        const [pi, li] = pair();
        if (pi < 0 || li < 0) { drop(`${r.type}: no match for what it ties`); continue; }
        relations.push({ type: r.type === "atmiddle" ? "midpoint" : "on_line", of: [ps[pi], ls[li]] });
      } else if (/^(parallel|perpendicular|samelength)$/.test(r.type) && es.length === 2 && ls.every(x => x !== undefined)) {
        relations.push({ type: r.type === "samelength" ? "equal" : r.type, of: ls });
      } else if (AXIS[r.type] && es.length === 1 && ls[0] !== undefined) relations.push({ type: AXIS[r.type], of: [ls[0]] });
      else if (r.type === "distance" && r.dimension && es.length === 2) {
        const k = dims.findIndex(d => d.fullName === r.dimension);
        const ends = es.map((e, i) => ps[i] !== undefined ? { point: ps[i] } : ls[i] !== undefined ? { line: ls[i] } : null);
        if (k < 0 || ends.some(x => !x)) { drop("distance: no match for what it ties"); continue; }
        relations.push({ type: "distance", of: ends, dimension: k });
      } else drop(`${r.type}: not a relation a 3D sketch takes`);
    }
    const dimensions = dims.map(d => follow ? `"${d.name}@${follow}"` : tidy(d.value * MM));
    const reply = peet.apply({ op: "sketch3d", name, points, lines, relations, dimensions });
    if (!reply.ok) return { error: reply.error };
    const created = (reply.created || [])[0];
    if (created && created.status === "failed") { peet.apply({ op: "delete", feature: name }); return { error: created.message }; }
    dims.forEach((d, k) => peet.apply({ op: "name_dimension", feature: name, dimension: `d${k + 1}`, name: d.name }));
    if (!follow) {
      const total = (s.relations || []).length, kept = total - Object.values(dropped).reduce((a, b) => a + b, 0);
      report.sketches = report.sketches || {};
      report.sketches[f.name] = { relations: total, kept, dropped };
      for (const x of report.features) if (x.name === f.name) { x.result = "imported"; delete x.message; }
    }
    return { name };
  }

  /** The plane PeetCAD gives a sketch on this selector, without adding one. */
  function probe(on) {
    const reply = peet.ask({ op: "sketch", on, draw: [] });
    if (!reply.ok || !reply.plane) return null;
    return { x: reply.plane.x, y: reply.plane.y, n: reply.plane.normal, o: reply.plane.origin };
  }

  /** Where a SolidWorks sketch can go in PeetCAD: the selectors to try, most faithful first. */
  function placements(s, sf, needFace) {
    const list = [];
    const points = [];
    for (const g of s.segments) for (const k of ["center", "start", "end"]) if (g[k]) points.push(fromSketch(sf, g[k]));
    for (const p of s.points) points.push(fromSketch(sf, p));
    const faces = normal => {
      const at = [];
      if (s.on && s.on.box) at.push(mid(mm(s.on.box.slice(0, 3)), mm(s.on.box.slice(3))));
      at.push(...points.slice(0, 12));
      for (const p of at) list.push({ at: p.map(tidy), normal: normal.map(tidy) });
    };
    if (unfolded && unfolded.point && unfolded.normal) list.push({ at: mm(unfolded.point).map(tidy), normal: unfolded.normal.map(tidy) });
    const onFace = s.on && s.on.kind === "face" && s.on.surface === "plane";
    if (onFace) faces(s.on.normal);
    if (!onFace || !needFace) {
      const standard = standardPlane(sf);
      if (standard) list.push(standard);
      if (s.on && s.on.kind === "feature" && planes.has(s.on.name)) list.push(planes.get(s.on.name));
    }
    if (!onFace) { faces(sf.n); faces(mul(sf.n, -1)); }
    return list;
  }

  /**
   * Adds a SolidWorks sketch's geometry to the part. `draw`, if given, makes the items in place of the
   * sketch's own lines and arcs (a hole's circles at the sketch's points).
   */
  /**
   * `onFace`, if given, is the one place to put the sketch: a face parallel to it anywhere along its normal,
   * the sketch projected onto it.
   */
  function importSketch(f, { name = f.name, needFace = false, draw = null, onFace = null, holes = null } = {}) {
    const s = withBlocks(f.sketch);
    if (!s || s.error) return { error: "the sketch was not exported" };
    if (s.is3D) return { error: "a 3D sketch" };
    const sf = frameOf(s.modelToSketch);
    // The face may not be exactly where SolidWorks has it (a flange at an angle comes out a tenth of a
    // millimetre away). The nearest one within reach is taken, the sketch is projected onto it, and the
    // report says how far off it was.
    let on = null, pf = null, away = Infinity;
    for (const candidate of onFace ? [onFace] : placements(s, sf, needFace)) {
      const plane = probe(candidate);
      if (!plane || Math.abs(dot(plane.n, sf.n)) < 0.9999) continue;
      const d = Math.abs(dot(sub(plane.o, sf.o), sf.n));
      if (d < away) { on = candidate; pf = plane; away = d; }
      if (d < 1e-4) break;
    }
    // Nothing there (a sketch on a plane SolidWorks made for it, or on a face PeetCAD has elsewhere): a plane
    // where SolidWorks' sketch is, as SolidWorks' cuts and extrusions go from any plane
    // A sketch suppressed here is in the export as SolidWorks last solved it, which can be for another part
    // the file was made from (Esc Surround R Top's slots, still on Esc Surround Horizontal's flanges). In
    // SolidWorks it is on a face of what it is built on, wherever that is now, and its normal is the face's:
    // the nearest such face of a feature it is built on along its normal is where it goes.
    if ((!on || away > NEAR) && !onFace && f.suppressed) {
      const mine = new Set((f.parents || []).flatMap(p => made.get(p) || []));
      let best = null;
      for (const face of mine.size ? peet.ask({ op: "faces" }).faces || [] : []) {
        if (face.surface !== "plane" || !face.normal || dot(face.normal, sf.n) < 0.9999) continue;
        if (!(face.made_by || []).some(m => mine.has(m.feature))) continue;
        const d = Math.abs(dot(sub(face.center, sf.o), sf.n));
        if (!best || d < best.d) best = { d, sel: { at: face.center.map(tidy), normal: face.normal.map(tidy) } };
      }
      const plane = best && probe(best.sel);
      if (plane && Math.abs(dot(plane.n, sf.n)) > 0.9999) {
        on = best.sel; pf = plane; away = 0;
        report.notes.push(`${name} is suppressed here and in the export away from its face (as SolidWorks last solved it): it is put on the face of ${f.parents.join(", ")} it faces the way of, ${best.d.toFixed(3)} mm along its normal from there.`);
      }
    }
    let planeMade = null;
    if ((!on || away > NEAR) && !onFace && !needFace) {
      planeMade = planeWhere(name + " plane", sf);
      if (planeMade) { on = planeMade; pf = probe(planeMade); away = 0; }
    }
    if (!on || (away > NEAR && !onFace)) return { error: needFace ? "no flat face of the part lies where the sketch does" : "no plane or face lies where the sketch does" };

    const toPeet = p => { const d = sub(fromSketch(sf, p), pf.o); return [tidy(dot(d, pf.x)), tidy(dot(d, pf.y))]; };
    const sameHand = dot(cross(pf.x, pf.y), cross(sf.x, sf.y)) > 0;
    const items = [];
    const left = new Set();
    const own = [];   // the SolidWorks segments drawn: { g, as, turned } (an arc's ends may be the other way round)
    if (draw) items.push(...draw(toPeet));
    else {
      const ends = [];
      s.segments.forEach((g, i) => {
        const as = "e" + i;
        const construction = g.construction || holes ? { construction: true } : {};
        if (g.type === "line" && g.start && g.end) {
          items.push({ type: "line", from: toPeet(g.start), to: toPeet(g.end), ...construction, as });
          ends.push([as + ".start", toPeet(g.start)], [as + ".end", toPeet(g.end)]);
          own.push({ g, as, turned: false });
        } else if (g.type === "arc" && g.circle) {
          items.push({ type: "circle", center: toPeet(g.center), radius: tidy(g.radius * MM), ...construction, as });
          own.push({ g, as, turned: false });
        } else if (g.type === "arc") {
          // PeetCAD's arcs run counter-clockwise from start to end
          const ccw = (g.direction === 1) === sameHand;
          const [start, end] = ccw ? [g.start, g.end] : [g.end, g.start];
          items.push({ type: "arc", center: toPeet(g.center), start: toPeet(start), end: toPeet(end), ...construction, as });
          ends.push([as + ".start", toPeet(start)], [as + ".end", toPeet(end)]);
          own.push({ g, as, turned: !ccw });
        } else if (g.type === "spline" && g.points && g.points.length >= 2) {
          // SolidWorks' spline points are the points it runs through
          const points = g.points.map(toPeet);
          items.push({ type: "spline", points, ...construction, as });
          ends.push([as + ".start", points[0]], [as + ".end", points[points.length - 1]]);
          own.push({ g, as, turned: false });
        } else if (g.type === "text" && sameHand) {
          // Its point is the sketch point with the text's id. Its font, height, width and spacing are in
          // exports from the fifth on; before, they are fitted to SolidWorks' faces of the feature that uses
          // the sketch, once it is drawn.
          const at = textPoint(s, g) || { x: 0, y: 0 };
          const string = g.text.replace(/<[^>]*>/g, "");
          if (string !== g.text) report.notes.push(`${name}'s text ${JSON.stringify(g.text)} has formatting, left out: drawn as ${JSON.stringify(string)}.`);
          // SolidWorks' x axis in the PeetCAD sketch: which way the text runs, turned by the text's own angle
          const format = textFormat(g);
          const angle = Math.atan2(dot(sf.x, pf.y), dot(sf.x, pf.x)) * 180 / Math.PI + (format && format.escapement ? format.escapement * 180 / Math.PI : 0);
          const style = !format ? {} : {
            font: fontOf(format),
            ...(format.charHeight > 0 ? { height: tidy(format.charHeight * MM) } : {}),
            ...(format.widthFactor > 0 ? { width: tidy(format.widthFactor) } : {}),
            ...(format.charSpacingFactor > 0 ? { spacing: tidy(format.charSpacingFactor) } : {})
          };
          items.push({ type: "text", at: toPeet(at), string, angle: tidy(angle), ...style, ...construction, as });
          own.push({ g, as, turned: false, text: true });
        } else left.add(g.type === "text" ? "text (mirrored, which PeetCAD's text can't be)" : g.type);
      });
      // Points drawn on their own, which relations and dimensions go to
      const used = new Set(s.segments.flatMap(g => ["start", "end", "center"].filter(k => g[k]).map(k => spot(g[k]))));
      for (const g of s.segments) if (g.type === "text" && textPoint(s, g)) used.add(spot(textPoint(s, g)));
      s.points.forEach((p, i) => {
        if (used.has(spot(p)) || (Math.abs(p.x) < 1e-12 && Math.abs(p.y) < 1e-12)) return;
        used.add(spot(p));
        items.push({ type: "point", at: toPeet(p), as: "p" + i });
        own.push({ point: p, as: "p" + i });
      });
      // Ends that meet are joined, as they are in SolidWorks, so that outlines close
      const groups = new Map();
      for (const [label, p] of ends) {
        const key = p.map(v => v.toFixed(4)).join(",");
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(label);
      }
      for (const labels of groups.values()) for (let i = 1; i < labels.length; i++) items.push({ type: "coincident", of: [labels[0], labels[i]] });
      // A hole wizard's sketch: a circle of the hole's size on each of its points, which keeps to the
      // point when the sketch's dimensions move it
      if (holes) holes.points.forEach((p, k) => {
        const key = toPeet(p).map(v => v.toFixed(4)).join(",");
        const mine = s.points.findIndex(q => spot(q) === spot(p));
        const label = groups.has(key) ? groups.get(key)[0] : mine >= 0 && items.some(x => x.as === "p" + mine) ? "p" + mine : null;
        items.push({ type: "circle", center: toPeet(p), radius: tidy(holes.radius), as: "h" + k });
        if (label) items.push({ type: "coincident", of: ["h" + k + ".center", label] });
      });
    }
    if (items.length === 0) return { error: left.size ? `only ${[...left].join(" and ")}, which PeetCAD sketches don't have` : "nothing in it" };

    const reply = peet.apply({ op: "sketch", on, name, draw: items });
    if (!reply.ok) return { error: reply.error };
    const partial = [...left];
    if (s.blockInstances) partial.push("sketch blocks");
    if (s.blocksDrawn) report.notes.push(`${name} has ${s.blocksDrawn} sketch block${s.blocksDrawn > 1 ? "s" : ""}, drawn as plain geometry.`);
    if (away > 1e-4) report.notes.push(`${name} is on a face ${away.toFixed(3)} mm from where SolidWorks has it.`);
    const result = { name: reply.created[0].name, toPeet, frame: pf, partial, dimensions: new Map(), ids: new Map(), values: new Map((f.dimensions || []).map(d => [d.fullName, d.value])) };
    for (const o of own) { const d = (reply.drawn || []).find(x => x.as === o.as); if (d && o.g) result.ids.set(o.g.id.join("."), d.id); }
    // What was drawn where SolidWorks has it, in the model, for another sketch that relates to it
    result.drawnAt = [];
    for (const o of own) {
      const d = (reply.drawn || []).find(x => x.as === o.as);
      if (!d || o.text) continue;
      const at = p => p && mul(fromSketch(sf, p), 1);
      if (o.point) { result.drawnAt.push({ type: "point", id: d.id, at: at(o.point) }); continue; }
      const g = o.g;
      result.drawnAt.push({ type: g.type, circle: !!g.circle, id: d.id, start: at(g.start), end: at(g.end), center: at(g.center), radius: g.radius && g.radius * MM, normal: sf.n });
      for (const [k, id] of [["start", o.turned ? d.end : d.start], ["end", o.turned ? d.start : d.end], ["center", d.center]]) if (g[k] && id !== undefined) result.drawnAt.push({ type: "point", id, at: at(g[k]) });
    }
    for (const o of own) if (o.text) fitText(f, result, o.g, (reply.drawn || []).find(x => x.as === o.as));
    if (!draw && relations) relate(f, result, s, sf, own, reply.drawn || []);
    return result;
  }

  /**
   * Fits a text's font and height to SolidWorks' faces of the feature that uses its sketch (a cut of the
   * text), which the export has and the text's own font and height are not. Each font that SolidWorks
   * libraries use for text is tried at the height that makes the text as tall as those faces; the one whose
   * outline's box is nearest theirs is kept. A text no feature uses keeps PeetCAD's default.
   */
  function fitText(f, sk, g, drawn) {
    if (!drawn || !drawn.id) return;
    const segments = (f.sketch.segments || []).filter(x => !x.construction);
    if (segments.some(x => x.type !== "text")) return;
    const faces = (f.children || []).flatMap(n => facesOf(n)).filter(face => face.box);
    if (!faces.length) return;
    const pf = sk.frame;
    const lo = [Infinity, Infinity], hi = [-Infinity, -Infinity];
    for (const face of faces) {
      const [a, b] = [mm(face.box.slice(0, 3)), mm(face.box.slice(3))];
      for (const corner of [0, 1, 2, 3, 4, 5, 6, 7].map(k => [k & 1 ? b[0] : a[0], k & 2 ? b[1] : a[1], k & 4 ? b[2] : a[2]])) {
        const d = sub(corner, pf.o);
        const q = [dot(d, pf.x), dot(d, pf.y)];
        for (const i of [0, 1]) { lo[i] = Math.min(lo[i], q[i]); hi[i] = Math.max(hi[i], q[i]); }
      }
    }
    const set = (font, height) => {
      const reply = peet.apply({ op: "set_text", sketch: sk.name, text: drawn.id, font, height: tidy(height) });
      return reply.ok && reply.text && reply.text.bounds ? reply.text : null;
    };
    const offOf = t => Math.max(...[0, 1].flatMap(i => [Math.abs(t.bounds[0][i] - lo[i]), Math.abs(t.bounds[1][i] - hi[i])]));
    // The export has the text's font: it is kept, and how far its outline is from SolidWorks' faces said
    const format = textFormat(g);
    if (format && format.charHeight > 0) {
      const font = fontOf(format), height = tidy(format.charHeight * MM);
      const t = set(font, height);
      if (!t) return;
      const off = offOf(t);
      let note = `${sk.name}'s text ${JSON.stringify(g.text)}: ${font}${t.drawn_in ? ` (not installed: drawn in ${t.drawn_in})` : ""}, ${height} mm high, as the export has it; its outline's box is ${off.toFixed(4)} mm from SolidWorks' faces of ${f.children.join(", ")}`;
      // Not the size SolidWorks' faces are: the height that makes it so, kept with the exported font, says by
      // how much PeetCAD's height (a capital letter's) and SolidWorks' differ
      if (off > 0.05) {
        const [tl, th] = t.bounds;
        const span = hi[1] - lo[1] >= hi[0] - lo[0] ? 1 : 0;
        const fitted = height * (hi[span] - lo[span]) / (th[span] - tl[span]);
        const u = fitted > 0 && isFinite(fitted) ? set(font, Math.round(fitted * 1e4) / 1e4) : null;
        if (u && offOf(u) < off) note += `; made ${u.height} mm high to fit them (${(u.height / height).toFixed(4)} times SolidWorks' height), ${offOf(u).toFixed(4)} mm from them`;
        else set(font, height);
      }
      report.notes.push(note + ".");
      return;
    }
    let best = null;
    for (const font of TEXT_FONTS) {
      const trial = set(font, 10);
      if (!trial) continue;
      // The outline grows from the text's point in proportion to its height
      const [tl, th] = trial.bounds;
      const span = hi[1] - lo[1] >= hi[0] - lo[0] ? 1 : 0;
      const height = 10 * (hi[span] - lo[span]) / (th[span] - tl[span]);
      const t = height > 0 && isFinite(height) ? set(font, Math.round(height * 1e4) / 1e4) : null;
      if (!t) continue;
      const off = offOf(t);
      if (!best || off < best.off) best = { font, height: t.height, off, drawnIn: t.drawn_in };
    }
    if (!best) return;
    set(best.font, best.height);
    report.notes.push(`${sk.name}'s text ${JSON.stringify(g.text)}: ${best.font}${best.drawnIn ? ` (not installed: drawn in ${best.drawnIn})` : ""}, ${best.height} mm high, fitted to SolidWorks' faces of ${f.children.join(", ")} (its outline's box is ${best.off.toFixed(4)} mm from theirs): the export has no font or height.`);
  }

  /**
   * Adds a sketch's relations and dimensions, so that the sketch can be driven as it is in SolidWorks.
   * They are added all at once; if that moves the geometry or over-defines the sketch they are taken back and
   * added one at a time, keeping those that leave the sketch as it was drawn. In a sketch SolidWorks has
   * suppressed, relations to the model and dimensions that move it are kept too, as SolidWorks solves it again.
   */
  function relate(f, sk, s, sf, own, drawn) {
    const pf = sk.frame, name = sk.name;
    const unsolved = unsolvedSketches.has(f.name);
    const tally = { relations: s.relations.length, kept: 0, dropped: {} };
    let current = null;
    const drop = why => { tally.dropped[why] = (tally.dropped[why] || 0) + 1; if (process.env.SW_DROPS && current) console.error("DROP", name, why, JSON.stringify(current)); };
    report.sketches = report.sketches || {};
    report.sketches[name] = tally;

    // The entity ids PeetCAD gave what was drawn, found again from SolidWorks' coordinates
    const ids = new Map(drawn.filter(d => d.as).map(d => [d.as, d]));
    // Segments by what they were drawn as: SolidWorks numbers each kind of segment on its own, so a line
    // and an arc can have the same id
    const points = new Map(), segments = new Map(), segmentIds = new Map();
    for (const o of own) {
      const d = ids.get(o.as);
      if (!d) continue;
      if (o.point) { points.set(spot(o.point), d.id); continue; }
      // A text's point is the one thing of it a relation can tie
      if (o.text) { const at = textPoint(s, o.g); if (at && d.point !== undefined && !points.has(spot(at))) points.set(spot(at), d.point); continue; }
      segments.set(o.as, { id: d.id, g: o.g, start: o.turned ? d.end : d.start, end: o.turned ? d.start : d.end, center: d.center, turned: !!o.turned });
      if (!segmentIds.has(o.g.id.join("."))) segmentIds.set(o.g.id.join("."), segments.get(o.as));
      const set = (p, id) => { if (p && id !== undefined && !points.has(spot(p))) points.set(spot(p), id); };
      set(o.g.start, o.turned ? d.end : d.start);
      set(o.g.end, o.turned ? d.start : d.end);
      set(o.g.center, d.center);
    }
    const atOrigin = sk.toPeet({ x: 0, y: 0 });
    if (!points.has(spot({ x: 0, y: 0 })) && Math.abs(atOrigin[0]) < 1e-6 && Math.abs(atOrigin[1]) < 1e-6) points.set(spot({ x: 0, y: 0 }), "origin");

    const byId = new Map(s.points.map(p => [p.id.join("."), p]));
    const originOk = Math.abs(atOrigin[0]) < 1e-6 && Math.abs(atOrigin[1]) < 1e-6;
    const pointOf = e => {
      const mine = byId.get(e.id.join("."));
      // A point at the origin that is not one of the sketch's own is the origin itself, which SolidWorks
      // does not list among a sketch's points
      // (A block's point at its own origin is given there too: it is found where the block puts it.)
      const atZero = e.x !== undefined && Math.abs(e.x) < 1e-12 && Math.abs(e.y) < 1e-12;
      if (atZero && (s.blocks || []).some(b => (b.points || []).some(q => q.id.join(".") === e.id.join(".") && Math.abs(q.x) < 1e-12 && Math.abs(q.y) < 1e-12))) return undefined;
      if (!mine && atZero) return originOk ? "origin" : undefined;
      const p = e.x !== undefined ? e : mine;   // the first export has ids only
      return p ? points.get(spot(p)) : undefined;
    };
    // A segment is found by where it is. The ids a relation gives are not the ids the sketch lists its
    // segments under (they can be another segment's, or nothing's), so they are only used for the first
    // export, which has nothing else.
    const shapeOf = g => g.type === "line" && g.start && g.end ? "line " + [spot(g.start), spot(g.end)].sort().join(" ")
      : g.type === "arc" && g.center ? `arc ${spot(g.center)} ${Math.round(g.radius * 1e7)} ${g.start && g.end ? [spot(g.start), spot(g.end)].sort().join(" ") : ""}` : null;
    const byShape = new Map();
    for (const found of segments.values()) { const key = shapeOf(found.g); if (key && !byShape.has(key)) byShape.set(key, found); }
    const segmentOf = e => e.type ? byShape.get(shapeOf(e)) : segmentIds.get(e.id.join("."));

    // Which way round the sketch's axes are in PeetCAD's sketch: horizontal there may be vertical here
    const across = Math.abs(dot(sf.x, pf.x)) > 0.9999 ? false : Math.abs(dot(sf.x, pf.y)) > 0.9999 ? true : null;
    const level = which => across === null ? null : (which === "horizontal") !== across ? "horizontal" : "vertical";

    // What a relation refers to outside the sketch is projected into it first
    const projected = new Map();
    const projectItem = item => {
      const key = JSON.stringify(item);
      if (!projected.has(key)) {
        const reply = peet.apply({ op: "draw", sketch: name, draw: [item] });
        const made = reply.ok && (reply.drawn || [])[0];
        // A circular edge seen face-on comes in as an arc or a circle, which takes what the sketch's arcs take
        const round = !!made && (made.type === "arc" || made.type === "circle");
        // A straight edge projects to a line from its start to its end
        const ends = made && made.type === "line" && item.edge && item.edge.between ? item.edge.between.map((p, i) => ({ id: [made.start, made.end][i], at: flat(p) })) : undefined;
        projected.set(key, made ? { id: made.id !== undefined ? made.id : (made.curves || made.points || [])[0], point: item.vertex !== undefined || made.type === "point", round, circle: made.type === "circle", center: round ? made.center : undefined, ends } : null);
      }
      return projected.get(key) || undefined;
    };
    const project = d => {
      if (!d) return undefined;
      let item = null;
      if (d.kind === "edge" && d.curve === "line" && d.start && d.end) {
        // The edge with SolidWorks' ends if PeetCAD has one (a midpoint or an end of it is then the
        // same), else the one through the middle of SolidWorks'
        const whole = projectItem({ type: "project", edge: { between: [mm(d.start).map(tidy), mm(d.end).map(tidy)] } });
        if (whole && whole.id !== undefined) return whole;
        item = { type: "project", edge: { at: mid(mm(d.start), mm(d.end)).map(tidy) } };
      }
      else if (d.kind === "edge" && d.curve === "circle" && d.circle) {
        // An arc (a bend's end), found by the middle of it; a half circle by a point a quarter round from its
        // start, either way; a whole circle (the export gives no ends) by any point of it
        const c = mm(d.circle.slice(0, 3)), axis = unit(d.circle.slice(3, 6)), r = d.circle[6] * MM;
        const tries = [];
        if (d.start && d.end) {
          const half = add(sub(mm(d.start), c), sub(mm(d.end), c)), n = len(half);
          const q = cross(axis, sub(mm(d.start), c)), l = len(q);
          if (n > 1e-6 * r) tries.push(add(c, mul(half, r / n)));
          else if (l > 1e-9) tries.push(add(c, mul(q, r / l)), sub(c, mul(q, r / l)));
        } else {
          const q = cross(axis, Math.abs(axis[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0]);
          tries.push(add(c, mul(q, r / len(q))));
        }
        for (const at of tries) {
          const p = projectItem({ type: "project", edge: { at: at.map(tidy) } });
          if (p && p.id !== undefined) return p;
        }
        return undefined;
      }
      else if (d.kind === "face" && d.surface === "cylinder" && d.cylinder) {
        // A point on a cylinder's axis (a circle's centre put on a hole's): the centre of a circular edge of it
        const c = mm(d.cylinder.slice(0, 3)), axis = unit(d.cylinder.slice(3, 6)), r = d.cylinder[6] * MM;
        const off = p => { const v = sub(p, c); return len(sub(v, mul(axis, dot(v, axis)))); };
        for (const x of modelEdges()) {
          if (x.curve !== "circle" || [x.from, x.middle].some(p => Math.abs(off(p) - r) > 1e-3) || Math.abs(dot(sub(x.from, x.middle), axis)) > 1e-6) continue;
          const p = projectItem({ type: "project", edge: { at: x.middle } });
          if (p && p.round && p.center !== undefined) return { id: p.center, point: true };
        }
        return undefined;
      }
      else if (d.kind === "vertex" && d.point) item = { type: "project", vertex: { at: mm(d.point).map(tidy) } };
      else if (d.kind === "feature" && d.type === "RefPlane" && planes.has(d.name)) item = { type: "project", plane: planes.get(d.name) };
      return item ? projectItem(item) : undefined;
    };

    // A relation to the model ties a hidden copy of the edge or vertex in the sketch, which the sketch does not
    // list. Where the export has no `defined` for it (an edge that later features cut away, or that SolidWorks
    // has lost track of) or that could not be projected (a face seen edge-on), the model edge or vertex the
    // copy lies over is found among the part's edges as they are now (the sketch's place in the tree) and
    // projected. Its copy is where the edge is seen from the sketch, so the edge may be off the sketch's plane.
    let edgesNow = null;
    const flat = p => { const d = sub(p, pf.o); return [dot(d, pf.x), dot(d, pf.y), dot(d, pf.n)]; };
    // Not an edge of a feature that is suppressed in some configuration the sketch is live in: the sketch
    // would lose it there, where SolidWorks' copy stays where it was
    const modelEdges = () => {
      if (edgesNow) return edgesNow;
      const unsure = new Set(), off = switchedWhereLive(f.name);
      for (const face of peet.ask({ op: "faces" }).faces || []) if ((face.made_by || []).some(m => off.has(m.feature))) unsure.add(face.body + "|" + face.index);
      edgesNow = (peet.ask({ op: "edges" }).edges || []).filter(e => !(e.faces || []).some(i => unsure.has(e.body + "|" + i)))
        .map(e => ({ ...e, a: flat(e.from), b: flat(e.to), m: flat(e.middle) }));
      return edgesNow;
    };
    const hidden = e => {
      const near = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]);
      let best = null, item = null;
      const consider = (score, it) => { if (best === null || score < best) { best = score; item = it; } };
      if (e.kind === "sketchPoint") {
        const at = sk.toPeet(e);
        for (const x of modelEdges()) for (const [q, w] of [[x.a, x.from], [x.b, x.to]]) if (near(q, at) < 1e-3) consider(Math.abs(q[2]), { type: "project", vertex: { at: w } });
      } else if (e.type === "line" && e.start && e.end) {
        const [s0, s1] = [sk.toPeet(e.start), sk.toPeet(e.end)];
        const d = [s1[0] - s0[0], s1[1] - s0[1]], l = Math.hypot(...d);
        if (l < 1e-6) return undefined;
        const off = q => Math.abs(d[0] * (q[1] - s0[1]) - d[1] * (q[0] - s0[0])) / l;
        const along = q => ((q[0] - s0[0]) * d[0] + (q[1] - s0[1]) * d[1]) / l;
        for (const x of modelEdges()) {
          // A circular edge seen edge on (a shaft's end seen from its side) covers a line: taken where no
          // straight edge does
          if (x.curve === "circle" && off(x.a) < 1e-3 && off(x.m) < 1e-3 && near(x.a, x.m) > 1e-6) {
            consider(1e6 + Math.abs(x.m[2]), { type: "project", edge: { at: x.middle } });
            continue;
          }
          if (x.curve !== "line" || Math.hypot(x.a[0] - x.b[0], x.a[1] - x.b[1]) < 1e-6) continue;
          if (off(x.a) > 1e-3 || off(x.b) > 1e-3 || Math.abs(x.a[2] - x.b[2]) > 1e-3) continue;
          // Over the copy (or part of it), the nearest to the sketch first, then the one whose ends are the copy's
          const [lo, hi] = [Math.min(along(x.a), along(x.b)), Math.max(along(x.a), along(x.b))];
          if (hi < 1e-3 || lo > l - 1e-3) continue;
          const ends = Math.min(near(x.a, s0) + near(x.b, s1), near(x.a, s1) + near(x.b, s0));
          consider(Math.abs(x.a[2]) * 1e3 + ends, { type: "project", edge: { between: [x.from, x.to] } });
        }
      } else if (e.type === "arc" && e.center && e.radius) {
        const c = sk.toPeet(e.center), r = e.radius * MM;
        for (const x of modelEdges()) {
          if (x.curve !== "circle" || [x.a, x.b, x.m].some(q => Math.abs(near(q, c) - r) > 1e-3) || Math.abs(x.a[2] - x.m[2]) > 1e-3) continue;
          consider(Math.abs(x.m[2]), { type: "project", edge: { at: x.middle } });
        }
      }
      return item ? projectItem(item) : fromSketches(e);
    };
    // Failing the model, a copy of another sketch's point, line, circle or arc (SolidWorks relates a sketch
    // to another's entities, and converts them): that entity, seen from this sketch, projected so that it
    // follows the other sketch
    const fromSketches = e => {
      const seen = p => { const d = sub(p, pf.o); return [dot(d, pf.x), dot(d, pf.y)]; };
      const near = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]) < 1e-4;
      for (const other of sketches.values()) {
        if (other.name === name || !other.drawnAt) continue;
        const parallel = x => x.normal && Math.abs(Math.abs(dot(x.normal, pf.n)) - 1) < 1e-9;
        const hit = other.drawnAt.find(x => {
          if (e.kind === "sketchPoint") return x.type === "point" && near(seen(x.at), sk.toPeet(e));
          if (e.type === "line" && e.start && e.end && x.type === "line" && x.start && x.end) {
            const [a, b] = [sk.toPeet(e.start), sk.toPeet(e.end)], [p, q] = [seen(x.start), seen(x.end)];
            return (near(a, p) && near(b, q)) || (near(a, q) && near(b, p));
          }
          if (e.type === "arc" && e.center && x.type === "arc" && x.center && parallel(x))
            return near(sk.toPeet(e.center), seen(x.center)) && Math.abs(e.radius * MM - x.radius) < 1e-4;
          return false;
        });
        if (hit) return projectItem({ type: "project", sketch: other.name, entity: hit.id });
      }
      return undefined;
    };
    // A point of SolidWorks' that is not the sketch's own is a model vertex its relation refers to (Battery
    // Bracket's Sketch8 measures its flange's height from the far corner of the flange's edge): it is
    // projected from where it is, if the model has a vertex there
    const vertexAt = e => e && e.x !== undefined ? projectItem({ type: "project", vertex: { at: fromSketch(sf, e).map(tidy) } }) : undefined;


    /** The model edge a line lies on, projected, with the line's ends coincident with its ends: one item, and the other in `with`. */
    const convertedEdge = l => {
      if (!l.g.start || !l.g.end) return null;
      const made = project({ kind: "edge", curve: "line", start: mul(fromSketch(sf, l.g.start), 1 / MM), end: mul(fromSketch(sf, l.g.end), 1 / MM) });
      if (!made || made.point) return null;
      const entities = peet.ask({ op: "feature", feature: name }).entities || [];
      const edge = entities.find(e => e.id === made.id), line = entities.find(e => e.id === l.id);
      if (!edge || !line || edge.start === undefined || line.start === undefined) return null;
      const d = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]);
      const same = d(line.from, edge.from) + d(line.to, edge.to) <= d(line.from, edge.to) + d(line.to, edge.from);
      return { type: "coincident", of: [line.start, same ? edge.start : edge.end], with: [{ type: "coincident", of: [line.end, same ? edge.end : edge.start] }] };
    };
    const dimensions = new Map((f.dimensions || []).map(d => [d.fullName, d]));
    // A distance to a circle's or arc's near or far side (SolidWorks' arc conditions: Min and Max): its two ends
    // (points, circles or arcs, or a line and a circle or arc), which side of each and which way, measured as
    // PeetCAD's `sides` measure it. The export's arc end conditions (format 6) say which sides where it has them;
    // failing those, the sides that give SolidWorks' value, the fewest away from the centre first. Null where the
    // centres give the value (an ordinary distance) or nothing does.
    // SolidWorks gives the two slots of a symmetric relation between slots as nulls (format 5; format 6 gives
    // them by their centres): they are the two slot centres (points no curve ends or turns on) mirrored about
    // the relation's line, if just one such pair is
    const usedPoints = new Set(s.segments.flatMap(g => [g.start, g.end, g.center].filter(Boolean).map(p => p.id.join("."))));
    const slotted = r => {
      const es = r.entities || [];
      if (r.type !== "symmetric" || es.length !== 3 || es.filter(e => !e).length !== 2) return es;
      const axis = es.find(e => e && e.kind === "sketchSegment" && e.start && e.end);
      if (!axis) return es;
      const [o, t] = [axis.start, axis.end], u = [t.x - o.x, t.y - o.y], n2 = u[0] * u[0] + u[1] * u[1];
      if (n2 < 1e-18) return es;
      const mirror = p => { const q = ((p.x - o.x) * u[0] + (p.y - o.y) * u[1]) / n2; return [2 * (o.x + u[0] * q) - p.x, 2 * (o.y + u[1] * q) - p.y]; };
      const free = s.points.filter(p => !usedPoints.has(p.id.join(".")));
      const pairs = [];
      for (let i = 0; i < free.length; i++) for (let j = i + 1; j < free.length; j++) {
        const m = mirror(free[i]);
        if (Math.hypot(m[0] - free[j].x, m[1] - free[j].y) < 1e-8 && Math.hypot(free[i].x - free[j].x, free[i].y - free[j].y) > 1e-8) pairs.push([free[i], free[j]]);
      }
      // Not a pair another symmetric relation ties already
      const tied = new Set(s.relations.filter(x => x !== r && x.type === "symmetric").flatMap(x => (x.entities || []).filter(e => e && e.kind === "sketchPoint").map(e => e.id.join("."))));
      const open = pairs.filter(([a, b]) => !tied.has(a.id.join(".")) || !tied.has(b.id.join(".")));
      if (open.length !== 1) return es;
      return [{ kind: "sketchPoint", ...open[0][0] }, { kind: "sketchPoint", ...open[0][1] }, axis];
    };
    const ARC_SIDE = { 0: "center", 1: "center", 2: "near", 3: "far" };
    const sideDistance = ({ pts, seen, rounds, lines, outLines, dim, value }) => {
      const ends = [...pts.map(p => ({ id: p.id, kind: "point" })), ...seen.map(p => ({ id: p.id, kind: "point" })),
        ...rounds.map(g => ({ id: g.id, kind: "round" })), ...lines.map(l => ({ id: l.id, kind: "line" })), ...outLines.map(id => ({ id, kind: "line" }))];
      if (ends.length !== 2 || !ends.some(e => e.kind === "round")) return null;
      const now = new Map((peet.ask({ op: "feature", feature: name }).entities || []).map(e => [e.id, e]));
      const geo = e => {
        if (e.id === "origin") return { c: [0, 0], r: 0 };
        const x = now.get(e.id);
        if (!x) return null;
        if (e.kind === "line") return x.from && x.to ? { a: x.from, b: x.to } : null;
        if (e.kind === "point") return x.at ? { c: x.at, r: 0 } : null;
        const r = typeof x.radius === "number" ? x.radius : x.from && x.at ? Math.hypot(x.from[0] - x.at[0], x.from[1] - x.at[1]) : null;
        return x.at && r !== null ? { c: x.at, r } : null;
      };
      const g = ends.map(geo);
      if (g.some(x => !x)) return null;
      const toward = s => s === "near" ? 1 : s === "far" ? -1 : 0;
      const measure = (sides, along) => {
        const li = ends.findIndex(e => e.kind === "line");
        const circular = ends.map(e => e.kind === "round");
        let delta, radii;
        if (li >= 0) {
          if (along !== "straight") return null;
          const l = g[li], o = g[1 - li], d = [l.b[0] - l.a[0], l.b[1] - l.a[1]], n = Math.hypot(...d);
          if (n < 1e-9) return null;
          delta = (d[0] * (o.c[1] - l.a[1]) - d[1] * (o.c[0] - l.a[0])) / n;
          radii = li === 0 ? [0, o.r] : [o.r, 0];
        } else {
          const [a, b] = g;
          delta = along === "straight" ? Math.hypot(b.c[0] - a.c[0], b.c[1] - a.c[1]) : along === "horizontal" ? b.c[0] - a.c[0] : b.c[1] - a.c[1];
          radii = [a.r, b.r];
        }
        const nested = li < 0 && circular[0] && circular[1] && Math.hypot(g[1].c[0] - g[0].c[0], g[1].c[1] - g[0].c[1]) < Math.abs(radii[0] - radii[1]);
        let total = Math.abs(delta);
        for (const k of [0, 1]) total += (nested && radii[k] < radii[1 - k] ? 1 : -1) * toward(sides[k]) * radii[k];
        return Math.abs(total);
      };
      // Which way: along the sketch's x or y where the dimension line says so (in PeetCAD's sketch, which may be
      // turned), else straight
      const line = dim.lineDirection && len(dim.lineDirection) > 1e-9 ? unit(dim.lineDirection.slice(0, 3)) : null;
      const said = line ? (Math.abs(dot(line, sf.x)) > 0.9999 ? "horizontal" : Math.abs(dot(line, sf.y)) > 0.9999 ? "vertical" : null)
        : dim.displayType === 11 ? "horizontal" : dim.displayType === 12 ? "vertical" : null;
      const ways = said && level(said) ? [level(said), "straight"] : ["straight"];
      const options = e => e.kind === "round" ? ["center", "near", "far"] : ["center"];
      const conditions = Array.isArray(dim.arcEndConditions) && dim.arcEndConditions.some(c => c !== null && c !== undefined)
        ? dim.arcEndConditions.map(c => ARC_SIDE[c] || "center").sort().join() : null;
      const away = sides => sides.filter(s => s !== "center").length * 2 + sides.filter(s => s === "far").length;
      const combos = [];
      for (const a of options(ends[0])) for (const b of options(ends[1])) combos.push([a, b]);
      combos.sort((x, y) => away(x) - away(y));
      for (const sides of combos) {
        if (conditions !== null && [...sides].sort().join() !== conditions) continue;
        for (const along of ways) {
          const m = measure(sides, along);
          if (m === null || Math.abs(m - value) > 1e-4) continue;
          if (sides.every(s => s === "center")) return null;
          return { type: along === "straight" ? "distance" : `${along}_distance`, of: ends.map(e => e.id), sides, value };
        }
      }
      return null;
    };
    // An equation sets these, so they drive the sketch even where SolidWorks calls them driven; one whose equation
    // is disabled keeps the value it set and still drives it (VIC CT Metering Panel's D5, another value elsewhere)
    const byEquation = new Set((cur.equations || []).map(e => (/^\s*"([^"]+@[^"]+)"\s*=/.exec(e.expression) || [])[1]).filter(Boolean));
    const names = new Set();
    const items = [];
    // Sketch patterns: how far a copy is moved from its seed, if it is the seed moved (a line either way
    // round, a circle the same size); and each seed's copies
    const moveOf = (a, b) => {
      const d = (p, q) => [q.x - p.x, q.y - p.y], off = (u, v) => Math.hypot(u[0] - v[0], u[1] - v[1]);
      if (a.type === "arc" && b.type === "arc" && a.circle && b.circle && Math.abs(a.radius - b.radius) < 1e-9) return d(a.center, b.center);
      if (a.type !== "line" || b.type !== "line") return null;
      const straight = [d(a.start, b.start), d(a.end, b.end)], crossed = [d(a.start, b.end), d(a.end, b.start)];
      const [t, u] = off(...straight) <= off(...crossed) ? straight : crossed;
      return off(t, u) < 1e-9 ? t : null;
    };
    // Offset Entities' distance, which SolidWorks gives a relation of its own, with nothing it ties
    const offsetRelation = s.relations.find(r => r.type === "offsetedge" && !r.suppressed && r.dimension && dimensions.get(r.dimension));
    const offsetDim = offsetRelation && dimensions.get(offsetRelation.dimension);
    let offsetUsed = false, offsetItem = null;
    const copiesOf = new Map();
    for (const r of s.relations) if (r.type === "patterned" && !r.suppressed && (r.entities || []).length === 2 && r.entities.every(e => e && e.kind === "sketchSegment")) {
      const [seed, copy] = r.entities.map(segmentOf);
      if (!seed || !copy) continue;
      if (!copiesOf.has(seed.id)) copiesOf.set(seed.id, []);
      copiesOf.get(seed.id).push(copy);
    }
    for (const r of s.relations) {
      current = r;
      if (r.suppressed) { drop("suppressed"); continue; }
      const pts = [], segs = [], copies = [], unplaced = [], seen = [];
      // A point that is not the sketch's own, tied to one of its own where it lies, in a relation to the model,
      // is the hidden copy of what the relation is defined by (on it, as the relation has it). So is one a point
      // pierces, which is on another sketch or the model, not this one.
      const hiddenPoint = e => e && e.kind === "sketchPoint" && ((r.defined || []).length || r.type === "atpierce") && e.x !== undefined && !byId.has(e.id.join("."))
        && (r.entities || []).some(o => o && o !== e && o.kind === "sketchPoint" && o.x !== undefined && byId.has(o.id.join(".")) && spot(o) === spot(e));
      const ents = slotted(r), bySlots = (!!r.entities && ents !== r.entities) || ents.some(e => e && e.kind === "sketchSlot");
      for (const e0 of ents) {
        // A slot (format 6 gives it by its centre) is its centre point
        const e = e0 && e0.kind === "sketchSlot" ? e0.centerPoint || (e0.center && { kind: "sketchPoint", id: [-1, -1], ...e0.center }) : e0;
        if (!e) continue;
        if (e0 !== e && e.kind === undefined) e.kind = "sketchPoint";
        if (hiddenPoint(e)) { copies.push(e); continue; }
        if (e.kind === "sketchPoint") { const id = pointOf(e); if (id !== undefined) pts.push({ id, e: e.x !== undefined ? e : byId.get(e.id.join(".")) }); else if (e.x !== undefined) unplaced.push(e); }
        else if (e.kind === "sketchSegment") {
          const g = segmentOf(e);
          // A converted edge's hidden copy lies on the segment converted from it: it is the model's edge
          if (g && segs.includes(g)) copies.push(e);
          else if (g) segs.push(g); else if (e.type) unplaced.push(e);
        }
      }
      // What is in a block is given where the block's definition has it: it is found where an instance puts
      // it, the instance nearest what else the relation ties where more than one would do
      const placedAt = [...pts.map(p => p.e), ...segs.map(g => g.g.start || g.g.center)].filter(Boolean);
      for (const e of unplaced) {
        let found = null;
        for (const block of s.blocks || []) {
          const there = { ...inBlock(block, e), id: e.id };
          const hit = e.kind === "sketchPoint" ? pointOf(there) : segmentOf(there);
          if (hit === undefined) continue;
          const at = there.x !== undefined ? there : there.start || there.center;
          const gap = placedAt.length && at ? Math.min(...placedAt.map(q => Math.hypot(q.x - at.x, q.y - at.y))) : 0;
          if (!found || gap < found.gap) found = { gap, hit, there };
        }
        if (!found) copies.push(e);
        else if (e.kind === "sketchPoint") pts.push({ id: found.hit, e: found.there });
        else segs.push(found.hit);
      }
      let outside = [];
      let lost = false;
      for (const d of r.defined || []) {
        if (!d && bySlots) continue;
        const p = project(d);
        if (p && p.id !== undefined) {
          outside.push(p);
          // A vertex where the sketch sees it, in the sketch's own coordinates (metres)
          if (p.point && d.kind === "vertex" && d.point) { const q = sub(mm(d.point), sf.o); seen.push({ id: p.id, e: { x: dot(q, sf.x) / MM, y: dot(q, sf.y) / MM } }); }
        } else lost = true;
      }
      // The hidden copies stand for what the relation is defined by where the export has nothing for it, or
      // where what it has could not be projected
      if (copies.length && (!r.defined || (lost && copies.length === r.defined.length))) {
        const found = copies.map(hidden);
        if (found.every(p => p && p.id !== undefined)) {
          outside = found; lost = false;
          // A vertex's copy is where the sketch sees it, as the sketch's own points are
          copies.forEach((e, i) => { if (found[i].point && e.x !== undefined) seen.push({ id: found[i].id, e }); });
        }
      }
      // An offset's hidden copy is the offset curve itself, which nothing in the model is: its source is found
      // from the offset's distance below
      if (r.type === "offsetedge") { lost = false; outside = []; }
      if (lost) { drop(`${r.type} to something outside the sketch that could not be projected`); continue; }
      for (const o of outside) if (o.round) segs.push({ id: o.id, center: o.center, g: { type: "arc", circle: o.circle }, projected: true });
      const lines = segs.filter(g => g.g.type === "line"), rounds = segs.filter(g => g.g.type === "arc");
      const outLines = outside.filter(o => !o.point && !o.round).map(o => o.id), outPoints = outside.filter(o => o.point).map(o => o.id);
      const anyLines = [...lines.map(l => l.id), ...outLines], anyPoints = [...pts.map(p => p.id), ...outPoints];
      const all = [...anyPoints, ...segs.map(g => g.id), ...outLines];
      const dim = r.dimension && dimensions.get(r.dimension);
      // Two things SolidWorks ties that are one in PeetCAD's sketch (ends the importer joined when it drew
      // them, or a line SolidWorks lists twice) are tied already
      if (!dim && all.length === 2 && all[0] === all[1] && /^(coincident|colinear|useedge|parallel|samelength|concentric|coradial|horizontal|vertical|horizpoints|vertpoints)$/.test(r.type)) { tally.kept++; continue; }
      let item = null;
      switch (r.type) {
        case "horizontal": case "vertical":
          if (level(r.type) && (lines.length === 1 || anyPoints.length === 2)) item = { type: level(r.type), of: lines.length === 1 ? [lines[0].id] : anyPoints };
          break;
        case "horizpoints": case "vertpoints":
          if (level(r.type.slice(0, 4) === "hori" ? "horizontal" : "vertical") && anyPoints.length === 2) item = { type: level(r.type.slice(0, 4) === "hori" ? "horizontal" : "vertical"), of: anyPoints };
          break;
        // A point where a curve of another sketch, or of the model, pierces this one's plane: SolidWorks' copy of
        // it is a point there, which this one is on
        case "atpierce": if (anyPoints.length === 2 && all.length === 2) item = { type: "coincident", of: anyPoints }; break;
        case "coincident": if (all.length === 2 && anyPoints.length >= 1) item = { type: "coincident", of: [anyPoints[0], all.find(x => x !== anyPoints[0])] }; break;
        case "colinear": case "useedge": {
          // A line converted from an edge is the edge: its ends are the edge's ends, which it follows
          const edge = outside.length === 1 && outside[0].ends;
          if (r.type === "useedge" && lines.length === 1 && edge && lines[0].start !== undefined) {
            const [a, b] = [sk.toPeet(lines[0].g.start), sk.toPeet(lines[0].g.end)];
            const gap = (p, q) => Math.hypot(p[0] - q.at[0], p[1] - q.at[1]);
            const [s0, s1] = gap(a, edge[0]) + gap(b, edge[1]) <= gap(a, edge[1]) + gap(b, edge[0]) ? edge : [edge[1], edge[0]];
            if (gap(a, s0) < 1e-4 && gap(b, s1) < 1e-4) { item = { type: "coincident", of: [lines[0].start, s0.id], with: [{ type: "coincident", of: [lines[0].end, s1.id] }] }; break; }
          }
          // A line converted from a model edge the export doesn't name (both its entities are the line, drawn where
          // the edge is): its ends on the edge's, so that it follows the edge (nVent's flange outline, 65 mm or 800 mm long)
          if (r.type === "useedge" && lines.length >= 1 && lines.every(l => l.id === lines[0].id) && outLines.length === 0) { item = convertedEdge(lines[0]); if (item) break; }
          if (anyLines.length === 2) item = { type: "collinear", of: anyLines };
          // An arc on a circular edge (converted from it)
          else if (r.type === "useedge" && anyLines.length === 0 && anyPoints.length === 0 && rounds.length === 2) item = { type: "coradial", of: rounds.map(g => g.id) };
          break;
        }
        case "parallel": case "perpendicular": if (anyLines.length === 2) item = { type: r.type, of: anyLines }; break;
        case "tangent": if (segs.length + outLines.length === 2 && rounds.length >= 1) item = { type: "tangent", of: [...segs.map(g => g.id), ...outLines] }; break;
        case "concentric": if (rounds.length === 2) item = { type: "concentric", of: rounds.map(g => g.id) }; break;
        case "coradial": if (rounds.length === 2) item = { type: "coradial", of: rounds.map(g => g.id) }; break;
        case "samelength": if (segs.length === 2) item = { type: "equal", of: segs.map(g => g.id) }; break;
        case "atmiddle":
          if (anyPoints.length === 1 && anyLines.length === 1) item = { type: "midpoint", of: [anyPoints[0], anyLines[0]] };
          // The middle of an arc (a circle has none)
          else if (anyPoints.length === 1 && anyLines.length === 0 && rounds.length === 1 && rounds[0].g.circle === false) item = { type: "midpoint", of: [anyPoints[0], rounds[0].id] };
          break;
        case "fixed": if (all.length === 1) item = { type: "fix", of: all }; break;
        case "symmetric":
          if (anyPoints.length === 2 && anyLines.length === 1) item = { type: "symmetric", of: [...anyPoints, anyLines[0]] };
          // Two circles or arcs about a line: their centres mirrored, the same size
          else if (anyPoints.length === 0 && rounds.length === 2 && anyLines.length === 1 && rounds.every(g => g.center !== undefined))
            item = { type: "symmetric", of: [...rounds.map(g => g.center), anyLines[0]], with: [{ type: "equal", of: rounds.map(g => g.id) }] };
          // Two lines about a third: each end mirrored onto one of the other's
          else if (anyPoints.length === 0 && rounds.length === 0 && lines.length === 3 && outLines.length === 0) {
            // Which line is the axis is not said: the one the other two are mirror images about
            let best = null;
            for (const k of [2, 0, 1]) {
              const axis = lines[k], [a, b] = lines.filter((_, i) => i !== k);
              const [o, t] = [[axis.g.start.x, axis.g.start.y], [axis.g.end.x, axis.g.end.y]];
              const u = [t[0] - o[0], t[1] - o[1]], n = Math.hypot(...u);
              if (n < 1e-12) continue;
              const mirror = p => { const q = ((p.x - o[0]) * u[0] + (p.y - o[1]) * u[1]) / (n * n), f = [o[0] + u[0] * q, o[1] + u[1] * q]; return [2 * f[0] - p.x, 2 * f[1] - p.y]; };
              const gap = (p, q) => { const m = mirror(p); return Math.hypot(m[0] - q.x, m[1] - q.y); };
              // (`start` and `end` are SolidWorks' ends, whichever way PeetCAD runs the segment)
              const straight = gap(a.g.start, b.g.start) + gap(a.g.end, b.g.end), crossed = gap(a.g.start, b.g.end) + gap(a.g.end, b.g.start);
              const off = Math.min(straight, crossed);
              if (off < 1e-7 && (!best || off < best.off))
                best = { off, item: { type: "symmetric", of: [a.start, straight <= crossed ? b.start : b.end, axis.id], with: [{ type: "symmetric", of: [a.end, straight <= crossed ? b.end : b.start, axis.id] }] } };
            }
            if (best) item = best.item;
          }
          break;
        // A copy in a linear sketch pattern: the seed moved a whole number of the pattern's steps, the step
        // being where its first copy is
        case "patterned": {
          if (segs.length !== 2 || outside.length || segs.some(g => !g.g.start && !g.g.center)) break;
          const [seed, copy] = segs;
          const moved = moveOf(seed.g, copy.g);
          // A side of a polygon turned from the one before (SolidWorks' polygon ties each side to the next so): the
          // same length, both tangent to the polygon's circle, which keeps it regular as SolidWorks' does
          const touching = !moved && seed.g.type === "line" && copy.g.type === "line" && [...segments.values()].find(c => {
            if (c.g.type !== "arc" || !c.g.center || !(c.g.radius > 0)) return false;
            const gap = l => { const d = [l.end.x - l.start.x, l.end.y - l.start.y]; return Math.abs(d[0] * (c.g.center.y - l.start.y) - d[1] * (c.g.center.x - l.start.x)) / Math.hypot(...d); };
            return [seed.g, copy.g].every(l => Math.abs(gap(l) - c.g.radius) < 1e-9);
          });
          if (touching) { item = { type: "equal", of: [seed.id, copy.id], with: [{ type: "tangent", of: [copy.id, touching.id] }] }; break; }
          if (!moved && seed.g.type === copy.g.type && (seed.g.type === "line" || (seed.g.circle && copy.g.circle))) { drop("patterned: a copy turned from its seed (a circular sketch pattern), which PeetCAD has no relation for"); continue; }
          if (!moved) break;
          // The first copy: the nearest of the seed's copies that this one is a whole number of steps from
          let first = null;
          for (const other of copiesOf.get(seed.id) || []) {
            const step = moveOf(seed.g, other.g);
            const l2 = step && step[0] * step[0] + step[1] * step[1];
            if (!l2 || Math.abs(step[0] * moved[1] - step[1] * moved[0]) > 1e-9 * Math.sqrt(l2)) continue;
            const n = (moved[0] * step[0] + moved[1] * step[1]) / l2;
            if (n < 0.5 || Math.abs(n - Math.round(n)) > 1e-6) continue;
            if (!first || l2 < first.l2) first = { g: other, l2 };
          }
          if (first) item = { type: "patterned", of: first.g.id === copy.id ? [seed.id, copy.id] : [seed.id, copy.id, first.g.id] };
          break;
        }
        // Offset Entities: the offset curve kept offset from the model edge it is offset from (PeetCAD's
        // `offset` dimension), one distance (SolidWorks' dimension) for every piece. The edge is the one that far
        // from the curve (parallel to a line, round the same centre as an arc), nearest the sketch.
        case "offsetedge": {
          if (!offsetDim) break;
          // (The distance's own relation is kept with the first piece)
          if (!r.entities || !r.entities.length) continue;
          const own = segs.length === 1 && !segs[0].projected ? segs[0] : null;
          if (!own || !(own.g.type === "line" ? own.g.start && own.g.end : own.g.center && own.g.radius)) break;
          const gap = offsetDim.value * MM, near = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]);
          let best = null;
          const consider = (x, it) => { if (!best || Math.abs(x.m[2]) < best.z) best = { it, z: Math.abs(x.m[2]) }; };
          if (own.g.type === "line") {
            const [s0, s1] = [sk.toPeet(own.g.start), sk.toPeet(own.g.end)];
            const d = [s1[0] - s0[0], s1[1] - s0[1]], l = Math.hypot(...d);
            const off = q => (d[0] * (q[1] - s0[1]) - d[1] * (q[0] - s0[0])) / l;
            for (const x of modelEdges()) {
              if (x.curve !== "line" || Math.abs(x.a[2] - x.b[2]) > 1e-3) continue;
              const [u, v] = [off(x.a), off(x.b)];
              if (Math.abs(u - v) > 1e-6 || Math.abs(Math.abs(u) - gap) > 1e-4) continue;
              consider(x, { type: "project", edge: { between: [x.from, x.to] } });
            }
          } else {
            const c = sk.toPeet(own.g.center), r0 = own.g.radius * MM;
            for (const x of modelEdges()) {
              if (x.curve !== "circle" || Math.abs(x.a[2] - x.m[2]) > 1e-3) continue;
              const rr = near(x.m, c);
              if (Math.abs(near(x.a, c) - rr) > 1e-4 || Math.abs(Math.abs(rr - r0) - gap) > 1e-4) continue;
              consider(x, { type: "project", edge: { at: x.middle } });
            }
          }
          const source = best && projectItem(best.it);
          if (!source || source.id === undefined || source.point || source.round !== (own.g.type === "arc")) break;
          // An offset of nothing is the curve itself, converted: PeetCAD's dimensions are never 0
          if (gap < 1e-9) { item = { type: own.g.type === "arc" ? "coradial" : "collinear", of: [own.id, source.id] }; break; }
          const first = !offsetUsed;
          offsetUsed = true;
          item = { type: "offset_distance", of: [own.id, source.id], value: tidy(gap) };
          if (first) { offsetItem = item; item.extra = 1; item.sw = offsetDim.fullName; item.name = offsetDim.name.trim().replace(/["@]/g, "_") || "D"; while (names.has(item.name)) item.name += "_"; names.add(item.name); }
          else { item.value = offsetItem.name; item.name = offsetItem.name + "_"; while (names.has(item.name)) item.name += "_"; names.add(item.name); }
          items.push(item);
          continue;
        }
        case "linearpattcnt": case "circularpattcnt":
          drop(`${r.type}: the count of a sketch pattern, which PeetCAD's sketch has no match for (the copies are drawn, and tied to the seed)`);
          continue;
        // A point where two curves cross: on both
        case "atintersect": {
          const curves = [...segs.map(g => g.id), ...outLines];
          if (anyPoints.length === 1 && curves.length === 2) item = { type: "coincident", of: [anyPoints[0], curves[0]], with: [{ type: "coincident", of: [anyPoints[0], curves[1]] }] };
          break;
        }
        case "diameter": case "radius": if (rounds.length === 1 && dim) item = { type: r.type, of: [rounds[0].id], value: tidy(dim.value * MM) }; break;
        case "angle": {
          if (anyLines.length !== 2 || !dim) break;
          item = { type: "angle", of: anyLines, value: tidy(dim.value * 180 / Math.PI) };
          // SolidWorks' angle is between the lines, on the side its dimension is drawn; PeetCAD's is
          // between the lines' directions, start to end: the other one where the lines run so
          if (lines.length === 2) {
            const way = l => { const d = [l.g.end.x - l.g.start.x, l.g.end.y - l.g.start.y]; return l.turned ? d.map(v => -v) : d; };
            const [a, b] = lines.map(way);
            const now = Math.acos(Math.max(-1, Math.min(1, (a[0] * b[0] + a[1] * b[1]) / Math.hypot(...a) / Math.hypot(...b)))) * 180 / Math.PI;
            if (Math.abs(now - (180 - item.value)) < Math.abs(now - item.value)) { item.value = tidy(180 - item.value); item.supplementary = true; }
          }
          break;
        }
        case "doubledistance": {
          const centre = lines.find(l => l.g.construction), other = lines.find(l => l !== centre);
          const point = anyPoints[0] !== undefined ? anyPoints[0] : other && other.start;
          if (centre && point !== undefined && dim) item = { type: "doubled_distance", of: [point, centre.id], value: tidy(dim.value * MM) };
          break;
        }
        case "distance": {
          if (!dim) break;
          const value = tidy(dim.value * MM);
          // To a circle's near or far side (SolidWorks' arc conditions, Min and Max), where SolidWorks' value is
          // not between the centres
          if (rounds.length) {
            const sided = sideDistance({ pts, seen, rounds, lines, outLines, dim, value });
            if (sided) { item = sided; break; }
          }
          // The two points measured between: the sketch's own, vertices', and circles' centres
          const two = [...pts, ...seen, ...rounds.filter(g => g.center !== undefined && g.g.center).map(g => ({ id: g.center, e: g.g.center }))];
          if (two.length === 2 && anyPoints.length + rounds.length === 2 && anyLines.length === 0) {
            // Between two points SolidWorks measures along x, along y or straight, and the export does not
            // say which: it is the one whose size the points are apart
            const dx = Math.abs(two[0].e.x - two[1].e.x), dy = Math.abs(two[0].e.y - two[1].e.y), v = dim.value;
            // The dimension line's direction is in the model's coordinates: along the sketch's x, its y, or neither
            const line = dim.lineDirection && len(dim.lineDirection) > 1e-9 ? unit(dim.lineDirection.slice(0, 3)) : null;
            const said = !line ? undefined : Math.abs(dot(line, sf.x)) > 0.9999 ? "horizontal" : Math.abs(dot(line, sf.y)) > 0.9999 ? "vertical" : null;
            const kind = said !== undefined ? said
              : dim.displayType === 11 || (Math.abs(v - dx) < 1e-9 && dy > 1e-9) ? "horizontal" : dim.displayType === 12 || (Math.abs(v - dy) < 1e-9 && dx > 1e-9) ? "vertical" : null;
            if (kind && !level(kind)) break;
            item = { type: kind ? level(kind) + "_distance" : "distance", of: two.map(p => p.id), value };
          } else if (rounds.length && rounds.every(g => g.center !== undefined) && anyPoints.length + rounds.length === 2 && lines.length + outLines.length === 0) item = { type: "distance", of: [...anyPoints, ...rounds.map(g => g.center)], value };
          else if (rounds.length === 1 && rounds[0].center !== undefined && anyPoints.length === 0 && anyLines.length === 1) item = { type: "distance", of: [rounds[0].center, anyLines[0]], value };
          else if (anyPoints.length === 2) item = { type: "distance", of: anyPoints, value };
          else if (anyPoints.length === 1 && anyLines.length === 1) item = { type: "distance", of: [anyPoints[0], anyLines[0]], value };
          else if (lines.length === 2) item = { type: "distance", of: [lines[0].start, lines[1].id], value };
          else if (lines.length === 1 && outLines.length === 1) item = { type: "distance", of: [lines[0].start, outLines[0]], value };
          // A line's length: the dimension ties that line alone (not a line and something not found)
          else if (lines.length === 1 && all.length === 1 && (r.entities || []).length === 1) item = { type: "length", of: [lines[0].id], value };
          break;
        }
      }
      if (!item) { drop(`${r.type}${r.defined ? " to the model" : ""}: no match for what it ties (${pts.length} points, ${lines.length} lines, ${rounds.length} arcs of ${(r.entities || []).length}${dim || !r.dimension ? "" : ", its dimension not found"})`); continue; }
      if (dim && item.value !== undefined) {
        // SolidWorks' own name, so that it is NAME@SKETCH as SolidWorks has it ("Dia.@Sketch1")
        let dimName = dim.name.trim().replace(/["@]/g, "_") || "D";
        while (names.has(dimName)) dimName += "_";
        names.add(dimName);
        item.name = dimName;
        // SolidWorks calls a dimension driven both when it is for reference and when an equation sets it.
        // One an equation sets still drives the sketch: its value is carried over, per configuration. So
        // does one an assembly's equation sets, which SolidWorks reports read-only and no equation of the
        // part names (Weather Shield Door's Height@Sketch2, set by Door Assembly; its sketch fully defined).
        if (dim.drivenState === 1 && !dim.readOnly && !byEquation.has(dim.fullName.split("@").slice(0, 2).join("@"))) item.driven = true;
        item.sw = dim.fullName;
      }
      // What the model or a dimension's value sets, which may have changed since SolidWorks last solved the sketch
      if (outside.length || item.value !== undefined) item.moves = true;
      items.push(item);
    }
    current = null;
    if (offsetRelation && !offsetUsed) drop(offsetDim.value === 0 ? "offsetedge: an offset of 0, which PeetCAD's dimensions can't be (its pieces are on what they are offset from)" : "offsetedge: the offset's distance, with none of its pieces found");
    if (items.length === 0) return;

    const entities = () => peet.ask({ op: "feature", feature: name }).entities || [];
    const shapeKey = list => JSON.stringify(list.map(e => [e.id, e.from, e.to, e.center, e.radius, e.at].map(x => Array.isArray(x) ? x.map(v => +v.toFixed(4)) : typeof x === "number" ? +x.toFixed(4) : x)));
    const drawn0 = entities(), before = shapeKey(drawn0);
    // How far anything in the sketch is from where it was drawn
    const movedBy = now => {
      const was = new Map(drawn0.map(e => [e.id, e]));
      let most = 0;
      for (const e of now) {
        const b = was.get(e.id);
        if (!b) continue;
        for (const k of ["from", "to", "at"]) if (Array.isArray(b[k]) && Array.isArray(e[k])) most = Math.max(most, Math.hypot(...b[k].map((v, i) => v - e[k][i])));
        if (typeof b.radius === "number" && typeof e.radius === "number") most = Math.max(most, Math.abs(b.radius - e.radius));
      }
      return most;
    };
    // A relation PeetCAD makes of more than one (`with`) is kept or dropped as one
    const clean = item => { const { sw, supplementary, moves, extra, with: also, ...rest } = item; return [rest, ...(also || [])]; };
    const keep = item => {
      if (!item.also) tally.kept++;
      if (item.extra) tally.kept += item.extra;
      if (item.sw) sk.dimensions.set(item.sw, item.name);
      if (item.sw && item.supplementary) (sk.supplementary || (sk.supplementary = new Set())).add(item.sw);
    };
    const backup = peet.file + ".before";
    const save = () => { fs.copyFileSync(peet.file, backup); return peet.script.length; };
    const restore = mark => { fs.copyFileSync(backup, peet.file); peet.script.length = mark; };
    // Left where it was drawn, to within a micron: PeetCAD's model edges can be that far from SolidWorks'
    // (a roof at an angle), which a relation to one makes up
    const fits = reply => {
      if (!reply.ok || reply.definition === "over_defined") return false;
      const now = entities();
      return shapeKey(now) === before || movedBy(now) <= 1e-3;
    };

    let mark = save();
    const moving = [];
    if (fits(peet.apply({ op: "draw", sketch: name, draw: items.flatMap(clean) }))) items.forEach(keep);
    else {
      restore(mark);
      for (const item of items) {
        mark = save();
        const reply = peet.apply({ op: "draw", sketch: name, draw: clean(item) });
        if (fits(reply)) keep(item);
        else {
          restore(mark);
          if (reply.ok && reply.definition !== "over_defined" && unsolved && item.moves) moving.push(item);
          else {
            if (item.extra) drop("offsetedge: the offset's distance, its first piece dropped");
            if (!item.also) drop(`${item.type}: ${!reply.ok ? "refused" : reply.definition === "over_defined" ? "over-defines the sketch" : "moves the sketch"}`);
          }
        }
      }
    }
    // A sketch SolidWorks has suppressed is in the export as it was last solved, which the model and the
    // sketch's dimensions may have moved on from. SolidWorks solves it again when it is brought back, which
    // moves it to where its relations to the model and its dimensions put it: those are kept, and move it.
    for (const item of moving) {
      mark = save();
      const reply = peet.apply({ op: "draw", sketch: name, draw: clean(item) });
      if (reply.ok && reply.definition !== "over_defined") keep(item);
      else { restore(mark); if (!item.also) drop(`${item.type}: ${!reply.ok ? "refused" : "over-defines the sketch"}`); }
    }
    const by = moving.length ? movedBy(entities()) : 0;
    if (by > 1e-3) report.notes.push(`${name} is suppressed in SolidWorks, whose export has it as it was last solved: its relations to the model and its dimensions move it up to ${by.toFixed(3)} mm, as SolidWorks moves it when it solves it again.`);
    if (fs.existsSync(backup)) fs.unlinkSync(backup);
  }

  /** The sketch a feature is made from: one absorbed under it, or one above it in the tree. */
  function sketchOf(f) {
    const own = (f.sub || []).find(c => SKETCH.test(c.type));
    if (own) return own;
    for (const name of f.parents || []) {
      const p = byName.get(name);
      if (p && SKETCH.test(p.type)) return p;
    }
    return null;
  }
  /** The PeetCAD sketch for a feature, importing it now if it has not been yet. */
  function useSketch(f, options) {
    const source = sketchOf(f);
    if (!source) return { error: "its sketch was not found" };
    if (!options && sketches.has(source.name)) return sketches.get(source.name);
    const imported = importSketch(source, options);
    if (!imported.error && !options) sketches.set(source.name, imported);
    return imported;
  }

  /**
   * Gives the sizes of the features imported so far SolidWorks' names for them, so that equations and FabSys
   * reach them by the addresses they use: `D1@LPattern2` for a size of the feature itself, and the whole
   * `D2@Sketch10` for one SolidWorks keeps in a sketch of the feature's own that PeetCAD's feature does not
   * have (an edge flange's length and angle). A size is named only where PeetCAD's feature has it with the
   * same value, as the same field. A flange on several edges is several PeetCAD flanges: a size in one
   * edge's outline is that piece's, the others are the first's, and with `link` a piece whose length has no
   * name of its own follows the first's where they are the same.
   */
  /**
   * Which set-back of a flange piece (`offset_start` or `offset_end`) a length in its outline sketch `owner` is,
   * from where the dimension is: SolidWorks measures a set-back from an end of the edge to the outline, so the
   * two things it joins are both near that end. Null where that can't be told (the dimension's ends are not
   * points and lines across the edge, it does not measure `want` along the edge, or which end is the PeetCAD
   * flange's start is not known), or where the two are not near one end.
   */
  function setBackField(f, owner, d, want) {
    const at = flangeEnds.get(`${f.name}|${owner}`), s = (f.sub || []).find(x => x.name === owner);
    const rel = at && at.startAtFrom !== null && s && s.sketch && (s.sketch.relations || []).find(r => r.dimension === d.fullName);
    if (!rel || !rel.entities || rel.entities.length !== 2) return null;
    const dir = unit(sub(at.to, at.from)), whole = len(sub(at.to, at.from));
    const along = q => dot(sub(fromSketch(at.sf, q), at.from), dir);
    // A point's place along the edge, or a line's if it runs across the edge
    // Each entity's places along the edge: a point's, a line's if it runs across the edge; an arc's centre and
    // its two sides along the edge (SolidWorks' arc conditions: Center, Min and Max), where the export's arc end
    // conditions (format 6) don't say which
    const conditions = Array.isArray(d.arcEndConditions) ? d.arcEndConditions : [];
    const options = rel.entities.map((e, k) => {
      if (e.kind === "sketchPoint" && typeof e.x === "number") return [along(e)];
      if (e.point && e.point.length === 3) return [dot(sub(mm(e.point), at.from), dir)];
      if (e.type === "arc" && e.center && typeof e.radius === "number") {
        const c = along(e.center), r = e.radius * MM;
        return conditions[k] === 2 || conditions[k] === 3 ? [c - r, c + r] : conditions[k] === 1 ? [c] : [c, c - r, c + r];
      }
      if (e.start && e.end) { const a = along(e.start), b = along(e.end); return Math.abs(a - b) < 1e-6 ? [a] : []; }
      return [];
    });
    const places = options[0].flatMap(p => options[1].map(q => [p, q])).find(([p, q]) => Math.abs(Math.abs(p - q) - want) <= 1e-3);
    if (!places) return null;
    const middle = (places[0] + places[1]) / 2;
    if (Math.abs(middle - whole / 2) < 1e-6) return null;
    return (middle < whole / 2) === at.startAtFrom ? "offset_start" : "offset_end";
  }

  function nameDimensions(link = true) {
    for (const [swName, names] of made) {
      if (dimensionsNamed.has(swName)) continue;
      dimensionsNamed.add(swName);
      const f = byName.get(swName), fields = f && DIMENSION_FIELDS[f.type];
      if (!fields || !names.length) continue;
      const values = (f.definition && f.definition.values) || {};
      // A flange on several edges has an outline sketch for each, in the edges' order, and is a PeetCAD
      // flange for each: a size in an outline is that piece's
      const profiles = (f.sub || []).filter(s => SKETCH.test(s.type) && s.sketch && !s.sketch.error).map(s => s.name);
      const pieceOf = owner => profiles.length === names.length && profiles.includes(owner) ? names[profiles.indexOf(owner)] : names[0];
      const theirs = new Map();
      const dimensionsOf = piece => {
        if (!theirs.has(piece)) theirs.set(piece, peet.ask({ op: "dimensions", feature: piece }).dimensions || []);
        return theirs.get(piece);
      };
      const done = new Set(), lengths = new Map();
      for (const d of f.dimensions || []) {
        const [short, owner] = d.fullName.split("@");
        // A dimension of a sketch PeetCAD has as a sketch of that name is that sketch's. A flange's outline
        // is a sketch named after the flange, whose dimension follows the flange's.
        const sk = sketches.get(owner);
        if (owner !== f.name && ((sk && sk.name === owner) || !(f.sub || []).some(s => s.name === owner))) continue;
        let field = [...new Set(Object.keys(fields).filter(k => typeof values[k] === "number" && Math.abs(values[k] - d.value) < 1e-12).map(k => fields[k]))];
        const piece = pieceOf(owner);
        const numberOf = x => typeof x.value === "object" ? x.value.value : x.value;
        const want = d.valueType === 1 ? d.value * 180 / Math.PI : d.valueType === 2 ? d.value : d.value * MM;
        // A length in a flange's outline that is not its length is where the outline stops short of an end
        // of the edge: the set-back at the end of the edge the dimension measures from, or failing that the
        // set-back the importer read off it, if it is just one of them
        if (!field.length && f.type === "EdgeFlange" && profiles.includes(owner) && d.valueType === 0) {
          const atEnd = setBackField(f, owner, d, want);
          field = atEnd ? [atEnd] : dimensionsOf(piece).filter(x => /^offset_(start|end)$/.test(x.field) && Math.abs(numberOf(x) - want) < 1e-6).map(x => x.field);
        }
        if (field.length !== 1 || done.has(piece + "|" + field[0])) continue;
        const mine = dimensionsOf(piece).find(x => x.field === field[0]);
        const value = mine && numberOf(mine);
        if (typeof value !== "number" || Math.abs(value - want) > 1e-6 * Math.max(1, Math.abs(want))) continue;
        const name = owner === f.name ? short : `${short}@${owner}`;
        if (!peet.apply({ op: "name_dimension", feature: piece, dimension: field[0], name }).ok) continue;
        done.add(piece + "|" + field[0]);
        const address = name.includes("@") ? name : `${name}@${piece}`;
        if (field[0] === "length") lengths.set(piece, { address, value });
        if (link && sk && sk.dimensions && sk.dimensions.has(d.fullName)) peet.apply({ op: "set_dimension", sketch: sk.name, name: sk.dimensions.get(d.fullName), value: `"${address}"`, configurations: "all" });
      }
      const first = lengths.get(names[0]);
      if (!link || f.type !== "EdgeFlange" || !first) continue;
      for (const other of names.slice(1)) {
        const length = (dimensionsOf(other).find(x => x.field === "length") || {}).value;
        if (!lengths.has(other) && typeof length === "number" && Math.abs(length - first.value) < 1e-9) peet.apply({ op: "edit", feature: other, length: `"${first.address}"` });
      }
    }
  }

  /** The middle of a feature's flat faces, by area: in SolidWorks' final body, and in the part as it is now. */
  function middleInSolidWorks(name) {
    let sum = [0, 0, 0], area = 0;
    for (const b of cur.bodies || []) for (const face of b.faces) if (face.feature === name && face.surface === "plane") {
      sum = add(sum, mul(mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3))), face.area));
      area += face.area;
    }
    return area ? mul(sum, 1 / area) : null;
  }
  function middleInPeet(names) {
    // The middle of each face's box, as SolidWorks' are taken: a face's `center` is a point on it, which
    // on a long face can be far from its middle
    const faces = (peet.ask({ op: "faces" }).faces || []).filter(face => face.surface === "plane" && (face.made_by || []).some(m => names.includes(m.feature) && (m.side === "top" || m.side === "bottom")));
    const boxes = new Map();
    if (faces.length) for (const e of peet.ask({ op: "edges" }).edges || []) for (const i of e.faces || []) {
      const key = e.body + "|" + i;
      const b = boxes.get(key) || { lo: [Infinity, Infinity, Infinity], hi: [-Infinity, -Infinity, -Infinity] };
      for (const p of [e.from, e.to, e.middle]) if (p) { b.lo = b.lo.map((v, k) => Math.min(v, p[k])); b.hi = b.hi.map((v, k) => Math.max(v, p[k])); }
      boxes.set(key, b);
    }
    let sum = [0, 0, 0], area = 0;
    for (const face of faces) {
      const b = boxes.get(face.body + "|" + face.index);
      sum = add(sum, mul(b ? mid(b.lo, b.hi) : face.center, face.area_mm2));
      area += face.area_mm2;
    }
    return area ? mul(sum, 1 / area) : null;
  }
  /**
   * Builds a flange, and turns it over if it came out on the other side of the sheet from where SolidWorks
   * has it. Which way a flange bends is not in its definition: it follows from the edge picked.
   */
  function buildFlange(f, op, swName = f.name, where = null, known = false) {
    const theirs = where ? [where] : swName ? facesOf(swName).filter(face => face.surface === "plane" && face.area * 1e6 > 4 * (thickness || 1) ** 2).map(face => mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)))) : [];
    const want = theirs.length ? theirs : null;
    const off = names => { const at = middleInPeet(names); return at ? Math.min(...theirs.map(p => len(sub(p, at)))) : 0; };
    const good = Math.max(1, 0.2 * (op.length || 0), (thickness || 0) * 0.75);
    // The same flange turned over and, if it is set back unevenly, with its set-backs at the other ends:
    // which end of an edge is its start is not something the two programs agree on.
    const uneven = (op.offset_start || 0) !== (op.offset_end || 0) && !known;
    const swapped = { ...op, offset_start: op.offset_end, offset_end: op.offset_start };
    for (const k of ["offset_start", "offset_end"]) if (swapped[k] === undefined) delete swapped[k];
    const variants = [op, { ...op, flip: !op.flip }, ...(uneven ? [swapped, { ...swapped, flip: !op.flip }] : [])];
    let best = null, first = null;
    for (const variant of variants) {
      const built = build(f, variant);
      if (!first) first = built;
      if (!built.ok) continue;
      if (!want) return built;
      const d = off(built.names);
      // Set back unevenly, the two ways round are a little apart: only an exact fit settles it at once
      if (d < (uneven ? 1e-3 : good)) return built;
      if (!best || d < best.d) best = { variant, d };
      peet.apply({ op: "delete", features: built.names });
    }
    if (!best) return first;
    const built = build(f, best.variant);
    if (built.ok && best.d >= good) built.warning = `it is ${best.d.toFixed(1)} mm from where SolidWorks has it`;
    return built;
  }

  /**
   * The corners between the pieces of one SolidWorks flange feature (a flange on several edges), treated as
   * SolidWorks treats them: neither piece reaches the corner, and the bends end in line with their
   * flanges, so the flat pattern has one inside corner there. SolidWorks' gap is between the two pieces'
   * nearest edges, corner to corner; PeetCAD's is how far each stops short of the other's inner face, which
   * for a square corner is that over root 2. A corner with a flange of another feature is left alone: there
   * SolidWorks leaves the earlier flange whole, as PeetCAD's own corner does.
   * `pieces` are the flanges made, each with the ends of its SolidWorks edge. Returns the corner feature's
   * name, if there were corners.
   */
  /**
   * The corners new flanges make with flanges that were there before, treated as SolidWorks leaves them:
   * nothing is run on to close the corner. The earlier flange ends where its own edge does, and the later
   * one stops short of it by the bend radius, which is how much shorter the earlier bend made its edge.
   * (PeetCAD's own corner runs the earlier flange on to the later one's outer face.) Every corner the new
   * flanges touch gets this; `corners` then sets the ones among the pieces of one feature.
   */
  function plainCorners(name, flangeNames, bendRadius, kind = "plain", relief = "flush") {
    const tops = (peet.ask({ op: "faces" }).faces || []).filter(face => face.surface === "plane"
      && (face.made_by || []).length === 1 && flangeNames.includes(face.made_by[0].feature) && face.made_by[0].side === "top");
    if (tops.length === 0) return null;
    const op = { op: "corner", name, faces: tops.map(face => ({ at: face.center.map(tidy), normal: face.normal.map(tidy) })), kind, gap: tidy(Math.max(bendRadius, 0.01)), relief };
    const reply = peet.apply(op);
    if (!reply.ok) return null;
    const made = (reply.created || [])[0];
    if (!made || made.status === "failed") { peet.apply({ op: "delete", feature: name }); return null; }
    cornerOps.set(made.name, op);
    return made.name;
  }

  /**
   * How far the flanges built so far are from SolidWorks': the difference in the area of their flat faces,
   * flange feature by flange feature. Cuts made later take the same from both, so it tells two ways of
   * treating a corner apart even though SolidWorks' body is the finished one.
   */
  function flangeMismatch(features) {
    const ours = peet.ask({ op: "faces" }).faces || [];
    let off = 0;
    for (const [swName, peetNames] of features) {
      const theirs = facesOf(swName).filter(face => face.surface === "plane").reduce((sum, face) => sum + face.area * 1e6, 0);
      if (!theirs) continue;
      const mine = ours.filter(face => face.surface === "plane" && (face.made_by || []).some(m => peetNames.includes(m.feature) && m.side !== "bend")).reduce((sum, face) => sum + face.area_mm2, 0);
      off += Math.abs(mine - theirs);
    }
    return off;
  }

  /**
   * How much of the area of a SolidWorks feature's flat faces lies in planes that PeetCAD's features have
   * faces in, facing the same way: 1 when every one of its planes is there. Unlike an area, it is not upset
   * when later features take a face away in one program and not the other.
   */
  function planesMatched(swName, peetNames) {
    const ours = (peet.ask({ op: "faces" }).faces || []).filter(x => x.surface === "plane" && x.normal && x.center && (x.made_by || []).some(m => peetNames.includes(m.feature)));
    let all = 0, found = 0;
    for (const face of facesOf(swName)) {
      if (!face.plane || !face.normal) continue;
      const n = unit(face.normal), offset = dot(n, mm(face.plane.slice(3, 6)));
      all += face.area;
      if (ours.some(x => dot(n, x.normal) > 0.9999 && Math.abs(dot(n, x.center) - offset) < 1e-3)) found += face.area;
    }
    return all ? found / all : 0;
  }

  /**
   * The corners a new flange feature makes with the flanges (edge and mitre) made before it. A plain corner
   * is what SolidWorks leaves between two flange features in the parts looked at, but not in all of them
   * (flanges at an angle differ), so it is kept only where it brings the flanges nearer SolidWorks' than
   * PeetCAD's own corner does. Failing that, PeetCAD's own corner, without the rectangle its relief cuts
   * from the face and the bends: SolidWorks cuts nothing at a corner that the bends do not share. Returns
   * the corner feature's name, if one was kept.
   */
  function cornersWithOthers(swName, flangeNames, radius) {
    const earlier = [...flanges.keys(), ...miters.keys()].filter(n => n !== swName);
    const sofar = [...earlier.map(n => [n, made.get(n) || []]), [swName, flangeNames]];
    const before = flangeMismatch(sofar);
    const name = swName + " corners with others";
    const plain = plainCorners(name, flangeNames, radius);
    if (plain && flangeMismatch(sofar) < before - 1e-3) return plain;
    if (plain) peet.apply({ op: "delete", feature: plain });
    const butt = plainCorners(name, flangeNames, radius, "butt", "tear");
    if (butt && flangeMismatch(sofar) <= before + 1e-3) return butt;
    if (butt) peet.apply({ op: "delete", feature: butt });
    return null;
  }

  /**
   * How much of the area of a SolidWorks feature's flat faces PeetCAD's features have a face for in the
   * same place: in the same plane, facing the same way, with a point of it inside the SolidWorks face's
   * box. Unlike `planesMatched`, it tells apart a cut's walls in two flanges, which are in the same planes.
   */
  function wallsMatched(swName, peetNames) {
    const ours = (peet.ask({ op: "faces" }).faces || []).filter(x => x.surface === "plane" && x.normal && x.center && (x.made_by || []).some(m => peetNames.includes(m.feature)));
    let all = 0, found = 0;
    for (const face of facesOf(swName)) {
      if (!face.plane || !face.normal || !face.box) continue;
      const n = unit(face.normal), offset = dot(n, mm(face.plane.slice(3, 6)));
      const lo = mm(face.box.slice(0, 3)).map(v => v - 0.5), hi = mm(face.box.slice(3)).map(v => v + 0.5);
      all += face.area;
      if (ours.some(x => dot(n, x.normal) > 0.9999 && Math.abs(dot(n, x.center) - offset) < 1e-3 && x.center.every((v, k) => v >= lo[k] && v <= hi[k]))) found += face.area;
    }
    return all ? found / all : 0;
  }

  function corners(name, gap, pieces, radius = 0) {
    const picks = [];
    const faces = () => peet.ask({ op: "faces" }).faces || [];
    let all = null;
    for (let i = 0; i < pieces.length; i++) for (let j = i + 1; j < pieces.length; j++) {
      let shared = [pieces[i].start, pieces[i].end].find(p => [pieces[j].start, pieces[j].end].some(q => len(sub(p, q)) < 1e-3));
      // Edges of two faces that a bend joins, in line across it (the lips round the side of a channel):
      // their ends are the bend apart, and the flanges are mitred with the whole gap between them
      let across = false;
      if (!shared) {
        const [di, dj] = [pieces[i], pieces[j]].map(p => unit(sub(p.end, p.start)));
        const near = ((thickness || 0) + radius) * 2;
        for (const p of [pieces[i].start, pieces[i].end]) for (const q of [pieces[j].start, pieces[j].end]) {
          const gapAt = sub(q, p);
          if (shared || len(gapAt) > near || Math.abs(dot(di, dj)) > 1e-6) continue;
          // Square to both edges, so the two run on in line round the bend
          if (Math.abs(dot(gapAt, di)) > 1e-3 * near + 1e-6 && Math.abs(dot(gapAt, dj)) > 1e-3 * near + 1e-6) { shared = mid(p, q); across = true; }
        }
        if (!shared) continue;
      }
      all = all || faces();
      // The end face of one of the two pieces at this corner picks the corner: a face that PeetCAD names
      // after that piece and that is square to the piece's own edge. (Where one piece runs past the
      // other its end is flush with the other's outer face and is not a face of its own, and the end
      // of the piece that stops short may carry its neighbour's name: so both are looked for, and the
      // larger taken.)
      const ends = [];
      for (const piece of [pieces[i], pieces[j]]) {
        const along = unit(sub(piece.end, piece.start));
        ends.push(...all.filter(face => face.surface === "plane" && Math.abs(dot(face.normal, along)) > (across ? 0.5 : 0.999)
          && (face.made_by || []).some(m => m.feature === piece.name && m.side === "wall")
          && Math.abs(dot(sub(face.center, shared), along)) < 0.25 * len(sub(piece.end, piece.start))));
      }
      ends.sort((a, b) => b.area_mm2 - a.area_mm2);
      // SolidWorks' gap is between the two pieces' inner faces' ends, which each stop short of the other's
      // inner face by the gap times the cosine of half the face's angle at the corner
      const away = piece => { const far = len(sub(piece.start, shared)) < 1e-3 ? piece.end : piece.start; return unit(sub(far, shared)); };
      const angle = Math.acos(Math.max(-1, Math.min(1, dot(away(pieces[i]), away(pieces[j])))));
      const g = across ? Math.max(gap, 0) : Math.max(gap, 0) * Math.cos(angle / 2);
      if (ends[0]) picks.push({ face: { at: ends[0].center.map(tidy), normal: ends[0].normal.map(tidy) }, gap: tidy(g) });
    }
    // One corner feature for each gap
    const byGap = new Map();
    for (const p of picks) { if (!byGap.has(p.gap)) byGap.set(p.gap, []); byGap.get(p.gap).push(p.face); }
    const names = [];
    for (const [g, faces] of byGap) {
      const one = name + (names.length ? " " + (names.length + 1) : "");
      const op = { op: "corner", name: one, faces, kind: "open", gap: g, relief: "slanted" };
      const reply = peet.apply(op);
      if (!reply.ok) continue;
      const made = (reply.created || [])[0];
      if (!made || made.status === "failed") { peet.apply({ op: "delete", feature: one }); continue; }
      cornerOps.set(made.name, op);
      names.push(made.name);
    }
    return names;
  }

  /** The straight edge of the part nearest a SolidWorks edge: parallel to it and within reach. */
  function findEdge(s, t) {
    const m = mid(s, t), along = unit(sub(t, s)), whole = len(sub(t, s));
    let best = null;
    for (const edge of peet.ask({ op: "edges" }).edges || []) {
      if (edge.curve !== "line") continue;
      const d = sub(edge.to, edge.from);
      if (len(d) < 1e-9 || Math.abs(dot(unit(d), along)) < 0.9999) continue;
      // How far the SolidWorks edge's middle is from this edge, if it is alongside it
      const q = sub(m, edge.from), u = unit(d), p = dot(q, u);
      if (p < -NEAR || p > len(d) + NEAR) continue;
      const away = len(sub(q, mul(u, p)));
      const score = away + Math.abs(len(d) - whole) * 1e-3;
      if (away <= NEAR && (!best || score < best.score)) best = { score, away, from: edge.from, to: edge.to, middle: edge.middle, body: edge.body, faces: edge.faces };
    }
    return best;
  }

  /**
   * Whether an edge of the sheet starts at its `from`, as a flange on it counts its start: the end from which
   * the sheet is on the left, looking down on the sheet's top face. Null if it can't be told.
   */
  function startsAtFrom(edge) {
    if (!edge.faces || edge.faces.length !== 2) return null;
    const faces = (peet.ask({ op: "faces" }).faces || []).filter(x => x.body === edge.body && edge.faces.includes(x.index));
    const side = m => face => (face.made_by || []).some(x => x.side === m);
    const flat = faces.find(face => side("top")(face) || side("bottom")(face)), wall = faces.find(face => face !== flat);
    if (!flat || !wall || !flat.normal || !wall.normal) return null;
    const up = side("top")(flat) ? flat.normal : mul(flat.normal, -1);
    const along = dot(sub(edge.to, edge.from), cross(mul(wall.normal, -1), up));
    return Math.abs(along) > 1e-9 ? along > 0 : null;
  }

  /** The faces a feature has in SolidWorks' final body. */
  const facesOf = name => (cur.bodies || []).flatMap(b => b.faces).filter(face => face.feature === name);

  /**
   * A sketch on the flat face of the part through a point, looking along a direction, for geometry that comes
   * from SolidWorks' faces and not from one of its sketches. `draw` gets a function from model points to
   * the sketch's own.
   */
  function sketchOnFace(name, point, normal, draw) {
    for (const n of [normal, mul(normal, -1)]) {
      const on = { at: point.map(tidy), normal: n.map(tidy) };
      const pf = probe(on);
      if (!pf || Math.abs(dot(pf.n, n)) < 0.9999) continue;
      const flat = p => { const d = sub(p, pf.o); return [tidy(dot(d, pf.x)), tidy(dot(d, pf.y))]; };
      const reply = peet.apply({ op: "sketch", on, name, draw: draw(flat) });
      return reply.ok ? { name: reply.created[0].name } : { error: reply.error };
    }
    return { error: "no flat face of the part is there" };
  }

  /** A round face of SolidWorks' body: its axis, the point of the axis level with its middle, and its size. */
  function roundFace(face) {
    const axis = unit(face.cylinder.slice(3, 6)), centre = mm(face.cylinder.slice(0, 3)), radius = face.cylinder[6] * MM;
    const lo = mm(face.box.slice(0, 3)), hi = mm(face.box.slice(3)), boxMiddle = mid(lo, hi);
    const height = Math.abs(dot(sub(hi, lo), axis));
    const level = add(centre, mul(axis, dot(sub(boxMiddle, centre), axis)));
    return { axis, radius, height, level, top: add(level, mul(axis, height / 2)), towards: sub(boxMiddle, level), sweep: height > 1e-6 ? Math.min(Math.PI, face.area * 1e6 / (radius * height)) : 0 };
  }

  /**
   * A slot from the hole wizard, which PeetCAD's hole does not make: a sheet metal cut of a slot shape, read
   * off the two round ends the slot has in SolidWorks' body.
   */
  function slots(f) {
    if (!sheetBody()) return record(f, "not supported", "a slot from the hole wizard, in a part that is not sheet metal");
    // From the hole wizard's own sketch: a line from tip to tip for each slot, the slot's length long, drawn
    // as a slot of the hole's diameter whose ends' centres are a radius in from the tips. Failing that, the
    // slots are read off their round ends in SolidWorks' body.
    const v = values(f);
    const diameter = (v.HoleDiameter || v.ThruHoleDiameter || v.Diameter || 0) * MM, length = (v.Length || 0) * MM;
    const where = (f.sub || []).find(c => SKETCH.test(c.type) && c.sketch && !c.sketch.error && c.sketch.segments.some(g => g.type === "line"));
    let centres = where ? where.sketch.segments.filter(g => g.type === "line" && g.start && g.end && Math.abs(Math.hypot(g.end.x - g.start.x, g.end.y - g.start.y) * MM - length) < 1e-3) : [];
    // A slot goes at each of the feature's points, turned as the line drawn through one of them is:
    // SolidWorks draws that line through the first point only (Gland Plate Frame's slots)
    const points = (Array.isArray(v.GetSketchPoints) ? v.GetSketchPoints : []).filter(p => p && where && p.sketch === where.name && typeof p.x === "number" && typeof p.y === "number");
    const middleOf = g => ({ x: (g.start.x + g.end.x) / 2, y: (g.start.y + g.end.y) / 2 });
    const through = p => g => Math.hypot(middleOf(g).x - p.x, middleOf(g).y - p.y) * MM < 1e-3;
    const drawn = centres.filter(g => points.some(p => through(p)(g)));
    if (drawn.length) {
      const hx = (drawn[0].end.x - drawn[0].start.x) / 2, hy = (drawn[0].end.y - drawn[0].start.y) / 2;
      centres = points.map(p => drawn.find(through(p)) || { type: "line", start: { x: p.x - hx, y: p.y - hy }, end: { x: p.x + hx, y: p.y + hy } });
    }
    // The slots as the hole wizard's sketch has them: `keep` keeps them whatever they come to
    const fromSketch = keep => {
      if (!(diameter > 0 && length > 0 && centres.length)) return false;
      const sk = importSketch(where, { name: f.name + " sketch", needFace: true, draw: toPeet => centres.map(g => {
        const k = diameter / 2 / length;
        const at = t => ({ x: g.start.x + (g.end.x - g.start.x) * t, y: g.start.y + (g.end.y - g.start.y) * t });
        return { type: "slot", from: toPeet(at(k)), to: toPeet(at(1 - k)), radius: tidy(diameter / 2) };
      }) });
      if (sk.error) return false;
      const built = sheetCut(f, sk.name, reachOf(v.EndCondition, (v.HoleDepth || v.ThruHoleDepth || 0) * MM, false), sk.frame);
      // Kept only if its faces come to what SolidWorks' slots' do: a feature can have slots on several
      // faces, which its one sketch does not show
      const theirs = facesOf(f.name).reduce((sum, face) => sum + face.area * 1e6, 0);
      const ours = built.ok ? (peet.ask({ op: "faces" }).faces || []).filter(x => (x.made_by || []).some(m => built.names.includes(m.feature))).reduce((sum, x) => sum + x.area_mm2, 0) : 0;
      if (built.ok && (keep || Math.abs(ours - theirs) <= 0.01 * theirs + 0.5)) { finish(f, built); return true; }
      if (built.ok) peet.apply({ op: "delete", features: built.names });
      peet.apply({ op: "delete", feature: sk.name });
      return false;
    };
    if (fromSketch(false)) return;
    const ends = facesOf(f.name).filter(face => face.cylinder).map(roundFace).filter(e => e.height > 1e-6);
    const pairs = [];
    while (ends.length >= 2) {
      const a = ends.shift();
      let k = 0;
      ends.forEach((e, i) => { if (len(sub(e.level, a.level)) < len(sub(ends[k].level, a.level))) k = i; });
      pairs.push([a, ends.splice(k, 1)[0]]);
    }
    if (pairs.length === 0) return fromSketch(true) || record(f, "not supported", "a slot from the hole wizard, whose ends are not in SolidWorks' body to copy");
    // One slot can leave its two ends twice over (a face each side of a cut through it): once is enough
    const seen = new Set(), groups = new Map();
    for (const [a, b] of pairs) {
      const key = [a.level, b.level].map(p => p.map(v => v.toFixed(3)).join(",")).sort().join(" ");
      if (seen.has(key)) continue;
      seen.add(key);
      // The slots of one feature can be on several faces (a hole wizard feature takes points on any face)
      const face = a.axis.map(v => Math.abs(v).toFixed(4)).join(",") + "|" + Math.abs(dot(a.top, a.axis)).toFixed(3);
      if (!groups.has(face)) groups.set(face, []);
      groups.get(face).push([a, b]);
    }
    const names = [];
    let problem = null, n = 0;
    for (const group of groups.values()) {
      const name = f.name + (n++ ? " " + n : "");
      const sk = sketchOnFace(name + " sketch", group[0][0].top, group[0][0].axis, flat => group.map(([a, b]) => ({ type: "slot", from: flat(a.level), to: flat(b.level), radius: tidy(a.radius) })));
      if (sk.error) { problem = sk.error; continue; }
      const built = build(f, { op: "sheet_cut", sketch: sk.name, name });
      if (!built.ok) { problem = built.message; peet.apply({ op: "delete", feature: sk.name }); continue; }
      names.push(...built.names);
    }
    if (names.length === 0) return fromSketch(true) || record(f, "failed", problem || "its slots could not be read");
    made.set(f.name, names);
    return record(f, "imported with a warning", `as a sheet metal cut of the slots' shape, which does not follow the part${problem ? "; some slots were left: " + problem : ""}`);
  }

  /**
   * A break corner, or a fillet on a sheet's corner. PeetCAD has no such feature and its fillet turns sheet
   * metal into a plain solid, so the corner is cut off with a sheet metal cut shaped like what SolidWorks
   * removed, read off the round faces the feature left. The shape is right; it does not follow the part.
   */
  /**
   * A break corner as PeetCAD's own feature. The corners are found from the faces the feature left in
   * SolidWorks' body: a round's corner is the edge through the sheet nearest it beyond its radius from its
   * axis, in front of it (a round that a hole cuts in two is one, about one axis), and a chamfer's face is in
   * front of its corner. Kept if PeetCAD's rounds come to the area SolidWorks' have. False if a corner's
   * edge is not in the part or the corner can't be broken so.
   */
  /**
   * A break corner on faces, as SolidWorks' is when its entities are faces: every corner of each, found
   * again when the part changes (Weather Shield Door's, whose corners are where a cut is in one
   * configuration and where another cut is in the other). Kept only if what it rounds or cuts comes to
   * what SolidWorks' does.
   */
  function cornersOfFaces(f) {
    const v = values(f);
    if (f.type !== "BreakCorner" || !sheetBody() || !(v.Distance > 0)) return false;
    const entities = v.Entities || [];
    if (!entities.length || !entities.every(e => e && e.kind === "face" && e.surface === "plane" && e.point && e.normal)) return false;
    const theirs = facesOf(f.name);
    if (!theirs.length) return false;
    const kind = theirs.some(x => x.cylinder) ? "fillet" : "chamfer";
    // SolidWorks' selection is of pieces of faces: where a cut splits a face in another configuration, the
    // same entity there is one of its pieces, and here (where the face is whole) a point on that piece
    // finds the same face, and in that configuration finds the piece. Two entities on a face that is whole
    // here are each a piece there (Weather Shield Door's flange, cut in two in HingeRight only).
    const pieces = doc.configurations.filter(c => c !== config && !c.flatPattern && c.features)
      .map(c => (values(flatten(c).get(f.name) || {}).Entities || []));
    const within = (e, o) => o && o.kind === "face" && o.surface === "plane" && o.point && o.normal && e.box && o.area < e.area - 1e-12
      && dot(unit(o.normal), unit(e.normal)) > 1 - 1e-9 && Math.abs(dot(sub(o.point, e.point), unit(e.normal))) < 1e-5
      && o.point.every((x, k) => x > e.box[k] - 1e-5 && x < e.box[k + 3] + 1e-5);
    const faces = entities.map((e, i) => {
      const piece = pieces.map(list => list[i]).filter(o => within(e, o)).sort((a, b) => a.area - b.area)[0];
      return { at: mm((piece || e).point).map(tidy), normal: unit(e.normal).map(tidy) };
    });
    const mark = peet.script.length;
    const built = build(f, { op: "break_corner", name: f.name, kind, size: tidy(v.Distance * MM), edges: [], faces });
    if (!built.ok) { peet.script.length = mark; return false; }
    const area = theirs.reduce((t, x) => t + x.area * 1e6, 0);
    const ours = (peet.ask({ op: "faces" }).faces || []).filter(x => (x.made_by || []).some(m => built.names.includes(m.feature))).reduce((t, x) => t + x.area_mm2, 0);
    if (Math.abs(ours - area) > 1e-3 * area + 0.01) { peet.apply({ op: "delete", features: built.names }); peet.script.length = mark; return false; }
    made.set(f.name, built.names);
    record(f, "imported");
    return true;
  }

  function breakCorners(f) {
    if (!sheetBody() || !thickness) return false;
    const edges = (peet.ask({ op: "edges" }).edges || []).filter(e => e.curve === "line" && e.from && e.to && Math.abs(len(sub(e.to, e.from)) - thickness) < 1e-3);
    const groups = new Map();
    const take = (kind, size, fits, near) => {
      let best = null;
      for (const e of edges) {
        if (!fits(e)) continue;
        const far = len(sub(e.middle, near));
        if (!best || far < best.far) best = { e, far };
      }
      if (!best) return false;
      const key = kind + "|" + size.toFixed(3);
      if (!groups.has(key)) groups.set(key, { kind, size, edges: [] });
      if (!groups.get(key).edges.includes(best.e)) groups.get(key).edges.push(best.e);
      return true;
    };
    const faces = facesOf(f.name);
    if (faces.length === 0) return false;
    // The rounds, each about one axis with one radius
    const rounds = [];
    for (const face of faces) {
      const middle = mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)));
      if (face.cylinder) {
        const axis = unit(face.cylinder.slice(3, 6)), on = mm(face.cylinder.slice(0, 3)), radius = face.cylinder[6] * MM;
        const offAxis = q => { const v = sub(q, on); return len(sub(v, mul(axis, dot(v, axis)))); };
        const same = rounds.find(r => Math.abs(r.radius - radius) < 1e-6 && Math.abs(Math.abs(dot(r.axis, axis)) - 1) < 1e-9 && offAxis(r.on) < 1e-6 && Math.abs(dot(sub(middle, r.middles[0][0]), axis)) < 1e-3);
        if (same) { same.middles.push([middle, face.area]); same.area += face.area; continue; }
        if (offAxis(middle) < 1e-9) return false;
        rounds.push({ axis, on, radius, middles: [[middle, face.area]], area: face.area });
      } else if (face.plane && face.normal) {
        // A chamfer's face is the sheet's thickness high, across the corner: the corner's edge is in front
        // of its middle (half its length in front on a square corner). The distance along each side is
        // from the corner to an end of the face.
        const normal = unit(face.normal), length = face.area * 1e6 / thickness;
        const ahead = e => { const v = sub(e.middle, middle); return { s: Math.abs(dot(v, normal)), off: len(sub(v, mul(normal, dot(v, normal)))) }; };
        const fits = e => { const { s, off } = ahead(e); return Math.abs(dot(unit(sub(e.to, e.from)), normal)) < 1e-4 && off < 0.02 && s > 1e-3 && s < length; };
        let best = null;
        for (const e of edges) if (fits(e) && (!best || ahead(e).s < ahead(best).s)) best = e;
        if (!best) return false;
        const size = Math.hypot(length / 2, ahead(best).s);
        if (!take("chamfer", size, e => e === best, middle)) return false;
      } else return false;
    }
    for (const r of rounds) {
      const total = r.middles.reduce((t, [, a]) => t + a, 0);
      const middle = r.middles.reduce((t, [q, a]) => add(t, mul(q, a / total)), [0, 0, 0]);
      const radial = q => { const v = sub(q, r.on); return sub(v, mul(r.axis, dot(v, r.axis))); };
      const towards = unit(radial(middle));
      // The corner is beyond the radius from the axis, on the round's side of it: on the line that halves
      // the corner between two straight sides, near it where a side is an arc
      const fits = e => {
        if (Math.abs(dot(unit(sub(e.to, e.from)), r.axis)) < 0.9999) return false;
        const v = radial(e.middle), d = len(v);
        return d > r.radius + 1e-3 && d < 4 * r.radius && dot(v, towards) > 0.7 * d;
      };
      if (!take("fillet", r.radius, fits, middle)) return false;
    }
    const names = [];
    let n = 0;
    for (const g of groups.values()) {
      const built = build(f, { op: "break_corner", name: f.name + (n++ ? " " + n : ""), kind: g.kind, size: tidy(g.size), edges: g.edges.map(e => ({ between: [e.from, e.to] })) });
      if (!built.ok) { if (names.length) peet.apply({ op: "delete", features: names }); return false; }
      names.push(...built.names);
    }
    // The rounds must come to as much as SolidWorks' (a later feature can take some of them): a corner
    // not found leaves less
    if (rounds.length) {
      const theirs = rounds.reduce((t, r) => t + r.area * 1e6, 0);
      const ours = (peet.ask({ op: "faces" }).faces || []).filter(x => x.surface === "cylinder" && (x.made_by || []).some(m => names.includes(m.feature))).reduce((t, x) => t + x.area_mm2, 0);
      if (ours < theirs * (1 - 1e-3) - 0.01) { peet.apply({ op: "delete", features: names }); return false; }
    }
    made.set(f.name, names);
    record(f, "imported");
    return true;
  }

  /**
   * Hole wizard holes placed on a 3D sketch, whose points are in the model's coordinates and may be on
   * several faces. Each is drilled into the flat face of SolidWorks' body it lies on, square to it, from a
   * sketch on that face with the points in it: one sketch and one hole feature for each face.
   */
  function holesOn3D(f, v, points, diameter) {
    const sf = frameOf(f.sub.find(c => c.sketch && c.sketch.is3D).sketch.modelToSketch);
    const groups = new Map();
    for (const p of points) {
      const at = fromSketch(sf, p);
      // The flat face it is on: in the face's plane and within its box
      const face = (cur.bodies || []).flatMap(b => b.faces).find(x => x.plane && x.normal && x.box && x.feature !== f.name
        && Math.abs(dot(unit(x.normal), sub(at, mm(x.plane.slice(3, 6))))) < 1e-3
        && mm(x.box.slice(0, 3)).every((lo, k) => at[k] >= lo - 1e-3) && mm(x.box.slice(3)).every((hi, k) => at[k] <= hi + 1e-3));
      if (!face) return record(f, "failed", "a point of its 3D sketch is on no flat face of SolidWorks' body");
      const n = unit(face.normal), key = n.map(x => x.toFixed(6)).join(",") + "|" + dot(n, at).toFixed(4);
      if (!groups.has(key)) groups.set(key, { n, points: [] });
      groups.get(key).points.push(at);
    }
    const sheet = sheetBody();
    const names = [];
    let i = 0;
    for (const g of groups.values()) {
      const name = f.name + (i++ ? ` ${i}` : "");
      const sk = sketchOnFace(name + " sketch", g.points[0], g.n, flat => g.points.map(q => sheet
        ? { type: "circle", center: flat(q), radius: tidy(diameter / 2) } : { type: "point", at: flat(q) }));
      if (sk.error) return record(f, names.length ? "imported in part" : "failed", sk.error);
      let built;
      if (sheet) built = build(f, { op: "sheet_cut", sketch: sk.name, name });
      else for (const reverse of [false, true]) {
        built = build(f, { op: "hole", sketch: sk.name, name, ...holeShape(f, v, diameter), reverse });
        if (built.ok) break;
      }
      if (!built.ok) { peet.apply({ op: "delete", feature: sk.name }); return record(f, names.length ? "imported in part" : "failed", built.message); }
      names.push(sk.name, ...built.names);
    }
    made.set(f.name, names);
    record(f, "imported", "from its 3D sketch: each hole drilled square into the face its point is on");
  }

  /**
   * A hole wizard hole's shape as PeetCAD's hole has it: its end, the drill's point and a countersink at its
   * mouth. A blind hole whose bottom is a flat face of the hole's size in SolidWorks' body is flat-bottomed
   * (a dowel hole), whatever drill angle its definition gives.
   */
  function holeShape(f, v, diameter) {
    const blind = v.EndCondition === 0 && (v.HoleDepth || v.ThruHoleDepth);
    const shape = { diameter: tidy(diameter), ...(blind ? { end: "blind", depth: tidy(blind * MM) } : { end: "through_all" }) };
    const bottom = Math.PI * diameter * diameter / 4;
    const flat = facesOf(f.name).some(face => face.surface === "plane" && Math.abs(face.area * 1e6 - bottom) < 1e-3 * bottom);
    if (blind && (flat || v.DrillAngle > 0)) shape.tip_angle = flat ? 180 : tidy(v.DrillAngle * 180 / Math.PI);
    if (v.NearSideCounterSink && v.NearCounterSinkDiameter * MM > diameter + 1e-6 && v.NearCounterSinkAngle > 0)
      Object.assign(shape, { kind: "countersink", countersink_diameter: tidy(v.NearCounterSinkDiameter * MM), countersink_angle: tidy(v.NearCounterSinkAngle * 180 / Math.PI) });
    return shape;
  }

  /**
   * SolidWorks' Corner Relief: the corners of the flanges it is on get a circular or square relief. The
   * API gives no definition of one, so its shape is read off the faces it left in SolidWorks' body: round
   * faces are a circle's (their radius is its), flat ones a square's, whose size is found by making it once
   * and moving its walls to where SolidWorks' are. It becomes a `corner` on the same corners as the ones the
   * flanges were given, with the same kind and gap and its own relief.
   */
  function cornerRelief(f) {
    const faces = facesOf(f.name);
    if (!faces.length) return record(f, "not supported", "none of its faces is left in SolidWorks' body to read it from");
    const exists = n => peet.ask({ op: "feature", feature: n }).ok;
    const corners = [...cornerOps.entries()].filter(([n]) => (f.parents || []).some(p => n.startsWith(p + " corners")) && exists(n));
    if (!corners.length) return record(f, "failed", "no corner of the flanges it is on was made in the part");
    const rounds = faces.filter(face => face.cylinder);
    const relief = rounds.length ? "circular" : "square";
    const names = [];
    const make = size => {
      if (names.length) peet.apply({ op: "delete", features: names.splice(0) });
      for (const [i, [, op]] of corners.entries()) {
        const built = build(f, { ...op, name: f.name + (i ? ` ${i + 1}` : ""), relief, relief_size: tidy(size) });
        if (!built.ok) return built;
        names.push(...built.names);
      }
      return { ok: true };
    };
    let size;
    if (rounds.length) {
      // The commonest radius: a circle's faces in the face, the bends and the flanges are all of it
      const radii = rounds.map(face => tidy(face.cylinder[6] * MM)).sort((a, b) => a - b);
      size = 2 * radii[Math.floor(radii.length / 2)];
      const built = make(size);
      if (!built.ok) return record(f, "failed", built.message);
    } else {
      // A square's walls are flat and size / 2 from its centre: made at a first guess, each wall is out by
      // half the difference
      const walls = faces.filter(face => face.plane && face.normal && face.box);
      if (!walls.length) return record(f, "not supported", "a corner relief with faces of no shape it has");
      size = 2 * Math.max(...walls.map(face => Math.max(...sub(mm(face.box.slice(3)), mm(face.box.slice(0, 3))))));
      const key = x => x.center.map(v => v.toFixed(4)).join(",");
      const before = new Set((peet.ask({ op: "faces" }).faces || []).map(key));
      for (let round = 0; round < 3; round++) {
        const built = make(size);
        if (!built.ok) return record(f, "failed", built.message);
        const fresh = (peet.ask({ op: "faces" }).faces || []).filter(x => x.surface === "plane" && x.normal && !before.has(key(x)));
        const deltas = [];
        for (const face of walls) {
          const n = unit(face.normal), at = mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)));
          let best = null;
          for (const x of fresh) if (dot(n, x.normal) > 0.9999 && (!best || len(sub(x.center, at)) < len(sub(best.center, at)))) best = x;
          if (best && len(sub(best.center, at)) < size + 5) deltas.push(dot(n, sub(best.center, mm(face.plane.slice(3, 6)))));
        }
        if (!deltas.length) return record(f, "failed", "its walls did not come out where SolidWorks has them");
        deltas.sort((a, b) => a - b);
        const delta = deltas[Math.floor(deltas.length / 2)];
        if (Math.abs(delta) < 1e-4) break;
        size += 2 * delta;
        if (size <= 0) return record(f, "failed", "its walls did not come out where SolidWorks has them");
      }
    }
    made.set(f.name, names);
    record(f, "imported", `as a ${relief} relief ${tidy(size)} mm across, read off its faces in SolidWorks' body (the export has no definition of a corner relief)`);
  }

  /**
   * A library feature: what is in it comes just before it in the tree, as features of their own, and is
   * imported as they are. Its members are the export's own list of them (`subFeatures`), or failing that
   * the features just above it that it is made from, up to the sketch they start with.
   */
  function libraryFeature(f) {
    let members = f.subFeatures;
    if (!members) {
      const parents = new Set(f.parents || []), list = cur.features, at = list.findIndex(x => x.name === f.name);
      members = [];
      for (let i = at - 1; i >= 0 && parents.has(list[i].name); i--) {
        members.unshift(list[i].name);
        if (SKETCH.test(list[i].type)) break;
      }
    }
    if (!members.length) return record(f, "failed", "what is in it was not found");
    // A sketch that is only drawn as part of the feature that uses it has no row of its own
    const rowOf = n => [...report.features].reverse().find(x => x.name === n);
    const left = members.filter(n => rowOf(n) ? !/^imported/.test(rowOf(n).result) : !SKETCH.test((byName.get(n) || {}).type || ""));
    if (left.length) return record(f, "failed", `what is in it was not all imported: ${left.join(", ")}`);
    made.set(f.name, members.flatMap(n => made.get(n) || []));
    record(f, "imported", `as what is in it, imported where it is in the tree: ${members.join(", ")}`);
  }

  function roundCorners(f) {
    if (cornersOfFaces(f) || breakCorners(f)) return;
    const rounds = facesOf(f.name).filter(face => face.cylinder);
    const flats = facesOf(f.name).filter(face => face.plane && face.normal);
    if (rounds.length + flats.length === 0) return record(f, "not supported", "none of its faces is left in SolidWorks' body to copy");
    const groups = new Map();
    // A chamfered corner leaves a flat face across the corner, the sheet's thickness high. The corner that
    // was cut off is the corner of that face's box on its outer side.
    for (const face of flats) {
      const normal = unit(face.normal), lo = mm(face.box.slice(0, 3)), hi = mm(face.box.slice(3)), middle = mid(lo, hi), size = sub(hi, lo);
      const up = ["x", "y", "z"].find(k => Math.abs(dot(normal, AXES[k])) < 1e-6 && Math.abs(dot(size, AXES[k]) - (thickness || 0)) < 1e-3);
      if (!up) continue;
      const axis = AXES[up], height = dot(size, axis), length = face.area * 1e6 / height;
      const along = unit(cross(axis, normal));
      const level = sub(middle, mul(axis, dot(sub(middle, lo), axis) - height / 2));
      let corner = level;
      for (const k of ["x", "y", "z"]) if (k !== up) corner = add(corner, mul(AXES[k], Math.sign(dot(normal, AXES[k])) * dot(size, AXES[k]) / 2));
      const key = axis.map(a => Math.abs(a).toFixed(4)).join(",") + "|" + Math.abs(dot(hi, axis)).toFixed(3);
      if (!groups.has(key)) groups.set(key, { axis, top: add(sub(level, mul(normal, 0.5)), mul(axis, height / 2)), corners: [] });
      groups.get(key).corners.push({ chamfer: [add(level, mul(along, length / 2)), corner, sub(level, mul(along, length / 2))] });
    }
    for (const face of rounds) {
      const { axis, radius, height, level, top, towards, sweep } = roundFace(face);
      if (height < 1e-6 || len(towards) < 1e-9) continue;
      const key = axis.map(a => Math.abs(a).toFixed(4)).join(",") + "|" + Math.abs(dot(top, axis)).toFixed(3);
      if (!groups.has(key)) groups.set(key, { axis, top, corners: [] });
      groups.get(key).corners.push({ level, radius, sweep, towards: unit(towards) });
    }
    const names = [];
    let problem = null, i = 0;
    for (const group of groups.values()) {
      const name = f.name + (i++ ? " " + (i) : "");
      const sk = sketchOnFace(name + " sketch", group.top, group.axis, flat => {
        const items = [];
        group.corners.forEach((c, k) => {
          if (c.chamfer) { items.push({ type: "polyline", points: c.chamfer.map(flat), closed: true }); return; }
          // In the sketch: the arc's centre, its two ends and the corner it rounds
          const o = flat(c.level), ahead = flat(add(c.level, c.towards));
          const angle = Math.atan2(ahead[1] - o[1], ahead[0] - o[0]);
          const at = (a, r) => [tidy(o[0] + r * Math.cos(a)), tidy(o[1] + r * Math.sin(a))];
          const from = at(angle - c.sweep / 2, c.radius), to = at(angle + c.sweep / 2, c.radius), corner = at(angle, c.radius / Math.cos(c.sweep / 2));
          items.push({ type: "polyline", points: [to, corner, from], as: "c" + k }, { type: "arc", center: o, start: from, end: to, as: "r" + k },
            { type: "coincident", of: [`c${k}.0.start`, `r${k}.end`] }, { type: "coincident", of: [`c${k}.1.end`, `r${k}.start`] });
        });
        return items;
      });
      if (sk.error) { problem = sk.error; continue; }
      const built = build(f, { op: "sheet_cut", sketch: sk.name, name });
      if (!built.ok) { problem = built.message; peet.apply({ op: "delete", feature: sk.name }); continue; }
      names.push(...built.names);
    }
    if (names.length === 0) return record(f, "failed", problem || "its corners could not be read");
    made.set(f.name, names);
    record(f, "imported with a warning", `as a sheet metal cut of the corners' shape, which does not follow the part${problem ? "; some corners were left: " + problem : ""}`);
  }

  /**
   * Cuts a rectangular flange down to the outline SolidWorks has for it. What is cut is everything between
   * the outline and a box a little larger than it, from the outline's foot (its line on the edge) outwards.
   */
  function cutToOutline(f, name, { sketch, foot, above = 0 }, map = null) {
    const segments = sketch.sketch.segments.filter(g => !g.construction && g !== foot);
    if (segments.some(g => !(g.type === "line" || (g.type === "arc" && !g.circle)) || !g.start || !g.end)) return { error: "its outline has more than lines and arcs" };
    // The outline in order, from one end of the foot round to the other
    const near = (a, b) => Math.hypot(a.x - b.x, a.y - b.y) < 1e-7;
    const chain = [], left = [...segments];
    let at = foot.start;
    while (left.length) {
      const k = left.findIndex(g => near(g.start, at) || near(g.end, at));
      if (k < 0) return { error: "its outline is not one closed loop" };
      const g = left.splice(k, 1)[0];
      at = near(g.start, at) ? g.end : g.start;
      chain.push(g);
    }
    if (!near(at, foot.end)) return { error: "its outline is not one closed loop" };
    const draw = flat => {
      const A = flat(foot.start), B = flat(foot.end);
      const width = Math.hypot(B[0] - A[0], B[1] - A[1]);
      const u = [(B[0] - A[0]) / width, (B[1] - A[1]) / width];
      let v = [-u[1], u[0]];
      const s_ = q => (q[0] - A[0]) * u[0] + (q[1] - A[1]) * u[1], h_ = q => (q[0] - A[0]) * v[0] + (q[1] - A[1]) * v[1];
      const points = chain.flatMap(g => [flat(g.start), flat(g.end)]);
      if (points.reduce((sum, q) => sum + h_(q), 0) < 0) v = [-v[0], -v[1]];
      const margin = 0.5;
      // Round the arcs too, and past the flange's end where it was made longer than the outline
      const round = chain.flatMap(pointsAlong).map(q => flat(q));
      const lo = Math.min(0, ...points.map(s_), ...round.map(s_)) - margin, hi = Math.max(width, ...points.map(s_), ...round.map(s_)) + margin;
      const top = Math.max(...points.map(h_), ...round.map(h_), above) + margin;
      const out = (a, b) => [tidy(A[0] + u[0] * a + v[0] * b), tidy(A[1] + u[1] * a + v[1] * b)];
      const items = [], ends = [];
      chain.forEach((g, i) => {
        const as = "o" + i;
        if (g.type === "line") {
          items.push({ type: "line", from: flat(g.start), to: flat(g.end), as });
          ends.push([as + ".start", flat(g.start)], [as + ".end", flat(g.end)]);
          return;
        }
        // PeetCAD's arcs run counter-clockwise: the order of the ends is the one that sweeps the arc's angle
        const c = flat(g.center), a = flat(g.start), b = flat(g.end);
        const turn = ((Math.atan2(b[1] - c[1], b[0] - c[0]) - Math.atan2(a[1] - c[1], a[0] - c[0])) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI);
        const sweep = g.length / g.radius;
        const [start, end] = Math.abs(turn - sweep) <= Math.abs(2 * Math.PI - turn - sweep) ? [a, b] : [b, a];
        items.push({ type: "arc", center: c, start, end, as });
        ends.push([as + ".start", start], [as + ".end", end]);
      });
      items.push({ type: "polyline", points: [B, out(hi, 0), out(hi, top), out(lo, top), out(lo, 0), A], as: "box" });
      ends.push(["box.0.start", B], ["box.4.end", A]);
      const groups = new Map();
      for (const [label, q] of ends) {
        const key = q.map(x => x.toFixed(4)).join(",");
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(label);
      }
      for (const labels of groups.values()) for (let i = 1; i < labels.length; i++) items.push({ type: "coincident", of: [labels[0], labels[i]] });
      return items;
    };
    // On the flange's own face: the middle of the outline is on it. A corner of the outline's foot can
    // be on the edge of the face the flange is bent from as well, which is no place for a cut.
    const sf = frameOf(sketch.sketch.modelToSketch);
    const corners = chain.flatMap(g => [g.start, g.end]);
    // `map` moves the outline (to the other side of a mirror plane)
    const place = q => map ? map(fromSketch(sf, q)) : fromSketch(sf, q);
    const middle = place({ x: corners.reduce((t, q) => t + q.x, 0) / corners.length, y: corners.reduce((t, q) => t + q.y, 0) / corners.length });
    const normal = map ? unit(sub(map(add(sf.o, sf.n)), map(sf.o))) : sf.n;
    let sk = sketchOnFace(name + " sketch", middle, normal, flat => draw(q => flat(place(q))));
    if (sk.error && !map) sk = importSketch(sketch, { name: name + " sketch", needFace: true, draw });
    if (sk.error) return { error: sk.error };
    const reply = peet.apply({ op: "sheet_cut", sketch: sk.name, name });
    if (!reply.ok) { peet.apply({ op: "delete", feature: sk.name }); return { error: reply.error }; }
    return { sketch: sk.name, name: reply.created[0].name };
  }

  const edgeSelector = e => ({ at: mid(mm(e.start), mm(e.end)).map(tidy) });
  /**
   * A base flange from an open profile: a bent strip as deep as SolidWorks' first distance, to one side or
   * both (end condition 5, the mid plane). Lines in line are joined first: SolidWorks takes a bend of
   * nothing between them, PeetCAD refuses one. Which side of the profile the sheet is on and which way the
   * depth goes are not in the definition: each way is tried, and the one kept whose faces lie where the base
   * flange's faces do in SolidWorks' body.
   */
  function openBaseFlange(f, v, op, drawn) {
    const symmetric = v.D1OffsetType === 5;
    const depth = (v.D1OffsetDistance || 0) * MM;
    if (!(depth > 0)) return { ok: false, message: "an open profile with no depth" };
    let sketch = op.sketch;
    const joined = joinInLine(drawn);
    if (joined.length < drawn.length) {
      const sk = importSketch(sketchOf(f), {
        name: f.name + " profile",
        draw: toPeet => {
          const items = [], ends = new Map();
          joined.forEach((g, i) => {
            items.push({ type: "line", from: toPeet(g.start), to: toPeet(g.end), as: "j" + i });
            for (const [k, p] of [["start", g.start], ["end", g.end]]) {
              const key = spot(p);
              if (!ends.has(key)) ends.set(key, []);
              ends.get(key).push(`j${i}.${k}`);
            }
          });
          for (const labels of ends.values()) for (let i = 1; i < labels.length; i++) items.push({ type: "coincident", of: [labels[0], labels[i]] });
          return items;
        },
      });
      if (sk.error) return { ok: false, message: sk.error };
      sketch = sk.name;
      report.notes.push(`${f.name}'s profile has lines in line, joined into one in ${sk.name}.`);
    }
    const wanted = facesOf(f.name).filter(face => face.plane).map(face => {
      const n = unit(face.plane.slice(0, 3));
      return { n, offset: dot(n, mm(face.plane.slice(3, 6))) };
    });
    const score = () => {
      const faces = (peet.ask({ op: "faces" }).faces || []).filter(x => x.surface === "plane" && x.normal);
      return wanted.filter(w => faces.some(x => Math.abs(dot(w.n, x.normal)) > 0.9999 && Math.abs(dot(w.n, x.center) - w.offset) < 1e-3)).length;
    };
    let best = null;
    const mark = peet.script.length;
    // Each bend's own bend model, from SolidWorks' sketched bends under it, given to the corner of the same
    // angle. Which end of the profile PeetCAD counts the bends from is tried both ways.
    const own = (f.sub || []).filter(x => x.type === "SketchBend" && !x.suppressed).map(x => values(x)).map(b => ({
      angle: b.BendAngle, model: b.UseDefaultBendAllowance === false && b.GetCustomBendAllowance ? bendOf(b) : null, used: false,
    }));
    const orders = [undefined];
    if (own.some(b => b.model)) {
      const corners = profileAngles(joined);
      const bends = corners.map(a => {
        const b = own.filter(x => !x.used).sort((p, q) => Math.abs(p.angle - a) - Math.abs(q.angle - a))[0];
        if (!b || Math.abs(b.angle - a) > 1e-3) return null;
        b.used = true;
        return b.model;
      });
      if (bends.some(Boolean)) orders.splice(0, 1, bends, [...bends].reverse());
    }
    for (const bends of orders) for (const reverse of [op.reverse, !op.reverse]) {
      for (const flip_depth of symmetric ? [false] : [false, true]) {
        const tried = { ...op, sketch, reverse, depth: tidy(depth), ...(symmetric ? { symmetric: true } : { flip_depth }), ...(bends ? { bends } : {}) };
        const built = build(f, tried);
        if (!built.ok) { if (!best) best = { built }; continue; }
        const s = score();
        peet.apply({ op: "delete", features: built.names });
        if (!best || !best.op || s > best.score) best = { op: tried, score: s };
      }
    }
    // The tries leave the part as it was: they are no part of the script
    peet.script.length = mark;
    return best.op ? build(f, best.op) : best.built;
  }

  /**
   * A full round fillet (a face rounded off between the two faces beside it), whose faces the export does not
   * name. Between two parallel faces it is a fillet of half the width on the face's two edges, which meet
   * where the face was: those edges are read off SolidWorks' round faces of it.
   */
  function fullRound(f) {
    const round = facesOf(f.name).filter(face => face.surface === "cylinder" && face.cylinder && face.box);
    if (!round.length || round.length !== facesOf(f.name).length) return record(f, "not supported", "a full round fillet that is not round faces between parallel faces");
    const picks = [];
    let size = null;
    for (const face of round) {
      const axis = unit(face.cylinder.slice(3, 6)), centre = mm(face.cylinder.slice(0, 3)), r = face.cylinder[6] * MM;
      if (size !== null && Math.abs(size - r) > 1e-6) return record(f, "not supported", "a full round fillet of more than one radius");
      size = r;
      const lo = mm(face.box.slice(0, 3)), hi = mm(face.box.slice(3));
      const towards = sub(mid(lo, hi), centre), bulge = unit(sub(towards, mul(axis, dot(towards, axis))));
      const side = cross(axis, bulge);
      const corners = [0, 1, 2, 3, 4, 5, 6, 7].map(i => [i & 1 ? hi[0] : lo[0], i & 2 ? hi[1] : lo[1], i & 4 ? hi[2] : lo[2]]);
      const along = corners.map(p => dot(sub(p, centre), axis));
      const [t0, t1] = [Math.min(...along), Math.max(...along)];
      for (const s of [1, -1]) {
        const p = add(add(centre, mul(bulge, r)), mul(side, s * r));
        picks.push({ between: [add(p, mul(axis, t0)).map(tidy), add(p, mul(axis, t1)).map(tidy)] });
      }
    }
    const built = build(f, { op: "fillet", name: f.name, size: tidy(size), edges: picks, chain: false });
    if (built.ok) built.warning = [built.warning, "a full round fillet, as a fillet of half the width on the two edges of the face it rounds off (read off SolidWorks' round face): it does not follow the width"].filter(Boolean).join("; ");
    return finish(f, built);
  }

  /**
   * The face SolidWorks' extrusion goes up to (end condition 4), as a selector, if it is a flat face parallel
   * to the sketch: PeetCAD's extrusion can go up to it and follow it.
   */
  /**
   * A linear pattern direction (`n` is "D1" or "D2") that SolidWorks runs up to a reference (`D1EndCondition`
   * 1, in exports from the fifth on), as `linear_pattern`'s `up_to` fields: { fields } to use, { why } if it
   * can't be, null if the direction doesn't run up to a reference. `d` is the way the copies go. SolidWorks'
   * count (or spacing) is checked first against the rule PeetCAD follows, worked out on SolidWorks' own faces.
   */
  function upToReference(v, n, d) {
    if (v[n + "EndCondition"] !== 1) return null;
    if (!d) return { why: "its direction is not known" };
    // Something to run up to or measure from, with how far along `d` it is from a point
    const bound = (e, what) => {
      if (!e) return { why: `its ${what} was not exported` };
      if (e.kind === "face" && e.surface === "plane" && e.normal && e.point) {
        const normal = unit(e.normal), o = mm(e.point);
        return { sel: { plane: { at: o.map(tidy), normal: e.normal.map(tidy) } }, from: p => dot(sub(o, p), normal) / dot(d, normal), across: Math.abs(dot(d, normal)), at: o };
      }
      if ((e.kind === "plane" || (e.kind === "feature" && e.type === "RefPlane")) && planes.has(e.name) && planeFrames.has(e.name)) {
        const pf = planeFrames.get(e.name);
        return { sel: { plane: planes.get(e.name) }, from: p => dot(sub(pf.o, p), pf.n) / dot(d, pf.n), across: Math.abs(dot(d, pf.n)), at: pf.o };
      }
      if (e.kind === "vertex" && e.point) {
        const o = mm(e.point);
        return { sel: { point: { at: o.map(tidy) } }, from: p => dot(sub(o, p), d), across: 1, at: o };
      }
      // A straight edge square to the direction is as far along it as either of its ends
      if (e.kind === "edge" && e.curve === "line" && e.start && e.end && Math.abs(dot(unit(sub(e.end, e.start)), d)) < 1e-6) {
        const o = mm(e.start);
        return { sel: { point: { at: o.map(tidy) } }, from: p => dot(sub(o, p), d), across: 1, at: o };
      }
      return { why: `its ${what} is ${e.kind === "edge" ? "an edge not square to the direction" : "a " + (e.surface ? e.surface + " " : "") + e.kind}, which PeetCAD's pattern can't run up to` };
    };
    const to = bound(v[n + "EndReference"], "reference");
    if (to.why) return to;
    if (to.across < 1e-9) return { why: "its reference runs along the direction" };
    const seed = v[n + "EndUseSeedReference"] ? bound(v[n + "EndSeedReference"], "seed reference") : null;
    if (seed && seed.why) return seed;
    // The seed's centre: the middle of the box round SolidWorks' faces of what is copied
    const faces = (v.PatternFeatureArray || []).flatMap(p => p ? facesOf(p.name) : []).filter(x => x.box);
    if (!faces.length) return { why: "the faces of what it copies are not in SolidWorks' body" };
    const lo = [0, 1, 2].map(i => Math.min(...faces.map(x => x.box[i]))), hi = [3, 4, 5].map(i => Math.max(...faces.map(x => x.box[i])));
    const centre = mm(lo.map((x, i) => (x + hi[i]) / 2));
    const offset = (v[n + "EndRefOffset"] || 0) * MM, reverse = !!v[n + "EndRefReverseOffset"];
    const span = to.from(centre) - (seed ? seed.from(centre) : 0) - (reverse ? -offset : offset);
    const spacing = v[n + "Spacing"] * MM, count = v[n + "TotalInstances"];
    const keepsSpacing = v[n + "EndUseSpacing"] !== false;
    if (!(span > 1e-9)) return { why: `its reference, less the offset, is ${tidy(span)} mm along the direction from the seed` };
    if (keepsSpacing && Math.floor(span / spacing + 1e-9) + 1 !== count)
      return { why: `SolidWorks has ${count} copies ${tidy(spacing)} mm apart, where ${tidy(span)} mm from the seed to the reference, less the offset, makes room for ${Math.floor(span / spacing + 1e-9) + 1}` };
    if (!keepsSpacing && (count < 2 || Math.abs(span / (count - 1) - spacing) > 1e-6))
      return { why: `SolidWorks has ${count} copies ${tidy(spacing)} mm apart, where spreading them over the ${tidy(span)} mm from the seed to the reference, less the offset, puts them ${tidy(span / Math.max(1, count - 1))} mm apart` };
    const fields = { up_to: to.sel };
    if (offset) fields.up_to_offset = tidy(offset);
    if (reverse) fields.up_to_reverse = true;
    if (seed) fields.up_to_seed = seed.sel;
    if (!keepsSpacing) fields.up_to_keeps = "count";
    // Where the reference is, for finding a vertex as far along if PeetCAD's part hasn't the same one
    return { fields, at: to.across > 1 - 1e-6 ? to.at : null };
  }

  function upToFace(end, v, sf) {
    const face = end === 4 && v["GetFace(true)"];
    if (!face || face.kind !== "face" || face.surface !== "plane" || !face.normal || !face.point) return null;
    if (Math.abs(Math.abs(dot(unit(face.normal), sf.n)) - 1) > 1e-9) return null;
    return { at: mm(face.point).map(tidy), normal: face.normal.map(tidy) };
  }

  /**
   * How far past its own sheet a SolidWorks cut-extrude or hole reaches, as `sheet_cut` fields: null for
   * one that goes through its own sheet only. Through next, and up to a vertex or a surface (or an offset
   * from one), are as deep as SolidWorks' walls of the cut reach.
   */
  function reachOf(end, depth, both) {
    // Through next: as far as a surface that covers the whole profile, which is the far face of the
    // sheet for a hole and past the sides for a notch across a channel
    if (end === 2) return { end: "blind", upTo: true, next: true, ...(both ? { both: true } : {}) };
    if (end === 0 && depth <= (thickness || 0) + 1e-6) return null;
    if (end === 0) return { end: "blind", depth: tidy(depth), ...(both ? { both: true } : {}) };
    if (end === 6) return { end: "blind", depth: tidy(depth / 2), both: true };
    if ([3, 4, 5].includes(end)) return { end: "blind", upTo: true, ...(both ? { both: true } : {}) };
    return { end: "through_all", ...(both ? { both: true } : {}), ...(end !== 1 ? { warning: `SolidWorks' end condition ${end} is cut through all` } : {}) };
  }

  /**
   * A sheet metal cut, as far past its own sheet as `reach` says. Which way is into the sheet is not in
   * SolidWorks' definition in a form that can be read off directly: each way is tried, and the one kept
   * that makes the most of SolidWorks' walls of the cut where they are, and then whose faces come
   * nearest SolidWorks' in area.
   */
  function sheetCut(f, sketch, reach, frame) {
    const op = { op: "sheet_cut", sketch, name: f.name };
    if (!reach) return build(f, op);
    let { warning, upTo, next, ...fields } = reach;
    if (upTo) {
      // As deep as the walls go from the sketch's plane, either way
      const reach = facesOf(f.name).filter(face => face.box).flatMap(face => [mm(face.box.slice(0, 3)), mm(face.box.slice(3))]).map(p => Math.abs(dot(sub(p, frame.o), frame.n)));
      // Through next that goes no further than its own sheet is the plain cut
      if (next && (!reach.length || Math.max(...reach) <= (thickness || 0) + 1e-3)) return build(f, op);
      if (reach.length) fields.depth = tidy(Math.max(...reach) + 0.01);
      else { fields = { end: "through_all", ...(fields.both ? { both: true } : {}) }; warning = "SolidWorks' cut goes up to a surface, and none of its walls is in SolidWorks' body to say where: it is cut through all"; }
    }
    const built = nearest(f, [false, true].map(reverse => ({ ...op, ...fields, ...(reverse ? { reverse } : {}) })));
    if (built.ok && warning) built.warning = warning;
    return built;
  }

  /** SolidWorks' bends of feature `top` (its OneBend and SketchBend sub-features), in order. */
  const bendsOf = top => {
    const out = [];
    const walk = g => { if (g.suppressed) return; if (/^(OneBend|SketchBend)$/.test(g.type)) out.push(g.name); (g.sub || []).forEach(walk); };
    (top.sub || []).forEach(walk);
    return out;
  };
  const sameBends = (a, b) => {
    const x = (a || []).map(g => g.name).sort(), y = (b || []).map(g => g.name).sort();
    return x.length === y.length && x.every((n, i) => n === y[i]);
  };

  /**
   * The bends an Unfold or a Fold names, as PeetCAD selectors: none (every bend) where it names every bend
   * SolidWorks has before it, else a face of each, among the bend faces of what its feature was imported as.
   */
  function bendSelectors(f, bends) {
    const names = (bends || []).map(b => b.name);
    if (!names.length) return {};
    const before = [];
    for (const top of cur.features) {
      if (top.name === f.name) break;
      if (!top.suppressed && top.type !== "FlatPattern") before.push(...bendsOf(top));
    }
    if (before.every(n => names.includes(n))) return {};
    const faces = peet.ask({ op: "faces" }).faces || [];
    const list = [];
    for (const name of names) {
      const owner = cur.features.find(top => bendsOf(top).includes(name));
      const ours = owner && made.get(owner.name);
      if (!ours) return { error: `the feature bend ${name} is of was not imported` };
      // PeetCAD's bends of it in order, each its faces round one axis
      const groups = [];
      for (const n of ours) {
        for (const face of faces) {
          if (!face.axis_point || !(face.made_by || []).some(m => m.feature === n && m.side === "bend")) continue;
          const key = n + "|" + face.axis_point.map(x => x.toFixed(3)).join(",");
          if (!groups.some(g => g.key === key)) groups.push({ key, face });
        }
      }
      const g = groups[bendsOf(owner).indexOf(name)];
      if (!g) return { error: `PeetCAD's ${owner.name} has no bend for ${name}` };
      list.push({ body: g.face.body, index: g.face.index });
    }
    return { list };
  }

  /**
   * Of several ways to build a feature, the one that comes out nearest SolidWorks': that makes the most of
   * SolidWorks' flat faces of it where they are, and then whose faces come nearest SolidWorks' in area. Each
   * is built and taken back again, and the tries are left out of the script.
   */
  function nearest(f, ops) {
    const swFaces = facesOf(f.name);
    const theirs = swFaces.reduce((sum, face) => sum + face.area * 1e6, 0);
    // Where SolidWorks' faces of it are, by area: what tells round holes cut one way from the other
    const swMiddle = theirs > 0 ? mul(swFaces.reduce((sum, face) => add(sum, mul(mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3))), face.area * 1e6)), [0, 0, 0]), 1 / theirs) : null;
    const mark = peet.script.length;
    let best = null;
    for (const op of ops) {
      const tried = build(f, op);
      if (!tried.ok) { if (!best) best = { failed: tried }; continue; }
      const mine = (peet.ask({ op: "faces" }).faces || []).filter(x => (x.made_by || []).some(m => tried.names.includes(m.feature)));
      const ours = mine.reduce((sum, x) => sum + x.area_mm2, 0);
      // With nothing of it in SolidWorks' body (it is suppressed there), the way that makes something
      const off = theirs > 0 ? Math.abs(ours - theirs) : -ours, planes = wallsMatched(f.name, tried.names);
      const away = swMiddle && ours > 0 ? len(sub(mul(mine.reduce((sum, x) => add(sum, mul(x.center, x.area_mm2)), [0, 0, 0]), 1 / ours), swMiddle)) : 0;
      peet.apply({ op: "delete", features: tried.names });
      const same = best && !best.failed && Math.abs(planes - best.planes) < 1e-6 && Math.abs(off - best.off) < 1e-6;
      if (!best || best.failed || planes > best.planes + 1e-6 || (planes > best.planes - 1e-6 && off < best.off - 1e-6) || (same && away < best.away - 1)) best = { op, off, planes, away };
    }
    peet.script.length = mark;
    return best.failed ? best.failed : build(f, best.op);
  }

  /**
   * A reference plane turned about one of the model's axes: a standard plane through that axis turned to
   * it, then a plane offset from that one to where SolidWorks has it. Which standard plane, which way round
   * and which side are tried until the plane comes out where SolidWorks' is. It is a fixed plane, as the
   * offset planes are.
   */
  function turnedPlane(f, pf) {
    const name = f.name.trim();
    const about = ["x", "y", "z"].find(k => Math.abs(dot(pf.n, AXES[k])) < 1e-9);
    if (!about) return record(f, "not supported", "a plane that is not square to a plane of the model");
    const bases = { x: [["top", "z"], ["front", "y"]], y: [["top", "z"], ["right", "x"]], z: [["front", "y"], ["right", "x"]] }[about];
    const distance = tidy(Math.abs(dot(pf.o, pf.n)));
    const mark = peet.script.length;
    for (const [base, normal] of bases) {
      const turn = Math.acos(Math.min(1, Math.abs(dot(pf.n, AXES[normal])))) * 180 / Math.PI;
      for (const angle of [turn, -turn, 180 - turn, turn - 180]) {
        const turned = name + " turned";
        if (!peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name: turned }).ok) continue;
        const at = probe(turned);
        // (Replies give a turned normal to six places)
        const parallel = q => q && Math.abs(Math.abs(dot(q.n, pf.n)) - 1) < 1e-5;
        if (!parallel(at)) { peet.apply({ op: "delete", feature: turned }); continue; }
        for (const flip of distance > 1e-9 ? [false, true] : [false]) {
          if (!peet.apply({ op: "plane", from: turned, distance, flip, name }).ok) continue;
          const where = probe(name);
          if (parallel(where) && Math.abs(dot(sub(where.o, pf.o), pf.n)) < 1e-3) {
            // The part has the two planes: the script keeps them and not the tries
            peet.script.length = mark;
            peet.script.push({ op: "plane", from: base, about, angle: tidy(angle), name: turned }, { op: "plane", from: turned, distance, flip, name });
            planes.set(f.name, name);
            return record(f, "imported", "as a fixed plane turned about an axis");
          }
          peet.apply({ op: "delete", feature: name });
        }
        peet.apply({ op: "delete", feature: turned });
      }
    }
    peet.script.length = mark;
    return record(f, "failed", "the plane did not come out where SolidWorks has it");
  }

  /**
   * A plane where SolidWorks has a sketch's plane \`pf\`, turned about \`about\` (an edge, before a flange is
   * put on it): a standard plane turned about it, offset if the sketch is not on the edge. Named \`name\`;
   * false if no way round puts it there. The tries are left out of the script.
   */
  function planeAbout(name, about, pf) {
    const mark = peet.script.length;
    const parallel = q => q && Math.abs(Math.abs(dot(q.n, pf.n)) - 1) < 1e-5;
    for (const [base, normal] of [["top", [0, 0, 1]], ["front", [0, 1, 0]], ["right", [1, 0, 0]]]) {
      const turn = Math.acos(Math.min(1, Math.abs(dot(pf.n, normal)))) * 180 / Math.PI;
      for (const angle of [turn, -turn, 180 - turn, turn - 180]) {
        const turned = name + " turned";
        if (!peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name: turned }).ok) continue;
        const at = probe(turned);
        if (!parallel(at)) { peet.apply({ op: "delete", feature: turned }); continue; }
        const distance = tidy(Math.abs(dot(sub(pf.o, at.o), at.n)));
        for (const flip of distance > 1e-6 ? [false, true] : [null]) {
          const op = flip === null ? null : { op: "plane", from: turned, distance, flip, name };
          if (op && !peet.apply(op).ok) continue;
          const where = probe(op ? name : turned);
          if (parallel(where) && Math.abs(dot(sub(where.o, pf.o), pf.n)) < 1e-3) {
            // The part has the planes: the script keeps them and not the tries
            peet.script.length = mark;
            if (op) peet.script.push({ op: "plane", from: base, about, angle: tidy(angle), name: turned }, op);
            else {
              peet.run({ op: "delete", feature: turned }, true);
              peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name });
            }
            return true;
          }
          if (op) peet.apply({ op: "delete", feature: name });
        }
        peet.apply({ op: "delete", feature: turned });
      }
    }
    peet.script.length = mark;
    return false;
  }

  /**
   * A plane named `name` where SolidWorks has a sketch's plane `sf`: an offset of the standard plane it is
   * parallel to, or of one turned about an axis of the model to it. Its name, or null if none comes out
   * there. The tries are left out of the script.
   */
  function planeWhere(name, sf) {
    const mark = peet.script.length;
    const axis = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(sf.n, AXES[k])) - 1) < 1e-9);
    if (axis) {
      const from = { x: "right", y: "front", z: "top" }[axis];
      const distance = tidy(Math.abs(dot(sf.o, AXES[axis])));
      for (const flip of distance > 1e-9 ? [false, true] : [false]) {
        const op = distance > 1e-9 ? { op: "plane", from, distance, flip, name } : { op: "plane", from, distance: 0, name };
        if (!peet.apply(op).ok) continue;
        const where = probe(name);
        if (where && samePlane(where, sf)) { peet.script.length = mark; peet.script.push(op); return name; }
        peet.apply({ op: "delete", feature: name });
      }
      peet.script.length = mark;
      return null;
    }
    const about = ["x", "y", "z"].find(k => Math.abs(dot(sf.n, AXES[k])) < 1e-9);
    if (!about) return null;
    return planeAbout(name, about, sf) ? name : null;
  }

  const values = f => (f.definition && f.definition.values) || {};

  function bendOf(v) {
    const c = v.GetCustomBendAllowance;
    if (c && c.type === 4) return { deduction: tidy(c.bendDeduction * MM) };
    if (c && c.type === 3) return { allowance: tidy(c.bendAllowance * MM) };
    if (c && c.type === 1) report.notes.push("The bend allowance comes from a bend table, which PeetCAD doesn't read: the K-factor is used.");
    return { k_factor: (c && c.kFactor) || v.KFactor || 0.5 };
  }

  const LATER = "left for the configuration it is live in";
  /** A feature a pattern or mirror copies that is suppressed in SolidWorks and could not be built: SolidWorks copies nothing of it. */
  const unbuilt = p => p && !made.has(p.name) && (byName.get(p.name) || {}).suppressed === true;
  const elsewhere = doc.configurations.filter(c => c !== config && !c.flatPattern).map(flatten);
  const liveElsewhere = name => elsewhere.some(m => m.has(name) && !m.get(name).suppressed);

  // ---- Bodies: which is which, and where SolidWorks moved them ----

  /**
   * The PeetCAD features whose faces are on the body SolidWorks calls `name` (named after the last feature that
   * changed it, "Split1[2]" for a piece): a body moved by a Move/Copy Body is the body it moved.
   */
  function bodyFeatures(name) {
    const base = String(name).replace(/\[\d+\]$/, "");
    if (moves.has(base)) return moves.get(base);
    return made.get(base) || null;
  }
  /** A face of the body SolidWorks calls `name`, as a selector, with the body's index; null if it is not there. */
  function bodyPick(name) {
    const names = bodyFeatures(name);
    if (!names || !names.length) return null;
    const face = (peet.ask({ op: "faces" }).faces || []).find(x => (x.made_by || []).some(m => names.includes(m.feature)));
    return face ? { body: face.body, pick: { body: face.body, index: face.index } } : null;
  }

  /** The flat and round faces of a body of the part, for matching: directions, a point on each, sizes. */
  function oursToMatch(body) {
    return (peet.ask({ op: "faces" }).faces || []).filter(x => x.body === body).flatMap(x =>
      x.surface === "plane" && x.normal ? [{ kind: "plane", d: unit(x.normal), p: x.center, area: x.area_mm2 }]
        : x.surface === "cylinder" && x.axis && x.axis_point ? [{ kind: "cylinder", d: unit(x.axis), p: x.axis_point, r: x.radius, area: x.area_mm2 }]
          : []);
  }
  /** The same of a body in SolidWorks' export. */
  function theirsToMatch(body) {
    return body.faces.flatMap(x =>
      x.surface === "plane" && x.plane ? [{ kind: "plane", d: unit(x.normal || x.plane.slice(0, 3)), p: mm(x.plane.slice(3, 6)), area: x.area * 1e6 }]
        : x.surface === "cylinder" && x.cylinder ? [{ kind: "cylinder", d: unit(x.cylinder.slice(3, 6)), p: mm(x.cylinder.slice(0, 3)), r: x.cylinder[6] * MM, area: x.area * 1e6 }]
          : []);
  }

  /**
   * The rigid motion that puts the faces `ours` where `theirs` are, found from faces of the same kind and size:
   * `{ m, t, matched, of }`, `m` the rotation's matrix (rows), `t` the translation, `matched` of `of` faces of
   * ours that land on one of theirs. Null if none puts more than half there.
   */
  function register(ours, theirs) {
    const same = (a, b) => a.kind === b.kind && Math.abs(a.area - b.area) <= 1e-4 * Math.max(1, a.area) + 1e-4
      && (a.kind !== "cylinder" || Math.abs(a.r - b.r) < 1e-4);
    const cands = ours.map(o => theirs.filter(t => same(o, t)));
    const order = ours.map((_, i) => i).filter(i => cands[i].length).sort((i, j) => cands[i].length - cands[j].length);
    if (!order.length) return null;
    const frame = (u, w) => { const e1 = unit(u), e2 = unit(sub(w, mul(e1, dot(w, e1)))); return [e1, e2, cross(e1, e2)]; };
    const perp = u => unit(cross(u, Math.abs(u[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0]));
    const turn = (E, F) => ({ E, F });
    const rotate = (R, x) => add(add(mul(R.F[0], dot(R.E[0], x)), mul(R.F[1], dot(R.E[1], x))), mul(R.F[2], dot(R.E[2], x)));
    // Turns that take two faces' directions to two of theirs at the same angle (a direction either way round)
    const turns = [turn([[1, 0, 0], [0, 1, 0], [0, 0, 1]], [[1, 0, 0], [0, 1, 0], [0, 0, 1]])];
    const i1 = order[0], u1 = ours[i1].d;
    const i2 = order.find(i => len(cross(u1, ours[i].d)) > 0.1);
    for (const t1 of cands[i1]) for (const s1 of [1, -1]) {
      const w1 = mul(t1.d, s1);
      if (i2 === undefined) { turns.push(turn(frame(u1, perp(u1)), frame(w1, perp(w1)))); continue; }
      const u2 = ours[i2].d;
      for (const t2 of cands[i2]) for (const s2 of [1, -1]) {
        const w2 = mul(t2.d, s2);
        if (Math.abs(dot(u1, u2) - dot(w1, w2)) > 1e-6) continue;
        turns.push(turn(frame(u1, u2), frame(w1, w2)));
      }
    }
    // Where a face of ours lands on one of theirs, given a turn and a shift
    const lands = (o, t, R, shift) => {
      const d = rotate(R, o.d);
      if (Math.abs(Math.abs(dot(d, t.d)) - 1) > 1e-6) return false;
      const p = add(rotate(R, o.p), shift);
      return o.kind === "plane" ? Math.abs(dot(sub(p, t.p), t.d)) < 1e-3 : len(cross(sub(p, t.p), t.d)) < 1e-3;
    };
    let best = null;
    for (const R of turns) {
      // Each pair of faces that point the same way says where the shift is across them
      const rows = [];
      for (const [i, o] of ours.entries()) for (const t of cands[i]) {
        const d = rotate(R, o.d);
        if (Math.abs(Math.abs(dot(d, t.d)) - 1) > 1e-6) continue;
        const gap = sub(t.p, rotate(R, o.p));
        const across = t.kind === "plane" ? [t.d] : [perp(t.d), cross(t.d, perp(t.d))];
        rows.push(across.map(n => [n, dot(n, gap)]));
        if (rows.length > 14) break;
      }
      const shifts = [[0, 0, 0]];
      for (let a = 0; a < rows.length; a++) for (let b = a; b < rows.length; b++) for (let c = b; c < rows.length; c++) {
        const eqs = [...new Set([a, b, c])].flatMap(k => rows[k]);
        const shift = solve3(eqs);
        if (shift) shifts.push(shift);
      }
      for (const shift of shifts) {
        const matched = ours.filter((o, i) => cands[i].some(t => lands(o, t, R, shift))).length;
        if (!best || matched > best.matched) best = { R, shift, matched };
      }
    }
    if (!best || best.matched * 2 <= ours.length) return null;
    const m = [0, 1, 2].map(r => [0, 1, 2].map(c => rotate(best.R, AXES["xyz"[c]])[r]));
    return { m, t: best.shift, matched: best.matched, of: ours.length };
  }
  /** The least-squares point of plane equations `[[normal, offset], …]`, if they fix it in all three directions. */
  function solve3(eqs) {
    const A = [[0, 0, 0], [0, 0, 0], [0, 0, 0]], b = [0, 0, 0];
    for (const [n, d] of eqs) for (let r = 0; r < 3; r++) { b[r] += n[r] * d; for (let c = 0; c < 3; c++) A[r][c] += n[r] * n[c]; }
    const det = M => M[0][0] * (M[1][1] * M[2][2] - M[1][2] * M[2][1]) - M[0][1] * (M[1][0] * M[2][2] - M[1][2] * M[2][0]) + M[0][2] * (M[1][0] * M[2][1] - M[1][1] * M[2][0]);
    const D = det(A);
    if (Math.abs(D) < 1e-9) return null;
    return [0, 1, 2].map(k => det(A.map((row, r) => row.map((x, c) => (c === k ? b[r] : x)))) / D);
  }
  /** A rotation matrix as turns about X, then Y, then Z, in degrees (what move_body takes). */
  function eulerXYZ(m) {
    const y = Math.asin(Math.max(-1, Math.min(1, -m[2][0])));
    const x = Math.atan2(m[2][1], m[2][2]), z = Math.atan2(m[1][0], m[0][0]);
    return [x, y, z].map(a => tidy(a * 180 / Math.PI));
  }

  /**
   * The mates SolidWorks places a moved body by (Move/Copy Body's constraints), as move_body's `mates`: each end
   * found among the part's faces and edges, on the moved body where it is before the move (SolidWorks gives the
   * ends where the move put them: `motion`, the move found from its body, takes them back). `{ mates }`, or
   * `{ why }` if one can't be had.
   */
  function bodyMates(f, body, motion) {
    const live = (f.sub || []).filter(x => x.mate && !x.suppressed);
    if (!live.length) return { why: "it has no mates" };
    const R = motion.m, t = motion.t;
    const turn = x => [0, 1, 2].map(r => dot(R[r], x));
    const back = x => [0, 1, 2].map(c => R[0][c] * x[0] + R[1][c] * x[1] + R[2][c] * x[2]);
    const faces = peet.ask({ op: "faces" }).faces || [];
    const edges = peet.ask({ op: "edges" }).edges || [];
    const CLOSE = 1e-3, along = (a, b) => Math.abs(Math.abs(dot(unit(a), unit(b))) - 1) < 1e-6;
    // An end: what it is (a plane, an axis or a point) and its selector, on the moved body or on the rest
    const find = (e, moved) => {
      if (!e.params || e.params.length < 6) return null;
      const ent = e.entity || {};
      const p0 = mm(e.params.slice(0, 3)), d0 = unit(e.params.slice(3, 6)), r = (e.params[6] || 0) * MM;
      const P = moved ? back(sub(p0, t)) : p0, D = moved ? back(d0) : d0;
      const A = ent.point && (moved ? back(sub(mm(ent.point), t)) : mm(ent.point));
      const mine = x => (moved ? x.body === body : x.body !== body);
      const nearest = list => list.sort((a, b) => (A ? len(sub(a.at, A)) - len(sub(b.at, A)) : b.area - a.area))[0];
      // swSelectType_e: 2 a face, 1 an edge
      if (e.referenceType === 2) {
        const round = ent.surface ? ent.surface === "cylinder" : r > 0;
        const hits = faces.filter(x => mine(x) && (round
          ? x.surface === "cylinder" && x.axis && x.axis_point && along(x.axis, D) && len(cross(sub(x.axis_point, P), D)) < CLOSE && Math.abs(x.radius - r) < CLOSE
          : x.surface === "plane" && x.normal && x.center && along(x.normal, D) && Math.abs(dot(sub(x.center, P), D)) < CLOSE));
        const hit = nearest(hits.map(x => ({ x, at: x.center, area: x.area_mm2 })));
        if (!hit) return null;
        const x = hit.x;
        return round ? { kind: "line", p: x.axis_point, d: unit(x.axis), sel: { face: { body: x.body, index: x.index } } }
          : { kind: "plane", p: x.center, d: unit(x.normal), sel: { face: { body: x.body, index: x.index } } };
      }
      if (e.referenceType === 1 && (ent.curve === "circle" || r > 0)) {
        const hits = edges.filter(x => mine(x) && x.curve === "circle" && x.center && x.axis && along(x.axis, D) && len(sub(x.center, P)) < CLOSE && Math.abs(x.radius - r) < CLOSE);
        if (hits.length !== 1) return null;
        const x = hits[0];
        return { kind: "line", p: x.center, d: unit(x.axis), sel: { edge: { body: x.body, index: x.index } } };
      }
      return null;
    };
    const mates = [];
    for (const s of live) {
      const kind = s.mate.type;
      if (!/^(coincident|concentric|parallel|perpendicular|distance|angle)$/.test(kind)) return { why: `${s.name} is a ${kind} mate, which PeetCAD's body mates don't have` };
      const ends = s.mate.entities || [];
      if (ends.length !== 2) return { why: `${s.name} doesn't have two ends` };
      // Which end is on the moved body
      let pair = null;
      for (const [im, it] of [[1, 0], [0, 1]]) {
        const moved = find(ends[im], true), to = find(ends[it], false);
        if (moved && to) { pair = { moved, to }; break; }
      }
      if (!pair) return { why: `the ends of ${s.name} were not found on the moved body and the rest of the part` };
      const { moved, to } = pair;
      // Where the moved end is once moved, to tell which way round and which side
      const md = turn(moved.d), mp = add(turn(moved.p), t);
      const op = { type: kind, name: s.name, moved: moved.sel, to: to.sel };
      if (moved.kind === "plane" && to.kind === "plane" && dot(md, to.d) > 0) op.aligned = true;
      const dim = s.dimensions && s.dimensions[0] && s.dimensions[0].value;
      if (kind === "distance") {
        const offset = to.kind === "plane" ? dot(sub(mp, to.p), to.d) : moved.kind === "plane" ? dot(sub(to.p, mp), md) : null;
        op.distance = tidy(dim != null ? dim * MM : Math.abs(offset || 0));
        if (offset != null && offset < 0) op.flip = true;
      } else if (kind === "angle") {
        if (dim == null) return { why: `${s.name} has no angle` };
        op.angle = tidy(dim * 180 / Math.PI);
        if (Math.abs(dot(md, to.d) + Math.cos(dim)) < Math.abs(dot(md, to.d) - Math.cos(dim))) op.flip = true;
      }
      mates.push(op);
    }
    return { mates };
  }

  /**
   * The axis a feature turns about, as a selector for an axis: an axis SolidWorks names that was imported, a
   * standard axis, or the axis of a round face of the part along the line of a revolve it was made from (a
   * temporary axis). Null if none is found.
   */
  function axisOf(f, ref) {
    if (ref && ref.kind === "feature" && ref.name && made.has(ref.name)) return made.get(ref.name)[0];
    const lines = [];
    for (const name of [ref && ref.name, ...(f.parents || [])]) {
      const r = name && byName.get(name);
      if (!r || !/^(Revolution|RevCut)$/.test(r.type)) continue;
      const a = values(r).Axis, sketch = sketchOf(r);
      if (!a || !a.start || !a.end || !sketch || !sketch.sketch) continue;
      const sf = frameOf(sketch.sketch.modelToSketch);
      const p = fromSketch(sf, a.start);
      lines.push({ p, d: unit(sub(fromSketch(sf, a.end), p)) });
    }
    for (const { p, d } of lines) {
      const k = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(d, AXES[k])) - 1) < 1e-9);
      if (k && len(cross(p, AXES[k])) < 1e-6) return k;
      const round = (peet.ask({ op: "faces" }).faces || []).find(x => x.surface === "cylinder" && x.axis_point && Math.abs(Math.abs(dot(unit(x.axis), d)) - 1) < 1e-9 && len(cross(sub(x.axis_point, p), d)) < 1e-6);
      if (round) {
        const name = f.name + " axis";
        if (peet.apply({ op: "axis", name, face: { body: round.body, index: round.index } }).ok) return name;
      }
    }
    return null;
  }

  /**
   * A feature as SolidWorks has it: one that is suppressed is built like any other and then suppressed, in
   * every configuration. It can't be compared with anything, since SolidWorks' body does not have it.
   */
  function handle(f) {
    if (!f.suppressed || STRUCTURE.test(f.type) || f.type === "SheetMetal" || f.type === "RefPlane") return handleLive(f);
    // One that is live in another configuration is left for that configuration: there SolidWorks' body has
    // it, so the side a flange bends to and the like can be checked, and the part is the size it is drawn for.
    // So is a sketch: where it is suppressed, SolidWorks' export has it as it was last solved, not where it
    // is (Weather Shield Door's Sketch18, 100 mm off), and made now it would be on whatever is there instead
    // of what it is on (Meter Bracket's Sketch69, on the left-hand return where the right-hand one is).
    if (cur === config && liveElsewhere(f.name)) return record(f, LATER);
    const before = report.features.length, had = made.has(f.name);
    // What it is built on that was imported asleep (a flange its sketch is on, the slot a sketch converts the
    // edges of) is woken while it is built, as it is there in SolidWorks wherever this is unsuppressed
    const woken = [], seen = new Set();
    (function wake(parents) {
      for (const p of parents || []) {
        if (seen.has(p)) continue;
        seen.add(p);
        if (asleep.has(p)) woken.push(...made.get(p));
        wake((byName.get(p) || {}).parents);
      }
    })(f.parents);
    const sleep = on => { for (const n of woken) peet.apply({ op: "suppress", feature: n, on, configurations: "all" }); };
    sleep(false);
    asleepNow = !liveElsewhere(f.name);
    try { handleLive({ ...f, suppressed: false }); } finally { asleepNow = false; }
    sleep(true);
    const names = made.get(f.name);
    const built = !had && names && names.length > 0 && names.every(n => peet.apply({ op: "suppress", feature: n, on: true, configurations: "all" }).ok);
    if (built) asleep.add(f.name);
    for (const x of report.features.slice(before)) {
      if (x.name !== f.name) continue;
      if (built) x.result = "imported and suppressed";
      else if (SKETCH.test(f.type) && /^imported/.test(x.result)) x.result = "imported (suppressed in SolidWorks)";
      else { x.message = "and it could not be imported: " + (x.message || x.result); x.result = "suppressed"; if (names && !had) { peet.apply({ op: "delete", features: names }); made.delete(f.name); } }
    }
  }

  function handleLive(f) {
    const v = values(f);
    if (STRUCTURE.test(f.type)) return;
    if (f.type === "SheetMetal") { if (!sheet) sheet = v; return; }

    if (f.type === "RefPlane") {
      if (!f.planeTransform) return;
      const pf = planeFrameOf(f.planeTransform);
      planeFrames.set(f.name, pf);
      const standard = standardPlane(pf);
      if (standard) { planes.set(f.name, standard); return; }
      if (f.suppressed) return record(f, "suppressed");
      // A plane square to an axis is an offset of a standard plane. It is where SolidWorks has it, but it
      // no longer follows what it was defined from.
      const axis = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(pf.n, AXES[k])) - 1) < 1e-9);
      if (!axis) return turnedPlane(f, pf);
      const from = { x: "right", y: "front", z: "top" }[axis];
      const distance = Math.abs(dot(pf.o, AXES[axis]));
      // PeetCAD trims the spaces off a name ("Hood Zenith ")
      const name = f.name.trim();
      for (const flip of [false, true]) {
        const op = { op: "plane", from, distance: tidy(distance), flip, name };
        const tried = peet.apply(op);
        if (!tried.ok) return record(f, "failed", tried.error);
        const where = probe(name);
        if (where && samePlane(where, pf)) { planes.set(f.name, name); return record(f, "imported", "as a fixed offset plane"); }
        peet.apply({ op: "delete", feature: name });
      }
      return record(f, "failed", "the plane did not come out where SolidWorks has it");
    }

    if (SKETCH.test(f.type)) {
      if (!f.sketch || f.sketch.error || (f.sketch.segments.length === 0)) return;
      const imported = importSketch(f);
      if (imported.error) return record(f, "left for the feature that uses it", imported.error);
      sketches.set(f.name, imported);
      return record(f, imported.partial.length ? "imported in part" : "imported", imported.partial.length ? "without its " + imported.partial.join(" and ") : null);
    }

    switch (f.type) {
      case "SMBaseFlange": {
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const settings = sheet || v;
        thickness = (v.Thickness || settings.Thickness) * MM;
        let radius = (settings.BendRadius || v.BendRadius || 0) * MM;
        const relief = { 1: "rectangular", 2: "tear", 3: "obround" }[settings.AutoReliefType];
        // Which side of the sketch plane the sheet is on is read off the faces the base flange has in
        // SolidWorks' body: the one that is not on the sketch plane is a thickness away, on that side.
        // Failing that, behind the plane, where most of the library's are.
        const sf = frameOf(sketchOf(f).sketch.modelToSketch);
        let side = -1;
        for (const b of cur.bodies || []) for (const face of b.faces) if (face.feature === f.name && face.plane && Math.abs(dot(face.plane.slice(0, 3), sf.n)) > 0.9999) {
          const offset = dot(sub(mm(face.plane.slice(3)), sf.o), sf.n);
          if (Math.abs(Math.abs(offset) - thickness) < 1e-3) side = Math.sign(offset);
        }
        const direction = mul(sf.n, side);
        const op = {
          op: "base_flange", sketch: sk.name, name: f.name, thickness: tidy(thickness), radius: tidy(radius), bend: bendOf(settings),
          ...(relief ? { relief } : {}), ...(settings.ReliefRatio ? { relief_ratio: settings.ReliefRatio } : {}),
          reverse: dot(direction, sk.frame.n) < 0,
        };
        const drawn = sketchOf(f).sketch.segments.filter(g => !g.construction);
        // A closed sketch on a part that has sheet metal already is a tab, which SolidWorks adds to the body
        // whose face it is on. One that touches no sheet is a body of its own, as a base flange is.
        if (!isOpen(drawn) && sheetBody()) {
          const tab = build(f, { op: "base_flange", sketch: sk.name, name: f.name, tab: true });
          if (tab.ok) return finish(f, tab);
        }
        if (!isOpen(drawn)) return finish(f, build(f, op));
        return finish(f, openBaseFlange(f, v, op, drawn));
      }

      case "EdgeFlange": {
        const edges = (v.Edges || []).filter(e => e && e.start && e.end);
        if (edges.length === 0) return record(f, "failed", "its edges were not exported");
        const position = { 1: "material_inside", 2: "material_outside", 3: "bend_outside" }[v.PositionType];
        if (!position) return record(f, "not supported", `flange position type ${v.PositionType}`);
        const angle = v.BendAngle * 180 / Math.PI;
        const radius = v.UseDefaultBendRadius === false ? v.BendRadius * MM : (sheet ? sheet.BendRadius * MM : 0);
        // PeetCAD measures a flange from the outer corner. SolidWorks can also measure from the inner corner
        // or from where the bend ends.
        const notes = [];
        let length = v.OffsetDistance * MM;
        if (v.OffsetDimType === 2) length += thickness * Math.tan(v.BendAngle / 2);
        // Measured to the plane that touches the outside of the bend: for a square bend that is the outer
        // corner too. For other angles it is not, and the flange comes out a little long or short.
        if (v.OffsetDimType === 3 && Math.abs(v.BendAngle - Math.PI / 2) > 1e-6) notes.push("its length is measured to the bend's tangent, which is only the same as PeetCAD's at 90 degrees");
        // SolidWorks' position offset: the flange set out from its edge (in, reversed)
        const setOut = v.UsePositionOffset && v.PositionOffsetDistance > 0 ? tidy(v.PositionOffsetDistance * MM * (v.ReversePositionOffset ? -1 : 1)) : 0;
        if (setOut && v.PositionOffsetType !== 1) notes.push(`its position offset is up to something (type ${v.PositionOffsetType}): the distance it had is used`);
        // Its own sketch is the flange's outline. One narrower than the edge sets the flange back from the ends.
        const profiles = (f.sub || []).filter(c => SKETCH.test(c.type) && c.sketch && !c.sketch.error);
        // Every edge is found before any flange is added: a flange changes the ends of its neighbours
        const found = edges.map(e => findEdge(mm(e.start), mm(e.end)));
        if (found.some(e => !e)) return record(f, "failed", `no edge of the part is within ${NEAR} mm of where SolidWorks has the flange's edge`);
        const far = Math.max(...found.map(e => e.away));
        if (far > 1e-3) notes.push(`its edge is ${far.toFixed(3)} mm from where SolidWorks has it`);
        // On several edges each edge has an outline of its own, and SolidWorks makes each piece as long as its
        // outline reaches: a rectangle that rises further or less far from its foot than the first one makes
        // that piece longer or shorter by as much (the flange's length is the first one's). At 90 degrees the
        // foot is on the face of the sheet the flange is bent towards, so the length is the rise and the
        // thickness, whichever outline comes first. An outline of another shape is made to its own reach.
        const rises = profiles.length === edges.length && edges.length > 1 ? edges.map((e, i) => {
          const drawn = profiles[i].sketch.segments.filter(g => !g.construction);
          if (drawn.length !== 4 || drawn.some(g => g.type !== "line" || !g.start || !g.end)) return null;
          const sf = frameOf(profiles[i].sketch.modelToSketch), s = mm(e.start), along = unit(sub(mm(e.end), s));
          const away = drawn.flatMap(g => [g.start, g.end]).map(q => { const d = sub(fromSketch(sf, q), s); return len(sub(d, mul(along, dot(d, along)))); });
          // An outline that is not at its edge (a suppressed flange's, as SolidWorks last solved it for
          // another part: Esc Surround R Top's, still where Esc Surround Horizontal's edges are) says
          // nothing of where the piece reaches: the flange's length stands
          if (Math.min(...away) > (thickness || 0) + radius + 0.05) return null;
          return Math.max(...away) - Math.min(...away);
        }) : null;
        const square = Math.abs(angle - 90) < 1e-6 && thickness;
        const lengthOf = index => !rises || rises[index] === null ? length
          : square ? rises[index] + thickness : rises[0] !== null ? length + rises[index] - rises[0] : length;
        const ops = found.map((edge, index) => {
          const s = edge.from, along = unit(sub(edge.to, edge.from)), whole = len(sub(edge.to, edge.from));
          let offsets = {}, outline = null, used = null;
          // SolidWorks lists one outline per edge, in the edges' order. Failing that, each is tried.
          for (const p of profiles.length === found.length ? [profiles[index]] : profiles) {
            const sf = frameOf(p.sketch.modelToSketch);
            // The outline's foot is its line along the edge that is nearest the edge. It may be a thickness
            // away (drawn from the other face of the sheet). A neighbouring flange's outline has a side
            // near this edge too, so only a line along the edge counts, and only if it is this close.
            let foot = null;
            for (const g of p.sketch.segments) {
              if (g.type !== "line" || !g.start || !g.end) continue;
              const a = sub(fromSketch(sf, g.start), s), b = sub(fromSketch(sf, g.end), s);
              if (len(sub(b, a)) < 1e-6 || Math.abs(dot(unit(sub(b, a)), along)) < 0.9999) continue;
              const away = len(sub(a, mul(along, dot(a, along))));
              if (away > (thickness || 0) + radius + Math.max(0.05, 2 * edge.away)) continue;
              if (!foot || away < foot.away - 1e-6) foot = { away, on: [dot(a, along), dot(b, along)], g };
            }
            if (!foot) continue;
            const on = foot.on;
            let start = Math.min(...on), end = whole - Math.max(...on);
            // A rectangle is four lines, each along the edge or square to it. Anything else is cut to shape.
            const drawn = p.sketch.segments.filter(g => !g.construction);
            const square = g => { const d = unit(sub(fromSketch(sf, g.end), fromSketch(sf, g.start))), c = Math.abs(dot(d, along)); return c > 0.9999 || c < 1e-4; };
            // How far the outline rises from its foot, which the flange has to reach before it is cut to it
            const fa = fromSketch(sf, foot.g.start), fu = unit(sub(fromSketch(sf, foot.g.end), fa));
            const rise = Math.max(...drawn.flatMap(pointsAlong).map(q => { const d = sub(fromSketch(sf, q), fa); return len(sub(d, mul(fu, dot(d, fu)))); }));
            // A rectangle taller than the flange's length (one edge of a flange on several, its profile drawn
            // taller than the others) is as tall as it is drawn: it is the flange's outline
            const taller = Math.abs(angle - 90) < 1e-6 && rise > length - (thickness || 0) + 0.01;
            // A rectangle whose sketch has a length that is none of the flange's own sizes (its height from
            // a foot a thickness off the corner, or at an angle: Box Roof Top's Depth@Sketch13, 700 where the
            // flange is 703.1 to its outer corner) is the flange's outline too, as in SolidWorks, so that the
            // dimension drives the flange and can be set. One flange on one edge only.
            const sizes = [length, Math.abs(start), Math.abs(end)];
            const unnamed = found.length === 1 && (p.dimensions || []).some(d => d.valueType === 0 && d.drivenState !== 1
              && Math.abs(d.value * MM - rise) < 1e-6 && sizes.every(x => Math.abs(d.value * MM - x) > 1e-6));
            if (taller || unnamed || drawn.length !== 4 || drawn.some(g => g.type !== "line" || !g.start || !g.end || !square(g))) {
              outline = { sketch: p, foot: foot.g, rise };
              // An outline that widens past an end of the edge its foot runs to: the flange is made that
              // wide (it overhangs the edge from the bend on) and cut back to the outline.
              const reach = drawn.flatMap(g => [g.start, g.end]).filter(Boolean).map(q => dot(sub(fromSketch(sf, q), s), along));
              if (Math.abs(start) < 1e-3 && Math.min(...reach) < -1e-3) start = Math.min(...reach);
              if (Math.abs(end) < 1e-3 && whole - Math.max(...reach) < -1e-3) end = whole - Math.max(...reach);
            } else if (Math.abs(v.BendAngle - Math.PI / 2) < 1e-9 && v.OffsetType === 1) {
              // A rectangle of a flange on several edges has a height of its own, which SolidWorks keeps
              // when it differs from the feature's length. The length is from the face of the sheet the
              // flange turns away from: the rectangle's height, and a thickness more if its foot is on the
              // other face (which SolidWorks' body has a face a thickness behind)
              const fa = fromSketch(sf, foot.g.start), fu = unit(sub(fromSketch(sf, foot.g.end), fa));
              const ups = drawn.flatMap(g => [g.start, g.end]).map(q => { const d = sub(fromSketch(sf, q), fa); return sub(d, mul(fu, dot(d, fu))); });
              const top = ups.reduce((a, b) => len(b) > len(a) ? b : a);
              const rise = len(top), u = rise > 1e-9 ? unit(top) : null;
              const levels = u ? (cur.bodies || []).flatMap(b => b.faces).filter(x => x.plane && x.normal && Math.abs(dot(unit(x.normal), u)) > 0.9999).map(x => dot(u, mm(x.plane.slice(3, 6)))) : [];
              const at = level => levels.some(o => Math.abs(o - level) < 1e-3);
              const t = thickness || 0, level = u ? dot(u, fa) : 0;
              const own = !u || !at(level) || at(level - t) === at(level + t) ? null : at(level - t) ? rise + t : rise;
              if (found.length > 1 && own !== null && Math.abs(own - length) > 1e-3) offsets.length = tidy(own);
            }
            // A foot that runs past an end of the edge is a flange wider than its edge: a negative offset.
            if (Math.abs(start) > 1e-3) offsets.offset_start = tidy(start);
            if (Math.abs(end) > 1e-3) offsets.offset_end = tidy(end);
            used = p;
            break;
          }
          // An outline taller than the flange: the flange is made taller (by more than it can need, the cut
          // takes the rest), so that there is sheet to cut the outline from
          // (A flange's end is its length less the sheet's thickness from the face its outline is drawn from.)
          const own = lengthOf(index);
          if (Math.abs(own - length) > 1e-6) offsets.length = tidy(own);
          if (outline && outline.rise > own - (thickness || 0) + 0.01) {
            offsets.length = tidy(outline.rise + 2 * ((thickness || 0) + radius) + 1);
            outline.above = offsets.length;
            notes.push("its outline reaches further than its length: the flange is made long enough and cut to the outline");
          }
          // The set-backs were read from the edge's `from`: if the flange counts its start from the other end,
          // they change places
          let known = false;
          if ((offsets.offset_start || 0) !== (offsets.offset_end || 0)) {
            const fromStart = startsAtFrom(edge);
            known = fromStart !== null;
            if (fromStart === false) {
              const [a, b] = [offsets.offset_start, offsets.offset_end];
              delete offsets.offset_start; delete offsets.offset_end;
              if (b !== undefined) offsets.offset_start = b;
              if (a !== undefined) offsets.offset_end = a;
            }
          }
          // Which end of the edge is the flange's start, for naming its set-backs (see setBackField)
          if (used) flangeEnds.set(`${f.name}|${used.name}`, { from: edge.from, to: edge.to, sf: frameOf(used.sketch.modelToSketch), startAtFrom: startsAtFrom(edge) });
          return { edge: { at: edge.middle.map(tidy) }, offsets, outline, found: edge, known };
        });
        // A relief of the flange's own, where SolidWorks does not use the body's. One given as a width and
        // a depth becomes a ratio of its width: PeetCAD's relief is as deep past the bend as it is wide.
        const ownRelief = {};
        if (v.UseDefaultBendRelief === false) {
          // A relief under a micron wide (and not a ratio) cuts nothing: it is a tear
          const none = !v.UseReliefRatio && v.ReliefWidth > 0 && v.ReliefWidth * MM < 1e-3;
          const kind = none ? "tear" : { 1: "rectangular", 2: "tear", 3: "obround" }[v.AutoReliefType];
          if (kind) ownRelief.relief = kind;
          if (kind && kind !== "tear") {
            if (v.UseReliefRatio && v.ReliefRatio > 0) ownRelief.relief_ratio = tidy(v.ReliefRatio);
            else if (v.ReliefWidth > 0 && thickness) {
              ownRelief.relief_ratio = tidy(v.ReliefWidth * MM / thickness);
              if (Math.abs(v.ReliefWidth - v.ReliefDepth) > 1e-9) notes.push("its relief has a width and a depth of its own: the width is used for both");
            }
          }
        }
        const common = {
          op: "edge_flange", name: f.name, length: tidy(length), angle: tidy(angle), position,
          ...(setOut ? { position_offset: setOut } : {}),
          ...(v.UseDefaultBendRadius === false ? { radius: tidy(radius) } : {}),
          ...ownRelief,
          // A bend allowance of the flange's own
          ...(v.UseDefaultBendAllowance === false && v.GetCustomBendAllowance ? { bend: bendOf(v) } : {}),
        };
        // Up to a vertex: the flange runs up to the vertex of the part where SolidWorks' is, its end through
        // it square to the flange, and follows it. The length it had is where PeetCAD starts from.
        if (v.OffsetType !== 1) {
          const ref = v.OffsetReference;
          const upTo = v.OffsetType === 2 && ref && ref.kind === "vertex" && ref.point ? { at: mm(ref.point).map(tidy) } : null;
          const fits = upTo && [false, true].some(flip => {
            const reply = peet.ask({ ...common, up_to: upTo, edge: ops[0].edge, ...ops[0].offsets, flip });
            return reply.ok && !(reply.created || []).some(c => c.status === "failed");
          });
          if (fits) common.up_to = upTo;
          else notes.push(`its length is up to ${upTo ? "a vertex" : `something (type ${v.OffsetType})`} in SolidWorks${upTo ? " that the part does not have" : ""}: the distance it had is used`);
        }
        // An outline that is not a rectangle, as the flange's own: drawn on a plane turned about the edge to
        // where SolidWorks' profile sketch is. The flange is made as wide and as long as it reaches and cut
        // to it, and follows it.
        // A piece of a flange on several edges is named after it, numbered; with a space where SolidWorks
        // has a feature of that name already (Edge-Flange1 on two edges beside an Edge-Flange11)
        const pieceName = i => ops.length === 1 ? f.name : byName.has(f.name + (i + 1)) ? `${f.name} ${i + 1}` : f.name + (i + 1);
        const outlined = [];
        ops.forEach((o, i) => {
          if (!o.outline || rectangular.has(f.name)) return;
          const piece = pieceName(i);
          const sf = frameOf(o.outline.sketch.sketch.modelToSketch);
          // Under SolidWorks' name for it, so that its dimensions have SolidWorks' addresses
          // (Depth@Sketch13), unless something has that name already
          const skName = peet.ask({ op: "feature", feature: o.outline.sketch.name }).ok ? piece + " outline" : o.outline.sketch.name;
          // On a face of the part where SolidWorks' profile plane is (the end face along the edge, for a
          // flange with its material outside), as SolidWorks has it: its origin is the model's, so the
          // outline stays where it is drawn when another configuration moves the edge. A plane turned
          // about the edge moves its origin with the edge, and the outline's distances can then solve to
          // the wrong side.
          const middle = mid(o.found.from, o.found.to).map(tidy);
          const face = [sf.n, mul(sf.n, -1)].map(n => ({ at: middle, normal: n.map(tidy) })).find(sel => {
            const p = probe(sel);
            return p && Math.abs(dot(p.n, sf.n)) > 0.9999 && Math.abs(dot(sub(p.o, sf.o), sf.n)) < 1e-4;
          });
          if (face) {
            const sk = importSketch(o.outline.sketch, { name: skName, onFace: face });
            if (!sk.error) { o.native = { plane: null, sketch: sk.name, sk }; outlined.push(sk.name); return; }
          }
          const plane = piece + " outline plane";
          if (!planeAbout(plane, { between: [o.found.from.map(tidy), o.found.to.map(tidy)] }, sf)) return;
          const sk = importSketch(o.outline.sketch, { name: skName, onFace: plane });
          if (sk.error) { peet.apply({ op: "delete", features: [plane, plane + " turned"] }); return; }
          o.native = { plane, sketch: sk.name, sk };
          outlined.push(plane, plane + " turned", sk.name);
        });
        const withOutline = o => o.native ? { outline: o.native.sketch } : {};
        let built;
        if (ops.length === 1) built = buildFlange(f, { ...common, edge: ops[0].edge, ...ops[0].offsets, ...withOutline(ops[0]) }, f.name, null, ops[0].known);
        else {
          built = { ok: true, names: [] };
          ops.forEach((o, i) => {
            if (!built.ok) return;
            const one = buildFlange(f, { ...common, name: pieceName(i), edge: o.edge, ...o.offsets, ...withOutline(o) }, f.name, null, o.known);
            if (!one.ok) built = one; else built.names.push(...one.names);
          });
          // A piece that can't be built takes the ones built before it away with it
          if (!built.ok) for (let i = ops.length - 1; i >= 0; i--) if (peet.ask({ op: "feature", feature: pieceName(i) }).ok) peet.apply({ op: "delete", feature: pieceName(i) });
        }
        // An outline that cuts the sheet apart is not where the flange is (SolidWorks draws some so)
        if (built.ok && outlined.length && /separate pieces/.test(built.warning || "")) {
          peet.apply({ op: "delete", features: built.names });
          built = { ok: false, message: built.warning };
        }
        if (!built.ok && outlined.length) {
          // The outline could not be the flange's own: a rectangle, cut to it afterwards
          peet.apply({ op: "delete", features: outlined.filter(n => (peet.ask({ op: "feature", feature: n }).ok)) });
          rectangular.add(f.name);
          return handleLive(f);
        }
        if (built.ok && outlined.length) {
          built.names.push(...outlined.filter(n => peet.ask({ op: "feature", feature: n }).ok));
          // Its dimensions take each configuration's values, as a sketch's do
          for (const o of ops) if (o.native && !sketches.has(o.outline.sketch.name)) sketches.set(o.outline.sketch.name, o.native.sk);
          // Made to its outline, so the stand-in's notes do not apply
          for (let k = notes.length - 1; k >= 0; k--) if (/its outline reaches further/.test(notes[k])) notes.splice(k, 1);
          if (!ops.some(o => o.outline && !o.native)) built.warning = notes.length ? [...new Set(notes)].join("; ") : built.warning;
        }
        // An outline that is not a rectangle and could not be the flange's own: the flange is cut to it
        if (built.ok) ops.forEach((o, i) => {
          if (!o.outline || o.native) return;
          const cut = cutToOutline(f, pieceName(i) + " outline", o.outline);
          if (cut.name) { built.names.push(cut.sketch, cut.name); notes.push("its outline is not a rectangle: a rectangular flange is made and cut to the outline, which does not follow the part"); return; }
          notes.push(`its outline is not a rectangle: a rectangular flange is made (${cut.error})`);
          // Made longer for an outline it could not then be cut to: back to its own length
          if (o.offsets.length !== undefined) peet.apply({ op: "edit", feature: pieceName(i), length: tidy(lengthOf(i)) });
        });
        if (built.ok && notes.length) built.warning = [...new Set(notes)].join("; ");
        const gap = (v.GapDistance || 0) * MM;
        if (built.ok) {
          // The flanges themselves, without the planes, sketches and cuts their outlines brought
          const pieces = ops.map((o, i) => pieceName(i));
          const flangeNames = built.names.filter(n => pieces.includes(n));
          const others = cornersWithOthers(f.name, flangeNames, radius);
          if (others) built.names.push(others);
          if (ops.length === edges.length && edges.length > 1 && pieces.every(n => flangeNames.includes(n))) {
            built.names.push(...corners(f.name + " corners", gap, edges.map((e, i) => ({ name: pieces[i], start: mm(e.start), end: mm(e.end) })), radius));
          }
        }
        if (built.ok) flanges.set(f.name, { name: f.name, common, gap, pieces: edges.map((e, i) => ({ start: mm(e.start), end: mm(e.end), offsets: ops[i].offsets, outline: ops[i].outline })) });
        return finish(f, built);
      }

      case "SMMiteredFlange": {
        // A profile run along a chain of edges. PeetCAD's profile starts at the top or bottom corner of the
        // edge and is that face of the sheet carried on, measured to the virtual sharps: SolidWorks' is
        // drawn the same way, from the edge's end, on a plane square to it.
        const edges = (v.Edges || []).filter(e => e && e.start && e.end);
        if (edges.length === 0) return record(f, "failed", "its edges were not exported");
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const found = edges.map(e => findEdge(mm(e.start), mm(e.end)));
        if (found.some(e => !e)) return record(f, "failed", `no edge of the part is within ${NEAR} mm of where SolidWorks has the flange's edge`);
        if (v.UseDefaultBendRadius === false && sheet && Math.abs(v.BendRadius - sheet.BendRadius) > 1e-9) return record(f, "not supported", "a mitre flange with a bend radius of its own");
        if (v.UseDefaultBendAllowance === false) return record(f, "not supported", "a mitre flange with a bend allowance of its own");
        const op = {
          op: "miter_flange", name: f.name, sketch: sk.name, edges: found.map(e => ({ at: e.middle.map(tidy) })),
          gap: tidy((v.GapDistance || 0) * MM),
          ...(v.StartOffset ? { offset_start: tidy(v.StartOffset * MM) } : {}), ...(v.EndOffset ? { offset_end: tidy(v.EndOffset * MM) } : {}),
        };
        // Which side of the profile the sheet is on: PeetCAD's follows from the corner of the edge the
        // profile starts at, SolidWorks' from the flange's position. The profile is tried as drawn and
        // moved a thickness each way across the sketch, so that it starts at the edge's other corner, and
        // the one kept whose flat faces come nearest SolidWorks'.
        const source = sketchOf(f);
        const lines = source.sketch.segments.filter(g => !g.construction);
        // Each shift moves the whole profile, or only one end of it: SolidWorks' profile can start at the
        // corner the sheet is not on, a thickness from PeetCAD's.
        const shifts = [[0, 0, null]];
        if (thickness && lines.every(g => g.type === "line" && g.start && g.end)) {
          const d = thickness / MM;
          const count = new Map();
          for (const g of lines) for (const p of [g.start, g.end]) count.set(spot(p), (count.get(spot(p)) || 0) + 1);
          const ends = [...count.entries()].filter(([, n]) => n === 1).map(([k]) => k);
          for (const only of [null, ...ends]) shifts.push([d, 0, only], [-d, 0, only], [0, d, only], [0, -d, only]);
        }
        const moved = ([dx, dy, only], name) => importSketch(source, {
          name,
          draw: toPeet => {
            const at = p => only && spot(p) !== only ? toPeet(p) : toPeet({ x: p.x + dx, y: p.y + dy });
            const items = lines.map((g, i) => ({ type: "line", from: at(g.start), to: at(g.end), as: "m" + i }));
            lines.forEach((g, i) => lines.forEach((h, j) => { if (j > i && spot(g.end) === spot(h.start)) items.push({ type: "coincident", of: [`m${i}.end`, `m${j}.start`] }); }));
            return items;
          },
        });
        const mark = peet.script.length;
        let best = null;
        for (const shift of shifts) {
          const s = shift === shifts[0] ? sk : moved(shift, f.name + " profile");
          if (s.error) continue;
          const tried = build(f, { ...op, sketch: s.name });
          if (tried.ok) {
            const off = flangeMismatch([[f.name, tried.names]]), planes = planesMatched(f.name, tried.names);
            if (!best || best.failed || planes > best.planes + 1e-6 || (planes > best.planes - 1e-6 && off < best.off - 1e-6)) best = { shift, off, planes };
            peet.apply({ op: "delete", features: tried.names });
          } else if (!best) best = { failed: tried };
          if (s !== sk) peet.apply({ op: "delete", feature: s.name });
          peet.script.length = mark;
          if (best && best.planes > 1 - 1e-6 && best.off < 1) break;
        }
        if (!best || best.failed) return finish(f, best ? best.failed : { ok: false, message: "it can't be built" });
        const s = best.shift === shifts[0] ? sk : moved(best.shift, f.name + " profile");
        const built = build(f, { ...op, sketch: s.name });
        if (built.ok) {
          if (s !== sk) built.names.push(s.name);
          if (best.planes < 1 - 1e-3) built.warning = `${(100 * (1 - best.planes)).toFixed(0)}% of its flat faces by area are in no plane PeetCAD's flange has`;
          const others = cornersWithOthers(f.name, built.names.filter(n => n !== s.name), (sheet ? sheet.BendRadius : 0) * MM);
          if (others) built.names.push(others);
          miters.set(f.name, { op, source, lines, shift: best.shift, edges });
          if (best.shift !== shifts[0]) report.notes.push(`${f.name}'s profile is moved a thickness across its sketch (${s.name}), so that the sheet is on the side of it that SolidWorks has.`);
        }
        return finish(f, built);
      }

      case "ICE": case "Cut": case "Extrusion": case "Boss": {
        // "ICE" is SolidWorks' type for boss-extrudes and cut-extrudes alike. The second export says which
        // (IsBossFeature). The first does not: there the name SolidWorks gave it decides, and a renamed one is
        // taken for a cut, the commoner of the two, and for a boss if nothing can be cut.
        const says = typeof v.IsBossFeature === "boolean";
        // A part's first extrude is a "base extrude", which IsBossFeature does not count as a boss
        const boss = f.type === "Extrusion" || f.type === "Boss" || (f.type === "ICE" && (says ? v.IsBossFeature || v.IsBaseExtrude === true : /^Boss|^Extrude/i.test(f.name)));
        const unsure = !says && f.type === "ICE" && !/^Boss|^Extrude|^Cut/i.test(f.name);
        // Walls drafted as they are extruded, which way the draft leans
        const drafting = v["GetDraftWhileExtruding(true)"] && v["GetDraftAngle(true)"] > 0
          ? { draft: tidy(v["GetDraftAngle(true)"] * 180 / Math.PI), ...(v["GetDraftOutward(true)"] ? { draft_outward: true } : {}) } : {};
        const extrude = () => {
          const sk = useSketch(f);
          if (sk.error) return { ok: false, message: sk.error };
          const end = v["GetEndCondition(true)"];
          if (![0, 3, 4, 5, 6].includes(end)) return { ok: false, unsupported: true, message: `an extrusion with end condition ${end}` };
          // Both ways by one depth each is a mid-plane extrusion twice as deep
          const evenly = v.BothDirections && end === 0 && v["GetEndCondition(false)"] === 0 && Math.abs(v["GetDepth(true)"] - v["GetDepth(false)"]) < 1e-12;
          if (v.BothDirections && end !== 6 && !evenly) return { ok: false, unsupported: true, message: "an extrusion in both directions" };
          const sf = frameOf(sketchOf(f).sketch.modelToSketch);
          let along = mul(sf.n, v.ReverseDirection ? -1 : 1);
          let depth = v["GetDepth(true)"] * MM;
          if (evenly && drafting.draft) return { ok: false, unsupported: true, message: "an extrusion drafted both ways" };
          if (evenly) return build(f, { op: "extrude", sketch: sk.name, name: f.name, depth: tidy(2 * depth), end: "mid_plane" });
          // Up to a flat face parallel to the sketch, which SolidWorks names: up to that face, as SolidWorks has it
          const target = upToFace(end, v, sf);
          if (target) {
            const built = build(f, { op: "extrude", sketch: sk.name, name: f.name, end: { up_to: target }, ...drafting });
            if (built.ok) return built;
          }
          const vertex = end === 5 && v["GetVertex(true)"] && v["GetVertex(true)"].point;
          if (vertex) {
            // Up to a vertex: as far as the vertex SolidWorks names is from the sketch's plane
            const level = dot(sub(mm(vertex), sf.o), sf.n);
            if (Math.abs(level) > 1e-6) {
              const built = build(f, { op: "extrude", sketch: sk.name, name: f.name, depth: tidy(Math.abs(level)), reverse: level * dot(sf.n, sk.frame.n) < 0, ...drafting });
              if (built.ok) built.warning = [built.warning, "it goes up to a vertex in SolidWorks: it is made as deep as the vertex is"].filter(Boolean).join("; ");
              return built;
            }
          }
          if ([3, 4, 5].includes(end)) {
            // Up to a vertex or a surface: as far as SolidWorks' faces of it go from the sketch's plane, on
            // the side they are on
            const levels = facesOf(f.name).filter(face => face.box).flatMap(face => [mm(face.box.slice(0, 3)), mm(face.box.slice(3))]).map(p => dot(sub(p, sf.o), sf.n));
            if (!levels.length) return { ok: false, unsupported: true, message: `an extrusion up to something (end condition ${end}) whose faces are not in SolidWorks' body` };
            const [lo, hi] = [Math.min(...levels), Math.max(...levels)];
            along = hi >= -lo ? sf.n : mul(sf.n, -1);
            depth = Math.max(hi, -lo);
          }
          const built = build(f, {
            op: "extrude", sketch: sk.name, name: f.name, depth: tidy(depth),
            ...(end === 6 ? { end: "mid_plane" } : {}), reverse: dot(along, sk.frame.n) < 0, ...drafting,
          });
          if (built.ok && [3, 4, 5].includes(end)) built.warning = [built.warning, `it goes up to something in SolidWorks (end condition ${end}): it is made as deep as it went there`].filter(Boolean).join("; ");
          return built;
        };
        if (boss) {
          const built = extrude();
          return built.unsupported ? record(f, "not supported", built.message) : finish(f, built);
        }
        const sheetPart = sheetBody();
        const end = v["GetEndCondition(true)"];
        const depth = (v["GetDepth(true)"] || 0) * MM;
        if (sheetPart) {
          if (end === 0 && !v.LinkToThickness && thickness && depth < thickness - 1e-6)
            return record(f, "not supported", `a cut ${tidy(depth)} mm deep into ${tidy(thickness)} mm sheet (an etch): sheet metal cuts go through`);
          // Cut while the part is unfolded: with PeetCAD's unfold, as SolidWorks cuts it, straight
          // through the part as it lies (through the folded part where the unfold is suppressed). Through
          // next goes through all of it, the cut starting in the sheet it ends in. With no unfold of
          // PeetCAD's: through the sheet, where it lies in the blank.
          let reach = unfolded && !unfolded.live ? null : reachOf(end, depth, v.BothDirections);
          if (unfolded && unfolded.live && reach && reach.next) reach = { end: "through_all", ...(reach.both ? { both: true } : {}) };
          const sk = useSketch(f);
          let built = sk.error ? { ok: false, message: sk.error } : sheetCut(f, sk.name, reach, sk.frame);
          if (!built.ok && sketchOf(f)) {
            // A sheet metal cut's sketch has to be on a face of the sheet. One on a plane is put on the face there.
            const moved = useSketch(f, { name: f.name + " sketch", needFace: true });
            if (!moved.error) built = sheetCut(f, moved.name, reach, moved.frame);
            if (!built.ok && !moved.error) peet.apply({ op: "delete", feature: moved.name });
          }
          return finish(f, built);
        }
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const target = upToFace(end, v, frameOf(sketchOf(f).sketch.modelToSketch));
        // The side of the sketch that is cut away: outside it where SolidWorks flips it
        const side = { ...drafting, ...(v.FlipSideToCut ? { flip_side: true } : {}) };
        const shape = { ...(end === 0 ? { end: "blind", depth: tidy(depth) } : end === 6 ? { end: "mid_plane", depth: tidy(depth) } : end === 2 ? { end: "through_next" } : target ? { end: { up_to: target } } : { end: "through_all" }), ...side };
        let built;
        // Which way it goes: the way that cuts where SolidWorks' cut did
        built = nearest(f, [false, true].map(reverse => ({ op: "cut", sketch: sk.name, name: f.name, ...shape, reverse })));
        let upTo = !!target;
        if (!built.ok && target) { upTo = false; built = nearest(f, [false, true].map(reverse => ({ op: "cut", sketch: sk.name, name: f.name, end: "through_all", ...side, reverse }))); }
        if (built.ok && ![0, 1, 2, 6, 9].includes(end) && !upTo) built.warning = `SolidWorks' end condition ${end} is cut through all`;
        if (!built.ok && unsure) {
          const added = extrude();
          if (added.ok) return finish(f, added);
        }
        return finish(f, built);
      }

      // SolidWorks unfolds the part, cuts it flat and folds it again. PeetCAD's sheet metal cut is made in
      // the flat pattern to begin with, so there is nothing to unfold: the cuts in between are drawn on
      // the face that stays put, where SolidWorks has them once the part is laid flat.
      case "CornerRelief": return cornerRelief(f);
      case "LibraryFeature": return libraryFeature(f);

      case "UnFold": {
        // SolidWorks lays the bends flat, the fixed face staying where it is; the features up to
        // the Fold are made on the flat part (and where Unfold and Fold are suppressed, on the
        // folded one)
        unfolded = { ...(v.FixedFace || {}) };
        const fixed = v.FixedFace && v.FixedFace.point && v.FixedFace.normal
          ? { at: mm(v.FixedFace.point).map(tidy), normal: v.FixedFace.normal.map(tidy) } : null;
        if (!fixed) return record(f, "failed", "SolidWorks' fixed face is not in the export");
        const bends = bendSelectors(f, v.Bends);
        if (bends.error) return record(f, "failed", bends.error);
        const built = build(f, { op: "unfold", name: f.name, fixed, ...(bends.list ? { bends: bends.list } : {}) });
        if (built.ok) unfolded.live = true;
        return finish(f, built);
      }
      case "Fold": {
        const live = unfolded && unfolded.live;
        unfolded = null;
        if (!live) return record(f, "failed", "the Unfold it folds back was not imported");
        // Every bend laid flat, as SolidWorks' folds do in the library; other bends are named
        const unfold = [...cur.features].reverse().find(g => g.type === "UnFold" && !g.suppressed && made.has(g.name));
        const same = unfold && sameBends(values(unfold).Bends, v.Bends);
        const bends = same ? {} : bendSelectors(f, v.Bends);
        if (bends.error) return record(f, "failed", bends.error);
        return finish(f, build(f, { op: "fold", name: f.name, ...(bends.list ? { bends: bends.list } : {}) }));
      }

      case "Revolution": case "RevCut": {
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const axis = v.Axis && v.Axis.kind === "sketchSegment" && v.Axis.id ? sk.ids.get(v.Axis.id.join(".")) : undefined;
        const angle = (v["GetRevolutionAngle(true)"] || 2 * Math.PI) * 180 / Math.PI;
        let built;
        for (const reverse of [false, true]) {
          // A boss that SolidWorks does not merge is a body of its own
          const apart = f.type === "Revolution" && v.Merge === false ? { operation: "new_body" } : {};
          built = build(f, { op: f.type === "RevCut" ? "cut_revolve" : "revolve", sketch: sk.name, name: f.name, ...apart, angle: tidy(angle), ...(axis !== undefined ? { axis: { line: axis } } : {}), ...(reverse ? { reverse } : {}) });
          if (built.ok || Math.abs(angle - 360) < 1e-6) break;
        }
        return finish(f, built);
      }

      case "Draft": {
        // Faces tilted about a neutral plane, picked by a point on each. Which way they lean is tried both
        // ways, and the one kept that puts the faces where SolidWorks has them.
        if (v.Type !== 0 || !v.NeutralPlane) return record(f, "not supported", "a draft other than about a neutral plane");
        const n = v.NeutralPlane;
        const neutral = n.kind === "feature" ? planes.get(n.name) : n.kind === "face" && n.surface === "plane" && n.point && n.normal ? { at: mm(n.point).map(tidy), normal: n.normal.map(tidy) } : null;
        if (!neutral) return record(f, "failed", "its neutral plane was not imported");
        const faces = (v.FacesToDraft || []).filter(x => x && x.point).map(x => ({ at: mm(x.point).map(tidy) }));
        if (!faces.length || faces.length !== (v.FacesToDraft || []).length) return record(f, "failed", "the faces it drafts were not exported");
        const angle = tidy(v.Angle * 180 / Math.PI);
        return finish(f, nearest(f, [false, true].map(flip => ({ op: "draft", name: f.name, faces, neutral, angle, flip }))));
      }

      case "ExtruThin": {
        // A thin extrude of circles is a tube: the same as extruding each circle with a second one a wall away
        const source = sketchOf(f);
        const circles = source && source.sketch && !source.sketch.error ? source.sketch.segments.filter(g => !g.construction) : [];
        if (!source || circles.length === 0 || circles.some(g => !(g.type === "arc" && g.circle))) return record(f, "not supported", "a thin extrude of something other than circles");
        if (v["GetEndCondition(true)"] !== 0 || v.BothDirections) return record(f, "not supported", "a thin extrude that is not blind in one direction");
        const wall = v["GetWallThickness(true)"] * MM;
        // ThinWallType: 0 one way, 1 the other way, 2 both ways about the sketch, 3 two different walls
        const [out, into] = v.ThinWallType === 0 ? [wall, 0] : v.ThinWallType === 1 ? [0, wall] : v.ThinWallType === 2 ? [wall / 2, wall / 2] : [wall, (v["GetWallThickness(false)"] || 0) * MM];
        const sk = importSketch(source, {
          name: f.name + " sketch",
          draw: toPeet => circles.flatMap(g => [{ type: "circle", center: toPeet(g.center), radius: tidy(g.radius * MM + out) }, { type: "circle", center: toPeet(g.center), radius: tidy(g.radius * MM - into) }].filter(c => c.radius > 1e-6)),
        });
        if (sk.error) return record(f, "failed", sk.error);
        const along = mul(frameOf(source.sketch.modelToSketch).n, v.ReverseDirection ? -1 : 1);
        const built = build(f, { op: "extrude", sketch: sk.name, name: f.name, depth: tidy(v["GetDepth(true)"] * MM), reverse: dot(along, sk.frame.n) < 0 });
        if (built.ok) built.warning = "as an extrude of each circle and a second circle a wall away";
        return finish(f, built);
      }

      case "BreakCorner": case "Fillet": {
        if (!sheetBody()) {
          // On a solid: PeetCAD's fillet, on the edges SolidWorks rounded. An edge is picked by its ends, a
          // round one by a point on it.
          if (f.type === "Fillet" && v.Type === 3) return fullRound(f);
          if (f.type !== "Fillet" || v.Type !== 0 || v.IsMultipleRadius || !(v.DefaultRadius > 0)) return record(f, "not supported", f.type === "Fillet" ? "a fillet that is not one radius along edges" : null);
          const picks = [];
          for (const e of v.Edges || []) {
            if (e && e.start && e.end) picks.push({ between: [mm(e.start).map(tidy), mm(e.end).map(tidy)] });
            else if (e && e.circle) {
              const centre = mm(e.circle.slice(0, 3)), axis = unit(e.circle.slice(3, 6)), radius = e.circle[6] * MM;
              const across = unit(cross(axis, Math.abs(axis[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0]));
              picks.push({ at: add(centre, mul(across, radius)).map(tidy) });
            } else return record(f, "not supported", "a fillet on an edge that was not exported");
          }
          if (picks.length === 0) return record(f, "not supported", "a fillet of faces or loops");
          return finish(f, build(f, { op: "fillet", name: f.name, size: tidy(v.DefaultRadius * MM), edges: picks, chain: v.PropagateToTangentFaces !== false }));
        }
        return roundCorners(f);
      }

      case "Chamfer": {
        if (sheetBody()) return record(f, "not supported", "a chamfer on sheet metal");
        // Type 1 is an angle and a distance, 2 two distances. PeetCAD's chamfer sets back by one size on both
        // faces, which is either at 45° or with the two distances the same.
        const dims = new Map((f.dimensions || []).map(d => [d.name, d.value]));
        const size = v.Type === 1 && Math.abs(v.EdgeChamferAngle - Math.PI / 4) < 1e-9 ? dims.get("D1")
          : v.Type === 2 && dims.has("D1") && Math.abs(dims.get("D1") - dims.get("D2")) < 1e-12 ? dims.get("D1") : null;
        if (!(size > 0)) return record(f, "not supported", "a chamfer whose set-backs differ, or at a vertex");
        const picks = [];
        for (const e of v.Edges || []) {
          if (e && e.start && e.end) picks.push({ between: [mm(e.start).map(tidy), mm(e.end).map(tidy)] });
          else if (e && e.circle) {
            const centre = mm(e.circle.slice(0, 3)), axis = unit(e.circle.slice(3, 6)), radius = e.circle[6] * MM;
            picks.push({ at: add(centre, mul(unit(cross(axis, Math.abs(axis[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0])), radius)).map(tidy) });
          } else return record(f, "not supported", "a chamfer on an edge that was not exported");
        }
        // A face chamfers every edge round it: the edges of the part that lie on it
        for (const face of v.Faces || []) {
          if (!face || face.surface !== "plane" || !face.normal || !face.point) return record(f, "not supported", "a chamfer of a face that is not flat");
          const lo = mm(face.box.slice(0, 3)), hi = mm(face.box.slice(3)), o = mm(face.point), n = unit(face.normal);
          const on = p => Math.abs(dot(sub(p, o), n)) < 1e-3 && p.every((x, i) => x > lo[i] - 1e-3 && x < hi[i] + 1e-3);
          for (const e of peet.ask({ op: "edges" }).edges || []) {
            if ([e.from, e.middle, e.to].every(on)) picks.push(e.curve === "line" ? { between: [e.from, e.to] } : { at: e.middle });
          }
        }
        if (picks.length === 0) return record(f, "failed", "no edge of the part is where SolidWorks' chamfer is");
        return finish(f, build(f, { op: "chamfer", name: f.name, size: tidy(size * MM), edges: picks, chain: v.TangentPropagation !== false }));
      }

      case "RefAxis": {
        // The export has an axis's kind and no more; what it is on is read off what it was made from.
        // Type 3 is the axis of a round face, 0 a line.
        const parents = (f.parents || []).map(p => byName.get(p)).filter(Boolean);
        if (v.Type === 3) {
          const from = parents.find(p => made.has(p.name));
          const names = from ? made.get(from.name) : [];
          const round = (peet.ask({ op: "faces" }).faces || []).filter(x => x.axis && (x.made_by || []).some(m => names.includes(m.feature)));
          const line = x => [x.axis, x.center];
          if (!round.length || round.some(x => { const [a, c] = line(x), [a0, c0] = line(round[0]); return Math.abs(Math.abs(dot(a, a0)) - 1) > 1e-9 || len(cross(sub(c, c0), a0)) > 1e-6; }))
            return record(f, "not supported", "an axis of a round face, where what it was made from has no one axis");
          // Picked by what made it where that is the one face, which follows the part
          const by = (round[0].made_by || [])[0];
          const all = peet.ask({ op: "faces" }).faces || [];
          const unique = by && all.filter(x => (x.made_by || []).some(m => m.feature === by.feature && m.side === by.side)).length === 1;
          return finish(f, build(f, { op: "axis", name: f.name, face: unique ? { feature: by.feature, side: by.side } : { body: round[0].body, index: round[0].index } }));
        }
        if (v.Type === 0) {
          // The line a revolve of its sketch turns about, on two of the model's planes
          const sketch = parents.find(p => SKETCH.test(p.type));
          const axisOf = r => (values(r).Axis || {});
          const used = sketch && [...byName.values()].filter(r => /Revolution|RevCut/.test(r.type) && axisOf(r).sketch === sketch.name).map(axisOf);
          const g = used && used[0] && used[0].start && used[0].end ? used[0] : null;
          if (g) {
            const sf = frameOf(sketch.sketch.modelToSketch);
            const p0 = fromSketch(sf, g.start), d = unit(sub(fromSketch(sf, g.end), p0));
            const on = [["top", [0, 0, 1]], ["front", [0, 1, 0]], ["right", [1, 0, 0]]].filter(([, n]) => Math.abs(dot(n, d)) < 1e-9 && Math.abs(dot(n, p0)) < 1e-6).map(([name]) => name);
            if (on.length >= 2) return finish(f, build(f, { op: "axis", name: f.name, a: on[0], b: on[1] }));
          }
          return record(f, "not supported", "an axis along a line that is not on two of the model's planes");
        }
        return record(f, "not supported", `an axis of kind ${v.Type}`);
      }

      case "SolidToSheetMetal": {
        // The solid becomes sheet metal of the thickness, bend radius and bend model of the sheet metal
        // feature that goes with it, hanging from its fixed face into the solid
        const settings = (f.parents || []).map(p => byName.get(p)).find(p => p && p.type === "SheetMetal");
        const sv = settings ? values(settings) : {};
        const fixed = sv.FixedReference;
        if (!fixed || fixed.surface !== "plane" || !fixed.normal || !fixed.point) return record(f, "not supported", "a conversion whose fixed face is not a flat face");
        if (!sheet) sheet = sv;
        thickness = sv.Thickness * MM;
        const op = {
          op: "convert_to_sheet", name: f.name, face: { at: mm(fixed.point).map(tidy), normal: fixed.normal.map(tidy) },
          bend: bendOf(sv), thickness: tidy(thickness),
        };
        return finish(f, build(f, op));
      }

      case "Sweep": case "SweepCut": {
        // A profile sketch carried along a path sketch, which SolidWorks names
        const own = name => {
          const source = name && byName.get(name);
          if (!source || !SKETCH.test(source.type)) return { error: "a sweep of something other than a sketch, or along it" };
          if (sketches.has(source.name)) return sketches.get(source.name);
          const imported = importSketch(source);
          if (!imported.error) sketches.set(source.name, imported);
          return imported;
        };
        const profile = own(v.Profile && v.Profile.name), along = own(v.Path && v.Path.name);
        if (profile.error || along.error) return record(f, "failed", profile.error || along.error);
        return finish(f, build(f, { op: f.type === "SweepCut" ? "cut_sweep" : "sweep", name: f.name, profile: profile.name, path: along.name }));
      }

      // A body split in pieces that are joined again later: PeetCAD keeps one body, and what is done in
      // between is done to all of it, which is the same where it stays within the piece it was made on
      case "Split": {
        const piece = b => b && typeof b.name === "string" && b.name.startsWith(f.name + "[");
        const combine = [...byName.values()].find(c => c.type === "CombineBodies" && !c.suppressed && (values(c).BodiesToCombine || []).some(piece));
        if (!combine) return record(f, "not supported", "a split into bodies that are not joined again");
        rejoined.set(combine.name, f.name);
        made.set(f.name, []);
        return record(f, "imported with a warning", `nothing to do: ${combine.name} joins its pieces again; what is between is made on the whole body`);
      }
      case "CombineBodies": {
        if (!rejoined.has(f.name)) return record(f, "not supported", "a combination of bodies");
        made.set(f.name, []);
        return record(f, "imported", `nothing to do: it joins again what ${rejoined.get(f.name)} split`);
      }

      case "HoleWzd": {
        // A slot no longer than it is wide is a round hole, as SolidWorks makes it
        const slotWidth = (v.HoleDiameter || v.ThruHoleDiameter || v.Diameter || 0) * MM;
        if (/slot/i.test(f.name) && !(v.Length * MM <= slotWidth + 1e-6)) return slots(f);
        const where = (f.sub || []).find(c => SKETCH.test(c.type) && c.sketch && !c.sketch.error && c.sketch.points.length);
        if (!where) return record(f, "failed", "the sketch of its positions was not exported");
        const wanted = new Set((v.GetSketchPoints || []).filter(p => p && p.id).map(p => p.id.join(".")));
        const points = where.sketch.points.filter(p => wanted.size === 0 || wanted.has(p.id.join(".")));
        const diameter = (v.ThruHoleDiameter || v.HoleDiameter || v.Diameter || 0) * MM;
        if (!diameter || points.length === 0) return record(f, "failed", "its size or its positions were not exported");
        if (where.sketch.is3D) return holesOn3D(f, v, points, diameter);
        if (sheetBody()) {
          // The hole's own sketch with its relations and dimensions, so that the holes go where another
          // configuration puts them (VIC CT Metering Panel's, left and right hinged); failing that, the circles
          let sk = { error: true };
          if (relations && (where.dimensions || []).length) {
            sk = importSketch(where, { name: f.name + " sketch", needFace: true, holes: { points, radius: diameter / 2 } });
            if (!sk.error) sketches.set(where.name, sk);
          }
          const draw = toPeet => points.map(p => ({ type: "circle", center: toPeet(p), radius: tidy(diameter / 2) }));
          if (sk.error) sk = importSketch(where, { name: f.name + " sketch", needFace: true, draw });
          if (sk.error) return record(f, "failed", sk.error);
          const reach = reachOf(v.EndCondition, (v.HoleDepth || v.ThruHoleDepth || 0) * MM, false);
          const built = sheetCut(f, sk.name, reach, sk.frame);
          if (!built.ok) peet.apply({ op: "delete", feature: sk.name });
          return finish(f, built);
        }
        const sk = importSketch(where, { name: f.name + " sketch", draw: toPeet => points.map(p => ({ type: "point", at: toPeet(p) })) });
        if (sk.error) return record(f, "failed", sk.error);
        let built;
        for (const reverse of [false, true]) {
          built = build(f, { op: "hole", sketch: sk.name, name: f.name, ...holeShape(f, v, diameter), reverse });
          if (built.ok) break;
        }
        if (!built.ok) peet.apply({ op: "delete", feature: sk.name });
        return finish(f, built);
      }

      case "SketchHole": {
        // SolidWorks' simple hole: a circle sketched on a face and cut to an end condition, as a cut-extrude is
        const before = report.features.length;
        handleLive({ ...f, type: "ICE", definition: { ...f.definition, values: { ...v, IsBossFeature: false, BothDirections: false, "GetEndCondition(true)": v.Type, "GetDepth(true)": v.Depth } } });
        for (const x of report.features.slice(before)) if (x.name === f.name) x.type = f.type;
        return;
      }

      case "CirPattern": {
        const seeds = [];
        for (const p of v.PatternFeatureArray || []) {
          if (unbuilt(p)) continue;
          if (!p || !made.has(p.name)) return record(f, "failed", `what it copies (${p ? p.name : "nothing"}) was not imported`);
          seeds.push(...made.get(p.name));
        }
        if (seeds.length === 0 || v.BodyPattern) return record(f, "not supported", "a pattern of faces or bodies");
        if (v.Direction2 || v.Symmetric) return record(f, "not supported", "a circular pattern in two directions");
        const count = v.TotalInstances;
        if (!(count > 1)) return record(f, "imported", "nothing to do: one instance");
        // Equal spacing: the angle is the whole span; otherwise the angle between copies
        const span = (v.EqualSpacing ? v.Spacing : v.Spacing * (count - 1)) * 180 / Math.PI;
        const axis = axisOf(f, v.Axis);
        if (!axis) return record(f, "failed", "its axis was not found");
        const extra = axis === f.name + " axis" ? [axis] : [];
        const op = flip => ({ op: "circular_pattern", name: f.name, features: seeds, axis, count, angle: tidy(span), flip });
        // All the way round, the way it goes makes no difference; otherwise the one nearest SolidWorks' is kept
        const built = Math.abs(span - 360) < 1e-6 ? build(f, op(!!v.ReverseDirection)) : nearest(f, [op(!!v.ReverseDirection), op(!v.ReverseDirection)]);
        if (built.ok) built.names = [...extra, ...built.names];
        else if (extra.length) peet.apply({ op: "delete", features: extra });
        return finish(f, built);
      }

      case "MoveCopyBody": {
        // Where SolidWorks put the bodies (by mates, or by a distance and angles) is read off its body: the
        // motion that takes the faces of the body here onto the faces of one of SolidWorks' bodies
        const bodies = (v.Bodies || []).filter(b => b && b.kind === "body");
        if (bodies.length !== 1) return record(f, "not supported", "a move of several bodies, or of none");
        // Placed by mates, and with none, it leaves the body where it is
        if (v.TransformType === 1 && !v.Copy && !(f.sub || []).some(x => /^Mate/.test(x.type))) {
          const source = bodyFeatures(bodies[0].name);
          if (!source) return record(f, "failed", `the body it moves (${bodies[0].name}) was not found`);
          moves.set(f.name, source);
          made.set(f.name, []);
          return record(f, "imported", "nothing to do: it has no mates to move the body by");
        }
        const picked = bodyPick(bodies[0].name);
        if (!picked) return record(f, "failed", `the body it moves (${bodies[0].name}) was not found`);
        const ours = oursToMatch(picked.body);
        let best = null;
        for (const b of cur.bodies || []) {
          const found = register(ours, theirsToMatch(b));
          // A copy is not where the original is
          const still = found && len(found.t) < 1e-6 && found.m.every((row, r) => row.every((x, c) => Math.abs(x - (r === c ? 1 : 0)) < 1e-9));
          if (found && !(v.Copy && still) && (!best || found.matched > best.matched)) best = found;
        }
        if (!best) return record(f, "failed", "where SolidWorks put the body could not be found from its faces");
        const [rx, ry, rz] = eulerXYZ(best.m), [dx, dy, dz] = best.t.map(tidy);
        const nothing = [rx, ry, rz, dx, dy, dz].every(x => Math.abs(x) < 1e-6);
        const source = bodyFeatures(bodies[0].name);
        if (nothing && !v.Copy) {
          moves.set(f.name, source);
          made.set(f.name, []);
          return record(f, "imported", "nothing to do: it leaves the body where it is");
        }
        const op = { op: "move_body", name: f.name, bodies: [picked.pick], dx, dy, dz, rx, ry, rz, ...(v.Copy ? { copy: true, copies: v.NumberOfCopies || 1 } : {}) };
        // Placed by mates: the same mates, solved from where they put it, so that it follows the part. Where
        // they can't be had, or don't all hold, it is moved where they put it (a stand-in).
        let mated = null, why = null;
        if (v.TransformType === 1 && !v.Copy) {
          const found = bodyMates(f, picked.body, best);
          why = found.why || null;
          if (found.mates) {
            const tried = build(f, { ...op, mates: found.mates });
            if (tried.ok && !tried.warning) mated = tried;
            else {
              why = tried.ok ? tried.warning : tried.message;
              if (tried.ok) peet.apply({ op: "delete", features: tried.names });
            }
          }
        }
        const built = mated || build(f, op);
        if (built.ok) {
          moves.set(f.name, v.Copy ? built.names : source);
          if (best.matched < best.of) built.warning = `${best.matched} of the body's ${best.of} flat and round faces are where SolidWorks has them`;
          if (v.TransformType === 1 && !mated) built.warning = (built.warning ? built.warning + "; " : "") + `SolidWorks places it by mates, which could not be used (${why || "copies"}): it is moved where they put it`;
        }
        return finish(f, built);
      }

      case "MirrorStock": case "Stock": {
        // Another part's bodies, mirrored for a mirror part: that part is imported from its own export first,
        // into a folder of this part's, and inserted from there
        const from = v.PathName ? String(v.PathName).split(/[\\/]/).pop() : null;
        const exported = from && doc.__file ? path.join(path.dirname(doc.__file), from + ".swx.json") : null;
        if (!exported || !fs.existsSync(exported)) return record(f, "failed", `the export of the part it inserts (${from || "unknown"}) was not found`);
        const folder = path.join(dir, doc.name + ".parts");
        fs.mkdirSync(folder, { recursive: true });
        const inner = JSON.parse(fs.readFileSync(exported, "utf8"));
        inner.__file = exported;
        const theirs = importPart(inner, folder);
        report.notes.push(`${f.name} inserts ${inner.name}, imported on its own first (${theirs.verdict}).`);
        let mirror = null;
        if (f.type === "MirrorStock") {
          // The transform is the reflection: its normal is the direction it turns round
          const t = v.GetTransform && v.GetTransform.data;
          if (!t) return record(f, "failed", "its mirror plane was not exported");
          const m = [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]];
          const n = [0, 1, 2].map(k => 1 - m[k][k]);
          const normal = unit([Math.sqrt(Math.max(0, n[0] / 2)), Math.sqrt(Math.max(0, n[1] / 2)), Math.sqrt(Math.max(0, n[2] / 2))]);
          const shift = mm([t[9], t[10], t[11]]);
          const pf = { n: normal, o: mul(normal, dot(shift, normal) / 2) };
          const k = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(normal, AXES[k])) - 1) < 1e-9);
          if (!k) return record(f, "not supported", "a mirror part in a plane that is not square to an axis");
          mirror = standardPlane(pf) || planeWhere(f.name + " plane", pf);
          if (!mirror) return record(f, "failed", "its mirror plane could not be made");
        }
        const file = path.relative(dir, path.join(folder, inner.name + ".peet")).split(path.sep).join("/");
        return finish(f, build(f, { op: "insert_part", name: f.name, file, ...(mirror ? { mirror } : {}) }));
      }

      case "WeldmentFeature":
        made.set(f.name, []);
        return record(f, "imported", "nothing to do: it marks the part as a weldment");

      case "WeldMemberFeat": {
        // A profile swept along sketch lines, a body for each line. The export has the lines (in model
        // coordinates) and the profile's sketch; not which group each line is in, so groups are read off the
        // lines as SolidWorks makes them: lines joined end to end at corners of two, or lines side by side
        const lines = (v.PathSegments || []).filter(g => g && g.type === "line" && g.start && g.end).map(g => ({ a: mm([g.start.x, g.start.y, g.start.z]), b: mm([g.end.x, g.end.y, g.end.z]) }));
        if (!lines.length || lines.length !== (v.PathSegments || []).length) return record(f, "not supported", "a structural member along something other than lines");
        const profile = (f.sub || []).find(x => SKETCH.test(x.type) && x.sketch && !x.sketch.error);
        const planeOf = (f.sub || []).find(x => x.type === "RefPlane" && x.planeTransform);
        if (!profile || !planeOf) return record(f, "failed", "its profile was not exported");
        const near = (p, q) => len(sub(p, q)) < 1e-6;
        const dirOf = l => unit(sub(l.b, l.a));
        const groups = [];
        for (const l of lines) {
          const g = groups[groups.length - 1];
          const ends = g ? g.flatMap(x => [x.a, x.b]) : [];
          const count = p => ends.filter(q => near(p, q)).length;
          const joins = g && [l.a, l.b].some(p => count(p) === 1);
          const beside = g && g.every(x => Math.abs(Math.abs(dot(dirOf(x), dirOf(l))) - 1) < 1e-9) && [l.a, l.b].every(p => count(p) === 0);
          if (g && (joins || beside)) g.push(l); else groups.push([l]);
        }
        // The profile: drawn on the standard plane nearest square to the first line, about the point the lines
        // go through, turned to where SolidWorks has it on that line
        const pf = planeFrameOf(planeOf.planeTransform);
        const d0 = dirOf(groups[0][0]);
        const std = [["top", [1, 0, 0], [0, 1, 0], [0, 0, 1]], ["front", [1, 0, 0], [0, 0, 1], [0, -1, 0]], ["right", [0, 1, 0], [0, 0, 1], [1, 0, 0]]]
          .sort((p, q) => Math.abs(dot(q[3], d0)) - Math.abs(dot(p[3], d0)))[0];
        const [stdName, xs, ys, ns] = std;
        // Where PeetCAD's turn takes the sketch's X: the smallest turn from its normal to the line
        const turnX = (() => {
          const c = dot(ns, d0);
          if (c < -1 + 1e-12) return mul(xs, -1 + 2 * dot(xs, ys) ** 2); // half a turn about Y
          const k = cross(ns, d0), s = len(k);
          if (s < 1e-12) return xs;
          const axis = mul(k, 1 / s), ang = Math.atan2(s, c);
          // Rodrigues
          return add(add(mul(xs, Math.cos(ang)), mul(cross(axis, xs), Math.sin(ang))), mul(axis, dot(axis, xs) * (1 - Math.cos(ang))));
        })();
        const swX = pf.x, swY = pf.y;
        if (Math.abs(dot(cross(swX, swY), d0) - 1) > 1e-6) return record(f, "not supported", "a profile turned over (mirrored) on its line");
        const rotate = tidy(Math.atan2(dot(cross(turnX, swX), d0), dot(turnX, swX)) * 180 / Math.PI);
        const at = v.LocateProfilePoint && v.LocateProfilePoint.x !== undefined ? v.LocateProfilePoint : { x: 0, y: 0 };
        const p2 = q => [tidy((q.x - at.x) * MM), tidy((q.y - at.y) * MM)];
        const items = [];
        for (const g of profile.sketch.segments) {
          if (g.construction) continue;
          if (g.type === "line" && g.start && g.end) items.push({ type: "line", from: p2(g.start), to: p2(g.end) });
          else if (g.type === "arc" && g.circle) items.push({ type: "circle", center: p2(g.center), radius: tidy(g.radius * MM) });
          else if (g.type === "arc") {
            const [s, e] = g.direction === 1 ? [g.start, g.end] : [g.end, g.start];
            items.push({ type: "arc", center: p2(g.center), start: p2(s), end: p2(e) });
          } else return record(f, "not supported", `a profile with a ${g.type}`);
        }
        const names = [];
        const undo = () => { if (names.length) peet.apply({ op: "delete", features: names.slice().reverse() }); };
        const profileName = f.name + " profile";
        if (!peet.apply({ op: "sketch", on: stdName, name: profileName, draw: items }).ok) return record(f, "failed", "its profile could not be drawn");
        names.push(profileName);
        // Each group's lines, in sketches on the planes they lie in; or, where SolidWorks' lines are a 3D
        // sketch's, that 3D sketch, with the group's lines its own and the rest construction lines (a copy
        // for each group after the first, whose dimensions follow the first's)
        const corner = v.ApplyCornerTreatment && v.CornerTreatmentType === 1 ? "miter" : "plain";
        let warning = null;
        const segs = v.PathSegments || [];
        const of3d = segs.length && segs.every(g => g.sketch && g.sketch === segs[0].sketch) && byName.get(segs[0].sketch);
        const sketch3d = of3d && of3d.sketch && of3d.sketch.is3D ? of3d : null;
        const segOf = l => segs.find(g => near(mm([g.start.x, g.start.y, g.start.z]), l.a) && near(mm([g.end.x, g.end.y, g.end.z]), l.b));
        for (const [gi, group] of groups.entries()) {
          const left = [...group], paths = [];
          if (sketch3d && group.every(segOf)) {
            const name = gi === 0 ? sketch3d.name : `${sketch3d.name} group${gi + 1}`;
            const made3d = import3dSketch(sketch3d, name, new Set(group.map(l => segOf(l).id.join(","))), gi === 0 ? null : sketch3d.name);
            if (!made3d.error) {
              names.push(name);
              paths.push(name);
              left.length = 0;
            } else if (gi === 0) warning = `its 3D sketch could not be made (${made3d.error}): its lines are drawn in sketches on their planes`;
          }
          while (left.length) {
            const l = left[0], d = dirOf(l);
            // A plane through the line and another line of the group not along it, or any plane through it
            const other = left.slice(1).flatMap(x => [x.a, x.b]).find(p => len(cross(sub(p, l.a), d)) > 1e-6);
            let n = other ? unit(cross(d, sub(other, l.a))) : unit(cross(d, Math.abs(d[2]) < 0.9 ? [0, 0, 1] : [1, 0, 0]));
            const onPlane = x => [x.a, x.b].every(p => Math.abs(dot(sub(p, l.a), n)) < 1e-6);
            const these = left.filter(onPlane);
            for (const x of these) left.splice(left.indexOf(x), 1);
            const sketchName = `${f.name} path${gi + 1}.${paths.length + 1}`;
            const planeName = planeWhere(sketchName + " plane", { n, o: mul(n, dot(l.a, n)) });
            if (!planeName) { undo(); return record(f, "not supported", "a structural member along lines on a plane that is not square to an axis"); }
            if (planeName !== "top" && planeName !== "front" && planeName !== "right") names.push(planeName);
            const pl = probe(planeName);
            const to2 = p => [tidy(dot(sub(p, pl.o), pl.x)), tidy(dot(sub(p, pl.o), pl.y))];
            if (!peet.apply({ op: "sketch", on: planeName, name: sketchName, draw: these.map(x => ({ type: "line", from: to2(x.a), to: to2(x.b) })) }).ok) { undo(); return record(f, "failed", "its path could not be drawn"); }
            names.push(sketchName);
            paths.push(sketchName);
          }
          const built = build(f, { op: "structural_member", name: groups.length === 1 ? f.name : `${f.name} group${gi + 1}`, profile: profileName, paths, corner, ...(rotate ? { rotate } : {}) });
          if (!built.ok) { undo(); return record(f, "failed", built.message); }
          names.push(...built.names);
          warning = warning || built.warning;
        }
        // SolidWorks' bodies are named after the feature, numbered: "<name>[1]" and so on
        made.set(f.name, names);
        return record(f, warning ? "imported with a warning" : "imported", [warning, groups.length > 1 ? `its lines in ${groups.length} groups, one structural member each, as SolidWorks groups them` : null].filter(Boolean).join("; ") || null);
      }

      case "LPattern": {
        const seeds = [];
        for (const p of v.PatternFeatureArray || []) {
          if (unbuilt(p)) continue;
          if (!p || !made.has(p.name)) return record(f, "failed", `what it copies (${p ? p.name : "nothing"}) was not imported`);
          seeds.push(...made.get(p.name));
        }
        if (seeds.length === 0) return record(f, "not supported", "a pattern of faces or bodies");
        // The way the copies go, read off SolidWorks' body: from a face of what is copied to the same face of
        // a copy, a whole number of spacings away. `along`, if known, only settles which way.
        const seen = (spacing, along) => {
          const originals = (v.PatternFeatureArray || []).flatMap(p => facesOf(p.name));
          const votes = new Map();
          for (const c of facesOf(f.name)) for (const o of originals) {
            if (c.surface !== o.surface || Math.abs(c.area - o.area) > 1e-9 + 1e-6 * o.area) continue;
            const delta = sub(mid(mm(c.box.slice(0, 3)), mm(c.box.slice(3))), mid(mm(o.box.slice(0, 3)), mm(o.box.slice(3))));
            const k = Math.round(len(delta) / spacing);
            if (k < 1 || Math.abs(len(delta) - k * spacing) > 1e-3) continue;
            const d = unit(delta);
            if (along && Math.abs(Math.abs(dot(d, along)) - 1) > 1e-6) continue;
            const key = d.map(x => x.toFixed(4)).join(",");
            votes.set(key, { d, n: ((votes.get(key) || {}).n || 0) + 1 });
          }
          const [best, next] = [...votes.values()].sort((a, b) => b.n - a.n);
          // As many copies one way as the other (seeds at both ends, each copied inwards): it doesn't say
          if (best && next && next.n === best.n && dot(next.d, best.d) < -1 + 1e-6) return null;
          return best ? best.d : null;
        };
        const direction = (axis, reversed, spacing, alone) => {
          let d = null;
          if (axis && axis.start && axis.end) {
            d = mul(unit(sub(mm(axis.end), mm(axis.start))), reversed ? -1 : 1);
            // Which way along an edge SolidWorks goes is not always from its start to its end: the way
            // its body has the copies settles it
            const way = seen(spacing * MM, d);
            if (way) d = way;
          } else if (axis && axis.kind === "face" && axis.normal) {
            // Along a flat face's normal
            d = mul(unit(axis.normal), reversed ? -1 : 1);
            const way = seen(spacing * MM, d);
            if (way) d = way;
          } else if (alone) d = seen(spacing * MM, null);
          if (!d) return null;
          const k = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(d, AXES[k])) - 1) < 1e-9);
          if (k) return { axis: k, flip: dot(d, AXES[k]) < 0, d };
          if (axis && axis.start && axis.end) return { axis: edgeSelector(axis), flip: dot(unit(sub(mm(axis.end), mm(axis.start))), d) < 0, d };
          // Any straight edge of the part that way
          const edge = (peet.ask({ op: "edges" }).edges || []).find(e => e.curve === "line" && len(sub(e.to, e.from)) > 1e-6 && Math.abs(Math.abs(dot(unit(sub(e.to, e.from)), d)) - 1) < 1e-9);
          return edge ? { axis: { between: [edge.from, edge.to] }, flip: dot(unit(sub(edge.to, edge.from)), d) < 0, d } : null;
        };
        const grid = (v.D2TotalInstances || 1) > 1;
        const first = direction(v.D1Axis, v.D1ReverseDirection, v.D1Spacing, !grid);
        if (!first) return record(f, "failed", "its direction was not exported");
        const op = { op: "linear_pattern", name: f.name, features: seeds, direction: first.axis, flip: first.flip, spacing: tidy(v.D1Spacing * MM), count: v.D1TotalInstances };
        // SolidWorks' "up to reference" (exports from the fifth on): the count follows the part
        const reach = upToReference(v, "D1", first.d);
        if (reach && reach.fields) { Object.assign(op, reach.fields); upToPatterns.add(f.name); }
        else if (reach) report.notes.push(`${f.name} runs up to a reference in SolidWorks; it is made with SolidWorks' count, which won't follow the part: ${reach.why}.`);
        if ((v.D2TotalInstances || 1) > 1) {
          const second = direction(v.D2Axis, v.D2ReverseDirection, v.D2Spacing, false);
          if (!second) return record(f, "failed", "its second direction was not exported");
          const along = { direction: second.axis, flip: second.flip, spacing: tidy(v.D2Spacing * MM), count: v.D2TotalInstances };
          // Only the seed copied the second way: two patterns of the seed, one each way
          if (v.D2PatternSeedOnly) {
            const one = v.D1TotalInstances > 1 ? build(f, op) : { ok: true, names: [] };
            if (!one.ok) return finish(f, one);
            const two = build(f, { op: "linear_pattern", name: f.name + " second direction", features: seeds, ...along });
            if (!two.ok && one.names.length) peet.apply({ op: "delete", features: one.names });
            return finish(f, two.ok ? { ok: true, names: [...one.names, ...two.names], warning: one.warning || two.warning } : two);
          }
          Object.assign(op, { direction2: along.direction, flip2: along.flip, spacing2: along.spacing, count2: along.count });
        }
        let built = build(f, op);
        if (!built.ok && reach && reach.fields) {
          // PeetCAD's part may not have the vertex or face SolidWorks runs up to where SolidWorks has it: a vertex
          // as far along the direction does as well. Failing that, SolidWorks' count.
          const level = reach.at && (peet.ask({ op: "edges" }).edges || []).flatMap(e => [e.from, e.to])
            .filter(p => p && Math.abs(dot(sub(p, reach.at), first.d)) < 1e-6).sort((a, b) => len(sub(a, reach.at)) - len(sub(b, reach.at)))[0];
          const tried = built.message;
          if (level) built = build(f, { ...op, up_to: { point: { at: level.map(tidy) } } });
          if (built.ok) report.notes.push(`${f.name} runs up to a vertex of PeetCAD's part as far along it as SolidWorks' reference (${tried}).`);
          else {
            for (const k of Object.keys(reach.fields)) delete op[k];
            upToPatterns.delete(f.name);
            built = build(f, op);
            report.notes.push(`${f.name} runs up to a reference in SolidWorks that PeetCAD's part doesn't have (${tried}); it is made with SolidWorks' count, which won't follow the part.`);
          }
        }
        return finish(f, built);
      }

      case "MirrorPattern": {
        const seeds = [], bent = [], mitred = [], sheetMetal = [];
        for (const p of v.PatternFeatureArray || []) {
          if (p && flanges.has(p.name)) { bent.push(flanges.get(p.name)); sheetMetal.push(p.name); continue; }
          if (p && miters.has(p.name)) { mitred.push(miters.get(p.name)); sheetMetal.push(p.name); continue; }
          if (unbuilt(p)) continue;
          if (!p || !made.has(p.name)) return record(f, "failed", `what it mirrors (${p ? p.name : "nothing"}) was not imported`);
          seeds.push(...made.get(p.name));
        }
        if (seeds.length + bent.length + mitred.length === 0) return record(f, "not supported", "a mirror of faces or bodies");
        const plane = v.Plane && planes.get(v.Plane.name);
        const where = v.Plane && planeFrames.get(v.Plane.name);
        if (!plane || !where) return record(f, "failed", "its mirror plane was not imported");
        const names = [];
        let warning = null, nativeFailed = null;
        // PeetCAD's mirror copies edge and mitre flanges, and the sheet metal cuts that shape their outlines,
        // as SolidWorks' does. Only if a copy can't be made is a mirrored flange made as a flange of its own on
        // the edge across the plane, which no longer follows the original.
        if (sheetMetal.length) {
          const kinds = new Set(["Edge flange", "Sheet metal cut", "Miter flange"]);
          const copied = sheetMetal.flatMap(n => made.get(n) || []).filter(n => kinds.has((peet.ask({ op: "feature", feature: n }).type || "")));
          const built = build(f, { op: "mirror", name: f.name, features: [...seeds, ...copied], plane });
          if (built.ok && !built.warning) {
            made.set(f.name, built.names);
            return record(f, "imported");
          }
          if (built.ok) peet.apply({ op: "delete", features: built.names });
          nativeFailed = built.ok ? built.warning : built.message;
        }
        const across = p => sub(p, mul(where.n, 2 * dot(sub(p, where.o), where.n)));
        const order = [...flanges.values()];
        bent.sort((a, b) => order.indexOf(a) - order.indexOf(b));
        const pieces = bent.flatMap(b => b.pieces.map(piece => ({ ...piece, common: b.common, gap: b.gap, alone: b.pieces.length === 1, original: middleInSolidWorks(b.name) })));
        for (const [i, p] of pieces.entries()) {
          const edge = findEdge(across(p.start), across(p.end));
          if (!edge) return record(f, names.length ? "imported in part" : "failed", "the edge across the mirror plane was not found for a flange it mirrors");
          const op = { ...p.common, name: pieces.length === 1 && !seeds.length ? f.name : `${f.name} flange${i + 1}`, edge: { at: edge.middle.map(tidy) }, ...p.offsets };
          const built = buildFlange(f, op, "", p.alone && p.original ? across(p.original) : null);
          if (!built.ok) return record(f, names.length ? "imported in part" : "failed", built.message);
          names.push(...built.names);
          warning = "its flanges are made as flanges of their own, which do not follow the originals" + (nativeFailed ? ` (PeetCAD's mirror: ${nativeFailed})` : "");
          if (p.outline) {
            const cut = cutToOutline(f, op.name + " outline", p.outline, across);
            if (cut.name) names.push(cut.sketch, cut.name);
            else warning += `; one is left a rectangle (${cut.error})`;
          }
        }
        // A mitre flange likewise: its profile, moved across the plane, along the edges across it
        for (const [i, m] of mitred.entries()) {
          const name = `${f.name} mitre${i + 1}`;
          const edges = m.edges.map(e => findEdge(across(mm(e.start)), across(mm(e.end))));
          if (edges.some(e => !e)) return record(f, names.length ? "imported in part" : "failed", "the edges across the mirror plane were not found for a mitre flange it mirrors");
          const sf = frameOf(m.source.sketch.modelToSketch);
          const [dx, dy, only] = m.shift;
          const model = p => across(fromSketch(sf, only && spot(p) !== only ? p : { x: p.x + dx, y: p.y + dy }));
          // On the face across the plane from the one the profile is drawn on, picked by a point inside it
          const on = m.source.sketch.on;
          const inside = on && on.point ? across(mm(on.point)) : model(m.lines[0].start);
          const normal = unit(sub(across(add(sf.o, sf.n)), across(sf.o)));
          const sk = sketchOnFace(name + " profile", inside, normal, flat => {
            const items = m.lines.map((g, k) => ({ type: "line", from: flat(model(g.start)), to: flat(model(g.end)), as: "m" + k }));
            m.lines.forEach((g, k) => m.lines.forEach((h, j) => { if (j > k && spot(g.end) === spot(h.start)) items.push({ type: "coincident", of: [`m${k}.end`, `m${j}.start`] }); }));
            return items;
          });
          if (sk.error) return record(f, names.length ? "imported in part" : "failed", "the mirrored mitre flange's profile: " + sk.error);
          const built = build(f, { ...m.op, name, sketch: sk.name, edges: edges.map(e => ({ at: e.middle.map(tidy) })) });
          if (!built.ok) { peet.apply({ op: "delete", feature: sk.name }); return record(f, names.length ? "imported in part" : "failed", built.message); }
          names.push(sk.name, ...built.names);
          const others = cornersWithOthers(f.name, built.names, (sheet ? sheet.BendRadius : 0) * MM);
          if (others) names.push(others);
          warning = "its flanges are made as flanges of their own, which do not follow the originals";
        }
        if (seeds.length) {
          const built = build(f, { op: "mirror", name: f.name, features: seeds, plane });
          if (!built.ok) return record(f, names.length ? "imported in part" : "failed", built.message);
          names.push(...built.names);
        }
        made.set(f.name, names);
        return record(f, warning ? "imported with a warning" : "imported", warning);
      }

      default:
        return record(f, "not supported", null);
    }
  }

  for (const f of config.features) handle(f);

  // ---- The STEP fallback ----
  // A part nothing could be built of (a weldment, a body SolidWorks imported, a mirrored part) is
  // SolidWorks' own body from its STEP file: the right shape, and nothing to drive.
  let fallback = null;
  if (stepDir && config.step && !(peet.ask({ op: "bodies" }).bodies || []).length) {
    const file = path.join(path.resolve(stepDir), config.step);
    const imported = fs.existsSync(file) && peet.apply({ op: "import_step", path: file });
    if (imported && imported.ok) {
      fallback = config.step;
      report.fallback = { step: config.step };
      report.notes.push(`Nothing could be built from SolidWorks' features, so the part is SolidWorks' own body from ${config.step}: it has the right shape and can't be driven.`);
      const left = (imported.warnings || []).filter(w => / was left out/.test(w));
      if (left.length) report.notes.push(`${left.length} of the STEP file's bodies could not be made valid solids and were left out: ${left.join(" ")}`);
      if (config.flatDxf) {
        const sheetMetal = peet.apply({ op: "convert_to_sheet" });
        report.fallback.sheet_metal = sheetMetal.ok ? true : sheetMetal.error;
      }
    }
  }

  // ---- Compare with SolidWorks ----
  const mass = peet.ask({ op: "mass" });
  const bodies = peet.ask({ op: "bodies" }).bodies || [];
  const total = (mass && mass.total) || {};
  const sw = config.bodies || [];
  let swVolume = sw.reduce((a, b) => a + b.volume, 0) * 1e9;
  const step = stepDir ? measureStep(stepDir, doc.name, config.name) : null;
  if (step) {
    report.notes.push(`The volume and box to match are SolidWorks' STEP body's, ${tidy(step.volume)} mm³; SolidWorks reported ${tidy(swVolume)}.`);
    swVolume = step.volume;
  }
  const swArea = sw.reduce((a, b) => a + b.area, 0) * 1e6;
  // SolidWorks' box is loose round a freeform face (by a tenth of a millimetre at the end of a slanted bend);
  // its STEP body's, measured as PeetCAD measures its own, is not, and nor is the exporter's tight box
  const swBox = step ? step.box : sw.length ? [0, 1, 2].map(i => Math.min(...sw.map(b => (b.tightBox || b.box)[i])) * MM).concat([3, 4, 5].map(i => Math.max(...sw.map(b => (b.tightBox || b.box)[i])) * MM)) : null;
  const box = total.min ? [...total.min, ...total.max] : null;
  const boxOff = box && swBox ? Math.max(...box.map((x, i) => Math.abs(x - swBox[i]))) : null;
  const volumeOff = total.volume_mm3 && swVolume ? Math.abs(total.volume_mm3 - swVolume) / swVolume : null;
  const flat = bodies.find(b => b.flat_size);
  let swFlat = config.flatDxf ? dxfSize(path.join(path.dirname(doc.__file), config.flatDxf)) : null;
  const sizeOff = (p, q) => Math.max(...[...p].sort((a, b) => a - b).map((x, i) => Math.abs(x - [...q].sort((a, b) => a - b)[i])));
  let flatOff = flat && swFlat ? sizeOff(flat.flat_size, swFlat) : null;
  // SolidWorks wrote some flat DXFs of the folded part seen square to two of its axes, not of its blank: the
  // drawing is the size of the part's box that way (as the box is measured, a hundredth or so loose round
  // a freeform face), and the part is bent (its box is thicker than the sheet)
  if (flatOff !== null && flatOff >= 1e-2 && thickness) {
    const seen = [swBox, box].filter(Boolean).some(b => {
      const size = [0, 1, 2].map(i => b[i + 3] - b[i]);
      return [[0, 1], [0, 2], [1, 2]].some(([i, j]) => sizeOff([size[i], size[j]], swFlat) < 2e-2 && size[3 - i - j] > thickness + 1e-2);
    });
    if (seen) { flatOff = null; report.notes.push("SolidWorks' flat DXF is a view of the folded part, not its blank: the flat size is not compared"); }
  }
  // SolidWorks draws some blanks turned in the DXF: measured square to the drawing's own edges, as PeetCAD
  // measures its blank, if that is nearer
  if (flatOff !== null && flatOff >= 1e-2) {
    const square = dxfSquareSize(path.join(path.dirname(doc.__file), config.flatDxf));
    if (square && sizeOff(flat.flat_size, square.size) < flatOff) {
      report.notes.push(`SolidWorks drew the blank turned by ${square.angle}° in its flat DXF (${swFlat.join(" × ")} as drawn): its size is measured square to its edges`);
      swFlat = square.size;
      flatOff = sizeOff(flat.flat_size, swFlat);
    }
  }
  report.solidworks = { bodies: sw.length, volume_mm3: +swVolume.toFixed(3), area_mm2: +swArea.toFixed(3), box: swBox && swBox.map(tidy), flat_size: swFlat };
  report.peetcad = { bodies: bodies.length, volume_mm3: total.volume_mm3, area_mm2: total.area_mm2, box, flat_size: flat && flat.flat_size, sheet_metal: bodies.some(b => b.sheet_metal) };
  report.difference = { volume: volumeOff, box_mm: boxOff, flat_mm: flatOff };
  const left = report.features.filter(x => x.result === "not supported" || x.result === "failed");
  report.failures = (peet.ask({ op: "status" }).failures || []).map(x => `${x.name}: ${x.message}`);
  report.verdict =
    bodies.length === 0 ? "nothing built"
      : volumeOff !== null && volumeOff < 1e-5 && boxOff < 1e-3 && (flatOff === null || flatOff < 1e-2) ? (left.length ? "matches, with features left out" : "matches")
        : !left.length && volumeOff !== null && volumeOff < 0.05 && boxOff < 1e-3 && (flatOff === null || flatOff < 1e-2) ? "same size and flat pattern, volume differs"
        : left.length ? "differs: features left out" : "differs";

  if (bodies.length) {
    // ---- The names SolidWorks' faces and edges have, which FabSys mates and sketches by ----
    const names = { wanted: 0, given: 0, missed: [] };
    for (const b of sw) {
      for (const face of b.faces.filter(x => x.name)) {
        names.wanted++;
        // A point on the face (the second export has one); failing that the middle of its box
        const at = (face.point ? mm(face.point) : mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)))).map(tidy);
        const tries = [...(face.normal ? [{ at, normal: face.normal.map(tidy) }] : []), { at }];
        if (tries.some(sel => peet.apply({ op: "name_face", face: sel, name: face.name }).ok)) names.given++; else names.missed.push(face.name);
      }
      for (const edge of b.namedEdges || []) {
        names.wanted++;
        const sel = edge.start && edge.end ? { at: mid(mm(edge.start), mm(edge.end)).map(tidy) } : null;
        if (sel && peet.apply({ op: "name_edge", edge: sel, name: edge.name }).ok) names.given++; else names.missed.push(edge.name);
      }
    }
    if (names.wanted) report.names = names;

    // ---- SolidWorks' names for the sizes of features ----
    nameDimensions();

    // ---- Equations: the ones in force ----
    // A global variable becomes a parameter. A dimension set by a formula, a sketch's or a feature's (a
    // flange's length, a pattern's spacing or count), gets the formula as its value. A formula reads and sets
    // dimensions by SolidWorks' own names, which the sketches' dimensions and the features' sizes have (see
    // nameDimensions). A feature suppressed by a condition gets a suppression rule. Each is kept only if the
    // part is still the same afterwards.
    const equations = { inForce: 0, imported: 0, left: {} };
    const ident = t => t.replace(/[^A-Za-z0-9_]/g, "_").replace(/^(?=\d)/, "_");
    // A global variable keeps SolidWorks' name ("Base Height"), which FabSys sets it by; expressions quote
    // one that is not a plain word, as SolidWorks does. One PeetCAD can't take (with '@' or '"') is made a word.
    const WORD = /^[A-Za-z_][A-Za-z0-9_]*$/;
    const globalName = t => (t = t.trim()) && !/["@]/.test(t) ? t : ident(t);
    const inExpression = name => WORD.test(name) ? name : `"${name}"`;
    const globals = new Map();
    const dimensionOf = ref => {
      const sk = sketches.get(ref.split("@")[1]);
      const full = sk && [...sk.dimensions.keys()].find(k => k.startsWith(ref + "@"));
      if (!full) return null;
      const source = ((byName.get(ref.split("@")[1]) || {}).dimensions || []).find(x => x.fullName === full);
      return { sk, full, peetName: sk.dimensions.get(full), value: sk.values.get(full), angular: !!source && source.valueType === 1, count: !!source && source.valueType === 2 };
    };
    const promote = ref => {
      if (promoted.has(ref)) return promoted.get(ref).parameter;
      const d = dimensionOf(ref);
      const feature = byName.get(ref.split("@")[1]), names = made.get(ref.split("@")[1]);
      if (!d && feature && names && feature.type === "LPattern") {
        const own = (feature.dimensions || []).find(x => x.fullName.startsWith(ref + "@"));
        if (!own) return null;
        const parameter = ident(ref.split("@").reverse().join("_")), count = own.valueType === 2;
        if (!peet.apply({ op: "set_parameter", name: parameter, value: count ? String(own.value) : tidy(own.value * MM) + "mm" }).ok) return null;
        if (!peet.apply({ op: "edit", feature: names[0], [count ? "count" : "spacing"]: parameter }).ok) return null;
        promoted.set(ref, { parameter, full: own.fullName, count });
        return parameter;
      }
      if (!d) return null;
      const parameter = ident(ref.split("@").reverse().join("_"));
      const value = d.angular ? String(tidy(d.value * 180 / Math.PI)) : d.count ? String(d.value) : tidy(d.value * MM) + "mm";
      if (!peet.apply({ op: "set_parameter", name: parameter, value }).ok) return null;
      if (!peet.apply({ op: "set_dimension", sketch: d.sk.name, name: d.peetName, value: parameter, configurations: "all" }).ok) return null;
      promoted.set(ref, { parameter, full: d.full, angular: d.angular, count: d.count });
      return parameter;
    };
    // The addresses PeetCAD has (NAME@FEATURE), asked for again after anything that names a dimension
    let addresses = null;
    const hasAddress = address => {
      if (!addresses) addresses = new Set((peet.ask({ op: "dimensions" }).dimensions || []).map(d => d.address));
      return addresses.has(address);
    };
    // SolidWorks' sheet metal sizes by the name of its sheet metal feature ("Bend Radius@Sheet-Metal") are the
    // base flange's, which has them in PeetCAD
    const sheetSize = ref => {
      const [short, owner] = ref.split("@");
      const field = { "Bend Radius": "radius", Thickness: "thickness" }[short];
      const base = [...made.entries()].find(([swName]) => (byName.get(swName) || {}).type === "SMBaseFlange");
      if (!field || !/^Sheet-Metal\d*$/.test(owner) || !base) return false;
      addresses = null;
      return peet.apply({ op: "name_dimension", feature: base[1][0], dimension: field, name: ref }).ok;
    };
    // A sketch that no feature uses, whose dimension an equation reads (a spacing kept in a sketch of its
    // own), is imported for it
    const sketchOfOwn = ref => {
      const owner = byName.get(ref.split("@")[1]);
      if (!owner || !SKETCH.test(owner.type) || sketches.has(owner.name) || owner.suppressed) return false;
      const imported = importSketch(owner);
      if (imported.error) return false;
      sketches.set(owner.name, imported);
      addresses = null;
      return hasAddress(ref);
    };
    // A SolidWorks sketch imported under another name (a hole wizard's placement sketch is the hole's own
    // sketch in PeetCAD): its dimension's address there
    const renamed = ref => {
      const [short, owner] = ref.split("@");
      const sk = sketches.get(owner);
      if (!sk || !sk.name || sk.name === owner) return null;
      const address = `${(sk.dimensions && sk.dimensions.get(ref)) || short}@${sk.name}`;
      return hasAddress(address) ? address : null;
    };
    // Global variables whose equation is disabled keep the value they had, which SolidWorks reads
    const disabledGlobals = new Map((config.equations || []).filter(e => e.disabled && e.globalVariable).map(e => [(/^\s*"([^"]+)"/.exec(e.expression) || [])[1], e.value]).filter(([n]) => n));
    const translate = text => {
      let failed = null;
      let out = text.replace(/"([^"]*)"/g, (whole, inner) => {
        const ref = inner.trim();
        if (/^(un)?suppressed$/i.test(ref)) return /^un/i.test(ref) ? "0" : "1";
        if (ref.includes("@")) {
          if (ref.split("@").length > 2) { failed = "it reads a value of another document"; return "0"; }
          if (!hasAddress(ref)) { const other = renamed(ref); if (other) return `"${other}"`; }
          if (!hasAddress(ref) && !sheetSize(ref) && !sketchOfOwn(ref)) { failed = "it reads a dimension that was not imported"; return "0"; }
          return `"${ref}"`;
        }
        if (!globals.has(ref) && disabledGlobals.has(ref) && peet.apply({ op: "set_parameter", name: globalName(ref), value: String(disabledGlobals.get(ref)) }).ok) globals.set(ref, inExpression(globalName(ref)));
        if (!globals.has(ref)) { failed = "it reads a variable that is not defined"; return "0"; }
        return globals.get(ref);
      });
      out = out.replace(/\b(un)?suppressed\b/gi, word => /^un/i.test(word) ? "0" : "1");
      // SolidWorks writes its comparisons spaced out: `< =`
      out = out.replace(/([<>])\s+(?=[=>])/g, "$1");
      return failed ? { failed } : { value: out.trim() };
    };
    const volumeNow = () => ((peet.ask({ op: "mass" }).total) || {}).volume_mm3;
    const steady = total.volume_mm3;
    // The sheet's thickness is "Thickness@Sheet-Metal" in SolidWorks, which FabSys sets on every sheet metal
    // part it configures (its ConfigureSheetMetal): the base flange's thickness has that address. "Thickness",
    // SolidWorks' own name for it in equations, reads it there. Without a base flange, "Thickness" is a
    // parameter.
    const sheetThickness = thickness && sheetSize("Thickness@Sheet-Metal");
    if (sheetThickness) globals.set("Thickness", "\"Thickness@Sheet-Metal\"");
    else if (thickness && (config.equations || []).some(e => !e.disabled && /"Thickness"/.test(e.expression))) {
      const base = [...made.entries()].find(([swName]) => (byName.get(swName) || {}).type === "SMBaseFlange");
      if (peet.apply({ op: "set_parameter", name: "Thickness", value: tidy(thickness) + "mm" }).ok) {
        globals.set("Thickness", "Thickness");
        if (base) peet.apply({ op: "edit", feature: base[1][0], thickness: "Thickness" });
      }
    }
    /** Imports one equation, kept if `same()` says the part is the same afterwards. Returns why not, or null. */
    const importEquation = (e, same) => {
      const m = /^\s*"([^"]+)"\s*=\s*([\s\S]*)$/.exec(e.expression);
      if (!m) return "not understood";
      const target = m[1].trim();
      const backup = peet.file + ".before", mark = peet.script.length;
      fs.copyFileSync(peet.file, backup);
      const knownPromoted = new Map(promoted), knownGlobals = new Map(globals), knownSketches = new Set(sketches.keys());
      const forget = () => { for (const [swName, sk] of [...sketches]) if (!knownSketches.has(swName)) { sketches.delete(swName); if (report.sketches) delete report.sketches[sk.name]; } };
      const undo = why => { fs.copyFileSync(backup, peet.file); peet.script.length = mark; addresses = null; forget(); promoted.clear(); for (const [k, v] of knownPromoted) promoted.set(k, v); globals.clear(); for (const [k, v] of knownGlobals) globals.set(k, v); return why; };
      const t = translate(m[2]);
      if (t.failed) return undo(t.failed);
      let ok = false, why = "refused";
      if (e.globalVariable) {
        // A variable with nothing after its `=` is 0 in SolidWorks
        ok = peet.apply({ op: "set_parameter", name: globalName(target), value: t.value || String(e.value || 0) }).ok;
        if (ok) globals.set(target, inExpression(globalName(target)));
      } else if (/suppressed/i.test(m[2])) {
        const names = made.get(target);
        if (!names) why = "it suppresses something that was not imported";
        else ok = names.every(n => peet.apply({ op: "set_suppression_expression", feature: n, value: t.value }).ok);
      } else if (target.split("@").length === 2) {
        if (hasAddress(target) || sheetSize(target)) {
          // SolidWorks' values are plain numbers in its units; PeetCAD's keep theirs. A count worked out from
          // lengths is a length here: in millimetres, it is the number SolidWorks has.
          const set = value => peet.apply({ op: "set", name: target, value, configurations: "all" });
          let reply = set(t.value);
          if (!reply.ok && /needs a (plain )?number, but the expression gives a length/.test(reply.error || "")) reply = set(`(${t.value}) / 1mm`);
          ok = reply.ok;
          // (by its full name, which the other configurations are compared by: a formula gives the value in every one)
          if (ok) formulas.add(([...byName.values()].flatMap(x => x.dimensions || []).find(x => x.fullName.startsWith(target + "@")) || {}).fullName || target);
        } else {
          // The feature the dimension belongs to, or whose own sketch it is in
          const owner = target.split("@")[1];
          const feature = byName.get(owner) && made.has(owner) ? owner : [...byName.values()].find(x => (x.sub || []).some(s => s.name === owner) && made.has(x.name));
          why = (sketches.get(owner) || {}).name === owner ? "it sets a sketch dimension that was not imported"
            : feature ? "it sets a size of a feature that has no match" : NOT_IMPORTED;
        }
      } else why = "it sets a value of another document";
      if (!ok) return undo(why);
      if (!same()) return undo("it changes the part: the translation does not mean what SolidWorks' equation does");
      return null;
    };
    const NOT_IMPORTED = "it sets a size of a feature that was not imported";
    const later = [];
    for (const e of config.equations || []) {
      if (e.disabled) continue;
      equations.inForce++;
      const why = importEquation(e, () => { const after = volumeNow(); return !steady || (after && Math.abs(after - steady) <= 1e-6 * steady); });
      if (!why) { equations.imported++; continue; }
      equations.left[why] = (equations.left[why] || 0) + 1;
      if (why === NOT_IMPORTED) later.push(e);
    }
    if (fs.existsSync(peet.file + ".before")) fs.unlinkSync(peet.file + ".before");
    // A disabled global variable is still there in SolidWorks, with the value it kept, and FabSys sets it by
    // name (its SetGlobalVariable finds an equation by its name, in force or not, and fails if there is none):
    // each is a parameter with that value
    const parameters = new Set((peet.ask({ op: "parameters" }).parameters || []).map(x => x.name));
    for (const [name, value] of disabledGlobals) {
      const n = globalName(name);
      if (globals.has(name) || parameters.has(n) || typeof value !== "number") continue;
      if (peet.apply({ op: "set_parameter", name: n, value: String(value) }).ok) { globals.set(name, inExpression(n)); parameters.add(n); }
    }
    if (equations.inForce) report.equations = equations;
    // One that sets a size of a feature imported later, for a configuration it is live in, is tried again
    // then: kept if no configuration changes
    equationsLater = () => {
      if (!later.length) return;
      const shown = peet.ask({ op: "status" }).configuration;
      const volumes = () => {
        const list = (peet.ask({ op: "configurations" }).configurations || []).map(c => c.name);
        const out = list.map(name => peet.apply({ op: "configuration", configuration: name }).ok ? volumeNow() : null);
        peet.apply({ op: "configuration", configuration: shown });
        return out;
      };
      for (const e of later) {
        const before = volumes();
        const why = importEquation(e, () => volumes().every((v, i) => v === before[i] || (v && before[i] && Math.abs(v - before[i]) <= 1e-6 * before[i])));
        if (why) continue;
        equations.imported++;
        if (!--equations.left[NOT_IMPORTED]) delete equations.left[NOT_IMPORTED];
      }
      if (fs.existsSync(peet.file + ".before")) fs.unlinkSync(peet.file + ".before");
    };

    // ---- The other configurations ----
    // Each is added as a copy of the active one and made to agree with SolidWorks: the sketch dimensions
    // that differ, the features that are suppressed there, and the features that are live there and
    // suppressed in the active one, which are imported now, with that configuration shown.
    const others = fallback ? [] : doc.configurations.filter(c => c !== config && !c.flatPattern);
    if (others.length) {
      report.configurations = [];
      if (config.name !== "Default") peet.apply({ op: "edit_configuration", configuration: "Default", name: config.name });
      const all = [config, ...others];
      const maps = new Map(all.map(c => [c, flatten(c)]));
      const done = new Set();
      const entries = new Map();
      // Features imported for another configuration and suppressed at once in the active one, which the
      // configurations added after are copies of: SwName -> how many had been added then
      const early = new Map();
      const addedAt = new Map();
      /**
       * What SolidWorks lets differ between configurations besides sizes and suppression: a sketch's plane
       * (the Door's hinge holes, on the right-hand return in Default and the left-hand one in R Hinges) and a
       * cut's or extrusion's direction. A sketch goes on a plane or face where SolidWorks' is in `c`, one that
       * puts every point of it where SolidWorks has it there (its drawing is the same in every
       * configuration); a direction that is the other way round from the active configuration's is turned.
       */
      const settleSettings = (c, entry, there) => {
        const home = maps.get(config);
        // Looked at with this configuration shown
        const shown = peet.ask({ op: "status" }).configuration;
        if (shown !== c.name) peet.apply({ op: "configuration", configuration: c.name });
        for (const [swName, sk] of sketches) {
          const mine = there.get(swName), base = home.get(swName);
          if (!mine || !base || !mine.sketch || !base.sketch || mine.sketch.is3D || done.has(c.name + "|plane|" + swName)) continue;
          const sfC = frameOf(mine.sketch.modelToSketch), sfA = frameOf(base.sketch.modelToSketch);
          const pts = [...(base.sketch.points || []), ...(base.sketch.segments || []).flatMap(g => [g.start, g.end, g.center]).filter(Boolean)];
          const moved = pts.some(p => len(sub(fromSketch(sfC, p), fromSketch(sfA, p))) > 1e-4);
          if (!moved || !sk.frame || !sk.toPeet) continue;
          // One on a face that has moved here too is where SolidWorks has it already
          done.add(c.name + "|plane|" + swName);
          const now = (peet.ask({ op: "feature", feature: sk.name }) || {}).plane;
          if (now && Math.abs(Math.abs(dot(now.normal, sfC.n)) - 1) < 1e-6 && Math.abs(dot(sub(now.origin, sfC.o), sfC.n)) < 1e-4) continue;
          const want = pts.map(p => sk.toPeet(p));
          let placed = false;
          for (const candidate of placements(mine.sketch, sfC, false)) {
            const pf = probe(candidate);
            if (!pf || Math.abs(dot(pf.n, sfC.n)) < 0.9999 || Math.abs(dot(sub(pf.o, sfC.o), sfC.n)) > 1e-4) continue;
            const fits = pts.every((p, i) => { const d = sub(fromSketch(sfC, p), pf.o); return Math.abs(dot(d, pf.x) - want[i][0]) < 1e-4 && Math.abs(dot(d, pf.y) - want[i][1]) < 1e-4; });
            if (!fits) continue;
            placed = peet.apply({ op: "edit", feature: sk.name, on: candidate, configurations: [c.name] }).ok;
            if (placed) break;
          }
          // Failing a face, a plane where SolidWorks' sketch is here, facing the way the sketch's plane
          // faces in the active configuration (a face there faces the other way: the Door's other return):
          // an offset of a standard plane, or of one turned right round
          const axis = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(sfC.n, AXES[k])) - 1) < 1e-9);
          if (!placed && axis) {
            const name = `${sk.name} plane in ${c.name}`, from = { x: "right", y: "front", z: "top" }[axis];
            const about = { x: "y", y: "z", z: "x" }[axis], turned = name + " turned";
            const distance = tidy(Math.abs(dot(sfC.o, AXES[axis])));
            const fitsOn = pf => pf && pts.every((p, i) => { const d = sub(fromSketch(sfC, p), pf.o); return Math.abs(dot(d, pf.n)) < 1e-4 && Math.abs(dot(d, pf.x) - want[i][0]) < 1e-4 && Math.abs(dot(d, pf.y) - want[i][1]) < 1e-4; });
            for (const turn of [false, true]) {
              if (turn && !peet.apply({ op: "plane", from, about, angle: 180, name: turned }).ok) break;
              for (const flip of [false, true]) {
                if (!peet.apply({ op: "plane", from: turn ? turned : from, distance, flip, name }).ok) continue;
                if (fitsOn(probe(name))) {
                  const made = turn ? [turned, name] : [name];
                  if (made.every(n => peet.apply({ op: "move", feature: n, before: sk.name }).ok)
                    && peet.apply({ op: "edit", feature: sk.name, on: name, configurations: [c.name] }).ok) { placed = true; break; }
                }
                peet.apply({ op: "delete", feature: name });
              }
              if (placed) break;
              if (turn) peet.apply({ op: "delete", feature: turned });
            }
          }
          if (placed) { if (entry) entry.dimensions++; }
          else if (entry) entry.missing.push(`${swName}'s plane, which is elsewhere here`);
        }
        for (const [swName, names] of made) {
          const mine = there.get(swName), base = home.get(swName);
          if (!mine || !base || !/^(ICE|Cut|Extrusion|Boss)$/.test(mine.type) || done.has(c.name + "|reverse|" + swName)) continue;
          const [a, b] = [values(mine), values(base)];
          if (typeof a.ReverseDirection !== "boolean" || a.ReverseDirection === b.ReverseDirection) continue;
          // The way it goes is its sketch's normal, turned round if reversed: the same both ways
          // where the sketch faces the other way here too
          const sketchIn = (f, map) => (f.sub || []).find(x => SKETCH.test(x.type)) || (f.parents || []).map(n => map.get(n)).find(x => x && SKETCH.test(x.type));
          const [ska, skb] = [sketchIn(mine, there), sketchIn(base, home)];
          if (ska && skb && ska.sketch && skb.sketch && dot(frameOf(ska.sketch.modelToSketch).n, frameOf(skb.sketch.modelToSketch).n) < 0) continue;
          done.add(c.name + "|reverse|" + swName);
          let turned = false;
          for (const n of names) {
            const f = peet.ask({ op: "feature", feature: n });
            if (!f.ok || !f.fields || typeof f.fields.reverse !== "boolean") continue;
            turned = peet.apply({ op: "edit", feature: n, reverse: !f.fields.reverse, configurations: [c.name] }).ok || turned;
          }
          if (turned) { if (entry) entry.dimensions++; }
          else if (entry) entry.missing.push(`${swName}'s direction, the other way here`);
        }
        if (shown !== c.name) peet.apply({ op: "configuration", configuration: shown });
      };
      /** Makes a configuration agree with SolidWorks' for what has been imported so far. */
      const settle = c => {
        const entry = entries.get(c), there = maps.get(c);
        for (const [swName, sk] of sketches) {
          const feature = there.get(swName);
          for (const d of (feature && feature.dimensions) || []) {
            const was = sk.values && sk.values.get(d.fullName), peetName = sk.dimensions && sk.dimensions.get(d.fullName);
            if (was === undefined || Math.abs(was - d.value) < 1e-10 || done.has(c.name + "|" + d.fullName)) continue;
            // A reference dimension (driven, and set by no equation) only measures what the rest makes
            const ref = d.fullName.split("@").slice(0, 2).join("@");
            if (d.drivenState === 1 && !d.readOnly && !peetName && !(c.equations || []).some(e => !e.disabled && e.expression.replace(/\s/g, "").startsWith(`"${ref}"=`))) continue;
            done.add(c.name + "|" + d.fullName);
            let value = tidy(d.valueType === 1 ? d.value * 180 / Math.PI : d.value * MM);
            if (sk.supplementary && sk.supplementary.has(d.fullName)) value = tidy(180 - value);
            // A formula gives the value in every configuration. A dimension that follows a parameter gets
            // its value through the parameter.
            if (formulas.has(d.fullName)) continue;
            const follows = [...promoted.values()].find(p => p.full === d.fullName);
            if (follows) {
              if (peet.apply({ op: "set_parameter", name: follows.parameter, value: follows.angular || follows.count ? String(value) : value + "mm", configurations: c.name }).ok) { if (entry) entry.dimensions++; }
              else if (entry) entry.missing.push(`${d.fullName.split("@").slice(0, 2).join("@")} = ${value}`);
              continue;
            }
            if (peetName && peet.apply({ op: "set_dimension", sketch: sk.name, name: peetName, value, configurations: c.name }).ok) { if (entry) entry.dimensions++; }
            else if (entry) entry.missing.push(`${d.fullName.split("@").slice(0, 2).join("@")} = ${value}`);
          }
        }
        if (c !== config) settleSettings(c, entry, there);
        for (const [swName, names] of made) {
          const feature = there.get(swName);
          if (!feature || done.has(c.name + "|suppress|" + swName)) continue;
          // What was imported asleep is suppressed everywhere, and woken where SolidWorks has it live. One
          // suppressed early in the active configuration is suppressed in the copies made of it since.
          const on = !!feature.suppressed;
          const copied = early.has(swName) && (c === config || addedAt.get(c) > early.get(swName));
          if (on === (asleep.has(swName) || copied)) continue;
          done.add(c.name + "|suppress|" + swName);
          if (names.every(n => peet.apply({ op: "suppress", feature: n, on, configurations: [c.name] }).ok)) { if (entry) entry.suppressed++; }
          else if (entry) entry.missing.push(`${swName} ${on ? "suppressed" : "brought back"}`);
        }
      };
      for (const other of others) {
        const entry = { name: other.name, dimensions: 0, suppressed: 0, added: 0, missing: [] };
        report.configurations.push(entry);
        entries.set(other, entry);
        if (!peet.apply({ op: "add_configuration", name: other.name, copy: config.name }).ok) { entry.verdict = "could not be added"; entries.delete(other); continue; }
        addedAt.set(other, addedAt.size + 1);
        settle(other);
        // What is live here and was suppressed where it has been looked at so far
        cur = other;
        byName = maps.get(other);
        const before = report.features.length;
        const had = new Set(made.keys());
        // Where each SolidWorks feature is in the tree (one under another, where that one is)
        const order = new Map();
        other.features.forEach((top, i) => { const walk = g => { order.set(g.name, i); (g.sub || []).forEach(walk); }; walk(top); });
        const tree = () => (peet.ask({ op: "features" }).features || []).map(x => x.name);
        for (const f of other.features) {
          // A feature between an Unfold and its Fold is cut in the flat here too (Top Hat's Cut-Extrude2)
          if (!f.suppressed && f.type === "UnFold") unfolded = { ...(values(f).FixedFace || {}), live: made.has(f.name) };
          if (!f.suppressed && f.type === "Fold") unfolded = null;
          if (f.suppressed || STRUCTURE.test(f.type) || f.type === "SheetMetal" || f.type === "RefPlane" || made.has(f.name) || sketches.has(f.name)) continue;
          if (report.features.some(x => x.name === f.name && x.result !== "suppressed" && x.result !== LATER)) continue;   // tried already, where it was live
          const was = new Set(tree());
          handle(f);
          // What it made goes where SolidWorks has it in the tree, not at the end: before what was made of
          // the features after it (Weather Shield Door's Cut-Extrude2, whose corners Break-Corner1 rounds)
          const added = tree().filter(n => !was.has(n));
          if (!added.length) continue;
          const after = new Set();
          for (const [swName, names] of made) if (order.get(swName) > order.get(f.name)) names.forEach(n => after.add(n));
          for (const [swName, sk] of sketches) if (order.get(swName) > order.get(f.name)) after.add(sk.name);
          const ahead = tree().find(n => after.has(n) && was.has(n));
          if (!ahead) continue;
          for (const n of added) if (!peet.apply({ op: "move", feature: n, before: ahead }).ok) break;
        }
        unfolded = null;
        // What was imported here is suppressed in the active configuration now, where SolidWorks has it
        // suppressed: the configurations still to come are copies of it, and are not rebuilt with it there
        // (Top Hat's Edge-Flange3, whose outline turns inside out where its edge is short)
        const home = maps.get(config);
        for (const [swName, names] of made) {
          const there = home.get(swName);
          if (had.has(swName) || asleep.has(swName) || !there || !there.suppressed) continue;
          if (names.every(n => peet.apply({ op: "suppress", feature: n, on: true, configurations: [config.name] }).ok)) {
            early.set(swName, addedAt.size);
            done.add(config.name + "|suppress|" + swName);
          }
        }
        entry.added = report.features.slice(before).filter(x => /^imported/.test(x.result) && !SKETCH.test(x.type)).length;
        for (const x of report.features.slice(before)) if (x.result === "failed" || x.result === "not supported") entry.missing.push(`${x.name}: ${x.result}`);
        // Sizes of features themselves (a flange's length, a pattern's count, a moved body's mate distance):
        // set in this configuration by SolidWorks' address where the part has it, else not carried over yet
        const here = maps.get(config);
        let known = null;
        for (const [swName, feature] of maps.get(other)) {
          const base = here.get(swName);
          if (!base || feature.suppressed || base.suppressed || SKETCH.test(feature.type)) continue;
          for (const d of feature.dimensions || []) {
            const was = (base.dimensions || []).find(x => x.fullName === d.fullName);
            const [short, owner] = d.fullName.split("@");
            const own = owner === swName || (feature.sub || []).some(x => x.name === owner && !SKETCH.test(x.type));
            if (!was || Math.abs(was.value - d.value) <= 1e-10 || !own || sketches.has(owner)) continue;
            const address = `${short}@${owner}`;
            if (!known) known = new Set((peet.ask({ op: "dimensions" }).dimensions || []).map(x => x.address));
            const value = d.valueType === 1 ? tidy(d.value * 180 / Math.PI) : d.valueType === 2 ? d.value : tidy(d.value * MM);
            if (known.has(address) && !formulas.has(d.fullName) && peet.apply({ op: "set", name: address, value: String(value), configurations: [other.name] }).ok) entry.dimensions++;
            else if (owner === swName) entry.missing.push(`${address}, a feature's own size`);
          }
        }
      }
      cur = config;
      byName = maps.get(config);
      // A linear pattern whose count differs between configurations (SolidWorks' "up to reference", whose
      // reference the export does not have) gets its count from a parameter, set in each configuration to
      // SolidWorks' count there
      for (const [swName] of made) {
        if (upToPatterns.has(swName)) continue;
        const counts = all.filter(c => c === config || entries.has(c)).map(c => {
          const x = maps.get(c).get(swName);
          const d = x && x.type === "LPattern" && (x.dimensions || []).find(d => d.valueType === 2 && d.fullName.split("@")[1] === swName);
          return d ? { c, d } : null;
        }).filter(Boolean);
        if (new Set(counts.map(k => k.d.value)).size < 2) continue;
        const parameter = promote(counts[0].d.fullName.split("@").slice(0, 2).join("@"));
        for (const { c, d } of counts) {
          const entry = entries.get(c);
          if (!parameter || !peet.apply({ op: "set_parameter", name: parameter, value: String(d.value), configurations: c.name }).ok) { if (entry) entry.missing.push(`${swName}'s count = ${d.value}`); }
          else if (entry) entry.dimensions++;
        }
      }
      // Features imported for a later configuration are suppressed where SolidWorks has them suppressed,
      // the active configuration included
      for (const c of all) if (c === config || entries.has(c)) settle(c);
      // Equations that set sizes of the features imported for other configurations
      nameDimensions(false);
      if (equationsLater) equationsLater();
      for (const other of others) {
        const entry = entries.get(other);
        if (!entry || !peet.apply({ op: "configuration", configuration: other.name }).ok) continue;
        const now = (peet.ask({ op: "mass" }).total) || {};
        const step = stepDir ? measureStep(stepDir, doc.name, other.name) : null;
        const volume = step ? step.volume : (other.bodies || []).reduce((sum, body) => sum + body.volume, 0) * 1e9;
        const theirBox = step ? step.box : (other.bodies || []).length ? [0, 1, 2].map(i => Math.min(...other.bodies.map(body => (body.tightBox || body.box)[i])) * MM).concat([3, 4, 5].map(i => Math.max(...other.bodies.map(body => (body.tightBox || body.box)[i])) * MM)) : null;
        const off = now.volume_mm3 && volume ? Math.abs(now.volume_mm3 - volume) / volume : null;
        const boxDiff = now.min && theirBox ? Math.max(...[...now.min, ...now.max].map((x, i) => Math.abs(x - theirBox[i]))) : null;
        entry.difference = { volume: off, box_mm: boxDiff };
        entry.verdict = off !== null && off < 1e-5 && boxDiff < 1e-3 ? "matches" : off !== null && off < 0.05 && boxDiff < 1e-3 ? "same size, volume differs" : "differs";
        // A configuration with every feature suppressed has no body in SolidWorks: nor should it here
        if (!step && !(other.bodies || []).length) {
          entry.difference = { volume: now.volume_mm3 ? 1 : 0, box_mm: 0 };
          entry.verdict = now.volume_mm3 ? "differs" : "matches";
        }
        entry.missing = [...new Set(entry.missing)];
      }
      peet.apply({ op: "configuration", configuration: config.name });
      // The active configuration must still be what it was
      const after = (peet.ask({ op: "mass" }).total) || {};
      if (total.volume_mm3 && after.volume_mm3 && Math.abs(after.volume_mm3 - total.volume_mm3) > 1e-6 * total.volume_mm3) report.notes.push(`Adding the other configurations changed ${config.name}: its volume went from ${total.volume_mm3} to ${after.volume_mm3}.`);
    }
  }

  // The sizes of features imported for other configurations get SolidWorks' names too
  nameDimensions(false);
  fs.writeFileSync(path.join(dir, doc.name + ".jsonl"), peet.script.map(l => typeof l === "string" ? l : JSON.stringify(l)).join("\n") + "\n");
  fs.writeFileSync(path.join(dir, doc.name + ".report.json"), JSON.stringify(report, null, 1));
  return report;
}

/**
 * The volume and box of SolidWorks' STEP body for a configuration, as PeetCAD measures them, or null if there
 * is no such file or PeetCAD can't read it. The exporter writes `<Name>.SLDPRT.<configuration>.step`, with characters a
 * file name can't have as `_`, and the extension's case as the part's file has it.
 */
function measureStep(dir, name, configuration) {
  if (!fs.existsSync(dir)) return null;
  const want = `${name}.sldprt.${configuration.replace(/[<>:"/\\|?*]/g, "_")}.step`.toLowerCase();
  const file = fs.readdirSync(dir).find(f => f.toLowerCase() === want);
  if (!file) return null;
  const reply = spawnSync(peetExe, ["op", JSON.stringify({ op: "import_step", path: path.join(dir, file) }), JSON.stringify({ op: "mass" }), "--no-save", "-q"], { encoding: "utf8", maxBuffer: 1 << 28 });
  try {
    const lines = (reply.stdout || "").trim().split("\n");
    // A body PeetCAD had to leave out makes the file no measure of SolidWorks' part
    const imported = JSON.parse(lines[0]);
    if ((imported.warnings || []).some(w => / was left out/.test(w))) return null;
    const total = JSON.parse(lines.pop()).total;
    return total && total.volume_mm3 > 0 && total.min ? { volume: total.volume_mm3, box: [...total.min, ...total.max] } : null;
  } catch { return null; }
}

/** The size of a DXF drawing, from its header. */
function dxfSize(file) {
  if (!fs.existsSync(file)) return null;
  const lines = fs.readFileSync(file, "latin1").split(/\r?\n/).map(l => l.trim());
  const at = name => { const i = lines.indexOf(name); return i < 0 ? null : [parseFloat(lines[i + 2]), parseFloat(lines[i + 4])]; };
  const min = at("$EXTMIN"), max = at("$EXTMAX");
  return min && max ? [tidy(max[0] - min[0]), tidy(max[1] - min[1])] : null;
}

/**
 * The size of a DXF drawing measured square to its own edges, where SolidWorks drew the blank turned
 * (Peaked Angled Roof's, by the 3° its fixed face leans): the angle most of the length of its lines runs
 * at, modulo 90°, and the drawing's size turned back by it. Null if the drawing is square already.
 */
function dxfSquareSize(file) {
  if (!fs.existsSync(file)) return null;
  const lines = fs.readFileSync(file, "latin1").split(/\r?\n/).map(l => l.trim());
  const start = lines.indexOf("ENTITIES");
  if (start < 0) return null;
  const entities = [];
  for (let i = start + 1; i + 1 < lines.length; i += 2) {
    const [code, value] = [lines[i], lines[i + 1]];
    if (code === "0") { if (value === "ENDSEC") break; entities.push({ type: value, g: {} }); continue; }
    const e = entities[entities.length - 1];
    if (e) (e.g[code] = e.g[code] || []).push(parseFloat(value));
  }
  const points = [], length = new Map();
  for (const { type, g } of entities) {
    if (type === "LINE" && g["11"]) {
      const a = [g["10"][0], g["20"][0]], b = [g["11"][0], g["21"][0]];
      points.push(a, b);
      const angle = ((Math.atan2(b[1] - a[1], b[0] - a[0]) * 180 / Math.PI) % 90 + 90) % 90;
      const key = Math.round(angle * 1000) % 90000;
      length.set(key, (length.get(key) || 0) + Math.hypot(b[0] - a[0], b[1] - a[1]));
    } else if ((type === "ARC" || type === "CIRCLE") && g["40"]) {
      const [from, to] = type === "ARC" ? [g["50"][0], g["51"][0]] : [0, 360];
      const sweep = ((to - from) % 360 + 360) % 360 || 360;
      for (let k = 0; k <= 90; k++) {
        const t = (from + sweep * k / 90) * Math.PI / 180;
        points.push([g["10"][0] + g["40"][0] * Math.cos(t), g["20"][0] + g["40"][0] * Math.sin(t)]);
      }
    } else if (g["10"] && g["20"]) g["10"].forEach((x, k) => g["20"][k] !== undefined && points.push([x, g["20"][k]]));
  }
  const best = [...length].sort((a, b) => b[1] - a[1])[0];
  if (!best || best[0] === 0 || points.length === 0) return null;
  const angle = -best[0] / 1000 * Math.PI / 180, c = Math.cos(angle), s = Math.sin(angle);
  const turned = points.map(([x, y]) => [c * x - s * y, s * x + c * y]);
  const size = i => Math.max(...turned.map(p => p[i])) - Math.min(...turned.map(p => p[i]));
  const degrees = best[0] / 1000;
  return { angle: tidy(Math.min(degrees, 90 - degrees)), size: [tidy(size(0)), tidy(size(1))] };
}

// ---- Main ----

fs.mkdirSync(outDir, { recursive: true });
const files = fs.statSync(input).isDirectory()
  ? fs.readdirSync(input).filter(f => f.endsWith(".swx.json")).sort().map(f => path.join(input, f))
  : [input];
const summary = [];
const chosen = files.filter(file => !only || path.basename(file).toLowerCase().includes(only.toLowerCase()));
if (jobs > 1 && chosen.length > 1) {
  // Each part in a process of its own, then the reports they wrote are read back for the summary
  const { spawn } = require("child_process");
  const queue = [...chosen];
  const pass = [...(relations ? [] : ["--no-relations"]), "--peet", peetExe, ...(stepDir ? ["--step", stepDir] : [])];
  const next = () => {
    const file = queue.shift();
    if (!file) return Promise.resolve();
    return new Promise(done => {
      const child = spawn(process.execPath, [__filename, file, outDir, ...pass], { stdio: ["ignore", "pipe", "inherit"] });
      child.stdout.on("data", d => process.stdout.write(d));
      child.on("close", done);
    }).then(next);
  };
  Promise.all(Array.from({ length: jobs }, next)).then(() => {
    for (const file of chosen) {
      const doc = JSON.parse(fs.readFileSync(file, "utf8"));
      if (doc.documentType !== "part") continue;
      const at = path.join(outDir, doc.name + ".report.json");
      summary.push(fs.existsSync(at) ? JSON.parse(fs.readFileSync(at, "utf8"))
        : { name: doc.name, verdict: "the importer failed", error: "it wrote no report", features: [] });
    }
    writeSummary();
  });
} else {
for (const file of chosen) {
  const doc = JSON.parse(fs.readFileSync(file, "utf8"));
  if (doc.documentType !== "part") continue;
  doc.__file = file;
  const started = Date.now();
  let report;
  try {
    report = importPart(doc, outDir);
  } catch (e) {
    report = { name: doc.name, verdict: "the importer failed", error: String(e && e.stack || e), features: [] };
    fs.writeFileSync(path.join(outDir, doc.name + ".report.json"), JSON.stringify(report, null, 1));
    if (files.length === 1) console.error(report.error);
  }
  report.seconds = +((Date.now() - started) / 1000).toFixed(1);
  summary.push(report);
  const left = report.features.filter(x => x.result === "not supported" || x.result === "failed");
  console.log(`${report.verdict.padEnd(32)} ${report.name}${left.length ? "   [" + [...new Set(left.map(x => x.type))].join(", ") + "]" : ""}`);
}
writeSummary();
}
function writeSummary() {
  if (files.length <= 1) return;
  fs.writeFileSync(path.join(outDir, "summary.json"), JSON.stringify(summary.map(r => ({
    name: r.name, verdict: r.verdict, seconds: r.seconds, difference: r.difference, error: r.error,
    left: r.features.filter(x => x.result === "not supported" || x.result === "failed").map(x => ({ type: x.type, name: x.name, result: x.result, message: x.message })),
  })), null, 1));
  const counts = {};
  for (const r of summary) counts[r.verdict] = (counts[r.verdict] || 0) + 1;
  console.log(JSON.stringify(counts));
}
