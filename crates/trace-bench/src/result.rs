//! Per-task result rows, and what you can honestly say about a set of them.

use serde::{Deserialize, Serialize};

use trace_core::event::{AbortReason, Usage};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TaskResult {
    pub task_id: String,
    pub repeat: u32,
    /// Decided by the task's own verification suite. Never by the agent.
    pub passed: bool,
    pub turns: u64,
    pub wall_ms: u64,
    pub tokens: Usage,
    pub usd: f64,
    pub abort_reason: Option<AbortReason>,
    pub model: String,
    /// A score without a commit is a rumour.
    pub harness_commit: String,
    pub config_hash: String,
    /// Path to the full trajectory, so any row can be opened and read.
    pub trajectory: String,
    /// Populated when the harness itself failed, as distinct from the agent
    /// failing the task. Conflating the two silently deflates your score.
    pub error: Option<String>,
}

impl TaskResult {
    pub fn cache_hit_rate(&self) -> f64 {
        if self.tokens.input == 0 {
            0.0
        } else {
            self.tokens.cached_input as f64 / self.tokens.input as f64
        }
    }
}

/// What a set of result rows adds up to.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Aggregate {
    pub model: String,
    pub tasks: usize,
    pub repeats: usize,
    pub runs: usize,
    /// Mean pass rate across repeats.
    pub pass_rate: f64,
    /// Standard deviation of the per-repeat pass rates.
    ///
    /// Reported always, because a score without a spread invites you to read
    /// noise as progress. If sigma is larger than the delta you are excited
    /// about, you have not measured anything yet.
    pub pass_rate_sigma: f64,
    pub total_usd: f64,
    pub usd_per_run: f64,
    pub mean_turns: f64,
    pub tokens: Usage,
    pub cache_hit_rate: f64,
    pub harness_errors: usize,
    /// Tasks that passed on some repeats and failed on others — the ones
    /// worth looking at before trusting any headline number.
    pub flaky_task_ids: Vec<String>,
}

pub fn aggregate(model: &str, rows: &[TaskResult], repeats: usize) -> Aggregate {
    if rows.is_empty() {
        return Aggregate {
            model: model.to_string(),
            ..Default::default()
        };
    }

    let mut task_ids: Vec<String> = rows.iter().map(|r| r.task_id.clone()).collect();
    task_ids.sort();
    task_ids.dedup();

    // Pass rate per repeat, so the spread describes run-to-run variance rather
    // than task difficulty.
    let mut per_repeat: Vec<f64> = Vec::new();
    for r in 0..repeats as u32 {
        // Harness errors are excluded, not counted as failures. A provider
        // outage is not the agent getting the task wrong, and folding the two
        // together quietly deflates the score with no trace in the number.
        let of_repeat: Vec<&TaskResult> = rows
            .iter()
            .filter(|x| x.repeat == r && x.error.is_none())
            .collect();
        if of_repeat.is_empty() {
            continue;
        }
        let passed = of_repeat.iter().filter(|x| x.passed).count();
        per_repeat.push(passed as f64 / of_repeat.len() as f64);
    }

    let mean = |v: &[f64]| -> f64 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let pass_rate = mean(&per_repeat);
    let sigma = if per_repeat.len() < 2 {
        0.0
    } else {
        let m = pass_rate;
        // Sample standard deviation: with a handful of repeats the population
        // form understates the spread, which is the wrong way to be wrong.
        let var =
            per_repeat.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (per_repeat.len() - 1) as f64;
        var.sqrt()
    };

    let mut flaky: Vec<String> = task_ids
        .iter()
        .filter(|id| {
            let outcomes: Vec<bool> = rows
                .iter()
                .filter(|r| &&r.task_id == id && r.error.is_none())
                .map(|r| r.passed)
                .collect();
            outcomes.iter().any(|x| *x) && outcomes.iter().any(|x| !*x)
        })
        .cloned()
        .collect();
    flaky.sort();

    let mut tokens = Usage::default();
    for r in rows {
        tokens.add(&r.tokens);
    }

    let total_usd: f64 = rows.iter().map(|r| r.usd).sum();
    let cache_hit_rate = if tokens.input == 0 {
        0.0
    } else {
        tokens.cached_input as f64 / tokens.input as f64
    };

    Aggregate {
        model: model.to_string(),
        tasks: task_ids.len(),
        repeats: per_repeat.len(),
        runs: rows.len(),
        pass_rate,
        pass_rate_sigma: sigma,
        total_usd,
        usd_per_run: total_usd / rows.len() as f64,
        mean_turns: rows.iter().map(|r| r.turns as f64).sum::<f64>() / rows.len() as f64,
        tokens,
        cache_hit_rate,
        harness_errors: rows.iter().filter(|r| r.error.is_some()).count(),
        flaky_task_ids: flaky,
    }
}
