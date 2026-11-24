//! Rasteriser tests: text, shapes, waveform, progress, captions, the preview
//! compositor and the overlay files written for export.

mod common;
use common::*;
use mango_core::captions::{Cue, CueWord};
use mango_core::render::ExportSettings;
use mango_core::*;
use mango_media::raster::*;
use mango_media::{Frame, Peaks};

const W: u32 = 640;
const H: u32 = 360;

fn comp() -> Composition {
    let mut c = Composition::new("Test");
    c.width = W;
    c.height = H;
    c
}

fn project() -> Project {
    Project::new("Test")
}

fn text_layer(text: &str, tr: Transform, style: impl FnOnce(&mut TextStyle)) -> Layer {
    let mut st = TextStyle { text: text.into(), size: 0.1, color: Rgba::rgb(255, 0, 0), bold: true, shadow: false, ..TextStyle::default() };
    style(&mut st);
    let mut l = Layer::new("Text", 0.0, 5.0, LayerContent::Text(st));
    l.transform = tr;
    l
}

fn tr(x: f64, y: f64, w: f64, h: f64) -> Transform {
    Transform { x, y, w, h, rotation_deg: 0.0, opacity: 1.0 }
}

fn render(layer: &Layer) -> Frame {
    let f = render_layer(&project(), &comp(), layer, 0.0, W, H, None).expect("generated layers render");
    assert_eq!((f.width, f.height), (W, H));
    assert_eq!(f.rgba.len(), (W * H * 4) as usize);
    f
}

/// Bounding box (x0, y0, x1, y1 inclusive) of pixels matching `pred`.
fn bbox(f: &Frame, pred: impl Fn([u8; 4]) -> bool) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..f.height {
        for x in 0..f.width {
            if pred(f.pixel(x, y)) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
                });
            }
        }
    }
    b
}

fn count(f: &Frame, pred: impl Fn([u8; 4]) -> bool) -> usize {
    f.rgba.chunks_exact(4).filter(|p| pred([p[0], p[1], p[2], p[3]])).count()
}

fn ink(p: [u8; 4]) -> bool {
    p[3] > 0
}
fn solid(p: [u8; 4]) -> bool {
    p[3] > 128
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[test]
fn text_lands_where_the_transform_says() {
    for (cx, cy) in [(0.5, 0.5), (0.25, 0.2), (0.7, 0.8)] {
        let f = render(&text_layer("Hxx Hxx", tr(cx, cy, 0.4, 0.2), |_| {}));
        let (x0, y0, x1, y1) = bbox(&f, ink).expect("text draws");
        let (mx, my) = ((x0 + x1) as f64 / 2.0 / W as f64, (y0 + y1) as f64 / 2.0 / H as f64);
        assert!((mx - cx).abs() < 0.015, "centre x {mx} for transform x {cx}");
        assert!((my - cy).abs() < 0.03, "centre y {my} for transform y {cy}");
        // Inside the layer box, and transparent everywhere outside it.
        let (bx0, bx1) = ((cx - 0.2) * W as f64, (cx + 0.2) * W as f64);
        let (by0, by1) = ((cy - 0.1) * H as f64, (cy + 0.1) * H as f64);
        assert!(x0 as f64 >= bx0 && x1 as f64 <= bx1, "ink x {x0}..{x1} inside box {bx0}..{bx1}");
        assert!(y0 as f64 >= by0 && y1 as f64 <= by1, "ink y {y0}..{y1} inside box {by0}..{by1}");
        // Height of capital letters is about 0.7 of the font size (0.1 * 360 = 36 px).
        let cap = (y1 - y0 + 1) as f64;
        assert!(cap > 20.0 && cap < 32.0, "cap height {cap} px for a 36 px font");
    }
}

#[test]
fn text_size_is_a_fraction_of_canvas_height() {
    let height = |size: f64, h: u32| {
        let f = render_layer(&project(), &comp(), &text_layer("H", tr(0.5, 0.5, 1.0, 1.0), |s| s.size = size), 0.0, h * 16 / 9, h, None).unwrap();
        let (_, y0, _, y1) = bbox(&f, solid).unwrap();
        (y1 - y0 + 1) as f64
    };
    let (small, big) = (height(0.1, 360), height(0.2, 360));
    assert!((big / small - 2.0).abs() < 0.15, "doubling the size doubles the glyphs: {small} -> {big}");
    // The same layer at twice the canvas size is the same picture, twice as big.
    let doubled = height(0.1, 720);
    assert!((doubled / small - 2.0).abs() < 0.15, "rendering at 2x scales the text 2x: {small} -> {doubled}");
}

#[test]
fn text_content_and_style_change_the_pixels() {
    let base = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.3), |_| {}));
    assert_eq!(base, render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.3), |_| {})), "rendering is deterministic");
    assert_ne!(base, render(&text_layer("Hallo", tr(0.5, 0.5, 0.8, 0.3), |_| {})), "different text, different pixels");
    let regular = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.3), |s| s.bold = false));
    assert!(count(&base, solid) as f64 > count(&regular, solid) as f64 * 1.15, "bold has more ink: {} vs {}", count(&base, solid), count(&regular, solid));
    let italic = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.3), |s| s.italic = true));
    assert_ne!(base, italic, "italic differs");
    assert!(count(&italic, solid) > 100);
    // A different installed family draws differently.
    assert!(font_family_installed("Helvetica Neue") && font_family_installed("Courier New"), "macOS system fonts expected");
    let courier = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.3), |s| s.font = "Courier New".into()));
    assert_ne!(base, courier);
    let (cx0, _, cx1, _) = bbox(&courier, ink).unwrap();
    let (hx0, _, hx1, _) = bbox(&base, ink).unwrap();
    assert!((cx1 - cx0) as f64 > (hx1 - hx0) as f64 * 1.1, "monospace Courier sets 'Hello' wider than Helvetica: {} vs {}", cx1 - cx0, hx1 - hx0);
}

#[test]
fn font_lookup_is_case_insensitive_and_falls_back() {
    let base = render(&text_layer("Fallback", tr(0.5, 0.5, 0.9, 0.3), |s| s.font = "Helvetica Neue".into()));
    let lower = render(&text_layer("Fallback", tr(0.5, 0.5, 0.9, 0.3), |s| s.font = "helvetica neue".into()));
    assert_eq!(base, lower, "family names match regardless of case");
    assert_eq!(resolve_font_family("HELVETICA NEUE").as_deref(), Some("Helvetica Neue"));
    assert!(!font_family_installed("No Such Font 12345"));
    assert_eq!(resolve_font_family("No Such Font 12345").as_deref(), Some("Helvetica Neue"), "missing families fall back to Helvetica Neue");
    let missing = render(&text_layer("Fallback", tr(0.5, 0.5, 0.9, 0.3), |s| s.font = "No Such Font 12345".into()));
    assert_eq!(missing, base, "a missing font renders with the fallback, not nothing");
    let empty_name = render(&text_layer("Fallback", tr(0.5, 0.5, 0.9, 0.3), |s| s.font = String::new()));
    assert!(count(&empty_name, solid) > 300);
    let families = font_families();
    assert!(families.len() > 20 && families.iter().any(|f| f == "Helvetica Neue"));
}

