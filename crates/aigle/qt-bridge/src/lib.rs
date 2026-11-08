//! Narrow, panic-contained C boundary. The Qt worker owns all mutable editor state;
//! retained frames are immutable Arcs and never borrow the editor.
mod file_actions;
mod display;
mod platform;
mod credentials;
mod mask_edit;
mod retouch_edit;
mod sync_edits;
mod library_targets;

use aigle_core::{
    EditParams, MaskId,
    geometry::{Point, Rect},
    masks::{LocalAdjustment, LocalParams, MaskShape, Pt, Stroke},
    retouch::{RedEye, Spot, SpotMode},
};
use aigle_doc::{EditCommand, GestureId, ParamOp, PhotoDoc};
use aigle_engine_api::*;
use aigle_library::{Catalog, PhotoKey, PhotoMeta, Store};
use anyhow::{Context, Result, bail, ensure};
use futures::FutureExt;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::Arc,
};

pub struct AigleEditor {
    comparisons: Vec<Photo>,
    data_dir: PathBuf,
    engine: Engine,
    catalog: Catalog,
    store: Store,
    photo: Option<Photo>,
    export: Option<Job<ExportResult>>,
    export_result: Value,
    import: Option<ImportTask>,
    duplicates: Option<ImportTask>,
    agent: Option<AgentTask>,
    model_discovery: Option<ModelDiscovery>,
    display_detection: Option<display::Detection>,
    comparison_display_detection: Option<display::Detection>,
}
struct ModelDiscovery {
    result: std::sync::mpsc::Receiver<Result<Vec<String>, String>>,
    state: Value,
}
struct AgentTask {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    events: futures::channel::mpsc::UnboundedReceiver<aigle_agent::AgentEvent>,
    result: std::sync::mpsc::Receiver<Result<(EditParams, String), String>>,
    base: EditParams,
    base_rev: aigle_core::Revision,
    candidate: Option<EditParams>,
    state: Value,
}
impl Drop for AgentTask {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
struct ImportTask {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    status: Arc<std::sync::Mutex<Value>>,
}
impl Drop for ImportTask {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
impl ImportTask {
    fn status(&self) -> Value {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
struct RetouchPreview { session: String, base_rev: aigle_core::Revision }
struct Photo {
    identity: String,
    document_id: u64,
    retouch_preview: Option<RetouchPreview>,
    source: SourceRef,
    session: Session,
    doc: PhotoDoc,
    meta: PhotoMeta,
    info: watch::Receiver<LoadState>,
    view: View,
    before: View,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    before_frames: watch::Receiver<Option<Arc<Frame>>>,
    histogram: watch::Receiver<Option<Arc<Histogram>>>,
    dirty: bool,
    saved_rev: aigle_core::Revision,
    uncropped: bool,
    crop_preview: Option<EditParams>,
    crop_anchor: Option<(Option<GestureId>, aigle_core::params::CropRect)>,
    crop_preview_revision: u64,
    copy: Option<i64>,
}
impl Photo {
    fn apply(&mut self, cmd: EditCommand, hint: Interaction) -> Result<()> {
        let cmd = self.constrain_crop(cmd);
        self.doc.apply(cmd, hint)?;
        Ok(())
    }

    // Keep geometry adjustment and its constrained crop in the same undo command.
    fn constrain_crop(&mut self, mut cmd: EditCommand) -> EditCommand {
        let touches_crop = cmd.ops.iter().any(|op| match op {
            ParamOp::Set { path, .. } | ParamOp::Reset { path } | ParamOp::Add { path, .. } =>
                path.as_str().starts_with("geometry.crop") || path.as_str() == "geometry",
            ParamOp::ReplaceAll { .. } => true,
            _ => false,
        });
        if touches_crop {
            self.crop_anchor = None;
            return cmd;
        }
        let Ok(next) = self.doc.preview(&cmd.ops) else { return cmd };
        let head = &self.doc.head().geometry;
        let geometry = &next.geometry;
        if geometry.scalars == head.scalars && geometry.orientation_delta == head.orientation_delta {
            return cmd;
        }
        let anchor = match &self.crop_anchor {
            Some((gesture, crop)) if gesture.is_some() && *gesture == cmd.gesture => crop.clone(),
            _ => head.crop.clone(),
        };
        let LoadState::Ready(info) = self.info.borrow().clone() else { return cmd };
        let size = aigle_engine::geometry_map(&info, &next).source_size();
        let frame = aigle_core::crop::Frame::new(size, geometry);
        let fitted = frame.crop_of(frame.shrink_to_fit(frame.rect_of(&anchor)));
        self.crop_anchor = Some((cmd.gesture, anchor));
        if fitted != geometry.crop {
            cmd.ops.push(ParamOp::set("geometry.crop", json!(fitted)));
        }
        cmd
    }
}
pub struct AigleFrame(Arc<Frame>);
#[repr(C)]
#[derive(Default)]
pub struct AiglePixels {
    data: *const u8,
    length: usize,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
    revision: u64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}
fn cstring(value: Value) -> *mut c_char {
    CString::new(value.to_string())
        .expect("JSON escapes NUL")
        .into_raw()
}
fn guarded<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}
unsafe fn text<'a>(p: *const c_char) -> Result<&'a str> {
    ensure!(!p.is_null(), "null string");
    // SAFETY: caller promises a live NUL-terminated string for this call.
    Ok(unsafe { CStr::from_ptr(p) }.to_str()?)
}
#[unsafe(no_mangle)]
pub extern "C" fn aigle_abi_version() -> u32 {
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn aigle_initialize() {
    if aigle_isolate::is_worker() {
        aigle_isolate::serve(&[aigle_engine::WORKER_OPS, aigle_library::WORKER_OPS].concat());
    }
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("AIGLE_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .try_init();
    if let Ok(exe) = std::env::current_exe() {
        aigle_isolate::enable(exe, vec![]);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_editor_new(
    config: *const c_char,
    error: *mut *mut c_char,
) -> *mut AigleEditor {
    if !error.is_null() {
        unsafe {
            *error = ptr::null_mut();
        }
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<AigleEditor> {
        let config: Value = serde_json::from_str(unsafe { text(config)? })?;
        let data = PathBuf::from(
            config["data_dir"]
                .as_str()
                .context("data_dir is required")?,
        );
        std::fs::create_dir_all(&data)?;
        let cfg = if config["engine"].is_object() {
            serde_json::from_value(config["engine"].clone())?
        } else {
            EngineConfig::default()
        };
        Ok(AigleEditor {
            comparisons: vec![],
            data_dir: data.clone(),
            engine: aigle_engine::create(cfg),
            catalog: Catalog::open(&data.join("catalog.sqlite3"))?,
            store: Store::new(),
            photo: None,
            export: None,
            export_result: Value::Null,
            import: None,
            duplicates: None,
            agent: None,
            model_discovery: None,
            display_detection: None,
            comparison_display_detection: None,
        })
    }));
    match result {
        Ok(Ok(editor)) => Box::into_raw(Box::new(editor)),
        other => {
            let message = match other {
                Ok(Err(e)) => format!("{e:#}"),
                _ => "Rust engine initialization panicked".into(),
            };
            if !error.is_null() {
                unsafe {
                    *error = cstring(json!({"error": message}));
                }
            }
            ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_editor_free(editor: *mut AigleEditor) {
    if !editor.is_null() {
        guarded((), || drop(unsafe { Box::from_raw(editor) }));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_string_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}
/// Stateless crop geometry query; safe to call independently of an editor thread.
/// The caller owns the returned JSON string, including errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_crop_geometry(request: *const c_char) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Value> {
        use aigle_core::crop::{Frame, Handle, drag};
        use aigle_core::params::GeometryParams;
        let r: Value = serde_json::from_str(unsafe { text(request)? })?;
        let w = r["width"].as_u64().context("missing width")?;
        let h = r["height"].as_u64().context("missing height")?;
        ensure!(w > 0 && h > 0 && w <= u32::MAX as u64 && h <= u32::MAX as u64, "invalid extent");
        let g: GeometryParams = serde_json::from_value(r["geometry"].clone())?;
        let frame = Frame::new(aigle_core::geometry::Size::new(w as u32, h as u32), &g);
        let crop: aigle_core::params::CropRect = serde_json::from_value(r["crop"].clone())?;
        let start = frame.shrink_to_fit(frame.rect_of(&crop));
        let out = if r["action"] == "drag" {
            let handles = [Handle::Move, Handle::NW, Handle::NE, Handle::SE,
                           Handle::SW, Handle::N, Handle::E, Handle::S, Handle::W];
            let handle = r["handle"].as_u64().and_then(|i| handles.get(i as usize)).context("invalid handle")?;
            let dx = r["dx"].as_f64().context("missing dx")?;
            let dy = r["dy"].as_f64().context("missing dy")?;
            drag(&frame, start, *handle, dx * frame.w, dy * frame.h,
                 aigle_core::crop::ratio(g.aspect, frame.w, frame.h))
        } else if r["action"] == "aspect" {
            match aigle_core::crop::ratio(g.aspect, frame.w, frame.h) {
                Some(k) => frame.with_aspect(frame.rect_of(&crop), k),
                None => frame.rect_of(&crop),
            }
        } else if r["action"] == "fit" {
            start
        } else { anyhow::bail!("invalid crop action") };
        Ok(json!(frame.crop_of(out)))
    }));
    cstring(match result {
        Ok(Ok(value)) => json!({"ok":value}),
        Ok(Err(e)) => json!({"error":format!("{e:#}")}),
        Err(_) => json!({"error":"Crop geometry panicked"}),
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_command(
    editor: *mut AigleEditor,
    request: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Value> {
        let editor = unsafe { editor.as_mut() }.context("null editor")?;
        let request = serde_json::from_str(unsafe { text(request)? })?;
        editor.command(request)
    }));
    cstring(match result {
        Ok(Ok(value)) => json!({"ok": value}),
        Ok(Err(e)) => json!({"error":format!("{e:#}")}),
        Err(_) => json!({"error":"Rust engine command panicked"}),
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_frame_take(
    editor: *mut AigleEditor,
    original: u32,
) -> *mut AigleFrame {
    guarded(ptr::null_mut(), || {
        let Some(photo) = (unsafe { editor.as_mut() }).and_then(|e| e.photo.as_mut()) else {
            return ptr::null_mut();
        };
        let rx = if original == 0 {
            &mut photo.frames
        } else {
            &mut photo.before_frames
        };
        if !rx.has_changed().unwrap_or(false) {
            return ptr::null_mut();
        }
        let frame = rx.borrow_and_update().clone();
        frame.map_or(ptr::null_mut(), |f| Box::into_raw(Box::new(AigleFrame(f))))
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_frame_pixels(frame: *const AigleFrame) -> AiglePixels {
    guarded(AiglePixels::default(), || {
        let Some(frame) = (unsafe { frame.as_ref() }) else {
            return AiglePixels::default();
        };
        let f = &frame.0;
        let p = &f.pixels;
        let r = &f.viewport.region;
        AiglePixels {
            data: p.data.as_ptr(),
            length: p.data.len(),
            width: p.width,
            height: p.height,
            stride: p.stride,
            format: match p.format {
                PixelFormat::Bgra8 => 0,
                PixelFormat::Rgba16F => 1,
            },
            revision: f.rev.0,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    })
}
unsafe fn preview_frame(
    editor: *mut AigleEditor,
    path: *const c_char,
    max_px: u32,
    error: *mut *mut c_char,
    edited: bool,
) -> *mut AigleFrame {
    if !error.is_null() {
        unsafe { *error = ptr::null_mut() };
    }
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Frame> {
        let editor = unsafe { editor.as_mut() }.context("null editor")?;
        ensure!(
            (1..=if edited { 4096 } else { 1024 }).contains(&max_px),
            "Invalid preview size"
        );
        let path = PathBuf::from(unsafe { text(path)? });
        let params = if edited {
            let mut params = editor.store.read(&path)?.meta.edits.unwrap_or_default();
            if let Some(photo) = &editor.photo {
                if photo.copy.is_none()
                    && matches!(&photo.source, SourceRef::File(open) if open == &path)
                {
                    params = (**photo.doc.head()).clone();
                }
            }
            Some(Arc::new(params))
        } else {
            None
        };
        let thumbnail = editor
            .engine
            .thumbnail(SourceRef::File(path), params, max_px, Priority(-10))
            .wait()?;
        let width = thumbnail.pixels.width;
        let height = thumbnail.pixels.height;
        Ok(Frame {
            rev: Default::default(),
            viewport: Viewport {
                region: aigle_core::geometry::Rect::new(0.0, 0.0, width as f64, height as f64),
                output_size: (width, height),
                display: DisplayProfileId::Srgb,
            },
            quality: Quality::Full,
            pixels: thumbnail.pixels,
        })
    }));
    match result {
        Ok(Ok(frame)) => Box::into_raw(Box::new(AigleFrame(Arc::new(frame)))),
        other => {
            let message = match other {
                Ok(Err(e)) => format!("{e:#}"),
                _ => "Thumbnail panicked".into(),
            };
            if !error.is_null() {
                unsafe { *error = cstring(json!({"error":message})) };
            }
            ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_thumbnail(
    editor: *mut AigleEditor,
    path: *const c_char,
    max_px: u32,
    error: *mut *mut c_char,
) -> *mut AigleFrame {
    unsafe { preview_frame(editor, path, max_px, error, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_comparison_preview(
    editor: *mut AigleEditor,
    path: *const c_char,
    max_px: u32,
    error: *mut *mut c_char,
) -> *mut AigleFrame {
    unsafe { preview_frame(editor, path, max_px, error, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_comparison_frame_take(
    editor: *mut AigleEditor,
    index: u32,
) -> *mut AigleFrame {
    guarded(ptr::null_mut(), || {
        let Some(p) =
            (unsafe { editor.as_mut() }).and_then(|e| e.comparisons.get_mut(index as usize))
        else {
            return ptr::null_mut();
        };
        if !p.frames.has_changed().unwrap_or(false) {
            return ptr::null_mut();
        }
        p.frames
            .borrow_and_update()
            .clone()
            .map(|f| Box::into_raw(Box::new(AigleFrame(f))))
            .unwrap_or(ptr::null_mut())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aigle_frame_free(frame: *mut AigleFrame) {
    if !frame.is_null() {
        guarded((), || drop(unsafe { Box::from_raw(frame) }));
    }
}

impl AigleEditor {
    fn photo(&mut self) -> Result<&mut Photo> {
        self.photo.as_mut().context("Open a photo first")
    }
    fn save(&mut self) -> Result<()> {
        let Some(p) = &mut self.photo else {
            return Ok(());
        };
        if !p.dirty {
            return Ok(());
        }
        if let SourceRef::File(path) = &p.source {
            p.meta.edits = Some((**p.doc.head()).clone());
            if let Some(id) = p.copy {
                self.catalog.set_copy_meta(id, &p.meta)?;
            } else {
                self.store.write(path, &p.meta)?;
            }
            self.catalog.set_history(
                &PhotoKey {
                    path: path.clone(),
                    copy: p.copy,
                },
                &p.doc.saved_history(),
            )?;
        }
        p.dirty = false;
        p.saved_rev = p.doc.rev();
        Ok(())
    }
    fn open(&mut self, source: SourceRef, copy: Option<i64>) -> Result<Value> {
        self.save()?;
        self.agent = None;
        let (meta, history, warnings) = if let SourceRef::File(path) = &source {
            ensure!(path.is_file(), "Photo does not exist: {}", path.display());
            let loaded = self.store.read(path)?;
            let meta = if let Some(id) = copy {
                ensure!(
                    self.catalog
                        .copies_of(&[path.clone()])?
                        .iter()
                        .any(|c| c.id == id),
                    "Virtual copy belongs to another photo"
                );
                self.catalog
                    .copy_meta(id)?
                    .context("Virtual copy no longer exists")?
            } else {
                loaded.meta
            };
            (
                meta,
                self.catalog.history(&PhotoKey {
                    path: path.clone(),
                    copy,
                })?,
                loaded.warnings,
            )
        } else {
            (PhotoMeta::default(), None, vec![])
        };
        let initial = meta.edits.clone().unwrap_or_default();
        let mut doc = if let Some(h) = history {
            PhotoDoc::restore(initial, h)?
        } else {
            PhotoDoc::new(initial)?
        };
        let session = self.engine.open(source.clone());
        doc.attach(session.clone());
        let view = session.create_view(ViewSpec::fit(1000, 700));
        let mut spec = ViewSpec::fit(1000, 700);
        spec.params = ParamsSource::Original;
        let before = session.create_view(spec);
        static NEXT_DOCUMENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let identity = match &source {
            SourceRef::File(path) => self.catalog.photo_identity(path)?,
            _ => format!("synthetic-{}", NEXT_DOCUMENT.load(std::sync::atomic::Ordering::Relaxed)),
        };
        self.photo = Some(Photo {
            identity,
            document_id: NEXT_DOCUMENT.fetch_add(1,std::sync::atomic::Ordering::Relaxed),
            retouch_preview: None,
            info: session.info(),
            frames: view.frames(),
            before_frames: before.frames(),
            histogram: view.histogram(),
            saved_rev: doc.rev(),
            source,
            session,
            doc,
            meta,
            view,
            before,
            dirty: false,
            uncropped: false,
            crop_preview: None,
            crop_anchor: None,
            crop_preview_revision: 0,
            copy,
        });
        Ok(json!({"warnings":warnings}))
    }
    fn command(&mut self, r: Value) -> Result<Value> {
        match r["cmd"].as_str().context("cmd is required")? {
            "open" => {
                return self.open(
                    SourceRef::File(PathBuf::from(r["path"].as_str().context("path required")?)),
                    r["copy"].as_i64(),
                );
            }
            "synthetic" => {
                return self.open(
                    SourceRef::Synthetic {
                        pattern: SyntheticPattern::ColorChart,
                        width: 1200,
                        height: 800,
                    },
                    None,
                );
            }
            "save" => {
                self.save()?;
            }
            "close" => {
                self.save()?;
                self.agent = None;
                self.photo = None;
            }
            "retouch_placement" => {
                let p=self.photo()?;
                ensure!(r["document_id"].as_u64()==Some(p.document_id) && r["revision"].as_u64()==Some(p.doc.rev().0),"Photo or edits changed during retouch placement");
                let LoadState::Ready(info)=p.info.borrow().clone() else {bail!("Photo is still loading")};
                let map=aigle_engine::geometry_map(&info,p.doc.head());let size=map.source_size();
                let points:[Point;2]=serde_json::from_value(r["points"].clone()).context("Two retouch placement points required")?;
                ensure!(points.iter().all(|point|point.x.is_finite() && point.y.is_finite()),"Invalid retouch points");
                let point=|at:Point|{let at=map.crop_to_source(at);aigle_core::masks::Pt{x:(at.x/f64::from(size.w)).clamp(0.,1.) as f32,y:(at.y/f64::from(size.h)).clamp(0.,1.) as f32}};
                let radius=r["radius"].as_f64().unwrap_or(0.015).clamp(0.0005,0.25) as f32;
                let mut candidate=(**p.doc.head()).clone();
                let (kind,index)=if r["tool"]=="red_eye" {
                    let index=candidate.red_eye.len();candidate.red_eye.push(RedEye::new(point(points[0]),radius));("eye",index)
                }else{let index=candidate.spots.len();candidate.spots.push(Spot::new(if r["tool"]=="clone"{SpotMode::Clone}else{SpotMode::Heal},point(points[0]),point(points[1]),radius));("spot",index)};
                return Ok(json!({"session":r["session"],"document_id":p.document_id,"placement":true,"guide":retouch_edit::guide(&candidate,&map,kind,index)?}));
            }
            "retouch_cancel" => {
                if let Some(p) = &mut self.photo {
                    if p.retouch_preview.as_ref().is_some_and(|preview| r["session"] == preview.session) {
                        p.retouch_preview = None;
                        p.session.set_pending(None,Interaction::Final);
                        p.view.set_params_source(ParamsSource::Current);
                    }
                }
            }
            "retouch_preview" | "retouch_commit" => {
                ensure!(self.agent.as_ref().is_none_or(|agent| agent.candidate.is_none()), "Apply or discard the agent proposal before retouching");
                let p=self.photo()?;
                ensure!(r["document_id"].as_u64()==Some(p.document_id),"Photo changed; select the correction again");
                ensure!(r["revision"].as_u64()==Some(p.doc.rev().0),"Edits changed during retouching; select the correction again");
                ensure!(!p.uncropped,"Close the crop tool before retouching");
                let map=match p.info.borrow().clone() {LoadState::Ready(info)=>Some(aigle_engine::geometry_map(&info,p.doc.head())),_=>None};
                let session=r["session"].as_str().context("Retouch session required")?.to_owned();
                let mut next=retouch_edit::change(p.doc.head(),map.as_ref(),&r)?;
                next.sanitize()?;
                ensure!(p.retouch_preview.as_ref().is_none_or(|preview| preview.session==session && preview.base_rev==p.doc.rev()),"Another retouch gesture is active");
                if r["cmd"]=="retouch_preview" {
                    let map=map.as_ref().context("Photo is still loading")?;
                    ensure!(matches!(r["action"].as_str(),Some("set"|"transform")|None),"Deletion and clearing require a committed action");
                    let kind=r["kind"].as_str().context("Correction type required")?;
                    let index=r["index"].as_u64().context("Correction index required")? as usize;
                    let guide=retouch_edit::guide(&next,map,kind,index)?;
                    p.crop_preview_revision=p.crop_preview_revision.max(p.doc.rev().0)+1;
                    p.session.set_pending(Some((aigle_core::Revision(p.crop_preview_revision),Arc::new(next.clone()))),Interaction::Continuous);
                    p.view.set_params_source(ParamsSource::Pending);
                    p.retouch_preview=Some(RetouchPreview{session:session.clone(),base_rev:p.doc.rev()});
                    return Ok(json!({"session":session,"document_id":p.document_id,"guide":guide}));
                }
                p.retouch_preview=None;
                p.session.set_pending(None,Interaction::Final);
                p.view.set_params_source(ParamsSource::Current);
                p.apply(EditCommand::user(p.doc.rev(),r["label"].as_str().unwrap_or("Retouch correction"),vec![ParamOp::set("spots",json!(next.spots)),ParamOp::set("red_eye",json!(next.red_eye))]),Interaction::Final)?;
                p.dirty=p.doc.rev()!=p.saved_rev;
            }
            "scopes" => {
                let request_id = r["request_id"].as_str().context("Scope request ID required")?;
                let Some(p) = self.photo.as_ref() else { return Ok(json!({"request_id":request_id,"available":false})); };
                let histogram = p.histogram.borrow().clone();
                let Some(histogram) = histogram else { return Ok(json!({"request_id":request_id,"available":false})); };
                let target = r["target"].as_u64().unwrap_or(0);
                ensure!(target <= 2, "Unknown scope gamut target");
                let roi = if r["roi"].is_null() { None } else {
                    let rect: [f32;4] = serde_json::from_value(r["roi"].clone()).context("Scope region needs four coordinates")?;
                    ensure!(rect.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                        && rect[0] < rect[2] && rect[1] < rect[3], "Invalid scope region");
                    Some(rect)
                };
                let data = aigle_engine::scopes::analyze(&histogram, roi, target as usize);
                return Ok(json!({"request_id":request_id,"available":true,"data":data,
                    "width":histogram.scope.width,"height":histogram.scope.height,
                    "thumbnail":aigle_engine::scopes::thumbnail(&histogram),
                    "targets":aigle_engine::scopes::reference_targets(data.hdr),
                    "diffuse_white":aigle_engine::scopes::pq(203.) / aigle_engine::scopes::pq(1000.)}));
            }
            "agent_models" => {
                ensure!(self.model_discovery.as_ref().is_none_or(|job| job.state["running"] != true),
                    "Model discovery is already running");
                let endpoint = r["endpoint"].as_str().context("Endpoint required")?.to_owned();
                aigle_agent::agent::endpoint_base(&endpoint)?;
                let key = r["api_key"].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
                    .or_else(|| std::env::var("AIGLE_AGENT_API_KEY").ok().filter(|s| !s.is_empty()));
                let request_id = r["request_id"].as_str().context("Request ID required")?.to_owned();
                let provider_id = r["provider_id"].as_str().unwrap_or("default").to_owned();
                let (tx, result) = std::sync::mpsc::channel();
                self.model_discovery = Some(ModelDiscovery {
                    result, state: json!({"request_id":request_id,"running":true}),
                });
                std::thread::spawn(move || {
                    let result = (|| {
                        let key = match key {
                            Some(key) => key,
                            None => credentials::load(&endpoint, &provider_id)?.unwrap_or_default(),
                        };
                        aigle_agent::fetch_models(endpoint, key)
                    })().map_err(|error: anyhow::Error| format!("{error:#}"));
                    let _ = tx.send(result);
                });
            }
            "agent_models_cancel" => {
                if self.model_discovery.as_ref().is_some_and(|job| job.state["request_id"] == r["request_id"]) {
                    self.model_discovery = None;
                }
            }
            "agent_credential" => {
                let endpoint = r["endpoint"].as_str().context("Endpoint required")?;
                let provider = r["provider_id"].as_str().unwrap_or("default");
                match r["action"].as_str() {
                    Some("save") => credentials::save(endpoint, provider, Some(r["api_key"].as_str().context("API key required")?))?,
                    Some("delete") => credentials::save(endpoint, provider, None)?,
                    Some("delete_provider") => credentials::remove_provider(endpoint, provider)?,
                    _ => bail!("Unknown credential action"),
                }
                return Ok(json!({"stored": r["action"] == "save"}));
            }
            "schema" => {
                let mut schema = EditParams::json_schema();
                for spec in LocalParams::SPECS {
                    let node = &mut schema["properties"]["masks"]["items"]["properties"]["adjust"]
                        ["properties"][spec.name];
                    node["minimum"] = json!(spec.min);
                    node["maximum"] = json!(spec.max);
                    node["x-step"] = json!(spec.step);
                    node["x-unit"] = json!(spec.unit);
                }
                schema["x-metadata-fields"] = json!(
                    aigle_library::Field::ALL
                        .iter()
                        .map(|f| f.name())
                        .collect::<Vec<_>>()
                );
                let formats = [
                    ("JPEG", "jpg", EncodedFormat::Jpeg { quality: 90 }),
                    ("PNG", "png", EncodedFormat::Png),
                    (
                        "TIFF · 16-bit",
                        "tif",
                        EncodedFormat::Tiff { sixteen_bit: true },
                    ),
                    (
                        "TIFF · 8-bit",
                        "tif",
                        EncodedFormat::Tiff { sixteen_bit: false },
                    ),
                    ("WebP", "webp", EncodedFormat::WebP { quality: 90 }),
                    ("AVIF", "avif", EncodedFormat::Avif { quality: 90 }),
                    ("HEIC", "heic", EncodedFormat::Heic { quality: 90 }),
                    ("JPEG XL", "jxl", EncodedFormat::Jxl { quality: 90 }),
                    ("HDR PNG · PQ", "png", EncodedFormat::HdrPng),
                    ("HDR TIFF · float", "tif", EncodedFormat::HdrTiff),
                    ("OpenEXR", "exr", EncodedFormat::OpenExr),
                    (
                        "Ultra HDR JPEG",
                        "jpg",
                        EncodedFormat::UltraHdr { quality: 90 },
                    ),
                    ("DNG", "dng", EncodedFormat::Dng),
                ];
                schema["x-export-formats"] = json!(formats.into_iter().filter(|(_,_,f)|aigle_engine::format_available(*f)).map(|(name,extension,format)|json!({"name":name,"extension":extension,"format":format})).collect::<Vec<_>>());
                return Ok(schema);
            }
            "remote_open" => {
                let server = r["server"].as_str().context("Server required")?;
                let id = r["id"].as_str().context("Photo ID required")?;
                let token = std::env::var("THESAUROS_TOKEN")
                    .context("Set THESAUROS_TOKEN to connect to Thesauros")?;
                let remote = aigle_remote::Remote::new(server, token)?;
                let cache = self
                    .data_dir
                    .join("thesauros")
                    .join(blake3::hash(server.as_bytes()).to_hex().as_str());
                let path = remote.checkout_or_open(id, &cache)?;
                let mut result = self.open(SourceRef::File(path.clone()), None)?;
                result["path"] = json!(path);
                return Ok(result);
            }
            "snapshot" => {
                let p = self.photo()?;
                let name = r["name"].as_str().context("Name is required")?.trim();
                ensure!(!name.is_empty(), "Snapshot needs a name");
                p.meta.snapshots.push(aigle_library::Snapshot {
                    name: name.into(),
                    params: (**p.doc.head()).clone(),
                });
                p.dirty = true;
            }
            "restore_snapshot" => {
                let p = self.photo()?;
                let index = r["index"].as_u64().context("Index required")? as usize;
                let snapshot = p
                    .meta
                    .snapshots
                    .get(index)
                    .context("Snapshot no longer exists")?;
                p.apply(
                    EditCommand::user(
                        p.doc.rev(),
                        format!("Snapshot: {}", snapshot.name),
                        vec![ParamOp::ReplaceAll {
                            params: Box::new(snapshot.params.clone()),
                        }],
                    ),
                    Interaction::Final,
                )?;
                p.dirty = true;
            }
            "create_copies" => return self.create_library_copies(&r),
            "create_copy" => {
                self.save()?;
                let p = self.photo.as_ref().context("Open a photo first")?;
                let SourceRef::File(path) = &p.source else {
                    bail!("Generated images cannot have virtual copies")
                };
                let name = r["name"].as_str().unwrap_or("Copy");
                let mut meta = p.meta.clone();
                meta.edits = Some((**p.doc.head()).clone());
                let id = self.catalog.create_copy(path, name, &meta)?;
                self.catalog.set_history(
                    &PhotoKey {
                        path: path.clone(),
                        copy: Some(id),
                    },
                    &p.doc.saved_history(),
                )?;
                return Ok(json!({"id":id,"path":path,"name":name}));
            }
            "remove_copy" => {
                let p = self.photo.as_ref().context("Open a virtual copy first")?;
                let id = r["id"].as_i64().context("Copy required")?;
                ensure!(
                    p.copy == Some(id),
                    "Only the open virtual copy can be deleted"
                );
                let SourceRef::File(path) = &p.source else {
                    bail!("File required")
                };
                let path = path.clone();
                // Open the master before deletion: a missing/broken master must not
                // leave the editor holding a deleted copy that autosave could recreate.
                let mut result = self.open(SourceRef::File(path.clone()), None)?;
                self.catalog.remove_copy(id)?;
                result["path"] = json!(path);
                result["id"] = json!(id);
                return Ok(result);
            }
            "rename_copy" => {
                let p = self.photo.as_ref().context("Open a virtual copy first")?;
                let id = r["id"].as_i64().context("Copy required")?;
                ensure!(
                    p.copy == Some(id),
                    "Only the open virtual copy can be renamed"
                );
                let name = r["name"].as_str().context("Name required")?.trim();
                ensure!(!name.is_empty(), "Copy name cannot be empty");
                self.catalog.rename_copy(id, name)?;
                return Ok(json!({"id":id,"name":name}));
            }
            "copies" | "library_copies" => {
                let paths = if r["cmd"] == "library_copies" {
                    let paths: Vec<PathBuf> = serde_json::from_value(r["paths"].clone())?;
                    ensure!(paths.len() <= 10000, "Choose at most 10000 photos");
                    paths
                } else {
                    let p = self.photo.as_ref().context("Open a photo first")?;
                    let SourceRef::File(path) = &p.source else { return Ok(json!([])); };
                    vec![path.clone()]
                };
                return Ok(json!(self.catalog.copies_of(&paths)?.iter()
                    .map(|c| json!({"id":c.id,"name":c.name,"path":c.path}))
                    .collect::<Vec<_>>()));
            }
            "comparison_open" => {
                let targets = library_targets::targets(&r)?;
                ensure!((2..=4).contains(&targets.len()), "Choose two to four photos");
                self.save()?;
                self.comparisons.clear();
                // Reuse Photo's session/view ownership without changing the active document.
                let active = self.photo.take();
                let agent = self.agent.take();
                let result = (|| -> Result<()> {
                    for key in targets {
                        self.open(SourceRef::File(key.path), key.copy)?;
                        self.comparisons.push(self.photo.take().unwrap());
                    }
                    Ok(())
                })();
                self.photo = active;
                self.agent = agent;
                if result.is_err() {
                    self.comparisons.clear();
                }
                result?;
            }
            "comparison_close" => {
                self.comparison_display_detection = None;
                self.comparisons.clear();
            }
            "comparison_detect_display" => {
                self.comparison_display_detection = Some(display::Detection::start(
                    self.engine.clone(), r["screen"].as_str().unwrap_or_default().to_owned()));
            }
            "comparison_display_state" => {
                if let Some(display) = &mut self.comparison_display_detection {
                    display.poll();
                    return Ok(display.state.clone());
                }
                return Ok(Value::Null);
            }
            "comparison_state" => {
                return Ok(json!(
                    self.comparisons
                        .iter()
                        .map(|p| {
                            let load = p.info.borrow().clone();
                            let extent = if let LoadState::Ready(info) = &load {
                                let size =
                                    aigle_engine::geometry_map(info, p.doc.head()).crop_extent();
                                Some([size.0, size.1])
                            } else {
                                None
                            };
                            json!({"load":load,"extent":extent})
                        })
                        .collect::<Vec<_>>()
                ));
            }
            "comparison_viewport" => {
                let index = r["index"].as_u64().context("Index required")? as usize;
                let p = self
                    .comparisons
                    .get_mut(index)
                    .context("Comparison closed")?;
                let vp: Viewport = serde_json::from_value(r["viewport"].clone())?;
                ensure!(
                    vp.output_size.0 > 0
                        && vp.output_size.1 > 0
                        && vp.output_size.0 <= 4096
                        && vp.output_size.1 <= 4096,
                    "Invalid comparison output size"
                );
                let rect = vp.region;
                ensure!(
                    [rect.x, rect.y, rect.w, rect.h]
                        .iter()
                        .all(|v| v.is_finite())
                        && rect.w > 0.
                        && rect.h > 0.,
                    "Invalid comparison region"
                );
                p.view.set_viewport(vp, Interaction::Final);
            }
            "file_action" => return self.file_action(&r),
            "find_duplicates" => {
                ensure!(
                    self.duplicates
                        .as_ref()
                        .is_none_or(|t| t.status()["running"] != true),
                    "A duplicate scan is already running"
                );
                let paths: Vec<PathBuf> = serde_json::from_value(r["paths"].clone())?;
                ensure!(
                    !paths.is_empty() && paths.len() <= 100000,
                    "Select 1 to 100000 photos"
                );
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let status = Arc::new(std::sync::Mutex::new(json!({"running":true,"id":r["id"]})));
                let worker_cancel = cancel.clone();
                let worker_status = status.clone();
                std::thread::spawn(move || {
                    let report = std::panic::catch_unwind(|| {
                        aigle_library::duplicates::find(paths, &worker_cancel)
                    });
                    let mut status = worker_status.lock().unwrap_or_else(|p| p.into_inner());
                    status["running"] = json!(false);
                    let cancelled = worker_cancel.load(std::sync::atomic::Ordering::Relaxed);
                    status["cancelled"] = json!(cancelled);
                    match report {
                        Ok(report) if !cancelled => {
                            status["groups"] = json!(report.groups);
                            status["failures"] = json!(report.failed);
                        }
                        Err(_) => status["error"] = json!("Duplicate scan failed"),
                        _ => {}
                    }
                });
                self.duplicates = Some(ImportTask { cancel, status });
            }
            "cancel_duplicates" => {
                if let Some(task) = &self.duplicates {
                    task.cancel
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            "library_metadata" => {
                let path = PathBuf::from(r["path"].as_str().context("Path required")?);
                let copy = match r.get("copy") {
                    None | Some(Value::Null) => None,
                    Some(id) => Some(id.as_i64().filter(|id| *id > 0).context("Invalid virtual copy ID")?),
                };
                let loaded = self.target_meta(&PhotoKey { path: path.clone(), copy });
                let mut meta = loaded.as_ref().cloned().unwrap_or_default();
                if let Some(p) = &self.photo {
                    if p.copy == copy
                        && matches!(&p.source, SourceRef::File(open) if open == &path)
                    {
                        meta = p.meta.clone();
                    }
                }
                let facts = aigle_library::read_facts(&path);
                let modified = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|t| t.as_secs_f64());
                return Ok(
                    json!({"path":path,"copy":copy,"meta_loaded":loaded.is_ok(),"facts_loaded":true,
                    "rating":loaded.as_ref().ok().map(|_| meta.rating),"flag":meta.flag,"label":meta.label,
                    "tags":meta.tags(),"keywords":meta.all_keywords(),"iptc":meta.iptc,"stack":meta.stack,
                    "camera":facts.camera,"lens":facts.lens,"captured":facts.captured,"modified":modified,
                    "metadata_error":loaded.err().map(|e| e.to_string())}),
                );
            }
            "albums" => return Ok(json!(self.catalog.albums()?)),
            "create_album" => {
                let paths: Vec<PathBuf> = serde_json::from_value(r["paths"].clone())?;
                return Ok(json!(self.catalog.create_album(
                    r["name"].as_str().context("Name required")?,
                    &paths
                )?));
            }
            "album_paths" | "update_album" | "rename_album" | "delete_album" => {
                let id = r["id"].as_i64().context("Album required")?;
                let mut name = self
                    .catalog
                    .albums()?
                    .into_iter()
                    .find(|(key, _)| *key == id)
                    .context("Album no longer exists")?
                    .1;
                match r["cmd"].as_str().unwrap() {
                    "update_album" => {
                        let paths: Vec<PathBuf> = serde_json::from_value(r["paths"].clone())?;
                        ensure!(
                            !paths.is_empty() && paths.len() <= 10000,
                            "Select 1 to 10000 photos"
                        );
                        self.catalog.update_album(
                            id,
                            &paths,
                            r["add"].as_bool().context("Add/remove required")?,
                        )?;
                    }
                    "rename_album" => {
                        name = r["name"]
                            .as_str()
                            .context("Name required")?
                            .trim()
                            .to_string();
                        self.catalog.rename_album(id, &name)?;
                    }
                    "delete_album" => {
                        self.catalog.delete_album(id)?;
                        return Ok(json!({"id":id,"name":name}));
                    }
                    _ => {}
                }
                return Ok(json!({"id":id,"name":name,"paths":self.catalog.album_paths(id)?}));
            }
            "catalog_restore" => {
                let path = PathBuf::from(r["path"].as_str().context("Backup path required")?);
                let live = self.data_dir.join("catalog.sqlite3");
                ensure!(
                    path.canonicalize()? != live.canonicalize()?,
                    "Choose a backup, not the live catalog"
                );
                ensure!(
                    self.export.is_none()
                        && self
                            .import
                            .as_ref()
                            .is_none_or(|t| t.status()["running"] != true)
                        && self
                            .duplicates
                            .as_ref()
                            .is_none_or(|t| t.status()["running"] != true),
                    "Finish active jobs before restoring the catalog"
                );
                Catalog::validate_backup(&path)?;
                self.save()?;
                let recovery = self.data_dir.join(format!(
                    "catalog-before-restore-{}.sqlite3",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_nanos()
                ));
                self.catalog.backup(&recovery)?;
                self.agent = None;
                self.comparisons.clear();
                self.photo = None;
                if let Err(error) = self.catalog.restore(&path) {
                    let rollback = self.catalog.restore(&recovery);
                    return Ok(
                        json!({"restored":false,"recovery":recovery,"error":error.to_string(),
                        "rollback_error":rollback.err().map(|e|e.to_string())}),
                    );
                }
                return Ok(json!({"restored":true,"recovery":recovery}));
            }
            "catalog_backup" => {
                self.save()?;
                self.catalog
                    .backup(&PathBuf::from(r["path"].as_str().context("Path required")?))?;
            }
            "import" => {
                ensure!(
                    self.import
                        .as_ref()
                        .is_none_or(|t| t.status()["running"] != true),
                    "An import is already running"
                );
                let source = PathBuf::from(r["source"].as_str().context("Source folder required")?);
                let options = aigle_library::import::ImportOptions {
                    dest: PathBuf::from(
                        r["destination"]
                            .as_str()
                            .context("Destination folder required")?,
                    ),
                    folders: r["folders"]
                        .as_str()
                        .unwrap_or(aigle_library::import::DEFAULT_FOLDERS)
                        .into(),
                    names: r["names"]
                        .as_str()
                        .unwrap_or(aigle_library::import::DEFAULT_NAMES)
                        .into(),
                    backup: r["backup"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from),
                };
                aigle_library::import::check(&options)?;
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let status = Arc::new(std::sync::Mutex::new(
                    json!({"running":true,"phase":"Scanning"}),
                ));
                self.import = Some(ImportTask {
                    cancel: cancel.clone(),
                    status: status.clone(),
                });
                std::thread::spawn(move || {
                    let result = (|| -> Result<Value> {
                        let sources = aigle_library::import::read_sources(
                            &source,
                            aigle_library::import::needs_facts(&options),
                            &cancel,
                        )?;
                        let plan = aigle_library::import::plan(&sources, &options)?;
                        let report = aigle_library::import::run(
                            &plan,
                            &options,
                            &cancel,
                            &mut |progress| {
                                *status.lock().unwrap_or_else(|p| p.into_inner()) = json!({"running":true,"phase":"Copying","files":progress.files,"total":progress.total_files,"bytes":progress.bytes,"total_bytes":progress.total_bytes});
                            },
                        );
                        Ok(
                            json!({"running":false,"imported":report.imported,"duplicates":report.duplicates,"failed":report.failed,"backup_failed":report.backup_failed,"cancelled":report.cancelled}),
                        )
                    })();
                    *status.lock().unwrap_or_else(|p| p.into_inner()) = match result {
                        Ok(v) => v,
                        Err(e) => json!({"running":false,"error":format!("{e:#}")}),
                    };
                });
            }
            "cancel_import" => {
                if let Some(task) = &self.import {
                    task.cancel
                        .store(true, std::sync::atomic::Ordering::Release);
                }
            }
            "scan" => {
                let path = PathBuf::from(r["path"].as_str().context("path required")?);
                let files = if r["recursive"] == true {
                    aigle_library::scan_recursive(&path)?
                } else {
                    aigle_library::scan(&path)?
                };
                return Ok(json!(
                    files
                        .iter()
                        .map(|p| p.path.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                ));
            }
            "paste_selected" | "sync_selected" => return self.selective_edits(&r),
            "edit" => {
                let p = self.photo()?;
                if p.retouch_preview.take().is_some() { p.session.set_pending(None,Interaction::Final);p.view.set_params_source(ParamsSource::Current); }
                let ops: Vec<ParamOp> = serde_json::from_value(r["ops"].clone())?;
                let mut cmd =
                    EditCommand::user(p.doc.rev(), r["label"].as_str().unwrap_or("Edit"), ops);
                if let Some(g) = r["gesture"].as_u64() {
                    cmd = cmd.in_gesture(GestureId(g));
                }
                p.apply(
                    cmd,
                    if r["continuous"] == true {
                        Interaction::Continuous
                    } else {
                        Interaction::Final
                    },
                )?;
                p.dirty = p.doc.rev() != p.saved_rev;
            }
            "undo" => {
                let p = self.photo()?;
                if p.retouch_preview.take().is_some() {p.session.set_pending(None,Interaction::Final);p.view.set_params_source(ParamsSource::Current);}
                p.doc.undo();
                p.dirty = p.doc.rev() != p.saved_rev;
            }
            "redo" => {
                let p = self.photo()?;
                if p.retouch_preview.take().is_some() {p.session.set_pending(None,Interaction::Final);p.view.set_params_source(ParamsSource::Current);}
                p.doc.redo();
                p.dirty = p.doc.rev() != p.saved_rev;
            }
            "goto" => {
                let p = self.photo()?;
                if p.retouch_preview.take().is_some() {p.session.set_pending(None,Interaction::Final);p.view.set_params_source(ParamsSource::Current);}
                p.doc
                    .goto(r["position"].as_u64().context("position required")? as usize);
                p.dirty = true;
            }
            "viewport" => {
                let vp: Viewport = serde_json::from_value(r["viewport"].clone())?;
                ensure!(
                    vp.output_size.0 > 0
                        && vp.output_size.1 > 0
                        && vp.output_size.0 <= 16384
                        && vp.output_size.1 <= 16384,
                    "Invalid viewport size"
                );
                let rect = &vp.region;
                ensure!(
                    [rect.x, rect.y, rect.w, rect.h]
                        .iter()
                        .all(|v| v.is_finite())
                        && rect.w > 0.0
                        && rect.h > 0.0,
                    "Invalid viewport region"
                );
                let hint = if r["continuous"] == true {
                    Interaction::Continuous
                } else {
                    Interaction::Final
                };
                // A throttled canvas update can already be queued when a file
                // action closes the active photo. There is no view to update.
                if let Some(p) = self.photo.as_mut() {
                    p.view.set_viewport(vp.clone(), hint);
                    p.before.set_viewport(vp, hint);
                }
            }
            "overlays" => {
                let p = self.photo()?;
                p.view
                    .set_overlays(serde_json::from_value(r["overlays"].clone())?);
            }
            "batch_metadata" => {
                let targets = library_targets::targets(&r)?;
                ensure!(
                    !targets.is_empty() && targets.len() <= 10000,
                    "Select 1 to 10000 photos"
                );
                let rating = r
                    .get("rating")
                    .map(|v| v.as_i64().context("Rating must be an integer"))
                    .transpose()?;
                ensure!(
                    rating.is_none_or(|v| (0..=5).contains(&v)),
                    "Rating must be 0 to 5"
                );
                let flag: Option<aigle_library::Flag> = r
                    .get("flag")
                    .map(|v| serde_json::from_value(v.clone()))
                    .transpose()?;
                ensure!(
                    r.get("label").is_none_or(|v| v.is_null() || v.is_string()),
                    "Label must be text or null"
                );
                ensure!(
                    r.get("stack").is_none_or(|v| v.is_null()
                        || v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128)),
                    "Stack must be an identifier or null"
                );
                let tags: Option<Vec<String>> = r
                    .get("tags")
                    .map(|v| serde_json::from_value(v.clone()))
                    .transpose()?;
                let tag_mode = r.get("tag_mode").and_then(Value::as_str).unwrap_or("");
                ensure!(
                    (tags.is_none() && r.get("tag_mode").is_none())
                        || (tags.is_some() && matches!(tag_mode, "add" | "remove" | "replace" | "patch")),
                    "Keywords require an explicit add, remove, replace, or patch mode"
                );
                let remove_tags: Vec<String> = r.get("remove_tags")
                    .map(|v| serde_json::from_value(v.clone())).transpose()?.unwrap_or_default();
                ensure!(r.get("remove_tags").is_none() || tag_mode == "patch", "Keyword removals require patch mode");
                let iptc: Option<std::collections::BTreeMap<aigle_library::Field, String>> = r
                    .get("iptc")
                    .map(|v| serde_json::from_value(v.clone()))
                    .transpose()?;
                ensure!(
                    rating.is_some()
                        || flag.is_some()
                        || r.get("label").is_some()
                        || r.get("stack").is_some()
                        || tags.is_some()
                        || iptc.as_ref().is_some_and(|v| !v.is_empty()),
                    "No metadata changes supplied"
                );
                self.save()?;
                let mut updated = vec![];
                let mut failures = vec![];
                let mut seen = std::collections::HashSet::new();
                for key in targets {
                    let path = &key.path;
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    let result = (|| -> Result<PhotoMeta> {
                        ensure!(path.is_file(), "Photo does not exist");
                        let mut meta = self.target_meta(&key)?;
                        if let Some(v) = rating {
                            meta.rating = v as i8;
                        }
                        if let Some(v) = flag {
                            meta.flag = v;
                        }
                        if let Some(v) = r.get("label") {
                            meta.label = v.as_str().map(str::to_owned);
                        }
                        if let Some(stack) = r.get("stack") {
                            meta.stack = stack.as_str().map(str::to_owned);
                        }
                        if let Some(tags) = &tags {
                            let mut normalized = PhotoMeta::default();
                            normalized.set_tags(tags);
                            let requested = normalized.tags();
                            let mut existing = meta.tags();
                            match tag_mode {
                                "add" => existing.extend(requested),
                                "remove" => existing.retain(|tag| {
                                    !requested
                                        .iter()
                                        .any(|v| v.to_lowercase() == tag.to_lowercase())
                                }),
                                "replace" => existing = requested,
                                "patch" => {
                                    let mut removed = PhotoMeta::default();
                                    removed.set_tags(&remove_tags);
                                    let removed = removed.tags();
                                    existing.retain(|tag| !removed.iter().any(|v| v.to_lowercase() == tag.to_lowercase()));
                                    existing.extend(requested);
                                }
                                _ => unreachable!(),
                            }
                            meta.set_tags(existing);
                        }
                        if let Some(fields) = &iptc {
                            for (field, value) in fields {
                                meta.set_field(*field, value);
                            }
                        }
                        self.write_target_meta(&key, &meta)?;
                        Ok(meta)
                    })();
                    match result {
                        Ok(meta) => {
                            updated.push(json!({"path":path,"copy":key.copy,"rating":meta.rating,"flag":meta.flag,"label":meta.label}));
                            if let Some(p) = &mut self.photo {
                                if p.copy == key.copy
                                    && matches!(&p.source, SourceRef::File(open) if open == path)
                                {
                                    p.meta = meta;
                                }
                            }
                        }
                        Err(error) => failures.push(json!({"path":path,"copy":key.copy,"error":error.to_string()})),
                    }
                }
                return Ok(json!({"updated":updated,"failures":failures}));
            }
            "metadata" => {
                let p = self.photo()?;
                let mut meta = p.meta.clone();
                if let Some(fields) = r["iptc"].as_object() {
                    for (name, value) in fields {
                        let field = serde_json::from_value(json!(name))?;
                        meta.set_field(
                            field,
                            value.as_str().context("Metadata fields must be text")?,
                        );
                    }
                }
                if r.get("flag").is_some() {
                    meta.flag = serde_json::from_value(r["flag"].clone())?;
                }
                if let Some(rating) = r["rating"].as_i64() {
                    ensure!((-1..=5).contains(&rating), "Rating must be -1 to 5");
                    meta.rating = rating as i8;
                }
                if let Some(tags) = r["tags"].as_array() {
                    meta.set_tags(tags.iter().filter_map(Value::as_str));
                }
                if r.get("label").is_some() {
                    meta.label = r["label"].as_str().map(str::to_owned);
                }
                p.meta = meta;
                p.dirty = true;
            }
            "auto_tone" => {
                let p = self.photo()?;
                let params = aigle_agent::auto_tone::neutral(p.doc.head());
                let analysis = p
                    .session
                    .analyze(
                        Arc::new(params),
                        AnalysisSet {
                            histogram: true,
                            clipping: true,
                            wb: false,
                            noise: false,
                            ..Default::default()
                        },
                    )
                    .wait()?;
                let ops = aigle_agent::auto_tone::ops(
                    analysis
                        .histogram
                        .as_ref()
                        .context("No histogram returned")?,
                    analysis
                        .clipping
                        .as_ref()
                        .context("No clipping analysis returned")?,
                );
                p.apply(
                    EditCommand::user(p.doc.rev(), "Auto tone", ops),
                    Interaction::Final,
                )?;
                p.dirty = true;
            }
            "agent_start" => {
                ensure!(
                    self.agent
                        .as_ref()
                        .is_none_or(|t| t.state["running"] != true),
                    "An agent is already running"
                );
                let p = self.photo.as_ref().context("Open a photo first")?;
                ensure!(
                    matches!(*p.info.borrow(), LoadState::Ready(_)),
                    "Photo is still loading"
                );
                let settings = aigle_agent::ActiveModel {
                    endpoint: r["endpoint"].as_str().context("Endpoint required")?.into(),
                    model: r["model"].as_str().context("Model required")?.into(),
                    api_key: String::new(),
                };
                let key = r["api_key"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| std::env::var("AIGLE_AGENT_API_KEY").ok().filter(|key| !key.is_empty()));
                let key = match key {
                    Some(key) => key,
                    None => credentials::load(&settings.endpoint, r["provider_id"].as_str().unwrap_or("default"))?
                        .context("No saved API key. Enter a key in Provider & model or set AIGLE_AGENT_API_KEY")?,
                };
                let prompt = r["prompt"].as_str().context("Prompt required")?.to_owned();
                let base = (**p.doc.head()).clone();
                let base_rev = p.doc.rev();
                let session = p.session.clone();
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let (events, rx) = futures::channel::mpsc::unbounded();
                let (tx, result) = std::sync::mpsc::channel();
                static NEXT_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                let run_id = NEXT_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.agent = Some(AgentTask {
                    cancel: cancel.clone(),
                    events: rx,
                    result,
                    base: base.clone(),
                    base_rev,
                    candidate: None,
                    state: json!({"running":true,"run_id":run_id,"events":[],"message":"Inspecting image and refining edits…"}),
                });
                std::thread::spawn(move || {
                    let result =
                        aigle_agent::run(settings, key, session, base, prompt, cancel, events)
                            .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(result);
                });
            }
            "agent_cancel" | "agent_reject" => {
                self.agent = None;
                if let Some(p) = &self.photo {
                    p.session.set_pending(None, Interaction::Final);
                    p.view.set_params_source(ParamsSource::Current);
                }
            }
            "agent_accept" => {
                let task = self.agent.as_ref().context("No agent proposal")?;
                let candidate = task.candidate.as_ref().context("Agent has not finished")?;
                let ops = aigle_agent::profile_ops(&task.base, candidate);
                let p = self.photo.as_mut().context("Open a photo first")?;
                let mut command = EditCommand::user(task.base_rev, "Agent edits", ops);
                command.origin = aigle_doc::Origin::Agent {
                    run_id: task.base_rev.0,
                    step: 0,
                };
                p.apply(command, Interaction::Final)?;
                p.dirty = true;
                p.session.set_pending(None, Interaction::Final);
                p.view.set_params_source(ParamsSource::Current);
                self.agent = None;
            }
            "import_profile" => {
                self.photo()?;
                let kind = match r["kind"].as_str() {
                    Some("dcp") => ImportKind::Dcp,
                    Some("lut") => ImportKind::LookLut,
                    _ => bail!("Choose a DCP camera profile or cube look LUT"),
                };
                let path = PathBuf::from(r["path"].as_str().context("Profile path required")?);
                let imported = self.engine.import(kind, std::fs::read(&path)?).wait()?;
                let (field, value) = if kind == ImportKind::Dcp {
                    ("profile.camera", json!(aigle_core::params::CameraProfile::Dcp(imported.data_ref.clone())))
                } else { ("profile.look", json!(imported.data_ref)) };
                let p = self.photo()?;
                p.apply(EditCommand::user(p.doc.rev(), format!("Import {}", imported.name),
                    vec![ParamOp::set(field, value)]), Interaction::Final)?;
                p.dirty = true;
                return Ok(json!({"name":imported.name,"camera_model":imported.camera_model}));
            }
            "match_lens" => {
                let p = self.photo()?;
                let found = p.session.match_lens().wait()?;
                let matched = found.first().context("No matching lens profile found")?;
                let name = format!("{} {}", matched.lens_maker, matched.lens_model);
                p.apply(EditCommand::user(p.doc.rev(), format!("Lens profile: {name}"),
                    vec![ParamOp::set("lens.profile", json!(aigle_core::params::LensProfile::Resolved(matched.data_ref.clone())))]), Interaction::Final)?;
                p.dirty = true;
                return Ok(json!({"name":name}));
            }
            "auto_wb" => {
                let p = self.photo()?;
                let wb = p.session.auto_white_balance(None).wait()?;
                p.apply(
                    EditCommand::user(
                        p.doc.rev(),
                        "Auto white balance",
                        vec![
                            ParamOp::set("wb.mode", "custom"),
                            ParamOp::set("wb.temperature", wb.temperature),
                            ParamOp::set("wb.tint", wb.tint),
                        ],
                    ),
                    Interaction::Final,
                )?;
                p.dirty = true;
            }
            "crop_orientation" => {
                ensure!(
                    self.agent.is_none(),
                    "Apply or discard the Agent proposal before cropping"
                );
                let p = self.photo()?;
                ensure!(p.uncropped, "Open the crop tool first");
                let orientation: aigle_core::params::OrientationDelta =
                    serde_json::from_value(r["orientation"].clone())?;
                ensure!(orientation.quarter_turns < 4, "Invalid orientation");
                let mut draft = (**p.doc.head()).clone();
                draft.geometry.orientation_delta = orientation;
                p.crop_preview_revision = p.crop_preview_revision.max(p.doc.rev().0) + 1;
                p.session.set_pending(
                    Some((
                        aigle_core::Revision(p.crop_preview_revision),
                        Arc::new(draft.clone()),
                    )),
                    Interaction::Final,
                );
                p.view.set_params_source(ParamsSource::Pending);
                p.crop_preview = Some(draft);
            }
            "tool_mode" => {
                let p = self.photo()?;
                if r["tool"]!="retouch" && p.retouch_preview.take().is_some() {p.session.set_pending(None,Interaction::Final);p.view.set_params_source(ParamsSource::Current);}
                p.uncropped = r["tool"] == "crop";
                if !p.uncropped && p.crop_preview.take().is_some() {
                    p.session.set_pending(None, Interaction::Final);
                    p.view.set_params_source(ParamsSource::Current);
                }
                p.view.set_uncropped(p.uncropped);
                p.before.set_uncropped(p.uncropped);
            }
            "tool" => {
                let p = self.photo()?;
                if r["document_id"].is_u64(){ensure!(r["document_id"].as_u64()==Some(p.document_id) && r["revision"].as_u64()==Some(p.doc.rev().0),"Photo or edits changed during retouch creation");}
                let LoadState::Ready(info) = p.info.borrow().clone() else {
                    bail!("Photo is still loading")
                };
                let map = aigle_engine::geometry_map(&info, p.doc.head());
                let size = map.source_size();
                let points: Vec<Point> = serde_json::from_value(r["points"].clone())?;
                ensure!(
                    !points.is_empty()
                        && points.len() <= 100000
                        && points.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
                    "Invalid tool points"
                );
                let normalize = |at: Point| {
                    let source = map.crop_to_source(at);
                    Pt {
                        x: (source.x / size.w as f64).clamp(0.0, 1.0) as f32,
                        y: (source.y / size.h as f64).clamp(0.0, 1.0) as f32,
                    }
                };
                let from = points[0];
                let to = *points.last().unwrap();
                let radius = r["radius"].as_f64().unwrap_or(0.02).clamp(0.0005, 0.25) as f32;
                let tool = r["tool"].as_str().context("tool required")?;
                let mut ops = vec![];
                match tool {
                    "crop" => {
                        let mut params = p
                            .crop_preview
                            .clone()
                            .unwrap_or_else(|| (**p.doc.head()).clone());
                        if let Some(aspect) = r.get("aspect") {
                            let parsed: aigle_core::params::Aspect = serde_json::from_value(aspect.clone())?;
                            if let aigle_core::params::Aspect::Ratio { w, h } = parsed {
                                ensure!(w > 0 && h > 0, "Invalid crop aspect ratio");
                            }
                            params.geometry.aspect = parsed;
                            ops.push(ParamOp::set("geometry.aspect", aspect.clone()));
                        }
                        if let Some(orientation) = r.get("orientation") {
                            params.geometry.orientation_delta =
                                serde_json::from_value(orientation.clone())?;
                            ensure!(
                                params.geometry.orientation_delta.quarter_turns < 4,
                                "Invalid orientation"
                            );
                            ops.push(ParamOp::set(
                                "geometry.orientation_delta",
                                orientation.clone(),
                            ));
                        }
                        let size = aigle_engine::geometry_map(&info, &params).source_size();
                        let mut geometry = params.geometry;
                        if let Some(angle) = r["angle"].as_f64() {
                            ensure!(
                                angle.is_finite() && (-45.0..=45.0).contains(&angle),
                                "Invalid crop angle"
                            );
                            geometry.scalars.angle = angle as f32;
                            ops.push(ParamOp::set("geometry.angle", angle));
                        }
                        let full = aigle_core::crop::Frame::new(size, &geometry);
                        let (from, to) = if r["normalized"] == true {
                            (
                                Point::new(from.x * size.w as f64, from.y * size.h as f64),
                                Point::new(to.x * size.w as f64, to.y * size.h as f64),
                            )
                        } else {
                            (from, to)
                        };
                        let rect = Rect::new(
                            from.x.min(to.x),
                            from.y.min(to.y),
                            (from.x - to.x).abs(),
                            (from.y - to.y).abs(),
                        );
                        ensure!(
                            rect.w >= full.min_side() && rect.h >= full.min_side(),
                            "Draw a larger crop rectangle"
                        );
                        let crop = full.crop_of(full.shrink_to_fit(rect));
                        ops.push(ParamOp::set("geometry.crop", json!(crop)));
                    }
                    "color_range" | "color_pick" => {
                        let sample = p.session.sample(from, radius * size.long_edge() as f32, SampleStage::SceneLinear).wait()?;
                        if tool == "color_range" {
                            let id = MaskId(p.doc.head().masks.iter().map(|m| m.id.0).max().unwrap_or(0) + 1);
                            ops.push(ParamOp::AddMask { mask: LocalAdjustment {
                                id, name: format!("Color range {}", id.0), opacity: 1.0, invert: false,
                                adjust: LocalParams::default(),
                                shape: MaskShape::Range { range: aigle_core::masks::RangeKind::Color {
                                    samples: vec![sample.rgb], tolerance: 20.0 }, smoothness: 20.0 },
                            }});
                        } else {
                            let id = r["mask_id"].as_u64().context("Select a color mask first")?;
                            let (i, mask) = p.doc.head().masks.iter().enumerate().find(|(_, m)| u64::from(m.id.0) == id).context("Mask no longer exists")?;
                            let mut shape = mask.shape.clone();
                            let MaskShape::Range { range: aigle_core::masks::RangeKind::Color { samples, .. }, .. } = &mut shape else { bail!("Select a color range mask"); };
                            *samples = vec![sample.rgb];
                            ops.push(ParamOp::set(&format!("masks.{i}.shape"), json!(shape)));
                        }
                    }
                    "wb" => {
                        let wb = p
                            .session
                            .neutral_white_balance(from, radius * size.long_edge() as f32)
                            .wait()?;
                        ops.extend([
                            ParamOp::set("wb.temperature", wb.temperature),
                            ParamOp::set("wb.tint", wb.tint),
                            ParamOp::set("wb.mode", "custom"),
                        ]);
                    }
                    "heal" | "clone" => {
                        let target = normalize(from);
                        let source = if (to.x - from.x).hypot(to.y - from.y) > 2.0 {
                            normalize(to)
                        } else {
                            p.session.spot_source(target, radius).wait()?
                        };
                        let mut spots = p.doc.head().spots.clone();
                        let mut spot=Spot::new(
                            if tool == "heal" {
                                SpotMode::Heal
                            } else {
                                SpotMode::Clone
                            },
                            target,
                            source,
                            radius,
                        );
                        spot.feather=r["feather"].as_f64().unwrap_or(50.) as f32;
                        spot.opacity=r["opacity"].as_f64().unwrap_or(1.) as f32;
                        spots.push(spot);
                        ops.push(ParamOp::set("spots", json!(spots)));
                    }
                    "red_eye" => {
                        let mut eyes = p.doc.head().red_eye.clone();
                        eyes.push(RedEye {
                            center: normalize(from),
                            radius,
                            pupil: r["pupil"].as_f64().unwrap_or(50.) as f32,
                            darken: r["darken"].as_f64().unwrap_or(50.) as f32,
                        });
                        ops.push(ParamOp::set("red_eye", json!(eyes)));
                    }
                    "mask_transform" => {
                        let id = r["id"].as_u64().context("Mask ID required")?;
                        let mut masks = p.doc.head().masks.clone();
                        let mask = masks
                            .iter_mut()
                            .find(|m| u64::from(m.id.0) == id)
                            .context("Mask no longer exists")?;
                        match r["handle"].as_str() {
                            Some("move") => {
                                let a = map.crop_to_source(from);
                                let b = map.crop_to_source(to);
                                mask_edit::translate(
                                    &mut mask.shape,
                                    ((b.x - a.x) / size.w as f64) as f32,
                                    ((b.y - a.y) / size.h as f64) as f32,
                                )?;
                            }
                            Some("radius_x") | Some("radius_y") => {
                                let at = map.crop_to_source(to);
                                mask_edit::resize_radial(
                                    &mut mask.shape,
                                    Pt {
                                        x: (at.x / size.w as f64) as f32,
                                        y: (at.y / size.h as f64) as f32,
                                    },
                                    if r["handle"] == "radius_x" { 0 } else { 1 },
                                    size.w as f32,
                                    size.h as f32,
                                )?;
                            }
                            _ => match &mut mask.shape {
                                MaskShape::Linear { from: a, to: b } => {
                                    *a = normalize(from);
                                    *b = normalize(to);
                                }
                                // Compatibility with callers using the original center/corner contract.
                                MaskShape::Radial {
                                    center,
                                    radii,
                                    angle,
                                    ..
                                } if *angle == 0.0 => {
                                    let a = normalize(from);
                                    let b = normalize(to);
                                    *center = a;
                                    *radii = [
                                        (a.x - b.x).abs().max(0.005),
                                        (a.y - b.y).abs().max(0.005),
                                    ];
                                }
                                _ => bail!("Choose a mask handle"),
                            },
                        }
                        ops.push(ParamOp::set("masks", json!(masks)));
                    }
                    "radial" | "linear" | "brush" => {
                        let a = normalize(from);
                        let b = normalize(to);
                        let shape = match tool {
                            "linear" => MaskShape::Linear { from: a, to: b },
                            "radial" => MaskShape::Radial {
                                center: a,
                                radii: [(a.x - b.x).abs().max(0.005), (a.y - b.y).abs().max(0.005)],
                                angle: 0.0,
                                feather: 50.0,
                            },
                            _ => MaskShape::Brush {
                                strokes: vec![Stroke {
                                    points: points.iter().map(|p| normalize(*p)).collect(),
                                    size: radius,
                                    feather: 50.0,
                                    flow: 100.0,
                                    erase: false,
                                }],
                            },
                        };
                        let id = MaskId(
                            p.doc.head().masks.iter().map(|m| m.id.0).max().unwrap_or(0) + 1,
                        );
                        ops.push(ParamOp::AddMask {
                            mask: LocalAdjustment {
                                id,
                                name: format!("{} {}", tool, id.0),
                                shape,
                                invert: false,
                                opacity: 1.0,
                                adjust: LocalParams::default(),
                            },
                        });
                    }
                    _ => bail!("Unknown tool: {tool}"),
                }
                p.apply(
                    EditCommand::user(p.doc.rev(), tool, ops),
                    Interaction::Final,
                )?;
                p.dirty = true;
                if tool == "color_range" {
                    return Ok(json!({"created_mask":p.doc.head().masks.last().map(|m|m.id)}));
                }
            }
            "export_inputs" => {
                let mut inputs = self.export_inputs(&r)?;
                if r["preview"] == true {
                    if let Some(first) = inputs.as_array_mut().and_then(|items| items.first_mut()) {
                        let source: SourceRef = serde_json::from_value(first["source_ref"].clone())?;
                        let params: EditParams = serde_json::from_value(first["params"].clone())?;
                        match self.engine.thumbnail(source, Some(Arc::new(params)), 388, Priority(-10)).wait() {
                            Ok(image) => first["preview"] = json!({"width":image.pixels.width,
                                "height":image.pixels.height,"stride":image.pixels.stride,
                                "bgra":BASE64.encode(&image.pixels.data)}),
                            Err(error) => first["preview_error"] = json!(format!("{error:#}")),
                        }
                    }
                }
                return Ok(inputs);
            },
            "export" => {
                ensure!(self.export.is_none(), "An export is already running");
                let opts: ExportOptions = serde_json::from_value(r["options"].clone())?;
                ensure!(
                    r["overwrite"] == true || !opts.path.exists(),
                    "Destination exists; choose a new filename"
                );
                let source = if r.get("source_ref").is_some() {
                    serde_json::from_value::<SourceRef>(r["source_ref"].clone())?
                } else if let Some(path) = r["source"].as_str() {
                    SourceRef::File(PathBuf::from(path))
                } else {
                    self.photo
                        .as_ref()
                        .context("Open a photo first")?
                        .source
                        .clone()
                };
                let params = if r["params"].is_object() {
                    Arc::new(serde_json::from_value::<EditParams>(r["params"].clone())?)
                } else {
                    self.photo
                        .as_ref()
                        .context("Supply edit parameters or open a photo")?
                        .doc
                        .head()
                        .clone()
                };
                if let SourceRef::File(source) = &source {
                    ensure!(
                        opts.path != *source
                            && (opts.path.canonicalize().ok().is_none()
                                || opts.path.canonicalize().ok() != source.canonicalize().ok()),
                        "Export cannot replace the source photo"
                    );
                }
                self.export = Some(self.engine.export(source, params, opts));
                self.export_result = json!({"running":true,"job_id":r["job_id"]});
            }
            "cancel_export" => {
                self.export = None;
                self.export_result =
                    json!({"cancelled":true,"job_id":self.export_result["job_id"]});
            }
            "detect_display" => {
                self.display_detection = Some(display::Detection::start(self.engine.clone(), r["screen"].as_str().unwrap_or_default().to_owned()));
            }
            "state" => {
                if let Some(display) = &mut self.display_detection { display.poll(); }
                if let Some(job) = &mut self.model_discovery {
                    if job.state["running"] == true {
                        match job.result.try_recv() {
                            Ok(result) => {
                                job.state["running"] = json!(false);
                                match result {
                                    Ok(models) => job.state["models"] = json!(models),
                                    Err(error) => job.state["error"] = json!(error),
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                job.state["running"] = json!(false);
                                job.state["error"] = json!("Model discovery worker stopped");
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                    }
                }
                if let Some(task) = &mut self.agent {
                    while let Ok(event) = task.events.try_recv() {
                        match event {
                            aigle_agent::AgentEvent::Profile(profile) => {
                                task.state["message"] = json!(profile.explanation);
                                task.state["events"].as_array_mut().unwrap().push(json!({"role":"assistant","text":profile.explanation,"profile":profile.adjustments}));
                            }
                            aigle_agent::AgentEvent::Stats(stats) => {
                                task.state["stats"] = json!({"requests":stats.requests,"seconds":stats.elapsed_seconds,"input_tokens":stats.usage.input_tokens,"output_tokens":stats.usage.output_tokens,"cache_rate":stats.cache_rate()})
                            }
                            aigle_agent::AgentEvent::Preview(bytes) => {
                                task.state["events"].as_array_mut().unwrap().push(json!({"role":"assistant","text":"","image":BASE64.encode(bytes)}));
                            }
                        }
                    }
                    let completion = match task.result.try_recv() {
                        Ok(result) => Some(result),
                        Err(std::sync::mpsc::TryRecvError::Disconnected) if task.state["running"] == true => Some(Err("Agent worker stopped before returning a result".into())),
                        _ => None,
                    };
                    if let Some(result) = completion {
                        task.state["running"] = json!(false);
                        match result {
                            Ok((params, explanation)) if self.photo.as_ref().is_some_and(|p| p.doc.rev() == task.base_rev) => {
                                task.state["message"] = json!(explanation);
                                task.state["proposal"] = json!(true);
                                task.state["params"] = json!(params);
                                if let Some(p) = &self.photo {
                                    p.session.set_pending(
                                        Some((p.doc.rev().next(), Arc::new(params.clone()))),
                                        Interaction::Final,
                                    );
                                    p.view.set_params_source(ParamsSource::Pending);
                                }
                                task.candidate = Some(params);
                            }
                            Ok(_) => task.state["error"] = json!("The photo was edited during this run. Start a new run to propose edits from the current version."),
                            Err(error) => task.state["error"] = json!(error),
                        }
                    }
                }
                if let Some(task) = &mut self.agent {
                    if task.candidate.is_some() && self.photo.as_ref().is_none_or(|p| p.doc.rev() != task.base_rev) {
                        task.candidate = None;
                        task.state["proposal"] = json!(false);
                        task.state["error"] = json!("The photo changed after this proposal. Start a new run.");
                        if let Some(p) = &self.photo {
                            p.session.set_pending(None, Interaction::Final);
                            p.view.set_params_source(ParamsSource::Current);
                        }
                    }
                }
                if let Some(job) = &mut self.export {
                    if let Some(result) = job.now_or_never() {
                        let job_id = self.export_result["job_id"].clone();
                        self.export_result = match result {
                            Ok(v) => json!({"result":v}),
                            Err(e) => json!({"error":e.to_string()}),
                        };
                        self.export_result["job_id"] = job_id;
                        self.export = None;
                    }
                }
                let Some(p) = &mut self.photo else {
                    return Ok(
                        json!({"display_profile":self.display_detection.as_ref().map(|d|&d.state),"model_discovery":self.model_discovery.as_ref().map(|job| &job.state),"open":false,"export":self.export_result,"import":self.import.as_ref().map(ImportTask::status),"duplicates":self.duplicates.as_ref().map(ImportTask::status),"agent":self.agent.as_ref().map(|t|&t.state)}),
                    );
                };
                let load = p.info.borrow().clone();
                let extent = match &load {
                    LoadState::Ready(i) => Some(
                        aigle_engine::geometry_map(
                            i,
                            &if p.uncropped {
                                p.crop_preview.as_ref().unwrap_or(p.doc.head()).uncropped()
                            } else {
                                (**p.doc.head()).clone()
                            },
                        )
                        .crop_extent(),
                    ),
                    _ => None,
                };
                let mut state = json!({"display_profile":self.display_detection.as_ref().map(|d|&d.state),"photo_identity":p.identity,"retouch_preview":p.retouch_preview.is_some(),"document_id":p.document_id,"model_discovery":self.model_discovery.as_ref().map(|job| &job.state),"open":true,"source_ref":p.source,"load":load,"extent":extent,"revision":p.doc.rev(),"dirty":p.dirty,
                    "can_undo":p.doc.can_undo(),"can_redo":p.doc.can_redo(),"position":p.doc.position(),
                    "rating":p.meta.rating,"label":p.meta.label,"flag":p.meta.flag,"tags":p.meta.tags(),"iptc":p.meta.iptc,"export":self.export_result,
                    "copy":p.copy,"snapshots":p.meta.snapshots.iter().map(|s|s.name.as_str()).collect::<Vec<_>>(),"import":self.import.as_ref().map(ImportTask::status),"duplicates":self.duplicates.as_ref().map(ImportTask::status),"agent":self.agent.as_ref().map(|t|&t.state)});
                if r["revision"].as_u64() != Some(p.doc.rev().0) {
                    if let LoadState::Ready(info) = p.info.borrow().clone() {
                        let map = aigle_engine::geometry_map(&info, p.doc.head());
                        let size = map.source_size();
                        let project = |pt: Pt| {
                            map.source_to_crop(Point::new(
                                pt.x as f64 * size.w as f64,
                                pt.y as f64 * size.h as f64,
                            ))
                        };
                        state["retouch_guides"] = json!(retouch_edit::guides(p.doc.head(),&map));
                        state["mask_guides"] = json!(
                            p.doc
                                .head()
                                .masks
                                .iter()
                                .map(|mask| {
                                    let mut guide = json!({"id":mask.id});
                                    match &mask.shape {
                                        MaskShape::Linear { from, to } => {
                                            guide["kind"] = json!("linear");
                                            guide["from"] = json!(project(*from));
                                            guide["to"] = json!(project(*to));
                                        }
                                        MaskShape::Radial {
                                            center,
                                            radii,
                                            angle,
                                            ..
                                        } => {
                                            let a = (*angle as f64).to_radians();
                                            let ellipse = |t: f64| {
                                                let x = radii[0] as f64 * size.w as f64 * t.cos();
                                                let y = radii[1] as f64 * size.h as f64 * t.sin();
                                                project(Pt {
                                                    x: center.x
                                                        + ((x * a.cos() - y * a.sin())
                                                            / size.w as f64)
                                                            as f32,
                                                    y: center.y
                                                        + ((x * a.sin() + y * a.cos())
                                                            / size.h as f64)
                                                            as f32,
                                                })
                                            };
                                            guide["kind"] = json!("radial");
                                            guide["from"] = json!(project(*center));
                                            guide["to"] = json!(ellipse(0.));
                                            guide["axis_y"] =
                                                json!(ellipse(std::f64::consts::FRAC_PI_2));
                                            guide["outline"] = json!(
                                                (0..=64)
                                                    .map(|i| ellipse(
                                                        i as f64 * std::f64::consts::TAU / 64.
                                                    ))
                                                    .collect::<Vec<_>>()
                                            );
                                        }
                                        shape if mask_edit::movable(shape) => {
                                            let mut points = vec![];
                                            mask_edit::anchors(shape, &mut points);
                                            if !points.is_empty() {
                                                let center = Pt {
                                                    x: points.iter().map(|p| p.x).sum::<f32>()
                                                        / points.len() as f32,
                                                    y: points.iter().map(|p| p.y).sum::<f32>()
                                                        / points.len() as f32,
                                                };
                                                guide["kind"] = json!("move");
                                                guide["from"] = json!(project(center));
                                                guide["to"] = json!(project(center));
                                            }
                                        }
                                        _ => {}
                                    }
                                    guide["coverage"] =
                                        json!(aigle_engine::masks::geometric_thumbnail(
                                            p.doc.head(),
                                            mask.id,
                                            &map,
                                            32
                                        ));
                                    guide
                                })
                                .collect::<Vec<_>>()
                        );
                    }
                    state["params"] = json!(p.doc.head());
                    state["history"] = json!(
                        p.doc
                            .history()
                            .iter()
                            .map(|h| json!({"label":h.label,"origin":h.origin,"applied":h.applied}))
                            .collect::<Vec<_>>()
                    );
                }
                if p.histogram.has_changed().unwrap_or(false) {
                    state["histogram"] =
                        json!(p.histogram.borrow_and_update().as_ref().map(
                            |h| json!({"red":h.red,"green":h.green,"blue":h.blue,"luma":h.luma})
                        ));
                }
                return Ok(state);
            }
            other => bail!("Unknown command: {other}"),
        }
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stateless_crop_query_constrains_perspective_and_rotation() {
        use aigle_core::{crop::Frame, geometry::Size, params::{CropRect, GeometryParams}};
        let query = |request: Value| {
            let bytes = CString::new(request.to_string()).unwrap();
            let response = unsafe { aigle_crop_geometry(bytes.as_ptr()) };
            let value: Value = serde_json::from_str(unsafe { CStr::from_ptr(response) }.to_str().unwrap()).unwrap();
            unsafe { aigle_string_free(response) };
            value
        };
        for geometry in [json!({"angle":30}), json!({"perspective_vertical":40,"perspective_horizontal":-25,"angle":12})] {
            let g: GeometryParams = serde_json::from_value(geometry.clone()).unwrap();
            let frame = Frame::new(Size::new(1200,800), &g);
            let fitted = query(json!({"action":"fit","width":1200,"height":800,
                "geometry":geometry,"crop":{"x":0,"y":0,"w":1,"h":1}}));
            let crop: CropRect = serde_json::from_value(fitted["ok"].clone()).unwrap();
            assert!(crop.w < 1.0 || crop.h < 1.0);
            // Serialized f32 edges may differ by a fraction of a source pixel.
            let rect = frame.rect_of(&crop);
            let inset = aigle_core::geometry::Rect::new(rect.x+0.001,rect.y+0.001,rect.w-0.002,rect.h-0.002);
            assert!(frame.valid(inset));
            for handle in 0..=8 {
                let moved = query(json!({"action":"drag","width":1200,"height":800,
                    "geometry":geometry,"crop":crop,"handle":handle,"dx":0.8,"dy":-0.8}));
                let out: CropRect = serde_json::from_value(moved["ok"].clone()).unwrap();
                let rect = frame.rect_of(&out);
                assert!(rect.w > 0.0 && rect.h > 0.0);
                assert!(frame.valid(aigle_core::geometry::Rect::new(rect.x+0.001,rect.y+0.001,rect.w-0.002,rect.h-0.002)));
            }
        }
        assert!(query(json!({"width":0,"height":800})).get("error").is_some());
        let response = unsafe { aigle_crop_geometry(std::ptr::null()) };
        assert!(unsafe { CStr::from_ptr(response) }.to_str().unwrap().contains("error"));
        unsafe { aigle_string_free(response) };
    }

    unsafe fn call(editor: *mut AigleEditor, request: Value) -> Value {
        let request = CString::new(request.to_string()).unwrap();
        let response = unsafe { aigle_command(editor, request.as_ptr()) };
        let value: Value =
            serde_json::from_str(unsafe { CStr::from_ptr(response) }.to_str().unwrap()).unwrap();
        unsafe { aigle_string_free(response) };
        assert!(value.get("error").is_none(), "{value}");
        value["ok"].clone()
    }

    #[test]
    fn selected_copy_file_actions_validate_identity_and_allow_missing_originals() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path()}).to_string()).unwrap();
        unsafe {
            let mut error = ptr::null_mut();
            let editor = aigle_editor_new(config.as_ptr(), &mut error);
            assert!(!editor.is_null());
            assert!(error.is_null());
            let missing = dir.path().join("missing.png");
            let other = dir.path().join("other.png");
            let id = (*editor).catalog.create_copy(&missing, "Copy", &PhotoMeta::default()).unwrap();
            assert!((*editor).command(json!({"cmd":"file_action","action":"remove_copy","path":other,"copy":id})).is_err());
            let renamed = call(editor, json!({"cmd":"file_action","action":"rename_copy","path":missing,"copy":id,"destination":"Renamed"}));
            assert_eq!(renamed["ok"], true);
            assert_eq!((*editor).catalog.copies_of(std::slice::from_ref(&missing)).unwrap()[0].name, "Renamed");
            let removed = call(editor, json!({"cmd":"file_action","action":"remove_copy","path":missing,"copy":id}));
            assert_eq!(removed["ok"], true);
            assert!((*editor).catalog.copies_of(&[missing]).unwrap().is_empty());
            aigle_editor_free(editor);
        }
    }

    #[test]
    fn agent_events_and_stale_proposals_are_handled_without_live_provider() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(2), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let (events, rx) = futures::channel::mpsc::unbounded();
        let (tx, result) = std::sync::mpsc::channel();
        let base = (**editor.photo.as_ref().unwrap().doc.head()).clone();
        editor.agent = Some(AgentTask { cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            events:rx, result, base:base.clone(), base_rev:editor.photo.as_ref().unwrap().doc.rev(),
            candidate:None, state:json!({"running":true,"run_id":1,"events":[]}) });
        events.unbounded_send(aigle_agent::AgentEvent::Preview(vec![1,2,3])).unwrap();
        events.unbounded_send(aigle_agent::AgentEvent::Profile(aigle_agent::Profile {
            explanation:"Warm light".into(), adjustments: [("light.exposure".into(),json!(0.5))].into()
        })).unwrap();
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert_eq!(state["agent"]["events"][0]["image"], "AQID");
        assert_eq!(state["agent"]["events"][1]["profile"]["light.exposure"], 0.5);
        editor.command(json!({"cmd":"edit","ops":[{"op":"set","path":"light.exposure","value":1.0}]})).unwrap();
        tx.send(Ok((base,"Proposal".into()))).unwrap();
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert!(state["agent"]["error"].as_str().unwrap().contains("edited during"));
        assert_ne!(state["agent"]["proposal"], true);
        assert!(editor.command(json!({"cmd":"agent_accept"})).is_err());
        assert_eq!(editor.photo.as_ref().unwrap().doc.head().light.exposure, 1.0);
        let base = (**editor.photo.as_ref().unwrap().doc.head()).clone();
        let mut candidate = base.clone();
        candidate.light.exposure = 0.5;
        let (_events, rx) = futures::channel::mpsc::unbounded();
        let (tx, result) = std::sync::mpsc::channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        editor.agent = Some(AgentTask { cancel:cancel.clone(), events:rx, result, base,
            base_rev:editor.photo.as_ref().unwrap().doc.rev(), candidate:None,
            state:json!({"running":true,"run_id":2,"events":[]}) });
        tx.send(Ok((candidate,"Accepted proposal".into()))).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["agent"]["proposal"], true);
        editor.command(json!({"cmd":"agent_accept"})).unwrap();
        assert_eq!(editor.photo.as_ref().unwrap().doc.head().light.exposure, 0.5);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        editor.command(json!({"cmd":"undo"})).unwrap();
        assert_eq!(editor.photo.as_ref().unwrap().doc.head().light.exposure, 1.0);
    }

    #[test]
    fn color_mask_sampling_preserves_other_masks_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(2), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !matches!(*editor.photo.as_ref().unwrap().info.borrow(), LoadState::Ready(_)) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        editor.command(json!({"cmd":"tool","tool":"color_range","points":[{"x":100.,"y":100.}],"radius":0.01})).unwrap();
        let first = editor.photo.as_ref().unwrap().doc.head().masks[0].clone();
        let MaskShape::Range { range: aigle_core::masks::RangeKind::Color { samples, tolerance }, .. } = &first.shape else { panic!("Expected color range"); };
        assert_eq!(samples.len(), 1);
        assert_eq!(*tolerance, 20.0);
        editor.command(json!({"cmd":"tool","tool":"color_range","points":[{"x":600.,"y":400.}]})).unwrap();
        let second = editor.photo.as_ref().unwrap().doc.head().masks[1].clone();
        editor.command(json!({"cmd":"tool","tool":"color_pick","mask_id":first.id,"points":[{"x":900.,"y":600.}],"radius":0.01})).unwrap();
        let head = editor.photo.as_ref().unwrap().doc.head();
        assert_ne!(head.masks[0].shape, first.shape);
        assert_eq!(head.masks[1], second);
        editor.command(json!({"cmd":"undo"})).unwrap();
        assert_eq!(editor.photo.as_ref().unwrap().doc.head().masks[0], first);
        assert!(editor.command(json!({"cmd":"tool","tool":"color_pick","mask_id":99999,"points":[{"x":100.,"y":100.}]})).is_err());
    }

    #[test]
    fn imported_lut_is_validated_undoable_and_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(2), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let path = dir.path().join("identity.cube");
        std::fs::write(&path, r#"TITLE "Identity"
LUT_3D_SIZE 2
0 0 0
1 0 0
0 1 0
1 1 0
0 0 1
1 0 1
0 1 1
1 1 1
"#).unwrap();
        let reply = editor.command(json!({"cmd":"import_profile","kind":"lut","path":path})).unwrap();
        assert_eq!(reply["name"], "Identity");
        assert!(editor.photo.as_ref().unwrap().doc.head().profile.look.is_some());
        std::fs::remove_file(&path).unwrap();
        editor.command(json!({"cmd":"undo"})).unwrap();
        assert!(editor.photo.as_ref().unwrap().doc.head().profile.look.is_none());
        editor.command(json!({"cmd":"redo"})).unwrap();
        assert!(editor.photo.as_ref().unwrap().doc.head().profile.look.is_some());
        let revision = editor.photo.as_ref().unwrap().doc.rev();
        std::fs::write(&path, "invalid profile").unwrap();
        assert!(editor.command(json!({"cmd":"import_profile","kind":"dcp","path":path})).is_err());
        assert_eq!(editor.photo.as_ref().unwrap().doc.rev(), revision);
    }

    #[test]
    fn retouch_previews_cancel_commit_once_undo_and_reopen() {
        let dir=tempfile::tempdir().unwrap();
        let review=std::env::var_os("AIGLE_RETOUCH_REVIEW_DIR").map(PathBuf::from);
        let input_dir=review.as_deref().unwrap_or(dir.path());std::fs::create_dir_all(input_dir).unwrap();
        let input=input_dir.join("retouch.png");
        if input.exists(){std::fs::remove_file(&input).unwrap();}
        let config=CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig{backend:BackendPreference::Cpu,worker_threads:Some(2),..Default::default()}}).to_string()).unwrap();
        let mut error=ptr::null_mut();let raw=unsafe{aigle_editor_new(config.as_ptr(),&mut error)};assert!(!raw.is_null()&&error.is_null());let mut editor=unsafe{Box::from_raw(raw)};
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        editor.command(json!({"cmd":"export","options":ExportOptions{path:input.clone(),format:EncodedFormat::Png,long_edge:Some(600),..Default::default()}})).unwrap();
        let deadline=Instant::now()+Duration::from_secs(30);
        loop {let state=editor.command(json!({"cmd":"state"})).unwrap();assert!(state["export"].get("error").is_none(),"{state}");if state["export"]["result"].is_object(){break;}assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(10));}
        editor.command(json!({"cmd":"open","path":input})).unwrap();
        let deadline=Instant::now()+Duration::from_secs(30);
        loop {if matches!(*editor.photo.as_ref().unwrap().info.borrow(),LoadState::Ready(_)){break;}assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(10));}
        let spots=vec![Spot::new(SpotMode::Heal,aigle_core::masks::Pt{x:0.4,y:0.5},aigle_core::masks::Pt{x:0.2,y:0.5},0.035),Spot::new(SpotMode::Clone,aigle_core::masks::Pt{x:0.65,y:0.3},aigle_core::masks::Pt{x:0.8,y:0.3},0.025)];
        let eyes=vec![RedEye::new(aigle_core::masks::Pt{x:0.7,y:0.7},0.02)];
        editor.command(json!({"cmd":"edit","ops":[{"op":"set","path":"spots","value":spots},{"op":"set","path":"red_eye","value":eyes},{"op":"set","path":"geometry.angle","value":12.0}]})).unwrap();
        editor.command(json!({"cmd":"tool","tool":"clone","radius":0.01,"feather":27.,"opacity":0.4,"points":[{"x":100.,"y":100.},{"x":150.,"y":100.}]})).unwrap();
        let added=editor.photo.as_ref().unwrap().doc.head().spots.last().unwrap().clone();assert_eq!(added.feather,27.);assert_eq!(added.opacity,0.4);
        editor.command(json!({"cmd":"undo"})).unwrap();
        editor.command(json!({"cmd":"tool","tool":"red_eye","radius":0.01,"pupil":31.,"darken":73.,"points":[{"x":100.,"y":100.}]})).unwrap();
        let added=editor.photo.as_ref().unwrap().doc.head().red_eye.last().unwrap().clone();assert_eq!(added.pupil,31.);assert_eq!(added.darken,73.);
        editor.command(json!({"cmd":"undo"})).unwrap();
        editor.command(json!({"cmd":"save"})).unwrap();
        let state=editor.command(json!({"cmd":"state"})).unwrap();assert_eq!(state["retouch_guides"].as_array().unwrap().len(),3);
        let before=(**editor.photo.as_ref().unwrap().doc.head()).clone();let revision=editor.photo.as_ref().unwrap().doc.rev();let doc=editor.photo.as_ref().unwrap().document_id;
        fn full_pixels(editor:&AigleEditor,rev:aigle_core::Revision)->Arc<[u8]> {
            let deadline=Instant::now()+Duration::from_secs(30);
            loop {
                if let Some(frame)=editor.photo.as_ref().unwrap().frames.borrow().clone() {
                    if frame.rev==rev && frame.quality==Quality::Full {return frame.pixels.data.clone();}
                }
                assert!(Instant::now()<deadline,"Retouch render timed out for {rev:?}");std::thread::sleep(Duration::from_millis(10));
            }
        }
        let original_pixels=full_pixels(&editor,revision);
        let pixel_preview=json!({"cmd":"retouch_preview","document_id":doc,"revision":revision.0,"session":"pixel-preview","action":"set","kind":"spot","index":1,"expected":before.spots[1],"field":"opacity","value":0.0});
        editor.command(pixel_preview).unwrap();
        let pending_revision=aigle_core::Revision(editor.photo.as_ref().unwrap().crop_preview_revision);
        let preview_pixels=full_pixels(&editor,pending_revision);
        assert_ne!(blake3::hash(&original_pixels),blake3::hash(&preview_pixels),"Pending retouch must change the actual rendered pixels");
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["retouch_preview"],true);
        editor.command(json!({"cmd":"retouch_cancel","session":"pixel-preview"})).unwrap();
        assert_eq!(blake3::hash(&full_pixels(&editor,revision)),blake3::hash(&original_pixels));
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["retouch_preview"],false);
        let placement=editor.command(json!({"cmd":"retouch_placement","document_id":doc,"revision":revision.0,"session":"placement","tool":"clone","radius":0.03,"points":[{"x":150.,"y":120.},{"x":200.,"y":120.}]})).unwrap();
        assert_eq!(placement["placement"],true);assert_eq!(placement["guide"]["outline"].as_array().unwrap().len(),65);
        assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),before);
        let mut r=json!({"cmd":"retouch_preview","document_id":doc,"revision":revision.0,"session":"gesture","action":"set","kind":"spot","index":0,"expected":before.spots[0],"field":"radius","value":0.05});
        for v in [0.04,0.06,0.05]{r["value"]=json!(v);let result=editor.command(r.clone()).unwrap();assert_eq!(result["guide"]["value"]["radius"],json!(v as f32));}
        assert_eq!(editor.photo.as_ref().unwrap().doc.rev(),revision);assert!(!editor.photo.as_ref().unwrap().dirty);
        editor.command(json!({"cmd":"retouch_cancel","session":"other"})).unwrap();assert!(editor.photo.as_ref().unwrap().retouch_preview.is_some());
        editor.command(json!({"cmd":"retouch_cancel","session":"gesture"})).unwrap();assert!(editor.photo.as_ref().unwrap().retouch_preview.is_none());assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),before);
        editor.command(r.clone()).unwrap();r["cmd"]=json!("retouch_commit");editor.command(r.clone()).unwrap();
        let after=(**editor.photo.as_ref().unwrap().doc.head()).clone();assert_eq!(after.spots[0].radius,0.05);assert_eq!(after.spots[0].source,before.spots[0].source);assert_eq!(after.red_eye,before.red_eye);
        editor.command(json!({"cmd":"undo"})).unwrap();assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),before);
        editor.command(json!({"cmd":"redo"})).unwrap();assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),after);
        assert!(editor.command(r.clone()).is_err());r["revision"]=json!(editor.photo.as_ref().unwrap().doc.rev().0);r["document_id"]=json!(doc+99);assert!(editor.command(r.clone()).is_err());
        editor.command(json!({"cmd":"save"})).unwrap();editor.command(json!({"cmd":"close"})).unwrap();editor.command(json!({"cmd":"open","path":input})).unwrap();
        assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),after);
        assert_ne!(editor.photo.as_ref().unwrap().document_id,doc);
        let p=editor.photo.as_ref().unwrap();
        let delete=json!({"cmd":"retouch_commit","document_id":p.document_id,"revision":p.doc.rev().0,"session":"delete-eye","action":"delete","kind":"eye","index":0,"expected":after.red_eye[0]});
        editor.command(delete).unwrap();assert!(editor.photo.as_ref().unwrap().doc.head().red_eye.is_empty());assert_eq!(editor.photo.as_ref().unwrap().doc.head().spots,after.spots);
        editor.command(json!({"cmd":"undo"})).unwrap();assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),after);
        let p=editor.photo.as_ref().unwrap();editor.command(json!({"cmd":"retouch_commit","document_id":p.document_id,"revision":p.doc.rev().0,"session":"clear","action":"clear"})).unwrap();
        assert!(editor.photo.as_ref().unwrap().doc.head().spots.is_empty());assert!(editor.photo.as_ref().unwrap().doc.head().red_eye.is_empty());
        editor.command(json!({"cmd":"undo"})).unwrap();assert_eq!(**editor.photo.as_ref().unwrap().doc.head(),after);
        editor.command(json!({"cmd":"save"})).unwrap();
    }

    #[test]
    fn scopes_measure_rendered_sdr_hdr_and_regions_without_editing() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(2), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        assert_eq!(editor.command(json!({"cmd":"scopes","request_id":"empty"})).unwrap()["available"],false);
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let whole = loop {
            let result = editor.command(json!({"cmd":"scopes","request_id":"whole"})).unwrap();
            if result["available"] == true { break result; }
            assert!(Instant::now() < deadline, "Histogram timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(whole["request_id"], "whole");
        assert_eq!(whole["data"]["hdr"],false);
        assert!(whole["data"]["count"].as_u64().unwrap() > 0);
        let before = editor.photo.as_ref().unwrap().doc.rev();
        assert_eq!(whole["data"]["bins"][0], json!(editor.photo.as_ref().unwrap().histogram.borrow().as_ref().unwrap().red));
        let region = editor.command(json!({"cmd":"scopes","request_id":"region","roi":[0.,0.,0.5,0.5],"target":2})).unwrap();
        assert!(region["data"]["count"].as_u64().unwrap() < whole["data"]["count"].as_u64().unwrap());
        assert_eq!(editor.photo.as_ref().unwrap().doc.rev(),before);
        assert!(editor.command(json!({"cmd":"scopes","request_id":"bad","roi":[0.5,0.,0.1,1.]})).is_err());
        assert!(editor.command(json!({"cmd":"scopes","request_id":"bad","target":42})).is_err());
        editor.command(json!({"cmd":"viewport","viewport":{"region":{"x":0.,"y":0.,"w":1200.,"h":800.},"output_size":[120,80],"display":"HdrRec2020"}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let result = editor.command(json!({"cmd":"scopes","request_id":"hdr"})).unwrap();
            if result["data"]["hdr"] == true {
                assert!(result["diffuse_white"].as_f64().unwrap() > 0.5);
                assert_eq!(result["targets"].as_array().unwrap().len(),6);
                break;
            }
            assert!(Instant::now() < deadline,"HDR histogram timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(editor.photo.as_ref().unwrap().doc.rev(),before);
    }

    #[test]
    fn model_discovery_does_not_block_editor_commands() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" { break; }
            }
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let body = r#"{"object":"list","data":[{"id":"vision-a"}],"has_more":false}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"agent_models","request_id":"async-test",
            "endpoint":format!("http://{address}/v1"),"api_key":"test-key"})).unwrap();
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["model_discovery"]["running"], true);
        assert!(editor.command(json!({"cmd":"schema"})).unwrap().is_object());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = editor.command(json!({"cmd":"state"})).unwrap();
            if state["model_discovery"]["running"] == false {
                assert_eq!(state["model_discovery"]["models"], json!(["vision-a"]));
                break;
            }
            assert!(Instant::now() < deadline, "Discovery did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        server.join().unwrap();
    }

    #[test]
    fn discovery_state_and_cancellation_work_without_an_open_photo() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        let (tx, result) = std::sync::mpsc::channel();
        editor.model_discovery = Some(ModelDiscovery {
            result, state: json!({"request_id":"one","running":true}),
        });
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert_eq!(state["open"], false);
        assert_eq!(state["model_discovery"]["running"], true);
        editor.command(json!({"cmd":"agent_models_cancel","request_id":"other"})).unwrap();
        assert!(editor.model_discovery.is_some());
        tx.send(Ok(vec!["vision-model".into()])).unwrap();
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert_eq!(state["model_discovery"]["models"], json!(["vision-model"]));
        assert_eq!(state["model_discovery"]["running"], false);
        editor.command(json!({"cmd":"agent_models_cancel","request_id":"one"})).unwrap();
        assert!(editor.model_discovery.is_none());
        let (tx, result) = std::sync::mpsc::channel();
        editor.model_discovery = Some(ModelDiscovery {
            result, state: json!({"request_id":"two","running":true}),
        });
        tx.send(Err("Unauthorized".into())).unwrap();
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert_eq!(state["model_discovery"]["error"], "Unauthorized");
        let (tx, result) = std::sync::mpsc::channel();
        editor.model_discovery = Some(ModelDiscovery {
            result, state: json!({"request_id":"three","running":true}),
        });
        drop(tx);
        let state = editor.command(json!({"cmd":"state"})).unwrap();
        assert_eq!(state["model_discovery"]["error"], "Model discovery worker stopped");
    }

