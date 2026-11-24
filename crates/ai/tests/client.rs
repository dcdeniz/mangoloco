//! Offline tests of the HTTP client against a scripted mock of the Messages API.
mod common;

use common::*;
use mango_ai::{AiError, Client, tools};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn ok(text_: &str) -> serde_json::Value {
    message(vec![text(text_)])
}

#[test]
fn retries_429_honouring_retry_after_then_succeeds() {
    let mock = Mock::start(vec![Reply::Status(429, vec![("retry-after", "1".into())], api_error("rate_limit_error", "slow down")), Reply::Json(ok("shorter text"))]);
    let t = Instant::now();
    let out = tools::rewrite(&mock_client(&mock), "some long text", "make it shorter").unwrap();
    assert_eq!(out, "shorter text");
    assert!(t.elapsed() >= Duration::from_millis(950), "waited only {:?}", t.elapsed());
    assert_eq!(mock.requests().len(), 2);
}

#[test]
fn retries_529_overloaded_with_backoff_then_succeeds() {
    let mock = Mock::start(vec![
        Reply::Status(529, vec![], api_error("overloaded_error", "Overloaded")),
        Reply::Status(500, vec![], api_error("api_error", "boom")),
        Reply::Sse(ok("done")),
    ]);
    assert_eq!(tools::rewrite(&mock_client(&mock), "text", "x").unwrap(), "done");
    assert_eq!(mock.requests().len(), 3);
}

#[test]
fn gives_up_after_max_retries_with_a_typed_error() {
    let overloaded = || Reply::Status(529, vec![], api_error("overloaded_error", "Overloaded"));
    let mock = Mock::start(vec![overloaded(), overloaded(), overloaded(), overloaded()]);
    let err = tools::rewrite(&mock_client(&mock).with_max_retries(2), "text", "x").unwrap_err();
    assert_eq!(err, AiError::Overloaded);
    assert_eq!(mock.requests().len(), 3, "first attempt plus two retries");

    let mock = Mock::start(vec![Reply::Status(429, vec![], api_error("rate_limit_error", "x")), Reply::Status(429, vec![], api_error("rate_limit_error", "x"))]);
    assert_eq!(tools::rewrite(&mock_client(&mock).with_max_retries(1), "text", "x").unwrap_err(), AiError::RateLimited);
}

