//! The policy engine.
//!
//! Several of these correspond directly to numbered entries in the manual's
//! escape suite. They test the *policy* layer only — the OS layer underneath
//! is what catches what this misses, and it gets its own tests. Neither is
//! trusted to be complete alone.

mod common;

use std::path::PathBuf;

use common::TempDir;

use trace_core::policy::{baseline, glob, path, Action, Effect, Policy, Vars};

const POLICY: &str = r#"
default = "allow"

[[rules]]
id = "fs-write-workspace"
effect = "allow"
tool = "edit|write"
path = "$WORKSPACE/**"

[[rules]]
id = "fs-deny-dotgit-hooks"
effect = "deny"
tool = "*"
path = "$WORKSPACE/.git/hooks/**"
reason = "hooks execute later, outside the sandbox"

[[rules]]
id = "net-default"
effect = "deny"
tool = "bash"
net = ["*"]

[[rules]]
id = "net-registries"
effect = "allow"
tool = "bash"
net = ["registry.npmjs.org", "pypi.org"]

[[rules]]
id = "destructive"
effect = "prompt"
tool = "bash"
cmd = ["rm -rf *", "git push *", "* --force *"]
"#;

fn policy() -> Policy {
    Policy::from_toml(POLICY).expect("parse policy")
}

fn vars(ws: &TempDir) -> Vars {
    Vars {
        workspace: ws.path().to_path_buf(),
        session: "s-test".into(),
    }
}

fn act(tool: &str) -> Action {
    Action {
        tool: tool.into(),
        ..Default::default()
    }
}

// ───────────────────────────────────────────────────────── DSL

#[test]
fn the_manual_rule_shape_parses() {
    let p = policy();
    assert_eq!(p.rules.len(), 5);
    assert_eq!(p.default, Effect::Allow);
    assert_eq!(p.rules[1].effect, Effect::Deny);
    assert_eq!(
        p.rules[1].reason.as_deref(),
        Some("hooks execute later, outside the sandbox")
    );
}

/// A decision names a rule. Two rules answering to one name makes the log
/// unattributable.
#[test]
fn duplicate_rule_ids_are_refused() {
    let src = r#"
default = "deny"
[[rules]]
id = "same"
effect = "allow"
[[rules]]
id = "same"
effect = "deny"
"#;
    let err = Policy::from_toml(src).unwrap_err().to_string();
    assert!(err.contains("duplicate rule id"), "{err}");
}

// ───────────────────────────────────────────── precedence

#[test]
fn deny_beats_prompt_beats_allow() {
    assert!(Effect::Deny > Effect::Prompt);
    assert!(Effect::Prompt > Effect::Allow);

    let ws = TempDir::new("policy-precedence");
    let p = policy();

    // `rm -rf` in the workspace matches both the allow and the prompt rule.
    let action = Action {
        tool: "bash".into(),
        command: Some("rm -rf build/".into()),
        ..Default::default()
    };
    assert_eq!(p.evaluate(&action, &vars(&ws)).effect, Effect::Prompt);
}

/// Order affects which rule is *named*, never the outcome — so reordering a
/// policy file cannot quietly weaken it.
#[test]
fn reordering_rules_cannot_change_the_outcome() {
    let ws = TempDir::new("policy-order");
    let forward = policy();
    let mut backward = policy();
    backward.rules.reverse();

    let hooks = ws.join(".git/hooks/post-commit");
    let action = Action {
        tool: "write".into(),
        paths: vec![hooks],
        ..Default::default()
    };

    assert_eq!(
        forward.evaluate(&action, &vars(&ws)).effect,
        backward.evaluate(&action, &vars(&ws)).effect
    );
    assert_eq!(forward.evaluate(&action, &vars(&ws)).effect, Effect::Deny);
}

#[test]
fn an_unmatched_action_gets_the_default() {
    let ws = TempDir::new("policy-default");
    let mut p = policy();
    let d = p.evaluate(&act("read"), &vars(&ws));
    assert_eq!(d.effect, Effect::Allow);
    assert_eq!(d.rule, "default");

    p.default = Effect::Deny;
    assert_eq!(p.evaluate(&act("read"), &vars(&ws)).effect, Effect::Deny);
}

/// In a headless run there is nobody to ask, so `prompt` must not be treated
/// as permission.
#[test]
fn prompt_is_not_allowed() {
    let ws = TempDir::new("policy-prompt");
    let action = Action {
        tool: "bash".into(),
        command: Some("git push --force origin main".into()),
        ..Default::default()
    };
    let d = policy().evaluate(&action, &vars(&ws));
    assert_eq!(d.effect, Effect::Prompt);
    assert!(!d.allowed(), "prompt must not count as allowed");
}

// ─────────────────────────────────── escape suite: paths

