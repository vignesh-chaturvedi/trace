//! TRACE — a coding-agent runtime built so that every run is measurable,
//! recoverable, and trainable.
//!
//! The load-bearing ideas, in one place:
//!
//! * **The log is the session.** State is not held in memory and checkpointed
//!   occasionally; it is an append-only ledger from which all state is
//!   derived. [`log`]
//! * **Context is a pure function of the log.** `build_context(events, cfg,
//!   upto)` reads no clock, no environment, and no filesystem, so replaying a
//!   log reproduces the exact bytes that were sent. [`context`]
//! * **The prefix never moves.** System prompt, tool schemas, and AGENTS.md
//!   form a byte-stable region that provider caches can hit turn after turn,
//!   and a lint fails the build if anything volatile creeps in.
//!   [`context::lint`]
//! * **Losing work is a bug, not an accident.** Tool calls are durable before
//!   they execute, so a crash leaves an answerable question rather than a
//!   silently repeated `git push`. [`runtime::recovery`]

pub mod config;
pub mod context;
pub mod error;
pub mod event;
pub mod hash;
pub mod log;
pub mod message;
pub mod policy;
pub mod provider;
pub mod runtime;
pub mod secrets;
pub mod tools;

pub use config::Config;
pub use error::{Error, Result};
pub use event::{Body, Event, Seq};
pub use message::{Message, Role};
