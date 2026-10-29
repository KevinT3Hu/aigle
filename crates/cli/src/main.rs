//! `aigle`: the command line (doc 01 §8). `aigle export` renders photos headless with one
//! set of export settings. Photos are pipelined (doc 05 §10): up to `-j` exports are in
//! flight, and the engine's memory admission decides how many decode at once, so decoding
//! the next photo overlaps rendering and encoding the current ones.
//!
//! Stateless: no catalog, no disk cache; sidecars are only read (`--sidecar`). It writes
//! the output files and, for DNG, the output's sidecar holding the edits.

mod args;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aigle_core::EditParams;
use aigle_engine_api::{
    BackendPreference, CancelToken, EncodedFormat, Engine, EngineConfig, ExportOptions, JobError,
    ProcessKind, SourceRef, is_photo_path,
};
use serde::Serialize;

use args::{Command, Export};

fn main() {
    if aigle_isolate::is_worker() {
        aigle_isolate::serve(&[aigle_engine::WORKER_OPS, aigle_library::WORKER_OPS].concat());
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("AIGLE_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let code = match args::parse(std::env::args().skip(1)) {
        Ok(Command::Help) => {
            print!("{}", args::HELP);
            0
        }
        Ok(Command::Version) => {
            println!("aigle {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(Command::Export(e)) => match export(&e) {
            Ok(code) => code,
            Err(msg) => {
                eprintln!("aigle: {msg}");
                2
            }
        },
        Err(msg) => {
            eprintln!("aigle: {msg}\nTry `aigle --help`.");
            2
        }
    };
    std::process::exit(code);
}

/// `{name}` and `{seq}` (zero-padded to the width of `total`) in `template`; path
/// separators become `_`, an empty result falls back to the photo's name.
fn render_template(template: &str, photo: &Path, seq: usize, total: usize) -> String {
    let stem = photo
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    let width = total.max(1).to_string().len();
    let out = template
        .replace("{name}", &stem)
        .replace("{seq}", &format!("{seq:0width$}"))
        .replace(['/', '\\', '\0'], "_");
    match out.trim() {
        "" | "." | ".." => stem,
        s => s.to_string(),
    }
}

/// The photos named on the command line, folders expanded (not recursively).
fn photos(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for p in inputs {
        if p.is_dir() {
            let found = aigle_library::scan(p).map_err(|e| format!("{}: {e}", p.display()))?;
            if found.is_empty() {
                eprintln!("aigle: {}: no photos", p.display());
            }
            out.extend(found.into_iter().map(|e| e.path));
        } else if p.is_file() {
            if !is_photo_path(p) {
                return Err(format!("{}: not a photo aigle can read", p.display()));
            }
            out.push(p.clone());
        } else {
            return Err(format!("{}: no such file or folder", p.display()));
        }
    }
    Ok(out)
}

/// Destination of each photo: the template's name in `dir`, numbered `-2`, `-3`, … where it
/// would collide with another photo of the batch or (unless `overwrite`) an existing file.
fn destinations(photos: &[PathBuf], e: &Export, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let ext = e.format.extension();
    let mut taken = HashSet::new();
    photos
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let stem = render_template(&e.template, p, i + 1, photos.len());
            let path = std::iter::once(e.output.join(format!("{stem}.{ext}")))
                .chain((2..).map(|n| e.output.join(format!("{stem}-{n}.{ext}"))))
                .find(|c| !taken.contains(c) && (e.overwrite || !exists(c)))
                .expect("unbounded");
            taken.insert(path.clone());
            path
        })
        .collect()
}

#[derive(Serialize)]
struct Outcome {
    input: PathBuf,
    output: PathBuf,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    seconds: f64,
}

#[derive(Serialize)]
struct Summary {
    exported: usize,
    failed: usize,
    interrupted: bool,
    backend: String,
    seconds: f64,
    photos: Vec<Outcome>,
}

