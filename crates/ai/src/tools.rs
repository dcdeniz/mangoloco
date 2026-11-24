//! One-shot AI tools.
//!
//! Each tool sends the script to the model once and returns a *proposal*:
//! text to show, or structured suggestions the user can accept. None of them
//! changes the editor. The `apply_*` helpers turn an accepted proposal into
//! [`Command`]s for `Editor::apply_all`.
//!
//! Structured proposals are never trusted as returned. The model refers to
//! numbered sentences (or, for clarity edits, indexed words) of the script;
//! everything is mapped back and validated against the composition
//! ([`validate_chapters`], [`validate_clips`], [`validate_clarity_edits`])
//! before it is returned.

use crate::client::{AiError, Ask, Client, Effort};
use crate::context::{self, Sentence};
use mango_core::detect;
use mango_core::transcript_export::{self, timecode};
use mango_core::{Command, Composition, CompositionId, Editor};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write as _;

type Result<T> = std::result::Result<T, AiError>;

const SYSTEM: &str = "You work inside Mango Loco, an audio and video editor in which the user edits a recording by editing its transcript. \
You are given the transcript of the user's recording as numbered sentences with timecodes (minutes:seconds), and one task. \
Base everything on what is actually said in the transcript: never invent facts, quotes, names, numbers or links. \
The transcript is spoken language from automatic transcription, so read through its disfluencies and small recognition errors. \
Write in the language of the transcript unless the task says otherwise. \
Return only what the task asks for, with no preamble and no closing remarks.";

const WRITER_SYSTEM: &str = "You are a writing assistant inside Mango Loco, an audio and video editor used by podcasters and video creators. \
Return only the requested text, with no preamble and no closing remarks.";

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

struct Script<'a> {
    comp: &'a Composition,
    sentences: Vec<Sentence>,
    /// The transcript block shared by every transcript tool, so it is cached across them.
    block: String,
}

fn script_of(editor: &Editor) -> Result<Script<'_>> {
    let comp = editor.comp().ok_or_else(|| AiError::NothingToDo("No composition is open.".into()))?;
    let sentences = context::sentences(comp);
    if sentences.is_empty() {
        return Err(AiError::NothingToDo("The script has no words yet. Transcribe a recording first.".into()));
    }
    let mut block = format!(
        "<transcript title={:?} duration=\"{}\" sentences=\"{}\">\n",
        comp.name,
        timecode(comp.script_duration()),
        sentences.len()
    );
    block.push_str(&context::render_sentences(&editor.project, &sentences, true));
    block.push_str("</transcript>");
    Ok(Script { comp, sentences, block })
}

/// Transcript first with a cache breakpoint, then the task: tools run one
/// after another on the same script reuse the cached transcript.
fn transcript_messages(block: &str, task: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": [
        {"type": "text", "text": block, "cache_control": {"type": "ephemeral"}},
        {"type": "text", "text": task},
    ]})]
}

fn text_task(client: &Client, block: &str, task: &str, max_tokens: u32) -> Result<String> {
    let messages = transcript_messages(block, task);
    let out = client.ask(&Ask { model: client.model(), system: SYSTEM, messages: &messages, max_tokens, effort: Effort::Medium, schema: None })?;
    Ok(out.trim().to_string())
}

fn writer_task(client: &Client, model: &str, prompt: String, effort: Effort) -> Result<String> {
    let messages = vec![json!({"role": "user", "content": prompt})];
    let out = client.ask(&Ask { model, system: WRITER_SYSTEM, messages: &messages, max_tokens: 32_000, effort, schema: None })?;
    Ok(out.trim().to_string())
}

/// Parses the model's JSON answer. Structured outputs make it valid JSON, but
/// it is still parsed defensively (a proxy may not enforce the schema).
fn parse_json<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).map(|s| s.trim_end_matches("```")).unwrap_or(t).trim();
    serde_json::from_str::<T>(t).map_err(|e| AiError::InvalidOutput(format!("the answer was not in the expected format ({e})")))
}

/// Asks for JSON matching `schema`. On a follow-up attempt, `feedback` (the
/// previous answer and what was wrong with it) is appended to the conversation.
fn json_task<T: for<'de> Deserialize<'de>>(client: &Client, model: &str, system: &str, messages: &[Value], schema: &Value, effort: Effort) -> Result<(T, String)> {
    let text = client.ask(&Ask { model, system, messages, max_tokens: 32_000, effort, schema: Some(schema) })?;
    Ok((parse_json::<T>(&text)?, text))
}

fn with_feedback(mut messages: Vec<Value>, previous_answer: &str, feedback: &str) -> Vec<Value> {
    messages.push(json!({"role": "assistant", "content": previous_answer}));
    messages.push(json!({"role": "user", "content": feedback}));
    messages
}

fn object_schema(field: &str, item: Value) -> Value {
    json!({
        "type": "object",
        "properties": {field: {"type": "array", "items": item}},
        "required": [field],
        "additionalProperties": false
    })
}

fn clean_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Text proposals
// ---------------------------------------------------------------------------

/// A short summary of what is said in the active composition.
pub fn summarize(client: &Client, editor: &Editor) -> Result<String> {
    let s = script_of(editor)?;
    text_task(client, &s.block, "Task: summarise this recording in one short paragraph (3 to 5 sentences) of plain text: what it is about and its main points. No heading.", 8_000)
}

