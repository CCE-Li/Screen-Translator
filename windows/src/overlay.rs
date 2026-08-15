//! Transparent always-on-top overlay window.
//!
//! Rendering: GDI+ draws into a 32bpp premultiplied-ARGB DIB section whose
//! contents are presented with `UpdateLayeredWindow` (per-pixel alpha). This
//! gives us rounded rects, antialiased text and shadows without any UI toolkit.
//!
//! Modes:
//!   * Work — window is `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_LAYERED`,
//!     mouse clicks fall through to the app below.
//!   * Edit — transparency flags are removed so the window receives mouse
//!     input for creating/moving/deleting regions.

use std::ptr::null_mut;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateDIBSection, CreateCompatibleDC, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BLENDFUNCTION, BI_RGB, DIB_RGB_COLORS, HDC, HBITMAP, HGDIOBJ,
};
use windows::Win32::Graphics::GdiPlus::{
    GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GdipAddPathArc, GdipClosePathFigure,
    GdipCreateBitmapFromScan0, GdipCreateFont, GdipCreateFontFamilyFromName, GdipCreatePath,
    GdipCreatePen1, GdipCreateSolidFill, GdipCreateStringFormat, GdipDeleteBrush, GdipDeleteFont,
    GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen, GdipDeleteStringFormat,
    GdipDisposeImage, GdipDrawPath, GdipDrawString, GdipFillPath, GdipFillRectangle,
    GdipGetImageGraphicsContext, GdipSetSmoothingMode,
    GdipSetStringFormatAlign, GdipSetStringFormatLineAlign, GdipSetTextRenderingHint,
    GpBitmap, GpBrush, GpFont, GpFontCollection, GpFontFamily, GpGraphics, GpImage, GpPath, GpPen,
    GpStringFormat, GpSolidFill, Status, StringAlignment, FillModeAlternate, FontStyleRegular,
    RectF, SmoothingModeAntiAlias, StringAlignmentCenter, StringAlignmentNear,
    TextRenderingHintAntiAlias, UnitPixel,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetSystemMetrics, RegisterClassW, SetWindowPos, WNDCLASSW,
    CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use screen_translator_core::config::OverlayStyle;
use screen_translator_core::types::{OverlayText, Rect};

/// GDI+ pixel formats (not exposed as named constants by the bindings).
const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x0E200B;
/// GWLP_EXSTYLE is absent from the bindings; -20 is its canonical value.
const GWLP_EXSTYLE: windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX =
    windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX(-20);

/// One active drag rectangle during region editing.
#[derive(Debug, Clone, Copy)]
pub struct DragRect {
    pub start: (i32, i32),
    pub current: (i32, i32),
}

/// Live performance numbers shown in the overlay HUD.
#[derive(Debug, Clone, Copy)]
pub struct StatsSnapshot {
    pub fps: f32,
    pub target_fps: f32,
    pub capture_ms: f32,
    pub ocr_ms: f32,
    pub translate_ms: f32,
    pub ocr_runs: u64,
    pub pending: usize,
    pub cache_hit: f32,
    pub cpu: f32,
    pub mem_mb: f32,
}

impl StatsSnapshot {
    fn lines(&self) -> Vec<String> {
        vec![
            format!(
                "fps {:.1}/{:.0}  cap {:.0}ms  ocr {:.0}ms  tr {:.0}ms",
                self.fps, self.target_fps, self.capture_ms, self.ocr_ms, self.translate_ms
            ),
            format!(
                "ocr {}  pending {}  tc-hit {:.0}%  cpu {:.0}%  mem {:.0}MB",
                self.ocr_runs,
                self.pending,
                self.cache_hit * 100.0,
                self.cpu,
                self.mem_mb
            ),
        ]
    }
}

/// Which edge/corner of a region is being resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

/// Distance (px) from an edge within which a resize handle activates.
const HANDLE_TOLERANCE: f32 = 10.0;

/// Return the resize handle under `(x, y)`, if any.
pub fn hit_test_resize(rect: Rect, x: f32, y: f32) -> Option<ResizeHandle> {
    let (l, t) = (rect.x, rect.y);
    let (r, b) = (rect.x + rect.width, rect.y + rect.height);
    let tol = HANDLE_TOLERANCE;
    let near_l = (x - l).abs() <= tol;
    let near_r = (x - r).abs() <= tol;
    let near_t = (y - t).abs() <= tol;
    let near_b = (y - b).abs() <= tol;
    if near_t && near_l {
        Some(ResizeHandle::TopLeft)
    } else if near_t && near_r {
        Some(ResizeHandle::TopRight)
    } else if near_b && near_l {
        Some(ResizeHandle::BottomLeft)
    } else if near_b && near_r {
        Some(ResizeHandle::BottomRight)
    } else if near_t {
        Some(ResizeHandle::Top)
    } else if near_b {
        Some(ResizeHandle::Bottom)
    } else if near_l {
        Some(ResizeHandle::Left)
    } else if near_r {
        Some(ResizeHandle::Right)
    } else {
        None
    }
}

/// Resize `orig` by dragging `handle` to `(x, y)`. The opposite side stays
/// anchored; minimum size is enforced.
pub fn apply_resize(orig: Rect, handle: ResizeHandle, x: f32, y: f32) -> Rect {
    const MIN_W: f32 = 24.0;
    const MIN_H: f32 = 14.0;
    let mut r = orig;
    match handle {
        ResizeHandle::Left => {
            let right = r.x + r.width;
            let new_x = x.min(right - MIN_W);
            r.x = new_x;
            r.width = right - new_x;
        }
        ResizeHandle::Right => {
            r.width = (x - r.x).max(MIN_W);
        }
        ResizeHandle::Top => {
            let bottom = r.y + r.height;
            let new_y = y.min(bottom - MIN_H);
            r.y = new_y;
            r.height = bottom - new_y;
        }
        ResizeHandle::Bottom => {
            r.height = (y - r.y).max(MIN_H);
        }
        ResizeHandle::TopLeft => {
            let right = r.x + r.width;
            let bottom = r.y + r.height;
            let new_x = x.min(right - MIN_W);
            let new_y = y.min(bottom - MIN_H);
            r.x = new_x;
            r.y = new_y;
            r.width = right - new_x;
            r.height = bottom - new_y;
        }
        ResizeHandle::TopRight => {
            let bottom = r.y + r.height;
            let new_y = y.min(bottom - MIN_H);
            r.y = new_y;
            r.height = bottom - new_y;
            r.width = (x - r.x).max(MIN_W);
        }
        ResizeHandle::BottomLeft => {
            let right = r.x + r.width;
            let new_x = x.min(right - MIN_W);
            r.x = new_x;
            r.width = right - new_x;
            r.height = (y - r.y).max(MIN_H);
        }
        ResizeHandle::BottomRight => {
            r.width = (x - r.x).max(MIN_W);
            r.height = (y - r.y).max(MIN_H);
        }
    }
    r
}

pub struct OverlayWindow {
    pub hwnd: HWND,
    width: u32,
    height: u32,
    gdi_token: usize,
    bitmap: *mut GpBitmap,
    scan0: *mut u8,
    dib_dc: HDC,
    dib: HBITMAP,
    style: OverlayStyle,
    mode: bool, // true = work (click-through), false = edit
    stats: Option<StatsSnapshot>,
    last_items: Vec<OverlayText>,
}

impl OverlayWindow {
    pub fn from_hwnd(hwnd: HWND, style: OverlayStyle) -> anyhow::Result<Self> {
        unsafe {
            let screen_w = GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CXVIRTUALSCREEN);
            let screen_h = GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CYVIRTUALSCREEN);

            let mut overlay = Self {
                hwnd,
                width: screen_w.max(1) as u32,
                height: screen_h.max(1) as u32,
                gdi_token: 0,
                bitmap: null_mut(),
                scan0: null_mut(),
                dib_dc: HDC::default(),
                dib: HBITMAP::default(),
                style,
                mode: false,
                stats: None,
                last_items: Vec::new(),
            };
            overlay.init_gdiplus()?;
            overlay.init_dib()?;

            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_XVIRTUALSCREEN),
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_YVIRTUALSCREEN),
                screen_w,
                screen_h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|e| anyhow::anyhow!("SetWindowPos: {e}"))?;

            Ok(overlay)
        }
    }

    fn init_gdiplus(&mut self) -> anyhow::Result<()> {
        unsafe {
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: false.into(),
                SuppressExternalCodecs: false.into(),
            };
            let mut token: usize = 0;
            let status = GdiplusStartup(
                &mut token,
                &input,
                std::ptr::null_mut::<windows::Win32::Graphics::GdiPlus::GdiplusStartupOutput>(),
            );
            if status != Status(0) {
                return Err(anyhow::anyhow!("GdiplusStartup failed: {status:?}"));
            }
            self.gdi_token = token;
            Ok(())
        }
    }

    fn init_dib(&mut self) -> anyhow::Result<()> {
        unsafe {
            let w = self.width as i32;
            let h = self.height as i32;
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: (w as u32 * h as u32 * 4),
                    ..Default::default()
                },
                bmiColors: [Default::default(); 1],
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            let dib = CreateDIBSection(
                None,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
            .map_err(|e| anyhow::anyhow!("CreateDIBSection: {e}"))?;
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                let _ = DeleteObject(dib.into());
                return Err(anyhow::anyhow!("CreateCompatibleDC failed"));
            }
            SelectObject(dc, HGDIOBJ(dib.0));

            let mut bitmap: *mut GpBitmap = null_mut();
            let status = GdipCreateBitmapFromScan0(
                w,
                h,
                w * 4,
                PIXEL_FORMAT_32BPP_PARGB,
                Some(bits as *const u8),
                &mut bitmap,
            );
            if status != Status(0) {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(dib.0));
                return Err(anyhow::anyhow!("GdipCreateBitmapFromScan0: {status:?}"));
            }

            self.bitmap = bitmap;
            self.scan0 = bits as *mut u8;
            self.dib = dib;
            self.dib_dc = dc;
            Ok(())
        }
    }

    pub fn set_mode(&mut self, work: bool) -> anyhow::Result<()> {
        self.mode = work;
        unsafe {
            let ex = if work {
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT
            } else {
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            };
            let r = windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
                self.hwnd,
                GWLP_EXSTYLE,
                ex.0 as isize,
            );
            if r == 0 {
                return Err(anyhow::anyhow!("SetWindowLongPtrW exstyle failed"));
            }
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
            )
            .map_err(|e| anyhow::anyhow!("SetWindowPos in set_mode: {e}"))?;
        }
        Ok(())
    }

    /// Clear the overlay (transparent).
    pub fn clear(&mut self) {
        self.render(|_g| {});
    }

    /// Update the performance HUD snapshot; redraws the work view.
    pub fn set_stats(&mut self, stats: StatsSnapshot) {
        self.stats = Some(stats);
        if self.mode {
            let items = std::mem::take(&mut self.last_items);
            self.render_work(&items);
            self.last_items = items;
        }
    }

    /// Render work-mode translation overlays.
    pub fn render_work(&mut self, items: &[OverlayText]) {
        self.last_items = items.to_vec();
        let style = self.style.clone();
        let opacity = style.opacity.clamp(0.0, 1.0);
        let hud = self.stats;
        let w = self.width as f32;
        self.render(move |g| {
            unsafe {
                let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
                let _ = GdipSetTextRenderingHint(g, TextRenderingHintAntiAlias);
            }
            let mut family: *mut GpFontFamily = null_mut();
            unsafe {
                let name = PCWSTR(style.font_family.encode_utf16().collect::<Vec<u16>>().as_ptr());
                GdipCreateFontFamilyFromName(name, null_mut::<GpFontCollection>(), &mut family);
                if family.is_null() {
                    GdipCreateFontFamilyFromName(
                        PCWSTR(windows::core::w!("Microsoft YaHei UI").as_ptr()),
                        null_mut::<GpFontCollection>(),
                        &mut family,
                    );
                }
            }
            for item in items {
                draw_text(
                    g,
                    family,
                    item,
                    &style,
                    opacity,
                );
            }
            if let Some(h) = hud {
                draw_stats_hud(g, family, &h, w);
            }
            unsafe {
                if !family.is_null() {
                    GdipDeleteFontFamily(family);
                }
            }
        });
    }

    /// Render edit-mode region rectangles, resize handles and drag feedback.
    /// `active` is the region/drag rectangle currently being created/moved/resized.
    pub fn render_edit(
        &mut self,
        regions: &[screen_translator_core::config::RegionConfig],
        active: Option<Rect>,
    ) {
        let w = self.width as f32;
        let h = self.height as f32;
        self.render(move |g| {
            unsafe {
                let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
                let _ = GdipSetTextRenderingHint(g, TextRenderingHintAntiAlias);
            }
            // Dim the whole screen slightly so regions stand out.
            let dim = 0x33000000u32;
            let mut dim_brush: *mut GpSolidFill = null_mut();
            unsafe {
                GdipCreateSolidFill(dim, &mut dim_brush);
                if !dim_brush.is_null() {
                    GdipFillRectangle(g, dim_brush as *mut GpBrush, 0.0, 0.0, w, h);
                }
            }

            for region in regions {
                draw_region_box(g, region.rect, 0xFF00C8FF, 0x4000C8FF, 2.0);
                draw_resize_handles(g, region.rect);
            }
            if let Some(rect) = active {
                if rect.width > 2.0 || rect.height > 2.0 {
                    draw_region_box(g, rect, 0xFFFFFF00, 0x50FFFFFF, 2.0);
                }
            }

            // Hint text at top center.
            let mut family: *mut GpFontFamily = null_mut();
            unsafe {
                GdipCreateFontFamilyFromName(
                    PCWSTR(windows::core::w!("Microsoft YaHei UI").as_ptr()),
                    null_mut::<GpFontCollection>(),
                    &mut family,
                );
            }
            let hint = "Edit mode: drag = create/move region, right-click = delete, hotkey = work mode";
            let hint_box = RectF {
                X: 0.0,
                Y: 12.0,
                Width: w,
                Height: 40.0,
            };
            draw_text_basic(g, family, hint, 18.0, hint_box, 0xE0FFFFFF, StringAlignmentCenter);
            unsafe {
                if !family.is_null() {
                    GdipDeleteFontFamily(family);
                }
            }
        });
    }

    /// Present the current DIB to the layered window.
    pub fn present(&self) {
        unsafe {
            let blend = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1,
            };
            let size = SIZE {
                cx: self.width as i32,
                cy: self.height as i32,
            };
            let pt = POINT { x: 0, y: 0 };
            let _ = windows::Win32::UI::WindowsAndMessaging::UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pt as *const POINT),
                Some(&size as *const SIZE),
                Some(self.dib_dc),
                Some(&pt as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                windows::Win32::UI::WindowsAndMessaging::ULW_ALPHA,
            );
        }
    }

    fn render(&mut self, f: impl FnOnce(*mut GpGraphics)) {
        unsafe {
            let mut graphics: *mut GpGraphics = null_mut();
            let status = GdipGetImageGraphicsContext(self.bitmap as *mut GpImage, &mut graphics);
            if status != Status(0) || graphics.is_null() {
                return;
            }
            f(graphics);
            GdipDeleteGraphics(graphics);
            self.present();
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            if !self.bitmap.is_null() {
                GdipDisposeImage(self.bitmap as *mut GpImage);
            }
            if !self.dib.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.dib.0));
            }
            if !self.dib_dc.is_invalid() {
                let _ = DeleteDC(self.dib_dc);
            }
            if self.gdi_token != 0 {
                GdiplusShutdown(self.gdi_token);
            }
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