#[test]
fn text_colours_match_the_style() {
    for color in [Rgba::rgb(255, 0, 0), Rgba::rgb(12, 200, 99), Rgba::WHITE, Rgba::BLACK] {
        let f = render(&text_layer("MMMM", tr(0.5, 0.5, 0.9, 0.5), |s| {
            s.color = color;
            s.size = 0.3;
        }));
        let full = count(&f, |p| p[3] == 255);
        assert!(full > 2000, "big bold text has solid interiors, {full} px");
        // Every fully covered pixel is exactly the text colour (straight alpha).
        assert_eq!(count(&f, |p| p[3] == 255 && [p[0], p[1], p[2]] != [color.r, color.g, color.b]), 0, "colour {color:?}");
        // Edge pixels are the same colour with partial alpha, not darkened (no premultiplied leak).
        let off = count(&f, |p| p[3] > 40 && p[3] < 255 && (0..3).any(|c| (p[c] as i32 - [color.r, color.g, color.b][c] as i32).abs() > 6));
        assert_eq!(off, 0, "anti-aliased edges keep the text colour");
    }
    // Colour alpha and layer opacity both scale the alpha.
    let half = render(&text_layer("MMMM", tr(0.5, 0.5, 0.9, 0.5), |s| {
        s.color = Rgba::rgba(255, 0, 0, 128);
        s.size = 0.3;
    }));
    assert!(bbox(&half, |p| p[3] > 130).is_none() && count(&half, |p| p[3] >= 126) > 2000, "50% colour alpha");
    let mut layer = text_layer("MMMM", tr(0.5, 0.5, 0.9, 0.5), |s| s.size = 0.3);
    layer.transform.opacity = 0.25;
    let quarter = render(&layer);
    let max_a = quarter.rgba.chunks_exact(4).map(|p| p[3]).max().unwrap();
    assert!((62..=66).contains(&max_a), "25% opacity gives alpha 64, got {max_a}");
    layer.transform.opacity = 0.0;
    assert!(render(&layer).is_fully_transparent());
}

#[test]
fn text_alignment_moves_ink_within_the_box() {
    let edges = |align: TextAlign| {
        let f = render(&text_layer("Hi", tr(0.5, 0.5, 0.8, 0.3), |s| s.align = align));
        let (x0, _, x1, _) = bbox(&f, solid).unwrap();
        (x0 as f64, x1 as f64)
    };
    let (box_l, box_r) = (0.1 * W as f64, 0.9 * W as f64);
    let (l0, l1) = edges(TextAlign::Left);
    let (c0, c1) = edges(TextAlign::Center);
    let (r0, r1) = edges(TextAlign::Right);
    assert!(l0 - box_l < 8.0 && l0 >= box_l - 1.0, "left-aligned text starts at the box's left edge: {l0} vs {box_l}");
    assert!(box_r - r1 < 8.0 && r1 <= box_r + 1.0, "right-aligned text ends at the box's right edge: {r1} vs {box_r}");
    assert!(((c0 + c1) / 2.0 - W as f64 / 2.0).abs() < 4.0, "centred text is centred");
    assert!((l1 - l0 - (r1 - r0)).abs() <= 2.0 && (l1 - l0 - (c1 - c0)).abs() <= 2.0, "alignment does not change the text's width");
}

#[test]
fn explicit_newlines_and_wrapping_make_lines() {
    let height = |text: &str, w: f64| {
        let f = render(&text_layer(text, tr(0.5, 0.5, w, 0.9), |s| s.size = 0.08));
        let (x0, y0, x1, y1) = bbox(&f, solid).unwrap();
        ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64, x0 as f64, x1 as f64)
    };
    let line = 0.08 * H as f64 * 1.2; // line height is 1.2 x font size
    let (_, one, _, _) = height("Hxg", 0.9);
    let (_, two, _, _) = height("Hxg\nHxg", 0.9);
    let (_, three, _, _) = height("Hxg\nHxg\nHxg", 0.9);
    assert!((two - one - line).abs() < 3.0, "a newline adds one line height ({line}): {one} -> {two}");
    assert!((three - two - line).abs() < 3.0, "{two} -> {three}");
    // The same sentence in a narrow box wraps: taller, and never wider than the box.
    let sentence = "The quick brown fox jumps";
    let (wide_w, wide_h, _, _) = height(sentence, 0.95);
    let (narrow_w, narrow_h, nx0, nx1) = height(sentence, 0.3);
    assert!(wide_h < line * 1.2, "fits on one line in a wide box, height {wide_h}");
    assert!(narrow_h > line * 2.0, "wraps to several lines in a narrow box, height {narrow_h}");
    assert!(narrow_w <= 0.3 * W as f64 + 1.0 && narrow_w < wide_w, "wrapped lines fit the box width: {narrow_w}");
    assert!(nx0 >= 0.35 * W as f64 - 1.0 && nx1 <= 0.65 * W as f64 + 1.0, "ink {nx0}..{nx1} within the box");
    // A single word longer than the box breaks rather than overflowing.
    let (long_w, long_h, _, _) = height("Donaudampfschifffahrtsgesellschaft", 0.2);
    assert!(long_w <= 0.2 * W as f64 + 1.0 && long_h > line * 1.5, "an over-long word is broken: {long_w} x {long_h}");
    // Multi-line text is centred vertically on the transform centre.
    let f = render(&text_layer("Hxg\nHxg\nHxg", tr(0.5, 0.3, 0.9, 0.2), |s| s.size = 0.08));
    let (_, y0, _, y1) = bbox(&f, solid).unwrap();
    assert!(((y0 + y1) as f64 / 2.0 / H as f64 - 0.3).abs() < 0.03);
}

#[test]
fn background_box_outline_and_shadow() {
    let plain = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |_| {}));
    let (px0, py0, px1, py1) = bbox(&plain, ink).unwrap();
    let red = |p: [u8; 4]| p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60;
    let blue = |p: [u8; 4]| p[3] > 200 && p[2] > 200 && p[0] < 60 && p[1] < 60;

    // Background: a blue box around the text, text still red on top.
    let boxed = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |s| s.background = Some(Rgba::rgb(0, 0, 255))));
    let (bx0, by0, bx1, by1) = bbox(&boxed, blue).expect("background box drawn");
    assert!(bx0 < px0 && bx1 > px1 && by0 < py0 && by1 > py1, "box {:?} surrounds text {:?}", (bx0, by0, bx1, by1), (px0, py0, px1, py1));
    assert!(bx0 + 30 > px0 && bx1 < px1 + 30, "box hugs the text rather than filling the layer");
    assert_eq!(count(&boxed, red), count(&plain, red), "text is drawn over the box unchanged");
    let area = ((bx1 - bx0 + 1) * (by1 - by0 + 1)) as usize;
    assert!(count(&boxed, |p| p[3] > 200) > area * 9 / 10, "the box is filled");
    // Rounded corners: the very corner of the box's bounding rectangle is empty.
    assert_eq!(boxed.pixel(bx0, by0)[3], 0);

    // Outline: blue ring around red glyphs, 4 px wide.
    let outlined = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |s| s.outline = Some((Rgba::rgb(0, 0, 255), 4.0))));
    let (ox0, oy0, ox1, oy1) = bbox(&outlined, |p| p[3] > 128).unwrap();
    let (sx0, sy0, sx1, sy1) = bbox(&plain, |p| p[3] > 128).unwrap();
    for (grow, what) in [(sx0 as i64 - ox0 as i64, "left"), (ox1 as i64 - sx1 as i64, "right"), (sy0 as i64 - oy0 as i64, "top"), (oy1 as i64 - sy1 as i64, "bottom")] {
        assert!((3..=5).contains(&grow), "outline grows the ink by its width on the {what}: {grow} px");
    }
    assert!(count(&outlined, blue) > 500, "outline colour present");
    assert!(count(&outlined, red) as f64 > count(&plain, red) as f64 * 0.9, "the fill stays on top of the outline");
    // Outline width is in composition pixels: at half the canvas size it is half as wide.
    let small = render_layer(&project(), &comp(), &text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |s| s.outline = Some((Rgba::rgb(0, 0, 255), 4.0))), 0.0, W / 2, H / 2, None).unwrap();
    let small_plain = render_layer(&project(), &comp(), &text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |_| {}), 0.0, W / 2, H / 2, None).unwrap();
    let grow = bbox(&small_plain, |p| p[3] > 128).unwrap().0 as i64 - bbox(&small, |p| p[3] > 128).unwrap().0 as i64;
    assert!((1..=3).contains(&grow), "outline scales with the canvas: {grow} px at half size");

    // Shadow: dark, soft, offset down and to the right; the glyphs are unchanged.
    let shadowed = render(&text_layer("Hello", tr(0.5, 0.5, 0.8, 0.4), |s| s.shadow = true));
    let dark = |p: [u8; 4]| p[3] > 30 && p[0] < 40 && p[1] < 40 && p[2] < 40;
    let (dx0, dy0, dx1, dy1) = bbox(&shadowed, dark).expect("shadow drawn");
    assert!(dx1 > px1 && dy1 > py1, "shadow extends right and down of the text");
    assert!(dx0 >= px0.saturating_sub(4) && dy0 >= py0.saturating_sub(4), "and hardly left or up: {:?} vs {:?}", (dx0, dy0), (px0, py0));
    assert_eq!(count(&shadowed, |p| p[3] == 255 && p[0] == 255 && p[1] == 0), count(&plain, |p| p[3] == 255 && p[0] == 255 && p[1] == 0));
}

