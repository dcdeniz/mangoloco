//! Offline tests of the chat co-editor: the tool-use loop, undo semantics,
//! index safety, cancellation and error handling, against a scripted mock.
mod common;

use common::*;
use mango_ai::context::script_rev;
use mango_ai::{AiError, Assistant, ChatMessage, Event, Role};
use mango_core::detect::{self, FillerOptions};
use mango_core::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn rev(e: &Editor) -> String {
    script_rev(e.comp().unwrap())
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// Runs one turn against scripted replies and returns (result, events, mock).
fn run(e: &mut Editor, user: &str, replies: Vec<Reply>) -> (Result<mango_ai::TurnResult, AiError>, Vec<Event>, Mock) {
    let mock = Mock::start(replies);
    let mut a = Assistant::new(mock_client(&mock));
    let mut events = Vec::new();
    let r = a.send(e, user, &mut |ev| events.push(ev), &no_cancel());
    (r, events, mock)
}

fn call(name: &str, input: Value) -> Reply {
    Reply::Sse(message(vec![tool_use("toolu_1", name, input)]))
}

fn say(s: &str) -> Reply {
    Reply::Sse(message(vec![text(s)]))
}

#[test]
fn tool_loop_end_to_end_is_one_undo_step_and_one_undo_restores_everything() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let um = word_index(&e, "um");
    let uh = word_index(&e, "uh");

    let (r, events, mock) = run(
        &mut e,
        "Remove the first um and the first uh, and turn on captions.",
        vec![
            Reply::Sse(message(vec![text("Let me look."), tool_use("toolu_a", "search_script", json!({"query": "um"}))])),
            Reply::Sse(message(vec![
                tool_use("toolu_b", "apply_commands", json!({"rev": r0, "commands": [
                    {"cmd": "delete_range", "start": um, "end": um + 1},
                    {"cmd": "delete_range", "start": uh, "end": uh + 1},
                ]})),
                tool_use("toolu_c", "set_captions", json!({"color": "#FFFF00"})),
            ])),
            say("Removed two fillers and turned captions on."),
        ],
    );
    let r = r.unwrap();
    assert_eq!(r.reply, "Let me look.\n\nRemoved two fillers and turned captions on.");
    assert!(r.changed);
    assert_eq!(r.undo_steps, 1);
    assert_eq!(r.commands_applied, 3);
    assert_eq!(r.usage.requests, 3);

    // The edits landed.
    let words = playing_words(&e);
    assert_eq!(words.iter().filter(|w| *w == "um").count(), 4, "one of five ums removed");
    assert_eq!(words.iter().filter(|w| *w == "uh").count(), 3, "one of four uhs removed");
    assert_eq!(words[0], "hello");
    assert!(e.comp().unwrap().captions.enabled);
    assert_eq!(e.comp().unwrap().captions.color, Rgba::rgb(255, 255, 0));

    // Exactly one undo step, labelled, and a single undo restores the original project.
    assert_eq!(undo_depth(&mut e), depth + 1);
    assert_eq!(e.undo_label(), Some("Assistant: Remove the first um and the first uh, and turn o…"));
    assert!(e.is_dirty());
    assert!(e.undo());
    assert_eq!(e.project, original);
    assert!(e.redo());
    assert!(e.comp().unwrap().captions.enabled);
    assert_eq!(playing_words(&e).len(), words.len());

    // Events: streamed text, then tools in order, then Done.
    let streamed: String = events.iter().filter_map(|ev| if let Event::Text(t) = ev { Some(t.as_str()) } else { None }).collect();
    assert_eq!(streamed, "Let me look.\n\nRemoved two fillers and turned captions on.");
    let tools: Vec<String> = events
        .iter()
        .filter_map(|ev| match ev {
            Event::ToolStarted { name, .. } => Some(format!("start {name}")),
            Event::ToolFinished { name, ok, .. } => Some(format!("end {name} {ok}")),
            _ => None,
        })
        .collect();
    assert_eq!(tools, ["start search_script", "end search_script true", "start apply_commands", "end apply_commands true", "start set_captions", "end set_captions true"]);
    assert_eq!(events.last(), Some(&Event::Done));

    // What the model was sent.
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3);
    let first_user = &reqs[0].body["messages"][0]["content"];
    assert!(first_user[0]["text"].as_str().unwrap().contains("<project_state>"));
    assert!(first_user[0]["text"].as_str().unwrap().contains(&format!("{um}:Um,")));
    assert_eq!(first_user[1]["text"], "Remove the first um and the first uh, and turn on captions.");
    let results = tool_results(&reqs[1]);
    assert_eq!(results.len(), 1);
    assert!(results[0].0.contains("5 matches") && !results[0].1, "{}", results[0].0);
    // Both results of the second assistant message go back in one user message.
    let last = reqs[2].body["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"].as_array().unwrap().len(), 2);
    assert_eq!(last["content"][0]["tool_use_id"], "toolu_b");
    assert_eq!(last["content"][1]["tool_use_id"], "toolu_c");
    let batch = &tool_results(&reqs[2])[1].0;
    assert!(batch.contains("OK: applied 2 commands") && batch.contains("TOKEN INDICES SHIFTED"), "{batch}");
    // The cached prefix (system + tools) is byte-identical on every request of the conversation.
    for r in &reqs {
        assert_eq!(r.body["system"], reqs[0].body["system"]);
        assert_eq!(r.body["tools"], reqs[0].body["tools"]);
        assert_eq!(r.body["system"][0]["cache_control"], json!({"type": "ephemeral", "ttl": "1h"}));
        assert_eq!(r.body["cache_control"], json!({"type": "ephemeral"}));
    }
    assert!(reqs[0].body["tools"].as_array().unwrap().len() >= 15);
}

