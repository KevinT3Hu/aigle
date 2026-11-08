# Aigle Qt editor

The default editor is C++23 and Qt 6 Widgets. Rust owns decoding, processing,
document validation, history, sidecars, catalog data and agent execution. Qt owns
the desktop UI and the image presentation surface. Neither Qt nor GPUI patches
are needed to build this frontend.

The workspace implements the approved photo-first design in native Qt Widgets:
neutral dark panels, a generous Vulkan canvas, compact controls, Library/Develop
modes, a full-height inspector, and a filmstrip beneath the canvas/reference rail.
The reference rail and filmstrip collapse on smaller windows. See the
[design review](../../docs/design/qt-review/README.md) and
[native captures and remaining differences](../../docs/design/qt-review/native/README.md).

## GUI data boundaries

Use named C++ value types for communication between GUI components and the
engine worker. Commands use the `EngineCommand` variant, replies use
`EngineReply`, and polling updates use `EditorState`. Domain records cover
viewport, scopes, geometry, retouch, masks, library selection, agents and export
jobs. Keep JSON conversion in the `*_json.hpp` adapters at wire and persistence
boundaries; add a typed command or record when extending an internal interface.

JSON remains appropriate for runtime schemas and the arbitrary adjustment
parameter documents those schemas describe. These documents are intentionally
dynamic; fixed metadata and control messages should use typed fields instead.

## Build and run

Prepare the managed native dependencies as described in `native/README.md`.
Install CMake 3.25+, Ninja, a C++23 compiler, Vulkan development files, and Qt
6.11+ development packages for Widgets, Gui, GuiPrivate, Svg, ShaderTools and Test.
The GuiPrivate package supplies Qt's documented QRhi headers. Match its version
to the Qt runtime; QRhi has limited compatibility guarantees across Qt releases.

```sh
cmake -S . -B target/qt -G Ninja -DCMAKE_BUILD_TYPE=Debug
cmake --build target/qt --target aigle-gui --parallel
target/qt/apps/aigle/aigle-gui /path/to/photo.NEF
target/qt/apps/aigle/aigle-gui --synthetic
target/qt/apps/aigle/aigle-gui --hdr /path/to/photo.exr
target/qt/apps/aigle/aigle-gui --remote https://photos.example.com PHOTO_ID
```

The Qt application is built and launched directly with CMake. Corrosion invokes
Cargo to build the Rust engine bridge. Thesauros checkout uses the existing
`THESAUROS_TOKEN` environment variable.

File and folder dialogs use the system picker. On Linux, Aigle defaults to Qt's
`xdgdesktopportal` platform theme; install the Qt desktop portal plugin,
`xdg-desktop-portal`, and a portal backend for your desktop. An explicitly set
`QT_QPA_PLATFORMTHEME` is respected. Qt falls back to its own dialog if native
desktop integration is unavailable.

CMake integrates the Rust bridge through Corrosion v0.6.1, fetched with
`FetchContent_Declare` from commit `1499b14e4906a2890f5cee1547c8848db261753d`
and a verified SHA-256 archive hash. Corrosion imports only the bridge's `cdylib`
target with `LOCKED` and the selected Cargo profile, and manages its build,
linking and installation. Rust artifacts live under the CMake build directory's
`cargo/` cache; the shared library is copied beside the Qt executable by Corrosion.

`Rust_TOOLCHAIN` defaults to the channel in `rust-toolchain.toml`. Corrosion
selects matching Cargo and rustc binaries, including when rustup is only in
`~/.cargo/bin`. Override it explicitly with `-DRust_TOOLCHAIN=...` when needed.
The bridge also uses the selected compiler when an editor or build environment inherits
a conflicting `RUSTC` environment variable.

The first CMake configure downloads Corrosion. To use a local copy for offline
builds, set `-DFETCHCONTENT_SOURCE_DIR_CORROSION=/path/to/corrosion` to an unpacked
copy of the pinned source.

If an earlier mixed-toolchain build reports incompatible `cc` or `pkg_config`
artifacts (E0514), run `~/.cargo/bin/cargo clean -p cc -p pkg-config` from the
repository root, then reconfigure/build CMake. This clears those Rust dependency
artifacts while preserving `target/native` and the other build caches.

