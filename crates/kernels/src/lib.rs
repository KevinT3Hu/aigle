//! aigle-kernels (doc 04 §3).
//!
//! Rules for kernel code: `no_std`, no allocation, no panics on the hot path, no trait objects
//! or recursion, all math in f32, no fast-math and no implicit FMA contraction. Storage
//! precision is a property of the buffer, not the kernel. Transcendentals come from `libm`
//! on the host, so the CPU reference path is bitwise deterministic across machines, and
//! from GLSL.std.450 in the SPIR-V build.
//!
//! The SPIR-V entry points live in `crates/kernels-build/shader`, a separate crate built
//! only by `cargo xtask kernels` with the pinned nightly; this crate stays a plain `rlib`
//! that compiles on stable.
//!
//! Kernel shapes: pointwise (`fn(px, params) -> px`), stencil (via [`resample::Rgb2D`]),
//! pyramid, reduce. Parameter structs are `#[repr(C)]` plain data so they can be uploaded as
//! GPU uniforms unchanged.

#![cfg_attr(not(test), no_std)]

pub mod c2;
pub mod d3;
pub mod detail;
pub mod dither;
pub mod luts;
pub mod masks;
pub mod math;
pub mod pointwise;
pub mod resample;
pub mod tiles;

pub type Rgb = [f32; 3];
pub type Mat3f = [[f32; 3]; 3];
