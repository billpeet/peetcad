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
// The assembled geometry against SolidWorks' mass properties (format 6 exports): volume and box, every configuration
const massed = reports.flatMap(r => (r.configurations || []).filter(c => c.mass && !c.mass.error).map(c => ({ ...c.mass, at: `${r.name} (${c.name})` })));
if (massed.length) {
  const off = massed.filter(m => !m.matches);
  console.log(`assembled geometry: ${massed.length - off.length} of ${massed.length} configurations match SolidWorks' volume and box${off.length ? `; not: ${off.map(m => `${m.at} volume x${m.volume_ratio}${m.box_outside_mm ? `, box ${m.box_outside_mm} mm out` : ""}`).join("; ")}` : ""}`);
}
for (const [v, n] of [...verdicts].sort((a, b) => b[1] - a[1])) console.log(String(n).padStart(4), v);
// In-context references (format 7 exports): each relation of a part's sketch to another component
// Relations of in-context sketches that tie only the part's own geometry are the part's: not counted
const contexts = reports.flatMap(r => (r.inContext || []).filter(x => x.result !== "the part's own").map(x => ({ ...x, assembly: r.name })));
if (contexts.length) {
  const made = contexts.filter(x => x.result === "imported");
  const hand = contexts.filter(x => x.handMade).length;
  console.log(`in-context references: ${made.length} of ${contexts.length} follow their components (${new Set(made.map(x => x.assembly)).size} assemblies)${hand ? `; ${hand} from hand-made format 7 data` : ""}`);
  for (const x of contexts.filter(x => x.result !== "imported")) add(`in-context reference ${x.result}: ${general(x.why)}`, x.assembly);
}
// The configurations other than the active one, each checked against SolidWorks' placements for it
const others = reports.flatMap(r => (r.configurations || []).filter(c => !c.active).map(c => ({ ...c, assembly: r.name })));
if (others.length) {
  const byVerdict = new Map();
  for (const c of others) byVerdict.set(c.verdict, (byVerdict.get(c.verdict) || 0) + 1);
  const live = others.reduce((n, c) => n + (c.matesLive || 0), 0), imported = others.reduce((n, c) => n + (c.matesImported || 0), 0);
  console.log(`\n${others.length} other configurations (of ${new Set(others.map(c => c.assembly)).size} assemblies), ${imported} of ${live} live mates imported:`);
  for (const [v, n] of [...byVerdict].sort((a, b) => b[1] - a[1])) console.log(String(n).padStart(4), v);
}
console.log("\nWhat was left out, failed or stood in for (count, assemblies):");
for (const [k, t] of [...tally].sort((a, b) => b[1].n - a[1].n)) console.log(String(t.n).padStart(4), `${k}  [${t.who.size}: ${[...t.who].slice(0, 4).join(", ")}${t.who.size > 4 ? ", …" : ""}]`);
if (flag === "--assemblies") for (const r of reports) {
  console.log(`\n${r.name}: ${r.verdict}, off ${r.off_mm} mm, freedom ${r.freedom}`);
  for (const c of r.components || []) if (c.result !== "inserted" || c.off_mm > 1e-3) console.log(`  component ${c.component}: ${c.result}${c.why ? " (" + c.why + ")" : ""}${c.off_mm > 1e-3 ? ` off ${c.off_mm} mm, freedom ${c.freedom}` : ""}`);
  for (const m of r.mates || []) if (m.result !== "imported") console.log(`  ${m.suppressed ? "(suppressed) " : ""}${m.mate} ${m.type}: ${m.result}: ${String(m.why).slice(0, 200)}`);
  for (const c of (r.configurations || []).filter(c => !c.active)) {
    console.log(`  configuration ${c.name}: ${c.verdict}, off ${c.off_mm} mm, live mates ${c.matesImported}/${c.matesLive}`);
    for (const o of c.off || []) console.log(`    component ${o.component} off ${o.off_mm} mm, freedom ${o.freedom}`);
    for (const m of c.mates || []) if (!m.suppressed && m.result !== "imported") console.log(`    ${m.mate} ${m.type}: ${m.result}: ${String(m.why).slice(0, 200)}`);
    for (const n of c.notes || []) console.log(`    note: ${n}`);
  }
}
