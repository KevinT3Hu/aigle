# Qt restart — workflow parity inventory

> Historical design/migration record: the GPUI frontend and vendored fork have
> been removed. Current GUI sources and build instructions are in `apps/aigle/`.

Initial source inventory, 2026-11-07. Companion to the
[design brief](07-qt-gui-restart.md). Paths below are relative to the repository
root. **No row is runtime-verified by this inventory.**

Qt is the sole release GUI. GPUI serves only as a development reference. There
are no existing users; settings, credentials, and saved-search migration are not
release requirements.

All baseline workflows below are required for replacement unless an explicit
design decision changes them. “Partial” means some code exists, not that it is
usable or equivalent. “Audit” means insufficient evidence to classify coverage.
These are workflow families; Gate A must expand them into individual acceptance
cases, not close a family after testing one example.

Legacy source prefix: `crates/aigle/app/src/`. Qt source prefix:
`apps/aigle/src/`. Bridge: `crates/aigle/qt-bridge/src/lib.rs`.

| ID | Baseline workflow / evidence | Qt attempt: source-level finding | Proposed destination and acceptance case |
| --- | --- | --- | --- |
| L01 | Open photo/folder, subfolders, import; `workspace.rs`, `import.rs`, `library.rs` | Partial: open/scan/import commands and menu exist; full import options/subfolder behavior need audit | Library sources + import dialog; import with progress/cancel, verify outputs, reopen source and preserve selection |
| L02 | Grid/filmstrip, navigation, multi-selection, filter/sort; `grid.rs`, `filmstrip.rs`, `library_bar.rs` | Partial: QListWidget grid/filmstrip; full selection/filter/sort parity needs audit | Shared library model; modifier/range selection and navigation agree in both views; large library remains responsive |
| L03 | Metadata search, exclusions, saved filters/sort; `library.rs`, `saved_searches.rs` | Implemented in Qt: metadata terms/exclusions, rating/flag/label/camera/lens/date/keyword filters, four sort keys, persistent saved searches; large-library/platform runtime audit remains | Library search + Saved searches; match keyword/camera and excluded term, save criteria, restart and reopen |
| L04 | Stars, independent pick/reject flags, color labels; `actions.rs`, `library.rs` | Partial: rating/tag editing visible; remaining culling controls need audit | Grid/filmstrip + selection inspector; batch flag/label/rating persist independently and filter correctly |
| L05 | 2–4 photo survey, linked zoom/pan, reject/remove; `compare.rs` | Implemented SDR: 2–4 full-resolution edited-master viewport renders, actual pixels, linked relative zoom/pan, pick/reject and removal; HDR and large-RAW performance audit remain | Compare selected; link views at consistent magnification and remove a candidate without deleting its file |
| L06 | Albums and missing members; `workspace.rs`, `library.rs` | Implemented: create/open, add/remove members, rename/delete, missing-member indication and retained album identity; native interaction audit pending | Sources → Albums; add/remove members, delete album without deleting originals, show missing member |
| L07 | Manual stacks and exact duplicate scanning/cancel; `library.rs`, `library_bar.rs` | Implemented: stack/unstack/collapse in current source; cancellable exact-file duplicate scan with review selection and failures | Library actions; stack/unstack persists, exact duplicate results distinguish virtual copies and never auto-delete |
| L08 | Rename/move/trash originals and sidecars; `rename.rs`, `library.rs` | Implemented: single rename, queued move/trash, sidecars/catalog identity, per-file errors and cancel remaining; native/platform failure-path audit pending | Selection actions; preview affected files, cancellation and failure reporting, catalog identity follows move |
| L09 | Editable EXIF/IPTC-related metadata/keywords, batch mixed values; `metadata_panel.rs` | Partial: information and rating/tags exist; full editable fields/batch semantics need audit | Selection metadata inspector; edit one field across selection without replacing unrelated metadata |
| D01 | Global adjustment groups; `panel.rs`, `meta.rs` | Partial: many schema-driven scalar controls exist | Edit inspector; inventory every path, unit, range, enable condition, section reset and undo behavior |
| D02 | Point RGB/channel and parametric curves; `curve.rs`, `panel.rs` | Partial: custom point-curve widget and scalar controls exist | Curves group; add/move/remove points, switch channels, reset, undo a whole gesture |
| D03 | Camera profiles, looks/LUTs, lens matching, RAW controls; `panel.rs` | Partial: several structured editors substitute for dedicated workflows | Profile/RAW and Optics groups; import/select profile, lens match, unsupported-file errors and persisted choice |
| D04 | Crop/aspect/orientation/perspective/straighten; `crop.rs`, `tools.rs`, `panel.rs` | Partial: canvas crop and generic crop/aspect editing; interaction parity unproven | Crop inspector + handles; constrained resize, custom ratio, flip/rotate, horizon straighten, Apply/Cancel/undo |
| D05 | Heal/clone/red-eye creation and correction editing; `retouch_panel.rs`, `tools.rs` | Partial: creation gestures and structured collections | Retouch inspector; select existing correction, move source/target and resize, delete, undo, reopen |
| D06 | Brush/linear/radial/luminance/color masks and local parameters; `mask_panel.rs`, `tools.rs` | Gap/partial: creation exists; graphical-handle gap acknowledged; collection editor is not parity | Mask inspector; create, select, manipulate handles/feather, sample range, adjust/invert/rename/remove and persist |
| D07 | Add/subtract/intersect captured mask coverage; `mask_panel.rs`, feature-expansion record | Audit: structured data exposure alone is insufficient | Mask list actions; combine coverage with correct inversion, explain snapshot behavior and show depth-limit errors |
| D08 | History, gesture undo/redo, snapshots; `history.rs`, `photo.rs` | Partial: goto/undo/redo/snapshot commands exist | History/Snapshots rail; one drag = one undo, branch history, restore snapshot, restart and inspect persisted history |
| D09 | Virtual copies and removal; `library.rs`, `workspace.rs` | Implemented: create/list/open/rename/delete current copy; master and copy history separation covered by bridge tests; grid copy presentation remains partial | Library copy identity + photo actions; independent edits/history/metadata; master sidecar never overwritten by copy |
| D10 | Presets and selective copy/paste/sync; `presets.rs`, `copy_settings.rs` | Implemented: preset load/save, selective 17-group clipboard and batch master sync with preserved unchecked groups, per-target undo/history and failure reports; native sync interaction audit pending | Presets rail + selective settings dialog; copy chosen groups, batch sync explicitly, preserve unchecked groups |
| V01 | Fit/1:1/zoom/pan, original/split comparison, clipping; `canvas.rs`, `viewport.rs` | Partial: core navigation/overlays exist | Canvas toolbar; cursor anchoring, crop/orientation transforms, before/after and clipping agree across zoom states |
| V02 | Histograms, cumulative, waveform, RGB parade, vectorscope, region analysis; `scopes.rs`, `docs/development/scopes.md` | Implemented: seven Qt modes use shared Rust analysis; exact full-crop bins, bounded region/SDR-HDR density analysis, cached mouse/keyboard readouts, gamut targets and reference guides; numerical, bridge, Qt interaction tests and native 1440/1024 captures verified, physical HDR/display validation remains under V03/X01 | Scope selector in left rail; modes/channels/scaling/region/hover match analysis semantics, with correct SDR/HDR units |
| V03 | Display management, soft proof, gamut warnings, HDR mastering/presentation; `display.rs`, `canvas.rs`, `panel.rs` | Partial: QRhi HDR and mastering parameters; proofing/display preference parity needs audit | Display status + View controls + HDR inspector; test supported HDR and explicit fallback, screen transitions, proof targets |
| E01 | Single/batch export formats and options; `export.rs` | Gap: eight hard-coded formats and limited options vs legacy capability-filtered format/options UI | Export dialog; audit all supported formats including DNG restrictions, quality/size/color/naming, metadata, watermarks, sharpening, presets, paired SDR |
| E02 | Persistent queue, per-file progress, pause/resume/retry/cancel/collisions; `export_queue.rs` | Implemented: durable source/edit/options snapshots, paused restart recovery, per-file status/errors, pause/resume/retry/cancel and collision refusal; encoder percentage and full native interaction audit pending | Jobs → Exports; restart with pending work paused, retry failure, refuse accidental overwrite and explain collisions |
| P01 | Sidecar autosave, external conflicts, catalog history; `photo.rs`, `workspace.rs` | Partial: autosave/conflict handling exists | Persistent save status; external modification test, save failure recovery, close/reopen persistence |
| P02 | Catalog backup/validated restore/recovery snapshot; `workspace.rs`, feature-expansion record | Implemented: backup, validated restore, automatic recovery snapshot and rollback attempt; bridge/catalog round-trip tests pass; native interaction audit pending | File menu/dialogs; restore validation and recovery; explain that originals/sidecars are not in catalog backup |
| P03 | Locate missing original and move related state; `library.rs`, feature-expansion record | Implemented: single missing-original relink, sidecar/catalog conflict refusal and identity preservation; bulk relinking remains separate backlog | Missing-photo state; relink with sidecar/catalog conflicts handled without overwriting existing edits |
| S01 | General preferences, English/Chinese, display/subfolders; `settings.rs`, `i18n.rs` | Partial: persistent staged preferences for recursive scanning, thumbnails, HDR default and autosave; localization and remaining settings pending | Preferences; staged Save/Cancel and restart persistence |
| S02 | Configurable commands/chords and conflict-aware editor; `keymap.rs`, `keybinding_editor.rs` | Partial: persistent main-window bindings/chords, conflict detection, defaults and staged Save/Cancel; fixed dialog shortcuts and unbound commands remain | Preferences → Shortcuts; record chord, replace conflict explicitly, cancel/save, retain text-input priority |
| A01 | Multi-provider/model configuration, discovery, secure credential storage; `providers.rs`, `settings.rs` | Implemented for OpenAI-compatible providers on Linux/FreeBSD: named profile selection/add/edit/remove, persistent per-profile model lists/selection, asynchronous discovery with manual fallback and stale-result rejection, profile/endpoint-scoped Secret Service credentials; live-provider/vision capability and other-platform secure storage audit remain | Preferences → Providers; store and restore Qt keys securely, add/remove providers, capability/error feedback and selected model persistence |
| A02 | Per-photo chat, Markdown, profiles/images, usage, cancel, stale-edit protection; `agent_ui.rs`, `agent.rs` | Partial/different: preview/Apply flow; path/copy-scoped persistent transcripts and follow-up context implemented with widget coverage; move/rename identity, Markdown/images and live-provider parity need audit | Agent inspector; successful/cancelled/failed run, navigate or edit during run, reject stale result, undo accepted edit |
| R01 | Thesauros checkout/edit/sync/error behavior; `main.rs`, `crates/remote`, server README | Partial: remote_open exists; end-to-end sync/auth/conflict parity needs audit | Remote source workflow; authenticate, checkout, edit/save/sync, disconnect and conflict recovery |
| X01 | Keyboard/focus/accessibility, localization, HiDPI, packaging | Existing smoke tests are not full UX coverage | Every surface; test keyboard-only operation, names/roles, text scaling, Chinese labels, supported Linux display backends and packaged launch |

