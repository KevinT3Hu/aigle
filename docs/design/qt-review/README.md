# Aigle visual design review — first pass

2026-11-07 · Visual design approved; native Qt implementation requested and built.

See [native screenshots, verification, and remaining differences](native/README.md).
The sections below describe the original presentation prototype.

Open [the interactive preview](index.html) in a desktop browser. It also works
directly from disk, with no build step or external requests. The top review bar
switches between Develop, Library, Masks, Crop, Agent, and Export.

This is a disposable HTML/CSS presentation prototype for the **Qt6 desktop
application**, not a proposed web frontend or toolkit decision. The app source,
Rust services, existing Qt implementation, preferences, and photographs are not
modified by the preview. Session changes are in browser memory only.

## Screens to review

| State | Desktop screenshot | What to assess |
| --- | --- | --- |
| Develop | [1440×900](screenshots/develop-1440.png) | Photo prominence, neutral surround, adjustment density, scopes/history placement |
| Library | [1440×900](screenshots/library-1440.png) | Fit-inside thumbnails, portrait photos, selection borders, mixed metadata, copy identity |
| Masks | [1440×900](screenshots/masks-1440.png) | Named mask list, on-image controls, local/global distinction, overlay controls |
| Crop | [1440×900](screenshots/crop-1440.png) | Handles, guide, aspect controls, explicit Apply/Cancel |
| Agent | [1440×900](screenshots/agent-1440.png) | Proposal hierarchy, image context, Apply/Discard, composer placement |
| Export | [1440×900](screenshots/export-1440.png) | Options hierarchy, filename/output preview, background delivery |
| Laptop Develop | [1024×700](screenshots/develop-1024.png) | Collapsed rail/filmstrip, usable canvas, scrollable inspector |
| Laptop Library | [1024×700](screenshots/library-1024.png) | Sources hidden, selection inspector, grid legibility |

## Decisions embodied in this pass

- Quiet neutral panels and a slightly darker canvas surround. Selection uses a
  thin blue-gray indicator; photographs are not tinted by selection.
- Conventional title/menu rows, two workspace modes, and one restrained toolbar.
- **208 px reference rail + 312 px inspector at 1440 px**, slightly narrower than
  the initial 224/320 proposal, to give the image more room. At 1920 px these
  become 224/328. At 1024 px the inspector is 300 px and the canvas area is 724 px.
- The filmstrip spans the reference rail and canvas, while the inspector retains
  full height. At narrow widths a rail toggle temporarily replaces the inspector.
- Exposure, white balance, and tone are one expanded group. Other groups collapse
  and the inspector scrolls independently. Control rows align labels and values.
- Crop and Masks replace the global controls in the same inspector. The visible
  Done/Apply/Cancel controls return to global editing.
- Library batch selection displays mixed ratings rather than a misleading single
  value. Virtual copies are labeled separately from their originals.
- Agent proposals have an explicit Apply/Discard step, still a proposed behavior
  change. Applying the sample proposal creates one simulated undo entry.
- Export options and the output summary share one dialog; long-running delivery
  is represented by Jobs. The actual format list must come from engine capability
  discovery during implementation.

## Component specification represented by the mockups

| Component | Visual/interaction specification |
| --- | --- |
| Workspace tab | Text label, 2 px active underline, no filled tab container |
| Tool button | 26–28 px minimum height, 14–16 px icon with visible name, quiet active fill |
| Adjustment row | 12 px label/value, editable numeric field, 2 px slider track and 9 px knob; stable reset slot |
| Group header | 39 px row, disclosure marker, optional modified dot; one divider between groups |
| Photo tile | Neutral fit-inside image area, filename/rating/copy label inside border; selection check and outline |
| Mask row | Coverage thumbnail, name/type, selected border; inspector below applies to selected mask |
| Dialog | Restrained border/radius, clear title, grouped fields, output summary, Cancel and primary action |
| Status | 22 px footer for saving, selection and actual display mode; detailed errors need a persistent state |
| Focus | 2 px contrasting outline on native interactive elements |

Typography uses a system sans fallback. The prototype tests the hierarchy and
density; actual Qt font metrics, accessibility semantics, and screen-scale
behavior still need validation. The small metadata labels are deliberately
secondary; no control depends on color alone.

## Interactive coverage

Try these routes:

1. **Develop:** type an exposure value or move its slider; Undo restores it.
   Search adjustments, open groups, switch photos, collapse the rail/filmstrip.
