//! Engine tests: text-based editing, time mapping, detection, captions,
//! transcript export, undo, persistence. No media files needed.

use mango_core::captions::{self, CueOptions};
use mango_core::detect::{self, FillerKind, FillerOptions};
use mango_core::editor::RemoveMode;
use mango_core::script::{self, FindOptions};
use mango_core::transcript_export::{self, TranscriptFormat, TranscriptOptions};
use mango_core::*;

/// Builds an asset whose transcript is `words`, each `word_len` long with
/// `gap` seconds between them, starting at 0.5 s.
fn asset_with(words: &[&str], word_len: f64, gap: f64) -> MediaAsset {
    let mut t = 0.5;
    let mut tw = Vec::new();
    for w in words {
        tw.push(TranscriptWord { text: w.to_string(), start: t, end: t + word_len, speaker: None, confidence: 0.9 });
        t += word_len + gap;
    }
    MediaAsset {
        id: AssetId::new(),
        name: "take.mp4".into(),
        path: "/tmp/take.mp4".into(),
        kind: MediaKind::Video,
        duration: t + 0.5,
        width: 1280,
        height: 720,
        fps: 30.0,
        has_audio: true,
        has_video: true,
        sample_rate: 48000,
        channels: 2,
        folder: None,
        transcript: Some(Transcript { language: "en".into(), words: tw }),
    }
}

fn editor_with(words: &[&str], word_len: f64, gap: f64) -> (Editor, AssetId) {
    let a = asset_with(words, word_len, gap);
    let id = a.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset: a }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    (e, id)
}

fn words(e: &Editor) -> Vec<String> {
    e.comp().unwrap().script.iter().filter(|t| t.shows_text()).map(|t| t.text.clone()).collect()
}

fn word_index(e: &Editor, w: &str) -> usize {
    e.comp().unwrap().script.iter().position(|t| t.is_word() && t.text == w).unwrap_or_else(|| panic!("no word {w}"))
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn script_covers_whole_asset() {
    let (e, id) = editor_with(&["Hello", "there", "world."], 0.3, 0.2);
    let c = e.comp().unwrap();
    let dur = e.project.asset(id).unwrap().duration;
    approx(c.script_duration(), dur);
    // Tokens tile the source with no holes.
    let mut cursor = 0.0;
    for t in &c.script {
        approx(t.src_start, cursor);
        cursor = t.src_end;
    }
    approx(cursor, dur);
    // One clip, no cuts, canvas adopted from the video.
    assert_eq!(c.script_clips().len(), 1);
    assert!(c.edit_boundaries().is_empty());
    assert_eq!((c.width, c.height), (1280, 720));
    assert_eq!(words(&e), ["Hello", "there", "world."]);
}

#[test]
fn deleting_a_word_cuts_its_media() {
    let (mut e, id) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let before = e.comp().unwrap().script_duration();
    let i = word_index(&e, "two");
    let removed = e.comp().unwrap().script[i].duration();
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let c = e.comp().unwrap();
    assert_eq!(words(&e), ["one", "three", "four"]);
    approx(c.script_duration(), before - removed);
    let clips = c.script_clips();
    assert_eq!(clips.len(), 2, "one cut makes two clips");
    assert_eq!(clips[0].asset, Some(id));
    approx(clips[1].comp_start, clips[0].duration());
    assert!(clips[1].src_start > clips[0].src_end);
    assert_eq!(c.edit_boundaries().len(), 1);
}

#[test]
fn time_mapping_round_trips_across_a_cut() {
    let (mut e, id) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let i = word_index(&e, "two");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let c = e.comp().unwrap();
    let j = word_index(&e, "three");
    let t = c.time_of_token(j) + 0.1;
    let (asset, src) = c.source_at(t).unwrap();
    assert_eq!(asset, id);
    approx(src, c.script[j].src_start + 0.1);
    approx(c.time_of_source(id, src).unwrap(), t);
    assert_eq!(c.token_at_time(t), Some(j));
    // The deleted word's source time no longer plays.
    let deleted_src = 0.5 + 0.5 + 0.1; // inside "two"
    assert!(c.time_of_source(id, deleted_src).is_none());
}

#[test]
fn ignore_skips_playback_but_keeps_text_and_restores() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "two");
    e.apply(Command::IgnoreRange { start: i, end: i + 1, ignored: true }).unwrap();
    let c = e.comp().unwrap();
    assert!(c.script[i].ignored);
    assert_eq!(c.script.iter().filter(|t| t.is_word()).count(), 3, "text is still in the script");
    assert!(c.script_duration() < full);
    assert_eq!(c.script_clips().len(), 2);
    e.apply(Command::IgnoreRange { start: i, end: i + 1, ignored: false }).unwrap();
    approx(e.comp().unwrap().script_duration(), full);
    assert_eq!(e.comp().unwrap().script_clips().len(), 1);
}

#[test]
fn remove_ignored_deletes_struck_text() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let i = word_index(&e, "two");
    e.apply(Command::IgnoreRange { start: i, end: i + 1, ignored: true }).unwrap();
    e.apply(Command::RemoveIgnored).unwrap();
    assert_eq!(e.comp().unwrap().script.iter().filter(|t| t.is_word()).count(), 2);
    assert!(e.comp().unwrap().script.iter().all(|t| !t.ignored));
}

