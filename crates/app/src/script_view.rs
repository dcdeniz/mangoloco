//! The script editor: the transcript as editable, clickable words.

use crate::model::*;
use crate::theme;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_core::{Token, TokenKind};

impl Workspace {
    pub(crate) fn render_script(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let empty = self.model.comp().is_none_or(|c| c.script.is_empty());
        let has_selection = !self.model.selection().is_empty();
        let font = self.model.script_font_px;

        let toolbar = h_flex()
            .id("script-toolbar")
            .flex_none()
            .w_full()
            .px_4()
            .py_2()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .child(Button::new("tb-delete").ghost().small().label("Delete").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.delete(true);
            }))))
            .child(Button::new("tb-ignore").ghost().small().label("Ignore").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.toggle_ignore();
            }))))
            .child(Button::new("tb-gap").ghost().small().label("Gap").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.replace_with_gap();
            }))))
            .child(Button::new("tb-restore").ghost().small().label("Restore").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.restore();
            }))))
            .child(Button::new("tb-correct").ghost().small().label("Correct").disabled(empty).on_click(cx.listener(|this, _, window, cx| {
                let seed = this.model.correction_seed();
                this.open_prompt(Ask::Correct, &seed, window, cx);
            })))
            .child(Button::new("tb-hide").ghost().small().label("Hide text").tooltip("Keep the media, remove the words from the transcript and captions").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.hide_text();
            }))))
            .child(div().w(px(1.)).h(px(16.)).mx_1().bg(t.border))
            .child(Button::new("tb-scene").ghost().small().label("Scene").disabled(empty).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                m.toggle_scene_break();
            }))))
            .child(Button::new("tb-marker").ghost().small().label("Marker").disabled(empty).on_click(cx.listener(|this, _, window, cx| {
                let seed = this.model.comp().and_then(|c| c.script.get(this.model.head)).and_then(|t| t.marker.clone()).unwrap_or_default();
                this.open_prompt(Ask::Marker, &seed, window, cx);
            })))
            .child(Button::new("tb-speaker").ghost().small().label("Speaker").disabled(empty).on_click(cx.listener(|this, _, window, cx| {
                this.open_prompt(Ask::Speaker, "", window, cx);
            })))
            .child(Button::new("tb-highlight").ghost().small().label("Highlight").disabled(empty).on_click(|_, window, cx| window.dispatch_action(Box::new(Highlight), cx)))
            .child(Button::new("tb-comment").ghost().small().label("Comment").disabled(empty).on_click(cx.listener(|this, _, window, cx| {
                this.open_prompt(Ask::Comment, "", window, cx);
            })))
            .child(Button::new("tb-dup").ghost().small().label("Duplicate to…").disabled(!has_selection).on_click(|_, window, cx| window.dispatch_action(Box::new(DuplicateTo), cx)))
            .child(div().flex_1())
            .child(Button::new("tb-find").ghost().small().label("Find").on_click(|_, window, cx| window.dispatch_action(Box::new(Find), cx)));

        let body = if empty {
            v_flex()
                .id("script-empty").test_support()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Add a recording to start"))
                .child(div().text_color(t.muted_foreground).child("Import audio or video and it is transcribed here. Then edit the text to edit the media."))
                .child(Button::new("empty-import").primary().label("Import media").on_click(|_, window, cx| window.dispatch_action(Box::new(ImportMedia), cx)))
                .into_any_element()
        } else {
            list(self.script_list.clone(), cx.processor(|this, ix: usize, _window, cx| this.render_paragraph(ix, cx).into_any_element()))
                .size_full()
                .into_any_element()
        };

        v_flex()
            .id("script-pane").test_support()
            .flex_1()
            .min_w(px(320.))
            .h_full()
            .key_context("ScriptEditor")
            .track_focus(&self.script_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.script_focus.focus(window, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.dragging_script = false;
                }),
            )
            .on_action(cx.listener(|this, _: &DeleteBackward, _, cx| this.act(cx, |m| {
                m.delete(false);
            })))
            .on_action(cx.listener(|this, _: &Ignore, _, cx| this.act(cx, |m| {
                m.toggle_ignore();
            })))
            .on_action(cx.listener(|this, _: &Restore, _, cx| this.act(cx, |m| {
                m.restore();
            })))
            .on_action(cx.listener(|this, _: &ReplaceWithGap, _, cx| this.act(cx, |m| {
                m.replace_with_gap();
            })))
            .on_action(cx.listener(|this, _: &CaretLeft, _, cx| this.act(cx, |m| m.move_caret(-1, false))))
            .on_action(cx.listener(|this, _: &CaretRight, _, cx| this.act(cx, |m| m.move_caret(1, false))))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.act(cx, |m| m.move_caret(-1, true))))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.act(cx, |m| m.move_caret(1, true))))
            .on_action(cx.listener(|this, _: &ParagraphUp, _, cx| this.act(cx, |m| m.move_paragraph(-1, false))))
            .on_action(cx.listener(|this, _: &ParagraphDown, _, cx| this.act(cx, |m| m.move_paragraph(1, false))))
            .on_action(cx.listener(|this, _: &SelectParagraphUp, _, cx| this.act(cx, |m| m.move_paragraph(-1, true))))
            .on_action(cx.listener(|this, _: &SelectParagraphDown, _, cx| this.act(cx, |m| m.move_paragraph(1, true))))
            .on_action(cx.listener(|this, _: &Correct, window, cx| {
                let seed = this.model.correction_seed();
                if !seed.is_empty() {
                    this.open_prompt(Ask::Correct, &seed, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SceneBreak, _, cx| this.act(cx, |m| {
                m.toggle_scene_break();
            })))
            .on_action(cx.listener(|this, _: &Marker, window, cx| {
                let seed = this.model.comp().and_then(|c| c.script.get(this.model.head)).and_then(|t| t.marker.clone()).unwrap_or_default();
                this.open_prompt(Ask::Marker, &seed, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Speaker, window, cx| this.open_prompt(Ask::Speaker, "", window, cx)))
            .on_action(cx.listener(|this, _: &ParagraphBreak, _, cx| this.act(cx, |m| {
                m.toggle_paragraph_break();
            })))
            .on_action(cx.listener(|this, _: &Bold, _, cx| this.act(cx, |m| {
                m.toggle_bold();
            })))
            .on_action(cx.listener(|this, _: &Italic, _, cx| this.act(cx, |m| {
                m.toggle_italic();
            })))
            .on_action(cx.listener(|this, _: &ScriptBigger, _, cx| {
                this.model.script_font_px = (this.model.script_font_px + 1.0).min(32.0);
                this.script_list.remeasure();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScriptSmaller, _, cx| {
                this.model.script_font_px = (this.model.script_font_px - 1.0).max(11.0);
                this.script_list.remeasure();
                cx.notify();
            }))
            .child(toolbar)
            .child(div().flex_1().min_h_0().w_full().text_size(px(font)).child(body))
    }

    /// One paragraph: optional marker heading and scene divider, the speaker
    /// label, then the words.
    fn render_paragraph(&mut self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let Some(range) = self.script_paragraphs.get(ix).cloned() else { return div().into_any_element() };
        let Some(comp) = self.model.comp() else { return div().into_any_element() };
        if range.end > comp.script.len() {
            return div().into_any_element();
        }
        let first = &comp.script[range.start];
        let playing = self.model.playing_token();
        let speaker = first.speaker.and_then(|id| self.model.editor.project.speaker(id)).map(|s| (s.name.clone(), theme::from_core(s.color)));
        // Show the speaker label only when it changes.
        let prev_speaker = ix.checked_sub(1).and_then(|p| self.script_paragraphs.get(p)).and_then(|r| comp.script.get(r.start)).map(|t| t.speaker);
        let show_speaker = speaker.is_some() && prev_speaker != Some(first.speaker);
        let time = format_time(comp.time_of_token(range.start));
        let marker = first.marker.clone();
        let scene_break = range.start > 0 && first.scene_start;
        let scene_no = if scene_break { comp.scene_index_of_token(range.start) + 1 } else { 0 };
        let caret = self.model.head;
        let has_range = self.model.has_range;
        let find_hits: Vec<std::ops::Range<usize>> = self.model.find.as_ref().map(|f| f.hits.clone()).unwrap_or_default();

        let mut words = h_flex().flex_wrap().items_baseline().gap_y(px(2.)).w_full();
        for i in range.clone() {
            let tok = &comp.script[i];
            if let Some(note) = &tok.note {
                words = words.child(div().px(px(3.)).italic().text_color(t.muted_foreground).child(format!("({note})")));
            }
            if !AppModel::is_visible(tok) {
                continue;
            }
            let selected = self.model.is_selected(i);
            let is_caret = !has_range && caret == i;
            let is_playing = playing == Some(i);
            let in_find = find_hits.iter().any(|h| h.contains(&i));
            words = words.child(self.render_token(i, tok, selected, is_caret, is_playing, in_find, cx));
        }

        v_flex()
            .id(("para", ix))
            .w_full()
            .px_8()
            .when(scene_break, |d| {
                d.child(
                    h_flex()
                        .id(("scene-break", ix)).test_support()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .pt_3()
                        .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(theme::lagoon()).child(format!("/ Scene {scene_no}")))
                        .child(div().flex_1().h(px(1.)).bg(theme::lagoon().opacity(0.4))),
                )
            })
            .when_some(marker, |d, name| {
                d.child(div().id(("marker", ix)).test_support().pt_4().pb_1().text_size(px(self.model.script_font_px * 1.25)).font_weight(FontWeight::BOLD).child(name))
            })
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_3()
                    .py_2()
                    .child(
                        v_flex()
                            .w(px(84.))
                            .flex_none()
                            .pt(px(3.))
                            .when_some(speaker.filter(|_| show_speaker), |d, (name, color)| {
                                d.child(div().id(("speaker", ix)).test_support().text_xs().font_weight(FontWeight::BOLD).text_color(color).truncate().child(name))
                            })
                            .child(div().text_xs().text_color(t.muted_foreground).child(time)),
                    )
                    .child(div().flex_1().min_w_0().line_height(relative(1.6)).child(words)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_token(&self, i: usize, tok: &Token, selected: bool, is_caret: bool, is_playing: bool, in_find: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let is_gap = tok.kind != TokenKind::Word;
        let label: SharedString = match tok.kind {
            TokenKind::Word => tok.text.clone().into(),
            TokenKind::Gap => format!("⋯ {:.1}s", tok.duration()).into(),
            TokenKind::Blank => format!("▢ {:.1}s", tok.duration()).into(),
        };
        let low_confidence = tok.kind == TokenKind::Word && tok.confidence < 0.5;
        let highlight = tok.highlight.map(|h| theme::alpha(theme::HIGHLIGHTS[h as usize % theme::HIGHLIGHTS.len()], 0.35));
        div()
            .id(("tok", i)).test_support()
            .px(px(3.))
            .rounded(px(4.))
            .cursor_pointer()
            .flex_none()
            .when(is_gap, |d| d.text_size(px(self.model.script_font_px * 0.75)).text_color(t.muted_foreground).mx(px(2.)).border_1().border_color(t.border))
            .when(tok.bold, |d| d.font_weight(FontWeight::BOLD))
            .when(tok.italic, |d| d.italic())
            .when(low_confidence, |d| d.underline())
            .when_some(highlight, |d, h| d.bg(h))
            .when(tok.ignored, |d| d.line_through().text_color(t.muted_foreground.opacity(0.7)))
            .when(in_find, |d| d.bg(theme::alpha(theme::HIGHLIGHTS[0], 0.45)))
            .when(is_playing && !selected, |d| d.bg(theme::mango().opacity(0.3)))
            .when(is_caret, |d| d.border_l_2().border_color(theme::mango()))
            .when(selected, |d| d.bg(theme::lagoon().opacity(0.4)))
            .hover(|s| s.bg(t.muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                    match ev.click_count {
                        2 => this.model.select_sentence(i),
                        n if n >= 3 => this.model.select_paragraph(i),
                        _ => {
                            this.model.click_token(i, ev.modifiers.shift);
                            this.dragging_script = true;
                        }
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _window, cx| {
                if this.dragging_script && ev.pressed_button == Some(MouseButton::Left) && this.model.head != i {
                    this.model.drag_to_token(i);
                    cx.notify();
                }
            }))
            .child(label)
    }
}