#[test]
fn emoji_mixed_scripts_and_odd_input_do_not_crash() {
    // Emoji draws in colour (not in the red text colour).
    let f = render(&text_layer("Go 🥭🔥", tr(0.5, 0.5, 0.9, 0.5), |s| s.size = 0.2));
    assert!(count(&f, solid) > 500);
    if font_family_installed("Apple Color Emoji") {
        let colourful = count(&f, |p| p[3] == 255 && (p[1] > 90 || p[2] > 90));
        assert!(colourful > 300, "colour emoji should bring colours other than the text's red, found {colourful} px");
    }
    for text in ["مرحبا بالعالم", "日本語のテキスト", "Ünïcödé ĞğİıŞş", "a\u{0301}\u{200d}\u{fe0f}", "\t\ttabs\t", "👨‍👩‍👧‍👦", "\n\n\n", " ", ""] {
        let f = render(&text_layer(text, tr(0.5, 0.5, 0.9, 0.9), |s| s.outline = Some((Rgba::BLACK, 3.0))));
        if text.trim().is_empty() {
            assert!(f.is_fully_transparent(), "{text:?} draws nothing");
        } else {
            assert!(count(&f, ink) > 50, "{text:?} draws something");
        }
    }
    // Degenerate numbers: no panic, sensible output.
    for (size, w, x, rot) in [(0.0, 0.5, 0.5, 0.0), (-1.0, 0.5, 0.5, 0.0), (f64::NAN, 0.5, 0.5, 0.0), (50.0, 0.5, 0.5, 0.0), (0.1, 0.0, 0.5, 0.0), (0.1, f64::NAN, 0.5, 0.0), (0.1, 0.5, f64::INFINITY, 0.0), (0.1, 0.5, 0.5, f64::NAN), (0.1, 1e9, 0.5, 0.0)] {
        let mut l = text_layer("Edge", tr(x, 0.5, w, 0.5), |s| s.size = size);
        l.transform.rotation_deg = rot;
        let f = render(&l);
        assert_eq!(f.rgba.len(), (W * H * 4) as usize);
    }
    // Zero-sized and tiny canvases.
    let l = text_layer("Edge", tr(0.5, 0.5, 1.0, 1.0), |_| {});
    for (w, h) in [(1u32, 1u32), (2, 300), (300, 2)] {
        let f = render_layer(&project(), &comp(), &l, 0.0, w, h, None).unwrap();
        assert_eq!((f.width, f.height), (w, h));
    }
    // A very long paragraph.
    let long = "word ".repeat(2000);
    let f = render(&text_layer(&long, tr(0.5, 0.5, 0.9, 0.9), |s| s.size = 0.03));
    assert!(count(&f, ink) > 10_000);
}

#[test]
fn rotation_turns_text_about_its_centre() {
    let flat = render(&text_layer("HHHHHH", tr(0.5, 0.5, 0.6, 0.2), |_| {}));
    let mut l = text_layer("HHHHHH", tr(0.5, 0.5, 0.6, 0.2), |_| {});
    l.transform.rotation_deg = 90.0;
    let turned = render(&l);
    let (fx0, fy0, fx1, fy1) = bbox(&flat, solid).unwrap();
    let (tx0, ty0, tx1, ty1) = bbox(&turned, solid).unwrap();
    let (fw, fh) = ((fx1 - fx0) as i64, (fy1 - fy0) as i64);
    let (tw, th) = ((tx1 - tx0) as i64, (ty1 - ty0) as i64);
    assert!((tw - fh).abs() <= 3 && (th - fw).abs() <= 3, "a quarter turn swaps width and height: {fw}x{fh} -> {tw}x{th}");
    let centre = |a: u32, b: u32| (a + b) as f64 / 2.0;
    assert!((centre(tx0, tx1) - W as f64 / 2.0).abs() < 6.0 && (centre(ty0, ty1) - H as f64 / 2.0).abs() < 6.0, "still centred on the transform centre");
    assert!((count(&turned, solid) as f64 / count(&flat, solid) as f64 - 1.0).abs() < 0.08, "same amount of ink");
    // 45 degrees: the box grows in both directions.
    l.transform.rotation_deg = 45.0;
    let (dx0, dy0, dx1, dy1) = bbox(&render(&l), solid).unwrap();
    assert!((dx1 - dx0) as i64 > fh + 20 && (dy1 - dy0) as i64 > fh + 20 && ((dx1 - dx0) as i64) < fw);
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

fn shape_layer(kind: ShapeKind, t: Transform, f: impl FnOnce(&mut Shape)) -> Layer {
    let mut s = Shape { kind, fill: Rgba::rgb(10, 120, 250), stroke: None, corner_radius: 0.0 };
    f(&mut s);
    let mut l = Layer::new("Shape", 0.0, 5.0, LayerContent::Shape(s));
    l.transform = t;
    l
}

#[test]
fn rectangle_fills_exactly_its_box_in_its_colour() {
    // Box: x 160..480, y 90..270 on a 640x360 canvas.
    let f = render(&shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.5, 0.5), |_| {}));
    assert_eq!(bbox(&f, ink), Some((160, 90, 479, 269)));
    assert_eq!(count(&f, ink), 320 * 180);
    assert_eq!(count(&f, |p| p == [10, 120, 250, 255]), 320 * 180, "every pixel is exactly the fill colour");
    assert_eq!(f.pixel(159, 180)[3], 0);
    assert_eq!(f.pixel(480, 180)[3], 0);
    // Semi-transparent fill and layer opacity multiply.
    let mut l = shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.5, 0.5), |s| s.fill = Rgba::rgba(10, 120, 250, 128));
    l.transform.opacity = 0.5;
    let p = render(&l).pixel(320, 180);
    assert!((p[3] as i32 - 64).abs() <= 1, "128/255 fill at 50% opacity is alpha 64, got {}", p[3]);
    assert!((p[0] as i32 - 10).abs() <= 3 && (p[1] as i32 - 120).abs() <= 3 && (p[2] as i32 - 250).abs() <= 5, "colour survives un-premultiplying: {p:?}");
}

