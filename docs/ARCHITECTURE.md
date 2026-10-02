# How the source is organized

A map of `src/`: what each file is responsible for, how a click or a key press
travels through the code, and where to look when you want to change
something. For *what* the app does, see [SPEC.md](SPEC.md).

## The files at a glance

| File | Lines | Responsibility | Talks to Windows? |
|---|---|---|---|
| `main.rs` | ~520 | Program entry: creates the window, runs the message loop, routes messages to `App`. Also the edit-box hooks, menus, font dialog and the generated tray icon. | Yes |
| `app.rs` | ~2700 | The widget itself: its state, layout, drawing and input handling. | Yes |
| `doc.rs` | ~390 | The list data and every operation on it (add, move, indent, archive, restore). | No |
| `history.rs` | ~100 | Undo and redo. | No |
| `model.rs` | ~120 | The shapes of the data saved to disk: a line, a list, the settings. | No |
| `store.rs` | ~100 | Reading and writing the JSON files. | Files only |
| `gfx.rs` | ~430 | Drawing helpers on top of Direct2D and DirectWrite. | Yes |
| `platform.rs` | ~225 | Small Windows services: tray icon, Start with Windows, the date, Show-desktop detection. | Yes |
| `build.rs` (repo root) | 12 | Runs at compile time; embeds a manifest that turns on modern edit-box features. | Build only |

The split follows one rule: **logic that can be tested without a window lives
in files that never call Windows** (`doc.rs`, `history.rs`, `model.rs`). Those
are what `cargo test` covers. Everything that touches the screen, the mouse or
the system is in the other files.

## How the pieces depend on each other

```
main.rs ──► app.rs ──► doc.rs ──► model.rs
               │  ├──► history.rs
               │  ├──► store.rs ──► model.rs
               │  ├──► gfx.rs
               │  └──► platform.rs
```

Arrows point from the user to the used. `doc.rs` and `model.rs` know nothing
about windows or drawing, so they could be reused unchanged by, say, a future
Microsoft To Do sync.

## Each file in more detail

### `main.rs`: startup and message routing

- **`main()`** makes the process DPI-aware, enforces a single instance (a
  second launch just shows the first), registers the window classes, creates
  the window at its saved position, builds the `App`, then runs the
  **message loop**: Windows hands the app one event at a time (a click, a key,
  "please repaint") and the loop dispatches each to `wndproc`. Between events
  the app does nothing, which is why idle CPU is 0%.
- **`wndproc`** answers three messages itself (frame removal, activation, the
  minimum window size) and passes everything else to `App::handle`.
- **`with_app`** is how `wndproc` reaches the `App`. See *The re-entrancy
  rule* below for why it is written defensively.
- **`run_deferred`** shows the ⋯ and tray menus and the font dialog. These are
  *modal* (they run their own loop until closed), so they run only after
  `App` has finished handling the current message.
- **`edit_proc` / `add_proc`** hook the two text boxes (the inline editor and
  the Add box) so Enter, Esc, Tab, Ctrl+B/I/U/H and Backspace-on-empty reach
  the app instead of the text box.
- **`make_icon`** draws the tray icon pixel by pixel, so the app needs no
  icon file.

### `app.rs`: the widget

The `App` struct holds everything the widget knows: the document, the
settings, which view is showing, the selection, the line being edited, drag
state, scroll position, and the rectangles of everything currently on screen.

It is organized in sections, in this order:

| Section | What it does | Key functions |
|---|---|---|
| look | Colors, sizes, icon glyphs as constants | |
| changes | Every edit to the list goes through `change()`, which records an undo step, applies the edit, saves and redraws | `change`, `undo`, `redo`, `format_sel`, `indent_sel`, `delete_sel` |
| edit boxes | Creating, positioning and committing the inline editor and the Add box; new lines from empty-space clicks and Enter | `start_edit`, `start_new_line`, `commit_edit`, `cancel_edit`, `toggle_kind` |
| layout | Works out where everything goes for the current window size, and records clickable areas | `relayout`, `layout_rows`, `hit_test` |
| paint | Draws the header (or the hidden-header strip), format bar, rows, Add box and popups | `paint`, `paint_header`, `paint_rows`, ... |
| input | Mouse down / move / up, wheel, keys, drag and drop, resize edges | `on_down`, `on_move`, `on_up`, `on_key`, `nc_hit` |
| window state | Keep on top, collapse, show/hide, Win+D handling | `set_on_top`, `set_collapsed`, `check_show_desktop` |
| menus | The ⋯ and tray menu contents and their commands | `more_menu`, `tray_menu`, `menu_command` |
| dispatch | `handle()`: one `match` that maps each Windows message to the functions above | `handle` |

**Layout and hit-testing share one source of truth.** `relayout()` computes
every row and button rectangle once and stores them with a `Hit` label
(`Hit::Check(id)`, `Hit::Pin`, `Hit::ListBlank`, ...). Painting draws at those
rectangles and clicks are resolved against the same list, so what you see is
exactly what you can click.

