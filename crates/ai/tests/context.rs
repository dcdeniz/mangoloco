//! Offline tests of what the model is shown: the indexed script format,
//! revisions, sentences, and windowing of long scripts.
mod common;

use common::*;
use mango_ai::context::{self, describe_project, describe_script, script_rev};
use mango_core::*;

/// Every `N:word` pair in a rendering.
fn indexed_words(rendered: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in rendered.lines().filter(|l| l.starts_with('¶')) {
        let body = line.split_once(": ").map(|x| x.1).unwrap_or("");
        for piece in body.split(' ') {
            let piece = piece.trim_start_matches("~~").trim_end_matches("~~").trim_end_matches('}');
            if let Some((n, w)) = piece.split_once(':')
                && let Ok(n) = n.parse::<usize>()
                    && !w.is_empty() {
                        out.push((n, w.to_string()));
                    }
        }
    }
    out
}

#[test]
fn every_index_in_the_rendering_points_at_that_word() {
    let e = podcast_editor();
    let comp = e.comp().unwrap();
    let text = describe_script(comp, &e.project, 0..comp.script.len());
    let pairs = indexed_words(&text);
    let all_words: Vec<(usize, String)> = comp.script.iter().enumerate().filter(|(_, t)| t.is_word()).map(|(i, t)| (i, t.text.clone())).collect();
    assert_eq!(pairs, all_words, "the rendering lists every word exactly once with its token index");
    assert!(pairs.len() > 250);

    // Paragraph headers carry real ranges, times and speakers.
    let pars = comp.paragraphs();
    let headers: Vec<&str> = text.lines().filter(|l| l.starts_with('¶')).collect();
    assert_eq!(headers.len(), pars.len());
    for (h, p) in headers.iter().zip(&pars) {
        let speaker = e.project.speaker_name(comp.script[p.start].speaker);
        let expect = format!("¶ {}..{} {} {}:", p.start, p.end, transcript_export::timecode(comp.time_of_token(p.start)), speaker);
        assert!(h.starts_with(&expect), "{h} vs {expect}");
    }
    assert!(text.starts_with(&format!("script rev {} · tokens 0..{} (showing 0..{})", script_rev(comp), comp.script.len(), comp.script.len())));

    // A sub-range shows exactly the words inside it, clipped mid-paragraph.
    let part = describe_script(comp, &e.project, 40..100);
    let got: Vec<usize> = indexed_words(&part).into_iter().map(|p| p.0).collect();
    let want: Vec<usize> = all_words.iter().map(|p| p.0).filter(|i| (40..100).contains(i)).collect();
    assert_eq!(got, want);
    assert!(part.contains("¶ 40..58 "), "{part}");
    // Out-of-range requests are clamped, not a panic.
    assert!(describe_script(comp, &e.project, 5000..9000).contains("nothing in this range"));
    assert!(describe_script(comp, &e.project, 300..9000).contains("showing 300..342"));
}

