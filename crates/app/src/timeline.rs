//! The timeline: ruler, script track with word bar, layer lanes, playhead.

use crate::model::*;
use crate::theme;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::*;
use mango_core::{LayerContent, LayerId, Secs};
use std::cell::Cell;
use std::rc::Rc;

pub const RULER_H: f32 = 22.0;
pub const SCRIPT_H: f32 = 64.0;
pub const LANE_H: f32 = 26.0;
/// Pixels from a layer edge within which a drag trims instead of moves.
pub const EDGE_GRAB: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Seek,
    MoveLayer { id: LayerId, grab_offset: Secs },
    TrimLeft { id: LayerId, last_t: Secs },
    TrimRight { id: LayerId, last_t: Secs },
}

/// What is under a point of the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Ruler,
    Script,
    Layer { id: LayerId, edge: Edge },
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Body,
    Right,
}

#[derive(Default)]
pub struct TimelineState {
    /// Bounds of the drawing area in window coordinates, updated every paint.
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    pub drag: Option<Drag>,
}

/// One row of layers. Rows are assigned from `track`, top row = highest.
pub fn lanes(model: &AppModel) -> Vec<i32> {
    let mut tracks: Vec<i32> = model.comp().map(|c| c.layers.iter().map(|l| l.track).collect()).unwrap_or_default();
    tracks.sort_unstable_by(|a, b| b.cmp(a));
    tracks.dedup();
    tracks
}

impl TimelineState {
    pub fn width(&self) -> f64 {
        f64::from(self.bounds.get().size.width).max(100.0)
    }

    pub fn x_to_time(&self, model: &AppModel, x: Pixels) -> Secs {
        let local = f64::from(x - self.bounds.get().origin.x);
        ((local + model.timeline_scroll) / model.px_per_sec).max(0.0)
    }

    pub fn hit(&self, model: &AppModel, pos: Point<Pixels>) -> Hit {
        let b = self.bounds.get();
        let y = f32::from(pos.y - b.origin.y);
        let t = self.x_to_time(model, pos.x);
        if y < RULER_H {
            return Hit::Ruler;
        }
        let lanes = lanes(model);
        let lanes_h = lanes.len() as f32 * LANE_H;
        if y < RULER_H + lanes_h {
            let row = ((y - RULER_H) / LANE_H) as usize;
            let Some(track) = lanes.get(row) else { return Hit::Empty };
            let Some(c) = model.comp() else { return Hit::Empty };
            let grab = EDGE_GRAB / model.px_per_sec;
            // Topmost (last added) layer on this track under the point.
            for l in c.layers.iter().rev().filter(|l| l.track == *track) {
                if t >= l.start - grab && t <= l.end() + grab {
                    let edge = if (t - l.start).abs() <= grab {
                        Edge::Left
                    } else if (t - l.end()).abs() <= grab {
                        Edge::Right
                    } else {
                        Edge::Body
                    };
                    return Hit::Layer { id: l.id, edge };
                }
            }
            return Hit::Empty;
        }
        if y < RULER_H + lanes_h + SCRIPT_H { Hit::Script } else { Hit::Empty }
    }
}

/// Scroll offset that keeps the playhead visible, paging when it leaves.
pub fn scroll_for_playhead(playhead: Secs, px_per_sec: f64, scroll: f64, width: f64) -> f64 {
    let x = playhead * px_per_sec - scroll;
    if x > width * 0.9 {
        (playhead * px_per_sec - width * 0.1).max(0.0)
    } else if x < 0.0 {
        (playhead * px_per_sec - width * 0.1).max(0.0)
    } else {
        scroll
    }
}

/// Spacing of ruler labels, in seconds, for a zoom level.
pub fn ruler_step(px_per_sec: f64) -> f64 {
    const STEPS: [f64; 14] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];
    STEPS.into_iter().find(|s| s * px_per_sec >= 70.0).unwrap_or(3600.0)
}

