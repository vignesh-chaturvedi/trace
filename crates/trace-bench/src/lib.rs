//! The benchmark rig.
//!
//! Phase 1 made every run measurable. This is what does the measuring.
//!
//! The rig is small; the discipline around it is the point. A benchmark that
//! scores from the agent's own summary, reuses a workspace between repeats, or
//! quietly relaxes a timeout will produce numbers — they just will not mean
//! anything, and you will not find out until someone else fails to reproduce
//! them.

pub mod adapter;
pub mod bundle;
pub mod container;
pub mod report;
pub mod result;
pub mod scan;
pub mod sweep;
pub mod task;

pub use adapter::{Adapter, LocalAdapter, Verdict};
pub use bundle::Bundle;
pub use container::{ContainerAdapter, ContainerConfig, MOUNT};
pub use result::{aggregate, Aggregate, TaskResult};
pub use sweep::{run_sweep, SweepOptions, SweepReport, MIN_REPEATS};
pub use task::Task;
