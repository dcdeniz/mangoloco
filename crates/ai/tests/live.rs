//! Live tests against the real Claude API.
//!
//! They run whenever a key is available (`ANTHROPIC_API_KEY`, or a `.env`
//! file above the crate) and are skipped, with a note on stderr, when there
//! is none. They assert outcomes on the editor, not the model's wording.
//!
//! The model defaults to the cheapest one that passes this suite; override
//! with `MANGO_AI_TEST_MODEL=<model id>` to test another (for example the
//! shipped default).
mod common;

use common::*;
use mango_ai::context;
use mango_ai::tools::{self, Intensity, Platform};
use mango_ai::{Assistant, Client, Event, TurnResult};
use mango_core::detect::{self, FillerOptions};
use mango_core::*;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn model() -> String {
    std::env::var("MANGO_AI_TEST_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "claude-haiku-5-5".to_string())
}

fn client() -> Option<Client> {
    match api_key() {
        Some(k) => Some(Client::new(k).with_model(&model())),
        None => {
            eprintln!("SKIPPED: no ANTHROPIC_API_KEY in the environment or a .env file");
            None
        }
    }
}

/// Runs one assistant turn, logging what happened (never the key).
fn turn(a: &mut Assistant, e: &mut Editor, msg: &str) -> TurnResult {
    let t = Instant::now();
    let mut log = Vec::new();
    let r = a.send(
        e,
        msg,
        &mut |ev| {
            if let Event::ToolFinished { name, ok, summary } = ev {
                log.push(format!("{name}{} [{summary}]", if ok { "" } else { " FAILED" }));
            }
        },
        &AtomicBool::new(false),
    );
    let r = r.unwrap_or_else(|err| panic!("turn {msg:?} failed: {err:?}; tools so far: {log:?}"));
    eprintln!(
        "LIVE model={} secs={:.1} requests={} in={} out={} cache_write={} cache_read={} cost=${:.4} msg={msg:?}\n  tools: {log:?}\n  reply: {}",
        model(),
        t.elapsed().as_secs_f64(),
        r.usage.requests,
        r.usage.input_tokens,
        r.usage.output_tokens,
        r.usage.cache_creation_input_tokens,
        r.usage.cache_read_input_tokens,
        r.usage.estimated_cost_usd(&model()).unwrap_or(f64::NAN),
        r.reply.replace('\n', " ")
    );
    r
}

fn has(e: &Editor, w: &str) -> bool {
    playing_words(e).iter().any(|x| x == w)
}

/// Checks the single-undo contract after a turn that changed the project.
fn assert_one_undo_restores(e: &mut Editor, original: &Project, depth_before: usize) {
    assert_eq!(undo_depth(e), depth_before + 1, "the turn must add exactly one undo step");
    assert!(e.undo_label().unwrap().starts_with("Assistant: "));
    let edited = e.project.clone();
    assert!(e.undo());
    assert_eq!(&e.project, original, "one undo must restore the project exactly");
    assert!(e.redo());
    assert_eq!(e.project, edited);
}

// --- assistant ------------------------------------------------------------------

#[test]
fn live_remove_the_filler_words() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    assert!(detect::find_fillers(e.comp().unwrap(), &FillerOptions::default()).len() >= 12);
    let r = turn(&mut Assistant::new(c), &mut e, "Remove the filler words.");
    assert!(r.changed);
    let left = detect::find_fillers(e.comp().unwrap(), &FillerOptions::default());
    assert!(left.is_empty(), "fillers left: {left:?}");
    assert!(!has(&e, "um") && !has(&e, "uh"));
    // Content survives.
    for w in ["hello", "welcome", "kitchen", "sourdough", "starter", "loaf", "marathon", "berlin", "shoes", "chicago", "listening"] {
        assert!(has(&e, w), "content word {w:?} was lost");
    }
    let before = original.compositions[0].word_count(0..original.compositions[0].script.len());
    let after = playing_words(&e).len();
    assert!(after >= before - 30, "removed far more than the fillers: {before} -> {after}");
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_cut_the_part_about_a_topic() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Cut the part where they talk about sourdough and baking bread.");
    assert!(r.changed);
    for w in ["sourdough", "starter", "loaf", "loaves", "flour", "oven", "dough"] {
        assert!(!has(&e, w), "bread-topic word {w:?} still plays");
    }
    // The other topic, the intro and the outro stay.
    for w in ["marathon", "berlin", "september", "miles", "shoes", "chicago", "kitchen", "welcome", "listening"] {
        assert!(has(&e, w), "{w:?} should have been kept");
    }
    let text = playing_text(&e);
    assert!(text.contains("the hardest part was not the long runs"), "{text}");
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_captions_yellow_one_line_uppercase() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Add captions in yellow, one line, uppercase.");
    assert!(r.changed);
    let cap = &e.comp().unwrap().captions;
    assert!(cap.enabled);
    assert!(cap.color.r >= 200 && cap.color.g >= 180 && cap.color.b <= 120, "not yellow: {}", cap.color.hex());
    assert_eq!(cap.max_lines, 1);
    assert!(cap.uppercase);
    assert_eq!(e.comp().unwrap().script, original.compositions[0].script, "the script must not change");
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_make_it_vertical_for_tiktok() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Make this vertical for TikTok.");
    assert!(r.changed);
    let comp = e.comp().unwrap();
    assert_eq!((comp.width, comp.height), (1080, 1920));
    assert_eq!(comp.script, original.compositions[0].script);
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_add_a_title_for_the_first_three_seconds() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Add a title that says \"Bread & Miles\" for the first three seconds.");
    assert!(r.changed);
    let comp = e.comp().unwrap();
    assert_eq!(comp.layers.len(), 1, "exactly one layer");
    let layer = &comp.layers[0];
    let LayerContent::Text(style) = &layer.content else { panic!("not a text layer: {:?}", layer.content) };
    assert_eq!(style.text, "Bread & Miles");
    assert!(layer.start.abs() < 0.05, "starts at {}", layer.start);
    assert!((layer.duration - 3.0).abs() < 0.05, "lasts {}", layer.duration);
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_add_chapter_markers() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Add chapter markers.");
    assert!(r.changed);
    let comp = e.comp().unwrap();
    let markers = comp.script_markers();
    assert!((2..=6).contains(&markers.len()), "{markers:?}");
    let sentences = context::sentences(comp);
    for (i, _, name) in &markers {
        assert!(sentences.iter().any(|s| s.start == *i), "marker {name:?} at token {i} is not at a sentence start");
        assert!(!name.trim().is_empty());
    }
    assert!(markers.windows(2).all(|w| w[0].0 < w[1].0));
    // One chapter opens where the conversation turns to running: Ana's "Okay, let's switch gears…"
    // (or, at the latest, Ben's first answer about the marathon).
    let (okay, did) = (word_index(&e, "okay"), word_index(&e, "berlin") - 6);
    assert!(markers.iter().any(|(i, _, _)| (okay..=did).contains(i)), "no chapter at the running topic ({okay}..={did}): {markers:?}");
    // And one covers the bread topic or the opening before it.
    assert!(markers.iter().any(|(i, _, _)| *i < okay));
    assert_eq!(comp.script.len(), original.compositions[0].script.len());
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_a_question_is_answered_without_editing() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "How long is this and how many speakers are there?");
    assert!(!r.changed);
    assert_eq!((r.undo_steps, r.commands_applied), (0, 0));
    assert_eq!(e.project, original);
    assert_eq!(undo_depth(&mut e), depth);
    let reply = r.reply.to_lowercase();
    // 119.0 s = 1:59.
    let secs = e.comp().unwrap().duration();
    assert!((118.5..119.5).contains(&secs));
    let duration_ok = ["1:59", "119", "2 min", "two min", "1 minute and 59", "1 minute 59", "1 min 59", "1m 59", "1m59"].iter().any(|d| reply.contains(d));
    assert!(duration_ok, "the reply does not state the duration (1:59): {reply}");
    let speakers_ok = reply.contains("two") || reply.contains("2 speakers") || (reply.contains("ana") && reply.contains("ben"));
    assert!(speakers_ok, "the reply does not state the two speakers: {reply}");
}

