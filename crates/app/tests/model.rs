//! Behaviour tests for the app model: what the keyboard and mouse do,
//! without a window.

use mango_core::editor::RemoveMode;
use mango_core::*;
use mangoloco::model::*;

fn model(words: &[&str]) -> AppModel {
    model_with_gap(words, 0.2)
}

fn model_with_gap(words: &[&str], gap: f64) -> AppModel {
    let mut t = 0.5;
    let mut tw = Vec::new();
    for w in words {
        tw.push(TranscriptWord { text: w.to_string(), start: t, end: t + 0.3, speaker: None, confidence: 1.0 });
        t += 0.3 + gap;
    }
    let asset = MediaAsset {
        id: AssetId::new(),
        name: "take.mp4".into(),
        path: "/tmp/take.mp4".into(),
        kind: MediaKind::Video,
        duration: t + 0.5,
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: true,
        has_video: true,
        sample_rate: 48000,
        channels: 2,
        folder: None,
        transcript: Some(Transcript { language: "en".into(), words: tw }),
    };
    let id = asset.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    AppModel::new(e)
}

fn words(m: &AppModel) -> Vec<String> {
    m.comp().unwrap().script.iter().filter(|t| t.shows_text()).map(|t| t.text.clone()).collect()
}

fn idx(m: &AppModel, w: &str) -> usize {
    m.comp().unwrap().script.iter().position(|t| t.is_word() && t.text == w).unwrap_or_else(|| panic!("no word {w}"))
}

fn selected_text(m: &AppModel) -> String {
    m.comp().unwrap().text_of(m.selection())
}

#[test]
fn click_places_caret_and_seeks() {
    let mut m = model(&["one", "two", "three"]);
    let i = idx(&m, "two");
    m.click_token(i, false);
    assert!(m.selection().is_empty());
    assert_eq!(m.target(), i..i + 1);
    assert!((m.playhead - m.comp().unwrap().time_of_token(i)).abs() < 1e-9);
    assert_eq!(m.playing_token(), Some(i));
}

#[test]
fn shift_click_and_drag_select_ranges_in_both_directions() {
    let mut m = model(&["one", "two", "three", "four"]);
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "four"), true);
    assert_eq!(selected_text(&m), "two three four");
    m.click_token(idx(&m, "three"), false);
    m.drag_to_token(idx(&m, "one"));
    assert_eq!(selected_text(&m), "one two three");
    // Dragging back onto the anchor keeps a one-word selection.
    m.drag_to_token(idx(&m, "three"));
    assert_eq!(selected_text(&m), "three");
}

#[test]
fn arrows_skip_short_silences_and_extend_with_shift() {
    let mut m = model(&["one", "two", "three"]);
    m.click_token(idx(&m, "one"), false);
    m.move_caret(1, false);
    assert_eq!(m.head, idx(&m, "two"), "the 0.2 s gap is not a stop");
    m.move_caret(1, true);
    assert_eq!(selected_text(&m), "two three");
    m.move_caret(-1, false);
    assert_eq!(m.head, idx(&m, "two"));
    assert!(m.selection().is_empty());
    // At the edges nothing moves.
    m.click_token(idx(&m, "one"), false);
    m.move_caret(-1, false);
    m.move_caret(-1, false);
    assert!(m.head <= idx(&m, "one"));
}

#[test]
fn long_silences_are_caret_stops() {
    let mut m = model_with_gap(&["one", "two"], 2.0);
    m.click_token(idx(&m, "one"), false);
    m.move_caret(1, false);
    let c = m.comp().unwrap();
    assert_eq!(c.script[m.head].kind, TokenKind::Gap);
    assert!(AppModel::is_visible(&c.script[m.head]));
}

