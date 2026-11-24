//! Blocking client for the Claude Messages API.
//!
//! Every request is streamed (server-sent events), so long generations never
//! hit an idle HTTP timeout and cancellation takes effect within ~100 ms. The
//! stream is assembled back into a complete message before anything acts on
//! it. Requests are retried with backoff on 429 / 5xx / overloaded / network
//! errors (honouring `retry-after`), never on other 4xx.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The most capable generally available model. Used by the assistant and the
/// judgement-heavy one-shot tools unless overridden with [`Client::with_model`].
pub const DEFAULT_MODEL: &str = "claude-fable-5-2";
/// Cheap, fast model used for mechanical work (translation).
pub const FAST_MODEL: &str = "claude-haiku-5-5";

const API_VERSION: &str = "2026-10-21";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// A `retry-after` longer than this is reported to the user instead of slept through.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

/// Errors, typed so the UI can show a useful message. `Display` is written
/// for end users. No variant ever contains the API key.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AiError {
    #[error("No Anthropic API key is set. Add your key in Settings.")]
    MissingKey,
    #[error("The Anthropic API key was rejected. Check the key in Settings.")]
    InvalidKey,
    #[error("The Claude API is rate limiting this key. Try again in a moment.")]
    RateLimited,
    #[error("Claude is overloaded right now. Try again in a moment.")]
    Overloaded,
    #[error("Could not reach the Claude API: {0}")]
    Network(String),
    /// The API rejected the request (400, 402, 403, 404, 413 …). Carries the API's message.
    #[error("The Claude API rejected the request: {0}")]
    BadRequest(String),
    /// The model declined to answer (`stop_reason: "refusal"`).
    #[error("Claude declined this request.")]
    Refused,
    #[error("Cancelled.")]
    Cancelled,
    /// 5xx other than overloaded, after retries.
    #[error("The Claude API had a server error: {0}")]
    Server(String),
    /// The response was not a valid Messages API response.
    #[error("Unexpected response from the Claude API: {0}")]
    Malformed(String),
    /// The model answered, but the answer failed validation against the project.
    #[error("The model's answer could not be used: {0}")]
    InvalidOutput(String),
    /// The request cannot be made on this project (for example, the script is empty).
    #[error("{0}")]
    NothingToDo(String),
}

/// How much the model thinks before answering (`output_config.effort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }
}

/// Token usage, summed over the requests of one operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub requests: u32,
    /// Uncached input tokens.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    /// Rough cost in US dollars at list prices (as of 2026-11), or `None` for
    /// a model this crate has no price for.
    pub fn estimated_cost_usd(&self, model: &str) -> Option<f64> {
        // (input, output, cache read) per million tokens; 5-minute cache writes cost 1.25x input.
        let (i, o, r) = match model {
            m if m.starts_with("claude-fable-5-2") || m.starts_with("claude-mythos-5-2") => (8.0, 40.0, 0.20),
            m if m.starts_with("claude-opus-5-5") => (4.0, 20.0, 0.20),
            m if m.starts_with("claude-sonnet-5-5") => (2.0, 10.0, 0.10),
            m if m.starts_with("claude-haiku-5-5") => (0.10, 0.50, 0.01),
            _ => return None,
        };
        Some(
            (self.input_tokens as f64 * i
                + self.output_tokens as f64 * o
                + self.cache_creation_input_tokens as f64 * i * 1.25
                + self.cache_read_input_tokens as f64 * r)
                / 1_000_000.0,
        )
    }
}

#[derive(Clone)]
pub struct Client {
    api_key: String,
    model: String,
    fast_model: String,
    base_url: String,
    max_retries: u32,
    retry_base: Duration,
    effort: Option<Effort>,
    cancel: Option<Arc<AtomicBool>>,
    /// Set once the API has told us this key cannot use server-side fallbacks.
    fallbacks_unavailable: Arc<AtomicBool>,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is deliberately not printed.
        f.debug_struct("Client")
            .field("model", &self.model)
            .field("fast_model", &self.fast_model)
            .field("base_url", &self.base_url)
            .field("api_key", &if self.api_key.is_empty() { "<unset>" } else { "<redacted>" })
            .finish()
    }
}

