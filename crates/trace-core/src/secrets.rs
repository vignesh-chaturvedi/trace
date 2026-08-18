//! Credentials: loading them, keeping them out of tool processes, and keeping
//! them out of the log.
//!
//! The key itself never enters `Config`, the event log, or the context — only
//! the *name* of the variable holding it does (`model.api_key_env`). That is
//! deliberate: trajectories are meant to be published, committed, and fed into
//! a training pipeline, and a harness that writes a secret into the artifact it
//! exists to share is a harness with one very bad day ahead of it.
//! `tests/secrets.rs` asserts the key never reaches a log.

use std::collections::BTreeMap;
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

/// Replaces known secret values with `[redacted:name]`.
///
/// **Runs at the writer, not at display time.** The distinction is the whole
/// point. Redacting in a viewer leaves the secret sitting in the JSONL on
/// disk, which is the file that gets committed, published, and fed to Phase 4
/// as training data. By the time anyone looks at a UI it is far too late.
///
/// Redaction is by *value*: anything matching a registered secret goes,
/// wherever it appears — a tool that echoed an environment variable, a curl
/// command with a token in the URL, a stack trace quoting a config.
#[derive(Default, Clone, Debug)]
pub struct Redactor {
    /// name -> value, longest value first so an overlapping shorter secret
    /// cannot mask a longer one.
    secrets: Vec<(String, String)>,
}

/// Below this length a "secret" matches too much ordinary text to be redacted
/// safely. A four-character token would turn every log line into confetti, and
/// a redactor people switch off protects nothing.
pub const MIN_SECRET_LEN: usize = 8;

impl Redactor {
    pub fn new() -> Redactor {
        Redactor::default()
    }

    /// Register a secret. Values shorter than [`MIN_SECRET_LEN`] are ignored.
    pub fn register(&mut self, name: impl Into<String>, value: impl Into<String>) -> bool {
        let value = value.into();
        if value.trim().len() < MIN_SECRET_LEN {
            return false;
        }
        self.secrets.push((name.into(), value));
        // Longest first: if one secret contains another, redacting the short
        // one first would leave a partially-redacted long one behind, which
        // still leaks its tail.
        self.secrets.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        true
    }

    /// Register every named environment variable that is set.
    pub fn from_env<I, S>(names: I) -> Redactor
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut r = Redactor::new();
        for name in names {
            let name = name.as_ref();
            if let Ok(value) = std::env::var(name) {
                r.register(name, value);
            }
        }
        r
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    /// Redact every registered secret from `text`.
    ///
    /// Also matches the JSON-escaped form, because this runs on a serialized
    /// line: a secret containing a quote or backslash appears on disk escaped,
    /// and matching only the raw form would sail straight past it.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (name, value) in &self.secrets {
            let marker = format!("[redacted:{name}]");
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &marker);
            }
            let escaped = json_escape(value);
            if escaped != *value && out.contains(&escaped) {
                out = out.replace(&escaped, &marker);
            }
        }
        out
    }

    /// Would `text` leak anything? Useful in tests and assertions.
    pub fn leaks(&self, text: &str) -> Option<&str> {
        self.secrets
            .iter()
            .find(|(_, v)| text.contains(v.as_str()) || text.contains(&json_escape(v)))
            .map(|(name, _)| name.as_str())
    }
}

/// The body of a JSON string literal, without the surrounding quotes.
fn json_escape(value: &str) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    encoded
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(&encoded)
        .to_string()
}

/// Environment variables a tool process is allowed to inherit.
///
/// Everything else is dropped. Tool processes get a scrubbed environment and
/// never see an API key: escape test 05 is "dump env and grep for
/// key-shaped strings", and the answer has to be that there is nothing there.
pub const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG", "LC_ALL", "TZ", "TMPDIR", "PWD",
];

/// Build a scrubbed environment from the current one.
///
/// Allowlist rather than denylist. A denylist has to enumerate every way a
/// credential can be named — `*_TOKEN`, `*_KEY`, `*_SECRET`, `AWS_*`, and
/// whatever the next vendor invents — and it is wrong the first time someone
/// picks a name nobody predicted.
pub fn scrubbed_env() -> BTreeMap<String, String> {
    ENV_ALLOWLIST
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}
