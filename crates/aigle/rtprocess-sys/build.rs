//! Compile the C ABI shim; patched librtprocess comes from the vcpkg overlay port.
#[path = "../../../build-support/native.rs"]
mod native;
fn main() {
    native::configure();
    let lib = native::probe("phaos-rtprocess", "0.11");
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++11")
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .define("LIBRTPROCESS_STATIC", None)
        .flag_if_supported("-fno-fast-math")
        .flag_if_supported("-ffp-contract=off")
        .file("shim/aigle_rtprocess.cpp");
    for p in &lib.include_paths {
        b.include(p);
    }
    b.compile("aigle_rtprocess_shim");
    println!("cargo:rerun-if-changed=shim");
}
