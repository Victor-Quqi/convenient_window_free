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
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    ShowWindow, UpdateLayeredWindow, SW_SHOWNA, ULW_ALPHA,
};

/// Straight RGBA and radius in DIPs: a soft dark halo under a blue core.
const HALO: ([u8; 4], f32) = ([20, 28, 43, 48], 2.0);
const CORE: ([u8; 4], f32) = ([80, 143, 235, 238], 1.25);

pub(super) struct GestureHintManager {
    layers: Vec<GestureLayer>,
    text_cache: Option<(String, u32, i32, TextMask)>,
    gesture_id: Option<u64>,
    last: Option<Point>,
    label: Option<String>,
}

impl GestureHintManager {
    pub(super) fn new() -> Self {
        Self {
            layers: Vec::new(),
            text_cache: None,
            gesture_id: None,
            last: None,
            label: None,
        }
    }

    pub(super) fn update(&mut self, id: u64, points: &[Point], label: Option<&str>) {
        if self.gesture_id != Some(id) {
            self.hide();
            self.gesture_id = Some(id);
        }
        let label = label.filter(|label| !label.trim().is_empty());
        if points.is_empty() && self.label.as_deref() == label {
            return;
        }
        self.label = label.map(str::to_owned);
        // The last drawn point joins the new segments to the stroke.
        let path: Vec<_> = self
            .last
            .into_iter()
            .chain(points.iter().copied())
            .collect();
        let Some(&cursor) = path.last() else {
            return;
        };
        self.last = Some(cursor);
        let screens = screens();
        let badge = label
            .zip(screens.iter().find(|screen| screen.bounds.contains(cursor)))
            .and_then(|(label, screen)| {
                let text = cached_text(&mut self.text_cache, label, screen)?;
                Some((text, badge_rect(cursor, text, screen.work, screen.dpi)))
            });
        let count = self.layers.len().max(screens.len());
        self.layers.resize_with(count, GestureLayer::new);
        for (layer, screen) in self.layers.iter_mut().zip(&screens) {
            layer.draw(screen, &path, badge);
        }
        for layer in self.layers.iter_mut().skip(screens.len()) {
            layer.reset();
        }
    }

    pub(super) fn hide(&mut self) {
        for layer in &mut self.layers {
            layer.reset();
        }
        self.gesture_id = None;
        self.last = None;
        self.label = None;
    }
}

fn cached_text<'a>(
    cache: &'a mut Option<(String, u32, i32, TextMask)>,
    label: &str,
    screen: &Screen,
) -> Option<&'a TextMask> {
    let width = screen.work.width();
    if !cache.as_ref().is_some_and(|(name, dpi, cached_width, _)| {
        name == label && *dpi == screen.dpi && *cached_width == width
    }) {
        *cache = text_mask(label, screen.dpi, width)
            .map(|mask| (label.to_owned(), screen.dpi, width, mask));
    }
    cache.as_ref().map(|(.., mask)| mask)
}

/// One click-through window per monitor, presenting that monitor's surface.
struct GestureLayer {
    window: HintWindow,
    surface: Option<Surface>,
    presented: bool,
}

impl GestureLayer {
    fn new() -> Self {
        Self {
            window: HintWindow::create(),
            surface: None,
            presented: false,
        }
    }

    fn draw(&mut self, screen: &Screen, path: &[Point], badge: Option<(&TextMask, Rect)>) {
        if self
            .surface
            .as_ref()
            .is_some_and(|surface| surface.monitor != screen.bounds || surface.dpi != screen.dpi)
        {
            self.reset();
        }
        let surface = self
            .surface
            .get_or_insert_with(|| Surface::new(screen.bounds, screen.dpi));
        if surface.draw(path, badge) {
            if let Some(canvas) = &surface.canvas {
                self.presented = present(self.window, &canvas.dib, canvas.rect, self.presented);
            }
        }
    }

