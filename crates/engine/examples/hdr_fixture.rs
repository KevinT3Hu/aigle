//! Reproducible synthetic HDR photograph/test scene with absolute luminance patches.
//! `cargo run --release -p aigle-engine --example hdr_fixture -- /tmp/aigle-hdr`
use aigle_engine_api::{
    BackendPreference, DisplayProfileId, EncodedFormat, EngineConfig, ExportOptions, SourceRef,
};
use exr::prelude::*;
use rayon::prelude::*;
use std::{
    ops::{Add, Mul, Neg, Sub},
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone, Copy)]
struct V(f32, f32, f32);
impl Add for V {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        V(self.0 + b.0, self.1 + b.1, self.2 + b.2)
    }
}
impl Sub for V {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        self + (-b)
    }
}
impl Neg for V {
    type Output = Self;
    fn neg(self) -> Self {
        self * (-1.0)
    }
}
impl Mul<f32> for V {
    type Output = Self;
    fn mul(self, b: f32) -> Self {
        V(self.0 * b, self.1 * b, self.2 * b)
    }
}
impl Mul for V {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        V(self.0 * b.0, self.1 * b.1, self.2 * b.2)
    }
}
impl V {
    fn dot(self, b: Self) -> f32 {
        self.0 * b.0 + self.1 * b.1 + self.2 * b.2
    }
    fn norm(self) -> Self {
        self * (1.0 / self.dot(self).sqrt())
    }
    fn reflect(self, n: Self) -> Self {
        self - n * (2.0 * self.dot(n))
    }
    fn rgb(self) -> [f32; 3] {
        [self.0, self.1, self.2]
    }
}
struct Hit {
    t: f32,
    normal: V,
    color: V,
    metal: f32,
}
fn sphere(o: V, d: V, c: V, r: f32, color: V, metal: f32) -> Option<Hit> {
    let v = o - c;
    let b = v.dot(d);
    let disc = b * b - v.dot(v) + r * r;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t > 0.001).then(|| Hit {
        t,
        normal: (o + d * t - c) * (1.0 / r),
        color,
        metal,
    })
}
fn hit(o: V, d: V) -> Option<Hit> {
    let mut closest = None;
    let mut best = 1e6;
    for (c, r, color, metal) in [
        (V(0.15, 0.8, -0.4), 0.8, V(0.95, 0.67, 0.25), 0.92),
        (V(1.55, 0.52, 0.45), 0.52, V(0.7, 0.018, 0.01), 0.15),
        (V(-1.25, 0.43, 0.8), 0.43, V(0.92, 0.94, 0.98), 0.98),
    ] {
        if let Some(h) = sphere(o, d, c, r, color, metal)
            && h.t < best
        {
            best = h.t;
            closest = Some(h);
        }
    }
    // Floor, rear wall and side walls. A small open studio rather than an LDR skybox.
    for (axis, p, n, color) in [
        (1, 0.0, V(0.0, 1.0, 0.0), V(0.29, 0.13, 0.055)),
        (2, -3.0, V(0.0, 0.0, 1.0), V(0.42, 0.44, 0.47)),
        (0, -3.0, V(1.0, 0.0, 0.0), V(0.28, 0.34, 0.40)),
        (0, 3.0, V(-1.0, 0.0, 0.0), V(0.43, 0.36, 0.27)),
    ] {
        let a = o.rgb()[axis];
        let b = d.rgb()[axis];
        if b.abs() < 1e-5 {
            continue;
        }
        let t = (p - a) / b;
        if t <= 0.001 || t >= best {
            continue;
        }
        let pos = o + d * t;
        if pos.1 > 3.2 || pos.2 > 4.0 {
            continue;
        }
        let color = if axis == 1 {
            let grain = (pos.0 * 24.0 + (pos.2 * 3.0).sin() * 0.8).sin() * 0.04;
            let seam = if (pos.0 * 1.7).fract().abs() < 0.018 {
                0.65
            } else {
                1.0
            };
            color * ((1.0 + grain) * seam)
        } else {
            color
        };
        best = t;
        closest = Some(Hit {
            t,
            normal: n,
            color,
            metal: if axis == 1 { 0.04 } else { 0.0 },
        });
    }
    closest
}
fn window(p: V) -> Option<V> {
    if p.2 < -2.99 && (-2.55..-0.4).contains(&p.0) && (0.75..2.9).contains(&p.1) {
        let u = (p.0 + 2.55) / 2.15;
        let v = (p.1 - 0.75) / 2.15;
        let frame = !(0.025..=0.975).contains(&u)
            || !(0.025..=0.975).contains(&v)
            || (u - 0.5).abs() < 0.012
            || (v - 0.45).abs() < 0.012;
        if frame {
            return Some(V(0.07, 0.075, 0.08));
        }
        let sun = (-(u - 0.75).powi(2) * 500.0 - (v - 0.75).powi(2) * 500.0).exp();
        let skyline = 0.15 + (u * 53.0).sin().abs() * 0.09;
        if v < skyline {
            return Some(V(0.25, 0.35, 0.4) * (0.6 + u));
        }
        return Some(V(2.4 + sun * 16.0, 3.7 + sun * 15.0, 5.8 + sun * 12.0) * (0.7 + 0.3 * v));
    }
    None
}
fn ray(o: V, d: V, depth: u32) -> V {
    let Some(h) = hit(o, d) else {
        return V(0.13, 0.18, 0.25);
    };
    let p = o + d * h.t;
    if let Some(emission) = window(p) {
        return emission;
    }
    let mut lit = V(0.035, 0.04, 0.055);
    for (light, color, strength) in [
        (V(-1.5, 2.2, -2.6), V(0.75, 0.86, 1.0), 28.0),
        (V(2.2, 3.0, 3.0), V(1.0, 0.79, 0.53), 9.0),
    ] {
        let l = light - p;
        let dist = l.dot(l);
        let direction = l.norm();
        let shadow =
            hit(p + h.normal * 0.003, direction).is_some_and(|ob| ob.t * ob.t < dist - 0.1);
        if !shadow {
            let cosine = h.normal.dot(direction).max(0.0);
            lit = lit + color * (strength * cosine / dist);
        }
    }
    let diffuse = h.color * lit;
    if h.metal > 0.0 && depth < 4 {
        let reflected = ray(p + h.normal * 0.003, d.reflect(h.normal).norm(), depth + 1);
        diffuse * (1.0 - h.metal) + reflected * h.color * h.metal
    } else {
        diffuse
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("aigle-hdr"));
    std::fs::create_dir_all(&dir)?;
    let (w, h) = (1536usize, 960usize);
    let mut rgb = vec![[0.0f32; 3]; w * h];
    rgb.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let mut acc = V(0.0, 0.0, 0.0);
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let dx = ((x as f32 + sx) / w as f32 * 2.0 - 1.0) * (w as f32 / h as f32) * 0.43;
                let dy = (1.0 - (y as f32 + sy) / h as f32 * 2.0) * 0.43;
                acc = acc + ray(V(0.0, 1.45, 5.2), V(dx, dy - 0.06, -1.0).norm(), 0);
            }
            *p = (acc * 0.25).rgb();
        }
    });
    let patches = [0.0f32, 0.1, 1.0, 10.0, 80.0, 203.0, 1000.0, 4000.0];
    for (i, nits) in patches.into_iter().enumerate() {
        let (x0, x1) = (i * w / 8, (i + 1) * w / 8);
        for y in h - 90..h {
            for x in x0..x1 {
                rgb[y * w + x] = if y < h - 33 {
                    [nits / aigle_engine::hdr::REFERENCE_WHITE; 3]
                } else {
                    [0.02; 3]
                };
            }
        }
        let (tw, th, mask) = aigle_vips_sys::text_mask(&format!("{nits} nits"), "sans", 20)?;
        for y in 0..th as usize {
            for x in 0..tw as usize {
                let (ox, oy) = (x0 + 8 + x, h - 29 + y);
                if ox < x1 && oy < h {
                    let a = mask[y * tw as usize + x] as f32 / 255.0;
                    rgb[oy * w + ox] = [0.02 + 0.98 * a; 3];
                }
            }
        }
    }
    let max =
        rgb.iter().flatten().copied().fold(0.0f32, f32::max) * aigle_engine::hdr::REFERENCE_WHITE;
    let hdr_pixels = rgb.iter().filter(|p| p.iter().any(|v| *v > 1.0)).count();
    let mut attrs = LayerAttributes::named("Synthetic HDR studio");
    attrs.white_luminance = Some(203.0);
    attrs.comments = Some(Text::from(
        "Synthetic ray-traced scene. Patch labels are scene-input luminance; edit rendering applies the HDR shoulder.",
    ));
    let layer = Layer::new(
        (w, h),
        attrs,
        Encoding::SMALL_FAST_LOSSLESS,
        SpecificChannels::rgb(|p: Vec2<usize>| {
            let p = rgb[p.y() * w + p.x()];
            (p[0], p[1], p[2])
        }),
    );
    let mut image = Image::from_layer(layer);
    image.attributes.chromaticities = Some(exr::meta::attribute::Chromaticities {
        red: Vec2(0.64, 0.33),
        green: Vec2(0.30, 0.60),
        blue: Vec2(0.15, 0.06),
        white: Vec2(0.3127, 0.3290),
    });
    let path = dir.join("studio-scene.exr");
    image.write().to_file(&path)?;
    drop(image);
    drop(rgb);
    let engine = aigle_engine::create(EngineConfig {
        backend: BackendPreference::Cpu,
        ..Default::default()
    });
    for (format, name, output) in [
        (
            EncodedFormat::HdrPng,
            "studio-hdr.png",
            DisplayProfileId::HdrRec2020,
        ),
        (
            EncodedFormat::Jpeg { quality: 95 },
            "studio-sdr.jpg",
            DisplayProfileId::Srgb,
        ),
    ] {
        engine
            .export(
                SourceRef::File(path.clone()),
                Arc::new(Default::default()),
                ExportOptions {
                    path: dir.join(name),
                    format,
                    output,
                    ..Default::default()
                },
            )
            .wait()
            .map_err(|e| e.to_string())?;
    }
    println!(
        "{}: {w}x{h}, max component {max:.1} nits, {:.1}% of pixels above diffuse white",
        path.display(),
        hdr_pixels as f64 / (w * h) as f64 * 100.0
    );
    println!(
        "HDR PNG: {}\nSDR reference: {}",
        dir.join("studio-hdr.png").display(),
        dir.join("studio-sdr.jpg").display()
    );
    Ok(())
}
