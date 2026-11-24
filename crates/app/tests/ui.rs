//! End-to-end UI tests: the real window code, driven by simulated mouse and
//! keyboard in a headless window.

use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, point, px};
use mango_core::*;
use mangoloco::workspace::Workspace;

fn editor(words: &[&str]) -> Editor {
    let mut t = 0.5;
    let mut tw = Vec::new();
    for w in words {
        tw.push(TranscriptWord { text: w.to_string(), start: t, end: t + 0.3, speaker: None, confidence: 1.0 });
        t += 0.5;
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
    e
}

fn open(cx: &mut TestAppContext, e: Editor) -> (AnyWindowHandle, Entity<Workspace>) {
    mangoloco::preview::set_headless(true);
    cx.update(|cx| {
        mangoloco::init(cx);
        mangoloco::open_main_window(e, cx).unwrap()
    })
}

fn idx(ws: &Workspace, w: &str) -> usize {
    ws.model.comp().unwrap().script.iter().position(|t| t.is_word() && t.text == w).unwrap_or_else(|| panic!("no word {w}"))
}

fn words(ws: &Workspace) -> Vec<String> {
    ws.model.comp().unwrap().script.iter().filter(|t| t.shows_text()).map(|t| t.text.clone()).collect()
}

macro_rules! ui {
    ($cx:expr, $win:expr, |$window:ident, $app:ident| $body:block) => {
        $cx.update_window($win, |_, $window, $app| {
            $window.render_frame($app);
            $body
        })
        .unwrap()
    };
}

#[gpui_kit::test]
fn empty_project_invites_import(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, Editor::new_project("Untitled"));
    ui!(cx, win, |window, app| {
        assert!(window.find("script-empty").visible());
        assert!(window.try_find(("tok", 0usize)).is_none());
        // Keys on an empty script do nothing and do not panic.
        window.press("delete", app);
        window.press("backspace", app);
        window.press("space", app);
        window.press("cmd-z", app);
    });
    ws.read_with(cx, |ws, _| {
        assert!(!ws.model.playing, "nothing to play");
        assert!(!ws.model.editor.can_undo());
    });
}

