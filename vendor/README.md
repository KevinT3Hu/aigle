# GPUI native canvas patches

These are crates.io sources for `gpui-pre`, `gpui-pre-linux`, and
`gpui-pre-wgpu` version **0.3.9**, based on Zed revision
`93d745668805029476e6d7fbb19652116fed51de`.
Upstream: https://github.com/zed-industries/zed . Each crate retains its Apache-2.0
license. The workspace patches crates.io to these local copies.

Local changes:

* Expose an optional owned native-surface owner through PlatformWindow/Window;
  Wayland supplies a cloned Connection and parent WlSurface.
* Add a native-surface-hole background and Window paint method.
* Draw those holes in order with replacement blending in the WGPU renderer,
  preserving content clipping and later GPUI overlays.

The application creates an input-transparent HDR subsurface below the parent.
GPUI still renders editor controls and handles input. No GPUI image-atlas or
8-bit image conversion is used for HDR canvas pixels.
