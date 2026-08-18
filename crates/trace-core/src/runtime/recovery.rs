//! Crash recovery.
//!
//! The subtlest part of the phase, and the part most harnesses get wrong.
//!
//! ```text
//! tool_call   event written and fsynced  BEFORE execution
//! tool_result event written              AFTER  execution
//!
//! therefore on resume:
//!   tool_call WITH   matching result  -> completed, safe
//!   tool_call WITHOUT matching result -> UNKNOWN. It may have run.
//! ```
//!
//! The unknown case must never be blindly re-executed. `rm -rf build/` twice
//! is fine; `git push` twice is not, and the harness has no way to tell them
//! apart. So the harness does not decide — it records what it knows and hands
//! the model the job of re-establishing ground truth, which is the one actor
//! here that can actually look.

use crate::event::{Body, Event, Seq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub seq: Seq,
    pub call_id: String,
    pub command: String,
}

pub const RECOVERY_NOTE: &str =
    "interrupted before the result was recorded; effect unknown, not re-executed";

/// Tool calls with no matching result.
pub fn find_orphans(events: &[Event]) -> Vec<Orphan> {
    let answered: Vec<&str> = events
        .iter()
        .filter_map(|e| e.as_tool_result().map(|r| r.call_id.as_str()))
        .collect();

    events
        .iter()
        .filter_map(|e| {
            let call = e.as_tool_call()?;
            if answered.contains(&call.id.as_str()) {
                return None;
            }
            Some(Orphan {
                seq: e.seq,
                call_id: call.id.clone(),
                command: call
                    .args
                    .get("cmd")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<unknown command>")
                    .to_string(),
            })
        })
        .collect()
}

/// Whether the log ends in a state a session can simply continue from.
pub fn is_clean(events: &[Event]) -> bool {
    find_orphans(events).is_empty()
        && !matches!(
            events.last().map(|e| &e.body),
            Some(Body::SessionEnd(_)) | Some(Body::Abort(_))
        )
}
