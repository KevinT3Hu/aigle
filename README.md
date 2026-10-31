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
