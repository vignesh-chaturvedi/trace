//! The wire shape of a conversation turn.
//!
//! `Message` is what `build_context` produces and what a provider consumes. It
//! is deliberately provider-agnostic: the OpenAI-compatible adapter translates
//! it at the edge, so nothing upstream is coupled to one vendor's schema.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// Assistant messages only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
    /// Tool messages only: which call this is the result of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// A tool invocation as it appears inside an assistant message.
///
/// `args` is a `BTreeMap`, not a `serde_json::Map`. That is not a style
/// preference: hash-map key order is the single most common cause of a
/// zero-percent cache hit rate, because the bytes change every turn while the
/// content does not. `BTreeMap` makes the ordering structural.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    pub args: BTreeMap<String, JsonValue>,
    /// Provider metadata attached to this call, echoed back verbatim.
    ///
    /// Gemini 3.x returns an encrypted `thought_signature` here and **rejects
    /// the next turn with a 400 if it is not sent back** — the model's
    /// reasoning state is stateless across turns, and the signature is how it
    /// is carried. Dropping it produces a harness that can make exactly one
    /// tool call per session.
    ///
    /// Kept opaque and untyped on purpose. This is a passthrough channel, not
    /// a place to grow per-vendor branches: whatever arrived goes back out
    /// unchanged, and a provider that sends nothing costs nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<JsonValue>,
}

/// A deterministically-ordered JSON value.
///
/// `serde_json::Value` holds objects in a `Map` whose iteration order depends
/// on crate features. This type only admits ordered maps, so any value that
/// reaches the context is byte-stable by construction.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Convert from an arbitrary `serde_json::Value`, sorting every object key
    /// on the way in. This is the only sanctioned entry point for JSON that
    /// arrived from a provider.
    pub fn from_json(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => JsonValue::Null,
            serde_json::Value::Bool(b) => JsonValue::Bool(*b),
            serde_json::Value::Number(n) => JsonValue::Int(n.as_i64().unwrap_or(0)),
            serde_json::Value::String(s) => JsonValue::Str(s.clone()),
            serde_json::Value::Array(a) => {
                JsonValue::Array(a.iter().map(JsonValue::from_json).collect())
            }
            serde_json::Value::Object(o) => JsonValue::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), JsonValue::from_json(v)))
                    .collect(),
            ),
        }
    }
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message {
            role: Role::System,
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }
}
