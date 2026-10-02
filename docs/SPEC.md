# To-do widget: v1 specification

This consolidates every decision made during the design rounds. The interactive
mockups ("Current reference" page) are the visual reference:
https://claude.ai/artifact/QargbH5eBTiNwcTdZDtjUg

Where this file and older mockup pages disagree, this file wins.

## Platform and budget

- Rust, native Win32 through Microsoft's `windows` crate. One `.exe`, no
  runtime, no installer.
- Idle: 0% CPU (the app waits on the message queue), RAM in single-digit MB.
  Measured in Task Manager, not assumed.
- Offline only. No network access.
- Drawing: Direct2D + DirectWrite. Solid dark panel (no frosted glass in v1).
- Dark theme only in v1.

## Window

- Borderless popup window with rounded corners (DWM), on the desktop.
- Tool window (`WS_EX_TOOLWINDOW`): no taskbar button, not in Alt+Tab. Reached
  through the tray icon.
- **Keep on top** (pin button): on = always on top; off = a normal window that
  other windows can cover and that comes forward when clicked.
- Survives **Win+D** (Show desktop) in both modes, using Rainmeter's detection
  approach (foreground event hook + a helper window at the bottom of the
  z-order).
- **Move** by dragging empty header space. **Resize** from any edge. Minimum
  300 x 180 px. Default 440 x 460 px at the top-right of the primary monitor.
  Position and size are saved.
- **Lock position and size** (⋯ menu) disables moving and resizing.
- **Collapsed state**: a bar (list icon, "To-do", expand chevron) as wide as
  the window. Collapse from the header chevron.
- **Hide header**: ⋯ › Hide header, Ctrl+H, or the tray menu. A thin strip
  stays at the top: its center handle brings the header back, the rest of it
  still drags the window. Remembered between runs.
- Per-monitor DPI aware.

## Lists and views

- Two lists: **Main** (default view) and **Long-term**, switched by tabs.
  (Main is stored as `"next"` in the files, its original name.)
- **Archive** view, opened from the ⋯ menu.
- No "N left" counts anywhere.

## Lines

- Every line is either a **task** (square checkbox) or a **text line** (no
  checkbox, cannot be finished). An empty text line is a blank spacer.
- **Click empty space** below the last line to start a new text line, like a
  text editor. A new line that is left empty is discarded, so stray clicks
  leave nothing behind.
- **Enter while editing** opens a new line directly below, of the same kind
  and indent. Enter on an empty new line just stops editing.
- **Right-click** (or a two-finger tap on a touchpad) on a line switches it
  between task and text line. If the line is part of a multi-selection, the
  whole selection switches.
- The square in the Add box chooses what Enter adds there (task or text
  line). In text mode, Enter on an empty box adds a blank line.
- The checkbox button in the format bar converts selected lines between task
  and text line.
- **Checkbox**: a small square, about 0.85x the text size.
- **Finished task**: square box with a plain gray check (no color fill), text
  struck through and dimmed. It stays in place.
- **Every line is independent.** Indent (Tab / Shift+Tab, 0-3 levels) is
  layout only. Checking, formatting, archiving, deleting or moving a line never
  affects the lines below it.
- **Editing**: click any line (finished or not) to edit its text in place.
  Enter or clicking away saves; Esc cancels. Clearing a task's text and saving
  deletes the task. A cleared text line becomes a blank spacer. Backspace on an
  empty line removes it.

## Selection and moving

- Plain click: edit that line (and select it).
- Shift+click: select a range from the last clicked line. Ctrl+click: add or
  remove one line. Esc clears the selection.
- With several lines selected: dragging, Tab / Shift+Tab, the format bar and
  Delete act on all of them.
- **Move**: press a line and drag (no handle). Starts after the system drag
  threshold. Dragging a selected line moves the whole selection; scattered
  lines land together, in order. Dropping on the other tab moves the lines to
  the end of that list.
- Alt+Up / Alt+Down move the selected lines by one position.

## Formatting

- Hidden until the **A** button in the header is pressed. Then one line:
  B, I, U | text color | outdent, indent | task/text | Aa.
- B, I, U, color, indent and task/text apply to the selected lines only, never
  to the lines below them. Formatting is per whole line, not per word.
- Ctrl+B / Ctrl+I / Ctrl+U work without opening the bar.
- **Aa** sets the whole list's font family, size and default text color (the
  Windows font dialog in v1).
- Default text: Segoe UI Variable, 10.5 pt (Windows 11's standard body text).

## Archive and delete

- Hovering a finished task shows **Archive** and **Delete**.
- ⋯ › **Archive finished tasks** archives every finished task (only finished
  tasks; text lines are never archived).
- Archive view: each line shows its source list and date, with **Restore**
  (back to the end of its list) and **Delete** (permanent).
- Archiving and deleting are by hand only.

## Undo and redo

- Ctrl+Z / Ctrl+Y (and Ctrl+Shift+Z), also in the ⋯ menu with the action name.
- One step per action: edit, check-off, move, format change, indent, line-type
  change, add, delete, archive, restore, bulk archive.
- Undo reaches into the archive.
- Memory only, up to 100 steps, cleared when the app closes.
- After undo or redo a short message shows what happened, with a Redo button.

## Tray and startup

- Tray icon: left click shows or hides the widget. Right-click menu: Show/Hide,
  Show/Hide header, Always on top, Quit.
- ⋯ menu: Undo, Redo, Archive finished tasks, Open archive, Lock position and
  size, Start with Windows, Hide header, Hide to tray, Quit.
- Start with Windows: a value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Off by default.
- Only one instance runs; starting it again shows the existing window.

## Storage

All in `%APPDATA%\TodoWidget\`, written only when something changes, via a
temp file + rename:

- `tasks.json`: Main and Long-term lines, in display order.
- `archive.json`: archived tasks with the date and the list they came from.
- `settings.json`: window position, size, collapsed, keep on top, lock, font
  family, font size, default text color, last view.

A file that fails to parse is renamed to `*.corrupt-<timestamp>.json`, never
silently overwritten. A UTF-8 byte-order mark (which Notepad may add) is
accepted.

How the code is organized: [ARCHITECTURE.md](ARCHITECTURE.md).

## Out of scope for v1

- Global quick-add hotkey.
- Frosted glass backdrop, light theme.
- Due dates, tags, priorities.
- Per-word formatting.
- Microsoft To Do sync.
