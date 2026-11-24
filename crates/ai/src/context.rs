//! Compact text renderings of a project for the model.
//!
//! # Script format
//!
//! One line per paragraph, every word prefixed with its token index:
//!
//! ```text
//! script rev 3fa9c1 · tokens 0..212 (showing 0..212) · ranges are start..end, end exclusive
//! ¶ 0..58 00:00 Ana: 0:So 1:um, 3:welcome 4:to 5:the 6:show. (7: 1.8s pause) ~~8:I 9:think~~ 10:I 11:think …
//! # marker at 58: "Pricing"
//! --- scene 2 starts at 58 ---
//! ¶ 58..120 00:31 Ben: ✂ 58:Thanks 59:for …
//! ```
//!
//! * `N:word` — the word is token `N`. Indices that are skipped are short silences.
//! * `(N: 1.8s pause)` / `(N..M: 2.4s pause)` — a silence of at least [`LONG_GAP_SECS`].
//! * `~~ … ~~` — ignored (struck through): still in the script, skipped in playback and export.
//! * `{hidden N:word}` — media plays but the text is hidden from captions and exports.
//! * `✂` before a word — media was deleted between it and the previous word.
//!
//! Token indices are only meaningful together with the script *revision*
//! ([`script_rev`]): a short fingerprint of the token structure that changes
//! whenever indices shift. Tools that take indices require the revision the
//! model read them at, so stale indices are rejected instead of applied.

use mango_core::transcript_export::timecode;
use mango_core::{Composition, Editor, LayerContent, MediaKind, Project, Secs, SpeakerId, TokenKind, script};
use std::fmt::Write as _;
use std::ops::Range;

/// Scripts up to this many words are included in full in [`describe_project`];
/// longer ones are outlined and read in pages through tools.
pub const FULL_SCRIPT_WORD_LIMIT: usize = 2000;
/// Most words one page of the script contains.
pub const PAGE_WORD_LIMIT: usize = 1500;
/// Silences at least this long are shown in the script.
pub const LONG_GAP_SECS: Secs = 1.0;

/// Short fingerprint of the script's token structure (which media each index
/// points at). Changes whenever token indices shift; does not change when
/// only flags or text change (ignoring, correcting, markers).
pub fn script_rev(comp: &Composition) -> String {
    // FNV-1a, written out so the value is stable across runs and toolchains.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    eat(comp.id.0.as_bytes());
    eat(&(comp.script.len() as u64).to_le_bytes());
    for t in &comp.script {
        eat(&[t.kind as u8]);
        eat(t.asset.0.as_bytes());
        eat(&t.src_start.to_bits().to_le_bytes());
        eat(&t.src_end.to_bits().to_le_bytes());
    }
    format!("{:06x}", (h ^ (h >> 24) ^ (h >> 48)) & 0xff_ffff)
}

fn speaker_label(project: &Project, id: Option<SpeakerId>) -> &str {
    match id {
        Some(_) => project.speaker_name(id),
        None => "",
    }
}

/// Number of word tokens in a range, including ignored and hidden ones.
fn words_in(comp: &Composition, range: Range<usize>) -> usize {
    comp.script[range].iter().filter(|t| t.is_word()).count()
}

/// End of a page that starts at `start`: at most `max_words` words, ending on
/// a paragraph boundary when possible.
pub fn page_end(comp: &Composition, start: usize, max_words: usize) -> usize {
    let len = comp.script.len();
    let start = start.min(len);
    let mut words = 0usize;
    let mut last_par = None;
    for (i, t) in comp.script.iter().enumerate().skip(start) {
        if i > start && (t.paragraph_start || t.scene_start || t.marker.is_some()) {
            last_par = Some(i);
        }
        if t.is_word() {
            words += 1;
            if words > max_words {
                return last_par.unwrap_or(i);
            }
        }
    }
    len
}

