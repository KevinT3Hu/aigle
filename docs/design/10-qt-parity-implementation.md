# Qt parity implementation — 2026-11-08

User objective: implement all gaps in the source audit and collect decisions or
blockers together after independent work. This supplements, not replaces, the
broader acceptance inventory in 08 and 09. No runtime claim follows from code alone.

## Scope and evidence

- Soft proof / gamut warning: View menu wired to engine overlays; overlay patches
  preserve clipping/mask/proof state and are reapplied when opening a photo.
  Build and interaction verification pending.
- Monitor ICC: background colord/environment discovery, validated ICC import,
  per-screen re-detection, stale-screen rejection, Auto/sRGB preference and fallback
  implemented. Canvas viewport/profile routing regression added. Physical monitor
  output, compositor behavior and multi-screen native verification pending.
- Dedicated DCP/LUT import, profile reset/status, lens matching/disable: implemented
  through engine validation and document history. LUT pin/import/undo/redo and
  invalid-DCP no-mutation regression passed. Real camera DCP/lens native review pending.
- Color-range masks: on-image creation/resampling, tolerance/smoothness, luminance
  range editing and all 15 local parameters implemented. Bridge sampling/undo and
  Qt nested-shape preservation regressions pass. Native gesture review pending.
- Export arbitrary size, saved named presets, last-settings restore, image/text
  watermarks with size/opacity/margin/anchor/color, artist/copyright overrides:
  implemented. Qt preset-to-request and HDR/paired SDR tests pass. Native layout,
  actual rendered watermark exports, unavailable-preset formats and full batch
  naming audit still pending.
- Chinese localization: 747 static Qt messages plus 98 dynamic layout/parameter
  labels translated and compiled into the binary, including bundled Qt standard
  dialog translations. Persisted system/English/Chinese selection is implemented.
  Widget/dynamic-label/source-filename regression passes. Language currently applies
  after restart; live switching, full schema-enum coverage and native text/layout
  review remain pending.
- Shortcut editor: expanded from 16 bindings to main menu, tool, proof-target,
  library, file, and queue actions, including initially unbound commands. Existing
  staged/conflict-aware persistence retained. Generic dialog confirm/cancel and
  comparison fit/actual-pixel bindings are now configurable with context-aware
  conflict detection. Chord/text-input/old-Escape-suppression regression passes.
  Complete GPUI action audit, composer/other tool contexts and native focus
  verification remain pending.
- Agent: Markdown (raw HTML and external resource loading disabled), persistent
  per-message previews and expandable adjustment profiles, run/event deduplication,
  stable catalog photo identity plus copy identity, stale-run/proposal rejection,
  stopped-worker errors implemented. Catalog schema 4 adds identities and preserves
  them transactionally through move/relink. Catalog identity tests, Qt rich-event /
  restart/copy-isolation tests and Rust mocked stale-result test pass. Accepted-edit
  undo regression added. Live provider and native cancellation/navigation audit pending.
- Import settings persistence: source/destination/templates/backup restored;
  saved on acceptance, cancelled choices not saved. Verification pending.
- Additional audit areas: virtual-copy grid identity, mixed metadata presentation,
  parameter/gesture parity, physical HDR/display remain pending.
- Remote source audit: GPUI main.rs:80–108 and Qt bridge remote_open both use
  Remote::checkout_or_open with THESAUROS_TOKEN, then edit/save local sidecars.
  GPUI explicitly requires thesauros-client push for synchronization; neither GUI
  implements an integrated sync/disconnect/conflict UI. Do not label that as a Qt
  regression. End-to-end remote/server verification remains pending.

## Verification log

- Full Qt application and test-target build succeeded in target/qt.
- First ctest run exposed retouch body clicks captured by the radius handle's
  oversized hit region. Radius grip tolerance now matches the 6-pixel rim;
  existing body/rim/source/Alt-placement regression test covers the fix.
- Markdown/import/proof UI changes still require dedicated interaction coverage;
  existing suite alone does not establish complete parity.

- Current Qt build and all six CTest suites pass after the new export/range tests.
- Focused Rust import and color sampling tests pass with
  `AIGLE_DATA_DIR=/tmp/aigle-qt-parity-data` (engine content store is separate from
  bridge catalog data_dir; this environment isolates imported fixture data).

- Agent/display batch: full Qt build and six CTest suites pass, including monitor
  profile routing, rich transcript persistence, duplicate-event suppression and
  virtual-copy isolation. Eight catalog tests pass. All 16 enabled bridge tests
  pass (one local mock-server test required an unsandboxed rerun because socket
  binding was denied); one pre-existing integration test remains ignored.
- Agent regression verifies stale running-result refusal and accepted edit undo;
  no live-provider claim is made.

- Localization/dialog batch: Qt build and all six CTest suites pass. The compiled
  catalog has 845 complete app translations with matching numbered placeholders;
  Qt standard dialog translations are merged into the embedded resource. Tests
  verify Chinese labels, unchanged source filenames, dialog chord activation,
  ordinary text entry, and suppression of the old Escape binding.

## Decisions/blockers to batch

None yet. Physical HDR/display and live service checks require actual facilities;
record what is available before deciding whether user input is needed.

## Export restrictions and watermark verification — 2026-11-08

- Export snapshots identify RAW sources. Mixed/non-RAW batches exclude DNG;
  unavailable preset formats require an explicit alternative instead of silently
  choosing a different codec. Invalid format selections disable submission.
- Batch counts are shown correctly. Disabled DNG watermarks do not trigger text
  validation. A real master/copy PNG export verifies image-watermark pixels.
- Format/preset regression, full Qt build and all six CTest suites pass. Rust
  round-trip test including RAW-eligibility assertions passes. All 904 Chinese
  translations are complete and numbered placeholders match.
- Actual text-watermark output, RAW/DNG fixtures and native/error-path coverage
  remain outstanding.

## Selection-based virtual-copy creation — 2026-11-08

- Library Create virtual copy now operates on selected master/copy identities;
  Develop retains current-photo creation/open behavior. Library creation leaves
  the current document open and selects the resulting rows.
- Bridge validates identities before creation, deduplicates targets, saves active
  edits, preserves each source's metadata/edits and clears inherited snapshots,
  matching GPUI library creation. Names receive unique numeric suffixes per photo.
  Per-target storage failures return both successful copies and explicit errors.
- Extended real Window/bridge regression creates from master and copy together,
  checks distinct names and independently inherited ratings, new-row selection,
  and absence of an open command. Full build and all six CTest suites pass;
  existing Rust round-trip test also passes.
- Unified copy rename/delete and mixed file operations remain unfinished, as do
  live language switching and the broader native/runtime acceptance inventory.

## Selected copy file operations — 2026-11-08

- Selection Rename now renames a virtual copy without opening it. Move resolves
  copy selections to underlying originals and deduplicates paths. Trash queues
  physical originals once, omits their selected copies, and deletes selected
  copies belonging to originals that remain. Confirmation states both counts.
- Copy operations validate path/ID ownership. Deletion works with missing originals
  and releases a deleted active document without autosaving it back. Queued work
  retains cancellation and per-target error handling.
- Window/bridge regression verifies cancelled copy trash, selected-copy rename,
  and deletion of multiple copies including the open copy while preserving the
  original. Rust regression verifies ownership refusal and rename/deletion with a
  missing original. Full build and all six Qt suites pass; focused Rust test passes.
  All 902 active app translations are complete with matching placeholders.
- Mixed physical/copy trash and move still need dedicated end-to-end verification;
  dedicated Photo-menu copy commands still target the current open copy. Broader
  live language switching and native/runtime acceptance work remains outstanding.

## Mixed file-operation end-to-end verification — 2026-11-08

- Added a real Window/bridge fixture with isolated settings/data and actual files.
  Selecting an original plus its open virtual copy sends exactly one physical
  move. Both identities appear at the destination, the copy remains active, and
  its export snapshot retains the unsaved exposure edit after moving.
- Mixed Trash selects that original and copy plus a copy of a second original.
  Confirmation shows one original and one standalone copy. Exactly one physical
  trash and one copy deletion are queued; the second original remains and rescanning
  shows no surviving copy of it. This executes the filesystem operation rather
  than only asserting the request shape.
- Focused workflow test passes. Full Qt build and all six CTest suites pass.
  Native platform/failure-path coverage, live language switching, and the broader
  editing/display/provider acceptance inventory remain open.

## Live translation infrastructure — 2026-11-08

- Translation installation now replaces owned translators, supporting repeated
  English/Chinese transitions without accumulating translators. Added explicit,
  owner-scoped callbacks for application labels; destruction removes their event
  filters. User content is never searched or reverse-translated.
- Main command-registry labels retain literal translation sources and refresh in
  place, preserving QAction identities, connections and shortcut assignments.
- Regression checks repeated language transitions, unchanged modified user text,
  translator cleanup, and destroyed callback owners. Build and six Qt suites pass.
- This is infrastructure progress, not complete live-language parity. Preferences
  deliberately remain restart-only until menus, workspace controls, inspector
  labels, dynamic text and other persistent widgets all have explicit refresh
  coverage and document/selection/unfinished-edit preservation tests.

## Persistent workspace translation bindings — 2026-11-08

- Menu titles, source headings/buttons, static workspace labels, toolbar text,
  search hints, several tooltips and Edit/Agent tabs now refresh explicitly in
  place. Library sort captions update under a signal blocker, preserving index
  and avoiding accidental sort/selection changes.
