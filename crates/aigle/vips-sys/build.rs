//! Compile the C shim against the pinned, codec-complete vcpkg stack.
#[path = "../../../build-support/native.rs"]
mod native;
fn main() {
    native::configure();
    let vips = native::probe("vips", "8.18");
    let lcms = native::probe("lcms2", "2.9");
    let exif = native::probe("libexif", "0.6");
    let mut b = cc::Build::new();
    b.file("shim/aigle_vips.c")
        .std("c11")
        .opt_level(2)
        .warnings(false);
    for p in vips
        .include_paths
        .iter()
        .chain(&exif.include_paths)
        .chain(&lcms.include_paths)
    {
        b.include(p);
    }
    for (key, value) in vips
        .defines
        .iter()
        .chain(&lcms.defines)
        .chain(&exif.defines)
    {
        b.define(key, value.as_deref());
    }
    b.compile("aigle_vips");
    if std::env::var("TARGET")
        .unwrap_or_default()
        .ends_with("-msvc")
    {
        // GLib/GIO and Cairo rely on these MSVC default system libraries,
        // which their pkg-config metadata does not include.
        for lib in ["user32", "shell32", "ole32", "uuid"] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    println!("cargo:rerun-if-changed=shim");
}