fn normalize_drag(d: DragRect) -> Rect {
    let x = d.start.0.min(d.current.0) as f32;
    let y = d.start.1.min(d.current.1) as f32;
    let w = (d.start.0.max(d.current.0) - x as i32) as f32;
    let h = (d.start.1.max(d.current.1) - y as i32) as f32;
    Rect::new(x, y, w, h)
}

/// Convert an active drag into a normalized screen rectangle.
pub fn drag_to_rect(d: DragRect) -> Rect {
    normalize_drag(d)
}

fn draw_region_box(
    g: *mut GpGraphics,
    rect: Rect,
    border: u32,
    fill: u32,
    line_width: f32,
) {
    unsafe {
        use windows::Win32::Graphics::GdiPlus::GdipAddPathRectangle;
        let mut fill_brush: *mut GpSolidFill = null_mut();
        GdipCreateSolidFill(fill, &mut fill_brush);
        if !fill_brush.is_null() {
            GdipFillRectangle(g, fill_brush as *mut GpBrush, rect.x, rect.y, rect.width, rect.height);
        }
        let mut pen: *mut GpPen = null_mut();
        GdipCreatePen1(border, line_width, UnitPixel, &mut pen);
        if !pen.is_null() {
            let mut path: *mut GpPath = null_mut();
            GdipCreatePath(FillModeAlternate, &mut path);
            if !path.is_null() {
                let r = RectF { X: rect.x, Y: rect.y, Width: rect.width, Height: rect.height };
                GdipAddPathRectangle(path, r.X, r.Y, r.Width, r.Height);
                GdipDrawPath(g, pen, path);
                GdipDeletePath(path);
            }
            GdipDeletePen(pen);
        }
        if !fill_brush.is_null() {
            GdipDeleteBrush(fill_brush as *mut GpBrush);
        }
    }
}

