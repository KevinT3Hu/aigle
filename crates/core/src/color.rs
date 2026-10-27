//! Colour-science helpers in f64 (doc 02 §6). Kernels receive the folded f32 matrices.
//!
//! Working space: linear Rec.2020, D65.

/// Row-major 3×3 matrix.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Linear Rec.2020 → CIE XYZ (D65).
pub const REC2020_TO_XYZ: Mat3 = [
    [
        0.636_958_048_301_291,
        0.144_616_903_586_208,
        0.168_880_975_164_172,
    ],
    [
        0.262_700_212_011_267,
        0.677_998_071_518_871,
        0.059_301_716_469_862,
    ],
    [
        0.000_000_000_000_000,
        0.028_072_693_049_087,
        1.060_985_057_710_791,
    ],
];

/// Linear sRGB / Rec.709 → CIE XYZ (D65).
pub const SRGB_TO_XYZ: Mat3 = [
    [
        0.412_390_799_265_959,
        0.357_584_339_383_878,
        0.180_480_788_401_834,
    ],
    [
        0.212_639_005_871_510,
        0.715_168_678_767_756,
        0.072_192_315_360_734,
    ],
    [
        0.019_330_818_715_592,
        0.119_194_779_794_626,
        0.950_532_152_249_661,
    ],
];

/// Linear Display P3 (DCI-P3 primaries, D65) → CIE XYZ.
pub const DISPLAY_P3_TO_XYZ: Mat3 = [
    [
        0.486_570_948_648_216,
        0.265_667_693_169_093,
        0.198_217_285_234_363,
    ],
    [
        0.228_974_564_069_749,
        0.691_738_521_836_506,
        0.079_286_914_093_745,
    ],
    [
        0.000_000_000_000_000,
        0.045_113_381_858_903,
        1.043_944_368_900_976,
    ],
];

