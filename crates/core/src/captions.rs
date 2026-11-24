//! Caption cues from the script, subtitle file formats, and ASS generation
//! for burned-in captions.

use crate::model::*;

#[derive(Clone, Debug, PartialEq)]
pub struct CueWord {
    pub text: String,
    pub start: Secs,
    pub end: Secs,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: Secs,
    pub end: Secs,
    pub lines: Vec<String>,
    pub words: Vec<CueWord>,
    pub speaker: Option<SpeakerId>,
}

impl Cue {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CueOptions {
    pub max_chars_per_line: usize,
    pub max_lines: usize,
    /// Start a new cue after a silence this long.
    pub break_on_pause: Secs,
    pub uppercase: bool,
}

impl Default for CueOptions {
    fn default() -> Self {
        Self { max_chars_per_line: 42, max_lines: 2, break_on_pause: 0.7, uppercase: false }
    }
}

impl From<&CaptionSettings> for CueOptions {
    fn from(c: &CaptionSettings) -> Self {
        Self { max_chars_per_line: c.max_chars_per_line.max(4), max_lines: c.max_lines.max(1), break_on_pause: 0.7, uppercase: c.uppercase }
    }
}

/// Builds caption cues in composition time from the script as it plays.
pub fn cues(comp: &Composition, opts: &CueOptions) -> Vec<Cue> {
    let starts = comp.token_starts();
    let mut out: Vec<Cue> = Vec::new();
    let mut cur: Option<Cue> = None;
    let mut silence: Secs = 0.0;
    let max_line = opts.max_chars_per_line.max(4);
    let max_lines = opts.max_lines.max(1);

    let flush = |cur: &mut Option<Cue>, out: &mut Vec<Cue>| {
        if let Some(c) = cur.take() {
            if !c.words.is_empty() {
                out.push(c);
            }
        }
    };

    for (i, tok) in comp.script.iter().enumerate() {
        if tok.ignored {
            continue;
        }
        if !tok.shows_text() {
            silence += tok.play_duration();
            continue;
        }
        let text = if opts.uppercase { tok.text.to_uppercase() } else { tok.text.clone() };
        let (ws, we) = (starts[i], starts[i] + tok.play_duration());
        let mut new_cue = cur.is_none();
        if let Some(c) = &cur {
            let speaker_change = c.speaker != tok.speaker;
            let paused = silence >= opts.break_on_pause;
            let last_line_len = c.lines.last().map(|l| l.chars().count()).unwrap_or(0);
            let fits_line = last_line_len + 1 + text.chars().count() <= max_line;
            let fits_cue = fits_line || c.lines.len() < max_lines;
            let prev_ended_sentence = c.words.last().is_some_and(|w| crate::script::ends_sentence(&w.text));
            // Prefer breaking at a sentence end once the cue is reasonably full.
            let total: usize = c.lines.iter().map(|l| l.chars().count()).sum();
            let sentence_break = prev_ended_sentence && total >= max_line / 2;
            if speaker_change || paused || !fits_cue || sentence_break || tok.paragraph_start || tok.scene_start {
                new_cue = true;
            }
        }
        if new_cue {
            flush(&mut cur, &mut out);
            cur = Some(Cue { start: ws, end: we, lines: vec![String::new()], words: Vec::new(), speaker: tok.speaker });
        }
        let c = cur.as_mut().unwrap();
        let line = c.lines.last_mut().unwrap();
        if line.is_empty() {
            line.push_str(&text);
        } else if line.chars().count() + 1 + text.chars().count() <= max_line {
            line.push(' ');
            line.push_str(&text);
        } else {
            c.lines.push(text.clone());
        }
        c.end = we;
        c.words.push(CueWord { text, start: ws, end: we });
        silence = 0.0;
    }
    flush(&mut cur, &mut out);

    // Hold each cue briefly after its last word, without overlapping the next.
    let n = out.len();
    for i in 0..n {
        let next_start = if i + 1 < n { out[i + 1].start } else { f64::INFINITY };
        let min_end = out[i].start + 0.6;
        out[i].end = (out[i].end + 0.25).max(min_end).min(next_start).max(out[i].end);
    }
    out
}

fn ts(t: Secs, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{}{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, sep, ms % 1000)
}

pub fn to_srt(cues: &[Cue], project: Option<&Project>, show_speakers: bool) -> String {
    let mut s = String::new();
    let mut last_speaker: Option<Option<SpeakerId>> = None;
    for (i, c) in cues.iter().enumerate() {
        s.push_str(&format!("{}\n{} --> {}\n", i + 1, ts(c.start, ','), ts(c.end, ',')));
        let mut text = c.text();
        if show_speakers && last_speaker != Some(c.speaker) {
            if let (Some(p), Some(_)) = (project, c.speaker) {
                text = format!("{}: {}", p.speaker_name(c.speaker), text);
            }
        }
        last_speaker = Some(c.speaker);
        s.push_str(&text);
        s.push_str("\n\n");
    }
    s
}

pub fn to_vtt(cues: &[Cue], project: Option<&Project>, show_speakers: bool) -> String {
    let mut s = String::from("WEBVTT\n\n");
    for (i, c) in cues.iter().enumerate() {
        s.push_str(&format!("{}\n{} --> {}\n", i + 1, ts(c.start, '.'), ts(c.end, '.')));
        let text = c.text();
        match (show_speakers, project, c.speaker) {
            (true, Some(p), Some(_)) => s.push_str(&format!("<v {}>{}", p.speaker_name(c.speaker), text)),
            _ => s.push_str(&text),
        }
        s.push_str("\n\n");
    }
    s
}

/// One subtitle entry parsed from an SRT or VTT file.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCue {
    pub start: Secs,
    pub end: Secs,
    pub text: String,
}

