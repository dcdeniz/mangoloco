//! The inspector under the canvas: properties of the composition, the
//! selected layer, captions, script audio and the current scene.

use crate::model::*;
use crate::theme;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_core::*;
use mango_core::{Animation, Rgba, TextAlign, TextStyle, Transform};

pub const SWATCHES: [u32; 10] = [0xFFFFFF, 0x000000, 0xFF8A00, 0x14A3C7, 0xE63E6D, 0x4CAF50, 0xF2C91D, 0x8E5BD9, 0x2A2E36, 0xC8CDD6];

type Edit = Box<dyn Fn(&mut AppModel) + 'static>;

fn edit(f: impl Fn(&mut AppModel) + 'static) -> Edit {
    Box::new(f)
}

/// Rounds to `step` so repeated nudges do not accumulate float noise.
fn nudge(v: f64, by: f64, lo: f64, hi: f64) -> f64 {
    let step = by.abs();
    (((v + by) / step).round() * step).clamp(lo, hi)
}

impl Workspace {
    fn row(&self, label: &'static str, cx: &mut Context<Self>) -> Div {
        h_flex().w_full().items_center().gap_2().py(px(3.)).child(div().w(px(96.)).flex_none().text_xs().text_color(cx.theme().muted_foreground).child(label))
    }

    /// `label  [-] value [+]`.
    fn stepper(&self, id: &'static str, label: &'static str, value: String, minus: Edit, plus: Edit, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        self.row(label, cx)
            .child(Button::new((id, 0usize)).ghost().xsmall().label("−").on_click(cx.listener(move |this, _, _, cx| {
                minus(&mut this.model);
                cx.notify();
            })))
            .child(div().id((id, 2usize)).w(px(74.)).text_sm().text_center().child(value))
            .child(Button::new((id, 1usize)).ghost().xsmall().label("+").on_click(cx.listener(move |this, _, _, cx| {
                plus(&mut this.model);
                cx.notify();
            })))
    }

    /// A row of mutually exclusive choices.
    fn chips(&self, id: &'static str, label: &'static str, options: Vec<(String, bool, Edit)>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut wrap = h_flex().flex_wrap().gap_1().flex_1();
        for (i, (text, on, f)) in options.into_iter().enumerate() {
            let b = Button::new((id, i)).xsmall().label(text).on_click(cx.listener(move |this, _, _, cx| {
                f(&mut this.model);
                cx.notify();
            }));
            wrap = wrap.child(if on { b.primary() } else { b.outline() });
        }
        self.row(label, cx).items_start().child(wrap)
    }

    fn toggle(&self, id: &'static str, label: &'static str, on: bool, f: Edit, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let b = Button::new(id).xsmall().label(if on { "On" } else { "Off" }).on_click(cx.listener(move |this, _, _, cx| {
            f(&mut this.model);
            cx.notify();
        }));
        self.row(label, cx).child(if on { b.primary() } else { b.outline() })
    }