fn load_params(path: &Path) -> Result<EditParams, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut p: EditParams =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    p.sanitize()
        .map_err(|e| format!("{}: invalid params: {e}", path.display()))?;
    Ok(p)
}

/// The edits for `photo`: its sidecar's with `--sidecar`, else `base`.
fn edits_for(photo: &Path, e: &Export, base: &Arc<EditParams>) -> Result<Arc<EditParams>, String> {
    if !e.sidecar {
        return Ok(base.clone());
    }
    let loaded = aigle_library::Store::new()
        .read(photo)
        .map_err(|err| format!("sidecar: {err}"))?;
    for w in &loaded.warnings {
        eprintln!("aigle: {}: {w}", photo.display());
    }
    Ok(loaded.meta.edits.map_or_else(|| base.clone(), Arc::new))
}

fn export(e: &Export) -> Result<i32, String> {
    let photos = photos(&e.inputs)?;
    if photos.is_empty() {
        return Err("no photos to export".into());
    }
    if !aigle_engine::format_available(e.format) {
        return Err(format!(
            "this system's libvips cannot write {} files",
            e.format.extension()
        ));
    }
    let base = Arc::new(match &e.params {
        Some(p) => load_params(p)?,
        None => EditParams::default(),
    });
    std::fs::create_dir_all(&e.output).map_err(|err| format!("{}: {err}", e.output.display()))?;
    let outputs = destinations(&photos, e, Path::exists);

    aigle_isolate::enable("/proc/self/exe".into(), Vec::new());
    let engine = aigle_engine::create(EngineConfig {
        process: ProcessKind::Cli,
        memory_budget: e.memory_mib.map(|m| m << 20),
        backend: if e.cpu {
            BackendPreference::Cpu
        } else {
            BackendPreference::Auto
        },
        ..Default::default()
    });
    let (kind, why) = engine.backend();
    let backend = format!("{kind:?}: {why}");
    tracing::info!("backend {backend}");

    let stop = Arc::new(AtomicBool::new(false));
    let running: Arc<Mutex<Vec<(u64, CancelToken)>>> = Default::default();
    watch_signals(stop.clone(), running.clone());

    let total = photos.len();
    let threads = e
        .jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()))
        .min(total);
    let queue: Mutex<VecDeque<(PathBuf, PathBuf)>> =
        Mutex::new(photos.into_iter().zip(outputs).collect());
    let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::with_capacity(total));
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Some((input, output)) = queue.lock().expect("queue").pop_front() else {
                        return;
                    };
                    let started = Instant::now();
                    let r = export_one(&engine, e, &base, &input, &output, &running);
                    let interrupted = matches!(r, Err(ref m) if m == CANCELLED);
                    let mut res = results.lock().expect("results");
                    let outcome = Outcome {
                        ok: r.is_ok(),
                        width: r.as_ref().ok().map(|s| s.0),
                        height: r.as_ref().ok().map(|s| s.1),
                        error: r.err(),
                        seconds: started.elapsed().as_secs_f64(),
                        input,
                        output,
                    };
                    if !interrupted {
                        progress(&outcome, res.len() + 1, total);
                    }
                    res.push(outcome);
                }
            });
        }
    });
    drop(engine);

    let photos = results.into_inner().expect("results");
    let interrupted = stop.load(Ordering::SeqCst);
    let exported = photos.iter().filter(|o| o.ok).count();
    let summary = Summary {
        exported,
        failed: photos.len() - exported,
        interrupted,
        backend,
        seconds: t0.elapsed().as_secs_f64(),
        photos,
    };
    if e.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("serialisable")
        );
    }
    eprintln!(
        "Exported {} of {total} photos to {} in {:.1} s{}",
        summary.exported,
        e.output.display(),
        summary.seconds,
        match (summary.failed, interrupted) {
            (_, true) => "; interrupted".to_string(),
            (0, _) => String::new(),
            (n, _) => format!("; {n} failed"),
        }
    );
    Ok(if interrupted {
        130
    } else if summary.failed > 0 {
        1
    } else {
        0
    })
}

