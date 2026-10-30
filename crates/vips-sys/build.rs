//! Builds the aigle libvips shim against libvips ≥ 8.15, libexif and lcms2, found with
//! pkg-config. On Debian/Ubuntu: `apt install libvips-dev libexif-dev` (libvips brings lcms2
//! for ICC and libheif for HEIF/AVIF as runtime modules).

fn main() {
    let lib = |name: &str, version: &str, pkg: &str| {
        pkg_config::Config::new()
            .atleast_version(version)
            .probe(name)
            .unwrap_or_else(|e| panic!("{name} ≥ {version} not found ({e}); install {pkg}"))
    };
    let vips = lib("vips", "8.15", "libvips-dev");
    let lcms = lib("lcms2", "2.9", "liblcms2-dev");
    let exif = lib("libexif", "0.6", "libexif-dev");
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
