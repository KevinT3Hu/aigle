# Native Qt implementation review

> Historical design/migration record: the GPUI frontend and vendored fork have
> been removed. Current GUI sources and build instructions are in `apps/aigle/`.

2026-11-07. The user approved the visual direction and requested its implementation
in Qt6. The frontend now uses native Qt Widgets and the existing Vulkan canvas;
the HTML prototype is only the visual reference.

## Captures

| State | Native capture | Reference |
| --- | --- | --- |
| Develop | [1440×900](develop-1440.png) | [Design](../screenshots/develop-1440.png) |
| Library | [1440×900](library-1440.png) | [Design](../screenshots/library-1440.png) |
| Masks | [1440×900](masks-1440.png) | [Design](../screenshots/masks-1440.png) |
| Crop | [1440×900](crop-1440.png) | [Design](../screenshots/crop-1440.png) |
| Agent | [1440×900](agent-1440.png) | [Design](../screenshots/agent-1440.png) |
| Export | [1440×900](export-1440.png) | [Design](../screenshots/export-1440.png) |
| Compact Develop | [1024×700](develop-1024.png) | [Design](../screenshots/develop-1024.png) |
| Compact Library | [1024×700](library-1024.png) | [Design](../screenshots/library-1024.png) |

Captured on Linux through XCB at device scale 2, normalized to logical pixels.
The main Develop photo bounds match the reference: x250, y154, 837×558. Title,
menu, toolbar, reference rail, inspector, canvas footer and filmstrip follow the
approved dimensions and neutral palette. Qt font rendering and native widget
metrics introduce small differences from Chromium.

All photographs are temporary copies of the design JPEGs. Names, metadata,
histogram and adjustment values come from the application. The Masks capture
uses two real masks saved through the bridge into the temporary photo's sidecar;
no production capture code inserts fixture edits. Current mask swatches evaluate geometric coverage through the production kernel,
including opacity/inversion; range and semantic swatches remain unavailable.
The screenshots above predate this change. The Agent capture shows
the unconfigured empty state; no model request or fictional conversation was
used. Crop shows the actual full-image draft, not the prototype's sample crop.

## Implemented

- Responsive Library/Develop shell, fit-inside thumbnails, selection borders,
  rating controls, filename filtering, sorting and thumbnail sizing.
- Full-height Edit/Agent inspector, compact adjustment rows, search, grouped
  controls, history/snapshots/presets, and metadata access. Existing adjustment
  paths remain available.
- Staged crop rectangle, corner and move handles, aspect locking, angle preview,
  Apply/Cancel and Enter/Escape. Crop plus angle is one undoable bridge edit.
- Named mask list, brush/linear/radial creation, linear/radial guide manipulation,
  overlay/invert, local adjustments, duplicate/delete/rename, luminance range
  creation, and Add/Subtract/Intersect coverage combination.
- Agent conversation/composer, provider configuration dialog, actual proposal
  summary and preview, explicit Apply/Discard, and cancellation.
- Export dialog with engine-derived format capabilities, presets, output naming,
  dimensions/color summary, metadata policy, sharpening, watermark and paired
  SDR delivery. Export jobs durably retain source, edit, and option snapshots.
  Cancel marks pending jobs cancelled and cancels the active export.

## Verification

- Full CMake Qt build passed.
- All five CTest targets passed. Widget tests cover as-shot WB preservation,
  search, slider gesture grouping, mask adjustment preservation, luminance range
  creation, and separate HDR/SDR export options, in addition to value editors.
  Added regression coverage for mask swatch list rebuilds and Escape/focus-loss/
  pointer-ungrab cancellation followed by release.
- Rust bridge tests passed (5 tests), including transformed radial handles, grouped
  mask translation, coverage opacity/inversion, crop/angle undo, mask geometry
  updates, export, sidecar/catalog round trips and frame ownership.
- Native synthetic edit → undo → redo → render smoke test passed on XCB.
- Visually inspected all six desktop states and both compact states above.
  After the mask update, also checked native coverage swatches and a 37-degree
  radial mask on a photo with straighten and perspective correction.
- GPU processing failed its existing self-test by four code values and fell back
  to CPU. Vulkan presentation worked. This run does not certify GPU processing,
  HDR output, display transitions, screen readers or other desktop platforms.

## Remaining differences and release limits

This implements the visual foundation and connected editing paths, but is not a
pixel-identical reproduction of every simulated state or a full legacy parity
certificate. In particular:

