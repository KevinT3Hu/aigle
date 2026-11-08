//! Bootstrap and install the pinned vcpkg graph; Cargo build scripts never download.
#[path = "../../../../build-support/native-config.rs"]
mod config;
use std::{fs, process::Command};

fn run(cmd: &mut Command) -> Result<(), String> {
    let description = format!("{cmd:?}");
    let status = cmd.status().map_err(|e| format!("{description}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{description} failed: {status}"))
    }
}

pub fn install(server: bool) -> Result<(), String> {
    // Avoid canonicalize's Windows verbatim (\\?\) prefix: native build tools
    // need an ordinary absolute path.
    let root = config::root();
    let expected_inputs = config::fingerprint(&root);
    let revision =
        fs::read_to_string(root.join("native/vcpkg-revision")).map_err(|e| e.to_string())?;
    let vcpkg = root.join("target/native/vcpkg");
    if !vcpkg.join(".git").is_dir() {
        fs::create_dir_all(vcpkg.parent().unwrap()).map_err(|e| e.to_string())?;
        run(Command::new("git")
            .args([
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                "https://github.com/microsoft/vcpkg.git",
            ])
            .arg(&vcpkg))?;
    }
    let present = Command::new("git")
        .current_dir(&vcpkg)
        .args(["cat-file", "-e", &format!("{}^{{commit}}", revision.trim())])
        .status()
        .map_err(|e| e.to_string())?;
    if !present.success() {
        run(Command::new("git")
            .current_dir(&vcpkg)
            .args(["fetch", "origin", revision.trim()]))?;
    }
    run(Command::new("git")
        .current_dir(&vcpkg)
        .args(["checkout", "--detach", revision.trim()]))?;
    // Rebootstrap when the pinned tool revision changes as well as on first use.
    let tool_stamp = vcpkg.join(".phaos-tool-revision");
    if fs::read_to_string(&tool_stamp).unwrap_or_default() != revision
        || !vcpkg
            .join(if cfg!(windows) { "vcpkg.exe" } else { "vcpkg" })
            .is_file()
    {
        if cfg!(windows) {
            run(Command::new("cmd").current_dir(&vcpkg).args([
                "/c",
                "bootstrap-vcpkg.bat",
                "-disableMetrics",
            ]))?;
        } else {
            run(Command::new("sh")
                .current_dir(&vcpkg)
                .args(["bootstrap-vcpkg.sh", "-disableMetrics"]))?;
        }
        fs::write(&tool_stamp, &revision).map_err(|e| e.to_string())?;
    }
    let host = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x64-linux-phaos",
        ("aarch64", "linux") => "arm64-linux-phaos",
        ("x86_64", "windows") => "x64-windows-phaos",
        _ => {
            return Err(
                "native-deps supports Linux x86-64/AArch64 and Windows x64 MSVC hosts".into(),
            );
        }
    };
    let target = host;
    let mut cmd = Command::new(vcpkg.join(if cfg!(windows) { "vcpkg.exe" } else { "vcpkg" }));
    cmd.env_remove("VCPKG_ROOT");
    cmd.current_dir(&root).arg("install");
    if cfg!(windows) {
        // Several ports share MSYS packages. Keep downloads so each port does
        // not fetch the same build tools again.
        cmd.args([
            "--clean-buildtrees-after-build",
            "--clean-packages-after-build",
        ]);
    } else {
        cmd.arg("--clean-after-build");
    }
    cmd.arg(format!("--triplet={target}"))
        .arg(format!("--host-triplet={host}"))
        .arg(format!(
            "--x-install-root={}",
            root.join("target/native/installed").display()
        ));
    if server {
        cmd.arg("--x-no-default-features");
    }
    run(&mut cmd)?;
    if config::fingerprint(&root) != expected_inputs {
        return Err(
            "native dependency inputs changed during installation; rerun `cargo xtask native-deps`"
                .into(),
        );
    }
    let prefix = config::prefix(&root, target);
    fs::write(prefix.join(".phaos-build-id"), expected_inputs).map_err(|e| e.to_string())?;
    println!("Native dependencies ready in {}", prefix.display());
    Ok(())
}
