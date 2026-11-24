//! Whole-app end-to-end test on real media: import a recording through the
//! window, let it transcribe, edit the text, export, and check the file that
//! comes out.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use mango_core::*;
use mangoloco::services::{ExportChoice, ExportKind};
use mangoloco::workspace::Workspace;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn run(cmd: &mut Proc) -> String {
    let out = cmd.output().expect("tool runs");
    assert!(out.status.success(), "{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn tool(name: &str) -> PathBuf {
    mango_core::tools::find_tool(name).unwrap_or_else(|| panic!("{name} is required for this test"))
}

fn duration_of(path: &Path) -> f64 {
    run(Proc::new(tool("ffprobe")).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path)).trim().parse().unwrap()
}

/// Advances the app (timers, finished jobs) until `done` or the deadline.
fn pump(cx: &mut TestAppContext, ws: &Entity<Workspace>, what: &str, secs: u64, mut done: impl FnMut(&Workspace) -> bool) {
    let start = Instant::now();
    loop {
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        if ws.read_with(cx, |ws, _| done(ws)) {
            return;
        }
        if start.elapsed() > Duration::from_secs(secs) {
            let (toast, jobs) = ws.read_with(cx, |ws, _| (ws.model.toast.clone(), ws.jobs.list.iter().map(|j| format!("{} {:.0}% {}", j.label, j.shared.progress() * 100.0, j.shared.note())).collect::<Vec<_>>()));
            panic!("timed out waiting for {what}; toast={toast:?} jobs={jobs:?}");
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn spoken_words(ws: &Workspace) -> Vec<String> {
    ws.model.comp().unwrap().script.iter().filter(|t| t.shows_text()).map(|t| mango_core::script::normalize_word(&t.text)).collect()
}

fn in_window<R>(cx: &mut TestAppContext, win: AnyWindowHandle, ws: &Entity<Workspace>, f: impl FnOnce(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>) -> R) -> R {
    cx.update_window(win, |_, window, app| ws.update(app, |ws, cx| f(ws, window, cx))).unwrap()
}

#[gpui_kit::test]
fn import_transcribe_edit_export(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    // Keep the test away from the user's settings, caches and models.
    unsafe {
        std::env::set_var("MANGO_DATA_DIR", dir.path().join("data"));
        std::env::set_var("MANGO_CACHE_DIR", dir.path().join("cache"));
        std::env::set_var("MANGO_MODELS_DIR", repo().join("models"));
    }
    assert!(repo().join("models/ggml-base.en.bin").is_file(), "the base.en speech model must be in models/");

    // A real recording: synthesised speech over generated video.
    let aiff = dir.path().join("speech.aiff");
    run(Proc::new("/usr/bin/say").args(["-v", "Samantha", "-o"]).arg(&aiff).arg("Hello there. Um, this is a test of the mango editor. The weather is lovely today. Goodbye everyone."));
    let clip = dir.path().join("talk.mp4");
    run(Proc::new(tool("ffmpeg"))
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-i"])
        .arg(&aiff)
        .args(["-shortest", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ar", "48000"])
        .arg(&clip));
    let source_len = duration_of(&clip);

    mangoloco::preview::set_headless(true);
    let (win, ws) = cx.update(|cx| {
        mangoloco::init(cx);
        mangoloco::open_main_window(Editor::new_project("E2E"), cx).unwrap()
    });

    // Import through the same path as the menu and drag-and-drop.
    in_window(cx, win, &ws, |ws, window, cx| ws.import_paths(vec![clip.clone()], window, cx));
    pump(cx, &ws, "import and transcription", 180, |ws| ws.jobs.is_empty() && ws.model.editor.project.assets.first().is_some_and(|a| a.transcript.is_some()));

    let words = ws.read_with(cx, |ws, _| spoken_words(ws));
    let said = words.join(" ");
    for expected in ["hello", "test", "mango", "editor", "weather", "lovely", "goodbye"] {
        assert!(words.iter().any(|w| w == expected), "expected to hear {expected:?} in: {said}");
    }
    ws.read_with(cx, |ws, _| {
        let c = ws.model.comp().unwrap();
        assert!((c.script_duration() - source_len).abs() < 0.12, "the script covers the whole recording: {} vs {source_len}", c.script_duration());
        assert_eq!((c.width, c.height), (640, 360), "the canvas adopted the recording's size");
        let a = &ws.model.editor.project.assets[0];
        assert!(a.has_video && a.has_audio && (a.fps - 30.0).abs() < 0.01);
    });

    // Edit by text: cut "The weather is lovely today." by selecting it in the window.
    let (a, b) = ws.read_with(cx, |ws, _| {
        let s = &ws.model.comp().unwrap().script;
        let find = |w: &str| s.iter().position(|t| t.is_word() && mango_core::script::normalize_word(&t.text) == w).unwrap();
        let weather = find("weather");
        let the = (0..weather).rev().find(|&i| s[i].is_word()).unwrap();
        (the, find("today"))
    });
    ws.read_with(cx, |ws, _| assert_eq!(mango_core::script::normalize_word(&ws.model.comp().unwrap().script[a].text), "the"));
    cx.update_window(win, |_, window, app| {
        window.render_frame(app);
        window.click(("tok", a), app);
        window.click_with_modifiers(("tok", b), gpui_kit::Modifiers { shift: true, ..Default::default() }, app);
        window.press("delete", app);
    })
    .unwrap();
    let before_fillers = ws.read_with(cx, |ws, _| ws.model.duration());
    // And remove the "um" with the clean-up tool.
    cx.update_window(win, |_, window, app| {
        window.render_frame(app);
        window.click(("side-tab", 1usize), app);
        window.click("fill-del", app);
    })
    .unwrap();
    let (edited_len, words) = ws.read_with(cx, |ws, _| (ws.model.duration(), spoken_words(ws)));
    assert!(!words.iter().any(|w| w == "weather" || w == "lovely" || w == "um"), "{words:?}");
    assert!(words.iter().any(|w| w == "goodbye") && words.iter().any(|w| w == "mango"));
    assert!(edited_len < before_fillers && before_fillers < source_len - 1.0, "cutting a sentence removed real time: {source_len} -> {before_fillers} -> {edited_len}");

    // Turn on captions and add a title so the export exercises overlays.
    in_window(cx, win, &ws, |ws, _, cx| {
        let mut cap = ws.model.comp().unwrap().captions.clone();
        cap.enabled = true;
        assert!(ws.model.run(Command::SetCaptions { captions: cap }));
        ws.model.seek(0.0);
        ws.model.clear_selection();
        assert!(ws.model.add_text("Mango Loco").is_some());
        cx.notify();
    });

    // Export a video.
    let out = dir.path().join("out.mp4");
    in_window(cx, win, &ws, |ws, _, cx| ws.export_to(out.clone(), ExportChoice { kind: ExportKind::Mp4, ..Default::default() }, cx));
    pump(cx, &ws, "video export", 240, |ws| ws.jobs.is_empty() && ws.last_export.is_some());
    assert!(out.is_file(), "toast: {:?}", ws.read_with(cx, |ws, _| ws.model.toast.clone()));
    let out_len = duration_of(&out);
    assert!((out_len - edited_len).abs() < 0.1, "export length {out_len} should match the edit {edited_len}");
    let streams = run(Proc::new(tool("ffprobe")).args(["-v", "error", "-show_entries", "stream=codec_name,width,height", "-of", "csv=p=0"]).arg(&out));
    assert!(streams.contains("h264,640,360") && streams.contains("aac"), "{streams}");

    // The exported sound really has the sentence and the filler cut: listen to it.
    let model = repo().join("models/ggml-base.en.bin");
    let heard = mango_speech::transcribe::transcribe(&out, &mango_speech::transcribe::Options::new(model), &mut |_| {}, &Default::default()).unwrap();
    let heard: Vec<String> = heard.words.iter().map(|w| mango_core::script::normalize_word(&w.text)).collect();
    let heard_text = heard.join(" ");
    for gone in ["weather", "lovely", "today", "um"] {
        assert!(!heard.iter().any(|w| w == gone), "{gone:?} should have been cut, but the export says: {heard_text}");
    }
    for kept in ["hello", "mango", "editor", "goodbye"] {
        assert!(heard.iter().any(|w| w == kept), "{kept:?} should still be there: {heard_text}");
    }

    // Text exports.
    let srt = dir.path().join("out.srt");
    in_window(cx, win, &ws, |ws, _, cx| ws.export_to(srt.clone(), ExportChoice { kind: ExportKind::Srt, ..Default::default() }, cx));
    let srt_text = std::fs::read_to_string(&srt).unwrap();
    assert!(srt_text.starts_with("1\n00:00:0"), "{srt_text}");
    assert!(srt_text.to_lowercase().contains("goodbye") && !srt_text.to_lowercase().contains("weather"), "{srt_text}");

    // Audio-only export of just a selection.
    let (s, e) = ws.read_with(cx, |ws, _| {
        let sc = &ws.model.comp().unwrap().script;
        let find = |w: &str| sc.iter().position(|t| t.is_word() && mango_core::script::normalize_word(&t.text) == w).unwrap();
        (find("this"), find("editor") + 1)
    });
    let mp3 = dir.path().join("bit.mp3");
    let want = in_window(cx, win, &ws, |ws, _, cx| {
        ws.last_export = None;
        ws.model.select_range(s..e);
        let c = ws.model.comp().unwrap();
        let want = c.time_of_token(e) - c.time_of_token(s);
        ws.export_to(mp3.clone(), ExportChoice { kind: ExportKind::Mp3, selection_only: true, ..Default::default() }, cx);
        want
    });
    pump(cx, &ws, "audio export", 120, |ws| ws.jobs.is_empty() && ws.last_export.is_some());
    let got = duration_of(&mp3);
    assert!((got - want).abs() < 0.15, "selection export is {got}s, selection is {want}s");

    // Save, reopen, and the edit is still there.
    let project = dir.path().join("e2e.mango");
    in_window(cx, win, &ws, |ws, _, _| ws.model.editor.save(&project).unwrap());
    let reopened = Editor::load(&project).unwrap();
    assert_eq!(reopened.project, ws.read_with(cx, |ws, _| ws.model.editor.project.clone()));

    // Leave no background work behind (the speech engine must be idle at exit).
    ws.update(cx, |ws, _| ws.jobs.shutdown());
}

fn api_key() -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    let env = std::fs::read_to_string(repo().join(".env")).ok()?;
    env.lines().find_map(|l| l.strip_prefix("ANTHROPIC_API_KEY=")).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

fn talk_editor() -> Editor {
    let text = "Welcome back to the show. Um, today we are talking about, uh, editing video by editing text. \
You record something and it gets transcribed. Then you delete the words you do not want. \
Our sponsor this week is Acme Rockets. Acme Rockets make the fastest rockets you can buy. Use the code MANGO for ten percent off. \
Now, back to editing. Um, the timeline is still there when you need it.";
    let mut words = Vec::new();
    let mut t = 0.4;
    for w in text.split_whitespace() {
        let d = 0.1 + 0.04 * w.len() as f64;
        words.push(TranscriptWord { text: w.to_string(), start: t, end: t + d, speaker: None, confidence: 1.0 });
        t += d + if w.ends_with('.') { 0.5 } else { 0.05 };
    }
    let asset = MediaAsset {
        id: AssetId::new(),
        name: "talk.mp4".into(),
        path: "/nonexistent/talk.mp4".into(),
        kind: MediaKind::Video,
        duration: t + 0.5,
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: false,
        has_video: false,
        sample_rate: 48000,
        channels: 2,
        folder: None,
        transcript: Some(Transcript { language: "en".into(), words }),
    };
    let id = asset.id;
    let mut e = Editor::new_project("Assistant");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    e
}

/// The assistant, driven through the app: a request edits the project as one
/// undo step, and a question answers without editing. Uses the real API.
#[gpui_kit::test]
fn assistant_edits_through_the_app(cx: &mut TestAppContext) {
    let Some(key) = api_key() else {
        eprintln!("skipped: no ANTHROPIC_API_KEY in the environment or .env");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MANGO_DATA_DIR", dir.path().join("data"));
        std::env::set_var("MANGO_CACHE_DIR", dir.path().join("cache"));
    }
    mangoloco::preview::set_headless(true);
    let (win, ws) = cx.update(|cx| {
        mangoloco::init(cx);
        mangoloco::open_main_window(talk_editor(), cx).unwrap()
    });
    in_window(cx, win, &ws, |ws, _, _| {
        ws.settings.api_key = key.clone();
        // The cheaper model keeps this test quick; the default is covered by the assistant crate's own live suite.
        ws.settings.assistant_model = mango_ai::FAST_MODEL.to_string();
    });
    let before = ws.read_with(cx, |ws, _| (ws.model.editor.project.clone(), ws.model.editor.undo_depth(), ws.model.duration()));
    assert!(!mango_core::detect::find_fillers(&before.0.compositions[0], &Default::default()).is_empty());

    // 1. An edit request.
    assert!(in_window(cx, win, &ws, |ws, _, cx| ws.send_chat_text("Remove the filler words, and cut the whole sponsor read about Acme Rockets.", cx)));
    assert!(ws.read_with(cx, |ws, _| ws.ai.busy));
    pump(cx, &ws, "the assistant's edit", 180, |ws| !ws.ai.busy && ws.jobs.is_empty());
    ws.read_with(cx, |ws, _| {
        let c = ws.model.comp().unwrap();
        let playing: Vec<String> = c.script.iter().filter(|t| t.shows_text()).map(|t| mango_core::script::normalize_word(&t.text)).collect();
        let text = playing.join(" ");
        assert!(mango_core::detect::find_fillers(c, &Default::default()).is_empty(), "fillers remain: {text}");
        assert!(!playing.iter().any(|w| w == "acme" || w == "rockets" || w == "sponsor"), "sponsor read remains: {text}");
        for kept in ["welcome", "transcribed", "timeline"] {
            assert!(playing.iter().any(|w| w == kept), "{kept} was lost: {text}");
        }
        assert!(ws.model.duration() < before.2 - 3.0, "the cut removed real time");
        assert_eq!(ws.model.editor.undo_depth(), before.1 + 1, "the whole response is one undo step");
        assert_eq!(ws.ai.chat.len(), 2, "the conversation shows the request and the reply");
        assert!(!ws.ai.chat[1].text.trim().is_empty());
        assert!(ws.model.toast.as_ref().is_some_and(|t| !t.error), "{:?}", ws.model.toast);
    });
    // One undo restores everything.
    cx.update_window(win, |_, window, app| {
        window.render_frame(app);
        window.click("undo", app);
    })
    .unwrap();
    ws.read_with(cx, |ws, _| assert_eq!(ws.model.editor.project, before.0));

    // 2. A question changes nothing.
    let depth = ws.read_with(cx, |ws, _| ws.model.editor.undo_depth());
    assert!(in_window(cx, win, &ws, |ws, _, cx| ws.send_chat_text("How many times is Acme Rockets mentioned? Answer with a number.", cx)));
    pump(cx, &ws, "the assistant's answer", 120, |ws| !ws.ai.busy && ws.jobs.is_empty());
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.model.editor.project, before.0, "a question must not edit");
        assert_eq!(ws.model.editor.undo_depth(), depth);
        let reply = ws.ai.chat.last().unwrap().text.to_lowercase();
        assert!(reply.contains('2') || reply.contains("two") || reply.contains("twice"), "reply: {reply}");
    });

    // 3. A one-shot tool: chapters land as markers in one undo step.
    assert!(in_window(cx, win, &ws, |ws, _, cx| ws.run_ai_tool("Summary", cx, |c, e| Ok(mangoloco::services_ai::AiOutcome::Text { title: "Summary".into(), text: mango_ai::tools::summarize(c, e)? }))));
    pump(cx, &ws, "the summary", 120, |ws| ws.jobs.is_empty());
    ws.read_with(cx, |ws, _| {
        let (title, text) = ws.ai.result.clone().expect("a summary to show");
        assert_eq!(title, "Summary");
        assert!(text.to_lowercase().contains("edit"), "summary: {text}");
        assert_eq!(ws.model.editor.project, before.0, "writing tools do not edit");
    });

    // 4. A bad key is reported plainly and changes nothing.
    in_window(cx, win, &ws, |ws, _, _| {
        ws.settings.api_key = "sk-ant-invalid".into();
        ws.ai.assistant = None;
    });
    assert!(in_window(cx, win, &ws, |ws, _, cx| ws.send_chat_text("Remove the filler words.", cx)));
    pump(cx, &ws, "the rejected key", 60, |ws| !ws.ai.busy && ws.jobs.is_empty());
    ws.read_with(cx, |ws, _| {
        let toast = ws.model.toast.clone().unwrap();
        assert!(toast.error && toast.text.contains("API key"), "{toast:?}");
        assert_eq!(ws.model.editor.project, before.0);
    });
    ws.update(cx, |ws, _| ws.jobs.shutdown());
}

/// Playback through the app on real media: the picture is decoded, the clock
/// runs, deleted media is skipped, and playback stops at the end.
#[gpui_kit::test]
fn playback_follows_the_edit(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("MANGO_DATA_DIR", dir.path().join("data"));
        std::env::set_var("MANGO_CACHE_DIR", dir.path().join("cache"));
    }
    // Four seconds of video with a tone, and a made-up transcript of one word per second.
    let clip = dir.path().join("clip.mp4");
    run(Proc::new(tool("ffmpeg"))
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30:duration=4", "-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "15", "-c:a", "aac", "-ar", "48000"])
        .arg(&clip));
    let mut asset = mango_media::probe::probe(&clip).unwrap();
    asset.transcript = Some(Transcript {
        language: "en".into(),
        words: ["zero", "one", "two", "three"].iter().enumerate().map(|(i, w)| TranscriptWord { text: w.to_string(), start: i as f64, end: i as f64 + 1.0, speaker: None, confidence: 1.0 }).collect(),
    });
    let id = asset.id;
    let mut e = Editor::new_project("Playback");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    // Cut "one" (source second 1 to 2).
    let i = e.comp().unwrap().script.iter().position(|t| t.text == "one").unwrap();
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();

    mangoloco::preview::set_headless(true);
    let (win, ws) = cx.update(|cx| {
        mangoloco::init(cx);
        mangoloco::open_main_window(e, cx).unwrap()
    });
    // Audio is prepared in the background and a first picture arrives while paused.
    pump(cx, &ws, "audio preparation and the first picture", 60, |ws| ws.jobs.is_empty() && ws.canvas_debug().1);
    let total = ws.read_with(cx, |ws, _| ws.model.duration());
    assert!((total - 3.0).abs() < 0.05, "one second was cut: {total}");

    // Play from the top with the space bar.
    cx.update_window(win, |_, window, app| {
        window.render_frame(app);
        let first = ws.read(app).model.comp().unwrap().script.iter().position(|t| t.is_word()).unwrap();
        window.click(("tok", first), app);
        window.press("space", app);
    })
    .unwrap();
    assert!(ws.read_with(cx, |ws, _| ws.model.playing));
    // The simulated clock advances 20 ms per pump step; watch the playhead cross the cut.
    let mut seen_after_cut = false;
    let mut last = 0.0;
    pump(cx, &ws, "playback to reach the end", 60, |ws| {
        assert!(ws.model.playhead + 1e-6 >= last || !ws.model.playing, "the playhead never runs backwards while playing");
        last = ws.model.playhead;
        if ws.model.playhead > 1.2 && ws.model.playing {
            // Past the cut, the script position is in source second 2 or later: "one" was skipped.
            let (_, src) = ws.model.comp().unwrap().source_at(ws.model.playhead).unwrap();
            assert!(src >= 2.0, "at {:.2}s the source should be past the cut, got {src:.2}", ws.model.playhead);
            seen_after_cut = true;
        }
        !ws.model.playing
    });
    assert!(seen_after_cut);
    ws.read_with(cx, |ws, _| {
        assert!((ws.model.playhead - total).abs() < 0.05, "playback stops at the end: {} of {total}", ws.model.playhead);
        assert!(ws.canvas_debug().1, "a picture is on the canvas");
        assert!(ws.model.toast.as_ref().is_none_or(|t| !t.error), "{:?}", ws.model.toast);
    });
    // Space at the end starts again from the top.
    cx.update_window(win, |_, window, app| {
        window.render_frame(app);
        window.press("space", app);
    })
    .unwrap();
    pump(cx, &ws, "restart", 20, |ws| ws.model.playing && ws.model.playhead < 1.0);
    cx.update_window(win, |_, window, app| window.press("space", app)).unwrap();
    ws.update(cx, |ws, _| ws.jobs.shutdown());
}