#[test]
fn restore_brings_back_deleted_media() {
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "two");
    let j = word_index(&e, "three");
    e.apply(Command::DeleteRange { start: i, end: j + 1 }).unwrap();
    assert_eq!(words(&e), ["one", "four"]);
    let b = e.comp().unwrap().edit_boundaries();
    assert_eq!(b.len(), 1);
    e.apply(Command::RestoreAt { at: b[0] }).unwrap();
    assert_eq!(words(&e), ["one", "two", "three", "four"]);
    approx(e.comp().unwrap().script_duration(), full);
    assert!(e.comp().unwrap().edit_boundaries().is_empty());
    assert!(e.apply(Command::RestoreAt { at: 2 }).is_err(), "nothing left to restore");
}

#[test]
fn replace_with_gap_keeps_timing() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "two");
    let after = e.comp().unwrap().time_of_token(i + 1);
    e.apply(Command::ReplaceWithGap { start: i, end: i + 1 }).unwrap();
    let c = e.comp().unwrap();
    approx(c.script_duration(), full);
    assert_eq!(c.script[i].kind, TokenKind::Blank);
    approx(c.time_of_token(i + 1), after);
    assert!(c.source_at(c.time_of_token(i) + 0.01).is_none(), "a gap clip plays no source");
    let clips = c.script_clips();
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[1].asset, None);
}

#[test]
fn moving_text_moves_media() {
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "four");
    let dest = word_index(&e, "two");
    e.apply(Command::MoveRange { start: i, end: i + 1, dest }).unwrap();
    assert_eq!(words(&e), ["one", "four", "two", "three"]);
    approx(e.comp().unwrap().script_duration(), full);
    // Source order is now non-monotonic, so there are cuts.
    assert!(e.comp().unwrap().edit_boundaries().len() >= 2);
}

#[test]
fn copy_paste_duplicates_media() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let i = word_index(&e, "two");
    let copied = e.comp().unwrap().script[i..i + 1].to_vec();
    let d = copied[0].duration();
    let full = e.comp().unwrap().script_duration();
    let end = e.comp().unwrap().script.len();
    e.apply(Command::PasteTokens { at: end, tokens: copied }).unwrap();
    assert_eq!(words(&e), ["one", "two", "three", "two"]);
    approx(e.comp().unwrap().script_duration(), full + d);
}

#[test]
fn paste_from_unknown_file_is_rejected() {
    let (mut e, _) = editor_with(&["one"], 0.3, 0.2);
    let foreign = Token::new(TokenKind::Word, "x", AssetId::new(), 0.0, 1.0);
    let before = e.comp().unwrap().script.len();
    assert!(e.apply(Command::PasteTokens { at: 0, tokens: vec![foreign] }).is_err());
    assert_eq!(e.comp().unwrap().script.len(), before);
    assert!(!e.can_undo() || e.undo_label() != Some("Paste"));
}

#[test]
fn correcting_text_never_changes_media() {
    let (mut e, _) = editor_with(&["I'm", "gonna", "go"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "gonna");
    let (s, en) = (e.comp().unwrap().script[i].src_start, e.comp().unwrap().script[i].src_end);
    e.apply(Command::CorrectRange { start: i, end: i + 1, text: "going to".into() }).unwrap();
    assert_eq!(words(&e), ["I'm", "going", "to", "go"]);
    let c = e.comp().unwrap();
    approx(c.script_duration(), full);
    approx(c.script[i].src_start, s);
    approx(c.script[i + 1].src_end, en);
    approx(c.script[i].src_end, c.script[i + 1].src_start);
    assert_eq!(c.script_clips().len(), 1, "a correction is not a cut");
}

#[test]
fn correct_all_fixes_every_instance_and_keeps_punctuation() {
    let (mut e, id) = editor_with(&["Mangoloco", "is", "great.", "Try", "mangoloco,", "now"], 0.3, 0.1);
    e.apply(Command::CorrectAll { from: "mangoloco".into(), to: "Mango Loco".into() }).unwrap();
    assert_eq!(words(&e), ["Mango Loco", "is", "great.", "Try", "Mango Loco,", "now"]);
    let t = e.project.asset(id).unwrap().transcript.as_ref().unwrap();
    assert_eq!(t.words[0].text, "Mango Loco");
}

#[test]
fn hidden_text_keeps_media_but_leaves_captions_and_export() {
    let (mut e, _) = editor_with(&["keep", "secret", "this"], 0.3, 0.2);
    let full = e.comp().unwrap().script_duration();
    let i = word_index(&e, "secret");
    e.apply(Command::HideText { start: i, end: i + 1, hidden: true }).unwrap();
    approx(e.comp().unwrap().script_duration(), full);
    assert_eq!(words(&e), ["keep", "this"]);
    let cues = captions::cues(e.comp().unwrap(), &CueOptions::default());
    assert!(!cues.iter().any(|c| c.text().contains("secret")));
}

#[test]
fn shorten_gaps_trims_only_long_silences() {
    let a = {
        let mut a = asset_with(&["one", "two", "three"], 0.3, 0.2);
        // Open a 3 s pause before "three".
        let t = a.transcript.as_mut().unwrap();
        t.words[2].start += 3.0;
        t.words[2].end += 3.0;
        a.duration += 3.0;
        a
    };
    let id = a.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset: a }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    let full = e.comp().unwrap().script_duration();
    e.apply(Command::ShortenGaps { longer_than: 1.0, shorter_than: None, target: 0.4 }).unwrap();
    let c = e.comp().unwrap();
    approx(c.script_duration(), full - 3.2 + 0.4);
    assert_eq!(words(&e), ["one", "two", "three"]);
    // The short 0.2 s gap between "one" and "two" is untouched.
    let i = word_index(&e, "two");
    approx(c.script[i - 1].duration(), 0.2);
    // Head and tail of the silence are kept, so the cut is inside the pause.
    assert_eq!(detect::find_gaps(c, 0.39).iter().filter(|(_, d)| (*d - 0.4).abs() < 1e-6).count(), 1);
}

