//! The open folder: its photos, their sidecar metadata and thumbnails (doc 01 §9, doc 03 §8).
//!
//! Thumbnails are requested only for the index ranges the grid and the filmstrip currently
//! show; leaving a range drops its jobs, which cancels them in the engine. Thumbnails of
//! edited photos follow the edits about a second after the last change.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aigle_core::EditParams;
use aigle_doc::Origin;
use aigle_engine_api::{Engine, Priority, SourceRef};
use aigle_library::{
    Catalog, CopyId, PhotoEntry, PhotoKey, PhotoMeta, Snapshot, Store, VirtualCopy, WriteReport,
    Writer,
};
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

/// A virtual copy shown in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct CopyInfo {
    pub id: CopyId,
    pub name: String,
}

pub struct Item {
    pub entry: PhotoEntry,
    /// `Some` for a virtual copy of `entry`.
    pub copy: Option<CopyInfo>,
    /// `None` until the sidecar has been read.
    pub meta: Option<PhotoMeta>,
    pub thumb: Option<Arc<RenderImage>>,
    /// The edits the thumbnail should show; trails `meta.edits` by [`SAVE_DELAY`].
    shown: Option<Arc<EditParams>>,
    /// The edits `thumb` was made from.
    thumb_of: Option<Option<Arc<EditParams>>>,
    failed_for: Option<Option<Arc<EditParams>>>,
}

