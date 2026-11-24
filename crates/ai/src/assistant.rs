//! The chat co-editor.
//!
//! [`Assistant::send`] runs one user turn: the model reads the project,
//! calls tools, and the tools edit a private working copy of the project.
//! When the turn finishes, the working copy is committed to the real editor
//! as exactly one undo step. If the turn fails or is cancelled, nothing is
//! committed and the conversation is rolled back, so the project is always
//! either fully before or fully after a response.

use crate::agent_tools;
use crate::client::{AiError, Client, Effort, Message, Request, Usage};
use crate::context;
use mango_core::{Command, Editor};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

const SYSTEM_PROMPT: &str = r#"You are the editing assistant inside Mango Loco, a desktop audio and video editor that works like a text editor. A recording is transcribed; the transcript is the "script"; removing words from the script removes that media from the cut. You edit the user's project by calling tools. The user watches the project change and can undo your whole response with one Undo.

# What you see

Every user message begins with the current project state. The script is shown one paragraph per line:

¶ 120..159 00:42 Ana: 120:So 121:um, 123:welcome (124: 1.8s pause) ~~125:I 126:think~~ 127:I 128:think …

- `N:word` means the word is token N. Tokens are words and silences; an index that is skipped is a short silence.
- `(N: 1.8s pause)` is a long silence.
- `~~ … ~~` is ignored (struck-through) text: it stays in the script but is skipped in playback and export, and the user can restore it at any time.
- `{hidden N:word}` plays, but its text is hidden from captions and exports.
- `✂` before a word means media was deleted between it and the previous word.
- Timecodes are composition time.

A long script is shown as an outline of paragraphs. Read exact words and indices with read_script, and find things with search_script.

# Ranges and revisions

- A range is start..end in token indices: start is inclusive, end is exclusive. The header `¶ 120..159` is such a range. To cover the words numbered 14 to 16, use start 14 and end 17.
- Cut whole sentences or clauses, including their punctuation, so that what remains reads naturally.
- Indices are only valid for the script revision ("rev") they were read at. Pass that rev with every index-based call. Within one call, every index refers to that rev even when the call removes several ranges, so never adjust indices yourself. When a result says TOKEN INDICES SHIFTED, read the script again before the next index-based call.
- Results of edits quote the text they affected. Check that it is what you meant, and correct it if it is not.

# How to edit

- For mechanical clean-up use the detector-backed tools: remove_fillers, remove_retakes, shorten_gaps (and list_fillers, list_retakes, list_gaps to answer questions). They are exact and exhaustive; do not hunt for "um" by eye.
- For subjective trims (cutting a topic, a tangent, a section, tightening), strike the text through with edit_ranges action "ignore" instead of deleting it, so the user can review and restore it. Delete only when the user explicitly asks for something to be deleted for good. To bring back something that was ignored, use action "unignore".
- Never invent content. You cannot add spoken words; the script only contains what was recorded. correct_range is only for fixing transcription mistakes. Text you write (titles, chapter names) must reflect what is actually in the project, and when the user dictates text, use it verbatim.
- If the request refers to something that is not in the project (a word that is never said, a layer that does not exist), verify with search_script or the overview, then say so plainly and change nothing. Do not substitute a different edit.
- Questions (how long is this, how many speakers, what do they talk about) are answered from the project state, without editing.
- If a request is ambiguous in a way that changes what would be cut, ask one short question instead of guessing. Otherwise act.
- A tool error means that call changed nothing. Read the message, fix the call, and try again.
- You cannot import or transcribe files, export, or undo. The user does those in the app.

# Replies

Reply in one or two plain sentences: what you did or found, with the concrete numbers from the tool results (words removed, seconds saved, new duration). No preamble, no token indices, no headings, and no commentary between tool calls."#;

/// Progress of a turn, for the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A piece of the assistant's reply as it streams.
    Text(String),
    ToolStarted { name: String, summary: String },
    ToolFinished { name: String, ok: bool, summary: String },
    /// The turn finished successfully. Not sent when `send` returns an error.
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    /// Everything the assistant said this turn.
    pub reply: String,
    /// Whether the project changed.
    pub changed: bool,
    /// Editor commands applied during the turn.
    pub commands_applied: usize,
    /// Undo steps added: 1 when the project changed, otherwise 0.
    pub undo_steps: usize,
    /// Tokens used by the turn.
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// What a tool call did, for display under an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAction {
    pub name: String,
    pub summary: String,
    pub ok: bool,
}

