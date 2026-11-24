//! Rasteriser for generated layers and captions.
//!
//! ffmpeg builds without libfreetype/libass cannot draw text, so everything
//! that is not media (titles, shapes, captions, progress bars, waveform
//! graphics) is drawn here into RGBA images. The live preview and the export
//! use the same code, so what you see is what is exported.
//!
//! Conventions shared with `mango_core::render`:
//! * `Transform.x/y` is the centre of the layer box and `w/h` its size, as
//!   fractions of the canvas; rotation is clockwise in degrees about the
//!   centre.
//! * Sizes given in pixels in the model (outline and stroke widths) are in
//!   *composition* pixels and are scaled by `canvas height / comp.height`.
//! * `Shape::corner_radius` is a fraction of the box's shorter side
//!   (`0..=0.5`), like `VideoEffect::CornerRadius`.

use crate::frames::{Frame, encode_png};
use crate::waveform::Peaks;
use anyhow::{Context, Result};
use cosmic_text as ct;
use mango_core::captions::{Cue, CueOptions};
use mango_core::render::{ANIM_SECS, ExportSettings, OverlayInput, ffconcat};
use mango_core::{
    Animation, CaptionHighlight, Composition, Layer, LayerContent, MediaKind, Project, Rgba, Shape, ShapeKind, TextAlign, TextStyle, Transform,
    WaveformKind, WaveformStyle,
};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia as ts;

/// Seconds of audio shown across a waveform layer.
pub const WAVEFORM_WINDOW_SECS: f64 = 4.0;
/// Bars across a waveform layer.
const WAVEFORM_BARS: usize = 64;
/// Resolution of [`composition_peaks`].
pub const COMPOSITION_PEAKS_PER_SECOND: u32 = 50;
/// Captions wrap at this fraction of the canvas width.
const CAPTION_WIDTH: f64 = 0.9;
const LINE_HEIGHT: f32 = 1.2;
const MAX_TEXT_DIM: u32 = 16_384;

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

struct Fonts {
    system: ct::FontSystem,
    cache: ct::SwashCache,
    /// Lower-cased family name -> the name as the font database spells it.
    families: HashMap<String, String>,
    rendered: HashMap<u64, Arc<TextImage>>,
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let system = ct::FontSystem::new();
        let mut families = HashMap::new();
        for face in system.db().faces() {
            for (name, _) in &face.families {
                families.entry(name.to_lowercase()).or_insert_with(|| name.clone());
            }
        }
        Mutex::new(Fonts { system, cache: ct::SwashCache::new(), families, rendered: HashMap::new() })
    })
}

fn lock_fonts() -> std::sync::MutexGuard<'static, Fonts> {
    // A panic while rasterising must not take text rendering down for good.
    fonts().lock().unwrap_or_else(|e| e.into_inner())
}

/// Families tried, in order, when the requested one is not installed.
const FALLBACK_FAMILIES: [&str; 4] = ["Helvetica Neue", "Helvetica", "Arial", "Arial Unicode MS"];

impl Fonts {
    fn resolve(&self, family: &str) -> Option<String> {
        let wanted = family.trim().to_lowercase();
        if let Some(n) = self.families.get(&wanted) {
            return Some(n.clone());
        }
        FALLBACK_FAMILIES.iter().find_map(|f| self.families.get(&f.to_lowercase()).cloned())
    }
}

/// The installed family that text set in `family` is drawn with: the family
/// itself when installed (matched case-insensitively), otherwise the
/// fallback. `None` means the generic sans-serif face is used.
pub fn resolve_font_family(family: &str) -> Option<String> {
    lock_fonts().resolve(family)
}

/// Whether a font family with this name is installed.
pub fn font_family_installed(family: &str) -> bool {
    lock_fonts().families.contains_key(&family.trim().to_lowercase())
}

/// All installed font family names, sorted, for font pickers.
pub fn font_families() -> Vec<String> {
    let mut v: Vec<String> = lock_fonts().families.values().cloned().collect();
    v.sort_by_key(|a| a.to_lowercase());
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct Span {
    text: String,
    color: Rgba,
    /// Invisible spans take up room but draw nothing (caption reveal).
    visible: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct TextParams {
    spans: Vec<Span>,
    font: String,
    /// Font size in pixels.
    px: f32,
    bold: bool,
    italic: bool,
    align: TextAlign,
    /// Width the text wraps to, in pixels.
    wrap_width: f32,
    background: Option<Rgba>,
    /// Outline colour and radius in pixels.
    outline: Option<(Rgba, f32)>,
    shadow: bool,
}

impl TextParams {
    fn key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for s in &self.spans {
            s.text.hash(&mut h);
            (s.color.r, s.color.g, s.color.b, s.color.a, s.visible).hash(&mut h);
        }
        self.font.hash(&mut h);
        self.px.to_bits().hash(&mut h);
        (self.bold, self.italic, self.align as u8).hash(&mut h);
        self.wrap_width.to_bits().hash(&mut h);
        self.background.map(|c| (c.r, c.g, c.b, c.a)).hash(&mut h);
        self.outline.map(|(c, w)| (c.r, c.g, c.b, c.a, w.to_bits())).hash(&mut h);
        self.shadow.hash(&mut h);
        h.finish()
    }
}

/// Rasterised text: a premultiplied pixmap plus where the layout box sits in it.
struct TextImage {
    pix: ts::Pixmap,
    /// Padding around the layout box on every side.
    pad: i32,
    /// Size of the layout box (wrap width x height of all lines).
    box_w: f32,
    box_h: f32,
}

fn over(dst: &mut [u8], src: [f32; 4]) {
    // src and dst premultiplied, 0..=255 floats / bytes.
    let inv = 1.0 - src[3] / 255.0;
    for c in 0..4 {
        dst[c] = (src[c] + dst[c] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    }
}

/// Grows an alpha mask by `radius` pixels with a soft (anti-aliased) edge.
fn dilate(alpha: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    let r = radius.clamp(0.0, 64.0);
    let reach = (r + 0.5).ceil() as i32;
    let mut kernel: Vec<(i32, i32, u16)> = Vec::new();
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            let wgt = (r + 0.5 - d).clamp(0.0, 1.0);
            if wgt > 0.0 {
                kernel.push((dx, dy, (wgt * 256.0).round() as u16));
            }
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let a = alpha[y * w + x] as u16;
            if a == 0 {
                continue;
            }
            for &(dx, dy, wgt) in &kernel {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let v = ((a * wgt) >> 8).min(255) as u8;
                let o = &mut out[ny as usize * w + nx as usize];
                if v > *o {
                    *o = v;
                }
            }
        }
    }
    out
}

