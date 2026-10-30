//! Exercise the actual GPUI GPU pipeline: a native image hole must preserve
//! parent clipping and leave subsequent crop handles/menus visible.
use gpui::{
    Background, BorderStyle, Bounds, ContentMask, Corners, Edges, Hsla, PlatformHeadlessRenderer,
    Quad, Scene, point, size,
};
use gpui_wgpu::WgpuHeadlessRenderer;
fn quad(x: f32, y: f32, w: f32, h: f32, color: Hsla) -> Quad {
    let bounds = Bounds::new(point(x.into(), y.into()), size(w.into(), h.into()));
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds,
        content_mask: ContentMask { bounds },
        background: color.into(),
        border_color: color,
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
    }
}
#[test]
fn native_hole_clears_background_but_preserves_clipping_and_tool_overlays() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    scene.insert_primitive(quad(0.0, 0.0, 64.0, 64.0, gpui::red()));
    let mut hole = quad(8.0, 8.0, 48.0, 48.0, gpui::black());
    hole.background = Background::native_surface_hole();
    hole.content_mask.bounds = quad(16.0, 16.0, 32.0, 32.0, gpui::black()).bounds;
    scene.insert_primitive(hole);
    scene.insert_primitive(quad(24.0, 24.0, 8.0, 8.0, gpui::blue()));
    scene.finish();
    let image = renderer.render_scene_to_image(&scene, size(64.into(), 64.into()))?;
    for (x, y, want) in [
        (4, 4, [255, 0, 0, 255]),
        (10, 10, [255, 0, 0, 255]),
        (20, 20, [0, 0, 0, 0]),
        (27, 27, [0, 0, 255, 255]),
        (52, 52, [255, 0, 0, 255]),
    ] {
        let got = image.get_pixel(x, y).0;
        assert!(
            got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3),
            "({x},{y}): {got:?}, expected {want:?}"
        );
    }
    Ok(())
}