/// One message of the conversation as the user saw it. Serialisable, so a
/// conversation can be saved with the project and resumed with
/// [`Assistant::from_history`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ToolAction>,
}

pub struct Assistant {
    client: Client,
    /// Fixed for the life of the conversation (cached prefix; thinking blocks are bound to it).
    tools: Value,
    /// The conversation as sent to the API, append-only.
    messages: Vec<Value>,
    history: Vec<ChatMessage>,
    /// The project state most recently shown to the model.
    last_state: Option<String>,
    effort: Effort,
    max_steps: usize,
}

impl Assistant {
    pub fn new(client: Client) -> Self {
        // Unbuffered tool-input streaming is only requested from the real API; proxies may not know the field.
        let tools = agent_tools::definitions(client.is_official_endpoint());
        Self { client, tools, messages: Vec::new(), history: Vec::new(), last_state: None, effort: Effort::Medium, max_steps: 24 }
    }

    /// Resumes a saved conversation. Only the visible text is restored: the
    /// model sees what was said and a note of what was done, not the old tool
    /// traffic, and always gets the current project state with the next message.
    pub fn from_history(client: Client, history: Vec<ChatMessage>) -> Self {
        let mut a = Self::new(client);
        for m in &history {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let mut text = m.text.trim().to_string();
            if m.role == Role::Assistant {
                let done: Vec<&str> = m.actions.iter().filter(|x| x.ok).map(|x| x.summary.as_str()).filter(|s| !s.is_empty()).collect();
                if !done.is_empty() {
                    text = format!("{text}\n[Actions taken in that turn: {}]", done.join("; "));
                }
            }
            if text.trim().is_empty() {
                text = "(no text)".into();
            }
            // The API needs the conversation to start with the user and alternate.
            match a.messages.last_mut() {
                Some(last) if last["role"] == role => {
                    let prev = last["content"].as_str().unwrap_or("").to_string();
                    last["content"] = json!(format!("{prev}\n\n{text}"));
                }
                None if role == "assistant" => {}
                _ => a.messages.push(json!({"role": role, "content": text})),
            }
        }
        if a.messages.last().is_some_and(|m| m["role"] == "user") {
            a.messages.push(json!({"role": "assistant", "content": "(no reply was recorded)"}));
        }
        a.history = history;
        a
    }

