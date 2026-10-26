//! Human-readable photo metadata for the info tab and the canvas overlay.

use aigle_engine_api::{CameraInfo, ImageInfo, SourceKind};

/// Facts about the file on disk.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFacts {
    pub name: String,
    pub path: Option<String>,
    pub bytes: Option<u64>,
}

pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(&'static str, String)>,
}

fn trim_num(v: f32, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// `1/250 s`, `0.5 s`, `2 s`, `30 s`.
pub fn shutter(t: f32) -> String {
    if t >= 0.3 {
        format!("{} s", trim_num(t, 1))
    } else {
        format!("1/{} s", (1.0 / t).round() as u32)
    }
}

pub fn aperture(f: f32) -> String {
    format!("f/{}", trim_num(f, 1))
}

pub fn focal(mm: f32) -> String {
    format!("{} mm", trim_num(mm, 1))
}

/// `+0.7 EV`, `-1 EV`, `0 EV`.
pub fn bias(ev: f32) -> String {
    let v = (ev * 10.0).round() / 10.0;
    if v == 0.0 {
        "0 EV".into()
    } else {
        format!("{}{} EV", if v > 0.0 { "+" } else { "" }, trim_num(v, 1))
    }
}

/// `Nikon D3200`, without repeating the make when the model already starts with it.
pub fn camera_name(c: &CameraInfo) -> String {
    let (make, model) = (c.make.trim(), c.model.trim());
    if make.is_empty() || model.to_lowercase().starts_with(&make.to_lowercase()) {
        model.to_string()
    } else if model.is_empty() {
        make.to_string()
    } else {
        format!("{make} {model}")
    }
}

pub fn lens_name(c: &CameraInfo) -> Option<String> {
    if let Some(m) = c.lens_model.as_deref().filter(|m| !m.trim().is_empty()) {
        return Some(m.trim().to_string());
    }
    let (a, b) = c.lens_focal_range?;
    if a <= 0.0 {
        return None;
    }
    Some(if (b - a).abs() < 0.05 {
        focal(a)
    } else {
        format!("{}–{} mm", trim_num(a, 1), trim_num(b, 1))
    })
}

fn exposure_program(p: u16) -> Option<&'static str> {
    Some(match p {
        1 => "手动",
        2 => "程序自动",
        3 => "光圈优先",
        4 => "快门优先",
        5 => "创意程序",
        6 => "运动程序",
        7 => "人像",
        8 => "风景",
        9 => "B 门",
        _ => return None,
    })
}

fn metering_mode(m: u16) -> Option<&'static str> {
    Some(match m {
        1 => "平均",
        2 => "中央重点",
        3 => "点测光",
        4 => "多点",
        5 => "评价 / 矩阵",
        6 => "局部",
        _ => return None,
    })
}

fn exposure_mode(m: u16) -> Option<&'static str> {
    Some(match m {
        0 => "自动曝光",
        1 => "手动曝光",
        2 => "自动包围",
        _ => return None,
    })
}

fn white_balance(m: u16) -> Option<&'static str> {
    Some(match m {
        0 => "自动",
        1 => "手动",
        _ => return None,
    })
}

/// EXIF `Flash`: bit 0 = fired; 0x20 = no flash function.
fn flash(f: u16) -> &'static str {
    if f & 0x20 != 0 {
        "无闪光功能"
    } else if f & 1 != 0 {
        "已闪光"
    } else {
        "未闪光"
    }
}

/// `2016:11:30 13:10:53` → `2016-11-30 13:10:53`, with the UTC offset if known.
pub fn date_time(exif: &str, offset: Option<&str>) -> String {
    let mut s = exif.to_string();
    if s.len() >= 10 && s.is_char_boundary(10) {
        s.replace_range(..10, &s[..10].replace(':', "-"));
    }
    match offset {
        Some(o) => format!("{s}（UTC{o}）"),
        None => s,
    }
}

fn coord(v: f64, pos: char, neg: char) -> String {
    format!("{:.5}° {}", v.abs(), if v >= 0.0 { pos } else { neg })
}

fn bytes(n: u64) -> String {
    let mib = n as f64 / (1024.0 * 1024.0);
    if mib >= 1.0 {
        format!("{mib:.1} MB")
    } else {
        format!("{:.0} KB", n as f64 / 1024.0)
    }
}

/// The exposure line of the overlay: `50 mm · f/2.8 · 1/250 s · ISO 800 · +0.7 EV`.
pub fn exposure_line(c: &CameraInfo) -> String {
    let mut parts = Vec::new();
    if let Some(f) = c.focal_length {
        parts.push(focal(f));
    }
    if let Some(a) = c.f_number {
        parts.push(aperture(a));
    }
    if let Some(t) = c.exposure_time {
        parts.push(shutter(t));
    }
    if let Some(i) = c.iso {
        parts.push(format!("ISO {}", i.round() as u32));
    }
    if let Some(b) = c.shot.exposure_bias.filter(|b| b.abs() >= 0.05) {
        parts.push(bias(b));
    }
    parts.join(" · ")
}