/// Draw the performance HUD panel in the top-right corner.
fn draw_stats_hud(g: *mut GpGraphics, family: *mut GpFontFamily, stats: &StatsSnapshot, screen_w: f32) {
    let lines = stats.lines();
    let panel_w = 340.0f32;
    let line_h = 15.0f32;
    let pad = 8.0f32;
    let panel_h = pad * 2.0 + lines.len() as f32 * line_h + 2.0;
    let rect = RectF {
        X: (screen_w - panel_w - 12.0).max(0.0),
        Y: 10.0,
        Width: panel_w,
        Height: panel_h,
    };
    unsafe {
        let mut path: *mut GpPath = null_mut();
        GdipCreatePath(FillModeAlternate, &mut path);
        if !path.is_null() {
            let r = 6.0f32.min(rect.Height / 2.0);
            let rr = RectF { X: rect.X, Y: rect.Y, Width: rect.Width, Height: rect.Height };
            rounded_rect_path(path, rr, r);
            let mut brush: *mut GpSolidFill = null_mut();
            GdipCreateSolidFill(0xB8000000, &mut brush);
            if !brush.is_null() {
                GdipFillPath(g, brush as *mut GpBrush, path);
                GdipDeleteBrush(brush as *mut GpBrush);
            }
            GdipDeletePath(path);
        }
        for (i, line) in lines.iter().enumerate() {
            let r = RectF {
                X: rect.X + pad,
                Y: rect.Y + pad + i as f32 * line_h,
                Width: rect.Width - pad * 2.0,
                Height: line_h,
            };
            draw_text_basic(g, family, line, 12.0, r, 0xE0FFFFFF, StringAlignmentNear);
        }
    }
}

