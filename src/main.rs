#![windows_subsystem = "windows"]

mod app;
mod doc;
mod gfx;
mod history;
mod model;
mod platform;
mod store;

use std::cell::RefCell;
use std::ffi::c_void;

use windows::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::LOGFONTW;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, GetDC, MONITOR_DEFAULTTONULL, MonitorFromRect, ReleaseDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::Dialogs::{
    CF_EFFECTS, CF_INITTOLOGFONTSTRUCT, CF_LIMITSIZE, CF_NOSCRIPTSEL, CF_NOSTYLESEL,
    CF_NOVERTFONTS, CF_SCREENFONTS, CHOOSEFONTW, ChooseFontW,
};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

use app::{App, Deferred, MIN_H, MIN_W, Out, PILL_H, SENTINEL_CLASS, WM_ADD_ENTER, WM_EDIT_CMD};

const MAIN_CLASS: PCWSTR = w!("TodoWidgetWnd");

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app unless it is already borrowed (a nested message).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        let app = guard.as_mut()?;
        Some(f(app))
    })
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // One instance: a second launch just shows the first.
        let _mutex = CreateMutexW(None, true, w!("Local\\TodoWidget.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(MAIN_CLASS, None) {
                let _ = ShowWindow(existing, SW_SHOW);
                let _ = SetForegroundWindow(existing);
            }
            return;
        }

        let inst = GetModuleHandleW(None).unwrap_or_default();
        let big_icon = make_icon(32);
        let small_icon = make_icon(16);

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big_icon,
            hIconSm: small_icon,
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let sc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(sentinel_proc),
            hInstance: inst.into(),
            lpszClassName: SENTINEL_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&sc);

        // Never shown: it only marks a z-order position (see platform.rs).
        let sentinel = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            SENTINEL_CLASS,
            w!("To-do sentinel"),
            WS_POPUP | WS_DISABLED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .unwrap_or_default();

        let store = store::Store::open();
        let mut settings = store.load_settings();
        let (x, y, cw, ch) = initial_rect(&mut settings);

        let mut ex = WS_EX_TOOLWINDOW;
        if settings.on_top {
            ex |= WS_EX_TOPMOST;
        }
        let Ok(hwnd) = CreateWindowExW(
            ex,
            MAIN_CLASS,
            w!("To-do"),
            WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
            x,
            y,
            cw,
            ch,
            None,
            None,
            Some(inst.into()),
            None,
        ) else {
            return;
        };

        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const c_void,
            4,
        );
        let dark = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const c_void,
            4,
        );
        let border = COLORREF(0x00483F38);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const c_void,
            4,
        );
        // Apply WM_NCCALCSIZE (no frame) before the first paint.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );

        let app = App::new(hwnd, sentinel, settings, store, small_icon);
        APP.with(|c| *c.borrow_mut() = Some(app));
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| c.borrow_mut().take());
    }
}

/// Saved rectangle if it is still on a monitor; otherwise 440 x 460 at the
/// top-right of the primary work area.
fn initial_rect(settings: &mut model::Settings) -> (i32, i32, i32, i32) {
    unsafe {
        if let Some([x, y, w, h]) = settings.rect {
            let rc = RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            };
            if w > 0 && h > 0 && !MonitorFromRect(&rc, MONITOR_DEFAULTTONULL).is_invalid() {
                return (x, y, w, h);
            }
        }
        let s = GetDpiForSystem().max(96) as f32 / 96.0;
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let w = (440.0 * s) as i32;
        let expanded = (460.0 * s) as i32;
        settings.expanded_height = Some(expanded);
        let h = if settings.collapsed {
            (PILL_H * s) as i32
        } else {
            expanded
        };
        let margin = (24.0 * s) as i32;
        (work.right - w - margin, work.top + margin, w, h)
    }
}