/// Show notes in Markdown: a summary, key points with timestamps, and the
/// people, products and links that are mentioned.
pub fn show_notes(client: &Client, editor: &Editor) -> Result<String> {
    let s = script_of(editor)?;
    text_task(
        client,
        &s.block,
        "Task: write show notes for this episode in Markdown with exactly these sections:\n\
         ## Summary\nTwo or three sentences.\n\
         ## Key points\nA bulleted list of the main points in order; start each bullet with the timecode where it is discussed, as [mm:ss] (drop the tenths).\n\
         ## Mentions\nA bulleted list of the people, companies, products, books and websites that are named in the transcript. Only include links that are spoken in the transcript. If nothing is named, write \"None.\"",
        16_000,
    )
}

/// A long-form blog post in Markdown based on the recording.
pub fn blog_post(client: &Client, editor: &Editor) -> Result<String> {
    let s = script_of(editor)?;
    text_task(
        client,
        &s.block,
        "Task: turn this recording into a blog post in Markdown: a title as a level-1 heading, a short introduction, sections with level-2 headings that follow the recording's topics, and a brief conclusion. \
         Write in clean prose rather than as a transcript, in the speakers' voice where natural. Use only what is said in the recording.",
        32_000,
    )
}

/// A YouTube description. When the composition has chapter markers, the
/// chapter list is included exactly as the app exports it.
pub fn youtube_description(client: &Client, editor: &Editor) -> Result<String> {
    let s = script_of(editor)?;
    let has_markers = !s.comp.script_markers().is_empty() || s.comp.markers.iter().any(|m| m.chapter);
    let chapters = if has_markers { transcript_export::chapters_text(s.comp) } else { String::new() };
    let mut task = String::from(
        "Task: write a YouTube description for this video in plain text: a first line that hooks the viewer (it is shown before \"more\"), then one or two short paragraphs on what the video covers, then up to five relevant hashtags on the last line. \
         Do not invent links, sponsors or calls to action that are not in the transcript.",
    );
    if has_markers {
        let _ = write!(task, "\nAfter the paragraphs and before the hashtags, include this chapter list exactly as given, one per line, under the line \"Chapters:\":\n{chapters}");
    } else {
        task.push_str("\nDo not include a chapter list or timestamps.");
    }
    let mut out = text_task(client, &s.block, &task, 8_000)?;
    // The chapter list must be exact: make sure every line survived, otherwise add the list ourselves.
    if has_markers && !chapters.lines().all(|l| out.contains(l.trim())) {
        let _ = write!(out, "\n\nChapters:\n{chapters}");
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    X,
    LinkedIn,
    Instagram,
    TikTok,
    Threads,
}

impl Platform {
    pub const ALL: [Platform; 5] = [Platform::X, Platform::LinkedIn, Platform::Instagram, Platform::TikTok, Platform::Threads];

    pub fn label(self) -> &'static str {
        match self {
            Platform::X => "X",
            Platform::LinkedIn => "LinkedIn",
            Platform::Instagram => "Instagram",
            Platform::TikTok => "TikTok",
            Platform::Threads => "Threads",
        }
    }

    /// Hard character limit of a post on the platform.
    pub fn max_chars(self) -> usize {
        match self {
            Platform::X => 280,
            Platform::LinkedIn => 3000,
            Platform::Instagram => 2200,
            Platform::TikTok => 2200,
            Platform::Threads => 500,
        }
    }

    fn brief(self) -> &'static str {
        match self {
            Platform::X => "a single post for X of at most 270 characters including spaces and hashtags: one sharp idea from the recording, at most two hashtags",
            Platform::LinkedIn => "a LinkedIn post of 80 to 200 words: a strong first line, short paragraphs separated by blank lines, a takeaway, and up to three hashtags at the end",
            Platform::Instagram => "an Instagram caption of 40 to 120 words: a hook in the first line, a short body, and five to eight hashtags on the last line",
            Platform::TikTok => "a TikTok caption of at most 150 characters followed by three to five hashtags",
            Platform::Threads => "a Threads post of at most 480 characters including spaces: conversational, one idea, at most one hashtag",
        }
    }
}

/// A post promoting the recording, written for one platform and within its length limit.
pub fn social_post(client: &Client, editor: &Editor, platform: Platform) -> Result<String> {
    let s = script_of(editor)?;
    let task = format!("Task: write {} promoting this recording. Plain text only, no Markdown, no surrounding quotes.", platform.brief());
    let messages = transcript_messages(&s.block, &task);
    let ask = |messages: &[Value]| client.ask(&Ask { model: client.model(), system: SYSTEM, messages, max_tokens: 8_000, effort: Effort::Medium, schema: None }).map(|t| t.trim().to_string());
    let first = ask(&messages)?;
    let limit = platform.max_chars();
    if first.chars().count() <= limit {
        return Ok(first);
    }
    let feedback = format!("That is {} characters; the limit on {} is {limit}. Rewrite it to fit, keeping the main idea. Return only the post.", first.chars().count(), platform.label());
    let second = ask(&with_feedback(messages, &first, &feedback))?;
    if second.chars().count() <= limit {
        Ok(second)
    } else {
        Err(AiError::InvalidOutput(format!("the post is longer than {} allows ({} characters, limit {limit})", platform.label(), second.chars().count())))
    }
}

