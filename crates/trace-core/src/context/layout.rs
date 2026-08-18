//! The prefix-stable region.
//!
//! Providers cache on exact prefix match: any byte that changes near the front
//! invalidates everything after it. So the front of every request is assembled
//! here, from session-constant inputs only, and never from anything that
//! varies turn to turn.
//!
//! The invariant, stated so it can be tested:
//!
//! ```text
//! For any session S and any two consecutive turns i, j:
//!   common_prefix_bytes(ctx_i, ctx_j) >= bytes(stable_region)
//! ```
//!
//! `{cwd}` is the sole substitution, and it is fixed for the life of a
//! session. Everything else volatile — turn counts, budget remaining, clock —
//! belongs at the tail, where it costs nothing.

use crate::config::Config;
use crate::event::SessionStart;
use crate::tools::schema::{registry, schemas_json};

pub struct StableRegion {
    /// The system message content.
    pub system: String,
    /// The serialized tool block sent alongside it.
    pub tools_json: String,
}

impl StableRegion {
    pub fn bytes(&self) -> usize {
        self.system.len() + self.tools_json.len()
    }
}

pub const AGENTS_HEADER: &str = "\n\n--- repository conventions (AGENTS.md) ---\n";

pub fn stable_region(cfg: &Config, start: &SessionStart) -> StableRegion {
    let mut system = cfg.prompt.system.replace("{cwd}", &start.cwd);

    if !start.agents_md.trim().is_empty() {
        system.push_str(AGENTS_HEADER);
        system.push_str(start.agents_md.trim_end());
    }

    StableRegion {
        system,
        tools_json: schemas_json(&registry()),
    }
}
