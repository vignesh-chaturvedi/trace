//! Test plan: "Layout lint fails on a deliberately-planted timestamp in the
//! system prompt" and "Tool schema serialization is byte-stable across 100
//! randomized-order builds."

mod common;

use std::collections::BTreeMap;

use common::test_config;

use trace_core::context::lint::{self, Severity};
use trace_core::event::SessionStart;
use trace_core::message::JsonValue;
use trace_core::tools::schema::{registry, schemas_json};

fn start() -> SessionStart {
    SessionStart {
        cwd: "/work/repo".into(),
        task: "fix the failing test".into(),
        ..Default::default()
    }
}

#[test]
fn default_layout_is_clean() {
    let findings = lint::lint(&test_config(), &start());
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "default layout should lint clean: {errors:?}"
    );
}

#[test]
fn planted_timestamp_fails_the_lint() {
    let mut cfg = test_config();
    cfg.prompt
        .system
        .push_str("\n\nSession started at 2026-08-18T09:14:22Z.");

    let findings = lint::lint(&cfg, &start());

    assert!(
        lint::has_errors(&findings),
        "lint missed a planted timestamp"
    );
    assert!(
        findings.iter().any(|f| f.rule == "timestamp-in-prefix"),
        "wrong rule fired: {findings:?}"
    );
}

#[test]
fn planted_turn_counter_fails_the_lint() {
    let mut cfg = test_config();
    cfg.prompt
        .system
        .push_str("\n\nYou have used 4 turns of your 60 turn budget.");

    let findings = lint::lint(&cfg, &start());

    assert!(lint::has_errors(&findings));
    assert!(findings.iter().any(|f| f.rule == "counter-in-prefix"));
}

#[test]
fn planted_context_percentage_fails_the_lint() {
    let mut cfg = test_config();
    cfg.prompt.system.push_str("\n\nContext remaining: 62%.");

    assert!(lint::has_errors(&lint::lint(&cfg, &start())));
}

/// A temp-dir workspace does not break caching *within* a session, so it is
/// not an error — but no two sessions will ever share a cache entry, which is
/// worth knowing.
#[test]
fn temp_workspace_warns_but_does_not_fail() {
    let cfg = test_config();
    let start = SessionStart {
        cwd: "/tmp/task-xyz".into(),
        ..start()
    };

    let findings = lint::lint(&cfg, &start);
    assert!(!lint::has_errors(&findings));
    assert!(findings.iter().any(|f| f.rule == "temp-path-in-prefix"));
}

/// The manual's "silent killer": same schemas, different bytes, every turn.
///
/// The defence here is structural — `JsonValue` has no unordered map variant —
/// so this test builds the same schema from a hundred different key insertion
/// orders and asserts the serialization never moves.
#[test]
fn tool_schemas_are_byte_stable_across_randomized_builds() {
    let canonical = schemas_json(&registry());

    // A tiny LCG: reproducible shuffling without a dependency.
    let mut seed = 0x2545F491_4F6CDD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let keys = [
        ("type", JsonValue::Str("object".into())),
        ("description", JsonValue::Str("A tool.".into())),
        (
            "required",
            JsonValue::Array(vec![JsonValue::Str("cmd".into())]),
        ),
        ("properties", JsonValue::Object(BTreeMap::new())),
        ("additionalProperties", JsonValue::Bool(false)),
    ];

    let mut previous: Option<String> = None;

    for _ in 0..100 {
        let mut order: Vec<usize> = (0..keys.len()).collect();
        for i in (1..order.len()).rev() {
            let j = (next() as usize) % (i + 1);
            order.swap(i, j);
        }

        let mut map: BTreeMap<String, JsonValue> = BTreeMap::new();
        for &i in &order {
            map.insert(keys[i].0.to_string(), keys[i].1.clone());
        }

        let serialized = serde_json::to_string(&JsonValue::Object(map)).unwrap();
        if let Some(prev) = &previous {
            assert_eq!(
                prev, &serialized,
                "schema bytes changed with key insertion order; the cache would never hit"
            );
        }
        previous = Some(serialized);
    }

    // And the real registry is stable across rebuilds too.
    for _ in 0..100 {
        assert_eq!(canonical, schemas_json(&registry()));
    }
}

/// Registration order must not leak into the wire bytes either.
#[test]
fn registry_is_sorted_by_name() {
    let tools = registry();
    let mut names: Vec<_> = tools.iter().map(|t| t.name.clone()).collect();
    let unsorted = names.clone();
    names.sort();
    assert_eq!(names, unsorted);
}