/// Three box-blur passes (a close approximation of a Gaussian).
fn blur(alpha: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 || w == 0 || h == 0 {
        return alpha.to_vec();
    }
    let mut a: Vec<f32> = alpha.iter().map(|v| *v as f32).collect();
    let mut b = vec![0f32; w * h];
    let win = (2 * radius + 1) as f32;
    for _ in 0..3 {
        for y in 0..h {
            let row = &a[y * w..(y + 1) * w];
            let mut sum = 0.0;
            for i in 0..=radius.min(w - 1) {
                sum += row[i];
            }
            for x in 0..w {
                b[y * w + x] = sum / win;
                if x + radius + 1 < w {
                    sum += row[x + radius + 1];
                }
                if x >= radius {
                    sum -= row[x - radius];
                }
            }
        }
        for x in 0..w {
            let mut sum = 0.0;
            for i in 0..=radius.min(h - 1) {
                sum += b[i * w + x];
            }
            for y in 0..h {
                a[y * w + x] = sum / win;
                if y + radius + 1 < h {
                    sum += b[(y + radius + 1) * w + x];
                }
                if y >= radius {
                    sum -= b[(y - radius) * w + x];
                }
            }
        }
    }
    a.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect()
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<ts::Path> {
    if !(w > 0.0 && h > 0.0) || !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
        return None;
    }
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    if r < 0.01 {
        return Some(ts::PathBuilder::from_rect(ts::Rect::from_xywh(x, y, w, h)?));
    }
    let k = r * 0.552_284_8;
    let (x1, y1) = (x + w, y + h);
    let mut pb = ts::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn paint(c: Rgba) -> ts::Paint<'static> {
    let mut p = ts::Paint::default();
    p.set_color_rgba8(c.r, c.g, c.b, c.a);
    p.anti_alias = true;
    p
}

fn render_text(fonts: &mut Fonts, p: &TextParams) -> Option<Arc<TextImage>> {
    let key = p.key();
    if let Some(img) = fonts.rendered.get(&key) {
        return Some(img.clone());
    }
    let img = Arc::new(render_text_uncached(fonts, p)?);
    if fonts.rendered.len() >= 48 {
        fonts.rendered.clear();
    }
    if fonts.cache.image_cache.len() > 8192 {
        fonts.cache.image_cache.clear();
    }
    fonts.rendered.insert(key, img.clone());
    Some(img)
}

fn render_text_uncached(fonts: &mut Fonts, p: &TextParams) -> Option<TextImage> {
    let px = p.px;
    if !(px.is_finite() && px >= 1.0) || !(p.wrap_width.is_finite() && p.wrap_width >= 1.0) {
        return None;
    }
    if p.spans.iter().all(|s| s.text.trim().is_empty()) {
        return None;
    }
    let px = px.min(4096.0);
    let wrap_width = p.wrap_width.min(MAX_TEXT_DIM as f32);
    let family_name = fonts.resolve(&p.font);
    let family = match &family_name {
        Some(n) => ct::Family::Name(n),
        None => ct::Family::SansSerif,
    };
    let base = ct::Attrs::new()
        .family(family)
        .weight(if p.bold { ct::Weight::BOLD } else { ct::Weight::NORMAL })
        .style(if p.italic { ct::Style::Italic } else { ct::Style::Normal });
    let align = match p.align {
        TextAlign::Left => ct::Align::Left,
        TextAlign::Center => ct::Align::Center,
        TextAlign::Right => ct::Align::Right,
    };
    let mut buffer = ct::Buffer::new(&mut fonts.system, ct::Metrics::new(px, px * LINE_HEIGHT));
    buffer.set_wrap(ct::Wrap::WordOrGlyph);
    buffer.set_size(Some(wrap_width), None);
    let spans: Vec<(&str, ct::Attrs)> = p.spans.iter().enumerate().map(|(i, s)| (s.text.as_str(), base.clone().metadata(i))).collect();
    buffer.set_rich_text(spans, &base, ct::Shaping::Advanced, Some(align));
    buffer.shape_until_scroll(&mut fonts.system, false);

    // Layout extents.
    struct Placed {
        key: ct::CacheKey,
        x: i32,
        y: i32,
        span: usize,
    }
    let mut placed: Vec<Placed> = Vec::new();
    let (mut ink_l, mut ink_r) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut block_h = 0f32;
    for run in buffer.layout_runs() {
        block_h = block_h.max(run.line_top + run.line_height);
        for g in run.glyphs {
            ink_l = ink_l.min(g.x);
            ink_r = ink_r.max(g.x + g.w);
            let ph = g.physical((0.0, run.line_y), 1.0);
            placed.push(Placed { key: ph.cache_key, x: ph.x, y: ph.y, span: g.metadata });
        }
    }
    if placed.is_empty() || block_h <= 0.0 {
        return None;
    }
    let outline_r = p.outline.map(|(_, w)| w.clamp(0.0, 64.0)).unwrap_or(0.0);
    let shadow_off = if p.shadow { (px * 0.05).max(1.0) } else { 0.0 };
    let shadow_blur = if p.shadow { ((px * 0.04).round() as usize).max(1) } else { 0 };
    let bg_pad_x = px * 0.35;
    let bg_pad_y = px * 0.12;
    let pad = (outline_r + shadow_off + 3.0 * shadow_blur as f32 + bg_pad_x.max(bg_pad_y) + px * 0.35).ceil() as i32 + 2;
    let pw = (wrap_width.ceil() as i64 + 2 * pad as i64).clamp(1, MAX_TEXT_DIM as i64) as usize;
    let ph = (block_h.ceil() as i64 + 2 * pad as i64).clamp(1, MAX_TEXT_DIM as i64) as usize;

    // Glyph fill layer (premultiplied RGBA).
    let mut fill = vec![0u8; pw * ph * 4];
    for g in &placed {
        let Some(span) = p.spans.get(g.span) else { continue };
        if !span.visible {
            continue;
        }
        let Some(image) = fonts.cache.get_image(&mut fonts.system, g.key).as_ref() else { continue };
        let gx = g.x + pad + image.placement.left;
        let gy = g.y + pad - image.placement.top;
        let (iw, ih) = (image.placement.width as i32, image.placement.height as i32);
        let color = span.color;
        let is_color = match image.content {
            ct::SwashContent::Mask => false,
            ct::SwashContent::Color => true,
            ct::SwashContent::SubpixelMask => continue,
        };
        let need = (iw as usize) * (ih as usize) * if is_color { 4 } else { 1 };
        if image.data.len() < need {
            continue;
        }
        for row in 0..ih {
            let y = gy + row;
            if y < 0 || y >= ph as i32 {
                continue;
            }
            for col in 0..iw {
                let x = gx + col;
                if x < 0 || x >= pw as i32 {
                    continue;
                }
                let di = (y as usize * pw + x as usize) * 4;
                let src = if is_color {
                    let si = (row * iw + col) as usize * 4;
                    let d = &image.data[si..si + 4];
                    let a = d[3] as f32 * (color.a as f32 / 255.0);
                    // Colour bitmaps (emoji) are straight alpha.
                    [d[0] as f32 * a / 255.0, d[1] as f32 * a / 255.0, d[2] as f32 * a / 255.0, a]
                } else {
                    let cov = image.data[(row * iw + col) as usize] as f32 / 255.0;
                    let a = cov * color.a as f32;
                    [color.r as f32 * a / 255.0, color.g as f32 * a / 255.0, color.b as f32 * a / 255.0, a]
                };
                if src[3] > 0.0 {
                    over(&mut fill[di..di + 4], src);
                }
            }
        }
    }

    let mut pix = ts::Pixmap::new(pw as u32, ph as u32)?;

    // 1. Background box around the lines (including not-yet-revealed words,
    //    so the box does not jump as words appear).
    if let Some(bg) = p.background {
        if bg.a > 0 && ink_r > ink_l {
            let x = pad as f32 + ink_l - bg_pad_x;
            let y = pad as f32 - bg_pad_y;
            if let Some(path) = rounded_rect_path(x, y, ink_r - ink_l + 2.0 * bg_pad_x, block_h + 2.0 * bg_pad_y, px * 0.2) {
                pix.fill_path(&path, &paint(bg), ts::FillRule::Winding, ts::Transform::identity(), None);
            }
        }
    }

    let fill_alpha: Vec<u8> = fill.chunks_exact(4).map(|c| c[3]).collect();
    let outline_alpha = match p.outline {
        Some((c, _)) if outline_r > 0.01 && c.a > 0 => Some(dilate(&fill_alpha, pw, ph, outline_r)),
        _ => None,
    };

    let data = pix.data_mut();
    // 2. Drop shadow of everything drawn above it.
    if p.shadow {
        let solid = outline_alpha.as_deref().unwrap_or(&fill_alpha);
        let blurred = blur(solid, pw, ph, shadow_blur);
        let off = shadow_off.round() as i32;
        for y in 0..ph as i32 {
            let sy = y - off;
            if sy < 0 {
                continue;
            }
            for x in 0..pw as i32 {
                let sx = x - off;
                if sx < 0 {
                    continue;
                }
                let a = blurred[sy as usize * pw + sx as usize] as f32 * 0.6;
                if a > 0.0 {
                    let di = (y as usize * pw + x as usize) * 4;
                    over(&mut data[di..di + 4], [0.0, 0.0, 0.0, a]);
                }
            }
        }
    }
    // 3. Outline.
    if let (Some(mask), Some((c, _))) = (&outline_alpha, p.outline) {
        for (i, m) in mask.iter().enumerate() {
            if *m > 0 {
                let a = *m as f32 * c.a as f32 / 255.0;
                over(&mut data[i * 4..i * 4 + 4], [c.r as f32 * a / 255.0, c.g as f32 * a / 255.0, c.b as f32 * a / 255.0, a]);
            }
        }
    }
    // 4. Glyphs.
    for (d, s) in data.chunks_exact_mut(4).zip(fill.chunks_exact(4)) {
        if s[3] > 0 {
            over(d, [s[0] as f32, s[1] as f32, s[2] as f32, s[3] as f32]);
        }
    }
    Some(TextImage { pix, pad, box_w: wrap_width, box_h: block_h })
}