- The Crop panel now includes drawn-horizon straightening and selectable thirds,
  center-cross, and hidden guides, plus staged rotate-left/right and horizontal/vertical
  flip controls. Orientation previews use the engine pending layer; Cancel drops
  the preview and Apply commits orientation, angle, and crop in one undo step.
  The screenshots above predate these additional controls.
- Radial guides now project a sampled ellipse and separate radius handles through
  image geometry. Brush and fully geometric combined masks have translation handles;
  translation preserves relative shape and clamps the group to source bounds.
  Escape, focus loss, pointer ungrab, zoom, and tool changes cancel unfinished drags.
  Radius handles update the exact outline after release; live drag guides remain
  an approximation under nonlinear perspective/lens transforms. Range/semantic
  components cannot yet be moved geometrically or shown in coverage swatches.
- Library multi-selection supports batch ratings, color labels, and pick/reject
  flags with per-file failure reports. Selection metadata actions also add, remove,
  or replace keywords and set/clear one descriptive field while preserving unrelated
  metadata and edits. Removal matches whole keywords or hierarchy paths, ignoring
  case; replacing with empty input clears keywords. Batch actions target master
  photos, not virtual copies.
  Bridge regression coverage checks keyword modes, hierarchy normalization, field
  preservation, persistence, partial failures, and active-photo synchronization.
- Albums now support adding selected photos, removing selected members, renaming,
  and deleting the album without touching originals. Album identity stays active
  when opening its members across folders, and missing originals remain listed.
  Virtual copies can be renamed or deleted from Photo actions. Deletion opens the
  master first and removes copy history, preventing autosave from reviving the copy.
  Bridge/catalog tests cover duplicate membership, rename conflicts, persistence,
  missing-master failure, copy-history removal, and unchanged original/sidecar bytes.
  Native end-to-end interaction coverage for these new menu flows remains pending.
- Library actions now create stacks, unstack all members of selected stacks in
  the current folder/album, and collapse each stack to its first matching member
  in the active sort. Stack badges show membership counts in the current source.
  Exact-duplicate scanning runs on a cancellable worker, groups identical file
  bytes, reports unreadable files, and lets the user select a group for review.
  It never deletes duplicates automatically.
- File actions rename one original, move/trash selections, and locate one missing
  original with system pickers. Photos and sidecars move together; destination
  file/catalog conflicts are rejected, with rollback on catalog failure. History,
  copies, and album membership follow moves/relinks. Trash retains catalog records
  so restoring files to their original paths restores their associations. Queue
  cancellation stops before the next photo, not during a filesystem operation.
  Relinking retains old sidecars. Bulk relinking remains out of scope for this pass.
  An isolated system-Trash smoke check verified photo/sidecar removal and recovery
  with rating and virtual copy retained. Automated tests cover stack persistence/collapse, duplicates, move collisions,
  rollback, relink, and preservation of edits/copies/albums. Full native menu-flow
  verification and cross-filesystem failure injection remain pending.
- Export jobs persist atomically in `aigle/qt-export-queue.json` under the data
  directory, protected by a single-writer lock. Jobs → Export jobs shows per-file
  status/errors, pause-after-current, resume, retry, retry-to-new-file, cancel-all,
  and clear-finished. Restart always pauses dispatch; formerly running jobs become
  interrupted and need explicit retry. Existing outputs are refused on retry.
  Completed/failed/cancelled records remain visible until cleared or retried.
  Snapshot exports work without an open photo. Source files and referenced assets
  must still exist; their bytes are not embedded in the queue. Progress is per-file
  status, not an encoder percentage. Native capture checked the recovered Jobs
  dialog; tests cover persistence, interruption, pause, cancellation, stale replies,
  retry destination, corruption/write failure, writer locking, and no-photo export.
- Compare selected photos opens 2–4 edited masters with independent engine sessions
  and viewport renders from full-resolution source data. Linked/independent relative
  zoom and normalized pan, Fit all, Actual pixels (1), wheel/drag and keyboard
  navigation, flags, and pane removal are available. Actual pixels accounts for
  device scaling. Viewport requests are coalesced at 80ms; closing releases the
  comparison sessions without replacing the active editor document. Native four-pane
  layout checked; bridge tests exercise region rendering and session cleanup.
  Presentation is SDR sRGB; HDR comparison remains unfinished. The current source
  sessions retain decoded image data, so large-RAW memory/performance needs audit.