- Real Window regression switches to Chinese and back while a metadata title is
  modified. It verifies the draft and modified flag, photo identity, selection,
  command label and custom export shortcut, with zero emitted engine requests.
- Full build and all six CTest suites pass. No new translation strings were added.
- Live preference activation remains pending: inspector controls, dynamic labels,
  remaining workspace/tool/accessibility text and persistent dialogs still need
  explicit refresh coverage. Existing restart behavior is retained until covered.

## Metadata and inspector translation bindings — 2026-11-08

- Metadata field labels, accessibility names, loading/mixed hints and tooltips
  refresh from explicit sources without changing text, pending edits or targets.
  Adjustment search hints, tabs and schema section captions refresh in place.
- Translation callbacks now respond only to the application's language event,
  avoiding redundant execution for each receiving child widget.
- Regression verifies a modified mixed-value title survives Chinese/English
  transitions with no writes; adjustment search text and selected tab survive
  with no parameter commands. Full build and six Qt suites pass.
- Remaining live-language work includes individual parameter labels, group
  headers, tool/profile controls, dynamic status/count text, agent/scopes panels
  and persistent dialogs. Preferences remain restart-only pending that coverage.

## Parameter and group translation bindings — 2026-11-08

- Schema-driven numeric, boolean and enum labels now retain translation callbacks.
  Slider accessibility names, reset hints and enum item captions refresh in place;
  enum updates block signals. Group captions and searchable headings refresh
  without changing expansion state.
- Extended gesture regression verifies the same exposure slider and value survive
  a Chinese/English switch, group expansion stays unchanged, and no edit commands
  are emitted. Existing gesture undo checks continue afterward and pass.
- Full build and six CTest suites pass. Remaining live-language coverage includes
  structured-editor launchers, profile/tool controls, dynamic status/count labels,
  agent/scopes panels and persistent dialogs. Preference activation remains open.

## Profile and structured-editor translation bindings — 2026-11-08

- Structured-editor launchers, profile import/reset controls, white-balance mode
  captions, automatic/matching controls and profile status labels refresh in place.
  Status translation reads existing parameters; it never reimports or resets them.
- Regression verifies imported DCP/LUT and resolved lens status through language
  transitions, preserved custom white-balance selection, no emitted edits and safe
  binding cleanup after schema replacement. Focused test and all six Qt suites pass.
- An initial full-suite run stalled and was interrupted. A diagnostic widget-suite
  run passed (39 tests, one native-only skip), followed by a full CTest pass with
  a 60-second timeout. Cause of the transient stall was not established.
- Live preference activation and remaining tool/agent/scopes/dynamic-text coverage
  remain outstanding; this does not claim complete language-switch parity.

## Crop and mask control translation bindings — 2026-11-08

- Added explicit bindings for crop/mask page titles, static buttons/labels,
  tooltips, aspect choices, crop guides, overlay and invert controls. Combo caption
  refreshes block signals and retain selection. Bindings die with rebuilt pages.
- Crop regression switches languages with a staged -7.5-degree straighten angle,
  verifies control identity/angle and combo selections, and checks that no edits
  or angle changes are emitted. Build and all six CTest suites pass.
- Tool-panel numeric row labels, orientation buttons and some inline/help labels
  still need bindings, alongside retouch/agent/scopes and dynamic workspace text.
  Live preference activation remains pending; full parity is not yet achieved.

## Tool numeric labels and orientation translation — 2026-11-08

- Numeric tool rows retain literal source labels and refresh visible captions and
  both spinbox/slider accessibility names. This covers crop angle, mask coverage
  controls and all local-adjustment rows. Orientation and duplicate/delete buttons
  now have explicit bindings too.
- Crop regression checks translated angle accessibility, staged-value preservation
  and English restoration after rebuilding tool pages. Build and six Qt suites pass.
- Remaining language work includes tool help/inline labels and unit captions,
  retouch, agent/scopes, dynamic workspace text and persistent dialogs. The language
  preference still applies after restart until that coverage is complete.

## Retouch control translation — 2026-11-08

- Retouch heading/actions, creation and selected-spot mode captions, numeric
  labels and accessibility names now refresh explicitly. Combo text updates
  block signals, preserving correction mode and creation defaults.
- Radius gesture regression switches languages during preview, checks unchanged
  radius/slider-down state and command count, then verifies the same session commits
  on release. Existing cancellation coverage continues to pass.
- Build and all six CTest suites pass. Retouch dynamic correction-list/selection
  text and help/status still need refresh bindings, alongside the remaining
  agent/scopes/workspace language coverage. Live preference activation is pending.

## Retouch dynamic text translation — 2026-11-08

- Correction captions update existing list items under signal blocking. Selection
  labels retain the displayed correction value, including previews, without calling
  control synchronization. Help and preview hints refresh; error text is preserved.
- Extended preview regression verifies selected list-item identity and stored key
  survive translation, in addition to staged radius and same-session commit checks.
  Build and all six Qt suites pass.
- Remaining language coverage includes agent/scopes, dynamic workspace labels,
  some tool inline/help/unit captions and persistent dialogs. Preferences remain
  restart-only until live switching is complete and verified.

## Agent control translation — 2026-11-08

- Composer/proposal controls, hints, accessibility and photo context refresh in
  place without touching prompt or transcript content. Moving a photo with stable
  conversation identity now updates the retained path before the early return,
  preventing later context refresh from reverting to the old filename.
- Regression verifies the moved filename, preserved unfinished prompt and identical
  transcript HTML through language switching. Build and six Qt suites pass.
- Transcript presentation labels, usage/status and provider dialogs still need
  live-language coverage, alongside scopes and remaining workspace/tool captions.
  Preferences remain restart-only; the broader parity objective stays open.

## Transcript presentation translation — 2026-11-08

- Existing conversation presentation now refreshes author and profile-link labels
  from stored message data. Expanded profiles remain expanded, image resources
  are restored, and scroll offsets are reapplied. It does not persist messages.
- Rich-event regression verifies translated links, unchanged message count/text,
  expanded parameter data, preview resource and byte-identical saved conversation
  settings. Build and all six CTest suites pass.
- Agent usage/status, provider dialogs, scopes and remaining workspace/tool text
  still need live translation coverage. Live preference activation remains pending.

## Agent summary translation — 2026-11-08

- Proposal summaries and usage statistics retain their data and render independently
  of agent event ingestion. Language updates cannot replay events or change proposal
  availability. Previously hard-coded exposure/highlights/shadows/temperature labels
  now use existing translated labels. Conversation-save failures retain their status.
- Regression verifies translated summary/usage, unchanged numeric values and zero
  emitted agent commands. Build and all six Qt suites pass.
- Provider dialogs, scopes and remaining workspace/tool text still require language
  coverage. Live preference activation and the broader parity audit remain open.

## Scope control translation — 2026-11-08

- Scope modes, mode tooltips, channel accessibility, gamut label, log/region and
  whole-crop captions refresh without replacing controls. Graph repaint updates
  painted labels; combo refresh blocks signals.
- Regression preserves vectorscope mode, Rec.2020 target and logarithmic state,
  with no mode-change signal or emitted analysis command during translation.
  Build and all six Qt suites pass.
- Scope help/statistics/cached readouts and remaining provider/workspace/tool text
  still need bindings before activating live language preferences.

## Scope measurement text translation — 2026-11-08

- Statistics retain explicit waiting/updating/error/ready state and cached analysis
  data, allowing text-only translation without replaying replies or requesting work.
  Help text and active graph measurement readouts refresh from their existing state.
- Regression supplies known analysis percentages and verifies their translated
  presentation preserves values with no additional analysis command. Build and all
  six CTest suites pass.
- Remaining language audit includes graph/thumbnail auxiliary tooltips, provider
  dialogs and workspace/tool text. Preferences still require restart pending full
  coverage; broader editing/display/service parity verification remains open.

## Auxiliary scope and tool text — 2026-11-08

- Scope-region accessibility/instructions, graph help tooltip, crop help,
  aspect/guide headings and mask-creation buttons now refresh explicitly.
- Full build and six Qt suites pass. These changes only update application text;
  existing crop, mask and scope state-preservation regressions remain green.
- Remaining live-language work includes provider dialogs, dynamic workspace text,
  additional construction-only controls and final coverage audit. The preference
  still requires restart, and the full parity goal remains open.

## Dynamic library labels — 2026-11-08

- Selection/grid/filmstrip counts, album/folder headings and breadcrumb captions
  have a separate text-only refresh path, preserving literal filenames and album
  names. Language changes do not call selection metadata synchronization.
- Window translation regression verifies translated selection counts alongside
  preserved modified metadata, selected identities and zero engine requests.
  Build and all six Qt suites pass.
- Status/history and remaining construction-only labels/provider dialogs still
  need the final language coverage audit before enabling the live preference.

## Reference and metadata workspace captions — 2026-11-08

- Reference tab captions, snapshot/preset/import/metadata buttons, canvas tool
  captions and metadata form headings now refresh explicitly. Snapshot/preset
  names and metadata field contents remain user data.
- Window regression checks translated reference captions with the same selected
  tab, alongside existing draft/selection/shortcut preservation assertions.
  Build and all six Qt suites pass.
- Live preference activation remains pending the remaining status/history,
  accessibility/control and dialog coverage audit. Full parity is still open.

## Remaining command-label translation — 2026-11-08

- Culling/navigation/inspector actions retain callable translation sources,
  including rating interpolation and color names. Tool-local, dialog and comparison
  command labels also refresh while retaining QAction identity and key bindings.
