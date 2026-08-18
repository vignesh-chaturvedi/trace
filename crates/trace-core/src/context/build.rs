//! `build_context` — the pure function at the centre of the runtime.
//!
//! ```text
//! build_context(events: &[Event], cfg: &Config, upto: Seq) -> Context
//!
//! forbidden inside: SystemTime::now(), env::*, fs::*, rand,
//!                   network, process ids, iteration over unordered maps
//! everything volatile must arrive via `events` or `cfg`
//! ```
//!
//! Purity is the whole point. If this function reads a clock or the
//! filesystem, replaying a log gives you a context that merely resembles what
//! was sent, and every downstream guarantee — provable replay, offline
//! ablation, training on real trajectories — degrades into "probably".
//!
//! Note in particular that `Event::ts_ms` is never read here. It is the most
//! tempting impurity, and `tests/determinism.rs` rewrites every timestamp in a
//! fixture and asserts the output does not move.

use crate::config::Config;
use crate::event::{Body, Compaction, Event, Seq, SessionStart};
use crate::hash::hash_bytes;
use crate::message::{Message, Role};

use super::layout::{stable_region, StableRegion};
use super::tokens;
use super::truncate::truncate;

pub struct Context {
    pub messages: Vec<Message>,
    /// Bytes of the region that must be byte-identical across turns.
    pub stable_prefix_bytes: usize,
    /// The tool block, sent as a separate request field but part of the
    /// cacheable prefix.
    pub tools_json: String,
}

impl Context {
    /// The identity recorded on every `model_request`. Two lines of code at
    /// the call site; it is what makes replay provable rather than plausible.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(&self.messages).expect("messages always serialize");
        hash_bytes(&[bytes.as_slice(), self.tools_json.as_bytes()].concat())
    }

    pub fn est_tokens(&self) -> u64 {
        tokens::estimate(&self.messages) + tokens::estimate_str(&self.tools_json)
    }
}

pub const COMPACTION_PREAMBLE: &str =
    "[earlier turns were compacted; the notes below are what the previous \
     context recorded as load-bearing]\n\n";

pub const RECOVERY_TEMPLATE: &str = "\
The previous session was interrupted while running:
  $ {cmd}
Its result is unknown. Verify the current state before continuing
(check git status, re-read the affected files) and then proceed.";

pub const ORPHAN_RESULT: &str =
    "[interrupted: this command was started but its result was never recorded. \
     Treat its effect as unknown.]";

pub fn build_context(events: &[Event], cfg: &Config, upto: Seq) -> Context {
    let default_start = SessionStart::default();
    let start = events
        .iter()
        .find_map(|e| e.as_session_start())
        .unwrap_or(&default_start);

    let region: StableRegion = stable_region(cfg, start);

    let mut messages = Vec::with_capacity(events.len() + 4);
    messages.push(Message::system(region.system.clone()));
    messages.push(Message::user(start.task.clone()));

    let compactions = collect_compactions(events, upto);
    let mut emitted: Vec<Seq> = Vec::new();

    for ev in events.iter().take_while(|e| e.seq <= upto) {
        // A compacted range renders as its summary, once, at the position
        // where the range began. The replaced events stay in the log — this
        // only changes what is *shown* to the model.
        if let Some(c) = covering(&compactions, ev.seq) {
            if !emitted.contains(&c.at) {
                emitted.push(c.at);
                let mut text = String::from(COMPACTION_PREAMBLE);
                text.push_str(&c.body.summary);
                messages.push(Message::user(text));
            }
            continue;
        }

        match &ev.body {
            Body::ModelResponse(r) => {
                let m = &r.message;
                if !m.content.is_empty() || !m.tool_calls.is_empty() {
                    messages.push(m.clone());
                }
            }
            Body::ToolResult(t) => {
                // The log holds the full output; the limit is applied here, so
                // replaying under a different limit is a real ablation.
                let body = truncate(&t.output, cfg.context.truncate_limit);
                let mut text = body.text;
                if t.timed_out {
                    text.push_str("\n\n[killed by the harness: exceeded the tool timeout]");
                }
                messages.push(Message::tool_result(&t.call_id, text));
            }
            Body::Recovery(r) => {
                messages.push(Message::user(
                    RECOVERY_TEMPLATE.replace("{cmd}", &r.command),
                ));
            }
            Body::Observation(o) => {
                messages.push(Message::user(o.text.clone()));
            }
            // `tool_call` events exist so the call is durable before it runs;
            // the call itself is already carried by the assistant message.
            // `model_request`, `checkpoint`, `policy_decision`, `abort`,
            // `session_end` and `compaction` contribute no conversation text.
            _ => {}
        }
    }

    seal_orphan_calls(&mut messages);

    // The reinforcement frame is derived, never logged. Keeping it out of the
    // ledger means flipping `reinforce` and replaying the same log is a clean
    // A/B — the frame cannot leave residue in the trajectory it is being
    // measured against. It sits last, after the cacheable prefix, so it costs
    // nothing in cache terms.
    if cfg.context.reinforce {
        if let Some(frame) = reinforcement_frame(events, upto, start) {
            messages.push(Message::user(frame));
        }
    }

    Context {
        stable_prefix_bytes: region.bytes(),
        tools_json: region.tools_json,
        messages,
    }
}