fn parse_ts(s: &str) -> Option<Secs> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.len() {
        3 => (parts[0].parse::<f64>().ok()?, parts[1].parse::<f64>().ok()?, parts[2].parse::<f64>().ok()?),
        2 => (0.0, parts[0].parse::<f64>().ok()?, parts[1].parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Parses SRT or WebVTT.
pub fn parse_subtitles(input: &str) -> Vec<ParsedCue> {
    let mut out = Vec::new();
    let text = input.replace("\r\n", "\n").replace('\u{feff}', "");
    for block in text.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let mut timing: Option<(Secs, Secs)> = None;
        let mut body: Vec<&str> = Vec::new();
        for line in lines.by_ref() {
            if timing.is_none() {
                if let Some((a, b)) = line.split_once("-->") {
                    let b = b.split_whitespace().next().unwrap_or("");
                    if let (Some(s), Some(e)) = (parse_ts(a), parse_ts(b)) {
                        timing = Some((s, e));
                    }
                }
                continue;
            }
            body.push(line);
        }
        if let Some((start, end)) = timing {
            let mut t = body.join(" ");
            // Strip simple tags such as <v Name> and <i>.
            while let (Some(a), Some(b)) = (t.find('<'), t.find('>')) {
                if a < b {
                    t.replace_range(a..=b, "");
                } else {
                    break;
                }
            }
            let t = t.trim().to_string();
            if !t.is_empty() {
                out.push(ParsedCue { start, end, text: t });
            }
        }
    }
    out
}

/// Spreads subtitle cues into word-level timings (even split by characters),
/// for importing an existing transcript.
pub fn parsed_cues_to_words(cues: &[ParsedCue]) -> Vec<TranscriptWord> {
    let mut words = Vec::new();
    for c in cues {
        let ws: Vec<&str> = c.text.split_whitespace().collect();
        let total: usize = ws.iter().map(|w| w.chars().count().max(1)).sum();
        if total == 0 {
            continue;
        }
        let mut t = c.start;
        for w in ws {
            let d = (c.end - c.start) * w.chars().count().max(1) as f64 / total as f64;
            words.push(TranscriptWord { text: w.to_string(), start: t, end: t + d, speaker: None, confidence: 1.0 });
            t += d;
        }
    }
    words
}

// ---------------------------------------------------------------------------
// ASS (for burn-in through ffmpeg's `subtitles`/`ass` filter)
// ---------------------------------------------------------------------------

fn ass_color(c: Rgba) -> String {
    // &HAABBGGRR, alpha inverted (00 = opaque).
    format!("&H{:02X}{:02X}{:02X}{:02X}", 255 - c.a, c.b, c.g, c.r)
}

fn ass_ts(t: Secs) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, (cs / 6000) % 60, (cs / 100) % 60, cs % 100)
}

