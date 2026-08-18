//! Tool declarations, serialized deterministically.
//!
//! This file is where the manual's "silent killer" lives or dies: tool schemas
//! serialized from a hash map with non-deterministic key order produce
//! identical *content* and different *bytes* every turn, which means a zero
//! percent cache hit rate and nothing in the code that looks wrong.
//!
//! The defence is structural rather than disciplinary. Schemas are built from
//! [`JsonValue`], whose only map type is a `BTreeMap`, so there is no way to
//! express an unordered schema in the first place.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::message::JsonValue;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
}

/// How the scheduler is allowed to run a tool concurrently with others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    /// Observes the workspace without changing it.
    Read,
    /// Changes a specific path, named by an argument.
    Write,
    /// Arbitrary side effects. Sequential unless explicitly marked pure.
    Exec,
}

pub fn obj(pairs: impl IntoIterator<Item = (&'static str, JsonValue)>) -> JsonValue {
    let map: BTreeMap<String, JsonValue> =
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    JsonValue::Object(map)
}

pub fn s(v: &str) -> JsonValue {
    JsonValue::Str(v.to_string())
}

pub fn arr(items: impl IntoIterator<Item = JsonValue>) -> JsonValue {
    JsonValue::Array(items.into_iter().collect())
}

pub const BASH: &str = "bash";

/// Phase 1 keeps the control group's single tool.
///
/// Not an oversight — every tool added is a variable in the ablation P4 will
/// run, so tools arrive in Phase 2 with a measured delta attached to each. The
/// scheduler below is already general enough to take them.
pub fn registry() -> Vec<ToolSchema> {
    let mut tools = vec![ToolSchema {
        name: BASH.to_string(),
        description: "Run a bash command in the workspace. Use it for everything: \
                      reading files, searching, editing, running tests, installing \
                      dependencies. Output is truncated in the middle if long."
            .to_string(),
        parameters: obj([
            ("type", s("object")),
            (
                "properties",
                obj([(
                    "cmd",
                    obj([
                        ("type", s("string")),
                        ("description", s("The command to run.")),
                    ]),
                )]),
            ),
            ("required", arr([s("cmd")])),
        ]),
    }];

    // Sorted by name so the serialized block is stable regardless of the order
    // registration happens to occur in.
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

/// The exact bytes of the tool block that enter the cacheable prefix.
pub fn schemas_json(tools: &[ToolSchema]) -> String {
    serde_json::to_string(tools).expect("tool schemas are always serializable")
}

/// Concurrency class for a tool call.
///
/// Unknown names are treated as `Exec`: the safe default is "assume it has
/// side effects", so a tool added without updating this table loses
/// parallelism rather than correctness.
pub fn kind_of(name: &str) -> ToolKind {
    match name {
        "read" | "grep" | "ls" | "glob" => ToolKind::Read,
        "edit" | "write" | "patch" | "apply_patch" => ToolKind::Write,
        _ => ToolKind::Exec,
    }
}

/// The path a write tool targets, if it names one.
pub fn write_target(args: &BTreeMap<String, JsonValue>) -> Option<&str> {
    for key in ["path", "file_path", "file"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            return Some(v);
        }
    }
    None
}