#[test]
fn ellipse_corner_radius_stroke_and_line() {
    let area = |f: &Frame| count(f, solid) as f64;
    let rect = 320.0 * 180.0;
    // Ellipse: pi/4 of the box, corners empty, centre filled.
    let e = render(&shape_layer(ShapeKind::Ellipse, tr(0.5, 0.5, 0.5, 0.5), |_| {}));
    assert!((area(&e) / rect - std::f64::consts::FRAC_PI_4).abs() < 0.01, "ellipse covers pi/4 of its box: {}", area(&e) / rect);
    assert_eq!(bbox(&e, solid).map(|b| (b.0 as i64 - 160, b.1 as i64 - 90, b.2 as i64 - 479, b.3 as i64 - 269)).map(|d| d.0.abs() <= 1 && d.1.abs() <= 1 && d.2.abs() <= 1 && d.3.abs() <= 1), Some(true));
    assert_eq!(e.pixel(165, 95)[3], 0);
    assert_eq!(e.pixel(320, 180), [10, 120, 250, 255]);
    // Corner radius 0.5 of the shorter side: a stadium. Area = rect - (4 - pi) r^2.
    let r = 90.0;
    let rounded = render(&shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.5, 0.5), |s| s.corner_radius = 0.5));
    let expected = rect - (4.0 - std::f64::consts::PI) * r * r;
    assert!((area(&rounded) / expected - 1.0).abs() < 0.01, "rounded area {} vs {expected}", area(&rounded));
    assert_eq!(rounded.pixel(162, 92)[3], 0, "corner is cut");
    assert_eq!(rounded.pixel(320, 91)[3], 255, "edge midpoints are kept");
    // Out-of-range radius is clamped, not a panic.
    let clamped = render(&shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.5, 0.5), |s| s.corner_radius = 7.0));
    assert_eq!(clamped, rounded);
    // Stroke: stays inside the box, 10 px wide, in its own colour.
    let stroked = render(&shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.5, 0.5), |s| {
        s.fill = Rgba::rgba(0, 0, 0, 0);
        s.stroke = Some((Rgba::rgb(250, 200, 0), 10.0));
    }));
    assert_eq!(bbox(&stroked, ink), Some((160, 90, 479, 269)), "the stroke does not spill outside the box");
    assert_eq!(stroked.pixel(165, 180), [250, 200, 0, 255]);
    assert_eq!(stroked.pixel(175, 180)[3], 0, "hollow inside the stroke");
    let ring = 2.0 * 10.0 * (320.0 + 180.0) - 4.0 * 100.0;
    assert!((area(&stroked) / ring - 1.0).abs() < 0.02, "stroke area {} vs {ring}", area(&stroked));
    // Line: a horizontal bar through the middle; stroke sets thickness and colour.
    let line = render(&shape_layer(ShapeKind::Line, tr(0.5, 0.5, 0.5, 0.5), |s| s.stroke = Some((Rgba::rgb(250, 200, 0), 6.0))));
    assert_eq!(bbox(&line, solid), Some((160, 177, 479, 182)));
    assert_eq!(line.pixel(320, 180), [250, 200, 0, 255]);
    let fat = render(&shape_layer(ShapeKind::Line, tr(0.5, 0.5, 0.5, 0.05), |_| {}));
    assert_eq!(bbox(&fat, solid), Some((160, 171, 479, 188)), "without a stroke the line is as thick as its box");
    assert_eq!(fat.pixel(320, 180), [10, 120, 250, 255]);
    // Rotated a quarter turn the line is vertical.
    let mut l = shape_layer(ShapeKind::Line, tr(0.5, 0.5, 0.5, 0.5), |s| s.stroke = Some((Rgba::rgb(250, 200, 0), 6.0)));
    l.transform.rotation_deg = 90.0;
    let (x0, y0, x1, y1) = bbox(&render(&l), solid).unwrap();
    assert!((x1 - x0) <= 6 && (y1 - y0) >= 318, "{:?}", (x0, y0, x1, y1));
}

#[test]
fn media_layers_are_not_rendered_here() {
    let id = AssetId::new();
    for content in [LayerContent::Video(MediaClip::new(id)), LayerContent::Audio(MediaClip::new(id)), LayerContent::Image(ImageClip { asset: id, crop: Crop::default(), fit: FitMode::Fit, effects: vec![], zoom_to: 1.0 })] {
        assert!(render_layer(&project(), &comp(), &Layer::new("m", 0.0, 1.0, content), 0.0, W, H, None).is_none());
    }
}

// ---------------------------------------------------------------------------
// Progress and waveform
// ---------------------------------------------------------------------------

#[test]
fn progress_bar_fills_to_the_playhead() {
    let mut c = comp();
    let mut l = Layer::new("Bar", 0.0, 10.0, LayerContent::Progress { color: Rgba::rgb(255, 0, 0), track_color: Rgba::rgb(0, 0, 255) });
    l.transform = tr(0.5, 0.9, 0.8, 0.05); // x 64..576, y 315..333
    c.layers.push(l.clone());
    assert_eq!(c.duration(), 10.0);
    let red = |p: [u8; 4]| p[0] > 200 && p[2] < 60 && p[3] > 200;
    let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 60 && p[3] > 200;
    for (t, frac) in [(0.0, 0.0), (2.5, 0.25), (5.0, 0.5), (9.0, 0.9), (10.0, 1.0), (15.0, 1.0), (-1.0, 0.0)] {
        let f = render_layer(&project(), &c, &l, t, W, H, None).unwrap();
        let fill_end = (0..W).rev().find(|x| red(f.pixel(*x, 324))).map(|x| x + 1).unwrap_or(64);
        let expected = 64.0 + 512.0 * frac;
        assert!((fill_end as f64 - expected).abs() <= 1.5, "t={t}: fill ends at {fill_end}, expected {expected}");
        let (x0, y0, x1, y1) = bbox(&f, ink).unwrap();
        assert_eq!((x0, x1, y0, y1), (64, 575, 315, 332), "the bar occupies exactly its box at t={t}");
        if frac < 0.95 {
            assert!(blue(f.pixel(560, 324)), "track colour right of the fill at t={t}");
        }
    }
    // Rounded ends.
    let f = render_layer(&project(), &c, &l, 5.0, W, H, None).unwrap();
    assert_eq!(f.pixel(64, 315)[3], 0);
    assert_eq!(f.pixel(575, 332)[3], 0);
}

/// 20 s of peaks at 50/s: loud (0.8) for even seconds, quiet (0.1) for odd.
fn square_peaks() -> Peaks {
    let mut max = Vec::new();
    for i in 0..1000 {
        max.push(if (i / 50) % 2 == 0 { 0.8f32 } else { 0.1 });
    }
    Peaks { per_second: 50, min: max.iter().map(|v| -v).collect(), max }
}

#[test]
fn waveform_draws_the_audio_around_now() {
    let peaks = square_peaks();
    let wave = |kind: WaveformKind| {
        let mut l = Layer::new("Wave", 0.0, 20.0, LayerContent::Waveform(WaveformStyle { kind, color: Rgba::rgb(0, 200, 100) }));
        l.transform = tr(0.5, 0.5, 1.0, 0.5); // full width, y 90..270: 4 s across 640 px -> 160 px per second
        l
    };
    let col = |f: &Frame, x: u32| -> (u32, Option<(u32, u32)>) {
        let ys: Vec<u32> = (0..H).filter(|y| f.pixel(x, *y)[3] > 128).collect();
        (ys.len() as u32, ys.first().map(|a| (*a, *ys.last().unwrap())))
    };
    // t = 7.0: the window is 5..9 s. Seconds 5 and 7 are quiet, 6 and 8 loud.
    let f = render_layer(&project(), &comp(), &wave(WaveformKind::Mirror), 7.0, W, H, Some(&peaks)).unwrap();
    let tallest = |f: &Frame, from: u32, to: u32| (from..to).map(|x| col(f, x).0).max().unwrap();
    let (q1, l1, q2, l2) = (tallest(&f, 20, 140), tallest(&f, 180, 300), tallest(&f, 340, 460), tallest(&f, 500, 620));
    assert!((l1 as i32 - 144).abs() <= 2 && (l2 as i32 - 144).abs() <= 2, "loud seconds: 0.8 x 180 px = 144, got {l1} and {l2}");
    assert!((q1 as i32 - 18).abs() <= 2 && (q2 as i32 - 18).abs() <= 2, "quiet seconds: 0.1 x 180 px = 18, got {q1} and {q2}");
    // Mirror is symmetric about the centre line; Bars stand on the bottom edge.
    let x_loud = (180..300).find(|x| col(&f, *x).0 > 100).unwrap();
    let (_, span) = col(&f, x_loud);
    let (top, bottom) = span.unwrap();
    assert!(((top + bottom) as f64 / 2.0 - 179.5).abs() <= 1.0, "mirror bars are centred vertically: {top}..{bottom}");
    let bars = render_layer(&project(), &comp(), &wave(WaveformKind::Bars), 7.0, W, H, Some(&peaks)).unwrap();
    let (n, span) = col(&bars, x_loud);
    assert_eq!(span.unwrap().1, 269, "bars rise from the bottom of the box");
    assert!((n as i32 - 144).abs() <= 2);
    assert_ne!(bars, f);
    // Individual bars with gaps between them.
    let row: Vec<bool> = (0..W).map(|x| bars.pixel(x, 265)[3] > 128).collect();
    let runs = row.windows(2).filter(|w| !w[0] && w[1]).count();
    assert!((60..=66).contains(&runs), "about 64 separate bars, found {runs}");
    // Line: a thin trace, tall where loud, flat where quiet.
    let line = render_layer(&project(), &comp(), &wave(WaveformKind::Line), 7.0, W, H, Some(&peaks)).unwrap();
    let (lx0, ly0, lx1, ly1) = bbox(&line, solid).unwrap();
    assert!(lx0 <= 2 && lx1 >= W - 3 && ly0 >= 90 && ly1 <= 269, "the trace spans the box: {:?}", (lx0, ly0, lx1, ly1));
    let extent = |f: &Frame, from: u32, to: u32| {
        let ys: Vec<u32> = (from..to).flat_map(|x| (0..H).filter(move |y| f.pixel(x, *y)[3] > 128)).collect();
        ys.iter().max().unwrap() - ys.iter().min().unwrap()
    };
    assert!(extent(&line, 200, 280) > 120 && extent(&line, 40, 120) < 30, "trace swings with the level: loud {} px, quiet {} px", extent(&line, 200, 280), extent(&line, 40, 120));
    assert!(count(&line, solid) < count(&f, solid) / 2, "a trace, not filled bars: {} vs {}", count(&line, solid), count(&f, solid));
    // Symmetric about the centre line.
    let ys: Vec<u32> = (0..H).filter(|y| line.pixel(240, *y)[3] > 128).collect();
    assert!(((ys[0] + ys[ys.len() - 1]) as f64 / 2.0 - 179.5).abs() <= 1.5, "the trace is mirrored about the centre: {ys:?}");
    // It scrolls: half a second later the loud block has moved 80 px left.
    let later = render_layer(&project(), &comp(), &wave(WaveformKind::Mirror), 7.5, W, H, Some(&peaks)).unwrap();
    let first_loud = |f: &Frame| (0..W).find(|x| col(f, *x).0 > 100).unwrap() as i64;
    let moved = first_loud(&f) - first_loud(&later);
    assert!((moved - 80).abs() <= 4 || first_loud(&later) < 6, "scrolls 160 px per second: moved {moved}");
    // Without peaks, and beyond the audio, it is a flat line of minimum-height bars.
    for (p, t) in [(None, 7.0), (Some(&peaks), 500.0)] {
        let flat = render_layer(&project(), &comp(), &wave(WaveformKind::Mirror), t, W, H, p).unwrap();
        let (_, y0, _, y1) = bbox(&flat, ink).unwrap();
        assert!(y1 - y0 <= 3, "flat without audio");
        assert!(count(&flat, ink) > 300);
    }
    // Colour is the style's.
    assert_eq!(f.pixel(x_loud, 180), [0, 200, 100, 255]);
}