#[test]
fn live_an_impossible_request_changes_nothing_and_says_so() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let r = turn(&mut Assistant::new(c), &mut e, "Remove the word 'zebra'.");
    assert!(!r.changed, "nothing should have been edited");
    assert_eq!(e.project, original);
    assert_eq!(undo_depth(&mut e), depth);
    let reply = r.reply.to_lowercase();
    assert!(reply.contains("zebra"), "{reply}");
    let honest = ["not ", "n't", "never", "no ", "couldn't find", "isn't", "nothing"].iter().any(|n| reply.contains(n));
    assert!(honest, "the reply should say the word is not there: {reply}");
    assert!(!reply.contains("removed the word") && !reply.contains("i removed"), "{reply}");
}

#[test]
fn live_a_follow_up_restores_what_the_previous_turn_cut() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original_words = playing_words(&e);
    let depth = undo_depth(&mut e);
    let mut a = Assistant::new(c);
    let r1 = turn(&mut a, &mut e, "Cut Ana's intro at the start: everything before Ben first speaks.");
    assert!(r1.changed);
    assert!(!has(&e, "kitchen") && !has(&e, "guest"), "the intro still plays: {}", playing_text(&e));
    assert!(has(&e, "thanks") && has(&e, "sourdough") && has(&e, "marathon"));
    assert!(playing_text(&e).starts_with("thanks ana"), "{}", &playing_text(&e)[..60]);

    let r2 = turn(&mut a, &mut e, "Actually, keep the intro.");
    assert!(r2.changed);
    assert_eq!(playing_words(&e), original_words, "the intro must be back and nothing else changed");
    assert_eq!(undo_depth(&mut e), depth + 2, "one undo step per turn");
    assert_eq!(a.history().len(), 4);
}

