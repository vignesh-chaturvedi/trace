//! The policy engine: the first of two independent layers.
//!
//! Two things this module assumes about itself, both load-bearing:
//!
//! **The model is never the enforcement layer.** Nothing here asks the model
//! to behave. A policy decision is made from the request, before the tool
//! runs, and no wording in the transcript changes it. That is what makes the
//! prompt-injection test in Phase 3 a test of the *policy* rather than a test
//! of the model's willpower.
//!
//! **Policy is assumed buggy.** This layer will have gaps — it pattern-matches
//! shell text, and shell is not a language you can pattern-match soundly. The
//! OS layer underneath (Landlock, seccomp, Seatbelt) is what catches what this
//! misses. Neither layer is trusted to be complete on its own, which is why
//! they are independent rather than layered on the same mechanism.

pub mod glob;
pub mod path;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// What a rule says should happen.
///
/// Ordered so that `max()` implements the precedence rule directly:
/// deny beats prompt beats allow.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    #[default]
    Allow,
    /// Ask a human. In a headless run this is a denial, because there is
    /// nobody to ask and proceeding anyway would make the effect a lie.
    Prompt,
    Deny,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub effect: Effect,
    /// Tool name, an alternation like `edit|write`, or `*`.
    #[serde(default = "star")]
    pub tool: String,
    /// Path glob. `$WORKSPACE` and `$SESSION` are expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Hostnames, or `["*"]` for any network access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<Vec<String>>,
    /// Command-text globs, matched against the whole command string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,
    /// Why this rule exists. Shown to the model on a denial, so it can adapt
    /// instead of retrying the same blocked command until the turn cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn star() -> String {
    "*".to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Applied when no rule matches.
    ///
    /// Explicit rather than implied. A reader should not have to infer whether
    /// an unmatched request is permitted, and the two sensible answers differ
    /// enough (a dev laptop, a CI runner) that guessing serves neither.
    #[serde(default)]
    pub default: Effect,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// What is about to happen, described in the terms rules are written in.
#[derive(Clone, Debug, Default)]
pub struct Action {
    pub tool: String,
    /// Paths the action will touch, as far as the caller can tell.
    pub paths: Vec<PathBuf>,
    pub hosts: Vec<String>,
    pub command: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub effect: Effect,
    /// The rule that decided this, or `"default"`.
    pub rule: String,
    pub reason: String,
}

impl Decision {
    pub fn allowed(&self) -> bool {
        self.effect == Effect::Allow
    }
}

/// Values substituted into path patterns.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    pub workspace: PathBuf,
    pub session: String,
}

impl Policy {
    pub fn from_toml(src: &str) -> Result<Policy> {
        let policy: Policy =
            toml::from_str(src).map_err(|e| Error::Config(format!("policy: {e}")))?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn load(path: &Path) -> Result<Policy> {
        let src = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Policy::from_toml(&src).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }

    fn validate(&self) -> Result<()> {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for rule in &self.rules {
            if rule.id.trim().is_empty() {
                return Err(Error::Config("every rule needs an id".into()));
            }
            // Duplicate ids make a decision unattributable: the log names a
            // rule and two different rules answer to that name.
            *seen.entry(rule.id.as_str()).or_default() += 1;
        }
        if let Some((id, _)) = seen.iter().find(|(_, n)| **n > 1) {
            return Err(Error::Config(format!("duplicate rule id {id:?}")));
        }
        Ok(())
    }

    /// Decide what should happen.
    ///
    /// **The most specific matching rule wins; deny beats prompt beats allow
    /// among rules of equal specificity.**
    ///
    /// This is longest-prefix-match, the same rule a routing table uses, and
    /// it is a deliberate reading of the manual's "deny > prompt > allow".
    /// Taken literally, strongest-always-wins makes the manual's own example
    /// policy incoherent: `net-default` denies `net = ["*"]` and
    /// `net-registries` allows `pypi.org`, so a literal deny-always-wins
    /// leaves the allowlist as dead code that reads as though it works. An
    /// allowlist carved out of a broad denial is the entire point of writing
    /// one.
    ///
    /// The cost is real and worth stating: a narrow `allow` can override a
    /// broad `deny`. That is what makes allowlists expressible, and it is why
    /// this layer is not trusted alone — the OS layer underneath does not
    /// consult these rules at all.
    ///
    /// Specificity is compared per facet, path first, so a rule about
    /// `.git/hooks/**` outranks one about the whole workspace regardless of
    /// how the tool patterns happen to be spelled. Order affects nothing:
    /// reordering a policy file cannot change an outcome.
    pub fn evaluate(&self, action: &Action, vars: &Vars) -> Decision {
        let mut best: Option<(&Rule, Effect, Specificity)> = None;

        for rule in &self.rules {
            let Some(effect) = self.rule_applies(rule, action, vars) else {
                continue;
            };
            let spec = specificity(rule, vars);
            let wins = match &best {
                Some((_, current_effect, current_spec)) => {
                    (spec, effect) > (*current_spec, *current_effect)
                }
                None => true,
            };
            if wins {
                best = Some((rule, effect, spec));
            }
        }

        let best = best.map(|(r, e, _)| (r, e));

        match best {
            Some((rule, effect)) => Decision {
                effect,
                rule: rule.id.clone(),
                reason: rule
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("matched rule {}", rule.id)),
            },
            None => Decision {
                effect: self.default,
                rule: "default".into(),
                reason: format!("no rule matched; policy default is {:?}", self.default),
            },
        }
    }

