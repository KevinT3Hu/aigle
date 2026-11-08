The Qt frontend follows the approved photo-first layout in
`docs/design/qt-review/`: 28 px title row, 24 px menu row, 44 px application bar,
208 px reference rail, 312 px inspector, 116 px filmstrip and 22 px status row
at 1440×900. Dimensions are logical pixels and adapt to the window width.

`develop-layout.json` groups existing adjustment paths into Light & white
balance, Curves, Color, Detail, Optics & geometry, Effects, RAW & profiles,
and HDR mastering. Scalar types, ranges, defaults and steps come from the Rust
engine schema.

`icons/` retains the original SVG path data from `ui.rs`. `theme.qss` and
`src/theme.cpp` implement the approved neutral palette. Assets are embedded
through Qt resources, so installed builds do not depend on the source tree.
