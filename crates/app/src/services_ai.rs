//! Claude-backed features: the chat co-editor and the one-shot AI tools.

use crate::jobs::Completion;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_ai::tools::{self, Intensity, Platform};
use mango_ai::{AiError, Assistant, ChatMessage, Client, Event, Role};
use mango_core::{Command, Editor};
use std::sync::{Arc, Mutex};

/// What a one-shot tool produced.
pub enum AiOutcome {
    /// Text to show, copy or save.
    Text { title: String, text: String },
    /// Edits to apply as one undo step, with the message to show afterwards.
    Edits { label: String, commands: Vec<Command>, done: String },
    /// Nothing to change; just a message.
    Note(String),
}

pub struct AiState {
    /// The conversation. `None` while a turn is running on its thread.
    pub assistant: Option<Assistant>,
    pub chat: Vec<ChatMessage>,
    pub pending_user: Option<String>,
    events: Arc<Mutex<Vec<Event>>>,
    pub streaming: String,
    pub actions: Vec<(String, Option<bool>)>,
    pub busy: bool,
    pub job: Option<u64>,
    pub input: Entity<InputState>,
    pub result: Option<(String, String)>,
    _subs: Vec<Subscription>,
}

impl AiState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask for an edit, or ask about the recording"));
        let subs = vec![cx.subscribe_in(&input, window, |this, _input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.send_chat(window, cx);
            }
        })];
        Self {
            assistant: None,
            chat: Vec::new(),
            pending_user: None,
            events: Default::default(),
            streaming: String::new(),
            actions: Vec::new(),
            busy: false,
            job: None,
            input,
            result: None,
            _subs: subs,
        }
    }

    /// Moves streamed events into the visible state. True if anything arrived.
    pub fn drain_events(&mut self) -> bool {
        let events: Vec<Event> = std::mem::take(&mut *self.events.lock().unwrap());
        let any = !events.is_empty();
        for ev in events {
            match ev {
                Event::Text(t) => self.streaming.push_str(&t),
                Event::ToolStarted { summary, .. } => self.actions.push((summary, None)),
                Event::ToolFinished { summary, ok, .. } => {
                    match self.actions.iter_mut().rev().find(|a| a.1.is_none()) {
                        Some(a) => *a = (summary, Some(ok)),
                        None => self.actions.push((summary, Some(ok))),
                    }
                    // Text before a tool call is commentary; the final reply follows.
                    self.streaming.clear();
                }
                Event::Done => {}
            }
        }
        any
    }
}

impl Workspace {
    /// A client from the saved key (or the environment), with the chosen model.
    fn ai_client(&mut self) -> Option<Client> {
        match self.settings.resolved_api_key() {
            Some(key) => {
                let mut c = Client::new(key);
                let model = self.settings.assistant_model.trim();
                if !model.is_empty() {
                    c = c.with_model(model);
                }
                Some(c)
            }
            None => {
                self.model.fail(AiError::MissingKey);
                None
            }
        }
    }

    pub(crate) fn set_api_key(&mut self, key: &str) -> bool {
        self.settings.api_key = key.to_string();
        match self.settings.save() {
            Ok(()) => {
                // A new key means a new client; the conversation text is kept.
                if let Some(a) = self.ai.assistant.take() {
                    self.ai.chat = a.history().to_vec();
                }
                self.model.say(if key.is_empty() { "API key removed" } else { "API key saved" });
                true
            }
            Err(e) => {
                self.model.fail(format!("Could not save settings: {e:#}"));
                false
            }
        }
    }

    /// A private copy of the project for background work.
    fn scratch_editor(&self) -> Editor {
        let mut e = Editor::new(self.model.editor.project.clone());
        e.active = self.model.editor.active;
        e
    }

    // --- chat -----------------------------------------------------------------