#[test]
fn set_gap_duration_shortens_and_lengthens() {
    let (mut e, _) = editor_with(&["one", "two"], 0.3, 1.0);
    let i = word_index(&e, "two") - 1;
    assert_eq!(e.comp().unwrap().script[i].kind, TokenKind::Gap);
    let full = e.comp().unwrap().script_duration();
    e.apply(Command::SetGapDuration { at: i, secs: 0.2 }).unwrap();
    approx(e.comp().unwrap().script_duration(), full - 0.8);
    e.undo();
    e.apply(Command::SetGapDuration { at: i, secs: 2.5 }).unwrap();
    approx(e.comp().unwrap().script_duration(), full + 1.5);
    assert!(e.apply(Command::SetGapDuration { at: i, secs: -1.0 }).is_err());
}

#[test]
fn scenes_follow_script_edits() {
    let (mut e, _) = editor_with(&["a", "b", "c", "d", "e", "f"], 0.3, 0.2);
    let c_i = word_index(&e, "c");
    let e_i = word_index(&e, "e");
    e.apply(Command::SetSceneBreak { at: c_i, on: true }).unwrap();
    e.apply(Command::SetSceneBreak { at: e_i, on: true }).unwrap();
    let c = e.comp().unwrap();
    let spans = c.scene_spans();
    assert_eq!(spans.len(), 3);
    assert_eq!(c.scenes.len(), 3);
    approx(spans[1].start, c.time_of_token(c_i));
    approx(spans[2].end, c.script_duration());
    assert_eq!(c.scene_at_time(spans[1].start + 0.01), 1);
    // Clips never straddle a scene boundary.
    assert_eq!(c.script_clips().len(), 3);
    // Deleting the whole middle scene leaves two scenes.
    e.apply(Command::DeleteRange { start: c_i, end: e_i }).unwrap();
    assert_eq!(e.comp().unwrap().scene_spans().len(), 2);
    assert_eq!(e.comp().unwrap().scenes.len(), 2);
    // Removing the boundary merges.
    let e_i = word_index(&e, "e");
    e.apply(Command::SetSceneBreak { at: e_i, on: false }).unwrap();
    assert_eq!(e.comp().unwrap().scene_spans().len(), 1);
}

#[test]
fn markers_move_with_their_words() {
    let (mut e, _) = editor_with(&["intro", "words", "chapter", "two", "here"], 0.3, 0.2);
    let i = word_index(&e, "chapter");
    e.apply(Command::SetMarker { at: i, name: Some("Chapter Two".into()) }).unwrap();
    let t0 = e.comp().unwrap().script_markers()[0].1;
    // Delete an earlier word: the marker's time moves earlier, still on its word.
    let w = word_index(&e, "words");
    let d = e.comp().unwrap().script[w].duration();
    e.apply(Command::DeleteRange { start: w, end: w + 1 }).unwrap();
    let m = e.comp().unwrap().script_markers();
    assert_eq!(m.len(), 1);
    approx(m[0].1, t0 - d);
    assert_eq!(e.comp().unwrap().script[m[0].0].text, "chapter");
    let ch = transcript_export::chapters_text(e.comp().unwrap());
    assert!(ch.starts_with("00:00 Intro\n"), "{ch}");
    assert!(ch.contains("Chapter Two"));
}

#[test]
fn speakers_and_paragraphs() {
    let (mut e, _) = editor_with(&["hi", "there.", "hello", "back."], 0.3, 0.2);
    let i = word_index(&e, "hello");
    let n = e.comp().unwrap().script.len();
    e.apply(Command::SetSpeaker { start: 0, end: i, speaker: Some("Ana".into()) }).unwrap();
    e.apply(Command::SetSpeaker { start: i, end: n, speaker: Some("Ben".into()) }).unwrap();
    assert_eq!(e.project.speakers.len(), 2);
    assert!(e.comp().unwrap().script[i].paragraph_start);
    let txt = transcript_export::export_transcript(&e.project, e.comp().unwrap(), &TranscriptOptions::default());
    assert!(txt.contains("Ana:\nhi there."), "{txt}");
    assert!(txt.contains("Ben:\nhello back."), "{txt}");
    e.apply(Command::RenameSpeaker { from: "Ben".into(), to: "Benny".into() }).unwrap();
    let txt = transcript_export::export_transcript(&e.project, e.comp().unwrap(), &TranscriptOptions::default());
    assert!(txt.contains("Benny:"));
    assert!(e.apply(Command::RenameSpeaker { from: "Nobody".into(), to: "X".into() }).is_err());
}

