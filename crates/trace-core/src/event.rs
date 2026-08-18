//! The session ledger.
//!
//! Every fact about a session lives here, appended once and never mutated.
//! Two rules govern this file and everything that touches it:
//!
//! 1. **Append, never edit.** Compaction *declares* that a range is replaced;
//!    it does not delete it. That is what makes replay lossless and lets P4
//!    train on the uncompacted trajectory even though inference ran compacted.
//! 2. **Store raw, truncate late.** `ToolResult` holds the complete tool
//!    output. `build_context` applies the truncation limit as a pure function
//!    of config, so changing that limit and replaying is a real offline
//!    ablation rather than a re-run.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::message::{JsonValue, Message};

pub type Seq = u64;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub seq: Seq,
    /// Wall-clock time of the append. Recorded for humans and for cost
    /// accounting. **Never read by `build_context`** — it is the most obvious
    /// way to accidentally make the context builder impure.
    pub ts_ms: u64,
    pub session: String,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Body {
    SessionStart(SessionStart),
    ModelRequest(ModelRequest),
    ModelResponse(ModelResponse),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Compaction(Compaction),
    Checkpoint(Checkpoint),
    Recovery(Recovery),
    Observation(Observation),
    PolicyDecision(PolicyDecision),
    Abort(Abort),
    SessionEnd(SessionEnd),
}

/// Events after which a crash must not lose the record.
///
/// Fsyncing every event makes long sessions crawl; fsyncing none means a crash
/// costs the tail of a trajectory you may want for training. `ToolCall` is the
/// load-bearing one — the ordering rule in `runtime::recovery` depends on it
/// reaching disk before the command runs.
pub const FSYNC_TYPES: &[&str] = &[
    "session_start",
    "model_response",
    "tool_call",
    "checkpoint",
    "policy_decision",
];

impl Body {
    pub fn kind(&self) -> &'static str {
        match self {
            Body::SessionStart(_) => "session_start",
            Body::ModelRequest(_) => "model_request",
            Body::ModelResponse(_) => "model_response",
            Body::ToolCall(_) => "tool_call",
            Body::ToolResult(_) => "tool_result",
            Body::Compaction(_) => "compaction",
            Body::Checkpoint(_) => "checkpoint",
            Body::Recovery(_) => "recovery",
            Body::Observation(_) => "observation",
            Body::PolicyDecision(_) => "policy_decision",
            Body::Abort(_) => "abort",
            Body::SessionEnd(_) => "session_end",
        }
    }

    pub fn needs_fsync(&self) -> bool {
        FSYNC_TYPES.contains(&self.kind())
    }
}

/// Everything volatile the context builder is allowed to know.
///
/// The builder cannot call `env::current_dir()` or read `AGENTS.md` itself
/// without becoming impure, so those values are captured once, here, and
/// travel with the log. A session is therefore replayable on a different
/// machine, in a different directory, years later.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SessionStart {
    pub task: String,
    pub cwd: String,
    pub model: String,
    /// Hash of the config this session ran under. A replay under a different
    /// config is a legitimate ablation, but it should be visible, not silent.
    pub config_hash: String,
    pub harness_commit: String,
    /// Contents of AGENTS.md, captured at start. Part of the stable region.
    #[serde(default)]
    pub agents_md: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelRequest {
    /// Hash of the exact `Vec<Message>` sent. This single field is what turns
    /// replay from "probably right" into "provably identical".
    pub context_hash: String,
    pub messages: usize,
    pub est_tokens_in: u64,
    /// Bytes of the stable (cacheable) prefix at the time of the request.
    pub stable_prefix_bytes: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelResponse {
    pub message: Message,
    pub usage: Usage,
    pub stop_reason: String,
    pub wall_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /// Prompt tokens served from the provider's cache. Session cache hit rate
    /// is `sum(cached_input) / sum(input)`.
    pub cached_input: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cached_input += other.cached_input;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: BTreeMap<String, JsonValue>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub exit_code: i32,
    /// The **complete** output. Never truncated at rest.
    pub output: String,
    pub wall_ms: u64,
    /// Set when the tool was killed by the harness rather than exiting.
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Compaction {
    /// Inclusive range of sequence numbers this event stands in for.
    pub replaces_from: Seq,
    pub replaces_to: Seq,
    /// What the model chose to carry forward, written by the model itself
    /// during the flush turn.
    pub summary: String,
    /// Hash of the replaced events, so an expansion can be proven to have
    /// restored the same set of bytes it stood in for.
    pub provenance: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub label: String,
    /// Commit on the run branch capturing tracked workspace contents.
    pub git_ref: String,
    pub log_seq: Seq,
    /// Hash of untracked and ignored files that git will not capture but the
    /// task may depend on.
    pub workspace_hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Recovery {
    /// The `tool_call` that has no matching result.
    pub orphan_seq: Seq,
    pub orphan_call_id: String,
    pub command: String,
    pub note: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Observation {
    pub source: ObservationSource,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    DoomLoop,
    Reinforcement,
    System,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PolicyDecision {
    pub tool: String,
    pub allowed: bool,
    pub rule: String,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Abort {
    pub reason: AbortReason,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    TurnCap,
    Budget,
    WallTimeout,
    ProviderError,
    PolicyDenial,
    Interrupted,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionEnd {
    pub outcome: Outcome,
    pub turns: u64,
    pub usd: f64,
    pub usage: Usage,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Aborted,
    Error,
}

impl Event {
    pub fn kind(&self) -> &'static str {
        self.body.kind()
    }

    pub fn as_session_start(&self) -> Option<&SessionStart> {
        match &self.body {
            Body::SessionStart(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match &self.body {
            Body::ToolCall(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_tool_result(&self) -> Option<&ToolResult> {
        match &self.body {
            Body::ToolResult(t) => Some(t),
            _ => None,
        }
    }
}
