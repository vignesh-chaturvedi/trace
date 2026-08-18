//! Test plan (P0, carried forward): "Provider fixture tests: recorded
//! responses replayed offline, no network."
//!
//! The fixture is keyed by context hash, which is what makes it faithful
//! rather than approximate. If replay drifts by one byte, the lookup misses
//! and the test fails loudly instead of quietly answering the wrong question.

mod common;

use common::{test_config, TempDir};

use trace_core::context::build_context;
use trace_core::event::Body;
use trace_core::provider::{
    FixtureProvider, Provider, Recording, RecordingProvider, ScriptedProvider,
};
use trace_core::runtime::session::{Session, StartArgs};

fn script() -> Vec<trace_core::provider::Response> {
    vec![
        ScriptedProvider::bash("c1", "echo one > out.txt"),
        ScriptedProvider::bash("c2", "cat out.txt"),
        ScriptedProvider::say("verified: the file says one"),
    ]
}

fn run(
    dir: &TempDir,
    name: &str,
    provider: &dyn Provider,
) -> (std::path::PathBuf, Vec<trace_core::Event>) {
    let log_path = dir.join(&format!("{name}.jsonl"));
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let mut session = Session::start(
        test_config(),
        StartArgs {
            log_path: &log_path,
            session_id: format!("s-{name}"),
            task: "write and read a file".into(),
            workspace,
            agents_md: String::new(),
            harness_commit: "test".into(),
        },
    )
    .unwrap();

    session.run(provider, &mut |_| {}).unwrap();
    let events = trace_core::log::read(&log_path).unwrap().events;
    (log_path, events)
}

/// Record a run, then drive an identical session entirely from the recording.
#[test]
fn a_recorded_session_replays_offline() {
    let dir = TempDir::new("fixture");
    let fixture_path = dir.join("recorded.jsonl");

    let recorder = RecordingProvider::new(ScriptedProvider::new(script()), &fixture_path).unwrap();
    let (_, live_events) = run(&dir, "live", &recorder);
    drop(recorder);

    // Same session again, no scripted provider and no network in sight.
    let fixture = FixtureProvider::load(&fixture_path).unwrap();
    let (_, replayed_events) = run(&dir, "replayed", &fixture);

    let kinds =
        |evs: &[trace_core::Event]| -> Vec<&'static str> { evs.iter().map(|e| e.kind()).collect() };
    assert_eq!(kinds(&live_events), kinds(&replayed_events));

    // And every context hash matches turn for turn.
    let live_hashes: Vec<String> = live_events
        .iter()
        .filter_map(|e| match &e.body {
            Body::ModelRequest(r) => Some(r.context_hash.clone()),
            _ => None,
        })
        .collect();
    let replay_hashes: Vec<String> = replayed_events
        .iter()
        .filter_map(|e| match &e.body {
            Body::ModelRequest(r) => Some(r.context_hash.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(live_hashes, replay_hashes);
    assert!(live_hashes.len() >= 3);
}

/// A fixture that quietly answers a context it never saw would hide the exact
/// divergence it exists to detect.
#[test]
fn a_drifted_context_misses_rather_than_guessing() {
    let dir = TempDir::new("fixture-miss");
    let fixture_path = dir.join("recorded.jsonl");

    let recorder = RecordingProvider::new(ScriptedProvider::new(script()), &fixture_path).unwrap();
    run(&dir, "live", &recorder);
    drop(recorder);

    let fixture = FixtureProvider::load(&fixture_path).unwrap();
    let log_path = dir.join("drifted.jsonl");
    let workspace = dir.join("ws");

    let mut session = Session::start(
        test_config(),
        StartArgs {
            log_path: &log_path,
            session_id: "s-drift".into(),
            // A different task means a different first user message, so every
            // context downstream differs too.
            task: "something else entirely".into(),
            workspace,
            agents_md: String::new(),
            harness_commit: "test".into(),
        },
    )
    .unwrap();

    let err = session.run(&fixture, &mut |_| {}).unwrap_err();
    assert!(
        err.to_string().contains("no recorded response"),
        "expected a miss, got: {err}"
    );
}

#[test]
fn recordings_are_keyed_by_the_same_hash_the_log_records() {
    let dir = TempDir::new("fixture-key");
    let fixture_path = dir.join("recorded.jsonl");

    let recorder = RecordingProvider::new(ScriptedProvider::new(script()), &fixture_path).unwrap();
    let (_, events) = run(&dir, "live", &recorder);
    drop(recorder);

    let recorded: Vec<Recording> = std::fs::read_to_string(&fixture_path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    let cfg = test_config();
    for (i, ev) in events
        .iter()
        .filter(|e| matches!(e.body, Body::ModelRequest(_)))
        .enumerate()
    {
        let Body::ModelRequest(req) = &ev.body else {
            unreachable!()
        };
        assert_eq!(recorded[i].context_hash, req.context_hash);
        // ...and rebuilding offline lands on the same key.
        assert_eq!(
            build_context(&events, &cfg, ev.seq - 1).hash(),
            recorded[i].context_hash
        );
    }
}
