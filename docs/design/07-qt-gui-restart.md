# Aigle GUI restart — design brief

Status: visual direction approved and native Qt6 implementation requested by the
user, 2026-11-07. The photo-first layout is implemented; full workflow parity and
the release gates below remain open.

Companion: [workflow parity inventory](08-qt-workflow-parity.md).

Approved visual reference: [interactive mockups and screenshots](qt-review/README.md).
Implementation evidence: [native screenshots and verification](qt-review/native/README.md).
Visual approval does not close the runtime baseline audit.

## Product contract

Aigle is a desktop RAW editor with a library, precise non-destructive editing,
local adjustments, color analysis, and an editing assistant. The photograph is
the visual center. Common work should be direct; advanced work should be
discoverable without editing serialized data.

The restart succeeds when an existing user can complete their workflows with
equal or better control, and the interface feels deliberately designed. A
parameter appearing in a dialog is not evidence that its workflow is supported.
Visual quality and workflow parity are separate release requirements.

Keep the Rust processing/document foundation where it meets the contract.
Evaluate the Qt bridge and presentation work for reuse independently of the
current window implementation. Preserve the current working tree as reference;
this plan does not call for deleting or reverting the previous attempt.

## Evidence from the current attempt

This is a source review, not a completed runtime or screenshot audit:

- `apps/aigle/src/adjustments.cpp` sends masks, spots, crop structures, profiles,
  and other complex values through a generic structured editor. The redesigned
  UI needs dedicated interactions for these tasks.
- `apps/aigle/src/window.cpp` filters the displayed item text, whereas the legacy
  library supports metadata-aware search, saved criteria, and exclusion terms.
- Its export dialog offers eight hard-coded formats and size/color-space fields.
  The legacy export UI has capability-filtered formats, quality, naming, presets,
  metadata policy, sharpening, watermarks, paired SDR delivery, and a persistent
  queue. Existing bridge export support is not equivalent to that workflow.
- Develop allocates fixed 260/340 px sidebars. At a 900 px minimum width, those
  sidebars consume two thirds of the window before other spacing. The new layout
  must budget the canvas explicitly.
- `apps/aigle/README.md` acknowledges missing graphical mask handles, saved-search
  management, shortcut editing, and waveform/vectorscope replacements. It also
  contains stale layout/build descriptions; documentation is evidence to check,
  not a parity certificate.
- Agent acceptance and credential storage differ between frontends. Those are
  product/data compatibility decisions, not incidental toolkit choices.

## Workspace and navigation

Two primary modes: **Library** and **Develop**. Compare is a temporary Library
workspace for two to four selected photos, not a third application mode. Export
and import run as background jobs with a persistent activity entry point.

Keep the window controls and menus conventional. Place Library/Develop on the
left of a restrained application bar, the current source/photo in the middle,
and Jobs/Export on the right. Preferences live in the application menu and keep
the existing shortcut. Avoid a toolbar containing every command.

