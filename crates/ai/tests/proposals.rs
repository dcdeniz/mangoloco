//! Offline tests of the one-shot tools: every validator that stands between
//! model output and the project, and the tools end to end against a mock.
mod common;

use common::*;
use mango_ai::tools::{self, Chapter, ClarityEdit, ClipProposal, Intensity, Platform};
use mango_ai::{AiError, context};
use mango_core::*;
use serde_json::{Value, json};

fn answer(v: Value) -> Reply {
    Reply::Sse(message(vec![text(&v.to_string())]))
}

fn say(s: &str) -> Reply {
    Reply::Sse(message(vec![text(s)]))
}

fn clip(start: usize, end: usize, title: &str) -> ClipProposal {
    ClipProposal { start, end, title: title.into(), reason: "r".into(), duration: 0.0 }
}

/// The sentence number (1-based) of the sentence containing word `w`.
fn sentence_no(e: &Editor, w: &str) -> usize {
    let i = word_index(e, w);
    context::sentences(e.comp().unwrap()).iter().position(|s| s.start <= i && i < s.end).unwrap() + 1
}

// --- validators -------------------------------------------------------------

#[test]
fn chapters_are_snapped_to_sentence_starts_and_bad_ones_dropped() {
    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let sentences = context::sentences(comp);
    let (lets, berlin, okay) = (word_index(&e, "let's"), word_index(&e, "berlin"), word_index(&e, "okay"));
    let raw = vec![
        (berlin, "Marathon".to_string()),         // mid-sentence: snapped back to "I ran the Berlin marathon…"
        (99_999, "Beyond the end".to_string()),   // not a token: dropped
        (comp.script.len(), "At the end".into()), // one past the last token: dropped
        (lets, "  Sourdough \n basics ".into()),  // title whitespace is cleaned
        (lets + 2, "Duplicate of sourdough".into()), // same sentence as the previous: dropped
        (okay, "".into()),                        // empty title: dropped
        (2, "Intro".into()),                      // a silence token before the first word of its sentence… snapped to sentence 1
        (word_index(&e, "ana"), "Too close".into()), // second sentence, under 10 s after the first chapter: dropped
    ];
    let chapters = tools::validate_chapters(comp, raw);
    let got: Vec<(&str, &str)> = chapters.iter().map(|c| (c.title.as_str(), comp.script[c.token].text.as_str())).collect();
    assert_eq!(got, [("Intro", "Um,"), ("Sourdough basics", "Let's"), ("Marathon", "I")]);
    for c in &chapters {
        assert!(sentences.iter().any(|s| s.start == c.token), "{c:?} is not at a sentence start");
        assert_eq!(c.time, comp.time_of_token(c.token));
    }
    assert!(chapters.windows(2).all(|w| w[0].token < w[1].token && w[1].time - w[0].time >= tools::MIN_CHAPTER_SPACING));
    assert!(tools::validate_chapters(comp, vec![(99_999, "x".into())]).is_empty());
    assert!(tools::validate_chapters(Editor::new_project("e").comp().unwrap(), vec![(0, "x".into())]).is_empty());

    // Applying them sets markers without shifting indices.
    let mut e2 = podcast_editor();
    let chapters: Vec<Chapter> = tools::validate_chapters(e2.comp().unwrap(), vec![(0, "Intro".into()), (word_index(&e2, "okay"), "Running".into())]);
    let rev = context::script_rev(e2.comp().unwrap());
    e2.apply_all("Chapters", tools::apply_chapters(&chapters)).unwrap();
    let c2 = e2.comp().unwrap();
    assert_eq!(context::script_rev(c2), rev);
    let names: Vec<String> = c2.script_markers().into_iter().map(|m| m.2).collect();
    assert_eq!(names, ["Intro", "Running"]);
    assert_eq!(transcript_export::chapters(c2).len(), 2);
}