#[test]
fn a_long_retry_after_is_reported_instead_of_slept_through() {
    let mock = Mock::start(vec![Reply::Status(429, vec![("retry-after", "600".into())], api_error("rate_limit_error", "x")), Reply::Json(ok("never"))]);
    let t = Instant::now();
    assert_eq!(tools::rewrite(&mock_client(&mock), "text", "x").unwrap_err(), AiError::RateLimited);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn unauthorized_is_invalid_key_and_not_retried() {
    let mock = Mock::start(vec![Reply::Status(401, vec![], api_error("authentication_error", "invalid x-api-key")), Reply::Json(ok("never"))]);
    assert_eq!(tools::rewrite(&mock_client(&mock), "text", "x").unwrap_err(), AiError::InvalidKey);
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn other_4xx_are_bad_request_with_the_api_message_and_not_retried() {
    for status in [400, 403, 404, 413] {
        let mock = Mock::start(vec![Reply::Status(status, vec![], api_error("invalid_request_error", "max_tokens: too large")), Reply::Json(ok("never"))]);
        let err = tools::rewrite(&mock_client(&mock), "text", "x").unwrap_err();
        assert_eq!(err, AiError::BadRequest("max_tokens: too large".into()), "status {status}");
        assert_eq!(mock.requests().len(), 1);
    }
}

#[test]
fn x_should_retry_false_stops_retries() {
    let mock = Mock::start(vec![Reply::Status(500, vec![("x-should-retry", "false".into())], api_error("api_error", "no")), Reply::Json(ok("never"))]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "text", "x").unwrap_err(), AiError::Server(_)));
    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn missing_key_fails_before_any_request() {
    let mock = Mock::start(vec![Reply::Json(ok("never"))]);
    let client = Client::new("  ").with_base_url(&mock.url);
    assert_eq!(tools::rewrite(&client, "text", "x").unwrap_err(), AiError::MissingKey);
    assert!(mock.requests().is_empty());
}

#[test]
fn malformed_responses_are_clean_errors() {
    // Not JSON at all, labelled as JSON.
    let mock = Mock::start(vec![Reply::Raw("application/json", "<html>gateway</html>".into())]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
    // JSON, but not a message.
    let mock = Mock::start(vec![Reply::Raw("application/json", r#"{"hello":"world"}"#.into())]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
    // A 200 HTML page.
    let mock = Mock::start(vec![Reply::Raw("text/html", "<html>captive portal</html>".into())]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
    // An event stream with a broken event.
    let mock = Mock::start(vec![Reply::Raw("text/event-stream", "event: message_start\ndata: {not json}\n\n".into())]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
    // Content blocks that are not objects.
    let mock = Mock::start(vec![Reply::Json(json!({"content": [42], "stop_reason": "end_turn"}))]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
    // An empty answer.
    let mock = Mock::start(vec![Reply::Json(message(vec![]))]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Malformed(_)));
}

#[test]
fn a_stream_that_ends_early_is_retried_then_reported_as_a_network_error() {
    let cut = || Reply::Raw("text/event-stream", "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{}}}\n\n".into());
    let mock = Mock::start(vec![cut(), Reply::Sse(ok("recovered"))]);
    assert_eq!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap(), "recovered");

    let mock = Mock::start(vec![cut(), cut()]);
    assert!(matches!(tools::rewrite(&mock_client(&mock).with_max_retries(1), "t", "x").unwrap_err(), AiError::Network(_)));
}

#[test]
fn an_error_event_inside_a_stream_is_classified_and_retried() {
    let overloaded = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{}}}\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
    let mock = Mock::start(vec![Reply::Raw("text/event-stream", overloaded.into()), Reply::Sse(ok("fine"))]);
    assert_eq!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap(), "fine");
    assert_eq!(mock.requests().len(), 2);
}

#[test]
fn connection_refused_is_a_network_error() {
    // Bind and drop a listener to get a port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let client = Client::new("k").with_base_url(&format!("http://127.0.0.1:{port}")).with_max_retries(1).with_retry_base_delay(Duration::from_millis(5));
    assert!(matches!(tools::rewrite(&client, "t", "x").unwrap_err(), AiError::Network(_)));
}

#[test]
fn refusal_and_truncation_are_typed() {
    let mut refused = ok("");
    refused["stop_reason"] = json!("refusal");
    let mock = Mock::start(vec![Reply::Json(refused)]);
    assert_eq!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::Refused);

    let mut cut = ok("half an ans");
    cut["stop_reason"] = json!("max_tokens");
    let mock = Mock::start(vec![Reply::Sse(cut)]);
    assert!(matches!(tools::rewrite(&mock_client(&mock), "t", "x").unwrap_err(), AiError::InvalidOutput(_)));
}

#[test]
fn request_shape_headers_and_key_hygiene() {
    let mock = Mock::start(vec![Reply::Sse(ok("hola"))]);
    let client = Client::new("sk-test-SECRET").with_base_url(&mock.url).with_model("model-main").with_fast_model("model-fast");
    assert_eq!(tools::translate_text(&client, "hello", "Spanish").unwrap(), "hola");
    let req = &mock.requests()[0];
    assert_eq!(req.path, "/v1/messages");
    assert_eq!(req.headers["x-api-key"], "sk-test-SECRET");
    assert_eq!(req.headers["anthropic-version"], "2026-10-21");
    assert_eq!(req.body["model"], "model-fast", "translation runs on the fast model");
    assert_eq!(req.body["stream"], true);
    assert!(req.body["max_tokens"].as_u64().unwrap() >= 1024);
    assert_eq!(req.body["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(req.body.get("temperature").is_none() && req.body.get("thinking").is_none());
    assert!(!req.body.to_string().contains("SECRET"), "the key must only travel in the header");
    // The key never shows up in Debug output or in errors.
    assert!(!format!("{client:?}").contains("SECRET"));
    let mock = Mock::start(vec![Reply::Status(401, vec![], api_error("authentication_error", "invalid x-api-key"))]);
    let err = tools::rewrite(&Client::new("sk-test-SECRET").with_base_url(&mock.url), "t", "x").unwrap_err();
    assert!(!format!("{err:?} {err}").contains("SECRET"));
}

#[test]
fn from_env_reports_a_missing_key() {
    // Only checks the unset case when the variable is really unset, to avoid racing other tests.
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        let err = Client::from_env().unwrap_err();
        assert_eq!(err.downcast_ref::<AiError>(), Some(&AiError::MissingKey));
    }
}

#[test]
fn the_client_cancel_flag_aborts_a_stalled_one_shot_request() {
    let mock = Mock::start(vec![Reply::Stall(ok("this never finishes"), Duration::from_secs(20))]);
    let flag = Arc::new(AtomicBool::new(false));
    let client = mock_client(&mock).with_cancel_flag(flag.clone());
    let f2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        f2.store(true, Ordering::SeqCst);
    });
    let t = Instant::now();
    assert_eq!(tools::rewrite(&client, "t", "x").unwrap_err(), AiError::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(3), "cancel took {:?}", t.elapsed());
}