#[test]
fn delete_and_backspace() {
    let mut m = model(&["one", "two", "three", "four"]);
    m.click_token(idx(&m, "two"), false);
    assert!(m.delete(true));
    assert_eq!(words(&m), ["one", "three", "four"]);
    assert_eq!(m.head, idx(&m, "three"), "caret lands on the word that took its place");
    assert!(m.delete(false), "backspace removes the word before the caret");
    assert_eq!(words(&m), ["three", "four"]);
    m.select_all();
    assert!(m.delete(true));
    assert!(words(&m).is_empty());
    assert!(!m.delete(true), "nothing left to delete");
    assert!(!m.delete(false));
    // Four undos bring everything back (the fourth is the failed-nothing no-op guard).
    assert!(m.undo() && m.undo() && m.undo());
    assert_eq!(words(&m), ["one", "two", "three", "four"]);
}

#[test]
fn ignore_toggles() {
    let mut m = model(&["one", "two", "three"]);
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "three"), true);
    let full = m.duration();
    assert!(m.toggle_ignore());
    assert!(m.duration() < full);
    assert!(m.comp().unwrap().script[idx(&m, "two")].ignored);
    assert!(m.toggle_ignore(), "toggling an all-ignored selection restores it");
    assert!((m.duration() - full).abs() < 1e-9);
}

#[test]
fn cut_copy_paste_moves_and_duplicates() {
    let mut m = model(&["one", "two", "three", "four"]);
    assert!(!m.paste(), "empty clipboard");
    assert!(m.toast.as_ref().unwrap().error);
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "three"), true);
    assert_eq!(m.cut().as_deref(), Some("two three"));
    assert_eq!(words(&m), ["one", "four"]);
    let end = m.comp().unwrap().script.len();
    m.select_range(end..end);
    assert!(m.paste());
    assert_eq!(words(&m), ["one", "four", "two", "three"]);
    assert_eq!(selected_text(&m), "two three", "the pasted text is selected");
    // Paste over a selection replaces it, as one undo step.
    m.click_token(idx(&m, "one"), false);
    m.click_token(idx(&m, "one"), true);
    assert!(m.paste());
    assert_eq!(words(&m), ["two", "three", "four", "two", "three"]);
    assert!(m.undo());
    assert_eq!(words(&m), ["one", "four", "two", "three"]);
}

#[test]
fn correct_uses_selection_or_caret_word() {
    let mut m = model(&["their", "going", "home"]);
    m.click_token(idx(&m, "their"), false);
    assert_eq!(m.correction_seed(), "their");
    assert!(m.correct("they're"));
    assert_eq!(words(&m), ["they're", "going", "home"]);
    m.click_token(idx(&m, "going"), false);
    m.click_token(idx(&m, "home"), true);
    assert_eq!(m.correction_seed(), "going home");
    assert!(m.correct("going back home"));
    assert_eq!(words(&m), ["they're", "going", "back", "home"]);
}

#[test]
fn restore_at_caret_and_in_selection() {
    let mut m = model(&["one", "two", "three", "four"]);
    assert!(!m.restore(), "nothing cut yet");
    m.click_token(idx(&m, "two"), false);
    m.delete(true);
    m.click_token(idx(&m, "three"), false);
    assert!(m.restore());
    assert_eq!(words(&m), ["one", "two", "three", "four"]);
    // Two separate cuts restored together by selecting across them.
    m.click_token(idx(&m, "two"), false);
    m.delete(true);
    m.click_token(idx(&m, "four"), false);
    m.delete(true);
    assert_eq!(words(&m), ["one", "three"]);
    m.select_all();
    assert!(m.restore());
    assert_eq!(words(&m), ["one", "two", "three", "four"]);
}

#[test]
fn scene_and_paragraph_breaks_at_caret() {
    let mut m = model(&["one", "two", "three"]);
    m.click_token(0, false);
    assert!(!m.toggle_scene_break(), "cannot split before the first token");
    m.click_token(idx(&m, "two"), false);
    assert!(m.toggle_scene_break());
    assert_eq!(m.comp().unwrap().scene_spans().len(), 2);
    assert!(m.toggle_scene_break());
    assert_eq!(m.comp().unwrap().scene_spans().len(), 1);
    assert!(m.toggle_paragraph_break());
    assert_eq!(m.comp().unwrap().paragraphs().len(), 2);
}