#[test]
fn live_several_permanent_deletes_hit_exactly_the_right_words() {
    // Deleting shifts token indices; both deletions must still land on the right sentences.
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let before = playing_words(&e);
    let r = turn(&mut Assistant::new(c), &mut e, "Permanently delete (not just strike through) two sentences: Ana's \"I'm Ana.\" near the start, and Ben's \"And shoes matter.\"");
    assert!(r.changed);
    let mut expected = before.clone();
    for phrase in [vec!["i'm", "ana"], vec!["and", "shoes", "matter"]] {
        let at = expected.windows(phrase.len()).position(|w| w == phrase.as_slice()).unwrap();
        expected.drain(at..at + phrase.len());
    }
    assert_eq!(playing_words(&e), expected, "exactly those five words must be gone");
    let comp = e.comp().unwrap();
    assert!(comp.script.iter().all(|t| !t.ignored), "deleted, not struck through");
    assert_eq!(comp.script.iter().filter(|t| t.is_word()).count(), expected.len());
    assert_one_undo_restores(&mut e, &original, depth);
}

#[test]
fn live_an_edit_in_a_long_script_is_found_through_the_tools() {
    // ~3400 words: the model only gets an outline and has to search and read to find the passage.
    let Some(c) = client() else { return };
    let mut turns: Vec<(&str, &str)> = Vec::new();
    for rep in 0..12 {
        turns.extend_from_slice(PODCAST);
        if rep == 6 {
            turns.push(("Ben", "By the way, my cousin keeps a purple giraffe statue in his garden. It is enormous."));
            turns.push(("Ana", "That is wonderful."));
        }
    }
    let mut e = editor_from_turns(&turns);
    let original = e.project.clone();
    let depth = undo_depth(&mut e);
    let before = playing_words(&e);
    assert!(before.len() > context::FULL_SCRIPT_WORD_LIMIT);
    assert!(context::describe_project(&e).contains("SCRIPT (long; outline of paragraphs)"));
    let r = turn(&mut Assistant::new(c), &mut e, "Cut the bit where Ben talks about his cousin's purple giraffe statue.");
    assert!(r.changed);
    assert!(!has(&e, "giraffe") && !has(&e, "cousin") && !has(&e, "statue"), "{}", r.reply);
    let after = playing_words(&e);
    assert!(before.len() - after.len() <= 20, "cut {} words for a two-sentence aside", before.len() - after.len());
    for w in ["berlin", "sourdough", "chicago", "kitchen"] {
        assert_eq!(after.iter().filter(|x| *x == w).count(), before.iter().filter(|x| *x == w).count(), "{w:?} must be untouched");
    }
    assert_one_undo_restores(&mut e, &original, depth);
}

// --- one-shot tools ---------------------------------------------------------------

