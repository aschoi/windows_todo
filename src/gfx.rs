//! Direct2D + DirectWrite drawing. All coordinates are DIPs; the render
//! target's DPI is set to the window's, so Direct2D does the scaling.

use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{BOOL, HSTRING, Result};
use windows_numerics::Vector2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Rect {
    pub fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { l, t, r, b }
    }
    pub fn xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            l: x,
            t: y,
            r: x + w,
            b: y + h,
        }
    }
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.l + dx, self.t + dy, self.r - dx, self.b - dy)
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.l,
            top: self.t,
            right: self.r,
            bottom: self.b,
        }
    }
}

pub fn color(rgb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a,
    }
}

#[derive(Clone, Copy, Default)]
pub struct Style {
    pub bold: bool,
    pub semibold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    formats: HashMap<(String, u32, u32, bool), IDWriteTextFormat>,
    pub icon_font: String,
    hardware: bool,
}

// Drawing primitives take position, color and alpha together.
#[allow(clippy::too_many_arguments)]
impl Gfx {
    pub fn new() -> Result<Self> {
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let mut g = Self {
            d2d,
            dw,
            rt: None,
            brush: None,
            formats: HashMap::new(),
            icon_font: "Segoe Fluent Icons".into(),
            // Software rendering avoids loading a Direct3D device: less memory,
            // and plenty fast for a small list. TODO_WIDGET_GPU=1 to compare.
            hardware: std::env::var_os("TODO_WIDGET_GPU").is_some(),
        };
        if !g.has_family("Segoe Fluent Icons") {
            g.icon_font = "Segoe MDL2 Assets".into();
        }
        Ok(g)
    }

    fn has_family(&self, name: &str) -> bool {
        unsafe {
            let mut coll = None;
            if self.dw.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let mut index = 0u32;
            let mut exists = BOOL(0);
            coll.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        }
    }