#[test]
fn a_failing_batch_is_reported_to_the_model_and_nothing_is_applied() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let len = e.comp().unwrap().script.len();
    let (r, _, mock) = run(
        &mut e,
        "cut stuff",
        vec![
            call("apply_commands", json!({"rev": r0, "commands": [
                {"cmd": "ignore_range", "start": 3, "end": 13, "ignored": true},
                {"cmd": "delete_range", "start": len - 2, "end": len + 50},
            ]})),
            say("I could not do that."),
        ],
    );
    let r = r.unwrap();
    assert!(!r.changed);
    assert_eq!((r.undo_steps, r.commands_applied), (0, 0));
    assert_eq!(e.project, original, "the valid first command must have been rolled back");
    assert_eq!(undo_depth(&mut e), depth);
    assert!(!e.is_dirty() || depth > 0);
    let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
    assert!(is_error);
    assert!(text_.contains("command 2 of 2 (delete_range) failed"), "{text_}");
    assert!(text_.contains("outside the script"), "{text_}");
    assert!(text_.contains("Nothing was applied"), "{text_}");
}

#[test]
fn invalid_and_forbidden_commands_are_rejected_with_the_exact_reason() {
    let cases: Vec<(Value, &str)> = vec![
        (json!({"cmd": "explode_everything"}), "not a valid command"),
        (json!({"cmd": "delete_range", "start": "three", "end": 5}), "not a valid command"),
        (json!({"cmd": "delete_range", "start": 9, "end": 9}), "is empty"),
        (json!({"cmd": "delete_range", "start": 9, "end": 4}), "is empty"),
        (json!({"cmd": "set_marker", "at": 100000, "name": "x"}), "outside the script"),
        (json!({"cmd": "set_gap_duration", "at": 3, "secs": 0.2}), "is a word, not a pause"),
        (json!({"cmd": "remove_asset", "id": "00000000-0000-0000-0000-000000000000"}), "not available to the assistant"),
        (json!({"cmd": "remove_layer", "id": "00000000-0000-0000-0000-000000000000"}), "unknown layer"),
        (json!({"cmd": "remove_matches", "query": "zebra", "mode": "Delete"}), "was not found"),
        (json!({"cmd": "correct_all", "from": "zebra", "to": "horse"}), "does not occur"),
        (json!({"cmd": "set_canvas", "width": 4, "height": 4, "fill": true}), "canvas size"),
        (json!({"cmd": "delete_composition"}), "only composition"),
        (json!({"cmd": "restore_at", "at": 5}), "no cut before token 5"),
    ];
    for (cmd, expect) in cases {
        let mut e = podcast_editor();
    let r0 = rev(&e);
        let original = e.project.clone();
        let (r, _, mock) = run(&mut e, "x", vec![call("apply_commands", json!({"rev": r0, "commands": [cmd.clone()]})), say("no")]);
        assert!(!r.unwrap().changed, "{cmd}");
        assert_eq!(e.project, original, "{cmd}");
        let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
        assert!(*is_error && text_.contains(expect), "{cmd} -> {text_}");
    }
}

#[test]
fn indices_in_one_call_all_refer_to_the_revision_that_was_read() {
    // Delete three separated ranges and ignore a fourth, all in original indices and out of order.
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let script = e.comp().unwrap().script.clone();
    let (um, uh, bread, marathon) = (word_index(&e, "um"), word_index(&e, "uh"), word_index(&e, "bread"), word_index(&e, "marathon"));
    let ignored_words: Vec<String> = script[marathon - 4..marathon + 1].iter().filter(|t| t.is_word()).map(|t| t.text.clone()).collect();
    let before = playing_words(&e);
    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            call("apply_commands", json!({"rev": r0, "commands": [
                {"cmd": "delete_range", "start": bread, "end": bread + 1},
                {"cmd": "delete_range", "start": um, "end": um + 1},
                {"cmd": "ignore_range", "start": marathon - 4, "end": marathon + 1, "ignored": true},
                {"cmd": "delete_range", "start": uh, "end": uh + 1},
                {"cmd": "set_marker", "at": marathon + 6, "name": "After"},
            ]})),
            say("ok"),
        ],
    );
    assert!(r.unwrap().changed);
    let comp = e.comp().unwrap();
    // Exactly the addressed words are gone or struck through.
    let got_ignored: Vec<String> = comp.script.iter().filter(|t| t.is_word() && t.ignored).map(|t| t.text.clone()).collect();
    assert_eq!(got_ignored, ignored_words);
    let mut expected = before.clone();
    for w in ["um", "uh", "bread"] {
        let pos = expected.iter().position(|x| x == w).unwrap();
        expected.remove(pos);
    }
    let ignored_norm: Vec<String> = ignored_words.iter().map(|w| script::normalize_word(w)).collect();
    let start = expected.windows(ignored_norm.len()).position(|w| w == ignored_norm.as_slice()).unwrap();
    expected.drain(start..start + ignored_norm.len());
    assert_eq!(playing_words(&e), expected);
    // The marker sits on the token that was at `marathon + 6` in the original script.
    let (mi, _, name) = comp.script_markers().into_iter().next().unwrap();
    assert_eq!(name, "After");
    assert_eq!((comp.script[mi].src_start, comp.script[mi].src_end), (script[marathon + 6].src_start, script[marathon + 6].src_end));
    assert!(tool_results(&mock.requests()[1])[0].0.contains("TOKEN INDICES SHIFTED"));
}

