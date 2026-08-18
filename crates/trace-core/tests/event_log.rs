//! The ledger's own invariants: gapless numbering, one event per line, fsync on
//! boundaries, and a rebuildable index.

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::event::{Body, Observation, ObservationSource, FSYNC_TYPES};
use trace_core::log::{self, EventLog};
use trace_core::provider::ScriptedProvider;

#[test]
fn sequence_is_monotonic_and_gapless() {
    let dir = TempDir::new("seq");
    let mut log = EventLog::create(dir.join("s.jsonl"), "s1").unwrap();

    for i in 0..50 {
        let ev = log
            .append(Body::Observation(Observation {
                source: ObservationSource::System,
                text: format!("event {i}"),
            }))
            .unwrap();
        assert_eq!(ev.seq, i + 1);
    }

    let events = log::read(dir.join("s.jsonl")).unwrap().events;
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(ev.seq, i as u64 + 1);
    }
}

/// One event per line is what makes the log greppable, tailable, and
/// streamable into a training pipeline. Embedded newlines would end that.
#[test]
fn one_event_is_always_one_line() {
    let dir = TempDir::new("lines");
    let mut log = EventLog::create(dir.join("s.jsonl"), "s1").unwrap();

    log.append(Body::Observation(Observation {
        source: ObservationSource::System,
        text: "line one\nline two\r\nand a \" quote and a \\ backslash".into(),
    }))
    .unwrap();

    let raw = std::fs::read_to_string(dir.join("s.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 1);

    let events = log::read(dir.join("s.jsonl")).unwrap().events;
    let Body::Observation(o) = &events[0].body else {
        panic!()
    };
    assert!(o.text.contains("line one\nline two"));
}

#[test]
fn resume_continues_the_sequence() {
    let dir = TempDir::new("resume-seq");
    let path = dir.join("s.jsonl");

    let mut log = EventLog::create(&path, "s1").unwrap();
    for _ in 0..3 {
        log.append(Body::Observation(Observation {
            source: ObservationSource::System,
            text: "x".into(),
        }))
        .unwrap();
    }
    drop(log);

    let (mut log, events) = EventLog::resume(&path).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(log.next_seq(), 4);

    let ev = log
        .append(Body::Observation(Observation {
            source: ObservationSource::System,
            text: "after resume".into(),
        }))
        .unwrap();
    assert_eq!(ev.seq, 4);
    assert_eq!(ev.session, "s1", "session identity must survive a resume");
}

#[test]
fn creating_over_an_existing_log_is_refused() {
    let dir = TempDir::new("nooverwrite");
    let path = dir.join("s.jsonl");
    let _log = EventLog::create(&path, "s1").unwrap();
    assert!(
        EventLog::create(&path, "s2").is_err(),
        "silently appending to another session's log is worse than an error"
    );
}

/// The boundary events are the ones a crash must not lose. `tool_call` in
/// particular: the whole recovery story rests on it reaching disk before the
/// command runs.
#[test]
fn fsync_covers_the_boundary_events() {
    for kind in ["session_start", "model_response", "tool_call", "checkpoint"] {
        assert!(FSYNC_TYPES.contains(&kind), "{kind} must be fsynced");
    }
}

#[test]
fn body_kind_matches_the_serialized_tag() {
    let dir = TempDir::new("tags");
    let cfg = test_config();
    let turns = vec![
        ScriptedProvider::bash("c1", "echo hi"),
        ScriptedProvider::say("done"),
    ];
    let (path, events) = scripted_session(&dir, &cfg, "t", turns);

    let raw = std::fs::read_to_string(&path).unwrap();
    for (line, ev) in raw.lines().zip(&events) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["type"].as_str().unwrap(), ev.kind());
        assert_eq!(v["seq"].as_u64().unwrap(), ev.seq);
    }
}

#[test]
fn the_index_is_rebuildable_from_the_logs() {
    let dir = TempDir::new("index");
    let cfg = test_config();
    let turns = vec![
        ScriptedProvider::bash("c1", "echo hi"),
        ScriptedProvider::say("all done"),
    ];
    let (path, _) = scripted_session(&dir, &cfg, "index me", turns);

    let summary = log::index::summarize(&path).unwrap();
    assert_eq!(summary.task, "index me");
    assert_eq!(summary.harness_commit, "testcommit");
    assert_eq!(summary.tool_calls, 1);
    assert!(summary.turns >= 2);
    assert!(summary.usage.input > 0);

    // Cache hit rate is the number that tells you the prefix-stable layout is
    // working, so it has to survive into the index.
    assert!(summary.cache_hit_rate > 0.0);
    assert!(
        (summary.cache_hit_rate - 0.45).abs() < 0.01,
        "{}",
        summary.cache_hit_rate
    );

    let rows = log::rebuild_index(dir.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(dir.join("index.jsonl").exists());
}