#[test]
fn live_text_tools_return_well_formed_output() {
    let Some(c) = client() else { return };
    let e = podcast_editor();
    let about = |s: &str| {
        let s = s.to_lowercase();
        (s.contains("bread") || s.contains("sourdough") || s.contains("bak")) && (s.contains("marathon") || s.contains("run"))
    };

    let summary = tools::summarize(&c, &e).unwrap();
    assert!(about(&summary) && summary.split_whitespace().count() >= 25 && !summary.contains('#'), "{summary}");

    let notes = tools::show_notes(&c, &e).unwrap();
    for section in ["## Summary", "## Key points", "## Mentions"] {
        assert!(notes.contains(section), "missing {section}: {notes}");
    }
    let stamps: Vec<&str> = notes.match_indices('[').filter_map(|(i, _)| notes.get(i + 1..i + 6)).filter(|t| t.as_bytes()[2] == b':' && t.chars().filter(|c| c.is_ascii_digit()).count() == 4).collect();
    assert!(stamps.len() >= 2, "no [mm:ss] timestamps in key points: {notes}");
    for t in &stamps {
        let secs = t[..2].parse::<f64>().unwrap() * 60.0 + t[3..].parse::<f64>().unwrap();
        assert!(secs <= e.comp().unwrap().duration() + 1.0, "timestamp {t} is past the end");
    }
    assert!(notes.contains("Berlin") || notes.contains("Chicago"), "mentions should name what is said: {notes}");

    let blog = tools::blog_post(&c, &e).unwrap();
    assert!(blog.starts_with("# ") && blog.contains("\n## ") && about(&blog) && blog.split_whitespace().count() >= 150, "{blog}");

    for p in [Platform::X, Platform::Threads, Platform::LinkedIn, Platform::Instagram, Platform::TikTok] {
        let post = tools::social_post(&c, &e, p).unwrap();
        assert!(!post.trim().is_empty() && post.chars().count() <= p.max_chars(), "{p:?}: {} chars: {post}", post.chars().count());
        assert!(about(&post) || post.to_lowercase().contains("ben"), "{p:?}: {post}");
    }
}

#[test]
fn live_titles_count_is_respected() {
    let Some(c) = client() else { return };
    let e = podcast_editor();
    for count in [1usize, 5] {
        let titles = tools::titles(&c, &e, count).unwrap();
        assert_eq!(titles.len(), count, "{titles:?}");
        let mut unique = titles.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), count, "{titles:?}");
        assert!(titles.iter().all(|t| !t.trim().is_empty() && t.chars().count() <= 100 && !t.contains('\n')), "{titles:?}");
    }
}

#[test]
fn live_youtube_description_includes_existing_chapters() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let (lets, okay) = (word_index(&e, "let's"), word_index(&e, "okay"));
    e.apply_all(
        "chapters",
        vec![
            Command::SetMarker { at: 1, name: Some("Welcome".into()) },
            Command::SetMarker { at: lets, name: Some("Sourdough".into()) },
            Command::SetMarker { at: okay, name: Some("Marathon training".into()) },
        ],
    )
    .unwrap();
    let chapters = transcript_export::chapters_text(e.comp().unwrap());
    let d = tools::youtube_description(&c, &e).unwrap();
    for line in chapters.lines() {
        assert!(d.contains(line), "chapter line {line:?} missing from: {d}");
    }
    assert!(d.split_whitespace().count() >= 30, "{d}");
}

#[test]
fn live_chapters_are_in_bounds_increasing_and_on_sentence_starts() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let chapters = tools::chapters(&c, &e).unwrap();
    let comp = e.comp().unwrap();
    let sentences = context::sentences(comp);
    assert!((2..=6).contains(&chapters.len()), "{chapters:?}");
    for ch in &chapters {
        assert!(ch.token < comp.script.len());
        assert!(sentences.iter().any(|s| s.start == ch.token), "{ch:?} is not at a sentence start");
        assert_eq!(ch.time, comp.time_of_token(ch.token));
        assert!(!ch.title.is_empty());
    }
    assert!(chapters.windows(2).all(|w| w[0].token < w[1].token && w[1].time - w[0].time >= tools::MIN_CHAPTER_SPACING), "{chapters:?}");
    assert_eq!(chapters[0].token, sentences[0].start, "the first chapter starts at the beginning");
    let (okay, did) = (word_index(&e, "okay"), word_index(&e, "berlin") - 6);
    assert!(chapters.iter().any(|ch| (okay..=did).contains(&ch.token)), "no chapter at the running topic: {chapters:?}");
    // Accepting them works.
    let n = chapters.len();
    e.apply_all("Chapters", tools::apply_chapters(&chapters)).unwrap();
    assert_eq!(e.comp().unwrap().script_markers().len(), n);
}

