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

const FAKE_KEY: &str = "sk-test-DO-NOT-LEAK-2f8a91c4";

#[test]
fn a_key_never_appears_in_the_log_or_the_context() {
    let dir = TempDir::new("secrets");
    let mut cfg = test_config();
    cfg.model.api_key_env = "TRACE_TEST_KEY".into();

    std::env::set_var("TRACE_TEST_KEY", FAKE_KEY);

    let turns = vec![
        ScriptedProvider::bash("c1", "echo working"),
        ScriptedProvider::say("done"),
    ];
    let (log_path, events) = scripted_session(&dir, &cfg, "do something", turns);

    // Nothing on disk.
    let raw = std::fs::read_to_string(&log_path).unwrap();
    assert!(!raw.contains(FAKE_KEY), "the key reached the event log");

    // Nothing in the rendered context either.
    let ctx = trace_core::context::build_context(&events, &cfg, events.last().unwrap().seq);
    let rendered = serde_json::to_string(&ctx.messages).unwrap();
    assert!(!rendered.contains(FAKE_KEY), "the key reached the context");

    // Config records the variable's name, never its value.
    let serialized = serde_json::to_string(&cfg).unwrap();
    assert!(serialized.contains("TRACE_TEST_KEY"));
    assert!(!serialized.contains(FAKE_KEY));

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