#[test]
fn find_matches_phrases_across_gaps() {
    let (e, _) = editor_with(&["The", "quick", "brown", "fox.", "A", "quick", "one"], 0.3, 0.2);
    let c = e.comp().unwrap();
    let hits = script::find(c, "quick", &FindOptions::default());
    assert_eq!(hits.len(), 2);
    let hits = script::find(c, "quick brown fox", &FindOptions::default());
    assert_eq!(hits.len(), 1);
    assert_eq!(c.text_of(hits[0].clone()), "quick brown fox.");
    assert!(script::find(c, "QUICK", &FindOptions { match_case: true, ..Default::default() }).is_empty());
    assert!(script::find(c, "qui", &FindOptions { whole_words: true, ..Default::default() }).is_empty());
    assert_eq!(script::find(c, "qui", &FindOptions::default()).len(), 2);
    assert!(script::find(c, "   ", &FindOptions::default()).is_empty());
}

#[test]
fn filler_detection_and_removal() {
    let (mut e, _) = editor_with(&["So,", "um", "I", "I", "think,", "you", "know,", "it's", "a", "kind", "of", "bird", "uh", "that", "that", "works"], 0.25, 0.1);
    let hits = detect::find_fillers(e.comp().unwrap(), &FillerOptions::default());
    let kinds: Vec<(FillerKind, String)> = hits.iter().map(|h| (h.kind, h.text.clone())).collect();
    assert!(kinds.contains(&(FillerKind::Filler, "um".into())));
    assert!(kinds.contains(&(FillerKind::Filler, "uh".into())));
    assert!(kinds.contains(&(FillerKind::Repeat, "I".into())));
    assert!(kinds.contains(&(FillerKind::Phrase, "you know,".into())));
    assert!(!kinds.iter().any(|k| k.1.starts_with("kind")), "'a kind of bird' is real speech");
    assert!(!kinds.iter().any(|k| k.1 == "that"), "'that that' is legitimate");
    let full = e.comp().unwrap().script_duration();
    e.apply(Command::RemoveFillers { options: FillerOptions::default(), mode: RemoveMode::Delete }).unwrap();
    assert_eq!(words(&e), ["So,", "I", "think,", "it's", "a", "kind", "of", "bird", "that", "that", "works"]);
    assert!(e.comp().unwrap().script_duration() < full);
    assert!(detect::find_fillers(e.comp().unwrap(), &FillerOptions::default()).is_empty());
    // One undo step restores everything.
    e.undo();
    approx(e.comp().unwrap().script_duration(), full);
}

#[test]
fn filler_removal_as_ignore_keeps_text() {
    let (mut e, _) = editor_with(&["well", "um", "yes"], 0.25, 0.1);
    e.apply(Command::RemoveFillers { options: FillerOptions::default(), mode: RemoveMode::Ignore }).unwrap();
    let c = e.comp().unwrap();
    assert_eq!(c.script.iter().filter(|t| t.is_word()).count(), 3);
    assert_eq!(c.script.iter().filter(|t| t.ignored).count(), 1);
}

#[test]
fn retake_detection_marks_the_earlier_take() {
    let (mut e, _) = editor_with(
        &["Welcome", "to", "the", "show", "today", "we", "Welcome", "to", "the", "show.", "Today", "we", "talk", "about", "mangoes."],
        0.25,
        0.1,
    );
    let hits = detect::find_retakes(e.comp().unwrap(), 3, 60);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].text, "Welcome to the show today we");
    e.apply(Command::RemoveRetakes { min_words: 3, mode: RemoveMode::Ignore }).unwrap();
    assert_eq!(words(&e), ["Welcome", "to", "the", "show.", "Today", "we", "talk", "about", "mangoes."]);
}

#[test]
fn remove_matches_reports_missing_text() {
    let (mut e, _) = editor_with(&["alpha", "beta"], 0.25, 0.1);
    let err = e.apply(Command::RemoveMatches { query: "gamma".into(), mode: RemoveMode::Delete }).unwrap_err();
    assert!(err.to_string().contains("not found"));
    e.apply(Command::RemoveMatches { query: "beta".into(), mode: RemoveMode::Delete }).unwrap();
    assert_eq!(words(&e), ["alpha"]);
}

#[test]
fn captions_respect_line_limits_and_timing() {
    let text = "This is a fairly long sentence that needs to be wrapped across several caption cards. And here is another one.";
    let ws: Vec<&str> = text.split(' ').collect();
    let (e, _) = editor_with(&ws, 0.25, 0.05);
    let c = e.comp().unwrap();
    let opts = CueOptions { max_chars_per_line: 20, max_lines: 2, ..Default::default() };
    let cues = captions::cues(c, &opts);
    assert!(cues.len() >= 3);
    let mut all = Vec::new();
    for (i, cue) in cues.iter().enumerate() {
        assert!(cue.lines.len() <= 2, "{cue:?}");
        for l in &cue.lines {
            assert!(l.chars().count() <= 20, "line too long: {l:?}");
        }
        assert!(cue.end > cue.start);
        if i > 0 {
            assert!(cue.start >= cues[i - 1].end - 1e-9, "cues overlap");
        }
        all.extend(cue.words.iter().map(|w| w.text.clone()));
    }
    assert_eq!(all, ws, "every word appears exactly once, in order");
    // Cue times are composition times of the words.
    let first = word_index(&e, "This");
    approx(cues[0].start, c.time_of_token(first));
}

