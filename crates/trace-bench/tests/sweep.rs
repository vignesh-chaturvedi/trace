//! The rig's own discipline, tested.
//!
//! A benchmark that scores from the agent's summary, reuses a workspace
//! between repeats, or quietly relaxes a timeout still produces numbers. They
//! just do not mean anything, and nobody finds out until a reproduction fails.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use trace_core::config::Config;
use trace_core::provider::{Provider, Response, ScriptedProvider};

use trace_bench::adapter::LocalAdapter;
use trace_bench::report;
use trace_bench::result::{aggregate, TaskResult};
use trace_bench::sweep::{run_sweep, SweepOptions};
use trace_bench::task::Task;

static N: AtomicU32 = AtomicU32::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Scratch {
        let p = std::env::temp_dir().join(format!(
            "trace-bench-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tasks_root() -> PathBuf {
    // crates/trace-bench -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tasks")
}

fn cfg() -> Config {
    let mut c = Config::default();
    c.model.name = "scripted".into();
    c.model.price_in_per_mtok = 1.0;
    c.model.price_out_per_mtok = 4.0;
    c.limits.tool_timeout_ms = 20_000;
    c
}

fn opts(dir: &Scratch, repeats: u32) -> SweepOptions {
    SweepOptions {
        repeats,
        limit: None,
        out_dir: dir.path().join("bench"),
        harness_commit: "testcommit".into(),
        verbose: false,
    }
}

fn only(id: &str) -> Vec<Task> {
    vec![Task::load(&tasks_root().join(id)).expect("load task")]
}

fn factory(turns: Vec<Response>) -> impl Fn(&Config) -> trace_core::Result<Box<dyn Provider>> {
    // A fresh provider per attempt, so each repeat replays the script from the
    // start rather than continuing where the previous one stopped.
    move |_| Ok(Box::new(ScriptedProvider::new(turns.clone())) as Box<dyn Provider>)
}

// ---------------------------------------------------------------- tasks

#[test]
fn the_shipped_tasks_all_load() {
    let tasks = Task::load_all(&tasks_root()).expect("load all");
    assert!(tasks.len() >= 3);
    // Sorted, so two machines run the same sweep in the same order.
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn a_task_without_verification_is_rejected() {
    let dir = Scratch::new("noverify");
    let t = dir.path().join("bad");
    std::fs::create_dir_all(t.join("workspace")).unwrap();
    std::fs::write(
        t.join("task.toml"),
        "id = \"bad\"\nprompt = \"do a thing\"\n",
    )
    .unwrap();

    let err = Task::load(&t).unwrap_err().to_string();
    assert!(err.contains("verify.sh"), "{err}");
}

/// The benchmark is allowed to be stricter than the operator, never more
/// permissive. Tuning limits locally is how numbers stop transferring.
#[test]
fn task_limits_tighten_but_never_loosen() {
    let task = only("fix-sum").remove(0);

    let mut generous = cfg();
    generous.limits.max_turns = 500;
    generous.limits.max_usd = 100.0;
    let applied = task.apply_limits(&generous);
    assert_eq!(applied.limits.max_turns, 20);
    assert!((applied.limits.max_usd - 0.25).abs() < 1e-9);

    let mut strict = cfg();
    strict.limits.max_turns = 5;
    strict.limits.max_usd = 0.01;
    let applied = task.apply_limits(&strict);
    assert_eq!(applied.limits.max_turns, 5, "a task must not raise the cap");
    assert!((applied.limits.max_usd - 0.01).abs() < 1e-9);
}

// ---------------------------------------------------------------- scoring

#[test]
fn an_agent_that_fixes_the_bug_passes() {
    let dir = Scratch::new("pass");
    let turns = vec![
        ScriptedProvider::bash(
            "c1",
            "sed -i.bak 's/total - n/total + n/' sum.sh && bash sum.sh",
        ),
        ScriptedProvider::say("Fixed the operator; sum.sh now prints 108."),
    ];

    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(turns),
        &opts(&dir, 3),
    )
    .unwrap();

    assert_eq!(report.rows.len(), 3);
    assert!(report.rows.iter().all(|r| r.passed), "{:?}", report.rows);
    assert!((report.aggregate.pass_rate - 1.0).abs() < 1e-9);
    assert_eq!(report.aggregate.pass_rate_sigma, 0.0);
}

/// The failure mode the manual names outright: the agent says it is done, and
/// a harness that believes it reports a pass for work that never happened.
#[test]
fn an_agent_that_only_claims_success_fails() {
    let dir = Scratch::new("liar");
    let turns = vec![ScriptedProvider::say(
        "I have fixed sum.sh and verified it. All tests pass.",
    )];

    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(turns),
        &opts(&dir, 3),
    )
    .unwrap();

    assert!(
        report.rows.iter().all(|r| !r.passed),
        "the agent's summary was trusted over the task's own suite"
    );
    assert_eq!(report.aggregate.pass_rate, 0.0);
    assert!(report.rows.iter().all(|r| r.error.is_none()));
}

/// An agent that deletes the source and declares victory must not pass either.
#[test]
fn destroying_the_workspace_does_not_pass() {
    let dir = Scratch::new("vandal");
    let turns = vec![
        ScriptedProvider::bash("c1", "rm -f sum.sh numbers.txt"),
        ScriptedProvider::say("Cleaned up. Done."),
    ];

    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(turns),
        &opts(&dir, 3),
    )
    .unwrap();
    assert!(report.rows.iter().all(|r| !r.passed));
}