### Develop — proposed spatial wireframe

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Aigle   Library  Develop       DSC_2048.NEF                  Jobs  Export…   │
├─────────────────┬──────────────────────────────────────┬───────────────────┤
│ Histogram ▾     │ Pan  Crop  Heal  Red eye  Masks       │ Edit      Agent   │
│                 │                 Before/after  Fit ▾  ├───────────────────┤
│ Channels / Log  ├──────────────────────────────────────┤ Find adjustment…  │
├─────────────────┤                                      │ ▾ Light & WB      │
│ History         │                                      │   As shot  Auto   │
│ Snapshots       │                                      │   Temperature     │
│ Presets         │              PHOTOGRAPH              │   ─────●──  5200 K│
│                 │                                      │   Exposure        │
│ Exposure +0.35  │                                      │   ────●── +0.35 EV│
│ White balance   │                                      │ ▸ Curves          │
│ Original        │                                      │ ▸ Color           │
│                 │                                      │ ▸ Detail          │
│                 ├──────────────────────────────────────┤ ▸ Optics/Geometry │
│                 │ 35 mm · f/4 · 1/250 · ISO 100        │ ▸ Effects         │
├─────────────────┴──────────────────────────────────────┤ ▸ RAW / Profiles  │
│ Folder / album   Filter   Sort       Selected 1 of 126 │ ▸ HDR mastering   │
│ [thumbnail] [thumbnail] [selected] [thumbnail] […]      │ Info / Metadata   │
├────────────────────────────────────────────────────────┴───────────────────┤
│ Saved                                  Display: SDR · sRGB                 │
└────────────────────────────────────────────────────────────────────────────┘
```

This is an information/layout wireframe, not a visual approval mockup. The
filmstrip spans the left and canvas areas; the right inspector retains its
height. Photo metadata on the canvas footer is optional. The left rail groups
scopes above a switchable History/Snapshots/Presets section; it does not stack
three full lists permanently.

- At 1440×900 logical px: start with a 224 px left rail, a 320 px inspector,
  44 px application bar, 36 px canvas tool row, 112 px filmstrip, and 24 px
  status row. Remaining width belongs to the canvas; splitters are resizable.
- Below 1280 px: collapse the left rail by default, retaining a labeled button
  to open it. Below 1100 px: start with the filmstrip collapsed too. Never
  silently discard the selected image, active tool, or inspector state.
- At the provisional minimum 1024×700: retain a 300 px inspector and at least
  640 px canvas width with the left rail closed. Opening the rail at this size
  temporarily replaces the inspector; closing it restores the inspector.
- Preserve the user's panel choices per workspace and screen size class. Do not
  auto-collapse repeatedly while a user drags a splitter. No arbitrary docking
  or workspace-layout editor is needed for the restart.
- Edit has collapsible, searchable groups. Search reveals and focuses the real
  control; it does not create a second copy of the control. Common light/WB
  controls start open. Information and editable metadata share a secondary view.
- Selecting Crop, Retouch, or Masks shows its purpose-built inspector in the
  same right rail, with an explicit return to global adjustments. Merely opening
  a tab must not create a mask or activate an unrelated drawing tool.

### Library — proposed spatial wireframe

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Aigle   Library  Develop       Pictures / Autumn            Jobs  Export…  │
├─────────────────┬──────────────────────────────────────┬───────────────────┤
│ Folders         │ Search photos…       Filter   Sort ▾ │ Selection: 3      │
│   Autumn        │ 126 photos · 3 selected              │                   │
│ Albums          ├──────────────────────────────────────┤ Rating / Flag     │
│ Saved searches  │ [photo]     [photo]     [photo]      │ Color label       │
│                 │ filename    filename    filename     │ Keywords          │
│                 │ rating      rating      rating       │ Copyright         │
│                 │                                      │                   │
│                 │ [photo]     [photo]     [photo]      │ Mixed values      │
│                 │ filename    filename    filename     │ shown explicitly  │
│                 │ rating      rating      rating       │                   │
├─────────────────┴──────────────────────────────────────┴───────────────────┤
│ Import…           Compare selected   Sync edits…            Thumbnail size │
└────────────────────────────────────────────────────────────────────────────┘
```

Images use a consistent cell with fit-inside thumbnails: never crop away image
content just to fill a grid. Keep rating, flag, copy/stack identity, and selection
legible. Use a thin selection border plus a check marker, not a strong tint over
the photograph. The metadata inspector can collapse; multi-selection shows mixed
values and the number of affected photos. Batch actions apply to the selection;
Develop adjustments apply only to the active photo unless Sync is invoked.

## Visual system to prototype

These are starting tokens for a desktop mockup, not final approved colors:

| Token | Proposal | Purpose |
| --- | --- | --- |
| Canvas surround | `#181818` | Neutral photo surround; no gradients or texture |
| Panel | `#242424` | Quiet, flat editing surface |
| Raised/input | `#2D2D2D` | Small separation for fields and menus |
| Divider | `#3B3B3B` | Structural boundary, not a border around every group |
| Primary text | `#E6E6E6` | Labels and editable values |
| Secondary text | `#B0B0B0` | Units, source details, supporting labels |
| Focus/selection accent | `#8AAED4` | Restrained focus ring and active indicators |