- Edit menu supports selective adjustment copy/paste and sync from the open photo
  to selected master targets (excluding the source path). The 17-group chooser
  leaves crop/geometry, masks, heal/clone and red-eye unchecked by default. Selected
  groups replace whole groups; other adjustments and metadata are preserved.
  Clipboard captures the source settings at copy time; sync captures source and
  target list before showing the chooser. Each target gets one persisted history
  step, with per-file errors and duplicate target suppression. Tests cover selective
  preservation, metadata, clipboard-style paste undo, sync/reopen/undo and invalid
  group rejection. Sync is not a global multi-photo undo transaction.
- File → Library → Restore catalog validates schema, required columns, SQLite
  integrity, and foreign-key references before saving current edits and making a
  timestamped recovery backup. Active jobs block restoration. The document and
  comparison sessions close before replacement; a restore failure attempts rollback.
  The result reports the recovery path, and the next restore picker starts there.
  Originals/XMP and the independent export queue are not restored. Tests cover
  invalid/malformed backup rejection, catalog replacement, recovery restore, and
  sidecar preservation. Native restore-dialog interaction remains to be audited.
- Agent transcripts now persist per original path and virtual-copy identity in Qt
  settings. Completed assistant responses and user prompts survive photo changes
  and restarts; follow-up requests include their conversation context. Transient
  progress is excluded and repeated state polls do not duplicate responses. Widget
  regression coverage checks persistence, photo/copy isolation, escaped text and
  follow-up request content. Conversations do not yet follow moved/renamed files;
  Linux/FreeBSD provider keys now use the desktop Secret Service via `oo7`, with
  profile/endpoint scoping, explicit deletion and session/environment alternatives.
  The provider dialog waits for successful storage before accepting preferences
  and displays keyring errors inline. Named provider profiles now support selection,
  create/edit/remove, per-profile selected/discovered models, and isolated session
  keys. Profile removal deletes that profile's stored keys across earlier endpoints
  before removing preferences. Model discovery uses the shared Rust OpenAI-compatible
  discovery service on a background thread, with correlated results, cancellation,
  manual fallback, and a 20-second HTTP timeout. Returned models are not a guarantee
  of vision capability. Live-provider validation and secure storage on other
  platforms remain unfinished.
  Verified 2026-11-07: full Qt GUI build, all five CTest targets (including
  session-only keys, deletion, cancelled preferences, locked-keyring retry and
  stale-response rejection), credential identity unit test, and actual oo7
  save/read/replace/isolate/delete against a private D-Bus/temporary GNOME
  keyring using `apps/aigle/tests/keyring_round_trip.sh`. No live model request
  or other-platform credential backend was exercised.
  Additional verification 2026-11-07: provider restart/selection/session isolation,
  creation cancellation, profile-removal failure/retry, discovered model selection,
  manual fallback and discovery cancellation/stale replies passed in Qt tests.
  All five CTest targets and the GUI build pass. Shared-agent discovery passed
  against a local HTTP server, checking endpoint normalization, authentication,
  sorting and deduplication. Bridge tests check no-photo state/error/cancellation
  and successful editor commands while a discovery response is deliberately held.
  The private Secret Service test additionally verifies same-endpoint provider
  isolation, cleanup across earlier endpoints and earlier Qt credential compatibility.
- Library search matches filename/path, keywords, descriptive metadata, camera,
  lens, capture date, and labels; whitespace-separated terms are combined and
  `-term` excludes matches. The Filter dialog supports ratings, independent flags,
  labels, exact camera/lens/keyword, and capture-date prefixes. Filename, rating,
  capture-date, and modified-date sorts support both directions, with unknown
  metadata last. Metadata loads on the engine worker for unopened photos; results
  update as it arrives. Hidden results are deselected before batch operations.
- Saved searches persist all criteria and sort order in Qt settings, with save,
  open, rename, and delete actions. No legacy import or migration is required.
- Automated query tests cover metadata/excluded terms, hierarchy levels, missing
  metadata, independent rejection flags, unknown-last sorting, saved-search schema
  validation, and settings persistence. Bridge tests cover library metadata reads
  without changing active edits.
- Edit → Preferences provides staged Save/Cancel/defaults for recursive folder
  scanning, thumbnail width, default HDR canvas, and autosave interval. Changes
  apply immediately and persist; range validation protects loading stored settings.
  Named provider/security settings are available in the Agent inspector; localization
  and remaining general preferences are unfinished.
- Main-window shortcut editing is available under Edit → Keyboard shortcuts, with
  persistent portable bindings, multi-key chords, staged Save/Cancel, defaults,
  and duplicate/prefix conflict rejection. Typing and standard editing shortcuts
  remain with text widgets. Comparison-dialog shortcuts remain fixed; commands
  without existing default bindings are not yet included.