    fn rule_applies(&self, rule: &Rule, action: &Action, vars: &Vars) -> Option<Effect> {
        if !glob::matches_alternation(&rule.tool, &action.tool) {
            return None;
        }

        // A rule constrains only the facets it names, and every named facet
        // must match. A rule naming a facet the action has nothing for does
        // not apply — a path rule says nothing about a request with no paths.
        let mut named_any = false;

        if let Some(pattern) = &rule.path {
            named_any = true;
            let expanded = expand(pattern, vars);
            let hit = action
                .paths
                .iter()
                .any(|p| glob::matches(&expanded, &path::for_matching(p)));
            if !hit {
                return None;
            }
        }

        if let Some(hosts) = &rule.net {
            named_any = true;
            let hit = action
                .hosts
                .iter()
                .any(|h| hosts.iter().any(|pattern| glob::matches_flat(pattern, h)));
            if !hit {
                return None;
            }
        }

        if let Some(patterns) = &rule.cmd {
            named_any = true;
            let command = action.command.as_deref().unwrap_or("");
            let hit = patterns.iter().any(|p| glob::matches_flat(p, command));
            if !hit {
                return None;
            }
        }

        // A rule naming only a tool applies to every action by that tool.
        let _ = named_any;
        Some(rule.effect)
    }
}

fn expand(pattern: &str, vars: &Vars) -> String {
    // The workspace is resolved, not pasted in verbatim. On macOS `/tmp` is a
    // symlink to `/private/tmp`, so an unresolved `$WORKSPACE` pattern would
    // never match the canonical paths it is compared against — a rule that
    // silently matches nothing.
    let workspace = path::resolve(&vars.workspace);
    pattern
        .replace("$WORKSPACE", &workspace.to_string_lossy())
        .replace("$SESSION", &vars.session)
}

/// A permissive starting point: everything allowed except the traps.
///
/// Ships as the default so the harness is usable out of the box, while still
/// closing the gaps that are pure downside — writing git hooks, editing the
/// policy that governs you, touching the harness that runs you.
pub fn baseline() -> Policy {
    Policy {
        default: Effect::Allow,
        rules: vec![
            Rule {
                id: "deny-git-hooks".into(),
                effect: Effect::Deny,
                tool: "*".into(),
                path: Some("$WORKSPACE/.git/hooks/**".into()),
                net: None,
                cmd: None,
                reason: Some(
                    "a hook written now executes later, in another process, outside this sandbox"
                        .into(),
                ),
            },
            Rule {
                id: "deny-policy-self-edit".into(),
                effect: Effect::Deny,
                tool: "*".into(),
                path: Some("**/policy/**".into()),
                net: None,
                cmd: None,
                reason: Some("a sandbox that can widen itself is not a sandbox".into()),
            },
            Rule {
                id: "deny-ssh-keys".into(),
                effect: Effect::Deny,
                tool: "*".into(),
                path: Some("**/.ssh/**".into()),
                net: None,
                cmd: None,
                reason: Some("credentials are brokered, never read from disk".into()),
            },
        ],
    }
}

/// How narrowly a rule is written, compared facet by facet.
///
/// Path first, because path is the facet policies are mostly about: a rule
/// naming `.git/hooks/**` should outrank one naming the whole workspace
/// whether or not their tool patterns happen to be the same length.
type Specificity = (usize, usize, usize, usize);

fn specificity(rule: &Rule, vars: &Vars) -> Specificity {
    let path = rule
        .path
        .as_ref()
        .map(|p| literal_len(&expand(p, vars)))
        .unwrap_or(0);

    // The *narrowest* alternative decides. A list is only as specific as the
    // most permissive thing it admits, so `["pypi.org", "*"]` must not
    // outrank `["pypi.org"]`.
    let net = rule
        .net
        .as_ref()
        .map(|v| v.iter().map(|s| literal_len(s)).min().unwrap_or(0))
        .unwrap_or(0);

    let cmd = rule
        .cmd
        .as_ref()
        .map(|v| v.iter().map(|s| literal_len(s)).min().unwrap_or(0))
        .unwrap_or(0);

    let tool = if rule.tool == "*" {
        0
    } else {
        literal_len(&rule.tool)
    };

    (path, net, cmd, tool)
}

/// Characters that actually constrain something. `*` and `?` do not.
fn literal_len(pattern: &str) -> usize {
    pattern.chars().filter(|c| !matches!(c, '*' | '?')).count()
}