    pub fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ai.input.read(cx).value().trim().to_string();
        if text.is_empty() || self.ai.busy {
            return;
        }
        if self.send_chat_text(&text, cx) {
            self.ai.input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Runs one assistant turn on a copy of the project; its edits land as a
    /// single undo step when it finishes.
    pub fn send_chat_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.ai.busy {
            return false;
        }
        let mut assistant = match self.ai.assistant.take() {
            Some(a) => a,
            None => {
                let Some(client) = self.ai_client() else {
                    cx.notify();
                    return false;
                };
                Assistant::from_history(client, self.ai.chat.clone())
            }
        };
        let mut scratch = self.scratch_editor();
        let started_at = self.model.editor.revision;
        let events = self.ai.events.clone();
        let message = text.to_string();
        self.ai.busy = true;
        self.ai.pending_user = Some(message.clone());
        self.ai.streaming.clear();
        self.ai.actions.clear();
        let id = self.jobs.start("Assistant", move |job| -> Completion {
            job.set_note("Thinking");
            let result = assistant.send(&mut scratch, &message, &mut |ev| events.lock().unwrap().push(ev), &job.cancel);
            Box::new(move |ws, window, cx| {
                ws.ai.drain_events();
                ws.ai.busy = false;
                ws.ai.job = None;
                ws.ai.pending_user = None;
                ws.ai.streaming.clear();
                ws.ai.actions.clear();
                ws.ai.chat = assistant.history().to_vec();
                match result {
                    Ok(turn) => {
                        if turn.changed {
                            if ws.model.editor.revision != started_at {
                                ws.model.fail("The project changed while the assistant was working, so its edits were not applied. Ask again");
                            } else {
                                let label = scratch.undo_label().unwrap_or("Assistant").to_string();
                                ws.model.editor.commit_state(&label, scratch.project, scratch.active);
                                ws.model.after_edit();
                                ws.model.clear_selection();
                                ws.model.say("The assistant edited the project. Undo reverts all of it");
                            }
                        }
                    }
                    Err(AiError::Cancelled) => {
                        ws.model.say("Stopped. Nothing was changed");
                        ws.ai.input.update(cx, |input, cx| input.set_value(message.clone(), window, cx));
                    }
                    Err(e) => {
                        ws.model.fail(e);
                        ws.ai.input.update(cx, |input, cx| input.set_value(message.clone(), window, cx));
                    }
                }
                ws.ai.assistant = Some(assistant);
                cx.notify();
            })
        });
        self.ai.job = Some(id);
        true
    }

