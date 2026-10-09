#!/usr/bin/env node
// Sums up a folder of import reports: how many parts match SolidWorks, and what stopped the others.
//
//   node summarise.js <folder of reports> [--parts]
"use strict";
const fs = require("fs");
const path = require("path");

const dir = process.argv[2];
if (!dir) {
  console.error("Usage: node summarise.js <folder of reports> [--parts]");
  process.exit(2);
}
const reports = fs.readdirSync(dir).filter(f => f.endsWith(".report.json")).map(f => JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")));

const verdicts = {};
const reasons = new Map();
for (const r of reports) {
  verdicts[r.verdict] = (verdicts[r.verdict] || 0) + 1;
  const seen = new Set();
  for (const f of r.features || []) {
    if (f.result !== "failed" && f.result !== "not supported") continue;
    // One line per kind of reason: sizes, names and coordinates are taken out
    const why = (f.message || "").replace(/\{.*\}/g, "{…}").replace(/\(.*?\)/g, "(…)").replace(/-?\d+(\.\d+)?/g, "N").replace(/'[^']*'/g, "'…'").slice(0, 120);
    const key = `${f.type}: ${f.result}${why ? ", " + why : ""}`;
    if (!reasons.has(key)) reasons.set(key, { features: 0, parts: 0 });
    reasons.get(key).features++;
    if (!seen.has(key)) { reasons.get(key).parts++; seen.add(key); }
  }
}

console.log(`${reports.length} parts`);
for (const [v, n] of Object.entries(verdicts).sort((a, b) => b[1] - a[1])) console.log(`${String(n).padStart(5)}  ${v}`);
console.log("\nWhat was left out or failed (features, parts):");
for (const [k, v] of [...reasons.entries()].sort((a, b) => b[1].parts - a[1].parts)) console.log(`${String(v.features).padStart(5)} ${String(v.parts).padStart(4)}  ${k}`);

if (process.argv.includes("--parts")) {
  console.log("\nParts that differ with nothing left out:");
  for (const r of reports.filter(r => r.verdict === "differs")) {
    const d = r.difference || {};
    console.log(`  ${r.name}: volume ${d.volume === null ? "?" : (d.volume * 100).toFixed(3) + "%"}, box ${d.box_mm === null ? "?" : d.box_mm.toFixed(3) + " mm"}, flat ${d.flat_mm === null || d.flat_mm === undefined ? "?" : d.flat_mm.toFixed(3) + " mm"}`);
  }
}
