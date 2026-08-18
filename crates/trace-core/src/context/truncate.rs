//! Middle-out truncation of tool output.
//!
//! The one thing naive harnesses get wrong that measurably costs score: a
//! `cat` of a large file or a test run with 40k lines of output destroys the
//! context and the agent never recovers.
//!
//! Two decisions matter here. Keep the **tail**, because errors and summaries
//! cluster at the end of test output, so head-only truncation throws away the
//! part that carries the signal. And **tell the model how to get more** — an
//! elided middle with no instructions produces an agent that re-runs the same
//! command hoping for a different length.
//!
//! This runs at context-build time, not at log-write time. The log keeps the
//! full output, so `truncate_limit` is a replay-time ablation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    pub text: String,
    pub dropped_lines: usize,
    pub dropped_bytes: usize,
}

impl Truncated {
    pub fn was_truncated(&self) -> bool {
        self.dropped_bytes > 0
    }
}

const HEAD_SHARE: f64 = 0.55;
const TAIL_SHARE: f64 = 0.35;

pub fn truncate(out: &str, limit: usize) -> Truncated {
    if out.len() <= limit {
        return Truncated {
            text: out.to_string(),
            dropped_lines: 0,
            dropped_bytes: 0,
        };
    }

    let head_len = floor_boundary(out, (limit as f64 * HEAD_SHARE) as usize);
    let tail_len = (limit as f64 * TAIL_SHARE) as usize;
    let tail_start = ceil_boundary(out, out.len().saturating_sub(tail_len));

    // A pathological limit could make the two halves meet or cross. Return the
    // original rather than emitting overlapping text that reads as duplicated
    // output.
    if head_len >= tail_start {
        return Truncated {
            text: out.to_string(),
            dropped_lines: 0,
            dropped_bytes: 0,
        };
    }

    let head = &out[..head_len];
    let tail = &out[tail_start..];
    let dropped = &out[head_len..tail_start];
    let dropped_lines = dropped.lines().count();
    let dropped_bytes = dropped.len();

    let mut text = String::with_capacity(head.len() + tail.len() + 200);
    text.push_str(head);
    text.push_str("\n\n[... ");
    text.push_str(&dropped_lines.to_string());
    text.push_str(" lines omitted (");
    text.push_str(&out.len().to_string());
    text.push_str(
        " bytes total). Narrow the command - grep, head -n, tail -n, or write to a file and read a range ...]\n\n",
    );
    text.push_str(tail);

    Truncated {
        text,
        dropped_lines,
        dropped_bytes,
    }
}

/// Largest char boundary at or below `i`. Slicing a `&str` at a byte offset
/// that lands inside a multi-byte character panics, and tool output is full of
/// UTF-8 — box-drawing characters in test runners, emoji in CI logs.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary at or above `i`.
fn ceil_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}