#[test]
fn captions_follow_edits() {
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let before = captions::cues(e.comp().unwrap(), &CueOptions::default());
    let i = word_index(&e, "two");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let after = captions::cues(e.comp().unwrap(), &CueOptions::default());
    assert_eq!(after[0].text(), "one three four");
    assert!(after.last().unwrap().end < before.last().unwrap().end);
}

#[test]
fn srt_and_vtt_round_trip() {
    let (e, _) = editor_with(&["Hello", "world.", "Second", "line", "here."], 0.4, 0.1);
    let cues = captions::cues(e.comp().unwrap(), &CueOptions { max_chars_per_line: 14, max_lines: 1, ..Default::default() });
    let srt = captions::to_srt(&cues, Some(&e.project), false);
    assert!(srt.starts_with("1\n00:00:00,500 --> "), "{srt}");
    let parsed = captions::parse_subtitles(&srt);
    assert_eq!(parsed.len(), cues.len());
    for (p, c) in parsed.iter().zip(&cues) {
        assert!((p.start - c.start).abs() < 0.002 && (p.end - c.end).abs() < 0.002);
        assert_eq!(p.text, c.text());
    }
    let vtt = captions::to_vtt(&cues, Some(&e.project), false);
    assert!(vtt.starts_with("WEBVTT\n\n"));
    assert!(vtt.contains("00:00:00.500 --> "));
    assert_eq!(captions::parse_subtitles(&vtt).len(), cues.len());
    let words = captions::parsed_cues_to_words(&parsed);
    assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Hello", "world.", "Second", "line", "here."]);
    assert!(captions::parse_subtitles("not a subtitle file").is_empty());
}

#[test]
fn transcript_formats() {
    let (mut e, _) = editor_with(&["Tom", "&", "Jerry", "<3", "é"], 0.3, 0.1);
    let i = word_index(&e, "Jerry");
    e.apply(Command::IgnoreRange { start: i, end: i + 1, ignored: true }).unwrap();
    e.apply(Command::SetMarker { at: 0, name: Some("Part 1".into()) }).unwrap();
    let c = e.comp().unwrap();
    let mk = |format, include_ignored| transcript_export::export_transcript(&e.project, c, &TranscriptOptions { format, include_ignored, ..Default::default() });
    let txt = mk(TranscriptFormat::Txt, false);
    assert!(txt.contains("# Part 1") && txt.contains("Tom & <3 é") && !txt.contains("Jerry"), "{txt}");
    let md = mk(TranscriptFormat::Markdown, true);
    assert!(md.contains("## Part 1") && md.contains("~~Jerry~~"), "{md}");
    let html = mk(TranscriptFormat::Html, true);
    assert!(html.contains("<h2>Part 1</h2>") && html.contains("Tom &amp; <s>Jerry</s> &lt;3 é"), "{html}");
    let rtf = mk(TranscriptFormat::Rtf, false);
    assert!(rtf.starts_with("{\\rtf1") && rtf.contains("\\u233?") && rtf.trim_end().ends_with('}'), "{rtf}");
    let tc = transcript_export::export_transcript(&e.project, c, &TranscriptOptions { paragraph_timecodes: true, timecode_offset: 3600.0, ..Default::default() });
    assert!(tc.contains("[1:00:00]"), "{tc}");
}

#[test]
fn undo_redo_and_failed_commands() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let original = e.project.clone();
    let i = word_index(&e, "two");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let edited = e.project.clone();
    assert_eq!(e.undo_label(), Some("Delete"));
    assert!(e.undo());
    assert_eq!(e.project, original);
    assert!(e.redo());
    assert_eq!(e.project, edited);
    assert!(!e.redo());
    // A failing command changes nothing and adds no undo step.
    let rev = e.revision;
    assert!(e.apply(Command::DeleteRange { start: 5, end: 9999 }).is_err());
    assert_eq!(e.project, edited);
    assert_eq!(e.revision, rev);
    // A no-op adds no undo step either.
    e.apply(Command::DeleteRange { start: 1, end: 1 }).unwrap();
    assert_eq!(e.undo_label(), Some("Delete"));
    // A new edit clears redo.
    e.undo();
    e.apply(Command::RenameComposition { name: "Renamed".into() }).unwrap();
    assert!(!e.can_redo());
}

#[test]
fn batch_is_one_undo_step_and_atomic() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let original = e.project.clone();
    let i = word_index(&e, "two");
    let bad = e.apply_all("Batch", vec![Command::DeleteRange { start: i, end: i + 1 }, Command::DeleteRange { start: 500, end: 600 }]);
    assert!(bad.is_err());
    assert_eq!(e.project, original, "a failed batch rolls back completely");
    e.apply_all("Batch", vec![Command::DeleteRange { start: i, end: i + 1 }, Command::RenameComposition { name: "X".into() }]).unwrap();
    assert_eq!(e.undo_label(), Some("Batch"));
    e.undo();
    assert_eq!(e.project, original);
}