`app.rs` is the largest file by far. If it keeps growing, the natural split is
along the sections above (`layout.rs`, `paint.rs`, `input.rs`), which already
share no state beyond `App` itself.

### `doc.rs`: the list data

`Doc` holds two vectors: `tasks` (Main and Long-term lines, in display order)
and `archive`. Every operation is a plain function on those vectors:
`add`, `remove`, `set_text`, `indent`, `move_group`, `move_to_list`, `nudge`,
`archive`, `restore`, `purge`. The rule that every line is independent
(indent is layout only) lives here, and its unit tests sit at the bottom of
the file.

### `history.rs`: undo and redo

Before each change, `App` hands `History` a copy of both lists and a label
("Move 3 lines"). Undo swaps the current lists for the saved copy. Copying
whole lists sounds wasteful, but a to-do list is a few kilobytes; even 100
steps cost well under a megabyte, and it can never get an inverse operation
wrong. History lives only in memory.

### `model.rs`: what is saved

`Item` is one line: text, task or text, done, indent, list, bold, italic,
underline, color, and the archive date for archived lines. `Settings` is the
window position and size, collapsed, header hidden, keep on top, lock, font
and last view. These structs are the JSON format: the field names here are the
keys you see in the files.

The **Main** list is stored as `"next"`, its original name, so files written
before the rename still load.

### `store.rs`: the files

Reads and writes `tasks.json`, `archive.json` and `settings.json` in
`%APPDATA%\TodoWidget` (or the folder in `TODO_WIDGET_DATA`). Writes go to a
temporary file that is flushed to disk and then renamed over the real one, so
a crash mid-write cannot leave a half-written file. A file that cannot be
parsed is renamed aside (`*.corrupt-<time>.json`), never silently overwritten.
A leading UTF-8 byte-order mark, which Notepad may add, is accepted.

### `gfx.rs`: drawing

A thin layer over Direct2D (shapes) and DirectWrite (text). All coordinates
are **DIPs** (device-independent pixels, 1/96 inch). The render target is told
the window's DPI and does the scaling, so the rest of the code never thinks
about 125% or 150% displays. It uses Direct2D's software renderer, which
measured about 8 MB in Task Manager versus about 40 MB of private memory for
the GPU one.

### `platform.rs`: Windows services

- Tray icon add and remove.
- Start with Windows: one registry value under `HKCU\...\Run`.
- Today's date for the archive.
- **Show desktop (Win+D) detection**, following Rainmeter's approach. A hidden
  "sentinel" window is kept at the very bottom of the window stack; when the
  desktop comes to the foreground and is now *above* the sentinel, Show
  desktop is on, and `app.rs` lifts the widget above it until another window
  takes the foreground.

## How a click travels through the code

Clicking a task's checkbox:

1. Windows puts `WM_LBUTTONDOWN` in the queue; the loop in `main()` dispatches
   it to `wndproc`.
2. `wndproc` calls `App::handle`, which calls `on_down`.
3. `on_down` converts the pixel position to DIPs and asks `hit_test`, which
   returns `Hit::Check(id)`.
4. `on_down` calls `change("Check off", ...)`: `History` records the current
   lists, `Doc::apply` flips `done`, `store` saves `tasks.json`, `relayout`
   recomputes positions, and `invalidate` asks Windows for a repaint.
5. Windows later sends `WM_PAINT`, and `paint` draws the struck-through line.

A key press in the inline editor takes a short detour: the text box's hook
(`edit_proc` in `main.rs`) does not act itself, it *posts* a message
(`WM_EDIT_CMD`) to the main window, which `handle` routes to `on_edit_cmd`.

## The re-entrancy rule

Some Windows calls (moving a window, changing focus, destroying a text box)
send messages back to the window *immediately*, while the app is still in the
middle of handling the previous one. In Rust, the `App` is borrowed mutably
during handling, so it cannot safely be borrowed again.

The code handles this in three ways:

- `with_app` uses `try_borrow_mut`: a nested message finds the `App` busy and
  falls back to Windows' default handling. Nothing in the app depends on
  those nested messages.
- The text-box hooks only **post** messages, which arrive later, after the
  current one is finished.
- Menus and the font dialog are returned as `Deferred` work and run after the
  borrow ends.

If you add a feature that calls a Windows function from inside `App`, check
whether that function sends messages synchronously; if its result matters,
follow one of these patterns.

## Where to change common things

| To change... | Look in |
|---|---|
| Colors, spacing, header height, checkbox size | constants at the top of `app.rs`; `layout_rows` for row metrics |
| A tab's label | `ListId::label` in `model.rs` |
| What a click does | `on_down` (and `on_up` for clicks on line text) in `app.rs` |
| Keyboard shortcuts | `on_key` in `app.rs`; `edit_proc` in `main.rs` while editing |
| Menu items | `more_menu` / `tray_menu` / `menu_command` in `app.rs` |
| List behavior (moving, archiving rules) | `doc.rs`, plus a test there |
| What is saved | `model.rs` (add a field with a default so old files still load) |
| Where files go | `store.rs` |
