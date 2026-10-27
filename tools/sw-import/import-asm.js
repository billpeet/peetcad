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

// A part in another configuration than the one its file is in: PeetCAD's components can't choose a part's
// configuration (parity A5), so a copy of the part with that configuration made active stands in for it.
const variants = new Map();
function partIn(file, configuration, report) {
  const base = partFileOf(file);
  if (!base) return { error: "the part was not imported" };
  const pr = partReport(file);
  const active = pr && pr.configuration;
  if (!configuration || configuration === active) return { path: base };
  const key = base + "|" + configuration;
  if (variants.has(key)) return variants.get(key);
  const dir = path.join(outDir, "parts");
  fs.mkdirSync(dir, { recursive: true });
  const copy = path.join(dir, `${path.basename(base, ".peet")} @ ${configuration.replace(/[<>:"/\\|?*]/g, "_")}.peet`);
  fs.copyFileSync(base, copy);
  const r = spawnSync(peetExe, ["run", "-", "-f", copy, "-q"], { input: JSON.stringify({ op: "configuration", configuration }) + "\n", encoding: "utf8" });
  const line = (r.stdout || "").split("\n").find(l => l.startsWith("{"));
  const reply = line ? JSON.parse(line) : { ok: false, error: r.stderr };
  const made = reply.ok ? { path: copy, standIn: `the part in its configuration "${configuration}" is a copy with that configuration active` }
    : { error: `the part has no configuration "${configuration}" (${reply.error || "refused"})` };
  variants.set(key, made);
  return made;
}

// ---- Mates ----

const MATE_KINDS = { MateCoincident: "coincident", MateConcentric: "concentric", MateParallel: "parallel", MateDistanceDim: "distance", MateWidth: "width" };
// SolidWorks' Front Plane is XY, PeetCAD's top; its Top Plane is XZ, PeetCAD's front (whose normal is -Y where
// SolidWorks' is +Y); Right is YZ in both.
const STANDARD_PLANES = { "Front Plane": "top", "Top Plane": "front", "Right Plane": "right" };
const STANDARD_NORMALS = { top: [0, 0, 1], front: [0, -1, 0], right: [1, 0, 0] };

// The faces and edges of each part file, as PeetCAD has them, to find a mate's end by its geometry
const geometryCache = new Map();
function geometryOf(file) {
  if (geometryCache.has(file)) return geometryCache.get(file);
  const ask = op => {
    const r = spawnSync(peetExe, ["op", JSON.stringify(op), "-f", file, "-q", "--no-save"], { encoding: "utf8", maxBuffer: 1 << 28 });
    const line = (r.stdout || "").split("\n").find(l => l.startsWith("{"));
    return line ? JSON.parse(line) : {};
  };
  const g = { faces: ask({ op: "faces" }).faces || [], edges: ask({ op: "edges" }).edges || [] };
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
  if (!x && e.params && comp) {
    // swMateEntity2ReferenceType_e: 0 point, 1 line, 2 plane, 3 cylinder, 4 cone, 5 sphere, 7 circle
    const back = inverse(matrixOf(comp.transform));
    const p = mm(toWorld(back, e.params.slice(0, 3)));
    const d = norm3(toWorld({ R: back.R, t: [0, 0, 0] }, e.params.slice(3, 6)));
    const r = e.params[6] * MM;
    // A plane's direction is its face's outward normal (the mates' alignments agree with it), and the point
    // where SolidWorks has the face: one of several faces in that plane facing that way, the nearest
    if (e.referenceType === 2) return { kind: "plane", n: d, oriented: true, p, near: p };
    if (e.referenceType === 0) return { kind: "point", p };
    if (e.referenceType === 7) return { kind: "circle", c: p, n: d, r };
    if ([1, 3, 4].includes(e.referenceType)) return { kind: "axis", o: p, d, r: r > 0 ? r : null };
  }
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
    // A straight edge's direction in PeetCAD's mates is its curve's, which `edges` doesn't give: none
    const edge = pick(edges.filter(g => g.curve === "line"), g => {
      const d = Math.min(len3(sub3(g.from, shape.a)) + len3(sub3(g.to, shape.b)), len3(sub3(g.from, shape.b)) + len3(sub3(g.to, shape.a)));
      return d < 2 * TOL ? d : null;
    });
    return edge && { edge: sel(edge) };
  }
  if (shape.kind === "point") {
    const vertex = edges.flatMap(g => [g.from, g.to]).find(q => q && len3(sub3(q, shape.p)) < TOL);
    return vertex ? { vertex: { at: vertex.map(tidy) } } : null;
  }
  return null;
}