/// RGB → XYZ matrix from primaries and white chromaticities (white normalised to Y = 1).
pub fn primaries_to_xyz(r: (f64, f64), g: (f64, f64), b: (f64, f64), w: (f64, f64)) -> Mat3 {
    let cols = [
        xy_to_xyz(r.0, r.1),
        xy_to_xyz(g.0, g.1),
        xy_to_xyz(b.0, b.1),
    ];
    let m = [
        [cols[0][0], cols[1][0], cols[2][0]],
        [cols[0][1], cols[1][1], cols[2][1]],
        [cols[0][2], cols[1][2], cols[2][2]],
    ];
    let s = mul_vec(
        &invert(&m).expect("primaries are independent"),
        xy_to_xyz(w.0, w.1),
    );
    [
        [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
        [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
        [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
    ]
}

/// CAT16 forward matrix (XYZ → sharpened LMS), Li et al. 2017.
pub const CAT16: Mat3 = [
    [0.401_288, 0.650_173, -0.051_461],
    [-0.250_268, 1.204_414, 0.045_854],
    [-0.002_079, 0.048_952, 0.953_127],
];

/// D65 white point chromaticity.
pub const D65_XY: (f64, f64) = (0.3127, 0.3290);

/// ICC PCS illuminant (D50) chromaticity, as XYZ (0.9642, 1, 0.8249).
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

/// Bradford cone response matrix (Lam 1985), the ICC chromatic adaptation.
pub const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Bradford adaptation from white `src` to white `dst` (XYZ).
pub fn bradford(src: [f64; 3], dst: [f64; 3]) -> Mat3 {
    let ls = mul_vec(&BRADFORD, src);
    let ld = mul_vec(&BRADFORD, dst);
    let gain = diag([ld[0] / ls[0], ld[1] / ls[1], ld[2] / ls[2]]);
    let inv = invert(&BRADFORD).expect("Bradford invertible");
    mul(&inv, &mul(&gain, &BRADFORD))
}

/// Linear Adobe RGB (1998) → XYZ (D65).
pub fn adobe_rgb_to_xyz() -> Mat3 {
    primaries_to_xyz((0.64, 0.33), (0.21, 0.71), (0.15, 0.06), D65_XY)
}

/// Linear ProPhoto (ROMM) RGB → XYZ relative to its native D50 white.
pub fn prophoto_to_xyz_d50() -> Mat3 {
    primaries_to_xyz((0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001), D50_XY)
}

/// Linear ProPhoto RGB → XYZ (D65): native matrix followed by Bradford D50 → D65, so
/// ProPhoto white maps to D65 like the other spaces.
pub fn prophoto_to_xyz() -> Mat3 {
    mul(
        &bradford(xy_to_xyz(D50_XY.0, D50_XY.1), xy_to_xyz(D65_XY.0, D65_XY.1)),
        &prophoto_to_xyz_d50(),
    )
}

/// Rec.2020 luminance weights.
pub const REC2020_LUMA: [f64; 3] = [0.2627, 0.6780, 0.0593];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

pub fn mul_vec(a: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

pub fn diag(d: [f64; 3]) -> Mat3 {
    [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]
}

pub fn scale(a: &Mat3, s: f64) -> Mat3 {
    a.map(|r| r.map(|v| v * s))
}

pub fn invert(m: &Mat3) -> Option<Mat3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-15 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ])
}

pub fn to_f32(m: &Mat3) -> [[f32; 3]; 3] {
    m.map(|r| r.map(|v| v as f32))
}

/// xy chromaticity → XYZ with Y = 1.
pub fn xy_to_xyz(x: f64, y: f64) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

pub fn xyz_to_xy(v: [f64; 3]) -> (f64, f64) {
    let s = v[0] + v[1] + v[2];
    (v[0] / s, v[1] / s)
}

/// CIE 1960 uv → xy.
pub fn uv_to_xy(u: f64, v: f64) -> (f64, f64) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

/// xy → CIE 1960 uv.
pub fn xy_to_uv(x: f64, y: f64) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

/// DNG convention (dng_temperature.cpp): tint = −3000 × offset along the isotherm. The
/// isotherm direction points towards +u/−v (magenta), so positive tint is magenta.
const TINT_SCALE: f64 = -3000.0;

/// Robertson (1968) isotherm table as used by the DNG SDK: (mired, u, v, isotherm slope) in
/// CIE 1960 uv, 0–600 mired (∞–1667 K).
const ROBERTSON: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

fn unit(du: f64, dv: f64) -> (f64, f64) {
    let l = (du * du + dv * dv).sqrt();
    (du / l, dv / l)
}

/// Temperature (K) and tint → xy chromaticity of the scene illuminant (DNG SDK
/// `dng_temperature::Get_xy_coord`).
pub fn cct_tint_to_xy(temperature: f64, tint: f64) -> (f64, f64) {
    let r = 1e6 / temperature.max(1.0);
    let offset = tint / TINT_SCALE;
    let n = ROBERTSON.len();
    let mut i = 0;
    while i < n - 2 && r >= ROBERTSON[i + 1].0 {
        i += 1;
    }
    let (r0, u0, v0, t0) = ROBERTSON[i];
    let (r1, u1, v1, t1) = ROBERTSON[i + 1];
    let f = ((r1 - r) / (r1 - r0)).clamp(0.0, 1.0);
    let mut u = u0 * f + u1 * (1.0 - f);
    let mut v = v0 * f + v1 * (1.0 - f);
    let (a1, b1) = unit(1.0, t0);
    let (a2, b2) = unit(1.0, t1);
    let (du, dv) = unit(a1 * f + a2 * (1.0 - f), b1 * f + b2 * (1.0 - f));
    u += du * offset;
    v += dv * offset;
    uv_to_xy(u, v)
}

/// xy → (temperature K, tint) (DNG SDK `dng_temperature::Set_xy_coord`). Inverse of
/// [`cct_tint_to_xy`].
pub fn xy_to_cct_tint(x: f64, y: f64) -> (f64, f64) {
    let (u, v) = xy_to_uv(x, y);
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    let n = ROBERTSON.len();
    for i in 1..n {
        let (du, dv) = unit(1.0, ROBERTSON[i].3);
        let (uu, vv) = (u - ROBERTSON[i].1, v - ROBERTSON[i].2);
        let mut dt = -uu * dv + vv * du;
        if dt <= 0.0 || i == n - 1 {
            if dt > 0.0 {
                dt = 0.0;
            }
            dt = -dt;
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let (p, q) = (ROBERTSON[i - 1], ROBERTSON[i]);
            let temperature = 1e6 / (p.0 * f + q.0 * (1.0 - f));
            let uu = u - (p.1 * f + q.1 * (1.0 - f));
            let vv = v - (p.2 * f + q.2 * (1.0 - f));
            let (du, dv) = unit(du * (1.0 - f) + last_du * f, dv * (1.0 - f) + last_dv * f);
            return (temperature, (uu * du + vv * dv) * TINT_SCALE);
        }
        last_dt = dt;
        last_du = du;
        last_dv = dv;
    }
    unreachable!("loop returns at the last index")
}

/// CAT16 (full adaptation, D = 1) from source white `src` to destination white `dst`, both XYZ.
pub fn cat16(src: [f64; 3], dst: [f64; 3]) -> Mat3 {
    let ls = mul_vec(&CAT16, src);
    let ld = mul_vec(&CAT16, dst);
    let gain = diag([ld[0] / ls[0], ld[1] / ls[1], ld[2] / ls[2]]);
    let inv = invert(&CAT16).expect("CAT16 invertible");
    mul(&inv, &mul(&gain, &CAT16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn inverse_and_white() {
        let inv = invert(&REC2020_TO_XYZ).unwrap();
        let id = mul(&inv, &REC2020_TO_XYZ);
        for (i, row) in id.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!(approx(*v, if i == j { 1.0 } else { 0.0 }, 1e-12));
            }
        }
        // Rec.2020 white is D65.
        let (x, y) = xyz_to_xy(mul_vec(&REC2020_TO_XYZ, [1.0, 1.0, 1.0]));
        assert!(approx(x, D65_XY.0, 1e-4) && approx(y, D65_XY.1, 1e-4));
    }

    #[test]
    fn cct_round_trip() {
        for &(t, tint) in &[
            (2850.0, 0.0),
            (5000.0, 10.0),
            (6504.0, -20.0),
            (12000.0, 5.0),
        ] {
            let (x, y) = cct_tint_to_xy(t, tint);
            let (t2, tint2) = xy_to_cct_tint(x, y);
            assert!(approx(t, t2, t * 1e-4), "{t} -> {t2}");
            assert!(approx(tint, tint2, 0.05), "{tint} -> {tint2}");
        }
        // Illuminant A is close to the locus at ~2856 K.
        let (t, tint) = xy_to_cct_tint(0.44757, 0.40745);
        assert!(approx(t, 2856.0, 15.0), "{t}");
        assert!(tint.abs() < 3.0, "{tint}");
        // D65 lies above the locus (green side): the DNG SDK reports about 6504 K, tint +10.
        let (t, tint) = xy_to_cct_tint(D65_XY.0, D65_XY.1);
        assert!(approx(t, 6504.0, 30.0), "{t}");
        assert!(approx(tint, 10.0, 2.0), "{tint}");
    }

    #[test]
    fn primaries_reproduce_standard_matrices() {
        let p3 = primaries_to_xyz((0.680, 0.320), (0.265, 0.690), (0.150, 0.060), D65_XY);
        let srgb = primaries_to_xyz((0.64, 0.33), (0.30, 0.60), (0.15, 0.06), D65_XY);
        for i in 0..3 {
            for j in 0..3 {
                assert!(approx(p3[i][j], DISPLAY_P3_TO_XYZ[i][j], 1e-4));
                assert!(approx(srgb[i][j], SRGB_TO_XYZ[i][j], 1e-4));
            }
        }
    }

    #[test]
    fn wide_gamut_spaces() {
        // Published Adobe RGB (1998) matrix.
        let a = adobe_rgb_to_xyz();
        let want = [
            [0.5767, 0.1856, 0.1882],
            [0.2973, 0.6274, 0.0753],
            [0.0270, 0.0707, 0.9911],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!(approx(a[i][j], want[i][j], 1e-3), "{a:?}");
            }
        }
        // ProPhoto white is D50 natively and D65 after adaptation.
        let w50 = xyz_to_xy(mul_vec(&prophoto_to_xyz_d50(), [1.0; 3]));
        assert!(approx(w50.0, D50_XY.0, 1e-4) && approx(w50.1, D50_XY.1, 1e-4));
        let w65 = xyz_to_xy(mul_vec(&prophoto_to_xyz(), [1.0; 3]));
        assert!(approx(w65.0, D65_XY.0, 1e-4) && approx(w65.1, D65_XY.1, 1e-4));
        // Bradford D65 → D50 matches the ICC-published matrix.
        let b = bradford(xy_to_xyz(D65_XY.0, D65_XY.1), xy_to_xyz(D50_XY.0, D50_XY.1));
        assert!(approx(b[0][0], 1.0479, 2e-3) && approx(b[0][1], 0.0229, 2e-3));
        assert!(approx(b[2][2], 0.7521, 2e-3));
    }

    #[test]
    fn cat16_maps_white_to_white() {
        let a = xy_to_xyz(0.44757, 0.40745);
        let d65 = xy_to_xyz(D65_XY.0, D65_XY.1);
        let m = cat16(a, d65);
        let w = mul_vec(&m, a);
        for k in 0..3 {
            assert!(approx(w[k], d65[k], 1e-9));
        }
    }
}