/// `count` distinct title ideas (1 to 20), best first.
pub fn titles(client: &Client, editor: &Editor, count: usize) -> Result<Vec<String>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let count = count.min(20);
    let s = script_of(editor)?;
    let schema = object_schema("titles", json!({"type": "string"}));
    #[derive(Deserialize)]
    struct Out {
        titles: Vec<String>,
    }
    let task = format!("Task: propose exactly {count} different titles for this recording, best first. Each under 70 characters, specific to what is said, no numbering, no quotes. Answer as JSON.");
    let messages = transcript_messages(&s.block, &task);
    let (out, raw): (Out, String) = json_task(client, client.model(), SYSTEM, &messages, &schema, Effort::Medium)?;
    let mut titles = validate_titles(out.titles, count);
    if titles.len() < count {
        let feedback = format!("I need exactly {count} distinct, non-empty titles; that gave {}. Answer again with exactly {count}.", titles.len());
        let (again, _): (Out, String) = json_task(client, client.model(), SYSTEM, &with_feedback(messages, &raw, &feedback), &schema, Effort::Medium)?;
        for t in validate_titles(again.titles, count) {
            if titles.len() < count && !titles.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                titles.push(t);
            }
        }
    }
    if titles.len() < count {
        return Err(AiError::InvalidOutput(format!("asked for {count} titles but only {} usable ones came back", titles.len())));
    }
    Ok(titles)
}

/// Trims, removes numbering/quotes, drops empty and duplicate titles, and keeps at most `count`.
pub fn validate_titles(raw: Vec<String>, count: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        let t = clean_line(&t);
        let t = t.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '•' | ' ')).trim_matches(['"', '\u{201C}', '\u{201D}', ' ']).to_string();
        if !t.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
        if out.len() == count {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Chapters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    /// Token index of the first word of the sentence that opens the chapter.
    pub token: usize,
    /// Composition time of that token.
    pub time: f64,
    pub title: String,
}

/// Chapters must be at least this far apart (video platforms reject closer ones).
pub const MIN_CHAPTER_SPACING: f64 = 10.0;

/// Turns raw `(token, title)` pairs into valid chapters: indices outside the
/// script and empty titles are dropped, every index is snapped to the start
/// of the sentence containing it, and the result is sorted, de-duplicated and
/// thinned so that chapters are at least [`MIN_CHAPTER_SPACING`] apart.
pub fn validate_chapters(comp: &Composition, raw: Vec<(usize, String)>) -> Vec<Chapter> {
    let sentences = context::sentences(comp);
    let mut out: Vec<Chapter> = Vec::new();
    for (idx, title) in raw {
        let title: String = clean_line(&title).chars().take(100).collect();
        if title.is_empty() {
            continue;
        }
        let Some(token) = context::snap_to_sentence_start(comp, &sentences, idx) else { continue };
        out.push(Chapter { token, time: comp.time_of_token(token), title });
    }
    out.sort_by_key(|c| c.token);
    out.dedup_by_key(|c| c.token);
    let mut kept: Vec<Chapter> = Vec::new();
    for c in out {
        if kept.last().is_none_or(|p| c.time - p.time >= MIN_CHAPTER_SPACING) {
            kept.push(c);
        }
    }
    kept
}

/// Proposes chapter markers. Every chapter starts at the first word of a
/// sentence; chapters are in order and at least [`MIN_CHAPTER_SPACING`] apart.
pub fn chapters(client: &Client, editor: &Editor) -> Result<Vec<Chapter>> {
    let s = script_of(editor)?;
    let schema = object_schema(
        "chapters",
        json!({
            "type": "object",
            "properties": {
                "sentence": {"type": "integer", "description": "Number of the sentence (the N of SN) where the chapter begins."},
                "title": {"type": "string"}
            },
            "required": ["sentence", "title"],
            "additionalProperties": false
        }),
    );
    #[derive(Deserialize)]
    struct Item {
        sentence: i64,
        title: String,
    }
    #[derive(Deserialize)]
    struct Out {
        chapters: Vec<Item>,
    }
    let minutes = s.comp.script_duration() / 60.0;
    let task = format!(
        "Task: divide this recording into chapters for a video description. Start a chapter where the topic really changes; the first chapter begins at S1. \
         For a recording of about {minutes:.0} minutes, use roughly one chapter every two to five minutes of content, at least two chapters when there is more than one topic, and never chapters closer than 10 seconds. \
         Give each a short title (2 to 6 words) naming what is discussed there. Answer as JSON, with `sentence` the number of the sentence that opens the chapter."
    );
    let messages = transcript_messages(&s.block, &task);
    let (out, _): (Out, String) = json_task(client, client.model(), SYSTEM, &messages, &schema, Effort::Medium)?;
    let n = s.sentences.len() as i64;
    let raw: Vec<(usize, String)> = out.chapters.into_iter().filter(|c| (1..=n).contains(&c.sentence)).map(|c| (s.sentences[(c.sentence - 1) as usize].start, c.title)).collect();
    let chapters = validate_chapters(s.comp, raw);
    if chapters.is_empty() {
        return Err(AiError::InvalidOutput("no usable chapters came back".into()));
    }
    Ok(chapters)
}

/// `SetMarker` commands for accepted chapters. Markers do not shift token indices.
pub fn apply_chapters(chapters: &[Chapter]) -> Vec<Command> {
    chapters.iter().map(|c| Command::SetMarker { at: c.token, name: Some(c.title.clone()) }).collect()
}

