//! The widget: state, layout, painting and input.
//!
//! Re-entrancy rule: window procedures can be re-entered while `App` is
//! borrowed (SetWindowPos, SetFocus, DestroyWindow send messages
//! synchronously). Those nested messages fall through to DefWindowProc, so
//! nothing here relies on them; the edit-box subclasses only *post* messages,
//! and modal UI (menus, font dialog) runs after the borrow is released.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, EndPaint, FONT_CHARSET, FW_BOLD, FW_NORMAL,
    HBRUSH, HDC, HFONT, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ScreenToClient,
    SetBkColor, SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    EM_CHARFROMPOS, EM_SETCUEBANNER, EM_SETMARGINS, EM_SETSEL, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_MENU, VK_RETURN, VK_SHIFT, VK_TAB,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::doc::{Doc, View};
use crate::gfx::{Gfx, Rect, Style, TextBlock};
use crate::history::History;
use crate::model::{Item, ItemFile, Kind, ListId, Settings, parse_hex, to_hex};
use crate::platform::{self, DesktopWatch, WM_DESKTOP_FOREGROUND, WM_OTHER_FOREGROUND, WM_TRAY};
use crate::store::Store;

// ---------------------------------------------------------------- look

const PANEL: u32 = 0x1E2229;
const TEXT: u32 = 0xF3F4F6;
const SUB: u32 = 0xA9B2BE;
const MUTED: u32 = 0x8A94A1;
const FAINT: u32 = 0x5E6773;
const ACCENT: u32 = 0x62BDFF;
const FIELD: u32 = 0x262B33;
const POPUP: u32 = 0x2C2C2C;
const DANGER: u32 = 0xF6A878;
const CHECK: u32 = 0xC9D1DB;
const WHITE: u32 = 0xFFFFFF;
/// Solid equivalents of the translucent row highlights, for the edit box.
const SEL_SOLID: u32 = 0x2E3138;
const HOVER_SOLID: u32 = 0x252930;
const SWATCHES: [Option<u32>; 6] = [
    None,
    Some(0x62BDFF),
    Some(0xF6A878),
    Some(0x7FD69B),
    Some(0xF28B9B),
    Some(0xC9A7FF),
];

const UI_FONT: &str = "Segoe UI Variable Text";
const HEADER_H: f32 = 48.0;
/// Height of the strip left when the header is hidden.
const STRIP_H: f32 = 14.0;
pub const PILL_H: f32 = 44.0;
const EDGE: f32 = 6.0;
pub const MIN_W: f32 = 300.0;
pub const MIN_H: f32 = 180.0;

const G_PIN: char = '\u{E718}';
const G_PIN_ON: char = '\u{E841}';
const G_MORE: char = '\u{E712}';
const G_UP: char = '\u{E70E}';
const G_DOWN: char = '\u{E70D}';
const G_BACK: char = '\u{E72B}';
const G_DELETE: char = '\u{E74D}';
const G_FONT: char = '\u{E8D2}';
const G_LIST: char = '\u{E8FD}';
const G_CHECK: char = '\u{E73E}';

// ---------------------------------------------------------------- messages

pub const WM_EDIT_CMD: u32 = WM_APP + 1;
pub const WM_ADD_ENTER: u32 = WM_APP + 2;
pub const WM_QUIT_APP: u32 = WM_APP + 3;
const ID_EDIT: usize = 100;
const ID_ADD: usize = 101;
const TIMER_DESKTOP: usize = 1;
pub const SENTINEL_CLASS: PCWSTR = w!("TodoWidgetSentinel");

mod cmd {
    pub const ENTER: usize = 1;
    pub const ESC: usize = 2;
    pub const TAB: usize = 3;
    pub const SHIFT_TAB: usize = 4;
    pub const BACK_EMPTY: usize = 5;
    pub const KILL_FOCUS: usize = 6;
    pub const BOLD: usize = 7;
    pub const ITALIC: usize = 8;
    pub const UNDERLINE: usize = 9;
    pub const HEADER: usize = 10;
}

mod menu_id {
    pub const UNDO: u32 = 1;
    pub const REDO: u32 = 2;
    pub const ARCHIVE_FINISHED: u32 = 3;
    pub const OPEN_ARCHIVE: u32 = 4;
    pub const LOCK: u32 = 5;
    pub const AUTOSTART: u32 = 6;
    pub const HIDE: u32 = 7;
    pub const QUIT: u32 = 8;
    pub const HEADER: u32 = 9;
    pub const SHOW_HIDE: u32 = 20;
    pub const ON_TOP: u32 = 21;
}

pub struct MenuEntry {
    pub id: u32,
    pub label: String,
    pub checked: bool,
    pub enabled: bool,
}

impl MenuEntry {
    fn item(id: u32, label: impl Into<String>, checked: bool, enabled: bool) -> Self {
        Self {
            id,
            label: label.into(),
            checked,
            enabled,
        }
    }
    fn sep() -> Self {
        Self {
            id: 0,
            label: String::new(),
            checked: false,
            enabled: true,
        }
    }
}

/// Work that must run after the `App` borrow is released.
pub enum Deferred {
    Menu {
        entries: Vec<MenuEntry>,
        x: i32,
        y: i32,
    },
    FontDialog,
}

pub enum Out {
    Done(LRESULT),
    Default,
    Defer(Deferred),
    Destroy,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Caption,
    ShowHeader,
    Tab(ListId),
    Back,
    Format,
    Pin,
    More,
    Collapse,
    Expand,
    Bold,
    Italic,
    Underline,
    Color,
    Outdent,
    Indent,
    KindToggle,
    Font,
    Swatch(usize),
    ChipDelete,
    ChipClear,
    ToastRedo,
    AddKind,
    Check(u64),
    Text(u64),
    RowArchive(u64),
    RowDelete(u64),
    Restore(u64),
    Purge(u64),
    ListBlank,
}

struct Row {
    id: u64,
    rect: Rect,
    text_x: f32,
    text_y: f32,
    text_w: f32,
    block: Option<TextBlock>,
    sub: Option<TextBlock>,
    check: Option<Rect>,
}

struct Edit {
    id: u64,
    hwnd: HWND,
    font: HFONT,
    original: String,
    text: String,
    /// A line created by clicking empty space or pressing Enter: it only
    /// becomes real (and undoable) once it has text. Holds the document as it
    /// was before the line was inserted.
    fresh: Option<(Vec<Item>, Vec<Item>)>,
}

