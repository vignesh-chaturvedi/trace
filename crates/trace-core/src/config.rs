//! Session configuration.
//!
//! `Config` is an input to `build_context`, which means it is part of the
//! determinism contract: the same config and the same events must produce the
//! same bytes. Every field is therefore a plain value in declaration order —
//! no maps, no `Option<Value>`, nothing whose serialization depends on
//! insertion history.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub model: ModelConfig,
    pub limits: Limits,
    pub context: ContextConfig,
    pub guards: GuardConfig,
    pub prompt: PromptConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub name: String,
    /// Any OpenAI-compatible endpoint: the vendor's, a gateway, or the vLLM
    /// server that will host the P4 fine-tune.
    pub base_url: String,
    /// Name of the environment variable holding the key. The key itself never
    /// enters config, the log, or the context.
    pub api_key_env: String,
    /// Pinned, because unpinned temperature turns score variance into noise
    /// you cannot attribute.
    pub temperature: f64,
    pub context_limit: u64,
    pub price_in_per_mtok: f64,
    pub price_out_per_mtok: f64,
    pub price_cached_in_per_mtok: f64,
    /// Send `stream_options: {include_usage: true}`.
    ///
    /// OpenAI needs this or a streaming response carries no usage at all and
    /// the cache hit rate silently reads as zero forever. Some compatible
    /// layers reject the unknown field outright, so it is a switch rather than
    /// a constant. Turning it off costs you usage accounting, not correctness.
    pub stream_usage: bool,
    /// Client-side throttle. 0 disables it.
    ///
    /// A free tier with a 15 RPM cap will otherwise spend a sweep generating
    /// 429s instead of results. Pacing on the client is cheaper than retrying
    /// on the server.
    pub requests_per_minute: u32,
    /// Retries for 429 and 5xx, with exponential backoff.
    pub max_retries: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_turns: u64,
    /// Per-task, not per-run. A per-run cap is how a sweep costs 5x the
    /// estimate before anything trips.
    pub max_usd: f64,
    pub tool_timeout_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    /// Fraction of the model's context limit at which compaction triggers.
    pub compact_at: f64,
    /// Turns at the head that compaction must never touch.
    pub keep_recent: u64,
    /// Byte budget for a single tool result *as rendered into context*. The
    /// log keeps the full output, so this is replay-adjustable.
    pub truncate_limit: usize,
    /// Objective-and-status frame appended after each tool result. Behind a
    /// flag because it is one of the cleanest available ablations.
    pub reinforce: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GuardConfig {
    /// How many recent tool calls the doom-loop detector considers.
    pub loop_window: usize,
    /// Identical (call, result) fingerprints within the window before the
    /// detector speaks up.
    pub loop_threshold: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PromptConfig {
    /// `{cwd}` is the only substitution, and it is session-stable. Anything
    /// turn-varying in here destroys the cache — `context::lint` enforces it.
    pub system: String,
}

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are a software engineer working in a terminal on a real repository.

You have one tool: bash. Use it for everything - reading files, searching,
editing, running tests, installing dependencies.

- Verify before you claim. Run the tests.
- Prefer small checkable steps over large speculative ones.
- If a command fails, read the error before trying again.
- When the task is complete and verified, say so and stop.

Working directory: {cwd}";

impl Default for ModelConfig {
    fn default() -> Self {
        ModelConfig {
            name: "gpt-4.1-mini".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            temperature: 0.0,
            context_limit: 128_000,
            price_in_per_mtok: 0.0,
            price_out_per_mtok: 0.0,
            price_cached_in_per_mtok: 0.0,
            stream_usage: true,
            requests_per_minute: 0,
            max_retries: 5,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_turns: 60,
            max_usd: 2.0,
            tool_timeout_ms: 120_000,
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        ContextConfig {
            compact_at: 0.75,
            keep_recent: 6,
            truncate_limit: 8_000,
            reinforce: false,
        }
    }
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            loop_window: 8,
            loop_threshold: 3,
        }
    }
}

impl Default for PromptConfig {
    fn default() -> Self {
        PromptConfig {
            system: DEFAULT_SYSTEM_PROMPT.into(),
        }
    }
}

impl Config {
    pub fn from_toml(src: &str) -> Result<Config> {
        toml::from_str(src).map_err(|e| Error::Config(e.to_string()))
    }

    pub fn load(path: &std::path::Path) -> Result<Config> {
        let src = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Config::from_toml(&src)
    }

    /// Stable identity of this config, recorded on every session so a score
    /// can always be attributed to the settings that produced it.
    pub fn hash(&self) -> String {
        let json = serde_json::to_vec(self).expect("config is always serializable");
        crate::hash::hash_bytes(&json)
    }

    /// Cost of a single response, in USD.
    pub fn price(&self, usage: &crate::event::Usage) -> f64 {
        let m = &self.model;
        let fresh_in = usage.input.saturating_sub(usage.cached_input) as f64;
        (fresh_in * m.price_in_per_mtok
            + usage.cached_input as f64 * m.price_cached_in_per_mtok
            + usage.output as f64 * m.price_out_per_mtok)
            / 1_000_000.0
    }
}