/// Draws a text image so that its layout box is centred on `(cx, cy)`.
fn draw_text_image(canvas: &mut ts::Pixmap, img: &TextImage, cx: f32, cy: f32, rotation_deg: f64, opacity: f32) {
    let left = cx - img.box_w / 2.0 - img.pad as f32;
    let top = cy - img.box_h / 2.0 - img.pad as f32;
    let pp = ts::PixmapPaint { opacity: opacity.clamp(0.0, 1.0), blend_mode: ts::BlendMode::SourceOver, quality: ts::FilterQuality::Nearest };
    if rotation_deg.abs() < 1e-6 {
        canvas.draw_pixmap(left.round() as i32, top.round() as i32, img.pix.as_ref(), &pp, ts::Transform::identity(), None);
    } else {
        let pp = ts::PixmapPaint { quality: ts::FilterQuality::Bilinear, ..pp };
        let xf = ts::Transform::from_rotate_at(rotation_deg as f32, cx, cy).pre_translate(left, top);
        canvas.draw_pixmap(0, 0, img.pix.as_ref(), &pp, xf, None);
    }
}

fn text_params(style: &TextStyle, tr: &Transform, w: u32, h: u32, px_scale: f64) -> TextParams {
    TextParams {
        spans: vec![Span { text: style.text.clone(), color: style.color, visible: true }],
        font: style.font.clone(),
        px: (style.size * h as f64) as f32,
        bold: style.bold,
        italic: style.italic,
        align: style.align,
        wrap_width: (tr.w.max(0.0) * w as f64).max(1.0) as f32,
        background: style.background,
        outline: style.outline.map(|(c, wd)| (c, (wd * px_scale) as f32)),
        shadow: style.shadow,
    }
}

// ---------------------------------------------------------------------------
// Shapes, waveform, progress
// ---------------------------------------------------------------------------

/// Layer box in canvas pixels: centre and size.
struct BoxPx {
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
}

fn layer_box(tr: &Transform, w: u32, h: u32) -> BoxPx {
    BoxPx { cx: (tr.x * w as f64) as f32, cy: (tr.y * h as f64) as f32, w: (tr.w.max(0.0) * w as f64) as f32, h: (tr.h.max(0.0) * h as f64) as f32 }
}

fn box_transform(b: &BoxPx, rotation_deg: f64) -> ts::Transform {
    // Paths are built with the box centre at the origin.
    if rotation_deg.abs() < 1e-6 { ts::Transform::from_translate(b.cx, b.cy) } else { ts::Transform::from_translate(b.cx, b.cy).pre_rotate(rotation_deg as f32) }
}