    fn swatches(&self, id: &'static str, label: &'static str, current: Rgba, set: impl Fn(&mut AppModel, Rgba) + Clone + 'static, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let mut wrap = h_flex().flex_wrap().gap_1().flex_1();
        for (i, hex) in SWATCHES.iter().enumerate() {
            let c = Rgba::rgb((hex >> 16) as u8, (hex >> 8) as u8, *hex as u8);
            let on = (c.r, c.g, c.b) == (current.r, current.g, current.b);
            let set = set.clone();
            wrap = wrap.child(
                div()
                    .id((id, i))
                    .size(px(18.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(rgb(*hex))
                    .border_2()
                    .border_color(if on { theme::mango() } else { border })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        set(&mut this.model, Rgba::rgba(c.r, c.g, c.b, current.a.max(1)));
                        cx.notify();
                    })),
            );
        }
        self.row(label, cx).child(wrap).into_any_element()
    }

    fn section(&self, title: &'static str, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div().pt_3().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(cx.theme().foreground).child(title)
    }

    pub(crate) fn render_inspector(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let tabs = [
            (InspectorTab::Composition, "Canvas"),
            (InspectorTab::Layer, "Layer"),
            (InspectorTab::Captions, "Captions"),
            (InspectorTab::Audio, "Audio"),
            (InspectorTab::Scene, "Scene"),
        ];
        let current = self.model.inspector;
        let tab_bar = h_flex().flex_none().w_full().gap_1().px_3().py_2().border_y_1().border_color(t.border).children(tabs.into_iter().enumerate().map(|(i, (tab, name))| {
            let b = Button::new(("insp-tab", i)).xsmall().label(name).on_click(cx.listener(move |this, _, _, cx| {
                this.model.inspector = tab;
                cx.notify();
            }));
            if tab == current { b.primary() } else { b.ghost() }
        }));
        let body = match current {
            InspectorTab::Composition => self.inspect_composition(cx).into_any_element(),
            InspectorTab::Layer => self.inspect_layer(cx).into_any_element(),
            InspectorTab::Captions => self.inspect_captions(cx).into_any_element(),
            InspectorTab::Audio => self.inspect_audio(cx).into_any_element(),
            InspectorTab::Scene => self.inspect_scene(cx).into_any_element(),
        };
        v_flex().id("inspector").w_full().h(px(330.)).flex_none().child(tab_bar).child(div().id("inspector-body").flex_1().min_h_0().px_3().pb_3().overflow_y_scrollbar().child(body))
    }

    fn inspect_composition(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.model.comp() else { return v_flex().child("No composition") };
        let (w, h, fps, bg, loud) = (c.width, c.height, c.fps, c.background, c.master_loudness);
        let aspects = AspectPreset::ALL
            .into_iter()
            .map(|p| {
                let (pw, ph) = p.size();
                (p.label().to_string(), (pw, ph) == (w, h), edit(move |m| {
                    m.run(Command::SetCanvas { width: pw, height: ph, fill: true });
                }))
            })
            .collect();
        let rates = [24.0, 25.0, 30.0, 50.0, 60.0]
            .into_iter()
            .map(|r: f64| {
                (format!("{r}"), (fps - r).abs() < 0.01, edit(move |m| {
                    m.run(Command::SetFps { fps: r });
                }))
            })
            .collect();
        let louds = [None, Some(-14.0), Some(-16.0), Some(-18.0), Some(-23.0)]
            .into_iter()
            .map(|l: Option<f64>| {
                (l.map(|v| format!("{v} LUFS")).unwrap_or_else(|| "Off".into()), loud == l, edit(move |m| {
                    m.run(Command::SetMasterLoudness { lufs: l });
                }))
            })
            .collect();
        v_flex()
            .child(self.section("Canvas", cx))
            .child(self.row("Size", cx).child(div().id("canvas-size").text_sm().child(format!("{w} × {h}"))))
            .child(self.chips("aspect", "Aspect ratio", aspects, cx))
            .child(self.chips("fps", "Frame rate", rates, cx))
            .child(self.swatches("bg", "Background", bg, |m, c| {
                m.run(Command::SetBackground { color: c });
            }, cx))
            .child(self.section("Sound", cx))
            .child(self.chips("loud", "Loudness", louds, cx))
            .child(self.section("Add to canvas", cx))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(Button::new("add-text").outline().xsmall().label("Text").on_click(|_, window, cx| window.dispatch_action(Box::new(AddText), cx)))
                    .child(Button::new("add-rect").outline().xsmall().label("Rectangle").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.add_shape(ShapeKind::Rectangle);
                    }))))
                    .child(Button::new("add-ellipse").outline().xsmall().label("Ellipse").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.add_shape(ShapeKind::Ellipse);
                    }))))
                    .child(Button::new("add-line").outline().xsmall().label("Line").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.add_shape(ShapeKind::Line);
                    }))))
                    .child(Button::new("add-wave").outline().xsmall().label("Waveform").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.add_waveform();
                    }))))
                    .child(Button::new("add-progress").outline().xsmall().label("Progress bar").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.add_progress_bar();
                    })))),
            )
    }

    fn inspect_layer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(l) = self.model.layer().cloned() else {
            return v_flex().pt_4().gap_2().text_sm().text_color(cx.theme().muted_foreground).child("Select a layer on the canvas or timeline, or add one from the Canvas tab.");
        };
        let tr = l.transform;
        let pct = |v: f64| format!("{:.0}%", v * 100.0);
        let le = |f: fn(&mut Layer)| {
            edit(move |m: &mut AppModel| {
                m.edit_layer(f);
            })
        };
        let anims = |out: bool, current: Animation| -> Vec<(String, bool, Edit)> {
            [
                (Animation::None, "None"),
                (Animation::Fade, "Fade"),
                (Animation::SlideLeft, "Slide ←"),
                (Animation::SlideRight, "Slide →"),
                (Animation::SlideUp, "Slide ↑"),
                (Animation::SlideDown, "Slide ↓"),
                (Animation::ZoomIn, "Zoom"),
            ]
            .into_iter()
            .map(|(a, name)| {
                (name.to_string(), a == current, edit(move |m| {
                    m.edit_layer(|l| if out { l.animation_out = a } else { l.animation_in = a });
                }))
            })
            .collect()
        };
        let mut col = v_flex()
            .child(
                h_flex()
                    .pt_2()
                    .gap_1()
                    .flex_wrap()
                    .child(div().id("layer-name").text_sm().font_weight(FontWeight::BOLD).flex_1().truncate().child(l.name.clone()))
                    .child(Button::new("layer-hide").outline().xsmall().label(if l.hidden { "Show" } else { "Hide" }).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.edit_layer(|l| l.hidden = !l.hidden);
                    }))))
                    .child(Button::new("layer-lock").outline().xsmall().label(if l.locked { "Unlock" } else { "Lock" }).on_click(cx.listener(|this, _, _, cx| {
                        this.act(cx, |m| {
                            // Unlocking must work on a locked layer, so bypass the lock check.
                            if let Some(mut l) = m.layer().cloned() {
                                l.locked = !l.locked;
                                m.run(Command::UpdateLayer { layer: l });
                            }
                        })
                    })))
                    .child(Button::new("layer-dup").outline().xsmall().label("Duplicate").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.duplicate_layer();
                    }))))
                    .child(Button::new("layer-del").outline().xsmall().label("Delete").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                        m.delete_layer();
                    })))),
            )
            .child(self.section("Timing", cx))
            .child(self.stepper("l-start", "Start", format!("{:.2}s", l.start), le(|l| l.start = nudge(l.start, -0.1, 0.0, 1e6)), le(|l| l.start = nudge(l.start, 0.1, 0.0, 1e6)), cx))
            .child(self.stepper("l-dur", "Duration", format!("{:.2}s", l.duration), le(|l| l.duration = nudge(l.duration, -0.1, 0.1, 1e6)), le(|l| l.duration = nudge(l.duration, 0.1, 0.1, 1e6)), cx))
            .child(self.stepper("l-fi", "Fade in", format!("{:.1}s", l.fade_in), le(|l| l.fade_in = nudge(l.fade_in, -0.1, 0.0, 10.0)), le(|l| l.fade_in = nudge(l.fade_in, 0.1, 0.0, 10.0)), cx))
            .child(self.stepper("l-fo", "Fade out", format!("{:.1}s", l.fade_out), le(|l| l.fade_out = nudge(l.fade_out, -0.1, 0.0, 10.0)), le(|l| l.fade_out = nudge(l.fade_out, 0.1, 0.0, 10.0)), cx))
            .child(self.stepper("l-order", "Order", format!("{}", l.track), le(|l| l.track = (l.track - 1).max(1)), le(|l| l.track = (l.track + 1).min(99)), cx));

        if l.is_visual() {
            col = col
                .child(self.section("Position and size", cx))
                .child(self.stepper("l-x", "X", pct(tr.x), le(|l| l.transform.x = nudge(l.transform.x, -0.01, -0.5, 1.5)), le(|l| l.transform.x = nudge(l.transform.x, 0.01, -0.5, 1.5)), cx))
                .child(self.stepper("l-y", "Y", pct(tr.y), le(|l| l.transform.y = nudge(l.transform.y, -0.01, -0.5, 1.5)), le(|l| l.transform.y = nudge(l.transform.y, 0.01, -0.5, 1.5)), cx))
                .child(self.stepper("l-w", "Width", pct(tr.w), le(|l| l.transform.w = nudge(l.transform.w, -0.02, 0.02, 3.0)), le(|l| l.transform.w = nudge(l.transform.w, 0.02, 0.02, 3.0)), cx))
                .child(self.stepper("l-h", "Height", pct(tr.h), le(|l| l.transform.h = nudge(l.transform.h, -0.02, 0.02, 3.0)), le(|l| l.transform.h = nudge(l.transform.h, 0.02, 0.02, 3.0)), cx))
                .child(self.stepper("l-rot", "Rotation", format!("{:.0}°", tr.rotation_deg), le(|l| l.transform.rotation_deg = nudge(l.transform.rotation_deg, -5.0, -360.0, 360.0)), le(|l| l.transform.rotation_deg = nudge(l.transform.rotation_deg, 5.0, -360.0, 360.0)), cx))
                .child(self.stepper("l-op", "Opacity", pct(tr.opacity), le(|l| l.transform.opacity = nudge(l.transform.opacity, -0.05, 0.0, 1.0)), le(|l| l.transform.opacity = nudge(l.transform.opacity, 0.05, 0.0, 1.0)), cx))
                .child(
                    self.row("Align", cx).child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(Button::new("al-l").outline().xsmall().label("Left").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.x = l.transform.w / 2.0);
                            }))))
                            .child(Button::new("al-c").outline().xsmall().label("Center").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.x = 0.5);
                            }))))
                            .child(Button::new("al-r").outline().xsmall().label("Right").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.x = 1.0 - l.transform.w / 2.0);
                            }))))
                            .child(Button::new("al-t").outline().xsmall().label("Top").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.y = l.transform.h / 2.0);
                            }))))
                            .child(Button::new("al-m").outline().xsmall().label("Middle").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.y = 0.5);
                            }))))
                            .child(Button::new("al-b").outline().xsmall().label("Bottom").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform.y = 1.0 - l.transform.h / 2.0);
                            }))))
                            .child(Button::new("al-full").outline().xsmall().label("Fill canvas").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.edit_layer(|l| l.transform = Transform { rotation_deg: l.transform.rotation_deg, opacity: l.transform.opacity, ..Default::default() });
                            })))),
                    ),
                )
                .child(self.section("Animation", cx))
                .child(self.chips("anim-in", "In", anims(false, l.animation_in), cx))
                .child(self.chips("anim-out", "Out", anims(true, l.animation_out), cx));
        }

        match &l.content {
            LayerContent::Text(s) => {
                let aligns = [(TextAlign::Left, "Left"), (TextAlign::Center, "Center"), (TextAlign::Right, "Right")]
                    .into_iter()
                    .map(|(a, n)| {
                        (n.to_string(), s.align == a, edit(move |m| {
                            m.edit_layer(|l| {
                                if let LayerContent::Text(s) = &mut l.content {
                                    s.align = a;
                                }
                            });
                        }))
                    })
                    .collect();
                let fonts = ["Helvetica Neue", "Avenir Next", "Georgia", "Futura", "Menlo", "Impact"]
                    .into_iter()
                    .map(|f| {
                        (f.to_string(), s.font == f, edit(move |m| {
                            m.edit_layer(|l| {
                                if let LayerContent::Text(s) = &mut l.content {
                                    s.font = f.to_string();
                                }
                            });
                        }))
                    })
                    .collect();
                macro_rules! text_edit {
                    ($f:expr) => {
                        edit(|m| {
                            m.edit_layer(|l| {
                                if let LayerContent::Text(s) = &mut l.content {
                                    let f: fn(&mut TextStyle) = $f;
                                    f(s);
                                }
                            });
                        })
                    };
                }
                let seed = s.text.clone();
                col = col
                    .child(self.section("Text", cx))
                    .child(
                        self.row("Content", cx).child(div().id("text-content").flex_1().text_sm().truncate().child(s.text.replace('\n', " ⏎ "))).child(Button::new("text-edit").outline().xsmall().label("Edit").on_click(
                            cx.listener(move |this, _, window, cx| this.open_prompt(Ask::EditLayerText, &seed, window, cx)),
                        )),
                    )
                    .child(self.chips("font", "Font", fonts, cx))
                    .child(self.stepper("t-size", "Size", format!("{:.1}%", s.size * 100.0), text_edit!(|s| s.size = nudge(s.size, -0.005, 0.01, 0.5)), text_edit!(|s| s.size = nudge(s.size, 0.005, 0.01, 0.5)), cx))
                    .child(self.swatches("t-color", "Colour", s.color, |m, c| {
                        m.edit_layer(|l| {
                            if let LayerContent::Text(s) = &mut l.content {
                                s.color = c;
                            }
                        });
                    }, cx))
                    .child(self.chips("t-align", "Align", aligns, cx))
                    .child(self.toggle("t-bold", "Bold", s.bold, text_edit!(|s| s.bold = !s.bold), cx))
                    .child(self.toggle("t-italic", "Italic", s.italic, text_edit!(|s| s.italic = !s.italic), cx))
                    .child(self.toggle("t-shadow", "Shadow", s.shadow, text_edit!(|s| s.shadow = !s.shadow), cx))
                    .child(self.toggle("t-outline", "Outline", s.outline.is_some(), text_edit!(|s| s.outline = if s.outline.is_some() { None } else { Some((Rgba::BLACK, 3.0)) }), cx))
                    .child(self.toggle("t-box", "Background box", s.background.is_some(), text_edit!(|s| s.background = if s.background.is_some() { None } else { Some(Rgba::rgba(0, 0, 0, 180)) }), cx));
            }
            LayerContent::Shape(s) => {
                col = col
                    .child(self.section("Shape", cx))
                    .child(self.swatches("s-fill", "Fill", s.fill, |m, c| {
                        m.edit_layer(|l| {
                            if let LayerContent::Shape(s) = &mut l.content {
                                s.fill = c;
                            }
                        });
                    }, cx))
                    .child(self.stepper(
                        "s-radius",
                        "Corner radius",
                        format!("{:.0}%", s.corner_radius * 100.0),
                        le(|l| {
                            if let LayerContent::Shape(s) = &mut l.content {
                                s.corner_radius = nudge(s.corner_radius, -0.05, 0.0, 0.5);
                            }
                        }),
                        le(|l| {
                            if let LayerContent::Shape(s) = &mut l.content {
                                s.corner_radius = nudge(s.corner_radius, 0.05, 0.0, 0.5);
                            }
                        }),
                        cx,
                    ))
                    .child(self.toggle(
                        "s-stroke",
                        "Outline",
                        s.stroke.is_some(),
                        le(|l| {
                            if let LayerContent::Shape(s) = &mut l.content {
                                s.stroke = if s.stroke.is_some() { None } else { Some((Rgba::WHITE, 4.0)) };
                            }
                        }),
                        cx,
                    ));
            }
            LayerContent::Waveform(s) => {
                let kinds = [(WaveformKind::Bars, "Bars"), (WaveformKind::Line, "Line"), (WaveformKind::Mirror, "Mirror")]
                    .into_iter()
                    .map(|(k, n)| {
                        (n.to_string(), s.kind == k, edit(move |m| {
                            m.edit_layer(|l| {
                                if let LayerContent::Waveform(s) = &mut l.content {
                                    s.kind = k;
                                }
                            });
                        }))
                    })
                    .collect();
                col = col.child(self.section("Waveform", cx)).child(self.chips("w-kind", "Style", kinds, cx)).child(self.swatches("w-color", "Colour", s.color, |m, c| {
                    m.edit_layer(|l| {
                        if let LayerContent::Waveform(s) = &mut l.content {
                            s.color = c;
                        }
                    });
                }, cx));
            }
            LayerContent::Progress { color, .. } => {
                col = col.child(self.section("Progress bar", cx)).child(self.swatches("p-color", "Colour", *color, |m, c| {
                    m.edit_layer(|l| {
                        if let LayerContent::Progress { color, .. } = &mut l.content {
                            *color = c;
                        }
                    });
                }, cx));
            }
            LayerContent::Image(im) => {
                col = col.child(self.section("Image", cx)).child(self.fit_chips("i-fit", im.fit, cx)).child(self.stepper(
                    "i-zoom",
                    "Slow zoom",
                    format!("{:.0}%", im.zoom_to * 100.0),
                    le(|l| {
                        if let LayerContent::Image(i) = &mut l.content {
                            i.zoom_to = nudge(i.zoom_to, -0.05, 0.5, 2.0);
                        }
                    }),
                    le(|l| {
                        if let LayerContent::Image(i) = &mut l.content {
                            i.zoom_to = nudge(i.zoom_to, 0.05, 0.5, 2.0);
                        }
                    }),
                    cx,
                ));
                col = col.child(self.effect_toggles(&im.effects, cx));
            }
            LayerContent::Video(mc) | LayerContent::Audio(mc) => {
                macro_rules! clip_edit {
                    ($f:expr) => {
                        le(|l| {
                            if let LayerContent::Video(m) | LayerContent::Audio(m) = &mut l.content {
                                let f: fn(&mut MediaClip) = $f;
                                f(m);
                            }
                        })
                    };
                }
                if matches!(l.content, LayerContent::Video(_)) {
                    col = col.child(self.section("Video", cx)).child(self.fit_chips("v-fit", mc.fit, cx)).child(self.effect_toggles(&mc.effects, cx));
                }
                col = col
                    .child(self.section("Sound", cx))
                    .child(self.stepper("c-gain", "Volume", format!("{:+.0} dB", mc.audio.gain_db), clip_edit!(|m| m.audio.gain_db = nudge(m.audio.gain_db, -1.0, -60.0, 24.0)), clip_edit!(|m| m.audio.gain_db = nudge(m.audio.gain_db, 1.0, -60.0, 24.0)), cx))
                    .child(self.toggle("c-mute", "Mute", mc.audio.muted, clip_edit!(|m| m.audio.muted = !m.audio.muted), cx))
                    .child(self.stepper("c-pan", "Pan", format!("{:+.1}", mc.audio.pan), clip_edit!(|m| m.audio.pan = nudge(m.audio.pan, -0.1, -1.0, 1.0)), clip_edit!(|m| m.audio.pan = nudge(m.audio.pan, 0.1, -1.0, 1.0)), cx))
                    .child(self.stepper("c-speed", "Speed", format!("{:.2}x", mc.speed), clip_edit!(|m| m.speed = nudge(m.speed, -0.25, 0.25, 4.0)), clip_edit!(|m| m.speed = nudge(m.speed, 0.25, 0.25, 4.0)), cx))
                    .child(self.toggle("c-duck", "Lower under speech", mc.duck.is_some(), clip_edit!(|m| m.duck = if m.duck.is_some() { None } else { Some(Ducking::default()) }), cx))
                    .child(self.toggle("c-loop", "Loop", mc.loop_playback, clip_edit!(|m| m.loop_playback = !m.loop_playback), cx));
            }
        }
        col
    }

    fn fit_chips(&self, id: &'static str, current: FitMode, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let opts = [(FitMode::Fit, "Fit"), (FitMode::Fill, "Fill"), (FitMode::Stretch, "Stretch")]
            .into_iter()
            .map(|(f, n)| {
                (n.to_string(), f == current, edit(move |m| {
                    m.edit_layer(|l| match &mut l.content {
                        LayerContent::Video(c) => c.fit = f,
                        LayerContent::Image(c) => c.fit = f,
                        _ => {}
                    });
                }))
            })
            .collect();
        self.chips(id, "Scale", opts, cx)
    }

    /// On/off buttons for the visual effects of the selected media layer.
    fn effect_toggles(&self, effects: &[VideoEffect], cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let opts = effect_catalog()
            .into_iter()
            .map(|(name, make)| {
                let sample = make();
                let on = effects.iter().any(|e| std::mem::discriminant(e) == std::mem::discriminant(&sample));
                (name.to_string(), on, edit(move |m| {
                    m.edit_layer(|l| {
                        let list = match &mut l.content {
                            LayerContent::Video(c) => &mut c.effects,
                            LayerContent::Image(c) => &mut c.effects,
                            _ => return,
                        };
                        toggle_effect(list, make());
                    });
                }))
            })
            .collect();
        self.chips("fx", "Effects", opts, cx)
    }

    fn inspect_captions(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.model.comp() else { return v_flex() };
        let cap = c.captions.clone();
        macro_rules! cap_edit {
            ($f:expr) => {
                edit(|m| {
                    if let Some(mut cap) = m.comp().map(|c| c.captions.clone()) {
                        let f: fn(&mut CaptionSettings) = $f;
                        f(&mut cap);
                        m.run(Command::SetCaptions { captions: cap });
                    }
                })
            };
        }
        let modes = [(CaptionHighlight::None, "Plain"), (CaptionHighlight::ActiveWord, "Active word"), (CaptionHighlight::Reveal, "Reveal")]
            .into_iter()
            .map(|(h, n)| {
                (n.to_string(), cap.highlight == h, edit(move |m| {
                    if let Some(mut cap) = m.comp().map(|c| c.captions.clone()) {
                        cap.highlight = h;
                        m.run(Command::SetCaptions { captions: cap });
                    }
                }))
            })
            .collect();
        let set_color = |pick: fn(&mut CaptionSettings) -> &mut Rgba| {
            move |m: &mut AppModel, c: Rgba| {
                if let Some(mut cap) = m.comp().map(|c| c.captions.clone()) {
                    *pick(&mut cap) = c;
                    m.run(Command::SetCaptions { captions: cap });
                }
            }
        };
        v_flex()
            .child(self.section("Captions", cx))
            .child(self.toggle("cap-on", "Show captions", cap.enabled, cap_edit!(|c| c.enabled = !c.enabled), cx))
            .child(self.chips("cap-mode", "Style", modes, cx))
            .child(self.stepper("cap-size", "Size", format!("{:.1}%", cap.size * 100.0), cap_edit!(|c| c.size = nudge(c.size, -0.005, 0.015, 0.3)), cap_edit!(|c| c.size = nudge(c.size, 0.005, 0.015, 0.3)), cx))
            .child(self.stepper("cap-y", "Vertical position", format!("{:.0}%", cap.y * 100.0), cap_edit!(|c| c.y = nudge(c.y, -0.02, 0.05, 0.95)), cap_edit!(|c| c.y = nudge(c.y, 0.02, 0.05, 0.95)), cx))
            .child(self.stepper("cap-chars", "Characters per line", format!("{}", cap.max_chars_per_line), cap_edit!(|c| c.max_chars_per_line = c.max_chars_per_line.saturating_sub(2).max(8)), cap_edit!(|c| c.max_chars_per_line = (c.max_chars_per_line + 2).min(80)), cx))
            .child(self.stepper("cap-lines", "Lines", format!("{}", cap.max_lines), cap_edit!(|c| c.max_lines = c.max_lines.saturating_sub(1).max(1)), cap_edit!(|c| c.max_lines = (c.max_lines + 1).min(4)), cx))
            .child(self.swatches("cap-color", "Text colour", cap.color, set_color(|c| &mut c.color), cx))
            .child(self.swatches("cap-hi", "Active word", cap.highlight_color, set_color(|c| &mut c.highlight_color), cx))
            .child(self.toggle("cap-bold", "Bold", cap.bold, cap_edit!(|c| c.bold = !c.bold), cx))
            .child(self.toggle("cap-upper", "Uppercase", cap.uppercase, cap_edit!(|c| c.uppercase = !c.uppercase), cx))
            .child(self.toggle("cap-box", "Background box", cap.background.is_some(), cap_edit!(|c| c.background = if c.background.is_some() { None } else { Some(Rgba::rgba(0, 0, 0, 170)) }), cx))
            .child(self.toggle("cap-outline", "Outline", cap.outline.is_some(), cap_edit!(|c| c.outline = if c.outline.is_some() { None } else { Some((Rgba::BLACK, 2.0)) }), cx))
    }

    fn inspect_audio(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.model.comp() else { return v_flex() };
        let a = c.script_audio.clone();
        macro_rules! audio_edit {
            ($f:expr) => {
                edit(|m| {
                    if let Some(mut a) = m.comp().map(|c| c.script_audio.clone()) {
                        let f: fn(&mut AudioSettings) = $f;
                        f(&mut a);
                        m.run(Command::SetScriptAudio { audio: a });
                    }
                })
            };
        }
        let fx = audio_effect_catalog()
            .into_iter()
            .map(|(name, make)| {
                let sample = make();
                let on = a.effects.iter().any(|e| std::mem::discriminant(e) == std::mem::discriminant(&sample));
                (name.to_string(), on, edit(move |m| {
                    if let Some(mut a) = m.comp().map(|c| c.script_audio.clone()) {
                        let e = make();
                        match a.effects.iter().position(|x| std::mem::discriminant(x) == std::mem::discriminant(&e)) {
                            Some(i) => {
                                a.effects.remove(i);
                            }
                            None => a.effects.push(e),
                        }
                        m.run(Command::SetScriptAudio { audio: a });
                    }
                }))
            })
            .collect();
        let enhance = a.effects.iter().find_map(|e| if let AudioEffect::SpeechEnhance { intensity } = e { Some(*intensity) } else { None });
        let mut col = v_flex()
            .child(self.section("Script track", cx))
            .child(self.stepper("a-gain", "Volume", format!("{:+.0} dB", a.gain_db), audio_edit!(|a| a.gain_db = nudge(a.gain_db, -1.0, -60.0, 24.0)), audio_edit!(|a| a.gain_db = nudge(a.gain_db, 1.0, -60.0, 24.0)), cx))
            .child(self.toggle("a-mute", "Mute", a.muted, audio_edit!(|a| a.muted = !a.muted), cx))
            .child(self.stepper("a-pan", "Pan", format!("{:+.1}", a.pan), audio_edit!(|a| a.pan = nudge(a.pan, -0.1, -1.0, 1.0)), audio_edit!(|a| a.pan = nudge(a.pan, 0.1, -1.0, 1.0)), cx))
            .child(self.section("Effects (applied on export)", cx))
            .child(self.chips("a-fx", "Effects", fx, cx));
        if let Some(i) = enhance {
            let set = |by: f64| {
                edit(move |m: &mut AppModel| {
                    if let Some(mut a) = m.comp().map(|c| c.script_audio.clone()) {
                        for e in &mut a.effects {
                            if let AudioEffect::SpeechEnhance { intensity } = e {
                                *intensity = nudge(*intensity, by, 0.0, 1.0);
                            }
                        }
                        m.run(Command::SetScriptAudio { audio: a });
                    }
                })
            };
            col = col.child(self.stepper("a-enh", "Enhance amount", format!("{:.0}%", i * 100.0), set(-0.1), set(0.1), cx));
        }
        col
    }

    fn inspect_scene(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(c) = self.model.comp() else { return v_flex() };
        let scene = c.scene_at_time(self.model.playhead);
        let count = c.scene_spans().len();
        let st = c.scenes.get(scene).cloned().unwrap_or_default();
        let placement = st.script_video.clone().unwrap_or_else(|| c.script_video.clone());
        let transitions = Transition::ALL
            .into_iter()
            .map(|tr| {
                (tr.label().to_string(), st.transition == tr, edit(move |m| {
                    if let Some(c) = m.comp() {
                        let scene = c.scene_at_time(m.playhead);
                        let mut s = c.scenes.get(scene).cloned().unwrap_or_default();
                        s.transition = tr;
                        m.run(Command::SetScene { scene, settings: s });
                    }
                }))
            })
            .collect();
        let place = |name: &'static str, tr: Transform, fit: FitMode| {
            let on = (placement.transform.x - tr.x).abs() < 1e-6 && (placement.transform.w - tr.w).abs() < 1e-6 && (placement.transform.y - tr.y).abs() < 1e-6;
            (name.to_string(), on, edit(move |m| {
                if let Some(c) = m.comp() {
                    let scene = c.scene_at_time(m.playhead);
                    let mut s = c.scenes.get(scene).cloned().unwrap_or_default();
                    let mut p = s.script_video.clone().unwrap_or_else(|| c.script_video.clone());
                    p.transform = tr;
                    p.fit = fit;
                    s.script_video = Some(p);
                    m.run(Command::SetScene { scene, settings: s });
                }
            }))
        };
        let layouts = vec![
            place("Full frame", Transform::default(), FitMode::Fill),
            place("Fit", Transform::default(), FitMode::Fit),
            place("Left half", Transform { x: 0.25, y: 0.5, w: 0.5, h: 1.0, ..Default::default() }, FitMode::Fill),
            place("Right half", Transform { x: 0.75, y: 0.5, w: 0.5, h: 1.0, ..Default::default() }, FitMode::Fill),
            place("Corner", Transform { x: 0.82, y: 0.8, w: 0.3, h: 0.3, ..Default::default() }, FitMode::Fill),
            place("Framed", Transform { x: 0.5, y: 0.5, w: 0.8, h: 0.8, ..Default::default() }, FitMode::Fit),
        ];
        let fx = effect_catalog()
            .into_iter()
            .map(|(name, make)| {
                let sample = make();
                let on = placement.effects.iter().any(|e| std::mem::discriminant(e) == std::mem::discriminant(&sample));
                (name.to_string(), on, edit(move |m| {
                    if let Some(c) = m.comp() {
                        let scene = c.scene_at_time(m.playhead);
                        let mut s = c.scenes.get(scene).cloned().unwrap_or_default();
                        let mut p = s.script_video.clone().unwrap_or_else(|| c.script_video.clone());
                        toggle_effect(&mut p.effects, make());
                        s.script_video = Some(p);
                        m.run(Command::SetScene { scene, settings: s });
                    }
                }))
            })
            .collect();
        let dur = |by: f64| {
            edit(move |m: &mut AppModel| {
                if let Some(c) = m.comp() {
                    let scene = c.scene_at_time(m.playhead);
                    let mut s = c.scenes.get(scene).cloned().unwrap_or_default();
                    s.transition_secs = nudge(s.transition_secs, by, 0.1, 3.0);
                    m.run(Command::SetScene { scene, settings: s });
                }
            })
        };
        let (tr, secs) = (st.transition, st.transition_secs);
        v_flex()
            .child(self.section("Scene", cx))
            .child(self.row("Current", cx).child(div().id("scene-current").text_sm().child(format!("Scene {} of {}", scene + 1, count))))
            .child(self.chips("sc-layout", "Speaker video", layouts, cx))
            .child(self.chips("sc-fx", "Video effects", fx, cx))
            .when(scene > 0, |d| {
                d.child(self.section("Transition into this scene", cx))
                    .child(self.chips("sc-tr", "Transition", transitions, cx))
                    .child(self.stepper("sc-dur", "Duration", format!("{secs:.1}s"), dur(-0.1), dur(0.1), cx))
                    .child(self.row("", cx).child(Button::new("sc-all").outline().xsmall().label("Apply to all scenes").on_click(cx.listener(move |this, _, _, cx| {
                        this.act(cx, |m| {
                            m.run(Command::SetAllTransitions { transition: tr, secs });
                        })
                    }))))
            })
            .when(scene == 0 && count > 1, |d| d.child(div().pt_2().text_xs().text_color(cx.theme().muted_foreground).child("Move the playhead into a later scene to set its transition.")))
            .when(count == 1, |d| d.child(div().pt_2().text_xs().text_color(cx.theme().muted_foreground).child("Type / in the script, or press S, to start a new scene.")))
    }
}

