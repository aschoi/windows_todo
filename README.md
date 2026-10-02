# To-do widget

A small to-do list that lives on the Windows 11 desktop. Native Win32 in Rust,
one `.exe`, no runtime. The full behavior is specified in
[docs/SPEC.md](docs/SPEC.md); how the code is organized is in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build and run

```
cargo build --release
target\release\win_widget_todo.exe
```

Needs the MSVC toolchain (pinned in `rust-toolchain.toml`) and the Visual
Studio C++ build tools for the linker.

## Data

`%APPDATA%\TodoWidget\` holds `tasks.json`, `archive.json` and
`settings.json`. Files are written only on change, via a temp file and a
rename. A file that fails to parse is renamed to `*.corrupt-<timestamp>.json`
rather than overwritten.

Set `TODO_WIDGET_DATA` to another folder to run against test data without
touching your real list.

## Tests

```
cargo test
```

The document logic (moving, archiving, editing rules) and undo history are
unit-tested. Window behavior is checked by hand:

- Win+D with Keep on top off and on; Win+D again to restore.
- Restart Explorer (Task Manager › Windows Explorer › Restart): the tray icon
  comes back.
- Drag between monitors with different scaling.
- Shift+click / Ctrl+click selection, Tab / Shift+Tab, Delete, Alt+Up/Down,
  Ctrl+Z / Ctrl+Y.
- ⋯ menu, tray menu, Aa font dialog, Start with Windows.

## Rendering

Direct2D with a software render target by default: it measured about 8 MB in
Task Manager versus about 40 MB of private memory with the GPU target, and a
small list does not need the GPU. Set `TODO_WIDGET_GPU=1` to compare.
