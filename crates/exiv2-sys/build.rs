//! Builds the aigle Exiv2 shim against the system Exiv2 ≥ 0.27, found with pkg-config. On
//! Debian/Ubuntu: `apt install libexiv2-dev`.

fn main() {
    let exiv2 = pkg_config::Config::new()
        .atleast_version("0.27")
        .probe("exiv2")
        .unwrap_or_else(|e| panic!("exiv2 ≥ 0.27 not found ({e}); install libexiv2-dev"));
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++14")
        .file("shim/aigle_exiv2.cpp")
        .opt_level(2)
        .warnings(false)
        .flag_if_supported("-Wno-deprecated-declarations");
    for p in &exiv2.include_paths {
        b.include(p);
    }
    b.compile("aigle_exiv2");
    println!("cargo:rerun-if-changed=shim");
}
