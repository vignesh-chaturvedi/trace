//! The bash tool.
//!
//! Three details here are not optional, and each of them costs a day to
//! diagnose if you skip it:
//!
//! * **stdin is `/dev/null`.** An interactive command waiting on a prompt is
//!   the single most common way an agent run hangs forever. Nothing can ask
//!   the agent a question.
//! * **stderr is merged at the shell.** The model needs errors interleaved
//!   with output in the order they actually happened; two separate pipes
//!   reassembled afterwards do not reproduce that.
//! * **the child gets its own process group, and the timeout kills the
//!   group.** Killing only the direct child leaves `npm test`'s spawned
//!   workers holding the pipe, so the read never reaches EOF and the timeout
//!   you carefully implemented hangs anyway.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct BashOutcome {
    pub exit_code: i32,
    /// Complete, untruncated. Truncation is a context-build concern.
    pub output: String,
    pub wall_ms: u64,
    pub timed_out: bool,
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

pub fn run_bash(cmd: &str, cwd: &Path, timeout: Duration) -> Result<BashOutcome> {
    let started = Instant::now();

    // `-c`, never `-lc`: a login shell sources the user's profile, which makes
    // the tool's behaviour depend on whose machine it runs on.
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(format!("exec 2>&1\n{cmd}"))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|e| Error::other(format!("failed to spawn bash: {e}")))?;

    let pid = child.id() as i32;
    let mut stdout = child.stdout.take().expect("stdout was piped");

    // Drain the pipe on its own thread. A command that writes more than the
    // pipe buffer (64k on Linux) blocks forever if nobody is reading while we
    // wait for exit.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    kill_group(pid);
                    break child
                        .wait()
                        .map_err(|e| Error::other(format!("wait after kill failed: {e}")))?;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(Error::other(format!("wait failed: {e}"))),
        }
    };

    let bytes = reader.join().unwrap_or_default();
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if timed_out {
        output.push_str(&format!(
            "\n[timed out after {}ms; process group killed]",
            timeout.as_millis()
        ));
    }

    Ok(BashOutcome {
        exit_code: exit_code(&status),
        output,
        wall_ms: started.elapsed().as_millis() as u64,
        timed_out,
    })
}

#[cfg(unix)]
fn kill_group(pid: i32) {
    // SIGKILL rather than SIGTERM: this path only runs after the command has
    // already blown its deadline, and a process that ignores TERM would sit
    // there until the next timeout.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: i32) {}

#[cfg(unix)]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(-1))
}

#[cfg(not(unix))]
fn exit_code(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}