#[test]
fn composition_peaks_follow_the_edit() {
    // Units are 0.5 s tones at amplitude 0.5; cut units 2..4 into a silent gap.
    let (mut e, _) = coded_editor(8);
    let i = word_index(&e, "w2");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 2 }).unwrap();
    let p = composition_peaks(&e.project, e.comp().unwrap(), 50).unwrap();
    assert_eq!(p.len(), 200, "4 s at 50 peaks per second");
    assert!((p.amplitude(0.1, 0.9) - 0.5).abs() < 0.05, "tone level {}", p.amplitude(0.1, 0.9));
    assert!(p.amplitude(1.05, 1.95) < 0.01, "the gap is silent: {}", p.amplitude(1.05, 1.95));
    assert!((p.amplitude(2.1, 3.9) - 0.5).abs() < 0.05);
    // A layer's audio is added where the layer sits, with its gain.
    let music = tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25); // amplitude 1/32
    let id = add_asset(&mut e, &music);
    let mut clip = MediaClip::new(id);
    clip.audio.gain_db = 20.0; // -> 0.3125
    e.apply(Command::AddLayer { layer: Layer::new("Music", 1.0, 1.0, LayerContent::Audio(clip)) }).unwrap();
    let p = composition_peaks(&e.project, e.comp().unwrap(), 50).unwrap();
    assert!((p.amplitude(1.1, 1.9) - 0.3125).abs() < 0.03, "layer audio in the gap: {}", p.amplitude(1.1, 1.9));
    // Muting the script leaves only the layer.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { muted: true, ..AudioSettings::default() } }).unwrap();
    let p = composition_peaks(&e.project, e.comp().unwrap(), 50).unwrap();
    assert!(p.amplitude(0.1, 0.9) < 0.01 && p.amplitude(1.1, 1.9) > 0.25);
}

// ---------------------------------------------------------------------------
// Captions
// ---------------------------------------------------------------------------

fn cue(start: f64, lines: &[&str]) -> Cue {
    let mut words = Vec::new();
    let mut t = start;
    for line in lines {
        for w in line.split_whitespace() {
            words.push(CueWord { text: w.to_string(), start: t, end: t + 0.4 });
            t += 0.5;
        }
    }
    Cue { start, end: t, lines: lines.iter().map(|l| l.to_string()).collect(), words, speaker: None }
}

fn caption_comp(f: impl FnOnce(&mut CaptionSettings)) -> Composition {
    let mut c = comp();
    c.captions = CaptionSettings { enabled: true, size: 0.1, color: Rgba::rgb(0, 255, 0), highlight_color: Rgba::rgb(255, 0, 0), outline: None, y: 0.8, ..CaptionSettings::default() };
    f(&mut c.captions);
    c
}

const GREEN: fn([u8; 4]) -> bool = |p| p[3] > 200 && p[1] > 200 && p[0] < 60 && p[2] < 60;
const RED: fn([u8; 4]) -> bool = |p| p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60;

#[test]
fn captions_show_only_during_cues_at_the_configured_position() {
    let cues = vec![cue(1.0, &["Hello there"]), cue(5.0, &["Second cue"])];
    let c = caption_comp(|_| {});
    for t in [0.0, 0.99, 2.0, 4.99, 6.0, 100.0] {
        assert!(render_captions(&c, &cues, t, W, H).is_none(), "no caption at {t}s");
    }
    assert!(render_captions(&c, &[], 1.5, W, H).is_none());
    let f = render_captions(&c, &cues, 1.5, W, H).expect("cue 1 showing");
    assert_eq!((f.width, f.height), (W, H));
    let (x0, y0, x1, y1) = bbox(&f, ink).unwrap();
    assert!(((x0 + x1) as f64 / 2.0 - 320.0).abs() < 6.0, "captions are centred horizontally");
    assert!(((y0 + y1) as f64 / 2.0 / H as f64 - 0.8).abs() < 0.03, "caption block is centred on y=0.8, is at {}", (y0 + y1) as f64 / 2.0 / H as f64);
    assert_ne!(f, render_captions(&c, &cues, 5.5, W, H).unwrap(), "each cue has its own text");
    assert_eq!(count(&f, |p| p[3] == 255 && [p[0], p[1], p[2]] != [0, 255, 0]), 0, "plain captions are all in the caption colour");
    // Moving y moves the block; near the edge it stays on the canvas.
    let top = render_captions(&caption_comp(|s| s.y = 0.2), &cues, 1.5, W, H).unwrap();
    let (_, ty0, _, ty1) = bbox(&top, ink).unwrap();
    assert!(((ty0 + ty1) as f64 / 2.0 / H as f64 - 0.2).abs() < 0.03);
    let edge = render_captions(&caption_comp(|s| s.y = 1.0), &cues, 1.5, W, H).unwrap();
    let (_, _, _, ey1) = bbox(&edge, ink).unwrap();
    assert!(ey1 < H - 1, "captions at y=1.0 are kept on screen, bottom at {ey1}");
    // Size, bold and font are honoured.
    let big = render_captions(&caption_comp(|s| s.size = 0.2), &cues, 1.5, W, H).unwrap();
    let (_, by0, _, by1) = bbox(&big, ink).unwrap();
    assert!(((by1 - by0) as f64 / (y1 - y0) as f64 - 2.0).abs() < 0.25, "double size, double height");
    let light = render_captions(&caption_comp(|s| s.bold = false), &cues, 1.5, W, H).unwrap();
    assert!(count(&f, GREEN) as f64 > count(&light, GREEN) as f64 * 1.1, "bold captions have more ink");
    assert_ne!(f, render_captions(&caption_comp(|s| s.font = "Courier New".into()), &cues, 1.5, W, H).unwrap());
}