Use the platform UI font at 13–14 logical px; secondary text at least 12 px.
Respect system text scaling and Chinese font fallback. Use tabular numerals for
editable values. Avoid uppercase section headings and broad, saturated fills.
Use a 4 px spacing unit, 8–12 px within groups, and 16 px between groups. Controls
are generally 28–32 px tall, with 4 px corner radii. Icons use one consistent
16–18 px family; essential tool names remain visible. Focus must be obvious.

One adjustment row has a label and right-aligned number/unit, with a compact
slider beneath. The value supports direct entry, arrow increments, and fine
adjustment. Modified state and reset stay discoverable by keyboard as well as
pointer. Reset visibility must not make neighboring content jump.

Colorful controls are justified for color editing and scopes; other chrome stays
neutral. No decorative cards, heavy shadows, giant rounded controls, or oversized
headings in the working surface. Validate text/control contrast, including
inactive and selected states, in the actual Qt prototype.

## Interaction contracts

| Workflow | Required behavior |
| --- | --- |
| Slider/curve drag | Immediate bounded preview; one gesture becomes one undo step; final-quality render on completion; Escape cancels an in-progress gesture |
| Crop/straighten | Visible handles, aspect lock/presets/custom ratio, rotate/flip, horizon line; clear Apply/Cancel; image-coordinate behavior survives zoom and orientation |
| Masks | Named/selectable list; create brush/linear/radial/range masks; visible handles, feather, invert, opacity, local controls; combine existing coverage with accurately described snapshot semantics |
| Healing/clone/red eye | Select and modify existing corrections on the photo; visualize source/target; adjust radius/feather/opacity where supported; delete and undo |
| Navigation | Fit, actual pixels, cursor-anchored zoom and pan; preserve selection/context through Library/Develop; Escape follows the active interaction, not a global destructive action |
| Before/after | Distinguish original comparison, split view, and multi-photo survey; show which comparison is active and use consistent viewport geometry |
| Save/conflict | Nonblocking Saving/Saved indicator; failed or conflicting save stays visible with recovery action; never report Saved for only a rendered preview |
| Export | Single and batch export, capability-filtered formats, full options and presets; destination/name preview; per-file queue, pause/resume/retry/cancel and collision handling |
| Agent | Provider/model selector, image/profile messages, cancellation, usage and stale-result handling; propose explicit Preview → Apply/Discard as a deliberate behavior change to review |
| Preferences | Staged Save/Cancel, languages, display settings, subfolders, provider/model management, keyring-backed secrets, editable conflict-aware shortcuts |

The proposed agent Apply action creates one undoable document edit. Intermediate
previews never replace the saved document. Keep per-photo conversations and make
the proposal's base revision explicit internally. Restoring existing provider
credentials and settings is part of parity; plaintext/in-memory-only credentials
are not a replacement for the existing persistent keyring workflow.

## States that must be designed

Prepare designs for: empty library; no search matches; missing original; decoding
and progressive preview; unsupported/corrupt file; catalog unavailable; selected
virtual copy; remote checkout and failure; save conflict; unavailable export
codec; paused/failed export; agent disconnected/running/cancelled/stale result;
HDR unavailable or lost after moving windows between displays.

Use inline status for local issues, a jobs surface for long-running work, and
dialogs for decisions that need them. Keep the previous useful image visible
during an update, but identify it as loading/stale. Errors must not vanish just
because the user changes panels. Standard text selection/editing shortcuts take
precedence while typing; image shortcuts resume outside text inputs.

## Architecture decision to validate

Qt6 is settled; **Qt Widgets versus Qt Quick is not yet settled**. Start by
evaluating Widgets with a small, deliberately styled component set and custom
canvas/scopes. It has the closest reusable implementation and established desktop
menus/dialogs. Qt Quick is an alternative if it materially improves the approved
design or input behavior; do not switch merely to obtain different defaults.

