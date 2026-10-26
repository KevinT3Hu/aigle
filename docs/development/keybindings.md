# Keyboard shortcuts

Commands are named GPUI actions, defined in `crates/app/src/actions.rs`. Menus and
keyboard bindings invoke the same actions; keyboard handling contains no command logic.
The bundled defaults are in [keymap.json](../../crates/app/keymap.json).

Use **View → Keyboard Shortcuts → Edit Keybindings…** to open the in-app dialog.
Search by command name, category, action ID, or shortcut (for example, `Ctrl+O`).
The **Modified Only** filter narrows the list to customized commands.

Select a command to edit its shortcuts. Type one or more shortcuts separated by
commas, or click **Record Shortcut**, press a shortcut or chord, then click
**Finish Recording**. Escape cancels recording and restores the previous input.
Use **Unbind** to remove all shortcuts for a command, **Reset to Default** to
restore one command, or **Reset All to Defaults** to restore the entire map.

The editor flags both duplicate shortcuts and chord-prefix conflicts. A binding
owned by another command changes only after **Replace Conflicting Bindings** is
chosen. **Assign Shortcut** stages a change; switching commands also stages a valid
pending edit. **Save** stages the selected edit, persists all changes, and applies
them immediately. **Cancel** or Escape closes without changing the active map or
saved file. The dialog traps focus and recording intercepts keys before app actions.

Preferences are stored at `$XDG_CONFIG_HOME/aigle/keymap.json`, falling back to
`~/.config/aigle/keymap.json`. Disabled default keys are saved explicitly, and the
file is replaced atomically after validation. A save error keeps the dialog open
and leaves the active map unchanged.

## Advanced configuration

The JSON file is optional for normal use. It can also be edited manually.
The file is a JSON object mapping a shortcut to an action name. For example:

```json
{
  "ctrl-o": null,
  "ctrl-p": "aigle::Open",
  "ctrl-k ctrl-s": "aigle::EditKeybindings",
  "ctrl-alt-r": "aigle::ReloadKeybindings",
  "m": "aigle::ToggleMask"
}
```

This removes Ctrl+O, binds Ctrl+P to Open, and adds a two-stroke shortcut for the
keymap editor. Values of `null` disable a default shortcut. Other entries override
the same key or add a new shortcut. Unspecified shortcuts keep their defaults:
removing an entry restores its default rather than disabling it. Mapping an action
to another key adds an alias unless its previous binding is explicitly disabled.

After manually editing the file, select **View → Keyboard Shortcuts → Reload Keybindings** to
apply it immediately. Menus and in-app shortcut hints follow the effective map.
You can also bind the Reload action itself as shown above. To restore all defaults,
delete the file or replace its contents with `{}` and reload.

Shortcuts use GPUI notation: `ctrl`, `alt`, `shift`, `super`, and key names such as
`enter`, `escape`, `left`, or letters. Separate strokes with spaces for a chord.
For example, `ctrl-shift-z` is a simultaneous shortcut, while `ctrl-k ctrl-s` is a
sequence. App actions use the Workspace context; Quit is global. Text editing and
other component bindings are retained when reloading.

Invalid JSON, invalid shortcuts, and unknown actions produce an error notification.
A failed reload leaves the working map intact. An invalid file at startup uses the
bundled defaults and shows an error. The file must use valid JSON, without comments.

## Actions

All names below have the `aigle::` prefix:

- Files: `Open`, `OpenFolder`, `Export`, `Quit`.
- Navigation: `ShowGrid`, `ShowDevelop`, `OpenSelected`, `Left`, `Right`, `Up`, `Down`.
- Rating: `Rate0` through `Rate5`; color labels: `Label1` through `Label4`.
- Editing: `Undo`, `Redo`, `ToggleCrop`, `ToggleMask`, `ToggleWbPicker`, `SwapAspect`, `Cancel`.
- View: `ToggleZoom`, `ToggleBefore`, `ToggleSplit`, `ToggleInfo`, `ToggleHistory`, `ToggleOverlay`.
- Language: `LanguageEnglish`, `LanguageChinese`.
- Shortcuts: `EditKeybindings`, `ReloadKeybindings`.