#[test]
fn ignored_hidden_pauses_cuts_markers_scenes_and_notes_are_marked() {
    let mut e = podcast_editor();
    let (hello, podcast, okay) = (word_index(&e, "hello"), word_index(&e, "podcast"), word_index(&e, "okay"));
    e.apply(Command::IgnoreRange { start: hello, end: podcast + 1, ignored: true }).unwrap();
    e.apply(Command::HideText { start: 14, end: 15, hidden: true }).unwrap();
    e.apply(Command::SetMarker { at: okay, name: Some("Running".into()) }).unwrap();
    e.apply(Command::SetSceneBreak { at: okay, on: true }).unwrap();
    e.apply(Command::SetNote { at: okay + 2, note: Some("laughs".into()) }).unwrap();
    let comp = e.comp().unwrap();
    let text = describe_script(comp, &e.project, 0..comp.script.len());
    assert!(text.contains("1:Um, ~~3:hello 4:and 5:welcome 6:back 7:to 8:the 9:the 10:Kitchen 11:Table 12:podcast.~~ {hidden 14:I'm} 15:Ana."), "{text}");
    assert!(text.contains("(34: 2.7s pause)"), "{text}");
    assert!(text.contains(&format!("--- scene 2 starts at {okay} ---\n# marker at {okay}: \"Running\"\n¶ {okay}..")), "{text}");
    assert!(text.contains(&format!("(note at {}: laughs) {}:let's", okay + 2, okay + 2)), "{text}");
    assert!(!text.contains('✂'));
    // The round trip still holds with all the decoration.
    for (i, w) in indexed_words(&text) {
        assert_eq!(comp.script[i].text, w);
    }

    // A deletion shows up as a cut on the word after it, and shifts the revision.
    let rev_before = script_rev(comp);
    let lets = word_index(&e, "let's");
    e.apply(Command::DeleteRange { start: 58, end: lets }).unwrap();
    let comp = e.comp().unwrap();
    let text = describe_script(comp, &e.project, 0..comp.script.len());
    assert!(text.contains("✂ 58:Let's"), "{text}");
    assert_ne!(script_rev(comp), rev_before);

    // The summary reports the same things in words.
    let summary = context::describe_summary(&e);
    assert!(summary.contains("10 ignored (struck through)"), "{summary}");
    assert!(summary.contains("SCENES: 2"), "{summary}");
    assert!(summary.contains("\"Running\" at token"), "{summary}");
    assert!(summary.contains("SPEAKERS: 2 — Ana ("), "{summary}");
}

#[test]
fn the_revision_changes_exactly_when_indices_shift() {
    let mut e = podcast_editor();
    let r0 = script_rev(e.comp().unwrap());
    // Flags and text do not move indices.
    e.apply(Command::IgnoreRange { start: 3, end: 9, ignored: true }).unwrap();
    e.apply(Command::SetMarker { at: 70, name: Some("x".into()) }).unwrap();
    e.apply(Command::CorrectRange { start: 10, end: 11, text: "Kitchn".into() }).unwrap();
    e.apply(Command::SetSpeaker { start: 0, end: 20, speaker: Some("Zed".into()) }).unwrap();
    e.apply(Command::SetCaptions { captions: CaptionSettings { enabled: true, ..Default::default() } }).unwrap();
    assert_eq!(script_rev(e.comp().unwrap()), r0);
    // Anything that adds or removes tokens does.
    let mut revs = vec![r0.clone()];
    for cmd in [
        Command::DeleteRange { start: 3, end: 4 },
        Command::InsertGap { at: 10, secs: 0.5 },
        Command::ShortenGaps { longer_than: 1.0, shorter_than: None, target: 0.4 },
        Command::MoveRange { start: 20, end: 25, dest: 80 },
        Command::ReplaceWithGap { start: 40, end: 44 },
    ] {
        e.apply(cmd.clone()).unwrap();
        let r = script_rev(e.comp().unwrap());
        assert!(!revs.contains(&r), "{cmd:?} must change the revision");
        revs.push(r);
    }
    // Undo brings the old revision back; another composition never shares one.
    while e.undo() {}
    e.redo();
    e.redo();
    assert_eq!(script_rev(e.comp().unwrap()), r0);
    let len = e.comp().unwrap().script.len();
    e.apply(Command::CompositionFromRange { start: 0, end: len, name: "copy".into() }).unwrap();
    assert_ne!(script_rev(e.comp().unwrap()), r0);
}

fn long_editor(words: usize) -> Editor {
    // ~2 hours: sentences of 12 words, speaker changes every 5 sentences.
    let mut turns: Vec<(String, String)> = Vec::new();
    let mut n = 0;
    while n < words {
        let speaker = if turns.len().is_multiple_of(2) { "Host" } else { "Guest" };
        let mut text = String::new();
        for s in 0..5 {
            for w in 0..12 {
                text.push_str(&format!("w{} ", n));
                n += 1;
                if w == 11 {
                    text.pop();
                    text.push_str(". ");
                }
            }
            if s == 2 {
                text.push_str("[1.6] ");
            }
        }
        turns.push((speaker.to_string(), text));
    }
    let refs: Vec<(&str, &str)> = turns.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    editor_from_turns(&refs)
}

