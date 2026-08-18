//! Loading an API key from a `.env` file.
//!
//! The key itself never enters `Config`, the event log, or the context — only
//! the *name* of the variable holding it does (`model.api_key_env`). That is
//! deliberate: trajectories are meant to be published, committed, and fed into
//! a training pipeline, and a harness that writes a secret into the artifact it
//! exists to share is a harness with one very bad day ahead of it.
//! `tests/secrets.rs` asserts the key never reaches a log.

use std::path::Path;

use crate::error::{Error, Result};

/// Read `path` and export any variables it defines.
///
/// Returns the number of variables set. A missing file is not an error — most
/// runs get their key from the real environment, and requiring a `.env` would
/// break CI for no reason.
///
/// **Real environment variables always win.** A stale `.env` on a developer's
/// laptop must never quietly override the key CI injected; the surprising
/// direction of that precedence is how the wrong account gets billed.
///
/// Call this once, at the very start of `main`, before any threads exist:
/// `set_var` mutates process-global state and is only sound while the program
/// is still single-threaded.
pub fn load_dotenv(path: impl AsRef<Path>) -> Result<usize> {
    let path = path.as_ref();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(Error::io(path, e)),
    };

    let mut set = 0;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            return Err(Error::Config(format!(
                "{}:{}: expected KEY=VALUE, got {raw:?}",
                path.display(),
                i + 1
            )));
        };

        let key = key.trim();
        if key.is_empty() {
            return Err(Error::Config(format!(
                "{}:{}: empty variable name",
                path.display(),
                i + 1
            )));
        }

        if std::env::var_os(key).is_some() {
            continue;
        }

        std::env::set_var(key, unquote(value.trim()));
        set += 1;
    }

    Ok(set)
}

/// Strip one matching pair of surrounding quotes.
///
/// Keys are commonly pasted with quotes still attached, and an unstripped
/// quote produces a 401 that looks like a bad key rather than a bad file.
fn unquote(v: &str) -> &str {
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

/// Where a key is expected, and how to say so when it is missing.
pub fn missing_key_help(var: &str) -> String {
    format!(
        "no API key found.\n\n\
         Set {var} in one of:\n  \
           .env in this directory   (cp .env.example .env, then paste the key)\n  \
           the environment          (export {var}=sk-...)\n\n\
         No key is needed for: cargo test, trace replay, trace lint, trace inspect,\n\
         or any run with --fixture."
    )
}
