# GPUI fork and submodule

`gpui/` is a Git submodule of
[KevinT3Hu/aigle-gpui](https://github.com/KevinT3Hu/aigle-gpui), forked from
[zed-industries/zed](https://github.com/zed-industries/zed). All three locally
patched crates share this upstream, so one submodule keeps them on one baseline.
Cargo's `[patch.crates-io]` entries use:

| Published crate | Submodule path |
| --- | --- |
| `gpui-pre` | `gpui/crates/gpui` |
| `gpui-pre-linux` | `gpui/crates/gpui_linux` |
| `gpui-pre-wgpu` | `gpui/crates/gpui_wgpu` |

The current branch is `aigle/gpui-0.3.10`, based on Zed
`08cbb88a152c3751a43e853425a70e9f631a5918`. The parent repository's gitlink pins
its exact commit; updating the branch remotely does not silently change builds.
The code retains its upstream Apache-2.0 licence.

## Initial checkout

```sh
git submodule update --init --recursive
```

No patch application or generated source directories are needed. The checked-out
submodule already contains the committed HDR changes. Each patched crate has an
independent Cargo workspace and the matching published `gpui-pre` manifest, so
Cargo does not need to resolve or build the rest of Zed.

The fork is the Zed monorepo. To keep its working tree small after initialization:

```sh
git -C vendor/gpui sparse-checkout init --cone
git -C vendor/gpui sparse-checkout set crates/gpui crates/gpui_linux crates/gpui_wgpu aigle
```

## Upgrade a baseline

The fork keeps published Cargo packaging separate from two focused HDR commits:

* Native surface ownership and hole-painting APIs in GPUI and Wayland.
* Replacement blending for those holes in the WGPU renderer.

Its `aigle/baseline.json` records the snapshot version, upstream revision, and
verified crate archive hashes. `aigle/gpui-VERSION-base` tags mark the packaging
baseline. The commits after each tag are the HDR patches to replay.

Follow [`gpui/aigle/README.md`](gpui/aigle/README.md) to import the next published
snapshot on a new versioned branch with `aigle/import_snapshot.py`, then
cherry-pick those HDR commits. The script refuses to overwrite a patched
baseline. Do not force-push a branch already pinned by this project.

After the fork's new branch and baseline tag have been pushed, in this repository:

```sh
git -C vendor/gpui fetch origin aigle/gpui-NEXT_VERSION
git -C vendor/gpui switch --detach FETCH_HEAD
git config -f .gitmodules submodule.gpui.branch aigle/gpui-NEXT_VERSION
# Update the gpui-pre-* and compatible gpui-component/assets Cargo.toml pins.
cargo update
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
# Also smoke-test the editor and Wayland HDR canvas.
git add .gitmodules vendor/gpui Cargo.toml Cargo.lock crates/aigle/app/Cargo.toml
```

Review and commit the parent changes only after validation. `git submodule status`
and `git -C vendor/gpui log --oneline` show exactly which baseline and patches a
checkout uses. Historical crate patch files remain available in parent Git history;
active patches now live in the fork's commit history instead of duplicate files.