impl Workspace {
    fn timeline_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.timeline_focus.focus(window, cx);
        let t = self.timeline.x_to_time(&self.model, ev.position.x);
        match self.timeline.hit(&self.model, ev.position) {
            Hit::Layer { id, edge } => {
                self.model.selected_layer = Some(id);
                self.model.inspector = InspectorTab::Layer;
                let start = self.model.comp().and_then(|c| c.layer(id)).map(|l| l.start).unwrap_or(0.0);
                self.timeline.drag = Some(match edge {
                    Edge::Left => Drag::TrimLeft { id, last_t: t },
                    Edge::Right => Drag::TrimRight { id, last_t: t },
                    Edge::Body => Drag::MoveLayer { id, grab_offset: t - start },
                });
            }
            Hit::Ruler | Hit::Script | Hit::Empty => {
                self.model.selected_layer = None;
                self.model.seek(t);
                if let Some(tok) = self.model.playing_token() {
                    self.model.select_range(tok..tok);
                    self.reveal_token(tok);
                }
                self.timeline.drag = Some(Drag::Seek);
            }
        }
        cx.notify();
    }

    fn timeline_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if ev.pressed_button != Some(MouseButton::Left) {
            if self.timeline.drag.take().is_some() {
                cx.notify();
            }
            return;
        }
        let Some(drag) = self.timeline.drag else { return };
        let t = self.timeline.x_to_time(&self.model, ev.position.x);
        match drag {
            Drag::Seek => self.model.seek(t),
            Drag::MoveLayer { id, grab_offset } => {
                let target = self.snap(t - grab_offset, Some(id));
                self.model.live_edit_layer(id, |l| l.start = target.max(0.0));
            }
            Drag::TrimLeft { id, last_t } => {
                let d = t - last_t;
                if d.abs() > 1e-4 {
                    self.model.live_trim(id, true, d);
                    self.timeline.drag = Some(Drag::TrimLeft { id, last_t: t });
                }
            }
            Drag::TrimRight { id, last_t } => {
                let d = t - last_t;
                if d.abs() > 1e-4 {
                    self.model.live_trim(id, false, d);
                    self.timeline.drag = Some(Drag::TrimRight { id, last_t: t });
                }
            }
        }
        cx.notify();
    }

    fn timeline_mouse_up(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.timeline.drag.take() {
            // Layer drags mutate the layer live; commit them as one undo step.
            if !matches!(drag, Drag::Seek) {
                self.model.commit_live_edit();
            }
            cx.notify();
        }
    }

    /// Snaps a time to the playhead, scene starts and other layers' edges
    /// when within a few pixels.
    fn snap(&self, t: Secs, ignore: Option<LayerId>) -> Secs {
        let tol = 6.0 / self.model.px_per_sec;
        let Some(c) = self.model.comp() else { return t };
        let mut points: Vec<Secs> = vec![0.0, self.model.playhead];
        points.extend(c.scene_spans().into_iter().map(|s| s.start));
        for l in &c.layers {
            if Some(l.id) != ignore {
                points.push(l.start);
                points.push(l.end());
            }
        }
        points.into_iter().filter(|p| (p - t).abs() <= tol).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t)
    }

    pub(crate) fn render_timeline(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let model = &mut self.model;
        // Keep the playhead in view during playback.
        if model.playing && model.follow_playhead {
            model.timeline_scroll = scroll_for_playhead(model.playhead, model.px_per_sec, model.timeline_scroll, self.timeline.width());
        }
        let model = &self.model;
        let lanes = lanes(model);
        let height = RULER_H + lanes.len() as f32 * LANE_H + SCRIPT_H + 8.0;
        let paint = TimelinePaint::collect(model, &lanes, cx);
        let bounds_cell = self.timeline.bounds.clone();
        let playing = model.playing;
        let time = format!("{} / {}", format_time_precise(model.playhead), format_time(model.duration()));

        let transport = h_flex()
            .id("transport")
            .flex_none()
            .w_full()
            .px_3()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .child(Button::new("tl-prev").ghost().small().label("⏮").tooltip("Previous scene").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| m.jump_scene(-1)))))
            .child(
                Button::new("tl-play")
                    .primary()
                    .small()
                    .icon(if playing { IconName::Pause } else { IconName::Play })
                    .tooltip(if playing { "Pause (Space)" } else { "Play (Space)" })
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePlay), cx)),
            )
            .child(Button::new("tl-next").ghost().small().label("⏭").tooltip("Next scene").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| m.jump_scene(1)))))
            .child(div().id("timecode").test_support().text_sm().font_family("Menlo").child(time))
            .child(Button::new("tl-split").ghost().small().label("Split").tooltip("Split at the playhead (S)").on_click(|_, window, cx| window.dispatch_action(Box::new(Split), cx)))
            .child(div().flex_1())
            .child(self.render_meter(cx))
            .child(Button::new("tl-zoom-out").ghost().small().label("−").tooltip("Zoom out").on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomOut), cx)))
            .child(Button::new("tl-zoom-fit").ghost().small().label("Fit").tooltip("Fit the whole composition").on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomFit), cx)))
            .child(Button::new("tl-zoom-in").ghost().small().label("+").tooltip("Zoom in").on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomIn), cx)));

        v_flex()
            .id("timeline-pane").test_support()
            .flex_none()
            .w_full()
            .border_t_1()
            .border_color(t.border)
            .key_context("Timeline")
            .track_focus(&self.timeline_focus)
            .child(transport)
            .child(
                div()
                    .id("timeline").test_support()
                    .w_full()
                    .h(px(height))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.timeline_mouse_down(ev, window, cx)))
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.timeline_mouse_move(ev, cx)))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.timeline_mouse_up(cx)))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.timeline_mouse_up(cx)))
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                        let d = ev.delta.pixel_delta(window.line_height());
                        if ev.modifiers.platform {
                            // Zoom around the pointer.
                            let before = this.timeline.x_to_time(&this.model, ev.position.x);
                            this.model.zoom(if f32::from(d.y) > 0.0 { 1.15 } else { 1.0 / 1.15 });
                            let local = f64::from(ev.position.x - this.timeline.bounds.get().origin.x);
                            this.model.timeline_scroll = (before * this.model.px_per_sec - local).max(0.0);
                        } else {
                            let dx = if f32::from(d.x).abs() > f32::from(d.y).abs() { f32::from(d.x) } else { f32::from(d.y) };
                            let max = (this.model.duration() * this.model.px_per_sec - this.timeline.width() * 0.5).max(0.0);
                            this.model.timeline_scroll = (this.model.timeline_scroll - dx as f64).clamp(0.0, max);
                        }
                        cx.notify();
                    }))
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                bounds_cell.set(bounds);
                            },
                            move |bounds, _, window, cx| paint.paint(bounds, window, cx),
                        )
                        .size_full(),
                    ),
            )
    }
}

