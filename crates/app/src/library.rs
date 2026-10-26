//! The open folder: its photos, their sidecar metadata and thumbnails (doc 01 §9, doc 03 §8).
//!
//! Thumbnails are requested only for the index ranges the grid and the filmstrip currently
//! show; leaving a range drops its jobs, which cancels them in the engine. Thumbnails of
//! edited photos follow the edits about a second after the last change.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aigle_core::EditParams;
use aigle_engine_api::{Engine, Priority, SourceRef};
use aigle_library::{PhotoEntry, PhotoMeta, Store, WriteReport, Writer};
use futures::StreamExt;
use gpui::{AppContext, Context, EventEmitter, RenderImage, Task};

use crate::canvas::render_image;

/// Long edge of grid and filmstrip thumbnails, in device px.
pub const THUMB_PX: u32 = 360;
/// Thumbnails kept in memory; the ones furthest from what is shown go first.
const KEEP: usize = 500;
const SAVE_DELAY: Duration = Duration::from_secs(1);
const META_CHUNK: usize = 48;

/// Who asks for thumbnails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    Grid = 0,
    Filmstrip = 1,
}

pub enum LibraryEvent {
    /// Open photo `index` in develop (double-click, filmstrip click).
    Open(usize),
    Notice(String),
}

pub struct Item {
    pub entry: PhotoEntry,
    /// `None` until the sidecar has been read.
    pub meta: Option<PhotoMeta>,
    pub thumb: Option<Arc<RenderImage>>,
    /// The edits the thumbnail should show; trails `meta.edits` by [`SAVE_DELAY`].
    shown: Option<Arc<EditParams>>,
    /// The edits `thumb` was made from.
    thumb_of: Option<Option<Arc<EditParams>>>,
    failed_for: Option<Option<Arc<EditParams>>>,
}

impl Item {
    pub fn rating(&self) -> i8 {
        self.meta.as_ref().map_or(0, |m| m.rating)
    }

    pub fn label(&self) -> Option<&str> {
        self.meta.as_ref().and_then(|m| m.label.as_deref())
    }

    pub fn edited(&self) -> bool {
        self.meta.as_ref().is_some_and(|m| m.edits.is_some())
    }
}

struct ThumbJob {
    index: usize,
    edits: Option<Arc<EditParams>>,
    _task: Task<()>,
}

pub struct Library {
    engine: Engine,
    pub dir: Option<PathBuf>,
    pub items: Vec<Item>,
    pub selected: Option<usize>,
    pub loading: bool,
    store: Arc<Store>,
    writer: Writer,
    wanted: [Range<usize>; 2],
    jobs: Vec<ThumbJob>,
    generation: u64,
    scan: Option<Task<()>>,
    refresh: Option<Task<()>>,
    _reports: Task<()>,
}

impl EventEmitter<LibraryEvent> for Library {}

