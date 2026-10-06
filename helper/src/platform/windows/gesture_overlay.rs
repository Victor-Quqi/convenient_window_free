use super::hints::HintWindow;
use crate::platform::{Point, Rect};
use windows::core::w;
use windows::Win32::Foundation::{BOOL, COLORREF, HANDLE, HWND, LPARAM, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    EnumDisplayMonitors, GdiFlush, GetMonitorInfoW, GetTextExtentPoint32W, SelectObject, SetBkMode,
    SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, HBITMAP, HDC, HGDIOBJ, HMONITOR,
    MONITORINFO, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, ShowWindow, UpdateLayeredWindow, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWNA,
    ULW_ALPHA,
};

pub(super) struct GestureHintManager {
    windows: Vec<GestureLayer>,
    text_cache: Option<(String, u32, i32, TextMask)>,
    gesture_id: Option<u64>,
    last: Option<Point>,
}

struct GestureLayer {
    window: HintWindow,
    bounds: Rect,
    dpi: u32,
    dib: Option<Dib>,
    stroke: Option<StrokeSurface>,
    badge: Option<Rect>,
    presented: bool,
}

impl GestureLayer {
    fn reset(&mut self) {
        self.window.hide();
        self.dib = None;
        self.stroke = None;
        self.badge = None;
        self.presented = false;
    }
}

impl GestureHintManager {
    pub(super) fn new() -> Self {
        Self {
            windows: Vec::new(),
            text_cache: None,
            gesture_id: None,
            last: None,
        }
    }

    pub(super) fn update(&mut self, id: u64, points: &[Point], label: Option<&str>) {
        if self.gesture_id != Some(id) {
            self.hide();
            self.gesture_id = Some(id);
        }
        let previous = self.last;
        let Some(cursor) = points.last().copied().or(previous) else {
            return;
        };
        self.last = Some(cursor);
        let mut monitors = Vec::<(Rect, Rect)>::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(collect_monitor),
                LPARAM((&mut monitors as *mut Vec<(Rect, Rect)>) as isize),
            );
        }
        for (index, (bounds, work)) in monitors.iter().copied().enumerate() {
            if index == self.windows.len() {
                self.windows.push(GestureLayer {
                    window: HintWindow::create(),
                    bounds: Rect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    dpi: 96,
                    dib: None,
                    stroke: None,
                    badge: None,
                    presented: false,
                });
            }
            let layer = &mut self.windows[index];
            let hwnd = HWND(layer.window.hwnd as *mut core::ffi::c_void);
            if layer.bounds != bounds {
                layer.reset();
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        bounds.left,
                        bounds.top,
                        1,
                        1,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                }
                layer.bounds = bounds;
            }
            let dpi = unsafe { GetDpiForWindow(hwnd).max(96) };
            if layer.dpi != dpi {
                layer.reset();
                layer.dpi = dpi;
            }
            let label = label.filter(|s| bounds.contains(cursor) && !s.trim().is_empty());
            let text = if let Some(label) = label {
                if !self
                    .text_cache
                    .as_ref()
                    .is_some_and(|(name, scale, width, _)| {
                        name == label && *scale == dpi && *width == work.width()
                    })
                {
                    self.text_cache = text_mask(label, dpi, work.width())
                        .map(|mask| (label.to_owned(), dpi, work.width(), mask));
                }
                self.text_cache.as_ref().map(|(_, _, _, mask)| mask)
            } else {
                None
            };
            let badge = text.map(|text| badge_rect(cursor, text, work, dpi));
            if layer.stroke.is_none() {
                let margin = dip(3.0, dpi).ceil() as i32;
                let touches = previous.iter().chain(points).any(|p| {
                    p.x >= bounds.left - margin
                        && p.x < bounds.right + margin
                        && p.y >= bounds.top - margin
                        && p.y < bounds.bottom + margin
                });
                // A segment can cross a monitor even when both endpoints are outside it.
                let crosses = previous
                    .into_iter()
                    .chain(points.iter().copied())
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|pair| {
                        segment_bounds(pair[0], pair[1], margin)
                            .and_then(|r| intersect(r, bounds))
                            .is_some()
                    });
                if !touches && !crosses && badge.is_none() {
                    continue;
                }
                layer.stroke = Some(StrokeSurface::new(bounds));
                layer.dib = Dib::new(bounds.width(), bounds.height());
            }
            let Some(dib) = layer.dib.as_mut() else {
                continue;
            };
            let stroke = layer.stroke.as_mut().unwrap();
            let mut dirty = stroke.append(previous, points, dpi);
            let pixels = dib.pixels();
            for &offset in &stroke.changed {
                pixels[offset * 4..offset * 4 + 4]
                    .copy_from_slice(&stroke.pixels[offset * 4..offset * 4 + 4]);
            }
            if let Some(old) = layer.badge {
                copy_rect(pixels, &stroke.pixels, bounds, old);
                dirty = union(dirty, Some(old));
            }
            if let (Some(text), Some(badge)) = (text, badge) {
                paint_badge(pixels, bounds, text, badge, dpi);
                dirty = union(dirty, intersect(badge, bounds));
            }
            layer.badge = badge;
            if dirty.is_some() {
                layer.presented = present(layer.window, dib, bounds, layer.presented);
            }
        }
        for layer in self.windows.iter_mut().skip(monitors.len()) {
            layer.reset();
        }
    }

    pub(super) fn hide(&mut self) {
        for layer in &mut self.windows {
            layer.reset();
        }
        self.gesture_id = None;
        self.last = None;
    }
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Rect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        }),
        (a, b) => a.or(b),
    }
}