/// Renders tokens `range` of the script in the indexed format described in
/// the module docs. The range is clamped to the script.
pub fn describe_script(comp: &Composition, project: &Project, range: Range<usize>) -> String {
    let len = comp.script.len();
    let range = script::clamp_range(&range, len);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "script rev {} · tokens 0..{} (showing {}..{}) · ranges are start..end, end exclusive",
        script_rev(comp),
        len,
        range.start,
        range.end
    );
    if range.is_empty() {
        out.push_str(if len == 0 { "(the script is empty)\n" } else { "(nothing in this range)\n" });
        return out;
    }
    let starts = comp.token_starts();
    let cuts = comp.edit_boundaries();
    let mut scene = comp.scene_index_of_token(range.start);
    for par in comp.paragraphs() {
        let (s, e) = (par.start.max(range.start), par.end.min(range.end));
        if s >= e {
            continue;
        }
        let first = &comp.script[par.start];
        if s == par.start {
            if par.start > 0 && first.scene_start {
                scene += 1;
                let _ = writeln!(out, "--- scene {} starts at {} ---", scene + 1, par.start);
            }
            if let Some(m) = &first.marker {
                let _ = writeln!(out, "# marker at {}: {:?}", par.start, m);
            }
        }
        let speaker = speaker_label(project, comp.script[s].speaker);
        let _ = write!(out, "¶ {}..{} {}", s, e, timecode(starts[s]));
        if !speaker.is_empty() {
            let _ = write!(out, " {speaker}");
        }
        out.push(':');
        // Pieces of the line with their ignored state; ignored runs are wrapped in ~~ … ~~.
        let mut pieces: Vec<(String, bool)> = Vec::new();
        let mut i = s;
        while i < e {
            let t = &comp.script[i];
            if t.kind == TokenKind::Word {
                let mut p = String::new();
                if let Some(n) = &t.note {
                    let _ = write!(p, "(note at {i}: {n}) ");
                }
                if cuts.binary_search(&i).is_ok() {
                    p.push_str("✂ ");
                }
                if t.hidden_text {
                    let _ = write!(p, "{{hidden {}:{}}}", i, t.text);
                } else {
                    let _ = write!(p, "{}:{}", i, t.text);
                }
                pieces.push((p, t.ignored));
                i += 1;
            } else {
                // A run of silence tokens with the same ignored state.
                let run_start = i;
                let mut total = 0.0;
                while i < e && comp.script[i].kind != TokenKind::Word && comp.script[i].ignored == t.ignored {
                    total += comp.script[i].duration();
                    i += 1;
                }
                if total >= LONG_GAP_SECS {
                    let p = if i - run_start == 1 { format!("({run_start}: {total:.1}s pause)") } else { format!("({run_start}..{i}: {total:.1}s pause)") };
                    pieces.push((p, t.ignored));
                }
            }
        }
        for (k, (p, ignored)) in pieces.iter().enumerate() {
            out.push(' ');
            if *ignored && (k == 0 || !pieces[k - 1].1) {
                out.push_str("~~");
            }
            out.push_str(p);
            if *ignored && pieces.get(k + 1).is_none_or(|n| !n.1) {
                out.push_str("~~");
            }
        }
        out.push('\n');
    }
    out
}

/// At most about this many lines in an outline; paragraphs of a very long
/// script are grouped into sections to stay within it.
const OUTLINE_LINES: usize = 250;

