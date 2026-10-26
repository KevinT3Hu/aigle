//! Builds the vendored LibRaw (tag 0.22.2, commit 24fa7e54) and the aigle shim.
//!
//! Native code is always optimised, even in dev builds, because unoptimised decoders are
//! unusably slow. OpenMP is off: the engine scheduler owns the CPU (doc 05 §5). The
//! reentrant build is used (no `LIBRAW_NOTHREADS`): one instance per decode worker.
//! zlib is required (deflate DNG); libjpeg is not used (lossy DNG is not supported yet).

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("vendor/LibRaw");

    let mut sources = Vec::new();
    collect(&vendor.join("src"), &mut sources);
    // `*_ph.cpp` are placeholders for optional packs and must not be compiled.
    sources.retain(|p| {
        !p.file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_ph.cpp")
    });
    sources.sort();
    sources.push(root.join("shim/aigle_libraw.cpp"));

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++11")
        .opt_level(2)
        .debug(false)
        .warnings(false)
        .include(&vendor)
        .define("USE_ZLIB", None)
        .flag_if_supported("-w")
        .files(&sources);
    build.compile("aigle_libraw");

    println!("cargo:rustc-link-lib=z");
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=vendor");
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read vendor dir") {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "cpp") {
            out.push(p);
        }
    }
}
