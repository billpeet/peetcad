//! The Phase 7 exit criterion, through the operations: an enclosure assembled from the
//! chassis sample, a cover, the housing and patterned fasteners is fully mated, has no
//! interference, has a bill of materials that matches the hand count and the
//! hand-calculated masses and flat sizes, and follows a change to the chassis's width.
//!
//! (The two timing criteria are checked where they can be measured: a mate solve during
//! a drag in `peet-model/tests/mates.rs`, drawing a thousand instances in
//! `peet-render/tests/gpu.rs`.)

use peet_document::Session;
use peet_model::samples::{self, chassis_size as cs, enclosure_size as es, housing_size as hs};
use peet_ops::{Headless, Undo, apply_session_json};
use serde_json::{Value, json};

struct Run {
    host: Headless,
    session: Session,
}

impl Run {
    fn ok(&mut self, op: Value) -> Value {
        let reply = apply_session_json(&mut self.host, &mut self.session, &op, Undo::Step);
        assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
        reply.json
    }
}

fn opened() -> Run {
    let mut run = Run {
        host: Headless::default(),
        session: Session::default(),
    };
    let made = run.ok(json!({"op": "open_sample", "sample": "assembly"}));
    assert_eq!(made["kind"], "assembly");
    assert_eq!(made["components"], 13);
    run
}

/// The components by name, with where each is.
fn places(listed: &Value) -> Vec<(String, [f64; 3])> {
    listed["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let at = c["at"].as_array().unwrap();
            (
                c["name"].as_str().unwrap().to_owned(),
                [0, 1, 2].map(|i| at[i].as_f64().unwrap()),
            )
        })
        .collect()
}

fn close(a: f64, b: f64, relative: f64) -> bool {
    (a - b).abs() <= relative * b.abs().max(1e-12)
}