impl Workspace {
    /// A two-bar level meter of what is playing, -48 dB to 0 dB.
    fn render_meter(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let (l, r) = self.preview.levels();
        let frac = |v: f32| ((20.0 * v.max(1e-6).log10() + 48.0) / 48.0).clamp(0.0, 1.0);
        let bar = |v: f32| {
            let f = frac(v);
            let color = if f > 0.97 { theme::hibiscus() } else if f > 0.85 { gpui_kit::yellow() } else { rgb(theme::LEAF).into() };
            div().w(px(90.)).h(px(4.)).rounded(px(2.)).bg(t.muted).child(div().h_full().w(relative(f)).rounded(px(2.)).bg(color))
        };
        v_flex().id("meter").gap(px(2.)).mr_2().child(bar(l)).child(bar(r))
    }
}

/// Everything the timeline canvas needs, copied out of the model so the
/// paint closure owns its data.
struct TimelinePaint {
    px_per_sec: f64,
    scroll: f64,
    duration: Secs,
    playhead: Secs,
    lanes: usize,
    clips: Vec<(Secs, Secs, bool)>,                  // start, end, is gap clip
    words: Vec<(Secs, Secs, SharedString, bool)>,    // start, end, text, selected
    scenes: Vec<Secs>,
    markers: Vec<(Secs, SharedString)>,
    comments: Vec<Secs>,
    layers: Vec<(usize, Secs, Secs, SharedString, Hsla, bool, bool)>, // lane, start, end, name, colour, selected, hidden
    fg: Hsla,
    muted: Hsla,
    border: Hsla,
    panel: Hsla,
}