    fn reset(&mut self) {
        self.window.hide();
        self.surface = None;
        self.presented = false;
    }
}

/// What one monitor shows of the current stroke.
struct Surface {
    monitor: Rect,
    dpi: u32,
    pen: Pen,
    canvas: Option<Canvas>,
    badge: Option<Rect>,
}

impl Surface {
    fn new(monitor: Rect, dpi: u32) -> Self {
        Self {
            monitor,
            dpi,
            pen: Pen::new(dpi),
            canvas: None,
            badge: None,
        }
    }

    /// Draws new segments and moves the badge; returns whether any pixel changed.
    fn draw(&mut self, path: &[Point], badge: Option<(&TextMask, Rect)>) -> bool {
        let badge = badge.filter(|(_, rect)| intersect(*rect, self.monitor).is_some());
        let reach = self.pen.reach.ceil() as i32;
        let needed = path
            .windows(2)
            .map(|pair| segment_bounds(pair[0], pair[1], reach))
            .chain(badge.map(|(_, rect)| rect))
            .filter_map(|rect| intersect(rect, self.monitor))
            .fold(None, |all, rect| union(all, Some(rect)));
        if let Some(needed) = needed {
            self.reserve(needed);
        }
        let Some(canvas) = self.canvas.as_mut() else {
            return false;
        };
        let mut changed = false;
        if let Some(old) = self.badge.take() {
            canvas.repaint(&self.pen, old);
            changed = true;
        }
        for pair in path.windows(2) {
            changed |= canvas.stroke(&self.pen, pair[0], pair[1]);
        }
        if let Some((text, rect)) = badge {
            paint_badge(canvas.dib.pixels(), canvas.rect, text, rect, self.dpi);
            self.badge = Some(rect);
            changed = true;
        }
        changed
    }

    /// Grows the canvas over `needed` with headroom, so a stroke that keeps extending
    /// reallocates only a logarithmic number of times; it never outgrows the monitor.
    fn reserve(&mut self, needed: Rect) {
        let current = self.canvas.as_ref().map(|canvas| canvas.rect);
        if current.is_some_and(|rect| intersect(rect, needed) == Some(needed)) {
            return;
        }
        let step = dip(128.0, self.dpi).round() as i32;
        let wanted = match current {
            None => needed.inflate(step),
            Some(rect) => {
                let x = step.max(rect.width() / 2);
                let y = step.max(rect.height() / 2);
                Rect {
                    left: if needed.left < rect.left {
                        needed.left - x
                    } else {
                        rect.left
                    },
                    top: if needed.top < rect.top {
                        needed.top - y
                    } else {
                        rect.top
                    },
                    right: if needed.right > rect.right {
                        needed.right + x
                    } else {
                        rect.right
                    },
                    bottom: if needed.bottom > rect.bottom {
                        needed.bottom + y
                    } else {
                        rect.bottom
                    },
                }
            }
        };
        let Some(mut canvas) = intersect(wanted, self.monitor).and_then(Canvas::new) else {
            return;
        };
        if let Some(old) = &self.canvas {
            canvas.adopt(&self.pen, old);
        }
        self.canvas = Some(canvas);
    }
}

/// The stroke as a function of distance from its path. Overlapping segments keep the
/// nearest one, so joints and retraced lines never darken.
struct Pen {
    reach: f32,
    step: f32,
    /// Premultiplied BGRA for each nearness.
    colors: [[u8; 4]; 256],
}

impl Pen {
    fn new(dpi: u32) -> Self {
        // Coverage falls off linearly across one pixel around each edge.
        let edge = |(_, radius): ([u8; 4], f32)| dip(radius, dpi) + 0.5;
        let reach = edge(HALO);
        let step = reach / 255.0;
        let colors = std::array::from_fn(|nearness| {
            let distance = (255 - nearness) as f32 * step;
            let mut pixel = [0; 4];
            for layer in [HALO, CORE] {
                blend(
                    &mut pixel,
                    layer.0,
                    (edge(layer) - distance).clamp(0.0, 1.0),
                );
            }
            pixel
        });
        Self {
            reach,
            step,
            colors,
        }
    }

