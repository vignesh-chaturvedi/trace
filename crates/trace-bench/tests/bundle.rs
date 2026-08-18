//! The shareable bundle, and the gate that stops it leaking.
//!
//! The situation being defended: a file written on someone else's machine,
//! sent over WhatsApp, by a person doing the repo owner a favour. A wrong
//! number costs an afternoon. A leaked key costs them their credential.

use trace_bench::scan;

/// Build a credential-shaped string at run time.
///
/// Never write one as a literal. GitHub's push protection blocks a push
/// containing one — correctly, because a scanner cannot tell a test fixture
/// from a live key, and one that tried to would be worth much less. Joining
/// the halves at call time exercises exactly the same code path without
/// leaving the pattern in a file anyone can grep.
fn shaped(prefix: &str, body: &str) -> String {
    format!("{prefix}{body}")
}

/// A realistic-looking OpenAI key, assembled at run time.
fn fake_key() -> String {
    shaped("sk-", "proj-7Qb2mVx9LpAe4RtYuIoP3sDfGhJkZxCvBnM1qW")
}

// ─────────────────────────────────────────── secret scan

#[test]
fn a_planted_openai_key_is_caught() {
    let key = fake_key();
    let text = format!("some log output\nAUTH=Bearer {key}\nmore output\n");
    let findings = scan::scan(&text);
    assert!(!findings.is_empty(), "an obvious key sailed through");
    assert!(findings.iter().any(|f| f.kind.contains("OpenAI")));
}

#[test]
fn common_provider_prefixes_are_recognised() {
    let cases = [
        (
            shaped("AIza", "SyD-9tQwErTyUiOpAsDfGhJkLzXcVbNm12"),
            "Google",
        ),
        (shaped("ghp_", "16CharsMinimumAbcdefghijklmnop"), "GitHub"),
        (shaped("xoxb-", "1234567890-ABCDEFGHIJKLMNOP"), "Slack"),
        (shaped("AKIA", "IOSFODNN7EXAMPLE1234"), "AWS"),
        (shaped("hf_", "QwErTyUiOpAsDfGhJkLzXcVbNm"), "Hugging Face"),
    ];
    for (value, label) in cases {
        let findings = scan::scan(&format!("key = {value}"));
        assert!(!findings.is_empty(), "{label} key was not caught");
    }
}

/// The bundle is full of content hashes. Flagging every one would make the
/// scan noise, and a noisy gate is one people route around with --force.
#[test]
fn content_hashes_are_not_flagged() {
    let text = "\
| config hash | `895782c056e8a1b2c3d4e5f6` |
| task set hash | `02e7038b06ff9e8d7c6b5a49` |
context_hash: c45b70f6d07157e2cad8c5326e6fc0258189e68b0b7c3eb2db6da46ed77ec398
";
    assert!(scan::scan(text).is_empty(), "{:?}", scan::scan(text));
}

/// Ordinary prose about credentials must not trip it either — the bundle's own
/// "what is in this file" section talks about API keys by necessity.
#[test]
fn prose_about_secrets_is_not_a_secret() {
    let text = "\
It does not contain your API key. The harness never writes credentials to a
log - tool processes run with a scrubbed environment, and every line is
redacted on its way to disk.
";
    assert!(scan::scan(text).is_empty(), "{:?}", scan::scan(text));
}

/// A high-entropy value next to a credential word, even with no known prefix.
#[test]
fn an_unknown_token_next_to_a_credential_word_is_caught() {
    let text = "api_key=Zx9KpQ2mVbN4RtYuI7oP3sDfGhJ8kLzC";
    assert!(!scan::scan(text).is_empty(), "unfamiliar key shape missed");
}

#[test]
fn already_redacted_lines_do_not_trip_the_scan() {
    let text = "token was [redacted:OPENAI_API_KEY] in this line";
    assert!(scan::scan(text).is_empty());
}

/// The refusal must not print the thing it is refusing to leak.
#[test]
fn the_refusal_message_never_prints_the_secret() {
    let key = fake_key();
    let findings = scan::scan(&format!("Authorization: Bearer {key}"));
    let message = scan::describe(&findings);

    assert!(
        !message.contains(&key),
        "the error leaked the key it caught"
    );
    assert!(
        !message.contains("7Qb2mVx9LpAe"),
        "a recognisable chunk leaked"
    );
    assert!(message.contains("refusing to write"));
}

// ─────────────────────────────────────── bundle round trip

mod fixture {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use trace_bench::adapter::LocalAdapter;
    use trace_bench::sweep::{run_sweep, SweepOptions};
    use trace_bench::{Bundle, Task};
    use trace_core::config::Config;
    use trace_core::provider::{Provider, ScriptedProvider};

    static N: AtomicU32 = AtomicU32::new(0);