#[test]
fn compositions_duplicate_and_split_out() {
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let first = e.active.unwrap();
    let i = word_index(&e, "two");
    let j = word_index(&e, "three");
    e.apply(Command::CompositionFromRange { start: i, end: j + 1, name: "Clip".into() }).unwrap();
    assert_ne!(e.active.unwrap(), first);
    assert_eq!(words(&e), ["two", "three"]);
    assert!(e.comp().unwrap().script[0].paragraph_start);
    e.apply(Command::DuplicateComposition { name: None }).unwrap();
    assert_eq!(e.comp().unwrap().name, "Clip (Copy)");
    assert_eq!(e.project.compositions.len(), 3);
    e.apply(Command::DeleteComposition).unwrap();
    assert_eq!(e.project.compositions.len(), 2);
    e.apply(Command::SelectComposition { id: first }).unwrap();
    assert_eq!(words(&e), ["one", "two", "three", "four"], "the source composition is untouched");
    e.apply(Command::NewComposition { name: "Clip".into() }).unwrap();
    assert_eq!(e.comp().unwrap().name, "Clip 2", "names are made unique");
}

#[test]
fn layers_add_split_reorder_remove() {
    let (mut e, id) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let title = Layer::new("Title", 0.5, 2.0, LayerContent::Text(TextStyle::default()));
    let tid = title.id;
    e.apply(Command::AddLayer { layer: title.clone() }).unwrap();
    assert!(e.apply(Command::AddLayer { layer: title }).is_err(), "duplicate ids are rejected");
    let mut music = Layer::new("Music", 0.0, 10.0, LayerContent::Audio(MediaClip::new(id)));
    music.fade_out = 1.0;
    let mid = music.id;
    e.apply(Command::AddLayer { layer: music }).unwrap();
    approx(e.comp().unwrap().duration(), 10.0);
    e.apply(Command::SplitLayer { id: mid, time: 4.0 }).unwrap();
    let c = e.comp().unwrap();
    assert_eq!(c.layers.len(), 3);
    let left = c.layer(mid).unwrap();
    let right = c.layers.last().unwrap();
    approx(left.duration, 4.0);
    approx(right.start, 4.0);
    approx(right.duration, 6.0);
    approx(left.fade_out, 0.0);
    approx(right.fade_out, 1.0);
    match &right.content {
        LayerContent::Audio(m) => approx(m.src_in, 4.0),
        _ => panic!(),
    }
    assert!(e.apply(Command::SplitLayer { id: tid, time: 99.0 }).is_err());
    e.apply(Command::ReorderLayer { id: tid, delta: 3 }).unwrap();
    assert_eq!(e.comp().unwrap().layer(tid).unwrap().track, 4);
    e.apply(Command::DuplicateLayer { id: tid }).unwrap();
    e.apply(Command::RemoveLayer { id: tid }).unwrap();
    assert!(e.comp().unwrap().layer(tid).is_none());
    assert!(e.apply(Command::RemoveLayer { id: tid }).is_err());
    let ghost = Layer::new("Ghost", 0.0, 1.0, LayerContent::Audio(MediaClip::new(AssetId::new())));
    assert!(e.apply(Command::AddLayer { layer: ghost }).is_err(), "layers must point at project files");
}

#[test]
fn files_in_use_cannot_be_removed() {
    let (mut e, id) = editor_with(&["one"], 0.3, 0.2);
    assert!(e.apply(Command::RemoveAsset { id }).is_err());
    let n = e.comp().unwrap().script.len();
    e.apply(Command::DeleteRange { start: 0, end: n }).unwrap();
    e.apply(Command::RemoveAsset { id }).unwrap();
    assert!(e.project.assets.is_empty());
}

#[test]
fn comments_follow_their_moment() {
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let j = word_index(&e, "three");
    let t = e.comp().unwrap().time_of_token(j) + 0.05;
    e.apply(Command::AddComment { time: t, author: "Me".into(), text: "Tighten this".into() }).unwrap();
    let i = word_index(&e, "one");
    let d = e.comp().unwrap().script[i].duration();
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let c = e.comp().unwrap().comments[0].clone();
    approx(e.comment_time(&c), t - d);
    e.apply(Command::ResolveComment { id: c.id, resolved: true }).unwrap();
    e.apply(Command::ReplyComment { id: c.id, author: "You".into(), text: "Done".into() }).unwrap();
    assert!(e.comp().unwrap().comments[0].resolved);
    assert_eq!(e.comp().unwrap().comments[0].replies.len(), 1);
    assert!(e.apply(Command::AddComment { time: 0.0, author: "Me".into(), text: "  ".into() }).is_err());
}

#[test]
fn transcribing_later_fills_in_words_without_losing_cuts() {
    // Import with no transcript: the script is one silent stretch.
    let mut a = asset_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let transcript = a.transcript.take().unwrap();
    let id = a.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset: a }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    assert_eq!(e.comp().unwrap().script.len(), 1);
    let full = e.comp().unwrap().script_duration();
    // Cut the first second on the timeline before any transcript exists.
    e.apply(Command::SplitAt { time: 1.0 }).unwrap();
    e.apply(Command::DeleteRange { start: 0, end: 1 }).unwrap();
    approx(e.comp().unwrap().script_duration(), full - 1.0);
    e.apply(Command::SetTranscript { asset: id, transcript }).unwrap();
    // "one" (0.5–0.8) was cut; "two" starts at 1.0.
    assert_eq!(words(&e), ["two", "three", "four"]);
    approx(e.comp().unwrap().script_duration(), full - 1.0);
}