2. **Library:** search the sample set, filter four-star photos, change sort,
   Ctrl/Cmd-click multi-select, rate the selection, double-click to Develop.
3. **Masks:** select Sky gradient, toggle the overlay, drag the center handle.
4. **Crop:** drag the rectangle or lower-right handle, change aspect/guide,
   cancel or simulate acceptance.
5. **Agent:** Apply/Discard the sample proposal; applying is one undoable change.
6. **Export:** switch presets/formats, inspect dependent fields and output naming,
   then simulate completion in Jobs.

Keys: G = Library, D = Develop, Ctrl/Cmd+Z = Undo, backslash = before/after.
Text entry keeps its normal keyboard behavior. Escape exits an active tool or
closes the export dialog. Enter accepts the crop.

## Deliberate simulation limits

- The four photographs are JPEG samples. RAW filenames, capture settings, camera
  metadata, agent messages, and usage figures are fictional design fixtures.
- The histogram is computed from the source JPEG and stays fixed. Exposure,
  contrast, saturation, before/after, and zoom use approximate browser effects;
  this is not evidence of RAW, color-management, HDR, or actual-pixel correctness.
- Mask dragging/overlay and crop geometry are presentation interactions. Their
  coordinates do not round-trip through PhotoDoc. Crop acceptance does not crop
  engine pixels; the sample mask opacity/feather controls do not run a renderer.
- The radial mask entry shows selection hierarchy; this pass demonstrates only
  linear-mask handle movement. The overlay is illustrative rather than measured
  mask coverage. Rotate/flip are temporary visual demonstrations.
- Export performs no filesystem writes; Agent makes no model requests. Sample
  export formats/options illustrate layout, not the complete supported inventory.
- Buttons outside the implemented routes identify their scope with a preview
  notice. Retouch, compare, sync, saved-search editing, full metadata editing,
  error/recovery screens, and detailed settings still need design work. They have
  not been dropped from the [parity inventory](../08-qt-workflow-parity.md).
- Full redo/history traversal, persistent tool coordinates, pointer capture
  cancellation, and gesture rollback are requirements for the Qt slice, not
  implemented guarantees of this presentation prototype.

## Validation performed

Chromium render and interaction checks on 2026-11-07:

- All six states rendered at 1440×900, 1024×700, and 1920×1080 app sizes without
  horizontal page or inspector overflow and without JavaScript runtime errors.
- Inspected screenshots for Develop, Library, Masks, Crop, Agent, Export and the
  laptop layouts. Fixed thumbnail metadata escaping the selection border and
  incorrectly nested metadata controls. Screenshots above reflect the fixes.
- Checked exposure input/undo, library search/multi-selection and mixed ratings,
  mask overlay/movement, crop resizing/cancel, agent apply/undo/discard, and
  export preset dependencies/simulated job completion.
- The mockup runs offline from local assets. No native application build or engine
  tests were run, since this pass changes design artifacts only.

Still unverified: native Qt layout/input, 150%/200% device-scale behavior, Chinese
text, screen readers, performance, HDR and display transitions, and every baseline
workflow. Passing the mockup checks does not close the replacement gate.

## Original review questions

1. Does this density feel right, particularly the light/WB group and the left
   scope/history rail? The next iteration should adjust geometry before adding
   more screens.
2. Is the full-height inspector with the filmstrip only beneath the canvas the
   right balance?
3. Should the Agent keep the proposed explicit Apply/Discard step?

The user approved this visual design and requested its Qt implementation.
Remaining tool, settings, and failure states still need review. Gate A's runtime
baseline audit remains open.

## Sample assets

Downloaded on 2026-11-07 from Unsplash for design review under the
[Unsplash license](https://unsplash.com/license). Source image endpoints:

- [Mountain valley](https://images.unsplash.com/photo-1464822759023-fed622ff2c3b)
- [Forest](https://images.unsplash.com/photo-1441974231531-c6227db76b6e)
- [Deer and mountain](https://images.unsplash.com/photo-1472396961693-142e6e269027)
- [Sunset landscape](https://images.unsplash.com/photo-1500534623283-312aade485b7)

These are external sample assets, not the user's photographs. The duplicated
thumbnails illustrate virtual copies. Icons are copied from the existing
`apps/aigle/assets/icons` collection, preserving the existing project's design
source. No new icon system or generated imagery was introduced.
