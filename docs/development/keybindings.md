# Keyboard shortcuts

The Qt editor registers commands and their defaults in
[`window_shortcuts.cpp`](../../apps/aigle/src/window_shortcuts.cpp). Menus and
shortcuts invoke the same actions. Context checks keep canvas tool bindings from
intercepting typing and standard editing shortcuts in text fields.

Open **Keyboard shortcuts…** to inspect and edit command bindings. Shortcuts use
Qt key-sequence notation. The dialog validates overlapping bindings before Save;
Cancel discards edits. Customized bindings are stored in Qt settings under
`shortcuts/<command-id>`. The reload command reapplies saved bindings.

The legacy GPUI action registry and JSON keymap have been removed.