#[test]
fn caption_lines_follow_the_cue() {
    let c = caption_comp(|_| {});
    let one = render_captions(&c, &[cue(0.0, &["One two three four"])], 0.1, W, H).unwrap();
    let two = render_captions(&c, &[cue(0.0, &["One two", "three four"])], 0.1, W, H).unwrap();
    let (ox0, oy0, ox1, oy1) = bbox(&one, ink).unwrap();
    let (tx0, ty0, tx1, ty1) = bbox(&two, ink).unwrap();
    let line = 0.1 * H as f64 * 1.2;
    assert!(((ty1 - ty0) as f64 - (oy1 - oy0) as f64 - line).abs() < 4.0, "the second line adds one line height");
    assert!((tx1 - tx0) < (ox1 - ox0), "two short lines are narrower than one long one");
    assert!((((ty0 + ty1) as f64 / 2.0) / H as f64 - 0.8).abs() < 0.03, "the block stays centred on y");
    // A cue wider than the canvas wraps instead of running off screen.
    let long = render_captions(&c, &[cue(0.0, &["This caption line is far too long to fit across the canvas at this size"])], 0.1, W, H).unwrap();
    let (lx0, ly0, lx1, ly1) = bbox(&long, ink).unwrap();
    assert!(lx0 >= W / 20 - 2 && lx1 <= W - W / 20 + 2, "kept within 90% of the width: {lx0}..{lx1}");
    assert!((ly1 - ly0) as f64 > line * 1.5);
}

#[test]
fn active_word_highlight_follows_the_spoken_word() {
    let cues = vec![cue(1.0, &["alpha beta", "gamma"])]; // words at 1.0, 1.5, 2.0; cue ends 2.5
    let c = caption_comp(|s| s.highlight = CaptionHighlight::ActiveWord);
    let frames: Vec<Frame> = [1.2, 1.7, 2.2].iter().map(|t| render_captions(&c, &cues, *t, W, H).unwrap()).collect();
    let boxes: Vec<(u32, u32, u32, u32)> = frames.iter().map(|f| bbox(f, RED).expect("one word highlighted")).collect();
    // Same layout every time: only colours change.
    let all_ink: Vec<_> = frames.iter().map(|f| bbox(f, ink).unwrap()).collect();
    assert!(all_ink.windows(2).all(|w| w[0] == w[1]), "the caption does not move as the highlight advances: {all_ink:?}");
    assert!(boxes[1].0 > boxes[0].2, "second word is right of the first: {boxes:?}");
    assert!(boxes[2].1 > boxes[0].3, "third word is on the next line: {boxes:?}");
    for f in &frames {
        assert!(count(f, GREEN) > 300 && count(f, RED) > 150, "one highlighted word among base-colour words");
        assert_eq!(count(f, |p| p[3] == 255 && [p[0], p[1], p[2]] != [0, 255, 0] && [p[0], p[1], p[2]] != [255, 0, 0]), 0, "only the two caption colours appear");
    }
    // Between words (1.45: "alpha" has ended, "beta" not begun) the last spoken word stays lit.
    assert_eq!(bbox(&render_captions(&c, &cues, 1.45, W, H).unwrap(), RED), Some(boxes[0]));
    // Plain mode has no highlight at all.
    let plain = render_captions(&caption_comp(|_| {}), &cues, 1.7, W, H).unwrap();
    assert_eq!(count(&plain, RED), 0);
    assert_eq!(bbox(&plain, ink), Some(all_ink[0]), "highlighting does not change the layout");
}

#[test]
fn reveal_shows_words_up_to_the_spoken_one_with_a_stable_box() {
    let cues = vec![cue(1.0, &["alpha beta", "gamma"])];
    let c = caption_comp(|s| {
        s.highlight = CaptionHighlight::Reveal;
        s.background = Some(Rgba::rgb(0, 0, 255));
    });
    let blue: fn([u8; 4]) -> bool = |p| p[3] > 200 && p[2] > 200 && p[0] < 60 && p[1] < 60;
    let f1 = render_captions(&c, &cues, 1.2, W, H).unwrap();
    let f2 = render_captions(&c, &cues, 1.7, W, H).unwrap();
    let f3 = render_captions(&c, &cues, 2.2, W, H).unwrap();
    let (g1, g2, g3) = (bbox(&f1, GREEN).unwrap(), bbox(&f2, GREEN).unwrap(), bbox(&f3, GREEN).unwrap());
    assert!(count(&f1, GREEN) < count(&f2, GREEN) && count(&f2, GREEN) < count(&f3, GREEN), "words accumulate");
    assert_eq!(g1.0, g2.0, "already revealed words do not move");
    assert!(g2.2 > g1.2 + 20, "second word appears to the right");
    assert!(g3.3 > g2.3 + 20, "third word appears on the next line");
    // The background box is the full cue's from the start.
    assert_eq!(bbox(&f1, |p| p[3] > 0), bbox(&f3, |p| p[3] > 0), "box does not grow as words appear");
    assert!(count(&f1, blue) > count(&f3, blue), "unrevealed words leave the box showing");
    // Fully revealed equals the plain rendering.
    let plain = render_captions(&caption_comp(|s| s.background = Some(Rgba::rgb(0, 0, 255))), &cues, 2.2, W, H).unwrap();
    assert_eq!(f3, plain);
}

#[test]
fn caption_background_and_outline() {
    let cues = vec![cue(0.0, &["Boxed text"])];
    let plain = render_captions(&caption_comp(|_| {}), &cues, 0.1, W, H).unwrap();
    let (px0, py0, px1, py1) = bbox(&plain, ink).unwrap();
    let boxed = render_captions(&caption_comp(|s| s.background = Some(Rgba::rgba(0, 0, 0, 160))), &cues, 0.1, W, H).unwrap();
    let (bx0, by0, bx1, by1) = bbox(&boxed, ink).unwrap();
    assert!(bx0 < px0 && by0 < py0 && bx1 > px1 && by1 > py1, "the box surrounds the text");
    let p = boxed.pixel((bx0 + px0) / 2, (by0 + by1) / 2);
    assert!((p[3] as i32 - 160).abs() <= 2 && p[0] < 4, "semi-transparent black box, got {p:?}");
    assert!(count(&boxed, GREEN) >= count(&plain, GREEN), "the text is drawn over the box");
    let outlined = render_captions(&caption_comp(|s| s.outline = Some((Rgba::BLACK, 3.0))), &cues, 0.1, W, H).unwrap();
    let (ox0, _, ox1, _) = bbox(&outlined, |p| p[3] > 128).unwrap();
    let (sx0, _, sx1, _) = bbox(&plain, |p| p[3] > 128).unwrap();
    assert!((2..=4).contains(&(sx0 as i64 - ox0 as i64)) && (2..=4).contains(&(ox1 as i64 - sx1 as i64)), "3 px outline");
    assert!(count(&outlined, |p| p[3] > 200 && p[0] < 30 && p[1] < 30 && p[2] < 30) > 400, "black outline pixels");
}

#[test]
fn caption_cues_come_from_the_script_and_uppercase_is_applied_by_core() {
    let (mut e, _) = coded_editor(6);
    e.apply(Command::SetCaptions { captions: CaptionSettings { enabled: true, uppercase: true, max_chars_per_line: 8, max_lines: 1, ..CaptionSettings::default() } }).unwrap();
    let cues = caption_cues(e.comp().unwrap());
    assert!(cues.len() >= 2, "{cues:?}");
    assert_eq!(cues[0].words[0].text, "W0");
    assert_eq!(cues[0].start, 0.0);
    let f = render_captions(e.comp().unwrap(), &cues, 0.2, 320, 180).expect("first cue shows from the first word");
    assert!(count(&f, ink) > 100);
    // Upper- and lower-case cues render differently.
    let mut lower = e.comp().unwrap().clone();
    lower.captions.uppercase = false;
    let lower_cues = caption_cues(&lower);
    assert_eq!(lower_cues[0].words[0].text, "w0");
    assert_ne!(f, render_captions(&lower, &lower_cues, 0.2, 320, 180).unwrap());
}

