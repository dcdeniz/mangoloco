//! The preview canvas: the composition frame at the playhead, with direct
//! manipulation of the selected layer.

use crate::model::*;
use crate::theme;
use crate::workspace::*;
use gpui_kit::component::*;
use gpui_kit::*;
use mango_core::{Layer, LayerId, Secs, Transform};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CanvasDrag {
    Move { id: LayerId, grab_dx: f64, grab_dy: f64 },
    Resize { id: LayerId },
}

#[derive(Default)]
pub struct CanvasState {
    /// Where the composition frame was drawn, in window coordinates.
    pub frame_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub drag: Option<CanvasDrag>,
    /// Decoded picture for the script track at the playhead.
    pub picture: Option<Arc<RenderImage>>,
    /// Rasterised titles, shapes and captions at the playhead.
    pub overlay: Option<Arc<RenderImage>>,
    /// Pictures for video/image layers, drawn in their boxes.
    pub layer_pictures: Vec<(LayerId, Arc<RenderImage>)>,
}

/// The largest rectangle of aspect `aw:ah` centred inside `outer`.
pub fn fit_rect(outer: Bounds<Pixels>, aw: f64, ah: f64) -> Bounds<Pixels> {
    let (ow, oh) = (f64::from(outer.size.width).max(1.0), f64::from(outer.size.height).max(1.0));
    let scale = (ow / aw.max(1.0)).min(oh / ah.max(1.0));
    let (w, h) = (aw * scale, ah * scale);
    Bounds::new(point(outer.origin.x + px(((ow - w) / 2.0) as f32), outer.origin.y + px(((oh - h) / 2.0) as f32)), size(px(w as f32), px(h as f32)))
}

/// The smallest rectangle of aspect `aw:ah` centred on `outer` that covers it.
pub fn cover_rect(outer: Bounds<Pixels>, aw: f64, ah: f64) -> Bounds<Pixels> {
    let (ow, oh) = (f64::from(outer.size.width).max(1.0), f64::from(outer.size.height).max(1.0));
    let scale = (ow / aw.max(1.0)).max(oh / ah.max(1.0));
    let (w, h) = (aw * scale, ah * scale);
    Bounds::new(point(outer.origin.x + px(((ow - w) / 2.0) as f32), outer.origin.y + px(((oh - h) / 2.0) as f32)), size(px(w as f32), px(h as f32)))
}

/// A layer's box inside the frame, from its normalised transform.
pub fn layer_rect(frame: Bounds<Pixels>, tr: &Transform) -> Bounds<Pixels> {
    let (fw, fh) = (f64::from(frame.size.width), f64::from(frame.size.height));
    let (w, h) = (tr.w * fw, tr.h * fh);
    Bounds::new(
        point(frame.origin.x + px((tr.x * fw - w / 2.0) as f32), frame.origin.y + px((tr.y * fh - h / 2.0) as f32)),
        size(px(w.max(1.0) as f32), px(h.max(1.0) as f32)),
    )
}

/// Visual layers showing at `t`, topmost first.
pub fn layers_at(model: &AppModel, t: Secs) -> Vec<&Layer> {
    let Some(c) = model.comp() else { return Vec::new() };
    let mut v: Vec<&Layer> = c.layers.iter().filter(|l| l.is_visual() && !l.hidden && t >= l.start && t < l.end()).collect();
    v.sort_by_key(|l| std::cmp::Reverse(l.track));
    v
}

const HANDLE: f32 = 12.0;

impl Workspace {
    /// Whether an overlay, a script picture, and how many layer pictures are loaded.
    pub fn canvas_debug(&self) -> (bool, bool, usize) {
        (self.canvas.overlay.is_some(), self.canvas.picture.is_some(), self.canvas.layer_pictures.len())
    }

    fn canvas_norm(&self, pos: Point<Pixels>) -> (f64, f64) {
        let f = self.canvas.frame_bounds.get();
        let (fw, fh) = (f64::from(f.size.width).max(1.0), f64::from(f.size.height).max(1.0));
        (f64::from(pos.x - f.origin.x) / fw, f64::from(pos.y - f.origin.y) / fh)
    }