struct Span<'a> {
    /// Seq of the compaction event that declared this span.
    at: Seq,
    body: &'a Compaction,
}

fn collect_compactions(events: &[Event], upto: Seq) -> Vec<Span<'_>> {
    events
        .iter()
        .take_while(|e| e.seq <= upto)
        .filter_map(|e| match &e.body {
            Body::Compaction(c) => Some(Span { at: e.seq, body: c }),
            _ => None,
        })
        .collect()
}

/// The span that swallows `seq`, preferring the widest.
///
/// Compactions chain: a later one may cover an earlier compaction event along
/// with the turns it stood for. Taking the widest span means the newest
/// summary wins and no stale summary leaks through underneath it.
fn covering<'a>(spans: &'a [Span<'a>], seq: Seq) -> Option<&'a Span<'a>> {
    spans
        .iter()
        .filter(|s| seq >= s.body.replaces_from && seq <= s.body.replaces_to)
        .max_by_key(|s| s.body.replaces_to)
}

/// Give every tool call a result.
///
/// A crash between a call and its result leaves an assistant message
/// advertising a call that nothing answers. Providers reject that outright, so
/// a resumed session would fail on its first request. Filling the hole with an
/// explicit "unknown" both satisfies the API and tells the model the truth.
fn seal_orphan_calls(messages: &mut Vec<Message>) {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role != Role::Assistant || messages[i].tool_calls.is_empty() {
            i += 1;
            continue;
        }

        let expected: Vec<String> = messages[i]
            .tool_calls
            .iter()
            .map(|c| c.id.clone())
            .collect();

        // Results for this assistant turn run until the next non-tool message.
        let mut end = i + 1;
        let mut seen: Vec<String> = Vec::new();
        while end < messages.len() && messages[end].role == Role::Tool {
            if let Some(id) = &messages[end].tool_call_id {
                seen.push(id.clone());
            }
            end += 1;
        }

        let missing: Vec<String> = expected
            .into_iter()
            .filter(|id| !seen.contains(id))
            .collect();

        for (n, id) in missing.into_iter().enumerate() {
            messages.insert(end + n, Message::tool_result(id, ORPHAN_RESULT));
        }

        i = end.max(i + 1);
    }
}

/// A compact objective-and-status frame.
///
/// Derived entirely from the events, so it stays pure: the turn number is a
/// count of responses in the log, not a counter held in the runtime.
fn reinforcement_frame(events: &[Event], upto: Seq, start: &SessionStart) -> Option<String> {
    let turns = events
        .iter()
        .take_while(|e| e.seq <= upto)
        .filter(|e| matches!(e.body, Body::ModelResponse(_)))
        .count();

    if turns == 0 {
        return None;
    }

    Some(format!(
        "[status] objective: {}\n\
         turns used: {}\n\
         Before the next command, state in one line what you now know that you \
         did not know before the last one. If it is nothing, change approach.",
        start.task.trim(),
        turns
    ))
}