- Build and all six Qt suites pass, including existing shortcut restoration and
  language-state-preservation coverage. Remaining status/dialog coverage and live
  preference activation are still outstanding; full parity remains unproven.

## Provider dialog static translation — 2026-11-08

- Provider dialog title, form labels, profile/discovery controls and credential
  hints refresh without replacing inputs or changing staged provider/key values.
- Modal regression verifies draft name, test-only key and password masking survive
  translation, then cancels without saving. Build and six Qt suites pass.
- Asynchronous provider status text, remaining workspace status/accessibility and
  dialog coverage still require auditing before live preference activation.

## Provider status translation — 2026-11-08

- Discovery/keyring progress, validation, errors and model-count messages retain
  callable text sources with copied result arguments. Translation refreshes only
  the label, without replaying requests or changing pending-operation state.
- Modal regression checks an existing endpoint-validation message translates while
  staged name/key fields remain intact. Build and all six Qt suites pass.
- Remaining workspace status/history and dialog coverage still require the final
  live-language audit; the preference remains restart-only for now.

## Document status and catalog audit — 2026-11-08

- Loading/save/retouch-preview/export status and the history Original label refresh
  from retained state without re-entering updateState or triggering autosave.
  Literal engine errors and temporary status-bar messages are preserved.
- Regenerated translation catalog: 902 active messages, no new or unfinished
  translations. Full build and all six Qt suites pass.
- Temporary status messages, remaining construction-only accessibility text and
  persistent dialog coverage still need final audit before live preference wiring.
  The broader parity goal remains active and incomplete.

## Translation refresh lifecycle — 2026-11-08

- Translator replacement now suppresses intermediate application-binding refreshes
  and sends one final language event. Reapplying the effective language is a no-op,
  avoiding redundant transcript rebuilding and label updates.
- Regression verifies one callback refresh per change, none for unchanged language,
  translator cleanup and destroyed binding owners. Full build and six Qt suites pass.
- This addresses refresh behavior, not remaining coverage. Live preference wiring,
  temporary status/dialog auditing and broader parity verification remain open.

## Original view and composer isolation — 2026-11-08