#[test]
fn clips_out_of_bounds_overlapping_or_outside_the_duration_are_rejected() {
    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let len = comp.script.len();
    let sentences = context::sentences(comp);
    let (lets, okay, wow, berlin) = (word_index(&e, "let's"), word_index(&e, "okay"), word_index(&e, "wow"), word_index(&e, "berlin"));
    let raw = vec![
        clip(berlin, berlin + 30, "Berlin"),       // mid-sentence on both sides: widened to whole sentences
        clip(okay, wow, "Overlaps Berlin"),        // overlaps the first: rejected
        clip(lets, len + 10, "Past the end"),      // out of bounds: rejected
        clip(50, 40, "Backwards"),                 // rejected
        clip(lets, lets + 3, "Too short"),         // one short sentence: under min
        clip(0, len, "Everything"),                // over max
        clip(lets, lets + 60, "  Sourdough   start "), // fine
        clip(len - 20, len, "Outro"),              // valid, but two clips were already accepted
    ];
    let (clips, rejected) = tools::validate_clips(comp, raw, 2, 6.0, 40.0);
    assert_eq!(clips.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Berlin", "Sourdough start"]);
    assert_eq!(rejected.len(), 5, "{rejected:#?}");
    assert!(rejected[0].contains("overlaps another clip"));
    assert!(rejected[1].contains("not inside the script") && rejected[2].contains("not inside the script"));
    assert!(rejected[3].contains("outside 6–40s") && rejected[4].contains("outside 6–40s"));
    for c in &clips {
        assert!(c.end <= len && c.start < c.end);
        assert!(sentences.iter().any(|s| s.start == c.start), "starts on a sentence");
        assert!(sentences.iter().any(|s| s.end == c.end), "ends on a sentence");
        assert!(script::ends_sentence(&comp.script[c.end - 1].text));
        assert!((c.duration - comp.range_duration(c.start..c.end)).abs() < 1e-9);
        assert!((6.0..=40.0).contains(&c.duration));
    }
    assert_eq!(comp.script[clips[0].start].text, "I");
    assert!(clips[0].start < berlin && clips[0].end > berlin + 20 && comp.script[clips[0].end - 1].text == "peak.");
    assert!(clips[0].end <= clips[1].start || clips[1].end <= clips[0].start);

    // Applying creates one composition per clip and leaves the source active and untouched.
    let mut e2 = podcast_editor();
    let comp2 = e2.comp().unwrap();
    let (clips, _) = tools::validate_clips(comp2, vec![clip(word_index(&e2, "okay"), word_index(&e2, "wow"), "Running"), clip(0, 30, "Hello")], 5, 3.0, 60.0);
    assert_eq!(clips.len(), 2);
    let source = e2.active.unwrap();
    let before = comp2.script.clone();
    let depth = undo_depth(&mut e2);
    e2.apply_all("Create clips", tools::apply_clips(source, &clips)).unwrap();
    assert_eq!(e2.active, Some(source));
    assert_eq!(e2.comp().unwrap().script, before);
    assert_eq!(e2.project.compositions.len(), 3);
    for (c, made) in clips.iter().zip(&e2.project.compositions[1..]) {
        assert_eq!(made.name, c.title);
        assert!((made.script_duration() - c.duration).abs() < 1e-6);
    }
    assert!(e2.project.compositions[1].text_of(0..999).starts_with("Okay, let's switch gears"));
    assert_eq!(undo_depth(&mut e2), depth + 1, "accepting clips is one undo step");
}

#[test]
fn clarity_edits_are_bounded_trimmed_and_merged() {
    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let len = comp.script.len();
    let edit = |s: usize, e: usize, r: &str| ClarityEdit { start: s, end: e, reason: r.into() };
    let raw = vec![
        edit(21, 25, "false start"),   // "today we have," plus the silence after it: trailing silence trimmed
        edit(23, 24, "same thing"),    // inside the previous: merged
        edit(len - 1, len + 5, "x"),   // out of bounds: dropped
        edit(40, 40, "empty"),         // dropped
        edit(34, 35, "just a pause"),  // no words: dropped
        edit(105, 110, "stutter"),     // "and I, I mean,"
        edit(110, 111, "adjacent"),    // touching the previous: merged
        edit(1, 2, "um"),
    ];
    let edits = tools::validate_clarity_edits(comp, raw);
    assert_eq!(edits.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>(), [(1, 2), (21, 24), (105, 110)]);
    assert_eq!(edits[1].reason, "false start; same thing");
    for e_ in &edits {
        assert!(comp.script[e_.start].is_word() && comp.script[e_.end - 1].is_word());
    }
    // Applying strikes the text through (restorable), without shifting indices.
    let mut e2 = podcast_editor();
    let rev = context::script_rev(e2.comp().unwrap());
    let words_before = playing_words(&e2).len();
    e2.apply_all("Edit for clarity", tools::apply_clarity_edits(&edits)).unwrap();
    assert_eq!(context::script_rev(e2.comp().unwrap()), rev);
    assert_eq!(playing_words(&e2).len(), words_before - 1 - 3 - 4);
    assert!(playing_text(&e2).contains("so uh today we have a really fun episode"));
    assert!(e2.comp().unwrap().script.iter().filter(|t| t.ignored).all(|t| t.is_word() || t.kind == TokenKind::Gap));
    // Ignored text is not offered again.
    assert!(tools::validate_clarity_edits(e2.comp().unwrap(), vec![edit(21, 24, "again")]).is_empty());
}

#[test]
fn titles_and_speaker_turns_are_cleaned() {
    let t = tools::validate_titles(vec!["1. \"Bread and Miles\"".into(), "  ".into(), "bread AND miles".into(), "- From Starter\nto Finish Line".into(), "Third".into(), "Fourth".into()], 3);
    assert_eq!(t, ["Bread and Miles", "From Starter to Finish Line", "Third"]);

    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let (thanks, lets, berlin) = (word_index(&e, "thanks"), word_index(&e, "let's"), word_index(&e, "berlin"));
    let turns = tools::validate_speaker_turns(
        comp,
        vec![(berlin, "Ben:".into()), (5, "Ana".into()), (thanks, " Ben ".into()), (99_999, "Ghost".into()), (lets, "Ana".into()), (lets + 20, "".into()), (lets + 21, "".into())],
    );
    let i_ran = context::snap_to_sentence_start(comp, &context::sentences(comp), berlin).unwrap();
    // First turn moved to the very first sentence; mid-sentence starts snapped; the two unnamed turns in a row merged.
    assert_eq!(turns[0], (context::sentences(comp)[0].start, "Ana".to_string()));
    assert_eq!(turns[1], (thanks, "Ben".to_string()));
    assert_eq!(turns[2], (lets, "Ana".to_string()));
    assert_eq!(turns[3].1, "Speaker");
    assert_eq!(turns.last().unwrap(), &(i_ran, "Ben".to_string()));
    assert_eq!(turns.len(), 5);

    // Applying labels each turn up to the next one.
    let unlabelled: Vec<(&str, &str)> = PODCAST.iter().map(|(_, t)| ("", *t)).collect();
    let mut e2 = editor_from_turns(&unlabelled);
    let len = e2.comp().unwrap().script.len();
    let t0 = context::sentences(e2.comp().unwrap())[0].start;
    let thanks2 = word_index(&e2, "thanks");
    let lets2 = word_index(&e2, "let's");
    let cmds = tools::apply_speaker_labels(len, &[(t0, "Ana".into()), (thanks2, "Ben".into()), (lets2, "Ana".into())]);
    e2.apply_all("Label speakers", cmds).unwrap();
    let c = e2.comp().unwrap();
    let name = |i: usize| e2.project.speaker_name(c.script[i].speaker).to_string();
    assert_eq!((name(t0 + 3), name(thanks2), name(thanks2 + 5), name(lets2), name(len - 2)), ("Ana".into(), "Ben".into(), "Ben".into(), "Ana".into(), "Ana".into()));
    assert!(c.script[thanks2].paragraph_start && c.script[lets2].paragraph_start);
}

// --- tools against the mock ---------------------------------------------------

#[test]
fn translate_captions_returns_one_translation_per_cue_in_order() {
    let cues: Vec<String> = vec!["Hello and welcome".into(), "".into(), "to the show".into(), "My guest is Ben".into()];
    // Out of order, with a stray id and a duplicate: still mapped by id.
    let mock = Mock::start(vec![answer(json!({"translations": [
        {"id": 4, "text": "Mi invitado es Ben"}, {"id": 1, "text": " Hola y bienvenidos "}, {"id": 9, "text": "sobra"}, {"id": 3, "text": "al programa"}, {"id": 3, "text": "duplicado"}
    ]}))]);
    let client = mock_client(&mock).with_fast_model("fast-model");
    let out = tools::translate_captions(&client, &cues, "Spanish").unwrap();
    assert_eq!(out, ["Hola y bienvenidos", "", "al programa", "Mi invitado es Ben"]);
    let req = &mock.requests()[0];
    assert_eq!(req.body["model"], "fast-model");
    assert_eq!(req.body["output_config"]["format"]["type"], "json_schema");
    let prompt = req.body["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("Translate these 3 cues into Spanish") && !prompt.contains("\"id\": 2"), "empty cues are not sent: {prompt}");
    assert!(req.body["system"][0]["text"].as_str().unwrap().contains("exactly one translation per cue"));
}