#[test]
fn the_enclosure_is_fully_mated_and_nothing_interferes() {
    let mut run = opened();
    let listed = run.ok(json!({"op": "components"}));
    assert_eq!(listed["freedom"], 0, "nothing is left free");
    let components = listed["components"].as_array().unwrap();
    assert_eq!(components.len(), 13);
    for c in components {
        assert_eq!(c["status"], "ok", "{c}");
        assert_eq!(c["freedom"], 0, "{c}");
    }
    // One component is fixed; every other is held by mates or placed by a pattern.
    let fixed: Vec<&str> = components
        .iter()
        .filter(|c| c["fixed"] == true)
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(fixed, ["Chassis"]);
    let copies = components.iter().filter(|c| c.get("pattern").is_some());
    assert_eq!(copies.count(), 5 + 3);
    assert_eq!(listed["parts"].as_array().unwrap().len(), 5);
    let patterns = listed["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 2);
    assert_eq!(
        (patterns[0]["name"].as_str(), patterns[0]["type"].as_str()),
        (Some("Bolts"), Some("circular"))
    );
    assert_eq!(
        (patterns[1]["name"].as_str(), patterns[1]["type"].as_str()),
        (Some("Screws"), Some("linear"))
    );
    for p in patterns {
        assert_eq!(p["status"], "ok", "{p}");
    }

    let mates = run.ok(json!({"op": "mates"}));
    let all = mates["mates"].as_array().unwrap();
    assert_eq!(all.len(), 12);
    for m in all {
        assert_eq!(m["status"], "ok", "{m}");
    }
    let status = run.ok(json!({"op": "status"}));
    assert_eq!(status["failures"], json!([]));

    // Nothing runs into anything: screws in their holes, heads on their seats, the
    // housing on the cover and the cover on the rim only touch.
    let found = run.ok(json!({"op": "interference"}));
    assert_eq!(found["clear"], true, "{found}");
    assert_eq!(found["interferences"], json!([]));
    assert!(found.get("unchecked").is_none(), "{found}");
    assert!(found["compared"].as_u64().unwrap() >= 12, "{found}");
}

#[test]
fn the_bill_of_materials_matches_the_hand_count_and_masses() {
    let mut run = opened();
    let bom = run.ok(json!({"op": "bom"}));
    let rows = bom["rows"].as_array().unwrap();
    let row = |part: &str| {
        let found = rows.iter().find(|r| r["part"] == part);
        found.unwrap_or_else(|| panic!("no line for {part} in {bom}"))
    };
    // By hand: one chassis, one cover, one housing, six bolts, four screws.
    let counted: Vec<(&str, u64)> = rows
        .iter()
        .map(|r| (r["part"].as_str().unwrap(), r["quantity"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        counted,
        [
            ("Chassis", 1),
            ("Cover", 1),
            ("Housing", 1),
            ("Bolt M8", 6),
            ("Screw M4", 4)
        ]
    );
    assert_eq!(bom["quantity"], 13);

    // Masses, from the sizes: a volume in mm³ and a density in kg/m³.
    let kg = |volume: f64, density: f64| volume * density * 1e-9;
    let cover = kg(samples::cover_volume(), es::STEEL);
    let housing = kg(samples::housing_volume(), es::ALUMINIUM);
    let bolt = kg(samples::socket_screw_volume(es::M8), es::STEEL);
    let screw = kg(samples::socket_screw_volume(es::M4), es::STEEL);
    // The cover: 240 x 160 x 1.5 less a 32 mm opening and six 9 mm holes, in steel.
    let pi = std::f64::consts::PI;
    let by_hand = (240.0 * 160.0 - pi * 16.0 * 16.0 - 6.0 * pi * 4.5 * 4.5) * 1.5 * 7850e-9;
    assert!(
        close(cover, by_hand, 1e-12) && close(cover, 0.438_2, 1e-3),
        "{cover}"
    );
    for (part, material, mass, tolerance) in [
        ("Cover", "Mild steel", cover, 1e-6),
        // Curved bodies are measured from a fine mesh.
        ("Housing", "Aluminium 6061", housing, 3e-4),
        ("Bolt M8", "Alloy steel", bolt, 3e-4),
        ("Screw M4", "Alloy steel", screw, 3e-4),
    ] {
        let r = row(part);
        assert_eq!(r["material"], material);
        let got = r["mass_kg"].as_f64().unwrap();
        assert!(
            close(got, mass, tolerance),
            "{part}: {got} kg, by hand {mass}"
        );
        let total = r["total_mass_kg"].as_f64().unwrap();
        let quantity = r["quantity"].as_f64().unwrap();
        assert!(close(total, mass * quantity, tolerance), "{part}: {total}");
    }
    // The chassis: its flat blank by hand (as its DXF is checked in Phase 5), and its
    // mass from the volume the part itself reports.
    let chassis = row("Chassis");
    assert_eq!(chassis["material"], "Mild steel");
    assert_eq!(chassis["thickness"], cs::THICKNESS);
    assert_eq!(chassis["flat_size"], json!([333.57168, 215.876415]));
    assert_eq!(chassis["bends"], 7);
    let chassis_mass = chassis["mass_kg"].as_f64().unwrap();
    assert!(
        close(chassis_mass, kg(101_686.151_357, es::STEEL), 1e-6),
        "{chassis_mass}"
    );
    // The cover is a sheet too: flat, so its blank is its outline.
    let cover_row = row("Cover");
    assert_eq!(cover_row["thickness"], es::COVER_T);
    assert_eq!(cover_row["flat_size"], json!([cs::WIDTH, cs::DEPTH]));
    assert_eq!(cover_row["bends"], 0);
    assert!(row("Housing").get("flat_size").is_none());

    // The whole: the sum of the lines, and what `mass` weighs.
    let sum = chassis_mass + cover + housing + 6.0 * bolt + 4.0 * screw;
    let total = bom["mass_kg"].as_f64().unwrap();
    assert!(close(total, sum, 2e-4), "{total} / {sum}");
    let mass = run.ok(json!({"op": "mass"}));
    let weighed = mass["total"]["mass_kg"].as_f64().unwrap();
    assert!(close(weighed, sum, 2e-4), "{weighed} / {sum}");
    assert_eq!(mass["total"]["center_of_gravity_of"], "mass");
    assert!(mass.get("without_material").is_none());
    assert_eq!(mass["components"].as_array().unwrap().len(), 13);
}

#[test]
fn changing_the_chassis_width_moves_every_component_to_the_right_place() {
    let mut run = opened();
    let before = places(&run.ok(json!({"op": "components"})));
    let (hx, hy) = es::HOUSING_AT;
    let top = cs::WALL + es::COVER_T;
    let at = |all: &[(String, [f64; 3])], name: &str| {
        let found = all.iter().find(|(n, _)| n == name);
        found.unwrap_or_else(|| panic!("no component {name}")).1
    };
    assert_eq!(at(&before, "Cover"), [0.0, 0.0, cs::WALL]);
    assert_eq!(at(&before, "Housing"), [hx, hy, top]);

    // The chassis, opened from the assembly, made 40 wider, and stored back.
    let wider = 40.0;
    run.ok(json!({"op": "open_component", "component": "Chassis"}));
    let edited = run.ok(
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "d1", "value": cs::WIDTH + wider}),
    );
    // The chassis itself takes the change: nothing in it fails.
    assert!(edited.get("failures").is_none(), "{edited}");
    let chassis = run.ok(json!({"op": "status"}));
    assert_eq!(chassis["failures"], json!([]), "{chassis}");
    let bodies = run.ok(json!({"op": "bodies"}));
    assert_eq!(bodies["bodies"][0]["size"][0], cs::WIDTH + wider);
    run.ok(json!({"op": "save"}));
    run.ok(json!({"op": "close"}));

    // Every mate still holds and nothing is free: the faces the mates and the patterns
    // are on were found again in the changed part.
    let listed = run.ok(json!({"op": "components"}));
    assert_eq!(listed["freedom"], 0);
    for c in listed["components"].as_array().unwrap() {
        assert_eq!(c["status"], "ok", "{c}");
    }
    for p in listed["patterns"].as_array().unwrap() {
        assert_eq!(p["status"], "ok", "{p}");
    }
    let mates = run.ok(json!({"op": "mates"}));
    for m in mates["mates"].as_array().unwrap() {
        assert_eq!(m["status"], "ok", "{m}");
    }
    let after = places(&listed);
    assert_eq!(after.len(), 13);
    for (name, now) in &after {
        let was = at(&before, name);
        // What hangs on the right wall went with it: the cover (flush with that wall),
        // the housing on the cover, and the six bolts in the housing. What is in the
        // chassis's own holes, which are measured from its left, stayed in them.
        let moved = if name.starts_with("Screw") || name == "Chassis" {
            0.0
        } else {
            wider
        };
        let expected = [was[0] + moved, was[1], was[2]];
        for axis in 0..3 {
            assert!(
                (now[axis] - expected[axis]).abs() < 1e-6,
                "{name} is at {now:?}, not {expected:?}"
            );
        }
    }
    assert_eq!(at(&after, "Cover"), [wider, 0.0, cs::WALL]);
    // The bolts are still on their circle round the housing, the screws in the holes.
    let seat = top + hs::FLANGE_T - 8.6;
    let first = at(&after, "Bolt-1");
    assert!(
        (first[0] - (hx + wider + hs::BOLT_CIRCLE_R)).abs() < 1e-6
            && (first[2] - seat).abs() < 1e-6
    );
    let found = run.ok(json!({"op": "interference"}));
    assert_eq!(found["clear"], true, "{found}");
    // One undo step of the assembly takes it all back.
    run.ok(json!({"op": "undo"}));
    assert_eq!(places(&run.ok(json!({"op": "components"}))), before);
}

#[test]
fn the_enclosure_survives_its_file_and_goes_out_as_step() {
    let mut run = opened();
    let dir = std::env::temp_dir().join(format!("peet-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let listed = run.ok(json!({"op": "components"}));
    run.ok(json!({"op": "save", "path": path("enclosure.peet")}));
    let mut again = Run {
        host: Headless::default(),
        session: Session::default(),
    };
    again.ok(json!({"op": "open", "path": path("enclosure.peet")}));
    assert_eq!(again.ok(json!({"op": "components"})), listed);
    assert_eq!(again.session.model, run.session.model);

    // Five parts, thirteen placed occurrences.
    let out = run.ok(json!({"op": "export", "path": path("enclosure.step")}));
    assert_eq!(
        (
            out["parts"].as_u64(),
            out["components"].as_u64(),
            out["bodies"].as_u64()
        ),
        (Some(5), Some(13), Some(13))
    );
    // Shown exploded, it is still the same assembly.
    let steps = run.ok(json!({"op": "explode_steps"}));
    assert_eq!(steps["steps"].as_array().unwrap().len(), 4);
    run.ok(json!({"op": "explode"}));
    assert_eq!(run.ok(json!({"op": "components"})), listed);
    std::fs::remove_dir_all(dir).ok();
}