    /// 255 on the path, 0 at and beyond the halo's outer edge.
    fn nearness(&self, distance: f32) -> u8 {
        (255.0 - distance / self.step).round().max(0.0) as u8
    }
}

/// The part of a monitor the stroke has reached: one nearness byte per pixel, plus
/// the bitmap handed to the window.
struct Canvas {
    rect: Rect,
    /// Zero where the stroke never came, so untouched pages are never committed.
    nearness: Vec<u8>,
    dib: Dib,
}

impl Canvas {
    fn new(rect: Rect) -> Option<Self> {
        Some(Self {
            rect,
            nearness: vec![0; rect.width() as usize * rect.height() as usize],
            dib: Dib::new(rect.width(), rect.height())?,
        })
    }

    /// Takes over the stroke drawn on a smaller canvas inside this one.
    fn adopt(&mut self, pen: &Pen, old: &Canvas) {
        let width = old.rect.width() as usize;
        for (row, y) in (old.rect.top..old.rect.bottom).enumerate() {
            let target = offset(self.rect, old.rect.left, y);
            self.nearness[target..target + width]
                .copy_from_slice(&old.nearness[row * width..(row + 1) * width]);
        }
        self.repaint(pen, old.rect);
    }

    fn repaint(&mut self, pen: &Pen, area: Rect) {
        let Some(area) = intersect(area, self.rect) else {
            return;
        };
        let pixels = self.dib.pixels();
        for y in area.top..area.bottom {
            let row = offset(self.rect, area.left, y);
            for offset in row..row + area.width() as usize {
                pixels[offset * 4..offset * 4 + 4]
                    .copy_from_slice(&pen.colors[self.nearness[offset] as usize]);
            }
        }
    }