impl Client {
    pub fn new(api_key: impl Into<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            // Time until response headers arrive. The stream opens promptly even when the model thinks for long.
            .timeout_recv_response(Some(Duration::from_secs(180)))
            // Upper bound for one whole streamed response.
            .timeout_recv_body(Some(Duration::from_secs(20 * 60)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            api_key: api_key.into().trim().to_string(),
            model: DEFAULT_MODEL.to_string(),
            fast_model: FAST_MODEL.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: 3,
            retry_base: Duration::from_millis(800),
            effort: None,
            cancel: None,
            fallbacks_unavailable: Arc::new(AtomicBool::new(false)),
            agent,
        }
    }

    /// Reads `ANTHROPIC_API_KEY`. Fails with [`AiError::MissingKey`] when it is unset or empty.
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(k) if !k.trim().is_empty() => Ok(Self::new(k)),
            _ => Err(AiError::MissingKey.into()),
        }
    }

    /// Model for the assistant and the judgement-heavy tools. Default [`DEFAULT_MODEL`].
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.trim().to_string();
        self
    }

    /// Model for mechanical tools (translation). Default [`FAST_MODEL`].
    pub fn with_fast_model(mut self, model: &str) -> Self {
        self.fast_model = model.trim().to_string();
        self
    }

    /// Points the client at another server (tests use a local mock).
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.trim().trim_end_matches('/').to_string();
        self
    }

    /// Overrides the per-operation default effort for every request.
    pub fn with_effort(mut self, effort: Option<Effort>) -> Self {
        self.effort = effort;
        self
    }

    /// Retries after the first attempt on 429 / 5xx / overloaded / network errors. Default 3.
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// First backoff delay; doubles each retry. A `retry-after` header takes precedence.
    pub fn with_retry_base_delay(mut self, d: Duration) -> Self {
        self.retry_base = d;
        self
    }

    /// A flag that aborts any request made through this client (and its
    /// clones) when set. This is how the UI cancels a one-shot tool.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn fast_model(&self) -> &str {
        &self.fast_model
    }
    pub(crate) fn effort_override(&self) -> Option<Effort> {
        self.effort
    }
    pub(crate) fn is_official_endpoint(&self) -> bool {
        self.base_url == DEFAULT_BASE_URL
    }

    fn cancelled(&self, extra: Option<&AtomicBool>) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) || extra.is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// Sends one Messages API request, retrying as described in the module
    /// docs. `on_text` receives text deltas as they stream.
    pub(crate) fn send(&self, req: &Request<'_>, on_text: &mut dyn FnMut(&str), cancel: Option<&AtomicBool>) -> Result<Message, AiError> {
        if self.api_key.is_empty() {
            return Err(AiError::MissingKey);
        }
        let mut attempt: u32 = 0;
        loop {
            if self.cancelled(cancel) {
                return Err(AiError::Cancelled);
            }
            let mut emitted = false;
            let use_fallbacks = self.is_official_endpoint() && supports_fallbacks(req.model) && !self.fallbacks_unavailable.load(Ordering::SeqCst);
            let result = self.send_once(
                req,
                use_fallbacks,
                &mut |t: &str| {
                    emitted = true;
                    on_text(t)
                },
                cancel,
            );
            let failure = match result {
                Ok(m) => return Ok(m),
                Err(f) => f,
            };
            if use_fallbacks && failure.beta_rejected {
                // This key cannot use the fallback beta: carry on without it.
                self.fallbacks_unavailable.store(true, Ordering::SeqCst);
                continue;
            }
            // Never replay a request whose text has already been shown to the user.
            if !failure.retryable || emitted || attempt >= self.max_retries {
                return Err(failure.error);
            }
            let delay = failure.retry_after.unwrap_or_else(|| backoff(self.retry_base, attempt));
            if delay > MAX_RETRY_WAIT {
                return Err(failure.error);
            }
            self.sleep(delay, cancel)?;
            attempt += 1;
        }
    }

    fn sleep(&self, d: Duration, cancel: Option<&AtomicBool>) -> Result<(), AiError> {
        let until = Instant::now() + d;
        loop {
            if self.cancelled(cancel) {
                return Err(AiError::Cancelled);
            }
            let now = Instant::now();
            if now >= until {
                return Ok(());
            }
            std::thread::sleep((until - now).min(Duration::from_millis(50)));
        }
    }

    fn send_once(&self, req: &Request<'_>, use_fallbacks: bool, on_text: &mut dyn FnMut(&str), cancel: Option<&AtomicBool>) -> Result<Message, Failure> {
        let body = req.body(use_fallbacks).to_string();
        let url = format!("{}/v1/messages", self.base_url);
        let mut http = self
            .agent
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream, application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION);
        if use_fallbacks {
            http = http.header("anthropic-beta", FALLBACK_BETA);
        }
        let resp = http.send(body.as_bytes()).map_err(|e| Failure::retryable(AiError::Network(describe_transport_error(&e))))?;

        let status = resp.status().as_u16();
        let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        if !(200..300).contains(&status) {
            let retry_after = header("retry-after").and_then(|v| v.trim().parse::<f64>().ok()).filter(|s| s.is_finite() && *s >= 0.0).map(Duration::from_secs_f64);
            let should_retry = header("x-should-retry");
            let text = resp.into_body().with_config().limit(256 * 1024).read_to_string().unwrap_or_default();
            return Err(classify_http_error(status, retry_after, should_retry.as_deref(), &text));
        }
        let is_json = header("content-type").is_some_and(|c| c.to_ascii_lowercase().contains("application/json"));
        let reader = resp.into_body().into_reader();
        if is_json {
            let mut text = String::new();
            reader.take(64 * 1024 * 1024).read_to_string(&mut text).map_err(|e| Failure::retryable(AiError::Network(e.to_string())))?;
            let v: Value = serde_json::from_str(&text).map_err(|_| Failure::fatal(AiError::Malformed("the response was not JSON".into())))?;
            return message_from_json(&v, on_text);
        }
        self.read_stream(reader, on_text, cancel)
    }

    /// Reads an SSE stream on a helper thread so the caller can notice
    /// cancellation even while the socket is silent.
    fn read_stream(&self, reader: impl Read + Send + 'static, on_text: &mut dyn FnMut(&str), cancel: Option<&AtomicBool>) -> Result<Message, Failure> {
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(reader);
            loop {
                let mut line = String::new();
                match lines.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            break; // the receiver gave up (cancel or error): dropping the reader closes the connection
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        });

        let mut asm = Assembler::default();
        let mut data = String::new();
        loop {
            if self.cancelled(cancel) {
                return Err(Failure::fatal(AiError::Cancelled));
            }
            let line = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => return Err(Failure::retryable(AiError::Network(e))),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !data.is_empty() {
                    let done = asm.event(&data, on_text)?;
                    data.clear();
                    if done {
                        return asm.finish();
                    }
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(d.trim_start());
            }
            // `event:` lines, comments and ids are not needed: every data payload carries its own `type`.
        }
        if !data.is_empty() && asm.event(&data, on_text)? {
            return asm.finish();
        }
        if asm.started {
            Err(Failure::retryable(AiError::Network("the response ended before it was complete".into())))
        } else {
            Err(Failure::fatal(AiError::Malformed("the response was not a Messages API stream".into())))
        }
    }
}

