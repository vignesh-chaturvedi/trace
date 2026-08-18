# TRACE

A coding-agent runtime built so that every run is **measurable, recoverable, and trainable**.

Phase 1 (Runtime) is implemented. Written in Rust: the sandboxing Phase 3 calls for
(Landlock, seccomp) is native here rather than a native-addon fight, `serde` +
`BTreeMap` make the determinism invariants structural instead of merely tested, and
the whole thing ships as one static binary.

```bash
cargo build --release
cargo test --workspace
```

## The four ideas

**The log is the session.** State is not held in memory and checkpointed occasionally.
It is an append-only JSONL ledger, and everything else is derived from it. Flat text
rather than a database, because the log has to be greppable, diffable, tailable, and
streamable straight into a training pipeline. An index can be rebuilt if it rots; a
proprietary binary log cannot.

**Context is a pure function of the log.** `build_context(events, cfg, upto)` reads no
clock, no environment, no filesystem. Replaying a log therefore reproduces the exact
bytes that were sent — not an approximation of them. Every `model_request` event
records the hash of the context it sent, which is what turns replay from *probably
right* into *provably identical*.

**The prefix never moves.** System prompt, tool schemas, and `AGENTS.md` form a
byte-stable region that provider caches hit turn after turn. A lint fails the build if
anything volatile creeps in, because cache discipline erodes silently: someone adds a
"turns remaining" line, the hit rate goes to zero, and nothing in the code looks wrong.

**Losing work is a bug, not an accident.** A `tool_call` is durable *before* it runs
and its result is written *after*, so a crash leaves an answerable question rather than
a silently repeated `git push`.

## Commands

```bash
trace run "fix the failing test" --workspace ./repo
trace replay runs/s019xyz.jsonl     # rebuild every context, offline, no key
trace lint                          # is the cacheable prefix actually stable?
trace inspect runs/s019xyz.jsonl    # turns, tokens, cost, cache hit rate
trace index runs/                   # rebuild the session index
trace rewind runs/s019xyz.jsonl     # restore the workspace to a checkpoint
trace resume runs/s019xyz.jsonl     # continue after a crash
```

Exit codes carry meaning: `0` completed, `2` budget or turn cap, `4` harness error.

## Layout

```
crates/trace-core/
  event.rs            the ledger's schema; append-only, never mutated
  config.rs           an input to build_context, so part of the determinism contract
  log/
    writer.rs         monotonic seq, one event per line, fsync on boundary events
    reader.rs         torn-write repair; mid-file damage is an error, not a repair
    index.rs          rebuildable summaries (task, cost, cache hit rate)
  context/
    build.rs          build_context — the pure function at the centre
    layout.rs         the prefix-stable region
    lint.rs           the build-failing check that keeps it that way
    truncate.rs       middle-out truncation, applied at build time
    tokens.rs         cheap estimate; the provider's usage is ground truth
  provider/
    openai.rs         any OpenAI-compatible endpoint, streaming, usage accounting
    fixture.rs        record/replay keyed by context hash — the test suite's backend
    script.rs         drive a session deterministically to author the first fixture
  tools/
    bash.rs           stdin closed, stderr merged, process-group kill on timeout
    schema.rs         schemas that cannot be serialized out of order
    schedule.rs       what may run concurrently
  runtime/
    session.rs        the loop
    compaction.rs     flush turn, declared replacement, provenance
    checkpoint.rs     git_ref + log_seq + workspace_hash
    recovery.rs       orphaned calls are UNKNOWN, never re-executed
    guards.rs         doom-loop detection, budget, turn cap
crates/trace-cli/     a thin consumer of the above
```

## Two decisions worth knowing about

**Tool output is stored whole and truncated late.** The log keeps the complete output;
`truncate_limit` is applied when the context is built. So changing that limit and
replaying is a real offline ablation rather than a re-run, and the log never loses
what a future analysis might want.

**The reinforcement frame is derived, never logged.** It is computed by
`build_context` from the events themselves, so flipping `reinforce` and replaying the
same log is a clean A/B — the frame cannot leave residue in the trajectory it is being
measured against.

## Where the tests stand

82 tests, no network, no API key. The interesting ones:

| Claim | Test |
|---|---|
| Replay is byte-identical | `determinism.rs` — rebuilds every recorded `context_hash` |
| No clock reaches the context | `determinism.rs` — rewrites every timestamp, expects no change |
| Lint catches a planted timestamp | `layout.rs` |
| Schemas are byte-stable | `layout.rs` — 100 randomized key orders |
| The prefix holds for 50 turns | `layout.rs` — common-prefix bytes per turn pair |
| Compaction round-trips | `compaction.rs` — expand → original event set |
| Every kill point resumes | `recovery.rs` — truncates at **every byte offset**, not 20 random ones |
| Interrupted calls are not re-run | `recovery.rs` |
| Doom loop fires on repeats, not on flaky output | `guards.rs` |
| Budget aborts mid-stream, recorded | `guards.rs` |
| Checkpoints do not touch HEAD or the index | `checkpoint.rs` |

## Phase 1 exit criteria

| Criterion | Status |
|---|---|
| Byte-identical replay | Done — `trace replay`, enforced in CI by `determinism.rs` |
| Clean resume from any kill point | Done — exhaustive byte-offset sweep |
| Cache hit > 90% on a 50-turn session | **Harness half done.** Prefix stability across 50 turns is tested. The hit rate itself needs a live provider run — accounting is wired through `usage.cached_input` and reported by `trace inspect`. |
| Score within noise of P0 | **Blocked.** Phase 0 does not exist yet, so there is no baseline to compare against. |
| Writeup on prefix-stable design published | Not started (not code) |

## Not yet built

Phase 0's benchmark rig (Harbor adapter, sweeps, repeats) was never written, so there
is no baseline score. The P0 substrate that Phase 1 *wraps* — provider client, bash
tool, truncation — is here, but the measurement rig around it is not.

Phase 2 adds editing tools. The scheduler already classifies `read`/`grep`/`ls` and
`edit`/`write` for concurrency, so they slot in without rework; Phase 1 deliberately
ships bash-only so each tool added later carries a measured delta.
