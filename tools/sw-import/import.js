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
const fromSketch = (f, p) => add(add(add(f.o, mul(f.x, p.x * MM)), mul(f.y, p.y * MM)), mul(f.n, (p.z || 0) * MM));
const samePlane = (a, b) => Math.abs(Math.abs(dot(a.n, b.n)) - 1) < 1e-7 && Math.abs(dot(sub(a.o, b.o), b.n)) < 1e-4;
/** A sketch point's place as a key: two points a tenth of a micron apart are one. */
const spot = p => `${Math.round(p.x * 1e7)},${Math.round(p.y * 1e7)}`;
/** How far, in mm, an edge or a face may be from where SolidWorks has it and still be taken for the same one. */
const NEAR = 2;

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
  const promoted = new Map();    // "D1@Sketch1" -> the parameter that sketch dimension now follows
  const formulas = new Set();    // the sketch dimensions (full names) whose value is a formula
  const asleep = new Set();      // SolidWorks features imported and then suppressed in every configuration
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
  function importSketch(f, { name = f.name, needFace = false, draw = null } = {}) {
    const s = f.sketch;
    if (!s || s.error) return { error: "the sketch was not exported" };
    if (s.is3D) return { error: "a 3D sketch" };
    const sf = frameOf(s.modelToSketch);
    // The face may not be exactly where SolidWorks has it (a flange at an angle comes out a tenth of a
    // millimetre away). The nearest one within reach is taken, the sketch is projected onto it, and the
    // report says how far off it was.
    let on = null, pf = null, away = Infinity;
    for (const candidate of placements(s, sf, needFace)) {
      const plane = probe(candidate);
      if (!plane || Math.abs(dot(plane.n, sf.n)) < 0.9999) continue;
      const d = Math.abs(dot(sub(plane.o, sf.o), sf.n));
      if (d < away) { on = candidate; pf = plane; away = d; }
      if (d < 1e-4) break;
    }
    if (!on || away > NEAR) return { error: needFace ? "no flat face of the part lies where the sketch does" : "no plane or face lies where the sketch does" };

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
        } else left.add(g.type);
      });
      // Points drawn on their own, which relations and dimensions go to
      const used = new Set(s.segments.flatMap(g => ["start", "end", "center"].filter(k => g[k]).map(k => spot(g[k]))));
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
    if (away > 1e-4) report.notes.push(`${name} is on a face ${away.toFixed(3)} mm from where SolidWorks has it.`);
    const result = { name: reply.created[0].name, toPeet, frame: pf, partial, dimensions: new Map(), ids: new Map(), values: new Map((f.dimensions || []).map(d => [d.fullName, d.value])) };
    for (const o of own) { const d = (reply.drawn || []).find(x => x.as === o.as); if (d && o.g) result.ids.set(o.g.id.join("."), d.id); }
    if (!draw && relations) relate(f, result, s, sf, own, reply.drawn || []);
    return result;
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
            const kind = dim.displayType === 11 || (Math.abs(v - dx) < 1e-9 && dy > 1e-9) ? "horizontal" : dim.displayType === 12 || (Math.abs(v - dy) < 1e-9 && dx > 1e-9) ? "vertical" : null;
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
  function plainCorners(name, flangeNames, bendRadius) {
    const tops = (peet.ask({ op: "faces" }).faces || []).filter(face => face.surface === "plane"
      && (face.made_by || []).length === 1 && flangeNames.includes(face.made_by[0].feature) && face.made_by[0].side === "top");
    if (tops.length === 0) return null;
    const reply = peet.apply({ op: "corner", name, faces: tops.map(face => ({ at: face.center.map(tidy), normal: face.normal.map(tidy) })), kind: "plain", gap: tidy(Math.max(bendRadius, 0.01)), relief: "flush" });
    if (!reply.ok) return null;
    const made = (reply.created || [])[0];
    if (!made || made.status === "failed") { peet.apply({ op: "delete", feature: name }); return null; }
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
    const reply = peet.apply({ op: "corner", name, faces: picks, kind: "open", gap: tidy(Math.max(gap, 0) / Math.SQRT2), relief: "flush" });
    if (!reply.ok) return null;
    const made = (reply.created || [])[0];
    if (!made || made.status === "failed") { peet.apply({ op: "delete", feature: name }); return null; }
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
    const ends = facesOf(f.name).filter(face => face.cylinder).map(roundFace).filter(e => e.height > 1e-6);
    const pairs = [];
    while (ends.length >= 2) {
      const a = ends.shift();
      let k = 0;
      ends.forEach((e, i) => { if (len(sub(e.level, a.level)) < len(sub(ends[k].level, a.level))) k = i; });
      pairs.push([a, ends.splice(k, 1)[0]]);
    }
    if (pairs.length === 0) return record(f, "not supported", "a slot from the hole wizard, whose ends are not in SolidWorks' body to copy");
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
    if (names.length === 0) return record(f, "failed", problem || "its slots could not be read");
    made.set(f.name, names);
    return record(f, "imported with a warning", `as a sheet metal cut of the slots' shape, which does not follow the part${problem ? "; some slots were left: " + problem : ""}`);
  }

  /**
   * A break corner, or a fillet on a sheet's corner. PeetCAD has no such feature and its fillet turns sheet
   * metal into a plain solid, so the corner is cut off with a sheet metal cut shaped like what SolidWorks
   * removed, read off the round faces the feature left. The shape is right; it does not follow the part.
   */
  function roundCorners(f) {
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

  const edgeSelector = e => ({ at: mid(mm(e.start), mm(e.end)).map(tidy) });
  const values = f => (f.definition && f.definition.values) || {};

  function bendOf(v) {
    const c = v.GetCustomBendAllowance;
    if (c && c.type === 4) return { deduction: tidy(c.bendDeduction * MM) };
    if (c && c.type === 3) return { allowance: tidy(c.bendAllowance * MM) };
    if (c && c.type === 1) report.notes.push("The bend allowance comes from a bend table, which PeetCAD doesn't read: the K-factor is used.");
    return { k_factor: (c && c.kFactor) || v.KFactor || 0.5 };
  }

  /**
   * A feature as SolidWorks has it: one that is suppressed is built like any other and then suppressed, in
   * every configuration. It can't be compared with anything, since SolidWorks' body does not have it.
   */
  function handle(f) {
    if (!f.suppressed || STRUCTURE.test(f.type) || f.type === "SheetMetal" || f.type === "RefPlane") return handleLive(f);
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
      const pf = frameOf(f.planeTransform);
      planeFrames.set(f.name, pf);
      const standard = standardPlane(pf);
      if (standard) { planes.set(f.name, standard); return; }
      if (f.suppressed) return record(f, "suppressed");
      // A plane square to an axis is an offset of a standard plane. It is where SolidWorks has it, but it
      // no longer follows what it was defined from.
      const axis = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(pf.n, AXES[k])) - 1) < 1e-9);
      if (!axis) return record(f, "not supported", "a plane that is not square to an axis");
      const from = { x: "right", y: "front", z: "top" }[axis];
      const distance = Math.abs(dot(pf.o, AXES[axis]));
      for (const flip of [false, true]) {
        const op = { op: "plane", from, distance: tidy(distance), flip, name: f.name };
        const tried = peet.apply(op);
        if (!tried.ok) return record(f, "failed", tried.error);
        const where = probe(f.name);
        if (where && samePlane(where, pf)) { planes.set(f.name, f.name); return record(f, "imported", "as a fixed offset plane"); }
        peet.apply({ op: "delete", feature: f.name });
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
        if (radius < 0.01) { radius = 0.01; report.notes.push("The bend radius is 0 in SolidWorks; 0.01 mm is used."); }
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
        return finish(f, build(f, {
          op: "base_flange", sketch: sk.name, name: f.name, thickness: tidy(thickness), radius: tidy(radius), bend: bendOf(settings),
          ...(relief ? { relief } : {}), ...(settings.ReliefRatio ? { relief_ratio: settings.ReliefRatio } : {}),
          reverse: dot(direction, sk.frame.n) < 0,
        }));
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
        // Its own sketch is the flange's outline. One narrower than the edge sets the flange back from the ends.
        const profiles = (f.sub || []).filter(c => SKETCH.test(c.type) && c.sketch && !c.sketch.error);
        // Every edge is found before any flange is added: a flange changes the ends of its neighbours
        const found = edges.map(e => findEdge(mm(e.start), mm(e.end)));
        if (found.some(e => !e)) return record(f, "failed", `no edge of the part is within ${NEAR} mm of where SolidWorks has the flange's edge`);
        const far = Math.max(...found.map(e => e.away));
        if (far > 1e-3) notes.push(`its edge is ${far.toFixed(3)} mm from where SolidWorks has it`);
        const ops = found.map((edge, index) => {
          const s = edge.from, along = unit(sub(edge.to, edge.from)), whole = len(sub(edge.to, edge.from));
          let offsets = {};
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
              if (!foot || away < foot.away - 1e-6) foot = { away, on: [dot(a, along), dot(b, along)] };
            }
            if (!foot) continue;
            const on = foot.on;
            const start = Math.min(...on), end = whole - Math.max(...on);
            if (start > 1e-3) offsets.offset_start = tidy(start);
            if (end > 1e-3) offsets.offset_end = tidy(end);
            if (p.sketch.segments.filter(g => !g.construction).length !== 4 || p.sketch.segments.some(g => g.type !== "line")) notes.push("its outline is not a rectangle: a rectangular flange is made");
            break;
          }
          return { edge: { at: edge.middle.map(tidy) }, offsets };
        });
        const common = {
          op: "edge_flange", name: f.name, length: tidy(length), angle: tidy(angle), position,
          ...(v.UseDefaultBendRadius === false ? { radius: tidy(Math.max(radius, 0.01)) } : {}),
        };
        let built;
        if (ops.length === 1) built = buildFlange(f, { ...common, edge: ops[0].edge, ...ops[0].offsets });
        else {
          built = { ok: true, names: [] };
          ops.forEach((o, i) => {
            if (!built.ok) return;
            const one = buildFlange(f, { ...common, name: f.name + (i + 1), edge: o.edge, ...o.offsets });
            if (!one.ok) built = one; else built.names.push(...one.names);
          });
        }
        if (built.ok && notes.length) built.warning = [...new Set(notes)].join("; ");
        const gap = (v.GapDistance || 0) * MM;
        if (built.ok) {
          const flangeNames = [...built.names];
          // A plain corner is what SolidWorks leaves between two flange features in the parts looked at,
          // but not in all of them (flanges at an angle differ), so it is kept only where it brings the
          // flanges nearer SolidWorks' than PeetCAD's own corner does.
          const sofar = [...[...flanges.keys()].map(n => [n, made.get(n) || []]), [f.name, flangeNames]];
          const before = flangeMismatch(sofar);
          const plain = plainCorners(f.name + " corners with others", flangeNames, radius);
          if (plain && flangeMismatch(sofar) < before - 1e-3) built.names.push(plain);
          else if (plain) peet.apply({ op: "delete", feature: plain });
          if (flangeNames.length === edges.length && edges.length > 1) {
            const corner = corners(f.name + " corners", gap, edges.map((e, i) => ({ name: flangeNames[i], start: mm(e.start), end: mm(e.end) })));
            if (corner) built.names.push(corner);
          }
        }
        if (built.ok) flanges.set(f.name, { name: f.name, common, gap, pieces: edges.map((e, i) => ({ start: mm(e.start), end: mm(e.end), offsets: ops[i].offsets })) });
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
          if (![0, 6].includes(end)) return { ok: false, unsupported: true, message: `an extrusion with end condition ${end}` };
          if (v.BothDirections && end !== 6) return { ok: false, unsupported: true, message: "an extrusion in both directions" };
          const along = mul(frameOf(sketchOf(f).sketch.modelToSketch).n, v.ReverseDirection ? -1 : 1);
          return build(f, {
            op: "extrude", sketch: sk.name, name: f.name, depth: tidy(v["GetDepth(true)"] * MM),
            ...(end === 6 ? { end: "mid_plane" } : {}), reverse: dot(along, sk.frame.n) < 0,
          });
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
          const sk = useSketch(f);
          let built = sk.error ? { ok: false, message: sk.error } : build(f, { op: "sheet_cut", sketch: sk.name, name: f.name });
          if (!built.ok && sketchOf(f)) {
            // A sheet metal cut's sketch has to be on a face of the sheet. One on a plane is put on the face there.
            const moved = useSketch(f, { name: f.name + " sketch", needFace: true });
            if (!moved.error) built = build(f, { op: "sheet_cut", sketch: moved.name, name: f.name });
            if (!built.ok && !moved.error) peet.apply({ op: "delete", feature: moved.name });
          }
          return finish(f, built);
        }
        const sk = useSketch(f);
        if (sk.error) return record(f, "failed", sk.error);
        const shape = end === 0 ? { end: "blind", depth: tidy(depth) } : end === 6 ? { end: "mid_plane", depth: tidy(depth) } : { end: "through_all" };
        let built;
        for (const reverse of [false, true]) {
          built = build(f, { op: "cut", sketch: sk.name, name: f.name, ...shape, reverse });
          if (built.ok) break;
        }
        if (built.ok && ![0, 1, 6, 9].includes(end)) built.warning = `SolidWorks' end condition ${end} is cut through all`;
        if (!built.ok && unsure) {
          const added = extrude();
          if (added.ok) return finish(f, added);
        }
        return finish(f, built);
      }

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
        if (!sheetBody()) return record(f, "not supported", null);
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
        if (sheetBody()) {
          const sk = importSketch(where, { name: f.name + " sketch", needFace: true, draw: toPeet => points.map(p => ({ type: "circle", center: toPeet(p), radius: tidy(diameter / 2) })) });
          if (sk.error) return record(f, "failed", sk.error);
          const built = build(f, { op: "sheet_cut", sketch: sk.name, name: f.name });
          if (!built.ok) peet.apply({ op: "delete", feature: sk.name });
          return finish(f, built);
        }
        const sk = importSketch(where, { name: f.name + " sketch", draw: toPeet => points.map(p => ({ type: "point", at: toPeet(p) })) });
        if (sk.error) return record(f, "failed", sk.error);
        const blind = v.EndCondition === 0 && (v.HoleDepth || v.ThruHoleDepth);
        let built;
        for (const reverse of [false, true]) {
          built = build(f, { op: "hole", sketch: sk.name, name: f.name, diameter: tidy(diameter), ...(blind ? { end: "blind", depth: tidy(blind * MM) } : { end: "through_all" }), reverse });
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
        const direction = (axis, reversed) => {
          if (!axis || !axis.start || !axis.end) return null;
          const d = mul(unit(sub(mm(axis.end), mm(axis.start))), reversed ? -1 : 1);
          const k = ["x", "y", "z"].find(k => Math.abs(Math.abs(dot(d, AXES[k])) - 1) < 1e-9);
          return k ? { axis: k, flip: dot(d, AXES[k]) < 0 } : { axis: edgeSelector(axis), flip: !!reversed };
        };
        const first = direction(v.D1Axis, v.D1ReverseDirection);
        if (!first) return record(f, "failed", "its direction was not exported");
        const op = { op: "linear_pattern", name: f.name, features: seeds, direction: first.axis, flip: first.flip, spacing: tidy(v.D1Spacing * MM), count: v.D1TotalInstances };
        if ((v.D2TotalInstances || 1) > 1) {
          const second = direction(v.D2Axis, v.D2ReverseDirection);
          if (!second) return record(f, "failed", "its second direction was not exported");
          Object.assign(op, { direction2: second.axis, flip2: second.flip, spacing2: tidy(v.D2Spacing * MM), count2: v.D2TotalInstances });
        }
        return finish(f, build(f, op));
      }

      case "MirrorPattern": {
        const seeds = [], bent = [];
        for (const p of v.PatternFeatureArray || []) {
          if (p && flanges.has(p.name)) { bent.push(flanges.get(p.name)); continue; }
          if (!p || !made.has(p.name)) return record(f, "failed", `what it mirrors (${p ? p.name : "nothing"}) was not imported`);
          seeds.push(...made.get(p.name));
        }
        if (seeds.length + bent.length === 0) return record(f, "not supported", "a mirror of faces or bodies");
        const plane = v.Plane && planes.get(v.Plane.name);
        const where = v.Plane && planeFrames.get(v.Plane.name);
        if (!plane || !where) return record(f, "failed", "its mirror plane was not imported");
        const names = [];
        let warning = null;
        // PeetCAD's mirror does not copy flanges, so a mirrored flange is made as a flange of its own on the
        // edge across the plane. It is where the mirror puts it, but it no longer follows the original.
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
  const swBox = sw.length ? [0, 1, 2].map(i => Math.min(...sw.map(b => b.box[i])) * MM).concat([3, 4, 5].map(i => Math.max(...sw.map(b => b.box[i])) * MM)) : null;
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
          if (report.features.some(x => x.name === f.name && x.result !== "suppressed")) continue;   // tried already, where it was live
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
        const theirBox = (other.bodies || []).length ? [0, 1, 2].map(i => Math.min(...other.bodies.map(body => body.box[i])) * MM).concat([3, 4, 5].map(i => Math.max(...other.bodies.map(body => body.box[i])) * MM)) : null;
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
for (const file of files) {
  if (only && !path.basename(file).toLowerCase().includes(only.toLowerCase())) continue;
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
if (files.length > 1) {
  fs.writeFileSync(path.join(outDir, "summary.json"), JSON.stringify(summary.map(r => ({
    name: r.name, verdict: r.verdict, seconds: r.seconds, difference: r.difference, error: r.error,
    left: r.features.filter(x => x.result === "not supported" || x.result === "failed").map(x => ({ type: x.type, name: x.name, result: x.result, message: x.message })),
  })), null, 1));
  const counts = {};
  for (const r of summary) counts[r.verdict] = (counts[r.verdict] || 0) + 1;
  console.log(JSON.stringify(counts));
}
