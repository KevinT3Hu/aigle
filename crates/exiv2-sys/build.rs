//! Compile the C++ metadata shim against managed Exiv2.
#[path = "../../build-support/native.rs"]
mod native;
fn main() {
    native::configure();
    let exiv2 = native::probe("exiv2", "0.27");
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++17")
        .file("shim/aigle_exiv2.cpp")
        .opt_level(2)
        .warnings(false)
        .flag_if_supported("-Wno-deprecated-declarations");
    for p in &exiv2.include_paths {
        b.include(p);
    }
    for (key, value) in &exiv2.defines {
        b.define(key, value.as_deref());
    }
    b.compile("aigle_exiv2");
    println!("cargo:rerun-if-changed=shim");
}
