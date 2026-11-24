//! Timeline interchange: hand the cut to another editor.
//!
//! Only the script track's cuts and markers travel; effects, layers and
//! captions do not exist in these formats in a portable way.

use crate::model::*;
use crate::script::ScriptClip;

fn frames(t: Secs, fps: f64) -> i64 {
    (t * fps).round() as i64
}

/// Non-drop-frame timecode `HH:MM:SS:FF` at the nearest integer frame rate.
pub fn timecode(t: Secs, fps: f64) -> String {
    let base = fps.round().max(1.0) as i64;
    let f = frames(t.max(0.0), fps);
    format!("{:02}:{:02}:{:02}:{:02}", f / (base * 3600), (f / (base * 60)) % 60, (f / base) % 60, f % base)
}

fn reel(name: &str) -> String {
    let r: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>().to_uppercase();
    if r.is_empty() { "AX".into() } else { r }
}

/// CMX3600 EDL of the script track. Source timecodes assume each file starts
/// at 00:00:00:00.
pub fn to_edl(project: &Project, comp: &Composition) -> String {
    let fps = comp.fps;
    let mut s = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", comp.name.replace('\n', " "));
    let mut n = 0;
    for clip in comp.script_clips() {
        let (rs, re) = (clip.comp_start, clip.comp_end());
        if frames(re, fps) <= frames(rs, fps) {
            continue;
        }
        n += 1;
        let asset = clip.asset.and_then(|id| project.asset(id));
        let (name, track) = match asset {
            Some(a) => (reel(&a.name), if a.has_video && a.has_audio { "AA/V" } else if a.has_video { "V" } else { "AA" }),
            None => ("BL".to_string(), "V"),
        };
        // Keep source and record lengths identical in frames.
        let len = frames(re, fps) - frames(rs, fps);
        let src_in = frames(clip.src_start, fps);
        let tc = |f: i64| timecode(f as f64 / fps, fps);
        s.push_str(&format!("{:03}  {:<8} {:<5} C        {} {} {} {}\n", n, name, track, tc(src_in), tc(src_in + len), tc(frames(rs, fps)), tc(frames(rs, fps) + len)));
        if let Some(a) = asset {
            s.push_str(&format!("* FROM CLIP NAME: {}\n", a.name));
            s.push_str(&format!("* SOURCE FILE: {}\n", a.path.display()));
        }
    }
    for (_, t, name) in comp.script_markers() {
        s.push_str(&format!("* MARKER {} {}\n", timecode(t, fps), name.replace('\n', " ")));
    }
    s
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Rational time in FCPXML form, on the composition's frame grid.
fn rt(t: Secs, fps: f64) -> String {
    // 30000/1001-style rates keep an exact timebase; others use fps*100.
    let (num, den) = frame_duration(fps);
    let f = frames(t, fps);
    format!("{}/{}s", f * num, den)
}

/// `(numerator, denominator)` of one frame's duration in seconds.
fn frame_duration(fps: f64) -> (i64, i64) {
    for (rate, nd) in [(23.976, (1001, 24000)), (29.97, (1001, 30000)), (59.94, (1001, 60000))] {
        if (fps - rate).abs() < 0.01 {
            return nd;
        }
    }
    (100, (fps * 100.0).round().max(1.0) as i64)
}

fn file_url(p: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// FCPXML 1.9 project of the script track, with markers.
pub fn to_fcpxml(project: &Project, comp: &Composition) -> String {
    let fps = comp.fps;
    let (num, den) = frame_duration(fps);
    let clips: Vec<ScriptClip> = comp.script_clips().into_iter().filter(|c| frames(c.comp_end(), fps) > frames(c.comp_start, fps)).collect();
    let mut used: Vec<AssetId> = Vec::new();
    for c in &clips {
        if let Some(id) = c.asset {
            if !used.contains(&id) {
                used.push(id);
            }
        }
    }
    let total = clips.last().map(|c| c.comp_end()).unwrap_or(0.0);
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.9\">\n  <resources>\n");
    s.push_str(&format!(
        "    <format id=\"r1\" name=\"FFVideoFormat{}x{}\" frameDuration=\"{}/{}s\" width=\"{}\" height=\"{}\"/>\n",
        comp.width, comp.height, num, den, comp.width, comp.height
    ));
    for (i, id) in used.iter().enumerate() {
        if let Some(a) = project.asset(*id) {
            s.push_str(&format!(
                "    <asset id=\"a{}\" name=\"{}\" start=\"0s\" duration=\"{}\" hasVideo=\"{}\" hasAudio=\"{}\" format=\"r1\" audioSources=\"1\" audioChannels=\"{}\" audioRate=\"{}\">\n      <media-rep kind=\"original-media\" src=\"{}\"/>\n    </asset>\n",
                i + 1,
                xml(&a.name),
                rt(a.duration, fps),
                if a.has_video { 1 } else { 0 },
                if a.has_audio { 1 } else { 0 },
                a.channels.max(1),
                if a.sample_rate > 0 { a.sample_rate } else { 48000 },
                xml(&file_url(&a.path))
            ));
        }
    }
    s.push_str("  </resources>\n  <library>\n");
    s.push_str(&format!("    <event name=\"{}\">\n      <project name=\"{}\">\n", xml(&project.name), xml(&comp.name)));
    s.push_str(&format!("        <sequence format=\"r1\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\" audioLayout=\"stereo\" audioRate=\"48k\">\n          <spine>\n", rt(total, fps)));
    let markers = comp.script_markers();
    for c in &clips {
        let (start, end) = (c.comp_start, c.comp_end());
        let dur_frames = frames(end, fps) - frames(start, fps);
        let dur = format!("{}/{}s", dur_frames * num, den);
        let inner: Vec<String> = markers
            .iter()
            .filter(|(_, t, _)| *t >= start - 1e-9 && *t < end)
            .map(|(_, t, name)| {
                // Marker times are in the clip's own (source) timebase.
                let local = c.src_start + (t - start);
                format!("              <marker start=\"{}\" duration=\"{}/{}s\" value=\"{}\"/>\n", rt(local, fps), num, den, xml(name))
            })
            .collect();
        match c.asset.and_then(|id| used.iter().position(|u| *u == id).map(|i| (i, id))) {
            Some((i, id)) => {
                let name = project.asset(id).map(|a| a.name.clone()).unwrap_or_default();
                let head = format!("            <asset-clip ref=\"a{}\" name=\"{}\" offset=\"{}\" start=\"{}\" duration=\"{}\"", i + 1, xml(&name), rt(start, fps), rt(c.src_start, fps), dur);
                if inner.is_empty() {
                    s.push_str(&format!("{head}/>\n"));
                } else {
                    s.push_str(&format!("{head}>\n{}            </asset-clip>\n", inner.concat()));
                }
            }
            None => s.push_str(&format!("            <gap name=\"Gap\" offset=\"{}\" start=\"0s\" duration=\"{}\"/>\n", rt(start, fps), dur)),
        }
    }
    s.push_str("          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n</fcpxml>\n");
    s
}
