# Managed native dependencies

Run `cargo xtask native-deps` once before building Aigle, or
`cargo xtask native-deps --server` for the smaller Thesauros-only dependency graph.
Then use ordinary `cargo build`, `cargo test`, and `cargo run` commands.

The task bootstraps the exact vcpkg revision in `vcpkg-revision` under
`target/native/vcpkg` and installs into `target/native/installed/<triplet>`.
Build scripts only compile our C/C++ ABI shims. They require this private prefix
and reject missing/stale native dependencies rather than falling back to system
image libraries. They never download dependencies during `cargo build`.

Linux x86-64/AArch64 and Windows x64 MSVC triplets are defined.
The compiler, libc/C++/OpenMP runtime, platform graphics/windowing APIs, and build
tools still come from the host. This is dependency management, not a hermetic
compiler toolchain. Aigle's Qt frontend requires the host Qt and Vulkan development packages.
Install Git, a C/C++ compiler, pkg-config, CMake, Ninja, Python 3, curl,
zip/unzip, tar, Autoconf/Automake/Libtool, Bison/Flex and NASM.
vcpkg may fetch additional pinned build tools. No root installation is performed.

Native libraries are static, PIC and release-optimized even for Cargo dev builds.
Codec modules are linked into libvips rather than relying on runtime system
plugins. SQLite remains bundled by its existing Cargo dependency. Font files and
Fontconfig configuration remain runtime data supplied by the host.

## Overlay ownership

| Port | Reason for overlay |
| --- | --- |
| `libvips` | Extend pinned upstream port with required HEIF/AVIF, JPEG XL and Ultra HDR features; retain SVG watermark decoding. |
| `libultrahdr` | Missing from the pinned stock registry; enable ISO and XMP metadata. |
| `libraw` | Pin 0.22.2, apply the Nikon patch, preserve serial reentrant decoder build. |
| `librtprocess` | Missing stock port; preserve cancellation, deterministic parallel processing and selected-source OpenMP. |

All source modifications live in named `.patch` files in `ports/`. The original
source is fetched at a pinned upstream revision and verified using SHA512.
`verify-sources.cmake` additionally checks that the patch series reconstructs the
exact source files that were previously tested in the project. Custom
`CMakeLists.txt` files are build recipes, separate from algorithm changes.
Source verification normalizes CRLF to LF before hashing on all platforms.

`librtprocess` enables OpenMP only for `rcd.cc` and `CA_correct.cc` on GNU.
AMaZE, Markesteijn, borders and highlight reconstruction remain serial. The
separately compiled LibRaw CFA shim retains its bounded OpenMP implementation;
LibRaw's decoder files remain serial. Float/optimization settings are explicit.
Do not replace this with upstream's blanket OpenMP option.

## Updating dependencies

1. Update the pinned vcpkg revision and matching `builtin-baseline` together.
2. Rebase affected overlay recipes and named source patches. Record a new upstream
   revision and archive SHA512 when changing a library version.
3. Update the patched-source hashes only after reviewing the resulting diff and
   rerunning regression tests; they are not hashes to bypass mechanically.
4. Run `cargo xtask native-deps` again. Local overlay/triplet/manifest changes
   invalidate the Cargo build stamp. vcpkg's ABI cache accounts for recipe inputs.
5. Run native cancellation/determinism, real RAW, metadata and HDR/codec tests.

vcpkg's default binary cache is reused between runs. CI caches that directory.
Intermediate native builds are cleaned to keep disk use bounded. Windows keeps
downloaded archives so ports can reuse their shared MSYS build tools.
Project Git pins overlays; the vcpkg registry baseline pins stock ports. Use a
custom registry later if the ports need versioned reuse across repositories.

## Windows x64

Install Rust (the workspace toolchain is selected by `rust-toolchain.toml`),
Git for Windows, and Visual Studio with Desktop development with C++ and a
Windows SDK. Run `cargo xtask native-deps` from PowerShell at the repository
root. The task uses the Windows vcpkg bootstrap and builds static libraries
with the dynamic MSVC runtime. Its managed pkgconf executable is used by Cargo;
no system pkg-config installation is required.

For the desktop frontend, also install MSVC Qt 6.11+ with Widgets, GuiPrivate,
Svg, ShaderTools, Test and LinguistTools, plus CMake and Ninja. The native
dependency task supplies Vulkan headers and the loader on Windows.
From a Visual Studio developer shell with Qt on PATH:

```powershell
cargo xtask native-deps
cmake -S . -B target/windows-qt -G Ninja -DCMAKE_BUILD_TYPE=Debug -DCMAKE_PREFIX_PATH=C:/Qt/6.11.3/msvc2022_64
cmake --build target/windows-qt --target aigle-gui --parallel 4
windeployqt --debug target/windows-qt/apps/aigle/aigle-gui.exe
```

Windows parser workers use anonymous pipes, including cancellation and crash
recovery. Pixel replies use owned buffers rather than Linux memfd mappings.

See [the patch audit](../docs/development/native-dependencies.md) for decisions,
measurements and known regression-test limitations.
