//! Every task must prove two things about itself.
//!
//! **The seed fails.** A task whose verification already passes before anyone
//! touches it scores every agent as correct, including one that did nothing.
//! One of those in a set silently inflates every number you report.
//!
//! **The solution passes.** A task nobody can solve reads as a model that
//! cannot do the work, and there is no way to tell the difference from the
//! outside. The reference fix is never shown to an agent; it exists so the
//! set can be checked rather than trusted.
//!
//! Run on the host with plain bash — no model, no network, no container.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use trace_bench::task::Task;

static N: AtomicU32 = AtomicU32::new(0);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// A throwaway copy of a task's seed workspace.
fn stage(task: &Task, tag: &str) -> PathBuf {
    let dir = repo_root().join("target").join(format!(
        "task-hygiene/{}-{tag}-{}-{}",
        task.id,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create staging dir");

    copy_dir(&task.seed_workspace(), &dir);
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("read seed workspace") {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.metadata().unwrap().is_dir() {
            std::fs::create_dir_all(&dst).unwrap();
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// Run a script inside a staged workspace and return (exit code, output).
fn run(script: &Path, workspace: &Path) -> (i32, String) {
    let local = workspace.join(".script.sh");
    std::fs::copy(script, &local).expect("stage script");

    let out = Command::new("bash")
        .arg(".script.sh")
        .current_dir(workspace)
        .output()
        .expect("run script");

    let _ = std::fs::remove_file(&local);
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn all_tasks() -> Vec<Task> {
    Task::load_all(&repo_root().join("tasks")).expect("load tasks")
}

#[test]
fn every_task_fails_before_it_is_fixed() {
    for task in all_tasks() {
        let ws = stage(&task, "seed");
        let (code, out) = run(&task.verify_script(), &ws);
        assert_ne!(
            code, 0,
            "task {:?} passes verification on its untouched seed, so it scores \
             a do-nothing agent as correct:\n{out}",
            task.id
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}

#[test]
fn every_task_passes_once_it_is_fixed() {
    for task in all_tasks() {
        if !task.has_solution() {
            continue;
        }
        let ws = stage(&task, "solved");

        let (code, out) = run(&task.solution_script(), &ws);
        assert_eq!(code, 0, "solution for {:?} did not run:\n{out}", task.id);

        let (code, out) = run(&task.verify_script(), &ws);
        assert_eq!(
            code, 0,
            "task {:?} is not solvable by its own reference fix, so it would \
             read as a model failure that no model could avoid:\n{out}",
            task.id
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}

/// A task with no reference fix cannot be checked at all.
#[test]
fn every_task_ships_a_reference_solution() {
    let missing: Vec<String> = all_tasks()
        .into_iter()
        .filter(|t| !t.has_solution())
        .map(|t| t.id)
        .collect();

    assert!(
        missing.is_empty(),
        "these tasks have no solution.sh, so nothing proves they are solvable: {missing:?}"
    );
}

/// Verification must not depend on anything the agent could have deleted, and
/// must not leave debris in the workspace it graded.
#[test]
fn verification_cleans_up_after_itself() {
    for task in all_tasks() {
        let ws = stage(&task, "debris");
        let before = listing(&ws);
        let _ = run(&task.verify_script(), &ws);
        let after = listing(&ws);

        assert_eq!(
            before, after,
            "verifying {:?} left files behind; a later stage would see them",
            task.id
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The set has to span a difficulty range, or it measures one thing.
#[test]
fn the_set_covers_a_difficulty_range() {
    use trace_bench::task::Difficulty;

    let tasks = all_tasks();
    for level in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
        assert!(
            tasks.iter().any(|t| t.difficulty == level),
            "no {level:?} tasks in the set"
        );
    }
    assert!(
        tasks.len() >= 8,
        "only {} tasks; too few to average",
        tasks.len()
    );
}

/// Tasks must be quick enough that a sweep is a feedback loop.
#[test]
fn verification_is_fast() {
    for task in all_tasks() {
        let ws = stage(&task, "timing");
        let started = std::time::Instant::now();
        let _ = run(&task.verify_script(), &ws);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(30),
            "verifying {:?} took {elapsed:?}; a sweep runs this hundreds of times",
            task.id
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}
