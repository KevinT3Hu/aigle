//! The open folder: its photos, their sidecar metadata, EXIF facts and thumbnails (doc 01
//! §9, doc 03 §8), and the filtered, sorted view of them that the grid and filmstrip show.
//!
//! Every photo and virtual copy of the folder is kept in storage order; the view lists the
//! ones that pass the [`Filter`] in [`Sort`] order. Public indices are view positions.
//!
//! Thumbnails are requested only for the view ranges the grid and the filmstrip currently
//! show; leaving a range drops its jobs, which cancels them in the engine. Thumbnails of
//! edited photos follow the edits about a second after the last change.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aigle_core::EditParams;
use aigle_doc::Origin;
use aigle_engine_api::{Engine, JobError, Priority, SourceRef};
use aigle_library::{
    Catalog, CopyId, Facts, Field, PhotoEntry, PhotoKey, PhotoMeta, Snapshot, Store, VirtualCopy,
    WriteReport, Writer,
};
use futures::StreamExt;
use gpui::{AppContext, Context, EventEmitter, RenderImage, Task};

use crate::canvas::render_image;
use crate::view::{Facets, Filter, Sort};

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
    /// `None` until the EXIF has been read.
    pub facts: Option<Facts>,
    pub thumb: Option<Arc<RenderImage>>,
    /// The edits the thumbnail should show; trails `meta.edits` by [`SAVE_DELAY`].
    shown: Option<Arc<EditParams>>,
    /// The edits `thumb` was made from.
    thumb_of: Option<Option<Arc<EditParams>>>,
    failed_for: Option<Option<Arc<EditParams>>>,
    /// Why the photo cannot be shown, when its thumbnail failed.
    pub unreadable: Option<String>,
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

/// The file name for renaming `file_name` to `stem`, keeping the extension; `None` when
/// `stem` is not a usable name (empty, hidden, or with a path separator).
pub fn renamed(file_name: &str, stem: &str) -> Option<String> {
    let stem = stem.trim();
    if stem.is_empty() || stem.starts_with('.') || stem.contains(['/', '\0']) {
        return None;
    }
    Some(match Path::new(file_name).extension() {
        Some(ext) => format!("{stem}.{}", ext.to_string_lossy()),
        None => stem.to_string(),
    })
}

impl Item {
    fn new(entry: PhotoEntry, copy: Option<CopyInfo>, meta: Option<PhotoMeta>) -> Item {
        Item {
            entry,
            copy,
            shown: meta.as_ref().and_then(|m| m.edits.clone()).map(Arc::new),
            meta,
            facts: None,
            thumb: None,
            thumb_of: None,
            failed_for: None,
            unreadable: None,
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

    fn set_meta(&mut self, meta: PhotoMeta) {
        self.shown = meta.edits.clone().map(Arc::new);
        self.meta = Some(meta);
    }

    #[cfg(test)]
    pub fn test(
        name: &str,
        copy: Option<CopyId>,
        meta: Option<PhotoMeta>,
        facts: Option<Facts>,
    ) -> Item {
        let entry = PhotoEntry {
            path: PathBuf::from("/p").join(name),
            name: name.to_string(),
            bytes: 1,
            modified: None,
        };
        let copy = copy.map(|id| CopyInfo {
            id,
            name: format!("Copy {id}"),
        });
        let mut it = Item::new(entry, copy, meta);
        it.facts = facts;
        it
    }
}

struct ThumbJob {
    key: PhotoKey,
    edits: Option<Arc<EditParams>>,
    _task: Task<()>,
}

/// What a file operation did to one photo.
type Outcome = (PathBuf, Result<(), String>);

/// A listed folder: its photos, their virtual copies and their cached EXIF facts.
type Listing = (Vec<PhotoEntry>, Vec<VirtualCopy>, HashMap<PathBuf, Facts>);

pub struct Library {
    engine: Engine,
    pub dir: Option<PathBuf>,
    /// Photos of subfolders are listed too.
    recursive: bool,
    /// Every photo and copy of the folder.
    all: Vec<Item>,
    /// Storage indices of the shown items, in order.
    view: Vec<usize>,
    /// Storage index by key.
    slots: HashMap<PhotoKey, usize>,
    /// View position by key.
    positions: HashMap<PhotoKey, usize>,
    filter: Filter,
    sort: Sort,
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

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn recursive_path() -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join("subfolders"))
}

fn load_recursive() -> bool {
    recursive_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.trim() == "on")
}