/// The table of contents of a long script: one line per paragraph (or, for
/// very long scripts, per run of paragraphs) with its token range, time,
/// speakers, word count and opening words. The ranges tile the whole script.
pub fn describe_outline(comp: &Composition, project: &Project) -> String {
    let len = comp.script.len();
    let mut out = String::new();
    let _ = writeln!(out, "script rev {} · tokens 0..{} · outline only: read exact words and indices with read_script", script_rev(comp), len);
    let starts = comp.token_starts();
    let pars = comp.paragraphs();
    let total_words = words_in(comp, 0..len);
    // Sections of at least this many words; 0 keeps every paragraph on its own line.
    let min_section = if pars.len() > OUTLINE_LINES { total_words / OUTLINE_LINES } else { 0 };
    let mut sections: Vec<Range<usize>> = Vec::new();
    let mut words_in_section = 0usize;
    for par in pars {
        let first = &comp.script[par.start];
        let hard_break = first.scene_start || first.marker.is_some();
        let n = words_in(comp, par.clone());
        match sections.last_mut() {
            Some(last) if !hard_break && words_in_section < min_section => {
                last.end = par.end;
                words_in_section += n;
            }
            _ => {
                sections.push(par);
                words_in_section = n;
            }
        }
    }
    let mut scene = 0usize;
    for sec in sections {
        let first = &comp.script[sec.start];
        if sec.start > 0 && first.scene_start {
            scene += 1;
            let _ = writeln!(out, "--- scene {} starts at {} ---", scene + 1, sec.start);
        }
        if let Some(m) = &first.marker {
            let _ = writeln!(out, "# marker at {}: {:?}", sec.start, m);
        }
        let playing: Vec<&str> = comp.script[sec.clone()].iter().filter(|t| t.shows_text()).map(|t| t.text.as_str()).collect();
        let total = words_in(comp, sec.clone());
        let mut speakers: Vec<&str> = Vec::new();
        for t in comp.script[sec.clone()].iter().filter(|t| t.is_word()) {
            let name = speaker_label(project, t.speaker);
            if !name.is_empty() && !speakers.contains(&name) {
                speakers.push(name);
            }
        }
        let _ = write!(out, "¶ {}..{} {}", sec.start, sec.end, timecode(starts[sec.start]));
        if !speakers.is_empty() {
            let _ = write!(out, " {}", speakers.iter().take(4).copied().collect::<Vec<_>>().join(", "));
        }
        let _ = write!(out, " ({} words", playing.len());
        if total > playing.len() {
            let _ = write!(out, ", {} ignored/hidden", total - playing.len());
        }
        out.push_str("): ");
        out.push_str(&playing.iter().take(12).copied().collect::<Vec<_>>().join(" "));
        if playing.len() > 12 {
            out.push_str(" …");
        }
        out.push('\n');
    }
    out
}

fn aspect(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    let (a, b) = (w / g, h / g);
    if a <= 64 && b <= 64 { format!("{a}:{b}") } else { format!("{:.2}:1", w as f64 / h.max(1) as f64) }
}

fn layer_kind(c: &LayerContent) -> String {
    match c {
        LayerContent::Video(_) => "video".into(),
        LayerContent::Image(_) => "image".into(),
        LayerContent::Audio(_) => "audio".into(),
        LayerContent::Text(t) => format!("text {:?}", t.text),
        LayerContent::Shape(_) => "shape".into(),
        LayerContent::Waveform(_) => "waveform".into(),
        LayerContent::Progress { .. } => "progress bar".into(),
    }
}

