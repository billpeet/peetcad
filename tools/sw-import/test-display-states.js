#!/usr/bin/env node
/**
 * Tests display-states.js on a hand-made format 8 export (there is no real one yet): the plan it works out, and
 * that plan made in a PeetCAD assembly by `peet`.
 *
 *   node tools/sw-import/test-display-states.js [--peet <peet>]
 */
const fs = require("fs");
const os = require("os");
const path = require("path");
const assert = require("assert");
const { spawnSync } = require("child_process");
const { displayPlan, applyDisplayPlan, componentLookOps } = require("./display-states");

const args = process.argv.slice(2);
const at = args.indexOf("--peet");
const repo = path.resolve(__dirname, "..", "..");
const peetExe = at >= 0 ? args[at + 1]
  : [path.join(repo, "target", "release", "peet.exe"), path.join(repo, "target", "debug", "peet.exe")].find(p => fs.existsSync(p)) || "peet";

// Red, green, blue, ambient, diffuse, specular, shininess, transparency, emission
const values = (r, g, b, t = 0) => [r, g, b, 0.5, 1, 0.5, 0.3, t, 0];
const grey = values(0.75, 0.75, 0.75);
const component = (p, extra = {}) => ({ path: p, file: `C:\\Lib\\${p.split("-")[0]}.SLDPRT`, configuration: "Default", modelAppearance: grey, appearance: grey, visible: 1, ...extra });

// Two configurations, display states linked: Default has "Working" (active: the lid see-through by its appearance,
// the tray blue) and "Lid off" (the lid hidden, the tray transparent by SolidWorks' Change Transparency only);
// Long has one display state, with the tray red, and a component inside a sub-assembly hidden.
const exp = {
  formatVersion: 8,
  linkDisplayStatesToConfigurations: true,
  activeConfiguration: "Default",
  configurations: [
    {
      name: "Default",
      components: [component("Tray-1", { appearance: values(0.125, 0.25, 0.75) }), component("Lid-1", { appearance: values(0.75, 0.75, 0.75, 0.6) })],
      displayStates: [
        {
          name: "Working", active: true, components: [
            { path: "Tray-1", visible: 1, displayMode: 8, transparency: 1, appearance: values(0.125, 0.25, 0.75), appearances: [] },
            { path: "Lid-1", visible: 1, displayMode: 8, transparency: 0, appearances: [{ rgb: [191, 191, 191], transparency: 0.6, entities: [{ kind: "component", path: "Lid-1" }] }] },
          ],
        },
        {
          name: "Lid off", active: false, components: [
            { path: "Tray-1", visible: 1, displayMode: 0, transparency: 0 },
            { path: "Lid-1", visible: 0, displayMode: 8, transparency: 1 },
          ],
        },
      ],
    },
    {
      name: "Long",
      components: [component("Tray-1"), component("Lid-1"), component("Sub-1", { file: "C:\\Lib\\Sub.SLDASM" }), component("Sub-1/Inner-1")],
      displayStates: [
        {
          name: "Long look", active: true, components: [
            { path: "Tray-1", visible: 1, appearance: values(1, 0, 0) },
            { path: "Lid-1", visible: 1 },
            { path: "Sub-1/Inner-1", visible: 0 },
          ],
        },
      ],
    },
  ],
};

// ---- The plan ----
const plan = displayPlan(exp, exp.configurations[0], ["Tray-1", "Lid-1"]);
assert.strictEqual(plan.linked, true);
assert.deepStrictEqual(plan.configurations.map(c => [c.name, c.active, c.states.map(s => s.name)]),
  [["Default", true, ["Working", "Lid off"]], ["Long", false, ["Long look"]]]);