/** A reference plane's normal in its part, as PeetCAD has it. */
const planeNormals = new Map();
function planeNormal(file, plane) {
  if (STANDARD_NORMALS[plane]) return STANDARD_NORMALS[plane];
  const key = file + "|" + plane;
  if (!planeNormals.has(key)) {
    const r = spawnSync(peetExe, ["op", JSON.stringify({ op: "feature", feature: plane }), "-f", file, "-q", "--no-save"], { encoding: "utf8" });
    const line = (r.stdout || "").split("\n").find(l => l.startsWith("{"));
    const reply = line ? JSON.parse(line) : {};
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
 * A mate end in PeetCAD's terms: the component path, and the face, edge, vertex or plane of its part. With it,
 * what it is to a mate (`element`: plane or axis) and `sign`: whether PeetCAD's direction for it is SolidWorks'
 * (1) or the other way (-1), so that SolidWorks' alignment and flipped dimension can be said in PeetCAD's terms.
 */
function endOf(e, byPath, assembly, partFile) {
  if (!e.component || e.component === assembly) return { error: "an end on the assembly's own planes or origin, which PeetCAD's mates can't take (parity A12)" };
  const comp = byPath.get(e.component);
  if (!comp) return { error: `its component ${e.component} is not in the assembly` };
  const component = e.component.split("/");
  const signOf = dir => {
    const sw = swDirection(e, comp);
    if (!dir || !sw) return null;
    const d = dot3(norm3(dir), sw);
    return Math.abs(Math.abs(d) - 1) < 1e-4 ? Math.sign(d) : null;
  };
  if (e.entity && e.entity.kind === "feature" && e.entity.type === "RefPlane") {
    const plane = STANDARD_PLANES[e.entity.name] || e.entity.name;
    const file = partFile(comp);
    return { component, plane, element: "plane", sign: file ? signOf(planeNormal(file, plane)) : null };
  }
  if (e.entity && /^sketch/.test(e.entity.kind)) return { error: "an end on a sketch's point or line, which PeetCAD's mates can't take" };
  const shape = shapeOf(e, comp);
  if (!shape) return { error: `an end PeetCAD has no match for (${e.entity ? e.entity.kind : "reference type " + e.referenceType})` };
  const file = partFile(comp);
  if (!file) return { error: "its part was not imported" };
  const found = findIn(file, shape);
  if (!found) return { error: `no ${shape.kind === "plane" ? "flat face" : shape.kind === "axis" ? "round face or edge on that axis" : shape.kind} of ${path.basename(file, ".peet")} is where SolidWorks' is`, unmatched: true };
  const { dir, ...where } = found;
  const element = shape.kind === "plane" ? "plane" : shape.kind === "axis" || shape.kind === "circle" ? "axis" : shape.kind;
  return { component, ...where, element, sign: signOf(dir), ...(e.entity ? {} : { guessed: true }) };
}

// ---- Import ----

const reports = [];
const imported = new Map();   // export file -> { peet, report }

function walkFeatures(list, f) { for (const x of list || []) { f(x); walkFeatures(x.sub, f); } }

/**
 * Imports an assembly. `listed` is the components to use when its file has no export: those its parent lists
 * under it, with their transforms in the parent's coordinates, and `frame` the sub-assembly's transform there.
 */
function importAssembly(name, exp, outFile, listed = null, frame = null) {
  const report = { name, configuration: null, components: [], mates: [], left: [], standIns: [], notes: [] };
  const peet = new Peet(outFile);
  let cfg = null;
  if (exp) {
    cfg = exp.configurations.find(c => c.name === exp.activeConfiguration) || exp.configurations[0];
    report.configuration = cfg.name;
    if (exp.configurations.length > 1) report.notes.push(`${exp.configurations.length} configurations: only "${cfg.name}" is imported (PeetCAD's assemblies have none, parity A5)`);
    if ((cfg.equations || []).length) report.notes.push(`${cfg.equations.length} equations not imported (parity A6, A7)`);
  }
  const all = listed || cfg.components || [];
  // In the sub-assembly's own coordinates
  const local = frame ? inverse(matrixOf(frame)) : null;
  const placeOf = c => local ? then(matrixOf(c.transform), local) : matrixOf(c.transform);
  const top = all.filter(c => !c.path.includes("/"));
  const byPath = new Map(all.map(c => [c.path, c]));
  const want = new Map();

  for (const c of top) {
    const row = { component: c.path, file: path.basename(c.file || ""), configuration: c.configuration, suppressed: c.suppression === 0, fixed: !!c.fixed };
    report.components.push(row);
    let source;
    if (/\.sldasm$/i.test(c.file || "")) {
      const sub = importSub(c, all);
      if (sub.error) { row.result = "left out"; row.why = sub.error; report.left.push(`${c.path}: ${sub.error}`); continue; }
      source = sub.path;
      if (sub.standIn) report.standIns.push(`${c.path}: ${sub.standIn}`);
    } else {
      const part = partIn(c.file, c.configuration, report);
      if (part.error) { row.result = "left out"; row.why = part.error; report.left.push(`${c.path}: ${part.error}`); continue; }
      source = part.path;
      if (part.standIn) report.standIns.push(`${c.path}: ${part.standIn}`);
      const pr = partReport(c.file);
      if (pr && pr.verdict !== "matches") row.partVerdict = pr.verdict;
    }
    // A sub-assembly goes where the components fixed in it are: SolidWorks' own transform for a sub-assembly it
    // solves as flexible need not be (the template hinges' are hundreds of millimetres from their fixed leaf)
    const anchor = /\.sldasm$/i.test(c.file || "") ? anchorOf(c, all) : null;
    let m = placeOf(c);
    if (anchor) {
      const at = local ? then(anchor, local) : anchor;
      const d = apart(at, m);
      if (d > 1e-3) { row.placedBy = `its fixed component ${anchor.by}, ${tidy(d)} mm from SolidWorks' transform for the sub-assembly`; m = at; }
    }
    const rot = axisAngle(m);
    const op = { op: "insert", path: source, name: c.path, at: m.t.map(v => tidy(v * MM)), ...(rot ? { rotate: rot } : {}), fixed: !!c.fixed };
    const reply = peet.apply(op);
    if (!reply.ok) { row.result = "failed"; row.why = reply.error; report.left.push(`${c.path}: ${reply.error}`); continue; }
    row.result = "inserted";
    want.set(c.path, m);
    // A sub-assembly SolidWorks solves as flexible: its own mates are solved here, and what moves in it starts
    // where SolidWorks has it
    const flexible = /\.sldasm$/i.test(c.file || "") && flexibleOf(c, all);
    if (flexible) {
      const made = peet.apply({ op: "flexible", component: c.path });
      if (made.ok) {
        row.flexible = flexible;
        for (const x of all.filter(x => x.path.startsWith(c.path + "/"))) {
          // A sub-assembly in it, where its own fixed component is, as above
          const anchorX = /\.sldasm$/i.test(x.file || "") ? anchorOf(x, all) : null;
          const p = anchorX ? (local ? then(anchorX, local) : anchorX) : placeOf(x), r = axisAngle(p);
          const placed = peet.apply({ op: "place", component: x.path.split("/"), at: p.t.map(v => tidy(v * MM)), ...(r ? { rotate: r } : {}) });
          // (Those fixed in their sub-assembly are refused: they move with it)
          if (placed.ok) want.set(x.path, p);
        }
      } else report.notes.push(`${c.path}: could not be made flexible: ${made.error}`);
    }
    if (c.suppression === 0) peet.apply({ op: "suppress", component: c.path });
  }

  // Mates
  if (cfg) walkFeatures(cfg.features, f => {
    if (!/^Mate/.test(f.type) || f.type === "MateGroup" || !f.mate) return;
    const row = { mate: f.name, type: f.type, suppressed: !!f.suppressed };
    report.mates.push(row);
    const kind = MATE_KINDS[f.type];
    if (!kind) { row.result = "not supported"; row.why = `PeetCAD has no ${f.type.replace(/^Mate/, "")} mate (parity A4)`; return; }
    const partFile = comp => /\.sldprt$/i.test(comp.file || "") ? partIn(comp.file, comp.configuration).path || null : null;
    const ends = (f.mate.entities || []).map(e => endOf(e, byPath, name, partFile));
    const bad = ends.find(e => e.error);
    if (bad) { row.result = bad.unmatched ? "failed" : "not supported"; row.why = bad.error; return; }
    if (ends.some(e => !want.has(e.component[0]))) { row.result = "left out"; row.why = "a component it joins was left out"; return; }
    if (ends.some(e => e.guessed)) row.guessed = true;
    const strip = e => { const { guessed, element, sign, ...rest } = e; return rest; };
    // A component SolidWorks has suppressed: its mates wait for it, as SolidWorks' do
    const waits = ends.some(e => (byPath.get(e.component[0]) || {}).suppression === 0);
    if (waits) row.waits = "a component it joins is suppressed";
    let op;
    if (kind === "width") {
      // SolidWorks lists the width's pair, then the tab: two faces, or one round face or axis
      if (ends.length !== 4 && ends.length !== 3) { row.result = "not supported"; row.why = `a width mate of ${ends.length} ends`; return; }
      const width = ends.slice(0, 2), tab = ends.slice(2);
      const one = list => new Set(list.map(e => e.component[0])).size === 1;
      if (!one(width) || !one(tab) || width[0].component[0] === tab[0].component[0]) {
        row.result = "not supported"; row.why = "a width mate whose width pair or tab is on more than one component"; return;
      }
      if (tab.length === 1 && tab[0].element !== "axis") { row.result = "not supported"; row.why = "a width mate whose one tab end is not round"; return; }
      op = { op: "mate", type: "width", name: f.name, a: width.map(strip), b: tab.length === 1 ? strip(tab[0]) : tab.map(strip) };
      // swWidthMateConstraintType_e: 0 centred, 1 free, 2 dimension, 3 percent
      const v = (f.definition && f.definition.values) || {};
      if (v.ConstraintType === 1) op.mode = "free";
      else if (v.ConstraintType === 2) { op.mode = "dimension"; op.distance = tidy(v.DistanceFromEnd * MM); }
      else if (v.ConstraintType === 3) { op.mode = "percent"; op.percent = tidy(v.PercentDistanceFromEnd * 100); }
      if (v.FlipDimension && op.mode && op.mode !== "free") op.a.reverse();
    } else {
      if (ends.length !== 2) { row.result = "not supported"; row.why = `${ends.length} ends`; return; }
      const [ea, eb] = ends;
      op = { op: "mate", type: kind, name: f.name, a: strip(ea), b: strip(eb) };
      // swMateAlign_e: 0 aligned, 1 anti-aligned, 2 closest, of the ends' directions as SolidWorks has them;
      // PeetCAD's may point the other way (a reference plane's, an axis's), which `sign` says.
      const same = ea.element === eb.element && (ea.element === "plane" || ea.element === "axis");
      const known = ea.sign && eb.sign;
      if (same && f.mate.alignment === 2) {
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
        if (!d) { row.result = "failed"; row.why = "its distance was not exported"; return; }
        op.distance = tidy(d.value * MM);
        // SolidWorks' distance from a plane is along its outward normal unless the dimension is flipped;
        // PeetCAD measures from the first end if it is a plane, else from the second
        const from = ea.element === "plane" ? ea : eb.element === "plane" ? eb : null;
        if (from && (f.mate.flipped ? 1 : -1) * (from.sign || 1) > 0) op.flip_dimension = true;
        if (from && !from.sign) row.note = "which side the distance is on is SolidWorks', not checked against PeetCAD's directions";
      }
    }
    if (f.suppressed) op.suppressed = true;
    const mark = peet.backup();
    const reply = peet.apply(op);
    const status = reply.mate && reply.mate.status;
    const fine = status === "ok" || (status === "suppressed" && (f.suppressed || waits));
    if (!reply.ok || (status && !fine)) {
      peet.restore(mark);
      row.result = "failed";
      row.why = reply.error || (reply.mate && (reply.mate.message || reply.mate.status));
      return;
    }
    // How far from SolidWorks' placement the components are with this mate
    const off = components => {
      let worst = 0;
      for (const c of withMoving(components)) { const m = want.get(c.name); if (m) worst = Math.max(worst, apart(matrixFromPeet(c), m)); }
      return worst;
    };
    const now = peet.ask({ op: "components" });
    const moved = tidy(off(now.components));
    row.result = "imported";
    if (moved) row.moved = moved;
    if (f.suppressed && !waits) {
      // What it would do if FabSys unsuppressed it: solved then, as SolidWorks does
      const [un, placed] = peet.askAll([{ op: "suppress", mate: f.name, on: false }, { op: "components" }]);
      const s = un && un.mate ? un.mate.status : (un && un.error) || "no reply";
      row.unsuppressed = { status: s, ...(un && un.mate && un.mate.message ? { message: un.mate.message } : {}), off_mm: placed ? tidy(off(placed.components)) : null };
    }
  });

  // Where the mates left each component, against SolidWorks
  const placed = peet.ask({ op: "components" });
  let worst = 0;
  for (const c of withMoving(placed.components)) {
    const m = want.get(c.name);
    if (!m) continue;
    const d = apart(matrixFromPeet(c), m);
    worst = Math.max(worst, d);
    const row = report.components.find(r => r.component === c.name);
    if (row) { row.off_mm = tidy(d); row.status = c.status; row.freedom = c.freedom; }
    else if (c.inside) {
      // A component that moves in a flexible sub-assembly
      const sub = report.components.find(r => r.component === c.inside);
      if (sub) (sub.moving = sub.moving || []).push({ component: c.name, off_mm: tidy(d), freedom: c.freedom });
    }
  }
  report.freedom = placed.freedom;
  report.off_mm = tidy(worst);
  const live = report.mates.filter(m => !m.suppressed);
  const missing = report.components.filter(c => c.result !== "inserted").length + live.filter(m => m.result !== "imported").length;
  const partsOff = report.components.some(c => c.partVerdict);
  report.verdict = worst > 1e-3 ? "components placed differently"
    : missing ? "placed as SolidWorks has them, with components or mates left out"
    : partsOff ? "placed as SolidWorks has them; a part differs"
    : "matches";
  fs.writeFileSync(outFile.replace(/\.peet$/, ".jsonl"), peet.script.map(o => typeof o === "string" ? o : JSON.stringify(o)).join("\n") + "\n");
  fs.writeFileSync(outFile.replace(/\.peet$/, ".report.json"), JSON.stringify(report, null, 1));
  try { fs.unlinkSync(outFile + ".before"); } catch {}
  return report;
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
  // A sub-assembly used in another configuration than the one it is imported in (a stand-in, above) is placed
  // otherwise for that reason, and has that configuration's mates: not taken for flexible
  const exp = exportOf(c.file);
  let active = null;
  try { active = exp && JSON.parse(fs.readFileSync(exp)).activeConfiguration; } catch {}
  if (c.configuration && active && c.configuration !== active) return null;
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
 * the sub-assembly's own file has it. Null if it has none (or no export to tell).
 */
function anchorOf(c, all) {
  const own = ownComponents(c);
  if (!own) return null;
  for (const x of all.filter(x => x.path.startsWith(c.path + "/") && !x.path.slice(c.path.length + 1).includes("/"))) {
    const mine = own.get(x.path.slice(c.path.length + 1));
    if (!mine || !mine.fixed || !mine.transform || !x.transform) continue;
    return { ...then(inverse(matrixOf(mine.transform)), matrixOf(x.transform)), by: mine.path };
  }
  return null;
}

/** A sub-assembly, imported once per file (and per configuration), as an assembly of its own. */
const subs = new Map();
function importSub(c, all) {
  const key = (c.file + "|" + (c.configuration || "")).toLowerCase();
  if (subs.has(key)) return subs.get(key);
  const exp = exportOf(c.file);
  const name = path.basename(c.file).replace(/\.sldasm$/i, "");
  const file = path.join(outDir, "sub-assemblies", `${name}${c.configuration && c.configuration !== "Default" ? " @ " + c.configuration : ""}.peet`);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  let made;
  try {
    if (exp) {
      const j = JSON.parse(fs.readFileSync(exp));
      if (!j.configurations || !j.configurations.length) throw new Error("its export is empty: " + (j.error || ""));
      const r = importAssembly(name, j, file);
      made = { path: file, report: r };
      if (c.configuration && c.configuration !== j.activeConfiguration) made.standIn = `it is used in its configuration "${c.configuration}" and imported in "${j.activeConfiguration}"`;
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
  };
  summary.push(row);
  console.log(`${r.verdict.padEnd(60)} ${name}  (components ${row.components - row.componentsLeft}/${row.components}, mates ${row.matesImported}/${row.mates}, off ${r.off_mm} mm)`);
}
fs.writeFileSync(path.join(outDir, "summary.json"), JSON.stringify(summary, null, 1));
