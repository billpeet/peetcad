// Tallies a folder of exports (*.swx.json): which features, definition values, sketch geometry and relations
// the library uses, and how often. Counts are for each document's active configuration unless they say otherwise.
//
//   node tally.js <folder of exports> [> tally.json]
const fs = require("fs");
const path = require("path");

const dir = process.argv[2];
if (!dir) {
  console.error("Usage: node tally.js <folder of exports>");
  process.exit(2);
}

const NOISE = /Folder$|Light$|Cabinet$|^AnnotationViewFeat$|^OriginProfileFeature$|^SelectionSetNode$|^Attribute$|^GridFeature$/;

/** A counter of values, with the documents each was seen in. */
class Tally {
  constructor() { this.map = new Map(); }
  add(key, doc, n = 1) {
    key = String(key);
    let e = this.map.get(key);
    if (!e) this.map.set(key, e = { n: 0, docs: new Set() });
    e.n += n;
    if (doc) e.docs.add(doc);
  }
  out() {
    return Object.fromEntries([...this.map.entries()].sort((a, b) => b[1].n - a[1].n).map(([k, e]) => [k, `${e.n} in ${e.docs.size}`]));
  }
}
const tallies = {};
const T = name => tallies[name] || (tallies[name] = new Tally());
const round = (v, unit = 1000, places = 3) => typeof v === "number" ? +(v * unit).toFixed(places) : v;
const deg = v => typeof v === "number" ? +(v * 180 / Math.PI).toFixed(2) : v;

const totals = { parts: 0, assemblies: 0, sheetMetalParts: 0, multiBodyParts: 0, partsWithEquations: 0 };

function walk(features, visit, parent = null, depth = 0) {
  for (const f of features || []) {
    visit(f, parent, depth);
    walk(f.sub, visit, f, depth + 1);
  }
}

function sketchOn(s) {
  if (!s.on) return "nothing";
  if (s.on.kind === "feature") return /^(Front|Top|Right) Plane$/.test(s.on.name) ? "a standard plane" : s.on.name ? "a reference plane" : "an unnamed plane (a flange's own)";
  if (s.on.kind === "face") return s.on.surface === "plane" ? "a flat face" : "a " + s.on.surface + " face";
  return s.on.kind;
}

function entityKind(e) {
  if (!e) return "nothing";
  if (e.kind === "edge") return "edge: " + e.curve;
  if (e.kind === "feature") return e.type === "RefPlane" ? "reference plane" : "feature: " + e.type;
  if (e.kind === "face") return "face: " + e.surface;
  return e.kind;
}

