//! Shared test support: a synthetic podcast project, a scripted mock of the
//! Messages API, and API-key discovery for the live tests.
#![allow(dead_code)]

use mango_core::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Synthetic project
// ---------------------------------------------------------------------------

/// A rambling two-speaker podcast: an intro with filler words and a repeated
/// false start ("today we have, today we have"), a section about sourdough
/// bread, a section about marathon running, and an outro. `[2.2]` is a pause
/// in seconds.
pub const PODCAST: &[(&str, &str)] = &[
    ("Ana", "Um, hello and welcome back to the the Kitchen Table podcast. I'm Ana. So, uh, today we have, today we have a really fun episode for you. [2.2] My guest is Ben, who, um, you know, bakes bread and also runs marathons. Ben, welcome."),
    ("Ben", "Thanks, Ana. Uh, it's great to be here."),
    ("Ana", "Let's start with the bread. How did you get into sourdough?"),
    ("Ben", "So I started baking sourdough, um, about five years ago. My neighbor gave me a jar of starter, and I, I mean, I had no idea what I was doing. The first loaf was basically a brick. [1.8] But you learn. You feed the starter every day with flour and water, and you, uh, you wait. The oven has to be really hot, like two hundred and fifty degrees. Now I bake two loaves every weekend."),
    ("Ana", "That sounds delicious. What is the biggest mistake beginners make with sourdough?"),
    ("Ben", "Honestly, they rush it. Good bread needs time, and, um, a wet dough."),
    ("Ana", "Okay, let's switch gears and talk about running. You ran your first marathon last year, right?"),
    ("Ben", "I did. I ran the Berlin marathon in, uh, in September. Training took about six months. I was running, you know, forty miles a week at the peak. [2.5] The hardest part was not the long runs, it was getting up at five in the morning. And shoes matter. I went through three pairs of shoes during training."),
    ("Ana", "Wow. Would you run another marathon?"),
    ("Ben", "Absolutely. I've already signed up for Chicago next fall."),
    ("Ana", "Amazing. Ben, thank you so much for coming on. And, um, thanks to everyone for listening. See you next week."),
];

/// Builds an editor whose single composition plays a transcribed recording
/// of `turns` (speaker, text). Words are 0.28 s long.
pub fn editor_from_turns(turns: &[(&str, &str)]) -> Editor {
    let mut e = Editor::new_project("Kitchen Table 12");
    let mut t = 0.4;
    let mut words = Vec::new();
    for (speaker, text) in turns {
        let id = if speaker.is_empty() { None } else { Some(e.project.ensure_speaker(speaker)) };
        for w in text.split_whitespace() {
            if let Some(p) = w.strip_prefix('[').and_then(|w| w.strip_suffix(']')) {
                t += p.parse::<f64>().expect("pause");
                continue;
            }
            words.push(TranscriptWord { text: w.to_string(), start: t, end: t + 0.28, speaker: id, confidence: 0.95 });
            t += 0.28
                + if w.ends_with(['.', '?', '!']) {
                    0.45
                } else if w.ends_with(',') {
                    0.18
                } else {
                    0.05
                };
        }
        t += 0.3;
    }
    let asset = MediaAsset {
        id: AssetId::new(),
        name: "episode-12.mp4".into(),
        path: "/tmp/episode-12.mp4".into(),
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
        transcript: Some(Transcript { language: "en".into(), words }),
    };
    let id = asset.id;
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    e
}

pub fn podcast_editor() -> Editor {
    editor_from_turns(PODCAST)
}

/// Lower-cased playing words without punctuation.
pub fn playing_words(e: &Editor) -> Vec<String> {
    e.comp().unwrap().script.iter().filter(|t| t.shows_text()).map(|t| script::normalize_word(&t.text)).collect()
}

pub fn playing_text(e: &Editor) -> String {
    playing_words(e).join(" ")
}

/// Index of the first word token whose normalised text is `w`.
pub fn word_index(e: &Editor, w: &str) -> usize {
    e.comp().unwrap().script.iter().position(|t| t.is_word() && script::normalize_word(&t.text) == w).unwrap_or_else(|| panic!("no word {w}"))
}

/// Number of undo steps on the editor (it has no accessor; this undoes and redoes).
pub fn undo_depth(e: &mut Editor) -> usize {
    let mut n = 0;
    while e.undo() {
        n += 1;
    }
    for _ in 0..n {
        assert!(e.redo());
    }
    n
}

// ---------------------------------------------------------------------------
// Mock Messages API
// ---------------------------------------------------------------------------