impl Library {
    pub fn new(engine: Engine, cx: &mut Context<Self>) -> Library {
        let store = Arc::new(Store::new());
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<WriteReport>();
        let writer = Writer::new(store.clone(), SAVE_DELAY, move |r| {
            let _ = tx.unbounded_send(r);
        });
        let reports = cx.spawn(async move |this, cx| {
            while let Some((photo, r)) = rx.next().await {
                let Err(e) = r else { continue };
                let name = photo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let msg = if e.is_read_only() {
                    format!("文件夹不可写，{name} 的修改未保存")
                } else if matches!(e, aigle_library::Error::Conflict(_)) {
                    format!("{name} 的 XMP 已被其他程序修改，未覆盖")
                } else {
                    format!("{name} 的 XMP 保存失败：{e}")
                };
                if this
                    .update(cx, |_, cx| cx.emit(LibraryEvent::Notice(msg)))
                    .is_err()
                {
                    break;
                }
            }
        });
        Library {
            engine,
            dir: None,
            items: Vec::new(),
            selected: None,
            loading: false,
            store,
            writer,
            wanted: [0..0, 0..0],
            jobs: Vec::new(),
            generation: 0,
            scan: None,
            refresh: None,
            _reports: reports,
        }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|it| it.entry.path == path)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected?)
    }

    pub fn folder_name(&self) -> String {
        self.dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Scan `dir`, then read every sidecar in the background. `select` is selected once
    /// listed (the first photo otherwise).
    pub fn open_folder(&mut self, dir: PathBuf, select: Option<PathBuf>, cx: &mut Context<Self>) {
        self.writer.flush();
        self.generation += 1;
        self.jobs.clear();
        self.drop_thumbs(cx);
        self.items.clear();
        self.selected = None;
        self.wanted = [0..0, 0..0];
        self.dir = Some(dir.clone());
        self.loading = true;
        let generation = self.generation;
        let store = self.store.clone();
        cx.notify();
        self.scan = Some(cx.spawn(async move |this, cx| {
            let scan_dir = dir.clone();
            let listed = cx
                .background_spawn(async move { aigle_library::scan(&scan_dir) })
                .await;
            let paths = match this.update(cx, |l, cx| l.listed(generation, listed, select, cx)) {
                Ok(Some(p)) => p,
                _ => return,
            };
            let mut warnings = Vec::new();
            for (start, chunk) in (0..).step_by(META_CHUNK).zip(paths.chunks(META_CHUNK)) {
                let store = store.clone();
                let chunk = chunk.to_vec();
                let read = cx
                    .background_spawn(async move {
                        chunk.iter().map(|p| store.read(p)).collect::<Vec<_>>()
                    })
                    .await;
                let ok = this.update(cx, |l, cx| {
                    if l.generation != generation {
                        return false;
                    }
                    for (i, r) in (start..).zip(read) {
                        let Some(it) = l.items.get_mut(i) else { break };
                        if it.meta.is_some() {
                            continue;
                        }
                        let meta = match r {
                            Ok(loaded) => {
                                warnings.extend(
                                    loaded
                                        .warnings
                                        .iter()
                                        .map(|w| format!("{}: {w}", it.entry.name)),
                                );
                                loaded.meta
                            }
                            Err(e) => {
                                warnings.push(e.to_string());
                                PhotoMeta::default()
                            }
                        };
                        it.shown = meta.edits.clone().map(Arc::new);
                        it.meta = Some(meta);
                    }
                    l.request_thumbs(cx);
                    cx.notify();
                    true
                });
                if !matches!(ok, Ok(true)) {
                    return;
                }
            }
            let _ = this.update(cx, |l, cx| {
                l.loading = false;
                for w in &warnings {
                    tracing::warn!("{w}");
                }
                if let Some(w) = warnings.first() {
                    let more = if warnings.len() > 1 {
                        format!("（另有 {} 条）", warnings.len() - 1)
                    } else {
                        String::new()
                    };
                    cx.emit(LibraryEvent::Notice(format!("XMP：{w}{more}")));
                }
                cx.notify();
            });
        }));
    }

    fn listed(
        &mut self,
        generation: u64,
        listed: std::io::Result<Vec<PhotoEntry>>,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<PathBuf>> {
        if self.generation != generation {
            return None;
        }
        let entries = match listed {
            Ok(e) => e,
            Err(e) => {
                self.loading = false;
                cx.emit(LibraryEvent::Notice(format!("无法读取文件夹：{e}")));
                cx.notify();
                return None;
            }
        };
        self.items = entries
            .into_iter()
            .map(|entry| Item {
                entry,
                meta: None,
                thumb: None,
                shown: None,
                thumb_of: None,
                failed_for: None,
            })
            .collect();
        self.selected = select
            .and_then(|p| self.index_of(&p))
            .or((!self.items.is_empty()).then_some(0));
        cx.notify();
        Some(self.items.iter().map(|it| it.entry.path.clone()).collect())
    }

    /// The metadata of a photo by path, read from its sidecar now.
    pub fn read_meta(&self, path: &Path) -> PhotoMeta {
        match self.store.read(path) {
            Ok(l) => {
                for w in &l.warnings {
                    tracing::warn!(path = %path.display(), "{w}");
                }
                l.meta
            }
            Err(e) => {
                tracing::warn!("{e}");
                PhotoMeta::default()
            }
        }
    }

    /// The metadata of `index`, reading the sidecar now if the background pass has not
    /// reached it yet.
    pub fn meta_now(&mut self, index: usize) -> PhotoMeta {
        let Some(it) = self.items.get(index) else {
            return PhotoMeta::default();
        };
        if it.meta.is_none() {
            let meta = self.read_meta(&it.entry.path);
            let it = &mut self.items[index];
            it.shown = meta.edits.clone().map(Arc::new);
            it.meta = Some(meta);
        }
        self.items[index].meta.clone().unwrap_or_default()
    }

    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.items.len() && self.selected != Some(index) {
            self.selected = Some(index);
            cx.notify();
        }
    }

    /// Show thumbnails for `range` of `who`; jobs outside every shown range are cancelled.
    pub fn want(&mut self, who: Consumer, range: Range<usize>, cx: &mut Context<Self>) {
        let range = range.start.min(self.items.len())..range.end.min(self.items.len());
        if self.wanted[who as usize] != range {
            self.wanted[who as usize] = range;
            self.request_thumbs(cx);
        }
    }

    fn is_wanted(&self, i: usize) -> bool {
        self.wanted.iter().any(|r| r.contains(&i))
    }

    fn request_thumbs(&mut self, cx: &mut Context<Self>) {
        let wanted = self.wanted.clone();
        self.jobs
            .retain(|j| wanted.iter().any(|r| r.contains(&j.index)));
        let center = self.selected.unwrap_or(0);
        for i in wanted.iter().flat_map(|r| r.clone()) {
            let Some(it) = self.items.get(i) else {
                continue;
            };
            if it.meta.is_none() {
                continue;
            }
            let edits = it.shown.clone();
            let current = Some(edits.clone());
            if it.thumb_of == current
                || it.failed_for == current
                || self.jobs.iter().any(|j| j.index == i && j.edits == edits)
            {
                continue;
            }
            self.jobs.retain(|j| j.index != i);
            let prio = 10_000 - (i.abs_diff(center).min(10_000) as i32);
            let job = self.engine.thumbnail(
                SourceRef::File(it.entry.path.clone()),
                edits.clone(),
                THUMB_PX,
                Priority(prio),
            );
            let generation = self.generation;
            let for_edits = edits.clone();
            let task = cx.spawn(async move |this, cx| {
                let r = job.await;
                let _ = this.update(cx, |l, cx| {
                    if l.generation != generation {
                        return;
                    }
                    l.jobs.retain(|j| !(j.index == i && j.edits == for_edits));
                    let Some(it) = l.items.get_mut(i) else { return };
                    match r.ok().and_then(|t| render_image(&t.pixels)) {
                        Some(img) => {
                            if let Some(old) = it.thumb.replace(img) {
                                cx.drop_image(old, None);
                            }
                            it.thumb_of = Some(for_edits);
                        }
                        None => it.failed_for = Some(for_edits),
                    }
                    l.evict(cx);
                    cx.notify();
                });
            });
            self.jobs.push(ThumbJob {
                index: i,
                edits,
                _task: task,
            });
        }
    }

    /// Drop the thumbnails furthest from the shown ranges until at most [`KEEP`] remain.
    fn evict(&mut self, cx: &mut Context<Self>) {
        let mut held: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.items[i].thumb.is_some() && !self.is_wanted(i))
            .collect();
        let total = held.len() + self.wanted.iter().map(|r| r.len()).sum::<usize>();
        if total <= KEEP {
            return;
        }
        let distance = |i: usize| {
            self.wanted
                .iter()
                .filter(|r| !r.is_empty())
                .map(|r| {
                    if i < r.start {
                        r.start - i
                    } else {
                        i + 1 - r.end
                    }
                })
                .min()
                .unwrap_or(usize::MAX)
        };
        held.sort_by_key(|&i| std::cmp::Reverse(distance(i)));
        for i in held.into_iter().take(total - KEEP) {
            let it = &mut self.items[i];
            if let Some(old) = it.thumb.take() {
                cx.drop_image(old, None);
            }
            it.thumb_of = None;
        }
    }

    fn drop_thumbs(&mut self, cx: &mut Context<Self>) {
        for it in &mut self.items {
            if let Some(old) = it.thumb.take() {
                cx.drop_image(old, None);
            }
        }
    }

    fn save(&self, index: usize) {
        if let Some(it) = self.items.get(index)
            && let Some(meta) = &it.meta
        {
            self.writer.schedule(it.entry.path.clone(), meta.clone());
        }
    }

    pub fn set_rating(&mut self, index: usize, rating: i8, cx: &mut Context<Self>) {
        self.meta_now(index);
        if let Some(meta) = self.items.get_mut(index).and_then(|it| it.meta.as_mut())
            && meta.rating != rating
        {
            meta.rating = rating;
            self.save(index);
            cx.notify();
        }
    }

    /// Set color label `label`, or clear it if the photo already has it.
    pub fn toggle_label(&mut self, index: usize, label: &str, cx: &mut Context<Self>) {
        self.meta_now(index);
        if let Some(meta) = self.items.get_mut(index).and_then(|it| it.meta.as_mut()) {
            meta.label = if meta.label.as_deref() == Some(label) {
                None
            } else {
                Some(label.to_string())
            };
            self.save(index);
            cx.notify();
        }
    }

    /// The open photo `path` has new edits: save them and refresh its thumbnail shortly.
    pub fn photo_edited(&mut self, path: &Path, head: &Arc<EditParams>, cx: &mut Context<Self>) {
        let edits = (**head != EditParams::default()).then(|| (**head).clone());
        let Some(i) = self.index_of(path) else {
            let mut meta = self.read_meta(path);
            meta.edits = edits;
            self.writer.schedule(path.to_path_buf(), meta);
            return;
        };
        self.meta_now(i);
        let Some(meta) = self.items[i].meta.as_mut() else {
            return;
        };
        if meta.edits == edits {
            return;
        }
        meta.edits = edits;
        self.save(i);
        let shown = (**head != EditParams::default()).then(|| head.clone());
        let generation = self.generation;
        self.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |l, cx| {
                if l.generation == generation
                    && let Some(it) = l.items.get_mut(i)
                {
                    it.shown = shown;
                    l.request_thumbs(cx);
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// Write every pending sidecar change now (closing a photo, quitting).
    pub fn flush(&self) {
        self.writer.flush();
    }
}

/// Lightroom's color labels (`xmp:Label`) for keys 6–9, with their display colors.
pub const LABELS: [(&str, u32); 4] = [
    ("Red", 0xe5484d),
    ("Yellow", 0xf5d90a),
    ("Green", 0x46a758),
    ("Blue", 0x3e63dd),
];

pub fn label_color(label: &str) -> Option<u32> {
    LABELS
        .iter()
        .chain(&[("Purple", 0x8e4ec6)])
        .find(|(n, _)| n.eq_ignore_ascii_case(label))
        .map(|(_, c)| *c)
}