#[test]
fn translate_captions_length_mismatch_is_retried_then_a_clear_error() {
    let cues: Vec<String> = (1..=5).map(|i| format!("line {i}")).collect();
    // First answer merges cues (3 for 5); the retry supplies the missing ones.
    let mock = Mock::start(vec![
        answer(json!({"translations": [{"id": 1, "text": "línea 1"}, {"id": 2, "text": "línea 2 y 3"}, {"id": 5, "text": "línea 5"}]})),
        answer(json!({"translations": [{"id": 3, "text": "línea 3"}, {"id": 4, "text": "línea 4"}, {"id": 1, "text": "otra"}]})),
    ]);
    let out = tools::translate_captions(&mock_client(&mock), &cues, "Spanish").unwrap();
    assert_eq!(out, ["línea 1", "línea 2 y 3", "línea 3", "línea 4", "línea 5"]);
    assert_eq!(out.len(), cues.len());
    let retry = &mock.requests()[1].body["messages"];
    assert_eq!(retry.as_array().unwrap().len(), 3);
    assert!(retry[2]["content"].as_str().unwrap().contains("missing for the cues with id 3, 4"), "{}", retry[2]["content"]);

    // Still short after the retry: an error, never a silently misaligned list.
    let short = || answer(json!({"translations": [{"id": 1, "text": "uno"}, {"id": 2, "text": ""}]}));
    let mock = Mock::start(vec![short(), short()]);
    let err = tools::translate_captions(&mock_client(&mock), &cues, "Spanish").unwrap_err();
    assert_eq!(err, AiError::InvalidOutput("the translation came back with 2 cues for 5 (cue 2 is missing); nothing was changed".into()));
    assert_eq!(mock.requests().len(), 2);

    // Not JSON at all.
    let mock = Mock::start(vec![say("Here are your translations!")]);
    assert!(matches!(tools::translate_captions(&mock_client(&mock), &cues, "Spanish").unwrap_err(), AiError::InvalidOutput(_)));
    // Nothing to translate: no request.
    let mock = Mock::start(vec![]);
    assert_eq!(tools::translate_captions(&mock_client(&mock), &["".into(), "  ".into()], "Spanish").unwrap(), ["", ""]);
    assert!(tools::translate_captions(&mock_client(&mock), &[], "Spanish").unwrap().is_empty());
    assert!(matches!(tools::translate_captions(&mock_client(&mock), &cues, " ").unwrap_err(), AiError::NothingToDo(_)));
    assert!(mock.requests().is_empty());
}