    /// Brings pixels near `a`–`b` closer to the stroke; returns whether any changed.
    fn stroke(&mut self, pen: &Pen, a: Point, b: Point) -> bool {
        let rect = self.rect;
        let pixels = self.dib.pixels();
        let dx = (b.x - a.x) as f32;
        let dy = (b.y - a.y) as f32;
        let length = dx * dx + dy * dy;
        let reach = pen.reach;
        let top = ((a.y.min(b.y) as f32 - reach).floor() as i32).max(rect.top);
        let bottom = ((a.y.max(b.y) as f32 + reach).ceil() as i32).min(rect.bottom);
        let mut changed = false;
        for y in top..bottom {
            let py = y as f32 + 0.5 - a.y as f32;
            // Bound each scanline, avoiding a full rectangular scan for long diagonals.
            let (t0, t1) = if dy.abs() > f32::EPSILON {
                let u = (py - reach) / dy;
                let v = (py + reach) / dy;
                (u.min(v).clamp(0.0, 1.0), u.max(v).clamp(0.0, 1.0))
            } else {
                (0.0, 1.0)
            };
            let x0 = a.x as f32 + dx * t0;
            let x1 = a.x as f32 + dx * t1;
            let left = ((x0.min(x1) - reach).floor() as i32).max(rect.left);
            let right = ((x0.max(x1) + reach).ceil() as i32).min(rect.right);
            for x in left..right {
                let px = x as f32 + 0.5 - a.x as f32;
                let t = if length > 0.0 {
                    ((px * dx + py * dy) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let nearness = pen.nearness((px - t * dx).hypot(py - t * dy));
                let offset = offset(rect, x, y);
                if nearness > self.nearness[offset] {
                    self.nearness[offset] = nearness;
                    pixels[offset * 4..offset * 4 + 4]
                        .copy_from_slice(&pen.colors[nearness as usize]);
                    changed = true;
                }
            }
        }
        changed
    }
}

fn offset(rect: Rect, x: i32, y: i32) -> usize {
    (y - rect.top) as usize * rect.width() as usize + (x - rect.left) as usize
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

fn segment_bounds(a: Point, b: Point, margin: i32) -> Rect {
    Rect {
        left: a.x.min(b.x) - margin,
        top: a.y.min(b.y) - margin,
        right: a.x.max(b.x) + margin + 1,
        bottom: a.y.max(b.y) + margin + 1,
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

struct Screen {
    bounds: Rect,
    work: Rect,
    dpi: u32,
}

fn screens() -> Vec<Screen> {
    let mut screens = Vec::<Screen>::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect_screen),
            LPARAM((&mut screens as *mut Vec<Screen>) as isize),
        );
    }
    screens
}

unsafe extern "system" fn collect_screen(
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
        let (mut dpi, mut dpi_y) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
        let screens = &mut *(data.0 as *mut Vec<Screen>);
        screens.push(Screen {
            bounds: convert(info.rcMonitor),
            work: convert(info.rcWork),
            dpi: dpi.max(96),
        });
    }
    true.into()
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

    /// Screen-space pixel of a surface; transparent where it has no canvas.
    fn pixel(surface: &mut Surface, x: i32, y: i32) -> [u8; 4] {
        match surface.canvas.as_mut() {
            Some(canvas) if canvas.rect.contains(Point { x, y }) => {
                let offset = offset(canvas.rect, x, y) * 4;
                canvas.dib.pixels()[offset..offset + 4].try_into().unwrap()
            }
            _ => [0; 4],
        }
    }

    /// Screen-space BGRA over `area`; transparent where the surface has no canvas.
    fn snapshot(surface: &mut Surface, area: Rect) -> Vec<u8> {
        let mut pixels = vec![0; area.width() as usize * area.height() as usize * 4];
        if let Some(canvas) = surface.canvas.as_mut() {
            if let Some(shared) = intersect(canvas.rect, area) {
                let row = shared.width() as usize * 4;
                for y in shared.top..shared.bottom {
                    let from = offset(canvas.rect, shared.left, y) * 4;
                    let to = offset(area, shared.left, y) * 4;
                    pixels[to..to + row].copy_from_slice(&canvas.dib.pixels()[from..from + row]);
                }
            }
        }
        pixels
    }

    fn assert_same_pixels(actual: &mut Surface, expected: &mut Surface) {
        let area = union(
            actual.canvas.as_ref().map(|canvas| canvas.rect),
            expected.canvas.as_ref().map(|canvas| canvas.rect),
        )
        .unwrap();
        let (actual, expected) = (snapshot(actual, area), snapshot(expected, area));
        if let Some(index) = actual.iter().zip(&expected).position(|(a, b)| a != b) {
            let pixel = (index / 4) as i32;
            let x = area.left + pixel % area.width();
            let y = area.top + pixel / area.width();
            panic!("pixel ({x}, {y}) differs");
        }
    }

    fn single_pass(
        monitor: Rect,
        dpi: u32,
        points: &[Point],
        badge: Option<(&TextMask, Rect)>,
    ) -> Surface {
        let mut surface = Surface::new(monitor, dpi);
        surface.draw(points, badge);
        surface
    }

    fn uhd(left: i32, top: i32) -> Rect {
        Rect {
            left,
            top,
            right: left + 3840,
            bottom: top + 2160,
        }
    }

    fn wave(count: i32) -> Vec<Point> {
        (0..count)
            .map(|i| Point {
                x: -600 + i % 550,
                y: -200 + ((i as f32 * 0.05).sin() * 140.0) as i32,
            })
            .collect()
    }

    /// Draws like the hint thread does, returning how often the canvas was reallocated.
    fn draw_in_batches(surface: &mut Surface, points: &[Point], batch: usize) -> usize {
        let mut last = None;
        let mut reallocations = 0;
        for chunk in points.chunks(batch) {
            let before = surface.canvas.as_ref().map(|canvas| canvas.rect);
            let path: Vec<_> = last.into_iter().chain(chunk.iter().copied()).collect();
            surface.draw(&path, None);
            last = chunk.last().copied();
            let after = surface.canvas.as_ref().map(|canvas| canvas.rect);
            reallocations += usize::from(before.is_some() && before != after);
        }
        reallocations
    }

    #[test]
    fn incremental_batches_match_a_single_pass_while_the_canvas_grows() {
        let monitor = uhd(-1920, -1080);
        let points = wave(1600);
        for dpi in [96, 144, 192] {
            let mut incremental = Surface::new(monitor, dpi);
            draw_in_batches(&mut incremental, &points, 17);
            let mut expected = single_pass(monitor, dpi, &points, None);
            assert_same_pixels(&mut incremental, &mut expected);
            // A further segment touches only its own neighbourhood.
            let before = incremental.canvas.as_ref().unwrap().nearness.clone();
            let end = *points.last().unwrap();
            let next = Point {
                x: end.x + 10,
                y: end.y + 10,
            };
            assert!(incremental.draw(&[end, next], None));
            let near = segment_bounds(end, next, incremental.pen.reach.ceil() as i32);
            let canvas = incremental.canvas.as_ref().unwrap();
            for y in canvas.rect.top..canvas.rect.bottom {
                for x in canvas.rect.left..canvas.rect.right {
                    if !near.contains(Point { x, y }) {
                        let offset = offset(canvas.rect, x, y);
                        assert_eq!(canvas.nearness[offset], before[offset]);
                    }
                }
            }
            assert!(!incremental.draw(&[next], None));
        }
    }

    #[test]
    fn canvas_follows_the_stroke_instead_of_the_monitor() {
        let monitor = Rect {
            left: 0,
            top: 0,
            right: 7680,
            bottom: 4320,
        };
        // A typical gesture on an 8K monitor at 200%.
        let points: Vec<_> = (0..=300)
            .map(|i| Point {
                x: 3000 + i,
                y: 2000 + (i * i) / 450,
            })
            .collect();
        let mut surface = Surface::new(monitor, 192);
        draw_in_batches(&mut surface, &points, 16);
        let rect = surface.canvas.as_ref().unwrap().rect;
        let bytes = rect.width() as usize * rect.height() as usize * 5;
        assert!(bytes < 6 << 20, "{rect:?} uses {bytes} bytes");

        // A spiral over a whole 4K monitor stays inside it after few reallocations.
        let monitor = uhd(-3840, 0);
        let sweep: Vec<_> = (0..2400)
            .map(|i| {
                let angle = i as f32 * 0.02;
                let radius = i as f32 * 0.5;
                Point {
                    x: -1920 + (angle.cos() * radius * 1.7) as i32,
                    y: 1080 + (angle.sin() * radius) as i32,
                }
            })
            .collect();
        let mut surface = Surface::new(monitor, 192);
        let reallocations = draw_in_batches(&mut surface, &sweep, 16);
        let canvas = surface.canvas.as_ref().unwrap().rect;
        assert_eq!(intersect(canvas, monitor), Some(canvas));
        assert!(reallocations <= 12, "{reallocations} reallocations");
        assert_same_pixels(&mut surface, &mut single_pass(monitor, 192, &sweep, None));
    }

    #[test]
    fn strokes_split_across_monitors_and_stay_within_each() {
        let left = uhd(-3840, 0);
        let right = Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        let points = [Point { x: -200, y: 300 }, Point { x: 200, y: 340 }];
        for (monitor, dpi, inside) in [(left, 192, -100), (right, 120, 100)] {
            let mut surface = single_pass(monitor, dpi, &points, None);
            let canvas = surface.canvas.as_ref().unwrap().rect;
            assert_eq!(intersect(canvas, monitor), Some(canvas));
            let y = 300 + (inside + 200) / 10;
            assert_ne!(pixel(&mut surface, inside, y)[3], 0);
        }
        let mut elsewhere = single_pass(right, 120, &[Point { x: -900, y: 10 }, points[0]], None);
        assert!(elsewhere.canvas.is_none());
        assert!(!elsewhere.draw(&[], None));
    }

    #[test]
    fn moving_badge_restores_the_stroke_beneath_it() {
        let monitor = uhd(0, 0);
        let points: Vec<_> = wave(900)
            .into_iter()
            .map(|p| Point {
                x: p.x + 800,
                y: p.y + 600,
            })
            .collect();
        for dpi in [96, 144] {
            let text = text_mask("关闭窗口", dpi, monitor.width()).unwrap();
            let mut surface = single_pass(monitor, dpi, &points, None);
            for cursor in [points[100], points[450], points[899]] {
                let badge = badge_rect(cursor, &text, monitor, dpi);
                assert!(surface.draw(&[], Some((&text, badge))));
                let mut expected = single_pass(monitor, dpi, &points, Some((&text, badge)));
                assert_same_pixels(&mut surface, &mut expected);
            }
            assert!(surface.draw(&[], None));
            assert_same_pixels(&mut surface, &mut single_pass(monitor, dpi, &points, None));
        }
    }

    #[test]
    fn stroke_is_antialiased_premultiplied_and_never_darkens_when_retraced() {
        for dpi in [96, 144, 192] {
            let pen = Pen::new(dpi);
            assert_eq!(pen.colors[0], [0; 4]);
            assert!(pen
                .colors
                .iter()
                .all(|pixel| pixel[..3].iter().all(|&c| c <= pixel[3])));
            let a = Point { x: 10, y: 10 };
            let b = Point { x: 80, y: 65 };
            let mut surface = single_pass(uhd(0, 0), dpi, &[a, b], None);
            let canvas = surface.canvas.as_ref().unwrap();
            assert!(canvas.nearness.iter().any(|&nearness| {
                let alpha = pen.colors[nearness as usize][3];
                alpha > 0 && alpha < pen.colors[255][3]
            }));
            assert!(!surface.draw(&[b, a], None));
            assert!(!surface.draw(&[a, b, a], None));
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
    fn gesture_text_and_stroke_scale_with_dpi() {
        let mut previous = (0, 0.0);
        for dpi in [96, 144, 192] {
            let text = text_mask("粘贴", dpi, 1280).unwrap();
            let reach = Pen::new(dpi).reach;
            assert!(text.width > previous.0 && reach > previous.1);
            previous = (text.width, reach);
            assert!(text.coverage.iter().any(|&c| c > 0));
            assert_eq!(text.height, dip(17.0, dpi).ceil() as i32);
            let long = text_mask(&"很长的自定义手势名称".repeat(12), dpi, 1280).unwrap();
            assert!(long.width <= dip(180.0, dpi).round() as i32);
        }
    }

    #[test]
    fn gesture_native_layers_are_click_through_and_hide_together() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DestroyWindow, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible,
            GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
        };
        let screens = screens();
        let index = screens
            .iter()
            .position(|screen| screen.bounds.contains(Point { x: 80, y: 80 }))
            .unwrap();
        let screen = &screens[index];
        let bounds = screen.bounds;
        let foreground = unsafe { GetForegroundWindow() };
        let mut hint = GestureHintManager::new();
        let mut points = vec![Point { x: 80, y: 80 }, Point { x: 80, y: 180 }];
        hint.update(1, &points, Some("粘贴"));
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        for layer in &hint.layers {
            let flags =
                unsafe { GetWindowLongPtrW(HWND(layer.window.hwnd as *mut _), GWL_EXSTYLE) } as u32;
            assert_ne!(flags & WS_EX_TRANSPARENT.0, 0);
            assert_ne!(flags & WS_EX_NOACTIVATE.0, 0);
        }
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
            let layer = &mut hint.layers[index];
            assert!(layer.presented);
            let surface = layer.surface.as_mut().unwrap();
            let canvas = surface.canvas.as_ref().unwrap().rect;
            let mut actual = RECT::default();
            unsafe { GetWindowRect(HWND(layer.window.hwnd as *mut _), &mut actual).unwrap() };
            assert_eq!(
                (actual.left, actual.top, actual.right, actual.bottom),
                (canvas.left, canvas.top, canvas.right, canvas.bottom)
            );
            let text = label.and_then(|label| text_mask(label, screen.dpi, screen.work.width()));
            let badge = text.as_ref().map(|text| {
                let cursor = *points.last().unwrap();
                (text, badge_rect(cursor, text, screen.work, screen.dpi))
            });
            let mut expected = single_pass(bounds, screen.dpi, &points, badge);
            assert_same_pixels(surface, &mut expected);
        }
        hint.hide();
        for layer in &hint.layers {
            let hwnd = HWND(layer.window.hwnd as *mut _);
            assert!(layer.surface.is_none());
            assert!(!unsafe { IsWindowVisible(hwnd).as_bool() });
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

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
        let bounds = screens()[0].bounds;
        let x = bounds.left + 160;
        let y = bounds.top + 120;
        let mut samples = Vec::new();
        for step in 0..12 {
            // The stroke keeps leaving its canvas, so presenting must survive growth.
            let points = if step == 0 {
                vec![Point { x, y }, Point { x, y: y + 40 }]
            } else {
                vec![Point {
                    x: x + step * step * 12,
                    y: y + 40 + step * 40,
                }]
            };
            hint.update(77, &points, None);
            for layer in &hint.layers {
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
            let end = *points.last().unwrap();
            unsafe {
                let dc = GetDC(HWND::default());
                let old = GetPixel(dc, x, y + 20).0;
                let new = GetPixel(dc, end.x, end.y).0;
                let _ = ReleaseDC(HWND::default(), dc);
                samples.push((old, new));
            }
        }
        hint.hide();
        for layer in &hint.layers {
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

    /// Strokes over the union of `monitors`: an L of 400 x 300 DIPs, a wavy swipe
    /// across every monitor, and a spiral reaching the edges.
    fn sample_strokes(monitors: &[Rect], dpi: u32) -> [(&'static str, Vec<Point>); 3] {
        let all = monitors
            .iter()
            .fold(None, |all, &monitor| union(all, Some(monitor)))
            .unwrap();
        let (cx, cy) = ((all.left + all.right) / 2, (all.top + all.bottom) / 2);
        let unit = |value: f32| dip(value, dpi);
        let gesture = (0..700)
            .map(|i| match i {
                0..400 => Point {
                    x: cx,
                    y: cy + unit(i as f32 * 0.75) as i32,
                },
                _ => Point {
                    x: cx + unit((i - 400) as f32 * 1.33) as i32,
                    y: cy + unit(300.0) as i32,
                },
            })
            .collect();
        let swipe = (0..4000)
            .map(|i| Point {
                x: all.left + 40 + (all.width() - 80) * i / 4000,
                y: cy + (unit(200.0) * (i as f32 * 0.01).sin()) as i32,
            })
            .collect();
        let sweep = (0..6000)
            .map(|i| {
                let turn = i as f32 / 6000.0;
                let angle = turn * 40.0;
                Point {
                    x: cx + (angle.cos() * turn * (all.width() / 2 - 20) as f32) as i32,
                    y: cy + (angle.sin() * turn * (all.height() / 2 - 20) as f32) as i32,
                }
            })
            .collect();
        [("gesture", gesture), ("swipe", swipe), ("sweep", sweep)]
    }

    #[test]
    #[ignore = "manual working-set measurement; run alone"]
    fn gesture_canvas_working_set() {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        let working_set = || {
            let mut counters = PROCESS_MEMORY_COUNTERS::default();
            let size = std::mem::size_of_val(&counters) as u32;
            unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size).unwrap() };
            counters.WorkingSetSize as f64 / f64::from(1 << 20)
        };
        let uhd8 = Rect {
            left: 0,
            top: 0,
            right: 7680,
            bottom: 4320,
        };
        for (name, monitors, dpi) in [
            ("4K 150%", vec![uhd(0, 0)], 144),
            ("8K 300%", vec![uhd8], 288),
            (
                "3x4K 150%",
                vec![uhd(-3840, 0), uhd(0, 0), uhd(3840, 0)],
                144,
            ),
        ] {
            for (stroke, points) in sample_strokes(&monitors, dpi) {
                let before = working_set();
                let mut surfaces: Vec<_> = monitors
                    .iter()
                    .map(|&monitor| Surface::new(monitor, dpi))
                    .collect();
                for surface in &mut surfaces {
                    draw_in_batches(surface, &points, 16);
                }
                let grown = working_set() - before;
                let canvases: f64 = surfaces
                    .iter()
                    .filter_map(|surface| surface.canvas.as_ref())
                    .map(|canvas| f64::from(canvas.rect.width() * canvas.rect.height()))
                    .sum();
                eprintln!(
                    "{name:10} {stroke:8} canvas {:6.2} Mpx, working set +{grown:6.1} MiB",
                    canvases / 1e6
                );
            }
        }
    }

    #[test]
    #[ignore = "manual release-mode rendering benchmark"]
    fn gesture_large_stroke_render_budget() {
        use std::time::Instant;
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
        let mut surface = Surface::new(uhd(0, 0), 192);
        let mut previous = None;
        let mut times = Vec::new();
        for batch in points.chunks(16) {
            let path: Vec<_> = previous.into_iter().chain(batch.iter().copied()).collect();
            let start = Instant::now();
            std::hint::black_box(surface.draw(&path, None));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            previous = batch.last().copied();
        }
        let mean = |slice: &[f64]| slice.iter().sum::<f64>() / slice.len() as f64;
        eprintln!(
            "4K incremental CPU, 16 new points/frame: first 100 {:.3} ms; last 100 {:.3} ms",
            mean(&times[..100]),
            mean(&times[times.len() - 100..])
        );

        let mut hint = GestureHintManager::new();
        let native = screens()[0].bounds;
        let native_points: Vec<_> = points
            .iter()
            .map(|point| Point {
                x: native.left + 30 + point.x * (native.width() - 60) / 3840,
                y: native.top + 30 + point.y * (native.height() - 60) / 2160,
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
        for layer in &hint.layers {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(HWND(
                    layer.window.hwnd as *mut _,
                ));
            }
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
            let text = text_mask("关闭窗口", dpi, bounds.width()).unwrap();
            let badge = badge_rect(*points.last().unwrap(), &text, bounds, dpi);
            let mut surface = single_pass(bounds, dpi, &points, Some((&text, badge)));
            assert!(surface.canvas.is_some());
            let Some(directory) = std::env::var_os("GESTURE_VISUAL_OUTPUT") else {
                continue;
            };
            std::fs::create_dir_all(&directory).unwrap();
            for (theme, background) in [("light", [247u8, 248, 250]), ("dark", [28u8, 31, 37])] {
                let mut rgb = Vec::with_capacity((bounds.width() * bounds.height() * 3) as usize);
                for y in bounds.top..bounds.bottom {
                    for x in bounds.left..bounds.right {
                        let pixel = pixel(&mut surface, x, y);
                        let alpha = pixel[3] as f32 / 255.0;
                        for c in 0..3 {
                            rgb.push(
                                (pixel[2 - c] as f32 + background[c] as f32 * (1.0 - alpha)).round()
                                    as u8,
                            );
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