fn supports_fallbacks(model: &str) -> bool {
    ["claude-fable-", "claude-mythos-", "claude-opus-5", "claude-sonnet-5-5"].iter().any(|p| model.starts_with(p))
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    let exp = base.saturating_mul(1u32 << attempt.min(6));
    // Up to 25% jitter, from the clock (no RNG dependency needed for this).
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    exp + exp.mul_f64((nanos % 1000) as f64 / 4000.0)
}

fn describe_transport_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "the request timed out".to_string(),
        ureq::Error::HostNotFound => "the server could not be found (are you offline?)".to_string(),
        ureq::Error::ConnectionFailed => "the connection failed (are you offline?)".to_string(),
        other => other.to_string(),
    }
}

struct Failure {
    error: AiError,
    retryable: bool,
    retry_after: Option<Duration>,
    beta_rejected: bool,
}

impl Failure {
    fn fatal(error: AiError) -> Self {
        Self { error, retryable: false, retry_after: None, beta_rejected: false }
    }
    fn retryable(error: AiError) -> Self {
        Self { error, retryable: true, retry_after: None, beta_rejected: false }
    }
}

fn api_error_parts(body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("").to_string();
    let mut message = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string();
    if message.is_empty() {
        message = body.trim().chars().take(300).collect();
    }
    (kind, message)
}