function definitionValues(f, doc) {
  const v = f.definition && f.definition.values;
  if (!v) return;
  const t = f.type;
  const add = (what, value) => T(`${t}: ${what}`).add(value, doc);
  switch (t) {
    case "SheetMetal":
      add("thickness mm", round(v.Thickness));
      add("bend radius mm", round(v.BendRadius));
      add("bend allowance type (1 table, 2 K-factor, 3 allowance, 4 deduction)", v.GetCustomBendAllowance ? v.GetCustomBendAllowance.type : v.BendAllowanceType);
      add("auto relief type", v.UseAutoRelief ? v.AutoReliefType : "none");
      add("relief ratio", v.ReliefRatio);
      break;
    case "SMBaseFlange":
      add("reverse thickness", v.D2ReverseOffset === true);
      add("offset directions", v.OffsetDirections);
      break;
    case "EdgeFlange":
      add("position type (1 material inside, 2 material outside, 3 bend outside, 4 virtual sharp, 5 tangent)", v.PositionType);
      add("angle deg", deg(v.BendAngle));
      add("edges", v.GetEdgeCount);
      add("own bend radius", v.UseDefaultBendRadius === false ? round(v.BendRadius) : "default");
      add("offset type", v.OffsetType);
      add("length measured (OffsetDimType)", v.OffsetDimType);
      add("position offset", v.UsePositionOffset ? "yes" : "no");
      add("trim side bends", v.UsePositionTrimSideBends === true);
      add("own relief", v.UseDefaultBendRelief === false ? `type ${v.AutoReliefType}` : "default");
      add("own bend allowance", v.UseDefaultBendAllowance === false);
      break;
    case "OneBend":
      add("angle deg", deg(v.BendAngle));
      add("down", v.BendDown === true);
      break;
    case "ICE": case "Cut": case "Extrusion": case "Boss": case "ExtruThin":
      add("end condition forward (0 blind, 1 through all, 2 through next, 3 up to vertex, 4 up to surface, 5 offset from surface, 6 mid plane, 9 through all both)", v["GetEndCondition(true)"]);
      add("both directions", v.BothDirections === true);
      add("normal cut", v.NormalCut === true);
      add("link to thickness", v.LinkToThickness === true);
      add("flip side to cut", v.FlipSideToCut === true);
      add("draft", v["GetDraftWhileExtruding(true)"] === true);
      add("thin wall", v.ThinWallType !== undefined && v.ThinWallType >= 0 && t === "ExtruThin");
      add("from (0 sketch plane)", v.FromType);
      break;
    case "HoleWzd":
      add("type", v.Type);
      add("kind by name", /Slot/i.test(f.name) ? "slot" : /CBORE|Counterbore/i.test(f.name) ? "counterbore" : /CSK|Countersink/i.test(f.name) ? "countersink" : /Tap|M\d/i.test(f.name) ? "tapped or metric" : "plain");
      add("end condition", v.EndCondition);
      add("diameter mm", round(v.ThruHoleDiameter || v.HoleDiameter || v.Diameter));
      add("points", v.GetSketchPointCount);
      break;
    case "LPattern":
      for (const p of v.PatternFeatureArray || []) add("of", p && p.type);
      add("grid", (v.D2TotalInstances || 1) > 1);
      add("direction from", entityKind(v.D1Axis));
      add("instances", v.D1TotalInstances);
      add("skips instances", (v.SkippedItemArray || []).length > 0);
      add("patterns faces or bodies", (v.PatternFaceArray || []).length > 0 || (v.PatternBodyArray || []).length > 0);
      break;
    case "MirrorPattern":
      for (const p of v.PatternFeatureArray || []) add("of", p && p.type);
      add("about", entityKind(v.Plane));
      add("mirrors faces or bodies", (v.PatternFaceArray || []).length > 0 || (v.PatternBodyArray || []).length > 0);
      break;
    case "CirPattern":
      for (const p of v.PatternFeatureArray || []) add("of", p && p.type);
      break;
    case "BreakCorner":
      add("size mm", round(v.Distance));
      for (const e of v.Entities || []) add("on", entityKind(e));
      break;
    case "Fillet":
      add("radius mm", round(v.DefaultRadius));
      add("type", v.Type);
      break;
    case "SketchBend":
      add("angle deg", deg(v.BendAngle));
      add("position", v.PositionType);
      break;
    case "RefPlane":
      if (!/^(Front|Top|Right) Plane$/.test(f.name)) {
        add("type", v.Type2 !== undefined ? v.Type2 : v.Type);
        add("selections", v.GetSelectionsCount);
        for (const s of v.Selections || []) add("from", entityKind(s));
        add("distance mm", round(v.Distance));
        add("angle deg", deg(v.Angle));
      }
      break;
  }
}

