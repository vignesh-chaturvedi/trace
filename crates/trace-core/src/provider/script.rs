//! A provider that reads from a script instead of a model.
//!
//! Public rather than test-only. Anything built on this runtime needs a way to
//! drive a session deterministically — a fixture is keyed by context hash and
//! so cannot be written before the contexts exist, which makes it useless for
//! authoring the *first* run. A script closes that gap: write the turns you
//! want, run the session, and the resulting log becomes the fixture.

use std::cell::RefCell;

use crate::error::{Error, Result};
use crate::event::Usage;
use crate::message::{JsonValue, Message, Role, ToolCallRef};

use super::{Flow, Provider, Request, Response};

pub struct ScriptedProvider {
    turns: Vec<Response>,
    cursor: RefCell<usize>,
    /// Bytes per streamed delta. Small values exercise the streaming path and
    /// the mid-stream guards properly.
    chunk: usize,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<Response>) -> Self {
        ScriptedProvider {
            turns,
            cursor: RefCell::new(0),
            chunk: 16,
        }
    }

    pub fn with_chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk.max(1);
        self
    }

    /// A turn that says something and stops.
    pub fn say(text: &str) -> Response {
        Response {
            message: Message::assistant(text),
            usage: Usage {
                input: 1000,
                output: 20,
                cached_input: 0,
            },
            stop_reason: "stop".into(),
        }
    }

    /// A turn that runs one bash command.
    pub fn bash(id: &str, cmd: &str) -> Response {
        Response {
            message: Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![ToolCallRef {
                    id: id.to_string(),
                    name: "bash".to_string(),
                    args: [("cmd".to_string(), JsonValue::Str(cmd.to_string()))]
                        .into_iter()
                        .collect(),
                    extra: None,
                }],
                tool_call_id: None,
            },
            usage: Usage {
                input: 1000,
                output: 20,
                cached_input: 900,
            },
            stop_reason: "tool_calls".into(),
        }
    }
}

impl Provider for ScriptedProvider {
    fn complete(
        &self,
        _req: &Request<'_>,
        on_delta: &mut dyn FnMut(&str) -> Flow,
    ) -> Result<Response> {
        let mut cursor = self.cursor.borrow_mut();
        let response =
            self.turns.get(*cursor).cloned().ok_or_else(|| {
                Error::Provider(format!("script exhausted after {} turns", *cursor))
            })?;
        *cursor += 1;
        drop(cursor);

        // Stream in chunks so callers that watch the stream — the mid-stream
        // budget guard in particular — see the same shape a real provider
        // produces.
        let content = response.message.content.as_bytes();
        let mut at = 0usize;
        while at < content.len() {
            let mut end = (at + self.chunk).min(content.len());
            while end < content.len() && !response.message.content.is_char_boundary(end) {
                end += 1;
            }
            if on_delta(&response.message.content[at..end]) == Flow::Stop {
                break;
            }
            at = end;
        }

        Ok(response)
    }
}
