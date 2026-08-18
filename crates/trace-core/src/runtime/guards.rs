//! Guards against the failure modes that waste a run.

use crate::config::GuardConfig;
use crate::event::{Body, Event, ToolCall, ToolResult};
use crate::hash::hash_chunks;

/// Identity of "this exact call producing this exact result".
///
/// Hashing the **result** as well as the call is what separates a loop from
/// legitimate retrying. Running a flaky test three times is reasonable
/// engineering. Running it three times and getting byte-identical failure
/// output three times is an agent that has stopped learning anything.
pub fn fingerprint(call: &ToolCall, result: &ToolResult) -> String {
    let args = serde_json::to_string(&call.args).unwrap_or_default();
    let exit = result.exit_code.to_string();
    hash_chunks([
        call.name.as_bytes(),
        args.as_bytes(),
        result.output.as_bytes(),
        exit.as_bytes(),
    ])
}

pub struct DoomLoop {
    pub fingerprint: String,
    pub count: usize,
    pub text: String,
}

/// `{n}` is substituted with the observed count, so the nudge cannot drift out
/// of sync with a reconfigured `loop_threshold` and tell the model something
/// its own transcript contradicts.
pub const DOOM_LOOP_NUDGE: &str = "You have run this exact command {n} times \
with the same result. It will not change. State what you actually know, what \
you don't, and try a different approach.";

/// Look for a repeated (call, result) pair in the recent window.
///
/// Fires on the turn the count *reaches* the threshold rather than on every
/// turn past it. Repeating the nudge every subsequent turn trains the model to
/// ignore it, and the observation is itself context the agent has to read.
pub fn detect_doom_loop(events: &[Event], cfg: &GuardConfig) -> Option<DoomLoop> {
    let pairs = recent_pairs(events, cfg.loop_window);
    let latest = pairs.last()?;

    let count = pairs.iter().filter(|f| *f == latest).count();
    if count != cfg.loop_threshold {
        return None;
    }

    Some(DoomLoop {
        fingerprint: latest.clone(),
        count,
        text: DOOM_LOOP_NUDGE.replace("{n}", &count.to_string()),
    })
}

/// Fingerprints of the last `window` completed tool calls, oldest first.
fn recent_pairs(events: &[Event], window: usize) -> Vec<String> {
    let mut out = Vec::new();

    for (i, ev) in events.iter().enumerate() {
        let Body::ToolResult(result) = &ev.body else {
            continue;
        };
        // Walk back for the call this answers. A crash can leave a call with
        // no result, but never a result with no call.
        let call = events[..i]
            .iter()
            .rev()
            .find_map(|e| e.as_tool_call().filter(|c| c.id == result.call_id));
        if let Some(call) = call {
            out.push(fingerprint(call, result));
        }
    }

    if out.len() > window {
        out.drain(..out.len() - window);
    }
    out
}
