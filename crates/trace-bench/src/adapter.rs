//! Preparing a workspace and grading what came back.
//!
//! One rule governs this file: **the agent must never be able to reach the
//! thing that grades it.** The verification script is copied in only after the
//! agent has stopped, run, and deleted. An agent that can read `verify.sh` can
//! satisfy it without doing the work, and an agent that can edit it can pass
//! unconditionally — in both cases your benchmark reports a number that means
//! nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use trace_core::error::{Error, Result};
use trace_core::tools::bash::run_bash;
use trace_core::tools::exec::{Executor, HostExec};

use crate::task::Task;

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub passed: bool,
    pub exit_code: i32,
    pub output: String,
}

pub trait Adapter {
    /// Materialize a fresh workspace for one attempt.
    fn prepare(&self, task: &Task, run_dir: &Path) -> Result<PathBuf>;

    /// Where this attempt's tool calls should run. Defaults to the host.
    fn executor(&self, workspace: &Path) -> Result<Arc<dyn Executor>> {
        Ok(Arc::new(HostExec::new(workspace)))
    }

    /// Grade the workspace. Called once, after the agent has stopped.
    fn verify(&self, task: &Task, workspace: &Path) -> Result<Verdict>;

    /// Release anything the attempt allocated.
    ///
    /// Called even when the attempt failed, because the failure path is
    /// exactly when a leaked container costs you the rest of the sweep.
    fn cleanup(&self, _workspace: &Path) {}
}

/// Runs tasks directly on the host in a scratch directory.
///
/// Not a substitute for a container — it offers no isolation, and Phase 3 is
/// where that gets addressed. It exists so the rig itself is testable and so a
/// sweep can run before any container infrastructure does.
pub struct LocalAdapter;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(300);

impl Adapter for LocalAdapter {
    fn prepare(&self, task: &Task, run_dir: &Path) -> Result<PathBuf> {
        let workspace = run_dir.join("workspace");
        if workspace.exists() {
            std::fs::remove_dir_all(&workspace).map_err(|e| Error::io(&workspace, e))?;
        }
        std::fs::create_dir_all(&workspace).map_err(|e| Error::io(&workspace, e))?;

        let seed = task.seed_workspace();
        if seed.exists() {
            copy_dir(&seed, &workspace)?;
        }

        // A fresh git repo per attempt, so checkpoints work and so the agent
        // sees a normal-looking project rather than a bare directory.
        let _ = run_bash(
            "git init -q . && git add -A && \
             git -c user.name=bench -c user.email=bench@localhost commit -qm task --allow-empty",
            &workspace,
            Duration::from_secs(60),
        );

        Ok(workspace)
    }

    fn verify(&self, task: &Task, workspace: &Path) -> Result<Verdict> {
        let script = workspace.join(".trace-verify.sh");
        std::fs::copy(task.verify_script(), &script)
            .map_err(|e| Error::io(task.verify_script(), e))?;

        let outcome = run_bash("bash .trace-verify.sh", workspace, VERIFY_TIMEOUT)?;

        // Remove it immediately. If a later stage ever reuses this workspace,
        // the script must not still be sitting there.
        let _ = std::fs::remove_file(&script);

        Ok(Verdict {
            passed: outcome.exit_code == 0 && !outcome.timed_out,
            exit_code: outcome.exit_code,
            output: outcome.output,
        })
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from).map_err(|e| Error::io(from, e))? {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());

        let meta = entry.metadata().map_err(|e| Error::io(&src, e))?;
        if meta.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| Error::io(&dst, e))?;
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| Error::io(&src, e))?;
            // Preserve the executable bit; a seed workspace often ships a
            // script the task expects to be runnable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode();
                let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}