#[test]
fn split_at_playhead_makes_a_scene_mid_word() {
    let mut m = model(&["one", "two", "three"]);
    let i = idx(&m, "two");
    let t = m.comp().unwrap().time_of_token(i) + 0.15;
    m.seek(t);
    let full = m.duration();
    assert!(m.split_at_playhead());
    let c = m.comp().unwrap();
    let spans = c.scene_spans();
    assert_eq!(spans.len(), 2);
    assert!((spans[1].start - t).abs() < 1e-6);
    assert!((c.duration() - full).abs() < 1e-9, "splitting never changes duration");
    // At the very start or end there is nothing to split.
    m.seek(0.0);
    assert!(!m.split_at_playhead());
}

#[test]
fn transport_clamps_and_stops_at_end() {
    let mut m = model(&["one", "two"]);
    let d = m.duration();
    m.seek(1e9);
    assert_eq!(m.playhead, d);
    m.seek(-5.0);
    assert_eq!(m.playhead, 0.0);
    m.toggle_play();
    assert!(m.playing);
    assert!(m.tick(0.5));
    assert!((m.playhead - 0.5).abs() < 1e-9);
    m.set_rate(2.0);
    m.tick(0.25);
    assert!((m.playhead - 1.0).abs() < 1e-6);
    assert!(!m.tick(1e6), "reaching the end stops playback");
    assert_eq!(m.playhead, d);
    m.toggle_play();
    assert!(m.playing && m.playhead == 0.0, "play at the end restarts from the top");
    m.set_rate(99.0);
    assert_eq!(m.rate, 3.0);
}

#[test]
fn frame_stepping_snaps_to_frames() {
    let mut m = model(&["one", "two"]);
    m.seek(0.5);
    m.step_frames(1);
    assert!((m.playhead - 16.0 / 30.0).abs() < 1e-9);
    m.step_frames(-100);
    assert_eq!(m.playhead, 0.0);
}

#[test]
fn jump_to_time_parses_common_formats() {
    assert_eq!(parse_time("90"), Some(90.0));
    assert_eq!(parse_time("1:30"), Some(90.0));
    assert_eq!(parse_time(" 1:02:03 "), Some(3723.0));
    assert_eq!(parse_time("0:01.5"), Some(1.5));
    assert_eq!(parse_time("1:75"), None);
    assert_eq!(parse_time("abc"), None);
    assert_eq!(parse_time(""), None);
    assert_eq!(parse_time("-3"), None);
    let mut m = model(&["one", "two"]);
    assert!(m.jump_to_time("0:01").is_ok());
    assert_eq!(m.playhead, 1.0);
    assert!(m.jump_to_time("9:00").unwrap_err().to_string().contains("only"));
    assert!(m.jump_to_time("soon").is_err());
    assert_eq!(m.playhead, 1.0, "a bad jump leaves the playhead alone");
}

#[test]
fn scene_and_marker_navigation() {
    let mut m = model(&["a", "b", "c", "d"]);
    m.click_token(idx(&m, "c"), false);
    m.toggle_scene_break();
    m.set_marker(Some("Part two".into()));
    let t = m.comp().unwrap().time_of_token(idx(&m, "c"));
    m.seek(0.0);
    m.jump_scene(1);
    assert!((m.playhead - t).abs() < 1e-9);
    m.jump_scene(1);
    assert_eq!(m.playhead, m.duration());
    m.jump_scene(-1);
    assert!((m.playhead - t).abs() < 1e-9);
    m.seek(0.0);
    m.jump_marker(1);
    assert!((m.playhead - t).abs() < 1e-9);
    m.jump_marker(1);
    assert!((m.playhead - t).abs() < 1e-9, "no later marker: stay put");
}

#[test]
fn find_selects_hits_and_cycles() {
    let mut m = model(&["the", "cat", "and", "the", "hat"]);
    m.open_find();
    m.set_find_query("the");
    assert_eq!(m.find.as_ref().unwrap().hits.len(), 2);
    let first = idx(&m, "the");
    assert_eq!(m.selection().start, first);
    m.find_next(1);
    assert_eq!(selected_text(&m), "the");
    assert!(m.selection().start > first);
    m.find_next(1);
    assert_eq!(m.selection().start, first, "wraps around");
    m.find_next(-1);
    assert!(m.selection().start > first);
    // Editing keeps results current.
    m.select_range(first..first + 1);
    m.delete(true);
    assert_eq!(m.find.as_ref().unwrap().hits.len(), 1);
    assert!(m.remove_find_hits(RemoveMode::Delete));
    assert_eq!(words(&m), ["cat", "and", "hat"]);
    assert!(m.find.as_ref().unwrap().hits.is_empty());
    m.set_find_query("zebra");
    assert!(!m.remove_find_hits(RemoveMode::Delete));
    assert!(m.toast.as_ref().unwrap().error);
    m.close_find();
    assert!(m.find.is_none());
}