#[test]
fn translate_captions_batches_long_lists() {
    let cues: Vec<String> = (1..=170).map(|i| format!("cue {i}")).collect();
    let batch = |from: usize, to: usize| answer(json!({"translations": (from..=to).map(|i| json!({"id": i, "text": format!("señal {i}")})).collect::<Vec<_>>()}));
    let mock = Mock::start(vec![batch(1, 80), batch(81, 160), batch(161, 170)]);
    let out = tools::translate_captions(&mock_client(&mock), &cues, "Spanish").unwrap();
    assert_eq!(out.len(), 170);
    assert!(out.iter().enumerate().all(|(i, t)| *t == format!("señal {}", i + 1)));
    assert_eq!(mock.requests().len(), 3);
}

#[test]
fn chapters_from_the_model_are_validated_against_the_script() {
    let e = podcast_editor();
    let n = context::sentences(e.comp().unwrap()).len();
    let mock = Mock::start(vec![answer(json!({"chapters": [
        {"sentence": sentence_no(&e, "okay"), "title": "Marathon training"},
        {"sentence": 0, "title": "Sentence zero does not exist"},
        {"sentence": n + 7, "title": "Past the end"},
        {"sentence": -3, "title": "Negative"},
        {"sentence": 1, "title": "Welcome"},
        {"sentence": sentence_no(&e, "let's"), "title": "Sourdough"},
        {"sentence": sentence_no(&e, "let's"), "title": "Sourdough again"},
    ]}))]);
    let chapters = tools::chapters(&mock_client(&mock), &e).unwrap();
    let comp = e.comp().unwrap();
    assert_eq!(chapters.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Welcome", "Sourdough", "Marathon training"]);
    assert_eq!(chapters.iter().map(|c| comp.script[c.token].text.as_str()).collect::<Vec<_>>(), ["Um,", "Let's", "Okay,"]);
    assert!(chapters.windows(2).all(|w| w[0].time < w[1].time));
    // The transcript goes first with a cache breakpoint; the task follows.
    let content = &mock.requests()[0].body["messages"][0]["content"];
    assert!(content[0]["text"].as_str().unwrap().contains("S1 [00:00.4] Ana: Um, hello and welcome"));
    assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
    assert!(content[1]["text"].as_str().unwrap().starts_with("Task: divide this recording into chapters"));

    // Nothing usable is an error, not an empty success.
    let mock = Mock::start(vec![answer(json!({"chapters": [{"sentence": 9999, "title": "x"}]}))]);
    assert!(matches!(tools::chapters(&mock_client(&mock), &e).unwrap_err(), AiError::InvalidOutput(_)));
}

#[test]
fn find_clips_validates_and_asks_once_for_replacements() {
    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let n = context::sentences(comp).len();
    let (bread_a, bread_b) = (sentence_no(&e, "started"), sentence_no(&e, "learn"));
    let (run_a, run_b) = (sentence_no(&e, "okay"), sentence_no(&e, "peak"));
    let mock = Mock::start(vec![
        answer(json!({"clips": [
            {"first_sentence": bread_a, "last_sentence": bread_b, "title": "How I started baking", "reason": "A complete story."},
            {"first_sentence": bread_a + 1, "last_sentence": bread_b + 1, "title": "Overlapping", "reason": "x"},
            {"first_sentence": 1, "last_sentence": n, "title": "The whole thing", "reason": "x"},
            {"first_sentence": n + 1, "last_sentence": n + 4, "title": "Not in the script", "reason": "x"},
        ]})),
        answer(json!({"clips": [
            {"first_sentence": bread_a, "last_sentence": bread_b, "title": "How I started baking", "reason": "A complete story."},
            {"first_sentence": run_a, "last_sentence": run_b, "title": "Training for Berlin", "reason": "Stands alone."},
        ]})),
    ]);
    let clips = tools::find_clips(&mock_client(&mock), &e, 2, 10.0, 30.0, Some("running or baking")).unwrap();
    assert_eq!(clips.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["How I started baking", "Training for Berlin"]);
    for c in &clips {
        assert!((10.0..=30.0).contains(&c.duration), "{c:?}");
        assert!(script::ends_sentence(&comp.script[c.end - 1].text));
    }
    assert!(clips[0].end <= clips[1].start);
    let reqs = mock.requests();
    assert!(reqs[0].body["messages"][0]["content"][1]["text"].as_str().unwrap().contains("Only pick passages about this topic: running or baking"));
    let feedback = reqs[1].body["messages"][2]["content"].as_str().unwrap();
    assert!(feedback.contains("overlaps another clip") && feedback.contains("outside 10–30s") && feedback.contains("sentence numbers that do not exist"), "{feedback}");

    // If the second answer is no better, only the valid clips are returned.
    let bad = || answer(json!({"clips": [{"first_sentence": 1, "last_sentence": n, "title": "Everything", "reason": "x"}]}));
    let mock = Mock::start(vec![bad(), bad()]);
    assert!(tools::find_clips(&mock_client(&mock), &e, 3, 10.0, 30.0, None).unwrap().is_empty());
    // Impossible requests never reach the API.
    let mock = Mock::start(vec![]);
    assert!(matches!(tools::find_clips(&mock_client(&mock), &e, 3, 600.0, 900.0, None).unwrap_err(), AiError::NothingToDo(_)));
    assert!(matches!(tools::find_clips(&mock_client(&mock), &e, 3, 30.0, 10.0, None).unwrap_err(), AiError::NothingToDo(_)));
    assert!(tools::find_clips(&mock_client(&mock), &e, 0, 10.0, 30.0, None).unwrap().is_empty());
    assert!(mock.requests().is_empty());
}

#[test]
fn edit_for_clarity_validates_ranges_and_shows_the_detector_hints() {
    let e = podcast_editor();
    let len = e.comp().unwrap().script.len();
    let mock = Mock::start(vec![answer(json!({"edits": [
        {"start": 21, "end": 25, "reason": "false start"},
        {"start": len - 3, "end": len + 9, "reason": "out of range"},
        {"start": -5, "end": 3, "reason": "negative"},
        {"start": 50, "end": 45, "reason": "backwards"},
    ]}))]);
    let edits = tools::edit_for_clarity(&mock_client(&mock), &e, Intensity::Low).unwrap();
    assert_eq!(edits, [ClarityEdit { start: 21, end: 24, reason: "false start".into() }]);
    let req = &mock.requests()[0];
    let prompt = req.body["messages"][0]["content"].as_str().unwrap();
    assert!(prompt.contains("21:today 22:we 23:have, 25:today 26:we 27:have"), "{prompt}");
    assert!(prompt.contains("- 21..25 \"today we have,\" is said again right after, at 25..28"), "{prompt}");
    assert!(req.body["system"][0]["text"].as_str().unwrap().contains("Intensity: LOW"));

    // A proposal that guts the recording is refused.
    let mock = Mock::start(vec![answer(json!({"edits": [{"start": 0, "end": len - 10, "reason": "all of it"}]}))]);
    assert!(matches!(tools::edit_for_clarity(&mock_client(&mock), &e, Intensity::Heavy).unwrap_err(), AiError::InvalidOutput(_)));
    // An empty proposal is a valid answer.
    let mock = Mock::start(vec![answer(json!({"edits": []}))]);
    assert!(tools::edit_for_clarity(&mock_client(&mock), &e, Intensity::Medium).unwrap().is_empty());
}

#[test]
fn titles_count_is_enforced_with_one_retry() {
    let e = podcast_editor();
    let mock = Mock::start(vec![answer(json!({"titles": ["Bread", "bread", ""]})), answer(json!({"titles": ["Bread", "Miles", "Starter", "Extra"]}))]);
    assert_eq!(tools::titles(&mock_client(&mock), &e, 3).unwrap(), ["Bread", "Miles", "Starter"]);
    assert!(mock.requests()[1].body["messages"][2]["content"].as_str().unwrap().contains("exactly 3"));

    let mock = Mock::start(vec![answer(json!({"titles": ["One", "Two", "Three", "Four"]}))]);
    assert_eq!(tools::titles(&mock_client(&mock), &e, 2).unwrap(), ["One", "Two"]);

    let mock = Mock::start(vec![answer(json!({"titles": ["Only"]})), answer(json!({"titles": ["Only"]}))]);
    assert!(matches!(tools::titles(&mock_client(&mock), &e, 4).unwrap_err(), AiError::InvalidOutput(_)));
    assert!(tools::titles(&mock_client(&Mock::start(vec![])), &e, 0).unwrap().is_empty());
}

#[test]
fn youtube_description_always_carries_the_exact_chapter_list() {
    let mut e = podcast_editor();
    let okay = word_index(&e, "okay");
    e.apply(Command::SetMarker { at: 1, name: Some("Welcome".into()) }).unwrap();
    e.apply(Command::SetMarker { at: okay, name: Some("Running".into()) }).unwrap();
    let chapters = transcript_export::chapters_text(e.comp().unwrap());
    assert_eq!(chapters.lines().count(), 2);

    // The model forgot the list: it is appended.
    let mock = Mock::start(vec![say("A chat about bread and running.\n\n#baking #running")]);
    let out = tools::youtube_description(&mock_client(&mock), &e).unwrap();
    assert!(out.ends_with(&format!("Chapters:\n{chapters}")), "{out}");
    assert!(mock.requests()[0].body["messages"][0]["content"][1]["text"].as_str().unwrap().contains(&chapters));
    // The model included it: nothing is added.
    let full = format!("Hook.\n\nChapters:\n{chapters}\n\n#baking");
    let mock = Mock::start(vec![say(&full)]);
    assert_eq!(tools::youtube_description(&mock_client(&mock), &e).unwrap(), full);
    // No markers: the model is told not to make up timestamps.
    let plain = podcast_editor();
    let mock = Mock::start(vec![say("Hook.")]);
    assert_eq!(tools::youtube_description(&mock_client(&mock), &plain).unwrap(), "Hook.");
    assert!(mock.requests()[0].body["messages"][0]["content"][1]["text"].as_str().unwrap().contains("Do not include a chapter list"));
}

#[test]
fn social_posts_respect_platform_limits() {
    let e = podcast_editor();
    let long = "x".repeat(300);
    let mock = Mock::start(vec![say(&long), say("Short and sweet. #bread")]);
    assert_eq!(tools::social_post(&mock_client(&mock), &e, Platform::X).unwrap(), "Short and sweet. #bread");
    assert!(mock.requests()[1].body["messages"][2]["content"].as_str().unwrap().contains("That is 300 characters; the limit on X is 280"));

    let mock = Mock::start(vec![say(&long), say(&long)]);
    assert!(matches!(tools::social_post(&mock_client(&mock), &e, Platform::X).unwrap_err(), AiError::InvalidOutput(_)));
    // The same text is fine where the limit is higher.
    let mock = Mock::start(vec![say(&long)]);
    assert_eq!(tools::social_post(&mock_client(&mock), &e, Platform::LinkedIn).unwrap(), long);
    for p in Platform::ALL {
        assert!(p.max_chars() >= 280 && !p.label().is_empty());
    }
}

#[test]
fn text_tools_send_the_transcript_and_return_trimmed_text() {
    let e = podcast_editor();
    let mock = Mock::start(vec![say("\n A summary. \n"), say("## Summary\nx"), say("# Post"), say("Hola"), say("Tighter."), say("A script."), say("- outline"), say("- idea")]);
    let c = mock_client(&mock).with_model("main-model");
    assert_eq!(tools::summarize(&c, &e).unwrap(), "A summary.");
    assert_eq!(tools::show_notes(&c, &e).unwrap(), "## Summary\nx");
    assert_eq!(tools::blog_post(&c, &e).unwrap(), "# Post");
    assert_eq!(tools::translate_text(&c, "Hello", "Spanish").unwrap(), "Hola");
    assert_eq!(tools::rewrite(&c, "Some text that is long.", "tighter").unwrap(), "Tighter.");
    assert_eq!(tools::write_script(&c, "intro for a baking show").unwrap(), "A script.");
    assert_eq!(tools::outline(&c, "episode on sourdough").unwrap(), "- outline");
    assert_eq!(tools::brainstorm(&c, "episode ideas").unwrap(), "- idea");
    let reqs = mock.requests();
    // The three transcript tools share one byte-identical cached prefix (system + transcript block).
    for r in &reqs[..3] {
        assert_eq!(r.body["model"], "main-model");
        assert_eq!(r.body["system"], reqs[0].body["system"]);
        assert_eq!(r.body["messages"][0]["content"][0], reqs[0].body["messages"][0]["content"][0]);
        assert!(r.body.get("cache_control").is_none(), "no automatic breakpoint after the one-off task text");
    }
    let transcript = reqs[0].body["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(transcript.contains("Berlin marathon") && transcript.contains("sentences=\""));
    assert!(reqs[5].body["messages"][0]["content"].as_str().unwrap().contains("Brief: intro for a baking show"));

    // Ignored text is not sent to the model.
    let mut cut = podcast_editor();
    let (a, b) = (word_index(&cut, "okay"), word_index(&cut, "amazing"));
    cut.apply(Command::IgnoreRange { start: a, end: b, ignored: true }).unwrap();
    let mock = Mock::start(vec![say("s")]);
    tools::summarize(&mock_client(&mock), &cut).unwrap();
    let t = mock.requests()[0].body["messages"][0]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(!t.contains("Berlin") && t.contains("sourdough"));
}

