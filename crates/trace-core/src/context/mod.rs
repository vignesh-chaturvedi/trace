//! Turning a log into the exact bytes sent to a model.
//!
//! Everything in this module is a pure function of `(events, config)`. That is
//! enforced three ways: by construction (no I/O is reachable from here), by
//! [`tests/determinism.rs`](../../tests/determinism.rs), which rewrites every
//! timestamp in a fixture and asserts the output is unchanged, and by
//! [`lint`], which fails the build if the cacheable prefix stops being stable.

pub mod build;
pub mod layout;
pub mod lint;
pub mod tokens;
pub mod truncate;

pub use build::{build_context, Context};
pub use layout::{stable_region, StableRegion};
pub use lint::{lint, Finding, Severity};
pub use truncate::{truncate, Truncated};
