//! Resolving a path to the thing it actually refers to.
//!
//! Every path is canonicalized **before** it is matched against a rule. Skip
//! this and the policy engine is decorative: `$WORKSPACE/../../etc/shadow`
//! reads as a workspace path to a naive matcher, and a symlink planted inside
//! the workspace points anywhere at all while still looking local.
//!
//! Escape tests 03 and 04 exist to keep this honest.

use std::path::{Component, Path, PathBuf};

/// Resolve a path to its canonical form, following symlinks.
///
/// `std::fs::canonicalize` only works on paths that already exist, which is
/// not good enough: a rule has to decide about a file the agent is about to
/// *create*. So the deepest existing ancestor is canonicalized for real, and
/// the remaining components are normalized lexically on top.
///
/// The asymmetry is deliberate and safe in the direction that matters. The
/// existing part is where symlinks can hide, and that part is fully resolved.
pub fn resolve(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }

    // Walk down from the root, resolving each component as it is added.
    //
    // Not up from the leaf. `Path::file_name()` returns `None` for a path
    // ending in `..`, so a walk-up loop silently *drops* the components that
    // constitute a traversal: `a/b/../../../etc/shadow` collapses to
    // `a/b/etc/shadow` and looks local. That is escape 04 succeeding while the
    // resolver reports everything is fine.
    //
    // Descending also gets symlinks right. Canonicalizing after each component
    // means a later `..` applies to the symlink's *target*, which is what the
    // kernel does.
    let mut out = PathBuf::new();

    for c in path.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                out.push(name);
                if let Ok(real) = std::fs::canonicalize(&out) {
                    out = real;
                }
            }
        }
    }

    out
}

/// Collapse `.` and `..` textually, without touching the filesystem.
///
/// Used only for paths with no existing ancestor. Not a substitute for
/// `resolve` — it cannot see symlinks, and a lexical normalizer applied to a
/// path containing one produces a confident wrong answer.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        push_component(&mut out, &c);
    }
    out
}

fn push_component(out: &mut PathBuf, c: &Component<'_>) {
    match c {
        Component::ParentDir => {
            out.pop();
        }
        Component::CurDir => {}
        other => out.push(other.as_os_str()),
    }
}

/// Is `path` inside `root`, after both are resolved?
pub fn is_within(root: &Path, path: &Path) -> bool {
    let root = resolve(root);
    let path = resolve(path);
    path.starts_with(&root)
}

/// Render a path for pattern matching: canonical, with forward slashes.
pub fn for_matching(path: &Path) -> String {
    let s = resolve(path).to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}
