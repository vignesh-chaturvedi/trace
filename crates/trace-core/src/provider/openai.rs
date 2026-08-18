//! An OpenAI-compatible client.
//!
//! "Compatible" rather than "OpenAI" is the point: the same adapter reaches
//! the vendor API, a gateway, or the vLLM server that will serve the P4
//! fine-tune. Swapping backends must never mean swapping harness code.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::event::Usage;
use crate::message::{JsonValue, Message, Role, ToolCallRef};

use super::{Flow, Provider, Request, Response};

/// A shared request pacer.
///
/// Deliberately separate from the provider and held behind an `Arc`, because
/// the quota being protected belongs to the **account**, not to a client
/// object. A sweep builds a fresh provider for every attempt; if each one
/// carried its own pacer, every task would start believing the last minute
/// never happened, and the limiter would only work for the very first task.
pub struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn per_minute(rpm: u32) -> Arc<RateLimiter> {
        Arc::new(RateLimiter {
            min_interval: if rpm == 0 {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(60.0 / rpm as f64)
            },
            last: Mutex::new(None),
        })
    }

    /// Block until the next request may be sent.
    pub fn acquire(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }
}

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    timeout_secs: u64,
    stream_usage: bool,
    max_retries: u32,
    limiter: Option<Arc<RateLimiter>>,
}

impl OpenAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        OpenAiProvider {
            base_url: base_url.into(),
            api_key: api_key.into(),
            timeout_secs: 600,
            stream_usage: true,
            max_retries: 5,
            limiter: None,
        }
    }

    /// Pace requests using a limiter shared with every other provider built
    /// against the same account.
    pub fn with_limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    fn throttle(&self) {
        if let Some(l) = &self.limiter {
            l.acquire();
        }
    }

    /// Disable `stream_options`, for endpoints that reject the field.
    pub fn with_stream_usage(mut self, on: bool) -> Self {
        self.stream_usage = on;
        self
    }

    /// Read the key from the environment variable named in config.
    ///
    /// The key never enters `Config`, the log, or the context — only its
    /// variable name does, so a published trajectory cannot leak it.
    pub fn from_env(base_url: impl Into<String>, key_var: &str) -> Result<Self> {
        let api_key = std::env::var(key_var)
            .map_err(|_| Error::Provider(crate::secrets::missing_key_help(key_var)))?;
        Ok(OpenAiProvider::new(base_url, api_key))
    }
}

impl Provider for OpenAiProvider {
    fn complete(
        &self,
        req: &Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> Flow,
    ) -> Result<Response> {
        let tools: Value = serde_json::from_str(req.tools_json)?;
        let mut body = json!({
            "model": req.model,
            "temperature": req.temperature,
            "messages": req.messages.iter().map(to_wire).collect::<Vec<_>>(),
            "tools": wrap_tools(&tools),
            "stream": true,
        });

        if self.stream_usage {
            body["stream_options"] = json!({ "include_usage": true });
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        // Retries wrap the request, never the parse. A stream that started and
        // then failed has already delivered deltas to the caller; replaying it
        // would duplicate them.
        let mut attempt = 0u32;
        let resp = loop {
            self.throttle();

            match ureq::post(&url)
                .set("Authorization", &format!("Bearer {}", self.api_key))
                .set("Content-Type", "application/json")
                .timeout(Duration::from_secs(self.timeout_secs))
                .send_json(body.clone())
            {
                Ok(r) => break r,
                Err(e) => {
                    let Some(delay) = self.retry_delay(&e, attempt) else {
                        return Err(Error::Provider(describe(e)));
                    };
                    attempt += 1;
                    std::thread::sleep(delay);
                }
            }
        };

        parse_stream(resp.into_reader(), on_delta)
    }
}

impl OpenAiProvider {
    /// How long to wait before retrying, or `None` if this should not be
    /// retried at all.
    ///
    /// Only 429 and 5xx are retried. A 400 or 401 will fail identically every
    /// time, and retrying it five times just delays a clear error message.
    fn retry_delay(&self, e: &ureq::Error, attempt: u32) -> Option<Duration> {
        if attempt >= self.max_retries {
            return None;
        }

        let server_hint = match e {
            ureq::Error::Status(code, resp) => {
                if *code != 429 && *code < 500 {
                    return None;
                }
                // The server knows more about its own limits than any backoff
                // curve does. Check the header first, then the body: Google
                // returns the delay as `retryDelay` inside the error payload
                // rather than as a header, and guessing when it told you
                // exactly is how a sweep sleeps 32s for a 7s problem.
                resp.header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
            }
            // Transport failures are usually transient: a dropped connection
            // mid-sweep should not end the sweep.
            ureq::Error::Transport(_) => None,
        };

        Some(server_hint.unwrap_or_else(|| backoff(attempt)))
    }
}

/// Pull `retryDelay` (e.g. `"7.775s"`) out of a provider error body.
///
/// Google reports it here rather than in a `Retry-After` header.
pub fn retry_delay_from_body(body: &str) -> Option<Duration> {
    let at = body.find("retryDelay")?;
    let rest = &body[at..];
    let start = rest.find(':')? + 1;
    let value: String = rest[start..]
        .chars()
        .skip_while(|c| c.is_whitespace() || *c == '"')
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    value.parse::<f64>().ok().map(Duration::from_secs_f64)
}

/// Exponential backoff with jitter, capped at a minute.
///
/// The jitter matters more than the curve: without it, every parallel client
/// that hit the same limit retries at the same instant and trips it again.
fn backoff(attempt: u32) -> Duration {
    let base = 1u64 << attempt.min(6); // 1, 2, 4 ... 64 seconds
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % 1000)
        .unwrap_or(0);
    Duration::from_secs(base.min(60)) + Duration::from_millis(jitter_ms)
}

fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            format!(
                "HTTP {code}: {}",
                body.chars().take(600).collect::<String>()
            )
        }
        ureq::Error::Transport(t) => format!("transport: {t}"),
    }
}

fn wrap_tools(tools: &Value) -> Value {
    let list = tools.as_array().cloned().unwrap_or_default();
    Value::Array(
        list.into_iter()
            .map(|t| json!({ "type": "function", "function": t }))
            .collect(),
    )
}

fn to_wire(m: &Message) -> Value {
    let role = match m.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };

    let mut obj = json!({ "role": role, "content": m.content });

    if !m.tool_calls.is_empty() {
        obj["tool_calls"] = Value::Array(
            m.tool_calls
                .iter()
                .map(|c| {
                    let mut call = json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": serde_json::to_string(&c.args).unwrap_or_default(),
                        }
                    });
                    // Straight back out, unexamined. See ToolCallRef::extra.
                    if let Some(extra) = &c.extra {
                        call["extra_content"] = serde_json::to_value(extra).unwrap_or(Value::Null);
                    }
                    call
                })
                .collect(),
        );
    }

    if let Some(id) = &m.tool_call_id {
        obj["tool_call_id"] = Value::String(id.clone());
    }

    obj
}

/// Accumulator for a tool call arriving in fragments.
///
/// The arguments field streams as arbitrary string slices that only become
/// valid JSON once the last one lands, so nothing can be parsed until the
/// stream ends.
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    args: String,
    /// Provider passthrough, taken from whichever delta carries it.
    extra: Option<Value>,
}

fn parse_stream(
    reader: impl std::io::Read,
    on_delta: &mut dyn FnMut(&str) -> Flow,
) -> Result<Response> {
    let reader = BufReader::new(reader);

    let mut content = String::new();
    let mut calls: BTreeMap<usize, PartialCall> = BTreeMap::new();
    let mut usage = Usage::default();
    let mut stop_reason = String::new();

    for line in reader.lines() {
        let line = line.map_err(|e| Error::Provider(format!("stream read failed: {e}")))?;
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            break;
        }

        let chunk: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            // A malformed keepalive should not lose a completed response.
            Err(_) => continue,
        };

        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            usage.input = u["prompt_tokens"].as_u64().unwrap_or(0);
            usage.output = u["completion_tokens"].as_u64().unwrap_or(0);
            // Compatible endpoints do not agree on where cached tokens live.
            // OpenAI and Gemini's compat layer use the first shape; native
            // Gemini and some gateways use one of the others. Reading all of
            // them costs nothing, and getting it wrong shows up as a
            // permanent, silent 0% cache hit rate.
            usage.cached_input = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|c| c.as_u64())
                .or_else(|| u.get("total_cached_tokens").and_then(|c| c.as_u64()))
                .or_else(|| u.get("cached_content_token_count").and_then(|c| c.as_u64()))
                .or_else(|| u.get("cache_read_input_tokens").and_then(|c| c.as_u64()))
                .unwrap_or(0);
        }

        let Some(choice) = chunk["choices"].get(0) else {
            continue;
        };

        if let Some(reason) = choice["finish_reason"].as_str() {
            stop_reason = reason.to_string();
        }

        let delta = &choice["delta"];

        if let Some(text) = delta["content"].as_str() {
            content.push_str(text);
            if on_delta(text) == Flow::Stop {
                stop_reason = "aborted".to_string();
                break;
            }
        }

        if let Some(tcs) = delta["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                let slot = calls.entry(idx).or_default();
                if let Some(id) = tc["id"].as_str() {
                    slot.id.push_str(id);
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    slot.name.push_str(name);
                }
                if let Some(args) = tc["function"]["arguments"].as_str() {
                    slot.args.push_str(args);
                }
                // Arrives on whichever chunk the provider chooses; take the
                // first non-null and keep it.
                if slot.extra.is_none() {
                    if let Some(extra) = tc.get("extra_content").filter(|v| !v.is_null()) {
                        slot.extra = Some(extra.clone());
                    }
                }
            }
        }
    }

    let tool_calls = calls
        .into_values()
        .map(|p| {
            let parsed: Value = serde_json::from_str(&p.args).unwrap_or_else(|_| json!({}));
            let args = match JsonValue::from_json(&parsed) {
                JsonValue::Object(m) => m,
                // A model that emits a non-object argument blob is a bug on
                // its side; surface it rather than dropping it.
                other => BTreeMap::from([("_raw".to_string(), other)]),
            };
            ToolCallRef {
                id: p.id,
                name: p.name,
                args,
                extra: p.extra.as_ref().map(JsonValue::from_json),
            }
        })
        .collect::<Vec<_>>();

    if stop_reason.is_empty() {
        stop_reason = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        }
        .to_string();
    }

    Ok(Response {
        message: Message {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
        },
        usage,
        stop_reason,
    })
}