fn draw_resize_handles(g: *mut GpGraphics, rect: Rect) {    unsafe {
        let (l, t, r, b) = (rect.x, rect.y, rect.x + rect.width, rect.y + rect.height);
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let s = 5.0;
        let points = [
            (l, t),
            (cx, t),
            (r, t),
            (r, cy),
            (r, b),
            (cx, b),
            (l, b),
            (l, cy),
        ];
        let mut brush: *mut GpSolidFill = null_mut();
        GdipCreateSolidFill(0xFFFFFFFF, &mut brush);
        if brush.is_null() {
            return;
        }
        for (x, y) in points {
            GdipFillRectangle(g, brush as *mut GpBrush, x - s / 2.0, y - s / 2.0, s, s);
        }
        GdipDeleteBrush(brush as *mut GpBrush);
    }
}

/// Draw one translated overlay item with rounded background + shadow.
fn draw_text(
    g: *mut GpGraphics,
    family: *mut GpFontFamily,
    item: &OverlayText,
    style: &OverlayStyle,
    opacity: f32,
) {
    let b = item.box_;
    let pad = 4.0;
    let bg_rect = RectF {
        X: b.x - pad,
        Y: b.y - pad,
        Width: b.width + pad * 2.0,
        Height: b.height + pad * 2.0,
    };
    let bg_color = parse_color(&style.background_color, opacity * style.background_opacity);
    let text_color = parse_color(&style.text_color, opacity);

    unsafe {
        // Background rounded rect.
        let mut path: *mut GpPath = null_mut();
        GdipCreatePath(FillModeAlternate, &mut path);
        if !path.is_null() {
            let r = style.corner_radius.clamp(0.0, bg_rect.Width / 2.0).min(bg_rect.Height / 2.0);
            rounded_rect_path(path, bg_rect, r);
            let mut brush: *mut GpSolidFill = null_mut();
            GdipCreateSolidFill(bg_color, &mut brush);
            if !brush.is_null() {
                GdipFillPath(g, brush as *mut GpBrush, path);
                GdipDeleteBrush(brush as *mut GpBrush);
            }
            GdipDeletePath(path);
        }

        let mut font: *mut GpFont = null_mut();
        GdipCreateFont(family, item.font_size, FontStyleRegular.0, UnitPixel, &mut font);
        if font.is_null() {
            return;
        }

        let mut format: *mut GpStringFormat = null_mut();
        GdipCreateStringFormat(0, 0, &mut format);
        if format.is_null() {
            GdipDeleteFont(font);
            return;
        }
        GdipSetStringFormatAlign(format, StringAlignmentCenter);
        GdipSetStringFormatLineAlign(format, StringAlignmentCenter);

        let text_rect = RectF {
            X: b.x,
            Y: b.y,
            Width: b.width,
            Height: b.height,
        };

        // Shadow: draw offset semi-transparent black text first.
        if style.shadow {
            let mut shadow_brush: *mut GpSolidFill = null_mut();
            GdipCreateSolidFill(0x80000000, &mut shadow_brush);
            if !shadow_brush.is_null() {
                let sr = RectF {
                    X: text_rect.X + 1.5,
                    Y: text_rect.Y + 1.5,
                    Width: text_rect.Width,
                    Height: text_rect.Height,
                };
                draw_string(g, &item.text, font, &sr, format, shadow_brush as *mut GpBrush);
                GdipDeleteBrush(shadow_brush as *mut GpBrush);
            }
        }

        let mut text_brush: *mut GpSolidFill = null_mut();
        GdipCreateSolidFill(text_color, &mut text_brush);
        if !text_brush.is_null() {
            draw_string(g, &item.text, font, &text_rect, format, text_brush as *mut GpBrush);
            GdipDeleteBrush(text_brush as *mut GpBrush);
        }

        GdipDeleteStringFormat(format);
        GdipDeleteFont(font);
    }
}