// ---------------------------------------------------------------------------
// Preview compositor
// ---------------------------------------------------------------------------

#[test]
fn overlay_composites_visible_layers_in_track_order_with_captions_on_top() {
    let mut c = caption_comp(|s| s.y = 0.5);
    let rect = |color: Rgba, track: i32, start: f64, dur: f64| {
        let mut l = Layer::new("r", start, dur, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: color, stroke: None, corner_radius: 0.0 }));
        l.transform = tr(0.5, 0.5, 0.5, 0.5);
        l.track = track;
        l
    };
    c.layers.push(rect(Rgba::rgb(255, 255, 255), 3, 0.0, 10.0)); // white, on top
    c.layers.push(rect(Rgba::rgb(0, 0, 255), 1, 0.0, 10.0)); // blue, beneath
    let mut hidden = rect(Rgba::rgb(255, 255, 0), 9, 0.0, 10.0);
    hidden.hidden = true;
    hidden.transform = tr(0.5, 0.5, 1.0, 1.0);
    c.layers.push(hidden);
    let mut later = rect(Rgba::rgb(255, 0, 255), 5, 4.0, 2.0);
    later.transform = tr(0.1, 0.1, 0.1, 0.1);
    c.layers.push(later);
    let cues = vec![cue(1.0, &["Hi"])];
    let p = project();

    let f = render_overlay(&p, &c, &cues, 0.5, W, H);
    assert_eq!(f.pixel(320, 100), [255, 255, 255, 255], "higher track wins regardless of insertion order");
    assert_eq!(f.pixel(10, 10)[3], 0, "transparent where nothing is drawn");
    assert_eq!(count(&f, |p| p[0] == 255 && p[1] == 255 && p[2] == 0), 0, "hidden layers are not drawn");
    assert_eq!(count(&f, GREEN), 0, "no caption before its cue");
    assert_eq!(f.pixel(64, 36)[3], 0, "a layer is not drawn before its start");

    let f = render_overlay(&p, &c, &cues, 1.2, W, H);
    assert!(count(&f, GREEN) > 100, "captions are drawn on top of every layer");
    let f = render_overlay(&p, &c, &cues, 4.0, W, H);
    assert_eq!(f.pixel(64, 36), [255, 0, 255, 255], "layer shows from its start time (inclusive)");
    let f = render_overlay(&p, &c, &cues, 6.0, W, H);
    assert_eq!(f.pixel(64, 36)[3], 0, "and is gone at its end time (exclusive)");
    let f = render_overlay(&p, &c, &cues, 10.0, W, H);
    assert!(f.is_fully_transparent(), "nothing after everything has ended");
    // Captions disabled: not drawn even when a cue is up.
    c.captions.enabled = false;
    assert_eq!(count(&render_overlay(&p, &c, &cues, 1.2, W, H), GREEN), 0);
    // Media layers are skipped, sizes are honoured, zero size does not panic.
    c.layers.push(Layer::new("v", 0.0, 10.0, LayerContent::Video(MediaClip::new(AssetId::new()))));
    let small = render_overlay(&p, &c, &cues, 0.5, 64, 36);
    assert_eq!((small.width, small.height, small.rgba.len()), (64, 36, 64 * 36 * 4));
    assert_eq!(small.pixel(32, 18), [255, 255, 255, 255]);
    let zero = render_overlay(&p, &c, &cues, 0.5, 0, 0);
    assert_eq!(zero.rgba.len(), 0);
}

#[test]
fn overlay_applies_fades() {
    let mut c = comp();
    let mut l = Layer::new("r", 2.0, 4.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: Rgba::rgb(255, 255, 255), stroke: None, corner_radius: 0.0 }));
    l.fade_in = 1.0;
    l.fade_out = 2.0;
    c.layers.push(l);
    let alpha = |t: f64| render_overlay(&project(), &c, &[], t, 64, 36).pixel(32, 18)[3] as i32;
    assert!((alpha(2.5) - 128).abs() <= 2, "halfway through the fade in: {}", alpha(2.5));
    assert_eq!(alpha(3.5), 255);
    assert!((alpha(5.0) - 128).abs() <= 2, "halfway through the fade out: {}", alpha(5.0));
    assert!(alpha(5.9) < 16);
    assert_eq!(alpha(1.9), 0);
}

// ---------------------------------------------------------------------------
// Export overlays on disk
// ---------------------------------------------------------------------------

fn read_png(path: &std::path::Path) -> Frame {
    mango_media::frames::frame_at(path, 0.0, 10_000, 10_000).unwrap()
}

fn parse_ffconcat(path: &std::path::Path) -> Vec<(std::path::PathBuf, Option<f64>)> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.starts_with("ffconcat version 1.0\n"), "{text}");
    let dir = path.parent().unwrap();
    let mut out: Vec<(std::path::PathBuf, Option<f64>)> = Vec::new();
    for line in text.lines().skip(1) {
        if let Some(name) = line.strip_prefix("file '") {
            out.push((dir.join(name.trim_end_matches('\'')), None));
        } else if let Some(d) = line.strip_prefix("duration ") {
            out.last_mut().unwrap().1 = Some(d.parse().unwrap());
        }
    }
    out
}

#[test]
fn export_overlays_static_layers_are_single_pngs() {
    let (mut e, _) = coded_editor(10);
    let mut title = text_layer("Title", tr(0.5, 0.2, 0.8, 0.3), |_| {});
    title.start = 1.0;
    title.duration = 2.5;
    title.track = 4;
    title.fade_in = 0.3;
    title.fade_out = 0.2;
    title.animation_in = Animation::SlideUp;
    title.animation_out = Animation::Fade;
    e.apply(Command::AddLayer { layer: title.clone() }).unwrap();
    let mut shape = shape_layer(ShapeKind::Ellipse, tr(0.5, 0.7, 0.2, 0.2), |_| {});
    shape.track = 2;
    e.apply(Command::AddLayer { layer: shape }).unwrap();
    let mut hidden = shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 1.0, 1.0), |_| {});
    hidden.hidden = true;
    e.apply(Command::AddLayer { layer: hidden }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = ExportSettings::for_composition(e.comp().unwrap());
    s.width = 320;
    s.height = 180;
    let overlays = overlays_for_export(&e.project, e.comp().unwrap(), &s, dir.path()).unwrap();
    assert_eq!(overlays.len(), 2, "one per visible generated layer; hidden ones are skipped");
    let o = &overlays[0];
    assert!(!o.sequence);
    assert_eq!((o.start, o.end, o.track, o.fade_in, o.fade_out), (1.0, 3.5, 4, 0.3, 0.2));
    assert_eq!((o.animation_in, o.animation_out), (Animation::SlideUp, Animation::Fade));
    assert_eq!(o.path.extension().unwrap(), "png");
    assert!(o.path.starts_with(dir.path()));
    // The PNG is the full canvas and is exactly what the preview draws.
    let on_disk = read_png(&o.path);
    assert_eq!((on_disk.width, on_disk.height), (320, 180));
    let expected = render_layer(&e.project, e.comp().unwrap(), &title, 1.0, 320, 180, None).unwrap();
    assert_eq!(on_disk, expected, "export and preview share one rasteriser");
    assert_eq!((overlays[1].track, overlays[1].start, overlays[1].end, overlays[1].sequence), (2, 0.0, 5.0, false));
    assert_ne!(overlays[0].path, overlays[1].path);
}

