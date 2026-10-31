//! Compile only the project C ABI shim; LibRaw and its patches are managed by vcpkg.
#[path = "../../build-support/native.rs"]
mod native;
fn main() {
    native::configure();
    let raw = native::probe("phaos-libraw", "0.22.2");
    let jpeg = native::probe("libjpeg", "1");
    let mut shim = cc::Build::new();
    shim.cpp(true)
        .std("c++11")
        .opt_level(3)
        .debug(false)
        .warnings(false)
        .file("shim/aigle_libraw.cpp");
    for p in raw.include_paths.iter().chain(&jpeg.include_paths) {
        shim.include(p);
    }
    if shim.get_compiler().is_like_gnu() {
        shim.flag("-fopenmp");
        println!("cargo:rustc-link-lib=gomp");
    }
    shim.compile("aigle_libraw_shim");
    println!("cargo:rerun-if-changed=shim");
}
