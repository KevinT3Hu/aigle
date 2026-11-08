# Qt parity completion work

Active user objective: finish all remaining items one by one. This checklist
preserves the full scope; completing one slice does not close the objective.
The individual baseline cases in [the inventory](08-qt-workflow-parity.md) and
[release gates](07-qt-gui-restart.md#delivery-gates) still apply. Implemented,
automatically tested, and native/runtime-verified are distinct evidence levels.

1. [x] **Scopes implementation and workflow verification (V02).** Seven modes,
   shared calculations, exact full-crop bins, bounded spatial/region analysis,
   SDR/HDR numerical units, target-gamut/output-boundary statistics, reference
   guides, cached pointer/keyboard measurements, local shortcut priority, region
   gestures/cancellation/reset, preferences, stale reply handling. Engine, bridge,
   Qt tests and native 1440×900/1024×700 captures are recorded in the
   [native review](qt-review/native/README.md#scopes-verification--2026-11-07).
   Physical HDR/display validation remains item 3/8 below.
2. [ ] **Specialized editing (D01–D08).** Start with on-image selection and editing
   of existing heal/clone/red-eye corrections: move target/source, radius,
   feather/opacity where supported, delete, gesture undo/cancel and reopen.
   Complete dedicated camera profile/LUT/lens/RAW workflows and remaining mask
   manipulation (including color/range workflows and nonlinear guide accuracy).
   Audit every global/curve parameter, enable condition, unit/range, reset,
   crop/orientation/perspective, combination, history/snapshot and undo workflow.
3. [ ] **Color/display (V01/V03/L05).** Soft proof, gamut warnings and display
   controls, HDR comparison, SDR/HDR fallbacks and screen transitions, viewport
   geometry/before-after/clipping across crop/orientation/zoom. Verify physical
   display output rather than using an SDR screenshot as HDR evidence.
4. [ ] **Preferences and shortcuts (S01/S02).** Chinese localization, remaining
   general/display preferences, all commands in the shortcut editor, configurable
   dialog bindings/chords, conflict/staging/persistence/text-input behavior.
5. [ ] **Agent completion (A01/A02).** Markdown, image/profile messages, stable
   conversation identity through moves/renames, live-provider/vision verification,
   secure storage outside Linux/FreeBSD. Verify cancellation, failures, navigation
   and overlapping edits, stale proposals, Apply/Discard and accepted-edit undo.
   Persistent chats and named providers/discovery/Linux credentials are already
   implemented with recorded tests; they are not substitutes for the other cases.
6. [ ] **Library and delivery (L01–L09/D09/D10/E01/E02/P01–P03).** Complete virtual
   copy presentation; import options/cancel/subfolders; selection/filter/sort and
   full editable/mixed metadata parity. Verify save conflicts/recovery, history,
   files/sidecars/catalog identity and recovery, copy/sync/presets, all supported
   export formats/options and restrictions, jobs/restart/collision behavior.
   Many implementations exist, but native/error-path audits are still required.
7. [ ] **Remote (R01).** Thesauros authentication, checkout, edits/save/sync,
   disconnect, permission/network failures and conflict recovery end to end.
8. [ ] **Release verification (X01 and gates A–E).** Complete case/action/parameter
   audit, native workflow and failure/recovery demonstrations, large-library and
   large-RAW performance/memory, keyboard/focus/accessibility, 100/150/200% HiDPI,
   Chinese/enlarged text, Linux XCB/Wayland, packaged install/launch, final visual
   review. Record evidence for every case or an explicit user-approved deviation.
   No physical device/display or live service claim may be inferred from a mock.

The original feature-expansion backlog listed separately in the inventory remains
separate from Qt regressions; this checklist does not silently add or remove it.
