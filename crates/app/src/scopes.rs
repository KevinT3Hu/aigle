//! Bounded, display-referred scopes. Spatial measurements use a 128×128 sample of
//! the rendered crop; they do not claim to detect pre-output clipping or RAW clipping.
use aigle_engine_api::Histogram;
use gpui::{
    Bounds, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, Pixels, Point, Render, Rgba, SharedString, Window, canvas, div, fill, point,
    prelude::*, px, rgb, rgba, size,
};
use gpui_component::button::Button;
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, Icon, Selectable, Sizable, h_flex, v_flex};
use std::{cell::Cell, rc::Rc, sync::Arc};

const LABELS: [&str; 7] = [
    "rgb",
    "luma",
    "channels",
    "cumulative",
    "waveform",
    "parade",
    "vectorscope",
];
const N: usize = 64;
// Small, embedded scope glyphs keep this toolbar independent of the optional icon bundle.
fn glyph(path: &str) -> Icon {
    Icon::default().data(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="{path}"/></svg>"#).as_bytes())
}
const SCOPE_GLYPHS: [&str; 7] = [
    "M3 20h18 M3 18l3-7 3 5 3-12 3 10 3-5 3 9",
    "M3 20h18 M4 18l4-2 4-12 4 12 4 2Z",
    "M3 20h18 M5 17V9 M8 17V5 M11 17v-5 M14 17V7 M17 17V4 M20 17v-6",
    "M3 3v18h18 M5 18h3l3-3 2-6 3-3h5",
    "M3 7h18 M3 17h18 M3 12l3-3 3 6 3-8 3 9 3-5 3 1",
    "M3 20h18 M4 15l2-7 2 4 M10 12l2-8 2 12 M16 16l2-6 2 3",
    "M12 3v18 M3 12h18 M5 5l14 14 M5 19L19 5 M8 3h8l5 9-5 9H8l-5-9Z",
];
fn t(key: &str) -> String {
    crate::i18n::t(&format!("panel.scope.{key}")).to_string()
}
fn pq(v: f32) -> f32 {
    let p = (v.max(0.0) / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p)).powf(2523.0 / 32.0)
}
fn nits(v: f32) -> f32 {
    let p = (v * pq(1000.0)).max(0.0).powf(32.0 / 2523.0);
    10000.0
        * ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p))
            .powf(16384.0 / 2610.0)
}
fn srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn rgb709(p: [f32; 4]) -> [f32; 3] {
    [
        1.660491 * p[0] - 0.587641 * p[1] - 0.072850 * p[2],
        -0.124550 * p[0] + 1.132_9 * p[1] - 0.008349 * p[2],
        -0.018151 * p[0] - 0.100579 * p[1] + 1.118_73 * p[2],
    ]
}
fn from709(p: [f32; 3]) -> [f32; 4] {
    [
        0.627404 * p[0] + 0.329283 * p[1] + 0.043313 * p[2],
        0.069097 * p[0] + 0.919540 * p[1] + 0.011362 * p[2],
        0.016391 * p[0] + 0.088013 * p[1] + 0.895595 * p[2],
        1.0,
    ]
}
fn graph_bounds(b: Bounds<Pixels>, plot: usize) -> Bounds<Pixels> {
    if plot != 6 {
        return b;
    }
    let side = b.size.width.min(b.size.height);
    Bounds::new(
        point(b.origin.x + (b.size.width - side) / 2.0, b.origin.y),
        size(side, side),
    )
}
fn level(v: f32, hdr: bool) -> f32 {
    if hdr {
        pq(v * 203.0) / pq(1000.0)
    } else {
        srgb(v)
    }
}
fn uv(p: [f32; 4], hdr: bool) -> [f32; 2] {
    let rgb = if hdr { [p[0], p[1], p[2]] } else { rgb709(p) };
    let [r, g, b] = rgb.map(|v| level(v, hdr));
    let (kr, kb) = if hdr {
        (0.2627, 0.0593)
    } else {
        (0.2126, 0.0722)
    };
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    [(b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr))]
}
#[derive(Clone)]
struct Data {
    bins: [Vec<u32>; 4],
    wave: [Vec<u32>; 4],
    vector: Vec<u32>,
    count: u64,
    low: u64,
    high: u64,
    gamut: u64,
    hdr: bool,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            bins: std::array::from_fn(|_| vec![0; 256]),
            wave: std::array::from_fn(|_| vec![0; N * N]),
            vector: vec![0; N * N],
            count: 0,
            low: 0,
            high: 0,
            gamut: 0,
            hdr: false,
        }
    }
}
fn analyze(h: &Histogram, roi: Option<[f32; 4]>, target: usize) -> Data {
    let gamut_matrix = {
        use aigle_core::color::*;
        let profile = match target {
            1 => DISPLAY_P3_TO_XYZ,
            2 => REC2020_TO_XYZ,
            _ => SRGB_TO_XYZ,
        };
        to_f32(&mul(
            &invert(&profile).expect("standard RGB matrix is invertible"),
            &REC2020_TO_XYZ,
        ))
    };
    let mut d = Data {
        hdr: h.scope.hdr,
        ..Default::default()
    };
    let [x0, y0, x1, y1] = roi.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let w = h.scope.width as usize;
    if w == 0 {
        d.bins = [
            h.red.clone(),
            h.green.clone(),
            h.blue.clone(),
            h.luma.clone(),
        ];
        return d;
    }
    for (i, &p) in h.scope.pixels.iter().enumerate() {
        let x = ((i % w) as f32 + 0.5) / w as f32;
        let y = ((i / w) as f32 + 0.5) / h.scope.height as f32;
        if p[3] <= 0.0 || x < x0 || x > x1 || y < y0 || y > y1 {
            continue;
        }
        d.count += 1;
        let rgb = if d.hdr { [p[0], p[1], p[2]] } else { rgb709(p) };
        let lum = 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
        let values = [
            level(rgb[0], d.hdr),
            level(rgb[1], d.hdr),
            level(rgb[2], d.hdr),
            level(lum, d.hdr),
        ];
        d.low += u64::from(rgb.iter().any(|&v| v <= 0.0));
        let ceiling = if d.hdr { 1000.0 / 203.0 } else { 1.0 };
        d.high += u64::from(
            rgb.iter()
                .any(|&v| v >= ceiling - if d.hdr { 0.002 } else { 0.00001 }),
        );
        let rgb_target: [f32; 3] = std::array::from_fn(|row| {
            gamut_matrix[row][0] * p[0] + gamut_matrix[row][1] * p[1] + gamut_matrix[row][2] * p[2]
        });
        // Test chromatic gamut independently of luminance/headroom.
        d.gamut += u64::from(rgb_target.iter().any(|&v| v < -0.0001));
        for (c, v) in values.into_iter().enumerate() {
            let v = v.clamp(0.0, 1.0);
            d.bins[c][(v * 255.0).round() as usize] += 1;
            d.wave[c][((1.0 - v) * (N - 1) as f32).round() as usize * N
                + (x * (N - 1) as f32).round() as usize] += 1;
        }
        let [u, v] = uv(p, d.hdr);
        let col = ((u + 0.5) * (N - 1) as f32)
            .round()
            .clamp(0.0, (N - 1) as f32) as usize;
        let row = ((0.5 - v) * (N - 1) as f32)
            .round()
            .clamp(0.0, (N - 1) as f32) as usize;
        d.vector[row * N + col] += 1;
    }
    // Preserve exact full-image distributions; only spatial/region plots are sampled.
    if roi.is_none() {
        d.bins = [
            h.red.clone(),
            h.green.clone(),
            h.blue.clone(),
            h.luma.clone(),
        ];
    }
    d
}