fn save_recursive(on: bool) {
    let Some(path) = recursive_path() else { return };
    let r = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, if on { "on\n" } else { "off\n" }));
    if let Err(e) = r {
        tracing::warn!(path = %path.display(), "{e}");
    }
}

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
                let name = file_name(&photo);
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
            recursive: load_recursive(),
            all: Vec::new(),
            view: Vec::new(),
            slots: HashMap::new(),
            positions: HashMap::new(),
            filter: Filter::default(),
            sort: Sort::default(),
            selected: None,
            selection: BTreeSet::new(),
            anchor: None,
            loading: false,
            store,
            writer,
            catalog,
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

    /// Shown photos.
    pub fn len(&self) -> usize {
        self.view.len()
    }

    /// Photos and copies in the folder, shown or not.
    pub fn total(&self) -> usize {
        self.all.len()
    }

    pub fn item(&self, index: usize) -> Option<&Item> {
        self.view.get(index).map(|&s| &self.all[s])
    }

    pub fn index_of(&self, key: &PhotoKey) -> Option<usize> {
        self.positions.get(key).copied()
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.item(self.selected?)
    }

    pub fn folder_name(&self) -> String {
        self.dir.as_deref().map(file_name).unwrap_or_default()
    }

    pub fn recursive(&self) -> bool {
        self.recursive
    }

    /// List the photos of subfolders too, or stop; the choice is remembered.
    pub fn set_recursive(&mut self, recursive: bool, cx: &mut Context<Self>) {
        if self.recursive == recursive {
            return;
        }
        self.recursive = recursive;
        save_recursive(recursive);
        if let Some(dir) = self.dir.clone() {
            let selected = self.selected_item().map(|it| it.entry.path.clone());
            self.open_folder(dir, selected, cx);
        }
        cx.notify();
    }

    /// The folder of `item` relative to the open folder, when it is a subfolder.
    pub fn subfolder(&self, item: &Item) -> Option<String> {
        let parent = item.entry.path.parent()?;
        let rel = parent.strip_prefix(self.dir.as_deref()?).ok()?;
        (!rel.as_os_str().is_empty()).then(|| rel.display().to_string())
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.rebuild_view(cx);
        }
    }

    pub fn set_sort(&mut self, sort: Sort, cx: &mut Context<Self>) {
        if self.sort != sort {
            self.sort = sort;
            self.rebuild_view(cx);
        }
    }

    /// The cameras, lenses, dates and keywords of the folder.
    pub fn facets(&self) -> Facets {
        Facets::of(&self.all)
    }

    fn key_at(&self, index: usize) -> Option<PhotoKey> {
        self.item(index).map(Item::key)
    }

    /// Storage indices changed: rebuild the key map and the view.
    fn reindex(&mut self, cx: &mut Context<Self>) {
        self.slots = self
            .all
            .iter()
            .enumerate()
            .map(|(i, it)| (it.key(), i))
            .collect();
        self.rebuild_view(cx);
    }

    /// Apply the filter and sort again, keeping the selected photos that are still shown.
    fn rebuild_view(&mut self, cx: &mut Context<Self>) {
        let selection: Vec<PhotoKey> = self
            .selection
            .iter()
            .filter_map(|&i| self.key_at(i))
            .collect();
        let primary = self.selected.and_then(|i| self.key_at(i));
        let anchor = self.anchor.and_then(|i| self.key_at(i));
        let had_selection = !selection.is_empty();

        let mut view: Vec<usize> = (0..self.all.len())
            .filter(|&s| self.filter.matches(&self.all[s]))
            .collect();
        view.sort_by(|&a, &b| self.sort.cmp(&self.all[a], &self.all[b]));
        self.positions = view
            .iter()
            .enumerate()
            .map(|(p, &s)| (self.all[s].key(), p))
            .collect();
        self.view = view;

        let pos = |k: &PhotoKey| self.positions.get(k).copied();
        self.selection = selection.iter().filter_map(pos).collect();
        self.selected = primary
            .as_ref()
            .and_then(pos)
            .or_else(|| self.selection.first().copied());
        self.anchor = anchor.as_ref().and_then(pos).or(self.selected);
        if had_selection && self.selection.is_empty() && !self.view.is_empty() {
            self.set_single(Some(0));
        }
        let n = self.view.len();
        for r in &mut self.wanted {
            *r = r.start.min(n)..r.end.min(n);
        }
        self.request_thumbs(cx);
        cx.notify();
    }

    /// Scan `dir`, then read every sidecar and EXIF in the background. `select` is selected
    /// once listed (the first photo otherwise).
    pub fn open_folder(&mut self, dir: PathBuf, select: Option<PathBuf>, cx: &mut Context<Self>) {
        self.flush(cx);
        self.generation += 1;
        self.jobs.clear();
        self.refresh.clear();
        self.drop_thumbs(cx);
        self.all.clear();
        self.view.clear();
        self.slots.clear();
        self.positions.clear();
        self.set_single(None);
        self.wanted = [0..0, 0..0];
        if self.dir.as_ref() != Some(&dir) {
            self.filter = self.filter.for_new_folder();
        }
        self.dir = Some(dir.clone());
        self.loading = true;
        let generation = self.generation;
        let recursive = self.recursive;
        let store = self.store.clone();
        let catalog = self.catalog.clone();
        cx.notify();
        self.scan = Some(cx.spawn(async move |this, cx| {
            let scan_dir = dir.clone();
            let lister = catalog.clone();
            let listed = cx
                .background_spawn(async move {
                    let entries = if recursive {
                        aigle_library::scan_recursive(&scan_dir)?
                    } else {
                        aigle_library::scan(&scan_dir)?
                    };
                    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
                    let copies = lister.copies_of(&paths).unwrap_or_else(|e| {
                        tracing::warn!("{e}");
                        Vec::new()
                    });
                    let facts = lister.facts_of(&entries).unwrap_or_else(|e| {
                        tracing::warn!("{e}");
                        HashMap::new()
                    });
                    Ok((entries, copies, facts))
                })
                .await;
            let entries = match this.update(cx, |l, cx| l.listed(generation, listed, select, cx)) {
                Ok(Some(e)) => e,
                _ => return,
            };
            let mut warnings = Vec::new();
            for chunk in entries.chunks(META_CHUNK) {
                let store = store.clone();
                let catalog = catalog.clone();
                let chunk = chunk.to_vec();
                let read = cx
                    .background_spawn(async move {
                        let mut fresh = Vec::new();
                        let read: Vec<_> = chunk
                            .into_iter()
                            .map(|(e, cached)| {
                                let r = store.read(&e.path);
                                let facts = (!cached).then(|| {
                                    let f = aigle_library::read_facts(&e.path);
                                    fresh.push((e.clone(), f.clone()));
                                    f
                                });
                                (e.path, r, facts)
                            })
                            .collect();
                        if !fresh.is_empty()
                            && let Err(e) = catalog.set_facts(&fresh)
                        {
                            tracing::warn!("{e}");
                        }
                        read
                    })
                    .await;
                let ok = this.update(cx, |l, cx| {
                    if l.generation != generation {
                        return false;
                    }
                    for (path, r, facts) in read {
                        l.loaded(path, r, facts, &mut warnings);
                    }
                    if crate::view::depends_on_loading(&l.filter, &l.sort) {
                        l.rebuild_view(cx);
                    } else {
                        l.request_thumbs(cx);
                        cx.notify();
                    }
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

    /// The sidecar of `path` was read, and its EXIF unless the facts were cached; its copies
    /// share the facts.
    fn loaded(
        &mut self,
        path: PathBuf,
        read: Result<aigle_library::Loaded, aigle_library::Error>,
        facts: Option<Facts>,
        warnings: &mut Vec<String>,
    ) {
        let Some(&s) = self.slots.get(&PhotoKey::master(&path)) else {
            return;
        };
        for it in self.all[s..]
            .iter_mut()
            .take_while(|it| it.entry.path == path)
            .filter(|_| facts.is_some())
        {
            it.facts = facts.clone();
        }
        let it = &mut self.all[s];
        if it.meta.is_some() {
            return;
        }
        let meta = match read {
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
        it.set_meta(meta);
    }

    /// The folder was listed; the photos are returned with whether their facts were cached.
    fn listed(
        &mut self,
        generation: u64,
        listed: std::io::Result<Listing>,
        select: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<(PhotoEntry, bool)>> {
        if self.generation != generation {
            return None;
        }
        let (entries, copies, mut facts) = match listed {
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
        let mut copies = copies.into_iter().peekable();
        let mut items = Vec::with_capacity(entries.len());
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = entry.path.clone();
            let known = facts.remove(&path);
            out.push((entry.clone(), known.is_some()));
            let mut it = Item::new(entry.clone(), None, None);
            it.facts = known.clone();
            items.push(it);
            while let Some(c) = copies.next_if(|c| c.path == path) {
                let info = CopyInfo {
                    id: c.id,
                    name: c.name,
                };
                let mut it = Item::new(entry.clone(), Some(info), Some(c.meta));
                it.facts = known.clone();
                items.push(it);
            }
        }
        self.all = items;
        self.reindex(cx);
        let first = select
            .and_then(|p| self.index_of(&PhotoKey::master(p)))
            .or((!self.view.is_empty()).then_some(0));
        self.set_single(first);
        cx.notify();
        Some(out)
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

    /// The metadata of storage slot `s`, reading the sidecar now if the background pass has
    /// not reached it yet.
    fn meta_at(&mut self, s: usize) -> PhotoMeta {
        let Some(it) = self.all.get(s) else {
            return PhotoMeta::default();
        };
        if it.meta.is_none() {
            let meta = self.read_meta(&it.entry.path);
            self.all[s].set_meta(meta);
        }
        self.all[s].meta.clone().unwrap_or_default()
    }

    /// The metadata of `index`, read now if needed.
    pub fn meta_now(&mut self, index: usize) -> PhotoMeta {
        match self.view.get(index) {
            Some(&s) => self.meta_at(s),
            None => PhotoMeta::default(),
        }
    }

    fn set_single(&mut self, index: Option<usize>) {
        self.selected = index;
        self.anchor = index;
        self.selection = index.into_iter().collect();
    }

    /// Select only `index`.
    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.len() && (self.selected != Some(index) || self.selection.len() != 1) {
            self.set_single(Some(index));
            cx.notify();
        }
    }

    /// Add `index` to the selection or take it out (Ctrl-click); the primary photo stays
    /// unless it is the one taken out.
    pub fn toggle_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.len() {
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
        if index >= self.len() {
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
        if self.view.is_empty() {
            return;
        }
        self.selection = (0..self.len()).collect();
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

    /// The selected photos in view order.
    pub fn selection(&self) -> Vec<usize> {
        self.selection.iter().copied().collect()
    }

    /// Show thumbnails for `range` of `who`; jobs outside every shown range are cancelled.
    pub fn want(&mut self, who: Consumer, range: Range<usize>, cx: &mut Context<Self>) {
        let n = self.len();
        let range = range.start.min(n)..range.end.min(n);
        if self.wanted[who as usize] != range {
            self.wanted[who as usize] = range;
            self.request_thumbs(cx);
        }
    }

    fn is_wanted(&self, index: usize) -> bool {
        self.wanted.iter().any(|r| r.contains(&index))
    }

    fn request_thumbs(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<usize> = self.wanted.iter().flat_map(|r| r.clone()).collect();
        let keys: HashSet<PhotoKey> = wanted.iter().filter_map(|&i| self.key_at(i)).collect();
        self.jobs.retain(|j| keys.contains(&j.key));
        let center = self.selected.unwrap_or(0);
        for i in wanted {
            let Some(it) = self.item(i) else { continue };
            if it.meta.is_none() {
                continue;
            }
            let key = it.key();
            let edits = it.shown.clone();
            let current = Some(edits.clone());
            if it.thumb_of == current
                || it.failed_for == current
                || self.jobs.iter().any(|j| j.key == key && j.edits == edits)
            {
                continue;
            }
            let path = it.entry.path.clone();
            self.jobs.retain(|j| j.key != key);
            let prio = 10_000 - (i.abs_diff(center).min(10_000) as i32);
            let job = self.engine.thumbnail(
                SourceRef::File(path),
                edits.clone(),
                THUMB_PX,
                Priority(prio),
            );
            let generation = self.generation;
            let (for_key, for_edits) = (key.clone(), edits.clone());
            let task = cx.spawn(async move |this, cx| {
                let r = job.await;
                let _ = this.update(cx, |l, cx| {
                    if l.generation != generation {
                        return;
                    }
                    l.jobs
                        .retain(|j| !(j.key == for_key && j.edits == for_edits));
                    let Some(&s) = l.slots.get(&for_key) else {
                        return;
                    };
                    let it = &mut l.all[s];
                    let shown = match r {
                        Ok(t) => {
                            render_image(&t.pixels).ok_or_else(|| crate::i18n::t("grid.unreadable"))
                        }
                        Err(JobError::Cancelled | JobError::Dropped) => return,
                        Err(e) => Err(e.to_string()),
                    };
                    match shown {
                        Ok(img) => {
                            if let Some(old) = it.thumb.replace(img) {
                                cx.drop_image(old, None);
                            }
                            it.thumb_of = Some(for_edits);
                            it.unreadable = None;
                        }
                        Err(why) => {
                            tracing::warn!(path = %it.entry.path.display(), "no thumbnail: {why}");
                            it.failed_for = Some(for_edits);
                            it.unreadable = Some(why);
                        }
                    }
                    l.evict(cx);
                    cx.notify();
                });
            });
            self.jobs.push(ThumbJob {
                key,
                edits,
                _task: task,
            });
        }
    }

    /// Drop the thumbnails furthest from the shown ranges until at most [`KEEP`] remain;
    /// photos the filter hides go first.
    fn evict(&mut self, cx: &mut Context<Self>) {
        let mut position = vec![None; self.all.len()];
        for (p, &s) in self.view.iter().enumerate() {
            position[s] = Some(p);
        }
        let mut held: Vec<usize> = (0..self.all.len())
            .filter(|&s| {
                self.all[s].thumb.is_some() && !position[s].is_some_and(|p| self.is_wanted(p))
            })
            .collect();
        let total = held.len() + self.wanted.iter().map(|r| r.len()).sum::<usize>();
        if total <= KEEP {
            return;
        }
        let distance = |s: usize| {
            let Some(i) = position[s] else {
                return usize::MAX;
            };
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
        held.sort_by_key(|&s| std::cmp::Reverse(distance(s)));
        for s in held.into_iter().take(total - KEEP) {
            let it = &mut self.all[s];
            if let Some(old) = it.thumb.take() {
                cx.drop_image(old, None);
            }
            it.thumb_of = None;
        }
    }

    fn drop_thumbs(&mut self, cx: &mut Context<Self>) {
        for it in &mut self.all {
            if let Some(old) = it.thumb.take() {
                cx.drop_image(old, None);
            }
        }
    }

    fn save(&mut self, s: usize, cx: &mut Context<Self>) {
        let Some(it) = self.all.get(s) else {
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

    /// Change the metadata of `index` with `change`, saving it if `change` returns true.
    fn change_meta(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut PhotoMeta) -> bool,
    ) {
        let Some(&s) = self.view.get(index) else {
            return;
        };
        self.meta_at(s);
        if let Some(meta) = self.all[s].meta.as_mut()
            && change(meta)
        {
            self.save(s, cx);
            cx.notify();
        }
    }

    pub fn set_rating(&mut self, index: usize, rating: i8, cx: &mut Context<Self>) {
        self.change_meta(index, cx, |m| {
            std::mem::replace(&mut m.rating, rating) != rating
        });
    }

    pub fn set_label(&mut self, index: usize, label: Option<&str>, cx: &mut Context<Self>) {
        self.change_meta(index, cx, |m| {
            let label = label.map(str::to_string);
            std::mem::replace(&mut m.label, label.clone()) != label
        });
    }

    pub fn set_field(&mut self, index: usize, field: Field, value: &str, cx: &mut Context<Self>) {
        self.change_meta(index, cx, |m| {
            let before = m.field(field).to_string();
            m.set_field(field, value);
            m.field(field) != before
        });
    }

    /// Add the keywords `add` to `index` and take out `remove`, as typed or shown by
    /// [`PhotoMeta::tags`] (`Places > France` is a hierarchical keyword); compared
    /// case-insensitively.
    pub fn edit_keywords(
        &mut self,
        index: usize,
        add: &[String],
        remove: &[String],
        cx: &mut Context<Self>,
    ) {
        let norm = |k: &String| aigle_library::normalize_tag(k).map(|k| k.to_lowercase());
        let gone: HashSet<String> = remove.iter().filter_map(norm).collect();
        self.change_meta(index, cx, |m| {
            let before = (m.keywords.clone(), m.hierarchy.clone());
            let tags: Vec<String> = m
                .tags()
                .into_iter()
                .filter(|k| !norm(k).is_some_and(|k| gone.contains(&k)))
                .chain(add.iter().cloned())
                .collect();
            m.set_tags(tags);
            (m.keywords.clone(), m.hierarchy.clone()) != before
        });
    }

    /// The open photo `key` has new edits: save them and refresh its thumbnail shortly.
    pub fn photo_edited(&mut self, key: &PhotoKey, head: &Arc<EditParams>, cx: &mut Context<Self>) {
        let edits = (**head != EditParams::default()).then(|| (**head).clone());
        let Some(&s) = self.slots.get(key) else {
            self.update_unlisted(key, cx, |m| m.edits = edits);
            return;
        };
        self.meta_at(s);
        let Some(meta) = self.all[s].meta.as_mut() else {
            return;
        };
        if meta.edits == edits {
            return;
        }
        meta.edits = edits;
        self.save(s, cx);
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
                if let Some(&s) = l.slots.get(&refresh_key) {
                    l.all[s].shown = shown;
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
        let Some(key) = self.key_at(index) else {
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
        let Some(&s) = self.slots.get(key) else {
            self.update_unlisted(key, cx, |m| m.snapshots = snapshots);
            return;
        };
        self.meta_at(s);
        if let Some(meta) = self.all[s].meta.as_mut()
            && meta.snapshots != snapshots
        {
            meta.snapshots = snapshots;
            self.save(s, cx);
        }
    }

    /// Make a virtual copy of each photo in `indices` with its current edits, rating,
    /// label and descriptions, placed after the photo's other copies. Returns the keys of
    /// the new copies.
    pub fn create_copies(&mut self, indices: &[usize], cx: &mut Context<Self>) -> Vec<PhotoKey> {
        let base = crate::i18n::t("library.virtual_copy");
        let mut created = Vec::new();
        let mut failed = None;
        let mut slots: Vec<usize> = indices
            .iter()
            .filter_map(|&i| self.view.get(i).copied())
            .collect();
        slots.sort_unstable_by(|a, b| b.cmp(a));
        for s in slots {
            let path = self.all[s].entry.path.clone();
            let src = self.meta_at(s);
            let meta = PhotoMeta {
                snapshots: Vec::new(),
                ..src
            };
            let name = crate::presets::unique_name(
                self.all
                    .iter()
                    .filter(|it| it.entry.path == path)
                    .filter_map(|it| it.copy.as_ref().map(|c| c.name.as_str())),
                &base,
            );
            match self.catalog.create_copy(&path, &name, &meta) {
                Ok(id) => {
                    let at = self
                        .all
                        .iter()
                        .rposition(|it| it.entry.path == path)
                        .map_or(self.all.len(), |p| p + 1);
                    let mut item = Item::new(
                        self.all[s].entry.clone(),
                        Some(CopyInfo { id, name }),
                        Some(meta),
                    );
                    item.facts = self.all[s].facts.clone();
                    self.all.insert(at, item);
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
        self.reindex(cx);
        let new: BTreeSet<usize> = created.iter().filter_map(|k| self.index_of(k)).collect();
        if let Some(&first) = new.iter().next() {
            self.selection = new;
            self.selected = Some(first);
            self.anchor = Some(first);
        }
        cx.notify();
        created
    }

    /// Take the items of `keys` out of storage (the caller reindexes).
    fn take_items(&mut self, keys: &HashSet<PhotoKey>) -> Vec<Item> {
        let mut taken = Vec::new();
        let mut i = 0;
        while i < self.all.len() {
            if keys.contains(&self.all[i].key()) {
                taken.push(self.all.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    }

    fn drop_items(items: Vec<Item>, cx: &mut Context<Self>) {
        for it in items {
            if let Some(t) = it.thumb {
                cx.drop_image(t, None);
            }
        }
    }

    /// After items were removed: select the photo that was primary if it is still shown,
    /// else the first one.
    fn reselect(&mut self, primary: Option<PhotoKey>) {
        let next = primary
            .and_then(|k| self.index_of(&k))
            .or((!self.view.is_empty()).then_some(0));
        self.set_single(next);
    }

    /// Delete the virtual copies among `indices`; photos are left alone. Returns the keys
    /// of the deleted copies.
    pub fn remove_copies(&mut self, indices: &[usize], cx: &mut Context<Self>) -> Vec<PhotoKey> {
        let mut removed = Vec::new();
        let mut failed = None;
        for &i in indices {
            let Some(it) = self.item(i) else { continue };
            let Some(c) = &it.copy else { continue };
            let (id, key) = (c.id, it.key());
            self.pending_copies.remove(&id);
            match self.catalog.remove_copy(id) {
                Ok(()) => removed.push(key),
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
        let gone = self.take_items(&removed.iter().cloned().collect());
        Self::drop_items(gone, cx);
        self.reindex(cx);
        self.reselect(primary);
        cx.notify();
        removed
    }

    /// The photo files behind `indices` (a virtual copy stands for its photo), in view
    /// order without repeats.
    pub fn photo_paths(&self, indices: &[usize]) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        indices
            .iter()
            .filter_map(|&i| self.item(i))
            .map(|it| it.entry.path.clone())
            .filter(|p| seen.insert(p.clone()))
            .collect()
    }

    /// Rename the photo at `index` to `stem` (keeping its extension) with its sidecars,
    /// copies and history, or rename a virtual copy. Returns the new key. The caller closes
    /// the photo in develop first.
    pub fn rename(
        &mut self,
        index: usize,
        stem: &str,
        cx: &mut Context<Self>,
    ) -> Result<PhotoKey, String> {
        let Some(it) = self.item(index) else {
            return Err(String::new());
        };
        let key = it.key();
        if let Some(c) = &it.copy {
            let id = c.id;
            let name = stem.trim().to_string();
            if name.is_empty() {
                return Err(crate::i18n::t("library.bad_name"));
            }
            self.catalog
                .rename_copy(id, &name)
                .map_err(|e| e.to_string())?;
            if let Some(&s) = self.slots.get(&key)
                && let Some(c) = self.all[s].copy.as_mut()
            {
                c.name = name;
            }
            self.rebuild_view(cx);
            return Ok(key);
        }
        let from = key.path.clone();
        let name =
            renamed(&it.entry.name, stem).ok_or_else(|| crate::i18n::t("library.bad_name"))?;
        let to = from.with_file_name(&name);
        if to == from {
            return Ok(key);
        }
        self.flush(cx);
        aigle_library::move_photo(&from, &to).map_err(|e| e.to_string())?;
        if let Err(e) = self.catalog.move_photo(&from, &to) {
            tracing::warn!("{e}");
        }
        self.store.forget(&from);
        self.refresh.retain(|k, _| k.path != from);
        for it in self.all.iter_mut().filter(|it| it.entry.path == from) {
            it.entry.path = to.clone();
            it.entry.name = name.clone();
        }
        self.reindex(cx);
        Ok(PhotoKey::master(to))
    }

    /// Run `op` on each photo file of `paths` in the background. Their items leave
    /// the library while it runs; the ones `op` fails on come back. `done` reports the
    /// outcome. With `relist`, the folder is listed again afterwards (photos moved within
    /// the listed folders).
    fn file_op(
        &mut self,
        paths: Vec<PathBuf>,
        op: impl Fn(&Path) -> Result<(), String> + Send + 'static,
        done: impl FnOnce(&[Outcome]) -> Option<String> + 'static,
        relist: bool,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.flush(cx);
        let primary = self.selected_item().map(Item::key);
        let keys: HashSet<PhotoKey> = self
            .all
            .iter()
            .filter(|it| paths.contains(&it.entry.path))
            .map(Item::key)
            .collect();
        for p in &paths {
            self.store.forget(p);
        }
        self.refresh.retain(|k, _| !paths.contains(&k.path));
        let taken = self.take_items(&keys);
        self.reindex(cx);
        self.reselect(primary);
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            let outcomes: Vec<Outcome> = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let r = op(&p);
                            (p, r)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |l, cx| {
                let failed: HashSet<&Path> = outcomes
                    .iter()
                    .filter(|(_, r)| r.is_err())
                    .map(|(p, _)| p.as_path())
                    .collect();
                let (back, gone): (Vec<Item>, Vec<Item>) = taken.into_iter().partition(|it| {
                    l.generation == generation && failed.contains(it.entry.path.as_path())
                });
                Self::drop_items(gone, cx);
                if !back.is_empty() {
                    l.all.extend(back);
                    l.reindex(cx);
                }
                if let Some(msg) = done(&outcomes) {
                    cx.emit(LibraryEvent::Notice(msg));
                }
                if relist
                    && l.generation == generation
                    && let Some(dir) = l.dir.clone()
                {
                    let selected = l.selected_item().map(|it| it.entry.path.clone());
                    l.open_folder(dir, selected, cx);
                }
                cx.notify();
            });
        });
        task.detach();
    }

    /// Move the photos behind `indices` into `dest` with their sidecars, virtual copies and
    /// history. The caller closes the photo in develop first.
    pub fn move_photos(&mut self, indices: &[usize], dest: PathBuf, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .photo_paths(indices)
            .into_iter()
            .filter(|p| p.parent() != Some(dest.as_path()))
            .collect();
        let relist = self.recursive && self.dir.as_ref().is_some_and(|d| dest.starts_with(d));
        let catalog = self.catalog.clone();
        let folder = file_name(&dest);
        let op = move |from: &Path| {
            let to = dest.join(from.file_name().unwrap_or_default());
            aigle_library::move_photo(from, &to).map_err(|e| e.to_string())?;
            if let Err(e) = catalog.move_photo(from, &to) {
                tracing::warn!("{e}");
            }
            Ok(())
        };
        let done = move |outcomes: &[Outcome]| {
            Some(report(outcomes, |n| {
                rust_i18n::t!("library.moved", arg0 = n, folder = folder).into_owned()
            }))
        };
        self.file_op(paths, op, done, relist, cx);
    }

    /// Move the photo files `paths` to the trash with their sidecars, and forget their
    /// virtual copies and history. The caller asks first and closes the photo in develop.
    pub fn trash_photos(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let catalog = self.catalog.clone();
        let op = move |p: &Path| {
            aigle_library::trash_photo(p).map_err(|e| e.to_string())?;
            if let Err(e) = catalog.forget_photo(p) {
                tracing::warn!("{e}");
            }
            Ok(())
        };
        let done = |outcomes: &[Outcome]| {
            Some(report(outcomes, |n| {
                rust_i18n::t!("library.trashed", arg0 = n).into_owned()
            }))
        };
        self.file_op(paths, op, done, false, cx);
    }

    /// Write every pending sidecar and catalog change now (closing a photo, quitting).
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.write_copies(cx);
        self.writer.flush();
    }
}

/// The notice after a file operation: the first failure, or `ok(count)`.
fn report(outcomes: &[Outcome], ok: impl FnOnce(usize) -> String) -> String {
    let failures: Vec<(&PathBuf, &String)> = outcomes
        .iter()
        .filter_map(|(p, r)| r.as_ref().err().map(|e| (p, e)))
        .collect();
    match failures.first() {
        None => ok(outcomes.len()),
        Some((p, e)) => rust_i18n::t!(
            "library.file_op_failed",
            arg0 = failures.len(),
            name = file_name(p),
            e = e
        )
        .into_owned(),
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

    #[test]
    fn renaming_keeps_the_extension() {
        assert_eq!(renamed("IMG_1.CR3", " sea ").as_deref(), Some("sea.CR3"));
        assert_eq!(renamed("raw", "b").as_deref(), Some("b"));
        for bad in ["", "  ", ".hidden", "a/b"] {
            assert_eq!(renamed("IMG_1.CR3", bad), None, "{bad:?}");
        }
    }
}