#[test]
fn addressing_a_token_destroyed_earlier_in_the_call_fails_and_rolls_back() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            call("apply_commands", json!({"rev": r0, "commands": [
                {"cmd": "delete_range", "start": 3, "end": 13},
                {"cmd": "ignore_range", "start": 8, "end": 20, "ignored": true},
            ]})),
            say("ok"),
        ],
    );
    assert!(!r.unwrap().changed);
    assert_eq!(e.project, original);
    let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
    assert!(*is_error && text_.contains("token 8 was removed or changed by an earlier command"), "{text_}");
}

#[test]
fn edit_ranges_takes_many_ranges_in_read_coordinates_and_merges_overlaps() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let before = playing_words(&e);
    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            // 3..8 "hello and welcome back to", overlapping 6..11 "back to the the Kitchen"; and a later range.
            call("edit_ranges", json!({"rev": r0, "action": "delete", "ranges": [{"start": 6, "end": 11}, {"start": 58, "end": 62}, {"start": 3, "end": 8}]})),
            say("ok"),
        ],
    );
    assert!(r.unwrap().changed);
    let mut expected = before.clone();
    let cut = |v: &mut Vec<String>, phrase: &str| {
        let p: Vec<String> = phrase.split(' ').map(str::to_string).collect();
        let at = v.windows(p.len()).position(|w| w == p.as_slice()).unwrap_or_else(|| panic!("{phrase}"));
        v.drain(at..at + p.len());
    };
    cut(&mut expected, "hello and welcome back to the the kitchen");
    cut(&mut expected, "thanks ana");
    assert_eq!(playing_words(&e), expected);
    let out = &tool_results(&mock.requests()[1])[0].0;
    assert!(out.contains("“hello and welcome back to the the Kitchen”") && out.contains("“Thanks, Ana.”"), "{out}");
}

#[test]
fn stale_indices_are_refused_after_a_shift_and_accepted_with_the_new_revision() {
    let mut e = podcast_editor();
    let rev0 = rev(&e);
    // What the script will look like after the first delete, to script the model's corrected call.
    let mut probe = podcast_editor();
    probe.project = e.project.clone();
    probe.active = e.active;
    probe.apply(Command::DeleteRange { start: 0, end: 3 }).unwrap();
    let rev1 = rev(&probe);
    assert_ne!(rev0, rev1);
    let table = probe.comp().unwrap().script.iter().position(|t| t.text == "Table").unwrap();

    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            call("edit_ranges", json!({"rev": rev0, "action": "delete", "ranges": [{"start": 0, "end": 3}]})),
            // The model wrongly reuses the old revision: refused.
            call("edit_ranges", json!({"rev": rev0, "action": "ignore", "ranges": [{"start": 10, "end": 12}]})),
            // No revision at all: refused.
            call("apply_commands", json!({"commands": [{"cmd": "delete_range", "start": 10, "end": 12}]})),
            // With the revision it was told about: accepted.
            call("edit_ranges", json!({"rev": rev1, "action": "ignore", "ranges": [{"start": table, "end": table + 1}]})),
            say("ok"),
        ],
    );
    let r = r.unwrap();
    assert!(r.changed);
    assert_eq!(r.commands_applied, 2);
    let reqs = mock.requests();
    let first = &tool_results(&reqs[1])[0].0;
    assert!(first.contains(&format!("the script is now rev {rev1}")), "{first}");
    let (stale, stale_err) = &tool_results(&reqs[2])[1];
    assert!(*stale_err && stale.contains("stale indices") && stale.contains(&rev1), "{stale}");
    let (norev, norev_err) = &tool_results(&reqs[3])[2];
    assert!(*norev_err && norev.contains("`rev` is required"), "{norev}");
    let (ok, ok_err) = &tool_results(&reqs[4])[3];
    assert!(!ok_err && ok.contains("ignore “Table”") && ok.contains("Token indices did not shift"), "{ok}");
    let ignored: Vec<&str> = e.comp().unwrap().script.iter().filter(|t| t.ignored).map(|t| t.text.as_str()).collect();
    assert_eq!(ignored, ["Table"]);
}

#[test]
fn cancel_after_an_applied_batch_leaves_the_project_untouched() {
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let mock = Mock::start(vec![call("remove_fillers", json!({})), say("never reached")]);
    let mut a = Assistant::new(mock_client(&mock));
    let cancel = AtomicBool::new(false);
    let mut saw_tool = false;
    let r = a.send(
        &mut e,
        "remove fillers",
        &mut |ev| {
            if let Event::ToolFinished { ok, .. } = ev {
                saw_tool = ok;
                cancel.store(true, Ordering::SeqCst); // the user hits Stop right after the first edit
            }
            assert!(!ev_is_done(&ev), "Done must not be sent for a cancelled turn");
        },
        &cancel,
    );
    assert_eq!(r.unwrap_err(), AiError::Cancelled);
    assert!(saw_tool, "the batch had been applied to the working copy");
    assert_eq!(e.project, original);
    assert_eq!(undo_depth(&mut e), depth);
    assert!(a.history().is_empty());
    assert_eq!(mock.requests().len(), 1, "no request after cancel");

    // The conversation was rolled back too: the next turn starts clean.
    let mock2 = Mock::start(vec![say("hi")]);
    let mut a = Assistant::new(mock_client(&mock2));
    a.send(&mut e, "hello", &mut |_| {}, &no_cancel()).unwrap();
    assert_eq!(mock2.requests()[0].body["messages"].as_array().unwrap().len(), 1);
}

