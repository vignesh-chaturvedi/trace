//! Token estimation.
//!
//! Deliberately a cheap heuristic rather than a real tokenizer. This number
//! only gates compaction, and a tokenizer would add a large dependency, a
//! model-specific vocabulary to keep in sync, and a second source of truth
//! that can disagree with the provider's own accounting.
//!
//! It errs high. Compacting slightly early costs a little context; compacting
//! too late costs the whole turn to a context-length error.
//!
//! The provider's reported `usage.input` is the ground truth. Both numbers are
//! recorded per request (`est_tokens_in` on `model_request`, `usage` on
//! `model_response`), so drift between them is measurable rather than
//! suspected.

use crate::message::Message;

/// Bytes per token. Real tokenizers land near 4.0 on English prose and lower
/// on code and JSON, which is most of what a coding agent sends.
const BYTES_PER_TOKEN: f64 = 3.5;

/// Per-message framing the provider adds around role and delimiters.
const MESSAGE_OVERHEAD: u64 = 4;

pub fn estimate_message(m: &Message) -> u64 {
    let mut bytes = m.content.len();
    for call in &m.tool_calls {
        bytes += call.name.len();
        bytes += call.id.len();
        bytes += serde_json::to_string(&call.args)
            .map(|s| s.len())
            .unwrap_or(0);
    }
    if let Some(id) = &m.tool_call_id {
        bytes += id.len();
    }
    (bytes as f64 / BYTES_PER_TOKEN).ceil() as u64 + MESSAGE_OVERHEAD
}

pub fn estimate(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message).sum()
}

pub fn estimate_str(s: &str) -> u64 {
    (s.len() as f64 / BYTES_PER_TOKEN).ceil() as u64
}