// ---------------------------------------------------------------------------
// Clips
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ClipProposal {
    /// First token (the first word of a sentence).
    pub start: usize,
    /// One past the last token (just after the last word of a sentence).
    pub end: usize,
    pub title: String,
    pub reason: String,
    /// Playing length in seconds.
    pub duration: f64,
}

/// Validates clip candidates against the composition. Each candidate is
/// widened to whole sentences, then rejected if it is out of bounds or empty,
/// shorter than `min_secs` or longer than `max_secs`, or overlaps a clip
/// accepted before it. Returns the accepted clips (at most `count`, in the
/// given order) and a note for every rejection.
pub fn validate_clips(comp: &Composition, raw: Vec<ClipProposal>, count: usize, min_secs: f64, max_secs: f64) -> (Vec<ClipProposal>, Vec<String>) {
    let sentences = context::sentences(comp);
    let len = comp.script.len();
    let mut ok: Vec<ClipProposal> = Vec::new();
    let mut rejected = Vec::new();
    for c in raw {
        let title = clean_line(&c.title);
        let name = if title.is_empty() { format!("{}..{}", c.start, c.end) } else { title.clone() };
        if c.start >= c.end || c.end > len {
            rejected.push(format!("{name:?}: the range {}..{} is not inside the script", c.start, c.end));
            continue;
        }
        let Some((start, end)) = crate::agent_tools::snap_to_sentences(&sentences, c.start, c.end) else {
            rejected.push(format!("{name:?}: the range contains no spoken text"));
            continue;
        };
        let duration = comp.range_duration(start..end);
        if duration < min_secs - 1e-6 || duration > max_secs + 1e-6 {
            rejected.push(format!("{name:?}: it is {duration:.1}s long, outside {min_secs:.0}–{max_secs:.0}s"));
            continue;
        }
        if ok.iter().any(|o| start < o.end && o.start < end) {
            rejected.push(format!("{name:?}: it overlaps another clip"));
            continue;
        }
        if ok.len() < count {
            let title = if title.is_empty() { format!("Clip {}", ok.len() + 1) } else { title.chars().take(100).collect() };
            ok.push(ClipProposal { start, end, title, reason: clean_line(&c.reason), duration });
        }
    }
    (ok, rejected)
}