fn draw_shape(canvas: &mut ts::Pixmap, shape: &Shape, b: &BoxPx, xf: ts::Transform, px_scale: f32) {
    if !(b.w > 0.0 && b.h > 0.0) {
        return;
    }
    let (x, y) = (-b.w / 2.0, -b.h / 2.0);
    match shape.kind {
        ShapeKind::Line => {
            // A horizontal line through the middle of the box, as long as the
            // box is wide. With a stroke it takes the stroke's colour and
            // width; otherwise it is as thick as the box is tall.
            let (color, thickness) = match shape.stroke {
                Some((c, wd)) if wd > 0.0 => (c, (wd as f32 * px_scale).max(0.5)),
                _ => (shape.fill, b.h),
            };
            if let Some(rect) = ts::Rect::from_xywh(x, -thickness / 2.0, b.w, thickness) {
                canvas.fill_path(&ts::PathBuilder::from_rect(rect), &paint(color), ts::FillRule::Winding, xf, None);
            }
        }
        ShapeKind::Rectangle | ShapeKind::Ellipse => {
            let make = |inset: f32| -> Option<ts::Path> {
                let (rw, rh) = (b.w - 2.0 * inset, b.h - 2.0 * inset);
                if !(rw > 0.0 && rh > 0.0) {
                    return None;
                }
                match shape.kind {
                    ShapeKind::Ellipse => ts::PathBuilder::from_oval(ts::Rect::from_xywh(x + inset, y + inset, rw, rh)?),
                    _ => {
                        let radius = (shape.corner_radius.clamp(0.0, 0.5) as f32 * b.w.min(b.h) - inset).max(0.0);
                        rounded_rect_path(x + inset, y + inset, rw, rh, radius)
                    }
                }
            };
            if shape.fill.a > 0 {
                if let Some(path) = make(0.0) {
                    canvas.fill_path(&path, &paint(shape.fill), ts::FillRule::Winding, xf, None);
                }
            }
            if let Some((c, wd)) = shape.stroke {
                let sw = (wd as f32 * px_scale).min(b.w.min(b.h));
                if sw > 0.0 && c.a > 0 {
                    // Inset so the stroke stays inside the layer box.
                    if let Some(path) = make(sw / 2.0) {
                        let stroke = ts::Stroke { width: sw, ..Default::default() };
                        canvas.stroke_path(&path, &paint(c), &stroke, xf, None);
                    }
                }
            }
        }
    }
}

fn draw_progress(canvas: &mut ts::Pixmap, color: Rgba, track: Rgba, fraction: f64, b: &BoxPx, xf: ts::Transform) {
    if !(b.w > 0.0 && b.h > 0.0) {
        return;
    }
    let (x, y) = (-b.w / 2.0, -b.h / 2.0);
    let radius = b.h.min(b.w) / 2.0;
    if track.a > 0 {
        if let Some(path) = rounded_rect_path(x, y, b.w, b.h, radius) {
            canvas.fill_path(&path, &paint(track), ts::FillRule::Winding, xf, None);
        }
    }
    let fw = b.w * fraction.clamp(0.0, 1.0) as f32;
    if fw > 0.01 && color.a > 0 {
        if let Some(path) = rounded_rect_path(x, y, fw, b.h, radius.min(fw / 2.0)) {
            canvas.fill_path(&path, &paint(color), ts::FillRule::Winding, xf, None);
        }
    }
}