const [working, lidOff] = plan.configurations[0].states;
assert.strictEqual(working.active, true);
assert.deepStrictEqual(working.looks, [
  { component: "Tray-1", visible: true, color: "#2040bf", transparency: 0 },
  { component: "Lid-1", visible: true, color: null, transparency: 0.6 },
]);
assert.deepStrictEqual(lidOff.looks, [
  { component: "Tray-1", visible: true, color: null, transparency: 0.75 },
  { component: "Lid-1", visible: false, color: null, transparency: 0 },
]);
assert.ok(plan.standIns.some(s => s.includes("Tray-1") && s.includes("75%")), plan.standIns.join("\n"));
assert.ok(plan.notes.some(n => n.includes("display mode 0")), plan.notes.join("\n"));
assert.ok(plan.notes.some(n => n.includes("Sub-1/Inner-1")), plan.notes.join("\n"));
assert.deepStrictEqual(plan.configurations[1].states[0].looks, [{ component: "Tray-1", visible: true, color: "#ff0000", transparency: 0 }]);
// Before format 8 there is nothing; without display states, the components' own looks
assert.strictEqual(displayPlan({ configurations: [{ name: "Default", components: [] }] }, null, []), null);
assert.deepStrictEqual(componentLookOps(exp.configurations[0], ["Tray-1", "Lid-1"]), [
  { op: "set_color", component: "Tray-1", color: "#2040bf" },
  { op: "set_transparency", component: "Lid-1", transparency: 0.6 },
]);
// Unlinked: the active configuration's display states are all of them
const unlinked = displayPlan({ ...exp, linkDisplayStatesToConfigurations: false }, exp.configurations[0], ["Tray-1", "Lid-1"]);
assert.deepStrictEqual(unlinked.configurations.map(c => c.name), ["Default"]);

// ---- Made in PeetCAD ----
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "peet-display-"));
const file = path.join(dir, "a.peet");
const run = (ops, save = true) => {
  const r = spawnSync(peetExe, ["run", "-", "-f", file, "-q", ...(save ? [] : ["--no-save"])],
    { input: ops.map(o => JSON.stringify(o)).join("\n") + "\n", encoding: "utf8" });
  return (r.stdout || "").split("\n").filter(l => l.startsWith("{")).map(l => JSON.parse(l));
};
const made = spawnSync(peetExe, ["op", JSON.stringify({ op: "new", assembly: true, discard: true }), JSON.stringify({ op: "save", path: file }), "-q"], { encoding: "utf8" });
assert.strictEqual(made.status, 0, made.stderr || made.stdout);
const setup = run([
  { op: "insert", sample: "chassis", name: "Tray-1" },
  { op: "insert", sample: "cover", name: "Lid-1", at: [0, 0, 60] },
  { op: "add_configuration", name: "Long" },
  { op: "configuration", configuration: "Default" },
]);
assert.ok(setup.every(r => r.ok), JSON.stringify(setup));
// What import-asm.js's Peet does
const peet = {
  apply(op) { return run([op])[0] || { ok: false, error: "no reply" }; },
  applyAll(ops) { return ops.map(op => this.apply(op)); },
};
const report = { standIns: [] };
applyDisplayPlan(peet, plan, report);
assert.deepStrictEqual(report.displayStates.failed, []);
const [listed, components] = run([{ op: "display_states" }, { op: "components" }], false);
assert.strictEqual(listed.linked, true);
assert.strictEqual(listed.shown, "Working");
const byName = Object.fromEntries(listed.display_states.map(s => [s.name, s]));
assert.deepStrictEqual(Object.keys(byName).sort(), ["Lid off", "Long look", "Working"]);
assert.strictEqual(byName["Long look"].configuration, "Long");
assert.deepStrictEqual(byName.Working.looks, [{ component: "Tray-1", color: "#2040bf" }, { component: "Lid-1", transparency: 0.6 }]);
assert.deepStrictEqual(byName["Lid off"].looks, [{ component: "Tray-1", transparency: 0.75 }, { component: "Lid-1", hidden: true }]);
assert.strictEqual(components.components[0].color, "#2040bf");
// Long shows its own
const [, long] = run([{ op: "configuration", configuration: "Long" }, { op: "display_states" }], false);
assert.strictEqual(long.shown, "Long look");
fs.rmSync(dir, { recursive: true, force: true });
console.log("display states: ok");
