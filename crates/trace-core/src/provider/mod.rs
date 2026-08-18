//! Talking to a model.
//!
//! The trait is narrow on purpose. Everything above it — the loop, the log,
//! compaction, guards — works against `Provider`, so the fixture
//! implementation is not a test double bolted on afterwards but a first-class
//! backend. That is what lets the entire test suite run with no network and no
//! key, and what will let P4 point the same runtime at a self-hosted
//! fine-tune by changing one URL.

pub mod fixture;
pub mod openai;
pub mod script;

use crate::error::Result;
use crate::event::Usage;
use crate::hash::hash_bytes;
use crate::message::Message;

pub use fixture::{FixtureProvider, Recording, RecordingProvider};
pub use openai::OpenAiProvider;
pub use script::ScriptedProvider;

pub struct Request<'a> {
    pub model: &'a str,
    pub temperature: f64,
    pub messages: &'a [Message],
    /// Serialized tool block. Already byte-stable; passed through verbatim so
    /// nothing between here and the wire can reorder it.
    pub tools_json: &'a str,
}

impl Request<'_> {
    /// Identity of this request. Matches `Context::hash`, so a fixture
    /// recorded from a live run is keyed by exactly the context that produced
    /// it.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self.messages).expect("messages always serialize");
        hash_bytes(&[bytes.as_slice(), self.tools_json.as_bytes()].concat())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub message: Message,
    pub usage: Usage,
    pub stop_reason: String,
}

/// What the caller wants the stream to do next.
///
/// Returning `Stop` is how the budget guard aborts *mid-stream* rather than
/// discovering after the fact that a single runaway response blew the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

pub trait Provider {
    /// Run one completion. `on_delta` receives text fragments as they stream
    /// in and decides whether to keep going.
    ///
    /// Streaming matters even when nothing is watching: it is how you notice a
    /// run has gone wrong at minute two instead of minute twenty.
    fn complete(
        &self,
        req: &Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> Flow,
    ) -> Result<Response>;
}
