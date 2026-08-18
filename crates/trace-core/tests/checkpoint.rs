//! Checkpoints and rewind.
//!
//! The property that matters most here is what checkpointing does *not* do:
//! it must never disturb the user's HEAD, branches, or staged changes. A
//! harness that quietly rewrites your index is worse than one that does not
//! checkpoint at all.

mod common;

use std::path::Path;
use std::process::Command;

use common::TempDir;

use trace_core::runtime::checkpoint::{create, is_git_repo, rewind, workspace_hash};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo(dir: &TempDir) -> std::path::PathBuf {
    let root = dir.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "original\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    root
}

#[test]
fn detects_a_git_workspace() {
    let dir = TempDir::new("ckpt-detect");
    let root = repo(&dir);
    assert!(is_git_repo(&root));
    assert!(!is_git_repo(dir.path()));
}

#[test]
fn captures_and_restores_tracked_files() {
    let dir = TempDir::new("ckpt-restore");
    let root = repo(&dir);

    std::fs::write(root.join("a.txt"), "checkpointed\n").unwrap();
    std::fs::write(root.join("new.txt"), "also captured\n").unwrap();
    let ckpt = create(&root, "s1", "before-risky-edit", 42).unwrap();
    assert_eq!(ckpt.log_seq, 42);

    std::fs::write(root.join("a.txt"), "broken\n").unwrap();
    std::fs::remove_file(root.join("new.txt")).unwrap();

    rewind(&root, &ckpt).unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "checkpointed\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new.txt")).unwrap(),
        "also captured\n",
        "untracked-but-not-ignored files should be captured by the checkpoint"
    );
}

/// The non-invasiveness guarantee.
#[test]
fn checkpointing_leaves_head_index_and_branches_alone() {
    let dir = TempDir::new("ckpt-noninvasive");
    let root = repo(&dir);

    // Put the repo in a specific state: one staged change, one unstaged.
    std::fs::write(root.join("staged.txt"), "staged\n").unwrap();
    git(&root, &["add", "staged.txt"]);
    std::fs::write(root.join("a.txt"), "unstaged edit\n").unwrap();

    let head_before = git(&root, &["rev-parse", "HEAD"]);
    let status_before = git(&root, &["status", "--porcelain"]);
    let branch_before = git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]);

    create(&root, "s1", "mid-task", 7).unwrap();

    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git(&root, &["status", "--porcelain"]), status_before);
    assert_eq!(
        git(&root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        branch_before
    );
}

#[test]
fn checkpoints_are_reachable_by_ref() {
    let dir = TempDir::new("ckpt-ref");
    let root = repo(&dir);

    let ckpt = create(&root, "sess", "one", 5).unwrap();
    let resolved = git(&root, &["rev-parse", "refs/trace/sess/5"]);
    assert_eq!(resolved, ckpt.git_ref);
}

/// The workspace hash covers what git will not: ignored files. It detects
/// change; it does not claim to be a content identity.
#[test]
fn workspace_hash_tracks_ignored_files() {
    let dir = TempDir::new("ckpt-wshash");
    let root = repo(&dir);
    std::fs::create_dir_all(root.join("ignored")).unwrap();

    let before = workspace_hash(&root).unwrap();
    std::fs::write(root.join("ignored/blob.bin"), "some cache\n").unwrap();
    let after = workspace_hash(&root).unwrap();

    assert_ne!(before, after, "an ignored file appeared and went unnoticed");
    assert_eq!(after, workspace_hash(&root).unwrap(), "hash is not stable");
}

/// Rewind reports drift in ignored files rather than deleting them. Removing
/// whatever git does not know about would take `node_modules` and build caches
/// with it, which is not this command's call to make.
#[test]
fn rewind_reports_untracked_drift_without_deleting() {
    let dir = TempDir::new("ckpt-drift");
    let root = repo(&dir);
    std::fs::create_dir_all(root.join("ignored")).unwrap();

    let ckpt = create(&root, "s1", "before", 1).unwrap();
    std::fs::write(root.join("ignored/cache.bin"), "built later\n").unwrap();

    let report = rewind(&root, &ckpt).unwrap();

    assert!(report.untracked_drifted, "drift went unreported");
    assert!(
        root.join("ignored/cache.bin").exists(),
        "rewind deleted an ignored file it never captured"
    );
}
