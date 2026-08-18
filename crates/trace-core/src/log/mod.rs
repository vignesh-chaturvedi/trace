//! The session log: JSONL on disk, append-only, crash-tolerant.
//!
//! Flat text rather than a database, deliberately. The log has to be
//! greppable, diffable, tailable, and streamable straight into a training
//! pipeline. An index can be rebuilt if it rots; a proprietary binary log
//! cannot.

pub mod index;
pub mod reader;
pub mod writer;

pub use index::{rebuild_index, SessionSummary};
pub use reader::{read, read_and_repair, ReadOutcome, Repair};
pub use writer::{Clock, EventLog, FixedClock, SystemClock};