    #[test]
    fn coverage_swatches_use_opacity_inversion_and_decline_image_dependent_shapes() {
        let mut params = EditParams::default();
        let id = MaskId(1);
        params.masks.push(LocalAdjustment {
            id,
            name: "Test".into(),
            opacity: 0.5,
            invert: false,
            adjust: LocalParams::default(),
            shape: MaskShape::Linear {
                from: Pt { x: 0., y: 0. },
                to: Pt { x: 1., y: 0. },
            },
        });
        let map = aigle_core::geometry::GeometryMap::new(
            aigle_core::geometry::Size::new(1200, 800),
            &params.geometry,
        );
        let normal = aigle_engine::masks::geometric_thumbnail(&params, id, &map, 32).unwrap();
        assert!(normal[0] > 120 && normal[31] < 5);
        params.masks[0].invert = true;
        let inverted = aigle_engine::masks::geometric_thumbnail(&params, id, &map, 32).unwrap();
        for (a, b) in normal.iter().zip(inverted) {
            assert!((i32::from(*a) + i32::from(b) - 128).abs() <= 1);
        }
        params.masks[0].shape = MaskShape::Range {
            range: aigle_core::masks::RangeKind::Luminance {
                low: 0.2,
                high: 0.7,
            },
            smoothness: 10.,
        };
        assert!(aigle_engine::masks::geometric_thumbnail(&params, id, &map, 32).is_none());
    }

