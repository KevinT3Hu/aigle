//! Shared native dependency configuration for Cargo shims and xtask.
#![allow(dead_code)]
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("vcpkg.json").is_file())
        .expect("workspace vcpkg.json")
        .to_path_buf()
}

pub fn triplet(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x64-linux-phaos",
        "aarch64-unknown-linux-gnu" => "arm64-linux-phaos",
        "x86_64-pc-windows-msvc" => "x64-windows-phaos",
        _ => panic!("managed native dependencies do not yet support {target}"),
    }
}

pub fn prefix(root: &Path, triplet: &str) -> PathBuf {
    root.join("target/native/installed").join(triplet)
}

/// Detect stale local overlays/configuration. Archive integrity uses SHA512 in ports.
pub fn fingerprint(root: &Path) -> String {
    fn collect(p: &Path, files: &mut Vec<PathBuf>) {
        if p.is_dir() {
            for entry in std::fs::read_dir(p).expect("native dependency inputs") {
                collect(&entry.expect("native input").path(), files);
            }
        } else if p.extension().is_some_and(|e| e != "md") {
            files.push(p.to_path_buf());
        }
    }
    let mut files = vec![
        root.join("vcpkg.json"),
        root.join("vcpkg-configuration.json"),
        root.join("native/vcpkg-revision"),
    ];
    collect(&root.join("native/ports"), &mut files);
    collect(&root.join("native/triplets"), &mut files);
    files.sort();
    let mut hash = 0xcbf29ce484222325u64;
    for file in files {
        for byte in file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .bytes()
            .chain(std::iter::once(0))
            .chain(std::fs::read(&file).expect("native dependency input"))
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}\n")
}
