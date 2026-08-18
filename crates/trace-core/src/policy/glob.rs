//! Glob matching for policy patterns.
//!
//! Hand-rolled rather than pulled from a crate, because the exact semantics
//! are a security boundary and they need to be readable in one sitting:
//!
//! * `*` matches any run of characters **within one path segment**
//! * `**` matches any number of segments, including none
//! * `?` matches one character
//! * everything else is literal
//!
//! The segment-awareness of `*` is the part that matters. A matcher where `*`
//! silently crosses `/` turns `$WORKSPACE/*` into a rule that also permits
//! `$WORKSPACE/../../etc/shadow`, and the rule still reads correct.

/// Does `text` match `pattern`?
pub fn matches(pattern: &str, text: &str) -> bool {
    match_from(pattern.as_bytes(), text.as_bytes())
}

/// Match with `/` treated as an ordinary character.
///
/// For non-path fields — tool names, hostnames, command text — where segment
/// semantics would be surprising rather than protective.
pub fn matches_flat(pattern: &str, text: &str) -> bool {
    match_flat(pattern.as_bytes(), text.as_bytes())
}

fn match_from(p: &[u8], t: &[u8]) -> bool {
    // `**` — consume any number of segments, including none.
    if p.starts_with(b"**") {
        let rest = strip_double_star(p);

        // `**` alone, or trailing, matches everything remaining.
        if rest.is_empty() {
            return true;
        }
        // Try the tail against this position and every subsequent segment
        // boundary.
        if match_from(rest, t) {
            return true;
        }
        for (i, c) in t.iter().enumerate() {
            if *c == b'/' && match_from(rest, &t[i + 1..]) {
                return true;
            }
        }
        return false;
    }

    match (p.first(), t.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(b'*'), _) => {
            // Single star: anything but a separator.
            if match_from(&p[1..], t) {
                return true;
            }
            let mut i = 0;
            while i < t.len() && t[i] != b'/' {
                i += 1;
                if match_from(&p[1..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        (Some(b'?'), Some(c)) if *c != b'/' => match_from(&p[1..], &t[1..]),
        (Some(a), Some(b)) if a == b => match_from(&p[1..], &t[1..]),
        _ => false,
    }
}

/// Skip `**` and the separator that usually follows it, so `a/**/b` also
/// matches `a/b`.
fn strip_double_star(p: &[u8]) -> &[u8] {
    let rest = &p[2..];
    if rest.first() == Some(&b'/') {
        &rest[1..]
    } else {
        rest
    }
}

fn match_flat(p: &[u8], t: &[u8]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(b'*'), _) => {
            if match_flat(&p[1..], t) {
                return true;
            }
            for i in 0..t.len() {
                if match_flat(&p[1..], &t[i + 1..]) {
                    return true;
                }
            }
            false
        }
        (Some(b'?'), Some(_)) => match_flat(&p[1..], &t[1..]),
        (Some(a), Some(b)) if a == b => match_flat(&p[1..], &t[1..]),
        _ => false,
    }
}

/// An alternation like `edit|write`, or `*` for any.
pub fn matches_alternation(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    pattern.split('|').any(|p| matches_flat(p.trim(), text))
}