    fn canvas_mouse_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        let frame = self.canvas.frame_bounds.get();
        let (nx, ny) = self.canvas_norm(ev.position);
        // Resize handle of the already-selected layer wins.
        if let Some(l) = self.model.layer() {
            let r = layer_rect(frame, &l.transform);
            let corner = point(r.origin.x + r.size.width, r.origin.y + r.size.height);
            if (ev.position.x - corner.x).abs() <= px(HANDLE) && (ev.position.y - corner.y).abs() <= px(HANDLE) && !l.locked {
                self.canvas.drag = Some(CanvasDrag::Resize { id: l.id });
                cx.notify();
                return;
            }
        }
        let hit = layers_at(&self.model, self.model.playhead)
            .into_iter()
            .find(|l| layer_rect(frame, &l.transform).contains(&ev.position))
            .map(|l| (l.id, l.transform.x, l.transform.y, l.locked));
        match hit {
            Some((id, x, y, locked)) => {
                self.model.selected_layer = Some(id);
                self.model.inspector = InspectorTab::Layer;
                if !locked {
                    self.canvas.drag = Some(CanvasDrag::Move { id, grab_dx: nx - x, grab_dy: ny - y });
                }
            }
            None => {
                self.model.selected_layer = None;
            }
        }
        cx.notify();
    }

    fn canvas_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(drag) = self.canvas.drag else { return };
        let (nx, ny) = self.canvas_norm(ev.position);
        // Snap the centre to the canvas centre lines.
        let snap = |v: f64| if (v - 0.5).abs() < 0.012 { 0.5 } else { v };
        match drag {
            CanvasDrag::Move { id, grab_dx, grab_dy } => {
                let (x, y) = (snap(nx - grab_dx), snap(ny - grab_dy));
                self.model.live_edit_layer(id, |l| {
                    l.transform.x = x.clamp(-0.5, 1.5);
                    l.transform.y = y.clamp(-0.5, 1.5);
                });
            }
            CanvasDrag::Resize { id } => {
                let keep_aspect = !ev.modifiers.shift;
                self.model.live_edit_layer(id, |l| {
                    // The top-left corner stays put; the centre moves with the size.
                    let (left, top) = (l.transform.x - l.transform.w / 2.0, l.transform.y - l.transform.h / 2.0);
                    let mut w = (nx - left).clamp(0.02, 3.0);
                    let mut h = (ny - top).clamp(0.02, 3.0);
                    if keep_aspect && l.transform.w > 0.0 && l.transform.h > 0.0 {
                        let ar = l.transform.w / l.transform.h;
                        if w / h > ar {
                            h = w / ar;
                        } else {
                            w = h * ar;
                        }
                    }
                    l.transform.w = w;
                    l.transform.h = h;
                    l.transform.x = left + w / 2.0;
                    l.transform.y = top + h / 2.0;
                });
            }
        }
        cx.notify();
    }

    fn canvas_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.canvas.drag.take().is_some() {
            self.model.commit_live_edit();
            cx.notify();
        }
    }

    pub(crate) fn render_canvas(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let model = &self.model;
        let (cw, ch, bg) = model.comp().map(|c| (c.width as f64, c.height as f64, theme::from_core(c.background))).unwrap_or((16.0, 9.0, gpui_kit::black()));
        let selected = model.layer().map(|l| (l.transform, l.locked));
        // Outlines for media layers whose picture is not available.
        let boxes: Vec<(Transform, SharedString, bool)> = layers_at(model, model.playhead)
            .into_iter()
            .rev()
            .filter(|l| matches!(l.content, mango_core::LayerContent::Video(_) | mango_core::LayerContent::Image(_)))
            .map(|l| (l.transform, l.name.clone().into(), self.canvas.layer_pictures.iter().any(|(id, _)| *id == l.id)))
            .collect();
        let layer_pics: Vec<(Transform, Arc<RenderImage>)> = layers_at(model, model.playhead)
            .into_iter()
            .rev()
            .filter_map(|l| self.canvas.layer_pictures.iter().find(|(id, _)| *id == l.id).map(|(_, p)| (l.transform, p.clone())))
            .collect();
        // Where the scene at the playhead puts the script's own video.
        let placement = model.comp().map(|c| {
            let scene = c.scene_at_time(model.playhead);
            c.scenes.get(scene).and_then(|s| s.script_video.clone()).unwrap_or_else(|| c.script_video.clone())
        });
        let picture = self.canvas.picture.clone().filter(|_| placement.as_ref().is_none_or(|p| !p.hidden));
        let (place_tr, place_fit) = placement.map(|p| (p.transform, p.fit)).unwrap_or((Transform::default(), mango_core::FitMode::Fit));
        let overlay = self.canvas.overlay.clone();
        let cell = self.canvas.frame_bounds.clone();
        let border = t.border;
        let muted = t.muted_foreground;

        div()
            .id("canvas").test_support()
            .w_full()
            .flex_1()
            .min_h(px(180.))
            .p_3()
            .bg(t.muted.opacity(0.4))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| this.canvas_mouse_down(ev, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.canvas_mouse_move(ev, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.canvas_mouse_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.canvas_mouse_up(cx)))
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        let frame = fit_rect(bounds, cw, ch);
                        cell.set(frame);
                        frame
                    },
                    move |_bounds, frame, window, _cx| {
                        window.paint_quad(fill(frame, bg));
                        window.with_content_mask(Some(ContentMask { bounds: frame }), |window| {
                            if let Some(p) = &picture {
                                let slot = layer_rect(frame, &place_tr);
                                let sz = p.size(0);
                                let (pw, ph) = (sz.width.0 as f64, sz.height.0 as f64);
                                let dest = match place_fit {
                                    mango_core::FitMode::Fit => fit_rect(slot, pw, ph),
                                    mango_core::FitMode::Fill => cover_rect(slot, pw, ph),
                                    mango_core::FitMode::Stretch => slot,
                                };
                                let clip = slot.intersect(&frame);
                                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                                    let _ = window.paint_image(dest, dest, Corners::default(), p.clone(), 0, false);
                                });
                            }
                            for (tr, p) in &layer_pics {
                                let _ = window.paint_image(layer_rect(frame, tr), layer_rect(frame, tr), Corners::default(), p.clone(), 0, false);
                            }
                            for (tr, _name, has_picture) in &boxes {
                                if !has_picture {
                                    window.paint_quad(quad(layer_rect(frame, tr), px(2.), muted.opacity(0.25), px(1.), muted, BorderStyle::Dashed));
                                }
                            }
                            if let Some(o) = &overlay {
                                let _ = window.paint_image(frame, frame, Corners::default(), o.clone(), 0, false);
                            }
                        });
                        window.paint_quad(outline(frame, border, BorderStyle::Solid));
                        if let Some((tr, locked)) = selected {
                            let r = layer_rect(frame, &tr);
                            let c = if locked { muted } else { theme::lagoon() };
                            window.paint_quad(outline(r, c, BorderStyle::Solid));
                            if !locked {
                                let h = Bounds::new(point(r.origin.x + r.size.width - px(5.), r.origin.y + r.size.height - px(5.)), size(px(10.), px(10.)));
                                window.paint_quad(quad(h, px(2.), gpui_kit::white(), px(1.5), c, BorderStyle::Solid));
                            }
                        }
                    },
                )
                .size_full(),
            )
    }

    pub(crate) fn render_right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let canvas = self.render_canvas(window, cx).into_any_element();
        let inspector = self.render_inspector(window, cx).into_any_element();
        v_flex().id("right-pane").w(px(440.)).flex_none().h_full().border_l_1().border_color(border).child(canvas).child(inspector)
    }
}