#[test]
fn live_clips_are_whole_sentences_within_the_duration_bounds() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let (min, max) = (12.0, 35.0);
    let clips = tools::find_clips(&c, &e, 2, min, max, None).unwrap();
    let comp = e.comp().unwrap();
    let sentences = context::sentences(comp);
    assert!(!clips.is_empty() && clips.len() <= 2, "{clips:?}");
    for cl in &clips {
        assert!(cl.start < cl.end && cl.end <= comp.script.len());
        assert!((min..=max).contains(&cl.duration), "{cl:?}");
        assert!((cl.duration - comp.range_duration(cl.start..cl.end)).abs() < 1e-9);
        assert!(sentences.iter().any(|s| s.start == cl.start), "{cl:?} does not start on a sentence");
        assert!(sentences.iter().any(|s| s.end == cl.end), "{cl:?} does not end on a sentence");
        assert!(!cl.title.is_empty() && !cl.reason.is_empty());
    }
    if clips.len() == 2 {
        assert!(clips[0].end <= clips[1].start || clips[1].end <= clips[0].start, "clips overlap: {clips:?}");
    }
    // A topic filter is honoured.
    let running = tools::find_clips(&c, &e, 1, min, max, Some("marathon running")).unwrap();
    assert_eq!(running.len(), 1);
    let text = comp.text_of(running[0].start..running[0].end).to_lowercase();
    assert!(text.contains("marathon") || text.contains("running") || text.contains("shoes"), "{text}");
    assert!(!text.contains("sourdough"), "{text}");
    // Accepting creates the compositions.
    let source = e.active.unwrap();
    e.apply_all("Clips", tools::apply_clips(source, &clips)).unwrap();
    assert_eq!(e.project.compositions.len(), 1 + clips.len());
    assert_eq!(e.active, Some(source));
}

#[test]
fn live_clarity_edits_target_the_false_start() {
    let Some(c) = client() else { return };
    let mut e = podcast_editor();
    let comp = e.comp().unwrap();
    let edits = tools::edit_for_clarity(&c, &e, Intensity::Low).unwrap();
    assert!(!edits.is_empty());
    for ed in &edits {
        assert!(ed.start < ed.end && ed.end <= comp.script.len());
        assert!(comp.script[ed.start].shows_text() && comp.script[ed.end - 1].shows_text(), "{ed:?}");
    }
    assert!(edits.windows(2).all(|w| w[0].end < w[1].start), "edits must be ordered and disjoint: {edits:?}");
    // The repeated false start "today we have, today we have": one of the two takes is struck.
    let first = word_index(&e, "today");
    let covers = |a: usize, b: usize| edits.iter().any(|ed| ed.start <= a && b <= ed.end);
    assert!(covers(first, first + 3) || covers(first + 4, first + 7), "the false start at {first} is not targeted: {edits:?}");
    // Low intensity keeps the content.
    e.apply_all("Clarity", tools::apply_clarity_edits(&edits)).unwrap();
    let text = playing_text(&e);
    assert_eq!(text.matches("today we have").count(), 1, "{text}");
    for w in ["sourdough", "starter", "marathon", "berlin", "shoes", "kitchen"] {
        assert!(has(&e, w), "{w:?} was cut at low intensity");
    }
    let before = context::sentences(&podcast_editor().comp().unwrap().clone()).iter().map(|s| s.text.split(' ').count()).sum::<usize>();
    assert!(playing_words(&e).len() * 10 >= before * 8, "low intensity removed more than a fifth");
}

fn looks_spanish(s: &str) -> bool {
    let t = format!(" {} ", s.to_lowercase().replace(['.', ',', '?', '!', '¿', '¡', '\n'], " "));
    let hits = [" el ", " la ", " de ", " que ", " y ", " en ", " los ", " es ", " un ", " una ", " con ", " por ", " para ", " mi ", " te ", " muy "].iter().filter(|w| t.contains(*w)).count();
    hits >= 4
}

