# Overlay patch inventory

Source changes are applied by `PATCHES` in each `portfile.cmake`, to SHA512-verified
upstream archives. Build recipes/options are separate CMake files. No modified
LibRaw/librtprocess source tree is checked into the project anymore.

| Patch | Purpose |
| --- | --- |
| `libraw/0001-nikon-buffered-entropy.patch` | Buffered Nikon entropy reads, local bit state and paired horizontal predictors. |
| `librtprocess/0001-cancellable-demosaic.patch` | AMaZE and Markesteijn cancellation. |
| `librtprocess/0002-rcd-parallel-correctness.patch` | RCD padding initialization, bounded teams and cancellation. |
| `librtprocess/0003-cancellable-deterministic-ca.patch` | CA race avoidance, deterministic sums, cancellation, bounded teams and matching public declarations. |
| `libvips/0001-disable-fuzz-build.patch` | Omit upstream fuzz executables from the dependency build (extracted from the stock vcpkg recipe's inline replacement). |

The LibRaw and librtprocess ports' `verify-sources.cmake` files record the exact
post-patch source SHA256 values from before migration. The patch-series names
separate the major algorithms; some API changes are shared in `librtprocess.h`.

The libvips overlay is derived from the stock port at the revision in
`../vcpkg-revision`, with extra codec features and the explicit patch above.
The other overlays are project-owned recipes. Third-party source copyrights
remain with their upstream authors; vcpkg installs their license notices into
`share/<port>/copyright`.

The rationale and measured patch necessity are in
[`docs/development/native-dependencies.md`](../../docs/development/native-dependencies.md).

## License of the derived vcpkg recipe

The libvips recipe is derived from Microsoft vcpkg under the following license:

```text
MIT License

Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify,
merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be included in all copies
or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE
OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```
