//! Tone-curve tables for D2 (doc 02 §6.4): parametric region curve ∘ point curve (RGB) ∘
//! point curve (per channel), sampled into one 4096-entry table per channel. Domain and
//! values are sRGB-encoded (what the histogram shows). The shapes live in
//! `aigle_core::params::tone`, shared with the curve editor.

use aigle_core::params::ToneCurve;
use aigle_core::params::tone::Spline;
use aigle_kernels::pointwise::CURVE_N;

/// `3 × CURVE_N` table (R, G, B), or `None` when the curve is the identity.
pub fn build(c: &ToneCurve) -> Option<Vec<f32>> {
    if c.is_identity() {
        return None;
    }
    let master = c.master_table(CURVE_N);
    let chans = [
        Spline::new(&c.point.red),
        Spline::new(&c.point.green),
        Spline::new(&c.point.blue),
    ];
    let mut out = Vec::with_capacity(3 * CURVE_N);
    for ch in &chans {
        for &m in &master {
            let v = ch.as_ref().map_or(m, |c| c.eval(m));
            out.push(v.clamp(0.0, 1.0) as f32);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::params::CurvePoint;

    #[test]
    fn identity_and_shapes() {
        let mut c = ToneCurve::default();
        assert!(build(&c).is_none());
        c.point.rgb = vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }];
        assert!(build(&c).is_none());
        // Darken the midtones: monotone, end points kept.
        c.point.rgb = vec![
            CurvePoint { x: 0.0, y: 0.0 },
            CurvePoint { x: 0.5, y: 0.35 },
            CurvePoint { x: 1.0, y: 1.0 },
        ];
        let t = build(&c).unwrap();
        assert!(t[CURVE_N / 2] < 0.4);
        assert!(t.windows(2).take(CURVE_N - 1).all(|w| w[1] >= w[0]));
        assert_eq!(t[0], 0.0);
        assert_eq!(t[CURVE_N - 1], 1.0);
        // Parametric shadows lift dark tones but keep black.
        let mut c = ToneCurve::default();
        c.parametric.shadows = 80.0;
        let t = build(&c).unwrap();
        assert!(t[CURVE_N / 8] > 0.125 + 0.05);
        assert_eq!(t[0], 0.0);
    }
}
