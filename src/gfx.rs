//! Direct2D / DirectWrite drawing for Muisland's layered windows.
//!
//! Everything is drawn in logical (96-dpi) units at the final device scale, with
//! per-primitive anti-aliasing and grayscale text, into a premultiplied 32-bit DIB
//! that `UpdateLayeredWindow` composes with real per-pixel transparency.
use std::{cell::RefCell, collections::HashMap, mem::ManuallyDrop};
use windows::{
    core::{Interface, PCWSTR},
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::{BOOL, RECT},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi as wg,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
pub const fn hex(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 255) as f32 / 255.,
        g: ((v >> 8) & 255) as f32 / 255.,
        b: (v & 255) as f32 / 255.,
        a: 1.,
    }
}
impl Color {
    pub fn alpha(self, a: f32) -> Self {
        Self {
            a: self.a * a.clamp(0., 1.),
            ..self
        }
    }
    pub fn mix(self, o: Color, t: f32) -> Self {
        let t = t.clamp(0., 1.);
        Self {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
    #[cfg(test)]
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
    fn d2d(self, alpha: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: self.r,
            g: self.g,
            b: self.b,
            a: (self.a * alpha).clamp(0., 1.),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
pub const fn r(x: f32, y: f32, w: f32, h: f32) -> R {
    R { x, y, w, h }
}
impl R {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> R {
        r(self.x + d, self.y + d, self.w - 2. * d, self.h - 2. * d)
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.
    }
    pub fn intersect(&self, o: &R) -> R {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        r(
            x,
            y,
            (self.right().min(o.right()) - x).max(0.),
            (self.bottom().min(o.bottom()) - y).max(0.),
        )
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.w,
            bottom: self.y + self.h,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}
#[derive(Clone, Copy)]
pub struct Font {
    pub size: f32,
    pub weight: u32,
    pub display: bool,
}
pub const fn font(size: f32, weight: u32) -> Font {
    Font {
        size,
        weight,
        display: size >= 18.,
    }
}

/// Process-wide factories and cached device-independent resources.
pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    round: ID2D1StrokeStyle,
    text_family: Vec<u16>,
    display_family: Vec<u16>,
    formats: RefCell<HashMap<(u32, u32, bool, bool), (IDWriteTextFormat, IDWriteInlineObject)>>,
    /// Shaped text is reused across frames; shaping is the costliest part of text.
    layouts: RefCell<HashMap<LayoutKey, IDWriteTextLayout>>,
    gradients: RefCell<HashMap<(u32, u32), ID2D1GradientStopCollection>>,
    target: ID2D1DCRenderTarget,
}
#[derive(Hash, PartialEq, Eq)]
struct LayoutKey {
    text: String,
    size: u32,
    weight: u32,
    display: bool,
    w: u32,
    h: u32,
    align: u8,
    wrap: bool,
    rtl: bool,
}
fn rgba(c: Color) -> u32 {
    let q = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    q(c.r) << 24 | q(c.g) << 16 | q(c.b) << 8 | q(c.a)
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
impl Gfx {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let round = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.,
                },
                None,
            )?;
            let mut collection = None;
            dw.GetSystemFontCollection(&mut collection, false)?;
            let exists = |name: &str| {
                collection.as_ref().is_some_and(|c| {
                    let mut index = 0;
                    let mut found = BOOL(0);
                    let n = wide(name);
                    c.FindFamilyName(PCWSTR(n.as_ptr()), &mut index, &mut found)
                        .is_ok()
                        && found.as_bool()
                })
            };
            let pick = |a: &str| if exists(a) { wide(a) } else { wide("Segoe UI") };
            let target = d2d.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                // Layered windows read pixels back every frame; the software
                // rasteriser avoids a GPU round trip and is faster here.
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            })?;
            Ok(Self {
                text_family: pick("Segoe UI Variable Text"),
                display_family: pick("Segoe UI Variable Display"),
                d2d,
                dw,
                round,
                formats: RefCell::new(HashMap::new()),
                layouts: RefCell::new(HashMap::new()),
                gradients: RefCell::new(HashMap::new()),
                target,
            })
        }
    }
    fn format(&self, f: Font, rtl: bool) -> Option<(IDWriteTextFormat, IDWriteInlineObject)> {
        let key = ((f.size * 10.) as u32, f.weight, f.display, rtl);
        if let Some(v) = self.formats.borrow().get(&key) {
            return Some(v.clone());
        }
        unsafe {
            let family = if f.display {
                &self.display_family
            } else {
                &self.text_family
            };
            let format = self
                .dw
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(f.weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    f.size,
                    PCWSTR(wide(if rtl { "ar-eg" } else { "en-us" }).as_ptr()),
                )
                .ok()?;
            let sign = self.dw.CreateEllipsisTrimmingSign(&format).ok()?;
            self.formats
                .borrow_mut()
                .insert(key, (format.clone(), sign.clone()));
            Some((format, sign))
        }
    }
    fn layout(
        &self,
        text: &str,
        f: Font,
        w: f32,
        h: f32,
        align: Align,
        wrap: bool,
        rtl: bool,
    ) -> Option<IDWriteTextLayout> {
        let key = LayoutKey {
            text: text.to_string(),
            size: (f.size * 20.).round() as u32,
            weight: f.weight,
            display: f.display,
            w: (w.max(1.) * 4.).round() as u32,
            h: (h.max(1.) * 4.).round() as u32,
            align: align as u8,
            wrap,
            rtl,
        };
        if let Some(layout) = self.layouts.borrow().get(&key) {
            return Some(layout.clone());
        }
        let layout = self.create_layout(text, f, w, h, align, wrap, rtl)?;
        let mut cache = self.layouts.borrow_mut();
        if cache.len() > 768 {
            cache.clear();
        }
        cache.insert(key, layout.clone());
        Some(layout)
    }
    #[allow(clippy::too_many_arguments)]
    fn create_layout(
        &self,
        text: &str,
        f: Font,
        w: f32,
        h: f32,
        align: Align,
        wrap: bool,
        rtl: bool,
    ) -> Option<IDWriteTextLayout> {
        let (format, sign) = self.format(f, rtl)?;
        let chars: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self
                .dw
                .CreateTextLayout(&chars, &format, w.max(1.), h.max(1.))
                .ok()?;
            let _ = layout.SetReadingDirection(if rtl {
                DWRITE_READING_DIRECTION_RIGHT_TO_LEFT
            } else {
                DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
            });
            let _ = layout.SetTextAlignment(match align {
                Align::Start => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::End => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = layout.SetParagraphAlignment(if wrap {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            });
            if !wrap {
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = layout.SetTrimming(
                    &DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    },
                    &sign,
                );
            }
            Some(layout)
        }
    }
    /// Natural width and height of a single line or wrapped paragraph.
    pub fn measure(&self, text: &str, f: Font, max_w: f32, wrap: bool) -> (f32, f32) {
        let Some(layout) = self.layout(text, f, max_w, 10000., Align::Start, wrap, false) else {
            return (text.chars().count() as f32 * f.size * 0.55, f.size * 1.35);
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            if layout.GetMetrics(&mut m).is_err() {
                return (0., 0.);
            }
        }
        (m.widthIncludingTrailingWhitespace, m.height)
    }
    /// Horizontal caret offset of a UTF-16 position within a single line.
    pub fn caret_x(&self, text: &str, f: Font, position: usize, rtl: bool) -> f32 {
        let Some(layout) = self.layout(text, f, 10000., 100., Align::Start, false, rtl) else {
            return 0.;
        };
        let (mut x, mut y) = (0., 0.);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = layout.HitTestTextPosition(position as u32, false, &mut x, &mut y, &mut m);
        }
        if rtl {
            let (w, _) = self.measure(text, f, 10000., false);
            10000. - x - (10000. - w)
        } else {
            x
        }
    }
}