/// Proposes up to `count` clips of `min_secs`..`max_secs` that stand on
/// their own, optionally about `topic`. Clips are whole sentences, within
/// the duration bounds and do not overlap; fewer than `count` are returned
/// when the recording does not contain that many.
pub fn find_clips(client: &Client, editor: &Editor, count: usize, min_secs: f64, max_secs: f64, topic: Option<&str>) -> Result<Vec<ClipProposal>> {
    if !(min_secs.is_finite() && max_secs.is_finite()) || min_secs < 0.0 || max_secs <= min_secs {
        return Err(AiError::NothingToDo("The clip length range is not valid.".into()));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let count = count.min(30);
    let s = script_of(editor)?;
    let total = s.comp.script_duration();
    if total < min_secs {
        return Err(AiError::NothingToDo(format!("The recording is {total:.0} seconds long, shorter than the minimum clip length.")));
    }
    let schema = object_schema(
        "clips",
        json!({
            "type": "object",
            "properties": {
                "first_sentence": {"type": "integer", "description": "Number of the first sentence of the clip."},
                "last_sentence": {"type": "integer", "description": "Number of the last sentence of the clip (inclusive)."},
                "title": {"type": "string"},
                "reason": {"type": "string", "description": "One sentence on why this works as a standalone clip."}
            },
            "required": ["first_sentence", "last_sentence", "title", "reason"],
            "additionalProperties": false
        }),
    );
    #[derive(Deserialize)]
    struct Item {
        first_sentence: i64,
        last_sentence: i64,
        title: String,
        reason: String,
    }
    #[derive(Deserialize)]
    struct Out {
        clips: Vec<Item>,
    }
    let mut task = format!(
        "Task: pick the {count} best passages of this recording to publish as standalone short clips, best first. \
         Each clip is a run of consecutive whole sentences, from `first_sentence` to `last_sentence` inclusive, and must last between {min_secs:.0} and {max_secs:.0} seconds: \
         a clip's length is the timecode of the sentence after its last sentence minus the timecode of its first sentence (the recording ends at {}). Check each length against the timecodes before answering. \
         A good clip makes sense without the rest of the recording, starts on a clear opening and ends on a complete thought. Clips must not overlap. \
         If fewer than {count} passages qualify, return fewer. Give each a short title and a one-sentence reason. Answer as JSON.",
        timecode(total)
    );
    if let Some(t) = topic.map(str::trim).filter(|t| !t.is_empty()) {
        let _ = write!(task, "\nOnly pick passages about this topic: {t}. If nothing in the recording is about it, return an empty list.");
    }
    let messages = transcript_messages(&s.block, &task);
    let n = s.sentences.len() as i64;
    let to_raw = |items: Vec<Item>| -> (Vec<ClipProposal>, usize) {
        let total = items.len();
        let raw = items
            .into_iter()
            .filter(|c| (1..=n).contains(&c.first_sentence) && (1..=n).contains(&c.last_sentence) && c.first_sentence <= c.last_sentence)
            .map(|c| ClipProposal {
                start: s.sentences[(c.first_sentence - 1) as usize].start,
                end: s.sentences[(c.last_sentence - 1) as usize].end,
                title: c.title,
                reason: c.reason,
                duration: 0.0,
            })
            .collect::<Vec<_>>();
        let bad = total - raw.len();
        (raw, bad)
    };
    let (out, raw_text): (Out, String) = json_task(client, client.model(), SYSTEM, &messages, &schema, Effort::Medium)?;
    let (raw, bad_refs) = to_raw(out.clips);
    let (mut clips, mut rejected) = validate_clips(s.comp, raw, count, min_secs, max_secs);
    if bad_refs > 0 {
        rejected.push(format!("{bad_refs} clip(s) referred to sentence numbers that do not exist (valid: 1 to {n})"));
    }
    if clips.len() < count && !rejected.is_empty() {
        // One corrective round: tell the model exactly what was wrong.
        let kept: Vec<String> = clips.iter().map(|c| format!("{:?} ({:.1}s)", c.title, c.duration)).collect();
        let feedback = format!(
            "Some of those could not be used:\n- {}\nKept so far: {}.\nAnswer again with the complete list of up to {count} clips: keep the usable ones and replace the others with passages that last between {min_secs:.0} and {max_secs:.0} seconds, are whole sentences, and do not overlap. Return fewer if nothing else qualifies.",
            rejected.join("\n- "),
            if kept.is_empty() { "none".to_string() } else { kept.join(", ") }
        );
        let (again, _): (Out, String) = json_task(client, client.model(), SYSTEM, &with_feedback(messages, &raw_text, &feedback), &schema, Effort::Medium)?;
        let (raw2, _) = to_raw(again.clips);
        // Clips already accepted keep their place; new candidates fill the remaining slots.
        let mut merged = clips.clone();
        merged.extend(raw2);
        clips = validate_clips(s.comp, merged, count, min_secs, max_secs).0;
    }
    Ok(clips)
}

/// Commands that create one composition per accepted clip from composition
/// `source` (the composition the clips were proposed for, which must be
/// active when the commands are applied). The source stays selected.
pub fn apply_clips(source: CompositionId, clips: &[ClipProposal]) -> Vec<Command> {
    let mut cmds = Vec::with_capacity(clips.len() * 2);
    for c in clips {
        cmds.push(Command::CompositionFromRange { start: c.start, end: c.end, name: c.title.clone() });
        // Creating a composition selects it; go back so the next range is taken from the source.
        cmds.push(Command::SelectComposition { id: source });
    }
    cmds
}

// ---------------------------------------------------------------------------
// Edit for clarity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intensity {
    /// Only what is clearly a mistake: false starts, abandoned sentences, stutters.
    Low,
    /// Also repetition and verbal throat-clearing.
    Medium,
    /// Also tangents and asides that do not serve the main thread.
    Heavy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClarityEdit {
    pub start: usize,
    /// Exclusive.
    pub end: usize,
    pub reason: String,
}

/// Validates clarity edits against the composition: ranges outside the
/// script, empty ranges and ranges without any playing word are dropped;
/// leading and trailing silence is trimmed off each range; overlapping and
/// touching ranges are merged. Returned in script order.
pub fn validate_clarity_edits(comp: &Composition, raw: Vec<ClarityEdit>) -> Vec<ClarityEdit> {
    let len = comp.script.len();
    let mut out: Vec<ClarityEdit> = Vec::new();
    for e in raw {
        if e.start >= e.end || e.end > len {
            continue;
        }
        let playing = |i: usize| comp.script[i].shows_text();
        let (Some(first), Some(last)) = ((e.start..e.end).find(|&i| playing(i)), (e.start..e.end).rfind(|&i| playing(i))) else { continue };
        out.push(ClarityEdit { start: first, end: last + 1, reason: clean_line(&e.reason) });
    }
    out.sort_by_key(|e| (e.start, e.end));
    let mut merged: Vec<ClarityEdit> = Vec::new();
    for e in out {
        match merged.last_mut() {
            Some(last) if e.start <= last.end => {
                if e.end > last.end {
                    last.end = e.end;
                }
                if !e.reason.is_empty() && !last.reason.contains(&e.reason) {
                    if !last.reason.is_empty() {
                        last.reason.push_str("; ");
                    }
                    last.reason.push_str(&e.reason);
                }
            }
            _ => merged.push(e),
        }
    }
    merged
}

/// Words per request when a long script is processed in several windows.
const CLARITY_WINDOW_WORDS: usize = 1800;

/// Proposes passages to strike through so the recording is clearer: false
/// starts and retakes, and with higher intensity repetition and tangents.
/// Nothing is rewritten; the proposal only removes.
pub fn edit_for_clarity(client: &Client, editor: &Editor, intensity: Intensity) -> Result<Vec<ClarityEdit>> {
    let comp = editor.comp().ok_or_else(|| AiError::NothingToDo("No composition is open.".into()))?;
    let total_words = comp.word_count(0..comp.script.len());
    if total_words == 0 {
        return Err(AiError::NothingToDo("The script has no words yet. Transcribe a recording first.".into()));
    }
    let level = match intensity {
        Intensity::Low => "LOW. Remove only what is clearly a slip: false starts and abandoned sentences that are restarted, stutters, and words repeated by mistake. Keep everything else, including digressions.",
        Intensity::Medium => "MEDIUM. Remove false starts, abandoned sentences, stutters and repeated words; also remove passages that repeat a point already made and empty lead-ins (\"so, yeah, anyway, like I said\"). Keep the content and the speakers' personality.",
        Intensity::Heavy => "HEAVY. Remove false starts, stutters and repetition, and also tangents, asides and small talk that do not serve the main thread, so that only the substance remains. Each speaker's points must still make sense.",
    };
    let system = format!(
        "You tighten spoken recordings inside Mango Loco, an editor in which removing words from the transcript removes them from the audio. \
         You are given part of a transcript in which every word is prefixed with its token index, as `N:word`, one paragraph per line (`¶ start..end time speaker:`). \
         `(N: 1.8s pause)` marks a silence, and `~~ … ~~` is text that is already removed: leave it alone.\n\
         Choose ranges of tokens to remove. Intensity: {level}\n\
         Rules: a range is `start` (index of the first word to remove) and `end` (index one past the last word to remove). \
         You can only remove; you cannot reword. After the removals the remaining words must read as correct, complete sentences, so cut at clause boundaries, \
         remove the abandoned first attempt rather than the final wording, and never remove a word that the rest of its sentence needs. \
         If nothing should be removed, return an empty list. Give a few words of reason for each range. Answer as JSON."
    );
    let schema = object_schema(
        "edits",
        json!({
            "type": "object",
            "properties": {
                "start": {"type": "integer", "description": "Token index of the first word to remove."},
                "end": {"type": "integer", "description": "Token index one past the last word to remove."},
                "reason": {"type": "string"}
            },
            "required": ["start", "end", "reason"],
            "additionalProperties": false
        }),
    );
    #[derive(Deserialize)]
    struct Item {
        start: i64,
        end: i64,
        reason: String,
    }
    #[derive(Deserialize)]
    struct Out {
        edits: Vec<Item>,
    }
    let retakes = detect::find_retakes(comp, 2, 60);
    let len = comp.script.len();
    let mut all: Vec<ClarityEdit> = Vec::new();
    let mut start = 0usize;
    while start < len {
        let end = context::page_end(comp, start, CLARITY_WINDOW_WORDS);
        if comp.word_count(start..end) > 0 {
            let mut prompt = context::describe_script(comp, &editor.project, start..end);
            // Detector findings are hints: the model decides, but should not miss these.
            let hints: Vec<String> = retakes
                .iter()
                .filter(|h| h.earlier.start >= start && h.earlier.end <= end)
                .take(40)
                .map(|h| format!("- {}..{} {:?} is said again right after, at {}..{}", h.earlier.start, h.earlier.end, h.text, h.later.start, h.later.end))
                .collect();
            if !hints.is_empty() {
                let _ = write!(prompt, "\nPossible false starts found by a simple detector (verify each before using it):\n{}\n", hints.join("\n"));
            }
            let messages = vec![json!({"role": "user", "content": prompt})];
            let (out, _): (Out, String) = json_task(client, client.model(), &system, &messages, &schema, Effort::Medium)?;
            for e in out.edits {
                // Only ranges inside the window that was shown are considered.
                if e.start >= start as i64 && e.end <= end as i64 && e.start < e.end {
                    all.push(ClarityEdit { start: e.start as usize, end: e.end as usize, reason: e.reason });
                }
            }
        }
        start = end.max(start + 1);
    }
    let edits = validate_clarity_edits(comp, all);
    let removed: usize = edits.iter().map(|e| comp.word_count(e.start..e.end)).sum();
    if removed * 10 > total_words * 8 {
        return Err(AiError::InvalidOutput("the proposal would remove almost the whole recording".into()));
    }
    Ok(edits)
}

/// `IgnoreRange` commands for accepted clarity edits: the text is struck
/// through, not deleted, so the user can review and restore each one. These
/// do not shift token indices, so any subset can be applied in any order.
pub fn apply_clarity_edits(edits: &[ClarityEdit]) -> Vec<Command> {
    edits.iter().map(|e| Command::IgnoreRange { start: e.start, end: e.end, ignored: true }).collect()
}

// ---------------------------------------------------------------------------
// Translation and writing
// ---------------------------------------------------------------------------

/// Cues per translation request.
const TRANSLATE_BATCH: usize = 80;

/// Translates caption cues. The result has exactly one translation per input
/// cue, in the same order. Cue boundaries are kept (each cue is translated on
/// its own, with the neighbouring cues as context), so timing still matches.
/// Empty cues stay empty. Fails with [`AiError::InvalidOutput`] if the model
/// does not return a translation for every cue after one retry.
pub fn translate_captions(client: &Client, cue_texts: &[String], target_language: &str) -> Result<Vec<String>> {
    let lang = target_language.trim();
    if lang.is_empty() {
        return Err(AiError::NothingToDo("Choose a language to translate to.".into()));
    }
    let system = format!(
        "You translate subtitles into {lang}. You are given consecutive subtitle cues of one recording as a JSON list of objects with `id` and `text`. \
         Translate the text of every cue into natural, idiomatic {lang} as it would be subtitled. \
         Return exactly one translation per cue with the same `id`: never merge, split, drop or reorder cues, even when a sentence runs across several cues \
         (then spread the translated sentence across those cues in order). Keep each translation about as long as the original, keep names and numbers, \
         and where a cue has a line break (\\n), keep one in the translation. Text already in {lang} is returned unchanged. Answer as JSON."
    );
    let schema = object_schema(
        "translations",
        json!({
            "type": "object",
            "properties": {"id": {"type": "integer"}, "text": {"type": "string"}},
            "required": ["id", "text"],
            "additionalProperties": false
        }),
    );
    #[derive(Deserialize)]
    struct Item {
        id: i64,
        text: String,
    }
    #[derive(Deserialize)]
    struct Out {
        translations: Vec<Item>,
    }
    let mut result: Vec<Option<String>> = cue_texts.iter().map(|t| if t.trim().is_empty() { Some(String::new()) } else { None }).collect();
    let todo: Vec<usize> = (0..cue_texts.len()).filter(|&i| result[i].is_none()).collect();
    for batch in todo.chunks(TRANSLATE_BATCH) {
        let cues: Vec<Value> = batch.iter().map(|&i| json!({"id": i + 1, "text": cue_texts[i]})).collect();
        let prompt = format!("Translate these {} cues into {lang}:\n{}", batch.len(), serde_json::to_string_pretty(&cues).unwrap_or_default());
        let mut messages = vec![json!({"role": "user", "content": prompt})];
        for attempt in 0..2 {
            let (out, raw): (Out, String) = json_task(client, client.fast_model(), &system, &messages, &schema, Effort::Low)?;
            let returned = out.translations.len();
            for t in out.translations {
                // Only ids that were asked for in this batch are accepted; the first answer for an id wins.
                let Some(i) = usize::try_from(t.id - 1).ok().filter(|i| batch.contains(i)) else { continue };
                if result[i].is_none() && !t.text.trim().is_empty() {
                    result[i] = Some(t.text.trim().to_string());
                }
            }
            let missing: Vec<usize> = batch.iter().copied().filter(|&i| result[i].is_none()).collect();
            if missing.is_empty() {
                break;
            }
            if attempt == 1 {
                return Err(AiError::InvalidOutput(format!(
                    "the translation came back with {returned} cues for {} (cue {} is missing); nothing was changed",
                    batch.len(),
                    missing[0] + 1
                )));
            }
            let ids: Vec<String> = missing.iter().map(|i| (i + 1).to_string()).collect();
            messages = with_feedback(
                messages,
                &raw,
                &format!("That has {returned} translations for {} cues. Translations are missing for the cues with id {}. Return translations for exactly those ids, one each.", batch.len(), ids.join(", ")),
            );
        }
    }
    Ok(result.into_iter().map(Option::unwrap_or_default).collect())
}

/// Translates free text, keeping its paragraphs and formatting.
pub fn translate_text(client: &Client, text: &str, target_language: &str) -> Result<String> {
    let lang = target_language.trim();
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    if lang.is_empty() {
        return Err(AiError::NothingToDo("Choose a language to translate to.".into()));
    }
    writer_task(
        client,
        client.fast_model(),
        format!("Translate the text between the <text> tags into {lang}. Keep the paragraphs, line breaks and any Markdown formatting, keep names and numbers, and return only the translation.\n<text>\n{text}\n</text>"),
        Effort::Low,
    )
}

/// Rewrites `text` following `instruction` (for example "make it shorter" or "more formal").
pub fn rewrite(client: &Client, text: &str, instruction: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(AiError::NothingToDo("There is no text to rewrite.".into()));
    }
    let instruction = if instruction.trim().is_empty() { "Improve the clarity and flow without changing the meaning." } else { instruction.trim() };
    writer_task(
        client,
        client.model(),
        format!("Rewrite the text between the <text> tags. Instruction: {instruction}\nKeep the meaning and the facts, keep the language of the text, and return only the rewritten text.\n<text>\n{text}\n</text>"),
        Effort::Medium,
    )
}

