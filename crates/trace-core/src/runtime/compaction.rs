//! Compaction.
//!
//! Two properties make this more than "summarize when full".
//!
//! **The model writes its own summary.** Before compacting, it gets one turn
//! to write down what it must not forget. This beats summarizing the
//! transcript from the outside for a simple reason: the model knows which
//! detail is load-bearing for the work it is doing, and an external
//! summarizer is guessing.
//!
//! **Nothing is deleted.** The compaction event *declares* a replacement; the
//! replaced events stay in the log. Replay can expand back to full fidelity,
//! which means compaction strategies are a pure offline ablation, and P4 can
//! train on the uncompacted trajectory even though inference ran compacted.

use crate::config::Config;
use crate::event::{Body, Compaction, Event, Seq};
use crate::hash::hash_chunks;

pub const FLUSH_PROMPT: &str = "\
Context is about to be compacted. Write down, compactly:
- the goal and current status
- decisions made and why (so you don't relitigate them)
- file paths and symbols you'll need again
- what you already tried that did NOT work
Everything not written here will be lost.";

pub fn should_compact(est_tokens: u64, cfg: &Config) -> bool {
    let budget = cfg.context.compact_at * cfg.model.context_limit as f64;
    est_tokens as f64 >= budget
}

/// The range a new compaction should replace: the oldest events not already
/// compacted, stopping short of the most recent turns.
///
/// `keep_recent` is not a nicety. The last few turns are where the agent's
/// working state lives — the file it just opened, the error it is mid-way
/// through fixing — and summarizing those is how a run loses its place.
pub fn range_for(events: &[Event], cfg: &Config, head: Seq) -> Option<(Seq, Seq)> {
    let first_uncompacted = events
        .iter()
        .filter_map(|e| match &e.body {
            Body::Compaction(c) => Some(c.replaces_to + 1),
            _ => None,
        })
        .max()
        // seq 1 is session_start, which is part of the stable region and must
        // survive every compaction.
        .unwrap_or(2);

    let to = head.checked_sub(cfg.context.keep_recent)?;
    if to < first_uncompacted {
        return None;
    }
    Some((first_uncompacted, to))
}

/// Hash of the events a compaction stands in for.
///
/// An expansion can be checked against this, so "replay restored the original"
/// is a claim the log can settle rather than an assumption.
pub fn provenance(events: &[Event], from: Seq, to: Seq) -> String {
    let lines: Vec<Vec<u8>> = events
        .iter()
        .filter(|e| e.seq >= from && e.seq <= to)
        .map(|e| serde_json::to_vec(e).unwrap_or_default())
        .collect();
    hash_chunks(lines.iter().map(|v| v.as_slice()))
}

pub fn verify(events: &[Event], c: &Compaction) -> bool {
    provenance(events, c.replaces_from, c.replaces_to) == c.provenance
}

/// The uncompacted event set: every event with the compaction declarations
/// removed.
///
/// This is the round trip. The result should equal what the log held before
/// any compaction happened, which is what `tests/compaction.rs` asserts.
pub fn expand(events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .filter(|e| !matches!(e.body, Body::Compaction(_)))
        .cloned()
        .collect()
}