#[test]
fn tools_refuse_to_run_without_anything_to_work_on() {
    let empty = Editor::new_project("Empty");
    let mock = Mock::start(vec![]);
    let c = mock_client(&mock);
    let nothing = |r: Result<(), AiError>| assert!(matches!(r, Err(AiError::NothingToDo(_))), "{r:?}");
    nothing(tools::summarize(&c, &empty).map(drop));
    nothing(tools::show_notes(&c, &empty).map(drop));
    nothing(tools::blog_post(&c, &empty).map(drop));
    nothing(tools::youtube_description(&c, &empty).map(drop));
    nothing(tools::social_post(&c, &empty, Platform::Threads).map(drop));
    nothing(tools::titles(&c, &empty, 3).map(drop));
    nothing(tools::chapters(&c, &empty).map(drop));
    nothing(tools::find_clips(&c, &empty, 2, 5.0, 30.0, None).map(drop));
    nothing(tools::edit_for_clarity(&c, &empty, Intensity::Medium).map(drop));
    nothing(tools::label_speakers(&c, &empty).map(drop));
    nothing(tools::rewrite(&c, "  ", "x").map(drop));
    nothing(tools::write_script(&c, "").map(drop));
    nothing(tools::outline(&c, " ").map(drop));
    nothing(tools::brainstorm(&c, "").map(drop));
    assert_eq!(tools::translate_text(&c, "", "Spanish").unwrap(), "");
    assert!(mock.requests().is_empty(), "none of these may spend tokens");
}