fn ev_is_done(ev: &Event) -> bool {
    matches!(ev, Event::Done)
}

#[test]
fn cancel_takes_effect_while_a_response_is_streaming() {
    let mut e = podcast_editor();
    let original = e.project.clone();
    let mock = Mock::start(vec![Reply::Stall(message(vec![text("Thinking about it")]), Duration::from_secs(20))]);
    let mut a = Assistant::new(mock_client(&mock));
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::SeqCst);
        });
        a.send(&mut e, "do something", &mut |_| {}, &cancel)
    });
    assert_eq!(r.unwrap_err(), AiError::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(3), "cancel took {:?}", t.elapsed());
    assert_eq!(e.project, original);

    // Already cancelled before the turn starts: no request at all.
    let mock = Mock::start(vec![say("never")]);
    let mut a = Assistant::new(mock_client(&mock));
    assert_eq!(a.send(&mut e, "x", &mut |_| {}, &AtomicBool::new(true)).unwrap_err(), AiError::Cancelled);
    assert!(mock.requests().is_empty());
}

#[test]
fn an_unknown_tool_name_gets_a_tool_error_and_the_turn_continues() {
    let mut e = podcast_editor();
    let original = e.project.clone();
    let (r, events, mock) = run(&mut e, "x", vec![call("launch_rocket", json!({"target": "moon"})), say("Sorry, I cannot do that.")]);
    let r = r.unwrap();
    assert_eq!(r.reply, "Sorry, I cannot do that.");
    assert!(!r.changed);
    assert_eq!(e.project, original);
    let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
    assert!(*is_error && text_.contains("there is no tool named \"launch_rocket\""), "{text_}");
    assert!(events.contains(&Event::ToolFinished { name: "launch_rocket".into(), ok: false, summary: "there is no tool named \"launch_rocket\". Use one of the tools you were given.".into() }));
}

#[test]
fn bad_tool_input_is_returned_to_the_model_not_executed() {
    // Input that is not valid JSON (stream cut inside a string).
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let mut broken = tool_use("toolu_1", "apply_commands", json!({}));
    broken["raw_input"] = json!("{\"commands\": [{\"cmd\": \"delete_range\", \"start\": 0, \"end\": 30}");
    let (r, _, mock) = run(&mut e, "x", vec![Reply::Sse(message(vec![broken])), say("ok")]);
    assert!(!r.unwrap().changed);
    assert_eq!(e.project, original);
    let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
    assert!(*is_error && text_.contains("INVALID_JSON") && text_.contains("delete_range"), "{text_}");
    // The echoed assistant turn still carries a well-formed (empty) input object.
    assert_eq!(mock.requests()[1].body["messages"][1]["content"][0]["input"], json!({}));

    // Valid JSON with the wrong shape, unknown fields and wrong types.
    for (tool, input, expect) in [
        ("edit_ranges", json!({"rev": "x", "action": "ignore"}), "missing field `ranges`"),
        ("edit_ranges", json!({"rev": r0, "action": "obliterate", "ranges": [{"start": 1, "end": 2}]}), "action \"obliterate\" is not valid"),
        ("read_script", json!({"start": -4}), "invalid tool input"),
        ("read_script", json!({"start": 999999}), "outside the script"),
        ("set_captions", json!({"colour": "red"}), "unknown field `colour`"),
        ("set_captions", json!({"color": "sparkly"}), "is not a colour"),
        ("set_canvas", json!({"aspect": "7:3"}), "is not one of"),
        ("add_text_layer", json!({"text": "  "}), "text is empty"),
        ("add_text_layer", json!({"text": "Hi", "duration": -1}), "must end after it starts"),
        ("create_clips", json!({"rev": r0, "clips": [{"start": 5, "end": 99999, "name": "x"}]}), "not a valid range"),
        ("add_chapters", json!({"rev": r0, "chapters": [{"at": 99999, "title": "x"}]}), "outside the script"),
        ("add_chapters", json!({"rev": rev(&e), "chapters": [{"at": 1, "title": "Hello"}, {"at": 14, "title": "Name"}]}), "chapters must be at least 10 seconds apart"),
        ("add_chapters", json!({"rev": rev(&e), "chapters": [{"at": 70, "title": "Bread"}, {"at": 72, "title": "Also bread"}]}), "both fall in the sentence"),
        ("apply_commands", json!({"commands": []}), "no commands were given"),
        ("apply_commands", json!({"commands": [{"cmd": "noop"}]}), "there is no command named \"noop\""),
        ("set_transitions", json!({"transition": "cross_dissolve"}), "single scene"),
        ("update_layer", json!({"id": "nope", "hidden": true}), "there is no layer with id"),
    ] {
        let (r, _, mock) = run(&mut e, "x", vec![call(tool, input.clone()), say("ok")]);
        assert!(!r.unwrap().changed, "{tool} {input}");
        assert_eq!(e.project, original, "{tool} {input}");
        let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
        assert!(*is_error && text_.contains(expect), "{tool} {input} -> {text_}");
    }
}

