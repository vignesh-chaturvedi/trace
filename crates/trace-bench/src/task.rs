//! What a benchmark task is.
//!
//! A task is self-contained on disk: a prompt, a seed workspace, a
//! verification command, and **its own limits**. The last of those is the one
//! that needs defending.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use trace_core::config::Config;
use trace_core::error::{Error, Result};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    /// What the agent is told. The only thing it sees.
    pub prompt: String,
    #[serde(default)]
    pub limits: TaskLimits,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// The benchmark's limits, not the operator's.
///
/// Tuning these locally is the fastest way to waste an entire evaluation: the
/// numbers stop transferring to a submission, and you find out only when
/// someone else fails to reproduce them. So [`Task::apply_limits`] lets a task
/// tighten the operator's config and never loosen it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct TaskLimits {
    pub max_turns: Option<u64>,
    pub max_usd: Option<f64>,
    pub tool_timeout_ms: Option<u64>,
    /// Wall-clock ceiling for the whole task, verification included.
    pub wall_timeout_secs: Option<u64>,
}

pub const TASK_FILE: &str = "task.toml";
pub const WORKSPACE_DIR: &str = "workspace";
pub const VERIFY_FILE: &str = "verify.sh";

impl Task {
    pub fn load(dir: &Path) -> Result<Task> {
        let manifest = dir.join(TASK_FILE);
        let src = std::fs::read_to_string(&manifest).map_err(|e| Error::io(&manifest, e))?;
        let mut task: Task = toml::from_str(&src)
            .map_err(|e| Error::Config(format!("{}: {e}", manifest.display())))?;
        task.dir = dir.to_path_buf();
        task.validate()?;
        Ok(task)
    }

    /// Load every task in a directory, sorted by id.
    ///
    /// Sorted because sweep order must not depend on how the filesystem
    /// happens to enumerate directories; two machines running the same sweep
    /// should do the same work in the same order.
    pub fn load_all(root: &Path) -> Result<Vec<Task>> {
        let entries = std::fs::read_dir(root).map_err(|e| Error::io(root, e))?;
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join(TASK_FILE).exists())
            .collect();
        dirs.sort();

        let mut tasks = Vec::with_capacity(dirs.len());
        for d in dirs {
            tasks.push(Task::load(&d)?);
        }

        if tasks.is_empty() {
            return Err(Error::Config(format!(
                "no tasks found under {} (a task is a directory containing {TASK_FILE})",
                root.display()
            )));
        }
        Ok(tasks)
    }

    fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            return Err(Error::Config(format!(
                "{}: empty task id",
                self.dir.display()
            )));
        }
        // The id becomes a filename in bench/runs/; keep it boring.
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::Config(format!(
                "{}: task id {:?} must be alphanumeric, dash, or underscore",
                self.dir.display(),
                self.id
            )));
        }
        if self.prompt.trim().is_empty() {
            return Err(Error::Config(format!(
                "{}: empty prompt",
                self.dir.display()
            )));
        }
        if !self.verify_script().exists() {
            return Err(Error::Config(format!(
                "{}: missing {VERIFY_FILE}. A task that cannot be verified cannot be scored, \
                 and scoring from the model's own summary is how a harness reports passes \
                 for work that never happened.",
                self.dir.display()
            )));
        }
        Ok(())
    }

    pub fn seed_workspace(&self) -> PathBuf {
        self.dir.join(WORKSPACE_DIR)
    }

    pub fn verify_script(&self) -> PathBuf {
        self.dir.join(VERIFY_FILE)
    }

    /// Fold the task's limits into a config.
    ///
    /// Tightening only. A task may say "40 turns"; it may not say "400"
    /// because the operator asked for 60. The benchmark is allowed to be
    /// stricter than you, never more permissive.
    pub fn apply_limits(&self, cfg: &Config) -> Config {
        let mut cfg = cfg.clone();
        if let Some(t) = self.limits.max_turns {
            cfg.limits.max_turns = cfg.limits.max_turns.min(t);
        }
        if let Some(u) = self.limits.max_usd {
            cfg.limits.max_usd = cfg.limits.max_usd.min(u);
        }
        if let Some(ms) = self.limits.tool_timeout_ms {
            cfg.limits.tool_timeout_ms = cfg.limits.tool_timeout_ms.min(ms);
        }
        cfg
    }

    pub fn wall_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.limits.wall_timeout_secs.unwrap_or(900))
    }
}
