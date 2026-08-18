use std::fs::OpenOptions;
use std::path::Path;

use crate::error::{Error, Result};
use crate::event::Event;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    /// Byte offset the file was (or should be) truncated to.
    pub truncated_to: u64,
    pub dropped_bytes: usize,
    /// Sequence number of the last event that survived.
    pub last_valid_seq: Option<u64>,
}

#[derive(Debug)]
pub struct ReadOutcome {
    pub events: Vec<Event>,
    /// `Some` when the tail of the file was a partial write.
    pub repair: Option<Repair>,
}

impl ReadOutcome {
    pub fn warning(&self) -> Option<String> {
        self.repair.as_ref().map(|r| {
            format!(
                "recovered from torn write at seq {} ({} bytes dropped)",
                r.last_valid_seq
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<none>".into()),
                r.dropped_bytes
            )
        })
    }
}

/// Parse a log without modifying it.
///
/// A process killed mid-write leaves a partial final line. That is handled
/// here, once, so nothing downstream ever has to think about it. Damage
/// anywhere *other* than the final line is not a torn write — it means events
/// were lost from the middle, which no consumer can compensate for, so it is a
/// hard error.
pub fn read(path: impl AsRef<Path>) -> Result<ReadOutcome> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;

    let mut events: Vec<Event> = Vec::new();
    let mut offset = 0usize; // start of the line under consideration
    let mut last_complete_end = 0usize; // byte offset just past the last '\n'
    let mut line_no = 0usize;

    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let nl = rest.iter().position(|&b| b == b'\n');

        let (line, terminated, consumed) = match nl {
            Some(i) => (&rest[..i], true, i + 1),
            None => (rest, false, rest.len()),
        };

        line_no += 1;

        if line.is_empty() {
            offset += consumed;
            if terminated {
                last_complete_end = offset;
            }
            continue;
        }

        match serde_json::from_slice::<Event>(line) {
            Ok(ev) => {
                if !terminated {
                    // Complete JSON with no terminator: the object made it out
                    // but the newline did not. Salvage the event and let the
                    // repair path re-terminate the file.
                    let seq = ev.seq;
                    events.push(ev);
                    return finish(
                        path,
                        events,
                        Some(Repair {
                            truncated_to: bytes.len() as u64,
                            dropped_bytes: 0,
                            last_valid_seq: Some(seq),
                        }),
                    );
                }
                events.push(ev);
                offset += consumed;
                last_complete_end = offset;
            }
            Err(e) => {
                if terminated {
                    // Corruption in the middle of the file. Not recoverable by
                    // truncation, because everything after it is still valid
                    // and would be silently discarded.
                    return Err(Error::CorruptLog {
                        path: path.to_path_buf(),
                        line: line_no,
                        detail: e.to_string(),
                    });
                }
                let repair = Repair {
                    truncated_to: last_complete_end as u64,
                    dropped_bytes: bytes.len() - last_complete_end,
                    last_valid_seq: events.last().map(|e| e.seq),
                };
                return finish(path, events, Some(repair));
            }
        }
    }

    finish(path, events, None)
}

/// Parse a log and repair a torn tail in place.
///
/// Repair is idempotent: reading a repaired file reports no damage, so a
/// resume loop cannot chew away at a healthy log.
pub fn read_and_repair(path: impl AsRef<Path>) -> Result<ReadOutcome> {
    let path = path.as_ref();
    let outcome = read(path)?;

    let Some(repair) = &outcome.repair else {
        return Ok(outcome);
    };

    if repair.dropped_bytes > 0 {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| Error::io(path, e))?;
        file.set_len(repair.truncated_to)
            .map_err(|e| Error::io(path, e))?;
        file.sync_all().map_err(|e| Error::io(path, e))?;
    } else {
        // The last event survived but its newline did not. Re-terminate so the
        // next append does not concatenate onto it.
        //
        // Append mode, not `write` + `write_all`: a handle opened for writing
        // starts at offset 0, so writing the terminator there would overwrite
        // the first byte of the first event and turn a one-byte loss into a
        // corrupt ledger.
        use std::io::Write;
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| Error::io(path, e))?;
        file.write_all(b"\n").map_err(|e| Error::io(path, e))?;
        file.sync_all().map_err(|e| Error::io(path, e))?;
    }

    Ok(outcome)
}

/// Validate the sequence before handing events out.
///
/// A gap means events vanished from the middle of the ledger. Everything
/// downstream — replay, recovery, compaction provenance — assumes gapless
/// numbering, so this fails loudly rather than producing a plausible-looking
/// wrong answer.
fn finish(path: &Path, events: Vec<Event>, repair: Option<Repair>) -> Result<ReadOutcome> {
    for pair in events.windows(2) {
        if pair[1].seq != pair[0].seq + 1 {
            return Err(Error::SequenceGap {
                path: path.to_path_buf(),
                prev: pair[0].seq,
                next: pair[1].seq,
            });
        }
    }
    Ok(ReadOutcome { events, repair })
}