impl TimelinePaint {
    fn collect(model: &AppModel, lanes: &[i32], cx: &App) -> Self {
        let t = cx.theme().clone();
        let mut p = TimelinePaint {
            px_per_sec: model.px_per_sec,
            scroll: model.timeline_scroll,
            duration: model.duration(),
            playhead: model.playhead,
            lanes: lanes.len(),
            clips: Vec::new(),
            words: Vec::new(),
            scenes: Vec::new(),
            markers: Vec::new(),
            comments: Vec::new(),
            layers: Vec::new(),
            fg: t.foreground,
            muted: t.muted_foreground,
            border: t.border,
            panel: t.muted,
        };
        let Some(c) = model.comp() else { return p };
        for clip in c.script_clips() {
            p.clips.push((clip.comp_start, clip.comp_end(), clip.asset.is_none()));
        }
        // Words only when zoomed in enough to read them.
        if model.px_per_sec >= 40.0 {
            let starts = c.token_starts();
            let (lo, hi) = (model.timeline_scroll / model.px_per_sec - 1.0, (model.timeline_scroll + 4000.0) / model.px_per_sec);
            for (i, tok) in c.script.iter().enumerate() {
                if !tok.shows_text() || starts[i] > hi || starts[i] + tok.play_duration() < lo {
                    continue;
                }
                p.words.push((starts[i], starts[i] + tok.play_duration(), tok.text.clone().into(), model.is_selected(i)));
            }
        }
        p.scenes = c.scene_spans().into_iter().skip(1).map(|s| s.start).collect();
        p.markers = c.script_markers().into_iter().map(|(_, t, n)| (t, n.into())).collect();
        p.comments = c.comments.iter().filter(|cm| !cm.resolved).map(|cm| model.editor.comment_time(cm)).collect();
        for l in &c.layers {
            let lane = lanes.iter().position(|t| *t == l.track).unwrap_or(0);
            let color: Hsla = match l.content {
                LayerContent::Video(_) => rgb(theme::LAGOON).into(),
                LayerContent::Image(_) => rgb(0x8E5BD9).into(),
                LayerContent::Audio(_) => rgb(theme::LEAF).into(),
                LayerContent::Text(_) => rgb(theme::MANGO).into(),
                LayerContent::Shape(_) => rgb(theme::HIBISCUS).into(),
                LayerContent::Waveform(_) | LayerContent::Progress { .. } => rgb(0xF2C91D).into(),
            };
            p.layers.push((lane, l.start, l.end(), l.name.clone().into(), color, model.selected_layer == Some(l.id), l.hidden));
        }
        p
    }

    fn x(&self, bounds: &Bounds<Pixels>, t: Secs) -> Pixels {
        bounds.origin.x + px((t * self.px_per_sec - self.scroll) as f32)
    }

