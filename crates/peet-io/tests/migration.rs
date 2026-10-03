//! Files saved by earlier versions still open.
//!
//! `fixtures/v4` holds the four samples as saved with model schema 4, the last one
//! before configurations. They must open as the same parts, each with the one
//! configuration every part starts with.

use peet_io::document::{Metadata, open, save};
use peet_kernel::validate::measure;
use peet_model::{Engine, Model, samples};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/v4/{name}.peet",
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
        // The same part as the sample is today.
        assert_eq!(model, now, "{name}");
        let v = volume(&mut model);
        assert!(v > 0.0, "{name}");

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