fn segment_bounds(a: Point, b: Point, margin: i32) -> Option<Rect> {
    Some(Rect {
        left: a.x.min(b.x) - margin,
        top: a.y.min(b.y) - margin,
        right: a.x.max(b.x) + margin + 1,
        bottom: a.y.max(b.y) + margin + 1,
    })
}

struct StrokeSurface {
    bounds: Rect,
    coverage: [Vec<u8>; 2],
    pixels: Vec<u8>,
    changed: Vec<usize>,
}

impl StrokeSurface {
    fn new(bounds: Rect) -> Self {
        let count = bounds.width() as usize * bounds.height() as usize;
        Self {
            bounds,
            coverage: [vec![0; count], vec![0; count]],
            pixels: vec![0; count * 4],
            changed: Vec::new(),
        }
    }

    fn append(&mut self, previous: Option<Point>, points: &[Point], dpi: u32) -> Option<Rect> {
        self.changed.clear();
        let mut last = previous;
        for &point in points {
            if let Some(last) = last {
                for (index, radius) in [2.0, 1.25].into_iter().enumerate() {
                    raster_segment(
                        &mut self.coverage[index],
                        &mut self.changed,
                        self.bounds,
                        last,
                        point,
                        dip(radius, dpi),
                    );
                }
            }
            last = Some(point);
        }
        let width = self.bounds.width() as usize;
        let mut dirty = None;
        for &offset in &self.changed {
            let pixel = &mut self.pixels[offset * 4..offset * 4 + 4];
            pixel.fill(0);
            blend(
                pixel,
                [20, 28, 43, 48],
                self.coverage[0][offset] as f32 / 255.0,
            );
            blend(
                pixel,
                [80, 143, 235, 238],
                self.coverage[1][offset] as f32 / 255.0,
            );
            let x = self.bounds.left + (offset % width) as i32;
            let y = self.bounds.top + (offset / width) as i32;
            dirty = union(
                dirty,
                Some(Rect {
                    left: x,
                    top: y,
                    right: x + 1,
                    bottom: y + 1,
                }),
            );
        }
        dirty
    }
}

fn copy_rect(destination: &mut [u8], source: &[u8], bounds: Rect, rect: Rect) {
    if let Some(rect) = intersect(bounds, rect) {
        for y in rect.top..rect.bottom {
            let offset = ((y - bounds.top) as usize * bounds.width() as usize
                + (rect.left - bounds.left) as usize)
                * 4;
            let end = offset + rect.width() as usize * 4;
            destination[offset..end].copy_from_slice(&source[offset..end]);
        }
    }
}

