#!/usr/bin/env node
// Says where an imported part differs from SolidWorks: the area of its flat faces, feature by feature and
// direction by direction, in both. A difference in volume shows up as a face that is larger or smaller.
//
//   node diagnose.js <export.swx.json> <imported .peet> [--peet <peet executable>]
"use strict";
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const args = process.argv.slice(2);
const at = args.indexOf("--peet");
const repo = path.resolve(__dirname, "..", "..");
const peetExe = at >= 0 ? args.splice(at, 2)[1]
  : [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe"), "peet"].find(p => p === "peet" || fs.existsSync(p));
if (args.length !== 2) {
  console.error("Usage: node diagnose.js <export.swx.json> <imported .peet> [--peet <peet executable>]");
  process.exit(2);
}
const doc = JSON.parse(fs.readFileSync(args[0], "utf8"));
const config = doc.configurations.find(c => c.name === doc.activeConfiguration) || doc.configurations[0];
const reply = spawnSync(peetExe, ["faces", "-f", args[1], "-q", "--headless"], { encoding: "utf8", maxBuffer: 1 << 28 });
const faces = JSON.parse((reply.stdout || "").split("\n").find(l => l.startsWith("{")) || "{}").faces || [];

const direction = n => n.map(v => Math.abs(v) < 0.02 ? 0 : +v.toFixed(2)).join(",");
// PeetCAD's pieces of one SolidWorks flange are numbered (Edge-Flange11, 12, ...) and its stand-ins are named
// after the feature ("X corners", "X sketch"): both are counted under the feature's own name.
const names = new Set();
// A bend is a feature under its flange in SolidWorks and part of the flange in PeetCAD: it is counted with
// the flange.
const flangeOf = new Map();
(function walk(list, parent) { for (const f of list || []) { names.add(f.name); if (parent && /Bend$/.test(f.type)) flangeOf.set(f.name, parent.name); walk(f.sub, f); } })(config.features, null);
const owner = name => {
  if (names.has(name)) return name;
  const trimmed = name.replace(/ (corners|sketch|flange\d*)$/, "");
  if (names.has(trimmed)) return trimmed;
  const unnumbered = name.replace(/\d$/, "");
  return names.has(unnumbered) ? unnumbered : name;
};

const table = new Map();
const add = (feature, dir, which, area) => {
  const key = feature + " | " + dir;
  if (!table.has(key)) table.set(key, [0, 0]);
  table.get(key)[which] += area;
};
let theirs = 0, ours = 0;
for (const b of config.bodies || []) for (const f of b.faces) {
  theirs += f.area * 1e6;
  add(flangeOf.get(f.feature) || f.feature || "?", f.normal ? direction(f.normal) : f.surface, 0, f.area * 1e6);
}
for (const f of faces) {
  ours += f.area_mm2;
  // A face cut from one feature by another carries both: it is counted under the first, as SolidWorks does
  const made = (f.made_by || [])[0];
  add(made ? owner(made.feature) : "?", f.surface === "plane" ? direction(f.normal) : f.surface, 1, f.area_mm2);
}

console.log(`${doc.name}: area in SolidWorks ${theirs.toFixed(2)}, in PeetCAD ${ours.toFixed(2)} (${(ours - theirs).toFixed(2)})`);
const rows = [...table.entries()].map(([k, [a, b]]) => [k, a, b, b - a]).filter(r => Math.abs(r[3]) > 0.05).sort((x, y) => Math.abs(y[3]) - Math.abs(x[3]));
console.log("feature | direction".padEnd(46) + "SolidWorks".padStart(12) + "PeetCAD".padStart(12) + "difference".padStart(12));
for (const [k, a, b, d] of rows.slice(0, 24)) console.log(k.padEnd(46) + a.toFixed(2).padStart(12) + b.toFixed(2).padStart(12) + d.toFixed(2).padStart(12));
if (rows.length === 0) console.log("(no face differs by more than 0.05 mm²)");
