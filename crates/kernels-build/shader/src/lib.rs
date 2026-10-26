//! GPU entry points (doc 04 §3). Each entry point only unpacks its parameters and calls the
//! shared per-pixel function from `aigle_kernels::tiles`; no image math lives here.
//!
//! Bindings (descriptor set 0) for every entry point:
//! 0: `f32` params (C1: followed by the mask program), 1: `u32` params, 2: input pixels,
//! 3: output pixels, 4: auxiliary `f32` data shared by the batch (C2 field / blue noise +
//! curve tables).
//!
//! The S-tier entry points (`s_*`) use bindings 0–3 only (2: source ROI, 3: the block's
//! `f32` workspace), with dynamic offsets.
//!
//! C1 tiles are 2 words per pixel (RGBA f16), or 5 when D-tier local fields are carried
//! (RGB f16, then contrast/whites, blacks/saturation, hue/0 as f16 pairs).

#![no_std]

use aigle_kernels::c2::FieldView;
use aigle_kernels::tiles::{self, PackedRoi};
use spirv_std::float::{f16x2_to_vec2, vec2_to_f16x2};
use spirv_std::glam::{UVec3, Vec2};
use spirv_std::spirv;

/// B1 (affine or warp grid; bilinear or Lanczos-3; per-channel TCA) ∘ C1 ∘ M ∘ C2 for one
/// Crop-grid tile.
#[spirv(compute(threads(8, 8)))]
pub fn c1_tile(
    #[spirv(global_invocation_id)] id: UVec3,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 0)] fp: &[f32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] up: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] src: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 3)] out: &mut [u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 4)] aux: &[f32],
) {
    let (w, h) = (up[0], up[1]);
    if id.x >= w || id.y >= h {
        return;
    }
    let roi = PackedRoi {
        data: src,
        x0: up[2],
        y0: up[3],
        w: up[4],
        h: up[5],
        full_w: up[6],
        full_h: up[7],
        atlas: up[16] != 0,
        table: up,
        tbase: tiles::C1_U32,
        full: up[17] != 0,
    };
    let field = FieldView { data: aux, w: up[8], h: up[9] };
    let (affine, params, c2) = tiles::unpack_c1(fp, up[10]);
    let woff = up[13] as usize;
    let geo = tiles::Geo {
        affine,
        warp: fp,
        wbase: woff + 1,
        wn: if woff == 0 { 0 } else { up[14] },
        cell: if woff == 0 { 1.0 } else { fp[woff] },
        flags: up[12],
    };
    let tab = tiles::C1Tables { data: fp, prog: tiles::C1_F32, color: up[15] as usize };
    let (px, d) = tiles::c1_px(&roi, &geo, id.x, id.y, &params, &c2, &field, &tab);
    let fields = up[11] != 0;
    if up[17] != 0 {
        let stride = if fields { 8 } else { 3 };
        let i = ((id.y * w + id.x) * stride) as usize;
        out[i] = px[0].to_bits();
        out[i + 1] = px[1].to_bits();
        out[i + 2] = px[2].to_bits();
        if fields {
            out[i + 3] = d[0].to_bits();
            out[i + 4] = d[1].to_bits();
            out[i + 5] = d[2].to_bits();
            out[i + 6] = d[3].to_bits();
            out[i + 7] = d[4].to_bits();
        }
        return;
    }
    let stride = if fields { 5 } else { 2 };
    let i = ((id.y * w + id.x) * stride) as usize;
    out[i] = vec2_to_f16x2(Vec2::new(px[0], px[1]));
    out[i + 1] = vec2_to_f16x2(Vec2::new(px[2], 0.0));
    if fields {
        out[i + 2] = vec2_to_f16x2(Vec2::new(d[0], d[1]));
        out[i + 3] = vec2_to_f16x2(Vec2::new(d[2], d[3]));
        out[i + 4] = vec2_to_f16x2(Vec2::new(d[4], 0.0));
    }
}

/// Write the outputs of an S-tier pass (`aigle_kernels::detail::out_pos`).
fn s_store(work: &mut [f32], up: &[u32], x: u32, y: u32, v: [f32; 4]) {
    use aigle_kernels::detail::{NO_POS, out_pos};
    let p0 = out_pos(up, 0, x, y);
    let p1 = out_pos(up, 1, x, y);
    let p2 = out_pos(up, 2, x, y);
    let p3 = out_pos(up, 3, x, y);
    if p0 != NO_POS {
        work[p0] = v[0];
    }
    if p1 != NO_POS {
        work[p1] = v[1];
    }
    if p2 != NO_POS {
        work[p2] = v[2];
    }
    if p3 != NO_POS {
        work[p3] = v[3];
    }
}