/// Escape 12: write `.git/hooks/post-commit`.
#[test]
fn escape_12_git_hooks_are_denied() {
    let ws = TempDir::new("escape-12");
    std::fs::create_dir_all(ws.path().join(".git/hooks")).unwrap();

    let action = Action {
        tool: "write".into(),
        paths: vec![ws.join(".git/hooks/post-commit")],
        ..Default::default()
    };
    let d = policy().evaluate(&action, &vars(&ws));
    assert_eq!(d.effect, Effect::Deny);
    assert_eq!(d.rule, "fs-deny-dotgit-hooks");
    assert!(d.reason.contains("outside the sandbox"));
}

/// Escape 04: `../../` traversal out of a nested directory.
///
/// The pattern `$WORKSPACE/**` must not match a path that merely *starts*
/// with the workspace text.
#[test]
fn escape_04_parent_traversal_leaves_the_workspace() {
    let ws = TempDir::new("escape-04");
    std::fs::create_dir_all(ws.path().join("nested/deep")).unwrap();

    let escaped = ws.join("nested/deep/../../../../../../etc/passwd");
    assert!(
        !path::is_within(ws.path(), &escaped),
        "traversal was treated as inside the workspace: {}",
        path::for_matching(&escaped)
    );

    let inside = ws.join("nested/deep/../file.txt");
    assert!(path::is_within(ws.path(), &inside));
}

/// Escape 03: symlink out of the workspace, then read through it.
#[cfg(unix)]
#[test]
fn escape_03_symlink_out_of_the_workspace_is_resolved() {
    let ws = TempDir::new("escape-03");
    let outside = TempDir::new("escape-03-outside");
    std::fs::write(outside.join("secret.txt"), "sensitive").unwrap();

    let link = ws.join("innocent.txt");
    std::os::unix::fs::symlink(outside.join("secret.txt"), &link).unwrap();

    // Textually the link is inside the workspace. It is not.
    assert!(link.starts_with(ws.path()));
    assert!(
        !path::is_within(ws.path(), &link),
        "a symlink out of the workspace was treated as local"
    );

    let strict = Policy::from_toml(
        r#"
default = "deny"
[[rules]]
id = "workspace-only"
effect = "allow"
tool = "*"
path = "$WORKSPACE/**"
"#,
    )
    .unwrap();

    let action = Action {
        tool: "read".into(),
        paths: vec![link],
        ..Default::default()
    };
    assert_eq!(
        strict.evaluate(&action, &vars(&ws)).effect,
        Effect::Deny,
        "reading through a symlink escaped the workspace rule"
    );
}

/// A path being created does not exist yet, and still has to resolve.
#[test]
fn a_path_that_does_not_exist_yet_still_resolves() {
    let ws = TempDir::new("policy-new-file");
    let fresh = ws.join("does/not/exist/yet.txt");
    assert!(path::is_within(ws.path(), &fresh));

    let escaping = ws.join("does/not/../../../../etc/shadow");
    assert!(!path::is_within(ws.path(), &escaping));
}

// ──────────────────────────────────── escape suite: net

/// Escape 08: outbound TCP to a host that is not allowlisted.
#[test]
fn escape_08_unlisted_host_is_denied() {
    let ws = TempDir::new("escape-08");
    let p = policy();

    let evil = Action {
        tool: "bash".into(),
        hosts: vec!["evil.example".into()],
        ..Default::default()
    };
    assert_eq!(p.evaluate(&evil, &vars(&ws)).effect, Effect::Deny);

    let allowed = Action {
        tool: "bash".into(),
        hosts: vec!["pypi.org".into()],
        ..Default::default()
    };
    assert_eq!(p.evaluate(&allowed, &vars(&ws)).effect, Effect::Allow);
}

/// Escape 09: DNS exfiltration to a subdomain of an allowlisted-looking host.
#[test]
fn escape_09_lookalike_subdomains_do_not_inherit_permission() {
    let ws = TempDir::new("escape-09");
    let p = policy();

    for host in [
        "pypi.org.evil.tld",
        "notpypi.org",
        "secret.pypi.org.attacker.net",
    ] {
        let action = Action {
            tool: "bash".into(),
            hosts: vec![host.into()],
            ..Default::default()
        };
        assert_eq!(
            p.evaluate(&action, &vars(&ws)).effect,
            Effect::Deny,
            "{host} was allowed by an allowlist meant for pypi.org"
        );
    }
}

// ───────────────────────────────────────────── baseline

#[test]
fn the_baseline_closes_the_pure_downside_gaps() {
    let ws = TempDir::new("baseline");
    let p = baseline();
    let v = vars(&ws);

    let denied = [
        ("write", ws.join(".git/hooks/pre-push")),
        ("read", PathBuf::from("/home/someone/.ssh/id_rsa")),
        ("write", PathBuf::from("/etc/trace/policy/ci.toml")),
    ];
    for (tool, target) in denied {
        let action = Action {
            tool: tool.into(),
            paths: vec![target.clone()],
            ..Default::default()
        };
        assert_eq!(
            p.evaluate(&action, &v).effect,
            Effect::Deny,
            "baseline allowed {}",
            target.display()
        );
    }

    // Ordinary work is still permitted.
    let ordinary = Action {
        tool: "write".into(),
        paths: vec![ws.join("src/main.rs")],
        ..Default::default()
    };
    assert_eq!(p.evaluate(&ordinary, &v).effect, Effect::Allow);
}

