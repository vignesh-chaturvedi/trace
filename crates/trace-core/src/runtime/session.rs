//! The loop.
//!
//! Structurally this is still the control group's twenty lines — request,
//! tool calls, results, repeat. What changed is where state lives. Nothing
//! here holds the conversation in memory and checkpoints it occasionally; the
//! log *is* the conversation, and every turn rebuilds the context from it.
//! That single inversion is what buys byte-identical replay, clean resume,
//! and offline ablation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::Config;
use crate::context::build_context;
use crate::error::{Error, Result};
use crate::event::{
    Abort, AbortReason, Body, Compaction, Event, ModelRequest, ModelResponse, Observation,
    ObservationSource, Outcome, Recovery, Seq, SessionEnd, SessionStart, ToolCall, ToolResult,
    Usage,
};
use crate::log::EventLog;
use crate::message::{JsonValue, Message};
use crate::provider::{Flow, Provider, Request};
use crate::tools::bash::run_bash;
use crate::tools::schedule::plan;
use crate::tools::schema::BASH;

use super::compaction::{provenance, range_for, should_compact, FLUSH_PROMPT};
use super::guards::detect_doom_loop;
use super::recovery::{find_orphans, RECOVERY_NOTE};

pub struct Session {
    pub cfg: Config,
    log: EventLog,
    events: Vec<Event>,
    workspace: PathBuf,
    usage: Usage,
    spend: f64,
    turns: u64,
}

#[derive(Debug, Clone)]
pub struct RunReport {
    pub outcome: Outcome,
    pub text: String,
    pub turns: u64,
    pub usage: Usage,
    pub usd: f64,
    pub cache_hit_rate: f64,
}

pub struct StartArgs<'a> {
    pub log_path: &'a Path,
    pub session_id: String,
    pub task: String,
    pub workspace: PathBuf,
    pub agents_md: String,
    pub harness_commit: String,
}

impl Session {
    pub fn start(cfg: Config, args: StartArgs<'_>) -> Result<Session> {
        let mut log = EventLog::create(args.log_path, args.session_id)?;
        let start = SessionStart {
            task: args.task,
            cwd: args.workspace.display().to_string(),
            model: cfg.model.name.clone(),
            config_hash: cfg.hash(),
            harness_commit: args.harness_commit,
            agents_md: args.agents_md,
        };
        let first = log.append(Body::SessionStart(start))?;

        Ok(Session {
            cfg,
            log,
            events: vec![first],
            workspace: args.workspace,
            usage: Usage::default(),
            spend: 0.0,
            turns: 0,
        })
    }

