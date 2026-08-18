//! Deciding what can run at the same time.
//!
//! ```text
//! reads (read, grep, ls)     -> parallel, always
//! writes to distinct paths   -> parallel
//! writes to the same path    -> sequential
//! bash                       -> sequential by default
//!                               (parallel only when explicitly marked pure)
//! ```
//!
//! Batches preserve the model's ordering: calls only ever join the batch being
//! accumulated, never an earlier one. So the worst case degrades to running
//! everything one at a time, which is exactly the P0 behaviour.

use crate::event::ToolCall;

use super::schema::{kind_of, write_target, ToolKind};

/// Indices into the original call list, to be run concurrently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch(pub Vec<usize>);

pub fn plan(calls: &[ToolCall]) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut claimed: Vec<String> = Vec::new();
    let mut sealed = false;

    for (i, call) in calls.iter().enumerate() {
        let kind = kind_of(&call.name);

        let joins = match kind {
            // An exec call can do anything, including writing files nobody
            // declared. It runs alone.
            ToolKind::Exec => false,
            ToolKind::Read => !sealed,
            ToolKind::Write => match write_target(&call.args) {
                Some(path) => !sealed && !claimed.iter().any(|p| p == path),
                // A write that does not name its target is indistinguishable
                // from an exec as far as safety goes.
                None => false,
            },
        };

        if !joins && !current.is_empty() {
            batches.push(Batch(std::mem::take(&mut current)));
            claimed.clear();
            sealed = false;
        }

        current.push(i);

        match kind {
            ToolKind::Exec => sealed = true,
            ToolKind::Write => match write_target(&call.args) {
                Some(path) => claimed.push(path.to_string()),
                None => sealed = true,
            },
            ToolKind::Read => {}
        }
    }

    if !current.is_empty() {
        batches.push(Batch(current));
    }

    batches
}
