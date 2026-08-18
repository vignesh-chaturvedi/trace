//! An OpenAI-compatible client.
//!
//! "Compatible" rather than "OpenAI" is the point: the same adapter reaches
//! the vendor API, a gateway, or the vLLM server that will serve the P4
//! fine-tune. Swapping backends must never mean swapping harness code.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::event::Usage;
use crate::message::{JsonValue, Message, Role, ToolCallRef};

use super::{Flow, Provider, Request, Response};

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    timeout_secs: u64,
}

impl OpenAiProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        OpenAiProvider {
            base_url: base_url.into(),
            api_key: api_key.into(),
            timeout_secs: 600,
        }
    }

    /// Read the key from the environment variable named in config.
    ///
    /// The key never enters `Config`, the log, or the context — only its
    /// variable name does, so a published trajectory cannot leak it.
    pub fn from_env(base_url: impl Into<String>, key_var: &str) -> Result<Self> {
        let api_key = std::env::var(key_var)
            .map_err(|_| Error::Provider(format!("environment variable {key_var} is not set")))?;
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
        let body = json!({
            "model": req.model,
            "temperature": req.temperature,
            "messages": req.messages.iter().map(to_wire).collect::<Vec<_>>(),
            "tools": wrap_tools(&tools),
            "stream": true,
            // Without this the streaming path returns no usage at all, and
            // cache hit rate silently reads as zero forever.
            "stream_options": { "include_usage": true },
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send_json(body)
            .map_err(|e| Error::Provider(describe(e)))?;

        parse_stream(resp.into_reader(), on_delta)
    }
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
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": serde_json::to_string(&c.args).unwrap_or_default(),
                        }
                    })
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
            usage.cached_input = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|c| c.as_u64())
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