## How a row becomes complete

For each individual acceptance case record:

1. Baseline evidence: source symbol, runnable reference behavior, and screenshot
   or short task recording. If source/docs disagree, record the discrepancy.
2. Destination screen and interaction specification, including keyboard path.
3. Required Rust service/bridge capability; existing, extraction needed, or new.
4. Fixture/setup, user actions, expected visible result, persisted result, undo
   semantics, and failure/cancellation behavior where applicable.
5. Status: not audited / designed / implemented / verified / accepted deviation.
6. Verification evidence and date; an accepted deviation names the user decision.

Only verified cases or explicitly accepted deviations satisfy the replacement
gate. A bridge unit test does not verify a GUI workflow; a screenshot does not
verify persistence, color output, or cancellation. Recheck the action registry,
parameter schema, menus, and context menus for omissions before closing Gate A.

## Existing unfinished features — separate backlog

`docs/development/feature-expansion.md` explicitly lists these as outstanding in
the original scope. They must not be mislabeled as Qt regressions or silently
represented by nonfunctional controls in mockups:

- Subject/sky/background inference and editable combined-mask components.
- Bulk folder relinking, persistent offline previews, backups including originals
  and sidecars.
- AI denoise, improved per-camera noise models, RAW-domain chroma reduction.
- HDR bracket merge, panoramas, focus stacking.
- Guided perspective and targeted on-image tone/color adjustment.
- Custom printer/paper ICC proofing, intent selection, print layouts.

Workspace customization and capture/tethering were explicitly excluded in that
record. Basic sidebar resizing/hiding in the proposed design is not a new docking
or workspace-customization project.

## Validation of this document

Initial evidence comes from the current working tree, including uncommitted Qt
work. The application was not built or launched for this planning pass. Runtime
parity, actual visual defects, performance, and display output remain Gate A/C
work. This ledger deliberately avoids marking an entire feature complete merely
because an action, command, or parameter exists.