struct Press {
    id: u64,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum DropAt {
    Row(u64, bool),
    Tab(ListId),
}

struct Drag {
    group: Vec<u64>,
    at: Option<DropAt>,
}

pub struct App {
    pub hwnd: HWND,
    sentinel: HWND,
    gfx: Gfx,
    store: Store,
    doc: Doc,
    pub settings: Settings,
    hist: History,
    view: View,
    sel: Vec<u64>,
    anchor: Option<u64>,
    hover: Option<u64>,
    fmt_open: bool,
    color_pop: bool,
    new_kind: Kind,
    edit: Option<Edit>,
    add: HWND,
    add_font: HFONT,
    edit_brush: HBRUSH,
    field_brush: HBRUSH,
    scroll: f32,
    press: Option<Press>,
    drag: Option<Drag>,
    toast: Option<String>,
    rows: Vec<Row>,
    hits: Vec<(Rect, Hit)>,
    overlay: Vec<(Rect, Hit)>,
    list_rect: Rect,
    add_rect: Rect,
    color_rect: Rect,
    pub dpi: u32,
    tracking: bool,
    raised: bool,
    desktop_tries: u32,
    pub taskbar_created: u32,
    tray_icon: HICON,
    _watch: Option<DesktopWatch>,
}

fn lo(v: isize) -> i32 {
    (v & 0xFFFF) as i16 as i32
}
fn hi(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as i16 as i32
}
fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}
fn colorref(rgb: u32) -> COLORREF {
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

impl App {
    pub fn new(
        hwnd: HWND,
        sentinel: HWND,
        settings: Settings,
        store: Store,
        tray_icon: HICON,
    ) -> Self {
        let tasks = store.load_tasks();
        let archive = store.load_archive();
        let view = View::List(settings.view);
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
        let mut app = Self {
            hwnd,
            sentinel,
            gfx: Gfx::new().expect("Direct2D / DirectWrite unavailable"),
            store,
            doc: Doc::new(tasks.items, archive.items),
            settings,
            hist: History::default(),
            view,
            sel: Vec::new(),
            anchor: None,
            hover: None,
            fmt_open: false,
            color_pop: false,
            new_kind: Kind::Task,
            edit: None,
            add: HWND::default(),
            add_font: HFONT::default(),
            edit_brush: unsafe { CreateSolidBrush(colorref(SEL_SOLID)) },
            field_brush: unsafe { CreateSolidBrush(colorref(FIELD)) },
            scroll: 0.0,
            press: None,
            drag: None,
            toast: None,
            rows: Vec::new(),
            hits: Vec::new(),
            overlay: Vec::new(),
            list_rect: Rect::default(),
            add_rect: Rect::default(),
            color_rect: Rect::default(),
            dpi,
            tracking: false,
            raised: false,
            desktop_tries: 0,
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            tray_icon,
            _watch: None,
        };
        app.create_add_box();
        app._watch = Some(DesktopWatch::start(hwnd));
        app.sentinel_to_bottom();
        platform::tray_add(hwnd, tray_icon);
        app.relayout();
        app
    }

    fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    fn client_dips(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let s = self.scale();
        (
            (rc.right - rc.left) as f32 / s,
            (rc.bottom - rc.top) as f32 / s,
        )
    }

    fn font_dip(&self) -> f32 {
        self.settings.font_size_pt * 96.0 / 72.0
    }

    fn ink(&self) -> u32 {
        parse_hex(&self.settings.ink).unwrap_or(TEXT)
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn sel_ordered(&self) -> Vec<u64> {
        self.doc
            .ids(self.view)
            .into_iter()
            .filter(|id| self.sel.contains(id))
            .collect()
    }

    // ------------------------------------------------------------ changes

    /// Every document change goes through here: one undo step, then save.
    fn change(&mut self, label: &str, f: impl FnOnce(&mut Doc)) {
        // A new line being typed becomes a real step before anything else.
        if let Some((tasks, archive)) = self.edit.as_mut().and_then(|e| e.fresh.take()) {
            self.hist.record(&tasks, &archive, "Add line");
        }
        self.hist.record(&self.doc.tasks, &self.doc.archive, label);
        f(&mut self.doc);
        self.after_doc_change();
    }

    fn after_doc_change(&mut self) {
        self.toast = None;
        let doc = &self.doc;
        self.sel.retain(|id| doc.contains_active(*id));
        if let Some(a) = self.anchor
            && !self.doc.contains_active(a)
        {
            self.anchor = None;
        }
        self.save_doc();
        self.relayout();
        self.invalidate();
    }

    fn save_doc(&self) {
        self.store.save_tasks(&ItemFile {
            version: 1,
            items: self.doc.tasks.clone(),
        });
        self.store.save_archive(&ItemFile {
            version: 1,
            items: self.doc.archive.clone(),
        });
    }

    fn save_settings(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rc);
        }
        self.settings.rect = Some([rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top]);
        self.store.save_settings(&self.settings);
    }

    fn undo(&mut self) {
        self.cancel_edit();
        if let Some(s) = self
            .hist
            .undo((self.doc.tasks.clone(), self.doc.archive.clone()))
        {
            self.doc.tasks = s.tasks;
            self.doc.archive = s.archive;
            self.after_doc_change();
            self.toast = Some(format!("Undone: {}", s.label));
            self.relayout();
        }
    }

    fn redo(&mut self) {
        self.cancel_edit();
        if let Some(s) = self
            .hist
            .redo((self.doc.tasks.clone(), self.doc.archive.clone()))
        {
            self.doc.tasks = s.tasks;
            self.doc.archive = s.archive;
            self.after_doc_change();
            self.toast = Some(format!("Redone: {}", s.label));
            self.relayout();
        }
    }

    fn plural(base: &str, n: usize) -> String {
        if n > 1 {
            format!("{base} {n} lines")
        } else {
            base.to_string()
        }
    }

    fn format_sel(
        &mut self,
        label: &str,
        f: impl Fn(&mut crate::model::Item, &[&crate::model::Item]),
    ) {
        let ids = self.sel_ordered();
        if ids.is_empty() || self.view == View::Archive {
            return;
        }
        let snapshot: Vec<crate::model::Item> = ids
            .iter()
            .filter_map(|id| self.doc.get(*id).cloned())
            .collect();
        let refs: Vec<&crate::model::Item> = snapshot.iter().collect();
        let label = if ids.len() > 1 {
            format!("{label} ({} lines)", ids.len())
        } else {
            label.to_string()
        };
        self.change(&label, |d| d.apply(&ids, |i| f(i, &refs)));
        self.refresh_edit_font();
    }

    fn toggle_style(&mut self, which: u8) {
        let label = ["Bold", "Italic", "Underline"][which as usize];
        self.format_sel(label, move |i, all| {
            let on = |x: &crate::model::Item| match which {
                0 => x.bold,
                1 => x.italic,
                _ => x.underline,
            };
            let set = !all.iter().all(|x| on(x));
            match which {
                0 => i.bold = set,
                1 => i.italic = set,
                _ => i.underline = set,
            }
        });
    }

    fn indent_sel(&mut self, delta: i8) {
        let ids = self.sel_ordered();
        if ids.is_empty() || self.view == View::Archive {
            return;
        }
        let label = Self::plural(if delta > 0 { "Indent" } else { "Outdent" }, ids.len());
        self.change(&label, |d| d.indent(&ids, delta));
    }

    fn delete_sel(&mut self) {
        let ids = self.sel_ordered();
        if ids.is_empty() || self.view == View::Archive {
            return;
        }
        if self.edit.as_ref().is_some_and(|e| ids.contains(&e.id)) {
            self.cancel_edit();
        }
        self.change(&Self::plural("Delete", ids.len()), |d| d.remove(&ids));
        self.sel.clear();
    }

    fn set_view(&mut self, v: View) {
        self.commit_edit();
        self.view = v;
        self.sel.clear();
        self.anchor = None;
        self.scroll = 0.0;
        self.color_pop = false;
        if let View::List(l) = v {
            self.settings.view = l;
            self.store.save_settings(&self.settings);
        }
        self.relayout();
        self.invalidate();
    }

    // ------------------------------------------------------------ edit boxes

    fn make_font(&self, bold: bool, italic: bool, underline: bool, strike: bool) -> HFONT {
        let px = (self.settings.font_size_pt * self.dpi as f32 / 72.0).round() as i32;
        let face = platform::wide(&self.settings.font_family);
        unsafe {
            CreateFontW(
                -px,
                0,
                0,
                0,
                if bold {
                    FW_BOLD.0 as i32
                } else {
                    FW_NORMAL.0 as i32
                },
                italic as u32,
                underline as u32,
                strike as u32,
                FONT_CHARSET(DEFAULT_CHARSET.0),
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                PCWSTR(face.as_ptr()),
            )
        }
    }

    fn create_add_box(&mut self) {
        unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let add = CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                10,
                10,
                Some(self.hwnd),
                Some(HMENU(ID_ADD as *mut _)),
                Some(inst.into()),
                None,
            )
            .unwrap_or_default();
            crate::subclass(add, crate::add_proc);
            self.add = add;
        }
        self.refresh_add_font();
    }

    fn refresh_add_font(&mut self) {
        let font = self.make_font(false, false, false, false);
        unsafe {
            SendMessageW(
                self.add,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            SendMessageW(self.add, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(0)));
            if !self.add_font.is_invalid() {
                let _ = DeleteObject(self.add_font.into());
            }
        }
        self.add_font = font;
        self.refresh_cue();
    }

    fn refresh_cue(&self) {
        let cue = if self.new_kind == Kind::Text {
            "Add a line of text (Enter on empty adds a blank line)"
        } else {
            "Add a task"
        };
        let wide = platform::wide(cue);
        unsafe {
            SendMessageW(
                self.add,
                EM_SETCUEBANNER,
                Some(WPARAM(1)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    }

    fn start_edit(&mut self, id: u64, click: Option<(i32, i32)>) {
        self.commit_edit();
        let Some(item) = self.doc.get(id).cloned() else {
            return;
        };
        if self.view == View::Archive {
            return;
        }
        let font = self.make_font(item.bold, item.italic, item.underline, item.done);
        let wide = platform::wide(&item.text);
        let hwnd = unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                w!("EDIT"),
                PCWSTR(wide.as_ptr()),
                WS_CHILD | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
                0,
                0,
                10,
                10,
                Some(self.hwnd),
                Some(HMENU(ID_EDIT as *mut _)),
                Some(inst.into()),
                None,
            )
            .unwrap_or_default()
        };
        if hwnd.is_invalid() {
            return;
        }
        unsafe {
            crate::subclass(hwnd, crate::edit_proc);
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(0)),
            );
            SendMessageW(hwnd, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(0)));
        }
        self.edit = Some(Edit {
            id,
            hwnd,
            font,
            original: item.text.clone(),
            text: item.text,
            fresh: None,
        });
        self.relayout();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetFocus(Some(hwnd));
            let len = SendMessageW(hwnd, WM_GETTEXTLENGTH, None, None).0;
            let mut caret = len;
            if let Some((x, y)) = click {
                let mut pt = POINT { x, y };
                let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let r = SendMessageW(
                    hwnd,
                    EM_CHARFROMPOS,
                    None,
                    Some(LPARAM(((pt.y as isize) << 16) | (pt.x as isize & 0xFFFF))),
                )
                .0;
                caret = (r & 0xFFFF).min(len);
            }
            SendMessageW(
                hwnd,
                EM_SETSEL,
                Some(WPARAM(caret as usize)),
                Some(LPARAM(caret)),
            );
        }
        self.invalidate();
    }

    /// Inserts an empty line (after `after`, or at the end) and edits it.
    fn start_new_line(&mut self, after: Option<u64>, kind: Kind, indent: u8) {
        let View::List(list) = self.view else { return };
        self.commit_edit();
        let before = (self.doc.tasks.clone(), self.doc.archive.clone());
        let id = self.doc.add(list, kind, "");
        if let Some(a) = after.filter(|a| self.doc.contains_active(*a)) {
            self.doc.move_group(&[id], a, true);
        }
        if let Some(item) = self.doc.get_mut(id) {
            item.indent = indent;
        }
        self.sel = vec![id];
        self.anchor = Some(id);
        self.start_edit(id, None);
        if let Some(e) = &mut self.edit {
            e.fresh = Some(before);
        }
        self.scroll_into_view(id);
    }

    fn scroll_into_view(&mut self, id: u64) {
        self.relayout();
        let list = self.list_rect;
        if let Some(r) = self.rows.iter().find(|r| r.id == id) {
            if r.rect.b > list.b {
                self.scroll += r.rect.b - list.b;
            } else if r.rect.t < list.t {
                self.scroll -= list.t - r.rect.t;
            }
            self.relayout();
        }
        self.invalidate();
    }

    fn toggle_kind(&mut self, id: u64) {
        if self.view == View::Archive {
            return;
        }
        let ids = if self.sel.contains(&id) && self.sel.len() > 1 {
            self.sel_ordered()
        } else {
            vec![id]
        };
        let to_task = self.doc.get(id).is_some_and(|i| !i.is_task());
        self.commit_edit();
        let label = if to_task {
            "Make task"
        } else {
            "Make text line"
        };
        let label = Self::plural(label, ids.len());
        self.change(&label, |d| {
            d.apply(&ids, |i| {
                i.kind = if to_task { Kind::Task } else { Kind::Text };
                i.done = false;
            })
        });
    }

    fn set_header_hidden(&mut self, hidden: bool) {
        self.settings.header_hidden = hidden;
        self.store.save_settings(&self.settings);
        self.relayout();
        self.invalidate();
    }

    fn destroy_edit(&mut self) -> Option<Edit> {
        let e = self.edit.take()?;
        unsafe {
            let _ = DestroyWindow(e.hwnd);
            let _ = DeleteObject(e.font.into());
            let _ = SetFocus(Some(self.hwnd));
        }
        Some(e)
    }

    fn edit_text(&self) -> Option<String> {
        let e = self.edit.as_ref()?;
        unsafe {
            let len = GetWindowTextLengthW(e.hwnd) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = GetWindowTextW(e.hwnd, &mut buf) as usize;
            Some(String::from_utf16_lossy(&buf[..n]))
        }
    }

    fn commit_edit(&mut self) {
        let Some(text) = self.edit_text() else { return };
        let e = self.destroy_edit().unwrap();
        if let Some((tasks, archive)) = e.fresh {
            if text.trim().is_empty() {
                // Nothing typed: the line never existed.
                self.doc.remove(&[e.id]);
                self.relayout();
                self.invalidate();
            } else {
                let kind = self.doc.get(e.id).map(|i| i.kind).unwrap_or(Kind::Text);
                self.hist.record(
                    &tasks,
                    &archive,
                    if kind == Kind::Task {
                        "Add task"
                    } else {
                        "Add line"
                    },
                );
                self.doc.set_text(e.id, &text);
                self.after_doc_change();
            }
            return;
        }
        if text == e.original || !self.doc.contains_active(e.id) {
            self.relayout();
            self.invalidate();
            return;
        }
        let clean_empty = text.trim().is_empty();
        let is_task = self.doc.get(e.id).is_some_and(|i| i.is_task());
        let label = if clean_empty && is_task {
            "Delete"
        } else {
            "Edit"
        };
        self.change(label, |d| {
            d.set_text(e.id, &text);
        });
    }

    fn cancel_edit(&mut self) {
        if let Some(e) = self.destroy_edit() {
            if e.fresh.is_some() {
                self.doc.remove(&[e.id]);
            }
            self.relayout();
            self.invalidate();
        }
    }

    fn refresh_edit_font(&mut self) {
        let Some(id) = self.edit.as_ref().map(|e| e.id) else {
            return;
        };
        let Some(item) = self.doc.get(id).cloned() else {
            return;
        };
        let font = self.make_font(item.bold, item.italic, item.underline, item.done);
        let e = self.edit.as_mut().unwrap();
        unsafe {
            SendMessageW(
                e.hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = DeleteObject(e.font.into());
        }
        e.font = font;
        self.relayout();
    }

    // ------------------------------------------------------------ layout

    fn relayout(&mut self) {
        self.hits.clear();
        self.overlay.clear();
        self.rows.clear();
        let (w, h) = self.client_dips();
        if self.settings.collapsed {
            self.hits.push((
                Rect::xywh(w - 40.0, (h - 32.0) / 2.0, 32.0, 32.0),
                Hit::Expand,
            ));
            self.hits.push((Rect::new(0.0, 0.0, w, h), Hit::Caption));
            self.position_children();
            return;
        }
        let archive = self.view == View::Archive;
        let semi = Style {
            semibold: true,
            ..Default::default()
        };

        // Header. When hidden, a thin strip stays: its center handle brings the
        // header back, the rest of it still drags the window.
        if self.settings.header_hidden {
            self.hits.push((
                Rect::xywh(w / 2.0 - 30.0, 0.0, 60.0, STRIP_H),
                Hit::ShowHeader,
            ));
            self.hits
                .push((Rect::new(0.0, 0.0, w, STRIP_H), Hit::Caption));
        }
        let header_h = if self.settings.header_hidden {
            STRIP_H
        } else {
            HEADER_H
        };
        let mut x = 10.0;
        if self.settings.header_hidden {
            // No header buttons.
        } else if archive {
            self.hits.push((Rect::xywh(x, 9.0, 104.0, 30.0), Hit::Back));
        } else {
            for l in [ListId::Next, ListId::Later] {
                let tw = self.gfx.measure(l.label(), UI_FONT, 13.0, semi) + 24.0;
                self.hits.push((Rect::xywh(x, 9.0, tw, 30.0), Hit::Tab(l)));
                x += tw + 2.0;
            }
        }
        let mut rx = w - 8.0 - 32.0;
        let mut buttons = vec![Hit::Collapse, Hit::More, Hit::Pin];
        if !archive {
            buttons.push(Hit::Format);
        }
        if self.settings.header_hidden {
            buttons.clear();
        }
        for b in buttons {
            self.hits.push((Rect::xywh(rx, 8.0, 32.0, 32.0), b));
            rx -= 34.0;
        }
        self.hits
            .push((Rect::new(0.0, 0.0, w, header_h), Hit::Caption));
        let mut y = header_h;

        // Format bar: B I U | color | outdent indent | task/text ... Aa
        if self.fmt_open && !archive {
            let bar = Rect::new(10.0, y, w - 10.0, y + 34.0);
            let mut bx = bar.l + 3.0;
            let by = bar.t + 3.0;
            for (hit, bw, gap) in [
                (Hit::Bold, 28.0, 0.0),
                (Hit::Italic, 28.0, 0.0),
                (Hit::Underline, 28.0, 9.0),
                (Hit::Color, 38.0, 9.0),
                (Hit::Outdent, 28.0, 0.0),
                (Hit::Indent, 28.0, 9.0),
                (Hit::KindToggle, 28.0, 0.0),
            ] {
                self.hits.push((Rect::xywh(bx, by, bw, 28.0), hit));
                if hit == Hit::Color {
                    self.color_rect = Rect::xywh(bx, by, bw, 28.0);
                }
                bx += bw + gap;
            }
            self.hits
                .push((Rect::xywh(bar.r - 3.0 - 40.0, by, 40.0, 28.0), Hit::Font));
            if self.color_pop {
                let pop = Rect::xywh(
                    self.color_rect.l,
                    bar.b + 4.0,
                    16.0 + 30.0 * SWATCHES.len() as f32,
                    38.0,
                );
                for i in 0..SWATCHES.len() {
                    self.overlay.push((
                        Rect::xywh(pop.l + 8.0 + 30.0 * i as f32, pop.t + 4.0, 30.0, 30.0),
                        Hit::Swatch(i),
                    ));
                }
            }
            y = bar.b + 8.0;
        }

        // Selection strip.
        if self.sel.len() > 1 && !archive {
            let chip = Rect::new(10.0, y, w - 10.0, y + 28.0);
            self.hits.push((
                Rect::xywh(chip.r - 44.0, chip.t + 2.0, 40.0, 24.0),
                Hit::ChipClear,
            ));
            self.hits.push((
                Rect::xywh(chip.r - 104.0, chip.t + 2.0, 58.0, 24.0),
                Hit::ChipDelete,
            ));
            y = chip.b + 6.0;
        }

        // Bottom: add box, toast.
        let mut bottom = if archive { h - 8.0 } else { h - 50.0 };
        self.add_rect = Rect::new(10.0, h - 50.0, w - 10.0, h - 10.0);
        if !archive {
            self.hits.push((
                Rect::xywh(self.add_rect.l + 7.0, self.add_rect.t + 7.0, 26.0, 26.0),
                Hit::AddKind,
            ));
        }
        if self.toast.is_some() && !archive {
            let t = Rect::new(10.0, bottom - 34.0, w - 10.0, bottom - 6.0);
            if self
                .toast
                .as_deref()
                .is_some_and(|s| s.starts_with("Undone"))
                && self.hist.redo_label().is_some()
            {
                self.hits.push((
                    Rect::xywh(t.r - 60.0, t.t + 2.0, 56.0, 24.0),
                    Hit::ToastRedo,
                ));
            }
            bottom = t.t;
        }
        self.list_rect = Rect::new(4.0, y, w - 4.0, bottom - 4.0);

        self.layout_rows();
        self.position_children();
    }

    fn layout_rows(&mut self) {
        let archive = self.view == View::Archive;
        let size = self.font_dip();
        let step = (size * 1.6).round();
        let bx = (size * 0.85).round() + 1.0;
        let line_h = size * 1.35;
        let list = self.list_rect;
        let family = self.settings.font_family.clone();
        let ids = self.doc.ids(self.view);

        let mut content = 0.0;
        let mut rows = Vec::with_capacity(ids.len());
        for id in &ids {
            let Some(item) = self.doc.get(*id).cloned() else {
                continue;
            };
            let left = list.l
                + 6.0
                + if archive {
                    0.0
                } else {
                    item.indent as f32 * step
                };
            let is_task = item.is_task();
            let text_x = left + if is_task { bx + 8.0 } else { 0.0 };
            let text_r = list.r - 6.0 - if archive { 112.0 } else { 0.0 };
            let editing = self.edit.as_ref().filter(|e| e.id == *id);
            let text = match editing {
                Some(e) => e.text.clone(),
                None => item.text.clone(),
            };
            let st = Style {
                bold: item.bold,
                italic: item.italic,
                underline: item.underline,
                strike: item.done,
                semibold: false,
            };
            let width = (text_r - text_x).max(20.0);
            let block = self.gfx.layout(
                if text.is_empty() { " " } else { &text },
                &family,
                size,
                st,
                width,
                true,
            );
            let th = block
                .as_ref()
                .map(|b| b.height)
                .unwrap_or(line_h)
                .max(line_h);
            let sub = if archive {
                let when = item
                    .archived_on
                    .as_deref()
                    .map(platform::short_date)
                    .unwrap_or_default();
                let s = format!("From {} · archived {}", item.list.label(), when);
                self.gfx
                    .layout(&s, UI_FONT, 11.0, Style::default(), width, false)
            } else {
                None
            };
            let row_h = th + 10.0 + if sub.is_some() { 16.0 } else { 0.0 };
            let check = is_task.then(|| Rect::xywh(left, 5.0 + (line_h - bx) / 2.0, bx, bx));
            rows.push(Row {
                id: *id,
                rect: Rect::new(list.l, content, list.r, content + row_h),
                text_x,
                text_y: 5.0,
                text_w: width,
                block,
                sub,
                check,
            });
            content += row_h + 1.0;
        }
        let max_scroll = (content - list.h()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let dy = list.t - self.scroll;
        for r in &mut rows {
            r.rect.t += dy;
            r.rect.b += dy;
            r.text_y += r.rect.t;
            if let Some(c) = &mut r.check {
                c.t += r.rect.t;
                c.b += r.rect.t;
            }
        }

        // Hits, most specific first.
        for r in &rows {
            if r.rect.b < list.t || r.rect.t > list.b {
                continue;
            }
            let item = self.doc.get(r.id).unwrap();
            let mid = r.rect.t + (r.rect.h() - 28.0) / 2.0;
            if archive {
                self.hits.push((
                    Rect::xywh(r.rect.r - 36.0, mid, 28.0, 28.0),
                    Hit::Purge(r.id),
                ));
                self.hits.push((
                    Rect::xywh(r.rect.r - 112.0, mid + 1.0, 70.0, 26.0),
                    Hit::Restore(r.id),
                ));
            } else if item.is_task() && item.done && self.hover == Some(r.id) && self.drag.is_none()
            {
                self.hits.push((
                    Rect::xywh(r.rect.r - 36.0, mid, 28.0, 28.0),
                    Hit::RowDelete(r.id),
                ));
                self.hits.push((
                    Rect::xywh(r.rect.r - 124.0, mid + 1.0, 84.0, 26.0),
                    Hit::RowArchive(r.id),
                ));
            }
            if let Some(c) = r.check {
                self.hits.push((c.inset(-4.0, -4.0), Hit::Check(r.id)));
            }
            self.hits.push((r.rect, Hit::Text(r.id)));
        }
        self.hits.push((list, Hit::ListBlank));
        self.rows = rows;
    }

    fn position_children(&self) {
        let s = self.scale();
        let px = |v: f32| (v * s).round() as i32;
        unsafe {
            let show_add = !self.settings.collapsed && self.view != View::Archive;
            if show_add {
                let a = self.add_rect;
                let eh = px(self.font_dip() * 1.45);
                let top = px(a.t + a.h() / 2.0) - eh / 2;
                let _ = SetWindowPos(
                    self.add,
                    None,
                    px(a.l + 42.0),
                    top,
                    px(a.w() - 52.0),
                    eh,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                let _ = ShowWindow(self.add, SW_HIDE);
            }
            if let Some(e) = &self.edit
                && let Some(r) = self.rows.iter().find(|r| r.id == e.id)
            {
                let h = r
                    .block
                    .as_ref()
                    .map(|b| b.height)
                    .unwrap_or(self.font_dip() * 1.35);
                let _ = SetWindowPos(
                    e.hwnd,
                    None,
                    px(r.text_x) - 3,
                    px(r.text_y),
                    px(r.text_w) + 6,
                    px(h) + 2,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn hit_test(&self, x: f32, y: f32) -> Option<Hit> {
        if let Some((_, h)) = self.overlay.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(*h);
        }
        for (r, h) in &self.hits {
            if !r.contains(x, y) {
                continue;
            }
            let in_list = matches!(
                h,
                Hit::Check(_)
                    | Hit::Text(_)
                    | Hit::RowArchive(_)
                    | Hit::RowDelete(_)
                    | Hit::Restore(_)
                    | Hit::Purge(_)
            );
            if in_list && !self.list_rect.contains(x, y) {
                continue;
            }
            return Some(*h);
        }
        None
    }

    fn row_at(&self, y: f32) -> Option<u64> {
        if y < self.list_rect.t || y > self.list_rect.b {
            return None;
        }
        self.rows
            .iter()
            .find(|r| y >= r.rect.t && y < r.rect.b)
            .map(|r| r.id)
    }

    // ------------------------------------------------------------ paint

    fn paint(&mut self) {
        self.relayout();
        if !self.gfx.begin(self.hwnd, self.dpi) {
            return;
        }
        self.gfx.clear(PANEL);
        let (w, h) = self.client_dips();
        if self.settings.collapsed {
            self.paint_pill(w, h);
        } else {
            if self.settings.header_hidden {
                self.paint_strip(w);
            } else {
                self.paint_header(w);
            }
            self.paint_format_bar(w);
            self.paint_chip(w);
            self.paint_rows();
            self.paint_bottom(w, h);
            self.paint_color_pop();
        }
        self.gfx.end();
    }

    fn hit_rect(&self, hit: Hit) -> Option<Rect> {
        self.hits
            .iter()
            .chain(self.overlay.iter())
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| *r)
    }

    fn paint_pill(&mut self, w: f32, h: f32) {
        self.gfx
            .icon(G_LIST, Rect::xywh(10.0, 0.0, 24.0, h), 16.0, ACCENT, 1.0);
        let semi = Style {
            semibold: true,
            ..Default::default()
        };
        self.gfx.text(
            "To-do",
            UI_FONT,
            14.0,
            semi,
            Rect::new(42.0, 0.0, w - 50.0, h),
            TEXT,
            1.0,
        );
        if let Some(r) = self.hit_rect(Hit::Expand) {
            self.gfx.icon(G_DOWN, r, 14.0, SUB, 1.0);
        }
    }

    fn paint_strip(&mut self, w: f32) {
        self.gfx
            .fill_round(Rect::xywh(w / 2.0 - 18.0, 4.0, 36.0, 4.0), 2.0, WHITE, 0.3);
    }

    fn paint_header(&mut self, _w: f32) {
        let semi = Style {
            semibold: true,
            ..Default::default()
        };
        if self.view == View::Archive {
            if let Some(r) = self.hit_rect(Hit::Back) {
                self.gfx
                    .icon(G_BACK, Rect::xywh(r.l, r.t, 24.0, r.h()), 13.0, TEXT, 1.0);
                self.gfx.text(
                    "Archive",
                    UI_FONT,
                    14.0,
                    semi,
                    Rect::new(r.l + 28.0, r.t, r.r, r.b),
                    TEXT,
                    1.0,
                );
            }
        } else {
            for l in [ListId::Next, ListId::Later] {
                let Some(r) = self.hit_rect(Hit::Tab(l)) else {
                    continue;
                };
                let on = self.view == View::List(l);
                let drop_here = self
                    .drag
                    .as_ref()
                    .is_some_and(|d| d.at == Some(DropAt::Tab(l)));
                if drop_here {
                    self.gfx.fill_round(r, 6.0, ACCENT, 0.25);
                    self.gfx
                        .stroke_round(r.inset(0.75, 0.75), 6.0, ACCENT, 1.0, 1.5);
                } else if on {
                    self.gfx.fill_round(r, 6.0, WHITE, 0.12);
                }
                let st = if on { semi } else { Style::default() };
                self.gfx.text(
                    l.label(),
                    UI_FONT,
                    13.0,
                    st,
                    r.inset(12.0, 0.0),
                    if on || drop_here { WHITE } else { SUB },
                    1.0,
                );
            }
        }
        let on_top = self.settings.on_top;
        for (hit, glyph) in [
            (Hit::Format, G_FONT),
            (Hit::Pin, if on_top { G_PIN_ON } else { G_PIN }),
            (Hit::More, G_MORE),
            (Hit::Collapse, G_UP),
        ] {
            let Some(r) = self.hit_rect(hit) else {
                continue;
            };
            let active = (hit == Hit::Format && self.fmt_open) || (hit == Hit::Pin && on_top);
            if active {
                let tint = if hit == Hit::Pin { ACCENT } else { WHITE };
                self.gfx
                    .fill_round(r, 6.0, tint, if hit == Hit::Pin { 0.2 } else { 0.14 });
            }
            let fg = if hit == Hit::Pin && on_top {
                ACCENT
            } else if active {
                WHITE
            } else {
                SUB
            };
            self.gfx.icon(glyph, r, 15.0, fg, 1.0);
        }
    }

    fn paint_format_bar(&mut self, _w: f32) {
        if !self.fmt_open || self.view == View::Archive {
            return;
        }
        let Some(b) = self.hit_rect(Hit::Bold) else {
            return;
        };
        let Some(font) = self.hit_rect(Hit::Font) else {
            return;
        };
        let bar = Rect::new(10.0, b.t - 3.0, font.r + 3.0, b.b + 3.0);
        self.gfx.fill_round(bar, 8.0, FIELD, 1.0);
        let ids = self.sel_ordered();
        let items: Vec<_> = ids
            .iter()
            .filter_map(|id| self.doc.get(*id).cloned())
            .collect();
        let has = !items.is_empty();
        let all = |f: fn(&crate::model::Item) -> bool| has && items.iter().all(f);
        let fg = if has { 0xD5DBE3 } else { FAINT };
        for (hit, label, st, on) in [
            (
                Hit::Bold,
                "B",
                Style {
                    bold: true,
                    ..Default::default()
                },
                all(|i| i.bold),
            ),
            (
                Hit::Italic,
                "I",
                Style {
                    italic: true,
                    ..Default::default()
                },
                all(|i| i.italic),
            ),
            (
                Hit::Underline,
                "U",
                Style {
                    underline: true,
                    ..Default::default()
                },
                all(|i| i.underline),
            ),
        ] {
            let Some(r) = self.hit_rect(hit) else {
                continue;
            };
            if on {
                self.gfx.fill_round(r, 6.0, WHITE, 0.16);
            }
            let family = if hit == Hit::Italic {
                "Georgia"
            } else {
                UI_FONT
            };
            let tw = self.gfx.measure(label, family, 14.0, st);
            self.gfx.text(
                label,
                family,
                14.0,
                st,
                Rect::new(r.l + (r.w() - tw) / 2.0, r.t, r.r, r.b),
                if on { WHITE } else { fg },
                1.0,
            );
        }
        // Separators.
        for after in [Hit::Underline, Hit::Color, Hit::Indent] {
            if let Some(r) = self.hit_rect(after) {
                self.gfx
                    .line(r.r + 4.5, r.t + 6.0, r.r + 4.5, r.b - 6.0, WHITE, 0.12, 1.0);
            }
        }
        if let Some(r) = self.hit_rect(Hit::Color) {
            if self.color_pop {
                self.gfx.fill_round(r, 6.0, WHITE, 0.16);
            }
            let c = items
                .first()
                .and_then(|i| i.color.as_deref().and_then(parse_hex))
                .unwrap_or(self.ink());
            self.gfx.circle(
                r.l + 13.0,
                r.t + r.h() / 2.0,
                7.0,
                c,
                if has { 1.0 } else { 0.4 },
            );
            self.gfx
                .stroke_circle(r.l + 13.0, r.t + r.h() / 2.0, 7.0, WHITE, 0.3, 1.0);
            self.gfx.icon(
                G_DOWN,
                Rect::xywh(r.l + 22.0, r.t, 14.0, r.h()),
                9.0,
                fg,
                1.0,
            );
        }
        for (hit, dir) in [(Hit::Outdent, -1.0f32), (Hit::Indent, 1.0)] {
            let Some(r) = self.hit_rect(hit) else {
                continue;
            };
            let (cx, cy) = (r.l + r.w() / 2.0, r.t + r.h() / 2.0);
            for (i, (x1, len)) in [(-7.0, 14.0), (-1.0, 8.0), (-1.0, 8.0)].iter().enumerate() {
                let y = cy - 5.0 + 5.0 * i as f32;
                self.gfx.line(cx + x1, y, cx + x1 + len, y, fg, 1.0, 1.5);
            }
            // Arrow head: points right for indent, left for outdent.
            let tip = if dir > 0.0 { cx - 3.0 } else { cx - 7.0 };
            let base = if dir > 0.0 { cx - 7.0 } else { cx - 3.0 };
            self.gfx.line(base, cy - 2.5, tip, cy, fg, 1.0, 1.5);
            self.gfx.line(base, cy + 2.5, tip, cy, fg, 1.0, 1.5);
        }
        if let Some(r) = self.hit_rect(Hit::KindToggle) {
            let is_task = items.first().map(|i| i.is_task()).unwrap_or(true);
            if has && is_task {
                self.gfx.fill_round(r, 6.0, WHITE, 0.16);
            }
            let b = Rect::xywh(r.l + 8.0, r.t + 8.0, 12.0, 12.0);
            self.gfx.stroke_round(b, 2.0, fg, 1.0, 1.5);
            self.gfx.icon(G_CHECK, b.inset(-1.0, -1.0), 9.0, fg, 1.0);
        }
        if let Some(r) = self.hit_rect(Hit::Font) {
            let geo = Style::default();
            self.gfx.text(
                "Aa",
                "Georgia",
                14.0,
                geo,
                Rect::new(r.l + 6.0, r.t, r.r, r.b),
                0xD5DBE3,
                1.0,
            );
            self.gfx.icon(
                G_DOWN,
                Rect::xywh(r.r - 14.0, r.t, 12.0, r.h()),
                9.0,
                0xD5DBE3,
                1.0,
            );
        }
    }

    fn paint_color_pop(&mut self) {
        if !self.color_pop || !self.fmt_open {
            return;
        }
        let Some(first) = self.overlay.first().map(|(r, _)| *r) else {
            return;
        };
        let pop = Rect::xywh(
            first.l - 8.0,
            first.t - 4.0,
            16.0 + 30.0 * SWATCHES.len() as f32,
            38.0,
        );
        self.gfx.fill_round(pop, 8.0, POPUP, 1.0);
        self.gfx
            .stroke_round(pop.inset(0.5, 0.5), 8.0, WHITE, 0.12, 1.0);
        let current = self
            .sel_ordered()
            .first()
            .and_then(|id| self.doc.get(*id))
            .and_then(|i| i.color.clone());
        let ink = self.ink();
        for (i, sw) in SWATCHES.iter().enumerate() {
            let r = self.overlay[i].0;
            let (cx, cy) = (r.l + 15.0, r.t + 15.0);
            self.gfx.circle(cx, cy, 10.0, sw.unwrap_or(ink), 1.0);
            let picked = current.as_deref().and_then(parse_hex) == *sw;
            self.gfx.stroke_circle(
                cx,
                cy,
                11.0,
                if picked { WHITE } else { 0x000000 },
                if picked { 1.0 } else { 0.25 },
                2.0,
            );
        }
    }

    fn paint_chip(&mut self, _w: f32) {
        if self.sel.len() <= 1 || self.view == View::Archive {
            return;
        }
        let Some(del) = self.hit_rect(Hit::ChipDelete) else {
            return;
        };
        let Some(esc) = self.hit_rect(Hit::ChipClear) else {
            return;
        };
        let chip = Rect::new(10.0, del.t - 2.0, esc.r + 4.0, del.b + 2.0);
        self.gfx.fill_round(chip, 8.0, ACCENT, 0.12);
        let n = self.sel.len();
        self.gfx.text(
            &format!("{n} lines selected"),
            UI_FONT,
            12.0,
            Style::default(),
            Rect::new(chip.l + 10.0, chip.t, del.l, chip.b),
            0xBFE3FF,
            1.0,
        );
        self.gfx.text(
            "Delete",
            UI_FONT,
            12.0,
            Style::default(),
            del.inset(8.0, 0.0),
            DANGER,
            1.0,
        );
        self.gfx.text(
            "Esc",
            UI_FONT,
            12.0,
            Style::default(),
            esc.inset(10.0, 0.0),
            0xD5DBE3,
            1.0,
        );
    }

    fn paint_rows(&mut self) {
        let list = self.list_rect;
        let archive = self.view == View::Archive;
        let ink = self.ink();
        let multi = self.sel.len() > 1;
        let drag_ids: Vec<u64> = self
            .drag
            .as_ref()
            .map(|d| d.group.clone())
            .unwrap_or_default();
        let drop = self.drag.as_ref().and_then(|d| d.at);
        let editing = self.edit.as_ref().map(|e| e.id);
        let rows = std::mem::take(&mut self.rows);
        self.gfx.clip(list);
        if rows.is_empty() {
            let msg = if archive {
                "Nothing archived."
            } else {
                "No tasks. Add one below."
            };
            let tw = self.gfx.measure(msg, UI_FONT, 13.0, Style::default());
            self.gfx.text(
                msg,
                UI_FONT,
                13.0,
                Style::default(),
                Rect::xywh(
                    list.l + (list.w() - tw) / 2.0,
                    list.t + 20.0,
                    tw + 2.0,
                    24.0,
                ),
                MUTED,
                1.0,
            );
        }
        for r in &rows {
            if r.rect.b < list.t || r.rect.t > list.b {
                continue;
            }
            let Some(item) = self.doc.get(r.id).cloned() else {
                continue;
            };
            let selected = !archive && self.sel.contains(&r.id);
            let hovered = !archive && self.hover == Some(r.id) && self.drag.is_none();
            let bg = r.rect.inset(2.0, 0.0);
            if selected {
                if multi {
                    self.gfx.fill_round(bg, 6.0, ACCENT, 0.18)
                } else {
                    self.gfx.fill_round(bg, 6.0, WHITE, 0.07)
                }
            } else if hovered {
                self.gfx.fill_round(bg, 6.0, WHITE, 0.035);
            }
            let dim = if drag_ids.contains(&r.id) { 0.4 } else { 1.0 };
            if let Some(c) = r.check {
                self.gfx
                    .stroke_round(c.inset(0.75, 0.75), 3.0, MUTED, dim, 1.5);
                if item.done {
                    self.gfx
                        .icon(G_CHECK, c, (c.w() * 0.78).max(7.0), CHECK, dim);
                }
            }
            if editing == Some(r.id) {
                let h = r.block.as_ref().map(|b| b.height).unwrap_or(16.0);
                self.gfx.stroke_round(
                    Rect::xywh(r.text_x - 4.0, r.text_y - 1.0, r.text_w + 8.0, h + 3.0),
                    4.0,
                    ACCENT,
                    1.0,
                    1.5,
                );
            } else if let Some(b) = &r.block {
                let c = item.color.as_deref().and_then(parse_hex).unwrap_or(ink);
                let a = if item.done { 0.5 } else { 1.0 };
                self.gfx.draw(b, r.text_x, r.text_y, c, a * dim);
            }
            if let Some(s) = &r.sub {
                let y = r.text_y + r.block.as_ref().map(|b| b.height).unwrap_or(16.0) + 1.0;
                self.gfx.draw(s, r.text_x, y, MUTED, 1.0);
            }
            if let Some(DropAt::Row(t, after)) = drop
                && t == r.id
            {
                let y = if after { r.rect.b } else { r.rect.t };
                self.gfx
                    .line(r.rect.l + 4.0, y, r.rect.r - 4.0, y, ACCENT, 1.0, 2.0);
            }
            // Row buttons.
            let solid = if selected { SEL_SOLID } else { HOVER_SOLID };
            if let Some(a) = self.hit_rect(Hit::RowArchive(r.id)) {
                let d = self.hit_rect(Hit::RowDelete(r.id)).unwrap_or(a);
                self.gfx.fill(
                    Rect::new(a.l - 8.0, r.rect.t + 1.0, d.r + 2.0, r.rect.b - 1.0),
                    solid,
                    1.0,
                );
                self.gfx.fill_round(a, 6.0, WHITE, 0.08);
                self.archive_icon(Rect::xywh(a.l + 8.0, a.t + 5.0, 16.0, 16.0), 0xD5DBE3);
                self.gfx.text(
                    "Archive",
                    UI_FONT,
                    12.0,
                    Style::default(),
                    Rect::new(a.l + 30.0, a.t, a.r, a.b),
                    0xD5DBE3,
                    1.0,
                );
                self.gfx.icon(G_DELETE, d, 14.0, DANGER, 1.0);
            }
            if let Some(rs) = self.hit_rect(Hit::Restore(r.id)) {
                self.gfx
                    .stroke_round(rs.inset(0.5, 0.5), 6.0, WHITE, 0.16, 1.0);
                let tw = self.gfx.measure("Restore", UI_FONT, 12.0, Style::default());
                self.gfx.text(
                    "Restore",
                    UI_FONT,
                    12.0,
                    Style::default(),
                    Rect::new(rs.l + (rs.w() - tw) / 2.0, rs.t, rs.r, rs.b),
                    0xD5DBE3,
                    1.0,
                );
                if let Some(p) = self.hit_rect(Hit::Purge(r.id)) {
                    self.gfx.icon(G_DELETE, p, 14.0, DANGER, 1.0);
                }
            }
        }
        self.gfx.unclip();
        // Scroll indicator.
        let content: f32 = rows.iter().map(|r| r.rect.h() + 1.0).sum();
        if content > list.h() + 1.0 {
            let frac = list.h() / content;
            let th = (list.h() * frac).max(24.0);
            let top = list.t + (list.h() - th) * (self.scroll / (content - list.h()));
            self.gfx
                .fill_round(Rect::xywh(list.r - 3.0, top, 3.0, th), 1.5, WHITE, 0.22);
        }
        self.rows = rows;
    }

    fn archive_icon(&mut self, r: Rect, fg: u32) {
        self.gfx.stroke_round(
            Rect::new(r.l + 1.0, r.t + 2.0, r.r - 1.0, r.t + 6.0),
            1.0,
            fg,
            1.0,
            1.4,
        );
        self.gfx.stroke_round(
            Rect::new(r.l + 2.5, r.t + 6.0, r.r - 2.5, r.b - 1.5),
            1.0,
            fg,
            1.0,
            1.4,
        );
        self.gfx
            .line(r.l + 6.5, r.t + 9.5, r.r - 6.5, r.t + 9.5, fg, 1.0, 1.4);
    }

    fn paint_bottom(&mut self, w: f32, _h: f32) {
        let archive = self.view == View::Archive;
        if let Some(t) = self.toast.clone()
            && !archive
        {
            let r = Rect::new(
                10.0,
                self.list_rect.b + 4.0,
                w - 10.0,
                self.list_rect.b + 32.0,
            );
            self.gfx.fill_round(r, 8.0, POPUP, 1.0);
            self.gfx
                .stroke_round(r.inset(0.5, 0.5), 8.0, WHITE, 0.10, 1.0);
            self.gfx.text(
                &t,
                UI_FONT,
                12.0,
                Style::default(),
                Rect::new(r.l + 10.0, r.t, r.r - 64.0, r.b),
                0xD5DBE3,
                1.0,
            );
            if let Some(b) = self.hit_rect(Hit::ToastRedo) {
                self.gfx.text(
                    "Redo",
                    UI_FONT,
                    12.0,
                    Style::default(),
                    b.inset(12.0, 0.0),
                    ACCENT,
                    1.0,
                );
            }
        }
        if archive {
            return;
        }
        let a = self.add_rect;
        self.gfx.fill_round(a, 8.0, FIELD, 1.0);
        self.gfx
            .stroke_round(a.inset(0.5, 0.5), 8.0, WHITE, 0.10, 1.0);
        if let Some(k) = self.hit_rect(Hit::AddKind) {
            self.gfx
                .stroke_round(k.inset(0.5, 0.5), 5.0, WHITE, 0.14, 1.0);
            if self.new_kind == Kind::Task {
                self.gfx.stroke_round(
                    Rect::xywh(k.l + 7.5, k.t + 7.5, 11.0, 11.0),
                    2.0,
                    SUB,
                    1.0,
                    1.5,
                );
            } else {
                let st = Style {
                    bold: true,
                    ..Default::default()
                };
                let tw = self.gfx.measure("T", "Georgia", 14.0, st);
                self.gfx.text(
                    "T",
                    "Georgia",
                    14.0,
                    st,
                    Rect::new(k.l + (k.w() - tw) / 2.0, k.t, k.r, k.b),
                    SUB,
                    1.0,
                );
            }
        }
    }

    // ------------------------------------------------------------ input

    fn dips(&self, lp: LPARAM) -> (f32, f32) {
        let s = self.scale();
        (lo(lp.0) as f32 / s, hi(lp.0) as f32 / s)
    }

    fn nc_hit(&self, lp: LPARAM) -> LRESULT {
        let mut pt = POINT {
            x: lo(lp.0),
            y: hi(lp.0),
        };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut pt);
        }
        let s = self.scale();
        let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
        let (w, h) = self.client_dips();
        if !self.settings.locked {
            let (l, r) = (x < EDGE, x >= w - EDGE);
            let (t, b) = (
                y < EDGE && !self.settings.collapsed,
                y >= h - EDGE && !self.settings.collapsed,
            );
            let code = match (l, r, t, b) {
                (true, _, true, _) => HTTOPLEFT,
                (_, true, true, _) => HTTOPRIGHT,
                (true, _, _, true) => HTBOTTOMLEFT,
                (_, true, _, true) => HTBOTTOMRIGHT,
                (true, ..) => HTLEFT,
                (_, true, ..) => HTRIGHT,
                (_, _, true, _) => HTTOP,
                (_, _, _, true) => HTBOTTOM,
                _ => HTCLIENT,
            };
            if code != HTCLIENT {
                return LRESULT(code as isize);
            }
            if self.hit_test(x, y) == Some(Hit::Caption) {
                return LRESULT(HTCAPTION as isize);
            }
        }
        LRESULT(HTCLIENT as isize)
    }

    fn on_down(&mut self, lp: LPARAM) -> Option<Deferred> {
        let (x, y) = self.dips(lp);
        let hit = self.hit_test(x, y);
        if self.color_pop && !matches!(hit, Some(Hit::Swatch(_)) | Some(Hit::Color)) {
            self.color_pop = false;
            self.invalidate();
        }
        let editing_hit =
            matches!(hit, Some(Hit::Text(id)) if self.edit.as_ref().is_some_and(|e| e.id == id));
        if !editing_hit {
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
        }
        let hit = hit?;
        match hit {
            Hit::Text(id) => {
                if self.view != View::Archive {
                    self.press = Some(Press {
                        id,
                        x: lo(lp.0),
                        y: hi(lp.0),
                    });
                    unsafe {
                        SetCapture(self.hwnd);
                    }
                }
            }
            Hit::Check(id) => {
                if let Some(item) = self.doc.get(id) {
                    let label = if item.done { "Uncheck" } else { "Check off" };
                    self.commit_edit();
                    self.change(label, |d| d.apply(&[id], |i| i.done = !i.done));
                    self.refresh_edit_font();
                }
            }
            Hit::Tab(l) => self.set_view(View::List(l)),
            Hit::Back => self.set_view(View::List(self.settings.view)),
            Hit::Format => {
                self.fmt_open = !self.fmt_open;
                self.color_pop = false;
            }
            Hit::Pin => self.set_on_top(!self.settings.on_top),
            Hit::More => {
                let r = self.hit_rect(Hit::More).unwrap_or_default();
                let s = self.scale();
                let mut pt = POINT {
                    x: (r.l * s) as i32,
                    y: (r.b * s) as i32,
                };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut pt);
                }
                return Some(Deferred::Menu {
                    entries: self.more_menu(),
                    x: pt.x,
                    y: pt.y,
                });
            }
            Hit::Collapse => self.set_collapsed(true),
            Hit::Expand => self.set_collapsed(false),
            Hit::Bold => self.toggle_style(0),
            Hit::Italic => self.toggle_style(1),
            Hit::Underline => self.toggle_style(2),
            Hit::Color => {
                if !self.sel.is_empty() {
                    self.color_pop = !self.color_pop;
                }
            }
            Hit::Swatch(i) => {
                let c = SWATCHES[i].map(to_hex);
                self.color_pop = false;
                self.format_sel("Color", move |item, _| item.color = c.clone());
            }
            Hit::Outdent => self.indent_sel(-1),
            Hit::Indent => self.indent_sel(1),
            Hit::KindToggle => {
                let to_task = self
                    .sel_ordered()
                    .first()
                    .and_then(|id| self.doc.get(*id))
                    .is_some_and(|i| !i.is_task());
                self.format_sel("Change line type", move |item, _| {
                    item.kind = if to_task { Kind::Task } else { Kind::Text };
                    item.done = false;
                });
            }
            Hit::Font => return Some(Deferred::FontDialog),
            Hit::ChipDelete => self.delete_sel(),
            Hit::ChipClear => self.sel.clear(),
            Hit::ToastRedo => self.redo(),
            Hit::AddKind => {
                self.new_kind = if self.new_kind == Kind::Task {
                    Kind::Text
                } else {
                    Kind::Task
                };
                self.refresh_cue();
                unsafe {
                    let _ = SetFocus(Some(self.add));
                }
            }
            Hit::RowArchive(id) => {
                self.commit_edit();
                let today = platform::today();
                self.change("Archive", |d| {
                    d.archive(&[id], &today);
                });
            }
            Hit::RowDelete(id) => {
                self.commit_edit();
                self.change("Delete", |d| d.remove(&[id]));
            }
            Hit::Restore(id) => self.change("Restore", |d| d.restore(id)),
            Hit::Purge(id) => self.change("Delete from archive", |d| d.purge(id)),
            Hit::ShowHeader => self.set_header_hidden(false),
            Hit::ListBlank => {
                // Like a text editor: clicking below the last line starts a
                // new text line (discarded if nothing is typed).
                self.sel.clear();
                self.anchor = None;
                if self.view != View::Archive {
                    self.start_new_line(None, Kind::Text, 0);
                    return None;
                }
                self.commit_edit();
            }
            Hit::Caption => {}
        }
        self.relayout();
        self.invalidate();
        None
    }

    fn on_move(&mut self, lp: LPARAM) {
        let (x, y) = self.dips(lp);
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking = true;
        }
        if let Some(p) = &self.press
            && self.drag.is_none()
        {
            let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if (lo(lp.0) - p.x).abs() > cx || (hi(lp.0) - p.y).abs() > cy {
                let id = p.id;
                self.commit_edit();
                let group = if self.sel.contains(&id) && self.sel.len() > 1 {
                    self.sel_ordered()
                } else {
                    vec![id]
                };
                self.drag = Some(Drag { group, at: None });
            }
        }
        if self.drag.is_some() {
            let at = self.drop_target(x, y);
            if let Some(d) = &mut self.drag
                && d.at != at
            {
                d.at = at;
                self.invalidate();
            }
            return;
        }
        let hover = self.row_at(y).filter(|_| self.list_rect.contains(x, y));
        if hover != self.hover {
            self.hover = hover;
            self.relayout();
            self.invalidate();
        }
    }

    fn drop_target(&self, x: f32, y: f32) -> Option<DropAt> {
        let d = self.drag.as_ref()?;
        if let View::List(cur) = self.view {
            for l in [ListId::Next, ListId::Later] {
                if l != cur && self.hit_rect(Hit::Tab(l)).is_some_and(|r| r.contains(x, y)) {
                    return Some(DropAt::Tab(l));
                }
            }
        }
        if !self.list_rect.contains(x, y) {
            return None;
        }
        let order = self.doc.ids(self.view);
        let first = order.iter().position(|id| d.group.contains(id))?;
        let target = self
            .row_at(y)
            .or_else(|| self.rows.last().filter(|r| y >= r.rect.b).map(|r| r.id))?;
        if d.group.contains(&target) {
            return None;
        }
        let ti = order.iter().position(|id| *id == target)?;
        Some(DropAt::Row(target, ti > first))
    }

    fn on_up(&mut self, lp: LPARAM) {
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(d) = self.drag.take() {
            self.press = None;
            let label = Self::plural("Move", d.group.len());
            match d.at {
                Some(DropAt::Row(t, after)) => {
                    self.change(&label, |doc| doc.move_group(&d.group, t, after))
                }
                Some(DropAt::Tab(l)) => {
                    self.change(&format!("{label} to {}", l.label()), |doc| {
                        doc.move_to_list(&d.group, l)
                    });
                    self.sel.clear();
                }
                None => {}
            }
            self.relayout();
            self.invalidate();
            return;
        }
        let Some(p) = self.press.take() else { return };
        let id = p.id;
        let order = self.doc.ids(self.view);
        if key_down(VK_SHIFT.0) && self.anchor.is_some_and(|a| order.contains(&a)) {
            self.commit_edit();
            let a = order.iter().position(|x| Some(*x) == self.anchor).unwrap();
            let b = order.iter().position(|x| *x == id).unwrap_or(a);
            let (lo_i, hi_i) = if a < b { (a, b) } else { (b, a) };
            self.sel = order[lo_i..=hi_i].to_vec();
        } else if key_down(VK_CONTROL.0) {
            self.commit_edit();
            if let Some(pos) = self.sel.iter().position(|x| *x == id) {
                self.sel.remove(pos);
            } else {
                self.sel.push(id);
            }
            self.anchor = Some(id);
        } else {
            self.sel = vec![id];
            self.anchor = Some(id);
            if self.edit.as_ref().is_none_or(|e| e.id != id) {
                self.start_edit(id, Some((lo(lp.0), hi(lp.0))));
            }
        }
        self.relayout();
        self.invalidate();
    }

    fn on_wheel(&mut self, wp: WPARAM) {
        let delta = hi(wp.0 as isize) as f32;
        self.commit_edit();
        self.scroll -= delta / 120.0 * 48.0;
        self.relayout();
        self.invalidate();
    }

    fn on_key(&mut self, vk: u16) -> bool {
        let ctrl = key_down(VK_CONTROL.0);
        let shift = key_down(VK_SHIFT.0);
        match vk {
            v if ctrl && v == b'Z' as u16 && !shift => self.undo(),
            v if ctrl && (v == b'Y' as u16 || (v == b'Z' as u16 && shift)) => self.redo(),
            v if ctrl && v == b'B' as u16 => self.toggle_style(0),
            v if ctrl && v == b'I' as u16 => self.toggle_style(1),
            v if ctrl && v == b'U' as u16 => self.toggle_style(2),
            v if ctrl && v == b'H' as u16 => self.set_header_hidden(!self.settings.header_hidden),
            v if v == VK_DELETE.0 || v == VK_BACK.0 => self.delete_sel(),
            v if v == VK_TAB.0 => self.indent_sel(if shift { -1 } else { 1 }),
            v if v == VK_ESCAPE.0 => {
                self.sel.clear();
                self.anchor = None;
                self.color_pop = false;
            }
            v if v == VK_RETURN.0 && self.sel.len() == 1 => {
                let id = self.sel[0];
                self.start_edit(id, None);
            }
            _ => return false,
        }
        self.relayout();
        self.invalidate();
        true
    }

    fn on_alt_arrow(&mut self, up: bool) -> bool {
        let View::List(l) = self.view else {
            return false;
        };
        let ids = self.sel_ordered();
        if ids.is_empty() {
            return false;
        }
        self.commit_edit();
        let mut probe = Doc::new(self.doc.tasks.clone(), Vec::new());
        if probe.nudge(l, &ids, up) {
            self.change(&Self::plural("Move", ids.len()), |d| {
                d.nudge(l, &ids, up);
            });
        }
        true
    }

    fn on_edit_cmd(&mut self, code: usize, from: HWND) {
        let Some(e) = &self.edit else { return };
        if e.hwnd != from {
            return;
        }
        let id = e.id;
        let fresh = e.fresh.is_some();
        match code {
            cmd::ENTER => {
                // Enter opens the next line, like an editor; Enter on an empty
                // new line just stops.
                let typed = !self.edit_text().unwrap_or_default().trim().is_empty();
                let (kind, indent) = self
                    .doc
                    .get(id)
                    .map(|i| (i.kind, i.indent))
                    .unwrap_or((Kind::Text, 0));
                if fresh && !typed {
                    self.commit_edit();
                } else {
                    self.start_new_line(Some(id), kind, indent);
                }
            }
            cmd::KILL_FOCUS => self.commit_edit(),
            cmd::HEADER => self.set_header_hidden(!self.settings.header_hidden),
            cmd::ESC => self.cancel_edit(),
            cmd::TAB | cmd::SHIFT_TAB => {
                if !self.sel.contains(&id) {
                    self.sel = vec![id];
                }
                self.indent_sel(if code == cmd::TAB { 1 } else { -1 });
            }
            cmd::BACK_EMPTY => {
                self.cancel_edit();
                if !fresh {
                    self.change("Delete", |d| d.remove(&[id]));
                }
            }
            cmd::BOLD => self.toggle_style(0),
            cmd::ITALIC => self.toggle_style(1),
            cmd::UNDERLINE => self.toggle_style(2),
            _ => {}
        }
        self.relayout();
        self.invalidate();
    }

    fn on_add_enter(&mut self) {
        let View::List(list) = self.view else { return };
        let text = unsafe {
            let len = GetWindowTextLengthW(self.add) as usize;
            let mut buf = vec![0u16; len + 1];
            let n = GetWindowTextW(self.add, &mut buf) as usize;
            String::from_utf16_lossy(&buf[..n])
        };
        let text = text.trim().to_string();
        if text.is_empty() && self.new_kind == Kind::Task {
            return;
        }
        let kind = self.new_kind;
        self.change(
            if kind == Kind::Task {
                "Add task"
            } else {
                "Add line"
            },
            |d| {
                d.add(list, kind, &text);
            },
        );
        unsafe {
            let _ = SetWindowTextW(self.add, w!(""));
        }
        self.scroll = f32::MAX;
        self.relayout();
        self.invalidate();
    }

    // ------------------------------------------------------------ window state

    fn set_on_top(&mut self, on: bool) {
        self.settings.on_top = on;
        self.raised = false;
        unsafe {
            let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
            let _ = SetWindowPos(
                self.hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.store.save_settings(&self.settings);
        self.invalidate();
    }

    fn set_collapsed(&mut self, collapsed: bool) {
        if collapsed == self.settings.collapsed {
            return;
        }
        self.commit_edit();
        let mut rc = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rc);
        }
        let s = self.scale();
        let height = if collapsed {
            self.settings.expanded_height = Some(rc.bottom - rc.top);
            (PILL_H * s).round() as i32
        } else {
            self.settings
                .expanded_height
                .unwrap_or((460.0 * s) as i32)
                .max((MIN_H * s) as i32)
        };
        self.settings.collapsed = collapsed;
        self.color_pop = false;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                rc.right - rc.left,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.save_settings();
        self.relayout();
        self.invalidate();
    }

    pub fn toggle_visible(&mut self) {
        unsafe {
            if IsWindowVisible(self.hwnd).as_bool() {
                self.commit_edit();
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    fn sentinel_to_bottom(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.sentinel,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Show desktop raised the desktop over every normal window: lift the
    /// widget above it until some other window comes to the foreground.
    fn check_show_desktop(&mut self) {
        if self.settings.on_top || self.raised || !unsafe { IsWindowVisible(self.hwnd) }.as_bool() {
            return;
        }
        if platform::show_desktop_active(SENTINEL_CLASS) {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            self.raised = true;
        } else if self.desktop_tries < 5 {
            // The z-order can settle a moment after the foreground change.
            self.desktop_tries += 1;
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_DESKTOP, 10, None);
            }
        }
    }

    fn end_show_desktop(&mut self, foreground: HWND) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_DESKTOP);
        }
        if !self.raised {
            return;
        }
        self.raised = false;
        if !self.settings.on_top {
            unsafe {
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
                // Tuck in just under the window the user switched to.
                let _ = SetWindowPos(self.hwnd, Some(foreground), 0, 0, 0, 0, flags);
            }
        }
        self.sentinel_to_bottom();
    }

    // ------------------------------------------------------------ menus

    fn more_menu(&self) -> Vec<MenuEntry> {
        let undo = self
            .hist
            .undo_label()
            .map(|l| format!("Undo {}\tCtrl+Z", l.to_lowercase()));
        let redo = self
            .hist
            .redo_label()
            .map(|l| format!("Redo {}\tCtrl+Y", l.to_lowercase()));
        let finished = self.doc.finished_ids().len();
        vec![
            MenuEntry::item(
                menu_id::UNDO,
                undo.clone().unwrap_or("Nothing to undo".into()),
                false,
                undo.is_some(),
            ),
            MenuEntry::item(
                menu_id::REDO,
                redo.clone().unwrap_or("Nothing to redo".into()),
                false,
                redo.is_some(),
            ),
            MenuEntry::sep(),
            MenuEntry::item(
                menu_id::ARCHIVE_FINISHED,
                format!("Archive finished tasks ({finished})"),
                false,
                finished > 0,
            ),
            MenuEntry::item(
                menu_id::OPEN_ARCHIVE,
                format!("Open archive ({})", self.doc.archive.len()),
                false,
                true,
            ),
            MenuEntry::sep(),
            MenuEntry::item(
                menu_id::LOCK,
                "Lock position and size",
                self.settings.locked,
                true,
            ),
            MenuEntry::item(
                menu_id::AUTOSTART,
                "Start with Windows",
                platform::autostart_enabled(),
                true,
            ),
            MenuEntry::sep(),
            MenuEntry::item(menu_id::HEADER, "Hide header\tCtrl+H", false, true),
            MenuEntry::item(menu_id::HIDE, "Hide to tray", false, true),
            MenuEntry::item(menu_id::QUIT, "Quit", false, true),
        ]
    }

    fn tray_menu(&self) -> Vec<MenuEntry> {
        let visible = unsafe { IsWindowVisible(self.hwnd) }.as_bool();
        vec![
            MenuEntry::item(
                menu_id::SHOW_HIDE,
                if visible { "Hide" } else { "Show" },
                false,
                true,
            ),
            MenuEntry::item(
                menu_id::HEADER,
                if self.settings.header_hidden {
                    "Show header"
                } else {
                    "Hide header"
                },
                false,
                true,
            ),
            MenuEntry::item(menu_id::ON_TOP, "Always on top", self.settings.on_top, true),
            MenuEntry::sep(),
            MenuEntry::item(menu_id::QUIT, "Quit", false, true),
        ]
    }

    pub fn menu_command(&mut self, id: u32) {
        match id {
            menu_id::UNDO => self.undo(),
            menu_id::REDO => self.redo(),
            menu_id::ARCHIVE_FINISHED => {
                self.commit_edit();
                let ids = self.doc.finished_ids();
                let today = platform::today();
                self.change("Archive finished tasks", |d| {
                    d.archive(&ids, &today);
                });
            }
            menu_id::OPEN_ARCHIVE => self.set_view(View::Archive),
            menu_id::LOCK => {
                self.settings.locked = !self.settings.locked;
                self.store.save_settings(&self.settings);
            }
            menu_id::AUTOSTART => platform::set_autostart(!platform::autostart_enabled()),
            menu_id::HIDE | menu_id::SHOW_HIDE => self.toggle_visible(),
            menu_id::HEADER => self.set_header_hidden(!self.settings.header_hidden),
            menu_id::ON_TOP => self.set_on_top(!self.settings.on_top),
            menu_id::QUIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_QUIT_APP, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
        self.relayout();
        self.invalidate();
    }

    pub fn font_dialog_init(&self) -> (String, f32, u32, u32) {
        (
            self.settings.font_family.clone(),
            self.settings.font_size_pt,
            self.ink(),
            self.dpi,
        )
    }

    pub fn apply_font(&mut self, family: String, size_pt: f32, ink: u32) {
        self.settings.font_family = family;
        self.settings.font_size_pt = size_pt.clamp(6.0, 36.0);
        self.settings.ink = to_hex(ink);
        self.store.save_settings(&self.settings);
        self.refresh_add_font();
        self.refresh_edit_font();
        self.relayout();
        self.invalidate();
    }

    // ------------------------------------------------------------ dispatch

    pub fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Out {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(self.hwnd, &mut ps);
                }
                self.paint();
                unsafe {
                    let _ = EndPaint(self.hwnd, &ps);
                }
                Out::Done(LRESULT(0))
            }
            WM_ERASEBKGND => Out::Done(LRESULT(1)),
            WM_SIZE => {
                self.relayout();
                self.invalidate();
                Out::Done(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.dpi = hi(wp.0 as isize) as u32;
                let r = unsafe { &*(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                self.refresh_add_font();
                self.refresh_edit_font();
                self.relayout();
                self.invalidate();
                Out::Done(LRESULT(0))
            }
            WM_NCHITTEST => Out::Done(self.nc_hit(lp)),
            WM_LBUTTONDOWN => match self.on_down(lp) {
                Some(d) => Out::Defer(d),
                None => Out::Done(LRESULT(0)),
            },
            WM_MOUSEMOVE => {
                self.on_move(lp);
                Out::Done(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.on_up(lp);
                Out::Done(LRESULT(0))
            }
            WM_RBUTTONUP => {
                // Right-click (or a two-finger tap) switches task <-> text line.
                let (x, y) = self.dips(lp);
                if let Some(Hit::Text(id) | Hit::Check(id)) = self.hit_test(x, y) {
                    self.toggle_kind(id);
                }
                Out::Done(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if self.hover.take().is_some() {
                    self.relayout();
                    self.invalidate();
                }
                Out::Done(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                if self.drag.take().is_some() {
                    self.invalidate();
                }
                self.press = None;
                Out::Done(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                self.on_wheel(wp);
                Out::Done(LRESULT(0))
            }
            WM_KEYDOWN => {
                if self.on_key(wp.0 as u16) {
                    Out::Done(LRESULT(0))
                } else {
                    Out::Default
                }
            }
            WM_SYSKEYDOWN => {
                let vk = wp.0 as u16;
                if key_down(VK_MENU.0)
                    && (vk == VK_UP.0 || vk == VK_DOWN.0)
                    && self.on_alt_arrow(vk == VK_UP.0)
                {
                    self.relayout();
                    self.invalidate();
                    Out::Done(LRESULT(0))
                } else {
                    Out::Default
                }
            }
            WM_COMMAND => {
                let id = wp.0 & 0xFFFF;
                let code = (wp.0 >> 16) as u32 & 0xFFFF;
                if id == ID_EDIT
                    && code == EN_CHANGE
                    && let Some(t) = self.edit_text()
                {
                    if let Some(e) = &mut self.edit {
                        e.text = t;
                    }
                    self.relayout();
                    self.invalidate();
                }
                Out::Done(LRESULT(0))
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as *mut _);
                let child = HWND(lp.0 as *mut _);
                let (fg, bg, brush) = if self.edit.as_ref().is_some_and(|e| e.hwnd == child) {
                    let id = self.edit.as_ref().unwrap().id;
                    let item = self.doc.get(id);
                    let c = item
                        .and_then(|i| i.color.as_deref().and_then(parse_hex))
                        .unwrap_or(self.ink());
                    (c, SEL_SOLID, self.edit_brush)
                } else {
                    (self.ink(), FIELD, self.field_brush)
                };
                unsafe {
                    SetTextColor(hdc, colorref(fg));
                    SetBkColor(hdc, colorref(bg));
                }
                Out::Done(LRESULT(brush.0 as isize))
            }
            WM_EDIT_CMD => {
                self.on_edit_cmd(wp.0, HWND(lp.0 as *mut _));
                Out::Done(LRESULT(0))
            }
            WM_ADD_ENTER => {
                self.on_add_enter();
                Out::Done(LRESULT(0))
            }
            WM_TRAY => {
                let event = (lp.0 & 0xFFFF) as u32;
                if event == WM_LBUTTONUP {
                    self.toggle_visible();
                } else if event == WM_RBUTTONUP {
                    let mut pt = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut pt);
                    }
                    return Out::Defer(Deferred::Menu {
                        entries: self.tray_menu(),
                        x: pt.x,
                        y: pt.y,
                    });
                }
                Out::Done(LRESULT(0))
            }
            WM_DESKTOP_FOREGROUND => {
                self.desktop_tries = 0;
                self.check_show_desktop();
                Out::Done(LRESULT(0))
            }
            WM_OTHER_FOREGROUND => {
                self.end_show_desktop(HWND(wp.0 as *mut _));
                Out::Done(LRESULT(0))
            }
            WM_TIMER if wp.0 == TIMER_DESKTOP => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_DESKTOP);
                }
                self.check_show_desktop();
                Out::Done(LRESULT(0))
            }
            WM_EXITSIZEMOVE => {
                self.save_settings();
                Out::Done(LRESULT(0))
            }
            WM_CLOSE => {
                // Alt+F4 hides to the tray; Quit is in the menus.
                self.toggle_visible();
                Out::Done(LRESULT(0))
            }
            WM_QUIT_APP => {
                self.commit_edit();
                self.save_settings();
                Out::Destroy
            }
            WM_DESTROY => {
                platform::tray_remove(self.hwnd);
                unsafe {
                    PostQuitMessage(0);
                }
                Out::Done(LRESULT(0))
            }
            m if m == self.taskbar_created && m != 0 => {
                platform::tray_add(self.hwnd, self.tray_icon);
                self.sentinel_to_bottom();
                Out::Done(LRESULT(0))
            }
            _ => Out::Default,
        }
    }
}
