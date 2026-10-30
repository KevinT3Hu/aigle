//! Builds the vendored librtprocess (CarVac/librtprocess master, commit 9a858270,
//! 2023-06-27) and the aigle shim. GPLv3-or-later, compatible with AGPL-3.0 (doc 01 §11).
//!
//! Local patches are marked "aigle patch": cancellation, bounded tile teams,
//! deterministic CA reduction and disjoint scratch writes.
//!
//! Always optimised. RCD and CA use bounded OpenMP with GCC; other stages stay serial.
//! Only SSE2 is assumed (x86-64 baseline), so the
//! output does not depend on the build machine (doc 02 §14).

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("vendor/librtprocess/src");
    let files = [
        "demosaic/amaze.cc",
        "demosaic/border.cc",
        "demosaic/markesteijn.cc",
        "postprocess/hilite_recon.cc",
    ];

    cc::Build::new()
        .cpp(true)
        .std("c++11")
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .define("LIBRTPROCESS_STATIC", None)
        .include(src.join("include"))
        .flag_if_supported("-w")
        .flag_if_supported("-fno-fast-math")
        .flag_if_supported("-ffp-contract=off")
        .files(files.iter().map(|f| src.join(f)))
        .file(root.join("shim/aigle_rtprocess.cpp"))
        .compile("aigle_rtprocess");

    // RCD and CA use bounded tile teams. CA reduces stored shifts in raster order
    // so dynamic worker scheduling cannot change floating-point results.
    let mut tiles = cc::Build::new();
    tiles
        .cpp(true)
        .std("c++11")
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .define("LIBRTPROCESS_STATIC", None)
        .include(src.join("include"))
        .flag_if_supported("-fno-fast-math")
        .flag_if_supported("-ffp-contract=off")
        .file(src.join("demosaic/rcd.cc"))
        .file(src.join("preprocess/CA_correct.cc"));
    if tiles.get_compiler().is_like_gnu() {
        tiles.flag("-fopenmp");
        println!("cargo:rustc-link-lib=gomp");
    }
    tiles.compile("aigle_rcd");

    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=vendor");
}
