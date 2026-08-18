use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::event::{Body, Event, Seq};

/// Time, injected.
///
/// Not ceremony: crash-recovery and compaction tests need to place events at
/// chosen timestamps, and a real clock in a test is a flaky test waiting to
/// happen.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock that advances only when told to.
pub struct FixedClock(AtomicU64);

impl FixedClock {
    pub fn new(start_ms: u64) -> Self {
        FixedClock(AtomicU64::new(start_ms))
    }

    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct EventLog {
    path: PathBuf,
    file: File,
    session: String,
    next_seq: Seq,
    clock: Arc<dyn Clock>,
}

impl EventLog {
    /// Start a new log. Fails if one already exists, because silently
    /// appending to someone else's session is worse than an error.
    pub fn create(path: impl AsRef<Path>, session: impl Into<String>) -> Result<EventLog> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        Ok(EventLog {
            path,
            file,
            session: session.into(),
            next_seq: 1,
            clock: Arc::new(SystemClock),
        })
    }

    /// Reopen an existing log for resume.
    ///
    /// Repairs a torn tail first, then continues numbering from the last
    /// surviving event, so `seq` stays gapless across a crash.
    pub fn resume(path: impl AsRef<Path>) -> Result<(EventLog, Vec<Event>)> {
        let path = path.as_ref().to_path_buf();
        let outcome = super::reader::read_and_repair(&path)?;
        let session = outcome
            .events
            .first()
            .map(|e| e.session.clone())
            .ok_or_else(|| {
                Error::other(format!("{} has no events to resume from", path.display()))
            })?;
        let next_seq = outcome.events.last().map(|e| e.seq + 1).unwrap_or(1);
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        let log = EventLog {
            path,
            file,
            session,
            next_seq,
            clock: Arc::new(SystemClock),
        };
        Ok((log, outcome.events))
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn next_seq(&self) -> Seq {
        self.next_seq
    }

    /// Append one event.
    ///
    /// The line and its terminator go out in a single `write_all` so a torn
    /// write can only ever lose a suffix — exactly the shape the reader knows
    /// how to repair. `sync_data` runs for boundary events only.
    pub fn append(&mut self, body: Body) -> Result<Event> {
        let event = Event {
            seq: self.next_seq,
            ts_ms: self.clock.now_ms(),
            session: self.session.clone(),
            body,
        };

        let mut line = serde_json::to_string(&event)?;
        debug_assert!(
            !line.contains('\n'),
            "serde_json escapes newlines; one event must be one line"
        );
        line.push('\n');

        self.file
            .write_all(line.as_bytes())
            .map_err(|e| Error::io(&self.path, e))?;

        if event.body.needs_fsync() {
            self.file
                .sync_data()
                .map_err(|e| Error::io(&self.path, e))?;
        }

        self.next_seq += 1;
        Ok(event)
    }

    /// Force everything written so far to disk. Used before handing control to
    /// something that might not come back — a tool exec, or process exit.
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_data().map_err(|e| Error::io(&self.path, e))
    }
}