unsafe extern "system" fn sentinel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // The whole window is client area; DWM still draws the shadow,
            // the 1 px border and the rounded corners.
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_NCACTIVATE => return DefWindowProcW(hwnd, msg, wp, LPARAM(-1)),
            WM_GETMINMAXINFO => {
                let s = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                let collapsed = with_app(|a| a.settings.collapsed).unwrap_or(true);
                let mmi = &mut *(lp.0 as *mut MINMAXINFO);
                let min_h = if collapsed { PILL_H } else { MIN_H };
                mmi.ptMinTrackSize = POINT {
                    x: (MIN_W * s) as i32,
                    y: (min_h * s) as i32,
                };
                return LRESULT(0);
            }
            _ => {}
        }
        match with_app(|a| a.handle(msg, wp, lp)) {
            Some(Out::Done(r)) => r,
            Some(Out::Defer(d)) => {
                run_deferred(hwnd, d);
                LRESULT(0)
            }
            Some(Out::Destroy) => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            Some(Out::Default) | None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn run_deferred(hwnd: HWND, d: Deferred) {
    match d {
        Deferred::Menu { entries, x, y } => unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let mut labels = Vec::new();
            for e in &entries {
                if e.id == 0 {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                    continue;
                }
                let mut flags = MF_STRING;
                if e.checked {
                    flags |= MF_CHECKED;
                }
                if !e.enabled {
                    flags |= MF_GRAYED;
                }
                labels.push(platform::wide(&e.label));
                let label = labels.last().unwrap();
                let _ = AppendMenuW(menu, flags, e.id as usize, PCWSTR(label.as_ptr()));
            }
            // Required so the menu closes when clicking elsewhere (tray menus).
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                x,
                y,
                None,
                hwnd,
                None,
            );
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            if cmd.0 > 0 {
                with_app(|a| a.menu_command(cmd.0 as u32));
            }
        },
        Deferred::FontDialog => unsafe {
            let Some((family, size, ink, _dpi)) = with_app(|a| a.font_dialog_init()) else {
                return;
            };
            let mut lf = LOGFONTW::default();
            let screen = GetDC(None);
            let ydpi = windows::Win32::Graphics::Gdi::GetDeviceCaps(
                Some(screen),
                windows::Win32::Graphics::Gdi::LOGPIXELSY,
            );
            ReleaseDC(None, screen);
            lf.lfHeight = -((size * ydpi as f32 / 72.0).round() as i32);
            for (dst, src) in lf.lfFaceName.iter_mut().take(31).zip(family.encode_utf16()) {
                *dst = src;
            }
            let rgb = ink;
            let mut cf = CHOOSEFONTW {
                lStructSize: std::mem::size_of::<CHOOSEFONTW>() as u32,
                hwndOwner: hwnd,
                lpLogFont: &mut lf,
                Flags: CF_INITTOLOGFONTSTRUCT
                    | CF_SCREENFONTS
                    | CF_EFFECTS
                    | CF_NOSTYLESEL
                    | CF_NOVERTFONTS
                    | CF_NOSCRIPTSEL
                    | CF_LIMITSIZE,
                rgbColors: COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)),
                nSizeMin: 6,
                nSizeMax: 36,
                ..Default::default()
            };
            if ChooseFontW(&mut cf).as_bool() {
                let end = lf.lfFaceName.iter().position(|c| *c == 0).unwrap_or(32);
                let face = String::from_utf16_lossy(&lf.lfFaceName[..end]);
                let c = cf.rgbColors.0;
                let rgb = ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
                let pt = cf.iPointSize as f32 / 10.0;
                with_app(|a| a.apply_font(face, pt, rgb));
            }
        },
    }
}

// ---------------------------------------------------------------- edit-box subclasses
//
// They only post messages to the main window; the app reacts outside of the
// edit control's own message handling.

pub fn subclass(hwnd: HWND, proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {
    unsafe {
        let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as usize as isize);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, old);
    }
}

unsafe fn call_old(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let old = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        let old: WNDPROC = std::mem::transmute::<isize, WNDPROC>(old);
        CallWindowProcW(old, hwnd, msg, wp, lp)
    }
}

fn post_parent(hwnd: HWND, msg: u32, wp: usize) {
    unsafe {
        if let Ok(parent) = GetParent(hwnd) {
            let _ = PostMessageW(Some(parent), msg, WPARAM(wp), LPARAM(hwnd.0 as isize));
        }
    }
}

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