fn ass_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('{', "(").replace('}', ")")
}

/// Renders caption cues as an ASS subtitle script sized for the composition.
pub fn to_ass(comp: &Composition, cues: &[Cue]) -> String {
    let st = &comp.captions;
    let (w, h) = (comp.width as f64, comp.height as f64);
    let font_px = (st.size * h).round().max(8.0);
    let (outline_color, outline_w) = st.outline.unwrap_or((Rgba::BLACK, 0.0));
    let boxed = st.background.is_some();
    let back = st.background.unwrap_or(Rgba::rgba(0, 0, 0, 0));
    // Alignment 2 = bottom centre; position with MarginV from the bottom.
    let margin_v = ((1.0 - st.y) * h - font_px * 0.6).max(0.0).round();
    let mut s = String::new();
    s.push_str("[Script Info]\nScriptType: v4.00+\nWrapStyle: 2\nScaledBorderAndShadow: yes\n");
    s.push_str(&format!("PlayResX: {}\nPlayResY: {}\n\n", w as u32, h as u32));
    s.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    s.push_str(&format!(
        "Style: Cap,{},{},{},{},{},{},{},0,0,0,100,100,0,0,{},{},0,2,{},{},{},1\n\n",
        st.font,
        font_px,
        ass_color(st.color),
        ass_color(st.color),
        if boxed { ass_color(back) } else { ass_color(outline_color) },
        ass_color(back),
        if st.bold { -1 } else { 0 },
        if boxed { 3 } else { 1 },
        if boxed { (font_px * 0.18).round() } else { outline_w },
        (w * 0.05).round(),
        (w * 0.05).round(),
        margin_v,
    ));
    s.push_str("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    let hi = ass_color(st.highlight_color);
    let base = ass_color(st.color);
    for c in cues {
        match st.highlight {
            CaptionHighlight::None => {
                let text = c.lines.iter().map(|l| ass_escape(l)).collect::<Vec<_>>().join("\\N");
                s.push_str(&format!("Dialogue: 0,{},{},Cap,,0,0,0,,{}\n", ass_ts(c.start), ass_ts(c.end), text));
            }
            CaptionHighlight::ActiveWord | CaptionHighlight::Reveal => {
                // One event per word interval, with the cue text re-coloured.
                let line_of: Vec<usize> = line_index_per_word(c);
                for (wi, word) in c.words.iter().enumerate() {
                    let start = if wi == 0 { c.start } else { word.start };
                    let end = if wi + 1 < c.words.len() { c.words[wi + 1].start } else { c.end };
                    if end - start <= 0.0 {
                        continue;
                    }
                    let mut text = String::new();
                    for (k, w2) in c.words.iter().enumerate() {
                        if k > 0 {
                            text.push_str(if line_of[k] != line_of[k - 1] { "\\N" } else { " " });
                        }
                        let esc = ass_escape(&w2.text);
                        match st.highlight {
                            CaptionHighlight::ActiveWord if k == wi => text.push_str(&format!("{{\\c{hi}&}}{esc}{{\\c{base}&}}")),
                            CaptionHighlight::Reveal if k > wi => text.push_str(&format!("{{\\alpha&HFF&}}{esc}{{\\alpha&H00&}}")),
                            _ => text.push_str(&esc),
                        }
                    }
                    s.push_str(&format!("Dialogue: 0,{},{},Cap,,0,0,0,,{}\n", ass_ts(start), ass_ts(end), text));
                }
            }
        }
    }
    s
}

fn line_index_per_word(c: &Cue) -> Vec<usize> {
    let mut out = Vec::with_capacity(c.words.len());
    let mut line = 0usize;
    let mut remaining = c.lines.first().map(|l| l.split_whitespace().count()).unwrap_or(0);
    for _ in &c.words {
        while remaining == 0 && line + 1 < c.lines.len() {
            line += 1;
            remaining = c.lines[line].split_whitespace().count();
        }
        out.push(line);
        remaining = remaining.saturating_sub(1);
    }
    out
}