fn badge_rect(cursor: Point, text: &TextMask, work: Rect, dpi: u32) -> Rect {
    label_rect(
        cursor,
        text.width + 2 * dip(9.0, dpi).round() as i32,
        text.height + 2 * dip(6.0, dpi).round() as i32,
        work,
        dpi,
    )
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        let convert = |r: RECT| Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        let monitors = &mut *(data.0 as *mut Vec<(Rect, Rect)>);
        monitors.push((convert(info.rcMonitor), convert(info.rcWork)));
    }
    true.into()
}

#[cfg(test)]
struct Frame {
    rect: Rect,
    pixels: Vec<u8>,
}

fn dip(value: f32, dpi: u32) -> f32 {
    value * dpi as f32 / 96.0
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let rect = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (rect.width() > 0 && rect.height() > 0).then_some(rect)
}

fn label_rect(cursor: Point, width: i32, height: i32, work: Rect, dpi: u32) -> Rect {
    let gap = dip(16.0, dpi).round() as i32;
    let inset = dip(6.0, dpi).round() as i32;
    let left = if cursor.x + gap + width + inset <= work.right {
        cursor.x + gap
    } else {
        cursor.x - gap - width
    };
    let top = if cursor.y + gap + height + inset <= work.bottom {
        cursor.y + gap
    } else {
        cursor.y - gap - height
    };
    let left = left.clamp(
        work.left + inset,
        (work.right - width - inset).max(work.left + inset),
    );
    let top = top.clamp(
        work.top + inset,
        (work.bottom - height - inset).max(work.top + inset),
    );
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

#[cfg(test)]
fn render(
    points: &[Point],
    label: Option<&str>,
    bounds: Rect,
    work: Rect,
    dpi: u32,
) -> Option<Frame> {
    let text = label
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| text_mask(s, dpi, work.width()));
    let mut stroke = StrokeSurface::new(bounds);
    stroke.append(None, points, dpi);
    if let Some(text) = text.as_ref() {
        let badge = badge_rect(*points.last()?, text, work, dpi);
        paint_badge(&mut stroke.pixels, bounds, text, badge, dpi);
    }
    Some(Frame {
        rect: bounds,
        pixels: stroke.pixels,
    })
}

#[cfg(test)]
fn render_with_text(
    points: &[Point],
    text: Option<&TextMask>,
    bounds: Rect,
    work: Rect,
    dpi: u32,
) -> Option<Frame> {
    if points.len() < 2 {
        return None;
    }
    let margin = dip(3.0, dpi).ceil() as i32;
    let stroke = Rect {
        left: points.iter().map(|p| p.x).min()? - margin,
        top: points.iter().map(|p| p.y).min()? - margin,
        right: points.iter().map(|p| p.x).max()? + margin + 1,
        bottom: points.iter().map(|p| p.y).max()? + margin + 1,
    };
    let badge = text.as_ref().map(|text| {
        let pad_x = dip(9.0, dpi).round() as i32;
        let pad_y = dip(6.0, dpi).round() as i32;
        label_rect(
            *points.last().unwrap(),
            text.width + 2 * pad_x,
            text.height + 2 * pad_y,
            work,
            dpi,
        )
    });
    let mut rect = intersect(stroke, bounds)?;
    if let Some(badge) = badge {
        rect = intersect(
            Rect {
                left: rect.left.min(badge.left),
                top: rect.top.min(badge.top),
                right: rect.right.max(badge.right),
                bottom: rect.bottom.max(badge.bottom),
            },
            bounds,
        )?;
    }
    let width = rect.width() as usize;
    let height = rect.height() as usize;
    let mut pixels = vec![0; width * height * 4];
    // Coverage is combined before compositing, so joints never become darker.
    for (radius, color) in [
        (dip(2.0, dpi), [20, 28, 43, 48]),
        (dip(1.25, dpi), [80, 143, 235, 238]),
    ] {
        let mut coverage = vec![0u8; width * height];
        let mut touched = Vec::new();
        for pair in points.windows(2) {
            raster_segment(&mut coverage, &mut touched, rect, pair[0], pair[1], radius);
        }
        touched.sort_unstable();
        touched.dedup();
        for offset in touched {
            blend(
                &mut pixels[offset * 4..offset * 4 + 4],
                color,
                coverage[offset] as f32 / 255.0,
            );
        }
    }
    if let (Some(text), Some(badge)) = (text, badge) {
        paint_badge(&mut pixels, rect, text, badge, dpi);
    }
    Some(Frame { rect, pixels })
}

