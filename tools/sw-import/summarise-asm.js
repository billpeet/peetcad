#!/usr/bin/env node
/**
 * Counts what an assembly import run (import-asm.js) left out and why.
 *   node tools/sw-import/summarise-asm.js <output folder> [--assemblies]
 */
const fs = require("fs");
const path = require("path");
const [dir, flag] = process.argv.slice(2);
const reports = fs.readdirSync(dir).filter(f => f.endsWith(".report.json")).map(f => JSON.parse(fs.readFileSync(path.join(dir, f))));
const tally = new Map();
const add = (k, who) => { const t = tally.get(k) || { n: 0, who: new Set() }; t.n++; t.who.add(who); tally.set(k, t); };
const verdicts = new Map();
const general = s => String(s || "").replace(/In [^:]+: /, "").replace(/\{[^}]*\}/g, "{…}").replace(/\[[^\]]*\]/g, "[…]").replace(/"[^"]*"/g, "\"…\"").replace(/\d+(\.\d+)?/g, "N").slice(0, 140);
for (const r of reports) {
  verdicts.set(r.verdict, (verdicts.get(r.verdict) || 0) + 1);
  for (const c of r.components || []) if (c.result !== "inserted") add(`component ${c.result}: ${general(c.why)}`, r.name);
  for (const m of r.mates || []) if (m.result !== "imported") add(`${m.suppressed ? "suppressed " : ""}mate ${m.result}: ${m.result === "not supported" ? m.why : general(m.why)}`, r.name);
  for (const s of r.standIns || []) add(`stand-in: ${general(s.replace(/^[^:]+: /, ""))}`, r.name);
  for (const n of r.notes || []) add(`note: ${general(n)}`, r.name);
  for (const c of r.components || []) if (c.off_mm > 1e-3) add(`component placed off (freedom ${c.freedom})`, r.name);
}
console.log(`${reports.length} assemblies`);
for (const [v, n] of [...verdicts].sort((a, b) => b[1] - a[1])) console.log(String(n).padStart(4), v);
console.log("\nWhat was left out, failed or stood in for (count, assemblies):");
for (const [k, t] of [...tally].sort((a, b) => b[1].n - a[1].n)) console.log(String(t.n).padStart(4), `${k}  [${t.who.size}: ${[...t.who].slice(0, 4).join(", ")}${t.who.size > 4 ? ", …" : ""}]`);
if (flag === "--assemblies") for (const r of reports) {
  console.log(`\n${r.name}: ${r.verdict}, off ${r.off_mm} mm, freedom ${r.freedom}`);
  for (const c of r.components || []) if (c.result !== "inserted" || c.off_mm > 1e-3) console.log(`  component ${c.component}: ${c.result}${c.why ? " (" + c.why + ")" : ""}${c.off_mm > 1e-3 ? ` off ${c.off_mm} mm, freedom ${c.freedom}` : ""}`);
  for (const m of r.mates || []) if (m.result !== "imported") console.log(`  ${m.suppressed ? "(suppressed) " : ""}${m.mate} ${m.type}: ${m.result}: ${String(m.why).slice(0, 200)}`);
}