    pub(crate) fn side_assistant(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let has_key = self.settings.resolved_api_key().is_some();
        let busy = self.ai.busy;
        let mut col = v_flex().id("assistant").w_full().p_3().gap_2();
        if !has_key {
            return col
                .child(div().text_sm().child("The assistant uses Claude to edit by request and to write summaries, chapters and clips."))
                .child(div().text_xs().text_color(t.muted_foreground).child("It needs an Anthropic API key. The key is stored on this Mac and sent only to Anthropic."))
                .child(Button::new("ai-key").primary().small().label("Add API key").on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::ApiKey, "", window, cx))));
        }
        if self.ai.chat.is_empty() && self.ai.pending_user.is_none() {
            col = col.child(div().text_sm().text_color(t.muted_foreground).child("Try: “remove the filler words”, “cut the part about pricing”, “add captions and make it vertical”, “how long is this?”"));
        }
        let bubble = |i: usize, m: &ChatMessage| {
            let user = m.role == Role::User;
            v_flex()
                .id(("chat", i))
                .test_support()
                .w_full()
                .p_2()
                .gap_1()
                .rounded_md()
                .when(user, |d| d.bg(t.muted))
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(if user { t.muted_foreground } else { crate::theme::mango() }).child(if user { "You" } else { "Assistant" }))
                .child(div().text_sm().child(m.text.clone()))
                .children(m.actions.iter().map(|a| div().text_xs().text_color(if a.ok { t.muted_foreground } else { crate::theme::hibiscus() }).child(format!("{} {}", if a.ok { "✓" } else { "✕" }, a.summary))))
        };
        for (i, m) in self.ai.chat.iter().enumerate() {
            col = col.child(bubble(i, m));
        }
        if let Some(u) = &self.ai.pending_user {
            col = col.child(bubble(usize::MAX - 1, &ChatMessage { role: Role::User, text: u.clone(), actions: Vec::new() }));
            col = col.child(
                v_flex()
                    .id("chat-live")
                    .w_full()
                    .p_2()
                    .gap_1()
                    .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(crate::theme::mango()).child("Assistant"))
                    .children(self.ai.actions.iter().map(|(s, ok)| {
                        div().text_xs().text_color(t.muted_foreground).child(format!("{} {}", match ok { None => "…", Some(true) => "✓", Some(false) => "✕" }, s))
                    }))
                    .child(div().text_sm().child(if self.ai.streaming.is_empty() { "Working…".to_string() } else { self.ai.streaming.clone() })),
            );
        }
        col.child(div().w_full().child(Input::new(&self.ai.input).id("chat-input")))
            .child(
                h_flex()
                    .gap_1()
                    .when(!busy, |d| d.child(Button::new("chat-send").primary().small().label("Send").on_click(cx.listener(|this, _, window, cx| this.send_chat(window, cx)))))
                    .when(busy, |d| {
                        d.child(Button::new("chat-stop").outline().small().label("Stop").on_click(cx.listener(|this, _, _, cx| {
                            if let Some(id) = this.ai.job {
                                this.jobs.cancel(id);
                            }
                            cx.notify();
                        })))
                    })
                    .child(Button::new("chat-clear").ghost().small().label("New chat").disabled(busy).on_click(cx.listener(|this, _, _, cx| {
                        this.ai.assistant = None;
                        this.ai.chat.clear();
                        cx.notify();
                    })))
                    .child(Button::new("chat-key").ghost().small().label("API key").on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::ApiKey, "", window, cx)))),
            )
    }

    // --- one-shot tools -------------------------------------------------------

    /// Runs an AI tool on a copy of the project in the background.
    pub fn run_ai_tool(&mut self, label: &str, cx: &mut Context<Self>, work: impl FnOnce(&Client, &Editor) -> Result<AiOutcome, AiError> + Send + 'static) -> bool {
        let Some(client) = self.ai_client() else {
            cx.notify();
            return false;
        };
        let scratch = self.scratch_editor();
        let started_at = self.model.editor.revision;
        let label = label.to_string();
        let job_label = label.clone();
        self.jobs.start(job_label, move |job| -> Completion {
            job.set_note("Asking Claude");
            let client = client.with_cancel_flag(job.cancel.clone());
            let result = work(&client, &scratch);
            Box::new(move |ws, _window, cx| {
                match result {
                    Ok(AiOutcome::Text { title, text }) => {
                        ws.ai.result = Some((title, text));
                        ws.model.side_panel = crate::model::SidePanel::AiTools;
                    }
                    Ok(AiOutcome::Note(n)) => ws.model.say(n),
                    Ok(AiOutcome::Edits { label, commands, done }) => {
                        if ws.model.editor.revision != started_at {
                            ws.model.fail("The script changed while Claude was working, so the result no longer lines up. Run it again");
                        } else {
                            match ws.model.editor.apply_all(&label, commands) {
                                Ok(()) => {
                                    ws.model.after_edit();
                                    ws.model.say(done);
                                }
                                Err(e) => ws.model.fail(format!("{label} could not be applied: {e:#}")),
                            }
                        }
                    }
                    Err(AiError::Cancelled) => ws.model.say(format!("{label} cancelled")),
                    Err(e) => ws.model.fail(format!("{label}: {e}")),
                }
                cx.notify();
            })
        });
        cx.notify();
        true
    }

    pub(crate) fn translate_subtitles(&mut self, language: &str, cx: &mut Context<Self>) -> bool {
        let language = language.trim().to_string();
        if language.is_empty() {
            return false;
        }
        self.run_ai_tool("Translate subtitles", cx, move |client, editor| {
            let comp = editor.comp().ok_or_else(|| AiError::NothingToDo("There is no script to translate".into()))?;
            let cues = mango_core::captions::cues(comp, &mango_core::captions::CueOptions::default());
            if cues.is_empty() {
                return Err(AiError::NothingToDo("There is no speech to translate".into()));
            }
            let texts: Vec<String> = cues.iter().map(|c| c.text().replace('\n', " ")).collect();
            let translated = tools::translate_captions(client, &texts, &language)?;
            let mut out = cues.clone();
            for (c, t) in out.iter_mut().zip(translated) {
                c.lines = vec![t];
            }
            Ok(AiOutcome::Text { title: format!("Subtitles in {language} (SRT)"), text: mango_core::captions::to_srt(&out, None, false) })
        })
    }

    pub(crate) fn social_post(&mut self, platform: &str, cx: &mut Context<Self>) -> bool {
        let key = platform.trim().to_lowercase();
        let Some(p) = Platform::ALL.into_iter().find(|p| p.label().to_lowercase() == key || (key == "twitter" && *p == Platform::X)) else {
            self.model.fail("Choose X, LinkedIn, Instagram, TikTok or Threads");
            return false;
        };
        self.run_ai_tool("Social post", cx, move |c, e| Ok(AiOutcome::Text { title: format!("Post for {}", p.label()), text: tools::social_post(c, e, p)? }))
    }

    pub(crate) fn write_with_ai(&mut self, kind: Ask, prompt: &str, cx: &mut Context<Self>) -> bool {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return false;
        }
        let (label, title): (&str, &str) = match kind {
            Ask::Brainstorm => ("Brainstorm", "Ideas"),
            Ask::Outline => ("Outline", "Outline"),
            _ => ("Write a script", "Script draft"),
        };
        self.run_ai_tool(label, cx, move |c, _| {
            let text = match kind {
                Ask::Brainstorm => tools::brainstorm(c, &prompt)?,
                Ask::Outline => tools::outline(c, &prompt)?,
                _ => tools::write_script(c, &prompt)?,
            };
            Ok(AiOutcome::Text { title: title.to_string(), text })
        })
    }

    pub(crate) fn render_ai_tools(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let has_key = self.settings.resolved_api_key().is_some();
        let has_script = self.model.comp().is_some_and(|c| c.script.iter().any(|t| t.shows_text()));
        let off = !has_key || !has_script;
        let heading = |s: &'static str| div().px_3().pt_3().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(t.muted_foreground).child(s);
        macro_rules! tool {
            ($id:expr, $label:expr, $title:expr, $f:expr) => {
                Button::new($id).outline().xsmall().label($label).disabled(off).on_click(cx.listener(|this, _, _, cx| {
                    this.run_ai_tool($title, cx, $f);
                }))
            };
        }
        let mut col = v_flex().w_full();
        if let Some((title, text)) = self.ai.result.clone() {
            let copy = text.clone();
            col = col.child(
                v_flex()
                    .id("ai-result")
                    .test_support()
                    .mx_3()
                    .mt_3()
                    .p_2()
                    .gap_2()
                    .rounded_md()
                    .bg(t.muted)
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(title))
                    .child(div().id("ai-result-text").max_h(px(260.)).overflow_y_scroll().text_xs().child(text))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Button::new("ai-copy").primary().xsmall().label("Copy").on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.model.say("Copied");
                                cx.notify();
                            })))
                            .child(Button::new("ai-close").ghost().xsmall().label("Close").on_click(cx.listener(|this, _, _, cx| {
                                this.ai.result = None;
                                cx.notify();
                            }))),
                    ),
            );
        }
        col = col.child(heading("WITH CLAUDE"));
        if !has_key {
            col = col.child(
                v_flex()
                    .px_3()
                    .gap_2()
                    .child(div().text_xs().text_color(t.muted_foreground).child("These tools need an Anthropic API key."))
                    .child(Button::new("tools-key").outline().xsmall().label("Add API key").on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::ApiKey, "", window, cx)))),
            );
        }
        col.child(
            v_flex()
                .px_3()
                .gap_2()
                .child(div().text_xs().text_color(t.muted_foreground).child("Edit. Each runs as one step you can undo."))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .child(tool!("ai-chapters", "Add chapters", "Add chapters", |c, e| {
                            let ch = tools::chapters(c, e)?;
                            let n = ch.len();
                            Ok(if n == 0 { AiOutcome::Note("No clear chapters found".into()) } else { AiOutcome::Edits { label: "Add chapters".into(), commands: tools::apply_chapters(&ch), done: format!("Added {n} chapter markers") } })
                        }))
                        .child(tool!("ai-clarity", "Edit for clarity", "Edit for clarity", |c, e| {
                            let edits = tools::edit_for_clarity(c, e, Intensity::Medium)?;
                            let n = edits.len();
                            Ok(if n == 0 {
                                AiOutcome::Note("Nothing to tighten".into())
                            } else {
                                AiOutcome::Edits { label: "Edit for clarity".into(), commands: tools::apply_clarity_edits(&edits), done: format!("Struck through {n} passages. Review them, then delete ignored text") }
                            })
                        }))
                        .child(tool!("ai-clips", "Find clips", "Find clips", |c, e| {
                            let source = e.active.ok_or_else(|| AiError::NothingToDo("There is no composition".into()))?;
                            let clips = tools::find_clips(c, e, 3, 15.0, 60.0, None)?;
                            let n = clips.len();
                            Ok(if n == 0 { AiOutcome::Note("No stand-alone clips found".into()) } else { AiOutcome::Edits { label: "Create clips".into(), commands: tools::apply_clips(source, &clips), done: format!("Created {n} clip compositions") } })
                        }))
                        .child(tool!("ai-speakers", "Label speakers", "Label speakers", |c, e| {
                            let turns = tools::label_speakers(c, e)?;
                            let len = e.comp().map(|c| c.script.len()).unwrap_or(0);
                            let n = turns.len();
                            Ok(if n == 0 { AiOutcome::Note("Could not tell speakers apart from the wording".into()) } else { AiOutcome::Edits { label: "Label speakers".into(), commands: tools::apply_speaker_labels(len, &turns), done: format!("Labelled {n} speaker turns from the wording. Check them") } })
                        })),
                )
                .child(div().pt_1().text_xs().text_color(t.muted_foreground).child("Write"))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .child(tool!("ai-summary", "Summary", "Summary", |c, e| Ok(AiOutcome::Text { title: "Summary".into(), text: tools::summarize(c, e)? })))
                        .child(tool!("ai-notes", "Show notes", "Show notes", |c, e| Ok(AiOutcome::Text { title: "Show notes".into(), text: tools::show_notes(c, e)? })))
                        .child(tool!("ai-titles", "Titles", "Titles", |c, e| Ok(AiOutcome::Text { title: "Title ideas".into(), text: tools::titles(c, e, 8)?.join("\n") })))
                        .child(tool!("ai-youtube", "YouTube description", "YouTube description", |c, e| Ok(AiOutcome::Text { title: "YouTube description".into(), text: tools::youtube_description(c, e)? })))
                        .child(tool!("ai-blog", "Blog post", "Blog post", |c, e| Ok(AiOutcome::Text { title: "Blog post".into(), text: tools::blog_post(c, e)? })))
                        .child(Button::new("ai-brainstorm").outline().xsmall().label("Brainstorm…").disabled(!has_key).on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::Brainstorm, "", window, cx))))
                        .child(Button::new("ai-outline").outline().xsmall().label("Outline…").disabled(!has_key).on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::Outline, "", window, cx))))
                        .child(Button::new("ai-script").outline().xsmall().label("Script…").disabled(!has_key).on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::WriteScript, "", window, cx))))
                        .child(Button::new("ai-social").outline().xsmall().label("Social post…").disabled(off).on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::SocialPost, "LinkedIn", window, cx))))
                        .child(Button::new("ai-translate").outline().xsmall().label("Translate subtitles…").disabled(off).on_click(cx.listener(|this, _, window, cx| this.open_prompt(Ask::Translate, "", window, cx)))),
                ),
        )
    }
}