fn paint_badge(pixels: &mut [u8], rect: Rect, text: &TextMask, badge: Rect, dpi: u32) {
    let width = rect.width() as usize;
    let radius = dip(6.0, dpi);
    if let Some(visible) = intersect(badge, rect) {
        for y in visible.top..visible.bottom {
            for x in visible.left..visible.right {
                let qx = ((x as f32 + 0.5) - (badge.left + badge.right) as f32 / 2.0).abs()
                    - (badge.width() as f32 / 2.0 - radius);
                let qy = ((y as f32 + 0.5) - (badge.top + badge.bottom) as f32 / 2.0).abs()
                    - (badge.height() as f32 / 2.0 - radius);
                let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                let offset = ((y - rect.top) as usize * width + (x - rect.left) as usize) * 4;
                blend(
                    &mut pixels[offset..offset + 4],
                    [30, 34, 41, 238],
                    (0.5 - distance).clamp(0.0, 1.0),
                );
            }
        }
    }
    let left = badge.left + (badge.width() - text.width) / 2;
    let top = badge.top + (badge.height() - text.height) / 2;
    for y in 0..text.height {
        for x in 0..text.width {
            if !rect.contains(Point {
                x: left + x,
                y: top + y,
            }) {
                continue;
            }
            let offset =
                ((top + y - rect.top) as usize * width + (left + x - rect.left) as usize) * 4;
            let coverage = text.coverage[(y * text.width + x) as usize] as f32 / 255.0;
            blend(
                &mut pixels[offset..offset + 4],
                [248, 249, 251, 255],
                coverage,
            );
        }
    }
}

