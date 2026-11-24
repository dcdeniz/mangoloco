//! Transcript export: plain text, Markdown, HTML, RTF, and chapter lists.

use crate::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscriptFormat {
    Txt,
    Markdown,
    Html,
    Rtf,
}

impl TranscriptFormat {
    pub fn extension(self) -> &'static str {
        match self {
            TranscriptFormat::Txt => "txt",
            TranscriptFormat::Markdown => "md",
            TranscriptFormat::Html => "html",
            TranscriptFormat::Rtf => "rtf",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptOptions {
    pub format: TranscriptFormat,
    pub include_title: bool,
    pub include_speakers: bool,
    pub include_markers: bool,
    pub include_ignored: bool,
    /// Timecode at the start of each paragraph.
    pub paragraph_timecodes: bool,
    /// Added to every timecode.
    pub timecode_offset: Secs,
}

impl Default for TranscriptOptions {
    fn default() -> Self {
        Self {
            format: TranscriptFormat::Txt,
            include_title: true,
            include_speakers: true,
            include_markers: true,
            include_ignored: false,
            paragraph_timecodes: false,
            timecode_offset: 0.0,
        }
    }
}

pub fn timecode(t: Secs) -> String {
    let s = t.max(0.0).floor() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

enum Block {
    Marker(String),
    Paragraph { speaker: Option<String>, time: Secs, runs: Vec<(String, bool)> }, // (text, ignored)
}

fn blocks(project: &Project, comp: &Composition, o: &TranscriptOptions) -> Vec<Block> {
    let starts = comp.token_starts();
    let mut out = Vec::new();
    let mut last_speaker: Option<Option<SpeakerId>> = None;
    for range in comp.paragraphs() {
        let first = &comp.script[range.start];
        if o.include_markers {
            if let Some(m) = &first.marker {
                out.push(Block::Marker(m.clone()));
            }
        }
        let mut runs: Vec<(String, bool)> = Vec::new();
        let mut speaker = None;
        let mut time = None;
        for i in range.clone() {
            let t = &comp.script[i];
            if let Some(note) = &t.note {
                runs.push((format!("({note})"), false));
            }
            if t.kind != TokenKind::Word || t.hidden_text || (t.ignored && !o.include_ignored) {
                continue;
            }
            if time.is_none() {
                time = Some(starts[i]);
                speaker = t.speaker;
            }
            runs.push((t.text.clone(), t.ignored));
        }
        if runs.is_empty() {
            continue;
        }
        let show_speaker = o.include_speakers && speaker.is_some() && last_speaker != Some(speaker);
        last_speaker = Some(speaker);
        out.push(Block::Paragraph {
            speaker: show_speaker.then(|| project.speaker_name(speaker).to_string()),
            time: time.unwrap_or(starts[range.start]) + o.timecode_offset,
            runs,
        });
    }
    out
}

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn esc_rtf(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\\' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 128 => o.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    o
}

pub fn export_transcript(project: &Project, comp: &Composition, o: &TranscriptOptions) -> String {
    let bl = blocks(project, comp, o);
    let join = |runs: &[(String, bool)], strike: &dyn Fn(&str) -> String| -> String {
        runs.iter().map(|(t, ign)| if *ign { strike(t) } else { t.clone() }).collect::<Vec<_>>().join(" ")
    };
    let mut s = String::new();
    match o.format {
        TranscriptFormat::Txt => {
            if o.include_title {
                s.push_str(&comp.name);
                s.push_str("\n\n");
            }
            for b in &bl {
                match b {
                    Block::Marker(m) => s.push_str(&format!("# {m}\n\n")),
                    Block::Paragraph { speaker, time, runs } => {
                        if let Some(sp) = speaker {
                            s.push_str(sp);
                            s.push_str(":\n");
                        }
                        if o.paragraph_timecodes {
                            s.push_str(&format!("[{}] ", timecode(*time)));
                        }
                        s.push_str(&join(runs, &|t| t.to_string()));
                        s.push_str("\n\n");
                    }
                }
            }
        }
        TranscriptFormat::Markdown => {
            if o.include_title {
                s.push_str(&format!("# {}\n\n", comp.name));
            }
            for b in &bl {
                match b {
                    Block::Marker(m) => s.push_str(&format!("## {m}\n\n")),
                    Block::Paragraph { speaker, time, runs } => {
                        if let Some(sp) = speaker {
                            s.push_str(&format!("**{sp}:** "));
                        }
                        if o.paragraph_timecodes {
                            s.push_str(&format!("`{}` ", timecode(*time)));
                        }
                        s.push_str(&join(runs, &|t| format!("~~{t}~~")));
                        s.push_str("\n\n");
                    }
                }
            }
        }
        TranscriptFormat::Html => {
            s.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>");
            s.push_str(&esc_html(&comp.name));
            s.push_str("</title></head><body>\n");
            if o.include_title {
                s.push_str(&format!("<h1>{}</h1>\n", esc_html(&comp.name)));
            }
            for b in &bl {
                match b {
                    Block::Marker(m) => s.push_str(&format!("<h2>{}</h2>\n", esc_html(m))),
                    Block::Paragraph { speaker, time, runs } => {
                        s.push_str("<p>");
                        if let Some(sp) = speaker {
                            s.push_str(&format!("<strong>{}:</strong> ", esc_html(sp)));
                        }
                        if o.paragraph_timecodes {
                            s.push_str(&format!("<small>{}</small> ", timecode(*time)));
                        }
                        let body = runs
                            .iter()
                            .map(|(t, ign)| if *ign { format!("<s>{}</s>", esc_html(t)) } else { esc_html(t) })
                            .collect::<Vec<_>>()
                            .join(" ");
                        s.push_str(&body);
                        s.push_str("</p>\n");
                    }
                }
            }
            s.push_str("</body></html>\n");
        }
        TranscriptFormat::Rtf => {
            s.push_str("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Helvetica;}}\\f0\\fs24\n");
            if o.include_title {
                s.push_str(&format!("{{\\b\\fs36 {}}}\\par\\par\n", esc_rtf(&comp.name)));
            }
            for b in &bl {
                match b {
                    Block::Marker(m) => s.push_str(&format!("{{\\b\\fs28 {}}}\\par\\par\n", esc_rtf(m))),
                    Block::Paragraph { speaker, time, runs } => {
                        if let Some(sp) = speaker {
                            s.push_str(&format!("{{\\b {}:}} ", esc_rtf(sp)));
                        }
                        if o.paragraph_timecodes {
                            s.push_str(&format!("[{}] ", timecode(*time)));
                        }
                        let body = runs
                            .iter()
                            .map(|(t, ign)| if *ign { format!("{{\\strike {}}}", esc_rtf(t)) } else { esc_rtf(t) })
                            .collect::<Vec<_>>()
                            .join(" ");
                        s.push_str(&body);
                        s.push_str("\\par\\par\n");
                    }
                }
            }
            s.push_str("}\n");
        }
    }
    s
}

/// Chapters from script markers: `(time, title)`. A chapter at 0:00 is added
/// when the first marker starts later, as video platforms require one.
pub fn chapters(comp: &Composition) -> Vec<(Secs, String)> {
    let mut out: Vec<(Secs, String)> = comp.script_markers().into_iter().map(|(_, t, n)| (t, n)).collect();
    for m in &comp.markers {
        if m.chapter {
            out.push((m.time, m.name.clone()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    if out.first().is_some_and(|c| c.0 > 0.5) {
        out.insert(0, (0.0, "Intro".to_string()));
    }
    out
}

/// "0:00 Title" lines for pasting into a video description.
pub fn chapters_text(comp: &Composition) -> String {
    chapters(comp).iter().map(|(t, n)| format!("{} {}", timecode(*t), n)).collect::<Vec<_>>().join("\n")
}

/// ffmpeg FFMETADATA chapter file content.
pub fn chapters_ffmetadata(comp: &Composition, title: Option<&str>) -> String {
    let mut s = String::from(";FFMETADATA1\n");
    if let Some(t) = title {
        s.push_str(&format!("title={}\n", t.replace('\n', " ").replace('=', "\\=")));
    }
    let ch = chapters(comp);
    let total = comp.duration();
    for (i, (t, name)) in ch.iter().enumerate() {
        let end = ch.get(i + 1).map(|c| c.0).unwrap_or(total);
        s.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            (t * 1000.0).round() as i64,
            (end * 1000.0).round() as i64,
            name.replace('\n', " ").replace('=', "\\=")
        ));
    }
    s
}
