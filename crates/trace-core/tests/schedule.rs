//! Test plan (W6): the parallel execution rules.
//!
//! ```text
//! reads (read, grep, ls)     -> parallel, always
//! writes to distinct paths   -> parallel
//! writes to the same path    -> sequential
//! bash                       -> sequential by default
//! ```

use trace_core::event::ToolCall;
use trace_core::message::JsonValue;
use trace_core::tools::schedule::plan;

fn call(name: &str, args: &[(&str, &str)]) -> ToolCall {
    ToolCall {
        id: format!("id-{name}"),
        name: name.into(),
        args: args
            .iter()
            .map(|(k, v)| (k.to_string(), JsonValue::Str(v.to_string())))
            .collect(),
    }
}

fn shape(calls: &[ToolCall]) -> Vec<Vec<usize>> {
    plan(calls).into_iter().map(|b| b.0).collect()
}

#[test]
fn reads_run_together() {
    let calls = vec![
        call("read", &[("path", "a.rs")]),
        call("grep", &[("pattern", "fn main")]),
        call("ls", &[("path", "src")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0, 1, 2]]);
}

#[test]
fn writes_to_distinct_paths_run_together() {
    let calls = vec![
        call("edit", &[("path", "a.rs")]),
        call("edit", &[("path", "b.rs")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0, 1]]);
}

#[test]
fn writes_to_the_same_path_are_sequential() {
    let calls = vec![
        call("edit", &[("path", "a.rs")]),
        call("edit", &[("path", "a.rs")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0], vec![1]]);
}

/// A bash command can do anything, including writing files nobody declared.
#[test]
fn bash_runs_alone() {
    let calls = vec![
        call("read", &[("path", "a.rs")]),
        call("bash", &[("cmd", "cargo test")]),
        call("read", &[("path", "b.rs")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0], vec![1], vec![2]]);
}

#[test]
fn consecutive_bash_calls_never_merge() {
    let calls = vec![
        call("bash", &[("cmd", "make")]),
        call("bash", &[("cmd", "make test")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0], vec![1]]);
}

/// A tool nobody classified is assumed to have side effects. The safe default
/// loses parallelism, not correctness.
#[test]
fn unknown_tools_are_treated_as_side_effecting() {
    let calls = vec![
        call("read", &[("path", "a.rs")]),
        call("some_new_tool", &[("x", "1")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0], vec![1]]);
}

/// A write that does not name its target is indistinguishable from an exec.
#[test]
fn writes_without_a_named_path_run_alone() {
    let calls = vec![
        call("read", &[("path", "a.rs")]),
        call("write", &[("contents", "hello")]),
    ];
    assert_eq!(shape(&calls), vec![vec![0], vec![1]]);
}

/// Batching must never reorder the model's calls.
#[test]
fn ordering_is_preserved() {
    let calls = vec![
        call("read", &[("path", "a.rs")]),
        call("edit", &[("path", "b.rs")]),
        call("bash", &[("cmd", "make")]),
        call("read", &[("path", "c.rs")]),
        call("edit", &[("path", "c.rs")]),
    ];

    let flat: Vec<usize> = plan(&calls).into_iter().flat_map(|b| b.0).collect();
    assert_eq!(flat, vec![0, 1, 2, 3, 4]);
}

#[test]
fn every_call_is_scheduled_exactly_once() {
    let calls: Vec<ToolCall> = (0..25)
        .map(|i| match i % 4 {
            0 => call("read", &[("path", "x.rs")]),
            1 => call("edit", &[("path", "y.rs")]),
            2 => call("bash", &[("cmd", "ls")]),
            _ => call("grep", &[("pattern", "z")]),
        })
        .collect();

    let mut flat: Vec<usize> = plan(&calls).into_iter().flat_map(|b| b.0).collect();
    flat.sort();
    assert_eq!(flat, (0..25).collect::<Vec<_>>());
}