    /// Thinking effort for assistant turns. Default medium.
    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = effort;
        self
    }

    /// Upper bound on model round-trips in one turn. Default 24.
    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n.max(1);
        self
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The conversation as the user saw it, for display and persistence.
    pub fn history(&self) -> &[ChatMessage] {
        &self.history
    }

    /// Forgets the conversation.
    pub fn reset(&mut self) {
        self.messages.clear();
        self.history.clear();
        self.last_state = None;
    }

    /// Runs one user turn against the editor.
    ///
    /// All edits of the turn become exactly one undo step labelled
    /// `Assistant: <start of the message>`; a turn without edits adds none.
    /// On any error (including [`AiError::Cancelled`]) the project is left
    /// exactly as it was and the turn is removed from the conversation.
    /// `cancel` is checked continuously, also while a response is streaming.
    pub fn send(&mut self, editor: &mut Editor, user_message: &str, on_event: &mut dyn FnMut(Event), cancel: &AtomicBool) -> Result<TurnResult, AiError> {
        let user_message = user_message.trim();
        if user_message.is_empty() {
            return Err(AiError::NothingToDo("The message is empty.".into()));
        }
        let saved_len = self.messages.len();
        let saved_state = self.last_state.clone();
        // The turn edits a private copy; the real editor is only touched on success.
        let mut work = Editor::new(editor.project.clone());
        work.active = editor.active;

        match self.run_turn(&mut work, user_message, on_event, cancel) {
            Ok(mut turn) => {
                let changed = work.project != editor.project || work.active != editor.active;
                if changed
                    && let Err(e) = commit(editor, work, &label_for(user_message)) {
                        self.messages.truncate(saved_len);
                        self.last_state = saved_state;
                        return Err(e);
                    }
                if turn.reply.trim().is_empty() {
                    let done: Vec<&str> = turn.actions.iter().filter(|a| a.ok && !a.summary.is_empty()).map(|a| a.summary.as_str()).collect();
                    turn.reply = if changed && !done.is_empty() { format!("Done: {}.", done.join("; ")) } else { "(The assistant did not reply.)".to_string() };
                }
                self.history.push(ChatMessage { role: Role::User, text: user_message.to_string(), actions: Vec::new() });
                self.history.push(ChatMessage { role: Role::Assistant, text: turn.reply.clone(), actions: turn.actions });
                on_event(Event::Done);
                Ok(TurnResult {
                    reply: turn.reply,
                    changed,
                    commands_applied: if changed { turn.commands } else { 0 },
                    undo_steps: usize::from(changed),
                    usage: turn.usage,
                })
            }
            Err(e) => {
                self.messages.truncate(saved_len);
                self.last_state = saved_state;
                Err(e)
            }
        }
    }

    fn request(&mut self, on_text: &mut dyn FnMut(&str), cancel: &AtomicBool, no_tools: bool) -> Result<Message, AiError> {
        let mut recovered = false;
        loop {
            let req = Request {
                model: self.client.model(),
                system: SYSTEM_PROMPT,
                tools: Some(&self.tools),
                messages: &self.messages,
                max_tokens: 16_000,
                effort: Some(self.client.effort_override().unwrap_or(self.effort)),
                schema: None,
                no_tools,
                cache_tail: true,
                long_lived_prefix: true,
            };
            match self.client.send(&req, on_text, Some(cancel)) {
                // Thinking blocks are bound to the model and conversation that produced them. If the API
                // rejects them (for example after the model was changed), drop them once and carry on.
                Err(AiError::BadRequest(m)) if !recovered && m.contains("thinking") && (m.contains("signature") || m.contains("different conversation")) => {
                    recovered = true;
                    for msg in &mut self.messages {
                        if let Some(blocks) = msg["content"].as_array_mut() {
                            blocks.retain(|b| b["type"] != "thinking" && b["type"] != "redacted_thinking");
                        }
                    }
                }
                other => return other,
            }
        }
    }

    fn run_turn(&mut self, work: &mut Editor, user_message: &str, on_event: &mut dyn FnMut(Event), cancel: &AtomicBool) -> Result<Turn, AiError> {
        if cancel.load(Ordering::SeqCst) {
            return Err(AiError::Cancelled);
        }
        let state = context::describe_project(work);
        let state_block = if self.last_state.as_deref() == Some(state.as_str()) {
            "<project_state>\nUnchanged since the project state shown in my previous message.\n</project_state>".to_string()
        } else {
            format!("<project_state>\n{state}</project_state>")
        };
        self.last_state = Some(state);
        self.messages.push(json!({"role": "user", "content": [
            {"type": "text", "text": state_block},
            {"type": "text", "text": user_message},
        ]}));

        let mut turn = Turn::default();
        let mut said_something = false;
        for step in 0..=self.max_steps {
            // Past the step limit, one last request without tools lets the model wrap up.
            let closing = step == self.max_steps;
            let mut first_delta = true;
            let msg = self.request(
                &mut |t: &str| {
                    if first_delta && said_something {
                        on_event(Event::Text("\n\n".into()));
                    }
                    first_delta = false;
                    on_event(Event::Text(t.to_string()));
                },
                cancel,
                closing,
            )?;
            turn.usage.add(&msg.usage);
            if msg.stop_reason == "refusal" {
                return Err(AiError::Refused);
            }
            let text = msg.text();
            if !text.trim().is_empty() {
                if said_something {
                    turn.reply.push_str("\n\n");
                }
                turn.reply.push_str(text.trim());
                said_something = true;
            }
            let content = msg.echo_content();
            if content.is_empty() {
                // Nothing to echo (an empty end_turn): close the turn with a placeholder so roles keep alternating.
                self.messages.push(json!({"role": "assistant", "content": "(no reply)"}));
                return Ok(turn);
            }
            self.messages.push(json!({"role": "assistant", "content": content}));

            let live = msg.live_from();
            let calls: Vec<(usize, &Value)> = msg.content.iter().enumerate().skip(live).filter(|(_, b)| b["type"] == "tool_use").collect();
            if calls.is_empty() {
                if msg.stop_reason == "pause_turn" && !closing {
                    continue;
                }
                return Ok(turn);
            }
            let truncated = msg.stop_reason == "max_tokens";
            let mut results = Vec::with_capacity(calls.len());
            for (index, block) in calls {
                if cancel.load(Ordering::SeqCst) {
                    return Err(AiError::Cancelled);
                }
                let id = block["id"].as_str().unwrap_or("");
                let name = block["name"].as_str().unwrap_or("");
                let outcome = if truncated {
                    agent_tools::ToolOutcome {
                        text: "ERROR: the response was cut off before this tool call was complete, so it was not run. Send it again, in smaller calls if it was large.".into(),
                        ok: false,
                        commands: 0,
                        summary: "Tool call was cut off".into(),
                    }
                } else if let Some(raw) = msg.bad_tool_inputs.get(&index) {
                    agent_tools::ToolOutcome {
                        text: json!({"INVALID_JSON": raw}).to_string(),
                        ok: false,
                        commands: 0,
                        summary: "Tool input was not valid JSON".into(),
                    }
                } else if closing {
                    agent_tools::ToolOutcome { text: "ERROR: no more tool calls can be made in this turn.".into(), ok: false, commands: 0, summary: "Step limit reached".into() }
                } else {
                    on_event(Event::ToolStarted { name: name.to_string(), summary: agent_tools::describe_call(name, &block["input"]) });
                    agent_tools::run(work, name, &block["input"])
                };
                on_event(Event::ToolFinished { name: name.to_string(), ok: outcome.ok, summary: outcome.summary.clone() });
                turn.commands += outcome.commands;
                turn.actions.push(ToolAction { name: name.to_string(), summary: outcome.summary, ok: outcome.ok });
                let mut result = json!({"type": "tool_result", "tool_use_id": id, "content": outcome.text});
                if !outcome.ok {
                    result["is_error"] = json!(true);
                }
                results.push(result);
            }
            // All results of one assistant message go back in a single user message.
            self.messages.push(json!({"role": "user", "content": results}));
            if closing {
                // The model ignored tool_choice none; end the turn with the conversation well-formed.
                self.messages.push(json!({"role": "assistant", "content": "(stopped: too many steps in one turn)"}));
                if !turn.reply.is_empty() {
                    turn.reply.push_str("\n\n");
                }
                turn.reply.push_str("I stopped because this needed too many steps. Check the result and tell me what is left to do.");
                return Ok(turn);
            }
        }
        Ok(turn)
    }
}

