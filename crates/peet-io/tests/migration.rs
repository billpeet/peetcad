//! Files saved by earlier versions still open.
//!
//! `fixtures/v4` holds the four samples as saved with model schema 4, the last one
//! before configurations. They must open as the same parts, each with the one
//! configuration every part starts with.
//!
//! `fixtures/v5` holds parts saved with model schema 5, the first with configurations
//! (which could differ in suppression and parameters only): the enclosure panel with
//! three configurations, saved with `Thick` active, and the bracket with one.

use peet_io::document::{Metadata, open, save};
use peet_kernel::validate::measure;
use peet_model::{Engine, Model, samples};

fn fixture(name: &str) -> Vec<u8> {
    fixture_of("v4", name)
}

fn fixture_of(version: &str, name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/{version}/{name}.peet",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn volume(model: &mut Model) -> f64 {
    let mut engine = Engine::new();
    let eval = engine.regenerate(model);
    assert!(eval.failures().next().is_none(), "every feature builds");
    eval.bodies.iter().map(|b| measure::volume(&b.solid)).sum()
}

#[test]
fn version_4_samples_open_as_parts_with_one_configuration() {
    for (name, (now, _)) in [
        ("bracket", samples::bracket()),
        ("enclosure", samples::enclosure()),
        ("chassis", samples::chassis()),
        ("housing", samples::housing()),
    ] {
        let opened = open(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(opened.warnings.is_empty(), "{name}: {:?}", opened.warnings);
        let mut model = opened.model;
        assert_eq!(model.configurations().len(), 1, "{name}");
        assert_eq!(model.active_configuration().name, "Default", "{name}");
        let v = volume(&mut model);
        assert!(v > 0.0, "{name}");
        if name == "chassis" {
            // The sample has changed since the file was saved: its rim is now sketched
            // at the left corner of the base, so that the part can be made wider. The
            // file still has it at the right, and builds the same solid.
            let mut now = now;
            let today = volume(&mut now);
            assert!((v - today).abs() < 1e-6, "{v} / {today}");
        } else {
            // The same part as the sample is today.
            assert_eq!(model, now, "{name}");
        }

        // Saved again it is a current file, and the same part.
        let again = open(&save(&model, &Metadata::default(), None).unwrap()).unwrap();
        assert_eq!(again.model, model, "{name}");
    }
}

#[test]
fn a_version_4_cache_is_dropped_and_the_part_rebuilt() {
    let opened = open(&fixture("bracket_cached")).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    assert!(opened.bodies.is_none() && opened.meshes.is_empty());
    let mut model = opened.model;
    let v = volume(&mut model);
    assert!((v - samples::bracket_volume(120.0)).abs() < 1e-6, "{v}");
}

#[test]
fn version_5_parts_open_with_their_configurations() {
    let opened = open(&fixture_of("v5", "enclosure_family")).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    let mut model = opened.model;
    let names: Vec<&str> = model
        .configurations()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["Default", "Thick", "Blank"]);
    assert_eq!(model.active_configuration().name, "Thick");
    assert_eq!(model.active_configuration().comment, "2 mm sheet");
    assert_eq!(model.parameters.get("thickness"), Some(2.0));
    let default = model.configuration_named("Default").unwrap().id;
    let blank = model.configuration_named("Blank").unwrap().id;
    assert_eq!(model.parameter_in("flange", default), Some("25mm"));
    assert_eq!(model.features_that_differ().len(), 2);
    for id in model.features_that_differ() {
        assert_eq!(model.suppressed_in(id, blank), Some(true));
        assert_eq!(model.suppressed_in(id, default), Some(false));
    }
    assert!(model.values_that_differ().is_empty());
    assert!(volume(&mut model) > 0.0);
    for c in [default, blank] {
        assert!(volume(&mut model.with_configuration(c)) > 0.0);
    }
    let again = open(&save(&model, &Metadata::default(), None).unwrap()).unwrap();
    assert_eq!(again.model, model);

    let mut bracket = open(&fixture_of("v5", "bracket")).unwrap().model;
    assert_eq!(bracket.configurations().len(), 1);
    assert_eq!(bracket, samples::bracket().0);
    assert!(volume(&mut bracket) > 0.0);
}
