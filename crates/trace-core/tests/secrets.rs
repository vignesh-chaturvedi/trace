//! The key must never reach an artifact.
//!
//! Trajectories are meant to be published, committed, and fed into a training
//! pipeline. A harness that writes a secret into the artifact it exists to
//! share has one very bad day ahead of it, so this is pinned rather than
//! assumed.

mod common;

use common::{scripted_session, test_config, TempDir};

use trace_core::provider::ScriptedProvider;
use trace_core::secrets::load_dotenv;

/// Assembled at run time, never written as a literal: a credential-shaped
/// constant in source is blocked by push protection, and rightly so.
fn shaped(prefix: &str, body: &str) -> String {
    format!("{prefix}{body}")
}

fn fake_key() -> String {
    shaped("sk-", "test-DO-NOT-LEAK-2f8a91c4")
}

#[test]
fn a_key_never_appears_in_the_log_or_the_context() {
    let dir = TempDir::new("secrets");
    let mut cfg = test_config();
    cfg.model.api_key_env = "TRACE_TEST_KEY".into();

    std::env::set_var("TRACE_TEST_KEY", fake_key());

    let turns = vec![
        ScriptedProvider::bash("c1", "echo working"),
        ScriptedProvider::say("done"),
    ];
    let (log_path, events) = scripted_session(&dir, &cfg, "do something", turns);

    // Nothing on disk.
    let raw = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !raw.contains(fake_key().as_str()),
        "the key reached the event log"
    );

    // Nothing in the rendered context either.
    let ctx = trace_core::context::build_context(&events, &cfg, events.last().unwrap().seq);
    let rendered = serde_json::to_string(&ctx.messages).unwrap();
    assert!(
        !rendered.contains(fake_key().as_str()),
        "the key reached the context"
    );

    // Config records the variable's name, never its value.
    let serialized = serde_json::to_string(&cfg).unwrap();
    assert!(serialized.contains("TRACE_TEST_KEY"));
    assert!(!serialized.contains(fake_key().as_str()));

    std::env::remove_var("TRACE_TEST_KEY");
}

#[test]
fn dotenv_sets_variables_and_ignores_comments() {
    let dir = TempDir::new("dotenv");
    let path = dir.join(".env");
    std::fs::write(
        &path,
        "# a comment\n\n\
         TRACE_A=plain\n\
         TRACE_B=\"double quoted\"\n\
         TRACE_C='single quoted'\n\
         export TRACE_D=exported\n",
    )
    .unwrap();

    assert_eq!(load_dotenv(&path).unwrap(), 4);
    assert_eq!(std::env::var("TRACE_A").unwrap(), "plain");
    assert_eq!(std::env::var("TRACE_B").unwrap(), "double quoted");
    assert_eq!(std::env::var("TRACE_C").unwrap(), "single quoted");
    assert_eq!(std::env::var("TRACE_D").unwrap(), "exported");

    for k in ["TRACE_A", "TRACE_B", "TRACE_C", "TRACE_D"] {
        std::env::remove_var(k);
    }
}

/// The surprising direction of this precedence is how the wrong account gets
/// billed: a stale `.env` on a laptop must not override what CI injected.
#[test]
fn the_real_environment_beats_the_file() {
    let dir = TempDir::new("dotenv-precedence");
    let path = dir.join(".env");
    std::fs::write(&path, "TRACE_PRECEDENCE=from-file\n").unwrap();

    std::env::set_var("TRACE_PRECEDENCE", "from-environment");
    assert_eq!(load_dotenv(&path).unwrap(), 0);
    assert_eq!(
        std::env::var("TRACE_PRECEDENCE").unwrap(),
        "from-environment"
    );

    std::env::remove_var("TRACE_PRECEDENCE");
}

#[test]
fn a_missing_file_is_not_an_error() {
    let dir = TempDir::new("dotenv-missing");
    assert_eq!(load_dotenv(dir.join("nope.env")).unwrap(), 0);
}

#[test]
fn a_malformed_line_is_reported_with_its_number() {
    let dir = TempDir::new("dotenv-bad");
    let path = dir.join(".env");
    std::fs::write(&path, "GOOD=1\nthis line has no equals sign\n").unwrap();

    let err = load_dotenv(&path).unwrap_err().to_string();
    assert!(err.contains(":2:"), "{err}");
    std::env::remove_var("GOOD");
}

// ─────────────────────────────────────────────── redaction

use std::sync::Arc;

use trace_core::event::{Observation, ObservationSource};
use trace_core::secrets::{scrubbed_env, Redactor, ENV_ALLOWLIST, MIN_SECRET_LEN};

fn token() -> String {
    shaped("sk-", "live-9f2a7c41b8e35d60")
}

#[test]
fn redaction_replaces_the_value_wherever_it_appears() {
    let key = token();
    let mut r = Redactor::new();
    r.register("OPENAI_API_KEY", key.clone());

    let text = format!("curl -H 'Authorization: Bearer {key}' https://api.example/v1");
    let out = r.redact(&text);

    assert!(!out.contains(key.as_str()));
    assert!(out.contains("[redacted:OPENAI_API_KEY]"));
    assert!(
        out.contains("https://api.example/v1"),
        "redaction ate the context"
    );
}

