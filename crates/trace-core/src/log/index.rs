//! A rebuildable index over session logs.
//!
//! The logs are the source of truth; this is a derived convenience for
//! questions like "which sessions passed task X". It is deliberately just
//! another JSONL file — if it rots, delete it and rebuild.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::event::{Body, Outcome, Usage};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub session: String,
    pub path: String,
    pub task: String,
    pub model: String,
    pub config_hash: String,
    pub harness_commit: String,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub events: usize,
    pub turns: u64,
    pub tool_calls: usize,
    pub usage: Usage,
    pub usd: f64,
    pub outcome: Option<Outcome>,
    /// `sum(cached_input) / sum(input)`, the number that tells you whether the
    /// prefix-stable layout is actually working.
    pub cache_hit_rate: f64,
    pub compactions: usize,
    pub recoveries: usize,
}

pub fn summarize(path: &Path) -> Result<SessionSummary> {
    let outcome = super::reader::read(path)?;
    let events = outcome.events;
    let first = events
        .first()
        .ok_or_else(|| Error::other(format!("{} is empty", path.display())))?;

    let mut s = SessionSummary {
        session: first.session.clone(),
        path: path.display().to_string(),
        task: String::new(),
        model: String::new(),
        config_hash: String::new(),
        harness_commit: String::new(),
        started_ms: first.ts_ms,
        ended_ms: events.last().map(|e| e.ts_ms).unwrap_or(first.ts_ms),
        events: events.len(),
        turns: 0,
        tool_calls: 0,
        usage: Usage::default(),
        usd: 0.0,
        outcome: None,
        cache_hit_rate: 0.0,
        compactions: 0,
        recoveries: 0,
    };

    for ev in &events {
        match &ev.body {
            Body::SessionStart(x) => {
                s.task = x.task.clone();
                s.model = x.model.clone();
                s.config_hash = x.config_hash.clone();
                s.harness_commit = x.harness_commit.clone();
            }
            Body::ModelResponse(x) => {
                s.turns += 1;
                s.usage.add(&x.usage);
            }
            Body::ToolCall(_) => s.tool_calls += 1,
            Body::Compaction(_) => s.compactions += 1,
            Body::Recovery(_) => s.recoveries += 1,
            Body::SessionEnd(x) => {
                s.outcome = Some(x.outcome);
                s.usd = x.usd;
            }
            _ => {}
        }
    }

    s.cache_hit_rate = cache_hit_rate(&s.usage);
    Ok(s)
}

pub fn cache_hit_rate(usage: &Usage) -> f64 {
    if usage.input == 0 {
        0.0
    } else {
        usage.cached_input as f64 / usage.input as f64
    }
}

/// Scan a directory of logs and write `index.jsonl` beside them.
pub fn rebuild_index(dir: &Path) -> Result<Vec<SessionSummary>> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;

    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter(|p| p.file_name().is_some_and(|n| n != "index.jsonl"))
        .collect();
    paths.sort();

    for p in paths {
        match summarize(&p) {
            Ok(s) => out.push(s),
            // One damaged log should not stop the index from being rebuilt.
            Err(e) => eprintln!("trace: skipping {}: {e}", p.display()),
        }
    }

    let mut buf = String::new();
    for s in &out {
        buf.push_str(&serde_json::to_string(s)?);
        buf.push('\n');
    }
    let index_path = dir.join("index.jsonl");
    std::fs::write(&index_path, buf).map_err(|e| Error::io(&index_path, e))?;
    Ok(out)
}
