//! Test plan: "Kill -9 harness at 20 random points; every restart resumes
//! cleanly" and "Torn-write recovery: truncate a log mid-line, reader repairs
//! it."
//!
//! The crash sweep here is stronger than the plan asks for. Rather than
//! killing the process at 20 random moments and hoping the interesting states
//! come up, it truncates the log at **every byte offset** — which enumerates
//! the complete space of states a `kill -9` can leave behind, deterministically
//! and without a timing race.

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::context::build_context;
use trace_core::event::Body;
use trace_core::log;
use trace_core::provider::ScriptedProvider;
use trace_core::runtime::recovery::find_orphans;
use trace_core::runtime::session::Session;

fn a_session(dir: &TempDir) -> (std::path::PathBuf, Vec<u8>) {
    let cfg = test_config();
    let turns = vec![
        ScriptedProvider::bash("c1", "echo first"),
        ScriptedProvider::bash("c2", "echo second"),
        ScriptedProvider::bash("c3", "echo third"),
        ScriptedProvider::say("finished"),
    ];
    let (path, _) = scripted_session(dir, &cfg, "do the thing", turns);
    let bytes = std::fs::read(&path).unwrap();
    (path, bytes)
}

#[test]
fn torn_final_line_is_repaired() {
    let dir = TempDir::new("torn");
    let (path, bytes) = a_session(&dir);
    let full = log::read(&path).unwrap().events.len();

    // Cut halfway through the last line, as an interrupted write would. The
    // file ends with a terminator, so the last line starts after the newline
    // before it.
    let end = bytes.len() - 1;
    let last_line_start = bytes[..end].iter().rposition(|&b| b == b'\n').unwrap() + 1;
    let cut = last_line_start + (end - last_line_start) / 2;
    std::fs::write(&path, &bytes[..cut]).unwrap();

    let outcome = log::read_and_repair(&path).unwrap();

    assert!(outcome.repair.is_some(), "damage was not detected");
    assert!(outcome.warning().unwrap().contains("torn write"));
    assert_eq!(outcome.events.len(), full - 1);

    // Repair is idempotent, and the repaired file is clean on re-read.
    let again = log::read(&path).unwrap();
    assert!(again.repair.is_none());
    assert_eq!(again.events.len(), full - 1);
}

/// A write that lost only its terminator has not lost the event. Truncating
/// the line away would discard a perfectly good record.
#[test]
fn a_lost_terminator_does_not_cost_an_event() {
    let dir = TempDir::new("noterm");
    let (path, bytes) = a_session(&dir);
    let full = log::read(&path).unwrap().events.len();

    std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();

    let outcome = log::read_and_repair(&path).unwrap();
    assert!(outcome.repair.is_some());
    assert_eq!(
        outcome.events.len(),
        full,
        "a salvageable event was thrown away"
    );

    // The file is well-formed again, and the first event is intact.
    let again = log::read(&path).unwrap();
    assert!(again.repair.is_none());
    assert_eq!(again.events.len(), full);
    assert_eq!(again.events[0].seq, 1);
    assert!(again.events[0].as_session_start().is_some());
}

#[test]
fn corruption_in_the_middle_is_an_error_not_a_repair() {
    let dir = TempDir::new("midcorrupt");
    let (path, bytes) = a_session(&dir);

    let text = String::from_utf8(bytes).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[2] = "{not json at all".into();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    // Truncating here would silently discard every valid event after the
    // damage, so this must fail loudly instead.
    assert!(log::read(&path).is_err());
}

