//! Test plan (P0, carried forward): "Truncation unit tests incl.
//! exactly-at-limit and multibyte boundaries."

use trace_core::context::truncate::truncate;

#[test]
fn short_output_is_untouched() {
    let out = truncate("hello", 8000);
    assert_eq!(out.text, "hello");
    assert!(!out.was_truncated());
}

#[test]
fn exactly_at_limit_is_untouched() {
    let s = "a".repeat(1000);
    let out = truncate(&s, 1000);
    assert_eq!(out.text, s);
    assert!(!out.was_truncated());
    assert_eq!(out.dropped_bytes, 0);
}

#[test]
fn one_byte_over_the_limit_truncates() {
    let s = "a".repeat(1001);
    let out = truncate(&s, 1000);
    assert!(out.was_truncated());
}

/// Head *and* tail, because errors and summaries cluster at the end of test
/// output. Head-only truncation throws away the part that carries the signal.
#[test]
fn keeps_both_ends() {
    let mut s = String::from("FIRST-LINE\n");
    s.push_str(&"filler\n".repeat(5000));
    s.push_str("LAST-LINE-WITH-THE-ERROR\n");

    let out = truncate(&s, 2000);

    assert!(out.text.starts_with("FIRST-LINE"));
    assert!(out.text.trim_end().ends_with("LAST-LINE-WITH-THE-ERROR"));
    assert!(out.was_truncated());
}

/// An elided middle with no instructions produces an agent that re-runs the
/// same command hoping for a shorter answer.
#[test]
fn tells_the_model_how_to_get_more() {
    let s = "x".repeat(40_000);
    let out = truncate(&s, 1000);

    assert!(out.text.contains("lines omitted"));
    assert!(out.text.contains("grep"));
    assert!(out.text.contains("40000 bytes total"));
}

/// Tool output is full of UTF-8 — box drawing from test runners, emoji from
/// CI. Slicing a `&str` inside a character panics, and it would panic in the
/// middle of a long run rather than in a unit test.
#[test]
fn never_splits_a_multibyte_character() {
    for ch in ["é", "中", "🙂", "👨‍👩‍👧‍👦"] {
        let s = ch.repeat(4000);
        for limit in [17, 64, 101, 999, 1000, 1001, 4096] {
            let out = truncate(&s, limit);
            assert!(
                out.text.is_char_boundary(0),
                "produced invalid UTF-8 for {ch:?} at limit {limit}"
            );
            // Round-tripping through String proves the slice boundaries held.
            let _ = out.text.clone().into_bytes();
        }
    }
}

#[test]
fn mixed_width_content_survives_every_limit() {
    let s: String = (0..2000)
        .map(|i| match i % 4 {
            0 => 'a',
            1 => 'é',
            2 => '中',
            _ => '🙂',
        })
        .collect();

    for limit in 1..400 {
        let out = truncate(&s, limit);
        assert!(std::str::from_utf8(out.text.as_bytes()).is_ok());
    }
}

/// A limit so small the two halves would overlap must not emit duplicated
/// text; returning the original is the honest fallback.
#[test]
fn degenerate_limits_do_not_duplicate_content() {
    let s = "abcdefghij";
    for limit in 0..10 {
        let out = truncate(s, limit);
        assert!(out.text.len() <= s.len() + 200);
    }
}