#[test]
fn live_translation_to_spanish_keeps_the_cue_count() {
    let Some(c) = client() else { return };
    let e = podcast_editor();
    let cues = captions::cues(e.comp().unwrap(), &captions::CueOptions::default());
    let texts: Vec<String> = cues.iter().map(|c| c.text()).collect();
    assert!(texts.len() >= 20, "{} cues", texts.len());
    let out = tools::translate_captions(&c, &texts, "Spanish").unwrap();
    assert_eq!(out.len(), texts.len(), "one translation per cue");
    assert!(out.iter().all(|t| !t.trim().is_empty()));
    assert!(looks_spanish(&out.join(" ")), "{out:?}");
    assert!(!looks_spanish(&texts.join(" ")));
    let unchanged = out.iter().zip(&texts).filter(|(a, b)| a == b).count();
    assert!(unchanged * 5 <= texts.len(), "{unchanged} of {} cues were left untranslated", texts.len());
    // Cue i still corresponds to cue i: names stay where they were.
    for (i, t) in texts.iter().enumerate() {
        for name in ["Berlin", "Chicago"] {
            if t.contains(name) {
                let near = out[i.saturating_sub(1)..(i + 2).min(out.len())].join(" ");
                assert!(near.contains(name) || near.contains("Berlín"), "cue {i} {t:?} -> {:?}", out[i]);
            }
        }
    }

    let es = tools::translate_text(&c, "Good bread needs time and a wet dough.\n\nShoes matter when you train for a marathon.", "Spanish").unwrap();
    assert!(looks_spanish(&es) || es.to_lowercase().contains("pan"), "{es}");
    assert_eq!(es.matches("\n\n").count(), 1, "paragraphs are kept: {es:?}");
}

#[test]
fn live_writing_tools() {
    let Some(c) = client() else { return };
    let original = "So basically what I wanted to say is that, you know, good bread really does need quite a lot of time to come out well.";
    let shorter = tools::rewrite(&c, original, "Make it much shorter and remove the filler.").unwrap();
    assert!(!shorter.is_empty() && shorter.len() < original.len() && shorter.to_lowercase().contains("bread"), "{shorter}");

    let script = tools::write_script(&c, "A 20-second intro for a podcast called Kitchen Table, hosted by Ana, about home baking.").unwrap();
    assert!(script.contains("Kitchen Table") && script.split_whitespace().count() >= 25, "{script}");
    let outline = tools::outline(&c, "A 10-minute video on feeding a sourdough starter.").unwrap();
    assert!(outline.lines().filter(|l| l.trim_start().starts_with(['-', '*']) || l.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit())).count() >= 4, "{outline}");
    let ideas = tools::brainstorm(&c, "Episode ideas for a podcast about baking bread at home.").unwrap();
    assert!(ideas.lines().filter(|l| l.trim_start().starts_with(['-', '*']) || l.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit())).count() >= 8, "{ideas}");
}

#[test]
fn live_speaker_labels_from_wording() {
    let Some(c) = client() else { return };
    let unlabelled: Vec<(&str, &str)> = PODCAST.iter().map(|(_, t)| ("", *t)).collect();
    let mut e = editor_from_turns(&unlabelled);
    let turns = tools::label_speakers(&c, &e).unwrap();
    let comp = e.comp().unwrap();
    let sentences = context::sentences(comp);
    assert_eq!(turns[0].0, sentences[0].start);
    assert!(turns.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 != w[1].1), "{turns:?}");
    for t in &turns {
        assert!(sentences.iter().any(|s| s.start == t.0), "turn {t:?} is not at a sentence start");
    }
    let mut names: Vec<&str> = turns.iter().map(|t| t.1.as_str()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 2, "two speakers expected: {turns:?}");
    // Whoever opens the show is not the one who says "Thanks, Ana." or who tells the baking story.
    let speaker_at = |i: usize| turns.iter().rev().find(|t| t.0 <= i).unwrap().1.clone();
    let host = speaker_at(word_index(&e, "hello"));
    assert_ne!(speaker_at(word_index(&e, "neighbor")), host, "{turns:?}");
    assert_eq!(speaker_at(word_index(&e, "delicious")), host, "{turns:?}");
    assert_ne!(speaker_at(word_index(&e, "berlin")), host, "{turns:?}");
    // The names are stated in the recording ("I'm Ana", "My guest is Ben").
    assert_eq!(host, "Ana");
    assert!(names.contains(&"Ben"), "{names:?}");
    let len = comp.script.len();
    e.apply_all("Speakers", tools::apply_speaker_labels(len, &turns)).unwrap();
    assert_eq!(e.project.speakers.len(), 2);
}
