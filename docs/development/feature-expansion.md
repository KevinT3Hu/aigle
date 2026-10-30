# Feature expansion status

Requested scope: the app-wide feature review, excluding workspace customization and
capture/tethering workflows. This is an implementation record, **not a claim that the
entire requested scope is finished**.

## Implemented stages

- Range masks: add luminance masks or pick a scene-linear color from the image;
  adjust bounds, tolerance and feathering. Combine another mask's coverage with
  Add, Subtract or Intersect. Coverage is copied at that moment, including inversion;
  the other mask's adjustments remain independent. Opacity is not copied. Deep
  combinations exceeding the kernel's eight-value stack are refused.
- Library text search: case-insensitive terms across names, paths, keywords, IPTC,
  labels, camera, lens and capture time. All terms must match; `-term` excludes a
  term. Metadata results update as sidecars and EXIF arrive. Save/load/delete filters
  and sort order with the controls beside the search input. Saved searches apply to
  the current folder or album; they do not search every mounted disk.
- Comparison: View → Compare selected photos opens two to four selected photos in
  a survey. Zoom/pan can be linked; centers are normalized to each crop, with the
  same pixel magnification. Each candidate can be rejected or removed from the
  comparison without deleting its original. Escape closes the survey.
- Export queue: the toolbar opens a persistent per-file queue. Pause stops at a
  file boundary; Resume continues; Retry requeues failures. Pending entries can be
  cancelled. Completion and errors remain visible. Restarted queues begin paused;
  interrupted files with existing output require inspection. Existing output is
  refused instead of overwritten. Queue storage is `export-queue.json` in the app
  data directory.
- Albums: the library's Albums menu creates an album from the selection, opens it,
  and adds/removes originals. Albums span folders and include those originals'
  virtual copies. Deleting an album keeps the files. Membership follows moves made
  through Aigle. Missing members stay listed so they can be relinked.
- Exact duplicates: the copy icon in the library bar scans the current listing in
  the background, grouping by size before streaming BLAKE3 fingerprints. Virtual
  copies are deduplicated by path before scanning. Click again to cancel or show
  all. Results identify identical files, not visually similar photos; no file is
  automatically deleted.
- HDR mastering: the Basic panel exposes diffuse white (80–400 nits), peak
  brightness (400–1000 nits), and highlight roll-off. These are non-destructive,
  undoable per-photo settings shared by native preview and HDR export, and are
  included in presets and selective settings copying. SDR rendering ignores them.
  The peak range stays within the current native preview's negotiated mastering
  range. Internal transport remains linear Rec.2020 with 1 = 203 nits.
- Paired delivery: HDR export can also enqueue an independently rendered SDR sRGB
  JPEG with a `-sdr` suffix, avoiding already reserved destination names. It uses
  the existing SDR rendering pipeline rather than embedding a gain map.
- Catalog recovery: File → Back up catalog writes a consistent SQLite snapshot.
  Restore validates the schema and database, asks for confirmation in the app, and
  creates a recovery snapshot first. **Catalog backups include history, virtual
  copies and albums; they do not include originals or XMP sidecars.**
- Missing originals: File → Locate missing original relinks the selected missing
  album member to an existing photo. Surviving sidecars are copied without
  overwriting destination sidecars, and history/copies/albums follow the new path.
  Existing destination catalog edits cause a conflict instead of being replaced.
- Precision crop: the line icon in the crop controls lets the user draw along a
  horizon or vertical edge. Rotation snaps that line to its nearest axis as one
  undoable edit. Escape cancels.

## Still outstanding

These remain part of the requested scope, not completed features:

- Independent pick/reject flags and related-shot stacks.
- Subject, sky and background inference masks; editable combined-mask components.
- Bulk moved-folder relinking, persistent offline image previews, and a backup/
  restore workflow that includes originals and XMP sidecars.
- AI denoise, improved per-camera noise profiles and RAW-domain chroma reduction.
- Aligned exposure-bracket HDR merge, panorama stitching and focus stacking.
- Guided perspective correction and targeted on-image tone/color adjustment.
- Custom printer/paper ICC proofing, rendering-intent controls and print layouts.

Workspace customization and capture workflows are deliberately excluded.

## Validation

Library tests cover album persistence/moves, exact duplicate grouping/cancellation,
backup/restore integrity and relinking conflicts. App tests cover search matching,
saved criteria, mask combination serialization/inversion, export recovery/output
collisions and straightening geometry. HDR pipeline tests compare exported PQ PNG
against the native preview's engine pixels with non-default mastering controls;
they do not measure the physical monitor's output.
