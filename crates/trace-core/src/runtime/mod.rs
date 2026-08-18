//! Running a session, and surviving the ways one can go wrong.

pub mod checkpoint;
pub mod compaction;
pub mod guards;
pub mod recovery;
pub mod session;

pub use checkpoint::{rewind, RewindReport};
pub use compaction::{expand, provenance, should_compact, FLUSH_PROMPT};
pub use guards::{detect_doom_loop, fingerprint, DoomLoop};
pub use recovery::{find_orphans, Orphan};
pub use session::{new_session_id, RunReport, Session, StartArgs};