// ------------------------------------------------------- isolation

/// If the agent can read the thing that grades it, it can satisfy it without
/// doing the work.
#[test]
fn the_verification_script_is_invisible_during_the_run() {
    let dir = Scratch::new("hidden");
    let turns = vec![
        ScriptedProvider::bash("c1", "ls -a . && cat verify.sh 2>&1 || true"),
        ScriptedProvider::say("looked around"),
    ];

    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(turns),
        &opts(&dir, 3),
    )
    .unwrap();

    let trajectory = &report.rows[0].trajectory;
    let events = trace_core::log::read(Path::new(trajectory)).unwrap().events;
    let seen: String = events
        .iter()
        .filter_map(|e| e.as_tool_result().map(|r| r.output.clone()))
        .collect();

    // Check for the script's *contents*, not its name: a failed `cat` echoes
    // the filename back, which would make a name-based assertion self-defeating.
    assert!(
        !seen.contains("PASS: sum.sh") && !seen.contains("expected 108"),
        "the agent could read the script that grades it:\n{seen}"
    );
    assert!(
        seen.contains("No such file") || seen.contains("cannot open"),
        "expected the cat to fail; instead got:\n{seen}"
    );
    // And it is nowhere in the listing either.
    let listing_has_it = seen
        .lines()
        .any(|l| l.trim() == "verify.sh" || l.trim() == ".trace-verify.sh");
    assert!(
        !listing_has_it,
        "the grader was sitting in the workspace:\n{seen}"
    );
}

/// Repeats must be independent. If repeat 2 inherits repeat 1's fix, the
/// variance you compute is a fiction.
#[test]
fn each_repeat_gets_a_clean_workspace() {
    let dir = Scratch::new("fresh");
    // Fix it only if it is still broken; then report what the file contains.
    let turns = vec![
        ScriptedProvider::bash("c1", "grep -c 'total - n' sum.sh || true"),
        ScriptedProvider::say("checked"),
    ];

    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(turns),
        &opts(&dir, 3),
    )
    .unwrap();

    for row in &report.rows {
        let events = trace_core::log::read(Path::new(&row.trajectory))
            .unwrap()
            .events;
        let out: String = events
            .iter()
            .filter_map(|e| e.as_tool_result().map(|r| r.output.clone()))
            .collect();
        assert!(
            out.trim().starts_with('1'),
            "repeat {} did not start from the pristine seed: {out:?}",
            row.repeat
        );
    }
}

// ---------------------------------------------------------------- rigour

#[test]
fn fewer_than_three_repeats_is_refused() {
    let dir = Scratch::new("repeats");
    let err = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(vec![ScriptedProvider::say("hi")]),
        &opts(&dir, 1),
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("not a measurement"), "{err}");
}

