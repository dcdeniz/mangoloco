//! The left sidebar: project files, editing tools, comments.

use crate::model::*;
use crate::theme;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_core::detect::FillerOptions;
use mango_core::editor::RemoveMode;
use mango_core::{Command, MediaKind};

impl Workspace {
    pub(crate) fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let current = self.model.side_panel;
        let tabs = [(SidePanel::Project, "Project"), (SidePanel::AiTools, "Tools"), (SidePanel::Assistant, "Assistant"), (SidePanel::Comments, "Comments")];
        let tab_bar = h_flex().flex_none().w_full().gap_1().px_2().py_2().border_b_1().border_color(t.border).children(tabs.into_iter().enumerate().map(|(i, (tab, name))| {
            let b = Button::new(("side-tab", i)).xsmall().label(name).on_click(cx.listener(move |this, _, _, cx| {
                this.model.side_panel = tab;
                cx.notify();
            }));
            if tab == current { b.primary() } else { b.ghost() }
        }));
        let body = match current {
            SidePanel::Project => self.side_project(cx).into_any_element(),
            SidePanel::AiTools => self.side_tools(cx).into_any_element(),
            SidePanel::Assistant => self.side_assistant(cx).into_any_element(),
            SidePanel::Comments => self.side_comments(cx).into_any_element(),
        };
        v_flex()
            .id("sidebar")
            .w(px(270.))
            .flex_none()
            .h_full()
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.border)
            .child(tab_bar)
            .child(div().id("sidebar-body").flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }

    fn side_heading(&self, title: &'static str, cx: &mut Context<Self>) -> Div {
        div().px_3().pt_3().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(cx.theme().muted_foreground).child(title)
    }

    fn side_project(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let muted = t.muted_foreground;
        let assets: Vec<(mango_core::AssetId, String, MediaKind, f64, bool)> =
            self.model.editor.project.assets.iter().map(|a| (a.id, a.name.clone(), a.kind, a.duration, a.transcript.is_some())).collect();
        let jobs = self.render_jobs(cx).into_any_element();
        let mut col = v_flex()
            .w_full()
            .child(
                h_flex()
                    .px_3()
                    .pt_3()
                    .gap_1()
                    .child(Button::new("import").primary().small().label("Import media").on_click(|_, window, cx| window.dispatch_action(Box::new(ImportMedia), cx)))
                    .child(Button::new("record").outline().small().label("Record").on_click(|_, window, cx| window.dispatch_action(Box::new(crate::services::RecordDialog), cx)))
                    .child(Button::new("speak").outline().small().label("Speech").on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::Speak, "", window, cx)))),
            )
            .child(jobs)
            .child(self.side_heading("FILES", cx));
        if assets.is_empty() {
            col = col.child(div().px_3().py_2().text_sm().text_color(muted).child("No files yet. Import audio, video or images, or drop them on the window."));
        }
        for (i, (id, name, kind, dur, has_transcript)) in assets.into_iter().enumerate() {
            let kind_label = match kind {
                MediaKind::Video => "Video",
                MediaKind::Audio => "Audio",
                MediaKind::Image => "Image",
            };
            let detail = if kind == MediaKind::Image { kind_label.to_string() } else { format!("{kind_label} · {}{}", format_time(dur), if has_transcript { "" } else { " · not transcribed" }) };
            col = col.child(
                v_flex()
                    .id(("asset", i)).test_support()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(t.border.opacity(0.5))
                    .child(div().text_sm().truncate().child(name))
                    .child(div().text_xs().text_color(muted).child(detail))
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .when(kind != MediaKind::Image, |d| {
                                d.child(Button::new(("asset-script", i)).outline().xsmall().label("Add to script").on_click(cx.listener(move |this, _, _, cx| {
                                    this.act(cx, |m| {
                                        m.run(Command::InsertAsset { asset: id, at: None });
                                    })
                                })))
                            })
                            .child(Button::new(("asset-layer", i)).outline().xsmall().label("Add as layer").on_click(cx.listener(move |this, _, _, cx| {
                                this.act(cx, |m| {
                                    m.add_asset_layer(id);
                                })
                            })))
                            .when(kind != MediaKind::Image, |d| {
                                d.child(Button::new(("asset-transcribe", i)).outline().xsmall().label(if has_transcript { "Re-transcribe" } else { "Transcribe" }).on_click(cx.listener(
                                    move |this, _, window, cx| this.transcribe_asset(id, window, cx),
                                )))
                            })
                            .child(Button::new(("asset-remove", i)).ghost().xsmall().label("Remove").on_click(cx.listener(move |this, _, _, cx| {
                                this.act(cx, |m| {
                                    m.run(Command::RemoveAsset { id });
                                })
                            }))),
                    ),
            );
        }
        col
    }

    fn side_tools(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let fillers = self.model.fillers(&FillerOptions::default()).len();
        let loose = self.model.fillers(&FillerOptions { loose: true, ..Default::default() }).len();
        let retakes = self.model.retakes().len();
        let gaps = self.model.comp().map(|c| mango_core::detect::find_gaps(c, 1.0).len()).unwrap_or(0);
        let ignored = self.model.comp().map(|c| c.script.iter().filter(|t| t.ignored).count()).unwrap_or(0);
        let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let card = |title: &'static str, detail: String| v_flex().px_3().py_2().gap_1().child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title)).child(div().text_xs().text_color(muted).child(detail));
        let ai = self.render_ai_tools(cx).into_any_element();
        v_flex()
            .w_full()
            .child(self.side_heading("CLEAN UP", cx))
            .child(
                card("Filler words", format!("{} found (um, uh, you know, repeats)", plural(fillers, "filler", "fillers"))).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(Button::new("fill-del").outline().xsmall().label("Delete").disabled(fillers == 0).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                            m.remove_fillers(FillerOptions::default(), RemoveMode::Delete);
                        }))))
                        .child(Button::new("fill-ign").outline().xsmall().label("Ignore").disabled(fillers == 0).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                            m.remove_fillers(FillerOptions::default(), RemoveMode::Ignore);
                        }))))
                        .child(Button::new("fill-loose").ghost().xsmall().label(format!("Also \"like\", \"so\" ({loose})")).disabled(loose == fillers).on_click(cx.listener(|this, _, _, cx| {
                            this.act(cx, |m| {
                                m.remove_fillers(FillerOptions { loose: true, ..Default::default() }, RemoveMode::Ignore);
                            })
                        }))),
                ),
            )
            .child(
                card("Retakes", format!("{} found. The earlier take is struck through so you can review it", plural(retakes, "retake", "retakes"))).child(Button::new("retakes").outline().xsmall().label("Mark retakes").disabled(retakes == 0).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.act(cx, |m| {
                            m.remove_retakes(RemoveMode::Ignore);
                        })
                    }),
                )),
            )
            .child(
                card("Word gaps", format!("{} longer than 1 second", plural(gaps, "pause", "pauses"))).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(Button::new("gaps-1").outline().xsmall().label("Over 1s → 0.4s").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                            m.shorten_gaps(1.0, 0.4);
                        }))))
                        .child(Button::new("gaps-05").outline().xsmall().label("Over 0.5s → 0.25s").on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                            m.shorten_gaps(0.5, 0.25);
                        })))),
                ),
            )
            .child(
                card("Ignored text", format!("{} struck through", plural(ignored, "word", "words"))).child(Button::new("rm-ignored").outline().xsmall().label("Delete all ignored text").disabled(ignored == 0).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.act(cx, |m| {
                            m.run(Command::RemoveIgnored);
                        })
                    },
                ))),
            )
            .child(self.side_heading("TRANSCRIPT", cx))
            .child(
                h_flex()
                    .px_3()
                    .gap_1()
                    .flex_wrap()
                    .child(Button::new("correct-all").outline().xsmall().label("Correct everywhere…").on_click(cx.listener(|this, _, window, cx| {
                        let seed = this.model.correction_seed();
                        let seed = if seed.is_empty() { String::new() } else { format!("{seed} => ") };
                        this.open_prompt(Ask::CorrectAll, &seed, window, cx);
                    })))
                    .child(Button::new("rename-speaker").outline().xsmall().label("Rename speaker…").disabled(self.model.editor.project.speakers.is_empty()).on_click(cx.listener(|this, _, window, cx| {
                        let seed = this.model.editor.project.speakers.first().map(|s| format!("{} => ", s.name)).unwrap_or_default();
                        this.open_prompt(Ask::RenameSpeaker, &seed, window, cx);
                    })))
                    .child(Button::new("glossary").outline().xsmall().label("Glossary…").on_click(cx.listener(|this, _, window, cx| {
                        let seed = this.model.editor.project.glossary.iter().map(|g| g.term.clone()).collect::<Vec<_>>().join(", ");
                        this.open_prompt(Ask::Glossary, &seed, window, cx);
                    }))),
            )
            .child(ai)
    }

    fn side_comments(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let muted = t.muted_foreground;
        let border = t.border;
        let mut items: Vec<(mango_core::CommentId, f64, String, String, bool, Vec<(String, String)>)> = self
            .model
            .comp()
            .map(|c| c.comments.iter().map(|cm| (cm.id, self.model.editor.comment_time(cm), cm.author.clone(), cm.text.clone(), cm.resolved, cm.replies.clone())).collect())
            .unwrap_or_default();
        items.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut col = v_flex().w_full().child(
            h_flex().px_3().pt_3().child(Button::new("comment-add").outline().small().label("Comment at playhead").on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::Comment, "", window, cx)))),
        );
        if items.is_empty() {
            col = col.child(div().px_3().py_3().text_sm().text_color(muted).child("No comments. Select part of the script and press Shift+Cmd+M."));
        }
        for (i, (id, time, author, text, resolved, replies)) in items.into_iter().enumerate() {
            col = col.child(
                v_flex()
                    .id(("comment", i)).test_support()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(border.opacity(0.5))
                    .when(resolved, |d| d.opacity(0.55))
                    .child(h_flex().gap_2().text_xs().child(div().font_weight(FontWeight::BOLD).child(author)).child(div().text_color(theme::lagoon()).child(format_time(time))))
                    .child(div().text_sm().child(text))
                    .children(replies.into_iter().map(|(a, r)| div().pl_3().text_xs().text_color(muted).child(format!("{a}: {r}"))))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Button::new(("comment-go", i)).ghost().xsmall().label("Go to").on_click(cx.listener(move |this, _, _, cx| {
                                this.act(cx, |m| {
                                    m.seek(time);
                                    if let Some(tok) = m.playing_token() {
                                        m.select_range(tok..tok);
                                    }
                                })
                            })))
                            .child(Button::new(("comment-resolve", i)).ghost().xsmall().label(if resolved { "Reopen" } else { "Resolve" }).on_click(cx.listener(move |this, _, _, cx| {
                                this.act(cx, |m| {
                                    m.run(Command::ResolveComment { id, resolved: !resolved });
                                })
                            })))
                            .child(Button::new(("comment-del", i)).ghost().xsmall().label("Delete").on_click(cx.listener(move |this, _, _, cx| {
                                this.act(cx, |m| {
                                    m.run(Command::DeleteComment { id });
                                })
                            }))),
                    ),
            );
        }
        col
    }
}
