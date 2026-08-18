//! A last look before anything leaves the machine.
//!
//! The bundle is written by someone who is not the repo owner and sent over
//! WhatsApp or email. If a credential is in it, the cost is not a wrong number
//! — it is their key, published, by a tool that told them it was safe.
//!
//! So this runs as a gate rather than a warning: the bundle is not written
//! if it trips. Heuristic, and deliberately biased toward false positives.
//! A refused bundle costs a minute of annoyance; a leaked one does not.
//!
//! This is the last of several layers, not the only one — tool processes
//! already run with a scrubbed environment and the log is redacted at the
//! writer. It exists because those layers are assumed to have gaps.

/// Provider key prefixes worth recognising by sight.
const KNOWN_PREFIXES: &[(&str, &str)] = &[
    ("sk-", "OpenAI-style secret key"),
    ("sk_live_", "live secret key"),
    ("sk_test_", "test secret key"),
    ("AIza", "Google API key"),
    ("ghp_", "GitHub personal access token"),
    ("gho_", "GitHub OAuth token"),
    ("github_pat_", "GitHub fine-grained token"),
    ("xoxb-", "Slack bot token"),
    ("xoxp-", "Slack user token"),
    ("AKIA", "AWS access key id"),
    ("ASIA", "AWS temporary access key id"),
    ("hf_", "Hugging Face token"),
    ("anthropic-", "Anthropic-style key"),
];

/// Words that make a nearby long string suspicious.
const CONTEXT_WORDS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "password",
    "passwd",
    "token",
    "bearer",
    "authorization",
    "credential",
    "private_key",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub line: usize,
    pub kind: String,
    /// A short, masked hint — never the value itself. Printing the secret to
    /// explain that a secret was found would be its own leak.
    pub hint: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {} ({})", self.line, self.kind, self.hint)
    }
}

/// Scan text for anything that looks like a credential.
pub fn scan(text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (n, line) in text.lines().enumerate() {
        let line_no = n + 1;

        // The bundle's own explanation of what it does not contain mentions
        // these words by necessity. Skipping it avoids a self-trip that would
        // teach people to pass --force.
        if line.contains("[redacted:") {
            continue;
        }

        for (prefix, kind) in KNOWN_PREFIXES {
            if let Some(at) = line.find(prefix) {
                let tail: String = line[at + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                // A bare prefix in prose is not a key; a long tail is.
                if tail.len() >= 12 {
                    findings.push(Finding {
                        line: line_no,
                        kind: (*kind).to_string(),
                        hint: format!("{prefix}...{} chars", tail.len()),
                    });
                }
            }
        }

        let lower = line.to_ascii_lowercase();
        if CONTEXT_WORDS.iter().any(|w| lower.contains(w)) {
            for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            {
                if token.len() >= 24 && looks_random(token) {
                    findings.push(Finding {
                        line: line_no,
                        kind: "high-entropy value next to a credential word".to_string(),
                        hint: format!("{} chars", token.len()),
                    });
                    break;
                }
            }
        }
    }

    findings.sort_by_key(|f| f.line);
    findings.dedup_by(|a, b| a.line == b.line && a.kind == b.kind);
    findings
}

/// Does this look like a key rather than an identifier or a hash?
///
/// Mixed case with digits is the signature of a generated credential. Content
/// hashes appear all over a bundle and are lowercase hex, so they are excluded
/// explicitly — flagging every hash would make the scan useless noise.
fn looks_random(token: &str) -> bool {
    let has_upper = token.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = token.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = token.chars().any(|c| c.is_ascii_digit());

    if token.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    has_upper && has_lower && has_digit
}

/// Human-readable refusal.
pub fn describe(findings: &[Finding]) -> String {
    let mut s =
        String::from("refusing to write the bundle: it appears to contain credentials.\n\n");
    for f in findings.iter().take(20) {
        s.push_str(&format!("  {f}\n"));
    }
    if findings.len() > 20 {
        s.push_str(&format!("  ... and {} more\n", findings.len() - 20));
    }
    s.push_str(
        "\nThis file was about to be sent to someone else. Check the lines above.\n\
         If they are false positives, re-run with --allow-suspected-secrets.\n",
    );
    s
}
