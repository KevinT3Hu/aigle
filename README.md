# Phaos

**Pháos** (φάος), Ancient Greek for “light”, is the shared Rust workspace for
**Aigle**, the RAW photo editor, and **Thesauros** (θησαυρός, “treasury” or
“storehouse”), its RAW library server.

Aigle brings out an image's light; Thesauros keeps the originals and their edits.

## Workspace

- `crates/aigle/`: crates used only by Aigle: `app`, `cli`, `engine`,
  `kernels`, `lensfun`, `rtprocess-sys`, `vips-sys`, and `xtask`. The separate
  nightly SPIR-V builder is in `crates/aigle/kernels-build`.
- `crates/thesauros/server`: Thesauros RAW library server and embedded web UI.
- Direct children of `crates/` are shared by both applications: `core`, `doc`,
  `engine-api`, `exiv2-sys`, `isolate`, `library`, `libraw-sys`, and `remote`.
  The remote crate also provides the `thesauros-client` checkout/sync tool.

Aigle's rendering implementation is separate from the shared engine interface;
Thesauros uses RAW previews and metadata without depending on that renderer.

All workspace members share `Cargo.lock`, `target/`, dependency versions, and
build profiles. The SPIR-V builder retains its separate nightly workspace.
The checkout formerly named `aigle` now lives at `~/code/phaos`.

Native image-processing libraries are pinned and built with vcpkg. Install the
host build tools, then run `cargo xtask native-deps` (Aigle) or
`cargo xtask native-deps --server` (Thesauros only). See
[managed native dependencies](native/README.md) for prerequisites and patch ownership.

```sh
cargo xtask native-deps
cargo check -p thesauros -p aigle-remote
cargo test -p thesauros -p aigle-remote
cargo run -p aigle-app -- /path/to/photo.NEF
cargo run -p aigle-cli -- --help
cargo run -p aigle-app -- --remote http://127.0.0.1:8787 PHOTO_ID
```

See [Thesauros setup and API](crates/thesauros/README.md) for server usage,
imports, uploads, and editing server photos in Aigle. Existing design and
development documentation remains under `docs/`.

Thesauros uses Topcoat with HTMX and compiled Tailwind assets, a durable SQLite
job queue, isolated metadata workers, and session/CSRF protection. Production
configuration, a systemd unit, and an HTTPS proxy example are in
[`crates/thesauros/deploy`](crates/thesauros/deploy).

## Aigle editing agent

Open **Settings** from the toolbar or press **Ctrl+,**. The dialog groups general
preferences (language, display color management, and library subfolders), the
providers and models, and keyboard shortcuts. Changes are staged until Save; Cancel
leaves them unchanged.

Under **Providers & models**, add one or more named providers, each with an
OpenAI-compatible endpoint and API key. Use **Fetch models** to load that
provider's `/models` list and add the models you want, or enter model IDs
manually. Choose models supporting **image inputs and function/tool calling**;
a discovery listing alone does not guarantee those capabilities.

Each provider's key has a separate platform keyring entry (Secret Service on
Linux), restored at startup and excluded from the settings file. Clear a key and
Save to remove it; removing a provider also removes its saved credential. Existing
single-provider settings and keyring credentials are migrated automatically.
Keyring failures are shown without falling back to plaintext storage.

Open a photo in Develop and use the **Edit / Agent** switch at the top of the
right pane. Agent mode replaces the adjustment tabs with a chat; plots stay in the left sidebar.
Messages support Markdown, selectable text, editing profile cards, and inline
rendered previews in a compact layout. The bar below the chatbox switches the
active provider and model; the selected pair is remembered across launches.
Enter sends a message; Shift+Enter inserts a new line.

Describe the desired edit and press Send. The agent receives a rendered JPEG
(up to 1024 pixels on the long edge), the current editing parameters, and this
photo's chat. [Rig 0.43.0](https://docs.rs/rig-agent/0.43.0/rig_agent/) manages model
requests, tool dispatch, multimodal tool results, and the bounded agent loop.
Its `present_profile` tool accepts the full editing parameter schema: numeric
adjustments, toggles, selectors, crop and orientation, RGB/channel point curves,
local masks, healing/cloning spots, and red-eye corrections. Dotted paths select
fields; structured values and arrays replace whole groups or collections.
Each proposal starts from the original edit, so refinements must include all
changes the model wants to keep. `suggest_edits` exposes the editor's auto tone,
auto/picked white balance, lens matching, healing-source selection, and color
sampling helpers. `see_preview` renders the proposed edits through Aigle's engine
so the model can inspect and refine the result. A Rig request hook sends preview images as base64 JPEGs in
following `user` messages, keeping tool acknowledgements text-only. This matches
Chat Completions endpoints such as DeepSeek that accept images only in user
messages; Rig continues to manage tool execution and the agent loop. A run is limited to 12 model requests, with a 120-second timeout
per request. The endpoint must support vision; models that reject image input or
finish without calling the preview tools fail without applying edits.

The completed profile is applied as one undoable agent edit. Intermediate
profiles do not change the document. Editing the photo during a run discards its
result. Cancel drops the local agent run and prevents applying the result; a provider
may continue processing a request it has already accepted. This edits existing adjustment controls; it does not
perform generative fill or synthesize image content.
