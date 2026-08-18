//! The `trace` command line.
//!
//! Deliberately thin. Every subcommand is a short call into `trace-core`; if
//! the CLI ever needs something the library cannot do, the library is wrong,
//! not the CLI.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};

use trace_core::config::Config;
use trace_core::context::{build_context, lint};
use trace_core::event::{Body, Outcome};
use trace_core::log;
use trace_core::provider::{FixtureProvider, OpenAiProvider, Provider, RecordingProvider};
use trace_core::runtime::session::{new_session_id, Session, StartArgs};

#[derive(Parser)]
#[command(
    name = "trace",
    version,
    about = "A measurable, recoverable coding-agent runtime"
)]
struct Cli {
    /// Config file. Defaults to ./trace.toml when present.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a task.
    Run {
        task: String,
        /// Directory the agent works in.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "runs")]
        out: PathBuf,
        /// Record every model exchange for offline replay.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Replay from a fixture instead of calling a provider. No network.
        #[arg(long)]
        fixture: Option<PathBuf>,
        #[arg(long)]
        max_turns: Option<u64>,
        #[arg(long)]
        budget: Option<f64>,
    },

    /// Continue an interrupted session.
    Resume {
        log: PathBuf,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        fixture: Option<PathBuf>,
    },

    /// Rebuild every context in a log and check it against what was recorded.
    ///
    /// Offline and network-free. This is the phase's central claim, reduced to
    /// one command that either passes or names the turn where it stopped.
    Replay { log: PathBuf },

    /// Check that the cacheable prefix is actually stable.
    Lint {
        /// Also fail on warnings (cross-session cache sharing).
        #[arg(long)]
        strict: bool,
    },

    /// Summarize a session log.
    Inspect { log: PathBuf },

    /// Rebuild the session index for a directory of logs.
    Index {
        #[arg(default_value = "runs")]
        dir: PathBuf,
    },