/// Everything about the composition except its words: canvas, speakers,
/// captions, layers, markers, other compositions, files.
pub fn describe_summary(editor: &Editor) -> String {
    let project = &editor.project;
    let mut out = String::new();
    let _ = writeln!(out, "PROJECT {:?}", project.name);
    let Some(comp) = editor.comp() else {
        out.push_str("No composition is open.\n");
        describe_files(project, &mut out);
        return out;
    };
    let playing = comp.word_count(0..comp.script.len());
    let all_words = words_in(comp, 0..comp.script.len());
    let ignored = comp.script.iter().filter(|t| t.is_word() && t.ignored).count();
    let dur = comp.duration();
    let _ = writeln!(
        out,
        "ACTIVE COMPOSITION {:?} (id {}): {}x{} ({}), {} fps, duration {} ({:.1}s), {} words playing{}",
        comp.name,
        comp.id,
        comp.width,
        comp.height,
        aspect(comp.width, comp.height),
        comp.fps,
        timecode(dur),
        dur,
        playing,
        if ignored > 0 { format!(", {ignored} ignored (struck through)") } else { String::new() },
    );
    if all_words == 0 {
        out.push_str("The script has no words (nothing transcribed or added yet).\n");
    }

    // Speakers, by words spoken.
    let mut counts: Vec<(Option<SpeakerId>, usize)> = Vec::new();
    for t in comp.script.iter().filter(|t| t.shows_text()) {
        match counts.iter_mut().find(|(id, _)| *id == t.speaker) {
            Some(c) => c.1 += 1,
            None => counts.push((t.speaker, 1)),
        }
    }
    let named: Vec<String> = counts.iter().filter(|(id, _)| id.is_some()).map(|(id, n)| format!("{} ({} words)", project.speaker_name(*id), n)).collect();
    let unlabelled: usize = counts.iter().filter(|(id, _)| id.is_none()).map(|(_, n)| *n).sum();
    let _ = write!(out, "SPEAKERS: ");
    if named.is_empty() {
        out.push_str("none labelled");
    } else {
        let _ = write!(out, "{} — {}", named.len(), named.join(", "));
        if unlabelled > 0 {
            let _ = write!(out, "; {unlabelled} words unlabelled");
        }
    }
    out.push('\n');

    let scenes = comp.scene_spans();
    let _ = writeln!(out, "SCENES: {}", scenes.len());

    let gaps = mango_core::detect::find_gaps(comp, LONG_GAP_SECS);
    if !gaps.is_empty() {
        let longest = gaps.iter().map(|g| g.1).fold(0.0, f64::max);
        let total: f64 = gaps.iter().map(|g| g.1).sum();
        let _ = writeln!(out, "SILENCES of {LONG_GAP_SECS:.0}s or more: {} (longest {:.1}s, {:.1}s in total)", gaps.len(), longest, total);
    }

    let c = &comp.captions;
    if c.enabled {
        let _ = writeln!(
            out,
            "CAPTIONS: on — color {}, font {:?}, size {:.3}, {}max {} line(s) of {} chars, y {:.2}, highlight {:?} ({}), background {}, outline {}{}",
            c.color.hex(),
            c.font,
            c.size,
            if c.uppercase { "UPPERCASE, " } else { "" },
            c.max_lines,
            c.max_chars_per_line,
            c.y,
            c.highlight,
            c.highlight_color.hex(),
            c.background.map(|b| b.hex()).unwrap_or_else(|| "none".into()),
            c.outline.map(|(col, w)| format!("{} {w}px", col.hex())).unwrap_or_else(|| "none".into()),
            if c.bold { ", bold" } else { "" },
        );
    } else {
        out.push_str("CAPTIONS: off\n");
    }

    if comp.layers.is_empty() {
        out.push_str("LAYERS: none\n");
    } else {
        out.push_str("LAYERS:\n");
        for l in &comp.layers {
            let _ = writeln!(
                out,
                "- id {} {:?}: {}, {:.2}s–{:.2}s, track {}{}",
                l.id,
                l.name,
                layer_kind(&l.content),
                l.start,
                l.end(),
                l.track,
                if l.hidden { ", hidden" } else { "" }
            );
        }
    }

    let markers = comp.script_markers();
    if markers.is_empty() {
        out.push_str("MARKERS (chapters): none\n");
    } else {
        out.push_str("MARKERS (chapters):\n");
        for (i, t, name) in markers {
            let _ = writeln!(out, "- {} {:?} at token {}", timecode(t), name, i);
        }
    }

    let others: Vec<String> =
        project.compositions.iter().filter(|o| o.id != comp.id).map(|o| format!("{:?} (id {}, {})", o.name, o.id, timecode(o.duration()))).collect();
    if !others.is_empty() {
        let _ = writeln!(out, "OTHER COMPOSITIONS: {}", others.join("; "));
    }
    describe_files(project, &mut out);
    out
}

fn describe_files(project: &Project, out: &mut String) {
    if project.assets.is_empty() {
        out.push_str("FILES: none\n");
        return;
    }
    out.push_str("FILES:\n");
    for a in &project.assets {
        let kind = match a.kind {
            MediaKind::Video => format!("video {}x{}", a.width, a.height),
            MediaKind::Audio => "audio".to_string(),
            MediaKind::Image => "image".to_string(),
        };
        let transcript = match &a.transcript {
            Some(t) => format!("transcribed ({} words)", t.words.len()),
            None => "not transcribed".to_string(),
        };
        let _ = writeln!(out, "- id {} {:?}: {}, {}, {}", a.id, a.name, kind, timecode(a.duration), transcript);
    }
}

