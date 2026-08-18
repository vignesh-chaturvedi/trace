//! Test plan: "Compaction round-trip: expand a compacted log -> original event
//! set."
//!
//! The round trip is what makes compaction a strategy rather than a loss.
//! Because a compaction event only *declares* a replacement, the original
//! events are still there — so compaction strategies can be compared offline, and P4 can
//! train on the full trajectory even though inference ran compacted.

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::context::build_context;
use trace_core::event::{Body, Compaction, Event};
use trace_core::provider::ScriptedProvider;
use trace_core::runtime::compaction::{expand, provenance, range_for, should_compact, verify};

fn session_events() -> (TempDir, Vec<Event>, trace_core::config::Config) {
    let dir = TempDir::new("compaction");
    let cfg = test_config();
    let turns = vec![
        ScriptedProvider::bash("c1", "echo one"),
        ScriptedProvider::bash("c2", "echo two"),
        ScriptedProvider::bash("c3", "echo three"),
        ScriptedProvider::bash("c4", "echo four"),
        ScriptedProvider::say("all done"),
    ];
    let (_, events) = scripted_session(&dir, &cfg, "count things", turns);
    (dir, events, cfg)
}

fn compact(events: &[Event], from: u64, to: u64, summary: &str) -> Event {
    Event {
        seq: events.last().unwrap().seq + 1,
        ts_ms: 42,
        session: events[0].session.clone(),
        body: Body::Compaction(Compaction {
            replaces_from: from,
            replaces_to: to,
            summary: summary.into(),
            provenance: provenance(events, from, to),
        }),
    }
}

#[test]
fn expansion_restores_the_original_event_set() {
    let (_dir, original, _cfg) = session_events();

    let mut compacted = original.clone();
    compacted.push(compact(&original, 2, 6, "did one and two"));

    let expanded = expand(&compacted);

    assert_eq!(
        expanded, original,
        "expanding a compacted log must yield exactly the original events"
    );
}

#[test]
fn provenance_detects_tampering() {
    let (_dir, events, _cfg) = session_events();
    let ev = compact(&events, 2, 6, "summary");
    let Body::Compaction(c) = &ev.body else {
        unreachable!()
    };

    assert!(verify(&events, c), "honest provenance should verify");

    let mut tampered = events.clone();
    let victim = tampered
        .iter_mut()
        .find(|e| e.seq >= 2 && e.seq <= 6 && matches!(e.body, Body::ToolResult(_)))
        .expect("the replaced range should contain a tool result to tamper with");
    let Body::ToolResult(r) = &mut victim.body else {
        unreachable!()
    };
    r.output.push_str(" (edited)");

    assert!(
        !verify(&tampered, c),
        "provenance must not verify against altered events"
    );
}

/// The point of compacting: the rendered context gets smaller, and what
/// survives is what the model itself chose to carry forward.
#[test]
fn compaction_shrinks_the_context_and_keeps_the_summary() {
    let (_dir, events, cfg) = session_events();
    let head = events.last().unwrap().seq;
    let before = build_context(&events, &cfg, head);

    let mut compacted = events.clone();
    compacted.push(compact(
        &events,
        2,
        6,
        "NOTES: ran one and two, both passed",
    ));
    let after = build_context(&compacted, &cfg, compacted.last().unwrap().seq);

    assert!(
        after.messages.len() < before.messages.len(),
        "compaction did not reduce the context"
    );
    assert!(after
        .messages
        .iter()
        .any(|m| m.content.contains("NOTES: ran one and two")));
    // The oldest turn's output is gone from the rendered context...
    assert!(!after.messages.iter().any(|m| m.content.contains("one\n")));
    // ...but never from the log.
    assert_eq!(expand(&compacted).len(), events.len());
}

/// Later compactions swallow earlier ones. Only the newest summary should
/// survive into the context; a stale one leaking through underneath it would
/// contradict it.
#[test]
fn chained_compactions_keep_only_the_newest_summary() {
    let (_dir, events, cfg) = session_events();

    let mut compacted = events.clone();
    compacted.push(compact(&events, 2, 5, "FIRST SUMMARY"));
    let wide = compact(
        &compacted,
        2,
        compacted.last().unwrap().seq,
        "SECOND SUMMARY",
    );
    compacted.push(wide);

    let ctx = build_context(&compacted, &cfg, compacted.last().unwrap().seq);
    let text: String = ctx.messages.iter().map(|m| m.content.clone()).collect();

    assert!(text.contains("SECOND SUMMARY"));
    assert!(!text.contains("FIRST SUMMARY"));
}

#[test]
fn keep_recent_protects_the_working_set() {
    let (_dir, events, mut cfg) = session_events();
    let head = events.last().unwrap().seq;

    cfg.context.keep_recent = 6;
    let (_from, to) = range_for(&events, &cfg, head).expect("a range should be available");
    assert!(
        to <= head - 6,
        "compaction range reached into the protected recent turns"
    );

    // Nothing to compact when everything is recent.
    cfg.context.keep_recent = head + 10;
    assert!(range_for(&events, &cfg, head).is_none());
}

#[test]
fn session_start_is_never_compacted() {
    let (_dir, events, cfg) = session_events();
    let head = events.last().unwrap().seq;
    let (from, _to) = range_for(&events, &cfg, head).unwrap();
    assert!(from >= 2, "seq 1 is the stable region and must survive");
}

#[test]
fn threshold_is_a_fraction_of_the_model_limit() {
    let mut cfg = test_config();
    cfg.model.context_limit = 1000;
    cfg.context.compact_at = 0.75;

    assert!(!should_compact(749, &cfg));
    assert!(should_compact(750, &cfg));
    assert!(should_compact(999, &cfg));
}
