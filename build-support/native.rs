//! Managed-only pkg-config probes, scoped to each native Cargo build script.
#[path = "native-config.rs"]
mod config;
use config::*;
pub fn configure() {
    let root = root();
    let target = std::env::var("TARGET").expect("Cargo TARGET");
    let prefix = prefix(&root, triplet(&target));
    let stamp = std::fs::read_to_string(prefix.join(".phaos-build-id")).unwrap_or_default();
    assert_eq!(
        stamp,
        fingerprint(&root),
        "native dependencies are missing or stale; run `cargo xtask native-deps` (or `--server` for Thesauros)"
    );
    let paths =
        std::env::join_paths([prefix.join("lib/pkgconfig"), prefix.join("share/pkgconfig")])
            .unwrap();
    // SAFETY: called once at build-script startup, before any cc worker threads exist.
    // Restrict this process's probes; other crates can still discover OS platform APIs.
    unsafe {
        for base in ["PKG_CONFIG_PATH", "PKG_CONFIG_LIBDIR"] {
            for key in [
                format!("{base}_{target}"),
                format!("{base}_{}", target.replace('-', "_")),
                format!("TARGET_{base}"),
                format!("HOST_{base}"),
            ] {
                std::env::remove_var(key);
            }
        }
        std::env::set_var("PKG_CONFIG_LIBDIR", &paths);
        std::env::set_var("PKG_CONFIG_PATH", &paths);
        if target.ends_with("windows-msvc") {
            let pkgconf = prefix.join("tools/pkgconf/pkgconf.exe");
            std::env::set_var("PKG_CONFIG", pkgconf);
        }
    }
    println!("cargo:rerun-if-changed={}", root.join("native").display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("vcpkg.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("vcpkg-configuration.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        prefix.join(".phaos-build-id").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build-support/native.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build-support/native-config.rs").display()
    );
}

pub fn probe(name: &str, minimum: &str) -> pkg_config::Library {
    let msvc = std::env::var("TARGET")
        .unwrap_or_default()
        .ends_with("-msvc");
    let library = pkg_config::Config::new()
        .cargo_metadata(!msvc)
        .statik(true)
        .atleast_version(minimum)
        .probe(name)
        .unwrap_or_else(|e| {
            panic!("managed {name} >= {minimum} unavailable: {e}; run `cargo xtask native-deps`")
        });
    if msvc {
        for path in &library.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &library.libs {
            // Some upstream .pc files use Unix -lfoo for an MSVC libfoo.lib
            // archive (notably libde265). Resolve the actual installed name.
            let installed = [name.clone(), format!("lib{name}")]
                .into_iter()
                .find(|candidate| {
                    library
                        .link_paths
                        .iter()
                        .any(|path| path.join(format!("{candidate}.lib")).is_file())
                });
            if let Some(name) = installed {
                println!("cargo:rustc-link-lib=static={name}");
            } else {
                println!("cargo:rustc-link-lib={name}");
            }
        }
        for path in &library.link_files {
            println!("cargo:rustc-link-arg={}", path.display());
        }
        for args in &library.ld_args {
            for arg in args {
                println!("cargo:rustc-link-arg={arg}");
            }
        }
    }
    library
}
