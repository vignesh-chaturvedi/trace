#![allow(dead_code)]

//! Shared test scaffolding.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use trace_core::config::Config;
use trace_core::event::Event;
use trace_core::provider::{Provider, Response, ScriptedProvider};
use trace_core::runtime::session::{Session, StartArgs};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch directory that removes itself.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("trace-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn test_config() -> Config {
    let mut cfg = Config::default();
    cfg.model.name = "test-model".into();
    cfg.model.price_in_per_mtok = 1.0;
    cfg.model.price_out_per_mtok = 4.0;
    cfg.model.price_cached_in_per_mtok = 0.1;
    cfg.limits.tool_timeout_ms = 10_000;
    cfg
}

/// Drive a full session from a script and return its log path plus events.
///
/// This is how fixtures come into existence: a scripted run produces a real
/// log, and that log is then the input to every replay and recovery test.
pub fn scripted_session(
    dir: &TempDir,
    cfg: &Config,
    task: &str,
    turns: Vec<Response>,
) -> (PathBuf, Vec<Event>) {
    let log_path = dir.join("session.jsonl");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let mut session = Session::start(
        cfg.clone(),
        StartArgs {
            log_path: &log_path,
            session_id: "s-test".into(),
            task: task.into(),
            workspace,
            agents_md: String::new(),
            harness_commit: "testcommit".into(),
        },
    )
    .expect("start session");

    let provider = ScriptedProvider::new(turns);
    let _ = session.run(&provider as &dyn Provider, &mut |_| {});

    let events = trace_core::log::read(&log_path).expect("read log").events;
    (log_path, events)
}