fn require_prompt(prompt: &str) -> Result<&str> {
    let p = prompt.trim();
    if p.is_empty() { Err(AiError::NothingToDo("Describe what you want first.".into())) } else { Ok(p) }
}

/// Writes a script to be read aloud, from a description of what it should cover.
pub fn write_script(client: &Client, prompt: &str) -> Result<String> {
    let p = require_prompt(prompt)?;
    writer_task(
        client,
        client.model(),
        format!("Write a script to be spoken aloud for a podcast or video, in plain text with short paragraphs and natural spoken phrasing, with no stage directions or headings unless asked for. If facts are needed that the brief does not give, leave a clearly marked [placeholder] instead of making them up.\nBrief: {p}"),
        Effort::Medium,
    )
}

/// An outline (Markdown list) for an episode or video.
pub fn outline(client: &Client, prompt: &str) -> Result<String> {
    let p = require_prompt(prompt)?;
    writer_task(
        client,
        client.model(),
        format!("Write an outline for a podcast episode or video as a Markdown list: the main sections in order, each with two to four short sub-points to cover. Keep it to one screen.\nBrief: {p}"),
        Effort::Medium,
    )
}

/// A list of ideas (Markdown list) on a subject.
pub fn brainstorm(client: &Client, prompt: &str) -> Result<String> {
    let p = require_prompt(prompt)?;
    writer_task(
        client,
        client.model(),
        format!("Brainstorm ideas for a podcaster or video creator as a Markdown list of 10 varied, specific ideas, one line each with a few words on the angle.\nSubject: {p}"),
        Effort::Medium,
    )
}

