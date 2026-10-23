use super::arcs::{Bezier, fit};
use super::*;
use crate::region::regions_of_curves;
use crate::sketch::EntityId;

fn drawn(string: &str, font: &str, height: f64) -> SketchText {
    let mut t = SketchText::new(string);
    t.font = font.to_owned();
    t.height = height;
    t.layout().unwrap();
    t
}

fn bounds(t: &SketchText) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for c in t.curves(DVec2::ZERO) {
        let (a, b) = c.bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    (lo, hi)
}

#[test]
fn built_in_font_is_the_height_asked() {
    let t = drawn("HI", font::FALLBACK, 10.0);
    assert_eq!(t.drawn_in, font::FALLBACK);
    assert!(!t.outline.is_empty());
    let (lo, hi) = bounds(&t);
    // H and I sit on the baseline and reach the capital height.
    assert!(lo.y.abs() < 1e-6, "{lo}");
    assert!((hi.y - 10.0).abs() < 1e-6, "{hi}");
    assert!(lo.x >= 0.0 && hi.x > 10.0);
}

#[test]
fn outline_closes_into_regions() {
    let t = drawn("O", font::FALLBACK, 10.0);
    let curves: Vec<(EntityId, Curve)> = t.curves(DVec2::ZERO).map(|c| (EntityId(1), c)).collect();
    let profile = regions_of_curves(&curves);
    assert!(profile.open_ends.is_empty(), "{:?}", profile.open_ends);
    // The ring and its counter.
    assert_eq!(profile.regions.len(), 2);
}

#[test]
fn a_missing_font_falls_back_with_a_warning() {
    let mut t = SketchText::new("A");
    t.font = "No Such Font 123".into();
    let warning = t.layout().unwrap();
    assert!(warning.unwrap().contains("not installed"));
    assert_eq!(t.drawn_in, font::FALLBACK);
}

#[test]
fn aligned_and_turned() {
    let mut t = drawn("H", font::FALLBACK, 10.0);
    let (lo, hi) = bounds(&t);
    t.align = TextAlign::Right;
    t.layout().unwrap();
    let (lo2, hi2) = bounds(&t);
    assert!(hi2.x < 0.0 && (hi2.x - lo2.x - (hi.x - lo.x)).abs() < 1e-9);
    t.align = TextAlign::Left;
    t.angle = std::f64::consts::FRAC_PI_2;
    t.layout().unwrap();
    let (lo3, hi3) = bounds(&t);
    assert!((lo3.x + hi.y).abs() < 1e-6);
    assert!((hi3.y - hi.x).abs() < 1e-6 && (lo3.y - lo.x).abs() < 1e-6);
}

/// Biarcs stay within the tolerance of the curve: the area under a quadratic and a cubic
/// with an inflection agree with a fine polygon of them.
#[test]
fn arcs_follow_the_curves() {
    let pieces = [
        Bezier(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(5.0, 10.0),
            DVec2::new(10.0, 0.0),
        ]),
        Bezier(vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(3.0, 8.0),
            DVec2::new(7.0, -8.0),
            DVec2::new(10.0, 0.0),
        ]),
    ];
    for piece in pieces {
        let mut out = Vec::new();
        fit(&piece, 1e-4, &mut out);
        assert!(out.iter().any(|c| matches!(c, GlyphCurve::Arc { .. })));
        // Every point of the curve is near the arcs.
        for i in 0..=200 {
            let t = f64::from(i) / 200.0;
            let p = bezier_point(&piece.0, t);
            let d = out
                .iter()
                .map(|c| c.placed(DVec2::ZERO, 0.0).distance(p))
                .fold(f64::INFINITY, f64::min);
            assert!(d < 1.5e-4, "{d} at {t}");
        }
        // And the arcs join end to end.
        for w in out.windows(2) {
            let (a, b) = (w[0].placed(DVec2::ZERO, 0.0), w[1].placed(DVec2::ZERO, 0.0));
            let ends = [a.start(), a.end()];
            assert!(
                ends.iter()
                    .any(|p| p.distance(b.start()) < 1e-9 || p.distance(b.end()) < 1e-9)
            );
        }
    }
}

fn bezier_point(p: &[DVec2], t: f64) -> DVec2 {
    let mut p = p.to_vec();
    for k in (1..p.len()).rev() {
        for i in 0..k {
            p[i] = p[i].lerp(p[i + 1], t);
        }
    }
    p[0]
}

/// Century Gothic, where it is installed, as SolidWorks draws it: hinted at 108 pixels
/// to the em, its capital height 78 pixels, so each edge of a T is on that grid.
#[test]
fn century_gothic_as_solidworks_draws_it() {
    if font::installed_font(DEFAULT_FONT).is_none() {
        return;
    }
    // Plinth PFC Long F's "TF", 12.7 mm high: SolidWorks' edges are at these
    // multiples of 12.7 / 39 from the point.
    let t = drawn("TF", DEFAULT_FONT, 12.7);
    let (lo, hi) = bounds(&t);
    let u = 12.7 / 39.0;
    eprintln!("TF: {} .. {}", lo / u, hi / u);
    assert!((lo.x / u - 0.5).abs() < 1e-6, "{}", lo.x / u);
    assert!((hi.x / u - 46.0).abs() < 1e-6, "{}", hi.x / u);
    assert!((hi.y - 12.7).abs() < 1e-9);
    // HingeMale's "MALE", 3.5 mm high: from 3 to 142.5.
    let t = drawn("MALE", DEFAULT_FONT, 3.5);
    let (lo, hi) = bounds(&t);
    let u = 3.5 / 39.0;
    eprintln!("MALE: {} .. {}", lo / u, hi / u);
    assert!((lo.x / u - 3.0).abs() < 1e-6, "{}", lo.x / u);
    assert!((hi.x / u - 142.5).abs() < 1e-6, "{}", hi.x / u);
    // Plinth Folded Long B's bold "T", 15 mm high: its bar from 0.5 to 21.5, 7 deep.
    if font::installed_font("Century Gothic Bold").is_some() {
        let t = drawn("T", "Century Gothic Bold", 15.0);
        let (lo, hi) = bounds(&t);
        let u = 15.0 / 39.0;
        assert!((lo.x / u - 0.5).abs() < 1e-6 && (hi.x / u - 21.5).abs() < 1e-6);
    }
}
