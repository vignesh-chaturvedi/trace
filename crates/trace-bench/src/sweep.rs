//! Running a sweep.
//!
//! The discipline the manual insists on, encoded:
//!
//! * **Three repeats minimum.** One repeat is not a measurement, and the rig
//!   refuses to pretend otherwise.
//! * **A fresh workspace per attempt.** Repeat 2 must not inherit repeat 1's
//!   half-finished edits, or the repeats stop being independent and the
//!   variance you compute is a fiction.
//! * **Never loosen the benchmark's limits.** [`Task::apply_limits`] can only
//!   tighten.
//! * **Score from the task's own suite.** The agent's summary is not evidence.
//! * **Every row carries its harness commit.** A score without one is a rumour.
//!
//! Sequential by design. Concurrency would help wall time and hurt everything
//! else right now: it complicates rate limiting, makes per-task cost harder to
//! attribute, and interleaves output. It is an optimization to make once the
//! numbers are trustworthy, not before.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use trace_core::config::Config;
use trace_core::error::{Error, Result};
use trace_core::event::{Body, Outcome};
use trace_core::provider::Provider;
use trace_core::runtime::session::{Session, StartArgs};

use crate::adapter::Adapter;
use crate::result::{aggregate, Aggregate, TaskResult};
use crate::task::Task;

pub struct SweepOptions {
    /// Minimum 3, enforced. See `MIN_REPEATS`.
    pub repeats: u32,
    /// Run only the first N tasks. For shakedown runs and thin API budgets.
    pub limit: Option<usize>,
    pub out_dir: PathBuf,
    pub harness_commit: String,
    /// Print a line per attempt.
    pub verbose: bool,
}

pub const MIN_REPEATS: u32 = 3;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SweepManifest {
    pub sweep_id: String,
    pub started_ms: u64,
    pub harness_commit: String,
    pub config_hash: String,
    pub model: String,
    pub repeats: u32,
    pub task_ids: Vec<String>,
}

#[derive(Debug)]
pub struct SweepReport {
    pub manifest: SweepManifest,
    pub rows: Vec<TaskResult>,
    pub aggregate: Aggregate,
    pub dir: PathBuf,
}

/// Build a provider for a given config. A factory rather than a shared
/// instance so a fixture-backed sweep can hand each run its own cursor.
pub type ProviderFactory<'a> = dyn Fn(&Config) -> Result<Box<dyn Provider>> + 'a;

pub fn run_sweep(
    tasks: &[Task],
    cfg: &Config,
    adapter: &dyn Adapter,
    provider_for: &ProviderFactory<'_>,
    opts: &SweepOptions,
) -> Result<SweepReport> {
    if opts.repeats < MIN_REPEATS {
        return Err(Error::Config(format!(
            "repeats = {} but the minimum is {MIN_REPEATS}. One repeat is not a measurement; \
             without a spread you cannot tell a real change from noise.",
            opts.repeats
        )));
    }

    let tasks: &[Task] = match opts.limit {
        Some(n) => &tasks[..n.min(tasks.len())],
        None => tasks,
    };

    let started_ms = now_ms();
    // Time of day, not just the date. Two sweeps of the same config on the
    // same day are two sweeps, and they must not land in one directory: the
    // event log refuses to overwrite an existing trajectory (correctly), so a
    // collision fails only the tasks that ran last time. That reads as "those
    // tasks are broken" rather than "you already ran this", which is a
    // genuinely misleading way to lose an afternoon.
    let sweep_id = format!(
        "{}-{:05}-{}",
        iso_date(started_ms),
        (started_ms / 1000) % 86_400,
        &cfg.hash()[..8]
    );
    let dir = opts.out_dir.join(&sweep_id).join(slug(&cfg.model.name));
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

    let manifest = SweepManifest {
        sweep_id: sweep_id.clone(),
        started_ms,
        harness_commit: opts.harness_commit.clone(),
        config_hash: cfg.hash(),
        model: cfg.model.name.clone(),
        repeats: opts.repeats,
        task_ids: tasks.iter().map(|t| t.id.clone()).collect(),
    };
    write_json(&dir.join("manifest.json"), &manifest)?;

    // Results are appended as they complete, so an interrupted sweep still
    // leaves everything it managed to measure.
    let results_path = dir.join("results.jsonl");
    let mut rows: Vec<TaskResult> = Vec::with_capacity(tasks.len() * opts.repeats as usize);

    for repeat in 0..opts.repeats {
        for task in tasks {
            let row = run_one(task, cfg, adapter, provider_for, opts, &dir, repeat);
            append_jsonl(&results_path, &row)?;

            if opts.verbose {
                let mark = match (&row.error, row.passed) {
                    (Some(_), _) => "ERR ",
                    (None, true) => "pass",
                    (None, false) => "fail",
                };
                println!(
                    "  {mark}  {:<28} r{repeat}  {:>3} turns  ${:.4}  {:.1}s",
                    row.task_id,
                    row.turns,
                    row.usd,
                    row.wall_ms as f64 / 1000.0
                );
            }
            rows.push(row);
        }
    }

    let agg = aggregate(&cfg.model.name, &rows, opts.repeats as usize);
    write_json(&dir.join("aggregate.json"), &agg)?;

    Ok(SweepReport {
        manifest,
        rows,
        aggregate: agg,
        dir,
    })
}

