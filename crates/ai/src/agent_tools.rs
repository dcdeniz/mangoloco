//! The tools the chat assistant can call, and their execution against an
//! editor.
//!
//! # Token indices cannot go stale
//!
//! Token indices shift whenever tokens are added or removed. Two rules make
//! that impossible to get wrong:
//!
//! 1. **Revision check.** Every script rendering states a revision
//!    ([`crate::context::script_rev`]). Every tool that takes token indices
//!    requires the revision those indices were read at and refuses to run if
//!    the script has shifted since. The result of every edit says whether
//!    indices shifted and gives the new revision.
//! 2. **Batch-relative addressing.** Inside one call, *all* indices refer to
//!    the script as it was at that revision, no matter how many earlier
//!    commands in the same call removed or inserted tokens. The tool tracks
//!    where each original token ended up and translates every later command.
//!    A command that addresses a token an earlier command destroyed is an
//!    error, and the whole call is rolled back.

use crate::context::{self, Sentence};
use mango_core::detect::{self, FillerKind, FillerOptions};
use mango_core::editor::RemoveMode;
use mango_core::script::FindOptions;
use mango_core::transcript_export::timecode;
use mango_core::{
    Animation, AspectPreset, CaptionHighlight, Command, Composition, CompositionId, Editor, Layer, LayerContent, LayerId, Rgba, TextAlign, TextStyle,
    TokenKind, Transform, Transition, script,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt::Write as _;

pub(crate) struct ToolOutcome {
    /// What the model reads.
    pub text: String,
    pub ok: bool,
    /// Commands applied to the editor by this call.
    pub commands: usize,
    /// One line for the UI.
    pub summary: String,
}

impl ToolOutcome {
    fn info(text: String, summary: impl Into<String>) -> Self {
        Self { text, ok: true, commands: 0, summary: summary.into() }
    }
    fn error(text: impl Into<String>) -> Self {
        let text = text.into();
        let first = text.lines().next().unwrap_or("").to_string();
        Self { text: format!("ERROR: {text}"), ok: false, commands: 0, summary: first }
    }
}

// ---------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------

/// Example commands for `apply_commands`, one per line of its reference. Each
/// must deserialise into a [`Command`]; a test checks that.
pub(crate) const COMMAND_EXAMPLES: &[(&str, &str)] = &[
    (r#"{"cmd":"ignore_range","start":14,"end":17,"ignored":true}"#, "strike through tokens 14,15,16 (ignored:false un-ignores). Reversible; preferred for trims."),
    (r#"{"cmd":"delete_range","start":14,"end":17}"#, "remove tokens from the script. Shifts indices."),
    (r#"{"cmd":"restore_range","start":10,"end":40}"#, "un-ignore the range and bring back media deleted inside it."),
    (r#"{"cmd":"restore_at","at":58}"#, "bring back media deleted just before token 58 (a word shown with ✂)."),
    (r#"{"cmd":"remove_ignored"}"#, "permanently delete everything that is struck through."),
    (r#"{"cmd":"replace_with_gap","start":14,"end":17}"#, "replace the range with silence of the same length (keeps timing)."),
    (r#"{"cmd":"insert_gap","at":20,"secs":0.5}"#, "insert silence before token 20."),
    (r#"{"cmd":"set_gap_duration","at":7,"secs":0.4}"#, "set the length of the pause token 7."),
    (r#"{"cmd":"move_range","start":14,"end":30,"dest":120}"#, "move the range so it begins before the token currently at 120."),
    (r#"{"cmd":"correct_range","start":14,"end":16,"text":"going to"}"#, "fix the transcript text of these words (media unchanged). Only for transcription mistakes."),
    (r#"{"cmd":"correct_all","from":"jon","to":"John"}"#, "fix one misrecognised word everywhere."),
    (r#"{"cmd":"hide_text","start":14,"end":17,"hidden":true}"#, "keep the media, hide the words from captions and exports."),
    (r#"{"cmd":"set_speaker","start":0,"end":58,"speaker":"Ana"}"#, "label who speaks (null clears)."),
    (r#"{"cmd":"rename_speaker","from":"Speaker 1","to":"Ana"}"#, ""),
    (r#"{"cmd":"set_paragraph_break","at":58,"on":true}"#, ""),
    (r#"{"cmd":"set_scene_break","at":58,"on":true}"#, "start a new scene at token 58 (transitions happen at scene breaks)."),
    (r#"{"cmd":"set_marker","at":58,"name":"Pricing"}"#, "chapter marker before token 58 (name null removes). Prefer the add_chapters tool."),
    (r#"{"cmd":"set_note","at":58,"note":"laughter"}"#, ""),
    (r#"{"cmd":"set_highlight","start":14,"end":17,"color":0}"#, "highlight colour index 0-5 (null clears)."),
    (r#"{"cmd":"set_style","start":14,"end":17,"bold":true,"italic":null}"#, ""),
    (r#"{"cmd":"remove_matches","query":"you know what","mode":"Ignore"}"#, "every occurrence of a phrase; mode is \"Ignore\", \"Delete\", \"ReplaceWithGap\" or \"HideText\"."),
    (r#"{"cmd":"split_at","time":12.5}"#, "split the token playing at 12.5 s."),
    (r#"{"cmd":"insert_asset","asset":"00000000-0000-0000-0000-000000000000","at":null}"#, "append a file (id from FILES) to the script, or insert before token `at`."),
    (r#"{"cmd":"new_composition","name":"Teaser"}"#, ""),
    (r#"{"cmd":"duplicate_composition","name":"Vertical cut"}"#, "copy the active composition and switch to the copy."),
    (r#"{"cmd":"rename_composition","name":"Episode 12"}"#, ""),
    (r#"{"cmd":"select_composition","id":"00000000-0000-0000-0000-000000000000"}"#, "switch the active composition (ids are in the overview)."),
    (r#"{"cmd":"delete_composition"}"#, "delete the active composition."),
    (r#"{"cmd":"set_fps","fps":30}"#, ""),
    (r#"{"cmd":"set_background","color":{"r":0,"g":0,"b":0,"a":255}}"#, ""),
    (r#"{"cmd":"set_master_loudness","lufs":-16}"#, "normalise loudness (null turns it off)."),
    (r#"{"cmd":"remove_layer","id":"00000000-0000-0000-0000-000000000000"}"#, ""),
    (r#"{"cmd":"duplicate_layer","id":"00000000-0000-0000-0000-000000000000"}"#, ""),
    (r#"{"cmd":"split_layer","id":"00000000-0000-0000-0000-000000000000","time":4.0}"#, ""),
    (r#"{"cmd":"reorder_layer","id":"00000000-0000-0000-0000-000000000000","delta":1}"#, "move a layer up (+) or down (-) in the stack."),
    (r#"{"cmd":"add_comment","time":12.5,"author":"Assistant","text":"Check this cut"}"#, ""),
    (r#"{"cmd":"rename_project","name":"Episode 12"}"#, ""),
];

fn command_reference() -> String {
    let mut s = String::new();
    for (ex, note) in COMMAND_EXAMPLES {
        s.push_str(ex);
        if !note.is_empty() {
            s.push_str("  — ");
            s.push_str(note);
        }
        s.push('\n');
    }
    s
}

const REV_DESC: &str = "The script revision (\"rev\") shown where you read the indices. The call is refused if the script has shifted since.";

fn range_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "start": {"type": "integer", "description": format!("Index of the first token {what}.")},
            "end": {"type": "integer", "description": "Index one past the last token (exclusive). To cover the words shown as 14:… to 16:…, use start 14 and end 17."}
        },
        "required": ["start", "end"],
        "additionalProperties": false
    })
}

/// The tool list. Deterministic: it is part of the cached prompt prefix.
pub(crate) fn definitions(eager_input_streaming: bool) -> Value {
    let color = "Colour as \"#RRGGBB\" (or a basic name such as \"yellow\").";
    let mut tools = json!([
        {
            "name": "get_project_overview",
            "description": "Returns the current state of the project: composition, canvas, speakers, captions, layers, markers, other compositions, files, and the script (complete when short, a paragraph outline when long). You are given this at the start of every user turn; call it again only when you need a fresh view after edits.",
            "input_schema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "read_script",
            "description": "Reads the script with the token index of every word, for the given token range (one page of about 1500 words at most; the result says where to continue). Call this to get exact indices before an index-based edit when you only have the outline, and to re-read after an edit that shifted indices.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "start": {"type": "integer", "description": "First token index to show. Default 0."},
                    "end": {"type": "integer", "description": "One past the last token index to show. Default: one page from start."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "search_script",
            "description": "Finds every occurrence of a word or phrase in the script (case-insensitive, punctuation ignored) and returns each hit's token range with surrounding words. Use it to locate what the user refers to, and to check whether something is in the script at all before saying it is not.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "A word or a short exact phrase as it would be spoken."},
                    "include_ignored": {"type": "boolean", "description": "Also search struck-through text. Default true."}
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_fillers",
            "description": "Lists the filler words the detector finds in the playing script (um, uh, \"you know\", \"I mean\", immediate repeats like \"the the\"), with token ranges. Use it when the user asks what fillers there are or how many. To remove them, call remove_fillers instead of removing them one by one.",
            "input_schema": {
                "type": "object",
                "properties": {"loose": {"type": "boolean", "description": "Also list context-dependent fillers (like, basically, actually, so, right) at clause edges. Default false."}},
                "additionalProperties": false
            }
        },
        {
            "name": "list_retakes",
            "description": "Lists false starts and retakes the detector finds: a run of words that is said and then said again shortly after. Each hit gives the abandoned earlier take (the part to remove) and the later take that replaces it.",
            "input_schema": {
                "type": "object",
                "properties": {"min_words": {"type": "integer", "description": "Shortest repeated run to report. Default 3."}},
                "additionalProperties": false
            }
        },
        {
            "name": "list_gaps",
            "description": "Lists silences (pauses) at least min_secs long, with token ranges, lengths and the words around them.",
            "input_schema": {
                "type": "object",
                "properties": {"min_secs": {"type": "number", "description": "Default 1.0."}},
                "additionalProperties": false
            }
        },
        {
            "name": "remove_fillers",
            "description": "Removes every filler word the detector finds (um, uh, and by default filler phrases and immediate repeats) in one step. Call this when the user asks to remove or clean up filler words; it is exhaustive and exact, unlike picking them out by hand.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["delete", "ignore"], "description": "delete (default) cuts them; ignore strikes them through so the user can review."},
                    "phrases": {"type": "boolean", "description": "Include \"you know\", \"I mean\", \"sort of\", \"kind of\". Default true."},
                    "repeats": {"type": "boolean", "description": "Include immediate repeats (\"the the\"). Default true."},
                    "loose": {"type": "boolean", "description": "Include like/basically/actually/so/right/okay/well at clause edges. Default false; only when the user asks for an aggressive clean-up."},
                    "custom": {"type": "array", "items": {"type": "string"}, "description": "Extra single words to treat as fillers."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "remove_retakes",
            "description": "Removes the abandoned earlier take of every false start / retake the detector finds (see list_retakes).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["ignore", "delete"], "description": "ignore (default) strikes them through; delete cuts them."},
                    "min_words": {"type": "integer", "description": "Shortest repeated run to treat as a retake. Default 3."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "shorten_gaps",
            "description": "Shortens every silence longer than longer_than seconds to target seconds. Call this for requests like \"tighten the pauses\" or \"remove dead air\".",
            "input_schema": {
                "type": "object",
                "properties": {
                    "longer_than": {"type": "number", "description": "Only silences longer than this. Default 1.0."},
                    "target": {"type": "number", "description": "New length of those silences. Default 0.4."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "edit_ranges",
            "description": "Ignores (strikes through), un-ignores, deletes, or silences one or more token ranges of the script in one step. This is the tool for cutting a passage, a sentence or a word. All ranges are given in the indices of the revision you read, even when several are deleted in the same call. The result echoes the affected text so you can confirm you cut what you meant to.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "rev": {"type": "string", "description": REV_DESC},
                    "action": {"type": "string", "enum": ["ignore", "unignore", "delete", "replace_with_gap"], "description": "ignore: strike through (skipped in playback and export, restorable by the user at any time) — the default choice for trims. unignore: restore struck-through text. delete: remove from the script. replace_with_gap: silence of the same length."},
                    "ranges": {"type": "array", "items": range_schema("to affect"), "description": "Token ranges, end exclusive."}
                },
                "required": ["rev", "action", "ranges"],
                "additionalProperties": false
            }
        },
        {
            "name": "apply_commands",
            "description": format!(
                "Applies a batch of editor commands atomically: either all succeed or none is applied and you get the exact error to fix. Use it only for edits that no dedicated tool covers, and only with at least one command. Every token index in the batch refers to the script at `rev` (the revision you read), regardless of earlier commands in the same batch; the tool translates them. Range fields are start (inclusive) and end (exclusive). Commands (JSON objects with a \"cmd\" field):\n{}",
                command_reference()
            ),
            "input_schema": {
                "type": "object",
                "properties": {
                    "rev": {"type": "string", "description": format!("{REV_DESC} Required when any command has token indices.")},
                    "commands": {"type": "array", "items": {"type": "object"}, "description": "Commands in the order to apply them."}
                },
                "required": ["commands"],
                "additionalProperties": false
            }
        },
        {
            "name": "set_captions",
            "description": "Turns burned-in captions on or off and sets their style. Only the fields you pass change. Call this for any request about captions or subtitles.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "description": "Default true."},
                    "color": {"type": "string", "description": format!("Text colour. {color}")},
                    "highlight": {"type": "string", "enum": ["none", "active_word", "reveal"], "description": "none: plain. active_word: the spoken word changes colour (karaoke style). reveal: words appear as spoken."},
                    "highlight_color": {"type": "string", "description": format!("Colour of the active word. {color}")},
                    "background": {"type": "string", "description": format!("Box behind the text, or \"none\". {color}")},
                    "outline_color": {"type": "string", "description": format!("Text outline, or \"none\". {color}")},
                    "outline_width": {"type": "number", "description": "Outline width in pixels."},
                    "font": {"type": "string", "description": "Font family name."},
                    "size": {"type": "number", "description": "Font size as a fraction of canvas height (default 0.055; 0.04 small, 0.08 large)."},
                    "position": {"type": "string", "enum": ["top", "middle", "bottom"]},
                    "max_lines": {"type": "integer", "description": "Lines per caption: 1 for one line, 2 for two."},
                    "max_chars_per_line": {"type": "integer", "description": "Default 32. Fewer for big text or vertical video."},
                    "uppercase": {"type": "boolean"},
                    "bold": {"type": "boolean"}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "add_text_layer",
            "description": "Adds a text layer (title, lower third, label, call to action) on top of the video for a span of time. Times are composition seconds; alternatively give token indices and the layer follows those words' times.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Exactly the text to show."},
                    "start": {"type": "number", "description": "Start time in seconds. Default 0."},
                    "duration": {"type": "number", "description": "Length in seconds. Default 3."},
                    "start_token": {"type": "integer", "description": "Instead of start: begin when this token begins (needs rev)."},
                    "end_token": {"type": "integer", "description": "Instead of duration: end when this token (exclusive) begins (needs rev)."},
                    "rev": {"type": "string", "description": REV_DESC},
                    "position": {"type": "string", "enum": ["top", "center", "lower_third", "bottom"], "description": "Default center."},
                    "size": {"type": "number", "description": "Font size as a fraction of canvas height. Default 0.08."},
                    "color": {"type": "string", "description": format!("Default white. {color}")},
                    "background": {"type": "string", "description": format!("Box behind the text, or \"none\" (default). {color}")},
                    "bold": {"type": "boolean", "description": "Default true."},
                    "italic": {"type": "boolean"},
                    "font": {"type": "string"},
                    "align": {"type": "string", "enum": ["left", "center", "right"]},
                    "animation": {"type": "string", "enum": ["none", "fade", "slide_up", "slide_down", "slide_left", "slide_right", "zoom_in", "zoom_out"], "description": "Entrance and exit animation. Default none."},
                    "name": {"type": "string", "description": "Layer name. Default: the text."}
                },
                "required": ["text"],
                "additionalProperties": false
            }
        },
        {
            "name": "update_layer",
            "description": "Changes an existing layer (id from the overview). Only the fields you pass change. To delete a layer use apply_commands with remove_layer.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Layer id."},
                    "name": {"type": "string"},
                    "start": {"type": "number", "description": "Seconds."},
                    "duration": {"type": "number", "description": "Seconds."},
                    "hidden": {"type": "boolean"},
                    "opacity": {"type": "number", "description": "0 to 1."},
                    "x": {"type": "number", "description": "Centre, fraction of canvas width (0 left, 1 right)."},
                    "y": {"type": "number", "description": "Centre, fraction of canvas height (0 top, 1 bottom)."},
                    "fade_in": {"type": "number"},
                    "fade_out": {"type": "number"},
                    "text": {"type": "string", "description": "Text layers only."},
                    "color": {"type": "string", "description": format!("Text layers only. {color}")},
                    "size": {"type": "number", "description": "Text layers only: fraction of canvas height."}
                },
                "required": ["id"],
                "additionalProperties": false
            }
        },
        {
            "name": "set_canvas",
            "description": "Sets the canvas size / aspect ratio of the active composition. Call this for requests like \"make it vertical\", \"for TikTok / Reels / Shorts\" (9:16), \"square\" (1:1), \"for YouTube\" (16:9).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "aspect": {"type": "string", "enum": ["16:9", "9:16", "1:1", "4:5", "4:3"], "description": "A standard size: 16:9 = 1920x1080, 9:16 = 1080x1920, 1:1 = 1080x1080, 4:5 = 1080x1350, 4:3 = 1440x1080."},
                    "width": {"type": "integer", "description": "Custom width in pixels (with height, instead of aspect)."},
                    "height": {"type": "integer"},
                    "fit": {"type": "string", "enum": ["fill", "fit"], "description": "fill (default): the video covers the canvas, cropping the overflow. fit: the whole video stays visible with bars."}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "set_transitions",
            "description": "Sets the transition used at every scene boundary. Transitions only exist between scenes; add scene breaks first (apply_commands set_scene_break) if the composition has one scene.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "transition": {"type": "string", "enum": ["cut", "cross_dissolve", "fade_through_black", "wipe_left", "wipe_right", "slide_left", "slide_up"]},
                    "secs": {"type": "number", "description": "Transition length. Default 0.5."}
                },
                "required": ["transition"],
                "additionalProperties": false
            }
        },
        {
            "name": "add_chapters",
            "description": "Adds chapter markers. A chapter is a topic, not a single question or remark: start one where the subject really changes, usually every few minutes, so a short recording gets two to four and an hour gets ten to twenty. Chapters must be at least 10 seconds apart. Each marker is placed at the start of the sentence containing the given token, so point at the first word of the sentence where the topic begins. Titles must describe what is actually said there.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "rev": {"type": "string", "description": REV_DESC},
                    "chapters": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "at": {"type": "integer", "description": "Token index of the first word of the chapter."},
                                "title": {"type": "string", "description": "Short chapter title."}
                            },
                            "required": ["at", "title"],
                            "additionalProperties": false
                        }
                    },
                    "replace_existing": {"type": "boolean", "description": "Remove the existing markers first. Default false."}
                },
                "required": ["rev", "chapters"],
                "additionalProperties": false
            }
        },
        {
            "name": "create_clips",
            "description": "Creates new compositions from token ranges of the active one (for highlights, shorts, teasers). The active composition stays selected and unchanged. Ranges are widened to whole sentences unless snap is false.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "rev": {"type": "string", "description": REV_DESC},
                    "clips": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "start": {"type": "integer", "description": "First token of the clip."},
                                "end": {"type": "integer", "description": "One past the last token (exclusive)."},
                                "name": {"type": "string", "description": "Name of the new composition."}
                            },
                            "required": ["start", "end", "name"],
                            "additionalProperties": false
                        }
                    },
                    "snap": {"type": "boolean", "description": "Widen to whole sentences. Default true."}
                },
                "required": ["rev", "clips"],
                "additionalProperties": false
            }
        }
    ]);
    if eager_input_streaming {
        for t in tools.as_array_mut().into_iter().flatten() {
            t["eager_input_streaming"] = json!(true);
        }
    }
    tools
}

/// One line describing a call for the UI, before it runs.
pub(crate) fn describe_call(name: &str, input: &Value) -> String {
    let n = |k: &str| input.get(k).and_then(Value::as_u64);
    let count = |k: &str| input.get(k).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    match name {
        "get_project_overview" => "Looking at the project".into(),
        "read_script" => match (n("start"), n("end")) {
            (Some(s), Some(e)) => format!("Reading the script ({s}–{e})"),
            (Some(s), None) => format!("Reading the script from {s}"),
            _ => "Reading the script".into(),
        },
        "search_script" => format!("Searching for “{}”", input.get("query").and_then(Value::as_str).unwrap_or("")),
        "list_fillers" => "Finding filler words".into(),
        "list_retakes" => "Finding retakes".into(),
        "list_gaps" => "Finding pauses".into(),
        "remove_fillers" => "Removing filler words".into(),
        "remove_retakes" => "Removing retakes".into(),
        "shorten_gaps" => "Shortening pauses".into(),
        "edit_ranges" => {
            let verb = match input.get("action").and_then(Value::as_str).unwrap_or("") {
                "ignore" => "Striking through",
                "unignore" => "Restoring",
                "delete" => "Deleting",
                "replace_with_gap" => "Silencing",
                _ => "Editing",
            };
            let c = count("ranges");
            format!("{verb} {c} passage{}", if c == 1 { "" } else { "s" })
        }
        "apply_commands" => {
            let c = count("commands");
            format!("Applying {c} edit{}", if c == 1 { "" } else { "s" })
        }
        "set_captions" => "Setting captions".into(),
        "add_text_layer" => format!("Adding text “{}”", input.get("text").and_then(Value::as_str).unwrap_or("")),
        "update_layer" => "Updating a layer".into(),
        "set_canvas" => "Changing the canvas".into(),
        "set_transitions" => "Setting transitions".into(),
        "add_chapters" => "Adding chapter markers".into(),
        "create_clips" => "Creating clips".into(),
        other => format!("Calling {other}"),
    }
}

// ---------------------------------------------------------------------------
// Stats and diffs
// ---------------------------------------------------------------------------

struct Stats {
    comp: Option<CompositionId>,
    words: usize,
    ignored: usize,
    duration: f64,
    rev: String,
    len: usize,
}

fn stats(ed: &Editor) -> Stats {
    match ed.comp() {
        Some(c) => Stats {
            comp: Some(c.id),
            words: c.word_count(0..c.script.len()),
            ignored: c.script.iter().filter(|t| t.is_word() && t.ignored).count(),
            duration: c.duration(),
            rev: context::script_rev(c),
            len: c.script.len(),
        },
        None => Stats { comp: None, words: 0, ignored: 0, duration: 0.0, rev: String::new(), len: 0 },
    }
}

fn precise(t: f64) -> String {
    format!("{} ({:.1}s)", timecode(t), t)
}

/// What changed between two states, including the all-important index notice.
fn diff_text(before: &Stats, after: &Stats) -> (String, String) {
    let mut text = String::new();
    let mut summary = String::new();
    if before.comp != after.comp {
        text.push_str("The active composition changed. ");
        if after.comp.is_some() {
            let _ = write!(text, "It has {} words playing, duration {}, script rev {} ({} tokens). Indices from the previous composition do not apply here.", after.words, precise(after.duration), after.rev, after.len);
        } else {
            text.push_str("No composition is open now.");
        }
        return (text, "Switched composition".into());
    }
    if after.words != before.words {
        let d = after.words as i64 - before.words as i64;
        let _ = write!(text, "Words playing: {} → {} ({:+}). ", before.words, after.words, d);
        let _ = write!(summary, "{} word{} {}", d.abs(), if d.abs() == 1 { "" } else { "s" }, if d < 0 { "removed" } else { "restored" });
    } else {
        let _ = write!(text, "Words playing: {} (unchanged). ", after.words);
    }
    if after.ignored != before.ignored {
        let _ = write!(text, "Struck-through words: {} → {}. ", before.ignored, after.ignored);
    }
    let dd = after.duration - before.duration;
    if dd.abs() >= 0.05 {
        let _ = write!(text, "Duration: {} → {} ({:+.1}s). ", precise(before.duration), precise(after.duration), dd);
        if !summary.is_empty() {
            summary.push_str(", ");
        }
        let _ = write!(summary, "{:+.1}s", dd);
    } else {
        let _ = write!(text, "Duration: {} (unchanged). ", precise(after.duration));
    }
    if before.rev == after.rev {
        let _ = write!(text, "\nToken indices did not shift: script rev is still {}.", after.rev);
    } else {
        let _ = write!(
            text,
            "\nTOKEN INDICES SHIFTED: the script is now rev {} with {} tokens (was rev {}). Every index you read before this call is stale. If, and only if, you still need another index-based edit, get fresh indices with read_script or search_script first.",
            after.rev, after.len, before.rev
        );
    }
    (text, summary)
}

/// Words of a range (ignored ones included), shortened in the middle.
fn excerpt(comp: &Composition, start: usize, end: usize) -> String {
    let r = script::clamp_range(&(start..end), comp.script.len());
    let words: Vec<&str> = comp.script[r].iter().filter(|t| t.is_word()).map(|t| t.text.as_str()).collect();
    if words.is_empty() {
        return "(silence)".into();
    }
    if words.len() <= 16 {
        format!("“{}”", words.join(" "))
    } else {
        format!("“{} … {}” ({} words)", words[..8].join(" "), words[words.len() - 6..].join(" "), words.len())
    }
}

// ---------------------------------------------------------------------------
// Batch engine
// ---------------------------------------------------------------------------

type Key = (u8, [u8; 16], u64, u64);

fn keys(comp: &Composition) -> Vec<Key> {
    comp.script.iter().map(|t| (t.kind as u8, *t.asset.0.as_bytes(), t.src_start.to_bits(), t.src_end.to_bits())).collect()
}

/// Where each token of `old` sits in `new`, or `None` if it is gone or changed.
fn align(old: &[Key], new: &[Key]) -> Vec<Option<usize>> {
    let mut map = vec![None; old.len()];
    let mut p = 0;
    while p < old.len() && p < new.len() && old[p] == new[p] {
        map[p] = Some(p);
        p += 1;
    }
    let mut s = 0;
    while s < old.len() - p && s < new.len() - p && old[old.len() - 1 - s] == new[new.len() - 1 - s] {
        map[old.len() - 1 - s] = Some(new.len() - 1 - s);
        s += 1;
    }
    // In the changed middle, match tokens whose key is unique on both sides (handles moves and multi-site edits).
    let (old_mid, new_mid) = (&old[p..old.len() - s], &new[p..new.len() - s]);
    if !old_mid.is_empty() && !new_mid.is_empty() {
        let mut seen_new: HashMap<&Key, (usize, usize)> = HashMap::new();
        for (j, k) in new_mid.iter().enumerate() {
            let e = seen_new.entry(k).or_insert((0, j));
            e.0 += 1;
        }
        let mut seen_old: HashMap<&Key, usize> = HashMap::new();
        for k in old_mid {
            *seen_old.entry(k).or_insert(0) += 1;
        }
        for (i, k) in old_mid.iter().enumerate() {
            if seen_old[k] == 1
                && let Some((1, j)) = seen_new.get(k) {
                    map[p + i] = Some(p + j);
                }
        }
    }
    map
}

/// Translates indices of the revision the model read into current indices.
struct Remap {
    /// Original index -> current index.
    map: Vec<Option<usize>>,
    cur_len: usize,
}

impl Remap {
    fn identity(len: usize) -> Self {
        Self { map: (0..len).map(Some).collect(), cur_len: len }
    }
    fn len(&self) -> usize {
        self.map.len()
    }
    fn tok(&self, i: usize) -> Result<usize, String> {
        if i >= self.len() {
            return Err(format!("token {i} is outside the script (valid tokens are 0..{})", self.len()));
        }
        self.map[i].ok_or_else(|| format!("token {i} was removed or changed by an earlier command in this call; apply the earlier commands first, re-read, then address it"))
    }
    /// An insertion point: before original token `i` (or at the end).
    fn pos(&self, i: usize) -> Result<usize, String> {
        if i > self.len() {
            return Err(format!("position {i} is outside the script (0..={})", self.len()));
        }
        Ok(self.map[i..].iter().flatten().next().copied().unwrap_or(self.cur_len))
    }
    fn range(&self, start: usize, end: usize) -> Result<(usize, usize), String> {
        if start >= end {
            return Err(format!("range {start}..{end} is empty: end is exclusive and must be greater than start"));
        }
        if end > self.len() {
            return Err(format!("range {start}..{end} is outside the script (valid tokens are 0..{})", self.len()));
        }
        let (a, b) = (self.tok(start)?, self.tok(end - 1)? + 1);
        if a >= b {
            return Err(format!("range {start}..{end} was reordered by an earlier command in this call; re-read and address it separately"));
        }
        Ok((a, b))
    }
    fn advance(&mut self, step: &[Option<usize>], new_len: usize) {
        for m in &mut self.map {
            *m = m.and_then(|i| step.get(i).copied().flatten());
        }
        self.cur_len = new_len;
    }
}

fn uses_indices(cmd: &Command) -> bool {
    use Command::*;
    matches!(
        cmd,
        DeleteRange { .. }
            | IgnoreRange { .. }
            | ReplaceWithGap { .. }
            | InsertGap { .. }
            | MoveRange { .. }
            | CorrectRange { .. }
            | HideText { .. }
            | SetSpeaker { .. }
            | SetParagraphBreak { .. }
            | SetSceneBreak { .. }
            | SetMarker { .. }
            | SetNote { .. }
            | SetHighlight { .. }
            | SetStyle { .. }
            | SetGapDuration { .. }
            | RestoreAt { .. }
            | RestoreRange { .. }
            | InsertAsset { at: Some(_), .. }
            | TrimBoundary { .. }
            | CompositionFromRange { .. }
    )
}

/// Rewrites a command's indices from the read revision to the current script.
fn rebase(cmd: Command, r: &Remap) -> Result<Command, String> {
    use Command::*;
    Ok(match cmd {
        DeleteRange { start, end } => {
            let (start, end) = r.range(start, end)?;
            DeleteRange { start, end }
        }
        IgnoreRange { start, end, ignored } => {
            let (start, end) = r.range(start, end)?;
            IgnoreRange { start, end, ignored }
        }
        ReplaceWithGap { start, end } => {
            let (start, end) = r.range(start, end)?;
            ReplaceWithGap { start, end }
        }
        InsertGap { at, secs } => InsertGap { at: r.pos(at)?, secs },
        MoveRange { start, end, dest } => {
            let (start, end) = r.range(start, end)?;
            MoveRange { start, end, dest: r.pos(dest)? }
        }
        CorrectRange { start, end, text } => {
            let (start, end) = r.range(start, end)?;
            CorrectRange { start, end, text }
        }
        HideText { start, end, hidden } => {
            let (start, end) = r.range(start, end)?;
            HideText { start, end, hidden }
        }
        SetSpeaker { start, end, speaker } => {
            let (start, end) = r.range(start, end)?;
            SetSpeaker { start, end, speaker }
        }
        SetParagraphBreak { at, on } => SetParagraphBreak { at: r.tok(at)?, on },
        SetSceneBreak { at, on } => SetSceneBreak { at: r.tok(at)?, on },
        SetMarker { at, name } => SetMarker { at: r.tok(at)?, name },
        SetNote { at, note } => SetNote { at: r.tok(at)?, note },
        SetHighlight { start, end, color } => {
            let (start, end) = r.range(start, end)?;
            SetHighlight { start, end, color }
        }
        SetStyle { start, end, bold, italic } => {
            let (start, end) = r.range(start, end)?;
            SetStyle { start, end, bold, italic }
        }
        SetGapDuration { at, secs } => SetGapDuration { at: r.tok(at)?, secs },
        RestoreAt { at } => RestoreAt { at: r.tok(at)? },
        RestoreRange { start, end } => {
            let (start, end) = r.range(start, end)?;
            RestoreRange { start, end }
        }
        InsertAsset { asset, at: Some(at) } => InsertAsset { asset, at: Some(r.pos(at)?) },
        TrimBoundary { at, side, delta } => TrimBoundary { at: r.tok(at)?, side, delta },
        CompositionFromRange { start, end, name } => {
            let (start, end) = r.range(start, end)?;
            CompositionFromRange { start, end, name }
        }
        other => other,
    })
}

/// Checks the editor does not make itself (it silently ignores some bad input).
fn validate(ed: &Editor, cmd: &Command) -> Result<(), String> {
    use Command::*;
    let finite = |v: f64, what: &str| if v.is_finite() { Ok(()) } else { Err(format!("{what} is not a number")) };
    match cmd {
        AddAsset { .. } | RemoveAsset { .. } | SetTranscript { .. } | PasteTokens { .. } => {
            return Err("this command is not available to the assistant: importing, removing and transcribing files is done by the user in the app".into());
        }
        _ => {}
    }
    let comp = ed.comp();
    match cmd {
        SetGapDuration { at, secs } => {
            finite(*secs, "secs")?;
            if comp.and_then(|c| c.script.get(*at)).is_some_and(|t| t.kind == TokenKind::Word) {
                return Err(format!("token {at} is a word, not a pause; set_gap_duration only applies to pause tokens"));
            }
        }
        CorrectRange { start, end, .. } => {
            if comp.is_some_and(|c| !c.script[script::clamp_range(&(*start..*end), c.script.len())].iter().any(|t| t.is_word())) {
                return Err("the range contains no words to correct".into());
            }
        }
        CorrectAll { from, .. } => {
            let key = script::normalize_word(from);
            let found = ed.project.compositions.iter().any(|c| c.script.iter().any(|t| t.is_word() && script::normalize_word(&t.text) == key));
            if key.is_empty() || !found {
                return Err(format!("the word {from:?} does not occur in the script"));
            }
        }
        SetParagraphBreak { at: 0, .. } | SetSceneBreak { at: 0, .. } => return Err("token 0 always starts the first paragraph and scene".into()),
        MoveRange { start, end, dest } => {
            if dest >= start && dest <= end {
                return Err("dest is inside the range being moved, so nothing would move".into());
            }
        }
        SplitAt { time } => {
            finite(*time, "time")?;
            let d = comp.map(|c| c.script_duration()).unwrap_or(0.0);
            if *time <= 0.0 || *time >= d {
                return Err(format!("time {time} is outside the script (0..{d:.2}s)"));
            }
        }
        InsertGap { secs, .. } => finite(*secs, "secs")?,
        ShortenGaps { longer_than, target, .. } => {
            finite(*longer_than, "longer_than")?;
            finite(*target, "target")?;
            if target >= longer_than {
                return Err("target must be shorter than longer_than".into());
            }
        }
        TrimBoundary { at, delta, .. } => {
            finite(*delta, "delta")?;
            if comp.is_some_and(|c| !c.edit_boundaries().contains(at)) {
                return Err(format!("there is no cut before token {at} (cuts are shown as ✂)"));
            }
        }
        RestoreAt { at } => {
            if comp.is_some_and(|c| !c.edit_boundaries().contains(at)) {
                return Err(format!("there is no cut before token {at} (cuts are shown as ✂), so there is nothing to restore"));
            }
        }
        AddLayer { layer } | UpdateLayer { layer } => {
            let t = &layer.transform;
            for (v, what) in [(layer.start, "start"), (layer.duration, "duration"), (t.x, "x"), (t.y, "y"), (t.w, "w"), (t.h, "h"), (t.opacity, "opacity")] {
                finite(v, what)?;
            }
            if layer.start < 0.0 || layer.duration <= 0.0 || layer.duration > 86_400.0 {
                return Err("layer start must be 0 or more and duration must be positive".into());
            }
        }
        SplitLayer { time, .. } => finite(*time, "time")?,
        AddComment { time, .. } => {
            finite(*time, "time")?;
            if *time < 0.0 {
                return Err("time must be 0 or more".into());
            }
        }
        SetAllTransitions { secs, .. } => finite(*secs, "secs")?,
        SetFps { fps } => finite(*fps, "fps")?,
        DeleteComposition => {
            if ed.project.compositions.len() <= 1 {
                return Err("this is the only composition; it cannot be deleted".into());
            }
        }
        NewComposition { name } | RenameComposition { name } | CompositionFromRange { name, .. }
            if name.trim().is_empty() => {
                return Err("name is empty".into());
            }
        _ => {}
    }
    Ok(())
}

fn cmd_name(cmd: &Command) -> String {
    serde_json::to_value(cmd).ok().and_then(|v| v["cmd"].as_str().map(str::to_string)).unwrap_or_else(|| "command".into())
}

/// One line saying what a (rebased) command is about to do, quoting the text it touches.
fn echo(ed: &Editor, cmd: &Command) -> String {
    use Command::*;
    let name = cmd_name(cmd);
    let Some(comp) = ed.comp() else { return name };
    match cmd {
        DeleteRange { start, end }
        | IgnoreRange { start, end, .. }
        | ReplaceWithGap { start, end }
        | HideText { start, end, .. }
        | RestoreRange { start, end }
        | SetSpeaker { start, end, .. }
        | SetHighlight { start, end, .. }
        | SetStyle { start, end, .. }
        | MoveRange { start, end, .. }
        | CompositionFromRange { start, end, .. } => {
            let verb = match cmd {
                IgnoreRange { ignored: false, .. } => "unignore".to_string(),
                IgnoreRange { .. } => "ignore".to_string(),
                _ => name,
            };
            format!("{verb} {} ({:.1}s)", excerpt(comp, *start, *end), comp.range_duration(*start..*end))
        }
        CorrectRange { start, end, text } => format!("correct {} → “{}”", excerpt(comp, *start, *end), text),
        SetMarker { at, name: Some(n) } => format!("marker {:?} at {} before {}", n, timecode(comp.time_of_token(*at)), excerpt(comp, *at, at + 6)),
        _ => name,
    }
}

struct BatchReport {
    lines: Vec<String>,
    applied: usize,
}

/// Applies commands whose indices are all relative to the script at `rev`.
/// Atomic: on any error the editor is exactly as before.
fn run_batch(ed: &mut Editor, rev: Option<&str>, cmds: Vec<Command>) -> Result<BatchReport, String> {
    if cmds.is_empty() {
        return Err("no commands were given".into());
    }
    let indexed = cmds.iter().any(uses_indices);
    let start_comp = ed.active;
    let mut remap = Remap::identity(0);
    if indexed {
        let comp = ed.comp().ok_or("no composition is open")?;
        let current = context::script_rev(comp);
        match rev {
            None => return Err(format!("`rev` is required for commands with token indices. The script is at rev {current}; pass the rev shown where you read the indices.")),
            Some(r) if r.trim() != current => {
                return Err(format!(
                    "stale indices: you passed rev {}, but the script is now rev {current} ({} tokens). Token indices have shifted since you read them. Call read_script or search_script to get current indices, then retry with rev {current}.",
                    r.trim(),
                    comp.script.len()
                ));
            }
            Some(_) => {}
        }
        remap = Remap::identity(comp.script.len());
    }
    let snapshot = (ed.project.clone(), ed.active);
    let mut lines = Vec::new();
    let total = cmds.len();
    for (n, cmd) in cmds.into_iter().enumerate() {
        let name = cmd_name(&cmd);
        let step = (|| -> Result<String, String> {
            let cmd = if uses_indices(&cmd) {
                if ed.active != start_comp {
                    return Err("an earlier command in this call switched to another composition, so these indices no longer apply; send index-based commands for the new composition in a separate call after reading its script".into());
                }
                rebase(cmd, &remap)?
            } else {
                cmd
            };
            validate(ed, &cmd)?;
            let line = echo(ed, &cmd);
            let before = if ed.active == start_comp { ed.comp().map(keys) } else { None };
            ed.apply(cmd).map_err(|e| e.to_string())?;
            if let (Some(before), true) = (before, ed.active == start_comp)
                && let Some(comp) = ed.comp() {
                    let after = keys(comp);
                    if indexed && before != after {
                        remap.advance(&align(&before, &after), after.len());
                    }
                }
            Ok(line)
        })();
        match step {
            Ok(line) => lines.push(line),
            Err(e) => {
                ed.project = snapshot.0;
                ed.active = snapshot.1;
                return Err(format!(
                    "command {} of {total} ({name}) failed: {e}\nNothing was applied: the whole call was rolled back. Fix the problem and send the complete call again.",
                    n + 1
                ));
            }
        }
    }
    Ok(BatchReport { applied: lines.len(), lines })
}

/// Runs a batch and formats the result for the model.
fn batch_outcome(ed: &mut Editor, rev: Option<&str>, cmds: Vec<Command>, fallback_summary: &str) -> ToolOutcome {
    let before = stats(ed);
    match run_batch(ed, rev, cmds) {
        Err(e) => ToolOutcome::error(e),
        Ok(report) => {
            let after = stats(ed);
            let (diff, summary) = diff_text(&before, &after);
            let mut text = format!("OK: applied {} command{}.\n", report.applied, if report.applied == 1 { "" } else { "s" });
            for (i, l) in report.lines.iter().enumerate().take(60) {
                let _ = writeln!(text, "{}. {}", i + 1, l);
            }
            if report.lines.len() > 60 {
                let _ = writeln!(text, "… and {} more", report.lines.len() - 60);
            }
            text.push_str(&diff);
            ToolOutcome { text, ok: true, commands: report.applied, summary: if summary.is_empty() { fallback_summary.to_string() } else { summary } }
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

fn parse<T: for<'de> Deserialize<'de>>(input: &Value) -> Result<T, String> {
    serde_json::from_value(input.clone()).map_err(|e| format!("invalid tool input: {e}"))
}

pub(crate) fn parse_color(s: &str) -> Result<Rgba, String> {
    let t = s.trim().to_lowercase();
    if let Some(c) = Rgba::parse_hex(&t) {
        return Ok(c);
    }
    Ok(match t.as_str() {
        "white" => Rgba::rgb(255, 255, 255),
        "black" => Rgba::rgb(0, 0, 0),
        "yellow" => Rgba::rgb(0xFF, 0xE6, 0x00),
        "red" => Rgba::rgb(0xE5, 0x39, 0x35),
        "green" => Rgba::rgb(0x43, 0xA0, 0x47),
        "blue" => Rgba::rgb(0x1E, 0x88, 0xE5),
        "orange" => Rgba::rgb(0xFF, 0x8A, 0x00),
        "pink" => Rgba::rgb(0xF0, 0x62, 0x92),
        "purple" => Rgba::rgb(0x8E, 0x5B, 0xD9),
        "cyan" => Rgba::rgb(0x00, 0xBC, 0xD4),
        "magenta" => Rgba::rgb(0xE0, 0x40, 0xFB),
        "gray" | "grey" => Rgba::rgb(0x9E, 0x9E, 0x9E),
        _ => return Err(format!("{s:?} is not a colour; use \"#RRGGBB\"")),
    })
}

fn optional_color(s: &str) -> Result<Option<Rgba>, String> {
    if matches!(s.trim().to_lowercase().as_str(), "none" | "" | "transparent" | "off") { Ok(None) } else { parse_color(s).map(Some) }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeIn {
    start: usize,
    end: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadIn {
    start: Option<usize>,
    end: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchIn {
    query: String,
    include_ignored: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListFillersIn {
    loose: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRetakesIn {
    min_words: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListGapsIn {
    min_secs: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveFillersIn {
    mode: Option<String>,
    phrases: Option<bool>,
    repeats: Option<bool>,
    loose: Option<bool>,
    #[serde(default)]
    custom: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveRetakesIn {
    mode: Option<String>,
    min_words: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShortenGapsIn {
    longer_than: Option<f64>,
    target: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRangesIn {
    rev: String,
    action: String,
    ranges: Vec<RangeIn>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyIn {
    rev: Option<String>,
    commands: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptionsIn {
    enabled: Option<bool>,
    color: Option<String>,
    highlight: Option<String>,
    highlight_color: Option<String>,
    background: Option<String>,
    outline_color: Option<String>,
    outline_width: Option<f64>,
    font: Option<String>,
    size: Option<f64>,
    position: Option<String>,
    max_lines: Option<usize>,
    max_chars_per_line: Option<usize>,
    uppercase: Option<bool>,
    bold: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextLayerIn {
    text: String,
    start: Option<f64>,
    duration: Option<f64>,
    start_token: Option<usize>,
    end_token: Option<usize>,
    rev: Option<String>,
    position: Option<String>,
    size: Option<f64>,
    color: Option<String>,
    background: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    font: Option<String>,
    align: Option<String>,
    animation: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateLayerIn {
    id: String,
    name: Option<String>,
    start: Option<f64>,
    duration: Option<f64>,
    hidden: Option<bool>,
    opacity: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
    fade_in: Option<f64>,
    fade_out: Option<f64>,
    text: Option<String>,
    color: Option<String>,
    size: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanvasIn {
    aspect: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionsIn {
    transition: String,
    secs: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChapterIn {
    at: usize,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaptersIn {
    rev: String,
    chapters: Vec<ChapterIn>,
    replace_existing: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipIn {
    start: usize,
    end: usize,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipsIn {
    rev: String,
    clips: Vec<ClipIn>,
    snap: Option<bool>,
}

fn remove_mode(s: Option<&str>, default: RemoveMode) -> Result<RemoveMode, String> {
    match s.map(|m| m.trim().to_lowercase()) {
        None => Ok(default),
        Some(m) if m == "delete" => Ok(RemoveMode::Delete),
        Some(m) if m == "ignore" => Ok(RemoveMode::Ignore),
        Some(m) => Err(format!("mode {m:?} is not valid; use \"delete\" or \"ignore\"")),
    }
}

fn check_rev(comp: &Composition, rev: &str) -> Result<(), String> {
    let current = context::script_rev(comp);
    if rev.trim() == current {
        Ok(())
    } else {
        Err(format!(
            "stale indices: you passed rev {}, but the script is now rev {current} ({} tokens). Token indices have shifted since you read them. Call read_script or search_script to get current indices, then retry with rev {current}.",
            rev.trim(),
            comp.script.len()
        ))
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Runs one tool call against the editor. Never panics on model input.
pub(crate) fn run(ed: &mut Editor, name: &str, input: &Value) -> ToolOutcome {
    let result = match name {
        "get_project_overview" => Ok(ToolOutcome::info(context::describe_project(ed), "Read the project")),
        "read_script" => read_script(ed, input),
        "search_script" => search_script(ed, input),
        "list_fillers" => list_fillers(ed, input),
        "list_retakes" => list_retakes(ed, input),
        "list_gaps" => list_gaps(ed, input),
        "remove_fillers" => remove_fillers(ed, input),
        "remove_retakes" => remove_retakes(ed, input),
        "shorten_gaps" => shorten_gaps(ed, input),
        "edit_ranges" => edit_ranges(ed, input),
        "apply_commands" => apply_commands(ed, input),
        "set_captions" => set_captions(ed, input),
        "add_text_layer" => add_text_layer(ed, input),
        "update_layer" => update_layer(ed, input),
        "set_canvas" => set_canvas(ed, input),
        "set_transitions" => set_transitions(ed, input),
        "add_chapters" => add_chapters(ed, input),
        "create_clips" => create_clips(ed, input),
        other => Err(format!("there is no tool named {other:?}. Use one of the tools you were given.")),
    };
    result.unwrap_or_else(ToolOutcome::error)
}

fn comp_of(ed: &Editor) -> Result<&Composition, String> {
    ed.comp().ok_or_else(|| "no composition is open".to_string())
}

fn read_script(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ReadIn = parse(input)?;
    let comp = comp_of(ed)?;
    let len = comp.script.len();
    let start = a.start.unwrap_or(0);
    if start > len {
        return Err(format!("start {start} is outside the script (0..{len})"));
    }
    let want_end = a.end.unwrap_or(len).min(len);
    if want_end < start {
        return Err(format!("end {want_end} is before start {start}"));
    }
    let end = want_end.min(context::page_end(comp, start, context::PAGE_WORD_LIMIT));
    let mut text = context::describe_script(comp, &ed.project, start..end);
    if end < want_end {
        let _ = writeln!(text, "(page ends at token {end}; continue with read_script start={end})");
    }
    Ok(ToolOutcome::info(text, format!("Read tokens {start}–{end}")))
}

fn search_script(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: SearchIn = parse(input)?;
    let comp = comp_of(ed)?;
    let query = a.query.trim();
    if query.is_empty() {
        return Err("query is empty".into());
    }
    let include_ignored = a.include_ignored.unwrap_or(true);
    let mut hits = script::find(comp, query, &FindOptions { whole_words: true, include_ignored, ..Default::default() });
    let mut partial = false;
    if hits.is_empty() {
        hits = script::find(comp, query, &FindOptions { whole_words: false, include_ignored, ..Default::default() });
        partial = !hits.is_empty();
    }
    let total_words = comp.script.iter().filter(|t| t.is_word()).count();
    if hits.is_empty() {
        let text = format!(
            "No matches for {query:?} in the script ({total_words} words searched{}). It is not said in this composition as transcribed.",
            if include_ignored { ", including struck-through text" } else { "" }
        );
        return Ok(ToolOutcome::info(text, "No matches"));
    }
    let starts = comp.token_starts();
    let word_idx: Vec<usize> = comp.script.iter().enumerate().filter(|(_, t)| t.is_word()).map(|(i, _)| i).collect();
    let mut text = format!(
        "script rev {} · {} {}match{} for {query:?} (ranges are start..end, end exclusive):\n",
        context::script_rev(comp),
        hits.len(),
        if partial { "partial " } else { "" },
        if hits.len() == 1 { "" } else { "es" }
    );
    for h in hits.iter().take(40) {
        let a_pos = word_idx.partition_point(|&i| i < h.start);
        let b_pos = word_idx.partition_point(|&i| i < h.end);
        let before: Vec<&str> = word_idx[a_pos.saturating_sub(6)..a_pos].iter().map(|&i| comp.script[i].text.as_str()).collect();
        let hit: Vec<&str> = word_idx[a_pos..b_pos].iter().map(|&i| comp.script[i].text.as_str()).collect();
        let after: Vec<&str> = word_idx[b_pos..(b_pos + 6).min(word_idx.len())].iter().map(|&i| comp.script[i].text.as_str()).collect();
        let ignored = comp.script[h.start].ignored;
        let _ = writeln!(
            text,
            "- {}..{} at {}{}: {} »{}« {}",
            h.start,
            h.end,
            timecode(starts[h.start]),
            if ignored { " (struck through)" } else { "" },
            before.join(" "),
            hit.join(" "),
            after.join(" ")
        );
    }
    if hits.len() > 40 {
        let _ = writeln!(text, "… and {} more", hits.len() - 40);
    }
    Ok(ToolOutcome::info(text, format!("{} match{}", hits.len(), if hits.len() == 1 { "" } else { "es" })))
}

fn list_fillers(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ListFillersIn = parse(input)?;
    let comp = comp_of(ed)?;
    let opts = FillerOptions { loose: a.loose.unwrap_or(false), ..Default::default() };
    let hits = detect::find_fillers(comp, &opts);
    if hits.is_empty() {
        return Ok(ToolOutcome::info("The detector finds no filler words in the playing script.".into(), "No fillers"));
    }
    let count = |k: FillerKind| hits.iter().filter(|h| h.kind == k).count();
    let mut text = format!(
        "script rev {} · {} fillers: {} um/uh, {} phrases, {} immediate repeats, {} loose\n",
        context::script_rev(comp),
        hits.len(),
        count(FillerKind::Filler),
        count(FillerKind::Phrase),
        count(FillerKind::Repeat),
        count(FillerKind::Loose)
    );
    for h in hits.iter().take(200) {
        let _ = writeln!(text, "- {}..{} at {} {:?}: {:?}", h.tokens.start, h.tokens.end, timecode(h.time), h.kind, h.text);
    }
    if hits.len() > 200 {
        let _ = writeln!(text, "… and {} more", hits.len() - 200);
    }
    Ok(ToolOutcome::info(text, format!("{} fillers", hits.len())))
}

fn list_retakes(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ListRetakesIn = parse(input)?;
    let comp = comp_of(ed)?;
    let hits = detect::find_retakes(comp, a.min_words.unwrap_or(3).clamp(2, 20), 60);
    if hits.is_empty() {
        return Ok(ToolOutcome::info("The detector finds no retakes or false starts.".into(), "No retakes"));
    }
    let mut text = format!("script rev {} · {} retakes (the earlier take is the abandoned one):\n", context::script_rev(comp), hits.len());
    for h in hits.iter().take(100) {
        let _ = writeln!(
            text,
            "- earlier take {}..{} at {}: {:?} → said again at {}..{}: {}",
            h.earlier.start,
            h.earlier.end,
            timecode(h.time),
            h.text,
            h.later.start,
            h.later.end,
            excerpt(comp, h.later.start, h.later.end)
        );
    }
    Ok(ToolOutcome::info(text, format!("{} retakes", hits.len())))
}

fn list_gaps(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ListGapsIn = parse(input)?;
    let comp = comp_of(ed)?;
    let min = a.min_secs.unwrap_or(1.0);
    if !min.is_finite() || min < 0.0 {
        return Err("min_secs must be 0 or more".into());
    }
    let gaps = detect::find_gaps(comp, min.max(0.05));
    if gaps.is_empty() {
        return Ok(ToolOutcome::info(format!("There are no pauses of {min:.1}s or longer."), "No pauses"));
    }
    let starts = comp.token_starts();
    let total: f64 = gaps.iter().map(|g| g.1).sum();
    let mut text = format!("script rev {} · {} pauses of {min:.1}s or longer, {total:.1}s in total:\n", context::script_rev(comp), gaps.len());
    let word_near = |mut i: isize, step: isize| -> &str {
        while i >= 0 && (i as usize) < comp.script.len() {
            if comp.script[i as usize].shows_text() {
                return comp.script[i as usize].text.as_str();
            }
            i += step;
        }
        "(edge)"
    };
    for (r, d) in gaps.iter().take(200) {
        let _ = writeln!(
            text,
            "- {}..{} at {}: {:.1}s, between “{}” and “{}”",
            r.start,
            r.end,
            timecode(starts[r.start]),
            d,
            word_near(r.start as isize - 1, -1),
            word_near(r.end as isize, 1)
        );
    }
    if gaps.len() > 200 {
        let _ = writeln!(text, "… and {} more", gaps.len() - 200);
    }
    Ok(ToolOutcome::info(text, format!("{} pauses", gaps.len())))
}

fn remove_fillers(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: RemoveFillersIn = parse(input)?;
    let mode = remove_mode(a.mode.as_deref(), RemoveMode::Delete)?;
    let options = FillerOptions { phrases: a.phrases.unwrap_or(true), repeats: a.repeats.unwrap_or(true), loose: a.loose.unwrap_or(false), custom: a.custom };
    let found = detect::find_fillers(comp_of(ed)?, &options);
    if found.is_empty() {
        return Ok(ToolOutcome::info("The detector finds no filler words in the playing script, so nothing was changed.".into(), "No fillers found"));
    }
    let before = stats(ed);
    let snapshot = (ed.project.clone(), ed.active);
    // Removing a filler can expose a new one ("the um the" → "the the"), so repeat until clean.
    let mut removed = 0usize;
    let mut passes = 0usize;
    let mut sample: Vec<String> = Vec::new();
    loop {
        let hits = detect::find_fillers(comp_of(ed)?, &options);
        if hits.is_empty() || passes >= 8 {
            break;
        }
        removed += hits.len();
        for h in &hits {
            if sample.len() < 12 && !sample.contains(&h.text.to_lowercase()) {
                sample.push(h.text.to_lowercase());
            }
        }
        if let Err(e) = ed.apply(Command::RemoveFillers { options: options.clone(), mode }) {
            ed.project = snapshot.0;
            ed.active = snapshot.1;
            return Err(format!("{e}. Nothing was changed."));
        }
        passes += 1;
    }
    let after = stats(ed);
    let (diff, _) = diff_text(&before, &after);
    let left = detect::find_fillers(comp_of(ed)?, &options).len();
    let text = format!(
        "OK: {} {removed} filler{} ({}).{}\n{diff}",
        if mode == RemoveMode::Delete { "deleted" } else { "struck through" },
        if removed == 1 { "" } else { "s" },
        sample.join(", "),
        if left > 0 { format!(" {left} remain.") } else { " None remain.".to_string() },
    );
    Ok(ToolOutcome { text, ok: true, commands: passes, summary: format!("Removed {removed} filler{} ({:+.1}s)", if removed == 1 { "" } else { "s" }, after.duration - before.duration) })
}

fn remove_retakes(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: RemoveRetakesIn = parse(input)?;
    let mode = remove_mode(a.mode.as_deref(), RemoveMode::Ignore)?;
    let min_words = a.min_words.unwrap_or(3).clamp(2, 20);
    let comp = comp_of(ed)?;
    let hits = detect::find_retakes(comp, min_words, 60);
    if hits.is_empty() {
        return Ok(ToolOutcome::info("The detector finds no retakes or false starts, so nothing was changed.".into(), "No retakes found"));
    }
    let listing: Vec<String> = hits.iter().take(20).map(|h| format!("- {:?}", h.text)).collect();
    let n = hits.len();
    let mut out = batch_outcome(ed, None, vec![Command::RemoveRetakes { min_words, mode }], "Removed retakes");
    if out.ok {
        out.text = format!("Removed the earlier take of {n} retake{}:\n{}\n{}", if n == 1 { "" } else { "s" }, listing.join("\n"), out.text);
        out.summary = format!("Removed {n} retake{}", if n == 1 { "" } else { "s" });
    }
    Ok(out)
}

fn shorten_gaps(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ShortenGapsIn = parse(input)?;
    let (longer_than, target) = (a.longer_than.unwrap_or(1.0), a.target.unwrap_or(0.4));
    if !(longer_than.is_finite() && target.is_finite()) || target < 0.0 || longer_than <= 0.0 {
        return Err("longer_than must be positive and target 0 or more".into());
    }
    let n = detect::find_gaps(comp_of(ed)?, longer_than + 1e-6).len();
    if n == 0 {
        return Ok(ToolOutcome::info(format!("There are no pauses longer than {longer_than:.1}s, so nothing was changed."), "No long pauses"));
    }
    let mut out = batch_outcome(ed, None, vec![Command::ShortenGaps { longer_than, shorter_than: None, target }], "Shortened pauses");
    if out.ok {
        out.summary = format!("Shortened pauses ({})", out.summary);
    }
    Ok(out)
}

fn edit_ranges(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: EditRangesIn = parse(input)?;
    if a.ranges.is_empty() {
        return Err("no ranges were given".into());
    }
    // Merge overlapping and touching ranges so each token is addressed once.
    let mut ranges: Vec<(usize, usize)> = a.ranges.iter().map(|r| (r.start, r.end)).collect();
    if let Some((s, e)) = ranges.iter().find(|(s, e)| s >= e) {
        return Err(format!("range {s}..{e} is empty: end is exclusive and must be greater than start"));
    }
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let action = a.action.trim().to_lowercase();
    if action == "unignore" {
        let comp = comp_of(ed)?;
        let any = merged.iter().any(|(s, e)| comp.script[script::clamp_range(&(*s..*e), comp.script.len())].iter().any(|t| t.ignored));
        if !any {
            return Err("nothing in these ranges is struck through, so there is nothing to un-ignore. Text that was deleted (not ignored) is brought back with apply_commands restore_range / restore_at.".into());
        }
    }
    let cmds: Vec<Command> = merged
        .into_iter()
        .map(|(start, end)| match action.as_str() {
            "ignore" => Ok(Command::IgnoreRange { start, end, ignored: true }),
            "unignore" => Ok(Command::IgnoreRange { start, end, ignored: false }),
            "delete" => Ok(Command::DeleteRange { start, end }),
            "replace_with_gap" => Ok(Command::ReplaceWithGap { start, end }),
            other => Err(format!("action {other:?} is not valid; use ignore, unignore, delete or replace_with_gap")),
        })
        .collect::<Result<_, _>>()?;
    Ok(batch_outcome(ed, Some(&a.rev), cmds, "Edited the script"))
}

/// Fills in what the model may reasonably leave out of raw command JSON.
fn normalise_command(mut v: Value) -> Value {
    if v["cmd"] == "add_layer" && v["layer"].is_object() {
        let layer = &mut v["layer"];
        if layer.get("id").is_none() {
            layer["id"] = serde_json::to_value(LayerId::new()).unwrap_or(Value::Null);
        }
        if layer.get("track").is_none() {
            layer["track"] = json!(1);
        }
        if layer.get("transform").is_none() {
            layer["transform"] = serde_json::to_value(Transform::default()).unwrap_or(Value::Null);
        }
        if layer.get("name").is_none() {
            layer["name"] = json!("Layer");
        }
    }
    v
}

fn apply_commands(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ApplyIn = parse(input)?;
    let mut cmds = Vec::with_capacity(a.commands.len());
    for (i, raw) in a.commands.into_iter().enumerate() {
        let name = raw.get("cmd").and_then(Value::as_str).unwrap_or("?").to_string();
        match serde_json::from_value::<Command>(normalise_command(raw)) {
            Ok(c) => cmds.push(c),
            Err(e) => {
                let e = e.to_string();
                // serde lists every variant for an unknown command; the tool description already has the usable ones.
                let why = if e.starts_with("unknown variant") || e.starts_with("missing field `cmd`") {
                    format!("there is no command named {name:?}")
                } else {
                    e
                };
                return Err(format!(
                    "command {} ({name}) is not a valid command: {why}\nNothing was applied. Each command is a JSON object with a \"cmd\" field in snake_case and exactly the fields shown in this tool's description.",
                    i + 1
                ));
            }
        }
    }
    Ok(batch_outcome(ed, a.rev.as_deref(), cmds, "Applied edits"))
}

fn set_captions(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: CaptionsIn = parse(input)?;
    let mut c = comp_of(ed)?.captions.clone();
    c.enabled = a.enabled.unwrap_or(true);
    if let Some(v) = &a.color {
        c.color = parse_color(v)?;
    }
    if let Some(v) = &a.highlight_color {
        c.highlight_color = parse_color(v)?;
    }
    if let Some(v) = &a.highlight {
        c.highlight = match v.trim().to_lowercase().as_str() {
            "none" => CaptionHighlight::None,
            "active_word" => CaptionHighlight::ActiveWord,
            "reveal" => CaptionHighlight::Reveal,
            other => return Err(format!("highlight {other:?} is not valid; use none, active_word or reveal")),
        };
    }
    if let Some(v) = &a.background {
        c.background = optional_color(v)?;
    }
    if let Some(v) = &a.outline_color {
        let width = a.outline_width.or(c.outline.map(|o| o.1)).unwrap_or(2.0);
        c.outline = optional_color(v)?.map(|col| (col, width));
    } else if let (Some(w), Some((col, _))) = (a.outline_width, c.outline) {
        c.outline = Some((col, w));
    }
    if let Some((_, w)) = c.outline
        && !(w.is_finite() && (0.0..=20.0).contains(&w)) {
            return Err("outline_width must be between 0 and 20".into());
        }
    if let Some(v) = &a.font {
        if v.trim().is_empty() {
            return Err("font is empty".into());
        }
        c.font = v.trim().to_string();
    }
    if let Some(v) = a.size {
        if !(v.is_finite() && (0.01..=0.3).contains(&v)) {
            return Err("size is a fraction of canvas height and must be between 0.01 and 0.3".into());
        }
        c.size = v;
    }
    if let Some(v) = &a.position {
        c.y = match v.trim().to_lowercase().as_str() {
            "top" => 0.12,
            "middle" | "center" | "centre" => 0.5,
            "bottom" => 0.86,
            other => return Err(format!("position {other:?} is not valid; use top, middle or bottom")),
        };
    }
    if let Some(v) = a.max_lines {
        if !(1..=4).contains(&v) {
            return Err("max_lines must be between 1 and 4".into());
        }
        c.max_lines = v;
    }
    if let Some(v) = a.max_chars_per_line {
        if !(8..=80).contains(&v) {
            return Err("max_chars_per_line must be between 8 and 80".into());
        }
        c.max_chars_per_line = v;
    }
    if let Some(v) = a.uppercase {
        c.uppercase = v;
    }
    if let Some(v) = a.bold {
        c.bold = v;
    }
    let desc = if c.enabled {
        format!(
            "Captions are on: color {}, {} line(s) of up to {} chars, {}size {:.3}, y {:.2}, highlight {:?}.",
            c.color.hex(),
            c.max_lines,
            c.max_chars_per_line,
            if c.uppercase { "uppercase, " } else { "" },
            c.size,
            c.y,
            c.highlight
        )
    } else {
        "Captions are off.".to_string()
    };
    let summary = if c.enabled { "Captions on" } else { "Captions off" };
    ed.apply(Command::SetCaptions { captions: c }).map_err(|e| e.to_string())?;
    Ok(ToolOutcome { text: format!("OK. {desc}"), ok: true, commands: 1, summary: summary.into() })
}

fn animation(s: &str) -> Result<Animation, String> {
    Ok(match s.trim().to_lowercase().as_str() {
        "none" => Animation::None,
        "fade" => Animation::Fade,
        "slide_up" => Animation::SlideUp,
        "slide_down" => Animation::SlideDown,
        "slide_left" => Animation::SlideLeft,
        "slide_right" => Animation::SlideRight,
        "zoom_in" => Animation::ZoomIn,
        "zoom_out" => Animation::ZoomOut,
        other => return Err(format!("animation {other:?} is not valid")),
    })
}

fn add_text_layer(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: TextLayerIn = parse(input)?;
    let text = a.text.trim();
    if text.is_empty() {
        return Err("text is empty".into());
    }
    let comp = comp_of(ed)?;
    if a.start_token.is_some() || a.end_token.is_some() {
        check_rev(comp, a.rev.as_deref().ok_or("`rev` is required with start_token / end_token")?)?;
    }
    let len = comp.script.len();
    let token_time = |i: usize, what: &str| if i <= len { Ok(comp.time_of_token(i)) } else { Err(format!("{what} {i} is outside the script (0..={len})")) };
    let start = match a.start_token {
        Some(i) => token_time(i, "start_token")?,
        None => a.start.unwrap_or(0.0),
    };
    let duration = match a.end_token {
        Some(i) => token_time(i, "end_token")? - start,
        None => a.duration.unwrap_or(3.0),
    };
    if !(start.is_finite() && duration.is_finite()) || start < 0.0 {
        return Err("start must be 0 or more".into());
    }
    if duration <= 0.0 || duration > 86_400.0 {
        return Err(format!("the layer would last {duration:.2}s; it must end after it starts"));
    }
    let mut style = TextStyle { text: text.to_string(), ..Default::default() };
    if let Some(v) = a.size {
        if !(v.is_finite() && (0.01..=0.5).contains(&v)) {
            return Err("size is a fraction of canvas height and must be between 0.01 and 0.5".into());
        }
        style.size = v;
    }
    if let Some(v) = &a.color {
        style.color = parse_color(v)?;
    }
    if let Some(v) = &a.background {
        style.background = optional_color(v)?;
    }
    if let Some(v) = a.bold {
        style.bold = v;
    }
    if let Some(v) = a.italic {
        style.italic = v;
    }
    if let Some(v) = &a.font
        && !v.trim().is_empty() {
            style.font = v.trim().to_string();
        }
    if let Some(v) = &a.align {
        style.align = match v.trim().to_lowercase().as_str() {
            "left" => TextAlign::Left,
            "center" | "centre" => TextAlign::Center,
            "right" => TextAlign::Right,
            other => return Err(format!("align {other:?} is not valid")),
        };
    }
    let y = match a.position.as_deref().map(|p| p.trim().to_lowercase()).as_deref() {
        None | Some("center") | Some("centre") | Some("middle") => 0.5,
        Some("top") => 0.14,
        Some("lower_third") => 0.74,
        Some("bottom") => 0.86,
        Some(other) => return Err(format!("position {other:?} is not valid; use top, center, lower_third or bottom")),
    };
    let name = a.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| text.chars().take(40).collect());
    let mut layer = Layer::new(name, start, duration, LayerContent::Text(style));
    layer.transform = Transform { x: 0.5, y, w: 0.9, h: 0.2, rotation_deg: 0.0, opacity: 1.0 };
    layer.track = comp.layers.iter().map(|l| l.track).max().unwrap_or(0).saturating_add(1).clamp(1, 99);
    if let Some(v) = &a.animation {
        let anim = animation(v)?;
        layer.animation_in = anim;
        layer.animation_out = anim;
        if anim == Animation::Fade {
            layer.fade_in = (duration / 4.0).min(0.4);
            layer.fade_out = (duration / 4.0).min(0.4);
        }
    }
    let id = layer.id;
    let end = layer.end();
    let comp_dur = comp.duration();
    ed.apply(Command::AddLayer { layer }).map_err(|e| e.to_string())?;
    let mut text_out = format!("OK: added text layer id {id} showing {text:?} from {start:.2}s to {end:.2}s.");
    if end > comp_dur + 0.01 && comp_dur > 0.0 {
        let _ = write!(text_out, " Note: this runs past the end of the script ({comp_dur:.2}s), which makes the composition longer.");
    }
    Ok(ToolOutcome { text: text_out, ok: true, commands: 1, summary: format!("Added text “{text}”") })
}

fn update_layer(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: UpdateLayerIn = parse(input)?;
    let comp = comp_of(ed)?;
    let mut layer = comp.layers.iter().find(|l| l.id.to_string() == a.id.trim()).cloned().ok_or_else(|| format!("there is no layer with id {:?}; layer ids are listed in the project overview", a.id))?;
    if let Some(v) = &a.name {
        layer.name = v.trim().to_string();
    }
    if let Some(v) = a.start {
        layer.start = v;
    }
    if let Some(v) = a.duration {
        layer.duration = v;
    }
    if let Some(v) = a.hidden {
        layer.hidden = v;
    }
    if let Some(v) = a.opacity {
        layer.transform.opacity = v.clamp(0.0, 1.0);
    }
    if let Some(v) = a.x {
        layer.transform.x = v;
    }
    if let Some(v) = a.y {
        layer.transform.y = v;
    }
    if let Some(v) = a.fade_in {
        layer.fade_in = v.max(0.0);
    }
    if let Some(v) = a.fade_out {
        layer.fade_out = v.max(0.0);
    }
    if a.text.is_some() || a.color.is_some() || a.size.is_some() {
        let LayerContent::Text(style) = &mut layer.content else {
            return Err("text, color and size only apply to text layers".into());
        };
        if let Some(v) = &a.text {
            if v.trim().is_empty() {
                return Err("text is empty".into());
            }
            style.text = v.trim().to_string();
        }
        if let Some(v) = &a.color {
            style.color = parse_color(v)?;
        }
        if let Some(v) = a.size {
            if !(v.is_finite() && (0.01..=0.5).contains(&v)) {
                return Err("size must be between 0.01 and 0.5".into());
            }
            style.size = v;
        }
    }
    let cmd = Command::UpdateLayer { layer: layer.clone() };
    validate(ed, &cmd)?;
    ed.apply(cmd).map_err(|e| e.to_string())?;
    Ok(ToolOutcome {
        text: format!("OK: layer {} {:?} is now {:.2}s–{:.2}s{}.", layer.id, layer.name, layer.start, layer.end(), if layer.hidden { ", hidden" } else { "" }),
        ok: true,
        commands: 1,
        summary: format!("Updated layer “{}”", layer.name),
    })
}

fn set_canvas(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: CanvasIn = parse(input)?;
    let (width, height) = match (a.aspect.as_deref().map(str::trim), a.width, a.height) {
        (Some(asp), None, None) => match asp {
            "16:9" => AspectPreset::Landscape16x9,
            "9:16" => AspectPreset::Portrait9x16,
            "1:1" => AspectPreset::Square1x1,
            "4:5" => AspectPreset::Portrait4x5,
            "4:3" => AspectPreset::Classic4x3,
            other => return Err(format!("aspect {other:?} is not one of 16:9, 9:16, 1:1, 4:5, 4:3; for other shapes pass width and height")),
        }
        .size(),
        (None, Some(w), Some(h)) => (w, h),
        _ => return Err("pass either aspect, or both width and height".into()),
    };
    let fill = match a.fit.as_deref().map(|f| f.trim().to_lowercase()).as_deref() {
        None | Some("fill") => true,
        Some("fit") => false,
        Some(other) => return Err(format!("fit {other:?} is not valid; use fill or fit")),
    };
    comp_of(ed)?;
    ed.apply(Command::SetCanvas { width, height, fill }).map_err(|e| e.to_string())?;
    let c = comp_of(ed)?;
    Ok(ToolOutcome {
        text: format!("OK: the canvas is now {}x{}; the video {}.", c.width, c.height, if fill { "fills it (overflow cropped)" } else { "fits inside it (bars where needed)" }),
        ok: true,
        commands: 1,
        summary: format!("Canvas {}x{}", c.width, c.height),
    })
}

fn set_transitions(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: TransitionsIn = parse(input)?;
    let transition = match a.transition.trim().to_lowercase().as_str() {
        "cut" => Transition::Cut,
        "cross_dissolve" => Transition::CrossDissolve,
        "fade_through_black" => Transition::FadeThroughBlack,
        "wipe_left" => Transition::WipeLeft,
        "wipe_right" => Transition::WipeRight,
        "slide_left" => Transition::SlideLeft,
        "slide_up" => Transition::SlideUp,
        other => return Err(format!("transition {other:?} is not valid")),
    };
    let secs = a.secs.unwrap_or(0.5);
    if !secs.is_finite() || !(0.05..=5.0).contains(&secs) {
        return Err("secs must be between 0.05 and 5".into());
    }
    let scenes = comp_of(ed)?.scene_spans().len();
    if scenes < 2 {
        return Err("this composition has a single scene, so there is no scene boundary to put a transition on. Add scene breaks first (apply_commands set_scene_break at the token where a new scene should start), then set the transition.".into());
    }
    ed.apply(Command::SetAllTransitions { transition, secs }).map_err(|e| e.to_string())?;
    Ok(ToolOutcome {
        text: format!("OK: {} at all {} scene boundaries, {secs:.2}s each.", transition.label(), scenes - 1),
        ok: true,
        commands: 1,
        summary: format!("Transitions: {}", transition.label()),
    })
}

fn add_chapters(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ChaptersIn = parse(input)?;
    let comp = comp_of(ed)?;
    check_rev(comp, &a.rev)?;
    if a.chapters.is_empty() {
        return Err("no chapters were given".into());
    }
    let sentences = context::sentences(comp);
    let mut placed: Vec<(usize, String, Option<usize>)> = Vec::new(); // (token, title, snapped from)
    for c in &a.chapters {
        let title = c.title.trim();
        if title.is_empty() {
            return Err(format!("the chapter at {} has an empty title", c.at));
        }
        let Some(at) = context::snap_to_sentence_start(comp, &sentences, c.at) else {
            return Err(format!("chapter {title:?}: token {} is outside the script (valid tokens are 0..{}). Nothing was added.", c.at, comp.script.len()));
        };
        if let Some(prev) = placed.iter().find(|p| p.0 == at) {
            return Err(format!("chapters {:?} and {title:?} both fall in the sentence starting at token {at}; give each chapter its own starting sentence. Nothing was added.", prev.1));
        }
        placed.push((at, title.chars().take(120).collect(), (at != c.at).then_some(c.at)));
    }
    placed.sort_by_key(|p| p.0);
    // Video platforms reject chapters shorter than 10 seconds, and they are a sign of over-splitting.
    for w in placed.windows(2) {
        let gap = comp.time_of_token(w[1].0) - comp.time_of_token(w[0].0);
        if gap < crate::tools::MIN_CHAPTER_SPACING {
            return Err(format!(
                "chapters {:?} and {:?} would be only {gap:.1}s apart; chapters must be at least {:.0} seconds apart. A chapter is a whole topic: merge the short ones and send the list again. Nothing was added.",
                w[0].1,
                w[1].1,
                crate::tools::MIN_CHAPTER_SPACING
            ));
        }
    }
    let mut cmds = Vec::new();
    if a.replace_existing.unwrap_or(false) {
        for (i, _, _) in comp.script_markers() {
            if !placed.iter().any(|p| p.0 == i) {
                cmds.push(Command::SetMarker { at: i, name: None });
            }
        }
    }
    for (at, title, _) in &placed {
        cmds.push(Command::SetMarker { at: *at, name: Some(title.clone()) });
    }
    let n = cmds.len();
    let snapshot = (ed.project.clone(), ed.active);
    if let Err(e) = ed.apply_all("Add chapters", cmds) {
        ed.project = snapshot.0;
        ed.active = snapshot.1;
        return Err(format!("{e}. Nothing was added."));
    }
    let comp = comp_of(ed)?;
    let mut text = format!("OK: {} chapter marker{} set (token indices did not shift):\n", placed.len(), if placed.len() == 1 { "" } else { "s" });
    for (at, title, snapped) in &placed {
        let _ = write!(text, "- {} {:?} at token {} before {}", timecode(comp.time_of_token(*at)), title, at, excerpt(comp, *at, at + 8));
        if let Some(from) = snapped {
            let _ = write!(text, " (moved from token {from} to the start of its sentence)");
        }
        text.push('\n');
    }
    Ok(ToolOutcome { text, ok: true, commands: n, summary: format!("Added {} chapter{}", placed.len(), if placed.len() == 1 { "" } else { "s" }) })
}

/// Widens a token range to whole sentences. `None` if it contains no text.
pub(crate) fn snap_to_sentences(sentences: &[Sentence], start: usize, end: usize) -> Option<(usize, usize)> {
    let first = sentences.iter().position(|s| s.end > start)?;
    let last = sentences.iter().rposition(|s| s.start < end)?;
    (first <= last).then(|| (sentences[first].start, sentences[last].end))
}

fn create_clips(ed: &mut Editor, input: &Value) -> Result<ToolOutcome, String> {
    let a: ClipsIn = parse(input)?;
    let comp = comp_of(ed)?;
    check_rev(comp, &a.rev)?;
    if a.clips.is_empty() {
        return Err("no clips were given".into());
    }
    let source = comp.id;
    let len = comp.script.len();
    let sentences = context::sentences(comp);
    let snap = a.snap.unwrap_or(true);
    let mut plan: Vec<(usize, usize, String)> = Vec::new();
    for c in &a.clips {
        if c.start >= c.end || c.end > len {
            return Err(format!("clip {:?}: range {}..{} is not a valid range of the script (0..{len}); end is exclusive. Nothing was created.", c.name, c.start, c.end));
        }
        if c.name.trim().is_empty() {
            return Err("a clip has an empty name. Nothing was created.".into());
        }
        let (s, e) = if snap { snap_to_sentences(&sentences, c.start, c.end).unwrap_or((c.start, c.end)) } else { (c.start, c.end) };
        if comp.word_count(s..e) == 0 {
            return Err(format!("clip {:?}: range {}..{} contains no playing words. Nothing was created.", c.name, c.start, c.end));
        }
        plan.push((s, e, c.name.trim().to_string()));
    }
    let mut cmds = Vec::new();
    for (s, e, name) in &plan {
        cmds.push(Command::CompositionFromRange { start: *s, end: *e, name: name.clone() });
        cmds.push(Command::SelectComposition { id: source });
    }
    let before_count = ed.project.compositions.len();
    let n = cmds.len();
    let snapshot = (ed.project.clone(), ed.active);
    if let Err(e) = ed.apply_all("Create clips", cmds) {
        ed.project = snapshot.0;
        ed.active = snapshot.1;
        return Err(format!("{e}. Nothing was created."));
    }
    let mut text = format!("OK: created {} composition{}; the active composition is unchanged and its indices did not shift:\n", plan.len(), if plan.len() == 1 { "" } else { "s" });
    for (k, (s, e, _)) in plan.iter().enumerate() {
        if let Some(c) = ed.project.compositions.get(before_count + k) {
            let src = comp_of(ed)?;
            let _ = writeln!(text, "- {:?} (id {}): tokens {}..{}, {:.1}s, {}", c.name, c.id, s, e, c.duration(), excerpt(src, *s, *e));
        }
    }
    Ok(ToolOutcome { text, ok: true, commands: n, summary: format!("Created {} clip{}", plan.len(), if plan.len() == 1 { "" } else { "s" }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_command_example_parses() {
        for (ex, _) in COMMAND_EXAMPLES {
            let v: Value = serde_json::from_str(ex).unwrap_or_else(|e| panic!("{ex}: {e}"));
            serde_json::from_value::<Command>(v).unwrap_or_else(|e| panic!("{ex}: {e}"));
        }
    }

    #[test]
    fn align_tracks_deletes_inserts_and_moves() {
        let k = |n: u64| -> Key { (0, [0; 16], n, n + 1) };
        let old: Vec<Key> = (0..8).map(k).collect();
        // delete 2..4
        let new: Vec<Key> = [0, 1, 4, 5, 6, 7].into_iter().map(k).collect();
        assert_eq!(align(&old, &new), vec![Some(0), Some(1), None, None, Some(2), Some(3), Some(4), Some(5)]);
        // move 1..3 to the end
        let moved: Vec<Key> = [0, 3, 4, 5, 6, 7, 1, 2].into_iter().map(k).collect();
        assert_eq!(align(&old, &moved), vec![Some(0), Some(6), Some(7), Some(1), Some(2), Some(3), Some(4), Some(5)]);
        // insert one in the middle
        let ins: Vec<Key> = [0, 1, 2, 99, 3, 4, 5, 6, 7].into_iter().map(k).collect();
        assert_eq!(align(&old, &ins)[3], Some(4));
    }
}
