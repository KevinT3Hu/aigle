//! Tone-curve shapes (doc 02 §6.4), shared by the renderer and the curve editor so that the
//! editor draws exactly what is rendered. Domain and values are sRGB-encoded in [0, 1].

use super::{CurvePoint, ParametricCurve, ToneCurve};

/// Parametric curve: four raised-cosine bumps centred at 1/8, 3/8, 5/8, 7/8 (shadows, darks,
/// lights, highlights), with an envelope that pins the end points. Not necessarily
/// monotone; [`ToneCurve`] users clamp and make it monotone.
pub fn parametric(p: &ParametricCurve, x: f64) -> f64 {
    let amounts = [p.shadows, p.darks, p.lights, p.highlights];
    let env = (8.0 * x * (1.0 - x)).min(1.0);
    let mut y = x;
    for (k, &a) in amounts.iter().enumerate() {
        let centre = 0.125 + 0.25 * k as f64;
        let d = (x - centre) / 0.25;
        if d.abs() < 1.0 {
            let bump = (std::f64::consts::FRAC_PI_2 * d).cos().powi(2);
            y += a as f64 / 100.0 * 0.25 * bump * env;
        }
    }
    y
}

pub fn parametric_is_identity(p: &ParametricCurve) -> bool {
    p.shadows == 0.0 && p.darks == 0.0 && p.lights == 0.0 && p.highlights == 0.0
}

/// Monotone cubic (Fritsch–Carlson) through points sorted by x; flat outside them.
#[derive(Clone, Debug)]
pub struct Spline {
    x: Vec<f64>,
    y: Vec<f64>,
    m: Vec<f64>,
}

impl Spline {
    /// `None` for fewer than two points (the identity).
    pub fn new(pts: &[CurvePoint]) -> Option<Spline> {
        if pts.len() < 2 {
            return None;
        }
        let x: Vec<f64> = pts.iter().map(|p| p.x as f64).collect();
        let y: Vec<f64> = pts.iter().map(|p| p.y as f64).collect();
        let n = x.len();
        let d: Vec<f64> = (0..n - 1)
            .map(|i| (y[i + 1] - y[i]) / (x[i + 1] - x[i]).max(1e-9))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            m[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                (d[i - 1] + d[i]) / 2.0
            };
        }
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
            } else {
                let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    m[i] = t * a * d[i];
                    m[i + 1] = t * b * d[i];
                }
            }
        }
        Some(Spline { x, y, m })
    }

    pub fn eval(&self, v: f64) -> f64 {
        let n = self.x.len();
        if v <= self.x[0] {
            return self.y[0];
        }
        if v >= self.x[n - 1] {
            return self.y[n - 1];
        }
        let i = self.x.partition_point(|&xi| xi <= v) - 1;
        let h = self.x[i + 1] - self.x[i];
        let t = (v - self.x[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.y[i]
            + (t3 - 2.0 * t2 + t) * h * self.m[i]
            + (-2.0 * t3 + 3.0 * t2) * self.y[i + 1]
            + (t3 - t2) * h * self.m[i + 1]
    }
}

/// A point curve with no effect: empty, one point, or the diagonal from 0 to 1.
pub fn points_are_identity(pts: &[CurvePoint]) -> bool {
    pts.len() < 2
        || (pts.iter().all(|q| q.x == q.y) && pts[0].x == 0.0 && pts[pts.len() - 1].x == 1.0)
}

/// The identity point curve the editor starts from.
pub fn identity_points() -> Vec<CurvePoint> {
    vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }]
}

impl ToneCurve {
    pub fn is_identity(&self) -> bool {
        parametric_is_identity(&self.parametric)
            && self.point.all().iter().all(|p| points_are_identity(p))
    }

    /// The parametric curve followed by the RGB point curve, made monotone and clamped,
    /// sampled at `n` evenly spaced inputs.
    pub fn master_table(&self, n: usize) -> Vec<f64> {
        let rgb = Spline::new(&self.point.rgb);
        let mut out = Vec::with_capacity(n);
        let mut prev = 0.0f64;
        for i in 0..n {
            let x = i as f64 / (n.max(2) - 1) as f64;
            let mut y = parametric(&self.parametric, x).clamp(0.0, 1.0).max(prev);
            prev = y;
            if let Some(r) = &rgb {
                y = r.eval(y);
            }
            out.push(y.clamp(0.0, 1.0));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spline_passes_through_points_and_stays_monotone() {
        let pts = [
            CurvePoint { x: 0.0, y: 0.0 },
            CurvePoint { x: 0.3, y: 0.5 },
            CurvePoint { x: 0.6, y: 0.55 },
            CurvePoint { x: 1.0, y: 1.0 },
        ];
        let s = Spline::new(&pts).unwrap();
        for p in pts {
            assert!((s.eval(p.x as f64) - p.y as f64).abs() < 1e-9);
        }
        let ys: Vec<f64> = (0..=200).map(|i| s.eval(i as f64 / 200.0)).collect();
        assert!(ys.windows(2).all(|w| w[1] >= w[0] - 1e-12));
        assert!(points_are_identity(&identity_points()));
        assert!(!points_are_identity(&pts));
        assert!(ToneCurve::default().is_identity());
    }
}
