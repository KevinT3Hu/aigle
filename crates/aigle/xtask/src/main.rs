//! Repository tasks.
//!
//! * `cargo xtask appimage` builds a Linux AppImage with its shared runtime libraries.
//! * `cargo xtask native-deps [--server]` installs pinned native libraries with vcpkg.
//! * `cargo xtask kernels [--check]` rebuilds the committed SPIR-V (doc 04 Q28) by running
//!   `crates/aigle/kernels-build` with its own pinned nightly.
//! * `cargo xtask data --update | --check` refreshes / verifies the bundled third-party data
//!   in `data/` (camconst, lensfun database) against
//!   `data/manifest.toml`, which records each file's upstream, revision, licence and hash.
//!   `--update` needs `git`, `curl`, `tar` and network access; `--check` is offline. Both
//!   use `sha256sum`.

mod appimage;
mod native;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "xtask",
    bin_name = "cargo xtask",
    about = "Repository build and maintenance tasks"
)]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build a Linux AppImage with its shared runtime libraries.
    Appimage(appimage::Options),
    /// Install pinned native libraries with vcpkg.
    NativeDeps {
        /// Install only the native dependencies needed by Thesauros.
        #[arg(long)]
        server: bool,
    },
    /// Rebuild the committed SPIR-V with the pinned nightly toolchain.
    Kernels {
        /// Fail if rebuilding changes the committed module.
        #[arg(long)]
        check: bool,
    },
    /// Update or verify the bundled third-party datasets.
    Data(DataOptions),
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct DataOptions {
    /// Refresh bundled datasets and their manifest (requires network access).
    #[arg(long)]
    update: bool,
    /// Verify bundled files against the manifest without network access.
    #[arg(long)]
    check: bool,
}