#[test]
fn a_two_hour_transcript_is_outlined_and_read_in_pages() {
    let e = long_editor(19_000);
    let comp = e.comp().unwrap();
    assert!(comp.script_duration() > 6500.0, "about two hours: {}", comp.script_duration());
    let total_words = comp.script.iter().filter(|t| t.is_word()).count();
    assert!(total_words >= 19_000);

    let state = describe_project(&e);
    assert!(state.contains("SCRIPT (long; outline of paragraphs)"), "{}", &state[..600]);
    assert!(state.contains("outline only: read exact words and indices with read_script"));
    assert!(indexed_words(&state).is_empty(), "the outline has no per-word indices");
    // The whole project state stays small: well under a tenth of the indexed script.
    let full = describe_script(comp, &e.project, 0..comp.script.len());
    assert!(state.len() < 40_000, "outline is {} chars", state.len());
    assert!(state.len() * 8 < full.len(), "outline {} vs full {}", state.len(), full.len());
    // The outline's ranges tile the whole script on paragraph boundaries, so any part can be addressed and read.
    let pars = comp.paragraphs();
    let ranges: Vec<(usize, usize)> = state
        .lines()
        .filter(|l| l.starts_with('¶'))
        .map(|l| {
            let r = l.split(' ').nth(1).unwrap().split_once("..").unwrap();
            (r.0.parse().unwrap(), r.1.parse().unwrap())
        })
        .collect();
    assert!(ranges.len() > 100 && ranges.len() <= 300, "{} outline lines", ranges.len());
    assert_eq!(ranges[0].0, 0);
    assert_eq!(ranges.last().unwrap().1, comp.script.len());
    for w in ranges.windows(2) {
        assert_eq!(w[0].1, w[1].0);
        assert!(pars.iter().any(|p| p.start == w[1].0), "section starts mid-paragraph at {}", w[1].0);
    }
    assert!(state.contains("Host, Guest (") || state.contains("Guest, Host ("), "sections name their speakers");
    assert!(state.contains(&format!("duration {}", transcript_export::timecode(comp.duration()))));

    // Pages: bounded, ending on paragraph boundaries, tiling the script exactly.
    let mut start = 0;
    let mut pages = 0;
    let mut seen = 0;
    while start < comp.script.len() {
        let end = context::page_end(comp, start, context::PAGE_WORD_LIMIT);
        assert!(end > start);
        let words = comp.script[start..end].iter().filter(|t| t.is_word()).count();
        assert!(words <= context::PAGE_WORD_LIMIT, "page of {words} words");
        assert!(end == comp.script.len() || pars.iter().any(|p| p.start == end), "page ends mid-paragraph at {end}");
        let page = describe_script(comp, &e.project, start..end);
        assert!(page.len() < 40_000);
        seen += indexed_words(&page).len();
        pages += 1;
        start = end;
    }
    assert_eq!(seen, total_words);
    assert!((13..=16).contains(&pages), "{pages} pages");

    // A short script is given in full instead.
    let short = describe_project(&podcast_editor());
    assert!(short.contains("SCRIPT (complete)") && !indexed_words(&short).is_empty());
}