// ───────────────────────────────────────────────── globs

/// `*` must not cross a path separator, or `$WORKSPACE/*` silently permits
/// everything beneath it and beyond.
#[test]
fn a_single_star_stays_within_one_segment() {
    assert!(glob::matches("/ws/*", "/ws/file.txt"));
    assert!(!glob::matches("/ws/*", "/ws/sub/file.txt"));
    assert!(glob::matches("/ws/**", "/ws/sub/deep/file.txt"));
    assert!(glob::matches("/ws/**", "/ws/file.txt"));
}

#[test]
fn double_star_matches_zero_segments() {
    assert!(glob::matches("a/**/b", "a/b"));
    assert!(glob::matches("a/**/b", "a/x/b"));
    assert!(glob::matches("a/**/b", "a/x/y/z/b"));
    assert!(!glob::matches("a/**/b", "a/x/y/c"));
}

#[test]
fn alternation_and_wildcards_on_tool_names() {
    assert!(glob::matches_alternation("edit|write", "edit"));
    assert!(glob::matches_alternation("edit|write", "write"));
    assert!(!glob::matches_alternation("edit|write", "bash"));
    assert!(glob::matches_alternation("*", "anything"));
}

#[test]
fn command_patterns_match_anywhere_in_the_text() {
    assert!(glob::matches_flat(
        "* --force *",
        "git push --force origin main"
    ));
    assert!(glob::matches_flat("rm -rf *", "rm -rf /"));
    assert!(!glob::matches_flat("rm -rf *", "rm file.txt"));
}

// ─────────────────────────────────────────── specificity

/// The manual's own example only works if a narrow allow can carve an
/// exception out of a broad deny. This pins that reading.
#[test]
fn a_narrow_allow_carves_out_of_a_broad_deny() {
    let ws = TempDir::new("spec-carve");
    let p = policy();

    let pypi = Action {
        tool: "bash".into(),
        hosts: vec!["pypi.org".into()],
        ..Default::default()
    };
    let d = p.evaluate(&pypi, &vars(&ws));
    assert_eq!(d.effect, Effect::Allow);
    assert_eq!(
        d.rule, "net-registries",
        "the broad deny shadowed the allowlist"
    );
}

/// The dangerous direction: a *broad* allow must never override a narrower
/// deny. This is the property that keeps the previous test from being a hole.
#[test]
fn a_broad_allow_never_overrides_a_narrow_deny() {
    let ws = TempDir::new("spec-narrow-deny");
    std::fs::create_dir_all(ws.path().join(".git/hooks")).unwrap();

    let action = Action {
        tool: "write".into(),
        paths: vec![ws.join(".git/hooks/post-commit")],
        ..Default::default()
    };
    // fs-write-workspace allows $WORKSPACE/**; fs-deny-dotgit-hooks denies a
    // strictly narrower path. The narrower rule must win.
    let d = policy().evaluate(&action, &vars(&ws));
    assert_eq!(
        d.effect,
        Effect::Deny,
        "a broad allow overrode a narrow deny"
    );
    assert_eq!(d.rule, "fs-deny-dotgit-hooks");
}

/// At equal specificity, deny still wins.
#[test]
fn deny_wins_among_equally_specific_rules() {
    let ws = TempDir::new("spec-tie");
    let p = Policy::from_toml(
        r#"
default = "allow"
[[rules]]
id = "allow-it"
effect = "allow"
tool = "bash"
cmd = ["deploy *"]
[[rules]]
id = "deny-it"
effect = "deny"
tool = "bash"
cmd = ["deploy *"]
"#,
    )
    .unwrap();

    let action = Action {
        tool: "bash".into(),
        command: Some("deploy production".into()),
        ..Default::default()
    };
    let d = p.evaluate(&action, &vars(&ws));
    assert_eq!(d.effect, Effect::Deny);
    assert_eq!(d.rule, "deny-it");
}

/// A list is only as specific as the loosest thing it admits, or
/// `["pypi.org", "*"]` would outrank `["pypi.org"]` while permitting the
/// entire internet.
#[test]
fn a_wildcard_in_a_list_does_not_buy_specificity() {
    let ws = TempDir::new("spec-list");
    let p = Policy::from_toml(
        r#"
default = "allow"
[[rules]]
id = "deny-net"
effect = "deny"
tool = "bash"
net = ["*"]
[[rules]]
id = "sneaky-allow"
effect = "allow"
tool = "bash"
net = ["pypi.org", "*"]
"#,
    )
    .unwrap();

    let action = Action {
        tool: "bash".into(),
        hosts: vec!["evil.example".into()],
        ..Default::default()
    };
    assert_eq!(
        p.evaluate(&action, &vars(&ws)).effect,
        Effect::Deny,
        "a wildcard smuggled into an allowlist beat the broad deny"
    );
}
