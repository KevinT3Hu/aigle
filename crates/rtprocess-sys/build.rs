//! Builds the vendored librtprocess (CarVac/librtprocess master, commit 9a858270,
//! 2023-06-27) and the aigle shim. GPLv3-or-later, compatible with AGPL-3.0 (doc 01 §11).
//!
//! The vendored sources carry one local patch, marked "aigle patch": demosaic and CA
//! correction stop and return `RP_CANCELLED` when their progress callback returns true.
//!
//! Always optimised; no OpenMP (doc 05 §5). Only SSE2 is assumed (x86-64 baseline), so the
//! output does not depend on the build machine (doc 02 §14).

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("vendor/librtprocess/src");
    let files = [
        "demosaic/amaze.cc",
        "demosaic/border.cc",
        "demosaic/markesteijn.cc",
        "demosaic/rcd.cc",
        "preprocess/CA_correct.cc",
        "postprocess/hilite_recon.cc",
    ];

    cc::Build::new()
        .cpp(true)
        .std("c++11")
        .opt_level(2)
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

    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=vendor");
}
