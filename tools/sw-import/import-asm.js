#!/usr/bin/env node
/**
 * Rebuilds SolidWorks assemblies in PeetCAD from their exports (tools/sw-export), using the parts a run
 * of import.js made, and says how close each came out.
 *
 *   node tools/sw-import/import-asm.js <export.swx.json | folder of exports> <parts run folder> <output folder>
 *        [--peet <peet>] [--only <text>]
 *
 * For each assembly it writes <name>.peet (an assembly), <name>.jsonl (the operations) and <name>.report.json.
 * A component is inserted where SolidWorks has it, then the mates are added: a mate that holds there moves
 * nothing, so the report's `moved` says how far PeetCAD's mates disagree with SolidWorks' placement.
 *
 * Sub-assemblies are imported first, from their own export where there is one (Hinge Assembly), or else
 * from the components their parent lists under them (Escutcheon Surround, whose file is not in Templates).
 */
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const args = process.argv.slice(2);
const option = name => { const i = args.indexOf(name); if (i < 0) return null; const v = args[i + 1]; args.splice(i, 2); return v; };
const repo = path.resolve(__dirname, "..", "..");
const peetExe = option("--peet")
  || [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe"), "peet"].find(p => p === "peet" || fs.existsSync(p));
const only = option("--only");
if (args.length !== 3) {
  console.error("Usage: node import-asm.js <export.swx.json | folder of exports> <parts run folder> <output folder> [--peet <peet>] [--only <text>]");
  process.exit(2);
}
const [input, partsDir, outDir] = args.map(a => path.resolve(a));
const exportDir = fs.statSync(input).isDirectory() ? input : path.dirname(input);
const MM = 1000;
const tidy = v => Math.abs(v) < 1e-12 ? 0 : +v.toFixed(6);

// ---- peet ----

class Peet {
  constructor(file) {
    this.file = file;
    this.script = [];
    if (fs.existsSync(file)) fs.unlinkSync(file);
    // `peet new` makes a part: an assembly is started in memory and saved where the file goes
    const made = spawnSync(peetExe, ["op", JSON.stringify({ op: "new", assembly: true, discard: true }), JSON.stringify({ op: "save", path: file }), "-q"], { encoding: "utf8" });
    if (made.status !== 0 || !fs.existsSync(file)) throw new Error("peet could not start an assembly: " + (made.stderr || made.stdout));
  }
  run(op, save) {
    const r = spawnSync(peetExe, ["run", "-", "-f", this.file, "-q", ...(save ? [] : ["--no-save"])],
      { input: JSON.stringify(op) + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
    const line = (r.stdout || "").split("\n").find(l => l.startsWith("{"));
    if (!line) return { ok: false, error: (r.stderr || "no reply").trim() };
    return JSON.parse(line);
  }
  apply(op) { const reply = this.run(op, true); if (reply.ok) this.script.push(op); return reply; }
  /** Several operations, one after another, saved; those that fail are left out and the rest still done. */
  applyAll(ops) {
    if (!ops.length) return [];
    const r = spawnSync(peetExe, ["run", "-", "-f", this.file, "-q", "--keep-going"],
      { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
    const replies = (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
    // A run with an operation that failed is not saved: the others again, by themselves
    const done = ops.filter((op, i) => replies[i] && replies[i].ok);
    if (done.length < ops.length && done.length) {
      spawnSync(peetExe, ["run", "-", "-f", this.file, "-q"],
        { input: done.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
    }
    this.script.push(...done);
    return replies;
  }
  ask(op) { return this.run(op, false); }
  /** The replies to several operations run one after another, nothing saved. */
  askAll(ops) {
    const r = spawnSync(peetExe, ["run", "-", "-f", this.file, "-q", "--no-save"],
      { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
    return (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
  }
  note(text) { this.script.push("# " + text); }
  backup() { fs.copyFileSync(this.file, this.file + ".before"); return this.script.length; }
  restore(mark) { fs.copyFileSync(this.file + ".before", this.file); this.script.length = mark; }
}

// ---- Transforms ----
//
// SolidWorks' transform (MathTransform.ArrayData) is a rotation R applied to row vectors, then a translation:
// world = p·R + t, metres. PeetCAD places a component at a point with an axis-angle rotation, millimetres.

const matrixOf = t => ({ R: [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]], t: [t[9], t[10], t[11]] });
const toWorld = (m, p) => [0, 1, 2].map(j => p[0] * m.R[0][j] + p[1] * m.R[1][j] + p[2] * m.R[2][j] + m.t[j]);
const inverse = m => {
  const R = [0, 1, 2].map(i => [0, 1, 2].map(j => m.R[j][i]));
  return { R, t: [0, 1, 2].map(j => -(m.t[0] * R[0][j] + m.t[1] * R[1][j] + m.t[2] * R[2][j])) };
};
/** The axis and angle (degrees) of the rotation that takes a part's axes to where SolidWorks' transform puts them. */
function axisAngle(m) {
  // Column-vector form A = Rᵀ: A[i][j] = R[j][i]
  const A = (i, j) => m.R[j][i];
  const c = Math.max(-1, Math.min(1, (A(0, 0) + A(1, 1) + A(2, 2) - 1) / 2));
  const angle = Math.acos(c);
  if (angle < 1e-12) return null;
  let axis;
  if (Math.PI - angle > 1e-6) axis = [A(2, 1) - A(1, 2), A(0, 2) - A(2, 0), A(1, 0) - A(0, 1)];
  else {
    // Half a turn: the axis from the diagonal
    const d = [0, 1, 2].map(i => Math.sqrt(Math.max(0, (A(i, i) + 1) / 2)));
    const k = d.indexOf(Math.max(...d));
    axis = [0, 1, 2].map(i => i === k ? d[k] : (A(k, i) + A(i, k)) / (4 * d[k]));
  }
  const n = Math.hypot(...axis);
  return { axis: axis.map(v => tidy(v / n)), angle: tidy(angle * 180 / Math.PI) };
}
/** Where PeetCAD says a component is, as a SolidWorks-style matrix, to set against SolidWorks' own. */
function matrixFromPeet(c) {
  const [x, y, z] = c.at.map(v => v / MM);
  if (!c.rotate || !c.rotate.angle) return { R: [[1, 0, 0], [0, 1, 0], [0, 0, 1]], t: [x, y, z] };
  const a = c.rotate.angle * Math.PI / 180, [u, v, w] = c.rotate.axis;
  const C = Math.cos(a), S = Math.sin(a), T = 1 - C;
  const A = [[T * u * u + C, T * u * v - S * w, T * u * w + S * v], [T * u * v + S * w, T * v * v + C, T * v * w - S * u], [T * u * w - S * v, T * v * w + S * u, T * w * w + C]];
  return { R: [0, 1, 2].map(i => [0, 1, 2].map(j => A[j][i])), t: [x, y, z] };
}
/** `m` then `n`: the placement `m` gives, placed by `n`. */
const then = (m, n) => ({
  R: [0, 1, 2].map(i => [0, 1, 2].map(j => m.R[i][0] * n.R[0][j] + m.R[i][1] * n.R[1][j] + m.R[i][2] * n.R[2][j])),
  t: toWorld(n, m.t),
});
/** How far apart two placements are: the largest distance between where they put points 100 mm round the origin. */
function apart(a, b) {
  return Math.max(...[[0, 0, 0], [0.1, 0, 0], [0, 0.1, 0], [0, 0, 0.1]].map(p => {
    const [u, v] = [toWorld(a, p), toWorld(b, p)];
    return Math.hypot(u[0] - v[0], u[1] - v[1], u[2] - v[2]) * MM;
  }));
}

// ---- Files ----

const exportsByName = new Map();
for (const f of fs.readdirSync(exportDir).filter(f => f.endsWith(".swx.json"))) exportsByName.set(f.replace(/\.swx\.json$/, "").toLowerCase(), path.join(exportDir, f));
const exportOf = file => exportsByName.get(path.basename(file).toLowerCase());
const partFileOf = file => {
  const p = path.join(partsDir, path.basename(file).replace(/\.sldprt$/i, "") + ".peet");
  return fs.existsSync(p) ? p : null;
};
const partReport = file => {
  const p = path.join(partsDir, path.basename(file).replace(/\.sldprt$/i, "") + ".report.json");
  return fs.existsSync(p) ? JSON.parse(fs.readFileSync(p)) : null;
};

/** The configurations a part's import has, its active one first (null if it has no report). */
function partConfigurations(file) {
  const pr = partReport(file);
  if (!pr) return null;
  return [pr.configuration, ...(pr.configurations || []).map(c => c.name)].filter(Boolean);
}

/**
 * A component's part as PeetCAD has it: its file and the configuration of it the component shows (SolidWorks'
 * referenced configuration; the component chooses it, so one file serves every configuration). `view` is what
 * the faces and edges are looked up in: the file, in that configuration where it is not the file's active one.
 */
function partOf(comp) {
  const base = partFileOf(comp.file);
  if (!base) return { error: "the part was not imported" };
  const configs = partConfigurations(comp.file) || [];
  const wanted = comp.configuration;
  if (!wanted) return { path: base, view: base, configuration: null };
  if (!configs.includes(wanted)) return { error: `the part's import has no configuration "${wanted}" (it has ${configs.join(", ") || "none"})` };
  return { path: base, view: wanted === configs[0] ? base : `${base}|${wanted}`, configuration: wanted };
}

/** A view (a part file, and a configuration after a "|") split into the two. */
const splitView = v => { const i = v.indexOf("|"); return i < 0 ? [v, null] : [v.slice(0, i), v.slice(i + 1)]; };
const viewName = v => { const [f, c] = splitView(v); return path.basename(f, ".peet") + (c ? ` (${c})` : ""); };
/** The replies to operations on a part, in a configuration of it where the view names one; nothing saved. */
function askView(view, ops) {
  const [file, configuration] = splitView(view);
  const all = configuration ? [{ op: "configuration", configuration }, ...ops] : ops;
  const r = spawnSync(peetExe, ["run", "-", "-f", file, "-q", "--no-save"],
    { input: all.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 28 });
  const replies = (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
  return configuration ? replies.slice(1) : replies;
}

// ---- Mates ----

const MATE_KINDS = {
  MateCoincident: "coincident", MateConcentric: "concentric", MateParallel: "parallel", MateDistanceDim: "distance", MateWidth: "width",
  MateSlot: "slot", MateLimitPlanarAngleDim: "limit_angle", MateLimitAngleDim: "limit_angle", MateLimitDistanceDim: "limit_distance",
  MateSymmetric: "symmetric",
  MatePath: "path",
  // SolidWorks' coincident of two origins (or coordinate systems): the points together
  MateCoordinate: "coincident",
};
const WHY_NOT = {};
// SolidWorks' Front Plane is XY, PeetCAD's top; its Top Plane is XZ, PeetCAD's front (whose normal is -Y where
// SolidWorks' is +Y); Right is YZ in both.
const STANDARD_PLANES = { "Front Plane": "top", "Top Plane": "front", "Right Plane": "right" };
const STANDARD_NORMALS = { top: [0, 0, 1], front: [0, -1, 0], right: [1, 0, 0] };

// The faces and edges of each part file, as PeetCAD has them, to find a mate's end by its geometry
const geometryCache = new Map();
function geometryOf(file) {
  if (geometryCache.has(file)) return geometryCache.get(file);
  const [faces, edges] = askView(file, [{ op: "faces" }, { op: "edges" }]);
  const g = { faces: (faces || {}).faces || [], edges: (edges || {}).edges || [] };
  geometryCache.set(file, g);
  return g;
}
const sub3 = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot3 = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const len3 = a => Math.hypot(a[0], a[1], a[2]);
const norm3 = a => { const l = len3(a); return a.map(v => v / l); };
/** Distance from a point to the line through `o` along unit `d`. */
const offLine = (p, o, d) => { const v = sub3(p, o); const t = dot3(v, d); return len3(sub3(v, d.map(x => x * t))); };
const TOL = 1e-3;   // mm

/**
 * What SolidWorks says a mate end is, in the part's coordinates (mm): a plane, an axis (a cylinder, a cone, a
 * circle's axis), a circle, a line or a point; from its description of the entity where it has one, else from
 * its numbers for it in the assembly (`params`: a point, a direction and a radius).
 */
function shapeOf(e, comp) {
  const x = e.entity;
  const mm = p => p.map(v => v * MM);
  if (x && x.kind === "face") {
    // `normal` is the face's outward normal; the plane's own may point either way
    if (x.plane) return { kind: "plane", n: norm3(x.normal || x.plane.slice(0, 3)), oriented: !!x.normal, p: mm(x.plane.slice(3, 6)), near: x.point && mm(x.point) };
    if (x.cylinder) return { kind: "axis", o: mm(x.cylinder.slice(0, 3)), d: norm3(x.cylinder.slice(3, 6)), r: x.cylinder[6] * MM, near: x.point && mm(x.point) };
    if (x.normal && x.point) return { kind: "plane", n: norm3(x.normal), p: mm(x.point), near: mm(x.point) };
  }
  if (x && x.kind === "edge") {
    if (x.circle) return { kind: "circle", c: mm(x.circle.slice(0, 3)), n: norm3(x.circle.slice(3, 6)), r: x.circle[6] * MM };
    if (x.start && x.end) return { kind: "line", a: mm(x.start), b: mm(x.end) };
  }
  if (x && x.kind === "vertex" && x.point) return { kind: "point", p: mm(x.point) };
  if (!x && e.params && comp) return shapeFromParams(e, comp.transform);
  return null;
}

/**
 * What SolidWorks' numbers for an end in the assembly (`params`: a point, a direction, a radius) say it is, in
 * the part placed by `transform`. `referenceType` is the kind of thing selected (swSelectType_e): 1 an edge,
 * 2 a face, 3 a vertex. A face with a radius is round (its axis), without one flat; an edge with a radius is
 * a circle, without one straight.
 */
function shapeFromParams(e, transform) {
  const mm = p => p.map(v => v * MM);
  const back = inverse(matrixOf(transform));
  const p = mm(toWorld(back, e.params.slice(0, 3)));
  const d = norm3(toWorld({ R: back.R, t: [0, 0, 0] }, e.params.slice(3, 6)));
  const r = e.params[6] * MM;
  // A plane's direction is its face's outward normal (the mates' alignments agree with it), and the point
  // where SolidWorks has the face: one of several faces in that plane facing that way, the nearest
  if (e.referenceType === 2) return r > 0 ? { kind: "axis", o: p, d, r, near: null } : { kind: "plane", n: d, oriented: true, p, near: p };
  if (e.referenceType === 1) return r > 0 ? { kind: "axis", o: p, d, r } : { kind: "straight", o: p, d };
  if (e.referenceType === 3) return { kind: "point", p };
  if (e.referenceType === 4) return { kind: "refplane", n: d, p };
  return null;
}

/**
 * The face or edge of the part that is that shape: a {body, index} selector (or a vertex), with `dir`, the
 * direction PeetCAD's mates take for it in the part (a flat face's outward normal, an axis's direction), where
 * it has one. Null if there is none.
 */
function findIn(file, shape) {
  const { faces, edges } = geometryOf(file);
  const pick = (list, score) => {
    const scored = list.map(x => [x, score(x)]).filter(([, v]) => v !== null).sort((a, b) => a[1] - b[1]);
    return scored.length ? scored[0][0] : null;
  };
  const sel = x => ({ body: x.body, index: x.index });
  if (shape.kind === "plane") {
    const inPlane = faces.filter(f => f.surface === "plane" && f.normal && Math.abs(Math.abs(dot3(f.normal, shape.n)) - 1) < 1e-6
      && Math.abs(dot3(sub3(f.center, shape.p), shape.n)) < TOL);
    // Facing the way SolidWorks' face does, where that is known: of two faces in one plane (the two sides of a
    // slot, a sheet's face and the next one's back), the one SolidWorks means
    const facing = shape.oriented ? inPlane.filter(f => dot3(f.normal, shape.n) > 0) : inPlane;
    const face = pick(facing.length ? facing : inPlane, f => shape.near ? len3(sub3(f.center, shape.near)) : 0);
    return face && { face: sel(face), dir: face.normal };
  }
  if (shape.kind === "axis") {
    const face = pick(faces.filter(f => f.axis && f.axis_point && Math.abs(Math.abs(dot3(f.axis, shape.d)) - 1) < 1e-6
      && offLine(f.axis_point, shape.o, shape.d) < TOL && (!shape.r || f.radius === undefined || Math.abs(f.radius - shape.r) < TOL)),
      f => shape.near ? len3(sub3(f.center, shape.near)) : 0);
    if (face) return { face: sel(face), dir: face.axis };
    const edge = pick(edges.filter(g => g.curve === "circle" && Math.abs(Math.abs(dot3(g.axis, shape.d)) - 1) < 1e-6 && offLine(g.center, shape.o, shape.d) < TOL
      && (!shape.r || Math.abs(g.radius - shape.r) < TOL)), () => 0);
    return edge && { edge: sel(edge), dir: edge.axis };
  }
  if (shape.kind === "circle") {
    const edge = pick(edges.filter(g => g.curve === "circle" && Math.abs(Math.abs(dot3(g.axis, shape.n)) - 1) < 1e-6
      && len3(sub3(g.center, shape.c)) < TOL && Math.abs(g.radius - shape.r) < TOL), () => 0);
    return edge && { edge: sel(edge), dir: edge.axis };
  }
  if (shape.kind === "line") {
    // A straight edge's direction in PeetCAD's mates is its curve's (`direction`), which need not run from
    // `from` to `to`
    const edge = pick(edges.filter(g => g.curve === "line"), g => {
      const d = Math.min(len3(sub3(g.from, shape.a)) + len3(sub3(g.to, shape.b)), len3(sub3(g.from, shape.b)) + len3(sub3(g.to, shape.a)));
      return d < 2 * TOL ? d : null;
    });
    return edge && { edge: sel(edge), dir: edge.direction };
  }
  if (shape.kind === "refplane") {
    // A reference plane of the part in that plane (SolidWorks gives only its numbers), else a flat face in it
    const plane = referencePlanes(file).find(r => Math.abs(Math.abs(dot3(r.normal, shape.n)) - 1) < 1e-6
      && Math.abs(dot3(sub3(r.origin, shape.p), shape.n)) < TOL);
    if (plane) return { plane: plane.name, dir: plane.normal };
    return findIn(file, { kind: "plane", n: shape.n, p: shape.p, near: null });
  }
  if (shape.kind === "straight") {
    // A straight edge on a line: of several, the one SolidWorks' point is on (or nearest)
    const edge = pick(edges.filter(g => g.curve === "line" && g.direction && Math.abs(Math.abs(dot3(g.direction, shape.d)) - 1) < 1e-6
      && offLine(g.from, shape.o, shape.d) < TOL), g => len3(sub3(g.middle, shape.o)));
    return edge && { edge: sel(edge), dir: edge.direction };
  }
  if (shape.kind === "point") {
    const vertex = edges.flatMap(g => [g.from, g.to]).find(q => q && len3(sub3(q, shape.p)) < TOL);
    return vertex ? { vertex: { at: vertex.map(tidy) } } : null;
  }
  return null;
}

/**
 * The entity of a part's sketch that SolidWorks' sketch end is: `e.entity` names the sketch, and SolidWorks'
 * numbers for the end (its point, its direction, a circle's radius) are taken back into the part and set against
 * the sketch's entities where PeetCAD has them (its sketch axes need not be SolidWorks'). A sketch's own points
 * (a line's ends, a circle's centre) count as points. Returns {sketch, entity, element, dir} or null.
 */
const sketchCache = new Map();
function findSketchEntity(file, e, transform) {
  const key = file + "|" + e.entity.sketch;
  if (!sketchCache.has(key)) {
    sketchCache.set(key, askView(file, [{ op: "feature", feature: e.entity.sketch }])[0] || null);
  }
  const s = sketchCache.get(key);
  if (!s || !s.plane || !s.entities) return null;
  const { origin, x, y, normal } = s.plane;
  const at = q => [0, 1, 2].map(i => origin[i] + q[0] * x[i] + q[1] * y[i]);
  const shape = shapeFromParams({ ...e, referenceType: 3 }, transform);
  const p = shape.p;
  const d = norm3(toWorld({ R: inverse(matrixOf(transform)).R, t: [0, 0, 0] }, e.params.slice(3, 6)));
  const kind = e.entity.kind === "sketchPoint" ? "point" : e.entity.type;
  if (kind === "point") {
    const points = [];
    for (const n of s.entities) {
      if (n.type === "point") points.push([n.id, n.at]);
      if (n.type === "line") points.push([n.start, n.from], [n.end, n.to]);
      if (n.type === "circle" || n.type === "arc") points.push([n.center, n.at]);
      if (n.type === "arc") points.push([n.start, n.from], [n.end, n.to]);
    }
    const found = points.find(([, q]) => len3(sub3(at(q), p)) < TOL);
    return found ? { sketch: s.name, entity: found[0], element: "point" } : null;
  }
  if (kind === "arc" || kind === "circle") {
    const r = e.params[6] * MM;
    const found = s.entities.find(n => (n.type === "circle" || n.type === "arc") && Math.abs(Math.abs(dot3(normal, d)) - 1) < 1e-6
      && offLine(at(n.at), p, d) < TOL && (n.type === "circle" ? Math.abs(n.radius - r) < TOL : Math.abs(len3(sub3(at(n.from), at(n.at))) - r) < TOL));
    return found ? { sketch: s.name, entity: found.id, element: "axis", dir: normal } : null;
  }
  if (kind === "line") {
    const found = s.entities.find(n => n.type === "line" && offLine(at(n.from), p, d) < TOL && offLine(at(n.to), p, d) < TOL);
    return found ? { sketch: s.name, entity: found.id, element: "axis", dir: norm3(sub3(at(found.to), at(found.from))) } : null;
  }
  return null;
}

/**
 * The sketch curves of a part as PeetCAD has them, in the part's coordinates (mm): for each line, arc and circle
 * of each sketch, {sketch, entity, kind, construction, a, b (a line's ends; an arc's), c, n, r (a circle's or
 * arc's centre, normal and radius)}, and the sketch's `used` (SolidWorks' part export has features made of it).
 */
const curveCache = new Map();
function sketchCurves(file) {
  if (curveCache.has(file)) return curveCache.get(file);
  const sketches = ((askView(file, [{ op: "features" }])[0] || {}).features || []).filter(f => /sketch/i.test(f.type || ""));
  const replies = sketches.length ? askView(file, sketches.map(f => ({ op: "feature", feature: f.id }))) : [];
  const sw = exportOf(path.basename(splitView(file)[0], ".peet") + ".sldprt");
  const swSketches = new Map();
  if (sw) {
    let exp = null;
    try { exp = JSON.parse(fs.readFileSync(sw)); } catch { }
    const cfg = exp && (exp.configurations || [])[0];
    walkFeatures(cfg && cfg.features, x => swSketches.set(x.name, x));
  }
  const out = [];
  replies.forEach((s, i) => {
    if (!s || !s.plane || !s.entities) return;
    const { origin, x, y, normal } = s.plane;
    const at = q => [0, 1, 2].map(k => origin[k] + q[0] * x[k] + q[1] * y[k]);
    const own = swSketches.get(s.name || sketches[i].name);
    const used = !!(own && (own.children || []).length);
    for (const n of s.entities) {
      const base = { sketch: s.name || sketches[i].name, entity: n.id, construction: !!n.construction, used };
      if (n.type === "line") out.push({ ...base, kind: "line", a: at(n.from), b: at(n.to) });
      else if (n.type === "circle") out.push({ ...base, kind: "circle", c: at(n.at), n: normal, r: n.radius });
      else if (n.type === "arc") out.push({ ...base, kind: "arc", c: at(n.at), n: normal, r: len3(sub3(at(n.from), at(n.at))), a: at(n.from), b: at(n.to) });
    }
  });
  curveCache.set(file, out);
  return out;
}

/** How far a point is from a sketch curve of `sketchCurves` (mm). */
function offCurve(p, g) {
  if (g.kind === "line") {
    const d = sub3(g.b, g.a), l = len3(d);
    if (l < 1e-9) return len3(sub3(p, g.a));
    const t = Math.max(0, Math.min(l, dot3(sub3(p, g.a), d) / l));
    return len3(sub3(p, g.a.map((v, k) => v + d[k] * t / l)));
  }
  const v = sub3(p, g.c), h = dot3(v, g.n), q = sub3(v, g.n.map(x => x * h)), ql = len3(q);
  const radial = Math.hypot(h, ql - g.r);
  if (g.kind === "circle" || ql < 1e-9) return radial;
  // On the arc's circle, between its ends (counter-clockwise about the normal from a to b)
  const ang = w => { const u = sub3(w, g.c); return Math.atan2(dot3(cross3(sub3(g.a, g.c), u), g.n), dot3(sub3(g.a, g.c), u)); };
  const wrap = t => (t + 2 * Math.PI) % (2 * Math.PI);
  const end = wrap(ang(g.b)) || 2 * Math.PI, here = wrap(ang(p));
  return here <= end + 1e-9 ? radial : Math.min(len3(sub3(p, g.a)), len3(sub3(p, g.b)));
}
const cross3 = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];

/**
 * A path mate's path as PeetCAD ends on `comp`'s part `file`. Format 6 exports what editing the mate selects
 * (`pathSelections`): the sketch segments (or edges) of the path. Format 5 has nothing for it: then the path is
 * the sketch curve the point is on where SolidWorks has it (a sketch no feature is made of, and not a
 * construction line, first), continued by the curves of its sketch joined to it end to end, one way on.
 * Returns {ends, how} or {error}.
 */
function pathEnds(f, comp, file, pointAt) {
  const curves = sketchCurves(file);
  const component = comp.path.split("/");
  const endFor = g => ({ component, sketch: g.sketch, entity: g.entity });
  const sel = ((f.pathSelections || {}).selections || []).filter(s => s.entity && s.component && s.component.split("/")[0] === component[0]);
  if (sel.length) {
    const back = inverse(matrixOf(comp.transform));
    const mm = p => p.map(v => v * MM);
    const ends = [];
    for (const s of sel) {
      const e = s.entity;
      if (e.kind === "sketchSegment") {
        // In its sketch's own coordinates: found among the curves of the sketch of that name by its ends' or its
        // centre's distances from the others, which no sketch frame changes
        const pts = [e.start, e.end, e.center].filter(Boolean).map(q => [q.x * MM, q.y * MM]);
        const span = (list) => list.length > 1 ? Math.hypot(list[0][0] - list[1][0], list[0][1] - list[1][1]) : null;
        const want = e.type === "arc" ? { kind: "arc", r: e.radius * MM, chord: span(pts) } : { kind: "line", length: span(pts) };
        const hits = curves.filter(g => g.sketch === e.sketch && (want.kind === "line" ? g.kind === "line" && Math.abs(len3(sub3(g.b, g.a)) - want.length) < TOL
          : (g.kind === "arc" || g.kind === "circle") && Math.abs(g.r - want.r) < TOL));
        if (hits.length !== 1) return { error: `the path's ${e.type} in ${e.sketch} is ${hits.length ? "one of several" : "not"} in ${viewName(file)}'s sketch` };
        ends.push(endFor(hits[0]));
      } else if (e.kind === "edge") {
        // An edge picked in the assembly: its geometry may be the assembly's or the part's
        const shapes = [shapeOf({ entity: e }, comp)];
        const moved = { ...e, start: e.start && toWorld(back, e.start), end: e.end && toWorld(back, e.end),
          circle: e.circle && [...toWorld(back, e.circle.slice(0, 3)), ...toWorld({ R: back.R, t: [0, 0, 0] }, e.circle.slice(3, 6)), e.circle[6]] };
        shapes.push(shapeOf({ entity: moved }, comp));
        const found = shapes.filter(Boolean).map(sh => findIn(file, sh)).find(x => x && x.edge);
        if (!found) return { error: `an edge of the path is not in ${viewName(file)}` };
        ends.push({ component, edge: found.edge });
      } else return { error: `a path piece SolidWorks gives as ${e.kind}` };
    }
    return { ends, how: "from the mate's selections" };
  }
  if (!pointAt) return { error: "the export has no path (format 5) and no place for the point" };
  const on = curves.filter(g => offCurve(pointAt, g) < TOL);
  const rank = g => (g.used ? 2 : 0) + (g.construction ? 1 : 0);
  const best = Math.min(...on.map(rank));
  const first = on.filter(g => rank(g) === best);
  if (first.length !== 1) return { error: first.length ? `the point is on ${first.length} sketch curves of ${viewName(file)} alike (${first.map(g => g.sketch).join(", ")}); the export (format 5) has no path` : `the point is on no sketch curve of ${viewName(file)}; the export (format 5) has no path` };
  // Continued end to end through its sketch while just one curve goes on from an end
  const chain = [first[0]];
  const endsOf = g => g.kind === "circle" ? [] : [g.a, g.b];
  let open = endsOf(first[0]);
  for (let guard = 0; guard < 100 && open.length; guard++) {
    const next = [];
    for (const p of open) {
      const touching = curves.filter(g => g.sketch === first[0].sketch && !chain.includes(g) && rank(g) === best && endsOf(g).some(q => len3(sub3(p, q)) < TOL));
      if (touching.length === 1) { chain.push(touching[0]); next.push(...endsOf(touching[0]).filter(q => len3(sub3(p, q)) >= TOL)); }
    }
    open = next;
  }
  return { ends: chain.map(endFor), how: `the export (format 5) has no path: the ${first[0].kind} of ${first[0].sketch} the point is on where SolidWorks has it${chain.length > 1 ? `, and ${chain.length - 1} joined to it` : ""}` };
}

/** The reference planes of a part, with where each is: {name, origin, normal}. */
const refPlaneCache = new Map();
function referencePlanes(file) {
  if (refPlaneCache.has(file)) return refPlaneCache.get(file);
  const features = ((askView(file, [{ op: "features" }])[0] || {}).features || []).filter(f => /plane|coordinate/i.test(f.type));
  const replies = features.length ? askView(file, features.map(f => ({ op: "feature", feature: f.id }))) : [];
  const planes = features.map((f, i) => ({ name: f.name, plane: (replies[i] || {}).plane }))
    .filter(f => f.plane).map(f => ({ name: f.name, origin: f.plane.origin, normal: f.plane.normal }));
  refPlaneCache.set(file, planes);
  return planes;
}

/** A reference plane's normal in its part, as PeetCAD has it. */
const planeNormals = new Map();
function planeNormal(file, plane) {
  if (STANDARD_NORMALS[plane]) return STANDARD_NORMALS[plane];
  const key = file + "|" + plane;
  if (!planeNormals.has(key)) {
    const reply = askView(file, [{ op: "feature", feature: plane }])[0] || {};
    planeNormals.set(key, reply.plane ? reply.plane.normal : null);
  }
  return planeNormals.get(key);
}

/**
 * The direction SolidWorks' alignment refers to at an end, in its part: its numbers for the end in the
 * assembly (a face's outward normal, an axis's direction) taken back into the part, or failing those its
 * description of the face.
 */
function swDirection(e, comp) {
  if (e.params && comp && len3(e.params.slice(3, 6)) > 0.5) {
    const back = inverse(matrixOf(comp.transform));
    return norm3(toWorld({ R: back.R, t: [0, 0, 0] }, e.params.slice(3, 6)));
  }
  const x = e.entity;
  if (x && x.normal) return norm3(x.normal);
  if (x && x.cylinder) return norm3(x.cylinder.slice(3, 6));
  if (x && x.circle) return norm3(x.circle.slice(3, 6));
  return null;
}

/**
 * A mate end in PeetCAD's terms: the component path (none for the assembly's own plane or origin), and the
 * face, edge, vertex, plane, origin or sketch entity of its part. With it, what it is to a mate (`element`:
 * plane, axis or point) and `sign`: whether PeetCAD's direction for it is SolidWorks' (1) or the other way
 * (-1), so that SolidWorks' alignment and flipped dimension can be said in PeetCAD's terms.
 */
function endOf(e, byPath, assembly, partFile, others = []) {
  const own = !e.component || e.component === assembly;
  const isOrigin = e.entity && e.entity.kind === "sketchPoint" && e.entity.sketch === "Origin";
  const isPlane = e.entity && e.entity.kind === "feature" && e.entity.type === "RefPlane";
  if (own) {
    // The assembly's own standard planes and origin, fixed where the assembly is
    if (isOrigin) return { origin: true, element: "point" };
    if (isPlane && STANDARD_PLANES[e.entity.name]) {
      const plane = STANDARD_PLANES[e.entity.name];
      const sw = e.params && len3(e.params.slice(3, 6)) > 0.5 ? norm3(e.params.slice(3, 6)) : null;
      const d = sw && dot3(STANDARD_NORMALS[plane], sw);
      return { plane, element: "plane", sign: d && Math.abs(Math.abs(d) - 1) < 1e-4 ? Math.sign(d) : null };
    }
    if (isPlane) return { error: `an end on the assembly's own reference plane ${e.entity.name}, which PeetCAD's assemblies don't have` };
    if (e.entity || !e.params) return { error: `an end on the assembly itself that is neither a standard plane nor the origin (${e.entity ? e.entity.kind : "nothing"})` };
    // SolidWorks names the assembly and gives no entity, only its numbers: geometry of a component it could not
    // resolve. The component whose part has that face or edge where SolidWorks' numbers put it is the one.
    const hits = [];
    for (const comp of byPath.values()) {
      // Not on a top-level component the mate's other ends are on: a mate joins two
      if (others.some(o => o.split("/")[0] === comp.path.split("/")[0])) continue;
      const file = /\.sldprt$/i.test(comp.file || "") ? partFile(comp) : null;
      const shape = file && shapeFromParams(e, comp.transform);
      const found = shape && findIn(file, shape);
      if (found) hits.push(comp.path);
    }
    if (!hits.length) return { error: "an end SolidWorks names the assembly for, with no entity: on a component it couldn't resolve (no face or edge of the assembly's parts is where its numbers put it)", unmatched: true };
    const end = endOf({ ...e, component: hits[0] }, byPath, assembly, partFile);
    return end.error ? end : { ...end, guessed: true, note: `SolidWorks names the assembly for an end with no entity; ${hits.length > 1 ? `of ${hits.join(", ")}, ` : ""}${hits[0]} has it` };
  }
  const comp = byPath.get(e.component);
  if (!comp) return { error: `its component ${e.component} is not in the assembly` };
  const component = e.component.split("/");
  const signOf = dir => {
    const sw = swDirection(e, comp);
    if (!dir || !sw) return null;
    const d = dot3(norm3(dir), sw);
    return Math.abs(Math.abs(d) - 1) < 1e-4 ? Math.sign(d) : null;
  };
  if (isOrigin) return { component, origin: true, element: "point" };
  if (isPlane) {
    const plane = STANDARD_PLANES[e.entity.name] || e.entity.name;
    const file = partFile(comp);
    return { component, plane, element: "plane", sign: file ? signOf(planeNormal(file, plane)) : null };
  }
  const file = partFile(comp);
  if (e.entity && /^sketch/.test(e.entity.kind)) {
    if (!file) return { error: "its part was not imported" };
    if (!e.params) return { error: "a sketch end SolidWorks gives no numbers for" };
    const found = findSketchEntity(file, e, comp.transform);
    if (!found) return { error: `no ${e.entity.type || "point"} of ${e.entity.sketch} in ${viewName(file)} is where SolidWorks' is`, unmatched: true };
    const { dir, element, ...where } = found;
    return { component, ...where, element, sign: signOf(dir) };
  }
  const shape = shapeOf(e, comp);
  if (!shape) return { error: `an end PeetCAD has no match for (${e.entity ? e.entity.kind : "reference type " + e.referenceType})` };
  if (!file) return { error: "its part was not imported" };
  // A round edge SolidWorks gives only numbers for: the circle where it has one, else a round face on its axis
  let found = null;
  if (!e.entity && e.referenceType === 1 && shape.kind === "axis") found = findIn(file, { kind: "circle", c: shape.o, n: shape.d, r: shape.r });
  found = found || findIn(file, shape);
  if (!found) return { error: `no ${shape.kind === "plane" ? "flat face" : shape.kind === "axis" ? "round face or edge on that axis" : shape.kind === "straight" ? "straight edge on that line" : shape.kind} of ${viewName(file)} is where SolidWorks' is`, unmatched: true };
  const { dir, ...where } = found;
  const element = shape.kind === "plane" || shape.kind === "refplane" ? "plane" : shape.kind === "point" ? "point" : "axis";
  return { component, ...where, element, sign: signOf(dir), ...(e.entity ? {} : { guessed: true }) };
}

// ---- Equations ----

/** A component's name as SolidWorks writes it in equations (`Door<1>.Part`, `Door-1.Part`), as PeetCAD's. */
const componentName = t => {
  const bare = t.trim().replace(/\.(part|assembly|sldprt|sldasm)$/i, "");
  const m = /^(.*)<(\d+)>$/.exec(bare);
  return m ? `${m[1]}-${m[2]}` : bare;
};
const isComponentRef = t => /\.(part|assembly)$/i.test(t.trim()) || /<\d+>$/.test(t.trim());
const WORD = /^[A-Za-z_][A-Za-z0-9_]*$/;
const globalName = t => (t = t.trim()) && !/["@]/.test(t) ? t : t.replace(/[^A-Za-z0-9_]/g, "_").replace(/^(?=\d)/, "_");
const inExpression = n => WORD.test(n) ? n : `"${n}"`;
const stripEnd = e => { const { guessed, element, sign, ...rest } = e; return rest; };

/**
 * The two faces a reference dimension of the assembly measures between. The export has only its value and
 * direction: of the flat mate ends facing along it, a pair on two components that far apart, preferring one
 * on a component the equation's target is mated to. All such pairs measure the same while the assembly holds.
 */
function referenceEnds(cfg, rd, target, byPath, name, partFile) {
  const L = rd.lineDirection;
  const ends = [];
  const matedTo = new Set();
  walkFeatures(cfg.features, f => {
    if (!/^Mate/.test(f.type) || !f.mate || f.suppressed) return;
    const comps = (f.mate.entities || []).map(e => e.component);
    if (target && comps.includes(target)) comps.forEach(c => c && c !== target && matedTo.add(c));
    for (const e of f.mate.entities || []) {
      if (!e.params || e.referenceType !== 2 || !e.component || e.component === name) continue;
      if (Math.abs(Math.abs(dot3(e.params.slice(3, 6), L)) - 1) > 1e-6) continue;
      ends.push({ e, h: dot3(e.params.slice(0, 3), L) });
    }
  });
  const pairs = [];
  for (const x of ends) for (const y of ends) {
    if (x.e.component === y.e.component || x.e.component === target || y.e.component === target) continue;
    if (x.h >= y.h || Math.abs(y.h - x.h - rd.value) > 1e-7) continue;
    pairs.push([x, y]);
  }
  const near = p => matedTo.has(p[0].e.component) || matedTo.has(p[1].e.component) ? 1 : 0;
  pairs.sort((p, q) => near(q) - near(p));
  for (const [x, y] of pairs) {
    const a = endOf(x.e, byPath, name, partFile), b = endOf(y.e, byPath, name, partFile);
    if (!a.error && !b.error) return { a: stripEnd(a), b: stripEnd(b) };
  }
  // Failing those, any two flat faces of the components (where SolidWorks has them) that far apart
  const faces = [];
  for (const c of cfg.components || []) {
    if (c.path.includes("/") || c.path === target || c.suppression === 0) continue;
    const file = partFile(c);
    if (!file) continue;
    const m = matrixOf(c.transform);
    for (const f of geometryOf(file).faces) {
      if (f.surface !== "plane" || !f.normal) continue;
      const n = toWorld({ R: m.R, t: [0, 0, 0] }, f.normal);
      if (Math.abs(Math.abs(dot3(n, L)) - 1) > 1e-6) continue;
      faces.push({ c: c.path, h: dot3(toWorld(m, f.center.map(v => v / MM)), L), sel: { component: [c.path], face: { body: f.body, index: f.index } } });
    }
  }
  const found = [];
  for (const x of faces) for (const y of faces) {
    if (x.c !== y.c && x.h < y.h && Math.abs(y.h - x.h - rd.value) <= 1e-7) found.push([x, y]);
  }
  const nearFace = p => matedTo.has(p[0].c) || matedTo.has(p[1].c) ? 1 : 0;
  found.sort((p, q) => nearFace(q) - nearFace(p));
  return found.length ? { a: found[0][0].sel, b: found[0][1].sel, guessed: true } : null;
}

/**
 * An assembly's equations, as SolidWorks has them: global variables become parameters (a disabled one keeps its
 * value, as in SolidWorks), a component's dimension set by an equation an equation of the assembly (`set` with
 * an expression), a component suppressed by a condition a suppression expression, and the reference dimensions
 * they read reference dimensions. A disabled equation is not imported: SolidWorks doesn't apply it. Each is kept
 * only if PeetCAD works it out to SolidWorks' value.
 */
function importEquations(peet, cfg, report, byPath, want, name) {
  report.equations = [];
  const partFile = comp => /\.sldprt$/i.test(comp.file || "") ? partOf(comp).view || null : null;
  const globals = new Map();
  const references = new Set();
  const annotations = ((cfg.features || []).find(f => f.type === "DetailCabinet") || {}).dimensions || [];
  const parse = e => /^\s*"([^"]+)"\s*=\s*([\s\S]*)$/.exec(e.expression);
  // Disabled global variables keep their value
  for (const e of cfg.equations) {
    const m = parse(e);
    if (!m || !e.disabled || !e.globalVariable || isComponentRef(m[1]) || m[1].includes("@")) continue;
    const n = globalName(m[1]);
    if (peet.apply({ op: "set_parameter", name: n, value: String(e.value || 0) }).ok) globals.set(m[1].trim(), inExpression(n));
  }
  const translate = (text, target) => {
    let failed = null;
    let out = text.replace(/"([^"]*)"/g, (whole, inner) => {
      const ref = inner.trim();
      if (/^(un)?suppressed$/i.test(ref)) return /^un/i.test(ref) ? "0" : "1";
      if (/@Annotations$/i.test(ref)) {
        const rdName = ref.split("@")[0];
        if (!references.has(rdName)) {
          const rd = annotations.find(d => d.name === rdName);
          const ends = rd && referenceEnds(cfg, rd, target, byPath, name, partFile);
          if (!ends) { failed = `it reads ${ref}, whose ends were not found`; return "0"; }
          const { guessed, ...between } = ends;
          if (guessed) report.notes.push(`${rdName}@Annotations is measured between two faces that are as far apart as SolidWorks' is, which may not be the ones it measures (the export does not say)`);
          const reply = peet.apply({ op: "reference_dimension", name: rdName, ...between });
          const v = reply.reference && reply.reference.value;
          if (!reply.ok || typeof v !== "number" || Math.abs(v - rd.value * MM) > 1e-4) {
            failed = `its reference dimension ${rdName} measures ${v} where SolidWorks' is ${tidy(rd.value * MM)}`;
            return "0";
          }
          references.add(rdName);
        }
        return `"${ref}"`;
      }
      if (ref.includes("@")) {
        // A dimension of a component (PeetCAD reads SolidWorks' form of it), or a mate's value
        if (!peet.ask({ op: "dimensions", name: ref }).ok) { failed = `it reads ${ref}, which PeetCAD's part does not have`; return "0"; }
        return `"${ref}"`;
      }
      if (!globals.has(ref)) { failed = `it reads the variable ${ref}, which is not defined`; return "0"; }
      return globals.get(ref);
    });
    out = out.replace(/\b(un)?suppressed\b/gi, w => /^un/i.test(w) ? "0" : "1");
    out = out.replace(/([<>])\s+(?=[=>])/g, "$1");
    return failed ? { failed } : { value: out.trim() };
  };
  for (const e of cfg.equations) {
    const row = { equation: e.expression.trim(), value: e.value };
    report.equations.push(row);
    const m = parse(e);
    if (!m) { row.result = "failed"; row.why = "not understood"; continue; }
    const target = m[1].trim();
    if (e.disabled) {
      row.result = "disabled";
      if (globals.has(target)) row.note = "a parameter with SolidWorks' value";
      continue;
    }
    const mark = peet.backup();
    const fail = why => { peet.restore(mark); row.result = "failed"; row.why = why; };
    const suppression = isComponentRef(target) && !target.includes("@");
    const last = target.split("@").pop();
    const component = suppression ? componentName(target) : target.includes("@") && isComponentRef(last) ? componentName(last) : null;
    if (component && !want.has(component)) { fail(`it sets something of ${component}, which was not imported`); continue; }
    const t = translate(m[2], component);
    if (t.failed) { fail(t.failed); continue; }
    if (suppression) {
      const reply = peet.apply({ op: "set_suppression_expression", component, value: t.value });
      if (!reply.ok) { fail(reply.error); continue; }
      const c = (peet.ask({ op: "components" }).components || []).find(x => x.name === component);
      const swSuppressed = (byPath.get(component) || {}).suppression === 0;
      if (!c || (c.status === "suppressed") !== swSuppressed) { fail(`PeetCAD has ${component} ${c && c.status === "suppressed" ? "suppressed" : "not suppressed"} by it, SolidWorks the other way`); continue; }
    } else if (e.globalVariable && !target.includes("@")) {
      const n = globalName(target);
      const reply = peet.apply({ op: "set_parameter", name: n, value: t.value || String(e.value || 0) });
      if (!reply.ok) { fail(reply.error); continue; }
      globals.set(target, inExpression(n));
    } else if (target.includes("@")) {
      const reply = peet.apply({ op: "set", name: target, value: t.value });
      if (!reply.ok) { fail(reply.error); continue; }
      const now = peet.ask({ op: "dimensions", name: target });
      const v = now.ok && now.dimensions && now.dimensions[0] && now.dimensions[0].value;
      if (typeof v !== "number" || Math.abs(v - e.value) > 1e-4 * Math.max(1, Math.abs(e.value))) { fail(`PeetCAD works it out to ${v} where SolidWorks has ${tidy(e.value)}`); continue; }
    } else { fail("it sets something PeetCAD has no address for"); continue; }
    row.result = "imported";
  }
  const live = report.equations.filter(r => r.result !== "disabled");
  const left = live.filter(r => r.result !== "imported").length;
  if (left) report.notes.push(`${left} of ${live.length} live equations not imported`);
}

// ---- Import ----

const reports = [];
const imported = new Map();   // export file -> { peet, report }

function walkFeatures(list, f) { for (const x of list || []) { f(x); walkFeatures(x.sub, f); } }

/** The mates of a configuration of an assembly's export, by name, in the tree's order. */
function matesOf(cfg) {
  const out = new Map();
  if (cfg) walkFeatures(cfg.features, f => { if (/^Mate/.test(f.type) && f.type !== "MateGroup" && f.mate) out.set(f.name, f); });
  return out;
}

/** `place` for a component (a name, or a path into a flexible sub-assembly) at SolidWorks' placement `m`. */
function placeOp(component, m) {
  // Not turned is said too: `place` without `rotate` keeps the turn the component has
  const rot = axisAngle(m) || { axis: [0, 0, 1], angle: 0 };
  return { op: "place", component, at: m.t.map(v => tidy(v * MM)), rotate: rot };
}

/**
 * Where SolidWorks has a top-level component, as a placement in the assembly's coordinates. A sub-assembly goes
 * where the components fixed in it are: SolidWorks' own transform for a sub-assembly it solves as flexible need
 * not be (the template hinges' are hundreds of millimetres from their fixed leaf). Returns the placement and, if
 * it is not SolidWorks' transform, why.
 */
function placementOf(c, all, placeOf, local) {
  const anchor = /\.sldasm$/i.test(c.file || "") ? anchorOf(c, all) : null;
  let m = placeOf(c);
  let note = null;
  if (anchor) {
    const at = local ? then(anchor, local) : anchor;
    const d = apart(at, m);
    if (d > 1e-3) { note = `its fixed component ${anchor.by}, ${tidy(d)} mm from SolidWorks' transform for the sub-assembly`; m = at; }
  }
  return { m, note };
}

/** Where SolidWorks has the components inside a flexible sub-assembly component `c`, by path: [path, placement]. */
function innerPlacements(c, all, placeOf, local) {
  return all.filter(x => x.path.startsWith(c.path + "/")).map(x => {
    // A sub-assembly in it, where its own fixed component is, as above
    const anchorX = /\.sldasm$/i.test(x.file || "") ? anchorOf(x, all) : null;
    return [x.path, anchorX ? (local ? then(anchorX, local) : anchorX) : placeOf(x)];
  });
}

/** Whether a sub-assembly's export has a configuration of that name (true where there is no export to tell). */
function subHas(file, configuration) {
  const exp = exportOf(file);
  if (!exp || !configuration) return true;
  try { return (JSON.parse(fs.readFileSync(exp)).configurations || []).some(x => x.name === configuration); } catch { return true; }
}

/**
 * The `mate` operation for SolidWorks' mate `f`, its ends found in the parts of the components of `byPath` (one
 * configuration's), or null with `row.result` and `row.why` saying why not.
 */
function mateOp(f, byPath, name, want, row) {
  const kind = MATE_KINDS[f.type];
  if (!kind) { row.result = "not supported"; row.why = WHY_NOT[f.type] || `PeetCAD has no ${f.type.replace(/^Mate/, "")} mate (parity A4)`; return null; }
  const partFile = comp => /\.sldprt$/i.test(comp.file || "") ? partOf(comp).view || null : null;
  const named = (f.mate.entities || []).map(e => e.component).filter(c => c && c !== name);
  if (kind === "path") return pathOp(f, byPath, name, want, row, partFile, named);
  const ends = (f.mate.entities || []).map(e => endOf(e, byPath, name, partFile, named));
  const bad = ends.find(e => e.error);
  if (bad) { row.result = bad.unmatched ? "failed" : "not supported"; row.why = bad.error; return null; }
  // An end with no component is on the assembly's own plane or origin
  const comps = ends.filter(e => e.component);
  if (comps.some(e => !want.has(e.component[0]))) { row.result = "left out"; row.why = "a component it joins was left out"; return null; }
  if (ends.some(e => e.guessed)) row.guessed = true;
  const notes = ends.map(e => e.note).filter(Boolean);
  if (notes.length) row.ends = notes;
  const strip = e => { const { guessed, element, sign, note, ...rest } = e; return rest; };
  // A component SolidWorks has suppressed: its mates wait for it, as SolidWorks' do
  row.waits = comps.some(e => (byPath.get(e.component[0]) || {}).suppression === 0) ? "a component it joins is suppressed" : undefined;
  const values = (f.definition && f.definition.values) || {};
  let op;
  if (kind === "width") {
    // SolidWorks lists the width's pair, then the tab: two faces, or one round face or axis. Either pair may
    // be on two components.
    if (ends.length !== 4 && ends.length !== 3) { row.result = "not supported"; row.why = `a width mate of ${ends.length} ends`; return null; }
    const width = ends.slice(0, 2), tab = ends.slice(2);
    if (tab.length === 1 && tab[0].element !== "axis") { row.result = "not supported"; row.why = "a width mate whose one tab end is not round"; return null; }
    op = { op: "mate", type: "width", name: f.name, a: width.map(strip), b: tab.length === 1 ? strip(tab[0]) : tab.map(strip) };
    // swWidthMateConstraintType_e: 0 centred, 1 free, 2 dimension, 3 percent
    const v = values;
    if (v.ConstraintType === 1) op.mode = "free";
    else if (v.ConstraintType === 2) { op.mode = "dimension"; op.distance = tidy(v.DistanceFromEnd * MM); }
    else if (v.ConstraintType === 3) { op.mode = "percent"; op.percent = tidy(v.PercentDistanceFromEnd * 100); }
    if (v.FlipDimension && op.mode && op.mode !== "free") op.a.reverse();
  } else if (kind === "symmetric") {
    // The two ends, then the plane of symmetry
    if (ends.length !== 3) { row.result = "not supported"; row.why = `a symmetric mate of ${ends.length} ends`; return null; }
    op = { op: "mate", type: "symmetric", name: f.name, a: strip(ends[0]), b: strip(ends[1]), plane: strip(ends[2]) };
  } else {
    if (ends.length !== 2) { row.result = "not supported"; row.why = `${ends.length} ends`; return null; }
    const [ea, eb] = ends;
    op = { op: "mate", type: kind, name: f.name, a: strip(ea), b: strip(eb) };
    if (f.type === "MateCoordinate") row.note = "SolidWorks' coordinate mate of two origins: the points together (whether it also lines up their axes is not exported)";
    const same = ea.element === eb.element && (ea.element === "plane" || ea.element === "axis");
    const known = ea.sign && eb.sign;
    if (kind === "slot") {
      // No constraint type is exported for any of the library's slot mates: SolidWorks' default, free
      // (swSlotMateConstraintType_e: 0 free, 1 centre, 2 distance along, 3 percent along)
      if (values.ConstraintType === 1) op.mode = "centered";
      else if (values.ConstraintType === 2) { op.mode = "dimension"; op.distance = tidy(values.DistanceFromEnd * MM); }
      else if (values.ConstraintType === 3) { op.mode = "percent"; op.percent = tidy(values.PercentDistanceFromEnd * 100); }
      if (values.FlipDimension && op.mode) op.flip_dimension = true;
    } else if (kind === "limit_angle" || kind === "limit_distance") {
      const angle = kind === "limit_angle";
      const [lo, hi] = angle ? [values.MinimumAngle, values.MaximumAngle] : [values.MinimumDistance, values.MaximumDistance];
      if (typeof lo !== "number" || typeof hi !== "number") { row.result = "failed"; row.why = "its limits were not exported"; return null; }
      const scale = angle ? 180 / Math.PI : MM;
      op.min = tidy(lo * scale);
      op.max = tidy(hi * scale);
      // Measured between the ends' directions as SolidWorks has them, or with the second turned round when it
      // flips the dimension; PeetCAD's directions may point the other way (`sign`)
      let flip = !!values.FlipDimension;
      if (angle && known && ea.sign * eb.sign < 0) flip = !flip;
      if (angle && !known) row.note = "the side the angle is measured on is SolidWorks', not checked against PeetCAD's directions";
      if (flip) op.flip_dimension = true;
    } else if (same && f.mate.alignment === 2) {
      // swMateAlign_e: 0 aligned, 1 anti-aligned, 2 closest, of the ends' directions as SolidWorks has them;
      // PeetCAD's may point the other way (a reference plane's, an axis's), which `sign` says.
      if (ea.element === "plane") op.align = "closest";
    } else if (same && known && (f.mate.alignment === 0 || f.mate.alignment === 1)) {
      const aligned = (f.mate.alignment === 0 ? 1 : -1) * ea.sign * eb.sign > 0;
      if (aligned) op.flip = true;
      else if (ea.element === "axis") op.align = "anti_aligned";
    } else if (same && ea.element === "plane" && f.mate.alignment === 0) {
      op.flip = true;
      row.note = "the alignment is SolidWorks', not checked against PeetCAD's directions";
    }
    if (kind === "distance") {
      const d = (f.dimensions || [])[0];
      if (!d) { row.result = "failed"; row.why = "its distance was not exported"; return null; }
      op.distance = tidy(d.value * MM);
      // SolidWorks' distance from a plane is along its outward normal unless the dimension is flipped;
      // PeetCAD measures from the first end if it is a plane, else from the second
      const from = ea.element === "plane" ? ea : eb.element === "plane" ? eb : null;
      if (from && (f.mate.flipped ? 1 : -1) * (from.sign || 1) > 0) op.flip_dimension = true;
      if (from && !from.sign) row.note = "which side the distance is on is SolidWorks', not checked against PeetCAD's directions";
    }
  }
  if (f.suppressed) op.suppressed = true;
  return op;
}

/**
 * The `mate` operation for SolidWorks' path mate `f`: the point (its first end), the path (`pathEnds`), and where
 * along it. Format 5 exports neither SolidWorks' path constraint (free, a distance or a percentage along the
 * path) nor which end a distance is from, only the dimension D1, which a path mate has whichever it is: so the
 * mate is tried as D1 from the path's start, from its end, and free (`row.alternatives`, tried by `addMate`),
 * and the one that leaves the components where SolidWorks has them is kept.
 */
function pathOp(f, byPath, name, want, row, partFile, named) {
  const [pe, ge] = f.mate.entities || [];
  if (!pe || !ge) { row.result = "not supported"; row.why = "a path mate without two ends"; return null; }
  let point = endOf(pe, byPath, name, partFile, named);
  // A sketch point the part's import doesn't have (a sketch of a point alone, which no feature uses): a vertex
  // there, or the centre of a round edge there, is the same point
  const pointComp = pe.component && byPath.get(pe.component);
  const pointFile = pointComp && partFile(pointComp);
  if (point.error && pe.params && pointFile) {
    const p = toWorld(inverse(matrixOf(pointComp.transform)), pe.params.slice(0, 3)).map(v => v * MM);
    const { edges } = geometryOf(pointFile);
    const vertex = edges.flatMap(g => [g.from, g.to]).find(q => q && len3(sub3(q, p)) < TOL);
    const round = edges.find(g => g.curve === "circle" && g.center && len3(sub3(g.center, p)) < TOL);
    const component = pe.component.split("/");
    if (vertex) point = { component, vertex: { at: vertex.map(tidy) }, element: "point" };
    else if (round) point = { component, edge: { body: round.body, index: round.index }, element: "point" };
    if (!point.error) row.ends = [`its point (${pe.entity && pe.entity.sketch ? `in ${pe.entity.sketch}, which ${viewName(pointFile)} hasn't` : "not found"}) as the ${vertex ? "vertex" : "centre of the round edge"} where SolidWorks has it`];
  }
  if (point.error) { row.result = point.unmatched ? "failed" : "not supported"; row.why = point.error; return null; }
  if (point.element !== "point") { row.result = "not supported"; row.why = "a path mate whose first end is not a point"; return null; }
  const comp = byPath.get(ge.component);
  if (!comp) { row.result = "not supported"; row.why = `its path's component ${ge.component} is not in the assembly`; return null; }
  const file = partFile(comp);
  if (!file) { row.result = "not supported"; row.why = "the path's part was not imported"; return null; }
  for (const c of [point.component, ge.component.split("/")]) if (c && !want.has(c[0])) { row.result = "left out"; row.why = "a component it joins was left out"; return null; }
  // Where SolidWorks has the point, in the path's part
  const pointAt = pe.params ? toWorld(inverse(matrixOf(comp.transform)), pe.params.slice(0, 3)).map(v => v * MM) : null;
  const pathAt = pathEnds(f, comp, file, pointAt);
  if (pathAt.error) { row.result = "failed"; row.why = pathAt.error; return null; }
  row.note = `its path ${pathAt.how}`;
  const { guessed, element, sign, note, ...a } = point;
  const op = { op: "mate", type: "path", name: f.name, a, b: pathAt.ends };
  const d = (f.dimensions || [])[0];
  if (d && typeof d.value === "number") {
    // A length along the path (valueType 0); otherwise a fraction of it
    if (d.valueType === 0) { op.mode = "dimension"; op.distance = tidy(d.value * MM); }
    else { op.mode = "percent"; op.percent = tidy(d.value <= 1 ? d.value * 100 : d.value); }
    const { mode, distance, percent, ...free } = op;
    row.alternatives = [
      { op: { ...op, flip_dimension: true }, note: `D1 (${op.distance !== undefined ? op.distance + " mm" : op.percent + "%"}) from the path's end` },
      { op: free, note: "free along the path: D1 doesn't hold where SolidWorks has the components (the export doesn't say whether it is a free path mate)" },
    ];
  }
  row.waits = [point.component, ge.component.split("/")].some(c => c && (byPath.get(c[0]) || {}).suppression === 0) ? "a component it joins is suppressed" : undefined;
  if (f.suppressed) op.suppressed = true;
  return op;
}

/** How far from SolidWorks' placements (`want`) the components a `components` reply lists are: the worst. */
function offFrom(want, components) {
  let worst = 0;
  for (const c of withMoving(components)) { const m = want.get(c.name); if (m) worst = Math.max(worst, apart(matrixFromPeet(c), m)); }
  return worst;
}

/**
 * Adds SolidWorks' mate `f` with `op`; records in `row` whether it was, and how far it left the components from
 * SolidWorks' placements. Returns whether it was added.
 */
function addMate(peet, f, op, row, want) {
  const waits = !!row.waits;
  const mark = peet.backup();
  const reply = peet.apply(op);
  const status = reply.mate && reply.mate.status;
  const fine = status === "ok" || (status === "suppressed" && (f.suppressed || waits));
  if (!reply.ok || (status && !fine)) {
    peet.restore(mark);
    row.result = "failed";
    row.why = reply.error || (reply.mate && (reply.mate.message || reply.mate.status));
    return false;
  }
  let now = peet.ask({ op: "components" });
  let moved = tidy(offFrom(want, now.components));
  // What the export doesn't settle (a path mate's constraint): the others, if one leaves things nearer where
  // SolidWorks has them
  if (row.alternatives && moved > 1e-3 && !f.suppressed) {
    let best = { moved, op, note: null };
    for (const alt of row.alternatives) {
      peet.restore(mark);
      const other = peet.apply(alt.op);
      const placed = other.ok && other.mate && other.mate.status === "ok" ? peet.ask({ op: "components" }) : null;
      const off = placed ? tidy(offFrom(want, placed.components)) : Infinity;
      if (off < best.moved) best = { moved: off, op: alt.op, note: alt.note };
      if (off <= 1e-3) break;
    }
    peet.restore(mark);
    peet.apply(best.op);
    now = peet.ask({ op: "components" });
    moved = tidy(offFrom(want, now.components));
    if (best.note) row.note = `${row.note ? row.note + "; " : ""}${best.note}`;
  }
  delete row.alternatives;
  row.result = "imported";
  delete row.why;
  if (moved) row.moved = moved;
  if (f.suppressed && !waits) {
    // What it would do if FabSys unsuppressed it: solved then, as SolidWorks does
    const [un, placed] = peet.askAll([{ op: "suppress", mate: f.name, on: false }, { op: "components" }]);
    const s = un && un.mate ? un.mate.status : (un && un.error) || "no reply";
    row.unsuppressed = { status: s, ...(un && un.mate && un.mate.message ? { message: un.mate.message } : {}), off_mm: placed ? tidy(offFrom(want, placed.components)) : null };
  }
  return true;
}

/**
 * Where the mates left each component, against SolidWorks' placements (`want`): fills each component's row
 * (`rows`, by name) with how far off it is, and returns the worst and the assembly's freedom.
 */
function checkPlacements(peet, want, rows) {
  const placed = peet.ask({ op: "components" });
  let worst = 0;
  for (const c of withMoving(placed.components)) {
    const m = want.get(c.name);
    if (!m) continue;
    const d = apart(matrixFromPeet(c), m);
    worst = Math.max(worst, d);
    const row = rows.find(r => r.component === c.name);
    if (row) { row.off_mm = tidy(d); row.status = c.status; row.freedom = c.freedom; }
    else if (c.inside) {
      // A component that moves in a flexible sub-assembly
      const sub = rows.find(r => r.component === c.inside);
      if (sub) (sub.moving = sub.moving || []).push({ component: c.name, off_mm: tidy(d), freedom: c.freedom });
    }
  }
  return { worst, freedom: placed.freedom };
}

const verdictOf = (worst, missing, partsOff) => worst > 1e-3 ? "components placed differently"
  : missing ? "placed as SolidWorks has them, with components or mates left out"
  : partsOff ? "placed as SolidWorks has them; a part differs"
  : "matches";

/**
 * Imports an assembly, every configuration of it. `listed` is the components to use when its file has no export:
 * those its parent lists under it, with their transforms in the parent's coordinates, and `frame` the
 * sub-assembly's transform there.
 *
 * The active configuration is imported first: its components (each in the configuration of its part SolidWorks
 * has it reference), its equations and its mates. A mate SolidWorks has suppressed there but live in another
 * configuration is left for that one, where the export describes its ends. Then each other configuration is added
 * as a copy of it, and made what SolidWorks has: each component's suppression, referenced configuration, whether it
 * is fixed, and where it is; every mate let go, then those live there put back one at a time (each moving nothing
 * if it agrees with SolidWorks), with their values there; and the mates left for it added. Each configuration's
 * placements are checked against SolidWorks' for it.
 */
function importAssembly(name, exp, outFile, listed = null, frame = null) {
  const report = { name, configuration: null, components: [], mates: [], left: [], standIns: [], notes: [], configurations: [] };
  const peet = new Peet(outFile);
  let cfg = null;
  let others = [];
  if (exp) {
    cfg = exp.configurations.find(c => c.name === exp.activeConfiguration) || exp.configurations[0];
    report.configuration = cfg.name;
    others = exp.configurations.filter(c => c !== cfg);
    // A new assembly's one configuration is "Default": it is SolidWorks' active one
    const comment = cfg.description && cfg.description !== cfg.name ? { comment: cfg.description } : {};
    if (cfg.name !== "Default" || comment.comment) peet.apply({ op: "edit_configuration", configuration: "Default", ...(cfg.name !== "Default" ? { name: cfg.name } : {}), ...comment });
    if (exp.configurations.some(c => c.derived)) report.notes.push("a derived configuration: imported as one of its own (PeetCAD's configurations are not derived from others)");
  }
  const all = listed || cfg.components || [];
  // In the sub-assembly's own coordinates
  const local = frame ? inverse(matrixOf(frame)) : null;
  const placeOf = c => local ? then(matrixOf(c.transform), local) : matrixOf(c.transform);
  const top = all.filter(c => !c.path.includes("/"));
  const byPath = new Map(all.map(c => [c.path, c]));
  // With the components of suppressed sub-assemblies, which mates can name
  const mateByPath = new Map(withHidden(all, listed ? null : cfg).map(c => [c.path, c]));
  const want = new Map();

  for (const c of top) {
    const row = { component: c.path, file: path.basename(c.file || ""), configuration: c.configuration, suppressed: c.suppression === 0, fixed: !!c.fixed };
    report.components.push(row);
    let source;
    let configuration = null;
    if (/\.sldasm$/i.test(c.file || "")) {
      const sub = importSub(c, all);
      if (sub.error) { row.result = "left out"; row.why = sub.error; report.left.push(`${c.path}: ${sub.error}`); continue; }
      source = sub.path;
      if (sub.standIn) report.standIns.push(`${c.path}: ${sub.standIn}`);
      // The sub-assembly's configuration it shows, as SolidWorks has it
      if (c.configuration && sub.configurations && sub.configurations.includes(c.configuration)) configuration = c.configuration;
    } else {
      const part = partOf(c);
      if (part.error) { row.result = "left out"; row.why = part.error; report.left.push(`${c.path}: ${part.error}`); continue; }
      source = part.path;
      configuration = part.configuration;
      const pr = partReport(c.file);
      if (pr && pr.verdict !== "matches") row.partVerdict = pr.verdict;
    }
    const { m, note } = placementOf(c, all, placeOf, local);
    if (note) row.placedBy = note;
    const rot = axisAngle(m);
    const op = { op: "insert", path: source, name: c.path, at: m.t.map(v => tidy(v * MM)), ...(rot ? { rotate: rot } : {}), fixed: !!c.fixed, ...(configuration ? { configuration } : {}) };
    const reply = peet.apply(op);
    if (!reply.ok) { row.result = "failed"; row.why = reply.error; report.left.push(`${c.path}: ${reply.error}`); continue; }
    row.result = "inserted";
    want.set(c.path, m);
    // A sub-assembly SolidWorks solves as flexible: its own mates are solved here, and what moves in it starts
    // where SolidWorks has it
    // (Whether a sub-assembly is flexible is the same in every configuration: one opened otherwise than its file
    // in any of them is)
    const flexible = /\.sldasm$/i.test(c.file || "") && (flexibleOf(c, all) || others.map(o => {
      const oc = (o.components || []).find(x => x.path === c.path);
      return oc && flexibleOf(oc, o.components || []);
    }).find(Boolean));
    if (flexible) {
      const made = peet.apply({ op: "flexible", component: c.path });
      if (made.ok) {
        row.flexible = flexible;
        for (const [inner, p] of innerPlacements(c, all, placeOf, local)) {
          // (Those fixed in their sub-assembly are refused: they move with it)
          if (peet.apply(placeOp(inner.split("/"), p)).ok) want.set(inner, p);
        }
      } else report.notes.push(`${c.path}: could not be made flexible: ${made.error}`);
    }
    if (c.suppression === 0) peet.apply({ op: "suppress", component: c.path });
  }

  // Equations, before the mates: a part an equation sizes (Plinth Folded's pipes) is mated at its size
  if (cfg && (cfg.equations || []).length) importEquations(peet, cfg, report, byPath, want, name);
  if (cfg && others.some(o => JSON.stringify((o.equations || []).filter(e => !e.disabled).map(e => e.expression)) !== JSON.stringify((cfg.equations || []).filter(e => !e.disabled).map(e => e.expression))))
    report.notes.push("the live equations differ between configurations: those of the active one are imported, for every configuration");

  // Mates. One SolidWorks has suppressed here and live in another configuration is added in that one, where the
  // export describes its ends (in this one it gives only numbers, or nothing).
  const liveIn = new Map();   // mate -> the first other configuration it is live in
  for (const o of others) for (const [n, f] of matesOf(o)) if (!f.suppressed && !liveIn.has(n)) liveIn.set(n, o.name);
  for (const [, f] of matesOf(cfg)) {
    const row = { mate: f.name, type: f.type, suppressed: !!f.suppressed };
    report.mates.push(row);
    if (f.suppressed && liveIn.has(f.name)) { row.result = "left for another configuration"; row.deferredTo = liveIn.get(f.name); continue; }
    const op = mateOp(f, mateByPath, name, want, row);
    if (op) addMate(peet, f, op, row, want);
  }
  if (others.length) {
    // The active configuration as imported, before the others are
    const { worst } = checkPlacements(peet, want, report.components);
    report.offBeforeOthers = tidy(worst);
  }

  // ---- The other configurations ----
  for (const o of others) importConfiguration(peet, report, { name, cfg, o, all: exp.configurations, top, placeOf, local });
  if (others.length) {
    const back = peet.apply({ op: "configuration", configuration: cfg.name });
    if (!back.ok) report.notes.push(`could not make ${cfg.name} active again: ${back.error}`);
  }

  // Where the mates left each component, against SolidWorks
  const { worst, freedom } = checkPlacements(peet, want, report.components);
  report.freedom = freedom;
  report.off_mm = tidy(worst);
  const live = report.mates.filter(m => !m.suppressed);
  const missing = report.components.filter(c => c.result !== "inserted").length + live.filter(m => m.result !== "imported").length;
  const partsOff = report.components.some(c => c.partVerdict);
  report.verdict = verdictOf(worst, missing, partsOff);
  report.mass = compareMass(peet, cfg);
  if (cfg) report.configurations.unshift({
    name: cfg.name, active: true, verdict: report.verdict, off_mm: report.off_mm, freedom,
    components: report.components.filter(c => c.result === "inserted" && !c.suppressed).length,
    matesLive: live.length, matesImported: live.filter(m => m.result === "imported").length,
    mass: report.mass,
  });
  fs.writeFileSync(outFile.replace(/\.peet$/, ".jsonl"), peet.script.map(o => typeof o === "string" ? o : JSON.stringify(o)).join("\n") + "\n");
  fs.writeFileSync(outFile.replace(/\.peet$/, ".report.json"), JSON.stringify(report, null, 1));
  try { fs.unlinkSync(outFile + ".before"); } catch {}
  return report;
}

/**
 * Adds configuration `o` of an assembly whose active configuration `cfg` was imported, and makes it what SolidWorks
 * has (see importAssembly). Adds a row to `report.configurations` with its verdict.
 */
function importConfiguration(peet, report, { name, cfg, o, all, top, placeOf, local }) {
  const row = { name: o.name, changed: [], mates: [], notes: [] };
  report.configurations.push(row);
  const comment = o.description && o.description !== o.name ? { comment: o.description } : {};
  const made = peet.apply({ op: "add_configuration", name: o.name, copy: cfg.name, ...comment });
  if (!made.ok) { row.verdict = "not imported"; row.why = made.error; return; }
  peet.note(`Configuration ${o.name}`);
  const oall = o.components || [];
  const obyPath = new Map(oall.map(c => [c.path, c]));
  const owant = new Map();
  const omates = matesOf(o);
  const activeMates = matesOf(cfg);

  // Every mate let go here, so the components can be put where SolidWorks has them in this configuration
  const imported = report.mates.filter(r => r.result === "imported");
  peet.applyAll(imported.map(r => ({ op: "suppress", mate: r.mate })));

  // The components: suppressed or not, the configuration of their part they show, fixed or not, and where
  for (const c of top) {
    const crow = report.components.find(r => r.component === c.path);
    const oc = obyPath.get(c.path);
    if (!crow || crow.result !== "inserted") continue;
    if (!oc) { row.notes.push(`${c.path} is not in this configuration's export`); continue; }
    const ops = [];
    const differs = [];
    if ((oc.suppression === 0) !== (c.suppression === 0)) { ops.push({ op: "suppress", component: c.path, on: oc.suppression === 0 }); differs.push(oc.suppression === 0 ? "suppressed" : "unsuppressed"); }
    if (oc.configuration !== c.configuration) {
      const sub = /\.sldasm$/i.test(oc.file || "");
      const ok = sub ? subHas(oc.file, oc.configuration) : !partOf(oc).error;
      if (ok && oc.configuration) { ops.push({ op: "component_configuration", component: c.path, configuration: oc.configuration }); differs.push(`in ${oc.configuration}`); }
      else row.notes.push(`${c.path}: its ${sub ? "sub-assembly" : "part"} has no configuration "${oc.configuration}" in PeetCAD`);
    }
    if (!!oc.fixed !== !!c.fixed) { ops.push({ op: "fix", component: c.path, on: !!oc.fixed }); differs.push(oc.fixed ? "fixed" : "floating"); }
    const { m } = placementOf(oc, oall, placeOf, local);
    ops.push(placeOp(c.path, m));
    owant.set(c.path, m);
    if (crow.flexible) for (const [inner, p] of innerPlacements(oc, oall, placeOf, local)) ops.push({ ...placeOp(inner.split("/"), p), inner, p });
    const replies = peet.applyAll(ops.map(({ inner, p, ...op }) => op));
    if (process.env.DEBUG_ASM) { const h = (peet.ask({ op: "components" }).components || []).find(x => x.name === process.env.DEBUG_ASM); console.error(c.path, JSON.stringify(replies.map(r => r.ok)), h && JSON.stringify([h.at, h.rotate])); }
    ops.forEach((op, i) => {
      const r = replies[i] || {};
      // (Those fixed in their sub-assembly are refused: they move with it)
      if (op.inner) { if (r.ok) owant.set(op.inner, op.p); return; }
      if (!r.ok) row.notes.push(`${c.path}: ${op.op} failed: ${r.error || "no reply"}`);
    });
    if (differs.length) row.changed.push({ component: c.path, differs });
  }

  // Where the components are before any mate is put back: SolidWorks' placements, if the places took
  {
    const placed = peet.ask({ op: "components" });
    row.offPlaced = [];
    for (const c of withMoving(placed.components)) {
      const m = owant.get(c.name);
      const d = m ? apart(matrixFromPeet(c), m) : 0;
      if (d > 1e-3) row.offPlaced.push({ component: c.name, off_mm: tidy(d) });
    }
  }

  // The mates, in the tree's order: those live here put back one at a time, with their values here; those left
  // for this configuration added
  for (const [mname, f] of omates) {
    const mrow = { mate: mname, type: f.type, suppressed: !!f.suppressed };
    row.mates.push(mrow);
    const first = report.mates.find(r => r.mate === mname);
    if (!first) { mrow.result = "not in the active configuration"; continue; }
    if (first.result === "left for another configuration" && first.deferredTo === o.name) {
      // Added here, where SolidWorks describes its ends; suppressed where SolidWorks has it suppressed
      const op = mateOp(f, new Map(withHidden(oall, o).map(c => [c.path, c])), name, owant, mrow);
      if (op && addMate(peet, f, op, mrow, owant)) {
        const suppressedIn = all.filter(x => x !== o && (matesOf(x).get(mname) || {}).suppressed).map(x => x.name);
        if (suppressedIn.length) peet.apply({ op: "suppress", mate: mname, configurations: suppressedIn });
        Object.assign(first, { result: "imported", configuration: o.name, ...(mrow.guessed ? { guessed: true } : {}) });
        delete first.why;
      } else {
        first.result = mrow.result;
        first.why = `in ${o.name}: ${mrow.why}`;
      }
      continue;
    }
    if (first.result !== "imported") { mrow.result = first.result; mrow.why = first.why; continue; }
    // A distance that differs here
    const active = activeMates.get(mname);
    const value = f.type === "MateDistanceDim" && (f.dimensions || [])[0] ? f.dimensions[0].value : null;
    const was = active && (active.dimensions || [])[0] ? active.dimensions[0].value : null;
    if (value !== null && was !== null && Math.abs(value - was) > 1e-9) {
      const set = peet.apply({ op: "set", name: `D1@${mname}`, value: tidy(value * MM), configurations: "this" });
      if (!set.ok) row.notes.push(`${mname}: its distance here could not be set: ${set.error}`);
    }
    if (f.suppressed) { mrow.result = "imported"; continue; }
    const mark = peet.backup();
    const reply = peet.apply({ op: "suppress", mate: mname, on: false });
    const status = reply.mate && reply.mate.status;
    const waits = (f.mate.entities || []).some(e => e.component && (obyPath.get(e.component.split("/")[0]) || {}).suppression === 0);
    if (!reply.ok || (status !== "ok" && !(status === "suppressed" && waits))) {
      peet.restore(mark);
      mrow.result = "failed";
      mrow.why = reply.error || (reply.mate && (reply.mate.message || reply.mate.status));
      continue;
    }
    mrow.result = "imported";
    const moved = tidy(offFrom(owant, peet.ask({ op: "components" }).components));
    if (moved) mrow.moved = moved;
  }

  // Where the mates left each component, against SolidWorks' placements in this configuration
  const placed = peet.ask({ op: "components" });
  let worst = 0;
  row.off = [];
  for (const c of withMoving(placed.components)) {
    const m = owant.get(c.name);
    if (!m) continue;
    const d = apart(matrixFromPeet(c), m);
    worst = Math.max(worst, d);
    if (d > 1e-3) row.off.push({ component: c.name, off_mm: tidy(d), freedom: c.freedom });
  }
  const freedom = placed.freedom;
  row.off_mm = tidy(worst);
  row.freedom = freedom;
  const live = row.mates.filter(m => !m.suppressed);
  const missing = report.components.filter(c => c.result !== "inserted").length + live.filter(m => m.result !== "imported").length;
  row.verdict = verdictOf(worst, missing, report.components.some(c => c.partVerdict));
  row.componentsLive = top.filter(c => (obyPath.get(c.path) || {}).suppression !== 0 && (report.components.find(r => r.component === c.path) || {}).result === "inserted").length;
  row.matesLive = live.length;
  row.matesImported = live.filter(m => m.result === "imported").length;
  row.mass = compareMass(peet, o);
}

/**
 * The assembly's volume and box as PeetCAD measures them (`mass`), against SolidWorks' mass properties of the
 * configuration (format 6: `massProperties`), so that the parts' geometry where they are is checked and not only
 * their placements. SolidWorks' box is the bodies' extreme points where the export has them (`tightBox`), else its
 * own (which can be loose); PeetCAD's is its components' boxes placed, which is loose round a turned part: so a box
 * is only said to differ where PeetCAD's doesn't hold SolidWorks'. Null without SolidWorks' numbers.
 */
function compareMass(peet, swCfg) {
  const sw = swCfg && swCfg.massProperties;
  if (!sw || typeof sw.volume !== "number") return null;
  const reply = peet.ask({ op: "mass" });
  const total = reply && reply.total;
  if (!total) return { error: (reply && reply.error) || "no mass" };
  const volume = sw.volume * 1e9;
  const out = { volume_mm3: tidy(total.volume_mm3), sw_volume_mm3: tidy(volume), volume_ratio: tidy(total.volume_mm3 / volume) };
  const swBox = sw.tightBox || sw.box;
  if (Array.isArray(swBox) && swBox.length >= 6 && total.min && total.max) {
    const lo = swBox.slice(0, 3).map(v => v * MM), hi = swBox.slice(3, 6).map(v => v * MM);
    // How far SolidWorks' box reaches outside PeetCAD's (0 if PeetCAD's holds it), and how much bigger PeetCAD's is
    const outside = Math.max(0, ...[0, 1, 2].flatMap(k => [total.min[k] - lo[k], hi[k] - total.max[k]]));
    const looser = Math.max(0, ...[0, 1, 2].flatMap(k => [lo[k] - total.min[k], total.max[k] - hi[k]]));
    Object.assign(out, { min: total.min, max: total.max, sw_min: lo.map(tidy), sw_max: hi.map(tidy), sw_box: sw.tightBox ? "extreme points" : "SolidWorks' box", box_outside_mm: tidy(outside), box_looser_mm: tidy(looser) });
  }
  out.matches = Math.abs(out.volume_ratio - 1) < 1e-4 && !(out.box_outside_mm > 1e-2);
  return out;
}

/** The components a `components` reply lists, and those that move in its flexible sub-assemblies, by path. */
function withMoving(components) {
  const out = [];
  for (const c of components || []) {
    out.push(c);
    for (const x of c.moving || []) out.push({ ...x, name: [c.name, ...x.path].join("/"), inside: c.name });
  }
  return out;
}

/**
 * Whether SolidWorks solves a sub-assembly component as flexible: its `solving` where the export has it, else
 * whether its components are placed relative to each other otherwise than in the sub-assembly's own file (a hinge
 * opened to another angle). Returns how it was told ("solving" or "inferred"), or null for rigid.
 */
function flexibleOf(c, all) {
  if (c.solving === "flexible") return "solving";
  if (c.solving === "rigid") return null;
  // (Set against the sub-assembly's own export in the configuration the component shows)
  const own = ownComponents(c);
  if (!own) return null;
  // Placed relative to the component fixed in it, as the parent lists them
  const anchor = anchorOf(c, all);
  const back = inverse(anchor || matrixOf(c.transform));
  // Its parts: SolidWorks' transforms for the sub-assemblies in it need not be where their parts are
  for (const x of all.filter(x => x.path.startsWith(c.path + "/") && !/\.sldasm$/i.test(x.file || ""))) {
    const mine = own.get(x.path.slice(c.path.length + 1));
    if (!mine || !mine.transform || !x.transform) continue;
    if (apart(then(matrixOf(x.transform), back), matrixOf(mine.transform)) > 1e-3) return "inferred";
  }
  return null;
}

/**
 * A configuration's components with, under each suppressed sub-assembly, its components: SolidWorks lists none
 * under one, but mates can name its parts (they wait for it). Format 6 exports them (`suppressedChildren`); else
 * they are taken from the sub-assembly's own export, in the configuration it shows, placed by its transform. For
 * finding mates' ends only: what is inserted is as before.
 */
function withHidden(list, cfg) {
  const out = [...list];
  const listed = new Set(list.map(c => c.path));
  const exported = (cfg && cfg.suppressedChildren) || [];
  for (const c of list) {
    if (c.suppression !== 0 || !/\.sldasm$/i.test(c.file || "") || list.some(x => x.path.startsWith(c.path + "/"))) continue;
    const mine = exported.filter(x => x.path.startsWith(c.path + "/"));
    if (mine.length) { for (const x of mine) if (!listed.has(x.path)) out.push({ ...x, hidden: true }); continue; }
    const own = ownComponents(c);
    if (!own || !c.transform) continue;
    const placed = matrixOf(c.transform);
    for (const [p, x] of own) {
      if (!x.transform || listed.has(c.path + "/" + p)) continue;
      const m = then(matrixOf(x.transform), placed);
      out.push({ ...x, path: c.path + "/" + p, transform: [...m.R.flat(), ...m.t, 1, 0, 0, 0], hidden: true });
    }
  }
  return out;
}

/** A sub-assembly's components as its own export has them (in the configuration its parent uses), by path. */
function ownComponents(c) {
  const exp = exportOf(c.file);
  if (!exp) return null;
  let j;
  try { j = JSON.parse(fs.readFileSync(exp)); } catch { return null; }
  const cfg = (j.configurations || []).find(x => x.name === c.configuration) || (j.configurations || []).find(x => x.name === j.activeConfiguration) || (j.configurations || [])[0];
  return cfg ? new Map((cfg.components || []).map(x => [x.path, x])) : null;
}

/**
 * Where a sub-assembly is, from the first component fixed in it: where its parent lists that component, less where
 * the sub-assembly's own file has it. A sub-assembly with nothing fixed in it (in the configuration shown) is placed
 * by its components where they all agree on one place for it (they are where its file has them, relative to each
 * other). Null if neither (or there is no export to tell).
 */
function anchorOf(c, all) {
  const own = ownComponents(c);
  if (!own) return null;
  const places = [];
  for (const x of all.filter(x => x.path.startsWith(c.path + "/") && !x.path.slice(c.path.length + 1).includes("/"))) {
    const mine = own.get(x.path.slice(c.path.length + 1));
    if (!mine || !mine.transform || !x.transform || mine.suppression === 0) continue;
    const at = { ...then(inverse(matrixOf(mine.transform)), matrixOf(x.transform)), by: mine.path };
    if (mine.fixed) return at;
    // (SolidWorks' transform for a sub-assembly in it need not be where its parts are: its parts only)
    if (!/\.sldasm$/i.test(x.file || "")) places.push(at);
  }
  if (places.length && places.every(p => apart(p, places[0]) < 1e-3)) return { ...places[0], by: `${places[0].by} (nothing is fixed in it, and its components agree)` };
  return null;
}

/**
 * A sub-assembly, imported once per file as an assembly of its own, with every configuration it has: a component
 * of it shows the one SolidWorks has it reference. A sub-assembly that was not exported is built from what its
 * parent lists under it.
 */
const subs = new Map();
function importSub(c, all) {
  const exp = exportOf(c.file);
  const key = c.file.toLowerCase();
  if (subs.has(key)) return subs.get(key);
  const name = path.basename(c.file).replace(/\.sldasm$/i, "");
  const file = path.join(outDir, "sub-assemblies", `${name}.peet`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  let made;
  try {
    if (exp) {
      const j = JSON.parse(fs.readFileSync(exp));
      if (!j.configurations || !j.configurations.length) throw new Error("its export is empty: " + (j.error || ""));
      const r = importAssembly(name, j, file);
      made = { path: file, report: r, configurations: j.configurations.map(x => x.name) };
    } else {
      // Not exported (outside Templates): what its parent lists under it, placed relative to it
      const prefix = c.path + "/";
      const listed = all.filter(x => x.path.startsWith(prefix)).map(x => ({ ...x, path: x.path.slice(prefix.length) }));
      if (!listed.length) throw new Error("not exported, and its parent lists nothing under it");
      const r = importAssembly(name, null, file, listed, c.transform);
      made = { path: file, report: r, standIn: "not exported: built from its components as its parent has them, without its mates" };
    }
  } catch (e) { made = { error: "the sub-assembly could not be imported: " + e.message }; }
  subs.set(key, made);
  return made;
}

fs.mkdirSync(outDir, { recursive: true });
const inputs = fs.statSync(input).isDirectory()
  ? fs.readdirSync(input).filter(f => /\.sldasm\.swx\.json$/i.test(f)).sort().map(f => path.join(input, f))
  : [input];
const summary = [];
for (const file of inputs.filter(f => !only || path.basename(f).toLowerCase().includes(only.toLowerCase()))) {
  const j = JSON.parse(fs.readFileSync(file));
  const name = j.name || path.basename(file).replace(/\.sldasm\.swx\.json$/i, "");
  if (!j.configurations || !j.configurations.length) { summary.push({ name, verdict: "not exported", error: j.error }); console.log(`${"not exported".padEnd(60)} ${name}`); continue; }
  const started = Date.now();
  let r;
  try { r = importAssembly(name, j, path.join(outDir, name + ".peet")); }
  catch (e) { r = { name, verdict: "the importer failed", error: e.stack }; }
  const mates = r.mates || [];
  const row = {
    name, verdict: r.verdict, seconds: Math.round((Date.now() - started) / 1000), off_mm: r.off_mm,
    components: (r.components || []).length, componentsLeft: (r.components || []).filter(c => c.result !== "inserted").length,
    mates: mates.length, matesImported: mates.filter(m => m.result === "imported").length,
    matesNotSupported: mates.filter(m => m.result === "not supported").length, standIns: (r.standIns || []).length,
    configurations: (r.configurations || []).map(c => ({ name: c.name, verdict: c.verdict, off_mm: c.off_mm, matesLive: c.matesLive, matesImported: c.matesImported })),
    equations: (r.equations || []).filter(e => e.result !== "disabled").length, equationsImported: (r.equations || []).filter(e => e.result === "imported").length,
  };
  summary.push(row);
  console.log(`${r.verdict.padEnd(60)} ${name}  (components ${row.components - row.componentsLeft}/${row.components}, mates ${row.matesImported}/${row.mates}, off ${r.off_mm} mm)`);
  for (const c of (r.configurations || []).filter(c => !c.active)) console.log(`${("  " + c.verdict).padEnd(60)}   configuration ${c.name}  (live mates ${c.matesImported}/${c.matesLive}, off ${c.off_mm} mm)`);
}
fs.writeFileSync(path.join(outDir, "summary.json"), JSON.stringify(summary, null, 1));