/// Window procedure for the inline edit box.
///
/// # Safety
/// Only for windows subclassed with [`subclass`], which stores the original
/// procedure in GWLP_USERDATA.
pub unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN => {
                let vk = wp.0 as u16;
                let ctrl = down(VK_CONTROL.0);
                let shift = down(VK_SHIFT.0);
                let code = match vk {
                    0x0D => Some(1),                         // Enter
                    0x1B => Some(2),                         // Esc
                    0x09 => Some(if shift { 4 } else { 3 }), // Tab
                    0x08 if GetWindowTextLengthW(hwnd) == 0 => Some(5),
                    0x42 if ctrl => Some(7),  // Ctrl+B
                    0x49 if ctrl => Some(8),  // Ctrl+I
                    0x55 if ctrl => Some(9),  // Ctrl+U
                    0x48 if ctrl => Some(10), // Ctrl+H: show/hide header
                    0x41 if ctrl => {
                        SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                        return LRESULT(0);
                    }
                    _ => None,
                };
                if let Some(c) = code {
                    post_parent(hwnd, WM_EDIT_CMD, c);
                    return LRESULT(0);
                }
            }
            // Swallow the characters produced by the keys handled above, so
            // the control neither inserts them nor beeps.
            WM_CHAR => {
                if matches!(wp.0, 0x0D | 0x0A | 0x09 | 0x1B | 0x01 | 0x02 | 0x15) {
                    return LRESULT(0);
                }
                // Ctrl+H also arrives as a backspace character.
                if wp.0 == 0x08 && down(VK_CONTROL.0) {
                    return LRESULT(0);
                }
            }
            WM_KILLFOCUS => post_parent(hwnd, WM_EDIT_CMD, 6),
            _ => {}
        }
        call_old(hwnd, msg, wp, lp)
    }
}

/// Window procedure for the Add box.
///
/// # Safety
/// Same contract as [`edit_proc`].
pub unsafe extern "system" fn add_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN if wp.0 == 0x0D => {
                post_parent(hwnd, WM_ADD_ENTER, 0);
                return LRESULT(0);
            }
            WM_KEYDOWN if wp.0 == 0x41 && down(VK_CONTROL.0) => {
                SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                return LRESULT(0);
            }
            WM_CHAR if matches!(wp.0, 0x0D | 0x0A | 0x01) => return LRESULT(0),
            _ => {}
        }
        call_old(hwnd, msg, wp, lp)
    }
}

// ---------------------------------------------------------------- icon

/// Draws the app icon in code: an accent rounded square with a white check.
/// 4x4 supersampling for smooth edges; no resource file needed.
fn make_icon(size: i32) -> HICON {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dc = GetDC(None);
        let color = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        ReleaseDC(None, dc);
        let Ok(color) = color else {
            return LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        let n = size as f32;
        let radius = n * 0.22;
        let seg = |px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)| {
            let (dx, dy) = (bx - ax, by - ay);
            let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
        };
        let (p1, p2, p3) = (
            (0.27 * n, 0.52 * n),
            (0.43 * n, 0.68 * n),
            (0.74 * n, 0.35 * n),
        );
        let stroke = (n * 0.085).max(1.2);
        for y in 0..size {
            for x in 0..size {
                let (mut cover, mut white) = (0.0f32, 0.0f32);
                for sy in 0..4 {
                    for sx in 0..4 {
                        let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                        let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                        let qx = (fx - n / 2.0).abs() - (n / 2.0 - 0.5 - radius);
                        let qy = (fy - n / 2.0).abs() - (n / 2.0 - 0.5 - radius);
                        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                            + qx.max(qy).min(0.0)
                            - radius;
                        if outside <= 0.0 {
                            cover += 1.0 / 16.0;
                            if seg(fx, fy, p1, p2).min(seg(fx, fy, p2, p3)) <= stroke {
                                white += 1.0 / 16.0;
                            }
                        }
                    }
                }
                let wf = if cover > 0.0 { white / cover } else { 0.0 };
                let mix = |a: u32, b: u32| (a as f32 * (1.0 - wf) + b as f32 * wf).round() as u32;
                let (r, g, b) = (mix(0x2B, 0xFF), mix(0x8F, 0xFF), mix(0xD9, 0xFF));
                let a = (cover * 255.0).round() as u32;
                px[(y * size + x) as usize] = (a << 24) | (r << 16) | (g << 8) | b;
            }
        }
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let info = ICONINFO {
            fIcon: BOOL(1),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