#[test]
fn export_overlays_animated_layers_are_frame_sequences() {
    let (mut e, _) = coded_editor(10); // 5 s
    let mut bar = Layer::new("Bar", 1.0, 2.0, LayerContent::Progress { color: Rgba::rgb(255, 0, 0), track_color: Rgba::rgb(0, 0, 255) });
    bar.transform = tr(0.5, 0.9, 1.0, 0.1);
    bar.track = 7;
    e.apply(Command::AddLayer { layer: bar.clone() }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = ExportSettings::for_composition(e.comp().unwrap());
    s.width = 320;
    s.height = 180;
    s.fps = 30.0;
    let overlays = overlays_for_export(&e.project, e.comp().unwrap(), &s, dir.path()).unwrap();
    assert_eq!(overlays.len(), 1);
    let o = &overlays[0];
    assert!(o.sequence);
    assert_eq!((o.start, o.end, o.track), (1.0, 3.0, 7));
    let entries = parse_ffconcat(&o.path);
    // The last file is listed twice (once with a duration, once to terminate).
    let (last, timed) = entries.split_last().unwrap();
    assert_eq!(last.1, None);
    assert_eq!(last.0, timed.last().unwrap().0);
    let total: f64 = timed.iter().map(|e| e.1.unwrap()).sum();
    assert!((total - 2.0).abs() < 1e-3, "durations add up to the layer's duration: {total}");
    assert!(timed.len() >= 55 && timed.len() <= 60, "about one image per frame (identical neighbours merge): {}", timed.len());
    assert!(timed.iter().all(|e| e.0.is_file() && e.1.unwrap() >= 1.0 / 30.0 - 1e-4));
    // Frame j is the layer at output frame 30 + j: check the first, middle and last against the rasteriser.
    let mut elapsed = 0.0;
    for (i, (path, d)) in timed.iter().enumerate() {
        if i == 0 || i == timed.len() / 2 || i == timed.len() - 1 {
            let t = 1.0 + elapsed;
            let expected = render_layer(&e.project, e.comp().unwrap(), &bar, t, 320, 180, None).unwrap();
            assert_eq!(read_png(path), expected, "sequence image {i} is the layer at {t}s");
        }
        elapsed += d.unwrap();
    }
    // With a range, frames outside it are blank (never shown), frames inside are real.
    let dir2 = tempfile::tempdir().unwrap();
    s.range = Some((2.0, 2.5));
    let ranged = overlays_for_export(&e.project, e.comp().unwrap(), &s, dir2.path()).unwrap();
    let entries = parse_ffconcat(&ranged[0].path);
    let timed = &entries[..entries.len() - 1];
    assert!((timed.iter().map(|e| e.1.unwrap()).sum::<f64>() - 2.0).abs() < 1e-3, "timing is unchanged by the range");
    assert!(read_png(&timed[0].0).is_fully_transparent(), "frames before the range are blank");
    assert!(timed.len() < 30, "only the range is rendered: {} images", timed.len());
    let mut elapsed = 0.0;
    let mut checked = 0;
    for (path, d) in timed {
        let t = 1.0 + elapsed;
        if (2.0..2.5).contains(&t) {
            assert_eq!(read_png(path), render_layer(&e.project, e.comp().unwrap(), &bar, t, 320, 180, None).unwrap(), "in-range frame at {t}s");
            checked += 1;
        }
        elapsed += d.unwrap();
    }
    assert!(checked >= 14, "the frames inside the range are all real: {checked}");
}

#[test]
fn export_overlays_captions_are_a_timed_sequence_above_all_layers() {
    let (mut e, _) = coded_editor(10);
    let i = word_index(&e, "w4");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 3 }).unwrap(); // pause 2.0..3.5
    let mut shape = shape_layer(ShapeKind::Rectangle, tr(0.5, 0.5, 0.2, 0.2), |_| {});
    shape.track = 12;
    e.apply(Command::AddLayer { layer: shape }).unwrap();
    for highlight in [CaptionHighlight::None, CaptionHighlight::ActiveWord, CaptionHighlight::Reveal] {
        e.apply(Command::SetCaptions { captions: CaptionSettings { enabled: true, highlight, max_chars_per_line: 12, max_lines: 1, ..CaptionSettings::default() } }).unwrap();
        let comp = e.comp().unwrap();
        let cues = caption_cues(comp);
        let dir = tempfile::tempdir().unwrap();
        let mut s = ExportSettings::for_composition(comp);
        s.width = 320;
        s.height = 180;
        let overlays = overlays_for_export(&e.project, comp, &s, dir.path()).unwrap();
        assert_eq!(overlays.len(), 2);
        let cap = &overlays[1];
        assert!(cap.sequence);
        assert_eq!(cap.track, 13, "captions sit above the highest layer track");
        assert_eq!((cap.start, cap.end), (0.0, comp.duration()));
        assert_eq!((cap.fade_in, cap.fade_out, cap.animation_in, cap.animation_out), (0.0, 0.0, Animation::None, Animation::None));
        let entries = parse_ffconcat(&cap.path);
        let timed = &entries[..entries.len() - 1];
        let total: f64 = timed.iter().map(|e| e.1.unwrap()).sum();
        assert!((total - comp.duration()).abs() < 1e-3, "{highlight:?}: the sequence covers the whole composition: {total}");
        // Walk the timeline: each image must be what the rasteriser draws at that time (or blank between cues).
        let mut t = 0.0;
        let mut images = 0;
        let mut blanks = 0;
        for (path, d) in timed {
            let d = d.unwrap();
            let mid = t + d / 2.0;
            let on_disk = read_png(path);
            assert_eq!((on_disk.width, on_disk.height), (320, 180));
            match render_captions(comp, &cues, mid, 320, 180) {
                Some(expected) => {
                    assert_eq!(on_disk, expected, "{highlight:?}: caption image at {mid:.3}s");
                    images += 1;
                }
                None => {
                    assert!(on_disk.is_fully_transparent(), "{highlight:?}: blank between cues at {mid:.3}s");
                    blanks += 1;
                }
            }
            // Boundaries are on the frame grid.
            assert!(((t * 30.0).round() - t * 30.0).abs() < 1e-3, "{highlight:?}: boundary {t} is on a frame");
            t += d;
        }
        assert!(blanks >= 1, "{highlight:?}: the pause has a blank stretch");
        let words: usize = cues.iter().map(|c| c.words.len()).sum();
        match highlight {
            CaptionHighlight::None => assert_eq!(images, cues.len(), "one image per cue"),
            _ => assert_eq!(images, words, "{highlight:?}: one image per word"),
        }
    }
    // Captions off: no caption overlay. Captions on but no words: none either.
    e.apply(Command::SetCaptions { captions: CaptionSettings::default() }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let s = ExportSettings::for_composition(e.comp().unwrap());
    assert_eq!(overlays_for_export(&e.project, e.comp().unwrap(), &s, dir.path()).unwrap().len(), 1);
}

#[test]
fn export_overlays_reject_bad_settings_and_create_the_directory() {
    let (mut e, _) = coded_editor(4);
    e.apply(Command::AddLayer { layer: text_layer("T", tr(0.5, 0.5, 1.0, 1.0), |_| {}) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = ExportSettings::for_composition(e.comp().unwrap());
    let nested = dir.path().join("a/b/c");
    assert_eq!(overlays_for_export(&e.project, e.comp().unwrap(), &s, &nested).unwrap().len(), 1);
    assert!(nested.is_dir());
    s.width = 0;
    assert!(overlays_for_export(&e.project, e.comp().unwrap(), &s, dir.path()).is_err());
    s.width = 64;
    s.fps = 0.0;
    assert!(overlays_for_export(&e.project, e.comp().unwrap(), &s, dir.path()).is_err());
}

#[test]
fn overlay_rendering_can_be_cancelled() {
    let (mut e, _) = coded_editor(124);
    let mut bar = Layer::new("Bar", 0.0, 62.0, LayerContent::Progress { color: Rgba::rgb(255, 0, 0), track_color: Rgba::rgb(0, 0, 255) });
    bar.transform = tr(0.5, 0.9, 1.0, 0.1);
    e.apply(Command::AddLayer { layer: bar }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = ExportSettings::for_composition(e.comp().unwrap());
    s.width = 1920;
    s.height = 1080;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let started = std::time::Instant::now();
    let err = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        overlays_for_export_cancellable(&e.project, e.comp().unwrap(), &s, dir.path(), Some(&cancel)).unwrap_err()
    });
    assert!(mango_media::is_cancelled(&err), "{err:#}");
    assert!(started.elapsed().as_secs_f64() < 2.0, "1860 full-HD frames were not all rendered: {:?}", started.elapsed());
}
