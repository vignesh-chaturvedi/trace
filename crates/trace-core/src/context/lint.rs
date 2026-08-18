//! The layout lint.
//!
//! A build-failing check, not a convention. Cache discipline erodes silently:
//! someone adds a "turns remaining" line to the system prompt, the hit rate
//! goes to zero, and nothing in the code looks wrong. Only a check that fails
//! the build catches that on the day it happens rather than at the next sweep.
//!
//! Two layers. The **structural** check builds real contexts at two different
//! points of a synthetic session, with different timestamps and different turn
//! counts, and asserts their common prefix still covers the whole stable
//! region. That is the invariant itself, and it subsumes everything else. The
//! **heuristic** scans then look for the specific mistakes that produce it, so
//! a failure names its own cause instead of just reporting a byte offset.

use crate::config::Config;
use crate::event::{Body, Event, ModelResponse, SessionStart, ToolResult, Usage};
use crate::message::Message;

use super::build::build_context;
use super::layout::stable_region;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Breaks caching within a single session. Fails the build.
    Error,
    /// Breaks cache sharing between sessions but not within one.
    Warn,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    pub rule: &'static str,
    pub detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warn => "warn ",
        };
        write!(f, "{tag} [{}] {}", self.rule, self.detail)
    }
}

pub fn lint(cfg: &Config, start: &SessionStart) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(check_prefix_stability(cfg, start));

    let region = stable_region(cfg, start);
    out.extend(scan(&region.system, "system prompt"));
    out.extend(scan(&region.tools_json, "tool schemas"));
    out.extend(check_prefix_size(cfg, start));

    out.sort_by_key(|f| f.severity);
    out
}

/// Size of the stable prefix, measured in tokens.
pub fn stable_prefix_tokens(cfg: &Config, start: &SessionStart) -> u64 {
    let r = stable_region(cfg, start);
    super::tokens::estimate_str(&r.system) + super::tokens::estimate_str(&r.tools_json)
}

/// The smallest prompt a provider will bother caching.
///
/// A byte-stable prefix earns nothing if it never reaches the provider's
/// minimum. This is the gap between "the layout is correct" and "the cache
/// actually hits", and it is invisible until you read someone's docs.
pub const CACHE_THRESHOLDS: &[(&str, u64)] = &[
    ("OpenAI", 1024),
    ("Anthropic", 1024),
    ("Gemini 2.5", 2048),
    ("Gemini 3.x Flash", 4096),
];

fn check_prefix_size(cfg: &Config, start: &SessionStart) -> Vec<Finding> {
    let tokens = stable_prefix_tokens(cfg, start);

    let unmet: Vec<&str> = CACHE_THRESHOLDS
        .iter()
        .filter(|(_, min)| tokens < *min)
        .map(|(name, _)| *name)
        .collect();

    if unmet.is_empty() {
        return Vec::new();
    }

    vec![Finding {
        severity: Severity::Warn,
        rule: "prefix-below-cache-threshold",
        detail: format!(
            "the stable region is ~{tokens} tokens, below the minimum these providers will \
             cache at all: {}. Caching will only begin once the conversation itself pushes \
             the total over the threshold, so early turns pay full price no matter how \
             stable the layout is.",
            unmet.join(", ")
        ),
    }]
}

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

/// The real check: does the prefix actually hold across turns?
fn check_prefix_stability(cfg: &Config, start: &SessionStart) -> Vec<Finding> {
    let events = synthetic_session(start);

    let early = build_context(&events, cfg, 3);
    let late = build_context(&events, cfg, events.last().map(|e| e.seq).unwrap_or(3));

    let a = render_prefix(&early.messages);
    let b = render_prefix(&late.messages);
    let shared = common_prefix_len(a.as_bytes(), b.as_bytes());

    let mut out = Vec::new();

    if early.tools_json != late.tools_json {
        out.push(Finding {
            severity: Severity::Error,
            rule: "tool-schema-unstable",
            detail: "serialized tool schemas differ between two turns of the same session; \
                     something in the schema is built from an unordered map"
                .into(),
        });
    }

    if shared < early.stable_prefix_bytes.min(a.len()) {
        out.push(Finding {
            severity: Severity::Error,
            rule: "prefix-unstable",
            detail: format!(
                "contexts at turn 1 and turn {} share only {shared} bytes, but the stable \
                 region is {} bytes; something before the conversation varies per turn",
                late.messages.len(),
                early.stable_prefix_bytes
            ),
        });
    }

    out
}

/// Everything up to and including the first user message — the part that must
/// not move.
fn render_prefix(messages: &[Message]) -> String {
    let mut s = String::new();
    for m in messages.iter().take(1) {
        s.push_str(&m.content);
    }
    s
}

fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// A two-turn session that exists only to be rendered twice.
///
/// The timestamps differ by an hour and the turn counts differ, so anything
/// reading a clock or a counter shows up as a prefix divergence.
fn synthetic_session(start: &SessionStart) -> Vec<Event> {
    let mk = |seq: u64, ts_ms: u64, body: Body| Event {
        seq,
        ts_ms,
        session: "lint".to_string(),
        body,
    };

    let response = |text: &str| ModelResponse {
        message: Message::assistant(text),
        usage: Usage::default(),
        stop_reason: "stop".into(),
        wall_ms: 0,
    };

    vec![
        mk(1, 1_000_000, Body::SessionStart(start.clone())),
        mk(2, 1_000_500, Body::ModelResponse(response("first"))),
        mk(
            3,
            1_001_000,
            Body::ToolResult(ToolResult {
                call_id: "c1".into(),
                exit_code: 0,
                output: "ok".into(),
                wall_ms: 3,
                timed_out: false,
            }),
        ),
        mk(4, 4_600_000, Body::ModelResponse(response("second"))),
        mk(
            5,
            4_601_000,
            Body::ToolResult(ToolResult {
                call_id: "c2".into(),
                exit_code: 0,
                output: "ok".into(),
                wall_ms: 3,
                timed_out: false,
            }),
        ),
        mk(6, 4_602_000, Body::ModelResponse(response("third"))),
    ]
}

/// Named scans for the specific ways the prefix usually rots.
fn scan(text: &str, where_: &'static str) -> Vec<Finding> {
    let mut out = Vec::new();

    if let Some(hit) = find_date_like(text) {
        out.push(Finding {
            severity: Severity::Error,
            rule: "timestamp-in-prefix",
            detail: format!("{where_} contains a date/time-like substring {hit:?}"),
        });
    }

    for kw in ["token", "turn", "remaining", "used", "%"] {
        if let Some(hit) = find_digit_near(text, kw) {
            out.push(Finding {
                severity: Severity::Error,
                rule: "counter-in-prefix",
                detail: format!("{where_} has a digit next to {kw:?}: {hit:?}"),
            });
        }
    }

    for dir in ["/tmp/", "/var/folders/", "/private/tmp/", r"\Temp\"] {
        if text.contains(dir) {
            out.push(Finding {
                severity: Severity::Warn,
                rule: "temp-path-in-prefix",
                detail: format!(
                    "{where_} references {dir:?}; stable within this session, but no two \
                     sessions will share a cache entry"
                ),
            });
        }
    }

    if let Some(hit) = find_hex_run(text, 12) {
        out.push(Finding {
            severity: Severity::Warn,
            rule: "opaque-id-in-prefix",
            detail: format!(
                "{where_} contains {hit:?}, which looks like a session or run id; \
                 it will differ on every run"
            ),
        });
    }

    out
}

/// `YYYY-MM-DD` or `HH:MM:SS`, without pulling in a regex engine.
fn find_date_like(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_digit());

    for i in 0..b.len() {
        if digit(i)
            && digit(i + 1)
            && digit(i + 2)
            && digit(i + 3)
            && b.get(i + 4) == Some(&b'-')
            && digit(i + 5)
            && digit(i + 6)
            && b.get(i + 7) == Some(&b'-')
            && digit(i + 8)
            && digit(i + 9)
        {
            return Some(text[i..i + 10].to_string());
        }
        if digit(i)
            && digit(i + 1)
            && b.get(i + 2) == Some(&b':')
            && digit(i + 3)
            && digit(i + 4)
            && b.get(i + 5) == Some(&b':')
            && digit(i + 6)
            && digit(i + 7)
        {
            return Some(text[i..i + 8].to_string());
        }
    }
    None
}

const NEAR: usize = 14;

/// A digit within `NEAR` bytes of a counter-ish word — "turns used: 12",
/// "83% of context remaining", "4310 tokens".
fn find_digit_near(text: &str, keyword: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0usize;

    while let Some(rel) = lower[from..].find(keyword) {
        let at = from + rel;
        let lo = at.saturating_sub(NEAR);
        let hi = (at + keyword.len() + NEAR).min(lower.len());
        let lo = floor_boundary(&lower, lo);
        let hi = ceil_boundary(&lower, hi);

        if lower[lo..hi].bytes().any(|c| c.is_ascii_digit()) {
            return Some(lower[lo..hi].trim().to_string());
        }
        from = at + keyword.len();
    }
    None
}

/// A long unbroken hex run, which is what session ids and content hashes look
/// like once they leak into a path.
fn find_hex_run(text: &str, min: usize) -> Option<String> {
    let b = text.as_bytes();
    let mut start = None;

    for i in 0..=b.len() {
        let is_hex = b
            .get(i)
            .is_some_and(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        match (is_hex, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) if i - s >= min => return Some(text[s..i].to_string()),
            (false, Some(_)) => start = None,
            _ => {}
        }
    }
    None
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}