    /// Creates the render target if needed and keeps it the client's size.
    pub fn begin(&mut self, hwnd: HWND, dpi: u32) -> bool {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let size = D2D_SIZE_U {
            width: (rc.right - rc.left).max(1) as u32,
            height: (rc.bottom - rc.top).max(1) as u32,
        };
        if self.rt.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: if self.hardware {
                    D2D1_RENDER_TARGET_TYPE_DEFAULT
                } else {
                    D2D1_RENDER_TARGET_TYPE_SOFTWARE
                },
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let Ok(rt) = (unsafe { self.d2d.CreateHwndRenderTarget(&props, &hprops) }) else {
                return false;
            };
            let Ok(brush) = (unsafe { rt.CreateSolidColorBrush(&color(0, 1.0), None) }) else {
                return false;
            };
            self.rt = Some(rt);
            self.brush = Some(brush);
        }
        let rt = self.rt.as_ref().unwrap();
        unsafe {
            if rt.GetPixelSize() != size {
                let _ = rt.Resize(&size);
            }
            rt.SetDpi(dpi as f32, dpi as f32);
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            rt.BeginDraw();
        }
        true
    }

    pub fn end(&mut self) {
        if let Some(rt) = &self.rt
            && unsafe { rt.EndDraw(None, None) }.is_err()
        {
            // Device lost or similar: rebuild on the next paint.
            self.rt = None;
            self.brush = None;
        }
    }

    fn rt(&self) -> &ID2D1HwndRenderTarget {
        self.rt.as_ref().unwrap()
    }

    fn brush(&self, rgb: u32, a: f32) -> &ID2D1SolidColorBrush {
        let b = self.brush.as_ref().unwrap();
        unsafe { b.SetColor(&color(rgb, a)) };
        b
    }

    pub fn clear(&self, rgb: u32) {
        unsafe { self.rt().Clear(Some(&color(rgb, 1.0))) };
    }

    pub fn fill(&self, r: Rect, rgb: u32, a: f32) {
        unsafe { self.rt().FillRectangle(&r.d2d(), self.brush(rgb, a)) };
    }

    pub fn fill_round(&self, r: Rect, radius: f32, rgb: u32, a: f32) {
        let rr = D2D1_ROUNDED_RECT {
            rect: r.d2d(),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.rt().FillRoundedRectangle(&rr, self.brush(rgb, a)) };
    }

    pub fn stroke_round(&self, r: Rect, radius: f32, rgb: u32, a: f32, width: f32) {
        let rr = D2D1_ROUNDED_RECT {
            rect: r.d2d(),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.rt()
                .DrawRoundedRectangle(&rr, self.brush(rgb, a), width, None)
        };
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, rgb: u32, a: f32) {
        let e = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.rt().FillEllipse(&e, self.brush(rgb, a)) };
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, rgb: u32, a: f32, width: f32) {
        let e = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.rt().DrawEllipse(&e, self.brush(rgb, a), width, None) };
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, rgb: u32, a: f32, width: f32) {
        unsafe {
            self.rt().DrawLine(
                Vector2 { X: x1, Y: y1 },
                Vector2 { X: x2, Y: y2 },
                self.brush(rgb, a),
                width,
                None,
            )
        };
    }

    pub fn clip(&self, r: Rect) {
        unsafe {
            self.rt()
                .PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
        };
    }

    pub fn unclip(&self) {
        unsafe { self.rt().PopAxisAlignedClip() };
    }

    fn format(&mut self, family: &str, size: f32, st: Style) -> Option<IDWriteTextFormat> {
        let weight: DWRITE_FONT_WEIGHT = if st.bold {
            DWRITE_FONT_WEIGHT_BOLD
        } else if st.semibold {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        let key = (
            family.to_string(),
            (size * 100.0) as u32,
            weight.0 as u32,
            st.italic,
        );
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let style = if st.italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let f = unsafe {
            self.dw
                .CreateTextFormat(
                    &HSTRING::from(family),
                    None,
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    &HSTRING::from("en-us"),
                )
                .ok()?
        };
        self.formats.insert(key, f.clone());
        Some(f)
    }

    /// Lays out text wrapped to `width` (or on one line if `wrap` is false).
    pub fn layout(
        &mut self,
        text: &str,
        family: &str,
        size: f32,
        st: Style,
        width: f32,
        wrap: bool,
    ) -> Option<TextBlock> {
        let fmt = self.format(family, size, st)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.dw
                .CreateTextLayout(&wide, &fmt, width.max(1.0), 10_000.0)
                .ok()?
        };
        unsafe {
            if !wrap {
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            }
            let range = DWRITE_TEXT_RANGE {
                startPosition: 0,
                length: wide.len() as u32,
            };
            if st.underline {
                let _ = layout.SetUnderline(true, range);
            }
            if st.strike {
                let _ = layout.SetStrikethrough(true, range);
            }
        }
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        Some(TextBlock {
            layout,
            width: m.widthIncludingTrailingWhitespace,
            height: m.height,
        })
    }

    pub fn draw(&self, block: &TextBlock, x: f32, y: f32, rgb: u32, a: f32) {
        unsafe {
            self.rt().DrawTextLayout(
                Vector2 { X: x, Y: y },
                &block.layout,
                self.brush(rgb, a),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            )
        };
    }

    /// Single-line text, vertically centered in `r`, left-aligned at r.l.
    pub fn text(
        &mut self,
        s: &str,
        family: &str,
        size: f32,
        st: Style,
        r: Rect,
        rgb: u32,
        a: f32,
    ) -> f32 {
        let Some(b) = self.layout(s, family, size, st, 10_000.0, false) else {
            return 0.0;
        };
        self.draw(&b, r.l, r.t + (r.h() - b.height) / 2.0, rgb, a);
        b.width
    }

    pub fn measure(&mut self, s: &str, family: &str, size: f32, st: Style) -> f32 {
        self.layout(s, family, size, st, 10_000.0, false)
            .map(|b| b.width)
            .unwrap_or(0.0)
    }

    /// An icon-font glyph centered in `r`.
    pub fn icon(&mut self, glyph: char, r: Rect, size: f32, rgb: u32, a: f32) {
        let family = self.icon_font.clone();
        let Some(fmt) = self.format(&family, size, Style::default()) else {
            return;
        };
        let mut buf = [0u16; 2];
        let wide = glyph.encode_utf16(&mut buf);
        let Ok(layout) = (unsafe { self.dw.CreateTextLayout(wide, &fmt, r.w(), r.h()) }) else {
            return;
        };
        unsafe {
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            self.rt().DrawTextLayout(
                Vector2 { X: r.l, Y: r.t },
                &layout,
                self.brush(rgb, a),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }
}

pub struct TextBlock {
    layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}