fn run_one(
    task: &Task,
    cfg: &Config,
    adapter: &dyn Adapter,
    provider_for: &ProviderFactory<'_>,
    opts: &SweepOptions,
    dir: &Path,
    repeat: u32,
) -> TaskResult {
    let started = Instant::now();
    let run_dir = dir.join(format!("{}.r{repeat}", task.id));
    let trajectory = run_dir.join("trajectory.jsonl");

    // The benchmark's limits, never the operator's, and only tighter.
    let cfg = task.apply_limits(cfg);

    let mut row = TaskResult {
        task_id: task.id.clone(),
        repeat,
        passed: false,
        turns: 0,
        wall_ms: 0,
        tokens: Default::default(),
        usd: 0.0,
        abort_reason: None,
        model: cfg.model.name.clone(),
        harness_commit: opts.harness_commit.clone(),
        config_hash: cfg.hash(),
        trajectory: trajectory.display().to_string(),
        error: None,
    };

    let finish = |mut row: TaskResult, started: Instant, err: Option<String>| -> TaskResult {
        row.wall_ms = started.elapsed().as_millis() as u64;
        if let Some(e) = err {
            row.error = Some(e);
            row.passed = false;
        }
        row
    };

    let workspace = match adapter.prepare(task, &run_dir) {
        Ok(w) => w,
        Err(e) => return finish(row, started, Some(format!("prepare failed: {e}"))),
    };

    let provider = match provider_for(&cfg) {
        Ok(p) => p,
        Err(e) => {
            adapter.cleanup(&workspace);
            return finish(row, started, Some(format!("provider unavailable: {e}")));
        }
    };

    let executor = match adapter.executor(&workspace) {
        Ok(x) => x,
        Err(e) => {
            adapter.cleanup(&workspace);
            return finish(row, started, Some(format!("executor unavailable: {e}")));
        }
    };

    let mut session = match Session::start_with(
        cfg.clone(),
        StartArgs {
            log_path: &trajectory,
            session_id: format!("{}-r{repeat}", task.id),
            task: task.prompt.clone(),
            workspace: workspace.clone(),
            agents_md: std::fs::read_to_string(workspace.join("AGENTS.md")).unwrap_or_default(),
            harness_commit: opts.harness_commit.clone(),
        },
        executor,
    ) {
        Ok(s) => s,
        Err(e) => {
            adapter.cleanup(&workspace);
            return finish(row, started, Some(format!("session start failed: {e}")));
        }
    };

    session.with_deadline(Instant::now() + task.wall_timeout());

    match session.run(provider.as_ref(), &mut |_| {}) {
        Ok(report) => {
            row.turns = report.turns;
            row.tokens = report.usage;
            row.usd = report.usd;
            if report.outcome == Outcome::Aborted {
                row.abort_reason = abort_reason(&trajectory);
            }
        }
        Err(e) => {
            // Salvage whatever the log recorded before the failure, so a
            // partial run still contributes cost accounting.
            if let Ok(summary) = trace_core::log::index::summarize(&trajectory) {
                row.turns = summary.turns;
                row.tokens = summary.usage;
            }
            adapter.cleanup(&workspace);
            return finish(row, started, Some(format!("run failed: {e}")));
        }
    }

    // The only thing that decides `passed`. Not the model's summary, not the
    // absence of an abort — the task's own suite, run after the fact.
    match adapter.verify(task, &workspace) {
        Ok(verdict) => {
            row.passed = verdict.passed;
            let _ = std::fs::write(
                run_dir.join("verify.log"),
                format!("exit {}\n\n{}", verdict.exit_code, verdict.output),
            );
        }
        Err(e) => {
            adapter.cleanup(&workspace);
            return finish(row, started, Some(format!("verify failed: {e}")));
        }
    }

    adapter.cleanup(&workspace);
    finish(row, started, None)
}

fn abort_reason(trajectory: &Path) -> Option<trace_core::event::AbortReason> {
    trace_core::log::read(trajectory)
        .ok()?
        .events
        .iter()
        .rev()
        .find_map(|e| match &e.body {
            Body::Abort(a) => Some(a.reason),
            _ => None,
        })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json + "\n").map_err(|e| Error::io(path, e))
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    writeln!(f, "{}", serde_json::to_string(value)?).map_err(|e| Error::io(path, e))
}

pub fn read_results(path: &Path) -> Result<Vec<TaskResult>> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line).map_err(|e| Error::CorruptLog {
            path: path.to_path_buf(),
            line: i + 1,
            detail: e.to_string(),
        })?);
    }
    Ok(out)
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` from epoch millis, via the civil-from-days algorithm. Avoids a
/// date dependency for the one place a sweep needs a human-readable stamp.
fn iso_date(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Unused today, kept honest: sweeps must not silently take longer than the
/// caller expects.
pub const DEFAULT_TASK_WALL: Duration = Duration::from_secs(900);
