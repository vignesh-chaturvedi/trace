//! The container adapter, exercised against a real runtime.
//!
//! These skip when no runtime is up, so the suite still passes on a laptop
//! without one. That convenience is also a trap: a skipped test reports "ok",
//! and a suite that is entirely skipped looks identical to a suite that
//! passed.
//!
//! So set `TRACE_REQUIRE_CONTAINER=1` and skipping becomes a hard failure.
//! CI should always set it. Without that, "all green" here means nothing more
//! than "it compiled".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use trace_core::config::Config;
use trace_core::provider::{Provider, ScriptedProvider};

use trace_bench::adapter::Adapter;
use trace_bench::container::{ContainerAdapter, ContainerConfig, MOUNT};
use trace_bench::sweep::{run_sweep, SweepOptions};
use trace_bench::Task;

static N: AtomicU32 = AtomicU32::new(0);

fn runtime_up() -> bool {
    trace_core::tools::exec::runtime_available("docker").is_ok()
}

/// Is a runtime mandatory in this environment?
fn runtime_required() -> bool {
    std::env::var("TRACE_REQUIRE_CONTAINER").is_ok_and(|v| v != "0" && !v.is_empty())
}

macro_rules! need_runtime {
    () => {
        if !runtime_up() {
            assert!(
                !runtime_required(),
                "TRACE_REQUIRE_CONTAINER is set but no container runtime is reachable. \
                 Refusing to report a pass for a test that did not run."
            );
            eprintln!("SKIP (no container runtime): {}", module_path!());
            return;
        }
    };
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Scratch {
        // Deliberately not `std::env::temp_dir()`. On macOS that is
        // /var/folders/..., which the container VM does not share, so every
        // bind mount would come up empty. `target/` sits inside the repo and
        // therefore inside the home directory the VM does share.
        let p = target_dir().join(format!(
            "ctr-scratch/{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    repo_root().join("target")
}

fn tasks_root() -> PathBuf {
    repo_root().join("tasks")
}

fn cfg() -> Config {
    let mut c = Config::default();
    c.model.name = "scripted".into();
    c.limits.tool_timeout_ms = 60_000;
    c
}

fn adapter() -> ContainerAdapter {
    ContainerAdapter::new(ContainerConfig {
        image: "python:3.12-slim".into(),
        ..Default::default()
    })
    .expect("start adapter")
}

/// The cache fix, stated as a test: the agent's working directory is the same
/// string on every attempt, so the cacheable prefix never moves between them.
#[test]
fn the_workdir_is_a_fixed_mount_point() {
    need_runtime!();
    let dir = Scratch::new("mount");
    let a = adapter();

    let ws1 = dir.path().join("one");
    let ws2 = dir.path().join("two");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();

    let e1 = a.executor(&ws1).expect("container 1");
    let e2 = a.executor(&ws2).expect("container 2");

    assert_eq!(e1.workdir(), MOUNT);
    assert_eq!(e2.workdir(), MOUNT);
    assert_eq!(
        e1.workdir(),
        e2.workdir(),
        "two attempts must present the same cwd, or they cannot share a cache prefix"
    );

    a.cleanup(&ws1);
    a.cleanup(&ws2);
}

#[test]
fn commands_run_inside_the_container_and_see_the_mount() {
    need_runtime!();
    let dir = Scratch::new("exec");
    let a = adapter();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("hello.txt"), "from the host\n").unwrap();

    let exec = a.executor(&ws).expect("container");
    let out = exec
        .run("pwd && cat hello.txt", std::time::Duration::from_secs(60))
        .unwrap();

    assert!(out.output.contains(MOUNT), "wrong cwd: {}", out.output);
    assert!(
        out.output.contains("from the host"),
        "mount not visible: {}",
        out.output
    );
    assert_eq!(out.exit_code, 0);

    // Writes flow back to the host side of the bind mount, which is how
    // verification and checkpoints still work.
    exec.run(
        "echo 'from the container' > back.txt",
        std::time::Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.join("back.txt")).unwrap().trim(),
        "from the container"
    );

    a.cleanup(&ws);
}

/// A task that can reach the internet can download the answer.
#[test]
fn the_network_is_off_by_default() {
    need_runtime!();
    let dir = Scratch::new("net");
    let a = adapter();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let exec = a.executor(&ws).expect("container");
    let out = exec
        .run(
            "python3 -c \"import socket; socket.create_connection(('1.1.1.1', 53), timeout=5)\"",
            std::time::Duration::from_secs(60),
        )
        .unwrap();

    assert_ne!(
        out.exit_code, 0,
        "the container reached the network: {}",
        out.output
    );
    a.cleanup(&ws);
}

/// Whatever the agent does inside the container, the host stays untouched.
#[test]
fn host_files_outside_the_mount_are_unreachable() {
    need_runtime!();
    let dir = Scratch::new("isolation");
    let a = adapter();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let canary = dir.path().join("canary.txt");
    std::fs::write(&canary, "do not touch\n").unwrap();

    let exec = a.executor(&ws).expect("container");
    let _ = exec.run(
        &format!("rm -f {} ; ls ..", canary.display()),
        std::time::Duration::from_secs(60),
    );

    assert!(
        canary.exists(),
        "the container deleted a host file outside the mount"
    );
    assert_eq!(std::fs::read_to_string(&canary).unwrap(), "do not touch\n");

    a.cleanup(&ws);
}

/// The silent-empty-mount trap, pinned.
#[test]
fn an_unshared_host_path_fails_loudly_instead_of_mounting_nothing() {
    need_runtime!();
    // macOS temp lives outside what the VM shares; on Linux it is shared and
    // the mount genuinely works, so there is nothing to assert there.
    if !cfg!(target_os = "macos") {
        return;
    }

    let a = adapter();
    let unshared = std::env::temp_dir().join(format!("trace-unshared-{}", std::process::id()));
    std::fs::create_dir_all(&unshared).unwrap();

    // `Arc<dyn Executor>` is not Debug, so match rather than unwrap_err.
    let err = match a.executor(&unshared) {
        Ok(_) => panic!(
            "an unshared path produced a working container; the empty-mount trap is unguarded"
        ),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("not visible inside the container"), "{err}");
    assert!(
        err.contains("file-sharing") || err.contains("shared path"),
        "{err}"
    );

    let _ = std::fs::remove_dir_all(&unshared);
}

#[test]
fn cleanup_removes_the_container() {
    need_runtime!();
    let dir = Scratch::new("cleanup");
    let a = adapter();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    let exec = a.executor(&ws).expect("container");
    assert!(exec.run("true", std::time::Duration::from_secs(30)).is_ok());

    a.cleanup(&ws);

    // The same workspace no longer has a container behind it.
    let out = exec
        .run("true", std::time::Duration::from_secs(30))
        .unwrap();
    assert_ne!(out.exit_code, 0, "the container survived cleanup");
}

/// End to end: a real sweep, in containers, scored inside the image the work
/// happened in.
#[test]
fn a_full_sweep_runs_in_containers() {
    need_runtime!();
    let dir = Scratch::new("sweep");
    let task = Task::load(&tasks_root().join("fix-sum")).unwrap();

    let turns = vec![
        ScriptedProvider::bash(
            "c1",
            "sed -i.bak 's/total - n/total + n/' sum.sh && bash sum.sh",
        ),
        ScriptedProvider::say("Fixed; sum.sh prints 108."),
    ];

    let report = run_sweep(
        &[task],
        &cfg(),
        &adapter(),
        &|_: &Config| Ok(Box::new(ScriptedProvider::new(turns.clone())) as Box<dyn Provider>),
        &SweepOptions {
            repeats: 3,
            limit: None,
            out_dir: dir.path().join("bench"),
            harness_commit: "ctrtest".into(),
            verbose: false,
        },
    )
    .unwrap();

    assert!(
        report.rows.iter().all(|r| r.passed),
        "container sweep did not pass: {:?}",
        report.rows
    );

    // And every attempt recorded the same cwd, which is the whole point.
    for row in &report.rows {
        let events = trace_core::log::read(Path::new(&row.trajectory))
            .unwrap()
            .events;
        let cwd = events[0].as_session_start().unwrap().cwd.clone();
        assert_eq!(cwd, MOUNT, "attempt recorded a varying cwd: {cwd}");
    }
}
