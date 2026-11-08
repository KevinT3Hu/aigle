# Platform boundaries and Windows status

The engine and current Qt GUI are not Windows-supported. This refactoring does
not add a Windows port. The managed native dependency build currently accepts only
Linux triplets; parser isolation deliberately produces a Windows compile-time
"not implemented" error instead of silently running untrusted parsers in-process.
The legacy GPUI frontend has been removed; Qt is the only desktop frontend.

## Ownership

- `aigle-core` and `aigle-engine-api`: platform-neutral image/edit types and APIs.
- `aigle-engine/src/platform`: computation's OS resources (RAM/cgroup/pressure
  discovery, filesystem capacity, and spill-buffer backing storage). GPU integration
  stays in the renderer backend. No window system, D-Bus or credential-store code.
- `aigle-paths`: application directory policy, extracted from the engine. Explicit
  `AIGLE_CACHE_DIR` and `AIGLE_DATA_DIR` overrides work independently of platform;
  native Windows defaults are TODOs and return no default directory.
- `aigle-isolate`: a platform-selected worker transport. Unix IPC and descriptor
  passing live in `unix.rs` and its blob module. Windows IPC is a deliberate build
  blocker until implemented.
- `aigle-qt-bridge/src/platform`: GUI monitor-profile discovery. The colord/D-Bus
  backend is selected only for Linux/FreeBSD. The Windows backend returns an
  explicit unsupported error; a user-specified ICC file still bypasses detection.
- `aigle-qt-bridge/src/credentials`: separate Secret Service and native-keyring
  backends. Existing Windows/macOS keyring code is retained, not replaced by a stub.
- `apps/aigle/src/platform.hpp`: short Qt desktop startup differences, selected
  with `Q_OS_*` macros. Substantial future backends should use separate source files.

Missing Windows resource APIs have TODOs with safe fallbacks: conservative RAM,
no pressure sample, unavailable disk capacity, and unsupported spill allocation
(the existing caller falls back to heap storage). Native image-library filename
adapters return explicit errors until their Unicode path APIs are implemented.
Catalog path decoding is explicitly unimplemented; removable-volume discovery
returns no volumes until a native backend exists.

Use Rust `#[cfg]`, not `cfg!`, to exclude unavailable OS APIs from compilation.
Select whole backend modules when implementations differ substantially; keep short
platform differences local. Portable Qt/Vulkan rendering code does not need OS
branches merely because it is native code. Linux deployment settings are guarded
in CMake. A future Windows port still needs native dependency triplets, worker IPC,
Unicode file/catalog paths, resource APIs and DLL/installer deployment, followed by
native build and runtime tests.