    pub struct Scratch(pub PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// Run a small sweep and bundle it. Returns (scratch, bundle, markdown).
    pub fn bundled(pass: bool) -> (Scratch, Bundle, String) {
        let dir = repo_root().join("target").join(format!(
            "bundle-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let scratch = Scratch(dir.clone());

        let task = Task::load(&repo_root().join("tasks/fix-sum")).unwrap();
        let tasks = vec![task];

        let turns = if pass {
            vec![
                ScriptedProvider::bash(
                    "c1",
                    "sed -i.bak 's/total - n/total + n/' sum.sh && rm -f sum.sh.bak",
                ),
                ScriptedProvider::say("fixed"),
            ]
        } else {
            vec![ScriptedProvider::say("I fixed it. Everything passes now.")]
        };

        let mut cfg = Config::default();
        cfg.model.name = "scripted".into();
        cfg.limits.tool_timeout_ms = 20_000;

        let report = run_sweep(
            &tasks,
            &cfg,
            &LocalAdapter,
            &|_: &Config| Ok(Box::new(ScriptedProvider::new(turns.clone())) as Box<dyn Provider>),
            &SweepOptions {
                repeats: 3,
                limit: None,
                out_dir: dir.join("bench"),
                harness_commit: "bundletest".into(),
                verbose: false,
            },
        )
        .unwrap();

        let bundle = Bundle::from_sweep(&report.dir, &tasks).unwrap();
        let markdown = bundle.to_markdown();
        (scratch, bundle, markdown)
    }

    pub fn tasks_at(root: &Path) -> Vec<Task> {
        Task::load_all(root).unwrap()
    }
}

use trace_bench::{Bundle, Task};

#[test]
fn a_bundle_survives_the_round_trip() {
    let (_s, original, markdown) = fixture::bundled(true);
    let parsed = Bundle::from_markdown(&markdown).expect("parse bundle");

    assert_eq!(parsed.format, original.format);
    assert_eq!(parsed.task_set_hash, original.task_set_hash);
    assert_eq!(parsed.rows.len(), original.rows.len());
    assert_eq!(parsed.aggregate.pass_rate, original.aggregate.pass_rate);
    assert_eq!(parsed.manifest.harness_commit, "bundletest");
}

/// The half a human reads has to actually say the important things.
#[test]
fn the_readable_half_carries_the_numbers() {
    let (_s, _b, markdown) = fixture::bundled(true);

    for expected in [
        "# TRACE benchmark results",
        "pass rate",
        "harness commit",
        "task set hash",
        "## What is in this file",
        "does **not** contain your API key",
    ] {
        assert!(
            markdown.contains(expected),
            "missing from bundle: {expected}"
        );
    }
}

/// Failures are the reason to look at someone else's run at all.
#[test]
fn failures_carry_an_excerpt() {
    let (_s, bundle, markdown) = fixture::bundled(false);

    assert_eq!(bundle.aggregate.pass_rate, 0.0);
    assert_eq!(bundle.failures.len(), 3);
    assert!(markdown.contains("## Failures"));

    let f = &bundle.failures[0];
    assert_eq!(f.task_id, "fix-sum");
    assert!(
        f.verify_output.contains("FAIL") || f.verify_output.contains("expected"),
        "verify output not captured: {:?}",
        f.verify_output
    );
}

/// A bundle must never claim a task set it did not run.
#[test]
fn the_hash_covers_only_the_tasks_that_ran() {
    let (_s, bundle, _m) = fixture::bundled(true);

    let one = vec![Task::load(&fixture::repo_root().join("tasks/fix-sum")).unwrap()];
    assert!(bundle.matches_task_set(&one));

    let all = fixture::tasks_at(&fixture::repo_root().join("tasks"));
    assert!(all.len() > 1);
    assert!(
        !bundle.matches_task_set(&all),
        "a one-task sweep claimed to match the full set"
    );
}

/// Content, not just names: a task whose verification was quietly loosened
/// keeps its id, and a comparison against it would be meaningless.
#[test]
fn changing_a_verify_script_changes_the_task_set_hash() {
    let root = fixture::repo_root().join("tasks");
    let before = trace_bench::bundle::task_set_hash(&fixture::tasks_at(&root));

    let victim = root.join("fix-sum/verify.sh");
    let original = std::fs::read_to_string(&victim).unwrap();
    std::fs::write(&victim, format!("{original}\n# loosened\n")).unwrap();
    let after = trace_bench::bundle::task_set_hash(&fixture::tasks_at(&root));
    std::fs::write(&victim, original).unwrap();

    assert_ne!(before, after, "verification changed and the hash did not");
}

#[test]
fn a_non_bundle_file_is_rejected_clearly() {
    let err = Bundle::from_markdown("# just some notes\n\nnothing here\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("is this a trace bundle"), "{err}");
}

/// A real bundle must pass its own gate, or the gate is unusable.
#[test]
fn a_genuine_bundle_passes_the_secret_scan() {
    let (_s, _b, markdown) = fixture::bundled(false);
    let findings = scan::scan(&markdown);
    assert!(
        findings.is_empty(),
        "the bundle tripped its own secret scan: {findings:?}"
    );
}
