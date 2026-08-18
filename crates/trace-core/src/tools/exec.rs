//! Where a tool call actually runs.
//!
//! Phase 1 ran everything on the host, in a directory. That was fine while the
//! only caller was a developer on their own machine, and it stops being fine
//! the moment a benchmark runs untrusted model output, or Phase 3 needs a
//! policy boundary to enforce.
//!
//! So execution is a seam. [`HostExec`] is the old behaviour;
//! [`ContainerExec`] runs the same command inside a container. Nothing above
//! this trait knows the difference — including the context builder, which sees
//! only [`Executor::workdir`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{Error, Result};

use super::bash::{run_bash, BashOutcome};

pub trait Executor: Send + Sync {
    fn run(&self, cmd: &str, timeout: Duration) -> Result<BashOutcome>;

    /// The working directory as the *agent* understands it.
    ///
    /// This string goes into the system prompt, which means it is part of the
    /// cacheable prefix. A host path under a per-attempt temp directory is
    /// therefore not merely untidy: it changes the prefix on every run, so no
    /// two attempts in a sweep can ever share a cache entry. A container that
    /// always mounts at the same path fixes that for free.
    fn workdir(&self) -> &str;

    /// Host-side path to the workspace, when there is one.
    ///
    /// Checkpoints and verification need to touch real files. A future
    /// executor with no host mount returns `None` and simply does not support
    /// host-side checkpointing.
    fn host_path(&self) -> Option<&Path>;
}

pub struct HostExec {
    cwd: PathBuf,
    workdir: String,
}

impl HostExec {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        HostExec {
            workdir: cwd.display().to_string(),
            cwd,
        }
    }
}

impl Executor for HostExec {
    fn run(&self, cmd: &str, timeout: Duration) -> Result<BashOutcome> {
        run_bash(cmd, &self.cwd, timeout)
    }

    fn workdir(&self) -> &str {
        &self.workdir
    }

    fn host_path(&self) -> Option<&Path> {
        Some(&self.cwd)
    }
}

/// Runs commands inside an already-started container via `docker exec`.
///
/// The harness itself stays on the host, holding the log and the provider
/// connection, so a container that dies takes the attempt with it but not the
/// ledger. That matches the checkpoint triple's reasoning: a sandbox should be
/// rebuildable, and binding a session's survival to a container means a
/// container failure kills the task.
pub struct ContainerExec {
    runtime: String,
    container: String,
    workdir: String,
    host_path: Option<PathBuf>,
}

impl ContainerExec {
    pub fn new(
        runtime: impl Into<String>,
        container: impl Into<String>,
        workdir: impl Into<String>,
    ) -> Self {
        ContainerExec {
            runtime: runtime.into(),
            container: container.into(),
            workdir: workdir.into(),
            host_path: None,
        }
    }

    /// Record the host side of a bind mount, so checkpoints and verification
    /// can still reach the files.
    pub fn with_host_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.host_path = Some(path.into());
        self
    }
}

impl Executor for ContainerExec {
    fn run(&self, cmd: &str, timeout: Duration) -> Result<BashOutcome> {
        let started = std::time::Instant::now();

        // `docker exec` is driven through the host's bash so the existing
        // timeout, process-group kill, and stdin-from-/dev/null behaviour all
        // still apply — to the exec client. The `--` guard stops a command
        // beginning with a dash from being read as docker's own flag.
        let inner = format!("exec 2>&1\n{cmd}");
        let script = format!(
            "{} exec -i -w {} {} bash -c {} </dev/null",
            self.runtime,
            shell_quote(&self.workdir),
            shell_quote(&self.container),
            shell_quote(&inner)
        );

        let mut outcome = run_bash(&script, Path::new("."), timeout)?;

        if outcome.timed_out {
            // Killing the exec client leaves the command running inside the
            // container. Stop the container itself, or the next attempt
            // inherits a busy machine and a mysteriously slow first command.
            let _ = Command::new(&self.runtime)
                .args(["kill", &self.container])
                .output();
        }

        outcome.wall_ms = started.elapsed().as_millis() as u64;
        Ok(outcome)
    }

    fn workdir(&self) -> &str {
        &self.workdir
    }

    fn host_path(&self) -> Option<&Path> {
        self.host_path.as_deref()
    }
}

/// Single-quote a string for safe interpolation into a shell command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Is a container runtime usable right now?
///
/// Checks the daemon, not just the binary: `docker` installed with nothing
/// running produces a confusing failure several steps later.
pub fn runtime_available(runtime: &str) -> Result<()> {
    let out = Command::new(runtime)
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .map_err(|e| Error::other(format!("{runtime} is not installed or not on PATH: {e}")))?;

    if !out.status.success() {
        return Err(Error::other(format!(
            "{runtime} is installed but not responding. Is the daemon running?\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}