fn classify_http_error(status: u16, retry_after: Option<Duration>, should_retry: Option<&str>, body: &str) -> Failure {
    let (kind, message) = api_error_parts(body);
    let mut f = classify_api_error(status, &kind, &message);
    f.retry_after = retry_after;
    if status == 429 {
        // A spend-cap 429 does not clear by waiting.
        let spend_cap = body.contains("enforced_spend_limit_reached");
        if spend_cap {
            f.retryable = false;
        }
    }
    match should_retry {
        Some("false") => f.retryable = false,
        Some("true") => f.retryable = true,
        _ => {}
    }
    f
}

fn classify_api_error(status: u16, kind: &str, message: &str) -> Failure {
    let with_status = |m: &str| if m.is_empty() { format!("HTTP {status}") } else { m.to_string() };
    if kind == "overloaded_error" || status == 529 {
        return Failure::retryable(AiError::Overloaded);
    }
    if kind == "rate_limit_error" || status == 429 {
        return Failure::retryable(AiError::RateLimited);
    }
    if kind == "authentication_error" || status == 401 {
        return Failure::fatal(AiError::InvalidKey);
    }
    if status >= 500 || kind == "api_error" || kind == "timeout_error" {
        return Failure::retryable(AiError::Server(with_status(message)));
    }
    if status == 408 || status == 409 {
        return Failure::retryable(AiError::Server(with_status(message)));
    }
    let mut f = Failure::fatal(AiError::BadRequest(with_status(message)));
    f.beta_rejected = message.contains("anthropic-beta") || message.contains("fallbacks");
    f
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// One Messages API request. `system` and `tools` must stay byte-identical
/// across the requests of a conversation: they are the cached prefix, and
/// thinking blocks are only valid while they do not change.
pub(crate) struct Request<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub tools: Option<&'a Value>,
    pub messages: &'a [Value],
    pub max_tokens: u32,
    pub effort: Option<Effort>,
    /// JSON schema for structured output (`output_config.format`).
    pub schema: Option<&'a Value>,
    /// Forbid tool calls for this request (used to close a turn).
    pub no_tools: bool,
    /// Cache the conversation up to its last block (multi-turn conversations).
    pub cache_tail: bool,
    /// Keep the tools + system prefix cached for an hour instead of five minutes.
    pub long_lived_prefix: bool,
}

impl Request<'_> {
    fn body(&self, use_fallbacks: bool) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "stream": true,
            "messages": self.messages,
        });
        if self.cache_tail {
            // Automatic caching of the growing conversation tail.
            body["cache_control"] = json!({"type": "ephemeral"});
        }
        if !self.system.is_empty() {
            // Explicit breakpoint: tools + system are the stable prefix shared by every request.
            let mut marker = json!({"type": "ephemeral"});
            if self.long_lived_prefix {
                // A chat is used on and off for a whole editing session: with the default 5-minute lifetime almost
                // every turn would pay to write the large tools + system prefix again. One hour costs more per write
                // (2x instead of 1.25x) but is then read for the rest of the session.
                marker["ttl"] = json!("1h");
            }
            body["system"] = json!([{"type": "text", "text": self.system, "cache_control": marker}]);
        }
        if let Some(tools) = self.tools {
            body["tools"] = tools.clone();
            if self.no_tools {
                body["tool_choice"] = json!({"type": "none"});
            }
        }
        let mut output_config = serde_json::Map::new();
        if let Some(e) = self.effort {
            output_config.insert("effort".into(), json!(e.as_str()));
        }
        if let Some(schema) = self.schema {
            output_config.insert("format".into(), json!({"type": "json_schema", "schema": schema}));
        }
        if !output_config.is_empty() {
            body["output_config"] = Value::Object(output_config);
        }
        if use_fallbacks {
            body["fallbacks"] = json!("default");
        }
        body
    }
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct Message {
    /// Content blocks exactly as the API produced them (thinking blocks included), for echoing back.
    pub content: Vec<Value>,
    pub stop_reason: String,
    pub usage: Usage,
    /// Tool-use blocks (by index into `content`) whose input was not valid JSON, with the raw text.
    pub bad_tool_inputs: HashMap<usize, String>,
}