const CANCELLED: &str = "cancelled";

fn export_one(
    engine: &Engine,
    e: &Export,
    base: &Arc<EditParams>,
    input: &Path,
    output: &Path,
    running: &Mutex<Vec<(u64, CancelToken)>>,
) -> Result<(u32, u32), String> {
    let params = edits_for(input, e, base)?;
    let job = engine.export(
        SourceRef::File(input.to_path_buf()),
        params.clone(),
        ExportOptions {
            path: output.to_path_buf(),
            format: e.format,
            long_edge: e.long_edge,
            output: e.color_space.clone(),
            sharpening: e.sharpening,
            metadata: e.metadata,
            artist: e.artist.clone(),
            copyright: e.copyright.clone(),
            watermark: e.watermark.clone(),
        },
    );
    let token = job.cancel_token().clone();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    running.lock().expect("running").push((id, token));
    let r = job.wait();
    running.lock().expect("running").retain(|(i, _)| *i != id);
    let done = r.map_err(|err| match err {
        JobError::Cancelled => CANCELLED.to_string(),
        other => other.to_string(),
    })?;
    if e.format == EncodedFormat::Dng && *params != EditParams::default() {
        aigle_library::write_dng_sidecar(Some(input), output, &params)
            .map_err(|err| format!("the DNG was written, but its edits were not: {err}"))?;
    }
    Ok((done.width, done.height))
}

fn progress(o: &Outcome, n: usize, total: usize) {
    let width = total.to_string().len();
    match (&o.error, o.width, o.height) {
        (None, Some(w), Some(h)) => eprintln!(
            "[{n:>width$}/{total}] {} -> {} ({w}×{h}, {:.1} s)",
            o.input.display(),
            o.output.display(),
            o.seconds
        ),
        (err, _, _) => eprintln!(
            "[{n:>width$}/{total}] {} failed: {}",
            o.input.display(),
            err.as_deref().unwrap_or("unknown error")
        ),
    }
}

/// First Ctrl-C / SIGTERM: stop taking photos and cancel the running exports (their
/// temporary files are removed). A second one exits at once.
fn watch_signals(stop: Arc<AtomicBool>, running: Arc<Mutex<Vec<(u64, CancelToken)>>>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) else {
        return;
    };
    std::thread::spawn(move || {
        for _ in signals.forever() {
            if stop.swap(true, Ordering::SeqCst) {
                std::process::exit(130);
            }
            eprintln!("aigle: interrupted; stopping (press Ctrl-C again to quit at once)");
            for (_, t) in running.lock().expect("running").iter() {
                t.cancel();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(template: &str, overwrite: bool) -> Export {
        match args::parse(
            ["export", "x", "-o", "/out", "--name", template]
                .into_iter()
                .map(String::from),
        )
        .unwrap()
        {
            Command::Export(mut e) => {
                e.overwrite = overwrite;
                *e
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn destinations_never_collide() {
        let photos = [
            PathBuf::from("/in/a.nef"),
            PathBuf::from("/in2/a.cr3"),
            PathBuf::from("/in/b.raf"),
        ];
        let exists = |p: &Path| p == Path::new("/out/b.jpg");
        let d = destinations(&photos, &opts("{name}", false), exists);
        assert_eq!(
            d,
            [
                PathBuf::from("/out/a.jpg"),
                PathBuf::from("/out/a-2.jpg"),
                PathBuf::from("/out/b-2.jpg"),
            ]
        );
        let d = destinations(&photos, &opts("{name}", true), exists);
        assert_eq!(d[2], PathBuf::from("/out/b.jpg"));
        let d = destinations(&photos, &opts("trip-{seq}", false), exists);
        assert_eq!(d[0], PathBuf::from("/out/trip-1.jpg"));
        assert_eq!(render_template("x/{name}", Path::new("p.nef"), 1, 1), "x_p");
    }
}