#[test]
fn a_tool_call_cut_off_by_max_tokens_is_not_run() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let mut cut = message(vec![tool_use("toolu_1", "edit_ranges", json!({"rev": r0, "action": "delete", "ranges": [{"start": 0, "end": 40}]}))]);
    cut["stop_reason"] = json!("max_tokens");
    let (r, _, mock) = run(&mut e, "x", vec![Reply::Sse(cut), say("ok")]);
    assert!(!r.unwrap().changed);
    assert_eq!(e.project, original);
    let (text_, is_error) = &tool_results(&mock.requests()[1])[0];
    assert!(*is_error && text_.contains("cut off"), "{text_}");
}

#[test]
fn a_question_adds_no_undo_step_and_history_carries_across_turns() {
    let mut e = podcast_editor();
    let depth = undo_depth(&mut e);
    let mock = Mock::start(vec![say("It is 1:59 long with two speakers."), call("set_canvas", json!({"aspect": "9:16"})), say("Now vertical."), say("Still vertical.")]);
    let mut a = Assistant::new(mock_client(&mock));
    let r = a.send(&mut e, "How long is this?", &mut |_| {}, &no_cancel()).unwrap();
    assert_eq!((r.changed, r.undo_steps, r.commands_applied), (false, 0, 0));
    assert_eq!(undo_depth(&mut e), depth);
    assert!(!e.is_dirty() || depth > 0);

    let r = a.send(&mut e, "Make it vertical.", &mut |_| {}, &no_cancel()).unwrap();
    assert!(r.changed);
    assert_eq!((e.comp().unwrap().width, e.comp().unwrap().height), (1080, 1920));
    assert_eq!(undo_depth(&mut e), depth + 1);
    a.send(&mut e, "And now?", &mut |_| {}, &no_cancel()).unwrap();

    let reqs = mock.requests();
    // Turn 2 sees turn 1; the unchanged project is not repeated.
    let m = reqs[1].body["messages"].as_array().unwrap();
    assert_eq!(m.len(), 3);
    assert_eq!(m[0]["content"][1]["text"], "How long is this?");
    assert_eq!(m[1]["content"][0]["text"], "It is 1:59 long with two speakers.");
    assert!(m[2]["content"][0]["text"].as_str().unwrap().contains("Unchanged since"));
    // Turn 3 gets the new state, because turn 2 changed the project.
    let m = reqs[3].body["messages"].as_array().unwrap();
    let state = m.last().unwrap()["content"][0]["text"].as_str().unwrap();
    assert!(state.contains("1080x1920 (9:16)"), "{state}");
    // Earlier messages are sent back byte-identical (append-only conversation).
    assert_eq!(reqs[3].body["messages"][0], reqs[0].body["messages"][0]);
    assert_eq!(reqs[3].body["messages"][2], reqs[1].body["messages"][2]);

    // History is what the user saw, and it serialises.
    let h = a.history().to_vec();
    assert_eq!(h.len(), 6);
    assert_eq!((h[0].role, h[0].text.as_str()), (Role::User, "How long is this?"));
    assert_eq!((h[3].role, h[3].text.as_str()), (Role::Assistant, "Now vertical."));
    assert_eq!(h[3].actions.len(), 1);
    assert_eq!(h[3].actions[0].summary, "Canvas 1080x1920");
    let json = serde_json::to_string(&h).unwrap();
    let back: Vec<ChatMessage> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, h);

    // A restored conversation replays as alternating plain-text turns.
    let mock2 = Mock::start(vec![say("Yes.")]);
    let mut restored = Assistant::from_history(mock_client(&mock2), back);
    assert_eq!(restored.history().len(), 6);
    restored.send(&mut e, "Is it still vertical?", &mut |_| {}, &no_cancel()).unwrap();
    let m = mock2.requests()[0].body["messages"].as_array().unwrap().clone();
    let roles: Vec<&str> = m.iter().map(|x| x["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant", "user", "assistant", "user"]);
    assert!(m[3]["content"].as_str().unwrap().contains("Canvas 1080x1920"));
    assert!(m[6]["content"][0]["text"].as_str().unwrap().contains("<project_state>"));

    a.reset();
    assert!(a.history().is_empty());
}

#[test]
fn an_api_error_mid_turn_changes_nothing_and_rolls_the_conversation_back() {
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let mock = Mock::start(vec![
        call("remove_fillers", json!({})),
        Reply::Status(401, vec![], api_error("authentication_error", "invalid x-api-key")),
        say("Hello again."),
    ]);
    let mut a = Assistant::new(mock_client(&mock));
    let mut events = Vec::new();
    let err = a.send(&mut e, "remove fillers", &mut |ev| events.push(ev), &no_cancel()).unwrap_err();
    assert_eq!(err, AiError::InvalidKey);
    assert_eq!(e.project, original, "edits of a failed turn must not be committed");
    assert_eq!(undo_depth(&mut e), depth);
    assert!(a.history().is_empty());
    assert!(!events.contains(&Event::Done));
    // The failed turn is gone from the conversation.
    a.send(&mut e, "hi", &mut |_| {}, &no_cancel()).unwrap();
    let m = mock.requests()[2].body["messages"].as_array().unwrap().clone();
    assert_eq!(m.len(), 1);
    assert!(m[0]["content"][0]["text"].as_str().unwrap().contains("<project_state>"), "the state is sent again after a rollback");

    // A refusal is an error too.
    let mut refused = message(vec![]);
    refused["stop_reason"] = json!("refusal");
    let (r, _, _) = run(&mut e, "x", vec![Reply::Sse(refused)]);
    assert_eq!(r.unwrap_err(), AiError::Refused);
    // And so is an empty message.
    let (r, _, mock) = run(&mut e, "   ", vec![]);
    assert!(matches!(r.unwrap_err(), AiError::NothingToDo(_)));
    assert!(mock.requests().is_empty());
}

