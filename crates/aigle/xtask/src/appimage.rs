//! Linux AppDir assembly and AppImage generation. Image codecs are already static.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(clap::Args)]
pub(crate) struct Options {
    /// Package the existing release binary without rebuilding it.
    #[arg(long)]
    no_build: bool,
    /// Write the AppImage to this path instead of the default target directory.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
    /// Use this appimagetool executable instead of searching PATH.
    #[arg(long = "appimagetool", value_name = "PATH")]
    tool: Option<PathBuf>,
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed: {status}"))
    }
}
fn mkdir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
}
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    mkdir(to)?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else {
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}
fn executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// glibc and its loader must come from the host as a matching set. GPU drivers/loaders
// are host-provided too; the application loads Vulkan dynamically.
fn host_library(name: &str) -> bool {
    [
        "libc.so.",
        "libm.so.",
        "libmvec.so.",
        "libpthread.so.",
        "libdl.so.",
        "librt.so.",
        "libresolv.so.",
        "libutil.so.",
        "ld-linux",
        "libvulkan.so.",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
fn libraries(ldd: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let mut result = Vec::new();
    for line in ldd.lines() {
        let Some((name, resolved)) = line.trim().split_once(" => ") else {
            continue;
        };
        if resolved.starts_with("not found") {
            return Err(format!("Missing runtime library: {name}"));
        }
        let path = resolved
            .rsplit_once(" (")
            .map_or(resolved, |(p, _)| p)
            .trim();
        if !Path::new(path).is_absolute() {
            return Err(format!("Unexpected ldd output: {line}"));
        }
        if !host_library(name) {
            result.push((name.into(), path.into()));
        }
    }
    Ok(result)
}

const INSTALL_TOOL: &str = "Install appimagetool from https://github.com/AppImage/appimagetool/releases and make it executable. Put it on PATH, set APPIMAGETOOL, or pass --appimagetool /path/to/appimagetool. Xtask does not download packaging tools.";

fn resolve_tool(
    explicit: Option<PathBuf>,
    search_path: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    fn is_executable(path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
    let candidate = if let Some(path) = explicit {
        if !is_executable(&path) {
            return Err(format!(
                "appimagetool is missing or not executable: {}.\n{INSTALL_TOOL}",
                path.display()
            ));
        }
        path
    } else {
        search_path
            .as_ref()
            .and_then(|paths| {
                std::env::split_paths(paths)
                    .map(|dir| dir.join("appimagetool"))
                    .find(|path| is_executable(path))
            })
            .ok_or_else(|| format!("appimagetool was not found on PATH.\n{INSTALL_TOOL}"))?
    };
    fs::canonicalize(&candidate).map_err(|e| format!("{}: {e}", candidate.display()))
}

pub fn build(opts: Options) -> Result<(), String> {
    if std::env::consts::OS != "linux" {
        return Err("AppImages must be built on Linux".into());
    }
    let arch = std::env::consts::ARCH;
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(format!("Unsupported AppImage architecture: {arch}"));
    }
    // Check the user-installed packager before building or touching the AppDir.
    let tool = resolve_tool(
        opts.tool
            .or_else(|| std::env::var_os("APPIMAGETOOL").map(PathBuf::from)),
        std::env::var_os("PATH"),
    )?;
    let root = super::root().canonicalize().map_err(|e| e.to_string())?;
    let metadata = super::run(Command::new("cargo").current_dir(&root).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
    ]))?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata).map_err(|e| e.to_string())?;
    let target = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .ok_or("Missing Cargo target directory")?,
    );
    let version = metadata["packages"]
        .as_array()
        .ok_or("Missing workspace packages")?
        .iter()
        .find(|p| p["name"] == "aigle-app")
        .and_then(|p| p["version"].as_str())
        .ok_or("Missing Aigle version")?;
    if !opts.no_build {
        run(Command::new("cargo").current_dir(&root).args([
            "build",
            "--release",
            "--locked",
            "-p",
            "aigle-app",
            "--bin",
            "aigle-gui",
        ]))?;
    }
    let binary = target.join("release/aigle-gui");
    if !binary.is_file() {
        return Err(format!(
            "{} missing; run without --no-build",
            binary.display()
        ));
    }
    let work = target.join("appimage");
    let appdir = work.join("Aigle.AppDir");
    let output = opts
        .output
        .unwrap_or_else(|| work.join(format!("Aigle-{version}-{arch}.AppImage")));
    let output = if output.is_absolute() {
        output
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(output)
    };
    if output.starts_with(&appdir) {
        return Err("Output must be outside the AppDir".into());
    }
    if appdir.exists() {
        fs::remove_dir_all(&appdir).map_err(|e| e.to_string())?;
    }
    let bin = appdir.join("usr/bin");
    let lib = appdir.join("usr/lib");
    mkdir(&bin)?;
    mkdir(&lib)?;
    mkdir(output.parent().ok_or("Output needs a parent directory")?)?;
    copy(&binary, &bin.join("aigle-gui"))?;
    executable(&bin.join("aigle-gui"))?;
    let ldd = super::run(Command::new("ldd").env("LC_ALL", "C").arg(&binary))?;
    let deps = libraries(&String::from_utf8_lossy(&ldd))?;
    for (name, source) in &deps {
        copy(source, &lib.join(name))?;
    }
    copy_tree(&root.join("data"), &appdir.join("usr/share/aigle"))?;
    let assets = root.join("crates/aigle/xtask/assets");
    for name in ["AppRun", "aigle.desktop", "aigle.svg"] {
        copy(&assets.join(name), &appdir.join(name))?;
    }
    executable(&appdir.join("AppRun"))?;
    copy(&appdir.join("aigle.svg"), &appdir.join(".DirIcon"))?;
    let applications = appdir.join("usr/share/applications");
    let icons = appdir.join("usr/share/icons/hicolor/scalable/apps");
    mkdir(&applications)?;
    mkdir(&icons)?;
    copy(
        &assets.join("aigle.desktop"),
        &applications.join("aigle.desktop"),
    )?;
    copy(&assets.join("aigle.svg"), &icons.join("aigle.svg"))?;
    // Include the managed native stack's copyright notices, plus the data manifest
    // and dataset licences already copied with data/.
    let triplet = if arch == "x86_64" {
        "x64-linux-phaos"
    } else {
        "arm64-linux-phaos"
    };
    let share = root
        .join("target/native/installed")
        .join(triplet)
        .join("share");
    let licenses = appdir.join("usr/share/doc/aigle/native-licenses");
    mkdir(&licenses)?;
    for entry in fs::read_dir(&share)
        .map_err(|e| format!("{}: {e}; run cargo xtask native-deps", share.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().join("copyright").is_file() {
            copy(
                &entry.path().join("copyright"),
                &licenses.join(entry.file_name()),
            )?;
        }
    }
    println!(
        "Bundled {} shared libraries in {}",
        deps.len(),
        appdir.display()
    );
    // The packaging tool can run without mounting itself through FUSE.
    run(Command::new(tool)
        .env("APPIMAGE_EXTRACT_AND_RUN", "1")
        .env("ARCH", arch)
        .args(["--no-appstream", "--comp", "zstd"])
        .arg(&appdir)
        .arg(&output))?;
    executable(&output)?;
    println!("AppImage: {}", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packager_must_be_installed_and_executable() {
        let dir = std::env::temp_dir().join(format!("aigle-tool-lookup-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("appimagetool");
        assert!(
            resolve_tool(None, Some(dir.clone().into_os_string()))
                .unwrap_err()
                .contains(INSTALL_TOOL)
        );
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        assert!(resolve_tool(Some(path.clone()), None).is_err());
        executable(&path).unwrap();
        let expected = path.canonicalize().unwrap();
        assert_eq!(
            resolve_tool(None, Some(dir.clone().into_os_string())).unwrap(),
            expected
        );
        assert_eq!(resolve_tool(Some(path), None).unwrap(), expected);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resolves_libraries_and_rejects_missing_dependencies() {
        let deps = libraries("linux-vdso.so.1 (0x1)\nlibc.so.6 => /lib/libc.so.6 (0x2)\nlibstdc++.so.6 => /usr/lib/libstdc++.so.6 (0x3)\n/lib64/ld-linux-x86-64.so.2 (0x4)\n").unwrap();
        assert_eq!(
            deps,
            vec![(
                "libstdc++.so.6".into(),
                PathBuf::from("/usr/lib/libstdc++.so.6")
            )]
        );
        assert!(libraries("libxcb.so.1 => not found").is_err());
        assert!(!host_library("libcrypt.so.1"));
    }
}
