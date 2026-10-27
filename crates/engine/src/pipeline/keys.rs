//! Per-product parameter keys, following the stage → parameter table (doc 02 §4, §11).
//!
//! The C1 tile fuses S ∘ B1 ∘ M ∘ C1 ∘ C2, so its key covers every path those stages read;
//! the output tile fuses D1 ∘ D2 ∘ D3 ∘ E1. The S key covers the S tier alone (texture runs
//! on the source grid with NR and sharpening, `crate::detail`). Paths that are still identity in the scaffold are
//! already included so that keys do not change meaning once the stages land.

use aigle_core::EditParams;
use aigle_engine_api::DisplayProfileId;

use crate::cache::{CacheKey, KeyBuilder};

/// Implementation versions of the fused products for process version 1 (doc 02 §13).
pub const C1_IMPL: u32 = 5;
pub const OUT_IMPL: u32 = 4;

pub fn s_key(a_key: &CacheKey, p: &EditParams) -> CacheKey {
    let mut k = KeyBuilder::new("S");
    let local = crate::masks::relevant(p, crate::masks::Tier::S);
    k.json(&p.process_version)
        .key(a_key)
        .json(&p.raw.ai_denoise)
        .json(&p.detail)
        .f32(p.presence.texture)
        // Local noise/sharpness/texture fields (only masks that use them).
        .json(&local);
    if !local.is_empty() {
        // Range masks are evaluated on the C1 output.
        k.json(&p.profile.camera).json(&p.wb).f32(p.light.exposure);
    }
    k.finish()
}

pub fn c1_key(a_key: &CacheKey, p: &EditParams) -> CacheKey {
    let mut k = KeyBuilder::new("S-B1-M-C1");
    k.json(&p.process_version)
        .key(a_key)
        // S1/S2
        .json(&p.raw.ai_denoise)
        .json(&p.detail)
        // B1
        .json(&p.geometry)
        .json(&p.lens)
        // M (fields consumed by S/C tiers)
        .json(&p.masks)
        // C1
        .json(&p.profile.camera)
        .json(&p.wb)
        .f32(p.light.exposure)
        // C2 (+ texture, S)
        .f32(p.light.highlights)
        .f32(p.light.shadows)
        .f32(p.presence.texture)
        .f32(p.presence.clarity)
        .f32(p.presence.dehaze);
    k.finish()
}

pub fn out_key(p: &EditParams, display: &DisplayProfileId) -> CacheKey {
    let l = &p.light;
    let mut k = KeyBuilder::new("C2-D-E1");
    k.json(&p.process_version)
        // D1
        .f32(l.contrast)
        .f32(l.whites)
        .f32(l.blacks)
        // D2
        .json(&p.curve)
        .json(&p.mixer)
        .json(&p.bw)
        .json(&p.grading)
        .f32(p.presence.vibrance)
        .f32(p.presence.saturation)
        .json(&p.profile.look)
        // D3
        .json(&p.effects)
        // E1
        .str(&format!("{display:?}"));
    k.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cache-key completeness (doc 02 §16): mutating a path changes exactly the keys of the
    /// products that read it.
    #[test]
    fn key_completeness_per_path() {
        let a = KeyBuilder::new("a").finish();
        let d = DisplayProfileId::Srgb;
        let base = EditParams::default();
        let (c0, o0) = (c1_key(&a, &base), out_key(&base, &d));
        for (path, spec) in EditParams::scalar_specs() {
            let mut p = base.clone();
            let v = if spec.default + spec.step <= spec.max {
                spec.default + spec.step
            } else {
                spec.default - spec.step
            };
            p.set(&path, v).unwrap();
            let (c, o) = (c1_key(&a, &p), out_key(&p, &d));
            let in_c1 = ["raw.", "detail.", "geometry.", "lens.", "wb."]
                .iter()
                .any(|g| path.starts_with(g))
                || [
                    "light.exposure",
                    "light.highlights",
                    "light.shadows",
                    "presence.texture",
                    "presence.clarity",
                    "presence.dehaze",
                ]
                .contains(&path.as_str());
            assert_eq!(c != c0, in_c1, "C1 key vs {path}");
            assert_eq!(o != o0, !in_c1, "output key vs {path}");
        }
    }
}