impl Message {
    /// Index of the first block produced by the model that finally answered
    /// (after the last server-side `fallback` boundary, if any).
    pub fn live_from(&self) -> usize {
        self.content.iter().rposition(|b| b["type"] == "fallback").map(|i| i + 1).unwrap_or(0)
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for b in &self.content[self.live_from()..] {
            if b["type"] == "text"
                && let Some(t) = b["text"].as_str() {
                    s.push_str(t);
                }
        }
        s
    }

    /// Content to send back as the assistant turn. After a mid-output
    /// fallback, thinking and tool-use blocks before the boundary must be
    /// omitted; text and the `fallback` block itself stay in place.
    pub fn echo_content(&self) -> Vec<Value> {
        let live = self.live_from();
        self.content
            .iter()
            .enumerate()
            .filter(|(i, b)| *i >= live || b["type"] == "text" || b["type"] == "fallback")
            .map(|(_, b)| b.clone())
            .collect()
    }
}

fn usage_from(v: &Value, into: &mut Usage) {
    let get = |k: &str| v.get(k).and_then(Value::as_u64);
    if let Some(n) = get("input_tokens") {
        into.input_tokens = n;
    }
    if let Some(n) = get("output_tokens") {
        into.output_tokens = n;
    }
    if let Some(n) = get("cache_creation_input_tokens") {
        into.cache_creation_input_tokens = n;
    }
    if let Some(n) = get("cache_read_input_tokens") {
        into.cache_read_input_tokens = n;
    }
}

fn stream_error(v: &Value) -> Failure {
    let kind = v.pointer("/error/type").and_then(Value::as_str).unwrap_or("");
    let message = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
    let status = match kind {
        "overloaded_error" => 529,
        "rate_limit_error" => 429,
        "authentication_error" => 401,
        "invalid_request_error" => 400,
        _ => 500,
    };
    classify_api_error(status, kind, message)
}

/// A complete (non-streamed) message, as some proxies and the test server return.
fn message_from_json(v: &Value, on_text: &mut dyn FnMut(&str)) -> Result<Message, Failure> {
    if v["type"] == "error" {
        return Err(stream_error(v));
    }
    let (Some(content), Some(stop_reason)) = (v["content"].as_array(), v["stop_reason"].as_str()) else {
        return Err(Failure::fatal(AiError::Malformed("the response was not a Messages API message".into())));
    };
    let mut m = Message { content: content.clone(), stop_reason: stop_reason.to_string(), ..Default::default() };
    for (i, b) in m.content.iter_mut().enumerate() {
        if !b.is_object() || !b["type"].is_string() {
            return Err(Failure::fatal(AiError::Malformed("the response contained an invalid content block".into())));
        }
        if b["type"] == "tool_use" && !b["input"].is_object() {
            m.bad_tool_inputs.insert(i, b["input"].to_string());
            b["input"] = json!({});
        }
    }
    usage_from(&v["usage"], &mut m.usage);
    m.usage.requests = 1;
    let text = m.text();
    if !text.is_empty() {
        on_text(&text);
    }
    Ok(m)
}

/// Rebuilds a message from stream events.
#[derive(Default)]
struct Assembler {
    started: bool,
    msg: Message,
    /// Accumulated `input_json_delta` text per content index.
    partial: HashMap<usize, String>,
}