#[test]
fn the_step_limit_closes_the_turn_without_tools() {
    let mut e = podcast_editor();
    let mock = Mock::start(vec![call("list_gaps", json!({})), call("list_fillers", json!({})), say("That is all I could do.")]);
    let mut a = Assistant::new(mock_client(&mock)).with_max_steps(2);
    let r = a.send(&mut e, "x", &mut |_| {}, &no_cancel()).unwrap();
    assert_eq!(r.reply, "That is all I could do.");
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3);
    assert!(reqs[1].body.get("tool_choice").is_none());
    assert_eq!(reqs[2].body["tool_choice"]["type"], "none");

    // A model that ignores it still cannot loop forever, and the conversation stays well-formed.
    let mock = Mock::start(vec![call("list_gaps", json!({})), call("list_gaps", json!({})), say("after")]);
    let mut a = Assistant::new(mock_client(&mock)).with_max_steps(1);
    let r = a.send(&mut e, "x", &mut |_| {}, &no_cancel()).unwrap();
    assert!(r.reply.contains("too many steps"));
    assert_eq!(mock.requests().len(), 2);
    a.send(&mut e, "next", &mut |_| {}, &no_cancel()).unwrap();
    let m = mock.requests()[2].body["messages"].as_array().unwrap().clone();
    let roles: Vec<&str> = m.iter().map(|x| x["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant", "user", "assistant", "user"]);
}

#[test]
fn thinking_blocks_are_echoed_back_unchanged() {
    let mut e = podcast_editor();
    let thinking = json!({"type": "thinking", "thinking": "", "signature": "c2lnbmF0dXJl"});
    let (r, _, mock) = run(&mut e, "x", vec![Reply::Sse(message(vec![thinking.clone(), tool_use("toolu_1", "list_gaps", json!({}))])), say("ok")]);
    r.unwrap();
    let echoed = &mock.requests()[1].body["messages"][1]["content"];
    assert_eq!(echoed[0], thinking);
    assert_eq!(echoed[1]["type"], "tool_use");
    assert_eq!(echoed[1]["input"], json!({}));
}

#[test]
fn a_rejected_thinking_signature_is_recovered_by_dropping_thinking_once() {
    let mut e = podcast_editor();
    let thinking = json!({"type": "thinking", "thinking": "", "signature": "abc"});
    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            Reply::Sse(message(vec![thinking, tool_use("toolu_1", "list_gaps", json!({}))])),
            Reply::Status(400, vec![], api_error("invalid_request_error", "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.")),
            say("ok"),
        ],
    );
    assert_eq!(r.unwrap().reply, "ok");
    let reqs = mock.requests();
    assert_eq!(reqs[1].body["messages"][1]["content"].as_array().unwrap().len(), 2);
    let retried = reqs[2].body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0]["type"], "tool_use");
}

