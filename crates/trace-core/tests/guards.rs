//! Test plan: "Doom-loop detector fires on a synthetic loop and not on
//! flaky-but-varying output" and "Budget guards abort with a recorded event,
//! mid-stream."

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::config::GuardConfig;
use trace_core::event::{
    AbortReason, Body, Event, ObservationSource, Outcome, ToolCall, ToolResult,
};
use trace_core::message::JsonValue;
use trace_core::provider::{Provider, Response, ScriptedProvider};
use trace_core::runtime::guards::{detect_doom_loop, fingerprint};

fn call(id: &str, cmd: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "bash".into(),
        args: [("cmd".to_string(), JsonValue::Str(cmd.into()))]
            .into_iter()
            .collect(),
    }
}

fn result(id: &str, output: &str, exit: i32) -> ToolResult {
    ToolResult {
        call_id: id.into(),
        exit_code: exit,
        output: output.into(),
        wall_ms: 1,
        timed_out: false,
    }
}

/// Build a log of alternating call/result pairs.
fn history(pairs: &[(&str, &str, i32)]) -> Vec<Event> {
    let mut events = Vec::new();
    let mut seq = 1u64;
    for (i, (cmd, out, exit)) in pairs.iter().enumerate() {
        let id = format!("c{i}");
        events.push(Event {
            seq,
            ts_ms: seq * 10,
            session: "s".into(),
            body: Body::ToolCall(call(&id, cmd)),
        });
        seq += 1;
        events.push(Event {
            seq,
            ts_ms: seq * 10,
            session: "s".into(),
            body: Body::ToolResult(result(&id, out, *exit)),
        });
        seq += 1;
    }
    events
}

#[test]
fn fires_on_an_identical_repeat() {
    let cfg = GuardConfig::default();
    let events = history(&[
        ("cargo test", "FAILED: 1 assertion", 1),
        ("cargo test", "FAILED: 1 assertion", 1),
        ("cargo test", "FAILED: 1 assertion", 1),
    ]);

    let hit = detect_doom_loop(&events, &cfg).expect("detector should fire");
    assert_eq!(hit.count, 3);
    assert!(hit.text.contains("try a different approach"));
    assert!(
        hit.text.contains("3 times"),
        "the nudge must report the count it actually saw: {}",
        hit.text
    );
}

/// The distinction the whole guard turns on. Retrying a flaky test three times
/// is engineering. Getting byte-identical failure output three times is an
/// agent that has stopped learning anything.
#[test]
fn does_not_fire_on_flaky_but_varying_output() {
    let cfg = GuardConfig::default();
    let events = history(&[
        ("cargo test", "FAILED: timeout after 30s", 1),
        ("cargo test", "FAILED: timeout after 31s", 1),
        ("cargo test", "ok. 41 passed", 0),
    ]);

    assert!(detect_doom_loop(&events, &cfg).is_none());
}

/// A reconfigured threshold must not leave the message claiming a number the
/// transcript does not support.
#[test]
fn the_nudge_reports_the_configured_threshold() {
    let cfg = GuardConfig {
        loop_window: 8,
        loop_threshold: 4,
    };
    let events = history(&[("make", "same", 2); 4]);
    let hit = detect_doom_loop(&events, &cfg).expect("detector should fire at 4");
    assert!(hit.text.contains("4 times"), "{}", hit.text);
}

#[test]
fn does_not_fire_below_the_threshold() {
    let cfg = GuardConfig::default();
    let events = history(&[("ls", "a b c", 0), ("ls", "a b c", 0)]);
    assert!(detect_doom_loop(&events, &cfg).is_none());
}

/// The same command producing a different exit code is new information.
#[test]
fn exit_code_is_part_of_the_fingerprint() {
    let a = fingerprint(&call("c1", "make"), &result("c1", "done", 0));
    let b = fingerprint(&call("c1", "make"), &result("c1", "done", 1));
    assert_ne!(a, b);
}

#[test]
fn identical_work_under_different_call_ids_still_matches() {
    let a = fingerprint(&call("call_aaa", "make"), &result("call_aaa", "done", 0));
    let b = fingerprint(&call("call_zzz", "make"), &result("call_zzz", "done", 0));
    assert_eq!(a, b, "the call id is incidental; the work is what repeats");
}