/// The complete crash-state sweep.
#[test]
fn resumes_cleanly_from_every_possible_kill_point() {
    let dir = TempDir::new("killsweep");
    let (source, bytes) = a_session(&dir);
    let cfg = test_config();
    let workspace = dir.join("ws");

    let mut resumable = 0usize;
    let mut with_orphans = 0usize;

    for cut in 1..bytes.len() {
        let path = dir.join("crashed.jsonl");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, &bytes[..cut]).unwrap();

        let outcome = log::read_and_repair(&path).expect("repair must never fail");
        if outcome.events.is_empty() {
            continue; // crashed before the first event was durable
        }

        // The ledger must always be internally consistent after repair.
        for pair in outcome.events.windows(2) {
            assert_eq!(
                pair[1].seq,
                pair[0].seq + 1,
                "gap after repair at cut {cut}"
            );
        }

        let mut session =
            Session::resume(cfg.clone(), &path, workspace.clone()).expect("resume must succeed");
        resumable += 1;

        let before = session.events().len();
        let orphans = find_orphans(session.events());
        if !orphans.is_empty() {
            with_orphans += 1;
        }

        // Resuming must record a recovery event per orphan and must not
        // re-issue the call.
        let ids: Vec<String> = orphans.iter().map(|o| o.call_id.clone()).collect();
        let provider = ScriptedProvider::new(vec![ScriptedProvider::say("ok")]);
        let _ = session.run(&provider, &mut |_| {});

        let appended = &session.events()[before..];
        for id in &ids {
            assert!(
                appended
                    .iter()
                    .any(|e| matches!(&e.body, Body::Recovery(r) if &r.orphan_call_id == id)),
                "no recovery event for orphaned call {id} at cut {cut}"
            );
            assert!(
                !appended
                    .iter()
                    .any(|e| e.as_tool_call().is_some_and(|c| &c.id == id)),
                "orphaned call {id} was re-executed at cut {cut}"
            );
        }

        // And the resumed context must be buildable — a provider would reject
        // an assistant turn advertising a call that nothing answers.
        let head = session.events().last().unwrap().seq;
        let ctx = build_context(session.events(), &cfg, head);
        assert_no_dangling_tool_calls(&ctx.messages, cut);
    }

    assert!(
        resumable > 20,
        "expected many resumable cut points, got {resumable}"
    );
    assert!(
        with_orphans > 0,
        "the sweep never produced an interrupted tool call, so it proved nothing"
    );
    let _ = source;
}

/// Every advertised tool call has a result, in the same block.
fn assert_no_dangling_tool_calls(messages: &[trace_core::Message], cut: usize) {
    use trace_core::Role;

    for (i, m) in messages.iter().enumerate() {
        if m.role != Role::Assistant || m.tool_calls.is_empty() {
            continue;
        }
        let answers: Vec<&str> = messages[i + 1..]
            .iter()
            .take_while(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();

        for call in &m.tool_calls {
            assert!(
                answers.contains(&call.id.as_str()),
                "call {} has no result at cut {cut}; a provider would reject this context",
                call.id
            );
        }
    }
}

/// The ordering rule, stated as a test: a result never precedes its call.
#[test]
fn tool_calls_are_durable_before_their_results() {
    let dir = TempDir::new("ordering");
    let (path, _) = a_session(&dir);
    let events = log::read(&path).unwrap().events;

    for (i, ev) in events.iter().enumerate() {
        let Some(result) = ev.as_tool_result() else {
            continue;
        };
        assert!(
            events[..i]
                .iter()
                .any(|e| e.as_tool_call().is_some_and(|c| c.id == result.call_id)),
            "a tool_result appeared before its tool_call"
        );
    }
}

#[test]
fn resume_does_not_restart_the_budget() {
    let dir = TempDir::new("budget-resume");
    let (path, _) = a_session(&dir);
    let cfg = test_config();

    let spent: f64 = log::read(&path)
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match &e.body {
            Body::ModelResponse(r) => Some(cfg.price(&r.usage)),
            _ => None,
        })
        .sum();
    assert!(spent > 0.0);

    let mut session = Session::resume(cfg.clone(), &path, dir.join("ws")).unwrap();
    let provider = ScriptedProvider::new(vec![ScriptedProvider::say("done")]);
    let report = session.run(&provider, &mut |_| {}).unwrap();

    assert!(
        report.usd > spent,
        "a resumed session must carry its prior spend, not reset it"
    );
    assert!(report.turns > 4, "turn count must carry across resume too");
}