For optimized builds, configure a separate `target/qt-release` directory with
`-DCMAKE_BUILD_TYPE=RelWithDebInfo -DAIGLE_RUST_PROFILE=release`. The installed
application is the C++ executable and `libaigle_qt_bridge.so`; Cargo is not needed
on the target machine. `cmake --install` installs the executable, bridge, camera
data, icon and desktop entry. `-DAIGLE_DEPLOY_QT=ON` additionally bundles matching
Qt libraries and plugins through Qt's deployment support. Bundling also requires
`patchelf` and the Qt WaylandClient development package on Linux.

## C++ editor tooling

The frontend's `.clangd` points to `target/qt/compile_commands.json`. CMake
exports this database automatically, so clangd uses the same C++23 flags, Qt
defines, QRhi headers and Rust bridge include directory as the build. No
machine-specific include paths or compilation-database symlinks are required.

Run the direct debug configure/build commands above once before opening the
frontend in a clangd-enabled editor; the build also generates Qt's MOC headers.
Reconfigure CMake after changing dependencies or build options. If you use a
different build directory, update `CompilationDatabase` in `.clangd` (relative
to `apps/aigle/`). See the [clangd configuration reference](https://clangd.llvm.org/config#compilationdatabase).

To check the configuration from the repository root:

```sh
clangd --check=apps/aigle/src/canvas.cpp
```

## Editing

The editor has library, adjustment, history, metadata and agent panels.
Choose an adjustment group or search by name. Numeric ranges, units, steps,
enum values and structured editors come from the Rust parameter schema. Each
control has a reset button. Tone curves have draggable spline control points.
Masks have a named list, on-canvas linear/radial handles, local adjustments,
range creation and coverage combination. Advanced collection data also remains
available through structured editors.

The canvas supports fit, actual pixels, cursor-anchored wheel zoom, drag pan,
before/after, clipping overlays, and HDR. The toolbar selects crop, neutral white
balance, healing, cloning, red eye, radial/linear masks and brush masks. Crop has staged corner/move handles, aspect locking, angle preview and explicit
Apply/Cancel. For healing/cloning, click to choose an automatic source or drag
from the target to an explicit source. Brush size is a percentage of the image's
long edge. Mask adjustments are edited in the Masks group. All edits go through
`PhotoDoc`, including agent proposals and undo/redo.

Sidecar autosave preserves existing metadata and refuses external-change
conflicts. History uses the existing `~/.local/share/aigle/catalog.sqlite3`.
Virtual copies remain in the catalog and never write their edits into the master
photo's sidecar. Photo actions can rename or delete the current copy; deletion
opens its master and removes the copy history. File → Library offers album
membership editing, rename, and delete. Album removal never deletes originals.
Library actions also stack/unstack photos, collapse stacks, scan exact duplicates,
rename/move/trash originals with sidecars, and locate missing originals. Duplicate
scanning only reports groups. File-action cancellation stops after the current
photo. Restore trashed originals and sidecars to their original paths to recover
catalog associations; catalog history/copies are retained. Snapshots, albums, presets, clipboard adjustments, metadata,
catalog backup, verified file import and asynchronous export use the existing
Rust services. Qt window layout and agent endpoint/model preferences use
`QSettings`. On Linux/FreeBSD, the Provider & model dialog stores API keys in the desktop
Secret Service keyring by default (using `oo7`). Uncheck storage for
a session-only key, or choose Remove saved key to delete the current profile/endpoint's credential.
Blank input preserves an existing saved key. Keys are scoped to the profile ID and normalized
provider endpoint and are not stored in Qt settings or returned by bridge replies.
Agent runs prefer a supplied session key, then `AIGLE_AGENT_API_KEY`, then the
saved key. Keyring failures are reported without a plaintext fallback. Other
platforms currently support session/environment keys only. The agent's pending result is previewed in the canvas
and committed only by Apply edits; stale overlapping edits are rejected by the
document conflict checks.

Agent transcripts persist per original path and virtual-copy identity in Qt settings.
Switching photos or restarting restores the transcript; follow-up requests include
prior user and completed assistant messages. Transient progress and API keys are
not written to the transcript. Conversations currently do not follow renamed or
moved originals. The provider selector keeps named profiles with independent endpoint/model choices
and session keys. Use New provider to create a profile, Save to commit, and Cancel
to discard dialog changes. Remove profile deletes its saved keys (including keys
for earlier endpoints) before removing the profile; a keyring error leaves it
available for retry. Model selections and discovered model lists persist per
profile. Discover models queries the provider's OpenAI-compatible `/models`
endpoint in the background. Failures leave manual model entry available; model
lists do not certify image support. Cancel detaches discovery, ignores late
results, and leaves settings unchanged. HTTP discovery has a 20-second timeout;
a detached request may finish in the background. Secure credential storage on
platforms other than Linux/FreeBSD remains unfinished.

Qt is the sole GUI. The legacy GPUI frontend and its vendored fork have been removed.
There are no existing users to migrate, so legacy settings, credentials, and
saved-search migration are outside scope.

Library search supports metadata terms and exclusions, detailed filters, and
filename/rating/capture/modified sorting. **Manage saved searches…** saves all
criteria in Qt settings. Specialized workflows such as complete preferences and exact live mask guides
under nonlinear transforms still need implementation and verification in Qt. General parameter
editors do not imply interaction parity with every old tool.

## Scopes

The Develop reference rail provides RGB/luminance histograms, separate channels,
cumulative luminance, waveform, RGB parade and vectorscope. Log changes density
scaling; R/G/B control channel visibility. Gamut statistics support sRGB, Display
P3 and Rec.2020. Region enables an SDR analysis thumbnail; click a sample cell or
drag a rectangle, then use Whole crop to restore exact full-crop histogram bins.
Spatial/region statistics use at most 128×128 samples and exclude transparency.

Focus the graph and use arrows/Home/End for measurements. On the region thumbnail,
arrows move the selection, Shift+arrows resize, Home resets and Escape cancels a
drag. Analysis never changes the document. HDR scales are PQ-normalized to 1000
cd/m², with a 203 cd/m² diffuse-white guide. See [scope semantics](../../docs/development/scopes.md)
for clipping/gamut limitations and the difference between analysis and HDR preview.

## Preferences

**Edit → Preferences…** configures subfolder scanning, thumbnail width, default
HDR canvas, and the autosave interval. Save applies changes immediately and keeps
them across launches; Cancel discards them. Restore Defaults is staged until Save.
HDR falls back to SDR on unsupported displays. Autosave remains enabled.

## Keyboard shortcuts

**Edit → Keyboard shortcuts…** edits the existing main-window bindings, including
multi-key chords. Clear or reassign conflicts before Save. Cancel leaves active
bindings unchanged; Restore Defaults is staged until Save. Text fields retain
normal typing and standard editing shortcuts. Comparison-dialog shortcuts are
currently fixed; unbound commands are not yet exposed in this editor.

## Catalog recovery

**File → Library → Back up catalog…** saves history, virtual copies, and albums.
**Restore catalog…** validates the chosen backup, saves current edits, creates a
recovery backup, and closes the open photo before replacing the catalog. Active
jobs must finish first. The result shows the recovery file; choose it in Restore
catalog to return to the previous catalog. Originals and XMP sidecars are separate
and are never replaced by catalog restore. Export jobs are stored separately.

## Copying and syncing adjustments

**Edit → Copy selected adjustments…** (Ctrl+Shift+C) chooses groups to capture.
**Paste selected adjustments** (Ctrl+Shift+V) replaces those groups on the open
photo as one undo step. Geometry, masks, and retouching are excluded by default.

For multiple targets, open the source photo, select target photos in Library,
then choose **Edit → Sync selected adjustments to selection…**. The chooser
identifies the source and lets you review targets. Sync updates selected masters,
excluding the source path; unchecked groups and metadata are preserved. Failures
are reported per file. Open a target and Undo to revert its sync step.

## Comparing photos

Select 2–4 photos in Library and choose **File → Library → Compare selected photos…**.
Scroll or use plus/minus to zoom, drag or use arrow keys to pan, and press F to fit
all panes. Link zoom and pan shares relative magnification and normalized position.
Pick/reject changes the master photo's flag; Remove from comparison only removes
its pane. The view renders each visible region from the full-resolution edited source.
Press **1** or **Actual pixels** for one source pixel per display pixel. Comparison
presentation is SDR sRGB; use Develop for HDR inspection.

## Export jobs

**Jobs → Export jobs…** lists saved exports with their destinations, status, and
errors. Pause finishes the current photo and holds the rest. Resume starts pending
jobs. Retry requeues a failed/cancelled/interrupted job; **Retry to new file…**
changes its destination without replacing an existing file. Cancel all stops the
active export and marks pending jobs cancelled.

The queue saves source references, edit snapshots, and export options atomically
in `~/.local/share/aigle/qt-export-queue.json` (respecting `XDG_DATA_HOME`). A lock
prevents two windows from modifying it. Restart always leaves the queue paused;
interrupted jobs require explicit retry. Sources and external assets must remain
available. Processing shows per-file status rather than encoder percentages.

## Presentation and ownership

`Canvas` embeds a `QWindow` in the Widgets UI and uses upstream QRhi/Vulkan. It
selects HDR10/PQ or extended linear scRGB only when the actual surface supports
it, otherwise it requests SDR frames. Engine HDR buffers remain RGBA16F linear
Rec.2020 (1.0 = 203 nits); a shader performs presentation encoding. UI widgets
stay on their normal SDR surfaces. Moving between screens renegotiates the
swapchain. Display HDR settings remain under the desktop compositor's control.

The engine QObject and its Rust editor live on a dedicated `QThread`. Requests
are queued; viewport requests are coalesced on the GUI thread. Rendered frames
cross as immutable reference-counted buffers, with explicit Rust release
functions and C++ RAII deleters. Frame data does not pass through JSON. Parser
workers branch before QApplication initialization, retaining crash isolation.
Current image transport is CPU memory plus texture upload, not GPU texture
sharing. There is no dependency on the legacy transparent-hole renderer.

The bridge's ABI version is 1. `aigle_command` takes UTF-8 JSON and returns an
owned UTF-8 JSON envelope: `{"ok": value}` or `{"error": message}`. Free every
response with `aigle_string_free`. Commands include open/close, state, schema,
edit, undo/redo/goto, viewport/overlays, tool/tool_mode, save, metadata, export,
import, agent start/accept/reject/cancel, snapshots, copies, albums and remote
checkout. `edit.ops` uses the existing serialized `ParamOp` format. Opaque editor
handles are confined to their owning worker thread. Immutable frame handles may
outlive that editor. Rust panics are contained at fallible C entry points.

## Verification

```sh
cargo test -p aigle-qt-bridge -p aigle-agent -p aigle-core
ctest --test-dir target/qt --output-on-failure
target/qt/apps/aigle/aigle-gui --smoke-test
target/qt/apps/aigle/aigle-gui --smoke-test --hdr
```

The native smoke test needs a working window-system/Vulkan presentation stack.
It renders a synthetic photo, edits exposure, undoes and redoes it, and renders
before/after. The HDR variant logs whether HDR presentation was negotiated; an
SDR fallback is reported explicitly. For SDR framebuffer capture, set
`AIGLE_SMOKE_CAPTURE=/tmp/canvas.png`; it also writes a composite window capture
at `/tmp/canvas.png-window.png`. The bridge tests exercise export, sidecar and
catalog round trips, SDR/HDR frame formats and buffers surviving editor teardown.

## Credential verification

The Qt widget tests cover keyring errors, retries, request correlation and keeping
API keys out of settings. To test a real Secret Service round trip without using
your desktop keyring, run from the repository root:

```sh
apps/aigle/tests/keyring_round_trip.sh
```

This requires `dbus-run-session`, `gnome-keyring-daemon`, `gdbus`, and `rg`. It
creates a private session bus and temporary keyring, and exercises missing-key
lookup, storage, replacement, profile/endpoint isolation, removal across endpoints,
and compatibility with the earlier Qt endpoint-only keys. The Rust test is
ignored during normal test runs because it requires this isolated service.

## Monitor profiles

Preferences offers Auto or sRGB for SDR presentation. Auto detects the current
monitor through colord on Linux/FreeBSD, validates and imports its ICC profile,
and falls back to sRGB with the detection error available in the display status
tooltip. `AIGLE_DISPLAY_ICC=/path/to/monitor.icc` overrides automatic detection.
Moving the canvas to another screen triggers discovery again. HDR presentation
uses its HDR output encoding rather than the SDR monitor profile.

Agent conversations use catalog photo identities, so moves and relinks performed
by Aigle preserve conversations, with separate conversations for virtual copies.
Catalog schema 4 adds these identities; opening an existing catalog upgrades it.
Older-schema backup files are rejected explicitly by the restore validator.