/// Repeating the nudge every turn past the threshold trains the model to
/// ignore it, and each repetition is context the agent has to read.
#[test]
fn fires_once_per_episode_not_every_turn() {
    let cfg = GuardConfig::default();
    let mut pairs = vec![("cargo test", "FAILED", 1); 3];
    assert!(detect_doom_loop(&history(&pairs), &cfg).is_some());

    pairs.push(("cargo test", "FAILED", 1));
    assert!(detect_doom_loop(&history(&pairs), &cfg).is_none());
}

/// End to end: a looping agent gets told so, in its own transcript.
#[test]
fn a_looping_session_records_an_observation() {
    let dir = TempDir::new("doomloop");
    let cfg = test_config();

    let turns = vec![
        ScriptedProvider::bash("c1", "echo stuck"),
        ScriptedProvider::bash("c2", "echo stuck"),
        ScriptedProvider::bash("c3", "echo stuck"),
        ScriptedProvider::say("giving up"),
    ];
    let (_, events) = scripted_session(&dir, &cfg, "loop forever", turns);

    assert!(
        events.iter().any(|e| matches!(
            &e.body,
            Body::Observation(o) if o.source == ObservationSource::DoomLoop
        )),
        "no doom-loop observation was recorded"
    );
}

/// A provider that streams far more than the budget allows.
struct Firehose;

impl Provider for Firehose {
    fn complete(
        &self,
        _req: &trace_core::provider::Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> trace_core::provider::Flow,
    ) -> trace_core::Result<Response> {
        let mut emitted = 0usize;
        for _ in 0..10_000 {
            if on_delta("....................") == trace_core::provider::Flow::Stop {
                break;
            }
            emitted += 20;
        }
        Ok(Response {
            message: trace_core::Message::assistant("x".repeat(emitted)),
            usage: trace_core::event::Usage {
                input: 1000,
                output: (emitted / 4) as u64,
                cached_input: 0,
            },
            stop_reason: "length".into(),
        })
    }
}

#[test]
fn budget_aborts_mid_stream_with_a_recorded_event() {
    let dir = TempDir::new("budget");
    let mut cfg = test_config();
    cfg.limits.max_usd = 0.01;
    cfg.model.price_out_per_mtok = 1000.0;

    let log_path = dir.join("s.jsonl");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let mut session = trace_core::runtime::session::Session::start(
        cfg,
        trace_core::runtime::session::StartArgs {
            log_path: &log_path,
            session_id: "s-budget".into(),
            task: "spend everything".into(),
            workspace,
            agents_md: String::new(),
            harness_commit: "test".into(),
        },
    )
    .unwrap();

    let mut streamed = 0usize;
    let report = session
        .run(&Firehose, &mut |d| streamed += d.len())
        .unwrap();

    assert_eq!(report.outcome, Outcome::Aborted);
    assert!(
        streamed < 200_000,
        "the stream ran to completion; the guard did not fire mid-stream"
    );

    // Recorded, not silently swallowed.
    let events = trace_core::log::read(&log_path).unwrap().events;
    assert!(events
        .iter()
        .any(|e| matches!(&e.body, Body::Abort(a) if a.reason == AbortReason::Budget)));
    assert!(events
        .iter()
        .any(|e| matches!(&e.body, Body::SessionEnd(s) if s.outcome == Outcome::Aborted)));
}

#[test]
fn turn_cap_aborts_with_a_recorded_event() {
    let dir = TempDir::new("turncap");
    let mut cfg = test_config();
    cfg.limits.max_turns = 3;

    let turns: Vec<Response> = (0..10)
        .map(|i| ScriptedProvider::bash(&format!("c{i}"), "echo hi"))
        .collect();
    let (log_path, _) = scripted_session(&dir, &cfg, "never stop", turns);

    let events = trace_core::log::read(&log_path).unwrap().events;
    assert!(events
        .iter()
        .any(|e| matches!(&e.body, Body::Abort(a) if a.reason == AbortReason::TurnCap)));

    let responses = events
        .iter()
        .filter(|e| matches!(e.body, Body::ModelResponse(_)))
        .count();
    assert_eq!(responses, 3, "the cap was not enforced at the right turn");
}
