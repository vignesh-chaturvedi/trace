//! Recorded provider responses, replayed offline.
//!
//! Keyed by the request hash, which is the same hash recorded on every
//! `model_request` event. That is what makes a fixture faithful rather than
//! approximate: if replay produces a byte-identical context, it finds the
//! recorded response; if it does not, the lookup misses and the test fails
//! loudly instead of quietly diverging.
//!
//! Every test in this crate runs against this, so `cargo test` needs no
//! network and no API key.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::{Flow, Provider, Request, Response};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Recording {
    pub context_hash: String,
    pub response: Response,
}

pub struct FixtureProvider {
    /// Multiple recordings may share a hash when a session legitimately
    /// revisits an identical context; they are replayed in the order captured.
    by_hash: BTreeMap<String, Vec<Response>>,
    cursor: RefCell<BTreeMap<String, usize>>,
    strict: bool,
}

impl FixtureProvider {
    pub fn new(recordings: Vec<Recording>) -> Self {
        let mut by_hash: BTreeMap<String, Vec<Response>> = BTreeMap::new();
        for r in recordings {
            by_hash.entry(r.context_hash).or_default().push(r.response);
        }
        FixtureProvider {
            by_hash,
            cursor: RefCell::new(BTreeMap::new()),
            strict: true,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let mut recordings = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            recordings.push(serde_json::from_str(line).map_err(|e| Error::CorruptLog {
                path: path.to_path_buf(),
                line: i + 1,
                detail: e.to_string(),
            })?);
        }
        Ok(FixtureProvider::new(recordings))
    }

    /// Reuse the last recording for a hash instead of failing when it runs
    /// out. Useful when exploring, dangerous in tests — a lenient fixture will
    /// happily hide the divergence it exists to detect.
    pub fn lenient(mut self) -> Self {
        self.strict = false;
        self
    }
}

impl Provider for FixtureProvider {
    fn complete(
        &self,
        req: &Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> Flow,
    ) -> Result<Response> {
        let hash = req.hash();
        let Some(list) = self.by_hash.get(&hash) else {
            return Err(Error::Provider(format!(
                "no recorded response for context {}: the replayed context does not match \
                 anything captured. Some input to build_context has drifted.",
                &hash[..16.min(hash.len())]
            )));
        };

        let mut cursor = self.cursor.borrow_mut();
        let n = cursor.entry(hash.clone()).or_insert(0);
        let idx = if *n < list.len() {
            let i = *n;
            *n += 1;
            i
        } else if self.strict {
            return Err(Error::Provider(format!(
                "fixture for context {} exhausted after {} responses",
                &hash[..16.min(hash.len())],
                list.len()
            )));
        } else {
            list.len() - 1
        };

        let resp = list[idx].clone();
        let _ = on_delta(&resp.message.content);
        Ok(resp)
    }
}

/// Wraps a live provider and writes every exchange to a fixture file.
///
/// Recording is a side effect of a normal run, so fixtures stay current
/// without anyone maintaining them by hand.
pub struct RecordingProvider<P: Provider> {
    inner: P,
    out: RefCell<std::fs::File>,
}

impl<P: Provider> RecordingProvider<P> {
    pub fn new(inner: P, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| Error::io(path, e))?;
        Ok(RecordingProvider {
            inner,
            out: RefCell::new(file),
        })
    }
}

impl<P: Provider> Provider for RecordingProvider<P> {
    fn complete(
        &self,
        req: &Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> Flow,
    ) -> Result<Response> {
        let response = self.inner.complete(req, on_delta)?;
        let rec = Recording {
            context_hash: req.hash(),
            response: response.clone(),
        };

        use std::io::Write;
        let mut file = self.out.borrow_mut();
        let line = serde_json::to_string(&rec)?;
        let _ = writeln!(file, "{line}");
        let _ = file.flush();

        Ok(response)
    }
}