/// The project as the model sees it at the start of a turn: the summary,
/// then the script — in full when it is short, as a paragraph outline (to be
/// paged through with tools) when it is long.
pub fn describe_project(editor: &Editor) -> String {
    let mut out = describe_summary(editor);
    if let Some(comp) = editor.comp() {
        out.push('\n');
        let len = comp.script.len();
        if words_in(comp, 0..len) <= FULL_SCRIPT_WORD_LIMIT {
            out.push_str("SCRIPT (complete):\n");
            out.push_str(&describe_script(comp, &editor.project, 0..len));
        } else {
            out.push_str("SCRIPT (long; outline of paragraphs):\n");
            out.push_str(&describe_outline(comp, &editor.project));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Sentences
// ---------------------------------------------------------------------------

/// A sentence of playing text.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    /// Index of its first playing word.
    pub start: usize,
    /// One past its last playing word.
    pub end: usize,
    /// Composition time at which it starts.
    pub time: Secs,
    pub speaker: Option<SpeakerId>,
    pub text: String,
}

/// Unpunctuated speech is broken into pseudo-sentences of this many words.
const MAX_SENTENCE_WORDS: usize = 45;

/// Splits the playing text into sentences. A sentence ends at `.`, `?` or
/// `!`, at a paragraph or speaker change, or after [`MAX_SENTENCE_WORDS`].
pub fn sentences(comp: &Composition) -> Vec<Sentence> {
    let starts = comp.token_starts();
    let mut out: Vec<Sentence> = Vec::new();
    let mut cur: Option<(Sentence, usize)> = None; // (sentence, words)
    let mut close_next = false;
    for (i, t) in comp.script.iter().enumerate() {
        if !t.shows_text() {
            // A paragraph break on a silent token still ends the sentence.
            if t.paragraph_start || t.scene_start {
                close_next = true;
            }
            continue;
        }
        let boundary = close_next
            || t.paragraph_start
            || t.scene_start
            || t.marker.is_some()
            || cur.as_ref().is_some_and(|(s, n)| s.speaker != t.speaker || *n >= MAX_SENTENCE_WORDS);
        if boundary
            && let Some((s, _)) = cur.take() {
                out.push(s);
            }
        close_next = false;
        let (s, n) = cur.get_or_insert_with(|| (Sentence { start: i, end: i + 1, time: starts[i], speaker: t.speaker, text: String::new() }, 0));
        if !s.text.is_empty() {
            s.text.push(' ');
        }
        s.text.push_str(&t.text);
        s.end = i + 1;
        *n += 1;
        if script::ends_sentence(&t.text) {
            close_next = true;
        }
    }
    if let Some((s, _)) = cur {
        out.push(s);
    }
    out
}

/// The sentence start at or before token `idx` (the start of the sentence
/// containing it), or the first sentence when `idx` precedes all text.
/// `None` when `idx` is outside the script or the script has no text.
pub fn snap_to_sentence_start(comp: &Composition, sentences: &[Sentence], idx: usize) -> Option<usize> {
    if idx >= comp.script.len() || sentences.is_empty() {
        return None;
    }
    let k = sentences.partition_point(|s| s.start <= idx);
    Some(sentences[k.saturating_sub(1)].start)
}

/// Numbered sentences with timecodes, for the one-shot tools:
/// `S12 [03:41.4] Ana: Sentence text.` (the speaker is shown when it changes).
pub fn render_sentences(project: &Project, sentences: &[Sentence], with_speakers: bool) -> String {
    let mut out = String::new();
    let mut last: Option<Option<SpeakerId>> = None;
    for (k, s) in sentences.iter().enumerate() {
        let tenths = (s.time.max(0.0) * 10.0).round() as u64;
        let _ = write!(out, "S{} [{:02}:{:02}.{}]", k + 1, tenths / 600, (tenths / 10) % 60, tenths % 10);
        if with_speakers && s.speaker.is_some() && last != Some(s.speaker) {
            let _ = write!(out, " {}:", project.speaker_name(s.speaker));
        }
        last = Some(s.speaker);
        let _ = writeln!(out, " {}", s.text);
    }
    out
}