#[test]
fn limit_truncates_the_task_set() {
    let dir = Scratch::new("limit");
    let mut o = opts(&dir, 3);
    o.limit = Some(1);

    let report = run_sweep(
        &Task::load_all(&tasks_root()).unwrap(),
        &cfg(),
        &LocalAdapter,
        &factory(vec![ScriptedProvider::say("nope")]),
        &o,
    )
    .unwrap();

    assert_eq!(report.aggregate.tasks, 1);
    assert_eq!(report.rows.len(), 3);
}

#[test]
fn every_row_carries_its_provenance() {
    let dir = Scratch::new("provenance");
    let report = run_sweep(
        &only("fix-sum"),
        &cfg(),
        &LocalAdapter,
        &factory(vec![ScriptedProvider::say("no")]),
        &opts(&dir, 3),
    )
    .unwrap();

    for row in &report.rows {
        assert_eq!(row.harness_commit, "testcommit");
        assert!(!row.config_hash.is_empty());
        assert!(
            Path::new(&row.trajectory).exists(),
            "trajectory is unreadable"
        );
    }
    // Results survive the process, appended as they complete.
    let results = report.dir.join("results.jsonl");
    assert_eq!(trace_bench::sweep::read_results(&results).unwrap().len(), 3);
    assert!(report.dir.join("manifest.json").exists());
    assert!(report.dir.join("aggregate.json").exists());
}

// ---------------------------------------------------------------- stats

fn row(task: &str, repeat: u32, passed: bool, error: bool) -> TaskResult {
    TaskResult {
        task_id: task.into(),
        repeat,
        passed,
        turns: 5,
        wall_ms: 1000,
        tokens: Default::default(),
        usd: 0.01,
        abort_reason: None,
        model: "m".into(),
        harness_commit: "c".into(),
        config_hash: "h".into(),
        trajectory: "t".into(),
        error: error.then(|| "boom".to_string()),
    }
}

#[test]
fn sigma_describes_run_to_run_variance() {
    // Two tasks, three repeats: 2/2, 1/2, 1/2 -> rates 1.0, 0.5, 0.5
    let rows = vec![
        row("a", 0, true, false),
        row("b", 0, true, false),
        row("a", 1, true, false),
        row("b", 1, false, false),
        row("a", 2, true, false),
        row("b", 2, false, false),
    ];
    let agg = aggregate("m", &rows, 3);

    assert!((agg.pass_rate - 2.0 / 3.0).abs() < 1e-9);
    assert!(
        agg.pass_rate_sigma > 0.28 && agg.pass_rate_sigma < 0.29,
        "{}",
        agg.pass_rate_sigma
    );
    assert_eq!(agg.flaky_task_ids, vec!["b".to_string()]);
}

/// A provider outage is not the agent getting the task wrong. Folding them
/// together silently deflates the score with no trace in the number.
#[test]
fn harness_errors_are_excluded_from_the_pass_rate() {
    let rows = vec![
        row("a", 0, true, false),
        row("b", 0, false, true),
        row("a", 1, true, false),
        row("b", 1, false, true),
        row("a", 2, true, false),
        row("b", 2, false, true),
    ];
    let agg = aggregate("m", &rows, 3);

    assert_eq!(agg.pass_rate, 1.0, "errored runs were counted as failures");
    assert_eq!(agg.harness_errors, 3);
    assert!(report::summary(&agg).contains("WARNING"));
}

#[test]
fn comparison_answers_within_noise_or_not() {
    let steady = |p: f64, s: f64| trace_bench::Aggregate {
        pass_rate: p,
        pass_rate_sigma: s,
        ..Default::default()
    };

    assert!(report::compare(&steady(0.50, 0.05), &steady(0.52, 0.05)).contains("within noise"));
    assert!(report::compare(&steady(0.50, 0.01), &steady(0.70, 0.01))
        .contains("improvement beyond noise"));
    assert!(report::compare(&steady(0.70, 0.01), &steady(0.50, 0.01))
        .contains("regression beyond noise"));
}
