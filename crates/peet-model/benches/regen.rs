//! Rebuild timings: `cargo bench -p peet-model`.
//!
//! Prints the time to rebuild the 20-feature sample bracket after a change to the first
//! sketch (everything downstream rebuilds), after a change to the last feature, after no
//! change at all, and from scratch. The budget from the roadmap is 100 ms for the first.

use std::time::Instant;

use peet_model::samples::bracket;
use peet_model::{Engine, Model};

fn set_width(model: &mut Model, width: f64) {
    let first = model.features().next().unwrap().id;
    let params = model.parameters.clone();
    let sketch = &mut model
        .feature_mut(first)
        .and_then(|f| f.sketch_mut())
        .unwrap()
        .sketch;
    let d1 = sketch.dimension_by_name("d1").unwrap();
    peet_sketch::expr::set_dimension_input(sketch, &params, d1, &width.to_string()).unwrap();
}

/// Runs `f` `runs` times and returns (best, median) in milliseconds.
fn time(runs: usize, mut f: impl FnMut(usize)) -> (f64, f64) {
    let mut samples: Vec<f64> = (0..runs)
        .map(|i| {
            let start = Instant::now();
            f(i);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    (samples[0], samples[runs / 2])
}

fn main() {
    let (mut model, mut engine) = bracket();
    let runs = 30;
    let report = |what: &str, (best, median): (f64, f64)| {
        println!("{what:<44} best {best:7.2} ms   median {median:7.2} ms");
    };
    report(
        "first sketch changed (all downstream)",
        time(runs, |i| {
            set_width(&mut model, 120.0 + (i % 7) as f64 * 5.0 + 1.0);
            engine.regenerate(&mut model);
        }),
    );
    let last_cut = model
        .features()
        .rev()
        .find(|f| f.extrude().is_some())
        .unwrap()
        .id;
    report(
        "last solid feature changed",
        time(runs, |i| {
            if let Some(e) = model.feature_mut(last_cut).and_then(|f| f.extrude_mut()) {
                e.params.reverse = i % 2 == 0;
            }
            engine.regenerate(&mut model);
        }),
    );
    report(
        "nothing changed",
        time(runs, |_| {
            engine.regenerate(&mut model);
        }),
    );
    report(
        "from scratch",
        time(runs, |_| {
            let mut fresh = Engine::new();
            fresh.regenerate(&mut model);
        }),
    );
}