fn draw_text_basic(
    g: *mut GpGraphics,
    family: *mut GpFontFamily,
    text: &str,
    font_size: f32,
    rect: RectF,
    color: u32,
    align: StringAlignment,
) {
    unsafe {
        let mut font: *mut GpFont = null_mut();
        GdipCreateFont(family, font_size, FontStyleRegular.0, UnitPixel, &mut font);
        if font.is_null() {
            return;
        }
        let mut format: *mut GpStringFormat = null_mut();
        GdipCreateStringFormat(0, 0, &mut format);
        if format.is_null() {
            GdipDeleteFont(font);
            return;
        }
        GdipSetStringFormatAlign(format, align);
        GdipSetStringFormatLineAlign(format, StringAlignmentNear);
        let mut brush: *mut GpSolidFill = null_mut();
        GdipCreateSolidFill(color, &mut brush);
        if !brush.is_null() {
            draw_string(g, text, font, &rect, format, brush as *mut GpBrush);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
        GdipDeleteStringFormat(format);
        GdipDeleteFont(font);
    }
}

unsafe fn draw_string(
    g: *mut GpGraphics,
    text: &str,
    font: *mut GpFont,
    rect: &RectF,
    format: *mut GpStringFormat,
    brush: *mut GpBrush,
) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    GdipDrawString(
        g,
        PCWSTR(wide.as_ptr()),
        -1,
        font,
        rect,
        format,
        brush,
    );
}