fn main() -> ExitCode {
    let res = match Cli::parse().command {
        Task::Appimage(options) => appimage::build(options),
        Task::NativeDeps { server } => native::install(server),
        Task::Kernels { check } => return kernels(check),
        Task::Data(options) => {
            if options.update {
                data_update()
            } else {
                data_check()
            }
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn kernels(check: bool) -> ExitCode {
    let dir = root().join("crates/aigle/kernels-build");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&dir)
        .args(["run", "--release", "-p", "aigle-kernels-build", "--"]);
    if check {
        cmd.arg("--check");
    }
    // Let rustup pick the toolchain from crates/aigle/kernels-build/rust-toolchain.toml instead of
    // inheriting the stable toolchain this xtask runs under.
    for var in [
        "RUSTUP_TOOLCHAIN",
        "CARGO",
        "RUSTC",
        "RUSTDOC",
        "CARGO_TARGET_DIR",
        "RUSTFLAGS",
    ] {
        cmd.env_remove(var);
    }
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("failed to run cargo in {}: {e}", dir.display());
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bundled data

/// One bundled upstream dataset.
struct Source {
    name: &'static str,
    description: &'static str,
    /// Project home.
    upstream: &'static str,
    license: &'static str,
    attribution: &'static str,
    /// Directory under `data/`.
    dir: &'static str,
    fetch: Fetch,
}

enum Fetch {
    /// One file from a git repository, pinned to the commit `HEAD` resolves to.
    GitFile {
        repo: &'static str,
        path: &'static str,
        raw: &'static str,
        out: &'static str,
    },
    /// The lensfun update server's version-1 tarball, pinned to its timestamp.
    LensfunV1,
}

const SOURCES: &[Source] = &[
    Source {
        name: "camconst",
        description: "Per-camera white levels (A2); only ranges.white / white_max / aperture_scaling are used",
        upstream: "https://github.com/Beep6581/RawTherapee",
        license: "GPL-3.0-or-later",
        attribution: "RawTherapee contributors",
        dir: "camconst",
        fetch: Fetch::GitFile {
            repo: "https://github.com/Beep6581/RawTherapee.git",
            path: "rtengine/camconst.json",
            raw: "https://raw.githubusercontent.com/Beep6581/RawTherapee",
            out: "camconst.json",
        },
    },
    Source {
        name: "lensfun",
        description: "Lens database, XML format version 1 (B1 lens correction)",
        upstream: "https://github.com/lensfun/lensfun (data/db), distributed via https://lensfun.github.io/db/",
        license: "CC-BY-SA-3.0",
        attribution: "the lensfun project and its contributors",
        dir: "lensfun/version_1",
        fetch: Fetch::LensfunV1,
    },
];

const MANIFEST: &str = "manifest.toml";

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

fn curl(url: &str, out: &Path) -> Result<(), String> {
    run(Command::new("curl")
        .args(["-sSfL", "--retry", "3", "-o"])
        .arg(out)
        .arg(url))
    .map(drop)
}

/// Fetch one source into `data/<dir>`; returns the pinned revision.
fn fetch(data: &Path, s: &Source) -> Result<String, String> {
    let dir = data.join(s.dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match &s.fetch {
        Fetch::GitFile {
            repo,
            path,
            raw,
            out,
        } => {
            let head = run(Command::new("git").args(["ls-remote", repo, "HEAD"]))?;
            let commit = String::from_utf8_lossy(&head)
                .split_whitespace()
                .next()
                .filter(|c| c.len() == 40)
                .ok_or(format!("{repo}: no HEAD commit"))?
                .to_string();
            curl(&format!("{raw}/{commit}/{path}"), &dir.join(out))?;
            Ok(format!("{commit} ({path})"))
        }
        Fetch::LensfunV1 => {
            let base = "https://lensfun.github.io/db";
            let v = run(Command::new("curl").args(["-sSfL", &format!("{base}/versions.json")]))?;
            // `[timestamp, [formats…], […]]`
            let v = String::from_utf8_lossy(&v);
            let ts: u64 = v
                .trim_start_matches(|c: char| c == '[' || c.is_whitespace())
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|t| t.parse().ok())
                .ok_or(format!("unexpected versions.json: {v}"))?;
            let tar = dir.join("version_1.tar.bz2");
            curl(&format!("{base}/version_1.tar.bz2"), &tar)?;
            run(Command::new("tar")
                .arg("-xjf")
                .arg(&tar)
                .arg("-C")
                .arg(&dir))?;
            std::fs::remove_file(&tar).map_err(|e| e.to_string())?;
            Ok(format!("database timestamp {ts} (version_1.tar.bz2)"))
        }
    }
}

/// Files under `dir`, relative to `base`, sorted.
fn files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(base, &p, out)?;
        } else {
            let rel = p.strip_prefix(base).expect("under base");
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn sha256(data: &Path, rel: &[String]) -> Result<Vec<(String, String)>, String> {
    if rel.is_empty() {
        return Ok(Vec::new());
    }
    let out = run(Command::new("sha256sum").current_dir(data).args(rel))?;
    let text = String::from_utf8_lossy(&out);
    let v: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| {
            let (h, f) = l.split_once("  ")?;
            Some((f.to_string(), h.to_string()))
        })
        .collect();
    if v.len() != rel.len() {
        return Err("sha256sum output does not match the file list".into());
    }
    Ok(v)
}

fn data_files(data: &Path) -> Result<Vec<String>, String> {
    let mut all = Vec::new();
    for s in SOURCES {
        files(data, &data.join(s.dir), &mut all)?;
    }
    Ok(all)
}

fn data_update() -> Result<(), String> {
    let data = root().join("data");
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let mut m = String::from(
        "# Bundled third-party data: upstream, pinned revision, licence and SHA-256 of every file.\n\
         # Generated by `cargo xtask data --update`; verified by `cargo xtask data --check`.\n\
         # Do not edit. The files are read at runtime (not compiled in); see\n\
         # crates/aigle/engine/src/data.rs for the lookup order.\n",
    );
    for s in SOURCES {
        eprintln!("fetching {} …", s.name);
        let rev = fetch(&data, s)?;
        eprintln!("  {rev}");
        m.push_str(&format!(
            "\n[[source]]\nname = {:?}\ndescription = {:?}\nupstream = {:?}\nrevision = {:?}\nlicense = {:?}\nattribution = {:?}\ndir = {:?}\n",
            s.name, s.description, s.upstream, rev, s.license, s.attribution, s.dir
        ));
    }
    let all = data_files(&data)?;
    m.push_str("\n[files]\n");
    for (f, h) in sha256(&data, &all)? {
        m.push_str(&format!("{f:?} = {h:?}\n"));
    }
    std::fs::write(data.join(MANIFEST), m).map_err(|e| e.to_string())?;
    eprintln!("wrote data/{MANIFEST} ({} files)", all.len());
    Ok(())
}

fn data_check() -> Result<(), String> {
    let data = root().join("data");
    let text = std::fs::read_to_string(data.join(MANIFEST))
        .map_err(|e| format!("data/{MANIFEST}: {e}; run `cargo xtask data --update`"))?;
    let mut want: Vec<(String, String)> = text
        .split_once("\n[files]\n")
        .ok_or("manifest has no [files] table")?
        .1
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(" = ")?;
            Some((
                k.trim_matches('"').to_string(),
                v.trim_matches('"').to_string(),
            ))
        })
        .collect();
    want.sort();
    let all = data_files(&data)?;
    let mut have = sha256(&data, &all)?;
    have.sort();
    if have == want {
        println!(
            "bundled data matches data/{MANIFEST} ({} files)",
            have.len()
        );
        return Ok(());
    }
    let mut msg = String::from("bundled data does not match the manifest:\n");
    for (f, h) in &want {
        match have.iter().find(|(g, _)| g == f) {
            None => msg.push_str(&format!("  missing  {f}\n")),
            Some((_, g)) if g != h => msg.push_str(&format!("  modified {f}\n")),
            _ => {}
        }
    }
    for (f, _) in &have {
        if !want.iter().any(|(g, _)| g == f) {
            msg.push_str(&format!("  extra    {f}\n"));
        }
    }
    msg.push_str("run `cargo xtask data --update` to refresh it from upstream");
    Err(msg)
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definitions_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn data_requires_exactly_one_operation() {
        for args in [
            vec!["xtask", "data"],
            vec!["xtask", "data", "--update", "--check"],
            vec!["xtask", "data", "--check", "extra"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        for flag in ["--check", "--update"] {
            assert!(Cli::try_parse_from(["xtask", "data", flag]).is_ok());
        }
    }

    #[test]
    fn parses_task_options_and_rejects_invalid_flags() {
        let cli = Cli::try_parse_from(["xtask", "native-deps", "--server"]).unwrap();
        assert!(matches!(cli.command, Task::NativeDeps { server: true }));
        let cli = Cli::try_parse_from(["xtask", "kernels", "--check"]).unwrap();
        assert!(matches!(cli.command, Task::Kernels { check: true }));
        assert!(Cli::try_parse_from(["xtask", "kernels", "--typo"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "appimage", "--output"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "appimage", "--unknown"]).is_err());
        assert!(
            Cli::try_parse_from([
                "xtask",
                "appimage",
                "--no-build",
                "--output",
                "output with spaces.AppImage",
                "--appimagetool",
                "/tmp/tool"
            ])
            .is_ok()
        );
    }
}