    /// Restore the workspace to a checkpoint.
    Rewind {
        log: PathBuf,
        /// Sequence number of the checkpoint. Defaults to the most recent.
        #[arg(long)]
        seq: Option<u64>,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load_config(cli.config.as_deref())?;

    match cli.command {
        Command::Run {
            task,
            workspace,
            out,
            record,
            fixture,
            max_turns,
            budget,
        } => {
            let mut cfg = cfg;
            if let Some(n) = max_turns {
                cfg.limits.max_turns = n;
            }
            if let Some(b) = budget {
                cfg.limits.max_usd = b;
            }
            run(cfg, task, workspace, out, record, fixture)
        }
        Command::Resume {
            log,
            workspace,
            fixture,
        } => resume(cfg, &log, workspace, fixture),
        Command::Replay { log } => replay(cfg, &log),
        Command::Lint { strict } => run_lint(cfg, strict),
        Command::Inspect { log } => inspect(&log),
        Command::Index { dir } => {
            let rows = log::rebuild_index(&dir)?;
            println!(
                "indexed {} sessions -> {}",
                rows.len(),
                dir.join("index.jsonl").display()
            );
            Ok(())
        }
        Command::Rewind {
            log,
            seq,
            workspace,
        } => rewind(&log, seq, &workspace),
    }
}

fn load_config(path: Option<&Path>) -> Result<Config> {
    match path {
        Some(p) => Ok(Config::load(p).with_context(|| format!("loading {}", p.display()))?),
        None if Path::new("trace.toml").exists() => Ok(Config::load(Path::new("trace.toml"))?),
        None => Ok(Config::default()),
    }
}

fn provider_for(
    cfg: &Config,
    fixture: Option<PathBuf>,
    record: Option<PathBuf>,
) -> Result<Box<dyn Provider>> {
    if let Some(path) = fixture {
        return Ok(Box::new(FixtureProvider::load(&path)?));
    }
    let live = OpenAiProvider::from_env(&cfg.model.base_url, &cfg.model.api_key_env)?;
    match record {
        Some(path) => Ok(Box::new(RecordingProvider::new(live, &path)?)),
        None => Ok(Box::new(live)),
    }
}

fn run(
    cfg: Config,
    task: String,
    workspace: PathBuf,
    out: PathBuf,
    record: Option<PathBuf>,
    fixture: Option<PathBuf>,
) -> Result<()> {
    let findings = lint::lint(&cfg, &Default::default());
    if lint::has_errors(&findings) {
        for f in findings
            .iter()
            .filter(|f| f.severity == lint::Severity::Error)
        {
            eprintln!("trace: {f}");
        }
        bail!("layout lint failed; the cacheable prefix is not stable");
    }

    let session_id = new_session_id(now_ms(), std::process::id());
    let log_path = out.join(format!("{session_id}.jsonl"));

    let agents_md = std::fs::read_to_string(workspace.join("AGENTS.md")).unwrap_or_default();

    let mut session = Session::start(
        cfg.clone(),
        StartArgs {
            log_path: &log_path,
            session_id,
            task,
            workspace: workspace.canonicalize().unwrap_or(workspace.clone()),
            agents_md,
            harness_commit: harness_commit(),
        },
    )?;

    let provider = provider_for(&cfg, fixture, record)?;
    let report = session.run(provider.as_ref(), &mut |delta| {
        use std::io::Write;
        print!("{delta}");
        let _ = std::io::stdout().flush();
    })?;

    println!();
    println!("--");
    println!("outcome        {:?}", report.outcome);
    println!("turns          {}", report.turns);
    println!(
        "tokens         {} in / {} out ({} cached)",
        report.usage.input, report.usage.output, report.usage.cached_input
    );
    println!("cache hit      {:.1}%", report.cache_hit_rate * 100.0);
    println!("cost           ${:.4}", report.usd);
    println!("log            {}", log_path.display());

    std::process::exit(match report.outcome {
        Outcome::Done => 0,
        Outcome::Aborted => 2,
        Outcome::Error => 4,
    });
}

fn resume(
    cfg: Config,
    log_path: &Path,
    workspace: PathBuf,
    fixture: Option<PathBuf>,
) -> Result<()> {
    let mut session = Session::resume(cfg.clone(), log_path, workspace)?;
    let provider = provider_for(&cfg, fixture, None)?;
    let report = session.run(provider.as_ref(), &mut |d| print!("{d}"))?;
    println!(
        "\n-- resumed: {:?} after {} turns",
        report.outcome, report.turns
    );
    Ok(())
}

/// Rebuild every context and compare against the hash recorded at the time.
fn replay(cfg: Config, log_path: &Path) -> Result<()> {
    let outcome = log::read(log_path)?;
    if let Some(w) = outcome.warning() {
        eprintln!("trace: {w}");
    }
    let events = outcome.events;

    let mut checked = 0usize;
    let mut mismatched = 0usize;

    for ev in &events {
        let Body::ModelRequest(req) = &ev.body else {
            continue;
        };
        // The request was built from everything that existed before it.
        let ctx = build_context(&events, &cfg, ev.seq - 1);
        let got = ctx.hash();
        checked += 1;

        if got != req.context_hash {
            mismatched += 1;
            eprintln!(
                "seq {}: context diverged\n  recorded {}\n  rebuilt  {}",
                ev.seq, req.context_hash, got
            );
            if mismatched == 1 {
                eprintln!(
                    "  (the first mismatch localizes the impurity; later ones are downstream)"
                );
            }
        }
    }

    if mismatched == 0 {
        println!("replay ok: {checked} contexts byte-identical to the recorded run");
        Ok(())
    } else {
        bail!("{mismatched}/{checked} contexts diverged")
    }
}

fn run_lint(cfg: Config, strict: bool) -> Result<()> {
    let findings = lint::lint(&cfg, &Default::default());
    for f in &findings {
        println!("{f}");
    }

    if lint::has_errors(&findings) {
        bail!("layout lint failed");
    }
    if strict && !findings.is_empty() {
        bail!("layout lint failed (strict)");
    }
    println!("layout ok: the stable region holds across turns");
    Ok(())
}

fn inspect(log_path: &Path) -> Result<()> {
    let s = log::index::summarize(log_path)?;
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(())
}

fn rewind(log_path: &Path, seq: Option<u64>, workspace: &Path) -> Result<()> {
    let events = log::read(log_path)?.events;

    let ckpt = events
        .iter()
        .rev()
        .find_map(|e| match &e.body {
            Body::Checkpoint(c) if seq.is_none() || seq == Some(c.log_seq) => Some(c.clone()),
            _ => None,
        })
        .context("no matching checkpoint in this log")?;

    let report = trace_core::runtime::rewind(workspace, &ckpt)?;
    println!(
        "restored tracked files to {} (log seq {})",
        &report.restored[..12.min(report.restored.len())],
        report.log_seq
    );
    if report.untracked_drifted {
        println!(
            "note: ignored/untracked files differ from the checkpoint. They were not touched — \
             git never captured them, so removing them was not this command's call to make."
        );
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Recorded on every session. A score without a commit is a rumour.
fn harness_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