/// The exit criterion, as far as the harness can prove it alone.
///
/// "Cache hit > 90% on a 50-turn session" has two halves. The provider's half
/// needs a live key and a real cache. The harness's half is this: across fifty
/// consecutive turns, every context must share the entire stable region as a
/// literal byte prefix with the one before it. If that holds and the hit rate
/// is still low, the problem is upstream of this code.
#[test]
fn a_fifty_turn_session_never_moves_its_prefix() {
    use trace_core::context::build_context;
    use trace_core::event::Body;
    use trace_core::provider::ScriptedProvider;

    let dir = common::TempDir::new("prefix-50");
    let cfg = test_config();

    let mut turns: Vec<_> = (0..50)
        .map(|i| ScriptedProvider::bash(&format!("c{i}"), &format!("echo turn {i}")))
        .collect();
    turns.push(ScriptedProvider::say("done"));

    let (_, events) = common::scripted_session(&dir, &cfg, "a long session", turns);

    let request_seqs: Vec<u64> = events
        .iter()
        .filter(|e| matches!(e.body, Body::ModelRequest(_)))
        .map(|e| e.seq)
        .collect();
    assert!(request_seqs.len() >= 50, "got {} turns", request_seqs.len());

    let render = |upto: u64| -> (Vec<u8>, usize, String) {
        let ctx = build_context(&events, &cfg, upto);
        let mut bytes = ctx.tools_json.clone().into_bytes();
        bytes.extend_from_slice(ctx.messages[0].content.as_bytes());
        (bytes, ctx.stable_prefix_bytes, ctx.tools_json)
    };

    for pair in request_seqs.windows(2) {
        let (a, stable, tools_a) = render(pair[0] - 1);
        let (b, _, tools_b) = render(pair[1] - 1);

        assert_eq!(tools_a, tools_b, "the tool block moved between turns");

        let shared = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
        assert!(
            shared >= stable,
            "turns {} and {} share only {shared} bytes of a {stable}-byte stable region",
            pair[0],
            pair[1]
        );
    }
}

/// Growth must be append-only across the turns that are actually sent.
///
/// The qualifier matters. Between a `tool_call` and its result the builder
/// seals the gap with an "unknown" placeholder, so a context built at that
/// instant is *not* an extension of the previous one. Nothing ever sends such
/// a context — requests are only built at turn boundaries — and the placeholder
/// exists precisely so that a session interrupted there can still resume. The
/// property to hold is therefore about request boundaries, not arbitrary
/// sequence numbers.
#[test]
fn contexts_grow_by_appending_between_requests() {
    use trace_core::context::build_context;
    use trace_core::event::Body;
    use trace_core::provider::ScriptedProvider;

    let dir = common::TempDir::new("append-only");
    let cfg = test_config();

    let mut turns: Vec<_> = (0..12)
        .map(|i| ScriptedProvider::bash(&format!("c{i}"), &format!("echo {i}")))
        .collect();
    turns.push(ScriptedProvider::say("done"));

    let (_, events) = common::scripted_session(&dir, &cfg, "grow", turns);

    let request_seqs: Vec<u64> = events
        .iter()
        .filter(|e| matches!(e.body, Body::ModelRequest(_)))
        .map(|e| e.seq)
        .collect();

    for pair in request_seqs.windows(2) {
        let earlier = build_context(&events, &cfg, pair[0] - 1);
        let later = build_context(&events, &cfg, pair[1] - 1);

        assert!(later.messages.len() > earlier.messages.len());
        for (i, msg) in earlier.messages.iter().enumerate() {
            assert_eq!(
                Some(msg),
                later.messages.get(i),
                "message {i} changed between requests at seq {} and {}",
                pair[0],
                pair[1]
            );
        }
    }
}

/// The placeholder is what makes an interrupted turn resumable at all: a
/// provider rejects an assistant message advertising a call that nothing
/// answers.
#[test]
fn an_unanswered_call_is_sealed_rather_than_left_dangling() {
    use trace_core::context::build_context;
    use trace_core::event::Body;
    use trace_core::provider::ScriptedProvider;
    use trace_core::Role;

    let dir = common::TempDir::new("sealed");
    let cfg = test_config();
    let turns = vec![
        ScriptedProvider::bash("c0", "echo 0"),
        ScriptedProvider::say("done"),
    ];
    let (_, events) = common::scripted_session(&dir, &cfg, "seal", turns);

    let call_seq = events
        .iter()
        .find(|e| matches!(e.body, Body::ToolCall(_)))
        .map(|e| e.seq)
        .expect("a tool call");

    // Cut the log the instant after the call became durable.
    let ctx = build_context(&events, &cfg, call_seq);
    let last = ctx.messages.last().unwrap();

    assert_eq!(last.role, Role::Tool);
    assert_eq!(last.tool_call_id.as_deref(), Some("c0"));
    assert!(last.content.contains("interrupted"));
}
