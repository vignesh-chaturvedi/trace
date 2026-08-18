//! Test plan: "Replay of every recorded fixture session produces byte-identical
//! contexts."
//!
//! Everything Phase 1 promises rests on this one property. If a rebuilt
//! context differs from what was sent by a single byte, then replay is an
//! approximation, offline ablations measure something other than what ran, and
//! P4 trains on trajectories that never happened.

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::context::build_context;
use trace_core::event::Body;
use trace_core::provider::ScriptedProvider;

fn script() -> Vec<trace_core::provider::Response> {
    vec![
        ScriptedProvider::bash("c1", "echo alpha"),
        ScriptedProvider::bash("c2", "printf 'beta\\ngamma\\n'"),
        ScriptedProvider::bash("c3", "ls"),
        ScriptedProvider::say("Done: verified all three."),
    ]
}

#[test]
fn replay_reproduces_every_recorded_context_hash() {
    let dir = TempDir::new("determinism");
    let cfg = test_config();
    let (_, events) = scripted_session(&dir, &cfg, "check a few things", script());

    let mut checked = 0;
    for ev in &events {
        let Body::ModelRequest(req) = &ev.body else {
            continue;
        };
        // The request was built from everything that existed before it.
        let rebuilt = build_context(&events, &cfg, ev.seq - 1);
        assert_eq!(
            rebuilt.hash(),
            req.context_hash,
            "context at seq {} did not reproduce",
            ev.seq
        );
        checked += 1;
    }

    assert!(
        checked >= 4,
        "expected several model requests, got {checked}"
    );
}

#[test]
fn building_twice_produces_identical_bytes() {
    let dir = TempDir::new("determinism-twice");
    let cfg = test_config();
    let (_, events) = scripted_session(&dir, &cfg, "check a few things", script());
    let head = events.last().unwrap().seq;

    let a = build_context(&events, &cfg, head);
    let b = build_context(&events, &cfg, head);

    assert_eq!(
        serde_json::to_vec(&a.messages).unwrap(),
        serde_json::to_vec(&b.messages).unwrap()
    );
    assert_eq!(a.tools_json, b.tools_json);
    assert_eq!(a.hash(), b.hash());
}

/// The strongest available check on purity.
///
/// A clock read anywhere inside the builder — directly, or by way of
/// `Event::ts_ms`, which is the tempting one — changes the output when the
/// timestamps change. Rewriting every timestamp in a real session and getting
/// the same bytes back rules that out for the whole module at once, which no
/// amount of reading the source can do.
#[test]
fn timestamps_do_not_reach_the_context() {
    let dir = TempDir::new("determinism-ts");
    let cfg = test_config();
    let (_, events) = scripted_session(&dir, &cfg, "check a few things", script());
    let head = events.last().unwrap().seq;

    let original = build_context(&events, &cfg, head);

    let shifted: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut e = e.clone();
            e.ts_ms = 1_700_000_000_000 + (i as u64) * 999_983;
            e
        })
        .collect();

    assert_eq!(
        original.hash(),
        build_context(&shifted, &cfg, head).hash(),
        "context changed when only timestamps changed"
    );
}

/// `upto` must mean what it says: a context built at turn N cannot contain
/// anything that had not happened yet.
#[test]
fn upto_is_a_hard_boundary() {
    let dir = TempDir::new("determinism-upto");
    let cfg = test_config();
    let (_, events) = scripted_session(&dir, &cfg, "check a few things", script());

    let early = build_context(&events, &cfg, 4);
    let late = build_context(&events, &cfg, events.last().unwrap().seq);

    assert!(early.messages.len() < late.messages.len());

    // Truncating the log entirely must give the same answer as bounding it.
    let truncated: Vec<_> = events.iter().filter(|e| e.seq <= 4).cloned().collect();
    assert_eq!(early.hash(), build_context(&truncated, &cfg, 4).hash());
}

/// Config is an input to the builder, so changing it must change the output —
/// otherwise the replay-time ablations Phase 1 exists to enable are inert.
#[test]
fn truncation_limit_is_a_replay_time_ablation() {
    let dir = TempDir::new("determinism-ablate");
    let cfg = test_config();

    let big = "x".repeat(50_000);
    let turns = vec![
        ScriptedProvider::bash("c1", &format!("printf '{}'", &big[..2000])),
        ScriptedProvider::say("done"),
    ];
    let (_, events) = scripted_session(&dir, &cfg, "make output", turns);
    let head = events.last().unwrap().seq;

    let mut tight = cfg.clone();
    tight.context.truncate_limit = 200;

    let wide = build_context(&events, &cfg, head);
    let narrow = build_context(&events, &tight, head);

    assert_ne!(
        wide.hash(),
        narrow.hash(),
        "truncate_limit had no effect; the log must store full output and truncate at build time"
    );
    assert!(narrow.est_tokens() < wide.est_tokens());
}