pub enum Reply {
    /// 200 with a complete message as `application/json`.
    Json(Value),
    /// 200 with the message as a server-sent event stream.
    Sse(Value),
    /// A non-200 response.
    Status(u16, Vec<(&'static str, String)>, String),
    /// 200 with an arbitrary body.
    Raw(&'static str, String),
    /// Starts a stream, sends the first events of the message, then keeps the connection open silently.
    Stall(Value, Duration),
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

pub struct Mock {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    remaining: Arc<Mutex<std::collections::VecDeque<Reply>>>,
}

impl Mock {
    /// Serves `replies` in order, one per request.
    pub fn start(replies: Vec<Reply>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let remaining = Arc::new(Mutex::new(replies.into_iter().collect::<std::collections::VecDeque<_>>()));
        let (req2, rem2) = (requests.clone(), remaining.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (req3, rem3) = (req2.clone(), rem2.clone());
                std::thread::spawn(move || {
                    let _ = serve(stream, &req3, &rem3);
                });
            }
        });
        Mock { url, requests, remaining }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// Replies that were scripted but never requested.
    pub fn unused(&self) -> usize {
        self.remaining.lock().unwrap().len()
    }
}

fn serve(mut stream: TcpStream, requests: &Mutex<Vec<Recorded>>, remaining: &Mutex<std::collections::VecDeque<Reply>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    requests.lock().unwrap().push(Recorded { path, headers, body });

    let reply = remaining.lock().unwrap().pop_front();
    let write = |stream: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &str| -> std::io::Result<()> {
        let mut out = format!("HTTP/1.1 {status} X\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        out.push_str(body);
        stream.write_all(out.as_bytes())?;
        stream.flush()
    };
    match reply {
        None => write(&mut stream, 500, &[("content-type", "application/json".into())], r#"{"type":"error","error":{"type":"api_error","message":"mock: no more scripted replies"}}"#),
        Some(Reply::Json(v)) => write(&mut stream, 200, &[("content-type", "application/json".into())], &v.to_string()),
        Some(Reply::Status(code, headers, body)) => {
            let mut h: Vec<(&str, String)> = vec![("content-type", "application/json".into())];
            h.extend(headers);
            write(&mut stream, code, &h, &body)
        }
        Some(Reply::Raw(content_type, body)) => write(&mut stream, 200, &[("content-type", content_type.into())], &body),
        Some(Reply::Sse(message)) => write(&mut stream, 200, &[("content-type", "text/event-stream".into())], &sse_body(&message)),
        Some(Reply::Stall(message, hold)) => {
            stream.write_all(b"HTTP/1.1 200 X\r\nconnection: close\r\ncontent-type: text/event-stream\r\n\r\n")?;
            // Everything up to (not including) the end of the message, then silence.
            let body = sse_body(&message);
            let cut = body.find("event: message_delta").unwrap_or(body.len());
            stream.write_all(&body.as_bytes()[..cut])?;
            stream.flush()?;
            std::thread::sleep(hold);
            Ok(())
        }
    }
}

/// Renders a complete message as the event stream the API would send,
/// splitting text and tool input into several deltas.
pub fn sse_body(message: &Value) -> String {
    let mut out = String::new();
    let mut ev = |v: Value| {
        out.push_str(&format!("event: {}\ndata: {}\n\n", v["type"].as_str().unwrap(), v));
    };
    ev(json!({"type": "message_start", "message": {"id": "msg_mock", "type": "message", "role": "assistant", "model": "mock", "content": [], "stop_reason": null,
        "usage": {"input_tokens": message["usage"]["input_tokens"].as_u64().unwrap_or(10), "output_tokens": 1}}}));
    ev(json!({"type": "ping"}));
    let halves = |s: &str| -> Vec<String> {
        let mid = s.char_indices().nth(s.chars().count() / 2).map(|(i, _)| i).unwrap_or(0);
        vec![s[..mid].to_string(), s[mid..].to_string()]
    };
    for (i, block) in message["content"].as_array().unwrap().iter().enumerate() {
        match block["type"].as_str().unwrap() {
            "text" => {
                ev(json!({"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}));
                for part in halves(block["text"].as_str().unwrap()) {
                    ev(json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": part}}));
                }
            }
            "tool_use" => {
                ev(json!({"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": block["id"], "name": block["name"], "input": {}}}));
                let raw = match block.get("raw_input") {
                    Some(r) => r.as_str().unwrap().to_string(), // lets a test send broken JSON
                    None => block["input"].to_string(),
                };
                for part in halves(&raw) {
                    ev(json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": part}}));
                }
            }
            "thinking" => {
                ev(json!({"type": "content_block_start", "index": i, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}));
                ev(json!({"type": "content_block_delta", "index": i, "delta": {"type": "thinking_delta", "thinking": block["thinking"]}}));
                ev(json!({"type": "content_block_delta", "index": i, "delta": {"type": "signature_delta", "signature": block["signature"]}}));
            }
            _ => ev(json!({"type": "content_block_start", "index": i, "content_block": block})),
        }
        ev(json!({"type": "content_block_stop", "index": i}));
    }
    ev(json!({"type": "message_delta", "delta": {"stop_reason": message["stop_reason"]}, "usage": {"output_tokens": message["usage"]["output_tokens"].as_u64().unwrap_or(20)}}));
    ev(json!({"type": "message_stop"}));
    out
}

pub fn text(s: &str) -> Value {
    json!({"type": "text", "text": s})
}

pub fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

/// A complete assistant message.
pub fn message(content: Vec<Value>) -> Value {
    let stop = if content.iter().any(|b| b["type"] == "tool_use") { "tool_use" } else { "end_turn" };
    json!({"id": "msg_mock", "type": "message", "role": "assistant", "model": "mock", "content": content, "stop_reason": stop,
        "usage": {"input_tokens": 100, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}})
}

pub fn api_error(kind: &str, msg: &str) -> String {
    json!({"type": "error", "error": {"type": kind, "message": msg}}).to_string()
}

/// A client pointed at the mock, with fast retries.
pub fn mock_client(mock: &Mock) -> mango_ai::Client {
    mango_ai::Client::new("test-key").with_base_url(&mock.url).with_retry_base_delay(Duration::from_millis(20))
}

/// The text of every tool_result in a recorded request, in order, with its error flag.
pub fn tool_results(req: &Recorded) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for m in req.body["messages"].as_array().unwrap() {
        if let Some(blocks) = m["content"].as_array() {
            for b in blocks {
                if b["type"] == "tool_result" {
                    out.push((b["content"].as_str().unwrap_or("").to_string(), b["is_error"] == true));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Live API
// ---------------------------------------------------------------------------

/// The API key from `ANTHROPIC_API_KEY`, or from a `.env` file found by
/// walking up from the crate directory. Never printed.
pub fn api_key() -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY")
        && !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.join(".env")) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("ANTHROPIC_API_KEY=") {
                    let v = v.trim().trim_matches(['"', '\'']);
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}