Before committing, compare the same inspector/canvas slice for HiDPI text,
keyboard/focus/accessibility, brush latency, scrolling, Linux Wayland behavior,
and SDR/HDR surface integration. A static mockup cannot validate these. Reuse of
QRhi/HDR code remains conditional on actual display tests. Avoid building two
complete frontends to make this decision.

Proposed boundaries:

- Rust services: PhotoDoc/history, validation, sidecars/catalog, processing,
  library operations, jobs, agent execution. Extract UI-independent legacy
  orchestration where needed; do not reimplement it in C++ by default.
- Bridge: versioned commands/results, capabilities, job IDs, document/photo IDs,
  revisions, errors and explicit buffer ownership. A parameter schema supplies
  validation/ranges, not the final layout or interaction design.
- Qt controllers/models: workspace selection, tool sessions, library rows,
  inspector presentation, job and agent presentation. One command registry drives
  menus, shortcuts, toolbar actions, and enabled state.
- Qt views: reusable adjustment row, group header, thumbnail cell, tool inspector,
  scopes, status, and dialogs. Domain mutations flow through document commands.
- Rendering: async/coalesced viewport updates; discard obsolete frames; never
  block input on decoding, export, network activity, or catalog scans. Keep HDR
  image presentation distinct from SDR chrome and report actual output mode.

## Delivery gates

| Gate | Deliverable | Exit evidence |
| --- | --- | --- |
| A — Baseline | Complete feature/action/parameter ledger; GPUI screenshots and task recordings; audit Qt behaviors | Every existing workflow has a destination, acceptance case, and explicit status; outstanding original features are separated |
| B — Design | Develop and Library visual mockups; crop/mask/agent/export states; component sheet; narrow-window designs | User reviews real photo examples and approves hierarchy, density, and interactions before replacement work |
| C — Technical slice | One photo → exposure → undo → save/reopen; one mask interaction; SDR/HDR presentation | Toolkit decision supported by actual Qt screenshots, input/accessibility checks, display tests, and measured responsiveness |
| D — Workflow migration | Library, editing/tools, analysis/color, delivery, settings/agent/remote in complete slices | Each ledger case is demonstrated end to end with persistence/error behavior; no generic editor counted as a finished tool |
| E — Replacement | Compatibility rehearsal, packaged app, full parity and visual review | No unresolved must-retain regression; approved deviations recorded; default launcher/package/docs switched together |

Migration slices are implementation order, not permission to drop later slices.
Do not call the restart complete at the end of the basic-editing slice. Retain a
usable reference frontend until Gate E; first verify its actual build/run state
because the working tree already contains substantial migration changes.

## Review and validation plan

- Compare identical representative files: landscape, portrait, high-ISO RAW,
  wide-gamut color, HDR highlight scene, rotated/cropped photo, local-mask photo,
  virtual copy, and missing/remote original. Use approved test assets.
- Review at 1440×900, 1920×1080, and 1024×700 logical px, at 100%, 150%, and 200%
  display scale. Include Chinese labels, enlarged fonts, keyboard-only navigation,
  and long filenames. Verify actual canvas geometry rather than screenshot scale.
- Use behavioral tests for gesture transactions, coordinate transforms, selection
  scope, cancellation/staleness, saves/conflicts, and queue recovery. Preserve
  engine/data tests and add Qt interaction tests for the user-visible contracts.
- Record screenshot baselines for components and complete workflows. Approve the
  baseline before using automated image diffs; a stable ugly screenshot is not a
  successful design test. Mockups alone cannot certify HDR/color correctness.
- Measure drag-to-preview and input responsiveness with fixed hardware/photos;
  set performance budgets after baseline capture, not invented timings. Large
  library tests must confirm virtualization and useful loading feedback.

## Next design review

Review the proposed panel placement and density first, then produce polished
Develop/Library mockups using representative photographs. Review crop, masks,
agent Apply/Discard, and export as concrete states before implementing the new
shell. The initial source inventory below is intentionally not a claim that the
full baseline audit or visual design approval has already happened.