pub struct Scopes {
    histogram: Option<Arc<Histogram>>,
    data: Arc<Data>,
    plot: usize,
    log: bool,
    channels: [bool; 3],
    target: usize,
    region: bool,
    roi: Option<[f32; 4]>,
    anchor: Option<[f32; 2]>,
    hover: Option<[f32; 2]>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    sample_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl Scopes {
    pub fn new() -> Self {
        Self {
            histogram: None,
            data: Arc::new(Data::default()),
            plot: 0,
            log: false,
            channels: [true; 3],
            target: 0,
            region: false,
            roi: None,
            anchor: None,
            hover: None,
            bounds: Rc::new(Cell::new(None)),
            sample_bounds: Rc::new(Cell::new(None)),
        }
    }
    pub fn set_histogram(&mut self, h: Option<Arc<Histogram>>, cx: &mut Context<Self>) {
        // A new image can arrive with the same dimensions: clear a stale region on unload.
        if h.is_none() {
            self.roi = None;
            self.anchor = None;
        }
        self.histogram = h;
        self.rebuild();
        cx.notify();
    }
    fn rebuild(&mut self) {
        self.data = Arc::new(
            self.histogram
                .as_ref()
                .map(|h| analyze(h, self.roi, self.target))
                .unwrap_or_default(),
        );
    }
    fn unit(bounds: Bounds<Pixels>, p: Point<Pixels>) -> [f32; 2] {
        [
            ((p.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0),
            ((p.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0),
        ]
    }
    fn select(&mut self, p: Point<Pixels>) {
        if let (Some(a), Some(b)) = (self.anchor, self.sample_bounds.get()) {
            let q = Self::unit(b, p);
            let (w, h) = self
                .histogram
                .as_ref()
                .map(|h| (h.scope.width.max(1) as f32, h.scope.height.max(1) as f32))
                .unwrap_or((1.0, 1.0));
            self.roi = Some([
                ((a[0].min(q[0]) * w).floor() / w).min(1.0 - 1.0 / w),
                ((a[1].min(q[1]) * h).floor() / h).min(1.0 - 1.0 / h),
                (((a[0].max(q[0]) * w).floor() + 1.0) / w).min(1.0),
                (((a[1].max(q[1]) * h).floor() + 1.0) / h).min(1.0),
            ]);
            self.rebuild();
        }
    }
}
fn line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, color: Rgba) {
    let mut p = PathBuilder::stroke(px(1.0));
    p.move_to(a);
    p.line_to(b);
    if let Ok(p) = p.build() {
        window.paint_path(p, color);
    }
}
fn paint(
    d: &Data,
    plot: usize,
    log: bool,
    channels: [bool; 3],
    hover: Option<[f32; 2]>,
    b: Bounds<Pixels>,
    window: &mut Window,
) {
    window.paint_quad(fill(b, rgb(0x141414)));
    let b = graph_bounds(b, plot);
    let (x, y, w, h) = (b.origin.x, b.origin.y, b.size.width, b.size.height);
    let colors = [
        rgba(0xff5050c0),
        rgba(0x50ff50c0),
        rgba(0x5090ffc0),
        rgba(0xddddddd0),
    ];
    for f in [0.25, 0.5, 0.75] {
        line(
            window,
            point(x, y + h * f),
            point(x + w, y + h * f),
            rgba(0xffffff20),
        );
    }
    if plot >= 4 {
        let is_vector = plot == 6;
        let cellw = if plot == 5 {
            w / 3.0 / N as f32
        } else {
            w / N as f32
        };
        let cellh = h / N as f32;
        let indices: &[usize] = if is_vector || plot == 4 {
            &[3]
        } else {
            &[0, 1, 2]
        };
        for &c in indices {
            if c < 3 && !channels[c] {
                continue;
            }
            let bins = if is_vector { &d.vector } else { &d.wave[c] };
            let peak = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
            for (i, &n) in bins.iter().enumerate().filter(|(_, n)| **n > 0) {
                let intensity = if log {
                    (1.0 + n as f32).ln() / (1.0 + peak).ln()
                } else {
                    (n as f32 / peak).sqrt()
                };
                let mut color = if is_vector {
                    rgba(0xe0c080ff)
                } else {
                    colors[c]
                };
                color.a = intensity;
                let left = if plot == 5 {
                    x + w * (c as f32 / 3.0)
                } else {
                    x
                };
                window.paint_quad(fill(
                    Bounds::new(
                        point(left + cellw * (i % N) as f32, y + cellh * (i / N) as f32),
                        size(cellw, cellh),
                    ),
                    color,
                ));
            }
        }
        if is_vector {
            line(
                window,
                point(x + w / 2.0, y),
                point(x + w / 2.0, y + h),
                rgba(0xffffff40),
            );
            line(
                window,
                point(x, y + h / 2.0),
                point(x + w, y + h / 2.0),
                rgba(0xffffff40),
            );
            // Encoded RGB cube targets at the scope ceiling, connected as a
            // reference polygon. This is not a proof of chromatic gamut membership.
            let mut targets = Vec::new();
            for (p, color) in [
                ([1., 0., 0.], 0xff5050ff),
                ([1., 1., 0.], 0xffff50ff),
                ([0., 1., 0.], 0x50ff50ff),
                ([0., 1., 1.], 0x50ffffff),
                ([0., 0., 1.], 0x5090ffff),
                ([1., 0., 1.], 0xff50ffff),
            ] {
                let p = if d.hdr {
                    [
                        p[0] * 1000.0 / 203.0,
                        p[1] * 1000.0 / 203.0,
                        p[2] * 1000.0 / 203.0,
                        1.0,
                    ]
                } else {
                    from709(p)
                };
                let [u, v] = uv(p, d.hdr);
                let q = point(x + w * (u + 0.5), y + h * (0.5 - v));
                targets.push(q);
                window.paint_quad(fill(
                    Bounds::new(point(q.x - px(2.), q.y - px(2.)), size(px(4.), px(4.))),
                    rgba(color),
                ));
            }
            for i in 0..targets.len() {
                line(
                    window,
                    targets[i],
                    targets[(i + 1) % targets.len()],
                    rgba(0xffffff40),
                );
            }
            // Conventional +I skin hue reference: 123 degrees from +Cb.
            let angle = 123.0_f32.to_radians();
            line(
                window,
                point(x + w / 2.0, y + h / 2.0),
                point(
                    x + w * (0.5 + 0.48 * angle.cos()),
                    y + h * (0.5 - 0.48 * angle.sin()),
                ),
                rgba(0xffc090a0),
            );
        }
    } else {
        let indices: &[usize] = if plot == 1 || plot == 3 {
            &[3]
        } else {
            &[0, 1, 2]
        };
        let peak = indices
            .iter()
            .filter(|&&c| c == 3 || channels[c])
            .flat_map(|&c| d.bins[c].iter().skip(1).take(254))
            .copied()
            .max()
            .unwrap_or(1)
            .max(1) as f64;
        for &c in indices {
            if c < 3 && !channels[c] {
                continue;
            }
            let bins = &d.bins[c];
            let total = bins.iter().map(|&v| u64::from(v)).sum::<u64>().max(1) as f64;
            let cw = if plot == 2 { w / 3.0 } else { w };
            let left = if plot == 2 { x + cw * c as f32 } else { x };
            let mut path = PathBuilder::fill();
            path.move_to(point(left, y + h));
            let mut sum = 0u64;
            for (i, &v) in bins.iter().enumerate() {
                sum += u64::from(v);
                let f = if plot == 3 {
                    sum as f64 / total
                } else if log {
                    (1.0 + v as f64).ln() / (1.0 + peak).ln()
                } else {
                    v as f64 / peak
                };
                path.line_to(point(
                    left + cw * i as f32 / 255.0,
                    y + h * (1.0 - f.min(1.0) as f32),
                ));
            }
            path.line_to(point(left + cw, y + h));
            path.close();
            if let Ok(p) = path.build() {
                window.paint_path(p, colors[c]);
            }
        }
    }
    if d.hdr && plot != 6 {
        let white = pq(203.0) / pq(1000.0);
        if plot >= 4 {
            line(
                window,
                point(x, y + h * (1.0 - white)),
                point(x + w, y + h * (1.0 - white)),
                rgba(0xffce60a0),
            );
        } else {
            let count = if plot == 2 { 3 } else { 1 };
            for c in 0..count {
                let xx = x + w * (c as f32 + white) / count as f32;
                line(window, point(xx, y), point(xx, y + h), rgba(0xffce60a0));
            }
        }
    }
    if let Some([u, v]) = hover {
        line(
            window,
            point(x + w * u, y),
            point(x + w * u, y + h),
            rgba(0xffffff80),
        );
        line(
            window,
            point(x, y + h * v),
            point(x + w, y + h * v),
            rgba(0xffffff80),
        );
    }
}
impl Render for Scopes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let d = self.data.clone();
        let plot = self.plot;
        let log = self.log;
        let channels = self.channels;
        let hover = self.hover;
        let bounds = self.bounds.clone();
        let graph = div()
            .id("scope-graph")
            .w_full()
            .h(px(140.0))
            .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, _, cx| {
                if let Some(b) = s.bounds.get() {
                    s.hover = Some(Self::unit(b, e.position));
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|s, on: &bool, _, cx| {
                if !on {
                    s.hover = None;
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |b, _, _| {
                        bounds.set(Some(graph_bounds(b, plot)));
                    },
                    move |b, _, w, _| paint(&d, plot, log, channels, hover, b, w),
                )
                .size_full(),
            );
        let selectors = h_flex()
            .gap_1()
            .children(LABELS.iter().enumerate().map(|(i, name)| {
                Button::new(SharedString::from(format!("scope-type-{i}")))
                    .icon(glyph(SCOPE_GLYPHS[i]))
                    .xsmall()
                    .selected(self.plot == i)
                    .accessibility_label(t(name))
                    .tooltip(format!("{} — {}", t(name), t(&format!("{name}_hint"))))
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.plot = i;
                        cx.notify();
                    }))
            }));
        let controls = h_flex()
            .gap_1()
            .child(
                Button::new("scope-log")
                    .icon(glyph("M3 3v18h18 M5 17c2-9 7-11 15-12"))
                    .accessibility_label(t("log"))
                    .tooltip(t("log_hint"))
                    .xsmall()
                    .selected(self.log)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.log = !s.log;
                        cx.notify();
                    })),
            )
            .children(["R", "G", "B"].into_iter().enumerate().map(|(i, label)| {
                Button::new(SharedString::from(format!("scope-channel-{i}")))
                    .label(label)
                    .tooltip(t(["red_hint", "green_hint", "blue_hint"][i]))
                    .xsmall()
                    .selected(self.channels[i])
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.channels[i] = !s.channels[i];
                        cx.notify();
                    }))
            }))
            .child(
                Button::new("scope-region")
                    .icon(glyph(
                        "M4 8V4h4 M16 4h4v4 M20 16v4h-4 M8 20H4v-4 M9 9h6v6H9Z",
                    ))
                    .accessibility_label(t("region"))
                    .tooltip(t("region_hint"))
                    .xsmall()
                    .selected(self.region)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.region = !s.region;
                        if !s.region {
                            s.roi = None;
                            s.anchor = None;
                            s.rebuild();
                        }
                        cx.notify();
                    })),
            );
        let guide = if self.plot == 6 {
            t("vector_guide")
        } else if self.data.hdr {
            t("hdr_scale")
        } else {
            t("sdr_scale")
        };
        let controls = controls.child(div().flex_1()).child(
            Button::new("scope-help")
                .icon(glyph(
                    "M12 8h.01 M11 11h1v6 M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20",
                ))
                .xsmall()
                .accessibility_label(t("help"))
                .tooltip(format!("{}\n{}", guide, t("sample_note"))),
        );
        let percent = |n: u64| 100.0 * n as f64 / self.data.count.max(1) as f64;
        let stats = format!(
            "{} {:.1}% · {} {:.1}% · {} {:.1}%",
            t("black"),
            percent(self.data.low),
            t("peak"),
            percent(self.data.high),
            t("gamut"),
            percent(self.data.gamut)
        );
        let gamut = h_flex()
            .gap_1()
            .child(div().text_xs().child(t("target")))
            .children(
                ["sRGB", "P3", "Rec.2020"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, label)| {
                        Button::new(SharedString::from(format!("scope-gamut-{i}")))
                            .label(label)
                            .tooltip(format!("{}: {label}", t("target_hint")))
                            .xsmall()
                            .selected(self.target == i)
                            .on_click(cx.listener(move |s, _, _, cx| {
                                s.target = i;
                                s.rebuild();
                                cx.notify();
                            }))
                    }),
            );
        let mut readout = if self.plot == 6 {
            "Cb / Cr".to_string()
        } else if self.data.hdr {
            "0–1000 nit".to_string()
        } else {
            "0–100%".to_string()
        };
        if let Some([x, y]) = self.hover {
            if self.plot == 6 {
                readout = format!("Cb {:+.3} · Cr {:+.3}", x - 0.5, 0.5 - y);
            } else {
                let v = if self.plot >= 4 {
                    1.0 - y
                } else if self.plot == 2 {
                    (x * 3.0).fract()
                } else {
                    x
                };
                let val = if self.data.hdr {
                    format!("{:.1} nit", nits(v))
                } else {
                    format!("{:.1}%", v * 100.0)
                };
                readout = if self.plot >= 4 {
                    format!(
                        "x {:.1}% · {val}",
                        if self.plot == 5 {
                            (x * 3.0).fract() * 100.0
                        } else {
                            x * 100.0
                        }
                    )
                } else {
                    let i = (v * 255.0).round().clamp(0.0, 255.0) as usize;
                    if self.plot == 3 {
                        let total = self.data.bins[3]
                            .iter()
                            .map(|&n| n as u64)
                            .sum::<u64>()
                            .max(1);
                        let sum = self.data.bins[3][..=i]
                            .iter()
                            .map(|&n| n as u64)
                            .sum::<u64>();
                        format!("{val} · ≤ {:.2}%", sum as f64 / total as f64 * 100.0)
                    } else {
                        format!(
                            "{val} · R {} G {} B {} Y {}",
                            self.data.bins[0][i],
                            self.data.bins[1][i],
                            self.data.bins[2][i],
                            self.data.bins[3][i]
                        )
                    }
                };
            }
        }
        let mut body = v_flex()
            .gap_1()
            .child(div().text_xs().child(t(LABELS[self.plot])))
            .child(graph)
            .child(selectors)
            .child(controls)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(readout),
            )
            .child(gamut)
            .child(
                div()
                    .id("scope-statistics")
                    .text_xs()
                    .child(stats)
                    .tooltip(|window, cx| Tooltip::new(t("sample_note")).build(window, cx)),
            );

        if self.region {
            let h = self.histogram.clone();
            let slot = self.sample_bounds.clone();
            let roi = self.roi;
            let thumbnail = div()
                .id("scope-region-image")
                .w_full()
                .h(px(100.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|s, e: &MouseDownEvent, _, cx| {
                        if let Some(b) = s.sample_bounds.get() {
                            s.anchor = Some(Self::unit(b, e.position));
                            s.select(e.position);
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_move(cx.listener(|s, e: &MouseMoveEvent, _, cx| {
                    if e.pressed_button == Some(MouseButton::Left) && s.anchor.is_some() {
                        s.select(e.position);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|s, e: &MouseUpEvent, _, cx| {
                        s.select(e.position);
                        s.anchor = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|s, e: &MouseUpEvent, _, cx| {
                        s.select(e.position);
                        s.anchor = None;
                        cx.notify();
                    }),
                )
                .child(
                    canvas(
                        move |b, _, _| {
                            slot.set(Some(b));
                        },
                        move |b, _, window, _| {
                            window.paint_quad(fill(b, rgb(0x141414)));
                            let Some(h) = h else { return };
                            let s = &h.scope;
                            if s.width == 0 || s.height == 0 {
                                return;
                            }
                            let step = (s.width.max(s.height) / 32).max(1) as usize;
                            let cw = b.size.width / s.width as f32;
                            let ch = b.size.height / s.height as f32;
                            for (i, &p) in s.pixels.iter().enumerate() {
                                if !(i % s.width as usize).is_multiple_of(step)
                                    || !(i / s.width as usize).is_multiple_of(step)
                                    || p[3] <= 0.0
                                {
                                    continue;
                                }
                                let rgb = rgb709(p).map(|v| {
                                    let v = if s.hdr {
                                        v.max(0.0) / (1.0 + v.max(0.0))
                                    } else {
                                        v
                                    };
                                    (srgb(v).clamp(0.0, 1.0) * 255.0).round() as u32
                                });
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            b.origin.x + cw * (i % s.width as usize) as f32,
                                            b.origin.y + ch * (i / s.width as usize) as f32,
                                        ),
                                        size(cw * step as f32, ch * step as f32),
                                    ),
                                    gpui::rgb(rgb[0] << 16 | rgb[1] << 8 | rgb[2]),
                                ));
                            }
                            if let Some([x0, y0, x1, y1]) = roi {
                                let a = point(
                                    b.origin.x + b.size.width * x0,
                                    b.origin.y + b.size.height * y0,
                                );
                                let z = point(
                                    b.origin.x + b.size.width * x1,
                                    b.origin.y + b.size.height * y1,
                                );
                                for (p, q) in [
                                    (a, point(z.x, a.y)),
                                    (point(z.x, a.y), z),
                                    (z, point(a.x, z.y)),
                                    (point(a.x, z.y), a),
                                ] {
                                    line(window, p, q, rgba(0xffd060ff));
                                }
                            }
                        },
                    )
                    .size_full(),
                );
            body = body.child(thumbnail).child(
                Button::new("scope-whole")
                    .icon(glyph(
                        "M4 8V4h4 M16 4h4v4 M20 16v4h-4 M8 20H4v-4 M8 12h8 M12 8v8",
                    ))
                    .accessibility_label(t("whole"))
                    .tooltip(t("whole"))
                    .xsmall()
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.roi = None;
                        s.anchor = None;
                        s.rebuild();
                        cx.notify();
                    })),
            );
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Histogram {
        Histogram {
            red: vec![0; 256],
            green: vec![0; 256],
            blue: vec![0; 256],
            luma: vec![0; 256],
            scope: aigle_engine_api::ScopeSamples {
                width: 2,
                height: 2,
                hdr: true,
                pixels: vec![
                    [0., 0., 0., 1.],
                    [1., 1., 1., 1.],
                    [5., 5., 5., 1.],
                    [1., 0., 0., 0.],
                ],
            },
        }
    }
    #[test]
    fn region_excludes_transparent_pixels_and_counts_highlights() {
        let h = fixture();
        let d = analyze(&h, Some([0., 0., 1., 1.]), 0);
        assert_eq!((d.count, d.low, d.high), (3, 1, 1));
        assert_eq!(d.wave[3].iter().sum::<u32>(), 3);
        assert_eq!(d.vector.iter().sum::<u32>(), 3);
        let d = analyze(&h, Some([0.5, 0., 1., 1.]), 0);
        assert_eq!((d.count, d.low, d.high), (1, 0, 0));
    }
    #[test]
    fn pq_guides_and_neutral_vectors() {
        assert!((nits(pq(203.) / pq(1000.)) - 203.).abs() < 0.1);
        assert!((nits(1.) - 1000.).abs() < 0.1);
        let [u, v] = uv([1., 1., 1., 1.], true);
        assert!(u.abs() < 1e-6 && v.abs() < 1e-6);
        let d = analyze(&fixture(), Some([0., 0., 1., 1.]), 2);
        assert_eq!(d.gamut, 0);
    }
    #[test]
    fn target_gamut_and_spatial_channel_locations() {
        let mut h = fixture();
        h.scope.width = 1;
        h.scope.height = 1;
        h.scope.pixels = vec![[1.0, 0.0, 0.0, 1.0]];
        let small = analyze(&h, Some([0., 0., 1., 1.]), 0);
        let wide = analyze(&h, Some([0., 0., 1., 1.]), 2);
        assert_eq!(small.gamut, 1);
        assert_eq!(wide.gamut, 0);
        assert_eq!(wide.bins[0].iter().sum::<u32>(), 1);
        assert_eq!(wide.bins[1][0], 1);
        assert!(
            wide.wave[0]
                .iter()
                .enumerate()
                .any(|(i, &n)| n == 1 && i % N == 32)
        );
        let [u, v] = uv(from709([1., 0., 0.]), false);
        assert!(u < 0.0 && (v - 0.5).abs() < 0.0001);
    }

    #[test]
    #[ignore = "manual scope analysis benchmark"]
    fn scope_analysis_benchmark() {
        let mut h = fixture();
        h.scope.width = 128;
        h.scope.height = 128;
        h.scope.pixels = (0..128 * 128)
            .map(|i| [(i % 128) as f32 / 32.0, (i / 128) as f32 / 32.0, 0.2, 1.0])
            .collect();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(analyze(std::hint::black_box(&h), None, 0));
        }
        eprintln!(
            "128x128 HDR scope analysis: {:.3} ms/update",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}
