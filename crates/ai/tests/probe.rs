//! Manual probe (ignored by default): latency and token use of one assistant
//! turn per model. Run with `cargo test -p mango-ai --test probe -- --ignored --nocapture`.
mod common;

use mango_ai::{Assistant, Client, Effort};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn probe(model: &str, effort: Effort, prompt: &str) {
    let Some(key) = common::api_key() else {
        eprintln!("no API key; skipping");
        return;
    };
    let mut e = common::podcast_editor();
    let mut a = Assistant::new(Client::new(key).with_model(model)).with_effort(effort);
    let t = Instant::now();
    let mut tools = Vec::new();
    let r = a.send(
        &mut e,
        prompt,
        &mut |ev| {
            if let mango_ai::Event::ToolFinished { name, ok, .. } = ev {
                tools.push(format!("{name}{}", if ok { "" } else { "!" }));
            }
        },
        &AtomicBool::new(false),
    );
    match r {
        Ok(r) => eprintln!(
            "PROBE model={model} effort={effort:?} secs={:.1} requests={} in={} out={} cache_write={} cache_read={} cost=${:.4} changed={} tools={tools:?}\n  reply: {}",
            t.elapsed().as_secs_f64(),
            r.usage.requests,
            r.usage.input_tokens,
            r.usage.output_tokens,
            r.usage.cache_creation_input_tokens,
            r.usage.cache_read_input_tokens,
            r.usage.estimated_cost_usd(model).unwrap_or(f64::NAN),
            r.changed,
            r.reply.replace('\n', " ")
        ),
        Err(err) => eprintln!("PROBE model={model} effort={effort:?} secs={:.1} ERROR {err:?}", t.elapsed().as_secs_f64()),
    }
}

#[test]
#[ignore = "manual: prints latency and token use per model"]
fn probe_models() {
    let models = std::env::var("MANGO_AI_PROBE_MODELS").unwrap_or_else(|_| "claude-haiku-5-5,claude-sonnet-5-5,claude-opus-5-5,claude-fable-5-2".into());
    let prompt = std::env::var("MANGO_AI_PROBE_PROMPT").unwrap_or_else(|_| "Remove the filler words.".into());
    for m in models.split(',') {
        probe(m.trim(), Effort::Medium, &prompt);
    }
}
