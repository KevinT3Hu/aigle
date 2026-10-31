//! Compile the C shim against the pinned, codec-complete vcpkg stack.
#[path = "../../../build-support/native.rs"]
mod native;
fn main() {
    native::configure();
    let vips = native::probe("vips", "8.18");
    let lcms = native::probe("lcms2", "2.9");
    let exif = native::probe("libexif", "0.6");
    let mut b = cc::Build::new();
    b.file("shim/aigle_vips.c").opt_level(2).warnings(false);
    for p in vips
        .include_paths
        .iter()
        .chain(&exif.include_paths)
        .chain(&lcms.include_paths)
    {
        b.include(p);
    }
    b.compile("aigle_vips");
    println!("cargo:rerun-if-changed=shim");
}