/// Overlay lines (camera · lens, exposure), empty when nothing is known.
pub fn overlay_lines(info: &ImageInfo) -> Vec<String> {
    let Some(c) = &info.camera else {
        return Vec::new();
    };
    let head = [Some(camera_name(c)), lens_name(c)]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    [head, exposure_line(c)]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
}

/// The info tab's sections; rows without data are left out.
pub fn sections(info: &ImageInfo, file: &FileFacts) -> Vec<Section> {
    let mut out = Vec::new();

    let mut rows = vec![("文件名", file.name.clone())];
    if let Some(p) = &file.path {
        rows.push(("位置", p.clone()));
    }
    if let Some(b) = file.bytes {
        rows.push(("文件大小", bytes(b)));
    }
    let (w, h) = (info.source_size.w, info.source_size.h);
    rows.push((
        "尺寸",
        format!("{w} × {h}（{:.1} MP）", w as f64 * h as f64 / 1e6),
    ));
    rows.push((
        "类型",
        match info.kind {
            SourceKind::Raw => "RAW",
            SourceKind::Rendered => "已渲染图像",
            SourceKind::Synthetic => "合成测试图",
        }
        .into(),
    ));
    out.push(Section {
        title: "文件",
        rows,
    });

    let Some(c) = &info.camera else {
        return out;
    };
    let s = &c.shot;
    let opt = |v: Option<String>| v.filter(|s| !s.is_empty());

    let mut rows = Vec::new();
    rows.push(("机身", camera_name(c)));
    if let Some(v) = opt(s.body_serial.clone()) {
        rows.push(("机身序列号", v));
    }
    if let Some(v) = opt(s.firmware.clone()) {
        rows.push(("固件", v));
    }
    if let Some(v) = &s.date_time {
        rows.push(("拍摄时间", date_time(v, s.utc_offset.as_deref())));
    }
    out.push(Section {
        title: "相机",
        rows,
    });

    let mut rows = Vec::new();
    if let Some(v) = lens_name(c) {
        rows.push(("镜头", v));
    }
    if let Some(v) = opt(c.lens_make.clone()) {
        rows.push(("镜头厂商", v));
    }
    if let Some(v) = opt(s.lens_serial.clone()) {
        rows.push(("镜头序列号", v));
    }
    if let Some(f) = c.focal_length {
        let mut v = focal(f);
        if let Some(e) = c.focal_length_35mm.filter(|e| (e - f).abs() >= 0.5) {
            v.push_str(&format!("（等效 {}）", focal(e)));
        }
        rows.push(("焦距", v));
    }
    if let Some(a) = s.max_aperture {
        rows.push(("最大光圈", aperture(a)));
    }
    if !rows.is_empty() {
        out.push(Section {
            title: "镜头",
            rows,
        });
    }

    let mut rows = Vec::new();
    if let Some(t) = c.exposure_time {
        rows.push(("快门", shutter(t)));
    }
    if let Some(a) = c.f_number {
        rows.push(("光圈", aperture(a)));
    }
    if let Some(i) = c.iso {
        rows.push(("ISO", format!("{}", i.round() as u32)));
    }
    if let Some(b) = s.exposure_bias {
        rows.push(("曝光补偿", bias(b)));
    }
    if let Some(v) = s.exposure_program.and_then(exposure_program) {
        rows.push(("曝光程序", v.into()));
    }
    if let Some(v) = s.exposure_mode.and_then(exposure_mode) {
        rows.push(("曝光模式", v.into()));
    }
    if let Some(v) = s.metering_mode.and_then(metering_mode) {
        rows.push(("测光", v.into()));
    }
    if let Some(f) = s.flash {
        rows.push(("闪光灯", flash(f).into()));
    }
    if let Some(v) = s.white_balance.and_then(white_balance) {
        rows.push(("机内白平衡", v.into()));
    }
    rows.push((
        "原照白平衡",
        format!(
            "{} K，色调 {}",
            info.as_shot.temperature.round() as i32,
            info.as_shot.tint.round() as i32
        ),
    ));
    out.push(Section {
        title: "拍摄",
        rows,
    });

    let mut rows = Vec::new();
    if let Some(v) = opt(s.artist.clone()) {
        rows.push(("作者", v));
    }
    if let Some(v) = opt(s.copyright.clone()) {
        rows.push(("版权", v));
    }
    if !rows.is_empty() {
        out.push(Section {
            title: "版权",
            rows,
        });
    }

    if let Some(g) = s.gps {
        let mut rows = vec![
            ("纬度", coord(g.latitude, 'N', 'S')),
            ("经度", coord(g.longitude, 'E', 'W')),
        ];
        if let Some(a) = g.altitude {
            rows.push(("海拔", format!("{} m", a.round() as i32)));
        }
        out.push(Section {
            title: "位置",
            rows,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::geometry::Size;
    use aigle_engine_api::{GpsPosition, ShotInfo, WbValuesDto};

    fn camera() -> CameraInfo {
        CameraInfo {
            make: "Canon".into(),
            model: "EOS 6D".into(),
            iso: Some(800.0),
            exposure_time: Some(1.0 / 250.0),
            f_number: Some(5.6),
            focal_length: Some(50.0),
            focal_length_35mm: Some(75.0),
            lens_make: None,
            lens_model: Some("EF50mm f/1.8 STM".into()),
            lens_focal_range: Some((50.0, 50.0)),
            shot: ShotInfo {
                date_time: Some("2017:01:08 11:18:13".into()),
                utc_offset: Some("+01:00".into()),
                exposure_bias: Some(2.0 / 3.0),
                exposure_program: Some(3),
                flash: Some(16),
                gps: Some(GpsPosition {
                    latitude: 34.0596,
                    longitude: -84.6817,
                    altitude: Some(261.0),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn values_format() {
        assert_eq!(shutter(1.0 / 250.0), "1/250 s");
        assert_eq!(shutter(0.5), "0.5 s");
        assert_eq!(shutter(30.0), "30 s");
        assert_eq!(aperture(8.0), "f/8");
        assert_eq!(aperture(2.8), "f/2.8");
        assert_eq!(focal(4.2), "4.2 mm");
        assert_eq!(bias(2.0 / 3.0), "+0.7 EV");
        assert_eq!(bias(-1.0), "-1 EV");
        assert_eq!(bias(0.01), "0 EV");
        assert_eq!(
            date_time("2017:01:08 11:18:13", Some("+01:00")),
            "2017-01-08 11:18:13（UTC+01:00）"
        );
        assert_eq!(flash(16), "未闪光");
        assert_eq!(flash(1), "已闪光");
        assert_eq!(flash(0x20), "无闪光功能");
    }

    #[test]
    fn names_do_not_repeat_the_make() {
        let mut c = camera();
        assert_eq!(camera_name(&c), "Canon EOS 6D");
        c.model = "Canon EOS 6D".into();
        assert_eq!(camera_name(&c), "Canon EOS 6D");
        c.lens_model = None;
        assert_eq!(lens_name(&c).as_deref(), Some("50 mm"));
        c.lens_focal_range = Some((24.0, 70.0));
        assert_eq!(lens_name(&c).as_deref(), Some("24–70 mm"));
    }

    #[test]
    fn overlay_and_sections() {
        let info = ImageInfo {
            kind: SourceKind::Raw,
            source_size: Size::new(6000, 4000),
            exif_orientation: 1,
            camera: Some(camera()),
            as_shot: WbValuesDto {
                temperature: 5203.4,
                tint: 3.0,
            },
        };
        assert_eq!(
            overlay_lines(&info),
            vec![
                "Canon EOS 6D · EF50mm f/1.8 STM".to_string(),
                "50 mm · f/5.6 · 1/250 s · ISO 800 · +0.7 EV".to_string(),
            ]
        );
        let file = FileFacts {
            name: "IMG_0001.CR2".into(),
            path: Some("/photos".into()),
            bytes: Some(23 * 1024 * 1024),
        };
        let s = sections(&info, &file);
        let titles: Vec<_> = s.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["文件", "相机", "镜头", "拍摄", "位置"]);
        let find = |k: &str| {
            s.iter()
                .flat_map(|s| &s.rows)
                .find(|r| r.0 == k)
                .map(|r| r.1.clone())
        };
        assert_eq!(find("尺寸").as_deref(), Some("6000 × 4000（24.0 MP）"));
        assert_eq!(find("文件大小").as_deref(), Some("23.0 MB"));
        assert_eq!(find("焦距").as_deref(), Some("50 mm（等效 75 mm）"));
        assert_eq!(find("曝光程序").as_deref(), Some("光圈优先"));
        assert_eq!(find("经度").as_deref(), Some("84.68170° W"));
        assert_eq!(find("原照白平衡").as_deref(), Some("5203 K，色调 3"));

        let bare = ImageInfo {
            camera: None,
            ..info
        };
        assert!(overlay_lines(&bare).is_empty());
        assert_eq!(sections(&bare, &file).len(), 1);
    }
}