/// `IMG_1 (Copy 1).CR3`: the photo's file name with the copy's name before the extension.
pub fn copy_display_name(file_name: &str, copy: &str) -> String {
    let p = Path::new(file_name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    match p.extension() {
        Some(ext) => format!("{stem} ({copy}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({copy})"),
    }
}

impl Item {
    fn new(entry: PhotoEntry, copy: Option<CopyInfo>, meta: Option<PhotoMeta>) -> Item {
        Item {
            entry,
            copy,
            shown: meta.as_ref().and_then(|m| m.edits.clone()).map(Arc::new),
            meta,
            thumb: None,
            thumb_of: None,
            failed_for: None,
        }
    }

    pub fn key(&self) -> PhotoKey {
        PhotoKey {
            path: self.entry.path.clone(),
            copy: self.copy.as_ref().map(|c| c.id),
        }
    }

    /// The file name, with the copy name for a virtual copy.
    pub fn name(&self) -> String {
        match &self.copy {
            Some(c) => copy_display_name(&self.entry.name, &c.name),
            None => self.entry.name.clone(),
        }
    }

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
    /// The primary selection: the photo develop opens and single-photo commands act on.
    pub selected: Option<usize>,
    /// Every selected photo; contains `selected`.
    selection: BTreeSet<usize>,
    /// Where a shift-click range starts.
    anchor: Option<usize>,
    pub loading: bool,
    store: Arc<Store>,
    writer: Writer,
    pub catalog: Arc<Catalog>,
    /// Item positions by key.
    index: HashMap<PhotoKey, usize>,
    wanted: [Range<usize>; 2],
    jobs: Vec<ThumbJob>,
    generation: u64,
    scan: Option<Task<()>>,
    /// Pending thumbnail refreshes after edits.
    refresh: HashMap<PhotoKey, Task<()>>,
    /// Virtual copy metadata waiting to be stored, and the timer that stores it.
    pending_copies: HashMap<CopyId, PhotoMeta>,
    copy_save: Option<Task<()>>,
    _reports: Task<()>,
}

impl EventEmitter<LibraryEvent> for Library {}

impl Library {
    pub fn new(engine: Engine, catalog: Arc<Catalog>, cx: &mut Context<Self>) -> Library {
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
                    rust_i18n::t!("library.folder_read_only", name = name).into_owned()
                } else if matches!(e, aigle_library::Error::Conflict(_)) {
                    rust_i18n::t!("library.xmp_conflict", name = name).into_owned()
                } else {
                    rust_i18n::t!("library.xmp_save_failed", name = name, e = e).into_owned()
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
            selection: BTreeSet::new(),
            anchor: None,
            loading: false,
            store,
            writer,
            catalog,
            index: HashMap::new(),
            wanted: [0..0, 0..0],
            jobs: Vec::new(),
            generation: 0,
            scan: None,
            refresh: HashMap::new(),
            pending_copies: HashMap::new(),
            copy_save: None,
            _reports: reports,
        }
    }

    pub fn index_of(&self, key: &PhotoKey) -> Option<usize> {
        self.index.get(key).copied()
    }

    fn reindex(&mut self) {
        self.index = self
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.key(), i))
            .collect();
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
        self.flush(cx);
        self.generation += 1;
        self.jobs.clear();
        self.refresh.clear();
        self.drop_thumbs(cx);
        self.items.clear();
        self.index.clear();
        self.set_single(None);
        self.wanted = [0..0, 0..0];
        self.dir = Some(dir.clone());
        self.loading = true;
        let generation = self.generation;
        let store = self.store.clone();
        let catalog = self.catalog.clone();
        cx.notify();
        self.scan = Some(cx.spawn(async move |this, cx| {
            let scan_dir = dir.clone();
            let listed = cx
                .background_spawn(async move {
                    let entries = aigle_library::scan(&scan_dir)?;
                    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                    let copies = catalog.copies_of(&paths).unwrap_or_else(|e| {
                        tracing::warn!("{e}");
                        Vec::new()
                    });
                    Ok((entries, copies))
                })
                .await;
            let paths = match this.update(cx, |l, cx| l.listed(generation, listed, select, cx)) {
                Ok(Some(p)) => p,
                _ => return,
            };
            let mut warnings = Vec::new();
            for chunk in paths.chunks(META_CHUNK) {
                let store = store.clone();
                let chunk = chunk.to_vec();
                let read = cx
                    .background_spawn(async move {
                        chunk
                            .into_iter()
                            .map(|p| {
                                let r = store.read(&p);
                                (p, r)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                let ok = this.update(cx, |l, cx| {
                    if l.generation != generation {
                        return false;
                    }
                    for (path, r) in read {
                        let Some(i) = l.index_of(&PhotoKey::master(path)) else {
                            continue;
                        };
                        let it = &mut l.items[i];
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
                        rust_i18n::t!("library.more_warnings", arg0 = warnings.len() - 1)
                            .into_owned()
                    } else {
                        String::new()
                    };
                    cx.emit(LibraryEvent::Notice(format!("XMP: {w}{more}")));
                }
                cx.notify();
            });
        }));
    }

    fn listed(
        &mut self,
        generation: u64,
        listed: std::io::Result<(Vec<PhotoEntry>, Vec<VirtualCopy>)>,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<PathBuf>> {
        if self.generation != generation {
            return None;
        }
        let (entries, copies) = match listed {
            Ok(e) => e,
            Err(e) => {
                self.loading = false;
                cx.emit(LibraryEvent::Notice(
                    rust_i18n::t!("library.read_folder_failed", e = e).into_owned(),
                ));
                cx.notify();
                return None;
            }
        };
        let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        let mut copies = copies.into_iter().peekable();
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = entry.path.clone();
            items.push(Item::new(entry.clone(), None, None));
            while let Some(c) = copies.next_if(|c| c.path == path) {
                let info = CopyInfo {
                    id: c.id,
                    name: c.name,
                };
                items.push(Item::new(entry.clone(), Some(info), Some(c.meta)));
            }
        }
        self.items = items;
        self.reindex();
        let first = select
            .and_then(|p| self.index_of(&PhotoKey::master(p)))
            .or((!self.items.is_empty()).then_some(0));
        self.set_single(first);
        cx.notify();
        Some(paths)
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

    fn set_single(&mut self, index: Option<usize>) {
        self.selected = index;
        self.anchor = index;
        self.selection = index.into_iter().collect();
    }

    /// Select only `index`.
    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.items.len() && (self.selected != Some(index) || self.selection.len() != 1) {
            self.set_single(Some(index));
            cx.notify();
        }
    }

    /// Add `index` to the selection or take it out (Ctrl-click); the primary photo stays
    /// unless it is the one taken out.
    pub fn toggle_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.items.len() {
            return;
        }
        if self.selection.remove(&index) {
            if self.selected == Some(index) {
                self.selected = self.selection.iter().next().copied();
            }
            if self.selected.is_none() {
                self.set_single(Some(index));
            }
        } else {
            self.selection.insert(index);
            if self.selected.is_none() {
                self.selected = Some(index);
            }
        }
        self.anchor = Some(index);
        cx.notify();
    }

    /// Select the range from the anchor to `index` (Shift-click), keeping the primary photo
    /// in it.
    pub fn extend_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.items.len() {
            return;
        }
        let from = self.anchor.unwrap_or(index);
        self.selection = (from.min(index)..=from.max(index)).collect();
        match self.selected {
            Some(s) if self.selection.contains(&s) => {}
            _ => self.selected = Some(from),
        }
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.selection = (0..self.items.len()).collect();
        if self.selected.is_none() {
            self.selected = Some(0);
            self.anchor = Some(0);
        }
        cx.notify();
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selection.contains(&index)
    }

    pub fn selection_len(&self) -> usize {
        self.selection.len()
    }

    /// The selected photos in folder order.
    pub fn selection(&self) -> Vec<usize> {
        self.selection.iter().copied().collect()
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

    fn save(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(it) = self.items.get(index) else {
            return;
        };
        let Some(meta) = it.meta.clone() else { return };
        match &it.copy {
            None => self.writer.schedule(it.entry.path.clone(), meta),
            Some(c) => self.schedule_copy(c.id, meta, cx),
        }
    }

    /// Store a virtual copy's metadata after the debounce delay, like sidecar writes.
    fn schedule_copy(&mut self, id: CopyId, meta: PhotoMeta, cx: &mut Context<Self>) {
        self.pending_copies.insert(id, meta);
        self.copy_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |l, cx| l.write_copies(cx));
        }));
    }

    fn write_copies(&mut self, cx: &mut Context<Self>) {
        self.copy_save = None;
        let mut failed = None;
        for (id, meta) in self.pending_copies.drain() {
            if let Err(e) = self.catalog.set_copy_meta(id, &meta) {
                tracing::warn!("{e}");
                failed.get_or_insert(e);
            }
        }
        if let Some(e) = failed {
            cx.emit(LibraryEvent::Notice(
                rust_i18n::t!("library.copy_save_failed", e = e).into_owned(),
            ));
        }
    }

    pub fn set_rating(&mut self, index: usize, rating: i8, cx: &mut Context<Self>) {
        self.meta_now(index);
        if let Some(meta) = self.items.get_mut(index).and_then(|it| it.meta.as_mut())
            && meta.rating != rating
        {
            meta.rating = rating;
            self.save(index, cx);
            cx.notify();
        }
    }

    pub fn set_label(&mut self, index: usize, label: Option<&str>, cx: &mut Context<Self>) {
        self.meta_now(index);
        if let Some(meta) = self.items.get_mut(index).and_then(|it| it.meta.as_mut())
            && meta.label.as_deref() != label
        {
            meta.label = label.map(str::to_string);
            self.save(index, cx);
            cx.notify();
        }
    }

    /// The open photo `key` has new edits: save them and refresh its thumbnail shortly.
    pub fn photo_edited(&mut self, key: &PhotoKey, head: &Arc<EditParams>, cx: &mut Context<Self>) {
        let edits = (**head != EditParams::default()).then(|| (**head).clone());
        let Some(i) = self.index_of(key) else {
            self.update_unlisted(key, cx, |m| m.edits = edits);
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
        self.save(i, cx);
        let shown = (**head != EditParams::default()).then(|| head.clone());
        let generation = self.generation;
        let refresh_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |l, cx| {
                if l.generation != generation {
                    return;
                }
                l.refresh.remove(&refresh_key);
                if let Some(i) = l.index_of(&refresh_key) {
                    l.items[i].shown = shown;
                    l.request_thumbs(cx);
                    cx.notify();
                }
            });
        });
        self.refresh.insert(key.clone(), task);
        cx.notify();
    }

    /// Change the stored metadata of a photo or copy that is not in the open folder.
    fn update_unlisted(
        &mut self,
        key: &PhotoKey,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut PhotoMeta),
    ) {
        match key.copy {
            None => {
                let mut meta = self.read_meta(&key.path);
                change(&mut meta);
                self.writer.schedule(key.path.clone(), meta);
            }
            Some(id) => {
                let mut meta = self.pending_copies.get(&id).cloned().or_else(|| {
                    self.catalog
                        .copy_meta(id)
                        .inspect_err(|e| tracing::warn!("{e}"))
                        .ok()
                        .flatten()
                });
                if let Some(meta) = meta.as_mut() {
                    change(meta);
                    self.schedule_copy(id, meta.clone(), cx);
                }
            }
        }
    }

    /// Apply `edit` to the stored edits of `index` (a photo not open in develop), recording
    /// it in the photo's saved history as `label`.
    pub fn edit_stored(
        &mut self,
        index: usize,
        label: &str,
        origin: Origin,
        edit: impl FnOnce(&EditParams) -> Result<EditParams, aigle_doc::CommandError>,
        cx: &mut Context<Self>,
    ) -> Result<(), aigle_doc::CommandError> {
        let Some(key) = self.items.get(index).map(Item::key) else {
            return Ok(());
        };
        let current = self.meta_now(index).edits.unwrap_or_default();
        let next = edit(&current)?;
        if next == current {
            return Ok(());
        }
        let catalog = self.catalog.clone();
        let (history_key, label, after) = (key.clone(), label.to_string(), next.clone());
        cx.background_spawn(async move {
            if let Err(e) = catalog.push_history(&history_key, &current, &label, origin, &after) {
                tracing::warn!("{e}");
            }
        })
        .detach();
        self.photo_edited(&key, &Arc::new(next), cx);
        Ok(())
    }

    /// Store the snapshots of `key`.
    pub fn set_snapshots(
        &mut self,
        key: &PhotoKey,
        snapshots: Vec<Snapshot>,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.index_of(key) else {
            self.update_unlisted(key, cx, |m| m.snapshots = snapshots);
            return;
        };
        self.meta_now(i);
        if let Some(meta) = self.items[i].meta.as_mut()
            && meta.snapshots != snapshots
        {
            meta.snapshots = snapshots;
            self.save(i, cx);
        }
    }

    /// Make a virtual copy of each photo in `indices` with its current edits, rating and
    /// label, placed after the photo's other copies. Returns the keys of the new copies.
    pub fn create_copies(&mut self, indices: &[usize], cx: &mut Context<Self>) -> Vec<PhotoKey> {
        let base = crate::i18n::t("library.virtual_copy");
        let mut created = Vec::new();
        let mut failed = None;
        let mut indices = indices.to_vec();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for i in indices {
            let Some(path) = self.items.get(i).map(|it| it.entry.path.clone()) else {
                continue;
            };
            let src = self.meta_now(i);
            let meta = PhotoMeta {
                snapshots: Vec::new(),
                ..src
            };
            let name = crate::presets::unique_name(
                self.items
                    .iter()
                    .filter(|it| it.entry.path == path)
                    .filter_map(|it| it.copy.as_ref().map(|c| c.name.as_str())),
                &base,
            );
            match self.catalog.create_copy(&path, &name, &meta) {
                Ok(id) => {
                    let at = self
                        .items
                        .iter()
                        .rposition(|it| it.entry.path == path)
                        .map_or(self.items.len(), |p| p + 1);
                    let entry = self.items[i].entry.clone();
                    let info = CopyInfo { id, name };
                    self.items
                        .insert(at, Item::new(entry, Some(info), Some(meta)));
                    created.push(PhotoKey {
                        path,
                        copy: Some(id),
                    });
                }
                Err(e) => {
                    tracing::warn!("{e}");
                    failed.get_or_insert(e);
                }
            }
        }
        if let Some(e) = failed {
            cx.emit(LibraryEvent::Notice(
                rust_i18n::t!("library.copy_create_failed", e = e).into_owned(),
            ));
        }
        self.items_moved(cx);
        let new: BTreeSet<usize> = created.iter().filter_map(|k| self.index_of(k)).collect();
        if let Some(&first) = new.iter().next() {
            self.selection = new;
            self.selected = Some(first);
            self.anchor = Some(first);
        }
        cx.notify();
        created
    }

    /// Delete the virtual copies among `indices`; photos are left alone. Returns the keys
    /// of the deleted copies.
    pub fn remove_copies(&mut self, indices: &[usize], cx: &mut Context<Self>) -> Vec<PhotoKey> {
        let mut removed = Vec::new();
        let mut failed = None;
        for &i in indices {
            let Some(it) = self.items.get(i) else {
                continue;
            };
            let Some(c) = &it.copy else { continue };
            self.pending_copies.remove(&c.id);
            match self.catalog.remove_copy(c.id) {
                Ok(()) => removed.push(it.key()),
                Err(e) => {
                    tracing::warn!("{e}");
                    failed.get_or_insert(e);
                }
            }
        }
        if let Some(e) = failed {
            cx.emit(LibraryEvent::Notice(
                rust_i18n::t!("library.copy_delete_failed", e = e).into_owned(),
            ));
        }
        if removed.is_empty() {
            return removed;
        }
        let primary = self
            .selected_item()
            .map(|it| PhotoKey::master(&it.entry.path));
        for key in &removed {
            if let Some(i) = self.index_of(key) {
                let it = self.items.remove(i);
                if let Some(t) = it.thumb {
                    cx.drop_image(t, None);
                }
                self.reindex();
            }
        }
        self.items_moved(cx);
        let next = primary
            .and_then(|k| self.index_of(&k))
            .or((!self.items.is_empty()).then_some(0));
        self.set_single(next);
        cx.notify();
        removed
    }

    /// After items were inserted or removed: positions of jobs and the selection are stale.
    fn items_moved(&mut self, cx: &mut Context<Self>) {
        self.reindex();
        self.jobs.clear();
        self.refresh.clear();
        for it in &mut self.items {
            it.shown = it.meta.as_ref().and_then(|m| m.edits.clone()).map(Arc::new);
        }
        let n = self.items.len();
        self.selection.retain(|&i| i < n);
        self.selected = self.selected.filter(|&i| i < n);
        self.request_thumbs(cx);
    }

    /// Write every pending sidecar and catalog change now (closing a photo, quitting).
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.write_copies(cx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_names_keep_the_extension() {
        assert_eq!(
            copy_display_name("IMG_1.CR3", "Copy 2"),
            "IMG_1 (Copy 2).CR3"
        );
        assert_eq!(copy_display_name("raw", "Copy 1"), "raw (Copy 1)");
        assert_eq!(
            Path::new(&copy_display_name("a.b.nef", "C")).file_stem(),
            Some(std::ffi::OsStr::new("a.b (C)"))
        );
    }
}