/// The property the whole thing exists for: the secret must not be on disk,
/// because the log is what gets committed, published, and trained on.
#[test]
fn a_secret_never_reaches_the_log_file() {
    let dir = TempDir::new("redact-log");
    let path = dir.join("s.jsonl");

    let key = token();
    let mut r = Redactor::new();
    r.register("TOKEN", key.clone());

    let mut log = trace_core::log::EventLog::create(&path, "s1")
        .unwrap()
        .with_redactor(Arc::new(r.clone()));

    log.append(trace_core::Body::Observation(Observation {
        source: ObservationSource::System,
        text: format!("the tool printed {key} to stdout"),
    }))
    .unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains(key.as_str()),
        "the secret is sitting in the log file"
    );
    assert!(raw.contains("[redacted:TOKEN]"));
    assert!(r.leaks(&raw).is_none());
}

/// Redaction must not corrupt the ledger it is protecting.
#[test]
fn a_redacted_log_is_still_valid_jsonl() {
    let dir = TempDir::new("redact-valid");
    let path = dir.join("s.jsonl");

    // A secret containing characters JSON has to escape.
    let awkward = r#"tok"en\with/slashes-and-quotes"#;
    let mut r = Redactor::new();
    r.register("AWKWARD", awkward);

    let mut log = trace_core::log::EventLog::create(&path, "s1")
        .unwrap()
        .with_redactor(Arc::new(r));

    for i in 0..3 {
        log.append(trace_core::Body::Observation(Observation {
            source: ObservationSource::System,
            text: format!("line {i} contains {awkward} inline"),
        }))
        .unwrap();
    }

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains(awkward), "the escaped form slipped through");

    // Every line still parses, and the reader still accepts the file.
    for line in raw.lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("line is not valid JSON");
    }
    let events = trace_core::log::read(&path).unwrap().events;
    assert_eq!(events.len(), 3);
}

/// If one secret contains another, redacting the short one first would leave
/// the long one's tail exposed.
#[test]
fn overlapping_secrets_are_fully_removed() {
    let short = "abcdefghij";
    let long = "abcdefghijKLMNOPQRST";

    let mut r = Redactor::new();
    r.register("SHORT", short);
    r.register("LONG", long);

    let out = r.redact(&format!("value={long} other={short}"));
    assert!(
        !out.contains("KLMNOPQRST"),
        "long secret partially survived: {out}"
    );
    assert!(r.leaks(&out).is_none(), "{out}");
}

/// A short "secret" matches too much ordinary text; redacting it would turn
/// every line into confetti and get the redactor switched off.
#[test]
fn values_below_the_minimum_length_are_refused() {
    let mut r = Redactor::new();
    assert!(!r.register("SHORT", "abc"));
    assert!(!r.register("EMPTY", ""));
    assert!(r.register("OK", "a".repeat(MIN_SECRET_LEN)));
    assert_eq!(r.len(), 1);
}

#[test]
fn a_redactor_with_nothing_registered_changes_nothing() {
    let r = Redactor::new();
    assert!(r.is_empty());
    assert_eq!(r.redact("ordinary text"), "ordinary text");
}

// ────────────────────────────────────── scrubbed env

/// Escape 05: dump the environment and grep for key-shaped strings.
#[test]
fn the_scrubbed_environment_carries_no_credentials() {
    std::env::set_var("TRACE_TEST_SECRET_TOKEN", token());
    std::env::set_var("SOME_VENDOR_CREDENTIAL", "another-secret-value");

    let env = scrubbed_env();

    assert!(!env.contains_key("TRACE_TEST_SECRET_TOKEN"));
    assert!(
        !env.contains_key("SOME_VENDOR_CREDENTIAL"),
        "an allowlist should not need to predict this name"
    );
    for value in env.values() {
        assert!(!value.contains(token().as_str()));
    }
    // Enough survives that a shell still works.
    assert!(env.contains_key("PATH"));

    std::env::remove_var("TRACE_TEST_SECRET_TOKEN");
    std::env::remove_var("SOME_VENDOR_CREDENTIAL");
}

#[test]
fn the_allowlist_holds_no_credential_shaped_names() {
    for name in ENV_ALLOWLIST {
        let upper = name.to_uppercase();
        for banned in ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"] {
            assert!(
                !upper.contains(banned),
                "{name} is on the allowlist and looks like a credential"
            );
        }
    }
}

/// End to end: a bash tool cannot see the key, even by dumping its whole
/// environment.
#[test]
fn a_bash_tool_cannot_read_the_api_key() {
    let dir = TempDir::new("scrub-bash");
    std::env::set_var("TRACE_LEAK_PROBE", token());

    let out = trace_core::tools::bash::run_bash(
        "env; echo ---; echo \"$TRACE_LEAK_PROBE\"",
        dir.path(),
        std::time::Duration::from_secs(20),
    )
    .unwrap();

    assert!(
        !out.output.contains(token().as_str()),
        "the tool process could read the key:\n{}",
        out.output
    );
    std::env::remove_var("TRACE_LEAK_PROBE");
}
