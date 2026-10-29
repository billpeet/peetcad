/**
 * Display states and components' looks from an assembly's export (format 8: tools/sw-export), for import-asm.js.
 *
 * SolidWorks keeps, per configuration, display states (`displayStates`, the active one first), each with every
 * component's visibility, display mode, transparency and colour in it; and whether display states are linked to
 * configurations (`linkDisplayStatesToConfigurations`). PeetCAD has the same (model schema 12): display states,
 * linked or not, each holding the assembly's own components' looks (shown, colour, transparency).
 *
 * `displayPlan` works out what to make from an export (no PeetCAD needed: it is tested on a hand-made export by
 * test-display-states.js); `applyDisplayPlan` makes it with operations.
 */

/** SolidWorks' MaterialPropertyValues (red, green, blue from 0 to 1, ambient, diffuse, specular, shininess,
 * transparency, emission) as a colour and a transparency, or null where there is none. */
function fromValues(v) {
  if (Array.isArray(v) && v.length === 1 && Array.isArray(v[0])) v = v[0];
  if (!Array.isArray(v) || v.length < 3) return null;
  const rgb = v.slice(0, 3);
  if (rgb.some(x => typeof x !== "number" || !(x >= 0 && x <= 1))) return null;
  const t = v[7];
  return { rgb: rgb.map(x => Math.round(x * 255)), transparency: typeof t === "number" && t >= 0 && t <= 1 ? t : null };
}

/** A render material (an appearance) of the export as a colour and a transparency. */
function fromAppearance(a) {
  if (!a || !Array.isArray(a.rgb) || a.rgb.length !== 3) return null;
  const t = a.transparency;
  return { rgb: a.rgb, transparency: typeof t === "number" && t >= 0 && t <= 1 ? t : null };
}

const hex = rgb => "#" + rgb.map(x => Math.max(0, Math.min(255, x)).toString(16).padStart(2, "0")).join("");
const sameColour = (a, b) => a && b && a.every((x, i) => Math.abs(x - b[i]) <= 1);

/** The appearance of `path` among a display state's appearances: one on the component itself, or on nothing named. */
function ownAppearance(list, path) {
  for (const a of list || []) {
    const on = Array.isArray(a.entities) ? a.entities : [];
    if (!on.length || on.some(e => e && e.kind === "component" && e.path === path)) {
      const look = fromAppearance(a);
      if (look) return look;
    }
  }
  return null;
}

/** swComponentVisibilityState_e (0 hidden, 1 shown), swVisibilityState_e (1 hidden, 2 shown), else what applying it gave. */
function visibleOf(entry) {
  if (entry.visible === 0 || entry.visible === 1) return entry.visible === 1;
  if (entry.visibility === 1 || entry.visibility === 2) return entry.visibility === 2;
  if (entry.appliedVisible === 0 || entry.appliedVisible === 1) return entry.appliedVisible === 1;
  return true;
}

// swDisplayMode_e: shaded (3), shaded with edges (7) and the default (8) are how PeetCAD draws every component
const DRAWN_AS_PEETCAD = new Set([3, 7, 8, -1]);

/**
 * What to make: `{ linked, configurations: [{ name, active, states: [{ name, active, looks }] }], notes, standIns }`.
 * `looks` holds, for each of `components` (the assembly's own, as imported: their paths), what is not the default
 * (shown, its part's colour, opaque): `{ component, visible, color, transparency }`. `active` is the export's active
 * configuration. Null where the export has no display states (before format 8, or none could be read).
 */
