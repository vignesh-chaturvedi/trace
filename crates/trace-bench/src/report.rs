//! Turning an aggregate into something a human can act on.

use crate::result::Aggregate;

/// A plain-text summary.
///
/// Sigma is printed next to the pass rate, never below it or in a footnote,
/// because the two numbers only mean something together. A 4-point improvement
/// with a 6-point spread is not an improvement.
pub fn summary(agg: &Aggregate) -> String {
    let mut s = String::new();

    s.push_str(&format!("model          {}\n", agg.model));
    s.push_str(&format!(
        "tasks          {} x {} repeats = {} runs\n",
        agg.tasks, agg.repeats, agg.runs
    ));
    s.push_str(&format!(
        "pass rate      {:.1}%  (sigma {:.1} points across repeats)\n",
        agg.pass_rate * 100.0,
        agg.pass_rate_sigma * 100.0
    ));
    s.push_str(&format!("mean turns     {:.1}\n", agg.mean_turns));
    s.push_str(&format!(
        "tokens         {} in / {} out ({} cached)\n",
        agg.tokens.input, agg.tokens.output, agg.tokens.cached_input
    ));
    s.push_str(&format!(
        "cache hit      {:.1}%\n",
        agg.cache_hit_rate * 100.0
    ));
    s.push_str(&format!(
        "cost           ${:.4} total, ${:.4} per run\n",
        agg.total_usd, agg.usd_per_run
    ));

    if agg.repeats < 2 {
        s.push_str("\nNOTE  a single repeat has no spread. Treat this as a smoke test,\n");
        s.push_str("      not a measurement.\n");
    }

    if !agg.flaky_task_ids.is_empty() {
        s.push_str(&format!(
            "\nflaky ({})    {}\n",
            agg.flaky_task_ids.len(),
            agg.flaky_task_ids.join(", ")
        ));
        s.push_str("      these passed on some repeats and failed on others. Look at them\n");
        s.push_str("      before trusting the headline number.\n");
    }

    if agg.harness_errors > 0 {
        s.push_str(&format!(
            "\nWARNING  {} run(s) failed inside the harness and were excluded from the\n",
            agg.harness_errors
        ));
        s.push_str("         pass rate. The score above describes fewer runs than you asked\n");
        s.push_str("         for. Fix these before comparing against anything.\n");
    }

    s
}

/// Compare two sweeps.
///
/// The comparison Phase 1 exists to pass: is the new number within noise of
/// the old one? Answered against the combined spread rather than by eyeballing
/// the difference.
pub fn compare(baseline: &Aggregate, candidate: &Aggregate) -> String {
    let delta = candidate.pass_rate - baseline.pass_rate;
    // Sum in quadrature: the spread of a difference of two independent
    // measurements, not the spread of either one.
    let combined = (baseline.pass_rate_sigma.powi(2) + candidate.pass_rate_sigma.powi(2)).sqrt();

    let verdict = if combined == 0.0 && delta == 0.0 {
        "identical"
    } else if delta.abs() <= combined {
        "within noise"
    } else if delta > 0.0 {
        "improvement beyond noise"
    } else {
        "regression beyond noise"
    };

    format!(
        "baseline       {:.1}%  (sigma {:.1})\n\
         candidate      {:.1}%  (sigma {:.1})\n\
         delta          {:+.1} points\n\
         combined sigma {:.1} points\n\
         verdict        {verdict}\n\n\
         cost           ${:.4} -> ${:.4} per run\n\
         cache hit      {:.1}% -> {:.1}%\n",
        baseline.pass_rate * 100.0,
        baseline.pass_rate_sigma * 100.0,
        candidate.pass_rate * 100.0,
        candidate.pass_rate_sigma * 100.0,
        delta * 100.0,
        combined * 100.0,
        baseline.usd_per_run,
        candidate.usd_per_run,
        baseline.cache_hit_rate * 100.0,
        candidate.cache_hit_rate * 100.0,
    )
}