#[test]
fn the_assistant_pages_a_long_script_through_read_script() {
    use mango_ai::Assistant;
    use serde_json::json;
    let mut e = long_editor(6_000);
    let mock = Mock::start(vec![
        Reply::Sse(message(vec![tool_use("t1", "read_script", json!({})), tool_use("t2", "read_script", json!({"start": 4000, "end": 4100}))])),
        Reply::Sse(message(vec![text("ok")])),
    ]);
    let mut a = Assistant::new(mock_client(&mock));
    a.send(&mut e, "x", &mut |_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    let reqs = mock.requests();
    let first = reqs[0].body["messages"][0]["content"][0]["text"].as_str().unwrap();
    assert!(first.contains("SCRIPT (long; outline of paragraphs)") && indexed_words(first).is_empty());
    let res = tool_results(&reqs[1]);
    let page = indexed_words(&res[0].0);
    assert!(page.len() <= context::PAGE_WORD_LIMIT && page.len() > 1000, "{}", page.len());
    let end = context::page_end(e.comp().unwrap(), 0, context::PAGE_WORD_LIMIT);
    assert!(res[0].0.contains(&format!("(page ends at token {end}; continue with read_script start={end})")), "{}", &res[0].0[res[0].0.len() - 200..]);
    let slice = indexed_words(&res[1].0);
    assert!(slice.iter().all(|(i, _)| (4000..4100).contains(i)) && !slice.is_empty());
}

#[test]
fn sentences_cover_the_playing_text_and_snap_correctly() {
    let mut e = podcast_editor();
    let comp = e.comp().unwrap();
    let s = context::sentences(comp);
    assert_eq!(s[0].text, "Um, hello and welcome back to the the Kitchen Table podcast.");
    assert_eq!(s[1].text, "I'm Ana.");
    assert!(s.iter().any(|x| x.text == "Thanks, Ana."));
    // Sentences are in order, do not overlap, start and end on words, and together hold every playing word.
    let mut words = 0;
    for w in s.windows(2) {
        assert!(w[0].end <= w[1].start);
    }
    for x in &s {
        assert!(comp.script[x.start].shows_text() && comp.script[x.end - 1].shows_text());
        assert_eq!(x.time, comp.time_of_token(x.start));
        words += x.text.split(' ').count();
    }
    assert_eq!(words, comp.word_count(0..comp.script.len()));
    // A speaker change ends a sentence even without punctuation ("Ben, welcome." / "Thanks, Ana.").
    let thanks = word_index(&e, "thanks");
    assert!(s.iter().any(|x| x.start == thanks));

    // Snapping: any token of a sentence maps to its first word; silence before the next sentence stays with the previous one.
    let berlin = word_index(&e, "berlin");
    let sentence = s.iter().find(|x| x.start <= berlin && berlin < x.end).unwrap();
    assert_eq!(comp.script[sentence.start].text, "I");
    assert_eq!(context::snap_to_sentence_start(comp, &s, berlin), Some(sentence.start));
    assert_eq!(context::snap_to_sentence_start(comp, &s, sentence.start), Some(sentence.start));
    assert_eq!(context::snap_to_sentence_start(comp, &s, 0), Some(s[0].start));
    assert_eq!(context::snap_to_sentence_start(comp, &s, comp.script.len()), None);
    assert_eq!(context::snap_to_sentence_start(comp, &s, 99_999), None);

    // Ignored text is not part of any sentence.
    let rendered = context::render_sentences(&e.project, &s, true);
    assert!(rendered.starts_with("S1 [00:00.4] Ana: Um, hello"), "{rendered}");
    assert!(rendered.contains("] Ben: Thanks, Ana."));
    e.apply(Command::IgnoreRange { start: 0, end: 35, ignored: true }).unwrap();
    let s2 = context::sentences(e.comp().unwrap());
    assert_eq!(s2[0].text, "My guest is Ben, who, um, you know, bakes bread and also runs marathons.");
    assert_eq!(s2[0].time, 0.0);

    // An empty project renders without panicking.
    let empty = Editor::new_project("Empty");
    assert!(context::sentences(empty.comp().unwrap()).is_empty());
    let state = describe_project(&empty);
    assert!(state.contains("The script has no words") && state.contains("FILES: none") && state.contains("(the script is empty)"), "{state}");
    let mut none = Editor::new_project("None");
    none.apply(Command::DeleteComposition).unwrap();
    assert!(describe_project(&none).contains("No composition is open."));
}
