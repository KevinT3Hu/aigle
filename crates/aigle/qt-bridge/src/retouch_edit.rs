//! Typed retouch edits and exact source-to-crop guide projection.
use aigle_core::{
    EditParams,
    geometry::{GeometryMap, Point},
    masks::Pt,
    retouch::{MAX_RADIUS, MIN_RADIUS},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub fn selected(params: &EditParams, kind: &str, index: usize) -> Result<Value> {
    match kind {
        "spot" => Ok(serde_json::to_value(
            params.spots.get(index).context("Spot no longer exists")?,
        )?),
        "eye" => Ok(serde_json::to_value(
            params
                .red_eye
                .get(index)
                .context("Eye correction no longer exists")?,
        )?),
        _ => bail!("Unknown correction type"),
    }
}
pub fn change(
    params: &EditParams,
    map: Option<&GeometryMap>,
    request: &Value,
) -> Result<EditParams> {
    let mut next = params.clone();
    let action = request["action"].as_str().unwrap_or("transform");
    if action == "clear" {
        next.spots.clear();
        next.red_eye.clear();
        return Ok(next);
    }
    let kind = request["kind"]
        .as_str()
        .context("Correction type required")?;
    let index = request["index"]
        .as_u64()
        .context("Correction index required")? as usize;
    let mut value = selected(params, kind, index)?;
    // Qt's shortest-double JSON round trip can change a final f64 bit even
    // though the stored f32 correction is identical. Compare domain values.
    let expected = match kind {
        "spot" => serde_json::to_value(serde_json::from_value::<aigle_core::retouch::Spot>(
            request["expected"].clone(),
        )?)?,
        "eye" => serde_json::to_value(serde_json::from_value::<aigle_core::retouch::RedEye>(
            request["expected"].clone(),
        )?)?,
        _ => bail!("Unknown correction type"),
    };
    ensure!(
        value == expected,
        "Correction changed; select it again before editing"
    );
    if action == "delete" {
        if kind == "spot" {
            next.spots.remove(index);
        } else {
            next.red_eye.remove(index);
        }
        return Ok(next);
    }
    if action == "set" {
        let field = request["field"]
            .as_str()
            .context("Correction field required")?;
        let allowed = if kind == "spot" {
            [
                "mode", "radius", "feather", "opacity", "target.x", "target.y", "source.x",
                "source.y",
            ]
            .as_slice()
        } else {
            ["radius", "pupil", "darken", "center.x", "center.y"].as_slice()
        };
        ensure!(allowed.contains(&field), "Unknown correction field");
        if let Some((parent, key)) = field.split_once('.') {
            value[parent][key] = request["value"].clone();
        } else {
            value[field] = request["value"].clone();
        }
    } else if action == "transform" {
        let map = map.context("Photo is still loading; geometry is required for handle edits")?;
        let points: [Point; 2] = serde_json::from_value(request["points"].clone())
            .context("Two retouch points required")?;
        ensure!(
            points.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
            "Invalid retouch points"
        );
        let size = map.source_size();
        let a = map.crop_to_source(points[0]);
        let b = map.crop_to_source(points[1]);
        let handle = request["handle"]
            .as_str()
            .context("Retouch handle required")?;
        let target = if kind == "spot" { "target" } else { "center" };
        match handle {
            "target" | "source" => {
                ensure!(
                    handle != "source" || kind == "spot",
                    "Red-eye corrections have no source handle"
                );
                let field = if handle == "source" { "source" } else { target };
                value[field]["x"] = json!(
                    (value[field]["x"].as_f64().unwrap() + (b.x - a.x) / f64::from(size.w))
                        .clamp(0., 1.)
                );
                value[field]["y"] = json!(
                    (value[field]["y"].as_f64().unwrap() + (b.y - a.y) / f64::from(size.h))
                        .clamp(0., 1.)
                );
            }
            "radius" => {
                let x = value[target]["x"].as_f64().unwrap() * f64::from(size.w);
                let y = value[target]["y"].as_f64().unwrap() * f64::from(size.h);
                value["radius"] = json!(
                    ((b.x - x).hypot(b.y - y) / f64::from(size.long_edge()))
                        .clamp(f64::from(MIN_RADIUS), f64::from(MAX_RADIUS))
                );
            }
            _ => bail!("Unknown retouch handle"),
        }
    } else {
        bail!("Unknown retouch action");
    }
    if kind == "spot" {
        next.spots[index] = serde_json::from_value(value)?;
    } else {
        next.red_eye[index] = serde_json::from_value(value)?;
    }
    next.sanitize()?;
    Ok(next)
}
pub fn guide(params: &EditParams, map: &GeometryMap, kind: &str, index: usize) -> Result<Value> {
    let value = selected(params, kind, index)?;
    let size = map.source_size();
    let project = |p: Pt| {
        map.source_to_crop(Point::new(
            f64::from(p.x) * f64::from(size.w),
            f64::from(p.y) * f64::from(size.h),
        ))
    };
    let target: Pt =
        serde_json::from_value(value[if kind == "spot" { "target" } else { "center" }].clone())?;
    let radius = value["radius"].as_f64().unwrap() * f64::from(size.long_edge());
    let circle = |center: Pt| {
        (0..=64)
            .map(|i| {
                let angle = i as f64 * std::f64::consts::TAU / 64.;
                project(Pt {
                    x: center.x + (radius * angle.cos() / f64::from(size.w)) as f32,
                    y: center.y + (radius * angle.sin() / f64::from(size.h)) as f32,
                })
            })
            .collect::<Vec<_>>()
    };
    let outline = circle(target);
    let mut result = json!({"kind":kind,"index":index,"value":value,"target":project(target),"radius_handle":outline[0],"outline":outline});
    if kind == "spot" {
        let source: Pt = serde_json::from_value(value["source"].clone())?;
        result["source"] = json!(project(source));
        result["source_outline"] = json!(circle(source));
    }
    Ok(result)
}
pub fn guides(params: &EditParams, map: &GeometryMap) -> Vec<Value> {
    (0..params.spots.len())
        .map(|i| guide(params, map, "spot", i).unwrap())
        .chain((0..params.red_eye.len()).map(|i| guide(params, map, "eye", i).unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::{
        geometry::Size,
        retouch::{RedEye, Spot, SpotMode},
    };
    fn fixture() -> (EditParams, GeometryMap) {
        let mut p = EditParams::default();
        p.geometry.scalars.angle = 27.;
        p.geometry.scalars.perspective_horizontal = 15.;
        p.spots.push(Spot::new(
            SpotMode::Clone,
            Pt { x: 0.5, y: 0.4 },
            Pt { x: 0.2, y: 0.7 },
            0.04,
        ));
        p.spots[0].opacity = 0.37;
        p.spots[0].feather = 63.;
        p.red_eye.push(RedEye::new(Pt { x: 0.7, y: 0.6 }, 0.02));
        use aigle_core::lens::{DistEntry, DistModel, LensProfileData, LensShot};
        let size = Size::new(1200, 800);
        let profile = LensProfileData {
            lens_maker: "Test".into(),
            lens_model: "Test".into(),
            calib_crop: 1.,
            calib_aspect: 1.5,
            camera_crop: 1.,
            min_focal: 24.,
            max_focal: 24.,
            distortion: vec![DistEntry {
                focal: 24.,
                real_focal: None,
                model: DistModel::Ptlens {
                    a: 0.01,
                    b: -0.05,
                    c: 0.02,
                },
            }],
            tca: vec![],
            vignetting: vec![],
        };
        let lens = profile.model(
            &LensShot {
                focal: 24.,
                aperture: 4.,
                distance: 1000.,
            },
            size,
            [1.; 3],
        );
        let map = GeometryMap::with_optics(
            size,
            &p.geometry,
            &aigle_core::geometry::Optics {
                lens: Some(lens),
                focal_35mm: Some(24.),
            },
        );
        (p, map)
    }
    #[test]
    fn transformed_target_source_and_radius_preserve_other_correction_settings() {
        let (p, map) = fixture();
        let from = map.source_to_crop(Point::new(600., 320.));
        let to = map.source_to_crop(Point::new(720., 400.));
        let mut r = json!({"kind":"spot","index":0,"expected":p.spots[0],"points":[from,to],"handle":"target"});
        let next = change(&p, Some(&map), &r).unwrap();
        assert!((next.spots[0].target.x - 0.6).abs() < 1e-5);
        assert!((next.spots[0].target.y - 0.5).abs() < 1e-5);
        assert_eq!(next.spots[0].source, p.spots[0].source);
        assert_eq!(next.spots[0].opacity, 0.37);
        assert_eq!(next.spots[0].feather, 63.);
        assert_eq!(next.red_eye, p.red_eye);
        r["handle"] = json!("source");
        let next = change(&p, Some(&map), &r).unwrap();
        assert!((next.spots[0].source.x - 0.3).abs() < 1e-5);
        assert_eq!(next.spots[0].target, p.spots[0].target);
        r["handle"] = json!("radius");
        r["points"] = json!([from, map.source_to_crop(Point::new(720., 320.))]);
        let next = change(&p, Some(&map), &r).unwrap();
        assert!((next.spots[0].radius - 0.1).abs() < 1e-5);
        assert_eq!(next.spots[0].target, p.spots[0].target);
        let guide = guide(&next, &map, "spot", 0).unwrap();
        let rim: Point = serde_json::from_value(guide["radius_handle"].clone()).unwrap();
        let rim = map.crop_to_source(rim);
        assert!((rim.x - 720.).abs() < 0.02);
        assert!((rim.y - 320.).abs() < 0.02);
        assert_eq!(guide["outline"].as_array().unwrap().len(), 65);
        assert_eq!(guide["source_outline"].as_array().unwrap().len(), 65);
    }
    #[test]
    fn equivalent_json_coordinate_round_trips_are_not_stale() {
        let (p, _map) = fixture();
        let mut expected = serde_json::to_value(&p.spots[0]).unwrap();
        expected["source"]["x"] = json!(0.2_f64);
        expected["source"]["y"] = json!(0.7_f64);
        expected["target"]["y"] = json!(0.4_f64);
        let r = json!({"kind":"spot","index":0,"expected":expected,"action":"set","field":"opacity","value":0.25});
        let next = change(&p, None, &r).unwrap();
        assert_eq!(next.spots[0].opacity, 0.25);
        assert_eq!(next.spots[0].source, p.spots[0].source);
    }
    #[test]
    fn stale_or_invalid_edits_fail_and_delete_remains_undoable_document_data() {
        let (p, map) = fixture();
        let mut r = json!({"kind":"spot","index":0,"expected":p.spots[0],"action":"set","field":"mode","value":"heal"});
        assert_eq!(
            change(&p, Some(&map), &r).unwrap().spots[0].mode,
            SpotMode::Heal
        );
        r["expected"]["opacity"] = json!(0.9);
        assert!(change(&p, Some(&map), &r).is_err());
        r = json!({"kind":"eye","index":0,"expected":p.red_eye[0],"action":"set","field":"opacity","value":0.2});
        assert!(change(&p, Some(&map), &r).is_err());
        r["field"] = json!("darken");
        r["value"] = json!(75);
        let next = change(&p, None, &r).unwrap();
        assert_eq!(next.red_eye[0].darken, 75.);
        assert_eq!(next.spots, p.spots);
        r["action"] = json!("delete");
        let next = change(&p, None, &r).unwrap();
        assert!(next.red_eye.is_empty());
        assert_eq!(next.spots, p.spots);
        assert_eq!(p.red_eye.len(), 1);
    }
}