for (const file of fs.readdirSync(dir).filter(f => f.endsWith(".swx.json")).sort()) {
  const doc = JSON.parse(fs.readFileSync(path.join(dir, file), "utf8"));
  const name = doc.name;
  if (!doc.completed) T("documents not fully exported").add(name, name);
  const active = doc.configurations.find(c => c.name === doc.activeConfiguration) || doc.configurations[0];
  const real = doc.configurations.filter(c => !c.flatPattern);

  if (doc.documentType === "assembly") {
    totals.assemblies++;
    walk(active.features, f => {
      if (NOISE.test(f.type)) return;
      T("assembly feature types").add(f.type + (f.suppressed ? " (suppressed)" : ""), name);
      if (f.mate && !f.mate.error) {
        T("mate types").add(f.mate.type + (f.suppressed ? " (suppressed)" : ""), name);
        T("mate alignment (0 aligned, 1 anti-aligned, 2 closest)").add(f.mate.alignment, name);
        for (const e of f.mate.entities) T("mate ends").add(e.entity ? entityKind(e.entity) : `empty (reference type ${e.referenceType})${f.suppressed ? ", suppressed mate" : ""}`, name);
        const paths = f.mate.entities.map(e => (e.component || "").split("/").length);
        T("mate reaches into a sub-assembly").add(paths.some(p => p > 1), name);
      }
    });
    for (const c of active.components || []) {
      T("component nesting depth").add(c.path.split("/").length, name);
      T("component suppression (0 suppressed, 2 resolved)").add(c.suppression, name);
      T("component fixed").add(c.fixed === true, name);
    }
    T("assembly configurations").add(real.length, name);
    continue;
  }

  totals.parts++;
  const bodies = active.bodies || [];
  if (bodies.some(b => b.sheetMetal)) totals.sheetMetalParts++;
  if (bodies.length !== 1) totals.multiBodyParts++;
  T("bodies per part").add(bodies.length, name);
  T("configurations per part (flat-pattern ones left out)").add(real.length, name);
  if ((active.equations || []).length) totals.partsWithEquations++;

  for (const b of bodies) {
    for (const f of b.faces) {
      T("face surfaces").add(f.surface, name);
      if (f.name) T("named faces by surface").add(f.surface, name);
    }
    T("named edges").add(b.namedEdges.length > 0, name);
  }

  for (const e of active.equations || []) {
    const x = e.expression;
    T("equations").add(e.disabled ? "disabled" : "enabled", name);
    T("equation kinds").add(
      /suppressed/i.test(x) ? "suppresses a feature by condition"
        : e.globalVariable ? "global variable"
          : /\bi?if\s*\(/i.test(x) ? "a dimension by condition"
            : "a dimension by formula", name);
    for (const m of x.matchAll(/\b([a-z]+)\s*\(/gi)) T("equation functions").add(m[1].toLowerCase(), name);
  }

  walk(active.features, (f, parent) => {
    if (NOISE.test(f.type)) return;
    const inFlat = parent && parent.type === "FlatPattern";
    if (inFlat) return;
    T("feature types").add(f.type, name);
    if (f.suppressed) T("feature types, suppressed").add(f.type, name);
    if (f.errorCode) T("features with an error").add(f.type, name);
    if (f.definition && f.definition.error) T("definitions not read").add(f.type, name);
    definitionValues(f, name);

    if (f.sketch && !f.sketch.error) {
      const s = f.sketch;
      T("sketches on").add(sketchOn(s), name);
      T("sketches: constrained status (2 under, 3 fully, 4 over, 5 no solution)").add(s.constrainedStatus, name);
      if (s.is3D) T("3D sketches").add(f.suppressed ? "suppressed" : "live", name);
      if (s.blockInstances) T("sketches with blocks").add("yes", name);
      T("sketch is used by").add(parent ? parent.type : (f.children || []).length ? "a later feature" : "nothing", name);
      for (const g of s.segments) T("sketch segments").add(g.type + (g.construction ? " (construction)" : "") + (g.type === "arc" ? (g.circle ? ": circle" : ": arc") : ""), name);
      for (const r of s.relations) {
        T("sketch relations").add(r.type, name);
        for (const e of r.defined || []) T("sketch relations to the model").add(`${r.type} to ${entityKind(e)}`, name);
        for (const e of r.defined || []) T("model references from sketches").add(entityKind(e), name);
      }
    }
    for (const d of f.dimensions || []) {
      if (f.sketch) T("sketch dimension display types (2 linear, 3 angular, 5 radial, 6 diameter)").add(d.displayType, name);
      if (f.sketch && !/^D\d+$/.test(d.name)) T("sketch dimensions with a name of their own").add("yes", name);
      if (d.drivenState === 1 && f.sketch) T("driven sketch dimensions").add("yes", name);
    }
  });

  // What differs between a part's configurations
  if (real.length > 1) {
    const flat = c => {
      const m = {};
      walk(c.features, f => {
        if (NOISE.test(f.type)) return;
        m[`suppression of a ${f.type}`] = (m[`suppression of a ${f.type}`] || "") + (f.suppressed ? "1" : "0");
        for (const d of f.dimensions || []) m["dim " + d.fullName] = round(d.value, 1, 9);
      });
      return m;
    };
    const base = flat(real[0]);
    const seen = new Set();
    for (const c of real.slice(1)) {
      const other = flat(c);
      for (const k of Object.keys(base)) if (base[k] !== other[k]) seen.add(k.startsWith("dim ") ? (/@Sketch|@.*Sketch/i.test(k) ? "a sketch dimension" : "a feature dimension") : k);
    }
    for (const k of seen) T("what differs between configurations").add(k, name);
    if (seen.size === 0) T("what differs between configurations").add("nothing found", name);
  }
}

console.log(JSON.stringify({ totals, ...Object.fromEntries(Object.entries(tallies).sort((a, b) => a[0].localeCompare(b[0])).map(([k, t]) => [k, t.out()])) }, null, 1));