/// Effects offered as simple on/off switches, with their default settings.
pub fn effect_catalog() -> Vec<(&'static str, fn() -> VideoEffect)> {
    vec![
        ("Green screen", || VideoEffect::ChromaKey { color: Rgba::rgb(0, 177, 64), similarity: 0.18, blend: 0.08 }),
        ("Blur", || VideoEffect::Blur { radius: 12.0 }),
        ("Black & white", || VideoEffect::Grayscale),
        ("Brighter", || VideoEffect::Brightness { value: 0.08 }),
        ("Contrast", || VideoEffect::Contrast { value: 1.2 }),
        ("Vivid", || VideoEffect::Saturation { value: 1.4 }),
        ("Warm", || VideoEffect::Temperature { kelvin: 5000.0 }),
        ("Vignette", || VideoEffect::Vignette),
        ("Film grain", || VideoEffect::Grain { amount: 0.25 }),
        ("Pixelate", || VideoEffect::Pixelate { size: 16 }),
        ("Invert", || VideoEffect::Invert),
        ("Mirror", || VideoEffect::Flip),
        ("Rounded", || VideoEffect::CornerRadius { fraction: 0.08 }),
    ]
}

pub fn audio_effect_catalog() -> Vec<(&'static str, fn() -> AudioEffect)> {
    vec![
        ("Speech enhance", || AudioEffect::SpeechEnhance { intensity: 0.6 }),
        ("Noise reduction", || AudioEffect::Denoise { strength_db: 12.0 }),
        ("Compressor", || AudioEffect::Compressor { threshold_db: -18.0, ratio: 3.0, attack_ms: 10.0, release_ms: 150.0, makeup_db: 3.0 }),
        ("Limiter", || AudioEffect::Limiter { ceiling_db: -1.0 }),
        ("High-pass 80 Hz", || AudioEffect::HighPass { hz: 80.0 }),
        ("Low-pass 12 kHz", || AudioEffect::LowPass { hz: 12000.0 }),
        ("Presence EQ", || AudioEffect::Eq { hz: 3500.0, gain_db: 3.0, q: 1.0 }),
        ("Bass shelf", || AudioEffect::LowShelf { hz: 120.0, gain_db: 3.0 }),
        ("Air shelf", || AudioEffect::HighShelf { hz: 9000.0, gain_db: 3.0 }),
        ("De-esser", || AudioEffect::DeEsser { intensity: 0.4 }),
        ("Gate", || AudioEffect::Gate { threshold_db: -45.0 }),
        ("Reverb", || AudioEffect::Reverb { mix: 0.25, room: 0.5 }),
        ("Level to -16", || AudioEffect::Loudness { target_lufs: -16.0 }),
    ]
}

pub fn toggle_effect(list: &mut Vec<VideoEffect>, e: VideoEffect) {
    match list.iter().position(|x| std::mem::discriminant(x) == std::mem::discriminant(&e)) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(e),
    }
}