fn rounded_rect_path(path: *mut GpPath, r: RectF, radius: f32) {
    unsafe {
        let (x, y, w, h) = (r.X, r.Y, r.Width, r.Height);
        let d = radius * 2.0;
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
    }
}

/// Parse "#RRGGBB" or "#AARRGGBB" into an ARGB u32; multiply alpha by `opacity`.
pub fn parse_color(s: &str, opacity: f32) -> u32 {
    let t = s.trim().trim_start_matches('#');
    let hex = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).unwrap_or(0);
    let (a, r, g, b) = if t.len() >= 8 {
        (hex(0), hex(2), hex(4), hex(6))
    } else {
        (255, hex(0), hex(2), hex(4))
    };
    let alpha = ((a as f32) * opacity.clamp(0.0, 1.0)) as u32;
    (alpha << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// WndProc is provided by app.rs (needs app state). Forward declaration.
pub unsafe extern "system" fn class_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    super::app::wnd_proc(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn hit_test_finds_corners_and_edges() {
        let rect = r(100.0, 100.0, 200.0, 100.0);
        assert_eq!(hit_test_resize(rect, 100.0, 100.0), Some(ResizeHandle::TopLeft));
        assert_eq!(hit_test_resize(rect, 300.0, 200.0), Some(ResizeHandle::BottomRight));
        assert_eq!(hit_test_resize(rect, 200.0, 100.0), Some(ResizeHandle::Top));
        assert_eq!(hit_test_resize(rect, 100.0, 150.0), Some(ResizeHandle::Left));
        assert_eq!(hit_test_resize(rect, 300.0, 150.0), Some(ResizeHandle::Right));
        assert_eq!(hit_test_resize(rect, 200.0, 200.0), Some(ResizeHandle::Bottom));
        // Inside, far from edges → no handle.
        assert_eq!(hit_test_resize(rect, 200.0, 150.0), None);
    }

    #[test]
    fn resize_bottom_right_keeps_top_left() {
        let orig = r(100.0, 100.0, 200.0, 100.0);
        let out = apply_resize(orig, ResizeHandle::BottomRight, 400.0, 300.0);
        assert_eq!(out, r(100.0, 100.0, 300.0, 200.0));
    }

    #[test]
    fn resize_left_keeps_right_edge_and_min_width() {
        let orig = r(100.0, 100.0, 200.0, 100.0);
        let out = apply_resize(orig, ResizeHandle::Left, 500.0, 150.0);
        // Right edge stays at 300; width clamped to MIN_W=24.
        assert_eq!(out.x, 300.0 - 24.0);
        assert_eq!(out.width, 24.0);
    }

    #[test]
    fn resize_top_reduces_height_keeping_bottom() {
        let orig = r(100.0, 100.0, 200.0, 100.0);
        let out = apply_resize(orig, ResizeHandle::Top, 150.0, 140.0);
        assert_eq!(out.y, 140.0);
        assert_eq!(out.height, 60.0);
    }
}

/// Register the overlay window class. Called once at startup.
pub fn register_overlay_class() -> anyhow::Result<()> {
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .map_err(|e| anyhow::anyhow!("GetModuleHandleW: {e}"))?;
        let wc = WNDCLASSW {
            style: windows::Win32::UI::WindowsAndMessaging::WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(class_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: windows::core::w!("ScreenTranslatorOverlay"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(anyhow::anyhow!(
                "RegisterClassW failed (class already registered?)"
            ));
        }
    }
    Ok(())
}












