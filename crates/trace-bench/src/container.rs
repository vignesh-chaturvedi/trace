//! A container-backed adapter.
//!
//! Two things this buys that [`LocalAdapter`](crate::adapter::LocalAdapter)
//! cannot:
//!
//! **Isolation.** A sweep runs model-authored shell commands with no human
//! reading them first. On the host, `rm -rf ~` is a bad afternoon.
//!
//! **A fixed workspace path.** The workspace mounts at `/workspace` for every
//! task and every repeat, so the `{cwd}` in the system prompt stops varying —
//! and one cacheable prefix is shared across the entire sweep instead of being
//! invalidated on every attempt. On the host that path is a per-attempt temp
//! directory, which quietly guarantees a cold cache every time.
//!
//! The harness itself stays outside, holding the log and the provider
//! connection, so a container that dies takes the attempt and not the ledger.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use trace_core::error::{Error, Result};
use trace_core::tools::exec::{runtime_available, shell_quote, ContainerExec, Executor};

use crate::adapter::{Adapter, Verdict};
use crate::task::Task;

/// The path a workspace is always mounted at, in every container, for every
/// task. Fixed on purpose — see the module docs.
pub const MOUNT: &str = "/workspace";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContainerConfig {
    pub runtime: String,
    pub image: String,
    /// Resource ceilings. These come from the benchmark and are applied as
    /// given; the rig never relaxes them to make a task pass.
    pub cpus: Option<String>,
    pub memory: Option<String>,
    pub pids: Option<u32>,
    /// Cut the container off from the network.
    ///
    /// Defaults to on. A task that can reach the internet can download the
    /// answer, and a benchmark that allows it is measuring something other
    /// than what it claims.
    pub network: bool,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        ContainerConfig {
            runtime: "docker".into(),
            image: "python:3.12-slim".into(),
            cpus: Some("2".into()),
            memory: Some("2g".into()),
            pids: Some(512),
            network: false,
        }
    }
}

pub struct ContainerAdapter {
    cfg: ContainerConfig,
    /// Workspace -> container id. Keyed by workspace rather than "whichever
    /// started last", so a failed attempt cannot make the next one verify
    /// inside the wrong container.
    live: std::sync::Mutex<BTreeMap<PathBuf, String>>,
}