fn draw_waveform(canvas: &mut ts::Pixmap, style: &WaveformStyle, peaks: Option<&Peaks>, t: f64, b: &BoxPx, xf: ts::Transform, px_scale: f32) {
    if !(b.w > 0.0 && b.h > 0.0) || style.color.a == 0 {
        return;
    }
    let amp = |from: f64, to: f64| -> f32 { peaks.map(|p| p.amplitude(from, to)).unwrap_or(0.0) };
    let (left, top) = (-b.w / 2.0, -b.h / 2.0);
    let dt = WAVEFORM_WINDOW_SECS / WAVEFORM_BARS as f64;
    let t0 = t - WAVEFORM_WINDOW_SECS / 2.0;
    // Buckets sit on an absolute time grid so the picture scrolls smoothly.
    let k0 = (t0 / dt).floor() as i64;
    let k1 = ((t0 + WAVEFORM_WINDOW_SECS) / dt).ceil() as i64;
    let x_of = |time: f64| -> f32 { left + ((time - t0) / WAVEFORM_WINDOW_SECS) as f32 * b.w };
    let min_h = (2.0 * px_scale).clamp(1.0, b.h);
    match style.kind {
        WaveformKind::Bars | WaveformKind::Mirror => {
            let pitch = b.w / WAVEFORM_BARS as f32;
            let bar_w = (pitch * 0.62).max(1.0);
            let mut pb = ts::PathBuilder::new();
            for k in k0..k1 {
                let (a, bnd) = (k as f64 * dt, (k + 1) as f64 * dt);
                let centre = x_of((a + bnd) / 2.0);
                let x0 = (centre - bar_w / 2.0).max(left);
                let x1 = (centre + bar_w / 2.0).min(left + b.w);
                if x1 - x0 < 0.05 {
                    continue;
                }
                let bh = (amp(a, bnd) * b.h).clamp(min_h, b.h);
                let y0 = match style.kind {
                    WaveformKind::Bars => top + b.h - bh,
                    _ => -bh / 2.0,
                };
                if let Some(r) = ts::Rect::from_xywh(x0, y0, x1 - x0, bh) {
                    pb.push_rect(r);
                }
            }
            if let Some(path) = pb.finish() {
                canvas.fill_path(&path, &paint(style.color), ts::FillRule::Winding, xf, None);
            }
        }
        WaveformKind::Line => {
            // The outline of the level over time: one line above the centre
            // and its mirror image below.
            let stroke_w = (b.h * 0.02).max(2.0 * px_scale).max(1.0);
            let half = (b.h - stroke_w).max(0.0) / 2.0;
            let mut points: Vec<(f32, f32)> = Vec::with_capacity((k1 - k0 + 2).max(0) as usize);
            for k in (k0 - 1)..=k1 {
                let (a, bnd) = (k as f64 * dt, (k + 1) as f64 * dt);
                let time = ((a + bnd) / 2.0).clamp(t0, t0 + WAVEFORM_WINDOW_SECS);
                points.push((x_of(time), amp(a, bnd) * half));
            }
            let mut pb = ts::PathBuilder::new();
            for sign in [-1.0f32, 1.0] {
                for (i, (x, y)) in points.iter().enumerate() {
                    if i == 0 {
                        pb.move_to(*x, sign * y);
                    } else {
                        pb.line_to(*x, sign * y);
                    }
                }
            }
            if let Some(path) = pb.finish() {
                let stroke = ts::Stroke { width: stroke_w, line_join: ts::LineJoin::Round, line_cap: ts::LineCap::Butt, ..Default::default() };
                canvas.stroke_path(&path, &paint(style.color), &stroke, xf, None);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

fn is_generated(content: &LayerContent) -> bool {
    matches!(content, LayerContent::Text(_) | LayerContent::Shape(_) | LayerContent::Waveform(_) | LayerContent::Progress { .. })
}

fn is_animated(content: &LayerContent) -> bool {
    matches!(content, LayerContent::Waveform(_) | LayerContent::Progress { .. })
}

/// Extra placement applied on top of the layer transform (preview animation).
#[derive(Clone, Copy)]
struct Extra {
    alpha: f32,
    dx: f32,
    dy: f32,
}

const NO_EXTRA: Extra = Extra { alpha: 1.0, dx: 0.0, dy: 0.0 };

fn px_scale(comp: &Composition, h: u32) -> f64 {
    if comp.height == 0 { 1.0 } else { h as f64 / comp.height as f64 }
}

fn draw_layer(canvas: &mut ts::Pixmap, comp: &Composition, layer: &Layer, t: f64, peaks: Option<&Peaks>, extra: Extra) {
    let (w, h) = (canvas.width(), canvas.height());
    let tr = &layer.transform;
    let opacity = (tr.opacity.clamp(0.0, 1.0) as f32 * extra.alpha).clamp(0.0, 1.0);
    if opacity <= 0.0 || !is_generated(&layer.content) {
        return;
    }
    let scale = px_scale(comp, h);
    let mut b = layer_box(tr, w, h);
    b.cx += extra.dx;
    b.cy += extra.dy;
    if !(b.cx.is_finite() && b.cy.is_finite() && b.w.is_finite() && b.h.is_finite() && tr.rotation_deg.is_finite()) {
        return;
    }
    if let LayerContent::Text(style) = &layer.content {
        let params = text_params(style, tr, w, h, scale);
        let img = render_text(&mut lock_fonts(), &params);
        if let Some(img) = img {
            draw_text_image(canvas, &img, b.cx, b.cy, tr.rotation_deg, opacity);
        }
        return;
    }
    let xf = box_transform(&b, tr.rotation_deg);
    let draw = |target: &mut ts::Pixmap| match &layer.content {
        LayerContent::Shape(shape) => draw_shape(target, shape, &b, xf, scale as f32),
        LayerContent::Waveform(style) => draw_waveform(target, style, peaks, t, &b, xf, scale as f32),
        LayerContent::Progress { color, track_color } => {
            let total = comp.duration();
            let fraction = if total > 0.0 { (t / total).clamp(0.0, 1.0) } else { 0.0 };
            draw_progress(target, *color, *track_color, fraction, &b, xf);
        }
        _ => {}
    };
    if opacity >= 0.999 {
        draw(canvas);
    } else if let Some(mut tmp) = ts::Pixmap::new(w, h) {
        // Group opacity: draw opaque, then blend the group.
        draw(&mut tmp);
        let pp = ts::PixmapPaint { opacity, blend_mode: ts::BlendMode::SourceOver, quality: ts::FilterQuality::Nearest };
        canvas.draw_pixmap(0, 0, tmp.as_ref(), &pp, ts::Transform::identity(), None);
    }
}

fn to_frame(pix: ts::Pixmap) -> Frame {
    let (width, height) = (pix.width(), pix.height());
    Frame { width, height, rgba: pix.take_demultiplied() }
}

/// Renders one generated layer (text, shape, waveform, progress) as a
/// full-canvas image of `w` x `h` at composition time `t`.
///
/// The layer's own time range and `hidden` flag are *not* applied here (the
/// caller decides when a layer shows; see [`render_overlay`]); `t` only
/// drives animated content. `peaks` is the composition audio (see
/// [`composition_peaks`]) for waveform layers; without it they draw silence.
/// Returns `None` for media layers, which ffmpeg / the player draw.
pub fn render_layer(project: &Project, comp: &Composition, layer: &Layer, t: f64, w: u32, h: u32, peaks: Option<&Peaks>) -> Option<Frame> {
    let _ = project;
    if !is_generated(&layer.content) {
        return None;
    }
    let mut canvas = ts::Pixmap::new(w.max(1), h.max(1))?;
    if w > 0 && h > 0 {
        draw_layer(&mut canvas, comp, layer, t, peaks, NO_EXTRA);
    }
    Some(to_frame(canvas))
}

// ---------------------------------------------------------------------------
// Captions
// ---------------------------------------------------------------------------

/// Which line of the cue each word sits on (mirrors `mango_core::captions`).
fn line_index_per_word(c: &Cue) -> Vec<usize> {
    let mut out = Vec::with_capacity(c.words.len());
    let mut line = 0usize;
    let mut remaining = c.lines.first().map(|l| l.split_whitespace().count()).unwrap_or(0);
    for w in &c.words {
        while remaining == 0 && line + 1 < c.lines.len() {
            line += 1;
            remaining = c.lines[line].split_whitespace().count();
        }
        out.push(line);
        // A "word" token may itself contain spaces after a correction.
        remaining = remaining.saturating_sub(w.text.split_whitespace().count().max(1));
    }
    out
}

/// The cue showing at `t`, and the index of the word being spoken.
fn cue_at(cues: &[Cue], t: f64) -> Option<(&Cue, usize)> {
    let cue = cues.iter().find(|c| t >= c.start && t < c.end)?;
    let active = cue.words.iter().rposition(|w| w.start <= t).unwrap_or(0);
    Some((cue, active))
}

fn caption_params(comp: &Composition, cue: &Cue, active: usize, w: u32, h: u32) -> TextParams {
    let st = &comp.captions;
    let lines = line_index_per_word(cue);
    let mut spans = Vec::with_capacity(cue.words.len());
    for (k, word) in cue.words.iter().enumerate() {
        let mut text = word.text.clone();
        if k + 1 < cue.words.len() {
            text.push(if lines[k + 1] != lines[k] { '\n' } else { ' ' });
        }
        let (color, visible) = match st.highlight {
            CaptionHighlight::None => (st.color, true),
            CaptionHighlight::ActiveWord => (if k == active { st.highlight_color } else { st.color }, true),
            CaptionHighlight::Reveal => (st.color, k <= active),
        };
        spans.push(Span { text, color, visible });
    }
    TextParams {
        spans,
        font: st.font.clone(),
        px: (st.size * h as f64) as f32,
        bold: st.bold,
        italic: false,
        align: TextAlign::Center,
        wrap_width: (CAPTION_WIDTH * w as f64).max(1.0) as f32,
        background: st.background,
        outline: st.outline.map(|(c, wd)| (c, (wd * px_scale(comp, h)) as f32)),
        shadow: false,
    }
}

fn draw_caption(canvas: &mut ts::Pixmap, comp: &Composition, cue: &Cue, active: usize) -> bool {
    let (w, h) = (canvas.width(), canvas.height());
    let params = caption_params(comp, cue, active, w, h);
    let img = render_text(&mut lock_fonts(), &params);
    let Some(img) = img else { return false };
    // Keep the whole block on the canvas even when `y` is near an edge.
    let half = img.box_h / 2.0;
    let margin = (h as f32 * 0.02).min(h as f32 / 2.0);
    let mut cy = (comp.captions.y * h as f64) as f32;
    let (lo, hi) = (half + margin, h as f32 - half - margin);
    if lo <= hi {
        cy = cy.clamp(lo, hi);
    } else {
        cy = h as f32 / 2.0;
    }
    draw_text_image(canvas, &img, w as f32 / 2.0, cy, 0.0, 1.0);
    true
}

/// Renders the caption card showing at `t`, or `None` when no cue is up.
/// `cues` come from [`mango_core::captions::cues`] (see [`caption_cues`]).
pub fn render_captions(comp: &Composition, cues: &[Cue], t: f64, w: u32, h: u32) -> Option<Frame> {
    let (cue, active) = cue_at(cues, t)?;
    let mut canvas = ts::Pixmap::new(w.max(1), h.max(1))?;
    if !draw_caption(&mut canvas, comp, cue, active) {
        return None;
    }
    Some(to_frame(canvas))
}

/// The caption cues for a composition, using its caption settings.
pub fn caption_cues(comp: &Composition) -> Vec<Cue> {
    mango_core::captions::cues(comp, &CueOptions::from(&comp.captions))
}

// ---------------------------------------------------------------------------
// Composition audio peaks (for waveform layers)
// ---------------------------------------------------------------------------

fn db_to_lin(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Approximate peaks of the composition's audio in composition time: the
/// script track plus unmuted audio/video layers (gain applied; effects,
/// ducking and volume automation are not). Source peaks come from
/// [`crate::waveform::peaks`], so this is cheap after the first call.
pub fn composition_peaks(project: &Project, comp: &Composition, per_second: u32) -> Result<Peaks> {
    let ps = per_second.max(1);
    let total = comp.duration().max(0.0);
    let n = (total * ps as f64).ceil() as usize;
    let mut min = vec![0f32; n];
    let mut max = vec![0f32; n];
    let mut cache: HashMap<mango_core::AssetId, Arc<Peaks>> = HashMap::new();
    let mut source = |id: mango_core::AssetId| -> Result<Option<Arc<Peaks>>> {
        if let Some(p) = cache.get(&id) {
            return Ok(Some(p.clone()));
        }
        let Some(asset) = project.asset(id) else { return Ok(None) };
        if !asset.has_audio || asset.kind == MediaKind::Image {
            return Ok(None);
        }
        let p = Arc::new(crate::waveform::peaks(&asset.path, ps).with_context(|| format!("reading the waveform of {}", asset.name))?);
        cache.insert(id, p.clone());
        Ok(Some(p))
    };
    // (comp_start, comp_end, src_start, speed, gain, loop length)
    let mut add = |p: &Peaks, comp_start: f64, comp_end: f64, src_start: f64, speed: f64, gain: f32| {
        let a = ((comp_start.max(0.0)) * ps as f64).round() as usize;
        let b = ((comp_end.min(total)) * ps as f64).round() as usize;
        for i in a..b.min(n) {
            let src_t = src_start + (i as f64 / ps as f64 - comp_start) * speed;
            let j = (src_t * ps as f64).floor();
            if j < 0.0 || j as usize >= p.len() {
                continue;
            }
            let j = j as usize;
            min[i] = (min[i] + p.min[j] * gain).clamp(-1.0, 1.0);
            max[i] = (max[i] + p.max[j] * gain).clamp(-1.0, 1.0);
        }
    };
    if !comp.script_audio.muted {
        let gain = db_to_lin(comp.script_audio.gain_db);
        for clip in comp.script_clips() {
            let Some(id) = clip.asset else { continue };
            if let Some(p) = source(id)? {
                add(&p, clip.comp_start, clip.comp_end(), clip.src_start, 1.0, gain);
            }
        }
    }
    for layer in &comp.layers {
        if layer.hidden || layer.duration <= 0.0 {
            continue;
        }
        let (LayerContent::Audio(m) | LayerContent::Video(m)) = &layer.content else { continue };
        if m.audio.muted {
            continue;
        }
        if let Some(p) = source(m.asset)? {
            add(&p, layer.start, layer.end(), m.src_in.max(0.0), m.speed.max(0.05), db_to_lin(m.audio.gain_db));
        }
    }
    Ok(Peaks { per_second: ps, min, max })
}

fn audio_signature(project: &Project, comp: &Composition) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    comp.id.hash(&mut h);
    (comp.script_audio.muted, comp.script_audio.gain_db.to_bits()).hash(&mut h);
    for c in comp.script_clips() {
        c.asset.hash(&mut h);
        if let Some(a) = c.asset.and_then(|id| project.asset(id)) {
            a.path.hash(&mut h);
        }
        (c.src_start.to_bits(), c.src_end.to_bits(), c.comp_start.to_bits()).hash(&mut h);
    }
    for l in &comp.layers {
        if let LayerContent::Audio(m) | LayerContent::Video(m) = &l.content {
            (l.id, l.hidden, l.start.to_bits(), l.duration.to_bits()).hash(&mut h);
            (m.asset, m.src_in.to_bits(), m.speed.to_bits(), m.audio.muted, m.audio.gain_db.to_bits()).hash(&mut h);
        }
    }
    h.finish()
}

/// [`composition_peaks`] memoised for the preview; `None` if a source cannot
/// be read (the waveform then draws as silence).
fn preview_peaks(project: &Project, comp: &Composition) -> Option<Arc<Peaks>> {
    static CACHE: OnceLock<Mutex<Option<(u64, Option<Arc<Peaks>>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let sig = audio_signature(project, comp);
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((s, p)) = guard.as_ref() {
        if *s == sig {
            return p.clone();
        }
    }
    let p = composition_peaks(project, comp, COMPOSITION_PEAKS_PER_SECOND).ok().map(Arc::new);
    *guard = Some((sig, p.clone()));
    p
}

// ---------------------------------------------------------------------------
// Preview overlay
// ---------------------------------------------------------------------------

/// Fade and slide for a layer at `t`, matching what the export does with
/// ffmpeg (`mango_core::render`): returns alpha and a pixel offset.
fn animation_at(layer: &Layer, t: f64, w: u32, h: u32) -> Extra {
    let (start, end) = (layer.start, layer.end());
    let half = ((end - start) / 2.0).max(0.0);
    let implied = |a: Animation| if matches!(a, Animation::Fade | Animation::ZoomIn | Animation::ZoomOut) { ANIM_SECS } else { 0.0 };
    let fi = (if layer.fade_in > 0.0 { layer.fade_in } else { implied(layer.animation_in) }).min(half);
    let fo = (if layer.fade_out > 0.0 { layer.fade_out } else { implied(layer.animation_out) }).min(half);
    let mut alpha = 1.0f64;
    if fi > 1e-4 {
        alpha *= ((t - start) / fi).clamp(0.0, 1.0);
    }
    if fo > 1e-4 {
        alpha *= ((end - t) / fo).clamp(0.0, 1.0);
    }
    let d = ANIM_SECS.min(half).max(0.001);
    let pin = 1.0 - (1.0 - ((t - start) / d).clamp(0.0, 1.0)).powi(3);
    let pout = ((t - (end - d)) / d).clamp(0.0, 1.0).powi(3);
    let (wf, hf) = (w as f64, h as f64);
    let (mut dx, mut dy) = (0.0, 0.0);
    match layer.animation_in {
        Animation::SlideLeft => dx += wf * (1.0 - pin),
        Animation::SlideRight => dx -= wf * (1.0 - pin),
        Animation::SlideUp => dy += hf * (1.0 - pin),
        Animation::SlideDown => dy -= hf * (1.0 - pin),
        _ => {}
    }
    match layer.animation_out {
        Animation::SlideLeft => dx -= wf * pout,
        Animation::SlideRight => dx += wf * pout,
        Animation::SlideUp => dy -= hf * pout,
        Animation::SlideDown => dy += hf * pout,
        _ => {}
    }
    Extra { alpha: alpha as f32, dx: dx as f32, dy: dy as f32 }
}

/// Everything generated that is visible at `t` — non-hidden generated layers
/// in track order with their fades and slide animations, then captions when
/// enabled — composited into one transparent frame for the live preview.
pub fn render_overlay(project: &Project, comp: &Composition, cues: &[Cue], t: f64, w: u32, h: u32) -> Frame {
    let Some(mut canvas) = ts::Pixmap::new(w, h) else {
        return Frame::transparent(w, h);
    };
    let mut visible: Vec<(i32, usize, &Layer)> = comp
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.hidden && l.duration > 0.0 && is_generated(&l.content) && t >= l.start && t < l.end())
        .map(|(i, l)| (l.track, i, l))
        .collect();
    visible.sort_by_key(|v| (v.0, v.1));
    let needs_peaks = visible.iter().any(|(_, _, l)| matches!(l.content, LayerContent::Waveform(_)));
    let peaks = if needs_peaks { preview_peaks(project, comp) } else { None };
    for (_, _, layer) in visible {
        draw_layer(&mut canvas, comp, layer, t, peaks.as_deref(), animation_at(layer, t, w, h));
    }
    if comp.captions.enabled {
        if let Some((cue, active)) = cue_at(cues, t) {
            draw_caption(&mut canvas, comp, cue, active);
        }
    }
    to_frame(canvas)
}

// ---------------------------------------------------------------------------
// Export overlays
// ---------------------------------------------------------------------------

/// A PNG sequence being assembled: frames with durations, identical
/// neighbours merged, identical images stored once.
struct Sequence<'a> {
    dir: &'a Path,
    /// (file name, duration)
    entries: Vec<(String, f64)>,
    by_hash: HashMap<u64, String>,
    names: HashSet<String>,
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl<'a> Sequence<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir, entries: Vec::new(), by_hash: HashMap::new(), names: HashSet::new() }
    }

    /// Appends an already-encoded PNG shown for `duration` seconds.
    fn push(&mut self, png: &[u8], duration: f64) -> Result<()> {
        if duration <= 0.0 {
            return Ok(());
        }
        let hash = content_hash(png);
        let name = match self.by_hash.get(&hash) {
            Some(n) => n.clone(),
            None => {
                let name = format!("f{:06}.png", self.names.len());
                let path = self.dir.join(&name);
                std::fs::write(&path, png).with_context(|| format!("could not write {}", path.display()))?;
                self.names.insert(name.clone());
                self.by_hash.insert(hash, name.clone());
                name
            }
        };
        match self.entries.last_mut() {
            Some((last, d)) if *last == name => *d += duration,
            _ => self.entries.push((name, duration)),
        }
        Ok(())
    }

    fn is_single_image(&self) -> bool {
        self.entries.len() == 1
    }

    fn finish(self) -> Result<std::path::PathBuf> {
        let list = self.dir.join("list.ffconcat");
        std::fs::write(&list, ffconcat(&self.entries)).with_context(|| format!("could not write {}", list.display()))?;
        Ok(list)
    }
}

/// Runs `job(i)` for `0..n` on a few threads, returning results in order.
fn parallel_map<T: Send>(n: usize, job: impl Fn(usize) -> Result<T> + Sync) -> Result<Vec<T>> {
    let threads = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4).clamp(1, 8).min(n.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::atomic::AtomicBool::new(false);
    let results: Mutex<Vec<Option<Result<T>>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= n || failed.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    let r = job(i);
                    if r.is_err() {
                        failed.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::with_capacity(n);
    let mut missing = false;
    for r in results {
        match r {
            Some(Ok(v)) => out.push(v),
            Some(Err(e)) => return Err(e),
            None => missing = true,
        }
    }
    if missing {
        anyhow::bail!("overlay rendering stopped early");
    }
    Ok(out)
}

fn in_range(range: Option<(f64, f64)>, from: f64, to: f64, slack: f64) -> bool {
    match range {
        Some((a, b)) => to >= a - slack && from <= b + slack,
        None => true,
    }
}

/// Writes the overlay inputs the export needs into `dir` and returns them.
///
/// One [`OverlayInput`] per visible generated layer: a single PNG when the
/// layer looks the same for its whole life (text, shapes), otherwise a PNG
/// sequence with an ffconcat list (waveform, progress; one frame per export
/// frame, identical neighbours merged). When captions are enabled, one more
/// sequence covering the whole composition, on a track above every layer.
/// Frames outside `settings.range` are left blank since they are never shown.
pub fn overlays_for_export(project: &Project, comp: &Composition, settings: &ExportSettings, dir: &Path) -> Result<Vec<OverlayInput>> {
    overlays_for_export_cancellable(project, comp, settings, dir, None)
}

/// [`overlays_for_export`] that stops with a [`crate::Cancelled`] error soon
/// after `cancel` is set.
pub fn overlays_for_export_cancellable(
    project: &Project,
    comp: &Composition,
    settings: &ExportSettings,
    dir: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<OverlayInput>> {
    let cancelled = || -> Result<()> {
        match cancel {
            Some(c) if c.load(Ordering::SeqCst) => Err(crate::Cancelled.into()),
            _ => Ok(()),
        }
    };
    let (w, h) = (settings.width, settings.height);
    let fps = settings.fps;
    anyhow::ensure!(w > 0 && h > 0, "export size must not be zero");
    anyhow::ensure!(fps.is_finite() && fps > 0.0, "export frame rate must be positive");
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let frame_dur = 1.0 / fps;
    let blank = encode_png(&Frame::transparent(w, h))?;
    let mut out: Vec<OverlayInput> = Vec::new();
    let mut peaks: Option<Peaks> = None;

    for (index, layer) in comp.layers.iter().enumerate() {
        if layer.hidden || layer.duration <= 0.0 || !is_generated(&layer.content) || layer.transform.opacity <= 0.0 {
            continue;
        }
        if !layer.start.is_finite() || !layer.duration.is_finite() {
            continue;
        }
        cancelled()?;
        let base = OverlayInput {
            path: dir.join(format!("layer_{index:03}.png")),
            sequence: false,
            start: layer.start,
            end: layer.end(),
            track: layer.track,
            fade_in: layer.fade_in,
            fade_out: layer.fade_out,
            animation_in: layer.animation_in,
            animation_out: layer.animation_out,
        };
        if !is_animated(&layer.content) {
            let frame = render_layer(project, comp, layer, layer.start, w, h, None).ok_or_else(|| anyhow::anyhow!("could not draw layer {}", layer.name))?;
            crate::frames::save_png(&frame, &base.path)?;
            out.push(base);
            continue;
        }
        if matches!(layer.content, LayerContent::Waveform(_)) && peaks.is_none() {
            peaks = Some(composition_peaks(project, comp, COMPOSITION_PEAKS_PER_SECOND)?);
        }
        let seq_dir = dir.join(format!("layer_{index:03}"));
        std::fs::create_dir_all(&seq_dir).with_context(|| format!("could not create {}", seq_dir.display()))?;
        // Overlay frame j is shown on output frame `first + j`.
        let first = (layer.start * fps - 1e-6).ceil().max(0.0) as u64;
        let count = (((layer.end() * fps - 1e-6).ceil().max(0.0) as u64).saturating_sub(first)).max(1) as usize;
        let peaks_ref = peaks.as_ref();
        let encoded: Vec<Option<Vec<u8>>> = parallel_map(count, |j| {
            cancelled()?;
            let t = (first + j as u64) as f64 / fps;
            if !in_range(settings.range, t, t + frame_dur, 2.0 * frame_dur) {
                return Ok(None);
            }
            let frame = render_layer(project, comp, layer, t, w, h, peaks_ref).ok_or_else(|| anyhow::anyhow!("could not draw layer {}", layer.name))?;
            Ok(Some(encode_png(&frame)?))
        })?;
        let mut seq = Sequence::new(&seq_dir);
        for png in &encoded {
            seq.push(png.as_deref().unwrap_or(&blank), frame_dur)?;
        }
        if seq.is_single_image() {
            // Never changed (e.g. a waveform over silence): a still will do.
            let only = seq_dir.join(&seq.entries[0].0);
            std::fs::copy(&only, &base.path).with_context(|| format!("could not write {}", base.path.display()))?;
            out.push(base);
        } else {
            let list = seq.finish()?;
            out.push(OverlayInput { path: list, sequence: true, ..base });
        }
    }

    if comp.captions.enabled {
        let cues = caption_cues(comp);
        let total = comp.duration();
        if !cues.is_empty() && total > 0.0 {
            // Cards and when they show, snapped to the output frame grid.
            let snap = |t: f64| (t * fps).round().max(0.0) / fps;
            let mut cards: Vec<(f64, f64, &Cue, usize)> = Vec::new();
            for cue in &cues {
                match comp.captions.highlight {
                    CaptionHighlight::None => cards.push((snap(cue.start), snap(cue.end), cue, 0)),
                    CaptionHighlight::ActiveWord | CaptionHighlight::Reveal => {
                        for (wi, word) in cue.words.iter().enumerate() {
                            let start = if wi == 0 { cue.start } else { word.start };
                            let end = if wi + 1 < cue.words.len() { cue.words[wi + 1].start } else { cue.end };
                            cards.push((snap(start), snap(end), cue, wi));
                        }
                    }
                }
            }
            // Cues are held a moment after their last word; never past the end.
            for card in &mut cards {
                card.0 = card.0.min(total);
                card.1 = card.1.min(total);
            }
            cards.retain(|c| c.1 > c.0 + 1e-9);
            let seq_dir = dir.join("captions");
            std::fs::create_dir_all(&seq_dir).with_context(|| format!("could not create {}", seq_dir.display()))?;
            let encoded: Vec<Option<Vec<u8>>> = parallel_map(cards.len(), |i| {
                cancelled()?;
                let (start, end, cue, active) = cards[i];
                if !in_range(settings.range, start, end, 2.0 * frame_dur) {
                    return Ok(None);
                }
                let mut canvas = ts::Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("invalid canvas size {w}x{h}"))?;
                draw_caption(&mut canvas, comp, cue, active);
                Ok(Some(encode_png(&to_frame(canvas))?))
            })?;
            let mut seq = Sequence::new(&seq_dir);
            let mut cursor = 0.0f64;
            for (card, png) in cards.iter().zip(&encoded) {
                let start = card.0.max(cursor);
                if card.1 <= start {
                    continue;
                }
                seq.push(&blank, start - cursor)?;
                seq.push(png.as_deref().unwrap_or(&blank), card.1 - start)?;
                cursor = card.1;
            }
            let end = total;
            seq.push(&blank, end - cursor)?;
            let track = comp.layers.iter().map(|l| l.track).max().unwrap_or(0).saturating_add(1).max(1);
            let list = seq.finish()?;
            out.push(OverlayInput {
                path: list,
                sequence: true,
                start: 0.0,
                end,
                track,
                fade_in: 0.0,
                fade_out: 0.0,
                animation_in: Animation::None,
                animation_out: Animation::None,
            });
        }
    }
    Ok(out)
}