- Scopes now expose all seven GPUI modes using the extracted shared engine
  analysis: RGB/luma/channel histograms, cumulative luminance, waveform, parade,
  and vectorscope. Histogram bins are exact at full histogram-render resolution;
  densities/regions/output-boundary and target-gamut percentages use bounded
  samples. Mouse and keyboard measurements use cached distributions. Region
  gestures, keyboard resizing/movement/reset, cancellation and late/photo reply
  rejection are covered by the new Qt scopes target. Scope preferences persist.
  Remaining specialized tools and failure states still require parity review.

See the [workflow inventory](../../08-qt-workflow-parity.md). Qt is the sole release
GUI; GPUI is a development reference, not a fallback to ship. Complete and verify
the required workflows in Qt before release. Existing-user migration is outside
scope because the project has no users yet.

## Reproduce

Use copies of photos and isolated settings/data directories:

```sh
cmake --build target/qt --parallel 4
QT_QPA_PLATFORM=xcb \
XDG_CONFIG_HOME=/tmp/aigle-review/config \
XDG_DATA_HOME=/tmp/aigle-review/data \
target/qt/apps/aigle/aigle-gui /tmp/aigle-review/photos/mountain.jpg \
  --capture-layout /tmp/develop.png --capture-state develop --capture-size 1440x900
```

States: `develop`, `library`, `crop`, `masks`, `agent`, `export`. Output composites
native widgets with the Vulkan framebuffer. The capture requires a working SDR
Vulkan presentation surface; it times out explicitly if capture cannot complete.
Xvfb without DRI3 and the tested Wayland capture path did not produce usable
captures on this machine. No output files are exported by the Export capture.

## Scopes verification — 2026-11-07

- Native XCB captures at device scale 2: [waveform, 1440×900](scope-waveform-1440.png),
  [vectorscope, 1440×900](scope-vectorscope-1440.png), and
  [region panel, 1024×700](scope-region-1024.png). Actual synthetic engine-rendered
  color chart; no fixture measurements or fake histograms are inserted by capture.
  The narrow layout replaces the inspector with the left rail and preserves a
  784-pixel canvas. Native screenshot inspection found no clipped scope controls.
- Full GUI build and all six CTest targets pass. `qt-scopes` checks seven rendered
  modes, cached hover/scaling changes, preference persistence, one-cell ROI,
  whole-crop reset, region keyboard/drag cancellation, stale replies, source reset,
  HDR measurement units and local navigation shortcut priority.
- Shared engine scope tests cover transparent-pixel exclusion, region counts,
  waveform positions, chromatic gamut independence from brightness, exact full
  bins, neutral vectors, HDR PQ/white/peak and analysis-thumbnail semantics.
- Bridge test uses real synthetic SDR then HDR renders and verifies ROI samples,
  original histogram bins, invalid-region/target rejection and unchanged document
  revision. No additional image render is requested by a scope interaction.
- Existing 128×128 HDR analysis benchmark: about 2.403 ms/update on this machine
  in the optimized development test profile (100 updates). This is analysis-only,
  not an end-to-end interaction budget or a claim about other hardware.
- GPU processing hit the existing four-code-value self-test failure and fell
  back to CPU; Vulkan presentation succeeded. Captures are SDR. Physical HDR,
  compositor/display transitions and the broader release gates remain open.

Reproduce with isolated config/data and `--synthetic --capture-layout PATH`:
`--capture-state scope-waveform`, `scope-parade`, `scope-vectorscope` or
`scope-region`, optionally `--capture-size 1024x700`.

## Wayland virtual-compositor capture — 2026-11-08

The previously unsuccessful Wayland capture was rechecked. The existing desktop
session again reported a visible but unexposed canvas and timed out. A private
D-Bus session running `kwin_wayland --virtual --width 1600 --height 1000 --socket
aigle-test --no-lockscreen --no-global-shortcuts --no-kactivities`, with temporary
XDG runtime/config/data directories, successfully exposed the canvas and captured
its Vulkan output. Application used `QT_QPA_PLATFORM=wayland`,
`WAYLAND_DISPLAY=aigle-test`, `--synthetic --capture-state crop-rotated-zoom`.

Inspected [Wayland rotated zoom capture](wayland-crop-rotated-zoom.png): 1440×900
logical, DPR 1, SDR; widgets, crop guide and image coverage render. Engine image
processing used CPU after the known GPU self-test mismatch. Portal activation
reported document mount and host registration warnings; native file dialogs,
physical display/HDR, input workflows and the existing desktop exposure problem
are not verified by this virtual-compositor capture. Private compositor and bus
were stopped and temporary directories removed after the run.