// ---------------------------------------------------------------------------
// Speaker labels
// ---------------------------------------------------------------------------

/// Turns raw `(token, name)` speaker turns into a valid labelling: tokens are
/// snapped to sentence starts, out-of-range entries dropped, the first turn
/// is moved to the first sentence, and consecutive turns by the same speaker
/// are merged. Names are trimmed; an empty name becomes "Speaker".
pub fn validate_speaker_turns(comp: &Composition, raw: Vec<(usize, String)>) -> Vec<(usize, String)> {
    let sentences = context::sentences(comp);
    let mut turns: Vec<(usize, String)> = raw
        .into_iter()
        .filter_map(|(idx, name)| {
            let name = clean_line(&name).trim_end_matches(':').chars().take(40).collect::<String>();
            context::snap_to_sentence_start(comp, &sentences, idx).map(|t| (t, if name.is_empty() { "Speaker".to_string() } else { name }))
        })
        .collect();
    turns.sort_by_key(|t| t.0);
    turns.dedup_by_key(|t| t.0);
    if let (Some(first), Some(s0)) = (turns.first_mut(), sentences.first()) {
        first.0 = s0.start;
    }
    turns.dedup_by(|b, a| a.1 == b.1);
    turns
}

/// Guesses who is speaking in a recording that has no speaker labels (for
/// example a single-file interview).
///
/// **This is a guess from the wording alone** — who asks, who answers, how
/// people address each other — not from the voices. It cannot tell apart
/// speakers who talk alike, and short interjections are often attributed to
/// the wrong person. Present the result to the user as a suggestion.
///
/// Returns speaker turns as `(token index where the turn starts, speaker
/// name)`, in order; each turn runs to the start of the next. Names are real
/// names when the transcript reveals them ("I'm Ana"), otherwise
/// "Speaker 1", "Speaker 2", …
pub fn label_speakers(client: &Client, editor: &Editor) -> Result<Vec<(usize, String)>> {
    let comp = editor.comp().ok_or_else(|| AiError::NothingToDo("No composition is open.".into()))?;
    let sentences = context::sentences(comp);
    if sentences.is_empty() {
        return Err(AiError::NothingToDo("The script has no words yet. Transcribe a recording first.".into()));
    }
    let system = "You are given the transcript of a recording as numbered sentences, without speaker labels. Work out from the wording who says each sentence. \
                  First decide how many people are talking. Use as few speakers as the wording supports: an interview is normally two people, a host who introduces the show and asks, and a guest who answers. \
                  Never add a speaker for a short reply, a change of topic or a pause; consecutive sentences belong to the same person unless the wording shows a change, such as a question followed by its answer, a greeting followed by its reply, or one person addressing the other. \
                  Then name them. Someone who introduces themselves (\"I'm Ana\") has that name. A name used to address someone (\"Ben, welcome\", \"Thanks, Ana\") is the name of the listener, not of the person saying it. \
                  Only when the transcript gives no name for a person, call them \"Speaker 1\", \"Speaker 2\" and so on, consistently. \
                  Return the speaker turns in order: for each turn, the number of its first sentence and the speaker. A turn continues until the next turn starts; the first turn starts at sentence 1. \
                  If the whole recording is clearly one person, return a single turn. Answer as JSON.";
    let schema = object_schema(
        "turns",
        json!({
            "type": "object",
            "properties": {
                "first_sentence": {"type": "integer", "description": "Number of the sentence where this speaker starts talking."},
                "speaker": {"type": "string"}
            },
            "required": ["first_sentence", "speaker"],
            "additionalProperties": false
        }),
    );
    #[derive(Deserialize)]
    struct Item {
        first_sentence: i64,
        speaker: String,
    }
    #[derive(Deserialize)]
    struct Out {
        turns: Vec<Item>,
    }
    let prompt = format!("<transcript sentences=\"{}\">\n{}</transcript>", sentences.len(), context::render_sentences(&editor.project, &sentences, false));
    let messages = vec![json!({"role": "user", "content": prompt})];
    // Attributing turns from wording alone is the hardest judgement any of these tools makes: think harder.
    let (out, _): (Out, String) = json_task(client, client.model(), system, &messages, &schema, Effort::High)?;
    let n = sentences.len() as i64;
    let raw: Vec<(usize, String)> = out.turns.into_iter().filter(|t| (1..=n).contains(&t.first_sentence)).map(|t| (sentences[(t.first_sentence - 1) as usize].start, t.speaker)).collect();
    let turns = validate_speaker_turns(comp, raw);
    if turns.is_empty() {
        return Err(AiError::InvalidOutput("no usable speaker turns came back".into()));
    }
    Ok(turns)
}

/// `SetSpeaker` commands for accepted speaker turns, for a script of
/// `script_len` tokens. Each turn is labelled up to the start of the next.
pub fn apply_speaker_labels(script_len: usize, turns: &[(usize, String)]) -> Vec<Command> {
    let mut cmds = Vec::with_capacity(turns.len());
    for (k, (start, name)) in turns.iter().enumerate() {
        let end = turns.get(k + 1).map(|t| t.0).unwrap_or(script_len);
        if *start < end && end <= script_len {
            cmds.push(Command::SetSpeaker { start: *start, end, speaker: Some(name.clone()) });
        }
    }
    cmds
}