impl ContainerAdapter {
    pub fn new(cfg: ContainerConfig) -> Result<ContainerAdapter> {
        runtime_available(&cfg.runtime)?;
        Ok(ContainerAdapter {
            cfg,
            live: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    /// Start a container for one attempt and return an executor bound to it.
    pub fn start(&self, host_workspace: &Path) -> Result<Arc<dyn Executor>> {
        let host = host_workspace
            .canonicalize()
            .map_err(|e| Error::io(host_workspace, e))?;

        let mut args: Vec<String> = vec![
            "run".into(),
            "-d".into(),
            "--rm".into(),
            "-v".into(),
            format!("{}:{MOUNT}", host.display()),
            "-w".into(),
            MOUNT.into(),
        ];

        if !self.cfg.network {
            args.push("--network".into());
            args.push("none".into());
        }
        if let Some(c) = &self.cfg.cpus {
            args.push("--cpus".into());
            args.push(c.clone());
        }
        if let Some(m) = &self.cfg.memory {
            args.push("--memory".into());
            args.push(m.clone());
        }
        if let Some(p) = self.cfg.pids {
            args.push("--pids-limit".into());
            args.push(p.to_string());
        }

        args.push(self.cfg.image.clone());
        // Something that stays alive without a TTY, so `exec` has a target.
        args.push("sleep".into());
        args.push("infinity".into());

        let out = Command::new(&self.cfg.runtime)
            .args(&args)
            .output()
            .map_err(|e| Error::other(format!("{} run failed: {e}", self.cfg.runtime)))?;

        if !out.status.success() {
            return Err(Error::other(format!(
                "could not start container from {}: {}",
                self.cfg.image,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(host.clone(), id.clone());

        // Prove the mount before handing the container to an agent.
        if let Err(e) = self.check_mount(&id, &host) {
            self.cleanup(&host);
            return Err(e);
        }

        Ok(Arc::new(
            ContainerExec::new(&self.cfg.runtime, id, MOUNT).with_host_path(host),
        ))
    }

    /// Confirm the bind mount actually carries the host workspace.
    ///
    /// On macOS the container runtime is a Linux VM, and only paths the VM has
    /// been told to share reach it. Bind-mounting an unshared path does not
    /// fail — Docker creates an empty directory and starts the container
    /// happily. The agent then finds an empty workspace, every task fails, and
    /// the sweep reports a model that cannot code.
    ///
    /// That is far too expensive a lie to leave undetected, so every container
    /// proves its mount before any agent touches it.
    fn check_mount(&self, container: &str, host: &Path) -> Result<()> {
        let name = format!(".trace-mount-check-{}", std::process::id());
        let probe = host.join(&name);
        std::fs::write(&probe, b"ok").map_err(|e| Error::io(&probe, e))?;

        let seen = Command::new(&self.cfg.runtime)
            .args(["exec", container, "cat", &format!("{MOUNT}/{name}")])
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok")
            .unwrap_or(false);

        let _ = std::fs::remove_file(&probe);

        if seen {
            return Ok(());
        }

        Err(Error::other(format!(
            "the workspace at {} is not visible inside the container.\n\n\
             The container runtime is a Linux VM, and it only sees host paths that have \
             been shared with it. Mounting an unshared path silently yields an empty \
             directory rather than an error, so every task would fail for a reason that \
             looks like the model's fault.\n\n\
             Fix by putting the sweep's output under a shared path (a directory inside \
             your home directory is shared by default), or by adding this path to the \
             runtime's file-sharing settings:\n  \
               colima:         colima stop && colima start --mount {}:w\n  \
               Docker Desktop: Settings > Resources > File sharing",
            host.display(),
            host.display()
        )))
    }

    fn container_for(&self, workspace: &Path) -> Option<String> {
        let key = workspace.canonicalize().ok()?;
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
    }

    fn teardown_all(&self) {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        for (_, id) in std::mem::take(&mut *live) {
            let _ = Command::new(&self.cfg.runtime)
                .args(["rm", "-f", &id])
                .output();
        }
    }
}

impl Drop for ContainerAdapter {
    fn drop(&mut self) {
        self.teardown_all();
    }
}

impl Adapter for ContainerAdapter {
    fn prepare(&self, task: &Task, run_dir: &Path) -> Result<PathBuf> {
        // Seeding happens on the host side of the bind mount; the container
        // sees the result at MOUNT.
        crate::adapter::LocalAdapter.prepare(task, run_dir)
    }

    fn executor(&self, workspace: &Path) -> Result<Arc<dyn Executor>> {
        self.start(workspace)
    }

    fn cleanup(&self, workspace: &Path) {
        let Ok(key) = workspace.canonicalize() else {
            return;
        };
        let id = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        if let Some(id) = id {
            let _ = Command::new(&self.cfg.runtime)
                .args(["rm", "-f", &id])
                .output();
        }
    }

    /// Grade inside the container, using the same image and limits the agent
    /// had.
    ///
    /// Verifying on the host would test a different machine than the one the
    /// work happened on — a different Python, a different libc — and a
    /// benchmark whose verdict depends on which side of the mount you stand on
    /// is not measuring the task.
    fn verify(&self, task: &Task, workspace: &Path) -> Result<Verdict> {
        let script = workspace.join(".trace-verify.sh");
        std::fs::copy(task.verify_script(), &script)
            .map_err(|e| Error::io(task.verify_script(), e))?;

        let container = self
            .container_for(workspace)
            .ok_or_else(|| Error::other("no container is running for this workspace"))?;

        let out = Command::new(&self.cfg.runtime)
            .args([
                "exec",
                "-i",
                "-w",
                MOUNT,
                &container,
                "bash",
                "-c",
                &format!("bash {} 2>&1", shell_quote(".trace-verify.sh")),
            ])
            .output()
            .map_err(|e| Error::other(format!("verify exec failed: {e}")))?;

        let _ = std::fs::remove_file(&script);

        Ok(Verdict {
            passed: out.status.success(),
            exit_code: out.status.code().unwrap_or(-1),
            output: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}
