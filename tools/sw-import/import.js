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
if (args.length !== 2) {
  console.error("Usage: node import.js <export.swx.json | folder of exports> <output folder> [--peet <peet executable>] [--only <text>]");
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
/**
 * A sketch with the segments of its blocks among its own. A block is a group of segments drawn once and
 * placed by a transform; PeetCAD sketches have no blocks, so each instance's segments are drawn where it puts
 * them and the grouping is lost. (The third export has what is in a block; before it they are missing.)
 */
function withBlocks(sketch) {
  if (!sketch || !sketch.blocks || !sketch.blocks.some(b => (b.segments || []).length)) return sketch;
  const segments = [...sketch.segments];
  sketch.blocks.forEach((block, k) => {
    const a = block.blockToSketch;
    // The same layout as every SolidWorks transform: a rotation by rows, then the translation and the scale
    const place = p => !p ? p : !a ? p : {
      ...p,
      x: (p.x * a[0] + p.y * a[3] + (p.z || 0) * a[6]) * (a[12] || 1) + a[9],
      y: (p.x * a[1] + p.y * a[4] + (p.z || 0) * a[7]) * (a[12] || 1) + a[10],
    };
    (block.segments || []).forEach((g, i) => segments.push({
      ...g, id: ["block" + k, i], start: place(g.start), end: place(g.end), center: place(g.center),
      radius: g.radius === undefined ? undefined : g.radius * ((a && a[12]) || 1),
    }));
  });
  return { ...sketch, segments, blockInstances: 0, blocksDrawn: sketch.blocks.length };
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

function importPart(doc, dir) {
  const config = doc.configurations.find(c => c.name === doc.activeConfiguration) || doc.configurations[0];
  const file = path.join(dir, doc.name + ".peet");
  const peet = new Peet(file);
  peet.note(`${path.basename(doc.sourcePath)}, configuration ${config.name}, imported from its SolidWorks export.`);

  const report = { name: doc.name, configuration: config.name, features: [], notes: [] };
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
  const flanges = new Map();     // SolidWorks edge flange name -> what it was made with, for mirroring it
  const rectangular = new Set(); // SolidWorks edge flanges whose outline could not be their own
  const miters = new Map();      // SolidWorks mitre flange name -> what it was made with, for mirroring it
  const promoted = new Map();    // "D1@Sketch1" -> the parameter that sketch dimension now follows
  const formulas = new Set();    // the sketch dimensions (full names) whose value is a formula
  const asleep = new Set();      // SolidWorks features imported and then suppressed in every configuration
  const cornerOps = new Map();   // PeetCAD corner feature name -> the operation that made it, for corner reliefs
  let unfolded = null;           // between an Unfold and its Fold: the face that stays where it is
  let sheet = null;             // the sheet metal settings in force
  let thickness = null;

  const record = (f, result, message) => report.features.push({ name: f.name, type: f.type, result, ...(message ? { message } : {}), ...(cur !== config ? { configuration: cur.name } : {}) });
  const sheetBody = () => (peet.ask({ op: "bodies" }).bodies || []).some(b => b.sheet_metal);

  /** Applies an operation that adds features. A feature that can't be built is taken out again. */
  function build(f, op) {
    const reply = peet.apply(op);
    if (!reply.ok) return { ok: false, message: reply.error };
    const created = reply.created || [];
    const failed = created.filter(c => c.status === "failed");
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
  function importSketch(f, { name = f.name, needFace = false, draw = null, onFace = null } = {}) {
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
        const construction = g.construction ? { construction: true } : {};
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
        } else if (g.type === "text" && sameHand) {
          // Its point is the sketch point with the text's id. The export has no font, height or angle:
          // they are fitted to SolidWorks' faces of the feature that uses the sketch, once it is drawn.
          const at = textPoint(s, g) || { x: 0, y: 0 };
          const string = g.text.replace(/<[^>]*>/g, "");
          if (string !== g.text) report.notes.push(`${name}'s text ${JSON.stringify(g.text)} has formatting, left out: drawn as ${JSON.stringify(string)}.`);
          // SolidWorks' x axis in the PeetCAD sketch: which way the text runs
          const angle = Math.atan2(dot(sf.x, pf.y), dot(sf.x, pf.x)) * 180 / Math.PI;
          items.push({ type: "text", at: toPeet(at), string, angle: tidy(angle), ...construction, as });
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
    for (const o of own) if (o.text) fitText(f, result, o.g, (reply.drawn || []).find(x => x.as === o.as));
    if (!draw && relations) relate(f, result, s, sf, own.filter(o => !o.text), reply.drawn || []);
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
      const off = Math.max(...[0, 1].flatMap(i => [Math.abs(t.bounds[0][i] - lo[i]), Math.abs(t.bounds[1][i] - hi[i])]));
      if (!best || off < best.off) best = { font, height: t.height, off, drawnIn: t.drawn_in };
    }
    if (!best) return;
    set(best.font, best.height);
    report.notes.push(`${sk.name}'s text ${JSON.stringify(g.text)}: ${best.font}${best.drawnIn ? ` (not installed: drawn in ${best.drawnIn})` : ""}, ${best.height} mm high, fitted to SolidWorks' faces of ${f.children.join(", ")} (its outline's box is ${best.off.toFixed(4)} mm from theirs): the export has no font or height.`);
  }

  /**
   * Adds a sketch's relations and dimensions, so that the sketch can be driven as it is in SolidWorks.
   * They are added all at once; if that moves the geometry or over-defines the sketch they are taken back and
   * added one at a time, keeping those that leave the sketch as it was drawn.
   */
  function relate(f, sk, s, sf, own, drawn) {
    const pf = sk.frame, name = sk.name;
    const tally = { relations: s.relations.length, kept: 0, dropped: {} };
    const drop = why => { tally.dropped[why] = (tally.dropped[why] || 0) + 1; };
    report.sketches = report.sketches || {};
    report.sketches[name] = tally;

    // The entity ids PeetCAD gave what was drawn, found again from SolidWorks' coordinates
    const ids = new Map(drawn.filter(d => d.as).map(d => [d.as, d]));
    const points = new Map(), segments = new Map();
    for (const o of own) {
      const d = ids.get(o.as);
      if (!d) continue;
      if (o.point) { points.set(spot(o.point), d.id); continue; }
      segments.set(o.g.id.join("."), { id: d.id, g: o.g, start: o.turned ? d.end : d.start, end: o.turned ? d.start : d.end, center: d.center });
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
      if (!mine && e.x !== undefined && Math.abs(e.x) < 1e-12 && Math.abs(e.y) < 1e-12) return originOk ? "origin" : undefined;
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
    const segmentOf = e => e.type ? byShape.get(shapeOf(e)) : segments.get(e.id.join("."));

    // Which way round the sketch's axes are in PeetCAD's sketch: horizontal there may be vertical here
    const across = Math.abs(dot(sf.x, pf.x)) > 0.9999 ? false : Math.abs(dot(sf.x, pf.y)) > 0.9999 ? true : null;
    const level = which => across === null ? null : (which === "horizontal") !== across ? "horizontal" : "vertical";

    // What a relation refers to outside the sketch is projected into it first
    const projected = new Map();
    const project = d => {
      if (!d) return undefined;
      let item = null;
      if (d.kind === "edge" && d.curve === "line" && d.start && d.end) item = { type: "project", edge: { at: mid(mm(d.start), mm(d.end)).map(tidy) } };
      else if (d.kind === "vertex" && d.point) item = { type: "project", vertex: { at: mm(d.point).map(tidy) } };
      else if (d.kind === "feature" && d.type === "RefPlane" && planes.has(d.name)) item = { type: "project", plane: planes.get(d.name) };
      if (!item) return undefined;
      const key = JSON.stringify(item);
      if (!projected.has(key)) {
        const reply = peet.apply({ op: "draw", sketch: name, draw: [item] });
        const made = reply.ok && (reply.drawn || [])[0];
        projected.set(key, made ? { id: made.id !== undefined ? made.id : (made.curves || made.points || [])[0], point: item.vertex !== undefined } : null);
      }
      return projected.get(key) || undefined;
    };

    const dimensions = new Map((f.dimensions || []).map(d => [d.fullName, d]));
    const byEquation = new Set((cur.equations || []).filter(e => !e.disabled).map(e => (/^\s*"([^"]+@[^"]+)"\s*=/.exec(e.expression) || [])[1]).filter(Boolean));
    const names = new Set();
    const items = [];
    for (const r of s.relations) {
      if (r.suppressed) { drop("suppressed"); continue; }
      const pts = [], segs = [];
      for (const e of r.entities || []) {
        if (!e) continue;
        if (e.kind === "sketchPoint") { const id = pointOf(e); if (id !== undefined) pts.push({ id, e: e.x !== undefined ? e : byId.get(e.id.join(".")) }); }
        else if (e.kind === "sketchSegment") { const g = segmentOf(e); if (g) segs.push(g); }
      }
      const outside = [];
      let lost = false;
      for (const d of r.defined || []) { const p = project(d); if (p && p.id !== undefined) outside.push(p); else lost = true; }
      if (lost) { drop(`${r.type} to something outside the sketch that could not be projected`); continue; }
      const lines = segs.filter(g => g.g.type === "line"), rounds = segs.filter(g => g.g.type === "arc");
      const outLines = outside.filter(o => !o.point).map(o => o.id), outPoints = outside.filter(o => o.point).map(o => o.id);
      const anyLines = [...lines.map(l => l.id), ...outLines], anyPoints = [...pts.map(p => p.id), ...outPoints];
      const all = [...anyPoints, ...segs.map(g => g.id), ...outLines];
      const dim = r.dimension && dimensions.get(r.dimension);
      let item = null;
      switch (r.type) {
        case "horizontal": case "vertical":
          if (level(r.type) && (lines.length === 1 || anyPoints.length === 2)) item = { type: level(r.type), of: lines.length === 1 ? [lines[0].id] : anyPoints };
          break;
        case "horizpoints": case "vertpoints":
          if (level(r.type.slice(0, 4) === "hori" ? "horizontal" : "vertical") && anyPoints.length === 2) item = { type: level(r.type.slice(0, 4) === "hori" ? "horizontal" : "vertical"), of: anyPoints };
          break;
        case "coincident": if (all.length === 2 && anyPoints.length >= 1) item = { type: "coincident", of: [anyPoints[0], all.find(x => x !== anyPoints[0])] }; break;
        case "colinear": case "useedge": if (anyLines.length === 2) item = { type: "collinear", of: anyLines }; break;
        case "parallel": case "perpendicular": if (anyLines.length === 2) item = { type: r.type, of: anyLines }; break;
        case "tangent": if (segs.length + outLines.length === 2 && rounds.length >= 1) item = { type: "tangent", of: [...segs.map(g => g.id), ...outLines] }; break;
        case "concentric": if (rounds.length === 2) item = { type: "concentric", of: rounds.map(g => g.id) }; break;
        case "coradial": if (rounds.length === 2) item = { type: "coradial", of: rounds.map(g => g.id) }; break;
        case "samelength": if (segs.length === 2) item = { type: "equal", of: segs.map(g => g.id) }; break;
        case "atmiddle": if (anyPoints.length === 1 && anyLines.length === 1) item = { type: "midpoint", of: [anyPoints[0], anyLines[0]] }; break;
        case "fixed": if (all.length === 1) item = { type: "fix", of: all }; break;
        case "symmetric": if (anyPoints.length === 2 && anyLines.length === 1) item = { type: "symmetric", of: [...anyPoints, anyLines[0]] }; break;
        case "diameter": case "radius": if (rounds.length === 1 && dim) item = { type: r.type, of: [rounds[0].id], value: tidy(dim.value * MM) }; break;
        case "angle": if (anyLines.length === 2 && dim) item = { type: "angle", of: anyLines, value: tidy(dim.value * 180 / Math.PI) }; break;
        case "doubledistance": {
          const centre = lines.find(l => l.g.construction), other = lines.find(l => l !== centre);
          const point = anyPoints[0] !== undefined ? anyPoints[0] : other && other.start;
          if (centre && point !== undefined && dim) item = { type: "doubled_distance", of: [point, centre.id], value: tidy(dim.value * MM) };
          break;
        }
        case "distance": {
          if (!dim) break;
          const value = tidy(dim.value * MM);
          if (pts.length === 2 && outside.length === 0) {
            // Between two points SolidWorks measures along x, along y or straight, and the export does not
            // say which: it is the one whose size the points are apart
            const dx = Math.abs(pts[0].e.x - pts[1].e.x), dy = Math.abs(pts[0].e.y - pts[1].e.y), v = dim.value;
            // The dimension line's direction is in the model's coordinates: along the sketch's x, its y, or neither
            const line = dim.lineDirection && len(dim.lineDirection) > 1e-9 ? unit(dim.lineDirection.slice(0, 3)) : null;
            const said = !line ? undefined : Math.abs(dot(line, sf.x)) > 0.9999 ? "horizontal" : Math.abs(dot(line, sf.y)) > 0.9999 ? "vertical" : null;
            const kind = said !== undefined ? said
              : dim.displayType === 11 || (Math.abs(v - dx) < 1e-9 && dy > 1e-9) ? "horizontal" : dim.displayType === 12 || (Math.abs(v - dy) < 1e-9 && dx > 1e-9) ? "vertical" : null;
            if (kind && !level(kind)) break;
            item = { type: kind ? level(kind) + "_distance" : "distance", of: pts.map(p => p.id), value };
          } else if (rounds.length && rounds.every(g => g.center !== undefined) && anyPoints.length + rounds.length === 2 && lines.length + outLines.length === 0) item = { type: "distance", of: [...anyPoints, ...rounds.map(g => g.center)], value };
          else if (rounds.length === 1 && rounds[0].center !== undefined && anyPoints.length === 0 && anyLines.length === 1) item = { type: "distance", of: [rounds[0].center, anyLines[0]], value };
          else if (anyPoints.length === 2) item = { type: "distance", of: anyPoints, value };
          else if (anyPoints.length === 1 && anyLines.length === 1) item = { type: "distance", of: [anyPoints[0], anyLines[0]], value };
          else if (lines.length === 2) item = { type: "distance", of: [lines[0].start, lines[1].id], value };
          else if (lines.length === 1 && outLines.length === 1) item = { type: "distance", of: [lines[0].start, outLines[0]], value };
          else if (lines.length === 1 && all.length === 1) item = { type: "length", of: [lines[0].id], value };
          break;
        }
      }
      if (!item) { drop(`${r.type}${r.defined ? " to the model" : ""}: no match for what it ties (${pts.length} points, ${lines.length} lines, ${rounds.length} arcs of ${(r.entities || []).length}${dim || !r.dimension ? "" : ", its dimension not found"})`); continue; }
      if (dim && item.value !== undefined) {
        let dimName = dim.name.replace(/[^A-Za-z0-9_]/g, "_").replace(/^(?=\d)/, "_");
        while (names.has(dimName)) dimName += "_";
        names.add(dimName);
        item.name = dimName;
        // SolidWorks calls a dimension driven both when it is for reference and when an equation sets it.
        // One an equation sets still drives the sketch: its value is carried over, per configuration.
        if (dim.drivenState === 1 && !byEquation.has(dim.fullName.split("@").slice(0, 2).join("@"))) item.driven = true;
        item.sw = dim.fullName;
      }
      items.push(item);
    }
    if (items.length === 0) return;

    const shape = () => JSON.stringify((peet.ask({ op: "feature", feature: name }).entities || []).map(e => [e.id, e.from, e.to, e.center, e.radius, e.at].map(x => Array.isArray(x) ? x.map(v => +v.toFixed(4)) : typeof x === "number" ? +x.toFixed(4) : x)));
    const before = shape();
    const clean = item => { const { sw, ...rest } = item; return rest; };
    const keep = item => { tally.kept++; if (item.sw) sk.dimensions.set(item.sw, item.name); };
    const backup = peet.file + ".before";
    const save = () => { fs.copyFileSync(peet.file, backup); return peet.script.length; };
    const restore = mark => { fs.copyFileSync(backup, peet.file); peet.script.length = mark; };
    const fits = reply => reply.ok && reply.definition !== "over_defined" && shape() === before;

    let mark = save();
    if (fits(peet.apply({ op: "draw", sketch: name, draw: items.map(clean) }))) items.forEach(keep);
    else {
      restore(mark);
      for (const item of items) {
        mark = save();
        const reply = peet.apply({ op: "draw", sketch: name, draw: [clean(item)] });
        if (fits(reply)) keep(item);
        else { restore(mark); drop(`${item.type}: ${!reply.ok ? "refused" : reply.definition === "over_defined" ? "over-defines the sketch" : "moves the sketch"}`); }
      }
    }
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
    let sum = [0, 0, 0], area = 0;
    for (const face of peet.ask({ op: "faces" }).faces || []) if (face.surface === "plane" && (face.made_by || []).some(m => names.includes(m.feature) && (m.side === "top" || m.side === "bottom"))) {
      sum = add(sum, mul(face.center, face.area_mm2));
      area += face.area_mm2;
    }
    return area ? mul(sum, 1 / area) : null;
  }
  /**
   * Builds a flange, and turns it over if it came out on the other side of the sheet from where SolidWorks
   * has it. Which way a flange bends is not in its definition: it follows from the edge picked.
   */
  function buildFlange(f, op, swName = f.name, where = null) {
    const theirs = where ? [where] : swName ? facesOf(swName).filter(face => face.surface === "plane" && face.area * 1e6 > 4 * (thickness || 1) ** 2).map(face => mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)))) : [];
    const want = theirs.length ? theirs : null;
    const off = names => { const at = middleInPeet(names); return at ? Math.min(...theirs.map(p => len(sub(p, at)))) : 0; };
    const good = Math.max(1, 0.2 * (op.length || 0), (thickness || 0) * 0.75);
    // The same flange turned over and, if it is set back unevenly, with its set-backs at the other ends:
    // which end of an edge is its start is not something the two programs agree on.
    const uneven = (op.offset_start || 0) !== (op.offset_end || 0);
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
      if (d < good) return built;
      if (!best || d < best.d) best = { variant, d };
      peet.apply({ op: "delete", features: built.names });
    }
    if (!best) return first;
    const built = build(f, best.variant);
    if (built.ok) built.warning = `it is ${best.d.toFixed(1)} mm from where SolidWorks has it`;
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

  function corners(name, gap, pieces) {
    const picks = [];
    const faces = () => peet.ask({ op: "faces" }).faces || [];
    let all = null;
    for (let i = 0; i < pieces.length; i++) for (let j = i + 1; j < pieces.length; j++) {
      const shared = [pieces[i].start, pieces[i].end].find(p => [pieces[j].start, pieces[j].end].some(q => len(sub(p, q)) < 1e-3));
      if (!shared) continue;
      all = all || faces();
      // The end face of one of the two pieces at this corner picks the corner: a face that PeetCAD names
      // after that piece and that is square to the piece's own edge. (Where one piece runs past the
      // other its end is flush with the other's outer face and is not a face of its own, and the end
      // of the piece that stops short may carry its neighbour's name: so both are looked for, and the
      // larger taken.)
      const ends = [];
      for (const piece of [pieces[i], pieces[j]]) {
        const along = unit(sub(piece.end, piece.start));
        ends.push(...all.filter(face => face.surface === "plane" && Math.abs(dot(face.normal, along)) > 0.999
          && (face.made_by || []).some(m => m.feature === piece.name && m.side === "wall")
          && Math.abs(dot(sub(face.center, shared), along)) < 0.25 * len(sub(piece.end, piece.start))));
      }
      ends.sort((a, b) => b.area_mm2 - a.area_mm2);
      if (ends[0]) picks.push({ at: ends[0].center.map(tidy), normal: ends[0].normal.map(tidy) });
    }
    if (picks.length === 0) return null;
    const op = { op: "corner", name, faces: picks, kind: "open", gap: tidy(Math.max(gap, 0) / Math.SQRT2), relief: "slanted" };
    const reply = peet.apply(op);
    if (!reply.ok) return null;
    const made = (reply.created || [])[0];
    if (!made || made.status === "failed") { peet.apply({ op: "delete", feature: name }); return null; }
    cornerOps.set(made.name, op);
    return made.name;
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
      if (away <= NEAR && (!best || score < best.score)) best = { score, away, from: edge.from, to: edge.to, middle: edge.middle };
    }
    return best;
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
    const centres = where ? where.sketch.segments.filter(g => g.type === "line" && g.start && g.end && Math.abs(Math.hypot(g.end.x - g.start.x, g.end.y - g.start.y) * MM - length) < 1e-3) : [];
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
   * SolidWorks' body: a round's axis is its radius over the cosine of half its sweep from the corner's edge
   * through the sheet, and a chamfer's face is in front of it. False if a corner's edge is not in the part
   * (the corner is not square, or the part differs there).
   */
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
    for (const face of faces) {
      const middle = mid(mm(face.box.slice(0, 3)), mm(face.box.slice(3)));
      if (face.cylinder) {
        const axis = unit(face.cylinder.slice(3, 6)), on = mm(face.cylinder.slice(0, 3)), radius = face.cylinder[6] * MM;
        const sweep = face.area * 1e6 / (radius * thickness);
        if (!(sweep > 0.05 && sweep < Math.PI - 0.05)) return false;
        const reach = radius / Math.cos(sweep / 2);
        const fits = e => {
          if (Math.abs(dot(unit(sub(e.to, e.from)), axis)) < 0.9999) return false;
          const v = sub(e.middle, on);
          return Math.abs(len(sub(v, mul(axis, dot(v, axis)))) - reach) < 0.02;
        };
        if (!take("fillet", radius, fits, middle)) return false;
      } else if (face.plane && face.normal) {
        // A chamfer's face is the sheet's thickness high and, on a square corner, its distance times
        // root 2 long: the corner's edge is half that length in front of its middle.
        const normal = unit(face.normal), length = face.area * 1e6 / thickness;
        const front = [1, -1].map(side => add(middle, mul(normal, side * length / 2)));
        const fits = e => Math.abs(dot(unit(sub(e.to, e.from)), normal)) < 1e-4 && front.some(q => len(sub(e.middle, q)) < 0.02);
        if (!take("chamfer", length / Math.SQRT2, fits, middle)) return false;
      } else return false;
    }
    const names = [];
    let n = 0;
    for (const g of groups.values()) {
      const built = build(f, { op: "break_corner", name: f.name + (n++ ? " " + n : ""), kind: g.kind, size: tidy(g.size), edges: g.edges.map(e => ({ between: [e.from, e.to] })) });
      if (!built.ok) { if (names.length) peet.apply({ op: "delete", features: names }); return false; }
      names.push(...built.names);
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
    if (breakCorners(f)) return;
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

  /**
   * Of several ways to build a feature, the one that comes out nearest SolidWorks': that makes the most of
   * SolidWorks' flat faces of it where they are, and then whose faces come nearest SolidWorks' in area. Each
   * is built and taken back again, and the tries are left out of the script.
   */
  function nearest(f, ops) {
    const theirs = facesOf(f.name).reduce((sum, face) => sum + face.area * 1e6, 0);
    const mark = peet.script.length;
    let best = null;
    for (const op of ops) {
      const tried = build(f, op);
      if (!tried.ok) { if (!best) best = { failed: tried }; continue; }
      const ours = (peet.ask({ op: "faces" }).faces || []).filter(x => (x.made_by || []).some(m => tried.names.includes(m.feature))).reduce((sum, x) => sum + x.area_mm2, 0);
      const off = Math.abs(ours - theirs), planes = wallsMatched(f.name, tried.names);
      peet.apply({ op: "delete", features: tried.names });
      if (!best || best.failed || planes > best.planes + 1e-6 || (planes > best.planes - 1e-6 && off < best.off - 1e-6)) best = { op, off, planes };
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
            peet.script.length = mark;
            peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name: turned });
            peet.apply({ op: "plane", from: turned, distance, flip, name });
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
            peet.script.length = mark;
            if (op) {
              peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name: turned });
              peet.apply(op);
            } else peet.apply({ op: "plane", from: base, about, angle: tidy(angle), name });
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

  const values = f => (f.definition && f.definition.values) || {};

  function bendOf(v) {
    const c = v.GetCustomBendAllowance;
    if (c && c.type === 4) return { deduction: tidy(c.bendDeduction * MM) };
    if (c && c.type === 3) return { allowance: tidy(c.bendAllowance * MM) };
    if (c && c.type === 1) report.notes.push("The bend allowance comes from a bend table, which PeetCAD doesn't read: the K-factor is used.");
    return { k_factor: (c && c.kFactor) || v.KFactor || 0.5 };
  }

  const LATER = "left for the configuration it is live in";
  const elsewhere = doc.configurations.filter(c => c !== config && !c.flatPattern).map(flatten);
  const liveElsewhere = name => elsewhere.some(m => m.has(name) && !m.get(name).suppressed);

  /**
   * A feature as SolidWorks has it: one that is suppressed is built like any other and then suppressed, in
   * every configuration. It can't be compared with anything, since SolidWorks' body does not have it.
   */
  function handle(f) {
    if (!f.suppressed || STRUCTURE.test(f.type) || f.type === "SheetMetal" || f.type === "RefPlane") return handleLive(f);
    // One that is live in another configuration is left for that configuration: there SolidWorks' body has
    // it, so the side a flange bends to and the like can be checked, and the part is the size it is drawn for.
    if (cur === config && !SKETCH.test(f.type) && liveElsewhere(f.name)) return record(f, LATER);
    const before = report.features.length, had = made.has(f.name);
    handleLive({ ...f, suppressed: false });
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
        if (v.OffsetType !== 1) notes.push("its length is up to a vertex in SolidWorks: the distance it had is used");
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
        const ops = found.map((edge, index) => {
          const s = edge.from, along = unit(sub(edge.to, edge.from)), whole = len(sub(edge.to, edge.from));
          let offsets = {}, outline = null;
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
            if (drawn.length !== 4 || drawn.some(g => g.type !== "line" || !g.start || !g.end || !square(g))) {
              // How far the outline rises from its foot, which the flange has to reach before it is cut to it
              const fa = fromSketch(sf, foot.g.start), fu = unit(sub(fromSketch(sf, foot.g.end), fa));
              const rise = Math.max(...drawn.flatMap(pointsAlong).map(q => { const d = sub(fromSketch(sf, q), fa); return len(sub(d, mul(fu, dot(d, fu)))); }));
              outline = { sketch: p, foot: foot.g, rise };
              // An outline that widens past an end of the edge its foot runs to: the flange is made that
              // wide (it overhangs the edge from the bend on) and cut back to the outline.
              const reach = drawn.flatMap(g => [g.start, g.end]).filter(Boolean).map(q => dot(sub(fromSketch(sf, q), s), along));
              if (Math.abs(start) < 1e-3 && Math.min(...reach) < -1e-3) start = Math.min(...reach);
              if (Math.abs(end) < 1e-3 && whole - Math.max(...reach) < -1e-3) end = whole - Math.max(...reach);
            }
            // A foot that runs past an end of the edge is a flange wider than its edge: a negative offset.
            if (Math.abs(start) > 1e-3) offsets.offset_start = tidy(start);
            if (Math.abs(end) > 1e-3) offsets.offset_end = tidy(end);
            break;
          }
          // An outline taller than the flange: the flange is made taller (by more than it can need, the cut
          // takes the rest), so that there is sheet to cut the outline from
          // (A flange's end is its length less the sheet's thickness from the face its outline is drawn from.)
          if (outline && outline.rise > length - (thickness || 0) + 0.01) {
            offsets.length = tidy(outline.rise + 2 * ((thickness || 0) + radius) + 1);
            outline.above = offsets.length;
            notes.push("its outline reaches further than its length: the flange is made long enough and cut to the outline");
          }
          return { edge: { at: edge.middle.map(tidy) }, offsets, outline, found: edge };
        });
        // A relief of the flange's own, where SolidWorks does not use the body's. One given as a width and
        // a depth becomes a ratio of its width: PeetCAD's relief is as deep past the bend as it is wide.
        const ownRelief = {};
        if (v.UseDefaultBendRelief === false) {
          const kind = { 1: "rectangular", 2: "tear", 3: "obround" }[v.AutoReliefType];
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
          const plane = piece + " outline plane";
          if (!planeAbout(plane, { between: [o.found.from.map(tidy), o.found.to.map(tidy)] }, sf)) return;
          const sk = importSketch(o.outline.sketch, { name: piece + " outline", onFace: plane });
          if (sk.error) { peet.apply({ op: "delete", features: [plane, plane + " turned"] }); return; }
          o.native = { plane, sketch: sk.name };
          outlined.push(plane, plane + " turned", sk.name);
        });
        const withOutline = o => o.native ? { outline: o.native.sketch } : {};
        let built;
        if (ops.length === 1) built = buildFlange(f, { ...common, edge: ops[0].edge, ...ops[0].offsets, ...withOutline(ops[0]) });
        else {
          built = { ok: true, names: [] };
          ops.forEach((o, i) => {
            if (!built.ok) return;
            const one = buildFlange(f, { ...common, name: pieceName(i), edge: o.edge, ...o.offsets, ...withOutline(o) });
            if (!one.ok) built = one; else built.names.push(...one.names);
          });
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
          if (o.offsets.length !== undefined) peet.apply({ op: "edit", feature: pieceName(i), length: common.length });
        });
        if (built.ok && notes.length) built.warning = [...new Set(notes)].join("; ");
        const gap = (v.GapDistance || 0) * MM;
        if (built.ok) {
          const flangeNames = [...built.names];
          const others = cornersWithOthers(f.name, flangeNames, radius);
          if (others) built.names.push(others);
          if (flangeNames.length === edges.length && edges.length > 1) {
            const corner = corners(f.name + " corners", gap, edges.map((e, i) => ({ name: flangeNames[i], start: mm(e.start), end: mm(e.end) })));
            if (corner) built.names.push(corner);
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
        const extrude = () => {
          const sk = useSketch(f);
          if (sk.error) return { ok: false, message: sk.error };
          const end = v["GetEndCondition(true)"];
          if (![0, 3, 4, 5, 6].includes(end)) return { ok: false, unsupported: true, message: `an extrusion with end condition ${end}` };
          if (v.BothDirections && end !== 6) return { ok: false, unsupported: true, message: "an extrusion in both directions" };
          const sf = frameOf(sketchOf(f).sketch.modelToSketch);
          let along = mul(sf.n, v.ReverseDirection ? -1 : 1);
          let depth = v["GetDepth(true)"] * MM;
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
            ...(end === 6 ? { end: "mid_plane" } : {}), reverse: dot(along, sk.frame.n) < 0,
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
          const reach = reachOf(end, depth, v.BothDirections);
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
        const shape = end === 0 ? { end: "blind", depth: tidy(depth) } : end === 6 ? { end: "mid_plane", depth: tidy(depth) } : { end: "through_all" };
        let built;
        // Which way it goes: the way that cuts where SolidWorks' cut did
        built = nearest(f, [false, true].map(reverse => ({ op: "cut", sketch: sk.name, name: f.name, ...shape, reverse })));
        if (built.ok && ![0, 1, 6, 9].includes(end)) built.warning = `SolidWorks' end condition ${end} is cut through all`;
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

      case "UnFold":
        unfolded = v.FixedFace || {};
        return record(f, "imported", "nothing to do: a sheet metal cut is made in the flat pattern");
      case "Fold":
        unfolded = null;
        return record(f, "imported", "nothing to do: a sheet metal cut is made in the flat pattern");

      case "Revolution": {
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const axis = v.Axis && v.Axis.kind === "sketchSegment" && v.Axis.id ? sk.ids.get(v.Axis.id.join(".")) : undefined;
        const angle = (v["GetRevolutionAngle(true)"] || 2 * Math.PI) * 180 / Math.PI;
        let built;
        for (const reverse of [false, true]) {
          built = build(f, { op: "revolve", sketch: sk.name, name: f.name, angle: tidy(angle), ...(axis !== undefined ? { axis: { line: axis } } : {}), ...(reverse ? { reverse } : {}) });
          if (built.ok || Math.abs(angle - 360) < 1e-6) break;
        }
        return finish(f, built);
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

      case "HoleWzd": {
        if (/slot/i.test(f.name)) return slots(f);
        const where = (f.sub || []).find(c => SKETCH.test(c.type) && c.sketch && !c.sketch.error && c.sketch.points.length);
        if (!where) return record(f, "failed", "the sketch of its positions was not exported");
        const wanted = new Set((v.GetSketchPoints || []).filter(p => p && p.id).map(p => p.id.join(".")));
        const points = where.sketch.points.filter(p => wanted.size === 0 || wanted.has(p.id.join(".")));
        const diameter = (v.ThruHoleDiameter || v.HoleDiameter || v.Diameter || 0) * MM;
        if (!diameter || points.length === 0) return record(f, "failed", "its size or its positions were not exported");
        if (where.sketch.is3D) return holesOn3D(f, v, points, diameter);
        if (sheetBody()) {
          const draw = toPeet => points.map(p => ({ type: "circle", center: toPeet(p), radius: tidy(diameter / 2) }));
          const sk = importSketch(where, { name: f.name + " sketch", needFace: true, draw });
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

      case "LPattern": {
        const seeds = [];
        for (const p of v.PatternFeatureArray || []) {
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
          const best = [...votes.values()].sort((a, b) => b.n - a.n)[0];
          return best ? best.d : null;
        };
        const direction = (axis, reversed, spacing, alone) => {
          let d = null;
          if (axis && axis.start && axis.end) d = mul(unit(sub(mm(axis.end), mm(axis.start))), reversed ? -1 : 1);
          else if (axis && axis.kind === "face" && axis.normal) {
            // Along a flat face's normal
            d = mul(unit(axis.normal), reversed ? -1 : 1);
            const way = seen(spacing * MM, d);
            if (way) d = way;
          } else if (alone) d = seen(spacing * MM, null);
          if (!d) return null;
          const k = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(d, AXES[k])) - 1) < 1e-9);
          if (k) return { axis: k, flip: dot(d, AXES[k]) < 0 };
          if (axis && axis.start && axis.end) return { axis: edgeSelector(axis), flip: !!reversed };
          // Any straight edge of the part that way
          const edge = (peet.ask({ op: "edges" }).edges || []).find(e => e.curve === "line" && len(sub(e.to, e.from)) > 1e-6 && Math.abs(Math.abs(dot(unit(sub(e.to, e.from)), d)) - 1) < 1e-9);
          return edge ? { axis: { between: [edge.from, edge.to] }, flip: dot(unit(sub(edge.to, edge.from)), d) < 0 } : null;
        };
        const grid = (v.D2TotalInstances || 1) > 1;
        const first = direction(v.D1Axis, v.D1ReverseDirection, v.D1Spacing, !grid);
        if (!first) return record(f, "failed", "its direction was not exported");
        const op = { op: "linear_pattern", name: f.name, features: seeds, direction: first.axis, flip: first.flip, spacing: tidy(v.D1Spacing * MM), count: v.D1TotalInstances };
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
        return finish(f, build(f, op));
      }

      case "MirrorPattern": {
        const seeds = [], bent = [], mitred = [], sheetMetal = [];
        for (const p of v.PatternFeatureArray || []) {
          if (p && flanges.has(p.name)) { bent.push(flanges.get(p.name)); sheetMetal.push(p.name); continue; }
          if (p && miters.has(p.name)) { mitred.push(miters.get(p.name)); sheetMetal.push(p.name); continue; }
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

  // ---- Compare with SolidWorks ----
  const mass = peet.ask({ op: "mass" });
  const bodies = peet.ask({ op: "bodies" }).bodies || [];
  const total = (mass && mass.total) || {};
  const sw = config.bodies || [];
  const swVolume = sw.reduce((a, b) => a + b.volume, 0) * 1e9;
  const swArea = sw.reduce((a, b) => a + b.area, 0) * 1e6;
  const swBox = sw.length ? [0, 1, 2].map(i => Math.min(...sw.map(b => (b.tightBox || b.box)[i])) * MM).concat([3, 4, 5].map(i => Math.max(...sw.map(b => (b.tightBox || b.box)[i])) * MM)) : null;
  const box = total.min ? [...total.min, ...total.max] : null;
  const boxOff = box && swBox ? Math.max(...box.map((x, i) => Math.abs(x - swBox[i]))) : null;
  const volumeOff = total.volume_mm3 && swVolume ? Math.abs(total.volume_mm3 - swVolume) / swVolume : null;
  const flat = bodies.find(b => b.flat_size);
  const swFlat = config.flatDxf ? dxfSize(path.join(path.dirname(doc.__file), config.flatDxf)) : null;
  const flatOff = flat && swFlat ? Math.max(...[...flat.flat_size].sort((a, b) => a - b).map((x, i) => Math.abs(x - [...swFlat].sort((a, b) => a - b)[i]))) : null;
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

    // ---- Equations: the ones in force ----
    // A global variable becomes a parameter. A dimension set by a formula gets the formula as its value. A
    // formula can't read a sketch dimension from outside its sketch, so a dimension that one reads is made
    // to follow a parameter of its own (Sketch1_Width), and the formula reads that. A feature suppressed by
    // a condition gets a suppression rule. Each is kept only if the part is still the same afterwards.
    const equations = { inForce: 0, imported: 0, left: {} };
    const ident = t => t.replace(/[^A-Za-z0-9_]/g, "_").replace(/^(?=\d)/, "_");
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
    const translate = text => {
      let failed = null;
      let out = text.replace(/"([^"]*)"/g, (whole, inner) => {
        const ref = inner.trim();
        if (/^(un)?suppressed$/i.test(ref)) return /^un/i.test(ref) ? "0" : "1";
        if (ref.includes("@")) {
          if (ref.split("@").length > 2) { failed = "it reads a value of another document"; return "0"; }
          const parameter = promote(ref);
          if (!parameter) { failed = "it reads a dimension that was not imported"; return "0"; }
          return parameter;
        }
        if (!globals.has(ref)) { failed = "it reads a variable that is not defined"; return "0"; }
        return globals.get(ref);
      });
      out = out.replace(/\b(un)?suppressed\b/gi, word => /^un/i.test(word) ? "0" : "1");
      return failed ? { failed } : { value: out.trim() };
    };
    const volumeNow = () => ((peet.ask({ op: "mass" }).total) || {}).volume_mm3;
    const steady = total.volume_mm3;
    // "Thickness" is SolidWorks' own name for the sheet's thickness, which equations read. It becomes a
    // parameter that the base flange follows.
    if (thickness && (config.equations || []).some(e => !e.disabled && /"Thickness"/.test(e.expression))) {
      const base = [...made.entries()].find(([swName]) => (byName.get(swName) || {}).type === "SMBaseFlange");
      if (peet.apply({ op: "set_parameter", name: "Thickness", value: tidy(thickness) + "mm" }).ok) {
        globals.set("Thickness", "Thickness");
        if (base) peet.apply({ op: "edit", feature: base[1][0], thickness: "Thickness" });
      }
    }
    for (const e of config.equations || []) {
      if (e.disabled) continue;
      equations.inForce++;
      const leave = why => { equations.left[why] = (equations.left[why] || 0) + 1; };
      const m = /^\s*"([^"]+)"\s*=\s*([\s\S]*)$/.exec(e.expression);
      if (!m) { leave("not understood"); continue; }
      const target = m[1].trim();
      const backup = peet.file + ".before", mark = peet.script.length;
      fs.copyFileSync(peet.file, backup);
      const known = new Map(promoted), knownGlobals = new Map(globals);
      const undo = why => { fs.copyFileSync(backup, peet.file); peet.script.length = mark; promoted.clear(); for (const [k, v] of known) promoted.set(k, v); globals.clear(); for (const [k, v] of knownGlobals) globals.set(k, v); leave(why); };
      const t = translate(m[2]);
      if (t.failed) { undo(t.failed); continue; }
      let ok = false, why = "refused";
      if (e.globalVariable) {
        ok = peet.apply({ op: "set_parameter", name: ident(target), value: t.value }).ok;
        if (ok) globals.set(target, ident(target));
      } else if (/suppressed/i.test(m[2])) {
        const names = made.get(target);
        if (!names) why = "it suppresses something that was not imported";
        else ok = names.every(n => peet.apply({ op: "set_suppression_expression", feature: n, value: t.value }).ok);
      } else if (target.split("@").length === 2) {
        const d = dimensionOf(target), feature = byName.get(target.split("@")[1]), names = made.get(target.split("@")[1]);
        if (d) {
          ok = peet.apply({ op: "set_dimension", sketch: d.sk.name, name: d.peetName, value: t.value, configurations: "all" }).ok;
          if (ok) formulas.add(d.full);
        } else if (feature && names && feature.type === "LPattern") {
          const own = (feature.dimensions || []).find(x => x.fullName.startsWith(target + "@"));
          if (own && promoted.has(target)) ok = peet.apply({ op: "set_parameter", name: promoted.get(target).parameter, value: t.value }).ok;
          else if (own) ok = peet.apply({ op: "edit", feature: names[0], [own.valueType === 2 ? "count" : "spacing"]: t.value }).ok;
          else why = "it sets a size of a feature that has no match";
        } else why = feature && !names ? "it sets a size of a feature that was not imported" : "it sets a size of a feature that has no match";
      } else why = "it sets a value of another document";
      if (!ok) { undo(why); continue; }
      const after = volumeNow();
      if (steady && (!after || Math.abs(after - steady) > 1e-6 * steady)) { undo("it changes the part: the translation does not mean what SolidWorks' equation does"); continue; }
      equations.imported++;
    }
    if (fs.existsSync(peet.file + ".before")) fs.unlinkSync(peet.file + ".before");
    if (equations.inForce) report.equations = equations;

    // ---- The other configurations ----
    // Each is added as a copy of the active one and made to agree with SolidWorks: the sketch dimensions
    // that differ, the features that are suppressed there, and the features that are live there and
    // suppressed in the active one, which are imported now, with that configuration shown.
    const others = doc.configurations.filter(c => c !== config && !c.flatPattern);
    if (others.length) {
      report.configurations = [];
      if (config.name !== "Default") peet.apply({ op: "edit_configuration", configuration: "Default", name: config.name });
      const all = [config, ...others];
      const maps = new Map(all.map(c => [c, flatten(c)]));
      const done = new Set();
      const entries = new Map();
      /** Makes a configuration agree with SolidWorks' for what has been imported so far. */
      const settle = c => {
        const entry = entries.get(c), there = maps.get(c);
        for (const [swName, sk] of sketches) {
          const feature = there.get(swName);
          for (const d of (feature && feature.dimensions) || []) {
            const was = sk.values && sk.values.get(d.fullName), peetName = sk.dimensions && sk.dimensions.get(d.fullName);
            if (was === undefined || Math.abs(was - d.value) < 1e-10 || done.has(c.name + "|" + d.fullName)) continue;
            done.add(c.name + "|" + d.fullName);
            const value = tidy(d.valueType === 1 ? d.value * 180 / Math.PI : d.value * MM);
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
        for (const [swName, names] of made) {
          const feature = there.get(swName);
          if (!feature || done.has(c.name + "|suppress|" + swName)) continue;
          // What was imported asleep is suppressed everywhere, and woken where SolidWorks has it live
          const on = !!feature.suppressed;
          if (on === asleep.has(swName)) continue;
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
        settle(other);
        // What is live here and was suppressed where it has been looked at so far
        cur = other;
        byName = maps.get(other);
        const before = report.features.length;
        for (const f of other.features) {
          if (f.suppressed || STRUCTURE.test(f.type) || f.type === "SheetMetal" || f.type === "RefPlane" || made.has(f.name) || sketches.has(f.name)) continue;
          if (report.features.some(x => x.name === f.name && x.result !== "suppressed" && x.result !== LATER)) continue;   // tried already, where it was live
          handle(f);
        }
        entry.added = report.features.slice(before).filter(x => /^imported/.test(x.result) && !SKETCH.test(x.type)).length;
        for (const x of report.features.slice(before)) if (x.result === "failed" || x.result === "not supported") entry.missing.push(`${x.name}: ${x.result}`);
        // Sizes of features themselves (a flange's length, a pattern's count) are not carried over yet
        const here = maps.get(config);
        for (const [swName, feature] of maps.get(other)) {
          const base = here.get(swName);
          if (!base || feature.suppressed || base.suppressed || SKETCH.test(feature.type)) continue;
          for (const d of feature.dimensions || []) {
            const was = (base.dimensions || []).find(x => x.fullName === d.fullName);
            if (was && Math.abs(was.value - d.value) > 1e-10 && d.fullName.split("@")[1] === swName) entry.missing.push(`${d.fullName.split("@").slice(0, 2).join("@")}, a feature's own size`);
          }
        }
      }
      cur = config;
      byName = maps.get(config);
      // Features imported for a later configuration are suppressed where SolidWorks has them suppressed,
      // the active configuration included
      for (const c of all) if (c === config || entries.has(c)) settle(c);
      for (const other of others) {
        const entry = entries.get(other);
        if (!entry || !peet.apply({ op: "configuration", configuration: other.name }).ok) continue;
        const now = (peet.ask({ op: "mass" }).total) || {};
        const volume = (other.bodies || []).reduce((sum, body) => sum + body.volume, 0) * 1e9;
        const theirBox = (other.bodies || []).length ? [0, 1, 2].map(i => Math.min(...other.bodies.map(body => (body.tightBox || body.box)[i])) * MM).concat([3, 4, 5].map(i => Math.max(...other.bodies.map(body => (body.tightBox || body.box)[i])) * MM)) : null;
        const off = now.volume_mm3 && volume ? Math.abs(now.volume_mm3 - volume) / volume : null;
        const boxDiff = now.min && theirBox ? Math.max(...[...now.min, ...now.max].map((x, i) => Math.abs(x - theirBox[i]))) : null;
        entry.difference = { volume: off, box_mm: boxDiff };
        entry.verdict = off !== null && off < 1e-5 && boxDiff < 1e-3 ? "matches" : off !== null && off < 0.05 && boxDiff < 1e-3 ? "same size, volume differs" : "differs";
        entry.missing = [...new Set(entry.missing)];
      }
      peet.apply({ op: "configuration", configuration: config.name });
      // The active configuration must still be what it was
      const after = (peet.ask({ op: "mass" }).total) || {};
      if (total.volume_mm3 && after.volume_mm3 && Math.abs(after.volume_mm3 - total.volume_mm3) > 1e-6 * total.volume_mm3) report.notes.push(`Adding the other configurations changed ${config.name}: its volume went from ${total.volume_mm3} to ${after.volume_mm3}.`);
    }
  }

  fs.writeFileSync(path.join(dir, doc.name + ".jsonl"), peet.script.map(l => typeof l === "string" ? l : JSON.stringify(l)).join("\n") + "\n");
  fs.writeFileSync(path.join(dir, doc.name + ".report.json"), JSON.stringify(report, null, 1));
  return report;
}

/** The size of a DXF drawing, from its header. */
function dxfSize(file) {
  if (!fs.existsSync(file)) return null;
  const lines = fs.readFileSync(file, "latin1").split(/\r?\n/).map(l => l.trim());
  const at = name => { const i = lines.indexOf(name); return i < 0 ? null : [parseFloat(lines[i + 2]), parseFloat(lines[i + 4])]; };
  const min = at("$EXTMIN"), max = at("$EXTMAX");
  return min && max ? [tidy(max[0] - min[0]), tidy(max[1] - min[1])] : null;
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
  const pass = [...(relations ? [] : ["--no-relations"]), "--peet", peetExe];
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
