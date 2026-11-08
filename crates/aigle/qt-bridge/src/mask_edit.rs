//! Source-space mask edits. Display points must be inverse-mapped before entering here.
use aigle_core::masks::{MaskShape, Pt};
use anyhow::{Result, bail};

pub fn movable(shape: &MaskShape) -> bool {
    match shape {
        MaskShape::Linear { .. } | MaskShape::Radial { .. } | MaskShape::Brush { .. } => true,
        MaskShape::Combine { parts, .. } => !parts.is_empty() && parts.iter().all(movable),
        _ => false,
    }
}
pub fn anchors(shape: &MaskShape, out: &mut Vec<Pt>) {
    match shape {
        MaskShape::Linear { from, to } => out.extend([*from, *to]),
        MaskShape::Radial { center, .. } => out.push(*center),
        MaskShape::Brush { strokes } => {
            out.extend(strokes.iter().flat_map(|s| s.points.iter()).copied())
        }
        MaskShape::Combine { parts, .. } => parts.iter().for_each(|p| anchors(p, out)),
        _ => {}
    }
}
pub fn translate(shape: &mut MaskShape, dx: f32, dy: f32) -> Result<()> {
    if !movable(shape) {
        bail!("This mask includes coverage that cannot be moved geometrically");
    }
    let mut points = vec![];
    anchors(shape, &mut points);
    let dx = dx.clamp(
        -points.iter().map(|p| p.x).fold(1., f32::min),
        1. - points.iter().map(|p| p.x).fold(0., f32::max),
    );
    let dy = dy.clamp(
        -points.iter().map(|p| p.y).fold(1., f32::min),
        1. - points.iter().map(|p| p.y).fold(0., f32::max),
    );
    fn shift(shape: &mut MaskShape, dx: f32, dy: f32) {
        let move_pt = |p: &mut Pt| {
            p.x += dx;
            p.y += dy;
        };
        match shape {
            MaskShape::Linear { from, to } => {
                move_pt(from);
                move_pt(to);
            }
            MaskShape::Radial { center, .. } => move_pt(center),
            MaskShape::Brush { strokes } => strokes
                .iter_mut()
                .for_each(|s| s.points.iter_mut().for_each(move_pt)),
            MaskShape::Combine { parts, .. } => parts.iter_mut().for_each(|s| shift(s, dx, dy)),
            _ => {}
        }
    }
    shift(shape, dx, dy);
    Ok(())
}
pub fn resize_radial(
    shape: &mut MaskShape,
    at: Pt,
    axis: usize,
    width: f32,
    height: f32,
) -> Result<()> {
    let MaskShape::Radial {
        center,
        radii,
        angle,
        ..
    } = shape
    else {
        bail!("Not a radial mask")
    };
    let a = angle.to_radians();
    let dx = (at.x - center.x) * width;
    let dy = (at.y - center.y) * height;
    if axis == 0 {
        radii[0] = ((dx * a.cos() + dy * a.sin()).abs() / width).clamp(0.005, 1.);
    } else {
        radii[1] = ((-dx * a.sin() + dy * a.cos()).abs() / height).clamp(0.005, 1.);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::masks::{CombineOp, Stroke};
    #[test]
    fn rotated_radial_resize_preserves_other_axis_angle_and_feather() {
        let mut shape = MaskShape::Radial {
            center: Pt { x: 0.5, y: 0.5 },
            radii: [0.1, 0.2],
            angle: 30.,
            feather: 63.,
        };
        let a = 30f32.to_radians();
        resize_radial(
            &mut shape,
            Pt {
                x: 0.5 + 0.3 * a.cos(),
                y: 0.5 + 0.6 * a.sin(),
            },
            0,
            1200.,
            600.,
        )
        .unwrap();
        let MaskShape::Radial {
            radii,
            angle,
            feather,
            ..
        } = shape
        else {
            panic!()
        };
        assert!((radii[0] - 0.3).abs() < 1e-6);
        assert_eq!(radii[1], 0.2);
        assert_eq!(angle, 30.);
        assert_eq!(feather, 63.);
    }
    #[test]
    fn combined_translation_clamps_as_a_group_and_preserves_brush_settings() {
        let mut shape = MaskShape::Combine {
            op: CombineOp::Subtract,
            parts: vec![
                MaskShape::Linear {
                    from: Pt { x: 0.2, y: 0.3 },
                    to: Pt { x: 0.8, y: 0.7 },
                },
                MaskShape::Brush {
                    strokes: vec![Stroke {
                        points: vec![Pt { x: 0.4, y: 0.5 }],
                        size: 0.04,
                        feather: 27.,
                        flow: 61.,
                        erase: true,
                    }],
                },
            ],
        };
        translate(&mut shape, 0.9, -0.9).unwrap();
        let MaskShape::Combine { op, parts } = shape else {
            panic!()
        };
        assert_eq!(op, CombineOp::Subtract);
        let MaskShape::Linear { from, to } = parts[0] else {
            panic!()
        };
        assert!((from.x - 0.4).abs() < 1e-6 && from.y.abs() < 1e-6);
        assert!((to.x - 1.).abs() < 1e-6 && (to.y - 0.4).abs() < 1e-6);
        let MaskShape::Brush { strokes } = &parts[1] else {
            panic!()
        };
        assert!(strokes[0].erase);
        assert_eq!(
            (strokes[0].size, strokes[0].feather, strokes[0].flow),
            (0.04, 27., 61.)
        );
    }
}
