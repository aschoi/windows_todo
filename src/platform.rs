//! Small Windows services: tray icon, Start with Windows, local date, and
//! detecting Show desktop (Win+D).

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, FindWindowExW, GetClassNameW, GetShellWindow, HICON, IsWindowVisible,
    PostMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::core::{PCWSTR, w};

pub const WM_TRAY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 10;
pub const WM_DESKTOP_FOREGROUND: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 11;
pub const WM_OTHER_FOREGROUND: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 12;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Local date as "YYYY-MM-DD".
pub fn today() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// "2026-09-30" -> "Sep 30".
pub fn short_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = iso.split('-');
    let (_, m, d) = (parts.next(), parts.next(), parts.next());
    match (
        m.and_then(|m| m.parse::<usize>().ok()),
        d.and_then(|d| d.parse::<u32>().ok()),
    ) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {}", MONTHS[m - 1], d),
        _ => iso.to_string(),
    }
}

// ---------------------------------------------------------------- tray

pub fn tray_add(hwnd: HWND, icon: HICON) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    for (dst, src) in nid.szTip.iter_mut().zip("To-do".encode_utf16()) {
        *dst = src;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub fn tray_remove(hwnd: HWND) {
    let nid = tray_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

// ---------------------------------------------------------------- Start with Windows

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("TodoWidget");

pub fn autostart_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let cmd = wide(&format!("\"{}\"", exe.display()));
                let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
                let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

// ---------------------------------------------------------------- Show desktop (Win+D)
//
// Same approach as Rainmeter (Library/System.cpp): watch foreground changes;
// when the shell's desktop window comes to the foreground, Show desktop is on
// exactly when that window now sits above a helper window we keep at the
// bottom of the z-order.

static TARGET: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub struct DesktopWatch {
    hook: HWINEVENTHOOK,
}

impl DesktopWatch {
    pub fn start(notify: HWND) -> Self {
        unsafe {
            TARGET.store(notify.0, Ordering::Relaxed);
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            Self { hook }
        }
    }
}

impl Drop for DesktopWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.hook);
        }
    }
}

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    unsafe {
        let msg = if is_desktop_window(hwnd) {
            WM_DESKTOP_FOREGROUND
        } else {
            WM_OTHER_FOREGROUND
        };
        let target = HWND(TARGET.load(Ordering::Relaxed));
        let _ = PostMessageW(Some(target), msg, WPARAM(hwnd.0 as usize), LPARAM(0));
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn is_desktop_window(hwnd: HWND) -> bool {
    matches!(class_of(hwnd).as_str(), "WorkerW" | "Progman")
}

/// The window that hosts the desktop icons: Progman on Windows 11 24H2+,
/// otherwise the WorkerW that contains SHELLDLL_DefView.
fn desktop_icons_host() -> Option<HWND> {
    unsafe {
        let progman = GetShellWindow();
        if !progman.is_invalid()
            && FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).is_ok()
        {
            return Some(progman);
        }
        let mut worker: Option<HWND> = None;
        while let Ok(w) = FindWindowExW(None, worker, w!("WorkerW"), None) {
            if FindWindowExW(Some(w), None, w!("SHELLDLL_DefView"), None).is_ok() {
                return Some(w);
            }
            worker = Some(w);
        }
        None
    }
}

/// True when Show desktop has raised the desktop above our sentinel, a hidden
/// window we keep at HWND_BOTTOM: FindWindowEx with a child-after argument
/// searches the windows *below* the host in z-order.
pub fn show_desktop_active(sentinel_class: PCWSTR) -> bool {
    let Some(host) = desktop_icons_host() else {
        return false;
    };
    unsafe {
        IsWindowVisible(host).as_bool()
            && FindWindowExW(None, Some(host), sentinel_class, None).is_ok()
    }
}
