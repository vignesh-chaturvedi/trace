//! Checkpoints and rewind.
//!
//! A checkpoint is a triple, because no single artifact captures a session's
//! state:
//!
//! | component         | captures                                            |
//! |-------------------|-----------------------------------------------------|
//! | `git_ref`         | tracked workspace contents                          |
//! | `log_seq`         | position in the session ledger                       |
//! | `workspace_hash`  | ignored files git will not capture                  |
//!
//! The sandbox is deliberately **not** in the triple. A sandbox should be
//! rebuildable from scratch, and binding a session to a specific container
//! means a container failure kills the task.
//!
//! Committing uses a scratch index and `commit-tree`, so checkpointing never
//! touches the user's HEAD, branches, staged changes, or reflog. An agent
//! harness that quietly rewrites your index is worse than one that does not
//! checkpoint at all.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::event::{Checkpoint, Seq};
use crate::hash::hash_chunks;

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    // An explicit identity, because `commit-tree` refuses to run without one
    // and a fresh CI container has no global git config. Checkpoints must not
    // depend on how the host happens to be set up.
    cmd.arg("-C")
        .arg(root)
        .args(["-c", "user.name=trace", "-c", "user.email=trace@localhost"])
        .args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .map_err(|e| Error::other(format!("git {} failed to spawn: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn is_git_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--git-dir"], None).is_ok()
}

pub fn create(root: &Path, session: &str, label: &str, log_seq: Seq) -> Result<Checkpoint> {
    let index = std::env::temp_dir().join(format!("trace-index-{session}-{log_seq}"));
    let _ = std::fs::remove_file(&index);

    // Seed the scratch index from HEAD when there is one, so the resulting
    // commit is a real delta rather than an unrelated tree.
    let head = git(root, &["rev-parse", "HEAD"], None).ok();
    if head.is_some() {
        git(root, &["read-tree", "HEAD"], Some(&index))?;
    }

    git(root, &["add", "-A"], Some(&index))?;
    let tree = git(root, &["write-tree"], Some(&index))?;
    let _ = std::fs::remove_file(&index);

    let message = format!("trace checkpoint {label} @ seq {log_seq}");
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message.as_str()];
    if let Some(h) = &head {
        args.push("-p");
        args.push(h.as_str());
    }
    let commit = git(root, &args, None)?;

    let refname = format!("refs/trace/{session}/{log_seq}");
    git(root, &["update-ref", &refname, &commit], None)?;

    Ok(Checkpoint {
        label: label.to_string(),
        git_ref: commit,
        log_seq,
        workspace_hash: workspace_hash(root)?,
    })
}

/// Restore tracked files from a checkpoint commit.
///
/// Untracked and ignored files are left alone: deleting whatever git does not
/// know about would take `node_modules`, build caches, and the occasional
/// unsaved scratch file with it. The workspace hash is re-checked afterwards
/// so the caller learns that those files have drifted rather than assuming the
/// restore was total.
pub fn rewind(root: &Path, ckpt: &Checkpoint) -> Result<RewindReport> {
    git(root, &["checkout", &ckpt.git_ref, "--", "."], None)?;
    let now = workspace_hash(root)?;
    Ok(RewindReport {
        restored: ckpt.git_ref.clone(),
        log_seq: ckpt.log_seq,
        untracked_drifted: now != ckpt.workspace_hash,
    })
}

#[derive(Debug, Clone)]
pub struct RewindReport {
    pub restored: String,
    pub log_seq: Seq,
    /// Ignored files differ from when the checkpoint was taken. Not an error —
    /// they were never captured — but the caller should say so out loud.
    pub untracked_drifted: bool,
}

/// A change-detector over the files git will not capture.
///
/// Hashes `(path, len, mtime)` rather than contents. Ignored trees are where
/// `node_modules` and `target/` live, and hashing gigabytes at every
/// checkpoint would make checkpointing something you turn off. This detects
/// change, which is all the triple needs it to do; it is not a content
/// identity and does not claim to be.
pub fn workspace_hash(root: &Path) -> Result<String> {
    let listing = match git(
        root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ],
        None,
    ) {
        Ok(l) => l,
        // Not a repo, or git unavailable: an empty hash is honest here.
        Err(_) => return Ok(hash_chunks(std::iter::empty())),
    };

    let mut entries: Vec<String> = Vec::new();
    for rel in listing.lines() {
        let path = root.join(rel);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        entries.push(format!("{rel}\u{1}{}\u{1}{mtime}", meta.len()));
    }
    entries.sort();

    Ok(hash_chunks(entries.iter().map(|s| s.as_bytes())))
}