    #[test]
    fn comparison_display_detection_is_independent_and_cleared_on_close() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        let (_main_tx, main_rx) = std::sync::mpsc::channel();
        editor.display_detection = Some(display::Detection { result: main_rx,
            state: json!({"screen":"main","running":false,"profile":"Srgb"}) });
        let (tx, result) = std::sync::mpsc::channel();
        editor.comparison_display_detection = Some(display::Detection { result,
            state: json!({"screen":"survey","running":true}) });
        tx.send(json!({"screen":"survey","running":false,"profile":{"Icc":"fixture"}})).unwrap();
        let state = editor.command(json!({"cmd":"comparison_display_state"})).unwrap();
        assert_eq!(state["screen"], "survey");
        assert_eq!(state["profile"], json!({"Icc":"fixture"}));
        assert_eq!(editor.display_detection.as_ref().unwrap().state["screen"], "main");
        editor.command(json!({"cmd":"comparison_close"})).unwrap();
        assert!(editor.command(json!({"cmd":"comparison_display_state"})).unwrap().is_null());
        assert!(editor.display_detection.is_some());
    }

    #[test]
    fn geometry_edits_constrain_crop_and_restore_gesture_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !matches!(*editor.photo.as_ref().unwrap().info.borrow(), LoadState::Ready(_)) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let before = editor.command(json!({"cmd":"state"})).unwrap()["params"].clone();
        for (angle, continuous) in [(30, true), (0, true), (20, false)] {
            editor.command(json!({"cmd":"edit","gesture":123,"continuous":continuous,
                "ops":[{"op":"set","path":"geometry.angle","value":angle}]})).unwrap();
            let state = editor.command(json!({"cmd":"state"})).unwrap();
            let crop = &state["params"]["geometry"]["crop"];
            if angle == 0 { assert_eq!(crop, &before["geometry"]["crop"]); }
            else { assert!(crop["w"].as_f64().unwrap() < 1.0); }
        }
        let after = editor.command(json!({"cmd":"state"})).unwrap()["params"].clone();
        editor.command(json!({"cmd":"undo"})).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["params"], before);
        editor.command(json!({"cmd":"redo"})).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["params"], after);
        editor.command(json!({"cmd":"edit","ops":[
            {"op":"set","path":"geometry.angle","value":0},
            {"op":"set","path":"geometry.crop","value":{"x":0,"y":0,"w":1,"h":1}}]})).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["params"]["geometry"]["crop"], before["geometry"]["crop"]);
        editor.command(json!({"cmd":"edit","ops":[
            {"op":"set","path":"geometry.perspective_vertical","value":40},
            {"op":"set","path":"geometry.constrain","value":false}]})).unwrap();
        let p = editor.photo.as_ref().unwrap();
        let LoadState::Ready(info) = p.info.borrow().clone() else { panic!("loaded") };
        let frame = aigle_core::crop::Frame::new(aigle_engine::geometry_map(&info,p.doc.head()).source_size(), &p.doc.head().geometry);
        let r = frame.rect_of(&p.doc.head().geometry.crop);
        assert!(r.w < frame.w || r.h < frame.h);
        assert!(frame.valid(Rect::new(r.x+0.001,r.y+0.001,r.w-0.002,r.h-0.002)));
    }

    #[test]
    fn raw_dng_fixture_renders_reopens_and_rejects_reconversion() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.dng");
        std::fs::write(&source, include_bytes!("../../../thesauros/server/tests/fixtures/synthetic.dng")).unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        let wait_loaded = |editor: &AigleEditor| {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match editor.photo.as_ref().unwrap().info.borrow().clone() {
                    LoadState::Ready(info) => return info,
                    LoadState::Failed(error) => panic!("DNG load failed: {error}"),
                    LoadState::Loading => {}
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        editor.command(json!({"cmd":"open","path":source})).unwrap();
        let info = wait_loaded(&editor);
        assert_eq!(info.kind, SourceKind::Raw);
        // First TIFF IFD is the 64x48 thumbnail; the RAW plane is 256x256.
        assert_eq!((info.source_size.w, info.source_size.h), (256,256));
        for (extension, format) in [("png", EncodedFormat::Png), ("dng", EncodedFormat::Dng)] {
            let output = dir.path().join(format!("export.{extension}"));
            editor.command(json!({"cmd":"export","options":ExportOptions{
                path:output.clone(),format,..Default::default()}})).unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let state = editor.command(json!({"cmd":"state"})).unwrap();
                if extension == "dng" {
                    if let Some(error) = state["export"]["error"].as_str() {
                        assert!(error.contains("already a DNG"), "{error}");
                        break;
                    }
                } else {
                    assert!(state["export"].get("error").is_none(), "{state}");
                    if state["export"].get("result").is_some() { break; }
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            if extension == "png" { assert!(std::fs::metadata(&output).unwrap().len() > 0); }
            else { assert!(!output.exists()); }
        }
        editor.command(json!({"cmd":"open","path":dir.path().join("export.png")})).unwrap();
        let rendered = wait_loaded(&editor);
        assert_eq!(rendered.kind, SourceKind::Rendered);
        assert_eq!((rendered.source_size.w, rendered.source_size.h), (256,256));
        editor.command(json!({"cmd":"open","path":source})).unwrap();
        assert_eq!(wait_loaded(&editor).kind, SourceKind::Raw);
    }

    #[test]
    fn curve_preview_gestures_undo_and_redo_as_single_edits() {
        let dir = tempfile::tempdir().unwrap();
        let config = CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig {
            backend: BackendPreference::Cpu, worker_threads: Some(1), ..Default::default()
        }}).to_string()).unwrap();
        let mut error = ptr::null_mut();
        let raw = unsafe { aigle_editor_new(config.as_ptr(), &mut error) };
        assert!(!raw.is_null() && error.is_null());
        let mut editor = unsafe { Box::from_raw(raw) };
        editor.command(json!({"cmd":"synthetic"})).unwrap();
        let before = editor.command(json!({"cmd":"state"})).unwrap()["params"].clone();
        let initial_preview = editor.command(json!({"cmd":"export_inputs","current":true,"preview":true})).unwrap();
        assert!(initial_preview[0]["preview_error"].is_null(), "{initial_preview}");
        assert!(initial_preview[0]["preview"]["width"].as_u64().unwrap() <= 388);
        assert!(!initial_preview[0]["preview"]["bgra"].as_str().unwrap().is_empty());
        let points = |y| json!([{"x":0.0,"y":0.0},{"x":0.5,"y":y},{"x":1.0,"y":1.0}]);
        for (gesture, channel, end) in [(101, "rgb", 0.75), (102, "red", 0.25)] {
            for (value, continuous) in [(0.5, true), (0.625, true), (end, true), (end, false)] {
                editor.command(json!({"cmd":"edit","label":"Curve","gesture":gesture,
                    "continuous":continuous,"ops":[{"op":"set",
                        "path":format!("curve.point.{channel}"),"value":points(value)}]})).unwrap();
                let state = editor.command(json!({"cmd":"state"})).unwrap();
                assert_eq!(state["params"]["curve"]["point"][channel], points(value));
            }
        }
        let edited = editor.command(json!({"cmd":"state"})).unwrap()["params"].clone();
        let edited_preview = editor.command(json!({"cmd":"export_inputs","current":true,"preview":true})).unwrap();
        assert_eq!(edited_preview[0]["params"], edited);
        assert_ne!(edited_preview[0]["preview"]["bgra"], initial_preview[0]["preview"]["bgra"]);
        editor.command(json!({"cmd":"undo"})).unwrap();
        let one_undo = editor.command(json!({"cmd":"state"})).unwrap()["params"].clone();
        assert_eq!(one_undo["curve"]["point"]["red"], before["curve"]["point"]["red"]);
        assert_eq!(one_undo["curve"]["point"]["rgb"], points(0.75));
        editor.command(json!({"cmd":"undo"})).unwrap();
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["params"], before);
        for _ in 0..2 { editor.command(json!({"cmd":"redo"})).unwrap(); }
        assert_eq!(editor.command(json!({"cmd":"state"})).unwrap()["params"], edited);
    }

    #[test]
    fn retained_frames_edits_history_export_and_sidecar_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config=CString::new(json!({"data_dir":dir.path(),"engine":EngineConfig{backend:BackendPreference::Cpu,worker_threads:Some(2),..Default::default()}}).to_string()).unwrap();
        unsafe {
            let mut error = ptr::null_mut();
            let editor = aigle_editor_new(config.as_ptr(), &mut error);
            assert!(!editor.is_null());
            assert!(error.is_null());
            call(editor, json!({"cmd":"synthetic"}));
            let wait_frame = |format| {
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let frame = aigle_frame_take(editor, 0);
                    if !frame.is_null() {
                        let pixels = aigle_frame_pixels(frame);
                        if pixels.format == format {
                            return frame;
                        }
                        aigle_frame_free(frame);
                    }
                    assert!(Instant::now() < deadline, "frame timed out");
                    std::thread::sleep(Duration::from_millis(10));
                }
            };
            let sdr = wait_frame(0);
            let initial = aigle_frame_pixels(sdr);
            assert_eq!(
                initial.length,
                initial.stride as usize * initial.height as usize
            );
            call(
                editor,
                json!({"cmd":"edit","label":"Exposure","ops":[{"op":"set","path":"light.exposure","value":1.25}]}),
            );
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"],
                1.25
            );
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"],
                0.0
            );
            call(editor, json!({"cmd":"redo"}));
            call(
                editor,
                json!({"cmd":"viewport","viewport":{"region":{"x":0.0,"y":0.0,"w":1200.0,"h":800.0},"output_size":[120,80],"display":"HdrRec2020"}}),
            );
            let hdr = wait_frame(1);
            let hp = aigle_frame_pixels(hdr);
            assert_eq!(hp.stride, hp.width * 8);
            call(
                editor,
                json!({"cmd":"tool","tool":"radial","points":[{"x":300.0,"y":200.0},{"x":500.0,"y":350.0}]}),
            );
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["masks"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            let original_mask = call(editor, json!({"cmd":"state"}))["params"]["masks"][0].clone();
            let guide = call(editor, json!({"cmd":"state"}))["mask_guides"][0].clone();
            let mut moved = guide.clone();
            moved["from"]["x"] = json!(guide["from"]["x"].as_f64().unwrap() + 60.0);
            moved["to"]["x"] = json!(guide["to"]["x"].as_f64().unwrap() + 60.0);
            call(
                editor,
                json!({"cmd":"tool","tool":"mask_transform","handle":"move","id":guide["id"],"points":[guide["from"],moved["from"]]}),
            );
            let moved_mask = call(editor, json!({"cmd":"state"}))["params"]["masks"][0].clone();
            assert_eq!(moved_mask["id"], original_mask["id"]);
            assert!((moved_mask["shape"]["center"]["x"].as_f64().unwrap() - 0.3).abs() < 1e-5);
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["masks"][0],
                original_mask
            );
            let mut rotated_mask = original_mask.clone();
            rotated_mask["shape"]["angle"] = json!(37.0);
            call(
                editor,
                json!({"cmd":"edit","label":"Rotated perspective mask","ops":[
                    {"op":"set","path":"masks","value":[rotated_mask]},
                    {"op":"set","path":"geometry.angle","value":12.0},
                    {"op":"set","path":"geometry.perspective_vertical","value":20.0}
                ]}),
            );
            let projected = call(editor, json!({"cmd":"state"}))["mask_guides"][0].clone();
            assert_eq!(projected["outline"].as_array().unwrap().len(), 65);
            assert_eq!(projected["coverage"].as_array().unwrap().len(), 1024);
            // Inverse-mapping the projected radius handle must recover the original
            // radius even with a rotated ellipse and transformed photo.
            call(
                editor,
                json!({"cmd":"tool","tool":"mask_transform","handle":"radius_x",
                "id":projected["id"],"points":[projected["from"],projected["to"]]}),
            );
            let resized = call(editor, json!({"cmd":"state"}))["params"]["masks"][0].clone();
            assert!(
                (resized["shape"]["radii"][0].as_f64().unwrap()
                    - rotated_mask["shape"]["radii"][0].as_f64().unwrap())
                .abs()
                    < 1e-5
            );
            assert_eq!(resized["shape"]["angle"], rotated_mask["shape"]["angle"]);
            assert_eq!(
                resized["shape"]["radii"][1],
                rotated_mask["shape"]["radii"][1]
            );
            // A no-op edit may be coalesced; restore the fixture explicitly, then
            // keep the existing undo/export checks independent of that policy.
            call(
                editor,
                json!({"cmd":"edit","label":"Restore geometry","ops":[
                    {"op":"set","path":"masks","value":[original_mask]},
                    {"op":"set","path":"geometry.angle","value":0.0},
                    {"op":"set","path":"geometry.perspective_vertical","value":0.0}
                ]}),
            );
            let before_crop = call(editor, json!({"cmd":"state"}))["params"].clone();
            call(editor, json!({"cmd":"tool_mode","tool":"crop"}));
            call(
                editor,
                json!({"cmd":"tool","tool":"crop","angle":5.0,"aspect":{"kind":"ratio","w":4,"h":5},"points":[{"x":60.0,"y":40.0},{"x":1140.0,"y":760.0}]}),
            );
            let cropped = call(editor, json!({"cmd":"state"}));
            assert_eq!(cropped["params"]["geometry"]["angle"], 5.0);
            assert_eq!(cropped["params"]["geometry"]["aspect"], json!({"kind":"ratio","w":4,"h":5}));
            assert!(cropped["params"]["geometry"]["crop"]["w"].as_f64().unwrap() < 1.0);
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                before_crop,
                "crop, aspect and angle must undo together"
            );
            let revision = call(editor, json!({"cmd":"state"}))["revision"].clone();
            let orientation = json!({"quarter_turns":1,"flip":true});
            call(
                editor,
                json!({"cmd":"crop_orientation","orientation":orientation}),
            );
            let preview = call(editor, json!({"cmd":"state"}));
            assert_eq!(
                preview["revision"], revision,
                "preview must not enter history"
            );
            assert_eq!(
                preview["params"], before_crop,
                "preview must not modify saved parameters"
            );
            assert_eq!(preview["extent"], json!([800.0, 1200.0]));
            call(editor, json!({"cmd":"tool_mode","tool":"pan"}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"], before_crop);
            call(editor, json!({"cmd":"tool_mode","tool":"crop"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["extent"],
                json!([1200.0, 800.0])
            );
            call(
                editor,
                json!({"cmd":"crop_orientation","orientation":orientation}),
            );
            call(
                editor,
                json!({"cmd":"tool","tool":"crop","orientation":orientation,"angle":3.0,
                "normalized":true,"points":[{"x":0.1,"y":0.1},{"x":0.9,"y":0.9}]}),
            );
            let applied = call(editor, json!({"cmd":"state"}));
            assert_eq!(
                applied["params"]["geometry"]["orientation_delta"],
                orientation
            );
            assert_eq!(applied["params"]["geometry"]["angle"], 3.0);
            call(editor, json!({"cmd":"tool_mode","tool":"pan"}));
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                before_crop,
                "orientation, angle and crop must undo together"
            );
            call(editor, json!({"cmd":"undo"}));
            let output = dir.path().join("photo.png");
            call(
                editor,
                json!({"cmd":"export","options":ExportOptions{path:output.clone(),format:EncodedFormat::Png,long_edge:Some(120),..Default::default()}}),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let state = call(editor, json!({"cmd":"state"}));
                assert!(state["export"].get("error").is_none(), "{state}");
                if state["export"].get("result").is_some() {
                    break;
                }
                assert!(Instant::now() < deadline, "export timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            let saved = call(editor, json!({"cmd":"state"}));
            call(editor, json!({"cmd":"close"}));
            let recovered_output = dir.path().join("recovered.png");
            call(
                editor,
                json!({"cmd":"export","job_id":"recovered-job","source_ref":saved["source_ref"],"params":saved["params"],
                "options":ExportOptions{path:recovered_output.clone(),format:EncodedFormat::Png,long_edge:Some(120),..Default::default()}}),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let state = call(editor, json!({"cmd":"state"}));
                assert_eq!(state["open"], false);
                assert_eq!(state["export"]["job_id"], "recovered-job");
                assert!(state["export"].get("error").is_none(), "{state}");
                if state["export"].get("result").is_some() {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(recovered_output.is_file());
            assert!((*editor).command(json!({"cmd":"export","job_id":"collision","source_ref":saved["source_ref"],"params":saved["params"],
                "options":ExportOptions{path:recovered_output.clone(),format:EncodedFormat::Png,..Default::default()}})).is_err());
            std::fs::remove_file(recovered_output).unwrap();
            call(editor, json!({"cmd":"open","path":output}));
            let before_saved_crop = call(editor, json!({"cmd":"state"}))["params"].clone();
            aigle_frame_free(wait_frame(0));
            call(editor, json!({"cmd":"tool_mode","tool":"crop"}));
            call(editor, json!({"cmd":"tool","tool":"crop","normalized":true,
                "aspect":{"kind":"ratio","w":7,"h":5},
                "points":[{"x":0.1,"y":0.1},{"x":0.8,"y":0.85}]}));
            call(editor, json!({"cmd":"tool_mode","tool":"pan"}));
            let saved_crop = call(editor, json!({"cmd":"state"}))["params"].clone();
            call(editor, json!({"cmd":"save"}));
            call(editor, json!({"cmd":"close"}));
            call(editor, json!({"cmd":"viewport","viewport":{
                "region":{"x":0.,"y":0.,"w":120.,"h":80.},
                "output_size":[120,80],"display":"Srgb"}}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["open"], false);
            call(editor, json!({"cmd":"open","path":output}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"], saved_crop,
                "custom crop aspect and bounds must survive sidecar reload");
            assert_eq!(saved_crop["geometry"]["aspect"], json!({"kind":"ratio","w":7,"h":5}));
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"], before_saved_crop);
            call(editor, json!({"cmd":"redo"}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"], saved_crop);
            call(editor, json!({"cmd":"undo"}));
            call(
                editor,
                json!({"cmd":"edit","label":"Exposure","ops":[{"op":"set","path":"light.exposure","value":0.75}]}),
            );
            call(
                editor,
                json!({"cmd":"metadata","rating":4,"tags":["Qt migration"]}),
            );
            call(editor, json!({"cmd":"save"}));
            call(editor, json!({"cmd":"close"}));
            call(editor, json!({"cmd":"open","path":output}));
            let state = call(editor, json!({"cmd":"state"}));
            assert_eq!(state["params"]["light"]["exposure"], 0.75);
            assert_eq!(state["rating"], 4);
            let preview_path = CString::new(output.to_str().unwrap()).unwrap();
            let mut preview_error = ptr::null_mut();
            let preview =
                aigle_comparison_preview(editor, preview_path.as_ptr(), 256, &mut preview_error);
            assert!(!preview.is_null());
            assert!(preview_error.is_null());
            let pixels = aigle_frame_pixels(preview);
            assert!(pixels.width <= 256 && pixels.height <= 256);
            assert_eq!(pixels.format, 0);
            let original = aigle_thumbnail(editor, preview_path.as_ptr(), 256, &mut preview_error);
            assert!(!original.is_null());
            let original_pixels = aigle_frame_pixels(original);
            assert_ne!(
                std::slice::from_raw_parts(pixels.data, pixels.length),
                std::slice::from_raw_parts(original_pixels.data, original_pixels.length),
                "Comparison preview must apply the saved exposure edit"
            );
            aigle_frame_free(original);
            aigle_frame_free(preview);
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                state["params"]
            );

            assert_eq!(state["tags"], json!(["Qt migration"]));
            assert!(state["can_undo"].as_bool().unwrap());
            let second = dir.path().join("second.png");
            std::fs::copy(&output, &second).unwrap();
            let missing = dir.path().join("missing.png");
            let batch = call(
                editor,
                json!({"cmd":"batch_metadata","paths":[output,second,output,missing],
                "rating":5,"flag":"picked","label":"green"}),
            );
            assert_eq!(batch["updated"].as_array().unwrap().len(), 2);
            assert_eq!(batch["failures"].as_array().unwrap().len(), 1);
            assert_eq!(batch["failures"][0]["path"], json!(missing));
            let current = call(editor, json!({"cmd":"state"}));
            assert_eq!(current["rating"], 5);
            assert_eq!(current["flag"], "picked");
            assert_eq!(current["label"], "green");
            assert_eq!(current["params"], state["params"]);
            assert_eq!(current["tags"], state["tags"]);
            call(editor, json!({"cmd":"open","path":second}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["rating"], 5);
            call(editor, json!({"cmd":"open","path":output}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["tags"], state["tags"]);
            call(
                editor,
                json!({"cmd":"metadata","iptc":{"title":"Keep title"}}),
            );
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[output,second],
                "tags":[" Travel ", "travel", "Places|France"],"tag_mode":"add",
                "iptc":{"caption":"Shared caption"}}),
            );
            let current = call(editor, json!({"cmd":"state"}));
            assert_eq!(current["iptc"]["title"], "Keep title");
            assert_eq!(current["iptc"]["caption"], "Shared caption");
            let metadata = call(editor, json!({"cmd":"library_metadata","path":output}));
            assert_eq!(metadata["meta_loaded"], true);
            assert_eq!(metadata["rating"], 5);
            assert_eq!(metadata["iptc"]["title"], "Keep title");
            assert!(
                metadata["keywords"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("France"))
            );
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                state["params"]
            );

            assert!(
                current["tags"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("Qt migration"))
            );
            assert_eq!(current["tags"].as_array().unwrap().len(), 3);
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[output,second],
                "tags":["TRAVEL","Places > France"],"tag_mode":"remove"}),
            );
            assert_eq!(call(editor, json!({"cmd":"state"}))["tags"], state["tags"]);
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[second],
                "tags":["Replacement"],"tag_mode":"replace","iptc":{"caption":""}}),
            );
            call(editor, json!({"cmd":"open","path":second}));
            let replaced = call(editor, json!({"cmd":"state"}));
            assert_eq!(replaced["tags"], json!(["Replacement"]));
            assert!(replaced["iptc"].get("caption").is_none());
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[second],
                "tags":[],"tag_mode":"replace"}),
            );
            assert_eq!(call(editor, json!({"cmd":"state"}))["tags"], json!([]));
            call(editor, json!({"cmd":"open","path":output}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                state["params"]
            );
            call(editor, json!({"cmd":"snapshot","name":"Saved look"}));
            let copy = call(editor, json!({"cmd":"create_copy","name":"Alternative"}));
            call(
                editor,
                json!({"cmd":"open","path":output,"copy":copy["id"]}),
            );
            call(
                editor,
                json!({"cmd":"edit","label":"Alternative exposure","ops":[{"op":"set","path":"light.exposure","value":0.25}]}),
            );
            call(editor, json!({"cmd":"save"}));
            call(editor, json!({"cmd":"open","path":output}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"],
                0.75,
                "Virtual copies must not modify the master sidecar"
            );
            call(
                editor,
                json!({"cmd":"open","path":output,"copy":copy["id"]}),
            );
            let state = call(editor, json!({"cmd":"state"}));
            assert_eq!(state["params"]["light"]["exposure"], 0.25);
            assert_eq!(state["snapshots"], json!(["Saved look"]));
            call(editor, json!({"cmd":"restore_snapshot","index":0}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"],
                0.75
            );
            call(
                editor,
                json!({"cmd":"rename_copy","id":copy["id"],"name":"Renamed copy"}),
            );
            assert_eq!(
                call(editor, json!({"cmd":"copies"}))[0]["name"],
                "Renamed copy"
            );
            // Library identities must distinguish copies sharing the same original.
            let master_meta = call(editor, json!({"cmd":"library_metadata","path":output}));
            let copy_id = copy["id"].clone();
            let target = json!({"path":output,"copy":copy_id});
            let copied = call(editor, json!({"cmd":"batch_metadata","targets":[target,target],
                "rating":1,"tags":["Copy only"],"tag_mode":"replace"}));
            assert_eq!(copied["updated"].as_array().unwrap().len(), 1);
            assert_eq!(copied["updated"][0]["copy"], copy_id);
            assert_eq!(call(editor, json!({"cmd":"state"}))["rating"], 1);
            let copy_meta = call(editor, json!({"cmd":"library_metadata","path":output,"copy":copy_id}));
            assert_eq!(copy_meta["tags"], json!(["Copy only"]));
            assert_eq!(call(editor, json!({"cmd":"library_metadata","path":output})), master_meta);
            let bad = call(editor, json!({"cmd":"batch_metadata","targets":[{"path":second,"copy":copy_id}],"rating":5}));
            assert_eq!(bad["failures"].as_array().unwrap().len(), 1);
            assert_eq!(bad["updated"], json!([]));
            assert!((*editor).command(json!({"cmd":"batch_metadata","targets":[{"path":output,"copy":"bad"}],"rating":5})).is_err());
            assert_eq!(call(editor, json!({"cmd":"library_copies","paths":[output]}))[0]["id"], copy_id);
            // Save a non-active copy's sync history, then reopen and undo it.
            call(editor, json!({"cmd":"open","path":output}));
            let mut source = call(editor, json!({"cmd":"state"}))["params"].clone();
            source["light"]["exposure"] = json!(1.5);
            let synced = call(editor, json!({"cmd":"sync_selected","targets":[target,target],"params":source,"groups":["light"]}));
            assert_eq!(synced["updated"], json!([target]));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"], 0.75);
            call(editor, json!({"cmd":"comparison_open","targets":[{"path":output},target]}));
            assert_eq!((&(*editor).comparisons)[0].copy, None);
            assert_eq!((&(*editor).comparisons)[1].copy, copy_id.as_i64());
            assert_eq!((&(*editor).comparisons)[1].doc.head().light.exposure, 1.5);
            call(editor, json!({"cmd":"comparison_close"}));
            call(editor, json!({"cmd":"open","path":output,"copy":copy_id}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"], 1.5);
            call(editor, json!({"cmd":"edit","label":"Unsaved export look","ops":[{"op":"set","path":"light.exposure","value":2.0}]}));
            let inputs = call(editor, json!({"cmd":"export_inputs","targets":[{"path":output},target,target]}));
            assert_eq!(inputs.as_array().unwrap().len(), 2);
            assert_eq!(inputs[0]["raw"], false);
            assert_eq!(inputs[1]["raw"], false);
            assert_eq!(inputs[0]["params"]["light"]["exposure"], 0.75);
            assert_eq!(inputs[1]["params"]["light"]["exposure"], 2.0);
            assert!(inputs[1]["name"].as_str().unwrap().contains("Renamed copy"));
            assert_eq!(call(editor, json!({"cmd":"state"}))["dirty"], true);
            assert!((*editor).command(json!({"cmd":"export_inputs","targets":[{"path":second,"copy":copy_id}]})).is_err());
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(inputs[1]["params"]["light"]["exposure"], 2.0, "Queued snapshots must not follow later edits");
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"], 0.75);
            assert_eq!(call(editor, json!({"cmd":"state"}))["tags"], json!(["Copy only"]));
            call(editor, json!({"cmd":"save"}));
            let master_bytes = std::fs::read(&output).unwrap();
            let sidecar_bytes = std::fs::read(output.with_extension("xmp")).unwrap();
            let temporarily_missing = dir.path().join("original.tmp");
            std::fs::rename(&output, &temporarily_missing).unwrap();
            assert!(
                (*editor)
                    .command(json!({"cmd":"remove_copy","id":copy["id"]}))
                    .is_err()
            );
            assert_eq!(call(editor, json!({"cmd":"state"}))["copy"], copy["id"]);
            assert_eq!(
                call(editor, json!({"cmd":"copies"}))
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            std::fs::rename(&temporarily_missing, &output).unwrap();
            call(editor, json!({"cmd":"remove_copy","id":copy["id"]}));
            let master = call(editor, json!({"cmd":"state"}));
            assert!(master["copy"].is_null());
            assert_eq!(master["params"]["light"]["exposure"], 0.75);
            assert_eq!(call(editor, json!({"cmd":"copies"})), json!([]));
            assert!(
                (*editor)
                    .catalog
                    .history(&PhotoKey {
                        path: output.clone(),
                        copy: copy["id"].as_i64()
                    })
                    .unwrap()
                    .is_none()
            );
            call(editor, json!({"cmd":"save"}));
            assert_eq!(std::fs::read(&output).unwrap(), master_bytes);
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                sidecar_bytes
            );
            assert!(
                (*editor)
                    .command(json!({"cmd":"remove_copy","id":copy["id"]}))
                    .is_err()
            );
            let album = call(
                editor,
                json!({"cmd":"create_album","name":"Review","paths":[output,output]}),
            );
            let added = call(
                editor,
                json!({"cmd":"update_album","id":album,"paths":[second,missing],"add":true}),
            );
            assert_eq!(added["paths"].as_array().unwrap().len(), 3);
            let renamed = call(
                editor,
                json!({"cmd":"rename_album","id":album,"name":"Reviewed"}),
            );
            assert_eq!(renamed["name"], "Reviewed");
            assert_eq!(renamed["paths"], added["paths"]);
            let removed = call(
                editor,
                json!({"cmd":"update_album","id":album,"paths":[second],"add":false}),
            );
            assert_eq!(removed["paths"].as_array().unwrap().len(), 2);
            let reopened = Catalog::open(&dir.path().join("catalog.sqlite3")).unwrap();
            assert_eq!(
                reopened.album_paths(album.as_i64().unwrap()).unwrap().len(),
                2
            );
            drop(reopened);
            call(editor, json!({"cmd":"delete_album","id":album}));
            assert_eq!(call(editor, json!({"cmd":"albums"})), json!([]));
            assert!(
                (*editor)
                    .command(json!({"cmd":"update_album","id":album,"paths":[second],"add":true}))
                    .is_err()
            );
            assert_eq!(std::fs::read(&output).unwrap(), master_bytes);
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                sidecar_bytes
            );
            assert!(second.is_file());
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[output,second],"stack":"test-stack"}),
            );
            assert_eq!(
                call(editor, json!({"cmd":"library_metadata","path":second}))["stack"],
                "test-stack"
            );
            call(
                editor,
                json!({"cmd":"batch_metadata","paths":[output,second],"stack":null}),
            );
            assert!(
                call(editor, json!({"cmd":"library_metadata","path":output}))["stack"].is_null()
            );
            call(
                editor,
                json!({"cmd":"find_duplicates","paths":[output,second,output,missing],"id":1}),
            );
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let scan = call(editor, json!({"cmd":"state"}))["duplicates"].clone();
                if scan["running"] == false {
                    assert_eq!(scan["groups"].as_array().unwrap().len(), 1);
                    assert_eq!(scan["groups"][0].as_array().unwrap().len(), 2);
                    assert_eq!(scan["failures"].as_array().unwrap().len(), 1);
                    assert_eq!(scan["id"], 1);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let copy = call(editor, json!({"cmd":"create_copy","name":"Move test"}));
            let album = call(
                editor,
                json!({"cmd":"create_album","name":"Moves","paths":[output]}),
            );
            let master_meta = std::fs::read(output.with_extension("xmp")).unwrap();
            let conflict = call(
                editor,
                json!({"cmd":"file_action","action":"move","path":output,"destination":second}),
            );
            assert_eq!(conflict["ok"], false);
            assert!(output.is_file());
            assert_eq!(conflict["opened"]["path"], json!(output));
            let reserved = dir.path().join("reserved.png");
            (*editor)
                .catalog
                .create_copy(&reserved, "Existing edits", &PhotoMeta::default())
                .unwrap();
            let rollback = call(
                editor,
                json!({"cmd":"file_action","action":"move","path":output,"destination":reserved}),
            );
            assert_eq!(rollback["ok"], false);
            assert!(!reserved.exists());
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                master_meta
            );
            (*editor).catalog.forget_photo(&reserved).unwrap();
            let moved = dir.path().join("moved.png");
            let result = call(
                editor,
                json!({"cmd":"file_action","action":"move","path":output,"destination":moved}),
            );
            assert_eq!(result["ok"], true);
            assert!(!output.exists());
            assert_eq!(std::fs::read(&moved).unwrap(), master_bytes);
            assert_eq!(
                std::fs::read(moved.with_extension("xmp")).unwrap(),
                master_meta
            );
            assert_eq!(
                call(editor, json!({"cmd":"copies"}))[0]["path"],
                json!(moved)
            );
            assert_eq!(
                call(editor, json!({"cmd":"album_paths","id":album}))["paths"],
                json!([moved])
            );
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"]["light"]["exposure"],
                0.75
            );
            // Simulate moving the original outside the editor, then locate it.
            let relocated = dir.path().join("relocated.png");
            std::fs::rename(&moved, &relocated).unwrap();
            let linked = call(
                editor,
                json!({"cmd":"file_action","action":"relink","path":moved,"destination":relocated}),
            );
            assert_eq!(linked["ok"], true);
            assert_eq!(
                std::fs::read(relocated.with_extension("xmp")).unwrap(),
                master_meta
            );
            assert!(
                moved.with_extension("xmp").exists(),
                "Relink retains the old sidecar"
            );
            assert_eq!(call(editor, json!({"cmd":"copies"}))[0]["id"], copy["id"]);
            std::fs::remove_file(moved.with_extension("xmp")).unwrap();
            let restored = call(
                editor,
                json!({"cmd":"file_action","action":"move","path":relocated,"destination":output}),
            );
            assert_eq!(restored["ok"], true);
            assert_eq!(
                call(editor, json!({"cmd":"album_paths","id":album}))["paths"],
                json!([output])
            );
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                master_meta
            );
            let before_comparison = call(editor, json!({"cmd":"state"}));
            call(
                editor,
                json!({"cmd":"comparison_open","paths":[output,second]}),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let states = call(editor, json!({"cmd":"comparison_state"}));
                if states[0]["extent"].is_array() && states[1]["extent"].is_array() {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            call(
                editor,
                json!({"cmd":"comparison_viewport","index":0,"viewport":{
                "region":{"x":10.,"y":10.,"w":40.,"h":30.},"output_size":[80,60],"display":"Srgb"}}),
            );
            loop {
                let frame = aigle_comparison_frame_take(editor, 0);
                if !frame.is_null() {
                    let pixels = aigle_frame_pixels(frame);
                    let matches = pixels.width == 80 && pixels.height == 60 && pixels.w == 40.;
                    aigle_frame_free(frame);
                    if matches {
                        break;
                    }
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(10));
            }
            // Comparison frames retain HDR half-float pixels until the GUI consumes them.
            call(editor, json!({"cmd":"comparison_viewport","index":0,"viewport":{
                "region":{"x":10.,"y":10.,"w":40.,"h":30.},
                "output_size":[80,60],"display":"HdrRec2020"}}));
            let deadline = Instant::now() + Duration::from_secs(30);
            let hdr_comparison = loop {
                let frame = aigle_comparison_frame_take(editor, 0);
                if !frame.is_null() {
                    let pixels = aigle_frame_pixels(frame);
                    if pixels.format == 1 && pixels.width == 80 && pixels.height == 60 {
                        assert_eq!(pixels.stride, pixels.width * 8);
                        break frame;
                    }
                    aigle_frame_free(frame);
                }
                assert!(Instant::now() < deadline, "HDR comparison frame timed out");
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                before_comparison["params"]
            );
            call(editor, json!({"cmd":"comparison_close"}));
            assert_eq!(call(editor, json!({"cmd":"comparison_state"})), json!([]));
            assert!(aigle_comparison_frame_take(editor, 0).is_null());
            // Closing a comparison session must not invalidate queued GUI frame ownership.
            let retained = aigle_frame_pixels(hdr_comparison);
            assert_eq!(retained.format, 1);
            assert_eq!(retained.length, retained.stride as usize * retained.height as usize);
            assert!(!retained.data.is_null());
            let _ = std::slice::from_raw_parts(retained.data, retained.length)[0];
            aigle_frame_free(hdr_comparison);
            let source_params = call(editor, json!({"cmd":"state"}))["params"].clone();
            call(editor, json!({"cmd":"open","path":second}));
            call(
                editor,
                json!({"cmd":"edit","label":"Target contrast","ops":[
                {"op":"set","path":"presence.clarity","value":25.}]}),
            );
            call(
                editor,
                json!({"cmd":"metadata","rating":2,"tags":["Keep target"]}),
            );
            call(editor, json!({"cmd":"save"}));
            let target_before = call(editor, json!({"cmd":"state"}));
            call(
                editor,
                json!({"cmd":"paste_selected","params":source_params,"groups":["light"]}),
            );
            let pasted = call(editor, json!({"cmd":"state"}));
            assert_eq!(pasted["params"]["light"], source_params["light"]);
            assert_eq!(
                pasted["params"]["presence"],
                target_before["params"]["presence"]
            );
            assert_eq!(pasted["rating"], 2);
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                target_before["params"]
            );
            call(editor, json!({"cmd":"open","path":output}));
            let synced = call(
                editor,
                json!({"cmd":"sync_selected","params":source_params,"groups":["light"],"paths":[second,second,missing]}),
            );
            assert_eq!(synced["updated"], json!([second]));
            assert_eq!(synced["failures"].as_array().unwrap().len(), 1);
            assert!((*editor).command(json!({"cmd":"sync_selected","params":source_params,"groups":["not_a_group"],"paths":[second]})).is_err());
            call(editor, json!({"cmd":"open","path":second}));
            let synced = call(editor, json!({"cmd":"state"}));
            assert_eq!(synced["params"]["light"], source_params["light"]);
            assert_eq!(
                synced["params"]["presence"],
                target_before["params"]["presence"]
            );
            assert_eq!(synced["tags"], json!(["Keep target"]));
            assert_eq!(synced["rating"], 2);
            call(editor, json!({"cmd":"undo"}));
            assert_eq!(
                call(editor, json!({"cmd":"state"}))["params"],
                target_before["params"]
            );
            call(editor, json!({"cmd":"open","path":output}));
            let destination = dir.path().join("imported");
            call(
                editor,
                json!({"cmd":"import","source":dir.path(),"destination":destination,"folders":"","names":"{name}"}),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let state = call(editor, json!({"cmd":"state"}));
                assert!(state["import"].get("error").is_none(), "{state}");
                if state["import"]["running"] == false {
                    assert_eq!(state["import"]["imported"].as_array().unwrap().len(), 2);
                    break;
                }
                assert!(Instant::now() < deadline, "import timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                std::fs::read(destination.join("photo.png")).unwrap(),
                std::fs::read(&output).unwrap()
            );
            let backup = dir.path().join("restore-source.sqlite3");
            call(editor, json!({"cmd":"catalog_backup","path":backup}));
            let original_albums = call(editor, json!({"cmd":"albums"}));
            call(
                editor,
                json!({"cmd":"create_album","name":"After backup","paths":[output]}),
            );
            let changed_albums = call(editor, json!({"cmd":"albums"}));
            let invalid = dir.path().join("invalid.sqlite3");
            std::fs::write(&invalid, b"not sqlite").unwrap();
            assert!(
                (*editor)
                    .command(json!({"cmd":"catalog_restore","path":invalid}))
                    .is_err()
            );
            assert_eq!(call(editor, json!({"cmd":"state"}))["open"], true);
            assert_eq!(call(editor, json!({"cmd":"albums"})), changed_albums);
            call(editor, json!({"cmd":"save"}));
            let sidecar = std::fs::read(output.with_extension("xmp")).unwrap();
            let restored = call(editor, json!({"cmd":"catalog_restore","path":backup}));
            assert_eq!(restored["restored"], true);
            assert_eq!(call(editor, json!({"cmd":"state"}))["open"], false);
            assert_eq!(call(editor, json!({"cmd":"albums"})), original_albums);
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                sidecar
            );
            let recovered = call(
                editor,
                json!({"cmd":"catalog_restore","path":restored["recovery"]}),
            );
            assert_eq!(recovered["restored"], true);
            assert_eq!(call(editor, json!({"cmd":"albums"})), changed_albums);
            assert_eq!(
                std::fs::read(output.with_extension("xmp")).unwrap(),
                sidecar
            );
            aigle_editor_free(editor);
            // Both buffers must remain valid after the editor and sessions die.
            assert_eq!(aigle_frame_pixels(sdr).data, initial.data);
            assert_eq!(aigle_frame_pixels(hdr).length, hp.length);
            assert!(!std::slice::from_raw_parts(initial.data, initial.length).is_empty());
            aigle_frame_free(sdr);
            aigle_frame_free(hdr);
        }
    }
    #[test]
    fn boundary_rejects_null_and_malformed_requests() {
        unsafe {
            let p = aigle_command(ptr::null_mut(), ptr::null());
            assert!(CStr::from_ptr(p).to_str().unwrap().contains("null editor"));
            aigle_string_free(p);
            assert_eq!(aigle_frame_pixels(ptr::null()).length, 0);
            aigle_frame_free(ptr::null_mut());
        }
    }
}