#[test]
fn after_a_server_side_fallback_only_the_final_models_tool_calls_run() {
    let mut e = podcast_editor();
    let r0 = rev(&e);
    let original = e.project.clone();
    let content = vec![
        json!({"type": "thinking", "thinking": "", "signature": "old"}),
        text("Let me "),
        tool_use("toolu_dead", "edit_ranges", json!({"rev": r0, "action": "delete", "ranges": [{"start": 0, "end": 300}]})),
        json!({"type": "fallback", "from": {"model": "a"}, "to": {"model": "b"}}),
        text("check the pauses."),
        tool_use("toolu_live", "list_gaps", json!({})),
    ];
    let (r, _, mock) = run(&mut e, "x", vec![Reply::Sse(message(content)), say("Four pauses.")]);
    let r = r.unwrap();
    assert!(!r.changed, "the abandoned model's delete must not run");
    assert_eq!(e.project, original);
    assert_eq!(r.reply, "check the pauses.\n\nFour pauses.");
    let echoed = mock.requests()[1].body["messages"][1]["content"].as_array().unwrap().clone();
    let kinds: Vec<&str> = echoed.iter().map(|b| b["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["text", "fallback", "text", "tool_use"]);
    assert_eq!(tool_results(&mock.requests()[1]).len(), 1);
}

#[test]
fn convenience_tools_edit_the_project_and_one_turn_is_one_undo_step() {
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let source = e.active.unwrap();
    let r0 = rev(&e);
    let (ana_q, okay, berlin, wow) = (word_index(&e, "let's"), word_index(&e, "okay"), word_index(&e, "berlin"), word_index(&e, "wow"));
    let (r, _, mock) = run(
        &mut e,
        "do everything",
        vec![
            Reply::Sse(message(vec![
                tool_use("t1", "set_captions", json!({"color": "yellow", "max_lines": 1, "uppercase": true, "position": "top", "highlight": "active_word", "outline_color": "none"})),
                tool_use("t2", "set_canvas", json!({"aspect": "9:16"})),
                tool_use("t3", "add_text_layer", json!({"text": "Kitchen Table", "start": 0, "duration": 3, "position": "top", "color": "#FF0000", "animation": "fade"})),
                // Chapters: one given mid-sentence ("Berlin") is moved to the start of its sentence.
                tool_use("t4", "add_chapters", json!({"rev": r0, "chapters": [{"at": 1, "title": "Intro"}, {"at": ana_q, "title": "Sourdough"}, {"at": berlin, "title": "Marathon"}]})),
                tool_use("t5", "create_clips", json!({"rev": r0, "clips": [{"start": okay + 2, "end": wow - 3, "name": "Marathon story"}]})),
                tool_use("t6", "add_text_layer", json!({"text": "Running", "start_token": okay, "end_token": wow, "rev": r0})),
            ])),
            // Index-shifting edits last.
            Reply::Sse(message(vec![tool_use("t7", "remove_fillers", json!({})), tool_use("t8", "shorten_gaps", json!({"longer_than": 1.0, "target": 0.4}))])),
            say("All done."),
        ],
    );
    let r = r.unwrap();
    let results = tool_results(&mock.requests().last().unwrap().clone());
    assert!(results.iter().all(|(_, err)| !err), "{results:#?}");
    assert!(r.changed);
    assert_eq!(r.undo_steps, 1);
    assert_eq!(undo_depth(&mut e), depth + 1);

    assert_eq!(e.active, Some(source), "creating clips leaves the source composition active");
    let c = e.comp().unwrap();
    assert!(c.captions.enabled && c.captions.uppercase);
    assert_eq!((c.captions.max_lines, c.captions.highlight, c.captions.outline), (1, CaptionHighlight::ActiveWord, None));
    assert_eq!(c.captions.color, Rgba::rgb(0xFF, 0xE6, 0x00));
    assert!(c.captions.y < 0.3);
    assert_eq!((c.width, c.height), (1080, 1920));
    assert_eq!(c.script_video.fit, FitMode::Fill);

    assert_eq!(c.layers.len(), 2);
    let LayerContent::Text(title) = &c.layers[0].content else { panic!() };
    assert_eq!((title.text.as_str(), c.layers[0].start, c.layers[0].duration), ("Kitchen Table", 0.0, 3.0));
    assert_eq!((title.color, c.layers[0].animation_in), (Rgba::rgb(255, 0, 0), Animation::Fade));
    assert!(c.layers[1].track > c.layers[0].track);
    // The token-anchored layer got its times from the script as it was when the layer was added.
    let before = &original.compositions[0];
    assert!((c.layers[1].start - before.time_of_token(okay)).abs() < 1e-9);
    assert!((c.layers[1].end() - before.time_of_token(wow)).abs() < 1e-9);

    // Chapters sit on sentence starts and survived the later deletions.
    // (The intro marker was on "Um,", which was then removed: the editor carries a marker over to what follows.)
    let first_word_after = |i: usize| c.script[i..].iter().find(|t| t.shows_text()).unwrap().text.clone();
    let markers: Vec<(String, String, bool)> = c.script_markers().into_iter().map(|(i, t, n)| (n, first_word_after(i), t < 0.5 || c.script[i].is_word())).collect();
    assert_eq!(markers, [("Intro".to_string(), "hello".to_string(), true), ("Sourdough".into(), "Let's".into(), true), ("Marathon".into(), "I".into(), true)]);
    let chapters_out = &results[3].0;
    assert!(chapters_out.contains("moved from token"), "{chapters_out}");

    // The clip was widened to whole sentences and taken before the fillers were removed.
    assert_eq!(e.project.compositions.len(), 2);
    let clip = &e.project.compositions[1];
    assert_eq!(clip.name, "Marathon story");
    assert_eq!((clip.width, clip.height), (1080, 1920));
    let clip_text = clip.text_of(0..clip.script.len());
    assert!(clip_text.starts_with("Okay, let's switch gears") && clip_text.ends_with("during training."), "{clip_text}");

    // No fillers are left, even ones exposed by removing others ("you, uh, you" → "you, you").
    assert!(detect::find_fillers(c, &FillerOptions::default()).is_empty());
    assert!(!playing_text(&e).contains("you you") && !playing_text(&e).contains("in in"));
    assert!(detect::find_gaps(c, 1.0).iter().all(|(r, _)| r.end == c.script.len() || r.start == 0), "inner pauses were shortened");
    for w in ["sourdough", "marathon", "berlin", "starter", "welcome", "listening"] {
        assert!(playing_words(&e).contains(&w.to_string()), "{w} must survive");
    }

    assert!(e.undo());
    assert_eq!(e.project, original);
    assert_eq!(e.active, Some(source));
}

#[test]
fn read_and_list_tools_report_the_script_accurately() {
    let mut e = podcast_editor();
    let (r, _, mock) = run(
        &mut e,
        "x",
        vec![
            Reply::Sse(message(vec![
                tool_use("t1", "read_script", json!({"start": 58, "end": 70})),
                tool_use("t2", "search_script", json!({"query": "zebra"})),
                tool_use("t3", "search_script", json!({"query": "the Berlin marathon"})),
                tool_use("t4", "list_fillers", json!({})),
                tool_use("t5", "list_retakes", json!({})),
                tool_use("t6", "list_gaps", json!({"min_secs": 2.0})),
                tool_use("t7", "get_project_overview", json!({})),
            ])),
            say("ok"),
        ],
    );
    assert!(!r.unwrap().changed);
    let res = tool_results(&mock.requests()[1]);
    assert!(res.iter().all(|(_, err)| !err));
    let rev_ = rev(&e);
    assert!(res[0].0.starts_with(&format!("script rev {rev_} · tokens 0..342 (showing 58..70)")), "{}", res[0].0);
    assert!(res[0].0.contains("¶ 58..70 00:20 Ben: 58:Thanks, 60:Ana. 62:Uh, 64:it's 65:great 66:to 67:be 68:here."), "{}", res[0].0);
    assert!(res[1].0.contains("No matches for \"zebra\""), "{}", res[1].0);
    let berlin = word_index(&e, "berlin");
    assert!(res[2].0.contains(&format!("- {}..{} at 01:", berlin - 1, berlin + 2)) && res[2].0.contains("»the Berlin marathon«"), "{}", res[2].0);
    let n = detect::find_fillers(e.comp().unwrap(), &FillerOptions::default()).len();
    assert!(res[3].0.contains(&format!("{n} fillers: 9 um/uh, 3 phrases, 2 immediate repeats")), "{}", res[3].0);
    assert!(res[4].0.contains("\"today we have,\"") && res[4].0.contains("said again at"), "{}", res[4].0);
    assert!(res[5].0.contains("3 pauses of 2.0s or longer"), "{}", res[5].0);
    assert!(res[6].0.contains("ACTIVE COMPOSITION") && res[6].0.contains("SCRIPT (complete)"), "{}", res[6].0);
}

#[test]
fn restoring_works_for_both_ignored_and_deleted_text() {
    let mut e = podcast_editor();
    let before = playing_words(&e);
    let r0 = rev(&e);
    // Turn 1 ignores the intro paragraph and deletes Ben's greeting.
    let mock = Mock::start(vec![
        Reply::Sse(message(vec![tool_use("t1", "edit_ranges", json!({"rev": r0, "action": "ignore", "ranges": [{"start": 0, "end": 35}]}))])),
        Reply::Sse(message(vec![tool_use("t2", "edit_ranges", json!({"rev": r0, "action": "delete", "ranges": [{"start": 58, "end": 70}]}))])),
        say("Cut."),
    ]);
    let mut a = Assistant::new(mock_client(&mock));
    assert!(a.send(&mut e, "cut the intro and the greeting", &mut |_| {}, &no_cancel()).unwrap().changed);
    assert!(!playing_text(&e).contains("kitchen") && !playing_text(&e).contains("great to be here"));
    // The cut is visible to the model as ✂ on the word after it.
    let state = mango_ai::context::describe_project(&e);
    assert!(state.contains("✂ 58:Let's"), "{state}");
    assert!(state.contains("~~1:Um, 3:hello"), "{state}");

    // Turn 2 restores both: un-ignore, and restore the deleted media at the cut.
    let r1 = rev(&e);
    let mock = Mock::start(vec![
        Reply::Sse(message(vec![
            tool_use("t3", "edit_ranges", json!({"rev": r1, "action": "unignore", "ranges": [{"start": 0, "end": 35}]})),
            tool_use("t4", "apply_commands", json!({"rev": r1, "commands": [{"cmd": "restore_at", "at": 58}]})),
        ])),
        say("Restored."),
    ]);
    let mut a = Assistant::new(mock_client(&mock));
    assert!(a.send(&mut e, "keep them after all", &mut |_| {}, &no_cancel()).unwrap().changed);
    assert_eq!(playing_words(&e), before);
    assert_eq!(rev(&e), r0, "the script is structurally back to the original");
}

#[test]
fn tool_definitions_are_well_formed_and_stable() {
    let tools = mango_ai::assistant::tool_definitions();
    let list = tools.as_array().unwrap();
    let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for required in ["read_script", "search_script", "get_project_overview", "list_fillers", "list_retakes", "list_gaps", "apply_commands", "remove_fillers", "shorten_gaps", "set_captions", "add_text_layer", "set_canvas", "add_chapters", "create_clips", "set_transitions", "edit_ranges"] {
        assert!(names.contains(&required), "missing tool {required}");
    }
    for t in list {
        assert!(t["description"].as_str().unwrap().len() > 40, "{}", t["name"]);
        assert_eq!(t["input_schema"]["type"], "object", "{}", t["name"]);
        assert_eq!(t["input_schema"]["additionalProperties"], false, "{}", t["name"]);
        for req in t["input_schema"]["required"].as_array().into_iter().flatten() {
            assert!(t["input_schema"]["properties"].get(req.as_str().unwrap()).is_some(), "{} requires unknown {req}", t["name"]);
        }
    }
    assert_eq!(tools.to_string(), mango_ai::assistant::tool_definitions().to_string(), "the tool list must be deterministic (it is cached)");
    let p = mango_ai::assistant::system_prompt();
    for must in ["end is exclusive", "rev", "ignore", "Never invent", "one or two plain sentences"] {
        assert!(p.contains(must), "system prompt lost: {must}");
    }
}