#[test]
fn label_speakers_maps_sentences_to_turn_starts() {
    let unlabelled: Vec<(&str, &str)> = PODCAST.iter().map(|(_, t)| ("", *t)).collect();
    let e = editor_from_turns(&unlabelled);
    let comp = e.comp().unwrap();
    let n = context::sentences(comp).len();
    let (thanks, lets) = (sentence_no(&e, "thanks"), sentence_no(&e, "let's"));
    let mock = Mock::start(vec![answer(json!({"turns": [
        {"first_sentence": 1, "speaker": "Ana"},
        {"first_sentence": thanks, "speaker": "Ben"},
        {"first_sentence": thanks + 1, "speaker": "Ben"},
        {"first_sentence": lets, "speaker": "Ana"},
        {"first_sentence": n + 3, "speaker": "Nobody"},
    ]}))]);
    let turns = tools::label_speakers(&mock_client(&mock), &e).unwrap();
    assert_eq!(turns.iter().map(|t| (comp.script[t.0].text.as_str(), t.1.as_str())).collect::<Vec<_>>(), [("Um,", "Ana"), ("Thanks,", "Ben"), ("Let's", "Ana")]);
    // The model is not shown any speaker names.
    let prompt = mock.requests()[0].body["messages"][0]["content"].as_str().unwrap().to_string();
    assert!(prompt.contains("S1 [00:00.4] Um, hello") && !prompt.contains("Ana:"), "{}", &prompt[..200]);
}