function displayPlan(exp, active, components) {
  const notes = [];
  const standIns = [];
  const configs = exp.configurations || [];
  if (!configs.some(c => Array.isArray(c.displayStates) && c.displayStates.length)) return null;
  // SolidWorks' default is linked; the export reads it two ways
  const linked = exp.linkDisplayStatesToConfigurations ?? exp.linkedDisplayState ?? true;
  const own = new Set(components);
  const plan = { linked: !!linked, configurations: [], notes, standIns };
  // Unlinked, every configuration has every display state: the active configuration's list is all of them
  const listed = linked ? [active, ...configs.filter(c => c !== active)] : [active];
  for (const cfg of listed) {
    const states = cfg.displayStates || [];
    if (!states.length) { notes.push(`${cfg.name}: no display states in the export`); continue; }
    const byPath = new Map((cfg.components || []).map(c => [c.path, c]));
    const out = { name: cfg.name, active: cfg === active, states: [] };
    for (const s of states) {
      const looks = [];
      for (const entry of s.components || []) {
        const c = byPath.get(entry.path);
        if (!own.has(entry.path)) {
          // PeetCAD's display states hold the assembly's own components; one inside a sub-assembly is drawn as
          // the sub-assembly's own display state has it
          if (entry.path.includes("/") && (!visibleOf(entry) || entry.transparency === 0))
            notes.push(`${cfg.name} / ${s.name}: ${entry.path} is inside a sub-assembly: its look there is not imported`);
          continue;
        }
        const visible = visibleOf(entry);
        // The colour SolidWorks gives the component in this display state, from the display state's settings, its
        // appearances, or (the active display state) the component's own colour
        const look = fromValues(entry.appearance) || ownAppearance(entry.appearances, entry.path)
          || (s.active && c ? fromValues(c.appearance) : null);
        // Its part's own colour: the component has one of its own only where it differs
        const part = c ? fromValues(c.modelAppearance) : null;
        const color = look && part && !sameColour(look.rgb, part.rgb) ? hex(look.rgb) : null;
        let transparency = look && look.transparency > 0 ? look.transparency : 0;
        if (!transparency && entry.transparency === 0) {
          // swTransparencyStateTransparent with no transparency in its appearance: SolidWorks' Change
          // Transparency, drawn at its own level
          transparency = 0.75;
          standIns.push(`${cfg.name} / ${s.name}: ${entry.path} is transparent (SolidWorks' Change Transparency) with no transparency in its appearance: 75% assumed`);
        }
        if (typeof entry.displayMode === "number" && !DRAWN_AS_PEETCAD.has(entry.displayMode))
          notes.push(`${cfg.name} / ${s.name}: ${entry.path} is drawn in display mode ${entry.displayMode} (swDisplayMode_e): PeetCAD draws every component shaded with edges`);
        if (c && /\.sldasm$/i.test(c.file || "") && entry.referencedDisplayState)
          notes.push(`${cfg.name} / ${s.name}: ${entry.path} shows its display state "${entry.referencedDisplayState}": PeetCAD shows the one the sub-assembly has shown`);
        if (visible && !color && !transparency) continue;
        looks.push({ component: entry.path, visible, color, transparency: +transparency.toFixed(4) });
      }
      out.states.push({ name: s.name, active: !!s.active, looks });
    }
    // The active display state is the first SolidWorks lists; one is shown in each configuration
    if (!out.states.some(s => s.active)) out.states[0].active = true;
    plan.configurations.push(out);
  }
  return plan;
}

/**
 * Makes `plan` in the assembly `peet` holds, whose active configuration is the plan's active one: every display
 * state added first (each starts as the components look then, all the default), then each component's look set in
 * the display states it differs in, then each configuration's active display state shown. Fills `report` with
 * what was made and what was not.
 */
function applyDisplayPlan(peet, plan, report) {
  const out = { linked: plan.linked, configurations: [], notes: plan.notes, failed: [] };
  report.displayStates = out;
  report.standIns.push(...plan.standIns);
  if (plan.linked) {
    const r = peet.apply({ op: "link_display_states" });
    if (!r.ok) { out.failed.push(`link_display_states: ${r.error}`); return out; }
  }
  const ids = [];
  for (const cfg of plan.configurations) {
    const row = { name: cfg.name, states: [] };
    out.configurations.push(row);
    for (const s of cfg.states) {
      const op = { op: "add_display_state", name: s.name, ...(cfg.active ? {} : { configuration: cfg.name }) };
      const r = peet.apply(op);
      if (!r.ok) { out.failed.push(`${cfg.name} / ${s.name}: ${r.error}`); continue; }
      ids.push({ cfg, s, id: r.display_state.id, row: { name: s.name, active: s.active, looks: 0, lookFailures: 0 } });
      row.states.push(ids[ids.length - 1].row);
    }
  }
  for (const { s, id, row } of ids) {
    const ops = [];
    for (const l of s.looks) {
      const at = { component: l.component, display_states: [id] };
      if (!l.visible) ops.push({ op: "show", ...at, on: false });
      if (l.color) ops.push({ op: "set_color", ...at, color: l.color });
      if (l.transparency) ops.push({ op: "set_transparency", ...at, transparency: l.transparency });
    }
    const replies = peet.applyAll(ops);
    row.looks = s.looks.length;
    ops.forEach((op, i) => {
      if (!(replies[i] && replies[i].ok)) {
        row.lookFailures++;
        out.failed.push(`${s.name}: ${op.op} ${op.component}: ${replies[i] ? replies[i].error : "no reply"}`);
      }
    });
  }
  // The active display state of each configuration; the active configuration's last, so it is the one shown
  for (const { cfg, s, id } of [...ids.filter(x => !x.cfg.active), ...ids.filter(x => x.cfg.active)]) {
    if (!s.active) continue;
    const r = peet.apply({ op: "display_state", display_state: id });
    if (!r.ok) out.failed.push(`${cfg.name}: showing ${s.name}: ${r.error}`);
  }
  return out;
}

/**
 * Without display states (an export before format 8, or none read): each component's look as the export has it now
 * (format 8 `visible`, `appearance`, `modelAppearance`), set on the component. Returns the operations.
 */
function componentLookOps(cfg, components) {
  const ops = [];
  const own = new Set(components);
  for (const c of cfg.components || []) {
    if (!own.has(c.path)) continue;
    if (c.visible === 0) ops.push({ op: "show", component: c.path, on: false });
    const look = fromValues(c.appearance);
    const part = fromValues(c.modelAppearance);
    if (look && part && !sameColour(look.rgb, part.rgb)) ops.push({ op: "set_color", component: c.path, color: hex(look.rgb) });
    if (look && look.transparency > 0) ops.push({ op: "set_transparency", component: c.path, transparency: +look.transparency.toFixed(4) });
  }
  return ops;
}

module.exports = { displayPlan, applyDisplayPlan, componentLookOps, fromValues };