fn raster_segment(
    coverage: &mut [u8],
    touched: &mut Vec<usize>,
    rect: Rect,
    a: Point,
    b: Point,
    radius: f32,
) {
    let dx = (b.x - a.x) as f32;
    let dy = (b.y - a.y) as f32;
    let length = dx * dx + dy * dy;
    let extent = radius + 0.5;
    let top = ((a.y.min(b.y) as f32 - extent).floor() as i32).max(rect.top);
    let bottom = ((a.y.max(b.y) as f32 + extent).ceil() as i32).min(rect.bottom);
    for y in top..bottom {
        let py = y as f32 + 0.5;
        // Bound each scanline, avoiding a full rectangular scan for long diagonals.
        let (t0, t1) = if dy.abs() > f32::EPSILON {
            let u = (py - extent - a.y as f32) / dy;
            let v = (py + extent - a.y as f32) / dy;
            (u.min(v).clamp(0.0, 1.0), u.max(v).clamp(0.0, 1.0))
        } else {
            (0.0, 1.0)
        };
        let x0 = a.x as f32 + dx * t0;
        let x1 = a.x as f32 + dx * t1;
        let left = ((x0.min(x1) - extent).floor() as i32).max(rect.left);
        let right = ((x0.max(x1) + extent).ceil() as i32).min(rect.right);
        for x in left..right {
            let px = x as f32 + 0.5 - a.x as f32;
            let py = py - a.y as f32;
            let t = if length > 0.0 {
                ((px * dx + py * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let ex = px - t * dx;
            let ey = py - t * dy;
            let distance = (ex * ex + ey * ey).sqrt();
            let value = ((extent - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
            let offset = (y - rect.top) as usize * rect.width() as usize + (x - rect.left) as usize;
            if value > coverage[offset] {
                touched.push(offset);
            }
            coverage[offset] = coverage[offset].max(value);
        }
    }
}

fn blend(pixel: &mut [u8], rgba: [u8; 4], coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let alpha = rgba[3] as f32 / 255.0 * coverage;
    for (channel, value) in pixel[..3].iter_mut().zip([rgba[2], rgba[1], rgba[0]]) {
        *channel = (value as f32 * alpha + *channel as f32 * (1.0 - alpha)).round() as u8;
    }
    pixel[3] = (255.0 * alpha + pixel[3] as f32 * (1.0 - alpha)).round() as u8;
}

struct TextMask {
    width: i32,
    height: i32,
    coverage: Vec<u8>,
}

fn text_mask(label: &str, dpi: u32, work_width: i32) -> Option<TextMask> {
    let max_width = (dip(180.0, dpi).round() as i32)
        .min(work_width - dip(30.0, dpi).round() as i32)
        .max(1);
    let height = dip(17.0, dpi).ceil() as i32;
    let mut dib = Dib::new(max_width, height)?;
    unsafe {
        let font = CreateFontW(
            -(dip(13.0, dpi).round() as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            4,
            0,
            w!("Microsoft YaHei UI"),
        );
        if font.0.is_null() {
            return None;
        }
        let old_font = SelectObject(dib.dc, font);
        let mut text: Vec<u16> = label
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect::<String>()
            .encode_utf16()
            .collect();
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dib.dc, &text, &mut size);
        let width = size.cx.max(1).min(max_width);
        let _ = SetBkMode(dib.dc, TRANSPARENT);
        let _ = SetTextColor(dib.dc, COLORREF(0x00FFFFFF));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        DrawTextW(
            dib.dc,
            &mut text,
            &mut rect,
            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        let _ = GdiFlush();
        SelectObject(dib.dc, old_font);
        let _ = DeleteObject(font);
        let mut coverage = Vec::with_capacity((width * height) as usize);
        for row in dib.pixels().chunks_exact((max_width * 4) as usize) {
            coverage.extend(
                row[..(width * 4) as usize]
                    .chunks_exact(4)
                    .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2])),
            );
        }
        Some(TextMask {
            width,
            height,
            coverage,
        })
    }
}

struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    len: usize,
}

impl Dib {
    fn new(width: i32, height: i32) -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(HDC::default());
            if dc.0.is_null() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) =
                CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
            else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bitmap);
            let mut dib = Self {
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                len: width as usize * height as usize * 4,
            };
            dib.pixels().fill(0);
            Some(dib)
        }
    }
    fn pixels(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

fn present(window: HintWindow, dib: &Dib, bounds: Rect, initialized: bool) -> bool {
    let hwnd = HWND(window.hwnd as *mut core::ffi::c_void);
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let position = POINT {
        x: bounds.left,
        y: bounds.top,
    };
    let size = SIZE {
        cx: bounds.width(),
        cy: bounds.height(),
    };
    let source = POINT::default();
    // Submit the complete cached bitmap; dirty-only layered updates can leave
    // later segments invisible even when Windows reports a successful update.
    unsafe {
        if UpdateLayeredWindow(
            hwnd,
            HDC::default(),
            Some(&position),
            Some(&size),
            dib.dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
        .is_err()
        {
            return false;
        }
        if !initialized {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an interactive Windows desktop for screen pixel checks"]
    fn gesture_native_trail_survives_message_pumps_and_updates() {
        use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
        use windows::Win32::UI::HiDpi::{
            SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            DestroyWindow, DispatchMessageW, PeekMessageW, SetWindowDisplayAffinity,
            TranslateMessage, MSG, PM_REMOVE, WDA_NONE,
        };
        let old_dpi =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let mut hint = GestureHintManager::new();
        let mut monitors = Vec::<(Rect, Rect)>::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(collect_monitor),
                LPARAM((&mut monitors as *mut Vec<(Rect, Rect)>) as isize),
            );
        }
        let bounds = monitors[0].0;
        let x = bounds.left + 160;
        let y = bounds.top + 120;
        let mut samples = Vec::new();
        for step in 0..6 {
            let points = if step == 0 {
                vec![Point { x, y }, Point { x, y: y + 40 }]
            } else {
                vec![Point {
                    x,
                    y: y + 40 + step * 40,
                }]
            };
            hint.update(77, &points, None);
            for layer in &hint.windows {
                unsafe {
                    SetWindowDisplayAffinity(HWND(layer.window.hwnd as *mut _), WDA_NONE).unwrap();
                }
            }
            let mut message = MSG::default();
            unsafe {
                while PeekMessageW(&mut message, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
            unsafe {
                let dc = GetDC(HWND::default());
                let old = GetPixel(dc, x, y + 20).0;
                let new = GetPixel(dc, x, y + 30 + step * 40).0;
                let _ = ReleaseDC(HWND::default(), dc);
                samples.push((old, new));
            }
        }
        hint.hide();
        for layer in &hint.windows {
            unsafe {
                let _ = DestroyWindow(HWND(layer.window.hwnd as *mut _));
            }
        }
        unsafe {
            SetThreadDpiAwarenessContext(old_dpi);
        }
        for (step, (old, new)) in samples.into_iter().enumerate() {
            let blue = |color: u32| ((color >> 16) & 255) > (color & 255) + 80;
            assert!(
                blue(old) && blue(new),
                "frame {step}: old={old:06x}, new={new:06x}"
            );
        }
    }

    #[test]
    fn gesture_incremental_batches_match_whole_stroke_and_keep_old_pixels() {
        let bounds = Rect {
            left: -640,
            top: -400,
            right: 0,
            bottom: 80,
        };
        let points: Vec<_> = (0..1600)
            .map(|i| Point {
                x: -600 + (i % 550),
                y: -200 + ((i as f32 * 0.05).sin() * 140.0) as i32,
            })
            .collect();
        for dpi in [96, 144, 192] {
            let mut stroke = StrokeSurface::new(bounds);
            let mut last = None;
            for batch in points.chunks(17) {
                stroke.append(last, batch, dpi);
                last = batch.last().copied();
            }
            let expected = render_with_text(&points, None, bounds, bounds, dpi).unwrap();
            for y in bounds.top..bounds.bottom {
                for x in bounds.left..bounds.right {
                    let offset = ((y - bounds.top) * bounds.width() + x - bounds.left) as usize * 4;
                    let value = if expected.rect.contains(Point { x, y }) {
                        let source = ((y - expected.rect.top) * expected.rect.width() + x
                            - expected.rect.left) as usize
                            * 4;
                        &expected.pixels[source..source + 4]
                    } else {
                        &[0; 4]
                    };
                    assert_eq!(&stroke.pixels[offset..offset + 4], value);
                }
            }
            let before = stroke.pixels.clone();
            stroke.append(
                Some(Point { x: -30, y: 40 }),
                &[Point { x: -20, y: 50 }],
                dpi,
            );
            assert!(stroke.changed.len() < 600);
            let unchanged_rows = (30 - bounds.top) as usize * bounds.width() as usize * 4;
            assert_eq!(&stroke.pixels[..unchanged_rows], &before[..unchanged_rows]);
            assert!(stroke.append(last, &[], dpi).is_none());
        }
    }

    #[test]
    fn gesture_native_layers_are_click_through_and_hide_together() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DestroyWindow, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible,
            GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
        };
        let foreground = unsafe { GetForegroundWindow() };
        let mut hint = GestureHintManager::new();
        hint.update(
            1,
            &[Point { x: 80, y: 80 }, Point { x: 80, y: 180 }],
            Some("粘贴"),
        );
        assert!(!hint.windows.is_empty());
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        for layer in &hint.windows {
            let flags =
                unsafe { GetWindowLongPtrW(HWND(layer.window.hwnd as *mut _), GWL_EXSTYLE) } as u32;
            assert_ne!(flags & WS_EX_TRANSPARENT.0, 0);
            assert_ne!(flags & WS_EX_NOACTIVATE.0, 0);
        }
        let bounds = hint.windows[0].bounds;
        let mut monitors = Vec::<(Rect, Rect)>::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(collect_monitor),
                LPARAM((&mut monitors as *mut Vec<(Rect, Rect)>) as isize),
            );
        }
        let work = monitors.iter().find(|(rect, _)| *rect == bounds).unwrap().1;
        let mut points = vec![Point { x: 80, y: 80 }, Point { x: 80, y: 180 }];
        for (x, y, label) in [
            (180, 180, None),
            (60, 180, Some("复制")),
            (60, 60, Some("关闭窗口")),
            (bounds.width() - 10, bounds.height() - 10, Some("粘贴")),
            (220, 220, None),
        ] {
            points.push(Point {
                x: bounds.left + x,
                y: bounds.top + y,
            });
            hint.update(1, &points[points.len() - 1..], label);
            let layer = &mut hint.windows[0];
            assert!(layer.presented);
            let mut actual = RECT::default();
            unsafe { GetWindowRect(HWND(layer.window.hwnd as *mut _), &mut actual).unwrap() };
            assert_eq!(
                (actual.left, actual.top, actual.right, actual.bottom),
                (bounds.left, bounds.top, bounds.right, bounds.bottom)
            );
            let expected = render(&points, label, bounds, work, layer.dpi).unwrap();
            assert_eq!(
                layer.dib.as_mut().unwrap().pixels(),
                expected.pixels.as_slice()
            );
        }
        hint.hide();
        for layer in &hint.windows {
            let hwnd = HWND(layer.window.hwnd as *mut _);
            assert!(!unsafe { IsWindowVisible(hwnd).as_bool() });
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

    #[test]
    #[ignore = "manual release-mode rendering benchmark"]
    fn gesture_large_stroke_render_budget() {
        use std::time::Instant;
        let bounds = Rect {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2160,
        };
        let points: Vec<_> = (0..40_000)
            .map(|i| {
                let row = i / 1800;
                let x = if row % 2 == 0 {
                    i % 1800
                } else {
                    1799 - i % 1800
                };
                Point {
                    x: 60 + 2 * x,
                    y: 60 + row * 80,
                }
            })
            .collect();
        let mut stroke = StrokeSurface::new(bounds);
        let mut previous = None;
        let mut times = Vec::new();
        for batch in points.chunks(16) {
            let start = Instant::now();
            std::hint::black_box(stroke.append(previous, batch, 192));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            previous = batch.last().copied();
        }
        let mean = |slice: &[f64]| slice.iter().sum::<f64>() / slice.len() as f64;
        eprintln!("4K incremental CPU, 16 new points/frame: first 100 {:.3} ms; last 100 {:.3} ms; total 40000 points",
            mean(&times[..100]), mean(&times[times.len()-100..]));
        let mut old = Vec::new();
        for &point in &points {
            crate::core::gesture::append_gesture_point(&mut old, point, false);
        }
        let start = Instant::now();
        for _ in 0..5 {
            std::hint::black_box(render_with_text(&old, None, bounds, bounds, 192).unwrap());
        }
        eprintln!(
            "4K full historical redraw CPU: {:.3} ms/frame, {} recognition points",
            start.elapsed().as_secs_f64() * 200.0,
            old.len()
        );

        let mut hint = GestureHintManager::new();
        let mut monitors = Vec::<(Rect, Rect)>::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(collect_monitor),
                LPARAM((&mut monitors as *mut Vec<(Rect, Rect)>) as isize),
            );
        }
        let native_bounds = monitors[0].0;
        let native_points: Vec<_> = points
            .iter()
            .map(|point| Point {
                x: native_bounds.left + 30 + point.x * (native_bounds.width() - 60) / 3840,
                y: native_bounds.top + 30 + point.y * (native_bounds.height() - 60) / 2160,
            })
            .collect();
        let start = Instant::now();
        hint.update(42, &native_points[..32000], Some("复制"));
        eprintln!(
            "Native initial backlog (32000 points): {:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let start = Instant::now();
        for batch in native_points[32000..].chunks(16) {
            hint.update(42, batch, Some("复制"));
        }
        eprintln!(
            "Native incremental draw + label + upload: {:.3} ms/frame",
            start.elapsed().as_secs_f64() * 2.0
        );
        hint.hide();
        for layer in &hint.windows {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(HWND(
                    layer.window.hwnd as *mut _,
                ));
            }
        }
    }

    #[test]
    fn gesture_label_stays_in_each_monitors_work_area_at_all_scales() {
        for dpi in [96, 144, 192] {
            for work in [
                Rect {
                    left: 0,
                    top: 0,
                    right: 1280,
                    bottom: 680,
                },
                Rect {
                    left: -2560,
                    top: -300,
                    right: 0,
                    bottom: 1100,
                },
            ] {
                for cursor in [
                    Point {
                        x: work.left,
                        y: work.top,
                    },
                    Point {
                        x: work.right - 1,
                        y: work.bottom - 1,
                    },
                    Point {
                        x: work.right - 1,
                        y: work.top,
                    },
                ] {
                    let rect = label_rect(
                        cursor,
                        dip(100.0, dpi) as i32,
                        dip(29.0, dpi) as i32,
                        work,
                        dpi,
                    );
                    assert!(rect.left >= work.left && rect.right <= work.right);
                    assert!(rect.top >= work.top && rect.bottom <= work.bottom);
                    assert!(!rect.contains(cursor));
                }
            }
        }
    }

    #[test]
    fn gesture_stroke_has_partial_coverage_and_no_dark_joints() {
        let rect = Rect {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let a = Point { x: 10, y: 10 };
        let b = Point { x: 80, y: 65 };
        let mut pixels = vec![0; 10000];
        raster_segment(&mut pixels, &mut Vec::new(), rect, a, b, 1.25);
        assert!(pixels.iter().any(|&v| v > 0 && v < 255));
        let once = pixels.clone();
        raster_segment(&mut pixels, &mut Vec::new(), rect, b, a, 1.25);
        assert_eq!(once, pixels);
        assert_eq!(pixels[0], 0);
    }

    #[test]
    fn gesture_render_scales_text_and_stroke_and_clips_to_monitor() {
        let mut previous_width = 0;
        for dpi in [96, 144, 192] {
            let text = text_mask("粘贴", dpi, 1280).unwrap();
            assert!(text.width > previous_width);
            previous_width = text.width;
            assert!(text.coverage.iter().any(|&c| c > 0));
            assert_eq!(text.height, dip(17.0, dpi).ceil() as i32);
            let bounds = Rect {
                left: -1280,
                top: 0,
                right: 0,
                bottom: 720,
            };
            let points = [Point { x: -100, y: 100 }, Point { x: 100, y: 300 }];
            let frame = render(&points, None, bounds, bounds, dpi).unwrap();
            assert!(frame.rect.left >= bounds.left && frame.rect.right <= bounds.right);
            for p in frame.pixels.chunks_exact(4) {
                assert!(p[..3].iter().all(|&c| c <= p[3]));
            }
            let long = text_mask(&"很长的自定义手势名称".repeat(12), dpi, 1280).unwrap();
            assert!(long.width <= dip(180.0, dpi).round() as i32);
        }
    }

    #[test]
    fn gesture_visual_samples_use_the_production_renderer() {
        for dpi in [96, 144, 192] {
            let scale = |v: i32| dip(v as f32, dpi).round() as i32;
            let bounds = Rect {
                left: 0,
                top: 0,
                right: scale(460),
                bottom: scale(300),
            };
            let points: Vec<_> = [
                (70, 48),
                (71, 78),
                (69, 110),
                (70, 144),
                (72, 171),
                (82, 184),
                (107, 188),
                (146, 187),
                (184, 188),
                (214, 187),
            ]
            .into_iter()
            .map(|(x, y)| Point {
                x: scale(x),
                y: scale(y),
            })
            .collect();
            let frame = render(&points, Some("关闭窗口"), bounds, bounds, dpi).unwrap();
            assert!(frame.pixels.iter().any(|&p| p > 0));
            let Some(directory) = std::env::var_os("GESTURE_VISUAL_OUTPUT") else {
                continue;
            };
            std::fs::create_dir_all(&directory).unwrap();
            for (theme, background) in [("light", [247u8, 248, 250]), ("dark", [28u8, 31, 37])] {
                let mut rgb = vec![0u8; (bounds.width() * bounds.height() * 3) as usize];
                for pixel in rgb.chunks_exact_mut(3) {
                    pixel.copy_from_slice(&background);
                }
                for y in frame.rect.top..frame.rect.bottom {
                    for x in frame.rect.left..frame.rect.right {
                        let source = ((y - frame.rect.top) * frame.rect.width() + x
                            - frame.rect.left) as usize
                            * 4;
                        let target = (y * bounds.width() + x) as usize * 3;
                        let alpha = frame.pixels[source + 3] as f32 / 255.0;
                        for c in 0..3 {
                            rgb[target + c] = (frame.pixels[source + 2 - c] as f32
                                + background[c] as f32 * (1.0 - alpha))
                                .round() as u8;
                        }
                    }
                }
                let path =
                    std::path::Path::new(&directory).join(format!("gesture-{dpi}-{theme}.png"));
                let file = std::fs::File::create(path).unwrap();
                let mut encoder =
                    png::Encoder::new(file, bounds.width() as u32, bounds.height() as u32);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&rgb)
                    .unwrap();
            }
        }
    }
}