/// A premultiplied top-down 32-bit DIB selected into a memory DC.
pub struct Surface {
    pub dc: wg::HDC,
    bmp: wg::HBITMAP,
    old: wg::HGDIOBJ,
    pub bits: *mut u8,
    /// Size in use (drawn and presented).
    pub w: i32,
    pub h: i32,
    /// Allocated size; a surface is reused while the used size fits.
    cap_w: i32,
    cap_h: i32,
}
impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Self> {
        unsafe {
            let w = w.max(1);
            let h = h.max(1);
            let dc = wg::CreateCompatibleDC(None);
            let mut info = wg::BITMAPINFO::default();
            info.bmiHeader.biSize = std::mem::size_of::<wg::BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = w;
            info.bmiHeader.biHeight = -h;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = wg::BI_RGB.0;
            let mut bits = std::ptr::null_mut();
            let bmp = wg::CreateDIBSection(dc, &info, wg::DIB_RGB_COLORS, &mut bits, None, 0)
                .ok()
                .filter(|b| !b.is_invalid())?;
            if bits.is_null() {
                let _ = wg::DeleteObject(bmp);
                let _ = wg::DeleteDC(dc);
                return None;
            }
            let old = wg::SelectObject(dc, bmp);
            Some(Self {
                dc,
                bmp,
                old,
                bits: bits.cast(),
                w,
                h,
                cap_w: w,
                cap_h: h,
            })
        }
    }
    /// Reuses `slot` when it can hold `w`×`h` without being far too large,
    /// otherwise allocates with headroom so a growing morph rarely reallocates.
    pub fn reuse(slot: &mut Option<Surface>, w: i32, h: i32) -> Option<&Surface> {
        let (w, h) = (w.max(1), h.max(1));
        let fits = slot.as_ref().is_some_and(|s| {
            s.cap_w >= w && s.cap_h >= h && s.cap_w * s.cap_h <= (w + 160) * (h + 160) * 2
        });
        if !fits {
            *slot = None;
            let mut fresh = Surface::new(w + 48, h + 48)?;
            fresh.w = w;
            fresh.h = h;
            *slot = Some(fresh);
        }
        let s = slot.as_mut()?;
        s.w = w;
        s.h = h;
        Some(s)
    }
    fn stride(&self) -> i32 {
        self.cap_w * 4
    }
    /// Pixel rows of the used area, tightly packed.
    pub fn pixels(&self) -> Vec<u8> {
        let all =
            unsafe { std::slice::from_raw_parts(self.bits, (self.stride() * self.cap_h) as usize) };
        let mut out = Vec::with_capacity((self.w * self.h * 4) as usize);
        for y in 0..self.h {
            let start = (y * self.stride()) as usize;
            out.extend_from_slice(&all[start..start + (self.w * 4) as usize]);
        }
        out
    }
    #[cfg(test)]
    pub fn alpha_at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        unsafe { *self.bits.add((y * self.stride() + x * 4 + 3) as usize) }
    }
    /// Copies another surface's used area to (x, y), clipped to this surface.
    pub fn blit(&self, src: &Surface, x: i32, y: i32) {
        let px = src.pixels();
        for row in 0..src.h {
            let ty = row + y;
            if ty < 0 || ty >= self.h {
                continue;
            }
            let x0 = x.max(0);
            let x1 = (x + src.w).min(self.w);
            if x1 <= x0 {
                continue;
            }
            let from = ((row * src.w + (x0 - x)) * 4) as usize;
            let len = ((x1 - x0) * 4) as usize;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    px.as_ptr().add(from),
                    self.bits.add((ty * self.stride() + x0 * 4) as usize),
                    len,
                );
            }
        }
    }
    pub fn hdc_raw(&self) -> *mut core::ffi::c_void {
        self.dc.0
    }
    pub fn hbitmap_raw(&self) -> *mut core::ffi::c_void {
        self.bmp.0
    }
    /// Lossless PNG (stored deflate blocks) of the premultiplied pixels on a backdrop.
    pub fn png(&self, backdrop: Option<Color>) -> Vec<u8> {
        let mut raw = Vec::with_capacity((self.w * self.h * 4 + self.h) as usize);
        let px = self.pixels();
        for y in 0..self.h {
            raw.push(0);
            for x in 0..self.w {
                let i = ((y * self.w + x) * 4) as usize;
                let (b, g, rr, a) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32, px[i + 3]);
                match backdrop {
                    Some(c) => {
                        let k = 1. - a as f32 / 255.;
                        raw.extend([
                            (rr + c.r * 255. * k).min(255.) as u8,
                            (g + c.g * 255. * k).min(255.) as u8,
                            (b + c.b * 255. * k).min(255.) as u8,
                            255,
                        ]);
                    }
                    None => {
                        let un = |v: f32| {
                            if a == 0 {
                                0
                            } else {
                                (v * 255. / a as f32).round().min(255.) as u8
                            }
                        };
                        raw.extend([un(rr), un(g), un(b), a]);
                    }
                }
            }
        }
        png_encode(self.w as u32, self.h as u32, &raw)
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            wg::SelectObject(self.dc, self.old);
            let _ = wg::DeleteObject(self.bmp);
            let _ = wg::DeleteDC(self.dc);
        }
    }
}
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}
fn png_encode(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// One frame of drawing. Coordinates are logical units, offset by `ox/oy`,
/// mirrored horizontally inside `mirror` when `rtl` is set, and multiplied by
/// `alpha`. Interactive regions are recorded in the same coordinate space.
pub struct Canvas<'a, A: Copy> {
    pub g: &'a Gfx,
    rt: Option<ID2D1RenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    pub alpha: f32,
    pub ox: f32,
    pub oy: f32,
    pub rtl: bool,
    pub mirror: f32,
    pub hits: Vec<(R, A)>,
    pub hit_clip: Option<R>,
    pub interactive: bool,
    zoom: (f32, f32, f32),
    base: (f32, f32, f32),
}
impl<'a, A: Copy> Canvas<'a, A> {
    /// Bind a surface. `k` is device pixels per logical unit, `(tx, ty)` a device offset.
    pub fn begin(g: &'a Gfx, surface: &Surface, k: f32, tx: f32, ty: f32) -> Option<Self> {
        unsafe {
            let rect = RECT {
                left: 0,
                top: 0,
                right: surface.w,
                bottom: surface.h,
            };
            // BindDC maps the sub-rectangle; the DIB stride is the allocation.
            g.target.BindDC(surface.dc, &rect).ok()?;
            let rt: ID2D1RenderTarget = g.target.cast().ok()?;
            rt.BeginDraw();
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            rt.Clear(Some(&D2D1_COLOR_F::default()));
            let brush = rt
                .CreateSolidColorBrush(&D2D1_COLOR_F::default(), None)
                .ok()?;
            let c = Self {
                g,
                rt: Some(rt),
                brush: Some(brush),
                alpha: 1.,
                ox: 0.,
                oy: 0.,
                rtl: false,
                mirror: 0.,
                hits: Vec::new(),
                hit_clip: None,
                interactive: true,
                zoom: (1., 0., 0.),
                base: (k, tx, ty),
            };
            c.apply_transform();
            Some(c)
        }
    }
    pub fn end(mut self) -> Vec<(R, A)> {
        if let Some(rt) = self.rt.take() {
            unsafe {
                let _ = rt.EndDraw(None, None);
            }
        }
        std::mem::take(&mut self.hits)
    }
    fn apply_transform(&self) {
        let (k, tx, ty) = self.base;
        let (z, cx, cy) = self.zoom;
        // Zoom about (cx, cy) in logical space, then map to device pixels.
        let m = Matrix3x2 {
            M11: k * z,
            M12: 0.,
            M21: 0.,
            M22: k * z,
            M31: tx + k * (cx - cx * z),
            M32: ty + k * (cy - cy * z),
        };
        if let Some(rt) = &self.rt {
            unsafe { rt.SetTransform(&m) };
        }
    }
    /// Visual-only zoom about a logical point (not applied to hit regions).
    pub fn set_zoom(&mut self, z: f32, cx: f32, cy: f32) {
        self.zoom = (z, cx, cy);
        self.apply_transform();
    }
    pub fn map(&self, b: R) -> R {
        let x = if self.rtl {
            self.mirror - b.x - b.w
        } else {
            b.x
        };
        r(x + self.ox, b.y + self.oy, b.w, b.h)
    }
    fn map_x(&self, x: f32) -> f32 {
        (if self.rtl { self.mirror - x } else { x }) + self.ox
    }
    fn solid(&self, c: Color) -> Option<(&ID2D1RenderTarget, &ID2D1SolidColorBrush)> {
        let rt = self.rt.as_ref()?;
        let brush = self.brush.as_ref()?;
        let col = c.d2d(self.alpha);
        if col.a <= 0.001 {
            return None;
        }
        unsafe { brush.SetColor(&col) };
        Some((rt, brush))
    }
    pub fn hit(&mut self, b: R, a: A) {
        if !self.interactive || self.alpha < 0.5 {
            return;
        }
        let mut m = self.map(b);
        if let Some(clip) = self.hit_clip {
            m = m.intersect(&clip);
        }
        if m.w > 0. && m.h > 0. {
            self.hits.push((m, a));
        }
    }
    pub fn push_clip(&self, b: R) {
        if let Some(rt) = &self.rt {
            let m = r(b.x, b.y, b.w, b.h);
            unsafe { rt.PushAxisAlignedClip(&m.d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        }
    }
    pub fn pop_clip(&self) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PopAxisAlignedClip() };
        }
    }
    /// Clip subsequent drawing to an anti-aliased rounded rectangle.
    pub fn push_round_clip(&self, b: R, radius: f32) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            let Ok(geometry) = self
                .g
                .d2d
                .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                    rect: b.d2d(),
                    radiusX: radius,
                    radiusY: radius,
                })
            else {
                return;
            };
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: D2D_RECT_F {
                    left: -1e6,
                    top: -1e6,
                    right: 1e6,
                    bottom: 1e6,
                },
                geometricMask: ManuallyDrop::new(geometry.cast().ok()),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: 1.,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            rt.PushLayer(&params, None);
            drop(ManuallyDrop::into_inner(params.geometricMask));
        }
    }
    pub fn pop_layer(&self) {
        if let Some(rt) = &self.rt {
            unsafe { rt.PopLayer() };
        }
    }

    pub fn fill(&self, b: R, radius: f32, c: Color) {
        let m = self.map(b);
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: m.d2d(),
                        radiusX: radius.min(m.w / 2.).min(m.h / 2.).max(0.),
                        radiusY: radius.min(m.w / 2.).min(m.h / 2.).max(0.),
                    },
                    brush,
                )
            };
        }
    }
    pub fn stroke(&self, b: R, radius: f32, width: f32, c: Color) {
        let m = self.map(b);
        let m = m.inset(width / 2.);
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.DrawRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: m.d2d(),
                        radiusX: (radius - width / 2.).max(0.),
                        radiusY: (radius - width / 2.).max(0.),
                    },
                    brush,
                    width,
                    None,
                )
            };
        }
    }
    fn gradient(
        &self,
        stops: &[(f32, Color)],
        from: (f32, f32),
        to: (f32, f32),
    ) -> Option<ID2D1LinearGradientBrush> {
        let rt = self.rt.as_ref()?;
        if self.alpha <= 0.001 {
            return None;
        }
        let key = (rgba(stops[0].1), rgba(stops[stops.len() - 1].1));
        let cached = self.g.gradients.borrow().get(&key).cloned();
        let collection = match cached {
            Some(c) => c,
            None => {
                let list: Vec<D2D1_GRADIENT_STOP> = stops
                    .iter()
                    .map(|(p, c)| D2D1_GRADIENT_STOP {
                        position: *p,
                        color: c.d2d(1.),
                    })
                    .collect();
                let c = unsafe {
                    rt.CreateGradientStopCollection(&list, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                        .ok()?
                };
                let mut cache = self.g.gradients.borrow_mut();
                if cache.len() > 256 {
                    cache.clear();
                }
                cache.insert(key, c.clone());
                c
            }
        };
        unsafe {
            let props = D2D1_BRUSH_PROPERTIES {
                opacity: self.alpha.clamp(0., 1.),
                transform: Matrix3x2::identity(),
            };
            rt.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: D2D_POINT_2F {
                        x: from.0,
                        y: from.1,
                    },
                    endPoint: D2D_POINT_2F { x: to.0, y: to.1 },
                },
                Some(&props),
                &collection,
            )
            .ok()
        }
    }
    /// Rounded rectangle filled with a gradient from `a` (start edge) to `b`.
    pub fn fill_gradient(&self, b: R, radius: f32, a: Color, c: Color, vertical: bool) {
        let m = self.map(b);
        let (from, to) = if vertical {
            ((m.x, m.y), (m.x, m.bottom()))
        } else if self.rtl {
            ((m.right(), m.y), (m.x, m.y))
        } else {
            ((m.x, m.y), (m.right(), m.y))
        };
        if let (Some(rt), Some(brush)) = (&self.rt, self.gradient(&[(0., a), (1., c)], from, to)) {
            unsafe {
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: m.d2d(),
                        radiusX: radius.min(m.w / 2.).min(m.h / 2.),
                        radiusY: radius.min(m.w / 2.).min(m.h / 2.),
                    },
                    &brush,
                )
            };
        }
    }
    /// Soft drop shadow drawn as concentric bands, so cost scales with the
    /// perimeter rather than the area. Reaches `spread * 1.25` past the shape.
    pub fn shadow(&self, b: R, radius: f32, spread: f32, strength: f32, c: Color) {
        let Some(rt) = &self.rt else { return };
        let Some(brush) = &self.brush else { return };
        let steps = 12;
        let dy = spread / 4.;
        let peak = 0.36 * strength;
        let band = spread / steps as f32;
        let base = self.map(r(b.x, b.y + dy, b.w, b.h));
        // Under the shape: the innermost shadow tone fills the offset area.
        let inner = c.alpha(peak);
        unsafe {
            brush.SetColor(&inner.d2d(self.alpha));
            rt.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: base.d2d(),
                    radiusX: radius.min(base.w / 2.).min(base.h / 2.),
                    radiusY: radius.min(base.w / 2.).min(base.h / 2.),
                },
                brush,
            );
            for i in 0..steps {
                let t = (i as f32 + 0.5) / steps as f32;
                // Smooth falloff resembling a Gaussian blur's edge profile.
                let a = peak * (1. - t).powf(2.2);
                let e = band * (i as f32 + 0.5);
                let rr = r(base.x - e, base.y - e, base.w + 2. * e, base.h + 2. * e);
                brush.SetColor(&c.alpha(a).d2d(self.alpha));
                rt.DrawRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rr.d2d(),
                        radiusX: radius + e,
                        radiusY: radius + e,
                    },
                    brush,
                    band + 0.35,
                    None,
                );
            }
        }
    }
    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let x = self.map_x(cx);
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x, y: cy + self.oy },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    brush,
                )
            };
        }
    }
    pub fn circle_gradient(&self, cx: f32, cy: f32, radius: f32, a: Color, b: Color) {
        let x = self.map_x(cx);
        let y = cy + self.oy;
        if let (Some(rt), Some(brush)) = (
            &self.rt,
            self.gradient(
                &[(0., a), (1., b)],
                (x - radius, y - radius),
                (x + radius, y + radius),
            ),
        ) {
            unsafe {
                rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x, y },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &brush,
                )
            };
        }
    }
    pub fn ring(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        let x = self.map_x(cx);
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x, y: cy + self.oy },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    brush,
                    width,
                    None,
                )
            };
        }
    }
    fn arc_geometry(
        &self,
        x: f32,
        y: f32,
        radius: f32,
        start: f32,
        sweep: f32,
    ) -> Option<ID2D1PathGeometry> {
        let point = |a: f32| D2D_POINT_2F {
            x: x + radius * a.sin(),
            y: y - radius * a.cos(),
        };
        unsafe {
            let geometry = self.g.d2d.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            sink.BeginFigure(point(start), D2D1_FIGURE_BEGIN_HOLLOW);
            // Split into halves so a full sweep remains well defined.
            let parts = if sweep.abs() > std::f32::consts::PI {
                2
            } else {
                1
            };
            for i in 1..=parts {
                let a = start + sweep * i as f32 / parts as f32;
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: point(a),
                    size: D2D_SIZE_F {
                        width: radius,
                        height: radius,
                    },
                    rotationAngle: 0.,
                    sweepDirection: if sweep >= 0. {
                        D2D1_SWEEP_DIRECTION_CLOCKWISE
                    } else {
                        D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                    },
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close().ok()?;
            Some(geometry)
        }
    }
    /// Arc from `start` (radians, 0 = 12 o'clock, clockwise) sweeping `sweep`, with round caps.
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, width: f32, start: f32, sweep: f32, c: Color) {
        if sweep.abs() < 0.001 {
            return;
        }
        let x = self.map_x(cx);
        let Some(geometry) = self.arc_geometry(x, cy + self.oy, radius, start, sweep) else {
            return;
        };
        if let Some((rt, brush)) = self.solid(c) {
            unsafe { rt.DrawGeometry(&geometry, brush, width, &self.g.round) };
        }
    }
    /// Arc stroked with a diagonal two-colour gradient.
    pub fn arc_gradient(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        width: f32,
        start: f32,
        sweep: f32,
        a: Color,
        b: Color,
    ) {
        if sweep.abs() < 0.001 {
            return;
        }
        let x = self.map_x(cx);
        let y = cy + self.oy;
        let Some(geometry) = self.arc_geometry(x, y, radius, start, sweep) else {
            return;
        };
        if let (Some(rt), Some(brush)) = (
            &self.rt,
            self.gradient(
                &[(0., a), (1., b)],
                (x - radius, y - radius),
                (x + radius, y + radius),
            ),
        ) {
            unsafe { rt.DrawGeometry(&geometry, &brush, width, &self.g.round) };
        }
    }
    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let (a, b) = (self.map_x(x0), self.map_x(x1));
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.DrawLine(
                    D2D_POINT_2F {
                        x: a,
                        y: y0 + self.oy,
                    },
                    D2D_POINT_2F {
                        x: b,
                        y: y1 + self.oy,
                    },
                    brush,
                    width,
                    &self.g.round,
                )
            };
        }
    }
    pub fn polyline(&self, pts: &[(f32, f32)], width: f32, c: Color, closed: bool) {
        if pts.len() < 2 {
            return;
        }
        let Some((rt, brush)) = self.solid(c) else {
            return;
        };
        unsafe {
            let Ok(geometry) = self.g.d2d.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = geometry.Open() else { return };
            let p = |(x, y): (f32, f32)| D2D_POINT_2F {
                x: self.map_x(x),
                y: y + self.oy,
            };
            sink.BeginFigure(p(pts[0]), D2D1_FIGURE_BEGIN_HOLLOW);
            for point in &pts[1..] {
                sink.AddLine(p(*point));
            }
            sink.EndFigure(if closed {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            if sink.Close().is_ok() {
                rt.DrawGeometry(&geometry, brush, width, &self.g.round);
            }
        }
    }
    /// Draws text inside `b`. Single-line text is vertically centred and trimmed.
    pub fn text(&self, s: &str, b: R, f: Font, c: Color, align: Align) {
        self.text_ex(s, b, f, c, align, false);
    }
    pub fn paragraph(&self, s: &str, b: R, f: Font, c: Color) -> f32 {
        self.text_ex(s, b, f, c, Align::Start, true)
    }
    fn text_ex(&self, s: &str, b: R, f: Font, c: Color, align: Align, wrap: bool) -> f32 {
        if s.is_empty() {
            return 0.;
        }
        let m = self.map(b);
        let Some(layout) = self.g.layout(s, f, m.w, m.h, align, wrap, self.rtl) else {
            return 0.;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut metrics);
        }
        if let Some((rt, brush)) = self.solid(c) {
            unsafe {
                rt.DrawTextLayout(
                    D2D_POINT_2F { x: m.x, y: m.y },
                    &layout,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                )
            };
        }
        metrics.height
    }
    pub fn measure(&self, s: &str, f: Font) -> f32 {
        self.g.measure(s, f, 10000., false).0
    }
}
