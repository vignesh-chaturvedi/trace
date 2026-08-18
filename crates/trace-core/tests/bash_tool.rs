//! The bash tool's three non-optional behaviours.

mod common;

use std::time::{Duration, Instant};

use common::TempDir;

use trace_core::tools::bash::run_bash;

#[test]
fn captures_output_and_exit_code() {
    let dir = TempDir::new("bash-basic");
    let out = run_bash("echo hello; exit 3", dir.path(), Duration::from_secs(10)).unwrap();
    assert_eq!(out.output.trim(), "hello");
    assert_eq!(out.exit_code, 3);
    assert!(!out.timed_out);
}

/// The model needs errors interleaved with output in the order they actually
/// happened. Two pipes reassembled afterwards do not reproduce that.
#[test]
fn stderr_is_interleaved_with_stdout() {
    let dir = TempDir::new("bash-stderr");
    let out = run_bash(
        "echo one; echo two >&2; echo three",
        dir.path(),
        Duration::from_secs(10),
    )
    .unwrap();

    let lines: Vec<&str> = out.output.lines().collect();
    assert_eq!(lines, vec!["one", "two", "three"]);
}

/// An interactive command waiting on a prompt is the single most common way an
/// agent run hangs forever.
#[test]
fn stdin_is_closed() {
    let dir = TempDir::new("bash-stdin");
    let started = Instant::now();
    let out = run_bash(
        "read -r line; echo \"got:$line\"",
        dir.path(),
        Duration::from_secs(10),
    )
    .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "read blocked on stdin"
    );
    assert!(out.output.contains("got:"));
    assert!(!out.timed_out);
}

#[test]
fn runs_in_the_given_workspace() {
    let dir = TempDir::new("bash-cwd");
    std::fs::write(dir.join("marker.txt"), "x").unwrap();
    let out = run_bash("ls", dir.path(), Duration::from_secs(10)).unwrap();
    assert!(out.output.contains("marker.txt"));
}

#[test]
fn a_slow_command_is_killed_at_the_timeout() {
    let dir = TempDir::new("bash-timeout");
    let started = Instant::now();
    let out = run_bash("sleep 30", dir.path(), Duration::from_millis(300)).unwrap();

    assert!(out.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the timeout did not fire"
    );
    assert!(out.output.contains("timed out"));
}

/// The reason the child gets its own process group. Killing only the direct
/// child leaves its spawned workers holding the pipe, so the read never
/// reaches EOF and the timeout hangs anyway.
#[test]
fn the_whole_process_group_dies_with_the_timeout() {
    let dir = TempDir::new("bash-pgroup");
    let started = Instant::now();

    let out = run_bash(
        "sleep 30 & sleep 30 & wait",
        dir.path(),
        Duration::from_millis(300),
    )
    .unwrap();

    assert!(out.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "orphaned children held the pipe open for {:?}",
        started.elapsed()
    );
}

/// Output larger than the pipe buffer must not deadlock the wait loop.
#[test]
fn large_output_does_not_deadlock() {
    let dir = TempDir::new("bash-big");
    let out = run_bash(
        "for i in $(seq 1 20000); do echo \"line $i of output\"; done",
        dir.path(),
        Duration::from_secs(30),
    )
    .unwrap();

    assert!(!out.timed_out);
    assert_eq!(out.exit_code, 0);
    assert!(out.output.len() > 300_000, "got {} bytes", out.output.len());
    assert!(out.output.contains("line 20000 of output"));
}