- Added an original-only View command alongside split before/after. Both commands
  are configurable; default keys follow GPUI (`\` original, `Y` split). Menu
  toggles are mutually exclusive. Original uses the existing original-frame
  texture and its viewport mapping in both SDR and HDR rendering paths.
- The entire agent composer subtree now consumes ShortcutOverride events, keeping
  application shortcuts from firing while its controls have focus. Normal key
  events still reach controls, including text editing and Shift+Enter.
- Qt application build and all six CTest suites pass. Added composer-subtree
  shortcut-override/text-entry regression. Original-only rendering still needs
  native visual verification; these checks do not prove physical display output.
- Next: copy-aware library rows and downstream metadata/export/sync/file actions;
  mixed metadata inspector, live language switching and remaining action coverage
  are still open. No new user decisions or blockers.

## Virtual-copy bridge identity — 2026-11-08

- Added explicit `{path, copy}` targets for batch metadata, selective adjustment
  sync, and comparison sessions. Legacy `paths` callers still address masters.
  Deduplication uses the full identity, and metadata reads/writes validate copy
  ownership instead of silently falling back to the master.
- Added `library_copies` for listing copies of a source without opening a photo;
  `library_metadata` now reads copy-specific fields and reports copy identity.
- Sync stores copy-specific metadata/history, supports reopening/undo, and keeps
  the master sidecar untouched. Comparison can load master and copy together.
- Expanded the real-image round-trip regression: copy-only metadata, duplicate
  targets, invalid/mismatched copy identity, source copy listing, sync/reopen/undo,
  and master/copy comparison. It passes. Bridge suite: 15 passed, one existing
  Secret Service test ignored, local-server model discovery excluded in this run.
- Qt row insertion is not enabled yet: selection, refresh, comparison controls,
  stacks, destructive file actions, and export UI still need coordinated wiring.
  Existing copy dialogs continue to work. This is backend progress, not completed
  virtual-copy GUI parity.

- Rebuilt the Qt application against the updated bridge; all six CTest suites pass.

## Virtual-copy Qt integration — 2026-11-08

- Grid/filmstrip now list copies independently with names, copy badges, distinct
  selection/open identity, metadata refresh, and create/rename/delete row updates.
- Batch metadata, stacks and selective sync carry full identities. Comparison
  accepts master/copy pairs, shows copy names, updates flags by identity, and removes
  only the chosen card. Duplicate scanning/review selects physical originals only.
- Physical file actions reject copy selections and explain the existing dedicated
  virtual-copy rename/delete commands. This prevents accidental master operations;
  unified selection-based copy rename/delete and mixed master/copy actions still
  need parity work. Current-photo export already snapshots the open copy's params;
  selection-based batch export remains outstanding (the current Export action
  exports only the open photo, plus its optional SDR companion).
- Added a real Qt Window/bridge regression that opens a PNG, creates a copy row,
  changes only its rating, renames it, verifies the file-action guard, deletes it,
  and checks the original remains. Extended the widget test target to include
  Window/comparison/scopes so subsequent workflows can be tested end to end.
- Qt build and all six CTest suites pass, including the new window regression.
  All new app strings have Chinese translations. Native visual and performance
  verification remain outstanding. Mixed metadata, live language switching,
  remaining shortcuts, export selection and physical/live-service checks remain.

## Mixed selection metadata — 2026-11-08

- Replaced the library's single-photo keyword input with a selection-aware
  inspector for shared keywords and all 16 descriptive IPTC fields. Differing
  values show Mixed values; edits write only the changed field to captured
  master/copy identities. Unloaded metadata disables editing until available.
- Keyword edits compute additions/removals from the shared set. New bridge patch
  mode applies both in one read/write, preserving each selected photo's private
  keywords and hierarchical keyword normalization.
- Selection changes commit modified fields to the previous identities. Polling
  preserves unfinished typing and pending values until refreshed metadata arrives.
  Per-target write failures clear pending display state and use existing error UI.
- Real Window/bridge regression verifies master/copy mixed titles, shared keyword
  edits, preserved private tags and distinct captions. A widget regression verifies
  that a selection change commits unfinished text to the previous copy, not the
  newly selected photo. Qt build, all six CTest suites, and the bridge real-image
  round-trip test pass. All 871 app translations are complete with matching
  numbered placeholders. Native layout/focus review remains outstanding.
- Remaining work includes unified copy file actions, selection-based batch export,
  live language switching, remaining command bindings and the broader runtime /
  display / service acceptance checks. No new user decisions or blockers.

## Culling/navigation shortcuts — 2026-11-08

- Added configurable rating 0–5, pick/reject/clear flag, all five color labels /
  clear label, select visible photos, open selected, four-direction photo/grid
  navigation, monochrome, fit/actual-pixel toggle, masks and information commands.
  Culling and navigation retain full master/copy identities and skip hidden rows.
- Added defaults for existing crop/WB/retouch/proof/history/file commands. Actual
  pixels defaults to Ctrl+1 to leave numeric keys for ratings; Z toggles fit/1:1.
- Keymap restoration prioritizes saved bindings over new defaults. Conflicts unbind
  only the affected later command, retain unrelated saved chords, and show a status
  notice instead of reverting the entire keymap. The editor can resolve conflicts.
- Text entry and native combo/list/slider navigation retain priority. Select-all
  batches selection updates instead of recomputing the inspector per selected row.
- Extended Window regression: saved actual-pixel binding survives a new numeric
  rating conflict, unrelated export chord survives, culling preserves independent
  rating/flag/label on a copy, and navigation opens the correct copy identity.
  Text and combo shortcut-override checks pass. Qt build and all six CTest suites
  pass; all 893 app translations are complete with matching placeholders.
- Still open: configurable tool-local crop confirm/cancel, aspect swap, overlay and
  retouch deletion; live language switching; unified copy file actions; selection
  batch export; remaining native/runtime/display/provider acceptance checks.

## Tool-local shortcuts — 2026-11-08

- Added configurable canvas cancellation, crop apply/aspect-swap, retouch delete
  and alternate-delete bindings, plus a mask-overlay action. QWindow tool bindings
  support up to four-stroke chords with prefix timeout, focus/tool-change reset,
  and autorepeat suppression. Old hard-coded bindings no longer fire after remap.
- Retouch slider gestures use the same cancellation binding; the correction list
  supports configured deletion. Text-entry focus retains editing priority.
- Context overlap now distinguishes dialogs/comparison from canvas/crop/retouch,
  so valid default bindings are not false conflicts. Local bindings consult the
  effective keymap (including disabled conflicts), and apply saved changes without
  rebuilding the canvas. Updated the retouch hint to avoid advertising stale keys.
- Added canvas/panel regression for remapped chords, old-key suppression, disabled
  alternate deletion, and prefix reset across tool changes. Existing drag and
  cancellation regressions still pass. Qt build and all six CTest suites pass;
  all 899 app translations are complete with matching numbered placeholders.
- Full native focus verification and crop ratio/guide presentation audit remain.
  Broader goal still includes live language switching, unified copy file actions,
  selection-based batch export and remaining runtime/display/provider checks.

## Selection-based batch export — 2026-11-08

- Export now resolves the selected master/copy identities through `export_inputs`
  before showing the dialog. Develop with a single selection uses the open photo,
  including generated sources. Current unsaved edits are captured without forcing
  a save; other targets use their own saved edits. Invalid/missing targets fail
  preparation rather than silently exporting a master or omitting a target.
- The dialog produces immutable per-photo queue requests and optional SDR companion
  jobs. Naming supports `{name}` and zero-padded `{seq}`, copy display names,
  separator sanitization, empty-name fallback, and numeric suffixes for duplicate
  destinations within a batch. Existing destination files are still refused.
  Preview lists output names; batch size text explains per-photo sizing. Jobs show
  the source/copy display name. Duplicate concurrent preparation is prevented.
- Window/bridge regression exports a selected master and copy to actual PNG files
  through the dialog and durable queue and verifies different rendered pixels.
  Dialog regression covers 12-file sequencing, duplicate filenames, sanitized
  templates and per-input parameter snapshots. Rust round-trip regression covers
  unsaved copy params, duplicate target identities, ownership refusal and snapshot
  immutability after undo. These tests pass; full Qt build and six CTest suites pass.
  All 902 app translations are complete with matching placeholders.
- Remaining export audit: all format/source restrictions (including DNG), unavailable
  saved-preset formats, real watermark outputs and native/error-path coverage.
  Live language switching, unified copy file actions and the broader acceptance
  checks remain open. No new user decisions or blockers.

## Live language preference activation — 2026-11-08

- Saving Preferences now applies the selected language immediately after closing
  the dialog. Cancel preserves both the persisted choice and effective language.
  Completed the new Language label's Chinese translation. This supersedes earlier
  restart-only entries; it does not imply every persistent label has live coverage.
- Added a real Window modal regression covering English/Chinese Save and Cancel,
  reopened preference selection, persistence and existing QAction label updates.
  The initial test timed out because its fixture lacked an organization name and
  QSettings rejected saving; an isolated organization fixes the fixture. Rebuilt
  successfully and all six CTest suites passed (6.18 seconds).
- Remaining concrete source gaps: curve continuous photo preview and histogram
  background; HDR comparison; 4:5 crop preset; direct language/display/keymap reload
  commands; remaining live translation coverage. Broader native, physical display,
  provider, export and parameter acceptance checks remain open.

## Continuous curve edits — 2026-11-08

- Point-curve dragging now emits continuous engine edits while moving and a final
  edit on release with the same gesture ID, using the existing document gesture
  protocol. Channel switching is disabled during the drag so edits retain their
  target channel. Structured modal curve editing retains staged acceptance.
- Added widget regression for preview-before-release, changed curve values,
  same gesture ID through final commit, and a distinct ID for the next drag.
  Application build and all six CTest suites pass (6.18 seconds).
- This verifies command routing, not native rendered preview latency or actual
  curve undo round trips. Histogram background, native curve interactions and
  remaining parity/acceptance work remain open.

## Curve histogram background — 2026-11-08

- Routed existing rendered-image histogram updates into the adjustment inspector.
  RGB curves use luminance bins; individual channels use their respective bins.
  The background matches GPUI's square-root scaling and interior-bin peak
  normalization, excluding clipped endpoints from normalization. New documents
  and closed documents clear cached histogram data; schema rebuilds retain it.
- Added rendered-widget regression for visible histogram pixels, clipped endpoint
  normalization, channel selection, clearing, and unchanged curve values/no edits.
  Build and all six CTest suites pass (6.02 seconds). Native layout review and
  broader curve interaction/undo verification remain outstanding.

## Portrait crop preset — 2026-11-08

- Added GPUI's missing 4:5 crop preset to the Qt aspect selector, its lock/relock
  mapping, and live translation list. All 903 active app translations are complete.
- Regression verifies selection emits 0.8, unlocking emits free aspect, relocking
  restores 0.8, and language switching preserves the selection without crop edits.
  Build and all six CTest suites pass (6.04 seconds).
- This closes the preset omission; native crop geometry/gesture verification and
  the broader editing/display/service acceptance inventory remain open.

## Direct settings and reload commands — 2026-11-08

- Added configurable English/Chinese language commands, automatic/sRGB display
  profile commands, and Reload keyboard shortcuts. Language/display choices persist
  before applying. All five commands have translated captions and start unbound.
- Extracted existing shortcut loading/conflict resolution into reloadShortcuts;
  reload synchronizes settings and updates effective local/dialog bindings without
  recreating actions or installing additional event filters.
- Extended real Window regression verifies language effects and persistence,
  display-choice persistence, externally changed shortcut loading, effective-keymap
  publication, and stable action counts across repeated reloads. Build and all six
  CTest suites pass (6.13 seconds). All 908 app translations are complete.
- Remaining work includes HDR comparison, final live-label coverage, curve native
  and undo verification, broader parameter/workflow audit and physical/live-service
  acceptance checks. No new user decisions are required for this batch.

## Curve gesture document verification — 2026-11-08

- Added a real bridge/document regression that sends multiple continuous RGB and
  red-channel edits followed by final commits. Intermediate document parameters
  change before release; each gesture undoes in one step, the channels remain
  independent, two undos restore all original parameters, and two redos restore
  the completed edits.
- Focused Rust test passes. Initial decimal-value equality exposed expected f32
  serialization rounding in the fixture; binary-exact test values now permit
  exact whole-parameter assertions. No engine fix was necessary.
- Together with the Qt drag-command test this verifies the gesture protocol and
  document undo semantics. Native visible preview latency remains unverified;
  HDR comparison and broader parity/acceptance work remain open.

## HDR comparison frame transport — 2026-11-08

- Confirmed comparison_viewport already accepts HDR output profiles. The Qt
  adapter previously forwarded only SDR QImages, dropping half-float HDR frames.
  Added comparisonFrameReady carrying shared immutable Frame ownership, session
  identity and candidate index for the forthcoming native comparison renderer.
  Existing SDR presentation remains operational; this is not HDR UI completion.
- Extended real-image bridge regression requests an HDR comparison viewport,
  verifies half-float format/stride/extent, closes the session, then reads and frees
  the retained frame. Current editor parameters remain unchanged. Test passes.
- Application build and all six CTest suites pass (6.59 seconds). Next required
  work is connecting HDR-capable comparison surfaces, linked navigation, output
  negotiation/fallback and screen changes; physical HDR verification remains open.

## Shared canvas navigation for HDR comparison — 2026-11-08

- Added normalized center/relative-fit zoom transfer to the native canvas viewport,
  with bounds checking, fit restoration and existing maximum-scale enforcement.
  Canvas can now apply transferred navigation and request matching rendered pixels.
- Geometry regression verifies linked magnification/center across different image
  and pane sizes, fit across different aspect ratios and boundary clamping.
  Build and all six CTest suites pass (6.16 seconds).
- This prepares reuse of the HDR canvas in comparison; the comparison dialog still
  uses its SDR QWidget renderer. Surface integration, navigation loop prevention,
  display fallback/status and physical output validation remain required.

## Native comparison canvas integration — 2026-11-08

- Both comparison entry points now pass the main canvas context. With Vulkan,
  each candidate owns a native HDR-capable Canvas and receives retained frames.
  Added an HDR toggle and per-candidate output status; SDR-only widget fallback
  remains for contexts without a Vulkan instance. Initial display preference is
  inherited from the main canvas. Native viewport requests replace SDR polling.
- Linked navigation uses normalized canvas navigation with a recursion guard;
  fit and actual-pixel actions address native surfaces. Existing identity-aware
  flags/removal remain. Frame/error callbacks are disconnected before teardown.
- Build and all six CTest suites pass (6.08 seconds); 910 app translations are
  complete. These offscreen tests do not exercise native Vulkan comparison.
- Required follow-up: native comparison startup/input/close checks, monitor-profile
  updates when moving the dialog, surface failure fallback, hidden-candidate/link
  edge cases, and physical HDR/SDR output verification. Full parity remains open.

## Native comparison startup and SDR fallback — 2026-11-08

- Launched the actual XCB/Vulkan application with isolated config/data and two
  temporary photo copies, requesting HDR. Both comparison canvases initialized,
  received frames, reported HDR unavailable → SDR, and closed normally (exit 0).
- QWidget layout capture omits native child surfaces. Extended the development
  comparison capture to save separate GPU readbacks; visually inspected portrait
  and landscape outputs, each correctly fitted without stretching. Evidence:
  qt-review/native/comparison-native-{layout,landscape,portrait}.png.
- Engine GPU image-processing self-test fell back to CPU after a four-code-value
  mismatch; presentation still used native Vulkan canvases. This does not establish
  physical HDR output or GPU processing correctness. Desktop screenshot tools were
  unavailable for this compositor; GPU readbacks provide the rendering evidence.
- Build and six CTest suites pass (6.76 seconds). Native input, linked-navigation
  edge cases, screen/profile transitions and surface-failure recovery remain open.

## Native comparison keyboard and removed candidates — 2026-11-08

- Comparison canvases now support plus/minus zoom and arrow pan. Configurable
  comparison fit/actual-pixel bindings invoke dialog-wide actions, and configured
  dialog cancellation closes comparison rather than invoking crop/tool cancel.
- Removed cards no longer submit viewports or drive linked navigation. Re-enabling
  links takes navigation from the first visible candidate instead of a removed one.
- Added QWindow event regression covering zoom/pan, remapped fit/pixels/close
  commands and suppression of their old bindings. Build and all six CTest suites
  pass (6.36 seconds). Native focus delivery and removal interactions still need
  end-to-end verification; monitor transitions/failure recovery remain open.

## Comparison monitor-profile routing — 2026-11-08

- Comparison now requests independent asynchronous monitor discovery on session
  startup and native screen changes. It uses sRGB while discovery runs, honors
  the persisted automatic/sRGB preference, and only applies profile replies whose
  screen matches each canvas. Main-editor discovery state is not overwritten.
- Added comparison display polling/transport scoped to the active engine session;
  closing comparison clears its detection state. Removed unused inherited-profile
  accessors from Canvas.
- Rust regression verifies separate main/comparison results and close cleanup.
  Application build and all six Qt suites pass (7.20 seconds). Physical monitor
  movement, calibrated output, surface-failure recovery and native input workflows
  still require verification; the complete parity goal remains open.

## Renderer recovery scheduling — 2026-11-08

- Fixed a recovery gap shared by main and comparison canvases: device-loss status
  promised retry but did not schedule one. Initialization, swapchain creation/
  resize, frame-begin and frame-end failures now schedule exposed-window retries
  with 250 ms exponential backoff capped at four seconds. Successful presentation
  cancels pending retry and resets the delay; hidden windows do not poll.
- Qt application build succeeded. Native XCB synthetic edit/undo/redo/render smoke
  passed with SDR presentation. The existing engine GPU self-test mismatch still
  selected CPU image processing. Device-loss fault injection is not yet verified;
  this does not establish recovery on a real failing driver.

## RAW decoding section reset parity — 2026-11-08

- Source audit found Qt RawSettings created controls but omitted reset paths.
  Added the four paths GPUI Extra::RawSettings resets: demosaic, highlight_recon,
  ca_correction and hot_pixels. They now also participate in modified-state
  tracking for the section reset control.
- Widget regression clicks the real Decoding section reset and verifies a single
  command containing exactly those four reset operations, with no unrelated paths.
  Build and all six CTest suites pass (6.43 seconds). Full parameter audit and
  real RAW processing/undo verification remain open.

## Source-specific RAW controls — 2026-11-08

- Matched GPUI source gating: camera DCP controls appear for RAW photos; decoding
  controls are replaced by an explanatory note for rendered/synthetic sources.
  Look LUT controls remain available. Source changes and schema reconstruction
  update visibility without issuing edits or discarding stored parameters.
- Widget regression covers Raw → Rendered → Synthetic → Raw, retained LUT access
  and zero edit commands. Build and all six Qt suites pass (6.23 seconds).
  Translation catalog contains 912 complete app strings. Broader parameter and
  native workflow verification remains open.

## Scalar inventory and accessible labels — 2026-11-08

- Compared GPUI TOOL_GROUPS scalar rows with Qt develop-layout.json: both contain
  the same 92 parameter paths. GPUI obtains min/max/step from EditParams::spec;
  Qt obtains these through EditParams::json_schema. Both special-case logarithmic
  white-balance temperature. This establishes source coverage, not every gesture.
- Numeric inputs now expose translated human-readable accessible names, retaining
  internal paths in parameterPath. Numeric and enum labels have explicit buddies;
  enums expose translated accessible names and stable path object names.
- Extended the existing live exposure-gesture regression to verify numeric and
  slider accessible names stay synchronized through translation. Build succeeds.
  Native assistive-technology review and broader acceptance checks remain open.

## Curve point deletion interactions — 2026-11-08

- Added GPUI's double-click removal of interior curve points. Endpoints remain
  protected. Right-clicking empty space or an endpoint no longer emits a no-op
  edit; clicks outside the graph no longer insert a clamped point.
- Regression covers interior deletion, endpoint protection, empty-space and
  outside-graph clicks. Application build succeeds. Drag-out deletion and
  on-curve insertion/grab-offset parity still need implementation; these remain
  explicit curve gaps alongside the broader acceptance inventory.

## Curve drag-out deletion — 2026-11-08

- Interior points dragged more than 24 logical pixels outside the graph are
  omitted from both rendered curve and continuous edits, matching GPUI. Returning
  inside restores the point before release; release outside commits its removal
  using the existing drag gesture. Endpoints cannot be removed this way.
- Widget regression verifies preview deletion, return restoration, final removal
  and endpoint protection. Build succeeds. On-curve insertion and grab offsets,
  native gesture review and remaining parity acceptance work are still open.

## Curve insertion and grab offsets — 2026-11-08

- New points now use the existing displayed spline's value at the clicked x,
  matching GPUI's on-curve insertion. The curve renderer and insertion share one
  evaluator. Pointer movement preserves the initial grab offset, preventing a
  point from jumping to the cursor when grabbed off-center or newly inserted.
- Regression checks off-curve insertion on identity and relative drag movement.
  Application build succeeds. Full spline/kernel numerical equivalence, native
  interactions and the remaining parity acceptance inventory remain open.
- Updated the earlier drag test to grab the resulting point's actual location
  instead of assuming cursor snapping. Full six-suite CTest rerun passes.

## Shared curve interpolation reference checks — 2026-11-08

- Added a common numerical fixture consumed by Rust Spline tests and Qt's actual
  click-to-insert test. It covers unequal slopes, a turning point, and a plateau,
  checking six interior samples to 1e-9. Both implementations match these expected
  values; source review also confirms the same tangent-limiting/Hermite formulas.
- Focused aigle-core test and Qt build pass. This bounds numerical evidence to
  the sampled cases, not all floating-point inputs. Remaining native/workflow,
  display/service and release acceptance work stays open.

## Qt text-watermark export verification — 2026-11-08

- Extended the real Window master/copy export workflow to submit text watermarks
  through ExportDialog and the durable queue. Checks both output files, serialized
  text content and visible glyph pixels on the uniform master image. Added a stable
  object name to the text input for interaction testing.
- Initial run reached a later file-action test before the second export completed;
  the fixture now waits for both outputs before continuing. Build and all six Qt
  suites pass (6.68 seconds). RAW/DNG fixtures, broader codec/error paths and native
  export layout checks remain open; this closes the PNG text-rendering evidence gap.

## Curve help and channel localization — 2026-11-08

- Updated curve instructions to describe on-curve insertion, double-click/right-click
  deletion and drag-out deletion. Help, accessible name/description and channel
  names now refresh live; stable channel data remains unchanged.
- Regression switches language with Green selected and verifies unchanged curve
  points/channel identity, translated help and zero edit commands. Build succeeds;
  917 active app translations are complete. Other persistent-dialog labels and
  native accessibility verification remain open.

## Restore saved crop aspect — 2026-11-08

- Crop canvas and inspector now initialize from geometry.aspect rather than always
  Original. The selector uses item ratio data, restores preset/custom ratios and
  Free/Original, and synchronizes the lock checkbox when selecting a preset.
- Custom 7:5 regression verifies restored display/value, no edits on opening and
  correct selection signal. Build and all six Qt suites pass (7.03 seconds).
- Saving newly selected aspect with Apply (and preserving Cancel semantics) still
  needs wiring; this change restores constraints already stored in the document.
  Broader crop/native and acceptance work remains open.

## Crop aspect commit and persistence — 2026-11-08

- Crop aspect changes now remain staged while editing and are serialized only by
  Apply. Original, Free and preset/custom ratios are encoded in the tool command,
  validated in the Qt bridge, and written into geometry.aspect in the same undo
  operation as the crop. Cancel leaves the document and history unchanged.
- Qt regression covers staging, cancel, and Apply with a 16:9 ratio; the bridge
  history test verifies the persisted 4:5 ratio. Build, focused Rust coverage and
  all six Qt CTest suites pass. Native crop workflow and broader acceptance work
  remain open.

## Custom crop sidecar and history round trip — 2026-11-08

- Extended the real-file bridge regression to apply a custom 7:5 crop, save,
  close and reopen the photo, then undo and redo the complete crop operation.
  Assertions compare the complete parameter snapshots, including aspect/bounds.
- The first run attempted cropping before asynchronous image loading completed;
  the fixture now waits for the decoded frame. The focused Rust integration test
  passes. This verifies bridge/sidecar/history persistence; native pointer and
  inspector workflow acceptance remains separate.

## Export dialog live translation — 2026-11-08

- Export field labels, buttons, headings, built-in presets, metadata/sharpening/
  watermark/size choices, placeholders, summary captions and computed preview
  now refresh through explicit translation bindings. Combo relabeling blocks
  signals and leaves saved preset names and entered text untouched.
- Added an English → Chinese → English regression checking the title, Cancel
  label, unchanged serialized export requests and entered watermark text. Build
  and all six Qt suites pass (7.15 seconds). Existing displayed validation errors,
  other persistent dialogs and native translated layout acceptance still need
  coverage; this does not close the broader localization acceptance item.

## Export validation translation and recovery — 2026-11-08

- Validation errors retain explicit translation source and argument, allowing
  already displayed messages (including unavailable formats and filename
  conflicts) to switch language without translating user data. Destination
  browse accessibility text refreshes as well.
- Moved preview translation binding to construction rather than settings restore,
  avoiding duplicate bindings on every preset restoration.
- Added regression for empty watermark validation, English/Chinese/English error
  refresh, unchanged requests, and successful dialog acceptance after correcting
  the input. Build and all six Qt suites pass (7.09 seconds). Other persistent
  dialogs and native localization/accessibility review remain open.

## Preferences live translation and staged values — 2026-11-08

- Preferences now refreshes its title, form labels, combo labels, checkbox text,
  accessibility names, tooltips, units, monitor fallback and explanation when
  the application language changes. Relabeling blocks combo signals and does not
  apply or reset staged settings.
- Extended the real Window preference test to switch English/Chinese/English
  while the modal dialog is open, preserving staged language, thumbnail width
  and autosave interval without persisting them. Existing Save/Cancel checks
  remain. Build and all six Qt suites pass (7.24 seconds). Native accessibility,
  translated layout and the remaining acceptance inventory are still open.

## Shortcut editor live translation — 2026-11-08

- Shortcut dialog title, headers, help, command rows, accessible names and
  conflict messages refresh live. QAction label changes refresh rows after
  action translation bindings run, avoiding stale labels from event-filter order.
  Staged key sequences remain untouched.
- Real Window regression creates a conflict, switches language, verifies staged
  sequences and translated accessible names, resolves the conflict, and cancels
  without altering active bindings. Build and all six Qt suites pass (7.46
  seconds). Native focus/accessibility and broader acceptance remain open.

## Import dialog translation and sync target review — 2026-11-08

- Import options refresh title, labels, accessible names, tooltip and placeholder
  live while preserving entered folder/name templates and backup paths.
- Real Window test drives source/destination pickers and options twice: Cancel
  preserves previous settings and emits no import; acceptance saves staged
  values and emits exactly one matching request. Both exercise language changes.
  Build and all six suites pass (7.80 seconds). This tests request submission,
  not physical import/copy or cancellation of a running import.
- Source audit also found sync target review converting identity objects to
  strings, producing blank entries. It now snapshots the selected row names
  and source paths alongside target identities, displaying those descriptions.
  The subsequent build passes; native sync review is still pending.

## Copy/sync dialog translation and target review regression — 2026-11-08

- Copy/sync dialog headings, explanations, adjustment-group labels, warning note
  and review button now refresh live without modifying checkbox state or target
  identities. Source-copy explanation uses the captured source identity.
- Real Window regression selects only geometry, switches languages, verifies all
  17 checkbox states, opens target review, and checks the original filename/path.
- Full suite initially timed out later at the library test's final deletion
  confirmation. Replaced its one-shot modal callback with a timer that waits for
  the actual confirmation and captures unexpected errors instead of hanging.
  Build and all six Qt suites pass (7.88 seconds), with no deletion error. Native
  copy/sync gestures and the broader acceptance inventory remain open.

## Duplicate review translation and queued viewport close race — 2026-11-08

- Duplicate progress/review headings, group labels, count summaries, failure
  labels and action buttons refresh live. A real two-file scan regression checks
  English/Chinese/English changes, unchanged paths/group data/current item,
  selection of both originals and preservation of both files.
- The expanded suite exposed a genuine intermittent error during deletion of the
  open virtual copy: a queued viewport command reached the backend after the
  document had closed. This refines the prior entry's test-timing diagnosis.
  The bridge still validates viewport requests but treats valid updates with no
  open view as no-ops. Window closure stops/clears pending viewport work.
- Added bridge coverage for a viewport arriving after close; the photo remains
  closed. Focused Rust integration, build and all six Qt suites pass (8.65
  seconds). Broader native/runtime and physical/service acceptance remains open.

## Saved-search dialog live translation — 2026-11-08

- Saved-search title, explanation and action buttons refresh live; user names
  and list selection remain untouched. A real Window regression uses a search
  named Library, changes language while selected, confirms unchanged persisted
  data, then opens it and verifies restored query/sort/direction.
- Build and all six Qt suites pass (8.63 seconds). Library-filter dialog live
  translation is still pending. Source review also found that its editable color
  label can retain a stale combo index while custom text is typed; serialization
  currently uses that index/data instead of the custom text. This is a concrete
  remaining fix, separate from native/physical/service acceptance.

## Custom label filtering and filter dialog translation — 2026-11-08

- Fixed custom color-label serialization: typed text that differs from the
  selected preset now becomes an Is criterion instead of saving stale preset
  data. Translation clears stale indices for custom text so a newly translated
  preset cannot reinterpret that text as a built-in choice.
- Library-filter title, heading, form labels/accessibility names, options and
  hints refresh live, retaining user metadata values, rating selection and
  custom label text selection.
- Real Window regression types a custom label over Red, switches to Chinese,
  accepts, and reopens to verify the custom label, camera and rating survived.
  Build and all six Qt suites pass (8.65 seconds). Native layout/accessibility
  and broader acceptance work remain open.

## Saved-search creation and duplicate-name validation — 2026-11-08

- Name entry for creating/renaming saved searches is now an owned QInputDialog
  with explicit object identity and live title/label translation. Staged names
  survive language changes; cancellation remains local to the naming flow.
- Real Window regression saves current query/rating sort/direction as “Travel”,
  verifies persisted criteria, then rejects a case-insensitive duplicate (“Library”)
  without mutating stored searches.
- Test setup now explicitly selects a current row before asserting it; focused
  preference/library workflow and all six Qt suites pass (8.92 seconds). Broader
  language coverage and native layout/accessibility acceptance remain open.


## Saved-search name dialog live translation and duplicate handling — 2026-11-08

- Saved-search create/rename now uses an explicit, owned name dialog whose title
  and label translate live while preserving staged text. List accessibility name
  is translated without changing actual saved names.
- Regression saves the current query and sort as “Travel”, verifies exact saved
  criteria, then attempts the case-insensitive existing name “Library”; the
  collision message is dismissed and existing entries remain unchanged.
- A crash in the new test was caused by checking a list row before selecting it;
  the fixture now selects row zero explicitly. Focused preference workflow and all
  six Qt suites pass (8.80 seconds). Remaining native and platform acceptance
  items stay open.

## Folder-specific filter lifecycle parity — 2026-11-08

- GPUI's `Filter::for_new_folder` preserves query/rating/label and clears
  camera/lens/date/keyword criteria. Qt previously carried all four folder-bound
  metadata filters into a different folder, often hiding every photo.
- Qt now applies the same reset when opening a different folder or a photo in a
  different folder, leaving same-folder refreshes unchanged. Added a focused
  criterion lifecycle regression; build and all six Qt suites pass (8.99
  seconds). Folder transitions involving albums and rapid asynchronous scans need
  native workflow acceptance alongside the remaining parity inventory.

## Folder/album listing transition reset — 2026-11-08

- Extended folder-specific filter reset to folder-to-album and album-to-folder
  transitions, including opening a photo while an album is active. Switching
  between albums preserves criteria, matching GPUI's shared `None` directory
  identity; same-folder rescans also preserve them.
- Qt build and all six CTest suites pass (8.99 seconds). Criterion reset is
  covered directly; native folder picker, album transition and rapid scan workflow
  remains an acceptance gap.

## Crop edge handles and outside rotation — 2026-11-08

- Added midpoint edge hit targets alongside corner resizing. Outside-crop drags
  now stage rotation about the crop center, matching GPUI's interaction type,
  and update the inspector angle; interrupted drags restore their initial angle.
- Added canvas regression for free top-edge resizing followed by outside rotation
  and one Apply command. Focused test and all six Qt suites pass (8.83 seconds).
- This is partial crop parity: shared perspective-aware drag constraints, fixed
  anchors at bounds, edge-crossing/minimum-size semantics, visible midpoint grips,
  double-click completion and native interaction acceptance still need work.

## Crop double-click completion — 2026-11-08

- Double-clicking inside the crop body now applies the staged crop and finishes
  the tool instead of changing zoom. Outside clicks, resize grips and non-left
  buttons do not complete the crop.
- Regression checks body completion, staged angle, unchanged zoom and excluded
  targets. Build and all six Qt suites pass (9.04 seconds). Perspective-aware
  constraints, boundary/minimum-size behavior, midpoint grip rendering and native
  interaction acceptance remain outstanding.

## Visible crop midpoint grips — 2026-11-08

- Canvas shader now draws all four midpoint grips in logical pixels. English and
  Chinese help describes edge resizing, outside rotation and double-click Apply.
- Built shader/application and passed all six Qt suites (8.37 seconds). Inspected
  native XCB/Vulkan screenshot at 1440×900 logical pixels, DPR 2; grips and help
  render correctly: [capture](qt-review/native/crop-midpoint-grips.png). Engine
  processing fell back to CPU after its GPU self-test mismatch; display was SDR.
  This is visual evidence, not physical HDR or pointer workflow verification.
- Perspective-aware constraints and boundary/minimum-size parity remain open.

## Crop resize boundaries and grab offset — 2026-11-08

- Resize uses displacement from the press position, retaining the grip grab
  offset. Moving edges stop before crossing their opposite edge, with minimum
  size based on 2% of the source short side, matching GPUI's rectangular math.
- Aspect-locked resizing scales back about the fixed corner/edge at image bounds;
  edge drags retain the orthogonal center. Pointer coordinates remain unbounded
  until rectangle constraints are applied.
- Added widget regression for no-motion grip offset and crossing the west edge
  on a nonsquare image, including the short-side minimum. Build and all six
  Qt suites pass (8.36 seconds).
- Perspective/straightening-aware shared constraints and native gesture acceptance
  remain open; rectangular bounds alone do not establish full crop parity.

## Shared crop drag constraints — 2026-11-08

- Added stateless `aigle_crop_geometry` ABI query with owned JSON response and
  panic/error containment. It calls core `Frame`/`drag` directly, without editor
  state or engine-thread round trips.
- Qt move and all eight resize handles now use GPUI's shared geometry, replacing
  the duplicated rectangular calculations. Angle changes fit the draft through
  the same geometry map. Outside rotation refits from its press-time rectangle
  so returning toward the original angle can grow the crop back; Escape restores
  the starting rectangle and angle.
- Rust regression covers rotated and combined perspective frames, all nine drag
  modes, valid bounds after serialization, invalid extent and null input.
  Canvas regression verifies a 30-degree square fit remains staged and interrupted
  rotation restores the draft. Rust focused test and all six Qt suites pass
  (8.42 seconds).
- Remaining crop audit includes aspect selection's largest-centered-rectangle
  semantics, inspector angle gesture anchoring, geometry adjustment constraints
  outside crop mode, and native pointer/render-coordinate agreement. These tests
  prove shared calculation and widget behavior, not complete native crop parity.

## Crop aspect centered refit — 2026-11-08

- Explicit ratio/Original selection now calls core `Frame::with_aspect`, matching
  GPUI's largest valid rectangle around the existing center. Free leaves the
  rectangle unchanged. The calculation retains perspective-aware constraints.
- Extent notifications retain the previous size-preserving ratio adjustment;
  they do not invoke explicit aspect selection or maximize a staged crop.
- Added widget regressions for off-center square expansion on a nonsquare image,
  Free/Original transitions, staged command behavior and extent-update stability.
  Build and all six Qt suites pass (8.26 seconds).
- Inspector angle gesture behavior, geometry edits outside crop mode and native
  pointer/render-coordinate agreement still require parity work. Inspection found
  the inspector slider currently emits changes on release, not throughout a drag.

## Live crop inspector angle gesture — 2026-11-08

- The crop angle slider now emits preview updates while held. Explicit gesture
  start/end signals connect the inspector to Canvas; fitting uses the rectangle
  captured at press so moving back toward the initial angle can restore its size.
  Switching tools clears the inspector gesture anchor.
- Changes remain staged until Apply. Added connected inspector/canvas regression
  for live updates before release, no premature backend commands, and full crop
  recovery after returning from 30 degrees to zero within one gesture.
- Build and all six Qt suites pass (8.30 seconds). Geometry edits outside crop
  mode and native pointer/render-coordinate acceptance remain outstanding.

## Geometry adjustments refit crop outside crop mode — 2026-11-08

- Generic bridge `edit` commands now follow GPUI's crop constraint hook. Geometry
  scalar/orientation changes refit via core Frame, append the fitted crop to the
  same command, and retain a gesture-start crop for continuous regrowth. Explicit
  crop/group/replacement edits preserve their supplied crop and clear the anchor.
- Added bridge regression for angle gesture regrowth, single-step undo/redo,
  explicit crop preservation, and perspective correction with automatic zoom
  disabled. Automatic zoom can already keep a full crop valid, so shrinkage alone
  is not a valid assertion for every perspective edit.
- Focused regression, real-file retained-frame/history/export/sidecar integration,
  and all six Qt suites pass (Qt: 8.68 seconds).
- This covers the generic adjustment command path. Other command sources (preset,
  imported profile, proposals, selective edits) still need a constraint-policy
  audit, along with native crop pointer/render-coordinate acceptance.

## Shared open-photo edit policy audit — 2026-11-08

- Centralized all bridge writes to an open Photo through `Photo::apply`, which
  invokes the crop constraint hook before document application. This includes
  snapshots, Agent acceptance, profile/lens import, automatic adjustments,
  retouch/tool commands, and selective paste/sync of the open target.
- Source audit: Agent `profile_ops` writes complete changed groups; selective
  copy/sync (and presets using those operations) writes selected complete groups.
  Geometry therefore includes its explicit crop and bypasses automatic refitting,
  as in GPUI. Snapshot ReplaceAll similarly preserves stored geometry. Offline
  sync operates on complete groups and needs no image-size-dependent refit.
- Full bridge suite: 21 passed, 1 ignored (isolated Secret Service session needed).
  Qt build and all six suites pass (8.21 seconds). This closes the command-source
  constraint-policy audit, not native crop interaction or broader platform parity.

## Rotated crop native rendering and pivot — 2026-11-08

- Added `crop-rotated` capture state, staging 30 degrees without applying edits.
  Inspected XCB/Vulkan capture at 1440×900 logical, DPR 2:
  [rotated crop](qt-review/native/crop-rotated.png). Grips remain axis-aligned as
  the image rotates beneath them; angle inspector and constrained crop render.
- Fixed shader rotation pivot: it previously derived the center of the returned
  frame tile from texture mapping. It now receives the actual image center in
  viewport coordinates, independent of tile region during pan/zoom.
- Six crop widget regressions also passed under XCB at 2× scaling (injected widget
  events, not desktop pointer automation). Build and all six Qt suites pass
  (8.50 seconds). Capture used SDR; engine processing fell back to CPU after the
  existing four-code-value GPU self-test mismatch.
- Zoomed/panned tile-boundary visual acceptance and physical pointer workflows
  remain unverified; the fit-view capture does not prove those cases.

## Rotated crop sampling coverage at zoom — 2026-11-08

- Crop viewport requests now include inverse-rotated viewport corners around the
  image center, unioned with the unrotated region for before-view coverage. Output
  resolution preserves pixel density up to the existing 16384-pixel dimension
  limit. Angle changes request continuous frames and schedule a settled frame.
- Added a pure viewport regression with an off-center zoom and analytically
  checked rotated corners. All six Qt suites pass (8.39 seconds).
- Added `crop-rotated-zoom` capture state. The first attempt was reset to fit by
  late viewport initialization; scheduling navigation after initialization produced
  the intended 3× zoom, off-center, 30-degree capture. Inspected image coverage:
  [capture](qt-review/native/crop-rotated-zoom.png). XCB/Vulkan, DPR 2, SDR with
  engine CPU fallback. No missing tile wedges appeared in this settled view.
- Rapid navigation/rotation, large RAW performance and physical pointer workflows
  remain unverified; a single settled synthetic capture does not prove those.

## Repository DNG fixture bridge verification — 2026-11-08

- Located the existing Thesauros synthetic DNG and added a bridge integration
  regression using a temporary copy. LibRaw loads its 256×256 RAW plane (the
  first TIFF IFD is a 64×48 preview), exposes RAW source kind, renders to PNG,
  and reopens both the rendered output and original source successfully.
- Confirmed engine policy rejects DNG-to-DNG conversion with “the photo is already
  a DNG” and creates no destination file. Focused regression passes (0.51 seconds).
- This fixture does not verify conversion from camera RAW to DNG, real-camera
  demosaicing quality, DCP matching, or lens correction. Those remain open.
  The export dialog's early handling of already-DNG inputs needs auditing against
  GPUI; the backend restriction is now covered with an actual source file.

## Early rejection of already-DNG export inputs — 2026-11-08

- Audit found DNG-to-DNG rejection is shared engine behavior, not a Qt-only
  missing capability. Qt now excludes DNG output when a known input path has a
  DNG suffix (case-insensitive), including mixed RAW/DNG batches and the single
  source dialog path. Snapshot source identity takes precedence over display name.
- Existing unavailable-preset validation requires an explicit alternative format,
  rather than silently changing a saved DNG preset. Other RAW conversion stays
  available. Engine validation remains authoritative for mislabeled files.
- Extended export regression covers uppercase DNG with a non-file display name,
  mixed batch submission prevention and single-source exclusion. Build and all
  six Qt suites pass (8.39 seconds). Real camera-RAW conversion remains unverified.

## Linux credential integration verified — 2026-11-08

- Environment inspection found `dbus-run-session` and `gnome-keyring-daemon`.
  Ran the normally ignored `isolated_keyring_round_trip` test against a real
  Secret Service daemon in a private session bus. XDG data/config/cache/runtime
  directories were temporary and GNOME_KEYRING_CONTROL was unset. The daemon
  used a disposable test password, was terminated afterward, and temporary
  directories were removed; the desktop keyring was not used.
- Command inside that session: `AIGLE_TEST_ISOLATED_KEYRING=1 cargo test -p
  aigle-qt-bridge isolated_keyring_round_trip -- --ignored --nocapture`.
  Result: 1 passed, 0 ignored (0.22 seconds).
- Coverage includes store/load/replace/remove, endpoint separation, provider
  isolation, legacy-entry migration, and removing one provider across endpoints
  without deleting another provider's key. Linux credential round-trip validation
  is no longer blocked on an unavailable Secret Service facility.
- This does not establish macOS/Windows secure storage, locked-keyring prompt
  handling, live model-provider authentication, or FreeBSD runtime behavior.

## Wayland launch/render facility verified — 2026-11-08

- Existing desktop Wayland capture timed out with the canvas visible/unexposed.
  Isolated virtual KWin on a private bus and temporary XDG directories succeeded,
  with configured, exposed Vulkan canvas. Inspected DPR 1 rotated/zoomed crop:
  [capture](qt-review/native/wayland-crop-rotated-zoom.png).
- This establishes a working Wayland launch/render path in this environment,
  superseding the older blanket lack of usable Wayland capture evidence. Portal
  mount/registration warnings, current desktop exposure behavior, native input,
  fractional scale, physical display and packaged launch still require checks.

## Installed-prefix launch and translation — 2026-11-08

- Installed current Qt build with deployment enabled into
  `/tmp/aigle-qt-install-review`. Launched from `/tmp` with LD_LIBRARY_PATH and
  QT_PLUGIN_PATH removed and isolated settings/data. `ldd` resolves the Qt
  libraries and Rust bridge from the prefix; embedded shaders render through
  Vulkan. Installed tree includes reference data, icon and desktop entry.
- Inspected English and Chinese captures at DPR 2. Chinese review exposed a
  missing Window-context Retouch translation; added it and marked all four
  toolbar labels with QT_TR_NOOP for extraction. Rebuilt/reinstalled and inspected
  [corrected installed Chinese capture](qt-review/native/installed-chinese-crop.png).
- All six Qt suites pass (8.41 seconds). Install emits CMake CMP0207 developer
  warnings but succeeds. Engine processing falls back to CPU on the known GPU
  self-test discrepancy. This verifies same-host prefix launch, not distribution
  portability/AppImage, reference-data consumption, or physical HDR output.

## AppImage deployed-library policy fix — 2026-11-08

- Qt deployment installs glibc and Vulkan loader dependencies into the AppDir's
  lib64 directory. Xtask's existing host-library filter only skipped a later
  manual copy into lib, leaving those deployed host libraries bundled despite
  its explicit policy to use the host loader/glibc/GPU stack.
- Packaging now removes host-library files and symlinks from both usr/lib and
  usr/lib64 after deployment/dependency collection. Qt, the Rust bridge and C++
  runtime remain bundled. Regression covers both layouts and a Vulkan symlink.
- All three AppImage unit tests pass. No appimagetool was found on PATH and no
  Qt release binary exists yet; this turn verifies the packaging fix, not an
  assembled AppImage or cross-distribution launch. Installed-prefix launch was
  verified separately above.

## Release AppImage assembly and launch — 2026-11-08

- Located cached appimagetool 1.9.1 under target/appimage-tools (absent from PATH).
  Built Qt/Rust release and assembled `/tmp/Aigle-qt-review.AppImage` using xtask.
- First artifact failed with undefined `aigle_comparison_preview`: manual ldd-based
  dependency copying shadowed CMake's current installed bridge in usr/lib64 with
  an older build-tree bridge in usr/lib. Fixed collection to preserve libraries
  already installed in either library directory, then rebuilt the artifact.
- Actual AppImage extract-and-run launch from /tmp with library/plugin overrides
  removed succeeds. Inspected native DPR 2 SDR Vulkan capture:
  [AppImage crop](qt-review/native/appimage-crop.png). Host-library exclusion
  inspected in AppDir; all three packaging tests pass.
- This verifies release assembly and same-host extract-and-run execution. FUSE
  mounting, cross-distribution portability, physical HDR and packaged Wayland
  remain separate checks. Engine uses CPU after the known GPU self-test mismatch.

## AppImage FUSE and Wayland launch — 2026-11-08

- Launched `/tmp/Aigle-qt-review.AppImage` directly with extract-and-run unset,
  library/plugin overrides removed, and isolated application settings. XCB capture
  completed successfully at DPR 2. A private-bus virtual KWin run also launched
  the binary under `/tmp/.mount_Aigle-…/usr/bin/aigle-gui` and produced a working
  Wayland/Vulkan crop capture, confirming the mounted artifact path.
- Inspected [packaged Wayland capture](qt-review/native/appimage-wayland-crop.png).
  Requested virtual-compositor scale 1.5, but application reported DPR 1; this is
  evidence for packaged Wayland launch/rendering only, not fractional scaling.
- Portal mount/registration warnings persist in the private session. GPU image
  processing uses CPU fallback. Cross-distribution compatibility, real fractional
  output scale, portal dialogs, physical display and native input remain open.

## Explicit fractional DPR rendering — 2026-11-08

- XCB native output has baseline DPR 2. QT_SCALE_FACTOR=1.5 produced DPR 3,
  not 1.5; corrected to QT_SCALE_FACTOR=0.75 and confirmed application log DPR
  1.5. Inspected [fractional DPR crop capture](qt-review/native/crop-zoom-dpr150.png)
  at 1440×900 logical, zoomed/off-center with 30-degree staged rotation.
- Four crop widget-event regressions pass under XCB with the same scale override:
  double-click completion, grip grab offset/minimum, live angle gesture/regrowth,
  edge resize/outside rotation. No missing image wedges or clipped inspector
  controls observed in this capture.
- This verifies Qt fractional-DPR rendering and injected widget gestures for the
  crop view. It does not verify compositor fractional-scale negotiation, physical
  pointer input, all dialogs, or enlarged-font accessibility.

## Chinese export dialog at fractional DPR — 2026-11-08

- Inspected [Chinese export capture](qt-review/native/export-chinese-dpr150.png)
  on XCB, confirmed DPR 1.5. Header, format/size controls, summary and bottom
  actions fit; further options reside in the existing scroll area. This capture
  does not verify scrolling/focus through every option or enlarged fonts.
- Capture exposed an implementation gap: export preview is supplied from the
  current library item's ImageRole (window.cpp showExport/captureLayout), so
  synthetic sources have a null pixmap and edited output is not guaranteed to be
  represented. A true export preview must be generated from the frozen source
  and edit snapshot rather than treating the thumbnail as output evidence.
  This remains open; no output-preview parity claim follows from this capture.

## Frozen-snapshot export image preview — 2026-11-08

- Normal export preparation requests a bounded 388px SDR thumbnail from the
  first frozen export source and its exact EditParams on the engine worker.
  Window decodes that result with dimension/stride checks instead of using the
  currently selected library item's thumbnail. Preview failure does not discard
  otherwise valid export inputs; null pixmaps are no longer scaled.
- Extended bridge regression verifies synthetic preview generation, exact edited
  snapshot identity, changed pixels after curve edits, and intact undo/redo.
  Focused Rust test and all six Qt suites pass (8.54 seconds).
- This is an edited source preview, not an export-settings simulation: watermarks,
  resize/sharpen, output profile and HDR encoding are not reflected. The dedicated
  layout-capture helper still supplies its older thumbnail fixture; native normal
  export preview and error presentation remain to be verified.

## Export preview status and scope — 2026-11-08

- Renamed the panel to “Edited photo preview” and added a tooltip explaining
  that SDR photo edits are shown, excluding export color/size/sharpen/watermarks.
  Failed or absent previews show a translated unavailable message; engine failure
  details appear in the tooltip without preventing otherwise valid exports.
- English/Chinese strings included. Widget regression verifies failure display,
  retained export availability, successful pixmap display and scope tooltip.
  Build and all six Qt suites pass (8.58 seconds).
- Native normal-export preview, full export-settings simulation, and remaining
  workflow/platform acceptance are still open; wording does not close those gaps.

## Native normal-export preview verified — 2026-11-08

- Added `export-live` capture state which invokes the normal exportPhoto request,
  receives frozen inputs/engine preview, and opens the actual modal dialog.
  Capture succeeds only with a non-null preview pixmap; it rejects the dialog
  afterward without queuing an export.
- Built and inspected [Chinese native export preview](qt-review/native/export-live-chinese-dpr150.png)
  on XCB at confirmed DPR 1.5. Synthetic image is visible in the preview, replacing
  the empty thumbnail in the older capture helper. No null-pixmap warning.
- This establishes native display of the engine-generated preview through the
  normal preparation path. Export-settings simulation, large-source preview
  latency/cancellation and broader workflow acceptance remain open.

## Remaining-scope correction and platform credential audit — 2026-11-08

- Rechecked GPUI export.rs: it defines export settings/controls and output
  requests, but no rendered preview of watermark/profile/sharpen settings.
  Therefore export-settings preview simulation mentioned in recent entries is
  optional enhancement work, not a GPUI parity gate. The Qt edited-source preview
  and its explicit limitations can stand without implementing that enhancement.
- Confirmed a real implementation gap in credentials.rs: outside Linux/FreeBSD,
  save(Some(key)) returns “Secure credential storage is unavailable”, load returns
  None, and deletion is a no-op. GPUI providers.rs uses platform read/write/delete
  credential APIs. Native macOS/Windows storage still requires implementation and
  native validation; passing Linux Secret Service tests does not close it.
- Preserve the remaining original acceptance scope from 08/09: individual global
  parameter conditions/reset/units, library/error/recovery workflows, real-camera
  profile/RAW cases, live provider/remote behavior, performance, accessibility and
  physical color/display checks. Recent packaging/crop captures prove only their
  recorded cases, not these broader requirements.

## Native credential implementation — 2026-11-08

- Replaced macOS/Windows unsupported-storage stubs with keyring 4.2.0 native
  Keychain/Credential Manager operations. Provider-specific secure entries hold
  normalized endpoint/key maps, allowing provider deletion across endpoints
  without a plaintext index. Missing entries are handled idempotently; native
  failures propagate to the caller. Platform credential-size limits still apply.
- Published API checked against https://docs.rs/keyring/4.2.0/keyring/ and its
  downloaded crate source. Cargo.lock includes the platform dependencies.
- Linux credential tests: 2 passed, 1 intentionally ignored. These establish
  normalization/provider isolation, not native platform compilation or storage.
  Added an opt-in native CRUD/replacement/endpoint-cleanup integration test using
  a unique disposable provider. It has not run on macOS or Windows.
- Existing Qt binaries: all six CTest suites pass (8.78 seconds). No native
  macOS/Windows build or runtime claim follows from this Linux test run.
- Remaining acceptance includes the parameter conditions/reset/unit audit,
  library cancellation/recovery, camera RAW/DCP/lens fixtures, live provider and
  remote workflows, performance, accessibility, and physical display/color tests.
  Full parity remains unconfirmed; export-settings preview simulation is not
  a GPUI parity requirement.