#[test]
fn filler_tools_report_what_they_did() {
    let mut m = model(&["so", "um", "this", "uh", "works"]);
    assert_eq!(m.fillers(&Default::default()).len(), 2);
    assert!(m.remove_fillers(Default::default(), RemoveMode::Delete));
    assert_eq!(m.toast.as_ref().unwrap().text, "Removed 2 filler words");
    assert_eq!(words(&m), ["so", "this", "works"]);
    assert!(!m.remove_fillers(Default::default(), RemoveMode::Delete));
    assert_eq!(m.toast.as_ref().unwrap().text, "No filler words found");
}

#[test]
fn shorten_gaps_reports_time_saved() {
    let mut m = model_with_gap(&["one", "two", "three"], 2.0);
    let before = m.duration();
    assert!(m.shorten_gaps(1.0, 0.3));
    assert!((before - m.duration() - 3.4).abs() < 1e-6);
    assert!(m.toast.as_ref().unwrap().text.contains("saving 0:03"));
    m.shorten_gaps(1.0, 0.3);
    assert_eq!(m.toast.as_ref().unwrap().text, "No word gaps longer than that");
}

#[test]
fn layers_default_span_and_editing() {
    let mut m = model(&["one", "two", "three", "four"]);
    // With a selection, a new layer spans the selection.
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "three"), true);
    let c = m.comp().unwrap();
    let (s, e) = (c.time_of_token(m.selection().start), c.time_of_token(m.selection().end));
    let id = m.add_text("Hello").unwrap();
    let l = m.layer().unwrap().clone();
    assert_eq!(l.id, id);
    assert!((l.start - s).abs() < 1e-9 && (l.end() - e).abs() < 1e-9);
    assert_eq!(m.inspector, InspectorTab::Layer);
    assert!(m.edit_layer(|l| l.transform.opacity = 0.5));
    assert_eq!(m.layer().unwrap().transform.opacity, 0.5);
    assert!(m.move_layer(id, -3.0));
    assert_eq!(m.layer().unwrap().start, 0.0, "layers cannot start before zero");
    assert!(m.trim_layer(id, false, 1.0));
    assert!((m.layer().unwrap().duration - (l.duration + 1.0)).abs() < 1e-9);
    assert!(m.edit_layer(|l| l.locked = true));
    assert!(!m.edit_layer(|l| l.transform.x = 0.1), "locked layers refuse edits");
    assert!(!m.move_layer(id, 2.0));
    // New layers stack above existing ones.
    m.clear_selection();
    let id2 = m.add_shape(ShapeKind::Rectangle).unwrap();
    assert!(m.comp().unwrap().layer(id2).unwrap().track > m.comp().unwrap().layer(id).unwrap().track);
    assert!(m.duplicate_layer());
    assert_ne!(m.selected_layer, Some(id2));
    assert!(m.delete_layer());
    assert_eq!(m.comp().unwrap().layers.len(), 2);
}

#[test]
fn media_layer_trim_from_the_left_moves_the_in_point() {
    let mut m = model(&["one", "two"]);
    let asset = m.editor.project.assets[0].id;
    let id = m.add_asset_layer(asset).unwrap();
    m.move_layer(id, 1.0);
    assert!(m.trim_layer(id, true, 0.5));
    let l = m.comp().unwrap().layer(id).unwrap();
    assert!((l.start - 1.5).abs() < 1e-9);
    match &l.content {
        LayerContent::Video(c) => assert!((c.src_in - 0.5).abs() < 1e-9),
        _ => panic!("expected a video layer"),
    }
    // Cannot trim left past the start of the source.
    assert!(m.trim_layer(id, true, -5.0));
    let l = m.comp().unwrap().layer(id).unwrap();
    match &l.content {
        LayerContent::Video(c) => assert!(c.src_in.abs() < 1e-9),
        _ => panic!(),
    }
    assert!((l.start - 1.0).abs() < 1e-9);
}