impl Assembler {
    /// Handles one `data:` payload. Returns true on `message_stop`.
    fn event(&mut self, data: &str, on_text: &mut dyn FnMut(&str)) -> Result<bool, Failure> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return Err(Failure::fatal(AiError::Malformed("the response stream contained invalid JSON".into())));
        };
        let malformed = || Failure::fatal(AiError::Malformed("the response stream was out of order".into()));
        match v["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.started = true;
                usage_from(&v["message"]["usage"], &mut self.msg.usage);
            }
            "content_block_start" => {
                let index = v["index"].as_u64().ok_or_else(malformed)? as usize;
                if !self.started || index != self.msg.content.len() || !v["content_block"].is_object() {
                    return Err(malformed());
                }
                let block = v["content_block"].clone();
                if block["type"] == "text" {
                    // After a server-side fallback, later text continues the reply.
                    if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                        on_text(t);
                    }
                }
                self.msg.content.push(block);
            }
            "content_block_delta" => {
                let index = v["index"].as_u64().ok_or_else(malformed)? as usize;
                let block = self.msg.content.get_mut(index).ok_or_else(malformed)?;
                let delta = &v["delta"];
                let append = |block: &mut Value, key: &str, add: &str| {
                    let cur = block[key].as_str().unwrap_or("").to_string();
                    block[key] = Value::String(cur + add);
                };
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let t = delta["text"].as_str().unwrap_or("");
                        append(block, "text", t);
                        if !t.is_empty() {
                            on_text(t);
                        }
                    }
                    "input_json_delta" => self.partial.entry(index).or_default().push_str(delta["partial_json"].as_str().unwrap_or("")),
                    "thinking_delta" => append(block, "thinking", delta["thinking"].as_str().unwrap_or("")),
                    "signature_delta" => append(block, "signature", delta["signature"].as_str().unwrap_or("")),
                    "citations_delta" => {
                        if !block["citations"].is_array() {
                            block["citations"] = json!([]);
                        }
                        if let Some(a) = block["citations"].as_array_mut() {
                            a.push(delta["citation"].clone());
                        }
                    }
                    _ => {} // unknown delta kinds are ignored
                }
            }
            "content_block_stop" => {
                let index = v["index"].as_u64().ok_or_else(malformed)? as usize;
                let block = self.msg.content.get_mut(index).ok_or_else(malformed)?;
                if block["type"] == "tool_use" {
                    let raw = self.partial.remove(&index).unwrap_or_default();
                    if !raw.trim().is_empty() {
                        match serde_json::from_str::<Value>(&raw) {
                            Ok(input) if input.is_object() => block["input"] = input,
                            _ => {
                                block["input"] = json!({});
                                self.msg.bad_tool_inputs.insert(index, raw);
                            }
                        }
                    } else if !block["input"].is_object() {
                        block["input"] = json!({});
                    }
                }
            }
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.msg.stop_reason = r.to_string();
                }
                usage_from(&v["usage"], &mut self.msg.usage);
            }
            "message_stop" => return Ok(true),
            "error" => return Err(stream_error(&v)),
            _ => {} // ping and unknown event types
        }
        Ok(false)
    }

    fn finish(mut self) -> Result<Message, Failure> {
        if !self.started || self.msg.stop_reason.is_empty() {
            return Err(Failure::fatal(AiError::Malformed("the response stream was incomplete".into())));
        }
        // Blocks left open (stream cut by max_tokens mid tool input) keep whatever was parsed.
        for (index, raw) in self.partial.drain() {
            if let Some(block) = self.msg.content.get_mut(index) {
                block["input"] = json!({});
                self.msg.bad_tool_inputs.insert(index, raw);
            }
        }
        self.msg.usage.requests = 1;
        Ok(self.msg)
    }
}

/// Convenience for single-shot text requests.
pub(crate) struct Ask<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a [Value],
    pub max_tokens: u32,
    pub effort: Effort,
    pub schema: Option<&'a Value>,
}

impl Client {
    /// Runs a tool-less request and returns the reply text.
    pub(crate) fn ask(&self, a: &Ask<'_>) -> Result<String, AiError> {
        let req = Request {
            model: a.model,
            system: a.system,
            tools: None,
            messages: a.messages,
            max_tokens: a.max_tokens,
            effort: Some(self.effort.unwrap_or(a.effort)),
            schema: a.schema,
            no_tools: false,
            cache_tail: false,
            long_lived_prefix: false,
        };
        let msg = self.send(&req, &mut |_| {}, None)?;
        match msg.stop_reason.as_str() {
            "refusal" => Err(AiError::Refused),
            "max_tokens" => Err(AiError::InvalidOutput("the answer was too long and was cut off".into())),
            _ => {
                let text = msg.text();
                if text.trim().is_empty() { Err(AiError::Malformed("the model returned an empty answer".into())) } else { Ok(text) }
            }
        }
    }
}
