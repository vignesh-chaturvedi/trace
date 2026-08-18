//! End to end, through the real binary.
//!
//! The CLI is meant to be a thin consumer of the library, so this checks the
//! seam rather than the logic: that a run produces a log, that `replay`
//! rebuilds every context from it offline, and that the exit codes mean
//! something to a CI job.

use std::path::{Path, PathBuf};
use std::process::Command;

use trace_core::config::Config;
use trace_core::provider::{Provider, RecordingProvider, ScriptedProvider};
use trace_core::runtime::session::{Session, StartArgs};

const BIN: &str = env!("CARGO_BIN_EXE_trace");

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("trace-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("ws")).unwrap();
        Scratch(path)
    }
    fn join(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CONFIG: &str = r#"
[model]
name = "test-model"
price_in_per_mtok = 1.0
price_out_per_mtok = 4.0
price_cached_in_per_mtok = 0.1

[limits]
tool_timeout_ms = 10000
"#;

const TASK: &str = "write a file and read it back";

/// Produce a fixture by running the session once through a script.
///
/// The fixture is keyed by context hash, so it only matches if this run and
/// the CLI's run agree byte for byte on config, task, and workspace — which is
/// itself part of what the test is checking.
fn record_fixture(scratch: &Scratch, cfg: &Config, workspace: &Path) -> PathBuf {
    let fixture = scratch.join("fixture.jsonl");
    let provider = RecordingProvider::new(
        ScriptedProvider::new(vec![
            ScriptedProvider::bash("c1", "echo hello > out.txt"),
            ScriptedProvider::bash("c2", "cat out.txt"),
            ScriptedProvider::say("Verified: out.txt contains hello."),
        ]),
        &fixture,
    )
    .unwrap();

    let mut session = Session::start(
        cfg.clone(),
        StartArgs {
            log_path: &scratch.join("seed.jsonl"),
            session_id: "seed".into(),
            task: TASK.into(),
            workspace: workspace.to_path_buf(),
            agents_md: String::new(),
            harness_commit: "seed".into(),
        },
    )
    .unwrap();
    session
        .run(&provider as &dyn Provider, &mut |_| {})
        .unwrap();

    fixture
}

fn trace(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN).args(args).output().expect("run trace");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn run_then_replay_then_inspect() {
    let scratch = Scratch::new("full");
    let config_path = scratch.join("trace.toml");
    std::fs::write(&config_path, CONFIG).unwrap();
    let cfg = Config::load(&config_path).unwrap();

    let workspace = scratch.join("ws").canonicalize().unwrap();
    let fixture = record_fixture(&scratch, &cfg, &workspace);

    // 1. Run.
    let (code, stdout, stderr) = trace(&[
        "--config",
        config_path.to_str().unwrap(),
        "run",
        TASK,
        "--workspace",
        workspace.to_str().unwrap(),
        "--out",
        scratch.join("runs").to_str().unwrap(),
        "--fixture",
        fixture.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "run failed\nstdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("outcome        Done"), "{stdout}");
    assert!(stdout.contains("cache hit"), "{stdout}");

    // The tool actually ran in the workspace.
    assert_eq!(
        std::fs::read_to_string(workspace.join("out.txt"))
            .unwrap()
            .trim(),
        "hello"
    );

    let log = std::fs::read_dir(scratch.join("runs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .expect("a log was written");

    // 2. Replay: offline, no provider, no key.
    let (code, stdout, stderr) = trace(&[
        "--config",
        config_path.to_str().unwrap(),
        "replay",
        log.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "replay failed\n{stdout}{stderr}");
    assert!(stdout.contains("byte-identical"), "{stdout}");

    // 3. Inspect.
    let (code, stdout, _) = trace(&["inspect", log.to_str().unwrap()]);
    assert_eq!(code, 0);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(summary["task"], TASK);
    assert_eq!(summary["outcome"], "done");
    assert_eq!(summary["tool_calls"], 2);

    // 4. Index.
    let (code, stdout, _) = trace(&["index", scratch.join("runs").to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("indexed 1 sessions"));
}

/// Replay must notice when the config it is handed would have produced a
/// different context. Silently succeeding here would make every downstream
/// ablation meaningless.
#[test]
fn replay_detects_a_config_that_changes_the_context() {
    let scratch = Scratch::new("drift");
    let config_path = scratch.join("trace.toml");
    std::fs::write(&config_path, CONFIG).unwrap();
    let cfg = Config::load(&config_path).unwrap();

    let workspace = scratch.join("ws").canonicalize().unwrap();
    let fixture = record_fixture(&scratch, &cfg, &workspace);

    let (code, _, _) = trace(&[
        "--config",
        config_path.to_str().unwrap(),
        "run",
        TASK,
        "--workspace",
        workspace.to_str().unwrap(),
        "--out",
        scratch.join("runs").to_str().unwrap(),
        "--fixture",
        fixture.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);

    let log = std::fs::read_dir(scratch.join("runs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .unwrap();

    let altered = scratch.join("altered.toml");
    std::fs::write(
        &altered,
        format!("{CONFIG}\n[prompt]\nsystem = \"A different system prompt. {{cwd}}\"\n"),
    )
    .unwrap();

    let (code, _, stderr) = trace(&[
        "--config",
        altered.to_str().unwrap(),
        "replay",
        log.to_str().unwrap(),
    ]);
    assert_ne!(code, 0, "replay under a changed config should not pass");
    assert!(stderr.contains("diverged"), "{stderr}");
}

/// Exit codes carry meaning, so CI can act on them.
#[test]
fn exit_codes_distinguish_outcomes() {
    let scratch = Scratch::new("exit");
    let config_path = scratch.join("trace.toml");
    std::fs::write(&config_path, format!("{CONFIG}\nmax_turns = 2\n")).unwrap();

    let workspace = scratch.join("ws").canonicalize().unwrap();
    let cfg = Config::load(&config_path).unwrap();
    let fixture = record_fixture(&scratch, &cfg, &workspace);

    // A turn cap the run cannot satisfy.
    let (code, stdout, _) = trace(&[
        "--config",
        config_path.to_str().unwrap(),
        "run",
        TASK,
        "--workspace",
        workspace.to_str().unwrap(),
        "--out",
        scratch.join("runs").to_str().unwrap(),
        "--fixture",
        fixture.to_str().unwrap(),
        "--max-turns",
        "1",
    ]);
    assert_eq!(code, 2, "budget/turn-cap aborts should exit 2\n{stdout}");
}

/// A planted timestamp in the system prompt must stop a run before it costs
/// anything.
#[test]
fn a_broken_layout_refuses_to_run() {
    let scratch = Scratch::new("lint");
    let config_path = scratch.join("trace.toml");
    std::fs::write(
        &config_path,
        format!("{CONFIG}\n[prompt]\nsystem = \"Now: 2026-08-18T09:00:00Z. {{cwd}}\"\n"),
    )
    .unwrap();

    let (code, _, stderr) = trace(&["--config", config_path.to_str().unwrap(), "lint"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("layout lint failed"), "{stderr}");
}