fn s_roi<'a>(src: &'a [u32], up: &'a [u32]) -> PackedRoi<'a> {
    let r = aigle_kernels::detail::U_ROI;
    PackedRoi {
        data: src,
        x0: up[r],
        y0: up[r + 1],
        w: up[r + 2],
        h: up[r + 3],
        full_w: up[r + 4],
        full_h: up[r + 5],
        atlas: false,
        table: up,
        tbase: 0,
        full: up[r + 6] != 0,
    }
}

/// S-tier passes (`aigle_kernels::detail`), one entry point per op group
/// (`detail::ENTRY_NAMES`). Binding 2 is the source ROI (RGBA f16), binding 3 the block's
/// `f32` workspace.
macro_rules! s_entry {
    ($name:ident, |$work:ident, $src:ident, $fp:ident, $up:ident, $x:ident, $y:ident| $body:expr) => {
        #[spirv(compute(threads(8, 8)))]
        pub fn $name(
            #[spirv(global_invocation_id)] id: UVec3,
            #[spirv(storage_buffer, descriptor_set = 0, binding = 0)] $fp: &[f32],
            #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] $up: &[u32],
            #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] $src: &[u32],
            #[spirv(storage_buffer, descriptor_set = 0, binding = 3)] work: &mut [f32],
        ) {
            use aigle_kernels::detail as s;
            if id.x >= $up[s::U_GW] || id.y >= $up[s::U_GH] {
                return;
            }
            let ($x, $y) = (id.x, id.y);
            let v = {
                let $work: &[f32] = work;
                $body
            };
            s_store(work, $up, id.x, id.y, v);
        }
    };
}

s_entry!(s_split, |_w, src, fp, up, x, y| s::split_px(&s_roi(src, up), fp, up, x, y));
s_entry!(s_fields, |_w, src, fp, up, x, y| s::fields_px(&s_roi(src, up), fp, up, x, y));
s_entry!(s_bilateral, |w, _src, fp, up, x, y| s::bilateral_px(w, fp, up, x, y));
s_entry!(s_box, |w, _src, fp, up, x, y| s::box_px(w, up, x, y));
s_entry!(s_gauss, |w, _src, fp, up, x, y| s::gauss_px(w, fp, up, x, y));
s_entry!(s_point, |w, _src, fp, up, x, y| s::point_px(w, fp, up, x, y));

/// D1 ∘ D2 ∘ D3 ∘ E1 ∘ quantise for one tile. Output: BGRA8, 1 × u32 per pixel.
#[spirv(compute(threads(8, 8)))]
pub fn out_tile(
    #[spirv(global_invocation_id)] id: UVec3,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 0)] fp: &[f32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] up: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] src: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 3)] out: &mut [u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 4)] aux: &[f32],
) {
    let (w, h) = (up[0], up[1]);
    if id.x >= w || id.y >= h {
        return;
    }
    let i = (id.y * w + id.x) as usize;
    let fields = up[9] != 0;
    let (gx, gy) = (up[2] + id.x, up[3] + id.y);
    if up[11] != 0 {
        // Full precision: f32 input, 16-bit output.
        let j = if fields { i * 8 } else { i * 3 };
        let px = [f32::from_bits(src[j]), f32::from_bits(src[j + 1]), f32::from_bits(src[j + 2])];
        let params = tiles::unpack_out(fp, up);
        let v = if fields {
            let d = [
                f32::from_bits(src[j + 3]),
                f32::from_bits(src[j + 4]),
                f32::from_bits(src[j + 5]),
                f32::from_bits(src[j + 6]),
                f32::from_bits(src[j + 7]),
            ];
            tiles::out_lin_local(px, Some(d), gx, gy, &params, aux)
        } else {
            tiles::out_lin_local(px, None, gx, gy, &params, aux)
        };
        let o = tiles::encode_out16(v, &params.e1);
        out[i * 2] = o[0];
        out[i * 2 + 1] = o[1];
        return;
    }
    let j = if fields { i * 5 } else { i * 2 };
    let rg = f16x2_to_vec2(src[j]);
    let b = f16x2_to_vec2(src[j + 1]);
    let px = [rg.x, rg.y, b.x];
    out[i] = if fields {
        let a = f16x2_to_vec2(src[j + 2]);
        let c = f16x2_to_vec2(src[j + 3]);
        let e = f16x2_to_vec2(src[j + 4]);
        let params = tiles::unpack_out(fp, up);
        tiles::out_px_local(px, Some([a.x, a.y, c.x, c.y, e.x]), gx, gy, &params, aux)
    } else if up[5] == 0 && up[6] == 0 && up[10] == 0 {
        let (d1, e1, d3, seed) = tiles::unpack_out_base(fp, up);
        tiles::out_px_base(px, gx, gy, &d1, &e1, &d3, seed, aux)
    } else {
        let params = tiles::unpack_out(fp, up);
        tiles::out_px(px, gx, gy, &params, aux)
    };
}