#[test]
fn glossary_rewrites_misrecognitions() {
    let mut a = asset_with(&["Welcome", "to", "mango", "loco,", "everyone"], 0.3, 0.1);
    let t = a.transcript.take().unwrap();
    let id = a.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset: a }).unwrap();
    e.apply(Command::SetGlossary { glossary: vec![GlossaryEntry { term: "Loco".into(), sounds_like: vec!["loko".into()] }] }).unwrap();
    e.apply(Command::SetTranscript { asset: id, transcript: t }).unwrap();
    let w = &e.project.asset(id).unwrap().transcript.as_ref().unwrap().words;
    assert_eq!(w[3].text, "Loco,");
}

#[test]
fn save_and_load_round_trip() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let i = word_index(&e, "two");
    e.apply(Command::IgnoreRange { start: i, end: i + 1, ignored: true }).unwrap();
    e.apply(Command::AddLayer { layer: Layer::new("Title", 0.0, 1.0, LayerContent::Text(TextStyle::default())) }).unwrap();
    e.apply(Command::SetCaptions { captions: CaptionSettings { enabled: true, ..Default::default() } }).unwrap();
    let dir = std::env::temp_dir().join(format!("mango-core-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.mango");
    assert!(e.is_dirty());
    e.save(&path).unwrap();
    assert!(!e.is_dirty());
    let loaded = Editor::load(&path).unwrap();
    assert_eq!(loaded.project, e.project);
    // A truncated file is an error, not a panic or an empty project.
    std::fs::write(&path, "{\"format_version\":1,").unwrap();
    assert!(Editor::load(&path).is_err());
    // A project from a newer version is refused.
    let mut newer = e.project.clone();
    newer.format_version = PROJECT_FORMAT_VERSION + 1;
    std::fs::write(&path, serde_json::to_string(&newer).unwrap()).unwrap();
    assert!(Editor::load(&path).err().unwrap().to_string().contains("newer version"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn commands_serialise_for_the_assistant_and_cli() {
    let cmd: Command = serde_json::from_str(r#"{"cmd":"delete_range","start":2,"end":5}"#).unwrap();
    assert_eq!(cmd, Command::DeleteRange { start: 2, end: 5 });
    let cmd: Command = serde_json::from_str(r#"{"cmd":"shorten_gaps","longer_than":1.0,"shorter_than":null,"target":0.3}"#).unwrap();
    assert_eq!(cmd, Command::ShortenGaps { longer_than: 1.0, shorter_than: None, target: 0.3 });
    assert!(serde_json::from_str::<Command>(r#"{"cmd":"format_disk"}"#).is_err());
}

#[test]
fn canvas_and_settings_validation() {
    let (mut e, _) = editor_with(&["one"], 0.3, 0.2);
    e.apply(Command::SetCanvas { width: 1080, height: 1920, fill: true }).unwrap();
    let c = e.comp().unwrap();
    assert_eq!((c.width, c.height, c.script_video.fit), (1080, 1920, FitMode::Fill));
    assert!(e.apply(Command::SetCanvas { width: 4, height: 1920, fill: true }).is_err());
    assert!(e.apply(Command::SetFps { fps: 0.0 }).is_err());
    assert!(e.apply(Command::SetMasterLoudness { lufs: Some(3.0) }).is_err());
    e.apply(Command::SetMasterLoudness { lufs: Some(-16.0) }).unwrap();
    assert!(e.apply(Command::InsertGap { at: 0, secs: f64::NAN }).is_err());
    assert_eq!(Rgba::parse_hex("#FF8A00"), Some(Rgba::rgb(0xFF, 0x8A, 0x00)));
    assert_eq!(Rgba::parse_hex("nope"), None);
}

#[test]
fn edl_lists_every_cut_with_matching_lengths() {
    use mango_core::timeline_export::{timecode, to_edl};
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    let i = word_index(&e, "two");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    e.apply(Command::SetMarker { at: word_index(&e, "four"), name: Some("End bit".into()) }).unwrap();
    let edl = to_edl(&e.project, e.comp().unwrap());
    assert!(edl.starts_with("TITLE: Composition 1\nFCM: NON-DROP FRAME\n"), "{edl}");
    let events: Vec<&str> = edl.lines().filter(|l| l.starts_with("00")).collect();
    assert_eq!(events.len(), 2, "{edl}");
    let tc_frames = |tc: &str| {
        let p: Vec<i64> = tc.split(':').map(|x| x.parse().unwrap()).collect();
        ((p[0] * 60 + p[1]) * 60 + p[2]) * 30 + p[3]
    };
    let mut record_cursor = 0;
    for ev in &events {
        let f: Vec<&str> = ev.split_whitespace().collect();
        assert_eq!(f[1], "TAKEMP4");
        assert_eq!(f[2], "AA/V");
        let (si, so, ri, ro) = (tc_frames(f[4]), tc_frames(f[5]), tc_frames(f[6]), tc_frames(f[7]));
        assert_eq!(so - si, ro - ri, "source and record lengths match: {ev}");
        assert_eq!(ri, record_cursor, "record side is contiguous: {ev}");
        record_cursor = ro;
    }
    // "two" was 0.5 s (word + gap after "one" starts at 1.0 s): second event's source starts later than the first's end.
    let a: Vec<&str> = events[0].split_whitespace().collect();
    let b: Vec<&str> = events[1].split_whitespace().collect();
    assert!(tc_frames(b[4]) > tc_frames(a[5]));
    assert!(edl.contains("* MARKER 00:00:0") && edl.contains("End bit"), "{edl}");
    assert_eq!(timecode(3661.5, 30.0), "01:01:01:15");
    assert_eq!(timecode(-1.0, 25.0), "00:00:00:00");
}

#[test]
fn fcpxml_is_well_formed_and_frame_exact() {
    use mango_core::timeline_export::to_fcpxml;
    let (mut e, _) = editor_with(&["one", "two", "three", "four"], 0.3, 0.2);
    e.project.assets[0].name = "Tom & \"Jerry\" <take>.mp4".into();
    e.project.assets[0].path = "/tmp/my takes/é.mp4".into();
    let i = word_index(&e, "two");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 1 }).unwrap();
    e.apply(Command::SetMarker { at: word_index(&e, "four"), name: Some("A < B".into()) }).unwrap();
    let x = to_fcpxml(&e.project, e.comp().unwrap());
    assert!(x.contains("<fcpxml version=\"1.9\">"));
    assert!(x.contains("name=\"Tom &amp; &quot;Jerry&quot; &lt;take&gt;.mp4\""), "{x}");
    assert!(x.contains("src=\"file:///tmp/my%20takes/%C3%A9.mp4\""), "{x}");
    assert!(x.contains("<gap name=\"Gap\""));
    assert!(x.contains("value=\"A &lt; B\""));
    assert_eq!(x.matches("<asset-clip ").count(), 2);
    // Every tag that opens closes (cheap well-formedness check; xmllint runs in the media tests).
    for tag in ["fcpxml", "resources", "library", "event", "project", "sequence", "spine"] {
        assert_eq!(x.matches(&format!("<{tag}")).count(), x.matches(&format!("</{tag}>")).count(), "{tag}");
    }
    // Offsets and durations tile the spine exactly, in 1/30 s units (100/3000 s).
    let grab = |line: &str, key: &str| -> i64 {
        let s = line.split(&format!("{key}=\"")).nth(1).unwrap();
        s.split('/').next().unwrap().trim_end_matches('s').parse().unwrap()
    };
    let mut cursor = 0;
    for line in x.lines().filter(|l| l.contains("<asset-clip ") || l.contains("<gap ")) {
        assert_eq!(grab(line, "offset"), cursor, "{line}");
        let d = grab(line, "duration");
        assert_eq!(d % 100, 0, "whole frames: {line}");
        cursor += d;
    }
    let seq = x.lines().find(|l| l.contains("<sequence ")).unwrap();
    assert_eq!(grab(seq, "duration"), cursor);
}

#[test]
fn commit_state_is_one_undo_step() {
    let (mut e, _) = editor_with(&["one", "two", "three"], 0.3, 0.2);
    let original = e.project.clone();
    let depth = e.undo_depth();
    // Work on a copy, several edits deep.
    let mut copy = Editor::new(e.project.clone());
    copy.active = e.active;
    let i = copy.comp().unwrap().script.iter().position(|t| t.text == "two").unwrap();
    copy.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    copy.apply(Command::RenameComposition { name: "Edited".into() }).unwrap();
    assert!(e.commit_state("Assistant: tidy", copy.project.clone(), copy.active));
    assert_eq!(e.undo_depth(), depth + 1);
    assert_eq!(e.undo_label(), Some("Assistant: tidy"));
    assert_eq!(e.comp().unwrap().name, "Edited");
    assert!(e.undo());
    assert_eq!(e.project, original);
    assert!(e.redo());
    assert_eq!(e.project, copy.project);
    // An identical copy adds nothing.
    assert!(!e.commit_state("Nothing", e.project.clone(), e.active));
    assert_eq!(e.undo_depth(), depth + 1);
    // A stale active id falls back to a real composition.
    let mut p = e.project.clone();
    p.name = "Renamed".into();
    assert!(e.commit_state("Rename", p, Some(CompositionId::new())));
    assert!(e.comp().is_some());
}

#[test]
fn project_times_survive_save_and_load_exactly() {
    let mut a = asset_with(&["x"], 0.3, 0.2);
    // Values that do not round-trip through a non-exact float parser.
    let t = a.transcript.as_mut().unwrap();
    t.words[0].start = 1.3800000000000001;
    t.words[0].end = 2.0100000000000002;
    let mut e = Editor::new_project("Floats");
    e.apply(Command::AddAsset { asset: a }).unwrap();
    let dir = std::env::temp_dir().join(format!("mango-core-float-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.mango");
    e.save(&path).unwrap();
    let loaded = Editor::load(&path).unwrap();
    assert_eq!(loaded.project, e.project);
    std::fs::remove_dir_all(&dir).ok();
}