#[gpui_kit::test]
fn click_word_then_delete_and_undo(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    let i = ws.read_with(cx, |ws, _| idx(ws, "two"));
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.head, i);
        assert!((ws.model.playhead - ws.model.comp().unwrap().time_of_token(i)).abs() < 1e-9, "clicking a word moves the playhead to it");
    });
    ui!(cx, win, |window, app| {
        window.press("delete", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(words(ws), ["one", "three", "four"]));
    ui!(cx, win, |window, app| {
        assert!(window.try_find(("tok", ws.read(app).model.comp().unwrap().script.len() + 5)).is_none());
        window.press("cmd-z", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(words(ws), ["one", "two", "three", "four"]));
    ui!(cx, win, |window, app| {
        window.press("cmd-shift-z", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(words(ws), ["one", "three", "four"]));
}

#[gpui_kit::test]
fn shift_click_selects_a_range_and_ignore_strikes_it(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    let (a, b) = ws.read_with(cx, |ws, _| (idx(ws, "two"), idx(ws, "three")));
    ui!(cx, win, |window, app| {
        window.click(("tok", a), app);
        window.click_with_modifiers(("tok", b), Modifiers { shift: true, ..Default::default() }, app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.comp().unwrap().text_of(ws.model.selection()), "two three"));
    let before = ws.read_with(cx, |ws, _| ws.model.duration());
    ui!(cx, win, |window, app| {
        window.press("cmd-backspace", app);
    });
    ws.read_with(cx, |ws, _| {
        let c = ws.model.comp().unwrap();
        assert!(c.script[a].ignored && c.script[b].ignored);
        assert!(ws.model.duration() < before);
        assert_eq!(c.script.iter().filter(|t| t.is_word()).count(), 4, "ignored words stay in the script");
    });
    // The struck words are still on screen and clickable.
    ui!(cx, win, |window, _app| {
        assert!(window.try_find(("tok", a)).is_some());
    });
}

#[gpui_kit::test]
fn dragging_across_words_selects_them(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    let (a, b) = ws.read_with(cx, |ws, _| (idx(ws, "one"), idx(ws, "three")));
    ui!(cx, win, |window, app| {
        let from = window.find(("tok", a)).bounds().center();
        let to = window.find(("tok", b)).bounds().center();
        window.pointer_down(from, MouseButton::Left, app);
        window.pointer_move(to, Some(MouseButton::Left), app);
        window.pointer_up(to, MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.comp().unwrap().text_of(ws.model.selection()), "one two three"));
    // Moving the mouse afterwards without a button held does not change it.
    ui!(cx, win, |window, app| {
        let p = window.find(("tok", a)).bounds().center();
        window.pointer_move(p, None, app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.comp().unwrap().text_of(ws.model.selection()), "one two three"));
}

#[gpui_kit::test]
fn arrows_move_and_shift_extends(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three"]));
    let (a, b, c) = ws.read_with(cx, |ws, _| (idx(ws, "one"), idx(ws, "two"), idx(ws, "three")));
    ui!(cx, win, |window, app| {
        window.click(("tok", a), app);
        window.press("right", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.head, b));
    ui!(cx, win, |window, app| {
        window.press("shift-right", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.selection(), b..c + 1));
    ui!(cx, win, |window, app| {
        window.press("cmd-a", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.selection().len(), ws.model.comp().unwrap().script.len()));
    ui!(cx, win, |window, app| {
        window.press("escape", app);
    });
    ws.read_with(cx, |ws, _| assert!(ws.model.selection().is_empty()));
}

#[gpui_kit::test]
fn correct_through_the_prompt_bar(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["their", "going", "home"]));
    let i = ws.read_with(cx, |ws, _| idx(ws, "their"));
    let dur = ws.read_with(cx, |ws, _| ws.model.duration());
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
        window.press("c", app);
        assert!(window.try_find("prompt-bar").is_some(), "C opens the correction bar");
        assert_eq!(window.find("prompt-input").value(), Some("their"), "pre-filled with the current text");
        window.press("cmd-a", app);
        window.input("they're", app);
        window.press("enter", app);
    });
    // The input reports Enter as an event, delivered once the current update ends.
    ui!(cx, win, |window, _app| {
        assert!(window.try_find("prompt-bar").is_none(), "Enter applies and closes");
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(words(ws), ["they're", "going", "home"]);
        assert!((ws.model.duration() - dur).abs() < 1e-9, "correcting never changes the media");
    });
    // Typing letters in the prompt must not trigger single-key shortcuts.
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
        window.press("c", app);
        window.press("cmd-a", app);
        window.input("s c /", app);
        window.press("escape", app);
    });
    ui!(cx, win, |window, _app| {
        assert!(window.try_find("prompt-bar").is_none(), "Escape cancels");
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(words(ws), ["they're", "going", "home"]);
        assert_eq!(ws.model.comp().unwrap().scene_spans().len(), 1, "no scene was split while typing");
    });
}

#[gpui_kit::test]
fn slash_makes_a_scene_and_hash_names_a_marker(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three"]));
    let i = ws.read_with(cx, |ws, _| idx(ws, "two"));
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
        window.press("/", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.comp().unwrap().scene_spans().len(), 2));
    ui!(cx, win, |window, app| {
        window.press("#", app);
        window.input("Chapter two", app);
        window.press("enter", app);
    });
    ws.read_with(cx, |ws, _| {
        let m = ws.model.comp().unwrap().script_markers();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].2, "Chapter two");
        assert_eq!(m[0].0, i);
    });
    ui!(cx, win, |window, _app| {
        assert!(window.find_all(("marker", 1usize)).len() + window.find_all(("marker", 0usize)).len() >= 1, "the marker heading is drawn");
    });
}

#[gpui_kit::test]
fn find_bar_searches_and_removes(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["the", "cat", "and", "the", "hat"]));
    ui!(cx, win, |window, app| {
        window.press("cmd-f", app);
        assert!(window.try_find("find-bar").is_some());
        window.input("the", app);
    });
    ws.read_with(cx, |ws, _| {
        let f = ws.model.find.as_ref().unwrap();
        assert_eq!((f.hits.len(), f.current), (2, 0));
    });
    ui!(cx, win, |window, app| {
        window.press("enter", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.find.as_ref().unwrap().current, 1));
    ui!(cx, win, |window, app| {
        window.click("find-delete", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(words(ws), ["cat", "and", "hat"]));
    ui!(cx, win, |window, app| {
        window.click("find-close", app);
        assert!(window.try_find("find-bar").is_none());
    });
}

#[gpui_kit::test]
fn space_plays_and_pauses(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three"]));
    let i = ws.read_with(cx, |ws, _| idx(ws, "one"));
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
        window.press("space", app);
    });
    ws.read_with(cx, |ws, _| assert!(ws.model.playing));
    ui!(cx, win, |window, app| {
        window.click("tl-play", app);
    });
    ws.read_with(cx, |ws, _| assert!(!ws.model.playing, "the transport button pauses"));
}

#[gpui_kit::test]
fn clicking_the_timeline_seeks(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    ui!(cx, win, |window, app| {
        let b = window.find("timeline").bounds();
        // 80 px per second by default, no scroll: 160 px in is 2.0 s.
        window.pointer_down(point(b.origin.x + px(160.), b.origin.y + px(8.)), MouseButton::Left, app);
        window.pointer_up(point(b.origin.x + px(160.), b.origin.y + px(8.)), MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| {
        assert!((ws.model.playhead - 2.0).abs() < 0.02, "playhead {}", ws.model.playhead);
        assert_eq!(Some(ws.model.head), ws.model.playing_token(), "the script caret follows the playhead");
    });
    // Scrubbing: drag along the ruler.
    ui!(cx, win, |window, app| {
        let b = window.find("timeline").bounds();
        let y = b.origin.y + px(8.);
        window.pointer_down(point(b.origin.x + px(80.), y), MouseButton::Left, app);
        window.pointer_move(point(b.origin.x + px(120.), y), Some(MouseButton::Left), app);
        window.pointer_up(point(b.origin.x + px(120.), y), MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| assert!((ws.model.playhead - 1.5).abs() < 0.02, "playhead {}", ws.model.playhead));
    // Past the end clamps.
    ui!(cx, win, |window, app| {
        let b = window.find("timeline").bounds();
        let p = point(b.origin.x + b.size.width - px(4.), b.origin.y + px(8.));
        window.pointer_down(p, MouseButton::Left, app);
        window.pointer_up(p, MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| assert!(ws.model.playhead <= ws.model.duration() + 1e-9));
}

#[gpui_kit::test]
fn dragging_a_layer_on_the_timeline_moves_it_as_one_undo_step(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    ws.update(cx, |ws, cx| {
        ws.model.seek(0.5);
        ws.model.add_text("Title");
        ws.model.edit_layer(|l| {
            l.start = 0.5;
            l.duration = 1.0;
        });
        cx.notify();
    });
    let id = ws.read_with(cx, |ws, _| ws.model.selected_layer.unwrap());
    ui!(cx, win, |window, app| {
        let b = window.find("timeline").bounds();
        // One lane, directly under the 22 px ruler. Grab the middle of the layer (1.0 s = 80 px).
        let y = b.origin.y + px(22. + 13.);
        window.pointer_down(point(b.origin.x + px(80.), y), MouseButton::Left, app);
        window.pointer_move(point(b.origin.x + px(120.), y), Some(MouseButton::Left), app);
        window.pointer_move(point(b.origin.x + px(160.), y), Some(MouseButton::Left), app);
        window.pointer_up(point(b.origin.x + px(160.), y), MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| {
        let l = ws.model.comp().unwrap().layer(id).unwrap();
        assert!((l.start - 1.5).abs() < 0.03, "start {}", l.start);
        assert!((l.duration - 1.0).abs() < 1e-9, "moving does not resize");
    });
    ui!(cx, win, |window, app| {
        window.press("cmd-z", app);
    });
    ws.read_with(cx, |ws, _| assert!((ws.model.comp().unwrap().layer(id).unwrap().start - 0.5).abs() < 1e-9, "one undo for the whole drag"));
    // Dragging the right edge trims.
    ui!(cx, win, |window, app| {
        let b = window.find("timeline").bounds();
        let y = b.origin.y + px(22. + 13.);
        window.pointer_down(point(b.origin.x + px(120.), y), MouseButton::Left, app);
        window.pointer_move(point(b.origin.x + px(200.), y), Some(MouseButton::Left), app);
        window.pointer_up(point(b.origin.x + px(200.), y), MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| {
        let l = ws.model.comp().unwrap().layer(id).unwrap();
        assert!((l.start - 0.5).abs() < 1e-9);
        assert!((l.duration - 2.0).abs() < 0.03, "duration {}", l.duration);
    });
}

#[gpui_kit::test]
fn dragging_on_the_canvas_moves_the_layer(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three", "four"]));
    ws.update(cx, |ws, cx| {
        ws.model.seek(0.6);
        ws.model.add_shape(ShapeKind::Rectangle);
        ws.model.selected_layer = None;
        cx.notify();
    });
    let id = ws.read_with(cx, |ws, _| ws.model.comp().unwrap().layers[0].id);
    ui!(cx, win, |window, app| {
        let c = window.find("canvas").bounds().center();
        // The rectangle is centred on a canvas whose frame is centred in the pane.
        window.pointer_down(c, MouseButton::Left, app);
        window.pointer_move(point(c.x + px(40.), c.y + px(20.)), Some(MouseButton::Left), app);
        window.pointer_up(point(c.x + px(40.), c.y + px(20.)), MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.selected_layer, Some(id), "clicking a layer selects it");
        let l = ws.model.comp().unwrap().layer(id).unwrap();
        assert!(l.transform.x > 0.52 && l.transform.y > 0.52, "moved right and down: {:?}", l.transform);
        assert_eq!(ws.model.editor.undo_label(), Some("Change layer"));
    });
    // Clicking empty canvas deselects.
    ui!(cx, win, |window, app| {
        let b = window.find("canvas").bounds();
        let p = point(b.origin.x + px(20.), b.origin.y + b.size.height / 2.);
        window.pointer_down(p, MouseButton::Left, app);
        window.pointer_up(p, MouseButton::Left, app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.selected_layer, None));
}

#[gpui_kit::test]
fn inspector_changes_the_canvas_and_captions(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two"]));
    ui!(cx, win, |window, app| {
        // Aspect chips: 16:9, 9:16, 1:1, 4:5, 4:3.
        window.click(("aspect", 1usize), app);
    });
    ws.read_with(cx, |ws, _| {
        let c = ws.model.comp().unwrap();
        assert_eq!((c.width, c.height), (1080, 1920));
    });
    ui!(cx, win, |window, app| {
        window.click(("insp-tab", 2usize), app);
        window.click("cap-on", app);
        window.click(("cap-mode", 1usize), app);
        window.click(("cap-size", 1usize), app);
    });
    ws.read_with(cx, |ws, _| {
        let cap = &ws.model.comp().unwrap().captions;
        assert!(cap.enabled);
        assert_eq!(cap.highlight, CaptionHighlight::ActiveWord);
        assert!((cap.size - 0.06).abs() < 1e-9, "size {}", cap.size);
    });
}

#[gpui_kit::test]
fn composition_tabs_and_new_composition(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two", "three"]));
    let first = ws.read_with(cx, |ws, _| ws.model.editor.active.unwrap());
    ui!(cx, win, |window, app| {
        window.click("new-comp", app);
        window.press("cmd-a", app);
        window.input("Shorts", app);
        window.press("enter", app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.editor.project.compositions.len(), 2);
        assert_eq!(ws.model.comp().unwrap().name, "Shorts");
        assert!(ws.model.comp().unwrap().script.is_empty());
    });
    ui!(cx, win, |window, app| {
        assert!(window.try_find("script-empty").is_some(), "the new composition is empty");
        window.click(("comp-tab", 0usize), app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.editor.active, Some(first));
        assert_eq!(words(ws), ["one", "two", "three"]);
    });
}

#[gpui_kit::test]
fn sidebar_tools_remove_fillers(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["so", "um", "this", "uh", "works"]));
    ui!(cx, win, |window, app| {
        window.click(("side-tab", 1usize), app);
        window.click("fill-del", app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(words(ws), ["so", "this", "works"]);
        assert_eq!(ws.model.toast.as_ref().unwrap().text, "Removed 2 filler words");
    });
    ui!(cx, win, |window, _app| {
        assert!(window.try_find("toast").is_some(), "the result is reported in the status bar");
    });
}

#[gpui_kit::test]
fn long_script_stays_usable(cx: &mut TestAppContext) {
    // Roughly an hour of speech.
    let many: Vec<String> = (0..9000).map(|i| if i % 17 == 16 { format!("w{i}.") } else { format!("w{i}") }).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let (win, ws) = open(cx, editor(&refs));
    let t0 = std::time::Instant::now();
    ui!(cx, win, |window, app| {
        window.render_frame(app);
        window.render_frame(app);
    });
    let per_frame = t0.elapsed() / 3;
    assert!(per_frame.as_millis() < 250, "a frame of a 9,000-word script took {per_frame:?}");
    // The last word is far off screen and not built; jumping there reveals it.
    let last = ws.read_with(cx, |ws, _| idx(ws, "w8999"));
    ui!(cx, win, |window, _app| {
        assert!(window.try_find(("tok", last)).is_none(), "off-screen words are virtualised");
    });
    ws.update(cx, |ws, cx| {
        ws.model.click_token(last, false);
        ws.reveal_caret();
        cx.notify();
    });
    ui!(cx, win, |window, _app| {
        assert!(window.try_find(("tok", last)).is_some(), "the caret's paragraph scrolled into view");
    });
}

#[gpui_kit::test]
fn duplicate_and_delete_composition_from_the_top_bar(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two"]));
    ui!(cx, win, |window, app| {
        assert_eq!(window.find("del-comp").disabled(), Some(true), "the only composition cannot be deleted");
        window.click("dup-comp", app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.editor.project.compositions.len(), 2);
        assert_eq!(ws.model.comp().unwrap().name, "Composition 1 (Copy)");
        assert_eq!(words(ws), ["one", "two"]);
    });
    ui!(cx, win, |window, app| {
        window.click("del-comp", app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.editor.project.compositions.len(), 1);
        assert_eq!(ws.model.comp().unwrap().name, "Composition 1");
    });
    ui!(cx, win, |window, app| {
        window.press("cmd-z", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.editor.project.compositions.len(), 2, "deleting a composition can be undone"));
}

#[gpui_kit::test]
fn transcript_tools_correct_everywhere_hide_text_and_glossary(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["mangoloco", "is", "great", "mangoloco", "rocks"]));
    let dur = ws.read_with(cx, |ws, _| ws.model.duration());
    ui!(cx, win, |window, app| {
        window.click(("side-tab", 1usize), app);
        window.click("correct-all", app);
        window.press("cmd-a", app);
        window.input("mangoloco => Mango Loco", app);
        window.press("enter", app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(words(ws), ["Mango Loco", "is", "great", "Mango Loco", "rocks"]));
    // A malformed correction keeps the prompt open and changes nothing.
    ui!(cx, win, |window, app| {
        window.click("correct-all", app);
        window.press("cmd-a", app);
        window.input("no arrow here", app);
        window.press("enter", app);
    });
    ui!(cx, win, |window, app| {
        assert!(window.try_find("prompt-bar").is_some(), "the prompt stays open for a fix");
        window.press("escape", app);
    });
    ws.read_with(cx, |ws, _| assert!(ws.model.toast.as_ref().unwrap().error));
    // Hide text: media stays, words leave the transcript.
    let i = ws.read_with(cx, |ws, _| idx(ws, "great"));
    ui!(cx, win, |window, app| {
        window.click(("tok", i), app);
        window.click("tb-hide", app);
    });
    ws.read_with(cx, |ws, _| {
        assert_eq!(words(ws), ["Mango Loco", "is", "Mango Loco", "rocks"]);
        assert!((ws.model.duration() - dur).abs() < 1e-9);
    });
    ui!(cx, win, |window, app| {
        assert!(window.try_find(("tok", i)).is_none(), "hidden words are not drawn");
        window.click("glossary", app);
        window.input("Mango Loco, GPUI", app);
        window.press("enter", app);
    });
    ws.read_with(cx, |ws, _| {
        let terms: Vec<&str> = ws.model.editor.project.glossary.iter().map(|g| g.term.as_str()).collect();
        assert_eq!(terms, ["Mango Loco", "GPUI"]);
    });
}

#[gpui_kit::test]
fn export_panel_opens_picks_a_format_and_closes(cx: &mut TestAppContext) {
    let (win, ws) = open(cx, editor(&["one", "two"]));
    ui!(cx, win, |window, app| {
        window.click("export", app);
    });
    ui!(cx, win, |window, app| {
        assert!(window.try_find("export-panel").is_some());
        // MEDIA order: MP4, HEVC, ProRes, WebM, GIF, MP3, WAV, M4A.
        window.click(("exp-kind", 5usize), app);
    });
    ws.read_with(cx, |ws, _| assert_eq!(ws.export_panel.as_ref().unwrap().kind, mangoloco::services::ExportKind::Mp3));
    ui!(cx, win, |window, app| {
        window.click("export-cancel", app);
        assert!(window.try_find("export-panel").is_none());
    });
    // With nothing to export, the panel does not open and says why.
    let (win2, ws2) = open(cx, Editor::new_project("Empty"));
    ui!(cx, win2, |window, app| {
        window.click("export", app);
    });
    ui!(cx, win2, |window, _app| {
        assert!(window.try_find("export-panel").is_none());
    });
    ws2.read_with(cx, |ws, _| assert!(ws.model.toast.as_ref().is_some_and(|t| t.error && t.text.contains("nothing to export"))));
}