    fn text(window: &mut Window, cx: &mut App, text: SharedString, origin: Point<Pixels>, size: f32, color: Hsla, max_w: Pixels) {
        if text.is_empty() || max_w < px(8.) {
            return;
        }
        let mut run = window.text_style().to_run(text.len());
        run.color = color;
        let line = window.text_system().shape_line(text, px(size), &[run], None);
        if line.width > max_w {
            return;
        }
        let _ = line.paint(origin, px(size * 1.3), TextAlign::Left, None, window, cx);
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let w = f64::from(bounds.size.width);
        let rect = |x0: Pixels, y0: f32, x1: Pixels, h: f32| Bounds::new(point(x0, bounds.origin.y + px(y0)), size((x1 - x0).max(px(1.)), px(h)));
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Ruler.
            let step = ruler_step(self.px_per_sec);
            let first = (self.scroll / self.px_per_sec / step).floor() as i64;
            let last = ((self.scroll + w) / self.px_per_sec / step).ceil() as i64;
            for k in first.max(0)..=last {
                let t = k as f64 * step;
                let x = self.x(&bounds, t);
                window.paint_quad(fill(rect(x, 14.0, x + px(1.), RULER_H - 14.0), self.border));
                let label = if step < 1.0 { format!("{:.1}", t) } else { format_time(t) };
                Self::text(window, cx, label.into(), point(x + px(3.), bounds.origin.y + px(2.)), 10.0, self.muted, px(80.));
            }
            // Layer lanes.
            for lane in 0..self.lanes {
                let y = RULER_H + lane as f32 * LANE_H;
                window.paint_quad(fill(rect(bounds.origin.x, y + LANE_H - 1.0, bounds.origin.x + bounds.size.width, 1.0), self.border.opacity(0.5)));
            }
            for (lane, s, e, name, color, selected, hidden) in &self.layers {
                let y = RULER_H + *lane as f32 * LANE_H + 2.0;
                let (x0, x1) = (self.x(&bounds, *s), self.x(&bounds, *e));
                let r = rect(x0, y, x1, LANE_H - 5.0);
                let body = color.opacity(if *hidden { 0.25 } else { 0.75 });
                window.paint_quad(quad(r, px(4.), body, px(if *selected { 2. } else { 0. }), self.fg, BorderStyle::Solid));
                Self::text(window, cx, name.clone(), point(x0 + px(5.), bounds.origin.y + px(y + 3.0)), 11.0, gpui_kit::black().opacity(0.85), x1 - x0 - px(8.));
            }
            // Script track.
            let sy = RULER_H + self.lanes as f32 * LANE_H + 4.0;
            for (s, e, gap) in &self.clips {
                let (x0, x1) = (self.x(&bounds, *s), self.x(&bounds, *e));
                if x1 < bounds.origin.x || x0 > bounds.origin.x + bounds.size.width {
                    continue;
                }
                let fillc = if *gap { self.panel.opacity(0.4) } else { self.panel };
                window.paint_quad(quad(rect(x0 + px(0.5), sy, x1 - px(0.5), SCRIPT_H - 8.0), px(5.), fillc, px(1.), self.border, BorderStyle::Solid));
            }
            for (s, e, text, selected) in &self.words {
                let (x0, x1) = (self.x(&bounds, *s), self.x(&bounds, *e));
                if *selected {
                    window.paint_quad(fill(rect(x0, sy + 1.0, x1, SCRIPT_H - 10.0), theme::lagoon().opacity(0.35)));
                }
                window.paint_quad(fill(rect(x0, sy + SCRIPT_H - 26.0, x0 + px(1.), 16.0), self.border));
                Self::text(window, cx, text.clone(), point(x0 + px(3.), bounds.origin.y + px(sy + SCRIPT_H - 26.0)), 10.5, self.fg, x1 - x0 - px(4.));
            }
            // Scene boundaries, markers, comments.
            let full_h = f32::from(bounds.size.height);
            for s in &self.scenes {
                let x = self.x(&bounds, *s);
                window.paint_quad(fill(rect(x, RULER_H, x + px(2.), full_h - RULER_H), theme::lagoon()));
            }
            for (t, name) in &self.markers {
                let x = self.x(&bounds, *t);
                window.paint_quad(fill(rect(x, 0.0, x + px(2.), RULER_H), rgb(0x8E5BD9)));
                Self::text(window, cx, name.clone(), point(x + px(5.), bounds.origin.y + px(10.)), 9.5, rgb(0x8E5BD9).into(), px(140.));
            }
            for t in &self.comments {
                let x = self.x(&bounds, *t);
                window.paint_quad(quad(rect(x - px(4.), 2.0, x + px(4.), 8.0), px(4.), rgb(0xF2C91D), px(0.), self.border, BorderStyle::Solid));
            }
            // End of composition.
            let xe = self.x(&bounds, self.duration);
            window.paint_quad(fill(rect(xe, RULER_H, xe + px(1.), full_h - RULER_H), self.border));
            // Playhead.
            let xp = self.x(&bounds, self.playhead);
            window.paint_quad(fill(rect(xp - px(0.5), 0.0, xp + px(1.0), full_h), theme::mango()));
            window.paint_quad(quad(rect(xp - px(5.), 0.0, xp + px(5.5), 10.0), px(3.), theme::mango(), px(0.), self.border, BorderStyle::Solid));
        });
    }
}