#[test]
fn full_length_layers_add_and_undo_in_one_step() {
    let mut m = model(&["one", "two", "three"]);
    let id = m.add_progress_bar().unwrap();
    let l = m.comp().unwrap().layer(id).unwrap();
    assert_eq!(l.start, 0.0);
    assert!((l.duration - m.comp().unwrap().script_duration()).abs() < 1e-9);
    assert!(m.undo());
    assert!(m.comp().unwrap().layers.is_empty());
    assert!(m.selected_layer.is_none(), "selection is cleared when its layer disappears");
}

#[test]
fn switching_composition_resets_the_view_without_an_undo_step() {
    let mut m = model(&["one", "two", "three"]);
    let first = m.editor.active.unwrap();
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "three"), true);
    assert!(m.duplicate_to_new_composition("Clip"));
    assert_eq!(words(&m), ["two", "three"]);
    assert_eq!((m.head, m.playhead), (0, 0.0));
    let label = m.editor.undo_label().map(str::to_string);
    assert!(m.select_composition(first));
    assert_eq!(m.editor.undo_label().map(str::to_string), label);
    assert_eq!(words(&m), ["one", "two", "three"]);
    m.clear_selection();
    assert!(!m.duplicate_to_new_composition("Empty"), "needs a selection");
}

#[test]
fn stats_line() {
    let mut m = model(&["one", "two", "three"]);
    assert!(m.stats().starts_with("3 words · 0:0"));
    m.click_token(idx(&m, "two"), false);
    m.click_token(idx(&m, "two"), true);
    assert_eq!(m.stats(), "1 word selected · 0:00");
    assert_eq!(group_thousands(1234567), "1,234,567");
    assert_eq!(format_time(3725.0), "1:02:05");
    assert_eq!(format_time_precise(65.49), "1:05.4");
}

#[test]
fn selection_survives_undo_past_its_tokens() {
    let mut m = model(&["one", "two", "three"]);
    let end = m.comp().unwrap().script.len();
    let extra = m.comp().unwrap().script[0..end].to_vec();
    m.clipboard = extra;
    m.select_range(end..end);
    assert!(m.paste());
    assert!(m.selection().end > end);
    assert!(m.undo());
    assert!(m.selection().end <= m.comp().unwrap().script.len());
    assert!(m.head <= m.comp().unwrap().script.len());
}

#[test]
fn dragging_a_layer_is_one_undo_step() {
    let mut m = model(&["one", "two", "three"]);
    let id = m.add_text("Hi").unwrap();
    let start = m.layer().unwrap().start;
    let steps_before = m.editor.undo_label().map(str::to_string);
    for k in 1..=20 {
        assert!(m.live_edit_layer(id, |l| l.start = start + k as f64 * 0.05));
    }
    assert!(m.is_live_editing());
    assert!((m.layer().unwrap().start - (start + 1.0)).abs() < 1e-9, "the drag is visible while it happens");
    assert_eq!(m.editor.undo_label().map(str::to_string), steps_before, "no undo steps during the drag");
    assert!(m.commit_live_edit());
    assert!(!m.is_live_editing());
    assert_eq!(m.editor.undo_label(), Some("Change layer"));
    assert!(m.undo());
    assert!((m.layer().unwrap().start - start).abs() < 1e-9, "one undo returns to the start of the drag");
    // A drag that ends where it began adds nothing.
    m.live_edit_layer(id, |l| l.start += 1.0);
    m.live_edit_layer(id, |l| l.start -= 1.0);
    assert!(!m.commit_live_edit());
    // Locked layers do not move.
    m.edit_layer(|l| l.locked = true);
    assert!(!m.live_edit_layer(id, |l| l.start = 9.0));
    assert!(!m.commit_live_edit());
}