#[derive(Default)]
struct Turn {
    reply: String,
    commands: usize,
    actions: Vec<ToolAction>,
    usage: Usage,
}

fn label_for(user_message: &str) -> String {
    let one_line = user_message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = one_line.chars().take(48).collect();
    if one_line.chars().count() > 48 {
        short.push('…');
    }
    format!("Assistant: {short}")
}

/// Replaces the editor's project with the turn's result as one undo step.
///
/// `Editor` has no API for "replace the state as one labelled undo step", so
/// this pushes the undo snapshot by applying a throw-away command under the
/// label and then installs the final state through the public fields. Undo
/// restores the snapshot taken before the throw-away command, i.e. exactly
/// the project as it was before the turn; redo restores the turn's result.
fn commit(editor: &mut Editor, work: Editor, label: &str) -> Result<(), AiError> {
    let scratch_name = format!("{}~", editor.project.name);
    editor
        .apply_all(label, vec![Command::RenameProject { name: scratch_name }])
        .map_err(|e| AiError::InvalidOutput(format!("could not record the undo step: {e}")))?;
    editor.project = work.project;
    editor.active = work.active;
    Ok(())
}

/// The tool definitions given to the model (name, description, input schema).
pub fn tool_definitions() -> Value {
    agent_tools::definitions(false)
}

/// The system prompt of the assistant.
pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
}