    /// Reopen an interrupted session.
    ///
    /// Cost and turn counters are rebuilt from the log rather than carried
    /// across, so a resumed session cannot quietly restart its budget.
    pub fn resume(cfg: Config, log_path: &Path, workspace: PathBuf) -> Result<Session> {
        let (log, events) = EventLog::resume(log_path)?;

        let mut usage = Usage::default();
        let mut turns = 0u64;
        for ev in &events {
            if let Body::ModelResponse(r) = &ev.body {
                usage.add(&r.usage);
                turns += 1;
            }
        }
        let spend = events
            .iter()
            .filter_map(|e| match &e.body {
                Body::ModelResponse(r) => Some(cfg.price(&r.usage)),
                _ => None,
            })
            .sum();

        Ok(Session {
            cfg,
            log,
            events,
            workspace,
            usage,
            spend,
            turns,
        })
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn log_path(&self) -> &Path {
        self.log.path()
    }

    pub fn session_id(&self) -> &str {
        self.log.session()
    }

    fn head(&self) -> Seq {
        self.events.last().map(|e| e.seq).unwrap_or(0)
    }

    fn append(&mut self, body: Body) -> Result<Seq> {
        let ev = self.log.append(body)?;
        let seq = ev.seq;
        self.events.push(ev);
        Ok(seq)
    }

    pub fn run(
        &mut self,
        provider: &dyn Provider,
        sink: &mut dyn FnMut(&str),
    ) -> Result<RunReport> {
        self.recover()?;

        loop {
            self.maybe_compact(provider)?;

            let head = self.head();
            let ctx = build_context(&self.events, &self.cfg, head);
            let est = ctx.est_tokens();

            self.append(Body::ModelRequest(ModelRequest {
                context_hash: ctx.hash(),
                messages: ctx.messages.len(),
                est_tokens_in: est,
                stable_prefix_bytes: ctx.stable_prefix_bytes,
            }))?;

            // The mid-stream budget guard needs an upper bound on what this
            // turn has already committed, and a per-byte rate for what is
            // still arriving.
            let committed = self.spend + est as f64 * self.cfg.model.price_in_per_mtok / 1e6;
            let headroom = self.cfg.limits.max_usd - committed;
            let usd_per_byte = self.cfg.model.price_out_per_mtok / 1e6 / 3.5;

            let mut streamed = 0usize;
            let mut overran = false;
            let response = {
                let req = Request {
                    model: &self.cfg.model.name,
                    temperature: self.cfg.model.temperature,
                    messages: &ctx.messages,
                    tools_json: &ctx.tools_json,
                };
                provider.complete(&req, &mut |delta| {
                    sink(delta);
                    streamed += delta.len();
                    if usd_per_byte > 0.0 && streamed as f64 * usd_per_byte > headroom {
                        overran = true;
                        Flow::Stop
                    } else {
                        Flow::Continue
                    }
                })?
            };

            self.usage.add(&response.usage);
            self.spend += self.cfg.price(&response.usage);
            self.turns += 1;

            let tool_calls = response.message.tool_calls.clone();
            let text = response.message.content.clone();

            self.append(Body::ModelResponse(ModelResponse {
                message: response.message,
                usage: response.usage,
                stop_reason: response.stop_reason,
                wall_ms: 0,
            }))?;

            if overran {
                return self.abort(
                    AbortReason::Budget,
                    format!(
                        "response exceeded the remaining budget mid-stream after {streamed} bytes"
                    ),
                );
            }

            // Termination is the model's decision, plus hard caps.
            if tool_calls.is_empty() {
                return self.finish(Outcome::Done, text);
            }

            if self.turns >= self.cfg.limits.max_turns {
                return self.abort(
                    AbortReason::TurnCap,
                    format!("reached the turn cap of {}", self.cfg.limits.max_turns),
                );
            }

            if self.spend > self.cfg.limits.max_usd {
                return self.abort(
                    AbortReason::Budget,
                    format!(
                        "spent ${:.4} of ${:.4}",
                        self.spend, self.cfg.limits.max_usd
                    ),
                );
            }

            let calls: Vec<ToolCall> = tool_calls
                .into_iter()
                .map(|c| ToolCall {
                    id: c.id,
                    name: c.name,
                    args: c.args,
                })
                .collect();

            self.execute(&calls)?;
            self.check_doom_loop()?;
        }
    }

    /// Run a turn's tool calls, honouring the concurrency rules.
    fn execute(&mut self, calls: &[ToolCall]) -> Result<()> {
        let timeout = Duration::from_millis(self.cfg.limits.tool_timeout_ms);

        for batch in plan(calls) {
            // Durability before execution, for every call in the batch. A
            // crash from here on leaves an answerable question rather than a
            // silently repeated side effect.
            for &i in &batch.0 {
                self.append(Body::ToolCall(calls[i].clone()))?;
            }
            self.log.sync()?;

            let mut results = run_batch(&batch.0, calls, &self.workspace, timeout);

            // Record in call order, never completion order. Otherwise a replay
            // of a parallel batch depends on which thread happened to finish
            // first, and determinism is gone.
            results.sort_by_key(|(i, _)| *i);

            for (i, result) in results {
                self.append(Body::ToolResult(ToolResult {
                    call_id: calls[i].id.clone(),
                    ..result
                }))?;
            }
        }

        Ok(())
    }

    fn check_doom_loop(&mut self) -> Result<()> {
        if let Some(loop_) = detect_doom_loop(&self.events, &self.cfg.guards) {
            self.append(Body::Observation(Observation {
                source: ObservationSource::DoomLoop,
                text: loop_.text,
            }))?;
        }
        Ok(())
    }

    /// Note every interrupted tool call before doing anything else.
    fn recover(&mut self) -> Result<()> {
        for orphan in find_orphans(&self.events) {
            self.append(Body::Recovery(Recovery {
                orphan_seq: orphan.seq,
                orphan_call_id: orphan.call_id,
                command: orphan.command,
                note: RECOVERY_NOTE.to_string(),
            }))?;
        }
        Ok(())
    }

    fn maybe_compact(&mut self, provider: &dyn Provider) -> Result<()> {
        let head = self.head();
        let ctx = build_context(&self.events, &self.cfg, head);

        if !should_compact(ctx.est_tokens(), &self.cfg) {
            return Ok(());
        }
        let Some((from, to)) = range_for(&self.events, &self.cfg, head) else {
            // Over the threshold but nothing old enough to compact. Pressing on
            // is the right call: the request may still fit, and the provider's
            // error is more informative than a guess here.
            return Ok(());
        };

        // The flush turn. One chance for the model to externalize what it must
        // not forget, before anything is taken away.
        let mut messages = ctx.messages.clone();
        messages.push(Message::user(FLUSH_PROMPT));

        let response = {
            let req = Request {
                model: &self.cfg.model.name,
                temperature: self.cfg.model.temperature,
                messages: &messages,
                tools_json: &ctx.tools_json,
            };
            provider.complete(&req, &mut |_| Flow::Continue)?
        };

        // The flush turn costs real money, so it counts against the budget. It
        // is not appended as a `model_response` because it is not part of the
        // conversation — its entire content survives verbatim as the summary
        // below, so nothing is lost.
        self.usage.add(&response.usage);
        self.spend += self.cfg.price(&response.usage);

        let provenance = provenance(&self.events, from, to);
        self.append(Body::Compaction(Compaction {
            replaces_from: from,
            replaces_to: to,
            summary: response.message.content,
            provenance,
        }))?;

        Ok(())
    }

    fn abort(&mut self, reason: AbortReason, detail: String) -> Result<RunReport> {
        self.append(Body::Abort(Abort {
            reason,
            detail: detail.clone(),
        }))?;
        self.finish(Outcome::Aborted, detail)
    }

    fn finish(&mut self, outcome: Outcome, text: String) -> Result<RunReport> {
        let report = RunReport {
            outcome,
            text: text.clone(),
            turns: self.turns,
            usage: self.usage,
            usd: self.spend,
            cache_hit_rate: crate::log::index::cache_hit_rate(&self.usage),
        };

        self.append(Body::SessionEnd(SessionEnd {
            outcome,
            turns: self.turns,
            usd: self.spend,
            usage: self.usage,
            text,
        }))?;

        Ok(report)
    }
}

fn run_batch(
    idxs: &[usize],
    calls: &[ToolCall],
    cwd: &Path,
    timeout: Duration,
) -> Vec<(usize, ToolResult)> {
    if idxs.len() == 1 {
        let i = idxs[0];
        return vec![(i, run_one(&calls[i], cwd, timeout))];
    }

    std::thread::scope(|scope| {
        let handles: Vec<_> = idxs
            .iter()
            .map(|&i| {
                let call = &calls[i];
                scope.spawn(move || (i, run_one(call, cwd, timeout)))
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| panic!("tool thread panicked")))
            .collect()
    })
}

fn run_one(call: &ToolCall, cwd: &Path, timeout: Duration) -> ToolResult {
    let blank = ToolResult {
        call_id: call.id.clone(),
        exit_code: -1,
        output: String::new(),
        wall_ms: 0,
        timed_out: false,
    };

    if call.name != BASH {
        return ToolResult {
            output: format!(
                "[no such tool: {:?}. The only available tool is `bash`.]",
                call.name
            ),
            ..blank
        };
    }

    let Some(JsonValue::Str(cmd)) = call.args.get("cmd") else {
        return ToolResult {
            output: "[bash requires a string argument `cmd`]".to_string(),
            ..blank
        };
    };

    match run_bash(cmd, cwd, timeout) {
        Ok(o) => ToolResult {
            call_id: call.id.clone(),
            exit_code: o.exit_code,
            output: o.output,
            wall_ms: o.wall_ms,
            timed_out: o.timed_out,
        },
        // A tool that fails to spawn is a fact the model should see, not an
        // error that kills the run.
        Err(e) => ToolResult {
            output: format!("[harness could not run the command: {e}]"),
            ..blank
        },
    }
}

/// A session id that sorts chronologically and needs no dependency.
pub fn new_session_id(now_ms: u64, salt: u32) -> String {
    format!("s{now_ms:012x}{salt:04x}")
}

impl Session {
    /// Convenience for callers that just want a checkpoint taken now.
    pub fn checkpoint(&mut self, label: &str) -> Result<()> {
        if !super::checkpoint::is_git_repo(&self.workspace) {
            return Err(Error::other(
                "workspace is not a git repository; checkpoints need one",
            ));
        }
        let seq = self.head();
        let ckpt = super::checkpoint::create(&self.workspace, self.log.session(), label, seq)?;
        self.append(Body::Checkpoint(ckpt))?;
        Ok(())
    }
}
